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
    /// On-chain `geoHash` field from ProviderRegistered — used as the routing region.
    #[serde(rename = "geoHash", default)]
    geo_hash: Option<String>,
    chains: Vec<ChainEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChainEntry {
    chain_id: String,
    /// Capability tier: 0=Standard 1=Archive 2=Debug 3=WebSocket
    tier: i32,
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
        "query": "{ indexers(where: { registered: true }, first: 1000) { address endpoint geoHash chains(where: { active: true }) { chainId tier } } }"
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

        // Build per-chain capability map from all active chain registrations.
        // A provider may have multiple registrations per chain (one per tier),
        // each independently activated via startService/stopService.
        let mut chain_caps: std::collections::HashMap<u64, Vec<crate::config::CapabilityTier>> =
            std::collections::HashMap::new();

        for c in &indexer.chains {
            let Ok(chain_id) = c.chain_id.parse::<u64>() else { continue };
            let tier = match c.tier {
                0 => Some(crate::config::CapabilityTier::Standard),
                1 => Some(crate::config::CapabilityTier::Archive),
                2 => Some(crate::config::CapabilityTier::Debug),
                _ => None, // tier 3 = WebSocket — transport layer, not a routing tier
            };
            if let Some(t) = tier {
                let entry = chain_caps.entry(chain_id).or_default();
                if !entry.contains(&t) {
                    entry.push(t);
                }
            }
        }

        if chain_caps.is_empty() {
            continue;
        }

        let chains: Vec<u64> = chain_caps.keys().copied().collect();
        // Global capabilities = union across all chains (used as fallback).
        let mut capabilities: Vec<crate::config::CapabilityTier> = Vec::new();
        for caps in chain_caps.values() {
            for t in caps {
                if !capabilities.contains(t) {
                    capabilities.push(t.clone());
                }
            }
        }

        providers.push(ProviderConfig {
            address,
            endpoint: indexer.endpoint.trim_end_matches('/').to_string(),
            chains,
            region: indexer.geo_hash.filter(|s| !s.is_empty()),
            capabilities,
            chain_capabilities: chain_caps,
        });
    }

    Ok(providers)
}
