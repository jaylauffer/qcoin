# QCoin

QCoin is a post-quantum-secure reserve asset and native multi-asset platform focused on game tokens and digital items. Learn more in the original design discussion: https://chatgpt.com/share/69214c32-7a8c-800e-a153-0669d7fc3101.

Follow-up: https://chatgpt.com/share/69248807-3d10-800e-a3d9-22ad6e7aa8a5

## Crate layout

- **qcoin-crypto** – Post-quantum signature abstractions.
- **qcoin-types** – Core types for hashes, transactions, blocks, and assets.
- **qcoin-script** – Minimal scripting language and engine traits.
- **qcoin-ledger** – In-memory UTXO set and ledger rules.
- **qcoin-consensus** – Consensus traits with a dummy PoS-like engine.
- **qcoin-node** – CLI/node that wires everything together.

## Build and run

```bash
cargo build
cargo run -p qcoin-node -- run
cargo run -p qcoin-node -- keygen
```

Runtime artifacts are written under `data/` by default (`data/qcoin-chain-state.json` and matching `*.blocks.json`), and are git-ignored.

## Node communication (HTTP + pull sync)

`qcoin-node` now supports static peers and block synchronization over HTTP:

- `GET /tip` -> current tip metadata (`height`, `tip_hash_hex`, `state_root_hex`)
- `GET /blocks/{height}` -> binary (`bincode`) encoded block for 1-based height
- `POST /blocks` -> submit binary (`bincode`) encoded block

### Quick 2-node local test

1. Generate validator keypair:

```bash
cargo run -p qcoin-node -- keygen > /tmp/qcoin_validator.json
PUB=$(grep -o '"public_key_hex\": \"[^\"]*\"' /tmp/qcoin_validator.json | cut -d'\"' -f4)
```

2. Start node A (producer + API):

```bash
cargo run -p qcoin-node -- run \
  --listen 127.0.0.1:9710 \
  --interval-seconds 1 \
  --keypair-json /tmp/qcoin_validator.json \
  --validator-public-key-hex "$PUB"
```

3. Sync node B from node A:

```bash
cargo run -p qcoin-node -- run \
  --once \
  --produce=false \
  --peer http://127.0.0.1:9710 \
  --validator-public-key-hex "$PUB" \
  --state-path data/qcoin_b_state.json \
  --blocks-path data/qcoin_b_blocks.json
```

### Run flags (selected)

- `--peer <url>` repeatable static peer list (example: `http://127.0.0.1:9710`)
- `--listen <addr>` HTTP bind address
- `--sync-interval-seconds <n>` periodic pull-sync interval
- `--produce=<true|false>` whether this node proposes local empty blocks
- `--network-config-json <path>` JSON file containing peer URLs and validator public keys
- `--validator-public-key-hex <hex>` validator set entries for signature/proposer checks
- `--keypair-json <path>` signer keypair file from `keygen`
- `--blocks-path <path>` explicit block history persistence path

## Manual systemd deployment

The repo contains deploy artifacts for running `qcoin-node` as a boot-time service:

- `deploy/qcoin-node.service`
- `deploy/qcoin-node-launch.sh`
- `deploy/qcoin-node.env.example`
- `deploy/network-config.10.10.10.1.example.json`

The recommended layout on a machine is:

- `/etc/systemd/system/qcoin-node.service`
- `/usr/local/bin/qcoin-node-launch.sh`
- `/etc/qcoin/qcoin-node.env`
- `/etc/qcoin/network-config.json`
- `/etc/qcoin/node-keypair.json`
- `/var/lib/qcoin/` for chain state and blocks
- `/var/log/qcoin/` for service logs

### 1. Build the release binary

```bash
cargo build --release -p qcoin-node
```

Expected binary path:

```bash
/home/jay/pudding/qcoin/target/release/qcoin-node
```

### 2. Install the service and launcher

```bash
sudo install -d -m 0755 /etc/qcoin /var/lib/qcoin /var/log/qcoin
sudo install -m 0755 deploy/qcoin-node-launch.sh /usr/local/bin/qcoin-node-launch.sh
sudo install -m 0644 deploy/qcoin-node.service /etc/systemd/system/qcoin-node.service
```

### 3. Create the node keypair JSON

Generate one keypair per machine:

```bash
./target/release/qcoin-node keygen > /tmp/node-keypair.json
sudo install -m 0600 /tmp/node-keypair.json /etc/qcoin/node-keypair.json
```

Capture the `public_key_hex` from each machine. All nodes must use the same validator set.

### 4. Create `/etc/qcoin/network-config.json`

This file is the source of truth for peers and validator public keys.

Example for machine `10.10.10.1`:

```json
{
  "peers": [
    "http://10.10.10.2:9700",
    "http://10.10.10.3:9700"
  ],
  "validator_public_key_hex": [
    "PUBKEY_FOR_10_10_10_1",
    "PUBKEY_FOR_10_10_10_2",
    "PUBKEY_FOR_10_10_10_3"
  ]
}
```

Install it:

```bash
sudo install -m 0644 deploy/network-config.10.10.10.1.example.json /etc/qcoin/network-config.json
```

Then edit `/etc/qcoin/network-config.json` and replace the placeholder validator keys with the real `public_key_hex` values.

### 5. Create `/etc/qcoin/qcoin-node.env`

Example for machine `10.10.10.1`:

```bash
QCOIN_BINARY=/home/jay/pudding/qcoin/target/release/qcoin-node
QCOIN_WORKDIR=/home/jay/pudding/qcoin
QCOIN_STATE_PATH=/var/lib/qcoin/qcoin-chain-state.json
QCOIN_BLOCKS_PATH=/var/lib/qcoin/qcoin-blocks.json
QCOIN_LISTEN=10.10.10.1:9700
QCOIN_INTERVAL_SECONDS=5
QCOIN_SYNC_INTERVAL_SECONDS=3
QCOIN_PRODUCE=true
QCOIN_SCHEME=dilithium2
QCOIN_KEYPAIR_JSON=/etc/qcoin/node-keypair.json
QCOIN_NETWORK_CONFIG_JSON=/etc/qcoin/network-config.json
```

You can start from the template:

```bash
sudo install -m 0644 deploy/qcoin-node.env.example /etc/qcoin/qcoin-node.env
```

Then edit `/etc/qcoin/qcoin-node.env` for the local machine.

### 6. Reload and start the service

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now qcoin-node.service
```

### 7. Inspect logs and status

```bash
systemctl --no-pager --full status qcoin-node.service
journalctl -u qcoin-node.service -f
tail -f /var/log/qcoin/node.log
tail -f /var/log/qcoin/node.err
```

### 8. Multi-node requirements

- Each machine needs its own `/etc/qcoin/node-keypair.json`.
- Each machine should have a machine-specific `/etc/qcoin/qcoin-node.env` with its own `QCOIN_LISTEN`.
- Each machine should have a machine-specific `/etc/qcoin/network-config.json` with its peers listed appropriately.
- The `validator_public_key_hex` array must be identical across all three machines.
- Peer URLs should point at reachable private addresses such as `http://10.10.10.x:9700`.

## Roadmap

- Replace dummy crypto with real post-quantum schemes.
- Implement a production-grade script engine.
- Ship full PoS consensus with validator sets and signatures.
- Add an L2 game rollup prototype for high-throughput in-game actions.

---

Let’s zoom out **to first principles** and talk about the *actual operational economics* of running a decentralized blockchain — not fantasy, not hype, but what *must* be true in the physical and economic world for a chain like **qcoin** to remain alive, secure, and valuable.

This is the question almost no blockchain project honestly asks:

> **How much liquidity (capital + node participation) does a new chain realistically require just to *exist*?**

Below is the real model.

---

# 1. A blockchain is a *market for block-space*

And markets need:

1. **Producers** → validators / block producers
2. **Consumers** → users who submit txs
3. **Money flow** → transaction fees or inflation
4. **An asset whose value anchors the whole thing** → QCOIN

If one of these is missing, the system dies.

---

# 2. Minimum viable liquidity = cost to attack < cost to defend

A chain is only “alive” if:

> **It costs more to attack than the expected benefit of attacking.**

This is the absolute economic backbone.

---

## For a PoS chain like qcoin:

Security depends on:

1. **Total stake securing the chain**
2. **Liquidity available to slash attackers**
3. **Value of the native asset**

The attacker must obtain > 33% (or > 66%) of the stake.
So chain security is directly proportional to:

### **Stake_value = Total_QCOIN_staked × QCOIN_price**

If attackers can cheaply acquire QCOIN, the chain is insecure.

Thus the real question becomes:

# 3. How much QCOIN value is required to repel realistic attacks?

Let’s do the math honestly.

---

# 4. Threat model: who will attack a new chain?

A new chain has three attack classes:

### **A. Low-level vandals / script kiddies**

Goal: disrupt.
Resources: small VPS farms.
Counter: spam fees + basic rate limits.

→ negligible capital requirement.

---

### **B. Opportunistic arbitrage attackers**

Goal: double-spend, reorg games, oracle manipulation.
Resources: $10k–$500k capital.
Counter: staking requirements + finality.

→ requires at least **$10M–$50M of staked value** to make attacks non-profitable.

---

### **C. Strategic adversaries (nation-states, serious funds)**

Goal: disruption or advantage.
Resources: $100M–$10B.

Countering this requires the chain to become extremely expensive to buy or corrupt.

→ requires **> $500M of staked value** for serious, sovereign-grade resistance.

---

# 5. Minimum Viable Liquidity Estimates

Here’s the real breakdown for qcoin’s goals:

## QCoin Goal #1: **Reserve Asset**

A reserve asset *must not be cheap to attack*.

If an attacker can buy 34% of the stake for $1M → qcoin is dead.

**Minimum stake to be believable as a “reserve asset”:**
👉 **$50M–$100M staked QCOIN**

Below this, the chain cannot credibly claim reserve currency properties.
It may still function as a toy/testnet/game chain — but not a monetary one.

---

## QCoin Goal #2: **Token Platform for Games**

Game chains have more relaxed security needs, because:

* double spends mostly hurt the game economy, not the external world
* games can turn off or pause withdrawals
* central studios can act as validators

**Minimum stake for game-friendly operation:**
👉 **$5M–$20M staked QCOIN**

This is basically enough to prevent opportunistic attacks but not enough to stop nation-state attacks.
For game ecosystems, this is fine.

---

### Combined Requirement for qcoin’s dual goals:

If qcoin wants both:

* Reserve-style trust
* Game-token settlement

The more demanding requirement wins.

Therefore:

# **qcoin needs ~$50M–$100M of staked value to be “viable” as a secure, decentralized, independent chain.**

Below that, it is a **semi-centralized experimental chain** (which may be acceptable early on).

---

# 6. Does this mean qcoin is impossible? No — because staking economics can bootstrap.

You don’t need $50M on day one.

You need:

1. **A validator set** with maybe 10–50 known participants
2. **Bootstrapped stake** (founders, treasury, early adopters)
3. **A reason to run nodes** (economic purpose)

Chains like Cosmos, Polkadot, Solana, Near started with:

* small validator sets
* a treasury holding a huge chunk of stake
* limited attack surface early on

So qcoin can realistically start with:

* **$1M–$3M staked**
* central control over validator rotation
* staking and slashing enabled in code but not yet fully decentralized

Then:

* games integrate
* token activity increases
* external liquidity enters
* value of QCOIN rises
* stake grows
* decentralization increases

This is *exactly* how every successful chain has launched.

---

# 7. Summary

### If qcoin is:

* **just a game chain** → $5M–$20M staked QCOIN needed
* **a reserve-grade asset** → $50M–$100M staked QCOIN needed
* **both** → use the higher number

### But early-phase bootstrapping only needs:

* **$1M–$3M stake**
* **small trusted validator set**
* **economic reason for validators to exist** (games, tokens, fees, treasury rewards)

---

# 8. The real hard part is NOT liquidity — it’s utility

If qcoin:

* solves game tokenization
* offers built-in PQ safety
* enables cross-IP/game-ecosystems assets
* gives studios instant token issuance
* has clean L1 semantics

Then **liquidity comes.**

But without real utility, no amount of money will secure a chain.

---

# 9. If you want, we can now model:

* Validator economics (inflation → yield → stake growth)
* Attack cost curves
* Liquidity ramps (seed → early adoption → public markets)
* Monetary policy for QCOIN
* Node incentives (fees vs. inflation)
* Treasury design & vesting
* How many validators are safe at each liquidity tier

Just tell me where you want to dive next.
