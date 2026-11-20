import Mathlib.Data.Finset.Basic
import Mathlib.Tactic

set_option autoImplicit false

/-!
# Law 118 — a build opens only a store format it can read

The data directory carries a `store-format` marker. The Rust `decide_store_format`
(`casper/src/storage.rs`) maps the marker and this build's version to one of five outcomes, and
`probe_store_format` refuses a newer or unreadable marker **before any store is opened**
(`node/src/runtime/node_runtime.rs`). The decision is total; the only unsafe state is opening a
store whose layout this build does not know, which is how a downgrade silently becomes an advance.

The trace `FormatTrace` records the marker, the build version, the decision and whether a durable
stamp followed. The evaluator recomputes `format_ok` and `stamp_ok` from those coordinates.
-/

namespace Rchain
namespace Casper
namespace StoreFormat

/-- The marker as read from disk. An absent marker is not evidence of an old store; an unreadable
    one is refused rather than guessed. -/
inductive Marker where
  | absent
  | version (n : Nat)
  | unreadable (raw : String)
  deriving DecidableEq, Repr

/-- The five outcomes of the store-format gate. -/
inductive Decision where
  | stamp (previous : Option Nat)
  | accept
  | advance (previous : Nat)
  | refuseNewer (onDisk : Nat)
  | refuseUnreadable (raw : String)
  deriving DecidableEq, Repr

/-- The total decision. -/
def decide (m : Marker) (build : Nat) : Decision :=
  match m with
  | .absent => .stamp none
  | .unreadable raw => .refuseUnreadable raw
  | .version n =>
      if n > build then .refuseNewer n
      else if n = build then .accept
      else .advance n

/-- Whether the decision opens the store. -/
def opens : Decision → Bool
  | .stamp _ => true
  | .accept => true
  | .advance _ => true
  | .refuseNewer _ => false
  | .refuseUnreadable _ => false

/-- **The safe predicate**: each decision matches the marker, and a newer marker is refused. -/
def storeSafe (build : Nat) (m : Marker) (d : Decision) : Prop :=
  match m, d with
  | .absent, .stamp none => True
  | .version n, .accept => n = build
  | .version n, .advance previous => previous = n ∧ n < build
  | .version n, .refuseNewer onDisk => onDisk = n ∧ n > build
  | _, .refuseUnreadable _ => True
  | _, _ => False

/-- **A newer store is never opened.** -/
theorem a_newer_store_is_never_opened (build n : Nat) (h : n > build) :
    opens (decide (.version n) build) = false := by
  simp [decide, opens, h]

/-- The decision is safe for every marker. -/
theorem decide_is_safe (build : Nat) (m : Marker) : storeSafe build m (decide m build) := by
  cases m with
  | absent => simp [storeSafe, decide]
  | unreadable raw => simp [storeSafe, decide]
  | version n =>
      simp only [decide]
      split
      · rename_i hgt
        simp [storeSafe, hgt]
      · split
        · rename_i hn
          simp [storeSafe, hn]
        · rename_i hnot_gt hnot_eq
          simp [storeSafe, hnot_eq]
          omega

/-- **Totality**: every marker has exactly one decision. -/
theorem store_decision_is_total (m : Marker) (build : Nat) :
    ∃! d : Decision, decide m build = d := by
  refine ⟨decide m build, rfl, ?_⟩
  intro d hd
  exact hd.symm

/-- The naive decision a build without the gate would make: open anything present. -/
def openAny : Marker → Decision
  | .absent => .stamp none
  | .version _ => .accept
  | .unreadable _ => .accept

/-- **The unsafe witness**: the naive decision accepts a marker from a newer build. -/
theorem opening_a_newer_store_is_the_unsafe_witness (build n : Nat) (h : n > build) :
    ¬ storeSafe build (.version n) (openAny (.version n)) := by
  intro hsafe
  simp [storeSafe, openAny] at hsafe
  omega

/-- The law's proposition. -/
def law118_statement : Prop :=
  ∀ (build : Nat) (m : Marker),
    storeSafe build m (decide m build) ∧
      (∀ n, m = .version n → n > build → opens (decide m build) = false)

theorem law118 : law118_statement := by
  intro build m
  refine ⟨decide_is_safe build m, ?_⟩
  intro n hm hgt
  subst hm
  exact a_newer_store_is_never_opened build n hgt

/-- The mutant: a build that opens a newer store. -/
def law118_mutant : Prop :=
  ∃ (build n : Nat), n > build ∧ opens (openAny (.version n)) = true

theorem law118_falsifier : law118_mutant := by
  exact ⟨4, 5, by decide, rfl⟩

end StoreFormat
end Casper
end Rchain
