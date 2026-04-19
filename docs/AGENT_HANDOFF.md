# QCoin Agent Handoff

Purpose: provide future agents with repo-specific operating guidance so changes remain consistent, safe, and reviewable.

## What this repo is

QCoin is currently a Rust workspace with these main crates:
- `qcoin-crypto`
- `qcoin-types`
- `qcoin-script`
- `qcoin-ledger`
- `qcoin-consensus`
- `qcoin-node`

The current implementation is still early-stage infrastructure. Treat it as a correctness-first system, not an optimization-first system.

## Current engineering priorities

Read these first, in this order:
1. `docs/TODO_AGENT_REVIEW.md`
2. `docs/WORK_QUEUE.md`
3. this file

Near-term priority is:
- integrity of persisted state
- predictable startup/recovery behavior
- explicit distributed behavior under peer divergence
- explicit native QCOIN monetary policy and issuance boundaries
- safe validator/config handling
- harden the now-present transaction admission path and in-memory mempool without pretending it is durable yet

## Rules of engagement

### 0. Remote `dev` is the lab authority
For live lab work, treat the tip of remote `dev` as the current source of
truth.

Before making claims about:
- current cluster behavior
- active network paths
- inspection surfaces
- deployment expectations

check the configured remotes directly, for example:
- `git ls-remote github refs/heads/dev`
- `git ls-remote gretta refs/heads/dev`

If local `dev` is behind, say so explicitly before reasoning from local code.

### 1. Do not casually change consensus semantics
Before changing any of the following, add a short design note under `docs/`:
- block header fields
- tx root/state root logic
- proposer selection
- validator ordering semantics
- timestamp validation rules
- on-disk persistence format
- native QCOIN issuance rules or fee semantics

### 2. Failure paths matter as much as happy paths
For every substantial change, add tests for:
- success path
- malformed input
- persistence failure or restart behavior where relevant
- invalid peer/validator/network input where relevant

### 3. Prefer explicitness over cleverness
This codebase benefits more from:
- clear invariants
- narrow interfaces
- deterministic behavior
than from abstraction-heavy refactors.

### 4. Do not silently change operator-visible behavior
If you change:
- CLI flags
- default paths
- HTTP endpoints
- deployment file expectations
- validator config behavior
then update the docs in the same change.

## Where to put things

### Tests
Preferred placement:
- crate-local unit tests near the module for invariant-heavy logic
- integration tests where behavior crosses crate boundaries

Important areas that should gain more tests over time:
- `qcoin-ledger`
- `qcoin-consensus`
- `qcoin-node`

### Docs
Put short design notes in `docs/`.
Good examples of note topics:
- persistence model
- startup repair behavior
- fork-choice policy
- monetary policy
- qcoin exit gate
- lab service bootstrap
- EAB anchor transaction model
- validator ordering semantics
- EAB integration contract

### Deployment artifacts
If touching `deploy/`, verify that:
- filenames still match docs
- flags still match `qcoin-node --help` behavior
- examples still reflect current runtime assumptions

Current operator runbooks:
- [LAB_SERVICE_BOOTSTRAP.md](LAB_SERVICE_BOOTSTRAP.md) for the real three-node service-managed lab
- [three-node-eab-workflow.md](three-node-eab-workflow.md) for the older generic three-node walkthrough
- [ROLLOUT_POLICY.md](ROLLOUT_POLICY.md) for branch/deploy/promotion discipline

## Build and validation expectations

At minimum, future agents should run and report:
- `cargo build`
- `cargo test`

If the change touches node runtime behavior, also exercise a minimal run path such as:
- key generation
- one-node startup
- two-node sync if relevant

## Areas that are easy to break

### Persistence coupling
`qcoin-node` currently persists chain state and block history. Be careful not to introduce:
- height/history mismatch
- unrecoverable partial writes
- incompatible file layout without migration notes

Current persistence policy is documented in [PERSISTENCE_MODEL.md](PERSISTENCE_MODEL.md).
Block history is currently the authoritative local record; chain state is a rebuildable cache repaired from block history on startup.

### Peer sync assumptions
Current long-running sync is simple UDP qcoin-wire replication driven by `loadngo-proactor` and `loadngo/network`, with a presence announce plus direct node-info exchange, optional multicast discovery, transaction announce/fetch over UDP, and no full fork resolution yet.
Presence announce is separate from tip sync and should remain low-frequency. Current policy is a 42-second announce against bootstrap targets only, with direct node-info replies rate-limited to once every 42 seconds per source so multicast discovery does not degrade into peer-to-peer chatter.
Current operator inspection is over the UDP qcoin wire:
- `qcoin-node node-info --target <addr:port>`
- `qcoin-node tip --target <addr:port>`
- `qcoin-node block --target <addr:port> --height <n>`

Do not assume the old HTTP adapter still exists on live `dev`.
Read [FORK_CHOICE_POLICY.md](FORK_CHOICE_POLICY.md) before changing distributed behavior here.
If adding divergence detection, keep the reported behavior explicit.

### Validator handling
Be careful with:
- duplicate keys
- key normalization
- mixed signature schemes
- reliance on validator ordering without documenting it

### Native asset policy
Current monetary-policy direction is documented in [MONETARY_POLICY.md](MONETARY_POLICY.md).
Be careful not to blur:
- generic asset issuance through `CreateAsset`
- native QCOIN issuance
- fees vs protocol rewards

### Timestamp logic
Any future-skew or monotonicity change can affect multi-node behavior quickly. Document and test such changes.

## What not to change casually

Avoid casual changes to:
- `ChainState` serialized shape
- `BlockHeader` encoding
- `Transaction` hashing/id rules
- native QCOIN issuance/conservation rules
- default data paths used by `qcoin-node`
- deploy examples

If one of these must change, include:
- rationale
- migration or compatibility note
- updated docs
- updated tests

## Recommended working style for future agents

For non-trivial tasks:
1. update the corresponding item in `docs/WORK_QUEUE.md`
2. add a design note if semantics are changing
3. implement narrowly
4. add tests before broad refactors
5. update docs in the same patch

## Commit/change summary expectations

When handing off work, include:
- what changed
- why it changed
- files touched
- tests added
- known tradeoffs left unresolved
- any operational or migration implications

## If working on QCoin + EAB integration

Do not assume the current mirroring behavior is the final contract.
Read [QCOIN_EXIT_GATE.md](QCOIN_EXIT_GATE.md) before deciding qcoin bring-up work is complete enough to deprioritize.
Current status: the live three-node lab passed the exit gate on `2026-04-16`; re-open it only if the cluster stops meeting those checks after further changes.
Read [ROLLOUT_POLICY.md](ROLLOUT_POLICY.md) before pushing new node changes directly onto live lab machines.
Read [EAB_ANCHOR_TRANSACTION_MODEL.md](EAB_ANCHOR_TRANSACTION_MODEL.md) first.
If changing interaction between the two repos, document:
- what EAB is allowed to submit
- who signs what
- whether remote qcoin-node acceptance is expected
- what happens on mismatch or mirror failure
