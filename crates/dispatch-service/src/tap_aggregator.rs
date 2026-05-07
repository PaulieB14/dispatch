//! Background RAV aggregation task.
//!
//! Runs on a configurable interval. For each distinct payer (gateway signer)
//! present in `tap_receipts`, it:
//!   1. Reconstructs all stored receipts as `SignedReceipt` structs.
//!   2. POSTs them to the gateway's `/rav/aggregate` endpoint.
//!   3. Upserts the returned `SignedRav` into `tap_ravs`.
//!
//! The RAV's `value_aggregate` equals the sum of ALL receipts ever sent in each
//! request — maintaining the monotonically-increasing invariant required by
//! GraphTallyCollector for on-chain collection.

use std::{sync::Arc, time::Duration};

use alloy_primitives::Bytes;

use crate::{
    config::Config,
    db::{
        receipts::{distinct_payers, fetch_by_payer, upsert_rav, RavRow},
        Pool,
    },
};

/// Spawn the aggregator loop. Returns immediately; the task runs until the
/// process exits.
pub fn spawn(config: Arc<Config>, pool: Pool) {
    let Some(url) = config.tap.aggregator_url.clone() else {
        tracing::info!("tap.aggregator_url not set — RAV aggregation disabled");
        return;
    };

    let interval = Duration::from_secs(config.tap.aggregation_interval_secs);
    tracing::info!(%url, interval_secs = interval.as_secs(), "RAV aggregator started");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("failed to build HTTP client");

    tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval).await;
            if let Err(e) = run_once(&url, &config, &pool, &client).await {
                tracing::warn!("RAV aggregation cycle failed: {e:#}");
            }
        }
    });
}

// ---------------------------------------------------------------------------
// One aggregation cycle
// ---------------------------------------------------------------------------

async fn run_once(
    aggregator_url: &str,
    config: &Config,
    pool: &Pool,
    client: &reqwest::Client,
) -> anyhow::Result<()> {
    let payers = distinct_payers(pool).await?;

    if payers.is_empty() {
        tracing::debug!("no receipts in db, skipping aggregation");
        return Ok(());
    }

    let service_provider = config.indexer.service_provider_address;
    let data_service = config.tap.data_service_address;
    let endpoint = format!("{aggregator_url}/rav/aggregate");

    for payer_hex in payers {
        if let Err(e) = aggregate_payer(
            pool,
            client,
            &endpoint,
            service_provider,
            data_service,
            &payer_hex,
        )
        .await
        {
            tracing::warn!(payer = %payer_hex, "RAV aggregation failed for payer: {e:#}");
        }
    }

    Ok(())
}
async fn aggregate_payer(
    pool: &Pool,
    client: &reqwest::Client,
    endpoint: &str,
    service_provider: alloy_primitives::Address,
    data_service: alloy_primitives::Address,
    payer_hex: &str,
) -> anyhow::Result<()> {
    let rows = fetch_by_payer(pool, payer_hex).await?;
    if rows.is_empty() {
        return Ok(());
    }

    let receipts: Vec<dispatch_tap::SignedReceipt> = rows
        .iter()
        .map(|row| {
            let value = row.value.parse::<u128>().unwrap_or(0);
            dispatch_tap::SignedReceipt {
                receipt: dispatch_tap::Receipt {
                    data_service,
                    service_provider,
                    timestamp_ns: row.timestamp_ns as u64,
                    nonce: row.nonce as u64,
                    value,
                    metadata: Bytes::from(row.metadata.clone()),
                },
                signature: row.signature.clone(),
            }
        })
        .collect();

    let payer: alloy_primitives::Address = payer_hex
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid payer address in db: {payer_hex}"))?;

    let body = serde_json::json!({
        "service_provider": service_provider,
        "payer": payer,
        "receipts": receipts,
    });

    let resp = client
        .post(endpoint)
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("POST {endpoint} failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("aggregator returned {status}: {text}");
    }

    let resp_json: serde_json::Value = resp.json().await?;
    let signed_rav: dispatch_tap::SignedRav =
        serde_json::from_value(resp_json["signed_rav"].clone())?;

    let rav = &signed_rav.rav;
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let collection_id_hex = format!("{:?}", rav.collection_id);
    let payer_hex_lower = format!("{:?}", rav.payer);
    let sp_hex = format!("{:?}", rav.service_provider);
    let ds_hex = format!("{:?}", rav.data_service);

    upsert_rav(
        pool,
        RavRow {
            collection_id: &collection_id_hex,
            payer_address: &payer_hex_lower,
            service_provider: &sp_hex,
            data_service: &ds_hex,
            timestamp_ns: rav.timestamp_ns as i64,
            value_aggregate: &rav.value_aggregate.to_string(),
            signature: &signed_rav.signature,
            last_updated: now_secs,
        },
    )
    .await?;

    tracing::info!(
        payer = %payer_hex,
        receipts = rows.len(),
        value_aggregate = %rav.value_aggregate,
        "RAV updated"
    );

    Ok(())
}
