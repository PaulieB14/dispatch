//! Dynamic provider discovery via The Graph subgraph.
//!
//! Polls the RPC network subgraph at a configurable interval and rebuilds the
//! provider registry from the response. Providers that disappear from the
//! subgraph (deregistered/inactive) are automatically removed.

use std::sync::Arc;

use serde::Deserialize;
use tokio::time::{interval, Duration, MissedTickBehavior};

use crate::{config::ProviderConfig, registry::Registry, server::AppState};

// ---------------------------------------------------------------------------
// Subgraph response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SubgraphResponse {
    data: SubgraphData,
}

#[derive(Deserialize)]
struct SubgraphData {
    indexers: Vec<IndexerEntry>,
}

#[derive(Deserialize)]
struct IndexerEntry {
    address: String,
    endpoint: String,
    chains: Vec<ChainEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChainEntry {
    chain_id: String,
}

// ---------------------------------------------------------------------------
// Discovery loop
// ---------------------------------------------------------------------------

pub async fn run(state: AppState) {
    let cfg = match &state.config.discovery {
        Some(c) => c.clone(),
        None => return, // No subgraph configured — use static providers only.
    };

    let mut tick = interval(Duration::from_secs(cfg.interval_secs));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

    tracing::info!(
        subgraph_url = %cfg.subgraph_url,
        interval_secs = cfg.interval_secs,
        "discovery task started"
    );

    loop {
        tick.tick().await;

        match fetch_providers(&state.http_client, &cfg.subgraph_url).await {
            Ok(providers) if !providers.is_empty() => {
                let new_registry = Registry::from_config(&providers);
                state.registry.store(Arc::new(new_registry));
                tracing::info!(count = providers.len(), "provider registry refreshed from subgraph");
            }
            Ok(_) => tracing::warn!("subgraph returned no active providers"),
            Err(e) => tracing::warn!(error = %e, "subgraph discovery failed"),
        }
    }
}

async fn fetch_providers(
    client: &reqwest::Client,
    subgraph_url: &str,
) -> anyhow::Result<Vec<ProviderConfig>> {
    let query = r#"{
        "query": "{ indexers(where: { registered: true }, first: 1000) { address endpoint chains(where: { active: true }) { chainId } } }"
    }"#;

    let resp: SubgraphResponse = client
        .post(subgraph_url)
        .header("Content-Type", "application/json")
        .body(query)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let mut providers = Vec::new();

    for indexer in resp.data.indexers {
        let address = match indexer.address.parse() {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!(address = %indexer.address, error = %e, "skipping indexer with invalid address");
                continue;
            }
        };

        let chains: Vec<u64> = indexer
            .chains
            .into_iter()
            .filter_map(|c| c.chain_id.parse::<u64>().ok())
            .collect();

        if chains.is_empty() {
            continue;
        }

        providers.push(ProviderConfig {
            address,
            endpoint: indexer.endpoint.trim_end_matches('/').to_string(),
            chains,
        });
    }

    Ok(providers)
}
