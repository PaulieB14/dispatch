# Contract Reference

`RPCDataService` is deployed on Arbitrum One at `0x7101d5c1a5c89c3647f5118da118e56c023ba0b9` as a **UUPS upgradeable proxy** (ERC1967). The proxy forwards all calls to an implementation contract; upgrades are owner-authorised via `upgradeToAndCall()`.

It inherits Horizon's `DataService` + `DataServiceFees` + `DataServicePausableUpgradeable` and implements `IDataService`.

---

## Functions

### `register(serviceProvider, data)`

Registers a new provider.

- `data`: `abi.encode(string endpoint, string geoHash, address paymentsDestination)`
- Validates: provision ≥ `minimumProvisionTokens`, thawing period ≥ `minimumThawingPeriod`
- Sets `paymentsDestination[serviceProvider]` (defaults to `serviceProvider` if zero address)
- Emits `ProviderRegistered`

### `setPaymentsDestination(destination)`

Changes the GRT recipient for collected fees. The new address takes effect on the next `collect()` call. Callable by a registered provider or their authorised operator at any time.

### `startService(serviceProvider, data)`

Activates a provider for a specific chain and tier.

- `data`: `abi.encode(uint64 chainId, uint8 tier, string endpoint)`
- Validates: `chainId` in `supportedChains`, provider registered, provision meets per-chain minimum
- Emits `ServiceStarted`

### `stopService(serviceProvider, data)`

Deactivates a provider for a `(chainId, tier)` pair.

### `deregister(serviceProvider, data)`

Removes the provider from the registry. Must stop all active services first.

### `collect(serviceProvider, paymentType, data)`

Redeems a signed RAV for GRT.

- `data`: `abi.encode(SignedRAV, tokensToCollect)`
- Reverts with `InvalidPaymentType` if `paymentType != QueryFee`
- Calls `GraphTallyCollector.collect()` — verifies EIP-712 signature, tracks cumulative value
- Applies a **2% data-service cut** on collected fees: 1% is burned (`BURN_CUT_PPM`), 1% is retained as contract revenue (`DATA_SERVICE_CUT_PPM`). The remainder routes to `paymentsDestination[serviceProvider]`
- Locks `fees × STAKE_TO_FEES_RATIO` via `_lockStake()` (releases after `minThawingPeriod`)

### `withdrawFees(to, amount)`

Owner-only. Transfers accumulated data-service revenue (the 1% `DATA_SERVICE_CUT_PPM` portion retained on each `collect()`) to `to`.

### `addChain(chainId, minProvisionTokens)` / `removeChain(chainId)`

Owner-only chain allowlist management. `minProvisionTokens = 0` uses the protocol default (555 GRT).

### `setMinThawingPeriod(period)`

Governance setter. Lower-bounded by `MIN_THAWING_PERIOD` (14 days).

### `slash(serviceProvider, data)`

No-op — reverts with "slashing not supported". Present to satisfy the `IDataService` interface.

---

## Parameters

| Parameter | Value | Notes |
|---|---|---|
| Minimum provision (`DEFAULT_MIN_PROVISION`) | 555 GRT | Governance-adjustable per chain |
| Burn cut (`BURN_CUT_PPM`) | 1% (10,000 PPM) | Burned from each `collect()` |
| Data service cut (`DATA_SERVICE_CUT_PPM`) | 1% (10,000 PPM) | Retained as revenue; owner withdraws via `withdrawFees()` |
| Minimum thawing period | 14 days | Governance-adjustable, lower-bounded |
| stakeToFeesRatio | 5 | Same as SubgraphService |
