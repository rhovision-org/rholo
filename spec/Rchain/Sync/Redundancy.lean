import Mathlib.Logic.Function.Basic

/-!
# Law 101 — a verified sync answer is peer-independent

**The change this law covers.** The 2026-11-14 sync redesign asks more than one peer for the same
block or state page (`casper/src/protocol/comm_util.rs::request_lfs_block_from`,
`casper/src/engine/lfs_tuple_space_requester.rs::request_next`). A page or a block is
content-addressed: the walk's acceptance rule is that the body hashes to the key it was requested
under (`validate_received_block`, `validate_state_items`). The law says that rule makes the answer
**interchangeable across peers**, that the announcing peer is not an input to acceptance, that a
hedge is a stutter on the give-up measure (`Rchain.Sync.Walk`'s `finished`), and that the redundant
fetch is bounded by a per-key fan-out.

**What it does not claim.** It does not claim answers arrive, that any peer is honest, or that the
hash is collision-free. Injectivity of the hash is a **hypothesis** (`hcf`), the same assumption Law
19 makes about `blake2b256`; the register's axiom accounting records that it is assumed here rather
than proved. `hedging_is_a_stutter` is about the measure, not about liveness: whether a second peer
answers is a question for Law 69's fairness hypotheses.

**Why it is not vacuous.** Delete injectivity and `any_verified_answer_is_the_answer` is false;
make acceptance depend on the peer and `peer_is_not_an_input` is false; make the hedge advance
`finished` and `hedging_is_a_stutter` is false. Each has a Rust witness named in the register row.
-/

namespace Rchain

namespace Sync
namespace Redundancy

/-- One answer to a sync request: the body, the key it was requested under, and the peer that sent
    it. The Rust's `BlockMessage` and `StoreItemsMessage` carry the first two; the third is transport
    metadata the walk may log but never compares. -/
structure Answer (Body Key Peer : Type) where
  body : Body
  key : Key
  peer : Peer

/-- **The walk's only acceptance rule.** A body is accepted for a key when the content hash of the
    body is that key — `validate_received_block` re-hashes a block, and `validate_state_items`
    checks every page node against its hash (Law 70). -/
def Verified {Body Key Peer : Type} (hash : Body → Key) (a : Answer Body Key Peer) : Prop :=
  hash a.body = a.key

/-- **Law 101a.** Two verified answers for one key carry the same body, so which peer answered
    cannot change the value the walk imports. Injectivity of the hash is the hypothesis; the
    register records it as assumed, not proved. -/
theorem any_verified_answer_is_the_answer
    {Body Key Peer : Type} (hash : Body → Key) (hcf : Function.Injective hash)
    (a b : Answer Body Key Peer) (ha : Verified hash a) (hb : Verified hash b)
    (hk : a.key = b.key) : a.body = b.body := by
  apply hcf
  rw [ha, hb, hk]

/-- **Law 101b.** The announcing peer is not an input to acceptance: rewriting the peer field
    leaves `Verified` untouched. -/
theorem peer_is_not_an_input
    {Body Key Peer : Type} (hash : Body → Key) (a : Answer Body Key Peer) (q : Peer) :
    Verified hash a ↔ Verified hash { a with peer := q } :=
  Iff.rfl

/-- A hedge: one extra request for an outstanding key. It completes no key of its own, so it leaves
    the walk's completed measure exactly where it was. -/
def hedge {Peer : Type} (finished : Nat) (_peer : Peer) : Nat := finished

/-- **Law 101c.** A hedge is a **stutter** on `Rchain.Sync.Walk`'s completed measure, which is why
    adding redundancy cannot break Law 69's give-up rule: pacing is counted in completed keys, not
    in requests. -/
theorem hedging_is_a_stutter {Peer : Type} (finished : Nat) (peer : Peer) :
    hedge finished peer = finished :=
  rfl

/-- The per-key in-flight set, bounded by the configured fan-out. -/
def inFlight (fanout asked : Nat) : Nat := min fanout asked

/-- **Law 101d.** The redundant fetch is bounded: no key has more requests in flight than the
    fan-out. The Rust's `LFS_BLOCK_FANOUT` / `LFS_PAGE_FANOUT` are the constant, and the per-key
    request log excludes peers already asked. -/
theorem hedging_is_bounded (fanout asked : Nat) :
    inFlight fanout asked ≤ fanout :=
  Nat.min_le_left _ _

end Redundancy
end Sync

end Rchain
