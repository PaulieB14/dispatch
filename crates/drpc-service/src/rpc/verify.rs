//! Verification tier classification for Ethereum JSON-RPC methods.
//!
//! Tier 1 — Merkle-provable: responses can be verified via EIP-1186 eth_getProof.
//! Tier 2 — Quorum-verifiable: correct but requires re-execution or cross-referencing.
//! Tier 3 — Non-deterministic: implementation-specific; reputation scoring only.

use alloy_primitives::B256;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationTier {
    /// Response is verifiable via Ethereum Merkle-Patricia trie proofs.
    MerkleProvable,
    /// Response is deterministic but requires quorum or re-execution to verify.
    Quorum,
    /// Response is non-deterministic; no on-chain dispute possible.
    Reputation,
}

/// Classify an RPC method into its verification tier.
pub fn tier_for_method(method: &str) -> VerificationTier {
    match method {
        // --- Tier 1: Merkle-provable ---
        "eth_getBalance"
        | "eth_getTransactionCount"
        | "eth_getStorageAt"
        | "eth_getCode"
        | "eth_getProof"
        | "eth_getBlockByHash"
        | "eth_getBlockByNumber" => VerificationTier::MerkleProvable,

        // --- Tier 2: Quorum-verifiable ---
        "eth_chainId"
        | "net_version"
        | "eth_blockNumber"
        | "eth_sendRawTransaction"
        | "eth_getTransactionReceipt"
        | "eth_getTransactionByHash"
        | "eth_getTransactionByBlockHashAndIndex"
        | "eth_getTransactionByBlockNumberAndIndex"
        | "eth_call"
        | "eth_getLogs"
        | "eth_getBlockReceipts"
        | "eth_feeHistory" => VerificationTier::Quorum,

        // --- Tier 3: Non-deterministic ---
        "eth_estimateGas"
        | "eth_gasPrice"
        | "eth_maxPriorityFeePerGas"
        | "eth_syncing"
        | "net_peerCount"
        | "net_listening"
        | "eth_mining"
        | "eth_hashrate" => VerificationTier::Reputation,

        // Unknown methods default to Quorum (conservative)
        _ => VerificationTier::Quorum,
    }
}

/// Extract block context (block_number, block_hash) from a JSON-RPC response result.
///
/// Used to anchor attestation hashes to a specific block. Where the response
/// naturally contains block fields (block objects, transaction receipts) we
/// pull them out directly. For state-query methods whose results are primitives
/// (eth_getBalance etc.) the response doesn't carry block context, so we return
/// (0, B256::ZERO) — the attestation still binds to method + params + response,
/// just without a block anchor.
pub fn extract_block_context(method: &str, result: &Value) -> (u64, B256) {
    match method {
        // Block-returning methods: result is a block object with "number" and "hash"
        "eth_getBlockByHash" | "eth_getBlockByNumber" => (
            parse_hex_u64(result.get("number").and_then(Value::as_str)),
            parse_b256(result.get("hash").and_then(Value::as_str)),
        ),
        // Transaction-returning methods: result has "blockNumber" and "blockHash"
        "eth_getTransactionReceipt"
        | "eth_getTransactionByHash"
        | "eth_getTransactionByBlockHashAndIndex"
        | "eth_getTransactionByBlockNumberAndIndex" => (
            parse_hex_u64(result.get("blockNumber").and_then(Value::as_str)),
            parse_b256(result.get("blockHash").and_then(Value::as_str)),
        ),
        // State-query and all other methods: no block context in the response body
        _ => (0, B256::ZERO),
    }
}

fn parse_hex_u64(s: Option<&str>) -> u64 {
    s.and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0)
}

fn parse_b256(s: Option<&str>) -> B256 {
    s.and_then(|s| {
        let bytes = alloy_primitives::hex::decode(s).ok()?;
        (bytes.len() == 32).then(|| B256::from_slice(&bytes))
    })
    .unwrap_or(B256::ZERO)
}

/// Compute units (CU) for a given method.
/// Phase 1: all methods return the flat baseline weight.
/// Phase 2: expand this with per-method weights from the RFC.
pub fn cu_weight(method: &str) -> u32 {
    match method {
        "eth_chainId" | "net_version" | "eth_blockNumber" => 1,
        "eth_getBalance"
        | "eth_getTransactionCount"
        | "eth_getCode"
        | "eth_getStorageAt"
        | "eth_sendRawTransaction" => 5,
        "eth_getBlockByHash" | "eth_getBlockByNumber" => 5,
        "eth_call"
        | "eth_estimateGas"
        | "eth_getTransactionReceipt"
        | "eth_getTransactionByHash" => 10,
        "eth_getLogs" => 20,
        _ => 10, // conservative default
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merkle_provable_methods() {
        assert_eq!(
            tier_for_method("eth_getBalance"),
            VerificationTier::MerkleProvable
        );
        assert_eq!(
            tier_for_method("eth_getProof"),
            VerificationTier::MerkleProvable
        );
    }

    #[test]
    fn non_deterministic_methods() {
        assert_eq!(
            tier_for_method("eth_gasPrice"),
            VerificationTier::Reputation
        );
        assert_eq!(
            tier_for_method("eth_estimateGas"),
            VerificationTier::Reputation
        );
    }

    #[test]
    fn unknown_method_defaults_to_quorum() {
        assert_eq!(
            tier_for_method("debug_traceTransaction"),
            VerificationTier::Quorum
        );
    }

    #[test]
    fn block_context_from_block_object() {
        let result = serde_json::json!({
            "number": "0x12d687",
            "hash": "0x0100000000000000000000000000000000000000000000000000000000000000"
        });
        let (num, hash) = extract_block_context("eth_getBlockByNumber", &result);
        assert_eq!(num, 0x12d687u64);
        assert_ne!(hash, B256::ZERO);
    }

    #[test]
    fn block_context_from_tx_receipt() {
        let result = serde_json::json!({
            "blockNumber": "0x10",
            "blockHash": "0x0200000000000000000000000000000000000000000000000000000000000000"
        });
        let (num, hash) = extract_block_context("eth_getTransactionReceipt", &result);
        assert_eq!(num, 16u64);
        assert_ne!(hash, B256::ZERO);
    }

    #[test]
    fn block_context_absent_for_state_query() {
        // eth_getBalance result is a hex string — no block context in the response
        let result = serde_json::json!("0x0de0b6b3a7640000");
        let (num, hash) = extract_block_context("eth_getBalance", &result);
        assert_eq!(num, 0);
        assert_eq!(hash, B256::ZERO);
    }

    #[test]
    fn block_context_handles_null_fields_gracefully() {
        let result = serde_json::json!({ "number": null, "hash": null });
        let (num, hash) = extract_block_context("eth_getBlockByHash", &result);
        assert_eq!(num, 0);
        assert_eq!(hash, B256::ZERO);
    }
}
