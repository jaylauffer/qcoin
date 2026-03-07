#!/usr/bin/env bash
set -euo pipefail

: "${QCOIN_BINARY:?QCOIN_BINARY is required}"
: "${QCOIN_WORKDIR:?QCOIN_WORKDIR is required}"
: "${QCOIN_STATE_PATH:?QCOIN_STATE_PATH is required}"
: "${QCOIN_BLOCKS_PATH:?QCOIN_BLOCKS_PATH is required}"
: "${QCOIN_LISTEN:?QCOIN_LISTEN is required}"
: "${QCOIN_KEYPAIR_JSON:?QCOIN_KEYPAIR_JSON is required}"
: "${QCOIN_NETWORK_CONFIG_JSON:?QCOIN_NETWORK_CONFIG_JSON is required}"

QCOIN_INTERVAL_SECONDS="${QCOIN_INTERVAL_SECONDS:-5}"
QCOIN_SYNC_INTERVAL_SECONDS="${QCOIN_SYNC_INTERVAL_SECONDS:-3}"
QCOIN_PRODUCE="${QCOIN_PRODUCE:-true}"
QCOIN_SCHEME="${QCOIN_SCHEME:-dilithium2}"

mkdir -p "$(dirname "$QCOIN_STATE_PATH")" "$(dirname "$QCOIN_BLOCKS_PATH")" /var/log/qcoin

run_args=(
  run
  --state-path "$QCOIN_STATE_PATH"
  --blocks-path "$QCOIN_BLOCKS_PATH"
  --listen "$QCOIN_LISTEN"
  --interval-seconds "$QCOIN_INTERVAL_SECONDS"
  --sync-interval-seconds "$QCOIN_SYNC_INTERVAL_SECONDS"
  --produce="$QCOIN_PRODUCE"
  --scheme "$QCOIN_SCHEME"
  --keypair-json "$QCOIN_KEYPAIR_JSON"
  --network-config-json "$QCOIN_NETWORK_CONFIG_JSON"
)

cd "$QCOIN_WORKDIR"
exec "$QCOIN_BINARY" "${run_args[@]}"
