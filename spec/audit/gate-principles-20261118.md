# Principle-level gate criteria — 2026-11-18

> **Scope correction, 2026-11-18 (sense check); extended 2026-11-19.** The gate register carries
> only boolean current-build tests. M5, M6, G6, C357, TA-C3, the withdrawn S6 row, the 44 HF51 rows
> and — since 2026-11-19 — the 12 host/live rows (G428-M1..M4, G584-G1..G5, TA-C1/C2, C343) are not
> gates; they are release/sign-off obligations in their own trackers. The criteria below remain the
> executable judges for those campaigns, but they are no longer gate coverage.
>
> **Superseded, 2026-11-19.** The 12 campaign rows were deleted and replaced by the seven claims in
> `spec/audit/release-claims.json` and `release-claims.md`. The criteria below are retained as
> historical adapters; they are not the new tracker. See `release-campaign-red-team-20261119.md`.

## Scope and the problem

The canonical register (`spec/audit/mainnet-gates.json`, **47 gates, all met**) records each gate
as prose plus an owner. Since 2026-11-19 the 12 host/live obligations are release campaigns
(`release-campaigns.json`), not gates; their executable criteria remain in `gate-criteria.json`
and are judged by `check-release-campaigns.py`. The executable checks are bespoke: each knows one
instance, and several hardcode the instance in code rather than data.

| checker | instance in the code |
|---|---|
| `tools/check-gate-criterion.py` | `G428-M4`/`G428-M5`; `FLOOR=270`, `CEILING=600`; dwell ≥ 3 |
| `tools/live-gate.py` | `ISSUE=584`; exactly four validators; the #572 preflight |
| `tools/c343-live.py` | block 6; the B/C/D routes; one table path |
| `tools/c357-acceptance.py` | the two shapes and their requirements; `MAX_WINDOW=600` |
| `tools/check-evidence-locality.py` | the M3/off-host trigger |
| `tools/check-bounded-ingress-queues.sh` | four named bindings in one file |

That shape is why closure waits on named people running named procedures: the test is written
for one campaign, not for the invariant the campaign is supposed to demonstrate.

## The distinction

A **criterion** is the prose bar in the register. An **instance** is one campaign's record. A
**principle** is the invariant that every instance of the same kind must satisfy. The register
keeps the criterion; the new layer names the principle and the instance parameters, so the same
code can test another campaign without an edit.

The layer is deliberately small:

1. `spec/audit/gate-criteria.json` — one entry per executable external gate: `principle`,
   `parameters`, `requires`, `negative_controls`, `adapter`, and `pass_condition_sha256`.
   The hash binds the entry to the prose bar, mirroring `witness_binding.asserts_sha256`. The
   prose stays authoritative; a changed bar makes the entry stale.
2. `tools/gate-principles.py` — one `evaluate(principle, observations, parameters)` over a
   closed set of principles. No predicate receives a gate id, person, host or endpoint.
3. `tools/gate-check.py` — the CLI: `PASS` (0), `FAIL` (1), `PRECONDITION` (3). A missing
   required input is never `PASS`.
4. `tests/test_gate_principles.py` — the generic meta-test over **every** entry: a satisfying
   base observation passes; every declared control produces its declared `FAIL`/`PRECONDITION`;
   a missing required input is `PRECONDITION`; malformed input yields a verdict, not an
   exception; at least one control is a contradiction; renaming every actor leaves the verdict
   unchanged; and the entry set equals the machine-checkable external gates.
5. `tools/gate-collect.py` — one read-only collector for any observer count. The config names
   aliases and routes; it does not name an owner. It reuses the `live-gate.py` sampler and
   `release-pin-probe.py` observe path rather than growing a second instrument.

## The closed set

| principle | invariant | gates |
|---|---|---|
| `identity_agreement` | observers agree on the selected pins; identities and endpoints are distinct; each record's bonded stake is a positive integer; count ≥ parameter | G584-G1 |
| `staged_lifecycle` | the declared subject is the sampled deploy (`subject.deploy_id == run.deploy_id`, with `submission` required); each subject reaches each required stage, in order, per observer; one inclusion identity; no success/failure contradiction | G584-G2 |
| `no_false_settlement` | no surface claims settlement before a finalized observation; finality agrees; heights are consistent | G584-G3 |
| `recovery_no_loss` | at least one accepted deploy exists; a post-window `finalizedSuccess` is for an accepted deploy; the same subject finalizes after an observed recovery; no accepted work is lost; no own-key halt or self-replay | G584-G4 |
| `independent_observations` | ≥N distinct endpoints/identities contribute; each alias covers the required routes; records are hash-covered; no duplicate endpoint | G584-G5 |
| `human_attestation` | one typed reviewer record: relationship, independence, verdict enum, evidence digest (equal to the evidence record's manifest digest and present in the cited record), https URL, UTC time, and a cited record inside the repository with a `manifest.json` whose digest the reviewer signed; reviewer ≠ owner in either direction | TA-C1, TA-C2 |
| `healing_anchor` | a fixed finalized anchor survives a K-sample dwell and every validator finalizes it; moving-head equality is not the criterion; the evidence record verifies with pins and its declared digest is `sha256(manifest.json)` | G428-M4 (standalone rejoin records) |
| `finality_campaign` | exactly N=4 and N=8 workloads on a declared multi-host topology with a host floor; each workload carries its M1 `capacity_tier` record, so "adequate independent-host capacity" is one predicate in both gates; deploy finalization, partition/rejoin with rotated non-bootstrap victims, host loss within the declared tolerance, restart/replay and safe recovery; the approved root is preserved throughout; a missing arm is `FAIL`, an unreadable field `PRECONDITION` | G428-M4 |
| `merge_accounting` | per-deploy effects; no unapplied kept write; no rejection without a conflict; kept effects read back from the produced state | G428-M5 |
| `phase_diagnosis` | the observation declares the required locality (`live-host` for the gate); the host document carries the full 14/12-field health/operator contract and the verifier's per-host cells (height gap, readiness conjunction, fault latch, isolation, reset marker); an incomplete contract is `PRECONDITION`, never `FAIL`; the phase table's conditions hold; block-6 disposition is structural; a later orphan overrides an earlier accepted; 404/unparsed is `PRECONDITION` | C343 |
| `lockstep_acceptance` | selected pins; derived/declared lockstep window; no mixed version; required shape; independent reviewer; accepted verdict; incident trace | C357 |
| `bounded_admission` | every ingress queue is bounded by a named constant; no unbounded channel; a full queue refuses in the bounded class | G428-M2 (source half) |
| `pressure_refusal` | the queues are bounded (composes `bounded_admission`), and a capped run refuses with 429/RESOURCE_EXHAUSTED and a retry signal, no 5xx, and a handler reachable after release, while the unset default refuses nothing; counts are recomputed from the named raw transcripts, a record that names raw files carries `SHA256SUMS.json`, the refusal message is bound to the source, the endpoint is non-local, every HTTP/direct surface names the measured listener, and the gRPC surface declares its origin (`endpoint` or `candidate-test`) with the record's candidate pins, a non-local endpoint on the same host and a digest-pinned candidate record; a `candidate-test` surface is candidate-code and cannot claim endpoint transport; a local-devnet record is refused | G428-M2 |
| `off_host_claim` | an off-host/live-host claim carries a routable peer; loopback/private/encoded addresses cannot satisfy it | G428-M3 (locality half) |
| `join_chain` | the six-stage chain is typed, `complete: true` and complete: all stages keyed by peer + anchor, a transport outcome whose words match (not a send line), served = received = restored = walked hashes, and every quoted line present in the named raw logs; an off-host record is digest-pinned however it was supplied (`record.files` or `record.dir`); the off-host policy composes `off_host_claim` | G428-M3 |
| `capacity_tier` | a declared tier is checked against measured per-validator RAM/CPU/disk, peak and replay RSS, replay time at the served height, OOM/restart deltas, and a stake/host map whose top-`k` host loss must leave a supermajority (`stake * 3 > total * 2`); a shared host without a per-validator cgroup share fails; a missing measurement is `PRECONDITION`, a contradiction `FAIL`; a fault-injection record or an override run is not a supported-tier measurement | G428-M1 |

`finality_campaign` is the M4 campaign's judge. It keeps `check_rejoin` as the anchor-retention
predicate and adds the envelope the pass condition names: exactly N=4 and N=8 workloads, a
declared multi-host topology with a host floor, each workload's M1 capacity record (so "adequate
independent-host capacity" is one predicate in both gates), the five arms, rotated non-bootstrap
victims, a host loss within the declared tolerance, restart/replay, and safe recovery with the
approved root preserved. `tools/m4-campaign.py collect` assembles the record from the campaign
artefacts and refuses to fabricate; the evidence record is the promotion layer's check, not a
self-referential digest inside the record.

`capacity_tier` is the M1 record's judge, and `tools/capacity-tier.py collect` is its producer.
The collector reads the declaration and the two arms' JSON, never Docker, and refuses rather than
filling a missing measurement. It classifies a run `fault-injection` when it used
`--allow-under-minimum` or ran below the declared share; the criterion treats that as unkeyable
evidence. The record's distinct host ids are a declaration: independent provisioning and the
host-kill arm are campaign facts, not properties this layer can verify. The tier floors are
criterion parameters (minimal 2 cores / 8 GiB / 100 GiB; comfortable 8 / 16 / 1000), so a policy
change is a data edit the self-test reads.

`human_attestation` is the only principle that consumes a human record. The layer checks the
record's shape and independence; it never manufactures a verdict, names a reviewer, or treats a
name as independence. `TA-C1` and `TA-C2` are now instantiated entries: the criterion checks the
reviewer record, while the campaign verdict remains the reviewer's. Their `witness_kind` and
`blocked`/`external` statuses are unchanged.

**The promotion checker applies the same pins.** `check-mainnet-gates.py` requires a `.py` witness
to define its symbol (a comment or docstring is not a witness), the control's `self_test` to live in
the witness's own file, `evidence.digest` to equal `sha256(manifest.json)`, and a human attestation's
`evidence_sha256` to equal that evidence digest and appear in the cited review record. The builder
repeats the evidence verification before it emits a `met` row, so a register is never generated from
an empty or unpinned record.

**The criterion is a promotion precondition, not a note.** A binding for a gate with a
`gate-criteria.json` entry carries
`criterion: {observations, sha256, verdict: "PASS"}`. The observation must be a file inside the
binding's verified evidence record, listed in its manifest, with the declared bytes, and
`evaluate_entry` must return `PASS`. The builder and the checker call one function
(`criterion_binding_problem`), so a row the builder emits is a row the checker accepts. Only when
the gate has an entry is the block required; a block on any other gate is refused as dead data.

**The witness rule is one implementation.** `tools/gate-witness.py` answers "is this path::symbol a
`#[test]`/`def`/`theorem`/`.md`/flag of this kind?"; `_binding_shape` and `resolve_witness` both call
it, so the builder cannot emit a `met` row the judge would refuse.

**The criteria self-test and the register check run in CI.** `tools/gate-selftest.py` builds one
satisfying observation per entry, applies every declared control, deletes every required input
(both through `evaluate_entry` and directly on the principle), and requires at least one `FAIL`
control. `check-mainnet-gates.py::check_criteria` calls it (memoised), and the CI-run
`tests/test_hardfork_51_closeout.py` executes `check-mainnet-gates.py --check`, which rebuilds the
register, verifies all 47 `met` rows' witnesses/evidence/pins and runs the self-test. In a depth-1
checkout the `register_dev_sha` ancestry check is recorded as `unverified-shallow` instead of
failing; a full checkout still enforces it. `--run-controls` exits non-zero on an `UNPROVEN` or
`ERROR` control. Rust controls execute against their declared mutations: the package is derived
from the path's nearest `Cargo.toml`, the target from `--lib`/`--test`, and the verdict from the
witness symbol's own libtest line — a nonzero exit alone is not a control. `--controls
{python,rust,all}` (default `all`) selects the kinds; `SKIPPED` is only "not selected". The met
ratchet has an independent floor (`MINIMUM_MET = 47` in the builder).

## What this is, and is not

It is a **record measurement**: a `PASS` says an observation document satisfies the principle.
It is not a gate promotion by itself: a gate still needs the witness/control/evidence/pin binding.
The gate register is 47 current-build gates, all met.

**Superseded for the release set, 2026-11-20.** The 12 campaigns are seven release claims in
`spec/audit/release-claims.json`, each with one executable test under `tests/`. A claim's status is
`pass`, `fail` or `not run`, measured from the newest committed record by the test's own
`check(record)`; there is no authored status and no `owed` observation. The criteria below remain
as historical adapters, not as the release tracker.

It does not replace the operator run. A multi-host host-loss arm, a measured soak and a signed
transfer still need operators and hardware. What changes is that their output is a standard
observation document any reviewer can evaluate; the owner is accountable for the evidence, not for
the test logic. For the live G584 gates, `tools/gate-collect.py` writes `observations.json` and
`evidence-pack pack` copies it to `raw/observations.json`; the observation's `record.dir` names the
committed evidence record (a scratch `target/` run fails the CI re-run).

`G584-G2/G3/G5` keep their `human-attestation` rows in the historical criteria. The executable
criterion is the campaign's measurement binding; changing a `witness_kind` is a separate governance
decision. `G428-M3`'s declared witness is `tools/m3-join-chain.py::check_chain`; the off-host
record remains an out-of-scope observation. (`G428-M5` is neither a gate nor a claim.) The layer
records the real adapters and does not silently rewrite those rows.

## Verification

```sh
python3 -m unittest discover -s tests -p test_gate_principles.py -v
python3 -m unittest discover -s tests -p test_gate_collect.py -v
python3 -m unittest discover -s tests -p test_gate_criterion.py -v
python3 -m unittest discover -s tests -p test_live_gate.py -v
python3 -m unittest discover -s tests -p test_c343_live.py -v
python3 -m unittest discover -s tests -p test_mainnet_gates.py -v
python3 -m unittest discover -s tests -p test_hardfork_51_closeout.py -v
python3 -m unittest discover -s tests -p test_m3_join_chain.py -v
python3 -m unittest discover -s tests -p test_m2_pressure.py -v
python3 tools/gate-selftest.py
python3 tools/gate-check.py --list
python3 tools/check-mainnet-gates.py --check        # 47 gates, all met, 0 unresolved
python3 tools/check-mainnet-gates.py --run-controls --controls python  # 2 CONTROLLED, 45 SKIPPED
python3 tools/check-mainnet-gates.py --run-controls                    # 47 CONTROLLED
python3 tools/check-mainnet-gates.py --assert-closeable   # exits 0 (register closed)
python3 -m unittest discover -s tests -p test_release_campaigns.py -v
python3 tools/check-release-campaigns.py --assert-ready   # exits 3 until every claim passes
python3 tools/release-check.py list                       # measured pass/fail/not run per claim
```

Expected after the 2026-11-20 replacement: 47 current-build gates, all met; seven release claims,
each with an existing test and a measured status; the gate register closes and release remains
blocked.
