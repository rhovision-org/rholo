# Security audit (October 2026)

This chapter is a point-in-time report of an adversarial read of the node at **`dev@be5016682`**
(2026-10-30). It records what the pass confirmed, what it could not settle, and how the node's safety
properties compare with three other production chains read from their own source at pinned revisions.

It is written to be useful when it is unflattering. The pass's principal finding is a **live instance
of the cost-model class the September 2026 pass opened** — collection conversions that sort for a flat
charge — together with five defects outside that class and two places where the assurance record says
more than the code does. The structural properties that make whole classes of defect unrepresentable
are unchanged, and they are in §6.

**Scope, and how to read it.** The pass itself was report-only: no production code was changed while it
ran. What you are reading is a photograph of `be5016682`, so the measurements in §3 and the verdicts in
§2 are true of that commit — and **the re-admission checks in §2 are what say whether they still hold**.
The eight findings were then remediated in the same PR (pass §149), so a reader of today's tree should
read §2 as *what was found* and §149 as *what changed*: the §2 rows are not rewritten, because a
findings page that edits its own findings cannot be checked against. Disposition is *not* this page's
claim — the check-off is [`spec/AUDIT.md`](../../../spec/AUDIT.md), the account of the pass is
[`spec/audit/passes.md`](../../../spec/audit/passes.md) §148 with the remediation as §149, and the
evidence is [`spec/audit/evidence/n20261030-reaudit/README.md`](../../../spec/audit/evidence/n20261030-reaudit/)
and [`spec/audit/evidence/c345-c352-remediation-20261030/`](../../../spec/audit/evidence/c345-c352-remediation-20261030/).
The September 2026 pass is preserved as §9.

Two design facts from the prior close-out still shape any remediation. Cancellation inside a running
builtin is **not achievable on this design** — a builtin runs its CPU synchronously inside
`Box::pin(async { … })` with no `await` before it, so a deadline bounds how long the *caller* waits and
never the work itself. And the release publishes a `sha256sum` and nothing further, because a signature
needs a trust root and a key policy that a code change should not choose silently.

## 1. Method

The approach is the project's audit history's: independent adversarial lenses, then a separate
refutation stage. This pass adopts the **four-verdict** vocabulary the repository actually defines
(`docs/src/spec/adversary.md`, `spec/audit/evidence/wargame/preregistration.md`):

- **`CONFIRMED`** — the mechanism was re-derived in the code and the falsifier that would kill it is
  named.
- **`NOT-RE-ESTABLISHED`** — plausible, but not re-provable from source; the row names the measurement
  it would need. The dynamic class (races, load, crash timing) lives here, deliberately.
- **`REFUTED`** — only with a **positive artifact**: a named line, an existing test, or a measurement
  showing the mechanism absent. An empty-handed refutation is forced to `NOT-RE-ESTABLISHED`; a
  self-reported clean is not evidence.
- **`UNSTATABLE`** — only for a proposer-side claim with no reachability fact.

**Three lenses, blind to each other and to the register.** *A* read work and the block path
(`rholang` cost sites, `models` canonicalisation, `casper` validation/processing, the gRPC ingress).
*B* read network ingress and the new surface (the OCapN crate and bridge, every HTTP/admin route, the
hostlist, the relay, config defaults). *C* read consensus, recovery, liveness, the Lean laws 86–90 and
the assurance apparatus. Each returned at most six candidates with a mechanism, reachability, an
artifact and a falsifier.

**Refutation and calibration.** Each candidate went to a separate referee that saw only the claim,
mechanism, class and falsifier. The lenses each supplied one candidate they expected a referee to
reject. **The authors were right once in three** — one was refuted, one returned
`NOT-RE-ESTABLISHED` (the mechanism was accurate but the harm was not), and one was *confirmed*: the
no-finality proposal bound, which the model itself exhibits as
`a_gate_without_the_bound_admits_a_private_chain`. The lesson is recorded rather than smoothed: a
proposer's confidence is not a control; the control is the verdict's artifact, and every `CONFIRMED`
below names one.

**What was not run.** No live multi-node attack; no external audit; no mutation score; no coverage
re-measure; no full reading of the three chain trees; no 600 s wall-clock wait (the deadline argument
is a projection from measured per-call time); no 100 000-entry relay exhaustion; no market-cap
measurement.

## 2. Results

| | |
|---|---|
| Confirmed P0 (chain split / fund loss / RCE) | **none** |
| Confirmed P1 (remote unauthenticated CPU denial of service; a proposal bound that is skipped) | **3** |
| Confirmed P2 | **3** |
| Confirmed P3 / assurance | **2** |
| Not re-established (mechanism true, harm not established) | **1** |
| Refuted, with a positive artifact | **1** |
| Re-verified and already recorded (no new row) | **1** |

The rows are **C345–C352** in [`spec/AUDIT.md`](../../../spec/AUDIT.md); their accounts are §148 of
`passes.md`. The absence of a P0 remains the headline, and it is a result about the prior work: on a
tree where earlier passes confirmed remote denial-of-service defects, this pass could not find a chain
split, a fund loss, or remote code execution.

**Carried findings from September**, re-derived rather than reopened:

| September item | State today |
|---|---|
| The gas model does not bound work (P1) | **Partially re-opened.** The September instances are fixed (§3); the *class* recurred in the collection conversions, the union asymmetry and `censure` (C345, C346). |
| The DAG can wedge silently after a crash (P2) | **Moot.** The fringe-data record retired (`casper/src/dag.rs:1439-1482`); the write-order finding and its silent-restore arm have nothing left to order against. |
| Governance arithmetic unchecked (P2) | **Closed.** Saturating folds; `censure` charged and bounded (and now found underpriced — C346). |
| Block `timestamp` unvalidated (P2) | **Half-closed.** A one-sided predicate landed: a block may not be stamped before its parents (`casper/src/validate.rs:544,562-581`). A far-future stamp is still accepted by design (#223); the decision is open. |
| DAG memory quadratic (P2) | **Accepted, measured.** The growth is a known function of a known input; sizing a host remains the answer and the rate is in §4. |
| 14 lower-severity items (P3) | Re-derived; most closed (deny policy, schema casing, openapi body, workflow pins, counts). The type-system counted classes are still reported, not enforced. |
| One not-re-established (`censure`) | Closed, then re-opened as underpriced (C346). |

**Both tables are the state at the pin.** All eight rows above were remediated in §149 (C348 kept as
an accepted residual), so the gas-model row's "partially re-opened" instance is now charged — output
size for the conversions, both operands for the union, `(U+1)³` for `censure` (Law 91) — and the
"underpriced" reading of `censure` is answered by the cube.

The P3 signing exception is a **decision, not work**: the release emits an `sha256sum` and no
signature or SBOM, because a signature needs a trust root this repository has not chosen.

## 3. The gas model does not bound work — September and October

This is the pass's principal finding and the reason this section keeps its number.

**What September measured, at `67dd6fb7b`.** `Set.union` deduplicated through a linear scan, so it was
Θ(N²) — an empirical exponent of **2.43** — and the cost table charged a flat **13 phlo**:

| N | union | total |
|---|---|---|
| 5 000 | 0.42 s | 0.45 s |
| 10 000 | 1.65 s | 1.7 s |
| 20 000 | 7.97 s | 8.0 s |
| 40 000 | 64.1 s | **98.0 s** |

**What landed since.** The dedup is sort-then-dedup (`models/src/sorter.rs:851-887`), the charges for
union/diff track their arguments (`rholang/src/accounting.rs:112-136` — **the union half was the right
operand only until §149**, and both are charged now), the parse is charged before evaluation
(`rholang/src/runtime.rs:336-346`), a block carries `MAX_BLOCK_PHLO = 25 500 000 000` and at most 255
deploys (`casper/src/validate.rs:784-809`), and the governance folds are bounded at
`MAX_GOV_UNIVERSE = 512` and charged (`rholang/src/system_processes.rs:1273-1292`, cubic for `censure`
since §149).

**What October measured, at `be5016682`** (release; `rholang/tests/gas_amplification.rs`,
`RNODE_GAS_PROBE=1`; full log in the evidence directory):

| operation | charged | work |
|---|---|---|
| `[N ints].toSet()` | **18 phlo at every N** (500 → 40 000) | wall time 7 → 724 ms; the conversion's own share grows ~N log N |
| `[N pairs].toMap()` | **no positive charge site** (delta negative at every N) | wall time 17 → 620 ms |
| `map.set(k,v)` | **22 phlo flat** (500 → 10 000) | clone + linear scan + full re-sort |
| `map.keys()` | flat method charge (10) | clone N keys + full sort |
| `setA ++ setB`, `\|A\|=\|B\|=N` | `3·N` | proportional — **the September union defect is closed** |
| `setA ++ {x}` | **13 phlo** | re-sorts `\|A\|+1` elements |
| `rho:gov:censure` at U | `N²`, N = list lengths | Θ(U³); U can reach 2N |

**The class, stated narrowly.** A signed deploy is an unauthenticated remote input. `toSet`/`toMap`,
`set`, `keys` and the asymmetric union perform superlinear work for a charge that does not grow with
it; `censure` is underpriced by a factor of U. The block caps bound *declared* phlo and *counts*, never
CPU, and the 600 s `BLOCK_VALIDATION_TIMEOUT` aborts the awaiting task but cannot interrupt a
synchronous builtin — it bounds the waiter, not the work. **These are the instances found by reading
the conversion and fold arms; that is an inspection, not a proof that no fourth exists**, and no gate
is about cost (§5).

**What §149 did, and what §150 added.** The conversions and `set`/`keys` now charge their output
(`3 · n`, computed before the work), `++`/`.union` charge **both** operands (as `diff_cost` already
charged `|left|·|right|`), and `censure` charges `(U+1)³`. The bound stayed at 512 because the cubic
path was then measured at it — 33 ms for a 512-member universe in release. **§150 then built the
instrument §149 lacked**: [`spec/COST.md`](../../../spec/COST.md) registers every priced operation with
the shape of its charge, a census refuses an arm that declares none, and a deterministic shape test
measures that a declared shape still grows — and its first census found **two more instances of the
class** (`set.add` C353, the tallies' uncounted option lists C354). What remains is the **log factor**
— the charge is linear for `O(n log n)` work, bounded, not exact — and the fact that the gate reads
declarations rather than work. The 600 s deadline stays as the liveness backstop it is.

## 4. Other confirmed findings

**The proposal finality bound is skipped when there is no finalised block (P1, C351).**
`create_block` computes the last finalised height and puts the whole bound **and** the isolation latch
inside `if let Some(finalized)` (`casper/src/blocks/proposer/proposer.rs:1103-1129`). On a fresh chain
or a sub-quorum view `last_finalized_block_hash()` is `None`, so nothing fires and a bonded validator
can extend its chain without a lag bound — the round gate limits the rate, not the height. The Lean
model totalises the case as genesis height 0 (`rewind_plan` does the same), and its
`a_gate_without_the_bound_admits_a_private_chain` is exactly the skipped arm. This is the one finding
the lenses expected to be refuted. **Remediated in §149**: the gate is total in the anchor.

**The OCapN greeter dials without the dial budget (P2, C347).** A peer that completes a CapTP session
can deliver a `handoff-give` to the published greeter, which dials the named exporter location without
taking a unit from the `MAX_DIALED_SESSIONS` reserve
(`ocapn/src/fixtures.rs::Greeter::receive`; contrast `ocapn/src/enliven.rs:171-182`). The registry caps
the count at `MAX_PEERS = 256`, so it is bounded — but at 16× the documented dial reserve, and a
documented 64-session surface becomes ~304. All four transports are off by default. **Remediated in
§149**: both dialing fixtures take one shared permit, before the connect.

**The transaction API's leg amount is bounded only by the coordinator's whole vault (P2, C348).** The
ingress checks leg count (32) and rate (10/s) and copies `amount` through; the participant rejects
`amount > balance` and accepts `amount == balance`, so one admitted leg can escrow the validator's
entire vault on that shard to a caller-chosen address. This is the residual of the already-known C121:
the load-bearing guards are the loopback admin bind and `enable-txn-api`, and a remote caller needs
`enable-devnet-admin-public` as well. **Kept as a residual** (§149): the API still does not cap a leg,
and the [operating page](operating.md) now says to cap the coordinator's vault rather than the
request.

**The body and its disposition row are two transactions (P2, C350).** The receiver stores the body
(`block_receiver.rs:559`) and then writes the `Pending` row (`:579`) in **different LMDB
environments** (`casper/src/storage.rs:76-112`), so a crash between them leaves a stored body with no
row. `reoffer_unsettled` iterates rows; the legacy scan is gated by a one-time marker; the periodic
audit reports and withholds readiness but does not adopt. Recovery is a peer re-delivery or an
operator reset. No safety breach — the verdict truth stays in DAG metadata — but a lost obligation.
**Remediated in §149**: the ledger shares the bodies' LMDB environment and `put_across` commits both
in one transaction.

**A websocket client holds that transport's accept loop for ~40 s (P3, C349).** The upgrade (10 s
timeout) and the in-band challenge (30 s read) run inside the per-transport accept task rather than
the session task (`ocapn/src/websocket.rs:229-257`; contrast `noise.rs:301-313`). The module doc names
the residual; only new sessions on that one optional transport are blocked. **Remediated in §149**:
the accept is a socket accept and the upgrade runs on the connection's first use.

**Two assurance findings.** Barrier B46 says "`Running` requires no unresolved orphan", but the code
sets `Running` after a bounded audit and gates **readiness** instead (`node_phase.rs:17-24`; the code's
own "visible, not settled" vocabulary in Law 88a is correct). And the HAZOP/resource-safety gates
check that a named symbol *occurs in a file*, not that the named test asserts the claim — the
discipline `check-rust-witnesses.sh` applies to the register's witness column is not applied to those
falsifier cells. The first is C352; the second is recorded here and in §5, not as a row, because it was
not carried through the referee queue. **Remediated in §149** for B46 (it names readiness); the
falsifier-cell gate remains the open instrument described in §5.

**Liveness enforcement, re-verified (no new row).** A coalition holding strictly >2/3 of active stake
can jail and then slash an honest validator whose blocks are merely *censored*: participation is read
from the finalised fringe only (`casper/src/multi_parent_casper.rs:277-284`), so a validator absent
from that fringe reads as absent, and a vote can reach it after jail. The hazard is **already
recorded** — resource safety case H-RS-17 and RCHIP #55 §8 — and the partition arm that would measure
it is still owed.

**New surfaces, measured.** The four OCapN transports (tcp-testing-only, unix, noise, websocket) are
all unset by default; the admin dial route is off and, when on, loopback-bound, 2/s, deny-local; the
deploy relay is off and re-offers only already-signed, already-validated deploys (its fan-out and
seen-set behaviour was **not** re-established as a security primitive); the hostlist is off, admits 128
entries, caps probe bodies at 64 KiB and refuses a probe target of the wrong host class; admission is
pool capacity with 429 + `Retry-After`, and the phase/fault gates refuse deploys on a syncing, faulted
or isolated shard.

## 5. What a green register does not cover

The check-off is [`spec/AUDIT.md`](../../../spec/AUDIT.md) and it says what it says — cite it, do not
restate a total here, because a hand-typed count is the claim that rots. What the register does not
cover is specific:

- **The cost gate reads declarations, not work.** §3 was the demonstration that no gate could see a
  charge that does not track its work: a live cost defect on a tree where every gate was green. There
  is one now — `spec/cost-register.tsv` with `tools/check-cost-register.py`, and a deterministic shape
  test beside it ([the cost model](../formal/cost-model.md), pass §150) — and its limits are the next
  bullet's shape: it fails on an arm with no declared charge, a row for nothing, and a charge moved out
  of its arm, and the shape test fails when a charge stops growing. It **cannot** see a `linear`
  declaration over `O(n log n)` work (the recorded log factor), a wrong declaration, or a mis-set
  constant. A new builtin that charges nothing still passes if it *declares* `identity` or `capped` —
  the claim is now written down and reviewable, which is the difference.
- **The assurance gates read names, not assertions.** A HAZOP falsifier or a resource-safety evidence
  cell that names a real test in the right file passes `--gate` without anyone checking that the test
  asserts the deviation's negation. The register's own witness column has a run-by-name gate; these
  cells do not.
- **The computed gates are weaker than they read.** The type-system gate's counted classes (`cast`,
  `lax`, `get`, `index`, `div`, `overflow`) are reported and not enforced; the ratchet that once failed
  the build is no longer wired.
- **The assurance floor moved down and nothing measures the new one.** The mutation harnesses were
  deleted; the line-coverage floor is execution coverage and is satisfiable by tests that assert only
  well-formedness. This pass could not put a mutation score on the workspace.
- **The push path is thinner than the nightly.** The formal gate, the coverage floor and the devnet
  fuzz are nightly or manual; a change that breaks a corpus can land on a push and stay green for a
  day.

**A green gate is not evidence of security.** It is evidence about the class that gate can see.

## 6. Inherent and extrinsic safety

Bug-hunting audits the *extrinsic* half of a security posture. The other half is **inherent safety**: a
property that follows from the semantics, without trusting the implementer or a runtime check.

**Two classes of privilege-escalation defect are unrepresentable in rholang, and one is proved.**

*Authority cannot be forged.* No grammar production writes a private name and no rholang operation
destructures one; a name exists only to be received.

*Authority cannot be captured across a call boundary.* A COMM step transfers exactly the evaluated
datum into the tuple space and substitutes it into the *receiver's* body in the receiver's environment.
Closedness is a **theorem**: a closed program cannot grow a free variable by reduction. Also proved:
substitution preserves sort and closedness, sort is functional and decidable, canonicalisation is
idempotent and commutative, and merge is a commutative monoid — so execution *order* cannot change the
state.

**Honest caveats.** Three, and they matter:

1. **Unforgeability is axiomatised, not proved** (law 19). Say "axiomatised at the crypto boundary",
   never "proved unforgeable".
2. **The `rho:*` namespace is ambient authority** — a fixed table of system channels resolved for
   every deploy. `rho:gov:*` is now charged and bounded (§3), but the channels themselves are ambient.
   The **OCapN bridge is a second ambient path**: a remote peer's delivery becomes a deploy the node
   signs with its own key. It is off by default, rate-limited, and the dial policy constrains its
   egress, but it is authority the language does not hand out — §4's greeter finding is in this
   surface.
3. **The blessed genesis keys are published constants**; what prevents their use is a negative
   allow-list, an access-control list restoring a forgeable capability.

New tied properties since September: the admission contract (law 86), the block disposition and
proposal gates (law 88), liveness enforcement (law 89, a hard fork), the hostlist (law 90, advisory),
and the exact decimal algebra (law 87, a hard fork). Each is machine-checked and tied to its Rust by a
corpus or named witnesses; none of them makes a cost charge proportional or a gate see work.

## 7. Comparison with other chains

The question is the same for all four: *is there an operation reachable by an unauthenticated remote
attacker whose cost to the node is superlinear in attacker input while the charge is flat?* **Every
one has one** — a defect class in VM cost models, not one chain's weakness.

Pinned and read 2026-10-30: Solana (Agave) `v4.4.0` / `9ef63128`; Sui `testnet-v1.83.0` / `16d2dc1e`; Bitcoin SV
`v1.2.3` / `6504a3af`.

| chain | worst instance at the pin | attacker pays | containment |
|---|---|---|---|
| **This node** | conversions sort for a flat 18 phlo; `set`/`keys` flat; `censure` N² for Θ(U³); asymmetric `++` 3 phlo | 18–22 phlo, or N² | per-block phlo cap (25.5e9), 255-deploy cap, 600 s deadline that bounds the waiter, not the work |
| **Solana (Agave)** | bignum is a complexity formula (base 422 / divisor 189), not September's flat 8 042; account data copied under a 64 MiB cap with no per-byte charge in the files read | 5 000 lamports whether it loads anything or not | compute-unit budget; 64 MiB cap |
| **Sui** | gas charges bytes proportionally; September's zkLogin cell could not be re-established at this pin | gas; transactions bounded | instruction tiers; size bound; verifier meter |
| **Bitcoin SV** | `OP_CAT` doubling with the 520-byte guard **pre-Genesis only** | fees on a failing transaction collect nothing | opcode cap is a parameter, not a consensus constant |

This node was mid-pack on amplification **at the pin**: the caps and the deadline existed, the
deadline bounds the waiter rather than a synchronous builtin, and the conversion family was unpriced.
**The same PR that reported it priced it** (§149): the conversions and the union charge their output,
and `censure` its cube (Law 91), so the row above is history rather than today's arithmetic — and what
remains is the log factor of a sort, not a flat charge. The
structural axes — authority, encapsulation, machine-checking — are unchanged from September; the
[README's table](../../../README.md) carries them. Sui's crypto/P2P and Solana's Agave VM are unvendored, so
those cells are **could not trace**, not cleared. Market-cap figures are dropped: they age fastest and
have no gate.

## 8. Open questions

These are decisions, not defects.

- **The cost gate's blind spots, not the gate itself.** The instrument exists now
  ([the cost model](../formal/cost-model.md), §150): a register over every priced operation, a census
  that refuses an undeclared arm, and a deterministic shape test. Its first census found two more live
  instances (`set.add`, and the tallies' uncounted option lists) — which is the argument for it. What
  it still cannot see is the *work*: a `linear` row over `O(n log n)` code, a wrong declaration, and a
  mis-set constant all pass. The first is the log-factor decision below; the last is a measurement
  nobody has done (one phlo per operation is a count, not a calibrated unit).
- **The charge's shape at the margin.** The conversions are linear for `O(n log n)` work, so a phlo buys
  work bounded by `charge · log charge`. Bounded is the requirement the defect violated; an exact price
  of a comparison is a larger decision.
- **A far-future block timestamp.** The lower bound landed; the future direction is still accepted by
  design (#223). The decision is open.
- **The transaction API's amount.** Decided as a residual (§149): no API cap, and the operator caps the
  coordinator's vault. If a network later wants a sub-balance bound, it is a policy on top of the
  chain's, and it is a decision rather than a fix.
- **The rejection sets.** `rejected_deploys` is compared on replay; `rejected_blocks` and
  `rejected_senders` are hashed and carried but not compared. Show reachability or record them inert.
- **The deploy relay.** Fan-out defaults to every peer and the seen set clears at 100 000; the harm was
  not established, but the fan-out policy is a tuning decision worth stating.
- **Active-validator-set selection.** The epoch seed is drawn from the finalised fringe; a proposer
  presenting a stale reachable fringe retains a handful of choices. The residuals are named in
  [`spec/RUST-VS-SCALA.md`](../../../spec/RUST-VS-SCALA.md) §3 item 12.
- **Permissive defaults** (epoch length 0, bond minimum 0) remain reachable only on ad-hoc runtimes;
  **term-depth margin** remains a proof debt; the **fringe liveness predicate** still counts messages
  rather than senders.
- **The hard-fork schedule** for laws 87 (decimal), 88 (disposition), 89 (liveness), 90 (hostlist) and
  91 (the charge magnitudes): which network arms which, and when. **91 is the one that ships ungated
  and cannot wait for a schedule** — it changes a block's post-state hash, so every validator on a
  network must move together and every pinned genesis hash must be regenerated (§149).

## 9. The September 2026 pass (historical)

The first pass ran at `67dd6fb7b` (2026-09-28) with three lenses and a rebuilt refutation stage using
**three** verdicts (`CONFIRMED` / `NOT-RE-ESTABLISHED` / `REFUTED`). It confirmed no P0, three P1 sharing
one root cause (the gas model), four P2 and fourteen P3, left one item not re-established, and refuted
ten candidates — including a deeply-nested-protobuf stack exhaustion (the decoder's recursion limit)
and a `qucalc` overflow panic (the spawn/join boundary).

Its principal finding was that `Set.union` deduplicated through a linear scan — Θ(N²·⁴³) for a flat 13
phlo, **98 CPU-seconds at N = 40 000** — with the aggravating facts that execution-time parsing was
uncharged, nothing bounded a block's total work, and a running call could not be interrupted. Its
remediation landed days later: sort-then-dedup, the block phlo and deploy-count caps, the parse charge,
the governance bound, and a sweep of charges that did not track their work. One planned fix was
reverted because the finding did not survive implementation.

Its other confirmed items were the DAG crash wedge (since retired with the fringe-data record), the
governance saturating arithmetic, the unvalidated block timestamp (since half-closed), the accepted
quadratic DAG residency, and fourteen lower-severity items. Its open questions — the active-set seed,
the permissive defaults, the depth margin, the fringe predicate — are carried into §8.

**These numbers are true of `67dd6fb7b`.** For a current status, read
[`spec/AUDIT.md`](../../../spec/AUDIT.md), not this section.
