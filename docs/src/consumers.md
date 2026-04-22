# Using the Network

Two ways to consume the Dispatch network: hit the gateway directly (zero setup) or use the consumer SDK (trustless, signs receipts locally).

---

## Via the Gateway

The gateway handles provider selection and TAP receipt signing. It exposes a standard JSON-RPC interface — point any Ethereum library at it.

**Live gateway:** `http://167.235.29.213:8080`

```bash
# curl
curl -s -X POST http://167.235.29.213:8080/rpc/42161 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'

# Check the attestation header
curl -si -X POST http://167.235.29.213:8080/rpc/42161 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
  | grep -E "x-drpc-attestation|result"
```

With ethers.js or viem, just swap in the gateway URL:

```typescript
import { createPublicClient, http } from "viem";
import { arbitrum } from "viem/chains";

const client = createPublicClient({
  chain: arbitrum,
  transport: http("http://167.235.29.213:8080/rpc/42161"),
});

const block = await client.getBlockNumber();
```

**Routes:**
```
POST /rpc/{chain_id}     # chain ID in path
POST /rpc                # chain ID via X-Chain-Id header
```

Currently live: **Arbitrum One (42161)** — Standard and Archive tiers.

---

## dispatch-proxy (drop-in local server)

The easiest way to point any existing app at the Dispatch network without changing application code. Starts a standard JSON-RPC HTTP server on localhost; MetaMask, Viem, Ethers.js, and curl all work against it without modification.

```bash
cd proxy
npm install
export DISPATCH_SIGNER_KEY=0x<your-private-key>
npm start
```

On startup it prints the MetaMask instructions and begins logging every request:

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
dispatch-proxy v0.1.0
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Chain:     Ethereum Mainnet (1)
Listening: http://localhost:8545
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Add to MetaMask → Settings → Networks → Add a network
  RPC URL:  http://localhost:8545
  Chain ID: 1
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

[12:34:56] ✓ eth_blockNumber      42ms  0.000004 GRT   total: 0.000004 GRT
[12:34:57] ✓ eth_getBalance       38ms  0.000008 GRT   total: 0.000012 GRT
```

**Configuration:**

| Variable | Default | Description |
|---|---|---|
| `DISPATCH_SIGNER_KEY` | *required* | Consumer private key for signing TAP receipts |
| `DISPATCH_CHAIN_ID` | `1` | Chain to proxy (1 = Ethereum, 42161 = Arbitrum One, etc.) |
| `DISPATCH_PORT` | `8545` | Local port to listen on |
| `DISPATCH_BASE_PRICE_PER_CU` | `4000000000000` | GRT wei per compute unit |

The proxy handles provider discovery, TAP receipt signing, QoS-scored provider selection, CORS, and JSON-RPC batch requests. On exit (Ctrl+C) it prints a session summary of total requests and GRT spent.

Unlike the gateway, the proxy runs locally and signs receipts with your own key — you pay providers directly from your own escrow. See [Funding the escrow](#funding-the-escrow) below.

---

## Consumer SDK

For trustless access — signs receipts locally and talks directly to providers, no gateway in the loop.

```bash
npm install @lodestar-dispatch/consumer-sdk
```

```typescript
import { DISPATCHClient } from "@lodestar-dispatch/consumer-sdk";

const client = new DISPATCHClient({
  chainId: 42161,                                               // Arbitrum One (only live chain)
  dataServiceAddress: "0xA983b18B8291F0c317Ba4Fe0dc0f7cc9373AF078",
  graphTallyCollector: "0x8f69F5C07477Ac46FBc491B1E6D91E2bb0111A9e",
  subgraphUrl: "https://api.studio.thegraph.com/query/1747796/rpc-network/v0.2.0",
  signerPrivateKey: process.env.CONSUMER_KEY as `0x${string}`,
  basePricePerCU: 4_000_000_000_000n,  // GRT wei per compute unit
});

const blockNumber = await client.request("eth_blockNumber", []);
const balance = await client.request("eth_getBalance", ["0x...", "latest"]);
```

The client discovers providers via the subgraph, selects one by QoS score, signs a TAP receipt per request, and tracks latency with an EMA.

### Low-level utilities

```typescript
import {
  discoverProviders,
  selectProvider,
  buildReceipt,
  signReceipt,
} from "@lodestar-dispatch/consumer-sdk";

// Discover active providers for a chain + tier
const providers = await discoverProviders({
  subgraphUrl: "https://api.studio.thegraph.com/query/1747796/rpc-network/v0.2.0",
  chainId: 42161,
  tier: 0,  // 0 = Standard, 1 = Archive
});

const provider = selectProvider(providers);

// Build and sign a receipt
const receipt = buildReceipt({
  dataService: "0xA983b18B8291F0c317Ba4Fe0dc0f7cc9373AF078",
  serviceProvider: provider.address,
  value: 4_000_000_000_000n,
});
const signed = await signReceipt(receipt, privateKey);
```

---

## Funding the escrow

Before GRT flows to providers you need to deposit into `PaymentsEscrow` on Arbitrum One. This is only required for the consumer SDK (direct provider access) — the gateway manages its own escrow.

```solidity
// 1. Approve the escrow contract
GRT.approve(0xf6Fcc27aAf1fcD8B254498c9794451d82afC673E, amount);

// 2. Deposit — keyed by (payer=you, collector=GraphTallyCollector, receiver=provider)
PaymentsEscrow.deposit(
    0x8f69F5C07477Ac46FBc491B1E6D91E2bb0111A9e,  // collector: GraphTallyCollector
    providerAddress,                                // receiver: the indexer you're paying
    amount
);
```

Deposits are keyed by `(payer, collector, receiver)`. `dispatch-service` draws down automatically on each `collect()` cycle (hourly by default). Check your balance with:

```bash
cast call 0xf6Fcc27aAf1fcD8B254498c9794451d82afC673E \
  "getBalance(address,address,address)(uint256)" \
  <YOUR_ADDRESS> \
  0x8f69F5C07477Ac46FBc491B1E6D91E2bb0111A9e \
  <PROVIDER_ADDRESS> \
  --rpc-url https://arb1.arbitrum.io/rpc
```
