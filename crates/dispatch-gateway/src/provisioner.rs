//! Automatic escrow provisioning for newly discovered providers.
//!
//! On each cycle, checks the PaymentsEscrow balance for every provider in the
//! current registry. If a provider's balance is below the configured threshold,
//! the gateway payer wallet automatically approves and deposits the configured
//! amount.
//!
//! Deposits use `deposit(collector, receiver, amount)` — the argument order
//! expected by dispatch-service's escrow checker
//! (`getBalance(payer, collector, receiver)`).

use std::{sync::Arc, time::Duration};

use alloy::{network::EthereumWallet, providers::ProviderBuilder, signers::local::PrivateKeySigner, sol};
use alloy_primitives::{Address, U256};
use tokio::time::{interval, MissedTickBehavior};

use crate::{config::ProvisioningConfig, server::AppState};

sol! {
    #[sol(rpc)]
    interface IERC20 {
        function approve(address spender, uint256 amount) external returns (bool);
        function allowance(address owner, address spender) external view returns (uint256);
    }

    #[sol(rpc)]
    interface IPaymentsEscrow {
        function getBalance(
            address payer,
            address collector,
            address receiver
        ) external view returns (uint256 balance, uint256 thawEndTimestamp, uint256 thawingTokens);

        function deposit(
            address collector,
            address receiver,
            uint256 amount
        ) external;
    }
}

pub async fn run(state: AppState) {
    let Some(cfg) = state.config.provisioning.clone() else {
        tracing::info!("no [provisioning] config — escrow auto-provisioning disabled");
        return;
    };

    let signer: PrivateKeySigner = match cfg.gateway_payer_private_key.parse() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("provisioner: invalid gateway_payer_private_key: {e}");
            return;
        }
    };
    let payer_address = signer.address();

    let rpc_url: reqwest::Url = match cfg.arbitrum_rpc_url.parse() {
        Ok(u) => u,
        Err(e) => {
            tracing::error!("provisioner: invalid arbitrum_rpc_url: {e}");
            return;
        }
    };

    tracing::info!(
        interval_secs = cfg.interval_secs,
        %payer_address,
        "escrow provisioner started"
    );

    let cfg = Arc::new(cfg);
    let mut tick = interval(Duration::from_secs(cfg.interval_secs));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tick.tick().await;

        let provider_addresses: Vec<Address> = state
            .registry
            .load()
            .all_providers()
            .iter()
            .map(|p| p.address)
            .collect();

        if provider_addresses.is_empty() {
            continue;
        }

        if let Err(e) = provision(
            &provider_addresses,
            &cfg,
            payer_address,
            signer.clone(),
            rpc_url.clone(),
        )
        .await
        {
            tracing::warn!(error = %e, "provisioning cycle failed");
        }
    }
}

async fn provision(
    receivers: &[Address],
    cfg: &ProvisioningConfig,
    payer_address: Address,
    signer: PrivateKeySigner,
    rpc_url: reqwest::Url,
) -> anyhow::Result<()> {
    let wallet = EthereumWallet::from(signer);
    let provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .wallet(wallet)
        .on_http(rpc_url);

    let grt = IERC20::new(cfg.grt_token_address, &provider);
    let escrow = IPaymentsEscrow::new(cfg.escrow_address, &provider);

    let collector = cfg.collector_address;
    let deposit_amount = U256::from(cfg.deposit_per_provider);
    let min_threshold = U256::from(cfg.min_escrow_threshold);

    for &receiver in receivers {
        let balance = match escrow
            .getBalance(payer_address, collector, receiver)
            .call()
            .await
        {
            Ok(r) => r.balance,
            Err(e) => {
                tracing::warn!(%receiver, error = %e, "failed to check escrow balance");
                continue;
            }
        };

        if balance >= min_threshold {
            tracing::debug!(%receiver, %balance, "escrow sufficient");
            continue;
        }

        tracing::info!(
            %receiver,
            current_balance = %balance,
            depositing = %deposit_amount,
            "provisioning escrow"
        );

        // Ensure allowance covers the deposit.
        let allowance = grt
            .allowance(payer_address, cfg.escrow_address)
            .call()
            .await
            .map(|r| r._0)
            .unwrap_or(U256::ZERO);

        if allowance < deposit_amount {
            match grt
                .approve(cfg.escrow_address, U256::MAX)
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("approve send: {e}"))?
                .watch()
                .await
            {
                Ok(_) => tracing::debug!(%receiver, "GRT approval confirmed"),
                Err(e) => {
                    tracing::warn!(%receiver, error = %e, "GRT approval failed");
                    continue;
                }
            }
        }

        match escrow
            .deposit(collector, receiver, deposit_amount)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("deposit send: {e}"))?
            .watch()
            .await
        {
            Ok(_) => tracing::info!(%receiver, %deposit_amount, "escrow provisioned ✓"),
            Err(e) => tracing::warn!(%receiver, error = %e, "deposit failed"),
        }
    }

    Ok(())
}
