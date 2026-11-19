# The last 12 gates — path and measurements, 2026-11-18

## The dependency fact

Nine of the twelve open gates declare `depends_on: G584-G1`:

| gate | depends on G584-G1 |
|---|---|
| G428-M1 | yes |
| G428-M2 | yes |
| G428-M4 | yes (and G520, met) |
| G584-G2, G3, G4, G5 | yes |
| TA-C1, TA-C2 | yes |
| G428-M3 | no |
| C343 | no |

`tools/build-mainnet-gates.py::_eligible` refuses to promote a row while any dependency is unmet.
So **no in-box test can flip one of those nine rows until G584-G1 is met.** The critical path is the
selected candidate and the four-host preflight; everything else is evidence preparation.

## A1.3 re-measured: the old N=8 failure is stale

The old verdict was a permanent finality freeze at N=8 on `bbabc39e8` (2026-10-07): 112–120 blocks,
all eight senders, `last-finalized-block` never advanced, with "supporting stake is not a
supermajority — 400 of 800". That tree predates #408 (`b47c6bc88`, "restore N=8 finality"), which is
an ancestor of the current `dev`.

Three local-devnet attempts on `4e097cc8d`, eight equal-stake validators, one deploy,
`--no-autopropose`, 2 GiB per node:

| attempt | blocks after deploy | senders | time to finality |
|---|---|---|---|
| 1 | 312 | 8 | 24 s |
| 2 | 64 | 8 | 7 s |
| 3 | 64 | 8 | 7 s |

All three finalise; the old permanent failure does not reproduce. The refusal string still appears
at low tips (0–8) while support forms; finality advances past it. Record:
`spec/audit/evidence/n8-finality-current-20261118/`.

This is a local falsifier, not the live TA-C1 witness: 2 GiB is below the documented 8 GiB tier, one
host, equal stakes, and the source is `dev`, not the selected candidate.

## #584 rehearsal: the window's tooling works, with one field requirement

A local four-validator devnet on the same tree, one signed deploy, sampled at 1 s with
`tools/live-gate.py`. The worksheet produced:

```
G1: PASS   G2: PASS   G3: PASS   G4: INCOMPLETE   G5: PASS   G6: PENDING
```

- **G2 needs the deploy relay.** `tools/live-gate.py::check_g2` requires **every** alias to observe
  `pooled`, `included` and `finalizedSuccess`. A single submission to one node only puts the deploy
  in that node's pool. The devnet's existing `DEVNET_NODE_ENV` switch carries
  `RNODE_DEPLOY_RELAY=1` (`node/src/runtime/node_runtime.rs:2707`); with it, one submission was
  observed by all four validators at every stage. The runbook now records the distribution as the
  typed `deploy_relay` preflight pin (`relay` or `four-ingress`); a missing plan is NO-GO.
- **1 s cadence catches the stages.** At 5 s, `pooled` and `includedUnfinalized` can be missed; G2
  then reports INCOMPLETE, and the runbook forbids a second deploy to manufacture the stage. The
  runbook now pre-registers 1 s and the sampler's default is 1 s.
- **The collector now carries the selected candidate.** `release-pin-probe.py::OPERATOR_FIELDS` was
  missing the three `selected_*` pins that `live-gate.py::check_g1` requires, so a real
  `merge --check` preflight could never pass G1. The two tools and the preflight schema now share
  one 15-field list, and the collector rejects a missing/malformed pin, a candidate/record
  mismatch, duplicate names or boot IDs, and a malformed relay plan. The corrected protocol is
  recorded in the local rehearsal `spec/audit/evidence/n487-live-window-rehearsal-20261118/`
  (`local-devnet`, not gate evidence).
- **G1 needs the operator fields**, including `evidence_destination`; a missing field is INCOMPLETE.
- **G5** requires every run file to be covered by `SHA256SUMS`; adding `submission.json` after
  sampling breaks it unless the sums are regenerated.
- **G4 is not read-only.** The rehearsal is INCOMPLETE without a chosen recovery arm (`recovery.json`);
  A4 alone is never G4.

The rehearsal's preflight was synthetic and is not gate evidence.

## M2: the criterion is pressure-aware; the positive waits on G1

`node/src/web/http.rs` has
`a_full_in_flight_budget_refuses_concurrent_bodies_with_429_and_never_5xx`: all four permits held,
32 simultaneous requests over a real TCP listener, every one 429 with `Retry-After`, no 5xx; after
release the same request reaches the extractor. The per-peer rate limiter is taken out of the
measurement.

The criterion now consumes that evidence. `gate-criteria.json` G428-M2 names `pressure_refusal`,
which composes the source-queue half (`bounded_admission`) with `tools/m2-pressure.py`: paired
capped/unset runs, the HTTP and raw-socket transcripts, the gRPC ingress and pool arms and a
permissive control, all with counts recomputed from the named transcripts, the refusal message
bound to the source and the files digest-pinned. A public-live record must name a non-local
endpoint; a `local-devnet` record is refused.

`tools/m2-pressure-arm.py` produces the record on a disposable devnet (two lifetimes: the cap is
read at listener startup), and the Rust gRPC arm binds the real `PeerAddressIo`/`DeployIngressGuard`
stack. The local rehearsal is `spec/audit/evidence/m2-pressure-local-20261119/` (`INCONCLUSIVE`):
the record passes its local policy and the criterion refuses it for locality. The default stays
opt-in (decision, 2026-11-18); the unset run is the failing control. Still owed: the public-live
campaign on the installed candidate, and the gate's `depends_on: G584-G1`.

## M3: the chain is checkable; the positive is still owed

`G428-M3` is the only open **G428** gate with no `depends_on` (G584-G1 and C343 also have empty
lists; G584-G1 is the external critical path). Its in-box half is now executable:
`tools/m3-join-chain.py` correlates the joiner's and the responder's logs into a typed six-stage
`chain.json`, and the M3 criterion (`join_chain`) refuses a send-log-only, missing-stage or
hash-mismatched chain and composes the off-host locality check.

- The responder now logs the requested anchor (stage 2), the served state hash (stage 3), and a
  `transport outcome delivered|failed` (stage 4); the joiner logs the sender key (stage 5), the
  completed page walk, and the served/accepted/restored hashes (stage 6).
- Run on a local devnet, the chain check passes (`--locality local-devnet`) while the M3 criterion
  correctly FAILs the record for locality.
- Still owed: a two-host off-host run (the runbook is pre-registered at
  `spec/audit/runbooks/m3-off-host-20261118.md`), the loopback refusal from a real second host, and
  failover (M3-11).

## C343: the collector emits the criterion shape

`tools/c343-live.py collect` now writes `raw/host-<alias>.observation.json` for each host — the
`{health, operator, block6}` document `phase_diagnosis` consumes, with `block6` as journal text
rather than a verdict — plus an index, and carries the run's locality. The criterion requires
`live-host`, so a local rehearsal cannot be read as the operator snapshot. The criterion and the
verifier now share one `host_evaluation`: the 14/12-field health/operator contract, consistency, the
named diagnosis, block-6 disposition, the height gap, readiness, the fault latch, isolation and the
reset marker. An incomplete document is `PRECONDITION`; a contradictory one is `FAIL`. The local
rehearsal is `spec/audit/evidence/c343-observation-shape-20261119/` (`INCONCLUSIVE`): the shape is
emitted, the verifier is `PRECONDITION` because a fresh devnet's journal has no block-6 disposition,
and the criterion refuses the document for locality. The live B/C/D snapshot remains owed.

## External inputs

| owner | input | unlocks |
|---|---|---|
| Patrick/Jim | selected candidate: source SHA, CI artifact, executable digest | G584-G1 and nine dependants |
| Aria/operator | per-host `release-pin-probe.py observe` on the four hosts; merged, checked preflight | G584-G1 |
| Patrick/Jim | window, operator, observers, independent reviewer, rollback owner, signed bounded native-REV transfer, GO | G584-G2, G3, G5 |
| Patrick/Jim | a selected recovery arm for G4 (restart or the runbook's chosen recovery) | G584-G4 |
| Patrick/Jim | testnet on the candidate and approval for kill/rejoin/bond arms | TA-C1, TA-C2 |
| Jim | declared capacity tier and stake↔failure-domain map; independent hosts; a fifth routable observer; N=8 capacity | M1, M3, M4 |
| Aria/operator | B/C/D read-only snapshot with the admin endpoint and a build that serves `/api/v1/status/operator` | C343 |

## What this does not establish

- No gate was promoted. The register is still 59 gates, 47 met, 12 open.
- The A1.3 record is `local-devnet`, not the live TA-C1 witness; the rehearsal's preflight is
  synthetic and is not #584 evidence.
- No public deployment, real key, transfer or disruptive public-network arm ran.
- M1/M4 also carry code gaps before their campaigns: a typed tier record (M1) and a cross-host arm
  (M4). M3's transport/stage-6 witness is now typed and checkable; its off-host arm and failover
  remain. They are not host-only tasks.
