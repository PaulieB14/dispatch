# Deploying dispatch-service on Railway

## Prerequisites
- GRT staked on Arbitrum One (≥ 10,000 GRT in a HorizonStaking provision for RPCDataService)
- Operator key from `~/.dispatch_operator_key` (address `0x6722C12779980a62edAe4895B45585214e0671eB`)
- Free Alchemy account → Arbitrum One HTTPS RPC URL

## Step 1 — Create Railway services

In your existing Railway project (where graph-advocate lives):

1. **Add Postgres**
   - Right-click in the project canvas → "Database" → "PostgreSQL"
   - Railway auto-injects `DATABASE_URL` into other services in the project.

2. **Add dispatch-service**
   - Click "New" → "Empty Service" → name it `dispatch-service`
   - Service Settings → Source → Connect Repo → `PaulieB14/dispatch-provider` (push your fork first)
   - Source → Root Directory → leave blank
   - Source → Dockerfile Path → `docker/Dockerfile.service`
   - Networking → Public Networking → enable, port `7700`

## Step 2 — Set env vars on dispatch-service

| Variable | Value |
|---|---|
| `SERVICE_PROVIDER_ADDRESS` | `0x...` of the wallet holding the GRT provision |
| `DISPATCH_OPERATOR_PRIVATE_KEY` | contents of `~/.dispatch_operator_key` (with `0x` prefix) |
| `DISPATCH_ARBITRUM_RPC` | `https://arb-mainnet.g.alchemy.com/v2/<API_KEY>` |
| `DISPATCH_ARBITRUM_BACKEND` | same Alchemy URL — used for serving requests |
| `DATABASE_URL` | auto-injected by Railway from the Postgres plugin |
| `PAYMENTS_DESTINATION` | cold wallet address that receives collected GRT (can equal provider address for now) |
| `RUST_LOG` | `info` |

## Step 3 — Push the configs

The `docker/config.toml` and `agent.config.json` reference `${VAR}` placeholders. Either:
- Templating layer in your Dockerfile entrypoint, OR
- Pre-process at deploy time

Simplest: write a tiny entrypoint script that runs `envsubst` on `config.toml` before launching `dispatch-service`. Add to `docker/Dockerfile.service`:

```dockerfile
RUN apt-get update && apt-get install -y gettext-base
COPY docker/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
ENTRYPOINT ["/entrypoint.sh"]
```

`docker/entrypoint.sh`:

```bash
#!/bin/bash
envsubst < /etc/dispatch/config.template.toml > /etc/dispatch/config.toml
exec dispatch-service --config /etc/dispatch/config.toml
```

## Step 4 — Deploy & verify

1. Push to GitHub → Railway auto-deploys
2. Check logs:
   ```
   INFO dispatch_service::server: dispatch-service starting addr=0.0.0.0:7700
   ```
3. Test the public URL:
   ```
   curl https://dispatch-service-production.up.railway.app/health
   ```
   Should return `{"status":"ok"}`.

## Step 5 — Run the indexer-agent (one time)

This registers your service on-chain.

```bash
cd ~/dispatch-provider/indexer-agent
npm install
SERVICE_PROVIDER_ADDRESS=0x... \
DISPATCH_OPERATOR_PRIVATE_KEY=$(cat ~/.dispatch_operator_key) \
DISPATCH_ARBITRUM_RPC=https://... \
PAYMENTS_DESTINATION=0x... \
AGENT_CONFIG=../agent.config.json \
npx tsx src/index.ts
```

The agent calls `register()` and `startService(42161, 0)` on-chain. ~$0.50 in gas.

## Step 6 — Verify on-chain

```bash
cast call 0xA983b18B8291F0c317Ba4Fe0dc0f7cc9373AF078 \
  "isRegistered(address)(bool)" \
  $SERVICE_PROVIDER_ADDRESS \
  --rpc-url https://arb1.arbitrum.io/rpc
```

Should return `true`.

## What's still TBD pre-stake

- [ ] Acquire 10,000 GRT on Arbitrum One
- [ ] Sign up for Alchemy / Chainstack free tier → grab Arbitrum HTTPS RPC URL
- [ ] Create Railway Postgres + dispatch-service services (no env vars yet)
- [ ] Push to a new repo `PaulieB14/dispatch-provider` so Railway can connect
- [ ] (Optional) write the `entrypoint.sh` envsubst layer

## Once GRT lands (the actual deploy)

Three on-chain transactions, then a config update:

1. Approve HorizonStaking to spend 10,000 GRT
2. `stakeTo(provider, 10_000e18)`
3. `provision(provider, 0xA983..., 10_000e18, 1_000_000, 1_209_600)`
4. (If operator ≠ provider) `setOperator(0xA983..., 0x6722...., true)` — authorize the operator key generated above
5. Set Railway env vars from Step 2 → service comes online
6. Run indexer-agent → registered on-chain
7. Live, earning per-request GRT
