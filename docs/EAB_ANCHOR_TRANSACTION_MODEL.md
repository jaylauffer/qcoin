# QCoin EAB Anchor Transaction Model

Purpose: define the proof-of-concept contract between `entitlement-achievement-blockchain` and `qcoin` so qcoin core nodes can serve as a stable proof layer without becoming the player-facing system.

## Why this exists

The current docs across `qcoin`, `entitlement-achievement-blockchain`, and `Zhoenus` agree on the system shape:

- `Zhoenus` is local-first gameplay
- `entitlement-achievement-blockchain` is the gameplay-facing authority
- `qcoin` is the auditable settlement / proof layer behind EAB

What is still ambiguous in code is the actual anchoring contract.

In particular, the current EAB qcoin mirror path still proposes qcoin blocks locally with a fresh dummy consensus engine. That is a bootstrap shortcut, not the intended long-term relationship between a real EAB service and a stable qcoin node cluster.

## Core roles

### Zhoenus

`Zhoenus` should:
- submit claims or run-summary events to EAB
- read authoritative rewards and receipts back from EAB
- never submit directly to qcoin for normal gameplay

### EAB

EAB should:
- decide which player-facing events become authoritative achievements or entitlements
- maintain the canonical player-facing receipt surface
- decide which authoritative records are worth anchoring into qcoin

### qcoin

qcoin should:
- accept deterministic anchor transactions from EAB
- order them on a small trusted validator cluster
- persist them durably
- return verifiable block inclusion proof material back to EAB

## What should be anchored first

For the first proof-of-concept, qcoin should anchor only authoritative EAB outcomes.

Anchor first:
- authoritative achievement awards
- authoritative entitlement grants

Do not anchor first:
- raw player claims
- raw offline evidence payloads
- routine gameplay telemetry
- local-only progression details
- every `run_completed` event from `Zhoenus`

Reason:
- claims are not authoritative awards
- raw telemetry is noisy and privacy-sensitive
- Zhoenus docs already position qcoin as an optional receipt layer behind EAB, not as the first sink for normal gameplay events

## Source-of-truth model

For the proof-of-concept:

- EAB remains the source of truth for player-facing reward semantics
- qcoin is the proof / audit layer for selected authoritative EAB records

That means:
- a reward is authoritative because EAB issued it according to policy
- qcoin anchoring proves that this authoritative record existed in a durable ordered ledger
- qcoin does not replace EAB's profile or reward model in the PoC

## Mirror consistency model

The preferred PoC consistency model is:

1. EAB appends the authoritative local record first.
2. EAB enqueues a qcoin anchor outbox item.
3. EAB submits that anchor transaction to qcoin asynchronously.
4. EAB records qcoin receipt fields only after qcoin acceptance/inclusion is confirmed.

Implications:
- a temporary qcoin outage must not erase an already-issued authoritative EAB award
- qcoin anchoring should be retried deterministically
- EAB should distinguish:
  - local authoritative receipt
  - qcoin-anchored receipt

This matches the local-first and offline-capable posture already documented for Zhoenus and EAB.

## Submission contract

EAB should submit **transactions**, not blocks.

Specifically:
- EAB should use qcoin's transaction submission surface
- qcoin nodes remain solely responsible for block proposal, validation, and ordering
- EAB should not sign or construct final qcoin blocks
- EAB should not rely on a local fresh dummy consensus engine as its integration contract

This keeps responsibilities clean:
- EAB expresses intent to anchor a record
- qcoin decides inclusion and returns proof material

## Anchor transaction shape for the PoC

Given current qcoin transaction kinds, the PoC anchor should use a metadata-only transaction.

PoC shape:
- transaction kind: `Transfer`
- inputs: none
- outputs: one output
- assets: empty
- `metadata_hash`: hash of a canonical EAB anchor payload

Important:
- this is a proof-of-concept anchoring shape, not the final ideal transaction taxonomy
- it is acceptable only because it creates no asset balances and exists solely to anchor metadata
- if qcoin later adds a dedicated `Anchor` transaction kind, the EAB contract should migrate to that explicit primitive

## Canonical EAB anchor payload

The qcoin transaction should not carry the full EAB record directly.
It should carry `metadata_hash = hash(canonical_anchor_payload)`.

The canonical anchor payload should include:
- `anchor_schema_version`
- `record_type`
  - `achievement_award`
  - `entitlement_grant`
- `player_id` or another stable pseudonymous account identifier
- `developer`
- `game`
- reward identifier
  - `achievement_id` or `entitlement_id`
- EAB transaction identifier
- EAB block hash
- authoritative timestamp
- optional quantity / expiration fields for entitlements
- optional claim identifier if the award came from claim promotion
- optional hash of private evidence, never the raw evidence payload itself

The canonical payload must be serialized deterministically so independent verifiers can recompute the same hash.

## Verification model

An external verifier should be able to:

1. obtain the authoritative EAB reward record
2. build the canonical anchor payload
3. hash it
4. find the qcoin transaction whose `metadata_hash` matches
5. confirm that transaction is included in a qcoin block accepted by the cluster

So the qcoin-backed receipt returned by EAB should eventually expose:
- qcoin transaction id
- qcoin block height
- qcoin block hash or tip context
- the canonical anchor payload hash

## What qcoin core nodes must be good at first

The qcoin cluster does **not** need full reserve-asset economics to serve this PoC.

It does need:
- stable validator configuration on a small trusted cluster
- crash-safe or at least repairable local persistence
- deterministic transaction submission and inclusion
- actionable sync and transport diagnostics
- explicit divergence detection for same-height / different-tip cases
- clear operator visibility into tip, peers, and failure state

For this use case, these qcoin work items matter more than native monetary policy:
- peer divergence detection
- sync robustness improvements
- validator configuration hardening
- timestamp policy hardening
- transaction submission hardening
- observability
- EAB integration cleanup

## Explicitly out of scope for this PoC

This proof-of-concept does **not** require:
- direct qcoin integration from `Zhoenus`
- native QCOIN balances in the client gameplay loop
- fee markets
- staking economics
- public wallet UX
- full fork resolution for open/adversarial networks

## Immediate implementation direction

1. qcoin:
   - keep the three-node validator cluster stable
   - improve divergence detection and operator diagnostics
   - preserve metadata-only anchor transaction support during the PoC

2. EAB:
   - stop treating local dummy block proposal as the intended integration model
   - replace mirror-by-block-submission with anchor-transaction submission
   - add an outbox/retry path for qcoin anchoring
   - expose qcoin receipt fields separately from local authoritative receipts

3. Zhoenus:
   - continue targeting EAB only
   - use EAB claims / receipts
   - treat qcoin as an optional proof layer behind EAB

## Short version

For the proof-of-concept:

- EAB issues the authoritative reward
- qcoin anchors a deterministic hash of that authoritative record
- Zhoenus talks to EAB, not qcoin
- qcoin nodes need to be stable proof infrastructure before they need to be a full monetary system
