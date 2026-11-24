# The public fleet campaign (`rc-public-fleet`)

The release claims have a local column and a public column. The local arms run on the Docker devnet;
the public arms run on four independent hosts. This page is the method for the public column: the
manifest, the driver, the arms it executes, and the rules that keep a public reading a test rather
than a restatement of an operator's claim.

The campaign runbook is
[`spec/audit/runbooks/rc-public-fleet-20261122.md`](https://github.com/rhovision-org/rholo/blob/dev/spec/audit/runbooks/rc-public-fleet-20261122.md).

## The manifest is an input, not a report

`spec/audit/runbooks/rc-public-fleet.manifest.schema.json` defines the campaign manifest: four nodes,
the candidate pins, one deploy plan, the action policy, and the typed `fault`, `availability`,
`pressure`, `join` and `upgrade` inputs the drivers consume. `tools/fleet_manifest.py` validates it
before anything runs.

The manifest may **not** carry the outcome. `harness_result`, `cycles`, `victim`, `boot_ids`,
`post_heads`, `certificates_after`, `pressure_record`, `join_result`, `audit_bootstrap`,
`upgrade_record` and a pre-made `deploy_id` are produced by the driver or read by the claim. A
manifest containing them is refused by name. This is the difference between "the operator says a
validator was restarted" and "the run stopped a validator and captured the new process boot id".

A public manifest also has to name four distinct, non-loopback, non-private hosts, and the observed
kernel boot ids must be four distinct values. Two hosts with two validators each are one failure
domain, not four.

## The driver

`tools/fleet-run.py` has two transports:

- `ssh-systemd` — the public fleet. HTTP reads are direct where the endpoint is public; admin reads
  are proxied over SSH, so a loopback-only admin port is not an assumption.
- `docker` — the local four-validator rehearsal. The same actions against a devnet.

Actions:

| action | what it does |
|---|---|
| `validate` | checks the manifest against the contract |
| `observe` | reads every node's candidate pins, genesis/bonds digests, process boot id, unit state and readiness; refuses on disagreement |
| `deploy` | submits the signed deploy from the manifest's ingress, then requires **every** validator to admit it |
| `recovery` | baseline → marker deploy → `includedUnfinalized` → choose victim → stop → survivors finalise past the kill → second marker settles → start → new process boot id → rejoin the common finalised head ≥ baseline + 3 → health counters and traces |
| `partition` | installs the declared partition on two rotated non-bootstrap victims, measures whether the victim keeps proposing while finalised is frozen, removes the partition, and records the cycles, anchor retention and finality traces |
| `availability` | samples at least ten finalised blocks and reads every body and state on every validator; `--join-artifact` adds RC-6's non-producer join witness |
| `pressure` | runs the capped/unset `m2-pressure-arm.py` lifetimes against the public endpoint, assembles the pressure record and restores the unit environment |
| `join` | starts a fresh fifth joiner from a non-producer, captures both logs, builds the six-stage chain, runs `rnode audit` and records the verdict |
| `traces` | collects the decision traces the reading needs |
| `manifest` | generates the Docker rehearsal manifest (with the typed block for `--claim`) |

`upgrade` (RC-7/public) lands in the separate `rc-fleet-upgrade` window; the typed manifest block
and the pre-registered runbook are already in the tree. Every action writes `fleet-result.json` at
the record root, which is the driver's own account that the locality gate reads.

The restart witness is `/health.bootId` — the node process's boot id. The kernel boot id
(`/proc/sys/kernel/random/boot_id`) does not change on `systemctl restart`; it establishes host
distinctness only.

## RC-1/public — truthful settlement

The claim submits one signed native-REV transfer with a marker through the driver. It first probes
that the marker is absent at the baseline finalised head, then reads the settlement route on every
validator, the finalised head on every validator, the marker at each head, and the per-block
`rejectedDeploys` after inclusion. Retention is sampled at two further epoch boundaries; the epoch
length is declared in the manifest. The record carries the merge traces; without them the reading is
`evidence-insufficient`, not `safe`.

## RC-4/public — recovery without loss

The claim asks the driver to perform the recovery. The driver chooses the victim after the marker
reaches `includedUnfinalized` — never the bootstrap, never the deploy ingress — stops the unit,
waits for the survivors to finalise past the kill and for a second deploy to settle, starts the
unit, and waits for a **new process boot id** and for the victim to report the common finalised head
at least three heights above the baseline. It records the stop/start commands, the boot ids, the
health counters (`proposalHalted`, `selfValidationRefusal`), the checkpoint certificates and the
victim's boot trace. A run without the trace reads `evidence-insufficient`.

## RC-2/public — safety under partition

The `fault` block names the policy, the p2p ports, the dwell and the mechanism; it may not name the
victim. The driver installs an nftables table (or a docker network disconnect in rehearsal) on a
rotated non-bootstrap victim, measures whether the victim keeps proposing while its finalised fence
is frozen, waits for the survivors to finalise past the anchor, removes the table in a `finally`,
and waits for the victim to rejoin. The claim then re-reads containment and regression itself. A
leaked partition or an unremoved table never passes.

## RC-3/public and RC-3/availability — state agreement and availability

RC-3/public uses the manifest's one deploy plan; the claim reads the per-block pre/post state and
rejected-deploy ledger on every validator and the kept marker. RC-3/availability samples at least
ten finalised heights and requires the raw body and a state query on every validator; a finalised
block that one validator cannot serve is `not_available` and fails the arm.

## RC-5/public — bounded ingress

The `pressure` block names the endpoint, the declared unit, the cap and the mode order. The driver
sets and removes `RNODE_HTTP_MAX_INFLIGHT_DEPLOY`, restarts the unit for each lifetime, captures the
unit environment and version, and runs the existing `m2-pressure-arm.py` phases and gRPC
observation. The claim re-runs `m2-pressure.py check`; the driver never supplies the verdict. A
local endpoint is refused as a precondition, and the proxy surface remains deferred.

## RC-6/public and RC-6/availability — join and audit

The `join` block names a fifth joiner host, a non-producer bootstrap, a fresh store and an audit
configuration. The driver checks the store is empty (it never deletes one), starts the joiner,
captures both logs, builds the six-stage chain, reads the operator checkpoint and runs `rnode
audit`. The availability arm runs the join first and tags the sweep with the join witness; the RC-6
predicate requires it, so the arm cannot close on an availability sweep alone.

## Launch gate

`tools/check-launch-gate.py` is the aggregate gate. `--gate` requires one dated disposition for
every open external row in `mainnet-gates.external.json`; `--assert-ready` is stricter — it exits 0
only when every claim arm is `safe` and every external row is `met` or `superseded`. A disposition
records an owner decision; it is not a waiver.

## Rehearsal

The same public path runs against four local validators:

```sh
DEVNET_NODE_MEMORY=6g DEVNET_NODE_ENV=RNODE_TRACE_DIR=/var/lib/rnode/traces \
  tools/devnet.sh up --validators 4 --fresh
python3 tools/fleet-run.py manifest --prefix rcfleet --transport docker \
  --out target/rc-public-fleet/rehearsal/manifest.json
python3 tools/release-check.py run --claim RC-1 --env public --rehearse \
  --public-manifest target/rc-public-fleet/rehearsal/manifest.json \
  --out target/rc-public-fleet/RC-1
python3 tools/release-check.py run --claim RC-4 --env public --rehearse \
  --public-manifest target/rc-public-fleet/rehearsal/manifest.json \
  --out target/rc-public-fleet/RC-4
python3 tools/fleet-run.py manifest --prefix rcfleet --claim RC-2 --transport docker \
  --out target/rc-public-fleet/rehearsal/manifest-rc2.json
python3 tools/release-check.py run --claim RC-2 --env public --rehearse \
  --public-manifest target/rc-public-fleet/rehearsal/manifest-rc2.json \
  --out target/rc-public-fleet/RC-2
python3 tools/fleet-run.py manifest --prefix rcfleet --claim RC-3 --transport docker \
  --out target/rc-public-fleet/rehearsal/manifest-rc3.json
python3 tools/release-check.py run --claim RC-3 --arm availability --env public --rehearse \
  --public-manifest target/rc-public-fleet/rehearsal/manifest-rc3.json \
  --out target/rc-public-fleet/RC-3-availability
tools/devnet.sh down -v
```

A rehearsal record is marked `rehearsal`, has locality `local-devnet`, reads `not run`, and
`release-check.py record` refuses to pack it. It cannot close a public arm. The join rehearsal needs
a fifth container and is pre-registered in window E; it is not improvised at run time.

## Reading the result

`python3 tools/release-check.py list` shows the derived reading. `safe` closes the arm;
`unsafe-point` names the false predicate and its Rust and Lean anchors; `model-incomplete` names the
trace that is the model gap; `evidence-insufficient` names the missing trace or coordinate;
`precondition` names the run that could not be made. `--assert-ready` exits 3 until every required
arm is `safe`.
