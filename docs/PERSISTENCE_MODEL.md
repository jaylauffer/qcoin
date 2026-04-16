# QCoin Persistence Model

This note documents the current on-disk persistence behavior in `qcoin-node`.

## Current files

`qcoin-node` currently persists two files:

- chain state snapshot
- block history snapshot

The default operator layout still places both under `/var/lib/qcoin/`.

## Source of truth

The local block-history file is treated as the authoritative record.
The chain-state file is a derived cache that exists to make startup cheaper.

That means:

- block application persists block history first
- chain state is written second
- startup rebuilds chain state from block history
- chain state is stored as a JSON snapshot wrapper with explicit array entries for UTXOs and assets, rather than raw JSON maps with binary/struct keys

## Startup repair policy

On startup, the node:

1. loads block history
2. replays that history from genesis
3. compares the rebuilt result with the stored chain-state snapshot

If the stored chain state is missing or does not match the replayed block history, the node rewrites the chain-state file to the replayed result and continues.

This handles the common mismatch case where a previous run wrote state ahead of block history.

## Failure policy

The node does **not** silently ignore malformed persistence files.

- malformed chain-state JSON is a startup error
- malformed block-history JSON is a startup error
- invalid blocks inside the stored block history are a startup error

This is intentional: block history is authoritative, so corruption there must be explicit.

## Durability notes

Current writes use:

- temp-file write
- file `sync_all()`
- rename into place
- parent-directory sync on Unix

This is safer than the earlier write path, but it is still not a full journaled or version-swapped storage design.

## Remaining limitations

Current persistence is still a bootstrap implementation:

- chain state and block history are separate files
- the full block-history snapshot is rewritten on each committed block
- there is no WAL or append-only block log yet
- there is no alternate-branch storage or reorg recovery

Future storage work should evolve this into a proper snapshot+journal or append-log model.
