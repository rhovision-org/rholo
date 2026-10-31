# Rust vs. Scala — how the rewrite made the fragile explicit

This page records how porting the RChain node from Scala/JVM (+ the C++ Rosette VM) to Rust
changed *how we reason about* the code's fragile patterns, bugs, and exploits — and why the Rust
node can now **surpass** the Scala original for production readiness, on top of the JVM's garbage
collection and memory problems.

It is the companion to [`AUDIT.md`](AUDIT.md) (the findings check-off) and
[`TYPE-SYSTEM.md`](TYPE-SYSTEM.md) (the type discipline). Where Scala and the specification
disagree, the specification is the oracle; the Scala code is reference material whose latent bugs are
**documented, not reproduced**.

---

## 1. The Scala fragility catalog (concrete, caught in the port)

The Scala node was not merely GC- and heap-bound — it was **notoriously fragile** at the exact
boundaries where correctness matters. The port caught each of these as a *type error* or a *code
review finding*, rather than as a runtime incident:

| # | Scala behavior | Why it is a bug / exploit | How Rust makes it impossible or explicit |
|---|---|---|---|
| 1 | `Costs.toProto` = `PCost(c.value)` — a negative `Long` gas cost wraps into a `uint64` | Over-charging a deploy wraps its cost to a huge unsigned value, corrupting accounting | Negative cost is **rejected** at the boundary (`casper/src/runtime_manager.rs`), not wrapped |
| 2 | Super-majority computed as `stake.toDouble / totalStake > 2d/3` | `f64` loses precision for stakes ≥ 2⁵³; two sides of a fork can disagree on a finality vote | Exact integer `3·stake > 2·total` in `i128` (`sdk/src/consensus.rs`) |
| 3 | `spatial_match_fn(…).ok()?.next()` — a `RholangError` swallowed as "no match" | A Law-5 `BugFoundError` is silently treated as a non-match, corrupting reduction | The error is recorded and propagated; matching is total in `Result` |
| 4 | `getUnsafe` / `.get(...).get` / `unwrap_or(0)` on a negative gas cost | Silent partiality: a missing key or negative value becomes `0`/`None` and the node keeps running on corrupt data | Refinement newtypes (`NonNegI64`, `BlockHeight`, `SeqNum`, `Port`, `WireLen`, `Hash32`) carry the invariant *structurally*; no `Deref`, no public `.0` |
| 5 | `maxMessageSize - 2048` in the chunker | Underflows (wraps) when the max size is small, disabling the size guard | `checked_sub` returns `Err` on a too-small max |
| 6 | Radix-tree node as `Vec[Item]` with `NUM_ITEMS = 256` | The "exactly 256 slots" invariant is implicit; a short/corrupt node panics on indexing | `[Item; 256]` fixed array — the invariant is the type |
| 7 | Exceptions as control flow (`throw`/`catch`, `???`, `NotImplementedError`, `BugFoundError`) | A `???` stub or a `BugFoundError` thrown deep in a `Future` aborts a task silently | `Result`/`Option` everywhere; `todo!()`/`unimplemented!()` are compile-time-visible and gated by the audit script |
| 8 | `HashMap` iteration order feeding `New.injections` / matcher state | Non-deterministic ordering → two nodes reach different state hashes → consensus fork | `BTreeMap`/`BTreeSet` (sorted) and the `Sorted<Par>` refinement (canonical by construction) |

The "fragility" was not incidental: the JVM hid these behind `null`, unchecked casts, boxed
primitives, and catch-all `Try`/`Either` recovery. The Scala node ran *despite* them — the Rust node
refuses to compile *until* they are made explicit.

---

## 2. How Rust's model enables the reasoning

The port does not just *translate* the Scala; it re-expresses each invariant so that the compiler
enforces it:

- **Ownership & the borrow checker** eliminate the aliasing races that made Scala's shared mutable
  `Ref`/`var` state (e.g. the global `connections` write-lock held across I/O, the `BlockRetriever`
  map) hard to audit. Rust's `Mutex`/`RwLock`/`Arc` make *who may mutate what, when* explicit.
- **`enum` + exhaustive `match`** replace null/partial-functions with closed sum types. A
  `NotImplementedError` in Scala becomes a `Result` arm in Rust — the compiler forces you to handle
  the failure case.
- **`Result`/`Option`** replace exceptions and `null`; every partial boundary is a type. The audit
  script (`tools/audit-type-system.sh`) then machine-gates the remaining `unwrap`/`expect`/`panic!`/
  `unsafe`/`assert!` sites.
- **Refinement newtypes** (`NonNegI64`, `BlockHeight`, `SeqNum`, `Port`, `WireLen`, `Hash32`)
  carry domain invariants in the type, so "is this stake negative?" or "is this height
  `-1`?" is not a runtime question — it cannot be represented.
- **`Send`/`Sync`** make concurrency safety a compile-time property, not a code-review convention.
- **Zero `unsafe`** across the crate graph — the entire node is safe Rust, so the class of memory
  bugs Scala/Rosette could hit (use-after-free in the C++ VM, JNI boundary errors) is absent by
  construction.
- **Deterministic collections** (`BTreeMap`/`BTreeSet`, explicit sorts) remove hash-iteration
  non-determinism, which is load-bearing for a consensus node.

---

## 3. Surpassing Scala for production readiness

Beyond the memory-safety argument, the Rust node is *more* production-ready than the Scala one in
concrete, auditable ways:

1. **No collector to pause it / no JVM heap sizing.** Scala boxed every `Par`/`Expr` node and every
   event in the hot reduction path; long-running nodes suffered stop-the-world pauses and heap
   pressure. The Rust binary ships no tracing collector, so no collector pauses it and there is no
   `-Xmx` to size. Value semantics and explicit allocation give predictable latency — but not a
   *bounded* footprint: memory that leaks or grows is safe code in any language, and this node
   measures its own residency and records the rate
   ([hardware requirements](../docs/src/node/validator-requirements.md)).
2. **No `Vec::with_capacity(attacker_count)` OOM.** The Scala scodec decoders (and the naive Rust
   port of them) trusted 32/64-bit length prefixes from the wire; a malicious length could allocate
   gigabytes. Rust made these *visible* as `with_capacity`/`try_into` sites, so they could be audited
   and bounded (see `AUDIT.md` C2/C3 and the scodec findings).
3. **The defensive wins are only expressible in a safe language** — semaphore-bounded dispatch,
   per-peer rate limits, a content-addressed Merkle radix tree, mutual-TLS identity pinning, and an
   exact integer consensus — and are now *structural*, not advisory.
4. **The port actively fixed Scala bugs** rather than reproducing them: negative cost, f64
   finality, the swallowed matcher error, the underflowing chunker, the `Vec` radix node, and —
   through this remediation — the unenforced `phlo_limit`, the equivocation/failed-block liveness
   gaps, and the remotely-triggerable panics that were faithful Scala behavior.
5. **The port adds a bound the Scala reference does not have, as a deliberate divergence.** The Scala
   `blockSummary` composes no per-block limit at all: the per-deploy phlo budget bounds each deploy
   *separately*, and nothing bounds the block, so a proposer could make one block arbitrarily expensive
   to replay on every validator. The receiving side now enforces two bounds — a deploy-count cap
   (`validate::deploy_count`, mirroring the proposer's own `MAX_BLOCK_DEPLOYS`, which the proposer had
   always applied to itself and the validator never applied to a peer) and a per-block phlo cap
   (`validate::block_phlo`, summed over the **signed** `phlo_limit` of the block's deploys rather than
   the proposer-supplied `cost` field, which replay recomputes and which a proposer would therefore be
   setting for itself). Both are protocol constants rather than config values: two operators running
   different values would disagree about which blocks are valid, which is a fork — the same reason
   `MAX_BLOCK_DEPLOYS` is a constant. Pre-testnet, so the hard fork costs nothing today.
   Rationale and measurement: [`docs/src/node/security-audit.md`](../docs/src/node/security-audit.md) §3.
6. **The set/map deduplication is ordered rather than scanned.** `par_set`/`par_map` deduplicated with a
   linear scan ahead of an already-Θ(N log N) sort, making every `Set`/`Map` operation Θ(N²) — and
   because a Set operation charges *flat* phlo, a 229 KB deploy bought roughly 98 CPU-seconds for 13
   phlo. Both now sort by `Par`'s total order and dedup the adjacent runs, which is byte-for-byte the
   same output at Θ(N log N). See `models/src/sorter.rs`.
7. **The DAG writes a block's fringe record before its metadata, where the Scala writes metadata
   first.** `BlockDagKeyValueStorage.insert` records the fringe data — the thing a block *refers to* —
   before `block_metadata_store.add`, which is the *pointer*: metadata is what makes a block known, so
   `contains` short-circuits a re-insert and the receiver drops a re-received known block. With the
   Scala's order a crash between the two writes left a block present with no fringe record, and
   nothing rebuilds one — `create`'s fold skips a missing entry in silence while its three sibling arms
   fail closed, and `get_pre_state_for_parents` then refuses every block for which the torn one is the
   max-fringe parent. On a single-validator node that was permanent and silent, recoverable only by
   deleting the shard data dir so `dag_set` empties and `NodeSyncing` runs. Data-then-pointer is the
   order `rspace/src/history/roots_store.rs` already uses; this brings the DAG side into line with it.
   **The Scala's own order is the deviation** (`BlockDagKeyValueStorage.scala:54` vs `:83`), so the
   reorder is the port's, and it is registered here rather than in a findings row. Found by the
   September 2026 audit (F-5), which demonstrated it with four tests over a reconstructed store.

   *Provenance note:* this change is recorded here rather than in its own commit message because a
   concurrent session's `git commit` swept the staged files into an unrelated commit
   (`19259c733`, "Depth.lean's own notes stop saying clause b is owed"). The code and its tests are
   correct in the tree; only the message that should have carried this reasoning was lost, so it is
   written down where the divergence already belongs.
8. **The block `version` is now checked on the acceptance path, where the Scala never checked it
   either.** `validate::version` (`SUPPORTED = [1]`) has existed since the port with a unit test and
   **no production caller**: the acceptance path read the block's hash, signature, shard and deploy
   data, and never the field naming the protocol it is written against. The Scala is the same — its
   `BlockReceiver` carries `// TODO: check valid version` in the same conjunction, and its
   `Validate.version` likewise has no caller — so this is a deliberate divergence rather than a
   fidelity fix: the reference accepts a `version: 999` block, this port now refuses one. It matters
   because the field is inside `hash_block`'s cover, so a future version bump would otherwise have
   been enforced by nothing, and two nodes disagreeing about which versions they accept is exactly
   the divergence the field exists to prevent. Found by the September 2026 audit (F-6).

   **Still open from the same finding:** `timestamp` is likewise inside the hash and read by no rule,
   and it is exposed to contracts on `rho:block:data`. It is not fixed here. A correct bound is a
   policy choice — a `now ± slack` window imports the node's clock into consensus, which this node's
   own audit flags elsewhere, while a monotonic "not before your parents" floor is skew-independent
   but needs a parent-block read the store API does not make obvious. Left for a decision rather than
   guessed at; the audit rated it P3 for the chain as it stands, since no default-genesis contract
   consumes the channel today.
9. **A `rho:gov:*` call is bounded and charged; the Scala's is neither.** The four governance
   handlers folded over the union of their arguments with **no charge at all** — neither the storage
   path (which charges for what moves, and their output is proportional to their input, so it cannot
   see a cubic fold) nor the reducer. `censure` was measured at 10.73 s for a universe of 8000, and
   reachable from any deploy. Two changes, both deliberate divergences: a member bound
   (`MAX_GOV_UNIVERSE`, 512, set from a measurement and not from a preference) and a charge
   proportional to the squared universe. **The bound came down from 4096 after measuring** — an
   uncapped 4097-member fold takes 14.2 s, and the fold alone is ~0.36 s at 512, which is the largest
   round size comfortably sub-second. Recorded here because a call the reference accepts is now
   refused, and because a call that used to fit under its phlo limit may now run out.

   **The charge was a square for cubic work, until the October re-audit's C346 (2026-10-30).**
   `censure` iterates a decreasing fixed point — at most `U + 1` rounds — and each round asks, of
   every member, how many censurers clear a level over the whole universe, so the work is Θ(U³) and
   the charge is now `(U+1)³`. The other three handlers fold in one pass and keep the squared charge;
   their work shape is unmeasured, and that is the row's recorded residual rather than a claim they
   are quadratic. **The bound stayed at 512 because the cubic path was then measured at it** — 33 ms
   for a 512-member levels map and 1–5 ms for the three mixed shapes, in release
   (`rholang/tests/gas_amplification.rs`; evidence in
   `spec/audit/evidence/c345-c352-remediation-20261030/`) — which is the measurement the previous
   paragraph said was outstanding. A `censure` at the bound now costs `(512+1)³` = 1.35e8 phlo, the
   same order as the time it buys under the table's one-phlo-per-operation calibration.

10. **`--` and `.diff` are charged for the product of their operands, not one of them.** The Scala
   charges `3 · |right|` while the work is `left.iter().filter(|p| !right.contains(p))` — a linear
   scan per element, so `|left| · |right|`. Charging one factor for a product is the sublinear-charge
   shape that lets a phlo buy unbounded work, and it is the shape that made the per-block phlo cap
   inoperative: a cap bounds work only when the charge tracks it. `indexOf` and `contains` are the
   same defect one table entry over — `n + m` charged for a scan whose worst case is `n · m` — and
   are charged the product too; `startsWith`/`endsWith` really are `n + m` and keep that charge.

11. **`replace` is charged for the string it is about to build, not for its inputs.** `input + old +
   new` bought `input + k·(new − old)` output, with `k` up to `input / old`, so a short needle and a
   long replacement amplified without bound — 100 KB × 100 KB is a 10 GB allocation. The charge is
   the output bound, computed in constant time and taken *before* the allocation, so the phlo budget
   is what stops it. Charged rather than refused, which is the least invasive form the fix could
   take: no new constant, and no term the reference accepts is now rejected.

   **One thing this pass reverted rather than kept.** The plan called for a per-byte charge on the
   crypto builtins, on the grounds that a 16 MiB hash cost a flat charge. Implementing it made the
   gap visible: those bytes have to be *produced or consumed through the storage path*, which charges
   proportionally to their size, so the hash reads bytes that were already paid for and a flat charge
   buys no unbounded work. A test was written for it and refused the term with the charge reverted —
   for the parse, not the hash — which is a test that would have gone on passing if the charge were
   deleted. It is not in the tree, and neither is the charge. A finding that does not survive being
   implemented is the finding that was wrong.

12. **The active validator set is drawn, not ranked — the reference's own TODO, and the port adds the
   entropy the reference does not have.** `Pos.rhox:718-726`'s `pickActiveValidators` takes the first
   `$$numberOfActiveValidators$$` entries of the bonds map in *key* order and carries the comment
   `// TODO: Randomly select 100 active validators once we have on-chain randomness`. The Scala
   reference selects the highest-staked; the port now draws **in proportion to stake**, without
   replacement, from the eligible pool (`rholang/src/native_state.rs`'s `select_active`), seeded by a
   `pos:epoch_seed` leaf written **one boundary ahead** from the state hash of the last **finalised
   fringe**, so the block that draws is not the block that chose the entropy — and the entropy itself is
   the >2/3-agreed frontier rather than anything one proposer reaches. The divergence is the reference's
   stated intent, and the part the reference lacks is the seed: the port's `BlockRandomSeed` was
   `hash(shard_id, block_number, sender, pre_state_hash)` computed *at the moment of use*, and all four
   inputs but the shard id are proposer-chosen — so the pre-change rule was a free, unbounded reroll by
   the one party that also chose the sample frame. The register row `spec/audit/passes.md:327` states
   the *reason* the cap exists (finality's supermajority is stake-weighted, so membership decides who
   can finalise) and is kept, rewritten rather than deleted.

   **The rule was uniform until 2026-10-02, and weighting is what closed the exposure below.** A uniform
   draw is a per-*key* rule: the first slot went to a dust validator as readily as to the largest one,
   so splitting a stake across keys bought slots (and income, since only drawn members are paid). The
   weighted rule gives the first slot to a validator with probability exactly `stake / total`, and the
   measurement that pins it is `the_first_slot_is_awarded_in_proportion_to_stake` (a three-to-one stake
   takes the slot `0.75` of the time over 512 fixed seeds; a uniform draw gives `0.51`) and
   `splitting_a_stake_across_keys_does_not_buy_slots`. **The uniform rule was the one decision in this
   item worth revisiting**, as this item used to say, and it has now been revisited.

   **Residuals, named rather than implied.**
   - **O1 — the seed-setter's influence, and where the line now falls.** The anchor is the **last
     finalised fringe's state hash**, and a lone proposer does not move the fringe: it is computed from
     the parents' seen-sets (`message_map::latest_fringe` plus `MergeScope::merge`) and is what >2/3 of
     the active set has agreed. What a proposer *can* still do is present a **stale** fringe — nothing
     requires a block to justify everything it has seen (`validate::check_justification_regression`
     forbids going *backwards* on the messages a block carries, not omitting them) — so the steering
     space is the number of *distinct fringes* its feasible candidates induce, which is normally exactly
     one. **The first version of this writer anchored on the writing block's pre-state and had the hole
     in full**: a pre-state is one per justification subset, so the writer could enumerate the seeds,
     evaluate the draws offline, and publish the block whose draw it liked. That is the measured
     difference between the two anchors, and it is why the pre-state is *gone* from the seed rather than
     kept beside the fringe. It can publish one candidate only — a second block at the same height and
     sequence number is refused at insert before any write (`casper/src/dag.rs:243-257`) — and the
     proposer of the *drawing* block has no say at all. A residual remains rather than a proof: closing
     it outright needs the commitment of the deferred commit-reveal or VRF writer, which is what the
     leaf is for. **Nothing about the anchor is published or verified** — each node derives it from its
     own DAG (`pre_state.fringe_state` on the play and validation paths; the block's stored metadata for
     the index and reporting paths) — so play and replay agree by construction rather than by a claim.
     **Measured live**: a 2-validator devnet with `--epoch-length 2` crossed 57 epoch boundaries with
     the anchor in place, every one of them evaluated twice — once by the proposer and once by
     validation — and no block refused. That is the check this design rests on: a `fringe_state_hash`
     that one path derived differently from the other would write a different seed leaf, and the second
     evaluation of the boundary would refuse it.
   - **O2 — capital can pre-position.** The seed is public before the boundary, so a validator can
     bond to enter or stage a withdrawal to leave the pool in time for `B_k`. Neither this design nor
     commit-reveal closes that without an extra rule (a withdrawal delay longer than the
     seed→snapshot window, or an earlier snapshot). This is a design gap, not an implementation one.
   - **O3 — CLOSED 2026-10-02: the selection is stake-weighted.** It read: *uniform selection is
     sybil-sensitive, in the weight set and in the income* — splitting a stake across `k` validators
     yielded roughly `k` times the expected slots of the same stake held whole, while a large honest
     validator was no likelier to be drawn than a dust one, with the cap (default 100) making it a live
     exposure in the finality weight set; and `epoch_rewards` pays only the members that were drawn, so
     stake stopped predicting income. The named drop-in is now the rule: `draw_weighted_without_replacement`
     in `rholang/src/native_state.rs` walks the pool in canonical `BTreeMap` order accumulating weights,
     takes the first entry past a uniform 128-bit draw, removes it and renormalises — exact integers
     throughout, no floats. **A residual remains and is stated rather than implied**: the draw is
     sequential, so the *first* slot is exactly proportional and the later ones are proportional to the
     weights that remain. **The measured direction of the residue is the opposite of what a per-key rule
     gave, and it is worth stating**: `the_cap_regime_no_longer_rewards_a_split_stake` re-measures the
     same pool the book publishes — one stake of 40 held as one key, four keys of 10, or twenty keys of
     2, against six rivals at 10, cap 4 — and reads **0.5328 / 0.4000 / 0.1770** where the uniform rule
     read 0.326 / 0.400 / 0.528. Splitting now *costs* 56 % instead of *earning* 32 %, because the cap
     is fixed and a large key both draws more often and crowds the denominator when it does. Neither
     regime is pro-rata; the cap is what breaks proportionality, and the two rules differ only in which
     side of it a staker lands on. Weighting is the side chosen — the side on which **stake buys
     weight**, which is what the cap exists to bound, with the concentration already the operator's own
     per-validator risk and the bond already bounded by `maximum_bond`. The exact-proportional
     alternative is a systematic (rotated-interval) scheme, which assigns a fixed share rather than a
     random one, and collides for any stake above `1/count` of the total. Falsifiers both ways:
     `the_first_slot_is_awarded_in_proportion_to_stake` (0.75 against a uniform draw's 0.51) and
     `splitting_a_stake_across_keys_does_not_buy_slots`.
   - **O4 — the absolute security budget now fluctuates** epoch to epoch, more so with a cap. It
     should be measured by simulation over many seeds with a stated tolerance rather than asserted.
   - **O5 — a delegation buys draw *weight*, and the direction it buys is concentration** (added
     2026-10-02 with the delegation primitive, #193). The issue that specified the primitive expected
     the opposite and said so in its constraints — that the draw being uniform over *keys* meant "a
     delegation buys a larger share of a drawn key but not draw weight". **That was written before O3
     closed and is no longer true of the tree**: the draw is stake-weighted, and a delegation joins the
     operator's aggregate `pos:bonds` entry, which is exactly what `select_active` weights. So a
     delegator's REV buys weight through the operator's key that it could not buy alone — and the
     measured regime from O3 is what says *which* weight: **0.5328 / 0.4000 / 0.1770** for one key of 40,
     four of 10, twenty of 2 against the cap. Delegating into **one large operator therefore earns above
     pro-rata**, so the primitive gives a small staker access to a concentration it cannot assemble
     alone rather than spreading anything, and no document should describe it as spreading risk. The
     consequence is not a defect in the primitive — it is O3's cap regime, and the primitive makes it
     reachable at a smaller stake — but it is the reason a "pooled staking" story must not be told about
     it, which is the same conclusion `docs/src/node/validator-economics.md`'s open question 1 reached
     before the primitive existed. **Two costs of the ledger are named rather than left to be found**:
     `pos:delegations` is unbounded in the number of delegators per operator (the per-delegation floor,
     reused from `minimum_bond`, is the DoS control — there is no cap on the count, which is a residual
     rather than a decision), and the boundary walk, which was already `O(validators)`, becomes
     `O(validators + delegations)`; the slash fan-out is `O(delegators of the offender)` and is paid only
     on a slash. Both are the linear-in-the-ledger pattern the rest of the epoch already has, so neither
     is a new class — but neither is measured, and a bound that is not measured is a claim rather than a
     bound.

   **What the consumer sweep found, since the rule is read in more places than the five named.**
   Every reader of `pos:active` needed no change — they decode the leaf, compare it to a state, or
   report it — but three of them read the set for a *decision*, and the draw changes what that
   decision covers:
   - `validate::neglected_invalid_block` rejects a block that neglects an invalid justification whose
     sender is in the block's carried `bonds`. With a draw that set is the drawn subset, so a
     *bonded-but-undrawn* validator's invalid block may now be neglected. Semantically right, since
     only drawn validators carry weight, but it is a narrowing of what the rule covers.
   - The proposer's own `check_active_validator` reads the newest block's carried map, so a pool member
     that was not drawn reports `NotBonded` and declines to propose for that epoch. No state change, and
     the correct reading of "active" — but this is the one place where a validator *notices* the draw,
     and **it is where the draw can cost liveness.** Measured twice on devnets, and the second run
     carries its own control.
     *3 validators, `--epoch-length 3 --active-validators 2`*: the chain ran to the boundary at block
     30, the draw left the only validator that could actually propose out of the set, and the chain
     halted — silently, because `NotBonded` had no log at the point of decision (now it has one, at the
     return in `blocks/proposer/proposer.rs`).
     *2 validators, `--epoch-length 2 --active-validators 1`*: the same, with the mechanism visible from
     both nodes — the boundary at block 4 drew `04dbe32c` in and `04f700a4` out; the drawn-*out* node
     logged the refusal above, and the drawn-*in* node logged the same refusal, because its own view of
     the active set was behind. Nothing proposed and the chain halted at block 5. **The control is the
     same chain with the cap removed** (`--active-validators` left at its default, so nothing is
     selected): 27 boundaries, no halt, and the same validator key in the set. So the *anchor* did not
     break the boundary — the cap did, by handing the epoch to a node whose view is stale.
     **The draw did not cause either halt; a pre-existing participation/state-staleness problem did, and
     the draw makes it reachable** — with no cap every bonded validator is always in the set, so there
     is always a proposer. The decision this leaves open, deliberately rather than by omission: the gate is *self-imposed* — a drawn-out validator's block would still be
     accepted by its peers, because no receiving-side rule tests the sender against the active set
     (`bonds_cache` compares the block's carried map to the state, which a non-active sender computes
     honestly) — so `check_active_validator` could ask "am I in the **pool**" instead and remove the
     hazard. That is a proposal-behaviour change, not a reading of the rule, so it is recorded here
     rather than taken.
   - The finaliser's pruned-history fallback (`multi_parent_casper.rs`, the arm that requires the
     justifications' carried maps to agree when the newest justification's state is unreadable) is now
     unsatisfiable across a boundary: the drawn set changes at every boundary even when no stake moves,
     so those maps always disagree there. That path already refuses rather than guessing — the shape of
     #73 — but the draw makes its refusal reachable where it previously was not. It fires only on a node
     whose history is pruned.


   **No law row changes.** Laws 44–47 constrain the epoch's timing, step order, conservation and the
   withdrawal machine; none states a membership predicate, and `Rchain/Pos.lean`'s `reselect` is a
   filter over the pool whose theorems are untouched — verified, not assumed. The model was already
   more abstract than the code, and this widens that gap in degree rather than in kind.

   Rationale, the reference point (`PatrickMockridge/Mudra`'s beacon, re-sourced from this chain's own
   entropy), and the negative results: [`docs/src/node/security-audit.md`](../docs/src/node/security-audit.md) §8.

   **What is deliberately not in this change: the beacon itself.** What remains of O1 is the
   stale-fringe choice — a commit-reveal round or a VRF accumulator removes it, and both change what
   the *writer* is, not what the *rule* is. The selection rule reads the `pos:epoch_seed` leaf and nothing
   else, so replacing the writer touches `close_block` step 5 and no consumer, no law, and no test of
   the draw. That separability is why the seed is a state leaf rather than a value threaded through the
   deploy, and it is the shape the next pass should take.

13. **The collection conversions and unions are charged for what they canonicalise** (the October
   re-audit's C345, 2026-10-30). `toSet`/`toMap` charged **nothing** for a sort, `set` a flat three,
   `keys` a flat ten, and `++`/`.union` charged the **right operand only** — so `bigSet ++ Set()` was
   free while it still rebuilt `bigSet`. Measured in release: `[N ints].toSet()` cost **18 phlo at
   N = 500 and at N = 40 000** while wall time grew from 7 ms to 724 ms. The charge is now
   `canonicalization_cost(n) = 3n`, taken from the operand size **before** the work (the same
   output-bound shape as `replace` in item 11), and a union is charged the sum of both operands, which
   is what makes it symmetric. **The charge is linear for `O(n log n)` work**, and that is stated
   rather than glossed: a phlo buys work bounded by `charge · log charge` — bounded, which is the
   defect answered, but not an exact comparison count. A collection **literal** stays uncharged: its
   bytes were charged at parse and the create path is not attacker-sized. The property is **Law 91a**
   (`spec/Rchain/Cost.lean`), with `rightOnlyUnionCost_is_not_symmetric` keeping the old formula so the
   symmetry statement has a model that fails it.

The honest caveat is in §5: the port is not yet *done* surpassing Scala. Several Scala behaviors were
initially carried over faithfully (the "deferred" surface, the panic-vs-exception sites) precisely
because they were faithful — and the remediation plan exists to convert those into Rust-strength
invariants.

---

## 4. What still lags (honest)

- **Formalization**: nothing here any more — law 1b's row reads "**no axioms, from twelve — the
  residual is empty**" (the list comparators' laws by induction on the list, the element laws as
  theorems in dependency order), and `Rchain/Sort.lean` declares no `axiom` at all. This bullet used to
  say "the thirty element-comparator axioms … remain to discharge"; it was stale by the time a reader met it
  and is corrected here rather than deleted, so the record shows what was discharged (AUDIT C72).
- **Native PoS lifecycle**: the dynamic-validator lifecycle is implemented natively — trusted
  stakeholder admission (`trust`/`untrust`), minimum/maximum-bond validation, pool updates with a
  top-N active cap applied **at epoch boundaries**, the epoch reward split and its committed-rewards
  map, staged withdrawals paid out of the staking vault after quarantine, and stake-confiscating
  slashing to the Coop vault (documented in `spec/RUST-FIRST.md`, modelled in `spec/Rchain/Pos.lean`).
  Still deferred: the vault **unforgeable-name capability** (the vault stays a balance map keyed by
  REV address) and the `revvaultexport` tooling.
- **Accepted-faithful residuals** (by design, not defects — see `spec/audit/passes.md` §5/§11): plaintext
  external-IP discovery (M7), the DAG `seen`-cache Θ(N²) *residency* (H6 — its per-clone cost is no
  longer paid: `seen` is shared behind `Arc`, and neither reading nor extending the DAG copies it,
  per the 2026-09-24 pass; the residual *size* is now reported rather than estimated, by the DAG's own
  `logical_bytes` gauge — measured live at **556 MB on the 5,885-block devnet chain, inside a 1.18 GiB
  process** that advances ~1.3 MB per block, where this row's predecessor figure was an estimate from
  Σ|seen| × 32 B on a tree that copied the DAG), and the rate-limited-but-plaintext
  Kademlia discovery bind.

The earlier "deferred/unwired" surface (Kademlia, the HTTP transaction API, block reporting, the
rholang parser's genesis gaps, peer store-items ingress) is now **wired and fixed** (see
`spec/audit/passes.md` §8/§11); the `rho:regex` system process never existed in the Scala oracle (the `regex`
crate was orphaned and has been removed). The audit gate (`tools/audit-type-system.sh`) is **clean** — zero production
`panic`/`unsafe`/silent-conversion, with the remaining `assert!` sites whitelisted as documented
internal invariants; equivocation rejection and finalizer fringe advancement now have regression tests
(`spec/TEST-COVERAGE.md` G1/G8).

---

## 5. Cross-links

- [`RUST-AFFORDANCES.md`](RUST-AFFORDANCES.md) — the complement of this page: this one registers where the
  port *differs from the reference*; that one records what the port could *build* because it is Rust, each
  gain with its artefact and its JVM cost.
- [`AUDIT.md`](AUDIT.md) — the check-off; [`audit/passes.md`](audit/passes.md) — the pass record and the Scala-deviation log (§6).
- [`TYPE-SYSTEM.md`](TYPE-SYSTEM.md) — the ρ→CoC type discipline and refinement types.
- [`RHO-CALCULUS.md`](RHO-CALCULUS.md) — the ρ-calculus grammar, sorts, and operations.
- [`INVENTORY.md`](INVENTORY.md) — the invariant catalog.
