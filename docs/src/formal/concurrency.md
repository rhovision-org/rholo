# Concurrency: the model, the calculus, and the soundness theorems

Concurrency is where this specification earns its keep: rholang is a concurrent language, so *which*
par-reduction orders are sound — and why the obvious answer ("disjoint footprints") is **wrong** — is a
law-level question rather than an implementation preference.

**What this document carries.** The four-level concurrency model and what each level may parallelize
(the model); the two core relations the rest builds on, structural congruence `≡` and reduction `⟶`
(laws 2 and 4); and the per-law concurrency profile with the soundness theorems it targets (the enabling,
constraining and supporting sets). The *scheduler* tiers — the claim queue, the DFS gate, and on-chain
validated speculation — are their own document, [Effect scheduling](scheduling.md); the level-4
determinism question is [Determinism of the block state transition](determinism.md).

**Laws this set pertains to:** 2 and 4 (the relations), 7–11 (the space's comm, merge and replay), 19
(the RNG's order-sensitivity), and the level-3/4 statements that laws 20–25 then make precise.

> The per-law **status** is the register's (`spec/LAWS.md`, emitted from `Rchain/Laws.lean` and refused
> stale by the gate); the totals are
> <!-- counts:laws-entries -->91 laws and 131 entries<!-- counts:end -->, of which
> <!-- counts:proved-laws -->84<!-- counts:end --> are proved at all. Nothing on this page restates a status.

---

## The concurrency model

> This section is the **specification of the node's concurrency model** — the statement of what may run
> concurrently, what must serialize, and *why*, founded in the law set. It is the target the Lean
> formalization (`spec/Rchain/`) proves. The two sections below spell out the machinery it rests on:
> *Structural congruence and reduction* (the core relations `≡` and `⟶`) and *Concurrent reduction*
> (the process-level contract and the theorems). The scheduler tiers are
> [Effect scheduling](scheduling.md), and this section ties them together and adds the block level.

### The invariant in one line

> **Concurrency is a scheduling freedom, never a semantics change.** Every concurrent execution — at
> any layer — must reach the same canonical state as the purely-sequential execution. The laws make
> this a *theorem*, not a convention: the reducer picks a **canonical, deterministic schedule** (DFS +
> content-sorted selection, Laws 4/8/11), and the state is content-addressed (Law 10). The *raw*
> nondeterministic `Reduce` relation is not even single-step deterministic up to `≡`, and is not
> confluent on the flat `Par` — determinism is a property of the chosen schedule, not of the relation.

### The three levels

Concurrency appears at three levels, each with its own enabling and constraining laws.

| Level | What runs concurrently | What serializes | Enabling laws | Constraining laws |
|-------|------------------------|-----------------|---------------|-------------------|
| **Reducer** (within a deploy) | the *pure* resolution of a `Par`'s terms — substitution, spatial matching, name allocation | the tuple-space effects, in DFS order | 1, 2, 19 | 4, 8 |
| **Effect** (matching + scheduling) | effects on **disjoint channels** (per-channel claim queues, Laws 20–22) | the per-channel commit order (DFS path order) | 20, 21, 22 | 4, 8, 11 |
| **Block** (validation) | replay of dependency-free blocks | DAG insertion, in topological order | 11 | 14, 15, 16, 18 |

### Level 1 — the reducer (fork-join over `|`)

A `Par` is a parallel composition (`|`, Law 2), so its sub-terms are *concurrent by construction*. The
reducer realizes this with a **fork-join**: `reduce_par` resolves every term's *pure* part
concurrently — the disjoint branches are spawned and joined (`rholang/src/reduce.rs:2211`,
`join_spawned` at `:2028`) — then applies the *effects* (the `produce`/`consume` calls that touch the
tuple space) in DFS order.

- **Why the pure part is concurrent.** Substitution, spatial matching, and `new`-name allocation are
  side-effect-free with respect to the tuple space; cost charges are atomic and each term's RNG is
  pre-split (`split_byte`/`split_short`, Law 19). So concurrent resolution is schedule-independent.
- **Why the effects are serial.** The effect *order* fixes which datum/continuation a comm consumes
  (Law 4/8), and a continuation's channel footprint is only known after its trigger effect runs — so
  the reducer keeps effects in DFS order to reproduce the sequential candidate choices.

See *Concurrent reduction* (below) for the theorems (independent-redex commute,
linearization).

### Level 2 — effect selection (content-addressed matching)

When a channel has more than one matching datum or continuation, the space must pick one. Selection is
**sorted-first by content hash** (`rspace/src/space_matcher.rs`, Law 8) rather than newest-first. This
removes the order-sensitivity of *which stored candidate* a comm consumes. See
[Sorted matching](../node/sorted-matching.md).

### Level 3 — effect scheduling (footprint is not enough; the claim queue is)

Two effects on **disjoint channels** touch disjoint state and commute (Law 9), so they may apply
concurrently. Two effects on the **same channel** must apply in DFS order (Law 4/8/11). But the
naive reading of "disjoint" is **insufficient**: a continuation's effects are discovered only after
its trigger runs, so an effect's *closure* can reach a "disjoint-looking" sibling's channel. The
channel-sharded scheduler (partition a `Par`'s effects by static footprint, run disjoint parts
concurrently) is therefore **unsound** — see [Effect scheduling](scheduling.md) S.3, and the
proved counterexample `Rchain.Effect.effect_reorder_diverges`.

The repair is the **channel scheduler** (Laws 20–22, see [The channel
scheduler](scheduling.md)): do not predict anything statically. Every effect *claims* the
channels it touches at its DFS path, and commits only when it is the head of every claimed channel
(Law 20) — per-channel order is enforced *dynamically*, by the queue, not by a static partition.
The **gate** variant runs effect `i` only after all earlier effects complete (Law 21) and is
provably the sequential fold; the **relaxed** variant applies the claim queue and lets
cross-channel commits interleave freely, at the cost of a non-sequential event log — which is
precisely why it is **off-chain only** (the casper block paths hard-reject it).

### Level 4 — cross-block validation (replay is verify-only)

Block validation *replays* a block's deploys against its recorded trace and checks the recomputed
post-state root (Law 11). Replay is **verify-only**: it does not change the committed state, and each
block's replay is self-contained (it starts from the block's `pre_state_hash`, a parent's committed
root). So **dependency-free blocks** — siblings whose parents are already in the DAG — can be replayed
concurrently, each on a freshly-forked `ReplayRhoRuntime` (`casper/src/runtime_manager.rs`
`fork_replay_runtime`), and the `block_processor` then inserts them serially in topological order.

This is the node's block-throughput scaling: it uses the *same* Laws 4/8/11 determinism as the other two
levels, but at the granularity of whole blocks.

### The limits of concurrency

Three levels of concurrency are sound and implemented:

1. **Level 1 — pure resolution.** A `Par`'s sub-terms (substitution, spatial matching, `new`-allocation)
   resolve concurrently; only the tuple-space effects serialize in DFS order. Sound because pure
   resolution is side-effect-free w.r.t. the tuple space and the RNG is pre-split (Law 19).
2. **Level 3 — the channel scheduler** (Laws 20–22): the **gate** mode (provably the sequential
   fold) and the **relaxed** mode (per-channel DFS order via the claim queue, free cross-channel
   interleaving — off-chain only, since its event log differs from the sequential one).
3. **Level 4 — cross-block replay.** Dependency-free blocks replay concurrently; insertion is serial.

**Static effect-level sharding is unsound** (`Rchain.Effect.effect_reorder_diverges`). Two distinct
obstacles stand in the way, and they are different in kind — the claim queue sidesteps both by
never predicting:

- **Obstacle A — the flat `Par` is not confluent.** The node's process is the flat field-wise `Par`;
  `parMerge` erases the tree structure that records *which* send pairs with *which* receive, so reduction
  is not even single-step deterministic up to `≡` (`Concurrent.lean`, `reduce_not_deterministic`).
  Confluence is recovered only in the **tree model** (`Tree.lean`, `reduceT_confluent`), where `par` is an
  injective constructor.
- **Obstacle B — a continuation's closure is dynamic.** The sound independence criterion is disjoint
  *closure*, not disjoint *footprint* (`Rchain.Effect.effect_commute_of_disjoint_closure`). But a
  continuation's closure depends on the datum the trigger matches: a receive `for (@x ← c) { @[x, *y]!(…) }`
  only reveals its output channel at match time. So closure is not statically decidable, and no *static*
  partition (by footprint, or even by closure) is sound. The claim queue does not need it: effects
  claim what they *actually* touch, at the path they *actually* run at, and the per-channel order is
  enforced at commit time.

### The path to pure ρ-calculus thread-level concurrency

The ρ-calculus *theoretically* permits `P | Q` to reduce `P` and `Q` on independent threads, confluently.
The channel scheduler realizes the *sound subset* of that: per-channel DFS order (Law 20) plus free
cross-channel interleaving, with the block paths gated to the sequential-equivalent modes. What remains
for full confluence — each still a research project, not an incremental scheduler tweak:

1. **Represent processes as trees, not the flat `Par`.** Adopt the tree model's `Proc` (explicit,
   injective `par` nodes) as the *execution* representation, so reduction is confluent
   (`reduceT_confluent`). The flat `Par` is the field-wise quotient that *causes* the non-confluence; a
   concurrent reducer must operate on the tree and flatten only at the canonicalization boundary
   (Obstacle A).

2. **Make closures static.** Effect-level independence is disjoint closure (Obstacle B). A
   **restricted calculus** whose channel positions are statically decidable — a receive body's channels
   computable without the matched datum — would make a closure-aware sharded scheduler sound, at the
   cost of the reflection that makes the ρ-calculus higher-order. (The dynamic alternative — tracking
   closures incrementally at runtime — is exactly the claim queue's specialization to per-channel order,
   and its parallelism collapses toward the DFS order in the general case, as the relaxed contract
   states.)

3. **Canonicalize after concurrent reduction.** Confluence yields "the same result up to `≡`"; consensus
   (Law 10) needs *one* canonical state. A concurrent reducer must end with a canonicalization step
   (Law 1 sort) that flattens the tree and orders its fields, turning the `≡`-class into a single
   content-addressed state.

Until these three are discharged formally (in `spec/Rchain/`), the reducer's on-chain concurrency is
bounded to Level 1, the gate mode, and Level 4; the relaxed mode carries the off-chain parallel
case.

### Soundness theorems (the Lean targets)

The model is sound if these hold. Each is the statement that a concurrent execution matches the
sequential one.

1. **Independent-redex commute** — two parallel steps on *independent* redexes commute
   (`parStep_comm`: `Reduce p p' → Reduce q q' → Reduce (p'|q) (p'|q') ∧ Reduce (p|q') (p'|q')`).
   The full **diamond/confluence does not hold** on the flat `Par`: a term with one receive and two
   sends on one channel is a redex in two ways, reducing to two inert, non-`≡` terms (see
   `spec/Rchain/Concurrent.lean`, `reduce_not_deterministic`). Confluence is a property of the *tree*
   model (explicit `par` nodes); the flat `Par` is its field-wise quotient.
2. **Linearization** — the sequential reducer (DFS, canonical order) is a valid refinement of the
   concurrent one; both reach the same `≡`-canonical state.
3. **Disjoint commute (footprint)** — `chans(e₁) ∩ chans(e₂) = ∅ ⇒ apply(e₁; e₂) ≡ apply(e₂; e₁)`.
   **False at the effect level** (`Rchain.Effect.effect_reorder_diverges`): the footprint reading of Law 9
   ignores the continuation closure.
4. **Closure commute (the sound condition)** — `closure(e₁) ∩ closure(e₂) = ∅ ⇒ apply(e₁; e₂) ≡
   apply(e₂; e₁)` (`Rchain.Effect.effect_commute_of_disjoint_closure`). Not statically decidable, so no
   static sharded scheduler is sound — the claim queue enforces the achievable special case (per-channel
   order) dynamically instead.
5. **Gate refinement** — running effect `i` only after effects `0..i−1` complete refines the
   sequential `apply` fold, and the linear chain of awaits that implements it is *transitively*
   complete (`Scheduler.lean`, `gate_await_closure_orders`); the one-hop (next-step footprint)
   variant is unsound (`one_hop_depth2_diverges`).
6. **Path-ordered commit** — a claim queue's commit sequence per channel follows the path-sorted
   claim order (`Scheduler.lean`, `queue_commit_path_ordered`), and a sorted queue's head is its
   path-smallest element (`pathSorted_head_minimal` — the bakery argument's core; the *global*
   liveness statement needs the finiteness the real system has, since `PathLt` is not well-founded
   on paths).
7. **Replay determinism** — recomputed COMM ⊆ recorded trace (Law 11), so concurrent re-validation
   reaches the recorded root.

### Formalization plan (`spec/Rchain/`)

- Already in `Rho.lean`: `StrCong` (`comm`/`assoc`/`ident`/`par`) and `Reduce` (`comm`/`parLeft`/
  `parRight`) — the *permission* for concurrent reduction.
- Already in `Random.lean` (axiom): the associative splittable RNG merge (Law 19).
- **Done** (`Concurrent.lean`): a parallel-step relation `⟹` (`ParStep`), the independent-redex commute
  (`parStep_comm`), linearization of `⟹` to `⟶`-sequences (`parStep_to_reduce`), the field-wise
  decomposition + inertness lemmas, `reduce_redex_unique` (an isolated redex is deterministic up to
  `StrCong`), and the counterexample `reduce_not_deterministic` showing the flat `Par` is not confluent.
- **Done** (`Tree.lean`): the **tree model** — `Proc` with explicit (injective) `par` nodes, `ReduceT`/
  `StrCongT`, and `reduceT_confluent` (the diamond holds up to `StrCongT`). `flatten : Proc → Par`
  bridges the two (`flatten_reduce`/`flatten_strCong`): tree confluence is a sound refinement of the
  flat `Reduce`, whose non-confluence is precisely the loss of tree structure under `parMerge`.
- **Done** (`Effect.lean`): the **effect-level model** — `Effect` (produce/consume with continuation),
  `State`, `apply`, `footprint`, `closure`. Proves the naive "disjoint footprint" lift to the tuple
  space is **unsound** (`effect_reorder_diverges`) and states the sound condition
  (`effect_commute_of_disjoint_closure`: disjoint *closure* ⇒ commute). This is what rules out the
  *static* channel-sharded effect scheduler.
- **Done** (`Scheduler.lean`): the **channel scheduler** — `DfsPath`/`PathLt` (lexicographic = DFS
  order), the claim queue with `queue_commit_path_ordered` and `pathSorted_head_minimal` (both
  proven), the gate with `gate_await_closure_orders` (proven) and the one-hop counterexample
  `one_hop_depth2_diverges` (proven), and `depth2_next_step_disjoint` for Law 22 — the half of it
  that has content, since computability at dispatch is a fact about `resolve_children`'s signature.

> **Formal.** The full law set is <!-- counts:laws-entries -->91 laws and 131 entries<!-- counts:end --> — [The laws](laws.md)
> is its reader-facing rendering and `spec/LAWS.md` the checked one; per-law source-of-truth pointers are
> in [`spec/INVENTORY.md`](../../../spec/INVENTORY.md), and the machine realization of each law is
> [The laws → Rust code](../contributor/laws-to-rust.md). This page is about the laws that govern
> concurrency — 2, 4, 7–11 and 19 — and the scheduler rows 20–25 that make them precise.

---

## Structural congruence and reduction

Two relations define the *dynamics* of the ρ-calculus: **structural congruence** `≡` (which names
terms that are the same up to reordering and identity) and **reduction** `⟶` (which names the single
step of computation). Both are defined in [`spec/Rchain/Rho.lean`](../../../spec/Rchain/Rho.lean).

### Structural congruence `≡` (Law 2, core)

`≡` is the smallest equivalence relation closed under:

```
refl   : p ≡ p
symm   : p ≡ q  →  q ≡ p
trans  : p ≡ q  →  q ≡ r  →  p ≡ r
comm   : p | q  ≡  q | p
assoc  : (p|q)|r  ≡  p|(q|r)
ident  : p | Nil  ≡  p
par    : p ≡ p'  →  q ≡ q'  →  p|q ≡ p'|q'
```

In Lean these are the constructors of `StrCong`; the equivalence theorem `strCong_equivalence` is
proven, as are `strCong_comm`, `strCong_assoc`, `strCong_ident`, and `strCong_nil_left`. This is the
"par order + `| Nil` + associativity + congruence" fragment of Law 2. The *full* Law 2 (deep
α-equivalence plus `@`/`*`) is **not a second obligation**: with de Bruijn *levels* the representation is
canonical, so deep α is equality, and what is left is this structural congruence. `spec/coq/Laws.v`'s
`alpha_equiv` is an `Inductive` mirroring `StrCong` — refl/symm/trans are constructors, not axioms —
which is the same statement, with a weight invariant and a non-vacuity witness behind it.

The executable form is `name-equivalence.k` (names equivalent up to par order, `| Nil`, top-level
arithmetic, α, and added `@`/`*`).

### Reduction `⟶` (Law 4, core)

`⟶` has one axiom — **COMM** — and two congruence rules:

```
comm     : Name!(x) | for(Name ← …){P}   ⟶   P[subst]        -- a send meets a receive
parLeft  : p ⟶ p'  →  p | q  ⟶  p' | q
parRight : q ⟶ q'  →  p | q  ⟶  p | q'
```

In Lean:

```lean
inductive Reduce : Par → Par → Prop where
  | comm (chan data body : Par) :
      Reduce (parMerge (sendPar chan [data]) (receivePar chan body)) body
  | parLeft {p p' q : Par} : Reduce p p' → Reduce (parMerge p q) (parMerge p' q)
  | parRight {p q q' : Par} : Reduce q q' → Reduce (parMerge p q) (parMerge p q')
```

The `comm` rule says: a send of `data` on `chan` composed with a receive on `chan` with body `body`
reduces to `body` (with `data` substituted for the bound variable — the capture-avoiding substitution
of Law 3). The two `par` rules say reduction happens anywhere inside a parallel composition.

The K executable form is `processes-semantics.k` (`*@P ⇒ P`, `@*C ⇒ C`, parallel spawn) and
`sending-receiving.k` (sends → out-cells, receives → in-cells, paired into a comm event).

### The full Law 4

The *full* Law 4 adds three clauses beyond this core, stated (not yet proven) in
[`spec/Rchain/Reduce.lean`](../../../spec/Rchain/Reduce.lean):

- **Determinism (first-match-wins)** — *withdrawn*: the flat `Par` is **not** single-step deterministic
  up to `≡` (`Rchain.Concurrent.reduce_not_deterministic`). What holds is that an *isolated* redex
  reduces uniquely up to `≡` (`Rchain.Concurrent.reduce_redex_unique`), and full confluence is recovered
  only in the tree model (`Rchain.Tree.reduceT_confluent`). In the node, determinism is supplied by the
  **chosen schedule** (DFS + content-sorted first-match-wins, Laws 1/4/8), not by the raw relation.
- **`new` freshness** — `reduce_freeVars_subset`: reduction never introduces a free variable
  (`freeVars q ⊆ freeVars p`).
- **Replication** — `!P` re-inserts the redex after a comm, and persistent send/receive (`!!`/`<=`)
  are matched without being absorbed (the K rule `persistent-sending-receiving.k`).

### What these guarantee

`⟶` is **not** single-step deterministic up to `≡`: the flat `Par` is not confluent (a term with one
receive and two sends on one channel is a redex in two ways — see
`Rchain.Concurrent.reduce_not_deterministic`). What *does* hold is that an isolated redex reduces uniquely
up to `≡` (`Rchain.Concurrent.reduce_redex_unique`), and confluence is recovered only in the tree model
(`Rchain.Tree.reduceT_confluent`). Determinism in the node is therefore a property of the **chosen
schedule** — the sequential reducer's canonical order (DFS + content-sorted first-match-wins, Laws 1/4/8),
which the concurrent scheduler linearizes to — not of the raw relation. That is the property a
blockchain's consensus depends on: every node computes the same state transition from the same deploy.

> Next: reducing independent sub-processes simultaneously — [Concurrent reduction](concurrency.md).

---

## Concurrent reduction

[Structural congruence and reduction](concurrency.md) defined the *sequential* dynamics of the
ρ-calculus: one `⟶` step contracts a single COMM redex, and reduction is a congruence under `|`. This
document extends that to the *concurrent* dynamics.

The ρ-calculus is a **concurrent** calculus: `|` is parallel composition, and the laws already grant
the permission to reduce independent sub-processes simultaneously. The node's reducer
(`rholang::reduce::DebruijnInterpreter`) exercises the Level-1 permission — it resolves a `Par`'s
*pure* sub-terms (substitution, spatial matching, `new`-allocation) concurrently, then applies the
tuple-space effects in DFS order — plus the effect-level modes of Laws 20–22 (`gate` and `relaxed`,
see [The channel scheduler](scheduling.md)). *Static* effect-level partitioning is
**unsound** — see [Effect scheduling](scheduling.md) S.3 — so the reducer deliberately does
not attempt it. This document is the specification of that concurrent execution model, **founded in
the law set** ([`spec/INVENTORY.md`](../../../spec/INVENTORY.md)).

Throughout, "sequential" and "concurrent" are about the *scheduler*: both reduce the same `⟶` relation
and must land on the same canonical state. The difference is whether independent redexes fire one-at-a-time
or simultaneously.

### The concurrency contract in one line

> **Reduction is permitted everywhere inside `|` (`parLeft`/`parRight`), and an *isolated* redex is
> unique up to `≡` (`reduce_redex_unique`) — but the flat calculus is *not* confluent, and the model
> proves it** (`reduce_not_deterministic`: one term reduces, via two different decompositions, to two
> non-`≡` terms). The determinism a node needs therefore does **not** come from the calculus: it comes
> from content-addressed candidate selection (law 8) and the scheduler (laws 20–25). Full confluence
> *is* a property of the tree model (`Tree.lean`'s `reduceT_confluent`) — the flat `Par` is that
> model's field-wise quotient, and the two are not interchangeable for confluence.

Everything below is a spelling-out of that sentence: which laws *grant* the permission, which laws
*fix* the result, and the theorems that say the two are compatible.

---

### A. Concurrency profile of the laws

Each law is classified by its role for concurrent reduction. The citation is the Rust realization that
currently carries it (from [`contributor/laws-to-rust.md`](../contributor/laws-to-rust.md)).

#### The enabling set — "you may parallelize"

| # | Law | What it grants | Rust realization |
|---|-----|----------------|------------------|
| **1** | `Par` commutative; canonicalization idempotent | Sub-process *order* is irrelevant; parallel results canonicalize identically | `Sorted<Par<S>>` (`models/src/sorted.rs`) |
| **2** | α/name equivalence: `\|` associative/commutative with `Nil`, a congruence | Reassociate the flat `Par` into independent work units | `Par<S>` structural equality (`models/src/ast.rs`) |
| **4** (core) | Reduction: `parLeft`/`parRight` | Reduction happens *anywhere inside* `\|` | `Reduce` in `spec/Rchain/Rho.lean` |
| **7** | Join commutativity (channel keys hashed in sorted order) | A multi-channel receive fires independent of message arrival order | `rspace::hashing::StableHashProvider::hash_seq` |
| **9** | Merge is a monoid; non-conflicting logs commute | Effects of *non-conflicting* COMMs compose order-independently | `rspace::merger::StateChange`/`ChannelChange` |
| **19** | `Blake2b512Random` associative splittable merge | Parallel branches carry independent RNG streams that merge deterministically | `crypto::hash::Blake2b512Random` |

#### The constraining set — "the result must be deterministic"

| # | Law | What it fixes | Rust realization |
|---|-----|---------------|------------------|
| **1** (tie-break) | canonical total order | *Which* candidate is "first" when several match | `Sorted<Par>` + `space_matcher.rs` |
| **4** (full) | `Reduce`'s COMM rule, first-match-wins; `reduce_redex_unique` (**proven**) — and `reduce_not_deterministic` (**proven**), the counterexample that says the flat `Par` is *not* confluent | An isolated redex produces one thing; *which* redex fires is the scheduler's business, and the flat calculus does not fix it | `Rchain/Rho.lean` (rule), `Rchain/Concurrent.lean` (theorems), `Rchain/Reduce.lean` `reduce_freeVars_subset` (**proven**) |
| **8** | Deterministic COMM (produce refs sorted; content-addressed events) | Reproducible candidate selection | `rspace::space_matcher` first-match-in-insertion-order |
| **11** | Replay determinism (recomputed COMM ⊆ recorded trace) | A re-execution — concurrent or not — reproduces the recorded trace | `rspace::ReplayRSpace` |
| **12** | Actor atomicity (single-threaded `mbox.nextMsg`) | One actor/message at a time; the *analog* here is per-channel serialization | (orphaned; carried by `TwoStepLock`) |
| **17** | Merge determinism (unique min-cost rejection); RNG merge commutative | Conflicting merges resolve to a unique winner | `NonNegI64` + `Blake2b512Random::merge` |
| **20** | Channel-task linearization (claim queue, per-channel DFS commit order) | Same-channel commits are path-ordered; a claim commits only as head of all its channels | `rspace::concurrent::channel_queue::ChannelClaimQueue` |
| **21** | DFS-gate linearization (`gate_exec_refines_apply`) | Effect `i` waits on `0..i−1` — the sequential fold, deterministically | `rholang::scheduler::EffectMode::Gate` |
| **22** | Next-step closure computable at dispatch | The matched datum is concrete, so the continuation's first-step footprint is computed at dispatch (`resolve_children`) | `rholang::reduce` |

#### The supporting set — "neutral, but load-bearing"

- **Law 3** — `sort(subst t) = subst(sort t)`: substitution commutes with canonical order, so a branch
  may substitute into an already-sorted subterm without re-sorting.
- **Law 5** — a pattern binds each free level at most once (`spatialMatch_implies_linear`, **proven** of
  a matcher that is *defined* — the axiom this page used to name, `BindsAtMostOnce`, was false as
  written, AUDIT C26): a pattern's substitution is well-defined regardless of the order
  parallel-matched sub-patterns bind variables.
- **Law 6** — no globally free variables (`Closed`): preserved under `|`, `≡`, and `⟶`, so splitting a
  closed program into branches and rejoining keeps everything total.
- **Law 10** — Merkle determinism: the content-addressed trie root *is* the state, so two merges of the
  same deltas converge on the same root.
- **Laws 14–16, 18** — consensus/storage invariants; they constrain the *block* layer, not the reducer.

---

### B. Formal definitions

The grammar and `≡` are those of the structural-congruence section above. We add:

**Channel footprint.** `chans(P)` is the finite set of channels that `P` produces on or consumes from
(the *sources* of its top-level sends and receives). For a `Par` this is the union over its terms.

**Independence.** Two processes are independent when they do not contend for any channel:

```
indep(P, Q)  ⇔  chans(P) ∩ chans(Q) = ∅
```

**Parallel step `⟹`.** A single concurrent step reduces a set of pairwise-independent redexes at once:

```
P ⟹ Q   when   P ≡ R₁ | … | Rₖ | S
               each Rᵢ ⟶ Rᵢ'   (a redex contraction)
               pairwise indep(Rᵢ, Rⱼ)  for i ≠ j
               Q ≡ R₁' | … | Rₖ' | S
```

The sequential step `⟶` is the special case `k = 1`. `⟹` subsumes `⟶`.

**Schedule.** A finite sequence of parallel steps `P = P₀ ⟹ P₁ ⟹ … ⟹ Pₙ`. The sequential reducer is the
schedule that always takes `k = 1` with the redex chosen in canonical order.

---

### C. Soundness theorems

These are the statements that make "linear scaling" *sound* — that concurrency is a scheduling freedom,
not a semantics change. Statements marked **proven** are already discharged in
[`spec/Rchain/Rho.lean`](../../../spec/Rchain/Rho.lean); **stated** are the obligations this document
adds (to be formalized as `Rchain/Concurrent.lean`).

#### C.1 Independent-redex commute (**proven** — `parStep_comm`)

```
Reduce p p'  ∧  Reduce q q'   ⇒   Reduce (p'|q) (p'|q')  ∧  Reduce (p|q') (p'|q')
```

*Proof sketch.* A redex on the left and a redex on the right of `|` have disjoint channel footprints,
so the two contractions neither read nor write a channel the other touches; they commute. `≡` (Law 2)
reassociates the resulting `Par`s.

**The diamond does *not* hold on the flat `Par`.** The general statement `P ⟹ Q₁ ∧ P ⟹ Q₂ ⇒ Q₁ ≡ Q₂` is
**false**: a term with one receive and two sends on the same channel is a redex in two ways, reducing to
two inert, non-`≡` send-only terms (see `spec/Rchain/Concurrent.lean`, `reduce_not_deterministic`).
Confluence is a property of the *tree* model (explicit `par` nodes), not of the field-wise flat `Par`.
Determinism is instead a property of the **chosen schedule** (the sequential reducer's canonical order,
Law 1/4/8) — the "same normal form" invariant below holds for *that* schedule, not for arbitrary `⟹`.

#### C.2 Linearization (**stated**)

The sequential reducer — one redex at a time, in canonical order (Law 1), first-match-wins (Law 4/8) —
is a valid refinement of `⟹`: for every sequential run there is a schedule of `⟹` steps with the same
normal form, and vice versa (every `⟹` schedule linearizes to some sequential run reaching a `≡`-equal
state). This is what makes the sequential `eval` loop a *correct* scheduler, and what any (future)
concurrent scheduler must preserve.

#### C.3 Commutative merge (**proved**, the Law 9/17 pair)

The *effect* of a set of COMM events on the tuple space is a commutative monoid over non-conflicting
events; conflicting events resolve to a unique min-cost winner (Law 17). Hence parallel branches may
apply their effects in any order and merge to the same state. Realized by the `StateChange` monoid +
`compute_trie_actions` in `rspace/src/merger/`, and the sorted distinct-branch RNG merge in
`rholang/src/merging.rs`.

#### C.4 RNG determinism (**stated**, Law 19)

Splitting a seed by index (`split_byte`/`split_short`) then merging branch RNGs **associatively and
sorted** is schedule-independent. Consequently `new`-name freshness does not depend on the interleaving
of parallel branches. The merge associativity/commutativity is already an **axiom by design** in
[`spec/Rchain/Crypto/Random.lean`](../../../spec/Rchain/Crypto/Random.lean).

---

### D. Realization map — theorem → mechanism

| Theorem | Mechanism carrying it (today, reused) | Location |
|---------|----------------------------------------|----------|
| C.1 Diamond | `≡` reassociation (`StrCong`) + per-channel atomicity | `spec/Rchain/Rho.lean`; `rspace/src/concurrent/{multi_lock,two_step_lock}.rs` |
| C.2 Linearization | canonical order `Sorted<Par>` + sorted-first candidate selection | `models/src/sorted.rs`; `rspace/src/space_matcher.rs` |
| C.3 Commutative merge | `StateChange`/`ChannelChange` monoid + `compute_trie_actions` | `rspace/src/merger/*` |
| C.4 RNG determinism | `Blake2b512Random::{split_byte,split_short,merge}` | `crypto/src/hash/blake2b512_random.rs` |
| C.5 Gate refinement | `EffectMode::Gate` (effect `i` after `0..i−1`) | `rholang/src/scheduler.rs`; `rholang/src/reduce.rs` |
| C.6 Path-ordered commit | `ChannelClaimQueue` + the phase-one/two produce split | `rspace/src/concurrent/channel_queue.rs`; `rspace/src/scheduled_space.rs` |

What is *not* present is a **static** effect-level partition — and, per
[Effect scheduling](scheduling.md) S.3/S.4, a sound one does not exist at the effect level (the
sound condition is disjoint *closure*, which is not statically decidable). The sound effect-level
mechanisms are dynamic instead: the DFS gate (C.5) and the per-channel claim queue (C.6), whose
**relaxed** mode trades cross-channel interleaving freedom for a non-sequential event log and is
therefore off-chain only (the casper block paths hard-reject it). On-chain, the reducer's parallelism
is Level 1 (pure resolution) plus the sequential-equivalent gate.

---

### Scope and deferrals

This document specifies **within-deploy** concurrency: independent sub-processes of a *single* program
(`|`-composition) reducing concurrently, synchronizing only at the tuple space. It deliberately does
**not** cover:

- **Deploy-level parallelism** — deploys in a block share the whole tuple space and are *not* statically
  independent; RChain's own model treats intra-block deploys as ordered/dependent
  (`casper/src/merging.rs`), so parallelizing them changes block semantics.
- **Sharding / namespace partitioning** ("nth sharding") — a separate horizontal-scaling design, not
  present anywhere in the current tree.
- **Actor runtime** (Laws 12–13) — the Rosette VM is orphaned in this port; its fork-join barrier is
  *prior art* for the scheduler's rejoin discipline, not a component to rebuild.

> Next: the effect level of the scheduler — [Effect scheduling](scheduling.md).
