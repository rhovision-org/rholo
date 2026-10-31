# Testnet acceptance specification

> **Status — 2026-10-04.** The green light is **not granted**. Criterion 1 and criterion 2 are under
> re-verification against the fixes that landed on 2026-10-04 (**C209** — the attestation guard reads
> every input from the seen view — and **C210**, a refused propose arming one retry); §3.1 and §3.2 carry
> the readings, and §3.4/§3.5 the cause and its history. Criterion 3 is a census that **cannot pass by
> construction** and is unchanged from the day this page was written. This page is the owner the three
> criteria did not have.

> **Update — 2026-10-13 (W-N3-01).** The proposer health is **no longer folded into the public
> `/api/status`**. It is served at **`/api/v1/status/operator` on the admin listener**, by
> `node/src/api/admin_web_api_impl.rs::operator_status`; the public `WebApi` no longer has the data at
> all, so the fold cannot move back without moving a capability. Every "`/api/status` folds
> `proposer_health()` in" claim on this page is read with that substitution, and the ones that named
> the old path have been updated. **What does not change** is the *shared cause* those rows identify —
> one in-process `ProposeHealth`, so `/metrics`, the status surface and `/health` remain one barrier
> rather than three (C195), and the per-shard gap H-U10-04 describes is a different surface from the
> status endpoint either way. Two related moves in the same change: `/api/v1/pos/delegations` is on the
> admin listener now (W-N3-03), and `data-at-name` reads are floored at the last finalised block
> (W-N3-04). **The deploy bounds moved too**: the ingress rate limits are keyed on the peer rather than
> one bucket per node, the pool has a per-deployer share under its global cap, and a deploy term over
> 64 KiB is refused at the ingress. Every "one shared limiter" or "global, not per-sender" cell in
> §1.7 and the bow-tie sections is read with that substitution; H-U7-01's threat is the one the
> 2026-10-13 work closes.

> **Update — 2026-10-14 (coverage-gap pass).** This page cites `FAUCET_MAX_DRIPS_PER_ADDRESS` in four
> places — §1's U7 row H-U7-07, two §2 barrier rows, and §2.5's pair P9 — and that symbol **no longer
> exists in `node/src`.** The per-address drip *budget* was replaced by a **finalised-balance
> eligibility rule**: a drip is refused when the recipient's finalised REV balance already holds at
> least one drip (`node/src/api/web_api_impl.rs::finalized_rev_balance`, refusals at `:1237` and
> `:1255`), with the reservation persisted before submission. Every claim here that names the old
> constant is read with that substitution. **The substitution is not a re-derivation, and one row
> turns on that:** P9's shared cause — "the drip increment precedes the submit" — is a statement about
> a counter that no longer exists, so the barrier that shares a cause with its threat may not be the
> same barrier at all. That row is owed a re-derivation, recorded in
> `spec/audit/evidence/coverage-gap-f9521edd7.md` (E4).

> **What this page is, in one sentence.** A HAZOP worksheet and a bow-tie analysis of the node, and the
> acceptance checklist that falls out of them, written so that nothing can be marked green without a
> named configuration and a committed run.

> **What this page is not.** It is a **liveness and membership** verdict, not a **safety** verdict. All
> three criteria measure whether the net keeps producing and who may join it. None measures whether two
> validators can each finalise a *conflicting* block. Safety is analysed here as top event TE-2 and is
> deliberately left off the green light (§3, CH-ACC-05). Do not read a green light here as a safety claim.

> **A name collision, stated once.** This page lives in the **book's** `docs/src/spec/`. It is not the
> repository's `spec/` tree (`spec/AUDIT.md`, `spec/findings.tsv`, the laws). Links out of this page use
> `../../../spec/…`, which resolves to the repository's `spec/`, not the book's.

---

## 0.1 The three criteria

This page exists because of issue [#214](https://github.com/rhovision-org/rholo/issues/214)
(*Testnet viability: the three acceptance criteria, as falsifiers*, 2026-10-03), which observed that the
three requirements deciding whether this net is worth anyone's trust were carried implicitly, by closed
issues — and so had **no owner, no measurement and no falsifier**.

| | Criterion | The witness that decides it |
|---|---|---|
| **1** | Anyone can propose, and production is bounded | the deploy **finalises** (`last-finalized-block` ≥ its height) — never "the height stopped moving" |
| **2** | Validators can be dropped and joined without risk | the survivors finalise **past** the kill; the rejoiner resumes with no operator action; a new validator **bonds onto a running net** and produces |
| **3** | Every attack vector is handled | a **rate and a class census** against a bounded-adversary statement. Not a pass/fail item, and it must not be reported as one |

## 0.2 The verdict, as of the tree under audit

| Criterion | Verdict | Basis |
|---|---|---|
| 1 — bounded production | ✅ **on A1.1, A1.2, A1.4, A1.5 · ❌ on A1.3** | re-verified by this pass on `07af032ad` (§3.1): six deploys addressed to a **single** validator now finalise (R1 24, R2 23, R3 23), where the pre-fix tree gave `none` reproducibly. The contradiction this page raised is **closed**: the N=3 pass does reproduce once the guard reads the seen view. **A1.2 (N=5) now passes and A1.3 (N=8) fails**, both measured 2026-10-07 on `bbabc39e8` — the N=5 deploy finalises in 5–6 s while the N=8 chain reaches 112–120 blocks across all eight validators and never finalises, "supporting stake is not a supermajority — 400 of 800". The residual §3.1 used to count here — arms leaving their sixth deploy short, **2 of 3** on `07af032ad`, **0 of 3** on jimscarver's `f1ca009`, **1 of 3** on `513e2192b` — is **not a race and not a loss**: §3.1's own C246 correction read all twelve arms of four runs and found a quiet chain's greatest `finalized` is `tip − 4`, frozen, so a deploy finalises iff production ran at least four heights past it and the spread is **slack, not the node**. A1.5's wording was the thing that failed, and it is **restated and closed** (§3.1, 2026-10-11). Recorded in §3.1 and on #213 |
| 2 — join and leave | **5 of 5 pass; A2.2 on the live testnet** | re-verified by this pass on `07af032ad` (§3.2): a deploy submitted **after** the kill finalises (block 14 at a tip of 13); the killed validator's restart resumes it with `start` as the only action (block 18); a deploy accepted **while the validator was absent** is included *and* finalised (block 14); a new validator **bonds onto a running net** and produces (`n220-join-results.md`); and the **leave path completes and pays out** on a rig — vault `99998098` → `99998964` after the quarantine deadline (`n220-leave-results.md`, A2.5). **A2.2 passes on the live testnet** at `777953de6`: after a plain `start` the rejoiner is level with the tip within 15 s, proposes when deployed to, and its blocks finalise ([#223](https://github.com/rhovision-org/rholo/issues/223)) |
| 3 — attack vectors | ⬛ **not a pass/fail item** | every cell in §3.C3 reads `absent` against a bounded-adversary statement that does not exist. The section cannot go green by construction |

> **Handover, 2026-10-04.** The maintainer has taken the decision: **the testnet goes up as it stands**, and
> it becomes the measurement platform. The three issues that owned this work — **#214** (this page),
> **#213** and **#223** — are **closed**, and everything still open or unmeasured is collected in one
> place: **[#242](https://github.com/rhovision-org/rholo/issues/242), *Testnet residuals***.
> **This page is unchanged and remains the reference** for the criteria, the worksheet, the bow-ties, the
> checklist and the register; #242 is the live list of what to measure next.
>
> **Closing those issues is not a claim that the residue is fixed.** The verdict table above is the state
> at handover, and #242 states each item in the same terms this page does: what was measured, on what tree,
> with what witness — and what is merely owed.

## 0.3 Method

Two standard techniques, composed.

**HAZOP** (IEC 61882). The node's functions are divided into ten **study nodes**, each with a one-sentence
**design intent**. The standard **guide words** are applied to each node's properties to force a
**deviation**: `node + property + guide word → deviation`. Each deviation carries a cause, a consequence
naming the top event it feeds, the existing safeguard, and the acceptance item it bears on. Not every
guide word has a referent at every node: a word is marked **vacuous** with a structural reason, or
**folded** into another, and the discarding is recorded rather than left implicit. Applying all eleven
words to every node is malpractice, and the worksheet says which words it discarded and why.

**Bow-tie** (IEC/ISO 31010). Each top event is drawn as threats → preventive barriers → **top event** →
consequences, with mitigating barriers and **degradation factors**. Every barrier is assessed for
**effectiveness, independence and dependability**. A barrier that shares a common cause with the threat
it guards does not count, and §2.5 lists the 65 cross-node pairs where that happens.

**The challenger.** A HAZOP's authority comes from the independent leader whose duty is to contest the
design intent, and an assurance case's from the **defeater** who hunts counter-evidence. This audit names
that role explicitly — a **challenger (black-hat review)**, with licence to contest *every* assumption on
the page. The role is not decorative: **75 challenges were raised against the first-pass worksheet, and
70 of them were upheld**. The register is §4.

> **A note on the name.** The brief called this role a "red hat". That is not a term of art for an
> adversarial reviewer — in de Bono's six-hats taxonomy the red hat is *emotion*, the opposite of the
> critical role, and it is otherwise a company. The named roles this page uses are the *challenger* /
> *black hat* (structured criticism), the *red team* (NIST adversary emulation), and the *defeater* in a
> GSN assurance case. The licence is recorded here, in the study charter, which is how all three
> traditions formalise it.

## 0.4 How to read this page

1. **§0** (this section) — the verdict, the method, the legend, and the open-challenge index. A reader
   who stops here has the answer.
2. **§1** — the HAZOP worksheet: ten study nodes. **It was a first pass, and it was contested; it has
   since been corrected** (2026-10-04). A row whose ID carries ⚠ still rests on something the
   adjudication left **unestablished** — an `unmeasured` barrier, a contested consequence, an owed run —
   and §4 holds the challenge that touched it. The adversarial pass found that most rows rested on
   evidence superseded by their own fixes — which is itself the audit's largest finding.
3. **§2** — the bow-tie analyses for the four top events, and the barrier-independence findings.
4. **§3** — **the acceptance checklist.** This is the green-light list. Every row names its
   configuration, its run, its tree, its instrument and its witness, and the two rules below make a
   false green impossible.
5. **§4** — the challenge register: every objection, its resolution, and the evidence that settled it.
6. **§5** — the deferrals: the 2026 L1 criteria that are *not* green-light items, each with the
   falsifiable trigger that promotes it.

## 0.5 The status vocabulary

Each value is defined by **what a reader is licensed to conclude**.

| Status | A reader may conclude |
|---|---|
| ✅ `pass` | the claim holds on the named tree, in the named configuration, with a committed run, and the falsifier was armed |
| ❌ `fail` | a committed run contradicts the claim — a **result**, not a defect in this page |
| ⬜ `untested` | no run exists. The row is an owed measurement |
| 🟨 `reported, artefact absent` | a run is asserted but its artefact is not in this tree. Someone reports it; it is not evidence here |
| 🟪 `contested` | an upheld challenge stands against the row. The status is not reportable |
| ⬛ `out of scope` | decided outside #214's three criteria; carries a promotion trigger |

## 0.6 The two rules that make green impossible without a run

1. **`pass` requires a non-empty `Configuration`, a `Run` that resolves to a tracked file, and a `Tree`
   that resolves to a real object.** Any of the three missing, and the row is `untested` or
   `reported, artefact absent` — it cannot be `pass`.
2. **`pass` requires a positive witness.** "No blocks were produced" witnesses `fail`, never `pass`. A
   chain that stops is the defect, not the pass.

## 0.7 The evidence rule

An issue's state is not evidence. A register row marked `done` is not evidence. A file existing is not
evidence that a barrier works. **Only a committed run, or a code symbol the reader actually read, is
evidence.** Where a cell rests on a judgement with no artefact behind it, it is tagged `[unhoused]` and
printed with the tag.

## 0.8 Open challenges

The register (§4) holds **75 node-level challenges**, of which **70 were upheld** and 5 refuted, plus
**6 acceptance-level challenges** (CH-ACC-01…06) raised directly against this page's own claims. One of
those six, **CH-ACC-03, has since been withdrawn** — it rested on a stale clone, not on the repository —
and §0.9 records why. The upheld node-level set is the honest state of the analysis, and the largest
class of them is not about the node at all: it is that the worksheet was built from **evidence superseded
by the very fixes it describes**.

**All 70 upheld challenges have since been applied to §1** (2026-10-04 — the correction pass §5.1 item 3
recorded as owed). So what is open now is not the register but the **owed runs** its cells name: every
cell this pass marked `unmeasured` is a measurement nobody has taken, and §1's ⚠ now marks exactly those.

## 0.9 Corrigendum — 2026-10-03

An earlier edition of this page (PR #217, commit `0ee0c3e4f`) carried a **false claim about the
repository**, and it is the class of error this page exists to catch. It is recorded here rather than
edited away, because this register's own rule is that a withdrawn claim keeps its original text beside
its correction.

**What the page said.** That `0c6c65979` — the tree issue #214 cites for criterion 1's N=3 pass — *"is
not an object in this repository"*, on the evidence of `git cat-file -t 0c6c65979` returning
`fatal: Not a valid object name`. The claim appeared in the §0.2 verdict box, in §3.1's `A1.1` row, and
as an upheld challenge, CH-ACC-03.

**Why it was wrong.** The audit ran on tree `1e5a64ed4` from a clone that had **never been fetched**.
`origin/dev` was two merges ahead, and `0c6c65979` — the merge of PR #212, 2026-10-03 11:11 — was among
the objects the clone did not hold. The command did not fail because the object is absent; it failed
because the local object database was stale. Both halves reproduce:

```console
$ git cat-file -t 0c6c65979                        # stale clone
fatal: Not a valid object name 0c6c65979
$ git fetch origin && git cat-file -t 0c6c65979    # fetched
commit
```

**The second consequence, and the larger one.** The same stale clone meant the audit ran **without
`#215`** — *"the round gate's escape needs a clock, not just a supply of attempts"* — which merged at
**14:14 on 2026-10-03**, before the audit began at about 15:20. Criterion 2's ❌ is a measurement on
`0c6c65979` and is correct *for that tree*; it is not a statement about the tip, and §3.2 now says so.

**What this does to the audit's thesis.** The page's headline finding is that the project's account of
itself was a fix behind its tree. **The audit was a fix behind its tree**, for the same reason, and no
care applied *inside* the analysis could have caught it — the fault was in the setup, before the first
agent ran. That is worth more than an apology: it is the rule. *Fetch before auditing, and never conclude
"absent from the repository" from a command run against a clone of unknown age.* A probe's failure is not
the defect.

**What changed.** CH-ACC-03 → `refuted` (§4.1). §0.2's criterion-1 reason and §3.1's `A1.1` row now rest
on a **committed run** rather than on the issue's report: the re-probe (§3.4) ran on the tip and found the
deploy does not finalise, so criterion 1 is ❌ *with an artefact*, not 🟨 *reported, artefact absent*.
§3.2 keeps its ❌ and now says the stall is upstream of the wedge. Nothing else in the page was found to
depend on the stale tree — the worksheets were built and checked at `1e5a64ed4` and state that tree on
their face.

---

# 1. HAZOP worksheet

Ten **study nodes** — the node's functions, not its files — each with a one-sentence design intent. The
eleven standard guide words were applied per node and each word was dispositioned as a **row** (a
credible deviation), **folded** into another word, or **vacuous** at that node *with a structural reason*.
A node whose rows number fewer than three must declare its vacuous set.

> **This worksheet was a first pass, and it was contested.** A challenger with licence to contest every
> assumption read each worksheet and raised **75 objections; the adjudicator upheld 70** and refuted 5. It
> **has since been corrected** — on 2026-10-04 every upheld objection was applied to the cell it names,
> and each touched row now cites its challenge id in an HTML comment. This is the correction pass §5.1
> item 3 recorded as owed.
>
> **What ⚠ still means.** A row whose ID carries ⚠ has an element the adjudication left **unestablished**:
> a barrier whose evidence is `unmeasured`, a consequence that is contested, or a deviation owed a run.
> Those are the owed runs, and they are listed in §4. ⚠ is no longer "read its challenge before believing
> the row" — the row has been corrected; it is "this row still rests on something nobody has measured".
>
> **Most of the objections were not about the node at all.** They were that the worksheet had been built
> from **evidence superseded by the very fixes it describes**: a code path HEAD no longer has, or a run
> produced before the fix it was meant to witness. That is the audit's largest single finding, and it is
> stated plainly here rather than buried: **the repository's hazard analysis was, at the start of this
> pass, roughly one fix behind itself.** Three nodes — **U1, U2 and U3** — are the sharpest case, because
> their rows were adjudicated *before* **C209/C210** landed; their corrections are therefore
> **re-derivations against the fixed tree**, not transcriptions, and they are marked as such.



> **§1.1 — corrected 2026-10-04.** 8 adversarial challenges against this node's worksheet; **8 upheld**, all applied below — and this node's corrections, with §1.2's and §1.3's, are the three in §1 that are **re-derived rather than transcribed**, because its rows were adjudicated on a tree that predates **C209/C210** (PR #219), the fix to the very stall H-U1-01 describes. A row whose ID still carries ⚠ has an element the adjudication left unestablished; every touched row cites its challenge id in an HTML comment.

### U1 — Block production / proposal

**Design intent.** When and only when a trigger fires, the node builds at most one block per round per validator, whose number and sequence are determined by its justification set, and offers it.

**Guide words.** row: NO, MORE, LESS, AS WELL AS, PART OF, OTHER THAN, EARLY · folded: LATE → NO, BEFORE → EARLY, AFTER → MORE · vacuous: REVERSE (the node has one directed action — build-and-offer; there is no reverse flow, and "a number that goes backwards" cannot be produced because `block_num`/`seq_num` are refined non-negative types whose validation (`casper/src/validate.rs:block_number`, `:sequence_number`) refuses any parent at or above the block's own number, so the intent's direction cannot invert).

### Table A — deviations

| ID | Guide word | Deviation | Cause (anchor) | Consequence → TE | A |
|---|---|---|---|---|---|
| ⚠ **H-U1-01** | NO | **trigger** — the round rests at one height and no block is built after it: production seals and the deploy in hand is held unfinalised | **re-derived against HEAD.** The row's original cause — `attest_warranted` keyed once per *height*, so `height > last` is false for every later block — is a code path `e6d41a347` **deleted**: HEAD keys the tap **per sender** (`node/src/runtime/node_runtime.rs:1664`, `:2839-2845`), and the committed falsifier `a_round_that_comes_to_rest_at_one_height_is_answered_for_every_peer` asserts **seven** answers at the resting height. The surviving mechanism is the **attestation guard**: `cadence_due` is read against a tip that the round's rest freezes, so the guard suppresses in perpetuity — the chain-level stall root-caused in §3.5 and fixed by **C209 + C210** (`casper/src/blocks/proposer/proposer.rs:attestation_suppressed`) | the chain seals at that height with the deploy held unfinalised → TE-1 | A1.1b |
| ⚠ **H-U1-02** | MORE | **count-per-round** — a validator emits a second block at the same `(sender, seq_num)`, i.e. equivocates with itself | `block-storage/src/dag/message_state.rs:has_advanced_past_the_round` compares `latest_msgs[sender].sender_seq` against `round_parents[sender].sender_seq`, and the stale-snapshot path (`casper/src/blocks/proposer/proposer.rs:validate_block` returning `ValidateError::SelfEquivocation` when the DAG insert carries `crate::dag::EQUIVOCATION_PREFIX`) admits a second proposal when the DAG advanced between the parent-set read and the insert (AUDIT §48) | a peer holds two signed blocks at one sequence number and records a slashable equivocation → TE-2 | A2.1a |
| ⚠ **H-U1-03** | LESS | **liveness floor** — escaped rounds move the tip but are documented not to advance the fringe, so offered blocks accumulate unfinalised faster than finality retires them | `block-storage/src/dag/message_state.rs:parents_for_new_block_escaping` ("**It may not advance the fringe**, and that is the accepted cost") taken by `casper/src/blocks/proposer/proposer.rs:create_block`'s `escape` branch | **contested as a growth driver.** The Θ(N²) residency is real and separately sourced (TE-4), but the escape is not shown to be its cause: `n148-results.md` attributes the growth it measured to the **storm** (production never released after a kill), a distinct mechanism → TE-4 | A1.1a |
| ⚠ **H-U1-04** | AS WELL AS | **content** — the block attaches a `Slash` system deploy for an offender its own bonds map no longer holds, re-attached on every block | `casper/src/blocks/proposer/proposer.rs:add_recorded_equivocations` gates the equivocation arm on `bonded`, which is `compute_bonds(pre_state_hash)` — the merged pre-state — and that read is a constant the merge never moves (C201: 133 parent sets, one pre-state hash; 59 re-slashes measured) | **contested / re-derived.** "Re-attached on every block" is a **conditional symptom of the frozen finality** (H-U1-01/03), not a per-block production deviation, and C201's resolution is that the frozen read is a **coupling** to finality rather than a defect: no node stopped proposing, and the quorum path is the same coupling → TE-1 | A2.1a |
| **H-U1-05** | PART OF | **offering** — the block is built, self-validated and inserted into the DAG, but the broadcast leg is fire-and-forget: a failed `send_block_hash` is neither surfaced nor retried, so the proposal reaches no peer while the node logs it as proposed | the callee returns **no status** and logs success unconditionally (`casper/src/protocol/comm_util.rs:182-186`); the send error is swallowed one frame down in `send_to_peers` (`comm_util.rs:83`), which drops `TransportLayer::broadcast`'s `Vec<CommErr<()>>` (`comm/src/transport/transport_layer.rs:18`) | no peer hears the hash, so the round cannot close on this block while this node believes it proposed → TE-1 | A1.1b |
| ⚠ **H-U1-06** | OTHER THAN | **content** — with `--autopropose` and a dev-mode deployer key, an empty pool injects a signed dummy `Nil` deploy, so the block carries a deploy that is not the pool's | `node/src/runtime/node_runtime.rs:dummy_deploy_opt` (built only when `dummy_deploy_key(conf.autopropose, conf.dev.deployer_private_key)` is `Some`) drives the dummy-deploy branch in `casper/src/blocks/proposer/proposer.rs:create_block` | production becomes wall-clock-bounded rather than content-bounded: the timer mints a block every 2 s with no ceiling, so the chain grows unbounded → TE-4 | A1.1a |
| ⚠ **H-U1-07** | EARLY | **escape timing** — the liveness escape matures on a local *attempt* count (one per decline), not on the DAG's `LIVENESS_WINDOW` heights of real silence, so a round can be escaped and its tip moved before the round's own clock would retire the quiet sender | `casper/src/blocks/proposer/proposer.rs:create_block`'s `escape` block increments `blocked_since_advance` once per declined proposal; **the increment cadence is the propose-trigger cadence** — per validated block with `--autopropose` on, per *remote* validated block (the attest tap, `node/src/runtime/node_runtime.rs:1637-1690`) with it off; the 2 s timer is not spawned at all when autopropose is off (`:1556` gates `:1597`). The round's clock is `block-storage/src/dag/liveness.rs:LIVENESS_WINDOW` (5 heights — frozen while the round cannot close) | the tip is moved on a subset before the round's clock agrees the sender is absent, retiring a merely-lagging validator from the live set and shrinking the partition → TE-2 | A2.1a |

### Table B — barriers

| Deviation | Preventive barrier (anchor) | Evidence | Independence |
|---|---|---|---|
| ⚠ **H-U1-01** | the liveness escape in `casper/src/blocks/proposer/proposer.rs:create_block` — bounded by `waited <= rchain_block_storage::dag::liveness::LIVENESS_WINDOW`, it moves the tip and lets the round's own clock age the quiet sender out | `unmeasured` for this barrier's own arm — the cited `n149-results.md` predates C209/C210 and witnesses the pre-fix seal, not the escape | `[self-referential]` — the escape's counter increments on a *declined proposal*, and the state it must escape is exactly one where `has_advanced_past_the_round` vetoes proposals (`proposer.rs:708`), so counter and veto read the same round state | <!-- CH-U1-01 upheld -->
| ⚠ **H-U1-02** | `casper/src/blocks/proposer/proposer.rs:create_block` returns `AlreadyProposedThisRound` while `has_advanced_past_the_round`; `casper/src/validate.rs:sequence_number` refuses `creator_latest_seq + 1 != seq_num` at every receiver | `unmeasured` | `[one-surface]` — the veto has no call site outside the proposer (`message_state.rs:176`, `proposer.rs:708`) and the receiver's `sequence_number` is the sole distributed surface, so one surface carries both halves | <!-- CH-U1-08 upheld -->
| ⚠ **H-U1-03** | the liveness rule `block-storage/src/dag/liveness.rs:live_weight_set` retires an absent sender from the partition so the round can still close | `unmeasured` for the escape mechanism — the cited `n148-results.md` does not contain this barrier and attributes the growth it measured to the storm | `[shared-cause]` — the same frozen tip that blocks the fringe also freezes the clock the retirement is measured from | <!-- CH-U1-03 upheld -->
| ⚠ **H-U1-04** | the `bonded` filter in `casper/src/blocks/proposer/proposer.rs:add_recorded_equivocations` plus the receiver re-check `casper/src/validate.rs:equivocation_is_proved` | `spec/audit/evidence/c201-proposer-read-results.md` | `[self-referential]` — both gate on the same `compute_bonds(pre_state_hash)` view that C201 shows is frozen | <!-- contested -->
| **H-U1-05** | `n/a — no barrier`: the proposer veto (`has_advanced_past_the_round`) and the receiver's `sequence_number` guard the *duplicate* this row's sibling names, but nothing in `propose_effect` surfaces or retries a failed broadcast | `unmeasured` | `n/a — no barrier` | <!-- CH-U1-08 upheld -->
| ⚠ **H-U1-06** | the `conf.autopropose` + deployer-key gate in `node/src/runtime/node_runtime.rs:dummy_deploy_key` confines the dummy deploy to a devnet | `unmeasured` — the gate is not what `n148-results.md` measures | `[shared-cause]` — the barrier's condition of effectiveness *is* the deviation's precondition (the flag), so one event both is the guarded occurrence and removes the barrier; there is no independent preventive barrier on an `--autopropose` devnet | <!-- CH-U1-07 upheld -->
| ⚠ **H-U1-07** | the bound `waited <= rchain_block_storage::dag::liveness::LIVENESS_WINDOW` in `casper/src/blocks/proposer/proposer.rs:create_block` | `unmeasured` | `[self-referential]` — the bound counts local attempts while the property it stands in for is measured in heights the deadlock freezes | <!-- CH-U1-04 upheld -->

### Provenance

read: `casper/src/blocks/proposer/proposer.rs` (`AlreadyProposedThisRound`, `suppress_attestation`, `attestation_suppressed`, `cadence_due`, `moving_attestation_stake`, `attestation_reaches_supermajority`, the `escape` block, `create_block`, `block_system_deploys`, `add_recorded_equivocations`, `propose_effect` closure) · `casper/src/blocks/proposer/block_creator.rs` · `casper/src/blocks/proposer/propose_result.rs` · `casper/src/blocks/proposer/mod.rs` · `casper/src/validate.rs` (`block_number`, `sequence_number`) · `block-storage/src/dag/liveness.rs` · `block-storage/src/dag/message_state.rs` (`has_advanced_past_the_round`, `parents_for_new_block_escaping`) · `node/src/runtime/node_runtime.rs` (`AUTOPROPOSE_INTERVAL`, `AUTOPROPOSE_MAX_CONSECUTIVE_FAILURES`, `attest_warranted`, the autopropose timer/tap, `propose_effect`) · `node/src/instances/proposer_instance.rs` · `spec/audit/evidence/n148-results.md` · `spec/audit/evidence/n149-results.md` · `spec/findings.tsv` (C200, C201) · `spec/audit/evidence/c201-proposer-read-results.md` · `casper/src/blocks/proposer/proposer.rs:attestation_guard_tests` · `the study charter (outside the repo)` (top events, acceptance items).
ran: `git rev-parse HEAD` → `1e5a64ed4149d5bd7fa15affb1022e3c89f6c492` (matches the audit tree); `git status --porcelain` → `?? .claude/` only (`dirty: false` for tracked files); `grep`/`sed` reads of the files above.
not read: `block-storage/src/dag/dag_storage.rs` (`insert` / `recorded_equivocations` implementation, only the call sites in proposer.rs and the C200 row) · `casper/src/dag.rs` (`EQUIVOCATION_PREFIX` string, only referenced) · `docs/src/node/{testnet,scaling,validator-requirements,operating,consensus}.md` · `spec/AUDIT.md` · `spec/audit/passes.md` · the full evidence bodies of `spec/audit/evidence/n148-*` and `n149-*` (headlines only).



> **§1.2 — corrected 2026-10-04.** 7 adversarial challenges against this node's worksheet; **6 upheld, 1 refuted**, all applied below. Like §1.1, several of these rows were adjudicated on a tree that predates **C209/C210**, so H-U2-03's mechanism is re-derived rather than transcribed. A row whose ID still carries ⚠ has an element the adjudication left unestablished; every touched row cites its challenge id in an HTML comment.

### U2 — Attestation, round closure, quorum

**Design intent.** A validator answers a remote block at most once per sender per height; a round
closes only when every live bonded sender has a message above the boundary; the quorum that closes it
is a strict supermajority of the whole bonded map.

**Guide words.** row: MORE, LESS, LATE, OTHER THAN · folded: AS WELL AS → MORE, BEFORE → LESS,
EARLY → LESS, NO/NOT → LATE, AFTER → LATE, **PART OF → OTHER THAN** · vacuous: **REVERSE (contested** —
the staleness arithmetic is *not* confined to `heights_behind` and the denominator's pinning is a
call-site choice rather than a signature invariant of `calculate_fringe`, so "no reversed path exists to
deviate" is unproven rather than established**)**.

> **PART OF is folded, not vacuous** (CH-U2-04). The sheet claimed "no path closes a round on a proper
> subset" because both closure predicates are set-equality. That is true of
> `check_min_messages` (`minimum_senders == bonded_senders`, `block-storage/src/dag/finalizer.rs:146`) but
> **not** of `advance_round`, whose `None` arm retires a never-spoken bonded sender once
> `tip - round_height > LIVENESS_WINDOW` — the round *does* close on a proper subset, and H-U2-04 is that
> deviation. The vacuity marker was the odd one out.

### Table A — deviations

| ID | Guide word | Deviation | Cause (anchor) | Consequence → TE | A |
|---|---|---|---|---|---|
| ⚠ **H-U2-01** | MORE | **configuration-gated: this deviation requires `--autopropose` AND the dev-mode deployer key, neither of which is a node default, so it cannot fire on the shipped configuration.** Given both, the node attests at every height instead of at its own quiet's cadence — production is not bounded | a reachable quorum's suppression is `!(new_state_transition \|\| cadence_due)`, and the injected dummy `Nil` deploy pins `new_state_transition` true whenever the pool is empty, so the term never suppresses (`casper/src/blocks/proposer/proposer.rs:attestation_suppressed`; dummy at `proposer.rs:dummy_deploy_opt`) | unbounded unfinalised production, ≈2.9 blocks/s with every validator live → TE-4 | A1.1 | <!-- CH-U2-01 upheld; CH-U2-02 upheld -->
| ⚠ **H-U2-02** | LESS | the node answers fewer remote (sender,height)s than it marked answered — a failed enqueue is recorded as answered and that height is never retried | the tap writes `answered.insert(sender, height)` *before* `tap_tx.try_send(...)`, so a full propose-queue drops the request while the height stays marked (`node/src/runtime/node_runtime.rs:attest_on_new_blocks`) | **re-scoped — a single drop is a one-height delay, not a permanent loss.** Restated as a LATE shape (one height behind, H-U2-03's family); the TE-1 "never produced" reading needs the enabling condition — a **persistently** full propose queue — which is `unmeasured` → TE-1 | A1.1 | <!-- CH-U2-06 upheld -->
| ⚠ **H-U2-03** | LATE | after a live bonded validator is lost, the round closes later than the design requires — in practice never, on the guard-live arm | `advance_round` retires a quiet sender only when `heights_behind(tip, height) > LIVENESS_WINDOW`, measured from a tip that `has_advanced_past_the_round` freezes; the escape's `blocked_since_advance` increments only on a proposal *request*, and with `--no-autopropose` the only requester is the attest tap, which stops once no new remote block arrives (`block-storage/src/dag/message_state.rs:has_advanced_past_the_round`; `casper/src/blocks/proposer/proposer.rs:propose` escape; `node/src/runtime/node_runtime.rs:attest_warranted`). **Re-derived:** the guard-live freeze is C209's `cadence_due`-against-frozen-tip coupling, fixed by **C209 + C210** | the round never closes → finality freezes while production continues → TE-1 | A2.1 | <!-- CH-U2-03 upheld -->
| H-U2-04 | OTHER THAN | the round waits for a sender *other than* one above the boundary — one with no message at all — and by a different clock than the fringe gate uses for the same notion | `advance_round`'s `None` arm retires a never-spoken bonded sender only when `tip - round_height > LIVENESS_WINDOW`, while `live_weight_set`'s `is_some_and` drops a never-spoken sender at once; a validator that bonds mid-chain and has not yet produced therefore holds closure on a clock the veto freezes (`block-storage/src/dag/message_state.rs:advance_round`; `block-storage/src/dag/liveness.rs:live_weight_set`) | a newly bonded validator stalls round closure before it speaks → TE-1, and bears on the join path | A2.3 |
| **H-U2-05** | PART OF | the round closes on a **proper subset** of the bonded map — the quiet senders retired by the live-set window are not waited for, so a boundary is taken that fewer than every bonded sender has seen | `block-storage/src/dag/message_state.rs:advance_round`'s `None` arm over `bonds.keys().all(...)`, which admits `round_parents ⊊ bonds.keys()` once a sender has aged out (`block-storage/src/dag/liveness.rs:LIVENESS_WINDOW`) | the closing partition is smaller than the map the fringe denominator sums over — the H-U3-02 trade seen from the round's side → TE-1 | A2.3 | <!-- CH-U2-04 upheld -->

### Table B — barriers

| Deviation | Preventive barrier (anchor) | Evidence | Independence |
|---|---|---|---|
| ⚠ **H-U2-01** | **none named.** `cadence_due` is struck: it is inert under this deviation's premise (the dummy deploy pins `new_state_transition` true, so the `\|\|` short-circuits it to `false` — it cannot suppress what it is cited to prevent) | `spec/audit/evidence/n149-results.md` is this deviation's **own demonstration** (the control arm: 857 blocks / 300 s, 856 carrying a deploy), not barrier evidence | `[barrier-is-the-threat]` — with `cadence_due` struck there is no independent preventive barrier at all; the per-sender bound that bounds a burst is the same rule that seals the round (C192) | <!-- CH-U2-01 upheld -->
| ⚠ **H-U2-02** | none: the `answered` map that bounds a burst is also what loses the attestation; only the warn log is left (`node/src/runtime/node_runtime.rs:attest_on_new_blocks`) | `unmeasured` — no committed run exercises the tap's queue-full drop | `[barrier-is-the-threat]` | <!-- contested -->
| ⚠ **H-U2-03** | the bounded escape, `parents_for_new_block_escaping` taken after `LIVENESS_WINDOW` declined attempts (`block-storage/src/dag/message_state.rs:parents_for_new_block_escaping`; `casper/src/blocks/proposer/proposer.rs:propose`) | `spec/audit/evidence/n149-results.md`'s **primary (guard-live) arm** is the freeze; the exact "guard-live **and** a validator lost" scenario is **`unmeasured`** — no committed run combines the two. `n148-results.md` is struck (it is the autopropose arm, where production *continues*, and `#213` is struck as an open issue rather than an artefact) | `[self-referential]` — the escape's clock and the round's clock are the same frozen tip | <!-- CH-U2-03 upheld -->
| H-U2-04 | the same window retirement, `tip - round_height > LIVENESS_WINDOW`, closes the boundary once the joiner is retired (`block-storage/src/dag/message_state.rs:advance_round`) | `spec/audit/evidence/n220-join-results.md` — a validator is bonded onto a running net and the boundary closes; the *silent*-joiner arm is now measured too (`n220-silent-join-results.md`: the three live validators finalise past the bond, 7 → 10) | `[self-referential]` |

### Provenance

read: `node/src/runtime/node_runtime.rs` (the attest tap `attest_on_new_blocks`, `attest_warranted`, `tap_validated_blocks`, the autopropose/timer taps, `attest_warranted_tests`) · `casper/src/blocks/proposer/proposer.rs` (`ProposeSource`, the attestation guard region, `moving_attestation_stake`, `cadence_due`, `attestation_suppressed`, `attestation_reaches_supermajority`, `attestation_guard_tests`, `attestation_suppression_tests`, the round-veto escape) · `block-storage/src/dag/liveness.rs` (whole file) · `block-storage/src/dag/finalizer.rs` (whole file) · `block-storage/src/dag/message_state.rs` (`advance_round`, `parents_for_new_block[_escaping]`, `has_advanced_past_the_round`, `a_round_closes_only_when_every_bonded_sender_has_spoken`) · `sdk/src/consensus.rs` (whole file) · `casper/tests/finalization.rs` (`the_round_closes_when_a_validator_goes_quiet_inside_the_window`, `a_second_proposal_in_a_round_keeps_its_sequence`) · `spec/findings.tsv` (rows C171/C174/C192/C193/C195/C188) · `spec/audit/evidence/n149-results.md` · `spec/audit/evidence/n148-results.md` · `spec/audit/evidence/n148-preregistration.md` · the study plan `the study charter (outside the repo)`
ran: `git rev-parse HEAD` → `1e5a64ed4149d5bd7fa15affb1022e3c89f6c492` (matches the tree under audit); `git status --porcelain` → only `?? .claude/` (untracked worktrees; no tracked modification); symbol greps over `node/ casper/ block-storage/ sdk/` and `spec/audit/evidence/`
not read: `casper/src/blocks/proposer/block_creator.rs` (only the `suppress_attestation` branch was grepped) · `docs/src/node/{testnet,scaling,operating,consensus}.md` · `spec/AUDIT.md`, `spec/audit/passes.md`, `spec/TYPE-SYSTEM.md` · the issue bodies of `#213`/`#214` (cited by number, not opened) · the full `casper/src/validate.rs`

**Note on H-U2-03's anchor.** `casper/tests/finalization.rs`'s doc for
`the_round_closes_when_a_validator_goes_quiet_inside_the_window` says the test "is `#[ignore]`d" and "red
today", but the tree carries a bare `#[test]` (no `#[ignore]`) and the escape it drives landed in the same
commit (`6c37a23f5`). I did not run the suite to decide which is true; the node-level wedge is anchored to
`#213` and `n148-results.md`, not to this test.



> **§1.3 — corrected 2026-10-04.** 8 adversarial challenges against this node's worksheet; **6 upheld, 2 refuted**, all applied below. Like §1.1 and §1.2, this node is not a transcription: H-U3-01's frozen gate and H-U3-04's window are the surface **C209/C210** fixed, and §3.5 assigns the root cause. A row whose ID still carries ⚠ has an element the adjudication left unestablished; every touched row cites its challenge id in an HTML comment.

### U3 — Finality / fringe / estimator

**Design intent.** The fringe advances only over a full partition whose every member has seen every
other, and a finalised block is never undone.

**Guide words.** row: NO/NOT, LESS, AS WELL AS, LATE, REVERSE, **EARLY** · folded: MORE → AS WELL AS, PART OF → AS WELL AS, OTHER THAN → AS WELL AS, AFTER → LATE, BEFORE → EARLY · vacuous: **none — the sheet's EARLY/BEFORE vacuity is contested** (CH-U3-03).

> **EARLY is not vacuous, and BEFORE's stated reason falls** (CH-U3-03). The sheet said the estimator is a
> *pure function* with "no merge scheduler, timer or ordered stage", so "too early" has no clock referent.
> That is true of `calculate_finalization_detailed` alone, and false of the node: there **is** an ordered
> sequence — insert → round boundary → parent set → gate — and "a step running before another" has a
> referent in it. The EARLY referent is recorded as **H-U3-06** below; BEFORE is folded into it.

### Table A — deviations

| ID | Guide word | Deviation | Cause (anchor) | Consequence → TE | A |
|---|---|---|---|---|---|
| ⚠ **H-U3-01** | NO/NOT | the fringe does not advance at all — finality pins while the chain keeps producing | `block-storage/src/dag/finalizer.rs:calculate_next_fringe_support_map` requires a candidate's parents to reach *beyond* the next layer (`parents ∖ next_layer_ids`), so a DAG that rests at one height yields `full_partitions: 0, supporting: 0` (`block-storage/src/dag/finalizer.rs:calculate_fringe_numbers`) — the gate refuses before any quorum test, and a fringe needs *new* layers to advance | finality frozen, production continues → **TE-1** | A1.1, A2.2 | <!-- contested -->
| **H-U3-02** | LESS | **re-classified: a design constraint, not a deviation from intent** — the live partition is *smaller* than the bonded set while the quorum denominator is the whole bonded map, because a permanent loss ≥ 1/3 of stake must stop finality rather than be papered over by a shrinking denominator | `block-storage/src/dag/liveness.rs:live_weight_set` shrinks the partition to validators within the window; `block-storage/src/dag/finalizer.rs:calculate_fringe` sums `total_stake` over `quorum_bonds` (the **whole** map). This is the safety–liveness trade of **Law 52a/52b**, decided in §59 — the denominator *cannot* shrink because no inactivity leak exists, and that is the design | the consequence stands (finality cannot resume without a live super-majority; a rejoining validator is *required*, not optional) → **TE-1 / TE-3** | A2.1, A2.2 | <!-- CH-U3-06 upheld -->
| ⚠ **H-U3-03** | AS WELL AS | a retired (never-evicted) sender's message is carried **inside `mv.parents`** — resolved through the *full* `msg_map` even after the sender is retired from the live set — so it becomes an extra seer the partition never chose | **re-worded off the justification set onto `mv.parents`.** The original locution ("carried in the justification set … becomes an extra seer") is refuted: `live_justifications` (`block-storage/src/dag/liveness.rs:169-174`) filters the *justification set* to live senders. What survives is the parent-walk: `block-storage/src/dag/finalizer.rs:calculate_next_fringe_support_map` resolves `mv.parents` through the full `msg_map`, landing a retired sender in a `seen_by` value and failing `calculate_fringe`'s `seen_by.values().all(\|v\| v == &must_be_seen)` | no candidate is ever a full partition; the gate reads `supporting: 0` → **TE-1** — the residual is **`unmeasured`** | A1.1, A2.2 | <!-- CH-U3-01 upheld -->
| ⚠ **H-U3-04** | LATE | a stopped validator is retired only after `LIVENESS_WINDOW` heights — measured from a tip the veto pins | **cause scope narrowed (CH-U3-07).** The original clause ("the tip cannot advance while the round is open") is refuted as universal. The reachable case is narrower and it is the one the test constructs: *once every surviving sender has already spoken this round*, the proposer's veto `has_advanced_past_the_round` (`casper/src/blocks/proposer/proposer.rs:708`) stops any proposal, so the tip freezes and `advance_round`'s `tip - height > LIVENESS_WINDOW` never elapses (`block-storage/src/dag/message_state.rs:advance_round`; `block-storage/src/dag/liveness.rs:LIVENESS_WINDOW`) | in the all-survivors-spoken state, production and finality both stop → **TE-1** | A2.1, A2.2 | <!-- CH-U3-07 upheld -->
| ⚠ **H-U3-05** | REVERSE | the estimator can publish an *older* fringe than its parents carried — the flip to a previous history | the published fringe is `new_fringe_opt.unwrap_or(parent_fringe)` (`block-storage/src/dag/message_state.rs:create_message`) and `parent_fringe` is `message_map::latest_fringe` over the **live** justifications only (`block-storage/src/dag/liveness.rs:calculate_finalization_detailed` builds `live_justifications`); retiring the validator that carries the freshest fringe restarts the derivation from an older carried fringe (`block-storage/src/dag/message_map.rs:latest_fringe`). **The law-15 appeal is re-worded (CH-U3-05):** law 15 holds the derived/published fringe above the previous one *per sender under the ingress rules* and names the "publish below previous fringe" shape (`the_fold_can_publish_below_the_previous_fringe`), refused by the **sequence rule**. Whether the shrinking-live-set path can publish below *without* breaking that rule is the residual question and is **`unmeasured`** | a block declares a fringe below its parents' → **TE-2** | — | <!-- CH-U3-05 upheld -->
| **H-U3-06** | EARLY | **the round boundary is taken before every bonded sender has advanced past it** — `round_parents` is a set-of-one (a stack, not a set) and the fringe gate refuses the candidate it hands on | `block-storage/src/dag/message_state.rs:80-82` — the boundary is taken over whatever has arrived, so `round_parents` can hold a single sender where the gate needs a partition; the preventive barrier is the wait-for-all-bonded-senders rule in `advance_round` (`bonds.keys().all(...)`, `:107`), and the refusal is the same one H-U3-01 records from the gate's side | the fringe gate answers `150 of 250` — no full partition → **TE-1** | A1.1 | <!-- CH-U3-03 upheld -->

### Table B — barriers

| Deviation | Preventive barrier (anchor) | Evidence | Independence |
|---|---|---|---|
| ⚠ **H-U3-01** | the round snapshot the proposer justifies — `block-storage/src/dag/message_state.rs:parents_for_new_block` hands the gate a cross-sender parent set with a `∖ next_layer` remainder | `casper/tests/finalization.rs::a_round_snapshot_of_the_latest_messages_is_what_the_gate_needs` (symbol read) | `[shared-cause]` with U2/U1 — whether *any* new block exists is the round/tap's decision, so one event disables the barrier and is what it guards | <!-- contested -->
| ⚠ **H-U3-02** | **none** — a permanent >1/3 loss is a deliberate stop; the partition shrinks but the denominator cannot (no inactivity leak exists) | `spec/audit/evidence/n148-results.md` (killing a validator freezes finality, 3/3) | — | <!-- CH-U3-06 upheld -->
| ⚠ **H-U3-03** | the liveness filter that restricts the justification set to live senders before the gate (`block-storage/src/dag/liveness.rs:calculate_finalization_detailed`, `live_justifications`) | `casper/tests/finalization.rs::a_validator_that_spoke_and_then_stopped_does_not_cap_the_fringe`; the residual (a retired sender inside `mv.parents` resolved through the full `msg_map`) is `unmeasured` on this tree | `[one-surface]` — the filter bounds the **justification-set** surface only; the `mv.parents` walk is a different surface and the filter does not reach it (the cross-node note that this mechanism also touches U2's live set may be kept as a note, not as this barrier's label) | <!-- CH-U3-08 upheld -->
| ⚠ **H-U3-04** | the escaping parent set — `block-storage/src/dag/message_state.rs:parents_for_new_block_escaping` moves the tip so the window ages the quiet sender out | `casper/tests/finalization.rs::the_round_closes_when_a_validator_goes_quiet_inside_the_window` | `[self-referential]` — the escape and the window both measure against a tip the deadlock itself freezes | <!-- contested -->
| ⚠ **H-U3-05** | the walk's cutoff — `block-storage/src/dag/finalizer.rs:self_parents` lets a minimum message move only along its own unfinalised chain; it bounds the walk but not its starting point | `unmeasured` | `[unhoused]` | <!-- contested -->

### Provenance

read: block-storage/src/dag/finalizer.rs, block-storage/src/dag/liveness.rs, block-storage/src/dag/message_state.rs, block-storage/src/dag/message_map.rs, block-storage/src/dag/metadata_store.rs, casper/tests/finalization.rs (partial), docs/src/node/consensus.md, spec/findings.tsv, spec/audit/passes.md (§26, §32, §34, §35), spec/audit/evidence/n149-results.md, spec/audit/evidence/n148-results.md, spec/audit/evidence/n127-liveness-results.md, spec/audit/evidence/participation-lag-results.md, the study charter (outside the repo) · ran: `git rev-parse HEAD`, `git status --porcelain`, `git cat-file`/`grep`/`sed`/`ls` (read-only) · not read: spec/Rchain/Casper/Fringe.lean (law 15 cited from doc comments), spec/Rchain/Casper/Dag.lean (law 14b), spec/RChain/Laws.lean, casper/src/multi_parent_casper.rs, casper/src/blocks/proposer/proposer.rs, node/src/runtime/node_runtime.rs, node/src/web/http.rs, rspace/src/merger/



> **§1.4 — corrected 2026-10-04.** 7 adversarial challenges against this node's worksheet; **7 upheld, 0 refuted**, all applied below. A row whose ID still carries ⚠ has an element the adjudication left unestablished (unmeasured, or owed a re-run); every touched row cites its challenge id in an HTML comment.

### U4 — Merge & DAG search

**Design intent.** The merge of a branch set is deterministic, order-independent, and declines safely when its budget is exhausted.

**Guide words.** row: MORE, LESS, AS WELL AS, PART OF, OTHER THAN, LATE · folded: NO/NOT → MORE, BEFORE → LATE, AFTER → LATE · vacuous: REVERSE (the outcome is a `min` over a fully materialised `BTreeSet`, so the order the branches arrive in cannot change it — there is no order to reverse), EARLY (the search is a synchronous pure function with no scheduler and no clock; the budget is metered in work units, not time, so nothing can fire "too soon")

### Table A — deviations

| ID | Guide word | Deviation | Cause (anchor) | Consequence → TE | A |
|---|---|---|---|---|---|
| ⚠ **H-U4-01** | MORE | search steps/options exceed `SearchBudget::NODE` (10,000,000 / 1,000,000); the search returns **no answer** and the block is dropped | `sdk/src/dag/merging.rs:SearchBudget::NODE` — the constant is *provisional* and its own doc says it was calibrated against the pre-quotient **accepted-set** unit, which the rejected-set quotient changed; `max_options` bounds only the result set, so a matching-shaped relation (m disjoint pairs ⇒ 2^m terminal states) can breach it before the step bound | search returns `Err` → `MergeScope::merge` maps it to `String` → `ValidateError::Internal` → a **drop with no re-queue**, so the node cannot advance at that height until the scope shrinks → TE-1 | A1.2 | <!-- contested -->
| H-U4-02 | LESS | the directed quotient returns **fewer rejection options** than the exact enumeration, so a different (cheaper) rejection is chosen than a peer's | `sdk/src/dag/merging.rs:enumerate_rejection_sets` merges any two states sharing one rejection union on the claim they have *identical futures*; a collapse that merged states with different futures would drop an option | the merge result is consensus-visible (law 17), so a short option set picks a different rejection → a node computes a different merge than its peers → TE-2 | — |
| ⚠ **H-U4-03** | AS WELL AS | resolution runs over the block-level **native-write** relation *as well as* the event-log conflicts, and a block is rejected whole when either relation conflicts | `casper/src/merging.rs:NativeRelations::conflicting` widens each chain's relation by ancestry-derived native writes (#83), and `casper/src/merging.rs:NativeRelations::reject_whole_blocks` propagates one rejected chain to its whole block | if the widened relation misses a concurrent-writer pair, `casper/src/merging.rs:MergeScope::fold` returns **no** `Err` — a relation omission is silent (last-writer-wins / a dropped native write), not caught → TE-1 | A1.2 | <!-- CH-U4-03 upheld -->
| ⚠ **H-U4-04** | PART OF | only the **search** is metered: the mergeable-overflow fold runs over the whole conflict set with no budget | `sdk/src/dag/merging.rs:fold_rejection` / `sdk/src/dag/merging.rs:traverse_tree` take no `SearchBudget`, and `traverse_tree` walks the dependency map with **no visited set** (the fragility the `NativeRelations` comment names: a self-loop makes "the merge never return") | work and residency proportional to the scope are paid outside any bound → a wide scope grows the heap the search budget exists to cap → TE-4 | A1.1 | <!-- contested -->
| ⚠ **H-U4-05** | OTHER THAN | the map handed to the search is **directed**, other than the symmetric/irreflexive shape the fast path's precondition requires, so the fast path declines on every real merge | `sdk/src/dag/merging.rs:resolve_conflict_set_with_census` builds `full_conflicts_map` by unioning each key's transitive dependency closure (`with_dependencies`) into its conflict set, which makes the relation asymmetric (measured 376–653 asymmetric pairs at 33–43 chains, `spec/audit/evidence/n117-after-fix-results.md:45-49`, corroborated by `n117-heap-profile-results.md:99`; `n127-directed-results.md` shows only the 9-chain null, 28/27 asymmetric) — **contested-pending-repoint (CH-U4-01)**; the mechanism claim stands | the fallback path is the one the node actually runs; before C178's quotient it expanded up to 1,663,395 states per merge at the 33–43-chain widths reached → TE-4 | A3 | <!-- CH-U4-01 upheld -->
| **H-U4-06** | LATE | the bound is consulted only **after** the scope-wide preprocessing, so a refusal comes later than the cost it was meant to avert | `casper/src/merging.rs:MergeScope::merge` builds `compute_relation_map_for_merge_set` (@1611) and **calls** `sdk/src/dag/merging.rs:resolve_conflict_set_with_census` (@1618), which builds the search's `full_conflicts_map` (a `with_dependencies` closure per key, @927-933) **before** `search` applies any budget | a wide scope pays cost and memory proportional to keys × closure before any budget check; the guard is applied after the work it should precede (Law 55's shape) → TE-4 | A1.1 | <!-- CH-U4-07 upheld -->

### Table B — barriers

| Deviation | Preventive barrier (anchor) | Evidence | Independence |
|---|---|---|---|
| **H-U4-01** | the budget is applied **before each unit of search work** — `the_work_never_exceeds_the_budget`, `spec/Rchain/Bounded.lean` (Law 55, `proved-model`) | `sdk/src/dag/merging.rs:a_budget_refuses_without_answering_and_never_changes_the_answer` @1204 (`SearchBudget::NODE` @1238, zero budget @1249 — observes the refusal); the previously cited `rejection_options_are_bounded_on_a_directed_shape` runs `SearchBudget::UNBOUNDED` and is H-U4-05's output-bound witness | [barrier-is-the-threat] — the budget's guard *is* the refusal that produces the row's drop (TE-1); its protective target is work/memory, not the dropped block | <!-- CH-U4-02, CH-U4-05 upheld -->
| H-U4-02 | the exactness differential against a literal Scala-shaped enumeration — `rejection_options_match_a_literal_enumeration` | `sdk/src/property_tests.rs:rejection_options_match_a_literal_enumeration` (compares shipped vs literal on symmetric **and** arbitrary directed/self-conflicting maps) and `sdk/src/dag/merging.rs:the_directed_path_quotients_states_by_their_rejection_union` | [shared-cause] — the oracle is a transcription of the same algorithm; a shared misreading passes both |
| ⚠ **H-U4-03** | the native relation is derived from the DAG's own ancestry — `casper/src/merging.rs:NativeRelations::conflicting` / `::fold` — so every node computes it identically; reclassified **agreement-only**, with no independent completeness check | `spec/audit/evidence/c207-merge-loses-native-writes.md` documents the *threat* (a lost native write), not the barrier; the barrier needs a positive falsifier (a scope whose relation omits a concurrent pair and the merge proceeds) — **unmeasured / does not exist** | [one-surface] (agreement) — no independent completeness check | <!-- CH-U4-03 upheld -->
| **H-U4-04** | **none** — `max-number-of-parents` (255) is a *refuted* candidate: it bounds no set a bonded network can produce (the parent set is one message per sender; the shipped active set is 100), so it is not a preventive barrier for the merge scope's width | `spec/findings.tsv` C191; `a_block_justifying_more_parents_than_the_network_allows_is_refused` (per C191's row) | [unhoused] — bounds no set a bonded network can produce | <!-- CH-U4-06 upheld -->
| ⚠ **H-U4-05** | the directed quotient bounds work by the **output** rather than the width | `spec/audit/evidence/n127-loaded-results.md` (161–174 chains → 153–290 states against the control's 650,159 at 29) and `sdk/tests/merging_scaling.rs:rejection_options_are_bounded_on_a_directed_shape` | — | <!-- contested -->
| **H-U4-06** | **none** — `max-number-of-parents` (255) is a *refuted* candidate (one message per sender; shipped active set 100), and `sdk/src/dag/merging.rs:with_dependencies`' finite ancestry bounds only the preprocessing's *termination*, not the *order* the LATE deviation is about; neither is preventive | `spec/findings.tsv` C191; `sdk/src/dag/merging.rs:with_dependencies` (read; frontier difference-bounded, so it terminates) | [unhoused] — the deviation's input has no barrier | <!-- CH-U4-06 upheld -->

### Provenance

read: `git rev-parse HEAD` = `1e5a64ed4149d5bd7fa15affb1022e3c89f6c492`; `git status --porcelain` = `?? .claude/` only (tracked tree clean) · `sdk/src/dag/merging.rs`: `with_dependencies`, `incompatible_with_final`, `SearchBudget` (`::NODE`/`::UNBOUNDED`), `SearchBudgetExceeded`, `SearchCensus::keys_are_a_symmetric_irreflexive_relation`, `search`, `enumerate_rejection_sets`, `enumerate_states`, `maximal_independent_sets`, `bron_kerbosch`, `compute_optimal_rejection`, `calc_merged_result`, `traverse_tree`, `fold_rejection`, `add_mergeable_overflow_rejections`, `resolve_conflict_set`, `resolve_conflict_set_with_census` · `casper/src/merging.rs`: `search_census` module, `MergeScope::merge`, `MergeScope::fold`, `NativeRelations::conflicting`/`depends`/`reject_whole_blocks`, `DeployChainIndex` · `sdk/tests/merging_scaling.rs` (whole) · `sdk/src/property_tests.rs`: `law17_the_survivors_of_a_rejection_option_are_conflict_free`, `rejection_options_match_a_literal_enumeration`, `law17_deploys_without_conflicts_need_no_rejection` · `spec/findings.tsv` rows C178, C180, C182, C191, C198 · `spec/AUDIT.md` check-off, C171/C178/C180/C182/C191/C198 rows · `spec/audit/evidence/n127-loaded-results.md`, `spec/audit/evidence/n127-directed-results.md` · `spec/Rchain/Bounded.lean`: `the_work_never_exceeds_the_budget`, `the_late_guard_overspends`, `a_larger_budget_does_not_change_an_answer` · `docs/src/node/running-a-public-testnet.md` · `the study charter (outside the repo)` (#214 criteria) · grep for `resolve_conflict_set`/`with_dependencies`/`max-number-of-parents`.
ran: `git rev-parse HEAD`, `git status --porcelain`, `ls`/`wc -l`/`test -f`, `grep -rn` over `--include=*.rs`/`*.tsv`/`*.md`/`*.lean`, `Read` on the files above. All read-only; no repository writes.
not read: the whole of `casper/src/merging.rs` (3,411 lines — read: search_census, merge, fold, NativeRelations, DeployChainIndex context only) · `spec/audit/passes.md` §28 (the C178 account behind the row) · the per-node artifacts under `spec/audit/evidence/n127-directed/` and `n127-loaded/` · `docs/src/node/consensus.md`, `scaling.md` · `spec/review-ledger.tsv` · the `rspace/src/merger/*` files beyond `mod.rs` (the merge here is the chain-index merge in `casper`/`sdk`, not the event-log merger).

**Anchor correction.** The task named `casper/src/dag.rs (resolve_conflict_set)`. No such path or symbol exists in this tree: `resolve_conflict_set`/`resolve_conflict_set_with_census` live in `sdk/src/dag/merging.rs`, the census and the node's call site in `casper/src/merging.rs`. `rspace/src/merger/` is the *event-log* merger, a different subsystem; `casper/src/dag.rs` would be a sibling path that is absent.



> **§1.5 — corrected 2026-10-04.** 8 adversarial challenges against this node's worksheet; **8 upheld, 0 refuted**, all applied below. A row whose ID still carries ⚠ has an element the adjudication left unestablished (unmeasured, or owed a re-run); every touched row cites its challenge id in an HTML comment.

### U5 — LFS sync & join / bootstrap

**Design intent.** A node with no state restores a consistent prefix (the finalized fringe plus
tuple-space) from a peer, then replays its own blocks to the same state hashes the rest of the network
holds.

**Guide words.** row: NO/NOT, PART OF, LATE, EARLY, REVERSE, MORE, LESS · folded: AS WELL AS → MORE, OTHER THAN → REVERSE, BEFORE → EARLY, AFTER → LATE · vacuous: (none — all eleven words have a
referent in this node's mechanisms; the empty vacuous set is why this sheet is flagged for a second
reader per §1 of the plan).

### Table A — deviations

| ID | Guide word | Deviation | Cause (anchor) | Consequence → TE | A |
|---|---|---|---|---|---|
| ⚠ **H-U5-01** | NO/NOT | the pre-attempt wait — never bounded: no `FinalizedFringe` ever reaches the handler, so no sync attempt is ever started and nothing errors | the sync starts *only* from an inbound fringe (`casper/src/engine/node_syncing.rs:on_finalized_fringe_message`); the request retries the **send** only, and the bound (`MAX_BOOTSTRAP_RETRIES = 10`) does **not** bound the wait: on give-up `keep_on_requesting_till_running` breaks with no `Err`, so `request_finalized_fringe` returns `Ok(())` and the node latches on **any** bootstrap that never answers — reachable-but-silent or unreachable alike (`casper/src/protocol/comm_util.rs:send_with_retry`/`keep_on_requesting_till_running`); the responder can answer nothing when its DAG has neither a latest fringe nor a height-0 block (`casper/src/engine/node_running.rs` `FinalizedFringeRequest` arm — `fringe_response = None`); `casper/src/engine/node_launch.rs:apply`'s `select!` over `handle_loop`/`finished`/`terminal` has no completion without a fringe | node latched in `NodeSyncing` for good **with a serving API and no error line** (the C181/**#102** latch class, one stage earlier) → TE-3 | A2.c | <!-- CH-U5-06 upheld, CH-U5-07 upheld -->
| **H-U5-02** | PART OF | the per-block fringe metadata — only part of it arrives, so a restored block is inserted with an empty fringe | requester sets `include_fringe_metadata=true` (`casper/src/protocol/comm_util.rs:request_finalized_fringe`), but the responder attaches ancestry only when asked and **skips any block whose metadata it lacks** (`casper/src/engine/node_running.rs:collect_fringe_ancestry` logs "no metadata … the joiner will derive its own"); the joiner's fallback is `BlockMetadata::from_block` (`casper/src/engine/node_syncing.rs:populate_dag`, the `fringe_of.get(&hash)` `None` arm, per-block WARN) | a boundary block whose `close_block` anchors the next epoch's seed to the fringe state replays to a **different post-state**, so the joiner cannot index its own restored chain — the measured #139 shape → **TE-3** (an outsider validator cannot join; TE-2 only if it then produces a block that finalises — unmeasured, and the #139 run shows it produces none) | A2.b | <!-- CH-U5-04 upheld -->
| H-U5-03 | LATE | the tuple-space leg — never gives up: the attempt's `join!` waits for **both** legs, and only the block leg has a pace bound | `casper/src/engine/lfs_tuple_space_requester.rs:request_tuple_space_roots` — the request loop resends on the idle timeout with **no `MAX_IDLE_ROUNDS`** and returns only on `is_finished()` or an error; contrast the block leg's `casper/src/engine/lfs_block_requester.rs:MAX_IDLE_ROUNDS`; the join is `casper/src/engine/node_syncing.rs:run_approved_state_sync`'s `tokio::join!` | a peer that serves blocks but never completes the trie (a page refused by `MAX_STORE_ITEMS_BYTES`, or an unreadable trie dropped — `casper/src/engine/node_running.rs:handle_store_items_request`) makes the attempt **pending forever**, so `MAX_SYNC_ATTEMPTS`/`terminal` are never reached → TE-3 | A2.b, A2.c |
| H-U5-04 | EARLY | the startup mode — chosen from the DAG's emptiness, not the approved fringe, so Running is entered before the restore completed | `casper/src/engine/node_launch.rs:apply` dispatches on `repr.dag_set.is_empty()`; `populate_dag` inserts blocks incrementally (`casper/src/engine/node_syncing.rs`), so a kill mid-walk leaves a **non-empty** DAG; the approved fringe is written only on success and has **no production reader** (`block-storage/src/syntax.rs:get_approved_block` is called only from its own test) | a node killed mid-`populate_dag` restarts into "Reconnecting to existing network" over a **strict prefix** of the chain, serving a latest-fringe it never derived → TE-3 (and TE-1 if it produces on it) | A2.b |
| ⚠ **H-U5-05** | REVERSE | the fringe sender — any peer's `FinalizedFringe` is processed as if it came from the bootstrap; the check logs "ignored" and then does the opposite | `casper/src/engine/node_syncing.rs:on_finalized_fringe_message` — `if !sender_is_bootstrap { log }` with **no `return`**; the slot is then written and (`start_requester`) the one-shot trigger is consumed for that same sender | a connected non-bootstrap peer can seize the sync target before the bootstrap answers (a fringe the bootstrap never approved, whose blocks it then serves) — **unmeasured** — or (the ordering where a non-bootstrap non-empty fringe arrives first) set the **first** attempt's target — at most one wasted attempt of `MAX_SYNC_ATTEMPTS` (3), after which the bootstrap's newer answer replaces the slot and is the one used → TE-2 / TE-3 | A3.x | <!-- CH-U5-02 upheld -->
| ⚠ **H-U5-06** | MORE | **contested** (re-homed as an efficiency observation, not a HAZOP deviation) — the trailing state pull: the pre/post trie root of **every** downloaded block is hydrated on top of the fringe root | `casper/src/engine/node_syncing.rs:run_approved_state_sync` builds `collect_block_state_roots` over the whole `height_map` and calls `request_tuple_space_roots` for each distinct root; the reason (read/validation APIs open the pre/post RSpace root of downloaded blocks directly) is stated only in the two source doc-comments (`casper/src/engine/node_syncing.rs:482-484`, `casper/src/engine/lfs_tuple_space_requester.rs:315-316`) — `spec/audit/passes.md` (C64) is the block-walk pace row and carries no independent cost record for the whole-root pull | the approved-state join hydrates the pre/post trie root of every downloaded block — deliberate (`f783c1b5a`, for read/validation APIs), O(chain length) by design, cost `unmeasured` → (no top-event edge: not a deviation) | A2.c | <!-- CH-U5-03 upheld, CH-U5-08 upheld -->
| ⚠ **H-U5-07** | LESS | the retry budget — only `MAX_SYNC_ATTEMPTS` (3) attempts before the node stops, so a modest bootstrap outage evicts the joiner | `casper/src/engine/node_syncing.rs:MAX_SYNC_ATTEMPTS`/`SYNC_RETRY_DELAY` and the `terminal` signal; `casper/src/engine/node_launch.rs:apply` turns `terminal` into a returned `Err`, which its spawn only logs — there is no process exit | the retry budget still evicts the joiner, and the node then keeps serving over a DAG it never synced while its peer pipeline is dead — the state the barrier claims to prevent → TE-3 | A2.b | <!-- CH-U5-01 upheld -->
| **H-U5-08** | NO/NOT | the block walk consumed a key before checking the body, and no request selected `Received` again | `validate_received_block` called `received` before `validate::block_hash` (`casper/src/engine/lfs_block_requester.rs:73-108`); `received` moved `Requested -> Received`, `get_next` re-asked only `Init`/`Requested`, and `is_finished` requires the request map empty (`casper/src/engine/mod.rs:102-114`, `:146-147`, `:178-180`) | one refused answer made the walk unable ever to finish — the live 3-finished-of-28 shape (#457) → TE-3 | A2.b | closed: **Law 81**, #457, register **C337**; `casper/src/engine/lfs_block_requester.rs::a_mature_chain_join_completes` |
| **H-U5-09** | REVERSE | the join path wrote the bootstrap's per-block fringe metadata unchecked | `populate_dag` copied `BlockFringe` into `BlockMetadata` (`casper/src/engine/node_syncing.rs`); `dag.insert` uses the `fringe` and `close_block` folds `fringe_state_hash` into the epoch seed | a fringe the block's justifications do not derive is written as the block's history; a multi-member state hash remains an assumed input → TE-3 | A2.b | **C338 closed**, **C339 open**: **Law 83**; `casper/src/engine/node_syncing.rs::an_ancestry_fringe_that_is_not_the_derived_fringe_is_refused` |

### Table B — barriers

| Deviation | Preventive barrier (anchor) | Evidence | Independence |
|---|---|---|---|
| ⚠ **H-U5-01** | **none effective for the pre-fringe wait**: the C181 `terminal` bound is only reachable *after* a fringe has arrived and three attempts have failed; no timeout wraps `apply`'s wait for the fringe itself (`casper/src/engine/node_launch.rs:apply`) | `unmeasured` — no committed run drives a bootstrap that receives the request and answers nothing | [shared-cause] with H-U5-03 (both are "no bound on waiting for the peer"; one event disables the guard and is the hazard) | <!-- contested -->
| ⚠ **H-U5-02** | **contested / `unmeasured` for the deviation's own (partial-arrival) arm**: the request asks for the metadata and the responder attaches it (`casper/src/protocol/comm_util.rs:request_finalized_fringe`, `casper/src/engine/node_running.rs:collect_fringe_ancestry`); `casper/src/engine/node_syncing.rs:populate_dag` applies it and **names** the fallback per block rather than silently — the only measured protection is on the **complete-metadata** arm, which the deviation is not | `unmeasured` — the n139 run (`spec/audit/evidence/n139-mature-join-results.md`, post-fix 4/5 green) is homogeneous and same-version, so it exercises the **complete**-metadata path only, not the partial-arrival path the deviation's Cause names; the committed raw run is the pre-fix reproduction (`spec/audit/evidence/n139-mature-join/048c2532f-20261001T101407Z/`) | downgraded (was `—`, independent) — the only measured protection is on the arm that is not under threat | <!-- CH-U5-05 upheld -->
| H-U5-03 | the block leg's pace bound `casper/src/engine/lfs_block_requester.rs:MAX_IDLE_ROUNDS` (3 idle rounds) and the `terminal` bound — neither covers the tuple-space leg, which is what the `join!` also waits on | symbol read (the asymmetry is in `request_tuple_space_roots`); the tuple-leg hang itself is `unmeasured` | [shared-cause] with H-U5-01 |
| H-U5-04 | C68's sequencing keeps a **failed** attempt out of Running and the approved fringe is not recorded on failure (`casper/src/engine/node_syncing.rs:notify_when_restored`) — but a *crash* mid-`populate_dag` is not covered, because the mode decision never reads the approved store | symbol read (`casper/src/engine/node_launch.rs:apply`; `block-storage/src/syntax.rs:get_approved_block` has no production caller); the crash path is `unmeasured` | — |
| ⚠ **H-U5-05** | **none**: the sender check is a log line, not a gate (`casper/src/engine/node_syncing.rs:on_finalized_fringe_message`) | symbol read | — | <!-- contested -->
| ⚠ **H-U5-06** | ~~the responder's page caps `casper/src/engine/node_running.rs:MAX_STORE_ITEMS_TAKE`/`MAX_STORE_ITEMS_BYTES` bound **one page**, not the number of roots the requester asks for~~ — **struck**: never a barrier against this surface (the deviation is by design, not a hazard) | `unmeasured` — the joiner's per-join cost on a mature chain is not a committed run | — | <!-- CH-U5-08 upheld -->
| ⚠ **H-U5-07** | **contested** — the `terminal` bound (C181) is the intent, but it **does not** stop a node serving a chain it never synced: `apply` returns `Err` to a spawn that only logs it (`casper/src/engine/node_launch.rs:apply`), leaving the listeners up; the newer-fringe trigger shortens a retry (`casper/src/engine/node_syncing.rs:MAX_SYNC_ATTEMPTS`/`fringe_arrived`) | symbol read; no run measures joiner survival across a transient bootstrap outage → `unmeasured` | [self-referential] — the message asserts the stop it does not perform | <!-- CH-U5-01 upheld -->
| **H-U5-08** | the walk's `known` gate and content check **before** `received`, the resend re-asking `Received`, and the give-up breakdown in the error | `casper/src/engine/lfs_block_requester.rs::a_mature_chain_join_completes`, `::a_forged_body_does_not_consume_the_key_and_is_re_requested`, `casper/src/engine/mod.rs::resend_re_requests_a_received_key` | independent of H-U5-03 — this is the block leg's own progress; the tuple leg's bound is untouched |
| **H-U5-09** | the derived-fringe check in `populate_dag`; the fringe's state hashes are logged and remain **C339** | `casper/src/engine/node_syncing.rs::an_ancestry_fringe_that_is_not_the_derived_fringe_is_refused`, `::an_ancestry_entry_for_an_unknown_block_is_refused`, `::two_ancestry_entries_for_one_block_are_refused` | independent — no other U5 barrier reads the ancestry metadata |

### Provenance

read: `spec/audit/evidence/n139-mature-join-results.md` · `casper/src/engine/node_syncing.rs` · `casper/src/engine/lfs_block_requester.rs` · `casper/src/engine/lfs_tuple_space_requester.rs` · `casper/src/engine/node_launch.rs` · `casper/src/engine/node_running.rs` · `casper/src/protocol/comm_util.rs` · `block-storage/src/syntax.rs` (approved-store read) · `node/src/runtime/node_runtime.rs` (shard/empty-DAG guard) · `spec/audit/passes.md` (C64/C65/C68/C102/C181) · `tools/devnet.sh` (lines 1-~200; `reset`/`up` semantics) · `.claude/plans/linear-spinning-eich.md` (study charter, node manifest, TE/A vocabulary) · ran: `git rev-parse HEAD`, `git status --porcelain`, `git ls-files --error-unmatch` (all anchors tracked), `grep`/`Read` over the above · not read: the Scala oracle (`legacy/` is empty in this tree, so `NodeSyncing.scala`/`LfsBlockRequester.scala` could not be read — the port comments' `:line` citations were taken as claims, not verified), `spec/AUDIT.md` §6 in full, `casper/src/validated/` (not present), the n139 per-attempt log files under `spec/audit/evidence/n139-mature-join/` (the results sheet was read, the raw logs were not).

**tree:** `1e5a64ed4149d5bd7fa15affb1022e3c89f6c492` · **dirty:** `?? .claude/` only (no tracked modification)



> **§1.6 — corrected 2026-10-04.** 9 adversarial challenges against this node's worksheet; **9 upheld, 0 refuted**, all applied below. A row whose ID still carries ⚠ has an element the adjudication left unestablished (unmeasured, or owed a re-run); every touched row cites its challenge id in an HTML comment.

### U6 — Bonding, epoch boundary, PoS

**Design intent.** A bond or withdrawal takes effect exactly once, at the next epoch boundary, on every node identically.

**Guide words.** row: NO/NOT, MORE, AS WELL AS, EARLY, LATE, OTHER THAN · folded: LESS → NO, PART OF → NO, BEFORE → EARLY, AFTER → LATE · vacuous: REVERSE (no inverse transition exists — a bond cannot debit the pool, a withdrawal cannot credit it, and the epoch boundary has no un-apply step: finality/state undo is a stated deferral, so an inverted bond/withdrawal is not a value the state machine can reach)

### Table A — deviations

| ID | Guide word | Deviation | Cause (anchor) | Consequence → TE | A |
|---|---|---|---|---|---|
| ⚠ **H-U6-01** | NO/NOT | A `delegate`/`trust`/`withdraw` deploy writes a **set-shaped** native leaf (`pos:delegations` / `pos:trusted` / `pos:pending_withdrawers`); when a concurrent sibling block writes the same leaf and neither has seen the other, the merge rejects one **whole** block and the deploy's write takes effect on **no** node. | The native channel merges whole-slot **snapshots**, and a set has no arithmetic to compose, so the merge's only recourse is `reject_whole_blocks` (`spec/audit/evidence/c207-merge-loses-native-writes.md` §5 residual; `spec/audit/passes.md` §61; `rholang/src/native_state.rs::delegate`, `::trust`, `::withdraw`) | The delegator/key cannot join or leave → TE-3 | A2.c | <!-- contested -->
| ⚠ **H-U6-02** | MORE | The boundary re-draws the active set from the epoch seed **in addition to** the epoch's membership changes, so the consensus set (`pos:active`, the leaf the finality gates read) moves at a boundary **only when `number_of_active_validators < pool size`** — reachable only in that regime. The original "moves at **every** boundary even when no stake moves" is struck: on the acceptance net the pool is one validator, so no cap bites and the draw is the identity. | `close_block` step 4 reselects from the pool behind the seed (`rholang/src/native_state.rs::close_block`, `select_active`, `rng_input`); the seeded draw is a deviation from the Scala's activate-all-bonds (`spec/RUST-VS-SCALA.md:310-315`, "the drawn set changes at every boundary even when no stake moves") | **Contested.** The justifications' carried bonds maps disagree, so the finaliser's pruned-history fallback refuses rather than guessing (`spec/Rchain/Casper/Bonds.lean::a_disagreeing_set_is_refused`) → **TE-1** (a refusal is a stall, not a divergence — reclassed from the original `→ TE-2`, kept below — CH-U6-04); and the refusal is **unreachable on the acceptance net** (no cap bites, so no boundary moves the set — CH-U6-03). The measured observable of the deviation is the drawn-out proposer declining (`NotBonded`) → chain halt, whose root cause is a pre-existing participation/state-staleness problem the draw makes reachable (`spec/RUST-VS-SCALA.md:288-300` — CH-U6-05). Original cell: 'The justifications' carried bonds maps disagree at every boundary, so the finaliser's pruned-history fallback refuses rather than guessing (`spec/Rchain/Casper/Bonds.lean::a_disagreeing_set_is_refused`) → TE-2'. | A2.a / A3 | <!-- CH-U6-03 upheld; CH-U6-04 upheld; CH-U6-05 upheld -->
| ⚠ **H-U6-03** | AS WELL AS | At a boundary height **every** proposer's block carries its own `close_block`, so the epoch transition runs on every sibling **as well as** the one the merge keeps; the sibling boundary blocks hold different absolute snapshots of `pos:committed`/`pos:vault` (their pots differ by what each block charged) and `close_block` is not idempotent, so the merge rejects all but one whole block and a deploy riding the loser is discarded. | `close_block`'s own non-idempotence note and its absolute snapshot leaves (`rholang/src/native_state.rs::close_block`); resolution pinned by `casper/src/merging.rs::boundary_merge_tests::sibling_boundaries_with_different_pots_merge`, measured in `spec/audit/passes.md` §61 (a `delegate` in block 206 discarded) | A bond/withdraw coalesced into the rejected boundary block cannot take effect → TE-3 | A2.c | <!-- contested -->
| ⚠ **H-U6-04** | EARLY | A bond-set change (any `bond`/`withdraw`/`slash` that moves the pool) that lands **before the chain's first finalisation** wedges the chain only as a compound: the fringe is empty **and** the newest justification's state is unreadable (pruned) **and** the justifications' carried bonds maps disagree. **Contested / pre-fix:** the original unqualified form ("while the finalized fringe is still empty — permanently wedges the chain: the change is carried by blocks whose bonds maps cannot be reconciled") is **not a present-tree behaviour** — the state read is the fix (`fd6665f3f`, ancestor of HEAD); the compound is itself **unmeasured** on this tree. | The finalizer's bonds fallback requires the justifications' carried maps to agree when the newest justification's state is unreadable, and there is no finalised fringe to read the map from yet (`spec/Rchain/Casper/Bonds.lean::a_disagreeing_set_is_refused`, whose comment records #73's shape; `docs/src/node/testnet.md:624`) | The chain halts and cannot recover → TE-1 | A1.1 / A2.c | <!-- CH-U6-06 upheld -->
| ⚠ **H-U6-05** | LATE | A withdrawal takes effect only when some **later** block crosses an epoch boundary, so on an idle chain the request looks ignored for as long as the net stays quiet; the refund is paid at the first epoch boundary `≥` the recorded deadline `quarantine_length + epoch_length·(1 + blockNumber/epoch_length)` — the quarantine is **inside** the deadline, and the only extra wait past it is to the next boundary, **bounded by `epoch_length`**. **Settled** by the 2026-10-04 read (`spec/audit/evidence/n220-leave-results.md`: `deadline=30 blocks_remaining=20` at a staging height of ~10 with `quarantine_length = 20` and `epoch_length = 10`, i.e. the recorded `quarantine_length + divisor·(1 + block_number/divisor)`, not the struck inversion). | `close_block` runs its sequence only inside a produced block and only at `block_number % max(epoch_length,1) == 0` (`rholang/src/native_state.rs::close_block`, `is_epoch_boundary`); the withdrawal deadline adds `quarantine_length` (`::withdraw`; Law 47, `spec/LAWS.md`) | A validator cannot promptly leave; its stake stays locked → TE-3 | A2.a | <!-- CH-U6-09 upheld -->
| ⚠ **H-U6-06** | OTHER THAN | `untrust` severs a validator's admission but does **other than** remove it: the revoked validator keeps its bond and its place in the pool and active set, so it keeps proposing and attesting until it withdraws or is slashed for an attributable fault. | `untrust` edits only `pos:trusted`; the former `slash`-on-untrust was removed (`rholang/src/native_state.rs::untrust`; AUDIT C111, `spec/audit/passes.md:4064`) | An operator cannot remove a peer it has decided is unsafe — the peer keeps voting until it withdraws or **equivocates** (a slashable, attributability-objective fault: a bonded equivocator is slashed at the `Malicious` tier and the evidence travels — `spec/AUDIT.md` C200; the C111 removal-of-slash-on-untrust stands beside it); the core hazard survives, narrowed → TE-2 (**contested** — the removal gap is a liveness/authority gap, and the class is re-checked under CH-U6-04's exercise) | A3 | <!-- CH-U6-01 upheld; CH-U6-02 upheld -->

### Table B — barriers

| Deviation | Preventive barrier (anchor) | Evidence | Independence |
|---|---|---|---|
| ⚠ **H-U6-01** | Cost accounting left the block's native sidecar and merges per accepted deploy, so the **universal** conflict on `pos:vault` is gone (`casper/src/merging.rs::a_cost_accounted_block_keeps_its_own_native_writes_through_the_merge`; `spec/audit/passes.md` §61) | `spec/audit/evidence/c207-merge-loses-native-writes.md` §5–§6 (committed measurement + the mutation each falsifier was run against) — evidences the **barrier** (the universal `pos:vault` conflict is gone) and the boundary pair, **not** this row's set-leaf deviation, which is **unmeasured**; TE-3 is contested until a run measures it | `[one-surface]` (contested; was `Partial`) — the barrier composes **sums** only; it does **not** cover the set-valued leaves, which is exactly the residual. It also does not cover H-U6-03 | <!-- CH-U6-07 upheld; CH-U6-08 upheld -->
| ⚠ **H-U6-02** | The reason the draw is safe is the finaliser's fallback **refusing** two disagreeing bonds maps rather than picking one (`spec/Rchain/Casper/Bonds.lean::a_disagreeing_set_is_refused`) | `spec/Rchain/Casper/Bonds.lean` (model witness, read); the live residual is `spec/RUST-VS-SCALA.md:310-315` — **unmeasured-live** beyond the registered note, and **unmeasured** for the acceptance net: the draw is cap-scoped, so this guard is only reachable in the `cap < pool` regime | `[shared-cause]` with H-U6-04 — both are the same agreement requirement in `compute_bonds`' fallback; one defect disables both (the acceptance net cannot reach it; the pairing holds only for the `cap < pool` regime) | <!-- CH-U6-03 upheld -->
| ⚠ **H-U6-03** | The merge rejects the losing sibling boundary block whole (`casper/src/merging.rs::sibling_boundaries_with_different_pots_merge`, `:3073`) | `spec/audit/passes.md` §61 (measured: `delegate` in block 206 discarded); `spec/audit/evidence/c207-merge-loses-native-writes.md` §5 second instance | `[barrier-is-the-threat]` (contested; was `[shared-cause]` with H-U6-01) — the merge's whole-block rejection of the loser is both the barrier and the deviation's harm; the `[shared-cause] with H-U6-01` pairing is a cross-row note, not this row's label | <!-- CH-U6-08 upheld -->
| ⚠ **H-U6-04** | The empty-fringe case is the #73 shape; the present rule refuses a disagreeing carried set and the **documented constraint** is to keep the fringe non-empty (`spec/Rchain/Casper/Bonds.lean:20-22`; `docs/src/node/testnet.md:624`) | `docs/src/node/testnet.md:619-628` (re-verified on a chain 2026-09-26: the withdraw at block 20 was absorbed **with a non-empty fringe**) — measures the **non-empty**-fringe withdraw and cannot witness the stated (empty-fringe) deviation, which remains **unmeasured** on this tree | `[self-referential]` (**contested**) — its referent is gone (an empty fringe does not freeze on this tree), so the label must be **re-chosen** against the narrowed deviation; the barrier (keep the fringe non-empty) is real but no longer self-referential to a wedge the code prevents | <!-- CH-U6-06 upheld -->
| ⚠ **H-U6-05** | `close_block` is invoked once per produced block and returns before any write off a boundary (`rholang/src/native_state.rs::close_block`, `is_epoch_boundary`) | `docs/src/node/testnet.md:619-623` — "a withdraw on a quiet net looks ignored for as long as the net stays quiet" (a reading on a live chain, not a committed run); the boundary-fires-only-in-a-block half is the symbol read | `[self-referential]` (contested; was `blank`) — no barrier independent of "someone produces a block"; the hazard is the liveness of block production itself (the barrier reads against the very liveness the hazard freezes; `[unhoused]` is not an independence class — this cell carries an anchor) | <!-- CH-U6-08 upheld -->
| ⚠ **H-U6-06** | The design decision that revocation is a governance act, not confiscation — an untrusted-but-bonded validator keeps its stake (AUDIT C111, `spec/audit/passes.md:4064`; `rholang/src/native_state.rs::untrust`) | `spec/audit/passes.md:4064` C111 (the removal of the `slash`-on-untrust, with its stated reason) | `[barrier-is-the-threat]` — the property that makes revocation safe (the bond is not the operator's to take) is the same property that makes it ineffective as a removal | <!-- CH-U6-01 upheld -->

### Provenance

read: `rholang/src/native_state.rs` (bond, withdraw, close_block, slash, trust, untrust, delegate, undelegate, select_active, epoch_divisor, is_epoch_boundary, epoch_reward, epoch_pot, apply_weight, PosParams) · `spec/LAWS.md` (laws 44, 47, 16d) · `spec/audit/evidence/c207-merge-loses-native-writes.md` · `spec/audit/passes.md` §61 · `docs/src/node/testnet.md` (600-669) · `spec/RUST-VS-SCALA.md` (300-329) · `spec/Rchain/Casper/Bonds.lean` (1-60, 106-199) · `spec/AUDIT.md` (C111, C207) · `spec/findings.tsv` (C204/C207 rows) · `casper/src/blocks/proposer/block_creator.rs` (205-232) · `casper/src/runtime_manager.rs` (880-940) · `the study charter (outside the repo)` · ran: `git rev-parse HEAD`, `git status --porcelain`, `git branch --show-current`, grep/wc/ls over the above · not read: `casper/src/merging.rs` (only grep hits), `casper/src/multi_parent_casper.rs`, `rholang/src/system_processes.rs`, `spec/audit/evidence/n193-delegation-live-results.md`, `docs/src/node/validator-economics.md`



> **§1.7 — corrected 2026-10-04.** 6 adversarial challenges against this node's worksheet; **5 upheld, 1 refuted**, all applied below. A row whose ID still carries ⚠ has an element the adjudication left unestablished (unmeasured, or owed a re-run); every touched row cites its challenge id in an HTML comment.

### U7 — Deploy pool, gas & admission

**Design intent.** A deploy is admitted once, held while valid, included exactly once or expired; the cost of admitting is bounded per unit time.

**Guide words.** row: MORE, LESS, PART OF, REVERSE, EARLY, LATE, BEFORE · folded: NO/NOT → MORE, AS WELL AS → LESS, AFTER → LATE · vacuous: OTHER THAN (the signature verifier refuses any non-canonical spelling — `crypto/src/signatures/signatures_alg.rs:normalize_signature_low_s` maps the high-S twin to the accepted one and `secp256k1::verify_bytes` refuses the twin — so a deploy cannot be admitted under a sig the pool keys on *other than* the one `repeat_deploy` dedups on; the guide word has no referent)

### Table A — deviations

| ID | Guide word | Deviation | Cause (anchor) | Consequence → TE | A |
|---|---|---|---|---|---|
| ⚠ **H-U7-01** | MORE | The admission bound is global and shared, not per-sender: one sender at the shared 100/s limiter can occupy the 10,000-slot pool and consume the whole rate budget, so every other sender is refused (`deploy pool is full` / rate limited) — cost is bounded per unit time but not fairly, so "anyone can propose" fails under contention | `node/src/web/http.rs:deploy_rate_limiter` and `node/src/api/grpc/mod.rs:serve_deploy` (one shared `RateLimiter::new(DEFAULT_API_RATE_LIMIT_PER_SEC)`, 100/s, per surface) + `casper/src/dag.rs:BlockDagKeyValueStorage::add_deploy` / `MAX_POOLED_DEPLOYS` (count cap) **+ `MAX_POOLED_DEPLOYS_PER_DEPLOYER` (the submitter's share — added 2026-10-13, W-N2-04)** | denial of admission for all other senders → TE-1 (user-transaction production stalls; the chain still proposes empty blocks) | A1.2, A3 | <!-- contested -->
| H-U7-02 | LESS | The admission bound is weaker than the inclusion bound: `phlo_limit` is checked only `>= 0` at ingress, but a deploy with `phlo_limit > MAX_BLOCK_PHLO` is admitted and can never be validly included; `select_deploys` filters only future/expired/replay (no phlo), so a proposer holding one builds a block its own `validate_block`→`block_summary` refuses, incrementing `consecutive_failures` until autopropose halts — the pool's usable capacity is less than the count bound implies | `casper/src/validate.rs:block_phlo` / `MAX_BLOCK_PHLO` + `casper/src/api/block_api_impl.rs:deploy` (`phlo_limit < 0` only) + `casper/src/blocks/proposer/proposer.rs:select_deploys` (count-only budget) | the proposer rejects its own block → TE-1 (the admitting node is wedged for the deploy's lifespan) | A1.1, A3 |
| ⚠ **H-U7-03** | PART OF | The per-block bounds cover only part of the block's work: `deploy_count` and `block_phlo` both iterate `state.deploys` and exclude `state.system_deploys`, so a block's system deploys are outside both the width bound and the phlo budget; the doc comment asserts they are "already bounded by max-number-of-parents" | `casper/src/validate.rs:deploy_count` and `casper/src/validate.rs:block_phlo` (iterate `state.deploys` only) — the doc comment's `max-number-of-parents` justification is not the code's bound; the system-deploy set is bounded by the shared 255 seed-index budget: `casper/src/blocks/proposer/proposer.rs:per_block_deploy_budget` (proposer side) + `casper/src/runtime_replay.rs:253` (`u8::try_from(terms.len() + i)`, validator side) | per-block replay work outside the gas budget is a bounded-accounting gap (no check counts system deploys against phlo or width), not an exhaustion halo — contested, no TE-4 | A3 | <!-- CH-U7-02 upheld -->
| ⚠ **H-U7-04** | REVERSE | A deploy leaves the pool at *insert*, not at *finalization*: `insert` deletes the block's deploy ids from `deploy_store` and indexes them in `deploy_index` for any block that passes validation; if that block is later orphaned the deploy is neither held nor finally included and `select_deploys` excludes it as `replay_attack`, so it is silently lost although never finalised | `casper/src/dag.rs:BlockDagKeyValueStorage::insert` (`deploy_store.delete(&deploy_hashes)`) + `casper/src/blocks/proposer/proposer.rs:select_deploys` (`replay_attack` filter) | an accepted deploy never finalises; an orphan branch the rest rejects consumes it → TE-2 | A1.1 | <!-- contested -->
| ⚠ **H-U7-05** | EARLY | The expiry clock is the maximum DAG height (`height_map` max, including unfinalized and orphaned branches), not the finalized height: `expire_deploys` drops a deploy once `latest_block_number - valid_after > DEPLOY_LIFESPAN`, so when finality lags or freezes the DAG height runs ahead and a deploy silently expires before the finalized chain has given it its 50-block window | `block-storage/src/dag/representation.rs:latest_block_number` (max `height_map` key) + `casper/src/dag.rs:BlockDagKeyValueStorage::expire_deploys`; the freeze is witnessed by `spec/audit/evidence/n148-results.md` (arm A — kill at T+120, finality frozen, height run-away, 3/3); the end-to-end early-expiry drop is still unmeasured | an accepted deploy is expired early → TE-4 (pool churn) / A1.1 | A1.1 | <!-- CH-U7-06 upheld -->
| H-U7-06 | LATE | Expiry runs only from `insert`, driven by an incoming accepted block: if block insertion stops — the node cannot propose and no peer sends blocks — `expire_deploys` never runs, so the pool never drains and a transient saturation becomes permanent even past every deploy's lifespan | `casper/src/dag.rs:BlockDagKeyValueStorage::insert` (the only caller of `expire_deploys`) + `#213` | a full pool stays full with stale deploys → TE-1 (admission denied permanently) | A1.2 |
| H-U7-07 | BEFORE | The faucet spends the per-address drip *before* the deploy is submitted: the drip counter is incremented, then `block_api.deploy` runs, and a failed submit does not roll it back, so a drip is consumed by an operation that produced nothing | `node/src/api/web_api_impl.rs:faucet` (drip increment precedes `block_api.deploy`) + `node/src/api/faucet.rs:sign_faucet_deploy` | a joining validator's funding drip is burned on a failed submit → TE-3 | A2.c |

### Table B — barriers

| Deviation | Preventive barrier (anchor) | Evidence | Independence |
|---|---|---|---|
| ⚠ **H-U7-01** | `KeyedRateLimiter` (100/s **per peer**) on both ingestion surfaces, plus the pool cap *and* a per-deployer share as backstops (all 2026-10-13) | `casper/src/dag.rs:BlockDagKeyValueStorage::add_deploy` (symbol read); `node/src/api/grpc/mod.rs:serve_deploy` (symbol read) | `[barrier-is-the-threat]` — the global bound both bounds the flood and is what denies honest senders | <!-- contested -->
| H-U7-02 | `block_summary`'s `block_phlo` refuses an over-budget block (consensus bound) — but at the block, not at admission | `casper/src/validate.rs:block_phlo` (symbol read) | `[one-surface]` — the bound sits on the block path only; no admission-side phlo filter exists, so it converts a bad deploy into a proposer halt |
| ⚠ **H-U7-03** | `casper/src/runtime_replay.rs:253`'s `u8` seed index (bounds the count) together with `slash_is_unjustified` (re-derives the slash set and tier from this node's own DAG, bounding the victims) | `casper/src/runtime_replay.rs:253` (symbol read); `casper/src/interpreter_util.rs:586:slash_is_unjustified` (symbol read) | `[one-surface]` | <!-- CH-U7-02 upheld; CH-U7-05 upheld -->
| H-U7-04 | `repeat_deploy` rejects a block re-carrying a deploy already in its parent chain, and `deploy_index` records the inclusion | `casper/src/validate.rs:repeat_deploy` (symbol read) | `[one-surface]` — `repeat_deploy`/`deploy_index` guards the double-inclusion surface only, not the pool-loss deviation H-U7-04 names | <!-- CH-U7-03 upheld; CH-U7-05 upheld -->
| ⚠ **H-U7-05** | none distinct — expiry and selection read different clocks, so no barrier separates them; the two surfaces diverge structurally (round-snapshot lag) and on orphans without any finality lag | `casper/src/blocks/proposer/proposer.rs:select_deploys` (symbol read); no committed run — `unmeasured` | `[self-referential]` | <!-- CH-U7-04 upheld -->
| H-U7-06 | `expire_deploys` runs on every accepted block insert | `casper/src/dag.rs:expire_deploys` (symbol read) | `[shared-cause]` — the same event (no accepted block / the #213 wedge) is what the barrier needs to run and what stops it |
| H-U7-07 | Per-address drip budget (`FAUCET_MAX_DRIPS_PER_ADDRESS`) and the deployer-own-address guard | `node/src/api/web_api_impl.rs:faucet` (symbol read) | `[barrier-is-the-threat]` — the budget both bounds abuse and is what a failed submit consumes |

### Provenance

read: casper/src/dag.rs · casper/src/validate.rs · node/src/api/faucet.rs · casper/src/api/block_api_impl.rs · casper/src/blocks/proposer/proposer.rs · casper/src/multi_parent_casper.rs · node/src/web/http.rs · node/src/api/grpc/mod.rs · node/src/api/grpc/deploy_grpc_service_v1.rs · node/src/api/web_api_impl.rs · block-storage/src/dag/representation.rs · casper/src/block_status.rs · crypto/src/signatures/signatures_alg.rs · crypto/src/signatures/secp256k1.rs · spec/AUDIT.md (grep) · spec/findings.tsv (grep) · the study charter (outside the repo)

ran: `git rev-parse HEAD` (→ 1e5a64ed4149d5bd7fa15affb1022e3c89f6c492) · `git status --porcelain` (→ `?? .claude/`) · read-only `grep`/`sed`/`wc`/`ls` over the paths above

not read: spec/audit/evidence/* (no committed U7 run exists) · docs/src/node/*.md · AGENTS.md in full · spec/laws.tsv · spec/audit/passes.md · the worktrees under .claude/worktrees/



> **§1.8 — corrected 2026-10-04.** 8 adversarial challenges against this node's worksheet; **7 upheld, 1 refuted**, all applied below. A row whose ID still carries ⚠ has an element the adjudication left unestablished (unmeasured, or owed a re-run); every touched row cites its challenge id in an HTML comment.

### U8 — Wire protocol, transport, discovery

**Design intent.** A message from a bounded-adversary peer is authenticated, bounded in size and rate, and cannot make the node do unbounded work.

**Guide words.** row: NO/NOT, MORE, AS WELL AS, PART OF, BEFORE, AFTER, EARLY, LATE · folded: LESS → MORE, OTHER THAN → PART OF · vacuous: **none — REVERSE is contested** (CH-U8-02). The note's blanket clause — "no one-way pipeline, ordering or merge whose direction could be reversed" — is **struck**: the transport is a bidirectional envelope, but "a verifier per direction" is a *certificate* direction, not a flow whose direction can be reversed, so the narrow half cannot carry the vacuity marker's load. The referent is recorded below as a **candidate** deviation; no run shows it producing harm, so REVERSE is *unproven*, not positively non-vacuous.

> **A candidate REVERSE deviation, recorded and not established** (CH-U8-02). An inbound
> `ProtocolHandshakeResponse` is accepted and its sender `add_conn`'d although the node has no record of
> soliciting a handshake — the diallee assumes the dialler's role, and that arm applies none of the
> `check_peer_on_same_network` gate the `ProtocolHandshake` arm applies
> (`comm/src/rp/handle_messages.rs:158-162` vs `:216`, `:237`). It is **not** written into Table A: no run
> shows it producing harm. Its nearest neighbours are H-U8-06's partial-guard class. It becomes a row when
> a run contradicts "unproven".

### Table A — deviations

| ID | Guide word | Deviation | Cause (anchor) | Consequence → TE | A |
|---|---|---|---|---|---|
| **H-U8-01** | NO | The transport inbound path (`send`/`stream`) has **no rate limiter** — only concurrency slots; the intent's "bounded in rate" is not met where peer messages actually arrive | `comm/src/transport/grpc_transport_receiver.rs:GrpcTransportReceiver::send` acquires `dispatch_slots` (`Semaphore`) and spawns, with no `RateLimiter`; the only limiters are `comm/src/discovery/grpc_kademlia_rpc_server.rs:GrpcKademliaRpcServer` (discovery, global) and `casper/src/engine/node_running.rs:PeerRateLimiter` (block requests only) | One authenticated peer drives all 1024 dispatch slots continuously; the node's dispatch to honest peers stays saturated → TE-1 | A3 | <!-- CH-U8-03 upheld -->
| ⚠ **H-U8-02** | MORE | More live TLS sessions than the table holds: `MAX_CONNECTIONS` (1024) caps the connections **table**, not the number of established inbound sessions | `comm/src/transport/grpc_transport_receiver.rs:accept_tls` accepts every TCP connection and bounds only *in-flight handshakes* (128, `MAX_CONCURRENT_HANDSHAKES`); `comm/src/rp/connect.rs:MAX_CONNECTIONS`/`add_conn` caps only the recorded table | A peer with many valid keys (one cert each) holds many live sessions, each a rustls state + task + fd → TE-4 | A3 | <!-- contested -->
| H-U8-03 | MORE | More of the shared request budget than one peer's share: the Kademlia rate limiter is **global**, not per-peer | `shared/src/rate_limiter.rs:RateLimiter` holds a single `window_start`/`count` for the whole service; `comm/src/discovery/grpc_kademlia_rpc_server.rs:DEFAULT_KADEMLIA_RATE_LIMIT_PER_SEC` (100/s) is one budget for all callers | One peer spends the 100/s window; every other peer's `send_ping`/`send_lookup` gets `ResourceExhausted`, so a joining node cannot discover peers → TE-3 | A2.3 |
| ⚠ **H-U8-04** | AS WELL AS | An inbound handshake carries an **outbound send as well**: answering it performs `transport.send` inside the dispatched handler, holding a dispatch slot across the outbound round-trip | `comm/src/rp/handle_messages.rs:handle_protocol_handshake` calls `transport.send(peer, response).await`; the slot is the receiver's `dispatch_slots` permit held for the spawned dispatch (`grpc_transport_receiver.rs:GrpcTransportReceiver::send`), bounded by `comm/src/transport/grpc_transport_client.rs:DEFAULT_SEND_TIMEOUT` (5 s) | A peer pins up to 1024 dispatch slots (5 s each) with handshakes whose reply never completes; the node stops answering every message type → TE-1 | A3 | <!-- contested -->
| ⚠ **H-U8-05** | AFTER | The streamed network-id/sender check runs **after** the whole stream is in memory | `comm/src/transport/grpc_transport_receiver.rs:GrpcTransportReceiver::stream` drains every chunk into a `Vec<Chunk>` first, then `comm/src/transport/stream_handler.rs:collect` runs the breaker (the `network_id`/`SenderNotVerified`/`MaxSizeReached` closure) | A foreign-network peer's stream is buffered up to the per-stream cap (256 MiB) and the aggregate budget (≈4 GiB) before it is refused → TE-4 | A3 | <!-- contested -->
| **H-U8-06** | PART OF | The SSRF/host guard covers **only part** of the ingress surface: the resolving guard is applied to the Kademlia ingress, but the transport handshake ingress classifies IP **literals only** | `comm/src/discovery/grpc_kademlia_rpc_server.rs:send_ping`/`send_lookup` call `comm/src/rp/handle_messages.rs:is_local_address_resolved`; but `comm/src/rp/handle_messages.rs:handle_protocol_handshake` → `check_peer_on_same_network` uses `is_local_address` (no resolution), then `add_conn` records the sender | A public node records a peer whose sender `host` is an attacker-controlled name and later **dials** it (`connect.rs:clear_connections`); the node's outbound work and connection table are aimed by the peer → TE-4 | A3 | <!-- CH-U8-04 upheld -->
| H-U8-07 | BEFORE | The SSRF check resolves the host **before** the dial, and the dial re-resolves — a short-TTL name answers public at check time and private at dial time (documented residual) | `comm/src/rp/handle_messages.rs:is_local_address_resolved` resolves at check time; the residual paragraph on that symbol names the check/dial window and says closing it needs the address pinned into `PeerNode` | The node dials a private/loopback address it would have refused to record, using a name it validated moments earlier → TE-4 | A3 |
| ⚠ **H-U8-08** | LATE | A finality-critical message is delivered **late**: the unary `Packet` path and the streamed-blob path share one bounded routing queue (depth 50), so a packet flood delays a streamed fringe | `node/src/runtime/node_runtime.rs:build_protocol_server` sends both `dispatch` (`handle_messages::handle`'s `Packet` arm) and `handle_streamed` into the *same* `routing_tx`; the queue is `mpsc::channel::<RoutingMessage>(50)` (`node_runtime.rs:setup_node_program`) | A peer's packet flood fills the FIFO ahead of a legitimate `FinalizedFringeRequest`/block stream, so it lands past the finality window → TE-1 | A3 | <!-- contested -->
| **H-U8-09** | EARLY | A connection is refused **early**, before any identity check: `accept_tls` drops the accepted TCP connection when no handshake slot is free | `comm/src/transport/grpc_transport_receiver.rs:accept_tls` `continue`s past the connection on `handshake_slots.try_acquire_owned()` failure (and on `listener.accept()` error), so an honest peer is dropped pre-authentication | The handshake-slot exhaustion is **per-service**: a peer holding all 128 transport handshake slots denies every inbound connection on the transport port (the joining validator's path, so TE-3 survives via that listener alone), while the Kademlia listener holds an independent 128-slot semaphore and is not denied by the same exhaustion — denying discovery needs a second, separate 128-connection exhaustion against the Kademlia port → TE-3 | A2.3 | <!-- CH-U8-01 upheld -->

### Table B — barriers

| Deviation | Preventive barrier (anchor) | Evidence | Independence |
|---|---|---|---|
| ⚠ **H-U8-01** | Concurrency, not rate: `comm/src/transport/grpc_transport_receiver.rs:ConcurrencyLimits` (`dispatch_slots`, `stream_slots`, `blob_slots`) returns `ResourceExhausted` at the cap | symbol read (`grpc_transport_receiver.rs`); the bound's mechanism is pinned by that file's test `a_full_dispatch_queue_is_refused` — a rate bound is **unmeasured** | blank (a concurrency gate is a different surface from a rate gate; it does not bound throughput over time) | <!-- contested -->
| ⚠ **H-U8-02** | **none** — the `comm/src/transport/grpc_transport_receiver.rs:accept_tls` handshake semaphore (`MAX_CONCURRENT_HANDSHAKES` + `HANDSHAKE_TIMEOUT`) guards a different deviation (R14, in-flight handshake pile-up); no barrier counts established sessions | symbol read (`grpc_transport_receiver.rs:accept_tls`); the established-session cap is **unmeasured** | blank | <!-- CH-U8-05 upheld -->
| H-U8-03 | `shared/src/rate_limiter.rs:RateLimiter` bounds total request rate (fixed 1 s window) | symbol read (`shared/src/rate_limiter.rs`); its unit test `admits_exactly_max_per_window_then_refuses` is committed | `[barrier-is-the-threat]` — the global limiter is both the bound and the exhaustible resource |
| ⚠ **H-U8-04** | `comm/src/transport/grpc_transport_client.rs:DEFAULT_SEND_TIMEOUT` (5 s) bounds each reply; the dispatch semaphore bounds concurrency | symbol read (`handle_messages.rs:handle_protocol_handshake`, the `if let Err(err) = transport.send(peer, response).await` arm); the amplification is **unmeasured** | blank | <!-- contested -->
| ⚠ **H-U8-05** | Per-stream cap `max_stream_message_size` and aggregate `stream_byte_budget` (`grpc_transport_receiver.rs:charge_stream_budget`) bound the buffered bytes | symbol read (`grpc_transport_receiver.rs:stream`, `stream_handler.rs:collect`); the cited `the_aggregate_budget_refuses_once_it_is_spent_and_returns_when_released` is the **byte half** (drives `charge_stream_budget`), not the breaker — the late `network_id`/`SenderNotVerified` check is **unmeasured** and owed a run | blank | <!-- CH-U8-08 upheld -->
| ⚠ **H-U8-06** | `comm/src/rp/handle_messages.rs:check_peer_on_same_network` (literal class) + `comm/src/peer_node.rs:MAX_HOST_BYTES` (length only) on the transport ingress | symbol read (`handle_messages.rs:is_local_address` vs `is_local_address_resolved`, and which callers use which) | `[one-surface]` — R24's resolution fix landed on the Kademlia ingress only | <!-- contested -->
| H-U8-07 | Only the check-time resolution `comm/src/rp/handle_messages.rs:is_local_address_resolved` (fail-closed on unresolved names) | symbol read (`handle_messages.rs:is_local_address_resolved`, residual paragraph); the TOCTOU window is **unmeasured** | blank |
| ⚠ **H-U8-08** | **none** — the await is the queueing mechanism, not a bound on it | residual control (AUDIT C105, the *loss* fix — not this deviation): `handle_messages.rs:handle` **awaits** `routing_queue.send`, so a packet is not lost, only queued; symbol read (`node_runtime.rs:build_protocol_server`, `mpsc::channel::<RoutingMessage>(50)`); the file's test `a_packet_is_not_dropped_when_the_routing_queue_is_full` is committed | `[barrier-is-the-threat]` — the await that queues is the same mechanism that delays | <!-- CH-U8-06 upheld -->
| ⚠ **H-U8-09** | `grpc_transport_receiver.rs:accept_tls` handshake semaphore bounds the slots; `HANDSHAKE_TIMEOUT` returns them after 10 s | symbol read (`grpc_transport_receiver.rs:accept_tls`); a per-source admission or a slot-fairness rule is **unmeasured** | blank | <!-- contested -->

### Provenance

read: `comm/src/rp/connect.rs`, `comm/src/rp/handle_messages.rs`, `comm/src/rp/protocol_helper.rs`, `comm/src/rp/rp_conf.rs`, `comm/src/peer_node.rs`, `comm/src/transport/grpc_transport_receiver.rs`, `comm/src/transport/grpc_transport_client.rs`, `comm/src/transport/stream_handler.rs`, `comm/src/transport/chunker.rs`, `comm/src/transport/hostname_trust_manager.rs`, `comm/src/transport/tls_conf.rs`, `comm/src/transport/transport_layer_syntax.rs`, `comm/src/discovery/grpc_kademlia_rpc_server.rs`, `comm/src/discovery/node_discovery.rs`, `comm/src/discovery/kademlia_node_discovery.rs`, `comm/src/discovery/kademlia_store.rs`, `comm/src/discovery/peer_table.rs`, `shared/src/rate_limiter.rs`, `casper/src/engine/node_running.rs` (PeerRateLimiter), `node/src/runtime/node_runtime.rs` (build_protocol_server, routing channel), `spec/findings.tsv` (R13/R14/R24/R26/R30/R31/R32, C136), `spec/AUDIT.md` (C115/C116/C117/C136 rows), `the study charter (outside the repo)` (§1 node list, §2 top events, §3 acceptance shape)

ran: `git rev-parse HEAD`; `git status --porcelain`; `grep`/`ls` for `RateLimiter`, `MAX_CONNECTIONS`, `accept_tls`, `routing` across `comm/`, `node/`, `casper/`, `shared/`, and `spec/` (all read-only)

not read: `comm/src/upnp/mod.rs`, `comm/src/upnp/gateway.rs` (UPnP/NAT external-address publishing — R31/C14 territory, outside the four anchors given); `comm/src/transport/grpc_transport.rs`, `grpc_transport_server.rs`, `messages.rs`, `packet_ops.rs`, `communication_response.rs`, `buffer/*` (dispatch/queue internals surveyed by name, not read line-by-line); `comm/src/who_am_i.rs`; `docs/src/node/testnet.md`, `security-audit.md`; `spec/audit/passes.md`, `spec/audit/evidence/*` (no run in the tree measures any wire-protocol barrier)



> **§1.9 — corrected 2026-10-04.** 6 adversarial challenges against this node's worksheet; **6 upheld, 0 refuted**, all applied below. A row whose ID still carries ⚠ has an element the adjudication left unestablished (unmeasured, or owed a re-run); every touched row cites its challenge id in an HTML comment.

### U9 — State, storage & DAG residency

**Design intent.** The DAG and its stores retain exactly what the liveness and finality rules may still need, at a residency an operator can size for.

**Guide words.** row: MORE, PART OF, LATE · folded: NO → MORE (absence of a release path *is* the over-retention), AS WELL AS → MORE (the extra material — full ancestry per message, every historical root — is more of the same retained state, not a second kind), REVERSE → MORE (append-not-replace is the mechanism of the MORE rows, and the backward `reset` to an old root is the roots-store MORE), OTHER THAN → MORE (the retained material is still state; keyed by content, not by role), EARLY → PART OF (the leaf-before-root commit *is* the partial-atomicity deviation), BEFORE → PART OF (same ordering), AFTER → LATE (the rebuild happens after the log is read) · contested: LESS (the stated structural reason is false — the DAG *does* have a removal path, but only for the advisory block-index cache (`casper/src/merging.rs:prune_cache`), while the message map (`block-storage/src/dag/message_map.rs:prune_fringe`) and the RSpace history (`rspace/src/history/radix_tree.rs:commit`, put-if-absent) never release; the LESS class must be re-argued from that fact).

### Table A — deviations

| ID | Guide word | Deviation | Cause (anchor) | Consequence → TE | A |
|---|---|---|---|---|---|
| ⚠ **H-U9-01** | MORE | **DAG message-state ancestry — every accepted message retains its whole ancestor `seen` set (Σ\|seen\| = N(N+1)/2) and no finalized ancestry is ever released** — an irreducible Θ(N²) full-ancestry cache (a law-15 construction, `block-storage/src/dag/message_state.rs:214-218`), whose retained depth is governed by the finalised fringe (the finality gap), not by the 5-height `LIVENESS_WINDOW`. **Contested:** the earlier "the finality rules need only the fringe plus the 5-height liveness window" sufficiency claim is refuted by the row's own anchors — a cap is what `spec/audit/passes.md:H6` records as breaking finalization — so the Θ(N²) is the retained value, not depth in excess of a 5-window | `block-storage/src/dag/representation.rs:seen_entries` (Σ\|seen\|, the Θ(N²) floor) + `block-storage/src/dag/message_map.rs:prune_fringe` (computes the index-cache fringe to prune; deletes nothing from the message map) + `block-storage/src/dag/liveness.rs:LIVENESS_WINDOW`; the recorded no-GC decision is `docs/src/node/scaling.md:residency-is-the-structural-limit` and `spec/audit/passes.md:H6` | Resident memory grows Θ(N²) (~1.3 MB/block measured; 556 MB of a 1.18 GiB process at 5,885 blocks) and the box exhausts → **TE-4** | A1.1 | <!-- CH-U9-03, CH-U9-04 upheld -->
| ⚠ **H-U9-02** | MORE | **The tuple-space history is append-only across all three stores** — cold leaves, radix nodes and roots are written put-if-absent and nothing is ever reclaimed; a `HistoryAction::Delete` rewrites the trie but frees no bytes, and every historical root stays "known" so `reset` can move back to any of them | `rspace/src/history/history_repository.rs:do_checkpoint_with_native` (cold leaves put-if-absent) + `rspace/src/history/radix_tree.rs:save_and_commit` / `:commit` (nodes put-if-absent, no delete) + `rspace/src/history/roots_store.rs:record_root` (every root kept) | On-disk history grows without bound and is never reclaimed → **TE-4** | A2.3 | <!-- contested -->
| ⚠ **H-U9-03** | PART OF | **The checkpoint is only partly atomic** — it commits the cold leaves, then the trie nodes, then the root, in *two* environments (`rspace/cold`, `rspace/history`), *three* logical Dbs and three separate transactions/awaits, so a crash between the steps leaves leaves/nodes that no recorded root references, retained forever | `rspace/src/history/history_repository.rs:do_checkpoint_with_native` (leaf put → node commit → root commit, three awaits) + `casper/src/storage.rs` (three Db environments: `rspace-cold`, `rspace-history`, `rspace-roots`); the write is named in `rspace/src/history/instances/rspace_history_reader_impl.rs` ("the two-step write the node performs at checkpoint") | Orphaned, unreferenced history data accumulates on every crash mid-checkpoint → **TE-4** | — | <!-- CH-U9-01 upheld -->
| H-U9-04 | LATE | **No snapshot is retained, so the DAG residency is rebuilt from the block log at every boot** — the "sum operator must size for" is the rebuild peak, not a stored image, and the rebuild is superlinear so a small host can fail to restart at all | `casper/src/dag.rs:BlockDagKeyValueStorage::create` (rebuilds the in-memory DAG from the stores each start) + `casper/src/block_metadata_store.rs:BlockMetadataStore::create`; measured in `docs/src/node/validator-requirements.md:ram-again-start-up-replay-is-the-floor` (1142-block replay: 57→284 MB, API after 225 s) and ranked as fix #2 in `docs/src/node/scaling.md:what-would-move-the-numbers` | A node that exhausts memory during replay never opens its API and loops on restart, so a validator cannot rejoin and an outsider cannot sync → **TE-3** | A2.3 |

### Table B — barriers

| Deviation | Preventive barrier (anchor) | Evidence | Independence |
|---|---|---|---|
| **H-U9-01** | **None preventive.** The node has no ancestry-release path; the decision was to accept and record the rate rather than bound it (`docs/src/node/scaling.md:residency-is-the-structural-limit`). What exists is a *detection* barrier only: the DAG publishes its own gauges | `casper/src/dag.rs:set_gauges` publishes `rchain.dag.seen_entries` / `logical_bytes`; `block-storage/src/dag/representation.rs:logical_bytes` is the value-accounting the gauges render; the measured rate is in `spec/audit/passes.md:H6` | one-surface (the gauges, `/api/status` and `/health` read the same in-process state — C195's class) | <!-- CH-U9-06 upheld -->
| ⚠ **H-U9-02** | **None preventive for size.** `RootRepository::validate_and_set_current_root` refuses an unknown root and `RootsStore::current_root` length-checks before building a hash — both guard *consistency*, neither bounds *growth* (`rspace/src/history/roots_store.rs:validate_and_set_current_root`) | The put-if-absent layout is read in `rspace/src/history/radix_tree.rs:commit`; the accepted residual ("what was fixed is every copy of it, not its size") is `spec/audit/passes.md:H6`; the orphan/size volume itself is `unmeasured` | one-surface (the consistency guard — `rspace/src/history/roots_store.rs:current_root` / `validate_and_set_current_root` — bounds the root-*consistency* surface; the growth deviation lives on the *size* surface it does not touch; shares the unit's absence-of-release cause with H-U9-01) | <!-- CH-U9-05 upheld -->
| ⚠ **H-U9-03** | The claim "each state change is a single LMDB transaction" (`docs/src/node/storage.md:atomicity`) is true **per store** but no barrier spans the leaf→node→root sequence, because the two environments are distinct and the three writes are three transactions (`casper/src/storage.rs`) | The two-step write is named in `rspace/src/history/instances/rspace_history_reader_impl.rs` and read in `history_repository.rs:do_checkpoint_with_native`; the recovered orphan volume is `unmeasured` | | <!-- CH-U9-01 upheld -->
| H-U9-04 | **Partial.** The exporter/importer move the *trie* between nodes (`node/src/runtime/node_runtime.rs:create_rspace_importer`, `rspace/src/history/export.rs:sequentialExport`), which shifts the sync cost but does not avoid the start-up DAG rebuild; no DAG snapshot exists (`casper/src/dag.rs` has no snapshot path) | `docs/src/node/validator-requirements.md:ram-again-start-up-replay-is-the-floor` (measured replay cost and the "1 GB host cannot restart a ~1000-block chain" finding) | |

### Provenance

read: `rspace/src/history/{mod,history,cold_store,roots_store,root_repository,radix_tree,export,key_segment,history_action}.rs`; `rspace/src/history/instances/radix_history.rs`; `rspace/src/history/instances/rspace_history_reader_impl.rs` (Rig two-step-write comment); `block-storage/src/dag/{representation,message_map,liveness,finalizer}.rs`; `casper/src/{storage.rs,dag.rs}`; `node/src/runtime/node_runtime.rs` (rspace store wiring); `docs/src/node/{scaling,storage,history-chain,validator-requirements}.md`; `spec/audit/passes.md` (§5 H6, §19 residency); `the study charter (outside the repo)` (study def: A1/A2/A3, TE-1..TE-4). · ran: `git rev-parse HEAD`, `git status --porcelain`, `ls`, `grep`, file reads (all read-only; no repository writes). · not read: `spec/AUDIT.md` C178/H6 rows in full; `spec/findings.tsv`; `spec/audit/evidence/` run artefacts in full (only grepped); `block-storage/src/dag/dag_storage.rs` and `metadata_store.rs` (grepped for removal paths, found none); `casper/src/merging.rs` (merge node, U4); the `bonds_map` population path per `Message`.

tree: `1e5a64ed4149d5bd7fa15affb1022e3c89f6c492` (matches the tree under audit) · dirty: `?? .claude/` only (no tracked modifications)



> **§1.10 — corrected 2026-10-04.** 8 adversarial challenges against this node's worksheet; **8 upheld, 0 refuted**, all applied below. A row whose ID still carries ⚠ has an element the adjudication left unestablished (unmeasured, or owed a re-run); every touched row cites its challenge id in an HTML comment.

### U10 — Operator surface

**Design intent.** An operator can observe block production, finality and the proposer-halt state, and restart the node back to the same chain state.

**Guide words.** row: NO/NOT, MORE, LESS, PART OF, OTHER THAN, LATE, AFTER · folded: REVERSE → NO/NOT, EARLY → LESS, AS WELL AS → PART OF, BEFORE → LATE *(contested — CH-U10-03: BEFORE and AFTER do not share a referent here)* · vacuous: none (7 rows ≥ 3). Note on the anchors: `/health` is **not a node route** — `node/src/web/http.rs::router` mounts `/version`, `/metrics`, `/status`, `/api/status` … and no `/health`; the `/health` the operator sees is an nginx+timer snapshot on the host (`docs/src/node/testnet.md` "Monitoring", `docs/src/node/validator-observer-setup.md` §8). That absence is itself load-bearing for H-U10-01's independence label.

### Table A — deviations

| ID | Guide word | Deviation | Cause (anchor) | Consequence → TE | A |
|---|---|---|---|---|---|
| ⚠ **H-U10-01** | NO/NOT | The proposer-halt is **not** reported by the CLI/gRPC `rnode status` surface — the wire `Status` has no health field, so a halted autopropose timer reads as a normal node to any CLI client | `models/src/casper/protocol/deploy_service.rs::Status` (8 fields, no health) and `node/src/api/grpc/tonic.rs::status` (builds only `status_to_wire`); `node/src/api/admin_web_api_impl.rs::operator_status` is the only path that folds in `proposer_health()`. C195 is the same missing-halt class on the *routed* HTTP path, fixed by `node/src/api/shard_routing.rs::proposer_health` | a halt goes unobserved through the surface an operator most likely scripts → TE-1 (escalation); the same missing signal reaches a client wired to the Scala-era `/status` comm route (H-U10-05's consequence, folded here) | A1.1 | <!-- CH-U10-06 upheld -->
| **H-U10-02** | MORE | **Historical / closed — C206 is fixed at HEAD, pinned by `node/src/web/pos_read.rs::a_pending_withdrawal_counts_down_to_the_deadline_the_store_holds`; the tree no longer produces this deviation.** `GET /api/v1/pos` reported a pending withdrawal's countdown **larger than truth by ~50,000 blocks** — the stored *deadline* was labelled `stagedAtBlock` and the quarantine added a second time | `node/src/web/pos_read.rs` derived `blocksRemaining = stagedAtBlock + quarantine_length − latest` over a value that already contained the quarantine (C206) | *(historical / closed — not a live deviation)* an operator would have waited past the withdrawal boundary it actually holds → TE-3 | A2.x | <!-- CH-U10-02 upheld -->
| ⚠ **H-U10-03** | LESS | `latestBlockNumber` on `/api/status` is a DAG **height**; a round mints one block per bonded validator, so an operator — or the #148/#149 probe — reading it as a block count **under-reports production by ~N** | `casper/src/api/block_api_impl.rs::status` sets it from `dag.get_representation().latest_block_number()` = the height-map max (`block-storage/src/dag/representation.rs::latest_block_number`); the field is named for blocks but counts rounds | criterion-1's own instrument mis-measures "production is bounded" (a live N-validator chain reads N× smaller) → TE-1 (false liveness reading) | A1.1 | <!-- contested -->
| ⚠ **H-U10-04** | PART OF | The halt is read from the **primary shard only**: `ShardRoutingBlockApi::proposer_health` delegates to `primary_api()`, so on a multi-shard gateway a non-primary shard whose timer has halted is invisible on `/api/v1/status/operator` | `node/src/api/shard_routing.rs::proposer_health` (contrast the per-shard `/metrics` gauge, which sees every shard) | a halted secondary shard goes unseen → TE-1 | A1.x | <!-- contested -->
| **H-U10-05** | OTHER THAN *(unsupported)* | `/status` answers about the **comm layer, not the node**: it returns only `address/version/peers/nodes` — no height, no finality, no halt — so a client wired to the Scala-era route reads a halted node as live — *(OTHER THAN unsupported: `/status` is the faithful port of the legacy `StatusInfo.service` (`legacy/node/src/main/scala/coop/rchain/node/web/StatusInfo.scala` @ `1b7583649`), so it answers as intended and OTHER THAN has no referent; the residual missing-signal point is folded into H-U10-01's NO/NOT row)* | `node/src/web/status_info.rs::Status` (four fields) mounted beside `/api/status` by `node/src/web/http.rs::router` | a monitor bound to `/status` cannot see a halt → TE-1 | A1.x | <!-- CH-U10-06 upheld -->
| **H-U10-06** | AFTER | After a restart the surface **forgets a halt it just reported**: `autopropose_timer_halted` lives in an in-memory atomic, so a process that latched it reports `false` again the instant it restarts — "restart to the same state" restores the chain, not the observation | `casper/src/api/block_api.rs::ProposeHealth` (`Arc<AtomicBool>` / `Arc<AtomicU64>`, no store); the halt is written by `casper/src/api/block_api.rs::note_timer_halted` and never persisted | an operator restarts to clear an alarm and gets a false all-clear; the root cause is lost → TE-1 (escalation) | A2.x | <!-- CH-U10-01 upheld -->
| H-U10-07 | LATE | After a restart the observation surface is **absent for the whole replay** (measured ~0.2 s and 0.25 MB per block; ~3.5 min at 1,140 blocks) and the unit still reports `active` while the API never answers — an operator cannot tell "replaying" from "wedged" | the API opens only after start-up replay: `docs/src/node/validator-observer-setup.md` §9 (restart replay) and §8 (`systemctl is-active` is not health) | a rejoining/restarting validator that is merely replaying looks stuck and may be killed → TE-3 | A2.x |

Ack of the folded words, so each has exactly one outcome: **REVERSE → NO/NOT** — at this node the reverse of the intent is "a halt that is not reported reads as healthy" (C195), the same missing-signal defect, not a separable one. **EARLY → LESS** — `latestBlockNumber` is the DAG head, so it reports a height *before* the blocks at that height are finalized; the same field and the same remedy (a field named for what it counts, plus a separate finalized read) as LESS, not separable. **AS WELL AS → PART OF** — "the surface shows one facet in addition to the intended one" reduces to the same referent here (a surface with partial scope: one shard, or one of two status shapes). **BEFORE → LATE — contested.** before and after are **not** the same referent: H-U10-06 (AFTER) is the halt-state loss at the restart instant (TE-1), while H-U10-07 (LATE) is the API's absence for the replay window (TE-3); the fold onto LATE rests on that pairing, not on an identity. <!-- CH-U10-03 upheld -->

### Table B — barriers

| Deviation | Preventive barrier (anchor) | Evidence | Independence |
|---|---|---|---|
| ⚠ **H-U10-01** | The HTTP path folds `proposer_health()` in (`node/src/api/admin_web_api_impl.rs::operator_status`); the routed path is guarded by `node/src/api/shard_routing.rs::proposer_health` (the C195 fix, asserted by `the_primary_answers_every_unrouted_method`) | read: `models/src/casper/protocol/deploy_service.rs::Status` (no health field); `node/src/api/grpc/tonic.rs::status`. The gRPC/CLI path has **no** halt barrier — `unmeasured` on the wire | `[one-surface]` — the halt is guarded on the HTTP surface only; the operator's other halt-reading view (`/metrics`) reads the *same* in-process state, so they are one barrier, **not two** | <!-- CH-U10-04 upheld -->
| ⚠ **H-U10-02** | The pinned arithmetic test `node/src/web/pos_read.rs::a_pending_withdrawal_counts_down_to_the_deadline_the_store_holds` — the pinned test is the remedy's own falsifier, not a preventive barrier against a live hazard (the deviation is historical / closed) | `node/src/web/pos_read.rs` (test present at line 260) — `unmeasured`: cites the test's presence, not a green run | | <!-- CH-U10-02 upheld, CH-U10-08 upheld -->
| **H-U10-03** | None names blocks — no field, and no operator doc, distinguishes the height from a block count | read: `block-storage/src/dag/representation.rs::latest_block_number`; `tools/probe-blocks-per-deploy.sh` samples `latestBlockNumber` as its height; ran: `spec/audit/evidence/n149-results.md:23` (blocks = N exactly, 3/3 at N ∈ {3,5,8}) and `spec/audit/evidence/n127-liveness-preregistration.md:20` (the `latestBlockNumber` = round-count correction), with artifacts under `spec/audit/evidence/n149-blocks/cf3945045-20261001T152307Z` | | <!-- CH-U10-05 upheld -->
| ⚠ **H-U10-04** | The per-shard halt gauge `rchain_proposer_shard_{n}_autopropose_timer_halted` | `node/src/runtime/node_runtime.rs::push_proposer_health`, test `a_halted_timer_is_readable_on_the_scrape` (present at line 3031) — `unmeasured`: cites the test's presence, not a green run | `[one-surface]` — `/api/v1/status/operator` reads one shard, `/metrics` reads all *(structural/unwitnessed coverage)*; the two disagree about a gateway. The gauge is a different surface than `/api/status`, but its coverage of non-primary shards is inferred from the per-shard emitter loop (`node_runtime.rs:1165`, `:1551`), not rendered in any committed scrape | <!-- CH-U10-07 upheld, CH-U10-08 upheld -->
| ⚠ **H-U10-05** | None — `/status` is a second, thinner shape; nothing steers a client to `/api/status` | read: `node/src/web/status_info.rs::Status` | | <!-- contested -->
| ⚠ **H-U10-06** | The deliberate design ("nothing restarts the timer but the process", `node/src/runtime/node_runtime.rs` `AUTOPROPOSE_MAX_CONSECUTIVE_FAILURES`) is also the hazard: a restart clears the symptom while the cause may persist | read: `casper/src/api/block_api.rs::ProposeHealth` (`Arc<AtomicBool>`) | `[barrier-is-the-threat]` | <!-- contested -->
| H-U10-07 | None on the node itself; the operator's external snapshot carries `api_reachable` but ignores the halt fields entirely | `docs/src/node/validator-observer-setup.md` §8 (snapshot/route note), §9 (replay); `docs/src/node/testnet.md` "Monitoring" | `[self-referential]` — the snapshot's liveness signal (`api_reachable`) is exactly what the replay suppresses |

### Provenance

read: `node/src/web/http.rs`, `node/src/web/status_info.rs`, `node/src/web/pos_read.rs`, `node/src/api/dto.rs`, `node/src/api/conversion.rs`, `node/src/api/shard_routing.rs`, `node/src/api/web_api_impl.rs`, `node/src/api/grpc/tonic.rs`, `node/src/runtime/node_runtime.rs`, `node/src/configuration/commandline/options.rs`, `models/src/casper/protocol/deploy_service.rs`, `casper/src/api/block_api.rs`, `casper/src/api/block_api_impl.rs`, `casper/src/protocol/client.rs`, `block-storage/src/dag/representation.rs`, `tools/probe-blocks-per-deploy.sh`, `spec/AUDIT.md` (C195, C206 rows), `spec/findings.tsv` (C195, C206), `spec/audit/passes.md` §C148, `docs/src/node/testnet.md`, `docs/src/node/running-a-public-testnet.md`, `docs/src/node/operating.md` (grep), `the study charter (outside the repo)` (study charter) · ran: `git rev-parse HEAD`, `git status --porcelain`, `git ls-files tools/`, grep/read only · not read: `spec/audit/evidence/*` result files (listed, not opened), the `/api/v1/openapi.json` body, `node/src/web/pos_read.rs` in full, `docs/src/node/scaling.md`, `docs/src/node/consensus.md`, `spec/TYPE-SYSTEM.md`.

HEAD: `1e5a64ed4149d5bd7fa15affb1022e3c89f6c492` (matches the audit tree). Working tree: clean except untracked `.claude/` (`?? .claude/`).

---

# 2. Bow-tie analyses


Four **top events** — the loss of control of a hazard — were drawn as a bow-tie: threats on the left,
preventive barriers between each threat and the top event, consequences on the right, mitigating barriers
after it, and **degradation factors** that weaken a barrier.

| | Top event |
|---|---|
| **TE-1** | The chain halts and cannot recover |
| **TE-2** | A fragment of the net reaches finality that the rest rejects |
| **TE-3** | An outsider validator cannot safely join or leave |
| **TE-4** | State, disk or memory exhaustion halts the net |

A fifth candidate — *"the operator cannot see or prove what happened"* — was **rejected as a top event**
and demoted to a degradation factor present in all four. It has no distinct consequence of its own (no
chain halts *because* monitoring is thin), and putting it on the top line would double-count it four
times. It reappears in every degradation-factor column, and its barrier-independence finding (the
`[one-surface]` class, §2.5) is one of the audit's sharpest results: `/metrics`, `/api/status` and
`/health` are not three independent views of the node's health; they read the same in-process state, so
they are one barrier, not three.

### TE-1 — The chain halts and cannot recover

**Top event.** The chain halts and cannot recover — finality stops advancing, production stops or
outruns it, and no in-protocol mechanism returns the node set to a finalising chain.

**Scope.** Synthesised from the ten study-node sheets (`s1-U1..U10`) and their challengers
(`s2-U1..U10`). Every cell carries an `path:symbol`, a committed `spec/audit/evidence/…` run, a `#NN`,
or `[unhoused]`/`unmeasured`. An issue's state and a register row marked done are not evidence; a
barrier whose only support is a "symbol read" is marked `unmeasured` unless a run exercises it.

---

**Threats.** *(the credible reasons loss of control occurs, drawn across all study nodes)*

- **U1/U2 — the proposal request stops.** Once a full round of attestations comes to rest at one height
  the attest tap issues no further proposal request; the round then cannot re-open.
  `node/src/runtime/node_runtime.rs:attest_warranted` (`height > last_attested_height`; per-sender since
  `e6d41a347`). Evidence `spec/audit/evidence/n149-results.md` — **contested** (CH-U1-01/02: the cited
  run is the pre-fix tree `cf3945045`, and it measures *N blocks built then nothing*, the seal being on
  the next round, not on all production).
- **U2/U3 — the round cannot close on a killed or lagging bonded sender.** The retirement clock is
  measured from a tip the round veto freezes. `block-storage/src/dag/message_state.rs:has_advanced_past_the_round`,
  `:advance_round`. Evidence `spec/audit/evidence/n149-results.md` (guard-live: N∈{3,5,8} finalise
  **never** in 180 s) and `spec/audit/evidence/n148-results.md` (kill freezes finality, 3/3).
- **U3 — the fringe gate refuses for want of a new layer.** A DAG resting at one height yields
  `full_partitions: 0, supporting: 0` before any quorum test. `block-storage/src/dag/finalizer.rs:calculate_next_fringe_support_map`,
  `:calculate_fringe_numbers`. Evidence: symbol read — the `0/0` figure is uncited (CH-U3-04).
- **U3/U6 — the quorum denominator is the whole bonded map.** A permanent ≥1/3 stake loss (equal stakes:
  killing one of three leaves 2/3, not `> 2/3`) makes the quorum permanently unreachable; a rejoining
  validator is *required*, not optional. `block-storage/src/dag/liveness.rs:live_weight_set` +
  `block-storage/src/dag/finalizer.rs:calculate_fringe`. Evidence `spec/audit/evidence/n148-results.md`.
- **U3 — a retired-but-never-evicted sender is carried in `mv.parents`.** Resolved through the full
  `msg_map`, it lands in a `seen_by` value so no candidate is a full partition.
  `block-storage/src/dag/finalizer.rs:calculate_next_fringe_support_map`. Evidence `unmeasured` (the
  Table-A mechanism is refuted by `live_justifications` — CH-U3-01).
- **U4 — the merge search exceeds its budget → no answer, block dropped with no re-queue.** The node
  cannot advance at that height until the scope shrinks. `sdk/src/dag/merging.rs:SearchBudget::NODE` +
  `casper/src/merging.rs:MergeScope::merge` (`ValidateError::Internal`). Evidence `sdk/tests/merging_scaling.rs`
  — **contested** (CH-U4-02: the cited gate runs `SearchBudget::UNBOUNDED`).
- **U4/U6 — the merge's `fold` Err on a missed concurrent-writer pair rejects the block whole.**
  `casper/src/merging.rs:NativeRelations::conflicting` / `MergeScope::fold`. Evidence
  `spec/audit/evidence/c207-merge-loses-native-writes.md`.
- **U6 — native set-leaf writes are lost at a boundary merge.** A `delegate`/`trust`/`withdraw` writing
  `pos:delegations`/`pos:trusted`/`pos:pending_withdrawers` is discarded whole when a concurrent sibling
  writes the same leaf. `rholang/src/native_state.rs::delegate`, `::trust`, `::withdraw`;
  `casper/src/merging.rs::boundary_merge_tests`. Evidence `spec/audit/evidence/c207-merge-loses-native-writes.md`
  (§5 residual, prose) + `spec/audit/passes.md` §61 (block 206 `delegate` discarded).
- **U6 — a bond-set change on an empty/pruned fringe refuses.** The finaliser's bonds fallback refuses a
  disagreeing carried set, halting the chain. `casper/src/multi_parent_casper.rs` (`compute_bonds`
  fallback) + `spec/Rchain/Casper/Bonds.lean::a_disagreeing_set_is_refused`. Evidence
  `docs/src/node/testnet.md:624` — **contested** (CH-U6-06: that is the pre-fix #73 shape; the empty-fringe
  wedge is `unmeasured`, and the one live reading witnesses the *non*-empty case).
- **U6 — the epoch draw can drop the only validator able to propose.** The chain halts silently
  (`NotBonded`). `rholang/src/native_state.rs::select_active`, `::close_block` (`is_epoch_boundary`).
  Evidence `spec/RUST-VS-SCALA.md:288-300` (measured twice on a devnet, with a control).
- **U7 — admission exhausted / the proposer rejects its own block → autopropose halts.** A `phlo_limit >
  MAX_BLOCK_PHLO` deploy is admitted, never validly includable; the proposer refuses its own block until
  `consecutive_failures` halts the timer. `casper/src/validate.rs:block_phlo` / `MAX_BLOCK_PHLO`;
  `casper/src/dag.rs:BlockDagKeyValueStorage::add_deploy`; `:insert` (expiry only from insert). Evidence
  `unmeasured` (no U7 run exists).
- **U1 — the broadcast is fire-and-forget.** A proposal reaches no peer while the node logs it proposed.
  `node/src/runtime/node_runtime.rs:propose_effect` → `casper/src/protocol/comm_util.rs:send_block_hash`
  (returns `()`, logs success unconditionally). Evidence `unmeasured` (CH-U1-06).
- **U8 — a peer pins the dispatch slots / a packet flood delays a finality-critical stream.**
  `comm/src/rp/handle_messages.rs:handle_protocol_handshake` (`transport.send(peer, response)` held over
  the reply) and `node/src/runtime/node_runtime.rs:build_protocol_server` (routing queue depth 50).
  Evidence `unmeasured` (no wire run in the tree).
- **U5 — a joiner replays a restored chain to a different post-state.** An empty fringe on a boundary
  block diverges the joiner's own chain — the #139 shape — and splits it if it then proposes.
  `casper/src/engine/node_syncing.rs:populate_dag`. Evidence `spec/audit/evidence/n139-mature-join-results.md`
  — **re-anchored to TE-3** (CH-U5-04: the run shows the bootstrap running on, not the chain halting).
- **U10 — the halt is unobservable on the surfaces an operator scripts.** A stalled autopropose timer
  reads as a normal node; a restart forgets the halt. `models/src/casper/protocol/deploy_service.rs::Status`
  (no health field), `node/src/api/grpc/tonic.rs::status`, `node/src/web/status_info.rs::Status`,
  `casper/src/api/block_api.rs:ProposeHealth` (in-memory). Evidence `spec/AUDIT.md` C195/C206;
  `docs/src/node/testnet.md:153`. This is the escalation factor: it converts a recoverable stall into an
  unrecoverable one.

---

**Preventive barriers.** *(what exists today between each threat and the top event)*

- **The liveness escape** — `casper/src/blocks/proposer/proposer.rs:create_block` (escape branch) /
  `block-storage/src/dag/message_state.rs:parents_for_new_block_escaping`. Evidence
  `spec/audit/evidence/n148-results.md`, `spec/audit/evidence/n149-results.md` — **contested**: the escape
  needs a proposal *request* to increment `blocked_since_advance`, and the tap seal removes exactly that
  request, so the barrier cannot fire (CH-U1-03/04, CH-U2-03).
- **The round veto + receiver sequence check** — `casper/src/blocks/proposer/proposer.rs:create_block`
  (`AlreadyProposedThisRound`), `casper/src/validate.rs:sequence_number`. Evidence `unmeasured`.
- **The liveness filter / window retirement** — `block-storage/src/dag/liveness.rs:live_weight_set`.
  Evidence `spec/audit/evidence/n148-results.md` (finality frozen at the kill, 3/3) — the deployment that
  *is* the threat on the guard-live arm (CH-U2-03).
- **The round snapshot** — `block-storage/src/dag/message_state.rs:parents_for_new_block`. Evidence
  `casper/tests/finalization.rs::a_round_snapshot_of_the_latest_messages_is_what_the_gate_needs` (symbol read).
- **The merge budget, applied before each unit of work** — `sdk/src/dag/merging.rs:search`, Law 55,
  `spec/Rchain/Bounded.lean:the_work_never_exceeds_the_budget`. Evidence `sdk/tests/merging_scaling.rs`
  — **contested** (the gate runs `UNBOUNDED`; the real falsifier is `sdk/src/dag/merging.rs:a_budget_refuses_without_answering_and_never_changes_the_answer`, CH-U4-02).
- **The exactness differential vs a literal enumeration** — `sdk/src/property_tests.rs:rejection_options_match_a_literal_enumeration`.
  Evidence: committed test (challenger calls it `independent`, correcting the sheet's `[shared-cause]`).
- **The ancestry-derived native relation** — `casper/src/merging.rs:NativeRelations::conflicting`. Evidence
  `spec/audit/evidence/c207-merge-loses-native-writes.md` — but that evidence documents the threat
  materialising, not the barrier holding (CH-U4-03).
- **The whole-block rejection of the losing sibling boundary block** —
  `casper/src/merging.rs::boundary_merge_tests::sibling_boundaries_with_different_pots_merge`. Evidence
  `spec/audit/passes.md` §61 (block 206 `delegate` discarded — i.e. the barrier/hazard, see SCP-1).
- **The finaliser refusing two disagreeing bonds maps** — `spec/Rchain/Casper/Bonds.lean::a_disagreeing_set_is_refused`.
  Evidence: model witness (read); the live residual is `unmeasured` beyond `spec/RUST-VS-SCALA.md:310-315`.
- **Cost accounting merged per accepted deploy** —
  `casper/src/merging.rs::a_cost_accounted_block_keeps_its_own_native_writes_through_the_merge`. Evidence
  `spec/audit/evidence/c207-merge-loses-native-writes.md` §5-6 — covers sums only, **not** the set-valued
  leaves (the H-U6-01 residual, unmeasured — CH-U6-07).
- **`slash_is_unjustified` re-derives the slash set** — `casper/src/interpreter_util.rs:slash_is_unjustified`.
  Evidence: symbol read (bounds the slash surface only — CH-U7-02).
- **The sync `terminal` bound** — `casper/src/engine/node_syncing.rs:MAX_SYNC_ATTEMPTS` + `fringe_arrived`.
  Evidence `unmeasured` — **contested** (CH-U5-01: `apply` returning `Err` only logs, it does not exit).
- **`block_phlo` refuses an over-budget block** — `casper/src/validate.rs:block_phlo`. Evidence
  `unmeasured` (sits on the block path only, not at admission).
- **`expire_deploys` on every accepted insert** — `casper/src/dag.rs:BlockDagKeyValueStorage::insert`.
  Evidence `unmeasured` (the H-U7-06 barrier-is-the-threat pair).
- **The send timeout + dispatch semaphore** — `comm/src/transport/grpc_transport_client.rs:DEFAULT_SEND_TIMEOUT`
  (5 s), `comm/src/transport/grpc_transport_receiver.rs:ConcurrencyLimits`. Evidence: symbol read; no wire
  run in the tree.
- **No-drop routing backpressure** — `comm/src/rp/handle_messages.rs:handle` awaits `routing_queue.send`.
  Evidence: committed test `a_packet_is_not_dropped_when_the_routing_queue_is_full`.
- **HTTP `/api/v1/status/operator` folds `proposer_health()` in** — `node/src/api/admin_web_api_impl.rs::operator_status`; routed path
  `node/src/api/shard_routing.rs::proposer_health`. Evidence `spec/AUDIT.md` C195 + the test
  `the_primary_answers_every_unrouted_method` (HTTP surface only — CH-U10-04).
- **Bond/epoch activation at the boundary** — `rholang/src/native_state.rs::close_block` +
  `:is_epoch_boundary`. Evidence `docs/src/node/testnet.md:619-628` (a reading on a live chain, not a
  committed run).
- **The SSRF guard** — `comm/src/rp/handle_messages.rs:check_peer_on_same_network`,
  `:is_local_address_resolved`. Evidence: symbol read (R24/C117; the handshake ingress keeps literal-only
  matching — CH-U8-06).

---

**Top event.** The chain halts and cannot recover.

---

**Consequences.** *(what follows once control is lost)*

- **Finality freezes at the seal/kill while production continues** (or both stop), and the unfinalised
  set grows without bound. `spec/audit/evidence/n148-results.md`, `n149-results.md`.
- **Deploys are held unfinalised or silently lost**: a `delegate`/`withdraw` riding a rejected block takes
  effect on no node (`spec/audit/passes.md` §61); a deploy whose block was orphaned is neither held nor
  finally included (`casper/src/dag.rs:BlockDagKeyValueStorage::insert`).
- **The quorum becomes permanently unreachable** after a ≥1/3 stake loss: a rejoining validator is
  required, not optional — recovery is impossible without it (`spec/audit/evidence/n148-results.md`).
- **The chain cannot self-heal**: the escape, the window retirement, the expiry and the fringe all measure
  against the same frozen tip, and there is no inactivity leak (deliberate, Law 44 —
  `spec/audit/evidence/n148-results.md`, `block-storage/src/dag/liveness.rs:LIVENESS_WINDOW`).
- **A new validator joining, and a killed validator rejoining** — the first has now run and passes
  (`spec/audit/evidence/n220-join-results.md`); the second is §3.2's reading. Both were `fail`/`unmeasured`
  when this first pass was written.
- **The halt is invisible on the CLI/gRPC and `/status` surfaces**; a restart clears the alarm without the
  cause, so the root cause and the observation are both lost (`spec/AUDIT.md` C195; `docs/src/node/testnet.md:153`).

---

**Mitigating / recovery barriers.** *(what reduces severity after the top event)*

- **The escaping parent set** — `block-storage/src/dag/message_state.rs:parents_for_new_block_escaping`
  moves the tip so the round can still close. Evidence `unmeasured`/self-referential (the escape and the
  window both measure against the frozen tip).
- **The sync `terminal` bound** — `casper/src/engine/node_syncing.rs:MAX_SYNC_ATTEMPTS`. Evidence
  `unmeasured`; **contested** (does not exit — CH-U5-01).
- **The newer-fringe trigger** `casper/src/engine/node_syncing.rs:fringe_arrived` shortens a retry.
  Evidence `unmeasured`.
- **Process restart** restores chain state and clears a latched autopropose halt — the symptom only.
  `casper/src/api/block_api.rs:ProposeHealth` (in-memory; no store).
- **The per-shard `/metrics` gauge** `rchain_proposer_shard_{n}_autopropose_timer_halted` surfaces a halted
  shard — `node/src/runtime/node_runtime.rs::push_proposer_health`. Evidence: test asserts **shard 0 only**
  (structural coverage, not witnessed — CH-U10-07).
- **The exporter/importer move the trie between nodes** — `node/src/runtime/node_runtime.rs:create_rspace_importer`,
  `rspace/src/history/export.rs:sequentialExport`. Evidence `docs/src/node/validator-requirements.md` (shifts
  the sync cost, not the start-up rebuild).
- **The DAG head-cache release at finality** — `casper/src/merging.rs:BlockIndex::prune_cache`. Evidence:
  symbol read (bounds resident state so a box can restart — the LESS referent, CH-U9-02).

---

**Degradation (escalation) factors.** *(conditions that weaken a barrier, each with its own control or "none")*

- **The frozen tip.** It is the clock every retirement barrier measures against; one event both freezes
  finality and disables the barriers meant to break it. Control: **none** (`[self-referential]`,
  `block-storage/src/dag/message_state.rs:has_advanced_past_the_round`).
- **`--autopropose` + the dev-mode deployer key.** The trigger of the unbounded-production deviation and
  its only gate are one boolean. Control: run the record without autopropose (the guard-live arm) —
  `node/src/configuration/defaults.conf:8`.
- **The 100/s limiter is a single exhaustible window.** Control: **none** (`[barrier-is-the-threat]`,
  `shared/src/rate_limiter.rs:RateLimiter`).
- **The attest tap's `answered` map.** Bounds a burst and loses the attestation — the same map.
  Control: **none** (`node/src/runtime/node_runtime.rs:attest_on_new_blocks`).
- **The parent cap (255).** C191 says it binds no set a bonded network can produce. Control: **none**
  (`spec/findings.tsv` C191; `casper/src/validate.rs:justification_count`).
- **The whole-map quorum denominator.** It is the safety requirement and cannot shrink. Control: **none**
  (deliberate stop — CH-U3-06).
- **Operator surfaces read the same in-process `ProposeHealth`.** `/metrics`, `/api/status` and the
  nginx-derived `/health` are one barrier, not three. Control: the per-shard gauge (structural, not
  witnessed — `node/src/api/shard_routing.rs::proposer_health`).
- **The halt lives in an in-memory atomic.** A restart forgets it. Control: **none**
  (`casper/src/api/block_api.rs:ProposeHealth`).
- **The transport inbound path has no rate limiter.** Control: the concurrency semaphore only
  (`comm/src/transport/grpc_transport_receiver.rs:GrpcTransportReceiver::send`).
- **A non-bootstrap `FinalizedFringe` is processed.** The sender check is a log line with no `return`.
  Control: **none effective** (`casper/src/engine/node_syncing.rs:on_finalized_fringe_message`).
- **The per-sender tap bounds proposals by the round rule (C192).** Control: the round veto — which is
  itself the seal (`[barrier-is-the-threat]`, `casper/src/blocks/proposer/proposer.rs:create_block`).

---

**Shared-cause pairs and independence findings.** *(each names both members and the shared cause)*

- **SCP-1 (canonical).** Threat "a bond/withdraw/delegate riding the losing sibling boundary block takes
  effect on no node" (U6 H-U6-01/H-U6-03) ↔ barrier "the merge rejects the losing sibling boundary block
  whole" (U6 H-U6-03, `casper/src/merging.rs::boundary_merge_tests::sibling_boundaries_with_different_pots_merge`).
  Shared cause: the merge's whole-block rejection — a bond takes effect only on the boundary block the
  merge keeps, so "no successful boundary merge happens" both disables the barrier (bond activation) and
  is the event the barrier exists to prevent ([barrier-is-the-threat]).
- **SCP-2.** Threat "the round never closes → finality freezes while production continues" (U2 H-U2-03) ↔
  barrier "the escape `parents_for_new_block_escaping` moves the tip so the round can close"
  (U3 H-U3-04). Shared cause: the frozen tip — the round's clock (`tip − round_height > LIVENESS_WINDOW`
  in `block-storage/src/dag/message_state.rs:advance_round`) and the escape's clock are the same tip the
  veto freezes. Cross-node U2↔U3.
- **SCP-3.** Threat "the attest tap stops issuing proposal requests once a round rests" (U1 H-U1-01) ↔
  barrier "the liveness escape increments `blocked_since_advance`"
  (`casper/src/blocks/proposer/proposer.rs:create_block`). Shared cause: the tap seal — the escape needs a
  proposal *request* to increment, and the tap seal removes exactly that request ([self-referential]).
- **SCP-4.** Threat "a pool that never drains" (U7 H-U7-06) ↔ barrier "`expire_deploys` runs on every
  accepted block insert" (`casper/src/dag.rs:BlockDagKeyValueStorage::insert`). Shared cause: the same
  event (an accepted block / the #213 wedge) is what the barrier needs to run and what stops it
  ([shared-cause]).
- **SCP-5.** Threat "finality frozen, the tip cannot advance" (U3 H-U3-01/H-U3-04) ↔ barrier
  "`live_weight_set` retires an absent sender so the round can close"
  (`block-storage/src/dag/liveness.rs:live_weight_set`). Shared cause: the frozen tip that blocks the
  fringe freezes the clock the retirement is measured from. Cross-node U1↔U3.
- **SCP-6.** Threat "a permanent ≥1/3 loss makes the quorum unreachable" (U3 H-U3-02) ↔ barrier "the whole
  bonded map as the quorum denominator" (`block-storage/src/dag/finalizer.rs:calculate_fringe` summing
  `total_stake` over `quorum_bonds`). Shared cause: the same denominator — the property that keeps the
  quorum safe is what makes it unreachable ([barrier-is-the-threat]).
- **SCP-7.** Threat "a retired-but-never-evicted sender carried in `mv.parents` → no full partition"
  (U3 H-U3-03 residual) ↔ barrier "the liveness filter `live_justifications` restricts the justification
  set to live senders" (`block-storage/src/dag/liveness.rs:calculate_finalization_detailed`). Shared
  cause: the live set — the same live partition gates the round and the fringe, and the filter does not
  reach the `mv.parents` surface ([shared-cause] per the sheet; the challenger relabels `[one-surface]` —
  **contested**).
- **SCP-8.** Threat "the finaliser refuses two disagreeing bonds maps → chain halts" (U6 H-U6-02) ↔
  barrier "the same agreement requirement in `compute_bonds`' fallback" (U6 H-U6-04). Shared cause: the
  `compute_bonds` agreement requirement — one defect disables both ([shared-cause]).
- **SCP-9.** Threat "the merge returns no answer (budget exceeded) → block dropped with no re-queue"
  (U4 H-U4-01) ↔ barrier "the budget applied before each unit of search work"
  (`sdk/src/dag/merging.rs:search`, Law 55). Shared cause: the budget's own refusal *is* the row's
  consequence — one event (the budget firing) is both the barrier and the drop ([barrier-is-the-threat]).
- **SCP-10.** Threat "the `fold` Err on a missed concurrent-writer pair rejects the block whole"
  (U4 H-U4-03) ↔ barrier "the native relation derived from the DAG's own ancestry, so every node computes
  it identically" (`casper/src/merging.rs:NativeRelations::conflicting`). Shared cause: the
  ancestry-derived native relation — the determinism that keeps nodes agreeing is the surface the
  deviation's completeness lives on; a missed pair makes *every* node drop identically ([one-surface]).
- **SCP-11.** Threat "the proposer builds a block its own `validate_block` refuses → autopropose halts"
  (U7 H-U7-02) ↔ barrier "`block_phlo` refuses an over-budget block"
  (`casper/src/validate.rs:block_phlo`). Shared cause: the phlo bound sits on the block path only, not at
  admission — the bound converts a bad deploy into a proposer halt ([one-surface]).
- **SCP-12.** Threat "the admission bound refuses honest senders" (U7 H-U7-01) ↔ barrier "the shared pool
  cap + per-surface limiters as backstop" (`casper/src/dag.rs:BlockDagKeyValueStorage::add_deploy`).
  Shared cause: the global bound both bounds the flood and is what refuses honest senders
  ([barrier-is-the-threat]).
- **SCP-13.** Threat "a halted node is invisible on the CLI/gRPC surface" (U10 H-U10-01) ↔ barrier "HTTP
  `/api/v1/status/operator` folds `proposer_health()` in" (`node/src/api/admin_web_api_impl.rs::operator_status`). Shared cause: the
  in-process `ProposeHealth` state — the halt is guarded on the HTTP surface only, and `/metrics`,
  `/api/status` and `/health` read the same in-process state, so they are one barrier, not three
  ([one-surface], proven by C195).
- **SCP-14.** Threat "a halted node reads as live after restart (false all-clear)" (U10 H-U10-06) ↔
  barrier "the deliberate design: nothing restarts the timer but the process"
  (`casper/src/api/block_api.rs:ProposeHealth`). Shared cause: the in-memory atomic + process-restart
  design — the property that makes the halt sticky is exactly the property that loses the observation on
  restart ([barrier-is-the-threat]).
- **SCP-15.** Threat "the sync waits unbounded for the pre-attempt fringe" (U5 H-U5-01) ↔ barrier "the
  tuple-space leg is unbounded, so `MAX_SYNC_ATTEMPTS`/`terminal` never fires" (U5 H-U5-03). Shared cause:
  no bound on waiting for the peer — one event disables the guard and is the hazard ([shared-cause]).
- **SCP-16.** Threat "the first stalled sync consumes the one-shot trigger" (U5 H-U5-05) ↔ barrier "C181's
  `terminal` bound stops a node serving a chain it never synced" (U5 H-U5-07). Shared cause: the same
  `terminal` bound — it both protects and evicts the joiner ([barrier-is-the-threat]).
- **SCP-17.** Threat "dispatch slots pinned by handshake replies → the node stops answering every message
  type" (U8 H-U8-04) ↔ threat "the SSRF dial of the attacker-claimed host in the same call" (U8 H-U8-06).
  Shared cause: the single `transport.send(peer, response)` in `comm/src/rp/handle_messages.rs:handle_protocol_handshake`
  — the 5 s pin is the SSRF dial aimed at a black-hole address ([shared-cause], CH-U8-07).
- **SCP-18.** Threat "one peer drains the global Kademlia rate window" (U8 H-U8-03) ↔ barrier "the global
  `RateLimiter` bounds total request rate" (`shared/src/rate_limiter.rs:RateLimiter`). Shared cause: the
  single window counter is both the admission bound and the exhaustible resource
  ([barrier-is-the-threat]).
- **SCP-19.** Threat "the SSRF check resolves the host before the dial, and the dial re-resolves"
  (U8 H-U8-07) ↔ barrier "`is_local_address_resolved` fail-closed on unresolved names"
  (`comm/src/rp/handle_messages.rs:is_local_address_resolved`). Shared cause: the same name — the guard
  and the dial resolve the same name; a short-TTL answer passes the check and *is* what the dial follows
  ([shared-cause]).
- **SCP-20.** Threat "the escape matures on a local attempt count while the round's clock is frozen
  heights" (U1 H-U1-07) ↔ barrier "the bound `waited <= LIVENESS_WINDOW`"
  (`casper/src/blocks/proposer/proposer.rs:create_block`). Shared cause: the bound counts local attempts
  while the property it substitutes for is measured in heights the deadlock freezes ([self-referential]).
- **SCP-21.** Threat "the stale `bonded` read feeds `check_active_validator` and the attestation quorum"
  (U1 H-U1-04) ↔ barrier "the `bonded` filter in `add_recorded_equivocations` + the receiver re-check
  `equivocation_is_proved`" (`casper/src/blocks/proposer/proposer.rs:add_recorded_equivocations`,
  `casper/src/validate.rs:equivocation_is_proved`). Shared cause: the same `compute_bonds(pre_state_hash)`
  view that C201 shows is frozen (`spec/audit/evidence/c201-proposer-read-results.md`) — **contested**
  (CH-U1-05: C201 is a coupling to frozen finality, and its evidence refutes the stated consequence).
- **SCP-22.** Threat "the node attests at every height (production unbounded)" (U2 H-U2-01) ↔ barrier "the
  cadence gate `cadence_due`" (`casper/src/blocks/proposer/proposer.rs:cadence_due`). Shared cause:
  `cadence_due` is the right operand of the OR the deviation short-circuits — the single event "dummy
  deploy pins `new_state_transition`" both disables the barrier and is the unbounded production
  ([barrier-is-the-threat]).

**Independence findings (non-pairs, recorded so the search is auditable).**

- **H-U3-03's barrier is `[one-surface]`, not `[shared-cause]`** — the filter (`live_justifications`) and
  the surviving deviation (`mv.parents` through the full `msg_map`) are different surfaces
  (`block-storage/src/dag/liveness.rs:168`, "C174 … left this side of it alone"). Recorded as contested
  in SCP-7.
- **H-U4-02's differential is `independent`** — the oracle enumerates the pre-quotient accepted set, a
  genuinely different algorithm (`sdk/src/property_tests.rs:rejection_options_match_a_literal_enumeration`).
- **H-U8-06, H-U9-02, H-U10-04 are `[one-surface]`** — each guards a surface (literal IP class / root
  consistency / one shard) that is not the deviation's surface.
- **U3's whole-map quorum denominator is `[unhoused]` as a barrier** — a denominator required for safety
  is not a barrier interposed against the halt it causes (CH-U3-02).

---

**Provenance (as actually observed).**
`git rev-parse HEAD` → `1e5a64ed4149d5bd7fa15affb1022e3c89f6c492` — **matches**
the stated audit tree. `git status --porcelain` → `?? .claude/` only (untracked worktrees; no tracked
modification). Read (in full): all ten `s1-U*.md` and all ten `s2-U*.md` under
`spec/RUST-VS-SCALA.md` (288-302), `spec/audit/evidence/n148-results.md`,
`spec/audit/evidence/n149-results.md`. Spot-checked at this HEAD: `casper/src/merging.rs`
(`boundary_merge_tests:2739`, `sibling_boundaries_with_different_pots_merge:3073`),
`rholang/src/native_state.rs` (`close_block:1426`, `is_epoch_boundary:746`),
`casper/src/api/block_api.rs` (`note_timer_halted:98`) vs `node/src/runtime/node_runtime.rs:1604` (call
site, not definition — CH-U10-01), `casper/src/protocol/comm_util.rs:send_block_hash:176` (returns `()`
— CH-U1-06). Not read: `spec/AUDIT.md`/`spec/findings.tsv` in full, the per-node evidence directories
under `spec/audit/evidence/*/`, `docs/src/node/{testnet,scaling,validator-requirements,operating,consensus}.md`
in full, `~/.claude/plans/linear-spinning-eich.md` §2 (the TE frame was taken from its headings).
`unmeasured` marks a barrier with no committed run; `[unhoused]` marks a judgement with no artefact behind
it. The S1/S2 sheets are the primary source; where a challenger overturned a row the challenge governs and
the row is marked contested.


### TE-2 — A fragment of the net reaches finality that the rest rejects

*Loss of control of the hazard. Synthesised from `s1-U1…U10` (Table A/B) and `s2-U1…U10`
(Table C / challenges / labels) by the S3 bow-tie owner. Every cell carries one anchor of the form
`path:symbol`, `spec/audit/evidence/<run>-results.md`, `#NN`, or the literal `[unhoused]`; where no
committed artefact measures a barrier the evidence cell says `unmeasured`.*

— **matches** the tree under audit. `git status --porcelain` → `?? .claude/` only (no tracked
modification). Read: all 20 sibling sheets; the study charter `~/.claude/plans/linear-spinning-eich.md`
(§2 top events, §2.4 independence classes); symbol verification by grep over `casper/ block-storage/ sdk/
rholang/ node/` and `spec/Rchain/Casper/Bonds.lean` (read-only). Every `path:symbol` below resolved at
HEAD (`git ls-files` tracks each file; `grep` finds each named symbol). No repository writes.

---

#### Threats.

Credible reasons a *fragment* reaches a finality (a fringe / finalised decision) the rest rejects, one per
TE-2-labelled deviation across the ten study nodes:

- **T1 — Self-equivocation admitted through a stale-snapshot window (U1).** A proposer can build a
  *second* block at the same `(sender, seq_num)` when the DAG advanced between the parent-set read and
  the insert, so a peer holds two signed blocks at one sequence and records a slashable equivocation
  (AUDIT §48). Anchor `casper/src/blocks/proposer/proposer.rs:validate_block`
  (`ValidateError::SelfEquivocation`) · `block-storage/src/dag/message_state.rs:has_advanced_past_the_round`.
  — H-U1-02 → TE-2.
- **T2 — The liveness escape fires on a local attempt count and retires a merely-lagging validator,
  shrinking the partition (U1).** The escape matures on one increment per declined proposal
  (2 s cadence, `AUTOPROPOSE_INTERVAL`) rather than the DAG's `LIVENESS_WINDOW` heights of real silence,
  so the tip moves on *a subset* before the round's own clock agrees the sender is absent. Anchor
  `casper/src/blocks/proposer/proposer.rs:create_block` (the `escape` block) ·
  `block-storage/src/dag/liveness.rs:LIVENESS_WINDOW`. — H-U1-07 → TE-2.
- **T3 — The estimator can publish a fringe *below* its parents' (U3).** The published fringe is
  `new_fringe_opt.unwrap_or(parent_fringe)` and `parent_fringe` is the latest fringe over the **live**
  justifications only; retiring the validator carrying the freshest fringe restarts the derivation from
  an older carried fringe. Anchor `block-storage/src/dag/message_state.rs:create_message` ·
  `block-storage/src/dag/message_map.rs:latest_fringe`; the property this violates is a stated
  **false** axiom (`spec/Rchain/Casper/Fringe.lean:fringe_monotone_is_false`, per s2-U3 CH-U3-05).
  — H-U3-05 → TE-2.
- **T4 — The merge returns a different (cheaper) rejection than a peer (U4).** The directed quotient
  merges two states that share one rejection union on the claim their *futures* are identical; a collapse
  that merged states with different futures drops an option, so a node computes a different merge. The
  merge result is consensus-visible (Law 17). Anchor `sdk/src/dag/merging.rs:enumerate_rejection_sets`
  · `sdk/src/property_tests.rs:law17_the_survivors_of_a_rejection_option_are_conflict_free`.
  — H-U4-02 → TE-2.
- **T5 — A joiner enters on an unapproved fringe or a divergent restored prefix (U5).** Any peer's
  `FinalizedFringe` is processed as if from the bootstrap — the sender check logs and does **not**
  `return` — and a boundary block whose fringe metadata is missing replays its `close_block` to a
  *different* post-state (the measured #139 shape). Anchor
  `casper/src/engine/node_syncing.rs:on_finalized_fringe_message` ·
  `casper/src/engine/node_syncing.rs:populate_dag` · `spec/audit/evidence/n139-mature-join-results.md`.
  — H-U5-05 and H-U5-02 → TE-2.
- **T6 — The epoch draw moves the active set and the carried bonds maps disagree (U6).** `close_block`
  reselects the active set from the pool behind the epoch seed, so the consensus set (`pos:active`, the
  leaf the finality gates read) moves at every boundary even when no stake moves; the justifications'
  carried bonds maps then disagree and the finaliser refuses rather than guessing. Anchor
  `rholang/src/native_state.rs:close_block` · `rholang/src/native_state.rs:select_active` ·
  `spec/Rchain/Casper/Bonds.lean:a_disagreeing_set_is_refused`. — H-U6-02 → TE-2.
- **T7 — An untrusted-but-bonded validator keeps its seat and keeps voting (U6).** `untrust` edits only
  `pos:trusted`; the revoked validator keeps its bond and its place in the pool/active set, so it keeps
  proposing and attesting and its blocks can carry a divergent finality. Anchor
  `rholang/src/native_state.rs:untrust`. — H-U6-06 → TE-2.
- **T8 — A deploy is consumed by an orphan branch the rest rejects (U7).** A deploy leaves the pool at
  block *insert*, not at finalisation, and `select_deploys` then excludes it as a replay; a block that is
  later orphaned neither holds nor finalises the deploy, so the accepted work divides. Anchor
  `casper/src/dag.rs:BlockDagKeyValueStorage::insert` ·
  `casper/src/blocks/proposer/proposer.rs:select_deploys`. — H-U7-04 → TE-2.

---

#### Preventive barriers.

What stands today between each threat and the top event. Evidence is a committed artefact, a symbol read,
or `unmeasured`.

| Threat | Preventive barrier (anchor) | Evidence | Class |
|---|---|---|---|
| T1 | proposer veto `AlreadyProposedThisRound` in `casper/src/blocks/proposer/proposer.rs:create_block`; receiver refusal `casper/src/validate.rs:sequence_number` (`creator_latest_seq + 1 != seq_num`) | `unmeasured` — no committed run drives the stale-snapshot window; s1-U1's own cell is `unmeasured` | `[one-surface]` — only the receiver's number check is cross-node; the proposer veto reads the same snapshot that can go stale (s2-U1 CH-U1-08) |
| T2 | the bound `waited <= ….LIVENESS_WINDOW` in `casper/src/blocks/proposer/proposer.rs:create_block` | `unmeasured` for the escape itself — `spec/audit/evidence/n148-results.md` contains **no** occurrence of the escape or `blocked_since_advance` (s2-U1 CH-U1-04); the cited artefact cannot see it | `[self-referential]` — the bound counts local attempts while the property it stands in for is measured in heights the deadlock freezes |
| T3 | the walk cutoff `block-storage/src/dag/finalizer.rs:self_parents` (lets a minimum message move only along its own unfinalised chain) | `unmeasured` — no run drives an older-fringe flip | `[unhoused]` — bounds the walk, not its starting point (s2-U3) |
| T4 | the exactness differential against a literal enumeration: `sdk/src/property_tests.rs:rejection_options_match_a_literal_enumeration` (symmetric **and** arbitrary directed/self-conflicting maps) | property test committed; but the row's *budget* barrier cites `sdk/tests/merging_scaling.rs:rejection_options_are_bounded_on_a_directed_shape`, which runs `SearchBudget::UNBOUNDED` and cannot witness it (s2-U4 CH-U4-02) | contested: s1 labels `[shared-cause]` (the oracle is a transcription of the same algorithm), s2-U4 CH-U4-02 argues the oracle is the **pre-quotient accepted-set** algorithm, a different one — label disputed |
| T5 | the request asks for the metadata and the responder attaches it: `casper/src/protocol/comm_util.rs:request_finalized_fringe` · `casper/src/engine/node_running.rs:collect_fringe_ancestry`; `casper/src/engine/node_syncing.rs:populate_dag` names its per-block fallback rather than acting silently | `spec/audit/evidence/n139-mature-join-results.md` (post-fix 4/5 green, zero state-hash disagreements) — but this exercises the **complete**-metadata arm only; the partial arm is `unmeasured` (s2-U5 CH-U5-05) | `independent` — real for the complete-metadata arm; the sender-check half has **no** gate (a log line), so T5's fringe-seizure half is unbarriered |
| T6 | the finaliser's fallback **refusing** two disagreeing carried bonds maps rather than picking one: `spec/Rchain/Casper/Bonds.lean:a_disagreeing_set_is_refused` (caller `casper/src/multi_parent_casper.rs` `compute_bonds`) | `spec/Rchain/Casper/Bonds.lean` (model witness, read); the live residual is `spec/RUST-VS-SCALA.md` — **unmeasured-live** | `[shared-cause]` — the same `compute_bonds` agreement requirement that is the barrier here is the mechanism that halts the chain in H-U6-04 (s2-U6 CH-U6-04) |
| T7 | the design decision that revocation is a governance act, not confiscation — an untrusted-but-bonded validator keeps its stake: `rholang/src/native_state.rs:untrust` | symbol read; the register account is `spec/audit/passes.md:4064` (the `AUDIT.md` the s1 cell named has no C111 row — s2-U6 CH-U6-01) | `[barrier-is-the-threat]` — the property that makes revocation safe (the bond is not the operator's to take) is exactly what makes it ineffective as removal |
| T8 | `casper/src/validate.rs:repeat_deploy` + `deploy_index` records the inclusion | symbol read | `[one-surface]` — prevents double-*inclusion*, not the pool loss the deviation names (s2-U7 CH-U7-03, its own cell concedes this) |

---

#### Top event.
A fragment of the net reaches finality that the rest rejects

---

#### Consequences.

Once control is lost:

- **Two fragment sets hold different finalized fringes at the same height, and a joiner cannot index its
  own restored chain** — the measured #139 shape (`spec/audit/evidence/n139-mature-join-results.md`:
  bootstrap 127/123, joiner stops 27/23).
- **An equivocating validator's two blocks both exist, and a fragment may finalise one of them**
  (`casper/src/validate.rs:equivocation_is_proved` records the fault, but the fork it created is not
  repaired) — H-U1-02.
- **The finaliser's bonds fallback refuses across the net and finality stalls** — the same mechanism
  reaches **TE-1** as well as TE-2, which s2-U6 CH-U6-04 flags as a TE-class collision: a refusal
  produces no divergent state, so one of H-U6-02/H-U6-04 must change class.
- **A bond/withdraw riding the rejected sibling takes effect on no node or on only one fragment** —
  H-U6-01, H-U6-03 (`spec/audit/passes.md` §61 measures the `delegate` in block 206 discarded;
  `spec/audit/evidence/c207-merge-loses-native-writes.md`).
- **A deploy already removed from the pool at insert never finalises for the rest** — H-U7-04.
- **Irreversible by construction: there is no finality undo** — the accepted residue of the port
  (charter §"Absent": "finality undo" is a deferral). A divergent finality cannot be rewound by any
  mechanism in the tree.

---

#### Mitigating barriers.

What reduces severity *after* the top event:

- **The finaliser refuses rather than guesses** — a divergent bonds map cannot be silently adopted; the
  fallback demands agreement (`spec/Rchain/Casper/Bonds.lean:a_disagreeing_set_is_refused`). This converts
  a silent split into a visible stall.
- **Every node re-derives an equivocation from its own DAG** —
  `casper/src/validate.rs:equivocation_is_proved`; the proposer folds the recorded equivocation into its
  slash set (`casper/src/blocks/proposer/proposer.rs:add_recorded_equivocations`), and per C200 the
  evidence travels, so the surviving chain converges on the culprit
  (`spec/audit/passes.md` §54 "Equivocation is slashable, and the evidence travels").
- **An unjustified slash cannot licence work** — `casper/src/interpreter_util.rs:slash_is_unjustified`
  re-derives the slash set and tier from this node's own DAG, so a fragment cannot drive a slash the rest
  would reject.
- **The exactness differential detects a quotient that drops an option before it ships** —
  `sdk/src/property_tests.rs:rejection_options_match_a_literal_enumeration`; the property is committed and
  passes on both symmetric and arbitrary directed maps.
- **Recovery from the divergent finality itself: an operator tool, and a stopgap — not the node.**
  *Corrected 2026-10-10 (§98; this cell read "none" until then, and that was true when it was written).*
  `tools/reconcile-network.sh` now brings a broken network back to **one chain without a genesis**: it
  attests each node's finalised block, verifies every block it touches from the preimage of its hash,
  computes the deepest block a strict supermajority of stake has finalised as the point to rewind to,
  enumerates what is above it per unique block (with the signatures of the deploys the merges rejected),
  and asserts converging block hashes *and* finality past the point or exits 9. Its operator path is
  exercised on a four-validator net whose finality is frozen by arithmetic
  (`spec/audit/evidence/n-anchor-drill/frozen-finality-recovery.txt`) and its four clauses on a staged
  divergence (`spec/audit/evidence/n-reconcile-drill/run-4-restore.txt`); the live-testnet venue is
  **owed** and named (this box holds no root key, and the live build predates the routes the meet needs).
  What it does *not* do is re-home a fragment: where nothing is finalised there is nothing to compute a
  point from, and the store-level restore (`--restore-from-master`) adopts the survivor's state wholesale
  — an operator asserting a winner, labelled as such and printed before it runs. **The node still has no
  inverse**: a running node that diverges cannot reset itself (F-U5-01, owed as #320), which is what makes
  an operator necessary rather than merely convenient. The operator restarting onto a chain it never
  synced (H-U5-04) is still not recovery but a second entry into T5.

---

#### Degradation factors.

Conditions that weaken a barrier — each with its own control, or `none`:

- **Client diversity is structurally absent (one client only)** — a shared misreading of the merge (the
  oracle is reachable from the same algorithm) passes every node identically. Anchor: charter §"Absent"
  ("client diversity — one client only, structurally absent"); s2-U4 CH-U4-02. **Control: none.**
- **The frozen tip (the #213 wedge) is the ambient condition that disables the reconciliation barriers.**
  Anchor `node/src/runtime/node_runtime.rs:attest_warranted` (the per-sender tap seal) ·
  `block-storage/src/dag/message_state.rs:has_advanced_past_the_round`. Control: the bounded escape
  `block-storage/src/dag/message_state.rs:parents_for_new_block_escaping` — which is `[self-referential]`.
- **The operator surface does not report the halt/divergence** — the wire `Status` has no health field
  (`models/src/casper/protocol/deploy_service.rs:Status`), so a fragment/diverge reads as healthy through
  the CLI/gRPC path an operator scripts. Control: the HTTP path folds `proposer_health()` in (`/api/v1/status/operator`)
  (`node/src/api/admin_web_api_impl.rs:operator_status`) and the per-shard `/metrics` gauge
  (`node/src/runtime/node_runtime.rs:push_proposer_health`) — but non-primary shard coverage is structural,
  not witnessed (s2-U10 CH-U10-07).
- **A permanent ≥ 1/3 loss shrinks the partition while the quorum denominator is the whole bonded map** —
  `block-storage/src/dag/finalizer.rs:calculate_fringe` (sums `total_stake` over `quorum_bonds`) versus
  `block-storage/src/dag/liveness.rs:live_weight_set`. Control: **none** (a deliberate stop; no inactivity
  leak — `spec/audit/evidence/n148-results.md`).
- **The tap seal removes exactly the proposal requests the escape needs to fire** —
  `node/src/runtime/node_runtime.rs:attest_warranted`. Control: the per-sender key fix
  `e6d41a347` (C192) — but **no committed run post-dates the fix** for this node (s2-U1 CH-U1-01).
- **The devnet launch config (`--autopropose` + a dummy deploy) widens production and the equivocation
  surface** — `node/src/runtime/node_runtime.rs:dummy_deploy_key`. Control: the same gate — which is
  `[shared-cause]` with the trigger, because the flag *is* the deviation's cause (s2-U1 CH-U1-07).
- **A joiner's sync target can be set by a non-bootstrap peer before the bootstrap answers** —
  `casper/src/engine/node_syncing.rs:on_finalized_fringe_message`. Control: the retry re-reads the target
  every attempt (`casper/src/engine/node_syncing.rs` retry task), so at most one of three attempts is
  wasted (s2-U5 CH-U5-02) — but the sender check itself remains no gate.
- **The epoch-length / active-validator cap decides whether the draw bites at all** — on a net whose pool
  is within the cap, `select_active` returns the pool unchanged, so T6 is unreachable until the cap bites
  (`rholang/src/native_state.rs:select_active`; s2-U6 CH-U6-03). Control: **none** — a configuration
  precondition, not a barrier.

---

#### Shared-cause pairs and independence findings.

Each names **both members** and the **shared cause** — the event that triggers the threat also disables
(or is) the barrier.

- **SC-1 (canonical).** *Members:* threat **T6** (carried bonds maps disagree → a node's bonds view
  diverges → TE-2; H-U6-02/H-U6-04) and barrier **the `compute_bonds` carried-map agreement requirement**
  (`spec/Rchain/Casper/Bonds.lean:a_disagreeing_set_is_refused`; caller
  `casper/src/multi_parent_casper.rs`). *Shared cause:* **a bond change takes effect only when blocks carry
  a reconciled bonds map, and the wedge (frozen finality) is the very event that stops blocks carrying it**
  — "no merge happens" both disables the barrier and is the divergence it guards. This is the charter's own
  §2.4 exemplar; H-U6-02's Table-B cell and s2-U6 CH-U6-04 both land on it.
- **SC-2.** *Members:* threat **T2** (escape retires a lagging validator → a fragment advances on a smaller
  partition; H-U1-07/H-U2-03) and barrier **`block-storage/src/dag/liveness.rs:live_weight_set`** (the
  liveness rule that retires an absent sender so the round can still close) together with the round's
  window retirement `block-storage/src/dag/message_state.rs:advance_round`. *Shared cause:* **the frozen
  tip** — the same tip that blocks the fringe also freezes the clock the retirement is measured from.
  (s1-U1 H-U1-03 `[shared-cause]`.) This is also a **two-barrier** pair: the escape and the window
  retirement both measure against one frozen tip.
- **SC-3.** *Members:* threat **the fringe does not advance / the DAG rests at one height** (H-U3-01 →
  TE-1, feeding T3) and barrier **`block-storage/src/dag/message_state.rs:parents_for_new_block`** (the
  cross-sender parent set the proposer justifies). *Shared cause:* **whether any new block/layer exists is
  the round/tap's own decision** — one event (no fresh round) is both the gate's missing input and the
  loss the gate would detect. (s1-U3 H-U3-01 `[shared-cause]`, spanning U1↔U2↔U3.)
- **SC-4.** *Members:* threat **T1** (a second block at one `(sender, seq_num)`; H-U1-02) and barrier
  **`casper/src/blocks/proposer/proposer.rs:create_block`'s `AlreadyProposedThisRound` veto**. *Shared
  cause:* **the DAG advancing between the parent-set read and the insert** — the same window both opens the
  deviation and is what the veto fails to observe, because the veto reads the same pre-advance snapshot.
  Cross-node half: only the receiver's `casper/src/validate.rs:sequence_number` is cross-node.
- **SC-5.** *Members:* threat **the tap never re-fires** (H-U1-01) and barrier **the liveness escape in
  `casper/src/blocks/proposer/proposer.rs:create_block`** (which increments `blocked_since_advance` only on
  a proposal *request*). *Shared cause:* **the same "no proposal request" event** — the tap seal removes
  exactly the request the escape needs, so the barrier cannot fire. Cross-node U1↔U2. (s1-U1 H-U1-01
  `[self-referential]`.)
- **SC-6.** *Members:* threat **the `bonded` re-slash read** (H-U1-04; a stale `compute_bonds(pre_state_hash)`
  also feeds `check_active_validator` and the attestation quorum) and barrier **the `bonded` filter in
  `casper/src/blocks/proposer/proposer.rs:add_recorded_equivocations` + the receiver re-check
  `casper/src/validate.rs:equivocation_is_proved`**. *Shared cause:* **both gate on the same frozen
  `compute_bonds(pre_state_hash)` view** — C201's own resolution is that this is "a coupling to frozen
  finality, not a defect" (`spec/audit/evidence/c201-proposer-read-results.md`). Cross-node: the frozen
  finality is the U1↔U2↔U3 wedge, so SC-6 links to SC-1 and SC-2 by their common cause.
- **SC-7.** *Members:* threat **H-U6-03** (boundary siblings hold different pots; a deploy riding the
  loser is discarded) and barrier **the merge's whole-block rejection**
  (`casper/src/merging.rs:MergeScope::fold` / `reject_whole_blocks`). *Shared cause:* **one whole-block
  rejection is both the guard against a conflicting native write (H-U6-01) and the harm (the discarded
  deploy)** — the two members are the same event. This is `[barrier-is-the-threat]` rather than
  `[shared-cause]`, and s2-U6 CH-U6-08 corrects s1's cross-row `[shared-cause]` label to it.
- **SC-8.** *Members:* threat **the deploy pool never drains** (H-U7-06) and barrier
  **`casper/src/dag.rs:expire_deploys`** (runs on every accepted block insert). *Shared cause:* **an
  accepted block insert** is what runs the barrier, and its absence (the #213 wedge) is what withholds it —
  the same event the barrier needs is what its failure withholds. Cross-node U7↔U1/U2/U3. (s1-U7 H-U7-06
  `[shared-cause]`.)
- **SC-9.** *Members:* threat **T5's fringe-seizure half** (a non-bootstrap peer sets the sync target;
  H-U5-05) and barrier **the sender check `casper/src/engine/node_syncing.rs:on_finalized_fringe_message`**
  (which logs "ignored" and then proceeds, with no `return`). *Shared cause:* **the same inbound
  `FinalizedFringe` message** both drives the sync and is what the barrier is supposed to filter — the
  barrier's only evidence is the comment describing a gate the code does not implement.
  `[self-referential]` (s2-U5).
- **SC-10.** *Members:* the two **barriers** — the escape's `waited <= …LIVENESS_WINDOW` bound
  (`casper/src/blocks/proposer/proposer.rs:create_block`) and the round's `tip - round_height >
  LIVENESS_WINDOW` retirement (`block-storage/src/dag/message_state.rs:advance_round`). *Shared cause:* both
  are measured against **the one frozen tip** the deadlock pins, so a single event disables both at once.
  Cross-node U1↔U2↔U3 (two barriers, not a threat/barrier pair).
- **SC-11 (independence finding — one mechanism, two top events).** *Members:* **H-U6-02** (the refusal →
  **TE-2**) and **H-U6-04** (the identical refusal → **TE-1**). *Shared cause:* the `compute_bonds`
  agreement requirement. The pages' own TE frame uses TE-1 for stalls and TE-2 for divergence, and a
  *refusal* produces no divergent state — one of the two rows must change class (s2-U6 CH-U6-04). This is
  the clearest place where the TE-2 boundary is drawn wrongly on a source sheet.
- **SC-12 (independence finding — the oracle).** *Members:* threat **T4** (a different merge than peers)
  and barrier **the exactness differential `sdk/src/property_tests.rs:rejection_options_match_a_literal_enumeration`**.
  *Alleged shared cause:* the oracle is a transcription of the shipped algorithm, so a shared misreading
  passes both (s1-U4 `[shared-cause]`). s2-U4 CH-U4-02 disputes this — the oracle is the pre-quotient
  **accepted-set** algorithm, a genuinely different one — so the pair is **contested**, not confirmed.
- **SC-13 (independence finding — the barrier is the consequence).** *Members:* threat **H-U4-01** (a merge
  that exceeds the search budget returns no answer and the block is dropped) and barrier **the budget
  applied before each unit of work** (`sdk/src/dag/merging.rs:SearchBudget::NODE`). *Shared cause:* the
  budget's refusal *is* the row's stated consequence (`SearchBudgetExceeded` → `ValidateError::Internal` →
  drop); one event is both barrier and harm — `[barrier-is-the-threat]`, which s2-U4 CH-U4-05 corrects
  from s1's `[self-referential]` (and the row's barrier evidence in fact runs `SearchBudget::UNBOUNDED`
  and cannot witness it — CH-U4-02).

**Cross-cutting observation.** SC-1, SC-2, SC-6, SC-8 and SC-10 all cluster on **one ambient cause: the
frozen tip / no-merge wedge (#213)**. Because that single condition disables the finality *and* the
reconciliation barriers simultaneously, TE-2 and TE-1 are not independent failure modes on this net — the
wedge is a common-mode cause of both, which is why the charter gives TE-2 its own promotion trigger
("any claim of safety, or the first contract holding value") rather than letting criterion 2's liveness
verdict cover it.


### TE-3 — An outsider validator cannot safely join or leave


**Scope note.** TE-3 is the charter's membership/liveness top event, and it is the one #214 criterion 2 measures. **As first measured** (before the 2026-10-04 fixes) all three failed or were unrun: (a) kill one of three `FAILS` (`#213`), (b) the killed validator rejoins `FAILS`, (c) a new validator bonds onto a running net `never run`. **The current readings are in §3.2** — this worksheet was the first pass — corrected 2026-10-04 (§5.1 item 3); where a verdict below has moved, §3.2 and §3.4 own it. Threats are grouped join-path / leave-path / network-cannot-accept / observation. Where a threat lands on TE-1 or TE-2 as well, that is named in the consequence — a wedged chain cannot accept a member, and a join that replays to a divergent state is TE-2's input.

---

**Threats.** (credible reasons loss of control occurs, one per study node; the sheet ID in brackets)

*Join path — a newcomer cannot complete bootstrap/sync:*
- The pre-fringe wait is unbounded: no `FinalizedFringe` is ever handled, so no sync attempt starts and nothing errors — the node latches in `NodeSyncing` with a serving API and no error line (`casper/src/engine/node_syncing.rs:on_finalized_fringe_message`; `casper/src/engine/node_launch.rs:apply` — the `select!` over `handle_loop`/`finished`/`terminal` has no completion without a fringe). [H-U5-01]
- The tuple-space leg never gives up: the attempt's `join!` waits for both legs and only the block leg has a pace bound (`casper/src/engine/lfs_tuple_space_requester.rs:request_tuple_space_roots` carries no `MAX_IDLE_ROUNDS`, against `casper/src/engine/lfs_block_requester.rs:MAX_IDLE_ROUNDS`). [H-U5-03]
- The retry budget evicts the joiner: `MAX_SYNC_ATTEMPTS` (3) stops the sync on a modest bootstrap outage (`casper/src/engine/node_syncing.rs:MAX_SYNC_ATTEMPTS`; per CH-U5-01 it only logs, it does not exit). [H-U5-07]
- A crash mid-`populate_dag` restarts into Running over a strict prefix: the mode is chosen from DAG emptiness, not the approved fringe, and the approved store has no production reader (`casper/src/engine/node_launch.rs:apply`; `block-storage/src/syntax.rs:get_approved_block` is called only from its own test). [H-U5-04]
- A non-bootstrap peer's fringe is processed as if from the bootstrap: the sender check is a log line, not a gate (`casper/src/engine/node_syncing.rs:on_finalized_fringe_message` — `if !sender_is_bootstrap { log }` with no `return`). [H-U5-05]
- Partial fringe metadata yields an empty fringe: the responder skips any block whose metadata it lacks and the joiner falls back to `BlockMetadata::from_block`, so a boundary block replays to a different post-state and the joiner cannot index its own restored chain (`casper/src/engine/node_running.rs:collect_fringe_ancestry`; `casper/src/engine/node_syncing.rs:populate_dag`) — the measured #139 shape. [H-U5-02]
- The join cost is O(chain length): the pre/post trie root of every downloaded block on top of the fringe root is hydrated (`casper/src/engine/node_syncing.rs:run_approved_state_sync`'s `collect_block_state_roots`), so "a fresh validator does not join in any useful sense". [H-U5-06]
- Start-up replay is the floor: a node that exhausts memory during replay never opens its API and loops on restart (`casper/src/dag.rs:BlockDagKeyValueStorage::create`; `docs/src/node/validator-requirements.md:ram-again-start-up-replay-is-the-floor`). [H-U9-04]
- Inbound admission is denied before authentication: `accept_tls` `continue`s past a TCP connection when no handshake slot is free, so an honest peer is dropped pre-auth (`comm/src/transport/grpc_transport_receiver.rs:accept_tls`). [H-U8-09]
- Peer discovery is a shared global budget: one peer spends the 100/s Kademlia window, so a joining node cannot discover peers (`comm/src/discovery/grpc_kademlia_rpc_server.rs:DEFAULT_KADEMLIA_RATE_LIMIT_PER_SEC`). [H-U8-03]
- The funding drip is burned: the faucet increments the per-address drip before `block_api.deploy` and a failed submit does not roll it back (`node/src/api/web_api_impl.rs:faucet`). [H-U7-07]

*Leave path — a bonded validator cannot exit cleanly:*
- A concurrent set-shaped PoS write is rejected whole: `delegate`/`trust`/`withdraw` write set leaves (`pos:delegations`/`pos:trusted`/`pos:pending_withdrawers`) and the merge rejects one whole block, so the deploy's write takes effect on no node (`rholang/src/native_state.rs:delegate`/`:trust`/`:withdraw`; residual of `spec/audit/evidence/c207-merge-loses-native-writes.md` §5). [H-U6-01]
- A withdraw coalesced into the losing boundary sibling is discarded: every proposer's block carries its own non-idempotent `close_block`, the merge rejects all but one, and a deploy riding the loser is lost (`rholang/src/native_state.rs:close_block`; `casper/src/merging.rs:sibling_boundaries_with_different_pots_merge`). [H-U6-03]
- A withdrawal takes effect only when a later block crosses a boundary: on an idle chain the request looks ignored, and the refund waits past its deadline (`rholang/src/native_state.rs:close_block`; `:is_epoch_boundary`; `docs/src/node/testnet.md:619-624`). [H-U6-05]
- The withdrawal countdown is misreported (~50,000 blocks high) because the stored deadline is labelled `stagedAtBlock` and the quarantine is added a second time (`node/src/web/pos_read.rs`; C206, since fixed). [H-U10-02]

*The network cannot accept or retain a member:*
- A newly bonded validator stalls round closure: `advance_round`'s never-spoken arm waits on `tip − round_height > LIVENESS_WINDOW`, a clock the round veto freezes (`block-storage/src/dag/message_state.rs:advance_round`, doc at `:76-84` — "waiting for a sender that has not spoken yet is what makes a round a round"). [H-U2-04]
- A permanent ≥1/3 loss makes the quorum unreachable: the denominator is the whole bonded map and no inactivity leak exists, so a rejoining validator is *required*, not optional (`block-storage/src/dag/liveness.rs:live_weight_set`, doc at `:18-23`). [H-U3-02]
- A bond-set change before the chain's first finalisation can wedge the chain when the justifications' carried maps disagree and the newest state is unreadable (`spec/Rchain/Casper/Bonds.lean:a_disagreeing_set_is_refused`; the `casper/src/multi_parent_casper.rs` fallback). [H-U6-04]
- A revoked validator is not removed: `untrust` edits only `pos:trusted`, so the peer keeps its bond, its place in the active set, and keeps proposing (`rholang/src/native_state.rs:untrust`). [H-U6-06]

*Observation — the escalation factor carried by all four top events:*
- A rejoining/restarting validator looks stuck while it replays: the API is absent for the whole replay and the unit still reports `active` (`docs/src/node/validator-observer-setup.md` §9 (restart replay)). [H-U10-07]

**Preventive barriers.** (what stands between each threat and the top event; anchor + evidence)

- [vs threat T6] The metadata request and the responder's attach: `casper/src/protocol/comm_util.rs:request_finalized_fringe` sets `include_fringe_metadata=true` and `casper/src/engine/node_running.rs:collect_fringe_ancestry` attaches ancestry — Evidence: `spec/audit/evidence/n139-mature-join-results.md` (post-fix 4/5 attempts green, zero state-hash disagreements; but the complete-metadata arm only — CH-U5-05). Independence `independent` (complete arm only).
- [vs T4] C68 sequencing keeps a failed attempt out of Running and does not record the approved fringe on failure: `casper/src/engine/node_syncing.rs:notify_when_restored` — Evidence: symbol read; the crash-mid-`populate_dag` path is `unmeasured`.
- [vs T3] The `terminal` bound itself: `casper/src/engine/node_syncing.rs:MAX_SYNC_ATTEMPTS` — Evidence: symbol read; no run measures joiner survival across a transient outage — `unmeasured`.
- [vs T2] The block leg's pace bound `casper/src/engine/lfs_block_requester.rs:MAX_IDLE_ROUNDS` (3 idle rounds); it does not cover the tuple leg — Evidence: symbol read (the asymmetry is in `request_tuple_space_roots`); the tuple-leg hang is `unmeasured`.
- [vs T1] **None effective**: no timeout wraps `apply`'s wait for the fringe itself (`casper/src/engine/node_launch.rs:apply`) — Evidence: `unmeasured`. Independence `[shared-cause]` with T2.
- [vs T5] **None**: the sender check is a log line, not a gate (`casper/src/engine/node_syncing.rs:on_finalized_fringe_message`) — Evidence: symbol read. Independence `[self-referential]`.
- [vs T7] The responder's page caps `casper/src/engine/node_running.rs:MAX_STORE_ITEMS_TAKE`/`MAX_STORE_ITEMS_BYTES` bound one page, not the number of roots the requester asks for — Evidence: `unmeasured`. Independence `[one-surface]`.
- [vs T12] Cost accounting left the block's native sidecar and merges per accepted deploy, so the universal `pos:vault` conflict is gone: `casper/src/merging.rs:a_cost_accounted_block_keeps_its_own_native_writes_through_the_merge` — Evidence: `spec/audit/evidence/c207-merge-loses-native-writes.md` §5–§6. Composes **sums** only; the set-leaf residual is unmeasured. Independence `[one-surface]`.
- [vs T13] The merge rejects the losing sibling boundary block whole: `casper/src/merging.rs:sibling_boundaries_with_different_pots_merge` — Evidence: `spec/audit/passes.md` §61 (measured: a `delegate` in block 206 discarded). Independence `[barrier-is-the-threat]` (the same whole-block rejection is the harm).
- [vs T14] `close_block` fires once per produced block and returns before any write off a boundary: `rholang/src/native_state.rs:is_epoch_boundary` — Evidence: `docs/src/node/testnet.md:619-623` (a reading on a live chain, not a committed run). Independence `[unhoused]`.
- [vs T18] The documented constraint to keep the fringe non-empty: `spec/Rchain/Casper/Bonds.lean` (constraint near the file head) + `docs/src/node/testnet.md:624` — Evidence: `docs/src/node/testnet.md:619-628` (re-verified 2026-09-26 with a non-empty fringe; the empty-fringe wedge itself is `unmeasured`). Independence `[self-referential]`.
- [vs T11] The per-address drip budget and own-address guard: `node/src/api/web_api_impl.rs:faucet` (`FAUCET_MAX_DRIPS_PER_ADDRESS = 10`) — Evidence: symbol read. Independence `[barrier-is-the-threat]`.
- [vs T10] `shared/src/rate_limiter.rs:RateLimiter` bounds total request rate (fixed 1 s window) — Evidence: committed unit test `admits_exactly_max_per_window_then_refuses`. Independence `[barrier-is-the-threat]`.
- [vs T9] `comm/src/transport/grpc_transport_receiver.rs:accept_tls`'s `MAX_CONCURRENT_HANDSHAKES` (128) semaphore + `HANDSHAKE_TIMEOUT` (10 s) frees stalled slots — Evidence: symbol read; a per-source admission or slot-fairness rule is `unmeasured`. Independence `[barrier-is-the-threat]`.
- [vs T8] The exporter/importer moves the trie between nodes (`node/src/runtime/node_runtime.rs:create_rspace_importer`; `rspace/src/history/export.rs:sequentialExport`) — it shifts the sync cost but does not avoid the start-up DAG rebuild — Evidence: `docs/src/node/validator-requirements.md:ram-again-start-up-replay-is-the-floor`. Independence `[one-surface]`.
- [vs T16] The same window retirement `tip − round_height > LIVENESS_WINDOW` closes the boundary once the joiner is retired: `block-storage/src/dag/message_state.rs:advance_round` — Evidence: `spec/audit/evidence/n220-join-results.md` (a validator is bonded onto a running net), with the **silent** joiner still unmeasured. Independence `[self-referential]`.
- [vs T17] **None**: a permanent >1/3 loss is a deliberate stop; the partition shrinks but the denominator cannot (no inactivity leak) — Evidence: `spec/audit/evidence/n148-results.md` (killing a validator freezes finality, 3/3). Independence `[unhoused]`.
- [vs T15] The pinned arithmetic test `node/src/web/pos_read.rs:a_pending_withdrawal_counts_down_to_the_deadline_the_store_holds` — Evidence: test present (not run); C206 `done`. Independence `independent`.
- [vs T20] **None** on the node itself; the operator's external snapshot carries `api_reachable` but ignores the halt fields — Evidence: `docs/src/node/validator-observer-setup.md` §8-§9. Independence `[self-referential]`.

**Top event.** An outsider validator cannot safely join or leave.

**Consequences.**
- The newcomer never completes bootstrap — latched in `NodeSyncing` with a serving API and no error line (T1), or stopped after 3 attempts (T3): the validator slot stays empty.
- A joiner replays to state hashes that disagree and cannot index its own restored chain (T6); if it then proposes on the divergent state → **TE-2** (a fragment the rest rejects). Anchored `spec/audit/evidence/n139-mature-join-results.md`.
- A node restarts into Running over a strict prefix and serves a latest-fringe it never derived (T4).
- A bonded validator cannot leave: its withdraw is discarded with the rejected boundary block (T13) or a concurrent set-leaf write takes effect on no node (T12); its stake stays locked until some later block crosses a boundary (T14); the operator mis-times the boundary (T15).
- The chain halts and cannot recover — the empty-fringe bond wedge (T18) → **TE-1**.
- Finality cannot resume because the whole-map denominator requires the lost validator back (T17): the network *needs* a rejoiner it cannot onboard — the join and leave failures compose.
- A round cannot close on a newly bonded validator (T16).
- A revoked-but-bonded peer keeps voting (T19).
- The operator cannot tell "joining" from "wedged" (T20): a merely-replaying validator may be killed.

**Mitigating / recovery barriers.** (reduce severity after the top event)

- The C181 `terminal` bound stops a node serving a chain it never synced — `casper/src/engine/node_syncing.rs:MAX_SYNC_ATTEMPTS`; but per CH-U5-01 it only logs, it does not exit, so the API stays up. Evidence: symbol read.
- The #139 fix carries the fringe on the sync — Evidence: `spec/audit/evidence/n139-mature-join-results.md` (post-fix, zero disagreements on the complete-metadata arm).
- The withdrawal's quarantine deadline eventually pays at the first boundary past quarantine — `rholang/src/native_state.rs:withdraw` (Law 47, `spec/LAWS.md`). Evidence: Law 47 row.
- The liveness rule retires an absent sender so a round can still close — `block-storage/src/dag/liveness.rs:live_weight_set`. Evidence: `spec/audit/evidence/n148-results.md`.
- The liveness escape (after `LIVENESS_WINDOW` declined attempts) is meant to break a round seal by moving the tip — `block-storage/src/dag/message_state.rs:parents_for_new_block_escaping`. Evidence: `spec/audit/evidence/n148-results.md` (frozen at the kill, 3/3).
- The operator can restart or resync; `reset` against a live bootstrap is the #139 recovery shape — Evidence: `spec/audit/evidence/n139-mature-join-results.md`.
- The faucet's per-address budget bounds the drip loss (10 drips/address) — `node/src/api/web_api_impl.rs:faucet`. Evidence: symbol read.

**Degradation factors.** (conditions that weaken a barrier; each with its own control or "none")

- `--autopropose` + a dev key pins `new_state_transition` true (a dummy `Nil` deploy), so attestation suppression never fires and production is unbounded — `node/src/runtime/node_runtime.rs:dummy_deploy_key`. Control: the same flag confines it to a devnet (`[shared-cause]` — the flag is both the deviation and its only gate). Anchor: `s2-U1.md` CH-U1-07, `s2-U2.md` CH-U2-02.
- Frozen finality: every clock a join/leave barrier reads (the round window, the escape, the deploy-expiry clock, the C201 `bonded` read) is measured from a tip the deadlock freezes — `block-storage/src/dag/message_state.rs:has_advanced_past_the_round`. Control: **none** (`[self-referential]`).
- The DAG's Θ(N²) `seen` residency and the replay peak — `block-storage/src/dag/representation.rs:seen_entries`. Control: none preventive; detection gauges only (`casper/src/dag.rs:set_gauges`).
- A saturated shared admission/pool budget denies honest newcomers — `node/src/api/grpc/mod.rs:serve_deploy` builds the limiter. Control: the global pool cap (`MAX_POOLED_DEPLOYS`); **no per-sender quota**.
- The operator surface reads the primary shard only on a gateway and carries no halt field — `node/src/api/shard_routing.rs:proposer_health`. Control: the per-shard `/metrics` gauge (structural, not witnessed — CH-U10-07).
- The join/sync observation is absent for the whole replay — `docs/src/node/validator-observer-setup.md` §9. Control: **none** on the node.

---

**Shared-cause pairs and independence findings.** (a pair is a threat and a barrier — or two barriers — that share a cause, so the event that triggers the threat also disables the barrier; each names both members and the shared cause)

- **P1 (the canonical instance for TE-3).** *Leave-threat* [T13/T14, H-U6-03/H-U6-05: a withdraw takes effect only on a merged block that crosses an epoch boundary] ↔ *Barrier* [`rholang/src/native_state.rs:close_block` + `:is_epoch_boundary`, which apply the leave]. Shared cause: the bound/frozen tip (`block-storage/src/dag/message_state.rs:has_advanced_past_the_round`) that stops merges and boundary production. The event that stops the leave from applying (no merge finishes) is exactly the consequence the leave path exists to escape — a validator cannot leave a chain that has stopped merging, and the stopped merge is why it wants to leave.
- **P2.** *Barrier* [B8, `casper/src/merging.rs:a_cost_accounted_block_keeps_its_own_native_writes_through_the_merge` — the merge's whole-block rejection for the `pos:vault` sum] ↔ *Barrier/harm* [B9, `casper/src/merging.rs:sibling_boundaries_with_different_pots_merge` — the same whole-block rejection for a sibling boundary]. Shared cause: `casper/src/merging.rs:reject_whole_blocks`. The one mechanism is the barrier for H-U6-01 and the threat's harm for H-U6-03 — one whole-block rejection is a guard in one row and the loss in the other.
- **P3.** *Threat* [T12: a concurrent set-leaf PoS write is lost] ↔ *Barrier* [B8: cost accounting composes per accepted deploy]. Shared cause: native state merges whole-slot **snapshots**, not transitions (`spec/audit/evidence/c207-merge-loses-native-writes.md` §5). A set has no composition, so the event that triggers the threat — two concurrent writers of one set leaf — is the one case the composing barrier cannot cover.
- **P4.** *Threat* [T18: a bond change before first finalisation wedges] ↔ *Threat/Barrier* [T17's barrier and H-U6-02's barrier, `spec/Rchain/Casper/Bonds.lean:a_disagreeing_set_is_refused`]. Shared cause: the `compute_bonds` carried-map agreement requirement in the finaliser's fallback. One defect in that fallback disables the guard for both — the chain that most needs to admit a member is the one that cannot reconcile its bonds maps.
- **P5.** *Threat* [T1: the pre-fringe wait is unbounded] ↔ *Threat* [T2: the tuple-space leg never gives up]. Shared cause (U5's own `[shared-cause]`): "no bound on waiting for the peer". One event — a peer that answers nothing — disables the C181 `terminal` guard and *is* the hazard. Both sit behind the same `tokio::join!`/`select!`.
- **P6.** *Threat* [T3: a joiner is evicted after 3 attempts] ↔ *Barrier* [B3, `casper/src/engine/node_syncing.rs:MAX_SYNC_ATTEMPTS`]. Shared cause: the single terminal bound. The same bound protects (a node will not serve a chain it never synced) and evicts the joiner riding out a short outage — `[barrier-is-the-threat]`.
- **P7.** *Threat* [T2: the tuple leg hangs] ↔ *Barrier* [B4, `casper/src/engine/lfs_block_requester.rs:MAX_IDLE_ROUNDS`]. Shared cause: one `tokio::join!` over both legs. The bound on one leg does not bound the join, and the tuple leg's silence makes the bound unreachable — the guard exists and cannot fire.
- **P8.** *Threat* [T8/T20: start-up replay never opens the API / the replaying validator looks stuck] ↔ *Barrier* [B19, the operator snapshot's `api_reachable`]. Shared cause: the start-up replay. The same replay that keeps a validator from rejoining suppresses the only liveness signal the operator surface has — `[self-referential]`.
- **P9.** *Threat* [T11: the funding drip is burned on a failed submit] ↔ *Barrier* [B12, `node/src/api/web_api_impl.rs:faucet` `FAUCET_MAX_DRIPS_PER_ADDRESS`]. Shared cause: the drip increment precedes the submit. The failed submit is what consumes the barrier — `[barrier-is-the-threat]`.
- **P10.** *Threat* [T10: one peer spends the discovery window] ↔ *Barrier* [B13, `shared/src/rate_limiter.rs:RateLimiter`]. Shared cause: the single global window counter. It is both the admission bound and the exhaustible resource — `[barrier-is-the-threat]`.
- **P11.** *Threat* [T9: a joining validator cannot connect] ↔ *Barrier* [B14, `comm/src/transport/grpc_transport_receiver.rs:accept_tls` handshake semaphore]. Shared cause: the handshake-capacity semaphore is exactly what, when exhausted, makes `accept_tls` drop an honest peer pre-auth — `[barrier-is-the-threat]`.
- **P12.** *Threat* [T16: round closure stalls on a newly bonded validator] ↔ *Barrier* [B16, `block-storage/src/dag/message_state.rs:advance_round`'s window retirement]. Shared cause: the frozen tip. The window's clock (`tip − round_height`) and the round's clock are the same tip, which the round veto freezes — `[self-referential]`.
- **P13.** *Threat* [T5: a non-bootstrap fringe is accepted] ↔ *Barrier* [B2/B6, `casper/src/engine/node_syncing.rs:on_finalized_fringe_message` sender check]. Shared cause: the same ungated acceptance site. The check that should gate the fringe asserts ("ignored") and then proceeds — the barrier's only evidence is the comment describing the gate it does not implement — `[self-referential]`.
- **P14.** *Threat* [T19: untrust does not remove a peer] ↔ *Barrier* [the design that revocation is a governance act, not confiscation — `rholang/src/native_state.rs:untrust`]. Shared cause: the bond is not the operator's to take. The property that makes revocation *safe* is the property that makes it ineffective as removal — `[barrier-is-the-threat]`.
- **P15.** *Threat* [T17: the whole-map quorum denominator requires the lost validator] ↔ *Barrier* [B17 — none; the whole-map denominator is the intent (`block-storage/src/dag/liveness.rs:18-23`) and the barrier protects liveness by refusing to shrink]. Shared cause: the same denominator is both the safety property and what makes a rejoiner mandatory. No barrier is interposed — `[unhoused]`.
- **P16 (the meta-cause).** *Threat cluster* [T12/T13/T14, the leave-path rejections] ↔ *Barrier cluster* [the C201 stale `bonded` read, `casper/src/blocks/proposer/proposer.rs:add_recorded_equivocations`, plus B8/B9 which both depend on the merged pre-state]. Shared cause: frozen finality pins the merged pre-state. A fresh pre-state would let the write land; the same freeze disables every barrier that would apply a leave and is the hazard. Anchored `spec/audit/evidence/c201-proposer-read-results.md` (133 parent sets, one pre-state hash).

*Additional independence findings (barrier-barrier, not threat-barrier):*
- The merge's whole-block rejection is the only mechanism for both the sum and the set leaf (B8, B9); neither covers concurrent set-leaf writers — the residual filed as its own issue, not fixed (C207 §5). So the leave path has one barrier shared by two deviations and neither reaches the third.
- B8 and B9 are both `[one-surface]`/`[barrier-is-the-threat]` on the merge; the join path's barriers (B1–B7) are on `node_syncing`/`node_launch` and share no surface with them — the join and leave failures are independent, which is why #214 criterion 2 fails at (a), (b) and (c) separately.
- The charter's rejected fifth top event ("the operator cannot see what happened") reappears here as T20/B19 — it is the escalation that turns a join failure into a killed validator, not a top event of its own.


---
te: TE-4
title: State, disk or memory exhaustion halts the net
tree: 1e5a64ed4149d5bd7fa15affb1022e3c89f6c492
owner: S3
provenance: >
  the study charter `~/.claude/plans/linear-spinning-eich.md` §2 (TE vocabulary);
  `spec/audit/evidence/n148-results.md`, `n149-results.md`, `n127-loaded-results.md`,
  `spec/audit/passes.md` §19 (residency), `docs/src/node/scaling.md` "Residency is the structural
  limit" / "What would move the numbers", `docs/src/node/validator-requirements.md`
  "RAM again — start-up replay is the floor".
  (matches the tree under audit); `git status --porcelain` → `?? .claude/` only (no tracked
  modification); read-only `grep` over `casper/ block-storage/ sdk/ comm/ node/ rspace/` confirming
  every `path:symbol` below resolves (`attestation_suppressed`, `parents_for_new_block_escaping`,
  `expire_deploys`' only caller, `SearchBudget::{NODE,UNBOUNDED}`, `traverse_tree`, `seen_entries`,
  `set_gauges`, `ConcurrencyLimits`, `MAX_CONCURRENT_HANDSHAKES`, `MAX_STORE_ITEMS_{TAKE,BYTES}`,
  `is_local_address[_resolved]`, `MAX_POOLED_DEPLOYS`).
  not run: no build, no test, no devnet — this sheet synthesises committed runs and read symbols;
  no repository writes.
---

### TE-4 — State, disk or memory exhaustion halts the net

> Framing: this is **loss of control of the hazard**. The hazard is resource consumption that cannot
> be released or capped. Control is lost when consumption outruns every bound the node holds — the
> DAG's resident `seen` ancestry, the on-disk history, the merge's search heap, the transport's
> dispatch/session/stream budget, or block production itself. The bow-tie answers: what drives the
> loss of control (left), what stands between it and the halt (centre), and what it leaves behind
> (right). One structural fact governs the whole page: **the node has no state-GC/pruning release
> path** (`docs/src/node/scaling.md:residency-is-the-structural-limit`; `spec/audit/passes.md:H6`),
> so most barriers below are *detection* or *bound-the-input*, never *release*.

#### Threats (left — credible reasons loss of control occurs, drawn from all study nodes)

- **T1 · DAG message-state ancestry grows Θ(N²) and is never released.** Every accepted message
  retains its whole ancestor `seen` set (Σ|seen| = N(N+1)/2); `prune_fringe` computes a fringe but
  the message map is never shrunk. `block-storage/src/dag/representation.rs:seen_entries`,
  `block-storage/src/dag/message_map.rs:prune_fringe`, `block-storage/src/dag/liveness.rs:LIVENESS_WINDOW`.
  **Measured:** 17,319,555 seen entries / 556 MB of a 1.18 GiB process at 5,885 blocks, ~1.3 MB/block
  — `docs/src/node/scaling.md` "Residency is the structural limit" and `spec/audit/passes.md:H6`
  (U9 H-U9-01; the "need only fringe+window" gloss is contested by CH-U9-04).
- **T2 · Tuple-space history is append-only across the stores.** Cold leaves, radix nodes and roots
  are written put-if-absent; `HistoryAction::Delete` rewrites the trie but frees no bytes; every
  historical root stays "known". `rspace/src/history/history_repository.rs:do_checkpoint_with_native`,
  `rspace/src/history/radix_tree.rs:commit` / `save_and_commit`,
  `rspace/src/history/roots_store.rs:record_root` (U9 H-U9-02; volume `unmeasured`).
- **T3 · The checkpoint is only partly atomic, so orphaned history accumulates on every crash
  mid-checkpoint.** Three writes (cold leaves → trie nodes → root) as three separate `await`s; a
  crash between them leaves unreferenced data retained for ever.
  `rspace/src/history/history_repository.rs:do_checkpoint_with_native`,
  `casper/src/storage.rs:rnode_db_mapping` (U9 H-U9-03; **CH-U9-01: two LMDB environments, three
  logical Dbs** — the "three environments" wording must not be repeated; volume `unmeasured`).
- **T4 · Unbounded block production: the autopropose timer mints a wall-clock block regardless of
  content.** With `--autopropose` a dummy signed `Nil` deploy is injected whenever the pool is empty,
  so production is 2 s-bounded with no ceiling and the chain grows unbounded.
  `node/src/runtime/node_runtime.rs:dummy_deploy_opt` / `dummy_deploy_key`,
  `casper/src/blocks/proposer/proposer.rs:create_block`. **Measured:** `spec/audit/evidence/n148-results.md`
  (production continues after a kill; dummy-deploy-driven) and `spec/audit/evidence/n149-results.md`
  (blocks per deploy = exactly the validator count). **Configuration-gated** — `node/src/configuration/defaults.conf`
  default is `autopropose = false`; the devnet launch (`tools/devnet.sh`) sets it true (CH-U2-02).
- **T5 · Escaped rounds move the tip without advancing the fringe, so offered blocks accumulate
  unfinalised faster than finality retires them.**
  `block-storage/src/dag/message_state.rs:parents_for_new_block_escaping` ("It may not advance the
  fringe") taken by `casper/src/blocks/proposer/proposer.rs:create_block` (U1 H-U1-03; the escape is
  `unmeasured` as the growth driver — CH-U1-03 — the unfinalised set after a kill is what
  `spec/audit/evidence/n148-results.md` measured).
- **T6 · Merge search state explosion: a matching-shaped conflict relation (m disjoint pairs ⇒ 2^m
  terminal states) can breach the option bound before the step bound.**
  `sdk/src/dag/merging.rs:SearchBudget::NODE` (provisional; its doc calibrates it against the
  pre-quotient accepted-set unit) and `sdk/src/dag/merging.rs:enumerate_states`. **Measured
  (pre-quotient control):** 615,599–650,159 states from 29 chains — `spec/audit/evidence/n127-loaded-results.md`
  (U4 H-U4-01 / H-U4-05).
- **T7 · Scope-wide merge preprocessing runs outside any budget, and `traverse_tree` walks the
  dependency map with no visited set.** `sdk/src/dag/merging.rs:fold_rejection` / `traverse_tree`
  (no `SearchBudget`, no visited set — the self-loop the `casper/src/merging.rs:NativeRelations`
  comment names: "the merge never returns"); `casper/src/merging.rs:MergeScope::merge` builds the
  relation map before `search` applies any budget (Law 55's `the_late_guard_overspends`) (U4 H-U4-04
  / H-U4-06).
- **T8 · Transport inbound has no rate limiter: one authenticated peer drives all 1024 dispatch slots.**
  `comm/src/transport/grpc_transport_receiver.rs:GrpcTransportReceiver::send` acquires
  `dispatch_slots` (a `Semaphore`) and spawns, with no `RateLimiter`; the only limiters on the
  peer-facing path are `comm/src/discovery/grpc_kademlia_rpc_server.rs:GrpcKademliaRpcServer` and
  `casper/src/engine/node_running.rs:PeerRateLimiter` (U8 H-U8-01). *Contested:* CH-U8-03 — dispatch
  saturation is *denial* (bounded at 1024 in-flight), so its TE-4 mapping vs TE-1 is not clean.
- **T9 · More live TLS sessions than the connections table holds.** `accept_tls` accepts every TCP
  connection and bounds only *in-flight handshakes* (`comm/src/transport/grpc_transport_receiver.rs:MAX_CONCURRENT_HANDSHAKES`
  = 128); `comm/src/rp/connect.rs:MAX_CONNECTIONS` / `add_conn` caps only the recorded table, so each
  live session is a rustls state + task + fd outside the cap (U8 H-U8-02; `unmeasured`).
- **T10 · A foreign-network / oversize stream is buffered before it is refused.**
  `comm/src/transport/grpc_transport_receiver.rs:GrpcTransportReceiver::stream` drains every chunk
  into a `Vec<Chunk>` first, then `comm/src/transport/stream_handler.rs:collect` runs the
  network-id/`SenderNotVerified`/`MaxSizeReached` breaker (U8 H-U8-05; the late check itself
  `unmeasured` — CH-U8-08).
- **T11 · The SSRF/host guard covers only part of the ingress: the transport handshake classifies IP
  literals only, the resolving guard lives on Kademlia.** `comm/src/rp/handle_messages.rs:is_local_address`
  (literal-only, used by `handle_protocol_handshake` → `check_peer_on_same_network`) vs
  `comm/src/rp/handle_messages.rs:is_local_address_resolved` (Kademlia only). A recorded peer whose
  `sender.host` is an attacker-controlled name is later dialled (`comm/src/rp/connect.rs:clear_connections`),
  aiming the node's outbound work and connection table (U8 H-U8-06 / H-U8-07; `unmeasured`).
- **T12 · A block's system deploys are outside both the per-block width bound and the phlo budget.**
  `casper/src/validate.rs:deploy_count` / `block_phlo` iterate `state.deploys` only;
  `casper/src/blocks/proposer/proposer.rs:per_block_deploy_budget` is the shared 255 budget. Per-block
  replay work outside the gas budget (U7 H-U7-03; bounded in practice by `slash_is_unjustified` —
  CH-U7-02, so the row's severity is contested).
- **T13 · A join hydrates the pre/post trie root of every downloaded block.**
  `casper/src/engine/node_syncing.rs:run_approved_state_sync` builds `collect_block_state_roots` over
  the whole `height_map` and calls `request_tuple_space_roots` per distinct root; the responder's page
  caps (`casper/src/engine/node_running.rs:MAX_STORE_ITEMS_TAKE` / `MAX_STORE_ITEMS_BYTES`) bound one
  page, not the number of roots. Cost is O(chain length) trie hydrations paid per peer (U5 H-U5-06;
  cost `unmeasured`).
- **T14 · Expiry and selection read different clocks, so a tall orphan branch or a lagging finality
  churns the pool.** `block-storage/src/dag/representation.rs:latest_block_number` (max `height_map`
  key, monotone) feeds `casper/src/dag.rs:BlockDagKeyValueStorage::expire_deploys`, while
  `casper/src/blocks/proposer/proposer.rs:select_deploys` reads `next_block_num`; a deploy can expire
  before the finalized chain gives it its window (U7 H-U7-05; premise measured —
  `spec/audit/evidence/n148-results.md`: height runs away while finality does not advance; the
  "same clock" framing is refuted, CH-U7-04).
- **T15 · The deploy pool drains only on an accepted block insert.** `casper/src/dag.rs:BlockDagKeyValueStorage::insert`
  is the only caller of `expire_deploys`; if insertion stops (the #213 wedge) the pool never drains,
  so a transient saturation becomes permanent past every deploy's lifespan (U7 H-U7-06).

#### Preventive barriers (centre-left — what exists between each threat and the top event)

- **P1 · Merge search budget applied before each unit of work** — `sdk/src/dag/merging.rs:SearchBudget::NODE`
  (Law 55, `spec/Rchain/Bounded.lean:the_work_never_exceeds_the_budget`). Evidence: the real falsifier
  is `sdk/src/dag/merging.rs:a_budget_refuses_without_answering_and_never_changes_the_answer`
  (`SearchBudget::NODE`, zero-budget case) — **not** `sdk/tests/merging_scaling.rs:rejection_options_are_bounded_on_a_directed_shape`,
  which runs `SearchBudget::UNBOUNDED` and cannot witness enforcement (CH-U4-02). Guards T6's *step*
  bound only, not its 2^m option shape and not T7's preprocessing.
- **P2 · The directed quotient bounds merge work by its output** —
  `sdk/src/dag/merging.rs:resolve_conflict_set_with_census` / `enumerate_rejection_sets`. Evidence:
  `spec/audit/evidence/n127-loaded-results.md` — worst merge 153–290 states at 161–174 chains against
  the control's 615,599–650,159 at 29.
- **P3 · The parent cap `max-number-of-parents` (255) bounds the merge scope's width** —
  `casper/src/validate.rs` (C191 row `spec/findings.tsv`). Evidence: per C191 the shipped default is
  2.55× the shipped active-validator count and binds no set a bonded network can produce (CH-U4-06),
  and it bounds *parents*, not the deploy chains `fold_rejection`/`traverse_tree` walk (`[one-surface]`).
- **P4 · Transport ConcurrencyLimits return `ResourceExhausted` at the cap** —
  `comm/src/transport/grpc_transport_receiver.rs:ConcurrencyLimits` (`dispatch_slots`, `stream_slots`,
  `blob_slots`). Evidence: symbol read; mechanism pinned by the committed test
  `comm/src/transport/grpc_transport_receiver.rs:a_full_dispatch_queue_is_refused`. Bounds in-flight
  count, **not** rate over time (T8).
- **P5 · Per-stream cap (`max_stream_message_size`, 256 MiB) + aggregate `stream_byte_budget` bound the
  buffered bytes** — `comm/src/transport/grpc_transport_receiver.rs:GrpcTransportReceiver::stream` /
  `charge_stream_budget`. Evidence: committed test `the_aggregate_budget_refuses_once_it_is_spent_and_returns_when_released`.
  Bounds bytes, not the late-check ordering (T10).
- **P6 · `MAX_CONCURRENT_HANDSHAKES` (128) + `HANDSHAKE_TIMEOUT` (10 s) bound in-flight handshakes** —
  `comm/src/transport/grpc_transport_receiver.rs:accept_tls`. Evidence: symbol read; it bounds
  *in-flight handshakes*, not *established sessions* (T9) — a barrier that does not guard its
  deviation (CH-U8-05).
- **P7 · The deploy pool cap and per-surface rate limiters** — `casper/src/dag.rs:MAX_POOLED_DEPLOYS`
  (10,000, checked in `add_deploy`), `node/src/api/grpc/mod.rs:serve_deploy`,
  `node/src/web/http.rs:deploy_rate_limiter`. Evidence: committed test `casper/src/dag.rs:add_deploy_rejects_when_pool_full`.
  Per-sender since 2026-10-13 (W-N2-01/04/08): the ingress limits are keyed on the peer and the pool has a per-deployer share (U7 H-U7-01; CH-U7-01 notes the limiters are per-surface, so
  starvation is per-surface, not "all senders").
- **P8 · Block-level phlo and deploy-count bounds refuse an over-budget block** —
  `casper/src/validate.rs:block_phlo` / `deploy_count`. Evidence: symbol read. Sits on the block path
  only (no admission-side phlo filter), so it converts a bad deploy into a proposer halt (U7 H-U7-02).
- **P9 · `slash_is_unjustified` re-derives the slash set from this node's own DAG, so a peer block can
  carry only slashes this node already agrees are justified, plus one `CloseBlock`** —
  `casper/src/interpreter_util.rs:slash_is_unjustified`. Evidence: symbol read — this is what bounds
  T12's system-deploy work (CH-U7-02).
- **P10 · The liveness retirement and the escape are meant to retire an absent sender so the round can
  still close** — `block-storage/src/dag/liveness.rs:live_weight_set`,
  `block-storage/src/dag/message_state.rs:parents_for_new_block_escaping`,
  `block-storage/src/dag/message_state.rs:advance_round`. Evidence: `spec/audit/evidence/n148-results.md`
  (finality frozen at the kill, 3/3) — the barrier does **not** hold; it is listed as a barrier only
  because it is the one that exists.
- **P11 · Detection gauges publish the growth** — `casper/src/dag.rs:set_gauges` publishes
  `rchain.dag.seen_entries` and `rchain.dag.logical_bytes` from
  `block-storage/src/dag/representation.rs:logical_bytes`. Evidence: the measured 556 MB / 17,319,555
  reading came from these gauges (`spec/audit/passes.md` §19). Detection, never prevention.
- **P12 · Exporter/importer trie transfer shifts the sync cost between nodes** —
  `node/src/runtime/node_runtime.rs:create_rspace_importer`, `rspace/src/history/export.rs:sequentialExport`.
  Evidence: symbol read; it does not avoid the start-up DAG rebuild (T-startup / C2).
- **P13 · For T1/T2/T3 there is no preventive barrier at all.** The node accepts and *records* the
  rate rather than bounding it — `docs/src/node/scaling.md:residency-is-the-structural-limit`,
  `spec/audit/passes.md:H6` (`unmeasured` for any release — none exists).

#### Top event

**State, disk or memory exhaustion halts the net.**

#### Consequences (right — what follows once control is lost)

- **C1 · The node's RSS exhausts the host; the process is OOM-killed or the box freezes.** Measured
  shape: 556 MB of `seen` alone inside a 1.18 GiB process at 5,885 blocks, advancing ~1.3 MB/block with
  no plateau — `docs/src/node/scaling.md` "Residency is the structural limit",
  `spec/audit/passes.md` §19.
- **C2 · A node that exhausts memory during start-up replay never opens its API and loops on restart,
  so a validator cannot rejoin and an outsider cannot sync** (TE-4 → TE-3 convergence) —
  `docs/src/node/validator-requirements.md` "RAM again — start-up replay is the floor" (1,142-block
  state: 57→284 MB, API after 225 s; "a 1 GB host … can be unable to restart a ~1000-block chain").
- **C3 · Unbounded on-disk growth** from the append-only tuple-space history and any crash-orphaned
  checkpoint data — `rspace/src/history/history_repository.rs:do_checkpoint_with_native`,
  `rspace/src/history/radix_tree.rs:commit` (volume `unmeasured`).
- **C4 · As nodes die the round cannot close and finality freezes — TE-4 → TE-1.** Measured: killing a
  validator freezes finality while production continues, 3/3 — `spec/audit/evidence/n148-results.md`;
  finality never advances at N ≥ 3, `spec/audit/evidence/n149-results.md`.
- **C5 · The operator cannot see it.** The wire `Status` carries no health field
  (`models/src/casper/protocol/deploy_service.rs:Status`) and the observation surface is absent for the
  whole replay (`docs/src/node/validator-observer-setup.md` §9 (restart replay)) — the operator
  surface is the escalation factor in all four top events (charter §2).

#### Mitigating / recovery barriers (centre-right — reduce severity after the top event)

- **M1 · Prometheus `/metrics` gauges** — `casper/src/dag.rs:set_gauges` exposes
  `rchain.dag.seen_entries` / `rchain.dag.logical_bytes`, so an operator can see growth before
  exhaustion. Evidence: the 556 MB reading was taken from `/metrics` (`spec/audit/passes.md` §19).
- **M2 · `/api/v1/status/operator` folds in the proposer health** — `node/src/api/admin_web_api_impl.rs:operator_status`,
  `node/src/api/shard_routing.rs:proposer_health` (the C195 fix). Evidence: symbol read; the per-shard
  gauge `node/src/runtime/node_runtime.rs:push_proposer_health` is asserted by the committed test
  `a_halted_timer_is_readable_on_the_scrape`.
- **M3 · The host OOM killer and the operator's resource-ceiling rule.** The surviving control is the
  operational rule (one heavyweight suite at a time, output to `target/`); anchor `[unhoused]` for a
  committed run — `unmeasured`.
- **M4 · Exporter/importer trie transfer** moves state between nodes to shorten a joiner's path —
  `node/src/runtime/node_runtime.rs:create_rspace_importer` (symbol read; shifts, does not remove, the
  cost).
- **M5 · Restart the process** clears an in-memory halt — but recovery is bounded below by C2's
  replay floor (`docs/src/node/validator-requirements.md` "start-up replay is the floor").
- **M6 · Drop the dummy deploy by omitting `--autopropose`** stops wall-clock production —
  `node/src/runtime/node_runtime.rs:dummy_deploy_key` (symbol read; this is also the only gate, see SC-2).

#### Degradation (escalation) factors (weaken a barrier; each with its own control or "none")

- **D1 · Finality frozen (the #213 wedge / the #148 kill freeze) freezes the release clock** — it
  weakens P10 (the retirement/escape read against a frozen tip). **Control:** none — open defect #213;
  evidence `spec/audit/evidence/n148-results.md`.
- **D2 · Finality lag makes the DAG height run ahead of the finalized chain** — it weakens the deploy
  expiry clock (T14). **Control:** none distinct — expiry (`casper/src/dag.rs:expire_deploys`) and
  selection (`casper/src/blocks/proposer/proposer.rs:select_deploys`) read different clocks (CH-U7-04).
- **D3 · No state-GC / DAG-pruning release path (recorded decision)** — it removes the only lever
  against T1/T2. **Control:** the recorded decision itself — `docs/src/node/scaling.md:residency-is-the-structural-limit`.
- **D4 · `--autopropose` + a deployer key (devnet config)** — the same flag that triggers T4 is the
  only gate; passing it both removes the barrier and is the deviation. **Control:** omit `--autopropose`
  — `node/src/runtime/node_runtime.rs:dummy_deploy_key`.
- **D5 · Start-up replay is superlinear, so recovery from C1/C2 has no bound** — it weakens M5.
  **Control:** none — ranked fix #2 in `docs/src/node/scaling.md:what-would-move-the-numbers`.
- **D6 · The observability barrier is one surface** — `/metrics`, `/api/status` and the nginx-derived
  `/health` read the *same* in-process state (`[one-surface]`, C195), so a growth/halt the node cannot
  self-report has no independent second read. **Control:** none; anchors `casper/src/dag.rs:set_gauges`,
  `node/src/api/admin_web_api_impl.rs:operator_status`.
- **D7 · The pool drains only on an accepted block insert** — the #213 wedge stops the drain, making a
  transient saturation permanent (T15). **Control:** none distinct —
  `casper/src/dag.rs:BlockDagKeyValueStorage::insert` is the only caller of `expire_deploys`.
- **D8 · A finite-RAM host with limited swap amplifies any of T1–C1** — the measured box froze under
  three concurrent sessions (memory `host-resource-ceiling`). **Control:** the operational rule
  (one heavyweight suite at a time) — `[unhoused]` for a committed run.

#### Shared-cause pairs and independence findings (hunt across nodes)

> The canonical pattern: a barrier depends on an event whose *absence* is the threat, so the one event
> both triggers the threat and disables the barrier. Below, each pair names both members and the shared
> cause. The direct TE-4 analogue of the charter's bond/merge exemplar is **SC-3** (the drain barrier
> needs a block insert; the wedge stops inserts) and **SC-1/SC-10** (the release/escape clocks are
> measured against the tip the deadlock freezes).

- **SC-1 [shared-cause] — T5 (unfinalised growth) ↔ P10 (the liveness-retirement clock).** Shared
  cause: **the frozen tip**. The same tip that lets offered blocks accumulate unfinalised (T5) is the
  tip the retirement clock `live_weight_set` is measured from, so the event both grows the set and
  disables the barrier that would retire the quiet sender. Anchors
  `block-storage/src/dag/liveness.rs:live_weight_set`,
  `block-storage/src/dag/message_state.rs:has_advanced_past_the_round` (U1 H-U1-03 `[shared-cause]`).
- **SC-2 [barrier-is-the-threat] — T4 (unbounded production) ↔ the attestation-suppression clause.**
  Shared cause: **the dummy `Nil` deploy pins `new_state_transition` true**. That single fact both *is*
  the unbounded production (T4) and short-circuits the suppression term `!(new_state_transition || cadence_due)`
  so the node attests regardless of `cadence_due`. Anchors
  `casper/src/blocks/proposer/proposer.rs:attestation_suppressed` (the `1518` OR),
  `node/src/runtime/node_runtime.rs:dummy_deploy_opt` (U2 H-U2-01 `[barrier-is-the-threat]`; CH-U2-01/02).
- **SC-3 [shared-cause] — T15 (pool never drains) ↔ P7/T15's driver `expire_deploys`.** Shared cause:
  **no accepted block insert**. `expire_deploys` runs *only* from
  `casper/src/dag.rs:BlockDagKeyValueStorage::insert`; the same event the drain barrier needs (an
  accepted block) is exactly what the #213 wedge withholds, so a full pool stays full. This is the
  TE-4 instance of the charter's exemplar. Anchor `casper/src/dag.rs:BlockDagKeyValueStorage::insert`
  / `expire_deploys` (U7 H-U7-06 `[shared-cause]`).
- **SC-4 [barrier-is-the-threat] — H-U6-01's barrier (cost-accounting merges per accepted deploy) ↔
  H-U6-03's hazard (a deploy riding the rejected sibling boundary block is discarded).** Shared cause:
  **the merge's whole-block rejection**. `reject_whole_blocks` is the barrier that preserves a native
  write in one row and the mechanism that discards a deploy in another; one whole-block rejection is
  both. Anchors `casper/src/merging.rs:NativeRelations::reject_whole_blocks`,
  `casper/src/merging.rs:sibling_boundaries_with_different_pots_merge` (U6 H-U6-01/H-U6-03;
  CH-U6-08 relabels H-U6-03 `[barrier-is-the-threat]`).
- **SC-5 [shared-cause] — T11 (SSRF dial) ↔ P11's sibling guard `is_local_address_resolved`.** Shared
  cause: **the name resolution**. The guard and the dial resolve the *same* name at different moments;
  a short-TTL answer that passes the check at check time resolves to a private/loopback address at dial
  time, so the event (the TTL flip) both passes the barrier and is what the dial follows. Anchors
  `comm/src/rp/handle_messages.rs:is_local_address_resolved` (check time; the residual paragraph on the
  same symbol names the dial time) (U8 H-U8-07 `[shared-cause]`).
- **SC-6 [shared-cause] — T8-adjacent (handshake reply pins a dispatch slot) ↔ T11 (SSRF dial).** Shared
  cause: **one line — `transport.send(peer, response)` in `handle_protocol_handshake` dialling the
  attacker-claimed `sender.host`.** The 5 s slot-pin is the SSRF dial aimed at a black-hole address, so
  the two deviations are one mechanism with two consequences. Anchor
  `comm/src/rp/handle_messages.rs:handle_protocol_handshake` (U8 CH-U8-07; merge of H-U8-04 and H-U8-06).
- **SC-7 [barrier-is-the-threat, ×2] — P7 (global pool cap + per-surface `RateLimiter`) ↔ the Kademlia
  `RateLimiter` (`comm/src/discovery/grpc_kademlia_rpc_server.rs:DEFAULT_KADEMLIA_RATE_LIMIT_PER_SEC`).**
  Shared cause: **the bound is global, not per-sender**. One sender spends the whole window and refuses
  every honest sender on that surface; the single counter is both the admission bound and the
  exhaustible resource. Anchors `shared/src/rate_limiter.rs:RateLimiter` (one `window_start`/`count`),
  `node/src/api/grpc/mod.rs:serve_deploy`, `casper/src/dag.rs:MAX_POOLED_DEPLOYS` (U7 H-U7-01 / U8
  H-U8-03, both `[barrier-is-the-threat]`).
- **SC-8 [one-surface] — P11 (U9 DAG gauges) ↔ M2 (U10 `/api/status` + `/metrics`).** Shared cause:
  **the same in-process state**. The observability barrier is one barrier, not three — `/metrics`,
  `/api/status` and `/health` all read the same cells, so a growth or halt the node cannot self-report
  has no independent second read. Anchors `casper/src/dag.rs:set_gauges`,
  `node/src/api/admin_web_api_impl.rs:operator_status`, `node/src/api/shard_routing.rs:proposer_health` (U9 H-U9-01 /
  U10 H-U10-01, `[one-surface]`; proven by C195).
- **SC-9 [shared-cause] — T13 (join's O(chain) trailing pull) ↔ the join's `terminal`/`MAX_SYNC_ATTEMPTS`
  guard.** Shared cause: **a peer that stops answering.** One event (the peer that never completes a
  leg) is both the hazard and what disables the guard — `request_tuple_space_roots` resends with no
  `MAX_IDLE_ROUNDS` and the `tokio::join!` never returns, so `terminal` is never reached. Anchors
  `casper/src/engine/lfs_tuple_space_requester.rs:request_tuple_space_roots`,
  `casper/src/engine/node_launch.rs:apply` (U5 H-U5-01/H-U5-03 `[shared-cause]`; CH-U5-07 broadens it).
- **SC-10 [self-referential] — the escape (`parents_for_new_block_escaping`) ↔ the round's window
  clock (`LIVENESS_WINDOW`).** Shared cause: **the frozen tip**. The escape's `blocked_since_advance`
  increments only on a proposal *request* the tap feeds, and the round's retirement clock is measured
  against the tip the deadlock freezes — one frozen tip is both. Anchors
  `casper/src/blocks/proposer/proposer.rs:create_block` (the `escape` block),
  `block-storage/src/dag/liveness.rs:LIVENESS_WINDOW` (U1 H-U1-07, U2 H-U2-03, U3 H-U3-04
  `[self-referential]`).
- **SC-11 [barrier-is-the-threat] — T6/P1: the merge budget's own constant ↔ its firing.** Shared
  cause: **the bound is hit.** `SearchBudgetExceeded` is *itself* the row's consequence
  (`SearchBudgetExceeded` → `String` → `ValidateError::Internal` → a dropped block with no re-queue),
  so the barrier and the threat are one event. Anchors `sdk/src/dag/merging.rs:SearchBudget::NODE` /
  `SearchBudgetExceeded`, `casper/src/merging.rs:MergeScope::merge` (U4 H-U4-01; CH-U4-05 relabels
  `[barrier-is-the-threat]`).
- **SC-12 [one-surface] — T7/P3: the parent cap (`max-number-of-parents`) ↔ the fold's width
  (`fold_rejection`/`traverse_tree`).** Shared cause: **"the merge scope" — the cap bounds parents, the
  fold walks deploy chains.** The bound and the input it is meant to bound are different surfaces, so
  the same "scope" that the cap is filed against is the one it does not reach. Anchors
  `casper/src/blocks/proposer/proposer.rs:per_block_deploy_budget`,
  `sdk/src/dag/merging.rs:fold_rejection` (U4 H-U4-04/H-U4-06 `[one-surface]`).
- **SC-13 [shared-cause] — T14 (early expiry) ↔ T15 (pool never drains).** Shared cause: **the frozen
  finality of #148/#149.** The same finality stall is what lets `latest_block_number` run ahead of the
  finalized chain (expiring deploys early, T14) *and* what stops the inserts `expire_deploys` needs
  (T15); the two deployment-clock deviations share one upstream event. Anchors
  `casper/src/dag.rs:BlockDagKeyValueStorage::expire_deploys`,
  `block-storage/src/dag/representation.rs:latest_block_number`, `spec/audit/evidence/n148-results.md`
  (U7 H-U7-05/H-U7-06).
- **SC-14 [one-surface] — P12 (exporter/importer) ↔ the start-up DAG rebuild.** Shared cause: **no
  persisted DAG snapshot.** `create_rspace_importer` moves the trie on the sync surface but cannot
  avoid the DAG rebuild (`casper/src/dag.rs:BlockDagKeyValueStorage::create`), which is the same
  no-snapshot event on the restart surface. Anchors
  `node/src/runtime/node_runtime.rs:create_rspace_importer`,
  `casper/src/dag.rs:BlockDagKeyValueStorage::create` (U9 H-U9-04 `[one-surface]`).

#### Independence findings (things this synthesis must carry forward)

- **The whole page rests on one unbarriered fact:** there is **no release path** for the DAG message
  map or the tuple-space history (P13). Every "barrier" for T1/T2/T3 is bound-the-input or detection
  only — the register itself says capping the residency "would break finalization"
  (`spec/audit/passes.md:H6`), which refutes U9's "need only fringe+window" gloss (CH-U9-04).
- **Two TE class assignments are internally inconsistent and must be reconciled before printing:**
  U8's `dispatch_slots` saturation is TE-4 in H-U8-01 and TE-1 in H-U8-04 (CH-U8-03); U6's
  bonds-map refusal is TE-2 in H-U6-02 and TE-1 in H-U6-04 (CH-U6-04). Neither is resolvable from the
  sheets alone; both bear on whether a row appears under this top event.
- **T4 and T8's severities are configuration-scoped, not node-invariant.** T4 needs `--autopropose`
  (CH-U2-02); T8's limiter set is per-surface (CH-U7-01) and its consequence is denial rather than
  memory (CH-U8-03). A reader must not print either as unconditional.
- **The strongest TE-4 threats are the measured ones (T1, T4, T6, T13-adjacent), and the strongest
  barriers are also measured (P2, P4, P5, P7, P11).** Every Table-B row on U8 is symbol-read only
  ("no committed run measures any U8 barrier"), so the transport half of this bow-tie is
  `unmeasured` end to end.
- **The recovery side is the weakest.** M1–M2 detect; M3–M5 recover only within the start-up replay
  floor (C2), which is itself unmitigated (D5). There is no barrier that reduces the *severity* of an
  exhaustion once the DAG history has grown — only detection and restart.

### 2.5 Barrier independence — the findings

Every barrier in §2 was assessed for effectiveness, independence and dependability, and a barrier that
shares a common cause with the threat it guards does not count. The bow-tie owners were explicitly
charged with hunting these **across** study nodes, not just within one. Four classes were used:

- `[shared-cause]` — the barrier and the threat it guards share a cause, so one event both disables the barrier and *is* the event it exists to prevent.
- `[barrier-is-the-threat]` — one rule is both the bound and the hazard.
- `[self-referential]` — the barrier's reference point (a tip, a clock, a window) is frozen by the failure it guards.
- `[one-surface]` — what looks like several independent barriers is one: the same in-process state, the same configuration, the same table.

The cross-node sweep returned **65** distinct pairs. Each names both members and the shared cause:

1. SC-1 (canonical) — threat T6/H-U6-02 carried bonds maps disagree vs the barrier the compute_bonds carried-map agreement (spec/Rchain/Casper/Bonds.lean:a_disagreeing_set_is_refused): a bond takes effect only on a merge and the wedge stops merges, so 'no merge happens' both disables the barrier and is the divergence it guards
2. SC-2 — threat T2/H-U1-07 (escape retires a lagging validator) vs barrier block-storage/src/dag/liveness.rs:live_weight_set and advance_round: the shared cause is the frozen tip, which blocks the fringe and freezes the clock the retirement is measured from (also a two-barrier pair with SC-10)
3. SC-3 — threat H-U3-01 (fringe does not advance; DAG rests at one height) vs barrier block-storage/src/dag/message_state.rs:parents_for_new_block: the shared cause is that whether any new block/layer exists is the round/tap's own decision
4. SC-4 — threat T1/H-U1-02 (a second block at one (sender, seq_num)) vs barrier the AlreadyProposedThisRound veto in casper/src/blocks/proposer/proposer.rs:create_block: the shared cause is the DAG advancing between the parent-set read and the insert, which both opens the deviation and is what the veto fails to observe
5. SC-5 — threat H-U1-01 (the tap never re-fires) vs barrier the liveness escape in casper/src/blocks/proposer/proposer.rs:create_block (increments blocked_since_advance only on a proposal request): the shared cause is 'no proposal request' — the tap seal removes exactly the request the escape needs
6. SC-6 — threat H-U1-04 (the stale bonded re-slash read feeding add_recorded_equivocations and the quorum) vs barrier the bonded filter in casper/src/blocks/proposer/proposer.rs:add_recorded_equivocations + casper/src/validate.rs:equivocation_is_proved: both gate on the same frozen compute_bonds(pre_state_hash) view (C201, a coupling to frozen finality)
7. SC-7 — threat H-U6-03 (boundary siblings with different pots; the deploy riding the loser is discarded) vs barrier the merge's whole-block rejection (casper/src/merging.rs:MergeScope::fold / reject_whole_blocks): one whole-block rejection is both the guard against a conflicting native write and the harm — [barrier-is-the-threat] (s2-U6 CH-U6-08)
8. SC-8 — threat H-U7-06 (the deploy pool never drains) vs barrier casper/src/dag.rs:expire_deploys: an accepted block insert is what runs the barrier and its absence (the #213 wedge) is what withholds it
9. SC-9 — threat T5's fringe-seizure half (H-U5-05, a non-bootstrap peer sets the sync target) vs barrier the sender check casper/src/engine/node_syncing.rs:on_finalized_fringe_message (logs 'ignored', no return): the same inbound FinalizedFringe both drives the sync and is what the barrier is supposed to filter — [self-referential]
10. SC-10 — two barriers sharing one cause: the escape's waited <= LIVENESS_WINDOW (casper/src/blocks/proposer/proposer.rs:create_block) and the round's tip - round_height > LIVENESS_WINDOW retirement (block-storage/src/dag/message_state.rs:advance_round), both measured against the one frozen tip the deadlock pins
11. SC-11 (independence finding) — H-U6-02 (refusal -> TE-2) vs H-U6-04 (the identical refusal -> TE-1): the shared cause is the compute_bonds agreement requirement, and one mechanism cannot be TE-2 in one row and TE-1 in another (a refusal produces no divergent state) — s2-U6 CH-U6-04
12. SC-12 (independence finding, contested) — threat T4 (a different merge than peers) vs barrier the exactness differential sdk/src/property_tests.rs:rejection_options_match_a_literal_enumeration: s1-U4 alleges [shared-cause] (the oracle transcribes the shipped algorithm), but s2-U4 CH-U4-02 argues the oracle is the pre-quotient accepted-set algorithm, a different one
13. SC-13 (independence finding) — threat H-U4-01 (a merge that exceeds the budget returns no answer and the block is dropped) vs barrier the budget applied before each unit of work (sdk/src/dag/merging.rs:SearchBudget::NODE): the budget's refusal IS the row's stated consequence — [barrier-is-the-threat], corrected by s2-U4 CH-U4-05 from s1's [self-referential]
14. P1 canonical: leave-threat T13/T14 (a withdraw takes effect only on a merged, finalised boundary block) ↔ the barrier rholang/src/native_state.rs:close_block/is_epoch_boundary that applies the leave — shared cause is the frozen tip (block-storage/src/dag/message_state.rs:has_advanced_past_the_round) that stops merges; no merge finishes both disables the leave barrier and is why the validator wants to leave
15. P2: barrier B8 (cost accounting merges per accepted deploy, casper/src/merging.rs:a_cost_accounted_block_keeps_its_own_native_writes_through_the_merge) ↔ barrier B9 (casper/src/merging.rs:sibling_boundaries_with_different_pots_merge) — shared cause casper/src/merging.rs:reject_whole_blocks; the one whole-block rejection is the guard in H-U6-01 and the harm in H-U6-03
16. P3: threat T12 (a concurrent set-leaf PoS write is lost) ↔ barrier B8 (cost accounting composes per accepted deploy) — shared cause: native state merges whole-slot snapshots not transitions (spec/audit/evidence/c207-merge-loses-native-writes.md §5); the event that triggers the threat (two concurrent set-leaf writers) is the one case the composing barrier cannot cover
17. P4: threat T18 (empty-fringe bond wedge) ↔ barrier H-U6-02/H-U6-04 (spec/Rchain/Casper/Bonds.lean:a_disagreeing_set_is_refused) — shared cause: the compute_bonds carried-map agreement requirement in the finaliser fallback; one defect disables the guard for both
18. P5: threat T1 (unbounded pre-fringe wait) ↔ threat T2 (tuple-space leg never gives up) — shared cause 'no bound on waiting for the peer'; one peer that answers nothing disables the C181 terminal guard and is the hazard
19. P6: threat T3 (joiner evicted after 3 attempts) ↔ barrier B3 (casper/src/engine/node_syncing.rs:MAX_SYNC_ATTEMPTS) — shared cause the single terminal bound, which both protects against un-synced serving and evicts the joiner [barrier-is-the-threat]
20. P7: threat T2 (tuple leg hangs) ↔ barrier B4 (casper/src/engine/lfs_block_requester.rs:MAX_IDLE_ROUNDS) — shared cause one tokio::join! over both legs; the bound on one leg does not bound the join and the tuple leg's silence makes the bound unreachable
21. P8: threat T8/T20 (start-up replay never opens the API / the replaying validator looks stuck) ↔ barrier B19 (the operator snapshot's api_reachable) — shared cause the start-up replay, which keeps the validator from rejoining and suppresses the only liveness signal the surface has [self-referential]
22. P9: threat T11 (funding drip burned on a failed submit) ↔ barrier B12 (node/src/api/web_api_impl.rs:faucet, FAUCET_MAX_DRIPS_PER_ADDRESS) — shared cause the drip increment precedes the submit; the failed submit is what consumes the barrier [barrier-is-the-threat]
23. P10: threat T10 (one peer spends the discovery window) ↔ barrier B13 (shared/src/rate_limiter.rs:RateLimiter) — shared cause the single global window counter, both the admission bound and the exhaustible resource [barrier-is-the-threat]
24. P11: threat T9 (a joining validator cannot connect) ↔ barrier B14 (comm/src/transport/grpc_transport_receiver.rs:accept_tls handshake semaphore) — shared cause the handshake-capacity semaphore; its exhaustion is what makes accept_tls drop an honest peer pre-auth [barrier-is-the-threat]
25. P12: threat T16 (round closure stalls on a newly bonded validator) ↔ barrier B16 (block-storage/src/dag/message_state.rs:advance_round window retirement) — shared cause the frozen tip, both the clock the barrier measures from and the state it exists to age past [self-referential]
26. P13: threat T5 (non-bootstrap fringe accepted) ↔ barrier B2/B6 (casper/src/engine/node_syncing.rs:on_finalized_fringe_message sender check) — shared cause the same ungated acceptance site; the check logs 'ignored' and proceeds [self-referential]
27. P14: threat T19 (untrust does not remove a peer) ↔ its own guard (revocation is a governance act, not confiscation, rholang/src/native_state.rs:untrust) — shared cause 'the bond is not the operator's to take': the property that makes revocation safe is what makes it ineffective [barrier-is-the-threat]
28. P15: threat T17 (whole-map quorum denominator requires the lost validator) ↔ barrier B17 (none; the denominator is the intent, block-storage/src/dag/liveness.rs:live_weight_set) — shared cause the same denominator is both the safety property and what makes a rejoiner mandatory; no barrier interposed [unhoused]
29. P16 meta-cause: leave-path threats T12/T13/T14 ↔ barriers B8/B9 and the C201 stale bonded read (casper/src/blocks/proposer/proposer.rs:add_recorded_equivocations) — shared cause frozen finality pins the merged pre-state; a fresh pre-state would let the write land, so the same freeze disables every leave barrier and is the hazard (spec/audit/evidence/c201-proposer-read-results.md: 133 parent sets, one pre-state hash)
30. SCP-1 (canonical): threat 'a bond/withdraw/delegate riding the losing sibling boundary block takes effect on no node' (U6 H-U6-01/H-U6-03) vs barrier 'the merge rejects the losing sibling boundary block whole' (casper/src/merging.rs::boundary_merge_tests) — shared cause: the merge's whole-block rejection, so 'no successful boundary merge happens' both disables bond activation and is the event the barrier exists to prevent
31. SCP-2: threat 'the round never closes -> finality freezes while production continues' (U2 H-U2-03) vs barrier 'the escape parents_for_new_block_escaping moves the tip' (U3 H-U3-04) — shared cause: the frozen tip (the round's clock and the escape's clock are the same tip the veto freezes); cross-node U2<->U3
32. SCP-3: threat 'the attest tap stops issuing proposal requests once a round rests' (U1 H-U1-01) vs barrier 'the liveness escape increments blocked_since_advance' (proposer.rs:create_block) — shared cause: the tap seal, which removes the request the escape needs to increment
33. SCP-4: threat 'a pool that never drains' (U7 H-U7-06) vs barrier 'expire_deploys runs on every accepted block insert' (casper/src/dag.rs:BlockDagKeyValueStorage::insert) — shared cause: the same event (an accepted block / the #213 wedge) is what the barrier needs and what stops it
34. SCP-5: threat 'finality frozen, the tip cannot advance' (U3 H-U3-01/H-U3-04) vs barrier 'live_weight_set retires an absent sender' (block-storage/src/dag/liveness.rs:live_weight_set) — shared cause: the frozen tip that blocks the fringe freezes the retirement clock; cross-node U1<->U3
35. SCP-6: threat 'a permanent >=1/3 loss makes the quorum unreachable' (U3 H-U3-02) vs barrier 'the whole bonded map as the quorum denominator' (finalizer.rs:calculate_fringe) — shared cause: the same denominator that keeps the quorum safe is what makes it unreachable
36. SCP-7: threat 'a retired-but-never-evicted sender in mv.parents -> no full partition' (U3 H-U3-03 residual) vs barrier 'live_justifications restricts the justification set to live senders' (liveness.rs:calculate_finalization_detailed) — shared cause: the same live partition gates the round and the fringe (contested: challenger relabels one-surface)
37. SCP-8: threat 'the finaliser refuses two disagreeing bonds maps -> chain halts' (U6 H-U6-02) vs barrier 'the same agreement requirement in compute_bonds fallback' (U6 H-U6-04) — shared cause: the compute_bonds agreement requirement, one defect disables both
38. SCP-9: threat 'the merge returns no answer (budget exceeded) -> block dropped with no re-queue' (U4 H-U4-01) vs barrier 'the budget applied before each unit of search work' (sdk/src/dag/merging.rs:search, Law 55) — shared cause: the budget's own refusal is the row's consequence, one event is both the barrier and the drop
39. SCP-10: threat 'the fold Err on a missed concurrent-writer pair rejects the block whole' (U4 H-U4-03) vs barrier 'the native relation derived from the DAG's own ancestry' (casper/src/merging.rs:NativeRelations::conflicting) — shared cause: the ancestry-derived relation, whose determinism is the surface the completeness deviation lives on
40. SCP-11: threat 'the proposer builds a block its own validate_block refuses -> autopropose halts' (U7 H-U7-02) vs barrier 'block_phlo refuses an over-budget block' (casper/src/validate.rs:block_phlo) — shared cause: the phlo bound sits on the block path only, converting a bad deploy into a proposer halt
41. SCP-12: threat 'the admission bound refuses honest senders' (U7 H-U7-01) vs barrier 'the shared pool cap + per-surface limiters' (casper/src/dag.rs:BlockDagKeyValueStorage::add_deploy) — shared cause: the global bound both bounds the flood and is what refuses honest senders
42. SCP-13: threat 'a halted node is invisible on the CLI/gRPC surface' (U10 H-U10-01) vs barrier 'HTTP /api/v1/status/operator folds proposer_health() in' (node/src/api/admin_web_api_impl.rs::operator_status) — shared cause: the in-process ProposeHealth state, so /metrics + /api/status + /health are one barrier, not three (C195)
43. SCP-14: threat 'a halted node reads as live after restart (false all-clear)' (U10 H-U10-06) vs barrier 'the deliberate design that nothing restarts the timer but the process' (casper/src/api/block_api.rs:ProposeHealth) — shared cause: the in-memory atomic + process-restart design; the property that makes the halt sticky is what loses the observation
44. SCP-15: threat 'the sync waits unbounded for the pre-attempt fringe' (U5 H-U5-01) vs barrier 'the tuple-space leg is unbounded so MAX_SYNC_ATTEMPTS/terminal never fires' (U5 H-U5-03) — shared cause: no bound on waiting for the peer, one event disables the guard and is the hazard
45. SCP-16: threat 'the first stalled sync consumes the one-shot trigger' (U5 H-U5-05) vs barrier 'C181's terminal bound stops a node serving a chain it never synced' (U5 H-U5-07) — shared cause: the same terminal bound both protects and evicts the joiner
46. SCP-17: threat 'dispatch slots pinned by handshake replies -> the node stops answering every message type' (U8 H-U8-04) vs threat 'the SSRF dial of the attacker-claimed host in the same call' (U8 H-U8-06) — shared cause: the single transport.send(peer, response) in comm/src/rp/handle_messages.rs:handle_protocol_handshake
47. SCP-18: threat 'one peer drains the global Kademlia rate window' (U8 H-U8-03) vs barrier 'the global RateLimiter bounds total request rate' (shared/src/rate_limiter.rs:RateLimiter) — shared cause: the single window counter is both the admission bound and the exhaustible resource
48. SCP-19: threat 'the SSRF check resolves the host before the dial and the dial re-resolves' (U8 H-U8-07) vs barrier 'is_local_address_resolved fail-closed on unresolved names' (handle_messages.rs:is_local_address_resolved) — shared cause: the same name; a short-TTL answer passes the check and is what the dial follows
49. SCP-20: threat 'the escape matures on a local attempt count while the round's clock is frozen heights' (U1 H-U1-07) vs barrier 'the bound waited <= LIVENESS_WINDOW' (proposer.rs:create_block) — shared cause: the bound counts local attempts while the property it substitutes for is measured in heights the deadlock freezes
50. SCP-21: threat 'the stale bonded read feeds check_active_validator and the attestation quorum' (U1 H-U1-04) vs barrier 'the bonded filter in add_recorded_equivocations + receiver re-check equivocation_is_proved' (proposer.rs:add_recorded_equivocations, validate.rs:equivocation_is_proved) — shared cause: the same compute_bonds(pre_state_hash) view C201 shows is frozen (contested, CH-U1-05)
51. SCP-22: threat 'the node attests at every height (production unbounded)' (U2 H-U2-01) vs barrier 'the cadence gate cadence_due' (proposer.rs:cadence_due) — shared cause: cadence_due is the right operand of the OR the deviation short-circuits, so the dummy-deploy event both disables the barrier and is the unbounded production
52. SC-1 [shared-cause] T5 unfinalised growth ↔ P10 liveness-retirement clock — shared cause: the frozen tip (one tip both grows the unfinalised set and is what the retirement clock is measured from): block-storage/src/dag/liveness.rs:live_weight_set, block-storage/src/dag/message_state.rs:has_advanced_past_the_round
53. SC-2 [barrier-is-the-threat] T4 unbounded production ↔ the attestation-suppression clause — shared cause: the dummy Nil deploy pins new_state_transition true, which both is the unbounded production and short-circuits !(new_state_transition || cadence_due): casper/src/blocks/proposer/proposer.rs:attestation_suppressed, node/src/runtime/node_runtime.rs:dummy_deploy_opt
54. SC-3 [shared-cause] T15 pool never drains ↔ expire_deploys (the drain barrier) — shared cause: no accepted block insert; expire_deploys runs only from casper/src/dag.rs:BlockDagKeyValueStorage::insert, so the #213 wedge withholds exactly the event the barrier needs (the TE-4 analogue of the charter's bond/merge exemplar)
55. SC-4 [barrier-is-the-threat] H-U6-01's cost-accounting merge barrier ↔ H-U6-03's rejected-sibling-deploy hazard — shared cause: the merge's whole-block rejection; casper/src/merging.rs:NativeRelations::reject_whole_blocks is barrier in one row and hazard in the other
56. SC-5 [shared-cause] T11 SSRF dial ↔ the guard is_local_address_resolved — shared cause: the name resolution; the guard and the dial resolve the same name, so a short-TTL answer passes the check and is what the dial follows: comm/src/rp/handle_messages.rs:is_local_address_resolved
57. SC-6 [shared-cause] handshake reply pins a dispatch slot ↔ T11 SSRF dial — shared cause: one line transport.send(peer, response) dialling the attacker-claimed sender.host in comm/src/rp/handle_messages.rs:handle_protocol_handshake
58. SC-7 [barrier-is-the-threat ×2] P7 global deploy pool/RateLimiter ↔ the Kademlia global RateLimiter — shared cause: the bound **was** global not per-sender, so one sender exhausted the single window and refused honest senders — **fixed 2026-10-13**, and this row records why the Kademlia half was already keyed: shared/src/rate_limiter.rs:RateLimiter, node/src/api/grpc/mod.rs:serve_deploy, casper/src/dag.rs:MAX_POOLED_DEPLOYS
59. SC-8 [one-surface] P11 U9 DAG gauges ↔ M2 U10 /api/v1/status/operator+/metrics — shared cause: the same in-process state; observation is one barrier not three (proven by C195): casper/src/dag.rs:set_gauges, node/src/api/admin_web_api_impl.rs:operator_status, node/src/api/shard_routing.rs:proposer_health
60. SC-9 [shared-cause] T13 join's O(chain) trailing pull ↔ the join's terminal/MAX_SYNC_ATTEMPTS guard — shared cause: a peer that stops answering; request_tuple_space_roots resends with no MAX_IDLE_ROUNDS so the join! never returns and terminal is never reached: casper/src/engine/lfs_tuple_space_requester.rs:request_tuple_space_roots, casper/src/engine/node_launch.rs:apply
61. SC-10 [self-referential] the escape (parents_for_new_block_escaping) ↔ the round's LIVENESS_WINDOW clock — shared cause: the frozen tip; the escape's counter increments only on a proposal request the tap feeds, and the retirement clock is measured against the tip the deadlock freezes: casper/src/blocks/proposer/proposer.rs:create_block, block-storage/src/dag/liveness.rs:LIVENESS_WINDOW
62. SC-11 [barrier-is-the-threat] T6/P1 merge budget's own constant ↔ its firing — shared cause: the bound is hit; SearchBudgetExceeded is itself the row's consequence (SearchBudgetExceeded → ValidateError::Internal → a dropped block with no re-queue): sdk/src/dag/merging.rs:SearchBudget::NODE, casper/src/merging.rs:MergeScope::merge
63. SC-12 [one-surface] T7/P3 parent cap max-number-of-parents ↔ the fold's width fold_rejection/traverse_tree — shared cause: 'the merge scope'; the cap bounds parents, the fold walks deploy chains, so the bound and the input it targets are different surfaces: casper/src/blocks/proposer/proposer.rs:per_block_deploy_budget, sdk/src/dag/merging.rs:fold_rejection
64. SC-13 [shared-cause] T14 early expiry ↔ T15 pool never drains — shared cause: the frozen finality of #148/#149; the same stall makes latest_block_number run ahead (early expiry) and stops the inserts expire_deploys needs: casper/src/dag.rs:BlockDagKeyValueStorage::expire_deploys, block-storage/src/dag/representation.rs:latest_block_number, spec/audit/evidence/n148-results.md
65. SC-14 [one-surface] P12 exporter/importer ↔ the start-up DAG rebuild — shared cause: no persisted DAG snapshot; create_rspace_importer moves the trie on the sync surface but cannot avoid casper/src/dag.rs:BlockDagKeyValueStorage::create: node/src/runtime/node_runtime.rs:create_rspace_importer, casper/src/dag.rs:BlockDagKeyValueStorage::create

A barrier that appears in this list is not a barrier the acceptance case may lean on. The `Independence`
column of the §1 worksheets carries the same labels, and §4 records the challenges that reclassified them.

**One of these classes has since been measured, diagnosed, and fixed.** The `[self-referential]` pattern —
*a rule whose reference point is frozen by the failure it guards* — was confirmed as the cause of the N ≥ 3
finality stall, and the instance was then located **more precisely than this page first stated**: it was
not (only) that the pace bound read a tip the suppression froze, but that the guard read *every* input from
the **round's snapshot** — which is the genesis alone before the first round closes — so a validator that
had "just spoken" saw nobody else moving, its cadence was never due, and it never spoke again (§3.5).
**The class stands; the named instance is corrected.** C209 reads every input from the seen view instead,
and the stall clears. This remains the reason a barrier assessment belongs in an acceptance case, and it is
also the reason the *first* diagnosis was not enough: the class being right did not make the instance
right.

---

# 3. The acceptance checklist

This is the green-light list. Each row names its **configuration**, the **run** that produced it, the
**tree** it ran on, the **instrument**, and the **witness** — the *positive* observation that decides it.
The two rules of §0.6 make a row green only when all of those resolve. A row that cannot be made green is
a row the net has not earned.

## 3.1 Criterion 1 — Anyone can propose, and production is bounded

> **§3.1 — 4 ✅ · 1 ❌ · 0 ⬜** (A1.5 restated and closed 2026-10-11; before that 3 ✅ · 2 ❌ · 0 ⬜ as of the 2026-10-07 re-run of A1.2/A1.3, and 2 ✅ · 1 ❌ · 2 ⬜ when
> this pass closed). A1.1 passes; **A1.2 (N=5) passes** — the deploy's block finalises in 5–6 s, in all
> three attempts — and **A1.3 (N=8) fails**, which is the re-run's finding rather than the pre-fix one:
> the chain reaches 112–120 blocks with all eight validators sending and **never finalises**, because
> the layer's supporting stake is never a supermajority (400 of 800 at best, and 0 of 800 in most
> refusals). **A1.5 fails** — a deploy landing in the final heights of a quiet net is included and never
> finalised, so "every deploy finalises" is a stronger claim than A1.1 makes and the net does not meet
> it; its character is measured in the correction below and in pass 71.

| ID | Claim (as a falsifier) | Falsifier | Configuration | Run | Tree | Instrument | Witness | Status | CH |
|---|---|---|---|---|---|---|---|---|---|
| A1.1 | In a 3-validator all-live net, one deploy mints a **bounded** number of blocks **and its block finalises** | production continues without bound, or the height merely stops without the deploy finalising | node argv `--propose-on-deploy --attest-on-new-blocks` with `--autopropose` **absent** (there is no `--no-autopropose` flag — see CH-ACC-04); 3 bonded validators 100/100/50; `--epoch-length 10` | `spec/audit/evidence/n213-blocks/07af032ad-20261004T074618Z/` (this pass) + `n220-join-results.md` | `07af032ad` | `n149-sample.py` sampling `/api/last-finalized-block`, plus the block-hash union | `last-finalized-block` ≥ the deploy block's height | ✅ **pass** | CH-ACC-01, CH-ACC-02 |
| A1.2 | N=5, all live, satisfies A1.1 | as A1.1 | **the #149 sweep's:** `--propose-on-deploy` with `--autopropose` absent, 5 bonded validators at the devnet's default **equal** stakes, default epoch length — **not** A1.1's `--stakes 100,100,50 --epoch-length 10` scaled | `spec/audit/evidence/n149-blocks/bbabc39e8-20261007T121036Z/` (3 attempts, 0 void) | `bbabc39e8` | `n149-sample.py` sampling `/api/last-finalized-block`, plus the block-hash union | the deploy's block is block **1** in all three attempts and `last-finalized-block` reaches **2** within 5–6 s; 25 blocks after the deploy, one per validator | ✅ **pass** (2026-10-07) | — |
| A1.3 | N=8, all live, satisfies A1.1 | as A1.1 | as A1.2, at eight equal-stake validators | `spec/audit/evidence/n149-blocks/bbabc39e8-20261007T121036Z/` (3 attempts, 0 void) | `bbabc39e8` | as A1.2 | **`last-finalized-block` never advances, in any of the three attempts**, against 112–120 blocks and all eight validators sending. The refusal is named: *"a layer exists but its supporting stake is not a supermajority — 400 of 800"*, and over one attempt's 57 refusals the support is **0 of 800 in 24 of them**, 100 in 21, 200 in 11, 300 in 1 (534 is the supermajority) | ❌ **fail** (2026-10-07) | — |
| A1.4 | the "bounded number" is a **block** count, not a height delta | a block-hash union over the same window differs from the height delta | as A1.1 | `spec/audit/evidence/n213-blocks/07af032ad-…/R1/blocks.tsv` | `07af032ad` | the block-hash union, not `latestBlockNumber` | the union counts 28 blocks from one sender where the height reaches 28 for all three — the two are not the same quantity | ✅ **pass** | CH-ACC-02 |
| A1.5 | **no deploy on a quiet net is silently lost** — *restated 2026-10-11*: a quiet chain leaves its last few heights unfinalised (`finalized = tip − 4`, frozen) **because nothing is being produced**, and every one of them finalises as soon as the chain produces again | a block left in that band that a later, *produced* chain does not finalise — a **loss**, which is the defect this row exists to catch | 3 validators 100/100/50, `--propose-on-deploy --attest-on-new-blocks`, `--epoch-length 10`, `--no-autopropose`, 6 deploys every 12 s then quiet | `spec/audit/evidence/n214-tail-lag-blocks/9f52d84d3-20261007T114830Z/` (3 arms: R1 bootstrap-shape, R2 rotating, R3 all-to-validator-1) **and** the band's twelve arms across four runs in `spec/audit/evidence/n214-rotation-blocks/` | `9f52d84d3` and `513e2192b` | the rig itself: quiet, read the wall, deploy once more, read again — a loss would show as a block still unfinalised after the chain produced | **R1 55/51 → 59/55, R2 35/31 → 39/35, R3 29/25 → 35/31: the block at the wall's tip, unfinalised while the chain was idle, finalised in every arm once production resumed** — a positive witness, and the arm's own loss-discriminator armed and not fired | ✅ **pass** (restated 2026-10-11) | — |

A1.1 **passes**, and the pass is a re-run by this pass rather than the fixer's word: six deploys to **one**
validator — the counter-example this page published — now finalise on all three trigger patterns
(R1 24, R2 23, R3 23; `R3` was `none`, reproducibly, before the fix), with **zero** round-gate escapes.
Before the fix the same rig gave `0 of 250 (0 full partition(s) among 3 candidate(s))`; see §3.4 for the
before/after pair and §3.5 for the cause.

> **A1.5 is that residual, promoted to a row** rather than left as a note — it is criterion 1's own
> wording, so it stays in scope. The rig sends **six** deploys and counts per-deploy, and **three runs of
> it now exist, which is what makes the row honest rather than a coin toss:**
>
> | run | tree | arms that left their sixth short |
> |---|---|---|
> | the pass that raised this row | `07af032ad` | **2 of 3** (R2 and R3) |
> | **jimscarver's regression run** | `f1ca009` | **0 of 3** — all six finalise in every arm (45≤51, 29≤31, 25≤25) |
> | this pass's re-run | `513e2192b` | **1 of 3** — R3's sixth is block 19 against finality 17, plateaued |
>
> **So the residual is intermittent, and the two accounts do not conflict — they are two samples.** They
> agree wherever they overlap (R1 and R2 finalise all six in both), and the only disagreement is one arm
> of one run. His figures were recomputed here from his own committed artefacts with the same rule, and
> they reproduce exactly; nothing in his run is in dispute. What the three samples together say is that a
> quiet chain's tail is **not reliably** finalised — which is why A1.5 stays ❌ and is **not** retired by
> the green run, and equally why the red one does not establish a rate.
>
> **The mechanism is the quiet net's, not the deploy's.** Under `--no-autopropose` production stops within
> a few heights of the last deploy, and a round closes only when the *next* round's messages exist — so
> the tail of a quiet chain is unfinalisable in principle, and whether a given deploy lands inside it is
> timing. In the run where it bit, the chain sat at height 22 with finality 17 for the rest of the window
> and the gate's own lines named partitions of `100 of 250` and `50 of 250`; it was not a truncated read.
> **A1.1 asks about *one* deploy and passes; A1.5 asks about all of them and does not reliably pass.**
> Whether that is a defect to fix or a criterion to restate is a maintainer's call, and it is carried in
> the handover below.
>
> **A filing correction this re-run forced.** A1.5 (and A1.1) cited `n213-blocks/07af032ad-…`, which
> carries `R1|R2|R3` — the six-deploy shape the *old* `n213-run.sh` had. The current `n213-run.sh` is the
> kill/restart rig, so the cite named a file whose rig no longer produces its shape; the six-deploy rig is
> `n214-rotation-run.sh`, and A1.5 now cites both its runs' artefacts.

> **Correction, 2026-10-07 (C246) — "intermittent" was the wrong word.** Re-reading all **twelve arms of
> four runs**, including a fresh three-arm run on `9f52d84d3`, the tail is not a race: a quiet chain's
> greatest `finalized` is **`tip − 4`** in eleven arms and `tip − 5` in one, frozen for 37 to 110
> one-second samples. **A deploy finalises iff production ran at least 4 heights past it** — the green
> runs left their sixth deploy 4, 6 or 10 heights below the tip, and every short arm left it **3**. The
> 2/3, 0/3, 1/3, 0/3 spread is the slack, not the node.
>
> **And the band is a lag rather than a loss.** `n214-tail-lag-run.sh` reads the wall, deploys once more,
> and in every arm the block at the wall's tip — unfinalised while the chain was idle — was finalised as
> soon as the chain produced again. The mechanism is `block-storage/src/dag/finalizer.rs`'s fringe, which
> needs a candidate whose parents reach *beyond* the next layer, and `proposer.rs`'s round gate, whose
> escape is evaluated only when something *asks* the node to propose.
>
> **So what fails is the criterion's own wording.** "Every consecutive deploy on a quiet net is finalised"
> measures the net *while it is idle*, and nothing can improve during an idle period. The property that
> matters — no deploy is silently lost — is met. Restating A1.5 to say so was recorded here as
> **proposed and not taken**, a maintainer's call; **it was taken on 2026-10-11** and the row is
> restated in place above, with the lag arms as its witness. The full account is
> `spec/audit/evidence/n214-rotation-results.md` plus pass 71.
>
> **And one demonstration stays owed, named rather than implied** (the account's own *"what is not yet
> captured"*): no single transcript yet holds a **deploy** in the band and rescues it — the rigs
> produced slack 4 in all three arms, where the demonstration wants an arm at 3. What the row's ✅
> rests on is the band's measurement plus the rescue of **the block at the wall's tip**, which is the
> same property observed one step earlier; the deploy-in-the-band transcript would be a second witness
> rather than a different claim.

## 3.2 Criterion 2 — Validators can be dropped and joined without risk

> **§3.2 — 5 ✅ · 0 ❌ · 0 ⬜.** A2.1, A2.3, A2.4 and A2.5 pass, and **A2.2 passes on the live testnet**:
> the rejoin failure found at `49337ee92` was fixed in `7d5c22a9c` and re-run on the testnet at `777953de6`
> ([#223](https://github.com/rhovision-org/rholo/issues/223)); see the note below. A2.5's ✅ is a
> **rig** result too, and the page keeps that distinction rather than blurring it: see its note.

| ID | Claim (as a falsifier) | Falsifier | Configuration | Run | Tree | Instrument | Witness | Status | CH |
|---|---|---|---|---|---|---|---|---|---|
| A2.1 | kill one of three at 100/100/50 on a live net; **the survivors finalise past the kill** | the survivors stop producing, or produce but stop finalising | 3 validators 100/100/50, and **the 50-stake validator is the one killed** — killing a 100-stake validator leaves 150 of 250 = 60 %, which is not `> ⅔`, so finality stopping there is the quorum and not a defect; node argv `--propose-on-deploy --attest-on-new-blocks`, `--autopropose` absent; `--epoch-length 10` | `spec/audit/evidence/n213-blocks/07af032ad-20261004T083816Z/` | `07af032ad` | the deploy's own block found by the union, plus `last-finalized-block` and `is-finalized` | **the block carrying a deploy submitted after the kill finalises** — block 14 at a tip of 13, finality 14 | ✅ **pass** | — |
| A2.2 | the killed validator **restarts and rejoins**; production and finality resume with **no operator action** | they do not resume | as A2.1 | live testnet, 2026-10-04 — transcript in [#223](https://github.com/rhovision-org/rholo/issues/223#issuecomment-5980123339) | `777953de6` (fix `7d5c22a9c`) | `last-finalized-block` per node, the rejoiner's own `proposed and added block` lines, and its `missing justification` count | killed at h=45 (f=41); survivors reached **h=79, f=75**; after a plain `start` the rejoiner was **level with the tip within 15 s**, then **proposed blocks #92–#94** when deployed to, with finality trailing at 4 and **zero `missing justification`** errors | ✅ **pass on a live net** | C211 |
| A2.3 | a deploy **accepted while the validator is absent** is included once the survivors can finalise | the deploy pool cannot be drained | as A2.1 | `spec/audit/evidence/n213-blocks/07af032ad-20261004T083816Z/` | `07af032ad` | as A2.1 | the deploy sent while `validator-2` was stopped is **included and its block finalises** (block 14) | ✅ **pass** | — |
| A2.4 | a **new validator bonds onto a running net** and produces | the bond never takes effect, or the new validator never proposes | a 4th validator is admitted, funded with `trust`, then `bond`s against the live net | `spec/audit/evidence/n220-join-results.md` | `9a75d45` (author's run; **read, not re-run** by this pass) | `rho:pos` bond state + the newcomer's producer key + the deploy's `is-finalized` | the bond lands in a boundary block, the newcomer produces, and its deploy's block finalises | ✅ **pass** (healthy joiner — see the caveat below) | CH-ACC-06 |
| A2.5 | a validator can **leave safely**: `withdraw` → epoch boundary → quarantine → payout | stake is stuck, or the payout never lands | 3 validators 100/100/50, `--epoch-length 10 --quarantine-length 20` | `spec/audit/evidence/n220-leave-results.md` | `a077a87e4` | the active set (`/api/v1/pos` `activeValidators`) **and the withdrawing validator's vault**, each read with the block its datum was produced in, and each probe recording its own deploy's status | the withdrawal stages (`deadline=30 blocks_remaining=20`), the boundary deactivates it (**active 3 → 2**), the entry clears, and **the payout lands: vault `99998098` → `99998964`, +866, blocks 13 → 34** | ✅ **pass** (a rig run — see the note) | CH-U6-09 |

**A2.5 has now been run, and it passes — on a rig.** `n220-leave-run.sh` (tree `a077a87e4`) shows the whole
leave: the `withdraw` stages with its own arithmetic (`deadline=30 blocks_remaining=20`), the epoch
boundary deactivates the validator (`activeValidators` 3 → 2), the pending entry clears, and **the payout
lands** — the withdrawing validator's vault moves `99998098` → `99998964`, **+866**, read at block 13 and
block 34 around a deadline of 30, with each probe recording its own deploy's status (`ok 13`, `ok 34`).
That is `close_block`'s step 3: `payable = bond + committed_reward` into `vault_address(validator)`, the
50-unit bond plus 816 of reward. Between the two reads validator 2 makes no deploy of its own — every
deploy in the window is signed by the **genesis** key — so nothing but the payout can move the balance.

**The two earlier readings of this row were both wrong, and both were the instrument.** The first reported
the payout unobserved and wondered whether `/api/v1/pos` had a read-path gap; it does not —
`pos:pending_withdrawers` holds a request only **until the next epoch boundary**, where `close_block` moves
it into `pos:withdrawers` (`rholang/src/native_state.rs:95-98`), so the window is one block at
`--epoch-length 10` and the rig's 15-second sleep simply missed it. The second read `0` for an account
funded 100,000,000 because **the probe reused its channel**: `listen-data-at-name` returns what is already
at the name and waits only if there is nothing, so the second read was handed the first read's datum —
provably, since the captured message's own `block_number` was 5, before the fund (7) and the withdraw (14).
**Seven instrument faults in all, every one of them mine and none of them the chain's**; they are listed in
`n220-leave-results.md`, because each is a way a probe can lie.

So **CH-U6-09 is settled against the worksheet** — the read path's own `deadline` and `blocks_remaining`
*are* the recorded `quarantine_length + divisor·(1 + block_number/divisor)`, not the inversion the worksheet
stated — and A2.5 is ✅ **as a rig run**: one host, one complete mesh, a shrunk epoch and quarantine. It
does not say a validator can leave safely on a **live** net, which is the distinction A2.2 is held to.

**A2.4 has now been run, and it passes** — the first live exercise of the C207 path (`bond`/`withdraw`/
`trust`/`delegate` were no-ops on any network that merges until 2026-10-03). `n220-join-run.sh` admits a
fourth key onto a running `--no-autopropose` net, funds it, bonds it: the bond lands in the boundary
block, the newcomer produces 14 of 84 blocks, and its own deploy's block finalises. **The caveat it first carried — a silent joiner — has since been run.**
`n220-silent-join-run.sh` bonds key 3 through `join-admit`/`join-bond` **without ever creating its
container** (both deploys reach the bootstrap; deploys are not gossiped), so a validator sits in the bond
pool that speaks to nobody. The three live validators hold 250 of the pool's 300 and **finalise past the
bond (7 → 10)**, quiet afterwards — the silent one is retired from the live partition by `LIVENESS_WINDOW`,
which is what Law 52b's clauses are about. **A silent bonded validator is not by itself a wedge.** It is a
*rig* result — one host, complete mesh, one silent validator of four — so it does not speak to a live net,
which is #223's question and is measured there. See `n220-silent-join-results.md`.

> **A2.2 was ⬜, not ✅ — the page's own lesson turned on itself.** Its earlier ✅ came from a **rig** run
> (`n213-run.sh` case (b): `stop`, `start`, a deploy finalises). Jim then ran the checklist on the **live
> testnet** at tree `49337ee92` and the rejoiner does not rejoin: the restarted node meshes and retrieves
> blocks (246 retrieval lines) but is capped at the height it died at — `h=35` while the survivors reached
> `h=48` — with exactly two `block summary failed: missing justification` errors and no recovery.
> **A rig pass is not what the row claims**, which is the distinction this page has applied to everyone
> else's evidence and had not applied to its own. See
> [#223](https://github.com/rhovision-org/rholo/issues/223); the chain-level consequences are
> nil — a rejoin **liveness** failure, not a safety one. **A2.4's ✅ is a rig result too** and the live run
> did not reach it; that is recorded as provenance, not as a doubt, because nothing has falsified it.
>
> **It has since been fixed and re-run on the same live testnet**, which is why the row is ✅ and the ⬜
> above is history rather than status. On the testnet at `777953de6` (three validators 100/100/50,
> `--no-autopropose`) the 50 was killed at h=45, the survivors went on to h=79 and f=75, and after a plain
> `start` the returner was **level with the tip within 15 s**, where the unfixed build had stayed 13 blocks
> behind indefinitely; it then **proposed blocks #92–#94** when deploys were addressed to it, finality
> trailing at 4, with **zero `missing justification`** errors
> ([transcript](https://github.com/rhovision-org/rholo/issues/223#issuecomment-5980123339)). The
> testnet was then rebuilt as four equal validators of 250, and stopping **A** (the genesis master) there
> left the other three finalising (f 65 → 90 while the height ran 69 → 94); A rejoined to the tip in under
> 20 s (`docs/src/node/testnet.md`). The distinction this note draws still holds: the ✅ is a live-net ✅,
> and the rig's ✅ was never the evidence for it.
>
> **This ✅ was first recorded in `4c51d2c` and lost in a later merge of `dev` (`4f2d996`)**, which is why
> the page read ⬜ after the live run had passed.

**What moved after this page was written, in order.** `#215` (2026-10-03 14:14) gave the round gate a
wall-clock escape — *"needs a clock, not just a supply of attempts"* — but that was on a tree this page's
first audit did not have. Then **C209** (2026-10-04, `f13e045c4`) moved the attestation guard's *every*
input onto `latest_msgs`, bounded by `ATTESTATION_HORIZON`, after a live run refuted its first cut — the
run being the counter-example §3.4 published. Then **C210** (`d960a0f18`) armed one `Automatic` retry just
past the stall bound so a refused propose on a quiet net is not left waiting. **#219 merged at
`07af032ad`**, and this pass re-ran the arms rather than reading them: the three deploy patterns and the
three kill/restart cases now pass, with the witnesses in §3.1 and §3.2. The rows above changed from ❌ to
✅ on that evidence — and one **residual** (§3.1) says "every deploy finalises" is stronger than any row
here claims.

## 3.3 Criterion 3 — Every attack vector is handled

> **§3.3 — a census, not a verdict.** No cell in this section carries a pass or fail glyph. The
> vocabulary here is `counted` / `not counted`.

Issue #214 is explicit that this is *not* a pass/fail item, so it is not presented as one. The instrument
is a **rate and a class census**: register rows opened and closed per window, by class, measured **against
a bounded-adversary statement**. That statement **exists** — [`adversary.md`](adversary.md), *The bounded
adversary this testnet claims to handle* — so every `Against` cell now names it rather than reading
`absent`, and its rows were tested **apriori** by the wargame in that page
([Table E](adversary.md#table-e--the-statement-test): two rows are *too narrow* — `operator error during
recovery` and `key compromise` — which is a finding about the **statement**, not the node). The section
still cannot go green by construction, which is the honest shape of criterion 3.

| Window (ISO) | Register commits | Rows closed | By class | Against |
|---|---|---|---|---|
| 2026-W39 | 17 | `not counted` | `not counted` (see below) | `adversary.md §The census` |
| 2026-W40 | 62 | `not counted` | `not counted` (see below) | `adversary.md §The census` |

The class column reads `not counted` for a measured reason, recounted 2026-10-14: **158 of the 364
`done` rows name a law or class; 206 carry an empty classification cell** (`spec/AUDIT.md`;
`spec/findings.tsv`). The register
therefore reports **volume, not class**. The wargame's ledger now supplies a **class vocabulary** for the
rows it adds (`unauth-actuation`, `key-at-rest`, `disclosure`, …), so the fill rate is measurable going
forward even though the 206 historical rows are not retro-migrated.

**Where the census stands, 2026-10-14.** The two windows above are the ones this pass measured. What has
changed since is the close-out of the wargame's falsifiers: the register grew from **250 rows at the end
of W40 to 365** today, and every one of the wargame's 97 Table A rows now carries a taken falsifier, a
gate citation, an arm run or a recorded decision — its page reads `owed:` on no row at all (pass record
§128–§136; register C313–C330).

**And `Rows closed` stays `not counted`, which a check confirms rather than assumes.**
`git log --date=format:'%G-W%V' -- spec/findings.tsv` shows the file arriving with **201 rows already
`done`**: the register was authored retroactively, so a row's `done` state records the day it was
*written up*, not the day its defect was closed — and a rate read off it would measure the writing, not
the work. The class column's fill rate is the figure that is measurable here, and it has moved from 36 of
242 to **158 of 364**.

The rate is the reason the gate is up: the register took **C195 through C207 in a single window**, and
C207 alone meant the join path had **never worked on a network that merges**.

---

## 3.4 The re-probe, 2026-10-03

The rows above were first written on `1e5a64ed4` from an unfetched clone (§0.9). This is the run that
replaces that reading, on the current tip.

**Tree `f9d36b9c4`** · image `sha256:eb2c2308…` · node binary `sha256:9659a2bc…`
Protocol: `spec/audit/evidence/n214-preregistration.md`. Transcript: `n214-results.md` and
`n214-blocks/f9d36b9c4-20261003T154406Z/`. Rig: three validators at 100/100/50, `--fresh`,
`--propose-on-deploy` with `--autopropose` absent, `--attest-on-new-blocks`, `--epoch-length 10`.

| arm | what it did | reading | verdict |
|---|---|---|---|
| **A** — criterion 1 | two deploys, 90 s apart, all three live throughout | blocks **6** then **1**; deploy blocks numbered 1 and 3; heights 1 → 4; **no sample reports a finalised block — 693 samples, all `none`** | ❌ **fail** |
| **B** — criterion 2 | one pre-kill deploy; stop `v2` (the 50-stake validator); 120 s; restart; 90 s | survivors alive throughout; **no finalised block at any point — before the kill, in the kill window, or after the restart** | ❌ **fail** |

**What arm A settles.** Production is bounded, all validators are live, and the deploy still does not
finalise. That is criterion 1's failure stated in its own terms: the witness is the deploy finalising, and
it does not.

**What arm B settles, and what it cannot.** The net finalises nothing *with three validators live*, so the
kill experiment has no finality to remove. Criterion 2 fails **upstream of the wedge** — this run does not
reproduce #213's specific shape (survivors minting one block each and then stopping); it finds a state
that is worse to reason about and simpler to state: **nothing finalises at all.**

> **A re-run on `ee1e204b3` (2026-10-04) shows arm B cannot discriminate at all — there is a second
> reason, and it survives every fix.** That arm deploys once *before* the kill and nothing inside its own
> window, and on a `--no-autopropose --propose-on-deploy` net an idle chain produces no block, so finality
> has nothing to advance onto whatever the code does. Re-run today, the chain sat at **height 6 /
> finality 2** from the kill through the whole 120 s window and the 90 s recovery, while the *same* rig's
> arm A reached **15/11**. So a `fail` here is **never criterion-2 evidence on its own**: it fires on a
> healthy chain for the same reason it fires on a stalled one. Nothing in §3.2 rested on it — those ✅s
> come from `n213-run.sh`, which deploys *inside* the kill window, and the ⬜ from the live net — but the
> distinction was not written down until the re-run forced it. See
> `spec/audit/evidence/n214-results.md`, "Re-run, 2026-10-04".

**The contradiction, closed.** #214 records criterion 1 *passing* at N=3 on `0c6c65979` — "finality reached
3" — while on `f9d36b9c4`, with the configuration the issue names, nothing finalised in either arm. **It is
settled: `0c6c65979` was right and `f9d36b9c4` was pre-fix.** The N=3 pass reproduces once the attestation
guard reads the seen view (C209) — re-verified by this pass on `07af032ad`, where six deploys to a single
validator finalise (24/23/23) against `none`, reproducibly, before. So criterion 1 did not regress between
the two trees; it was failing in the window this page measured, for the mechanism in §3.5. The `n149`
result (*finality never advances at N ≥ 3*) and the A/B on #213 (*"the stall is independent of the
escape"*) were both measurements of the same pre-fix behaviour — which is why the escape, correctly
fixed in #215, did not move them.

**A claim of mine that was wrong, kept because it cost a run.** The first attempt at this rig omitted
`--attest-on-new-blocks`, and this page attributed the result to that omission on the reading that the
positive flag is a clap `bool` defaulting **false**. It is not. The merged default is **true**
(`node/src/configuration/defaults.conf:16`) and the CLI flag only ever *adds* `true`, so **passing it is
optional and its absence cannot stop a node attesting**. That is why the two runs gave identical
results: the first was never void, it is a second agreeing attempt — kept in
`spec/audit/evidence/n214-repeat-attestation-on/` — renamed, because the old name misstated what it is. The correction is
#219's author's (`d960a0f18`), and it is the second time in this pass that a negative reading was mine and
not the code's (see §0.9).

**Limits.** One tree, one host, N=3 only — where a reading is single-attempt, the row says so. A1.2 (N=5)
and A1.3 (N=8) are untested. A single attempt is not a rate.

> **Correction, 2026-10-07 — the untested arms are a *budget*, not a capacity.** A1.2 and A1.3 read as
> though N=5 and N=8 could not be run here. They can: `tools/devnet.sh` takes up to **eight** validators
> (`MAX_VALIDATORS = 8`), at a default 4 GiB ceiling each, and N=2/3/5/8 have been run on this host
> before (the #149 attestation sweep — `n149-sweep-run.sh` carries `NS="2 3 5 8"`). The arms were left
> ⬜ by a **sweep-budget** decision, in the pre-registration's own words: *"the sweep budget is not a
> sweep"*. The other blocker this page once named, #153's parent bound, **closed on 2026-10-01**. So
> these two rows are runnable here and want deciding on, not provisioning for.

## 3.5 The cause of the stall, and its correction

> **Corrected 2026-10-04.** The diagnosis below named the right **component** and the wrong **instance**.
> The landed remedy is not the one this page proposed, and the locating evidence came from a live run: —
> *"deploys sent to a validator that did not sign the genesis never finalised. Before the first round
> closes the round snapshot is the genesis alone, a message from its signer at the tip, so read from the
> parents the signer had just spoken and nobody else was moving: its cadence was never due, its quorum
> never reachable, and it never spoke again"* (#219, `f13e045c4`).
>
> **The cause, corrected:** the guard read **every** input from the **round's snapshot** — which before the
> first round closes is the genesis alone — rather than from what the node has seen. The fix reads
> `latest_msgs` for all of them, bounded by `ATTESTATION_HORIZON = 3 × LIVENESS_WINDOW` so the C171 storm
> cannot return (**C209**), and arms one `Automatic` retry just past the stall bound when a propose is
> refused (**C210**). **Verified independently by this pass** — not taken from the fixer's report: see
> §3.1's rows and `spec/audit/evidence/n213-blocks/07af032ad-…/`.

**The first diagnosis, kept because it is half the story and because this page's own rule is that a
superseded claim stands beside its correction.**

**The finality gate is not what refuses. The attestation guard is**, and it holds a fixpoint.

`attestation_suppressed` is `!(new_state_transition || cadence_due)` under `paced && quorum_reachable`
(`casper/src/blocks/proposer/proposer.rs`). Once the deploy's round is complete, no immediate parent
carries a deploy — so `new_state_transition` is false — and no validator is behind the tip, so
`cadence_due` is false. `cadence_due` is measured as *heights behind the tip*, and **the tip is frozen by
the suppression itself**: a caught-up node can never fall behind a tip that only its own attestation could
advance. Every validator therefore suppresses, the round never closes, and the fringe — which needs the
**next** round's snapshot before it can advance at all — never moves. **The chain stops exactly one round
short of what finality requires**, which is why it produces blocks and finalises none.

This is the `[self-referential]` shape §2.5 names: *a rule that gates progress on the progress it gates.*

**Three independent confirmations.**

1. **The gate's own log** (it renders its refusal, so it cannot disagree with the decision it explains):
   `finality did not advance at tip 2: … 0 of 250 (0 full partition(s) among 3 candidate(s))` — three
   candidate senders, and **none seen by the whole partition** — with **zero** `round gate escaped` lines.
   The validators were *suppressed*, never round-blocked, which is why #213's A/B found the stall
   independent of the escape. Captured in `spec/audit/evidence/n214-blocks/…/stall-lines.txt`.
2. **Reproduced unchanged on `ed65317c5`** (which carries #216 = the C208 fix): identical block counts,
   no finality, identical stall lines. C208 does not touch this mechanism.
3. **The rotation experiment** (`spec/audit/evidence/n214-rotation-results.md`): six deploys addressed to
   **one** validator — whichever one — leave finality at `none`; the same six deploying **in rotation**
   across all three take it to **finalised 3**. Jim's observation on #214, reproduced as a measurement.

The escape cannot rescue it, and the experiment measures why: with a single trigger point the gate climbs
only to `100 of 250 (1 full partition among 3)` — exactly one validator's stake, which is the asymmetric
parent set an escape produces — short of the 167 needed. **`ROUND_STALL_ESCAPE` moves the number from 0
to 100; it cannot reach a supermajority.**

**One correction to #215's premise, which this pass found and the register had not.** In a quiescent chain
with `--autopropose` omitted **there is no attempt supply at all**: the taps fire once per remote height
and the timer is off. So a rule consulted only inside `create_block` cannot fire — *including*
`round_escape_owed` itself. #215's fix has a clock but still needs an attempt to consult it; its own commit
message names this trap half a layer down ("the bound was never reached, because it counts *attempts*, and
attempts are only supplied by something that asks this node to propose") without applying it to the fix.

**Consequence for the checklist.** §3.1's rows now **pass**, on a re-run this pass performed rather than on
the fixer's report, and the row that was ❌ is ✅ for a reason that survived the correction: the deploy
finalises because the round it needs now closes. The falsifier #213 named is met on the deploy arms — with
one **residual** recorded in §3.1 (the *last* of several consecutive deploys is included and may not
finalise, because production stops before finality catches up) which is a stronger claim than the row makes
and belongs on the issue rather than hidden here.

---

# 4. The challenge register

A challenge is **a doubt that names the observation which would settle it**. A doubt that cannot name one
is not a challenge — it is filed as an `owes` on the register instead. A challenge is resolved by an
**artefact**, never by agreement: `refuted` requires a cited artefact, `upheld` requires an artefact or a
demonstration that the claim is undecidable from its own evidence, and `open` requires a **named run**
that would settle it. An upheld challenge is not a deletion: it **downgrades** the contested row and the
row keeps its original claim beside the challenge, so a later reader can see what was believed and why.

## 4.1 Acceptance-level challenges

These six attack this page's own claims, not the node's internals. They were raised directly against the
acceptance rows in §3.

> **CH-ACC-01 — "Criterion 1 passes" is satisfied by the defect that fails criterion 2.** · class
> **witness** · contests **A1.1**.
> **The claim contested.** *"2 blocks minted per node (6 total), then flat for 200 s; finality reached,
> so the deploy's block finalised."*
> **The doubt.** The criterion has two halves — *bounded production* and *the deploy finalises* — and the
> #213 wedge satisfies the first **by construction**: a chain sealed at a height has bounded production
> trivially. As written, A1.1 cannot distinguish "bounded because the round rule worked" from "bounded
> because the chain is wedged". The evidence cannot decide it: the three further deploys #213 measured
> were submitted *after a kill*, so they say nothing about whether an all-live chain would accept a
> second deploy.
> **What would settle it.** The same tree, configuration and genesis, with **two marks**: deploy, wait to
> finality, deploy **again** — the second must finalise too. One `tools/probe-blocks-per-deploy.sh` run
> with two marks.
> **Resolution — `upheld`.** The claim is not decidable from the run that produced it.
> **If upheld.** A1.1 splits into **A1.1a** (bounded) and **A1.1b** (*still live after the bound*), and
> A1.1a alone can never be `pass`.

> **CH-ACC-02 — the instrument counts heights, and the claim counts blocks.** · class **instrument** ·
> contests **A1.1, A1.4**.
> **The doubt.** `tools/probe-blocks-per-deploy.sh` samples **`latestBlockNumber`** from `/api/status`
> and its own header calls the result "blocks per deploy". Those are different quantities, and this
> repository has been bitten by the substitution twice: a block's number is derived from its justification
> set (`max(justification height) + 1`), so **several blocks share a height**, and
> `spec/audit/evidence/n149-results.md` needed a separate block-hash union (`n149-blocks/`) to get a true
> count. The probe's headline number is a **height delta** and under-reports by roughly N.
> **What would settle it.** Re-derive the count from a block-hash union and state which quantity the cell
> holds.
> **Resolution — `upheld`.** The instrument does not measure the quantity the claim names.
> **If upheld.** A1.4 becomes a defect against the **tool**, and A1.1's number is ⬜ until the tool counts
> blocks.

> **CH-ACC-03 — the tree the criterion-1 pass cites is not in this repository.** · class **tree** ·
> contests **A1.1**.
> **The doubt.** Issue #214 cites the pass on `0c6c65979`. `git cat-file -t 0c6c65979` →
> `fatal: Not a valid object name`. The run is not reproducible from this tree, so under the evidence
> rule it is *reported*, not *demonstrated*.
> **Resolution — `refuted`, 2026-10-03 (after this page was first published).** The observation was an
> artifact of an **unfetched clone**, not a fact about the repository. `0c6c65979` resolves — it is the
> merge of PR #212, dated 2026-10-03 11:11 — and was on `origin/dev` when this audit began. An earlier
> edition of this page carried the challenge as `upheld` and used it in §0.2 and §3.1; the withdrawal is
> recorded in §0.9 rather than edited away, per this register's own rule.
> **If upheld.** *(Superseded — the challenge is withdrawn.)* The row's other two refusals stood on their
> own at the time: no committed run artefact existed, and the instrument reads a height, not a block count.
> Both have since been answered — run artefacts exist (`n213-blocks/`, `n220-join-blocks/`), and the
> block-hash union is what the rows now cite.

> **CH-ACC-04 — the configuration names a flag the node does not have.** · class **configuration** ·
> contests every `Configuration` cell in §3.1.
> **The doubt.** #214's configuration string reads `--no-autopropose`. There is **no such flag on the
> node**: `docs/src/node/operating.md` and `docs/src/node/testnet.md` both say you **omit** `--autopropose`
> to disable it, and `tools/devnet.sh` accepts `--no-autopropose` only because that is *the rig's* CLI.
> A configuration cell transcribed from the issue would not start a node — and an unstartable
> configuration is a run that cannot exist.
> **What would settle it.** Start one node with the cell's literal string.
> **Resolution — `upheld`.** Every §3.1 cell spells the **omission** as node argv.

> **CH-ACC-05 — the acceptance bar contains no safety criterion.** · class **inference** · contests the
> document's scope.
> **The doubt.** All three criteria measure **liveness, membership and process**. None measures whether
> two validators can each finalise a *conflicting* block. The safety direction is explicitly unguarded —
> there is no finality undo, no reorg depth beyond the fringe, no light client, and the on-chain
> certificate has a published blind spot. A net could satisfy 1, 2 and 3 while TE-2 is wholly
> unmitigated, and #214's own "no public claim about viability until 1 and 2 pass" would inherit the gap.
> **Resolution — `partially upheld`.** This is a scope decision, and the scope is #214's. TE-2 stays out
> of the green-light list but is a **top event** in §2 with a promotion trigger of its own ("any claim of
> safety, or the first contract holding value"), and §0 states the limit **in the sentence that makes the
> claim**.
> **If upheld further.** A fourth criterion is proposed to #214 as a separate issue rather than folded in
> here.

> **CH-ACC-06 — criterion 2(c) has never been run, and cannot be while 2(a) fails.** · class
> **inference** · contests **A2.4**.
> **The doubt.** A bond takes effect on a merge. A wedged chain merges nothing (#213). So the join path —
> the one C207 made functional on 2026-10-03 for the first time in the node's history — has **never been
> exercised on a live net**, and criterion 2(a) blocks the run that would. The criterion cannot be
> measured from where it stands.
> **Resolution — `superseded`, 2026-10-04.** The challenge was correct when it was raised and the
> prerequisite has since been met: 2(a) passes (§3.2) and the join has now been run
> (`n220-join-results.md`) — a validator is bonded onto a running net, produces, and its deploy finalises.
> The challenge is kept rather than deleted, because the shape it names has **not** gone away: a
> **silent** joiner is still unrun, and that is the case this challenge's reasoning actually points at.

## 4.2 Node-level challenges

The challenger raised **75** challenges against the ten study-node worksheets: **70 upheld, 5 refuted**.
The table gives each challenge's id, study node, class, the row it contests, the verdict, and what the
adjudicator established by read-only observation on tree `1e5a64ed4`. **Every upheld challenge has been
applied to §1** (2026-10-04); ⚠ now marks a §1 row that still carries an element the adjudication left
**unestablished** — an `unmeasured` barrier, a contested consequence, an owed run — not a row nobody has
read.

**A note on the line numbers in this table.** A citation of `spec/audit/passes.md` gives the line as it
stood on the day of adjudication, and **that file gains rows every time a pass is added**, so those
numbers drift: **C111's is `:4063` here and `:4064` in the corpus**, because the C211 correction (§65)
inserted a line above it. The body of this page carries the current number; this column carries the
adjudicator's. It is the same failure the worksheet was corrected for, one level down — and it is why the
cells above now prefer a named symbol (`::untrust`) to a bare range.

| CH | node | class | contests | verdict | what the adjudicator established |
|---|---|---|---|---|---|
| CH-U1-01 | U1 | witness | H-U1-01 (deviation + cause) and its Table-B barrier evidence | ❗ upheld | H-U1-01 is contested and needs re-derivation against HEAD. Its cause anchor (`attest_warranted`'s `height > last_attested_height` single-height rule) |
| CH-U1-02 | U1 | witness | H-U1-01 deviation wording | ❗ upheld | Reword H-U1-01's deviation clause (sheets/s1-U1.md:11): "no block is built at all" → "no block is built after the first round rests — the chain seals |
| CH-U1-03 | U1 | witness | H-U1-03 (consequence -> Theta(N^2) DAG residency) | ❗ upheld | Downgrade H-U1-03's Table-B evidence cell (`spec/audit/evidence/n148-results.md`) to `unmeasured` for the escape mechanism — the cited run is a non-wi |
| CH-U1-04 | U1 | witness | Table-B row H-U1-07 (barrier + its n148-results.md evidence) | ❗ upheld | Downgrade H-U1-07's evidence cell to `unmeasured` (contested) and correct its cadence; the barrier code stays. The row cites spec/audit/evidence/n148- |
| CH-U1-05 | U1 | inference | H-U1-04 (deviation 're-attached on every block' + consequenc | ❗ upheld | Downgrade H-U1-04 to contested: its deviation "re-attached on every block" is a conditional symptom of H-U1-01/03's frozen finality (not a per-block b |
| CH-U1-06 | U1 | instrument | H-U1-05 cause (propose_effect calls send_block_hash(...).awa | ❗ upheld | The cause-cell wording is defective but the deviation stands. Re-anchor H-U1-05's cause from "discards the result" (a Result the signature does not re |
| CH-U1-07 | U1 | tree | Table-B H-U1-06 independence label [one-surface] | ❗ upheld | Relabel H-U1-06's independence cell from `[one-surface]` to `[shared-cause]` — the s1 reason ("read at the same process boundary as the trigger it fee |
| CH-U1-08 | U1 | inference | Table-B completeness (H-U1-02 and H-U1-05) | ❗ upheld | Table B was incomplete. Assign `[one-surface]` to H-U1-02's barrier (proposer-local veto + cross-node receiver `sequence_number`; the escape is the se |
| CH-U2-01 | U2 | witness | s1 Table B, H-U2-01 barrier column (Preventive barrier + Evi | ❗ upheld | Correct the H-U2-01 barrier cell: strike `cadence_due` as a preventive barrier — it is short-circuited to `false` by the deviation's own premise (dumm |
| CH-U2-02 | U2 | configuration | s1 Table A, H-U2-01 row (whole) | ❗ upheld | Annotate H-U2-01 as configuration-gated: the deviation requires `--autopropose` AND the dev-mode deployer key — neither a node default — so it cannot |
| CH-U2-03 | U2 | witness | s1 Table B, H-U2-03 Evidence cell | ❗ upheld | Re-anchor H-U2-03's barrier-evidence cell. Strike the n148-results.md citation as guard-live evidence (it is the autopropose arm, and its "production |
| CH-U2-04 | U2 | inference | s1 Guide words — vacuity of PART OF | ❗ upheld | PART OF is refuted as a vacuity label on s1-U2: there IS a reachable proper-subset close, via LIVENESS_WINDOW retirement. The U2 vacuity list must dro |
| CH-U2-05 | U2 | tree | s1 Guide words — vacuity of REVERSE | ❗ upheld | Downgrade s1's REVERSE vacuity note (row vacuous: REVERSE) to contested: its stated justification is false as written on both counts. (a) the stalenes |
| CH-U2-06 | U2 | inference | s1 Table A, H-U2-02 consequence | ❗ upheld | Downgrade H-U2-02's consequence: the deviation (mark-before-send) stands, but "never produced → TE-1" is not entailed by a single drop — the mark is p |
| CH-U2-07 | U2 | inference | s1 Table B, H-U2-02 Independence column | ❌ refuted | No change to the H-U2-02 row or its label: the barrier is real — the `answered` map, the tap's burst bound (node_runtime.rs:1663, :1667-1686) — and `[ |
| CH-U3-01 | U3 | witness | H-U3-03 Table A deviation | ❗ upheld | Downgrade the H-U3-03 Table A deviation to contested/unmeasured: the justification-set locution ("carried in the justification set … becomes an extra |
| CH-U3-02 | U3 | witness | H-U3-04 Table B evidence | ❌ refuted | No change to H-U3-04 Table B evidence: the cited test is GREEN and not ignored, so it is a valid in-process witness for the escaping-parent-set barrie |
| CH-U3-03 | U3 | inference | guide-word vacuity (EARLY, BEFORE) | ❗ upheld | Downgrade the U3 guide-word vacuity (s1-U3.md:8) from `vacuous` to `contested`. EARLY is not vacuous: add the referent as a deviation — the round boun |
| CH-U3-04 | U3 | instrument | H-U3-01 cause anchor | ❌ refuted | No change to H-U3-01: the cause row stands with its A1.1/A2.2 marks and TE-1; the `0/0` figure is grounded, not uncited. Editorial follow-up only — re |
| CH-U3-05 | U3 | instrument | H-U3-05 law-15 appeal | ❗ upheld | Downgrade H-U3-05's law-15 appeal to contested and re-word the cause: law 15 is per-sender height monotonicity (under the ingress rules) plus construc |
| CH-U3-06 | U3 | tree | H-U3-02 Table A 'deviation' | ❗ upheld | Re-classify H-U3-02 in Table A: it is a design constraint (documented safety–liveness trade, Law 52 a/b + the §59 decision), not a "deviation from int |
| CH-U3-07 | U3 | instrument | H-U3-04 cause anchor | ❗ upheld | Downgrade the H-U3-04 cause anchor to contested and reword: the clause "the tip cannot advance while the round is open" is refuted. It becomes the con |
| CH-U3-08 | U3 | inference | H-U3-03 Table B independence | ❗ upheld | Relabel H-U3-03's Table-B barrier from `[shared-cause]` (with U2) to `[one-surface]`: the filter bounds the justification-set surface only, and the re |
| CH-U4-01 | U4 | instrument | H-U4-05 cause anchor | ❗ upheld | Re-point H-U4-05's cause anchor to `spec/audit/evidence/n117-after-fix-results.md:45-49` (corroborated by `n117-heap-profile-results.md:99`); add both |
| CH-U4-02 | U4 | witness | H-U4-01 barrier | ❗ upheld | Downgrade H-U4-01's evidence cell to misattributed: the cited `rejection_options_are_bounded_on_a_directed_shape` is H-U4-05's output-sensitivity gate |
| CH-U4-03 | U4 | inference | H-U4-03 barrier | ❗ upheld | Reclassify H-U4-03's barrier as agreement-only ([one-surface], matching s2's Table B): the barrier stays but carries no independent completeness check |
| CH-U4-04 | U4 | instrument | Sheet 'Anchor correction' | ❗ upheld | The s2-U4.md line-35 sentence "`casper/src/dag.rs` would be a sibling path that is absent" is false — the path is present (tracked, 1747 lines, declar |
| CH-U4-05 | U4 | inference | H-U4-01 independence label | ❗ upheld | Relabel H-U4-01's independence cell from `[self-referential]` to `[barrier-is-the-threat]`. The barrier itself (the budget applied before each unit, s |
| CH-U4-06 | U4 | inference | H-U4-04 & H-U4-06 barriers | ❗ upheld | Relabel the H-U4-04 and H-U4-06 Table-B barrier independence from `[one-surface]` to `[unhoused]` (and drop/reword the cap in the barrier cell): `max- |
| CH-U4-07 | U4 | instrument | H-U4-06 cause anchor | ❗ upheld | Amend the H-U4-06 cause anchor (s1-U4.md line 16): keep casper/src/merging.rs:MergeScope::merge for compute_relation_map_for_merge_set (@1611), and re |
| CH-U5-01 | U5 | inference | H-U5-07 (Cause: apply turns terminal into a returned Err (pr | ❗ upheld | Downgrade H-U5-07's barrier cell to contested and strike the Cause clause "(process exit)": `apply` returns `Err` to a fire-and-forget spawn that only |
| CH-U5-02 | U5 | inference | H-U5-05 (Consequence: a non-bootstrap peer can … consume the | ❗ upheld | H-U5-05's consequence clause "…or consume the trigger with junk so the real answer is discarded" is struck: the real bootstrap answer is not discarded |
| CH-U5-03 | U5 | tree | H-U5-06 (Cause: cost and its 'read APIs open those roots' re | ❗ upheld | H-U5-06's Cause anchor is false and must be corrected: strike "cost and its 'read APIs open those roots' reason are recorded in spec/audit/passes.md ( |
| CH-U5-04 | U5 | inference | H-U5-02 (Consequence: … the joiner cannot index its own rest | ❗ upheld | Reclassify H-U5-02's top event TE-1 → TE-3; strike the TE-1 assignment. The cited #139 shape is one node that cannot index its restored chain while th |
| CH-U5-05 | U5 | witness | H-U5-02 (Barrier Evidence: n139-mature-join-results.md — pos | ❗ upheld | Downgrade H-U5-02's barrier to contested / `unmeasured` for the deviation's own arm: the "post-fix 4/5 green" evidences the complete-metadata path onl |
| CH-U5-06 | U5 | tree | H-U5-01 (Consequence: the C181/C102 latch class, one stage e | ❗ upheld | H-U5-01's consequence keeps its substance but its citation is corrected: read "the C181/#102 latch class, one stage earlier", not "C102". C102 is the |
| CH-U5-07 | U5 | tree | H-U5-01 (Cause: the request retries the send only and stops  | ❗ upheld | Correct H-U5-01's Cause cell (the row's hazard/band/[shared-cause] label/A2.c are unchanged and reinforced): from "retries the send only and stops onc |
| CH-U5-08 | U5 | inference | H-U5-06 (Deviation: the trailing state pull … of every downl | ❗ upheld | Downgrade H-U5-06 from a Table A HAZOP deviation to contested and re-home it as an efficiency note ("the approved-state join hydrates every downloaded |
| CH-U6-01 | U6 | tree | H-U6-06 cause and Table B row | ❗ upheld | Rewrite H-U6-06's anchor file (citation defect, no new run): Table A cause cell "AUDIT C111, spec/AUDIT.md" → "AUDIT C111, spec/audit/passes.md:4063"; |
| CH-U6-02 | U6 | inference | H-U6-06 consequence: '(equivocation itself is not slashable) | ❗ upheld | H-U6-06's consequence parenthetical "(equivocation itself is not slashable)" is false on this tree and must be dropped or replaced with a C200 citatio |
| CH-U6-03 | U6 | configuration | H-U6-02 deviation | ❗ upheld | H-U6-02 is contested as written: its deviation ("moves at every boundary even when no stake moves") must be restated with the cap precondition (number |
| CH-U6-04 | U6 | inference | H-U6-02 consequence '→ TE-2' and its Table B [shared-cause]  | ❗ upheld | Downgrade H-U6-02's refusal-consequence cell to contested and reclass its TE-2 → TE-1 (the halt); H-U6-04 keeps TE-1. The Table-B `[shared-cause]` lab |
| CH-U6-05 | U6 | inference | H-U6-02 deviation/consequence | ❗ upheld | H-U6-02's consequence/`→ TE-2` fingerprint is downgraded to contested: it keeps an admitted-unmeasured consequence (the pruned-only fallback refusing) |
| CH-U6-06 | U6 | inference | H-U6-04 deviation, and its Table B [self-referential] eviden | ❗ upheld | Downgrade H-U6-04's unqualified deviation ("while the finalized fringe is still empty — permanently wedges the chain") to contested / pre-fix #73 beha |
| CH-U6-07 | U6 | witness | H-U6-01 Table B evidence cell (c207 evidence §5–§6) | ❗ upheld | Downgrade H-U6-01's Table B evidence cell to contested: `c207 §5–§6` evidences the barrier (the universal `pos:vault` conflict is gone) but not the ro |
| CH-U6-08 | U6 | inference | H-U6-01 / H-U6-03 / H-U6-05 Table B independence labels | ❗ upheld | The three s1-U6.md Table B Independence cells are downgraded to `contested` (original text preserved). Relabels: H-U6-01 → `[one-surface]` (the barrie |
| CH-U6-09 | U6 | inference | H-U6-05 cause | ❗ upheld | The challenge is upheld: H-U6-05's deviation-cell clause "the refund then waits `quarantine_length` more blocks past its deadline" inverts the recorde |
| CH-U7-01 | U7 | configuration | H-U7-01 Cause ("one shared RateLimiter::new(DEFAULT_API_RATE | ❌ refuted | No change — the barrier is real. The challenger's conclusion (that H-U7-01's consequence "denial of admission for all other senders" over-scopes and s |
| CH-U7-02 | U7 | inference | H-U7-03 Cause (casper/src/validate.rs:deploy_count — "the do | ❗ upheld | Downgrade H-U7-03 to contested. The deviation stands (system_deploys ARE excluded from deploy_count and block_phlo), but its Cause cell must be struck |
| CH-U7-03 | U7 | instrument | H-U7-04 Table-B row (barrier repeat_deploy / deploy_index; I | ❗ upheld | The H-U7-04 Table-B row is downgraded as an instrument: its Independence cell is relabelled `[one-surface]` (repeat_deploy/deploy_index guards the dou |
| CH-U7-04 | U7 | tree | H-U7-05 Table-B ("none distinct — expiry and selection read  | ❗ upheld | The Table-B H-U7-05 cell's stated rationale ("expiry and selection read the same clock, so no barrier separates them" / `[self-referential]`) is refut |
| CH-U7-05 | U7 | instrument | Table-B Independence column (H-U7-03 empty, H-U7-04 prose) | ❗ upheld | The U7 Table-B Independence column does not do its stated job: fill/relabel the two non-conforming cells — H-U7-03 ← `[one-surface]` (unlabelled barri |
| CH-U7-06 | U7 | witness | H-U7-05 Cause ("the freeze is #144/#148") | ❗ upheld | Repair H-U7-05's Cause cell: cite spec/audit/evidence/n148-results.md (arm A) instead of "#144/#148" — the deviation survives with its premise now bac |
| CH-U8-01 | U8 | tree | H-U8-09 consequence — "denies every inbound connection (tran | ❗ upheld | Correct the H-U8-09 consequence: downgrade the shared-denial clause to per-service. `accept_tls` is shared only as code (same mutual-TLS admission log |
| CH-U8-02 | U8 | inference | The 'vacuous: REVERSE' line in Guide words | ❗ upheld | Downgrade the U8 REVERSE vacuity note (s1-U8.md:11) from vacuous to contested. Record the referent as a candidate REVERSE deviation: an inbound Protoc |
| CH-U8-03 | U8 | inference | H-U8-01 consequence '→ TE-4' against H-U8-04 consequence '→  | ❗ upheld | Re-map H-U8-01's consequence from TE-4 to TE-1, aligning it with H-U8-04 (same `dispatch_slots` semaphore mechanism). Drop TE-4 for this row — saturat |
| CH-U8-04 | U8 | tree | H-U8-06 consequence — "later dials it (connect.rs:clear_conn | ❗ upheld | Downgrade the H-U8-06 consequence's citation to `connect.rs:clear_connections` alone — the `find_and_connect` half is mis-aimed (it dials the Kademlia |
| CH-U8-05 | U8 | inference | H-U8-02 Table B barrier row | ❗ upheld | The H-U8-02 barrier row is corrected: the `accept_tls` handshake semaphore is the barrier for R14 handshake pile-up (H-U8-09, where it is already cite |
| CH-U8-06 | U8 | inference | H-U8-08 Table B barrier row | ❗ upheld | Re-pair the H-U8-08 barrier row: the `handle`-awaits-`routing_queue.send` cell carries NO preventive barrier for the LATE deviation (write `none — the |
| CH-U8-07 | U8 | inference | Barrier-independence labelling of H-U8-04 vs H-U8-06 | ❌ refuted | No change — the barrier is real and the rows stay separate. H-U8-04 stays paired with the awaited reply send held under the dispatch slot (handle_mess |
| CH-U8-08 | U8 | witness | H-U8-05 Table B evidence cell | ❗ upheld | Downgrade H-U8-05's barrier-evidence cell: the named test is the aggregate byte budget's (`charge_stream_budget`), not "the breaker's unit tests" — re |
| CH-U9-01 | U9 | configuration | H-U9-03 deviation cell ('three separate LMDB environments, s | ❗ upheld | Downgrade the H-U9-03 cause cell (s1-U9.md:13) from "three separate LMDB environments" to "two environments (rspace/cold, rspace/history), three logic |
| CH-U9-02 | U9 | inference | the guide-word line's vacuous: LESS reason ('the DAG has no  | ❗ upheld | The `vacuous: LESS` reason in s1-U9.md's guide-word line ("the DAG has no removal path") is refuted; LESS has a referent (as finality advances the DAG |
| CH-U9-03 | U9 | witness | H-U9-01 cause cell: 'block-storage/src/dag/message_map.rs:pr | ❗ upheld | H-U9-01 cause cell (A1.1): the gloss "(computes a fringe, deletes nothing)" is confirmed false as written and must be scoped to "computes the index-ca |
| CH-U9-04 | U9 | inference | H-U9-01 deviation clause 'although the finality rules need o | ❗ upheld | H-U9-01's deviation clause "the finality rules need only the fringe plus the 5-height liveness window" is false as written and must be downgraded to c |
| CH-U9-05 | U9 | inference | H-U9-02 independence cell 'shared-cause (the same absence-of | ❗ upheld | Relabel H-U9-02's independence cell (s1-U9.md:21) from [shared-cause] to [one-surface]: the consistency guard bounds the root-consistency surface and |
| CH-U9-06 | U9 | instrument | H-U9-01 barrier evidence anchor 'casper/src/dag.rs:179-208' | ❗ upheld | Re-anchor the H-U9-01 barrier Evidence cell (s1-U9.md:20) from the bare range `casper/src/dag.rs:179-208` to `casper/src/dag.rs:set_gauges`. The barri |
| CH-U10-01 | U10 | tree | H-U10-06 cause cell | ❗ upheld | Re-point the H-U10-06 cause cell's "written by" clause from node/src/runtime/node_runtime.rs::note_timer_halted (a call site) to casper/src/api/block_ |
| CH-U10-02 | U10 | tree | H-U10-02 (deviation + consequence) and its Table-B barrier | ❗ upheld | Downgrade H-U10-02 to a historical near-miss: the tree at HEAD cannot produce the deviation (C206 done; `deadline − latest`, no double-add), yet the r |
| CH-U10-03 | U10 | inference | the header's guide-word folding and the 'vacuous: none (7 ro | ❗ upheld | The `BEFORE → LATE` fold clause is downgraded to contested: the Ack sentence "before/after are the same referent" is false as written and must be rewr |
| CH-U10-04 | U10 | witness | H-U10-01 independence cell | ❗ upheld | Downgrade the H-U10-01 independence cell wording: drop "the nginx-derived /health" from the enumeration of halt-reading views and change "one barrier, |
| CH-U10-05 | U10 | witness | H-U10-03 Table-B Evidence cell | ❗ upheld | No downgrade of H-U10-03's claim — it is re-anchored. Amend the Table-B Evidence cell to cite the committed run `spec/audit/evidence/n149-results.md:2 |
| CH-U10-06 | U10 | tree | H-U10-05 (deviation + OTHER THAN assignment) | ❗ upheld | Downgrade H-U10-05's OTHER THAN assignment to unsupported: the surface is the faithful port of legacy StatusInfo.service (address/version/peers/nodes, |
| CH-U10-07 | U10 | witness | H-U10-04 Table-B barrier (the contrast to /api/status) | ❗ upheld | Downgrade the H-U10-04 Table-B barrier's "/metrics reads all" half to structural/unwitnessed coverage: the label stays [one-surface] but its note must |
| CH-U10-08 | U10 | witness | Table-B Evidence cells for H-U10-02 and H-U10-04 | ❗ upheld | Downgrade the two Table-B Evidence cells (H-U10-02 and H-U10-04) to `unmeasured`: they record the barrier's text/presence, not a run. Clear it by comm |

---

# 5. Deferrals — the 2026 lens, and what it does not decide

The audit was calibrated against what a new L1 is expected to satisfy in 2026, not against 2020's bar.
But the green light asked for here is **#214's three criteria**, so the wider criteria appear below and
**not** in §3.

> **The rule for this table.** Nothing here may be cited as a reason to **grant** or to **withhold** the
> green light. Each row is here because the 2026 lens raises it and #214 does not. Each carries a
> **falsifiable promotion trigger** — not a date — and is promoted into §3 when the trigger fires.

| Item | Why it is not a green-light item | Promotion trigger (falsifiable) | Where it would live |
|---|---|---|---|
| Light client / header sync | absent; a full node is the only way to check the chain | the first consumer that does not run a full node | a new §3 criterion |
| Finality undo / reorg depth | absent; a finalised block is never undone | **any claim of safety** — the moment TE-2 is asserted rather than deferred | §2 TE-2 → §3 |
| Inactivity leak / stake decay | deliberately absent (income-only participation rule, Law 44) | a net where an absent validator's stake must not count for ever **and** the quorum denominator may shrink. **A law before it is code** (`candidate:inactivity-leak`), and a decision for the maintainer, not the implementer | §3 criterion 4 |
| Hard-fork / activation gate | **compiled, one row**: `rchain_models::block_version::SCHEDULE` (`models/src/block_version.rs`) holds `(version, from_height)` rows and the block path refuses a block whose version is not the one scheduled for its height, so a release can stage a rule at a height instead of a new genesis. The shipped table is `(1, 0)`, so nothing is staged yet; a governance-set table and per-chain heights are still absent | a long-lived net with outside validators, **or any wire-class change** | §3 |
| State GC / DAG pruning | absent by recorded decision; residency is Θ(N²) | measured residency crosses the host envelope, or a validator's duty cycle is day-scale | §3 |
| Snapshot / backup / restore tooling | absent — **but note the in-scope exception below** | a validator that must be rebuilt without a re-genesis | §3 |
| HSM / keystore / remote signer | absent; a raw key file on disk | any key with value | §3 |
| On-chain governance | design only (qucalc); every rule change is a re-genesis | a rule change that must be made **without** a re-genesis | §3 |
| External audit + bug bounty | none — the only audit is the internal September 2026 self-review | the first **public claim** of viability: a census is measured *against* an adversary, and a bounty is what prices an adversary's budget | §3 |
| Load / TPS harness | there is **no committed TPS number anywhere**, only a derived band that the scaling page itself marks as arithmetic on a misread height rate | the first consumer that needs a number | §3 |
| Dedicated soak harness | none; the longest committed windows are 180–300 s | any window longer than 300 s | §3 |
| Network-level fuzzing (`cargo-fuzz`) | only `tools/devnet-fuzz.py` + proptest exist | parity work on TE-2 (a fork is the fuzz target that matters) | §3 |
| Client diversity | **structurally absent** — there is one client | a second independent implementation, or the point at which a bug affecting >⅓ of the stake must not be able to stall finality | §3 |
| MEV / transaction-order auctions | **does not apply**: one proposer per round and no fee market for position, so there is no ordering auction to defer | — | not applicable |

**One row that is *in* scope despite the deferral.** The snapshot/restore tooling above is deferred, but
the **mature-chain LFS restore path is not** — C188/#139 records it broken, and it *is* criterion 2(c)'s
prerequisite (a joining validator must be able to restore a mature chain). It is measured in §3.2, not
deferred here.

---

## 5.1 Closing the loop

The audit's own method makes one demand of this page, and it is the demand the repository has been slow
to meet on itself.

**The worksheet in §1 was a first pass, and it was contested — and it has since been corrected.** The challenger upheld 70 of 75 objections to
it, and the dominant reason was not that the node is worse than the worksheet said — it is that the
worksheet was built from **evidence superseded by the fixes it describes**: a run produced before the fix
it was meant to witness, a code path HEAD no longer has, a falsifier inverted after the fact. The same
staleness is visible one level up, in the repository's own intent file, whose "operational target"
sentence named four defects as the things blocking a two-validator net — all four `done`, and since
overtaken by C209 and C210.

That is the honest state of the journey this page was asked to map: **the project has moved faster than
its own account of itself.** Criterion 2 was red on the audited tree and criterion 1 unproven when this
page was written — both now pass — but the deeper finding stands: the ledger of what-is-true lags the tree — exactly the failure the evidence rule
of §0.7 exists to catch. And the audit itself committed it: see §0.9.

**What would close it.** Not more prose. Three things, in order:

1. **A re-probe on the current tip** — **done**, §3.4, committed as `spec/audit/evidence/n214-*`. It failed
   both criteria on the pre-fix tree, with an artefact, and raised the question that mattered most: whether
   #214's N=3 pass reproduced. **It does** — re-verified on `07af032ad` (§3.1, §3.2), and §3.4's
   contradiction is closed.
2. **The cause** — **found and fixed**, §3.5. The first diagnosis named the right component and the wrong
   instance; the landed fix is **C209 + C210** (PR #219), verified independently on `07af032ad`, and A1.1
   is ✅ on that evidence.
3. **A correction pass over §1** — **done, 2026-10-04.** All 70 upheld challenges are applied to the cell
   each one names, and the 5 refuted ones are recorded as refuted with their rows left as they stood.
   **It was not one uniform job.** U4–U10 were mechanical — each §4 challenge names the cell, and the
   correction is a transcription of its adjudication. **U1–U3 were not**: their rows were adjudicated on a
   tree that predates **C209/C210**, so those corrections are re-derivations against the fixed tree, and
   H-U1-01's surviving mechanism (the attestation guard) is a different mechanism from the one the row
   originally cited (the tap). The pass is honest about what it could not do: **it corrects text, it does
   not measure**. Every cell it marked `unmeasured` is an owed run, and §1's ⚠ now marks exactly those.
4. **A fetch in the setup**, which is the one thing §0.9 says no amount of care inside the analysis
   substitutes for.

---

*Provenance of the analysis.* This page was produced by an adversarial wargame over tree `1e5a64ed4`: a
scope pass fixing ten study nodes; ten node readers producing the §1 worksheets; ten challengers reading
only their sibling sheet; four bow-tie owners hunting shared causes across nodes; 75 per-challenge
adjudicators settling each by read-only observation; and the register and checklists built from the
adjudications rather than from the readers' prose. The intermediate sheets live outside the repository
and are not evidence; only the artefacts they cite are.

*This page carries no gate.* It is not checked by CI — no such check exists — and a page that says a rule
holds while nothing enforces it is precisely the failure the review-ledger was retired for. The rules of
§0.6 are meant to be read and applied by a reviewer, not trusted to a script. The audit's own largest
finding — that hazard analysis had drifted behind the tree — is a warning against adding one more
unchecked register.
