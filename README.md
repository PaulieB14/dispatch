# drpc-service

> **Community project — not affiliated with or endorsed by The Graph Foundation or Edge & Node.**
> This is an independent hobby implementation exploring what a JSON-RPC data service on Horizon might look like.

A decentralised JSON-RPC data service built on [The Graph Protocol's Horizon framework](https://thegraph.com/docs/en/horizon/). Indexers stake GRT, register to serve specific chains, and get paid per request via [GraphTally](https://github.com/graphprotocol/graph-improvement-proposals/blob/main/gips/0054-graphtally.md) (TAP v2) micropayments.

Inspired by the [Q3 2026 "Experimental JSON-RPC Data Service"](https://thegraph.com/blog/graph-protocol-2026-technical-roadmap/) direction in The Graph's 2026 Technical Roadmap — but this codebase is an independent community effort, not an official implementation.

---

## Architecture

```
Consumer
   │
   ▼
drpc-gateway          ← QoS-scored provider selection, TAP receipt signing,
   │                    concurrent/quorum dispatch, rate limiting, Prometheus metrics
   │  POST /rpc/{chain_id}
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
└── drpc-oracle/       Block header oracle: L1 state roots → Arbitrum for slash()

contracts/
├── src/
│   ├── RPCDataService.sol        IDataService implementation (Horizon)
│   ├── interfaces/IRPCDataService.sol
│   └── lib/StateProofVerifier.sol   EIP-1186 MPT proof verification
├── test/
└── script/Deploy.s.sol

subgraph/             The Graph subgraph — indexes RPCDataService events
docker/               Docker Compose full-stack deployment
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
- **RAV aggregation endpoint** — `POST /rav/aggregate` triggers TAP agent RAV collection

Routes: `POST /rpc/{chain_id}` · `GET /ws/{chain_id}` · `GET /health` · `GET /version` · `GET /providers/{chain_id}` · `GET /metrics` · `POST /rav/aggregate`

### `drpc-oracle`
Lightweight daemon that feeds Ethereum L1 block headers to the RPCDataService contract on Arbitrum, enabling the on-chain `slash()` function to verify EIP-1186 Merkle proofs.

- Polls L1 `eth_getBlockByNumber("latest")` every ~12 seconds
- Skips duplicate blocks (in-memory deduplication)
- Submits `setTrustedStateRoot(blockHash, stateRoot)` to Arbitrum with configurable tx timeout

### `contracts/RPCDataService.sol`
On-chain contract inheriting Horizon's `DataService` + `DataServiceFees` + `DataServicePausable`.

Key functions:
- `register` — validates provision (≥ 25,000 GRT, ≥ 14-day thawing), stores provider metadata and `paymentsDestination`
- `setPaymentsDestination` — decouple the GRT payment recipient from the operator signing key
- `startService` — activates provider for a `(chainId, capabilityTier)` pair
- `stopService` / `deregister` — lifecycle management
- `collect` — enforces `QueryFee` payment type; routes through `GraphTallyCollector`, locks `fees × 5` in stake claims
- `slash` — Tier 1 Merkle fraud proof slashing via EIP-1186 MPT proofs (`StateProofVerifier.sol`)

Reference implementations: [`SubgraphService`](https://github.com/graphprotocol/contracts/tree/main/packages/subgraph-service) (live on Arbitrum One) and [`substreams-data-service`](https://github.com/graphprotocol/substreams-data-service) (pre-launch).

---

## Verification tiers

| Tier | Methods | Verification | Slashing |
|---|---|---|---|
| 1 — Merkle-provable | `eth_getBalance`, `eth_getStorageAt`, `eth_getCode`, `eth_getProof`, `eth_getBlockByHash` | EIP-1186 Merkle-Patricia proof against trusted block header (`drpc-oracle` feeds state roots) | ✅ Implemented |
| 2 — Quorum | `eth_call`, `eth_getLogs`, `eth_getTransactionReceipt`, `eth_blockNumber`, … | Multi-provider cross-reference; minority penalised in QoS | No |
| 3 — Non-deterministic | `eth_estimateGas`, `eth_gasPrice`, `eth_maxPriorityFeePerGas` | Reputation scoring only | No |

---

## Supported chains (Phase 1 + 2)

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

## Deployed contract addresses

All Horizon contracts live on **Arbitrum One** (chain ID 42161).

| Contract | Address |
|---|---|
| HorizonStaking | `0x00669A4CF01450B64E8A2A20E9b1FCB71E61eF03` |
| GraphTallyCollector | `0x8f69F5C07477Ac46FBc491B1E6D91E2be0111A9e` |
| PaymentsEscrow | `0x8f477709eF277d4A880801D01A140a9CF88bA0d3` |
| SubgraphService (reference) | `0xb2Bb92d0DE618878E438b55D5846cfecD9301105` |
| RPCDataService | TBD (deploy via `contracts/script/Deploy.s.sol`) |

Testnet (Arbitrum Sepolia, chain ID 421614): see [`contracts/.env.example`](contracts/.env.example).

---

## Getting started

### Prerequisites
- Rust stable (see `rust-toolchain.toml`)
- PostgreSQL 14+
- An Ethereum full node (Geth, Erigon, Reth, or Nethermind)
- [Foundry](https://getfoundry.sh) for contract work

### Docker Compose (quickest path)

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
cp crates/drpc-gateway/gateway.example.toml gateway.toml
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
# fill in PRIVATE_KEY, GRAPH_CONTROLLER, PAUSE_GUARDIAN
forge script script/Deploy.s.sol --rpc-url arbitrum_sepolia --broadcast --verify -vvvv
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
data_service_address     = "0x..."    # RPCDataService (after deployment)
authorized_senders       = ["0xDDE4cfFd3D9052A9cb618fC05a1Cd02be1f2F467"]
eip712_domain_name       = "TAP"
eip712_chain_id          = 42161
eip712_verifying_contract = "0x8f69F5C07477Ac46FBc491B1E6D91E2be0111A9e"

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
data_service_address  = "0x..."
base_price_per_cu     = 4_000_000_000_000   # ≈ $40/M requests at $0.09 GRT
eip712_domain_name    = "TAP"

[qos]
probe_interval_secs = 10
concurrent_k        = 3       # dispatch to top-3, first response wins
region_bonus        = 0.15    # score boost for same-region providers

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
data_service_address = "0x..."
```

---

## Roadmap

| Phase | Status | Scope |
|---|---|---|
| 1 — MVP | ✅ Complete | Core contract, indexer service, gateway, TAP payments, attestation, subgraph, CI |
| 2 — Foundation | ✅ Complete | Quorum consensus, CU-weighted pricing, 10+ chains, geographic routing, capability tiers, metrics, rate limiting, WebSocket, batch RPC, dynamic discovery |
| 3 — Full parity | ✅ Largely complete | Tier 1 fraud proof slashing, block header oracle, WebSocket subscriptions |
| 4 — Production | 🔲 Planned | TEE verification, P2P SDK, GRT issuance rewards, permissionless chains |

See [`ROADMAP.md`](ROADMAP.md) for full detail.

---

## Relation to existing Graph Protocol infrastructure

| Component | Status |
|---|---|
| HorizonStaking / GraphPayments / PaymentsEscrow | ✅ Reused as-is |
| GraphTallyCollector (TAP v2) | ✅ Reused as-is |
| `indexer-tap-agent` | ✅ Reused as-is (reads from `tap_receipts` table) |
| `indexer-service-rs` TAP middleware | ✅ Logic ported to `drpc-service` |
| `indexer-agent` | 🔄 Needs adaptation (chain registration instead of allocation management) |
| `edgeandnode/gateway` | 🔄 `drpc-gateway` implements equivalent logic for RPC |
| Graph Node | ❌ Not needed — standard Ethereum clients only |
| POI / SubgraphService dispute system | ❌ Replaced by tiered verification framework |

---

## License

Apache-2.0
