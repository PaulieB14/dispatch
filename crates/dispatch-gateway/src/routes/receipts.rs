/// Proxy routes for the receipt feed API.
///
/// GET /receipts/recent?limit=N   — latest N receipts (all consumers)
/// GET /receipts?payer=0x&limit=N — latest N receipts for a specific consumer
///
/// Forwards to the configured `[service].url`. Returns 503 if not configured.
use axum::{
    extract::{Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::Value;

use crate::server::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/receipts/recent", get(recent_handler))
        .route("/receipts", get(by_payer_handler))
}

#[derive(Deserialize)]
struct LimitQuery {
    limit: Option<u32>,
}

#[derive(Deserialize)]
struct PayerQuery {
    payer: String,
    limit: Option<u32>,
}

async fn recent_handler(
    State(state): State<AppState>,
    Query(params): Query<LimitQuery>,
) -> Result<Json<Value>, StatusCode> {
    let base_url = service_url(&state)?;
    let resp = state
        .http_client
        .get(format!("{base_url}/receipts/recent"))
        .query(&[("limit", params.limit.unwrap_or(50).to_string())])
        .send()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;

    if !resp.status().is_success() {
        return Err(StatusCode::BAD_GATEWAY);
    }

    let body: Value = resp.json().await.map_err(|_| StatusCode::BAD_GATEWAY)?;
    Ok(Json(body))
}

async fn by_payer_handler(
    State(state): State<AppState>,
    Query(params): Query<PayerQuery>,
) -> Result<Json<Value>, StatusCode> {
    let base_url = service_url(&state)?;
    let resp = state
        .http_client
        .get(format!("{base_url}/receipts"))
        .query(&[
            ("payer", params.payer.as_str()),
            ("limit", &params.limit.unwrap_or(50).to_string()),
        ])
        .send()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;

    if !resp.status().is_success() {
        return Err(StatusCode::BAD_GATEWAY);
    }

    let body: Value = resp.json().await.map_err(|_| StatusCode::BAD_GATEWAY)?;
    Ok(Json(body))
}

fn service_url(state: &AppState) -> Result<String, StatusCode> {
    state
        .config
        .service
        .as_ref()
        .map(|s| s.url.clone())
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)
}
