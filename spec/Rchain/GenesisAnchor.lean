import Rchain.Casper.Stake

/-!
# Law 84 — the genesis anchor is a function of the genesis inputs

**The defect this file models.** #456 measured two nodes with identical genesis inputs — the same
`bonds.txt`, `wallets.txt`, parameters and deployer key — producing **different genesis blocks**: the
sender was each node's local validator key, and the blessed deploys were signed by it, so both the
block's `sender` and every deploy's signature (which the block hash covers) differed. The post-state
differed too, because the master dictionary's admin capability is the ceremony key. A joiner therefore
cannot recompute the network's genesis; it must be handed the block (#456).

**The rule.** The ceremony's identity is the **first bond in the bonds file** — an input, like the
stakes and wallets. The block's sender, the blessed deploys' signer and the governance admin are that
key. Because the signing schemes in `crypto/` are deterministic (RFC-6979 for secp256k1, deterministic
Ed25519), identical inputs give identical bytes, so the genesis hash is a function of the inputs alone.
A node holding any other key must **refuse** to run the ceremony rather than build a private chain —
which is the failure the old `rnode-c` lived in for a month.

The model abstracts the hash and the post-state as functions of the creator: the law's content is that
the creator is **forced** by the inputs, so those functions can only be evaluated at one point.
-/

namespace Rchain

/-- The genesis inputs this law reasons about: the bonded validators **in bonds-file order**. -/
structure GenesisInputs where
  bonds : List Validator

/-- The deterministic ceremony identity: the first bond in the file. `none` is a genesis with no
    bonds, which is refused rather than created. -/
def genesisCreator (i : GenesisInputs) : Option Validator := i.bonds.head?

/-- Who may run the ceremony: only the derived creator. The Rust enforces the same predicate — a node
    whose local key is not the first bond refuses at `create_genesis` with both keys named. -/
def mayCreate (i : GenesisInputs) (v : Validator) : Prop := genesisCreator i = some v

/-- **Every genesis-derived value is a function of the inputs.** Two creators admitted by the same
    inputs are the same creator, so any hash (or state) computed from them agrees. -/
theorem the_genesis_anchor_is_a_function_of_the_inputs
    (hashOf : Validator → Nat) (i : GenesisInputs) {a b : Validator}
    (ha : mayCreate i a) (hb : mayCreate i b) : hashOf a = hashOf b := by
  unfold mayCreate genesisCreator at ha hb
  rw [ha] at hb
  injection hb with h
  rw [h]

/-- The same statement for the state the ceremony writes: the master dictionary's admin key and every
    genesis deploy's signer are the creator, so the post-state hash is a function of the inputs. -/
theorem the_genesis_state_is_a_function_of_the_inputs
    (stateOf : GenesisInputs → Validator → Nat) (i : GenesisInputs) {a b : Validator}
    (ha : mayCreate i a) (hb : mayCreate i b) : stateOf i a = stateOf i b := by
  unfold mayCreate genesisCreator at ha hb
  rw [ha] at hb
  injection hb with h
  rw [h]

/-- **The falsifier, red on the rule as it was.** An *open* ceremony — any local key may sign — lets
    one bonds file produce two different genesis hashes. The witness is the smallest one: one bond,
    two other keys. `mayCreate` refuses both under the rule, and the hashes differ. -/
theorem an_open_ceremony_reaches_two_genesis_hashes :
    ∃ (i : GenesisInputs) (a b : Validator),
      (fun v : Validator => v.id) a ≠ (fun v : Validator => v.id) b ∧
        ¬ mayCreate i a ∧ ¬ mayCreate i b := by
  refine ⟨⟨[⟨0⟩]⟩, ⟨1⟩, ⟨2⟩, ?_, ?_, ?_⟩ <;>
    simp [mayCreate, genesisCreator]

/-- The rule is not vacuous: the first bond is admitted. -/
theorem the_first_bond_may_create (v : Validator) (rest : List Validator) :
    mayCreate ⟨v :: rest⟩ v := by
  simp [mayCreate, genesisCreator]

end Rchain
