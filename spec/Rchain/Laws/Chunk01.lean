import Rchain.Laws.Base

/-!
# The law register, chunk 01 (laws 1–14)

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

def lawsChunk01 : List Law := [
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
      `conformance/sort.tsv`'s 42 verdicts, each `decide`d against the model's `cmpPar`, read back from \
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
      (below) and the other five were constructors (further below) — so the count is closed. **The \
      decimal class (`edecimal`, tag 14) joined on 2026-10-19 and was retired on 2026-10-31** (issue \
      #527, option R): the model is 29 classes covering the node's 33 variants, and rows 43–50 are \
      gone with the type. **Three of those eight were *conflations* rather than missing arms, and all \
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
      10 cross lemmas, and five arms in each of the three laws), and `emethod`'s score needed \
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
      **What the tie does not claim (2026-10-19, withdrawn 2026-10-31).** The retired `edecimal` \
      payload was the canonical scaled integer, and the bridge from the Rust `(coefficient, scale)` \
      pair to it — `scaled_value`'s injectivity and order-preservation on canonical values — was \
      never a theorem here, and neither were canonicalization, exact `+ - * /`, division's \
      exact-or-error condition, the wire's minimal-byte form, reducer dispatch or the cost model. \
      Law 87 carried that boundary and is now `retired` with the type (issue #527); the archived \
      algebra is `spec/Rchain/Decimal.lean`, and the record is `docs/src/formal/decimal-removed.md`" },
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
  { number := 3, layer := "Rholang",
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
      three, which law 1's row owns and counts. **Witness column (AUDIT C139).** A witness was added \
      on 2026-09-27: the converse, without which the hash half of this law was not falsifiable. The \
      equality-facing witness asserts `equal => same hash`, which a *constant* `Hash` satisfies — \
      measured: an empty `Hash for Sorted` body left both of law 2's witnesses green and the whole \
      `rchain-models` suite green (168 tests). `law2_unequal_canonical_forms_do_not_hash_alike` states \
      the other direction, and it is the one that fails under that mutation" },
  { number := 4, clause := "a", layer := "Rholang",
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
      so the law's statement is bounded by a published disproof rather than an assertion",
    note := "**Witness column (AUDIT C138).** The witness column changed on 2026-09-27, because both \
      names it carried were vacuous. `law3_substituting_a_closed_value_keeps_the_term_closed` asserts \
      closedness before and after the substitution — which the *identity* substitution satisfies — and \
      `law3_substitution_and_sorting_commute` reduces to `sort p = sort p` under the same mutation. \
      Measured: `substitute_par` returning its input leaves both green, while the two now named go red. \
      The right pointer was already in this row's `falsifiable` cell \
      (`an_open_value_at_the_variable_leaves_a_free_variable`); the witness column is where a reader \
      looks to ask whether the law is falsified, so it is the column that had to move" },
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
    formal := `Rchain.law11_statement,
    mutant := `Rchain.law11_mutant,
    falsifier := `Rchain.law11_falsifier,
    declarations := [`Rchain.Comm, `Rchain.produceRefs, `Rchain.commId, `Rchain.Trace, `Rchain.CommRef,
      `Rchain.refOf, `Rchain.refsOf, `Rchain.occurrences, `Rchain.removeOne, `Rchain.Replays,
      `Rchain.forwardHolds, `Rchain.reverseHolds, `Rchain.removeOne_eq_some_iff,
      `Rchain.replays_iff_same_occurrences, `Rchain.a_replay_agrees_with_its_record,
      `Rchain.the_forward_half_alone_admits_a_diverging_trace,
      `Rchain.the_reverse_half_alone_admits_a_phantom_recomputation,
      `Rchain.law11_statement, `Rchain.law11_mutant, `Rchain.law11_falsifier, `Rchain.law11
      ],
    axioms := [],
    rust := ["rspace/src/replay_rspace.rs", "rspace/src/space_matcher.rs"],
    witness := [`Rchain.the_forward_half_alone_admits_a_diverging_trace, `Rchain.the_reverse_half_alone_admits_a_phantom_recomputation, `Rchain.a_replay_agrees_with_its_record,
      `Rchain.law11_falsifier
      ],
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
]

end Laws
end Rchain
