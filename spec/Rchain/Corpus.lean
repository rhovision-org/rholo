import Rchain.Par
import Rchain.Sort
import Rchain.Surface
import Rchain.Match
import Rchain.Silence
import Rchain.Store
import Rchain.Protocol
import Rchain.Json
import Rchain.Syrup
import Rchain.Envelope
import Rchain.Lex
import Rchain.Parse
import Rchain.Casper.Validate
import Rchain.Casper.Rounds
import Rchain.Progress
import Rchain.Admission
import Rchain.HostList
import Rchain.Decimal
import Rchain.Disposition

/-!
# The conformance corpus, generated from the specification

`lake exe rchain-corpus --layer flags --out …` prints the corpus the Rust is checked against, 1:1.
Each case carries its own source text and the *pattern the model believes that text denotes*: the
verdict is the model's (`decide`d here, so `lake build` fails if the model and the case disagree), and
the Rust consumer parses the same source and computes its own verdict, so a source that drifted from the
model fails the check instead of being trusted. Two independent parties, one source of truth each.

Law 35 (concreteness soundness) is the first layer, because the silent defects of AUDIT C9-C22 all pass
through one predicate: `spatial_match`'s first line consults `pattern.connective_used` and short-circuits
to structural equality when it is false. So a collection pattern whose only non-concreteness is a
`..._`/`...rest` remainder either matches partially or never matches at all, depending on a flag set far
away in the normalizer — which is exactly C22 item 3, and C19/C20's root cause.
-/

namespace Rchain
namespace Corpus

/-- The number of cases the flags layer carries, stated here so the Rust consumer can assert it read
every one (a corpus that silently shrinks is a check that stopped checking). -/
def flagCaseCount : Nat := 17

/-- A corpus case: the receive bind's source, the pattern the model believes it denotes, and the
verdict `connectiveUsed` must return for that pattern. -/
structure FlagCase where
  /-- The bind as rholang spells it — `@<pattern>` for a collection (a collection is not a *name*), bare
  for a wildcard or a name. The Rust consumer wraps it as `for (<bind> <- chan) { Nil }` and reads the
  pattern's verdict. -/
  source : String
  /-- The pattern, as the model sees it. -/
  par : Par
  /-- What `connectiveUsed` must return. -/
  expected : Bool

/-- A `Par` holding one expression. -/
def one (e : Expr) : Par := Par.mk [] [] [] [e] [] [] [] []

/-- A map pattern. -/
def mapPat (kvs : List (Par × Par)) (r : Option Var) : Par := one (.emap kvs r)

/-- A list pattern. -/
def listPat (ps : List Par) (r : Option Var) : Par := one (.elist ps r)

/-- A set pattern. -/
def setPat (ps : List Par) (r : Option Var) : Par := one (.eset ps r)

/-- A tuple, as a pattern or as a datum — the shape whose matcher clause the model was missing
    (AUDIT C44): the port's `spatial_match` has an `ETuple` arm (`spatial_matcher.rs:538-544`) and the
    model's clauses did not, so a tuple pattern matched nothing in the model while the node matches
    it. The cases below are what pins it. -/
def tuplePat (ps : List Par) : Par := one (.etuple ps)

/-- An integer. -/
def intPar (n : Int) : Par := one (.ground (.int n))

/-- A string. -/
def strPar (s : String) : Par := one (.ground (.str (s.toList.map Char.toNat)))

/-- A name, as a pattern variable. -/
def namePar (n : Nat) : Par := one (.evar (.free n))

/-- The wildcard. -/
def wildPar : Par := one (.evar .wildcard)

/-- The wildcard remainder (`..._`). -/
def wildRem : Option Var := some .wildcard

/-- A named remainder (`...rest`). -/
def namedRem : Option Var := some (.free 0)

/-- The cases. The first three are the same shape with and without a tail: a *map* was the one form the
port forgot to mark non-concrete (C22 item 3), and it hid behind the cases that all carried a free
variable as well as a remainder. The last two are the production shapes the audit chased:
`Inbox.rho:141`'s `[type, subtype, map /\ {a: b, ..._}]` and `Group.rho:140`'s nested partial maps. -/
def flagCases : List FlagCase :=
  [ { source := "@{}", par := mapPat [] none, expected := false }
  , { source := "@{\"x\": 1}", par := mapPat [(strPar "x", intPar 1)] none, expected := false }
  , { source := "@{\"x\": 1, \"y\": 2}",
      par := mapPat [(strPar "x", intPar 1), (strPar "y", intPar 2)] none, expected := false }
  , { source := "@{\"x\": 1, ..._}", par := mapPat [(strPar "x", intPar 1)] wildRem,
      expected := true }
  , { source := "@{\"x\": 1, ...rest}", par := mapPat [(strPar "x", intPar 1)] namedRem,
      expected := true }
  , { source := "@{..._}", par := mapPat [] wildRem, expected := true }
  , { source := "@[]", par := listPat [] none, expected := false }
  , { source := "@[1, 2]", par := listPat [intPar 1, intPar 2] none, expected := false }
  , { source := "@[1, ..._]", par := listPat [intPar 1] wildRem, expected := true }
  , { source := "@[1, ...rest]", par := listPat [intPar 1] namedRem, expected := true }
  , { source := "@Set()", par := setPat [] none, expected := false }
  , { source := "@Set(1, 2)", par := setPat [intPar 1, intPar 2] none, expected := false }
  , { source := "@Set(1, ..._)", par := setPat [intPar 1] wildRem, expected := true }
  , { source := "@{\"x\": v7}", par := mapPat [(strPar "x", namePar 7)] none, expected := true }
  , { source := "_", par := wildPar, expected := true }
  , { source := "@[{\"x\": 1, ..._}]",
      par := listPat [mapPat [(strPar "x", intPar 1)] wildRem] none, expected := true }
  , { source := "@{\"g\": {\"info\": {\"x\": 1, ..._}, ..._}, ..._}",
      par := mapPat
        [(strPar "g", mapPat [(strPar "info", mapPat [(strPar "x", intPar 1)] wildRem)] wildRem)]
        none,
      expected := true }
  ]

/-- Every case's verdict holds of the model. `decide` discharges all of them because the predicate is
mutually *structural* recursion — each call takes a field of its argument — so the kernel reduces it by
computation. That is why `connectiveUsed` carries no `termination_by` clause: a well-founded definition
would not be reducible, and this theorem could not be closed by `decide`. -/
theorem flagCases_decide : flagCases.all (fun c => connectiveUsed c.par == c.expected) = true := by
  decide

/-- The layer carries exactly `flagCaseCount` cases. -/
theorem flagCases_length : flagCases.length = flagCaseCount := by decide

/-- One corpus line: layer, the source, the verdict. Tab-separated rather than JSON so the consumer
needs no dependency, and so a pattern's punctuation never has to be escaped. -/
def flagLine (c : FlagCase) : String :=
  "flags\t" ++ c.source ++ "\t" ++ (if c.expected then "true" else "false")

/-! ## Law 37 — the matching layer

Each case is a *pattern* (as a receive bind spells it — `@` for a collection, since a collection is
not a name) and a *target*, both as rholang source, plus the verdict `spatialMatch` gives them. The
Rust consumer runs `target` into a channel, receives with `pattern`, and must get the same verdict.
The two shapes the ten incidents lived in are here: a partial map with a ground entry (C19/C20) and a
partial map whose only non-concreteness is the remainder (C22 item 3), each with the shape that must
*not* match beside it. -/

/-- The number of cases the matching layer carries. -/
def matchCaseCount : Nat := 22

/-- One matching case: the bind's source, the target's source, the model's view of both, and the
verdict. The verdict is `decide`d against `spatialMatch` (`matchCases_decide`), which is what makes
this corpus a law's corpus rather than a table of opinions. -/
structure MatchCase where
  /-- The receive bind, as rholang spells it. -/
  bind : String
  /-- The datum, as rholang spells it. -/
  target : String
  /-- The pattern, as the model sees it. -/
  patternPar : Par
  /-- The target, as the model sees it. -/
  targetPar : Par
  /-- What `spatialMatch` must say. -/
  expected : Bool
  /-- Whether the row is really about *rejection* rather than matching: a pattern that binds a level
  twice is refused by the normalizer (loudly, `UnexpectedReuseOfProcContextFree`) before any match
  happens, so its honest verdict is neither `true` nor `false`. The Rust consumer asserts the refusal
  for these rows — that is law 5 in the port, where the Scala's `addedVars.distinct` runs. -/
  rejected : Bool := false

/-- A map pattern with string keys. -/
def mapOf (kvs : List (String × Par)) (r : Option Var) : Par :=
  mapPat (kvs.map (fun kv => (strPar kv.1, kv.2))) r

/-- The cases. Case 3 is the rgov gate's own shape (`{"read": *MCAread, ..._}` against a three-key
dictionary); case 13 is law 5's linearity — the shape the old axiom `pattern_binds_at_most_once`
declared impossible (AUDIT C26); case 14 is the same shape with *two* distinct variables, which that
axiom denied outright. -/
def matchCases : List MatchCase :=
  [ { bind := "@{\"x\": 1}", target := "{\"x\": 1}",
      patternPar := mapOf [("x", intPar 1)] none,
      targetPar := mapOf [("x", intPar 1)] none, expected := true }
  , { bind := "@{\"x\": 1}", target := "{\"x\": 1, \"y\": 2}",
      patternPar := mapOf [("x", intPar 1)] none,
      targetPar := mapOf [("x", intPar 1), ("y", intPar 2)] none, expected := false }
  , { bind := "@{\"x\": 1, ..._}", target := "{\"x\": 1, \"y\": 2}",
      patternPar := mapOf [("x", intPar 1)] wildRem,
      targetPar := mapOf [("x", intPar 1), ("y", intPar 2)] none, expected := true }
  , { bind := "@{\"x\": 1, ...rest}", target := "{\"x\": 1, \"y\": 2}",
      patternPar := mapOf [("x", intPar 1)] namedRem,
      targetPar := mapOf [("x", intPar 1), ("y", intPar 2)] none, expected := true }
  , { bind := "@{\"x\": 9, ..._}", target := "{\"x\": 1, \"y\": 2}",
      patternPar := mapOf [("x", intPar 9)] wildRem,
      targetPar := mapOf [("x", intPar 1), ("y", intPar 2)] none, expected := false }
  , { bind := "@{\"read\": v7, ..._}", target := "{\"read\": 1, \"write\": 2, \"grant\": 3}",
      patternPar := mapOf [("read", namePar 7)] wildRem,
      targetPar := mapOf [("read", intPar 1), ("write", intPar 2), ("grant", intPar 3)] none,
      expected := true }
  , { bind := "@[1, ..._]", target := "[1, 2, 3]",
      patternPar := listPat [intPar 1] wildRem,
      targetPar := listPat [intPar 1, intPar 2, intPar 3] none, expected := true }
  , { bind := "@[1]", target := "[1, 2]",
      patternPar := listPat [intPar 1] none,
      targetPar := listPat [intPar 1, intPar 2] none, expected := false }
  , { bind := "@Set(1, ..._)", target := "Set(1, 2)",
      patternPar := setPat [intPar 1] wildRem,
      targetPar := setPat [intPar 1, intPar 2] none, expected := true }
  , { bind := "@[]", target := "[]",
      patternPar := listPat [] none, targetPar := listPat [] none, expected := true }
  , { bind := "@{}", target := "{\"a\": 1}",
      patternPar := mapPat [] none,
      targetPar := mapOf [("a", intPar 1)] none, expected := false }
  , { bind := "@{..._}", target := "{\"a\": 1}",
      patternPar := mapPat [] wildRem,
      targetPar := mapOf [("a", intPar 1)] none, expected := true }
  , { bind := "@{\"a\": {\"b\": 1, ..._}, ..._}", target := "{\"a\": {\"b\": 1, \"c\": 2}, \"d\": 3}",
      patternPar := mapOf [("a", mapOf [("b", intPar 1)] wildRem)] wildRem,
      targetPar := mapOf [("a", mapOf [("b", intPar 1), ("c", intPar 2)] none), ("d", intPar 3)] none,
      expected := true }
  , { bind := "@{\"a\": v1, \"b\": v1}", target := "{\"a\": 1, \"b\": 2}",
      patternPar := mapOf [("a", namePar 1), ("b", namePar 1)] none,
      targetPar := mapOf [("a", intPar 1), ("b", intPar 2)] none, expected := false,
      rejected := true }
  , { bind := "@{\"a\": v1, \"b\": v2}", target := "{\"a\": 1, \"b\": 2}",
      patternPar := mapOf [("a", namePar 1), ("b", namePar 2)] none,
      targetPar := mapOf [("a", intPar 1), ("b", intPar 2)] none, expected := true }
    -- 15. the shape the model's clauses had no arm for: a **tuple**. The port matches it
    -- (`spatial_matcher.rs:538-544`), so an equal tuple must match — and until the arm was added the
    -- model answered `false`, which is AUDIT C44.
  , { bind := "@(1, 2)", target := "(1, 2)",
      patternPar := tuplePat [intPar 1, intPar 2],
      targetPar := tuplePat [intPar 1, intPar 2], expected := true }
    -- 16. the same arm, on the negative side: tuples of different arity do not match.
  , { bind := "@(1, 2)", target := "(1, 2, 3)",
      patternPar := tuplePat [intPar 1, intPar 2],
      targetPar := tuplePat [intPar 1, intPar 2, intPar 3], expected := false }
    -- 18. the shape that falsified the fuel's **measure** rather than its constant: a target padded
    -- with `Nil`s, which the *set* member must walk past to find the pattern's counterpart (it
    -- searches — `list_match_single` → `find_matches`, `spatial_matcher.rs:729-815`). `parNodes`
    -- counted a `Par` with an empty `exprs` field as zero nodes, so six `Nil`s were free in the budget
    -- while costing six steps: `matchFuel` was 12, the walk needed 13, and the model answered `false`
    -- where the node answers `true`. AUDIT C47; `Match.lean`'s
    -- `the_walk_past_empty_pars_is_paid_for` is the model-side ratchet. Six `Nil`s is the boundary —
    -- at five the old measure also answered `true`, which is why no unpadded case could find it.
  , { bind := "@Set(1, ..._)", target := "Set(Nil, Nil, Nil, Nil, Nil, Nil, 1)",
      patternPar := setPat [intPar 1] wildRem,
      targetPar := setPat (List.replicate 6 nilPar ++ [intPar 1]) none, expected := true }
    -- 19. the same padded shape in a **list**, on the rejection side: a list is matched positionally
    -- (`fold_match`), so a pattern element cannot skip a leading target element. The model *did* skip
    -- it — the searcher was wired into the list arm, so `@[1, ..._]` matched `[Nil, 1]` in the model
    -- and not on the node: the spec over-claimed a match, the one direction the boundary note says the
    -- corpus exists to catch. AUDIT C48; `Match.lean`'s `a_list_pattern_cannot_skip_a_target_element`.
  , { bind := "@[1, ..._]", target := "[Nil, 1]",
      patternPar := listPat [intPar 1] wildRem,
      targetPar := listPat [nilPar, intPar 1] none, expected := false }
    -- 20. the *second* instance of the measure defect, on another arm: `parNodesExpr` had **no
    -- `etuple` case**, so a tuple's contents were charged to no node and `matchFuel` did not grow with
    -- the nesting — while the tuple clause walks its elements through `matchListPos` exactly as the
    -- list arm does. Each nesting level costs the matcher 4 units and a `Par`-and-expression pair
    -- contributes 4, so `@((1, 2), (3, 4))` against itself needed 13 and was given 12: a `false` where
    -- the node answers `true`, and unlike case 18 **no padding** is needed to expose it. AUDIT C50;
    -- `Match.lean`'s `a_nested_tuple_is_paid_for` is the model-side ratchet.
  , { bind := "@((1, 2), (3, 4))", target := "((1, 2), (3, 4))",
      patternPar := tuplePat [tuplePat [intPar 1, intPar 2], tuplePat [intPar 3, intPar 4]],
      targetPar := tuplePat [tuplePat [intPar 1, intPar 2], tuplePat [intPar 3, intPar 4]],
      expected := true }
    -- 21. **the no-remainder length guard** — a clause, not a measure. A canonical shorter set pattern
    -- against a canonical longer target: the port refuses an unequal length *before* it searches
    -- (`exact_match = !wildcard && remainder.is_none()`, then `if exact_match && plen != tlen`,
    -- `spatial_matcher.rs:684-693`) and the model's walk did not — it drops *leading* targets, so
    -- `@Set(2)` matched `Set(1, 2)` here while the node answers `false`. Both values are canonical
    -- (sorted, duplicate-free), so this is **not** C54's duplicate-element shape: that row was
    -- withdrawn because the node's datum is `Set(1)` after `par_set`, and nothing here depends on
    -- evaluation changing a value. Measured on the node first (`false`), then `decide`d. `Match.lean`'s
    -- `a_shorter_set_pattern_is_refused` is the model-side ratchet, and
    -- `a_set_pattern_of_the_same_length_still_matches` is its control — without it the refusal would be
    -- satisfied by a member that matches nothing.
  , { bind := "@Set(2)", target := "Set(1, 2)",
      patternPar := setPat [intPar 2] none,
      targetPar := setPat [intPar 1, intPar 2] none, expected := false }
    -- 22. the same guard on the **map** arm, which is the same `list_match_single` on the port:
    -- `@{"b": 2}` against `{"a": 1, "b": 2}` — `false` on the node, and `true` in the model until the
    -- guard, because the walk dropped the leading `"a"` pair. `Match.lean`'s
    -- `a_shorter_map_pattern_is_refused`.
  , { bind := "@{\"b\": 2}", target := "{\"a\": 1, \"b\": 2}",
      patternPar := mapOf [("b", intPar 2)] none,
      targetPar := mapOf [("a", intPar 1), ("b", intPar 2)] none, expected := false }
  ]

/-- Every matching case's verdict holds of the model. `decide`, because the clauses are structurally
recursive on fuel — the reason `spatialMatch` carries fuel at all (`Rchain/Match.lean`'s note). -/
theorem matchCases_decide :
    matchCases.all (fun c =>
      if c.rejected then decide (linear c.patternPar = false) else spatialMatch c.targetPar c.patternPar == c.expected)
      = true := by
  decide

/-- The layer carries exactly `matchCaseCount` cases. -/
theorem matchCases_length : matchCases.length = matchCaseCount := by decide

/-- One matching corpus line: layer, the bind, the datum, the verdict. -/
def matchLine (c : MatchCase) : String :=
  "match\t" ++ c.bind ++ "\t" ++ c.target ++ "\t"
    ++ (if c.rejected then "rejected" else if c.expected then "true" else "false")

/-! ## Laws 38 and 40 — the silence layer

Each case is a *term* — a send and a receive on a channel, the receive's body reporting on `@"out"` —
and the verdict `takesStep` gives it: does the pair form a contract step at all? A term that does not
step produces nothing but the consumer's control datum, which is what "silent" means in the port, and
what the rule in `Rchain/Silence.lean` says: the contract rule carries the match as a hypothesis.

The last six cases are **law 40's** (*protocol agreement*: every call has an accepting receive at the
target's arity), and they are here rather than in a layer of their own because the verdict they need is
this one: a call whose arity matches no receive *is* a silent step, so `takesStep` is the same oracle
and a second consumer would be a copy of this one. What law 40 changed is the *rule* — `stepsInBinds`
now requires as many patterns as data, where it used to fail closed on anything but a single datum —
and those cases are what pins the change. -/

/-- The number of cases the silence layer carries (laws 38 and 40). -/
def silenceCaseCount : Nat := 13

/-- A silence case: the term as rholang spells it, the model's view of it, and whether it steps. -/
structure SilenceCase where
  /-- The term's source: a send, a receive whose body reports on `out`, and nothing else. -/
  term : String
  /-- The model's view of that term. -/
  par : Par
  /-- Does it form a contract step? -/
  steps : Bool

/-- The body a silence case's receive runs when it fires: `@"out"!("step")`. -/
def stepBody : Par := sendPar (strPar "out") [strPar "step"]

/-- A send-and-receive term on string channels, with `pattern` as the receive's bind. -/
def pairPar (sendChan : String) (datum : Par) (recvChan : String) (pattern : Par) : Par :=
  -- A `!` send and a `for` receive: both non-persistent, which is the shape the corpus's terms are.
  parMerge (sendParP (strPar sendChan) [datum] false)
    (receiveParP (strPar recvChan) pattern stepBody false)

/-- A send with `data` and a receive with `patterns` on one string channel: the shape a *call* and a
*contract head* make. Law 40's cases are exactly these, with the arities varied. -/
def callPar (data : List Par) (patterns : List Par) : Par :=
  -- A call (`!`, non-persistent) against a **contract head** (replicated) — the flags the node gives
  -- the two shapes, and the reason `ReduceP`'s constructors take them.
  parMerge (sendParP (strPar "c") data false)
    (receiveParPs (strPar "c") patterns stepBody true)

/-- A **join**: a receive with two binds, `for (x <- @"c"; y <- @"d")`, with a datum on the first
channel and none on the second. The node fires a join only when *every* bound channel holds a matching
datum, so this is not a step — and the model's search read one bind at a time, so it said it *was*
(AUDIT C45). Law 38's corpus case 13 is this term. -/
def joinPar (datum : Par) : Par :=
  parMerge (sendParP (strPar "c") [datum] false)
    (Par.mk [] [Receive.mk
        [ReceiveBind.mk [namePar 0] (strPar "c") 1, ReceiveBind.mk [namePar 1] (strPar "d") 1]
        stepBody false 2] [] [] [] [] [] [])

/-- The cases. Case 3 is the shape C22 item 3 turned on, seen from the reduction side rather than the
matcher's; case 5 is a store pair, the single-step half of the C22 item 1 class; cases 4 and 6 are the
two ways a pair fails to be a redex — the wrong channel, and a pattern that does not match. Cases 7-12
are law 40's: a 2-arity call to a two-pattern receive (accepted), 1- and 3-arity calls to the same
receive (silent), the 2-arity call to a *three*-pattern receive — C22 item 2's production instance,
where `extraSlots` called the directory's `write(@key, @value, ret)` with two arguments and none of the
three slots was ever written — the 3-arity call that is accepted, and two equal arities whose
*patterns* do not match (so the arity is not the only thing the rule reads). -/
def silenceCases : List SilenceCase :=
  [ { term := "@\"c\"!(1) | for (x <- @\"c\") { @\"out\"!(\"step\") }",
      par := pairPar "c" (intPar 1) "c" (namePar 0), steps := true }
  , { term := "@\"c\"!(1) | for (@{\"k\": 9} <- @\"c\") { @\"out\"!(\"step\") }",
      par := pairPar "c" (intPar 1) "c" (mapOf [("k", intPar 9)] none), steps := false }
  , { term := "@\"c\"!({\"x\": 1, \"y\": 2}) | for (@{\"x\": 1, ..._} <- @\"c\") { @\"out\"!(\"step\") }",
      par := pairPar "c" (mapOf [("x", intPar 1), ("y", intPar 2)] none) "c"
        (mapOf [("x", intPar 1)] wildRem),
      steps := true }
  , { term := "@\"c\"!(1) | for (x <- @\"d\") { @\"out\"!(\"step\") }",
      par := pairPar "c" (intPar 1) "d" (namePar 0), steps := false }
  , { term := "@\"s\"!({}) | for (@m <- @\"s\") { @\"out\"!(\"step\") }",
      par := pairPar "s" (mapPat [] none) "s" (namePar 0), steps := true }
  , { term := "@\"c\"!(1) | for (@2 <- @\"c\") { @\"out\"!(\"step\") }",
      par := pairPar "c" (intPar 1) "c" (intPar 2), steps := false }
    -- 7. law 40: two patterns, two data — the accepting call.
  , { term := "@\"c\"!(1, 2) | for (@a, @b <- @\"c\") { @\"out\"!(\"step\") }",
      par := callPar [intPar 1, intPar 2] [namePar 0, namePar 1], steps := true }
    -- 8. one datum against two patterns: silent.
  , { term := "@\"c\"!(1) | for (@a, @b <- @\"c\") { @\"out\"!(\"step\") }",
      par := callPar [intPar 1] [namePar 0, namePar 1], steps := false }
    -- 9. three data against two patterns: silent.
  , { term := "@\"c\"!(1, 2, 3) | for (@a, @b <- @\"c\") { @\"out\"!(\"step\") }",
      par := callPar [intPar 1, intPar 2, intPar 3] [namePar 0, namePar 1], steps := false }
    -- 10. C22 item 2's instance: `write!(key, value)` against `write(@key, @value, ret)`.
  , { term := "@\"c\"!(1, 2) | for (@k, @v, @ret <- @\"c\") { @\"out\"!(\"step\") }",
      par := callPar [intPar 1, intPar 2] [namePar 0, namePar 1, namePar 2], steps := false }
    -- 11. the same receive, called at its own arity: accepted.
  , { term := "@\"c\"!(1, 2, 3) | for (@k, @v, @ret <- @\"c\") { @\"out\"!(\"step\") }",
      par := callPar [intPar 1, intPar 2, intPar 3] [namePar 0, namePar 1, namePar 2], steps := true }
    -- 12. equal arities, and a pattern that does not match: the rule reads both.
  , { term := "@\"c\"!(9, {\"x\": 1, \"y\": 2}) | for (@2, @{\"x\": 1, ..._} <- @\"c\") { @\"out\"!(\"step\") }",
      par := callPar [intPar 9, mapOf [("x", intPar 1), ("y", intPar 2)] none]
        [intPar 2, mapOf [("x", intPar 1)] wildRem],
      steps := false }
    -- 13. **a join with one of its two channels filled**: `for (x <- @"c"; y <- @"d")` with a datum on
    -- `@"c"` and none on `@"d"`. The node fires a join only when every bound channel has a matching
    -- datum, so this is silence — and the search read the binds one at a time, so it said `true`. The
    -- pair with the rule, which has no join clause at all, is what made the statement false
    -- (AUDIT C45).
  , { term := "@\"c\"!(1) | for (x <- @\"c\"; y <- @\"d\") { @\"out\"!(\"step\") }",
      par := joinPar (intPar 1), steps := false }
  ]

/-- Every silence case's verdict holds of the model, `decide`d against `takesStep`. -/
theorem silenceCases_decide : silenceCases.all (fun c => takesStep c.par == c.steps) = true := by
  decide

/-- The layer carries exactly `silenceCaseCount` cases. -/
theorem silenceCases_length : silenceCases.length = silenceCaseCount := by decide

/-- One silence corpus line: layer, the term, whether it steps. -/
def silenceLine (c : SilenceCase) : String :=
  "silence\t" ++ c.term ++ "\t" ++ (if c.steps then "true" else "false")

/-! ## Law 41 — the store layer

Each case is a *term* whose store is read **twice** — a `contract` reader called two times, which is
what makes it replicable — and the number of answers that must come back: `2` when the reader restores
what it consumed, `1` when it has consumed the store for good. The model's side of the case is the same
question asked of `Rchain/Store.lean`: `storeSurvives` after one read. The two sides are deliberately
different shapes — the term has the surface syntax and two calls; the model has the flat `Par` and the
replication — and they have to agree on the verdict, which is the corpus's premise.

Case 2 is the defect that started this (AUDIT C22 item 1: `Inbox.rho`'s zero-argument `read`), case 3 is
its repair (`box!(Nil)`: the read is documented to remove the *messages*, not the container), case 4 is
the shape C22 item 3 taught from the matching side — a restore that lives in only one branch of a
`match` is not a restore, and the term makes the datum take the other branch so the model's conservative
verdict and the node's behaviour are the same claim — and case 5 is the restore that goes to the wrong
channel, which is what makes the model compare channels rather than ask whether *any* send is there.

A *peek* is deliberately absent, for the reason the Directory's reads were never this law's defect:
`for (map <<- mapCh)` consumes nothing, so it cannot consume the store. The model's `Receive` has a
*persistent* flag and no peek one, so a peek case would have to state its own verdict rather than derive
it.

Every term declares the reader's name in the `new` (`new box, read in …`), because a name that is
*not* bound may be used only **once**: the second use of a free name is `UnexpectedReuseOfNameContextFree`
— the Scala's `NameNormalizeMatcher.scala:52` and the port's `normalizer.rs:111`, faithfully the same
rule. A `contract`'s own name is not bound by its definition, so a reader called twice must have its
channel in scope. -/

/-- The number of cases the store layer carries. -/
def storeCaseCount : Nat := 5

/-- A store case: the term as rholang spells it, the model's view of it, and whether the store answers
the second read. -/
structure StoreCase where
  /-- The term: a store on `@"box"`, a reader, and exactly two calls of it. Its answers land on
  `@"out"`, one per call that fired. -/
  term : String
  /-- The model's view: the store's datum beside the replicated reader and its continuation. -/
  par : Par
  /-- Does the store answer the second read? -/
  survives : Bool

/-- A store's datum on `"box"`, beside a replicated reader of `"box"` whose continuation is `body`. -/
def storeWith (datum body : Par) : Par :=
  parMerge (sendPar (strPar "box") [datum]) (replicatedRead (strPar "box") (namePar 0) body)

/-- A `match` on the reader's datum whose case body restores the store — a restore that is only in one
branch, kept in `matches` rather than `sends`, which is why the model does not count it. (`Rchain.`-
qualified: this module's own `MatchCase` is the corpus's case record, a different type.) -/
def restoreInOneBranch (datum : Par) : Par :=
  Par.mk [] [] [] [] [Rchain.Match.mk (one (.evar (.free 0)))
    [Rchain.MatchCase.mk (strPar "m") (sendPar (strPar "box") [datum]) 0]] [] [] []

/-- The cases. -/
def storeCases : List StoreCase :=
  [ -- 1. the reader restores the datum it took: the store answers again.
    { term := "new box, read in { box!(\"m\") | contract read(_) = { for (x <- box) { @\"out\"!(*x) | box!(*x) } } | read!(Nil) | read!(Nil) }"
    , par := storeWith (strPar "m") (sendPar (strPar "box") [strPar "m"])
    , survives := true }
  , -- 2. the reader that consumes: AUDIT C22 item 1, `Inbox.rho`'s zero-argument `read`.
    { term := "new box, read in { box!(\"m\") | contract read(_) = { for (x <- box) { @\"out\"!(*x) } } | read!(Nil) | read!(Nil) }"
    , par := storeWith (strPar "m") (sendPar (strPar "out") [strPar "m"])
    , survives := false }
  , -- 3. the repair: the read removes the messages, and puts the emptied container back.
    { term := "new box, read in { box!([\"m\"]) | contract read(_) = { for (items <- box) { @\"out\"!(*items) | box!(Nil) } } | read!(Nil) | read!(Nil) }"
    , par := storeWith (listPat [strPar "m"] none) (sendPar (strPar "box") [listPat [] none])
    , survives := true }
  , -- 4. the restore is in one branch of a match, and the datum takes the other.
    { term := "new box, read in { box!(\"z\") | contract read(_) = { for (x <- box) { @\"out\"!(*x) | match *x { \"m\" => { box!(*x) } _ => { Nil } } } } | read!(Nil) | read!(Nil) }"
    , par := storeWith (strPar "z")
        (parMerge (sendPar (strPar "out") [strPar "z"]) (restoreInOneBranch (strPar "z")))
    , survives := false }
  , -- 5. the restore goes somewhere else: a datum put back on a *different* channel is not a restore,
    --    which is why the model compares channels rather than asking whether *any* send is there.
    { term := "new box, read in { box!(\"m\") | contract read(_) = { for (x <- box) { @\"out\"!(*x) | @\"boxx\"!(*x) } } | read!(Nil) | read!(Nil) }"
    , par := storeWith (strPar "m") (sendPar (strPar "boxx") [strPar "m"])
    , survives := false }
  ]

/-- Every store case's verdict holds of the model, `decide`d against `storeSurvives`. -/
theorem storeCases_decide :
    storeCases.all (fun c => storeSurvives (strPar "box") c.par == c.survives) = true := by
  decide

/-- The layer carries exactly `storeCaseCount` cases. -/
theorem storeCases_length : storeCases.length = storeCaseCount := by decide

/-- One store corpus line: layer, the term, and how many reads of the store the node must answer — two
calls in every term, so a store that survives answers both. -/
def storeLine (c : StoreCase) : String :=
  "store\t" ++ c.term ++ "\t" ++ (if c.survives then "2" else "1")

/-! ## Law 34 — the value-position rule (AUDIT C21)

`normalizeAt` threads the **accumulator** now (2026-09-24, G6), so "a value position is normalized
against an empty `par`" is a statement about a definition that could get it wrong: the ambient C21 leaked
into the condition *exists* in the model, and this layer is what refuses it. Each case is a term whose
`if` is *not* first in its `par` — so the `if` receives a non-empty ambient, which is the shape the defect
needed — plus that `if`'s condition, and each party normalizes the two source strings itself. A model
whose `ifThen` passed its ambient into the condition fails `c21Holds` on case 1, which is the falsifier
this layer now is rather than the tie it was: before the accumulator existed the defect was
*unrepresentable* here, and the row said so.

- The **model's** half is `decide`d here: the `Match` an `if` desugars into has the condition's
  normalization as its target, *and* the term's normalization is not the condition — the non-degeneracy
  half, so a case that accidentally was its own condition fails instead of passing quietly.
- The **node's** half is `rholang/tests/lean_c21_corpus.rs`: it parses both strings, normalizes them, and
  asserts the same relation.

The two sides never compare representations across the boundary — each compares terms it normalized
itself — so what ties them is the source text, as in every layer. Under the C21 defect the node's target
was `@"c"!(["a"]) | (1 == 1)` rather than `(1 == 1)`, which fails the node's half of every case here
while the model's half stays true: the corpus is built so that the *tie* is what breaks, and the
regression that motivated it (`normalizer.rs`'s `normalize_if` seeding the condition with `input.par`)
fails case 1 immediately.

Case 3 is the explicit `match` control — the desugaring that was never broken, which is the evidence that
this was `normalize_if`'s bug and not the rule's absence. Case 5's condition is a ground rather than an
expression, so the layer does not depend on the arithmetic clauses agreeing. -/

/-- The number of cases the c21 layer carries. -/
def c21CaseCount : Nat := 5

/-- A law-34 case: a term whose `if` is not first in its `par`, the `if`'s condition, and the model's
    view of each. The two source strings are what the Rust consumer reads; the two `Surf`s are the
    model's view of that same text, which is what every layer's cases carry. -/
structure C21Case where
  /-- The term, as rholang spells it: a statement followed by an `if` (or the explicit-`match`
      control). A *program* — quoted channels, so no free variable is left for the node to refuse. -/
  source : String
  /-- The `if`'s condition, as rholang spells it. Normalized alone, this is what the desugared
      `Match`'s target must be; for the `match` control it is the `match`'s target. -/
  condition : String
  /-- The term, as the model sees it. -/
  whole : Surf
  /-- The condition, as the model sees it. -/
  cond : Surf

/-- A string literal — the surface carries the raw literal, quotes included. -/
def sStr (raw : String) : Surf := .ground (.str raw)

/-- An integer literal. -/
def sInt (digits : String) : Surf := .ground (.int digits)

/-- `@"<channel>"!(data)`. -/
def sQuoteSend (channel : String) (data : List Surf) : Surf :=
  .send (.quote (sStr ("\"" ++ channel ++ "\""))) false [.collect (.list data none)]

/-- `@"out"!(msg)`. -/
def sOut (msg : String) : Surf := sQuoteSend "out" [sStr msg]

/-- `1 == 1` — the condition the expression cases share. -/
def sAlwaysTrue : Surf := .eq (sInt "1") (sInt "1")

/-- Whether two terms are the same, by law 1's canonical comparator — whose `eq_iff` is *proved*
    (`cmpPar_eq_iff`), so a `Bool` built from it means equality and not merely a hash. -/
def samePar (p q : Par) : Bool :=
  match cmpPar p q with
  | .eq => true
  | _ => false

/-- The target of a normalized term's sole `Match` — the position an `if` desugars into. -/
def soleTarget : Option Par → Option Par
  | some p =>
    match p.matches with
    | [Match.mk t _] => some t
    | _ => none
  | none => none

/-- **The value-position rule for one case**: the target is the condition, normalized alone — not the
    condition *preceded by* the terms before it, which is what C21 put there. -/
def c21Holds (c : C21Case) : Bool :=
  match soleTarget (normalizeAt c.whole nilPar []), normalizeAt c.cond nilPar [] with
  | some t, some q => samePar t q
  | _, _ => false

/-- The case is a **probe** rather than its own condition: the term normalizes to something other than
    the condition, so "the target is the condition" cannot hold of the whole by accident. -/
def c21IsProbe (c : C21Case) : Bool :=
  match normalizeAt c.whole nilPar [], normalizeAt c.cond nilPar [] with
  | some w, some q => !(samePar w q)
  | _, _ => false

/-- **The rule's second half, which `c21Holds` cannot see**: "only a statement continuation inherits
    what precedes it". Every case's `if` sits *behind* a statement, so the normalization of the whole
    must carry **both** — the preceding statement and the desugared `Match`. That is what makes "the
    condition ignores the ambient" a claim about a live ambient rather than a claim about `nilPar`; and
    it is the only check on the *sequencing* half, because a definition whose `.par` handed `nilPar` to
    its right side instead of the left's result still satisfies `c21Holds` — the target is unaffected —
    while the whole silently loses the statement that preceded it. -/
def c21CarriesThePreceding (c : C21Case) : Bool :=
  match normalizeAt c.whole nilPar [] with
  | some w => w.sends.length + w.receives.length + w.matches.length ≥ 2
  | none => false

/-- The cases, each a term whose `if` sits behind a statement — the wild shape — with the `match` control
    and a ground condition for the reasons in the section header. -/
def c21Cases : List C21Case :=
  [ -- 1. the shape AUDIT C21 found in the wild: the `if` behind a send, so the defect's target was
    --    that send *beside* the condition.
    { source := "@\"c\"!([\"a\"]) | if (1 == 1) { @\"out\"!(\"then\") } else { @\"out\"!(\"else\") }"
    , condition := "1 == 1"
    , whole := .par (sQuoteSend "c" [.collect (.list [sStr "\"a\""] none)])
        (.ifElse sAlwaysTrue (sOut "\"then\"") (sOut "\"else\""))
    , cond := sAlwaysTrue }
  , -- 2. no `else` branch: the same desugaring with one case.
    { source := "@\"c\"!(1) | if (1 == 1) { @\"out\"!(\"then\") }"
    , condition := "1 == 1"
    , whole := .par (sQuoteSend "c" [sInt "1"]) (.ifThen sAlwaysTrue (sOut "\"then\""))
    , cond := sAlwaysTrue }
  , -- 3. the control: an explicit `match`, the desugaring that never absorbed the preceding par.
    { source := "@\"c\"!(1) | match 1 { 1 => @\"out\"!(\"then\") }"
    , condition := "1"
    , whole := .par (sQuoteSend "c" [sInt "1"])
        (.match (sInt "1") [⟨sInt "1", sOut "\"then\""⟩])
    , cond := sInt "1" }
  , -- 4. two statements before the `if`: the accumulated par the defect folded in was longer.
    { source := "@\"c\"!(1) | @\"d\"!(2) | if (1 == 1) { @\"out\"!(\"then\") } else { @\"out\"!(\"else\") }"
    , condition := "1 == 1"
    , whole := .par (sQuoteSend "c" [sInt "1"])
        (.par (sQuoteSend "d" [sInt "2"]) (.ifElse sAlwaysTrue (sOut "\"then\"") (sOut "\"else\"")))
    , cond := sAlwaysTrue }
  , -- 5. a ground condition, so the layer does not rest on the arithmetic clauses agreeing.
    { source := "@\"c\"!(1) | if (true) { @\"out\"!(\"then\") } else { @\"out\"!(\"else\") }"
    , condition := "true"
    , whole := .par (sQuoteSend "c" [sInt "1"])
        (.ifElse (.ground (.bool true)) (sOut "\"then\"") (sOut "\"else\""))
    , cond := .ground (.bool true) }
  ]

/-- Every case holds of the model **and** is a probe, checked against the desugaring.

`native_decide` rather than `decide`, for the reason `Rchain/Effect.lean` uses it: the checker reduces
the canonical comparator (`cmpPar`, whose `eq_iff` is proved) over a desugared term, and the kernel's
reduction is too deep for it — the computation itself is fast (`#eval` agrees with the theorem) and the
same trust in the compiler the rest of the tree places elsewhere.

**Measured 2026-09-25**, when 30 of the tree's 36 `native_decide` sites were converted: `decide` here
fails (`tactic 'decide' failed for proposition`) on all four corpus verdicts below, so these four are the
tree's remaining compiler-trust sites and the reason `Rchain/LawsMain.lean`'s `compilerTrust` list is not
empty. The conversion is not a matter of budget — `decide` does not reduce the goal at all. -/
theorem c21Cases_decide :
    c21Cases.all (fun c => c21Holds c && c21IsProbe c) = true := by native_decide


/-- The layer carries exactly `c21CaseCount` cases. -/
theorem c21Cases_length : c21Cases.length = c21CaseCount := by decide

/-- **The sequencing half holds on the same cases**: every whole carries its preceding statement *and*
    its `Match`, so the ambient the rule talks about is visible in the result and not merely present in
    the argument. Falsified by mutation rather than argued: making the `.par` arm hand `nilPar` to its
    right side leaves `c21Cases_decide` green — the target is untouched — and fails this one. -/
theorem c21Cases_carry_the_preceding :
    c21Cases.all c21CarriesThePreceding = true := by native_decide

/-- One c21 corpus line: layer, the term, the condition. The consumer normalizes both and asserts the
    relation the model's half above asserts of its own view. -/
def c21Line (c : C21Case) : String :=
  "c21\t" ++ c.source ++ "\t" ++ c.condition

/-! ## Law 39 — the protocol layer

The catalog is the law (`Rchain/Protocol.lean`: `replyCatalog`, with its `decide`d consistency checks);
this layer is its rendering. Each row is a *probe*: call the urn with the arguments the row spells, and
the reply must arrive in the row's kind and slots. `args` and `slots` render as `-` when empty, so a
column is never blank and the consumer never has to guess what a missing column means. -/

/-- One protocol corpus line: layer, urn, the call's arguments, the call's **arity**, the reply's
kind, its slots.

**The arity column is emitted because its absence was a documented-but-missing field** (AUDIT C158):
`ReplyRow` has carried `callArity` since the catalog was written, this file's sibling prose and the
Rust consumer's own header both say the corpus line carries "the call's arity", and `protocolLine`
did not emit it — so `spec/conformance/protocol.tsv` had five columns and the tie between the
declared arity and the node was held only *indirectly*, by requiring a reply. For an entry that
sends nothing back there is no reply to lose, so a wrong arity there was silence with nothing to
report it.

Carried here, the column is read twice on the Rust side: the consumer checks it against the arity of
the probe it builds (`lean_protocol_corpus.rs`), and
`system_processes.rs`'s `every_catalog_urn_arity_matches_the_definition_the_node_installs` checks it
against the **node's** installed `Definition.arity` for every row — which is the half a non-replying
urn has no reply to expose. -/
def protocolLine (r : ReplyRow) : String :=
  "protocol\t" ++ r.urn ++ "\t" ++ (if r.args.isEmpty then "-" else r.args) ++ "\t"
    ++ toString r.callArity ++ "\t"
    ++ r.kind.tag ++ "\t" ++ (if r.slots.isEmpty then "-" else
      String.intercalate "," (r.slots.map SlotShape.tag))
    ++ "\t" ++ (if r.remainder then "true" else "false")

/-! ## Law 42 — the JSON layer

Each case is a *value* — a rholang expression the API would receive as a datum — the model's view of the
`Par` it becomes, and what the encode must produce (`none` is the envelope's "absent": the API renders
no value at all). The verdict is `decide`d by comparing the model's rendering of the encode against the
rendering of the declared `JE`, so a case that drifted fails `lake build`; the Rust consumer runs the
same source through the node's own codec and compares the JSON, so a case that disagrees with the node
fails the corpus. The cases cover the envelope's three counts (one → unwrapped, none → absent, two or
more → `ExprPar`) and every `JE` arm except the unforgeable leaf, which the model cannot decode back
(`Rchain/Json.lean`'s boundary note). -/

/-- A JSON case: the value as rholang spells it, the model's view of it, and the JSON the API must
expose — `none` for "no value at all" (the envelope's zero case). -/
structure JsonCase where
  /-- The value, as the API receives it: a datum in a term's send. -/
  source : String
  /-- The model's view of the `Par` that value becomes. -/
  par : Par
  /-- What the encode must produce. -/
  je : Option JE

/-- An integer value. -/
def intOf (n : Int) : Par := one (.ground (.int n))

/-- A boolean value. -/
def boolOf (b : Bool) : Par := one (.ground (.bool b))

/-- A `Par` whose only field is a bundle around `p` (the `expr_from_bundle` arm). -/
def bundleOf (p : Par) : Par := Par.mk [] [] [] [] [] [] [Bundle.mk p true false] []

/-- The cases. -/
def jsonCases : List JsonCase :=
  [ -- 1. one field: the envelope unwraps it.
    { source := "42", par := intOf 42, je := some (.int 42) }
  , { source := "true", par := boolOf true, je := some (.bool true) }
  , { source := "\"hello\"", par := strPar "hello", je := some (.str "hello") }
  , { source := "`rho:id:abc`", par := one (.ground (.uri (codePointsOf "rho:id:abc"))),
      je := some (.uri "rho:id:abc") }
  , -- 2. no field at all: the envelope is "absent", which a response renders as no value.
    { source := "Nil", par := Par.mk [] [] [] [] [] [] [] [], je := none }
  , -- 3. two or more: the envelope is an `ExprPar`.
    { source := "1 | 2", par := parMerge (intOf 1) (intOf 2), je := some (.par [.int 1, .int 2]) }
  , -- 4. the collections, and a tuple inside a list (the nesting the JSON exposes as nesting).
    { source := "[1, true]", par := listPat [intOf 1, boolOf true] none,
      je := some (.list [.int 1, .bool true]) }
  , { source := "Set(1, 2)", par := setPat [intOf 1, intOf 2] none,
      je := some (.set [.int 1, .int 2]) }
  , { source := "(1, \"a\")", par := one (.etuple [intOf 1, strPar "a"]),
      je := some (.tuple [.int 1, .str "a"]) }
  , { source := "[[1]]", par := listPat [listPat [intOf 1] none] none,
      je := some (.list [.list [.int 1]]) }
  , { source := "{\"a\": 1}", par := mapOf [("a", intOf 1)] none,
      je := some (.map [("a", .int 1)]) }
  , -- 5. a bundle exposes its body (one field, so the envelope unwraps that too).
    { source := "bundle+{1}", par := bundleOf (intOf 1), je := some (.int 1) }
  ]

/-- Every JSON case's verdict holds of the model: the encode of the case's `Par` renders to the same
text as the case's declared `JE`. `decide`d, so a case that drifted from `parToJE`/`render` fails the
build rather than being trusted. -/
theorem jsonCases_decide :
    jsonCases.all (fun c => (parToJE c.par).map render == c.je.map render) = true := by
  decide

/-- **Law 42 instantiated on the layer's cases, discharged by computation.** The general statement
(`Rchain/Json.lean`'s `decode_encode`) is an induction over the flat fields and is still owed; what this
theorem adds is that *these* cases are not owed: for each one the kernel reduces the encode, the decode
and the encode again, and the wire text is unchanged. `decide` closes it because every function involved
is structural recursion — the same reason the other layers' verdicts are `decide`d — so a case that
stopped round-tripping fails `lake build` rather than waiting for the induction. -/
theorem jsonCases_round_trip :
    jsonCases.all (fun c =>
      (((parToJE c.par).bind jeToPar).bind parToJE).map render
        == (parToJE c.par).map render) = true := by
  decide

/-- The count the Rust consumer asserts it read. -/
def jsonCaseCount : Nat := 12

/-- The layer carries exactly `jsonCaseCount` cases. -/
theorem jsonCases_length : jsonCases.length = jsonCaseCount := by decide

/-- One JSON corpus line: layer, the value, and the JSON the API must expose (`-` when the envelope is
"absent", so no column is ever blank). -/
def jsonLine (c : JsonCase) : String :=
  "json\t" ++ c.source ++ "\t" ++ (match c.je with | none => "-" | some j => render j)

/-! ## Law 59 — the wire-fidelity layer

Each case is a *value* — a rholang expression a peer would send — the model's view of the `Par` it
becomes, and the Syrup shape the encode must produce. The verdict is `decide`d by comparing the
model's rendering of the encode against the rendering of the declared shape, so a case that drifted
fails `lake build`; the Rust consumer runs the same source through the bridge and compares its own
rendering, so a case that disagrees with the node fails the corpus.

The cases are the law's own clauses: the tuple (C226 — sent as a list it comes back a list, which is
what made a peer able to hold a purse and not fund it), the URI (whose asymmetry was a lossy map), and
the containers around them. `renderSy` writes the **record** a tuple crosses as, so the tuple's row
shows the wire shape rather than the model's name for it. -/

/-- A wire-fidelity case: the value as rholang spells it, the model's view of it, and the Syrup shape
the encode must produce. -/
structure SyrupCase where
  /-- The value, as a peer sends it. -/
  source : String
  /-- The model's view of the `Par` that value becomes. -/
  par : Par
  /-- What the encode must produce. -/
  sy : Sy

/-- The two selector texts a tuple's record carries, as constants so `renderSy` descends only into
subterms. -/
def tupleLabelText : String := "'" ++ syString taggedLabel

def tupleTagText : String := "'" ++ syString tupleTag

mutual
  /-- The wire text of a Syrup value, in the compact form both sides write. A tuple is rendered as
  the tagged record it is, so the row shows what actually crosses. -/
  def renderSy : Sy → String
    | .bool b => if b then "t" else "f"
    | .int n => toString n
    | .str l => "\"" ++ syString l ++ "\""
    | .sym l => "'" ++ syString l
    | .bytes l => "b" ++ toString l.length
    | .float => "D"
    | .list ss => "[" ++ String.intercalate ", " (renderSys ss) ++ "]"
    | .struct kvs => "{" ++ String.intercalate ", " (renderKvs kvs) ++ "}"
    | .record xs => "<" ++ String.intercalate " " (renderSys xs) ++ ">"
    | .tuple ss =>
        "<" ++ tupleLabelText ++ " " ++ tupleTagText ++ " " ++
          "[" ++ String.intercalate ", " (renderSys ss) ++ "]>"

  def renderSys : List Sy → List String
    | [] => []
    | s :: rest => renderSy s :: renderSys rest

  def renderKvs : List (String × Sy) → List String
    | [] => []
    | (k, v) :: rest => (k ++ ": " ++ renderSy v) :: renderKvs rest
end

/-- The cases. -/
def syrupCases : List SyrupCase :=
  [ { source := "true", par := boolOf true, sy := .bool true }
  , { source := "42", par := intOf 42, sy := .int 42 }
  , { source := "-7", par := intOf (-7), sy := .int (-7) }
  , { source := "\"hi\"", par := strPar "hi", sy := .str (syChars "hi") }
  , { source := "`rho:id:abc`", par := one (.ground (.uri (syChars "rho:id:abc"))),
      sy := .sym (syChars "rho:id:abc") }
  , { source := "[]", par := listPat [] none, sy := .list [] }
  , { source := "[1, 2]", par := listPat [intOf 1, intOf 2] none, sy := .list [.int 1, .int 2] }
  , -- **C226's case**: a tuple must cross as a tuple, so a peer can send an amount back.
    { source := "(true, 0)", par := one (.etuple [boolOf true, intOf 0]),
      sy := .tuple [.bool true, .int 0] }
  , { source := "(1, \"a\")", par := one (.etuple [intOf 1, strPar "a"]),
      sy := .tuple [.int 1, .str (syChars "a")] }
  , { source := "[(1, 2)]", par := listPat [one (.etuple [intOf 1, intOf 2])] none,
      sy := .list [.tuple [.int 1, .int 2]] }
  , -- A tuple inside a tuple: the nesting an amount carries when its value is itself structured.
    { source := "((1, 2), 3)", par := one (.etuple [one (.etuple [intOf 1, intOf 2]), intOf 3]),
      sy := .tuple [.tuple [.int 1, .int 2], .int 3] }
  , { source := "{\"a\": 1}", par := mapOf [("a", intOf 1)] none, sy := .struct [("a", .int 1)] }
  ]

/-- Every case's verdict holds of the model: the encode of the case's `Par` renders to the same text as
the case's declared shape. `decide`d, so a case that drifted fails the build rather than being
trusted. -/
theorem syrupCases_decide :
    syrupCases.all (fun c => (parToSy c.par).map renderSy == some (renderSy c.sy)) = true := by
  decide

/-- **Law 59 instantiated on the layer's cases, discharged by computation.** The general statement is
`Rchain/Syrup.lean`'s `syrup_decode_encode`; what this adds is that *these* cases — the tuple above
all — are not owed: for each one the kernel reduces the encode, the decode and the encode again, and
the wire text is unchanged. -/
theorem syrupCases_round_trip :
    syrupCases.all (fun c =>
      (((parToSy c.par).bind syToPar).bind parToSy).map renderSy
        == (parToSy c.par).map renderSy) = true := by
  decide

/-- The count the Rust consumer asserts it read. -/
def syrupCaseCount : Nat := 12

/-- The layer carries exactly `syrupCaseCount` cases. -/
theorem syrupCases_length : syrupCases.length = syrupCaseCount := by decide

/-- One wire-fidelity corpus line: layer, the value, and the Syrup shape it must produce. -/
def syrupLine (c : SyrupCase) : String :=
  "syrup\t" ++ c.source ++ "\t" ++ renderSy c.sy

/-! ## Law 43 — the envelope layer

The catalog is the law (`Rchain/Envelope.lean`: `envelopeCatalog`, with its `decide`d checks); this
layer renders it. Each row is a response type — its name, the endpoint a reader should think of, and
either its struct keys or its tagged union's `tag → keys`. The Rust consumer holds *both* parties to
it: the DTO's own serialization and the served OpenAPI schema. -/

/-- One envelope corpus line: layer, the type, the endpoint, the keys, and the union's tags with their
own keys (`-` where a row has none, so a column is never blank). -/
def envelopeLine (r : EnvelopeRow) : String :=
  "envelope\t" ++ r.name ++ "\t" ++ r.endpoint ++ "\t"
    ++ (if r.keys.isEmpty then "-" else String.intercalate "," r.keys) ++ "\t"
    ++ (if r.variants.isEmpty then "-" else
      String.intercalate ";" (r.variants.map (fun v =>
        v.1 ++ ":" ++ String.intercalate "," v.2)))

/-! ## Law 32 — the lexeme layer

The table is the law (`Rchain/Lex.lean`: `lexemes`, with `lexemes_decide` checking distinctness, the
punctuation class and maximal munch); this layer renders it. Each row is a spelling, the token it lexes
to, what its sample must observe, and the sample itself — so the Rust consumer runs the sample and
reports; nothing about the meaning is the consumer's to decide. -/

/-- One lexeme corpus line: layer, the spelling, its token, the expectation, and the sample term. -/
def lexLine (l : Lexeme) : String :=
  "lex\t" ++ l.spelling ++ "\t" ++ l.token ++ "\t" ++ l.expected ++ "\t" ++ l.sample

/-! ## Laws 30 and 31 — the parse layer

The tables are the law (`Rchain/Parse.lean`: `grammarFragment`, `derives`, `parseDeviations`, and
`parseCases` with `parseCases_decide` checking every verdict); this layer renders them. Each row is a
source and what the node must do with it — `accept` or `reject` — so the Rust consumer
(`rholang/tests/lean_parse_corpus.rs`) runs the node's parser on that source and reports; nothing
about the verdict is the consumer's to decide.

The source is **rendered from the case's tokens** (`renderTokens`), not written beside them, so a row
cannot carry a spelling its verdict was not decided from — the drift is impossible by construction
rather than checked. -/

/-- One parse corpus line: layer, the source the tokens spell, the verdict the node must give, and
which half of the layer the row belongs to (`derivable` / `refused` / `deviation` / `printer`) — the
consumer pins a count for each half, so a list that silently shrank is caught where it shrank. -/
def parseLine (c : ParseCase) : String :=
  "parse\t" ++ renderTokens c.tokens ++ "\t" ++ (if c.expect then "accept" else "reject")
    ++ "\t" ++ c.kind.tag

/-! ## Law 1 — the canonical order, pairwise (the `sort` layer)

Every state hash in the system is `sortPar`'s output, and until this layer existed **nothing tied that
order to the node**: law 1a was `proved-model` over a model a human keeps in sync, and law 1b's
comparator laws are `owed` (the twelve axioms in `Rchain/Sort.lean`).

**The first design for this layer was vacuous, and finding that out is part of its content.** A corpus of
"the sorted spelling of this par" cannot tie a comparator at all: the AST's fields are *canonical* values
(`Sorted<Par>`), so both the node's sorted output and the expected spelling get re-sorted by the *same*
comparator before anything is compared — any total order satisfies `sortPar src = parse sorted`. That is
what canonicalization *means*: it is the quotient by the comparator, and a quotient cannot distinguish
two comparators that induce the same equality. The tie has to be **pairwise**, on structures small
enough that canonicalization is the identity — which is what this layer is: each case is a *pair* of
terms and the verdict the model's `cmpPar` gives them, so the node's `Ord` on the same pair is compared
against the model's answer, with nothing in between to hide a disagreement.

The cases pin the arms a port can get wrong *silently* — the leaf arms, the **ground constructor order**
(`bool < int < str`), the **collection constructor order** (`elist < etuple < eset`), list arity, an
arithmetic node's operand order, and the send channel arm — and the layer's own non-degeneracy check
requires **all three verdicts** to appear, so a table that answered `lt` everywhere could not pass.

Cases 13–19 are the drift this layer was built to find, now closed. The model ordered by
*declaration* order; the node orders by its **score tree** (`models/src/sorter.rs`'s `node_score(tag,
children)`), and the first run of this corpus showed the two disagree on four separate structures — a
send's field order, the par's own field order, the expression-class order and `Ground.bool`'s
polarity. `Rchain/Sort.lean`'s comparators were reordered to the tags and these rows are the
falsifiers: restore the declaration order and the rows stop `decide`ing. The verdicts are the node's
own answers, read off `sort_pars` by the Rust consumer's diagnostic *before* the model was changed, so
the model was aligned to the node rather than to a guess. The algebra gaps this layer closed are
recorded in `Sort.lean`'s note: rows 20–22 closed three of the eight missing constructors, rows 23–25
pin three constructors' *placement*, and rows 43–50 pin the decimal class (`DECIMAL` 14), which makes
the model 30 classes covering all 34 of the node's variants. -/

/-- The number of cases the `sort` layer carries. -/
def sortCaseCount : Nat := 50

/-- A law-1 case: two terms (as rholang spells them, so the Rust consumer reads the same text) and the
    verdict the model's `cmpPar` must give the pair. -/
structure SortCase where
  /-- The left term, as rholang spells it — a program, so the node's parser accepts it. -/
  left : String
  /-- The right term. -/
  right : String
  /-- The model's view of the left term. -/
  leftPar : Par
  /-- The model's view of the right term. -/
  rightPar : Par
  /-- What `cmpPar left right` must answer: `lt`, `eq` or `gt`. -/
  verdict : String

/-- `@"<channel>"!(<datum>)` as a `Par`: one send on a quoted channel. -/
def sendPar (channel : String) (datum : Par) : Par :=
  Par.mk [Send.mk (strPar ("\"" ++ channel ++ "\"")) [datum] false] [] [] [] [] [] [] []

/-- A list expression, as a `Par`. -/
def listExpr (ps : List Par) : Par := one (.elist ps none)

/-- A set expression, as a `Par`. -/
def setExpr (ps : List Par) : Par := one (.eset ps none)

/-- A tuple expression, as a `Par`. -/
def tupleExpr (ps : List Par) : Par := one (.etuple ps)

/-- An integer, as a `Par`. -/
def intExpr (n : Int) : Par := one (.ground (.int n))

/-- A boolean, as a `Par`. -/
def boolExpr (b : Bool) : Par := one (.ground (.bool b))

/-- A bigint literal, as a `Par`. `BIG_INT`(13) is an `Expr` leaf rather than a `Ground` one — the
    protobuf's `g_big_int` is an `Expr` variant and the port's normalizer converts the parse-level
    ground into it (`normalizer.rs:74-77`). -/
def bigintExpr (n : Int) : Par := one (.ebigint n)

/-- A decimal literal, as a `Par`: `decimalExpr 15 1` is `1.5d`. The payload is the value in
    units of `10^-38` (`MAX_SCALE`), exactly the node's `Decimal::scaled_value`. -/
def decimalExpr (c : Int) (s : Nat) : Par := one (.edecimal (Decimal.scaledValue ⟨c, s⟩))

/-- A method call, as a `Par`. The name is a **code-point list**, the model's convention for strings
    (`charsOf`, as `Ground.str` does) — the node carries a `String` and compares it byte-wise, and
    UTF-8 preserves code-point order. -/
def methodExpr (name : String) (target : Par) (args : List Par) : Par :=
  one (.emethod (charsOf name) target args)

/-- `a + b`, as a `Par`. -/
def plusExpr (a b : Par) : Par := one (.eplus a b)

/-- `a * b`, as a `Par`. -/
def multExpr (a b : Par) : Par := one (.emult a b)

/-- `a - b`, as a `Par`. -/
def minusExpr (a b : Par) : Par := one (.eminus a b)

/-- `new x in { body }` with `n` binders, as a `Par`. -/
def newPar (n : Nat) (body : Par) : Par :=
  Par.mk [] [] [New.mk n body] [] [] [] [] []

/-- The model's verdict for a pair, spelled as the corpus spells it. -/
def sortVerdict (p q : Par) : String :=
  match cmpPar p q with
  | .lt => "lt"
  | .eq => "eq"
  | .gt => "gt"

/-- One case holds when the model's verdict for the pair is the verdict the row states. -/
def sortHolds (c : SortCase) : Bool := sortVerdict c.leftPar c.rightPar == c.verdict

/-- The cases: the leaf arms, the two constructor orders, list arity, an arithmetic node, a send's
    channel, and the three verdicts between them. -/
def sortCases : List SortCase :=
  [ -- 1-3. integers: all three verdicts, so a table that answered one of them everywhere fails.
    { left := "@\"c\"!(1)", right := "@\"c\"!(2)", leftPar := sendPar "c" (intExpr 1),
      rightPar := sendPar "c" (intExpr 2), verdict := "lt" },
    { left := "@\"c\"!(2)", right := "@\"c\"!(1)", leftPar := sendPar "c" (intExpr 2),
      rightPar := sendPar "c" (intExpr 1), verdict := "gt" },
    { left := "@\"c\"!(2)", right := "@\"c\"!(2)", leftPar := sendPar "c" (intExpr 2),
      rightPar := sendPar "c" (intExpr 2), verdict := "eq" },
    -- 4-6. **the ground constructor order** (`bool < int < str`) — nothing else pins it, and a port
    --      that ordered grounds by their serialized bytes would put the string first.
    { left := "@\"c\"!(1)", right := "@\"c\"!(\"a\")", leftPar := sendPar "c" (intExpr 1),
      rightPar := sendPar "c" (strPar "\"a\""), verdict := "lt" },
    { left := "@\"c\"!(\"a\")", right := "@\"c\"!(true)", leftPar := sendPar "c" (strPar "\"a\""),
      rightPar := sendPar "c" (boolExpr true), verdict := "gt" },
    { left := "@\"c\"!(true)", right := "@\"c\"!(1)", leftPar := sendPar "c" (boolExpr true),
      rightPar := sendPar "c" (intExpr 1), verdict := "lt" },
    -- 7-9. **the collection constructor order** (`elist < etuple < eset`) and list arity.
    { left := "@\"c\"!([1])", right := "@\"c\"!(Set(1))", leftPar := sendPar "c" (listExpr [intExpr 1]),
      rightPar := sendPar "c" (setExpr [intExpr 1]), verdict := "lt" },
    --    (`(1, 2)`, not `(1)`: a parenthesised *single* expression is a group, not a one-element
    --    tuple — the corpus's own first run caught that spelling, because the node parsed `(1)` as the
    --    integer `1` and answered `gt` where the row said `lt`.)
    { left := "@\"c\"!([1])", right := "@\"c\"!((1, 2))", leftPar := sendPar "c" (listExpr [intExpr 1]),
      rightPar := sendPar "c" (tupleExpr [intExpr 1, intExpr 2]), verdict := "lt" },
    { left := "@\"c\"!([1])", right := "@\"c\"!([1, 2])",
      leftPar := sendPar "c" (listExpr [intExpr 1]),
      rightPar := sendPar "c" (listExpr [intExpr 1, intExpr 2]), verdict := "lt" },
    -- 10-11. an arithmetic node: the operand order decides, and a ground sorts before it.
    { left := "@\"c\"!(1 + 2)", right := "@\"c\"!(1 + 3)",
      leftPar := sendPar "c" (plusExpr (intExpr 1) (intExpr 2)),
      rightPar := sendPar "c" (plusExpr (intExpr 1) (intExpr 3)), verdict := "lt" },
    { left := "@\"c\"!(1)", right := "@\"c\"!(1 + 2)", leftPar := sendPar "c" (intExpr 1),
      rightPar := sendPar "c" (plusExpr (intExpr 1) (intExpr 2)), verdict := "lt" },
    -- 12. the send channel arm.
    { left := "@\"a\"!(1)", right := "@\"b\"!(1)", leftPar := sendPar "a" (intExpr 1),
      rightPar := sendPar "b" (intExpr 1), verdict := "lt" },
    -- 13-19. **the drift the corpus was built to find**, now aligned and pinned. Every verdict below
    --        was *observed* from the node (the consumer's diagnostic read it off `sort_pars` before
    --        the model was corrected); the model's comparators now give the same answers, because
    --        `Rchain/Sort.lean`'s `cmpPar`/`cmpSend`/`cmpExpr`/`cmpGround` were reordered to the
    --        node's score tags. Each row is the falsifier for one of those reorderings: put the old
    --        declaration order back and the corresponding row fails to `decide`.
    -- 13-15. the **expression-class order**: the tags put every collection (6-9) before the vars and
    --        operators (100+), and the operators among themselves run `EMULT 102 < EDIV 103 <
    --        EPLUS 104 < EMINUS 105`.
    { left := "@\"c\"!([1])", right := "@\"c\"!(1 + 2)", leftPar := sendPar "c" (listExpr [intExpr 1]),
      rightPar := sendPar "c" (plusExpr (intExpr 1) (intExpr 2)), verdict := "lt" },
    { left := "@\"c\"!(1 * 2)", right := "@\"c\"!(1 + 2)",
      leftPar := sendPar "c" (multExpr (intExpr 1) (intExpr 2)),
      rightPar := sendPar "c" (plusExpr (intExpr 1) (intExpr 2)), verdict := "lt" },
    { left := "@\"c\"!(1 - 2)", right := "@\"c\"!(1 * 2)",
      leftPar := sendPar "c" (minusExpr (intExpr 1) (intExpr 2)),
      rightPar := sendPar "c" (multExpr (intExpr 1) (intExpr 2)), verdict := "gt" },
    -- 16. **`Ground.bool`'s polarity**: the node scores `true` as 0 and `false` as 1, so `false` sorts
    --     *after* `true` — the reverse of `linearOrderComparator Bool`, and the reason `cmpBool` exists
    --     rather than a `linearOrderComparator Bool` in `cmpGround`.
    { left := "@\"c\"!(false)", right := "@\"c\"!(true)", leftPar := sendPar "c" (boolExpr false),
      rightPar := sendPar "c" (boolExpr true), verdict := "gt" },
    -- 17. **a send's field order**: the score compares `persistent` first, then the channel, then the
    --     data — so a *persistent* send on `a` sorts after a plain one on `b`, where comparing channels
    --     first (the model's old `cmpSend`) says the opposite. This is the pair the divergence note is
    --     about, and the corpus's first run is what found it.
    { left := "@\"a\"!!(1)", right := "@\"b\"!(1)",
      leftPar := sendParP (strPar "\"a\"") [intExpr 1] true, rightPar := sendPar "b" (intExpr 1),
      verdict := "gt" },
    -- 18-19. **the par's own field order**: the score gathers `exprs` before `news`, so a par whose only
    --        field is an expression sorts before one whose only field is a `new` — the reverse of the
    --        *port's* field order, and a pair no single-par observation can see (canonicalization would
    --        hide it inside the par). Both directions are rows, so a comparator with the two orders
    --        swapped fails one of them.
    { left := "new x in { Nil } | 1", right := "[1]",
      leftPar := parMerge (newPar 1 nilPar) (intExpr 1), rightPar := listExpr [intExpr 1],
      verdict := "lt" },
    { left := "[1]", right := "new x in { Nil } | 1",
      leftPar := listExpr [intExpr 1], rightPar := parMerge (newPar 1 nilPar) (intExpr 1),
      verdict := "gt" },
    -- 20-22. **The three conflations, now distinguished** (AUDIT C58). Each spelling normalizes to its
    -- own constructor in the node's algebra — `EAND` 113 vs `ESHORTAND` 123, `EOR` 114 vs `ESHORTOR`
    -- 124, `EEQ` 110 vs `EMATCHES` 118 — and to the *same* constructor here before this unit, so the
    -- model answered `eq` where the node answers `gt` and no row could be `decide`d (measured: the
    -- `native_decide` refusal at the falsifier stage). They are unit 1's falsifiers, and the model's
    -- re-tagging is what makes them pass: the node's verdict is `gt` for all three, and `cmpPar` now
    -- says `gt` too.
    { left := "1 && 2", right := "1 and 2",
      leftPar := one (.eshortand (one (.ground (.int 1))) (one (.ground (.int 2)))),
      rightPar := one (.eand (one (.ground (.int 1))) (one (.ground (.int 2)))),
      verdict := "gt" },
    { left := "1 || 2", right := "1 or 2",
      leftPar := one (.eshortor (one (.ground (.int 1))) (one (.ground (.int 2)))),
      rightPar := one (.eor (one (.ground (.int 1))) (one (.ground (.int 2)))),
      verdict := "gt" },
    { left := "1 matches 2", right := "1 == 2",
      leftPar := one (.ematches (one (.ground (.int 1))) (one (.ground (.int 2)))),
      rightPar := one (.eeq (one (.ground (.int 1))) (one (.ground (.int 2)))),
      verdict := "gt" },
    -- 23. **The one pair that left the boundary list.** `&&` and `||` are *operator tokens* in the
    -- node (`rholang/src/parser.rs:216-222` — `Tok::OrOr`), not method calls, which is what the
    -- boundary note in `rholang/tests/lean_sort_corpus.rs` claimed until 2026-09-24 and was wrong
    -- about. Both constructors exist here now, so the pair is statable: the node's verdict is `lt`
    -- (`ESHORTAND` 123 < `ESHORTOR` 124), observed from it before the row was written, and `cmpPar`
    -- says `lt` too. **`%%`/`%`, the `BigInt` pairs and `++`/`--` left the boundary list on
    -- 2026-09-25** — rows 26-42, once the five constructs had constructors, and `EMETHOD` left
    -- it with them.
    { left := "1 && 2", right := "1 || 2",
      leftPar := one (.eshortand (one (.ground (.int 1))) (one (.ground (.int 2)))),
      rightPar := one (.eshortor (one (.ground (.int 1))) (one (.ground (.int 2)))),
      verdict := "lt" },
    -- 24-25. **The interleaving with `emod`, which no other row can see.** Row 23 pins `&&` against
    -- `||` *within* the late block; these two pin the block's position against `EMOD` 122, which is
    -- the placement the arm order could have got wrong in a way nothing else would catch — every
    -- other spelling that separates an interleaved order from a *trailing* one involves a
    -- constructor the model lacks (`%%`, `BigInt`, `++`, `--`). Both verdicts were observed from the
    -- node before the rows were written (`rholang/tests/lean_sort_corpus.rs`'s boundary test prints
    -- them): `EMATCHES` 118 < `EMOD` 122 ⇒ `lt`, and `EMOD` 122 < `ESHORTAND` 123 ⇒ `gt`.
    -- The third pair observed with them, `1 matches 2` against `1 && 2`, is deliberately *not* a row:
    -- its verdict follows transitively from these two, and a row that cannot fail independently of
    -- another row is not a second falsifier. It stays a boundary observation.
    { left := "1 matches 2", right := "1 % 2",
      leftPar := one (.ematches (one (.ground (.int 1))) (one (.ground (.int 2)))),
      rightPar := one (.emod (one (.ground (.int 1))) (one (.ground (.int 2)))),
      verdict := "lt" },
    { left := "1 && 2", right := "1 % 2",
      leftPar := one (.eshortand (one (.ground (.int 1))) (one (.ground (.int 2)))),
      rightPar := one (.emod (one (.ground (.int 1))) (one (.ground (.int 2)))),
      verdict := "gt" },
    -- 26-42. **The five constructs that were unstatable until 2026-09-25**, each verdict read off the
    -- node before the row was written (the boundary test's print, `rholang/tests/lean_sort_corpus.rs`).
    -- `BIG_INT` 13 sorts *after* the collections and *before* the operators, `EMETHOD` 115 between
    -- `EOR` 114 and `EMATCHES` 118, and the three operators 119-121 between `EMATCHES` 118 and
    -- `EMOD` 122 — the interleaving no declaration-order comparator can express, which is why each is
    -- its own class. Rows 26-29 are `BIG_INT` on both of its sides; 30-35 the block the three
    -- operators sit in; 36-42 `EMETHOD`, whose score children are the node's own
    -- (`[tag, name, target, args…]`, `sorter.rs:677-698`).
    { left := "BigInt(1)", right := "BigInt(2)",
      leftPar := bigintExpr 1, rightPar := bigintExpr 2, verdict := "lt" },
    { left := "BigInt(42)", right := "42",
      leftPar := bigintExpr 42, rightPar := intExpr 42, verdict := "gt" },
    { left := "BigInt(0)", right := "[1]",
      leftPar := bigintExpr 0, rightPar := listExpr [intExpr 1], verdict := "gt" },
    { left := "BigInt(42)", right := "1 + 2",
      leftPar := bigintExpr 42, rightPar := plusExpr (intExpr 1) (intExpr 2), verdict := "lt" },
    { left := "Set(1) -- Set(1)", right := "Set(1)",
      leftPar := one (.eminusMinus (setExpr [intExpr 1]) (setExpr [intExpr 1])),
      rightPar := setExpr [intExpr 1], verdict := "gt" },
    { left := "[1] ++ [2]", right := "[1] + [2]",
      leftPar := one (.eplusPlus (listExpr [intExpr 1]) (listExpr [intExpr 2])),
      rightPar := plusExpr (listExpr [intExpr 1]) (listExpr [intExpr 2]), verdict := "gt" },
    { left := "1 %% 2", right := "1 % 2",
      leftPar := one (.epercentPercent (intExpr 1) (intExpr 2)),
      rightPar := one (.emod (intExpr 1) (intExpr 2)), verdict := "lt" },
    { left := "1 %% 2", right := "1 matches 2",
      leftPar := one (.epercentPercent (intExpr 1) (intExpr 2)),
      rightPar := one (.ematches (intExpr 1) (intExpr 2)), verdict := "gt" },
    { left := "1 && 2", right := "1 ++ 2",
      leftPar := one (.eshortand (intExpr 1) (intExpr 2)),
      rightPar := one (.eplusPlus (intExpr 1) (intExpr 2)), verdict := "gt" },
    { left := "1 ++ 2", right := "1 -- 2",
      leftPar := one (.eplusPlus (intExpr 1) (intExpr 2)),
      rightPar := one (.eminusMinus (intExpr 1) (intExpr 2)), verdict := "lt" },
    { left := "1 -- 2", right := "1 % 2",
      leftPar := one (.eminusMinus (intExpr 1) (intExpr 2)),
      rightPar := one (.emod (intExpr 1) (intExpr 2)), verdict := "lt" },
    { left := "1.foo(2)", right := "1 + 2",
      leftPar := methodExpr "foo" (intExpr 1) [intExpr 2],
      rightPar := plusExpr (intExpr 1) (intExpr 2), verdict := "gt" },
    { left := "1.foo(2)", right := "1 matches 2",
      leftPar := methodExpr "foo" (intExpr 1) [intExpr 2],
      rightPar := one (.ematches (intExpr 1) (intExpr 2)), verdict := "lt" },
    { left := "1.foo(2)", right := "1 %% 2",
      leftPar := methodExpr "foo" (intExpr 1) [intExpr 2],
      rightPar := one (.epercentPercent (intExpr 1) (intExpr 2)), verdict := "lt" },
    { left := "1.foo(2)", right := "2.foo(2)",
      leftPar := methodExpr "foo" (intExpr 1) [intExpr 2],
      rightPar := methodExpr "foo" (intExpr 2) [intExpr 2], verdict := "lt" },
    { left := "1.foo(2)", right := "1.bar(2)",
      leftPar := methodExpr "foo" (intExpr 1) [intExpr 2],
      rightPar := methodExpr "bar" (intExpr 1) [intExpr 2], verdict := "gt" },
    { left := "1.foo(2)", right := "1.foo(3)",
      leftPar := methodExpr "foo" (intExpr 1) [intExpr 2],
      rightPar := methodExpr "foo" (intExpr 1) [intExpr 3], verdict := "lt" },
    -- 43-50. The decimal class (`DECIMAL` 14): numeric order across scales, canonical equality,
    -- and its class position against Int (2), BigInt (13), the collections (6) and the arithmetic
    -- operators (104). Each verdict was read off the node before the row was written.
    { left := "@\"c\"!(1.5d)", right := "@\"c\"!(2d)",
      leftPar := sendPar "c" (decimalExpr 15 1), rightPar := sendPar "c" (decimalExpr 2 0),
      verdict := "lt" },
    { left := "@\"c\"!(2d)", right := "@\"c\"!(1.5d)",
      leftPar := sendPar "c" (decimalExpr 2 0), rightPar := sendPar "c" (decimalExpr 15 1),
      verdict := "gt" },
    { left := "@\"c\"!(1.10d)", right := "@\"c\"!(1.1d)",
      leftPar := sendPar "c" (decimalExpr 11 1), rightPar := sendPar "c" (decimalExpr 11 1),
      verdict := "eq" },
    { left := "@\"c\"!(1d)", right := "@\"c\"!(1.5d)",
      leftPar := sendPar "c" (decimalExpr 1 0), rightPar := sendPar "c" (decimalExpr 15 1),
      verdict := "lt" },
    { left := "@\"c\"!(1.5d)", right := "@\"c\"!(100)",
      leftPar := sendPar "c" (decimalExpr 15 1), rightPar := sendPar "c" (intExpr 100),
      verdict := "gt" },
    { left := "@\"c\"!(1.5d)", right := "@\"c\"!(BigInt(100))",
      leftPar := sendPar "c" (decimalExpr 15 1), rightPar := sendPar "c" (bigintExpr 100),
      verdict := "gt" },
    { left := "@\"c\"!(1.5d)", right := "@\"c\"!([1])",
      leftPar := sendPar "c" (decimalExpr 15 1),
      rightPar := sendPar "c" (listExpr [intExpr 1]), verdict := "gt" },
    { left := "@\"c\"!(1.5d)", right := "@\"c\"!(1 + 2)",
      leftPar := sendPar "c" (decimalExpr 15 1),
      rightPar := sendPar "c" (plusExpr (intExpr 1) (intExpr 2)), verdict := "lt" } ]

/-- Every case holds of the model — `cmpPar` gives the verdict the row states. `native_decide`, for the
    reason the `c21` checker uses it: the comparator's reduction over a term is too deep for the kernel
    and fast for the compiler — `decide` fails here too, measured 2026-09-25 with the other three. -/
theorem sortCases_decide : sortCases.all sortHolds = true := by native_decide

/-- **The layer is not degenerate**: all three verdicts appear, so a table that answered `lt` (or any
    single ordering) everywhere could not pass — the non-vacuity ratchet applied to this corpus. -/
theorem sortCases_verdicts :
    (sortCases.map (fun c => c.verdict)).eraseDups.length = 3 := by decide

/-- The layer carries exactly `sortCaseCount` cases. -/
theorem sortCases_length : sortCases.length = sortCaseCount := by decide

/-- One `sort` corpus line: layer, the two terms, and the verdict the node's comparator must give them. -/
def sortLine (c : SortCase) : String :=
  "sort\t" ++ c.left ++ "\t" ++ c.right ++ "\t" ++ c.verdict

/-! ## The `closed` layer — law 6's verdict, read off the node

Law 6 says a process is closed exactly when it has no free variable, and the node decides that with
`models::types::is_closed` (`models/src/types.rs:371`) — the same predicate `Closed::new` refuses on
rather than panicking. The register's row for law 6 was `proved-model` with the node side *anchored* but
not *replayed*; this layer is the replay: each row is a term as rholang spells it, the model's `Par` for
the same term, and the value `closed` must return. The consumer parses and normalizes the source with the
node's own parser and asserts the node's `is_closed` agrees — the two sides compare terms they built
themselves, which is what makes the source text the tie.

The cases are chosen to separate the predicate's parts rather than to be many: a ground (closed), a free
variable (open, and the reason the predicate exists), a bound variable (closed), a `new` whose body is
closed (closed — the binder is what makes it so), a send on a free channel (open) against one on a ground
channel (closed), a collection with a free element (open) against one without (closed), and an arithmetic
expression (closed). A receive is deliberately **not** here: the model's pattern variables resolve through
the binder stack (`nameVar`), and whether that matches the node's `is_closed` on a receive's pattern is a
question this layer would have to answer before it could pin it — the honest move is a case list that
pins what is agreed, with the question named here. -/

/-- One `closed` case: the term as rholang spells it, the model's `Par` for it, and the verdict. -/
structure ClosedCase where
  /-- The term as rholang spells it — what the consumer's parser reads. -/
  source : String
  /-- The same term, as the model sees it. -/
  par : Par
  /-- What `closed` must return. -/
  expected : Bool

/-- A variable occurrence, free. -/
def freeVarPar (k : Nat) : Par := one (.evar (.free k))

/-- A variable occurrence, bound. -/
def boundVarPar (k : Nat) : Par := one (.evar (.bound k))

/-- The cases. -/
def closedCases : List ClosedCase :=
  [ ⟨"1", intPar 1, true⟩
  , ⟨"\"a\"", one (.ground (.str [97])), true⟩
  , ⟨"x", freeVarPar 0, false⟩
  , ⟨"0", boundVarPar 0, true⟩
  , ⟨"new x in { Nil }", Par.mk [] [] [New.mk 1 nilPar] [] [] [] [] [], true⟩
  , ⟨"x!(1)", Par.mk [Send.mk (freeVarPar 0) [intPar 1] false] [] [] [] [] [] [] [], false⟩
  , ⟨"@1!(1)", Par.mk [Send.mk (intPar 1) [intPar 1] false] [] [] [] [] [] [] [], true⟩
  , ⟨"[1, x]", one (.elist [intPar 1, freeVarPar 0] none), false⟩
  , ⟨"[1, 2]", one (.elist [intPar 1, intPar 2] none), true⟩
  , ⟨"1 + 2", one (.eplus (intPar 1) (intPar 2)), true⟩
  ]

/-- The layer's checker: every case's verdict is what `closed` returns. -/
def closedHolds (c : ClosedCase) : Bool := closed c.par == c.expected

/-- Every case holds of the model. `decide` rather than `native_decide`: the terms are shallow enough
    for the kernel, and law 6's predicate is a structural checker rather than a comparator reduction. -/
theorem closedCases_decide : closedCases.all closedHolds = true := by decide

/-- The layer carries exactly `closedCaseCount` cases. -/
def closedCaseCount : Nat := 10

theorem closedCases_length : closedCases.length = closedCaseCount := by decide

/-- **The layer is not degenerate**: both verdicts appear, so a checker that answered `false` (or `true`)
    everywhere could not pass. Law 6's predicate is the interesting kind — the non-vacuity ratchet
    applied to this layer, as `sortCases_verdicts` is to the sorter's. -/
theorem closedCases_verdicts :
    (closedCases.map (fun c => c.expected)).eraseDups.length = 2 := by decide

/-- One `closed` corpus line: layer, the term, the verdict. -/
def closedLine (c : ClosedCase) : String :=
  "closed\t" ++ c.source ++ "\t" ++ (if c.expected then "true" else "false")

/-! ## The `stake` layer — law 14a's gate, read off the node

Law 14a's row is `proved-model` with the node side anchored (`sdk/src/consensus.rs`). This layer is the
replay: each row is a `(stake, total)` pair and the verdict `isSuperMajority` must return, `decide`d in
the tree, while the consumer calls the node's own `is_super_majority` on the same pair.

**The comparison is exact integer arithmetic, and the cases say so.** `3·stake > 2·total` — no division,
no rounding, no floating point anywhere: the boundary is *exactly* `3·stake = 2·total`, which is not a
supermajority, and one unit over it, which is. So the cases are the boundary family rather than samples:
`2·k` against `3·k` at two scales, one over at each, the degenerate pairs, and two pairs wide enough that
a narrower accumulator would have decided them wrongly — `3·stake` past the `i64` range in one case and
`stake` itself past it in the other, which is why the port's signature takes `i128` and the model's takes
`Nat`. A layer of small pairs would pass under an arithmetic that is neither exact nor wide; these do not.
-/

/-- One `stake` case: the two sums and the verdict. -/
structure StakeCase where
  /-- The supporting stake. -/
  stake : Nat
  /-- The total bonded stake. -/
  total : Nat
  /-- What `isSuperMajority` must say. -/
  expected : Bool

/-- `total * 2 / 3`, as the port's own property test computes the two-thirds mark. -/
def twoThirds (total : Nat) : Nat := total * 2 / 3

/-- The cases: the boundary, then its width. -/
def stakeCases : List StakeCase :=
  [ ⟨2, 3, false⟩                                   -- exactly two thirds …
  , ⟨2 * 2, 3 * 2, false⟩                           -- … at a second scale …
  , ⟨2 * 2 + 1, 3 * 2, true⟩                        -- … and one over it
  , ⟨twoThirds (2 ^ 32), 2 ^ 32, false⟩             -- the same boundary, computed the port's way
  , ⟨twoThirds (2 ^ 32) + 1, 2 ^ 32, true⟩
  , ⟨1, 1, true⟩
  , ⟨0, 0, false⟩
  , ⟨0, 1, false⟩
  , ⟨3, 4, true⟩                                    -- 75%
  , ⟨5, 8, false⟩                                   -- 62.5%: a majority, not a supermajority
  , ⟨2 ^ 62, 2 ^ 63 - 1, false⟩                     -- `3 * stake` is past the `i64` range …
  , ⟨2 ^ 63, 2 ^ 63 - 1, true⟩                      -- … and so is `stake` itself
  ]

/-- The layer's checker. -/
def stakeHolds (c : StakeCase) : Bool := decide (isSuperMajority c.stake c.total) == c.expected

/-- Every case holds of the model. `decide`: the predicate is `3·stake > 2·total` on `Nat`, so the kernel
    computes it — which is the point of the layer, since the node's spelling of the same comparison is
    what is being tied. -/
theorem stakeCases_decide : stakeCases.all stakeHolds = true := by decide

/-- The layer carries exactly `stakeCaseCount` cases. -/
def stakeCaseCount : Nat := 12

theorem stakeCases_length : stakeCases.length = stakeCaseCount := by decide

/-- **The layer is not degenerate**: both verdicts appear. -/
theorem stakeCases_verdicts :
    (stakeCases.map (fun c => c.expected)).eraseDups.length = 2 := by decide

/-- **And the width is exercised**: one case's `3 * stake` is past the `i64` range and another's `stake`
    is, so a comparison done in a 64-bit accumulator would decide at least one of them wrongly. That is
    the property the layer pins beyond the boundary itself — exactness *and* width. -/
theorem stakeCases_width :
    (stakeCases.any (fun c => 3 * c.stake > 2 ^ 63 - 1)) = true ∧
    (stakeCases.any (fun c => c.stake > 2 ^ 62)) = true := by decide

/-- One `stake` corpus line: layer, the two sums, the verdict. -/
def stakeLine (c : StakeCase) : String :=
  "stake\t" ++ toString c.stake ++ "\t" ++ toString c.total ++ "\t" ++
    (if c.expected then "true" else "false")

/-! ## The `block` layer — laws 16a and 16b, read off the node

The two number checks are one function each (`casper/src/validate.rs:141`, `:169`), and each answers
*valid* or a `BlockStatus` refusal. Each row here is a block as the checks read it — its number, its
sequence number, its sender, and its justifications as `(sender, number, seqNum, failed)` — with **two**
verdicts, one per law, and the consumer calls both node functions on the same data.

**One asymmetry is the layer's whole point.** The *model's* `BlockNumberValid` (`Casper/Validate.lean:70`)
is the max-non-failed-plus-one rule; the node's `block_number` enforces that rule **and** that every
resolved parent is *lower* than the block (H1b, `casper/src/validate.rs:152-154` — the port refuses a
parent at or above the child, failed or not; the Scala does not, which is §6's deviation). So the model
side of a case is the **conjunction** of the two, and the cases include one where they come apart: a
block whose only justification has failed. Its number must be `0` by the max rule and must exceed the
parent by the descent rule — so **no such block is valid**, which is a fact about the port that neither
check states alone. -/

/-- One `block` case: the validating block's three numbers, its justifications, and the two verdicts. -/
structure BlockCase where
  /-- The block's number. -/
  number : Nat
  /-- The block's sequence number. -/
  seqNum : Nat
  /-- The block's sender. -/
  sender : Nat
  /-- Its justifications as `(sender, number, seqNum, validationFailed)`. -/
  parents : List (Nat × Nat × Nat × Bool)
  /-- What the node's `block_number` must say (law 16a, with the descent rule). -/
  numberValid : Bool
  /-- What the node's `sequence_number` must say (law 16b). -/
  seqNumValid : Bool

/-- The model's `Block` for a case — the two inert fields (`timestamp`, `header`) are `0` and `[]`, which
    is what the existing falsifier witnesses use. -/
def blockOf (c : BlockCase) : Block :=
  ⟨c.number, c.seqNum, c.sender,
   c.parents.map (fun kv => ⟨kv.1, kv.2.1, kv.2.2.1, kv.2.2.2⟩), 0, []⟩

/-- The descent half of the node's `block_number`: every justification's recorded number is below the
    block's. A *failed* justification's recorded height is its claimed `block_num` (the port's
    `message_from_block_metadata`), so it is bounded too — which is what H1b added. -/
def parentsBelow (c : BlockCase) : Bool := c.parents.all (fun kv => decide (kv.2.1 < c.number))

/-- The model's `BlockNumberValid` as a `Bool`, so a case can be `decide`d against it: the predicate is a
    `match` on a computed `maxParentNumber`, and `Decidable` does not synthesize through that (measured —
    `failed to synthesize Decidable (BlockNumberValid (blockOf c))`). This is the register's own shape for
    such a mirror — a checker **proved** to be the predicate, not a second opinion. -/
def numberValidOf (b : Block) : Bool :=
  match maxParentNumber b.justifications with
  | none => b.number == 0
  | some n => b.number == n + 1

theorem numberValidOf_iff (b : Block) : numberValidOf b = true ↔ BlockNumberValid b := by
  unfold numberValidOf BlockNumberValid
  split <;> simp_all

/-- Law 16b's predicate as a `Bool`, for the same reason and with the same proof obligation. -/
def seqNumValidOf (b : Block) : Bool :=
  match senderLatestSeq b.sender b.justifications with
  | none => b.seqNum == 0
  | some n => b.seqNum == n + 1

theorem seqNumValidOf_iff (b : Block) : seqNumValidOf b = true ↔ SeqNumValid b := by
  unfold seqNumValidOf SeqNumValid
  split <;> simp_all

/-- A case holds when the node's two functions' verdicts are the case's. -/
def blockHolds (c : BlockCase) : Bool :=
  (numberValidOf (blockOf c) && parentsBelow c) == c.numberValid &&
  (seqNumValidOf (blockOf c)) == c.seqNumValid

/-- The cases: the base rule, the max rule, the skipped-failure rule, the two independence facts, and the
    failure-plus-descent case no valid block can satisfy. -/
def blockCases : List BlockCase :=
  [ ⟨0, 0, 0, [], true, true⟩                                       -- nothing to justify
  , ⟨2, 0, 0, [(1, 1, 0, false)], true, true⟩                       -- another sender's block: 16a only
  , ⟨2, 0, 2, [(1, 1, 0, false)], true, true⟩
  , ⟨4, 0, 0, [(1, 1, 0, false), (1, 3, 0, false)], true, true⟩     -- the max is the later parent
  , ⟨2, 0, 0, [(1, 1, 0, false), (1, 3, 0, false)], false, true⟩    -- … and one below it is refused
  , ⟨1, 5, 0, [(0, 0, 4, false)], true, true⟩                       -- the sender's latest decides 16b
  , ⟨1, 4, 0, [(0, 0, 4, false)], true, false⟩                      -- … and one below it is refused
  , ⟨2, 5, 0, [(0, 0, 1, false), (0, 1, 4, false)], true, true⟩     -- the latest is the max, not the last
  , ⟨1, 0, 1, [(0, 0, 9, false)], true, true⟩                       -- another sender's seqNum is ignored
  , ⟨0, 0, 0, [(1, 0, 0, true)], false, true⟩                       -- a failed justification: the rules part
  ]

/-- Every case holds of the model. `decide`: the two predicates are Nat comparisons over finite lists —
    the kernel reduces them — and `parentsBelow` is a list fold of the same shape. -/
theorem blockCases_decide : blockCases.all blockHolds = true := by decide

/-- The layer carries exactly `blockCaseCount` cases. -/
def blockCaseCount : Nat := 10

theorem blockCases_length : blockCases.length = blockCaseCount := by decide

/-- **The layer is not degenerate**: both verdicts appear *for each law*, so a checker answering one value
    everywhere cannot pass — and the two laws are exercised independently (the mixed cases). -/
theorem blockCases_verdicts :
    (blockCases.map (fun c => c.numberValid)).eraseDups.length = 2 ∧
    (blockCases.map (fun c => c.seqNumValid)).eraseDups.length = 2 ∧
    (blockCases.any (fun c => c.numberValid != c.seqNumValid)) = true := by decide

/-- One `block` corpus line: layer, the three numbers, the justifications, the two verdicts. Each
    justification is `sender:number:seqNum:failed`, so the column needs no quoting. -/
def blockLine (c : BlockCase) : String :=
  "block\t" ++ toString c.number ++ "\t" ++ toString c.seqNum ++ "\t" ++ toString c.sender ++ "\t" ++
    String.intercalate "," (c.parents.map fun kv =>
      s!"{kv.1}:{kv.2.1}:{kv.2.2.1}:{if kv.2.2.2 then "1" else "0"}") ++ "\t" ++
    (if c.numberValid then "true" else "false") ++ "\t" ++
    (if c.seqNumValid then "true" else "false")

/-! ## The `liveness` layer — law 58's window, read off the node

**What it ties.** Law 58's clause b turns on one predicate: a sender is *current* iff its latest message
is within the window of the tip. The model spells it `h + w ≥ tip` (`Rounds.lean`'s `currentOf`, which
`Rounds.lean`'s `window_iff_heights_behind` proves is the same test); the node spells it
`heights_behind(tip, h) ≤ window` (`block-storage/src/dag/liveness.rs:69`, `:93`). Each row is a tip, a
window, a bonds map and a latest-message map, and the verdict is the **live weight set** — the bonded
senders the filter keeps, with their stakes.

**The case the layer exists for is the absent one.** A bonded sender with **no message at all** is not
current: the node's lookup is an `is_some_and` and the model's is a `none => false`. That is C174's
requirement — without it a silent bonded validator makes the partition unsatisfiable and finality stops
whatever share the survivors hold (#70, measured 2026-09-29 at `100/100/50`). A row with a bonded sender
missing from `latest` is the check on it.

**Ordering is part of the encoding.** Both sides filter the bonds *in place* — the node over its
`BTreeMap`'s ascending keys, the model over its list — so the cases list bonds in ascending sender order
and the rendered set needs no sort. A case out of order would fail on a difference that is not the rule's.
-/

/-- A `liveness` case. -/
structure LivenessCase where
  /-- The tip the window is measured from. -/
  tip : Nat
  /-- `LIVENESS_WINDOW` as this case runs it. -/
  window : Nat
  /-- The bonds map, **ascending by sender** — the port's `BTreeMap` iteration order. -/
  bonds : List (Sender × Nat)
  /-- Each sender's latest height, ascending. **A bonded sender absent here has no message at all.** -/
  latest : List (Sender × Nat)
  /-- The live weight set the case expects, ascending. -/
  live : List (Sender × Nat)

/-- The live weight set the model computes for a case. -/
def livenessOf (c : LivenessCase) : List (Sender × Nat) := liveOf c.bonds c.latest c.tip c.window

/-- The layer's checker: the model's live set is the case's. -/
def livenessHolds (c : LivenessCase) : Bool := decide (livenessOf c = c.live)

/-- The cases. `100/100/50` is the rig's own bond shape — a tie to a shape nobody runs is a tie to
    nothing — and the quarantine/window arithmetic is small enough to check by hand. -/
def livenessCases : List LivenessCase :=
  [ -- every bonded sender current: the live set is the whole map
    ⟨10, 5, [(0, 100), (1, 100), (2, 50)], [(0, 10), (1, 9), (2, 8)], [(0, 100), (1, 100), (2, 50)]⟩
  , -- exactly at the window: `h + w = tip`, so the sender is live …
    ⟨10, 5, [(0, 100), (1, 100)], [(0, 5), (1, 6)], [(0, 100), (1, 100)]⟩
  , -- … and one past it is not: the same case, from the other side of the boundary
    ⟨10, 5, [(0, 100), (1, 100)], [(0, 4), (1, 10)], [(1, 100)]⟩
  , -- the case C174 is about: a bonded sender with NO message at all is dropped
    ⟨10, 5, [(0, 100), (1, 100), (2, 50)], [(0, 10), (1, 9)], [(0, 100), (1, 100)]⟩
  , -- nobody has spoken: the live set is empty while the quorum denominator is untouched
    ⟨10, 5, [(0, 100), (1, 100)], [], []⟩
  , -- a `latest` entry for a sender that is not bonded contributes nothing
    ⟨10, 5, [(0, 100)], [(0, 10), (7, 10)], [(0, 100)]⟩
  , -- window 0: only a message at the tip itself is live
    ⟨10, 0, [(0, 100), (1, 100)], [(0, 10), (1, 9)], [(0, 100)]⟩
  , -- a zero-stake bonded sender is live and contributes 0 — `NonNegI64` allows it
    ⟨5, 5, [(0, 0), (1, 7)], [(0, 5), (1, 5)], [(0, 0), (1, 7)]⟩
  , -- tip 0: the genesis boundary, where `h + w ≥ 0` holds for every present message
    ⟨0, 5, [(0, 3)], [(0, 0)], [(0, 3)]⟩
  , -- one of three silent and the other two current: 200 of the pool's 250
    ⟨100, 5, [(0, 100), (1, 100), (2, 50)], [(0, 100), (1, 96)], [(0, 100), (1, 100)]⟩
  , -- a message *ahead* of the tip is current, not stale — the sign `heights_behind`'s doc names
    ⟨10, 5, [(0, 100)], [(0, 12)], [(0, 100)]⟩
  ]

/-- **Every case holds of the model**: the live set `liveOf` computes is the set the case declares. The
    emitted corpus carries the declared verdict, so this theorem is what makes the file the model's
    belief rather than a list somebody typed. -/
theorem livenessCases_decide : livenessCases.all livenessHolds = true := by decide

/-- The layer carries exactly `livenessCaseCount` cases. -/
def livenessCaseCount : Nat := 11

theorem livenessCases_length : livenessCases.length = livenessCaseCount := by decide

/-- **The layer is not degenerate**: at least one case drops a bonded sender and at least one keeps every
    one of them. A layer that only ever returned the whole map would pass while the predicate did
    nothing. -/
theorem livenessCases_both_ways :
    livenessCases.any (fun c => decide (livenessOf c ≠ c.bonds)) = true ∧
    livenessCases.any (fun c => decide (livenessOf c = c.bonds)) = true := by decide

/-- **And the silent case is among them**: a case has fewer latest messages than bonds, which is the shape
    C174 requires to be dropped and #70 measured the absence of. -/
theorem livenessCases_drops_the_silent :
    livenessCases.any (fun c => decide (c.latest.length < c.bonds.length)) = true := by decide

/-- One `liveness` corpus line: layer, tip, window, bonds, latest, and the live set. -/
def livenessLine (c : LivenessCase) : String :=
  String.intercalate "\t"
    ["liveness", toString c.tip, toString c.window,
     String.intercalate "," (c.bonds.map fun p => s!"{p.1}:{p.2}"),
     String.intercalate "," (c.latest.map fun p => s!"{p.1}:{p.2}"),
     String.intercalate "," (c.live.map fun p => s!"{p.1}:{p.2}")]

end Corpus
end Rchain

open Rchain

/-! ## Law 16c — the `body` layer (AUDIT C57's option (b))

**What it ties.** The port has no `BodyProto`: a block is one `BlockMessageProto` with tags 1–17, and
`hash_block` clears exactly `blockHash` and `sig` and encodes the rest with `prost`. The model's
`encodeBody` narrows that to six fields, and this layer holds the model's `prost`-mirroring encoder
(`Rchain.Casper.Validate`'s `encodeProstBody`) and the node's `prost` to the **same bytes** — the two
rules a hand-written encoder gets wrong being **field order** (the model writes 4, 6, 5, 17, 9, 14;
`prost` writes ascending) and **default-skipping** (`prost` omits a field equal to its default; the
model writes unconditionally).

**The rows carry the node's bytes, observed first.** `node/tests/lean_body_corpus.rs`'s
`the_bytes_the_node_produces` printed `prost`'s encoding of these cases before any row was written; the
`bytesHex` below is that output, and `bodyCases_decide` then checks that the model's encoder reproduces
it. So the expectation is the node's answer, not the model's — the difference between this layer and a
corpus that only agrees with itself, and the reason the falsifier for it is a *Rust* mutation (the Lean
theorem alone would only show the model is self-consistent).

**The boundary, in two kinds, and they are not the same kind of omission.** *Fields omitted*:
`version`, `shardId`, `blockHash`, `preStateHash`, `postStateHash`, `bonds`, the three `rejected*` sets
and `sigAlgorithm` are in the proto and are not modelled here at all — that is C57's option (a).
*A field deliberately taken in the proto's shape*: `justifications` is `repeated bytes` in the proto and
a list of `Parent`s in the model, two representations of **different data** rather than two spellings of
one thing, so the encoder takes the proto's shape and the model's `Parent` stays outside. A reader who
"fixes" the second member back to `encodeParent` would be inventing a correspondence the node lacks.

**`state` is written unconditionally, and the observation is what decided that.** `prost`'s derive
writes a *present* sub-message even when it serialises to nothing ("present but empty" is not "absent");
the first row of the table below is the case where that is all that is written (`7200`), and the
observation confirmed it against the alternative reading, which would have skipped the field.

**This is the repository's first byte-level corpus layer.** Every other layer pins an identity (which
element sorts first, which token a spelling lexes to); a byte-level layer pins an *encoding*, and a
round trip cannot see either failure above (`from_bytes (to_bytes b) = b` holds under both orders and
every skipping rule). -/

/-- One hex digit, as a value below 16. The bounds are stated on `c.toNat` rather than with `Char`'s
    order, because `omega` cannot see through the latter. -/
def hexDigit? (c : Char) : Option (Fin 16) :=
  if h : 48 ≤ c.toNat ∧ c.toNat ≤ 57 then some ⟨c.toNat - 48, by omega⟩
  else if h : 97 ≤ c.toNat ∧ c.toNat ≤ 102 then some ⟨c.toNat - 87, by omega⟩
  else if h : 65 ≤ c.toNat ∧ c.toNat ≤ 70 then some ⟨c.toNat - 55, by omega⟩
  else none

/-- Lower-case or upper-case hex back to bytes. Corpus-local because this file is its only caller;
    `none` on a malformed string, so a mistyped row *fails* the layer's theorem rather than silently
    comparing against a truncated one. -/
def hexDecode? (s : String) : Option Msg :=
  let rec go : List Char → Msg → Option Msg
    | [], acc => some acc.reverse
    | a :: b :: rest, acc =>
      match hexDigit? a, hexDigit? b with
      | some x, some y => go rest (⟨x.val * 16 + y.val, by omega⟩ :: acc)
      | _, _ => none
    | [_], _ => none
  go s.toList []

/-- One `body` corpus line's case: the modelled subset as text, plus the bytes the node produced for it
    — observed from the node *before* the row was written. -/
structure BodyCase where
  /-- proto field 4. -/
  blockNumber : Nat
  /-- proto field 5, hex. -/
  senderHex : String
  /-- proto field 6. -/
  seqNum : Nat
  /-- proto field 9, `repeated bytes`, one hex string per entry. -/
  justificationsHex : List String
  /-- proto field 14, hex. Empty is the *present* empty sub-message, not an absent field. -/
  stateHex : String
  /-- proto field 17. `0` is the genesis value, which `prost` omits. -/
  timestamp : Nat
  /-- The node's bytes for this case, hex — the observation, not the model's output. -/
  bytesHex : String

/-- The case as the modelled subset, once every hex field has decoded. -/
def BodyCase.toProstBody (c : BodyCase) : Option ProstBody :=
  match hexDecode? c.senderHex, hexDecode? c.stateHex, c.justificationsHex.mapM hexDecode? with
  | some sender, some state, some js =>
    some { blockNumber := c.blockNumber, sender := sender, seqNum := c.seqNum,
           justifications := js, state := state, timestamp := c.timestamp }
  | _, _, _ => none

/-- The layer's check: the model's `prost`-mirroring encoder reproduces the node's observed bytes. A
    malformed hex field fails rather than passing vacuously. -/
def bodyHolds (c : BodyCase) : Bool :=
  match c.toProstBody, hexDecode? c.bytesHex with
  | some b, some bytes => encodeProstBody b == bytes
  | _, _ => false

def bodyCases : List BodyCase := [
  -- Order: four non-default fields, so the field order is observable from the second field onwards.
  -- The two justifications are distinct, so each one's own key+length is pinned rather than one length
  -- covering the pair.
  { blockNumber := 7,
    senderHex := "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20",
    seqNum := 3,
    justificationsHex := ["02030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f2021",
                          "030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122"],
    stateHex := "", timestamp := 1000,
    bytesHex := "20072a200102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f2030034a2002030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20214a20030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20212272008801e807" },
  -- Skipping at its extreme: everything default, so the present-but-empty `state` is the *only* field
  -- written. This is the row that decides the `state` reading named in the section comment.
  { blockNumber := 0, senderHex := "", seqNum := 0, justificationsHex := [], stateHex := "",
    timestamp := 0, bytesHex := "7200" },
  -- A genesis block with a sender: `timestamp = 0` is C57's smallest instance of the difference,
  -- alongside `blockNumber` and `seqNum`.
  { blockNumber := 0,
    senderHex := "0405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20212223",
    seqNum := 0,
    justificationsHex := ["05060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f2021222324"],
    stateHex := "", timestamp := 0,
    bytesHex := "2a200405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122234a2005060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20212223247200" },
  -- Short values: a one-byte sender and a one-byte justification pin the length prefixes at their
  -- smallest non-zero, which is where an off-by-one in a length shows up.
  { blockNumber := 1, senderHex := "2a", seqNum := 2, justificationsHex := ["7f"], stateHex := "",
    timestamp := 0, bytesHex := "20012a012a30024a017f7200" },
  -- No justifications at all, and a timestamp that is not the last field written: pins that an empty
  -- repeated field contributes nothing, and that a large varint is written whole.
  { blockNumber := 42,
    senderHex := "060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425",
    seqNum := 1, justificationsHex := [], stateHex := "", timestamp := 9999999,
    bytesHex := "202a2a20060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425300172008801fface204" } ]

/-- The layer carries exactly `bodyCaseCount` cases. -/
def bodyCaseCount : Nat := 5

theorem bodyCases_length : bodyCases.length = bodyCaseCount := by decide

/-- **The model reproduces the node's bytes** on every row. `native_decide`, for the reason the other
    layers use it: the reduction is deep for the kernel and fast for the compiler — `decide` fails here,
    measured 2026-09-25 with the other three. -/
theorem bodyCases_decide : bodyCases.all bodyHolds = true := by native_decide

/-- A byte string as lower-case hex, for the emitted row. -/
def bodyHexOf (b : Msg) : String := hexEncode (b.map (fun x => x.val))

/-- One `body` corpus line: layer, the modelled subset, and the bytes the node produces for it. The
    Rust consumer parses the same columns, so a row is the two encoders' shared expectation. -/
def bodyLine (c : BodyCase) : String :=
  String.intercalate "\t"
    ["body", toString c.blockNumber, c.senderHex, toString c.seqNum, toString c.timestamp,
     String.intercalate ";" c.justificationsHex, c.stateHex, c.bytesHex]

namespace Rchain
namespace Corpus

/-! ## Law 86 — the admission layer -/

/-- The admission corpus cases, each a shape the Lean pool and the Rust `casper::pool` policy both
    answer. The Rust consumer drives `check_admission`/`check_commit` and `budget::permits_for`; the
    cases that are not shared (rate/share bounds, the relay) stay unit tests rather than being
    claimed by a corpus that does not exercise them. -/
inductive AdmissionCase where
  | freeSlot
  | countBound
  | duplicateBound
  | onePermit
  | zeroHeadroom
  deriving DecidableEq, Repr

def admissionCaseCount : Nat := 5

open Admission in
def admissionHolds (c : AdmissionCase) : Bool :=
  match c with
  | .freeSlot =>
      match enqueue { capacity := 1, entries := [] } 0 with
      | .admitted _ => true
      | .refused _ _ => false
  | .countBound =>
      match enqueue { capacity := 1, entries := [{ id := 0, status := .pending }] } 1 with
      | .refused _ .full => true
      | _ => false
  | .duplicateBound =>
      match enqueue { capacity := 2, entries := [{ id := 0, status := .pending }] } 0 with
      | .refused _ .duplicate => true
      | _ => false
  | .onePermit =>
      match next
          { pool := { capacity := 1, entries := [{ id := 0, status := .pending }] },
            permits := 1 } with
      | some (batch, _) => !batch.isEmpty
      | none => false
  | .zeroHeadroom =>
      match next
          { pool := { capacity := 1, entries := [{ id := 0, status := .pending }] },
            permits := 0 } with
      | none => true
      | some _ => false

def admissionCases : List AdmissionCase :=
  [.freeSlot, .countBound, .duplicateBound, .onePermit, .zeroHeadroom]

theorem admissionCases_decide : admissionCases.all admissionHolds = true := by decide

theorem admissionCases_length : admissionCases.length = admissionCaseCount := by decide

def admissionCaseName : AdmissionCase → String
  | .freeSlot => "free_slot"
  | .countBound => "count_bound"
  | .duplicateBound => "duplicate_bound"
  | .onePermit => "one_permit"
  | .zeroHeadroom => "zero_headroom"

def admissionLine (c : AdmissionCase) : String :=
  String.intercalate "\t"
    ["admission", admissionCaseName c, if admissionHolds c then "true" else "false"]


/-! ## Law 90 — the publishable hostlist -/

def hostPeer (id : Nat) : Rchain.HostList.Peer :=
  { id := id, host := "h", protocolPort := 40400 }

def hostCtx (selfId : Nat) (selfProposer : Bool) (maxEntries : Nat) : Rchain.HostList.Ctx :=
  { networkId := "testnet", shardIds := ["/root"], maxEntries := maxEntries,
    self := hostPeer selfId, selfProposer := selfProposer }

def hostAnswer (id : Nat) : Option Rchain.HostList.Answer :=
  some { address := id, networkId := "testnet", shardId := "/root", proposer := true }

def hostObserver (id : Nat) : Option Rchain.HostList.Answer :=
  some { address := id, networkId := "testnet", shardId := "/root", proposer := false }

def hostForeignNetwork (id : Nat) : Option Rchain.HostList.Answer :=
  some { address := id, networkId := "othernet", shardId := "/root", proposer := true }

def hostCase (name : String) (ctx : Rchain.HostList.Ctx)
    (os : List Rchain.HostList.Outcome) (expected : List Nat) : Rchain.HostList.Case :=
  { name := name, ctx := ctx, outcomes := os, expected := expected }

/-- **The hostlist cases.** Each is a shape both sides run: the Lean `servedIds` above and the Rust
    `derive_hostlist`. The corpus line carries the **ids the derivation must produce**, so a drift in
    the filter, the ordering, dedupe, the bound or self's presence fails the tie on one side or the
    other — there is no expected value duplicated in prose. -/
def hostListCases : List Rchain.HostList.Case :=
  [ hostCase "self_only" (hostCtx 0 true 8) []
      [0]
  , hostCase "observer_excluded" (hostCtx 0 true 8)
      [{ peer := hostPeer 1, answer := hostObserver 1 }]
      [0]
  , hostCase "wrong_network" (hostCtx 0 true 8)
      [{ peer := hostPeer 1, answer := hostForeignNetwork 1 }]
      [0]
  , hostCase "wrong_shard" (hostCtx 0 true 8)
      [{ peer := hostPeer 1, answer := some { address := 1, networkId := "testnet", shardId := "/child", proposer := true } }]
      [0]
  , hostCase "unreachable" (hostCtx 0 true 8)
      [{ peer := hostPeer 1, answer := none }]
      [0]
  , hostCase "address_mismatch" (hostCtx 0 true 8)
      [{ peer := hostPeer 1, answer := some { address := 2, networkId := "testnet", shardId := "/root", proposer := true } }]
      [0]
  , hostCase "three_valid_sorted" (hostCtx 0 true 8)
      [{ peer := hostPeer 5, answer := hostAnswer 5 },
       { peer := hostPeer 3, answer := hostAnswer 3 },
       { peer := hostPeer 1, answer := hostAnswer 1 }]
      [0, 1, 3, 5]
  , hostCase "truncation_self_first" (hostCtx 0 true 2)
      [{ peer := hostPeer 1, answer := hostAnswer 1 },
       { peer := hostPeer 2, answer := hostAnswer 2 },
       { peer := hostPeer 3, answer := hostAnswer 3 }]
      [0, 1]
  , hostCase "duplicate_ids" (hostCtx 0 true 8)
      [{ peer := hostPeer 1, answer := hostAnswer 1 },
       { peer := hostPeer 1, answer := hostAnswer 1 }]
      [0, 1]
  , hostCase "self_not_a_proposer" (hostCtx 0 false 8)
      [{ peer := hostPeer 7, answer := hostAnswer 7 }]
      [7]
  ]

def hostListCaseCount : Nat := 10

def hostListHolds (c : Rchain.HostList.Case) : Bool :=
  Rchain.HostList.servedIds c.ctx c.outcomes == c.expected

theorem hostListCases_decide : hostListCases.all hostListHolds = true := by decide

theorem hostListCases_length : hostListCases.length = hostListCaseCount := by decide

def hostListLine (c : Rchain.HostList.Case) : String :=
  String.intercalate "\t"
    ["hostlist", c.name,
      if c.expected.isEmpty then "-" else String.intercalate "," (c.expected.map toString)]


/-! ## Law 51d — the paced gate (the `recovery` layer) -/

/-- One `recovery` case. `pace` is Law 51d's gate (first sighting, tick, expected
    `(escape?, new sighting)`); `eligible` is Law 88c's sweep predicate (stage, since, now,
    expected). The Rust consumer dispatches on the kind and calls the same predicate. -/
inductive RecoveryCase where
  | pace (first : Option Nat) (now : Nat) (expected : Bool × Option Nat)
  | eligible (stage : Rchain.Disposition.Recovery.Stage) (since : Nat) (now : Nat)
      (expected : Bool)
  | classify (inMap regression : Bool) (expected : String)
  deriving Repr

def recoveryHolds : RecoveryCase → Bool
  | .pace first now expected =>
      decide (Rchain.Pacing.take ⟨first⟩ now = (expected.1, { since := expected.2 }))
  | .eligible stage since now expected =>
      decide (Rchain.Disposition.Recovery.eligible ⟨stage, since⟩ now = expected)
  | .classify inMap regression expected =>
      decide (Rchain.Disposition.Recovery.outcomeName
        (Rchain.Disposition.Recovery.classify inMap regression) = expected)

def recoveryCases : List RecoveryCase :=
  [ .pace none 0 (false, some 0)
  , .pace (some 0) 14999 (false, some 0)
  , .pace (some 0) 15000 (true, some 15000)
  , .pace (some 0) 15001 (true, some 15001)
  , .pace (some 0) 30000 (true, some 30000)
  , .pace (some 15000) 15001 (false, some 15000)
  , .pace (some 15000) 30000 (true, some 30000)
  , .eligible .stored 0 59999 false
  , .eligible .stored 0 60000 true
  , .eligible .validating 0 60000 false
  , .eligible .retry 0 60000 false
  , .classify true true "regression"
  , .classify true false "pass"
  , .classify false true "unresolved"
  , .classify false false "unresolved"
  ]

def recoveryCaseCount : Nat := 15

theorem recoveryCases_decide : recoveryCases.all recoveryHolds = true := by decide

theorem recoveryCases_length : recoveryCases.length = recoveryCaseCount := by decide

def recoveryStageName : Rchain.Disposition.Recovery.Stage → String
  | .stored => "stored"
  | .validating => "validating"
  | .retry => "retry"

def recoveryLine : RecoveryCase → String
  | .pace first now expected =>
      String.intercalate "\t"
        ["recovery", "pace",
          match first with | none => "-" | some n => toString n,
          toString now,
          if expected.1 then "true" else "false",
          match expected.2 with | none => "-" | some n => toString n]
  | .eligible stage since now expected =>
      String.intercalate "\t"
        ["recovery", "eligible", recoveryStageName stage,
          toString since, toString now, if expected then "true" else "false"]
  | .classify inMap regression expected =>
      String.intercalate "\t"
        ["recovery", "classify", if inMap then "true" else "false",
          if regression then "true" else "false", expected]


/-! ## Law 87 — the decimal value algebra -/

open Rchain.Decimal in
inductive DecimalOp where
  | mk | parts | scaled | cmp | add | sub | mul | div
  deriving DecidableEq, Repr

open Rchain.Decimal in
inductive DecimalOutcome where
  | value (d : Decimal)
  | scaled (n : Int)
  | cmp (o : Ordering)
  | error (e : DecimalError)
  deriving DecidableEq, Repr

structure DecimalCase where
  name : String
  op : DecimalOp
  a : Decimal
  b : Option Decimal
  expected : DecimalOutcome
  deriving DecidableEq, Repr

open Rchain.Decimal in
def decimalRun (c : DecimalCase) : DecimalOutcome :=
  let b := c.b.getD ⟨0, 0⟩
  match c.op with
  | .mk =>
      match Decimal.new c.a.coefficient c.a.scale with
      | .ok d => .value d
      | .error e => .error e
  | .parts =>
      match Decimal.fromParts c.a.coefficient c.a.scale with
      | .ok d => .value d
      | .error e => .error e
  | .scaled => .scaled (Decimal.scaledValue c.a)
  | .cmp => .cmp (Decimal.cmpNumeric c.a b)
  | .add => .value (Decimal.add c.a b)
  | .sub => .value (Decimal.sub c.a b)
  | .mul =>
      match Decimal.mul c.a b with
      | .ok d => .value d
      | .error e => .error e
  | .div =>
      match Decimal.div c.a b with
      | .ok d => .value d
      | .error e => .error e

def decimalHolds (c : DecimalCase) : Bool := decide (decimalRun c = c.expected)

def dec (c : Int) (s : Nat) : Decimal := ⟨c, s⟩

open Rchain.Decimal in
def decOk (c : Int) (s : Nat) : DecimalOutcome := .value (dec c s)

open Rchain.Decimal in
def decScaled (n : Int) : DecimalOutcome := .scaled n

def decCmp (o : Ordering) : DecimalOutcome := .cmp o

open Rchain.Decimal in
def decErr (e : DecimalError) : DecimalOutcome := .error e

def decimalCases : List DecimalCase :=
  [ -- new: canonicalisation, zero, the bounds and the two constructor orderings
    { name := "new_negative_strip", op := .mk, a := dec (-110) 2, b := none,
      expected := decOk (-11) 1 },
    { name := "new_strip", op := .mk, a := dec 110 2, b := none, expected := decOk 11 1 },
    { name := "new_zero_scale", op := .mk, a := dec 0 5, b := none, expected := decOk 0 0 },
    { name := "new_zero_above_bound", op := .mk, a := dec 0 39, b := none,
      expected := decOk 0 0 },
    { name := "new_scale_38", op := .mk, a := dec 1 38, b := none, expected := decOk 1 38 },
    { name := "new_scale_39", op := .mk, a := dec 1 39, b := none,
      expected := decErr (.scaleOutOfRange 39) },
    { name := "new_huge_strip", op := .mk, a := dec ((10 : Int) ^ 39) 39, b := none,
      expected := decOk 1 0 },
    -- parts: the strict constructor and its two named refusals
    { name := "parts_negative", op := .parts, a := dec (-11) 1, b := none,
      expected := decOk (-11) 1 },
    { name := "parts_zero", op := .parts, a := dec 0 0, b := none, expected := decOk 0 0 },
    { name := "parts_zero_with_scale", op := .parts, a := dec 0 3, b := none,
      expected := decErr (.zeroWithScale 3) },
    { name := "parts_trailing_zero", op := .parts, a := dec 110 2, b := none,
      expected := decErr (.trailingZero 2) },
    { name := "parts_scale_39", op := .parts, a := dec 1 39, b := none,
      expected := decErr (.scaleOutOfRange 39) },
    -- scaled: the payload is the value in units of 10^-38
    { name := "scaled_one", op := .scaled, a := dec 1 0, b := none,
      expected := decScaled ((10 : Int) ^ 38) },
    { name := "scaled_fifteen", op := .scaled, a := dec 15 1, b := none,
      expected := decScaled (15 * (10 : Int) ^ 37) },
    { name := "scaled_zero", op := .scaled, a := dec 0 0, b := none, expected := decScaled 0 },
    { name := "scaled_negative", op := .scaled, a := dec (-11) 1, b := none,
      expected := decScaled (-11 * (10 : Int) ^ 37) },
    -- cmp: cross-scale, signs and the 38 boundary
    { name := "cmp_lt_cross", op := .cmp, a := dec 15 1, b := some (dec 2 0),
      expected := decCmp .lt },
    { name := "cmp_gt_cross", op := .cmp, a := dec 2 0, b := some (dec 15 1),
      expected := decCmp .gt },
    { name := "cmp_eq", op := .cmp, a := dec 5 1, b := some (dec 5 1), expected := decCmp .eq },
    { name := "cmp_negative", op := .cmp, a := dec (-15) 1, b := some (dec (-2) 0),
      expected := decCmp .gt },
    { name := "cmp_boundary", op := .cmp, a := dec 1 38, b := some (dec 2 38),
      expected := decCmp .lt },
    { name := "cmp_zero", op := .cmp, a := dec 0 0, b := some (dec 1 38),
      expected := decCmp .lt },
    -- add: alignment, carry/strip, cancellation and the 38 boundary
    { name := "add_cross", op := .add, a := dec 15 1, b := some (dec 2 0),
      expected := decOk 35 1 },
    { name := "add_boundary_strip", op := .add, a := dec 9 38, b := some (dec 1 38),
      expected := decOk 1 37 },
    { name := "add_cancel", op := .add, a := dec 1 0, b := some (dec (-1) 0),
      expected := decOk 0 0 },
    { name := "add_negative", op := .add, a := dec (-15) 1, b := some (dec (-2) 0),
      expected := decOk (-35) 1 },
    { name := "add_carry", op := .add, a := dec 5 1, b := some (dec 5 1),
      expected := decOk 1 0 },
    -- sub
    { name := "sub_cross", op := .sub, a := dec 2 0, b := some (dec 15 1), expected := decOk 5 1 },
    { name := "sub_boundary", op := .sub, a := dec 9 38, b := some (dec 1 38),
      expected := decOk 8 38 },
    { name := "sub_negative", op := .sub, a := dec 0 0, b := some (dec 1 38),
      expected := decOk (-1) 38 },
    -- mul: exact products, the scale-2 strip and the scale-40 boundary
    { name := "mul_cross", op := .mul, a := dec 15 1, b := some (dec 2 0),
      expected := decOk 3 0 },
    { name := "mul_square", op := .mul, a := dec 15 1, b := some (dec 15 1),
      expected := decOk 225 2 },
    { name := "mul_strip", op := .mul, a := dec 5 1, b := some (dec 2 1),
      expected := decOk 1 1 },
    { name := "mul_scale_40_error", op := .mul, a := dec 1 20, b := some (dec 1 20),
      expected := decErr (.scaleOutOfRange 40) },
    { name := "mul_scale_40_strip", op := .mul, a := dec 25 20, b := some (dec 4 20),
      expected := decOk 1 38 },
    { name := "mul_zero", op := .mul, a := dec 0 0, b := some (dec 1 38),
      expected := decOk 0 0 },
    -- div: the exact-or-error criterion, including 2^38 and 2^39
    { name := "div_half", op := .div, a := dec 1 0, b := some (dec 2 0), expected := decOk 5 1 },
    { name := "div_third_inexact", op := .div, a := dec 1 0, b := some (dec 3 0),
      expected := decErr .inexactDivision },
    { name := "div_two_pow_38", op := .div, a := dec 1 0,
      b := some (dec 274877906944 0),
      expected := decOk 363797880709171295166015625 38 },
    { name := "div_two_pow_39_inexact", op := .div, a := dec 1 0,
      b := some (dec 549755813888 0), expected := decErr .inexactDivision },
    { name := "div_zero", op := .div, a := dec 1 0, b := some (dec 0 0),
      expected := decErr .divisionByZero },
    { name := "div_negative", op := .div, a := dec (-1) 0, b := some (dec 2 0),
      expected := decOk (-5) 1 } ]

def decimalCaseCount : Nat := 42

theorem decimalCases_decide : decimalCases.all decimalHolds = true := by decide

theorem decimalCases_length : decimalCases.length = decimalCaseCount := by decide

theorem decimalCases_ops :
    (decimalCases.map (fun c => c.op)).eraseDups.length = 8 := by decide

theorem decimalCases_verdicts :
    (decimalCases.filterMap (fun c =>
      match c.expected with
      | .cmp o => some o
      | _ => none)).eraseDups.length = 3 := by decide

def decimalOpName : DecimalOp → String
  | .mk => "new"
  | .parts => "parts"
  | .scaled => "scaled"
  | .cmp => "cmp"
  | .add => "add"
  | .sub => "sub"
  | .mul => "mul"
  | .div => "div"

def decimalRender (d : Decimal) : String :=
  toString d.coefficient ++ ":" ++ toString d.scale

open Rchain.Decimal in
def decimalErrorName : DecimalError → String
  | .scaleOutOfRange s => "scale_out_of_range:" ++ toString s
  | .zeroWithScale s => "zero_with_scale:" ++ toString s
  | .trailingZero s => "trailing_zero:" ++ toString s
  | .divisionByZero => "division_by_zero"
  | .inexactDivision => "inexact_division"

def decimalOutcomeKind : DecimalOutcome → String
  | .value _ => "ok"
  | .scaled _ => "scaled"
  | .cmp _ => "cmp"
  | .error _ => "error"

def decimalOutcomePayload : DecimalOutcome → String
  | .value d => decimalRender d
  | .scaled n => toString n
  | .cmp .lt => "lt"
  | .cmp .eq => "eq"
  | .cmp .gt => "gt"
  | .error e => decimalErrorName e

def decimalLine (c : DecimalCase) : String :=
  String.intercalate "\t"
    ["decimal", c.name, decimalOpName c.op, decimalRender c.a,
     match c.b with | some b => decimalRender b | none => "-",
     decimalOutcomeKind c.expected, decimalOutcomePayload c.expected]

end Corpus
end Rchain

/-- `rchain-corpus --layer {flags|match|silence|store|protocol} [--out FILE]` — print the corpus
(stdout by default). -/
def main (args : List String) : IO UInt32 := do
  let want :=
    (args.find? (fun a => a == "flags" || a == "match" || a == "silence" || a == "store"
      || a == "c21" || a == "protocol" || a == "json" || a == "envelope"
      || a == "syrup"
      || a == "lex" || a == "sort" || a == "parse" || a == "body" || a == "closed"
      || a == "stake" || a == "block" || a == "liveness" || a == "admission"
      || a == "decimal" || a == "hostlist" || a == "recovery")).getD "flags"
  let (lines, count) :=
    if want == "c21" then (Corpus.c21Cases.map Corpus.c21Line, Corpus.c21CaseCount)
    else if want == "match" then (Corpus.matchCases.map Corpus.matchLine, Corpus.matchCaseCount)
    else if want == "silence" then
      (Corpus.silenceCases.map Corpus.silenceLine, Corpus.silenceCaseCount)
    else if want == "store" then
      (Corpus.storeCases.map Corpus.storeLine, Corpus.storeCaseCount)
    else if want == "protocol" then
      (replyCatalog.map Corpus.protocolLine, replyCaseCount)
    else if want == "json" then
      (Corpus.jsonCases.map Corpus.jsonLine, Corpus.jsonCaseCount)
    else if want == "syrup" then
      (Corpus.syrupCases.map Corpus.syrupLine, Corpus.syrupCaseCount)
    else if want == "envelope" then
      (envelopeCatalog.map Corpus.envelopeLine, envelopeCaseCount)
    else if want == "sort" then
      (Corpus.sortCases.map Corpus.sortLine, Corpus.sortCaseCount)
    else if want == "lex" then
      (lexemes.map Corpus.lexLine, lexemeCount)
    else if want == "parse" then
      (parseCases.map Corpus.parseLine, parseCaseCount)
    else if want == "body" then
      (bodyCases.map bodyLine, bodyCaseCount)
    else if want == "closed" then
      (Corpus.closedCases.map Corpus.closedLine, Corpus.closedCaseCount)
    else if want == "stake" then
      (Corpus.stakeCases.map Corpus.stakeLine, Corpus.stakeCaseCount)
    else if want == "block" then
      (Corpus.blockCases.map Corpus.blockLine, Corpus.blockCaseCount)
    else if want == "liveness" then
      (Corpus.livenessCases.map Corpus.livenessLine, Corpus.livenessCaseCount)
    else if want == "admission" then
      (Corpus.admissionCases.map Corpus.admissionLine, Corpus.admissionCaseCount)
    else if want == "decimal" then
      (Corpus.decimalCases.map Corpus.decimalLine, Corpus.decimalCaseCount)
    else if want == "hostlist" then
      (Corpus.hostListCases.map Corpus.hostListLine, Corpus.hostListCaseCount)
    else if want == "recovery" then
      (Corpus.recoveryCases.map Corpus.recoveryLine, Corpus.recoveryCaseCount)
    else (Corpus.flagCases.map Corpus.flagLine, Corpus.flagCaseCount)
  if lines.length != count then
    IO.eprintln s!"rchain-corpus: {want}: the case list and the declared count disagree"
    return 1
  let out := String.intercalate "\n" lines
  match args.findIdx? (fun a => a == "--out") with
  | some i =>
    match args[i + 1]? with
    | some path => IO.FS.writeFile path (out ++ "\n")
    | none => IO.eprintln "rchain-corpus: --out needs a path"; return 1
  | none => IO.println out
  return 0

