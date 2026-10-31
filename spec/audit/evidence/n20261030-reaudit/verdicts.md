# October 2026 re-audit — referee verdicts

Each referee saw only the claim, mechanism, class and offered falsifier. Verdict vocabulary and the
candidate ledger are in `preregistration.md`; the measurements in `gas-probe.log`.

## Confirmed

### A1 — the collection conversions sort for a flat charge

`rholang/src/reduce.rs::eval_method`, arms `"toSet"` (~1783) and `"toMap"` (~1810), call
`par_set`/`par_map` and add no per-element charge; the only charge on the path is the flat
`method_call_cost()` = 10. `models/src/sorter.rs::par_set`/`par_map` run two sorts — a derived-`Ord`
comparison sort and `sort_pars`/`sort_pairs` — each comparison O(element size).

Measured (`gas-probe.log`, release): `toSet` delta **18 phlo at every N** from 500 to 40 000 while the
converted wall time grows 7 → 724 ms; `toMap` delta negative at every N with wall time 17 → 620 ms.
Falsifier that would kill it: a measurement showing the conversion's charge grows with N — none
exists.

### A2 — `rho:gov:censure` charges N² for Θ(U³)

`rholang/src/system_processes.rs::gov_censure` → `charge_gov_fold` charges `gov_fold_cost(N) = N²`
with `N = c.len()+lv.len()+v.len()`, after checking `N <= MAX_GOV_UNIVERSE = 512`;
`qucalc/src/lib.rs::censure` is an outer `0..=U` loop × U members × an inner U filter. The internal
universe `U` can be up to `2N`, so the charge can understate the work by up to ~1024×. The code's own
comment concedes the cubic path "was not measured at the bound"; its recorded quadratic path is
~0.36 s at 512, and the cubic path executes that inner body up to U+1 times. No timing test exists.

### A3 — `++`/`.union` charge the right operand for a re-sort of the union

`reduce.rs:994,1010,1505,1518` charge `union_cost(right.len()) = 3·|right|`; each then clones
`left ++ right` and calls `par_set`/`par_map`, which fully re-canonicalize `|left|+|right|` elements.
With `|left| = N`, `|right| = 1` the charge is 3 phlo for Θ(N log N). The equal-operand measurement is
proportional to N (6 949 / 38 949 / 78 949 at N = 1 000 / 5 000 / 10 000), which is the *linear* part;
the asymmetry is the finding. The fix's shape already exists for `diff_cost`, which charges
`|left|·|right|`.

### A4/A5 — `set` and `keys` charge flat for a clone and a full sort

Not separately refereed (the stop rule); they are the same class as A1. The probe records `set` delta
**22 phlo flat** across N = 500…10 000 for a clone + scan + full re-sort; `keys` builds and sorts N keys
for a flat method charge.

### B1 — the greeter dials without the dial budget

`ocapn/src/fixtures.rs::Greeter::receive` (~181-199) dials and `registry.admit`s without
`try_acquire_owned`; `Enlivener::dial_and_fetch` (`ocapn/src/enliven.rs:171-182`) does acquire.
`publish_dialing_fixtures_with_budget` gives the budget to the enlivener only. **Correction from the
referee:** the count is capped at `MAX_PEERS = 256` (registry admission), not unbounded — but not at
`MAX_DIALED_SESSIONS = 16`, so the documented 16-session dial reserve is bypassed and the node can be
made to hold ~304 sessions against the 64 an operator is told. Default off; the four OCapN listeners
are unset.

### B2 — the txn API's leg amount is bounded only by the coordinator's full balance

`api_txn_run` (`node/src/web/http.rs:599-658`) checks leg count, txn id, `to` non-empty and shard, and
copies `amount` through; `GatewayTxn::run_under_lock` applies `NonNegI64` only; the participant's
`txn_prepare` rejects `amount > balance` and accepts `amount == balance`, so one leg can escrow the
coordinator's whole vault on that shard to a caller-chosen address. Off by default; needs a multi-shard
gateway + `enable-txn-api`, and a remote caller additionally needs `enable-devnet-admin-public`. The
referee classifies it as the residual of C121, not a new route.

### B3 — the websocket accept loop can be held ~40 s by one silent client

`ocapn/src/websocket.rs::accept_incoming_connection` (~229-257) runs the upgrade under a 10 s timeout
and the in-band challenge under a 30 s read **inside** the per-transport accept loop
(`node/src/api/ocapn.rs::accept_transport`, ~963-996; the session is spawned only after accept). The
module doc names the residual. Off by default; ws authenticates the server only. Transport-scoped.

### C1 — a >2/3 coalition can jail then slash a censored honest validator

Participation is read from the **finalised fringe only** (`casper/src/multi_parent_casper.rs:277-284`,
`block-storage/src/dag/message_map.rs::latest_fringe`), so a validator whose blocks are withheld never
enters it; `is_absent` then reads it as silent, two advancing boundaries jail it, a vote window opens,
and a strict >2/3 vote slashes and banishes it. **Already recorded**: resource safety case H-RS-17 and
RCHIP #55 §8 name exactly this, and the partition arm that would measure it is still owed.

### C2 — B46 names the wrong predicate

`docs/src/spec/resource-safety-case.md:55` says "`Running` requires no unresolved orphan", but
`startup_audit_then_running` (`casper/src/block_disposition.rs:951`) sets `Running` unconditionally
after a bounded drain, and `node_phase.rs:17-24` says so. Under the code's own vocabulary (an orphan
is an *unaccounted* body — Law 88a "visible, not settled") the code is correct; the **readiness** gate
(`/health`, `orphan_bodies`) is what withholds. Correction: name readiness, not the phase.

### C4 — body and disposition row are two transactions

`block_receiver.rs:559` stores the body, `:579` writes the `Pending` row, in different LMDB
environments (`casper/src/storage.rs:76-112`: `blockstorage` vs `dagstorage`), so a crash between them
orphans the body. `reoffer_unsettled` iterates rows; `scan_legacy_bodies` is gated by a one-time
marker; `audit_stored_bodies` reports and withholds readiness but does not adopt. Recovery: peer
re-delivery or operator reset. Moderate.

### C6 — the finality bound is skipped when no finalised block exists

`proposer.rs:1103-1129` puts the bound **and** the isolation latch inside `if let Some(finalized)`;
`last_finalized_block_hash()` is `None` on a fresh chain or a sub-quorum view, so a bonded validator
can extend its chain without a lag bound (the round gate bounds rate, not height). The Lean model
totalises the no-finality case as genesis height 0 (`rewind_plan` does the same), and
`Disposition.lean`'s `a_gate_without_the_bound_admits_a_private_chain` is exactly the skipped arm.
Classified as a defect in the totalisation of Law 88b, not an out-of-model precondition.

## Not re-established

**B6 — relay fan-out / seen-set.** The mechanism is accurate (default fan-out `usize::MAX`, seen set
cleared at 100 000, `RNODE_DEPLOY_RELAY=1` to enable). The *security harm* is not established: the
relay re-offers an already-signed, already-validated deploy to peers running the same ingress checks;
the node signs, spends and originates nothing. A tuning observation, not a primitive.

## Refuted

**A6 — string `length`.** The mechanism exists (flat 10 for `chars().count()`), but the work is Θ(n)
— linear, not superlinear — and the bytes were already charged at ~1 phlo/byte by the parse or by the
string-producing operation. Fails the claim's own bar; positive artifacts named.

## Calibration note

The lenses' three "expected refuted" candidates did not all behave as their authors predicted: A6 was
refuted, B6 returned `NOT-RE-ESTABLISHED` (the mechanism was accurate but the harm was not),
and C6 was **confirmed** — the author's expectation was wrong, not the referee. The lesson is recorded
in the page: a proposer's confidence is not a control; the control that matters is the verdict's
artifact. Every `CONFIRMED` above names one.
