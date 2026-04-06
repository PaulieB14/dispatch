pub mod health;
pub mod rpc;
pub mod ws;

use axum::Router;
use crate::server::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(health::router())
        .merge(rpc::router())
        .merge(ws::router())
        .with_state(state)
}
