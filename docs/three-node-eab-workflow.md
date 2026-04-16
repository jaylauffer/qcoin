# Three-Node Bring-Up and EAB Workflow

This guide covers a three-node qcoin lab using `10.10.10.1`, `10.10.10.2`, and `10.10.10.3`.

## Security first

If `10.10.10.3` is operating behind an untrusted tether or otherwise unverified upstream network path, do not promote it to a validator yet.

- Run `10.10.10.1` and `10.10.10.2` as producers.
- Run `10.10.10.3` as a non-producing observer with `QCOIN_PRODUCE=false`.
- Do not store irreplaceable signing material on `10.10.10.3` until both the host and the upstream path are verified.

That gives you a usable development network without assigning validator responsibility to the least-trusted node.

## Phase 1: Build once

From the qcoin repo root on each machine:

```bash
cargo build --release -p qcoin-node
```

## Phase 2: Generate/install node keypairs

Generate one keypair on each machine (`10.10.10.1`, `10.10.10.2`, and `10.10.10.3`):

```bash
./target/release/qcoin-node keygen > /tmp/node-keypair.json
```

Install each machine's keypair:

```bash
sudo install -d -m 0755 /etc/qcoin
sudo install -m 0600 /tmp/node-keypair.json /etc/qcoin/node-keypair.json
```

Extract and save the `public_key_hex` from `10.10.10.1` and `10.10.10.2`. Those two public keys are the initial validator set.

`10.10.10.3` should still have a stable local key at `/etc/qcoin/node-keypair.json`. If the file is missing, `qcoin-node` now generates and persists one automatically on first start.

If `10.10.10.3` is later cleaned and promoted to a validator, generate a third keypair there and update the validator set on all nodes.

## Phase 3: Install the service wrapper

On each machine:

```bash
sudo install -d -m 0755 /etc/qcoin /var/lib/qcoin /var/log/qcoin
sudo install -m 0755 deploy/qcoin-node-launch.sh /usr/local/bin/qcoin-node-launch.sh
sudo install -m 0644 deploy/qcoin-node.service /etc/systemd/system/qcoin-node.service
```

## Phase 4: Render per-node config files

Use `deploy/render-node-config.sh` to generate the local `qcoin-node.env`, a shared `cluster-manifest.json`, and an optional `network-config.json` when you want static peers in addition to multicast discovery.

Example for `10.10.10.1` as a producer:

```bash
./deploy/render-node-config.sh \
  --self-ip 10.10.10.1 \
  --output-dir /tmp/qcoin-config-10.10.10.1 \
  --validator-public-key-hex PUBKEY_FOR_10_10_10_1 \
  --validator-public-key-hex PUBKEY_FOR_10_10_10_2 \
  --reliable-node-public-key-hex PUBKEY_FOR_10_10_10_1 \
  --reliable-node-public-key-hex PUBKEY_FOR_10_10_10_2
```

Example for `10.10.10.2` as a producer:

```bash
./deploy/render-node-config.sh \
  --self-ip 10.10.10.2 \
  --output-dir /tmp/qcoin-config-10.10.10.2 \
  --validator-public-key-hex PUBKEY_FOR_10_10_10_1 \
  --validator-public-key-hex PUBKEY_FOR_10_10_10_2 \
  --reliable-node-public-key-hex PUBKEY_FOR_10_10_10_1 \
  --reliable-node-public-key-hex PUBKEY_FOR_10_10_10_2
```

Example for `10.10.10.3` as an observer:

```bash
./deploy/render-node-config.sh \
  --self-ip 10.10.10.3 \
  --output-dir /tmp/qcoin-config-10.10.10.3 \
  --validator-public-key-hex PUBKEY_FOR_10_10_10_1 \
  --validator-public-key-hex PUBKEY_FOR_10_10_10_2 \
  --produce false
```

The rendered manifest uses the default multicast group `ff02::5143:6f69:6e`. Leave the interface unset to let `qcoin-node` prefer the interface that owns `--listen`; it only falls back to broader Unix multicast-interface discovery when it cannot map the bind address cleanly. In the current node model, multicast only handles discovery. Once peers respond, block sync and propagation continue over unicast UDP.

If you must pin interfaces explicitly, do it per node:
- interface indexes are machine-local
- put `multicast_v6` in each node's local `network-config.json`
- do not assume one shared `cluster-manifest.json` interface index will be valid on every machine

The node now stays idle when there is no submitted work. If you want an old-style idle heartbeat for a one-off smoke test, set `QCOIN_PRODUCE_EMPTY_BLOCKS=true`, but leave it unset for normal development.

The first two nodes do not need `QCOIN_PRODUCE=true` in the env file. Because their keys are in the manifest validator set, they auto-produce by default. `10.10.10.3` is forced to observer mode with `--produce false`.

Install the rendered files on each machine:

```bash
sudo install -m 0644 /tmp/qcoin-config-10.10.10.X/qcoin-node.env /etc/qcoin/qcoin-node.env
sudo install -m 0644 /tmp/qcoin-config-10.10.10.X/cluster-manifest.json /etc/qcoin/cluster-manifest.json
```

Replace `10.10.10.X` with the local machine IP.

If you rendered a static peer file, install it too:

```bash
sudo install -m 0644 /tmp/qcoin-config-10.10.10.X/network-config.json /etc/qcoin/network-config.json
```

## Phase 5: Start qcoin

On each machine:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now qcoin-node.service
systemctl --no-pager --full status qcoin-node.service
```

Useful checks:

```bash
journalctl -u qcoin-node.service -f
curl http://10.10.10.1:9700/node-info
curl http://10.10.10.1:9700/tip
curl http://10.10.10.2:9700/tip
curl http://10.10.10.3:9700/tip
```

For transaction ingress, use the UDP qcoin wire instead of HTTP:

```bash
cargo run -p qcoin-node -- submit-tx \
  --target 10.10.10.1:9700 \
  --tx-json ./path/to/transaction.json
```

Before moving on to EAB-first work, evaluate the cluster against
[QCOIN_EXIT_GATE.md](QCOIN_EXIT_GATE.md).

## Phase 6: Bring up EAB against qcoin

Run the EAB service on the most trusted machine first. In this lab, `10.10.10.1` is the simplest anchor node.

```bash
cd ../entitlement-achievement-blockchain
LEDGER_BACKEND=qcoin \
LEDGER_TOPICS_PATH=player_logs \
QCOIN_STATE_PATH=qcoin_chain_state.json \
QCOIN_NODE_URL=http://10.10.10.1:9700 \
BIND_IP=10.10.10.1 \
BIND_PORT=8080 \
cargo run --manifest-path rust/Cargo.toml
```

Important detail:

- EAB only needs one reachable `QCOIN_NODE_URL`.
- QCoin peer sync handles propagation to the rest of the cluster.
- For development, point EAB at a producer node, not the observer.

## Phase 7: Development workflow

Use this sequence when you want to keep the cluster minimal:

1. Keep qcoin as the minimal cluster: two producers and one observer.
2. Develop EAB features against `10.10.10.1:8080`.
3. Verify mirrored qcoin progress via `GET /tip` on `10.10.10.1:9700`.
4. Only after the `10.10.10.3` host and network path are verified, add its public key to the shared `cluster-manifest.json` validator list on every node and remove the explicit `QCOIN_PRODUCE=false` override there.

That keeps hardware requirements flat while preserving a realistic multi-node workflow.
