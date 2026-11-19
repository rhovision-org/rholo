# G428-M1 supported-tier measurement — pre-registered protocol

**Status: DRAFT — NOT EXECUTED.** This runbook fixes the protocol before a run so the record cannot
be assembled around the result. It does not claim that any run has happened.

- Gate: `G428-M1` — supported capacity and physical fault domains.
- Evidence destination: `spec/audit/evidence/m1-tier-<date>/`.
- Prerequisite reading: `spec/audit/gate-principles-20261118.md` (`capacity_tier`) and
  `docs/src/node/validator-requirements.md` (the documented per-validator tier).

## 1. What this window must establish

On independently provisioned hosts, at a declared tier: each validator's memory share, CPU share,
free disk, peak RSS (including the restart/replay peak), replay time at the served height, and
OOM/restart state; plus a stake/host map whose declared one-host loss leaves a supermajority under
the code's own rule (`stake * 3 > total * 2`, `sdk/src/consensus.rs`).

The witness is one `observations.json` in the evaluator's shape, produced by
`tools/capacity-tier.py collect` from the arms' recorded JSON and judged by
`gate-check.py --gate G428-M1`. The record's `kind` must be `supported-tier`.

This window does **not** establish independent provisioning (the host ids are the operator's
declaration) and does not run the host-kill arm. It is a measurement window, not M1 acceptance.

## 2. Hard NO-GO conditions

- **One physical host labelled as several.** Four containers on one Docker host are a local
  rehearsal; their host ids are labels, not failure domains. Each declared host must be a distinct
  machine.
- **A run that used `--allow-under-minimum` or `--accept-under-minimum-risk`**, including a HOCON
  setting in a config file. The record's command lines are scanned; a run that needed the override
  is `fault-injection`, not supported-tier evidence.
- **A below-tier injection.** A 1 GiB (or otherwise below the declared per-validator minimum) run
  is fault injection; `n420-soak.py` says so in its own record.
- **A shared host without a per-validator cgroup share.** The documented rule is per validator
  (`validator-requirements.md:23-32`); two validators on one host need the sum, and a shared
  `MemTotal` is not a pass.
- **A missing or unreadable measurement.** No RSS, no replay time, no host core count, an
  unreadable command line, or a container the soak did not sample: the collector refuses and there
  is no record.
- **A declared tier contradicted by the measurement.** A 24 GiB replay peak on an 8 GiB declaration
  is a FAIL, not a rounding.
- **Any gate promotion is out of scope.** None is expected from this run.

## 3. Preflight pins (fill before starting; a missing pin is NO-GO)

| field | value |
|---|---|
| selected source SHA (40-hex) | |
| selected binary SHA-256 (64-hex) | |
| declared tier (`minimal` or `comfortable`) | |
| per-validator declared minima (`cores`, `ram_gib`, `disk_gib`) | |
| host name / machine id, per declared host | |
| validator id → host → container mapping | |
| stake per validator (and the genesis bond source) | |
| served height and the replay budget (seconds) | |
| peak-RSS budget (MiB) | |
| window start / end (UTC) | |
| operator / independent reviewer | |
| evidence destination | `spec/audit/evidence/m1-tier-<date>/` |

No public deployment, real key, transfer or disruptive arm. If the chain is public, the
authorization is recorded here before the run.

## 4. Procedure

1. Write the declaration (`tier.json`, schema 1): the tier, `per_validator`, `max_peak_rss_mb`,
   `max_replay_seconds`, `served_height`, `host_failure_tolerance` and one entry per validator
   with `id`, `host`, `container`, `stake`, `soak` and `restart` paths. Every host id names a
   distinct machine; every stake is the value in the genesis bonds.
2. On each host, start the devnet at the declared tier (the per-validator memory limit, no
   under-minimum override) and run the soak for the pre-registered window:
   ```sh
   DEVNET_NODE_MEMORY=<ram_gib>g tools/devnet.sh up --validators <n> --fresh
   python3 tools/n420-soak.py --out <run-dir>/host-<id>/soak --seconds <window>
   ```
   The soak records the operator half's `rssMb`, `memTotalMb`, `cgroupLimitMb`, `cpuQuota`,
   `dataDirFreeMb` and `rnodeProcesses`, plus each container's command line and `nproc`.
3. On each validator, run the restart/replay arm:
   ```sh
   python3 tools/resource-restart-arm.py --container <container> \
     --out <run-dir>/host-<id>/restart-<validator>
   ```
   It samples the rnode process's `VmRSS` every 2 s during replay and records the peak, the replay
   height and the replay time.
4. Assemble the record:
   ```sh
   python3 tools/capacity-tier.py collect \
     --declaration <run-dir>/tier.json --out <run-dir>/observations.json
   ```
   A refusal is a NO-GO, not a reason to edit the arm output.
5. Write the criterion observation **into the run directory before packing**:
   ```sh
   printf '{"record": {"dir": "%s"}}\n' "<record path>" > raw/observations.json
   ```
   The `record.dir` names the **committed evidence record**, not the scratch run directory: the
   register checker re-evaluates the observation from that path on every check.
6. Pack and verify:
   ```sh
   python3 tools/evidence-pack.py pack --run-dir raw --record <record> \
     --gate-id G428-M1 --verdict INCONCLUSIVE --locality external \
     --source-sha <40hex> --binary-sha256 <64hex>
   python3 tools/evidence-pack.py verify --record <record> --require-pins
   python3 tools/gate-check.py --gate G428-M1 --observations <record>/raw/observations.json
   ```
   `PRECONDITION` means the record cannot key the gate (a fault-injection or unreadable run);
   record it as INCOMPLETE, never repair it by re-running to manufacture a measurement.

## 5. Negative controls (each captured in the same window or a separate labelled record)

- **Below-tier injection.** Repeat a short soak at 1 GiB: the collector classifies it
  `fault-injection` and the criterion returns `PRECONDITION`.
- **Override run.** Start with `--allow-under-minimum`: the collector classifies it
  `fault-injection`.
- **Shared host without a cgroup share.** Declare two validators on one host with no cgroup limit
  and 8 GiB total: the criterion FAILs on the missing per-validator share.
- **Four validators on two hosts.** Declare one-host tolerance: the criterion FAILs the
  supermajority arithmetic.
- **Declared peak contradicted.** Substitute a 24 GiB peak into one validator: the criterion FAILs
  the budget.

A negative control is evidence only in a record that carries it; do not describe a control that was
not captured.

## 6. What this window does not establish

- It is not the host-kill arm (M4's cross-host arm / the M1 host-failure demonstration); the
  declared tolerance is arithmetic over the declared map, not a measured failover.
- It does not verify independent provisioning; the host ids and stakes are the operator's
  declaration and the reviewer's check against the genesis bonds.
- It is not a proof that the run happened: `execution_proven` is false; the digests prove the bytes.
- It does not promote `G428-M1`; the register's witness/control/evidence binding is a separate
  decision after the record is reviewed.
- A sampled peak RSS is a sampled maximum, not an exact high-water mark; free disk and uncapped
  CPU are reported shares, not partitioned allocations.
