# QCoin Work Queue

Purpose: break the higher-level review TODO into small, independently executable work items for future agents.

Status labels to use in updates:
- `todo`
- `in-progress`
- `blocked`
- `done`

Each item should be updated with:
- owner/agent
- date started
- date completed
- files touched
- tests added
- remaining risks

---

## QW-001 Crash-safe persistence design
Status: `todo`

Goal:
- choose and document a persistence model that prevents chain-state / block-history divergence

Tasks:
- inspect current `apply_block()` write path
- compare snapshot vs WAL vs manifest-swap approach
- write a short decision note under `docs/`

Definition of done:
- design note exists
- explicit chosen direction recorded
- migration impact noted

---

## QW-002 Crash-safe persistence implementation
Status: `todo`

Depends on:
- QW-001

Goal:
- implement crash-safe on-disk persistence

Tasks:
- update persistence code
- preserve startup compatibility or document migration
- ensure partial writes cannot leave unrecoverable mismatch

Definition of done:
- code merged
- failure-path tests added
- startup behavior documented

---

## QW-003 Startup repair policy
Status: `todo`

Goal:
- define and implement behavior when persisted files disagree

Tasks:
- choose repair behavior
- implement repair command or automatic repair path
- log actionable operator messages

Definition of done:
- mismatch cases handled deterministically
- tests cover state ahead, blocks ahead, and corruption cases

---

## QW-004 Peer divergence detection
Status: `todo`

Goal:
- detect when peers have same height but different tips

Tasks:
- compare local and remote tip hash/state root during sync
- surface divergence clearly in logs and return paths
- avoid silently assuming equal height means equal chain

Definition of done:
- divergence is detected
- operator can see clear diagnostics
- test covers equal-height fork case

---

## QW-005 Fork-choice policy note
Status: `todo`

Goal:
- define whether QCoin currently supports fork resolution or append-only replication

Tasks:
- write short note describing current and intended behavior
- define acceptance criteria for any future reorg support

Definition of done:
- document exists
- future agents are not left guessing about intended model

---

## QW-006 Sync robustness improvements
Status: `todo`

Goal:
- improve resilience and diagnosability of peer sync

Tasks:
- classify errors by transport/parse/validation/persistence
- improve log messages
- review timeout and retry behavior

Definition of done:
- logs are meaningfully actionable
- timeout and malformed-payload tests exist

---

## QW-007 Validator configuration hardening
Status: `todo`

Goal:
- validate validator input clearly and consistently

Tasks:
- reject malformed hex with precise errors
- define duplicate-key behavior
- define whether validator ordering is consensus-critical

Definition of done:
- validator config behavior is documented
- tests cover malformed, duplicate, and mismatched-scheme cases

---

## QW-008 Timestamp policy hardening
Status: `todo`

Goal:
- define acceptable timestamp rules for multi-node operation

Tasks:
- decide future-skew tolerance
- implement validation rule if desired
- document clock expectations

Definition of done:
- timestamp policy is explicit
- tests cover stale and future timestamps

---

## QW-009 Transaction submission API design
Status: `todo`

Goal:
- define how end users or services submit transactions to a node

Tasks:
- choose endpoint shape
- define pre-validation behavior
- define duplicate detection and response model

Definition of done:
- API design note exists
- mempool direction decided

---

## QW-010 Mempool implementation
Status: `todo`

Depends on:
- QW-009

Goal:
- support pending transaction collection and block inclusion

Tasks:
- add pending transaction queue
- include pending txs in locally produced blocks
- reject invalid or duplicate txs

Definition of done:
- valid tx can be submitted and mined
- tests cover acceptance, rejection, and inclusion

---

## QW-011 Ledger edge-case test expansion
Status: `todo`

Goal:
- deepen `qcoin-ledger` invariant coverage

Tasks:
- add multi-input multi-output conservation tests
- add metadata-only anchor style tests
- add cross-asset edge cases
- add repeated asset creation boundary cases

Definition of done:
- new tests materially expand invariant coverage

---

## QW-012 QCoin + EAB integration contract
Status: `todo`

Goal:
- make integration expectations explicit

Tasks:
- define what EAB is allowed to submit
- define validator/signing expectations
- define local-only vs remote-node mirror modes

Definition of done:
- written contract exists under `docs/`
- mismatch behavior is explicit

---

## QW-013 Observability pass
Status: `todo`

Goal:
- expose basic operator-facing state and diagnostics

Tasks:
- standardize logs
- consider health endpoint or sync status output
- include peer list, current tip, and last failure details

Definition of done:
- operator can understand current node state without reading source

---

## QW-014 Deployment/runbook cleanup
Status: `todo`

Goal:
- make deployment assets trustworthy for repeatable bring-up

Tasks:
- verify `deploy/` files match actual flags and behavior
- add minimal runbook
- add troubleshooting notes for validator and peer issues

Definition of done:
- docs allow a clean bring-up without source spelunking

---

## Suggested near-term execution order
1. QW-001
2. QW-002
3. QW-003
4. QW-004
5. QW-007
6. QW-008
7. QW-009
8. QW-010
9. QW-012
10. QW-013
11. QW-014
