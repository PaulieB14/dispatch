//! TAP v2 (GraphTally) receipt validation.
//!
//! Receipts are transmitted as JSON in the `TAP-Receipt` HTTP header.
//! Each receipt is an EIP-712 signed message. We recover the signer and
//! check it against the authorised sender list.

use alloy_primitives::{Address, B256};

use crate::error::ServiceError;

/// A receipt that has passed all validation checks.
pub struct ValidatedReceipt {
    pub receipt: dispatch_tap::Receipt,
    /// Address that signed the receipt (the gateway's operator key).
    pub signer: Address,
    /// Consumer address whose escrow will be debited — extracted from receipt
    /// metadata (first 20 bytes). Falls back to `signer` for old receipts that
    /// pre-date the consumer-pays model.
    pub payer: Address,
    /// JSON-RPC method name — extracted from receipt metadata bytes 20+.
    /// None for receipts from older gateway versions that only encoded the payer.
    pub method: Option<String>,
    pub signature: String,
}

/// Decode and validate a TAP receipt from the `TAP-Receipt` HTTP header value.
pub fn validate_receipt(
    header_value: &str,
    domain_sep: B256,
    authorized_senders: &[Address],
    data_service: Address,
    service_provider: Address,
    max_age_ns: u64,
    now_ns: u64,
) -> Result<ValidatedReceipt, ServiceError> {
    let signed: dispatch_tap::SignedReceipt = serde_json::from_str(header_value)
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

    let msg_hash = dispatch_tap::eip712_hash(domain_sep, r);
    let signer = dispatch_tap::recover_signer(msg_hash, &signed.signature)
        .map_err(|e| ServiceError::InvalidReceipt(format!("signature recovery failed: {e}")))?;

    if !authorized_senders.is_empty() && !authorized_senders.contains(&signer) {
        return Err(ServiceError::UnauthorizedSender(signer.to_string()));
    }

    // OVERRIDE: use the receipt signer as the payer field.
    //
    // Upstream extracts a consumer wallet from metadata first 20 bytes, but
    // the public Lodestar gateway's /rav/aggregate endpoint does a strict
    // `recovered_signer == payer` check that rejects mismatches. Cargopete
    // funds escrow at (gateway_signer, GraphTallyCollector, provider) for
    // each provider, so using the signer as payer matches both the gateway
    // aggregator's expectation and the on-chain escrow lookup.
    //
    // Tradeoff: breaks compatibility with future consumers who fund their
    // own (consumer_wallet, GTC, provider) escrow buckets. Revisit when
    // cargopete ships a delegated-signer fix to the gateway.
    let _metadata_payer = dispatch_tap::payer_from_metadata(&r.metadata);
    let payer = signer;
    // Method name is encoded as UTF-8 at bytes 20+ (gateway versions >= this change).
    let method = dispatch_tap::method_from_metadata(&r.metadata);

    Ok(ValidatedReceipt {
        receipt: signed.receipt,
        signer,
        payer,
        method,
        signature: signed.signature,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, Address, Bytes};
    use dispatch_tap::{address_from_key, create_receipt, domain_separator};
    use k256::ecdsa::SigningKey;

    const DATA_SERVICE: Address = address!("A983b18B8291F0c317Ba4Fe0dc0f7cc9373AF078");
    const TALLY_COLLECTOR: Address = address!("8f69F5C07477Ac46FBc491B1E6D91E2bb0111A9e");
    const PROVIDER: Address = address!("1111111111111111111111111111111111111111");
    const MAX_AGE_NS: u64 = 5 * 60 * 1_000_000_000; // 5 minutes

    fn signer_key() -> SigningKey {
        SigningKey::from_slice(&[0x42u8; 32]).unwrap()
    }

    fn other_key() -> SigningKey {
        SigningKey::from_slice(&[0x99u8; 32]).unwrap()
    }

    fn dom() -> alloy_primitives::B256 {
        domain_separator("GraphTallyCollector", 42161, TALLY_COLLECTOR)
    }

    fn now_ns() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64
    }

    fn make_header(key: &SigningKey) -> String {
        let signed = create_receipt(key, dom(), DATA_SERVICE, PROVIDER, 1_000, Bytes::default()).unwrap();
        serde_json::to_string(&signed).unwrap()
    }

    #[test]
    fn happy_path() {
        let key = signer_key();
        let signer = address_from_key(&key);
        let result = validate_receipt(&make_header(&key), dom(), &[signer], DATA_SERVICE, PROVIDER, MAX_AGE_NS, now_ns());
        assert!(result.is_ok());
        assert_eq!(result.unwrap().signer, signer);
    }

    #[test]
    fn empty_authorized_senders_accepts_any_signer() {
        let key = signer_key();
        let result = validate_receipt(&make_header(&key), dom(), &[], DATA_SERVICE, PROVIDER, MAX_AGE_NS, now_ns());
        assert!(result.is_ok());
    }

    #[test]
    fn invalid_json_is_rejected() {
        let result = validate_receipt("not-json-at-all", dom(), &[], DATA_SERVICE, PROVIDER, MAX_AGE_NS, now_ns());
        assert!(matches!(result, Err(ServiceError::InvalidReceipt(_))));
    }

    #[test]
    fn data_service_mismatch_is_rejected() {
        let key = signer_key();
        let wrong_ds = address!("2222222222222222222222222222222222222222");
        let signed = create_receipt(&key, dom(), wrong_ds, PROVIDER, 1_000, Bytes::default()).unwrap();
        let header = serde_json::to_string(&signed).unwrap();
        let result = validate_receipt(&header, dom(), &[], DATA_SERVICE, PROVIDER, MAX_AGE_NS, now_ns());
        assert!(matches!(result, Err(ServiceError::InvalidReceipt(_))));
    }

    #[test]
    fn service_provider_mismatch_is_rejected() {
        let key = signer_key();
        let wrong_provider = address!("3333333333333333333333333333333333333333");
        let signed = create_receipt(&key, dom(), DATA_SERVICE, wrong_provider, 1_000, Bytes::default()).unwrap();
        let header = serde_json::to_string(&signed).unwrap();
        let result = validate_receipt(&header, dom(), &[], DATA_SERVICE, PROVIDER, MAX_AGE_NS, now_ns());
        assert!(matches!(result, Err(ServiceError::InvalidReceipt(_))));
    }

    #[test]
    fn expired_receipt_is_rejected() {
        let key = signer_key();
        let header = make_header(&key);
        // Advance time past the max age window
        let future_ns = now_ns() + MAX_AGE_NS + 1;
        let result = validate_receipt(&header, dom(), &[], DATA_SERVICE, PROVIDER, MAX_AGE_NS, future_ns);
        assert!(matches!(result, Err(ServiceError::ReceiptExpired)));
    }

    #[test]
    fn invalid_signature_is_rejected() {
        let key = signer_key();
        let mut signed = create_receipt(&key, dom(), DATA_SERVICE, PROVIDER, 1_000, Bytes::default()).unwrap();
        // v = 0xff → rec_id = 255 − 27 = 228, not a valid recovery id (must be 0 or 1)
        signed.signature = format!("0x{}", "ff".repeat(65));
        let header = serde_json::to_string(&signed).unwrap();
        let result = validate_receipt(&header, dom(), &[], DATA_SERVICE, PROVIDER, MAX_AGE_NS, now_ns());
        assert!(matches!(result, Err(ServiceError::InvalidReceipt(_))));
    }

    #[test]
    fn unauthorized_sender_is_rejected() {
        let key = signer_key();
        let authorized = address_from_key(&other_key()); // different from signer_key
        let result = validate_receipt(&make_header(&key), dom(), &[authorized], DATA_SERVICE, PROVIDER, MAX_AGE_NS, now_ns());
        assert!(matches!(result, Err(ServiceError::UnauthorizedSender(_))));
    }
}
