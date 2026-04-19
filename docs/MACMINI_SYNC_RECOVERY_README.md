# Macmini Sync Recovery README

Purpose: hand off the current qcoin cluster sync failure to another agent
working on the Macmini node at `10.10.10.2`.

## Current Branch

Repository:

- `/home/jay/pudding/qcoin`

Active branch:

- `dev`

Worktree status before this README was added:

- clean

## Current Live Failure

The cluster is split.

Observed live state:

- `192.168.1.123` is validator `68de0d5f...` and is at height `17`
- `192.168.1.140` is validator `5c8d2619...` and is at height `17`
- `10.10.10.2` is validator `20029273...` and is at height `16`

That means the third validator is online again, but stale.

Because the validator set is currently operating with deterministic proposer
scheduling and append-only tip extension, this stale validator is enough to
stall progress:

- pending metadata-only transactions are accepted into mempools
- the cluster does not advance to height `18`

## Concrete Evidence

The third validator is definitely the missing manifest key:

- manifest validator keys include:
  - `68de0d5f...`
  - `5c8d2619...`
  - `20029273...`
- `10.10.10.2:9700/node-info` reports `node_public_key_hex` beginning with
  `20029273...`

But the third validator tip remains behind:

- `10.10.10.2:9700/tip`
  - `height: 16`
  - `tip_hash_hex: b600734a71c8589655fb79e9400faec7c5ab7adf6cd62016464d6c68f0fadf64`

The other active validators remain at:

- `height: 17`
- `tip_hash_hex: 8d7e7776f9fed48373e657e3f86ea4d86e9b349c2acb4043e1d556831523a42d`

## Most Likely Code/Protocol Problems

### 1. Node identity is bound too tightly to socket address

Relevant files:

- `qcoin-node/src/wire.rs`
- `qcoin-node/src/node.rs`

`NodeInfo` currently contains:

- wire version
- compatibility floor
- software version
- chain id
- node public key
- capability list

It does **not** advertise listen endpoints.

Current peer tracking in `node.rs` is keyed by `SocketAddr`:

- `known_peers: HashSet<SocketAddr>`
- `peer_node_info: HashMap<SocketAddr, NodeInfo>`
- `peer_tip_heights: HashMap<SocketAddr, u64>`

This is fragile on a mixed network:

- the same validator identity may appear on `10.10.10.2:9700`
- while other peers are operating on `192.168.1.x`
- or via IPv6 link-local multicast discovery

So a validator can be "alive" but still fail to participate in catch-up if
peers are not using the right endpoint for actual sync traffic.

### 2. Stale nodes reject higher blocks but do not self-heal

Relevant files:

- `qcoin-node/src/node.rs`
- `qcoin-consensus/src/lib.rs`

Current behavior:

- block broadcasts arrive as `SubmitBlock`
- stale node calls `apply_remote_block(block)`
- consensus rejects any block that is not exactly `local_height + 1` with the
  current local `parent_hash`

If a node is one block behind and receives a later block broadcast, it rejects
the block but does **not** immediately request the missing predecessor block.

That means:

- broadcast block propagation does not recover stale nodes
- recovery depends entirely on the separate tip-sync / block-request path

### 3. Recovery depends on periodic tip sync rather than immediate sync trigger

Relevant file:

- `qcoin-node/src/node.rs`

After `handle_node_info`, the peer is accepted, but there is no immediate
`TipRequest`.

Instead, the node waits for the periodic sync loop:

- `schedule_sync`
- `broadcast_tip_requests`

This is not necessarily the root bug, but it makes rejoin behavior slower and
more brittle than it needs to be.

## Likely Failure Story

The most plausible current story is:

1. the Macmini validator went offline
2. the rest of the cluster advanced to `17`
3. the Macmini came back and now responds on `10.10.10.2:9700`
4. it did not successfully complete missing-block catch-up
5. it remains at `16`
6. because the system is still append-only and proposer-scheduled, the cluster
   stalls instead of cleanly progressing

## Recommended Changes

These are the changes the Macmini agent should evaluate and likely implement.

### A. Extend `NodeInfo` with endpoint advertisement

Add explicit reachable endpoints to `NodeInfo`, for example:

- `listen_endpoints: Vec<String>`

Those should include the addresses the node expects peers to use for direct UDP
sync traffic.

This lets a node say:

- "I am validator `20029273...`"
- "you may reach me on `10.10.10.2:9700`"
- optionally also `192.168.1.x:9700` or IPv6 addresses where appropriate

### B. Decouple validator identity from single `SocketAddr`

Do not treat `SocketAddr` alone as durable peer identity.

At minimum:

- track node identity by `node_public_key_hex`
- allow one validator identity to own multiple observed endpoints
- choose a preferred endpoint for sync/data plane traffic

This can start as a minimal map rather than a full peer-management redesign.

### C. On stale-block rejection, trigger block backfill

When `SubmitBlock` is rejected because the block is ahead of the local height
or has a mismatched parent due to missing history:

- do not only reply with rejection
- immediately request the next missing block from that peer
- or trigger the same logic used by `handle_tip_response`

The stale node should actively try to recover from block broadcast, not only
from the periodic tip poll.

### D. Trigger immediate tip sync after successful `NodeInfo`

When `handle_node_info` accepts a peer:

- send `TipRequest` immediately
- avoid waiting for the next scheduled sync tick

That reduces rejoin latency and makes recovery more deterministic.

### E. Add a regression test for stale-node rejoin

This test is the real target.

Needed scenario:

1. start a 3-node validator cluster
2. let node C stop while A/B advance by one block
3. restart node C
4. prove node C catches up from `height 16` to `17`
5. prove the cluster can then produce `18`

Without this test, the bug is likely to recur in a slightly different form.

## Exact Files To Inspect

- `qcoin-node/src/wire.rs`
- `qcoin-node/src/node.rs`
- `qcoin-node/src/main.rs`
- `qcoin-consensus/src/lib.rs`
- `qcoin/docs/LAB_SERVICE_BOOTSTRAP.md`

## Non-Goals

Do not broaden this into:

- full fork choice
- consensus redesign
- monetary policy work
- wallet UX

This is a catch-up / peer-identity / sync-trigger bug.

## Short Version

The Macmini validator is back online but stuck one block behind.

Most likely causes:

- peer identity is too tied to observed socket address
- endpoint advertisement is missing from `NodeInfo`
- stale block rejection does not trigger missing-block recovery

Primary target:

- make a stale validator rejoin from `16 -> 17` reliably, then prove the
  cluster can advance to `18`
- rejoin sync is too dependent on periodic polling

Fix the stale-node rejoin path first.
