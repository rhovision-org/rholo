import Rchain.Casper.Validate

/-!
# Law 92 — a replay failure's cause depends on the block's provenance

The 2026-11-08 incident (#564): a proposer built a block, its own replay returned
`ReplayCostMismatch`, and the node classified that as `FailureCause::Divergence` — the cause that
means *this node's view disagrees with the producer's*. There is no producer to disagree with when the
producer is this node. The same replay failure, read as divergence, was not counted, surfaced or
bounded, and the proposer rebuilt the identical block until finality froze.

**The rule, stated before the Rust.** A replay failure has two possible meanings, and the block's
origin picks between them:

* **inbound** — the block came from a peer. Its replay disagreement may be a view difference, so it is
  `Divergence`; the block may be valid elsewhere, and the C173 restoring rule is keyed on exactly
  that cause.
* **self-built** — this node just built the block. Its play and its replay are two executions of
  *this node's own* state, so a disagreement is a local fault. It is never `Divergence`, because
  there is no second view for the node to be diverged from.

**Why this is a law and not a code comment.** The classification is the first hop of the liveness
failure: `handle_errors` collapsed every non-internal `ReplayFailure` into the same hash-mismatch
reading, so the proposer's divergence arm — deliberately not counted (C270) — received a fault that
was not a divergence at all. The model below is deliberately small: it is the decision table, not the
validator. It says which cause a caller may assign, and the Rust's `ValidationOrigin` is that table's
implementation. The bounded-retry policy (holding a deploy set out of selection) is node-local
operating policy, not a consensus rule; it is registered as C357 and is not part of this law.

Layer `Casper`.
-/

namespace Rchain

/-- Where the block being validated came from. -/
inductive ProvenanceOrigin where
  /-- A block received from a peer (or re-read from the store for revalidation). -/
  | peer
  /-- The block this node just built and is validating before broadcast. -/
  | selfBuilt
deriving DecidableEq, Repr

/-- What the replay comparison found. `other` is a pre-replay refusal or a matching replay; only the
replay-mismatch case is origin-sensitive. -/
inductive ProvenanceRefusal where
  | replayMismatch
  | other
deriving DecidableEq, Repr

/-- The causes a refusal may be assigned. Mirrors `FailureCause`, with `localFault` for the case the
Rust enum does not carry: a self-built block's own play/replay disagreement. -/
inductive ProvenanceCause where
  | attributable
  | divergence
  | localFault
deriving DecidableEq, Repr

/-- **The decision table.** Provenance decides the meaning of a replay mismatch; every other refusal
is unaffected. -/
def provenanceCause : ProvenanceOrigin → ProvenanceRefusal → ProvenanceCause
  | .peer, .replayMismatch => .divergence
  | .selfBuilt, .replayMismatch => .localFault
  | _, _ => .attributable

/-- The inbound half is unchanged by this law, and that is load-bearing: C173's restoring rule is
keyed on `Divergence`, so a peer's replay disagreement must keep that cause. -/
theorem an_inbound_replay_failure_is_divergence :
    provenanceCause .peer .replayMismatch = .divergence := rfl

/-- **The incident's classification error, refuted.** A self-built block's replay failure is not a
view divergence. -/
theorem a_self_replay_failure_is_not_divergence :
    provenanceCause .selfBuilt .replayMismatch ≠ .divergence := by decide

/-- The two origins really do disagree about the same failure — the law is not a restatement of a
constant. -/
theorem the_two_origins_disagree_about_the_same_failure :
    provenanceCause .peer .replayMismatch ≠ provenanceCause .selfBuilt .replayMismatch := by decide

/-- **Non-vacuity**: `localFault` is inhabited by the self-built case, so the decision table has an
output the Rust can actually reach. -/
theorem a_self_replay_failure_is_a_local_fault :
    provenanceCause .selfBuilt .replayMismatch = .localFault := rfl

/-- **The wrong table**: origin ignored, every replay mismatch read as divergence. This is the
    classification the incident's `handle_errors` collapsed to. -/
def provenanceCauseIgnoringOrigin : ProvenanceOrigin → ProvenanceRefusal → ProvenanceCause
  | _, .replayMismatch => .divergence
  | _, _ => .attributable

/-- **Law 92's mutant**: the origin-ignoring table maps a self-built replay mismatch to
    divergence — the defect shape the law forbids. -/
def law92_mutant : Prop :=
  provenanceCauseIgnoringOrigin .selfBuilt .replayMismatch = .divergence

/-- **The falsifier, fired**: the mutant really does make the self-built case a divergence. -/
theorem law92_falsifier : law92_mutant := rfl

/-- **Law 92's statement.** Provenance decides the cause: an inbound replay mismatch stays
    `divergence`, a self-built one is a `localFault`, the two origins disagree about the same
    failure, and the origin-ignoring mutant is exhibited. -/
def law92_statement : Prop :=
  provenanceCause .peer .replayMismatch = .divergence ∧
  provenanceCause .selfBuilt .replayMismatch ≠ .divergence ∧
  provenanceCause .peer .replayMismatch ≠ provenanceCause .selfBuilt .replayMismatch ∧
  provenanceCause .selfBuilt .replayMismatch = .localFault ∧
  law92_mutant

theorem law92 : law92_statement :=
  ⟨an_inbound_replay_failure_is_divergence,
   a_self_replay_failure_is_not_divergence,
   the_two_origins_disagree_about_the_same_failure,
   a_self_replay_failure_is_a_local_fault,
   law92_falsifier⟩

end Rchain
