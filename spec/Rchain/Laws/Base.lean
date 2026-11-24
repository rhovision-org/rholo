/-!
# The law register — the shared vocabulary

The `Status` vocabulary and the `Law` structure every register row is built from. The rows themselves
live in `Rchain/Laws/Chunk01..09.lean`, one module per law-number range; `Rchain/Laws.lean` assembles
them, and `Rchain/LawsMain.lean` checks the tree against them.

**Why the chunks are modules.** A single module holding all 187 entries elaborated at 15.1 GiB RSS on
the 16 GiB CI runner (measured 2026-11-23: `ps` peak 15,102,104 KiB). A row is mostly prose —
`statement`, `note`, `falsifiable` — held as string literals, and the elaborator's cost grows with the
register. A `def` boundary inside one module does not bound that memory: one `lean` process elaborates
every declaration in the module. A module boundary does, because Lake invokes a separate `lean` process
for each module, so the register is split at law-number boundaries.

Each chunk raises `maxHeartbeats` to 4,000,000 for the reason the original literal did (2026-10-08): a
row's elaboration is a unification problem over the structure's field types rather than a proof, and
one list fit Lean's 200,000 default at 58 laws and not at 59. The ceiling stays with the chunks; a
further raise is a signal about the register's shape, not a knob — the prose is the record, and a
`note` cut to fit a compiler budget is a note that stopped saying what it knows.

`tools/measure-lean-memory.sh` reports each chunk's peak, and the formal gate refuses a peak over its
budget rather than letting the runner's OOM killer decide.
-/

/-!

`spec/INVENTORY.md` is the prose catalog, and `docs/src/formal/laws.md`
— the folder's one-page rendering of the whole set — is its reader-facing counterpart, but
none of the three is *checkable*:
nothing noticed that the tree grew to 43 laws while both still
said 29, that the two tables contradicted each other on Laws 5 and 24, or that Law 1's "30 residual
axioms" were really 12. The register is the single source of truth those documents are generated from,
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

**The formal rung (2026-11-16).** A row may also name `formal` — the law's proposition, as a theorem or
a `def … : Prop` — with `mutant` (the rule being weakened: a hypothesis dropped, `>` weakened to `>=`,
a projection removed) and `falsifier` (a theorem whose type *mentions* the mutant, checked with
`Expr.getUsedConstants`). `Rchain/LawsMain.lean`'s check 10 resolves all three. The retrofit is a
ratchet: `formalMissingCeiling` is 152 and may only fall, and the first five rows are 104 (the
weighted checkpoint quorum), 72b (a completion record's release), 63b (the relay's conditional),
74b (a soft checkpoint's base) and 113 (a merge above a finalised base).
The declaration is now the statement, not a description of it.
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
  /-- **The law's proposition, as a named declaration of type `Prop`.** The register's `statement` is
  prose, and `declarations` are names: neither says *which term is the law*. This field does, so the
  statement can be checked rather than read. `Rchain/LawsMain.lean`'s check 10 requires the name to
  resolve to a theorem or a `def` of type `Prop`. `none` means the row has not been retrofitted; the
  ratchet is `formalMissingCeiling`, which may only fall. -/
  formal : Option Lean.Name := none
  /-- **The mutant the falsifier refutes**, when the falsifier is a mutation rather than a plain
  counterexample: the law's rule with a hypothesis dropped, a strict inequality weakened, or a
  projection removed. Check 10 requires it to be a `Prop`-typed declaration and requires the
  `falsifier`'s type to mention it — so a witness cannot be an unrelated theorem wearing the row's
  name. -/
  mutant : Option Lean.Name := none
  /-- **The theorem that makes the mutation fail.** Its type must mention `mutant` (or `formal` when no
  mutant is named); check 10 resolves both names, and check 4c's "must be a theorem" discipline is
  applied to it too. This is the machine half of "clearly falsifiable": the register now points at the
  proposition and at the theorem that breaks its mutant, not only at prose. -/
  falsifier : Option Lean.Name := none

end Laws
end Rchain
