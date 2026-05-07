//! Per-provider QoS tracking.
//!
//! Score = 35% latency + 35% availability + 30% freshness.
//! All fields use atomics so they can be updated concurrently from the probe
//! task and the request handler without locks on the hot path.

use std::sync::atomic::{AtomicU64, Ordering};

/// QoS state for a single provider.
#[derive(Debug, Default)]
pub struct ProviderQos {
    /// Latency EMA stored as fixed-point: actual_ms = raw / LATENCY_SCALE.
    latency_ema_raw: AtomicU64,

    /// Total requests dispatched to this provider.
    requests_total: AtomicU64,
    /// Successful responses (HTTP 2xx, valid JSON-RPC).
    requests_success: AtomicU64,

    /// Latest block number observed from this provider (per-chain max stored here
    /// as the max across all chains for simplicity in Phase 1).
    latest_block: AtomicU64,
}

/// Per-chain globally-known chain head. Shared across all providers.
#[derive(Debug, Default)]
pub struct ChainHead {
    pub block_number: AtomicU64,
}

const LATENCY_SCALE: u64 = 1_000;
const LATENCY_ALPHA_NUM: u64 = 1; // alpha = 0.1 = 1/10
const LATENCY_ALPHA_DEN: u64 = 10;

impl ProviderQos {
    /// Compute a combined score in [0.0, 1.0].
    pub fn score(&self, chain_head: u64) -> f64 {
        0.35 * self.latency_score()
            + 0.35 * self.availability_score()
            + 0.30 * self.freshness_score(chain_head)
    }

    /// Record a successful response and update latency EMA.
    pub fn record_success(&self, latency_ms: u64) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
        self.requests_success.fetch_add(1, Ordering::Relaxed);
        self.update_latency_ema(latency_ms);
    }

    /// Record a failed response (timeout, connection refused, bad status).
    pub fn record_failure(&self) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
        // success counter unchanged
        // Penalise latency by injecting a worst-case sample
        self.update_latency_ema(5_000);
    }

    /// Update the latest observed block number for freshness scoring.
    pub fn update_latest_block(&self, block: u64) {
        let current = self.latest_block.load(Ordering::Relaxed);
        if block > current {
            self.latest_block.store(block, Ordering::Relaxed);
        }
    }

    // -------------------------------------------------------------------------
    // Individual metric scores
    // -------------------------------------------------------------------------

    fn latency_score(&self) -> f64 {
        let raw = self.latency_ema_raw.load(Ordering::Relaxed);
        if raw == 0 {
            return 1.0; // no measurements yet — optimistic
        }
        let ms = raw / LATENCY_SCALE;
        // Linear decay: 0ms → 1.0, 500ms → 0.0
        1.0 - (ms as f64 / 500.0).min(1.0)
    }

    fn availability_score(&self) -> f64 {
        let total = self.requests_total.load(Ordering::Relaxed);
        if total == 0 {
            return 1.0; // no history — optimistic
        }
        let success = self.requests_success.load(Ordering::Relaxed);
        success as f64 / total as f64
    }

    fn freshness_score(&self, chain_head: u64) -> f64 {
        if chain_head == 0 {
            return 1.0; // chain head unknown — optimistic
        }
        let latest = self.latest_block.load(Ordering::Relaxed);
        if latest == 0 {
            return 1.0; // no block observed yet — optimistic
        }
        let behind = chain_head.saturating_sub(latest) as f64;
        // Exponential decay: 0 behind → 1.0, 5 behind → ~0.37, 20 behind → ~0.02
        (-behind / 5.0).exp()
    }

    // -------------------------------------------------------------------------
    // EMA update
    // -------------------------------------------------------------------------

    fn update_latency_ema(&self, new_ms: u64) {
        let new_raw = new_ms * LATENCY_SCALE;
        // CAS loop: EMA = alpha * new + (1 - alpha) * current
        loop {
            let current = self.latency_ema_raw.load(Ordering::Relaxed);
            let updated = if current == 0 {
                new_raw
            } else {
                // Fixed-point EMA with alpha = 1/10
                (LATENCY_ALPHA_NUM * new_raw + (LATENCY_ALPHA_DEN - LATENCY_ALPHA_NUM) * current)
                    / LATENCY_ALPHA_DEN
            };
            if self
                .latency_ema_raw
                .compare_exchange_weak(current, updated, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_provider_scores_optimistic() {
        let qos = ProviderQos::default();
        assert_eq!(qos.score(1000), 1.0);
    }

    #[test]
    fn failure_reduces_availability() {
        let qos = ProviderQos::default();
        qos.record_success(20);
        qos.record_failure();
        // 1 success, 1 failure → 50% availability
        assert!((qos.availability_score() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn stale_provider_scores_lower_freshness() {
        let qos = ProviderQos::default();
        qos.update_latest_block(100);
        let fresh = qos.freshness_score(101); // 1 block behind
        let stale = qos.freshness_score(121); // 21 blocks behind
        assert!(fresh > stale);
    }

    #[test]
    fn fast_provider_scores_higher_latency() {
        let fast = ProviderQos::default();
        let slow = ProviderQos::default();
        for _ in 0..10 {
            fast.record_success(20);
        }
        for _ in 0..10 {
            slow.record_success(300);
        }
        assert!(fast.latency_score() > slow.latency_score());
    }
}
