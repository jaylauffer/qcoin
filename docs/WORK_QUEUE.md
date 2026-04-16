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
Status: `done`

Goal:
- define and implement behavior when persisted files disagree

Tasks:
- choose repair behavior
- implement repair command or automatic repair path
- log actionable operator messages

Definition of done:
- mismatch cases handled deterministically
- tests cover state ahead, blocks ahead, and corruption cases

Result:
- implemented in `qcoin-node` with block history as the authoritative local record
- startup replays block history, repairs chain state to match, and refuses malformed persistence files explicitly
- documented in `docs/PERSISTENCE_MODEL.md`

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
Status: `done`

Goal:
- define whether QCoin currently supports fork resolution or append-only replication

Tasks:
- write short note describing current and intended behavior
- define acceptance criteria for any future reorg support

Definition of done:
- document exists
- future agents are not left guessing about intended model

Result:
- documented in `docs/FORK_CHOICE_POLICY.md`

---

## QW-006 Sync robustness improvements
Status: `todo`

Goal:
- improve resilience and diagnosability of peer sync

Tasks:
- classify errors by discovery/transport/parse/validation/persistence
- improve log messages
- review timeout and retry behavior for qcoin UDP wire traffic and multicast discovery
- include handshake/version mismatch diagnostics in peer failure reporting

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
Status: `in_progress`

Goal:
- define how end users or services submit transactions to a node

Tasks:
- document and stabilize the current UDP `SubmitTransaction` / `SubmitTransactionResponse` flow
- define pre-validation behavior
- define duplicate detection and response model
- decide whether a zero-config multicast submitter is needed beyond node-to-node announce/fetch

Definition of done:
- API design note exists
- mempool direction decided

---

## QW-010 Mempool implementation
Status: `in_progress`

Depends on:
- QW-009

Goal:
- support pending transaction collection and block inclusion

Tasks:
- harden the current in-memory pending transaction queue
- define restart behavior for uncommitted transactions
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
Status: `done`

Goal:
- make integration expectations explicit

Tasks:
- define what EAB is allowed to submit
- define validator/signing expectations
- define local-only vs remote-node mirror modes

Definition of done:
- written contract exists under `docs/`
- mismatch behavior is explicit

Result:
- documented in `docs/EAB_ANCHOR_TRANSACTION_MODEL.md`
- PoC contract is now explicit: EAB submits anchor transactions, not blocks
- current mismatch is explicit: local dummy block proposal is a bootstrap shortcut, not the intended stable integration model

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

## QW-014 QCoin exit gate
Status: `done`

Goal:
- define the exact point at which qcoin cluster bring-up stops being the main blocker and EAB work should take priority

Tasks:
- define the three-node operational checks
- state the accepted bootstrap limitations explicitly
- record what work begins after the gate passes

Definition of done:
- handoff gate exists under `docs/`
- three-node workflow links to it

Result:
- documented in `docs/QCOIN_EXIT_GATE.md`
- the qcoin-to-EAB focus shift is now an explicit lab decision, not a vague judgment call

---

## QW-015 Deployment/runbook cleanup
Status: `todo`

Goal:
- make deployment assets trustworthy for repeatable bring-up

Tasks:
- verify `deploy/` files match actual flags and behavior
- add minimal runbook
- add troubleshooting notes for validator, peer, and multicast interface issues

Definition of done:
- docs allow a clean bring-up without source spelunking

---

## QW-016 Monetary policy definition
Status: `done`

Goal:
- define native QCOIN clearly enough that future code changes stop conflating it with generic assets

Tasks:
- separate native QCOIN from `CreateAsset`-based assets
- record the intended source of supply
- define how fees and validator rewards fit into the first real monetary model

Definition of done:
- curated note exists under `docs/`
- README links to it
- future agents have one source of truth

Result:
- documented in `docs/MONETARY_POLICY.md`
- current repo truth is explicit: generic asset minting is not native QCOIN issuance
- chosen v1 direction is genesis allocations plus protocol block rewards, with fees flowing through the same reward path once enabled

---

## QW-017 Native QCOIN implementation
Status: `todo`

Depends on:
- QW-015

Goal:
- implement native QCOIN as a protocol asset instead of relying on generic asset issuance paths

Tasks:
- reserve a `QCOIN_ASSET_ID`
- add genesis allocation support
- add protocol reward semantics
- prevent `CreateAsset` and zero-input flows from minting native supply
- define first-pass fee handling

Definition of done:
- native QCOIN has protocol-defined supply rules
- invalid native-issuance attempts are rejected
- tests cover genesis, rewards, fees, and conservation

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
11. QW-015
12. QW-017
