import Rchain.Sort
import Mathlib.Tactic.Ring
import Mathlib.Tactic.Linarith

/-!
# Law 87 — the exact decimal value algebra

The Rust `Decimal` (`models/src/decimal.rs`) is `coefficient × 10^-scale` with a signed,
arbitrary-precision coefficient and a scale in `0..=38`; the canonical form strips trailing zeros
and makes every zero `(0, 0)`. The comparator tie (`#390`) models only the **scaled payload**
`Expr.edecimal` (`coefficient × 10^(38 - scale)`), because that is what the sorter compares. This
module states the value algebra the payload stands for: canonicalisation, the injective and
order-preserving embedding into `Int` at scale 38, exact `+ - *`, and division that is exact or an
explicit error.

The model is deliberately over `Int`/`Nat` and **not** over `ℚ`: `scaledValue` is the exact value
in units of `10^-38`, which is the node's representation, and the arithmetic identities are integer
identities. `Int`'s `/` and `%` are Euclidean where Rust's `BigInt` truncate; the only uses here are
a divisibility test and an exact division, where they agree, and the corpus pins a negative case.

What is not modelled, and stays with the Rust refinement and its tests: the wire's minimal-byte
encoding and tag 35, serde ingress, reducer dispatch, the cost model, and the equivalence of the
Rust digit-by-digit division loop to the `divAt`/`divUpTo` exactness search (the corpus is the tie).
-/

namespace Rchain

/-- The value's representation: `coefficient × 10^-scale`. The Rust fields are private and
    canonical by construction; here canonicity is a predicate on every law. -/
structure Decimal where
  coefficient : Int
  scale : Nat
  deriving Repr, DecidableEq

namespace Decimal

/-- The largest number of fractional decimal digits. The same protocol constant as
    `models/src/decimal.rs`'s `MAX_SCALE` and `spec/Rchain/Par.lean`'s `edecimal` comment; changing it
    is a hard fork that must move this module, the Rust value, the sorter and the corpus together. -/
def MAX_SCALE : Nat := 38

/-- The Rust `DecimalError` variants the algebra can produce. The wire-only variants
    (`CoefficientEmpty`, `CoefficientNotMinimal`) are outside this model. -/
inductive DecimalError where
  | scaleOutOfRange (scale : Nat)
  | zeroWithScale (scale : Nat)
  | trailingZero (scale : Nat)
  | divisionByZero
  | inexactDivision
  deriving Repr, DecidableEq

/-- No factor of ten in the coefficient at a positive scale. The scale-zero case is always reduced:
    the canonical zero and an integer both live there. -/
def Reduced (d : Decimal) : Prop := d.scale = 0 ∨ d.coefficient % 10 ≠ 0

/-- The canonical form: within the scale bound and reduced. -/
def Canonical (d : Decimal) : Prop := d.scale ≤ MAX_SCALE ∧ Reduced d

/-- Canonicalisation: strip trailing zeros while the scale is positive; every zero is `(0, 0)`. This
    is the Rust `Decimal::new` loop without the scale check (which `new` applies afterwards). -/
def canonicalize (c : Int) : Nat → Decimal
  | 0 => ⟨c, 0⟩
  | s + 1 =>
      if c % 10 = 0 then canonicalize (c / 10) s else ⟨c, s + 1⟩

/-- The Rust `Decimal::new`: canonicalise first, then bound. A zero therefore canonicalises to
    `(0, 0)` even when the requested scale is above the bound. -/
def new (c : Int) (s : Nat) : Except DecimalError Decimal :=
  let d := canonicalize c s
  if d.scale ≤ MAX_SCALE then .ok d else .error (.scaleOutOfRange d.scale)

/-- The Rust `Decimal::from_parts`: the scale bound is checked **first**, then the pair must already
    be canonical. A noncanonical pair is classified as zero-with-scale or trailing-zero. -/
def fromParts (c : Int) (s : Nat) : Except DecimalError Decimal :=
  if s > MAX_SCALE then
    .error (.scaleOutOfRange s)
  else
    let d := canonicalize c s
    if d.coefficient = c ∧ d.scale = s then
      .ok d
    else if c = 0 then
      .error (.zeroWithScale s)
    else
      .error (.trailingZero s)

/-- The value in units of `10^-38`: the exact integer the sorter and `Expr.edecimal` carry. -/
def scaledValue (d : Decimal) : Int :=
  d.coefficient * 10 ^ (MAX_SCALE - d.scale)

/-- Exact numeric comparison, mirroring `Decimal::cmp_numeric` (align at the larger scale). -/
def cmpNumeric (a b : Decimal) : Ordering :=
  if a.scale = b.scale then
    _root_.cmp a.coefficient b.coefficient
  else if a.scale < b.scale then
    _root_.cmp (a.coefficient * 10 ^ (b.scale - a.scale)) b.coefficient
  else
    _root_.cmp a.coefficient (b.coefficient * 10 ^ (a.scale - b.scale))

/-- Exact addition: align at the larger scale, then canonicalise. -/
def add (a b : Decimal) : Decimal :=
  let s := max a.scale b.scale
  canonicalize
    (a.coefficient * 10 ^ (s - a.scale) + b.coefficient * 10 ^ (s - b.scale)) s

/-- Exact subtraction. -/
def sub (a b : Decimal) : Decimal :=
  let s := max a.scale b.scale
  canonicalize
    (a.coefficient * 10 ^ (s - a.scale) - b.coefficient * 10 ^ (s - b.scale)) s

/-- Exact multiplication: the scale is the sum, then canonicalise and bound. -/
def mul (a b : Decimal) : Except DecimalError Decimal :=
  let d := canonicalize (a.coefficient * b.coefficient) (a.scale + b.scale)
  if d.scale ≤ MAX_SCALE then .ok d else .error (.scaleOutOfRange d.scale)

/-- The quotient at a candidate scale, in the model's own scaled units: `a/b × 10^s` as an integer,
    only when it is exact. -/
def divAt (a b : Decimal) (s : Nat) : Option Int :=
  let den := b.scaledValue
  if den = 0 then
    none
  else
    let num := a.scaledValue * 10 ^ s
    if num % den = 0 then some (num / den) else none

/-- The first scale in `0..=k` at which the quotient is exact, as a canonical decimal. -/
def divUpTo (a b : Decimal) : Nat → Option Decimal
  | 0 =>
      match divAt a b 0 with
      | some c => some (canonicalize c 0)
      | none => none
  | k + 1 =>
      match divUpTo a b k with
      | some q => some q
      | none =>
          match divAt a b (k + 1) with
          | some c => some (canonicalize c (k + 1))
          | none => none

/-- Exact division or an explicit error. A zero divisor is `DivisionByZero`; no exact scale within
    `MAX_SCALE` is `InexactDivision`. -/
def div (a b : Decimal) : Except DecimalError Decimal :=
  if b.coefficient = 0 then
    .error .divisionByZero
  else
    match divUpTo a b MAX_SCALE with
    | some q => .ok q
    | none => .error .inexactDivision

/-- The `Expr.edecimal` payload the comparator reads. -/
def edecimalOf (d : Decimal) : Expr := .edecimal d.scaledValue

/-! ## Canonicalisation -/

theorem canonicalize_scale_le (c : Int) (s : Nat) :
    (canonicalize c s).scale ≤ s := by
  induction s generalizing c with
  | zero => simp [canonicalize]
  | succ s ih =>
      simp only [canonicalize]
      split
      · exact le_trans (ih (c / 10)) (Nat.le_succ s)
      · exact Nat.le_refl _

theorem canonicalize_reduced (c : Int) (s : Nat) :
    Reduced (canonicalize c s) := by
  induction s generalizing c with
  | zero => exact Or.inl rfl
  | succ s ih =>
      simp only [canonicalize]
      split
      · exact ih (c / 10)
      · rename_i h
        exact Or.inr h

theorem canonicalize_zero (s : Nat) : canonicalize 0 s = ⟨0, 0⟩ := by
  induction s with
  | zero => rfl
  | succ s ih => simp [canonicalize, ih]

theorem canonicalize_cross (c : Int) (s : Nat) :
    (canonicalize c s).coefficient * 10 ^ s
      = c * 10 ^ (canonicalize c s).scale := by
  induction s generalizing c with
  | zero => simp [canonicalize]
  | succ s ih =>
      simp only [canonicalize]
      split
      · rename_i h
        have hc : c = (c / 10) * 10 := by
          have h' := Int.ediv_add_emod c 10
          rw [h] at h'
          simpa [mul_comm] using h'.symm
        calc
          (canonicalize (c / 10) s).coefficient * 10 ^ (s + 1)
              = ((canonicalize (c / 10) s).coefficient * 10 ^ s) * 10 := by
                  rw [pow_succ]
                  ring
          _ = ((c / 10) * 10 ^ (canonicalize (c / 10) s).scale) * 10 := by
                  rw [ih (c / 10)]
          _ = c * 10 ^ (canonicalize (c / 10) s).scale := by
                  conv_rhs =>
                    enter [1]
                    rw [hc]
                  ring
      · rfl


/-! ## Integer and canonicalisation helpers -/

theorem ten_pow_pos (n : Nat) : (0 : Int) < 10 ^ n := by
  induction n with
  | zero => norm_num
  | succ n ih =>
      rw [pow_succ]
      exact mul_pos ih (by norm_num)

theorem ten_pow_add (m n : Nat) :
    (10 : Int) ^ (m + n) = 10 ^ m * 10 ^ n := by
  rw [pow_add]

theorem ten_pow_sub {m n : Nat} (h : m ≤ n) :
    (10 : Int) ^ n = 10 ^ m * 10 ^ (n - m) := by
  rw [← Nat.add_sub_of_le h, pow_add]
  rw [show m + (n - m) - m = n - m by omega]

theorem cmp_mul_pos {x y : Int} {k : Int} (hk : 0 < k) :
    _root_.cmp (x * k) (y * k) = _root_.cmp x y := by
  rcases lt_trichotomy x y with h | h | h
  · rw [(cmp_eq_lt_iff x y).mpr h, (cmp_eq_lt_iff (x * k) (y * k)).mpr (Int.mul_lt_mul_of_pos_right h hk)]
  · subst h
    simp
  · rw [(cmp_eq_gt_iff x y).mpr h, (cmp_eq_gt_iff (x * k) (y * k)).mpr (Int.mul_lt_mul_of_pos_right h hk)]

theorem canonicalize_idempotent (c : Int) (s : Nat) :
    canonicalize (canonicalize c s).coefficient (canonicalize c s).scale
      = canonicalize c s := by
  induction s generalizing c with
  | zero => simp [canonicalize]
  | succ s ih =>
      simp only [canonicalize]
      split
      · exact ih (c / 10)
      · rename_i h
        simp [canonicalize, h]

theorem canonicalize_of_not_dvd {c : Int} {s : Nat} (hs : 0 < s) (h : c % 10 ≠ 0) :
    canonicalize c s = ⟨c, s⟩ := by
  cases s with
  | zero => omega
  | succ k => simp [canonicalize, h]

theorem canonicalize_eq_self_iff {c : Int} {s : Nat} (hs : s ≤ MAX_SCALE) :
    canonicalize c s = ⟨c, s⟩ ↔ Reduced ⟨c, s⟩ := by
  constructor
  · intro h
    rw [← h]
    exact canonicalize_reduced c s
  · intro h
    cases s with
    | zero => rfl
    | succ k =>
        apply canonicalize_of_not_dvd (Nat.succ_pos k)
        exact h.resolve_left (Nat.succ_ne_zero k)

theorem canonicalize_eq_mk_iff (c : Int) (s : Nat) :
    canonicalize c s = ⟨c, s⟩ ↔
      (canonicalize c s).coefficient = c ∧ (canonicalize c s).scale = s := by
  constructor
  · intro h
    rw [h]
    simp
  · rintro ⟨h1, h2⟩
    cases h : canonicalize c s with
    | mk coeff scale => simp_all

theorem scaledValue_canonicalize {c : Int} {s : Nat} (hs : s ≤ MAX_SCALE) :
    scaledValue (canonicalize c s) = c * 10 ^ (MAX_SCALE - s) := by
  simp only [MAX_SCALE] at hs ⊢
  set q := canonicalize c s
  have hqle : q.scale ≤ s := canonicalize_scale_le c s
  have hcross := canonicalize_cross c s
  have hcancel : q.coefficient * 10 ^ (s - q.scale) = c := by
    have hpow : (10 : Int) ^ s = 10 ^ q.scale * 10 ^ (s - q.scale) := ten_pow_sub hqle
    rw [hpow] at hcross
    have h' : (q.coefficient * 10 ^ (s - q.scale)) * 10 ^ q.scale = c * 10 ^ q.scale := by
      calc
        (q.coefficient * 10 ^ (s - q.scale)) * 10 ^ q.scale
            = q.coefficient * (10 ^ q.scale * 10 ^ (s - q.scale)) := by ring
        _ = c * 10 ^ q.scale := hcross
    exact mul_right_cancel₀ (ne_of_gt (ten_pow_pos q.scale)) h'
  unfold scaledValue
  simp only [MAX_SCALE]
  have hexp : 38 - q.scale = (s - q.scale) + (38 - s) := by omega
  rw [hexp, pow_add, ← hcancel]
  ring

theorem new_ok_iff (c : Int) (s : Nat) (d : Decimal) :
    new c s = .ok d ↔ d = canonicalize c s ∧ (canonicalize c s).scale ≤ MAX_SCALE := by
  unfold new
  by_cases h : (canonicalize c s).scale ≤ MAX_SCALE
  · simp only [h, if_true, true_and]
    constructor
    · intro hh
      exact ⟨(Except.ok.inj hh).symm, trivial⟩
    · intro hh
      rw [hh.1]
  · simp [h]

theorem new_error_iff (c : Int) (s : Nat) (e : DecimalError) :
    new c s = .error e ↔
      (canonicalize c s).scale > MAX_SCALE ∧
        e = .scaleOutOfRange (canonicalize c s).scale := by
  unfold new
  by_cases h : (canonicalize c s).scale ≤ MAX_SCALE
  · simp [h]
  · have h' : (canonicalize c s).scale > MAX_SCALE := Nat.lt_of_not_le h
    simp [h, h', eq_comm]

theorem fromParts_ok_iff (c : Int) (s : Nat) (d : Decimal) :
    fromParts c s = .ok d ↔
      s ≤ MAX_SCALE ∧ canonicalize c s = ⟨c, s⟩ ∧ d = ⟨c, s⟩ := by
  unfold fromParts
  by_cases hs : s > MAX_SCALE
  · simp [hs]
  · have hsle : s ≤ MAX_SCALE := Nat.le_of_not_gt hs
    simp only [hs, if_false]
    by_cases hp : (canonicalize c s).coefficient = c ∧ (canonicalize c s).scale = s
    · have hpair : canonicalize c s = ⟨c, s⟩ := (canonicalize_eq_mk_iff c s).mpr hp
      simp [hp, hpair, hsle, eq_comm]
    · have hpair : canonicalize c s ≠ ⟨c, s⟩ :=
        fun h => hp ((canonicalize_eq_mk_iff c s).mp h)
      by_cases hc0 : c = 0
      · subst hc0
        rw [canonicalize_zero s]
        have hs0 : s ≠ 0 := by
          intro hs0
          apply hp
          rw [hs0]
          simp [canonicalize]
        have h0s : 0 ≠ s := fun h => hs0 h.symm
        simp [h0s]
      · simp [hp, hpair, hc0]

theorem fromParts_zero_with_scale {s : Nat} (hpos : 0 < s) (hs : s ≤ MAX_SCALE) :
    fromParts 0 s = .error (.zeroWithScale s) := by
  unfold fromParts
  rw [if_neg (not_lt.mpr hs), canonicalize_zero]
  simp [hpos.ne']
  omega

theorem fromParts_trailing_zero {c : Int} {s : Nat}
    (hpos : 0 < s) (hs : s ≤ MAX_SCALE) (hc : c ≠ 0) (h10 : c % 10 = 0) :
    fromParts c s = .error (.trailingZero s) := by
  unfold fromParts
  rw [if_neg (not_lt.mpr hs)]
  have hne : canonicalize c s ≠ ⟨c, s⟩ := by
    intro heq
    cases s with
    | zero => omega
    | succ k =>
        have hcan : canonicalize c (k + 1) = canonicalize (c / 10) k := by
          simp [canonicalize, h10]
        have hle : (canonicalize (c / 10) k).scale ≤ k := canonicalize_scale_le (c / 10) k
        have hscale : (canonicalize c (k + 1)).scale = k + 1 := by rw [heq]
        rw [hcan] at hscale
        omega
  have hp : ¬ ((canonicalize c s).coefficient = c ∧ (canonicalize c s).scale = s) :=
    fun hp => hne ((canonicalize_eq_mk_iff c s).mpr hp)
  simp [hp, hc]


/-! ## 87b — the scaled-value embedding -/

theorem ten_pow_dvd_mul (x : Int) {k : Nat} (hk : 0 < k) :
    (10 : Int) ∣ x * 10 ^ k := by
  cases k with
  | zero => omega
  | succ m =>
      refine ⟨x * 10 ^ m, ?_⟩
      rw [pow_succ]
      ring

theorem scaledValue_injective_le {a b : Decimal} (ha : Canonical a) (hb : Canonical b)
    (hle : a.scale ≤ b.scale) (h : scaledValue a = scaledValue b) : a = b := by
  obtain ⟨haS, _⟩ := ha
  obtain ⟨hbS, hbR⟩ := hb
  simp only [MAX_SCALE] at haS hbS
  unfold scaledValue at h
  simp only [MAX_SCALE] at h
  have hcancel : a.coefficient * 10 ^ (b.scale - a.scale) = b.coefficient := by
    have hpow : (10 : Int) ^ (38 - a.scale) = 10 ^ (38 - b.scale) * 10 ^ (b.scale - a.scale) := by
      rw [← ten_pow_add]
      congr 1
      omega
    rw [hpow] at h
    have h' : (a.coefficient * 10 ^ (b.scale - a.scale)) * 10 ^ (38 - b.scale)
        = b.coefficient * 10 ^ (38 - b.scale) := by
      calc
        (a.coefficient * 10 ^ (b.scale - a.scale)) * 10 ^ (38 - b.scale)
            = a.coefficient * (10 ^ (38 - b.scale) * 10 ^ (b.scale - a.scale)) := by ring
        _ = b.coefficient * 10 ^ (38 - b.scale) := h
    exact mul_right_cancel₀ (ne_of_gt (ten_pow_pos (38 - b.scale))) h'
  by_cases hEq : a.scale = b.scale
  · have hk0 : b.scale - a.scale = 0 := by omega
    rw [hk0] at hcancel
    simp at hcancel
    cases a with
    | mk ca sa =>
        cases b with
        | mk cb sb => simp_all
  · have hlt : a.scale < b.scale := lt_of_le_of_ne hle hEq
    have hk : 0 < b.scale - a.scale := by omega
    have hdvd : (10 : Int) ∣ b.coefficient := by
      rw [← hcancel]
      exact ten_pow_dvd_mul a.coefficient hk
    have hmod : b.coefficient % 10 = 0 := Int.emod_eq_zero_of_dvd hdvd
    have hbpos : 0 < b.scale := by omega
    rcases hbR with hb0 | hbnz
    · omega
    · exact (hbnz hmod).elim

theorem scaledValue_injective {a b : Decimal} (ha : Canonical a) (hb : Canonical b)
    (h : scaledValue a = scaledValue b) : a = b := by
  rcases le_total a.scale b.scale with hle | hle
  · exact scaledValue_injective_le ha hb hle h
  · exact (scaledValue_injective_le hb ha hle h.symm).symm

theorem canonical_zero_unique {d : Decimal} (hd : Canonical d) (h : scaledValue d = 0) :
    d = ⟨0, 0⟩ := by
  obtain ⟨_, hR⟩ := hd
  unfold scaledValue at h
  rcases mul_eq_zero.mp h with hc | hp
  · have hs : d.scale = 0 := by
      rcases hR with hs | hz
      · exact hs
      · exfalso
        rw [hc] at hz
        exact hz (by norm_num)
    cases d with
    | mk c s => simp_all
  · exact (ne_of_gt (ten_pow_pos (MAX_SCALE - d.scale)) hp).elim

theorem cmpNumeric_eq_scaled {a b : Decimal} (ha : Canonical a) (hb : Canonical b) :
    cmpNumeric a b = _root_.cmp (scaledValue a) (scaledValue b) := by
  obtain ⟨haS, _⟩ := ha
  obtain ⟨hbS, _⟩ := hb
  simp only [MAX_SCALE] at haS hbS
  unfold cmpNumeric scaledValue
  simp only [MAX_SCALE]
  by_cases hEq : a.scale = b.scale
  · simp only [hEq, if_true]
    have hk : 0 < (10 : Int) ^ (38 - b.scale) := ten_pow_pos _
    rw [← cmp_mul_pos (x := a.coefficient) (y := b.coefficient) (k := 10 ^ (38 - b.scale)) hk]
  · by_cases hlt : a.scale < b.scale
    · simp only [hEq, hlt, if_false, if_true]
      have hk : 0 < (10 : Int) ^ (38 - b.scale) := ten_pow_pos _
      rw [← cmp_mul_pos (x := a.coefficient * 10 ^ (b.scale - a.scale))
        (y := b.coefficient) (k := 10 ^ (38 - b.scale)) hk]
      have harg : (a.coefficient * 10 ^ (b.scale - a.scale)) * 10 ^ (38 - b.scale)
          = a.coefficient * 10 ^ (38 - a.scale) := by
        calc
          (a.coefficient * 10 ^ (b.scale - a.scale)) * 10 ^ (38 - b.scale)
              = a.coefficient * (10 ^ (b.scale - a.scale) * 10 ^ (38 - b.scale)) := by ring
          _ = a.coefficient * 10 ^ ((b.scale - a.scale) + (38 - b.scale)) := by rw [pow_add]
          _ = a.coefficient * 10 ^ (38 - a.scale) := by
                have hexp : (b.scale - a.scale) + (38 - b.scale) = 38 - a.scale := by omega
                rw [hexp]
      rw [harg]
    · have hk : 0 < (10 : Int) ^ (38 - a.scale) := ten_pow_pos _
      simp only [hEq, hlt, if_false]
      rw [← cmp_mul_pos (x := a.coefficient)
        (y := b.coefficient * 10 ^ (a.scale - b.scale)) (k := 10 ^ (38 - a.scale)) hk]
      have harg : (b.coefficient * 10 ^ (a.scale - b.scale)) * 10 ^ (38 - a.scale)
          = b.coefficient * 10 ^ (38 - b.scale) := by
        calc
          (b.coefficient * 10 ^ (a.scale - b.scale)) * 10 ^ (38 - a.scale)
              = b.coefficient * (10 ^ (a.scale - b.scale) * 10 ^ (38 - a.scale)) := by ring
          _ = b.coefficient * 10 ^ ((a.scale - b.scale) + (38 - a.scale)) := by rw [pow_add]
          _ = b.coefficient * 10 ^ (38 - b.scale) := by
                have hexp : (a.scale - b.scale) + (38 - a.scale) = 38 - b.scale := by omega
                rw [hexp]
      rw [harg]

theorem cmpNumeric_eq_iff {a b : Decimal} (ha : Canonical a) (hb : Canonical b) :
    cmpNumeric a b = .eq ↔ a = b := by
  rw [cmpNumeric_eq_scaled ha hb, cmp_eq_eq_iff (scaledValue a) (scaledValue b)]
  exact ⟨fun h => scaledValue_injective ha hb h, fun h => by rw [h]⟩

theorem cmpNumeric_swap {a b : Decimal} (ha : Canonical a) (hb : Canonical b) :
    cmpNumeric b a = (cmpNumeric a b).swap := by
  rw [cmpNumeric_eq_scaled hb ha, cmpNumeric_eq_scaled ha hb]
  exact (cmp_swap (scaledValue a) (scaledValue b)).symm

theorem cmpNumeric_lt_trans {a b c : Decimal}
    (ha : Canonical a) (hb : Canonical b) (hc : Canonical c)
    (h1 : cmpNumeric a b = .lt) (h2 : cmpNumeric b c = .lt) :
    cmpNumeric a c = .lt := by
  rw [cmpNumeric_eq_scaled ha hb] at h1
  rw [cmpNumeric_eq_scaled hb hc] at h2
  rw [cmpNumeric_eq_scaled ha hc]
  exact (Rchain.Comparator.linearOrderComparator Int).lt_trans h1 h2


/-! ## 87c — exact addition, subtraction and multiplication -/

theorem add_canonical {a b : Decimal} (ha : Canonical a) (hb : Canonical b) :
    Canonical (add a b) := by
  obtain ⟨haS, _⟩ := ha
  obtain ⟨hbS, _⟩ := hb
  unfold add Canonical
  exact ⟨le_trans (canonicalize_scale_le _ _) (max_le haS hbS), canonicalize_reduced _ _⟩

theorem sub_canonical {a b : Decimal} (ha : Canonical a) (hb : Canonical b) :
    Canonical (sub a b) := by
  obtain ⟨haS, _⟩ := ha
  obtain ⟨hbS, _⟩ := hb
  unfold sub Canonical
  exact ⟨le_trans (canonicalize_scale_le _ _) (max_le haS hbS), canonicalize_reduced _ _⟩

theorem scaledValue_add {a b : Decimal} (ha : Canonical a) (hb : Canonical b) :
    scaledValue (add a b) = scaledValue a + scaledValue b := by
  obtain ⟨haS, _⟩ := ha
  obtain ⟨hbS, _⟩ := hb
  simp only [MAX_SCALE] at haS hbS
  set s := max a.scale b.scale
  have hs : s ≤ 38 := by dsimp [s]; exact max_le haS hbS
  have hsa : a.scale ≤ s := by dsimp [s]; exact le_max_left _ _
  have hsb : b.scale ≤ s := by dsimp [s]; exact le_max_right _ _
  change scaledValue
      (canonicalize (a.coefficient * 10 ^ (s - a.scale) + b.coefficient * 10 ^ (s - b.scale)) s)
    = scaledValue a + scaledValue b
  rw [scaledValue_canonicalize (by simpa [MAX_SCALE] using hs)]
  unfold scaledValue
  simp only [MAX_SCALE]
  have ha' : (10 : Int) ^ (38 - a.scale) = 10 ^ (s - a.scale) * 10 ^ (38 - s) := by
    rw [show 38 - a.scale = (s - a.scale) + (38 - s) by omega, pow_add]
  have hb' : (10 : Int) ^ (38 - b.scale) = 10 ^ (s - b.scale) * 10 ^ (38 - s) := by
    rw [show 38 - b.scale = (s - b.scale) + (38 - s) by omega, pow_add]
  rw [ha', hb']
  dsimp [s]
  ring

theorem scaledValue_sub {a b : Decimal} (ha : Canonical a) (hb : Canonical b) :
    scaledValue (sub a b) = scaledValue a - scaledValue b := by
  obtain ⟨haS, _⟩ := ha
  obtain ⟨hbS, _⟩ := hb
  simp only [MAX_SCALE] at haS hbS
  set s := max a.scale b.scale
  have hs : s ≤ 38 := by dsimp [s]; exact max_le haS hbS
  have hsa : a.scale ≤ s := by dsimp [s]; exact le_max_left _ _
  have hsb : b.scale ≤ s := by dsimp [s]; exact le_max_right _ _
  change scaledValue
      (canonicalize (a.coefficient * 10 ^ (s - a.scale) - b.coefficient * 10 ^ (s - b.scale)) s)
    = scaledValue a - scaledValue b
  rw [scaledValue_canonicalize (by simpa [MAX_SCALE] using hs)]
  unfold scaledValue
  simp only [MAX_SCALE]
  have ha' : (10 : Int) ^ (38 - a.scale) = 10 ^ (s - a.scale) * 10 ^ (38 - s) := by
    rw [show 38 - a.scale = (s - a.scale) + (38 - s) by omega, pow_add]
  have hb' : (10 : Int) ^ (38 - b.scale) = 10 ^ (s - b.scale) * 10 ^ (38 - s) := by
    rw [show 38 - b.scale = (s - b.scale) + (38 - s) by omega, pow_add]
  rw [ha', hb']
  dsimp [s]
  ring

theorem add_new {a b : Decimal} (ha : Canonical a) (hb : Canonical b) :
    new (a.coefficient * 10 ^ (max a.scale b.scale - a.scale)
        + b.coefficient * 10 ^ (max a.scale b.scale - b.scale)) (max a.scale b.scale)
      = .ok (add a b) := by
  obtain ⟨haS, _⟩ := ha
  obtain ⟨hbS, _⟩ := hb
  have hs : max a.scale b.scale ≤ MAX_SCALE := max_le haS hbS
  unfold new add
  rw [if_pos (le_trans (canonicalize_scale_le _ _) hs)]

theorem sub_new {a b : Decimal} (ha : Canonical a) (hb : Canonical b) :
    new (a.coefficient * 10 ^ (max a.scale b.scale - a.scale)
        - b.coefficient * 10 ^ (max a.scale b.scale - b.scale)) (max a.scale b.scale)
      = .ok (sub a b) := by
  obtain ⟨haS, _⟩ := ha
  obtain ⟨hbS, _⟩ := hb
  have hs : max a.scale b.scale ≤ MAX_SCALE := max_le haS hbS
  unfold new sub
  rw [if_pos (le_trans (canonicalize_scale_le _ _) hs)]

theorem add_comm {a b : Decimal} (ha : Canonical a) (hb : Canonical b) :
    add a b = add b a := by
  apply scaledValue_injective (add_canonical ha hb) (add_canonical hb ha)
  rw [scaledValue_add ha hb, scaledValue_add hb ha, Int.add_comm]

theorem add_sub_cancel {a b : Decimal} (ha : Canonical a) (hb : Canonical b) :
    sub (add a b) b = a := by
  apply scaledValue_injective (sub_canonical (add_canonical ha hb) hb) ha
  rw [scaledValue_sub (add_canonical ha hb) hb, scaledValue_add ha hb]
  ring

theorem mul_ok_iff (a b : Decimal) (d : Decimal) :
    mul a b = .ok d ↔
      d = canonicalize (a.coefficient * b.coefficient) (a.scale + b.scale) ∧
        (canonicalize (a.coefficient * b.coefficient) (a.scale + b.scale)).scale ≤ MAX_SCALE := by
  unfold mul
  by_cases h : (canonicalize (a.coefficient * b.coefficient) (a.scale + b.scale)).scale
      ≤ MAX_SCALE
  · simp only [h, if_true, true_and]
    constructor
    · intro hh
      exact ⟨(Except.ok.inj hh).symm, trivial⟩
    · intro hh
      rw [hh.1]
  · simp [h]

theorem mul_error_iff (a b : Decimal) (e : DecimalError) :
    mul a b = .error e ↔
      (canonicalize (a.coefficient * b.coefficient) (a.scale + b.scale)).scale > MAX_SCALE ∧
        e = .scaleOutOfRange
          (canonicalize (a.coefficient * b.coefficient) (a.scale + b.scale)).scale := by
  unfold mul
  by_cases h : (canonicalize (a.coefficient * b.coefficient) (a.scale + b.scale)).scale
      ≤ MAX_SCALE
  · simp [h]
  · have h' : (canonicalize (a.coefficient * b.coefficient) (a.scale + b.scale)).scale
        > MAX_SCALE := Nat.lt_of_not_le h
    simp [h, h', eq_comm]

theorem scaledValue_mul {a b q : Decimal} (ha : Canonical a) (hb : Canonical b)
    (h : mul a b = .ok q) :
    scaledValue q * 10 ^ MAX_SCALE = scaledValue a * scaledValue b := by
  obtain ⟨haS, _⟩ := ha
  obtain ⟨hbS, _⟩ := hb
  simp only [MAX_SCALE] at haS hbS
  simp only [MAX_SCALE]
  rw [mul_ok_iff] at h
  obtain ⟨rfl, hq⟩ := h
  set s := a.scale + b.scale
  set C := a.coefficient * b.coefficient
  have hs : s ≤ 76 := by dsimp [s]; omega
  have hq38 : (canonicalize C s).scale ≤ 38 := by simpa [MAX_SCALE] using hq
  have hle : (canonicalize C s).scale ≤ s := canonicalize_scale_le C s
  have hcross := canonicalize_cross C s
  have hcancel : C = (canonicalize C s).coefficient * 10 ^ (s - (canonicalize C s).scale) := by
    have hpow : (10 : Int) ^ s =
        10 ^ (canonicalize C s).scale * 10 ^ (s - (canonicalize C s).scale) := ten_pow_sub hle
    rw [hpow] at hcross
    have h' : ((canonicalize C s).coefficient * 10 ^ (s - (canonicalize C s).scale))
        * 10 ^ (canonicalize C s).scale = C * 10 ^ (canonicalize C s).scale := by
      calc
        ((canonicalize C s).coefficient * 10 ^ (s - (canonicalize C s).scale))
            * 10 ^ (canonicalize C s).scale
            = (canonicalize C s).coefficient
                * (10 ^ (canonicalize C s).scale * 10 ^ (s - (canonicalize C s).scale)) := by ring
        _ = C * 10 ^ (canonicalize C s).scale := hcross
    exact (mul_right_cancel₀ (ne_of_gt (ten_pow_pos (canonicalize C s).scale)) h').symm
  have hqscaled : scaledValue (canonicalize C s) * 10 ^ 38 =
      (canonicalize C s).coefficient * 10 ^ (76 - (canonicalize C s).scale) := by
    unfold scaledValue
    simp only [MAX_SCALE]
    rw [mul_assoc, ← pow_add]
    rw [show 38 - (canonicalize C s).scale + 38 = 76 - (canonicalize C s).scale by omega]
  have hC : C * 10 ^ (76 - s) =
      (canonicalize C s).coefficient * 10 ^ (76 - (canonicalize C s).scale) := by
    conv_lhs =>
      enter [1]
      rw [hcancel]
    rw [mul_assoc, ← pow_add]
    rw [show (s - (canonicalize C s).scale) + (76 - s)
        = 76 - (canonicalize C s).scale by omega]
  rw [hqscaled, ← hC]
  unfold scaledValue
  simp only [MAX_SCALE]
  dsimp [C, s]
  rw [show 76 - (a.scale + b.scale) = (38 - a.scale) + (38 - b.scale) by omega, pow_add]
  ring


/-! ## 87d — division is exact or an explicit error -/

theorem div_eq_of_mul_eq {num den q : Int} (hden : den ≠ 0)
    (h : q * den = num) (hmod : num % den = 0) : num / den = q := by
  have h' := Int.ediv_add_emod num den
  rw [hmod, add_zero] at h'
  have h'' : den * (num / den) = den * q := by
    rw [h', mul_comm den q]
    exact h.symm
  exact mul_left_cancel₀ hden h''

theorem divAt_spec (a b : Decimal) (s : Nat) (q : Int) :
    divAt a b s = some q ↔
      b.scaledValue ≠ 0 ∧ q * b.scaledValue = a.scaledValue * 10 ^ s := by
  unfold divAt
  by_cases hden : b.scaledValue = 0
  · simp [hden]
  · simp only [hden, if_false]
    let num := a.scaledValue * 10 ^ s
    change (if num % b.scaledValue = 0 then some (num / b.scaledValue) else none) = some q ↔
      b.scaledValue ≠ 0 ∧ q * b.scaledValue = a.scaledValue * 10 ^ s
    by_cases hmod : num % b.scaledValue = 0
    · rw [if_pos hmod]
      constructor
      · intro hq
        injection hq with hq
        constructor
        · exact hden
        · have h' := Int.ediv_add_emod num b.scaledValue
          rw [hmod, add_zero] at h'
          rw [← hq, mul_comm]
          exact h'
      · rintro ⟨_, hq⟩
        exact congrArg some (div_eq_of_mul_eq hden hq hmod)
    · rw [if_neg hmod]
      constructor
      · intro h
        cases h
      · rintro ⟨_, hq⟩
        exfalso
        apply hmod
        change (a.scaledValue * 10 ^ s) % b.scaledValue = 0
        rw [← hq]
        exact Int.emod_eq_zero_of_dvd ⟨q, by rw [mul_comm]⟩

theorem divUpTo_some_spec (a b : Decimal) (k : Nat) {q : Decimal}
    (h : divUpTo a b k = some q) :
    ∃ s, s ≤ k ∧ ∃ c, divAt a b s = some c ∧ q = canonicalize c s := by
  induction k generalizing q with
  | zero =>
      unfold divUpTo at h
      split at h
      · rename_i c hc
        injection h with hq
        exact ⟨0, le_rfl, c, hc, hq.symm⟩
      · contradiction
  | succ k ih =>
      unfold divUpTo at h
      split at h
      · rename_i q0 hq0
        injection h with hq
        obtain ⟨s, hs, c, hc, hqc⟩ := ih hq0
        exact ⟨s, Nat.le_succ_of_le hs, c, hc, hq.symm.trans hqc⟩
      · rename_i _hn
        split at h
        · rename_i c hc
          injection h with hq
          exact ⟨k + 1, le_rfl, c, hc, hq.symm⟩
        · contradiction

theorem divUpTo_none_iff (a b : Decimal) (k : Nat) :
    divUpTo a b k = none ↔ ∀ s, s ≤ k → divAt a b s = none := by
  induction k with
  | zero =>
      unfold divUpTo
      split <;> simp_all
  | succ k ih =>
      unfold divUpTo
      split
      · rename_i q hq
        constructor
        · intro h
          exact absurd h (by simp)
        · intro hall
          obtain ⟨s, hs, c, hc, _⟩ := divUpTo_some_spec a b k hq
          have hnone := hall s (Nat.le_succ_of_le hs)
          rw [hnone] at hc
          exact absurd hc (by simp)
      · rename_i hn
        split
        · rename_i c hc
          constructor
          · intro h
            exact absurd h (by simp)
          · intro hall
            have hnone := hall (k + 1) le_rfl
            rw [hc] at hnone
            exact absurd hnone (by simp)
        · rename_i hn2
          constructor
          · intro _ s hs
            by_cases hsk : s ≤ k
            · exact (ih.mp hn) s hsk
            · have hsk' : s = k + 1 := by omega
              rw [hsk']
              exact hn2
          · intro _hall
            rfl

theorem div_zero (a : Decimal) (s : Nat) :
    div a ⟨0, s⟩ = .error .divisionByZero := by
  unfold div
  simp


theorem scaledValue_ne_zero {d : Decimal} (h : d.coefficient ≠ 0) :
    d.scaledValue ≠ 0 := by
  unfold scaledValue
  exact mul_ne_zero h (ne_of_gt (ten_pow_pos _))

theorem divExact_divAt {a b q : Decimal} (hq : Canonical q)
    (hb0 : b.coefficient ≠ 0)
    (h : q.scaledValue * b.scaledValue = a.scaledValue * 10 ^ MAX_SCALE) :
    divAt a b q.scale = some q.coefficient := by
  obtain ⟨hqS, _⟩ := hq
  simp only [MAX_SCALE] at hqS h ⊢
  have hk : (10 : Int) ^ (38 - q.scale) ≠ 0 := ne_of_gt (ten_pow_pos _)
  have h' : (q.coefficient * b.scaledValue) * 10 ^ (38 - q.scale)
      = (a.scaledValue * 10 ^ q.scale) * 10 ^ (38 - q.scale) := by
    calc
      (q.coefficient * b.scaledValue) * 10 ^ (38 - q.scale)
          = (q.coefficient * 10 ^ (38 - q.scale)) * b.scaledValue := by ring
      _ = q.scaledValue * b.scaledValue := rfl
      _ = a.scaledValue * 10 ^ 38 := h
      _ = a.scaledValue * (10 ^ q.scale * 10 ^ (38 - q.scale)) := by
            rw [show (10 : Int) ^ 38 = 10 ^ q.scale * 10 ^ (38 - q.scale) by
              rw [← ten_pow_add]
              rw [show q.scale + (38 - q.scale) = 38 by omega]]
      _ = (a.scaledValue * 10 ^ q.scale) * 10 ^ (38 - q.scale) := by ring
  exact (divAt_spec a b q.scale q.coefficient).mpr
    ⟨scaledValue_ne_zero hb0, mul_right_cancel₀ hk h'⟩

theorem div_ok_spec {a b q : Decimal} (hb0 : b.coefficient ≠ 0)
    (h : div a b = .ok q) :
    Canonical q ∧ q.scaledValue * b.scaledValue = a.scaledValue * 10 ^ MAX_SCALE := by
  simp only [MAX_SCALE] at h ⊢
  unfold div at h
  rw [if_neg hb0] at h
  split at h
  · rename_i q0 hq0
    injection h with hq
    obtain ⟨s, hs, c, hc, hqcanon⟩ := divUpTo_some_spec a b 38 hq0
    have hs38 : s ≤ 38 := hs
    have hqeq : q = canonicalize c s := hq.symm.trans hqcanon
    have hspec := (divAt_spec a b s c).mp hc
    obtain ⟨_, hmul⟩ := hspec
    constructor
    · rw [hqeq]
      exact ⟨le_trans (canonicalize_scale_le c s) hs38, canonicalize_reduced c s⟩
    · rw [hqeq, scaledValue_canonicalize hs38]
      calc
        (c * 10 ^ (38 - s)) * b.scaledValue
            = (c * b.scaledValue) * 10 ^ (38 - s) := by ring
        _ = (a.scaledValue * 10 ^ s) * 10 ^ (38 - s) := by rw [hmul]
        _ = a.scaledValue * (10 ^ s * 10 ^ (38 - s)) := by ring
        _ = a.scaledValue * 10 ^ 38 := by
              rw [show (10 : Int) ^ 38 = 10 ^ s * 10 ^ (38 - s) by
                rw [← ten_pow_add]
                rw [show s + (38 - s) = 38 by omega]]
  · cases h

theorem div_error_iff {a b : Decimal} (hb0 : b.coefficient ≠ 0) :
    div a b = .error .inexactDivision ↔
      ∀ q, Canonical q →
        q.scaledValue * b.scaledValue ≠ a.scaledValue * 10 ^ MAX_SCALE := by
  simp only [MAX_SCALE] at hb0 ⊢
  constructor
  · intro h q hqcanon hdiv
    have hqS : q.scale ≤ 38 := hqcanon.1
    unfold div at h
    rw [if_neg hb0] at h
    split at h
    · cases h
    · rename_i hn
      have hnone := (divUpTo_none_iff a b 38).mp hn
      have hsome := divExact_divAt hqcanon hb0 hdiv
      have hle : q.scale ≤ 38 := hqS
      have hfalse := hnone q.scale hle
      rw [hsome] at hfalse
      cases hfalse
  · intro hno
    unfold div
    rw [if_neg hb0]
    split
    · rename_i q0 hq0
      exfalso
      have hok : div a b = .ok q0 := by
        unfold div
        rw [if_neg hb0, hq0]
      obtain ⟨hqcanon, hdiv⟩ := div_ok_spec hb0 hok
      exact hno q0 hqcanon hdiv
    · rfl


/-! ## 87e — the `Expr.edecimal` payload bridge -/

theorem cmpExpr_edecimalOf (a b : Decimal) :
    cmpExpr (edecimalOf a) (edecimalOf b) =
      _root_.cmp a.scaledValue b.scaledValue := by
  simp [edecimalOf]

theorem edecimalOf_eq_iff {a b : Decimal} (ha : Canonical a) (hb : Canonical b) :
    edecimalOf a = edecimalOf b ↔ a = b := by
  constructor
  · intro h
    have h' : a.scaledValue = b.scaledValue := by
      simpa [edecimalOf] using h
    exact scaledValue_injective ha hb h'
  · intro h
    rw [h]

theorem cmpNumeric_eq_cmpExpr_edecimalOf {a b : Decimal} (ha : Canonical a) (hb : Canonical b) :
    cmpNumeric a b = cmpExpr (edecimalOf a) (edecimalOf b) := by
  rw [cmpNumeric_eq_scaled ha hb, cmpExpr_edecimalOf]

end Decimal
end Rchain
