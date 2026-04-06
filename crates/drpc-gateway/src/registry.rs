//! Provider registry — maps chain IDs to available providers.
//!
//! Phase 1: static configuration loaded at startup.
//! Phase 2: dynamic discovery by watching RPCDataService events on-chain
//!           and querying the RPC network subgraph.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use alloy_primitives::Address;

use crate::{config::{CapabilityTier, ProviderConfig}, qos::ProviderQos};

/// A registered RPC provider.
#[derive(Debug)]
pub struct Provider {
    pub address: Address,
    pub endpoint: String,
    pub chains: Vec<u64>,
    pub region: Option<String>,
    pub capabilities: Vec<CapabilityTier>,
    pub qos: ProviderQos,
}

impl Provider {
    fn from_config(cfg: &ProviderConfig) -> Self {
        Self {
            address: cfg.address,
            endpoint: cfg.endpoint.trim_end_matches('/').to_string(),
            chains: cfg.chains.clone(),
            region: cfg.region.clone(),
            capabilities: cfg.capabilities.clone(),
            qos: ProviderQos::default(),
        }
    }
}

/// Chain-level shared state (chain head block number, updated by probes).
#[derive(Debug, Default)]
pub struct ChainState {
    pub head: AtomicU64,
}

impl ChainState {
    /// Update the known chain head. Only stores if the new value is higher.
    pub fn update_head(&self, block: u64) {
        let current = self.head.load(Ordering::Relaxed);
        if block > current {
            self.head.store(block, Ordering::Relaxed);
        }
    }
}

/// The provider registry, shared across all request handlers and the probe task.
#[derive(Debug)]
pub struct Registry {
    /// All providers indexed for fast iteration during probing.
    all: Vec<Arc<Provider>>,
    /// chain_id → list of providers serving that chain.
    by_chain: HashMap<u64, Vec<Arc<Provider>>>,
    /// chain_id → shared chain state (head block).
    chain_state: HashMap<u64, Arc<ChainState>>,
}

impl Registry {
    pub fn from_config(providers: &[ProviderConfig]) -> Self {
        let all: Vec<Arc<Provider>> = providers.iter().map(|c| Arc::new(Provider::from_config(c))).collect();

        let mut by_chain: HashMap<u64, Vec<Arc<Provider>>> = HashMap::new();
        let mut chain_state: HashMap<u64, Arc<ChainState>> = HashMap::new();

        for provider in &all {
            for &chain_id in &provider.chains {
                by_chain.entry(chain_id).or_default().push(provider.clone());
                chain_state.entry(chain_id).or_insert_with(|| Arc::new(ChainState::default()));
            }
        }

        Self { all, by_chain, chain_state }
    }

    /// All registered providers (for probing).
    pub fn all_providers(&self) -> &[Arc<Provider>] {
        &self.all
    }

    /// Providers registered to serve a given chain, with their chain head.
    /// Returns `None` if the chain is unknown.
    pub fn providers_for_chain(&self, chain_id: u64) -> Option<(&[Arc<Provider>], u64)> {
        let providers = self.by_chain.get(&chain_id)?;
        let head = self
            .chain_state
            .get(&chain_id)
            .map(|s| s.head.load(Ordering::Relaxed))
            .unwrap_or(0);
        Some((providers, head))
    }

    pub fn chain_state(&self, chain_id: u64) -> Option<Arc<ChainState>> {
        self.chain_state.get(&chain_id).cloned()
    }
}
