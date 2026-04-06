# dRPC Data Service — Roadmap

Aligns with The Graph's 2026 Technical Roadmap ("Experimental JSON-RPC Data Service", Q3 2026).

---

## Phase 1 — MVP (Q3 2026 research window)

**Goal:** Prove the architecture. Minimal viable service on Horizon.

- Chains: Ethereum mainnet + Arbitrum One + Optimism/Base
- Methods: Tier 1 Merkle-provable + essential Tier 2 quorum-verified (see RFC for method list)
- Payments: TAP/GraphTally as-is, flat rate ~$40/million requests
- On-chain: `RPCDataService.sol` — register, startService, stopService, collect, setPaymentsDestination
  - `paymentsDestination` mapping: decouple payment recipient from operator key (SubstreamsDataService pattern)
  - Explicit `QueryFee` enforcement in `collect()` — revert on other payment types
- Off-chain: `drpc-indexer-service` (Rust) — JSON-RPC proxy with TAP middleware
- Integration test suite: mock `HorizonStaking` only; use production `GraphTallyCollector` / `PaymentsEscrow` / `GraphPayments` (SubstreamsDataService approach — catches real EIP-712 and RAV bugs)
- No slashing disputes, no WebSocket, no archive, no debug/trace

## Phase 2 — Production Foundation (Q4 2026)

- `eth_call` and `eth_getLogs` with multi-provider consensus verification
- Expand to 10+ chains
- CU-weighted pricing (per-method compute units)
- Improved QoS scoring + geographic routing
- Granular dispute window configuration

## Phase 3 — Full Feature Parity (Q1 2027)

- WebSocket subscriptions (`eth_subscribe`)
- Archive tier support
- `debug_*` / `trace_*` methods
- Fraud proof slashing for Tier 1 disputes
- Chain-funded incentive pools (modelled on Lava Network)
- Permissionless chain registration (with bond mechanism)

## Phase 4 — Production Readiness (Q2 2027)

- TEE-based response verification options
- Cross-chain unified endpoint
- P2P SDK for trustless consumer-provider connections (removes gateway trust assumption)
- GRT issuance rewards (requires governance approval + proof-of-work mechanism)
