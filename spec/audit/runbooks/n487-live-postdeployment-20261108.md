# #487 / #584 — pre-registered public four-validator acceptance runbook

> **Historical / superseded (2026-11-22).** This runbook's G6 closing mechanism — a named independent
> reviewer's verdict against four-host pins and a UTC window — is **retired as a release condition**.
> The live verdict is now the release-claim reading: **RC-1/public** (truthful settlement) and
> **RC-4/public** (recovery without loss), derived by `tools/release-check.py` and enforced by
> `release-check.py --assert-ready`. No prior-test provenance satisfies a `not-observed` arm. The
> runbook is retained as the historical observation protocol and as diagnostic guidance for the
> `tools/live-gate.py` sampler; it does not close #584 or authorize a release. See
> `spec/audit/release-claims.json` and `spec/audit/release-campaigns.md`.

**Status:** DRAFT / NOT EXECUTED. Refreshed 2026-11-18 onto `dev@77953df4f` after the #611 local
rehearsal: the collector now carries and requires the selected-candidate pins, the deploy
distribution is a preflight pin, and the sampling cadence is 1 s. The scoring criteria and arms are
unchanged; candidate/approval pins remain unpopulated. Sections 1.1–1.2 make collection, independent
checks and the maintainer handoff explicit. This document is a proposed acceptance protocol, not
authorization to touch validators, submit transactions, restart nodes or claim a live result.

**Decision owner:** Jim + Patrick (maintainer go/no-go and final verdict); operator and independent evidence reviewer must be identified before execution. **Scope owner:** Aria (pre-registration/evidence design). Approval of this document alone does not authorize execution.

**Parent:** [#584](https://github.com/rhovision-org/rholo/issues/584), successor to closed #553; it carries the live #487 verdict as the frozen G1–G6 criteria. **Local comparator:** [n487-admission-finality-stall-n4-20261102](../evidence/n487-admission-finality-stall-n4-20261102/README.md). **Already satisfied separately:** the scoped #485/#492 historical differential, [merged #557](https://github.com/rhovision-org/rholo/pull/557) at `f5ec8e2b` (a historical, scoped record, not acceptance of the current public deployment). This runbook **does not reopen #485**.

## 0. Acceptance contract — freeze before the window

The operator must obtain explicit maintainer approval for a **named window** and the exact **signed, low-value native-REV test transfer** (source, destination, maximum amount/fee, responsible owner and test wallet authorization) before any write. No production secrets, private keys, signing material or validator control endpoints belong in Git. The public testnet, not a disposable DevNet, is the scope.

The existing option-2 contract in [admission.md](../../../docs/src/node/admission.md#finality-is-not-admission) is authoritative:

- `acceptsDeploys` is **local pool capacity**, not network finality, health quorum or settlement.
- Deploy **accepted** means the request was admitted; **pooled** means held but not executed; `ProcessedWithSuccess` / `ProcessedWithError` indicates execution in a block, **not finality**.
- Only `GET /api/v1/deploy-settlement/{id}` returning **`stage: "finalizedSuccess"` AND `settlementConfirmed: true`** for the actual **inclusion block** is a positive settlement result; cross-check finalized inclusion by block hash/height and independent validators. Other stages, especially `includedUnfinalized` and `finalityUnknown`, must never be advertised as settled.
- A single node's `GET /api/v1/finality-observation` always has `availability: "unknown"`; its numerical lag is an observation, not a quorum oracle.
- An unresolved, stale or partitioned view must yield unknown/unfinalized; no client may infer settlement from a local tip balance.

This protocol measures the existing behavior. It does **not** redefine admission policy or introduce a consensus or protocol change.

## 1. Immutable preflight pins (required, not populated by this draft)

Create a **signed or maintainer-reviewed, time-stamped preflight manifest** with all fields completed and committed/attached to the resulting evidence record *before* submitting the test deploy. Missing or mismatching pins are **NO-GO**; never infer them from an earlier DevNet run.

| Required field | Pre-registration requirement |
|---|---|
| Approved UTC start/end; operator, observers and rollback owner | Names and approval references, prior to test |
| Release source `SHA`, release/tag and branch | Exact approved commit (`selected_source_sha`, 40-hex) installed on all four public validators; not merely a green PR |
| Release build provenance | CI URL/attempt, artifact SHA-256 (`selected_artifact_digest`), executable `rnode` SHA-256 (`selected_binary_sha256`), `--version`, packaging/container digest if applicable |
| Four validator identities | Stable ID/alias, bonded stake, host/instance identifier, binary SHA, kernel `boot_id` (`boot_id_source: kernel`), relevant API addresses retained **privately** |
| Chain identity | Network/shard, genesis hash **and** genesis/bonds file digests, live bond set and total weight; verify same network/genesis on all four |
| Current deployment state | Per-node software version, online state, height, finalized height/hash, observed lag, `lastFinalityAgeMs`, `lastBlockAgeMs`, time-sync offset |
| Signed test deploy | Authorized account, designated receiver, test amount/max fee, deploy signature/hash/id, signed payload digest, validity window, non-replay plan |
| Deploy distribution | Typed `operator_fields.deploy_relay`: `mode` (`relay` or `four-ingress`), the four aliases, the peer external deploy port (`protocol port + 1`, default 40401; `RNODE_DEPLOY_RELAY_PORT` overrides), the UTC reachability check, and an empty `unreachable_aliases`. `relay` means `RNODE_DEPLOY_RELAY=1` on the ingress node(s) and each peer's deploy service reachable; `four-ingress` means the same signed deploy is offered to each validator and all four receipts are retained. No distribution plan is **NO-GO**: without one the other three validators never see `pooled`. |
| Rollback configuration | Approved previous build/config, snapshot/reference and restoration authority, host capacity and rollback location; no unsafe in-place rebuild |
| Evidence destination | Run ID, immutable source pins, capture paths, independent reviewer and redaction policy |

**Do not pin the 2026-11-02 DevNet binary as the live release.** That artifact established the local contract only. A current release build actually installed on the four public validators is mandatory.

**Hard NO-GO:** diverging binary/genesis/bond pins; unverified quorum/validator set; a missing or
inconsistent `deploy_relay` plan; synthetic boot IDs (`synthetic_boot_ids: true` or any
`boot_id_source` other than `kernel`); inability to observe finality and settlement surfaces;
missing approval to submit the test transfer; unresolved host-capacity or rollback exposure;
unstable time sync; unresolved network instability; or any request to run destructive
restart/partition experiments during this window.

### 1.1. Collect the private candidate record with the merged probe

Use [release-pin-probe.py](../../../tools/release-pin-probe.py) from the frozen runbook checkout; the collector and its [complete-form schema](n487-live-postdeployment.preflight.schema.json) landed in [#572](https://github.com/rhovision-org/rholo/pull/572). Collection is read-only; it does not install a release, submit a deploy, approve a window or establish acceptance. The designated operator runs host-local observation on **each of the four actual validator hosts**, using the service, executable, data/genesis directories and private endpoints that belong to that instance.

```sh
# On each validator host; the operator supplies these private, instance-specific values.
# N487_OUT is a new directory for this run, never a reused collection.
set -eu
umask 077
test ! -e "$N487_OUT"
mkdir -m 700 "$N487_OUT"
python3 tools/release-pin-probe.py observe \
  --out "$N487_OUT" --name "$N487_VALIDATOR_ALIAS" \
  --http "$N487_HTTP" --admin "$N487_ADMIN" \
  --local --service "$N487_SERVICE" --binary "$N487_BINARY" \
  --data-dir "$N487_DATA_DIR" --genesis-dir "$N487_GENESIS_DIR" \
  --config-file "$N487_CONFIG_FILE"
```

Assign four distinct aliases without path separators. Copy exactly one `validator-<alias>.json` per host into a **fresh private aggregation directory**. Retain original observations and their capture times; do not combine records from different installations/windows or fill missing observations from historical evidence. The executable path must be traced to the **running process**: hashing a spare binary at the same version is not an installed-process pin. Host-local `--version` and HTTP version strings are self-reports; resolve the full source commit and independently bind the installed executable digest to the approved CI artifact (URL, run attempt, archive digest, extracted binary digest and packaging/image digest).

Create `$N487_OPERATOR_JSON` privately from this **deliberately incomplete** input skeleton. Replace every null/empty field with approved values; this example is not approval, a release pin, a transfer or an executable test configuration.

```json
{
  "release_tag": null,
  "release_branch": null,
  "selected_source_sha": null,
  "selected_artifact_digest": null,
  "selected_binary_sha256": null,
  "window_start_utc": null,
  "window_end_utc": null,
  "operator": null,
  "observers": [],
  "independent_reviewer": null,
  "rollback_owner": null,
  "test_transfer": null,
  "rollback": null,
  "evidence_destination": null,
  "deploy_relay": null
}
```

`selected_source_sha` must be the full 40-hex approved commit (record an explicit reviewed "untagged
commit build" designation if there is no release tag; never invent a tag). `selected_artifact_digest`
is the SHA-256 of the approved CI artifact/archive/image, not the executable; `selected_binary_sha256`
is the installed executable's digest. The collector derives the manifest's `source` block from
`selected_source_sha` and cross-checks every validator record against all three pins; a short or
absent build self-report is a NO-GO, not a reason to shorten the pin.

`deploy_relay` is the typed distribution plan:
`{"mode": "relay" | "four-ingress", "aliases": [the four validator aliases],
"destination_port": 40401, "reachability_checked_utc": "<UTC>", "unreachable_aliases": []}`.
With `relay`, enable `RNODE_DEPLOY_RELAY=1` on the ingress node (all four for robustness) and verify
each peer's external deploy port (advertised protocol port + 1, default 40401, overridden by
`RNODE_DEPLOY_RELAY_PORT`) is reachable; after admission the relay offers the signed deploy to each
peer's deploy service, the sender keeps its own pool row, a bounded seen set stops circulation, and
the offer is async best-effort (offer/failure/capacity-refusal counters are on `/metrics`). With
`four-ingress`, the same signed deploy is offered to each validator's ingress and **all four
receipts** are retained; that is one deploy with four receipts, never four deploys. `merge --check`
validates the shape and that `aliases` names the four records; it proves the declared plan, not the
network.

`test_transfer` must include every signed-deploy field in §1, including approval reference, maximum fee, payload digest and non-replay plan. `rollback` must identify the prior build/config, state compatibility, snapshot, capacity, authority and separate procedure. The independent reviewer must differ from the operator. Keep signed payloads, endpoints, wallet authorizations and private identities outside Git.

```sh
# On the private aggregation host, after copying the four records.
set -eu
python3 tools/release-pin-probe.py merge \
  --out "$N487_OUT" --expect 4 --check \
  --operator-json "$N487_OPERATOR_JSON"
# Only after the private manifest has passed the human checks below:
python3 tools/release-pin-probe.py merge \
  --out "$N487_OUT" --expect 4 --check \
  --operator-json "$N487_OPERATOR_JSON" --redact
```

A failed `merge --check` means **NO-GO / incomplete or inconsistent collection**. Stop on any nonzero exit and inspect the error; invocation/IO failures also prevent a usable preflight. `observe` may exit 0 while recording read errors or unset fields, so inspect each record before merging. Exit 0 from the merge is only a preliminary collection result, **not GO**. The schema describes record shape, not operational truth. Neither tool checks maintainer authorization or demonstrates live execution.

| Check | Collector coverage / mandatory independent check |
|---|---|
| Record count and missing fields | `merge --expect 4 --check` checks count and recorded `unset` lists. Reviewer must verify actual values, four distinct bonded identities, and one fresh record per actual host; four filenames do not prove four validators. |
| Agreement | Collector compares source, network, shard, genesis hash/raw digest, bond digest, stake and binary digest, and requires every record's source/binary to equal `selected_source_sha`/`selected_binary_sha256` (`selected_candidate_matches_validators`). Reviewer must also compare full source/artifact provenance, bonds/file digests, live bond set and weights against the approved pins. Genesis bonds alone are not proof of the current live validator set. |
| Readiness and observations | A present `ready: false`, stale sample, endpoint/local read error or incomplete local provenance must not pass the human gate merely because `checks.ok` is true. Review `errors`, `local.local_errors`, boot IDs (`boot_id_source` must be `kernel`; `synthetic_boot_ids` must be false), API observability and the full A0 baseline; Date-header offset is coarse and not a time-sync attestation. Different latest finalized heights may be normal; conflicting hashes at the same finalized height are not. |
| Operator fields | Collector checks presence, pin formats, the selected-candidate match, distinct names/boot IDs and the `deploy_relay` shape. It does not check approval validity, nested transfer/rollback completeness, window ordering, reviewer independence or the actual relay reachability. Review every §1 field and approval reference independently. |
| Redaction | `--redact` masks known fields only. Read errors, node addresses, binary/config paths and nested free text may still expose host or endpoint details. Manually review the entire public copy; retain protected originals and document each redaction. |

Retain the collector revision/command, private manifest digest, reviewed public manifest digest and a separate signed/maintainer-reviewed **§1 pin sheet** for fields the collector does not preserve (especially CI provenance, live membership and approvals). Freeze the runbook revision, pin sheet, transfer identity, A0/A1–A4 criteria and approved UTC window together. Jim/Patrick must explicitly record GO against that exact set **before** the sole submission. Any installation, identity, boot, source/binary or membership drift invalidates that freeze: stop, recollect and obtain a new decision.

### 1.2. Concrete maintainer handoff / current blocker

As of this refresh, **no live candidate, four-host installed digest set, approved operator/window or authorized native-REV transfer is supplied here**. Readiness is **NO-GO / NOT EXECUTED**. Completing this runbook or passing offline collector tests does not discharge those inputs.

| Decision owner | Required next input | Completion evidence |
|---|---|---|
| Jim/Patrick | Select the installed candidate and approve its source-to-artifact provenance | Full source SHA, CI run/attempt and artifact/binary digests, matched on all four hosts |
| Designated validator operator | Collect the private four-host records and finish §1 pin sheet | Fresh per-host records, identity/stake/chain checks, capacity and rollback references; A0 baseline at the approved window |
| Jim/Patrick | Name operator, observer(s), independent reviewer, rollback owner and UTC window; authorize the exact bounded signed native-REV transfer | Explicit approval references tied to the frozen runbook, pins and deploy identity |
| Independent reviewer | Review private completeness and public redactions before GO; review live evidence afterward | Preflight review followed by a separate human live verdict under §4 |

These are external run inputs, not values for Aria to guess. Keep [#584](https://github.com/rhovision-org/rholo/issues/584) open until the actual live verdict.

### 1.3. Sample the run and check G1–G5 with the live-gate tool

[`tools/live-gate.py`](../../../tools/live-gate.py) is the instrument for A0–A4. `sample` is
**read-only** (GET only): it never submits a deploy, restarts a node, signs anything or reads a key.
The designated operator starts it before the sole submission and lets it run through the approved
window; the one authorized submission remains the operator's action.

```sh
python3 tools/live-gate.py sample --plan /secure/n487-plan.json \
    --out target/n487-live-<run-id> --interval 1 --duration 1020
```

**Why 1 s.** `check_g2` requires **every** alias to observe `pooled`, `includedUnfinalized`/
`finalityUnknown` (or a Processed tag) and `finalizedSuccess` directly, and this runbook forbids a
second deploy to manufacture a missed stage. The 2026-11-18 local four-validator rehearsal caught
all stages on all four aliases at 1 s and missed `pooled`/`included` at 5 s. The sampler's default is
now 1 s; the flag is written out so the pre-registered cadence is explicit. The 1020 s covers the
120 s A0 baseline plus the 900 s post-submission window. If a timeout stretches a tick, the event's
monotonic timestamp is authoritative; do not lower the cadence.

The private plan names the deploy id and four `{alias, base}` validators; the deploy id must be the
already-signed, approved deploy known before sampling starts. The plan is never committed, and the
run directory records aliases and endpoint digests rather than addresses. Each tick records
`/health`, `/api/status`, `/api/last-finalized-block`, `/api/v1/finality-observation`,
`/api/v1/deploy-status/{id}` and `/api/v1/deploy-settlement/{id}` on every alias, including 503s,
timeouts and connection errors, with UTC, monotonic time, HTTP status, the exact body and its digest.

After the window, author `submission.json` (the accepted-deploy receipt: deploy id, UTC, HTTP status
and signed-payload digest), `claims.json` (the record's own settlement claim and basis) and
`recovery.json` (the **named, approved recovery arm**, its window and commands,
`accepted_deploys_before`/`after`, `final_stages`, `lost_deploys`, the own-key/self-replay halt
observation and the post-recovery settlement), then:

```sh
python3 tools/live-gate.py check --run-dir target/n487-live-<run-id> \
    --preflight /secure/n487-preflight/preflight.json
```

`sample` writes a plain `SHA256SUMS` over its own files; after authoring the three records, hash them
too (or pack the record, which hashes everything), because `check` treats a partial sums file as
INCOMPLETE. `check` re-derives G1–G5 from those records and the private #572 manifest; it does not
trust the collector's `checks.ok` and cannot record G6. A supplied `--verdict` record is validated
(reviewer, named reviewer, verdict enum, evidence digest, https verdict URL, UTC timestamp and a
cited evidence record) and its G6 status is reported. Exit 0 means "G1–G5 all PASS; the worksheet is
complete", 1 a FAIL, 3 an INCOMPLETE (missing evidence is never a pass), 2 a usage error. The report
carries `G6: PENDING` when no record is supplied (and the record's status when one is);
`overall: NOT_A_VERDICT` and `execution_proven: false` are always present. The named independent
reviewer records the verdict; the tool only checks the record's shape.

**The promotion observation.** The executable criteria (G1–G5) are a promotion precondition, not a
parallel note: a binding for a G584 gate must carry `criterion: {observations, sha256,
verdict: "PASS"}`. `tools/gate-collect.py sample --config <plan> --out <run>` runs the same
sampler and writes `<run>/observations.json` in the evaluator's shape. Before packing, set that
document's `record.dir` to the **final committed evidence record** — the register checker
re-evaluates the observation from that path on every check, so a scratch `target/` path fails by
design. Pack the run so the file is `raw/observations.json`; the promotion binding names that path
and its sha256.

### 1.4. Criterion map

| Gate | Where it is evidenced |
|---|---|
| G1 installed pins | §1 and §1.1, re-derived by `live-gate check`; the selected-candidate pins and `deploy_relay` are required preflight fields |
| G2 signed deploy lifecycle | A1–A3 samples plus the operator's `submission.json`; G2 is per-alias, which is why the 1 s cadence and the deploy-distribution pin are preflight requirements |
| G3 no false settlement | §0 contract, §2 negative safety check, `claims.json` |
| G4 recovery without loss | the named recovery arm, `recovery.json`, post-recovery samples |
| G5 independent observations | four aliases' samples, hashed and redacted under §4 |
| G6 verdict | §4, by the named independent reviewer; never by a tool |

### 1.4.1 G6 verdict record

The named independent reviewer writes `verdict.json` with `reviewer`, `relationship`,
`independent: true`, `verdict` (`PASS`/`FAIL`/`INCONCLUSIVE`), `evidence_sha256`, `verdict_url`
(https), `signed_at` (UTC) and `evidence_record` (the retained record it cites). The schema is
`n487-g6-verdict.schema.json`; `live-gate.py check --verdict /secure/g6-verdict.json` validates the
record's shape and reports its status. A tool PASS of the record is not the verdict: the reviewer's
published conclusion remains the G6 record, and `overall` stays `NOT_A_VERDICT`.

## 2. Arms / sampling plan — freeze these criteria before execution

Keep this run **non-disruptive**. No deliberate validator shutdown, network partition, bond change or synthetic finality freeze on the public network. The exactly-2/3, recovery and partition negative controls have **already been exercised locally** in n487; they are not a license to recreate those failures in production.

| Arm | Proposed cadence / window | Required evidence | Verdict |
|---|---|---|---|
| A0 — read-only preflight | Sample all four validators at the run's 1-second interval for **120 seconds** before submitting any deploy | UTC wall-clock plus monotonic/sample sequence, validator boot ID, version/binary hash, latest height, finalized height/hash and lag/ages | All four present; same expected chain/bond set; no unexplained finality divergence |
| A1 — accepted / pooled | Submit **one** approved, bounded, already-signed native-REV test deploy once; poll its ID on all reachable validators every **1 second** | Raw redacted submission response(s); `/api/v1/deploy-status/{id}`; `/api/v1/deploy-settlement/{id}`; `/api/v1/finality-observation`; `/health` | Acceptance alone never marked settled; if `pooled` is seen, it is recorded as pending |
| A2 — inclusion before finality | Continue 1-second samples until inclusion/terminal verdict or approved end of window | Inclusion block hash, height and timestamp; `ProcessedWithSuccess/Error`, `includedUnfinalized` / `finalityUnknown`, `settlementConfirmed`, local and independent finalized frontiers | No `settlementConfirmed=true` for an inclusion block lacking verified finality |
| A3 — finalized and cross-checked | Continue at the 1-second cadence until test deploy reaches `finalizedSuccess` (or an explicit failure/timeout); observe 60 more seconds after finalization | `finalizedSuccess` and `settlementConfirmed=true`; inclusion block finalized on independently queried validators with consistent finalized hash; finality frontier and sample IDs | PASS only when the transfer is genuinely finalized; execution success is insufficient |
| A4 — recovery/retention check | Read-only audit of all submitted IDs and previously pooled work before and after the window | No missing accepted deploy, consistent finality observations, no false settlement, final raw captures | Evidence-only confirmation; do not force a network disruption to manufacture recovery |

**One deploy, four pools.** G2 requires **every** alias to observe `pooled`; a single submission
reaches only the receiving node's pool unless the deploy is distributed. The preflight's
`deploy_relay` records the chosen path:

- `relay`: `RNODE_DEPLOY_RELAY=1` on the ingress node (all four for robustness), each peer's external
  deploy port (advertised protocol port + 1, default 40401; `RNODE_DEPLOY_RELAY_PORT` overrides)
  verified reachable. After admission the relay offers the signed deploy to each peer's deploy
  service under normal ingress validation; the sender keeps its own pool row and a bounded seen set
  stops circulation. The offer is async best-effort: a peer can pool the deploy after the next tick,
  and a full pool answers 429/`RESOURCE_EXHAUSTED` (counted as a capacity refusal, not a relay
  fault). Check `/metrics` for offers, failures and capacity refusals.
- `four-ingress`: the same pre-signed deploy is offered to each validator's ingress and **all four
  receipts** are retained. That is one signed deploy with four receipts, not four deploys.

A peer that never sees `pooled` makes G2 INCOMPLETE; do not resubmit to manufacture the stage.
`merge --check` validates the plan's shape and aliases, not the network reachability.

**G4 needs a named arm, not just A4.** The maintainers must choose and record the recovery arm (a
restart or the runbook's chosen recovery) before GO, because A4 is deliberately read-only. If no
restart occurs naturally and no arm was approved, G4 is **INCOMPLETE** — it is never inferred from
ordinary operation.

**Sampling and event model.** Capture the state transition as an ordered event stream with independent UTC timestamps on **each** node and the client. Persist HTTP status and response bodies, headers, observations, block identities, request IDs, boot IDs, node identity and endpoint version for each 1-second sample. Correlate by the **exact deploy signature/id and inclusion block hash**, not by balances or a coincidental block number. Sample all four; if a validator is inaccessible, state that explicitly instead of silently counting fewer.

**Proposed maximum observation window:** 15 minutes after the single authorized submission (plus baseline/post-finalization samples), subject to maintainer pre-registration. At timeout without positive finalization, record **INCONCLUSIVE / NOT ACCEPTED**, not PASS. If A1 or A2 transition occurs between polls, retain the adjacent samples and mark the unobserved stage as **not directly observed**; do not fabricate a pooled or unfinalized observation. If the acceptance criterion requires direct proof of an intermediate state and it was missed, that arm is inconclusive. Do not submit extra deploys simply to manufacture a missing stage without another approved plan.

**Negative safety check:** Throughout A1–A4, `settlementConfirmed=true` is valid **only** when the stage is `finalizedSuccess` and `finalized=true` for the inclusion block (consistent with `tests/admission_finality_stall_controls.py`). The `finalizedError` stage is a terminal execution error, **not** successful settlement. `pooled`, `notProcessed`, `includedUnfinalized` and `finalityUnknown` must never claim successful settlement. Fail closed on inconsistent responses; do not trust tip balance alone.

## 3. Abort, containment and rollback

Immediately stop further submissions and mark the run **ABORTED / FAIL-SAFE** if any of these is seen:

1. Any `finalizedSuccess` / `settlementConfirmed=true` without verified finality of the **same inclusion block**, or conflicting finalized hashes at an identical finalized height across validators.
2. A test-client message claiming settlement from `ProcessedWithSuccess`, pool admission, tip balance, or `acceptsDeploys` alone.
3. One or more validator identities unexpectedly absent; version/genesis/bond drift; unstable finality/liveness; or observations too stale to distinguish progress.
4. An unexpected validator restart, repeated 5xx, evidence-capture failure, signing-authorization failure, malformed/duplicate deployment, unexpected fee/amount, or any breach of pre-agreed operational limits.
5. A maintainer/operator withdraws approval, the window expires, or the approved rollback condition is reached.

**Abort is not a consensus rollback.** Halt test submissions, preserve accepted deploy IDs and the full evidence record, and notify Jim/Patrick. Keep all four validators' data volumes intact. Do not delete/rebuild state, restart validators, revert a finalized transfer, or make network changes under this document.

If an operator-approved binary/configuration rollback becomes necessary, only the designated maintainer executes the separately pre-approved rollback procedure on the approved target/region, after confirming that state compatibility, snapshot retention and capacity are safe. If rollback cannot be executed safely, leave systems stable and escalate; never improvise a destructive restore. Record every operational action with timestamp and actor.

**Store-loss boundary (#576/#578).** If a store is already lost or an own-key equivocation halt is observed, abort this acceptance run and preserve the evidence. Starting the retained signing key on an empty store is unsafe; do not restart or clear a halt to force progress. The supported recovery ordering is backup restoration, or passive replication/catch-up before key activation under a separate maintainer-approved procedure, as documented in [testnet.md](../../../docs/src/node/testnet.md#recovery). The `DEVNET_RESET_PASSIVE=1 ... reset` / `activate` examples there are **disposable DevNet commands**, not instructions for these public validators. A source-tree fix merged in [#578](https://github.com/rhovision-org/rholo/pull/578) is not evidence that the installed candidate contains it; verify provenance first. Recovery or a changed boot invalidates the preflight and requires a fresh window/GO decision.

## 4. Evidence deliverable and independent human verdict

Publish a **redacted, reviewable** run record in `spec/audit/evidence/n487-live-postdeployment-<UTC-date-or-run-id>/` using the repository's `tools/evidence-pack.py` where applicable, with at minimum:

- `README.md`: exact scope, human verdict, complete preflight pins, arms, observation times, limitations, non-claims, and any abnormal events. Declare `**Evidence locality.** public-live — <one line>`; this is a public live run, neither a local devnet nor an #428 M3 off-host claim.
- `manifest.json` (`schema: 1`, `execution_proven: false`) and matching `SHA256SUMS.json`, checked by `python3 tools/evidence-pack.py verify --record <record>`; retain a conventional `SHA256SUMS` as well. The packed verdict is the reviewer's one of `PASS`, `FAIL`, `INCONCLUSIVE` or `ABORTED`.
- `gate-report.json` from `python3 tools/live-gate.py check` (aliases only; G6 is `PENDING` unless a `--verdict` record is supplied, and even then the report is input to the reviewer, not the verdict).
- Source SHA, release/image/binary hashes, associated CI artifacts, genesis/bond digests, immutable four-validator membership, the `deploy_relay` pin, and operator/independent reviewer identities (redacting private endpoints/credentials). A local rehearsal record is `local-devnet` and never gate evidence; a live preflight must have `synthetic_boot_ids: false`.
- Timestamped raw, redacted samples for `/health`, `/api/v1/finality-observation`, `/api/v1/deploy-status/{id}`, and `/api/v1/deploy-settlement/{id}` from all four validators, with gap/timeout information. Save the raw submission receipt and digest of the signed deploy without publishing signing material.
- Inclusion block identity, finalized frontier/hash, explicit observed transitions, the negative safety assertion (never falsely settled), and post-window accepted-work audit.
- Approved run window and go/no-go record; action log, abort/rollback decision and outcome, if applicable. Retain raw source securely and publish only reviewed redactions.

**Do not call the record PASS because the digests verify**: the hash index attests file identity only. The reviewer must check the actual observed events independently and write a human verdict against the frozen criteria. Likewise, a `gate-report.json` whose five criteria all read PASS is a complete worksheet, not an acceptance: a supplied G6 record's status is reported, but `overall` remains `NOT_A_VERDICT`.

### Decision table

- **PASS — live #487 gate:** preflight matches pins; exact current release binary is deployed to all four authorized validators; the approved native-REV deploy is actually observed through the required stages; the inclusion block is independently verified finalized; no premature settlement or lost accepted work; raw evidence is complete and reviewer signs off.
- **FAIL:** a demonstrated safety or contract violation, especially premature settlement, contradictory finalized identity or confirmed loss of accepted work.
- **INCONCLUSIVE:** missing stage sample, missing validator/evidence/pins, no finalization within approved window, observer disagreement or stale data. Never re-label as PASS.
- **ABORTED:** window or operational safety abort before an admissible verdict.

Only after an independent maintainer reviews the **live** record against this document may #584 be closed. A green CI badge, the n487 DevNet result, or merged #557 **alone** does not establish public-network release acceptance.

## 5. Responsibility and immutable boundary

- **Before the run:** Jim/Patrick approve source/binary pins, four-validator identities and stakes, authorized test deploy, UTC window, rollback owners and this exact acceptance matrix. Freeze this runbook revision as the scoring baseline.
- **During the run:** designated operator executes only pre-approved actions; evidence observer collects all four validators' data. Aria may prepare scripts or review evidence **without operating public validators**.
- **After the run:** publish hashed evidence and human verdict in a **separate evidence PR**. If the run reveals a software defect, create a **separate fix PR**; do not amend the pre-registered criteria after observing the outcome.

**Explicit non-claims:** This draft contains **no live-run result**, no current release pin, no authorization, no public transaction, no live testnet changes, and no #584 closure.
