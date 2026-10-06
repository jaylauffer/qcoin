# QCoin Task Rewards

Purpose: how a `loadngo` Task worker is paid in QCoin, what its operator configures,
and the stages from launch-time configuration to a wallet.

The Task side is in loadngo `docs/TASK_REWARD_FLOW.md`: accepting the work never
depends on the reward, each operator chooses the reward schemes it takes part in, and
QCoin is the first-party settler, run as an external command. This document is the
QCoin side of that plan. Written 2026-10-06. Stage 1 is built (2026-10-06):
spending key-locked outputs, `qcoin-node payee`, and `qcoin-node task-reward settle`
and `verify`. Stages 2 and 3 are not.

## Receiving needs no wallet

To be paid, a worker only has to name where the payment goes. On this chain that is
an **owner script hash**: an output belongs to whoever can produce a witness script
whose hash equals the output's `owner_script_hash` and that the script engine accepts.

So the worker's task node is launched with its payee and nothing else:

```sh
task-node … --reward-payee qcoin=<owner script hash, 64 hex digits>
```

- The task node holds no private key and keeps no balance. A stolen or misbehaving
  task node cannot spend what it earned.
- The payee is opaque to loadngo; only the QCoin settler interprets it.
- Spending, balances and key storage belong to a wallet the operator runs separately
  (stage 3). The task node never needs it.

Until 2026-10-06 the payee was not configurable: `task_submitter` paid
`blake3(worker_node_id)`, which no key controls.

## The standard payee script

An owner script hash is the hash of a script, so a payee needs one standard
single-key script that the operator's key can later spend:

1. The operator makes a keypair, one per task node (Jay, 2026-10-06):
   `qcoin-node keygen > worker-1.qcoin-key.json`. `keygen` prints the private key
   to stdout as JSON, so keep that file private (mode 0600). It should write
   `<name>.<scheme>.key` and `<name>.<scheme>.pub` itself, the layout of the
   existing keys in `~/.loadngo/keys`; not done yet.
2. `qcoin-node payee --keypair-json worker-1.qcoin-key.json` prints the owner script
   hash of the standard single-key script for that key (only the public half is
   read; `--public-key-hex` with `--scheme` also works). The script is
   `qcoin_ledger::single_key_script`, `[PushBytes(public key), CheckSig]`; the
   ledger's spend test uses the same function, so a payee printed here is one that
   key can spend.
3. The operator passes that hash to `--reward-payee qcoin=<hash>`.

A script made of `Nop` alone, as the ledger tests use, is spendable by anyone and must
never be a payee.

### Spending a key-locked output (fixed 2026-10-06)

Until 2026-10-06 (qcoin `efd8b79` and earlier) no `CheckSig` output could be spent
through the ledger. The ledger requires the witness script to hash to the output's
`owner_script_hash`, and `CheckSig` could only take its signature from a `PushBytes`
in that same script. The signature signs a sighash that commits to that hash, so it
cannot be in the script. A test against the old code confirmed it: the lock script
alone failed with `ScriptFailed` (stack underflow), and the script with the signature
inside failed with `ScriptHashMismatch`.

How it works now:

- A ledger witness carries `unlock`, a list of byte strings, beside `script` and
  `metadata`. The engine pushes them, in order, before the script runs. They are not
  part of the script hash. Each counts against the push size limit, the stack limit
  and gas, like a `PushBytes`.
- Checks pop what the locking script pushed before what the spender supplied:
  `CheckSig` pops the public key, then the signature; `CheckMultiSig` pops the keys,
  then the signatures; `CheckHashLock` pops the expected hash, then the preimage.
- The standard single-key payee script is `[PushBytes(public key), CheckSig]`, with
  the signature as its only unlocking item. Its hash is known before anything is
  signed.
- A witness written before `unlock` existed no longer decodes. Jay: no chain with
  such witnesses is live (the only one found is a `Nop` spend on agnes's April chain
  in `~/.qcoin`; the `chain1` data on agnes and dolores has no spends).

Tests: `key_locked_output_is_spent_with_signature_as_unlocking_data` and
`key_locked_output_rejects_missing_or_foreign_signatures` in `qcoin-ledger`;
`checks_signature_successfully` and `bounds_unlock_data_like_pushes` in
`qcoin-script`.

`CheckMultiSig` was not a real threshold until 2026-10-06: it paired the n-th
signature with the n-th key popped, so a 2-of-3 lock accepted only two particular
keys. Now any `threshold` of the `total` keys can sign: the spender supplies the
signatures in the order of their keys, and each key signs at most once
(`multisig_accepts_any_threshold_of_keys_with_signatures_in_key_order` in
`qcoin-script`). Gas is charged per signature check made rather than per
signature.

## Stages

### Stage 1: payee at launch, proof of accepted work

Built 2026-10-06.

```sh
# worker operator, once per task node
qcoin-node keygen > worker-1.qcoin-key.json
qcoin-node payee --keypair-json worker-1.qcoin-key.json      # prints <hash>
task-node … --reward-payee qcoin=<hash> \
  --reward-verify 'qcoin=qcoin-node task-reward verify --target <node> --payee <hash>'

# submitter operator
task_submitter … --reward 'qcoin=qcoin-node task-reward settle --target <node>'
```

- `task-reward settle` reads the settle request on stdin and submits one output to
  the payee carrying no assets, whose `metadata_hash` is the completion receipt's
  commitment. No value moves; the output is the worker's on-chain proof of accepted
  work. It then looks for the transaction in each new block, once a second on
  proactor timers, until the request's `wait_seconds` (30 by default): `settled`
  with `qcoin:tx:<id>@height:<n>`, or `pending` with `qcoin:tx:<id>`. An unreachable
  node, an unusable payee or a refused transaction gives `failed`.
- `task-reward verify` reads a settlement and exits 0 when its transaction is in a
  block and, with `--payee`, pays that payee; it prints the settlement as it now
  stands, so a `pending` one comes back `settled` once included. It does not check
  the commitment against the worker's own copy of the receipt.
- Not needed: a wallet, or any QCoin balance.
- Checked: unit tests against a fake chain (settled with height and payee, pending
  past the deadline, failures, verify before and after inclusion); and on a Mac with
  a local `qcoin-node run`, loadngo `task-node` and `task_submitter`: settled in
  block 1, the worker's verifier confirming it pays its payee, and with the node
  stopped, the work accepted with the reward `failed`.

### Stage 2: value-bearing rewards

The reward carries an amount, so the submitter must fund it.

- `TaskRequest.reward_offers[].terms` states the asset and amount, for example
  `{"asset": "<asset id hex>", "amount": 5}`. The worker sees the terms before it
  offers.
- The settler spends the submitter's own outputs: the submitter's funding key and its
  unspent outputs go to the settler (in configuration, never over the Task protocol),
  and it pays the amount to the worker's payee with change back to the submitter.
- Needs, beyond stage 1:
  - a query for the unspent outputs owned by a script hash; `qcoin-node` has `run`,
    `submit-tx`, `node-info`, `tip`, `block`, `keygen` and `chain-state`, and none of
    them answers that;
  - an asset to pay in. Native QCOIN supply is not implemented
    ([MONETARY_POLICY.md](MONETARY_POLICY.md)), and an asset made with
    `CreateAsset` is not QCOIN. Either the submitter issues a generic task-credit
    asset, or value-bearing rewards wait for native QCOIN.

### Stage 3: a wallet

A qcoin tool the operator runs, apart from any task node:

- holds the operator's keys, in files as above;
- derives payees for them (one per operator, or one per task node);
- tracks the unspent outputs its payees own, shows balances per asset, lists earned
  task receipts;
- builds, signs and submits spends;
- funds the submitter's settler in stage 2, in place of a raw key and output list.

It lives in qcoin, which depends on loadngo (`loadngo-pq-crypto`, `loadngo-proactor`),
never the reverse. A task node still takes only `--reward-payee`.

## Open

- **The asset for stage 2.** A submitter-issued task credit now, or wait for native
  QCOIN.

## Decided (Jay, 2026-10-06)

- **Payee form: the owner script hash.** Not the public key with the settler
  deriving the hash: a script hash also covers multi-key (`CheckMultiSig` is a real
  m-of-n since qcoin `4320770`) and other scripts without changing the flag.
- **One payee per task node.** `qcoin-node payee`, the docs and examples give each
  task node its own key and payee. That keeps one leaked node identity from linking
  all of an operator's earnings. Nothing enforces it: an operator who wants one
  balance can pass the same payee to several nodes.
