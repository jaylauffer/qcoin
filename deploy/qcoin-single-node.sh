#!/usr/bin/env bash
set -euo pipefail

CONFIG_PATH="${QCOIN_CONFIG:-/etc/qcoin/single-node.ini}"
SECTION=""

if [[ ! -f "$CONFIG_PATH" ]]; then
  echo "Config file not found: ${CONFIG_PATH}" >&2
  exit 1
fi

value_of() {
  local key="$1"
  awk -F= -v section="[$2]" -v key="$key" '
    $0 ~ /^\[/ { in_section = ($0 == section); next }
    in_section && $1 == key { gsub(/^[ \t]+|[ \t]+$/, "", $2); print $2; exit }
  ' "$CONFIG_PATH"
}

NODE_SECTION="node"
NODE_BINARY="$(value_of binary "$NODE_SECTION")"
STATE_PATH="$(value_of state_path "$NODE_SECTION")"
INTERVAL_SECONDS="$(value_of interval_seconds "$NODE_SECTION")"
RUN_ONCE="$(value_of run_once "$NODE_SECTION")"
WORKING_DIR="$(value_of working_dir "$NODE_SECTION")"

if [[ -z "${NODE_BINARY:-}" || -z "${STATE_PATH:-}" || -z "${INTERVAL_SECONDS:-}" || -z "${RUN_ONCE:-}" || -z "${WORKING_DIR:-}" ]]; then
  echo "Missing required values in ${CONFIG_PATH}" >&2
  exit 1
fi

RUN_ARGS=(run --state-path "$STATE_PATH" --interval-seconds "$INTERVAL_SECONDS")
if [[ "${RUN_ONCE,,}" == "true" ]]; then
  RUN_ARGS+=(--once)
fi

cd "$WORKING_DIR"
exec "$NODE_BINARY" "${RUN_ARGS[@]}"
