# Getting Started

## Hit the live network (quickest)

The gateway is live. No setup, no key, no GRT — just a standard JSON-RPC call:

```bash
curl -s -X POST http://167.235.29.213:8080/rpc/42161 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'
```

Every response carries an `x-drpc-attestation` header — an ECDSA signature from the provider over `keccak256(chainId || method || params || response || blockHash)`. You can verify this with the consumer SDK (see [Using the Network](consumers.md)).

---

## Smoke test a live provider

Fires real TAP-signed JSON-RPC requests directly at a provider endpoint, bypassing the gateway. Validates that receipts are accepted, responses are correct, and attestations are present.

```bash
# Full validated run against the live provider
DISPATCH_ENDPOINT=http://167.235.29.213:7700 \
DISPATCH_SIGNER_KEY=<gateway-signer-key> \
DISPATCH_PROVIDER_ADDRESS=0xb43B2CCCceadA5292732a8C58ae134AdEFcE09Bb \
cargo run --bin dispatch-smoke
```

Expected output:

```
dispatch-smoke
  endpoint   : http://167.235.29.213:7700
  chain_id   : 42161
  data_svc   : 0xA983b18B8291F0c317Ba4Fe0dc0f7cc9373AF078
  signer     : 0x7D14ae5f20cc2f6421317386Aa8E79e8728353d9

  [PASS] GET /health → 200 OK
  [PASS] eth_blockNumber — returns current block → "0x1b1623cf" [95ms]
  [PASS] eth_chainId — returns 0x61a9 (42161) → "0xa4b1" [58ms]
  [PASS] eth_getBalance — returns balance at latest block (Standard) → "0x6f3a59e597c5342" [74ms]
  [PASS] eth_getBalance — historical block (Archive) → "0x0" [629ms]
  [PASS] eth_getLogs — recent block range (Tier 2 quorum) → [{"address":"0xa62d...] [61ms]

  5 passed, 0 failed
```

`DISPATCH_SIGNER_KEY` must be the private key of an address in the provider's `authorized_senders` list. `DISPATCH_PROVIDER_ADDRESS` must match the provider's registered address exactly — it is embedded in the TAP receipt and validated on-chain.

---

## Run the drop-in proxy

Point MetaMask, Viem, or any Ethereum library at the Dispatch network without changing application code. The proxy runs locally and handles everything — key management, provider discovery, receipt signing.

```bash
cd proxy
npm install
npm start
```

On first run it generates a consumer keypair, prints your consumer address, and links to the funding dashboard. Fund the escrow at [lodestar-dashboard.com/dispatch](https://lodestar-dashboard.com/dispatch), then add `http://localhost:8545` to MetaMask as a custom network.

See [Using the Network → dispatch-proxy](consumers.md#dispatch-proxy-drop-in-local-server) for full configuration options.

---

## Run the local demo

Runs a complete local stack on Anvil — Horizon contracts, dispatch-service, dispatch-gateway — makes 5 RPC requests, submits a RAV, and proves GRT lands in the payment wallet. The full loop in one command.

Requires: [Foundry](https://getfoundry.sh) and Rust stable.

```bash
cd demo
npm install
npm start
```

---

## Build from source

```bash
cargo build
cargo test
```

---

## Docker Compose

The quickest path to a full running stack:

```bash
cp docker/config.example.toml  docker/config.toml
cp docker/gateway.example.toml docker/gateway.toml

# Fill in private keys, provider addresses, and backend RPC URLs
docker compose up
```

The default stack starts `dispatch-service`, `dispatch-gateway`, and PostgreSQL.
