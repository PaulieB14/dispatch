use alloy_primitives::{Address, Bytes};
use serde::{Deserialize, Serialize};

/// Extract the consumer (payer) address from a TAP receipt's metadata field.
///
/// The gateway encodes the consumer's Ethereum address as the first 20 bytes of
/// `metadata` before signing the receipt. Providers use this to determine whose
/// escrow to charge and to group receipts into per-consumer RAVs.
///
/// Returns `None` if the metadata is too short (e.g. old receipts with no payer).
pub fn payer_from_metadata(metadata: &Bytes) -> Option<Address> {
    if metadata.len() >= 20 {
        Some(Address::from_slice(&metadata[..20]))
    } else {
        None
    }
}

/// Extract the JSON-RPC method name from a TAP receipt's metadata field.
///
/// The gateway appends the method name as UTF-8 bytes starting at byte 20 of
/// `metadata` (after the 20-byte consumer address). Returns `None` if the
/// metadata is 20 bytes or fewer, or if the bytes are not valid UTF-8.
pub fn method_from_metadata(metadata: &Bytes) -> Option<String> {
    if metadata.len() > 20 {
        std::str::from_utf8(&metadata[20..]).ok().map(|s| s.to_string())
    } else {
        None
    }
}

/// EIP-712 type string for the TAP v2 Receipt struct.
/// Must match exactly what the deployed GraphTallyCollector uses.
pub const RECEIPT_TYPE_STRING: &str =
    "Receipt(address data_service,address service_provider,uint64 timestamp_ns,uint64 nonce,uint128 value,bytes metadata)";

/// A TAP v2 receipt — one per RPC request, signed by the gateway.
///
/// Mirrors the on-chain Solidity struct in GraphTallyCollector.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Receipt {
    pub data_service: Address,
    pub service_provider: Address,
    pub timestamp_ns: u64,
    pub nonce: u64,
    pub value: u128,
    #[serde(default)]
    pub metadata: Bytes,
}

/// An EIP-712 signed receipt, transmitted as JSON in the `TAP-Receipt` HTTP header.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedReceipt {
    pub receipt: Receipt,
    /// Hex-encoded 65-byte ECDSA signature: r(32) || s(32) || v(1).
    pub signature: String,
}
