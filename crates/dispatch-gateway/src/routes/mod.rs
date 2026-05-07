pub mod aggregate;
pub mod health;
pub mod metrics;
pub mod receipts;
pub mod rpc;
pub mod ws;

use crate::server::AppState;
use axum::Router;

pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(aggregate::router())
        .merge(health::router())
        .merge(metrics::router())
        .merge(receipts::router())
        .merge(rpc::router())
        .merge(ws::router())
        .with_state(state)
}
