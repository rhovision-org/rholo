import Mathlib.Tactic

/-!
# Law 112 — an accepted audit claim is one this node reproduced

**The change this law covers.** `rnode audit` reproduces a checkpoint from genesis, but its report is
a local file: anyone can copy it, alter it, or repeat the claim for free. A signed audit attestation
(`models/src/casper/protocol/audit.rs`) names the checkpoint, the derived root, a replay head and a
challenge nonce, and chains to the signer's previous claim. The **acceptance** rule
(`casper/src/audit.rs`) is reproduction: a claim enters the accepted set only when this node's own
replay agrees on the root and the replay head, and the claim extends its signer's chain.

**What this law says.** `accept` produces a `Verified` value only when the claim's root and replay
head are the truth's; a mismatched claim is refused; an unchained claim cannot extend a head.

**What it does not claim.** The signature is checked by the Rust caller; this law is about the
acceptance relation, not about key custody. A claim's *nonce* freshness and the anti-spam request
path are the service's (`Law 110`'s accounting and the rate limit), not this law.

**Why it is not vacuous.** Make `accept` total (always `some`) and `a_mismatched_claim_is_refused`
fails; ignore `previous` in `chain` and `an_unchained_claim_is_refused` fails. The Rust witnesses are
the verifier's reproduction and chain tests.
-/

namespace Rchain

namespace Sync
namespace Claim

/-- A signed claim: its previous claim, its own digest, and the facts it asserts. -/
structure Claim where
  previous : Option Nat
  digest : Nat
  root : Nat
  head : Nat
  deriving DecidableEq

/-- What this node's own replay produced. -/
structure Truth where
  root : Nat
  head : Nat
  deriving DecidableEq

/-- **A claim this node reproduced.** The only witness that a claim is accepted. -/
structure Verified (t : Truth) where
  claim : Claim
  root_eq : claim.root = t.root
  head_eq : claim.head = t.head

/-- Accept a claim only against the truth this node reproduced. -/
def accept (c : Claim) (t : Truth) : Option (Verified t) :=
  if h : c.root = t.root ∧ c.head = t.head then
    some ⟨c, h.1, h.2⟩
  else
    none

/-- **Law 112a — only a reproduced claim is accepted**, and its facts are the truth's. -/
theorem only_a_reproduced_claim_is_accepted (c : Claim) (t : Truth) (v : Verified t)
    (h : accept c t = some v) : v.claim.root = t.root ∧ v.claim.head = t.head := by
  unfold accept at h
  split at h
  · rename_i hcond
    simp only [Option.some.injEq] at h
    subst h
    exact hcond
  · contradiction

/-- **Law 112b — a mismatched claim is refused.** -/
theorem a_mismatched_claim_is_refused (c : Claim) (t : Truth) (h : c.root ≠ t.root) :
    accept c t = none := by
  unfold accept
  split
  · rename_i hcond
    exact absurd hcond.1 h
  · rfl

/-- Non-vacuity: a claim that reproduces the truth is accepted. -/
theorem a_reproduced_claim_is_accepted (c : Claim) (t : Truth)
    (root_eq : c.root = t.root) (head_eq : c.head = t.head) :
    (accept c t).isSome := by
  unfold accept
  split
  · simp
  · rename_i hcond
    exact absurd ⟨root_eq, head_eq⟩ hcond

/-- **A chained claim**: it names the previous accepted claim's digest and reproduces its truth. -/
def chain {t : Truth} (prev : Verified t) (c : Claim) (t' : Truth) : Option (Verified t') :=
  if c.previous = some prev.claim.digest then accept c t' else none

/-- **Law 112c — an unchained claim is refused.** -/
theorem an_unchained_claim_is_refused {t : Truth} (prev : Verified t) (c : Claim) (t' : Truth)
    (h : c.previous ≠ some prev.claim.digest) : chain prev c t' = none := by
  unfold chain
  simp [h]

/-- Non-vacuity of 112c: a properly chained and reproduced claim extends the head. -/
theorem a_chained_claim_extends {t : Truth} (prev : Verified t) (c : Claim) (t' : Truth)
    (linked : c.previous = some prev.claim.digest) (root_eq : c.root = t'.root)
    (head_eq : c.head = t'.head) : (chain prev c t').isSome := by
  unfold chain
  split
  · exact a_reproduced_claim_is_accepted c t' root_eq head_eq
  · rename_i hcond
    exact absurd linked hcond

end Claim
end Sync

end Rchain
