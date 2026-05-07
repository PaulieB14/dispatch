use std::time::{SystemTime, UNIX_EPOCH};

use alloy_primitives::{Address, Bytes, B256};
use k256::ecdsa::SigningKey;
use rand::Rng;
use thiserror::Error;

use crate::{
    eip712::eip712_hash,
    types::{Receipt, SignedReceipt},
};

#[derive(Debug, Error)]
pub enum SignError {
    #[error("signing failed: {0}")]
    Signing(#[from] k256::ecdsa::Error),
}

/// Create and sign a TAP v2 receipt.
///
/// Called by the gateway for each outgoing RPC request.
///
/// # Parameters
/// - `signing_key` — gateway's operator ECDSA key
/// - `domain_sep` — pre-computed EIP-712 domain separator for GraphTallyCollector
/// - `data_service` — RPCDataService contract address
/// - `service_provider` — the selected indexer's address
/// - `value` — payment in GRT wei (CU weight × base_price_per_cu)
/// - `metadata` — optional extra data (e.g. encoded CU weight for accounting)
pub fn create_receipt(
    signing_key: &SigningKey,
    domain_sep: B256,
    data_service: Address,
    service_provider: Address,
    value: u128,
    metadata: Bytes,
) -> Result<SignedReceipt, SignError> {
    let receipt = Receipt {
        data_service,
        service_provider,
        timestamp_ns: now_ns(),
        nonce: rand::thread_rng().gen::<u64>(),
        value,
        metadata,
    };

    let hash = eip712_hash(domain_sep, &receipt);

    let (sig, rec_id) = signing_key.sign_prehash_recoverable(hash.as_slice())?;

    let mut sig_bytes = [0u8; 65];
    sig_bytes[..64].copy_from_slice(&sig.to_bytes());
    sig_bytes[64] = rec_id.to_byte() + 27;

    Ok(SignedReceipt {
        receipt,
        signature: format!("0x{}", hex::encode(sig_bytes)),
    })
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eip712::domain_separator;

    fn test_key() -> SigningKey {
        // Known test key — never use in production
        SigningKey::from_slice(&[0x42u8; 32]).unwrap()
    }

    #[test]
    fn create_receipt_produces_unique_nonces() {
        let key = test_key();
        let dom = domain_separator("TAP", 42161, Address::ZERO);

        let r1 = create_receipt(
            &key,
            dom,
            Address::ZERO,
            Address::ZERO,
            100,
            Bytes::default(),
        )
        .unwrap();
        let r2 = create_receipt(
            &key,
            dom,
            Address::ZERO,
            Address::ZERO,
            100,
            Bytes::default(),
        )
        .unwrap();

        // Nonces must differ (random)
        assert_ne!(r1.receipt.nonce, r2.receipt.nonce);
    }

    #[test]
    fn create_receipt_signature_is_65_bytes() {
        let key = test_key();
        let dom = domain_separator("TAP", 42161, Address::ZERO);

        let signed = create_receipt(
            &key,
            dom,
            Address::ZERO,
            Address::ZERO,
            500,
            Bytes::default(),
        )
        .unwrap();
        let sig_bytes = hex::decode(signed.signature.trim_start_matches("0x")).unwrap();
        assert_eq!(sig_bytes.len(), 65);
    }
}
