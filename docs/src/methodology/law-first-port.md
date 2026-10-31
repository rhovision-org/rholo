# Paper A — The law-first port: a consensus node where the reference implementation is not the oracle

> **Draft. Not a published paper, and not a claim to be one.** This is a checkpoint of a paper that
> could be written from this tree. Its abstract, method and failure catalogue are grounded in artifacts
> that exist; **its evaluation section is not written, because the table it needs has not been mined.**
> §5 states exactly what is missing and the recipe for producing it. Nothing here should be cited as a
> result until that section exists.

## Abstract

We ported a consensus node from Scala to Rust and inverted the usual relationship between a port and its
reference. The reference implementation is not the oracle: the oracle is a register of
<!-- counts:laws -->91 laws<!-- counts:end -->, machine-checked in Lean 4 and Coq, from which conformance
corpora are **emitted** and run 1:1 through the node. The reference's surviving contribution is not truth
but *vectors* — a 165-file differential corpus of its own contracts. Around this inversion sits a wider
assurance apparatus: an authored findings register whose rows close only when the bad state is no longer
*representable*, a failure-mode HAZOP and a bounded-adversary tabletop whose worksheets are held to shape
by CI gates, eleven static gates, and an evidence discipline in which every run is committed rather than
cited. This draft describes the apparatus, states the design rules that produced it, catalogues the ways
it has failed, and specifies the missing experiment: an attribution of each finding to the layer that
surfaced it, with cost, which is the only honest way to answer whether the apparatus was worth building.

## 1. Introduction

A port is normally judged against the thing it ports. That framing has a defect that only shows up
late: it makes the *reference implementation* the specification. Every behaviour of the reference —
including its bugs, its unspecified corners and its platform accidents — becomes something the port is
obliged to reproduce. The port's test suite becomes a fidelity harness for a document nobody wrote down,
and the question "is this correct?" collapses into "does this match?".

This port refused that framing. The specification it answers to is a register of laws, independent of
both implementations, and the reference implementation is demoted to a source of test vectors.

The claim of this draft is methodological, not numerical. We describe a set of interlocking rules that
make the port's correctness claims *checkable by machine* rather than by review, and we state — plainly
and with citations into the tree — the ways those rules have failed in practice. The apparatus is larger
than the node it reviews, and that is a fact about soft targets: the node's language is a reflective
process calculus and its consensus is a proof-of-stake epoch over that calculus, so the surface where a
plausible-but-wrong claim can live is unusually wide.

## 2. Related work

The individual layers all have prior art, and a paper claiming novelty for any one of them would be
wrong. We say where each idea comes from, and what the combination adds, in §3.

- **Staged refinement with an explicit trusted base.** Kaizen (FMCAD 2019) combined Coq and Dafny to
  build a verified blockchain, with a refinement argument by *movers* and a named TCB. This draft adopts
  the discipline of naming the trusted base; its contribution is elsewhere (§3.1).
- **Coq models of consensus and contracts.** The Algorand consensus model and Mi-Cho-Coq for Tezos
  contracts were both presented at the first Formal Methods for Blockchains workshop (FMBC 2019), which
  remains the natural venue for this work.
- **A spec whose definitions are the definitions that run.** `etheorem` makes this argument for
  Ethereum's consensus specification in Lean 4: proof and vector describe the same object rather than
  drifting apart. We make the same argument for a *port* rather than a spec.
- **Rust verified by extraction and by harness.** A recent production L1 wires Lean 4 + Aeneas + Rocq +
  Kani + fuzzing over a Rust codebase, extracting the real MIR via charon rather than proving about a
  hand-written model. That project is the closest published neighbour to this one, and we are explicit
  that the *shape* is not novel — what differs is the direction of authority (§3.1).
- **The industrial experience report.** Newcombe et al., *How Amazon Web Services Uses Formal Methods*
  (CACM 2015), is the model for this draft, and the source of the paper's most quoted number
  (spec lines per bug found) — which §5 proposes to compute across seven layers rather than one.
- **Differential testing** as a technique is McKeeman's; **refinement** in the seL4 / IronFleet / Verdi
  lineage; **hazard study** is IEC 61882 HAZOP and Leveson's STPA. We did not invent them; we put them
  under CI (§3.6).

## 3. The method

### 3.1 The oracle inversion

The register in [`spec/INVENTORY.md`](../../../spec/INVENTORY.md) is one row per law, with the Rust
realization, the Lean/Coq target, the status, and the corpus and consumer for rows 30+. The laws are the
source of truth; the port answers to them; the Scala tree answers to nothing, because it is not here.

Two consequences. First, a law is a *change to three files* — the Lean definition, the corpus emitted
from it, and the Rust consumer — and the gate refuses any of the three drifting from the others. Second,
the reference's differential corpus is the only place it retains authority, and it retains it as *data*
(165 `.rho` files, run 1:1 through the node), never as *specification*.

### 3.2 Laws as checked data

The law register is not prose. It is a TSV with a Lean emission, a Coq half, a conformance corpus per
layer emitted by `lake exe rchain-corpus --layer <l>`, and a Rust consumer per corpus that must agree
1:1 with the cases it was given. Non-vacuity is enforced: a law whose statement no corpus case can
distinguish from its negation fails the gate. The corpus is the artifact that makes drift impossible
rather than merely unlikely — a definition cannot move without its corpus moving.

### 3.3 No silent partiality

The ρ→CoC type discipline ([`spec/TYPE-SYSTEM.md`](../../../spec/TYPE-SYSTEM.md)) makes totality the
default and partiality the exception that has to be *named*. A production `panic!`, an `unsafe` block,
or a silent numeric conversion in the Rust crates is refused by
[`tools/audit-type-system.sh`](../../../tools/audit-type-system.sh), which is a static gate in CI rather
than a review convention.

### 3.4 Close by construction

The findings register ([`spec/AUDIT.md`](../../../spec/AUDIT.md) over `spec/findings.tsv`) gives every
finding exactly one home. A row carries an id, the pass section that holds its full account, a state
(`todo` / `in progress` / `done`), and the law it is an instance of, or `candidate:<slug>` where it
proposes a law rather than instantiating one.

The **close condition** is the contribution: a row is `done` when the bad state it describes is no
longer *representable* in the code, and the row must carry the citation that forbids it — or it is a
recorded decision, which is a different and equally explicit outcome. A row with no law cell is
*unclassified*, and the emitter prints that count every run and refuses to let it grow. Two properties
follow: the register cannot silently drop an item, and a `done` row whose evidence resolves to nothing
is a build failure, not a stale comment.

Two rules keep the register honest as a record: a finding never spawns an issue by itself (an issue
needs a committed owner, and the register is the record), and a close-out is one PR, never a stack.

### 3.5 Gates that read the prose

Documentation drift is treated as a defect class, not a documentation problem. The prose that states a
register total must mark it —

```
... the register holds <!-- counts:laws -->91 laws<!-- counts:end --> ...
```

— and [`tools/emit-lean-counts.sh`](../../../tools/emit-lean-counts.sh) rewrites every marked span from
the register. The reason this draft is written the way it is: it scans a hand-maintained `FILES` list, and
the file you are reading is on it. A number stated *outside* that list is one nothing recomputes, which
is why the list has grown every time a stale count was found by hand. The appendix to the tool's own
header is a short history of exactly those misses.

### 3.6 Hazard study and adversarial rehearsal, under CI

The failure-mode HAZOP ([`docs/src/spec/failure-hazop.md`](../spec/failure-hazop.md)) drives all eleven
IEC 61882 guide words across 17 study nodes, producing 52 `F-U<n>-<m>` deviation rows, each with a
causal chain, a current behaviour, a class and a falsifier; the acceptance sheet adds a bow-tie whose top
events are named and numbered. The bounded-adversary tabletop
([`docs/src/spec/adversary.md`](../spec/adversary.md)) is 97 `W-N<n>-<m>` rows over 7 personas — Alice,
Bob, Eve, Mallory, Oscar, Root and the Operator — each row a persona acting a guide word against a study
node, with a decided verdict and an armed-or-owed falsifier.

The step past prior art: both worksheets are **checked artifacts**. `tools/check-wargame.sh` refuses a
row whose cell count disagrees with the page's own header, a row with no decided verdict or no falsifier,
a falsifier naming a path or symbol that does not exist in the tree, and a page that declares no referee
calibration. The rows are also *executed*: a single devnet boot drives the falsifiers that can be run,
and the runs are committed.

### 3.7 Evidence is committed, not cited

Every claimed run has a home under `spec/audit/evidence/` (44 suites). A citation to a `target/`
directory is unopenable and therefore not evidence. This is the practice that answers the reproduction
gap that verification-evidence methodology papers most often have to concede.

## 4. How the method fails

A method paper that reports only its successes is an advertisement. Each rule above was written *after*
a failure, and the failures are in the tree:

- **A gate that misses the instances it exists for is worse than none.** The counts scanner was
  line-based in its first version and matched one phrasing; it missed two of three stale claims in the
  file it was written to protect. It reads paragraphs now.
- **A register column nobody checks rots.** Eleven rows cited pass records that did not exist. The
  emitter now refuses a `section` that resolves to nothing.
- **A worksheet can be silently skipped.** A wargame row whose *shape* was wrong was invisible to every
  check the page had, and three rows went unseen until a later date.
- **An integration whose register rows are all `candidate:` slugs has no laws.** A fully-working
  subsystem can satisfy its own tests and still have no law written about it; the register makes that
  visible, and the visibility is the point.
- **A verification claim needs re-running.** A commit's "green" is not evidence. A stated constant is an
  unproved theorem until the gate has been run against the tree as it stands.

## 5. Evaluation — the one experiment this draft does not have

The question a reviewer will ask is the right one: **what did each layer buy, at what cost?** The answer
is a table in which each finding is attributed to the layer that surfaced it — proof, adversarial review,
HAZOP, wargame, fuzz, in-process test, live drill — with the cost of that layer, and the
Newcombe-style ratio (artifact size per defect found) computed per layer.

The data is partially there and the join is not written. Each register row names the pass section that
carries its account (`spec/findings.tsv`'s `section` column), and each pass section sits inside one of
the review programmes — the red-team series alone runs to pass 9 (§21). What does not yet exist is the
mapping from *pass section* to *layer*, and without it the register cannot answer the question:

1. Read the 141 `## N.` sections of [`spec/audit/passes.md`](../../../spec/audit/passes.md) and assign
   each to a layer.
2. Join `spec/findings.tsv` on its `section` column against that assignment (374 rows).
3. Weight by the size of each layer's artifact (laws + corpus cases; pass lines; worksheet rows; corpus
   files; test functions) to get a per-layer ratio.
4. Report the ratio **whether or not it flatters the proofs** — a layer that found nothing is a result.

The honest outcome may be that the proofs bought representation and the fuzzers bought defects. That is
worth publishing precisely because it is the thing practitioners want to know and almost never get told.

## 6. Threats to validity

- **n = 1, and no baseline.** This is one project's experience report, not a controlled study. No causal
  claim about the apparatus's efficacy is made anywhere above.
- **The trust base is large and named only in part.** The Lean kernel, the Coq kernel, the extractor,
  the Rust compiler, and the *laws themselves* are all trusted. "Who checks the laws?" is answered in
  this tree by the type discipline, the emitted corpora and the register — which is a mitigation, not a
  discharge, and it should be stated as one.
- **`TODO 0` is self-reported.** The register reads zero todo with one item in progress, and what that
  does *not* mean is recorded: worksheet falsifiers that are owed rather than run, and rows held against
  their own verdicts. A paper that omits those caveats is marketing.
- **Absence claims about the Scala reference are unverifiable here.** That tree is not in this
  repository. Comparisons are artifact-to-artifact — the fragility catalogue in
  [`spec/RUST-VS-SCALA.md`](../../../spec/RUST-VS-SCALA.md) §1 and the differential corpus — never
  process-to-process.

## 7. Conclusion

The port's correctness rests on an inverted authority: laws, not the implementation they came from. The
apparatus that holds that inversion — emitted corpora, a close-by-construction register, CI-gated hazard
worksheets, and committed evidence — is described here and its failure modes are catalogued with it.
The experiment that would turn this into a paper is §5's attribution table, and it is one afternoon of
register archaeology away.

## References

- `spec/INVENTORY.md`, `spec/TYPE-SYSTEM.md`, `spec/AUDIT.md`, `spec/findings.tsv`, `spec/audit/passes.md`
- `docs/src/spec/failure-hazop.md`, `docs/src/spec/adversary.md`, `docs/src/spec/testnet-acceptance.md`
- [`spec/README.md`](../../../spec/README.md) — the specification tree and its gates
- [Kaizen: Building a Performant Blockchain System Verified for Consensus and Integrity](https://dprg.cs.uiuc.edu/data/files/2019/FMCAD%20Slides.pdf) (FMCAD 2019)
- [Formal Methods for Blockchains (FMBC 2019)](https://link.springer.com/book/10.1007/978-3-030-54994-7?page=2) — the Algorand Coq model, Mi-Cho-Coq, and companion work
- [Making Tezos smart contracts more reliable with Coq](http://export-test.arxiv.org/pdf/2106.12973)
- Newcombe et al., *How Amazon Web Services Uses Formal Methods*, CACM 58(4), 2015
