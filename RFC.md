# RFC: dRPC Data Service on The Graph Horizon

**Status:** Implemented (see codebase)
**Target:** Q3 2026 experimental window
**Authors:** TBD
**Based on:** GIP-0066 (Horizon), GIP-0054 (GraphTally), GIP-0042 (World of Data Services)

---

## 1. Summary

This RFC describes a decentralised JSON-RPC (dRPC) data service built on The Graph Protocol's Horizon framework. It defines the on-chain contract interface, off-chain indexer service architecture, payment model, verification framework, and gateway integration required to serve Ethereum-compatible JSON-RPC requests under Horizon's economic security model.

The Graph's Horizon upgrade (mainnet December 2025, GIP-0066) explicitly supports this as the second production data service after SubgraphService. The 2026 Technical Roadmap names an "Experimental JSON-RPC Data Service" for Q3 2026.

---

## 2. Background

### 2.1 Horizon architecture

Horizon restructures the Graph Protocol into three independent layers:

**HorizonStaking** — provisions assign stake to a specific `(serviceProvider, dataService)` pair. The data service contract is the slashing authority for that provision.

**GraphPayments + PaymentsEscrow** — generic micropayment infrastructure. GraphTally (TAP v2) handles off-chain receipt accumulation and on-chain RAV redemption. `valueAggregate` in a RAV is cumulative and never resets; the collector tracks previously collected amounts to calculate deltas.

**DataService framework** — abstract Solidity contracts (`DataService`, `DataServiceFees`, `DataServicePausable`) providing composable building blocks. Any developer can deploy a new data service permissionlessly; governance approval is only required for GRT issuance rewards.

### 2.2 IDataService interface

```solidity
interface IDataService {
    function register(address serviceProvider, bytes calldata data) external;
    function deregister(address serviceProvider, bytes calldata data) external;
    function startService(address serviceProvider, bytes calldata data) external;
    function stopService(address serviceProvider, bytes calldata data) external;
    function collect(address serviceProvider, bytes calldata data) external returns (uint256);
    function slash(address serviceProvider, bytes calldata data) external;
    function acceptProvisionPendingParameters(address serviceProvider, bytes calldata data) external;
}
```

Lifecycle: `register → startService → [collect]* → stopService → deregister`

### 2.3 GraphTally payment lifecycle

1. Gateway deposits GRT into `PaymentsEscrow` per service provider
2. Per query: gateway sends signed TAP receipt (EIP-712 ECDSA; nonce = random uint64; value in GRT wei)
3. Receipts accumulate off-chain in indexer's PostgreSQL
4. TAP agent batches receipts, sends to gateway's `tap_aggregator` endpoint (JSON-RPC), receives signed RAV
5. RAV submitted on-chain: `DataService.collect()` → `GraphTallyCollector` → `PaymentsEscrow` → `GraphPayments`
6. Distribution: protocol tax → data service cut → delegator cut → service provider

**EIP-712 domain** (all Horizon contracts, Arbitrum One):
```
name: protocol-configured
version: "1"
chainId: 42161
verifyingContract: 0x8f69F5C07477Ac46FBc491B1E6D91E2be0111A9e  // GraphTallyCollector
```

### 2.4 TAP v2 receipt/RAV types

```rust
struct Receipt {
    address data_service;       // RPCDataService address
    address service_provider;   // Indexer address
    uint64  timestamp_ns;       // Unix nanoseconds
    uint64  nonce;              // Random uint64
    uint128 value;              // GRT wei
    bytes   metadata;           // Optional: CU count, method hash
}

struct ReceiptAggregateVoucher {
    address data_service;
    address service_provider;
    uint64  timestamp_ns;       // Max timestamp from batch
    uint128 value_aggregate;    // Cumulative, monotonically increasing
    bytes   metadata;
}
```

---

## 3. Deployment model

### 3.1 Unit of service

Unlike subgraphs (unique data transformations), RPC is generic per chain. The unit of service is a `(chainId, capabilityTier)` pair:

| Tier | Value | Methods | Infrastructure |
|---|---|---|---|
| Standard | 0 | All standard methods, last 128 blocks | Full node |
| Archive | 1 | Full historical state | Archive node |
| Debug/Trace | 2 | `debug_*`, `trace_*` | Full/archive + debug APIs |
| WebSocket | 3 | `eth_subscribe`, real-time | Full node + WS |

A provider's provisioned stake is shared across all chains they serve. No per-chain stake splitting.

### 3.2 Phase 1 scope

Chains: Ethereum mainnet (1), Arbitrum One (42161), Optimism (10), Base (8453).
Tiers: Standard only.
Methods: Tier 1 Merkle-provable + essential Tier 2 quorum-verified (see §5).

---

## 4. On-chain contract: RPCDataService

### 4.1 Contract structure

```solidity
contract RPCDataService is DataService, DataServicePausable, DataServiceFees {

    struct ChainConfig {
        bool     enabled;
        uint256  minProvisionTokens;   // Per-chain minimum (e.g. 25,000 GRT)
        string   metadata;
    }

    struct ChainRegistration {
        uint64  chainId;
        uint8   capabilityTier;
        string  endpoint;
        bool    active;
    }

    // Governance-controlled allowlist
    mapping(uint256 => ChainConfig) public supportedChains;

    // Provider state
    mapping(address => bool)                  public registeredProviders;
    mapping(address => ChainRegistration[])   public providerChains;
    mapping(address => address)               public paymentsDestination; // provider → payment recipient

    // Protocol parameters
    uint256 public minimumProvisionTokens;   // Default: 25,000 GRT
    uint64  public minimumThawingPeriod;     // Default: 14 days
    uint256 public stakeToFeesRatio;         // Default: 5

    function register(address serviceProvider, bytes calldata data) external override;
    function deregister(address serviceProvider, bytes calldata data) external override;
    function startService(address serviceProvider, bytes calldata data) external override;
    function stopService(address serviceProvider, bytes calldata data) external override;
    function collect(address serviceProvider, bytes calldata data) external override returns (uint256);
    function slash(address serviceProvider, bytes calldata data) external override;     // EIP-1186 MPT fraud proof
    function acceptProvisionPendingParameters(address sp, bytes calldata) external override;
    function setPaymentsDestination(address destination) external;   // Decouple payment recipient from operator key

    // Governance
    function addChain(uint256 chainId, ChainConfig calldata config) external onlyGovernor;
    function removeChain(uint256 chainId) external onlyGovernor;
}
```

> **Learnt from SubstreamsDataService:** The `paymentsDestination` mapping decouples the service provider's on-chain identity (operator key, used for signing) from the address that receives collected GRT. Operators running multiple services or using cold-storage payment wallets need this separation. SubstreamsDataService implements it as a simple `setPaymentsDestination()` call post-registration.

### 4.2 Function specifications

**register(serviceProvider, data)**
- `data`: `abi.encode(string endpoint, string geoHash, address paymentsDestination)`
- Validates: provision ≥ `minimumProvisionTokens`, thawing period ≥ `minimumThawingPeriod`
- Stores provider metadata; sets `paymentsDestination[serviceProvider]` (defaults to `serviceProvider` if zero address)
- Emits `ServiceProviderRegistered`

**setPaymentsDestination(destination)**
- Callable by a registered provider (or their authorised operator) at any time
- Updates `paymentsDestination[msg.sender]` — takes effect on the next `collect()` call
- Emits `PaymentsDestinationSet(serviceProvider, destination)`

**startService(serviceProvider, data)**
- `data`: `abi.encode(uint64 chainId, uint8 tier, string endpoint)`
- Validates: `chainId` in `supportedChains`, provider registered
- Pushes to `providerChains[serviceProvider]`
- Emits `ServiceStarted`

**stopService(serviceProvider, data)**
- `data`: `abi.encode(uint64 chainId, uint8 tier)`
- Sets `active = false` on matching registration
- Emits `ServiceStopped`

**collect(serviceProvider, data)**
- `data`: ABI-encoded `(SignedRAV, uint256 tokensToCollect)`
- Reverts with `InvalidPaymentType` if `paymentType != QueryFee` — explicit enforcement, not silent pass-through (learnt from SubstreamsDataService)
- Calls `GraphTallyCollector.collect()` to verify EIP-712 sig and pull from escrow
- Routes collected GRT to `paymentsDestination[serviceProvider]` (not necessarily `serviceProvider`)
- Locks `fees * stakeToFeesRatio` via `_createStakeClaim()` with `releaseAt = block.timestamp + thawingPeriod`
- Routes through `GraphPayments` for distribution
- Returns tokens collected

**slash(serviceProvider, data)**
- `data`: ABI-encoded `Tier1FraudProof` — block hash, account, dispute type (Balance/Nonce/Storage), claimed value, EIP-1186 account + storage proofs, challenger address
- Looks up trusted state root via `trustedStateRoots[blockHash]` (populated by `drpc-oracle`)
- Verifies proof via `StateProofVerifier.verifyAccount` / `verifyStorage` (OZ MPT library)
- Calls `HorizonStaking.slash(serviceProvider, slashAmount, verifierCut, challenger)` — 50% bounty to challenger

### 4.3 Provision parameters

| Parameter | Value |
|---|---|
| Minimum provision | 25,000 GRT per chain |
| Minimum thawing period | 14 days |
| stakeToFeesRatio | 5 |
| Max slash percentage | 10% (recommended 2.5%) |
| maxVerifierCut | Set by service provider, ≤ protocol max |

### 4.4 Deployment

- Deploy on Arbitrum One (all Horizon contracts live here)
- Constructor receives `GraphDirectory` immutable address book
- Governance role: multisig or timelock for chain allowlist management

---

## 5. Verification framework

RPC response verification is fundamentally different from subgraph POIs. A three-tier model is used.

### Tier 1 — Merkle-provable methods

Methods where correctness is verifiable via Ethereum's Merkle-Patricia trie:

| Method | Proof source | Notes |
|---|---|---|
| `eth_getBalance` | `accountProof` → balance field | Single `eth_getProof` call |
| `eth_getTransactionCount` | `accountProof` → nonce field | Same proof |
| `eth_getStorageAt` | `storageProof` → value | |
| `eth_getCode` | `accountProof` → codeHash; verify `keccak256(code) == codeHash` | Code fetched separately |
| `eth_getProof` | Self-verifying | Returns the proof itself |
| `eth_getBlockByHash` | `keccak256(RLP(header)) == hash` | Header-verifiable |
| `eth_getBlockByNumber` | Same once hash resolved | |

**EIP-1186 verification algorithm:**
1. Confirm `keccak256(RLP(header)) == trustedBlockHash`
2. Walk `accountProof` from header's `stateRoot` to leaf keyed by `keccak256(address)`
3. Verify extracted account RLP matches `[nonce, balance, storageHash, codeHash]`
4. For storage: walk `storageProof` from `storageHash` to leaf keyed by `keccak256(storageKey)`

**Performance:** Proofs are 500B–5KB, generation takes 5–20ms. Default: sign all responses, attach proofs on-demand or during random spot-checks. NOT on every request.

**Fraud proof slashing:** Challenger submits a `Tier1FraudProof` with the disputed block hash, EIP-1186 account/storage proofs, and claimed vs actual value. On-chain arbitration verifies via `StateProofVerifier.sol`. If valid: 10,000 GRT slashed, 50% to challenger. `drpc-oracle` maintains the `trustedStateRoots` mapping that makes verification possible.

**Competitive differentiation:** No existing dRPC protocol (Lava, Pocket, DRPC, Fluence) uses Merkle proofs. This is a significant differentiator.

### Tier 2 — Quorum-verified methods

Methods that are deterministic but require EVM re-execution to verify:

| Method | Priority |
|---|---|
| `eth_chainId`, `net_version` | P0 (static; trivially verified) |
| `eth_blockNumber` | P0 (quorum + beacon chain comparison) |
| `eth_sendRawTransaction` | P0 (self-authenticated; just relay) |
| `eth_getTransactionReceipt` | P0 (quorum; future: receipt trie proof) |
| `eth_call` | P1 (quorum; future: EVM re-execution) |
| `eth_estimateGas` | P1 (quorum sufficient; advisory) |
| `eth_gasPrice` | P1 (derivable from block headers) |
| `eth_getLogs` | P1 (quorum; future: bloom filter + receipt proof) |
| `eth_getTransactionByHash` | P1 (quorum; future: tx trie proof) |
| `eth_feeHistory` | P2 (derivable from verified block headers) |

Gateway sends request to N providers; majority response wins. Dispute triggers re-execution by additional randomly-selected providers. Phase 2: EVM re-execution disputes with economic consequences.

### Tier 3 — Non-deterministic methods

`eth_estimateGas`, `eth_gasPrice`, `eth_maxPriorityFeePerGas` — implementation-specific. No deterministic disputes possible. Reputation scoring only: providers returning statistically anomalous results receive lower QoS scores and reduced traffic. No slashing.

---

## 6. Payment model

### 6.1 GraphTally integration

GraphTally works as-is. The `data_service` field in TAP v2 receipts points to `RPCDataService` address, preventing cross-service replay.

TAP receipt overhead must remain **<5ms** per request:
- ECDSA signature verification: ~0.1ms
- Receipt storage (async): ~0ms on critical path
- Acceptable.

### 6.2 CU-weighted pricing

CU-weighted pricing is implemented. Compute unit weights:

| Method category | CU weight |
|---|---|
| `eth_chainId`, `net_version`, `eth_blockNumber` | 1 |
| `eth_getBalance`, `eth_getTransactionCount`, `eth_getCode`, `eth_getStorageAt` | 5 |
| `eth_sendRawTransaction` | 5 |
| `eth_getBlockByHash/Number` (header) | 5 |
| `eth_call`, `eth_estimateGas`, `eth_getTransactionReceipt`, `eth_getTransactionByHash` | 10 |
| `eth_getLogs` (bounded) | 20 |
| `debug_traceTransaction` | 500+ |

Target: $4–8/million CUs. Average mix (~10 CU/request) = $40–80/million requests.

### 6.3 WebSocket subscriptions (Phase 3)

Each push event (`newHeads`, log notification) generates one receipt. Long-lived subscriptions: per-event pricing aligned with existing TAP model.

---

## 7. QoS framework

### 7.1 Metrics

| Metric | Weight | Target |
|---|---|---|
| Latency | 30% | p50 <50ms, p95 <200ms |
| Availability | 30% | >99.9% rolling 24h |
| Data freshness | 25% | Within 1 block of chain head |
| Correctness | 15% | Spot-check pass rate |

### 7.2 Provider selection

Weighted random selection — higher QoS scores receive more traffic but not exclusively, enabling new provider discovery. Geographic routing sends requests to the closest provider.

Probe system: gateway sends synthetic `eth_blockNumber` to all providers every 10 seconds.

### 7.3 Failover

Concurrent dispatch to up to 3 providers. First valid response wins. If primary exceeds latency threshold, retry with next-best provider.

---

## 8. Off-chain indexer stack

### 8.1 drpc-indexer-service (Rust)

Fork of `indexer-service-rs`. Stateless, horizontally scalable.

**Reused:**
- TAP middleware (`tap-middleware` crate) — receipt validation is service-agnostic
- Configuration framework
- Metrics infrastructure

**Replaced:**
- GraphQL query handler → JSON-RPC proxy to Ethereum client
- Agora cost model → CU-weighted per-method pricing
- Subgraph routes → `/rpc/{chain_id}`, `/health`, `/chains`, `/version`
- GraphQL attestation → RPC attestation (see below)

**Request flow:**
```
Gateway → POST /rpc/{chain_id} + TAP-Receipt header
  → TAP middleware: validate receipt signature, sender, value, timestamp
  → Parse JSON-RPC method and params
  → Forward to backend Ethereum client
  → Sign response: keccak256(chainId || method || paramsHash || responseHash || blockHash)
  → Return response + attestation
```

### 8.2 RPC attestation scheme

```
attestation = sign(keccak256(abi.encode(
    chainId,
    keccak256(bytes(method)),
    keccak256(params),
    keccak256(response),
    blockNumber,
    blockHash
)))
```

Signed with indexer's operator key. Enables dispute submission for Tier 1 fraud proofs. Attached to every response as an HTTP header.

### 8.3 indexer-tap-agent (Rust)

Reused as-is. The agent is payment-protocol-generic. Adjustments:
- Update `data_service` address to `RPCDataService`
- Tune aggregation thresholds upward for RPC's higher request volume (10–100× subgraphs)

### 8.4 indexer-agent (TypeScript)

Adapted from `graphprotocol/indexer`. Replace subgraph allocation management with chain registration management:
- Monitor which chains the provider's nodes support (sync state, peer count, disk space)
- Call `RPCDataService.startService()` / `stopService()` accordingly
- Manage RAV redemption scheduling

### 8.5 Blockchain node infrastructure

Providers run standard Ethereum clients (Geth, Erigon, Reth, Nethermind). Many Graph indexers already operate full/archive nodes — this is a natural extension.

---

## 9. Gateway integration

### 9.1 Discovery

1. **On-chain**: `RPCDataService` emits `IndexerRegistered(address, chainId, tier, endpoint)` / `IndexerDeregistered` events
2. **RPC network subgraph**: indexes these events into queryable schema
3. **Gateway**: queries subgraph to build routing table; probes endpoints for liveness

### 9.2 Request routing

New routes in `edgeandnode/gateway`:
- `POST /rpc/{chain_id}` — standard JSON-RPC
- `POST /rpc/{chain_id}/ws` — WebSocket (Phase 3)

Method classification on ingress:
1. Parse `method` field from JSON-RPC request body
2. Look up tier (1/2/3) and CU weight
3. Compute cost; check against consumer budget
4. Select provider via QoS-weighted random selection
5. Attach TAP receipt (`data_service = RPCDataService address`)
6. Dispatch; validate Merkle proof if Tier 1 (Phase 2)

### 9.3 Receipt attachment

TAP v2 receipt `metadata` field: `abi.encode(uint32 cuWeight, bytes4 methodSelector)` — enables per-method economic accounting.

---

## 10. Integration testing strategy

Learnt from SubstreamsDataService's test architecture: **mock `HorizonStaking` only; use production `GraphTallyCollector`, `PaymentsEscrow`, and `GraphPayments`.**

Rationale: The EIP-712 signing, signer authorisation, cumulative RAV tracking, and token distribution logic live in those three contracts. Mocking them masks bugs that only surface against the real implementation — exactly the class of failure that kills an integration during testnet launch.

Test environment:
- Anvil local chain, deterministic accounts with fixed private keys
- Deploy mock: `MockHorizonStaking`, `MockController` (stubs only)
- Deploy production: `GraphTallyCollector`, `PaymentsEscrow`, `GraphPayments` from Horizon package
- Deploy `RPCDataService` against above
- Go/Rust integration tests validate EIP-712 hash compatibility across implementations

Reference: `github.com/graphprotocol/substreams-data-service/test/integration/` — especially `rav_test.go` for Go↔Solidity hash/signature cross-validation and `substreams_flow_test.go` for end-to-end RAV flow.

---

## 11. Deployed contract addresses

### Arbitrum One (chain ID 42161) — all Horizon contracts

| Contract | Address |
|---|---|
| HorizonStaking | `0x00669A4CF01450B64E8A2A20E9b1FCB71E61eF03` |
| SubgraphService | `0xb2Bb92d0DE618878E438b55D5846cfecD9301105` |
| GraphTallyCollector | `0x8f69F5C07477Ac46FBc491B1E6D91E2be0111A9e` |
| PaymentsEscrow | `0x8f477709eF277d4A880801D01A140a9CF88bA0d3` |
| DisputeManager | `0x0Ab2B043138352413Bb02e67E626a70320E3BD46` |
| RewardsManager | `0x971B9d3d0Ae3ECa029CAB5eA1fB0F72c85e6a525` |
| GRT Token | `0x9623063377AD1B27544C965cCd7342f7EA7e88C7` |

Gateway sender: `0xDDE4cfFd3D9052A9cb618fC05a1Cd02be1f2F467`
TAP aggregator: `https://tap-aggregator.network.thegraph.com`

### Arbitrum Sepolia (chain ID 421614) — testnet

| Contract | Address |
|---|---|
| HorizonStaking | `0x865365C425f3A593Ffe698D9c4E6707D14d51e08` |
| SubgraphService | `0xc24A3dAC5d06d771f657A48B20cE1a671B78f26b` |
| GraphTallyCollector | `0x382863e7B662027117449bd2c49285582bbBd21B` |
| PaymentsEscrow | `0x1e4dC4f9F95E102635D8F7ED71c5CdbFa20e2d02` |

---

## 12. Open questions

| # | Question | Recommended position |
|---|---|---|
| Q1 | Chain ID type | `uint256` (EIP-155) with governance allowlist Phase 1, permissionless + bond Phase 2 |
| Q2 | Minimum provision | 25,000 GRT per chain |
| Q3 | Thawing period | 14 days |
| Q4 | Gateway discovery | On-chain events + RPC network subgraph (mirrors SubgraphService pattern) |
| Q5 | RAV aggregation | Shared TAP/GraphTally infrastructure — data_service field differentiates |
| Q6 | stakeToFeesRatio | 5:1 (consistent with SubgraphService) |
| Q7 | Per-method pricing | ✅ CU-weighted (1–20 CU per method; configurable `base_price_per_cu`) |
| Q8 | eth_sendRawTransaction | Include at flat fee; no verification needed (self-authenticated) |

---

## 13. Source repositories

| Repo | Contents |
|---|---|
| `github.com/graphprotocol/contracts` | Horizon contracts (`packages/horizon/`), SubgraphService (`packages/subgraph-service/`) |
| `github.com/graphprotocol/substreams-data-service` | Second Horizon data service (pre-launch); reference for `paymentsDestination` pattern, integration test strategy, provider gateway architecture |
| `github.com/graphprotocol/indexer-rs` | `indexer-service-rs`, `indexer-tap-agent` — Rust workspace |
| `github.com/graphprotocol/indexer` | `indexer-agent` — TypeScript |
| `github.com/edgeandnode/gateway` | Production gateway — Rust, MIT |
| `github.com/edgeandnode/tap-graph` | TAP Solidity types |

---

## 14. Competitive landscape

| Protocol | Verification | Token | Chains | Notes |
|---|---|---|---|---|
| Pocket Network | Proof-of-Relay + quorum | POKT | 50+ | CU-based pricing; mint-burn model |
| Lava Network | On-chain QoS scoring + cross-ref | LAVA | 30+ | Spec-based chain definitions; chain-funded pools |
| Infura DIN | Watcher nodes + stake-backed SLAs | — (EigenLayer) | 30+ | 13B+ req/month; progressive decentralisation |
| Ankr | Internal QoS | ANKR | 80+ | 1T+ monthly requests; method-weighted pricing |
| dRPC.org | AI routing (no on-chain) | — | 90+ | No token; curated providers |
| **This service** | **Merkle proofs (Tier 1) + quorum** | **GRT** | **10+ chains** | **Integrated with subgraphs, Substreams, Amp** |

The Graph's competitive moat: RPC + indexed data (subgraphs) + streaming (Substreams) + SQL analytics (Amp) under one economic and security umbrella. One stake, one payment system, one network.
