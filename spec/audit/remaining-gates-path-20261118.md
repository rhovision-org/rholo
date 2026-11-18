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
  observed by all four validators at every stage. The live window must enable the relay on all four,
  or offer the same signed deploy to each node. The runbook's "submit once" wording should name the
  relay explicitly.
- **1 s cadence catches the stages.** At 5 s, `pooled` and `includedUnfinalized` can be missed; G2
  then reports INCOMPLETE, and the runbook forbids a second deploy to manufacture the stage.
- **G1 needs the operator fields**, including `evidence_destination`; a missing field is INCOMPLETE.
- **G5** requires every run file to be covered by `SHA256SUMS`; adding `submission.json` after
  sampling breaks it unless the sums are regenerated.
- **G4 is not read-only.** The rehearsal is INCOMPLETE without a chosen recovery arm (`recovery.json`);
  A4 alone is never G4.

The rehearsal's preflight was synthetic and is not gate evidence.

## M2: the pressure arm exists; promotion waits on G1

`node/src/web/http.rs` now has
`a_full_in_flight_budget_refuses_concurrent_bodies_with_429_and_never_5xx`: all four permits held,
32 simultaneous requests over a real TCP listener, every one 429 with `Retry-After`, no 5xx; after
release the same request reaches the extractor. The per-peer rate limiter is taken out of the
measurement.

The default stays opt-in (decision, 2026-11-18). `node/src/web/http.rs:112-118` says no default is
guessed without resource measurements; the gate's negative control is satisfied by running with the
cap set and treating the unset run as the failing control. Before M2 can bind, the criterion needs a
pressure adapter: `spec/audit/gate-criteria.json` currently evaluates only source shape
(`tools/gate-principles.py::bounded_admission`), so a source fixture would pass and neither the
pressure arm nor the unset-default mutation could fail.

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
- M1/M3/M4 also carry code gaps before their campaigns: a typed tier record, a transport-completion
  witness and failover (M3), and a cross-host arm (M4). They are not host-only tasks.
