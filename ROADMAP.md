# dRPC Data Service — Roadmap

Aligns with The Graph's 2026 Technical Roadmap ("Experimental JSON-RPC Data Service", Q3 2026).

---

## Phase 1 — MVP ✅ Complete

**Goal:** Prove the architecture. Minimal viable service on Horizon.

- [x] `RPCDataService.sol` — register, startService, stopService, collect, slash
- [x] `paymentsDestination` — decouple payment recipient from operator key
- [x] Explicit `QueryFee` enforcement in `collect()` — revert on other payment types
- [x] `drpc-service` (Rust) — JSON-RPC reverse proxy with TAP receipt validation
- [x] `drpc-gateway` (Rust) — QoS-aware routing, TAP receipt signing, metrics
- [x] RPC attestation scheme — `keccak256(method || params || response || blockHash)` signed by indexer
- [x] RPC network subgraph — indexes RPCDataService events for provider discovery
- [x] Integration tests — mock HorizonStaking only; real GraphTallyCollector/PaymentsEscrow/GraphPayments
- [x] EIP-712 cross-language compatibility tests (Solidity ↔ Rust)
- [x] Docker Compose full-stack deployment
- [x] GitHub Actions CI (Rust fmt/clippy/test + Solidity fmt/test)

---

## Phase 2 — Production Foundation ✅ Complete

Originally targeted Q4 2026. Completed ahead of schedule.

- [x] `eth_call` and `eth_getLogs` — multi-provider quorum consensus; minority providers penalised
- [x] 10+ chains — Ethereum, Arbitrum, Optimism, Base, Polygon, BNB, Avalanche, zkSync Era, Linea, Scroll
- [x] CU-weighted pricing — per-method compute units (1–20 CU); receipt value = CU × `base_price_per_cu`
- [x] QoS scoring — latency + availability + freshness, weighted random selection
- [x] Geographic routing — region-aware score bonus, proximity preference before latency data exists
- [x] Provider capability tiers — Standard / Archive / Debug; gateway filters by required tier per method
- [x] Dynamic provider discovery — subgraph-driven registry with configurable poll interval
- [x] Per-IP rate limiting — token-bucket via `governor`, configurable RPS + burst
- [x] Prometheus metrics — `drpc_requests_total`, `drpc_request_duration_seconds`
- [x] JSON-RPC batch support — concurrent dispatch, per-item error isolation

---

## Phase 3 — Full Feature Parity ✅ Largely Complete

Originally targeted Q1 2027.

- [x] WebSocket subscriptions — `eth_subscribe` / `eth_unsubscribe` proxied bidirectionally
- [x] Tier 1 fraud proof slashing — `slash()` with EIP-1186 MPT proofs via `StateProofVerifier.sol`
- [x] Block header trust oracle — `drpc-oracle` polls L1, submits state roots to Arbitrum for on-chain verification
- [ ] Archive tier routing — requires inspecting block parameters to detect archive requests (Phase 3 remainder)
- [ ] `debug_*` / `trace_*` routing — capability tier filter in place; subgraph schema extension needed for auto-discovery

---

## Phase 4 — Production Readiness (Q2 2027)

- [ ] TEE-based response verification
- [ ] Cross-chain unified endpoint
- [ ] P2P SDK for trustless consumer-provider connections (removes gateway trust assumption)
- [ ] GRT issuance rewards (requires governance approval + proof-of-work mechanism)
- [ ] Permissionless chain registration (with bond mechanism)
- [ ] Indexer agent TypeScript adaptation — `startService`/`stopService` automation
- [ ] Subgraph schema v2 — include region + capability tier for dynamic discovery
