# rc-public-fleet — the four-host public campaign

**Status:** pre-registered 2026-11-22. The live window has not been executed. The local
four-validator dress rehearsal is the pre-window test of the driver and the predicates; it is not
evidence and cannot close an arm.

**Rehearsal record (2026-11-22).** Four validators at 6 GiB with `RNODE_TRACE_DIR` set, fresh
genesis. Both public claim paths ran through `tools/fleet-run.py`: RC-1 submitted its own deploy and
sampled retention at two epoch boundaries; RC-4 chose validator-3, stopped it, waited for the
survivors to finalise past the kill and a survivor deploy to settle, restarted it, captured a new
process boot id and a rejoin three heights above the baseline. Both driver runs read `PASS`; both
records read `not run` because they are rehearsals. Read without the rehearsal marker, both
predicates pass. The rehearsal found and fixed four collector defects: the deploy-status enum was
not decoded, boot traces were pushed out of the record by merge history, the victim's boot trace was
not filtered by kind, and a single finalised-head snapshot raced a producing chain.

**Arms this window closes:** **RC-1/public** (truthful settlement) and **RC-4/public** (recovery
without loss). Every other public arm keeps `rc-public-fleet` as its next PR and is not claimed
here.

**Windows B–E (2026-11-24).** The same fleet and genesis carry the remaining public arms in four
separate pre-registered windows: B = RC-2/public partition, C = RC-3/public + RC-3/availability,
D = RC-5/public pressure, E = RC-6/public + RC-6/availability. Each has its own typed manifest
input and its own record. The rolling swap (RC-7/public) is the separate `rc-fleet-upgrade` window
and is not run here.

**What the window also records:** the source set and the N1/N4/N8 differential requested in review
of #642. Those are diagnostics; only the current candidate's public arms are claims.

## Decisions

- Four independent hosts, one validator each, on a **coordinated new genesis**.
- One signed native-REV transfer with a marker, submitted by the claim through
  `tools/fleet-run.py`. No operator-supplied deploy id.
- One single-validator stop during `includedUnfinalized`, chosen by the driver after inclusion and
  performed by the driver. No operator-supplied harness result, victim or boot ids.
- The candidate is the merged campaign PR; `source_sha == HEAD` at run time. A previous build's
  pins, a previous window's samples or an attestation do not satisfy a `not-observed` arm.

## Topology (fill from the pre-window inventory)

| node | declared host | ssh | unit | http | admin | data dir |
|---|---|---|---|---|---|---|
| validator-0 (ceremony) | | | | | | |
| validator-1 | | | | | | |
| validator-2 | | | | | | |
| validator-3 | | | | | | |

The four declared hosts must be distinct and non-loopback/non-private. Distinctness is also checked
from the observed kernel boot ids: four containers or two hosts with two validators each do not
satisfy the arm.

## Genesis

1. On validator-0 run the standalone ceremony with four bonds and the campaign wallet:
   `rnode run -s --bonds-file … --wallets-file … --validator-private-key-path …` plus the
   campaign's epoch/bond/shard parameters. Every other node starts with the same inputs minus `-s`
   and with `--bootstrap rnode://<id>@<validator-0>?protocol=40400&discovery=40404`.
2. Record, from the run's own output: the genesis block hash, the raw genesis digest, the canonical
   bonds digest, and the four node ids.
3. Enable decision traces on every unit:
   `Environment=RNODE_TRACE_DIR=/var/lib/rnode/traces` (systemd drop-in). Without it the run reads
   `evidence-insufficient`, not `safe`.

## Preflight and manifest

Collect the #559 §1 pins on each host and merge them:

```sh
tools/release-pin-probe.py observe --out target/rc-public-fleet/pins --name validator-N \
  --http http://127.0.0.1:40403 --admin http://127.0.0.1:40405 \
  --local --service rnode --binary /usr/local/bin/rnode --data-dir /var/lib/rnode
tools/release-pin-probe.py merge --out target/rc-public-fleet/pins --expect 4 --check \
  --operator-json <private> --redact
```

Write the campaign manifest from `spec/audit/runbooks/rc-public-fleet.manifest.schema.json`. It is
**input-only**; a manifest carrying `deploy.deploy_id`, `harness_result`, `victim`, `boot_ids`,
`post_heads` or `certificates_after` is refused.

```json
{
  "schema": 1, "campaign": "rc-public-fleet", "generated_at_utc": "<utc>",
  "locality": "public-live", "transport": "ssh-systemd",
  "provenance": {"source_sha": "<40-hex>", "binary_sha256": "<64-hex>",
                 "genesis_hash": "<64-hex>", "genesis_raw_sha256": "<64-hex>",
                 "bonds_sha256": "<64-hex>", "network_id": "<id>", "shard_id": "/<shard>"},
  "nodes": [
    {"name": "validator-0", "host": "<declared label>", "ssh": "root@<host>",
     "unit": "rnode", "binary_path": "/usr/local/bin/rnode", "data_dir": "/var/lib/rnode",
     "http": "https://<host>", "admin": "http://127.0.0.1:40405",
     "grpc_host": "127.0.0.1", "grpc_port": 40401}
  ],
  "deploy": {"from": "validator-0", "marker": "<marker>", "value": 42,
             "term_file": "target/rc-public-fleet/term.rho", "term_sha256": "<64-hex>",
             "phlo_limit": 1000000, "phlo_price": 1, "shard_id": "/<shard>",
             "private_key_path": "/etc/rnode/deploy.key",
             "relay": {"mode": "relay", "aliases": ["validator-0","validator-1","validator-2","validator-3"],
                       "reachability_checked_utc": "<utc>", "unreachable_aliases": []}},
  "actions": {"survivor_advance_blocks": 3, "rejoin_margin_blocks": 3,
              "epoch_length": <genesis epoch length>, "retention_epochs": 2}
}
```

`nodes` must contain all four entries. Validate and observe before any deploy:

```sh
python3 tools/fleet-run.py validate --manifest <manifest> --claim RC-1 --head <sha>
python3 tools/fleet-run.py observe  --manifest <manifest> --claim RC-1 --out target/rc-public-fleet/observe
```

`observe` refuses on a pin mismatch, a prior source, a missing process boot id, fewer than four
distinct hosts, or a node that is not ready. Its `fleet-result.json` is kept with the campaign.

## The run

RC-1/public submits its own deploy; the retention reading is sampled at two further epoch
boundaries.

```sh
python3 tools/release-check.py run --claim RC-1 --env public --public-manifest <manifest>
```

RC-4/public deploys a marker, waits for `includedUnfinalized`, chooses the victim, stops it, waits
for the survivors to finalise past the kill, starts it, and waits for the victim to rejoin the common
finalised head at least three heights above the baseline.

```sh
python3 tools/release-check.py run --claim RC-4 --env public --public-manifest <manifest>
```

Pack both records:

```sh
python3 tools/release-check.py record --claim RC-1 --record target/release-claims/RC-1/public
python3 tools/release-check.py record --claim RC-4 --record target/release-claims/RC-4/public
```

Fill the README's non-claims, declare **`public-live`** as the locality, and verify:

```sh
python3 tools/evidence-pack.py verify --record <record> --require-pins
python3 tools/check-evidence-locality.py --gate
```

## Windows B–E — the remaining public arms

The fleet is the same four hosts and the same coordinated genesis. Each window is separately
pre-registered and separately packed; a window that fails is recorded, not erased. **A window never
reads its result from the manifest.** The typed input block names the fault or the policy; the
driver chooses the victim, captures the witness and writes `fleet-result.json` at the record root.

### Window B — RC-2/public partition

Input: a `fault` block with `kind: partition`, `cycles`, `victim_policy`, `p2p_ports`,
`mechanism: nftables`, `dwell_blocks`, `dwell_seconds`, `heal_timeout_s`. It may not name the
victim.

```sh
python3 tools/release-check.py run --claim RC-2 --env public --public-manifest <manifest> \
  --out target/rc-public-fleet/RC-2
python3 tools/release-check.py record --claim RC-2 --arm public \
  --record target/rc-public-fleet/RC-2
```

The driver installs an nftables table that drops the other three peers' p2p ports on the chosen
victim, rotates the victim across the declared cycles, measures whether the victim keeps proposing
while its finalised fence is frozen, waits for the survivors to finalise past the anchor, removes
the table in a `finally`, and waits for the victim to rejoin. A removal failure is exit 1 and never
PASS. The claim then re-reads containment and regression itself.

NO-GO: a leaked partition (the victim's finalised height advances), a non-producing victim, an
unremoved table, or fewer than two cycles. Rollback: `nft delete table inet rc_partition_<pid>_<i>`
on the victim.

### Window C — RC-3/public and RC-3/availability

The `deploy` block is the manifest's one deploy plan. The driver submits it; the claim reads the
state triples and the kept marker. The availability input is `availability: {window_blocks >= 10,
probe_timeout_s}`.

```sh
python3 tools/release-check.py run --claim RC-3 --env public --public-manifest <manifest> \
  --out target/rc-public-fleet/RC-3
python3 tools/release-check.py run --claim RC-3 --arm availability --env public \
  --public-manifest <manifest> --out target/rc-public-fleet/RC-3-availability
python3 tools/release-check.py record --claim RC-3 --arm public --record target/rc-public-fleet/RC-3
python3 tools/release-check.py record --claim RC-3 --arm availability \
  --record target/rc-public-fleet/RC-3-availability
```

The availability driver samples at least ten finalised heights, requires each hash finalised on all
four validators, reads the raw body and a state query on every validator, and records
`not_available` for any miss. NO-GO: fewer than ten finalised blocks, a block not finalised by all
four, or any finalised body/state unavailable.

### Window D — RC-5/public pressure

Input: `pressure: {endpoint, host, port, cap, probes, mode_order, settle_s}`. Use a dedicated
non-validator RPC host so the capped/unset unit restarts do not disturb consensus. The endpoint
must be a non-local plain-HTTP listener; the proxy surface remains deferred and is not claimed.

```sh
python3 tools/release-check.py run --claim RC-5 --env public --public-manifest <manifest> \
  --out target/rc-public-fleet/RC-5
python3 tools/release-check.py record --claim RC-5 --arm public --record target/rc-public-fleet/RC-5
```

The driver sets/removes `RNODE_HTTP_MAX_INFLIGHT_DEPLOY`, restarts the declared unit for each
lifetime, captures the unit environment and version, runs `tools/m2-pressure-arm.py phase` for
capped and unset, runs its gRPC observation, assembles the record, and restores the original unit
environment in a `finally`. This needs a systemd unit, so it is SSH-only; the Docker transport
cannot mutate a running container's environment and is a precondition, not a rehearsal path. The
claim re-runs `tools/m2-pressure.py check`. NO-GO: a local endpoint, a 5xx, an accepted deploy that
later disappears, or a unit environment not restored.

### Window E — RC-6/public and RC-6/availability

Input: `join: {joiner, bootstrap_from, fresh_store, min_height, timeout_s, audit_config}`. The
joiner is a fifth host, not one of the four validators; `bootstrap_from` is a non-producer.

```sh
python3 tools/release-check.py run --claim RC-6 --env public --public-manifest <manifest> \
  --out target/rc-public-fleet/RC-6
python3 tools/release-check.py run --claim RC-6 --arm availability --env public \
  --public-manifest <manifest> --out target/rc-public-fleet/RC-6-availability
python3 tools/release-check.py record --claim RC-6 --arm public --record target/rc-public-fleet/RC-6
python3 tools/release-check.py record --claim RC-6 --arm availability \
  --record target/rc-public-fleet/RC-6-availability
```

The join driver verifies the joiner store is empty, starts the joiner, captures both logs, builds
the six-stage chain with `tools/m3-join-chain.py build`, reads the operator checkpoint, runs
`rnode audit` on the joiner and records the audit verdict and traces. The availability arm runs the
join first and then the sweep, so its observation carries the non-producer join witness the RC-6
predicate now requires. NO-GO: a store deleted to manufacture `fresh_store`, a loopback/private
bootstrap, an incomplete chain, or an audit that is not AGREE.

## Reading the result

| reading | meaning | next action |
|---|---|---|
| `safe` | every predicate holds on the recorded run | the arm closes |
| `unsafe-point` | a named trace predicate is false | fix the Rust anchor and the Lean law it violates |
| `model-incomplete` | every predicate holds and the outcome is still wrong | add the missing trace coordinate first |
| `evidence-insufficient` | a trace or coordinate is missing | fix the collector; do not infer a cause |
| `precondition` | the run could not be made | name and supply the coordinate |
| `not-observed` | no committed record | run the window |

## Differential annex (diagnostic, not a claim)

Record each source's build pin and, on the matched topology and workload, the N1/N4/N8 readings:

- baseline A `3b6d92d6…`, baseline B `faec64ce…`, integration `2770ef69…`, current `dev`, #642
  `57f35ca48`;
- no cross-used stores; a fresh genesis per run;
- failed attempts are recorded, not discarded;
- when a baseline passes and the candidate fails, bisect the first offending commit and name it.

The annex answers the review; it cannot close an arm.

## NO-GO

- Fewer than four distinct public hosts, or a loopback/private declared host.
- A manifest from another revision, or a binary whose observed commit/digest differs.
- A deploy that never reaches every validator's pool.
- A missing trace directory, or a run record with no `fleet-result.json`.
- Any operator-supplied result field in the manifest.
- A rehearsal record presented as public evidence.

## Non-claims

- The local rehearsal is not public evidence.
- This window does not claim RC-2/3/5/6/7 public arms; their faults are separate campaigns.
- A `safe` reading is a finite witness of one run on one candidate, not a universal safety or
  liveness theorem.
