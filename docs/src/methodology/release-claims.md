# Paper C — Release claims: a mainnet gate derived from committed evidence

> **Draft. Not a published paper, and not a claim to be one.** This is a checkpoint of the method by
> which this tree states, measures and closes a release claim. It describes mechanisms and the
> vocabulary of a reading; **it reports no arm's current reading and makes no readiness statement.**
> The register and the generated tracker are the authority for what is observed; this paper is the
> argument for why the observation is shaped the way it is. §6 states the measurement it still lacks.

## Abstract

Release readiness is normally an authored verdict or a dashboard. Both are claims about a moment and
an author, and a reviewer cannot re-enter either one. This project makes the unit of release truth an
observable product claim with one executable test, derives its reading by re-running that test over
the newest committed record, types closure by environment, and couples a false trace predicate to a
Rust symbol and a Lean theorem. Around that sit current-build gates whose witnesses must carry a
recorded negative control, an evidence record whose pins prove the bytes but deliberately not the
execution, and a supersession discipline that keeps every retired criterion mapped to what replaced
it. The paper describes the method and catalogues where it has failed. It claims a combination, not
any single ingredient, and it reports no reading of any arm.

## 1. Introduction

A release is usually authorized by a checklist, a green dashboard, or a named reviewer's opinion.
Each answers a question about a moment: the checklist was completed, the dashboard was green at some
poll, the reviewer signed on some date. None is re-enterable by a third party who was not there, and
none says what would have made it fail.

The move this repository makes is to make the **claim** the unit of release truth. A claim is an
observable product behaviour with a pass condition, a fail condition, an observable, a negative
control, an owner, a list of explicit non-claims, a decision-trace kind, and one or more arms. The
claim is not stored with a status. Its reading is derived from the newest committed record by
re-running the claim test's own `check(record)`; a status that disagrees with the record is coerced
to the record's verdict. Nothing about a reading is authored.

The contributions are five, and each has a limit:

1. **One test runs and re-checks.** The same module executes the campaign and judges the record it
   wrote. The limit is that a test can be weak: the mutation suite proves it fails on named
   mutations, not that it captures the claim.
2. **Closure is environment-typed.** A local mechanism check cannot close a public claim, and a
   rehearsal of the public path is refused as evidence. The limit is that a declared locality is
   still a declaration, checked only as far as its artifacts allow.
3. **Readings are ordered by conclusiveness.** Six readings replace pass/fail, each naming a
   different next action. The limit is that the vocabulary is close to prior art in fault-injection
   checkers; the ordering and the action binding are the contribution.
4. **A false predicate names a Rust/Lean seam.** `unsafe-point` carries the predicate's Rust symbol
   and Lean declaration. The limit is that the seam is one-directional: a true law does not make a
   claim pass.
5. **Supersession is typed and enforced.** A retired campaign, gate or attestation stays mapped to
   its fate and home, and its id cannot be reused. The limit is that the mapping records what the
   project chose to do, not that the choice was complete.

## 2. Related work

Every ingredient has prior art, and a paper claiming novelty for any one of them would be wrong.

- **Fault injection and history checking.** The Jepsen tradition injects faults, records a history
  and decides it with a checker, with verdicts that already distinguish pass, fail, inconclusive and
  timeout. This paper borrows "check the recorded history, not the label the system printed". It does
  not claim a new checker, a new consistency model, or any specific analysis this project did not
  run.
- **Chaos engineering.** Basiri et al. (IEEE Software, 2016) state the discipline as a declared
  hypothesis tested by a controlled fault. The mechanism here shares that shape but is deliberately
  narrower: the fault is preconditioned, the driver — not the operator — chooses the victim, and the
  campaign manifest cannot carry the outcome. This is not production chaos.
- **Reproducible builds and supply-chain provenance.** Reproducible-builds practice pins inputs and
  rebuilds; SLSA and in-toto pin provenance and attest what a builder did. This paper borrows "the
  evidence is an artifact with pins", and it explicitly refuses to claim execution: the record says
  `execution_proven: false` by construction. There is no claim of SLSA or in-toto compliance, of
  bit-reproducible binaries, or of builder attestation.
- **Formal-methods experience reports.** Newcombe et al. (CACM, 2015), Kaizen (FMCAD 2019) and the
  FMBC 2019 work are the model for an experience report with its costs stated. That lineage belongs
  to [Paper A](law-first-port.md), which covers the law-first port and its static apparatus; this
  paper is the release layer above it.
- **Audit and attestation practice.** Signed opinions and their non-claims are the assurance product
  in audit practice. This paper borrows the vocabulary of attestation and non-claims, and then
  retires a named-reviewer verdict as a release condition because a verdict is an attestation, not a
  measurement. It makes no claim of audit equivalence or compliance.
- **Runtime verification and property-based testing.** The trace predicates are runtime verification
  over a decision trace, and the mutation suite is in the property-based-testing tradition. The
  paper claims neither a monitoring algorithm nor a new testing technique.

### 2.1 What is novel, and what is not

| Element | Nearest prior art | What this paper claims | What it does not claim |
|---|---|---|---|
| Claim as release unit | CI gates, history checkers | one observable behaviour, one test that both runs and re-checks, no stored status | a new checker or consistency model |
| Environment-typed closure | staging vs production | locality is an arm property; a local or rehearsal record cannot close a public arm | that local runs are worthless |
| Conclusiveness-ordered readings | pass/fail, checker verdict classes | each reading names a different next action; the worst required arm wins | that the vocabulary is unprecedented |
| Trace-to-law seam | verified stacks, runtime verification | a false predicate names both `path.rs:symbol` and a Lean declaration | that the law register proves the node |
| Typed supersession | issue-tracker deprecation | retired criteria stay mapped; deleted ids are refused reuse | that the record is complete |
| Pin plus non-claim | SLSA, in-toto, reproducible builds | a digest-pinned record with `execution_proven: false` and an authored non-claims section | SLSA compliance or proven execution |

If prior art for the combination is found, the claim survives as an experience report of the
combination; it is not a priority claim.

## 3. The method

### 3.1 The claim is the unit of release truth

A claim is an observable product behaviour with pass and fail conditions, an observable, a negative
control, an owner, explicit non-claims, a decision-trace kind, and one or more arms. It is not a
gate: claims and current-build gates are disjoint registers, and a claim id may not appear in the
gate register. The claim register stores no status and no missing-observation field; both are
forbidden. The reading is derived from the newest committed record for the arm by re-running the
claim test's own `check(record)`. A record whose stored verdict is `pass` but whose predicate refutes
it is read as failing, and the coercion is itself tested.

### 3.2 Two environments; a local record is not a public one

Every claim carries two environments. The local environment is a four-validator Docker devnet; the
public environment is four independent hosts. The distinction is not prestige: local shows that a
mechanism is wired, public shows that it survives independent hosts and a real stop and restart. An
arm carries its own locality (`local-devnet`, `public-live`, `multi-host`, `off-host-claim`, and
others), and locality is checked against the artifacts where the checker can reach them.

A public arm is closed only by a public record. The public path can be rehearsed against the same
four validators on Docker, but the rehearsal is marked `rehearsal`, carries `local-devnet`, reads
`not run`, and the packer refuses to commit it. The harness that runs the public path — the fleet
driver, its transports and its actions — is described in
[The public fleet campaign](../node/public-fleet.md); this paper does not restate the procedure.

### 3.3 The six readings

Pass/fail is too coarse for a release claim. The reading names what can be concluded and what to do
next:

| Reading | What it names | What closes it |
|---|---|---|
| `not-observed` | no committed record for this arm | run the arm and commit the record |
| `precondition` | the run could not produce its observation | supply the named precondition and re-run |
| `evidence-insufficient` | the record lacks the trace or a required coordinate | capture or re-run with the decision trace |
| `unsafe-point` | a trace predicate is false | fix the predicate at its Rust anchor and Lean theorem |
| `model-incomplete` | every predicate holds and the claim still failed | extend the model or trace before changing the test |
| `safe` | the newest committed record passes its own predicate | close the arm, in its environment only |

The readings are ranked by **conclusiveness**, not by how bad the node is: `not-observed` is the
weakest (nothing was run), then `precondition`, `evidence-insufficient`, `unsafe-point`,
`model-incomplete`, and `safe`. The claim's reading is the worst required arm's. The ordering is the
design point: a named unsafe point tells you what to fix, while an unreadable record tells you only
that you cannot conclude. The aggregate therefore takes the position that establishes least, even
when another arm came closer to naming a defect.

Each non-`safe` reading has a close action in the register, written for the person who will act on
it. The close maps are not documentation: a reading with no action would be a reading nobody can
close.

### 3.4 The record: provenance pinned, execution not proven

A committed record is a directory under `spec/audit/evidence/` containing `manifest.json`,
`SHA256SUMS.json`, `README.md`, and the run's files under `raw/`. The manifest pins the source
commit, the binary SHA-256, the locality, and every file's hash and byte count. `execution_proven` is
always false: a digest chain proves that the bytes are the bytes, not that the campaign ran.

The packer writes a README skeleton with explicit fill markers. Verification refuses an unlisted
file, a bad hash, a placeholder left unfilled, a `PASS` verdict beside a failing arm, and — for the
current packer schema — a missing authored non-claims section. The non-claim is part of the record,
not an optional courtesy: it is where the run says what it did not establish.

### 3.5 The driver chooses the fault; the manifest is an input

A public campaign starts from a manifest naming four nodes, the candidate pins, one deploy plan, and
the action policy. The manifest may not carry the outcome. Keys the driver produces — the harness
result, the victim, the boot ids, the post heads, the certificates after the kill — are refused, and
the settlement claim additionally refuses a pre-made deploy id because it submits its own deploy.
This is the difference between "the operator says a validator was restarted" and "the driver stopped
a validator and captured the new process boot id".

The recovery arm is where the distinction bites. The driver waits for a marker deploy to reach
`includedUnfinalized`, then chooses the victim — never the bootstrap, never the deploy ingress —
stops it, waits for the survivors to finalise past the kill and for a second marker to settle, starts
it, and requires a **new process boot id** and a rejoin at the common finalised head at least three
heights above the baseline. The restart witness is the node's own `/health.bootId`; the kernel boot
id only establishes that four hosts are four hosts.

### 3.6 The gate layer: current-build gates with a bound witness

Beneath the claims is a register of current-build gates. A gate is a boolean test of the current
build. A decision, a reviewer's verdict, an operator export, or an unavailable historical archive is
not a gate; those are tracked separately, with their own statuses.

Every gate record names its pass and fail/block conditions, its observable, its witness, its negative
control, its locality, its evidence, its owner and close condition, and its non-claims. A witness
with no negative control is `unproven` and cannot make a gate `met`. A `met` gate carries a witness
binding: the acceptance hash, a control whose target is the witness, a real mutation, and the
expected red. The checker calls the control `CONTROLLED` only when the declared mutation made the
witness fail; a non-zero exit that did not run the right test is an error, not a control.

The rules that follow are stated normatively in [Gate measurement](../spec/gate-measurement.md) and
[Gate terms](../spec/gate-terms.md). This paper does not restate them. It notes only their shape: a
record check does not prove a campaign ran, Docker is never the sole evidence, `PRECONDITION` is
never `PASS`, one sign-off does not discharge several gates, and a name is not independence.

The aggregate launch gate has two halves. `tools/release-check.py --assert-ready` is the claims
half: every required arm must read `safe` over a committed record. `tools/check-launch-gate.py`
adds the external half: the open set is derived from `mainnet-gates.external.json`, each open row
needs exactly one dated decision in `launch-gate.dispositions.json`, and `--assert-ready` is green
only when every external row is `met` or `superseded`. A dated disposition records who decided and
when; it is not a pass and it cannot substitute for closure.

### 3.7 Supersession as a self-correcting method

The method records its own corrections. The live and host-level rows that had been written as
release campaigns were dispositioned rather than deleted: some were rewritten as claim arms, some
demoted to sizing guidance or a runbook, some retired into the evidence rule. A named reviewer's
verdict was a release condition and was retired, because the claim reading is the verdict. Every
deleted id remains in the superseded mapping with a fate and a home, and the checker refuses an
unmapped id, a reused id, or a claim set below the floor it enforces.

The moves are typed — campaign to claim arm, attestation to evidence rule, live snapshot to runbook,
sizing measurement to guidance — and the red-team review that forced the move is itself in the tree.
Supersession is not deletion: it is the record of what a thing became, so a reader can follow the old
criterion to its replacement instead of finding a hole.

### 3.8 Coupling to the law register

The claim tests do not trust the node's own boolean labels. They re-derive predicates from the
coordinates of the decision trace. When a predicate is false, the reading is `unsafe-point` and the
item names the predicate's anchors from the anchor table: a Rust `path.rs:symbol` and a Lean
declaration. When the trace or a required coordinate is missing, the reading is
`evidence-insufficient`, not unsafe. When every predicate holds and the claim still failed, the
reading is `model-incomplete`: the trace does not yet model the failure, and the next action is to
extend the model, not to weaken the test.

The other side of the seam is the law register. A law row carries its statement, the declarations
that establish it, the axioms it rests on, its emitted conformance corpus, its Rust anchors and
witnesses, the prose that would falsify it, and the `formal`/`mutant`/`falsifier` triple that makes
the proof rung checkable. The conformance gate refuses a stale register and runs every registered
Rust witness by name.

The seam is one-directional. A false observation names the artifact that should have prevented it;
a true law does not make a claim pass. The law register is a proof about the model, and the claim is
an observation about a run.

## 4. Two claims in detail

### 4.1 RC-1 — truthful settlement

The claim: if the settlement endpoint reports `finalizedSuccess` with `settlementConfirmed`, the
deploy's effect is present at the finalised head on every validator and remains present through two
further epoch boundaries. The observable is the settlement response, every validator's finalised
(height, hash), the marker read at that block, and the per-block rejected-deploy list.

The claim has three arms: a local mechanism check, a public four-host run, and a bond-leave arm on a
disposable devnet. The decision trace is the merge trace. Its predicates are `base_ok` — the merge
base is certified or every claim agrees on one hash; `fork_ok` — a fringe merge has no fork blocks;
`ledger_ok` — no user host in the merge's conflict scope appears in the rejected set; and
`scope_partition_ok` — the conflict chains equal the kept chains plus the rejected chains. The
anchors are the base-claim chooser and its Lean agreement theorem, the guarded fringe constructor and
its no-forks theorem, the finalised floor and its ledger theorem, and the merge scope and its kept
rule.

The negative controls are a marker absent before the write, a fixture where the effect is absent at
the finalised head, and a fixture where the deploy is rejected at final scope. The three failure
shapes are distinct: a false predicate is `unsafe-point` with a named anchor; a record without the
merge trace is `evidence-insufficient`; all predicates true with the write absent is
`model-incomplete`. The explicit non-claims are that a local pass is a mechanism check, that the
public claim needs the four-host record, and that the bond-leave arm does not claim a validator can
leave with an unpaid bond or skip quarantine.

### 4.2 RC-4 — recovery without loss

The claim: after one validator is stopped and restarted, or a partition heals, the survivors keep
finalising; every deploy accepted by a survivor is resolved and its effect is in the post-recovery
finalised state; the victim rejoins the same finalised head; and no spurious self-replay or own-key
halt fires. The observable is the survivors' finalised heads, each accepted deploy's resolution and
effect, the victim's boot id and finalised head, and the own-key and self-replay counters.

The arms cover a local restart, the public four-host stop/restart, a multi-host loss, a failover, and
a returner that missed a boundary. The decision trace is the boot trace, read at the agreement
decision rather than from the last filename written. Its predicates are `genesis_ok` — a
genesis-only run is accepted as genesis; `acquisition_ok` — an accepted boot's acquisition path is
certificate, stored or fetched; `verification_ok` — the served support is a supermajority of the served
stake; and `anchor_ok` — the run state has the certified anchor closure, either because the fringe
agrees or because the sync performed a light join. The anchors are the boot outcome and its safe-boot
theorem, the certificate acquisition and its refusal-coordinate theorem, and the rebuild window and
its running-is-anchored theorem.

The negative controls are the wrong-genesis arm and the interrupted-restart arm. A deploy accepted
only by the killed validator is not recoverable by design and must be declared, not counted. The
failure shapes are: no new process boot id is `precondition`; a record without the victim's boot
trace is `evidence-insufficient`; a false `anchor_ok` on a quorate certificate is `unsafe-point`. The
non-claims are that a lost store needs a fresh data directory and a checkpoint join, and that the
returner arm covers a missed boundary, not a missed bond change entirely.

### 4.3 The shape of the seven

The claim register defines seven claims. This table is a descriptive snapshot of their design space;
the register is the authority, and the table carries no readings.

| Claim | Assertion | Environments | Arms | Trace |
|---|---|---|---|---|
| RC-1 Truthful settlement | a confirmed settlement is present at every validator's finalised head and retained across two epoch boundaries | local, public | local, public, bond-leave | `merge` |
| RC-2 Safety | no two conflicting finalised histories; no finalised regression across restart, replay or partition | local, public | local, public, multi-host, equivocation, validator-set | `finality` |
| RC-3 State agreement across a merge | the same accepted block derives the same pre/post state and kept/rejected ledger on every validator | local, public | local, public, availability | `state` |
| RC-4 Recovery without loss | survivors keep finalising, accepted work survives, the victim rejoins the common finalised head, and no spurious halt fires | local, public | local, public, multi-host, failover, returner | `boot` |
| RC-5 Overload refusal | at the admission bound the node refuses explicitly with a retry signal, with no 5xx and no accepted-work loss | local, public | local, public, transport | `admission` |
| RC-6 Join and audit | an off-host joiner applies the certified anchor and reaches the same finalised head; audit reproduces the checkpoint claim | local, public | local, public, off-host, availability | `audit` |
| RC-7 Upgrade compatibility | a mixed-version fleet keeps one finalised chain or refuses on the consensus path; a store migrates; a newer format is refused | local, public | local (mixed-store mode), public (fleet-upgrade mode) | `format` |

The columns are the design space: an observable behaviour, two environments, arms that name
behaviours rather than scripts, and a decision-trace kind that is the surface a false predicate is
read from. The generated campaign tracker, not this table, carries what each arm reads.

## 5. How the method fails

A method paper that reports only its successes is an advertisement. Each rule above was written after
a failure, and the failures are in the tree.

### 5.1 Recorded corrections

- **A committed record overstated its own evidence.** The red-team review found that a check record's
  README claimed a height, all-validator agreement and a finalised status that its raw record did
  not contain, and that the harness invoked by the record was not present at the record's own source
  pin. The record could not reproduce itself; the correction is a committed harness and a record that
  carries every claim it makes.
- **An attestation criterion checked the attestation's shape, not the claim.** Its fixture could pass
  on a record that did not establish the claim. It was demoted to an evidence rule: an attestation
  says who saw which bytes; it does not make a claim pass.
- **A named reviewer's verdict was a release condition.** It was retired. The claim reading is the
  verdict; a name is not a measurement, and a sign-off is not re-enterable by a third party.
- **A record check was read as a campaign.** The record tools report `execution_proven: false` by
  construction. A record validates its claims, not the world.
- **A loopback observer was cited as an off-host positive.** The locality checker now refuses a local
  advertised peer in an off-host claim, requires a dial-back, and keeps the loopback diagnosis as a
  separate label that must say in words that it is not an off-host positive.
- **A deleted id could be reused or silently dropped.** Every deleted campaign, gate and criterion id
  remains mapped to a fate and a home, and the checker refuses an unmapped id or a claim that reuses
  one.
- **A status could be authored.** The register stores no status; a stored `pass` whose predicate
  refutes it is coerced to a failure. The test that proves the coercion is part of the suite.
- **A gate missed the instances it existed for.** A counts scanner was line-based and missed two of
  three stale totals; it now reads paragraphs. A worksheet row whose shape differed from its
  neighbours was invisible to every check the page had; the checker now refuses a row whose cell
  count disagrees with the header. [Paper A](law-first-port.md) §4 records this class from the static
  side.

### 5.2 What it still cannot catch

- **A weak claim test.** The mutation suite proves the test fails on the mutations it names, not that
  it captures the claim. A test that checks the wrong observable can pass its own fixture and read
  `safe`.
- **An unlabelled record.** The locality checker reaches records that declare a locality or mention
  the off-host question; a record that does neither is not dispositioned by it.
- **A declared locality its artifacts do not fully support.** The public label requires a fleet
  result with the SSH transport, four distinct hosts and boot ids, and the absence of synthetic boot
  ids; the other labels rest more on declaration than on a structural check.
- **A false predicate with no anchor.** The anchor table is finite. A new predicate without an entry
  yields an `unsafe-point` with no Rust or Lean anchor, which is a weaker item than the method
  intends.

## 6. Evaluation — what is missing

The claim is falsifiable, and the falsifiers are external to this draft:

1. A `safe` reading that a third party cannot recompute from the committed record and the test's own
   `check`.
2. A public arm closed by a non-public record: a rehearsal, a local run, an attestation, or a
   previous build's pins.
3. An `unsafe-point` whose named Rust symbol or Lean declaration does not resolve in the tree.
4. A reading whose close action is not actionable: no record to run, no precondition to supply, no
   trace to capture.
5. A superseded verdict that still gates, or a deleted id that vanished from the mapping.

What is still missing is the measurement that would show whether the reading vocabulary adds
information over a boolean. It has two halves. The first is a **reading history**: claim × arm ×
record × reading × time × close pull request, joined so that a reading can be followed from the
record that produced it to the change that closed it. The second is a **pass/fail counterfactual**:
for every record that did not read `safe`, would a boolean pass/fail have named the same next action?
If it would, the taxonomy is decoration; if it would not, the taxonomy is the contribution. Cost —
engineer time per reading, time to close, and the rate at which a reading must be re-derived — belongs
in the same table.

This is adjacent to the cost experiment Papers A and B name, but it is not the same table: those
attribute findings to assurance layers; this one attributes readings to actions.

This paper deliberately reports no arm's current reading and no readiness disposition. The register
and its generated tracker are the live status; a paper that restated them would be a second source of
truth and would age the moment a record was committed.

## 7. Threats to validity

- **n = 1, and no baseline.** This is one project's method report, not a controlled study. No causal
  claim about the method's efficacy is made.
- **The tests are authored.** Mutation sensitivity is not coverage. A weak test can read `safe`, and
  §5.2 states that limit rather than hiding it.
- **"Public" is not mainnet.** It means four independent hosts and a public record, not an
  adversarial public network. The method does not claim that a public arm survives an adversary.
- **Execution is pinned but trusted.** The digest chain proves the bytes; it does not prove the run.
  `execution_proven` is false by construction.
- **The law seam is one-directional.** A true law does not make a claim pass, and `model-incomplete`
  is a model gap, not a correctness proof.
- **The claim set is chosen, not derived.** Excluded behaviours are mapped to arms, but completeness
  is not proved. The paper is written by the project; the record, including this failure catalogue,
  is the mitigation.

## 8. Conclusion

The method's narrow claim is that release truth can be a derived property of committed records rather
than an authored verdict, provided the vocabulary says what can and cannot be concluded. The novelty
is the combination: environment-typed closure, a conclusiveness-ordered reading taxonomy, a
trace-to-law seam, and a supersession record that keeps retired criteria followable. None of the
ingredients is new, and the paper says so.

The method is not a substitute for a public campaign, an independent audit, or a proof. It is a way
to make those things checkable after the fact. The missing measurement is §6; until it exists, this
is a method report, not a result.

## References

In-tree:

- [`spec/audit/release-claims.json`](../../../spec/audit/release-claims.json) — the claim register
- [`spec/audit/release-campaigns.md`](../../../spec/audit/release-campaigns.md) — the generated tracker
- [`spec/audit/mainnet-gates.json`](../../../spec/audit/mainnet-gates.json) and
  [`spec/audit/mainnet-gates.external.json`](../../../spec/audit/mainnet-gates.external.json) — the
  current-build and external gate registers
- [`spec/audit/release-campaign-red-team-20261119.md`](../../../spec/audit/release-campaign-red-team-20261119.md)
  — the review that produced the claim set
- [`spec/audit/runbooks/rc-public-fleet.manifest.schema.json`](../../../spec/audit/runbooks/rc-public-fleet.manifest.schema.json)
  — the manifest contract
- [`spec/audit/evidence/`](../../../spec/audit/evidence/) — committed records
- [`tools/release-check.py`](../../../tools/release-check.py),
  [`tools/build-mainnet-gates.py`](../../../tools/build-mainnet-gates.py),
  [`tools/trace-evaluate.py`](../../../tools/trace-evaluate.py) — derivation and anchors
- [`tools/check-launch-gate.py`](../../../tools/check-launch-gate.py) and
  [`spec/audit/launch-gate.dispositions.json`](../../../spec/audit/launch-gate.dispositions.json)
  — the aggregate gate and its dated decisions
- [`tools/fleet-run.py`](../../../tools/fleet-run.py),
  [`tools/fleet_manifest.py`](../../../tools/fleet_manifest.py) — the public driver and manifest
  validator
- [`tools/evidence-pack.py`](../../../tools/evidence-pack.py),
  [`tools/check-evidence-locality.py`](../../../tools/check-evidence-locality.py) — the record and
  its locality rules
- [`tools/check-mainnet-gates.py`](../../../tools/check-mainnet-gates.py) — the gate and control
  checker
- [`tests/release-claim-1.py`](../../../tests/release-claim-1.py),
  [`tests/release-claim-4.py`](../../../tests/release-claim-4.py),
  [`tests/release_claim_common.py`](../../../tests/release_claim_common.py),
  [`tests/test_release_campaigns.py`](../../../tests/test_release_campaigns.py) — the claim tests
  and the offline proof suite
- [`spec/Rchain/Laws.lean`](../../../spec/Rchain/Laws.lean) and
  [`spec/Rchain/LawsMain.lean`](../../../spec/Rchain/LawsMain.lean) — the law register and its
  ceilings
- [The public fleet campaign](../node/public-fleet.md), [Gate measurement](../spec/gate-measurement.md),
  [Gate terms](../spec/gate-terms.md) — the normative and operational pages this paper points at
- [Paper A — the law-first port](law-first-port.md),
  [Paper B — auditable development by AI agents](agentic-assurance.md)

External:

- Newcombe et al., *How Amazon Web Services Uses Formal Methods*, CACM 58(4), 2015
- Basiri et al., "Chaos Engineering", IEEE Software 33(3), 2016
- [Jepsen analyses](https://jepsen.io/analyses)
- Torres-Arias et al., "in-toto: Providing farm-to-table guarantees for bits and bytes", USENIX
  Security 2019 — [in-toto](https://in-toto.io)
- [SLSA](https://slsa.dev) and [Reproducible Builds](https://reproducible-builds.org)
- [Kaizen: Building a Performant Blockchain System Verified for Consensus and
  Integrity](https://dprg.cs.uiuc.edu/data/files/2019/FMCAD%20Slides.pdf) (FMCAD 2019)
- [Formal Methods for Blockchains (FMBC 2019)](https://link.springer.com/book/10.1007/978-3-030-54994-7?page=2)
- Havelund and Goldberg, "Verify Your Runs", VSTTE 2005; Claessen and Hughes, "QuickCheck: a
  lightweight tool for random testing of Haskell programs", ICFP 2000
