# Payments

Dispatch uses [GraphTally (TAP v2)](https://github.com/graphprotocol/graph-improvement-proposals/blob/main/gips/0054-graphtally.md) — the same payment infrastructure used by The Graph's SubgraphService. GRT moves off-chain per request via signed receipts, then settles on-chain in batches.

---

## End-to-end flow

```
1. Consumer deposits GRT into PaymentsEscrow (keyed by their own address as payer)
2. Consumer includes X-Consumer-Address header on every gateway request
3. Per request: gateway signs a TAP receipt (EIP-712 ECDSA, random nonce, CU-weighted value)
   — consumer address is encoded in receipt metadata so the correct escrow is charged
4. Receipt sent in TAP-Receipt header alongside JSON-RPC request to dispatch-service
5. dispatch-service extracts consumer address from metadata, checks their escrow balance,
   validates signature, persists receipt to PostgreSQL
6. dispatch-service TAP aggregator batches receipts per consumer → sends to /rav/aggregate
7. Gateway returns a signed RAV (Receipt Aggregate Voucher) with payer = consumer address
8. dispatch-service collector submits RAV on-chain every hour: RPCDataService.collect()
                               → GraphTallyCollector (verifies EIP-712, tracks cumulative value)
                               → PaymentsEscrow (draws GRT from consumer's escrow)
                               → GraphPayments (distributes: protocol tax → delegators → provider)
                               → 2% data-service cut retained by RPCDataService (1% burned, 1% as revenue)
                               → remainder lands at paymentsDestination
```

`valueAggregate` in a RAV is **cumulative and never resets**. Each `collect()` call computes the delta from the last collected value. This means a lost RAV doesn't lose funds — the next RAV covers the gap.

---

## EIP-712 domain

All receipts and RAVs are signed against this domain on Arbitrum One:

```
name: protocol-configured
version: "1"
chainId: 42161
verifyingContract: 0x8f69F5C07477Ac46FBc491B1E6D91E2bb0111A9e  // GraphTallyCollector
```

The `data_service` field in every receipt is set to `RPCDataService`'s address, preventing cross-service receipt replay.

---

## CU-weighted pricing

Request value is proportional to compute units (CUs) — a weight assigned per method.

| Method | CU |
|---|---|
| `eth_chainId`, `net_version`, `eth_blockNumber` | 1 |
| `eth_getBalance`, `eth_getTransactionCount`, `eth_getCode`, `eth_getStorageAt` | 5 |
| `eth_sendRawTransaction`, `eth_getBlockByHash/Number` | 5 |
| `eth_call`, `eth_estimateGas`, `eth_getTransactionReceipt`, `eth_getTransactionByHash` | 10 |
| `eth_getLogs` (bounded) | 20 |
| `debug_traceTransaction` | 500+ |

Receipt value = `CU x base_price_per_cu`. Default `base_price_per_cu` is `4_000_000_000_000` GRT wei (~\$40/million requests at \$0.09 GRT).

---

## TAP receipt overhead

Receipt processing must not slow down requests. In practice:

| Operation | Latency |
|---|---|
| ECDSA signature verification | ~0.1ms |
| Receipt storage (async, not on critical path) | ~0ms |
| **Total overhead** | **&lt;1ms** |

---

## Data service fee cut

On each `collect()`, `RPCDataService` takes a **2% cut** from the fees routed through it:

| Portion | PPM | Behaviour |
|---|---|---|
| `BURN_CUT_PPM` | 10,000 (1%) | Burned via `GRT.burn()` — deflationary |
| `DATA_SERVICE_CUT_PPM` | 10,000 (1%) | Accumulated in the contract; owner withdraws via `withdrawFees()` |

The remaining 98% flows to the provider's `paymentsDestination`.

---

## Stake locking

On each `collect()`, `RPCDataService` locks `fees x stakeToFeesRatio` in a stake claim via `DataServiceFees._createStakeClaim()`. The claim releases after `thawingPeriod`. This ensures providers maintain sufficient economic stake relative to fees collected.

Default `stakeToFeesRatio` is 5 — consistent with SubgraphService.

---

## Payments destination

The GRT recipient on `collect()` is `paymentsDestination[serviceProvider]`, not necessarily the provider's staking address. Providers can separate their operator key from their payment wallet via `setPaymentsDestination(address)`. Defaults to the provider address on registration.
