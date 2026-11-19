# Principle-level gate criteria — 2026-11-18

> **Scope correction, 2026-11-18 (sense check).** The gate register now carries only boolean
> current-build tests. M5, M6, G6, C357, TA-C3, the withdrawn S6 row and the 44 HF51 rows are not
> gates; they are release/sign-off obligations in their own trackers. The M5 and C357 criteria below
> remain method records but are no longer gate coverage.

## Scope and the problem

The canonical register (`spec/audit/mainnet-gates.json`, 111 gates, 0 met) records each gate as
prose plus an owner. The executable checks that support the external gates are bespoke: each
knows one instance, and several hardcode the instance in code rather than data.

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
| `human_attestation` | one typed reviewer record: relationship, independence, verdict enum, evidence digest, https URL, UTC time, cited record; reviewer ≠ owner | M6, G6, TA-C1/C2 |
| `healing_anchor` | a fixed finalized anchor survives a K-sample dwell and every validator finalizes it; moving-head equality is not the criterion | G428-M4 |
| `merge_accounting` | per-deploy effects; no unapplied kept write; no rejection without a conflict; kept effects read back from the produced state | G428-M5 |
| `phase_diagnosis` | the observation declares the required locality (`live-host` for the gate); the host document carries the full 14/12-field health/operator contract and the verifier's per-host cells (height gap, readiness conjunction, fault latch, isolation, reset marker); an incomplete contract is `PRECONDITION`, never `FAIL`; the phase table's conditions hold; block-6 disposition is structural; a later orphan overrides an earlier accepted; 404/unparsed is `PRECONDITION` | C343 |
| `lockstep_acceptance` | selected pins; derived/declared lockstep window; no mixed version; required shape; independent reviewer; accepted verdict; incident trace | C357 |
| `bounded_admission` | every ingress queue is bounded by a named constant; no unbounded channel; a full queue refuses in the bounded class | G428-M2 (source half) |
| `pressure_refusal` | the queues are bounded (composes `bounded_admission`), and a capped run refuses with 429/RESOURCE_EXHAUSTED and a retry signal, no 5xx, and a handler reachable after release, while the unset default refuses nothing; counts are recomputed from the named raw transcripts, the refusal message is bound to the source, the endpoint is non-local, and a local-devnet record is refused | G428-M2 |
| `off_host_claim` | an off-host/live-host claim carries a routable peer; loopback/private/encoded addresses cannot satisfy it | G428-M3 (locality half) |
| `join_chain` | the six-stage chain is typed, `complete: true` and complete: all stages keyed by peer + anchor, a transport outcome whose words match (not a send line), served = received = restored = walked hashes, and every quoted line present in the named raw logs; an off-host record is digest-pinned; the off-host policy composes `off_host_claim` | G428-M3 |
| `resource_attribution` | process accounting counts only the node's processes; the declared count is required and must equal the observed count; every entry carries a positive integer `pid`, each listed once | G428-M1 |

`human_attestation` is the only principle that consumes a human record. The layer checks the
record's shape and independence; it never manufactures a verdict, names a reviewer, or treats a
name as independence.

## What this is, and is not

It is a **record measurement**: a `PASS` says an observation document satisfies the principle.
It is not a promotion. The register still refuses a `met` gate without the existing
witness/control/evidence/pin binding, and the register stays at 0 met.

It does not replace the campaign. A multi-host host-loss arm, a measured soak and a signed
transfer still need operators and hardware. What changes is that their output is a standard
observation document that any reviewer or CI job can evaluate; the owner is accountable for the
evidence, not for the test logic.

`G584-G2/G3/G5` keep their `human-attestation` register rows. The executable criterion is an
additional measurement binding cited by the evidence record; changing a `witness_kind` is a
separate governance decision. `G428-M3`'s declared witness is now `tools/m3-join-chain.py::check_chain` and
`G428-M5`'s is the `MergeReport` struct; both are `blocked`, so `resolve_witness` never runs.
The layer records the real adapters and does not silently rewrite those rows.

## Verification

```sh
python3 -m unittest discover -s tests -p test_gate_principles.py -v
python3 -m unittest discover -s tests -p test_gate_collect.py -v
python3 -m unittest discover -s tests -p test_gate_criterion.py -v
python3 -m unittest discover -s tests -p test_live_gate.py -v
python3 -m unittest discover -s tests -p test_mainnet_gates.py -v
python3 -m unittest discover -s tests -p test_m3_join_chain.py -v
python3 -m unittest discover -s tests -p test_m2_pressure.py -v
python3 tools/gate-check.py --list
python3 tools/check-mainnet-gates.py --check        # 59 gates, 47 met, 12 unresolved
python3 tools/check-mainnet-gates.py --assert-closeable   # exits 3
```

Expected unchanged by this layer: 59 gates, 47 met, 12 unresolved; closeout exits 3.
