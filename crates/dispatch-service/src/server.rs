use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, RwLock},
};

use alloy_primitives::Address;
use anyhow::Result;
use k256::ecdsa::SigningKey;
use tower_http::trace::TraceLayer;

use crate::{
    collector,
    config::Config,
    db,
    routes,
    tap_aggregator,
};

/// Shared application state — cheaply cloneable, lives for the process lifetime.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub http_client: reqwest::Client,
    /// Pre-computed EIP-712 domain separator for GraphTallyCollector.
    pub tap_domain_separator: alloy_primitives::B256,
    /// PostgreSQL pool. None if no database URL was configured.
    pub db_pool: Option<db::Pool>,
    /// Operator key — signs response attestations.
    pub signing_key: Arc<SigningKey>,
    /// Ethereum address derived from signing_key — embedded in attestations.
    pub signer_address: Address,
    /// Per-consumer accumulated unconfirmed receipt value (GRT wei).
    /// Reset for each consumer after a successful on-chain collect().
    pub consumer_credit: Arc<RwLock<HashMap<Address, u128>>>,
}

pub async fn run(config: Config) -> Result<()> {
    let tap_domain_separator = dispatch_tap::domain_separator(
        &config.tap.eip712_domain_name,
        config.tap.eip712_chain_id,
        config.tap.eip712_verifying_contract,
    );

    let signing_key = {
        let bytes = hex::decode(config.indexer.operator_private_key.trim_start_matches("0x"))?;
        SigningKey::from_slice(&bytes)?
    };
    let signer_address = dispatch_tap::address_from_key(&signing_key);

    let db_pool = if let Some(db_config) = &config.database {
        let pool = db::connect(&db_config.url).await?;
        db::run_migrations(&pool).await?;
        Some(pool)
    } else {
        tracing::warn!("no [database] configured — receipts will not be persisted");
        None
    };

    let consumer_credit: Arc<RwLock<HashMap<Address, u128>>> =
        Arc::new(RwLock::new(HashMap::new()));

    // Start background tasks if a database is configured.
    if let Some(ref pool) = db_pool {
        tap_aggregator::spawn(Arc::new(config.clone()), pool.clone());
        collector::spawn(Arc::new(config.clone()), pool.clone(), Arc::clone(&consumer_credit));
    }

    let state = AppState {
        config: Arc::new(config.clone()),
        http_client: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?,
        tap_domain_separator,
        db_pool,
        signing_key: Arc::new(signing_key),
        signer_address,
        consumer_credit,
    };

    let app = routes::router(state).layer(TraceLayer::new_for_http());

    let addr: SocketAddr = format!("{}:{}", config.server.host, config.server.port).parse()?;
    tracing::info!(%addr, "dispatch-service starting");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
