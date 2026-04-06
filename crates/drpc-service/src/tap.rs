//! TAP v2 (GraphTally) receipt validation.
//!
//! Receipts are transmitted as JSON in the `TAP-Receipt` HTTP header.
//! Each receipt is an EIP-712 signed message. We recover the signer and
//! check it against the authorised sender list.
//!
//! Full spec: GIP-0054 / GraphTally — <https://github.com/graphprotocol/graph-improvement-proposals>

use alloy_primitives::{keccak256, Address, Bytes, B256, U256};
use alloy_sol_types::SolValue;
use k256::ecdsa::{RecoveryId, Signature as K256Sig, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::error::ServiceError;

// ---------------------------------------------------------------------------
// TAP v2 Receipt types
// These mirror the on-chain Solidity structs in GraphTallyCollector.
// ---------------------------------------------------------------------------

/// EIP-712 type string for Receipt.
/// Must match exactly what GraphTallyCollector uses on-chain.
pub const RECEIPT_TYPE_STRING: &str =
    "Receipt(address data_service,address service_provider,uint64 timestamp_ns,uint64 nonce,uint128 value,bytes metadata)";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedReceipt {
    pub receipt: Receipt,
    /// Hex-encoded 65-byte ECDSA signature: r(32) || s(32) || v(1).
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub data_service: Address,
    pub service_provider: Address,
    pub timestamp_ns: u64,
    pub nonce: u64,
    pub value: u128,
    #[serde(default)]
    pub metadata: Bytes,
}

// ---------------------------------------------------------------------------
// Domain separator
// ---------------------------------------------------------------------------

/// Compute the EIP-712 domain separator for GraphTallyCollector.
///
/// Should be computed once at startup and stored in AppState.
pub fn domain_separator(name: &str, chain_id: u64, verifying_contract: Address) -> B256 {
    let type_hash = keccak256(
        b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
    );
    let name_hash = keccak256(name.as_bytes());
    let version_hash = keccak256(b"1");

    let encoded = (
        type_hash,
        name_hash,
        version_hash,
        U256::from(chain_id),
        verifying_contract,
    )
        .abi_encode();

    keccak256(&encoded)
}

// ---------------------------------------------------------------------------
// Validated receipt
// ---------------------------------------------------------------------------

/// A receipt that has passed all validation checks.
/// Carries the recovered signer address and raw signature for DB persistence.
pub struct ValidatedReceipt {
    pub receipt: Receipt,
    pub signer: Address,
    pub signature: String,
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Decode and validate a TAP receipt from the `TAP-Receipt` HTTP header value.
///
/// Checks:
/// - Receipt is parseable JSON
/// - `data_service` matches our contract address
/// - `service_provider` matches our indexer address
/// - Receipt is not older than `max_age_ns`
/// - Signature is valid and signer is in `authorized_senders`
pub fn validate_receipt(
    header_value: &str,
    domain_sep: B256,
    authorized_senders: &[Address],
    data_service: Address,
    service_provider: Address,
    max_age_ns: u64,
    now_ns: u64,
) -> Result<ValidatedReceipt, ServiceError> {
    let signed: SignedReceipt = serde_json::from_str(header_value)
        .map_err(|e| ServiceError::InvalidReceipt(e.to_string()))?;

    let r = &signed.receipt;

    if r.data_service != data_service {
        return Err(ServiceError::InvalidReceipt(format!(
            "data_service mismatch: expected {data_service}, got {}",
            r.data_service
        )));
    }

    if r.service_provider != service_provider {
        return Err(ServiceError::InvalidReceipt(format!(
            "service_provider mismatch: expected {service_provider}, got {}",
            r.service_provider
        )));
    }

    if now_ns.saturating_sub(r.timestamp_ns) > max_age_ns {
        return Err(ServiceError::ReceiptExpired);
    }

    let msg_hash = eip712_hash(domain_sep, r)?;
    let signer = recover_signer(msg_hash, &signed.signature)
        .map_err(|e| ServiceError::InvalidReceipt(format!("signature recovery failed: {e}")))?;

    if !authorized_senders.contains(&signer) {
        return Err(ServiceError::UnauthorizedSender(signer.to_string()));
    }

    Ok(ValidatedReceipt {
        receipt: signed.receipt,
        signer,
        signature: signed.signature,
    })
}

// ---------------------------------------------------------------------------
// EIP-712 internals
// ---------------------------------------------------------------------------

fn eip712_hash(domain_sep: B256, receipt: &Receipt) -> Result<B256, ServiceError> {
    let struct_hash = receipt_struct_hash(receipt);

    // EIP-712: keccak256(0x1901 || domainSeparator || structHash)
    let mut buf = [0u8; 66];
    buf[0] = 0x19;
    buf[1] = 0x01;
    buf[2..34].copy_from_slice(domain_sep.as_slice());
    buf[34..66].copy_from_slice(struct_hash.as_slice());

    Ok(keccak256(buf))
}

fn receipt_struct_hash(r: &Receipt) -> B256 {
    let type_hash = keccak256(RECEIPT_TYPE_STRING.as_bytes());
    let metadata_hash = keccak256(&r.metadata);

    // ABI-encode (typehash, data_service, service_provider, timestamp_ns, nonce, value, keccak256(metadata))
    // Dynamic `bytes` field is replaced by its keccak256 hash per EIP-712 spec.
    let encoded = (
        type_hash,
        r.data_service,
        r.service_provider,
        r.timestamp_ns,
        r.nonce,
        r.value,
        metadata_hash,
    )
        .abi_encode();

    keccak256(&encoded)
}

fn recover_signer(hash: B256, sig_hex: &str) -> anyhow::Result<Address> {
    let bytes = hex::decode(sig_hex.trim_start_matches("0x"))?;
    anyhow::ensure!(bytes.len() == 65, "signature must be 65 bytes, got {}", bytes.len());

    let v = bytes[64];
    let rec_id = RecoveryId::from_byte(v % 2)
        .ok_or_else(|| anyhow::anyhow!("invalid recovery id {v}"))?;

    let sig = K256Sig::from_slice(&bytes[..64])?;
    let vk = VerifyingKey::recover_from_prehash(hash.as_slice(), &sig, rec_id)?;

    // Ethereum address = last 20 bytes of keccak256(uncompressed_pubkey_without_prefix)
    let encoded = vk.to_encoded_point(false);
    let pubkey_hash = keccak256(&encoded.as_bytes()[1..]); // skip 0x04 prefix
    Ok(Address::from_slice(&pubkey_hash[12..]))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_separator_is_deterministic() {
        let contract: Address = "0x8f69F5C07477Ac46FBc491B1E6D91E2be0111A9e"
            .parse()
            .unwrap();
        let d1 = domain_separator("TAP", 42161, contract);
        let d2 = domain_separator("TAP", 42161, contract);
        assert_eq!(d1, d2);
    }

    #[test]
    fn domain_separator_differs_by_chain() {
        let contract: Address = "0x8f69F5C07477Ac46FBc491B1E6D91E2be0111A9e"
            .parse()
            .unwrap();
        let mainnet = domain_separator("TAP", 1, contract);
        let arbitrum = domain_separator("TAP", 42161, contract);
        assert_ne!(mainnet, arbitrum);
    }
}
