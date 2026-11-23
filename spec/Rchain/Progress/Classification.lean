import Rchain.Progress

/-!
# The deadly-embrace classification

A stall that stops block production is one failure mode with many surface forms. This module is the
canonical vocabulary for it, so the register does not grow a new law for every surface form:

* **Representation** — what kind of thing is stuck. `closedSilence` is a closed ρ term with no
  counterpart (the calculus, not a node defect); `protocolWait` has a step a delivery or round would
  enable; `protocolStall` has no restoring rule inside the observation window.
* **Shape** — Law 51's vocabulary (`Void`, `Terminal`, `Drift`, `Split`, `Historic`, `Stuck`, `Silent`,
  `Persistent`, `Unrestorable`, `Paced`).
* **Root cause** — where the fix goes, including the two causes of the cause: self-referential gating
  and loss-blind refusal.

`attributions` is the crosswalk: an explicit row for every law the classification must account for.
Every other register entry defaults to `independent`, and the register's check 11 refuses a progress
law that has been left at that default. The crosswalk is a lens, not a theorem: it does not claim the
128 laws are one theorem, and most of them are not about stalls at all.

**What is not here.** Positive liveness. `Fair` is named and unmodelled in `Rchain.Progress`; the wait
model below says a step is available, never that it is taken. The classification, attribution and
containment are the honest foundation, and the residue list records what the transition relation
cannot state at all.
-/

namespace Rchain
namespace Progress
namespace Classification

/-- What kind of thing is stuck. -/
inductive Representation where
  | closedSilence
  | protocolWait
  | protocolStall
  deriving DecidableEq, Repr

/-- Law 51's shapes, as an enumerated axis rather than a list of prose names. -/
inductive Shape where
  | void
  | terminal
  | drift
  | split
  | historic
  | stuck
  | silent
  | persistent
  | unrestorable
  | paced
  deriving DecidableEq, Repr

/-- Where the fix goes. `selfReferentialGating` and `lossBlindRefusal` are the two causes of the cause;
    the rest are the surface forms the HAZOP rows name. -/
inductive RootCause where
  | unsatisfiableRequirement
  | selfReferentialGating
  | lossBlindRefusal
  | divergentReaders
  | historicPredicate
  | recoveryBeforeAnchor
  | prematureVerdict
  | unverifiedTrust
  | mixedVersion
  deriving DecidableEq, Repr

/-- The HAZOP disposition ladder. -/
inductive Rung where
  | unrepresentable
  | failLoudly
  | resetAndCatchUp
  | runbook
  deriving DecidableEq, Repr

/-- A law's role in the classification. `ontology` states the vocabulary; `instance` is a law that
    inhabits a cell; `guard` is a law whose proof reddens when a fix is removed; `witness` is a
    non-vacuity example; `independent` is a law the classification deliberately does not claim. -/
inductive Role where
  | ontology
  | instanceRole
  | guard
  | witness
  | independent
  deriving DecidableEq, Repr

def Representation.wire : Representation → String
  | .closedSilence => "closed-silence"
  | .protocolWait => "protocol-wait"
  | .protocolStall => "protocol-stall"

def Shape.wire : Shape → String
  | .void => "void"
  | .terminal => "terminal"
  | .drift => "drift"
  | .split => "split"
  | .historic => "historic"
  | .stuck => "stuck"
  | .silent => "silent"
  | .persistent => "persistent"
  | .unrestorable => "unrestorable"
  | .paced => "paced"

def RootCause.wire : RootCause → String
  | .unsatisfiableRequirement => "unsatisfiable-requirement"
  | .selfReferentialGating => "self-referential-gating"
  | .lossBlindRefusal => "loss-blind-refusal"
  | .divergentReaders => "divergent-readers"
  | .historicPredicate => "historic-predicate"
  | .recoveryBeforeAnchor => "recovery-before-anchor"
  | .prematureVerdict => "premature-verdict"
  | .unverifiedTrust => "unverified-trust"
  | .mixedVersion => "mixed-version"

def Rung.wire : Rung → String
  | .unrepresentable => "R1"
  | .failLoudly => "R2"
  | .resetAndCatchUp => "R3"
  | .runbook => "R4"

def Role.wire : Role → String
  | .ontology => "ontology"
  | .instanceRole => "instance"
  | .guard => "guard"
  | .witness => "witness"
  | .independent => "independent"

/-- A point in the venn: representation × shape × root cause. -/
structure Cell where
  representation : Representation
  shape : Shape
  cause : RootCause
  deriving DecidableEq, Repr

/-- One law's place in the classification. `clause` is `""` for a law with no clause letter. -/
structure Attribution where
  number : Nat
  role : Role
  clause : String := ""
  cell : Option Cell := none
  rung : Option Rung := none
  note : String := ""
  deriving Repr

/-- A fact the transition relation does not model. Naming these is part of the claim: the
    classification is not total over the world. -/
structure Residue where
  name : String
  why : String
  witness : String
  deriving Repr

/-- **The crosswalk's explicit rows.** Every Progress-layer entry and every clause of a law in
    `coreProgress`, `stallRelevant` or `contextRelevant` must appear here; the register's check 11
    refuses the build otherwise. Rows default to `independent` when absent. -/
def attributions : List Attribution := [
  { number := 51, clause := "a", role := .ontology,
    note := "the shape vocabulary; the classification module is its machine form" },
  { number := 51, clause := "b", role := .ontology,
    note := "the cause vocabulary; selfReferentialGating and lossBlindRefusal are its causes of the cause" },
  { number := 51, clause := "c", role := .ontology,
    note := "the closed-calculus representation; silence is stuckness by the enabled tie" },
  { number := 51, clause := "d", role := .instanceRole, rung := some .failLoudly,
    cell := some { representation := .protocolWait, shape := .paced,
                   cause := .selfReferentialGating },
    note := "the pacing escape: a banked permit is the self-referential gate's smallest instance" },
  { number := 52, clause := "a", role := .guard, rung := some .unrepresentable,
    cell := some { representation := .protocolStall, shape := .void,
                   cause := .unsatisfiableRequirement },
    note := "the counterexample that reddens a denominator-shrinking fix" },
  { number := 52, clause := "b", role := .instanceRole, rung := some .unrepresentable,
    cell := some { representation := .protocolStall, shape := .void,
                   cause := .unsatisfiableRequirement } },
  { number := 53, clause := "a", role := .guard, rung := some .resetAndCatchUp,
    cell := some { representation := .protocolStall, shape := .terminal,
                   cause := .lossBlindRefusal },
    note := "the refusal that stays persistent until a restoring rule exists" },
  { number := 53, clause := "b", role := .instanceRole, rung := some .resetAndCatchUp,
    cell := some { representation := .protocolStall, shape := .terminal,
                   cause := .lossBlindRefusal } },
  { number := 53, clause := "c", role := .instanceRole, rung := some .resetAndCatchUp,
    cell := some { representation := .protocolStall, shape := .terminal,
                   cause := .recoveryBeforeAnchor } },
  { number := 54, clause := "a", role := .instanceRole, rung := some .failLoudly,
    cell := some { representation := .protocolWait, shape := .historic,
                   cause := .historicPredicate } },
  { number := 54, clause := "b", role := .instanceRole, rung := some .unrepresentable,
    cell := some { representation := .protocolStall, shape := .split,
                   cause := .divergentReaders } },
  { number := 58, role := .instanceRole, rung := some .failLoudly,
    cell := some { representation := .protocolWait, shape := .silent,
                   cause := .unsatisfiableRequirement } },
  { number := 61, clause := "a", role := .instanceRole, rung := some .failLoudly,
    cell := some { representation := .protocolWait, shape := .stuck,
                   cause := .lossBlindRefusal } },
  { number := 61, clause := "b", role := .guard, rung := some .failLoudly,
    cell := some { representation := .protocolStall, shape := .terminal,
                   cause := .lossBlindRefusal },
    note := "a deferred answer is written, so the fix cannot become a silent drop" },
  { number := 69, role := .instanceRole, rung := some .resetAndCatchUp,
    cell := some { representation := .protocolStall, shape := .drift,
                   cause := .selfReferentialGating } },
  { number := 71, role := .guard, rung := some .failLoudly,
    cell := some { representation := .protocolWait, shape := .stuck,
                   cause := .selfReferentialGating },
    note := "the catch-up window bounds what a joiner may wait on" },
  { number := 76, role := .instanceRole, rung := some .resetAndCatchUp,
    cell := some { representation := .protocolStall, shape := .terminal,
                   cause := .lossBlindRefusal } },
  { number := 83, role := .guard, rung := some .unrepresentable,
    cell := some { representation := .protocolStall, shape := .split,
                   cause := .unverifiedTrust },
    note := "derivable fringe is refused, so the trust boundary cannot be widened by a fix" },
  { number := 89, role := .instanceRole, rung := some .resetAndCatchUp,
    cell := some { representation := .protocolStall, shape := .void,
                   cause := .unsatisfiableRequirement } },
  { number := 94, role := .instanceRole, rung := some .resetAndCatchUp,
    cell := some { representation := .protocolStall, shape := .terminal,
                   cause := .recoveryBeforeAnchor } },
  { number := 104, role := .guard, rung := some .resetAndCatchUp,
    cell := some { representation := .protocolStall, shape := .terminal,
                   cause := .recoveryBeforeAnchor },
    note := "a boot verifies the certificate before it adopts it" },
  { number := 121, role := .instanceRole, rung := some .failLoudly,
    cell := some { representation := .protocolWait, shape := .historic,
                   cause := .historicPredicate } },
  { number := 123, role := .instanceRole, rung := some .failLoudly,
    cell := some { representation := .protocolWait, shape := .historic,
                   cause := .historicPredicate } },
  { number := 38, role := .ontology,
    note := "the closed-calculus silence: no step and no error" },
  { number := 20, role := .independent,
    note := "scheduler linearization; starvation is a residue, not a shape" }
]

/-- The laws every clause of which must be attributed. -/
def coreProgress : List Nat := [51, 52, 53, 54]

/-- Progress-family laws whose clauses the HAZOP study classifies. -/
def stallRelevant : List Nat := [58, 61, 69, 71, 76, 83, 89, 94, 104, 121, 123]

/-- Context laws that are explicitly considered, including those marked independent. -/
def contextRelevant : List Nat := [38, 20]

/-- The lookup the crosswalk uses; `none` means the entry is `independent` by default. -/
def attributionFor (number : Nat) (clause : String) : Option Attribution :=
  attributions.find? fun a => a.number == number && a.clause == clause

/-- The residues: facts that can stop a node and that the transition relation does not model. -/
def residues : List Residue := [
  { name := "scheduler starvation",
    why := "the relation fixes no schedule; Fair is named and unmodelled",
    witness := "Rchain.reduce_not_deterministic" },
  { name := "physical network partition",
    why := "delivery is a hypothesis, not an event the protocol controls",
    witness := "docs/src/formal/progress.md" },
  { name := "I/O and storage errors",
    why := "host facts outside the state relation",
    witness := "docs/src/node/storage.md" },
  { name := "wall-clock windows",
    why := "a timing hypothesis, not a transition",
    witness := "docs/src/formal/progress.md" },
  { name := "operator action",
    why := "the R4 runbook is outside the protocol",
    witness := "docs/src/spec/failure-hazop.md" },
  { name := "fleet version skew",
    why := "an external campaign fact, not a protocol state",
    witness := "docs/src/node/updating.md" }
]

/-- The vocabulary as a TSV: axis, constructor, wire name. -/
def vocabularyTsv : String :=
  let rows :=
    (Representation.closedSilence :: Representation.protocolWait :: Representation.protocolStall :: []).map
      (fun x => s!"representation\t{x.wire}") ++
    (Shape.void :: Shape.terminal :: Shape.drift :: Shape.split :: Shape.historic :: Shape.stuck ::
      Shape.silent :: Shape.persistent :: Shape.unrestorable :: Shape.paced :: []).map
      (fun x => s!"shape\t{x.wire}") ++
    (RootCause.unsatisfiableRequirement :: RootCause.selfReferentialGating ::
      RootCause.lossBlindRefusal :: RootCause.divergentReaders :: RootCause.historicPredicate ::
      RootCause.recoveryBeforeAnchor :: RootCause.prematureVerdict :: RootCause.unverifiedTrust ::
      RootCause.mixedVersion :: []).map
      (fun x => s!"root-cause\t{x.wire}") ++
    (Rung.unrepresentable :: Rung.failLoudly :: Rung.resetAndCatchUp :: Rung.runbook :: []).map
      (fun x => s!"rung\t{x.wire}") ++
    (Role.ontology :: Role.instanceRole :: Role.guard :: Role.witness :: Role.independent :: []).map
      (fun x => s!"role\t{x.wire}")
  String.intercalate "\n" ("axis\twire" :: rows)

/-! ## Law 51's proposition

The classification is not a new law number: it is Law 51's statement made machine-checkable. The
proposition conjoins the exact complement of the two polarities, the smallest wait (one step available
and not terminal), and the closed-calculus silence, and the falsifier exhibits the mutant in which the
polarities are *not* complements. None of these claims a step is ever taken. -/
def law51_statement : Prop :=
  (∀ (S : Rchain.System) (want : S.State → Prop),
      S.Persistent want ↔ S.Unrestorable (fun σ => ¬ want σ)) ∧
    Rchain.System.Enabled waitSystem false ∧
    waitSystem.Reach false true ∧
    (¬ waitSystem.Terminal waitGoal false) ∧
    (∀ (p : Par), takesStep p = false → silenceSystem.Stuck p)

/-- The mutant: a system whose two polarities disagree. -/
def law51_mutant : Prop :=
  ∃ (S : Rchain.System) (want : S.State → Prop),
    ¬ (S.Persistent want ↔ S.Unrestorable (fun σ => ¬ want σ))

theorem law51 : law51_statement := by
  refine ⟨?_, rfl, a_wait_is_one_step_from_the_goal, a_wait_is_not_terminal, ?_⟩
  · intro S want
    exact Rchain.System.persistent_iff_unrestorable_complement
  · intro p h
    exact a_silent_term_is_stuck p h

theorem law51_falsifier : ¬ law51_mutant := by
  rintro ⟨S, want, h⟩
  exact h (Rchain.System.persistent_iff_unrestorable_complement (S := S) (want := want))

end Classification
end Progress
end Rchain
