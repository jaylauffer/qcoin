# QCoin Rollout Policy

Purpose: define the minimum rollout discipline for the lab cluster so that
`dev` work, live node deployment, and `main` promotion have continuity.

This policy exists because an operational cluster is not enough by itself. The
lab also needs a repeatable answer to:

- what exact source revision is running on each node
- whether the live nodes all match each other
- when a `dev` state is stable enough to promote toward `main`

## Branch roles

- `main`: reviewed baseline
- `dev`: active integration branch for the next lab candidate

Rule:

- new qcoin node/runtime work lands on `dev` first
- `main` is updated only by rebase/fast-forward promotion after a verified lab
  rollout candidate passes

## Deployment rules

### 1. Live nodes must run one exact committed revision

Do not treat “close enough” as acceptable.

Before a rollout is considered real:

- all live nodes must be on the same git commit
- all live node checkouts must be clean (`git status --short` empty)
- the deployed commit must already exist in git history on `dev`

Directly editing a live checkout and restarting the service is allowed only as
an emergency recovery step, not as the normal rollout path.

### 2. Tag every rollout candidate

Before cutting a real lab rollout, create an annotated tag on `dev` for the
candidate commit.

Recommended pattern:

- `lab-YYYYMMDD.N`

Example:

- `lab-20260417.1`

That tag is the deployment identity. The live nodes should be updated to that
tag or the exact commit it points to.

### 3. Normal service nodes run release binaries

For sustained lab operation, services should point at:

- `target/release/qcoin-node`

`target/debug/qcoin-node` is acceptable only for short-lived stabilization or
thermal-constrained emergency work. It should not be the steady-state service
target once a rollout candidate is being verified.

### 4. Record the deployed version on each node

Each node should have a small deployed-version record alongside the service
configuration. At minimum record:

- git commit SHA
- rollout tag
- build profile (`release` or `debug`)
- deployment timestamp

The exact storage path can be local to the operator environment, but the rule
is: operators should not have to guess what is running.

## Candidate rollout flow

### Step 1. Stabilize on `dev`

- make code changes on `dev`
- run local validation
- keep docs in the same patch

### Step 2. Commit before rollout

- commit the rollout candidate on `dev`
- do not deploy from uncommitted working tree state

### Step 3. Tag the candidate

- create an annotated tag such as `lab-YYYYMMDD.N`

### Step 4. Roll that exact candidate to every node

On each node:

- update the repo checkout to the tagged commit
- confirm the checkout is clean
- build `target/release/qcoin-node`
- update the service config to point at the release binary
- restart the service

### Step 5. Verify the cluster on the deployed candidate

At minimum verify:

- all nodes report the same `chain_id`
- all nodes report the same `height`, `tip_hash_hex`, and `state_root_hex`
- ingress through each node causes cluster reconvergence
- restart behavior remains boring
- idle behavior stays quiet enough to trust the logs

This is the lab-level acceptance gate for the rollout candidate.

### Step 6. Promote from `dev` to `main`

Once the tagged candidate is accepted:

- open a PR from `dev` to `main`
- rebase/fast-forward only
- do not create merge commits

### Step 7. Mark the promoted baseline

After `main` is updated, create a stable baseline tag on `main`.

Recommended pattern:

- `baseline-YYYYMMDD.N`

This is the “known good” point to return to if later `dev` work destabilizes
the lab.

## Emergency hotfix rule

Emergency direct propagation to a live node is allowed only to recover service
or unblock diagnostics.

If it happens:

1. commit the same fix back onto `dev` immediately
2. do not stack more live-only edits on top of it
3. do not call the cluster “rolled out” until all nodes are moved to the
   committed/tagged revision

The emergency path is for recovery, not for normal iteration.

## Current gap to close

At the time this policy was written, the immediate normalization work is:

- move the Pi services from `target/debug/qcoin-node` to
  `target/release/qcoin-node`
- remove any live-node checkout drift
- deploy one exact tagged `dev` commit to all three nodes

Only after that should the current cluster state be treated as a proper rollout
candidate for PR promotion to `main`.
