# G428-M4 multi-host finality campaign — pre-registered protocol

**Status: DRAFT — NOT EXECUTED.** This runbook fixes the protocol before a run so the record
cannot be assembled around the result. It does not claim that any run has happened.

- Gate: `G428-M4` — finality, liveness and autonomous recovery.
- Evidence destination: `spec/audit/evidence/m4-multihost-<date>/`.
- Declaration template: `spec/audit/runbooks/m4-campaign-declaration.template.json`.
- Prerequisite reading: `spec/audit/gate-principles-20261118.md` (`finality_campaign`),
  `tools/m4-campaign.py`, `tests/consensus-partition-rejoin.py`, and the M1 runbook.

## 1. What this window must establish

Named, independently rerunnable N=4 and N=8 workloads on declared independent hosts, at a declared
M1 capacity tier: deploy finalization, partition/rejoin with rotated non-bootstrap victims, host
loss within the declared tolerance, restart/replay, and safe recovery, with the approved root
retained throughout. The record is judged by `gate-check.py --gate G428-M4`; its locality is
`multi-host`.

The approved root is a **pre-registered finalized block hash**, not a state root. Every root read
is a measured `/api/is-finalized/<hash>` answer; the record's non-claims say so.

## 2. Hard NO-GO conditions

- **One physical host labelled as several.** Four Docker containers on one machine are a local
  rehearsal. Each declared host must be a distinct machine.
- **A below-tier or override capacity run.** Each workload's capacity record is the M1 record and
  must be `supported-tier`; a below-tier or `--allow-under-minimum` run is unkeyable.
- **A missing arm.** Deploy finalization, partition/rejoin, host loss, restart/replay and safe
  recovery are all required; a missing arm is a FAIL, not a skip.
- **An approved root that was not pre-registered**, or a root read that is false or unequal.
- **N=4 or N=8 missing**, or a candidate that differs between the workloads.
- **A single-runner CI record labelled multi-host.** The existing harness is guarded to a
  disposable GitHub-hosted runner and one Docker network; a multi-host topology is required.
- **Lost accepted work, a spurious self-replay, or an own-key halt** on a healthy key.
- **M3-11 failover bundled into this window.** It is a separate runtime change.
- **Any gate promotion is out of scope.** None is expected from this run.

## 3. Preflight pins (fill before starting; a missing pin is NO-GO)

| field | value |
|---|---|
| selected source SHA (40-hex) | |
| selected binary SHA-256 (64-hex) | |
| declared hosts and machine ids | |
| bootstrap validator id (node 0) | |
| per-workload node-index → validator id map | |
| per-workload approved root (64-hex finalized block hash) | |
| per-workload capacity declaration (tier, minima, budgets, stake/host map) | |
| per-workload host to be lost (within the declared tolerance) | |
| rerun commands for N=4 and N=8 | |
| window start / end (UTC) | |
| operator / independent reviewer | |
| evidence destination | `spec/audit/evidence/m4-multihost-<date>/` |

No public deployment, real key or transfer is required for the partition/rejoin and host-loss
arms. If the candidate is on a public chain, the authorization is recorded here before the run.

## 4. Procedure

1. Provision the declared hosts and start the selected candidate on each workload's validators.
2. Produce each workload's capacity record with the M1 tools:
   ```sh
   python3 tools/capacity-tier.py collect --declaration <run>/<n>/tier.json \
     --out <run>/<n>/capacity/observations.json
   ```
3. Run the partition/rejoin workload on the multi-host topology. The harness writes
   `campaign.json`, `partition-result.json`, `partition-observations.json` and the typed
   `partition-rejoin.json` (cycle, victim index, anchor, dwell, window, per-validator retention).
   A run that does not write the typed file is not this campaign's evidence.
4. Read the approved root on every validator **before** the faults, and on the survivors after:
   ```sh
   python3 tools/m4-campaign.py observe-root --approved-root <64hex> --phase before \
     --validator v1=https://host-a --validator v2=https://host-b ... --out <run>/<n>/root-before.json
   ```
5. Stop the declared host, then:
   ```sh
   python3 tools/m4-campaign.py observe-host-loss --approved-root <64hex> \
     --lost host-d=https://host-d --survivor v1=https://host-a ... \
     --out <run>/<n>/host-loss.json
   python3 tools/m4-campaign.py observe-root --approved-root <64hex> --phase after \
     --validator v1=https://host-a ... --out <run>/<n>/root-after.json
   ```
   A lost endpoint that answers refuses the observation.
6. Run the restart/replay arm on a survivor:
   ```sh
   python3 tools/resource-restart-arm.py --container <container> --out <run>/<n>/restart
   ```
7. Run the chosen recovery arm and record the G4-shaped `recovery.json` (arm, window, accepted
   deploys before/after, `lost_deploys`, halt/self-replay flags, commands).
8. Write the declaration from the template and assemble:
   ```sh
   python3 tools/m4-campaign.py collect --declaration <run>/campaign.json \
     --out <record>/raw/observations.json
   ```
   A refusal is a NO-GO, not a reason to edit the harness output.
9. Judge and pack:
   ```sh
   python3 tools/gate-check.py --gate G428-M4 --observations <record>/raw/observations.json
   python3 tools/evidence-pack.py pack --run-dir <record>/raw --record <record> \
     --gate-id G428-M4 --verdict INCONCLUSIVE --locality external \
     --source-sha <40hex> --binary-sha256 <64hex>
   python3 tools/evidence-pack.py verify --record <record> --require-pins
   ```

## 5. Negative controls (each captured in a labelled record)

- **A single declared host.** `collect` refuses a capacity host outside `campaign.hosts`; the
  criterion FAILs the `multi-host` locality and the host floor.
- **N=4 only.** The criterion FAILs the missing workload.
- **A missing arm.** The criterion FAILs the absent arm.
- **A victim not rotated, or the bootstrap as victim.** The criterion FAILs.
- **A changed approved root.** The criterion FAILs the root equality.
- **A below-tier capacity record.** The criterion FAILs `capacity_tier`.
- **More lost hosts than the declared tolerance.** `collect` refuses the host-loss observation.

## 6. What this window does not establish

- **The multi-host driver is still a separate code slice.** The existing
  `tests/consensus-partition-rejoin.py` is guarded to one disposable GitHub-hosted runner; the
  criterion, collator and this runbook make a submitted multi-host campaign judgeable, but the
  operator must supply the multi-host topology and its driver.
- Host independence and the host stop are operator facts; distinct host ids are a declaration.
- `execution_proven` is false; the digests prove the bytes, not the run.
- The approved root is a finalized block hash; a state-root definition is a follow-up.
- It does not cover failover (M3-11), and it does not promote `G428-M4`.
