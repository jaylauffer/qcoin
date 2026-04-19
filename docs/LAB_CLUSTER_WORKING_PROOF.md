# Lab Cluster Working Proof

Purpose: document what the qcoin lab has actually proven, what "working proof"
means right now, and where the proof lifecycle still stops short of a complete
reward-anchor story.

This note is grounded in the current lab-facing docs and code under:

- `README.md`
- `docs/QCOIN_EXIT_GATE.md`
- `docs/LAB_SERVICE_BOOTSTRAP.md`
- `docs/EAB_ANCHOR_TRANSACTION_MODEL.md`
- `../entitlement-achievement-blockchain/rust/src/qcoin_ledger_storage.rs`

## Short version

The current lab has proven a real and useful thing:

- a three-node qcoin validator cluster can discover peers, accept transaction
  ingress from any node, converge on one tip, and expose durable block history

For the EAB proof-of-concept, the working proof object is:

- a deterministic qcoin transaction id
- plus visibility of that exact transaction in qcoin block history

It is **not** yet:

- native QCOIN monetary issuance
- a finalized external Merkle proof artifact
- a complete end-to-end authoritative reward-anchor lifecycle for every reward
  mutation

So the proof layer is working, but the higher-order reward lifecycle above it is
still incomplete.

## What the lab cluster has already proven

The qcoin exit gate was recorded as `passed` on `2026-04-16`.

Validated lab nodes:

- `linux-node-a`
- `linux-node-b`
- `macos-node`

Recorded convergence snapshot:

- `height: 4`
- `tip_hash_hex: 7d2ce57f5d73ef15c2ebc5b9c70e20207d7b20e239ffdd52e58e8f1238199973`
- `state_root_hex: 0a0c974fceadd63bb761f58459ee21342455d99b0cb5e6d3117f92d91630c2a7`

Operationally, this means the cluster has already demonstrated:

- multicast discovery/bootstrap
- coherent cluster identity
- stable tip convergence
- real transaction ingress through any node
- boring restart behavior through service management

That is the substrate proof.

## What "working proof" means in the current PoC

For the current EAB-facing proof-of-concept, qcoin is not the player-facing
authority.

Instead:

- EAB issues the authoritative local record
- qcoin provides durable ordering and inclusion proof for a deterministic hash
  of that record

So the working proof is:

1. EAB derives a deterministic anchor transaction
2. qcoin accepts and orders that transaction
3. the exact transaction becomes visible in cluster block history

That is enough to say:

- "this authoritative EAB record was anchored into the ordered qcoin ledger"

It is not yet enough to say:

- "we have a final polished proof artifact suitable for external wallets or a
  generalized receipt standard"

## The exact proof object in the current code

Today the proof object is a metadata-only qcoin transaction.

Current construction in EAB:

- transaction kind: `Transfer`
- inputs: none
- outputs: one output
- assets: empty
- `owner_script_hash`: `blake3(player_id)`
- `metadata_hash`: `blake3(canonical local EAB block JSON)`

Important:

- this is not a native QCOIN mint
- this is not asset issuance
- this is a metadata anchor used as proof-of-ordering substrate

So when we say "proof minted in qcoin" in the current lab context, the accurate
meaning is:

- a deterministic anchor transaction was admitted to the qcoin ledger and made
  visible in block history

## How the current anchor path works

For `LEDGER_BACKEND=qcoin`, EAB currently does:

1. append the authoritative local EAB block to the per-player log
2. derive the qcoin anchor transaction from that block
3. persist the anchor into `qcoin_anchor_outbox.json`
4. have a `loadngo-proactor` worker submit the transaction to qcoin over the
   UDP qcoin wire
5. use qcoin HTTP block-history inspection to determine whether the exact
   transaction became included

Submission transport:

- UDP qcoin wire
- frame magic: `QCN1`
- message: `SubmitTransaction`

Inclusion check:

- `qcoin-node tip --target <addr>`
- then reverse scan `qcoin-node block --target <addr> --height <n>`
- look for exact `tx_id`

Current proof material therefore consists of:

- `tx_id`
- `metadata_hash`
- block height where found
- block visibility on the cluster

## What the lab has proven beyond the substrate

The lab has already proven:

- real EAB activity can produce real qcoin anchor submissions
- outbox persistence survives restart
- at least one live anchor can reach qcoin inclusion strongly enough for the
  outbox to drain

That means the proof path is not hypothetical.

There is already a real bridge from:

- authoritative local EAB record

to:

- qcoin-visible anchor transaction

## What is still not fully proven

The missing piece is not the basic cluster.

The missing piece is the precise lifecycle from:

- qcoin acceptance

to:

- durable inclusion of the exact authoritative reward anchor

The current EAB-side discovery note already shows the gap:

- some anchors are accepted by qcoin
- some anchors become visible in qcoin history
- but later authoritative reward anchors are not yet tracked all the way
  through durable inclusion as a first-class lifecycle state

So the current lab proof is working, but not yet complete for every reward
mutation.

## Why the distinction matters

These states are different:

- `local authoritative success`
- `qcoin acceptance`
- `qcoin inclusion`

Current proof quality is only honest if we keep those states separate.

If we collapse them together, we overstate what the lab has proven.

## What the current proof is *not*

The current proof is not:

- native QCOIN reserve-asset issuance
- generic asset minting as product truth
- a Merkle branch receipt returned to external clients
- a proof that every reward anchor now reaches inclusion correctly
- a proof that qcoin itself should own gameplay semantics

The current proof is narrower and more useful:

- qcoin is stable enough to act as a durable ordered proof layer behind EAB on
  the three-node lab cluster

## What should be called "working" right now

The following statement is accurate:

`The qcoin lab cluster is working proof infrastructure for the EAB PoC.`

The following statement is too broad:

`The full qcoin-backed reward proof lifecycle is complete.`

That broader claim is still blocked on explicit acceptance-vs-inclusion
tracking for real authoritative reward anchors.

## What should be anchored later for loadngo task work

If the lab later wants qcoin-backed proof for `loadngo` task assignment and
completion, the object to anchor should be:

- a deterministic task completion receipt hash

not:

- raw task payloads
- raw model output
- transient queue state

That keeps qcoin acting as proof substrate rather than becoming the assignment
bus itself.

## Recommended operator language

When discussing the current lab state, use language like:

- `qcoin cluster proof substrate`
- `anchor accepted`
- `anchor included`
- `durable block-history visibility`

Avoid language like:

- `full reward proof complete`
- `native QCOIN mint proven`
- `final inclusion proof artifact shipped`

unless those stronger claims are actually implemented and validated later.

## Bottom line

What the lab has working today:

- a stable three-node qcoin cluster
- real transaction ingress through any node
- real metadata-anchor inclusion in qcoin history

What still remains:

- explicit, durable lifecycle tracking for every authoritative reward anchor
- a stronger proof artifact than "visible in block history"

That is the honest state of the proof minted on the lab cluster.
