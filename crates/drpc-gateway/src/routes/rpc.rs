use std::{sync::Arc, time::Instant};

use alloy_primitives::Bytes;
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::task::JoinSet;

use crate::{error::GatewayError, registry::Provider, selector, server::AppState};
use drpc_tap::create_receipt;

pub fn router() -> Router<AppState> {
    Router::new().route("/rpc/{chain_id}", post(rpc_handler))
}

// ---------------------------------------------------------------------------
// JSON-RPC types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
    pub id: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
    pub id: Option<Value>,
}

impl JsonRpcRequest {
    pub fn validate(&self) -> Result<(), GatewayError> {
        if self.jsonrpc != "2.0" {
            return Err(GatewayError::InvalidRequest(format!(
                "unsupported jsonrpc version: {}",
                self.jsonrpc
            )));
        }
        if self.method.is_empty() {
            return Err(GatewayError::InvalidRequest("method is empty".to_string()));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Quorum methods
// ---------------------------------------------------------------------------

/// Methods that require cross-provider consensus before returning.
///
/// For these methods the gateway waits for all k candidates to respond,
/// picks the majority result by value equality, rewards the majority, and
/// penalises any minority provider with a QoS failure.
fn requires_quorum(method: &str) -> bool {
    matches!(method, "eth_call" | "eth_getLogs")
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

async fn rpc_handler(
    State(state): State<AppState>,
    Path(chain_id): Path<u64>,
    Json(request): Json<JsonRpcRequest>,
) -> Result<Response, GatewayError> {
    request.validate()?;

    let (providers, chain_head) = state
        .registry
        .providers_for_chain(chain_id)
        .ok_or(GatewayError::UnsupportedChain(chain_id))?;

    if providers.is_empty() {
        return Err(GatewayError::NoProviders(chain_id));
    }

    let candidates = selector::select(providers, chain_head, state.config.qos.concurrent_k);
    let cu = cu_weight_for(&request.method);
    let receipt_value = cu as u128 * state.config.tap.base_price_per_cu;

    let (response, winner) = if requires_quorum(&request.method) {
        dispatch_quorum(&state, chain_id, &request, &candidates, receipt_value).await?
    } else {
        dispatch_concurrent(&state, chain_id, &request, &candidates, receipt_value).await?
    };

    tracing::debug!(
        method = %request.method,
        chain_id,
        provider = %winner.endpoint,
        cu,
        "served"
    );

    Ok(Json(response).into_response())
}

// ---------------------------------------------------------------------------
// Concurrent dispatch — first valid response wins (non-quorum methods)
// ---------------------------------------------------------------------------

async fn dispatch_concurrent(
    state: &AppState,
    chain_id: u64,
    request: &JsonRpcRequest,
    candidates: &[Arc<Provider>],
    receipt_value: u128,
) -> Result<(JsonRpcResponse, Arc<Provider>), GatewayError> {
    let mut set: JoinSet<Result<(JsonRpcResponse, Arc<Provider>), String>> = JoinSet::new();

    for provider in candidates {
        let client = state.http_client.clone();
        let signing_key = state.signing_key.clone();
        let domain_sep = state.tap_domain_separator;
        let data_service = state.config.tap.data_service_address;
        let req = request.clone();
        let p = provider.clone();

        set.spawn(async move {
            let signed = create_receipt(
                &signing_key,
                domain_sep,
                data_service,
                p.address,
                receipt_value,
                Bytes::default(),
            )
            .map_err(|e| e.to_string())?;

            let receipt_header =
                serde_json::to_string(&signed).map_err(|e| e.to_string())?;

            let url = format!("{}/rpc/{}", p.endpoint, chain_id);
            let start = Instant::now();

            let resp = client
                .post(&url)
                .header("TAP-Receipt", receipt_header)
                .json(&req)
                .send()
                .await
                .map_err(|e| format!("connection failed: {e}"))?;

            let ms = start.elapsed().as_millis() as u64;

            if !resp.status().is_success() {
                p.qos.record_failure();
                return Err(format!("HTTP {}", resp.status()));
            }

            let body = resp
                .json::<JsonRpcResponse>()
                .await
                .map_err(|e| format!("invalid response: {e}"))?;

            p.qos.record_success(ms);
            Ok((body, p))
        });
    }

    while let Some(join_result) = set.join_next().await {
        match join_result {
            Ok(Ok((response, provider))) => {
                set.abort_all();
                return Ok((response, provider));
            }
            Ok(Err(e)) => tracing::debug!(error = %e, "provider attempt failed"),
            Err(e) => tracing::debug!(error = %e, "task panicked"),
        }
    }

    Err(GatewayError::AllProvidersFailed(chain_id))
}

// ---------------------------------------------------------------------------
// Quorum dispatch — wait for all k, majority-vote on result
// ---------------------------------------------------------------------------

struct ProviderOutcome {
    response: JsonRpcResponse,
    provider: Arc<Provider>,
    latency_ms: u64,
}

async fn dispatch_quorum(
    state: &AppState,
    chain_id: u64,
    request: &JsonRpcRequest,
    candidates: &[Arc<Provider>],
    receipt_value: u128,
) -> Result<(JsonRpcResponse, Arc<Provider>), GatewayError> {
    let mut set: JoinSet<Result<ProviderOutcome, String>> = JoinSet::new();

    for provider in candidates {
        let client = state.http_client.clone();
        let signing_key = state.signing_key.clone();
        let domain_sep = state.tap_domain_separator;
        let data_service = state.config.tap.data_service_address;
        let req = request.clone();
        let p = provider.clone();

        set.spawn(async move {
            let signed = create_receipt(
                &signing_key,
                domain_sep,
                data_service,
                p.address,
                receipt_value,
                Bytes::default(),
            )
            .map_err(|e| e.to_string())?;

            let receipt_header =
                serde_json::to_string(&signed).map_err(|e| e.to_string())?;

            let url = format!("{}/rpc/{}", p.endpoint, chain_id);
            let start = Instant::now();

            let resp = client
                .post(&url)
                .header("TAP-Receipt", receipt_header)
                .json(&req)
                .send()
                .await
                .map_err(|e| format!("connection failed: {e}"))?;

            let latency_ms = start.elapsed().as_millis() as u64;

            if !resp.status().is_success() {
                return Err(format!("HTTP {}", resp.status()));
            }

            let response = resp
                .json::<JsonRpcResponse>()
                .await
                .map_err(|e| format!("invalid response: {e}"))?;

            Ok(ProviderOutcome { response, provider: p, latency_ms })
        });
    }

    // Collect all outcomes (don't abort early).
    let mut outcomes: Vec<ProviderOutcome> = Vec::new();
    while let Some(join_result) = set.join_next().await {
        match join_result {
            Ok(Ok(o)) => outcomes.push(o),
            Ok(Err(e)) => tracing::debug!(error = %e, "quorum provider failed"),
            Err(e) => tracing::debug!(error = %e, "quorum task panicked"),
        }
    }

    if outcomes.is_empty() {
        return Err(GatewayError::AllProvidersFailed(chain_id));
    }

    // Majority vote: find the result value shared by the most providers.
    // Uses serde_json::Value::eq for deep equality (correct for eth_call hex
    // strings and eth_getLogs log arrays).
    let winner_idx = majority_index(&outcomes);

    let winner_result = &outcomes[winner_idx].response.result;
    let mut minority_count = 0usize;

    for outcome in &outcomes {
        if &outcome.response.result == winner_result {
            outcome.provider.qos.record_success(outcome.latency_ms);
        } else {
            minority_count += 1;
            tracing::warn!(
                provider = %outcome.provider.endpoint,
                method = %request.method,
                chain_id,
                "quorum mismatch — penalising provider"
            );
            outcome.provider.qos.record_failure();
        }
    }

    if minority_count > 0 {
        tracing::info!(
            method = %request.method,
            chain_id,
            total = outcomes.len(),
            minority = minority_count,
            "quorum resolved"
        );
    }

    let winner = outcomes.swap_remove(winner_idx);
    Ok((winner.response, winner.provider))
}

/// Return the index in `outcomes` belonging to the majority result group.
/// Ties are broken in favour of the first candidate (lowest index).
fn majority_index(outcomes: &[ProviderOutcome]) -> usize {
    let n = outcomes.len();
    let mut best_idx = 0;
    let mut best_count = 0usize;

    for i in 0..n {
        let count = outcomes
            .iter()
            .filter(|o| o.response.result == outcomes[i].response.result)
            .count();
        if count > best_count {
            best_count = count;
            best_idx = i;
        }
    }

    best_idx
}

// ---------------------------------------------------------------------------
// CU weights
// ---------------------------------------------------------------------------

fn cu_weight_for(method: &str) -> u32 {
    match method {
        "eth_chainId" | "net_version" | "eth_blockNumber" => 1,
        "eth_getBalance" | "eth_getTransactionCount" | "eth_getCode" | "eth_getStorageAt"
        | "eth_sendRawTransaction" | "eth_getBlockByHash" | "eth_getBlockByNumber" => 5,
        "eth_call" | "eth_estimateGas" | "eth_getTransactionReceipt"
        | "eth_getTransactionByHash" => 10,
        "eth_getLogs" => 20,
        _ => 10,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_outcome(result: Option<Value>) -> ProviderOutcome {
        use crate::{qos::ProviderQos, registry::Provider};
        use alloy_primitives::Address;
        ProviderOutcome {
            response: JsonRpcResponse {
                jsonrpc: "2.0".into(),
                result,
                error: None,
                id: None,
            },
            provider: Arc::new(Provider {
                address: Address::ZERO,
                endpoint: String::new(),
                chains: vec![],
                qos: ProviderQos::default(),
            }),
            latency_ms: 10,
        }
    }

    #[test]
    fn majority_index_unanimous() {
        let outcomes = vec![
            make_outcome(Some(Value::String("0x1".into()))),
            make_outcome(Some(Value::String("0x1".into()))),
            make_outcome(Some(Value::String("0x1".into()))),
        ];
        assert_eq!(majority_index(&outcomes), 0);
    }

    #[test]
    fn majority_index_two_vs_one() {
        let outcomes = vec![
            make_outcome(Some(Value::String("0x1".into()))),
            make_outcome(Some(Value::String("0x2".into()))), // minority
            make_outcome(Some(Value::String("0x1".into()))),
        ];
        // indices 0 and 2 agree → winner is index 0
        assert_eq!(majority_index(&outcomes), 0);
    }

    #[test]
    fn majority_index_single_response() {
        let outcomes = vec![make_outcome(Some(Value::String("0xabc".into())))];
        assert_eq!(majority_index(&outcomes), 0);
    }

    #[test]
    fn requires_quorum_targets_correct_methods() {
        assert!(requires_quorum("eth_call"));
        assert!(requires_quorum("eth_getLogs"));
        assert!(!requires_quorum("eth_blockNumber"));
        assert!(!requires_quorum("eth_getBalance"));
        assert!(!requires_quorum("eth_sendRawTransaction"));
    }
}
