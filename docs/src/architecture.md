# Architecture

Dispatch has two deployment paths: a managed gateway (centralised routing, good for apps) and a direct consumer SDK (trustless, peer-to-peer).

```
Consumer (dApp)
   │
   ├── via consumer-sdk (trustless, direct)
   │     signs receipts locally, discovers providers via subgraph
   │
   └── via dispatch-gateway (managed)
         QoS-scored selection, TAP receipt signing
   │
   │  POST /rpc/{chain_id}
   │  TAP-Receipt: { signed EIP-712 receipt }
   ▼
dispatch-service          ← JSON-RPC proxy (single + batch), TAP receipt validation,
   │                         receipt persistence
   ▼
Ethereum client           ← Geth / Erigon / Reth / Nethermind
(full or archive)
```

---

## Payment flow

Receipts accumulate off-chain and settle on-chain in batches via GraphTally (TAP v2):

```
receipts (per request)
  → dispatch-service aggregates into RAV (every 60s)
  → RPCDataService.collect() (every hour)
  → GraphTallyCollector
  → PaymentsEscrow
  → GraphPayments
  → GRT to indexer (via paymentsDestination)
```

`valueAggregate` in a RAV is cumulative and never resets. The collector tracks previously collected amounts to calculate deltas on each `collect()` call.

---

## Workspace layout

```
crates/
├── dispatch-tap/      EIP-712 types, receipt signing (shared by service + gateway)
├── dispatch-service/  Indexer-side JSON-RPC proxy with TAP middleware
├── dispatch-gateway/  Gateway: QoS scoring, provider selection, receipt issuance
└── dispatch-smoke/    End-to-end smoke test against a live provider

contracts/
├── src/RPCDataService.sol   IDataService implementation (Horizon)
└── src/interfaces/

consumer-sdk/   TypeScript SDK — direct provider access without a gateway
indexer-agent/  TypeScript agent — automates provider lifecycle on-chain
subgraph/       The Graph subgraph — indexes RPCDataService events
docker/         Docker Compose full-stack deployment
demo/           Self-contained local demo: full payment loop on Anvil
```

---

## Horizon integration

Dispatch is a data service in the Horizon framework. Three Horizon layers are in play:

**HorizonStaking** — indexers call `provision(serviceProvider, RPCDataService, tokens, maxVerifierCut, thawingPeriod)`. Minimum 555 GRT, 14-day thawing period.

**GraphPayments + PaymentsEscrow** — consumers deposit GRT into escrow keyed by `(sender, serviceProvider)`. Every request carries a TAP receipt; the TAP agent batches these into RAVs redeemed via `collect()`.

**DataService framework** — `RPCDataService` inherits `DataService` + `DataServiceFees` + `DataServicePausableUpgradeable`. Deployed as a UUPS upgradeable proxy (ERC1967). The framework handles stake claim linked lists, fee locking at the configured ratio, and pause logic.
