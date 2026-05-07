/// TAP v2 Receipt Aggregate Voucher (RAV) — mirrors IGraphTallyCollector.ReceiptAggregateVoucher.
///
/// The gateway (payer) signs a RAV to acknowledge it owes the cumulative value to
/// the service provider. The provider submits the signed RAV on-chain to collect.
///
/// EIP-712 type:
///   ReceiptAggregateVoucher(bytes32 collectionId, address payer, address serviceProvider,
///     address dataService, uint64 timestampNs, uint128 valueAggregate, bytes metadata)
use alloy_primitives::{keccak256, Address, Bytes, B256};
use alloy_sol_types::SolValue;
use k256::ecdsa::SigningKey;
use serde::{Deserialize, Serialize};

use crate::eip712::{address_from_key, eip712_hash_raw};
use crate::sign::SignError;

pub const RAV_TYPE_STRING: &str =
    "ReceiptAggregateVoucher(bytes32 collectionId,address payer,address serviceProvider,address dataService,uint64 timestampNs,uint128 valueAggregate,bytes metadata)";

/// Receipt Aggregate Voucher — the gateway's signed commitment to pay.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Rav {
    pub collection_id: B256,
    pub payer: Address,
    pub service_provider: Address,
    pub data_service: Address,
    pub timestamp_ns: u64,
    /// Cumulative total owed since the beginning of the payer↔provider relationship.
    pub value_aggregate: u128,
    #[serde(default)]
    pub metadata: Bytes,
}

/// A signed RAV ready for on-chain redemption.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedRav {
    pub rav: Rav,
    /// Hex-encoded 65-byte ECDSA signature: r(32) || s(32) || v(1).
    pub signature: String,
}

/// Canonical collection ID for a (payer, serviceProvider, dataService) triple.
/// `keccak256(abi.encode(payer, serviceProvider, dataService))`
pub fn collection_id(payer: Address, service_provider: Address, data_service: Address) -> B256 {
    let encoded = (payer, service_provider, data_service).abi_encode();
    keccak256(&encoded)
}

/// Compute the EIP-712 struct hash for a RAV.
pub fn rav_struct_hash(rav: &Rav) -> B256 {
    let type_hash = keccak256(RAV_TYPE_STRING.as_bytes());
    let metadata_hash = keccak256(&rav.metadata);
    let encoded = (
        type_hash,
        rav.collection_id,
        rav.payer,
        rav.service_provider,
        rav.data_service,
        rav.timestamp_ns,
        rav.value_aggregate,
        metadata_hash,
    )
        .abi_encode();
    keccak256(&encoded)
}

/// Sign a RAV with the gateway's signing key and the pre-computed domain separator.
pub fn sign_rav(
    signing_key: &SigningKey,
    domain_sep: B256,
    rav: Rav,
) -> Result<SignedRav, SignError> {
    let hash = eip712_hash_raw(domain_sep, rav_struct_hash(&rav));
    let (sig, rec_id) = signing_key.sign_prehash_recoverable(hash.as_slice())?;
    let mut sig_bytes = [0u8; 65];
    sig_bytes[..64].copy_from_slice(&sig.to_bytes());
    sig_bytes[64] = rec_id.to_byte() + 27;
    Ok(SignedRav {
        rav,
        signature: format!("0x{}", hex::encode(sig_bytes)),
    })
}

/// Derive the signer address from a signing key (convenience wrapper).
pub fn signer_address(key: &SigningKey) -> Address {
    address_from_key(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eip712::{domain_separator, recover_signer};

    fn test_key() -> SigningKey {
        SigningKey::from_slice(&[0x42u8; 32]).unwrap()
    }

    #[test]
    fn sign_and_recover_rav_signer() {
        let key = test_key();
        let expected_signer = address_from_key(&key);
        let dom = domain_separator("GraphTallyCollector", 42161, Address::ZERO);

        let payer = expected_signer;
        let sp: Address = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
            .parse()
            .unwrap();
        let ds: Address = "0x1000000000000000000000000000000000000001"
            .parse()
            .unwrap();

        let rav = Rav {
            collection_id: collection_id(payer, sp, ds),
            payer,
            service_provider: sp,
            data_service: ds,
            timestamp_ns: 1_000_000_000,
            value_aggregate: 5_000_000_000_000,
            metadata: Bytes::default(),
        };

        let signed = sign_rav(&key, dom, rav).unwrap();
        let hash = eip712_hash_raw(dom, rav_struct_hash(&signed.rav));
        let recovered = recover_signer(hash, &signed.signature).unwrap();
        assert_eq!(recovered, expected_signer);
    }

    #[test]
    fn collection_id_deterministic() {
        let (p, sp, ds) = (
            Address::from([1u8; 20]),
            Address::from([2u8; 20]),
            Address::from([3u8; 20]),
        );
        assert_eq!(collection_id(p, sp, ds), collection_id(p, sp, ds));
    }

    #[test]
    fn collection_id_sensitive_to_order() {
        let (a, b, c) = (
            Address::from([1u8; 20]),
            Address::from([2u8; 20]),
            Address::from([3u8; 20]),
        );
        assert_ne!(collection_id(a, b, c), collection_id(b, a, c));
    }
}
