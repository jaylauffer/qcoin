#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  render-node-config.sh \
    --self-ip 10.10.10.1 \
    --output-dir ./out/10.10.10.1 \
    --validator-public-key-hex PUBKEY1 \
    --validator-public-key-hex PUBKEY2 \
    [--validator-public-key-hex PUBKEY3] \
    [--reliable-node-public-key-hex PUBKEY1] \
    [--peer-ip 10.10.10.2] \
    [--peer-ip 10.10.10.3] \
    [--multicast-v6-group ff02::5143:6f69:6e] \
    [--multicast-v6-interface 2] \
    [--produce true]

Writes:
  - qcoin-node.env
  - cluster-manifest.json
  - network-config.json (only when --peer-ip is provided)

Defaults:
  - chain id: 0
  - listen port: 9700
  - binary: /home/jay/pudding/qcoin/target/release/qcoin-node
  - workdir: /home/jay/pudding/qcoin
  - state path: /var/lib/qcoin/qcoin-chain-state.json
  - blocks path: /var/lib/qcoin/qcoin-blocks.json
  - keypair path: /etc/qcoin/node-keypair.json
  - cluster manifest path: /etc/qcoin/cluster-manifest.json
  - network config path: /etc/qcoin/network-config.json
  - multicast group: ff02::5143:6f69:6e
  - reliable node list: defaults to validator list
  - produce: unset, so the node auto-produces only when its key is in the manifest validator set
EOF
}

self_ip=""
output_dir=""
chain_id="0"
listen_port="9700"
binary_path="/home/jay/pudding/qcoin/target/release/qcoin-node"
workdir="/home/jay/pudding/qcoin"
state_path="/var/lib/qcoin/qcoin-chain-state.json"
blocks_path="/var/lib/qcoin/qcoin-blocks.json"
interval_seconds="5"
sync_interval_seconds="3"
produce=""
scheme="dilithium2"
keypair_json="/etc/qcoin/node-keypair.json"
cluster_manifest_json="/etc/qcoin/cluster-manifest.json"
network_config_json="/etc/qcoin/network-config.json"
multicast_v6_group="ff02::5143:6f69:6e"
multicast_v6_interface=""
declare -a validator_keys=()
declare -a reliable_keys=()
declare -a peer_ips=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --self-ip)
      self_ip="${2:-}"
      shift 2
      ;;
    --output-dir)
      output_dir="${2:-}"
      shift 2
      ;;
    --chain-id)
      chain_id="${2:-}"
      shift 2
      ;;
    --listen-port)
      listen_port="${2:-}"
      shift 2
      ;;
    --binary-path)
      binary_path="${2:-}"
      shift 2
      ;;
    --workdir)
      workdir="${2:-}"
      shift 2
      ;;
    --state-path)
      state_path="${2:-}"
      shift 2
      ;;
    --blocks-path)
      blocks_path="${2:-}"
      shift 2
      ;;
    --interval-seconds)
      interval_seconds="${2:-}"
      shift 2
      ;;
    --sync-interval-seconds)
      sync_interval_seconds="${2:-}"
      shift 2
      ;;
    --produce)
      produce="${2:-}"
      shift 2
      ;;
    --scheme)
      scheme="${2:-}"
      shift 2
      ;;
    --keypair-json)
      keypair_json="${2:-}"
      shift 2
      ;;
    --cluster-manifest-json)
      cluster_manifest_json="${2:-}"
      shift 2
      ;;
    --network-config-json)
      network_config_json="${2:-}"
      shift 2
      ;;
    --validator-public-key-hex)
      validator_keys+=("${2:-}")
      shift 2
      ;;
    --reliable-node-public-key-hex)
      reliable_keys+=("${2:-}")
      shift 2
      ;;
    --peer-ip)
      peer_ips+=("${2:-}")
      shift 2
      ;;
    --multicast-v6-group)
      multicast_v6_group="${2:-}"
      shift 2
      ;;
    --multicast-v6-interface)
      multicast_v6_interface="${2:-}"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ -z "$self_ip" || -z "$output_dir" ]]; then
  echo "--self-ip and --output-dir are required" >&2
  usage >&2
  exit 1
fi

if [[ ${#validator_keys[@]} -eq 0 ]]; then
  echo "At least one --validator-public-key-hex is required" >&2
  exit 1
fi

if [[ ${#reliable_keys[@]} -eq 0 ]]; then
  reliable_keys=("${validator_keys[@]}")
fi

mkdir -p "$output_dir"

env_path="$output_dir/qcoin-node.env"
manifest_path="$output_dir/cluster-manifest.json"
network_path="$output_dir/network-config.json"

cat >"$env_path" <<EOF
QCOIN_BINARY=$binary_path
QCOIN_WORKDIR=$workdir
QCOIN_STATE_PATH=$state_path
QCOIN_BLOCKS_PATH=$blocks_path
QCOIN_LISTEN=$self_ip:$listen_port
QCOIN_INTERVAL_SECONDS=$interval_seconds
QCOIN_SYNC_INTERVAL_SECONDS=$sync_interval_seconds
QCOIN_SCHEME=$scheme
QCOIN_KEYPAIR_JSON=$keypair_json
QCOIN_CLUSTER_MANIFEST_JSON=$cluster_manifest_json
EOF

if [[ -n "$produce" ]]; then
  printf 'QCOIN_PRODUCE=%s\n' "$produce" >>"$env_path"
fi

if [[ ${#peer_ips[@]} -gt 0 ]]; then
  printf 'QCOIN_NETWORK_CONFIG_JSON=%s\n' "$network_config_json" >>"$env_path"
fi

{
  printf '{\n'
  printf '  "chain_id": %s,\n' "$chain_id"
  printf '  "validator_public_key_hex": [\n'
  for i in "${!validator_keys[@]}"; do
    comma=","
    if [[ "$i" -eq $((${#validator_keys[@]} - 1)) ]]; then
      comma=""
    fi
    printf '    "%s"%s\n' "${validator_keys[$i]}" "$comma"
  done
  printf '  ],\n'
  printf '  "reliable_node_public_key_hex": [\n'
  for i in "${!reliable_keys[@]}"; do
    comma=","
    if [[ "$i" -eq $((${#reliable_keys[@]} - 1)) ]]; then
      comma=""
    fi
    printf '    "%s"%s\n' "${reliable_keys[$i]}" "$comma"
  done
  printf '  ],\n'
  printf '  "multicast_v6": [\n'
  printf '    {\n'
  printf '      "group": "%s"' "$multicast_v6_group"
  if [[ -n "$multicast_v6_interface" ]]; then
    printf ',\n'
    printf '      "interface": %s\n' "$multicast_v6_interface"
  else
    printf '\n'
  fi
  printf '    }\n'
  printf '  ]\n'
  printf '}\n'
} >"$manifest_path"

if [[ ${#peer_ips[@]} -gt 0 ]]; then
  {
    printf '{\n'
    printf '  "peers": [\n'
    for i in "${!peer_ips[@]}"; do
      comma=","
      if [[ "$i" -eq $((${#peer_ips[@]} - 1)) ]]; then
        comma=""
      fi
      printf '    "http://%s:%s"%s\n' "${peer_ips[$i]}" "$listen_port" "$comma"
    done
    printf '  ]\n'
    printf '}\n'
  } >"$network_path"
  echo "Wrote $network_path"
fi

echo "Wrote $env_path"
echo "Wrote $manifest_path"
