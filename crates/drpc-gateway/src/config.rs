use std::collections::HashMap;

use alloy_primitives::Address;
use anyhow::{Context, Result};
use serde::Deserialize;

/// RPC capability tier a provider can serve.
///
/// Providers declare which tiers they support; the gateway filters the candidate
/// pool to only providers capable of serving a given request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CapabilityTier {
    /// Standard full-node methods — last ~128 blocks.
    Standard,
    /// Full historical state — archive node required.
    Archive,
    /// `debug_*` and `trace_*` methods — debug API required.
    Debug,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub gateway: GatewayConfig,
    pub tap: TapConfig,
    pub qos: QosConfig,
    /// Static providers (used at startup or when no subgraph is configured).
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    /// Optional subgraph-based dynamic discovery.
    pub discovery: Option<DiscoveryConfig>,
    /// Optional per-IP rate limiting.
    pub rate_limit: Option<RateLimitConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct GatewayConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Geographic region of this gateway instance (e.g. "us-east", "eu-west").
    /// Used to prefer nearby providers before latency data is established.
    pub region: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TapConfig {
    /// Gateway operator private key (hex) — signs TAP receipts sent to providers.
    pub signer_private_key: String,
    /// RPCDataService contract address.
    pub data_service_address: Address,
    /// GRT wei charged per compute unit. Default ≈ $40/M requests at $0.09 GRT.
    #[serde(default = "default_base_price_per_cu")]
    pub base_price_per_cu: u128,
    /// EIP-712 domain name for GraphTallyCollector.
    pub eip712_domain_name: String,
    /// Chain ID where GraphTallyCollector is deployed (42161 = Arbitrum One).
    #[serde(default = "default_tap_chain_id")]
    pub eip712_chain_id: u64,
    /// GraphTallyCollector contract address.
    #[serde(default = "default_tap_verifying_contract")]
    pub eip712_verifying_contract: Address,
}

#[derive(Debug, Deserialize, Clone)]
pub struct QosConfig {
    /// How often to probe all providers with synthetic eth_blockNumber requests.
    #[serde(default = "default_probe_interval_secs")]
    pub probe_interval_secs: u64,
    /// Weights must sum to 1.0. Defaults match RFC spec.
    #[serde(default = "default_latency_weight")]
    pub latency_weight: f64,
    #[serde(default = "default_availability_weight")]
    pub availability_weight: f64,
    #[serde(default = "default_freshness_weight")]
    pub freshness_weight: f64,
    /// Number of providers to dispatch to concurrently (first response wins).
    #[serde(default = "default_concurrent_k")]
    pub concurrent_k: usize,
    /// Score bonus added for providers in the same region as this gateway.
    /// Helps bootstrap traffic to nearby providers before latency data exists.
    /// Default: 0.15 (adds 15 percentage points to same-region provider weight).
    #[serde(default = "default_region_bonus")]
    pub region_bonus: f64,
}

/// Static provider configuration.
/// Used as a fallback when no subgraph is configured, or as the initial set
/// before the first successful subgraph poll.
#[derive(Debug, Deserialize, Clone)]
pub struct ProviderConfig {
    /// Indexer's on-chain address (used as `service_provider` in TAP receipts).
    pub address: Address,
    /// Base URL of the indexer's drpc-service endpoint, e.g. "https://rpc.example.com".
    pub endpoint: String,
    /// Chain IDs this provider is registered to serve.
    pub chains: Vec<u64>,
    /// Geographic region of this provider (e.g. "us-east", "eu-west").
    /// Matched against `[gateway].region` for proximity-aware routing.
    pub region: Option<String>,
    /// Capability tiers this provider supports. Defaults to `[standard]`.
    #[serde(default = "default_capabilities")]
    pub capabilities: Vec<CapabilityTier>,
    /// Per-chain capability overrides, populated by dynamic discovery.
    /// When non-empty, used in place of the global `capabilities` field for
    /// per-chain tier filtering. Empty for static (TOML) provider configs.
    #[serde(default)]
    pub chain_capabilities: HashMap<u64, Vec<CapabilityTier>>,
}

/// Dynamic provider discovery via The Graph subgraph.
#[derive(Debug, Deserialize, Clone)]
pub struct DiscoveryConfig {
    /// GraphQL endpoint of the RPC network subgraph.
    pub subgraph_url: String,
    /// How often to poll the subgraph for provider updates (seconds).
    #[serde(default = "default_discovery_interval_secs")]
    pub interval_secs: u64,
}

/// Per-IP rate limiting for the RPC endpoint.
#[derive(Debug, Deserialize, Clone)]
pub struct RateLimitConfig {
    /// Steady-state requests per second per IP address.
    #[serde(default = "default_rps")]
    pub requests_per_second: u32,
    /// Burst capacity above the steady-state rate.
    #[serde(default = "default_burst")]
    pub burst: u32,
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = std::env::var("DRPC_GATEWAY_CONFIG")
            .unwrap_or_else(|_| "gateway.toml".to_string());
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read gateway config from {path}"))?;
        toml::from_str(&contents).context("failed to parse gateway config")
    }
}

fn default_capabilities() -> Vec<CapabilityTier> {
    vec![CapabilityTier::Standard]
}
fn default_region_bonus() -> f64 { 0.15 }
fn default_host() -> String { "0.0.0.0".to_string() }
fn default_port() -> u16 { 8080 }
fn default_base_price_per_cu() -> u128 { 4_000_000_000_000 } // 4e-6 GRT per CU
fn default_tap_chain_id() -> u64 { 42161 }
fn default_tap_verifying_contract() -> Address {
    "0x8f69F5C07477Ac46FBc491B1E6D91E2be0111A9e".parse().unwrap()
}
fn default_probe_interval_secs() -> u64 { 10 }
fn default_latency_weight() -> f64 { 0.30 }
fn default_availability_weight() -> f64 { 0.30 }
fn default_freshness_weight() -> f64 { 0.25 }
fn default_concurrent_k() -> usize { 3 }
fn default_discovery_interval_secs() -> u64 { 60 }
fn default_rps() -> u32 { 100 }
fn default_burst() -> u32 { 20 }
