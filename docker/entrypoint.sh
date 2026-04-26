#!/bin/bash
# Railway entrypoint: substitute env vars into the config template, then exec dispatch-service.
#
# Required env vars (set as Railway service variables):
#   SERVICE_PROVIDER_ADDRESS         — your on-chain provider address (the wallet holding GRT provision)
#   DISPATCH_OPERATOR_PRIVATE_KEY    — operator hot key, with 0x prefix
#   DISPATCH_ARBITRUM_RPC            — Alchemy/Chainstack Arbitrum One HTTPS URL
#   DISPATCH_ARBITRUM_BACKEND        — backend RPC for serving (can equal DISPATCH_ARBITRUM_RPC)
#   DATABASE_URL                     — auto-injected by Railway Postgres plugin
set -euo pipefail

CONFIG_TEMPLATE=${CONFIG_TEMPLATE:-/etc/dispatch/config.template.toml}
CONFIG_OUT=${DISPATCH_CONFIG:-/etc/dispatch/config.toml}

# Sanity-check required vars
for var in SERVICE_PROVIDER_ADDRESS DISPATCH_OPERATOR_PRIVATE_KEY DISPATCH_ARBITRUM_RPC DISPATCH_ARBITRUM_BACKEND DATABASE_URL; do
  if [ -z "${!var:-}" ]; then
    echo "ERROR: required env var $var is not set" >&2
    exit 1
  fi
done

mkdir -p "$(dirname "$CONFIG_OUT")"
envsubst < "$CONFIG_TEMPLATE" > "$CONFIG_OUT"

exec dispatch-service --config "$CONFIG_OUT"
