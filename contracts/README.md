# RPCDataService — Solidity

## Setup

```bash
# Install Foundry (if needed)
curl -L https://foundry.paradigm.xyz | bash && foundryup

# Install dependencies
forge install graphprotocol/contracts
forge install OpenZeppelin/openzeppelin-contracts
forge install OpenZeppelin/openzeppelin-contracts-upgradeable

# Build
forge build

# Test (unit tests with mocks)
forge test -vvv

# Fork tests against Arbitrum Sepolia
forge test --fork-url $ARBITRUM_SEPOLIA_RPC_URL -vvv
```

## Deployment

The contract deploys as a **UUPS upgradeable proxy** (ERC1967). The script deploys the implementation then a proxy, calling `initialize()` atomically.

```bash
cp .env.example .env
# fill in PRIVATE_KEY, OWNER, PAUSE_GUARDIAN, GRAPH_CONTROLLER, GRAPH_TALLY_COLLECTOR

# Arbitrum Sepolia (testnet)
forge script script/Deploy.s.sol --rpc-url arbitrum_sepolia --broadcast --verify -vvvv
```

The script logs both `RPC_DATA_SERVICE_ADDRESS` (proxy) and `RPC_DATA_SERVICE_IMPL` (implementation). Set `RPC_DATA_SERVICE_ADDRESS` in your `.env` — the proxy is the address all downstream services use.

## Key parameters

| Parameter | Value | Adjustable |
|---|---|---|
| Default minimum provision | 555 GRT | No (constant) |
| Burn cut | 1% of fees (`BURN_CUT_PPM = 10_000`) | No (constant) |
| Data service cut | 1% of fees (`DATA_SERVICE_CUT_PPM = 10_000`) | No (constant) |
| Minimum thawing period floor | 14 days | No (constant lower bound) |
| Minimum thawing period | 14 days initially | Yes — `setMinThawingPeriod()` (owner) |
| Stake-to-fees ratio | 5 (5:1) | No |
| Slash amount | 10,000 GRT | No |
| Challenger reward | 50% of slashed amount | No |
| Chain bond amount | 100,000 GRT | No |
| Network | Arbitrum One (chain ID 42161) | — |

## Contract functions

### Governance (owner-only)

| Function | Description |
|---|---|
| `addChain(chainId, minProvision)` | Add a chain to the supported set |
| `removeChain(chainId)` | Disable a chain (existing registrations unaffected) |
| `approveProposedChain(chainId, minProvision)` | Approve a permissionless chain proposal; refunds bond |
| `rejectProposedChain(chainId)` | Reject a proposal; forfeits bond to treasury |
| `setDefaultMinProvision(tokens)` | Update the default minimum provision |
| `setMinThawingPeriod(period)` | Update minimum thawing period (≥ 14 days) |
| `setTrustedStateRoot(blockHash, stateRoot)` | Register a trusted L1 state root for fraud proof verification |
| `setIssuancePerCU(rate)` | Set GRT issuance rate per compute unit (0 = disabled) |
| `depositRewardsPool(amount)` | Deposit GRT into the rewards pool |
| `withdrawRewardsPool(amount)` | Withdraw unused GRT from the rewards pool |
| `withdrawFees(to, amount)` | Withdraw accumulated data-service revenue (the 1% `DATA_SERVICE_CUT_PPM` portion) |
| `upgradeToAndCall(newImpl, data)` | Upgrade the proxy to a new implementation (UUPS) |

### Provider operations

| Function | Description |
|---|---|
| `register(provider, data)` | Register as a provider (data: `abi.encode(endpoint, geoHash, paymentsDestination)`) |
| `deregister(provider, data)` | Deregister (all services must be stopped first) |
| `startService(provider, data)` | Activate a `(chainId, tier)` service |
| `stopService(provider, data)` | Deactivate a `(chainId, tier)` service |
| `collect(provider, paymentType, data)` | Redeem a signed RAV; accrues issuance rewards if pool is funded |
| `slash(provider, data)` | Submit a Tier 1 EIP-1186 fraud proof |
| `setPaymentsDestination(destination)` | Change the GRT payment recipient address |
| `claimRewards()` | Claim accrued GRT issuance rewards |
| `proposeChain(chainId)` | Propose a new chain permissionlessly (locks 100k GRT bond) |

## Rewards pool

Issuance accrues automatically on every `collect()` call when `issuancePerCU > 0` and the rewards pool has GRT:

```
reward = fees × issuancePerCU / 1e18
reward = min(reward, rewardsPool)
pendingRewards[paymentsDestination] += reward
```

Providers call `claimRewards()` to transfer their `pendingRewards` balance. Governance funds the pool via `depositRewardsPool()`.
