//! On-chain escrow balance checker with a short-lived cache.
//!
//! Before serving any request we verify the consumer has funded their
//! PaymentsEscrow slot. Checking every request would be far too slow, so
//! results are cached for 30 seconds — tight enough to catch newly-drained
//! accounts without hammering Arbitrum One.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use alloy_primitives::{Address, U256};
use alloy_sol_types::{sol, SolCall};
use anyhow::Result;

// Only the selector + argument encoding matters here; we read the first
// uint256 from the raw return bytes rather than fully decoding the struct.
sol! {
    function getBalance(
        address payer,
        address collector,
        address receiver
    ) external view returns (uint256 balance, uint256 thawEndTimestamp, uint256 thawingTokens);
}

const CACHE_TTL: Duration = Duration::from_secs(30);

pub struct EscrowChecker {
    rpc_url: String,
    escrow_address: Address,
    /// GraphTallyCollector (the TAP collector contract).
    collector_address: Address,
    /// This indexer's service-provider address.
    receiver_address: Address,
    http: reqwest::Client,
    /// payer → (balance_wei, cached_at)
    // pub(crate) so unit tests can inspect cache state without a public API.
    pub(crate) cache: Mutex<HashMap<Address, (u128, Instant)>>,
}

impl EscrowChecker {
    pub fn new(
        rpc_url: String,
        escrow_address: Address,
        collector_address: Address,
        receiver_address: Address,
        http: reqwest::Client,
    ) -> Self {
        Self {
            rpc_url,
            escrow_address,
            collector_address,
            receiver_address,
            http,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Returns the escrow balance (GRT wei) for `payer`.
    /// Cached for 30 s; a fresh on-chain call is made on cache miss or expiry.
    pub async fn balance(&self, payer: Address) -> Result<u128> {
        // --- cache hit ---
        {
            let cache = self.cache.lock().unwrap();
            if let Some((bal, ts)) = cache.get(&payer) {
                if ts.elapsed() < CACHE_TTL {
                    return Ok(*bal);
                }
            }
        }

        // --- eth_call ---
        let call_data = getBalanceCall {
            payer,
            collector: self.collector_address,
            receiver: self.receiver_address,
        }
        .abi_encode();

        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_call",
            "params": [
                {
                    "to": format!("{:#x}", self.escrow_address),
                    "data": format!("0x{}", hex::encode(&call_data)),
                },
                "latest"
            ]
        });

        let resp: serde_json::Value = self
            .http
            .post(&self.rpc_url)
            .json(&payload)
            .send()
            .await?
            .json()
            .await?;

        let hex_result = resp["result"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("eth_call returned no result: {resp}"))?;

        let bytes = hex::decode(hex_result.trim_start_matches("0x"))?;
        if bytes.len() < 32 {
            anyhow::bail!("eth_call result too short ({} bytes)", bytes.len());
        }

        // First 32 bytes = balance uint256 (first struct field).
        let balance_u256 = U256::from_be_slice(&bytes[..32]);
        let balance: u128 = balance_u256.try_into().unwrap_or(u128::MAX);

        tracing::debug!(
            payer = %payer,
            balance,
            "escrow balance fetched"
        );

        // --- update cache ---
        {
            let mut cache = self.cache.lock().unwrap();
            cache.insert(payer, (balance, Instant::now()));
        }

        Ok(balance)
    }

    /// Invalidate the cached balance for `payer` (e.g. after a successful collect).
    pub fn invalidate(&self, payer: Address) {
        self.cache.lock().unwrap().remove(&payer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::post, Json, Router};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    /// ABI-encode the three-uint256 return value of getBalance, placing `balance`
    /// in the first slot (bytes 0-31, big-endian).
    fn abi_result(balance: u128) -> String {
        let mut encoded = vec![0u8; 96]; // 3 × 32 bytes
        encoded[16..32].copy_from_slice(&balance.to_be_bytes());
        format!("0x{}", hex::encode(encoded))
    }

    fn rpc_response(result: &str) -> serde_json::Value {
        serde_json::json!({"jsonrpc":"2.0","id":1,"result": result})
    }

    /// Spawn a one-shot mock RPC server. Returns the URL and a call counter.
    async fn mock_rpc(balance: u128) -> (String, Arc<AtomicUsize>) {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter2 = Arc::clone(&counter);
        let result = abi_result(balance);
        let app = Router::new().route(
            "/",
            post(move || {
                counter2.fetch_add(1, Ordering::SeqCst);
                let body = rpc_response(&result);
                async move { Json(body) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://127.0.0.1:{port}"), counter)
    }

    fn checker(url: &str) -> EscrowChecker {
        EscrowChecker::new(
            url.to_string(),
            Address::ZERO,
            Address::ZERO,
            Address::ZERO,
            reqwest::Client::new(),
        )
    }

    #[tokio::test]
    async fn returns_correct_non_zero_balance() {
        let (url, _) = mock_rpc(1_000_000_000_000_000_000u128).await;
        let bal = checker(&url).balance(Address::ZERO).await.unwrap();
        assert_eq!(bal, 1_000_000_000_000_000_000u128);
    }

    #[tokio::test]
    async fn returns_zero_balance() {
        let (url, _) = mock_rpc(0).await;
        let bal = checker(&url).balance(Address::ZERO).await.unwrap();
        assert_eq!(bal, 0);
    }

    #[tokio::test]
    async fn second_call_hits_cache_not_server() {
        let (url, counter) = mock_rpc(42).await;
        let c = checker(&url);
        c.balance(Address::ZERO).await.unwrap(); // fetches from server
        c.balance(Address::ZERO).await.unwrap(); // must use cache
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "server should be called exactly once"
        );
    }

    #[tokio::test]
    async fn invalidate_removes_cache_entry() {
        let (url, _) = mock_rpc(7).await;
        let c = checker(&url);
        c.balance(Address::ZERO).await.unwrap();
        assert!(c.cache.lock().unwrap().contains_key(&Address::ZERO));
        c.invalidate(Address::ZERO);
        assert!(!c.cache.lock().unwrap().contains_key(&Address::ZERO));
    }
}
