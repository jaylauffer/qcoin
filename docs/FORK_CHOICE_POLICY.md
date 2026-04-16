# QCoin Fork-Choice Policy

This note defines the current distributed behavior of `qcoin-node`.

## Current model

QCoin does **not** currently implement full fork choice or reorg handling.

The live node behaves as an append-only replicated chain with:

- an explicit validator set
- deterministic proposer ordering based on validator list position
- block acceptance only when the new block extends the node's current local tip

In practical terms, this means the node assumes there is one canonical branch and tries to keep peers aligned on that branch.

## What the node accepts today

A block is only accepted when all of the following are true:

- `block.header.height == local_height + 1`
- `block.header.parent_hash == local_tip_hash`
- timestamp and signature checks pass
- the proposer is exactly the validator expected for that height
- transaction root and state root both validate against the current local chain state

This is deterministic and simple, but it is **not** fork resolution.

## What the node does not do yet

The current implementation does **not**:

- keep alternate branches
- compare competing branches at the same height
- roll back already-applied blocks
- re-execute ledger state onto a different branch
- choose a winner by weight, votes, stake, work, or finality threshold

If two different blocks exist for the same height, the node does not have a rule like "pick the better branch." It only knows whether an incoming block extends the tip it already has.

## Meaning of "no full fork resolution yet"

When docs say "no full fork resolution yet," the exact meaning is:

- equal-height divergent tips are a divergence condition, not a resolvable normal case
- a longer branch that does not extend the current local tip is not automatically adopted
- a node that has already applied one branch cannot currently reorg onto another branch

So the current system is best described as:

- **consensus scheduling**: yes
- **append-only validation**: yes
- **fork choice / reorg support**: no

## Operational implications

For the current bootstrap cluster, operators should assume:

- validator ordering is consensus-critical
- all validators must share the same validator set and ordering
- a misconfigured validator or divergent local state can stall or split the cluster
- same-height different-tip behavior should be treated as an error condition to investigate, not something the node will self-heal

This is why small, controlled validator sets are acceptable right now, but free-form or adversarial networks are not yet.

## Relationship to peer sync

Current peer sync is sequential and tip-oriented:

- peers exchange hello information
- peers exchange current tip metadata
- if a peer is ahead and extends the local branch, the node requests the next missing height

This makes steady-state replication work well on a coherent cluster, but it does not define branch competition.

## What future fork resolution would require

Before QCoin can claim full fork choice, it needs all of the following:

- divergence detection for equal-height different-tip peers
- explicit policy for competing branches
- storage model for alternate branches or replay
- rollback/reorg path for chain state
- clear operator-visible diagnostics when divergence occurs

Until then, the correct description of the live system is:

**QCoin currently uses deterministic proposer scheduling with append-only tip extension, not a full fork-resolving blockchain protocol.**
