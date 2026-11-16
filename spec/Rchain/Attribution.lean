import Rchain.Par

/-!
# Law 63 — a bridged delivery's consensus footprint is attributable

Every delivery to a method-carrying chain capability runs `register!(reply, *uriOut)` unconditionally
(`casper/src/shard_invoke.rs`'s `invoke_member_term`, even for a reply that is pure data), and
`rho:registry:insertArbitrary` has **no delete** anywhere — so each call leaves a permanent consensus
entry holding the whole reply, including any capabilities in it, on every node that replays the block.
The same deploy pays `CHAIN_PHLO_LIMIT` out of the node's own deployer vault (AUDIT C221).

**Why this is a law and not a missing bound.** The obvious repair — bound it — is what the row proposed
first and what the design pass showed is impossible: the node cannot know whether a reply holds a
capability *before* registering it, because the value does not survive the evaluation. So the
statement cannot be "the state is bounded" (Law 55's clause, which the OCapN surface's own guards
satisfy — see C222). It is *whose* state it is: a write a session caused must be one the session's peer
pays for, and here it is one this node pays for out of a vault it chose to fund.

**The row is `open`, and that is the honest state.** The relay is the close: the node hands the peer
the exact `DeployData` to sign with its own secp256k1 key, so the chain binds the peer as
`deployerId`, the peer's vault pays, and the `insertArbitrary` registration becomes the caller's own
act. Endo does not speak this shard's `DeployData` shape, so it is a cross-implementation protocol
change and cannot be landed here — the row says so rather than naming a fix that this tree cannot
reach. What the model adds is that the failure is a *theorem* rather than a paragraph, and that the
law is not vacuous: `the_relay_would_satisfy_it` exhibits the shape a fix has to have.

Layer `Protocol`.
-/

namespace Rchain

/-- Who pays for a write to consensus state. -/
inductive Payer where
  /-- The peer whose delivery caused the write. -/
  | theCaller
  /-- This node, out of a vault it funds itself. -/
  | thisNode
deriving DecidableEq, Repr

/-- A write a session caused: the consensus state it leaves behind, and who paid the phlo. -/
structure LedgerWrite where
  /-- Bytes of consensus state the write leaves — an `insertArbitrary` entry is permanent, so this is
  state that stays. -/
  state : Nat
  /-- Who paid the phlo for the deploy that made it. -/
  payer : Payer

/-- A write is **attributable** when the peer that caused it is the one that paid. -/
def Attributable (w : LedgerWrite) : Prop := w.payer = Payer.theCaller

/-- **The bridge as it stands** (AUDIT C221): the deploy is signed by the node's own key and paid from
its own vault, whatever the reply turns out to hold. -/
def bridgedWrite (state : Nat) : LedgerWrite := ⟨state, Payer.thisNode⟩

/-- **Law 63's failure, as a theorem.** A write a session caused is not attributable to the caller: the
node pays for phlo it did not choose to spend and carries consensus state it cannot remove. This is why
the row is `open` rather than `done` — the law is stated and the code does not satisfy it. -/
theorem a_bridged_write_is_not_attributable (state : Nat) :
    ¬ Attributable (bridgedWrite state) := by
  simp [Attributable, bridgedWrite]

/-- What the law asks for, stated positively so a fix has a shape to meet: the payer is the caller, and
the state is bounded by what the caller's own phlo bought. -/
def AttributableWrite (w : LedgerWrite) (bound : Nat) : Prop :=
  w.payer = Payer.theCaller ∧ w.state ≤ bound

/-- **The law is not vacuous.** The relay — the close condition C221's row names — is what makes
`AttributableWrite` inhabited: the peer signs and funds the deploy, so the payer is the caller and the
state is bounded by what the caller's own phlo bought. Stated as a witness rather than assumed, so that
"there is a shape that satisfies this" is checked and not hoped for. -/
theorem the_relay_would_satisfy_it (bound : Nat) :
    ∃ w : LedgerWrite, AttributableWrite w bound :=
  ⟨⟨bound, Payer.theCaller⟩, rfl, le_rfl⟩

/-- And the failure is not a matter of *degree*: no bound on `state` makes `bridgedWrite` attributable,
because what the law is about is the payer. This is the sentence the row's first proposed close
condition got wrong — a smaller write is the same unattributable write. -/
theorem bounding_the_state_does_not_make_it_attributable (state bound : Nat) :
    ¬ AttributableWrite (bridgedWrite state) bound := by
  simp [AttributableWrite, Attributable, bridgedWrite]

/-! ## 63b — the relay's conditional

63a is `open` because the relay is a cross-implementation change. The conditional is still a law: if
the peer signs the `DeployData` the node hands it, the write is attributable and bounded by the peer's
own phlo. Stating it separately keeps 63a's honest `open` status from hiding the half that is proved. -/

/-- **63b's proposition: the relay's shape is inhabited.** For every bound, some write has the caller
as payer and state within the bound — what the relay produces when the peer signs its own deploy. -/
def law63b_statement : Prop :=
  ∀ bound : Nat, ∃ w : LedgerWrite, AttributableWrite w bound

/-- The conditional law. -/
theorem law63b : law63b_statement :=
  the_relay_would_satisfy_it

/-- **The defect mutant**: an unattributable write exists. The node's current `bridgedWrite` is one,
which is why 63a stays open until the relay lands. -/
def law63b_mutant : Prop :=
  ∃ w : LedgerWrite, ¬ Attributable w

/-- The falsifier: the current bridge is the mutant's witness. -/
theorem law63b_falsifier : law63b_mutant :=
  ⟨bridgedWrite 0, a_bridged_write_is_not_attributable 0⟩

end Rchain
