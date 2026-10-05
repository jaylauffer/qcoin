# QCoin Task Rewards

Purpose: how a `loadngo` Task worker is paid in QCoin, what its operator configures,
and the stages from launch-time configuration to a wallet.

The Task side is in loadngo `docs/TASK_REWARD_FLOW.md`: accepting the work never
depends on the reward, each operator chooses the reward schemes it takes part in, and
QCoin is the first-party settler, run as an external command. This document is the
QCoin side of that plan. Written 2026-10-06; nothing in it is built yet.

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

Today the payee is not configurable: `task_submitter` pays to
`blake3(worker_node_id)`, which no key controls.

## The standard payee script

An owner script hash is the hash of a script, so a payee needs one standard
single-key script that the operator's key can later spend:

1. The operator makes a keypair. `qcoin-node keygen` exists but prints the private
   key to stdout as JSON. It should write `<name>.<scheme>.key` (mode 0600) and
   `<name>.<scheme>.pub`, the layout of the existing keys in `~/.loadngo/keys`.
2. A new `qcoin-node payee --public-key <file.pub>` prints the owner script hash of
   the standard single-key script for that key.
3. The operator passes that hash to `--reward-payee`.

A script made of `Nop` alone, as the ledger tests use, is spendable by anyone and must
never be a payee.

### Prerequisite: a key-locked output cannot be spent today

Found by reading qcoin `4cb6407`; not yet confirmed by a test:

- The ledger hashes the whole witness script and requires it to equal the spent
  output's `owner_script_hash` (`qcoin-ledger/src/lib.rs`, in `apply_transaction`'s
  input loop).
- `CheckSig` takes the signature from the stack, and only `PushBytes` inside that same
  script fills the stack (`qcoin-script/src/lib.rs`, `DeterministicScriptEngine::eval`).
- So the signature is part of the hashed script. It signs a sighash that already
  commits to that hash, so it cannot be known when the output is created, and no
  `CheckSig` output can be spent through the ledger. The `CheckSig` test in
  `qcoin-script` passes because it calls the engine directly with the hash of the
  unsigned script.

The fix is for the witness to carry unlocking data (the signature) separately from the
committed script, pushed before the script runs, so the standard payee script is
`[PushBytes(public key), CheckSig]` and its hash is fixed in advance. This must be
settled before payee hashes are handed out: an output sent to a hash of the wrong
script form stays unspendable forever. Proof-only rewards (stage 1) carry no value, so
they lose nothing if this comes later, but the payee hashes should not have to change.

## Stages

### Stage 1: payee at launch, proof of accepted work

The initial implementation.

- Worker operator: keypair, `qcoin-node payee`, `task-node --reward-payee qcoin=<hash>`.
- Submitter operator: `task_submitter --reward qcoin="qcoin-node task-reward settle …"`.
- `task-reward settle` writes what the runtime writes today, a metadata-only output
  whose `metadata_hash` is the completion receipt's commitment, now owned by the
  worker's payee. No value moves; the output is the worker's on-chain proof of
  accepted work.
- `task-reward verify` lets a worker confirm a settlement reference: the transaction
  is in a block, pays its payee, and commits to its receipt.
- Needs: the standard payee script and `qcoin-node payee`; the settle and verify
  subcommands. Not needed: a wallet, or any QCoin balance.

### Stage 2: value-bearing rewards

The reward carries an amount, so the submitter must fund it.

- `TaskRequest.reward_offers[].terms` states the asset and amount, for example
  `{"asset": "<asset id hex>", "amount": 5}`. The worker sees the terms before it
  offers.
- The settler spends the submitter's own outputs: the submitter's funding key and its
  unspent outputs go to the settler (in configuration, never over the Task protocol),
  and it pays the amount to the worker's payee with change back to the submitter.
- Needs, beyond stage 1:
  - spendable key-locked outputs (the prerequisite above);
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

- **One payee per operator or per task node.** Per node separates the accounts of
  several nodes and keeps one leaked node identity from linking all of an operator's
  earnings; per operator is simpler to set up.
- **The asset for stage 2.** A submitter-issued task credit now, or wait for native
  QCOIN.
- **Payee form.** This document uses the owner script hash, which also covers later
  multi-key scripts. The alternative is the public key, with the settler deriving the
  hash; simpler for operators, but it ties payees to the single-key script.
