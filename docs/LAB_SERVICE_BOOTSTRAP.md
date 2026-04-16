# Lab Service Bootstrap

Purpose: define the documented service-managed bootstrap path for the current
three-node qcoin lab:

- `linux-node-a`
- `linux-node-b`
- `macos-node`

This runbook is the operator path we should use when validating restart
behavior for [QCOIN_EXIT_GATE.md](QCOIN_EXIT_GATE.md).

## Current gate status

The cluster is operational, but this runbook exists because the qcoin exit gate
is not fully met yet.

What still remains:

- service-managed restart validation
- repeated ingress checks against all three nodes
- reduction of remaining node-info exchange / duplicate-submit log noise

## Shared prerequisites

On all three machines:

- `/etc/qcoin/node-keypair.json` exists
- `/etc/qcoin/cluster-manifest.json` exists and is identical across the cluster
- the validator order in the manifest is identical on all three machines
- the qcoin repo checkout exists at the expected local path

Current lab paths:

- Linux nodes: `/home/jay/pudding/qcoin`
- macOS node: `/Users/jay/pudding/qcoin`

## Build the release binary

Run on each node from the qcoin repo root:

```bash
cargo build --release -p qcoin-node
```

Expected binary paths:

- Linux: `/home/jay/pudding/qcoin/target/release/qcoin-node`
- macOS: `/Users/jay/pudding/qcoin/target/release/qcoin-node`

## Linux nodes: systemd

Applies to:

- `linux-node-a`
- `linux-node-b`

### Install service assets

```bash
cd /home/jay/pudding/qcoin
sudo install -d -m 0755 /etc/qcoin /var/lib/qcoin /var/log/qcoin
sudo install -m 0755 deploy/qcoin-node-launch.sh /usr/local/bin/qcoin-node-launch.sh
sudo install -m 0644 deploy/qcoin-node.service /etc/systemd/system/qcoin-node.service
```

### Install per-node env file

On `linux-node-a`:

```bash
sudo tee /etc/qcoin/qcoin-node.env >/dev/null <<'EOF'
QCOIN_BINARY=/home/jay/pudding/qcoin/target/release/qcoin-node
QCOIN_WORKDIR=/home/jay/pudding/qcoin
QCOIN_STATE_PATH=/var/lib/qcoin/qcoin-chain-state.json
QCOIN_BLOCKS_PATH=/var/lib/qcoin/qcoin-blocks.json
QCOIN_LISTEN=<linux_node_a_lan_ip>:9700
QCOIN_INTERVAL_SECONDS=5
QCOIN_SYNC_INTERVAL_SECONDS=3
QCOIN_SCHEME=dilithium2
QCOIN_KEYPAIR_JSON=/etc/qcoin/node-keypair.json
QCOIN_CLUSTER_MANIFEST_JSON=/etc/qcoin/cluster-manifest.json
QCOIN_LOG_DIR=/var/log/qcoin
EOF
```

On `linux-node-b`:

```bash
sudo tee /etc/qcoin/qcoin-node.env >/dev/null <<'EOF'
QCOIN_BINARY=/home/jay/pudding/qcoin/target/release/qcoin-node
QCOIN_WORKDIR=/home/jay/pudding/qcoin
QCOIN_STATE_PATH=/var/lib/qcoin/qcoin-chain-state.json
QCOIN_BLOCKS_PATH=/var/lib/qcoin/qcoin-blocks.json
QCOIN_LISTEN=<linux_node_b_lan_ip>:9700
QCOIN_INTERVAL_SECONDS=5
QCOIN_SYNC_INTERVAL_SECONDS=3
QCOIN_SCHEME=dilithium2
QCOIN_KEYPAIR_JSON=/etc/qcoin/node-keypair.json
QCOIN_CLUSTER_MANIFEST_JSON=/etc/qcoin/cluster-manifest.json
QCOIN_LOG_DIR=/var/log/qcoin
EOF
```

### Enable and start

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now qcoin-node.service
systemctl --no-pager --full status qcoin-node.service
```

The Linux unit intentionally uses `ProtectSystem=full`, not `strict`. In this lab layout the stricter setting blocked writes under `/var/lib/qcoin` and caused repeated `Permission denied (os error 13)` failures during block production and sync.

### Inspect logs

```bash
journalctl -u qcoin-node.service -f
tail -f /var/log/qcoin/node.log
tail -f /var/log/qcoin/node.err
```

## macOS node: launchd LaunchAgent

Applies to:

- `macos-node`

This repo currently ships a user-level LaunchAgent example, not a system
LaunchDaemon.

That means:

- it survives process exit and normal logout/login cycles
- it starts automatically when the `jay` user logs in
- it does **not** start before login after a reboot

For the current lab, that is acceptable for the macOS node. If pre-login boot is
required later, adapt the same launcher to a system LaunchDaemon with `sudo`.

### Install user-level assets

```bash
cd /Users/jay/pudding/qcoin
mkdir -p /Users/jay/.config/qcoin /Users/jay/.qcoin /Users/jay/Library/Logs/qcoin /Users/jay/Library/LaunchAgents
cp deploy/qcoin-node.mac.env.example /Users/jay/.config/qcoin/qcoin-node.env
cp deploy/com.loadngo.qcoin-node.plist /Users/jay/Library/LaunchAgents/com.loadngo.qcoin-node.plist
```

Edit `/Users/jay/.config/qcoin/qcoin-node.env` so the listen address matches the
actual LAN address of the macOS node.

### Load and start

```bash
launchctl bootout "gui/$(id -u)" /Users/jay/Library/LaunchAgents/com.loadngo.qcoin-node.plist 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" /Users/jay/Library/LaunchAgents/com.loadngo.qcoin-node.plist
launchctl enable "gui/$(id -u)/com.loadngo.qcoin-node"
launchctl kickstart -k "gui/$(id -u)/com.loadngo.qcoin-node"
launchctl print "gui/$(id -u)/com.loadngo.qcoin-node"
```

### Inspect logs

```bash
tail -f /Users/jay/Library/Logs/qcoin/node.log
tail -f /Users/jay/Library/Logs/qcoin/node.err
```

## Verification

After the services are up, verify all three nodes:

```bash
for ip in <linux_node_a_ip> <linux_node_b_ip> <macos_node_ip>; do
  echo "== $ip =="
  curl -s "http://$ip:9700/node-info"
  echo
  curl -s "http://$ip:9700/tip"
  echo
done
```

The cluster should report:

- same `chain_id`
- same `height`
- same `tip_hash_hex`
- same `state_root_hex`

## Restart checks for the exit gate

Once service management is installed, validate:

1. restart `linux-node-a` only
2. restart `linux-node-b` only
3. restart `macos-node` only
4. cold stop and clean start all three through the service path

No restart should require:

- deleting chain state
- editing files by hand
- manual `nohup` recovery

## Transaction ingress checks

Run at least one real submission against each node:

```bash
cargo run -p qcoin-node -- submit-tx --target <linux_node_a_ip>:9700 --tx-json ./path/to/tx.json
cargo run -p qcoin-node -- submit-tx --target <linux_node_b_ip>:9700 --tx-json ./path/to/tx.json
cargo run -p qcoin-node -- submit-tx --target <macos_node_ip>:9700 --tx-json ./path/to/tx.json
```

After each submit, verify the cluster reconverges on one tip.
