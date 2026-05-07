//! GET /metrics — Prometheus text exposition.

use axum::{http::header, response::IntoResponse, routing::get, Router};

use crate::server::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/metrics", get(metrics_handler))
}

async fn metrics_handler() -> impl IntoResponse {
    let body = crate::metrics::render();
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}
