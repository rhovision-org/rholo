import Rchain.Laws.Base

/-!
# The law register, chunk 03 (laws 30–43)

One module of the register catalog. The register is split across modules because elaborating it as a
single module peaked at ~15 GiB on the 16 GiB CI runner (measured 2026-11-23); a module boundary is a
process boundary, so each chunk's elaboration memory is bounded. The register's numbering check
(`1..lawCeiling`) and `entryCeiling` make a dropped or duplicated row across chunk boundaries a build
failure. See `Rchain/Laws.lean` for the assembly and the measurement.
-/

set_option autoImplicit false
set_option maxHeartbeats 4000000

namespace Rchain
namespace Laws

def lawsChunk03 : List Law := [
  -- ── Laws 30–43: the surface the ten silent defects live in ──────────────────────────────────────

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
]

end Laws
end Rchain
