use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};

use crate::{
    attestation,
    db,
    error::ServiceError,
    rpc::{proxy, types::JsonRpcRequest},
    server::AppState,
    tap,
};

pub fn router() -> Router<AppState> {
    Router::new().route("/rpc/:chain_id", post(rpc_handler))
}

async fn rpc_handler(
    State(state): State<AppState>,
    Path(chain_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<JsonRpcRequest>,
) -> Result<Response, ServiceError> {
    request.validate()?;

    let backend_url = state
        .config
        .chains
        .backends
        .get(&chain_id.to_string())
        .ok_or(ServiceError::UnsupportedChain(chain_id))?
        .clone();

    // --- TAP receipt validation ---
    let receipt_header = headers
        .get("TAP-Receipt")
        .ok_or(ServiceError::MissingReceipt)?
        .to_str()
        .map_err(|_| ServiceError::InvalidReceipt("non-UTF8 TAP-Receipt header".to_string()))?;

    let now_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;

    let validated = tap::validate_receipt(
        receipt_header,
        state.tap_domain_separator,
        &state.config.tap.authorized_senders,
        state.config.tap.data_service_address,
        state.config.indexer.service_provider_address,
        state.config.tap.max_receipt_age_ns,
        now_ns,
    )?;

    // --- Credit limit check ---
    {
        let credit = state.consumer_credit.read().unwrap();
        let served = credit.get(&validated.signer).copied().unwrap_or(0);
        if served >= state.config.tap.credit_threshold {
            tracing::warn!(
                signer = %validated.signer,
                served,
                threshold = state.config.tap.credit_threshold,
                "consumer credit limit reached"
            );
            return Err(ServiceError::CreditLimitExceeded);
        }
    }

    tracing::debug!(method = %request.method, chain_id, "dispatching");

    // --- Forward to backend Ethereum client ---
    let response = proxy::forward(&state.http_client, &backend_url, &request).await?;

    // --- Increment consumer credit ---
    {
        let mut credit = state.consumer_credit.write().unwrap();
        *credit.entry(validated.signer).or_insert(0) += validated.receipt.value;
    }

    // --- Persist receipt (non-fatal if DB is unavailable) ---
    if let Some(pool) = &state.db_pool {
        if let Err(e) = db::receipts::insert(pool, chain_id, &validated).await {
            tracing::warn!(
                error = %e,
                signer = %validated.signer,
                chain_id,
                "failed to persist TAP receipt"
            );
        }
    }

    // --- Sign the response ---
    let params_json = serde_json::to_string(&request.params).unwrap_or_else(|_| "null".to_string());
    let result_json = match (&response.result, &response.error) {
        (Some(r), _) => serde_json::to_string(r).unwrap_or_else(|_| "null".to_string()),
        (_, Some(e)) => serde_json::to_string(e).unwrap_or_else(|_| "null".to_string()),
        _ => "null".to_string(),
    };

    let mut resp = Json(response).into_response();

    match attestation::sign(
        &state.signing_key,
        state.signer_address,
        chain_id,
        &request.method,
        &params_json,
        &result_json,
    ) {
        Ok(att) => {
            if let Ok(header_val) = serde_json::to_string(&att)
                .map_err(|e| e.to_string())
                .and_then(|s| s.parse().map_err(|e: axum::http::header::InvalidHeaderValue| e.to_string()))
            {
                resp.headers_mut().insert("x-drpc-attestation", header_val);
            }
        }
        Err(e) => tracing::warn!(error = %e, "failed to sign response"),
    }

    Ok(resp)
}
