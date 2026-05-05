# Dispatch Economics — Profit Calculation

There are two separate revenue streams for a Dispatch provider, and they work very differently.

---

## Stream 1: Subgraph Indexing Rewards

This only applies if you are also running as a Graph Protocol subgraph indexer. It is GRT inflation, distributed each epoch proportionally to signal and stake:

```
monthly_reward = monthly_issuance × (subgraph_signal / total_signal) × (our_allocation / subgraph_total_allocation)
```

- Network monthly issuance: ~26.1M GRT (as of Q2 2026)
- Total network signal: ~9M GRT
- Top indexers earn thousands of GRT/month from well-chosen allocations

These rewards are **not paid per-request** — they accrue continuously and are claimed when an allocation is closed. The key levers are subgraph selection (signal-to-stake ratio) and total GRT provisioned (self-stake + delegated).

This is the steady base income. Dispatch query fees are the variable upside.

---

## Stream 2: Dispatch RPC Query Fees

Fully pay-per-request via **GraphTally (TAP v2)** micropayments. No epoch, no allocation — every request generates a signed receipt, receipts aggregate into RAVs, RAVs are redeemed on-chain hourly.

### Pricing model

The gateway charges per **compute unit (CU)**, where one CU = `4_000_000_000_000` GRT wei = `4e-6 GRT`.

Method complexity sets the CU cost. Values below are confirmed from live receipt data:

| RPC Method | CUs | Per-provider receipt |
|---|---|---|
| `eth_chainId` | 1 | 4e-6 GRT |
| `eth_getBlockByHash` | 5 | 20e-6 GRT |
| `eth_getBlockByNumber` | 5 | 20e-6 GRT |
| `eth_getBlockReceipts` | 10 | 40e-6 GRT |
| `eth_call` | 10 | 40e-6 GRT |
| `eth_getLogs` | 20 | 80e-6 GRT |

**Concurrent dispatch multiplier:** The gateway is configured to dispatch to up to `concurrent_k = 3` providers simultaneously (first response wins), and all providers that respond receive a signed receipt — that is the cost of censorship-resistance and latency optimisation. With the network currently at 1 active provider, the effective multiplier is ×1. Once multiple providers are live, consumers pay the per-provider receipt value × k for each request.

### USD cost at different GRT prices

| Method | At $0.0246/GRT | At $0.09/GRT | Alchemy (reference) |
|---|---|---|---|
| `eth_blockNumber` /M calls | ~$0.35 | ~$1.08 | $4.50 |
| `eth_call` /M calls | ~$2.95 | ~$10.80 | $11.70 |
| `eth_getLogs` /M calls | ~$5.90 | ~$21.60 | $33.75 |

At $0.09/GRT Dispatch is competitive with or cheaper than Alchemy on every method. At today's depressed GRT price it is significantly cheaper — meaning providers earn less per request, but consumers get a good deal.

This math is verified by a test in the gateway codebase: `crates/dispatch-gateway/src/config.rs::tests::pricing_math`.

---

## Fee Distribution (on-chain)

When a provider calls `RPCDataService.collect()`, fees flow through the Horizon stack:

```
RAV value (gross)
  ├── ~2% protocol tax  → burned / treasury (GraphPayments)
  ├── 2% data service cut (RPCDataService)
  │     ├── 1% burned   (BURN_CUT_PPM = 10_000 ppm)
  │     └── 1% retained as data service revenue
  ├── delegator cut     (set by provider, e.g. 10% of remainder)
  └── provider          (remainder — typically ~86%)
```

Constants from `contracts/src/RPCDataService.sol`:

```solidity
uint256 public constant BURN_CUT_PPM       = 10_000;  // 1%
uint256 public constant DATA_SERVICE_CUT_PPM = 10_000;  // 1%
uint256 public constant STAKE_TO_FEES_RATIO  = 5;      // 5× stake locked per GRT collected
```

**Stake locking:** each `collect()` call locks `fees × 5` GRT in a stake claim for 14 days (dispute window). A provider collecting 100 GRT/month needs at least 500 GRT provisioned — identical to how SubgraphService works.

---

## Payment Lifecycle

```
Consumer signs TAP receipt per request (EIP-712, random nonce, CU-weighted value)
    ↓  (every 60 seconds)
dispatch-service aggregates receipts → RAV (Receipt Aggregate Voucher, monotonically increasing)
    ↓  (every hour)
provider calls RPCDataService.collect(signedRAV)
    → GraphTallyCollector validates RAV signature
    → PaymentsEscrow debits consumer's on-chain deposit
    → GraphPayments distributes: protocol tax → data service → delegators → provider
    → GRT lands in provider's paymentsDestination wallet
```

Consumers must pre-fund an escrow on-chain. The provider checks escrow balance before serving requests (configurable via `credit_threshold`; self-consumption can bypass this check via `bypass_consumers`).

---

## Provider P&L — Worked Example

Assume a provider handling **10M `eth_call` requests/month** at $0.09/GRT:

| Item | Value |
|---|---|
| Gross receipts (10M × 40e-6 GRT) | 400 GRT |
| Data service cut (2%) | –8 GRT |
| Protocol tax (~2%) | –8 GRT |
| Delegator cut (10%) | –38.4 GRT |
| **Provider net** | **~345.6 GRT ≈ $31** |
| Backend node cost (Chainstack Growth) | ~$200/month |

At 10M calls/month the Dispatch fees alone do not cover a dedicated archive node. Volume is everything — this becomes meaningful at **hundreds of millions of requests per month**, which requires either consumer acquisition or acting as the RPC backbone for other indexers.

The immediate value for early providers is not query-fee income but:
1. Production validation of the payment loop
2. First-mover positioning as the network grows
3. Dogfooding as your own consumer (self-consumption offsets some RPC costs)

---

## Break-even Analysis

**For query fees to cover a $200/month Chainstack node at $0.09/GRT:**

```
$200 ÷ ($10.80/M calls) ≈ 18.5M eth_call equivalents/month
```

Or at current $0.0246/GRT:

```
$200 ÷ ($2.95/M calls) ≈ 67.8M eth_call equivalents/month
```

That is ~2.3M calls/day — achievable at scale, but not from a single self-consumption loop.

**GRT price sensitivity:**

| GRT price | Revenue per 10M eth_calls | Node cost covered |
|---|---|---|
| $0.0246 | ~$8.50 | 4.3% |
| $0.05 | ~$17.30 | 8.7% |
| $0.09 | ~$31.10 | 15.6% |
| $0.20 | ~$69.10 | 34.6% |
| $0.50 | ~$172.80 | 86.4% |

The economics improve dramatically with GRT price — query fees are denominated in GRT, so each dollar of GRT appreciation directly increases USD revenue per request.

---

## Current State (Lodestar, May 2026)

- **Active providers:** 1 (`https://rpc.cargopete.com`)
- **Dispatch query fee revenue:** ~$0 net — operator is the only consumer (self-consumption loop)
- **Self-consumption escrow:** 9,363 GRT deposited; ~63 GRT/day consumed; ~149 days runway
- **Net cost of self-consumption:** ~2% protocol/data-service fees on recycled GRT — negligible
- **Real profit from Dispatch:** starts when external consumers appear

The subgraph indexing rewards (~5,578 GRT/month) are the primary income source today. Dispatch is live, the payment loop is proven, and the platform is ready for external consumer traffic.
