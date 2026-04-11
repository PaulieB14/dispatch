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

```bash
cp .env.example .env
# fill in PRIVATE_KEY, OWNER, PAUSE_GUARDIAN, GRT_TOKEN, GRAPH_CONTROLLER, GRAPH_TALLY_COLLECTOR

# Arbitrum Sepolia (testnet)
forge script script/Deploy.s.sol --rpc-url arbitrum_sepolia --broadcast --verify -vvvv
```

## Key parameters

| Parameter | Value | Adjustable |
|---|---|---|
| Default minimum provision | 25,000 GRT per chain | No (constant) |
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
