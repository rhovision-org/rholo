# Updating a running validator

One procedure for replacing the binary on a validator that is already part of a chain, so that every
update converges on the same shape and the same evidence. It exists because the absence of one cost two
avoidable cycles on 2026-10-29/30: the deployer key was inlined into a systemd drop-in (losing the
`EnvironmentFile` the base unit already had), and the base unit was left naming a binary that was no
longer running.

Read [Operating the node](operating.md) for the deployment, admission and finality behaviour this
assumes, and [The public testnet](testnet.md) for the concrete hosts.

## Decide first: swap, or rebuild

| the new build… | what it is | what the operator does |
|---|---|---|
| changes `--version` only, with identical genesis, store format **and** consensus rules | potentially compatible binary swap | verify old/new binary behavior and roll one validator at a time |
| writes a **higher store-format marker** | possibly irreversible data migration | **do not swap a populated store** until the exact-format copy/rollback/restart acceptance in [#520](https://github.com/rhovision-org/rholo/issues/520) passes |
| changes **gas charges** | may change genesis and replay state hashes | determine whether the chain requires a newly authorized genesis/activation; **never just regenerate a pin on an existing network** |
| changes consensus-visible validity rules | old and new binaries may disagree on the same signed block | require an explicit coordinated activation and compatibility plan; whether a rebuild is needed depends on the transition |

A changed `--version`, unchanged store marker and matching genesis pin are **not sufficient**
to establish binary compatibility. Compare the actual consensus, execution and state-format
changes against the released build; route any non-equivalent revision through the appropriate
release/upgrade decision before touching validators.

## Preflight — read-only, recorded

```sh
UNIT=rnode; DATA=/var/lib/rnode

systemctl show $UNIT -p ExecStart --value          # effective command, not the unit file
cat $DATA/store-format                             # the marker this store was stamped with
curl -s localhost:40403/api/v1/status | jq -r '.version.node, .latestBlockNumber'
curl -s localhost:40403/api/last-finalized-block | jq -r '.blockInfo.blockNumber'
free -m; df -h /var; uptime
```

Record every line of that in the change note: it is the only way to tell an update that worked from an
update that appeared to. Then confirm two things before touching anything:

- **every validator in the network is on the same build** and reports the same genesis;
- **the chain is quiet.** Never update under a block burst — a restart during one is what split this
  network 50/50 at height 128 ([#522](https://github.com/rhovision-org/rholo/issues/522)).
- **a verified, offline-restorable rollback exists for each affected validator**: preserve
  source/binary SHA-256, complete filesystem-consistent store and node-identity snapshots,
  current `store-format`, genesis pin and effective systemd drop-ins. Test the restore on
  disposable copies. Do not rely on a previous executable alone if the marker can advance.
- **a store-format/consensus/genesis compatibility decision is recorded** for this exact
  old/new pair. A green unit CI badge is not evidence of populated-store migration,
  independent replay or safe coordinated rollout (see #520 and #428).

## Stage the artifact

```sh
install -o root -g root -m 755 ./rnode /usr/local/bin/rnode-<sha8>
sha256sum /usr/local/bin/rnode-<sha8>   # must equal the recorded artifact hash
/usr/local/bin/rnode-<sha8> --version   # must equal the intended build
```

**Never stage under `/root`.** The unit runs as user `rnode`, which cannot traverse an `0700` home, and
the service fails immediately with `203/EXEC` — a start-up failure that looks like a bad artifact.

## Swap: a drop-in, never the base unit

The base unit is the historical record; the drop-in is the current selection. Editing the base unit
while a drop-in clears `ExecStart` changes nothing, and the node keeps running the old binary.

```ini
# /etc/systemd/system/<unit>.service.d/90-<purpose>.conf
[Service]
ExecStart=
ExecStart=/usr/local/bin/rnode-<sha8> … --expected-genesis-hash <the pin this network reports>
```

- The bare `ExecStart=` line is required: it clears the inherited command before the new one is set.
- **Keep `--deployer-private-key ${FAUCET_KEY}`** and let `EnvironmentFile=/etc/rnode/faucet.env` in the
  base unit supply it. Inlining the key works — and puts a secret in a second file that nobody rotates.
- The deployer key's address must be one of the **funded addresses in the genesis `wallets.txt`**.
  Nothing else on the chain is funded, and a deploy from an unfunded key is accepted and then fails.
- `chmod 600`, `chown root:root` the drop-in. It need not carry a secret, but it is operator state.
- `systemctl daemon-reload`, then start or restart the unit.

## Verify

```sh
systemctl show <unit> -p ExecStart --value      # the drop-in is what systemd resolved
systemctl is-active <unit>; systemctl show <unit> -p NRestarts --value
curl -s localhost:40403/api/v1/status | jq -r '.version.node, .degraded, .peers'
curl -s localhost:40403/api/last-finalized-block | jq -r '.blockInfo.blockNumber'
grep -c 'receiver state full'      <log>
grep -c 'justification regressed'  <log>
grep -c 'round gate escaped'       <log>
```

An update is complete only when all of these hold:

- the API's build string **equals the staged artifact**, not the previous one;
- `degraded: normal`, and `peers` has returned to the expected count;
- a deploy advances the tip **and** the finalized height;
- the three counts above are unchanged since the restart;
- and every other validator still agrees on height and on the finalized hash.

## Order of operations

**One validator at a time, on a quiet chain, verifying between each.** A validator returning to a
network that is not producing blocks cannot be distinguished from one that failed to return — so the
deploy that proves the update is the one made *after* the update, not the restart itself.

If the update changes genesis identity or consensus validity, **do not use this rolling-swap
procedure**. Follow the separately reviewed network activation/genesis plan and its
quorum, store and binary-compatibility gates; do not assume the transition is reversible.

## Rollback

- **Rename, do not delete, the previous binary.** `/usr/local/bin/rnode-<old-sha8>` stays on disk until a
  restart-under-load cycle has passed on the new build. Only then remove it, so that the base unit's
  stale command cannot be selected by accident later.
- **The store is the authority.** A build refuses a store stamped by a newer marker. An
  upgraded or partially migrated live store must never be passed to the old binary: restore
  the *verified pre-swap snapshot* along with its old executable and exact config/identity,
  or stop the rollout and escalate. Verify the original source-store hashes before restoration;
  do not attempt an unreviewed in-place downgrade.
- With the store intact, reverting the drop-in and `daemon-reload` restores the previous selection
  exactly.

## Traps, each of which has cost a cycle

| trap | symptom | handling |
|---|---|---|
| binary staged under `/root` | `203/EXEC` on start | install to `/usr/local/bin` |
| drop-in present but base unit edited | base names the old build; nothing changes | trust `systemctl show -p ExecStart`, not the file |
| key inlined instead of `${FAUCET_KEY}` | works, but the secret now lives in two files | keep `EnvironmentFile` |
| deployer key not in `wallets.txt` | deploy accepted, then fails | fund the address in the genesis |
| data directory from another shard | `belongs to shard '…', but this node is configured for shard '…'` | move the directory aside; do not reuse it |
| fresh data directory | new node id, joiners' `--bootstrap` stale | copy `node.key.pem` into it |
| restart during a burst | receiver full, dropped blocks, 50/50 split | never update under load |
| genesis pin not regenerated | builds disagree on the post-state hash | regenerate `--expected-genesis-hash`, move together |

## Acceptance is not this page

This procedure makes an update *repeatable*. It does not make it *accepted*: an update still needs the
evidence its own issue asks for — for the 2026-10-30 build, the store-migration and replay gates on
[#520](https://github.com/rhovision-org/rholo/issues/520) and the live arms on
[#522](https://github.com/rhovision-org/rholo/issues/522). A clean swap is a prerequisite, not a verdict.
