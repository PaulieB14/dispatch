# Day Of: GRT Staking + Dispatch Live Checklist

Everything you need the day your 10,000 GRT lands. Estimated time: ~30 minutes from "GRT in wallet" to "service registered on-chain."

## Pre-flight (do these BEFORE GRT arrives)

- [ ] **Provider wallet picked.** Fresh MetaMask EOA recommended. Set as `PROVIDER_ADDRESS`.
- [ ] **paymentsDestination wallet picked.** Cold wallet. Can equal provider, or use `graphadvocate.eth` (`0x575267eED09c338FAE5716A486A7B58A5749A292`).
- [ ] **Operator key generated.** ✅ Already done — `0x6722C12779980a62edAe4895B45585214e0671eB` at `~/.dispatch_operator_key`.
- [ ] **Alchemy Arbitrum URL ready.** ✅ Stored in `~/.dispatch_secrets.env`.
- [ ] **Repo pushed to GitHub.** Push your fork of `dispatch-provider` so Railway can pull it.
- [ ] **Railway services created** (Postgres + dispatch-service) but env vars empty. Don't deploy yet.

## When GRT lands

### Step 1 — Verify balance

```bash
export PROVIDER_ADDRESS=0xYOUR_PROVIDER_ADDRESS
export RPC=https://arb1.arbitrum.io/rpc

cast call 0x9623063377AD1B27544C965cCd7342f7EA7e88C7 \
  "balanceOf(address)(uint256)" \
  $PROVIDER_ADDRESS \
  --rpc-url $RPC
# Should print >= 10000000000000000000000

cast balance $PROVIDER_ADDRESS --rpc-url $RPC
# Should be a few finney for gas (~$3-5 of ETH minimum)
```

### Step 2 — Approve HorizonStaking to spend GRT

You will be prompted for the provider key. Don't paste it on the command line; use `cast wallet` or import to a Foundry keystore once.

```bash
# Set up a keystore once (interactive — paste private key, set password):
#   cast wallet import dispatch-provider --interactive
# Then reference by name:

cast send 0x9623063377AD1B27544C965cCd7342f7EA7e88C7 \
  "approve(address,uint256)" \
  0x00669A4CF01450B64E8A2A20E9b1FCB71E61eF03 \
  10000000000000000000000 \
  --account dispatch-provider \
  --rpc-url $RPC
```

### Step 3 — Stake 10,000 GRT to your provider address

```bash
cast send 0x00669A4CF01450B64E8A2A20E9b1FCB71E61eF03 \
  "stakeTo(address,uint256)" \
  $PROVIDER_ADDRESS \
  10000000000000000000000 \
  --account dispatch-provider \
  --rpc-url $RPC
```

### Step 4 — Provision for RPCDataService

```bash
cast send 0x00669A4CF01450B64E8A2A20E9b1FCB71E61eF03 \
  "provision(address,address,uint256,uint32,uint64)" \
  $PROVIDER_ADDRESS \
  0xA983b18B8291F0c317Ba4Fe0dc0f7cc9373AF078 \
  10000000000000000000000 \
  1000000 \
  1209600 \
  --account dispatch-provider \
  --rpc-url $RPC
```

Args explained:
- `0xA983...` — RPCDataService contract
- `10000000000000000000000` — 10k GRT in wei
- `1000000` — maxVerifierCut = 100% PPM (no slashing implemented, this number doesn't matter in practice)
- `1209600` — thawingPeriod = 14 days in seconds (contract minimum)

### Step 5 — Authorize the operator key

This lets your Railway operator sign on behalf of the provider.

```bash
cast send 0x00669A4CF01450B64E8A2A20E9b1FCB71E61eF03 \
  "setOperator(address,address,bool)" \
  0xA983b18B8291F0c317Ba4Fe0dc0f7cc9373AF078 \
  0x6722C12779980a62edAe4895B45585214e0671eB \
  true \
  --account dispatch-provider \
  --rpc-url $RPC
```

### Step 6 — Verify provision on-chain

```bash
cast call 0x00669A4CF01450B64E8A2A20E9b1FCB71E61eF03 \
  "getProvision(address,address)(uint256,uint256,uint256,uint32,uint64,uint64,uint32,uint64,uint256,uint32)" \
  $PROVIDER_ADDRESS \
  0xA983b18B8291F0c317Ba4Fe0dc0f7cc9373AF078 \
  --rpc-url $RPC
# First number must be >= 10000000000000000000000
```

### Step 7 — Set Railway env vars on dispatch-service

In Railway dashboard → dispatch-service → Variables:

| Variable | Value |
|---|---|
| `SERVICE_PROVIDER_ADDRESS` | `0xYOUR_PROVIDER_ADDRESS` |
| `DISPATCH_OPERATOR_PRIVATE_KEY` | `cat ~/.dispatch_operator_key` |
| `DISPATCH_ARBITRUM_RPC` | `cat ~/.dispatch_secrets.env` (DISPATCH_ARBITRUM_RPC line) |
| `DISPATCH_ARBITRUM_BACKEND` | same Alchemy URL |
| `DATABASE_URL` | auto-injected by Railway Postgres plugin |
| `RUST_LOG` | `info` |

### Step 8 — Deploy

Trigger a Railway redeploy. Watch logs for:

```
INFO dispatch_service::server: dispatch-service starting addr=0.0.0.0:7700
INFO dispatch_service::collector: on-chain RAV collector started interval_secs=3600
```

Then verify HTTPS:
```bash
curl https://dispatch-service-production.up.railway.app/health
# {"status":"ok"}
```

### Step 9 — Run indexer-agent ONCE to register on-chain

```bash
cd ~/dispatch-provider/indexer-agent
npm install
SERVICE_PROVIDER_ADDRESS=0xYOUR_PROVIDER_ADDRESS \
DISPATCH_OPERATOR_PRIVATE_KEY=$(cat ~/.dispatch_operator_key) \
DISPATCH_ARBITRUM_RPC=$(grep DISPATCH_ARBITRUM_RPC ~/.dispatch_secrets.env | cut -d'"' -f2) \
PAYMENTS_DESTINATION=0xYOUR_COLD_WALLET \
DISPATCH_ENDPOINT=https://dispatch-service-production.up.railway.app \
AGENT_CONFIG=$PWD/../agent.config.json \
npx tsx src/index.ts
```

Will call `register()` then `startService(42161, 0)`. ~$0.50 in gas.

### Step 10 — Verify registration

```bash
cast call 0xA983b18B8291F0c317Ba4Fe0dc0f7cc9373AF078 \
  "isRegistered(address)(bool)" \
  $PROVIDER_ADDRESS \
  --rpc-url $RPC
# Must be true

cast call 0xA983b18B8291F0c317Ba4Fe0dc0f7cc9373AF078 \
  "getChainRegistrations(address)" \
  $PROVIDER_ADDRESS \
  --rpc-url $RPC
# Should show (42161, 0, active=true)
```

🎉 Live. Gateway traffic should start flowing within minutes. RAV collection auto-runs hourly.

## Post-launch monitoring

```bash
# Watch logs
railway logs --deployment

# Look for:
# INFO dispatch_service::tap_aggregator: RAV aggregated collection_id=... value=...   (every 60s when traffic flows)
# INFO dispatch_service::collector: collect() success tx=0x...                       (every hour after first earnings)
```

## If something breaks

| Symptom | Likely cause |
|---|---|
| Boot fails: `ERROR: required env var X is not set` | Missing Railway env var |
| Boot fails: `failed to connect to postgres` | DATABASE_URL not injected — Postgres service not linked |
| `isRegistered` returns false after step 9 | indexer-agent didn't reach the contract — check operator key auth (step 5) |
| 401 on legit traffic | Gateway's signer not in `authorized_senders` (currently `[]` = accept any) |
| `collect()` failing | Operator wallet has no ETH on Arbitrum for gas |
