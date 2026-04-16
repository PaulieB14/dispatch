# Payments

Dispatch uses [GraphTally (TAP v2)](https://github.com/graphprotocol/graph-improvement-proposals/blob/main/gips/0054-graphtally.md) — the same payment infrastructure used by The Graph's SubgraphService. GRT moves off-chain per request via signed receipts, then settles on-chain in batches.

---

## End-to-end flow

```
1. Consumer deposits GRT into PaymentsEscrow for the gateway signer (or provider)
2. Per request: gateway signs a TAP receipt (EIP-712 ECDSA, random nonce, value in GRT wei)
3. Receipt sent in TAP-Receipt header alongside JSON-RPC request
4. dispatch-service validates signature, persists receipt to PostgreSQL
5. dispatch-service TAP aggregator batches receipts → sends to gateway's /rav/aggregate endpoint
6. Gateway returns a signed RAV (Receipt Aggregate Voucher); service upserts it to tap_ravs
7. dispatch-service collector submits RAV on-chain every hour: RPCDataService.collect()
                               → GraphTallyCollector (verifies EIP-712, tracks cumulative value)
                               → PaymentsEscrow (draws GRT from escrow)
                               → GraphPayments (distributes: protocol tax → delegators → provider)
                               → GRT lands at paymentsDestination
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

Receipt value = `CU × base_price_per_cu`. Default `base_price_per_cu` is `4_000_000_000_000` GRT wei (~$40/million requests at $0.09 GRT).

---

## TAP receipt overhead

Receipt processing must not slow down requests. In practice:

| Operation | Latency |
|---|---|
| ECDSA signature verification | ~0.1ms |
| Receipt storage (async, not on critical path) | ~0ms |
| **Total overhead** | **<1ms** |

---

## Stake locking

On each `collect()`, `RPCDataService` locks `fees × stakeToFeesRatio` in a stake claim via `DataServiceFees._createStakeClaim()`. The claim releases after `thawingPeriod`. This ensures providers maintain sufficient economic stake relative to fees collected.

Default `stakeToFeesRatio` is 5 — consistent with SubgraphService.

---

## Payments destination

The GRT recipient on `collect()` is `paymentsDestination[serviceProvider]`, not necessarily the provider's staking address. Providers can separate their operator key from their payment wallet via `setPaymentsDestination(address)`. Defaults to the provider address on registration.
