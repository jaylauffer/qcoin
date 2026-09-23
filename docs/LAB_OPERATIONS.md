# Lab operations

Service recovery verified, 2026-09-23. This is the current topology; April's
three-node walkthrough is historical, not the configuration to reinstall.

## Existing nodes

| Node | SSH | QCoin UDP | Service |
|---|---|---|---|
| Agnes | `jay@192.168.1.160` | `192.168.1.160:9700` | system `qcoin-node.service` |
| Dolores | `jay@10.10.10.6` | `192.168.1.140:9700` | Jay's user `qcoin-node.service` |

Both retain **chain 1**, the existing two-validator manifest and original
Dilithium2 keys. Do not regenerate keys, delete state, or enable empty blocks
to make the height move. An idle chain should not grow.

The Mac mini still has a different **chain 0**, height 24, and is deliberately
offline. Adding it to chain 1 needs a separate membership/configuration decision.
Do not start its old LaunchAgent as part of this recovery.

## Check and control

Run from the Mac's `qcoin/` checkout:

```sh
./target/release/qcoin-node tip --target 192.168.1.160:9700
./target/release/qcoin-node tip --target 192.168.1.140:9700
ssh jay@192.168.1.160 'systemctl is-active qcoin-node.service'
ssh jay@10.10.10.6 'systemctl --user is-active qcoin-node.service'
```

Both tips must agree on height, tip hash and state root; `node-info --target`
also reports the chain ID. A green
systemd status alone is insufficient. For an intentional restart:

```sh
ssh jay@192.168.1.160 'sudo systemctl restart qcoin-node.service'
ssh jay@10.10.10.6 'systemctl --user restart qcoin-node.service'
```

Use `stop` instead of `restart` to stop a node. Agnes uses a system unit;
Dolores uses a user unit with lingering enabled. Both units are enabled.
The current CLI can print an error and still exit zero: require a valid JSON
response, not just a successful shell exit status.

## Files

| Purpose | Agnes | Dolores |
|---|---|---|
| Release executable | `/home/jay/.local/bin/qcoin-node` | same |
| Environment | `/etc/qcoin/qcoin-node.env` | `/home/jay/.config/qcoin/qcoin-node.env` |
| Manifest and key | `/etc/qcoin/` | `/home/jay/.config/qcoin/` |
| State and blocks | `/var/lib/qcoin/*chain1.json` | `/home/jay/.local/share/qcoin/*chain1.json` |
| Logs | `/var/log/qcoin/node.log`, `node.err` | `/home/jay/.local/state/qcoin/node.log`, `node.err` |

Keep private key contents out of logs and reports. `node.err` also contains
historical errors: compare timestamps with the current service start.

## What broke and what changed

1. Both services referenced missing binaries under `target/release/`.
   Build output is disposable; the services now use an installed release copy
   under `/home/jay/.local/bin/`.
2. Both listen addresses were stale after the lab rearrangement. Corrected
   Agnes `.123` → `.160`, Dolores `.129` → `.140` on `192.168.1.0/24`.
3. Loadngo's epoll backend removed persistent socket-readiness registration
   after its first event. Nodes answered once, then accumulated unread UDP.
   Loadngo commit `ed3410fc3543d8b2193356d78734daaaecbdd037` fixes this;
   its new repeated-datagram test failed before and passed after on Dolores.
   The existing epoll suite now runs on Linux as well as Android.

Dependencies were reused from the Mac's cache and transferred over the LAN;
builds used `--locked --offline`. No key, membership, or chain reset occurred.
No Starlight, desktop, audio or Wi-Fi settings were changed.

Recovery backups (original environment, manifest, state and blocks; keys left
in place): Agnes `/var/tmp/qcoin-recovery.pHI85V`; Dolores
`/home/jay/.local/state/qcoin/recovery.QBGGjF`. Do not restore the old environment
unchanged: it contains the broken binary path and obsolete listen address.
Do not restore an old ledger over later accepted blocks.

## Verified deployment and evidence

Verified at approximately **08:43 UTC, 2026-09-23**:

- Both nodes advertise chain **1**, validator and block-producer capability.
- Repeated tip queries work from the Mac and from each Pi, including after
  controlled service restarts. Both retain height **2** and these identical values:
  - tip: `bce9a309a8ae002db31fda0d93fd5ede0010ca158ce212b183fd1d4e4aa6bb1b`
  - state root: `e114ca9e79ea4d3b8daad623ef5f0fc758c64668c7cba95df805fe761b91f815`
- Block 2 queried through either node yields identical JSON (SHA-256
  `5f5c70048ec11bb9e643aa318c80cc33e483c2d80fcd2bee88cdf899a8620c23`).
- Both logs record the other Pi as a reliable IPv6 link-local peer. UDP receive
  queues are empty; neither service has automatically restarted after recovery.
- A 10-second idle `top` sample reports QCoin at 0.0% CPU / 3.5 MiB RSS on
  Dolores, 0.1% / 3.3 MiB on Agnes. This is an idle snapshot, not a load or
  thermal-safety test. Error logs have not grown since the old failure loop.
- One Mac-to-Dolores identity query timed out; subsequent tip and identity
  queries passed without restarting anything. Transport reliability is not
  fully established, and the cause of that individual timeout is unproven.

Both installed ARM64 Linux executables have SHA-256:
`96216e6289d7b52238cc80d03a1175a019cf81cfdbf289b8c0c404e234837448`.
Built on Dolores with Rust 1.98.1, release profile, from clean source checkouts:

- QCoin: `6579c0a54635ef913806922a88049a6b5e68e840`
- Loadngo dependency: `ed3410fc3543d8b2193356d78734daaaecbdd037`

Each node has `deployed-version.json` beside its environment file. Agnes received
the identical executable, not a separate build. **Its older Loadngo checkout
was not updated**: consult these revisions before rebuilding there.

Validation: 25 QCoin node tests on both Linux and macOS; full proactor suites
on Linux (34 tests, including 14 epoll and 11 io_uring) and macOS (21 tests);
Linux proactor all-target/all-feature Clippy with `-D warnings`; proactor
formatting and diff checks. No full-workspace CI or Android device run was
performed for this repair. Loadngo fix committed locally; no push performed.

## Remaining limits

The active peer path still uses Wi-Fi because Agnes has no wired link. This
recovery does not claim to solve the lab's Wi-Fi reliability problem. If DHCP
changes either address again, review the environment before restarting; router
DHCP reservations are a separate configuration task.

Service recovery is not a new full acceptance of the historical three-node
exit gate. New transaction ingress and reconvergence require a separately
identified test workload; do not fabricate rewards or move assets just to
produce a green status report.
