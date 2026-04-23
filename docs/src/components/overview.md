# Components

The Dispatch workspace contains five Rust crates and two TypeScript packages.

| Crate | Role |
|---|---|
| `dispatch-tap` | Shared EIP-712 types and receipt signing primitives |
| `dispatch-service` | Indexer-side JSON-RPC proxy with TAP middleware |
| `dispatch-gateway` | Consumer-facing gateway: routing, QoS, receipt issuance |
| `dispatch-oracle` | Block header oracle feeding state roots on-chain |
| `dispatch-smoke` | End-to-end smoke test binary |

| Package | Role |
|---|---|
| `@dispatch/consumer-sdk` | TypeScript SDK for dApp developers |
| `@dispatch/indexer-agent` | TypeScript lifecycle agent for providers |
