# dRPC Data Service — Deliverables

## What Horizon already provides (reuse as-is)

| Component | Notes |
|---|---|
| HorizonStaking | Provision model maps directly to dRPC |
| GraphPayments | Generic fee distribution — no changes needed |
| PaymentsEscrow | Same escrow model for RPC payments |
| GraphTallyCollector | TAP v2 — already includes `data_service` field |
| DataService base contract | Inherit directly |
| DataServiceFees extension | Stake-backed fee locking via linked list |
| DataServicePausable extension | Emergency stop — inherit directly |
| indexer-tap-agent (Rust) | Receipt→RAV aggregation is service-agnostic |
| TAP middleware (indexer-service-rs) | Receipt validation pipeline is service-agnostic |
| PostgreSQL receipt/RAV schema | Reusable |
| Gateway payment infrastructure | Shared escrow/signer wallets |

## What must be adapted

| Component | Changes |
|---|---|
| `indexer-service-rs` | Fork: keep TAP middleware, replace GraphQL handler with JSON-RPC proxy, new routes (`/rpc/{chain_id}`), new cost model endpoint |
| `indexer-agent` (TypeScript) | Replace subgraph allocation logic with chain registration management (`startService`/`stopService` calls) |
| Gateway | Add JSON-RPC routing, RPC indexer discovery, per-method pricing, method classification (Tier 1/2/3), RPC dispatch |

## What must be built new

### 1. `RPCDataService.sol`
On-chain contract implementing `IDataService`. Inherits `DataService` + `DataServiceFees` + `DataServicePausable`.

Key responsibilities:
- `register` — validate provision (≥25,000 GRT/chain, ≥14d thawing), store provider metadata, set `paymentsDestination` (defaults to `serviceProvider`)
- `setPaymentsDestination` — decouple payment recipient from operator key (learnt from SubstreamsDataService; operators may use separate cold-storage wallets)
- `startService` — register provider for a `(chainId, capabilityTier, endpoint)` tuple
- `stopService` — deactivate chain registration
- `collect` — enforce `QueryFee` payment type explicitly (revert on others); decode `SignedRAV`, call `GraphTallyCollector`, route fees to `paymentsDestination`, lock stake at 5:1 ratio
- `slash` — Phase 2: Tier 1 fraud proof handling
- On-chain chain registry: `mapping(uint256 => ChainConfig)` with governance allowlist

### 2. `drpc-indexer-service` (Rust)
Fork of `indexer-service-rs`. Lightweight JSON-RPC reverse proxy.

Key responsibilities:
- TAP receipt validation (reuse `tap-middleware` wholesale)
- JSON-RPC request parsing and forwarding to backend Ethereum client
- RPC response attestation (sign `keccak256(method || params || response || blockHash)`)
- Optional Merkle proof attachment for Tier 1 methods (spot-checks / on-demand)
- Routes: `POST /rpc/{chain_id}`, `GET /health`, `GET /chains`, `GET /version`
- Stateless, horizontally scalable

### 3. RPC attestation scheme
Cryptographic proof that an indexer served a specific RPC response:
```
attestation = sign(keccak256(abi.encode(chainId, method, paramsHash, responseHash, blockNumber, blockHash)))
```
Signed with indexer's operator key. Enables dispute submission for Tier 1 fraud proofs.

### 4. RPC network subgraph
Indexes `RPCDataService` events for gateway discovery:
- `IndexerRegistered(address, uint256 chainId, uint8 tier, string endpoint)`
- `IndexerDeregistered(address, uint256 chainId)`
- Queryable: which indexers serve which chains at which capability tiers

### 5. Gateway RPC module
Extension to the existing `edgeandnode/gateway`:
- Detect JSON-RPC vs GraphQL — dispatch to appropriate pipeline
- RPC indexer discovery via network subgraph
- Method classification (Tier 1 / Tier 2 / Tier 3)
- CU cost computation per method
- Weighted random provider selection (QoS-scored)
- TAP receipt attachment with `data_service = RPCDataService address`
- Phase 1: simple latency + availability scoring
- Phase 2: Merkle proof verification for Tier 1, cross-referencing for Tier 2 spot-checks

### 6. Block header trust service
For Tier 1 Merkle proof verification, gateway needs trusted block hashes:
- Options: embedded light client, checkpoint service, or Ethereum consensus API
- Phase 1: checkpoint service (simpler); Phase 3: full light client

---

## Integration testing strategy

**Mock `HorizonStaking` only. Use production `GraphTallyCollector`, `PaymentsEscrow`, and `GraphPayments`.**

Learnt from `github.com/graphprotocol/substreams-data-service`: mocking the payment contracts hides EIP-712 hashing bugs, signer authorisation failures, and RAV replay issues that only surface against the real contracts. The staking mock is safe because provision validation logic is simple and deterministic.

Test environment: Anvil + `MockHorizonStaking` + `MockController` + production Horizon payment contracts. Rust integration tests should validate EIP-712 hash/signature compatibility between Rust signing code and Solidity verification, mirroring SubstreamsDataService's `rav_test.go` approach.

---

## Deployment model

Unit of service = `(chainId, capabilityTier)`:

| Tier | Methods | Infrastructure |
|---|---|---|
| 0 — Standard | All standard methods, last 128 blocks | Full node |
| 1 — Archive | Full historical state | Archive node (2–15+ TB) |
| 2 — Debug/Trace | `debug_*`, `trace_*` | Full/archive + debug APIs |
| 3 — WebSocket | `eth_subscribe`, real-time | Full node + WS endpoint |

Provider's provisioned stake is shared across all chains they serve (no per-chain stake splitting).

---

## Key parameters

| Parameter | Phase 1 value | Notes |
|---|---|---|
| Min provision per chain | 25,000 GRT | Governance-adjustable per chain |
| Thawing period | 14 days | Dispute window: 7–10 days |
| stakeToFeesRatio | 5 (5:1) | Consistent with SubgraphService |
| Max slash % | 10% (recommended 2.5%) | Tier 1 fraud proofs only |
| Chain ID type | uint256 (EIP-155) | Governance allowlist Phase 1 |
| Payment rate (Phase 1) | ~$40/M requests flat | CU-weighted pricing Phase 2 |
| TAP overhead target | <5ms | ECDSA verify ~0.1ms; acceptable |
