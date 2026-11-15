import Mathlib.Tactic

/-!
# Law 110 — a served history range is accounted before it leaves

**The change this law covers.** History below a checkpoint is a **service**. An archive holds the
bodies and is paid for serving them; the consumer in scope is the bodies-optional observer that does
not backfill. The node-side meter (`block-storage/src/history_meter.rs`) debits a range's bytes
before a body leaves, refuses an account short of the range, and refuses a replayed invoice or
nonce.

**What this law says.** `serve` returns a new account only when the credit pays for the bytes. A
served range therefore implies the range was paid for, the debit is exact (served credit plus the
range is the credit before), and an unpayable range is refused with the account unchanged.

**What it does not claim.** The meter enforces the operator's service arrangement; it does not
price the archive or decide the fee. The direct REV service fee is the decision of record (2026-11-15,
owner: Patrick; no consensus change), and the settlement of a failed challenge is Law 111. Neither is
a consensus rule.

**Why it is not vacuous.** Remove the credit check from `serve` and `an_unpaid_range_is_refused`
fails; debit a different amount and `a_served_range_is_accounted` fails. The Rust witness is the
meter's own tests, including the no-credit refusal leaving the account untouched.
-/

namespace Rchain

namespace Sync
namespace Service

/-- A peer's prepaid service account: bytes still usable, and bytes already served. -/
structure Account where
  credit : Nat
  served : Nat
  deriving DecidableEq

/-- What a range costs: its length in bytes. A definition, not an assumption — changing the price
    model changes this function and the theorem below with it. -/
def cost (bytes : Nat) : Nat := bytes

/-- **Serve a range.** The only constructor of a post-service account: `none` when the credit does
    not cover the range, so an account cannot be debited into debt. -/
def serve (a : Account) (bytes : Nat) : Option Account :=
  if h : cost bytes ≤ a.credit then
    some ⟨a.credit - cost bytes, a.served + bytes⟩
  else
    none

/-- **Law 110a — only a paid range is served.** -/
theorem served_requires_credit (a : Account) (bytes : Nat) (a' : Account)
    (h : serve a bytes = some a') : cost bytes ≤ a.credit := by
  unfold serve at h
  split at h
  · assumption
  · contradiction

/-- **Law 110b — an unpayable range is refused.** -/
theorem an_unpaid_range_is_refused (a : Account) (bytes : Nat)
    (h : a.credit < cost bytes) : serve a bytes = none := by
  unfold serve
  split
  · omega
  · rfl

/-- **Law 110c — the debit is exact.** A served range adds its bytes to `served` and takes exactly
    its cost from `credit`; no other accounting is possible. -/
theorem a_served_range_is_accounted (a : Account) (bytes : Nat) (a' : Account)
    (h : serve a bytes = some a') :
    a'.served = a.served + bytes ∧ a'.credit + cost bytes = a.credit := by
  unfold serve at h
  split at h
  · rename_i hle
    simp only [Option.some.injEq] at h
    subst h
    constructor
    · rfl
    · simpa [cost] using Nat.sub_add_cancel hle
  · contradiction

/-- Non-vacuity: an account with enough credit serves the range. -/
theorem a_paid_range_is_served (credit bytes : Nat) (h : cost bytes ≤ credit) :
    ∃ a', serve ⟨credit, 0⟩ bytes = some a' := by
  refine ⟨⟨credit - cost bytes, bytes⟩, ?_⟩
  unfold serve
  simp [h]

/-- Non-vacuity of 110b: a non-empty range from an account one byte short is refused. (The
    non-emptiness hypothesis is needed because `Nat` subtraction truncates: for a zero-byte range the
    "short" account is not short at all.) -/
theorem a_short_account_is_refused (bytes : Nat) (h : 1 ≤ cost bytes) :
    serve ⟨cost bytes - 1, 0⟩ bytes = none := by
  have hb : 1 ≤ bytes := by simpa [cost] using h
  have hnot : ¬ (bytes ≤ bytes - 1) := by omega
  simp [serve, cost, hnot]

end Service
end Sync

end Rchain
