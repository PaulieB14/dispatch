/// POST /rav/aggregate
///
/// The indexer (service) calls this endpoint periodically to aggregate its stored
/// receipts into a signed RAV. The gateway:
///   1. Verifies each receipt was signed by itself.
///   2. Sums the values and takes the max timestamp.
///   3. Signs and returns a ReceiptAggregateVoucher (RAV).
///
/// The RAV is cumulative: `value_aggregate` equals the sum of ALL receipts sent
/// in this request. The caller is responsible for passing all receipts (including
/// those from previous rounds) to maintain the monotonic guarantee required
/// by GraphTallyCollector.
use axum::{extract::{DefaultBodyLimit, State}, routing::post, Json, Router};
use serde::{Deserialize, Serialize};

use alloy_primitives::{Address, Bytes};
use dispatch_tap::{collection_id, eip712_hash, recover_signer, sign_rav, Rav, SignedRav, SignedReceipt};

use crate::{error::GatewayError, server::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/rav/aggregate", post(aggregate_handler))
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024)) // 64 MB — receipt batches can be large
}

#[derive(Debug, Deserialize)]
pub struct AggregateRequest {
    /// The indexer's on-chain service provider address.
    pub service_provider: Address,
    /// The consumer (payer) address whose escrow will be debited on-chain.
    /// When present, the RAV will have `payer = consumer_address` so the
    /// correct escrow account is charged. Omit only for backwards compatibility
    /// (falls back to the gateway's own signer address).
    pub payer: Option<Address>,
    /// All receipts to include in this RAV (for full cumulative aggregation, include
    /// all historical receipts, not just new ones).
    pub receipts: Vec<SignedReceipt>,
}

#[derive(Debug, Serialize)]
pub struct AggregateResponse {
    pub signed_rav: SignedRav,
}

async fn aggregate_handler(
    State(state): State<AppState>,
    Json(req): Json<AggregateRequest>,
) -> Result<Json<AggregateResponse>, GatewayError> {
    if req.receipts.is_empty() {
        return Err(GatewayError::InvalidRequest("receipts batch is empty".into()));
    }

    let data_service = state.config.tap.data_service_address;
    // Use the consumer-provided payer if present; fall back to gateway's own address
    // for backwards-compatible callers that pre-date the consumer-pays model.
    let payer = req.payer.unwrap_or(state.signer_address);
    let domain_sep = state.tap_domain_separator;

    let mut value_aggregate: u128 = 0;
    let mut timestamp_ns: u64 = 0;

    for signed in &req.receipts {
        let r = &signed.receipt;

        if r.data_service != data_service {
            return Err(GatewayError::InvalidRequest(format!(
                "receipt data_service mismatch: expected {data_service}, got {}",
                r.data_service
            )));
        }
        if r.service_provider != req.service_provider {
            return Err(GatewayError::InvalidRequest(format!(
                "receipt service_provider mismatch: expected {}, got {}",
                req.service_provider, r.service_provider
            )));
        }

        // Verify the receipt was signed by the gateway itself.
        // Note: receipts are signed by the gateway's signing key, not by the payer (consumer).
        let hash = eip712_hash(domain_sep, r);
        let recovered = recover_signer(hash, &signed.signature)
            .map_err(|e| GatewayError::InvalidRequest(format!("invalid receipt signature: {e}")))?;
        if recovered != state.signer_address {
            return Err(GatewayError::InvalidRequest(format!(
                "receipt not signed by this gateway: signer={recovered}"
            )));
        }

        value_aggregate = value_aggregate.saturating_add(r.value);
        timestamp_ns = timestamp_ns.max(r.timestamp_ns);
    }

    let cid = collection_id(payer, req.service_provider, data_service);

    let rav = Rav {
        collection_id: cid,
        payer,
        service_provider: req.service_provider,
        data_service,
        timestamp_ns,
        value_aggregate,
        metadata: Bytes::default(),
    };

    let signed_rav = sign_rav(&state.signing_key, domain_sep, rav)
        .map_err(|e| GatewayError::InvalidRequest(format!("RAV signing failed: {e}")))?;

    tracing::info!(
        service_provider = %req.service_provider,
        receipts = req.receipts.len(),
        value_aggregate,
        "issued signed RAV"
    );

    Ok(Json(AggregateResponse { signed_rav }))
}
