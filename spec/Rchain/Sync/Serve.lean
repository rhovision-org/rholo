import Mathlib.Logic.Function.Basic

/-!
# Law 103 — concurrent serving preserves answer identity and the in-flight bound

**The change this law covers.** The sync responder answered one peer message at a time on the
per-shard loop; the redesign moves the expensive read/answer kinds into a bounded spawned pool
(`casper/src/engine/node_launch.rs`). Concurrency raises two questions the serial loop did not have
to answer: can a request's answer be paired with the wrong request, and can the pool exceed its
bound?

**What this law says.** The answer is a pure function of the request and the immutable trie
(`serve` reads `path`), the announcing peer is not an input, the answer to one path does not depend
on another path's request, and the pool's in-flight count is bounded by its configured width.

**What it does not claim.** It does not model the store's internal locking, the trie's immutability
proof (Law 70), or the catch-up lane's ordering. The Rust's per-`(peer, path)` cursor chain is not in
this model; it is the responder's own test surface. This is the structural half of §165's C374.

**Why it is not vacuous.** Make `serve` read the peer and `the_peer_is_not_an_input` fails; drop the
`min` in `InFlight` and `in_flight_is_bounded` fails; make the same path's answer depend on another
request and `an_answer_is_a_function_of_its_own_request` fails.
-/

namespace Rchain

namespace Sync
namespace Serve

/-- One store-items request: the announcing peer and the trie path. -/
structure Req where
  peer : Nat
  path : Nat

/-- The answer is a read of the immutable trie at the request's path. -/
def serve (trie : Nat → Nat) (r : Req) : Nat := trie r.path

/-- **Law 103a — an answer is a function of its own request.** -/
theorem an_answer_is_a_function_of_its_own_request (trie : Nat → Nat) (r : Req) :
    serve trie r = trie r.path :=
  rfl

/-- **Law 103b — the announcing peer is not an input to the answer.** -/
theorem the_peer_is_not_an_input (trie : Nat → Nat) (r : Req) (q : Nat) :
    serve trie { r with peer := q } = serve trie r :=
  rfl

/-- The answers to a batch, in request order. -/
def answers (trie : Nat → Nat) (requests : List Req) : List Nat :=
  requests.map (serve trie)

/-- **Law 103c — serving is order-independent at the answer level.** Reversing the requests
    reverses the answers and changes no answer's value; only the list order moves. -/
theorem reversing_the_requests_reverses_the_answers (trie : Nat → Nat) (requests : List Req) :
    answers trie requests.reverse = (answers trie requests).reverse := by
  simp [answers]

/-- The number of handlers the pool can run: the width, capped by the requests offered. -/
def InFlight (width : Nat) (requests : List Req) : Nat := min width requests.length

/-- **Law 103d — the in-flight set is bounded by the configured width.** -/
theorem in_flight_is_bounded (width : Nat) (requests : List Req) :
    InFlight width requests ≤ width :=
  Nat.min_le_left _ _

end Serve
end Sync

end Rchain
