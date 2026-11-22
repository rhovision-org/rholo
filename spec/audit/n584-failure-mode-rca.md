# #584 failure-mode RCA — false settlement, finality stall and the verdict that was not a test

**Status:** authored 2026-11-22 with the #584 refactor. **Scope:** the failure mode behind the #487
false-settlement incident, the #564 finality stall, and the join/catch-up edges that #640/#641
measured. **Decision recorded here and in `release-claims.json`:** #584's G6 human verdict is retired
as a release condition; the release-claim reading (RC-1/public, RC-4/public) is the verdict. This
document is an RCA, not an acceptance record. It claims no release readiness.

## 1. Top event

> **A client or operator can conclude that a deploy is settled, or that the chain is final, when it
> is not; or finality can stop advancing while the node reports healthy and admits work, without a
> named blocker or a safe recovery.**

The event is a *conjunction of observability and liveness*: the node's local read can be mistaken for
a network fact, and a local proposal failure can remove the only path that would advance the network.
Both were measured on the public testnet; both are now closed by construction at the protocol layer.
The remaining root cause is the **verdict**: the old gate could only close on a named reviewer's
attestation and a previous build's pins, which is not a test of the current build.

## 2. Timelines

### 2.1 #487 — the false settlement (2026-10-28)

On binary `a5259969a7c5673ccdfc5dad97981d0a3607fcf0`, validator A reported `acceptsDeploys: true`,
height 1791, while finality was frozen at 1756; A held 250/1000 bonded stake, below the strict >2/3
quorum. A native-REV transfer was `ProcessedWithSuccess` in block 1781 (unfinalized). At tip the
sender balance read 0 and the target held 13.18 REV; at finalized height 1756 the sender still held
30,000,000 drops and the target 12.9 REV. An application reading tip balance, `ProcessedWithSuccess`
or `acceptsDeploys` would have treated an unfinalized transfer as settled.

Closure: #552 recorded the option-2 contract in `docs/src/node/admission.md`; the settlement endpoint
(`/api/v1/deploy-settlement/{id}`) and the node-local finality observation
(`/api/v1/finality-observation`) make the distinction explicit; the local four-validator campaign at
exactly 2/3 live stake (`spec/audit/evidence/n487-admission-finality-stall-n4-20261102/`) passed. The
live/current-build question was deferred to #584 and is now the RC-1/public claim.

### 2.2 #564 — the 45-hour finality freeze (2026-11-08)

Validator A began rejecting its own block #1115 with a deterministic `ReplayCostMismatch`
(`initial_cost=5576 replay_cost=5302`, a 274-phlo delta) and rebuilt it for ~45 hours without
broadcasting. A held the only non-empty deploy pool; the other validators had empty pools and the
chain was idle. Finality froze at #1110; the visible height drifted to #1116 across eight divergent
blocks. Restart reproduced the mismatch; the deadlock was broken by routing a deploy to a validator
with an independent pool, advancing the height until A's stuck deploys expired.

Two layers were separated:

- **RC-A — no safe exit.** The self-replay fault handler had no transition that changed the input,
  produced an attestation, reset to a finalized anchor, or halted every trigger loudly. C361–C365
  closed it: an attestation-only block over the same pre-state (so the validator's latest message and
  finality are not capped), a durable self-replay record written before the transition, one reset to
  the last finalized anchor, a shard-level halt read by every proposal trigger, and automatic resume
  after the reset's catch-up. Laws 92/94 model the provenance rule and the bounded exit.
- **RC-B — the 274-phlo trigger.** The newest-first event log was appended into replay candidate
  lists without restoring execution order, so a later request could precede the peek repetition its
  recorded COMM required. #570 (`7987aae09`) restored chronological COMM binding; Laws 95a/95b and
  C357 record the mechanism and its acceptance boundary. The §6 compatibility/lockstep decision is
  now moot for the RC public campaign because the campaign uses a **coordinated new genesis**; it
  remains the decision for any retained-history continuation.

### 2.3 #640 / #641 — the join that looks live and is frozen

The RC-4 drill measured a joined validator at height 183 while the network ran to 306, reporting
`running`, ready, no pipeline fault. The boot-only watchdog had exited at the first `Running`; the
view predicate compared the node's validated tip with the quorum-supported height computed from the
same validated messages, so the frozen node read current against itself. The same predicate used as
a global proposer gate froze a healthy network on a six-height skew. #641 corrected the view to
`peerTip − own`, made catch-up continuous, closed the broadcast/claims race, and replaced the
ancestry condemnation with a deferral/backfill obligation. Laws 116/121/122/123/124 are the model.

## 3. Fault tree

```
TOP: false settlement OR silent finality stall OR verdict without a test
├── F1 false settlement
│   ├── R1 admission/processing read as settlement        [#487; #552; Law 86a]
│   ├── R2 local-finality read unmodeled                  [representation.rs:62-143; residual: Law 125]
│   ├── R3 settlement surface unmodeled                   [http.rs:1556-1659; residual: Law 126]
│   ├── R4 declared fringe not compared on ingest         [residual: Law 127 / read-vs-claim]
│   └── R5 one public route treated as four observations  [RC-1/public manifest; G5 evidence rule]
├── F2 finality stall
│   ├── R6 self-replay failure read as divergence         [C357, C361; Laws 92/94]
│   ├── R7 no safe exit / retry the same block            [C362-C365; Law 94]
│   ├── R8 replay candidate binding order                 [#570; Laws 95a/95b]
│   ├── R9 sole-pool proposer + height-based expiry       [attestation fallback; Law 86e relay]
│   ├── R10 join/catch-up boot-only; wrong view predicate [J1/J3/J4/J9; Laws 121/123]
│   ├── R11 rewind re-signs a sequence                    [J8; Law 122]
│   ├── R12 broadcast race                                [J10; Law 124]
│   ├── R13 ancestry gap condemned as divergence          [J11; Law 116]
│   └── R14 equivocation treated as an obligation         [C367/C369; #577/#578]
├── F3 loss of accepted work
│   ├── R15 work only in one pool / no relay              [Law 86e; RC-4/public]
│   ├── R16 reset inventory skips absent bodies           [C366/C274]
│   └── R17 restart replays a stale pool                  [C359/C365; residual: Law 128]
└── F4 verdict integrity
    ├── R18 the verdict is a human/provenance claim       [this refactor: RC reading]
    ├── R19 collector's own ok flag; synthetic boot IDs   [#572; live-gate check_g1 retired as a gate]
    ├── R20 G4 arm never named; boot drift cancels the run [RC-4/public is the named host stop]
    └── R21 prior-test provenance                         [not accepted; fresh run only]
```

## 4. Root-cause closure matrix

| # | root cause | guard / law | evidence | status |
|---|---|---|---|---|
| R1 | admission/processing read as settlement | Law 86a; #552 contract; settlement endpoint | `docs/src/node/admission.md`; `n487-admission-finality-stall-n4-20261102` | closed |
| R2 | local-finality read unmodeled | **Law 125** (`FinalityRead.lean`) | `representation.rs` seen-closure test; route read | this PR |
| R3 | settlement surface unmodeled | **Law 126** (`DeploySettlement.lean`) | `issue_487_*` + exhaustive stage test | this PR |
| R4 | declared fringe not compared / read uses recomputation | **Law 127** (`FringeClaim.lean`) | ingest metadata test; restore/boot `fringe_agreement` | this PR |
| R5 | one route as four | RC-1/public manifest; evidence rule | `release-check.py`; `evidence-pack.py` | process |
| R6–R8 | self-replay provenance, safe exit, binding order | Laws 92/93/94/95a/95b; C357–C365 | #570, #573, #580; `n564-*` records | closed |
| R9 | sole-pool / expiry livelock | attestation fallback (C363); relay (Law 86e) | #564 recovery; `n564-self-replay-*` | closed (local); public arm open |
| R10–R13 | join/catch-up, re-sign, broadcast, ancestry | Laws 116/121/122/123/124 | #641; RC-4 local `safe@ec104fb` | closed |
| R14 | equivocation as obligation | C367/C369; #577/#578 | `n457` arms | closed |
| R15–R17 | accepted-work loss | **Law 128** (`RecoveryPreservation.lean`); C366/C359/C365 | RC-4 local; `reset.rs` inventory | this PR |
| R18 | human/provenance verdict | **#584 refactor**: RC reading is the verdict | `release-claims.json`; `--assert-ready` | this PR |
| R19 | collector/synthetic pins | #572; `check_g1` retired as a gate | `live-gate.py` diagnostic | this PR |
| R20 | G4 arm / boot freeze | RC-4/public's real host stop | `release-claims.json` RC-4 | process |
| R21 | prior-test provenance | `not-observed` only by a fresh run | `release-check.py` | this PR |

## 5. Register rows on the path that are weaker than their Rust

The RCA must not silently upgrade these; the plan either strengthens them or records a non-claim.

| row | weakness | treatment |
|---|---|---|
| **17d** | `vacuous` — native-effect attribution states a **type** fact (private field, no bare-action constructor), not a theorem; three Rust decode tests already pin the negative half | **no change.** The type is the guard; a Lean theorem over a constructor nobody writes is the shape the register already refused. This is a register-quality decision, not a #584 root cause |
| **111** | definition-level possession-challenge settlement, not deploy settlement; the `falsifiable` prose names the mutant, but no `mutant`/`falsifier` fields | register debt; not on the deploy-settlement path. Do not conflate it with Law 126 |
| **104** | the Lean `digest` is the identity function; the real binding is the Rust witness | record as a model limitation, not a new law |
| **80** | `CURRENT_ROOT` names only an accepted block; `rust` files and Lean witnesses exist, but no executable `rustWitness` | register debt; not required to close this failure mode |
| **85** | the fringe root is a member's claim; no executable `rustWitness` | register debt; Law 125/127 model the read that uses it |
| **58** | liveness-to-finality sufficiency is a named hypothesis, not a theorem | record as a residual; measure liveness in RC-4/public, do not prove it |
| 14a, 14b, 51a/b/d, 83, 84, 88c/d, 105, 106, 108, 112 | no mutant/falsifier | register debt; not required to close this failure mode |

## 6. The verdict root cause (F4/R18)

The old closing condition was:

1. a maintainer selects a candidate and a previous build's artifact/pins;
2. an operator collects four host records with privileged access;
3. the maintainers approve a UTC window, a signed transfer and a rollback owner;
4. an independent reviewer records G6.

Every step is an **attestation about a run**, not the run. The issue has stayed open because the
attestations were never supplied; and even if they were, a previous test on a previous build would
not be a test of the current one. The new system replaces the verdict, not the observation:

- the release claims run a **test with a pure predicate over its own trace**;
- `release-check.py` derives the reading from the newest **committed** record and recomputes the
  predicate; `target/` runs are scratch and never make the tracker ready;
- `--assert-ready` refuses until every required arm is `safe`;
- a `not-observed` arm is satisfied only by a fresh run on the current candidate.

The public campaign still needs four hosts and one deploy. That is an input to a test, not a
provenance claim; if the hosts are absent, the arm reads `precondition`/`not-observed` and the gate
refuses.

## 7. Residuals and non-claims

- **Not claimed:** release readiness. `release-check.py --assert-ready` still exits 3; RC-1/public
  and RC-4/public are `not-observed`; #428 remains NO-GO.
- **Not claimed:** a public campaign ran. This document is a source-level RCA; the RC public record is
  owed.
- **Not claimed:** the §6 retained-history decision. The RC public campaign uses a coordinated new
  genesis; a continuation of the old chain would need that decision separately.
- **Not claimed:** the `CURRENT_ROOT`/fringe-root laws are fully proved; 80/85 gain witnesses, and the
  digest identity in 104 remains a model limitation.
- **Not claimed:** Law 58's liveness-to-finality sufficiency is a theorem. It is a hypothesis; the
  campaign measures it.
