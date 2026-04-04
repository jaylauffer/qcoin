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

`10.10.10.3` still needs a local keypair file at `/etc/qcoin/node-keypair.json` even when running with `QCOIN_PRODUCE=false`; the service launch script requires `QCOIN_KEYPAIR_JSON` to exist on every node.

If `10.10.10.3` is later cleaned and promoted to a validator, generate a third keypair there and update the validator set on all nodes.

## Phase 3: Install the service wrapper

On each machine:

```bash
sudo install -d -m 0755 /etc/qcoin /var/lib/qcoin /var/log/qcoin
sudo install -m 0755 deploy/qcoin-node-launch.sh /usr/local/bin/qcoin-node-launch.sh
sudo install -m 0644 deploy/qcoin-node.service /etc/systemd/system/qcoin-node.service
```

## Phase 4: Render per-node config files

Use `deploy/render-node-config.sh` to generate the local `qcoin-node.env` and `network-config.json`.

Example for `10.10.10.1` as a producer:

```bash
./deploy/render-node-config.sh \
  --self-ip 10.10.10.1 \
  --output-dir /tmp/qcoin-config-10.10.10.1 \
  --validator-public-key-hex PUBKEY_FOR_10_10_10_1 \
  --validator-public-key-hex PUBKEY_FOR_10_10_10_2 \
  --produce true
```

Example for `10.10.10.2` as a producer:

```bash
./deploy/render-node-config.sh \
  --self-ip 10.10.10.2 \
  --output-dir /tmp/qcoin-config-10.10.10.2 \
  --validator-public-key-hex PUBKEY_FOR_10_10_10_1 \
  --validator-public-key-hex PUBKEY_FOR_10_10_10_2 \
  --produce true
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

Install the rendered files on each machine:

```bash
sudo install -m 0644 /tmp/qcoin-config-10.10.10.X/qcoin-node.env /etc/qcoin/qcoin-node.env
sudo install -m 0644 /tmp/qcoin-config-10.10.10.X/network-config.json /etc/qcoin/network-config.json
```

Replace `10.10.10.X` with the local machine IP.

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
curl http://10.10.10.1:9700/tip
curl http://10.10.10.2:9700/tip
curl http://10.10.10.3:9700/tip
```

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
4. Only after the `10.10.10.3` host and network path are verified, add its public key to every node's `validator_public_key_hex` array and switch it to `QCOIN_PRODUCE=true`.

That keeps hardware requirements flat while preserving a realistic multi-node workflow.
