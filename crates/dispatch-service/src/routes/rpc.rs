use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};

use crate::{
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

    tracing::debug!(method = %request.method, chain_id, "dispatching");

    // --- Forward to backend Ethereum client ---
    let response = proxy::forward(&state.http_client, &backend_url, &request).await?;

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

    Ok(Json(response).into_response())
}
