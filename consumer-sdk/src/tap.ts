import { createWalletClient, http } from "viem";
import { privateKeyToAccount } from "viem/accounts";
import { arbitrum } from "viem/chains";
import type { TapReceipt, SignedTapReceipt } from "./types.js";

// TAP v2 EIP-712 type definition.
// ALL field names are snake_case to match the Rust Receipt struct exactly.
// The EIP-712 type hash includes field names, so any deviation breaks cross-language
// signature verification.
const TAP_RECEIPT_TYPES = {
  Receipt: [
    { name: "data_service",     type: "address" },
    { name: "service_provider", type: "address" },
    { name: "timestamp_ns",     type: "uint64" },
    { name: "nonce",            type: "uint64" },
    { name: "value",            type: "uint128" },
    { name: "metadata",         type: "bytes" },
  ],
} as const;

export interface TapDomain {
  /** GraphTallyCollector contract address — the EIP-712 verifying contract. */
  verifyingContract: `0x${string}`;
  /** Defaults to 42161 (Arbitrum One). */
  chainId?: number;
  /** EIP-712 domain name. Defaults to "GraphTallyCollector". */
  name?: string;
}

/**
 * Sign a TAP receipt using EIP-712 typed data signing.
 *
 * The consumer (gateway) signs one receipt per request and attaches it in the
 * `X-Dispatch-Tap-Receipt` header. The provider batches receipts into a RAV and
 * submits it on-chain via `collect()`.
 */
export async function signReceipt(
  receipt: TapReceipt,
  domain: TapDomain,
  privateKey: `0x${string}`
): Promise<SignedTapReceipt> {
  const account = privateKeyToAccount(privateKey);
  const client = createWalletClient({
    account,
    chain: arbitrum,
    transport: http(),
  });

  const signature = await client.signTypedData({
    domain: {
      name: domain.name ?? "GraphTallyCollector",
      version: "1",
      chainId: domain.chainId ?? 42161,
      verifyingContract: domain.verifyingContract,
    },
    types: TAP_RECEIPT_TYPES,
    primaryType: "Receipt",
    message: {
      data_service:     receipt.dataService,
      service_provider: receipt.serviceProvider,
      timestamp_ns:     receipt.timestampNs,
      nonce:            receipt.nonce,
      value:            receipt.value,
      metadata:         receipt.metadata,
    },
  });

  return { receipt, signature };
}

/**
 * Serialise a SignedTapReceipt to the JSON wire format expected by dispatch-service.
 *
 * Uses template literals instead of JSON.stringify because:
 * 1. JSON.stringify throws on BigInt values.
 * 2. The Rust serde structs use snake_case field names, but TapReceipt uses camelCase.
 * 3. u64/u128 fields must be bare number literals in JSON, not strings.
 *    BigInt.toString() preserves exact decimal digits, so inlining via template
 *    literals gives serde_json the exact integer it needs.
 */
export function serializeSignedReceipt(sr: SignedTapReceipt): string {
  const r = sr.receipt;
  return `{"receipt":{"data_service":"${r.dataService}","service_provider":"${r.serviceProvider}","timestamp_ns":${r.timestampNs},"nonce":${r.nonce},"value":${r.value},"metadata":"${r.metadata}"},"signature":"${sr.signature}"}`;
}

/**
 * Construct a fresh TapReceipt with a monotonic timestamp and random nonce.
 */
export function buildReceipt(
  dataService: `0x${string}`,
  serviceProvider: `0x${string}`,
  value: bigint,
  metadata: `0x${string}` = "0x"
): TapReceipt {
  return {
    dataService,
    serviceProvider,
    timestampNs: BigInt(Date.now()) * 1_000_000n,
    nonce: BigInt(Math.floor(Math.random() * Number.MAX_SAFE_INTEGER)),
    value,
    metadata,
  };
}
