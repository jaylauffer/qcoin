# QCoin Exit Gate

Purpose: define the exact point at which the lab should stop prioritizing qcoin
cluster bring-up work and begin prioritizing EAB implementation on top of that
cluster.

This is an operational gate for the current three-device lab, not a statement
that qcoin is feature-complete.

## Current lab status

Status: `passed` on `2026-04-16`

Validated on the real service-managed lab nodes:

- `linux-node-a` via `systemd`
- `linux-node-b` via `systemd`
- `macos-node` via `launchd`

Verification snapshot at the close of the gate check:

- all three nodes reported `height: 4`
- all three nodes reported `tip_hash_hex: 7d2ce57f5d73ef15c2ebc5b9c70e20207d7b20e239ffdd52e58e8f1238199973`
- all three nodes reported `state_root_hex: 0a0c974fceadd63bb761f58459ee21342455d99b0cb5e6d3117f92d91630c2a7`
- transaction submission was exercised through all three nodes individually
- stderr line counts stayed flat for a full `45s` idle window after the runtime/service fixes

This status is specific to the current lab bootstrap cluster and should be
re-validated if service layout, manifest contents, storage paths, or node
runtime behavior changes materially.

## The decision rule

Shift primary focus from qcoin core bring-up to EAB work when qcoin is
stable enough to behave like boring proof infrastructure on the real lab
hardware.

For the current lab, that means the three-node cluster passes all checks below
on the actual devices.

## Lab nodes

Current expected lab nodes:

- `linux-node-a`
- `linux-node-b`
- `macos-node`

All checks below refer to that real three-node cluster, not a single-machine
simulation.

## Required checks

### 1. Zero-config bootstrap works

All three nodes must:

- start from the shared manifest
- discover each other automatically over multicast
- do so without requiring a manual static peer list

If a machine requires ad hoc peer wiring to join the cluster, the exit gate is
not met.

### 2. Cluster identity is coherent

All three nodes must report:

- the same `chain_id`
- the same validator set / manifest
- compatible wire version and node-info exchange

This is the minimum bar for saying the cluster is one coherent network.

### 3. Tip convergence is stable

After startup and after real transaction submission, all three nodes must agree
on:

- `height`
- `tip_hash_hex`
- `state_root_hex`

This must hold after repeated submissions, not just once.

### 4. Real transaction ingress works from any node

Submitting real transactions to any one of the three nodes must:

- be accepted through the current qcoin transaction submission path
- result in block production by the correct validator
- cause the whole cluster to reconverge on one tip

If submission works only against one preferred node, the exit gate is not met.

### 5. Restart behavior is boring

The cluster must survive:

- single-node restart of each node individually
- a cold restart of all three nodes

without:

- manual file surgery
- deleting chain state by hand
- ad hoc repair steps outside the documented startup path

Repairable startup behavior is acceptable. Operator babysitting is not.

### 6. Idle behavior is quiet

When no work is being submitted, the cluster should stay quiet enough that
operators can trust the logs.

That means no persistent recurrence of:

- `No route to host`
- repeated peer node-info/presence churn
- endless “peer has not completed node-info exchange” loops
- useless empty-block churn

The cluster does not need to be silent. It does need to be boring.

## Explicit non-requirements for this gate

The qcoin exit gate does **not** require:

- native QCOIN monetary policy completion
- fee markets
- staking economics
- open-network hardening
- full fork resolution
- missed-slot/view-change handling

Those matter later, but they are not required before EAB can begin using qcoin
as a proof layer in this controlled lab.

## Explicitly accepted bootstrap limitations

Crossing the exit gate does **not** mean qcoin is production-complete.

For this lab, the following limitations are explicitly accepted:

- current qcoin still uses deterministic proposer scheduling plus append-only
  tip extension
- there is no full fork choice/reorg handling yet
- there is no missed-slot/view-change logic yet
- validator liveness is still important for steady progress

The gate is about “stable enough for EAB anchoring PoC,” not “finished chain.”

## What happens after the gate passes

Once the gate passes, the next primary work is:

1. keep qcoin nodes running as stable lab infrastructure
2. move EAB onto the qcoin anchor transaction contract
3. add EAB qcoin outbox/retry behavior
4. begin migrating EAB background/runtime ownership toward `loadngo-proactor`

At that point, qcoin work continues, but it is no longer the main blocker for
the EAB proof-of-concept.

## Short version

The handoff point is:

**all three qcoin nodes start cleanly, discover each other automatically,
accept real transactions, converge on the same tip, and restart without drama.**
