import Rchain.Bounded

/-!
# Law 90 — the published hostlist is earned, bounded and fresh

A wallet knows one node. `GET /api/v1/nodes` lets that node answer with the *other* nodes a client can
send a deploy to, so load can be spread instead of concentrated on whichever endpoint is hardcoded
into a wallet (`docs/src/developer/building-apps.md` §3.5b).

Three things make that list safe to act on:

* **90a — membership is earned.** A peer is served only if a probe of its derived client endpoint
  completed, named *that* peer, reported the serving node's network and one of its shards, and
  reported `proposer = true`. A peer that does not answer, answers for another network or node, or
  cannot propose is not served — so every entry a client spreads a deploy over can include it.
* **90b — bounded, deduplicated, ordered.** The list is at most `maxEntries`, one entry per node id,
  the serving node first when it can propose, and the rest ordered by node id ascending — the
  response is a function of the bound, not of the network's size, and its order is stable.
* **90c — fresh.** The list is derived only from the probes of the cycle that produced it. A peer
  whose probe fails is absent from that cycle's list; there is no cache that could revive it.

The model abstracts the URL to the node id: the HTTP URL is `http(S)://<host>:<protocol + 3>`, a
function of the peer's endpoint, and the register's Rust anchors carry the derivation and the probe.
What the model does **not** claim, and the register's note says so: `proposer` is a peer's own claim
(plain HTTP, so an on-path observer can forge a well-formed answer), and the host-class and body-size
bounds are Rust-tested implementation facts rather than modelled here.
-/

namespace Rchain
namespace HostList

/-- A peer: its TLS identity and the endpoint its client port is derived from. -/
structure Peer where
  id : Nat
  host : String
  protocolPort : Nat
  deriving DecidableEq, Repr

/-- What a peer's `/api/v1/status` said, after parsing. `address` is the node id the answer names,
    `proposer` is the peer's claim that it holds a validator key. -/
structure Answer where
  address : Nat
  networkId : String
  shardId : String
  proposer : Bool
  deriving DecidableEq, Repr

/-- One probe: the peer, and its answer when it gave one. `none` is a failed, refused or malformed
    probe — never "probably fine". -/
structure Outcome where
  peer : Peer
  answer : Option Answer
  deriving DecidableEq, Repr

/-- The serving node and the bounds its answer is derived against. -/
structure Ctx where
  networkId : String
  shardIds : List String
  maxEntries : Nat
  self : Peer
  selfProposer : Bool
  deriving DecidableEq, Repr

/-- **The membership predicate (90a).** All four conditions, or the peer is not served. -/
def valid (ctx : Ctx) (o : Outcome) : Bool :=
  match o.answer with
  | none => false
  | some a =>
      a.networkId == ctx.networkId
        && ctx.shardIds.contains a.shardId
        && a.proposer
        && a.address == o.peer.id

/-- The largest peer id in the probe set, so the ordered walk below is finite. -/
def maxId (os : List Outcome) : Nat :=
  os.foldr (fun o m => max o.peer.id m) 0

/-- **The ordered peer part (90b).** Ids are walked in ascending order and kept only when some
    outcome for that peer passes `valid`; self is excluded (it is prepended when it can propose).
    Ascending order is by construction rather than by a sort, so it cannot drift from the bound. -/
def peerPart (ctx : Ctx) (os : List Outcome) : List Nat :=
  (List.range (maxId os + 1)).filter (fun i =>
    i != ctx.self.id && os.any (fun o => o.peer.id == i && valid ctx o))

/-- The served ids: self first when it can propose, then the ordered peer part, cut to the bound. -/
def servedIds (ctx : Ctx) (os : List Outcome) : List Nat :=
  let selfPart := if ctx.selfProposer then [ctx.self.id] else []
  (selfPart ++ peerPart ctx os).take ctx.maxEntries

/-- One conformance case: the context and probe set the Lean and the Rust both run, and the ids the
    derivation must produce. `Corpus.lean` carries the case list and emits it to
    `spec/conformance/hostlist.tsv`; `node/tests/lean_hostlist_corpus.rs` reads the same file. -/
structure Case where
  name : String
  ctx : Ctx
  outcomes : List Outcome
  expected : List Nat
  deriving Repr

/-! ## Theorems -/

/-- **A served peer id names a peer whose probe passed `valid`.** The membership side of 90a: no id
    reaches the list except through the filter over an outcome that passed every condition. -/
theorem peer_part_mem {ctx : Ctx} {os : List Outcome} {i : Nat}
    (hi : i ∈ peerPart ctx os) :
    ∃ o ∈ os, o.peer.id = i ∧ valid ctx o = true := by
  unfold peerPart at hi
  rw [List.mem_filter] at hi
  obtain ⟨_, hpred⟩ := hi
  rw [Bool.and_eq_true] at hpred
  obtain ⟨_, hany⟩ := hpred
  rw [List.any_eq_true] at hany
  obtain ⟨o, ho, hpred⟩ := hany
  rw [Bool.and_eq_true] at hpred
  exact ⟨o, ho, by simpa using hpred.1, hpred.2⟩

/-- **Every served entry is valid** (90a), whether it is self or a peer. -/
theorem every_served_entry_is_valid {ctx : Ctx} {os : List Outcome} {i : Nat}
    (hi : i ∈ servedIds ctx os) :
    (i = ctx.self.id ∧ ctx.selfProposer = true) ∨
      (∃ o ∈ os, o.peer.id = i ∧ valid ctx o = true) := by
  unfold servedIds at hi
  have htake : i ∈ (if ctx.selfProposer then [ctx.self.id] else []) ++ peerPart ctx os :=
    List.mem_of_mem_take hi
  rw [List.mem_append] at htake
  rcases htake with hself | hpeer
  · by_cases hp : ctx.selfProposer
    · left
      refine ⟨?_, hp⟩
      simp [hp] at hself
      exact hself
    · simp [hp] at hself
  · right
    exact peer_part_mem hpeer

/-- **Every served peer can propose** (90a): `valid` includes `a.proposer`, so a node that cannot
    include a deploy can never be an entry a client spreads one over. -/
theorem every_served_peer_is_a_proposer {ctx : Ctx} {os : List Outcome} {i : Nat}
    (hpeer : i ∈ peerPart ctx os) :
    ∃ o ∈ os, o.peer.id = i ∧ o.answer.any (fun a => a.proposer) = true := by
  obtain ⟨o, ho, hid, hv⟩ := peer_part_mem hpeer
  refine ⟨o, ho, hid, ?_⟩
  cases hans : o.answer with
  | none => simp [valid, hans] at hv
  | some a =>
      simp [valid, hans, Bool.and_eq_true] at hv
      simp [hans, hv.1.2]

/-- **The list is bounded** (90b): the response is a function of `maxEntries`, never of the network's
    size. -/
theorem the_list_is_bounded (ctx : Ctx) (os : List Outcome) :
    (servedIds ctx os).length ≤ ctx.maxEntries := by
  unfold servedIds
  exact Nat.le_trans (List.length_take_le _ _) (Nat.le_refl _)

/-- **Self is first** (90b) when it can propose and the bound is not zero. -/
theorem self_is_first (ctx : Ctx) (os : List Outcome)
    (h : ctx.selfProposer = true) (hpos : 0 < ctx.maxEntries) :
    (servedIds ctx os).head? = some ctx.self.id := by
  obtain ⟨n, hn⟩ : ∃ n, ctx.maxEntries = n + 1 :=
    Nat.exists_eq_succ_of_ne_zero (Nat.ne_of_gt hpos)
  unfold servedIds
  simp [h, hn]

/-- **Self is present** (90b) whenever it can propose and there is room for an entry. -/
theorem self_is_present_when_a_proposer (ctx : Ctx) (os : List Outcome)
    (h : ctx.selfProposer = true) (hpos : 0 < ctx.maxEntries) :
    ctx.self.id ∈ servedIds ctx os := by
  obtain ⟨n, hn⟩ : ∃ n, ctx.maxEntries = n + 1 :=
    Nat.exists_eq_succ_of_ne_zero (Nat.ne_of_gt hpos)
  unfold servedIds
  simp [h, hn]

/-- **A failed probe is absent** (90c): a candidate whose only outcome in the set failed is not
    served. The qualifier matters and is why the theorem is stated this way: two outcomes may share a
    node id, and the healthy one is evidence the peer is up. The cycle's candidate set holds one
    outcome per peer, which is the case this theorem covers. -/
theorem a_failed_single_probe_is_absent {ctx : Ctx} {p : Peer}
    (hnone : (Outcome.mk p none).answer = none) :
    p.id ∉ peerPart ctx [Outcome.mk p none] := by
  intro hmem
  obtain ⟨o, ho, hid, hv⟩ := peer_part_mem hmem
  rw [List.mem_singleton] at ho
  subst ho
  simp [valid, hnone] at hv

end HostList
end Rchain
