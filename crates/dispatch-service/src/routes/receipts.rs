/// GET /receipts/recent   — latest N receipts across all consumers
/// GET /receipts?payer=0x — latest N receipts for a specific consumer
use axum::{
    extract::{Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::{db, server::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/receipts/recent", get(recent_handler))
        .route("/receipts", get(by_payer_handler))
}

#[derive(Serialize)]
pub struct ReceiptItem {
    pub id: i64,
    pub payer: String,
    pub chain_id: i64,
    pub timestamp_ns: i64,
    pub value: String,
    pub method: Option<String>,
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
) -> Result<Json<Vec<ReceiptItem>>, StatusCode> {
    let pool = state
        .db_pool
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let limit = params.limit.unwrap_or(50).min(200) as i64;
    let rows = db::receipts::recent(pool, limit)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(rows.into_iter().map(into_item).collect()))
}

async fn by_payer_handler(
    State(state): State<AppState>,
    Query(params): Query<PayerQuery>,
) -> Result<Json<Vec<ReceiptItem>>, StatusCode> {
    let pool = state
        .db_pool
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let limit = params.limit.unwrap_or(50).min(200) as i64;
    let rows = db::receipts::by_payer_recent(pool, &params.payer, limit)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(rows.into_iter().map(into_item).collect()))
}

fn into_item(r: db::receipts::ReceiptRow) -> ReceiptItem {
    ReceiptItem {
        id: r.id,
        payer: r.payer_address,
        chain_id: r.chain_id,
        timestamp_ns: r.timestamp_ns,
        value: r.value,
        method: r.method,
    }
}
