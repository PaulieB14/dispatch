# drpc-service

> **Community project — not affiliated with or endorsed by The Graph Foundation or Edge & Node.**
> This is an independent hobby implementation exploring what a JSON-RPC data service on Horizon might look like.

A decentralised JSON-RPC data service built on [The Graph Protocol's Horizon framework](https://thegraph.com/docs/en/horizon/). Indexers stake GRT, register to serve specific chains, and get paid per request via [GraphTally](https://github.com/graphprotocol/graph-improvement-proposals/blob/main/gips/0054-graphtally.md) (TAP v2) micropayments.

Inspired by the [Q3 2026 "Experimental JSON-RPC Data Service"](https://thegraph.com/blog/graph-protocol-2026-technical-roadmap/) direction in The Graph's 2026 Technical Roadmap — but this codebase is an independent community effort, not an official implementation.

**Implementation status:** all five phases are feature-complete and deployed. `RPCDataService` is live on Arbitrum One at `0x73846272813065c3e4efdb3fb82e0d128c8c2364`. The subgraph, npm packages, and end-to-end test suite are all live. Run the full payment loop locally with `cd demo && npm install && npm start`.

---

## Network status

The network is live with its first active provider.

| Component | Status |
|---|---|
| `RPCDataService` contract | Live on Arbitrum One |
| Subgraph | Live on The Graph Studio |
| npm packages | Published (`@graph-drpc/consumer-sdk`, `@graph-drpc/indexer-agent`) |
| Active providers | **1** — `https://rpc.cargopete.com` (Arbitrum One, Standard + Archive) |
| `drpc-oracle` | Not running — `slash()` fraud proofs require it |
| Local demo | Working — full payment loop on Anvil with mock contracts |

The first provider is live at `https://rpc.cargopete.com`, serving Arbitrum One (chain ID 42161) with Standard and Archive tiers. Validated end-to-end with `drpc-smoke`: consumer signs TAP receipts, gateway routes to the provider, provider forwards to Chainstack, real RPC responses returned.

```
drpc-smoke
  endpoint   : http://rpc.cargopete.com
  chain_id   : 42161

  [PASS] GET /health → 200 OK
  [PASS] eth_blockNumber — returns current block → "0x1b01312d" [196ms]
  [PASS] eth_chainId — returns 0x61a9 (42161) → "0xa4b1" [73ms]
  [PASS] eth_getBalance — returns balance at latest block (Standard) → "0x6f3a59e597c5342" [94ms]
  [PASS] eth_getBalance — historical block (Archive) → "0x0" [649ms]
  [PASS] eth_getLogs — recent block range (Tier 2 quorum) → [...] [83ms]

  5 passed, 0 failed
```

To become the next provider: stake ≥ 25,000 GRT on Arbitrum One, run `drpc-service` pointing at an Ethereum node, and register via the indexer agent or directly via the contract.

---

## Architecture

```
Consumer (dApp)
   │
   ├── via consumer-sdk (trustless, direct)
   │     signs receipts locally, discovers providers via subgraph
   │
   └── via drpc-gateway (managed, centralised)
         QoS-scored selection, TAP receipt signing, quorum consensus
   │
   │  POST /rpc/{chain_id}  (or X-Chain-Id header on /rpc)
   │  TAP-Receipt: { signed EIP-712 receipt }
   ▼
drpc-service          ← JSON-RPC proxy, TAP receipt validation, response attestation,
   │                    receipt persistence (PostgreSQL → TAP agent → RAV redemption)
   ▼
Ethereum client       ← Geth / Erigon / Reth / Nethermind
(full or archive)

drpc-oracle           ← Block header oracle: polls L1 every ~12s, submits
                        state roots to Arbitrum for on-chain fraud proof verification
```

Payment flow (off-chain → on-chain):

```
receipts (per request) → TAP agent aggregates → RAV → RPCDataService.collect()
                                                         → GraphTallyCollector
                                                         → PaymentsEscrow
                                                         → GraphPayments
                                                         → GRT to indexer
```

---

## Workspace

```
crates/
├── drpc-tap/          Shared TAP v2 primitives: EIP-712 types, receipt signing
├── drpc-service/      Indexer-side JSON-RPC proxy with TAP middleware
├── drpc-gateway/      Gateway: provider selection, QoS scoring, receipt issuance
├── drpc-oracle/       Block header oracle: L1 state roots → Arbitrum for slash()
└── drpc-smoke/        End-to-end smoke test: signs real TAP receipts, hits a live provider

contracts/
├── src/
│   ├── RPCDataService.sol        IDataService implementation (Horizon)
│   ├── interfaces/IRPCDataService.sol
│   └── lib/StateProofVerifier.sol   EIP-1186 MPT proof verification
├── test/
└── script/
    ├── Deploy.s.sol              Mainnet deployment
    └── SetupE2E.s.sol            Local Anvil stack for tests and demo

consumer-sdk/         TypeScript SDK — dApp developers use this to talk to
                      providers directly without the gateway
indexer-agent/        TypeScript agent — automates provider register/startService/
                      stopService lifecycle with graceful shutdown
subgraph/             The Graph subgraph — indexes RPCDataService events
docker/               Docker Compose full-stack deployment
demo/                 Self-contained local demo: Anvil + contracts + Rust binaries
                      + consumer requests + collect() — full payment loop in one command
```

---

## Crates

### `drpc-tap`
Shared TAP v2 (GraphTally) primitives used by both service and gateway.
- `Receipt` / `SignedReceipt` types with serde
- EIP-712 domain separator and receipt hash computation
- `create_receipt()` — signs a receipt with a k256 ECDSA key

### `drpc-service`
Runs on the indexer alongside an Ethereum full/archive node.

Key responsibilities:
- Validate incoming TAP receipts (EIP-712 signature recovery, sender authorisation, staleness check)
- Forward JSON-RPC requests to the backend Ethereum client
- Sign responses with an attestation hash (`keccak256(chainId || method || params || response || blockHash)`)
- Persist receipts to PostgreSQL for the TAP agent
- WebSocket proxy for `eth_subscribe` / `eth_unsubscribe`

Routes: `POST /rpc/{chain_id}` · `GET /ws/{chain_id}` · `GET /health` · `GET /version` · `GET /chains`

### `drpc-gateway`
Sits between consumers and indexers. Manages provider discovery, quality scoring, and payment issuance.

Key responsibilities:
- Maintain a QoS score per provider (latency EMA, availability, block freshness)
- Probe all providers with synthetic `eth_blockNumber` every 10 seconds
- **Geographic routing** — region-aware score bonus, prefers nearby providers before latency data exists
- **Capability tier filtering** — Standard / Archive / Debug; `debug_*` / `trace_*` only routed to capable providers
- Select top-k providers via weighted random sampling, dispatch concurrently, return first valid response
- **Quorum consensus** for `eth_call` and `eth_getLogs` — majority vote; minority providers penalised
- **JSON-RPC batch** — concurrent per-item dispatch, per-item error isolation
- **WebSocket proxy** — bidirectional forwarding for real-time subscriptions
- Create and sign a fresh TAP receipt per request (EIP-712, random nonce, CU-weighted value)
- **Dynamic discovery** — polls the RPC network subgraph; rebuilds registry on each poll
- **Per-IP rate limiting** — token-bucket via `governor` (configurable RPS + burst)
- **Prometheus metrics** — `drpc_requests_total`, `drpc_request_duration_seconds`

Routes: `POST /rpc/{chain_id}` · `GET /ws/{chain_id}` · `GET /health` · `GET /version` · `GET /providers/{chain_id}` · `GET /metrics`

### `drpc-oracle`
Lightweight daemon that feeds Ethereum L1 block headers to the RPCDataService contract on Arbitrum, enabling the on-chain `slash()` function to verify EIP-1186 Merkle proofs.

- Polls L1 `eth_getBlockByNumber("latest")` every ~12 seconds
- Skips duplicate blocks (in-memory deduplication)
- Submits `setTrustedStateRoot(blockHash, stateRoot)` to Arbitrum with configurable tx timeout

### `consumer-sdk`
TypeScript package for dApp developers who want to send requests through the dRPC network without running a gateway.

Key features:
- `DRPCClient` — discovers providers via subgraph, signs TAP receipts per request, updates QoS scores with EMA
- `signReceipt` / `buildReceipt` — EIP-712 TAP v2 receipt construction and signing
- `discoverProviders` — subgraph GraphQL query returning active providers for a given chain and tier
- `selectProvider` — weighted random selection proportional to QoS score
- `computeAttestationHash` / `recoverAttestationSigner` — verify provider response attestations

Install: `npm install @graph-drpc/consumer-sdk`

### `indexer-agent`
TypeScript daemon automating the provider lifecycle on-chain.

- Polls on-chain registrations and reconciles against config every N seconds
- Calls `register`, `startService`, and `stopService` as needed
- Graceful shutdown: stops all active registrations before exiting on SIGTERM/SIGINT

Install: `npm install @graph-drpc/indexer-agent`

### `contracts/RPCDataService.sol`
On-chain contract inheriting Horizon's `DataService` + `DataServiceFees` + `DataServicePausable`.

Key functions:
- `register` — validates provision (≥ 25,000 GRT, ≥ 14-day thawing), stores provider metadata and `paymentsDestination`
- `setPaymentsDestination` — decouple the GRT payment recipient from the operator signing key
- `startService` — activates provider for a `(chainId, capabilityTier)` pair
- `stopService` / `deregister` — lifecycle management
- `collect` — enforces `QueryFee` payment type; routes through `GraphTallyCollector`, locks `fees × 5` in stake claims; accrues issuance rewards if pool is funded
- `slash` — Tier 1 Merkle fraud proof slashing via EIP-1186 MPT proofs (`StateProofVerifier.sol`)
- `claimRewards` — transfer accrued GRT issuance rewards to the caller
- `proposeChain` / `approveProposedChain` / `rejectProposedChain` — permissionless chain registration with 100k GRT bond
- `setMinThawingPeriod` — governance-adjustable thawing period (≥ 14 days)

Reference implementations: [`SubgraphService`](https://github.com/graphprotocol/contracts/tree/main/packages/subgraph-service) (live on Arbitrum One) and [`substreams-data-service`](https://github.com/graphprotocol/substreams-data-service) (pre-launch).

---

## Verification tiers

| Tier | Methods | Verification | Slashing |
|---|---|---|---|
| 1 — Merkle-provable | `eth_getBalance`, `eth_getStorageAt`, `eth_getCode`, `eth_getProof`, `eth_getBlockByHash` | EIP-1186 Merkle-Patricia proof against trusted block header (`drpc-oracle` feeds state roots) | ✅ Implemented |
| 2 — Quorum | `eth_call`, `eth_getLogs`, `eth_getTransactionReceipt`, `eth_blockNumber`, … | Multi-provider cross-reference; minority penalised in QoS | No |
| 3 — Non-deterministic | `eth_estimateGas`, `eth_gasPrice`, `eth_maxPriorityFeePerGas` | Reputation scoring only | No |

---

## Supported chains

| Chain | ID |
|---|---|
| Ethereum | 1 |
| Arbitrum One | 42161 |
| Optimism | 10 |
| Base | 8453 |
| Polygon | 137 |
| BNB Chain | 56 |
| Avalanche C-Chain | 43114 |
| zkSync Era | 324 |
| Linea | 59144 |
| Scroll | 534352 |

---

## Deployed addresses

All Horizon contracts live on **Arbitrum One** (chain ID 42161).

| Contract | Address |
|---|---|
| HorizonStaking | `0x00669A4CF01450B64E8A2A20E9b1FCB71E61eF03` |
| GraphPayments | `0xb98a3D452E43e40C70F3c0B03C5c7B56A8B3b8CA` |
| PaymentsEscrow | `0x8f477709eF277d4A880801D01A140a9CF88bA0d3` |
| GraphTallyCollector | `0x8f69F5C07477Ac46FBc491B1E6D91E2bb0111A9e` |
| RPCDataService | `0x73846272813065c3e4efdb3fb82e0d128c8c2364` |

Subgraph: `https://api.studio.thegraph.com/query/1747796/rpc-network/v0.1.1`

---

## Getting started

### Smoke test a live provider

Fires real TAP-signed JSON-RPC requests at a running provider and validates responses.

```bash
# Test the public provider (default)
cargo run --bin drpc-smoke

# Test your own provider
DRPC_ENDPOINT=http://localhost:8080 cargo run --bin drpc-smoke

# Full validated test with a registered provider key
DRPC_ENDPOINT=https://rpc.my-indexer.com \
DRPC_SIGNER_KEY=0x... \
DRPC_PROVIDER_ADDRESS=0x... \
cargo run --bin drpc-smoke
```

### Run the demo (quickest path)

Runs a complete local stack — Anvil, Horizon mock contracts, drpc-service, drpc-gateway — makes 5 RPC requests, submits a RAV, and proves GRT lands in the payment wallet.

Requires: [Foundry](https://getfoundry.sh) and Rust stable.

```bash
cd demo
npm install
npm start
```

### Docker Compose

```bash
cp docker/gateway.example.toml docker/gateway.toml
cp docker/config.example.toml  docker/config.toml
cp docker/oracle.example.toml  docker/oracle.toml
# Fill in private keys, provider addresses, backend URLs, and L1 RPC URL.
docker compose -f docker/docker-compose.yml up
```

### Build from source

```bash
cargo build
cargo test
```

### Run the indexer service

```bash
cp config.example.toml config.toml
# fill in: indexer address, operator private key, TAP config, backend node URLs
RUST_LOG=info cargo run --bin drpc-service
```

### Run the gateway

```bash
cp docker/gateway.example.toml gateway.toml
# fill in: signer key, data_service_address, provider list
RUST_LOG=info cargo run --bin drpc-gateway
```

### Run the oracle

```bash
cp docker/oracle.example.toml oracle.toml
# fill in: L1 RPC URL, Arbitrum RPC URL, owner private key, data_service_address
RUST_LOG=info cargo run --bin drpc-oracle
```

### Deploy the contract

```bash
cd contracts
forge build
forge test -vvv

cp .env.example .env
# fill in PRIVATE_KEY, OWNER, PAUSE_GUARDIAN, GRT_TOKEN, GRAPH_CONTROLLER, GRAPH_TALLY_COLLECTOR
forge script script/Deploy.s.sol --rpc-url arbitrum_one --broadcast --verify -vvvv
```

### Use the Consumer SDK

```bash
npm install @graph-drpc/consumer-sdk
```

```typescript
import { DRPCClient } from "@graph-drpc/consumer-sdk";

const client = new DRPCClient({
  chainId: 1,
  dataServiceAddress: "0x73846272813065c3e4efdb3fb82e0d128c8c2364",
  graphTallyCollector: "0x8f69F5C07477Ac46FBc491B1E6D91E2bb0111A9e",
  subgraphUrl: "https://api.studio.thegraph.com/query/1747796/rpc-network/v0.1.1",
  signerPrivateKey: process.env.CONSUMER_KEY as `0x${string}`,
  basePricePerCU: 4_000_000_000_000n,
});

const block = await client.request("eth_blockNumber", []);
```

### Run the indexer agent

```bash
npm install @graph-drpc/indexer-agent
```

```typescript
import { IndexerAgent } from "@graph-drpc/indexer-agent";

const agent = new IndexerAgent({
  arbitrumRpcUrl: "https://arb1.arbitrum.io/rpc",
  rpcDataServiceAddress: "0x73846272813065c3e4efdb3fb82e0d128c8c2364",
  operatorPrivateKey: process.env.OPERATOR_KEY as `0x${string}`,
  providerAddress: "0x...",
  endpoint: "https://rpc.my-indexer.com",
  geoHash: "u1hx",
  paymentsDestination: "0x...",
  services: [
    { chainId: 1,     tier: 0 },
    { chainId: 42161, tier: 0 },
  ],
});

await agent.reconcile(); // call on a cron/interval
```

---

## Configuration

### `config.toml` (drpc-service)

```toml
[server]
host = "0.0.0.0"
port = 7700

[indexer]
service_provider_address = "0x..."
operator_private_key      = "0x..."   # signs response attestations only

[tap]
data_service_address      = "0x73846272813065c3e4efdb3fb82e0d128c8c2364"
authorized_senders        = ["0x..."]  # gateway signer address(es)
eip712_domain_name        = "TAP"
eip712_chain_id           = 42161
eip712_verifying_contract = "0x8f69F5C07477Ac46FBc491B1E6D91E2bb0111A9e"

[database]
url = "postgres://user:pass@localhost/drpc"

[chains]
supported = [1, 42161, 10, 8453]

[chains.backends]
"1"     = "http://localhost:8545"
"42161" = "http://localhost:8546"
"10"    = "http://localhost:8547"
"8453"  = "http://localhost:8548"
```

### `gateway.toml` (drpc-gateway)

```toml
[gateway]
host   = "0.0.0.0"
port   = 8080
region = "eu-west"   # optional — used for geographic routing

[tap]
signer_private_key    = "0x..."
data_service_address  = "0x73846272813065c3e4efdb3fb82e0d128c8c2364"
base_price_per_cu     = 4000000000000   # ≈ $40/M requests at $0.09 GRT
eip712_domain_name    = "TAP"
eip712_chain_id       = 42161
eip712_verifying_contract = "0x8f69F5C07477Ac46FBc491B1E6D91E2bb0111A9e"

[qos]
probe_interval_secs = 10
concurrent_k        = 3       # dispatch to top-3, first response wins
region_bonus        = 0.15    # score boost for same-region providers

[discovery]
subgraph_url  = "https://api.studio.thegraph.com/query/1747796/rpc-network/v0.1.1"
interval_secs = 60

[[providers]]
address      = "0x..."
endpoint     = "https://rpc.my-indexer.com"
chains       = [1, 42161, 10, 8453]
region       = "eu-west"
capabilities = ["standard"]   # or ["standard", "archive", "debug"]
```

### `oracle.toml` (drpc-oracle)

```toml
[oracle]
poll_interval_secs = 12    # one Ethereum block
tx_timeout_secs    = 120

[l1]
rpc_url = "https://eth-mainnet.example.com/YOUR_KEY"

[arbitrum]
rpc_url              = "https://arb1.arbitrum.io/rpc"
signer_private_key   = "0x..."   # must be RPCDataService owner
data_service_address = "0x73846272813065c3e4efdb3fb82e0d128c8c2364"
```

---

## Roadmap

| Phase | Status | Scope |
|---|---|---|
| 1 — MVP | ✅ Complete | Core contract, indexer service, gateway, TAP payments, attestation, subgraph, CI |
| 2 — Foundation | ✅ Complete | Quorum consensus, CU-weighted pricing, 10+ chains, geographic routing, capability tiers, metrics, rate limiting, WebSocket, batch RPC, dynamic discovery |
| 3 — Full parity | ✅ Complete | Tier 1 fraud proof slashing, block header oracle, WebSocket subscriptions, archive/debug tier routing |
| 4 — Production readiness | ✅ Complete | Unified endpoint, permissionless chain registration, GRT issuance groundwork, indexer agent, subgraph v2 |
| 5 — Consumer SDK & rewards | ✅ Complete | Consumer SDK, rewards pool, dynamic thawing period |
| Deployment | ✅ Complete | Contract deployed on Arbitrum One, subgraph live, npm packages published, e2e tests passing, security review done |

See [`ROADMAP.md`](ROADMAP.md) for full detail.

---

## Relation to existing Graph Protocol infrastructure

| Component | Status |
|---|---|
| HorizonStaking / GraphPayments / PaymentsEscrow | ✅ Reused as-is |
| GraphTallyCollector (TAP v2) | ✅ Reused as-is |
| `indexer-tap-agent` | ✅ Reused as-is (reads from `tap_receipts` table) |
| `indexer-service-rs` TAP middleware | ✅ Logic ported to `drpc-service` |
| `indexer-agent` | ✅ `@graph-drpc/indexer-agent` npm package handles register/startService/stopService lifecycle |
| `edgeandnode/gateway` | ✅ `drpc-gateway` implements equivalent logic for RPC; `@graph-drpc/consumer-sdk` provides trustless alternative |
| Graph Node | ❌ Not needed — standard Ethereum clients only |
| POI / SubgraphService dispute system | ❌ Replaced by tiered verification framework |

---

## License

Apache-2.0
