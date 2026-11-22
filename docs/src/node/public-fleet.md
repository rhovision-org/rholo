# The public fleet campaign (`rc-public-fleet`)

The release claims have a local column and a public column. The local arms run on the Docker devnet;
the public arms run on four independent hosts. This page is the method for the public column: the
manifest, the driver, the two arms it executes, and the rules that keep a public reading a test
rather than a restatement of an operator's claim.

The campaign runbook is
[`spec/audit/runbooks/rc-public-fleet-20261122.md`](https://github.com/rhovision-org/rholo/blob/dev/spec/audit/runbooks/rc-public-fleet-20261122.md).

## The manifest is an input, not a report

`spec/audit/runbooks/rc-public-fleet.manifest.schema.json` defines the campaign manifest: four nodes,
the candidate pins, one deploy plan, the action policy. `tools/fleet_manifest.py` validates it
before anything runs.

The manifest may **not** carry the outcome. `harness_result`, `victim`, `boot_ids`, `post_heads`,
`certificates_after` and a pre-made `deploy_id` are produced by the driver or read by the claim. A
manifest containing them is refused. This is the difference between "the operator says a validator
was restarted" and "the run stopped a validator and captured the new process boot id".

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
| `traces` | collects the decision traces the reading needs |
| `manifest` | generates the Docker rehearsal manifest |

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
tools/devnet.sh down -v
```

A rehearsal record is marked `rehearsal`, has locality `local-devnet`, reads `not run`, and
`release-check.py record` refuses to pack it. It cannot close a public arm.

## Reading the result

`python3 tools/release-check.py list` shows the derived reading. `safe` closes the arm;
`unsafe-point` names the false predicate and its Rust and Lean anchors; `model-incomplete` names the
trace that is the model gap; `evidence-insufficient` names the missing trace or coordinate;
`precondition` names the run that could not be made. `--assert-ready` exits 3 until every required
arm is `safe`.
