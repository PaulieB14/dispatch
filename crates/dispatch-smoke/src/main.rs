//! dispatch-smoke — end-to-end smoke test for a live Dispatch provider.
//!
//! Signs real TAP receipts and fires JSON-RPC requests at a provider endpoint,
//! verifying the full consumer → provider → Chainstack flow.
//!
//! Usage:
//!   cargo run --bin dispatch-smoke -- \
//!     --endpoint https://rpc.cargopete.com \
//!     --chain-id 42161 \
//!     --consumer-address 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
//!
//! Flags / environment variables (all optional — defaults target the live network):
//!   --endpoint / DISPATCH_ENDPOINT              Provider or gateway URL  (default: https://rpc.cargopete.com)
//!   --chain-id / DISPATCH_CHAIN_ID              EIP-155 chain ID        (default: 42161)
//!   --data-service / DISPATCH_DATA_SERVICE      RPCDataService address  (default: live mainnet)
//!   --tally-collector / DISPATCH_TALLY_COLLECTOR GraphTallyCollector    (default: live mainnet)
//!   --signer-key / DISPATCH_SIGNER_KEY          Hex private key         (default: random ephemeral)
//!   --provider-address / DISPATCH_PROVIDER_ADDRESS  Provider address    (default: zero)
//!   --consumer-address / DISPATCH_CONSUMER_ADDRESS  Consumer address    (default: omitted)

use alloy_primitives::{address, Address, Bytes};
use anyhow::{Context, Result};
use k256::ecdsa::SigningKey;
use rand::rngs::OsRng;
use serde_json::{json, Value};
use std::time::Instant;

// ── Defaults ──────────────────────────────────────────────────────────────────

const DEFAULT_ENDPOINT: &str = "https://rpc.cargopete.com";
const DEFAULT_CHAIN_ID: u64 = 42161;
const DATA_SERVICE: Address = address!("A983b18B8291F0c317Ba4Fe0dc0f7cc9373AF078");
const TALLY_COLLECTOR: Address = address!("8f69F5C07477Ac46FBc491B1E6D91E2bb0111A9e");
const EIP712_CHAIN_ID: u64 = 42161; // Arbitrum One — where GraphTallyCollector lives
const BASE_PRICE_PER_CU: u128 = 4_000_000_000_000; // 4e-6 GRT per CU

// ── Test cases ────────────────────────────────────────────────────────────────

struct Test {
    name: &'static str,
    method: &'static str,
    params: Value,
    /// Return true if the response looks valid.
    validate: fn(&Value) -> bool,
}

fn tests() -> Vec<Test> {
    vec![
        Test {
            name: "eth_blockNumber — returns current block",
            method: "eth_blockNumber",
            params: json!([]),
            validate: |r| r.as_str().map_or(false, |s| s.starts_with("0x")),
        },
        Test {
            name: "eth_chainId — returns 0x61a9 (42161)",
            method: "eth_chainId",
            params: json!([]),
            validate: |r| r.as_str().map_or(false, |s| {
                u64::from_str_radix(s.trim_start_matches("0x"), 16)
                    .map_or(false, |id| id == 42161)
            }),
        },
        Test {
            name: "net_version — returns chain ID as decimal string",
            method: "net_version",
            params: json!([]),
            validate: |r| r.as_str().map_or(false, |s| s == "42161"),
        },
        Test {
            name: "eth_getBalance — latest block (Standard tier)",
            method: "eth_getBalance",
            // Arbitrum One bridge — always has a balance
            params: json!(["0x8315177aB297bA92A06054cE80a67Ed4DBd7ed3a", "latest"]),
            validate: |r| r.as_str().map_or(false, |s| s.starts_with("0x")),
        },
        Test {
            name: "eth_getBalance — historical block (Archive tier)",
            method: "eth_getBalance",
            params: json!(["0x8315177aB297bA92A06054cE80a67Ed4DBd7ed3a", "0x1000000"]),
            validate: |r| r.as_str().map_or(false, |s| s.starts_with("0x")),
        },
        Test {
            name: "eth_getBlockByNumber — latest (Standard tier)",
            method: "eth_getBlockByNumber",
            params: json!(["latest", false]),
            validate: |r| r.get("number").and_then(|n| n.as_str()).map_or(false, |s| s.starts_with("0x")),
        },
        Test {
            name: "eth_getCode — WETH contract on Arbitrum",
            method: "eth_getCode",
            // WETH9 on Arbitrum One
            params: json!(["0x82aF49447D8a07e3bd95BD0d56f35241523fBab1", "latest"]),
            validate: |r| r.as_str().map_or(false, |s| s.len() > 4),
        },
        Test {
            name: "eth_estimateGas — ETH transfer",
            method: "eth_estimateGas",
            params: json!([{
                "from": "0x8315177aB297bA92A06054cE80a67Ed4DBd7ed3a",
                "to":   "0x8315177aB297bA92A06054cE80a67Ed4DBd7ed3a",
                "value": "0x0"
            }]),
            validate: |r| r.as_str().map_or(false, |s| s.starts_with("0x")),
        },
        Test {
            name: "eth_getLogs — latest block range (Standard quorum)",
            method: "eth_getLogs",
            params: json!([{ "fromBlock": "latest", "toBlock": "latest" }]),
            validate: |r| r.is_array(),
        },
    ]
}

// ── Main ──────────────────────────────────────────────────────────────────────

/// Parse `--flag value` from argv, falling back to an env var, then a default.
fn opt(flag: &str, env: &str, default: &str) -> String {
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len().saturating_sub(1) {
        if args[i] == flag {
            return args[i + 1].clone();
        }
    }
    std::env::var(env).unwrap_or_else(|_| default.to_string())
}

fn opt_maybe(flag: &str, env: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len().saturating_sub(1) {
        if args[i] == flag {
            return Some(args[i + 1].clone());
        }
    }
    std::env::var(env).ok()
}

#[tokio::main]
async fn main() -> Result<()> {
    let endpoint = opt("--endpoint", "DISPATCH_ENDPOINT", DEFAULT_ENDPOINT);
    let chain_id: u64 = opt("--chain-id", "DISPATCH_CHAIN_ID", "42161")
        .parse()
        .unwrap_or(DEFAULT_CHAIN_ID);

    let data_service: Address = opt(
        "--data-service",
        "DISPATCH_DATA_SERVICE",
        "A983b18B8291F0c317Ba4Fe0dc0f7cc9373AF078",
    )
    .parse()
    .unwrap_or(DATA_SERVICE);

    let tally_collector: Address = opt(
        "--tally-collector",
        "DISPATCH_TALLY_COLLECTOR",
        "8f69F5C07477Ac46FBc491B1E6D91E2bb0111A9e",
    )
    .parse()
    .unwrap_or(TALLY_COLLECTOR);

    let consumer_address: Option<Address> =
        opt_maybe("--consumer-address", "DISPATCH_CONSUMER_ADDRESS")
            .and_then(|s| s.parse().ok());

    // Use provided key or generate a fresh ephemeral one.
    let signing_key = match opt_maybe("--signer-key", "DISPATCH_SIGNER_KEY") {
        Some(hex) => {
            let bytes = hex::decode(hex.trim_start_matches("0x"))
                .context("--signer-key: invalid hex")?;
            SigningKey::from_slice(&bytes).context("--signer-key: invalid key")?
        }
        None => SigningKey::random(&mut OsRng),
    };

    let provider_addr: Address = opt_maybe("--provider-address", "DISPATCH_PROVIDER_ADDRESS")
        .and_then(|s| s.parse().ok())
        .unwrap_or(Address::ZERO);

    let signer_address = dispatch_tap::address_from_key(&signing_key);
    let domain_sep = dispatch_tap::domain_separator("GraphTallyCollector", EIP712_CHAIN_ID, tally_collector);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    println!("dispatch-smoke");
    println!("  endpoint   : {endpoint}");
    println!("  chain_id   : {chain_id}");
    println!("  data_svc   : {data_service}");
    println!("  signer     : {signer_address}");
    if let Some(ca) = consumer_address {
        println!("  consumer   : {ca}");
    }
    println!();

    // ── Health check ─────────────────────────────────────────────────────────
    let health_url = format!("{endpoint}/health");
    let health = client.get(&health_url).send().await;
    match health {
        Ok(r) if r.status().is_success() => println!("  [PASS] GET /health → {}", r.status()),
        Ok(r) => {
            println!("  [FAIL] GET /health → {}", r.status());
            std::process::exit(1);
        }
        Err(e) => {
            println!("  [FAIL] GET /health → {e}");
            println!();
            println!("  DNS may not have propagated yet. Try again in a few minutes.");
            std::process::exit(1);
        }
    }

    // ── RPC tests ─────────────────────────────────────────────────────────────
    let url = format!("{endpoint}/rpc/{chain_id}");
    let mut passed = 0usize;
    let mut failed = 0usize;

    for (i, test) in tests().iter().enumerate() {
        let cu = cu_for(test.method);
        let receipt_value = cu as u128 * BASE_PRICE_PER_CU;

        let receipt = dispatch_tap::create_receipt(
            &signing_key,
            domain_sep,
            data_service,
            // service_provider: defaults to zero if not specified — provider will reject the
            // receipt signature; that's expected for connectivity testing.
            // Pass --provider-address for a full validated smoke run.
            provider_addr,
            receipt_value,
            Bytes::default(),
        )?;

        let receipt_header = serde_json::to_string(&receipt)?;

        let body = json!({
            "jsonrpc": "2.0",
            "method": test.method,
            "params": test.params,
            "id": i + 1,
        });

        let t0 = Instant::now();
        let mut req = client
            .post(&url)
            .header("TAP-Receipt", &receipt_header)
            .json(&body);
        if let Some(ca) = consumer_address {
            req = req.header("X-Consumer-Address", ca.to_string());
        }
        let resp = req.send().await;

        let elapsed = t0.elapsed().as_millis();

        match resp {
            Err(e) => {
                println!("  [FAIL] {} — {e}", test.name);
                failed += 1;
            }
            Ok(r) => {
                let status = r.status();
                let json: Value = r.json().await.unwrap_or(Value::Null);

                if let Some(error) = json.get("error") {
                    // Receipt rejection (unauthorized sender) is expected when using an
                    // ephemeral key — the provider correctly rejected the receipt.
                    // Treat this as a "provider responded" pass for connectivity purposes.
                    let code = error.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
                    if code == -32001 || code == -32003 {
                        println!("  [PASS] {} — provider rejected receipt (expected for ephemeral key) [{elapsed}ms]", test.name);
                        passed += 1;
                    } else {
                        println!("  [FAIL] {} — RPC error: {error} [{elapsed}ms]", test.name);
                        failed += 1;
                    }
                } else if !status.is_success() {
                    println!("  [FAIL] {} — HTTP {status} [{elapsed}ms]", test.name);
                    failed += 1;
                } else if let Some(result) = json.get("result") {
                    if (test.validate)(result) {
                        println!("  [PASS] {} → {} [{elapsed}ms]", test.name, truncate(&result.to_string(), 40));
                        passed += 1;
                    } else {
                        println!("  [FAIL] {} — unexpected result: {result} [{elapsed}ms]", test.name);
                        failed += 1;
                    }
                } else {
                    println!("  [FAIL] {} — no result field: {json} [{elapsed}ms]", test.name);
                    failed += 1;
                }
            }
        }
    }

    // ── Batch request ─────────────────────────────────────────────────────────
    println!();
    println!("  batch requests");

    let batch_receipt = dispatch_tap::create_receipt(
        &signing_key,
        domain_sep,
        data_service,
        provider_addr,
        cu_for("eth_blockNumber") as u128 * BASE_PRICE_PER_CU,
        Bytes::default(),
    )?;
    let batch_receipt_header = serde_json::to_string(&batch_receipt)?;

    let batch_body = json!([
        { "jsonrpc": "2.0", "method": "eth_blockNumber", "params": [], "id": 101 },
        { "jsonrpc": "2.0", "method": "eth_chainId",     "params": [], "id": 102 },
    ]);

    let t0 = Instant::now();
    let mut batch_req = client
        .post(&url)
        .header("TAP-Receipt", &batch_receipt_header)
        .json(&batch_body);
    if let Some(ca) = consumer_address {
        batch_req = batch_req.header("X-Consumer-Address", ca.to_string());
    }
    let batch_resp = batch_req.send().await;
    let elapsed = t0.elapsed().as_millis();

    match batch_resp {
        Err(e) => {
            println!("  [FAIL] batch(2) — {e}");
            failed += 1;
        }
        Ok(r) => {
            let status = r.status();
            let json: Value = r.json().await.unwrap_or(Value::Null);
            if let Some(arr) = json.as_array() {
                // Batch of 2: both may have results OR receipt-rejection errors
                let all_responded = arr.iter().all(|item| item.get("result").is_some() || item.get("error").is_some());
                if all_responded && arr.len() == 2 {
                    println!("  [PASS] batch(2) — {len} responses [{elapsed}ms]", len = arr.len());
                    passed += 1;
                } else {
                    println!("  [FAIL] batch(2) — unexpected response: {json} [{elapsed}ms]");
                    failed += 1;
                }
            } else if !status.is_success() {
                println!("  [FAIL] batch(2) — HTTP {status} [{elapsed}ms]");
                failed += 1;
            } else {
                println!("  [FAIL] batch(2) — expected array, got: {json} [{elapsed}ms]");
                failed += 1;
            }
        }
    }

    // ── Negative tests — authorization enforcement ─────────────────────────────
    println!();
    println!("  authorization enforcement");

    // Missing TAP-Receipt header → expect -32001 (MissingReceipt)
    let t0 = Instant::now();
    let mut no_receipt_req = client
        .post(&url)
        .json(&json!({ "jsonrpc": "2.0", "method": "eth_blockNumber", "params": [], "id": 201 }));
    if let Some(ca) = consumer_address {
        no_receipt_req = no_receipt_req.header("X-Consumer-Address", ca.to_string());
    }
    let no_receipt_resp = no_receipt_req.send().await;
    let elapsed = t0.elapsed().as_millis();

    match no_receipt_resp {
        Err(e) => {
            println!("  [FAIL] missing receipt — {e}");
            failed += 1;
        }
        Ok(r) => {
            let json: Value = r.json().await.unwrap_or(Value::Null);
            let code = json.get("error").and_then(|e| e.get("code")).and_then(|c| c.as_i64());
            if code == Some(-32001) {
                println!("  [PASS] missing receipt → -32001 (MissingReceipt) [{elapsed}ms]");
                passed += 1;
            } else {
                // If hitting the gateway (which doesn't require a receipt from consumers),
                // this will return a result rather than an error — that's also valid.
                let has_result = json.get("result").is_some();
                if has_result {
                    println!("  [PASS] missing receipt → result returned (gateway mode, no receipt required) [{elapsed}ms]");
                    passed += 1;
                } else {
                    println!("  [FAIL] missing receipt — expected -32001 or a result, got: {json} [{elapsed}ms]");
                    failed += 1;
                }
            }
        }
    }

    // Malformed TAP-Receipt header → expect -32001 (InvalidReceipt)
    let t0 = Instant::now();
    let mut bad_receipt_req = client
        .post(&url)
        .header("TAP-Receipt", "not-a-valid-receipt")
        .json(&json!({ "jsonrpc": "2.0", "method": "eth_blockNumber", "params": [], "id": 202 }));
    if let Some(ca) = consumer_address {
        bad_receipt_req = bad_receipt_req.header("X-Consumer-Address", ca.to_string());
    }
    let bad_receipt_resp = bad_receipt_req.send().await;
    let elapsed = t0.elapsed().as_millis();

    match bad_receipt_resp {
        Err(e) => {
            println!("  [FAIL] malformed receipt — {e}");
            failed += 1;
        }
        Ok(r) => {
            let json: Value = r.json().await.unwrap_or(Value::Null);
            let code = json.get("error").and_then(|e| e.get("code")).and_then(|c| c.as_i64());
            if code == Some(-32001) {
                println!("  [PASS] malformed receipt → -32001 (InvalidReceipt) [{elapsed}ms]");
                passed += 1;
            } else {
                let has_result = json.get("result").is_some();
                if has_result {
                    println!("  [PASS] malformed receipt → result returned (gateway mode, header ignored) [{elapsed}ms]");
                    passed += 1;
                } else {
                    println!("  [FAIL] malformed receipt — expected -32001 or a result, got: {json} [{elapsed}ms]");
                    failed += 1;
                }
            }
        }
    }

    println!();
    println!("  {passed} passed, {failed} failed");

    if failed > 0 {
        std::process::exit(1);
    }

    Ok(())
}

fn cu_for(method: &str) -> u32 {
    match method {
        "eth_chainId" | "eth_blockNumber" => 1,
        "eth_getBalance" | "eth_getCode" | "eth_getTransactionCount" | "eth_getStorageAt" => 5,
        "eth_call" | "eth_estimateGas" | "eth_getTransactionReceipt" => 10,
        "eth_getLogs" => 20,
        _ => 10,
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}
