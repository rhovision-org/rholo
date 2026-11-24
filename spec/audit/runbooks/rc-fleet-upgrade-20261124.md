# rc-fleet-upgrade — the RC-7/public rolling-swap window

**Status: DRAFT — NOT EXECUTED.** This runbook fixes the protocol before a run so the record cannot
be assembled around the result. It does not claim that any run has happened. The driver
(`tools/fleet-run.py upgrade`) lands in this window's PR; the in-tree readiness tranche adds the
typed `upgrade` manifest block, this runbook and the record shape.

- Arm: `RC-7/public` (rolling fleet swap keeps one finalised chain through the mixed window).
- Locality: `public-live`; the four-host public fleet of `rc-public-fleet` is the topology.
- Action PR: `rc-fleet-upgrade`.
- Evidence destination: `spec/audit/evidence/rc7-public-<date>-<sha7>/`.

## 1. What this window must establish

A mixed-version fleet either keeps one finalised chain and one state for a block produced during
the window, or refuses on the consensus path. After the swap the upgraded node reaches the same
finalised head. The populated-store migration drill passes, and a build refuses a store whose
format marker is higher than the one it writes.

## 2. Inputs (typed, never the outcome)

The manifest carries an `upgrade` block:

```json
{
  "old": {"image": "rnode:<old-tag>", "source_sha": "<40hex>",
          "binary_sha256": "<64hex>"},
  "new_binary_path": "/usr/local/bin/rnode-new",
  "old_binary_path": "/usr/local/bin/rnode-old",
  "order": ["validator-3", "validator-2", "validator-1", "validator-0"],
  "staging_host": "<staging-host>",
  "mixed_window_blocks": 3,
  "rollback_on_refusal": true,
  "timeout_s": 900
}
```

`order` is a permutation of the four node names. The old and new binary digests must differ. The
driver records the observed digest before and after each swap; the manifest never carries
`upgrade_record`, `mixed`, `migration_result`, `downgrade` or `rolled_back`.

## 3. Hard NO-GO conditions

- The old and new binary digests are equal, or `provenance.binary_sha256` is not the new digest.
- No block is finalised on all four validators during the mixed window.
- Two versions finalise different blocks or derive different post-state hashes without a
  consensus-path refusal.
- The old binary runs on a store written by the newer format.
- The populated-store migration drill fails.
- The roll is reported as a consensus-path refusal when it was a rollback.
- Any finalised height regresses or changes hash.

## 4. Procedure

1. Pin the candidate: source SHA, new binary SHA-256, genesis hash, bonds digest, network/shard,
   per-host `release-pin-probe.py observe` and a merged, checked preflight.
2. Record every node's observed `rnode --version` and binary digest; require the current digest to
   equal `provenance.binary_sha256` before any swap.
3. Stage the old and new binaries at the declared paths on every host. Do not overwrite the running
   binary; the swap is an explicit step.
4. For each node in `order`: stop the unit, swap the binary, start the unit, verify the served
   digest and `rnode --version`, and wait for the node to rejoin the common finalised head.
5. After the first swap, wait for a block finalised on all four; read its `preStateHash`,
   `postStateHash` and `rejectedDeploys` on an old node and a new node. Record `agreement: true`, or
   capture the old node's consensus-path refusal from its journal with the exact line.
6. After the last swap, require all four on the new digest and a common finalised head; read the
   manifest marker at that head.
7. On the staging host, run the populated-store migration drill
   (`tools/populated-store-migration-drill.py`) and the newer-store refusal; a missing staging host
   or Docker is a precondition, never a pass.
8. Assemble the typed `upgrade_record` from the driver result, then:
   ```sh
   python3 tools/release-check.py run --claim RC-7 --env public --public-manifest <manifest> \
     --out target/rc-public-fleet/RC-7
   python3 tools/release-check.py record --claim RC-7 --arm public \
     --record target/rc-public-fleet/RC-7
   python3 tools/evidence-pack.py verify --record <record> --require-pins
   python3 tools/check-evidence-locality.py --gate
   ```

## 5. Negative controls

- **Same digest:** the driver refuses the run (precondition).
- **No mixed block:** the arm fails; do not substitute a fresh-genesis run.
- **A changed finalised head:** the arm fails.
- **A rollback:** recorded as `rolled_back: true` and the arm fails; it is not a refusal.
- **A legacy certificate with no commitments:** refused by the normal certificate path.

## 6. What this window does not establish

- It does not claim a rolling replacement is an accepted procedure for unswept history; the
  compatibility/genesis boundary remains a release-owner decision.
- It does not establish the historical M5 replay or any M6 sign-off.
- `execution_proven` is false; the digests prove the bytes, not the run.
