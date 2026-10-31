/-!
# The law register — every law, in one place, with what each one rests on

**The heartbeat ceiling, and why it is raised here rather than the rows shortened** (2026-10-08). A row
is mostly prose — `statement`, `note`, `falsifiable` — held as string literals in one structure, and the
elaborator's cost grows with the register. Adding law 17c and 17d's rows took seven declarations past
Lean's 200,000 default and the build failed with `(deterministic) timeout at \`elaborator\`` (at
`Laws.lean:446, 460, 482, 610, 640, 774, 814` — declarations that had been inside the budget, so the
number that moved is the *register's*, not theirs). The ceiling is raised here instead of the prose
being trimmed because the prose **is** the record: a `note` cut to fit a compiler budget is a note that
stopped saying what it knows, and this file's own history is a list of the times a hand-written claim
drifted from the tree. The same tool, for the same reason, is already used in `Print.lean`
(`8000000`), `ValueDepth.lean` and `Depth.lean` (`4000000`) and `Par.lean` (`1000000`). The number
below is 5× the default and still an order of magnitude under the heaviest block in the tree; if it
ever needs raising again, that is a signal about this module's shape rather than about Lean.
-/
set_option maxHeartbeats 4000000

/-!

`spec/INVENTORY.md` is the prose catalog, and `docs/src/formal/laws.md`
— the folder's one-page rendering of the whole set — is its reader-facing counterpart, but
none of the three is *checkable*:
nothing noticed that the tree grew to 43 laws while both still
said 29, that the two tables contradicted each other on Laws 5 and 24, or that Law 1's "30 residual
axioms" were really 12. This module is the single source of truth those documents are generated from,
and `Rchain/LawsMain.lean` (the `rchain-laws` executable) is what enforces it:

1. **Numbering** — every law `1..lawCeiling` is present, with no gaps, so a law cannot be quietly
   dropped.
2. **Reference integrity** — every declaration a row names actually exists in `Rchain`. A renamed or
   deleted theorem fails this, instead of leaving a row that cites a proof that is gone.
3. **Axiom accounting** — the set of axioms cited by these rows is *exactly* the set of `axiom`
   declarations in the tree. This is the ratchet the `sorry` scan cannot be: `tools/check-lean-conformance.sh`
   catches a new `sorry`, but an `axiom` is how a law is usually assumed, and until this register existed
   a 63rd axiom was invisible.

## The status vocabulary, and why it is not "proven / stated"

The old vocabulary (`spec/INVENTORY.md`'s legend) had one word for two very different things. A law
proved *about the Lean model* and a law proved *and checked against the running node* both read
"proven", and the difference between them is exactly the difference that let C21 ship: the model's
`normalize_if` was never the question — the node's was.

- `provedTied` — proved, **and** tied to the node by a conformance corpus whose Rust consumer runs the
  same source text through the real thing (`spec/conformance/*.tsv`). The corpus is the only mechanical
  Lean↔Rust link this repo has. Check 5c requires one (a status that names a tie nothing checks is the
  word `proved-tied` with no tie behind it) and `corpusLayerFailures` requires the layer the row names to
  exist, so the two halves are enforced at the two levels they can be.
- `provedModel` — proved over the model; the tie to the Rust is prose in a mapping table. Honest, and
  weaker than it sounds: a `provedModel` law is a claim about a model that a human keeps in sync. A row
  may carry a corpus here too and still be `provedModel` — when the layer is *partial*, the model's
  algebra coarser than the node's — and then its note says why the corpus is not the tie the stronger
  word would claim.
- `axiomByDesign` — postulated because the primitive is cryptographic (the crypto law). The only status
  that should survive the work this register begins.
- `owed` — the definition exists and the proof does not.
- `deferred` — the `axiom` *is* the definition, so there is nothing yet to prove anything about.
  **Empty now**: the last examples (`substPar`, `joinKey`, `trieRoot`, `mergeChanges`) are definitions
  since the consolidation pass modelled the Rust's own.
- `open` — in the catalog, and its formalization is **not yet statable**: unlike `owed`, a definition is
  missing rather than a proof.
- `orphaned` — out of scope because the VM it describes was not ported.
- `retired` — the port has **no rule of this shape**, with the evidence in the row's note and its `rust`
  anchor (the code that was read). A decision recorded, not an omission: the alternative is an `open` row
  that will never close, which makes the count dishonest in the direction that matters least visibly.

**No bullet names law numbers, and that is a rule with a check behind it.** The lists that stood here
("(laws 30, 31, 33, 34, 36)" for `open`, "(laws 12, 13)" for `orphaned`) were wrong in four of five
entries by 2026-09-25 — every one of those rows had moved on — which is `spec/STYLE.md`'s own rule ("do
not restate a status in prose") broken in the one place a reader looks to learn the vocabulary.
`tools/audit-test-register.sh`'s check 12 now fails on a law number in a status bullet: the counts and the
per-row statuses live in `spec/laws.tsv`/`spec/LAWS.md`, which are emitted, and a reader who wants the
examples should read those.

`falsifiable` records what would have to be true for the law to be *false* — a witness, a negative case,
or the reason it cannot fail. A law that cannot fail constrains nothing: `numeric_channels_nonneg` was
`0 ≤ b.number` on a `Nat` (`Nat.zero_le`), and `finality_iff_supermajority` restated its own definition
(the pass has since deleted the first and re-scoped the second onto the finalizer's gate).
`none` means the witness is owed, and the count of those is the measure of how much of this catalog is
still unfalsifiable.

`witness` is `falsifiable`'s machine half — the Lean declarations the claim rests on, checked by name
exactly as `declarations` is. `falsifiable` itself is a sentence that mixes three kinds of name: Lean
declarations, Rust property tests (`an_open_value_at_the_variable_leaves_a_free_variable` in
`rholang/src/property_tests.rs`; `a_rigged_replay_matches_its_recorded_trace` in
`rspace/src/property_tests.rs`), and `file:line` references. Nothing read any of them, so a `falsifiable`
that had rotted — naming a theorem since renamed, or one never written — would have read exactly like a
live one. `witness` is the part that can be checked; a `proved*` row must name at least one declaration
or carry a `corpus`, and no witness may be an `axiom` — a row whose falsifier is the assumption that
cannot fail is naming decoration.
-/

-- The register is data, and `Name` is the one type it needs from outside the prelude. `autoImplicit`
-- off because the alternative is what happened on this file's first draft: `List Name` silently became
-- an *implicitly bound* `Name✝`, so the structure was accepted with a field type nobody wrote. A file
-- whose whole purpose is to be checkable should not accept a typo as a binder.
set_option autoImplicit false

namespace Rchain
namespace Laws

/-- What a law's proof is worth. See the module doc — this vocabulary is the point of the register. -/
inductive Status where
  /-- Proved, and tied to the node by a conformance corpus. -/
  | provedTied
  /-- Proved over the Lean model; the model↔Rust tie is prose, not machine-checked. -/
  | provedModel
  /-- Postulated by design: a cryptographic primitive (Law 19). -/
  | axiomByDesign
  /-- The definition exists; the proof is missing. -/
  | owed
  /-- The `axiom` is the definition — nothing to prove yet. -/
  | deferred
  /-- In the catalog; no formalization. -/
  | open
  /-- Out of scope: the VM it describes was not ported. -/
  | orphaned
  /-- **Proved, but the statement restates its own definition and so cannot fail.** This is the status
  the consolidation pass exists for: statements that are true and useless. Four have been removed under
  it — `next_step_closure_computable` (a `rfl`), `validated_speculation_refines_apply` (a disjunction
  whose second arm held for any run), `gate_replay_terminates` (`∃ st', f st = st'`, which is totality of
  a Lean function), and `finality_iff_supermajority` (two `Nat.mul_comm`s away from `isSuperMajority`'s
  own body, now re-scoped onto the finalizer's gate). Two remain, each with the re-scoping it needs
  written in its note. Calling such a row `provedModel` would be true and useless; `vacuous` says the
  proof is real and the *law* is not yet, and the note requirement is what stops the word becoming a
  resting place. -/
  | vacuous
  /-- **Retired: the port has no rule of this shape to state the law against**, evidenced rather than
  assumed. Distinct from `orphaned` (a VM that was not ported) and from `open` (work pending): a
  `retired` row is one whose own investigation found nothing in the code for the law to be *about* — the
  merge "does not choose among candidates", so a law about a unique minimum-cost candidate has nothing to
  be stated against, and the honest close is to say so rather than leave the row owed forever. The
  requirement is what keeps the word from becoming a resting place: a `retired` row must carry the
  evidence in its note **and** a `rust` anchor naming the code that was read, because the claim being
  made is a claim about that code. Retiring is a *decision taken*, recorded — the distinction this
  register exists to keep. -/
  | retired
  deriving BEq, DecidableEq, Repr

def Status.wire : Status → String
  | .provedTied    => "proved-tied"
  | .provedModel   => "proved-model"
  | .axiomByDesign => "axiom-by-design"
  | .owed          => "owed"
  | .deferred      => "deferred"
  | .open          => "open"
  | .orphaned      => "orphaned"
  | .vacuous       => "vacuous"
  | .retired       => "retired"

/-- One law, or one clause of a law whose clauses differ in status or in what they rest on. Law 16
carries four clauses and Law 1 two, because "Law 1 is proved, residually 30 axioms" was the sentence
that hid the truth (it is 12). -/
structure Law where
  /-- The law number: 1–43. A law with several clauses repeats its number. -/
  number : Nat
  /-- The clause letter — `""` for a single-clause law, else `"a"`, `"b"`, … -/
  clause : String := ""
  /-- Which layer of the system the law belongs to. -/
  layer : String
  /-- The law, in one line. The long form is `spec/INVENTORY.md`'s. -/
  statement : String
  status : Status
  /-- The declarations this law's status rests on: its headline theorem(s) or definition(s). -/
  declarations : List Lean.Name := []
  /-- The axioms this law rests on. The union over all rows must equal the tree's axiom set. -/
  axioms : List Lean.Name := []
  /-- The conformance corpus layer that ties this law to the node, if there is one. -/
  corpus : Option String := none
  /-- **The Rust this law models**, as `path` or `path:line` anchors — the field that makes "the model
  models the code" checkable rather than promised. `Rchain/LawsMain.lean` refuses a row that claims a
  model (`proved-tied`, `proved-model`, `axiom-by-design`, `vacuous`) without one, and refuses any
  anchor whose file does not exist. The anchor names the *code*, not the test: where a conformance
  corpus exists it is already in `corpus`, and where a property test exists it is named in the note. -/
  rust : List String := []
  /-- **The Rust *tests* this law's claim rests on**, as `path.rs:symbol` anchors — the Rust
  counterpart of `witness`, and the rung between a conformance corpus and prose. `witness` is checked
  by name against the elaborated environment and nothing read the Rust names in `falsifiable`/`note`,
  so a test that asserts a row's own case was indistinguishable from a sentence about one; an anchor
  here must be a file that exists **and contains `fn <symbol>`** (`Rchain/LawsMain.lean`'s
  `rustAnchorFailures`), and `tools/check-rust-witnesses.sh` **runs** each one — refusing a name that
  matches no test, because `cargo test <filter>` exits 0 on an empty match and a registry of renamed
  or `#[ignore]`d tests would otherwise run green while checking nothing.
  What it is *not*: a witness is not a proof. A corpus outranks it (three verdicts read off the node
  beat one assertion), and a row whose statement is over an abstract relation the Rust does not have
  says so in its note rather than implying the witness covers it. Populated 2026-09-24 from the 65
  tests a body-read classified as asserting a row's own case; the law-name scan that preceded it
  could not see nine of them, which is why the rule is *grep for the behaviour, not the law number*. -/
  rustWitness : List String := []
  /-- **The Coq declaration(s) this law is stated over**, as `spec/coq/<file>.v:<symbol>` anchors. The
  file must exist and the symbol must occur in it; the check is deliberately that, and not "is it a
  proof", because for most of this catalog's Coq half the honest answer is *no* — `Laws.v` states laws
  2–6 as axioms. Which of them are axioms is counted and printed by step 4b of
  `tools/check-lean-conformance.sh`, whose ceiling is the ratchet; this field's job is to stop the
  register's prose about Coq ("`Laws.v` has `substPar`") from being the only thing that names it. Empty
  on every row that makes no Coq claim, which is most of them. -/
  coq : List String := []
  /-- What would have to hold for this law to be false: a witness, a negative case, or why it cannot
  fail. `none` = owed. -/
  falsifiable : Option String := none
  /-- **The machine half of `falsifiable`**: the declarations the falsifiability claim rests on — a
  refutation (`*_is_false`), a negative case (`a_*`, `*_refuses_*`), a strictness or minimality fact, or
  the theorem that carries the claim where the law is a positive property with no failing instance in the
  model. Every name is checked against the elaborated environment, exactly like `declarations`; a
  `provedTied`/`provedModel` row must name at least one, or carry a `corpus` (the corpus *is* the
  falsifier for a tied row); and no name may be an `axiom` — a falsifier that cannot fail is not one.
  The standard is deliberately "the claim is anchored", not "a counterexample exists": a law of the shape
  `f (step p) ⊆ f p` has no failing instance *inside its own model*, so demanding one would produce
  filler witnesses, which is the failure mode the `vacuous` status exists to prevent. -/
  witness : List Lean.Name := []
  /-- A note where the status needs qualifying. -/
  note : String := ""

-- **The budget is raised for these literals, measured.** Elaborating a register chunk is a unification
-- problem over the rows' field types rather than a proof: one list fit the default 200 000 heartbeats at
-- 58 laws and not at 59, so the budget is raised here. The chunks are `lawsA`/`lawsB`/`lawsC` for a
-- *memory* reason recorded above them (19.1 GB as one literal against the 16 GB CI runner); the duplicate
-- they were once feared to hide is caught by the numbering and entry-ceiling checks, which is why the
-- reason to keep one literal retired with the measurement.
set_option maxHeartbeats 4000000

/-- Every law in the catalog, the orphaned ones and the open ones included, because a register
that lists only the formalized laws cannot notice a law that was dropped.

Statements are the one-line form; `spec/INVENTORY.md` carries the long form and the Rust realization.
`falsifiable` is `none` where the witness is owed, which is most of the non-corpus laws: naming that gap
is the purpose, not a defect of this file. -/
-- **Three chunks, not one literal.** The whole register elaborated at 19.1 GB peak
-- (measured 2026-10-25, rchain-laws build), over the 16 GB CI runner — the SIGTERM of
-- #417, now deterministic. Each chunk is elaborated on its own; the concatenation is a
-- value. A duplicate cannot hide between chunks: the numbering check requires every number
-- 1..`lawCeiling`, and `entryCeiling` pins the row count, so a repeated row leaves a
-- number missing (the reason to keep one literal retired with that argument).
def lawsA : List Law := [
  -- ── Rholang: the language (Laws 1–6) ────────────────────────────────────────────────────────────
  { number := 1, clause := "a", layer := "Rholang",
    rustWitness := [
      "models/src/property_tests.rs:law1_sorting_is_idempotent",
      "models/src/property_tests.rs:law1_parallel_composition_sorts_commutatively",
      "models/src/property_tests.rs:law2_sorting_a_sequence_depends_only_on_its_elements"],
    statement := "`Par`/`ESet`/`EMap` are commutative and canonicalization is idempotent and \
      commutative: `sort (sort p) = sort p`, `sort (p | q) = sort (q | p)`",
    status := .provedModel,
    declarations := [`Rchain.sortPar_idempotent, `Rchain.sortPar_comm, `Rchain.sortPar,
      `Rchain.parMerge],
    corpus := some "sort",
    rust := ["models/src/sorter.rs"],
    axioms := [],
    coq := ["spec/coq/Sort.v:sortPar_idempotent", "spec/coq/Sort.v:sortPar_comm"],
    falsifiable := some "`sortPar_idempotent`/`sortPar_comm` are theorems; `spec/INVENTORY.md`'s Law 1 \
      claim of idempotence is falsified by any leaf type whose comparator is not a total order — see \
      clause b, where exactly that is assumed rather than proved. The `sort` corpus is the tie: \
      `conformance/sort.tsv`'s 50 verdicts, each `decide`d against the model's `cmpPar`, read back from \
      the node by which element `sort_par` puts first (`rholang/tests/lean_sort_corpus.rs`)",
    note := "`sortPar_idempotent` is proved only *for* a comparator whose element laws hold; the \
      element-law half is clause b, and it is axioms. **The `sort` corpus found a divergence on its \
      first run, and it is now aligned and pinned** (2026-09-23): the model's comparators ordered by \
      *declaration* order while the node sorts by its **score tree** \
      (`models/src/sorter.rs`'s `sort_send`/`sort_expr` build `node_score(tag, children)` with the tags \
      in `BOOL=1 < INT=2 < STRING=3 < ELIST=6 < ETUPLE=7 < ESET=8 < EMAP=9 < … < BOUND_VAR=50 < … < \
      EVAR=100 < …`), and a reading of every `sort_*` against the model showed the divergence across \
      **eleven types** rather than the two the corpus first exhibited. Four structures were reordered to \
      the tags and are pinned by corpus rows 13–19, each verdict *observed* from the node (via \
      `sort_pars`) and the model corrected to it: a send's field order (`persistent` first — \
      `@\"a\"!!(1)` vs `@\"b\"!(1)` → `gt`), the par's own field order (`exprs` before `news`, \
      `unforgeables` last — `new x in { Nil } | 1` vs `[1]` → `lt`, and its reverse → `gt`), the \
      expression-class order (collections before vars and operators — `[1]` vs `1 + 2` → `lt`, `1 * 2` \
      vs `1 + 2` → `lt`, `1 - 2` vs `1 * 2` → `gt`), and `GBool`'s **polarity** (`true` scores 0 and \
      `false` 1, so `true` sorts first — `false` vs `true` → `gt`). Each row was falsified before it was \
      believed: restoring the old order stops `Rchain.Corpus`'s `sortCases_decide` from compiling, and \
      a runtime reporter names the row. **The rest could not be aligned without extending the model's \
      algebra** — 24 `Expr` constructors against the node's 33 when this was written, and the three the \
      re-tagging added were `ematches`/`eshortand`/`eshortor` — so it was a recorded boundary rather \
      than a fixed defect: `Receive`/`ReceiveBind`/`New`/`Bundle`/`EList`/`ESet`/`EMap`/`Var`/\
      `GUnforgeable` (where `gDeployerId`(10) sorts *before* `gDeployId`(11), the reverse of the port's \
      enum) / `Connective`; and the model then lacked **eight** `Expr`-level constructors, *counted* against \
      the node's tag table rather than summarised (AUDIT C58): `BIG_INT`(13), `EMETHOD`(115), \
      `EMATCHES`(118), `EPERCENT`(119), `EPLUSPLUS`(120), `EMINUSMINUS`(121), `ESHORTAND`(123), \
      `ESHORTOR`(124). **All eight are in the model as of 2026-09-25** — three were conflations \
      (below) and the other five were constructors (further below) — so the count is closed. **The
      decimal class (`edecimal`, tag 14) joined on 2026-10-19**, so the model's 30 classes cover all
      34 of the node's variants, and rows 43–50 pin it. **Three of those eight were *conflations* rather than missing arms, and all \
      three are closed** (re-measured 2026-09-25, and this sentence is the correction): `EMATCHES`, \
      `ESHORTAND` and `ESHORTOR` have constructors (`ematches`, `eshortand`, `eshortor`, \
      `Par.lean:67-69`), `Surface.lean`'s arms now **emit** them (`.eshortand`, `.eshortor`, \
      `.ematches` — they used to normalise `.matches`/`.shortAnd`/`.shortOr` onto `eeq`/`eand`/`eor`, \
      which made the model answer **`eq`** where the node's tag table says **`gt`**, a wrong verdict \
      on a statable pair), and `Rchain/Sort.lean`'s `exprTag` gives them the node's own tags — 118, \
      123 and 124. So the defect this paragraph described is *gone*, and what it left behind is the \
      boundary below. The genuinely missing five were `BIG_INT`(13), `EMETHOD`(115), \
      `EPERCENT`(119), `EPLUSPLUS`(120) and `EMINUSMINUS`(121), **and they are in the model as of \
      2026-09-25**: each is its own `Expr` class with the node's own tag, and corpus rows 26–42 pin \
      every one of them (each verdict read off the node before the row was written). Line them up and \
      the reason they were not \"five more arms\" is structural, and the fix is that structure rather \
      than more arms: the node's tags *interleave* (scalars 1–4, collections 6–9, `BIG_INT` 13, vars \
      50–52, operators 100–124, `EBYTEARR` 116) while a constructor-*kind* classification would put a \
      bigint with the other grounds (tag 0, ahead of `elist`(6)) and `EMETHOD`/the three operators \
      after every operator — so each of the five is a class of its own, and `exprTag` gives it the \
      node's literal. The two consequences the earlier note predicted are the two that landed: \
      `cmpExpr`'s arm-lemma family grew from 24 classes to 29 (5 pair lemmas, 5 extraction lemmas, \
      10 cross lemmas, and five arms in each of the three laws); the decimal took it to 30 (one more \
      pair lemma, one extraction lemma, two cross lemmas, and one arm in each law, rows 43–50), and \
      `emethod`'s score needed \
      `Rchain.Comparator.lex_lt_trans_at3` because its children are three deep. C58 carries the measurement of \
      the *gap*; the rows carry the closure. `Ground` is untouched and `ebigint` is an `Expr` leaf \
      because that is what the node has — the protobuf's `g_big_int` is an `Expr` variant and the \
      port's normalizer converts the parse-level ground into one (`normalizer.rs:74-77`). (`matches` was the \
      sixth of that list and is no longer on it: `ematches` has a constructor, a tag and an arm.) \
      `Ground.bytes` is the one \
      unpinnable divergence: the model has it at the wrong tag and the node's front end has no \
      byte-array literal, so the term is unspellable rather than merely mis-scored. `Rchain/Sort.lean`'s \
      note carries the table and the boundary. So the model's `sortPar` is now the node's `sort_par` over \
      the model's algebra rather than a claim about the wrong order (it was never a fork between nodes: \
      every node sorts by the score tree). The alignment is safe for the proofs: \
      `sortPar_idempotent`/`sortPar_comm` hold for any comparator, and \
      `eq_iff`/`swap`/`lt_trans` are order-independent, so the remaining axioms' statements do not change. \
      **What the tie does not claim (2026-10-19).** The model's `edecimal` payload is the canonical \
      scaled integer (`coefficient × 10^(38−scale)`), so the comparator laws are stated over that \
      representation and over `Int` order. The bridge from the Rust `(coefficient, scale)` pair to \
      that integer — `scaled_value`'s injectivity and order-preservation on canonical values — is \
      **not** a theorem here, and neither are canonicalization, exact `+ - * /`, division's \
      exact-or-error condition, the wire's minimal-byte form, the reducer's dispatch, or the cost \
      model. Those are carried today by the Rust refinement (`Decimal::new`/`from_wire`, private \
      fields) and its unit and corpus tests; the standalone `Rchain/Decimal.lean` plus the `decimal` \
      corpus is **Law 87** (87a–e), whose boundary is the same: wire bytes/tag 35, serde ingress, the \
      literal surface, reducer dispatch and the cost model remain Rust-tested, not modelled" },
  { number := 1, clause := "b", layer := "Rholang",
    rustWitness := ["models/src/property_tests.rs:law1b_every_permutation_of_a_collection_sorts_alike"],
    statement := "Each element comparator (`cmpPar`, `cmpSend`, …, `cmpConnective`) is a lawful total \
      order: `eq_iff`, `swap`, `lt_trans`",
    status := .provedModel,
    declarations := [`Rchain.cmpPar, `Rchain.cmpSend, `Rchain.cmpExpr, `Rchain.cmpNew_lt_trans,
      `Rchain.cmpSend_lt_trans, `Rchain.cmpReceiveBind_lt_trans, `Rchain.cmpReceive_lt_trans,
      `Rchain.cmpMatchCase_lt_trans, `Rchain.cmpMatch_lt_trans, `Rchain.cmpBundle_lt_trans,
      `Rchain.cmpConnective_lt_trans, `Rchain.cmpListPar_lt_trans,
      `Rchain.Comparator.lex_lt_trans, `Rchain.Comparator.lex_lt_trans_at,
      `Rchain.Comparator.lex_lt_trans_at3, `Rchain.Comparator.cmpPairF,
      `Rchain.exprTag, `Rchain.cmpExpr_eq_iff, `Rchain.cmpExpr_swap, `Rchain.cmpExpr_lt_trans,
      `Rchain.cmpExpr_tag_lt, `Rchain.cmpExpr_tag_gt, `Rchain.cmpExpr_ground, `Rchain.cmpExpr_elist,
      `Rchain.cmpOptionVar_eq_iff, `Rchain.cmpOptionVar_swap, `Rchain.cmpOptionVar_lt_trans,
      `Rchain.exprTag_le_of_cmpExpr_lt, `Rchain.cmpListExpr_lt_trans],
    corpus := some "sort",
    rust := ["models/src/sorter.rs"],
    axioms := [],
    coq := ["spec/coq/Sort.v:cmpPar"],
    falsifiable := some "eight of the ten element `lt_trans` laws are theorems now \
      (`cmpNew`/`cmpSend`/`cmpReceiveBind`/`cmpReceive`/`cmpMatchCase`/`cmpMatch`/`cmpBundle`/\
      `cmpConnective`), each by the `Comparator.lex_lt_trans` idiom the file's own note validates, and \
      the list laws are proved from them, and `cmpPar_lt_trans` joined them on 2026-09-24 — so a \
      counterexample would have to be a counterexample to those proofs too. **And the residual is \
      gone**: `cmpExpr`'s three are *theorems* now (2026-09-24), which is what the second half of this \
      cell used to be blocked on — the size of \
      its equation lemmas; the route is the tag-first definition (compare `exprTag`, then the \
      same-shape comparator) and its 30 `@[simp]` pair lemmas, each \
      `simp only [cmpExpr.eq_def, exprTag, Nat.reduceLT, ite_false, ite_true]`",
    note := "**no axioms, from twelve — the residual is empty** (2026-09-24). The list comparators' laws \
      were discharged by induction on the list; the eight element laws above are theorems, in dependency \
      order (an element law needs the list lemma of the types *below* it and a list lemma needs the \
      element law of its own type, so the two families interleave — the order in `Rchain.Sort` is that \
      earlier by induction on the list; the eight element laws above are now theorems, in dependency \
      order (an element law needs the list lemma of the types *below* it and a list lemma needs the \
      element law of its own type, so the two families interleave — the order in `Rchain.Sort` is that \
      topological order, and it is why the section is not alphabetical). **The last two landed on \
      2026-09-24**: `cmpPar_lt_trans` is a theorem — an eight-component `lex` chain, a member of \
      `Rchain.Sort`'s `mutual` block because the family is one strongly connected component, which is \
      what that file's note is about — and `cmpExpr`'s three are theorems by the route that note \
      predicted (the tag-first definition and its 30 `@[simp]` pair lemmas, since `cmpExpr` is compiled \
      as `WellFounded.fix` and the arm lemmas are what avoid unfolding it). This row's falsifier is \
      therefore the **`sort` corpus**, \
      as it is law 1a's: its pairwise verdicts are read back from the node by which element `sort_par` \
      puts first, so a comparator whose order drifted from the node's score tree fails a row rather than \
      a proof — which is exactly how the alignment was found in the first place (law 1a's note). The \
      `rust` anchor is \
      `models/src/sorter.rs`, because the order these comparators specify *is* the node's score tree \
      (`node_score(tag, children)`, compared element-wise by `compare_children`), and the `sort` corpus \
      pins four structures of it against the node — see law 1a's note. **What the fourth pass did \
      (2026-09-24)**: `cmpPar_lt_trans` is a **theorem** — the seven-deep ladder whose shape an earlier \
      note recorded — and it is a member of `Rchain.Sort`'s `mutual` block rather than a theorem of its \
      own, because the family is one strongly connected component. What blocked the first attempt is \
      kept in that file's note with the fix: `Cmp.lean`'s pointwise `lex_lt_trans_at`, so each level's \
      `h_lt` is a partial application on *fields* — the only shape the block's termination checker \
      accepts — plus the last-component and binder-collision details. **What the fifth pass did \
      (2026-09-24)**: `cmpExpr_eq_iff` is a **theorem**, and the finding is why it took two attempts — \
      `cmpExpr` is compiled as `WellFounded.fix` (its block is one SCC), so *nothing* unfolds it: `rfl` \
      does not reduce it on constructors, `cmpExpr.eq_def` times out at `whnf`, and a 441-goal \
      `simp [cmpExpr]` overflows the stack. The access path is one **arm lemma at a time** \
      (`simp only [cmpExpr.eq_def]` under a stated heartbeat budget) plus a new `exprTag` numbering the \
      arm order for the cross cases — whose side conditions need `Nat.reduceLT` in the `simp only` set. \
      And the law has to be written **one arm at a time too, calling each sub-law on the arm's own \
      pattern variables**: a `decreasing_by` over the 441-goal `simp` version cannot work, because \
      `simp` picks the call arguments and the termination checker is then asked for \
      `sizeOf p < 1 + sizeOf a` with nothing identifying `p`. What is left of this row is `cmpExpr`'s \
      none. `lt_trans` needed the tags rather than 9261 cases, and the arm machinery made \
      `eq_iff`/`swap` ordinary case analyses" },
  { number := 2, layer := "Rholang",
    rustWitness := [
      "models/src/property_tests.rs:law2_unequal_canonical_forms_do_not_hash_alike",
      "models/src/property_tests.rs:law2_canonical_equality_agrees_with_canonical_hashing"],
    statement := "α/name equivalence = par order + `| Nil` + top-level arithmetic + α + added \
      eval/quote",
    status := .provedModel,
    declarations := [`Rchain.StrCong, `Rchain.strCong_equivalence, `Rchain.strCong_comm,
      `Rchain.strCong_assoc, `Rchain.strCong_ident, `Rchain.strCong_nil_left],
    rust := ["models/src/ast.rs"],
    coq := ["spec/coq/Laws.v:alpha_equiv", "spec/coq/Laws.v:alpha_equiv_refl"],
    witness := [`Rchain.reduce_not_deterministic],
    falsifiable := some "`reduce_not_deterministic` (`Rchain/Concurrent.lean`) exhibits two distinct \
      reductions of one term, which is what makes `≡` — rather than syntactic identity — the relation \
      reduction needs",
    note := "**nobody owns “deep α”, and nobody needs to** (2026-09-23): with de Bruijn *levels* the \
      representation is canonical, so deep α-equivalence *is* equality — what is left is the *structural* \
      congruence (`P | Q = Q | P`, `P | Nil = P`, associativity, congruence), which is what `StrCong` \
      states and what the Coq mirror states. That mirror is an `Inductive` over `parMerge`/`nilPar` now, \
      with `refl`/`symm`/`trans` as **constructors** rather than axioms, a weight invariant \
      (`alpha_equiv_weight`) and a non-vacuity witness (`a_send_is_not_alpha_equiv_to_nil`). The note \
      this replaces said the deep-α half was Coq's obligation; no track can deliver it and none needs it" },
  { number := 3, layer := "Rholang **A witness was added on 2026-09-27 (AUDIT C139): the converse, without which the hash half of this law was not falsifiable.** The equality-facing witness asserts `equal => same hash`, which a *constant* `Hash` satisfies — measured: an empty `Hash for Sorted` body left both of law 2's witnesses green and the whole `rchain-models` suite green (168 tests). `law2_unequal_canonical_forms_do_not_hash_alike` states the other direction, and it is the one that fails under that mutation.",
    rustWitness := [
      "rholang/src/substitute.rs:substitutes_bound_var",
      "rholang/src/property_tests.rs:an_open_value_at_the_variable_leaves_a_free_variable"],
    statement := "Capture-avoiding de Bruijn substitution; `sort (subst t) = subst (sort t)`, and \
      substitution preserves closedness **given a closed image**",
    status := .provedModel,
    declarations := [`Rchain.substPar, `Rchain.substSend, `Rchain.substReceive,
      `Rchain.substReceiveBind, `Rchain.substNew, `Rchain.substMatch, `Rchain.substMatchCase,
      `Rchain.substBundle, `Rchain.substConnective, `Rchain.substListConnective,
      `Rchain.substExprsToPar, `Rchain.substExprToPar, `Rchain.substListPar,
      `Rchain.substListParPair, `Rchain.oneExpr, `Rchain.noSubst,
      `Rchain.the_identity_satisfies_sort_subst, `Rchain.the_identity_satisfies_subst_closed,
      `Rchain.bound_is_closed_free_is_not, `Rchain.sort_subst, `Rchain.subst_closed,
      `Rchain.sortPar_subst, `Rchain.sortListPar_subst, `Rchain.sortPar_parMerge,
      `Rchain.sortList_append_congr, `Rchain.sortList_cons, `Rchain.sortList_map_congr],
    rust := ["rholang/src/substitute.rs"],
    coq := ["spec/coq/Laws.v:substPar", "spec/coq/Laws.v:subst_commutes_sort"],
    witness := [`Rchain.the_identity_satisfies_sort_subst, `Rchain.the_identity_satisfies_subst_closed, `Rchain.bound_is_closed_free_is_not],
    falsifiable := some "`the_identity_satisfies_sort_subst` and \
      `the_identity_satisfies_subst_closed`: `noSubst` — the function that substitutes *nothing* — \
      satisfies both laws, so the trio is satisfied by a substitution that does not substitute; that \
      is the vacuity, published as theorems rather than asserted. The narrowed law's boundary is \
      pinned from the code side by `an_open_value_at_the_variable_leaves_a_free_variable` \
      (`rholang/src/property_tests.rs`), which holds exactly because the port's substitution is *not* \
      closed-preserving for an open image",
    note := "**`substPar` is a definition now** (2026-09-23, Programme D unit 5): a `mutual` family \
      mirroring `substitute_par_no_sort` and the arms it calls (`rholang/src/substitute.rs:116-306`) — \
      sends, receives and their binds, `new`, match cases, bundles, connectives and expressions, with \
      the list walks the port writes as loops, the splice a substituted occurrence performs \
      (`par_concat`, imported at `substitute.rs:13` and defined in `models/src/par_ops.rs:143`, \
      called in-file at `substitute.rs:56` and `:73`, `:160`, `:161`), the depth gate (`0` \
      substitutes, `d ≥ 1` is a pattern position, \
      `maybe_substitute_var` `substitute.rs:24-40`), and the set/map children sorted inside the recursion \
      (`sort_pars` `substitute.rs:437`, `sort_pairs` `:454`). Three differences from the port are *representational* and are written up in the \
      file: the model's `Var` is level-based so there is no environment `shift` to do under binders \
      (`env.rs:36-41`), `σ` is total where the port's `Env` is partial and errors on a \
      `FreeVar`/`Wildcard` at depth 0, and the port's bundle-of-bundle merge has no model counterpart. \
      **What that buys**: the two laws are now statements about a *function*, so they are falsifiable \
      instance by instance rather than postulates over nothing — the earlier vacuity is gone \
      (`noSubst`, `the_identity_satisfies_*` are kept for the record: they show the laws *alone* never \
      identified substitution, which is why the definition had to be written). **What is owed**: both \
      laws are still axioms, the proofs being the mutual induction over the twelve functions plus the \
      list facts; the closedness one carries the closed-image hypothesis, which the previous version \
      **lacked the hypothesis that makes it true**: `Closed` is `Ty.lean`'s \"no `Var.free`\" \
      (`closedVar` counts `bound` as closed), so for `σ` mapping a bound variable to an open term the \
      conclusion is false of any operation that actually substitutes \
      (`bound_is_closed_free_is_not`). The Rust's own test carries exactly that hypothesis \
      (`law3_substituting_a_closed_value_keeps_the_term_closed`, whose value is `arb_closed`), so the \
      statement is narrowed to `∀ v, Closed (σ v) → Closed t → Closed (substPar σ t)`. What closes \
      this row is Programme D's unit 5: define `substPar` by mirroring the code the law describes \
      (`rholang/src/substitute.rs:164` — an `Env<Par>` keyed by de Bruijn level with a shift, and a \
      `depth` incremented inside receive and match-case *patterns*), at which point both laws become \
      theorems about a definition. **And that is what happened (2026-09-24)**: the closedness law is a \
      *theorem* now — `subst_closed`'s statement unchanged, resting on a `mutual` **theorem** block over \
      the substitution family (21 members, one per helper) that states the checker-level facts \
      (`closed … = true`), with `closed_eq_Closed` carrying it back to the `Closed` form the law is \
      written in. The route this note predicted — a strong induction over the sum type — was **not \
      needed**: the mutual-theorem idiom `Sort.lean`'s comparator laws use carries this family too, and \
      the obstruction the earlier attempts hit was a *shape*, not a measure (the `termination_by` clause \
      must name a prefix of the equation patterns, so the members use variable patterns with the \
      destructuring inside). It needs `set_option maxHeartbeats 1000000`, measured. **And the commuting \
      law is proved too (2026-09-24)**: `sort_subst` is a *theorem* now, statement unchanged, resting on \
      a `mutual` block of 22 members — one per helper of the substitution family. What it needed beyond \
      the same induction is three shapes `Sort.lean` did not have: `sortList_append_congr` (a \
      substituted occurrence is spliced in by `parMerge`, so `sortList` of an *append* has to be a \
      function of the two sorts), `sortPar_parMerge` (the merge congruence), and `sortList_cons` (the \
      sorted form of a field is an `orderedInsert`, which an induction has to take apart); plus \
      permutation invariance of the two `parMerge` folds (`substExprsToPar`, `substListConnective`), \
      since `sortPar` sees a `parMerge` tree only as its sorted fields. The part worth keeping is a \
      *failure*: the ten list members cannot be a single application of `sortList_map_congr`, because \
      that applies the element law to an *arbitrary* element of the list and no termination measure \
      justifies it — Lean rejects the block — so they are structural inductions whose only two calls, \
      the head's element law and the tail's theorem, are subterms. This row has **no axioms of its \
      own**: `sort_subst` was the last one, and what the row's theorems still rest on is `cmpExpr`'s \
      three, which law 1's row owns and counts" },
  { number := 4, clause := "a", layer := "Rholang **The witness column changed on 2026-09-27 (AUDIT C138), because both names it carried were vacuous.** `law3_substituting_a_closed_value_keeps_the_term_closed` asserts closedness before and after the substitution — which the *identity* substitution satisfies — and `law3_substitution_and_sorting_commute` reduces to `sort p = sort p` under the same mutation. Measured: `substitute_par` returning its input leaves both green, while the two now named go red. The right pointer was already in this row's `falsifiable` cell (`an_open_value_at_the_variable_leaves_a_free_variable`); the witness column is where a reader looks to ask whether the law is falsified, so it is the column that had to move.",
    rustWitness := [
      "rholang/tests/execution.rs:peek_and_persistent_work",
      "rholang/tests/execution.rs:list_channel_matches"],
    statement := "Reduction (COMM): a send and a matching receive on one channel reduce to the \
      receive's body",
    status := .provedModel,
    declarations := [`Rchain.Reduce, `Rchain.reduce_closed, `Rchain.reduce_not_deterministic],
    rust := ["rholang/src/reduce.rs"],
    coq := ["spec/coq/Laws.v:reduce"],
    witness := [`Rchain.reduce_not_deterministic],
    falsifiable := some "`reduce_not_deterministic` proves confluence is **false** on the flat `Par`, \
      so the law's statement is bounded by a published disproof rather than an assertion"
    },
  { number := 4, clause := "b", layer := "Rholang",
    rustWitness := ["rholang/tests/execution.rs:law4_new_allocates_fresh_names_and_the_reduct_is_closed"],
    statement := "`new` yields fresh unforgeable names: reduction introduces no free variables it did \
      not already have",
    status := .provedModel,
    declarations := [`Rchain.reduce_freeVars_subset, `Rchain.freeVarOf_receivePar,
      `Rchain.freeVarOf_parMerge],
    rust := ["rholang/src/reduce.rs"],
    witness := [`Rchain.reduce_freeVars_subset],
    falsifiable := some "a `comm` whose receive body mentions a level free in neither the send nor the \
      receive would refute it — the shape a body that is not a function of the datum it consumed \
      produces, and the one the port's capture-avoiding `substitute_par` (`rholang/src/substitute.rs`) \
      exists to prevent",
    note := "proved in `Rchain/Reduce.lean` by induction on the derivation, against `freeVarOf`, which \
      is a definition (law 6) — so this is a statement about a defined predicate rather than about an \
      `axiom`, which is what the row used to be. `comm` is the arm with content (the reduct is the \
      receive's body, a component of the redex); the two congruence arms are `freeVarOf_parMerge` in \
      both directions. **The `new` half is stated in `Closed`'s terms, not here**: the base-sort \
      `Reduce` has no `new` rule — freshness of the bound name is a property of the model's \
      `GUnforgeable`, not of the relation — and what carries it is `Ty.lean`'s `closedNew` (a binder's \
      body is checked and nothing else) with `reduce_closed` composed in. So the row proves the \
      free-variable half in as many words, and names the half it does not." },
  { number := 5, layer := "Rholang",
    rustWitness := [
      "rholang/src/property_tests.rs:law5_a_pattern_that_binds_a_variable_twice_never_matches",
      "rholang/src/property_tests.rs:law5_a_pattern_that_binds_distinct_variables_matches"],
    statement := "Spatial matching; a free variable is bound at most once — enforced by the port's \
      **normalizer** before any matcher runs, and, inside the matcher, at the **entry**: \
      `spatial_match` refuses a pattern `linear` rejects before any clause runs, so neither its \
      element-pair nor its conjunction path is reached with a level bound twice (`aggregate_updates` \
      stays, as the collection path's check on the *bindings* rather than on the pattern)",
    status := .provedTied,
    declarations := [`Rchain.linear, `Rchain.spatialMatch, `Rchain.spatialMatches, `Rchain.spatialMatchCore,
      `Rchain.aggregateUpdates, `Rchain.aggregateUpdates_rejects_double_bind,
      `Rchain.freeMapMerge_overwrites, `Rchain.fuel_saturation,
      `Rchain.a_nested_tuple_is_paid_for, `Rchain.a_tuple_pays_for_its_own_contents,
      `Rchain.a_two_expression_pattern_refutes_the_modelled_tie,
      `Rchain.a_shorter_set_pattern_is_refused, `Rchain.a_shorter_map_pattern_is_refused,
      `Rchain.a_set_pattern_of_the_same_length_still_matches,
      `Rchain.a_permuted_pattern_is_refused, `Rchain.an_unaligned_variable_pattern_is_refused],
    corpus := some "match",
    rust := ["rholang/src/matcher/spatial_matcher.rs", "models/src/types.rs"],
    coq := ["spec/coq/Laws.v:spatial_matches", "spec/coq/Laws.v:linear"],
    witness := [`Rchain.aggregateUpdates_rejects_double_bind, `Rchain.freeMapMerge_overwrites,
      `Rchain.a_shorter_set_pattern_is_refused, `Rchain.a_shorter_map_pattern_is_refused,
      `Rchain.a_set_pattern_of_the_same_length_still_matches,
      `Rchain.a_permuted_pattern_is_refused],
    falsifiable := some "the corpus's three-valued verdicts (`true`/`false`/`rejected`) include the \
      rejected case a twice-bound pattern produces — the shape the previous law-5 axiom *denied* and \
      which `spec/conformance/match.tsv` now pins (AUDIT C26) — and `freeMapMerge_overwrites` is the \
      counterexample inside the model: the same repeated level the aggregation path refuses is silently \
      overwritten on the fold path. This row's fuel axiom was falsified **in fact**, not only in \
      principle (AUDIT C50): with the old measure a tuple nested three deep is `modelledPar`, \
      connective-free and equal to itself while the matcher answered `false`, so \
      `concrete_matches_iff_eq` evaluated to `false = true` — two lines of `decide`, and the fix is \
      what makes them fail",
    note := "`spatialMatch_implies_linear` is `h.2` of a conjunct inside `spatialMatch`'s own \
      definition, so it holds by construction — and it is **not the port's predicate**. The port's \
      *outermost* enforcing check is the **normalizer's**: a pattern that binds a name twice is \
      refused before any matcher runs, in both contexts (`normalizer.rs:111,289,590,1325`, \
      `UnexpectedReuseOfNameContextFree`/`…ProcContextFree`). **Probed on a devnet** (AUDIT C42): \
      `for (@[v, v] <- x)`, `for (v <- x & v <- y)` and `for (@{\"k\": v, ...v} <- x)` each return 400 \
      with `Free variable v is used twice as a binder …`, while a duplicated *datum* \
      (`x!([*a, *a])` against `for (@[p, q] <- x)`) returns 200 with the same unforgeable hash twice. \
      **The matcher now enforces it itself, and this is what C42 changed** (2026-09-24): `spatial_match` \
      (`spatial_matcher.rs:227`) is `if !linear(pattern) { no match }` followed by the clauses, which \
      moved to `spatial_match_core` — the same split, with the same names, that the model makes \
      (`spatialMatch … && linear pattern`) — and the store's own entry reaches the clauses the same \
      way, `RhoMatch::get` handing `&spatial_match` to `fold_match` (`rholang/src/storage.rs:75`). `linear` \
      (`models/src/types.rs:93`) is the walk `freeLevelsOfPar` mirrors, levels and all. **One guard is \
      enough, and that is an argument rather than a hope**: the clauses descend only into sub-terms \
      `linear`'s own walk reaches — a send's channel and data, a receive's body, a `new`'s body, a \
      `match`'s target and cases, a bundle's, a connective's members, a collection's elements — so a \
      sub-pattern's free levels are a sub-multiset of the whole pattern's, and a linear pattern has \
      only linear sub-patterns. Guarding every recursive call would re-walk the pattern at each depth \
      and buy nothing. The clauses' plain `insert` is still what they do — `freeMapMerge_overwrites` \
      is still the model's statement of that — and what the guard changes is that no pattern reaching \
      them carries a repeated level. **`aggregate_updates` stays as the inner check** \
      (`spatial_matcher.rs:686-696`): it reads the free maps the collection path's bipartite search \
      produced, each element having been matched from a fresh map (`:812`), and refuses a level that \
      two of them introduced — a clash *between bindings* rather than a property of the pattern, which \
      is the sense in which the entry guard does not subsume it as a check. From a term it has no \
      reachable producer any more (the normalizer refuses those shapes first), and the model keeps \
      both halves: `aggregateUpdates_rejects_double_bind` for the check and `freeMapMerge_overwrites` \
      for the clauses — and **both are theorems**, which is what this row's debt was \
      (`aggregateUpdates_rejects_double_bind` at `Match.lean:477`, `freeMapMerge_overwrites` at \
      `:489`; the file declares no axiom, so neither rests on one). The **tie** this row once shared is \
      law 37's and is **proved** there (`spatialMatches_iff_eq`) — the row is `proved-tied` on the \
      `match` corpus, whose three-valued verdicts are the node's half. \
      **A third thing this row's corpus found** (AUDIT C44): the clauses had no arm for a \
      **tuple**, which the port matches (`spatial_matcher.rs:538-544`) — so a tuple pattern the node \
      matches read as silence here, and the law's own statement was false of the model until \
      `modelledPar` was added to it. Cases 15/16 of `spec/conformance/match.tsv` are the pair that \
      caught it; the arm is not an axiom, it is a clause. **A fourth thing, and it is the one that \
      matters most** (AUDIT C50, 2026-09-24, found while *attempting* `fuel_saturation`): the fuel did \
      not pay for a **tuple's** walk. `parNodesExpr` had no `etuple` arm, so a tuple's contents were \
      charged to no node while the tuple clause walks them exactly as the list arm does — the budget \
      stayed constant as the nesting deepened. Two levels need 13 and were given 12, so the model \
      answered `false` where the node answers `true`, on a shape needing **no** padding (`@((1, 2), \
      (3, 4))` against itself; a search over 300 generated shapes rejected 40, and three-deep tuples \
      were rejected at every depth the measure could reach). Case 20 of `spec/conformance/match.tsv` is \
      the ratchet that would have caught it. The reason this belongs in the row rather than only in \
      AUDIT: **`concrete_matches_iff_eq` was *false* while the defect stood** — a tuple three deep is \
      `modelledPar`, connective-free and equal to itself, so its conclusion evaluated to `false = \
      true`. The refutation is two lines of `decide` and is *not* kept, because with the measure fixed \
      the same `decide` fails; what the episode says is that this row's axiom is only as true as the \
      fuel beneath it, which is exactly what `fuel_saturation` is for. **And the tie's own axiom was \
      false, so it is gone** (AUDIT C51, same day, found by asking what the statement says of a value \
      the model *admits*): `modelledPar` accepts a `Par` with two expressions, and the clauses have no \
      arm that accepts a multi-expression pattern, so the pattern is modelled, connective-free, equal \
      to itself and rejected — the tie's conclusion is `false = true` on it \
      (`a_two_expression_pattern_refutes_the_modelled_tie`). Same class as C44 one level up: not a \
      missing *clause* but a missing *hypothesis*. What this row owes is the tie for a pattern whose \
      expression list is a **singleton** — the shapes the clauses have an arm for — and it is owed \
      rather than asserted, because a statement about the model has to say what the model does on every \
      value it admits, not only the ones a stored datum can be. **A third hypothesis, found by probing \
      the domain rather than by reading it** (2026-09-24, AUDIT C54, and it cost one reverted change): \
      the collections' **contents must be canonical** beside the singleton list. A target with a \
      duplicated set element is matched by a shorter pattern in the model (`spatialMatch @{1, 1} @{1}` \
      answers `true` while `@{1, 1} ≠ @{1}`), so the tie is false on it — and the shape is \
      **unreachable**, because the node evaluates a set through `par_set`, which deduplicates and sorts \
      (`models/src/sorter.rs:834`, reached from `reduce.rs:783`), so no stored datum carries a duplicate. \
      Narrowing the statement to the invariant the code maintains is Law 10's `WellFormed` move again — \
      the model has no deduplicating set constructor to widen it with. The first attempt at this row \
      instead changed the *clauses* to enforce the port's no-remainder length check, which the corpus \
      consumer rejected on its first run — it reports the node answering `true`; C54 carries that record, and the \
      lesson is that a model change here is not believed until `lean_match_corpus` has run. \
      **The Coq half, split by kind** (2026-09-23): \
      `spec/coq/Laws.v`'s `linear` is a **definition** now (with `linear_decidable` and the witness \
      `a_double_binding_is_not_linear`), mirroring Lean's own predicate; `spatial_matches` stays a \
      **signature**, because mirroring the matcher in Coq is the analogue of this file's owed proofs \
      and not part of the honest-and-gated tier. \
      **And the member was over-claiming, which the guard fixes** (2026-09-24). Measured on the node \
      through the receive path and *then* `decide`d: `@Set(2)` against `Set(1, 2)` and `@{\"b\": 2}` \
      against `{\"a\": 1, \"b\": 2}` answer **false** there while the model answered `true` — the walk \
      drops leading targets, and with no remainder and no wildcard the port refuses an unequal length \
      before it searches at all (`exact_match = !wildcard && remainder.is_none()`, then \
      `if exact_match && plen != tlen`, `spatial_matcher.rs:684-693`). The clause now carries that \
      guard, and corpus rows 21/22 are the node's half of it. **This is AUDIT C54's reverted change \
      re-landed**, and why it was withdrawn is the part worth keeping: the row written for it \
      (`@Set(1)` against `Set(1, 1)`) is answered `true` by the node, because `par_set` deduplicates \
      what `eval_expr` stores — so that row compared the model's *literal* against a different \
      *value*, and the conclusion drawn from it (\"there was no defect\") does not follow. The guard \
      was right; the row was not. **The opposite direction is a residue rather than a repair**: a \
      pattern *permuted* relative to the target (`@Set(2, 1)`, `@Set(x, 1)` against `Set(1, 2)`) is \
      matched by the node, whose assignment backtracks, and refused by the walk here, which cannot \
      reorder. The two agree on canonical inputs — the tie's domain — and widening the walk is a \
      *measure* change, not another clause (a backtracking search needs a fuel at least quadratic in \
      the nodes where `matchFuel` is linear), so it is pinned by `a_permuted_pattern_is_refused` and \
      said here rather than left implicit" },
  { number := 6, layer := "Rholang",
    rustWitness := ["models/src/property_tests.rs:law6_a_closed_term_is_accepted_and_the_predicate_agrees"],
    statement := "No globally free variables in a program",
    status := .provedTied,
    corpus := some "closed",
    declarations := [`Rchain.Closed, `Rchain.closed, `Rchain.closed_eq_Closed, `Rchain.freeVarOf,
      `Rchain.freeVarOf_iff_closed, `Rchain.closed_iff_no_freeVars, `Rchain.Closed_parMerge_iff,
      `Rchain.Closed_receivePar_iff, `Rchain.closed_anyPat],
    rust := ["models/src/types.rs"],
    coq := ["spec/coq/Laws.v:closed", "spec/coq/Laws.v:closed_decidable"],
    witness := [`Rchain.free_var_is_not_closed, `Rchain.bound_var_is_closed, `Rchain.free_var_is_free_under_par],
    falsifiable := some "both sides are `decide`d on concrete terms: `bound_var_is_closed` (a `.bound` \
      occurrence is closed — the model reads it as a back-reference the normalizer resolves), \
      `free_var_is_not_closed` (a `.free 3` occurrence is not, and the **level** is what the predicate \
      is about: `freeVarOf p 4` is false of it), and `free_var_is_free_under_par` (a level free \
      inside a `|` is free in the whole). Two mutations were checked while writing it: making \
      `freeVarAt` accept `.bound k` as free breaks the tie's `Expr.evar` arm, and dropping one \
      disjunct from `freeVarOf`'s `Par` arm breaks its `Par` arm",
    note := "**the tie is a proof now.** `freeVarOf` is a `mutual` block mirroring `Ty.lean`'s \
      `closed*` block type for type (`∨` where the checker has `&&`), and `freeVarOf_iff_closed` is \
      the corresponding induction. It discharges two axioms — the opaque predicate and the tie — which \
      the row used to record as 'a defined-but-undefined-elsewhere predicate'. What makes the \
      mirroring exact is the model's de Bruijn *levels*: `closedVar` reads `.bound` as bound and \
      `.free` as open, so the checker refuses exactly the occurrences the predicate accepts. **The Coq \
      half is a definition too now** (2026-09-23): `spec/coq/Laws.v` defines `closed` over the flat \
      family — the recursive function passed as a value to `forallb`, the idiom the guard checker \
      accepts — keeps `closed_decidable`'s statement unchanged (a changed statement is a changed law), \
      and proves a free level is not closed" },

  -- ── RSpace: the tuple space (Laws 7–11) ─────────────────────────────────────────────────────────
  { number := 7, layer := "RSpace",
    rustWitness := ["rspace/src/property_tests.rs:law7_join_hash_commutes"],
    statement := "Join commutativity: channel keys are hashed in sorted order, so the join key is \
      invariant under permutation",
    status := .provedModel,
    declarations := [`Rchain.joinKey, `Rchain.joinKey_perm],
    axioms := [`Rchain.hashHashes],
    rust := ["rspace/src/hashing/stable_hash_provider.rs"],
    witness := [`Rchain.joinKey_perm, `Rchain.Comparator.sortList_perm],
    falsifiable := some "`joinKey_perm` follows from `Cmp.sortList_perm` — the join key is *defined* as \
      hash-of-sorted-hashes, mirroring `hash_seq` + `hash_hashes` — so removing the sort from the \
      definition would falsify it: two permutations of one channel list would then hash differently",
    note := "**this row's two axioms are gone.** `joinKey : List Channel → Nat` was opaque and \
      `joinKey_perm` a claim about it; the Rust's join key is *defined* (`hash_seq` sorts the channel \
      hashes, `hash_hashes` sorts again and hashes — `stable_hash_provider.rs:22-46`), so the invariance \
      is Law 1's canonicalization applied to a join key rather than an independent postulate. \
      `hashHashes` is the one primitive that stays axiomatized, in Law 19's class" },
  { number := 8, layer := "RSpace",
    rustWitness := [
      "rspace/src/property_tests.rs:law8_comm_sorts_produces",
      "rspace/src/space_matcher.rs:find_matching_data_candidate_finds_first_match"],
    statement := "Deterministic COMM: candidate selection is sorted-first by content hash and produce \
      refs are sorted, so the event trace is content-addressed",
    status := .provedModel,
    declarations := [`Rchain.Produce, `Rchain.Consume, `Rchain.Comm, `Rchain.produceRefs,
      `Rchain.commId, `Rchain.comm_content_addressed],
    rust := ["rspace/src/trace/event.rs", "rspace/src/space_matcher.rs", "rspace/src/rspace.rs"],
    axioms := [],
    witness := [`Rchain.comm_content_addressed, `Rchain.Comparator.sortList_perm],
    falsifiable := some "`comm_content_addressed` is proved from `sortList_perm`: two comms whose \
      produces are permutations of one another have the same identity. Dropping the sort from \
      `produceRefs` — which is what `Comm::apply` would be without `produce_refs.sort_by_key` — would \
      falsify it, and the Rust pins the ordering directly (`event.rs:165`'s `produce_refs.sort_by_key`, \
      and the `sort_by` on the candidate source at `space_matcher.rs:101` and `rspace.rs:169`)",
    note := "**both axioms are gone.** `produceRefs : Comm → List Nat` was opaque and \
      `comm_content_addressed` a claim about it; the refs are a *definition* now, mirroring \
      `Comm::apply`'s sort, and the content-addressing is Law 1's canonicalization applied to the event \
      log. The model keeps the arrival order in `Comm.produces` precisely so the sort has something to \
      remove" },
  { number := 9, layer := "RSpace",
    rustWitness := [
      "rspace/src/property_tests.rs:law9_channel_change_is_monoid",
      "rspace/src/property_tests.rs:law9_disjoint_state_changes_commute",
      "rspace/src/property_tests.rs:law9_state_change_combine_is_associative"],
    statement := "Merge is a monoid and non-conflicting logs commute — strengthened for effect \
      scheduling: disjoint **closure** (not footprint) implies commutation",
    status := .provedModel,
    declarations := [`Rchain.mergeChanges, `Rchain.mergeChanges_assoc, `Rchain.NonConflicting,
      `Rchain.mergeChanges_comm, `Rchain.nonConflicting_not_necessary, `Rchain.join_last_wins,
      `Rchain.effect_commute_of_disjoint_closure, `Rchain.effect_reorder_diverges],
    rust := ["rspace/src/merger/state_change.rs", "rspace/src/merger/event_log_merging_logic.rs"],
    axioms := [],
    witness := [`Rchain.mergeChanges_assoc, `Rchain.mergeChanges_comm, `Rchain.join_last_wins, `Rchain.nonConflicting_not_necessary, `Rchain.effect_reorder_diverges],
    falsifiable := some "`mergeChanges_assoc` is structural (concatenation of the added/removed lists, \
      right-biased overwrite of the join map); `mergeChanges_comm` *needs* the disjointness hypothesis — \
      without it the theorem is false twice over: the added/removed lists concatenate in operand order \
      (which is why the Rust's own test compares sorted multisets rather than lists, \
      `state_change.rs:224-238`), and a contested **join** is won by whichever side the fold reaches last \
      (`state_change.rs:186-189`, pinned by `:502-544` and stated as `join_last_wins`; the fold is \
      `casper/src/merging.rs:832-836`). `nonConflicting_not_necessary` proves the relation is sufficient \
      and not necessary, and `effect_reorder_diverges` remains the disproof of the weaker footprint \
      reading",
    note := "**all four axioms are gone.** `mergeChanges` and `NonConflicting` were axioms over \
      `StateChange = { id : Nat }` — and a claim about an undefined relation is a claim about nothing, \
      so the two laws could have been true of any relation one cared to imagine. The Rust gives the \
      definition: the added/removed lists concatenate (`ChannelChange::combine`, \
      `channel_change.rs:22-27`, reached from `state_change.rs:30`) and the join map is a \
      **right-biased overwrite** (`joins.insert(k, v)` in a loop over the right operand, \
      `state_change.rs:186-189` — the Scala's `x.map ++ y.map`, right-biased too, \
      `StateChange.scala:152`), so the model \
      overwrites and `join_last_wins` states which side wins — which the code's own \
      `combine_has_an_identity_and_a_right_biased_join_map` (`state_change.rs:508-546`, \"the later change's join body \
      wins\") pins. **Two findings are recorded here, one now fixed.** \
      (1) `NonConflicting` is *not* `are_conflicting` read negatively, and saying so was wrong: \
      `are_conflicting` is over two `EventLogIndex`es with three checks, one of which (a potential COMM) \
      is a shared-channel interaction and one of which (produces touching base joins) no state diff can \
      see (`event_log_merging_logic.rs:105-158`), and the predicate the merge branches on is broader \
      again (`casper/src/merging.rs:177-181`). What the model needs is the sufficient condition for \
      commutation, and it is named for that. (2) The Rust test named `combine_is_associative` \
      (`state_change.rs:203-238`) did **not** test associativity — its own comment said the law it pinned \
      was empty-is-identity — so the associativity the merge fold relies on \
      (`casper/src/merging.rs:832-836`) was **untested on the Rust side** (AUDIT C43). It is fixed: the \
      misnamed test is renamed to what it asserts, and \
      `property_tests.rs`'s `law9_state_change_combine_is_associative` is the test — over arbitrary state \
      changes including the join map, and falsified before it was believed (a left-side-dropping \
      `combine` makes it fail in 0.01s)" },
  { number := 10, layer := "RSpace",
    rustWitness := ["rspace/src/property_tests.rs:law10_merkle_root_is_insertion_order_independent"],
    statement := "Merkle determinism: the radix trie is content-addressed and collision-free **on the \
      nodes the trie can build** (`WellFormed`: 256 slots, 32-byte values, prefixes under 128 bytes), \
      with a defined empty root",
    status := .provedModel,
    declarations := [`Rchain.Item, `Rchain.Node, `Rchain.ItemWF, `Rchain.ItemsWF, `Rchain.WellFormed,
      `Rchain.byteOf, `Rchain.byteOf_injective, `Rchain.encodeItem, `Rchain.encodeItem_eq_nil_iff,
      `Rchain.encodeItem_head, `Rchain.encodeItem_injective_at, `Rchain.encodeNodeAux,
      `Rchain.encodeNodeAux_head, `Rchain.encodeNodeAux_head_ne_byteOf,
      `Rchain.encodeNodeAux_injective, `Rchain.encodeNode, `Rchain.encodeNode_injective,
      `Rchain.nodeHash, `Rchain.emptyNode, `Rchain.emptyNode_wellFormed, `Rchain.emptyRoot,
      `Rchain.root_collision_free, `Rchain.nodeHash_eq_emptyRoot,
      `Rchain.the_encoder_is_not_canonical_over_the_models_types],
    axioms := [],
    rust := ["rspace/src/history/radix_tree.rs"],
    witness := [`Rchain.the_encoder_is_not_canonical_over_the_models_types, `Rchain.encodeNode_injective, `Rchain.nodeHash_eq_emptyRoot, `Rchain.root_collision_free],
    falsifiable := some "`root_collision_free` composes Law 19's `blake2b256_collision_free` with \
      `encodeNode_injective`; `nodeHash_eq_emptyRoot` pins the empty root as a fixed point with nothing \
      else hashing to it. A serializer that dropped a field would falsify the first, and a second node \
      hashing to the empty root the second — which is why the store *refuses* a colliding write \
      (`save_node`, `radix_tree.rs:240-256` — its collision assert is the refusal) rather than tolerating one. **And the canonicity axiom was \
      itself falsified**: `the_encoder_is_not_canonical_over_the_models_types` exhibits two distinct \
      nodes with one encoding (a 35-byte value re-reads as a second item), which is why the axiom and \
      both theorems now carry `WellFormed` — the invariant the code carries in its types and the model \
      did not",
    note := "**the ghost constant is gone, and then the axiom it left behind was falsified.** \
      `trieRoot : NodeHash` had no arguments — a constant — which is why nothing could be proved about \
      it; the root became `nodeHash ∘ encodeNode`, with the collision statements theorems composing Law \
      19's axiom with `encodeNode_injective`. That axiom was then **false as stated** (2026-09-23): it \
      claimed canonicity over `Node := List Item` and `Hash := List Byte`, both wider than the code's \
      `[Item; 256]` and `Hash32([u8; 32])`, and the encoder writes neither width — so a value whose \
      length is not 32 has more than one reading, and a witness exists \
      (`the_encoder_is_not_canonical_over_the_models_types`; the two nodes are a one-item node with a \
      35-byte value and a two-item node, and their encodings are the same 37 bytes). The code is not \
      vulnerable: the type `[Item; 256]`, the 32-byte `Hash32`, and prefixes that are suffixes of a \
      32-byte key (so under 128, inside the 7-bit length field) make the second reading \
      unrepresentable. So the statement was narrowed to the invariant rather than the model widened: \
      `encodeNode` is now a **definition** mirroring `radix_tree.rs:48-77` (the port's `encode`; index truncation and all), \
      `WellFormed` names the invariant, `encodeNode_injective` carries it, and \
      `root_collision_free`/`nodeHash_eq_emptyRoot` inherit it — stronger where it matters, because the \
      hypothesis is exactly what the trie's operations maintain. **And the narrowed statement is now \
      proved, not owed** (2026-09-23, Programme D unit 11): `encodeNode_injective` is a theorem, by the \
      argument a decoder would make — each record carries its slot index and a `length | kind` byte, so \
      it determines its own extent, and `WellFormed` supplies the two facts that makes true (32-byte \
      payloads, prefixes under 128 so the 7-bit field is not truncated) plus the width that rules out a \
      stream ending in empty slots. So the hash path's canonicity no longer rests on an assumption: the \
      only axioms left under `root_collision_free` are Law 19's hash idealization" },
  { number := 11, layer := "RSpace",
    rustWitness := ["rspace/src/property_tests.rs:law11_a_replayed_script_matches_its_recording",
      "rspace/src/replay_rspace.rs:a_rigged_replay_matches_its_recorded_trace",
      "rspace/src/replay_rspace.rs:a_rig_whose_comm_never_happens_is_reported"],
    statement := "Replay determinism: the port's replay check — every recomputed COMM has a recorded \
      occurrence **and** no recorded COMM is left unconsumed — holds exactly when the recomputation and \
      the recorded trace have the same COMM occurrences",
    status := .provedModel,
    declarations := [`Rchain.Comm, `Rchain.produceRefs, `Rchain.commId, `Rchain.Trace, `Rchain.CommRef,
      `Rchain.refOf, `Rchain.refsOf, `Rchain.occurrences, `Rchain.removeOne, `Rchain.Replays,
      `Rchain.forwardHolds, `Rchain.reverseHolds, `Rchain.removeOne_eq_some_iff,
      `Rchain.replays_iff_same_occurrences, `Rchain.a_replay_agrees_with_its_record,
      `Rchain.the_forward_half_alone_admits_a_diverging_trace,
      `Rchain.the_reverse_half_alone_admits_a_phantom_recomputation],
    axioms := [],
    rust := ["rspace/src/replay_rspace.rs", "rspace/src/space_matcher.rs"],
    witness := [`Rchain.the_forward_half_alone_admits_a_diverging_trace, `Rchain.the_reverse_half_alone_admits_a_phantom_recomputation, `Rchain.a_replay_agrees_with_its_record],
    falsifiable := some "the equivalence is refutable from both sides, and each side has a witness in \
      the tree: `the_forward_half_alone_admits_a_diverging_trace` (one recomputed COMM, two recorded — \
      the forward half passes, the check fails, which is what the RCHAIN-3505 guard lets through on a \
      failed deploy) and `the_reverse_half_alone_admits_a_phantom_recomputation` (one recorded, two \
      recomputed — the reverse half passes, the check fails). In the port, each half has its own error \
      that the Rust's tests assert fires: `ReplayCommNotInTrace` (`replay_rspace.rs:341-345`, \
      `a_rigged_replay_matches_its_recorded_trace` `:683-699`) and `Unused COMM event` \
      (`check_replay_data`, `replay_rspace.rs:616-626`, `a_rig_whose_comm_never_happens_is_reported` `:711-744`)",
    note := "**The modelling step the row previously owed, landed** (2026-09-23, Programme D unit 9). \
      The row was `vacuous` because \"recompute\" and \"record\" were the *same function* in the model, \
      so the claim was `rfl`. The record is now an **input** — `Replays recomputed recorded` — and the \
      equivalence with occurrence equality is `replays_iff_same_occurrences`; the two sides are \
      independent values, so the claim can fail, and both halves are proved necessary. Two deliberate \
      scoping notes: what this does **not** model is the recomputation itself (`recomputed` is an input; \
      that the node recomputes the same log is law 25's business), and the record is a flat list of \
      refs, whereas the port's multimap is keyed per consume — a faithful-enough flattening because \
      `commId` already carries the consume, which is why the key here is `refOf` (`commId` flattened). \
      **The live hazard is stated as a theorem**: `check_replay_data_with_fix` \
      (`casper/src/runtime_replay.rs:587-599`) drops the reverse half when `eval_successful` is false, \
      so on that path the check *is* the forward half and a diverging trace passes — the \
      divergence-masking TODO the plan flagged, now a change the register can see" },

  -- ── Rosette: the actor VM (Laws 12–13) ──────────────────────────────────────────────────────────
  { number := 12, layer := "Rosette",
    statement := "Actor atomicity (single-threaded `mbox.nextMsg`)",
    status := .orphaned,
    falsifiable := none,
    note := "`rosette`/`roscala` are not wired into the build and the Rust reducer replaces the VM" },
  { number := 13, layer := "Rosette",
    statement := "Reflection: everything is an `Ob`; meta/parent chain; fork-join barrier",
    status := .orphaned,
    falsifiable := none,
    note := "**the same decision as row 12, and it needed saying here too** (2026-09-25): this is the \
      Rosette actor VM's reflection layer — `Ob` as the universal object, the meta/parent chain, and \
      the fork-join barrier whose fork is `OpFork` \
      (`legacy/roscala/src/main/scala/coop/rchain/roscala/Vm.scala:195`), with the `Ob`/`Meta`/`Ctxt` \
      triple under `legacy/roscala/src/main/scala/coop/rchain/roscala/ob/` — and \
      `legacy/rosette`/`legacy/roscala` are not wired into the build: the Rust reducer replaces the VM. \
      So there is nothing in this tree for the law to be a claim about, which is what `orphaned` says. \
      Stated rather than left to the status word because the register's own rule for `vacuous` applies \
      here too — a status with no note is an admission with no plan, and row 12's note is the same \
      sentence about a different part of the same VM" },

  -- ── Casper / Storage / Crypto (Laws 14–19) ──────────────────────────────────────────────────────
  { number := 14, clause := "a", layer := "Casper",
    rustWitness := [
      "block-storage/src/dag/finalizer.rs:law14_fringe_requires_supermajority",
      "sdk/src/property_tests.rs:law14_super_majority_is_strictly_more_than_two_thirds",
      "sdk/src/property_tests.rs:law14_the_two_thirds_boundary_survives_past_the_f64_mantissa",
      "casper/tests/finalization.rs:a_silent_bonded_validator_does_not_cap_the_fringe",
      "casper/src/blocks/proposer/proposer.rs:the_quorum_is_measured_against_the_whole_bonded_map_not_the_live_one",
      "block-storage/src/dag/liveness.rs:the_window_is_heights_behind_the_tip"],
    statement := "Finality is the fringe's advance gate: the fringe advances iff the supporting stake — \
      the stake of the candidates **seen by every sender of the partition** — is a strict supermajority \
      of the **whole bonded** stake, as the exact integer comparison `3·stake > 2·total` (no float \
      rounding). The two sets are separate arguments because the gate asks two questions: what a \
      candidate must have been seen *by* (the partition) and what the quorum is measured *against* (the \
      bonded map). The node passes the **live weight set** as the partition — the bonded validators \
      whose latest message is within a window of the tip — so a validator that has stopped producing \
      messages stops blocking the partition while its stake still counts against the quorum (#70); that \
      policy is §6's, and the gate below is proved with the partition as an argument",
    status := .provedTied,
    corpus := some "stake",
    declarations := [`Rchain.isSuperMajority, `Rchain.bondedSenders, `Rchain.stakeOf,
      `Rchain.stakeOf_eq_none, `Rchain.allBonded, `Rchain.bondedSupport,
      `Rchain.fullPartitionStake, `Rchain.totalStake, `Rchain.calculateFringe,
      `Rchain.calculateFringeOneMap, `Rchain.calculateFringeOneMap_eq_calculateFringe_self,
      `Rchain.finality_iff_supermajority, `Rchain.two_thirds_is_not_supermajority,
      `Rchain.above_two_thirds_is_supermajority, `Rchain.below_two_thirds_is_not_supermajority,
      `Rchain.large_stake_just_above_two_thirds_is_exact,
      `Rchain.i64_overflowing_stakes_do_not_wrap],
    axioms := [],
    rust := ["block-storage/src/dag/finalizer.rs", "sdk/src/consensus.rs"],
    witness := [`Rchain.two_thirds_is_not_supermajority, `Rchain.below_two_thirds_is_not_supermajority, `Rchain.large_stake_just_above_two_thirds_is_exact, `Rchain.i64_overflowing_stakes_do_not_wrap, `Rchain.stakeOf_eq_none, `Rchain.calculateFringeOneMap_eq_calculateFringe_self],
    falsifiable := some "each boundary is an independent witness, and each names the port's own test: \
      `two_thirds_is_not_supermajority` fails the moment the comparison is `≥` (`consensus.rs:24`); \
      `large_stake_just_above_two_thirds_is_exact` is false for the `f64` form the Scala oracle uses \
      (`stake.toDouble / totalStake > 2d / 3`, `legacy/sdk/.../consensus/Stake.scala:8`), which cannot \
      represent `2·2⁵³+1` (`sdk/src/consensus.rs:40`); `i64_overflowing_stakes_do_not_wrap` is the \
      case the port's `i128` exists for (`:50`); and `stakeOf_eq_none` is false for a gate that \
      indexed the bonds map by every support sender — the panic the port's `calculate_fringe` skips \
      instead, pinned by `calculate_fringe_ignores_non_bonded_sender` \
      (`block-storage/src/dag/finalizer.rs:447`, and `law14_fringe_requires_supermajority` at `:425`); \
      and the split itself is falsified by the *other* direction — reverting the node to one map for \
      both makes `a_silent_bonded_validator_does_not_cap_the_fringe`'s control arm the only arm, which \
      is the pre-2026-09-29 behaviour where a silent bonded validator capped finality at any stake share",
    note := "**the axiom that stood here was `Nat.mul_comm` twice** — `isSuperMajority s t ↔ s * 3 > \
      t * 2` restated the definition's own body, which is why the row was `vacuous` and why it tied \
      finality to nothing. It is a **theorem** now, and the law is the **gate**: `calculateFringe` is \
      the port's `calculate_fringe` (the full-partition filter, the skip for a non-bonded sender, the \
      exact integer comparison — `finalizer.rs:219-235`) and `nextFringe` is `next_fringe_detailed`'s \
      decision with `calculate_finalization`'s progress guard (`finalizer.rs:275-317`, `:361-363`). **Where the content is, stated \
      plainly**: the `↔`'s shape is the gate's own `if`, so the weight sits in *what the gate computes*, \
      and that is what the boundary theorems falsify — the strict `>`, the exact `3·stake > 2·total` at \
      the 2⁵³ boundary, and the non-bonded skip. Each is pinned by a named Rust test, which is what \
      makes the row a claim about code rather than arithmetic. **Two maps since 2026-09-29**, because \
      one map made the gate ask both questions of the same set: `calculateFringe`'s partition now ranges \
      over `partition`'s senders while `totalStake` sums `quorum`'s values, and \
      `calculateFringeOneMap_eq_calculateFringe_self` records that the one-map call is their identity — \
      so the boundary theorems above are unchanged rather than re-earned. The **policy** that chooses the \
      partition is the node's live weight set (`block-storage/src/dag/liveness.rs`, §6, AUDIT C174), and \
      it enters here as an argument, exactly as the support map does. Modelled, and **as a deliberate \
      deviation from the oracle**: `check_min_messages` ahead of the stake gate (`finalizer.rs:127`, \
      called at `:290`) demands the minimum-message **sender set** equal the bonded set, where the \
      Scala compares counts only and carries the epoch TODO saying so \
      (`legacy/block-storage/src/main/scala/coop/rchain/blockstorage/dag/Finalizer.scala:64-66`, \
      \"add support for epoch changes, simple comparison for senders count is not enough\"). Count-only \
      admitted `[A, A, B]` for bonds `{A, B, C}`: the layer fold collapsed the duplicate sender, and \
      the published fringe **omitted bonded validator `C`** while presenting A's stake twice — 90 of \
      100 on the merged change's own fixture. The model follows the port (`Rchain.checkMinMessages`), \
      the divergence is §6's, and the gate is pinned both ways by `the_gate_demands_the_bonded_senders` \
      here and by `check_min_messages_needs_all_bonded_senders` on the Rust side" },
  { number := 14, clause := "b", layer := "Casper",
    rustWitness := [
      "block-storage/src/dag/finalizer.rs:calculate_next_layer_picks_max_sender_seq",
      "block-storage/src/dag/finalizer.rs:check_min_messages_needs_all_bonded_senders"],
    statement := "A fringe holds one message per bonded validator (an antichain) — **of the fringe the \
      derivation publishes**, and with the *partition* as the set: the walk and the layer give \
      pairwise-distinct **senders**, and the coverage gate gives that the layer's sender set **is** the \
      partition map's, so `derivedFringe_holds_one_per_bonded` states one per validator directly — no \
      longer delegated to the upstream epoch TODO, which the port now departs from (law 14a's row, §6). \
      The partition is the node's live weight set (AUDIT C174), so on a chain where a bonded validator \
      has stopped, one per bonded validator is read against the ones still speaking; over a bare `Fringe` \
      the antichain claim is false and its refutation is proved",
    status := .provedModel,
    declarations := [`Rchain.derivedFringe_antichain, `Rchain.derivedFringe_holds_one_per_bonded,
      `Rchain.Fringe, `Rchain.fringe_antichain_is_false, `Rchain.Dag],
    axioms := [],
    rust := ["block-storage/src/dag/finalizer.rs"],
    witness := [`Rchain.derivedFringe_antichain, `Rchain.fringe_antichain_is_false,
      `Rchain.a_derivation_is_an_antichain, `Rchain.derivedFringe_holds_one_per_bonded,
      `Rchain.checkMinMessages_senders, `Rchain.the_gate_demands_the_bonded_senders],
    falsifiable := some "**both directions, and a mutation that makes the statement false rather than \
      merely unproved.** `fringe_antichain_is_false` refutes the unrestricted form on a bare `Fringe` — a \
      value the model can build and the derivation cannot publish — and `a_derivation_is_an_antichain` is \
      a `decide`d instance where the derivation *does* publish a two-sender layer. The mutation is one \
      word: `layerInsert`'s filter dropped (`m :: l.filter …` → `m :: l`), and then \
      `a_derivation_is_an_antichain` and `a_derivation_publishes_a_layer` are **false of the model** \
      (the layer's senders stop being `[0, 1]`) while `derivedFringe_antichain`'s proof breaks with them. \
      The coverage half has its own two-way witness: reverting `checkMinMessages` to the count \
      comparison makes `the_gate_demands_the_bonded_senders`' second conjunct **false of the model** — \
      it accepts `[0, 1]` against `{0, 1, 2}` — which is the case the merged Rust change refuses",
    note := "**the axiom that stood here was false**: it quantified over a *bare* `Fringe`, and a \
      `Fringe` is freely constructed, so the refutation is three lines. What it was missing was not a \
      hypothesis on the value but the **derivation**, and that is modelled now \
      (`Rchain/Casper/Dag.lean`, 2026-09-24): the walk (`self_parents`, `finalizer.rs:102-123`), the min \
      messages (`finalizer.rs:282-288`), the coverage gate (`finalizer.rs:127`, a sender-set comparison \
      since 2026-09-29 — the departure from the Scala law 14a's row and §6 record), the layer fold \
      (`calculate_next_layer`, `finalizer.rs:150-168`) and the stake gate (law 14a's \
      `calculate_fringe`, so its support map stays an argument as it is for `nextFringe`, and since \
      2026-09-29 so does the **partition**: `derivedFringe` takes the set whose senders must be covered \
      and the quorum's denominator separately, and the node passes the live weight set and the whole \
      bonded map, so a stopped validator stops blocking the coverage gate while its stake still counts \
      against the quorum (law 14a's row, AUDIT C174) — the same map twice being what the gate did \
      before). **And the \
      antichain is a property of the fold, not of the type it is stored in**: the port keeps the layer in \
      a `BTreeMap<sender, Message>`, which gives distinct senders for free — modelling *that* would make \
      this row true by construction, the shape G6 refused one unit earlier — so the model carries the \
      derivation's own data and states the antichain about the fold. **What the two theorems claim, and \
      where the second one's content is**: `derivedFringe_antichain` is the walk and the fold, \
      `derivedFringe_holds_one_per_bonded` is the gate — the fold preserves the sender *set* \
      (`mem_senders_nextLayer`), so the gate's equation between the min messages' senders and the bonded \
      set *is* one-per-bonded-validator. Neither statement needs the port's `BTreeMap`: what it needs is \
      the gate, which is why the row is named for the pair rather than for the map" },
  { number := 15, layer := "Casper",
    rustWitness := ["block-storage/src/property_tests.rs:law15_adding_blocks_only_grows_the_state"],
    statement := "The fringe is monotone by height **per sender** and the seen set is monotone (no \
      regression) — the **derived** fringe and the **constructed** seen set, the height claim **under the \
      ingress rules the port enforces at admission** (fork-freedom and the sequence rule, both refused \
      rather than observed: H-1's equivocation gate, `sequence_number`, \
      `check_justification_regression`); over bare values both claims are false and their refutations are \
      proved, and the **cross-sender** reading of the height claim is false too, of this tree *and* of the \
      oracle (`cross_sender_height_monotone_is_false`)",
    status := .provedModel,
    declarations := [`Rchain.Message, `Rchain.seenOf, `Rchain.Reaches,
      `Rchain.seen_monotone_of_reaches, `Rchain.seen_monotone_is_false,
      `Rchain.fringe_monotone_is_false, `Rchain.seenOf_contains_justifications, `Rchain.mem_seenOf_self,
      `Rchain.seen_subset_of_mem_seen, `Rchain.selfParents_skips_finalized,
      `Rchain.cross_sender_height_monotone_is_false, `Rchain.chain3,
      `Rchain.the_walk_is_oldest_first, `Rchain.a_chain_of_three_picks_the_oldest,
      `Rchain.ReachesF, `Rchain.NoFork, `Rchain.ReachesF.step, `Rchain.ReachesF.trans,
      `Rchain.selfParents_reaches, `Rchain.selfParents_height_lt, `Rchain.nofork_of_unfiltered,
      `Rchain.nofork_ancestors_go_through_the_parent, `Rchain.selfParents_above_a_finalized_ancestor,
      `Rchain.fork4, `Rchain.the_comparison_is_false_without_fork_freedom, `Rchain.fold4,
      `Rchain.minMsgs_fold4, `Rchain.fold4_is_fork_free,
      `Rchain.fold5, `Rchain.fold5_is_fork_free, `Rchain.fold5_satisfies_the_sequence_rule,
      `Rchain.the_fold_can_publish_below_the_previous_fringe, `Rchain.seedLayer,
      `Rchain.insertCandidate, `Rchain.seedLayer_nodup, `Rchain.insertCandidate_nodup,
      `Rchain.the_guard_keeps_the_newer_message, `Rchain.the_comparison_holds_on_fold5, `Rchain.SeqUnique,
      `Rchain.SeqStep, `Rchain.seqNum_lt_height_lt, `Rchain.entryFor, `Rchain.find?_sender_eq,
      `Rchain.insertCandidate_height_le, `Rchain.find?_filter_ne, `Rchain.entryFor_layerInsert_ne,
      `Rchain.entryFor_insertCandidate_ne, `Rchain.entryFor_insertCandidate_self,
      `Rchain.entryFor_insertCandidate_refused, `Rchain.entryFor_insertCandidate_some,
      `Rchain.entryFor_insertCandidate_none, `Rchain.entryFor_insertCandidate_le,
      `Rchain.mem_layerInsert, `Rchain.mem_insertCandidate, `Rchain.seedLayer_mem, `Rchain.parents_mem,
      `Rchain.foldl_insertCandidate_height_le, `Rchain.nextLayer_height_le_seed,
      `Rchain.mem_of_head?, `Rchain.reachesF_sender, `Rchain.selfParents_sender, `Rchain.minMsg_sender,
      `Rchain.minMsgs_mem_d, `Rchain.minMsgs_above, `Rchain.find?_sender_eq_nat,
      `Rchain.foldl_layerInsert_mem, `Rchain.seedLayer_entryFor_mem, `Rchain.seedLayer_entryFor_sender,
      `Rchain.foldl_insertCandidate_entryFor_none, `Rchain.nextLayer_above_the_previous_fringe,
      `Rchain.derivedFringe_above_the_previous_fringe, `Rchain.descendsB, `Rchain.descendsB_iff],
    axioms := [],
    rust := ["block-storage/src/dag/message_state.rs", "block-storage/src/dag/finalizer.rs"],
    witness := [`Rchain.fringe_monotone_is_false, `Rchain.seen_monotone_is_false,
      `Rchain.cross_sender_height_monotone_is_false, `Rchain.a_chain_of_three_picks_the_oldest,
      `Rchain.selfParents_above_a_finalized_ancestor,
      `Rchain.the_comparison_is_false_without_fork_freedom, `Rchain.fold4_is_fork_free,
      `Rchain.the_fold_can_publish_below_the_previous_fringe, `Rchain.fold5_is_fork_free,
      `Rchain.fold5_satisfies_the_sequence_rule, `Rchain.the_guard_keeps_the_newer_message,
      `Rchain.the_comparison_holds_on_fold5, `Rchain.seqNum_lt_height_lt,
      `Rchain.insertCandidate_height_le, `Rchain.minMsgs_above, `Rchain.nextLayer_height_le_seed,
      `Rchain.nextLayer_above_the_previous_fringe, `Rchain.derivedFringe_above_the_previous_fringe,
      `Rchain.d15_the_layer, `Rchain.d15_the_published_entry, `Rchain.d15_descends,
      `Rchain.d15_is_fork_free, `Rchain.d15_is_sequence_unique, `Rchain.d15_satisfies_the_sequence_rule,
      `Rchain.d15_the_comparison_holds],
    falsifiable := some "`fringe_monotone_is_false` exhibits two overlapping fringes (one at 5 and 1, one \
      at 3) where both arms of the disjunction fail — so the axiom was false as written; \
      `seen_monotone_is_false` exhibits two unrelated messages where `b` sees `a` and `a` sees `2` but \
      `b` does not. The constructive half is falsifiable too: `seenOf_contains_justifications` fails for \
      a `seenOf` that dropped the justifications' sets, which is the port's `new_seen` \
      (`message_state.rs:54-59`). **The comparison's own hypothesis is falsified rather than assumed**: \
      `the_comparison_is_false_without_fork_freedom` (`fork4`: `p` with a finalized parent at height 5 \
      and a second same-sender branch that reaches height 1 without passing through it) makes the \
      statement **false** of a DAG with a fork, so the theorem is about fork-free DAGs and `NoFork` is \
      what the ingress refusal (H-1, AUDIT C82/C84) supplies. **And the lift's own hypotheses are \
      falsified where they are dropped**: `the_fold_can_publish_below_the_previous_fringe` states the \
      conclusion's negation on `fold4`, a **fork-free** DAG whose `101` justifies the older `99` — so \
      fork-freedom alone does not carry the comparison, and the sequence rule is what refuses that shape; \
      and the fold's guard is load-bearing by a **one-word mutation** — `insertCandidate`'s \
      `if cur.seqNum < m.seqNum` replaced by `true` — under which `fold5` publishes the stale `c` where \
      the port publishes `z` (`the_guard_keeps_the_newer_message`)",
    note := "**two more false axioms, both refuted in the tree.** The content is the *derivation*: a \
      message's seen set is **constructed** as the union of its justifications' seen sets plus its own id \
      (`message_state.rs:54-59`), which the model now has (`seenOf`, with both halves proved: \
      `seenOf_contains_justifications` and `mem_seenOf_self`); and height monotonicity relates \
      *successive* fringes of one validator, which the finalizer's advance gate produces \
      (`finalizer.rs:275-317`, `:361-363`). **The transitive closure is proved, along the relation that does not need a lookup** (2026-09-24): \
      `Reaches a b` (a is a justification of b, or of a justification that reaches it) gives \
      `a.seen ⊆ b.seen` by induction (`seen_monotone_of_reaches`), and the hypothesis it needs — that a \
      message's seen set *is* `seenOf` of its justifications — is the port's own construction \
      (`message_state.rs:54-59`), stated rather than assumed. Falsified by mutation: a `seenOf` that \
      drops the union breaks both the one-step lemma and this proof's two steps. **And both things this \
      form did not cover arrived with the DAG model** (2026-09-24, `Rchain/Casper/Dag.lean`): the \
      literal id-based form is `seen_subset_of_mem_seen`, over `Dag.msg` — the id→message map this note \
      said the model would have to bring — with the port's construction (`Constructed`) and the DAG's \
      ordering facts (`Descends`: a parent resolves in the DAG and is strictly lower) as **hypotheses \
      rather than axioms**, because resolving an id back to a value needs uniqueness and a finite \
      descent. **And the descent is *enforced* rather than assumed** (H1b, `46c35b545`): `blockNumber` \
      refuses a block that names a resolved parent at or above its own number, the **failed** ones \
      included (`casper/src/validate.rs:226-230`), so `Descends` holds of **every state the port \
      admits** — where the reference validator, which filters failed parents out of its own \
      `blockNumber`, admits the violating block, the §6 deviation AUDIT C83 carries. The earlier plan \
      to weaken this to *unfailed* parents is therefore **superseded**: a weakened `Descends` forces \
      `unfailed` into `seen_subset_of_mem_seen`'s descent step, while the strong form is what the \
      ingress check guarantees. The per-sender reading of the height claim rests on `selfParents_skips_finalized`: the \
      walk filters by `!finalized.contains(x)` **and never traverses through an excluded message** (the \
      worklist is rebuilt from the survivors), so a min message is the previous sentinel's direct \
      successor rather than a descendant of something older. **And that settled which reading of the \
      height claim is true**: the cross-sender one is **false**, of this tree and of the oracle — \
      `prev = {m3 (sender 0, height 3)}` against a published `{q2 (sender 1, height 2)}`, because the \
      layer is built from the *justifications* and need not cover the senders `prev` covers, and the \
      gate cannot see it: what `calculate_fringe` reads is the **support map**, while \
      `Finalizer.scala:144-178` compares fringes *never* \
      (`LazyList.unfold(parentFringe)(nextFringe(_).map(nf => (nf, nf))).lastOption`). It is kept as \
      `cross_sender_height_monotone_is_false` rather than dropped, so the next reader sees which \
      reading was false and on what; AUDIT C69 carries the measurements, and §6 the port's own \
      termination guard — which the oracle does not have, and which is weaker than a cycle guard. \
      **What remains owed** is the last conjunct: the per-sender height *comparison* itself, the \
      mechanism proved and the arithmetic across a chain not — a small named remainder rather than the \
      two-sided gap this row used to carry. **And the statement it needs is sharper than it looks**: as \
      written the comparison is *false*, because the walk takes **every** same-sender parent, so a \
      same-sender fork on `mv`'s frontier puts a message from the other branch in its output, and that \
      branch is not below any given sentinel. **And the port does not merely happen to be fork-free — it \
      *refuses* a same-sender fork at ingress**, which is stronger than the datum this sentence used to \
      assert and is what the proof's hypothesis can lean on: H-1's equivocation detection rejects a \
      second distinct block by one sender reusing a `seq_num`, **before any partial write** \
      (`casper/src/dag.rs:244-252`, test `insert_rejects_equivocation_same_seq_num` at `:568`); \
      `sequence_number` requires a block to justify a same-sender block exactly one `seq_num` lower \
      (`casper/src/validate.rs:245-262`, test `sequence_number_must_be_creator_latest_plus_one` at \
      `:813`); and `check_justification_regression` admits at most one justification per sender and \
      demands that it be the latest (`casper/src/validate.rs:267-279`). **Why the port refuses it, and \
      why that refusal is a deviation the oracle does not share, is AUDIT C84** — the Scala has no \
      equivocation gate anywhere and its `validateDagState` checks only height contiguity, so the \
      premise this proof needs is **guaranteed here and only observed there**. H-1 landed **2026-08-20** \
      (`76415d6c6`), a month *before* the sentence it corrects was written — so the premise was carried \
      in good faith rather than checked, which is this programme's recurring class. The hypothesis is \
      therefore \"at most one same-sender parent per message\" — **guaranteed by an ingress refusal** \
      rather than observed of the data — and with it the comparison \
      follows from the boundary plus the descent. That is the shape of the last conjunct, named so \
      the next pass proves a true statement rather than a forked counterexample. The old row's claim that the seen set is monotone \"(no \
      regression)\" was true of the port and false of the value the axiom quantified over. **And the \
      owed conjunct's *step* was wrong until 2026-09-25** (AUDIT C93): `minMsgs` takes the *oldest* \
      non-finalized same-sender ancestor — the port's `chain.into_iter().last()` over \
      `[p] ++ self_parents p` — and the model read `getLast?` of `selfParents`, whose list is \
      **oldest-first** (the model prepends into its accumulator where the port `push`es its visit order, \
      so the port's chain is newest-first and its `.last()` is the oldest). Measured on a chain \
      `10 (h 0) ← 11 (h 1) ← 12 (h 2)`: the model's `selfParents` is `[10, 11]`, the port's chain is \
      `[11, 10]`, and the model answered `11` where the port answers `10` — the *newest* for the oldest. \
      **No test could see it**: every `decide`d instance in the file gave a sender at most one \
      same-sender ancestor, and with one element both ends agree, so a step of the derivation read the \
      opposite end of the chain while every check stayed green. The fix is `.head?`, pinned by \
      `chain3`'s two theorems (`the_walk_is_oldest_first` and `a_chain_of_three_picks_the_oldest`, the \
      instance that fails under `getLast?` — verified by reverting it), and the antichain above is \
      unaffected: which of two same-sender messages the layer fold keeps does not change its keys. \
      **What remains owed is still the comparison itself**, and it is now **one step narrower, with the \
      step named** (2026-09-25). A sentence here briefly said the obstacle was a *missing model* — that \
      `nextFringe` takes the next layer as an argument (`Fringe.lean:110`) so no two successive fringes \
      are related; **that was wrong**, a reading of `Fringe.lean` alone: `Rchain/Casper/Dag.lean`'s \
      `derivedFringe` derives the layer from the DAG *and* takes `prev` (`:140`), so the successive-fringe \
      relation exists. What is now **proved**, about the walk that derivation rests on, is the **sentinel \
      half**: `selfParents_above_a_finalized_ancestor` — for a justification, `minMsgs` returns a message \
      **strictly above every finalized same-sender ancestor of it** — under `NoFork`, the fork-freedom \
      ingress *refuses* rather than assumes (H-1 at the DAG insert, `sequence_number`, \
      `check_justification_regression`; AUDIT C82 and C84). Two things make it more than a spelling: the \
      walk's descent is proved first (`selfParents_height_lt`: everything the walk returns is reached \
      from the seed along same-sender parent edges, so `Descends` makes it strictly lower), and the \
      hypothesis is **falsified where it is dropped** — `the_comparison_is_false_without_fork_freedom` \
      exhibits a four-message fork where the walk steps *around* the finalized ancestor and returns a \
      height-1 message beneath a height-5 one. **What is still owed is the lift through the fold, and \
      *which* ingress rule it needs is a question two refutations narrowed without settling** — both \
      machine-checked rather than argued. `nextLayer` adds the min messages' *candidate parents* whose \
      sender is already a key (`:129-135`), and **neither fork-freedom nor the sequence rule excludes a \
      published message below the previous fringe**: `fold4` (two sender-0 blocks, `99` at height 2 and \
      `100` at height 5, neither with a parent, and `101` justifying `99`) is **fork-free** and still \
      publishes `99`, and `fold5` answers the next guess by adding `z` (seq 6) justifying `q` (seq 5) — so \
      every same-sender edge has consecutive `seqNum`s and the sequence rule holds — and it *still* \
      publishes the lower `c`, because `cands` is folded right-to-left and a candidate from an earlier \
      justification wins the sender's slot over the min message \
      (`the_fold_can_publish_below_the_previous_fringe`, with \
      `fold4_is_fork_free`, `fold5_is_fork_free` and `fold5_satisfies_the_sequence_rule` proving each \
      instance *has* the hypothesis it tests — the two refutation instances, and the two checks on the \
      fold that now mirrors the port). **And the fold itself was the thing in the way, which is now fixed rather than argued about.** The port's `calculate_next_layer` gives a candidate its sender's slot **only when its `sender_seq` is strictly greater** (`block-storage/src/dag/finalizer.rs:160-166`), and the port's own comment calls that guard *\"the Law 15 monotonicity invariant\"* (`message_state.rs:77`). `layerInsert` did not: it is the *seeding* rule (the port's `BTreeMap::collect`, last wins) and the model used it for the candidates too, justified by *\"the antichain does not depend on that choice\"* — true of law 14b's **keys**, false of the **message** law 15 is about. `seedLayer` and `insertCandidate` now carry both rules, and the difference shows on this file's own instance: `dag3`'s published layer was [10, 12] and is now [11, 12] — the model was publishing a **stale** same-sender message, which no law-14b check could see because the *keys* were right either way. **With the faithful fold the counterexample is `fold4` and nothing else**, stated per sender as law 15 states it: `the_fold_can_publish_below_the_previous_fringe` holds of a fork-free DAG whose `101` justifies the older `99` while `100` is finalized — a shape the **sequence rule** refuses, since `101`'s same-sender justification is not its sender's latest. `fold5` refutes nothing now: with the guard it publishes `z` (height 6) for sender 0 and the comparison holds (`the_guard_keeps_the_newer_message`, `the_comparison_holds_on_fold5`). **So the lift's hypothesis is the sequence rule, on a fold that mirrors the port** — which is what this row said before two of its own corrections, and it is now what the machine says rather than what a guess said. **And the lift is proved** (2026-09-25), so the row is no longer owed anything: the last two ingredients \
      are `seqNum_lt_height_lt` (on a sender, a higher `seqNum` is a higher height — from `SeqStep`, the \
      port's `sequence_number`, with `SeqUnique`, H-1's one-message-per-`seq_num`) and \
      `insertCandidate_height_le` (**the guarded insertion never lowers a sender's entry** — the port's \
      *\"Law 15 monotonicity invariant\"* as a theorem about this model's own fold). The composition is \
      `foldl_insertCandidate_height_le`: the step over the **whole** candidate list, whose accumulator is \
      generalized because the entry between two insertions is neither the seed's nor the layer's and it is \
      the one the next step's guard is about — and `nextLayer_height_le_seed` reads it where the row \
      needs it: a sender's entry in the layer `nextLayer` **publishes** is at a height no lower than the \
      **seeded** one. The comparison against `prev` is `minMsgs_above`, and the relation it needs is \
      exactly the one this row had named: every justification of a sender descends from the previous \
      fringe's message for that sender — the port's `sequence_number` plus \
      `check_justification_regression`, stated as the **hypothesis** it is rather than assumed, because \
      it is a fact about what a block may justify and not about the DAG value. \
      `nextLayer_above_the_previous_fringe` composes the two halves (per sender, the published layer lies \
      **strictly above** the previous fringe's message), and \
      `derivedFringe_above_the_previous_fringe` reads it at the `Fringe` `derivedFringe` returns — law \
      15's sentence with its hypothesis named. **What makes the hypotheses more than spelling**: `d15` \
      carries all four and a derivation that publishes (`d15_descends`, through a `Bool` mirror because \
      `Descends` quantifies over every message; `d15_is_fork_free`, `d15_is_sequence_unique`, \
      `d15_satisfies_the_sequence_rule`, each `decide`d), `d15_the_published_entry` pins the entry the \
      layer holds for the sender, and `d15_the_comparison_holds` is the theorem **applied** to it — \
      height 1 against height 2 — so no hypothesis is idle. The `rustWitness` is unchanged and stays a \
      **property test** (`block-storage/src/property_tests.rs:law15_adding_blocks_only_grows_the_state`): \
      this row's statement is the derivation-level fact, and the tie to the state it is tested against is \
      the port's own guard, which the fold above is now the model of rather than an approximation to" },
  { number := 16, clause := "a", layer := "Casper **This witness was strengthened on 2026-09-27 (AUDIT C140).** It asserted only that the seen set does not shrink and that the counts do not fall, and a DAG state that never changes satisfies both: making `add_block_to_dag_state` return its input left it green (measured), while five of law 18's tests and `add_block_to_dag_state_builds_child_map` went red. It now requires the block that was added to be *in* the new seen set and its height to be *in* the index — the growth the law is about, rather than its direction.",
    rustWitness := ["casper/src/validate.rs:block_number_must_be_parent_max_plus_one"],
    statement := "Block number = max(parent) + 1 — as the port's check, which **rejects** a block whose \
      number is not one more than the maximum of its non-failed justifications (`0` when there is none \
      live)",
    status := .provedTied,
    corpus := some "block",
    declarations := [`Rchain.Parent, `Rchain.maxParentNumber, `Rchain.BlockNumberValid,
      `Rchain.block_number_max_parent_plus_one, `Rchain.block_number_rejects,
      `Rchain.block_number_universal_is_false],
    axioms := [],
    rust := ["casper/src/validate.rs"],
    witness := [`Rchain.block_number_rejects, `Rchain.block_number_universal_is_false],
    falsifiable := some "`block_number_rejects` is the case the port returns `InvalidBlockNumber` for \
      (`validate.rs:236-240`): an off-by-one — `max + 2`, or `max` itself — fails it. The `-1` seed is \
      falsifiable on its own: a block with no live justification must be numbered `0`, so a model that \
      folded a maximum from `0` would demand `1` and reject the genesis-shaped case. And \
      `block_number_universal_is_false` exhibits the refutation of the axiom that stood here — which \
      quantified over every `Block` with no hypothesis at all",
    note := "**the axiom was false, not merely unproven**: it quantified over every `Block`, and a \
      `Block` is freely constructed, so one line refutes it (`block_number_universal_is_false`). The law \
      is re-scoped to the check the code has — a fold over the block's justifications, skipping the \
      failed ones and seeded `-1` (`validate.rs:222-234`) — and the proof is that predicate's \
      elimination, which is the honest shape: the port enforces this by *refusing blocks*, not by \
      maintaining an invariant it states. The model's `Block` carries `justifications` because the check \
      reads them; the `parents : List Nat` field this row's model used does not exist in the port" },
  { number := 16, clause := "b", layer := "Casper",
    rustWitness := ["casper/src/validate.rs:sequence_number_must_be_creator_latest_plus_one"],
    statement := "`seqNum` strictly increases **under the sender's justification**: the port requires the \
      block's `seqNum` to be one more than the maximum `seqNum` among the justifications whose sender is \
      this block's sender (`0` when there are none)",
    status := .provedTied,
    corpus := some "block",
    declarations := [`Rchain.senderLatestSeq, `Rchain.SeqNumValid, `Rchain.seq_num_strictly_increases,
      `Rchain.seq_num_universal_is_false],
    axioms := [],
    rust := ["casper/src/validate.rs"],
    witness := [`Rchain.seq_num_universal_is_false, `Rchain.seq_num_strictly_increases],
    falsifiable := some "a block whose `seqNum` skips or repeats the sender's latest justification is \
      rejected (`InvalidSequenceNumber`, `validate.rs:259-262`), and the `-1` seed is a case of its own: \
      a sender's first block must be `0`. `seq_num_universal_is_false` is the published refutation of \
      the axiom this replaces — and it refutes it **for a single sender**, which is why the re-scoping \
      is the justification relation and not the sender relation",
    note := "**the axiom was false as stated** (`prev.seqNum + 1 = next.seqNum` over any two blocks), and \
      the diagnosis in the row it replaces — \"the sender relation is missing\" — was wrong: a same-sender \
      pair with a non-consecutive `seqNum` refutes it just as well \
      (`seq_num_universal_is_false`). What the check folds over is the block's justifications **whose \
      sender matches**, against their maximum (`validate.rs:245-262`); the law is re-scoped to that, and \
      the proof is the predicate's elimination. The old model also carried a `seqNum`-ordering axiom over \
      any two blocks, which no port rule states" },
  { number := 16, clause := "c", layer := "Casper",
    rustWitness := ["models/src/casper/protocol/casper_message.rs:law16_to_proto_sorts_justifications"],
    statement := "Content addressing: `hash_block` clears `block_hash` and `sig` and hashes every other \
      proto field canonically, so equal hashes determine equal bodies **that the serializer can \
      represent** (`Canonical`: numbers inside `int64`, justifications in the port's sorted order)",
    status := .provedTied,
    corpus := some "body",
    declarations := [`Rchain.Block, `Rchain.BlockBody, `Rchain.Block.body, `Rchain.Parent.key,
      `Rchain.Canonical, `Rchain.sortParents, `Rchain.sortParents_of_pairwise, `Rchain.encodeParent,
      `Rchain.decodeParent, `Rchain.decodeParent_encodeParent, `Rchain.decodeParents,
      `Rchain.decodeParents_length_encodings, `Rchain.encodeBody, `Rchain.canonicalise,
      `Rchain.taggedVarint, `Rchain.taggedVarint_varint, `Rchain.taggedVarint_varintField,
      `Rchain.taggedVarint_bytesField, `Rchain.decodeBody, `Rchain.decodeBody_encodeBody,
      `Rchain.encodeBody_injective,
      `Rchain.the_body_encoder_is_not_injective_without_canonical,
      `Rchain.blockHash, `Rchain.content_addressing, `Rchain.blockHash_changes_with_header,
      `Rchain.a_body_encoder_that_truncates_is_not_injective,
      `Rchain.a_body_encoder_that_canonicalises_is_not_injective],
    axioms := [],
    rust := ["casper/src/proto_util.rs", "models/src/casper/protocol/casper_message.rs"],
    witness := [`Rchain.the_body_encoder_is_not_injective_without_canonical, `Rchain.a_body_encoder_that_truncates_is_not_injective, `Rchain.a_body_encoder_that_canonicalises_is_not_injective, `Rchain.content_addressing, `Rchain.blockHash_changes_with_header],
    falsifiable := some "`content_addressing` composes Law 19's `blake2b256_collision_free` with \
      `encodeBody_injective`, so a serializer that dropped a field would falsify it — and \
      `blockHash_changes_with_header` is the code's own `hash_block_changes_with_timestamp` \
      (`proto_util.rs:155-162`) derived: two blocks differing in **any** hashed field must hash \
      differently, which is why the informational timestamp is in the body even though no consensus rule \
      reads it. A `hash_block` that stopped clearing `sig` would falsify \
      `hash_block_is_deterministic_and_ignores_sig` (`:138-144`) and, transitively, this. **And the \
      canonicity axiom was falsified before it was narrowed**, two ways, because the model's body is \
      wider than the bytes the port writes: \
      `a_body_encoder_that_truncates_is_not_injective` (the proto's `blockNumber` is an `int64`, \
      `models/proto/casper.proto:49`, so any encoder that mirrors it identifies `2^63` with `2^63 + 2^64`) \
      and `a_body_encoder_that_canonicalises_is_not_injective` (`to_proto` sorts the justifications \
      before hashing, `casper_message.rs:636`, so it cannot distinguish a body from the same body \
      permuted). **And the axiom is gone**, so the refutation is now about the *definition the port \
      runs* rather than about a hypothesis: `the_body_encoder_is_not_injective_without_canonical` \
      exhibits `2^63` and `2^63 + 2^64` as one image of `encodeBody` itself, and `Canonical`'s number \
      bound is what recovers the value from the residue (`int64_eq_of_lt`, `Rchain/Proto.lean`). Drop \
      `Canonical` from `encodeBody_injective` and the proof breaks at `decodeBody_encodeBody`, which \
      returns `canonicalise b` — a different body exactly when the hypothesis fails",
    note := "**the old row was a postulate about a field no function computed** — `hash = \
      Blake2b256(block − {hash, sig})` over `Block.hash : Nat`, which the note below it admitted was not \
      true even of its own model (an injective `Nat → Nat` does not exist). `Block.hash` is gone; the \
      hash is *computed* (`blockHash`), and the content addressing is a **theorem**: the composition of \
      Law 19's collision-freedom with the serializer's canonicity. The two axioms that remain are Law \
      10's shape and for the same stated reason — a hash can make an encoding collision-resistant but \
      never canonical, so `encodeBody`'s injectivity is the serializer's assumption, not the hash's. \
      The body is the **hashed** body: the port hashes every field except `block_hash` and `sig` \
      (`proto_util.rs:58-64`, `BlockMessage` at `casper_message.rs:563-585`), so the model carries the \
      timestamp and the rest of the header, and `blockHash_changes_with_header` is the statement a \
      narrower body could not make. **What the third pass found (2026-09-23)**: the canonicity axiom \
      was still stated over types wider than the port's — a `Nat` block number against the proto's \
      `int64`, and `justifications` in arrival order against the sorted order `to_proto` writes — so it \
      was false of any encoder the port could be using, and the two refutation theorems above say so \
      with witnesses. The statement was narrowed to `Canonical` rather than the model widened (the \
      widths and the sorted order are properties of the *code*, which the model's validation rules \
      deliberately do not carry, since they fold over the list in any order). **The fourth pass \
      (2026-09-23) discharged it**: `encodeBody` is now a **definition** (`Rchain/Casper/Validate.lean`) \
      over protobuf's wire format (`Rchain/Proto.lean`: the base-128 `varint` and its self-delimiting \
      decode, `varintField`, `bytesField`, the 64-bit narrowing), with the round trip \
      (`decodeBody_encodeBody` — the encoder's own inverse returns `canonicalise b`, which is the \
      hypothesis's whole content) and injectivity (`encodeBody_injective`) as theorems about it, so this \
      row's axiom citation is empty. **What is still prose, and is named rather than implied**: `prost`'s \
      `encode_to_vec` is an external crate (0.13.5, `Cargo.lock`), so \"these bytes are the node's \
      bytes\" is a tie no Lean theorem can make — and it is now a tie the row **carries**: `corpus := \
      some \"body\"`, the layer that was the follow-up when this sentence was written (`Corpus.lean`'s \
      `bodyCases`, `emit-lean-corpus.sh`'s `LAYERS`, `spec/conformance/body.tsv`, the consumer \
      `node/tests/lean_body_corpus.rs`, the gate's layer-to-consumer map). The row is `proved-tied` on \
      it. **What the layer had to overcome was measured before it was built rather than after** (AUDIT \
      C57): the model's `encodeBody` and `prost` disagree in three ways that a byte comparison hits on \
      its first case — the model writes tags 4, 6, 5, 17, 9, 14 where `prost` writes the `.proto`'s \
      ascending order; `prost` omits default-valued fields and the model writes unconditionally; and the \
      model collapses nine fields \
      into one opaque `header` blob at tag 14, which `prost` reads as `state`. **What the layer covers is \
      the subset where the two structures correspond** — six fields, with the fields it does not model \
      and `justifications`' shape named as its boundary in the consumer's own doc — so what remains is \
      C57's \
      *option (a)*: mirroring `prost` over the full field list is a modelling change and not a plumbing \
      one, and it would put `decodeBody_encodeBody`/`encodeBody_injective` in play for those fields too. \
      C57 records the three \
      divergences, the compile-time hazard below, and the bounded alternative. **A \
      measurement worth keeping**: the first spelling of `decodeBody` was a `guard`-per-tag do-block, \
      and it did not slow the *elaborator* down — it slowed the **compiler** down: `lean --profile` \
      reports 128 ms of elaboration and `compilation of Rchain.decodeBody took 98.6s`, with the build \
      swelling past 5 GB until it aborted. One `taggedVarint` per field compiles in 224 ms" },
  { number := 16, clause := "d", layer := "Casper",
    statement := "The bonds cache equals the PoS state",
    status := .provedModel,
    declarations := [`Rchain.ActiveBonds, `Rchain.bondsOfState, `Rchain.Justification,
      `Rchain.newestJustification, `Rchain.bondsFromNewestState, `Rchain.bondsFromCarried,
      `Rchain.the_sources_agree, `Rchain.a_disagreeing_set_is_refused,
      `Rchain.a_bond_change_between_justifications_is_refused,
      `Rchain.a_bond_does_not_move_the_map_the_gates_read,
      `Rchain.the_pool_derived_map_moves_where_the_states_does_not],
    rust := ["casper/src/multi_parent_casper.rs", "casper/src/runtime_manager.rs",
      "rholang/src/native_state.rs"],
    rustWitness := ["casper/tests/consensus.rs:the_bond_cache_is_the_active_pos_state_and_a_differing_one_is_refused"],
    witness := [`Rchain.a_bond_change_between_justifications_is_refused,
      `Rchain.a_bond_does_not_move_the_map_the_gates_read],
    falsifiable := some "the two sources of the bonds map disagree exactly when the bond set moves across \
      the justifications, and there the fallback **refuses** rather than picking one \
      (`a_bond_change_between_justifications_is_refused`: validator 7 at stake 1 in the older state, 2 \
      in the newer — what law 44's gate lets happen between boundaries — gives `none` from the carried \
      maps and `some [(7, 2)]` from the newest state's). So the equality is a claim about an **honest, \
      unmoved** set of justifications, not a tautology: drop either hypothesis and the statement is \
      false, which is why the port reads the state wherever it can. The **state's** side is falsifiable \
      the same way, and it was measured: make `a_bond_does_not_move_the_map_the_gates_read` read `pool` \
      instead of `active` and it stops being `rfl` — the build fails, which is what the theorem's \
      partner asserts",
    note := "**the state's side is closed: the model now carries the map it was missing, and the map \
      the gates read is derived from it** (2026-09-25, closing AUDIT C92). The row was `owed` because \
      the model could not *hold* the map — `active` was a `List Validator`, ids with no stakes, so the \
      only stake-carrying field was `pool`, and a derivation from `pool` answers with the *current* \
      stakes, which for a validator that bonded after the last boundary is a different map from the \
      port's. `Rchain/Pos.lean`'s `active` is now `List (Validator × Nat)`, the pairs `select_active` \
      returns, and `reselect` keeps them instead of dropping them (`.map (·.1)` was the loss). With \
      that, the state's side is statable, and it is stated as **a pair, because either half alone is a \
      spelling**: `a_bond_does_not_move_the_map_the_gates_read` — a bond writes `pool` and leaves the \
      map the gates read exactly as it was, which is law 44's gate seen from the finalizer's side — and \
      `the_pool_derived_map_moves_where_the_states_does_not`, where the same bond moves a pool-derived \
      reading by one entry and no hypothesis is needed, because `bond` prepends unconditionally. \
      Together they say the port's reading of `pos:active` rather than `pos:bonds` is load-bearing \
      rather than cosmetic. **The sync site is unchanged**: `the_sources_agree` and \
      `a_disagreeing_set_is_refused` still mirror the port's three sources. **Law 44's \
      `a_boundary_activates_the_pool` is strictly stronger for the same reason** — it assumed \
      `s'.claims = []` only because an id-only active set could equal step 4's output when the filter \
      removed nobody, and carrying the stakes makes the conclusion the filter itself, so that \
      hypothesis became *inert* rather than the claim becoming weaker. **What this row still does not \
      say**, stated rather than implied: nothing here proves the *port* reads `pos:active` — that is \
      the Rust's own shape (`runtime_manager.rs:1374-1380`) — so this is a `proved-model` tie, and the \
      model change carried laws 44-47 with it" },
  { number := 17, clause := "a", layer := "Casper",
    rustWitness := [
      "sdk/src/property_tests.rs:law17_deploys_without_conflicts_need_no_rejection",
      "sdk/src/property_tests.rs:law17_the_chosen_rejection_is_one_of_the_options",
      "sdk/src/property_tests.rs:law17_the_chosen_rejection_minimizes_the_total_cost",
      "sdk/src/property_tests.rs:law17_the_survivors_of_a_rejection_option_are_conflict_free"],
    statement := "Merge determinism: a rejection resolves to a unique minimum-cost candidate — the \
      rejection option is the minimum of `(total cost, size, the sorted set)`, and a minimum of a set \
      is unique, so the resolution is a function of the conflict set and not of the iteration order",
    status := .provedModel,
    declarations := [`Rchain.RejectionOption, `Rchain.totalCost, `Rchain.optionKey,
      `Rchain.optionKeyComparator, `Rchain.optionComparator, `Rchain.pickRejection,
      `Rchain.pickRejection_eq_none_iff, `Rchain.pickRejection_mem, `Rchain.pickRejection_minimal,
      `Rchain.equal_cost_and_size_do_not_make_equal_options, `Rchain.the_minimum_is_unique,
      `Rchain.the_resolution_does_not_depend_on_the_iteration_order, `Rchain.Comparator.le_of_not_lt],
    axioms := [],
    rust := ["sdk/src/dag/merging.rs", "casper/src/merging.rs"],
    witness := [`Rchain.equal_cost_and_size_do_not_make_equal_options, `Rchain.the_minimum_is_unique, `Rchain.the_resolution_does_not_depend_on_the_iteration_order],
    falsifiable := some "the linearity the word *unique* needs is itself checked: \
      `equal_cost_and_size_do_not_make_equal_options` exhibits two options that agree on the key's \
      first two components (`[1, 2]` and `[1, 3]`, both cost 2 and length 2 under a unit cost) that the \
      comparator still separates (`cmp = lt`) — drop the set from the key and they compare equal, \
      `min_by` returns whichever the `BTreeSet` yielded first, and \
      `the_resolution_does_not_depend_on_the_iteration_order` is false. The port's own test asserts the \
      same case (`compute_optimal_rejection_minimizes_cost_then_size`: `{1}` must win)",
    note := "**The note this row carried was wrong, and the code was right** (found 2026-09-23, \
      Programme D unit 10). It said the port \"does not choose among candidates, so a claim about a \
      unique minimum-cost candidate has nothing in the code to be stated against\". It does choose: \
      `resolve_conflict_set` (`sdk/src/dag/merging.rs:395`) closes the conflict map under \
      dependencies, computes the rejection options, extends them with what an overflow forces, and \
      `compute_optimal_rejection` (`:279-295`) picks one — `options.iter().min_by(|a, b| (cost(a), \
      a.len(), a).cmp(&(cost(b), b.len(), b)))`. The row's word *unique* is therefore load-bearing \
      rather than decorative, and it is exactly the property the model proves: the key is linear, so \
      no two distinct options compare equal, so the minimum is unique, so `min_by` over a `BTreeSet` \
      cannot be observed to depend on iteration order. The model is of the *selection*; the conflict \
      predicate and the branch sets it runs over are Law 9's (`are_conflicting`, \
      `rspace/src/merger/event_log_merging_logic.rs:100-158`), which is modelled there" },
  { number := 17, clause := "b", layer := "Casper",
    rustWitness := ["rholang/src/merging.rs:calculate_diff_rejects_i64_overflow_instead_of_wrapping"],
    statement := "The merge's arithmetic is the checked 64-bit one — a value that would leave `i64` is \
      **refused, not wrapped** — and the merged RNG is a function of the *set* of branch generators",
    status := .provedModel,
    declarations := [`Rchain.checkedAdd, `Rchain.checkedSub, `Rchain.mergeRandoms,
      `Rchain.checkedAdd_refuses_overflow, `Rchain.checkedSub_refuses_overflow,
      `Rchain.merge_diff_round_trip, `Rchain.mergeRandoms_perm],
    rust := ["rholang/src/merging.rs", "rspace/src/merger/event_log_index.rs"],
    axioms := [],
    witness := [`Rchain.checkedAdd_refuses_overflow, `Rchain.checkedSub_refuses_overflow, `Rchain.mergeRandoms_perm],
    falsifiable := some "the refusal is a witness rather than a remark: `checkedAdd i64Max 1 = none` \
      and `checkedAdd i64Min (-1) = none` (`checkedAdd_refuses_overflow`), which a `checkedAdd` that \
      wrapped would fail; `mergeRandoms_perm` is the statement that the merged RNG does not depend on \
      the order branches arrived in, false the moment the caller's sort is removed",
    note := "**the law that stood here was false**: `numeric_channels_nonneg` claimed numeric channels \
      are non-negative, and they are signed `i64` with ordinary negative diffs \
      (`rholang/src/merging.rs:161-166`, the `NumberChannel` struct; the tests are \
      `mergeable_data_round_trips` (`:414`, whose `diff: -5` literal is at `:426`) and \
      `calculate_diff_handles_negative_and_absent_keys` (`:389`)). The non-negativity that \
      *is* true belongs to Law 14's bonds and to `NonNegI64` (`shared/src/refined.rs:64`), which types \
      bonds and heights, never numeric channels. **And the arithmetic is only half checked**: the merge \
      result uses `checked_add` (`rholang/src/merging.rs:102`) while the diff accumulator *was* a plain \
      `i64 +=` (`EventLogIndex::combine`, `rspace/src/merger/event_log_index.rs:156`, and the merge's \
      own fold at `casper/src/merging.rs:891` — both `checked_add` now) — a debug panic, a \
      release wrap. That half was a code finding (AUDIT C41) and is **fixed**: the accumulation is
      checked now and its error reaches the merge, with `combining_refuses_a_diff_that_leaves_i64`
      (`rspace/src/merger/event_log_index.rs`) failing on a `wrapping_add`" },
  { number := 17, clause := "c", layer := "Casper",
    rustWitness := [
      "casper/src/merging.rs:a_boundary_round_does_not_lose_a_contained_deploys_charge",
      "casper/src/merging.rs:a_rejected_boundary_chain_does_not_take_its_blocks_other_chains",
      "casper/src/merging.rs:the_merge_report_names_every_dropped_native_write",
      "rspace/src/native_store.rs:a_drained_write_names_the_deploy_that_made_it",
      "rspace/src/native_store.rs:a_write_outside_any_window_is_not_attributed_to_a_block"],
    statement := "The native relation is on the **chain**, not on the block: two concurrent native \
      writers of one slot conflict, and a chain that wrote nothing contended is not a conflict \
      partner at all — so a user deploy's effects cannot die with a boundary its block also carried",
    status := .provedModel,
    declarations := [`Rchain.Chain, `Rchain.chainConflict, `Rchain.hostSlots,
      `Rchain.hostConflict, `Rchain.incidentChains, `Rchain.foldRefuses, `Rchain.resolves,
      `Rchain.host_keys_conflict_where_the_chains_do_not,
      `Rchain.the_resolution_leaves_the_fold_nothing_to_refuse,
      `Rchain.the_fold_refuses_concurrent_writers_of_one_slot],
    axioms := [],
    rust := ["casper/src/merging.rs", "rspace/src/native_store.rs", "rholang/src/merging.rs"],
    witness := [`Rchain.host_keys_conflict_where_the_chains_do_not,
      `Rchain.the_resolution_leaves_the_fold_nothing_to_refuse,
      `Rchain.the_fold_refuses_concurrent_writers_of_one_slot],
    falsifiable := some "Two halves, and the falsifier is the first. \
      `host_keys_conflict_where_the_chains_do_not` exhibits the incident's own shape — one block \
      carrying a boundary chain that wrote `{1,2}` and a user chain that wrote ∅, plus a concurrent \
      sibling writing `{1,2}` — and shows the **block-level** relation holding of the user chain while \
      the **chain-level** one does not: that is the rule that stood, refuted on the pair of chains it \
      rejected, and it fails the moment the relation is keyed on the host again. \
      `the_resolution_leaves_the_fold_nothing_to_refuse` is the positive half and is a *bridge*, not a \
      restatement: `resolves` is stated on `chainConflict` and `foldRefuses` on its own \
      shared-slot-and-unsequenced shape, so drop the overlap conjunct from `chainConflict` and two \
      concurrent writers of one slot are kept, the fold's own `Err` fires, and the theorem is false. \
      `the_fold_refuses_concurrent_writers_of_one_slot` stops the refusal being an empty predicate — \
      without it the positive half would be a statement about a shape nothing can have.",
    note := "**What this row does not model, and why that is deliberate.** The *selection* over the \
      rejection options is law 17a's and is unchanged; the **option enumeration** over a chain-level \
      conflict graph — the maximal conflict-free sets and the budget that bounds them — is still not \
      modelled here, because it would be a second model of `compute_rejection_options` \
      (`sdk/src/dag/merging.rs`) and its subject is not what the defect falsified. What is modelled is \
      the relation and the acceptance predicate, which is. **The defect it answers** is the live one: \
      `spec/audit/evidence/n280-merge-loses-a-write-results.md` records a four-validator net losing a \
      deployed write at an epoch boundary, unanimously, with the boundary blocks' `CloseBlock` ids in \
      the rejected set and no node refusing a peer's block. The Rust side is structural as well as \
      proved — `spec/TYPE-SYSTEM.md` — because the block-level *set* is not constructible any more: \
      `BlockIndex.native_changes` is gone, `DeployChainIndex::native_effects` carries each chain's own \
      writes, and `reject_whole_blocks` is deleted rather than narrowed, so 'reject every chain of a \
      block because of a write the block made' has no operation left to express it." },
  { number := 17, clause := "d", layer := "Casper",
    rustWitness := [
      "rholang/src/merging.rs:a_legacy_sidecar_decodes_as_unattributed_and_not_as_empty",
      "rholang/src/merging.rs:an_attributed_sidecar_round_trips_and_a_damaged_one_is_not_read_as_empty",
      "rspace/src/native_store.rs:a_genesis_write_is_named_and_does_not_travel_in_a_sidecar"],
    statement := "A block's native effects are **attributed to the deploy that made them, by \
      construction**: an unattributed, block-level native-effect set is not a value — the sidecar's \
      shape has no arm for it, and a write outside any deploy's window is refused rather than filed \
      under the block",
    status := .vacuous,
    declarations := [`Rchain.Chain],
    axioms := [],
    rust := ["rspace/src/native_store.rs", "rholang/src/merging.rs"],
    falsifiable := some "the *negative* half is the machine half, as in law 22: the two decode tests \
      pin that a record written before attribution existed reads as `LegacyUnattributed` with its \
      actions intact and **not** as `Attributed(empty)` — collapsing either way is how a block's \
      effects come to be filed under the block or to vanish — and that a truncated or \
      trailing-byte record is an `Err` rather than an empty one. What would reopen the row is a second \
      constructor taking a bare action list, which is the shape the type exists to remove.",
    note := "**`vacuous` in the register's own sense, and the reason is law 22's.** The positive half — \
      \"an unattributed native-effect set is not a value\" — is a fact about a **type**, not a \
      theorem: `BlockNativeEffects` has a private field, no `Deref` and no getter, and its keys are \
      deploy ordinals, so there is no key that could mean \"the block\". Stating it in Lean would \
      prove that a type ignores a constructor nobody writes, which is the deleted \
      `next_step_closure_computable` shape this register already refused once. The artifact is \
      `rspace/src/native_store.rs`'s `BlockNativeEffects` + `NativeWriter` (whose `OutsideAnyDeploy` \
      arm is refused by `from_drain` rather than attributed to the block, and whose `Genesis` arm is \
      *named* rather than unattributed, and does not travel), the codec's three-way `SidecarRecord` \
      in `rholang/src/merging.rs`, and the gate that keeps the discipline: \
      `tools/audit-type-system.sh`'s construction scan. **Not a refinement in that gate's roster**, \
      deliberately: the gate's `escape` class is for types carrying a *predicate* (a validator the \
      constructor must run), and this type carries none — its invariant is its key type." },
  { number := 18, layer := "Storage",
    rustWitness := [
      "block-storage/src/dag/metadata_store.rs:law18_contiguous_height_map_is_valid",
      "block-storage/src/dag/metadata_store.rs:law18_height_map_with_holes_errors",
      "block-storage/src/property_tests.rs:law18_a_contiguous_chain_validates",
      "block-storage/src/property_tests.rs:law18_a_chain_with_a_hole_in_the_middle_is_refused",
      "block-storage/src/property_tests.rs:law18_a_missing_lowest_height_is_not_a_hole",
      "block-storage/src/property_tests.rs:law18_the_empty_dag_and_a_lone_block_are_contiguous",
      "block-storage/src/property_tests.rs:law18_the_state_does_not_depend_on_insertion_order",
      "models/src/block_metadata.rs:law18_fringe_hash_is_order_independent"],
    statement := "The store's own invariants: the height map is **contiguous** — no holes in block \
      heights — and the fringe identity is **order-independent**, because what the code keys on is a \
      `BTreeSet`, not a list",
    status := .provedModel,
    declarations := [`Rchain.HeightSet, `Rchain.Contiguous, `Rchain.contiguous_insert_succ,
      `Rchain.contiguous_skip_leaves_hole, `Rchain.height_map_universal_is_false, `Rchain.Fringe,
      `Rchain.fringeId, `Rchain.fringeId_perm,
      `Rchain.fringe_identity_order_independent_is_false],
    axioms := [],
    rust := ["block-storage/src/dag/metadata_store.rs", "models/src/block_metadata.rs",
      "block-storage/src/dag/finalizer.rs"],
    witness := [`Rchain.height_map_universal_is_false, `Rchain.fringe_identity_order_independent_is_false, `Rchain.contiguous_skip_leaves_hole, `Rchain.fringeId_perm],
    falsifiable := some "`contiguous_skip_leaves_hole` is the negative case the store's check exists for: \
      a block whose number is not the successor of the current maximum leaves a hole (what \
      `validate_dag_state` reports, `metadata_store.rs:83-86`) — and the store **cannot derive** the \
      positive direction for itself, since it inspects only the keys it is handed: that step runs through \
      Law 16a's block-number check. `fringeId_perm` fails for an identity that dropped the sort, which \
      is what `fringe_hash_of` would be without its `BTreeSet` input (`models/src/block_metadata.rs:31-34`), \
      and `height_map_universal_is_false` / `fringe_identity_order_independent_is_false` publish the \
      refutations of the two axioms this row replaces",
    note := "**two axioms gone, and neither was a law.** `height_map_contiguous` claimed a property of \
      *any* `List Block` — false, and refuted in the tree (a one-element list numbered 1 has an element \
      above 0 and no element at 0); what the port has is an invariant its *store* checks \
      (`metadata_store.rs:77-87`), so the model states contiguity as the store's meaning and proves the \
      two steps that matter: a successor insert preserves it, a skipped number breaks it. \
      `fringe_identity_order_independent` claimed `Perm → f = g`, also false of a hand-built `Fringe` \
      (refuted in the tree) — in the port the property is **structural**: `fringe` is a \
      `BTreeSet<BlockHash>` wherever it appears, so there is no list order to be invariant under, and the \
      model keeps a list only so the claim can be stated at all, then sorts (`fringeId`) and proves the \
      invariance the type supplies. **Clause split removed**: 18a and 18b now share a status, an empty \
      axiom set and a file" },
  { number := 19, layer := "Crypto",
    rustWitness := [
      "crypto/src/hash/blake2b512_random.rs:empty_gives_a_predictable_result",
      "crypto/src/hash/blake2b512_random.rs:merge_with_two_children",
      "crypto/src/hash/blake2b512_random.rs:merge_with_many_children",
      "crypto/src/hash/blake2b512_random.rs:merge_is_order_sensitive",
      "crypto/src/signatures/secp256k1.rs:creates_known_ecdsa_signature",
      "crypto/src/signatures/secp256k1.rs:verifies_known_signature",
      "crypto/src/encryption/curve25519.rs:decrypts",
      "crypto/src/encryption/x25519.rs:rfc7748_section_5_2_first_vector",
      "crypto/src/encryption/x25519.rs:rfc7748_section_5_2_second_vector",
      "crypto/src/encryption/x25519.rs:rfc7748_section_6_1_diffie_hellman_agrees_both_ways"],
    statement := "Blake2b256 is canonical and collision-free; the `Blake2b512Random` merge is n-ary \
      and **order-sensitive**; signatures verify what they sign; Curve25519 round-trips",
    status := .axiomByDesign,
    declarations := [`Rchain.blake2b256, `Rchain.blake2b256_output_is_32_bytes,
      `Rchain.blake2b256_collision_free, `Rchain.sign, `Rchain.verify,
      `Rchain.sign_verify_roundtrip, `Rchain.sharedSecret, `Rchain.curve25519_roundtrip,
      `Rchain.mergeRandom],
    rust := ["crypto/src/hash/blake2b256_hash.rs", "crypto/src/hash/blake2b512_random.rs"],
    axioms := [`Rchain.blake2b256, `Rchain.blake2b256_output_is_32_bytes,
      `Rchain.blake2b256_collision_free, `Rchain.sign, `Rchain.verify,
      `Rchain.sign_verify_roundtrip, `Rchain.sharedSecret, `Rchain.curve25519_roundtrip,
      `Rchain.mergeRandom],
    falsifiable := some "the Rust side is pinned by known-answer tests rather than by these axioms: \
      `crypto/`'s `Blake2b512Random`/`Secp256k1`/`Curve25519` vectors are what would catch a wrong \
      primitive, so the axioms are a *boundary* of the model, not a claim the model establishes",
    note := "the only `axiomByDesign` law, and where a **false axiom** was found: `mergeRandom_comm` \
      claimed the RNG merge commutes and the code's own test refutes it \
      (`crypto/src/hash/blake2b512_random.rs:548`, `merge_is_order_sensitive`). The merge is modelled \
      n-ary (`List Random → Random`) because that is its Rust signature, and *no* algebraic law of it \
      is claimed — the code has none to claim, so `mergeRandom_assoc` went with the commutativity. Law \
      17's RNG clause duplicated this one and is merged into it. **The types were retyped to mirror the \
      code's**: `Msg`/`Hash` are byte strings rather than opaque `Nat` wrappers, and the 32-byte width \
      is `blake2b256_output_is_32_bytes` (`Hash32`, `shared/src/refined.rs:302-311`) — which is what \
      lets Law 7's and Law 10's models be built out of hashes instead of guessed numbers. **And the \
      nine are not all load-bearing — measured 2026-09-25** with `Lean.collectAxioms` over every \
      declaration the register names: `blake2b256` is used by 7 of them (the block hash, the Merkle \
      node hash, content addressing), `blake2b256_collision_free` by 4, `mergeRandom` by 2, and Law 7's \
      `hashHashes` by 1 — while `blake2b256_output_is_32_bytes`, `sign`, `verify`, \
      `sign_verify_roundtrip`, `sharedSecret` and `curve25519_roundtrip` are used by **none**. Those \
      six are the crypto API stated as the model's boundary, so that laws about it are statable, not \
      assumptions any proof leaned on; the register now says which is which instead of leaving nine \
      axioms looking equally exercised. **The tie is a witness the gate runs** (`rustWitness`): the \
      known-answer vectors — the RNG's fixed empty-input stream and its two merge cases, a known ECDSA \
      signature and its verification, the `Curve25519` **sealed-box** round-trip \
      (`crypto/src/encryption/curve25519.rs:decrypts`, a `crypto_box` vector), and — since the OCapN \
      Noise transport needed the raw primitive rather than a box — RFC 7748's own `X25519` vectors, \
      §5.2's two and §6.1's Diffie-Hellman example (`crypto/src/encryption/x25519.rs`) — because they \
      are what catches a wrong \
      primitive. No test \
      can witness the idealization itself: `blake2b256_collision_free` states collision-*resistance* as \
      injectivity, which the pigeonhole refutes as a fact about the real function \
      (`spec/Rchain/Crypto/Spec.lean:41-49`)" },

  -- ── Scheduler: the effect scheduler (Laws 20–25) ─────────────────────────────────────────────────
  { number := 20, layer := "Scheduler",
    rustWitness := ["rspace/src/property_tests.rs:law20_per_channel_path_order"],
    statement := "Channel-task linearization (\"1 channel = 1 logical task\"): same-channel ops commit \
      in DFS path order through a per-channel claim queue, and the path-smallest pending claim is \
      always committable",
    status := .provedModel,
    declarations := [`Rchain.queue_commit_path_ordered, `Rchain.pathSorted_head_minimal],
    rust := ["rspace/src/concurrent/channel_queue.rs"],
    witness := [`Rchain.queue_commit_path_ordered, `Rchain.pathSorted_head_minimal],
    falsifiable := some "`queue_commit_path_ordered` is an induction on the run — a commit rule that \
      removed a claim which was not head-on-all would falsify it — and `pathSorted_head_minimal` is \
      the bakery argument's core: an unordered queue, or a head with a smaller path behind it, would \
      falsify that",
    note := "the axiom that stood here (`law20_deadlock_freedom`) is **deleted**, and the reason is a \
      finding: it quantified over every `Chan → Queue` and claimed some claim is head-on-all, which is \
      unprovable as stated — `PathLt` is not well-founded on paths (`[0] > [0,0] > [0,0,0] > …`), so an \
      unbounded pending set need have no minimum. The real system's paths are bounded by the depth of \
      the term being reduced and its pending set is finite, so the corrected statement carries that \
      finiteness as a hypothesis; what is proved here is the queue-level core, which needs none. The \
      Rust's liveness evidence is stress tests plus the queue's own deferral of cross-channel cycles \
      (`rspace/src/concurrent/channel_queue.rs:24-28`, risk R3) — a tested property, not a theorem. \
      **Also found while proving it**: `queue_commit_path_ordered` carried an unused \
      `hinit : ∀ ch, PathSorted (q0 ch)` — the same unused-hypothesis shape as Law 24's — and it is \
      gone, because the suffix property comes from the commit rule rather than from the initial sorting" },
  { number := 21, layer := "Scheduler",
    rustWitness := ["rholang/src/property_tests.rs:law21_the_gate_scheduler_refines_the_sequential_reference"],
    statement := "DFS-gate linearization: the gate scheduler refines sequential `Effect.apply`; one-hop \
      next-step pruning is unsound",
    status := .provedModel,
    declarations := [`Rchain.gate_await_closure_orders, `Rchain.one_hop_depth2_diverges],
    rust := ["rholang/src/reduce.rs", "rholang/src/scheduler.rs"],
    witness := [`Rchain.one_hop_depth2_diverges, `Rchain.gate_await_closure_orders],
    falsifiable := some "`one_hop_depth2_diverges` is a proved depth-2 counterexample to the pruning \
      rule the law forbids — the law and the disproof of its tempting weakening are published together \
      — and `gate_await_closure_orders` is false for a chain with a missing link (a dependency of \
      `j + 2 = i` would leave odd-indexed tasks unordered)",
    note := "`gate_exec_refines_apply` is **gone**: it defined `gateApply` as the sequential fold and \
      then proved the fold is the fold. The Rust's own comment above the gate says the same — 'Not a \
      speedup — the sound, sequential-equivalent carrier' (`reduce.rs:2568-2569`, in the gate's own comment) — so the content was \
      never in the identification. It is in the **dependency structure**, and that is what \
      `gate_await_closure_orders` proves: the immediate-predecessor await chain is transitively \
      complete, which is exactly the Rust's 'a linear chain of awaits, not the quadratic \
      all-predecessors join' (`reduce.rs:2570-2575`)" },
  { number := 22, layer := "Scheduler",
    rustWitness := ["rholang/src/reduce.rs:law22_the_next_step_closure_is_computable_at_dispatch"],
    statement := "Next-step closure is computable at dispatch (the matched datum is concrete); \
      computability does not make cross-channel pruning sound",
    status := .vacuous,
    declarations := [`Rchain.depth2_next_step_disjoint],
    rust := ["rholang/src/reduce.rs"],
    falsifiable := some "`depth2_next_step_disjoint` is the half with content: the closure is \
      computable yet cross-channel pruning by it is still unsound, and that is proved rather than \
      asserted",
    note := "the positive half is a fact about a **signature**, not a theorem, which is why this row is \
      `vacuous` rather than a proof claim: `resolve_children` takes no store \
      (`reduce.rs:resolve_match`, whose doc comment names the purity), so stating it in Lean \
      would prove that \
      a function ignores a parameter nobody passes. The theorem that stood here \
      (`next_step_closure_computable`) was `by rfl` and is **deleted** — and **that is the reason not to \
      close this row**: a future proof of the positive half would be `rfl`-shaped again, re-introducing \
      exactly the vacuity the deletion removed. **And the tempting positive law \
      is false**: resolving `p | q` term-wise does *not* give `resolve p ++ resolve q`, because the Rust \
      flattens all sends before all receives — one send and one receive in each half resolves as \
      `[sp,sq,rp,rq]` merged but `[sp,rp,sq,rq]` concatenated. Found by trying to prove it, which is \
      the argument for proving things" },
  { number := 23, layer := "Scheduler",
    rustWitness := ["rspace/src/property_tests.rs:law23_read_state_determines_outcome"],
    statement := "Read-determinism: an effect's chosen candidate, commit outcome and event trace are a \
      deterministic function of the state it reads",
    status := .provedModel,
    declarations := [`Rchain.read_state_determines_outcome],
    rust := ["rspace/src/space_matcher.rs", "rspace/src/rspace.rs"],
    witness := [`Rchain.certificate_blind_late_writer_diverges],
    falsifiable := some "stated as an implication from two states agreeing on the effect's closure, so \
      it is falsifiable by a state pair that agrees on the closure yet yields different traces — the \
      depth-2 stale read of Law 24 is exactly the shape that would produce one",
    note := "the property test `law23_read_state_determines_outcome` (`rspace/src/property_tests.rs`) \
      names it on the Rust side (commit 35dd13b62)" },
  { number := 24, layer := "Scheduler",
    rustWitness := ["rspace/src/property_tests.rs:law24_record_layer_and_validation",
      "rholang/src/reduce.rs:law24_the_effect_mode_is_what_enables_the_certificate"],
    statement := "DFS-order serializability: a concurrent execution is sound for the block path iff \
      every commit read exactly the state the DFS-earlier effects produced (the versioned \
      write-record layer)",
    status := .provedModel,
    declarations := [`Rchain.prefixVisible, `Rchain.ValidCommit, `Rchain.DFSSerializable,
      `Rchain.s3_pair_fails_validation, `Rchain.serializable_writer_chain,
      `Rchain.pinned_run_publication, `Rchain.certificate_blind_late_writer_diverges],
    rust := ["rspace/src/concurrent/channel_queue.rs", "casper/src/runtime_manager.rs"],
    witness := [`Rchain.certificate_blind_late_writer_diverges, `Rchain.s3_pair_fails_validation, `Rchain.serializable_writer_chain],
    falsifiable := some "the certificate's blind spot is *published* as a theorem \
      (`certificate_blind_late_writer_diverges`): a late-writer run passes the certificate and still \
      diverges, which is why the oracle backstop stays load-bearing",
    note := "the unused `DFSSerializable` hypothesis is **gone**, and with it the \
      `set_option linter.unusedVariables false` that suppressed the warning — the theorem \
      (`pinned_run_publication`, renamed to state what it proves) is dispatched + path-nodup + pinned \
      ⇒ the gate fold, with **no** certificate hypothesis, because a pinned run *is* the \
      serializability that matters. The certificate *finds* such runs; it is not why the fold is reached" },
  { number := 25, layer := "Scheduler",
    rustWitness := ["rholang/src/property_tests.rs:law25_the_validated_relaxed_scheduler_refines_sequential"],
    statement := "Validated speculation: commits may reorder iff each validates Law 24; invalidated \
      runs fall back to the whole-run gate re-run, so the published state is the sequential fold's",
    status := .provedModel,
    declarations := [`Rchain.published, `Rchain.published_state_is_the_oracles,
      `Rchain.fallback_rerun_published, `Rchain.Published],
    rust := ["casper/src/runtime_manager.rs", "rspace/src/concurrent/channel_queue.rs"],
    witness := [`Rchain.published_state_is_the_oracles, `Rchain.fallback_rerun_published],
    falsifiable := some "the rule is the Rust's own (`validate_relaxed_block`, \
      `casper/src/runtime_manager.rs:1095`; its acceptance is `comm_multisets_match` over both logs \
      with `oracle_hash == relaxed_hash`, `:1113-1132`): accept \
      the speculative run only when it agrees with the oracle, and otherwise ship the oracle's result. \
      A publication rule that shipped the speculative state unconditionally would falsify \
      `published_state_is_the_oracles`, which is why the theorem is stated over the *rule* rather than \
      over the fold. `fallback_rerun_published` separately pins that the path-sorted re-run commits \
      each path at most once",
    note := "`validated_speculation_refines_apply` is **gone**, and so is `gate_replay_terminates`: the \
      first was a disjunction whose second arm held for *every* run by the definition of `gateFold` (so \
      it said nothing about the certificate, the fallback, or the code), and the second was \
      `⟨gateRerun ops st, rfl⟩` — totality of a Lean function, which is why `∃ st', f st = st'` is \
      never a law. What replaces them is the port's actual publication rule, which is what makes the \
      sequential-oracle backstop load-bearing rather than decorative" },

  -- ── Cross-shard: two-phase commit (Laws 26–29) ───────────────────────────────────────────────────
  { number := 26, clause := "a", layer := "Cross-shard",
    rustWitness := [
      "node/src/web/http.rs:api_txn_run_rejects_an_invalid_leg_shard_or_an_empty_leg_list",
      "node/src/web/http.rs:a_leg_with_an_empty_to_is_rejected_before_the_gateway_runs",
      "casper/src/property_tests.rs:law26_a_shard_id_is_accepted_exactly_when_nonempty_ascii",
      "casper/src/property_tests.rs:law26_invalid_shard_names_are_refused",
      "casper/src/property_tests.rs:law26_a_child_id_nests_under_its_parent"],
    statement := "A deploy/block's effects bind to exactly one shard, and the leg a gateway admits \
      carries a validated shard id",
    status := .provedModel,
    declarations := [`Rchain.shard_scope_deterministic_is_false, `Rchain.ValidShardId, `Rchain.Leg,
      `Rchain.isAscii, `Rchain.isBlank, `Rchain.validShardId, `Rchain.IncomingLeg, `Rchain.AdmittedLeg,
      `Rchain.admitLeg, `Rchain.validShardId_empty, `Rchain.validShardId_implies_ne,
      `Rchain.the_admitted_leg_carries_a_validated_shard_id, `Rchain.admitLeg_rejects_an_empty_shard_id,
      `Rchain.admitLeg_rejects_a_non_ascii_shard_id, `Rchain.admitLeg_rejects_a_blank_recipient,
      `Rchain.admitLeg_admits_a_negative_amount],
    rust := ["node/src/web/http.rs", "shared/src/refined.rs"],
    witness := [`Rchain.admitLeg_rejects_a_non_ascii_shard_id,
      `Rchain.admitLeg_rejects_a_blank_recipient, `Rchain.shard_scope_deterministic_is_false],
    falsifiable := "`admitLeg_rejects_an_empty_shard_id` / `..._a_non_ascii_shard_id` / \
      `..._a_blank_recipient` are the three refusals the boundary performs, each `decide`d, so dropping \
      a check from `admitLeg` fails them (measured: dropping the ASCII and blank checks makes the build \
      fail on both witnesses *and* on `the_admitted_leg_carries_a_validated_shard_id`, whose three \
      conjuncts are the two `ShardId::try_from` checks plus the recipient one)",
    note := "**the axiom this row used to cite was FALSE** — `∀ l : Leg, ValidShardId l.shard` over a \
      freely constructible record, refuted by `Leg.mk \"\" 0 0` (`shard_scope_deterministic_is_false`, \
      2026-09-23). The axiom is deleted rather than kept beside its own refutation (a false axiom makes \
      everything provable). What the law is about is the *ingress*: the port rejects an invalid shard id \
      at the boundary (`ShardId::try_from` on `TxnLegDto.shard_id`, `node/src/web/http.rs:305-331`, with \
      the boundary test that pins the 400), so the narrowed statement is about the function that admits \
      a leg — which needed that function modelled. **That function is modelled now (2026-09-24, \
      Programme F) and the row is proved**: `admitLeg` mirrors the boundary's decision exactly — \
      `validShardId` (non-empty, ASCII) and a non-blank recipient — and \
      `the_admitted_leg_carries_a_validated_shard_id` states the law's content, with the three refusals \
      as `decide`d witnesses. Two things are named rather than implied. (1) **The amount is not checked \
      at the ingress** — the port's own comment records it is left to the ledger's `NonNegI64` \
      refinement in `GatewayTxn::run`, and `admitLeg_admits_a_negative_amount` is a *theorem* so a model \
      that added the check would fail it. (2) **The recipient check is modelled over code points**: \
      `String.trim` does not reduce in Lean, so `isBlank` (all characters `Char.isWhitespace`) is the \
      computable counterpart of `l.to.trim().is_empty()`, which is what keeps the witnesses `decide`d" },
  { number := 26, clause := "b", layer := "Cross-shard",
    statement := "The shard id is a validated, ordered value — the order is the **derived** one (the \
      port's `#[derive(…, Ord …)]` over the inner `String`), and naming a child under a valid shard \
      yields a valid one whose path **extends** its parent's",
    status := .provedModel,
    declarations := [`Rchain.shardChild, `Rchain.validShardId_child, `Rchain.shardChild_prefix,
      `Rchain.a_child_sorts_after_its_parent, `Rchain.a_grandchild_sorts_after_its_grandparent,
      `Rchain.the_child_of_an_invalid_parent_can_be_valid],
    axioms := [],
    rust := ["shared/src/refined.rs", "casper/src/gateway/mod.rs", "node/src/runtime/node_runtime.rs"],
    witness := [`Rchain.validShardId_child, `Rchain.shardChild_prefix,
      `Rchain.a_child_sorts_after_its_parent, `Rchain.the_child_of_an_invalid_parent_can_be_valid],
    falsifiable := some "**a mutation for each half.** The order half: `shardChild_prefix` fails for a `child` that \
      returned its parent, and the two `decide`d instances fail for one that prepended instead of \
      appending — while the *general* monotonicity is the named boundary (a library gap, not a port \
      claim), and the guard it would need (`n ≠ \"\"` at the root, where `child` appends only the name) \
      is a property of the order rather than of the prefix fact. The validity half: `validShardId_child` fails if the characterisation \
      drops its `isAscii n` term, and `the_child_of_an_invalid_parent_can_be_valid` is the `decide`d \
      witness that the row's original identity is **false** — `shardChild \"\" \"x\"` is `/x`, valid, \
      while `\"\"` is not",
    note := "**the order is derived, so the model needs no instance of its own** (2026-09-24): \
      `shared/src/refined.rs:387` is \
      `#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)] pub struct ShardId(String)` — the \
      lexicographic order on the id string, which `child`'s prefixing makes *structurally meaningful* (a \
      shard sorts before all of its descendants, and `is_descendant_of` is the separate structural \
      predicate that agrees with it). Where the order is observable is the two `BTreeMap<ShardId, _>` \
      sites — `shards` at `casper/src/gateway/mod.rs:65` and at \
      `node/src/runtime/node_runtime.rs:958` — deterministic \
      iteration, exactly where a silently different order would bite — and the model's `ShardId` *is* a \
      `String`, so `decide` and `omega` see the same order the port derives. **And the witness this row \
      carried needed correcting, which is the part worth keeping**: `validShardId (s.child n) = \
      validShardId s` is **false in both directions** — the *model's* `shardChild` joins the path with \
      no check, so a non-ASCII name yields an id `TryFrom` would have refused; and an invalid parent can \
      have a valid child (`shardChild \"\" \"x\" = \"/x\"`), which holds of the port too. **The port's \
      `child` did the same when this row was written** — it constructed the newtype **directly** \
      (`refined.rs:398-425`), bypassing `TryFrom`, which is AUDIT C78 — **and no longer does**: it \
      refuses an empty or non-ASCII name and returns `Result` (`shared/src/refined.rs:411-425`, whose \
      own doc names C78). So the port-side reading of that identity is false for a different reason now, \
      the constructor *refusing* where the model joins. What holds instead is the characterisation in \
      `validShardId_child`: given a **valid** parent, the child is valid exactly when the name is ASCII. \
      AUDIT C78 was a finding about the port's own refinement discipline rather than a deviation from \
      the oracle, and it is **closed**; this row states the corrected form rather than the convenient \
      one, with the refuted version kept beside it so the next reader does not re-derive it. **And the \
      order half's boundary is named with its cost**: \
      what the `BTreeMap` sites observe is `shardChild_prefix` (the child's path carries the parent's) \
      plus the two `decide`d instances; the general \"a child sorts after its parent\" is *statable*, \
      follows from this plus the lexicographic order's prefix property, and is **not a theorem here** \
      because this tree compares no `String`s at all (`Cmp.lean` orders by `GString` with no \
      string-order lemma) — proving it would be the tree's **first** string-order lemma, a modelling \
      decision rather than a proof step. That is a library gap and not a claim about the port, which is \
      why the row is `provedModel` on what is proved rather than `owed` on what the toolchain lacks" },
  { number := 26, clause := "c", layer := "Cross-shard",
    statement := "The RNG seed and unforgeable names are shard-scoped",
    status := .retired,
    rust := ["rholang/src/reduce.rs", "casper/src/tools.rs"],
    falsifiable := some "the law could have been true in exactly one way, and the reading below shows \
      the port does not take it: put a shard in the seed derivation — the unforgeable draw \
      (`rholang/src/reduce.rs`'s `alloc`), `unforgeable_name_rng` and the deploy's \
      `Blake2b512Random::from_init` (`casper/src/tools.rs`) — and the same deploy bytes in a second \
      shard would draw *different* names. Nothing in either tree does, so the claim has no instance to \
      be false of, which is why the row is retired rather than open",
    note := "**retired 2026-09-25, on the reading below — the port has no rule of this shape** \
      (`retired`'s words), and the evidence is the measurement that was already here: \
      **determined by reading the port, not by judgement** (2026-09-24), and the answer is that \
      the node scopes **neither** to a shard — so this is a **design claim** (law 48's shape), not a \
      port gap. The unforgeable names *are* the deploy's RNG stream: the `GPrivate` constructor over a draw: \
      `GUnforgeable::GPrivate(GPrivate { id: bytes })` with `bytes = rand.next()` (`rholang/src/reduce.rs:2198`, \
      in `alloc`); and \
      every seed site is shard-free — `unforgeable_name_rng` over the deployer and the timestamp \
      (`casper/src/tools.rs:11`), `Blake2b512Random::from_init(&deploy.to_bytes())` (`:26`), \
      `rng` over the signature (`:31`) — with no `shard` in any rand or seed site anywhere in `casper/src/`. \
      **The oracle matches**, which settles the disposition: `ProtoUtil.scala:36,49` carries `shardId` \
      only as the *deploy record's own field*, and nothing under `legacy/casper/…/runtime/` mentions a \
      shard at all — so the shard id is a deploy-level field, checked at the ingress (26a's `admitLeg`) \
      and binding the effect to one shard, not an input to the derivation. **Its uniqueness across \
      shards is a consequence of a deploy executing once, not of the derivation: the same deploy bytes \
      in a second shard would draw the same names.** Closing the row would need the shard in the seed \
      derivation *plus* a two-shard non-collision theorem — a design decision shared with the Scala" },
  { number := 27, layer := "Cross-shard",
    rustWitness := [
      "casper/src/property_tests.rs:law27_a_legless_record_cannot_commit",
      "casper/src/property_tests.rs:law27_an_abort_vote_prevents_a_later_commit",
      "casper/src/property_tests.rs:law27_and_law29_the_state_agrees_with_the_votes",
      "casper/tests/cross_shard_txn.rs:two_shard_2pc_commits_all",
      "casper/tests/cross_shard_txn.rs:two_shard_2pc_aborts_all_when_a_leg_fails"],
    statement := "Cross-shard atomicity (2PC): every leg that *prepared* reaches the one decision the \
      coordinator made — commit on all of them or abort on all of them",
    status := .provedModel,
    declarations := [`Rchain.txn_atomic_is_false, `Rchain.uniform, `Rchain.ShardOutcome,
      `Rchain.voteFromReply, `Rchain.decisionOf, `Rchain.phaseTwoWith, `Rchain.phaseTwoWith_cons,
      `Rchain.runPhaseTwo, `Rchain.phaseTwoWith_all_prepared, `Rchain.every_leg_reaches_the_one_decision,
      `Rchain.all_prepared_legs_commit, `Rchain.an_already_committed_leg_still_commits,
      `Rchain.an_aborted_reply_moves_the_decision],
    rust := ["casper/src/txn_coordinator.rs"],
    witness := [`Rchain.txn_atomic_is_false, `Rchain.an_already_committed_leg_still_commits, `Rchain.an_aborted_reply_moves_the_decision, `Rchain.every_leg_reaches_the_one_decision, `Rchain.all_prepared_legs_commit],
    falsifiable := some "`every_leg_reaches_the_one_decision` says every leg's phase-two outcome is the \
      *single* decision or `\"not prepared\"` — so a leg that voted abort cannot carry an outcome that \
      disagrees with a prepared leg's, and `all_prepared_legs_commit` is the all-ready case. The retry \
      path is pinned by `an_already_committed_leg_still_commits` (a `committed` reply plus a `ready` one \
      still commit) against `an_aborted_reply_moves_the_decision` (an `aborted` reply, or an error, is an \
      abort vote) — the two together are what `vote_from_reply`'s own comment argues for, and reading \
      `committed` as not-ready would flip the first into an abort and leave one shard committed and \
      another aborted",
    note := "**the axiom this row used to cite was FALSE** — `∀ r : Run, uniform r` over `Run := List \
      Outcome`, refuted by `[committed, aborted]` (`txn_atomic_is_false`, 2026-09-23). The narrowed \
      statement is the port's own: `run_2pc` decides once (`all_ready`), and phase two applies that \
      decision to every leg that voted ready — a leg that did not prepare never locked, so it has no \
      outcome to be uniform about (`casper/src/txn_coordinator.rs:152-192`). The Rust's comment on \
      `vote_from_reply` (`:209-217`) is the differential reference for the retry path: a re-run \
      answering `committed` must count as ready, or the other legs abort and the run stops being \
      uniform on exactly the path recovery makes reachable. **The run is modelled now** (2026-09-23, \
      Programme D unit 6): `runPhaseTwo` is the port's phase two, `decisionOf` its two decision lines \
      over `voteFromReply`'s booleans, and the three theorems above are the narrowed statement — so the \
      row is a model claim rather than a claim about the Rust evidenced by tests" },
  { number := 28, layer := "Cross-shard",
    rustWitness := ["rholang/src/native_state.rs:law28_txn_prepare_rejects_overdraw_and_is_idempotent"],
    statement := "Leg idempotency: `txn_prepare`/`txn_commit`/`txn_abort` are idempotent under the \
      transaction id — a retried leg returns the record it already has — and the two terminal verbs \
      refuse each other",
    status := .provedModel,
    declarations := [`Rchain.applyEffect, `Rchain.leg_idempotent, `Rchain.TxnRecord, `Rchain.Ledger,
      `Rchain.txnPrepare, `Rchain.txnCommit, `Rchain.txnAbort, `Rchain.txnPrepare_idempotent,
      `Rchain.txnCommit_idempotent, `Rchain.txnAbort_idempotent, `Rchain.commit_after_abort_is_an_error,
      `Rchain.abort_after_commit_is_an_error, `Rchain.prepare_refuses_overdraft],
    axioms := [],
    rust := ["rholang/src/native_state.rs"],
    witness := [`Rchain.commit_after_abort_is_an_error, `Rchain.abort_after_commit_is_an_error, `Rchain.prepare_refuses_overdraft, `Rchain.txnPrepare_idempotent],
    falsifiable := some "the negatives are the witnesses: a second `prepare` that re-escrowed would fail \
      `txnPrepare_idempotent`, and the port's own test pins the balance after a repeated \
      `txn_prepare`/`txn_commit` (`native_state.rs:txn_prepare`, `txn_prepare` and \
      `txn_commit` at `:1454`; the test is \
      `law28_txn_prepare_rejects_overdraw_and_is_idempotent`, `native_state.rs` (the symbol form: this test has moved twice now)); dropping the \
      early return \
      would let a retry fail on insufficient balance *after* the first call had already succeeded, which \
      the port's ordering (`native_state.rs:txn_prepare`, before the balance check) forbids. \
      `commit_after_abort_is_an_error` \
      fails for a verb that allowed the transition — the port returns \
      `Err(\"txn commit: already aborted\")` (`native_state.rs:txn_commit`), and `abort_after_commit_is_an_error` the mirror \
      (`native_state.rs:txn_prepare`). `prepare_refuses_overdraft` is the refusal with the port's own message (`native_state.rs:txn_prepare`)",
    note := "`leg_idempotent` is **proved now** — `funext` on a pointwise update, which is all it ever \
      needed — but it is the per-*shard-state* view, and the port's verbs are not pointwise updates: \
      they read a record, decide, and write a vault balance *and* a record. So the law is re-modelled on \
      the ledger the port keeps, where idempotence is the **early return on an existing record** \
      (`native_state.rs:txn_prepare`) rather than a coincidence of the arithmetic, and where the **fences** \
      are stated too — commit after abort is an error and abort after commit is an error \
      (`native_state.rs:txn_commit`, `native_state.rs:txn_abort`), which idempotence alone would permit. A fidelity note: the \
      model's `TxnState` carried a \
      fourth constructor (`proposed`) that the code does not have (`native_state.rs:vault_key`); a transaction with no \
      record is `none`, which is how the verbs spell it" },
  { number := 29, layer := "Cross-shard",
    rustWitness := [
      "casper/src/property_tests.rs:law29_a_terminal_record_never_changes_again",
      "casper/src/property_tests.rs:law27_an_abort_vote_prevents_a_later_commit",
      "casper/src/property_tests.rs:law27_a_legless_record_cannot_commit",
      "casper/src/property_tests.rs:law27_and_law29_the_state_agrees_with_the_votes"],
    statement := "The coordinator's decision is a *function* of its votes — `committed` iff every \
      participant voted ready — and it is a durable record a prepared participant can recover",
    status := .provedModel,
    declarations := [`Rchain.commit_record_deterministic_is_false, `Rchain.allReady,
      `Rchain.coordinatorDecision, `Rchain.allReady_eq_true,
      `Rchain.coordinator_decision_committed_iff, `Rchain.TxnState.IsTerminal,
      `Rchain.CoordRecord.recordVote, `Rchain.an_abort_is_absorbing, `Rchain.a_commit_is_absorbing,
      `Rchain.an_abort_vote_aborts_a_prepared_record, `Rchain.a_committed_record_stays_committed,
      `Rchain.Ledger.record_setRecord, `Rchain.txnCommit_fixes],
    rust := ["casper/src/txn_coordinator.rs"],
    witness := [`Rchain.commit_record_deterministic_is_false, `Rchain.coordinator_decision_committed_iff, `Rchain.an_abort_is_absorbing, `Rchain.a_commit_is_absorbing, `Rchain.a_committed_record_stays_committed, `Rchain.an_abort_vote_aborts_a_prepared_record],
    falsifiable := some "`coordinator_decision_committed_iff` is the decision half, proved against the \
      port's own two lines; the witness that the *old* statement was false is \
      `commit_record_deterministic_is_false` — a record whose state is `committed` while a vote is \
      `abort` — which contains no false claim and could not, since the record is a free type",
    note := "**the axiom this row used to cite was FALSE** — a property of *every* `CoordRecord` (`:77` \
      is freely constructible), refuted by `{state := committed, votes := [\"\", abort]}` \
      (`commit_record_deterministic_is_false`). Split in two: the **decision half is now proved** — \
      `allReady`/`coordinatorDecision` are the Rust's `all_ready`/`decision` lines, and \
      `coordinator_decision_committed_iff` says `committed` iff every vote is ready — while the \
      **durability half is now modelled too** (2026-09-23, Programme D unit 6): `CoordRecord.recordVote` \
      is the port's `record_vote` (`casper/src/gateway/ledger.rs:158-178`), whose first line — a terminal record \
      ignores later votes — is the fix for AUDIT C160, and the four theorems are the port's own \
      `an_abort_is_absorbing` / `a_commit_is_absorbing` plus the contrast that shows the guard is a \
      choice rather than a fact about votes (`an_abort_vote_aborts_a_prepared_record`). On the \
      participant's side the durability is `Ledger.record_setRecord` (a written record reads back) with \
      `txnCommit_fixes`; the row is `owed` no longer" },

  -- ── Laws 30–43: the surface the ten silent defects live in ──────────────────────────────────────
]

def lawsB : List Law := [
  { number := 30, layer := "Rholang",
    statement := "Every term the parser accepts is in the BNFC grammar (`rholang_mercury.cf`) — \
      **modulo the deviations law 31 names**, and checked as a `decide`d corpus with the soundness \
      direction separated: the derivable cases must be derivable, and the `refused` cases are an \
      accepted term outside the grammar",
    status := .provedTied,
    declarations := [`Rchain.grammarFragment, `Rchain.derives, `Rchain.parseDeviations,
      `Rchain.parseCases_decide, `Rchain.deviations_decide],
    corpus := some "parse",
    rust := ["rholang/src/parser.rs", "rholang/tests/lean_parse_corpus.rs"],
    witness := [`Rchain.parseCases_decide, `Rchain.deviations_decide],
    falsifiable := some "the corpus's **refused** half is the soundness direction made falsifiable: \
      `[1 2]`, `(1 2)`, `[1, 2 3]`, `{a : 1 b : 2}`, `Set(1 2)`, `(1, 2 3)`, `a.b(1 2)`, `c!(1 2)`, \
      `x |`, `a.b` and `new x in` are spelled with no separator or with trailing input, so the grammar \
      refuses them (AUDIT C30) and a parser that grew more permissive fails one — the Rust consumer \
      parses each spelling and must get the refusal. The derivable half is `parseCases_decide`; its \
      `contract` bind/param case is `false` *and* derivable, which is the deviation list showing beside \
      the derive rather than after it",
    note := "**the model exists now, and the corpus is not a tautology** (2026-09-24). `grammarFragment` \
      is the grammar as data — **48 rows, one per production of the `.cf`** (it was 13 when this row \
      landed; U9 grew it to every production, which is why law 31's coverage claim is now 81 of 81 \
      witnesses and `spec/conformance/parse.tsv` carries 123 cases — the two numbers count different \
      things: a *witness* is a spelling per production, a *case* is a corpus row) with `derives` a \
      computable predicate over it, and \
      `parseCases` is the corpus — the derivable cases, the refused cases, the deviation rows and the \
      printer's witnesses — with `parseCases_decide` a `decide`d theorem over all of it. **What its \
      first Rust run found is the reason the layer earns its name**: three rows disagreed with the \
      node, which is the direction a `decide`d corpus cannot catch, because a corpus that only agrees \
      with itself proves only that (`rholang/tests/lean_parse_corpus.rs` is the other half). **And the \
      openness this row carried resolves differently than it was written**: the parser's permissiveness \
      is the **deviation list**, not an unrecorded residual — the comma-less remainder `[1 ..._]` \
      needs no deviation at all (`[X] ::= X | X \",\" [X]` puts a separator only *between* elements, \
      and `ProcRemainder` follows the list carrying no terminal), which corrected AUDIT C24's reading \
      (the comma form is law 31's deviation, recorded below) — and the gate's reserved consumer is now \
      a real one (`tools/check-lean-conformance.sh:lean_parse_corpus`)" },
  { number := 31, layer := "Rholang",
    statement := "Every production of the grammar (`rholang_mercury.cf`) is accepted, modulo a data \
      list of documented deviations",
    status := .provedTied,
    declarations := [`Rchain.grammarFragment, `Rchain.derives, `Rchain.parseDeviations,
      `Rchain.parseCases_decide, `Rchain.deviations_decide],
    corpus := some "parse",
    rust := ["rholang/src/parser.rs", "rholang/tests/lean_parse_corpus.rs"],
    witness := [`Rchain.parseCases_decide, `Rchain.deviations_decide],
    falsifiable := some "**the coverage is the claim, and it is 81 of 81.** `Surface.lean`'s production \
      table witnesses every label of the `.cf`, `Rchain/Print.lean` spells each witness, and \
      `rholang/tests/lean_parse_corpus.rs` runs the node's parser on all 81 spellings — so a production \
      the port cannot read fails the consumer rather than a reviewer. The deviation list is the \
      qualification, and each row of it is a claim: `deviations_decide` refuses a row whose direction \
      the grammar contradicts. Its two `refuses` rows were found by the consumer's first run, not by \
      reading",
    note := "Law 31's deviation list is **data in `Rchain/Parse.lean` (`parseDeviations`)**, each row's \
      direction `decide`d against `derives`: the `accepts` rows are AUDIT C31's trailing-separator and \
      comma-before-remainder sites, and the `refuses` rows — a `NameRemainder` in a contract's \
      parameter list and `ReceiveSendSource`'s `Name \"?!\"` — were found by the layer's Rust \
      consumer, not by reading. A process-position connective is **not** a row: the parser accepts it \
      and the refusal is the normalizer's (`normalizer.rs:1762`, \
      `TopLevelLogicalConnectivesNotAllowedError`), which belongs to law 34/35's layer. **The fragment \
      is 48 rows and covers every production** (U9, 2026-09-24); what it does *not* model is named in \
      `parseBoundaries` — an element's interior (the atom row is the element slot itself, so a \
      single-element list derives by construction), the group `PExprs ::= \"(\" Proc4 \")\"`, comments, \
      the literal forms, and the `select`/`match` lists as *nested* sites" },
  { number := 32, layer := "Rholang",
    statement := "Lexical determinism: comments, the `_`/`_ident` rule, `bundle0`, number forms and \
      the operator spellings each lex one way",
    status := .provedTied,
    declarations := [`Rchain.lexemes, `Rchain.lexemes_decide, `Rchain.longestMatchIn],
    corpus := some "lex",
    rust := ["rholang/src/parser.rs", "node/tests/lean_lex_corpus.rs"],
    witness := [`Rchain.lexemes_decide],
    falsifiable := some "the `decide`d `lexemes_decide` fails if two spellings collide or if a row's \
      spelling is not its own longest match — a table where `<` shadowed `<=` breaks the `<=` row; the \
      Rust consumer (`node/tests/lean_lex_corpus.rs`) runs each sample through the real lexer, so a \
      source that drifted from the model fails instead of being trusted",
    note := "checked for the *operator* surface only; comments, `_`/`_ident`, `bundle0` and the literal \
      forms are named as the boundary and belong to laws 30/31/33" },
  { number := 33, layer := "Rholang",
    statement := "`parse (print p) ≡ p` on `Par` (the C13 round-trip) — **what is checked is two \
      halves**: the model's, that the printer's output is a grammar term (`derives` on `printToks`), \
      and the identity itself, which runs on the node in the Rust consumer, **modulo the three named \
      warts**",
    status := .provedTied,
    declarations := [`Rchain.printToks, `Rchain.printSurf, `Rchain.renderTokens, `Rchain.printWarts],
    corpus := some "parse",
    rust := ["rholang/src/pretty_printer.rs", "rholang/tests/lean_parse_corpus.rs"],
    witness := [`Rchain.warts_are_not_derivable],
    falsifiable := some "the consumer's round trip asserts each of the port's three printer warts with \
      its **own detector**, and each detector requires the loss it names: the urn one requires the term \
      to *have* a urn before it clears `New.uri`/`injections` (so it cannot fire on another wart's row), \
      the tuple one requires the reprint to be the element the tuple wrapped, and the `not` one requires \
      the reprint to be **unreadable** — the port's `~(x)` does not parse at all (`parser.rs`'s group \
      fallback answers \"a tuple needs a comma\", which C13's own test asserts of the printed form). A \
      wart whose detector does not fire fails, and a wart section exercises each detector on a term of \
      its own. The model's half is the completeness direction: each production's witness is printed and \
      the printed tokens must be derivable (`derives (printToks w.term)`, which is what filters \
      `printerCases`), so a printer that emitted an underivable spelling drops out of the corpus",
    note := "**the printer's half is a model claim now** (2026-09-24): `printToks`/`printSurf` mirror \
      `pretty_printer.rs` and `renderTokens` is the surface, so `derives (printToks t)` is a statement \
      about a *definition* rather than about the port's behaviour. The two other tables are data for \
      the same reason the rest of this tree keeps its gaps in tables: `printWarts` (the three faithful \
      warts above, the first two inherited from `PrettyPrinter.scala` and the third the layer's \
      consumer finding) and the layout / bundle-padding / core-vs-surface rows, each a spelling the \
      port emits by design. The C13 regression this row was opened for is the consumer: it prints each \
      witness through the node's own printer and parses the result back, which is the half the model \
      cannot state about the port. **And what U9 added** (2026-09-24): the identity half runs on the \
      node rather than here because the port's printer renders the core `Par` and this one the surface \
      (`Print.lean`'s `printBoundaries` names the difference), while the model's half is \
      completeness-shaped — the printer's output *is* a grammar term (`derives` on `printToks`, the 81 \
      `printer` rows). The three warts are the **port's**, not this printer's: this one emits `(1,)`, \
      `not x` and the urn, since a corpus row has to be a grammar term before it can witness anything. \
      `warts_are_not_derivable` kept its **name** across U9 so this row's citation never broke; its \
      content is now \"the port's two spelling warts are outside this fragment\", with the docstring \
      saying why the port's spelling is a grammar term all right but a *different* term" },
  { number := 34, layer := "Rholang",
    statement := "A *value* position (a condition, target, datum, element, pattern, name) is normalized \
      against an **empty** `par`; only a statement continuation inherits what precedes it. The \
      accumulator that makes the distinction is **modelled** — `normalizeAt` threads the port's \
      `ProcVisitInputs.par`, and only the sequencing arm (`.par`) hands it on — and **both halves are \
      checked on the shapes C21 broke**: the value-position half by the corpus (a condition seeded with \
      the ambient fails `c21Holds`) and the sequencing half by `c21CarriesThePreceding`",
    status := .provedTied,
    declarations := [`Rchain.normalizeAt, `Rchain.Surf],
    corpus := some "c21",
    rust := ["rholang/src/normalizer.rs", "rholang/tests/lean_c21_corpus.rs"],
    falsifiable := some "**the layer is the witness, and the model's half is now falsifiable too.** In \
      the port, reintroducing C21's defect in `normalize_if` — normalizing the condition against \
      `input.par` rather than `Par::default()` — fails case 1 at once, with the target reported as \
      `@\"c\"!([\"a\"])` beside `(1 == 1)` instead of the condition alone (verified before the corpus was \
      believed). **In the model it is one word**: the `ifThen` arm's condition call, `nilPar` → `acc`, \
      and `c21Cases_decide` fails to reduce. The sequencing half has its own falsifier, and it is the \
      only one that catches it: `.par` handing `nilPar` to its right side leaves `c21Cases_decide` green \
      — the target is untouched — and fails `c21Cases_carry_the_preceding`. The corpus has a second \
      failure mode of its own: its non-degeneracy half (`c21IsProbe`, decided with the rest) fails for a \
      case whose term normalizes to its own condition, so a case that proved nothing would fail rather \
      than pass quietly",
    note := "**the corpus is the check here, and the model's half is a check rather than a proof of the \
      rule** — which is the honest shape, not a weakness to hide. What the two sides do is normalize the \
      same source text independently: the model `decide`s (through `cmpPar`, whose `eq_iff` is proved) \
      that the desugared `Match`'s target is the condition alone, and `lean_c21_corpus.rs` asserts the \
      node's target equals its own normalization of the condition — so what breaks under a regression is \
      the **tie**, and it does. Case 3 is the explicit-`match` control (the desugaring that was never \
      broken) and case 5 a ground condition, so the layer does not rest on the arithmetic clauses \
      agreeing. AUDIT C21 is the history: the target became the preceding par, the pattern cases are \
      `true`/`false`, an unmatched `match` is not an error, and the `if` reduced to nothing at \
      `processedWithSuccess` — invisible for an `if` in first position, which is the idiom contracts \
      mostly use. **The accumulator this row was owed is modelled (2026-09-24, G6), and the sentence it \
      retires is the one above** — `normalizeAt` threaded only the binder stack, so the parameter whose \
      misuse was C21 did not exist and the rule held by construction. **The way it had to be done is \
      the part worth keeping**: the first attempt threaded the ambient only at the *entry* — a wrapper \
      whose `.par` chained and whose every other construct descended into an ambient-free core. It \
      compiled, and it was **correct and wrong**: it made C21's defect *unrepresentable* again, which \
      is this row's own complaint one level up — a rule that holds by construction is a rule nothing \
      can falsify. What works is the port's own shape: one function whose arms make the choice, each \
      call site spelling `nilPar` (a value position) or the inherited par (only `.par`'s sequencing), \
      mirroring `normalize_proc`'s `ProcVisitInputs.par` and `PPar(l, r)`'s `par: result.par` \
      (`rholang/src/normalizer.rs:188-199`), with each construct's own result merged into the \
      accumulator (`prepend_expr`'s shape). **What is still not proved is the universal form**: both \
      halves are checked on the shapes C21 broke, instance by instance, rather than by an induction over \
      `Surf` — a proof-shape question rather than a modelling one, named here rather than implied" },
  { number := 35, layer := "Rholang",
    statement := "`connective_used` is sound: it holds iff the term contains a connective, free \
      variable, wildcard or remainder, per collection form",
    status := .provedTied,
    declarations := [`Rchain.connectiveUsed, `Rchain.Var.isConnective, `Rchain.cmpOptionVar],
    corpus := some "flags",
    rust := ["rholang/src/normalizer.rs", "rholang/tests/lean_normalize_corpus.rs"],
    falsifiable := some "`flagCases_decide` (`Rchain/Corpus.lean`, 17 cases) is `decide`d, so a model \
      whose flag disagreed with its own case would not compile; \
      `rholang/tests/lean_normalize_corpus.rs` then reads each pattern's verdict out of the real node. \
      This layer found AUDIT C24",
    note := "the layer whose first line — `connective_used` short-circuiting to structural equality — \
      is where C19/C20/C22 item 3 all lived" },
  { number := 36, layer := "Rholang",
    statement := "The normalizer's output is well-scoped and closed (Law 6 through every path)",
    status := .provedModel,
    rust := ["rholang/src/normalizer.rs"],
    rustWitness := ["rholang/src/normalizer.rs:law36_normalization_preserves_closedness"],
    declarations := [`Rchain.ScopedIn, `Rchain.an_unscoped_name_occurrence_is_open,
      `Rchain.a_scoped_name_occurrence_is_closed, `Rchain.bindResult, `Rchain.closed_normalizeAt,
      `Rchain.closed_procsPar, `Rchain.closed_namesPar, `Rchain.closed_namePar,
      `Rchain.closed_collectPar, `Rchain.closed_kvsPar, `Rchain.closed_casesPar,
      `Rchain.closed_receiptsPar, `Rchain.closed_receiptPar, `Rchain.closed_bindsPar,
      `Rchain.closed_bindResult, `Rchain.closed_groundPar, `Rchain.closed_iff_Closed,
      `Rchain.closed_parMerge, `Rchain.closed_sendPar, `Rchain.closed_receivePar,
      `Rchain.closed_newPar, `Rchain.closed_matchPar, `Rchain.closed_bundlePar,
      `Rchain.closed_parOf, `Rchain.closed_parOf_elist, `Rchain.closed_parOf_eset,
      `Rchain.closed_parOf_emap, `Rchain.closed_parOf_etuple, `Rchain.closedVar_nameVar,
      `Rchain.findIdx?_some_of_mem],
    witness := [`Rchain.an_unscoped_name_occurrence_is_open],
    falsifiable := some "the statement is **false** of an un-scoped source, and that is the shape of its \
      hypothesis rather than a gap: `nameVar` answers `.free 0` for a name the binder stack does not \
      hold, and `Ty.Closed` counts a free variable as open — so `for (x <- c) { x!(1) }` alone desugars \
      to `closed = false` (measured, `#eval`) while the same term under `new c, x in { … }` desugars to \
      `closed = true`. Both directions of the mechanism are theorems \
      (`an_unscoped_name_occurrence_is_open`, `a_scoped_name_occurrence_is_closed`), so a reader can \
      falsify the claim's shape without trusting this sentence. The induction is falsifiable the same \
      way: `closed_parMerge` is what carries the accumulator, so a `parMerge` that dropped a field would \
      break every arm that builds one",
    note := "**proved 2026-09-25 — the induction landed, and what it says is exactly the statement**: \
      `closed_normalizeAt` is `∀ e acc Γ, Closed acc → ScopedIn Γ e → ∀ p, normalizeAt e acc Γ = some p → \
      closed p = true`, with ten companion theorems for the functions its arms call (`procsPar`, \
      `namesPar`, `namePar`, `collectPar`, `kvsPar`, `casesPar`, `receiptsPar`, `receiptPar`, \
      `bindsPar`, `bindResult`). **Every path, not a fragment**: the arms are the 45 constructors of \
      `Surf`, the 12 outside the domain return `none` and close by contradiction, and the 33 that \
      desugar reduce to `Closed_parMerge` plus the closedness of the `Par` they build — which is what the \
      leaf lemmas are, one per shape. **The two proof-engineering obstacles this row's earlier notes \
      named are both recorded where a next reader will meet them**: `bindsPar` had no equation lemma \
      until its nested `match` was lifted into `bindResult` (`failed to generate equational theorem`, \
      reproduced), and the list-valued arms had to be written as **equations** rather than by \
      `induction`/`cases` — with the latter the measure generalizes and the obligation becomes false \
      (`sizeOf pat < sizeOf cs` instead of `… < sizeOf (⟨pat, body⟩ :: cs)`), which is what the first \
      draft's termination failure was. **And the shape of the statement is measured, not chosen**: the \
      hypothesis is `ScopedIn Γ e` because the un-scoped reading is *false* \
      (`an_unscoped_name_occurrence_is_open`), and `closedVar_nameVar` is the step that makes a scoped \
      name a bound — hence closed — variable. **The tie is prose, and the row says so**: what is pinned \
      is the model; the node's normalizer is anchored by `rust` (`rholang/src/normalizer.rs`) and no \
      corpus replays *this* property, so the row is `provedModel` rather than `provedTied`" },

  { number := 37, layer := "Rholang",
    rustWitness := ["rholang/src/property_tests.rs:law5_a_ground_pattern_matches_only_itself"],
    statement := "Match soundness and completeness (Law 5 strengthened: partial collections, \
      wildcards, remainders) — **over the shapes the clauses cover, which `pathPar` names, the clauses decide exactly equality**: `spatialMatches t p ↔ t = p` (`spatialMatches_iff_eq`)",
    status := .provedTied,
    declarations := [`Rchain.spatialMatches_iff_eq, `Rchain.eq_imp_spatialMatches,
      `Rchain.the_tie_decides_a_set_pattern_and_its_shorter_neighbour, `Rchain.spatialMatchCore, `Rchain.spatialMatchExprs, `Rchain.spatialMatchExpr,
      `Rchain.matchListPar, `Rchain.matchListPos, `Rchain.matchMap, `Rchain.modelledPar,
      `Rchain.modelledExpr, `Rchain.arithmetic_pattern_refutes_the_unrestricted_tie,
      `Rchain.the_walk_past_empty_pars_is_paid_for,
      `Rchain.a_list_pattern_cannot_skip_a_target_element, `Rchain.fuel_saturation,
      `Rchain.a_nested_tuple_is_paid_for, `Rchain.a_two_expression_pattern_refutes_the_modelled_tie,
      `Rchain.a_shorter_set_pattern_is_refused, `Rchain.a_shorter_map_pattern_is_refused,
      `Rchain.a_set_pattern_of_the_same_length_still_matches,
      `Rchain.a_permuted_pattern_is_refused, `Rchain.an_unaligned_variable_pattern_is_refused,
      `Rchain.pathPar, `Rchain.linear_of_pathPar, `Rchain.pathExpr, `Rchain.pathPars, `Rchain.pathPairs,
      `Rchain.a_nested_multi_expression_par_is_outside_the_path_domain,
      `Rchain.a_single_expression_par_is_in_the_path_domain, `Rchain.spatialMatches_imp_eq],
    corpus := some "match",
    rust := ["rholang/src/matcher/spatial_matcher.rs"],
    witness := [`Rchain.arithmetic_pattern_refutes_the_unrestricted_tie, `Rchain.a_list_pattern_cannot_skip_a_target_element, `Rchain.the_walk_past_empty_pars_is_paid_for, `Rchain.a_shorter_set_pattern_is_refused, `Rchain.a_shorter_map_pattern_is_refused, `Rchain.a_permuted_pattern_is_refused, `Rchain.a_nested_multi_expression_par_is_outside_the_path_domain, `Rchain.a_single_expression_par_is_in_the_path_domain, `Rchain.spatialMatches_imp_eq, `Rchain.eq_imp_spatialMatches,
      `Rchain.the_tie_decides_a_set_pattern_and_its_shorter_neighbour],
    falsifiable := some "the corpus's 22 cases with three-valued verdicts; the once-false law-5 axiom was replaced \
      *because* a corpus case contradicted it (AUDIT C26), the fuel bound was one step short until the \
      `decide` refused to compile, and `concrete_matches_iff_eq` **was false as stated** until case 15 \
      — a tuple pattern, which the port matches (`spatial_matcher.rs:538-544`) and the clauses had no \
      arm for — made the `decide` refuse (AUDIT C44). Two further cases are the model's own defects, \
      one in each direction: case 18 (`@Set(1, ..._)` against `Set(Nil × 6, 1)`) is a match the model \
      *under*-claimed because the fuel's measure counted an empty `Par` as zero nodes while the set \
      member walks past it (AUDIT C47), and case 19 (`@[1, ..._]` against `[Nil, 1]`) is a match the \
      model *over*-claimed because the list arm was wired to the searcher (AUDIT C48) — the direction \
      the boundary note says the corpus exists to catch. The tie still carries `modelledPar` on both \
      sides, and `arithmetic_pattern_refutes_the_unrestricted_tie` is the term that says why it must: a \
      concrete arithmetic pattern equals itself and no clause matches it. Its domain is **three** \
      hypotheses by now — `modelledPar` on both sides, a singleton pattern expression list, and canonical \
      collection contents — and the third is AUDIT C54's: a target whose set carries a duplicate element \
      is matched by a shorter pattern in the model and is unconstructible on the node, so the statement \
      needs the invariant `par_set` maintains rather than a clause the model lacks",
    note := "shares the tie axiom with Law 5; its `fuel_saturation` is **discharged** now (2026-09-24) — \
      worth saying here because the tie is only as true as the fuel beneath it, and under the short \
      measure the tie was *false* (a tuple three deep is `modelledPar` and unmatchable when the fuel \
      ran out — AUDIT C50). **The tie's domain was too wide, not merely unproved**: \
      `connectiveUsed pattern = false` admits an arithmetic pattern, which is concrete and unmatchable, \
      so the statement was false of the model — the same class as C26 and C40, found by asking what the \
      statement says on a term the model has. **And it was false a second time, for a second reason \
      (AUDIT C51)**: with `modelledPar` as the only domain predicate the tie still admitted a `Par` with \
      **two** expressions, which the clauses reject while the value equals itself — so the axiom is \
      **deleted** and what the row owes is the tie for a **singleton** pattern, the shapes the clauses \
      cover (`a_two_expression_pattern_refutes_the_modelled_tie` is the ratchet, and the lesson is that \
      a domain predicate has to be checked against *every* value the model admits, not the ones a \
      reachable datum can be). **The clauses are form-specific** and that is load-bearing \
      in both directions: a list or tuple is positional (`fold_match`, the port's `EList`/`ETuple` \
      arms), a set or map searches (`list_match_single` → `find_matches`), and `matchListPos`/\
      `matchListPar` are the two members; `match.tsv` cases 18/19 pin one direction each. \
      **The set/map member's third direction, measured and then fixed** (2026-09-24, row 5's note \
      carries the detail): the walk over-claimed a *shorter canonical* pattern against a *longer \
      canonical* target, where the port refuses an unequal length before searching \
      (`spatial_matcher.rs:684-693`). The clause now has that guard, corpus rows 21/22 hold the node \
      to it, and the opposite residue — a permuted pattern, which the node matches and the walk \
      cannot — is pinned by `a_permuted_pattern_is_refused`. The tie's domain is therefore the \
      **canonical** shapes, where the walk and the port's backtracking assignment agree: not the \
      duplicate-element corner only (C54), but the sorted-and-duplicate-free form on both sides, \
      which is what `sortPar` fixes for the model's collections and `par_set` for the node's.       **And the domain itself was wrong, not just unproved** (2026-09-24, AUDIT C60): `modelledPar` plus       a singleton expression list plus canonical contents still admits a shape the clauses reject —       nest a two-expression `Par` inside a collection and the *inner* list is not a singleton, so       `spatialMatch` answers `false` for the value against itself while it is modelled and       connective-free (`a_nested_multi_expression_par_is_outside_the_path_domain`). The hypothesis has       to hold at *every* level, and `pathPar` is that predicate — a `Par` whose fields but `exprs` are       empty, holding exactly one expression, that expression a ground or a collection of `pathPar`s       with no remainder. It is what \"the shapes the clauses cover\" was always meant to name, and       `linear_of_pathPar` is the free-level consequence the row used to leave implicit. **AUDIT C60       also measures what the domain leaves out, and it is not all unmodelled shapes**: the port's       matcher never reaches these clauses for a *concrete* pattern at all — it short-circuits at       `if !pattern.connective_used { pattern == target }` (`spatial_matcher.rs:235-237`, named as such       in the port's own normalizer at `normalizer.rs:1601`) — and both sides of a real match are       already canonical, because the RSpace payload types are `Sorted<Par>`       (`models/src/runtime.rs:20-36`). Measured: the node **matches** `@Set(1 | 2)` against itself       (`true`) where the clauses here answer `false`. Closing that is a modelling change to       `spatialMatch` — the short-circuit and the canonicalization — rather than a clause, and it is       named as the row's next step instead of being assumed. **The tie is half proved** (2026-09-24): \
      `spatialMatches_imp_eq` is the *soundness* direction — an accepted match forces equality — over
      `pathPar` on both sides. It rests on nothing but the clauses: a six-member `mutual` family in
      `Match.lean` (`eq_of_core`/`eq_of_exprs`/`eq_of_expr`/`eq_of_listPos`/`eq_of_listPar`/`eq_of_map`),
      the thirty `rfl` clause reductions above them, and the length facts. **No fuel bound appears
      anywhere**: a shortfall can only answer `false`, so an answer of `true` at *any* fuel forces the
      shape — which is why this is the cheap half and the other is not. Falsified by mutation rather than
      argued: reversing the positional walk's element comparison breaks the family's `change` on the
      clause *and* breaks `fuel_saturation`. **The completeness direction landed the same day** — a
      `pathPar` pattern matches itself — as a six-member family in the same file, the mirror of
      `fuel_saturation` at the same depth-indexed bounds, so the row is closed by `spatialMatches_iff_eq`.
      Two things it cost, both worth keeping: the members must be stated at **their own fuel**
      (`… fuel …` with `bX ≤ fuel`), not at `fuel + 1`, or every inter-member call is off by one; and the
      bound's arithmetic needs `parNodes_pos` in scope explicitly (`omega` cannot see that a `Par`
      presents at least one node) *and* the goal's `bCore`/`bList` unfolded, since neither is a
      `[simp]`-tagged definition. Falsified by mutation: dropping the set guard's length condition breaks
      that family's `if_pos` step, the thirty reductions, and the guard's own ratchets. One trap, worth
      keeping: the family's members are `private`, so only
      `spatialMatches_imp_eq` is citable from the register — a row naming `eq_of_core` would fail the
      reference check" },
  { number := 38, layer := "Rholang",
    statement := "Silence is specified: an unmatched receive or `match` yields no reduction **and no \
      error**",
    status := .provedTied,
    declarations := [`Rchain.ReduceP, `Rchain.takesStep, `Rchain.receiveParP,
      `Rchain.takesStep_sound, `Rchain.takesStep_complete, `Rchain.takesStep_iff_reduces,
      `Rchain.takesStep_commPs_redex, `Rchain.takesStep_comm_redex, `Rchain.takesStep_parMerge_left,
      `Rchain.takesStep_parMerge_right, `Rchain.stepsInSends_sound, `Rchain.stepsInReceives_sound,
      `Rchain.stepsInBinds_sound, `Rchain.exists_redex_split, `Rchain.stepsInSends_append,
      `Rchain.stepsInSends_prepend, `Rchain.stepsInReceives_append, `Rchain.stepsInReceives_prepend,
      `Rchain.stepsInSends_prepend_recv],
    rust := ["rholang/src/reduce.rs:1994", "rholang/src/reduce.rs:1868"],
    corpus := some "silence",
    falsifiable := some "the corpus's cases each run on their own runtime with a control datum, and six \
      of the thirteen are *negative* — an unmatched map pattern, a receive on another channel, a \
      mismatched literal, and three calls at the wrong arity — so a receive that stepped when it should \
      not fails a case, and so does one that refused a step it should take; the corpus is this row's \
      witness (the row declares no separate `witness` because the falsifier *is* the corpus here, and \
      `silenceCases_decide` is the theorem that checks all thirteen). The statement's own history is the \
      rest of the evidence that it can fail: its first version was **false**, refuted by `chan = nilPar` \
      (AUDIT C40), and case 13 found that the *search* claimed a step for a join the node does not \
      perform (AUDIT C45)",
    note := "**both directions are proved, so `takesStep_iff_reduces` is a theorem and not an axiom** — \
      every step the rule has, the search reports (`takesStep_complete`), and everything the search \
      reports, the rule derives (`takesStep_sound`). The sound direction was not provable as written, and \
      the obstacle was a modelling gap rather than an induction: the rule's constructors built their \
      receive through `receiveParP`/`receiveParPs`, which *fix* `freeCount := patterns.length` and \
      `bindCount := 1`, so the rule could not derive what the search accepted — the port's `free_count` is \
      `count_no_wildcards` (`normalizer.rs:1326-1329`, the `ReceiveBind`'s `free_count`), so an ordinary `for (@a, @b <- c)` has \
      `freeCount = 0` against `patterns.length = 2` and the node contracts it (AUDIT C45). Both \
      constructors now take those two fields as parameters, and take the channel as *two* parameters with \
      a shared-name hypothesis — the node's own condition. That last change is also what let the *domain \
      hypothesis go*: the statement was briefly `(h : allStringChans p = true)`, added when the rule \
      fired on any channel at all, and once the rule carries `stringChan … = some name` itself the \
      hypothesis is a consequence of the rule rather than a side condition on the statement — so the \
      predicate that carried it is deleted, and the tie holds for every `Par`. **What remains is the \
      model's boundary, not a proof: there is no join rule.** A receive with two or more binds is a join, \
      the node fires one when every bound channel holds a matching datum, and this model's rule fires only \
      on a single-bind receive — the search agrees with the model, so a fully matched join is a step in \
      the node and none here (AUDIT C40, unchanged by this proof), which is a modelling gap to close \
      rather than an obligation to discharge" },
  { number := 39, layer := "Protocol",
    statement := "Every `rho:*` urn's reply arity and shape equals its `spec/API-SCHEMA.md` row",
    status := .provedTied,
    declarations := [`Rchain.replyCatalog, `Rchain.replyCatalog_decide],
    corpus := some "protocol",
    rust := ["rholang/src/system_processes.rs", "spec/API-SCHEMA.md"],
    witness := [`Rchain.replyCatalog_decide],
    falsifiable := some "`replyCatalog_decide` requires namespaced unique urns, the reply kind \
      agreeing with its slots, and the declared arity agreeing with the arguments as written; the gate \
      additionally requires a `spec/API-SCHEMA.md` row per catalog urn, so a urn without a documented \
      shape fails",
    note := "checked for the rows the surface language can spell; `ByteArray`-taking urns (crypto, \
      `deployerId:ops`, `authToken:ops`) have no grammar literal and stay pinned by hand-written probes" },
  { number := 40, layer := "Protocol",
    statement := "Every call in the protocol catalog has an accepting receive at the target's arity",
    status := .provedTied,
    declarations := [`Rchain.stepsInBinds, `Rchain.receiveParPs],
    rust := ["rholang/src/system_processes.rs"],
    corpus := some "silence",
    falsifiable := some "case 10 is `write!(key, value)` against a three-argument `write` — a call at \
      the wrong arity has no step *by the rule*, so the corpus's arity-varying cases are the negative \
      cases, and this is AUDIT C22 item 2's check",
    note := "shares the silence layer with Law 38, which the gate documents as deliberate" },
  { number := 41, layer := "Protocol",
    statement := "Channel balance: a replicable reader restores what it consumes",
    status := .provedTied,
    declarations := [`Rchain.replicatedRead, `Rchain.readStore, `Rchain.storeSurvives],
    corpus := some "store",
    rust := ["rspace/src/rspace.rs", "casper/tests/genesis_registry.rs"],
    falsifiable := some "`storeCases_decide` (5 cases) plus \
      `casper/tests/genesis_registry.rs`'s `a_read_does_not_destroy_the_inbox` on a real chain: a read \
      that consumed its datum would fail both, and C25 (`Group`'s `new` reading a dictionary nobody \
      writes) was found by the second",
    note := "the static walk over the vendored text was retired unshipped rather than committed with \
      an exception list (AUDIT C22 item 1)" },
  { number := 42, layer := "JSON",
    statement := "`rho_expr_to_par (expr_from_par p) = p`, and the `0 → absent`, `1 → unwrapped`, \
      `n → ExprPar` envelope rule",
    status := .provedTied,
    declarations := [`Rchain.parToJE, `Rchain.jeToPar, `Rchain.render, `Rchain.flatPar,
      `Rchain.decode_encode, `Rchain.parToJE_getD_round, `Rchain.parToJE_of_decodesTo,
      `Rchain.parsToPar_merged, `Rchain.unforgPair_refutes_the_old_statement],
    corpus := some "json",
    rust := ["node/src/api/rho_expr.rs"],
    witness := [`Rchain.unforgPair_refutes_the_old_statement, `Rchain.decode_encode, `Rchain.parToJE_getD_round],
    falsifiable := some "the **statement was false as written**, and the falsification is in the tree: \
      the domain predicate `flatPar` accepted a par of two unforgeables, which the decoder drops and \
      the encoder then writes as *no value at all* — `unforgPair_refutes_the_old_statement` is that \
      term, `flatPar e = true`, `jeToPar e = some nilPar`, `parToJE nilPar = none`. The predicate now \
      refuses an unforgeable, which is what the module doc already claimed. Behaviourally, \
      `jsonCases_decide` (12 cases) is `decide`d and `node/tests/lean_json_corpus.rs` runs each case \
      through the node's own `expr_from_par` *and* its `rho_expr_to_par` round-trip, so an envelope \
      rule that was wrong for `n = 2` fails on a case",
    note := "**the axiom is gone, and it was not merely owed.** The row used to cite `decode_encode` as \
      an axiom whose own docstring said the merge arithmetic was 'still to be written out'; what the \
      induction showed instead is that the *statement* was false of the model, in exactly the way law \
      5's and law 38's were (C26, C40): a hypothesis wide enough to accept a term the conclusion cannot \
      hold of. The proof needed the merge arithmetic to be *per field* (`parMerge` concatenates exprs, \
      then unforgeables, then bundles — so a merge's item list is not `rawItems p ++ rawItems q`) and \
      the unforgeable arm of the domain to be closed. The unforgeable leaf is still outside the model's \
      domain — its `GUnforgeable` carries a level, not the wire bytes — and stays pinned by \
      `rho_expr.rs`'s unit tests, which is now what `flatPar` says rather than what it wished" },
  { number := 43, layer := "JSON",
    statement := "Each endpoint's serialized shape equals the schema's (camelCase fields, `[]` \
      semantics, error precedence)",
    status := .provedTied,
    declarations := [`Rchain.envelopeCatalog, `Rchain.envelopeCatalog_decide],
    corpus := some "envelope",
    rust := ["node/src/api/grpc/tonic.rs", "node/src/api/dto.rs"],
    witness := [`Rchain.envelopeCatalog_decide],
    falsifiable := some "`envelopeCatalog_decide`: no key contains an underscore (C16's rule), keys \
      distinct, union tags capitalized, names unique; `node/tests/lean_envelope_corpus.rs` holds *both* \
      parties to the catalog — the DTOs' serialization and the served `OPENAPI_JSON` document — so a \
      row that no longer matches either one fails",
    note := "checked for the envelope's keys and tags; the types behind them and the document's \
      coverage are named as the boundary (AUDIT C29 fixed the two rows the check found stale)" },
  -- ── Proof-of-Stake: the epoch, its split, and the withdrawal (Laws 44–47) ───────────────────────
  { number := 44, layer := "PoS",
    rustWitness := [
      "rholang/src/native_state.rs:the_epoch_gate_does_nothing_off_a_boundary",
      "rholang/src/native_state.rs:bond_escrows_the_stake_and_activates_at_the_boundary",
      "rholang/src/property_tests.rs:law44_the_absence_rule_never_raises_a_reward",
      "rholang/src/property_tests.rs:law44_a_validator_inside_the_grace_is_kept",
      "rholang/src/property_tests.rs:law44_a_validator_outside_the_knee_is_not_paid",
      "rholang/src/property_tests.rs:law44_a_validator_that_never_spoke_is_not_paid",
      "rholang/src/property_tests.rs:law44_a_zero_knee_withholds_nothing",
      "rholang/src/property_tests.rs:law44_between_the_grace_and_the_knee_a_validator_is_paid_in_part",
      "rholang/src/native_state.rs:the_boundary_pays_on_the_participation_it_is_handed",
      "casper/src/runtime_manager.rs:a_boundarys_participation_reaches_play_and_replay_alike"],
    statement := "Membership takes effect at an **epoch boundary**: the epoch sequence runs only when \
      `blockNumber % epochLength = 0`, and off a boundary a bond is pooled but not activated, a \
      withdrawal is staged but not moved, and no claim is paid. The same machine carries the **slash**, \
      which is graded by the offence (AUDIT C199): `bps` basis points of everything the validator holds \
      in the PoS system — its bond, its accrued rewards and an escrowed claim — go to the Coop vault and \
      the remainder returns to its own vault, so the loss is bounded by the tier and a milder tier never \
      takes more. And the same vault pays for **work**: a share of a deploy's burned phlo goes to the \
      block's own signed sender (`payExecutor`), which is a transfer inside the staking vault — it moves \
      income, never stake, and it is monotone in what the deploy burned. A boundary may also **scale** a \
      drawn validator's reward by its participation (`absenceAdjusted`, over `participationWeight`), which \
      is income only: the weight is a function of the reward and of how far behind the last finalised \
      fringe the validator's latest *message* sits, so no stake is reachable from it. The weight is the \
      whole `10000` inside a **grace**, ramps linearly to `0` at a **knee**, and is never more than the \
      whole — which is what carries law 46 through the rule (`weighted_rewards_le_pot`: the split is \
      unchanged, and the rule only multiplies its outputs by at most one). **A partial weight withholds** \
      (`absence_withholds`), and that theorem is deliberately paired with the decided instance \
      `the_ramp_is_a_ramp`, because it is an *implication*: a weight that was always `10000` satisfies it \
      **vacuously** — measured, not argued, since that mutation fails the instance and leaves the \
      implication alone. Neither is a check without the other, which is the absence rule's own vacuity \
      one level up. `grace = knee` reproduces a binary threshold rule rather than losing it, and \
      `knee = 0` is the off switch",
    status := .provedModel,
    declarations := [`Rchain.PosState, `Rchain.PosClaim, `Rchain.PosRequest, `Rchain.totalRev,
      `Rchain.divisor, `Rchain.isBoundary, `Rchain.bond, `Rchain.epochStep, `Rchain.closeBlock,
      `Rchain.payDue, `Rchain.payDue_conserves, `Rchain.the_ledger_steps_leave_the_coins,
      `Rchain.epochStep_conserves, `Rchain.closeBlock_off_a_boundary,
      `Rchain.a_bond_pools_but_does_not_activate, `Rchain.a_boundary_activates_the_pool,
      `Rchain.atRisk, `Rchain.malicious, `Rchain.misdemeanour, `Rchain.honestMistake, `Rchain.slash,
      `Rchain.taken_le_risk, `Rchain.slash_conserves, `Rchain.a_milder_tier_takes_no_more,
      `Rchain.slash_clears_every_ledger,
      `Rchain.executorShare, `Rchain.producer_share_le_burned, `Rchain.payExecutor,
      `Rchain.payExecutor_conserves, `Rchain.payExecutor_leaves_the_stake,
      `Rchain.producer_pay_is_monotone,
      `Rchain.absenceAdjusted, `Rchain.absence_never_raises,
      `Rchain.a_returning_validator_is_paid_in_full, `Rchain.the_absence_rule_moves_no_stake,
      `Rchain.absence_withholds,
      `Rchain.participationWeight, `Rchain.participationWeight_le, `Rchain.participationWeight_full,
      `Rchain.participationWeight_zero, `Rchain.participationWeight_antitone,
      `Rchain.nsum_zipWith_le_map, `Rchain.weighted_rewards_le_pot,
      `Rchain.absence_withholds_past_the_knee, `Rchain.the_ramp_is_a_ramp],
    axioms := [],
    rust := ["rholang/src/native_state.rs"],
    witness := [`Rchain.closeBlock_off_a_boundary, `Rchain.epochStep_conserves, `Rchain.the_ledger_steps_leave_the_coins, `Rchain.a_bond_pools_but_does_not_activate, `Rchain.a_boundary_activates_the_pool, `Rchain.slash_conserves, `Rchain.a_milder_tier_takes_no_more, `Rchain.slash_clears_every_ledger, `Rchain.taken_le_risk, `Rchain.payExecutor_conserves, `Rchain.payExecutor_leaves_the_stake, `Rchain.producer_pay_is_monotone, `Rchain.absence_never_raises, `Rchain.a_returning_validator_is_paid_in_full, `Rchain.the_absence_rule_moves_no_stake, `Rchain.absence_withholds, `Rchain.participationWeight_le, `Rchain.participationWeight_full, `Rchain.participationWeight_zero, `Rchain.participationWeight_antitone, `Rchain.weighted_rewards_le_pot, `Rchain.absence_withholds_past_the_knee, `Rchain.the_ramp_is_a_ramp],
    falsifiable := some "`the_epoch_gate_does_nothing_off_a_boundary` builds the off-boundary state — a \
      staged withdrawal *and* a full reward pot — and asserts the whole state is unchanged, then that \
      the same call at the boundary moves it and pays it; the bond half is in the same test (pooled at \
      once, still not active at a non-boundary block), and the two membership changes carry their own: \
      `bond_escrows_the_stake_and_activates_at_the_boundary`, \
      `withdraw_stages_the_validator_until_the_next_boundary`",
    note := "**the state machine now exists, so the row is a model claim rather than a Rust claim** \
      (2026-09-23). `Rchain/Pos.lean` grew `PosState` (the three coin fields, the pool, the active map, \
      the three ledgers, the two parameters), the gate (`isBoundary`, `divisor = max(epochLength, 1)`), \
      and the four steps in `close_block`'s order (`epochStep` = commit → move → pay → reselect), with \
      the gate *outside* the transition so `closeBlock_off_a_boundary` is a fact about the gate rather \
      than a restatement of a branch. What the row's statement names is now proved: a bond pools the \
      stake and leaves the active map alone (`a_bond_pools_but_does_not_activate`), the boundary is what \
      activates it **with the stakes it selects** (`a_boundary_activates_the_pool`, a statement about \
      the selected pairs rather than a set of ids since AUDIT C92's model change), and **an epoch \
      conserves** — every transfer is a \
      vault-to-vault move, so user vaults + staking vault + Coop vault are invariant \
      (`epochStep_conserves`, from `payDue_conserves` plus the observation that the other three steps \
      are ledger steps). **The refusal is structural, not a hypothesis**: `payDue` is partial \
      (`Option`) and returns `none` when the vault cannot cover the payout, because the port's \
      `debit_pos_vault` *fails* the transfer (`native_state.rs:debit_pos_vault`, its refusal inside `debit_pos_vault`) and \
      `close_block` (`native_state.rs:close_block`) writes nothing on that path — the debit's `?` inside it \
      returns before any of its state writes — an unguarded `Nat` subtraction would truncate the debit and mint the \
      difference, which is the quiet-wrong-answer shape this project refuses everywhere else. Two \
      falsifications, both run: deleting the payout's vault debit makes `payDue_conserves` unprovable, \
      and weakening the guard (`≤` to always-pay) does the same — the second is the one that shows the \
      guard is what keeps the theorem true. The earlier note, kept for the record: \
      not `close_block`'s four steps or the gate over them, so the statement above is a claim about the \
      Rust, evidenced by those tests and anchored here. What is worth formalizing is not the gate \
      itself (`if boundary then … else s` restates its own definition, which is the `vacuous` shape) \
      but the **conservation** an epoch preserves: the staking vault plus the Coop vault plus every user \
      vault is invariant, which is what makes a payout a transfer rather than a mint. That is this \
      row's Programme C item. **The absence rule's third form, and what it cost (2026-10-02, #150).** \
      It was first a filter over a per-validator record in native state (`pos:last_spoke`), read out of \
      the boundary block's **pre-state** — which is a function of the proposer's justification set, and \
      nothing requires a block to justify everything it has seen, so the penalty could be *aimed* at a \
      chosen rival. It also measured the wrong event: an attestation block carries no system deploys and \
      so recorded nothing, while an attestation *is* a message. It now reads a participation map derived \
      from the **last finalised fringe**, which is what the epoch seed reads and for the same reason, and \
      the record it replaced is **retired** — leaf, codecs, op, proto field (reserved) and variant, with \
      the block-level system-deploy list losing an entry. **Two things that cost are stated here rather \
      than left to the diff**: retiring the entry shifts every positional seed in that list down by one, \
      which is a consensus change of the same class as an insertion; and a block written before the \
      retirement is refused at **replay** rather than at the wire, because an unknown proto oneof field \
      decodes to `Empty` and `replay_block_system_deploy` is what rejects an `Empty` entry in a block's \
      system-deploy list. The read's residual is the seed's own: the steering space is \"one per \
      reachable fringe\", so a stale fringe moves every validator's reading back together and singles \
      nobody out, but it is a reduction and not a closure" },
  { number := 45, layer := "PoS",
    rustWitness := ["rholang/src/native_state.rs:an_epoch_splits_the_pot_and_keeps_the_dust"],
    statement := "The epoch's split: `pot * (bondᵢ / minimumBond) / (activeBonds / minimumBond)` per \
      active validator, out of `pot = posBalance − totalBond − totalWithdraw − committedRewards`, \
      committed per validator and paid only when the validator leaves",
    status := .provedModel,
    declarations := [`Rchain.rewardPot, `Rchain.reward],
    rust := ["rholang/src/native_state.rs"],
    witness := [`Rchain.the_dust_is_real],
    falsifiable := some "`Rchain.the_dust_is_real` decides an instance: minimum bond 3, bonds `[4, 8]`, \
      pot 10 — the validators' scaled shares are `4/3 = 1` and `8/3 = 2`, so they are paid 2 and 5 and \
      the epoch distributes **7 of 10**. A statement that said the shares sum to the pot is refuted by \
      that line, and so is one that dropped either division. On the Rust side \
      `an_epoch_splits_the_pot_and_keeps_the_dust` builds exactly that state (bonds 4 and 8, \
      `minimum_bond` 3, a pot of 10 paid in as phlo) and reads the split back, so the implementation is \
      checked against the arithmetic rather than against a remembered number. **The two bonds differ by \
      more than one `minimumBond` for a reason** (AUDIT C149): this instance used to bond `[4, 5]`, where \
      both scaled shares are 1, so the factor `bond / minimumBond` under test was the identity and a \
      `reward` that dropped it satisfied this line exactly as the real one did — the degeneracy is in the \
      fixture's parameters, so no assertion-level reading and no coverage count can see it",
    note := "the formula is the Scala's `getCurrentEpochRewards` (`casper/src/genesis/resources/Pos.rhox:241-256`), and the port's \
      `epoch_reward` agrees with it wherever the contract is *defined*; where it is not — \
      `minimumBond = 0`, or a normaliser of zero — the contract divides by zero and faults the deploy, \
      and the port pays zero instead (registered in `spec/audit/passes.md` §6). The model's `reward` is total, so \
      the two-part statement is: the model is the formula, and the Rust is the model on the model's \
      domain" },
  { number := 46, layer := "PoS",
    rustWitness := ["rholang/src/native_state.rs:an_epoch_splits_the_pot_and_keeps_the_dust",
      "rholang/src/property_tests.rs:law46_the_shares_never_exceed_the_pot"],
    statement := "The split **does not conserve**: `Σ rewards ≤ pot`, and the difference is the dust of \
      two integer divisions — which is not lost but stays in the pot for the next epoch",
    status := .provedModel,
    declarations := [`Rchain.sum_rewards_le_pot, `Rchain.list_sum_div_le, `Rchain.div_add_div_le,
      `Rchain.nsum_map_mul_left, `Rchain.the_dust_is_real],
    rust := ["rholang/src/native_state.rs"],
    witness := [`Rchain.the_dust_is_real, `Rchain.sum_rewards_le_pot],
    falsifiable := some "the inequality is **strict in an instance**: `the_dust_is_real` is minimum \
      bond 3, bonds `[4, 8]`, pot 10, seven units distributed of ten (`decide`d, so the strictness is a \
      computation rather than a remark), and `an_epoch_splits_the_pot_and_keeps_the_dust` reads the \
      same three units of dust back out of the Rust's pot afterwards — the dust is still there for the \
      next epoch. **The bonds are 4 and 8, and that gap is load-bearing** (AUDIT C149): they were 4 and \
      5, where `4/3 = 5/3 = 1`, so the factor `bond / minimumBond` under test was the identity for every \
      validator the instance built and a `reward` that dropped the factor satisfied it exactly as the \
      real one did",
    note := "the row this register most needed from Programme B: a conservation law written as an \
      *equality* would have been false, and the Scala's own comment does not say which it means. The \
      two divisions are the whole content — the model decides the question by computing `Pos.rhox`'s \
      arithmetic in `Nat` — and the Lean statement's hypothesis is the case the contract leaves \
      defined (`0 < activeBonds / minimumBond`), which is the same boundary `spec/audit/passes.md` §6 records on \
      the Rust side" },
  { number := 47, layer := "PoS",
    rustWitness := [
      "rholang/src/native_state.rs:withdraw_stages_the_validator_until_the_next_boundary",
      "rholang/src/native_state.rs:a_released_withdrawal_pays_the_bond_plus_the_committed_rewards"],
    statement := "A withdrawal is **staged**: the request records `quarantineLength + epochLength * \
      (1 + blockNumber / epochLength)` and changes nothing else; the validator leaves the pool at the \
      next boundary, and is paid `bond + committed rewards` at the first boundary past its quarantine",
    status := .provedModel,
    declarations := [`Rchain.PosClaim, `Rchain.PosRequest, `Rchain.withdrawDeadline, `Rchain.stage,
      `Rchain.movePending, `Rchain.dueClaims, `Rchain.payoutOf, `Rchain.duePayout, `Rchain.setKey,
      `Rchain.lookup, `Rchain.a_staged_withdrawal_moves_no_coins,
      `Rchain.the_move_escrows_the_bond_and_pays_nothing,
      `Rchain.a_due_claim_is_paid_its_bond_plus_its_committed,
      `Rchain.a_claim_before_its_deadline_is_not_paid,
      `Rchain.the_move_does_not_disturb_the_ledger,
      `Rchain.the_reward_is_committed_before_the_leave],
    axioms := [],
    rust := ["rholang/src/native_state.rs"],
    witness := [`Rchain.a_staged_withdrawal_moves_no_coins, `Rchain.the_move_escrows_the_bond_and_pays_nothing, `Rchain.a_due_claim_is_paid_its_bond_plus_its_committed, `Rchain.a_claim_before_its_deadline_is_not_paid, `Rchain.the_move_does_not_disturb_the_ledger, `Rchain.the_reward_is_committed_before_the_leave],
    falsifiable := some "`withdraw_stages_the_validator_until_the_next_boundary` asserts the three \
      stages separately — still bonded and still active with a deadline after the request; out of the \
      pool and escrowed (vault balance still zero) after the first boundary; paid after the second — \
      and `a_released_withdrawal_pays_the_bond_plus_the_committed_rewards` asserts the *sum* \
      (`bond 40 + committed 5 = 45`) **and** that the staking vault is exactly emptied by it, which \
      fails if the payout is not the claim's own two parts",
    note := "the contract's ordering is the law's substance, and **it is now modelled** (2026-09-23): \
      the reward is committed *before* the move, so a validator earns in the epoch it leaves and is paid \
      that reward later, when it is no longer in the pool — which is why the claim stores the bond and \
      reads the reward from the committed map at payment time \
      (`casper/src/genesis/resources/Pos.rhox:582`, the `withdrawers` write, and `:604`, the \
      `payWithdrawer` send; the contract's \
      header comment describing the stored pair as `bond + reward` is the *payee's* sum, not the \
      record's). The three stages are three theorems over `Rchain/Pos.lean`'s state machine: staged \
      (`a_staged_withdrawal_moves_no_coins` — the deadline is `quarantineLength + epochLength * (1 + n / \
      divisor)` and the pool, active set and coins are untouched), escrowed \
      (`the_move_escrows_the_bond_and_pays_nothing` — the claim carries the bond and the payer's vault \
      is still empty), and paid (`a_due_claim_is_paid_its_bond_plus_its_committed`; \
      `a_claim_before_its_deadline_is_not_paid` is the other half of 'only expired quarantines'). The \
      ordering itself is `the_move_does_not_disturb_the_ledger` plus \
      `the_reward_is_committed_before_the_leave`, which is the port's own test run as a decided \
      instance; the *general* form of that instance (a `foldl` induction over `setKey` for any pool and \
      any reward function) remains owed, and is recorded as owed rather than glossed — the instance \
      catches a dropped or reordered commitment, the general lemma would catch nothing more about \
      *this* mechanism. **Read against the contract before accepting that argument (2026-09-24), and \
      it holds**: the removal *is* a `ListOps fold` \
      (`casper/src/genesis/resources/Pos.rhox:607`, `computeRemove`), but it folds over a literal \
      `setKey` sequence with no parametric pool or reward function to vary, so a general `foldl` \
      induction would quantify over a function the contract does not have — and what the ordering \
      actually is, and what the decided instance pins, is the *phase* order: `payWithdraw` reads \
      `committedRewards` at `:604` and `computeRemove` deletes it only from `:610`, the payments \
      awaited first (`:601`). Owed with that reason, rather than as an aspiration" },
  { number := 48, layer := "Casper",
    statement := "A **denied** deploy's effects are excluded from the merged state, and the merge's \
      objective counts its cost exactly as an included deploy's — so the fee consequence RCHIP-02 \
      proposes (the deployer is not charged, the validator is not rewarded) holds in neither tree",
    status := .retired,
    rust := ["casper/src/merging.rs", "rholang/src/native_state.rs", "docs/src/node/block-merge.md"],
    falsifiable := some "the statement is a claim about what the port *does not* do, so its falsifier \
      is an implementation: a test that refunds a denied deploy's phlo, or one that drops its cost \
      from the merge objective, would refute it. Both halves are visible today — a denied deploy \
      appears in `BlockMessage::rejected_deploys` while its `deploy_chain_cost` still counts toward \
      `deployChainCost`'s sum, and its phlo was taken by the pre-charge and now funds the epoch pot \
      (laws 45/46) rather than being returned",
    note := "**retired 2026-09-25: the port has no rule of this shape** — the first half of this row's \
      statement is true *by construction*, the second is a proposal, and so there is nothing here to \
      prove. That is a decision recorded, not an omission. **The construction, first**: the port's merge \
      builds its event-log index from the deploys it is handed and from nothing else — \
      `DeployChainIndex::apply`'s `for d in deploys { EventLogIndex::combine(&event_log_index, \
      &d.event_log_index) }` (`casper/src/merging.rs:192-215`) — and its caller hands it the **accepted** \
      set, with the denied deploys carried beside it in `rejected_deploys` (`:98-100`). A denied \
      deploy's effects are therefore absent from the merged state because they are never read: a fact \
      about a `for` loop, whose theorem would be one that cannot fail — and the rule this register \
      applies to its own models (G6's, and C95's) is that a statement holding by construction is a \
      statement nothing can falsify. So the first half is recorded rather than modelled, and what is \
      left is the second, which is a **design decision and not a gap. And it is a question shared with \
      the Scala, not a port divergence.** RCHIP-02 is a \
      proposal: `MergeScope.scala:87` defaults `rejectionCost` to `DeployChainIndex.deployChainCost`, \
      and `DeployChainIndex.scala:73` defines that as `deploysWithCost.map(_.cost).sum` — the same \
      objective this port's `merging.rs` computes over the same set. Nothing in either tree refunds a \
      denied deploy, and `docs/src/node/block-merge.md`'s open-items list has said so since the merge \
      work landed. **What Programme B changed about it**: before the staking vault existed the \
      deployer's phlo was *burned*, so 'the validator is not rewarded' was true for want of any \
      reward; now the phlo is in the pot and the epoch distributes it to the active set (laws 45/46), \
      so the RCHIP's second half has become false in a new way, indirectly and at epoch granularity — \
      and the port's epoch rewards have no per-deploy counterpart in which 'the validator is not \
      rewarded *for this deploy*' could even be stated. Closing it means choosing an objective and a \
      refund path that the Scala does not have, which is a consensus change without an oracle; the \
      decision taken is to leave it open *with the reason*, in the row and in the doc, rather than to \
      implement a proposal and call it a port" },
  -- ── Rholang: what a deploy is charged (Law 49) ─────────────────────────────────────────────────
  { number := 49, layer := "Rholang",
    rustWitness := ["rholang/src/storage.rs:a_matched_produce_refunds_its_storage_before_the_event_costs"],
    statement := "For a matched produce/consume the charged gas is the Scala's: the storage is charged       up front and what the match consumed is **refunded** — the continuation's consume storage and the       produce storage of every removed datum — *before* the event and COMM costs",
    status := .provedModel,
    declarations := [`Rchain.prefixes, `Rchain.peak, `Rchain.itotal, `Rchain.peak_four,
      `Rchain.peak_refunds_first, `Rchain.itotal_refunds_first],
    rust := ["rholang/src/storage.rs"],
    witness := [`Rchain.peak_refunds_first, `Rchain.itotal_refunds_first],
    falsifiable := some "two halves, and the Rust test asserts both: \
      `a_matched_produce_refunds_its_storage_before_the_event_costs` (1) checks the matched call's total \
      against the same op *without* a match plus the COMM cost minus the two refunds — which fails if a \
      refund is dropped — and (2) finds a balance between the two peaks at which the deploy completes, \
      failing one phlo below it. The Lean side carries the second half as arithmetic: \
      `peak_refunds_first` says the refunds-first order's peak is at most the other's, and \
      `itotal_refunds_first` says their totals are *equal* — so a test that checked only the total \
      would pass with the refunds charged last, and (2) is what cannot",
    note := "**the first law about gas rather than about state**, and the reason it needed a model is \
      the *peak*, not the sum: `CostAccounting::charge` refuses a step that would take the balance \
      negative, so what a deploy needs is the largest prefix total of its charges. A refund credited \
      before the event and COMM charges lowers that peak; one credited after them does not — the two \
      sequences then need 192 and 200 phlo respectively for the same 184 of work, which is what the \
      test's tight balance measures. The port charged the storage and never refunded it, recording the \
      gap as a 'safe over-charge': safe because no deployer is ever *under*-charged, which is exactly \
      why it survived review. Modelled at the instance the charge sequence has (two charges, two \
      refunds) rather than in general: the general form is the same argument iterated, and the \
      iteration needs a permutation lemma over `List` that carries no further content — `Charging.lean` \
      says so rather than proving a statement no caller uses" },
  { number := 50, clause := "a", layer := "Rholang",
    statement := "A term's **AST depth** is bounded: `parDepth` is the quantity, `maxAstDepth` (768) \
      is the bound, and the parser refuses a source whose tree exceeds it — so no consumer of a term \
      (the normalizer, the sorter, `well_scoped`, the evaluator, the matcher, the printer) recurses \
      deeper than the bound, on a stack the bound was measured against",
    status := .provedModel,
    declarations := [`Rchain.parDepth, `Rchain.maxAstDepth, `Rchain.notsDepth,
                     `Rchain.walkPar, `Rchain.walkExceeds, `Rchain.walkParDroppingExpr,
                     `Rchain.walkPar_iff_parDepth, `Rchain.walkExceeds_sound,
                     `Rchain.walkExceeds_complete, `Rchain.parDepth_notsDepth,
                     `Rchain.a_dropped_arm_breaks_soundness,
                     `Rchain.the_walk_refuses_the_mutant_witness],
    witness := [`Rchain.walkPar_iff_parDepth, `Rchain.walkExceeds_sound,
                `Rchain.walkExceeds_complete, `Rchain.a_dropped_arm_breaks_soundness,
                `Rchain.parDepth_notsDepth,
                `Rchain.the_walk_refuses_the_mutant_witness],
    rust := ["rholang/src/parser.rs"],
    rustWitness := [
      "rholang/src/parser.rs:rejects_a_deep_ast_that_stays_inside_both_component_guards",
      "rholang/src/parser.rs:accepts_a_deep_ast_under_the_budget",
      "rholang/src/parser.rs:a_maximal_chain_still_parses",
      "rholang/src/parser.rs:every_construct_in_the_parser_walk_is_descended"],
    falsifiable := some "**the walk's own children function is where it fails, so the falsifier is a \
      dropped arm — and it is a theorem now, not a comment.** `walkPar_iff_parDepth` is the agreement \
      (`walkPar p k = false ↔ parDepth p ≤ k`), of which the two directions this row quotes are \
      `walkExceeds_sound` and `walkExceeds_complete`. The witness is `notsDepth`, whose depth is \
      `parDepth_notsDepth`'s `2 * n + 1`: `a_dropped_arm_breaks_soundness` shows the walk with its \
      `exprs` arm deleted (`walkParDroppingExpr`) accepting `notsDepth 384` — depth 769, one past the \
      bound — so a walk that could not be broken that way would be a restatement of the definition \
      rather than a check of it, which is the vacuity law 22 records. \
      `the_walk_refuses_the_mutant_witness` is the contrast: the real walk refuses the very term the \
      mutant admits. **The witness had to be repaired to make that true** (2026-09-28): `notsDepth` \
      was `List.replicate n (enot unit)` — `n` *siblings*, so `parDepth` was 3 for every `n ≥ 1` and \
      the mutation test would have passed **vacuously**, accepting at the bound along with every \
      mutant of it. **The half that is not a Lean claim at all**: that the Rust walk descends into \
      every `Proc` constructor. No theorem can say it — this walk is over the de Bruijn `Par`, and \
      `exceeds_ast_depth` walks the parser's surface `Proc` — so it is pinned by \
      `every_construct_in_the_parser_walk_is_descended`, which parks a deep child in each position \
      `push_sub_procs` must reach",
    note := "**`proved-model` (2026-09-28), and the reason it was `owed` for two days is worth \
      keeping** (2026-09-26, AUDIT C99). The parser's older guards bound two *shapes* and compose into \
      nothing: `MAX_PARSE_DEPTH` bounds the parser's recursion, and a flat chain is built by a loop — \
      bounded frames, an AST of depth `n` — so `d` levels of `c` operators compose into an AST of \
      depth `d × c` with both guards satisfied. Measured on the node's own 32 MiB worker: a debug \
      build **aborts the process** between AST depth 732 and 994 (a ~4 KB deploy term, SIGABRT), and a \
      release build costs **64 s** at 8,040 and aborts at 50,100 — while the deploy path runs parse \
      and normalize *before* any phlo or balance check, so that is free to the submitter. The fix is \
      `MAX_AST_DEPTH` + the walk, and this row is that fix's *quantity*; the agreement between the two \
      is `walkPar_iff_parDepth`, in `Rchain/Depth.lean`'s 23-member `mutual` theorem block — the \
      recipe this row names, `Rchain/FreeVars.lean`'s, and the block carries **no `termination_by` \
      and no `decreasing_by`**. \
      **Why it took three attempts, and the correction** (2026-09-28): this file's own note had blamed \
      `termination_by`. `Rchain/FreeVars.lean` falsifies that — its definitions carry it and its 23 \
      theorems sit in a block without it. The variable was where the extra `Nat` sits: `FreeVars` \
      takes the recursed term first with the level as a plain pattern variable, while the walk as \
      first committed matched the budget *in the equation header*, which routes the definition through \
      a well-founded fixpoint whose equations the theorems must then be proved about. The reshape is \
      the row's own recipe applied correctly. Dropping `termination_by` was never the fix, and the \
      attempt that dropped it paid in `Decidable`. \
      **What this row does not claim.** `rholang/src/parser.rs::exceeds_ast_depth` walks the surface \
      `Proc`; its own doc comment calls its count \"a proxy — and a deliberate one\", because the `Par` \
      \"adds a small constant per construct\". So `walkPar_iff_parDepth` is an agreement *inside the \
      model*, and the bridge to the parser's tree is a modelling argument rather than a theorem. \
      **One of the consumers this clause's statement names is not bounded by this constant, and is \
      handled rather than bounded** (2026-09-26, AUDIT C101): the *evaluator*'s frames are larger than \
      the normalizer's — a parsed flat chain as send data (inside both parser guards) returned `Ok` at \
      300 and 320 links and **aborted the process** at 350, below this bound — and the fix is that a \
      left-nested operator chain is now evaluated **iteratively**, so its length costs no stack and no \
      second constant is needed. So the sentence above holds for the normalizer, the sorter, \
      `well_scoped`, the matcher and the printer *by this bound*, and for the evaluator *by \
      construction*; lowering this constant instead would have refused every chain of 257..512 links \
      the chain guard admits" },
  { number := 50, clause := "b", layer := "Rholang",
    statement := "A **runtime-built value** is depth-bounded on the route the parser cannot see: the \
      space refuses a produced value whose **`Par`-nesting** exceeds `maxValueDepth` (256), so a value \
      a program built by folding cannot reach a consumer with more than 256 `Par` levels — and at most \
      `3 * 256 = 768` of clause a's `parDepth`, the same number as `maxAstDepth`, which is the honest \
      form of the relation between the two quantities rather than an identification of them",
    status := .provedModel,
    declarations := [`Rchain.parDepth, `Rchain.maxValueDepth, `Rchain.pairsDepth,
                     `Rchain.parNestDepth, `Rchain.walkValuePar, `Rchain.valueWalkExceeds,
                     `Rchain.walkValueParDroppingExpr, `Rchain.walkValuePar_iff_parNestDepth,
                     `Rchain.valueWalkExceeds_sound, `Rchain.valueWalkExceeds_complete,
                     `Rchain.valueWalkExceeds_sound_parDepth,
                     `Rchain.parDepth_le_three_mul_parNestDepth,
                     `Rchain.parNestDepth_pairsDepth, `Rchain.parDepth_pairsDepth,
                     `Rchain.a_dropped_value_arm_breaks_soundness,
                     `Rchain.the_value_walk_refuses_the_mutant_witness],
    witness := [`Rchain.walkValuePar_iff_parNestDepth, `Rchain.valueWalkExceeds_sound,
                `Rchain.valueWalkExceeds_complete, `Rchain.valueWalkExceeds_sound_parDepth,
                `Rchain.parDepth_le_three_mul_parNestDepth, `Rchain.parNestDepth_pairsDepth,
                `Rchain.parDepth_pairsDepth, `Rchain.a_dropped_value_arm_breaks_soundness,
                `Rchain.the_value_walk_refuses_the_mutant_witness],
    rust := ["models/src/types.rs", "rholang/src/storage.rs"],
    rustWitness := [
      "models/src/types.rs:the_value_walk_admits_the_limit_and_refuses_past_it",
      "models/src/types.rs:every_construct_that_carries_a_par_is_walked",
      "rholang/src/storage.rs:a_value_at_the_bound_is_stored_and_one_past_it_is_refused_on_both_produce_paths"],
    falsifiable := some "**the falsifier is a dropped arm of the walk's children function, over the \
      value route's own shape — and it is a theorem now, not a comment.** \
      `walkValuePar_iff_parNestDepth` is the agreement (`walkValuePar p k = false ↔ parNestDepth p ≤ \
      k`), of which `valueWalkExceeds_sound` and `valueWalkExceeds_complete` are the two directions \
      this clause quotes. `pairsDepth` — nested pairs, what a folding contract builds and what an \
      attacker builds — has `parDepth_pairsDepth`'s `2 * n + 1` of `parDepth` against \
      `parNestDepth_pairsDepth`'s `n + 1` of nesting, so at `maxValueDepth` the guard admits \
      `pairsDepth 255` (`parDepth` 511) and refuses `pairsDepth 256` (`parDepth` 513). \
      `a_dropped_value_arm_breaks_soundness` shows the walk with its `exprs` arm deleted \
      (`walkValueParDroppingExpr`) accepting `pairsDepth 256` at the bound — so a walk that could not \
      be broken that way would be a restatement of the definition rather than a check of it, which is \
      the vacuity law 22 records — and `the_value_walk_refuses_the_mutant_witness` is the contrast. \
      **The Rust half of the same risk** (a constructor the walk never descends into) is not a Lean \
      claim at all — this walk is over the de Bruijn `Par`, the Rust value walk is over the Rust AST, \
      which has no Lean model — and is pinned by \
      `models/src/types.rs:every_construct_that_carries_a_par_is_walked`, which parks a depth-10 child \
      in each of the 22 positions the walk must reach and asks for a limit of 5. Verified by mutation \
      before it was trusted: disabling the `bundles` arm makes exactly that case fail",
    note := "**`proved-model` (2026-09-28), and the number is a measurement rather than a preference** \
      (2026-09-26, AUDIT \
      C100). A contract folding its accumulator into a deeper pair reaches depth `n` in `O(n)` reduce \
      steps, inside `DEFAULT_MAX_REDUCE_STEPS`; on the node's 32 MiB worker in a debug build a fold to \
      depth 101 costs 7.3 s of CPU and a fold to depth 401 **aborts the process** \
      (`thread 'tokio-rt-worker' has overflowed its stack`, SIGABRT) inside `eval_single_expr`'s \
      recursion over the value. So the bound sits below that abort, at 256, and *apart from* clause \
      a's 768 — the two walks have different frames (the parser route's overspill is ~1,000, the value \
      route's is ~401), and sharing one number would be a bound that does not fire before the crash it \
      exists to prevent, which is what this unit's first draft shipped. Depth counts *nesting*, not \
      length: a flat list of any size is depth 2, and it is the accumulator-nesting shape — which no \
      contract needs — that this refuses. \
      **What the guard covers, and the residues it does not.** It sits at \
      `rholang/src/storage.rs::ChargingRSpace::check_value_depth`, called by **both** produce paths, so \
      it bounds the values the space holds and therefore every later reader of them. Four routes \
      remain unguarded and are named rather than implied away: a produce's *channel*, a consume's \
      channels/patterns, a continuation's body, and state restored from persistence at boot. It also \
      does not bound the **evaluator's** recursion while building the value, because the guard runs \
      after that walk — the honest statement of the mechanism is that the deepest value ever built is \
      capped at the bound + 1 (iteration `i` evaluates depth `i` and produces depth `i + 1`). \
      **The accounting the owed statement got wrong, and what replaced it** (2026-09-28): this row \
      said the Rust walk gives an `Expr` node **no level of its own** while `parDepth` counts it, and \
      that the gap is therefore a slack bounded by `MAX_PARSE_DEPTH` (128) because \"no runtime path \
      builds `Expr` nodes\". Every clause of that is false. The walk gives **no element node** a level \
      — `push_value_fields` pushes its fields at the depth it was handed, as do `push_value_expr` and \
      `push_value_connective`, so `Send`/`Receive`/`New`/`Match`/`Bundle`/`MatchCase`/`Connective` \
      are transparent along with `Expr` — so the counted quantity is the number of **`Par` nodes** on \
      the deepest `Par`-chain, which is `Rchain.parNestDepth` in `Rchain/ValueDepth.lean` and not \
      `parDepth` at all. And a runtime path *does* build `Expr` nodes: `(a, b)` **is** \
      `Expr::ETuple`, built by the reducer, which is the shape `rholang/tests/deep_value_bound.rs` \
      exists to exercise. The gap is a *factor*, not a constant: \
      `Rchain.parDepth_le_three_mul_parNestDepth` proves it is at most three, and three is tight — two \
      element nodes can sit between consecutive `Par`s (`Receive`→`ReceiveBind`, \
      `Match`→`MatchCase`), so a `Match` ladder spends three levels per `Par` and \
      `parDepth ≤ 2 * parNestDepth` fails from two rungs. At this bound that is `3 * 256 = 768`. \
      **Two boundaries of the tie are named rather than hidden**: the Rust guard never pushes its root \
      (its fields go on the worklist at depth 2), so it accepts a value with no `Par` child when \
      `limit = 0` where the model refuses it, and the two agree for every `limit ≥ 1` — which is what \
      `maxValueDepth` is; and the guard walks `New.injections`, which the model's `New.mk` does not \
      have, so it is *stricter* there, which is the safe direction. \
      **The gap this unit's own claim hid**: the guard's first draft sat inside `produce`, under a \
      comment calling it the one place a value enters the space; \
      `rholang/src/storage.rs::produce_at` — the scheduled path the channel scheduler and the deferred \
      block paths use — reaches RSpace directly and never passed through it, so one of two produce \
      entries was open. One shared method now, and the test fails on the old shape: with the scheduled \
      call disabled, a 257-deep datum is *stored* (`ScheduledProduce { application: None, .. }`, no \
      error) and \
      `rholang/src/storage.rs:a_value_at_the_bound_is_stored_and_one_past_it_is_refused_on_both_produce_paths` \
      reports it. **The ordering between the two bounds is structural rather than asserted**: \
      `MAX_VALUE_DEPTH` is written as the smaller of 256 and `crate::parser::MAX_AST_DEPTH`, so a \
      parser bound \
      lowered below 256 lowers it too and the ordering cannot drift. (`min` spelled as an `if`, because it is not const-callable here.) Asserting it instead was tried \
      and refused twice, which is worth recording: a `#[test]` over two constants can never fail, and \
      the `const`-block the linter suggests in its place is a *panic site in production code*, which \
      the type-system gate counts — so the invariant is carried by the definition rather than by a \
      check. **Not closed by this clause, and filed separately**: a *parsed* term of depth \
      257..768 is still an abort route (the evaluator recurses before the guard sees the datum), and \
      nested tuples make the parser exponential — both are C-row findings of this pass, and neither is \
      a depth bound's business" },
  -- ── Progress: the shapes of non-progress (Laws 51) ───────────────────────────────────────────────
  { number := 51, clause := "a", layer := "Progress",
    rustWitness := [
      "casper/src/engine/lfs_block_requester.rs:a_walk_nobody_serves_fails_rather_than_hangs",
      "casper/src/engine/lfs_block_requester.rs:a_slow_but_progressing_walk_is_not_abandoned"],
    statement := "A protocol's failure to make progress has three **shapes**, and each implies a \
      different repair: the requirement is unsatisfiable in **every reachable state** (`Void` — change \
      the requirement), a satisfying state was left and **no rule restores it** (`Terminal` — give the \
      refused state an inverse), or a run's **measure never moves** however long it runs (`Drift` — \
      bound the rate). The vocabulary is parameterised over a transition system, so one set of \
      definitions serves the reducer and the protocol built on it, and each shape is separated from the \
      others by one question: `Void` from `Terminal` is whether the goal was ever satisfiable, \
      `Terminal` from `Drift` is whether anything moved at all",
    status := .provedModel,
    declarations := [`Rchain.System.Reach, `Rchain.System.Enabled, `Rchain.System.Stuck,
      `Rchain.System.Silent, `Rchain.System.Void, `Rchain.System.Terminal, `Rchain.System.Unrestorable,
      `Rchain.System.Persistent,
      `Rchain.System.Run, `Rchain.System.Drift, `Rchain.System.Paced],
    axioms := [],
    rust := ["casper/src/validate.rs", "block-storage/src/dag/liveness.rs",
      "casper/src/blocks/proposer/proposer.rs", "casper/src/engine/lfs_block_requester.rs"],
    witness := [`Rchain.System.void_goal_is_never_waiting,
      `Rchain.System.unrestorable_lost_is_never_regained, `Rchain.the_two_cycle_drifts],
    falsifiable := some "each shape is inhabited, which is what stops the vocabulary being a list of \
      empty names: `Rchain.the_two_cycle_drifts` exhibits a `Drift` on the smallest system that has one \
      (a 2-cycle with a constant measure, so no `Paced` bound holds), and the `Void` and `Terminal` \
      lemmas are *implications* whose hypotheses the instances must prove of real rules — C174's \
      partition filter is `Void` because a validator that never speaks has no message to be seen, and \
      C173's refusal is `Terminal` because the rules have no inverse for it. **Refuted by deleting a \
      hypothesis**: an `Unrestorable` property that a step restores makes \
      `Rchain.System.unrestorable_lost_is_never_regained` false by construction, and a system whose steps \
      all advance the measure makes `Rchain.the_two_cycle_drifts` unprovable — which is exactly the \
      obligation C171's fix is owed (`Paced`)",
    note := "**What this row is about, and why it is a law rather than a module.** Four findings of \
      2026-09-28/29 were the same property failing and were argued and fixed as one-offs: C174 (a \
      fringe partition no state could satisfy), C173 (a refusal no rule clears), C171 (steps forever, \
      finality unmoved), and the two *causes* below (C170, C172). Nothing in the tree stated the \
      general property, so two readings of one paragraph could disagree about a fix with nothing able \
      to settle it — and one of them proposed shrinking finality's quorum denominator, which the \
      counterexample under law 52's row now refuses. **`Fair` is named and not modelled**, and that is \
      a theorem of this tree rather than a preference: \"an enabled step that no schedule takes\" is \
      unstatable over a transition *relation*, and `Rchain.reduce_not_deterministic` \
      (`Rchain/Concurrent.lean`) is the repo's own proof that the flat calculus fixes no schedule at \
      all — so starvation is registered `open` rather than defined as a shape. **Not modelled either**: \
      the protocol's inactivity leak (a state change that burns a silent validator's *bond*) — and that \
      is a **decision rather than a debt** (2026-10-02, §59): the constraint governing this workstream is \
      that a staker must not lose its bond through no fault of its own, and a partition is the canonical \
      case of exactly that, so the instrument the port has instead is an **income-only** participation \
      rule (law 44) that scales an epoch's reward and cannot reach a stake. The citations this used to \
      carry (#24, #39) are both closed; the leak is not owed by anyone. **`Drift` has a port-side instance \
      now, not only C171's**: the LFS block walk's give-up rule (`MAX_IDLE_ROUNDS`, \
      `casper/src/engine/lfs_block_requester.rs`, §6, issue #102) is a `Paced` condition on a measure \
      the state already kept — `LfsState::finished`, monotone because `done` only adds and `add` \
      refuses a key that is there — and this row's two `rustWitness` entries falsify it in both \
      directions: deleting the give-up leaves the walk drifting, and deleting the reset abandons a \
      slow walk that is still completing blocks" },
  { number := 51, clause := "b", layer := "Progress",
    rustWitness := ["casper/src/blocks/proposer/proposer.rs:a_silent_validators_stale_message_does_not_carry_the_quorum"],
    statement := "The two **causes** of a non-progress shape, on the predicate side: a liveness \
      predicate read off a **history** is not reading the current view (`Historic` — C170's guard \
      counted a validator that had *ever* spoken), and two readers of one state can **split** (`Split` \
      — C172's index and store answered different questions about the same block). The causes produce \
      the shapes: the historic predicate is what let the guard's suppression never fire (C171's \
      `Drift`), and the split is what turned an unresolvable justification into a dropped block with \
      nothing to re-queue it (a `Terminal` stall)",
    status := .provedModel,
    declarations := [`Rchain.Historic, `Rchain.ReadsTheView, `Rchain.System.Split],
    axioms := [],
    rust := ["casper/src/blocks/proposer/proposer.rs", "casper/src/block_metadata_store.rs"],
    witness := [`Rchain.historic_refutes_reading_the_view,
      `Rchain.System.split_refutes_agreement],
    falsifiable := some "both are *refutations*, so the falsifier is a predicate that does what the \
      cause says cannot be done: a liveness predicate that reads only the current view refutes \
      `Rchain.Historic` (the window in `block-storage/src/dag/liveness.rs` is such a predicate, \
      and the Rust test named above is its witness), and two readers that cannot disagree refute \
      `Rchain.System.Split` (which is what C172's fix — one order, or one authoritative side — \
      establishes). **The index/store test for the second half lands with the fix itself** (PR #106, \
      `a_failed_store_write_is_not_left_in_the_index`): this row may not name it until the branch it is \
      on carries it, because the witness checker refuses a symbol that is not in the file",
    note := "The two axes are the point, and they are diagnostic rather than taxonomic: the shape says \
      *what* is stuck, the cause says *where the fix goes* — a `Drift` whose cause is `Historic` is \
      fixed in the predicate, not with a rate limiter, and C171 is exactly that case. \
      `docs/src/formal/progress.md` carries the triage table keyed on both columns, and is the page an \
      agent reads before proposing a fix" },
  { number := 51, clause := "c", layer := "Progress",
    rustWitness := [],
    statement := "The vocabulary is **shared with the ρ-calculus rather than parallel to it**: a \
      `System` is a step relation plus a decidable \"is a step available now\" predicate *tied to the \
      relation by the structure's own field*, and the calculus supplies that field with its own \
      `takesStep`/`ReduceP` tie. Silence in the calculus is therefore stuckness, by the tie, and the \
      layers differ in exactly one way: in the closed calculus nothing outside the term can add a send, \
      so a lone receive is silent forever, while at the protocol level delivery is itself a step — \
      which is why a blocked wait and a permanent stall must not share a name",
    status := .provedModel,
    declarations := [`Rchain.silenceSystem, `Rchain.twoCycle],
    axioms := [],
    rust := ["spec/conformance/silence.tsv", "rholang/tests/lean_silence_corpus.rs"],
    witness := [`Rchain.a_silent_term_is_stuck],
    falsifiable := some "`Rchain.a_silent_term_is_stuck` is the checkable half — a term that reports no \
      step has no step — and it is proved *from* the tie, so a `takesStep` that answered `false` where \
      a `ReduceP` step exists would falsify it. The calculus's own verdicts are pinned by the silence \
      corpus (`spec/conformance/silence.tsv`, 13 cases, `spec/Rchain/Silence.lean`'s `silenceCases_decide` and \
      `rholang/tests/lean_silence_corpus.rs`), which is what makes this a claim about the calculus \
      rather than about a definition in a new file",
    note := "The correspondence is documented, not proved as an embedding, and every row of it must say \
      which kind it is — `definitional`, `instance`, `theorem <name>`, or `not modelled, because \
      <reason>` — the register's own `status` discipline applied to a table that is prose. Two rows are \
      honestly `not modelled`: the join rule (`spec/Rchain/Silence.lean` already records that boundary, and \
      a half-filled join is silence there as it is in the node) and the inactivity leak above" },
  { number := 51, clause := "d", layer := "Progress",
    rustWitness := [
      "casper/src/blocks/proposer/proposer.rs:round_escape_permit_is_consumed_even_when_the_round_stays_open",
      "casper/src/blocks/proposer/proposer.rs:late_escape_and_repeated_ticks_do_not_bank_unused_permits"],
    statement := "**A time-gated action consumes its permit.** The proposer's escape from a round \
      that is not closing may fire only after a full `ROUND_STALL_ESCAPE` interval, and firing it \
      re-arms the sighting: a still-open round must wait another full interval. A banked permit — \
      the defect shape, `banked` — licenses every later tick once the first interval elapsed, so the \
      bound stops pacing anything and a fast producer can exhaust the finality-lag bound while its \
      peers are still assembling the next shared justification layer",
    status := .provedTied,
    corpus := some "recovery",
    declarations := [`Rchain.Pacing.Permit, `Rchain.Pacing.take, `Rchain.Pacing.banked,
      `Rchain.Pacing.a_permit_is_consumed, `Rchain.Pacing.the_banked_gate_fires_every_tick,
      `Rchain.Pacing.the_correct_gate_waits_a_full_interval],
    axioms := [],
    rust := ["casper/src/blocks/proposer/proposer.rs"],
    witness := [`Rchain.Pacing.a_permit_is_consumed,
      `Rchain.Pacing.the_banked_gate_fires_every_tick,
      `Rchain.Pacing.the_correct_gate_waits_a_full_interval],
    falsifiable := some "Deleting the sighting reset in `take_round_escape` makes \
      `round_escape_permit_is_consumed_even_when_the_round_stays_open` fire: the second tick at +1 ms \
      escapes. `late_escape_and_repeated_ticks_do_not_bank_unused_permits` pins the late sighting \
      (90 s) and the burst after it. The corpus row `pace 15000 15001` expects no escape and fires \
      under the banked model, while `the_banked_gate_fires_every_tick` exhibits that model's defect \
      as a theorem",
    note := "The code was adopted from candidate `b26bda1a` (branch \
      `aixaria0:verify/quiet-finality-integrated-20261030`): `proposer.rs` is byte-identical between \
      that branch's merge-base and this tree, so only the proposer hunk was taken. The fix is \
      **consensus-neutral** — it gates `AlreadyProposedThisRound`, not block validity, the state \
      hash or the wire — and `ROUND_STALL_ESCAPE` is unchanged at 15 s. The model's `bound` is 15 000 \
      ms; the corpus (`spec/conformance/recovery.tsv`) is the tie to `take_escape_permit`. The layer \
      is named `recovery` because the decline of an escape storm is a recovery property; later \
      clauses (the receiver re-offer and the fault classification) extend it." },
  { number := 52, clause := "a", layer := "Casper",
    rustWitness := [
      "casper/src/blocks/proposer/proposer.rs:the_quorum_is_measured_against_the_whole_bonded_map_not_the_live_one"],
    statement := "**Finality's quorum denominator is the whole bonded stake, and only the partition may \
      shrink.** Two disjoint groups cannot each hold a strict supermajority of one total \
      (`supermajorities_overlap`: `3·s₁ > 2·t` and `3·s₂ > 2·t` force `t < s₁ + s₂`), so a denominator \
      taken from the *live* set — the fix this repository's own plan proposed on 2026-09-29, *so a \
      silent validator leaves numerator and denominator together* — lets each side of a partition \
      finalise its own history. Shrinking the **partition** is what restores liveness, because a \
      validator that has stopped producing messages stops being required to have seen the cut",
    status := .provedModel,
    declarations := [`Rchain.supermajorities_overlap, `Rchain.bonds4, `Rchain.bondsAB, `Rchain.bondsCD,
      `Rchain.suppOf, `Rchain.Participation, `Rchain.Delivery, `Rchain.StalenessBound],
    axioms := [],
    rust := ["block-storage/src/dag/liveness.rs", "block-storage/src/dag/finalizer.rs",
      "casper/src/blocks/proposer/proposer.rs"],
    witness := [`Rchain.supermajorities_overlap, `Rchain.the_live_denominator_lets_two_sides_finalise,
      `Rchain.the_quorum_is_the_whole_bonded_map],
    falsifiable := some "the arithmetic is general and the counterexample is its instance on one fixture, \
      and the instance is **two-sided on purpose**: `Rchain.the_live_denominator_lets_two_sides_finalise` \
      shows the two disjoint sides each advancing at their own denominator (200 of 200) and both being \
      refused at the bonded one (200 of 400), while `Rchain.the_quorum_is_the_whole_bonded_map` shows the \
      *full* partition advancing there (300 of 400) — so the second cannot be read as *the gate never \
      fires*. A change that made the denominator the live set makes the second conjunct of the first \
      theorem false, which is the disagreement this week settled by an instance. **What it does not \
      claim**: that two conflicting *messages* cannot be supported — the step from *the supporting sets \
      overlap* to *the chain cannot fork* is the DAG's causality, which is Law 15's \
      (`Rchain.seen_monotone_of_reaches`) and is a named hook here rather than a theorem",
    note := "**This is the row the increment-2 plan needed and the tree did not have.** The plan handed \
      the live weight set to `calculate_finalization` as the bonds map — both questions of one map — and \
      the argument for it was that a silent validator leaves numerator and denominator together. It does, \
      and that is the defect: over the live set the gate is `3·F > 2·L` with `F ≤ L`, which holds for \
      *any* self-consistent subset, so under a network partition each side finalises its own view. The \
      shipped fix is asymmetric for this reason (partition = the live set, quorum = the whole bonded map, \
      registered in `spec/audit/passes.md` §6, AUDIT C174), and the phrasing of the requirement that this \
      law pins is the issue's own title: finality needs quorum *stake*, not live nodes. **The hypotheses \
      are named in the instance module** (`Rchain.Participation`, `Rchain.Delivery`, \
      `Rchain.StalenessBound`) rather than left implicit, because a liveness claim without its hypothesis \
      is Law 20's trap: `law20_deadlock_freedom` was an axiom until it was deleted as unprovable as \
      stated" },
  { number := 52, clause := "b", layer := "Casper",
    rustWitness := [
      "casper/tests/finalization.rs:a_silent_bonded_validator_does_not_cap_the_fringe",
      "block-storage/src/dag/liveness.rs:the_window_is_heights_behind_the_tip",
      "block-storage/src/dag/finalizer.rs:a_seer_outside_the_live_partition_does_not_void_a_candidate"],
    statement := "**The requirement must be one a reachable state can satisfy.** With the whole bonded set \
      as the *partition*, a validator that produces no message makes the full-partition filter \
      unsatisfiable — not slow, **empty**: `allBonded` demands that every seer's seen set contain the \
      bonded set, and no seer can have seen a validator that never spoke. That is C174's measured shape, \
      where the survivors at 80 % of the stake could not lift the gate at all, and it is Law 51's `Void` \
      with its proof obligation discharged here: the emptiness is *derived from the predicate* rather \
      than observed on a fixture. **The filter is containment, not equality** (#213): a seer outside the \
      live partition — a stopped validator that last spoke after the candidate — does not void a \
      candidate the whole partition has seen (`Rchain.a_seer_outside_the_partition_does_not_void_a_candidate`)",
    status := .provedModel,
    declarations := [`Rchain.allBonded_false_of_a_silent_seer,
      `Rchain.allBonded_false_of_seers_omitting,
      `Rchain.fullPartitionStake_eq_zero_of_a_silent_bonded,
      `Rchain.a_seer_outside_the_partition_does_not_void_a_candidate],
    axioms := [],
    rust := ["block-storage/src/dag/liveness.rs", "block-storage/src/dag/finalizer.rs",
      "casper/tests/finalization.rs"],
    witness := [`Rchain.fullPartitionStake_eq_zero_of_a_silent_bonded,
      `Rchain.the_whole_bonded_partition_is_unsatisfiable],
    falsifiable := some "the general theorem is an induction over the support map whose step is \
      `Rchain.allBonded_false_of_a_silent_seer` — a seer whose seen set omits a bonded validator cannot \
      contain the bonded set — so a filter that kept such a candidate would falsify it. Its fixture instance \
      is **two-sided**: `Rchain.the_whole_bonded_partition_is_unsatisfiable` has the gate refusing three \
      speaking validators' candidates at 300 of 400 *and* advancing the identical fixture once the fourth \
      speaks, so the first half is a finding rather than a definition. Delete the hypothesis that a \
      validator is silent and the theorem is unprovable, which is the honest shape: what the fix changed \
      was the *hypothesis* (the partition is `Rchain.Participation`'s live subset), not the arithmetic",
    note := "The two clauses of this law are the two halves of the increment that landed on 2026-09-29, \
      and they are separable: clause a is safety (the denominator) and clause b is liveness (the \
      partition). C174's measurement is what forced the second — the survivors at 80 % could not resume \
      finality while the third validator was stopped, and #105's run froze with the survivor at 91 %, \
      where the binding constraint was a *message* and not a stake share. **What clause b still does not \
      fix, said plainly**: a net that stays below two thirds permanently (three equal validators minus \
      one is exactly two thirds) still cannot finalise. **And that is a decision rather than a debt** \
      (2026-10-02, §59): the instrument that would price it is an inactivity leak, a state change that \
      burns a silent validator's *bond*, and the constraint governing this whole workstream is that a \
      staker must not lose its bond through no fault of its own — a partition is the canonical case of \
      exactly that. What the port has instead is an **income-only** participation rule (law 44), which \
      scales a validator's epoch reward and cannot reach a stake. The citations this used to carry (#24, \
      #39) are both closed, and the leak is not owed by anyone" },
  { number := 53, clause := "a", layer := "Casper",
    rustWitness := [
      "casper/src/validate.rs:neglected_invalid_block_detects_bonded_invalid_justification",
      "casper/src/dag.rs:h1b_a_justified_bonded_failed_block_is_refused_rather_than_forced"],
    statement := "**One attribution is terminal: the refusal persists and the rule refuses its children.** \
      A node that marks a bonded validator's block failed never follows that validator's chain again, \
      because (i) the record persists — `mark_failed` records the metadata of every \
      `ValidateError::ValidationFailed` and no rule clears it — and (ii) `neglected_invalid_block` refuses \
      any block justifying a failed **bonded** sender's block, before any other rule runs (and, \
      independently, `block_number`'s maximum skips failed justifications, so a child numbered above the \
      last non-failed height is refused too). Together they are `Rchain.System.persistent_blocks_the_goal`: \
      from a state holding the refusal, **no** reachable state admits a block above it. The node is \
      `Terminal`, not slow",
    status := .provedModel,
    declarations := [`Rchain.Strand, `Rchain.strandStep, `Rchain.strandSystem, `Rchain.neglects],
    axioms := [],
    rust := ["casper/src/validate.rs", "casper/src/dag.rs", "casper/src/multi_parent_casper.rs"],
    witness := [`Rchain.the_refusal_is_persistent, `Rchain.a_neglected_block_is_detected,
      `Rchain.a_refused_validator_is_never_followed],
    falsifiable := some "**the falsifier is a rule that restores, and the port now has one.** \
      `Rchain.the_refusal_is_persistent` is proved from the modelled step relation, so a step that \
      unmarked a sender — a revalidation of failed metadata, or a bounded re-fetch — makes it false *by \
      construction*. That is exactly what happened: clause **b** models the port's restoring rule as \
      `Rchain.restoreStep`, and `Rchain.the_refusal_is_not_persistent_once_a_rule_restores` **is this \
      cell executed** — the red the guard was stated to produce, read off the elaborated environment \
      rather than asserted. This clause is therefore stated over the rule set *without* the restoring \
      rule, and is the guard that fired. The rule half is non-vacuous on the model's own fixture \
      (`Rchain.a_neglected_block_is_detected`: a block justifying a failed bonded sender's block is \
      neglected, one justifying a failed *unbonded* sender's is not) — which is also why option (a) of \
      C173's decision, counting failed justifications in `block_number`'s maximum, ships as well: the \
      unbonded route is the one `neglected_invalid_block` cannot close",
    note := "This is C173, filed as #105 from a live-testnet run on 2026-09-29: a two-validator chain \
      froze with the survivor holding 91 % of the stake, and the reason was not the quorum — the \
      survivor had marked the other's block failed, so it refused every block above it for good. **Both \
      rules are the oracle's**, which is why the law was a *decision* rather than a bug fix: `block_number` \
      skipped failed justifications (the Scala's `if (!m.validationFailed)`) and `neglectedInvalidBlock` \
      is a straight port. **What was not upstream is the consequence**: the failure is attributed \
      (reached by every `ValidateError::ValidationFailed`), so the block was recorded failed *and* \
      slashable, and the one-block-per-node divergence became a deterministic estrangement with slash \
      evidence attached. **The decision was taken on 2026-10-01 and both halves shipped** — clause b's \
      restoring rule, and option (a), which is now a registered §6 divergence from the Scala (the \
      maximum counts failed justifications). **Independently of C172's fix**, which is a dropped \
      `Internal` rather than a recorded `ValidationFailed` and does not touch this path" },
  { number := 53, clause := "b", layer := "Casper",
    rustWitness := [
      "casper/src/multi_parent_casper.rs:only_a_divergence_is_restorable",
      "casper/tests/restoring_rule.rs:only_a_divergence_is_revalidated",
      "casper/tests/restoring_rule.rs:one_block_budget_bounds_the_revalidations",
      "casper/tests/restoring_rule.rs:a_record_at_the_attempt_limit_is_not_revalidated_again"],
    statement := "**A refusal has an inverse, and the inverse is bounded.** Clause a's `Terminal` is \
      repaired by a rule that *unmarks*: a node re-validates a failed record against its current view, \
      and a record that now passes is cleared — so the block above it is admitted rather than refused. \
      This is `restore_divergent_justifications` (`casper/src/multi_parent_casper.rs`), and it is \
      `Rchain.restoreStep`, the exact inverse of `strandStep`. **What makes it a rule and not a retry \
      loop is that its work is bounded by three quantities the protocol bounds**: it is keyed on the \
      *cause* (only a view-dependent `Divergence` is eligible — an `Attributable` failure is the \
      block's own fault and is permanent, a `Cascade` is not about that block at all); it is capped per \
      record by a count that is *persisted*, so a restart does not refresh the budget; and it is \
      budgeted per incoming block, so the work one block can provoke is a constant times the cost the \
      protocol already pays to validate it. Clearing a record keeps `slashable` exactly as it was — the \
      refusal has an inverse, attribution does not",
    status := .provedModel,
    declarations := [`Rchain.Strand, `Rchain.restoreStep, `Rchain.strandSystemRestoring],
    axioms := [],
    rust := ["casper/src/multi_parent_casper.rs", "casper/tests/restoring_rule.rs"],
    witness := [`Rchain.the_refusal_is_not_persistent_once_a_rule_restores,
      `Rchain.a_restoring_step_removes_a_refusal],
    falsifiable := some "**the falsifier is the absence of the step**: delete `restoreStep` from \
      `strandSystemRestoring`'s relation and the reachable set is clause a's again, so \
      `the_refusal_is_not_persistent_once_a_rule_restores` cannot be proved — persistence returns. On \
      the Rust side it is the three bounds: `only_a_divergence_is_restorable` (the cause key), \
      `a_record_at_the_attempt_limit_is_not_revalidated_again` (the cap, and that it is spent), and \
      `one_block_budget_bounds_the_revalidations` (the budget), each observed red by deleting the clause \
      it names — which is what keeps this out of `candidate:bounded-work-per-step`'s class (C180): work \
      whose cost grows with an input nothing bounds. **What the clause does not claim**: it does not \
      promise the inverse always fires — a node whose own view is still divergent re-validates to the \
      same refusal, and is right to keep refusing a block it cannot verify. The rule gives the refused \
      state an inverse, which is what `Terminal` demands; the refutation theorem is a statement about \
      the relation, not a liveness promise about any particular run",
    note := "C173's fix, #125, landed 2026-10-01. The name is deliberately a *clause* rather than a \
      new number: it is not a new shape but the other half of clause a's — `Persistent` and its dual \
      (`Rchain.System.Unrestorable`) are what Law 51's vocabulary exists to tell apart, and a \
      refusal that its own fault cannot clear is exactly the case where the two must be stated \
      together. The three Rust bounds are the ones the register's C180 class asks for, because a \
      restoring rule that re-validated on every child would be an unbounded re-fetch triggered by an \
      input nothing bounds" },
  { number := 53, clause := "c", layer := "Casper",
    rustWitness := [
      "casper/src/multi_parent_casper.rs:a_spent_budget_rewinds_only_where_a_height_can_be_named",
      "casper/src/dag.rs:the_reset_rewinds_the_dag_and_removes_the_bodies_it_dropped",
      "casper/src/interpreter_util.rs:a_reset_request_is_raised_once_taken_once_and_wakes_the_driver",
      "casper/tests/reset.rs:a_reset_request_rewinds_the_stores_when_the_switch_is_on_and_does_nothing_when_it_is_off"],
    statement := "**A refusal has a second inverse, and it is not an unmark.** Clause b clears a record: \
      the node re-validates a failed block against its current view and unmarks it if it now passes. The \
      port has since grown a different rule (#320, F-U5-01's acting half): a node whose revalidation \
      budget is spent **rewinds to its last finalised block** and re-syncs from there, and the records \
      above the anchor go with the state they were about. On this model the two are different steps and \
      the difference is the point — `restoreStep` removes one element from the head, while `rewindStep` \
      drops a *prefix*, leaving a suffix. A rewind that dropped nothing, or from the other end, would be \
      a different rule about a different mechanism. **And the step is guarded, because the port refuses \
      it in two of its three states**: `rewind_plan` answers with an anchor *and* its height (the arm \
      that acts), with an anchor whose height the view cannot place, or with nothing at all — and only \
      the first can rewind, because `drop_above` takes a height and a reset without one would have to \
      replay from genesis. `Rchain.a_rewind_is_not_available_without_an_anchor` is that guard as the \
      theorem it is; without it this clause would certify a relief the port withholds",
    status := .provedModel,
    declarations := [`Rchain.Strand, `Rchain.rewindStep, `Rchain.strandSystemRewinding],
    axioms := [],
    rust := ["casper/src/engine/reset.rs", "casper/src/engine/node_launch.rs",
      "casper/src/multi_parent_casper.rs"],
    witness := [`Rchain.the_refusal_is_not_persistent_once_a_rule_rewinds,
      `Rchain.a_rewinding_step_removes_a_refusal, `Rchain.a_rewind_is_not_available_without_an_anchor],
    falsifiable := some "**the falsifier is the absence of the step, and the guard is the second one.** \
      Delete `rewindStep` from `strandSystemRewinding`'s relation and the reachable set is clause a's \
      again, so `the_refusal_is_not_persistent_once_a_rule_rewinds` cannot be proved — persistence \
      returns. On the Rust side the three witnesses are the reset's own decisions rather than its \
      machinery: the decision rewinds only where a height can be named (the middle and no-anchor arms \
      refuse), the durable half leaves the DAG at the anchor with the dropped bodies gone, and the \
      request is raised once, taken once and wakes the driver. **What the clause does not claim**: it is \
      not a liveness promise about any particular run — a node that rewinds and cannot reach a peer \
      again is in C259's state, which is the net-wide reconciliation's subject and not this clause's",
    note := "#320, register C274, landed 2026-10-11. Modelled as a clause rather than a number for the \
      same reason clause b is: it is the *other* repair of clause a's `Terminal`, and a reader must be \
      able to tell the two apart — `Persistent` versus `Unrestorable` is the distinction Law 51's \
      vocabulary exists for, and \"the refusal was cleared\" and \"the state it was about was discarded\" \
      are different answers to the same terminal state. **The worksheet had this wrong**: \
      `failure-hazop.md` credited R3 with reddening `the_refusal_is_persistent`, and that reddening came \
      from clause b's restoring rule (#125), not from #320 — a rewind is not an unmark, and reddening the \
      persistence theorem with one would be a false falsifier, asserting a rule the port does not have \
      for that hash. The corrected cell is F-U5-02's" },
  { number := 54, clause := "a", layer := "Casper",
    rustWitness := [
      "casper/src/blocks/proposer/proposer.rs:a_silent_validators_stale_message_does_not_carry_the_quorum",
      "block-storage/src/dag/liveness.rs:a_validator_with_no_message_at_all_is_not_live"],
    statement := "**A liveness predicate reads the current view, not a history.** A predicate that searches \
      a retention map — the port's `latest_msgs`, which keeps a silent sender's last message indefinitely \
      — has a verdict that depends on what has *ever* happened, so it is `Historic` (Law 51's cause): two \
      histories that agree on the current view and differ only in their past get different answers from it. \
      The predicate that replaces it takes the sender's **entry** and tests it against the window, which is \
      what makes the same three-validator fixture read 200 rather than 300 — the false supermajority C170 \
      measured",
    status := .provedModel,
    declarations := [`Rchain.Retention, `Rchain.everSpoke, `Rchain.inWindow, `Rchain.Historic,
      `Rchain.ReadsTheView],
    axioms := [],
    rust := ["block-storage/src/dag/liveness.rs", "casper/src/blocks/proposer/proposer.rs",
      "casper/src/multi_parent_casper.rs"],
    witness := [`Rchain.everSpoke_is_historic, `Rchain.the_retention_reader_counts_a_stale_sender,
      `Rchain.historic_refutes_reading_the_view],
    falsifiable := some "both halves are refutations of the other reading: `Rchain.everSpoke_is_historic` \
      exhibits two maps that agree on the current view and disagree on `everSpoke`, so a predicate with \
      that shape can never be `Rchain.ReadsTheView` (which `Rchain.historic_refutes_reading_the_view` \
      proves as the general complement); and `Rchain.the_retention_reader_counts_a_stale_sender` carries \
      the port's own numbers into the model — sender 3, seven heights behind the tip, counted by the \
      retention reader and refused by the window. The Rust test named above fails with `left: 200, \
      right: 100`, which is the same disagreement in stake rather than in verdict, so the model and the \
      code are pinned to the same fixture",
    note := "**This is the cause rather than the shape**, and the distinction is what makes the      classification diagnostic: C170's predicate is why C171's storm ran (the guard's suppression never \
      fired while the quorum *was* reachable) and why the arithmetic looked wrong when the defect was in \
      what the guard was reading. The fix has two halves and only one of them is arithmetic: the predicate \
      now reads the entry the view supplies (`liveness::live_weight_set`), and the *window* makes \
      \"current\" a bounded claim rather than an appeal to history — so a predicate that reads a view \
      still needs `Rchain.StalenessBound` to be a liveness predicate at all. **What it does not claim**: \
      that every predicate over a map is historic (a map with exactly one entry per sender is its own \
      view, which is why the port quotients to the newest per sender); the cause is the *shape of the \
      read*, and `Rchain.everSpoke` is the shape" },
  { number := 54, clause := "b", layer := "Casper",
    rustWitness := [],
    statement := "**One view, or both written together.** When a node keeps two views of the same set — the \
      port's in-memory DAG index and the persisted store it is built from — and updates them by \
      **separate** steps, a state is reachable in which one holds a key the other does not, and the two \
      readers that disagree there are exactly the ones whose answers decide validation (`has_all_deps` \
      asks the index; `block_summary` asks the store). The fix's shape is one step writing both: \
      `the_atomic_order_cannot_split` proves the invariant for every reachable state from a consistent \
      start, and `the_separate_steps_can_split` exhibits the state where the invariant fails",
    status := .provedModel,
    declarations := [`Rchain.TwoViews, `Rchain.viewsStep, `Rchain.atomicStep],
    axioms := [],
    rust := ["casper/src/block_metadata_store.rs"],
    witness := [`Rchain.the_separate_steps_can_split, `Rchain.the_atomic_order_cannot_split],
    falsifiable := some "**the falsifier is a rule, not a schedule**: `Rchain.the_atomic_order_cannot_split` \
      is an induction whose step rewrites both views, so a rule that updates one side alone — which is the \
      port's own pre-fix order — makes it unprovable by construction, and \
      `Rchain.the_separate_steps_can_split` is that rule's reachable witness. The initial condition is a \
      hypothesis rather than a fact, and it is the honest one: a node whose two views start out of step \
      stays out of step, which is why the fix states *which* side a reader should ask as well as ordering \
      the writes. The Rust side of this clause is AUDIT C172's fix and its two tests \
      (`a_refused_height_gap_is_not_left_in_the_index`, \
      `a_failed_store_write_is_not_left_in_the_index`, PR #106): the row may not name them until the branch \
      carrying them is the base, because the witness checker refuses a symbol that is not in the file",
    note := "The second cause, and the one that shows why the two axes are one axis: C172 is not a \
      progress defect at all — nothing is stuck, two readers simply answer different questions about the \
      same block — and it *produces* a `Terminal` stall, because the reader that disagrees feeds \
      validation, the failure is attributed, and Law 53's refusal then makes the stall permanent. The \
      diagnosis is what links them: a rule read a view that was not the authoritative one. **Independent \
      of clause a**: there the fix is to read the view, here it is to have one" },
  { number := 55, clause := "a", layer := "Casper",
    rustWitness := [
      "sdk/src/dag/merging.rs:a_budget_refuses_without_answering_and_never_changes_the_answer",
      "sdk/tests/merging_scaling.rs:rejection_options_are_bounded_on_a_directed_shape"],
    statement := "**The guard is before the work.** Every step the protocol takes has a cost bounded by a \
      quantity the protocol bounds, and the bound is a **hypothesis of the step** rather than a check \
      after it: `spendStep` charges one unit only while `spent < limit`, so `the_work_never_exceeds_the_ \
      budget` holds in every reachable state and `an_exhausted_budget_enables_no_step` says there is \
      nothing to take once the budget is gone. **The defect this is the inverse of has a shape, not just \
      a symptom**, and it is in the same model: `lateGuardStep` charges the step and *then* asks whether \
      it was allowed, and `the_late_guard_overspends` exhibits a state one unit past the budget reachable \
      in a single step from an exhausted start. That the two relations differ is what makes this clause a \
      claim rather than a definition — every row of the class AUDIT C180 names had the late guard",
    status := .provedModel,
    declarations := [`Rchain.Work, `Rchain.spendStep, `Rchain.workSystem, `Rchain.lateGuardStep,
      `Rchain.lateGuardSystem],
    axioms := [],
    rust := ["sdk/src/dag/merging.rs", "casper/src/merging.rs",
      "casper/src/blocks/block_receiver.rs", "casper/src/blocks/block_processor.rs"],
    witness := [`Rchain.the_work_never_exceeds_the_budget, `Rchain.an_exhausted_budget_enables_no_step,
      `Rchain.the_late_guard_overspends],
    falsifiable := some "**the falsifier is a relation, not a schedule**: `Rchain.the_late_guard_overspends` \
      derives an over-budget state from a step relation that carries no guard, so a port whose bound sat \
      after the work would make `Rchain.the_work_never_exceeds_the_budget` unprovable by construction — \
      the budget would be a counter, not a bound. On the Rust side the pair is \
      `a_budget_refuses_without_answering_and_never_changes_the_answer` (the refusal fires) and \
      `rejection_options_are_bounded_on_a_directed_shape` (the work counting toward the budget is bounded \
      by the **output** rather than by the width — observed red with the quotient disabled: 16,383 states \
      for one option at fourteen keys, where the bounded search expands fourteen)",
    note := "**This is the class statement AUDIT C180 asked for, and C178 is its first held member.** \
      C180 was filed as `candidate:bounded-work-per-step` with three symptoms — the merge search \
      (C178), the unbounded ingress queue (C175) and the start-up envelope (#68) — because the register \
      had no law that named what they shared, which is how one cause came to be tracked as three. The \
      `rust` cell is the merge search and the ingress path, and the two bounds landed on 2026-10-01 \
      (#147, #151); the third member is #68's envelope, and C189 records that its measurement no longer \
      reproduces on the current build, which is a fact about the rig rather than about this clause. \
      **What the clause does not claim**: that the cost is *small*, only that it is bounded by something \
      the protocol bounds. Raising the cost of an attack is not the same as removing it, and the \
      honest sentence every row of this class carries is that below the bound the work may still be \
      exponential" },
  { number := 55, clause := "b", layer := "Casper",
    rustWitness := [
      "sdk/src/dag/merging.rs:a_budget_refuses_without_answering_and_never_changes_the_answer"],
    statement := "**The bound cannot change an answer.** A run whose budget suffices returns what the work \
      produced, and **every larger budget returns the same thing** — \
      `a_larger_budget_does_not_change_an_answer` — so two nodes that both complete compute the identical \
      answer however different their budgets are. A spent budget carries **nothing** \
      (`a_spent_budget_carries_no_answer`), which is the port's shape rather than an analogy: \
      `SearchBudgetExceeded` carries two counters and deliberately no option set, because a truncated \
      option set would pick a different rejection. Together the two make the bound a **resource policy \
      and not a consensus change** — which is what Law 17's determinism requires of anything that can \
      stop a search early, and what the class's rows could not state while the law was missing",
    status := .provedModel,
    declarations := [`Rchain.boundedRun],
    axioms := [],
    rust := ["sdk/src/dag/merging.rs", "casper/src/multi_parent_casper.rs"],
    witness := [`Rchain.a_spent_budget_carries_no_answer,
      `Rchain.a_larger_budget_does_not_change_an_answer,
      `Rchain.a_completion_within_the_budget_is_returned],
    falsifiable := some "**the falsifier is a refusal that carries a partial answer**, and it is the shape \
      the port's type refuses: `a_spent_budget_carries_no_answer` is the empty case, so a `Refused` \
      constructor holding a truncated accumulation would have no model here — which is why \
      `SearchBudgetExceeded` has fields `steps` and `options` and no set. \
      `a_completion_within_the_budget_is_returned` is the non-vacuity witness: the clause is about a run \
      that *does* answer, not about one that never does. The Rust side is \
      `a_budget_refuses_without_answering_and_never_changes_the_answer`, which asserts both halves on the \
      port's own fixtures: an exceeded search returns an `Err` with no option set, and a budget that is \
      not hit returns the identical set the unbounded search returns",
    note := "**Why the clause is stated over a schedule and not over a counter.** The claim is \
      *all-or-nothing*, and in the port that is a fact about a type: the refusal has no field to put a \
      partial answer in, so a caller cannot mistake a truncated search for a result. A model that \
      carried a counter and an accumulator would make the property a policy the code documents rather \
      than a thing the code cannot express — which is the difference between a law and a comment, and \
      the reason Law 51's vocabulary exists one level up. **What it does not claim**: that any \
      particular budget is safe. `SearchBudget::NODE` is a provisional number (C184) and this clause \
      says only that whatever it is, two nodes that finish with it agree" },
  { number := 56, clause := "a", layer := "Rholang",
    statement := "**A name is rooted in the identity that owns it.** `publish(path, value)` is \
      accepted iff the path's owner prefix equals the caller's **derived** REV address — derived \
      from the deployer id (`rho:rev:address(\"fromDeployerId\", …)`), never supplied by the caller — \
      so a write outside your own root is *inexpressible* rather than refused, and a caller with no \
      derivable identity is refused before any shared state is touched. Versions are append-only and \
      `seal` closes a path; the mutable pointer lives only in the **alias tier**, which one authority \
      governs.",
    status := .provedModel,
    declarations := [`Rchain.NsPath, `Rchain.NsState, `Rchain.Verdict, `Rchain.publish,
      `Rchain.setAlias, `Rchain.writekeyValid, `Rchain.unguardedPublish],
    axioms := [],
    rust := ["casper/src/genesis/resources/rgov/MasterDictionary.rho",
      "casper/src/genesis/rgov.rs", "casper/src/genesis/mod.rs"],
    rustWitness := [
      "casper/tests/master_dictionary.rs:the_four_must_fail_cases",
      "casper/tests/master_dictionary.rs:publish_is_append_only_and_versions_are_pinned",
      "casper/tests/master_dictionary.rs:seal_closes_a_path_and_the_key_still_resolves",
      "casper/tests/master_dictionary.rs:a_granted_key_works_until_it_is_revoked",
      "casper/tests/genesis_registry.rs:only_the_ceremony_key_can_reach_the_admin_handle"],
    witness := [`Rchain.a_stranger_publishing_under_alices_root_is_refused,
      `Rchain.alice_publishing_under_her_own_root_is_accepted,
      `Rchain.a_forged_identity_is_refused,
      `Rchain.an_unguarded_publish_hits_anothers_root,
      `Rchain.a_sealed_path_refuses_a_further_version_and_keeps_its_versions,
      `Rchain.a_non_root_may_not_set_a_short_name,
      `Rchain.a_revoked_writekey_is_refused],
    falsifiable := some "**the falsifier is the same function with the ownership test removed.** \
      `unguardedPublish` differs from `publish` in exactly that one comparison, and \
      `an_unguarded_publish_hits_anothers_root` proves that the inputs \
      `a_stranger_publishing_under_alices_root_is_refused` refuses are *accepted* by it — so the \
      refusal is a claim about the guard rather than a restatement of the code. The controls keep the \
      other three clauses honest in the same way: `alice_publishing_under_her_own_root_is_accepted` \
      and `the_root_may_set_a_short_name` show the refusals are not a `publish` that refuses \
      everything, and `a_sealed_path_refuses_a_further_version_and_keeps_its_versions` carries both \
      halves (the refusal *and* the surviving version), because a seal that also destroyed the log \
      would satisfy the first alone. The Rust side is `casper/tests/master_dictionary.rs`, which runs \
      the four must-fail cases against the **deployed** dictionary — writing under another's root, a \
      granted key after `revoke`, publishing to a sealed path, a non-root setting a short name — and \
      `rgov.rs::the_deployer_id_is_only_ever_the_address_derivation`, which asserts the disclosure \
      rule of SECURITY.md over the contract's source",
    note := "**This is the design #71's thread converged on, and it is why #71 was only half closed.** \
      #71's stopgap published a `grant` capability, which answers \"who may write\" by *grant*; it does \
      not answer who may claim a name at block 0, whether a publish overwrites or extends, or whether \
      the bundled classes belong in genesis. The rooted design answers the first by *derivation* — the \
      owner prefix comes from the caller's identity, so there is nothing to claim and no admission \
      policy to freeze — and this row's clauses b and c answer the other two: `publish` appends rather \
      than overwrites, and the alias tier is the one governed pointer. **What it does not claim**: \
      anything about *encoding*. The model is over abstract tokens because the law is about which \
      comparisons decide a write; the base58 form, and the fact that a forged id derives `Nil` rather \
      than a different address, are properties of `rho:rev:address` and are pinned by \
      `casper/tests/master_dictionary.rs`, not here. Nor does it claim that a contract cannot *disclose* \
      a deployer id it was given: that is a discipline over the chain's contracts, not a mechanism the \
      calculus can enforce, and the source scan is the floor under it" },
  { number := 57, layer := "PoS",
    statement := "**A delegator's stake is the operator's bond, attributed.** A delegation moves a \
      principal from the delegator's own vault into the staking vault and adds it to the operator's key \
      — the `pool` entry becomes the **aggregate** — so the stake counts in the draw and in what a slash \
      reaches exactly as the operator's own does, while the `pos:delegations` ledger records who owns \
      the part that is there. Each epoch's reward for that key is split pro-rata across the operator's \
      own stake and its delegations **exactly** — the operator keeps the integer-division remainder — so \
      a validator with delegators commits the same total it would have committed alone, and the split is \
      the **identity** when nobody has delegated. An undelegation is a request that moves nothing until \
      a boundary acts on it, so a delegator cannot escape a slash already in flight; and a second \
      delegation from the same delegator **adds** to the first rather than replacing it",
    status := .provedModel,
    declarations := [`Rchain.PosDelegation, `Rchain.PosUndelegation, `Rchain.PosDelegationClaim,
      `Rchain.delegatedTotal, `Rchain.proRata, `Rchain.delegate, `Rchain.undelegate,
      `Rchain.lookup_setKey_self, `Rchain.a_second_delegation_accumulates,
      `Rchain.the_total_belongs_to_one_operator, `Rchain.proRata_sum_le,
      `Rchain.split_sums_to_the_reward, `Rchain.split_with_no_delegators_is_the_operator,
      `Rchain.the_split_is_not_the_identity, `Rchain.the_refund_is_split_pro_rata,
      `Rchain.delegate_conserves,
      `Rchain.a_delegation_enlarges_the_pool, `Rchain.undelegate_moves_no_coins,
      `Rchain.a_slash_clears_the_delegations],
    axioms := [],
    rust := ["rholang/src/native_state.rs", "rholang/src/system_processes.rs"],
    rustWitness := [
      "rholang/src/native_state.rs:the_delegation_leaves_are_absent_until_the_first_delegation",
      "rholang/src/native_state.rs:the_split_is_not_the_identity",
      "rholang/src/property_tests.rs:law57_the_split_sums_to_the_reward",
      "rholang/src/property_tests.rs:law57_the_delegators_never_take_more_than_the_reward",
      "rholang/src/property_tests.rs:law57_the_split_with_no_delegators_is_the_identity",
      "rholang/src/native_state.rs:a_delegation_enlarges_the_pool_and_a_second_one_accumulates",
      "rholang/src/native_state.rs:a_delegation_reaches_the_active_set_that_casper_reads",
      "rholang/src/native_state.rs:a_boundary_splits_the_reward_across_the_delegations",
      "rholang/src/native_state.rs:a_slash_fans_out_to_each_delegators_own_vault_at_every_tier",
      "rholang/src/native_state.rs:an_undelegation_is_staged_then_paid_to_the_delegator",
      "rholang/src/native_state.rs:a_validator_with_delegations_cannot_withdraw",
      "rholang/src/native_state.rs:the_delegation_refusals",
      "rholang/src/native_state.rs:the_delegation_leaves_round_trip_or_refuse"],
    witness := [`Rchain.a_second_delegation_accumulates, `Rchain.the_total_belongs_to_one_operator,
      `Rchain.proRata_sum_le, `Rchain.split_sums_to_the_reward,
      `Rchain.split_with_no_delegators_is_the_operator, `Rchain.the_split_is_not_the_identity,
      `Rchain.the_refund_is_split_pro_rata, `Rchain.delegate_conserves, `Rchain.a_delegation_enlarges_the_pool,
      `Rchain.undelegate_moves_no_coins, `Rchain.a_slash_clears_the_delegations],
    falsifiable := some "**the split's fixture was measured against three plausible wrong splits, not \
      asserted.** `the_split_is_not_the_identity` decides `proRata 100 30 [10, 61] = [9, 60]` — a \
      denominator of 101 over principals 10 and 61, flooring to 9 and 60 and leaving 31 to the operator \
      — together with `proRata 100 0 [30, 70] = [30, 70]` and `proRata 100 40 [1, 2] = [2, 4]`. Compiled \
      against those clauses: the operator keeps everything, every delegator gets an equal share \
      (`amount / (base + weights.length)`), and the base is dropped from the denominator. **Clauses 1 \
      and 3 refute all three; clause 2 refutes only the first**, because with `base = 0` it cannot tell \
      a rule that divides by the base from one that ignores it — recorded rather than left implied. \
      **Clause 3 read `[1, 1, 1]` when it was written and that was nearly vacuous for the same reason**: \
      three equal weights pay `100 / 3` under the real rule and under the equal-share mutant alike, so \
      it distinguished nothing clause 1 did not; `[1, 2]` with base 40 reads `[2, 4]`, which the \
      equal-share mutant reads as `[2, 2]` and the base-drop mutant as `[33, 66]`. The dormancy half is \
      `split_with_no_delegators_is_the_operator`, and the aggregate half is \
      `a_delegation_enlarges_the_pool`, which is red if `delegate` writes the ledger without the pool",
    note := "**Landed in two units, and the row is honest about which half is which.** The model and the \
      state's shape came first (the same change's spec unit); the Rust — the four leaf codecs, the two \
      `rho:rchain:pos` ops, the boundary move and split, the slash fan-out and the two refusals — is the \
      second, and `rustWitness` is declared with it. **Every falsifier was run against a mutation that \
      removes the thing it pins**, which is the standard this register keeps and which the split's own \
      fixture is the newest example of: dropping the slash fan-out, writing an empty delegation leaf, \
      replacing instead of accumulating a second delegation, not splitting the reward, removing the \
      withdrawal guard and paying an undelegation to the operator each turn the matching test red. \
      **Two established laws are touched rather than restated**: `sum_rewards_le_pot` (46) is a statement \
      about the reward *before* this split, which `split_sums_to_the_reward` shows the split preserves \
      exactly; and `atRisk` (law 45's slash) now reads the aggregate pool entry, which is why the \
      delegation ledger is **not** added to it — counting a delegator's principal twice would overstate \
      what a slash may take, and the escrowed claim and the accrued delegated reward, which have left the \
      pool or never joined it, are what the sum does add. The port's `slash` has the same subtraction for \
      the same reason, and getting it wrong there is a slash that mints. **What this row does not \
      claim**: which vault receives a refund, in the *model*. `PosState.user` is the total over every \
      user vault, so the per-vault fan-out is inexpressible there and is pinned by \
      `a_slash_fans_out_to_each_delegators_own_vault_at_every_tier` and \
      `an_undelegation_is_staged_then_paid_to_the_delegator` instead — the same simplification the \
      withdrawal path already carries. Nor does the model claim anything about the port's **dormancy at \
      the store**: that the four leaves are left absent rather than written empty is a property of \
      `native_state.rs`'s write path (`set_*` is an unconditional `put`, so the delegation setters clear \
      instead), pinned by `the_delegation_leaves_are_absent_until_the_first_delegation`" },
  { number := 58, layer := "Casper",
    statement := "**The gate publishes on a fully-participating set — the forward dual Law 14 lacks.** \
      Law 14's gate is a biconditional over a **free** `supp : SupportMap`: it says the fringe advances \
      *given* a support map that is a strict supermajority of the bonded total, and never says one \
      **obtains**. The live stall of 2026-10-03 (C209) was exactly that gap — a fully-live \
      three-validator net produced blocks and finalised nothing — and the model could say nothing about \
      it, because `Participation` and `StalenessBound` were declared and used in no theorem. \
      `participation_all_current` supplies the missing step (a participation-closed view *is* the whole \
      bonded set, hence a full partition), `gate_publishes_of_a_full_partition` and \
      `nextFringe_publishes_of_a_full_partition` carry it through the gate to the fringe, and \
      `the_licence_ends_at_the_horizon` bounds the attestation licence so a finality stall cannot become \
      the C171 storm. **What is hypothesis, not theorem:** \"a fully-live set reaches a full partition \
      within N rounds\" quantifies over a schedule and this model has none — the tree's own \
      `reduce_not_deterministic` proves the flat calculus fixes no schedule, and Law 51's `Fair` is named \
      and undefined — so it stays a named hypothesis, as do `Delivery` and the guard's reader being \
      `ReadsTheView` (the model can refute the *old* reader, not prove the new one). **The \
      `3 × LIVENESS_WINDOW` factor is measured, not proved** (`quiet_chain_tests`: a healthy round \
      finalises a deploy within 3 heights, a killed-validator round within 7).",
    status := .provedTied,
    corpus := some "liveness",
    declarations := [`Rchain.currentOf, `Rchain.stalenessBound_iff, `Rchain.participation_all_current,
      `Rchain.seersOf, `Rchain.suppOfView, `Rchain.allBonded_seersOf, `Rchain.bondedSupport_suppOfView,
      `Rchain.filterMap_stakeOf_eq_totalStake, `Rchain.fullPartitionStake_suppOfView,
      `Rchain.gate_publishes_of_a_full_partition, `Rchain.nextFringe_publishes_of_a_full_partition,
      `Rchain.inHorizon, `Rchain.the_licence_ends_at_the_horizon,
      `Rchain.latestOf, `Rchain.liveOf, `Rchain.currentOf_iff, `Rchain.window_iff_heights_behind],
    axioms := [],
    rust := ["casper/src/blocks/proposer/proposer.rs", "block-storage/src/dag/liveness.rs",
      "block-storage/src/dag/finalizer.rs"],
    rustWitness := [
      "casper/src/blocks/proposer/proposer.rs:the_licence_ends_at_the_horizon_and_the_work_does_not",
      "casper/src/blocks/proposer/proposer.rs:every_deploy_finalises_and_then_the_chain_is_quiet",
      "casper/src/blocks/proposer/proposer.rs:on_the_round_snapshot_a_deploy_sent_to_one_validator_is_never_finalised",
      "casper/src/blocks/proposer/proposer.rs:reading_only_the_work_from_the_seen_view_strands_the_genesis_signer"],
    falsifiable := some "the two round-snapshot *controls* in `quiet_chain_tests` are the falsifier: \
      reverting the guard to the round's parents makes \
      `on_the_round_snapshot_a_deploy_sent_to_one_validator_is_never_finalised` pass and this law's \
      premise false, and `reading_only_the_work_from_the_seen_view_strands_the_genesis_signer` refutes \
      the first cut — so \"every input from the seen view\" carries the forward implication rather than \
      decorating it. The horizon boundary is two-sided (`the_licence_ends_at_the_horizon`, pinned in \
      Rust by the test of the same name).",
    witness := [`Rchain.gate_publishes_of_a_full_partition, `Rchain.participation_all_current,
      `Rchain.the_licence_ends_at_the_horizon],
    note := "**The follow-up this row used to name has landed.** The row was `proved-model` over an \
      algebra the note called coarser than the node's — the model's `Nat`/`Option` against the port's \
      `BlockHeight`/`BTreeMap` — and the `liveness` corpus is the tie: eleven cases, each a tip, a window, \
      a bonds map and a latest-message map, whose verdict is the live weight set the filter returns. \
      `liveOf` is the model's; `block-storage/src/dag/liveness.rs:live_weight_set` is the node's; \
      `currentOf_iff` and `window_iff_heights_behind` are what make those one test rather than two \
      spellings that happen to agree on the cases somebody thought of. The cases carry the boundary \
      (`h + w = tip` is live, one past it is not) and the shape C174 is about — a bonded sender with **no \
      message at all**, which the node's `is_some_and` drops and a default height would have kept. \
      `Participation` and `Delivery` — declared in `Rchain/Casper/Liveness.lean` and used by nothing \
      before this row — are consumed here; `StalenessBound` is reached through `stalenessBound_iff`. \
      **What the tie does not reach:** the `3 × LIVENESS_WINDOW` factor, which is measured rather than \
      proved." },
  { number := 59, clause := "a", layer := "Wire",
    statement := "**A value the bridge decoded is a value it encodes, and decodes again** — the CapTP \
      value map round-trips its domain. Stated as Law 42 states the JSON round trip: over the values \
      `syToPar` answers, so a shape outside the domain carries no claim.",
    status := .provedTied,
    declarations := [`Rchain.parToSy, `Rchain.syToPar, `Rchain.wireable, `Rchain.wireableList, `Rchain.syrup_decode_encode, `Rchain.parToSy_decoded, `Rchain.ssToPars_round, `Rchain.kvsSyToPars_round, `Rchain.syToPar_isSome, `Rchain.ssToPars_isSome],
    axioms := [],
    corpus := some "syrup",
    rust := ["ocapn/src/par_value.rs"],
    rustWitness := ["ocapn/src/par_value.rs:collections_round_trip_and_a_tuple_stays_a_tuple"],
    witness := [`Rchain.syrup_decode_encode, `Rchain.parToSy_decoded],
    falsifiable := some "**the tuple, and it was false when the law was written.** `(true, 0)` — every \
      `(ok, value)` reply in this codebase — crossed as a Syrup `List` and came back an `EList`, so \
      `syToPar` of what `parToSy` produced was not the tuple: a peer could hold an ERTP purse and not \
      fund it (AUDIT C226). The URI was the same failure on a leaf (`GUri → Symbol` with `Symbol` \
      refused back). `syrupCases_decide` pins twelve cases, of which the tuple and the nested tuple are \
      two, and `node/tests/lean_syrup_corpus.rs` runs every one through the node's own bridge.",
    note := "**The law decided the wire shape, and two candidates were refuted by the references \
      rather than by preference.** A Syrup `record` is *labelled* and the label must be a string, \
      selector or bytestring (`@endo/ocapn`'s `decode.js`), so a bare record would need the label \
      `true`; and a record is not in Endo's CapTP passable union (`{list, struct, tagged}`) either. A \
      tuple therefore crosses as OCapN's **tagged** value, `<desc:tagged 'rho:tuple' [fields…]>` — the \
      union's own extension point, whose `value` may be any passable. `Sy.tuple` is that shape named, \
      so the round trip descends structurally instead of through a label test the equation compiler \
      cannot see (`taggedRecord` is the same shape spelled as a record). **Three shapes are outside the \
      domain, named:** a `Float64` (Rholang has no float), a **small** `GBigInt` — Syrup's integer is \
      one type where Rholang has two, so `GBigInt 5` and `GInt 5` share a wire form — and a \
      `GDecimal` (Rholang has an exact decimal; Syrup has no counterpart). **What the tie \
      does not reach:** the refusals' *reason strings* are the node's (`BridgeError`), tied by the \
      corpus's sources rather than by a theorem." },
  { number := 59, clause := "b", layer := "Wire",
    statement := "**Two wireable Syrup values that decode to the same `Par` are the same wire form.** \
      The decode is injective on the domain, so no two shapes stand for one value.",
    status := .provedTied,
    declarations := [`Rchain.syToPar_injective, `Rchain.parToSy_decoded],
    axioms := [],
    corpus := some "syrup",
    rust := ["ocapn/src/par_value.rs"],
    witness := [`Rchain.syToPar_injective],
    falsifiable := some "**a rendering rule refutes it, which is what makes the clause load-bearing \
      rather than decorative.** 'An inbound list argument becomes a tuple' is the cheap fix for C226, \
      and under it a peer's `[brand, 10]` and a peer's `(brand, 10)` would be one `Par` reached from \
      two wire forms — `syToPar_injective` is false of it by construction. The corpus's \
      `[1, 2]` row beside its `(true, 0)` row is the same statement on the wire: the two render \
      differently, and `node/tests/lean_syrup_corpus.rs` asserts it of the node.",
    note := "**The clause the bridge had no way to state before.** With only an encode and a decode, \
      'a list and a tuple are different things' is a fact about two functions that happens to hold; \
      stated as injectivity it is a property of the map, and it is what makes the *shape* a decision \
      the law fixes. Both clauses 59a and 59b follow from `parToSy_decoded` — what a value the decoder \
      answered encodes back to — so the row carries one proof obligation, not two." },
  { number := 59, clause := "c", layer := "Wire",
    statement := "**The encoder never emits a shape outside the domain.** A value with no counterpart on \
      the other side is *refused*, never approximated by one that nearly fits — §1.6's no-silent-\
      partiality applied to a wire.",
    status := .provedTied,
    declarations := [`Rchain.parToSy_wireable, `Rchain.exprToSy_wireable, `Rchain.parsToSy_wireable, `Rchain.kvsToSy_wireable, `Rchain.wireable],
    axioms := [],
    corpus := some "syrup",
    rust := ["ocapn/src/par_value.rs"],
    witness := [`Rchain.parToSy_wireable],
    falsifiable := some "**the URI was the violation, and it is why this clause exists.** `GUri → \
      `Symbol` outbound with `Symbol` refused inbound is a *lossy* map: the encoder emits a shape from \
      which the value does not come back, which is exactly what this clause forbids. The domain's \
      `float`, small-`GBigInt` and `GDecimal` exclusions are the other side of it — shapes the encoder must not \
      produce and the decoder must not accept. `node/tests/lean_syrup_corpus.rs`'s round trip is the \
      falsifier on the node: an encoder that emitted a lossy shape fails on the case that carries it.",
    note := "**This is the clause that turns a documented decision into a checked one.** The bridge's \
      module doc used to call the `GUri`/`Symbol` asymmetry 'deliberate and worth a second opinion', \
      and the tuple's image 'the one loss, and it is the loss Syrup forces'. Both were true as prose \
      about two functions and neither was a property: 59c says the encoder's image lies inside the \
      domain, so a loss is a violation rather than a note. The leaf is `exprToSy_wireable` over the \
      expression constructors — every one the encoder refuses is refused by the definition's own \
      fall-through, so the proof is a case per constructor rather than an argument." },
]

def lawsC : List Law := [
  { number := 60, clause := "a", layer := "RSpace",
    statement := "**An install is total on a channel.** An install that would replace a *different* \
      installed continuation is **refused**, and an idempotent re-install of the same one changes \
      nothing — so an install that cannot take effect says so rather than dropping what was there.",
    status := .provedModel,
    declarations := [`Rchain.installed, `Rchain.InstallStep, `Rchain.ReplacingStep, `Rchain.a_different_install_has_no_step],
    axioms := [],
    rust := ["rspace/src/hot_store.rs", "rholang/src/system_processes.rs"],
    rustWitness := [ "rspace/src/hot_store.rs:a_second_different_install_on_a_channel_is_refused", "casper/tests/determinism.rs:a_minted_vault_handle_answers_its_balance_arm"],
    witness := [`Rchain.a_different_install_has_no_step],
    falsifiable := some "**the defect's shape is a second relation, and the row is the pair.** \
      `ReplacingStep` is what `installed_continuations.insert` did — the second install replaces the \
      first — and `the_replacing_rule_does_not_keep_the_first` states that the first is then gone. \
      `the_vault_handles_balance_arm_was_lost` is the same fact at the size of the finding: install \
      `transfer` after `balance` on one channel and the channel no longer carries `balance`. On the \
      port, `rspace`'s test asserts the `Err`, and \
      `casper/tests/determinism.rs:a_minted_vault_handle_answers_its_balance_arm` reads the arm that \
      used to answer nothing — a vault seeded with 1_000_000_000, so a handler that replied a \
      constant fails it.",
    note := "**The refusal is the clause, and it is why 60a is not the store's own type.** The map \
      holds one `WaitingContinuation` per channel, so a second `insert` *cannot* fail — the \
      statement has to be about the operation, not the container: an install that would replace a \
      different continuation has no step, which is what `install_continuation` now returns. The \
      idempotent case is load-bearing rather than incidental: play and replay both install the system \
      contracts over one store, and a rule that refused the *same* install twice would break every \
      deploy. **What the tie does not reach:** the model's continuations are abstract tokens, so it \
      says which one a channel keeps and nothing about what a continuation is; `locked_install`'s \
      consume comparison is the node's, pinned by the rspace test rather than by a theorem." },
  { number := 60, clause := "b", layer := "RSpace",
    statement := "**What a channel carries after any permitted run** — the continuation it was first \
      given. A contract with several methods is therefore **one** continuation that dispatches on the \
      method, not several installs on one name.",
    status := .provedModel,
    declarations := [`Rchain.a_permitted_run_keeps_the_first, `Rchain.InstallStep, `Rchain.installed],
    axioms := [],
    rust := ["rholang/src/system_processes.rs", "rspace/src/hot_store.rs"],
    rustWitness := [ "casper/tests/determinism.rs:a_minted_vault_handle_spends_in_the_deploy_that_minted_it"],
    witness := [`Rchain.a_permitted_run_keeps_the_first],
    falsifiable := some "**the pair with 60a's falsifier is the falsifier**, which is the shape \
      `Rchain/Casper/Dag.lean` establishes for law 14b: `a_permitted_run_keeps_the_first` is the law \
      on the rule, and `the_replacing_rule_does_not_keep_the_first` is the *same statement* false of \
      the rule the fix replaces. Without the second, the first would be a restatement of a container's \
      type; with it, the rule is what is being judged. On the port the control is a *movement*: \
      `a_minted_vault_handle_spends_in_the_deploy_that_minted_it` asserts the transfer moved \
      30_000_000, so the one-continuation handle is a working capability rather than merely a \
      non-refusing one.",
    note := "**The shape every multi-method contract here already used, now the only shape that \
      installs.** `rev_vault`, `ertp` and `pos` are one continuation at `arity: 1, remainder: true` \
      with the method dispatched inside; a vault handle is the one site that did it the other way, and \
      this is the row that says which way is the rule. **What the change costs:** the handle's \
      `body_ref` is derived from `(name, arity)` and the arity went 2/5 → 1, so newly minted handles \
      move — consensus-visible, with the note in `spec/GENESIS.md`. The oracle's \
      `RevVault.rho:196-200` spells the two arms as two `contract`s on one name, which is what the \
      port's one-continuation-per-channel store cannot express; the dispatch is where that difference \
      is paid." },
  { number := 61, clause := "a", layer := "Progress",
    statement := "**A delivery in flight does not stall the loop.** With an answer outstanding, the \
      session's loop can still read the next delivery — so an unrelated delivery on the same session \
      is served while an object waits on something outside the session.",
    status := .provedModel,
    declarations := [`Rchain.Delivery, `Rchain.LoopState, `Rchain.Step, `Rchain.BlockingStep, `Rchain.a_pending_answer_does_not_block_the_next_delivery],
    axioms := [],
    rust := ["ocapn/src/conn.rs", "ocapn/src/owner.rs", "ocapn/src/bootstrap.rs"],
    rustWitness := [ "ocapn/tests/session_owner.rs:a_claim_that_waits_does_not_stall_its_session"],
    witness := [`Rchain.a_pending_answer_does_not_block_the_next_delivery],
    falsifiable := some "**the defect's shape is a second relation, and the pair is the claim.** \
      `BlockingStep` is what polling inside the delivery did — an answer outstanding means the loop \
      stays exactly where it is — and `the_blocking_rule_never_shortens_the_queue` states that under \
      it **no** step shortens the queue while an answer is outstanding: the loop spins, and the \
      delivery behind it is unreachable. That is the stall, and it is false of `Step`, where \
      `Step.defer` reads the delivery and takes the answer on. On the port \
      `ocapn/tests/session_owner.rs:a_claim_that_waits_does_not_stall_its_session` sends a signed \
      handoff claim for a gift nobody will ever deposit and then, on the **same** socket, a fetch: the \
      fetch must be fulfilled within seconds, where the old shape could not read it until the \
      ten-second deposit wait ended.",
    note := "**The statement is about the loop, because that is where the stall was.** `handle_deliver` \
      is awaited on the task that owns the socket, so an object that awaits something outside the \
      session does not merely delay its own answer — it stops the session reading anything. The fix \
      is `Reply::Deferred`: the object hands the loop a future, the loop writes the answer when it \
      lands, and the *connection stays single-owner*, which is the invariant the whole module is \
      built on. **What the model does not claim:** deliveries are abstract tokens, so it says when the \
      loop reads and nothing about what a delivery says, and the queue is a `List` where the port's is \
      a socket. **What the deferral costs:** the answer position of a deferred delivery is recorded \
      with no object, so a delivery *pipelined* onto it breaks rather than resolving — the honest \
      answer while the answer itself is unknown, and re-recording it later would be the silent \
      re-point C223 already refuses." },
  { number := 61, clause := "b", layer := "Progress",
    statement := "**And the outstanding answer is still written** — deferring is not dropping. The loop \
      reaches a state with nothing pending.",
    status := .provedModel,
    declarations := [`Rchain.Reach, `Rchain.a_deferred_answer_is_still_written, `Rchain.Step],
    axioms := [],
    rust := ["ocapn/src/owner.rs", "ocapn/src/conn.rs"],
    rustWitness := [ "ocapn/tests/session_owner.rs:a_claim_that_waits_does_not_stall_its_session"],
    witness := [`Rchain.a_deferred_answer_is_still_written],
    falsifiable := some "**the clause a deferral most easily gets wrong**, and the reason it is stated \
      separately: 'the loop reads on' is trivially satisfiable by never answering at all. The proof \
      exhibits the run — serve, then fulfil — so a relation that only had the reading step would not \
      satisfy it. On the port the same shape is the conformance suite's \
      `test_valid_handoff_wait_deposit_gift`, which withdraws *before* the deposit and requires the \
      gift anyway: the deferred path is exercised by \
      `spec/audit/evidence/ocapn-conformance/run-11.txt` (24/24), and a deferral that dropped the \
      answer would fail it.",
    note := "**Two clauses rather than one, because they fail independently.** 61a says the loop is not \
      blocked; 61b says the work is not lost. A change that answered nothing would satisfy the first \
      and fail the second, which is why the row exists — and why the port's fix had to write the \
      answer *from the loop* (a spawned waiter would have had to reach the socket, which is the \
      single-owner invariant). **What the model does not claim:** that the answer is written in \
      bounded time — `DEPOSIT_WAIT` bounds the wait and the `Deferred` queue is bounded \
      (`MAX_DEFERRED_ANSWERS`), but neither bound is in this model." },
  { number := 62, layer := "Wire",
    statement := "**A peer cannot extend this node's reach.** The node never dials, on a peer's word, \
      anything that peer could not dial itself — so a *remote* peer cannot aim this node at its own \
      loopback services, while a loopback peer (the conformance suite's own case) still can.",
    status := .provedModel,
    declarations := [`Rchain.Origin, `Rchain.Target, `Rchain.ownReach, `Rchain.isLocal, `Rchain.permits, `Rchain.originBlind, `Rchain.the_node_never_exceeds_the_peers_own_reach, `Rchain.the_origin_blind_rule_exceeds_the_peers_reach, `Rchain.denying_local_targets_outright_would_refuse_a_local_peer],
    axioms := [],
    rust := ["ocapn/src/dial_policy.rs", "ocapn/src/netlayer.rs", "ocapn/src/enliven.rs", "ocapn/src/fixtures.rs"],
    rustWitness := [ "ocapn/src/dial_policy.rs:a_remote_peer_cannot_reach_this_nodes_own_services"],
    witness := [`Rchain.the_node_never_exceeds_the_peers_own_reach],
    falsifiable := some "**the defect's shape is a second rule, and the pair is the claim.** \
      `originBlind` judges the target without the peer — what the policy did before the socket address \
      was threaded to it — and `the_origin_blind_rule_exceeds_the_peers_reach` states that under it, \
      with the **default** configuration, a peer elsewhere reaches this node's own services: reach the \
      peer does not have. The law's own thesis is a *containment* rather than a list of denied \
      targets, proved for every configuration of the policy's ingredients (the always-refused set, the \
      operator's `deny_local`, and the origin), so refusing more can only shrink the node's side. \
      `denying_local_targets_outright_would_refuse_a_local_peer` is the control that rules out the \
      cheap fix: refusing local targets outright breaks the demo the conformance suite is. On the port, \
      `ocapn/src/dial_policy.rs:a_remote_peer_cannot_reach_this_nodes_own_services` asserts both halves \
      — remote refused, loopback permitted — and \
      `a_remote_peer_cannot_reach_this_node_by_name` that a *name* resolving to loopback is the same \
      dial.",
    note := "**The origin had to be carried to the policy, and that is most of the fix.** The policy \
      could not tell one peer from another because nothing on the dial path knew where the request came \
      from: `NetConn` gains a provided `peer_address`, the session handle carries it, and `Netlayer` \
      gains a provided `new_outgoing_connection_from` so the two dial sites that *do* know — the \
      enlivener and the handoff greeter, both session-local objects — can say which peer asked. Both \
      are **provided** methods: the netlayer standard fixes two functions, and a transport that cannot \
      report an origin keeps the behaviour it had rather than becoming non-conforming. **What the model \
      does not claim:** `Origin` is two-valued where the port has a socket address and a list of \
      ranges, and `Target` is three-valued where the port has every `IpAddr` — the model says the shape \
      of the rule and the port says which addresses are in which class, pinned by its own tests. \
      **What the operator still decides:** whether local targets are refused at all is `deny_local`, \
      unchanged; the origin rule is not switchable, because it is the containment rather than a \
      preference." },
  { number := 63, clause := "a", layer := "Protocol",
    statement := "**A bridged delivery's consensus footprint is attributable to a payer** — the state a \
      session causes is one the peer that caused it pays for, and this node neither pays phlo it did \
      not choose to spend nor carries consensus state it cannot remove.",
    status := .open,
    declarations := [`Rchain.Payer, `Rchain.Write, `Rchain.Attributable, `Rchain.bridgedWrite, `Rchain.a_bridged_write_is_not_attributable, `Rchain.AttributableWrite, `Rchain.the_relay_would_satisfy_it, `Rchain.bounding_the_state_does_not_make_it_attributable],
    axioms := [],
    falsifiable := some "**the falsifier is the state as it is, and it is a theorem rather than a \
      paragraph about one.** `a_bridged_write_is_not_attributable` states that a write a session \
      caused has `thisNode` as its payer, for every size of write: every delivery to a \
      method-carrying chain capability runs `insertArbitrary` unconditionally and the deploy is \
      signed by the node's key, so the chain binds the node and the node's vault pays. \
      `bounding_the_state_does_not_make_it_attributable` is the sentence the row's *first* proposed \
      close condition got wrong: the law is about **who pays**, so a smaller write is the same \
      unattributable write — which is why the repair is the relay and not a bound. \
      `the_relay_would_satisfy_it` shows the law is not vacuous: a write whose payer is the caller and \
      whose state is within what the caller's phlo bought exists, which is what the relay produces.",
    witness := [`Rchain.a_bridged_write_is_not_attributable, `Rchain.the_relay_would_satisfy_it],
    note := "**Why this one is `open` and the other three laws of this family are not.** The OCapN \
      surface's *bounds* are already guards before the work — `MAX_SESSIONS`, the export and answer \
      tables' caps, `MAX_DEFERRED_ANSWERS`, the bridged-deploy rate limiter — and that is **Law 55's \
      clause a**, which is why C222's row now cites `55a` rather than proposing a law of its own \
      (rule 3: one home). What no bound reaches is *attribution*: the node cannot know whether a \
      reply holds a capability before registering it, because the value does not survive the \
      evaluation, so it cannot refuse the write on that ground. The close is the **relay** — the node \
      hands the peer the exact `DeployData` to sign with its own secp256k1 key, so the chain binds the \
      peer as `deployerId` and the peer's vault pays — and it cannot be landed here: Endo does not \
      speak this shard's `DeployData` shape, so it is a cross-implementation protocol change. The row \
      names that rather than a fix this tree cannot reach. **What the model does not claim:** the \
      state is a byte count and the payer is two-valued, so it says the *shape* of the obligation and \
      nothing about what a reply holds; the surviving growth is a chain-wide property of the registry \
      (no delete anywhere), bounded economically by whoever pays, and that property is the registry \
      layer's rather than this one's." },
  { number := 64, clause := "a", layer := "Wire",
    statement := "**Where the draft and the implementations differ, the port speaks the \
      implementations' reading.** The prose is not the oracle; what every peer actually speaks is.",
    status := .provedModel,
    declarations := [`Rchain.Source, `Rchain.Form, `Rchain.Disagreement, `Rchain.speaks, `Rchain.the_port_speaks_the_implementations_reading, `Rchain.the_draft_is_not_the_oracle],
    axioms := [],
    rust := ["ocapn/src/captp.rs", "ocapn/src/session.rs", "casper/src/shard_invoke.rs"],
    rustWitness := [ "ocapn/tests/reference_vectors.rs:start_session_matches_the_reference", "casper/src/shard_invoke.rs:reply_channel_is_the_deploy_id"],
    witness := [`Rchain.the_port_speaks_the_implementations_reading, `Rchain.the_draft_is_not_the_oracle],
    falsifiable := some "**the figures are the drafts', and they disagree.** AUDIT C216: the draft \
      defines `op:start-session` with **five** fields and contradicts itself about one of them \
      (`Ed25519_SHA256` when constructing, `Ed25519` when receiving), while the suite's \
      `OpStartSession` carries **four** — so a port following the prose fails against every \
      implementation. AUDIT C218: `invoke_term` passed the reply channel as a backticked \
      `` `rho:rchain:deployId` ``, which is a `GUri` *ground* — an ordinary, guessable name — where the \
      reference binds the unforgeable per-deploy channel; the port spoke a reading **no** \
      implementation produces, and every reply went nowhere while the deploy reported success. \
      `the_draft_is_not_the_oracle` is the clause as a theorem: the two readings differ, so speaking \
      one is not speaking the other. The tie is the known-answer corpus — \
      `ocapn/tests/reference_vectors.rs`'s vectors are produced by the suite's own \
      `contrib/syrup.py` encoder, and `start_session_matches_the_reference` pins the four-field form.",
    note := "**Both halves of this row are about the same mistake, made in opposite directions.** \
      Following the prose (C216) fails against the peers; inventing a reading the prose and the peers \
      both lack (C218) fails silently, because the wire does not complain about a name nobody reads. \
      The oracle is the **implementation**, which is what `ocapn/src/captp.rs`'s module doc states and \
      what the KATs enforce. **What the model does not claim:** a reading is a `Nat` where the port has \
      a record shape, so the model says *which* reading is spoken and nothing about its fields; the \
      divergences themselves are rows in the findings register (C216, C218, C224), which is where a \
      reader finds what each one is." },
  { number := 64, clause := "b", layer := "Wire",
    statement := "**Where the implementations disagree with each other, the port accepts every reading \
      they produce** — the union on the reading side, because there is no single reading to speak.",
    status := .provedModel,
    declarations := [`Rchain.accepts, `Rchain.every_reference_reading_is_accepted, `Rchain.oneReading, `Rchain.one_reading_refuses_another, `Rchain.the_union_accepts_it],
    axioms := [],
    rust := ["ocapn/src/bootstrap.rs", "ocapn/src/session.rs"],
    rustWitness := [ "ocapn/src/bootstrap.rs:bootstrap_deliver_accepts_a_swiss_number_as_bytes_or_as_a_string"],
    witness := [`Rchain.every_reference_reading_is_accepted],
    falsifiable := some "**the falsifier is the port's own history, stated as a rule.** `oneReading` is \
      what 'the reference implementation is the oracle' was taken to mean before it was noticed that \
      there are two — a port that speaks one reading — and `one_reading_refuses_another` is AUDIT C217 \
      exactly: `@endo/ocapn` sends a swiss number as a Syrup **String** (which is what the draft says \
      it is) and the Python suite sends a **byte array**, so a port built to either alone refuses the \
      other. `the_union_accepts_it` states the contrast on the same row, so the pair is a claim rather \
      than a preference. On the port, \
      `bootstrap_deliver_accepts_a_swiss_number_as_bytes_or_as_a_string` asserts both readings are \
      accepted, and C224's locator-hints case is the same shape one field over.",
    note := "**Why the reading side is a union and the speaking side is not.** A port has to *choose* \
      one shape to emit, and choosing the implementations' is right because they are what peers read; \
      it does not have to choose one to *accept*, and choosing would refuse a peer that is not wrong. \
      The asymmetry is the law's content, not an inconsistency. **What the model does not claim:** \
      that every divergence is tolerated — C217's is, and C224's four are settled readings the port \
      *pins* rather than accepts both ways; the register's rows say which is which. **What the law \
      cannot reach:** a third implementation with a third reading would be another row, and nothing in \
      this model finds it — the corpus is where a new reading shows up." },
  { number := 65, layer := "Rholang",
    statement := "**A value with no faithful literal is refused, not written.** Every literal the \
      printer writes reads back as the value it was written for, and a value that has no such literal \
      is refused rather than written as one that reads back as something else.",
    status := .provedModel,
    declarations := [`Rchain.Literal, `Rchain.Faithful, `Rchain.mayWrite, `Rchain.the_printer_writes_only_what_reads_back, `Rchain.an_unfaithful_literal_is_refused, `Rchain.theQuotedStringCase, `Rchain.the_quoted_string_case_is_refused],
    axioms := [],
    rust := ["rholang/src/pretty_printer.rs", "casper/src/shard_invoke.rs"],
    rustWitness := [ "casper/src/shard_invoke.rs:an_argument_that_would_close_its_literal_is_refused"],
    witness := [`Rchain.the_printer_writes_only_what_reads_back, `Rchain.the_quoted_string_case_is_refused],
    falsifiable := some "**the falsifier is a concrete literal, and it was written into signed terms.** \
      Rholang's literal grammar has **no escape**, so a string containing a quote has no faithful \
      literal at all: `theQuotedStringCase` is one — the printer meant seven units and a reader gets \
      the three before the quote — and `the_quoted_string_case_is_refused` says both that it is \
      unfaithful and that the guard refuses it. Two production paths printed values into terms they \
      then parsed, and **the values arrived from a peer** in the OCapN bridge's case \
      (`casper/src/shard_invoke.rs`'s builders), so what was signed was a body the peer chose. On the \
      port, `an_argument_that_would_close_its_literal_is_refused` is that case.",
    note := "**This is Law 59's clause c for a second encoder.** 59c says an encoder never emits a shape \
      outside its domain; here the domain is the literals that read back, and the encoder is the \
      printer rather than the bridge. It is a separate law rather than a clause of 59 because the \
      *domain* is different — a printable literal is a syntactic object, not a wire value — and because \
      the failing thing is different: 59's writes go to a peer, this one's go into a term the node's \
      own key signs. **What the model does not claim:** the values are `Nat`s standing for a value and \
      its read-back, so it says the *guard* and nothing about the encoding; that Rholang *could* have \
      an escape (it cannot — that is the grammar's choice, and it is why the answer is refusal rather \
      than quoting) is stated in the module doc, not modelled. **What the tie does not reach:** the \
      three printer warts Law 33 names are a different row's." },

  -- ---------------------------------------------------------------------------------------------
  -- The distributed-system assumptions: an atomicity model and a model of join (2026-10-10).
  -- These six rows are the first laws about *the node as a distributed system* rather than about
  -- rholang evaluation. Each was written refutation-first, and two of them are the refutation: Law
  -- 66's statement is false of the code, and that is the theorem. Two adversarial reviewers attacked
  -- all six; their verdicts are in each row's `note`, including the verdicts that cost the plan its
  -- chosen fix.
  -- ---------------------------------------------------------------------------------------------

  { number := 66, layer := "Casper",
    statement := "**The value a node stores at a fringe key is not determined by that key.** The record \
      at `fringe_hash_of(fringe)` holds a `state_hash` the key never mentions: the merge that produced \
      it started from a **base** (the previous fringe's state, or the base block's post-state), and two \
      writers whose merges started from different bases store different states under one key. The join's \
      `min` tie-break then silently settles it — and `the_join_rewrites_a_writers_value` is what that \
      costs: a writer that had computed the other value finds its own block failing its own validation \
      (C270, measured)",
    status := .provedModel,
    declarations := [`Rchain.the_state_is_named_by_a_block, `Rchain.a_disagreement_is_visible,
      `Rchain.claimsOf, `Rchain.the_fringe_only_key_is_incomplete,
      `Rchain.the_join_rewrites_a_writers_value],
    axioms := [],
    rust := ["models/src/block_metadata.rs", "casper/src/dag.rs",
      "casper/src/multi_parent_casper.rs"],
    witness := [`Rchain.the_state_is_named_by_a_block],
    falsifiable := "Two honest writers whose merges started from different bases, reaching one fringe: \
      their records share the key and disagree about `stateHash`. The measured instance is the four-key \
      divergence in `spec/audit/evidence/n-anchor-drill/self-reject-stall.txt`.",
    note := "**What this law is, precisely, and what the reviewers took away from it.** The statement is \
      an *impossibility about the design*: a key reading only the fringe cannot determine a value that \
      also depends on the base. It is **not** a re-derivation of the measured incident — the witness \
      (`recordOf ∅ 0` / `recordOf ∅ 1`) is an artefact of the model's abstract `mergeFringe f b := b` \
      in which the state *is* the base by construction, while the incident's pair had non-empty fringes. \
      **The positive half of that module is vacuous and is deliberately not cited**: \
      `the_composite_key_determines_the_value` and its join-side twins are tuple injectivity (`rfl`), \
      i.e. the statement that `recordOf` is a function; and `no_fringe_only_key_can_be_complete` is a \
      near-duplicate whose universal quantifier adds nothing (a remark, not a cell). **The refutation \
      that mattered was of the fix, not of the claim**: keying by `(fringe, base)` is *not implementable \
      at the reader that needs it*. `multi_parent_casper.rs:156-165` reads by the fringe-only key, and \
      the base it would need is computed *later* (`:230-238`) as the base block's post-state else the \
      previous fringe's state — the base **is** the value that read returns, so a reader asking for a \
      fringe's state cannot name which base the record it wants was written under. The design chosen \
      afterwards (2026-10-10) is that the **value drops the derived state**: the record keeps the \
      contributions and the reader derives, which makes the store append-only — the property Law 68 \
      proves is what makes a read a function of its version. **Delivered 2026-10-10, and the row now
      states what the code does.** The store, the type, the join and the tie-break have retired: the
      claim travels with the block that made it (`BlockMetadata.fringe_state_hash`, written once) and
      the readers derive. `the_state_is_named_by_a_block` is the version rule they rely on, and
      `a_disagreement_is_visible` is this refutation's positive counterpart, which is why the
      refutation is kept rather than deleted. The readers take the claim of the block that **finalised**
      the fringe — the parent whose own `fringe` field is that set — and not the claims of the fringe's
      members, which are one fringe earlier: the first cut read the members, and a live four-validator
      devnet stalled at height 20 with `active-bonds leaf not found in state` until the drill caught it.
      With the correction the same rig runs level and advancing — 218 heights in seven minutes, all four
      within two heights of each other, zero self-rejections, zero `active-bonds` failures
      (`spec/audit/evidence/n-anchor-drill/divergence-fixed.txt`; `passes.md` §96)." },

  { number := 67, layer := "Casper",
    statement := "**The record join is a commutative idempotent semigroup at one key, so the value at a \
      key is a function of the set of arriving records — and it is not a monoid.** `joinRecord` copies \
      `fringe_hash` and `fringe` from its left operand, so commutativity holds exactly when both \
      arguments are records for one key (the code's own stated precondition, `dag.rs:79-80`), and it has \
      no identity, which the model proves absent rather than fabricating. The sharpening is the one that \
      matters: order-independence does not come from the arrivals agreeing — `min` is commutative, so two \
      records that **disagree** still fold to one, which is why a divergence was invisible in the store",
    status := .retired,
    declarations := [`Rchain.joinRecord_assoc, `Rchain.joinRecord_comm, `Rchain.joinRecord_idempotent,
      `Rchain.joinRecord_fold_perm, `Rchain.the_joined_value_is_a_function_of_the_set,
      `Rchain.the_join_changes_nothing_iff_the_report_adds_nothing],
    axioms := [],
    rust := ["casper/src/dag.rs:82"],
    witness := [`Rchain.joinRecord_fold_perm],
    falsifiable := "A pair of records at one key whose folds in two orders differ — \
      `joinRecord_not_commutative` exhibits the failure for records at *different* keys, so the \
      `AtOneKey` hypothesis cannot be read as a convenience.",
    note := "**Verified faithful field by field** against `join_fringe_records` (left-biased key data, \
      unioned `fringeDiff` and rejection sets, `min` state) by a review that tried and failed to break \
      the laws, and confirmed `the_join_changes_nothing_iff_the_report_adds_nothing` has no missing \
      conjunct. **What is not cited: that module's `writeKey` section.** `writers_at_one_writeKey_agree` \
      and its four siblings are consequences of the tuple identity `writeKey f b = (f, b)`, as vacuous \
      as Law 66's positive half — and their premise, that a caller can *form* a `writeKey`, is exactly \
      what the review refuted. One honesty note the review added: `joinRecord_not_commutative`'s witness \
      violates the model's own `fringeHash = fringeHashOf fringe` invariant, so it is a non-vacuity \
      check and not a pair the store can hold. **RETIRED 2026-10-10 with the map it describes.** There
      is no record join left to be a semigroup: the claims are per block and the 'join' is a union of
      them. The row is kept, as the register's ``retired`` status intends, because its finding is the
      reason the store went — order-independence did *not* come from the arrivals agreeing, which is
      why a divergence could hide in the store — and because a reader of the old design deserves to
      find that argument where the join used to be." },

  { number := 68, layer := "Casper",
    statement := "**A read is a function of the version it names, and what a store must be for that to \
      hold is append-only, not content-addressed.** Two content-addressed reads of one version agree \
      because a hash collision is impossible (Law 10's `root_collision_free`); a store that only ever \
      appends has the same property without hashing at all. The record store today is neither: its \
      `put` and join **merge into an existing key**, and a read for a fringe's state can then return \
      either of two values",
    status := .provedModel,
    declarations := [`Rchain.two_content_addressed_reads_of_one_version_agree,
      `Rchain.a_read_of_an_append_only_store_is_stable,
      `Rchain.a_read_of_a_blocks_claim_is_stable,
      `Rchain.a_non_content_addressed_read_is_not_a_function_of_its_version],
    axioms := [],
    rust := ["rspace/src/history/radix_tree.rs", "models/src/block_metadata.rs",
      "tools/reconcile-network.sh"],
    witness := [`Rchain.a_read_of_an_append_only_store_is_stable],
    falsifiable := "Two stores that differ only in the base of the record at one key, read at that key: \
      different values for one version. The Rust instance is the `fringe-data` environment; the \
      operational one is the recovery tool's `--restore-from-master`, whose own text calls a copy of a \
      live store a torn snapshot.",
    note := "**The dichotomy is the finding.** The positive half is a composition over Law 10 and adds \
      no new mathematics; the refutations restate Law 66's collision in store vocabulary, which the \
      review called duplicative and which is registered here as **clauses in substance rather than a \
      second law** — the declaration cited is the append-only half, the part that is new. The general \
      theorems (`read_is_a_function_of_its_version`, `read_reads_by_version`) are `rfl` unpackings of \
      `read` and are **not cited**. **And the row corrects the tool's own framing**: a torn copy of a \
      *content-addressed* store cannot violate a read for any key it holds whole, so the fiat is needed \
      only for the non-append-only half. That module's header claim — that the composite key would bring \
      the record store into the safe class — is unproved and, as Law 66 records, inexpressible; the \
      chosen design gets there another way. **Since 2026-10-10 it holds for the fringe state**, which
      is the point of the redesign: a claim is written once, at the block that made it, so
      `a_read_of_a_blocks_claim_is_stable` is this law's positive half instantiated at the claim store
      rather than a new law. The tool's fiat still applies to the chain-state *directories*, which are
      not where the claims live." },

  { number := 69, layer := "Sync",
    statement := "**A joiner's state-page walk need not terminate, and with a give-up rule it is paced.** \
      As written, the walk's request loop exits only on an error or `is_finished`, so a peer that simply \
      never answers leaves an infinite run — an infinite *trace*, not the absence of reachability, \
      because the walk can finish and need not. Add the idle give-up the block leg already has and every \
      turn spends fuel: no infinite run, and a bounded exit",
    status := .provedModel,
    declarations := [`Rchain.Sync.Walk.the_page_walk_as_written_spins_for_ever,
      `Rchain.Sync.Walk.the_fixed_walk_cannot_run_for_ever,
      `Rchain.Sync.Walk.the_page_walk_with_the_give_up_rule_is_paced],
    axioms := [],
    rust := ["casper/src/engine/lfs_tuple_space_requester.rs", "casper/src/engine/lfs_block_requester.rs",
      "casper/src/engine/node_syncing.rs"],
    witness := [`Rchain.Sync.Walk.the_page_walk_as_written_spins_for_ever],
    falsifiable := "A run of the machine as written that never reaches `is_finished` — and the adversary \
      need not answer unsolicited keys: the same constant trace is an infinite run using only the \
      request loop's resend, which the Rust does with no bound. The measured stall is C268's stuck \
      joiner. `ReachesTheGoal` is left as an unproved `Prop`: reaching the goal needs the *peer* to \
      answer, which a give-up rule cannot supply.",
    note := "**The review strengthened this one.** The falsifier looks as if it depends on an adversarial \
      environment answering a key the walk never requested; a reviewer compiled the variant that uses \
      only the resend, so it needs nothing but a silent peer — the machine the Rust is in, with no \
      bound. The modelled `accept` also makes the stall condition precise: the model cannot represent \
      \"answered the requested keys and still unfinished\", so the stall is exactly the open C268 \
      question, whether the cursor advances in a run that does not finish. `MAX_IDLE_ROUNDS = 3` matches \
      the block leg — and **the page walk's own timeout is 120 s, not the 30 s that constant's rationale \
      assumes**, so a copied value means six minutes of silence. **Two Rust defects were found while \
      proving this** and are filed as their own rows: that timeout mismatch, and an unfinished walk \
      returned as `Ok`." },

  { number := 70, layer := "Sync",
    statement := "**Importing the walk of a root yields exactly the trie rooted there, and the checker \
      is sound.** The walk delivers the reachable nodes however the pages are cut; a page-wise import \
      whose pages pass their checks reads at the root what the peer reads there. The joiner is not \
      trusting the peer, it is checking it — and the whole of that rests on one assumption, \
      `blake2b256_collision_free`, through Law 10",
    status := .provedModel,
    declarations := [`Rchain.Sync.import_reads_the_rooted_trie, `Rchain.Sync.reads_determined,
      `Rchain.Sync.WalkListing.pages_are_the_nodes],
    axioms := [],
    rust := ["rspace/src/state/mod.rs", "rspace/src/history/export.rs",
      "rspace/src/history/radix_tree.rs"],
    witness := [`Rchain.Sync.import_reads_the_rooted_trie],
    falsifiable := "Two stores that agree on the reachable set and read differently at the root: \
      impossible, which is `reads_determined`; or a page whose leaves do not hash to their keys, which \
      `validate_state_items` refuses.",
    note := "**Survives attack**, with the gap named: the model's checker is deliberately **weaker** than \
      `validate_state_items` — it keeps key-honesty and coverage and drops the Rust's key-*sequence* \
      equality, its boundedness against extras, and the data (leaf-value) store. That is the right \
      direction: the Rust's stronger check implies the model's hypotheses, so soundness is not \
      compromised, and the one unproved bridge (per-chunk sequence equality implies whole-walk coverage) \
      is prose, not a theorem. `reads_determined` does not itself invoke `root_collision_free` — the \
      axiom is consumed in `import_agrees`." },

  { number := 71, layer := "Sync",
    statement := "**A catch-up window bounds what a joining node holds, so its progress does not depend \
      on the network standing still.** The frontier is monotone along any run, stalls included; an \
      answered window advances it one width; and the pending set is a function of the *window* and the \
      frontier — never of how far above it the peer's tip has run. The hash-keyed downward walk has no \
      such property: what it must hold is the whole gap",
    status := .provedModel,
    declarations := [`Rchain.Sync.run_monotone, `Rchain.Sync.answered_run_reaches,
      `Rchain.Sync.windowPending_independent_of_tip, `Rchain.Sync.gapPending_exceeds_the_window],
    axioms := [],
    rust := ["casper/src/engine/catchup.rs", "casper/src/engine/node_running.rs"],
    witness := [`Rchain.Sync.run_monotone],
    falsifiable := "A run that lowers the frontier, or a tip above it that changes what is pending. The \
      measured instances of the failures this replaces are `run-2-catchup-stall.txt` (frozen at 121, 173 \
      drops) against `run-3-windowed-catchup.txt` (the same 1200-block gap closed in 98 s, zero drops).",
    note := "**Two corrections the review made.** The module **cited the wrong row** in its header — C268 \
      where the catch-up is C267; the rows were split after it was written, and the citation is fixed in \
      this pass. And `windowPending` counts **heights** while `MAX_PENDING_BLOCKS` counts **blocks**: the \
      bridge (a window of eight heights is at most 32 blocks on a four-validator shard) is prose in the \
      module, not a theorem, so the headline is a *proxy* for the bound the receiver needs, and this cell \
      says so. The step's terminal condition is an **idealisation** — the Rust frontier is the local \
      DAG's and can exceed a peer's advertised tip — so the bound statements describe a narrower machine \
      than the Rust; the window arithmetic itself was verified correct and faithful to `catchup.rs`." },

  { number := 72, layer := "Casper",
    statement := "**A completion record brackets every durable effect.** The reset's marker may be \
      written only once *every* effect it brackets has been applied — the DAG rewind, the body purge, \
      the runtime's durable root, **and the cleanup of the derived deploy and deployer indexes**. No \
      reachable state has the record written with an effect missing, and the boot repair re-applies the \
      whole set from any half-applied state",
    status := .provedModel,
    declarations := [`Rchain.ResetState, `Rchain.resetStep, `Rchain.skippedIndexStep,
      `Rchain.resetSystem, `Rchain.a_rewound_record_implies_every_effect_applied],
    axioms := [],
    rust := ["casper/src/engine/reset.rs", "casper/src/dag.rs",
      "rspace/src/history/history_repository.rs"],
    rustWitness := ["casper/tests/reset.rs:a_crash_after_the_rewind_is_repaired_at_boot_by_the_marker",
      "casper/tests/reset.rs:the_guard_refuses_a_rewinding_whose_anchor_body_is_absent"],
    witness := [`Rchain.an_early_commit_reaches_a_record_that_lies,
      `Rchain.a_skipped_index_cleanup_reaches_a_record_that_lies,
      `Rchain.a_reset_can_complete],
    falsifiable := "Two of the port's own orders. `earlyCommitStep` records the completion after the \
      rewind and the purge but before the runtime root; `skippedIndexStep` records it with the \
      derived-index effect optional — the early return in `drop_above` when the metadata drop is empty. \
      Each exhibits a reachable `rewound` record with an effect missing, so the law was red on the code \
      as it stood, and `a_reset_can_complete` keeps it from being vacuous.",
    note := "**Why the record must bracket all four effects.** `rewind_and_purge` writes the marker when \
      the first two have returned (`casper/src/engine/reset.rs:81`), the runtime root is written \
      *afterwards* through `HistoryRepository::reset` → `validate_and_set_current_root` \
      (`rspace/src/history/history_repository.rs:337`), and the derived indexes are cleaned by \
      `BlockDagKeyValueStorage::drop_above` (`casper/src/dag.rs:410`). A crash before any of them \
      leaves the record claiming the stores agree while one does not, and the boot guard reading \
      `rewound` re-applies nothing (`reset.rs:140-151`). `reset_volatile`'s doc \
      (`history_repository.rs:346-358`) names the same class for the merge. HAZOP rows F-U13-01/02 and \
      **F-U17-11** (docs/src/spec/failure-hazop.md): the root half landed earlier; the index half is \
      B39, and its rule is that stale rows are derived from the **surviving** block set, not from the \
      list the current call happened to drop." },

  { number := 73, layer := "Casper",
    statement := "**A raised request is not spent until the act commits.** The self-reset's request cell \
      is cleared by, and only by, the step that applies the reset's effects: no reachable state has the \
      request taken with nothing applied, so a raised request is always still actionable",
    status := .provedModel,
    declarations := [`Rchain.RequestState, `Rchain.spendRequestStep, `Rchain.spendSystem,
      `Rchain.a_request_is_not_spent_without_the_act],
    axioms := [],
    rust := ["casper/src/engine/reset.rs"],
    rustWitness := ["casper/tests/reset.rs:a_reset_request_rewinds_the_stores_when_the_switch_is_on_and_does_nothing_when_it_is_off"],
    witness := [`Rchain.a_split_take_reaches_a_state_that_spent_the_request,
      `Rchain.a_raised_request_can_be_acted_on],
    falsifiable := "The port's split: taking the request and applying the effects in two statements. \
      `a_split_take_reaches_a_state_that_spent_the_request` exhibits the reachable state that admits — \
      the cell empty, nothing applied, and nothing to re-raise it in-process — and \
      `a_raised_request_can_be_acted_on` keeps the law from being vacuous.",
    note := "**The port takes first and acts second.** `act_on_reset_request` calls `take_reset_request()` \
      as its opening statement (`casper/src/engine/reset.rs:215`) and only then runs `perform_reset`, \
      which can fail before touching a store (the anchor body absent, `:246-258`). A failure there spends \
      the request with nothing done; recovery is a restart, re-derivable only because the detector's \
      count (`restore_attempts`) is persisted. HAZOP row F-U11-03 (docs/src/spec/failure-hazop.md). \
      **Fixed 2026-10-13**: `act_on_reset_request` peeks (`reset_request()`) and runs the anchor \
      pre-flight *before* the take, so a failure that destroyed nothing leaves the request pending — \
      witnessed by the reset test named above; the switch-off arm still consumes, because that is a \
      terminal configuration rather than a transient fault." },

  { number := 74, layer := "RSpace",
    statement := "**A checkpoint does not drain an overlay it has not folded.** The hot-store and native \
      overlays are folded into the trie before they are cleared, so an interrupted checkpoint loses \
      nothing it has not committed — no reachable state has an overlay empty while its changes are \
      unfolded",
    status := .provedModel,
    declarations := [`Rchain.CheckpointState, `Rchain.checkpointStep, `Rchain.checkpointSystem,
      `Rchain.a_checkpoint_does_not_drain_what_it_has_not_folded],
    axioms := [],
    rust := ["rspace/src/rspace.rs", "rspace/src/native_store.rs"],
    witness := [`Rchain.a_drain_before_the_fold_reaches_a_state_that_lost_the_overlay,
      `Rchain.a_checkpoint_can_complete],
    falsifiable := "The port's order, clearing the native overlay before the fold. \
      `a_drain_before_the_fold_reaches_a_state_that_lost_the_overlay` exhibits the state it admits — the \
      overlay empty, the changes unfolded — and `a_checkpoint_can_complete` keeps the law from being \
      vacuous.",
    note := "**The two overlays are not treated alike.** `create_checkpoint` reads the hot-store changes \
      (`rspace/src/rspace.rs:604-605`, a snapshot that is not cleared) and clears the native overlay at \
      `:606-609` (whose `drain_native` ends in `overlay.clear()`, `rspace/src/native_store.rs:477`) \
      *before* the only fallible step, `checkpoint_with_native` (`:616-621`). A fold that returns `Err` \
      therefore loses the native mutations and records the failed drain in `last_native_*`, while the \
      hot-store side is untouched. The model is one overlay because the property is per-overlay — \
      applying it to both is what \"both overlays or neither\" means. HAZOP rows F-U15-01/02 \
      (docs/src/spec/failure-hazop.md). **Fixed 2026-10-13**: `create_checkpoint` snapshots the native \
      overlay before the drain and restores it if the fold fails, and writes `last_native_*` only after \
      the fold has committed. `rustWitness` is empty: the fold's failure path has no fault-injection \
      point in the rspace tests yet, so that falsifier is owed." },

  { number := 75, layer := "Casper",
    statement := "**A dependent action is taken only on a committed record.** The refusal budget's \
      increment and the reset request it can raise are ordered: no reachable state has the request raised \
      while the record it rests on is uncommitted, so the request's premise is always durable",
    status := .provedModel,
    declarations := [`Rchain.AttemptState, `Rchain.attemptStep, `Rchain.attemptSystem,
      `Rchain.a_dependent_action_is_taken_only_on_a_committed_record],
    axioms := [],
    rust := ["casper/src/multi_parent_casper.rs"],
    witness := [`Rchain.a_raised_request_on_an_uncommitted_record_is_reachable,
      `Rchain.a_committed_record_can_gate_an_action],
    falsifiable := "The port's order, raising the request on the uncommitted record. \
      `a_raised_request_on_an_uncommitted_record_is_reachable` exhibits the state it admits — the request \
      raised, the record it rests on uncommitted — and `a_committed_record_can_gate_an_action` keeps the \
      law from being vacuous.",
    note := "**The request runs on the value the store has not yet accepted.** `revalidated_record` builds \
      the incremented `BlockMetadata` and `note_unrestorable_if_exhausted` — which raises the request — \
      runs on it before the `update_metadata` that persists it (`casper/src/multi_parent_casper.rs:796` \
      and `:1202` precede `:797` and `:1214`). The node's read-modify-write half of the same node \
      (F-U14-01) is the shape law 73 states and is not modelled twice. HAZOP rows F-U14-01/02 \
      (docs/src/spec/failure-hazop.md). **Fixed 2026-10-13**: `note_unrestorable_if_exhausted` now runs \
      *after* the `update_metadata` that persists the count, so a request raised by it names a count the \
      store holds. `rustWitness` is owed — the write's failure path has no fault-injection point here." },

  { number := 76, layer := "Casper",
    statement := "**A concluded stop stops.** Once a fault the node cannot serve through is reported, \
      the node is not serving and cannot return to serving: the report and the stop are one step, and it \
      is the last step available",
    status := .provedModel,
    declarations := [`Rchain.ServeState, `Rchain.serveStep, `Rchain.serveSystem,
      `Rchain.a_reported_fault_stops_the_node],
    axioms := [],
    rust := ["node/src/runtime/shutdown.rs", "node/src/main.rs",
      "node/src/runtime/node_runtime.rs"],
    rustWitness := ["node/src/runtime/shutdown.rs:a_reported_fatal_resolves_the_wait_with_the_first_reason"],
    witness := [`Rchain.a_note_without_a_stop_leaves_the_node_serving,
      `Rchain.a_reported_fault_reaches_a_stopped_state],
    falsifiable := "The port's old shape, where reporting the fault leaves the node serving. \
      `a_note_without_a_stop_leaves_the_node_serving` exhibits the reachable state it admits — the latch \
      set, the listeners up — and `a_reported_fault_reaches_a_stopped_state` keeps the law from being \
      vacuous.",
    note := "**Two statements, one gap.** The self-reset's failed act and the LFS sync terminal both say \
      \"this node is stopping\", and both were a `return Err` from a task nobody joins: the launched \
      `node_launch` future's error was only logged and `NodeProgram::serve` never awaited it, so the \
      receiver, processor, proposer and API kept serving a node whose stores may have moved under a view \
      that was not rebuilt. **Fixed 2026-10-13**: `shutdown::report_fatal` sets a latch the spawn wrapper \
      writes and `main`'s wait selects on, stopping the process without draining. HAZOP row F-U12-03 \
      (docs/src/spec/failure-hazop.md)." },
  { number := 77, layer := "RSpace",
    statement := "**A reset is a projection onto its root.** The reset keeps the root it names and drops \
      the overlay, so an install made *before* a reset to that root is **unobservable** after it — and \
      the read path's construction, which installs the system processes, resets to the block's root and \
      installs them again, is therefore the **same function** as the one that resets and installs once",
    status := .provedModel,
    declarations := [`Rchain.StoreView, `Rchain.installInto, `Rchain.resetTo, `Rchain.foldingResetTo,
      `Rchain.installAll, `Rchain.readPathAsWritten, `Rchain.readPathDerived,
      `Rchain.the_first_pass_is_erased, `Rchain.an_install_before_a_reset_is_unobservable],
    axioms := [],
    rust := ["rholang/src/runtime.rs", "rspace/src/rspace.rs", "casper/src/runtime_manager.rs"],
    rustWitness := ["casper/tests/exploratory_reply.rs:a_read_makes_no_reset"],
    witness := [`Rchain.a_folding_reset_keeps_the_first_pass, `Rchain.the_two_reset_rules_differ],
    falsifiable := "The shape a *folding* reset would have — and it is not a straw man: folding an \
      overlay in rather than dropping it is what a **checkpoint** does, and Law 74 is the law that it \
      must fold what it drains. Under that rule the same pre-reset install **is** observable \
      (`a_folding_reset_keeps_the_first_pass`), so `the_first_pass_is_erased` is a claim about the \
      reset's rule rather than a restatement of a container. `the_two_reset_rules_differ` keeps it from \
      being vacuous: the two rules are distinguishable on a store with one definition installed.",
    note := "**Why this law exists, and what it is for.** The read path forks a runtime at the last \
      finalised block's post-state, `build_runtime_core` installs the 31 system-process definitions into \
      the store it builds (`rholang/src/runtime.rs:175`), and then `RhoRuntime::reset` **replaces** that \
      store (`ISpace::reset` assigns a fresh `InMemHotStore`, `rspace/src/rspace.rs:666`) and installs \
      all 31 again (`rholang/src/runtime.rs:447`). One read therefore pays the install pass twice and \
      one of them lands in a store that is discarded — 62 definitions installed to keep 31. The code \
      said so in a *comment* (\"the reset replaces the hot store, dropping the in-memory system-process \
      installs (they are not checkpointed)\"), and a fix chosen from a comment is a fix chosen from \
      whoever wrote the comment. This row makes the same content a law, so that removing the first pass \
      is **derived** — the two constructions are one function, so the change cannot alter any answer, and \
      the derivation is what the measurement is measured against (`casper::runtime_manager::\
      read_path_census`; issue #364). **What it does not claim**: nothing here is about cost. The law \
      says the constructions answer alike; it does not say keeping the pass is cheap, and only the \
      census says what removing it buys." },
  { number := 78, layer := "Casper",
    statement := "**The genesis carries a developer allocation, derived from the total the wallets \
      file allocates.** One allocation that no wallets file contains is credited at genesis — to a \
      **vesting account**, not a spendable vault: *when* it becomes spendable is Law 79 — of \
      `max(20 000 000 REV, 2% of the allocated total)`, so below a one-billion-REV allocation the \
      floor governs and at or above it the rate does, and the creator of the genesis and a node \
      replaying it derive the same amount from the same file",
    status := .provedModel,
    declarations := [`Rchain.devTaxAmount, `Rchain.devTaxFloor, `Rchain.devTaxBps,
      `Rchain.devTaxCrossover, `Rchain.devTaxAmount_ge_floor,
      `Rchain.devTaxAmount_eq_floor_of_le_crossover,
      `Rchain.devTaxAmount_eq_percentage_of_crossover_le,
      `Rchain.devTaxAmount_of_the_published_allocation],
    axioms := [],
    rust := ["casper/src/genesis/dev_tax.rs", "casper/src/genesis/mod.rs",
      "casper/src/engine/node_launch.rs", "node/src/configuration/configuration.rs"],
    rustWitness := ["casper/src/genesis/dev_tax.rs:the_floor_governs_below_a_billion_rev_and_two_percent_above"],
    witness := [`Rchain.the_rate_strictly_exceeds_the_floor_above_the_crossover,
      `Rchain.devTaxAmount_monotone],
    falsifiable := "Each arm is a theorem, and they are the falsifiers: \
      `devTaxAmount_eq_floor_of_le_crossover` says an allocation at or below one billion REV takes the \
      floor, `devTaxAmount_eq_percentage_of_crossover_le` says one above it takes the rate, and the \
      two would contradict each other if the rule were mis-stated. \
      `the_rate_strictly_exceeds_the_floor_above_the_crossover` is the non-vacuity witness — without \
      it both directions would hold of any constant — and \
      `devTaxAmount_of_the_published_allocation` pins the arm this network is on (908 962 714 REV \
      allocated, so the floor).",
    note := "**A deliberate deviation, registered as one.** The Scala ceremony installs exactly the \
      vaults its wallets file names; this port derives one more from their total, so the row is \
      `provedModel` — there is no conformance corpus for a rule the oracle does not have — and the \
      deviation is recorded in `spec/audit/passes.md` §139 with its own finding row. Three things the \
      law is *for*: the balance is installed outside the genesis block's deploys and content-addressed \
      into its post-state hash, so the arithmetic has to be one function of a file that a creator and \
      a joiner both read (AUDIT C46); only the *address* is a setting, never the amount; and the rule \
      has a home other than the comment that would otherwise be its only one. **What it does not \
      claim**: nothing here is about the allocation being owed, just, or final — those are the \
      decision this row records, taken 2026-10-16, and the arithmetic is the only part a machine can \
      check." },
  { number := 79, layer := "Casper",
    statement := "**The developer allocation unlocks on a schedule, and never runs ahead of it.** The \
      genesis credits it to a vesting account rather than a vault, and each epoch boundary moves what \
      has vested into the recipient's own vault: the vested fraction is the schedule position, linear \
      in the height, zero at genesis and total at the end, and a duration of `0` is *no schedule* \
      rather than a zero-length one",
    status := .provedModel,
    declarations := [`Rchain.devTaxVested, `Rchain.devTaxVested_zero_duration,
      `Rchain.devTaxVested_at_genesis, `Rchain.devTaxVested_at_end,
      `Rchain.devTaxVested_le_allocation, `Rchain.devTaxVested_monotone],
    axioms := [],
    rust := ["rholang/src/native_state.rs", "casper/src/genesis/mod.rs",
      "node/src/configuration/defaults.conf"],
    rustWitness := ["rholang/src/native_state.rs:the_dev_allocation_vests_linearly_and_releases_at_boundaries"],
    witness := [`Rchain.devTaxVested_le_allocation, `Rchain.devTaxVested_monotone,
      `Rchain.the_schedule_is_not_a_constant, `Rchain.the_schedule_truncates],
    falsifiable := "The bound and the monotonicity are the falsifiers, and they are what the release \
      depends on: the release moves `vested − released`, so `devTaxVested_le_allocation` is what keeps \
      the recipient's vault bounded by the allocation however long the chain runs, and \
      `devTaxVested_monotone` is what keeps that difference non-negative. \
      `the_schedule_is_not_a_constant` is the non-vacuity witness — without it every theorem here \
      would hold of `fun _ => allocation` — and `the_schedule_truncates` pins the direction of the \
      rounding, which is toward the network rather than the recipient.",
    note := "**Why this row exists, and what it is for.** Law 78 fixes *how much* the genesis owes; on \
      its own that is an allocation credited to a vault at genesis and therefore spendable at genesis — \
      an immediate claim on the whole amount. This row is the schedule that makes it not that. The \
      allocation is installed into a **vesting account** (`pos:dev-vesting`, holding the recipient, the \
      total and the unreleased balance in one record) by the same `install_genesis` both the ceremony \
      and a replaying node call, and `close_block` releases at epoch boundaries — last, and \
      deliberately independent of the reward steps, because it debits that account and credits a *user* \
      vault and so can reach neither the staking vault nor the pot. **The term is a genesis \
      parameter, not a constant** (`dev-tax-vesting-blocks`, 36 months at the cadence \
      `defaults.conf` states): the arithmetic is this row's, the term is the deal's, and the block \
      cadence is not a constant in this tree (`docs/src/node/scaling.md`), so a month is an assumption \
      the parameter makes visible. **What it does not claim**: nothing here is about the term being the \
      right one. **And the granularity is the epoch, not the block** — native state is written at \
      boundaries — which is stated because a reader comparing the schedule to a balance needs to know \
      which clock the balance follows." },

  { number := 80, layer := "Casper",
    statement := "**`CURRENT_ROOT` names only an accepted block.** A checkpoint may record a candidate \
      post-state as a *known* root, but the store's current-root pointer moves only when a block whose \
      post-state is that root is accepted into the DAG — so no reachable state has the pointer naming a \
      state no accepted block references, and a boot republishes only an accepted root",
    status := .provedModel,
    declarations := [`Rchain.PubState, `Rchain.pubStep, `Rchain.pubSystem,
      `Rchain.a_current_root_is_an_accepted_state, `Rchain.a_boot_republishes_only_an_accepted_root],
    axioms := [],
    rust := ["rspace/src/history/roots_store.rs", "rspace/src/history/history_repository.rs",
      "casper/src/runtime_replay.rs", "casper/src/blocks/block_processor.rs"],
    rustWitness := [],
    witness := [`Rchain.an_early_publish_reaches_an_orphan_root, `Rchain.publication_can_advance],
    falsifiable := "The port's order before the deferred publish: the replay checkpoint records the \
      candidate root and moves `CURRENT_ROOT` before `dag.insert` decides. \
      `an_early_publish_reaches_an_orphan_root` exhibits the reachable state that order admits — a \
      current root no accepted block references — so the law was red on the code as it stood, and \
      `publication_can_advance` keeps it from being vacuous.",
    note := "**Why recording a root is not publishing it.** Validation replays the block's deploys, \
      writes a checkpoint and the root tag, and only then calls `dag.insert` \
      (`casper/src/blocks/block_processor.rs:118`); the replay checkpoint reaches \
      `RootsStore::record_root` (`casper/src/runtime_replay.rs:302-306` → \
      `rspace/src/history/history_repository.rs:317`), which writes the root tag and `CURRENT_ROOT` in \
      one LMDB transaction. A block refused for a bad pre-state hash or dropped by a rewind therefore \
      leaves the pointer naming a post-state no accepted block carries — a durable pointer that is not \
      a function of the chain the node runs. The fix (B40, HAZOP F-U17-10) splits the write: candidate \
      checkpoints record the **known** root only, and the publish runs after acceptance at the insert \
      sites, with a non-decreasing-height guard and a boot reconciliation that republishes the latest \
      accepted root when it is known. `reset`'s `validate_and_set_current_root` keeps publishing: its \
      root is an accepted ancestor's. **What it does not claim**: the cross-store window between the \
      insert and the publish is narrowed, not closed — there is no WAL — and a crash there is repaired \
      at boot, best-effort if the root is unknown. `rustWitness` is owed until the deferred-publish \
      tests land." },

  { number := 81, layer := "Sync",
    statement := "**A join walk accepts a verified answer for any known key, and re-asks every \
      outstanding key.** The block walk's key can reach `Received` without being finished — an answer \
      that arrived before its request turn is dropped, and a body that fails its content check has \
      already consumed the key. As written, no request selects `Received` and no answer is accepted \
      for it, so the key is stranded and `is_finished` can never hold. The repair accepts an answer \
      for a known key whatever the request slot, validates before consuming, and re-requests `Init`, \
      `Requested` and `Received` on a resend; one accepted answer per round raises the finished \
      measure, which is bounded by the walk's key universe",
    status := .provedModel,
    declarations := [`Rchain.Sync.Join.accepted_answer_is_verified,
      `Rchain.Sync.Join.every_outstanding_key_is_requested_on_a_resend,
      `Rchain.Sync.Join.a_round_makes_progress,
      `Rchain.Sync.Join.the_written_walk_cannot_finish_from_a_stranded_key],
    axioms := [],
    rust := ["casper/src/engine/mod.rs", "casper/src/engine/lfs_block_requester.rs"],
    rustWitness := [
      "casper/src/engine/mod.rs:resend_re_requests_a_received_key",
      "casper/src/engine/mod.rs:received_accepts_a_known_key_before_its_request_turn",
      "casper/src/engine/mod.rs:received_still_rejects_an_unknown_key",
      "casper/src/engine/lfs_block_requester.rs:a_forged_body_does_not_consume_the_key_and_is_re_requested",
      "casper/src/engine/lfs_block_requester.rs:a_known_needed_block_is_accepted_before_its_request_is_sent",
      "casper/src/engine/lfs_block_requester.rs:a_mature_chain_join_completes",
      "block-storage/src/dag/codecs.rs:a_decoded_block_still_hashes_to_its_hash"],
    witness := [`Rchain.Sync.Join.the_written_walk_strands_a_received_key,
      `Rchain.Sync.Join.the_fixed_walk_can_finish_from_a_stranded_key,
      `Rchain.Sync.Join.a_rejected_body_leaves_the_walk_where_it_was],
    falsifiable := "A walk state with one `Received` key still in `latest`: \
      `the_written_walk_cannot_finish_from_a_stranded_key` shows every `WrittenStep` leaves it \
      `Received`, so no run reaches `isFinished` — the `(3 finished, 25 outstanding)` shape. The same \
      counts appeared in the live join, but a controlled A/B (2026-10-25) found that instance was inbound \
      reachability, not this defect; the stranded key is what the code admits and what this row fixes. \
      The same state reaches the goal in two turns of the repaired machine \
      (`the_fixed_walk_can_finish_from_a_stranded_key`), so the falsifier is not vacuous and the \
      repair is not a widening of the accepted set to unknown keys: `Step.unknown` still drops them.",
    note := "**Why the missing transition was not the whole defect.** The Rust consumes before it \
      validates (`lfs_block_requester.rs:73-108` calls `received` and only then checks the hash), so a \
      body that fails the check leaves the key `Received`; and `get_next` re-requests only `Init`, or \
      `Requested` on a resend (`mod.rs:102-114`). The model keeps both facts as `WrittenStep`, which is \
      what makes the falsifier the code's own shape rather than an invented one. The repaired machine's \
      fairness hypothesis is explicit: `a_round_makes_progress` needs a verified answer to exist \
      (`hanswer`) and the key universe to be finite and closed (`hK`); the Rust supplies the closure \
      through the fringe's ancestry but this module does not prove the DAG finite. **Law 69 is not \
      replaced**: it models the tuple-space page walk, whose validation failure aborts the attempt and \
      so has no silent stranded key. The witnesses are the 28-block `request_blocks` test and the \
      requester's rejection/early-answer tests; the codec round-trip witness rules out a wire cause \
      of a systematic hash mismatch." },

  { number := 82, layer := "PoS",
    statement := "**A validator's lifecycle has six transitions, and trust is the admission one.** \
      `observer → trusted → bonded → active → withdrawing → claimable`, with `removed` as the model's \
      reading of the Rust's absence: `trust`/`untrust` move the admission list (a trusted caller, \
      never a self-revocation), `bond` is self-only and requires trust and leaves the active set \
      alone, `withdraw` stages without moving a coin, the epoch boundary is the only step that can \
      *add* to the active set, and a slash needs evidence. Revocation is not confiscation: `untrust` \
      changes no stake ledger, so an untrusted-but-bonded validator stays in consensus",
    status := .provedModel,
    declarations := [`Rchain.Lifecycle, `Rchain.LifecycleStep, `Rchain.phaseOf,
      `Rchain.trust_only_from_a_trusted_key, `Rchain.untrust_leaves_the_bond_alone,
      `Rchain.the_trusted_set_never_empties, `Rchain.bond_is_self_only,
      `Rchain.activation_only_at_a_boundary],
    axioms := [],
    rust := ["rholang/src/native_state.rs", "rholang/src/system_processes.rs",
      "casper/src/genesis/mod.rs"],
    rustWitness := [
      "rholang/src/native_state.rs:trust_requires_a_trusted_caller",
      "rholang/src/native_state.rs:untrust_removes_trust_and_leaves_the_bond_alone",
      "rholang/src/native_state.rs:untrust_does_not_change_the_active_set",
      "rholang/src/native_state.rs:bond_escrows_the_stake_and_activates_at_the_boundary",
      "rholang/src/native_state.rs:the_epoch_gate_does_nothing_off_a_boundary",
      "rholang/src/native_state.rs:a_validator_with_delegations_cannot_withdraw",
      "rholang/src/native_state.rs:the_boundary_pays_on_the_participation_it_is_handed"],
    witness := [`Rchain.untrust_does_not_deactivate, `Rchain.the_bond_step_does_not_activate,
      `Rchain.a_justified_slash_is_a_step, `Rchain.a_bond_reaches_the_bonded_phase],
    falsifiable := "The rule this replaces is C111's: `untrust` called `slash` on a bonded target, so \
      one trusted signature moved another validator's entire stake to the Coop vault. In the model that \
      rule would be a second `doUntrust` writing `pos`, and `untrust_leaves_the_bond_alone` would be \
      unprovable by `rfl` — the theorem is the state equality itself, not a description of it. \
      `the_trusted_set_never_empties` excludes the other shape: a caller revoking itself out of the \
      admission list. `activation_only_at_a_boundary` excludes activation by `bond`.",
    note := "**The audit found the code matches the law, with one contradiction that is a comment.** \
      `trust`/`untrust` bind the caller to the unforgeable `deployerId` (`system_processes.rs:2043-2063`), \
      `bond`/`withdraw` bind the actor to the deployer (`:1852-1901`), `untrust` does not confiscate \
      (C111), and activation is `select_active` at a boundary. The stale module header at \
      `rholang/src/native_state.rs:25` says `slash`/`untrust` 'remove the validator and confiscate the \
      stake', which the code and C111 contradict; the comment is fixed, the semantics are not. \
      **What the model does not claim**: `Validator` is an id and the vault is one total, so the \
      per-vault debit of a bond is the Rust's property and not a theorem here; the Rust's min/max bond \
      bounds are policy outside the state machine; and `retired` is model-only — the Rust observes \
      removal as absence. `untrust` leaving an active validator active is deliberate and is named as a \
      non-claim of Law 83's trust model. The witnesses are the lifecycle's own tests: the caller \
      gate, revocation without confiscation, revocation without deactivation, the bond/boundary \
      split, the epoch gate, the delegation guard and the boundary payout." },

  { number := 83, layer := "Sync",
    statement := "**The join is content-trustless, not finality-trustless.** Every join input is \
      genesis-anchored (the configured genesis block/state and its `bond keys ∪ pos-multi-sig` trust \
      set), verified (block content hashes, trie pages by Law 70, ancestry membership and the \
      *derived* `fringe`), or residual (the bootstrap's choice of finalised root, the fringe's state \
      hashes — the root and each per-block `fringe_state_hash` — liveness, and wire \
      signatures/finality proofs). Two joiners that verify the same block map and hold the same trie \
      read the same value at the claimed root and answer the same block lookup; the residual is an \
      exact, checked enumeration",
    status := .provedModel,
    declarations := [`Rchain.Sync.Trust.Join, `Rchain.Sync.Trust.derivedFringe,
      `Rchain.Sync.Trust.join_is_content_trustless,
      `Rchain.Sync.Trust.a_lying_ancestry_fringe_is_refused,
      `Rchain.Sync.Trust.the_derivation_is_a_function_of_the_verified_inputs,
      `Rchain.Sync.Trust.the_residual_is_exactly_named],
    axioms := [],
    rust := ["casper/src/engine/node_syncing.rs", "casper/src/engine/lfs_block_requester.rs",
      "casper/src/genesis/mod.rs"],
    rustWitness := [
      "casper/src/engine/node_syncing.rs:an_ancestry_entry_for_an_unknown_block_is_refused",
      "casper/src/engine/node_syncing.rs:two_ancestry_entries_for_one_block_are_refused",
      "casper/src/engine/node_syncing.rs:an_ancestry_fringe_that_is_not_the_derived_fringe_is_refused"],
    witness := [`Rchain.Sync.Trust.a_lying_ancestry_state_hash_changes_replay,
      `Rchain.Sync.Trust.genesis_trust_anchor,
      `Rchain.Sync.Trust.a_different_multisig_list_changes_the_anchor,
      `Rchain.Sync.Trust.the_derivation_is_a_function_of_the_verified_inputs],
    falsifiable := "The peer-chosen field the join path could not check: \
      `a_lying_ancestry_state_hash_changes_replay` shows two ancestry rows with the same block and \
      fringe and a different `fringe_state_hash` produce different replay metadata, which \
      `close_block` folds into the epoch seed — so the fringe's state hash is a residual. \
      `a_lying_ancestry_fringe_is_refused` is the field that *is* derivable and therefore refused; \
      `a_different_multisig_list_changes_the_anchor` is the anchor's sensitivity. **A singleton pin \
      was tried and refuted**: it compared the root with the named block's pre- or post-state, and the \
      node's restore suite showed the served root is the state *at* the fringe — one fringe earlier \
      than the block's pre-state — so the check was removed rather than kept as false assurance, and \
      C339 now covers every fringe-state claim.",
    note := "**The exact wording matters.** The proved claim is *content* trustlessness: every state \
      byte and block body is checked against a hash, so two joiners with the same verified inputs \
      cannot read differently. It is **not** finality trustlessness: the joiner trusts the \
      bootstrap's choice of finalised root, its liveness, the per-block `fringe_state_hash` for a \
      multi-member fringe, and it checks neither block signatures nor a quorum finality proof. Those \
      are enumerated in `the_residual_is_exactly_named` and on the trust-model page. The Rust fix this \
      row accompanies verifies ancestry block membership, duplicate-freedom and the derived `fringe`, \
      and refuses a derivable fringe that lies; **C339 stays open** — the fringe's state hashes are \
      not derivable on the join path, so the residual stands. The witnesses are the join path's own \
      tests: membership, duplicate, derived-fringe, and the state-hash residual stated as one. A \
      singleton state pin was attempted and the node's restore suite refuted it; the law records the \
      absence rather than the false check." },
  { number := 84, layer := "Casper",
    statement := "**The genesis anchor is a function of the genesis inputs, and the ceremony refuses \
      any other key.** The block's sender, the blessed deploys' signer and the governance admin are \
      the **first bond in the bonds file** — an input — so identical inputs give identical bytes \
      (the signing schemes are deterministic) and therefore one genesis hash. A node whose local key \
      is not that bond must refuse to run the ceremony rather than build a private chain. The old \
      rule let each node's own key sign, so identical inputs gave different blocks (#456)",
    status := .provedModel,
    declarations := [`Rchain.GenesisInputs, `Rchain.genesisCreator, `Rchain.mayCreate,
      `Rchain.the_genesis_anchor_is_a_function_of_the_inputs,
      `Rchain.the_genesis_state_is_a_function_of_the_inputs],
    axioms := [],
    rust := ["casper/src/engine/node_launch.rs", "casper/src/genesis/mod.rs",
      "casper/src/bonds_parser.rs"],
    rustWitness := [
      "casper/src/engine/node_launch.rs:the_genesis_ceremony_is_the_first_bond_or_refused"],
    witness := [`Rchain.an_open_ceremony_reaches_two_genesis_hashes,
      `Rchain.the_first_bond_may_create],
    falsifiable := "An **open** ceremony — any local key may sign — reaches two genesis hashes from \
      one bonds file: `an_open_ceremony_reaches_two_genesis_hashes` exhibits it with the smallest \
      input (one bond, two other keys), and `mayCreate` refuses both under the rule. The measured \
      live instance is #456: two nodes, identical inputs, sender and post-state both different.",
    note := "**The rule is the first bond in bonds-file order, and the current network already obeys \
      it**: #456's table shows the network genesis sender is the first bond's key. So a correctly-run \
      ceremony produces the same hash as before; the change is the **refusal**, not a new genesis for \
      nets that followed it. What the model does not restate: that secp256k1 (RFC-6979) and Ed25519 \
      signatures are deterministic, which is what makes identical signed deploys give identical \
      bytes — that is `crypto/`'s property and its own tests. The Rust check is at `create_genesis`: \
      the first bond is read from the file (not the sorted map, which loses file order) and the local \
      key must equal it. The witness is the two-key ceremony test: the first bond is admitted, the \
      second refuses with both keys named, and the autogen path's `.sk` supplies the generated first \
      bond's key for a standalone node." },

  { number := 85, layer := "Casper",
    statement := "**The finalized fringe's root is a member's claim about a fringe it finalises, not a \
      state field of the named block.** The ordinary answer serves the max `(height, hash)` member's \
      `fringe_state_hash`; a recovery anchor and the fresh-shard answer serve a block's \
      `post_state_hash`; and a member's claim is the state at the fringe **it** finalises — one \
      fringe earlier than the set it belongs to. The served root is therefore always some member's \
      claim, but it need not equal either state the block carries, which is why the singleton pin was \
      false and removed. The joiner cannot recompute the merge, so the root is assumed (C339)",
    status := .provedModel,
    declarations := [`Rchain.FringeClaim, `Rchain.ClaimsTheStateAtItsFringe, `Rchain.servedRoot,
      `Rchain.the_served_root_is_a_members_claim,
      `Rchain.the_served_root_is_the_state_at_the_members_fringe],
    axioms := [],
    rust := ["casper/src/engine/node_running.rs", "casper/src/multi_parent_casper.rs",
      "casper/src/engine/node_syncing.rs"],
    rustWitness := [],
    witness := [`Rchain.a_members_claim_is_not_necessarily_a_block_state,
      `Rchain.the_served_root_is_a_members_claim],
    falsifiable := "The pin this branch first implemented: compare the served root with the named \
      block's pre- or post-state. `a_members_claim_is_not_necessarily_a_block_state` is its \
      refutation — a member's claim (the state at the empty fringe) is neither the block's pre-state \
      nor its post-state — and the node's `lfs_restore_dev_tax` suite found it in the genesis-branch \
      claim. The three answer branches are the remaining distinctions: they serve three different \
      kinds of value under one field name.",
    note := "**The member/finaliser distinction is the whole of it**, and `multi_parent_casper.rs` \
      records the stall it caused when a reader took a member for a finaliser (the base came out one \
      fringe early and the chain stopped at height 20). The responder's ordinary branch takes the max \
      member of the latest fringe and serves its membership claim anyway; the anchor and fresh-shard \
      branches serve `post_state_hash` instead. The law proves the served value is always a member's \
      claim (never an invented root) and that the claim is the state at that member's fringe; it does \
      **not** supply `stateAt` (the merge), which is why C339 stays open. Recomputing the merge on the \
      join path, or carrying a proof for the root, is the closing option; a pin is not." },

  { number := 86, clause := "a", layer := "Admission",
    statement := "**Admission is a bounded queue, and the decision reads only the pool's bounds.** A \
      free slot with a new id admits; a full pool refuses with a named reason. The resource word \
      (`normal`/`throttled`/`critical`/`unavailable`) is not an input to `enqueue`: it is an input to \
      the execution budget (86d). The legacy boolean gate is modelled as `legacyAdmit`, and \
      `the_legacy_gate_refuses_an_empty_slot` is the height-1688 state as a theorem",
    status := .provedTied,
    corpus := some "admission",
    declarations := [`Rchain.Admission.Pool, `Rchain.Admission.enqueue,
      `Rchain.Admission.legacyAdmit, `Rchain.Admission.an_empty_slot_admits,
      `Rchain.Admission.a_full_pool_refuses,
      `Rchain.Admission.the_legacy_gate_refuses_an_empty_slot],
    axioms := [],
    rust := ["casper/src/dag.rs", "casper/src/pool.rs", "node/src/resources/fallback.rs"],
    rustWitness := [],
    witness := [`Rchain.Admission.an_empty_slot_admits,
      `Rchain.Admission.a_full_pool_refuses,
      `Rchain.Admission.the_legacy_gate_refuses_an_empty_slot],
    falsifiable := "The live height-1688 state: an empty pool, a free slot, `throttled`, and a 503. \
      `the_legacy_gate_refuses_an_empty_slot` is that state proved; deleting the resource parameter \
      from `enqueue` is the change that would make it unprovable. A rule that admitted only above the \
      throttle floor would instead fail `an_empty_slot_admits`.",
    note := "The model's `capacity` is the count bound; the Rust's byte and per-deployer bounds are \
      the same `hasSlot` shape and are driven by the admission corpus when it lands. The row is \
      `proved-tied` on `spec/conformance/admission.tsv`; its Rust consumer drives \
      `casper::pool::check_admission`/`check_commit`, the same policy `dag.rs` calls. The corpus's \
      five cases cover the free slot, the count bound, the duplicate, one permit and zero headroom; \
      the rate, share and byte bounds are unit-tested in `casper::pool`. The law does **not** say a \
      node must accept unbounded work: a full pool is a real bound and refuses, with a retry \
      condition." },

  { number := 86, clause := "b", layer := "Admission",
    statement := "**No admitted work is silently lost.** A pool step advances an entry's status \
      (pending → running → done, or pending → expired) and never removes the row; every refusal \
      returns the pool it was given. `no_admitted_work_is_lost` proves every accepted id is in the \
      ledger at every reachable state, so a retirement is observable",
    status := .provedModel,
    declarations := [`Rchain.Admission.Accepted, `Rchain.Admission.StatusStep,
      `Rchain.Admission.Step, `Rchain.Admission.accepted_of_step,
      `Rchain.Admission.no_admitted_work_is_lost,
      `Rchain.Admission.a_dropping_pool_loses_work],
    axioms := [],
    rust := ["casper/src/dag.rs"],
    rustWitness := [],
    witness := [`Rchain.Admission.no_admitted_work_is_lost,
      `Rchain.Admission.a_refusal_changes_nothing],
    falsifiable := "`dropOnPressure` is the defect variant: it filters the entry out and \
      `a_dropping_pool_loses_work` exhibits the accepted id that is then absent. A step that removed \
      a row instead of advancing its status would make `no_admitted_work_is_lost` unprovable. The \
      expiry rule is the stated `pending → expired` transition, not an unobserved deletion.",
    note := "The ledger keeps `done` and `expired` rows rather than deleting them, so 'what happened \
      to this deploy?' is answerable after retirement. The Rust pool currently deletes an included \
      deploy on block insertion (`casper/src/dag.rs:1380-1417`) and keeps the deploy index; the model \
      states the property the index supplies, and the note names the difference. A rewind/reset is \
      outside this step relation and is Law 72's subject." },

  { number := 86, clause := "c", layer := "Admission",
    statement := "**Independent work commutes; conflicting work serialises.** At the effect level the \
      criterion is disjoint **closures** (`Independent`), not disjoint footprints; \
      `independent_pair_commutes` is the existing Law 9 theorem, and `independent_prefix_commutes` \
      moves an independent effect across a fold. At the state-change level, \
      `independent_change_commutes_with_a_fold` moves a `NonConflicting` change past a fold and \
      `independent_folds_commute` swaps two independent batches",
    status := .provedModel,
    declarations := [`Rchain.Admission.Independent,
      `Rchain.Admission.independent_pair_commutes,
      `Rchain.Admission.independent_prefix_commutes,
      `Rchain.Admission.independent_change_commutes_with_a_fold,
      `Rchain.Admission.independent_folds_commute,
      `Rchain.Admission.independent_batches_commute],
    axioms := [],
    rust := ["casper/src/runtime_manager.rs", "casper/src/merging.rs",
      "rspace/src/merger/state_change.rs"],
    rustWitness := [],
    witness := [`Rchain.Admission.independent_prefix_commutes,
      `Rchain.Admission.independent_change_commutes_with_a_fold,
      `Rchain.Admission.independent_folds_commute],
    falsifiable := "`effectA`/`effectB` have disjoint footprints and overlapping closures; \
      `effect_reorder_diverges` reaches different states in the two orders, so a footprint partition \
      is unsound. `nonConflicting_not_necessary` keeps the criterion honest: the law claims a \
      sufficient condition, not the port's conflict predicate.",
    note := "**The bridge is the obligation.** The Lean model's `NonConflicting` is over the \
      modelled `StateChange`; the Rust executor certifies a batch from its **dynamic** read/write \
      `TouchLog` (channels and native slots, read or written). That the touch log implies \
      `NonConflicting` is a refinement obligation, not a theorem here, which is why this row is \
      `proved-model` and never silently `proved-tied`. The first Rust release keeps \
      `batch.verify = true` (re-execute sequentially and compare state hash and every \
      `ProcessedDeploy`) with a circuit breaker; the differential batch suite is the witness." },

  { number := 86, clause := "d", layer := "Admission",
    statement := "**One permit progresses; capacity changes the schedule, not the queue.** A \
      non-empty pending pool with at least one permit has a step (`one_permit_progresses`), and \
      `next` takes at most `permits` ids (`the_batch_is_bounded_by_the_permits`). The defect shape is \
      `needsTwoPermits`: a non-empty pool with one permit and no step",
    status := .provedTied,
    corpus := some "admission",
    declarations := [`Rchain.Admission.Node, `Rchain.Admission.scheduleBatch,
      `Rchain.Admission.next, `Rchain.Admission.NodeStep,
      `Rchain.Admission.one_permit_progresses,
      `Rchain.Admission.scheduleBatch_length_le_permits,
      `Rchain.Admission.the_batch_is_bounded_by_the_permits,
      `Rchain.Admission.a_scheduler_that_needs_two_permits_stalls],
    axioms := [],
    rust := ["casper/src/budget.rs", "node/src/resources/budget.rs",
      "casper/src/blocks/proposer/proposer.rs"],
    rustWitness := [],
    witness := [`Rchain.Admission.one_permit_progresses,
      `Rchain.Admission.the_batch_is_bounded_by_the_permits,
      `Rchain.Admission.a_scheduler_that_needs_two_permits_stalls],
    falsifiable := "`needsTwoPermits` is the defect: a step relation that requires two permits \
      leaves a non-empty pool with one permit with no step, which \
      `a_scheduler_that_needs_two_permits_stalls` exhibits. The live node's boolean gate had the \
      stronger defect — no step even with every permit — and `the_legacy_gate_refuses_an_empty_slot` \
      is that half.",
    note := "The permit count is the headroom budget (64 MiB per execution in the first cut), never \
      a permission to enqueue. `enqueue` has no permit parameter; `next` does, so a smaller budget makes \
      a smaller batch and cannot change an admission answer. Critical/Unavailable may stop block \
      production; they do not stop admission or relay." },

  { number := 86, clause := "e", layer := "Admission",
    statement := "**Pending work is relocatable without loss.** A relay **copies** an entry to a peer \
      and leaves the sender's row alone (`relay_preserves_the_sender`); when the receiver has room \
      the copy is accepted (`relay_copies_to_the_receiver`); both pools keep every accepted id \
      (`relay_preserves_both_pools`). The defect shape is a **moving** relay, which loses work",
    status := .provedModel,
    declarations := [`Rchain.Admission.TwoNodes, `Rchain.Admission.relay,
      `Rchain.Admission.relay_preserves_the_sender,
      `Rchain.Admission.relay_copies_to_the_receiver,
      `Rchain.Admission.relay_preserves_both_pools,
      `Rchain.Admission.a_moving_relay_loses_work],
    axioms := [],
    rust := ["casper/src/dag.rs", "casper/src/api/block_api_impl.rs",
      "node/src/runtime/node_runtime.rs", "casper/src/protocol/client.rs"],
    rustWitness := [],
    witness := [`Rchain.Admission.relay_preserves_the_sender,
      `Rchain.Admission.relay_preserves_both_pools,
      `Rchain.Admission.a_moving_relay_loses_work],
    falsifiable := "`moveRelay` deletes from the sender and drops when the receiver is full; \
      `a_moving_relay_loses_work` exhibits the id absent from both pools. A relay that removed the \
      sender's row would make `relay_preserves_the_sender` unprovable.",
    note := "**The Rust relay is a copy over the deploy service, not a new wire message.** After an \
      admission, `BlockApiImpl` calls the operator-enabled `RelayFunction`, which offers the signed \
      deploy to each connected peer's external deploy service (`GrpcDeployService::deploy`). The \
      receiver runs the same signature, shard, rate, share, count and byte checks as a direct \
      submission, because it is the same `deploy` path; the sender keeps its own pool row. A bounded \
      seen set suppresses a relay circulating; the switch is `RNODE_DEPLOY_RELAY` (default off, API \
      port 40401, `RNODE_DEPLOY_RELAY_PORT` to change it). The model's receiver bound is the count \
      bound; the Rust's is the receiver's own pool." },

  { number := 87, clause := "a", layer := "Decimal",
    statement := "**Canonicalisation is a normal form.** `canonicalize` strips trailing zeros while \
      the scale is positive and sends every zero to `(0,0)`; `new` canonicalises **before** the \
      scale bound, and `fromParts` bounds **first** and then refuses a zero-with-scale or a \
      trailing-zero pair **by name**",
    status := .provedTied,
    corpus := some "decimal",
    declarations := [`Rchain.Decimal.Canonical, `Rchain.Decimal.canonicalize,
      `Rchain.Decimal.new, `Rchain.Decimal.fromParts,
      `Rchain.Decimal.canonicalize_idempotent, `Rchain.Decimal.canonicalize_cross,
      `Rchain.Decimal.scaledValue_canonicalize, `Rchain.Decimal.new_ok_iff,
      `Rchain.Decimal.new_error_iff, `Rchain.Decimal.fromParts_ok_iff,
      `Rchain.Decimal.fromParts_zero_with_scale,
      `Rchain.Decimal.fromParts_trailing_zero],
    axioms := [],
    rust := ["models/src/decimal.rs"],
    rustWitness := ["models/src/decimal.rs:canonicalisation_strips_trailing_zeros_and_normalises_zero",
      "models/src/decimal.rs:the_scale_bound_is_inclusive",
      "models/src/decimal.rs:strict_construction_refuses_a_noncanonical_pair",
      "models/tests/lean_decimal_corpus.rs:the_decimal_algebra_matches_the_lean_model"],
    witness := [`Rchain.Decimal.canonicalize_idempotent,
      `Rchain.Decimal.new_ok_iff,
      `Rchain.Decimal.fromParts_ok_iff,
      `Rchain.Decimal.fromParts_zero_with_scale,
      `Rchain.Decimal.fromParts_trailing_zero],
    falsifiable := "`new 10^39,39` is accepted as `(1,0)` while `new 1,39` is \
      `ScaleOutOfRange 39`: the bound is applied after canonicalisation. `new 0,39` is `(0,0)` \
      because the zero is canonicalised before the bound is read. `fromParts 0,3` is \
      `ZeroWithScale 3` and `fromParts 110,2` is `TrailingZero 2`; a constructor that classified by \
      the wrong test fails those rows, and deleting canonicalisation before the bound flips the \
      `10^39,39` row.",
    note := "The model's `Int`/`Nat` stand for the Rust `BigInt`/`u32`. The wire's minimal-byte form \
      and tag 35, serde ingress, the literal surface, reducer dispatch and the cost model are \
      outside Law 87 and remain Rust-tested. The corpus's `new`/`parts` rows take the raw pair, \
      because those are the constructors where a non-canonical input is legal (and refused by name)." },

  { number := 87, clause := "b", layer := "Decimal",
    statement := "**`scaledValue` is the value embedding.** On canonical values it is injective, \
      and `cmpNumeric` is the order of the scaled payload: `cmpNumeric a b = cmp (scaledValue a) \
      (scaledValue b)` with `eq_iff`, `swap` and `lt_trans`",
    status := .provedTied,
    corpus := some "decimal",
    declarations := [`Rchain.Decimal.scaledValue, `Rchain.Decimal.cmpNumeric,
      `Rchain.Decimal.canonical_zero_unique, `Rchain.Decimal.scaledValue_injective,
      `Rchain.Decimal.cmpNumeric_eq_scaled, `Rchain.Decimal.cmpNumeric_eq_iff,
      `Rchain.Decimal.cmpNumeric_swap, `Rchain.Decimal.cmpNumeric_lt_trans],
    axioms := [],
    rust := ["models/src/decimal.rs", "models/src/sorter.rs"],
    rustWitness := ["models/src/decimal.rs:scaled_value_is_unique_for_canonical_values",
      "models/src/decimal.rs:comparison_is_numeric_across_scales",
      "models/src/sorter.rs:smaller_decimal_values_come_first_across_scales",
      "models/tests/lean_decimal_corpus.rs:the_decimal_algebra_matches_the_lean_model"],
    witness := [`Rchain.Decimal.scaledValue_injective,
      `Rchain.Decimal.cmpNumeric_eq_scaled,
      `Rchain.Decimal.cmpNumeric_eq_iff,
      `Rchain.Decimal.cmpNumeric_swap,
      `Rchain.Decimal.cmpNumeric_lt_trans],
    falsifiable := "The pair `(10,1)` and `(1,0)` has one `scaledValue` and is not equal, so the \
      `Canonical` hypothesis is load-bearing. `cmp 15:1 2:0` is `lt`; a lexicographic \
      `(coefficient, scale)` order would say `gt`, which is the defect the sorter's DECIMAL score \
      avoids. `cmp -15:1 -2:0` is `gt`, pinning the sign against an absolute-value comparator.",
    note := "`scaledValue d = coefficient × 10^(MAX_SCALE − scale)`; `MAX_SCALE = 38` is the sorter's \
      score and the `Expr.edecimal` payload (`spec/Rchain/Par.lean`). The row is tied to the decimal \
      corpus for the value algebra and to `sort.tsv` rows 43–50 for the payload's class order." },

  { number := 87, clause := "c", layer := "Decimal",
    statement := "**`+`, `-` and `*` are exact.** The result is canonical; addition and \
      subtraction satisfy `scaledValue (add a b) = scaledValue a + scaledValue b` and its \
      subtraction dual, and a successful product satisfies `scaledValue q × 10^38 = scaledValue a × \
      scaledValue b`. `add` commutes and `sub (add a b) b = a`",
    status := .provedTied,
    corpus := some "decimal",
    declarations := [`Rchain.Decimal.add, `Rchain.Decimal.sub, `Rchain.Decimal.mul,
      `Rchain.Decimal.add_canonical, `Rchain.Decimal.sub_canonical,
      `Rchain.Decimal.scaledValue_add, `Rchain.Decimal.scaledValue_sub,
      `Rchain.Decimal.add_new, `Rchain.Decimal.sub_new,
      `Rchain.Decimal.add_comm, `Rchain.Decimal.add_sub_cancel,
      `Rchain.Decimal.mul_ok_iff, `Rchain.Decimal.mul_error_iff,
      `Rchain.Decimal.scaledValue_mul],
    axioms := [],
    rust := ["models/src/decimal.rs"],
    rustWitness := ["models/src/decimal.rs:arithmetic_is_exact",
      "models/tests/lean_decimal_corpus.rs:the_decimal_algebra_matches_the_lean_model"],
    witness := [`Rchain.Decimal.add_canonical, `Rchain.Decimal.scaledValue_add,
      `Rchain.Decimal.mul_ok_iff, `Rchain.Decimal.scaledValue_mul,
      `Rchain.Decimal.add_sub_cancel],
    falsifiable := "`9:38 + 1:38 = 1:37` and `5:1 + 5:1 = 1:0` pin carry and strip; `25:20 × 4:20 = \
      1:38` while `1:20 × 1:20` is `ScaleOutOfRange 40`, so canonicalisation before the bound is \
      observable. The product identity is `q.scaledValue × 10^38 = a.scaledValue × b.scaledValue`; \
      dropping the `10^38` is false by that factor, and the corpus's `mul` rows catch it.",
    note := "The Rust `add`/`sub` return `Result` because `Decimal::new` is fallible in general, but \
      for canonical operands the aligned scale is at most 38 and the result is always `Ok`; the model \
      is total and `add_new`/`sub_new` are the bridge. `mul` is `Except`: the only model error is \
      `ScaleOutOfRange` after canonicalisation; the u32-only `ScaleOverflow` is outside. Mixed \
      `Int`/`BigInt` promotion and the cost model are reducer concerns, Rust-tested, not modelled." },

  { number := 87, clause := "d", layer := "Decimal",
    statement := "**Division is exact or an explicit error.** A zero divisor is \
      `DivisionByZero`; a nonzero divisor returns a canonical quotient exactly when one exists \
      within `MAX_SCALE`, and otherwise `InexactDivision`. The quotient satisfies \
      `q.scaledValue × b.scaledValue = a.scaledValue × 10^38`",
    status := .provedTied,
    corpus := some "decimal",
    declarations := [`Rchain.Decimal.divAt, `Rchain.Decimal.divUpTo, `Rchain.Decimal.div,
      `Rchain.Decimal.divAt_spec, `Rchain.Decimal.divUpTo_none_iff,
      `Rchain.Decimal.divUpTo_some_spec, `Rchain.Decimal.div_zero,
      `Rchain.Decimal.div_ok_spec, `Rchain.Decimal.div_error_iff],
    axioms := [],
    rust := ["models/src/decimal.rs"],
    rustWitness := ["models/src/decimal.rs:division_is_exact_or_refused",
      "rholang/src/reduce.rs:decimal_division_is_exact_or_an_explicit_error",
      "models/tests/lean_decimal_corpus.rs:the_decimal_algebra_matches_the_lean_model"],
    witness := [`Rchain.Decimal.divAt_spec, `Rchain.Decimal.divUpTo_none_iff,
      `Rchain.Decimal.div_ok_spec, `Rchain.Decimal.div_error_iff,
      `Rchain.Decimal.div_zero],
    falsifiable := "`1/3` and `1/2^39` are `InexactDivision`; `1/2` and `1/2^38` are exact; `1/0` \
      is `DivisionByZero`. A rounding division, or a 39th digit, changes one of those. The division \
      corpus pins the inclusive 38-digit bound and the negative case `-1/2 = -5:1`.",
    note := "The theorem is the **exactness criterion**, not the algorithm: the Rust computes the \
      quotient one decimal digit at a time, while the model scans `divAt` over scales `0..=38`. The \
      corpus is the tie between them. `Int`'s Euclidean `/` and `%` agree with the Rust's truncating \
      `BigInt` on the only uses here — a divisibility test and an exact quotient — and the negative \
      corpus rows pin it." },

  { number := 87, clause := "e", layer := "Decimal",
    statement := "**The `edecimal` payload bridge.** `edecimalOf d = Expr.edecimal d.scaledValue`; \
      on canonical values it is injective, and `cmpExpr` on two payloads is `cmpNumeric`, so the \
      model's value order is the sorter's DECIMAL class order",
    status := .provedTied,
    corpus := some "sort",
    declarations := [`Rchain.Decimal.edecimalOf, `Rchain.Decimal.cmpExpr_edecimalOf,
      `Rchain.Decimal.edecimalOf_eq_iff,
      `Rchain.Decimal.cmpNumeric_eq_cmpExpr_edecimalOf],
    axioms := [],
    rust := ["models/src/sorter.rs"],
    rustWitness := ["models/src/sorter.rs:smaller_decimal_values_come_first_across_scales",
      "models/src/sorter.rs:decimal_keys_normalise_within_the_class_but_not_against_an_int",
      "rholang/tests/lean_sort_corpus.rs:the_canonical_order_is_the_lean_models_pairwise"],
    witness := [`Rchain.Decimal.cmpExpr_edecimalOf,
      `Rchain.Decimal.edecimalOf_eq_iff,
      `Rchain.Decimal.cmpNumeric_eq_cmpExpr_edecimalOf],
    falsifiable := "`sort.tsv` rows 43–50 put the Decimal class at rank 14 and order `1.5d` before \
      `2d`; a payload that carried the `(coefficient, scale)` pair instead of `scaledValue` flips \
      those rows, and a two-field `Expr.edecimal` would make the comparator's `eq_iff` false for \
      `(10,1)` and `(1,0)`.",
    note := "This clause is tied to the existing **`sort`** corpus (rows 43–50), not to the decimal \
      corpus: the value algebra is tied by `decimal.tsv`, while `Expr.edecimal` is compared by the \
      sorter corpus and `models/src/sorter.rs`'s DECIMAL score. Law 1a's note is updated to point \
      here; its statement, status and declarations are not widened." },

  { number := 88, clause := "a", layer := "Disposition",
    statement := "**A stored body is accounted for or visible.** A body in the block store is \
      either a DAG block (its metadata is the durable verdict) or carries an unsettled \
      `Pending`/`Faulted` row; the start-up audit adopts every unaccounted body before the phase \
      becomes `Running`, so that transition cannot skip a stored body. The law says **visible**, \
      not **settled**: a body whose justifications are not in the DAG stays `Pending` and visible, \
      and the bounded start-up drain does not wait for it",
    status := .provedModel,
    declarations := [`Rchain.Disposition.State,
      `Rchain.Disposition.Disposition, `Rchain.Disposition.audit,
      `Rchain.Disposition.adoptBody, `Rchain.Disposition.startup,
      `Rchain.Disposition.audit_covers,
      `Rchain.Disposition.audit_makes_an_unbacked_body_visible,
      `Rchain.Disposition.audit_does_not_change_dag,
      `Rchain.Disposition.startup_clean,
      `Rchain.Disposition.startup_without_audit_leaves_an_orphan,
      `Rchain.Disposition.a_dropped_row_is_an_orphan],
    axioms := [],
    rust := ["casper/src/block_disposition.rs", "casper/src/engine/node_launch.rs",
      "node/src/runtime/node_runtime.rs", "casper/src/dag.rs", "shared/src/typed_store.rs"],
    rustWitness := ["shared/src/typed_store.rs:keys_decodes_only_the_key_and_survives_an_undecodable_value",
      "casper/src/block_disposition.rs:the_legacy_scan_adopts_every_unaccounted_body_and_writes_its_marker",
      "casper/src/block_disposition.rs:a_stored_body_without_a_dag_entry_is_reoffered",
      "casper/src/block_disposition.rs:the_phase_stays_starting_until_the_drain_closes"],
    witness := [`Rchain.Disposition.audit_covers,
      `Rchain.Disposition.audit_makes_an_unbacked_body_visible,
      `Rchain.Disposition.audit_does_not_change_dag,
      `Rchain.Disposition.startup_clean,
      `Rchain.Disposition.startup_without_audit_leaves_an_orphan,
      `Rchain.Disposition.a_dropped_row_is_an_orphan],
    falsifiable := "`skipAudit` flips the phase without adopting: \
      `startup_without_audit_leaves_an_orphan` exhibits the stored body 7 with no DAG entry and no \
      row, which is the live incident's shape. Deleting a row without a DAG record \
      (`a_dropped_row_is_an_orphan`) is the other defect shape; an audit that removed rows instead \
      of adding them fails `audit_covers`.",
    note := "Node-local durable ingestion, not consensus state, so `proved-model` rather than a \
      corpus-tied row: there is no wire or derived value to tie. This row promotes C343's \
      `candidate:block-disposition-and-isolation`. The model assumes readable bodies; the Rust scan \
      skips a body whose bytes do not decode, that body remains an orphan, and the periodic \
      stored-body count audit (`orphanBodies`) withholds readiness. The bounded drain is a \
      liveness policy, not a state predicate: `Running` follows an audit attempt, and a \
      missing-parent row stays `Pending` by design." },

  { number := 88, clause := "b", layer := "Disposition",
    statement := "**A proposal is admitted only in `Running` and within the finality bound.** The \
      gate refuses outside the running phase and one height beyond `MAX_PROPOSAL_FINALITY_LAG`, \
      which is what bounds a private chain when no peer accepts it. The gate is **total in the \
      anchor**: a view with no finalised block measures from genesis, so `no anchor` is not `no \
      bound`",
    status := .provedModel,
    declarations := [`Rchain.Disposition.Phase,
      `Rchain.Disposition.MAX_PROPOSAL_FINALITY_LAG,
      `Rchain.Disposition.withinProposalFinalityBound,
      `Rchain.Disposition.gate, `Rchain.Disposition.Refusal,
      `Rchain.Disposition.GateResult,
      `Rchain.Disposition.gate_admits_only_in_running_and_within_bound,
      `Rchain.Disposition.gate_admits_at_the_bound,
      `Rchain.Disposition.gate_refuses_beyond_the_bound,
      `Rchain.Disposition.gate_refuses_outside_running,
      `Rchain.Disposition.a_gate_without_the_phase_admits_a_syncing_node,
      `Rchain.Disposition.a_gate_without_the_bound_admits_a_private_chain,
      `Rchain.Disposition.finalizedFloor, `Rchain.Disposition.totalGate,
      `Rchain.Disposition.floor_never_exceeds_the_anchor,
      `Rchain.Disposition.skipWithoutAnchor,
      `Rchain.Disposition.a_gate_that_skips_the_bound_without_an_anchor_admits_a_private_chain,
      `Rchain.Disposition.a_view_without_a_finalized_block_is_still_bounded,
      `Rchain.Disposition.the_anchor_is_passed_through],
    axioms := [],
    rust := ["casper/src/blocks/proposer/proposer.rs",
      "node/src/instances/proposer_instance.rs", "casper/src/engine/node_phase.rs",
      "node/src/web/http.rs"],
    rustWitness := ["casper/src/blocks/proposer/proposer.rs:the_proposal_finality_bound_passes_at_the_bound_and_refuses_beyond_it",
      "casper/src/blocks/proposer/proposer.rs:a_view_without_a_finalized_block_still_trips_the_bound",
      "casper/src/engine/node_phase.rs:two_cells_hold_independent_phases",
      "node/src/instances/proposer_instance.rs:a_syncing_cell_does_not_propose",
      "node/src/web/http.rs:health_is_ready_only_when_every_shard_runs"],
    witness := [`Rchain.Disposition.gate_admits_only_in_running_and_within_bound,
      `Rchain.Disposition.gate_admits_at_the_bound,
      `Rchain.Disposition.gate_refuses_beyond_the_bound,
      `Rchain.Disposition.gate_refuses_outside_running,
      `Rchain.Disposition.a_gate_without_the_phase_admits_a_syncing_node,
      `Rchain.Disposition.a_gate_without_the_bound_admits_a_private_chain,
      `Rchain.Disposition.a_gate_that_skips_the_bound_without_an_anchor_admits_a_private_chain,
      `Rchain.Disposition.a_view_without_a_finalized_block_is_still_bounded],
    falsifiable := "`legacyGate` admits a syncing node \
      (`a_gate_without_the_phase_admits_a_syncing_node`) and `unboundedGate` admits a tip beyond \
      the bound (`a_gate_without_the_bound_admits_a_private_chain`). A gate that reads the wrong \
      phase, or drops the bound, fails those rows. **And the third shape, which the Rust had** \
      (C351): `skipWithoutAnchor` answers `.allow` for every tip when the view has no fringe, which \
      `a_gate_that_skips_the_bound_without_an_anchor_admits_a_private_chain` exhibits; \
      `a_view_without_a_finalized_block_is_still_bounded` is total, and \
      `a_view_without_a_finalized_block_still_trips_the_bound` pins that the Rust now answers it \
      the same way.",
    note := "The bound is policy (`32`), not a measured optimum, and the Rust split is: the phase \
      gate in `proposer_instance`, the height gate in `proposer.rs`. This row promotes C343's \
      `candidate:block-disposition-and-isolation`. **The anchor is total since C351**: the gate was \
      wrapped in `if let Some(finalized)`, so a view with no finalised block skipped both the bound \
      and the isolation latch. Genesis is the floor (`finalized_floor`), which is the answer \
      `RewindPlan` already gives and the one `finalizedFloor` states; the model took a plain `Nat` \
      all along, so the Rust was the outlier. Normal operation is unchanged (at genesis the gap is \
      1 of 32). The model does not prove a peer accepted the chain — it bounds the private chain a \
      node can build; the finality algorithm and slashing are outside it." },

  { number := 88, clause := "c", layer := "Disposition",
    statement := "**An overflow refusal is a deferral, not an orphan.** The receiver stores the \
      body and writes the `Pending` row before it can refuse for capacity, so the refusal changes \
      no row: the obligation survives by construction. A runtime sweep re-offers an aged `Stored` \
      row once every justification is in the DAG; a row already in flight (`Validating`/`Retry`) is \
      never eligible; a held body's claim is taken once; and a re-offered row moves to `Retry`, so \
      a second sweep dispatches nothing",
    status := .provedTied,
    corpus := some "recovery",
    declarations := [`Rchain.Disposition.Recovery.Row, `Rchain.Disposition.Recovery.eligible,
      `Rchain.Disposition.Recovery.refuse,
      `Rchain.Disposition.Recovery.an_overflow_refusal_keeps_the_row,
      `Rchain.Disposition.Recovery.Receiver, `Rchain.Disposition.Recovery.claim,
      `Rchain.Disposition.Recovery.a_claim_is_taken_once,
      `Rchain.Disposition.Recovery.reoffer,
      `Rchain.Disposition.Recovery.reoffer_is_not_eligible,
      `Rchain.Disposition.Recovery.the_age_floor_gates_eligibility,
      `Rchain.Disposition.Recovery.an_inflight_row_is_not_eligible],
    axioms := [],
    rust := ["casper/src/block_disposition.rs", "casper/src/blocks/block_receiver.rs",
      "node/src/runtime/node_runtime.rs"],
    rustWitness := ["casper/src/block_disposition.rs:a_refused_stored_body_is_reoffered_by_the_runtime_sweep",
      "casper/src/block_disposition.rs:a_pending_body_waits_for_its_parent",
      "casper/src/block_disposition.rs:the_sweep_is_idempotent",
      "casper/src/blocks/block_receiver.rs:a_reoffer_claim_is_taken_once_and_never_steals_an_inflight_block"],
    witness := [`Rchain.Disposition.Recovery.an_overflow_refusal_keeps_the_row,
      `Rchain.Disposition.Recovery.a_claim_is_taken_once,
      `Rchain.Disposition.Recovery.reoffer_is_not_eligible,
      `Rchain.Disposition.Recovery.the_age_floor_gates_eligibility,
      `Rchain.Disposition.Recovery.an_inflight_row_is_not_eligible],
    falsifiable := "Removing the age floor makes the sweep race the receiver's own release and the \
      boot sweep (`the_age_floor_gates_eligibility` flips); dropping the `Validating`/`Retry` \
      exclusion makes a second sweep duplicate an in-flight body (`an_inflight_row_is_not_eligible`, \
      `the_sweep_is_idempotent`); dropping the claim lets the sweep and the receiver dispatch the \
      same held body (`a_reoffer_claim_is_taken_once...`); and a sweep that does not move the row \
      in flight fails `a_refused_stored_body_is_reoffered_by_the_runtime_sweep`'s `Retry` \
      assertion.",
    note := "The code was adopted from candidate `ac41fe7d`/`4d9883a4`/`20ef2505` and re-homed on \
      #499's `NodePhaseCell` (the candidate calls the pre-#499 free `wait_until_running`), with the \
      receiver claim added: the candidate's sweep could race the receiver's own release, because the \
      in-memory state is shared through the `Arc<Mutex<…>>`. **Liveness only** — the sweep dispatches \
      the same `block_processor` validation path as the boot audit, and no validity, state-hash or \
      wire rule changes. The model abstracts the store I/O, the missing-body arm and the queue. \
      Named residual: a just-released block is briefly neither held nor in the DAG; the 60 s floor \
      makes the window small, and the restart-under-load arm is the place to look for duplicate \
      proposals before adding an in-flight hash set." },

  { number := 88, clause := "d", layer := "Disposition",
    statement := "**An unresolvable justification is named, not silently passed into an \
      unclassified fault.** The regression check cannot be evaluated when a justification is absent \
      from the DAG message map; the current rule **passes** (the Scala's own `None`), and that \
      verdict is deliberate — only a detected regression refuses. What the outcome must carry is its \
      name: `Unresolved` is distinct from `Pass`, `NoOwnJustification` from both, and a later \
      height/sequence failure is recorded under the stage `missing justification`, not \
      `validation internal`",
    status := .provedTied,
    corpus := some "recovery",
    declarations := [`Rchain.Disposition.Recovery.Outcome,
      `Rchain.Disposition.Recovery.classify, `Rchain.Disposition.Recovery.refuses,
      `Rchain.Disposition.Recovery.outcomeName,
      `Rchain.Disposition.Recovery.unresolved_does_not_refuse,
      `Rchain.Disposition.Recovery.regression_refuses,
      `Rchain.Disposition.Recovery.pass_does_not_refuse],
    axioms := [],
    rust := ["casper/src/validate.rs", "casper/src/blocks/block_processor.rs"],
    rustWitness := ["casper/src/validate.rs:an_unknown_justification_passes_the_regression_check_and_is_named",
      "casper/src/blocks/block_processor.rs:a_missing_justification_is_recorded_under_its_own_stage"],
    witness := [`Rchain.Disposition.Recovery.unresolved_does_not_refuse,
      `Rchain.Disposition.Recovery.regression_refuses,
      `Rchain.Disposition.Recovery.pass_does_not_refuse],
    falsifiable := "Making `Unresolved` a refusal flips `unresolved_does_not_refuse` and the \
      corpus `classify false true unresolved` row — and is a consensus change, which is why the test \
      `an_unknown_justification_passes_the_regression_check_and_is_named` pins the passing verdict. \
      Folding the reason back into `validation internal` fails \
      `a_missing_justification_is_recorded_under_its_own_stage`.",
    note := "The verdicts are **unchanged**: a block that passed the regression check still passes \
      it, and one that failed still refuses with `JustificationRegression`. The later failure is the \
      height/sequence check's `Err(\"missing justification <hex>\")`, which the processor now records \
      under its own stage; the retry semantics are the same durable `Faulted` row. This is \
      classification and observability, not a validity rule — a design pass corrected an earlier \
      reading that called it a live drop (the processor already records and retries the fault)." },

  { number := 89, layer := "PoS",
    statement := "**Liveness enforcement is jail first; only a vote reaches the bond.** A validator \
      absent across advancing epoch boundaries is removed from the active set for `J` boundaries and \
      paid nothing, while its bond and its delegators' principal are untouched. A stake-weighted vote \
      of strictly more than two thirds of the active stake, opened only against an already-jailed \
      validator, may slash it at the configured tier and permanently eject it. A stalled boundary or \
      an off-boundary block changes nothing",
    status := .provedModel,
    declarations := [`Rchain.Liveness.State, `Rchain.Liveness.stepJail,
      `Rchain.Liveness.boundaryStep, `Rchain.Liveness.jail_never_touches_the_bond,
      `Rchain.Liveness.the_stall_rule_changes_nothing,
      `Rchain.Liveness.no_boundary_no_action,
      `Rchain.Liveness.one_lapse_is_not_sustained,
      `Rchain.Liveness.stallStep,
      `Rchain.Liveness.a_stall_that_counts_jails_an_innocent,
      `Rchain.Liveness.passes, `Rchain.Liveness.voteSlash,
      `Rchain.Liveness.vote_admits_only_above_threshold,
      `Rchain.Liveness.a_passing_vote_slashes_and_ejects,
      `Rchain.Liveness.a_simple_majority_is_not_enough,
      `Rchain.Liveness.openWindow,
      `Rchain.Liveness.a_vote_without_jail_is_not_opened,
      `Rchain.Liveness.burnStep,
      `Rchain.Liveness.jail_that_burns_the_bond_reaches_a_stake,
      `Rchain.Liveness.releaseIfPresent,
      `Rchain.Liveness.a_returning_validator_is_released,
      `Rchain.Liveness.renewIfAbsent,
      `Rchain.Liveness.a_still_absent_validator_is_renewed],
    axioms := [],
    rust := ["rholang/src/native_state.rs", "rholang/src/pos_liveness.rs",
      "rholang/src/system_processes.rs", "casper/src/genesis/mod.rs",
      "block-storage/src/dag/liveness.rs"],
    rustWitness := ["rholang/src/native_state.rs:a_sustained_absence_jails_without_touching_the_bond",
      "rholang/src/native_state.rs:one_boundary_omission_does_not_jail",
      "rholang/src/native_state.rs:a_stall_freezes_the_jail_clock",
      "rholang/src/native_state.rs:a_vote_below_two_thirds_does_not_slash",
      "rholang/src/native_state.rs:a_passing_vote_slashes_and_permanently_ejects",
      "rholang/src/native_state.rs:a_jailed_validator_returns_after_the_term",
      "rholang/src/native_state.rs:a_still_absent_validator_is_renewed",
      "rholang/src/native_state.rs:a_banished_validator_cannot_rebond",
      "rholang/src/native_state.rs:a_delegation_cannot_buy_slash_voting_power_mid_epoch",
      "rholang/src/native_state.rs:two_of_three_active_votes_cannot_cross_strict_threshold_via_delegation",
      "rholang/src/system_processes.rs:pos_vote_slash_is_authorised_by_the_deployer_id"],
    witness := [`Rchain.Liveness.jail_never_touches_the_bond,
      `Rchain.Liveness.the_stall_rule_changes_nothing,
      `Rchain.Liveness.no_boundary_no_action,
      `Rchain.Liveness.one_lapse_is_not_sustained,
      `Rchain.Liveness.a_stall_that_counts_jails_an_innocent,
      `Rchain.Liveness.vote_admits_only_above_threshold,
      `Rchain.Liveness.a_passing_vote_slashes_and_ejects,
      `Rchain.Liveness.a_simple_majority_is_not_enough,
      `Rchain.Liveness.a_vote_without_jail_is_not_opened,
      `Rchain.Liveness.jail_that_burns_the_bond_reaches_a_stake,
      `Rchain.Liveness.a_returning_validator_is_released,
      `Rchain.Liveness.a_still_absent_validator_is_renewed],
    falsifiable := "`stallStep` counts absence without checking finality progress and jails a \
      validator during a stall (`a_stall_that_counts_jails_an_innocent`); `burnStep` jails by \
      removing the pool entry and `jail_that_burns_the_bond_reaches_a_stake` exhibits the bond that \
      moved, while `stepJail` leaves `pool` unchanged; a simple-majority gate admits two of three \
      (`a_simple_majority_is_not_enough`); a vote against an active validator is not opened \
      (`a_vote_without_jail_is_not_opened`).",
    note := "RCHIP #55. The model separates the two steps: `stepJail` never writes `pool`, and \
      `voteSlash` writes it only when `passes` (strict `3 * for > 2 * total`, the same rule as \
      `is_super_majority`), and both sides of that inequality read the **active-set snapshot**, not \
      the mutable pool, so a mid-epoch delegation cannot buy voting power. The Rust side supplies the expiry/renewal arithmetic, the participation \
      map (from the receiver's finalised fringe), the vote-window tally, the delegation fan-out on a \
      slash, and the withdrawal escrow; those are outside the model. This is **not** an inactivity \
      leak: no automatic step touches a bond. Default off; a network arms it at genesis. The boundary- \
      skip hole (no validity rule requires a `CloseBlock` at a boundary) is a separate residual." },

  { number := 90, clause := "a", layer := "Node",
    statement := "**A served hostlist entry is earned.** A peer appears in `GET /api/v1/nodes` only \
      when a probe of its derived client endpoint (`protocol + 3`) completed in the current cycle \
      with the serving node's `networkId`, a `shardId` in its membership, `proposer = true`, and an \
      `address` naming that peer's node id. A peer that is not dialable, does not answer, answers for \
      another network, shard or node, or does not claim a proposer is not served; the serving node \
      itself is served only when it can propose",
    status := .provedTied,
    corpus := some "hostlist",
    declarations := [`Rchain.HostList.valid, `Rchain.HostList.peerPart,
      `Rchain.HostList.servedIds, `Rchain.HostList.every_served_entry_is_valid,
      `Rchain.HostList.every_served_peer_is_a_proposer,
      `Rchain.HostList.a_failed_single_probe_is_absent,
      `Rchain.HostList.peer_part_mem],
    axioms := [],
    rust := ["node/src/web/node_list.rs", "casper/src/api/block_api_impl.rs"],
    rustWitness := ["node/src/web/node_list.rs:an_observer_is_not_served",
      "node/src/web/node_list.rs:a_mismatched_network_is_not_served",
      "node/src/web/node_list.rs:a_peer_on_another_shard_is_not_served",
      "node/src/web/node_list.rs:a_peer_answering_for_another_node_id_is_not_served",
      "node/src/web/node_list.rs:an_unreachable_peer_is_not_served",
      "node/src/web/node_list.rs:a_non_json_answer_is_refused",
      "casper/src/api/block_api_impl.rs:proposer_is_the_validator_identity_not_the_mode"],
    witness := [`Rchain.HostList.every_served_entry_is_valid,
      `Rchain.HostList.every_served_peer_is_a_proposer,
      `Rchain.HostList.a_failed_single_probe_is_absent],
    falsifiable := "Dropping `proposer` from `valid` serves an observer that reports \
      `manualPropose: true` (`an_observer_is_not_served`, and `an_observer_is_not_served` in the \
      corpus); dropping `networkId` serves another chain's node; dropping the address check serves a \
      co-hosted validator under its neighbour's identity; treating a failed probe as success serves a \
      peer that did not answer. The `proposer` field itself is falsified at the source: \
      `manual_propose` is `!autopropose && !proposeOnDeploy`, so an unkeyed observer reports it \
      `true`, exactly like a manual-mode validator — the test pins that both report the same mode and \
      only the key separates them.",
    note := "**What the model does not claim.** `proposer` is a peer's own claim over plain HTTP, so \
      an on-path observer can forge a well-formed answer; the list is advisory and a client \
      re-verifies. Host-class filtering (a public node never probes a private peer, Law 62's rule), \
      DNS pinning, the body and field bounds, and the redirect refusal are Rust-tested \
      implementation facts rather than modelled. The probe adds no new host to the node's egress: it \
      dials a port on a host the node already dials under mutual TLS, and the request carries no \
      caller- or peer-supplied URL. **Rollout:** a node older than the `proposer` field lacks it and \
      is excluded until upgraded; safe, temporarily a shorter list." },

  { number := 90, clause := "b", layer := "Node",
    statement := "**The list is bounded, deduplicated and ordered.** At most `maxEntries` entries, at \
      most one per node id, the serving node first when it can propose, and the rest ascending by \
      node id — the response is a function of the bound, not of the network's size, and its order is \
      stable across cycles",
    status := .provedTied,
    corpus := some "hostlist",
    declarations := [`Rchain.HostList.servedIds, `Rchain.HostList.peerPart,
      `Rchain.HostList.the_list_is_bounded, `Rchain.HostList.self_is_first,
      `Rchain.HostList.self_is_present_when_a_proposer],
    axioms := [],
    rust := ["node/src/web/node_list.rs"],
    rustWitness := ["node/src/web/node_list.rs:the_list_is_sorted_and_bounded",
      "node/src/web/node_list.rs:a_duplicate_connection_is_served_once",
      "node/src/web/node_list.rs:self_is_first_and_flagged",
      "node/src/web/node_list.rs:self_is_absent_when_not_a_proposer"],
    witness := [`Rchain.HostList.the_list_is_bounded, `Rchain.HostList.self_is_first,
      `Rchain.HostList.self_is_present_when_a_proposer],
    falsifiable := "Removing the `take` makes the list a function of the peer count; sorting after \
      truncation drops self or reorders it; skipping the id dedupe serves a peer twice when two \
      connections name it. `truncation_self_first` and `duplicate_ids` in the corpus pin the first \
      and third.",
    note := "Ascending order is **by construction** — the peer part is walked over ascending ids — so \
      it needs no sort theorem; the corpus's expected id list is what ties the order to the Rust's \
      lexicographic `BTreeMap` order, which coincides for the fixed-length hex ids the node uses. \
      `latestBlockNumber` and `acceptsDeploys` are served and deliberately **not** filtered on: a \
      lagging or full proposer is still deploy-capable, and there is no oracle for a lag threshold." },

  { number := 90, clause := "c", layer := "Node",
    statement := "**The list is fresh.** It is derived only from the probes of the cycle that \
      produced it; a peer whose probe fails in a cycle is absent from that cycle's list, whatever the \
      previous cycle served. There is no cache that could revive an entry",
    status := .provedTied,
    corpus := some "hostlist",
    declarations := [`Rchain.HostList.peerPart, `Rchain.HostList.servedIds,
      `Rchain.HostList.a_failed_single_probe_is_absent],
    axioms := [],
    rust := ["node/src/web/node_list.rs"],
    rustWitness := ["node/src/web/node_list.rs:an_unreachable_peer_is_not_served",
      "node/src/web/node_list.rs:a_public_node_does_not_probe_a_private_peer"],
    witness := [`Rchain.HostList.a_failed_single_probe_is_absent],
    falsifiable := "Keeping a per-peer success cache across cycles revives a peer that has gone away; \
      treating a timed-out probe as success serves it immediately. The `unreachable` corpus case pins \
      the derivation; the provider rebuilds its snapshot from each cycle's outcomes and never reads \
      the previous one.",
    note := "The only freshness window is the previous snapshot being served while the next cycle \
      runs, bounded by one probe interval and documented. The provider's snapshot is swapped whole; \
      no lock is held across a probe, and the route takes no caller input." },

  { number := 91, clause := "a", layer := "Rholang",
    statement := "**A canonicalisation or a union charges the elements it canonicalises.** A \
      collection the reducer builds (`toSet`, `toMap`, `set`, `keys`) charges three phlo per element \
      of the output it is about to build, charged from the operand size before the work; a set or \
      map union charges both operands, so the charge is a function of the total element count and \
      does not depend on which operand is which",
    status := .provedModel,
    declarations := [`Rchain.canonicalizationCost, `Rchain.unionCost,
      `Rchain.canonicalizationCost_covers_the_operand, `Rchain.unionCost_is_the_sum_of_its_operands,
      `Rchain.unionCost_symmetric, `Rchain.rightOnlyUnionCost,
      `Rchain.rightOnlyUnionCost_is_not_symmetric],
    axioms := [],
    rust := ["rholang/src/reduce.rs", "rholang/src/accounting.rs"],
    rustWitness := ["rholang/src/reduce.rs:to_set_charges_its_elements",
      "rholang/src/reduce.rs:to_map_charges_its_elements",
      "rholang/src/reduce.rs:keys_charges_its_output",
      "rholang/src/reduce.rs:set_charges_the_map_it_rebuilds",
      "rholang/src/reduce.rs:a_set_add_charges_the_set_it_rebuilds",
      "rholang/src/reduce.rs:a_union_charges_both_operands_alike",
      "rholang/src/accounting.rs:collection_and_censure_charges_are_the_models"],
    witness := [`Rchain.canonicalizationCost_covers_the_operand, `Rchain.unionCost_symmetric],
    falsifiable := "The charge this replaced was flat — 18 phlo for a `toSet` of 500 elements and \
      of 40 000, while the sort's wall time grew from 7 ms to 724 ms — and the union charged the \
      right operand only, which `rightOnlyUnionCost_is_not_symmetric` exhibits as asymmetric at one \
      pair. Reverting either makes the corresponding witness test fail: `to_set_charges_its_elements` \
      sees a zero marginal charge, and `a_union_charges_both_operands_alike` sees the two arms \
      differ.",
    note := "**The log factor is named rather than hidden.** A canonicalisation sorts, so the work \
      is `O(n log n)` and the charge is linear: a phlo buys work bounded by `charge · log charge`, \
      which is bounded — the defect C345 names is *unbounded* work for a flat charge — but it is not \
      an exact comparison count, and this row does not claim one. **Not charged, and deliberately \
      so**: a collection literal the evaluator builds stays uncharged, because its bytes were \
      charged at parse and the create path is not attacker-sized. The tie is prose plus the Rust \
      witness, not a corpus: the cost is a closed form, so a corpus of the same numbers would \
      restate the witness. Consensus-visible (#51 category A): the charge moves refund and burned \
      amounts, hence every post-state hash. **The row was true and the code was not, in one arm**: \
      `set.add`/`set + x` charged a flat 3 phlo for the same re-canonicalisation until the cost \
      register's first census found it (C353). The statement did not need to change — the arm needed \
      to join it, which is what a law that is checked against a census can say and a law checked \
      against its own witnesses cannot." },

  { number := 91, clause := "b", layer := "Rholang",
    statement := "**The censure charge bounds the cubic fold.** `rho:gov:censure` charges `(U+1)³` \
      phlo for a member universe of `U` — a charge that covers every member and grows with the \
      universe — where the fold's fixed point runs at most `U+1` rounds over `U` members",
    status := .provedModel,
    declarations := [`Rchain.censureCost, `Rchain.censureCost_exceeds_the_universe,
      `Rchain.censureCost_monotone, `Rchain.censureCost_is_not_the_square],
    axioms := [],
    rust := ["rholang/src/system_processes.rs", "rholang/src/accounting.rs"],
    rustWitness := ["rholang/src/system_processes.rs:censure_is_charged_the_cube_of_its_universe",
      "rholang/src/accounting.rs:collection_and_censure_charges_are_the_models"],
    witness := [`Rchain.censureCost_exceeds_the_universe, `Rchain.censureCost_monotone],
    falsifiable := "The charge this replaced was `U²` for a `Θ(U³)` fixed point; \
      `censureCost_is_not_the_square` exhibits a universe at which the two differ, and \
      `censure_is_charged_the_cube_of_its_universe` reads the handler's own cost cell at two \
      universes, so a revert to the quadratic table fails there rather than in an estimate.",
    note := "**The bound is measured, not chosen.** `MAX_GOV_UNIVERSE` is 512, and the cubic path at \
      it was timed in release (`rholang/tests/gas_amplification.rs`): 33 ms for a 512-member levels \
      map, 1–5 ms for the three mixed shapes — so the constant stays and the `(512+1)³` = 1.35e8 phlo \
      it charges is the same order as the time it buys under the table's one-phlo-per-operation \
      calibration. The bound is on the **sum** of censures, levels and vouchers, which is what stops \
      a deep cascade: a discredit needs a quorum of about two thirds of the eligible set, and the \
      universe counts the censure pairs that must supply it. **The other three `rho:gov:*` folds \
      keep the squared charge**, and their work shapes were measured afterwards (C354): `trustLevels` \
      converges in at most five rounds because a conferral cannot raise a level past the rater's, and \
      `resolveWeights` walks each member's delegation chain — both quadratic, so the squared charge \
      matches the shape. The measurement was recorded in the cost register (`spec/COST.md`) rather \
      than left as a residual." },

  { number := 91, clause := "c", layer := "Rholang",
    statement := "**A tally charges the option lists it scans.** A ranked tally charges the worst \
      case — one scan per ranked entry per round, at most `longest` rounds — and an approval tally \
      charges every approved entry. The member maps keep the universe bound; the lists are priced \
      rather than refused",
    status := .provedModel,
    declarations := [`Rchain.tallyCost, `Rchain.tallyCost_covers_the_entries,
      `Rchain.tallyCost_is_the_worst_case],
    axioms := [],
    rust := ["rholang/src/system_processes.rs", "rholang/src/accounting.rs"],
    rustWitness := ["rholang/src/system_processes.rs:a_tally_charges_the_option_lists_it_scans",
      "rholang/src/accounting.rs:the_tally_charges_are_the_models"],
    witness := [`Rchain.tallyCost_covers_the_entries, `Rchain.tallyCost_is_the_worst_case],
    falsifiable := "The charge this replaced was `(ballots + weights)²`, a function the option lists \
      do not appear in at all; `a_tally_charges_the_option_lists_it_scans` reads the handler's own \
      cost cell for the same election at two option-list sizes and the difference is zero under that \
      charge, so a revert fails there. The probe measures the work the old charge ignored: 34 ms for \
      a ranked tally of 100 ballots over 4 000 options and 68 ms for an approval tally over 4 000.",
    note := "**Priced, not refused.** The member maps keep `MAX_GOV_UNIVERSE`; the option lists are \
      bounded by the charge itself — a ballot map whose lists are a million entries deep is priced \
      past the block phlo cap instead of being run — because a refusal here would reject a legitimate \
      election (a few hundred ballots over ten options) to stop one that the phlo budget already \
      stops. The ranked worst case is `longest × entries`: every round scans every entry, and a round \
      may eliminate only one option, so `longest` bounds the rounds. The floor is the approval side. \
      Consensus-visible (#51 category A)." },

]

def laws : List Law := lawsA ++ lawsB ++ lawsC

/-- Every law number the catalog defines. Laws with clauses repeat. -/
def numbers : List Nat := laws.map (·.number)

/-- The axioms claimed by some law. `Rchain/LawsMain.lean` requires this to equal the tree's `axiom`
set exactly — the ratchet that makes a new axiom impossible to add without a row that justifies it. -/
def claimedAxioms : List Lean.Name := (laws.map (·.axioms)).join

/-- The declarations every row cites, for the reference-integrity check. -/
def citedDeclarations : List Lean.Name := (laws.map (·.declarations)).join

end Laws
end Rchain
