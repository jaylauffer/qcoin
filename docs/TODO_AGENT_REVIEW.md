# QCoin Agent Review TODO

Purpose: give future agents a concrete, execution-oriented checklist for reviewing, hardening, and extending QCoin.

## Current snapshot

QCoin currently contains:
- `qcoin-crypto`
- `qcoin-types`
- `qcoin-script`
- `qcoin-ledger`
- `qcoin-consensus`
- `qcoin-node`

The node currently supports:
- `GET /node-info`
- `GET /tip`
- `GET /blocks/{height}`
- `POST /blocks`
- local block production
- explicit qcoin presence announce plus node-info compatibility exchange with chain and wire-version checks
- proactor-driven UDP peer sync for long-running nodes
- optional IPv6 multicast discovery for peer bootstrap
- HTTP compatibility sync for `--once`
- persisted chain state and block history

Current monetary-policy truth:
- generic asset issuance exists today
- native QCOIN monetary policy is documented in [MONETARY_POLICY.md](MONETARY_POLICY.md)
- native QCOIN issuance is not implemented yet

## Priority 0: preserve integrity and recoverability

### 1. Make persistence crash-safe
Current risk:
- `qcoin-node` saves chain state and block history as separate files.
- a crash between those writes can leave the node unrecoverable or refusing startup.

Tasks:
- review `apply_block()` persistence path
- design one of these approaches:
  - single snapshot file containing both chain state and block history
  - write-ahead journal plus replay
  - atomic manifest/version swap model
- implement crash-safe persistence
- document on-disk format

Required tests:
- simulate interrupted write after state save and before block-history save
- verify restart behavior is deterministic and recoverable
- verify no silent truncation or height mismatch remains

### 2. Add startup repair path
Current status:
- startup now treats block history as authoritative local history
- chain state is rebuilt or truncated to match block history on startup
- malformed state/history files still stop startup explicitly instead of being ignored

Documented behavior:
- see [PERSISTENCE_MODEL.md](PERSISTENCE_MODEL.md)

Required regression tests:
- state ahead of blocks
- blocks ahead of state
- corrupted history JSON
- corrupted state JSON

## Priority 1: correct distributed behavior

### 3. Define fork-choice and divergence handling
Current risk:
- peer sync still advances only by height
- equal-height divergent chains are not reconciled
- no rollback/reorg mechanism exists

Current documented rule:
- see [FORK_CHOICE_POLICY.md](FORK_CHOICE_POLICY.md)
- current qcoin is append-only tip extension, not full fork resolution

Tasks:
- define chain selection rule
- add detection for same-height different-tip cases
- add rollback or alternate branch handling if desired
- evolve beyond the current documented append-only replication model if desired

Required tests:
- two peers with same height and different tips
- remote longer valid chain
- remote invalid chain
- remote shorter chain

### 4. Improve peer sync robustness
Tasks:
- add explicit network timeouts and retry/backoff policy review
- classify sync failures into discovery, transport, parse, validation, and persistence errors
- improve sync logging for operator diagnosis
- consider sync checkpoint or batch mode for large histories

Required tests:
- timeout or no-response during UDP tip discovery
- timeout or no-response during UDP `BlockRequest` / `BlockResponse`
- malformed qcoin UDP frame
- valid block rejected by consensus

## Priority 2: consensus and validator handling

### 5. Harden validator configuration behavior
Tasks:
- review validator key parsing and normalization
- reject mixed schemes or invalid duplicates clearly
- define whether validator order is consensus-critical
- document proposer rotation semantics

Required tests:
- duplicate validator keys
- empty validator set
- malformed validator key hex
- mismatched signing scheme

### 6. Review timestamp semantics
Current risk:
- validation only checks that timestamp is strictly increasing versus previous block
- no future-skew policy is defined

Tasks:
- decide acceptable future clock skew
- implement validation rule if needed
- document operational expectations for multi-node deployments

Required tests:
- timestamp equal to previous block
- timestamp lower than previous block
- timestamp far in the future

## Priority 3: transaction flow and product completeness

### 7. Add transaction submission path
Current state:
- node accepts transactions over the UDP qcoin wire
- transactions land in an in-memory mempool
- producers stay idle by default unless there is pending work
- multicast is used for transaction announcement, while full transaction fetch stays unicast

Tasks:
- harden transaction admission policy and error reporting
- decide whether mempool persistence or replay is required for bootstrap nodes
- validate transactions before inclusion
- include queued transactions in produced blocks
- document transaction lifecycle

Required tests:
- valid transaction accepted into mempool
- invalid transaction rejected
- duplicate transaction rejected
- block producer includes pending transactions

### 8. Expand script and asset workflow coverage
Tasks:
- review `qcoin-ledger` asset and script invariants
- separate generic asset issuance from future native QCOIN rules
- identify missing transaction kinds and asset lifecycle rules
- define metadata and issuer authorization expectations more fully

Required tests to add:
- multi-input multi-output conservation case
- zero-asset outputs with metadata only
- repeated asset creation attempts across heights
- cross-asset conservation edge cases
- large witness payload rejection policy if needed

## Priority 4: operational polish

### 9. Improve node observability
Tasks:
- add structured logs or consistent log prefixes
- expose basic health and sync diagnostics
- record current peers, tip, last successful sync, and last validation failure

### 10. Deployment review
Tasks:
- review `deploy/` assets for completeness
- verify systemd environment assumptions
- confirm example configs match actual runtime flags
- add minimal operator runbook

## QCoin + EAB integration tasks

### 11. Stabilize remote block submission expectations
Known concern:
- EAB mirroring currently uses a fresh default dummy consensus engine when anchoring locally
- that may not align with a real qcoin-node validator set used for remote submission

Current design note:
- see [EAB_ANCHOR_TRANSACTION_MODEL.md](EAB_ANCHOR_TRANSACTION_MODEL.md)

Tasks:
- define supported integration contract between EAB and QCoin
- decide whether EAB should:
  - submit transactions only
  - submit unsigned block intents
  - sign with a configured validator identity
- remove any ambiguous behavior

Required tests:
- EAB mirror to local-only state
- EAB mirror to remote qcoin-node with matching validator
- EAB mirror to remote qcoin-node with mismatched validator

## Suggested execution order
1. crash-safe persistence
2. startup repair logic
3. divergence/fork handling definition
4. validator/timestamp hardening
5. transaction submission + mempool
6. EAB integration cleanup
7. observability and deployment polish

## Deliverables expected from future agents
For each completed task, future agents should produce:
- code changes
- tests covering failure paths and happy paths
- short design notes in `docs/`
- clear statement of remaining tradeoffs
