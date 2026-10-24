# The laws

rholo's behaviour is pinned by **laws** — one invariant per layer of the system. Each law maps a
language or system feature to its formalization: a Lean theorem, a Coq axiom, the executable K rule, and
the Rust realization. This page is the full set, in the order the register numbers them, with each set
saying **what it is about**.

> **The checked rendering is [`spec/LAWS.md`](../../../spec/LAWS.md)** — emitted from `Rchain/Laws.lean`
> and refused stale by the gate. It is the authority for every law's **status** (and for the per-row
> history: `spec/LAWS.md`, [`spec/INVENTORY.md`](../../../spec/INVENTORY.md) and `spec/AUDIT.md` each
> carry a part of it). It counts *clauses* rather than laws and separates **`proved-tied`** (proved *and*
> tied to the node by a conformance corpus) from **`proved-model`** (proved about a model a human keeps
> in sync); the prose on this page is the narrative reading of the same rows, and where the two
> disagree, the register is right. Its total is **<!-- counts:proved-laws -->73<!-- counts:end --> of
> <!-- counts:laws -->80 laws<!-- counts:end --> proved at all — <!-- counts:proved-tied-laws -->19<!-- counts:end --> tied to the node by a corpus,
> <!-- counts:proved-model-laws -->56<!-- counts:end --> over the model — with two more laws proved but *vacuous* (their statements restate their
> own definitions, and the register has a word for that).

## The set, and what each part of it is about

| Laws | Set | What it is about | The narrative |
|---|---|---|---|
| 1–6 | **Rholang** | the calculus itself: canonicalization, α-equivalence, substitution, reduction, matching, closedness | [Substitution and matching](substitution-matching.md) |
| 7–11 | **RSpace** | the tuple space: joins, the comm event, merge, the Merkle trie, replay | [The concurrency model](concurrency.md) |
| 12–13 | **Rosette** | the actor VM — orphaned: `rosette`/`roscala` is out of scope for this port | — |
| 14–19 | **Casper, storage, crypto** | finality and the fringe, the DAG's monotonicity, block validity, the merge's arithmetic, the height map, the cryptographic primitives | [Determinism of the block state transition](determinism.md) |
| 20–25 | **The scheduler** | effect scheduling: the per-channel claim queue, the DFS gate, and the on-chain validated-speculation layer | [Effect scheduling](scheduling.md) |
| 26–29 | **Cross-shard** | two-phase commit across shards: scope, atomicity, idempotency, the durable decision record | [Cross-shard transactions](cross-shard-transactions.md) |
| 30–43 | **The surface** | what a client writes and a matcher reads: grammar, lexing, normalization, matching, reply shapes, the JSON envelope | *(below)* |
| 44–47 | **The validator lifecycle** | the Proof-of-Stake epoch: the boundary gate, the reward split, its conservation, staged withdrawal | *(below)* |
| 48–49 | **Charging** | what a denied deploy does to the merged state, and what a matched deploy is charged | *(below)* |
| 50 | **Depth guards** | the two budgets a term's size is bounded by: the AST depth the parser refuses, and the value depth the space refuses — the second being the route the parser cannot see (a contract folding pairs at run time) | *(below)* |
| 51–54 | **Progress** | non-progress: its three shapes (a requirement nothing satisfies, a refused state no rule restores, a run whose measure never moves), its two predicate causes (a predicate that reads history, two readers that split), the two answers finality's gate must give (52: the partition may shrink, the quorum denominator may not), one attribution that is terminal (53: the refusal persists and the rule refuses its children), and which view a rule reads (54) | [Progress: the shapes of non-progress](progress.md) |
| 55–56 | **Bounded work and the rooted namespace** | every step's cost is bounded by a quantity the protocol bounds, and a name is rooted in the identity that owns it | *(below)* |
| 57 | **The delegator's stake** | a delegation joins the operator's pool and its reward splits pro-rata, so a validator with delegators commits what it would alone | *(below)* |
| 58 | **The finality gate's forward dual** | a fully-participating view *is* the whole bonded set, so the gate publishes — the direction Law 52's free support map left open | *(below)* |
| 59–65 | **The wire and the session** | the CapTP bridge's round trip and refusals, the install rule, the session loop, the dial policy, attribution, interop, and the printer's literal guard | *(below)* |
| 66–71 | **The distributed-system set** | the fringe record's key, the record join, versioned reads, the state-page walk's termination, transfer correctness, and the catch-up window | *(below)* |
| 72–77 | **The durable reset, the stop, and the read construction** | the reset's completion bracket, its request, a checkpoint that folds before it drains, a dependent action ordered on a committed record, a concluded stop, and the reset's projection onto its root | *(below)* |
| 78 | **The genesis's developer allocation** | a genesis credits one allocation no wallets file contains — `max(20 000 000 REV, 2% of the allocated total)` — derived from the same file the creator and a joiner both read, so the amount is one function of the allocation | *(below)* |
| 79 | **The allocation's schedule** | it is credited to a *vesting account*, not a spendable vault: the vested fraction is the schedule position, linear in the height, zero at genesis and total at the end, and a duration of `0` is no schedule at all | *(below)* |

The last three sets are the ones a graph of the *calculus* does not reach. Rows 30–43 exist because every
defect that started the formalisation programme (AUDIT C9–C22) lived there and **nothing errored**: an
unmatched `for` is not an error, a wrong reply shape is not an error, a swapped connective is not an
error — each read as a client bug, and each was found on a running node. Rows 44–49 exist because the
port had a validator lifecycle, a merge and a gas account and no law for any of them.

---

## Rholang — the language (Laws 1–6)

| Law | Invariant | Surface feature | Lean | Coq | K rule |
|---|---|---|---|---|---|
| **1** | canonicalization is idempotent & commutative: `sort(sort p)=sort p`, `sort(p\|q)=sort(q\|p)` | the sorted `Par`, commutative `ESet`/`EMap` | `Sort.lean` — `sortPar_idempotent` / `sortPar_comm` (the element-comparator laws behind them are theorems, not axioms) | `Sort.v` — `sortPar_idempotent` / `sortPar_comm` (axioms) | normalization (α + canonical `\|` sort) |
| **2** | α / name equivalence: par order, `\| Nil`, associativity, top-level arithmetic, α, `@`/`*` | `@`/`*`, `@{P\|Q}=@{Q\|P}` | `Rho.lean` — `StrCong` `≡` | `Laws.v` — `alpha_equiv` (an inductive mirroring `StrCong`; with de Bruijn *levels* deep α is equality, so no track owes it) | `name-equivalence.k`, `alpha-equivalence.k` |
| **3** | capture-avoiding de Bruijn substitution; `sort(subst t)=subst(sort t)` | variable binding | `Subst.lean` — `substPar` (a `mutual` family mirroring `rholang/src/substitute.rs`), `sort_subst`, `subst_closed` (given the closed image) | `Laws.v` — `substPar`, `subst_commutes_sort` (axioms) | `free.k` (substitution; `substitution.k` referenced) |
| **4** | reduction (COMM), first-match-wins, `new` freshness | send/receive, `match` | `Rho.lean` `Reduce`; `Concurrent.lean` `reduce_redex_unique` (an isolated redex reduces to its body, uniquely up to `≡`) and `reduce_not_deterministic` (the flat `Par` is *not* confluent, which is why the block path's determinism comes from the scheduler, Laws 20–25); `Reduce.lean` `reduce_freeVars_subset` | `Laws.v` — `reduce` (axiom) | `processes-semantics.k`, `sending-receiving.k`, `persistent-sending-receiving.k` |
| **5** | spatial matching, defined; an accepted match binds each free level at most once | patterns, `_`, `~`, `/\`, `\/` | `Match.lean` — `spatialMatchCore`/`spatialMatch` (a fuel-carrying matcher, so "does it match" is *computed*), `spatialMatches`, `spatialMatches_decidable` (an instance, not an axiom), `spatialMatch_implies_linear`, `linear_of_pathPar`. Law 37's tie — that the clauses decide equality on the shapes they have an arm for — is that row's theorem (`spatialMatches_iff_eq`, over `pathPar`) | `Laws.v` — `spatial_matches` (axiom); `linear` is a **definition** with `linear_decidable` and the witness `a_double_binding_is_not_linear` | `matching-function.k`, `specific-matching-rules.k`, `exact-matching-function.k`, `matching-with-par.k` |
| **6** | no globally free variables | `Closed` | `Ty.lean` `Closed` + `FreeVars.lean` `freeVarOf` (a `mutual` block mirroring the checker) and `closed_iff_no_freeVars` | `Laws.v` — `closed`, `closed_decidable` (axiom) | `free.k`, `program-restrictions.k` |

The reader-facing story — substitution, the matcher, and what silence means — is
[Substitution and matching](substitution-matching.md); the sort discipline the parser feeds is
[Grammar and sorts](grammar-sorts.md).

## RSpace — the tuple space (Laws 7–11)

| Law | Invariant | Feature | Lean |
|---|---|---|---|
| **7** | join commutativity (channel keys hashed in sorted order) | multi-channel receive | `RSpace/Join.lean` — `joinKey` is *defined* as hash-of-sorted-channel-hashes, so `joinKey_perm` is law 1's canonicalization applied to a join key |
| **8** | deterministic COMM (produce refs sorted; content-addressed events) | the comm event | `RSpace/Comm.lean` — `produceRefs`/`commId` defined over the Rust's own sort, `comm_content_addressed` |
| **9** | merge is a monoid; non-conflicting logs commute | state merging | `RSpace/Merge.lean` — `mergeChanges` defined (added/removed concatenate, the join map is right-biased overwrite), `NonConflicting` defined, `mergeChanges_assoc`/`_comm` |
| **10** | Merkle determinism (content-addressed radix trie, collision-free, empty root) | history | `RSpace/Merkle.lean` — `nodeHash`/`emptyNode`/`emptyRoot` defined over the node type the code has, `encodeNode_injective` over `WellFormed`, `root_collision_free`/`nodeHash_eq_emptyRoot` |
| **11** | replay determinism (recomputed COMM ⊆ recorded trace) | replay | `RSpace/Comm.lean` — the recorded trace is an **input** (`Replays recomputed recorded`), and the check holds exactly when the two agree on occurrences (`replays_iff_same_occurrences`); neither half suffices, each with a witness, and the second witness is the port's own guard — `check_replay_data_with_fix` drops the reverse half on a failed deploy (RCHAIN-3505) |

## Rosette — the actor VM (Laws 12–13)

| Law | Invariant | Status |
|---|---|---|
| **12** | actor atomicity (single-threaded `mbox.nextMsg`) | **orphaned** — `rosette`/`roscala` VM is out of scope |
| **13** | reflection (everything is an `Ob`; meta/parent chain; fork-join barrier) | **orphaned** |

## Casper / Storage / Crypto (Laws 14–19)

| Law | Invariant | Feature | Lean |
|---|---|---|---|
| **14** | finality requires **> 2/3** bonded stake; fringe = one message per validator | the fringe | `Casper/Stake.lean` — `calculateFringe` is the port's `calculate_fringe` (full-partition filter, non-bonded skip, exact-integer comparison) and `Fringe.lean`'s `finality_iff_supermajority` is the advance gate, with the 2/3, 2⁵³ and `i64` boundary tests; the antichain is proved **over the derivation** (`Casper/Dag.lean`: the walk, the min messages, the count gate and the layer fold, in the port's own order) — stated *about the fold* rather than about the `BTreeMap` the port stores, so that a one-word mutation (`layerInsert`'s filter dropped) makes it false rather than merely unproved |
| **15** | fringe monotone by height; seen-set monotone | the DAG | `Casper/Fringe.lean` — the port *constructs* a message's seen set (`seenOf`, with `seenOf_contains_justifications`/`mem_seenOf_self`); the closure the finalizer leans on is proved along justification reachability (`Reaches`/`seen_monotone_of_reaches`); what is owed is the *height* half, and the id-level form needs the DAG's own closure invariant stated |
| **16** | block number = max(parent)+1; seqNum strictly +1; content addressing; bonds cache = PoS | blocks | `Casper/Validate.lean` — the two number laws are the port's own checks (`BlockNumberValid`/`SeqNumValid`, folds over the justifications seeded `-1`) with the laws as their elimination; `content_addressing` follows from `blake2b256_collision_free` and the serializer's canonicity, and the serializer is a **definition** (`encodeBody` over protobuf's wire format, `Rchain/Proto.lean`) with `Canonical` load-bearing. The ties to `prost`'s bytes stay prose (no crate is vendored): the `body` conformance layer is the named follow-up |
| **17** | the merge's arithmetic is the checked 64-bit one — a value that would leave `i64` is **refused, not wrapped** — and the merged RNG is a function of the *set* of branch generators | merges | `Merging.lean` — `checkedAdd`/`checkedSub` with their refusal witnesses, `mergeRandoms_perm`. Numeric channels are signed `i64` and negative diffs are ordinary (`rholang/src/merging.rs:161-166`), which is why the non-negativity form was not the law |
| **18** | height map contiguous; fringe identity order-independent | storage | `Casper/Validate.lean` — `Contiguous` is the store's invariant with `contiguous_insert_succ`/`contiguous_skip_leaves_hole`, and `fringeId`/`fringeId_perm` give the order-independence the `BTreeSet` supplies |
| **19** | Blake2b256 canonical; the `Blake2b512Random` merge is n-ary and **order-sensitive**; sig verify/sign; Curve25519 round-trip | crypto | `Crypto/Random.lean` `mergeRandom` (n-ary, mirroring `merge(children: &[Self])`); `Crypto/Spec.lean` `blake2b256_collision_free`, `blake2b256_output_is_32_bytes` (the width the code's `Hash32` types), `sign_verify_roundtrip`, `curve25519_roundtrip` — **the register's axiom set is this row's cryptographic boundary, by design**. Order-independence is stated where it holds: at the *call site*, `Merging.lean`'s `mergeRandoms_perm` |

The state transition these laws constrain, and what determinism means for it, is
[Determinism of the block state transition](determinism.md).

## The scheduler — effect scheduling (Laws 20–25)

| Law | Invariant | Feature | Lean |
|---|---|---|---|
| **20** | channel-task linearization ("1 channel = 1 logical task"): a per-channel claim queue keeps same-channel commits in DFS path order; the path-smallest pending claim is always committable | the claim queue | `Scheduler.lean` — `queue_commit_path_ordered`, `pathSorted_head_minimal` (the bakery argument's core). The *global* liveness claim needs the finiteness the real system has: `PathLt` is not well-founded on paths, so an unbounded pending set need have no minimum |
| **21** | DFS-gate linearization: running effect `i` only after effects `0..i−1` complete is exactly the sequential apply fold; the one-hop (next-step-footprint) variant is unsound | the gate scheduler | `Scheduler.lean` — `gate_await_closure_orders` (the immediate-predecessor await chain is transitively complete, which is the Rust's "not the quadratic all-predecessors join"), `one_hop_depth2_diverges` (counterexample) |
| **22** | next-step closure is computable at dispatch (the matched datum is concrete); computability does *not* make cross-channel pruning sound | dispatch-time closure | `Scheduler.lean` — `depth2_next_step_disjoint` (the half with content). The positive half is a *signature* fact (`resolve_children` takes no store); the tempting distributivity law is false for the Rust's flattening order |
| **23** | read-determinism: an effect's outcome and event trace are a deterministic function of the state it reads | on-chain speculation | `SchedulerOnchain.lean` — `read_state_determines_outcome` |
| **24** | DFS-order serializability: a concurrent run is sound iff every commit read the state the DFS-earlier effects produced (versioned write-record layer; prefix visibility) | the validation certificate | `SchedulerOnchain.lean` — the witnesses, `record_determines_value`/`dispatched_record_at`, `serializable_writer_chain` and `pinned_run_publication`. The publication theorem carries **no** certificate hypothesis: the unused argument and the linter suppression that hid it are both gone |
| **25** | validated speculation: commits may reorder iff each validates Law 24; invalidated subtrees abort and gate-re-run, so the published log is the sequential fold's | the abort/re-run coordinator | `SchedulerOnchain.lean` — `published` + `published_state_is_the_oracles` (accept only on agreement with the oracle, else ship the oracle's result — so the certificate decides *how much* is re-run, never *what* is published), `Published` + `fallback_rerun_published` |

The reader-facing story — the claim queue, the DFS gate, the relaxed mode, the unsound one-hop variant,
and the on-chain extension — is [Effect scheduling](scheduling.md).

## Cross-shard — two-phase commit (Laws 26–29)

| Law | Invariant | Feature | Lean |
|---|---|---|---|
| **26** | shard scope determinism: a deploy/block's effects bind to exactly one shard; the shard id is a validated, ordered value; the RNG seed + unforgeable names are shard-scoped | the shard boundary | `CrossShard.lean` — the law's real content is the **ingress**: `admitLeg` mirrors `node/src/web/http.rs:203-218` (non-empty and ASCII shard id, non-blank recipient) and `the_admitted_leg_carries_a_validated_shard_id` states all three conjuncts, with the two boundaries — the amount is deliberately *not* checked at the ingress, and the recipient predicate is over code points — as theorems |
| **27** | cross-shard atomicity (2PC): a transaction commits on every participant or aborts on every one — no run leaves a strict subset committed | the coordinator | `CrossShard.lean` — the decision half is proved: `allReady`/`coordinatorDecision` mirror `txn_coordinator.rs:177-179` and `coordinator_decision_committed_iff` gives `committed` iff every vote is ready. The universal form over a freely constructible `Run` was **false** and is refuted (`txn_atomic_is_false`) |
| **28** | leg idempotency: `prepare`/`commit`/`abort` are idempotent under the transaction id | re-delivery / retry | `CrossShard.lean` — `leg_idempotent` |
| **29** | decision durability & record determinism: the coordinator's decision is a durable, content-addressed record; a prepared participant recovers it; re-derivable on replay | the merge/close record | `CrossShard.lean` — the decision half is proved (`coordinator_decision_committed_iff`); durability is the half with a record, and it is named rather than assumed |

The reader-facing story — the Git pull-request flow, the roles, the state machine, and the 2PC recovery
caveat — is [Cross-shard transactions: two-phase commit](cross-shard-transactions.md).

## The surface a client writes and a matcher reads (Laws 30–43)

Fourteen rows, `spec/INVENTORY.md` 30–43, covering the grammar, the normalizer, the matcher's patterns,
the reply shapes and the JSON. They close the same way the calculus rows do — a Lean declaration, a
corpus emitted from it, a Rust consumer held to the same cases — with one addition that matters: the
gate ([`tools/check-lean-conformance.sh`](https://github.com/rhovision-org/rholo/blob/dev/tools/check-lean-conformance.sh))
refuses a stale corpus, an unimported module, a `sorry`, and a corpus with no consumer, so a law that
stops holding is noticed by a machine rather than by a user.

| # | Law | Where it lives |
|---|---|---|
| 30 | every term the parser accepts is in the grammar | `Rchain/Parse.lean` — the grammar as **data** (`grammarFragment`, `derives`), the deviations as a list of named claims (`parseDeviations`), and a `decide`d corpus row per production |
| 31 | every grammar term is accepted, modulo a data list of deviations | the same: the fragment reads every production, and each deviation is a row with its own detector |
| 32 | each spelling lexes one way | `Rchain/Lex.lean` (`lexemes`, maximal munch) — the operator surface is checked |
| 33 | `parse (print p) ≡ p` | `Rchain/Print.lean` (`printToks`, `printSurf`, `renderTokens`) — checked in **two halves**: the model's, that the printer's output is a grammar term, and the identity itself, which runs on the node in the Rust consumer, modulo the named warts (`printWarts`) |
| 34 | a value position is normalized against an empty par; only a statement continuation inherits | `Surface.lean`'s `normalizeAt` + the `c21` corpus layer (`Corpus.lean`'s `c21Cases`) — both halves checked on the shapes C21 broke, the accumulator itself now **modelled** (`normalizeAt` threads the port's `ProcVisitInputs.par`, and only the sequencing arm hands it on) |
| 35 | concreteness is sound (connective, free var, wildcard **or remainder**) | `Rchain/Par.lean`'s `connectiveUsed` + the `flags` corpus |
| 36 | the normalizer's output is well-scoped and closed | `Surface.lean`'s `normalize` — modelled; the corpus that would hold it to the node does not exist |
| 37 | matching is sound and complete, over the shapes the clauses cover | `Rchain/Match.lean` — `pathPar` is that domain and `spatialMatches_iff_eq` is the tie; `match.tsv` ties the model to the node |
| 38 | silence is specified: no reduction, no error | `Rchain/Silence.lean`'s `ReduceP` — the rule is the law and the `silence` corpus ties it; the tie between the rule and the search is a theorem in both directions (`takesStep_sound`, `takesStep_complete`, so `takesStep_iff_reduces` is not an axiom). What is left is the model's boundary rather than a proof debt: it has no join rule, so a join with one channel filled is silent here and in the node, while a fully matched join is a step in the node and none here |
| 39 | every `rho:*` urn's reply shape equals its `API-SCHEMA.md` row | `Rchain/Protocol.lean`'s `replyCatalog` — checked for the rows the surface can spell |
| 40 | every call has an accepting receive at the target's arity | `Silence.lean`'s `stepsInBinds` + `ReduceP.commPs` (the rule clause that makes it a rule) |
| 41 | a replicable reader restores what it consumes | `Rchain/Store.lean` — the `store` corpus ties it (measured on the live contract) |
| 42 | the rho-value round-trip, and the envelope rule | `Rchain/Json.lean` — the corpus ties the wire form and `decode_encode` is a theorem; the axiom it replaced was not merely owed but **false**, because its domain predicate admitted a par of two unforgeables, which the decoder drops and the encoder writes as nothing at all (`unforgPair_refutes_the_old_statement`; the predicate now refuses one) |
| 43 | each endpoint's serialized shape equals the schema's | `Rchain/Envelope.lean`'s catalog — checked for the envelope's keys and tags |

### What each layer buys, in one line each

- **Grammar (30, 31, 32, 33)** — a term is what the grammar derives, and nothing else. C30's finding is
  the shape of it: `parse` returned `Ok` for a valid *prefix* (`null )` parsed), and because
  `source_to_adt_with_env` runs on `deploy.data.term`, a deploy could execute part of what a client wrote
  and report success.
- **Normalization (34, 36)** — a value position is normalized against an *empty* par; only a statement
  continuation inherits the accumulated one. C21 lived here: a non-first `if` reduced to nothing.
- **Matching (35, 37, 38)** — an unmatched receive performs no step and reports no error. That is the law,
  stated, and it is why the defects were expensive: the silence *is* the correct behaviour, so a broken
  pattern is indistinguishable from a pattern that legitimately matched nothing.
- **Replies and protocols (39, 40, 41)** — a reply's arity and shape are a row in a table, and a call at
  the wrong arity has no accepting receive *by the rule*.
- **JSON (42, 43)** — the wire form is the reference document's, arm for arm, and the served document is
  held to the same catalog the DTOs are. C38 is the finding: the code and the law agreed with each other
  and neither with a client, because the law had been written from the code.

**A law written from the code is a mirror.** The corpus cannot catch a wrong shape when the model and the
node were derived from each other; only the contract can, and it has to be read rather than assumed.

## The validator lifecycle — the Proof-of-Stake epoch (Laws 44–47)

The port had the lifecycle — bond, withdraw, slash, trust — and no epoch, so a membership change took
effect the moment its deploy ran and a reward law had no reward to be about. Every law below is anchored
to `rholang/src/native_state.rs`, the native replacement for `legacy/casper/src/main/resources/Pos.rhox`.
The Scala contract is the **differential reference** — the port must satisfy the law, and where the
contract's behaviour is the law, the row cites its line. The design decisions are in
[`spec/RUST-FIRST.md`](../../../spec/RUST-FIRST.md) (`## The staking vault`, `## The epoch`) and the
epoch-boundary deviations in `spec/audit/passes.md` §6.

| # | Law | Where it lives |
|---|---|---|
| 44 | membership takes effect at an **epoch boundary**: the sequence runs only when `blockNumber % epochLength = 0`; a bond is pooled but not activated, a withdrawal is staged but not moved, a claim is not paid | `rholang/src/native_state.rs`'s `close_block` gate (`is_epoch_boundary`); the machine is `Rchain/Pos.lean` (`isBoundary`, `epochStep`, `closeBlock`: commit → move → pay → reselect, in `close_block`'s order, with the gate *outside* the transition), witnessed by `closeBlock_off_a_boundary`, `a_bond_pools_but_does_not_activate` and `a_boundary_activates_the_pool` |
| 45 | the split: `pot * (bondᵢ / minimumBond) / (activeBonds / minimumBond)` per active validator, out of `pot = posBalance − totalBond − totalWithdraw − committedRewards`, committed per validator and paid only when the validator leaves | `Rchain/Pos.lean` (`rewardPot`, `reward`) — the formula is the Scala's `getCurrentEpochRewards` (`Pos.rhox:241-256`) and the tie to the Rust is named (`an_epoch_splits_the_pot_and_keeps_the_dust`). Where the contract is undefined — `minimumBond` 0, or a normaliser of 0 — it divides by zero and faults; the port pays zero, registered in `spec/audit/passes.md` §6 |
| 46 | the split **does not conserve**: `Σ rewards ≤ pot`, the difference being the dust of two integer divisions — which stays in the pot for the next epoch | `Rchain/Pos.lean` (`sum_rewards_le_pot`, `list_sum_div_le`, `div_add_div_le`, `the_dust_is_real`), with a strict instance `decide`d: minimum bond 3, bonds `[4, 5]`, pot 10 — **six distributed of ten** |
| 47 | a withdrawal is **staged**: the request records `quarantineLength + epochLength * (1 + blockNumber / epochLength)` and changes nothing else; the validator leaves the pool at the next boundary and is paid `bond + committed rewards` at the first boundary past its quarantine | `rholang/src/native_state.rs` (`withdraw`, `close_block`'s move and pay steps); `Rchain/Pos.lean`'s `stage`, `movePending`, `dueClaims`, `payoutOf`, with the three stages witnessed separately (`a_staged_withdrawal_moves_no_coins`, `the_move_escrows_the_bond_and_pays_nothing`, `a_due_claim_is_paid_its_bond_plus_its_committed`, `a_claim_before_its_deadline_is_not_paid`) and the ordering by `the_reward_is_committed_before_the_leave` |

### Why 46 is the row worth having

The contract's reward divides **twice**:

```text
reward_i = (posBalance - totalBond - totalWithdraw - totalCommittedRewards) * (bonds / minimumBond)
                                                                     / (activeBonds / minimumBond)
```

so `Σ reward_i` is the pot *minus* dust — the sum of the floored shares, re-floored. A law like "the
epoch distributes the pot" is therefore not merely unproved but **false**, and the difference is not
noise to be waved away: it is a quantity the next epoch redistributes, because the dust stays in the
staking vault and the next pot is the vault less the same claims. `Pos.lean`'s `the_dust_is_real` is
minimum bond 3, bonds `[4, 5]`, pot 10: each validator's scaled share is `4 / 3 = 5 / 3 = 1`, so each is
paid `10 * 1 / 3 = 3`, and the epoch moves **6 of 10 units**. The Rust test builds exactly that state —
the two bonds, the minimum bond, a pot of 10 paid in as phlo — and reads the split back, so the
implementation is checked against the arithmetic rather than against a remembered number.

### The order of the sequence is the law

An epoch boundary runs four steps, and their order carries meaning (`Pos.rhox:528-551`):

1. compute the epoch's rewards **from the state as it stands** and add them to `committedRewards`;
2. move each staged withdrawal into the claims map, taking it out of the pool;
3. pay every claim whose quarantine has elapsed — `bond + committed[pk]` — and clear both entries;
4. re-select the active set from what is left of the pool.

Because step 1 runs before step 2, a validator that asked to leave **earns the epoch it left in**, and
because its reward is paid at step 3 of a *later* boundary, the claim must store the bond and read the
reward out of the committed map at payment time rather than storing the sum. That is why the contract's
header comment describing the stored pair as `(bond + reward, quantunue)` is the *payee's* sum and not
the record: the code at `Pos.rhox:582` stores the bond alone, and `:604` adds the reward.

**The sequence is not idempotent** — a second call at the same height would redistribute the previous
epoch's dust, since the committed claims now cover the rest. The system deploy calls it once per block.

### What these laws rest on, and what is still owed

`close_block`'s *arithmetic* is modelled; its *transition* is not. The formalization worth writing for
laws 44 and 47 is not the gate itself — `if boundary then … else s` restates its own definition, which is
the `vacuous` shape this register has a word for — but the **conservation an epoch preserves**: the
staking vault plus the Coop vault plus every user vault is invariant, which is what makes a payout a
transfer rather than a mint. The row that found this the hard way is the port's `slash`, which credited
the Coop vault without debiting anything: the staking vault (law 45's pot source, `pos:vault`) is what
turned that into a transfer, and `total_rev` in the Rust tests is what holds it.

Two deviations from the contract are recorded in `spec/audit/passes.md` §6 and matter here: the active set is
chosen **top-N by descending stake** (the contract takes the first N entries of the bonds map in key
order, under a `TODO` for a random selection), and a slashed validator with a staged withdrawal is
removed from the pending map rather than left as a permanent zero-claim tombstone.

## Charging — the denied deploy, and what a matched one costs (Laws 48–49)

The last two rows are about **gas and consequences**, not about state, and both were decided rather than
ported.

| # | Law | Where it lives |
|---|---|---|
| 48 | a **denied** deploy's effects are excluded from the merged state, and the merge's objective counts its cost exactly as an included deploy's — so the fee consequence RCHIP-02 proposes (the deployer is not charged, the validator is not rewarded) holds in neither tree | `casper/src/merging.rs`, `rholang/src/native_state.rs`, `docs/src/node/block-merge.md` — **an open design question shared with the Scala**, not a port divergence: `MergeScope.scala:87` defaults `rejectionCost` to `DeployChainIndex.deployChainCost`, and `DeployChainIndex.scala:73` defines that as `deploysWithCost.map(_.cost).sum` — the same objective this port computes. Nothing in either tree refunds a denied deploy. Closing it means choosing an objective and a refund path the Scala does not have, which is a consensus change without an oracle; the decision taken is to leave it open *with the reason* |
| 49 | for a matched produce/consume the charged gas is the Scala's: the storage is charged up front and what the match consumed is **refunded** — the continuation's consume storage and the produce storage of every removed datum — *before* the event and COMM costs | `rholang/src/storage.rs`; the model is `Rchain/Charging.lean` (`prefixes`, `peak`, `itotal`, `peak_refunds_first`, `itotal_refunds_first`), and the Rust test asserts both halves: the matched call's total against the same op *without* a match plus the COMM cost minus the two refunds, and the balance at which the deploy completes versus one phlo below it. The second half matters because the *peak* is what refuses a step: a test that checked only the total would pass with the refunds charged last |

Law 49 is also a **hard fork**: the refund changes the recorded `PCost` of every deploy that matches, so a
chain that accepted such a block under the old (over-charging) rule diverges on it. Registered in
`spec/audit/passes.md` §6 with the reason the old value was already wrong.

---

## Depth guards (Law 50)

**What it is about.** A term's size is bounded twice, and for two different reasons. `MAX_AST_DEPTH`
bounds what the **parser** will build: a flat chain of operators is built by a *loop*, so the parser's own
recursion bound does not bound the tree it produces, and a source of a few kilobytes could otherwise make
every later consumer recurse arbitrarily deep. `maxValueDepth` bounds what the **space** will store: a
contract that folds a value into a deeper pair never passes the parser at all, so the parser's guard
cannot see it.

**Where it lives.** `Rchain/Depth.lean` — `parDepth` is the mutual depth over the model's syntax,
`walkPar_iff_parDepth` ties it to the walk the node runs, and `walkExceeds_sound`/`walkExceeds_complete`
are the two directions the guard needs; clause b's quantity is `parNestDepth`, at most three times
`parDepth`. The register's row carries the measurements (a debug build aborts the *process* between AST
depth 732 and 994; a release build costs 64 s at 8,040) and the boundary no theorem can state — that the
Rust walk descends into every `Proc` constructor, which is pinned by a test that parks a deep child in
each position the walk must reach.

## Progress — the shapes of non-progress (Laws 51–54)

**What it is about.** The same *kind* of defect arrived four times in two days under four names — a
fringe partition no state could satisfy (C174), a refusal no rule clears (C173), steps forever with the
finality measure flat (C171), a liveness predicate read off a map that keeps history (C170) — and each
was argued in prose and fixed alone. Law 51 is the general statement they were instances of: a
vocabulary for non-progress, parameterised over a transition system, so one set of definitions serves
the reducer and the protocol built on it.

**Three shapes, two causes.** The *shape* says why the obligation cannot be met, and therefore what kind
of repair applies:

| shape | form | C-finding | the fix it implies |
|---|---|---|---|
| `Void` | no state satisfies the requirement | C174 | change the requirement |
| `Terminal` | a satisfying state was left **and** no rule restores it | C173, C172 | give the refused state an inverse |
| `Drift` | a run's measure never moves, however long it runs | C171 | bound the rate — but read the cause first |

The *cause* says where the fix goes: a `Historic` predicate (C170's guard counted a validator that had
*ever* spoken) or a `Split` between two readers of one state (C172's index and store). The causes produce
the shapes — the historic predicate is what let the guard's suppression never fire, which is C171's
drift — and that pairing is what makes the vocabulary diagnostic rather than taxonomic.

**What it deliberately does not model.** Starvation — "an enabled step that no schedule takes" — is
unstatable over a transition *relation*, and this tree's own `reduce_not_deterministic`
(`Rchain/Concurrent.lean`) proves the flat calculus fixes no schedule at all; the shape is registered
`open`, with that reason, rather than defined. The protocol's inactivity leak is out of scope for the
same kind of honesty — it is a state change (burning a silent validator's stake) this layer does not
have — and it is now a **decision rather than a debt** (2026-10-02, §59): the leak burns a *bond*, and
the constraint governing this workstream is that a staker must not lose its bond through no fault of its
own, with a partition the canonical case. The port's instrument instead is an **income-only**
participation rule (law 44), which scales an epoch's reward and cannot reach a stake. The citations this
used to carry (#24, #39) are both closed.

**What is shared with the calculus, and how the sharing is checked.** A `System` is a step relation plus
a decidable "is a step available now" predicate, *tied to the relation by the structure's own field*. The
calculus supplies that field with `takesStep`/`ReduceP` (`spec/Rchain/Silence.lean`), so silence in the
calculus **is** stuckness by the tie (`Rchain.a_silent_term_is_stuck`) rather than by a claim. The layers
differ in exactly one way, and it is the distinction the vocabulary exists to keep sharp: in the closed
calculus nothing outside the term can add a send, so a lone receive is silent forever, while at the
protocol level delivery is itself a step — a blocked wait and a permanent stall must not share a name.

Its register row is `51a`–`51c` (`spec/LAWS.md`), its model is `spec/Rchain/Progress.lean`, and
[Progress: the shapes of non-progress](progress.md) is the triage table an agent reads before proposing
a fix.

### The finality instance (Law 52)

The first instance of law 51 is the one the vocabulary was written for, and it is the law this
repository needed on 2026-09-29 and did not have: the increment-2 plan proposed handing the *live* weight
set to the finalizer as **both** the set a candidate must be seen by and the set the supermajority is
measured against — "so a silent validator leaves numerator and denominator together" — and nothing in the
tree could refuse it.

**Clause a — the denominator is the whole bonded stake.** Two disjoint groups cannot each hold a strict
supermajority of one total: `3·s₁ > 2·t` and `3·s₂ > 2·t` force `t < s₁ + s₂`
(`supermajorities_overlap`). So a denominator taken from the live set makes *any* self-consistent subset
a supermajority of itself, and under a network partition each side would finalise its own view — two
conflicting finalisations. The falsifier is two-sided on one fixture: the two disjoint sides each advance
at their own denominator (200 of 200) and both are refused at the bonded one (200 of 400), while the
*full* partition advances there (300 of 400), so the result cannot be read as "the gate never fires".
What the clause does **not** claim: that two conflicting *messages* cannot be supported — the step from
"the supporting sets overlap" to "the chain cannot fork" is Law 15's, named as a hook rather than proved
here.

**Clause b — the requirement must be one a reachable state can satisfy.** With the whole bonded set as
the *partition*, a validator that produces no message makes the full-partition filter unsatisfiable — not
slow, **empty** — because `allBonded` demands that every seer's seen set contain the bonded set and no seer
can have seen a validator that never spoke. (Containment rather than equality since #213: a seer *outside* the
live partition does not void a candidate the partition has seen — AUDIT C208.) That is the `Void` shape of Law 51 with its proof obligation
discharged: the emptiness is derived from the predicate rather than observed on a fixture, and the
fixture instance is two-sided as well (three speaking validators refused at 300 of 400; the identical
fixture advancing once the fourth speaks). The hypotheses are named in the instance module
(`Participation`, `Delivery`, `StalenessBound`) because a liveness claim without its hypothesis is Law
20's trap. What it still does not fix, plainly: a net that stays below two thirds **permanently** still
cannot finalise — and that is a **decision rather than a debt** (2026-10-02, §59). The instrument that
would price it is an inactivity leak, a state change that burns a silent validator's *bond*, and the
constraint governing this workstream is that a staker must not lose its bond through no fault of its own
— a partition is the canonical case of exactly that. What the port has instead is an **income-only**
participation rule (law 44), which scales an epoch's reward and cannot reach a stake. It is not a recency
window, and the citations this used to carry (#24, #39) are both closed.

---

## Bounded work, the namespace, and the wire (Laws 55–65)

**What they are about.** Eleven laws that arrived *after* the progress vocabulary, over several
unrelated problem areas. Law 55 is the class **AUDIT C180** filed as `candidate:bounded-work-per-step`:
work whose cost grows with an input nothing bounds, and where a bound exists it belongs to the step
rather than to a check after it. Law 56 is the rooted namespace, where a name is rooted in a **derived**
identity so a write outside your own root is inexpressible rather than refused. Law 57 is the
Proof-of-Stake validator's delegator: a delegation joins the operator's pool and its reward splits
pro-rata, so a validator with delegators commits the same total it would have alone. Law 58 closes the
direction Law 52's free support map left open — a *fully-participating* view is the whole bonded set, so
the gate publishes. Laws 59–65 are the OCapN surface: the CapTP value bridge (59), the install rule
(60), the session loop (61), the dial policy (62), the one **open** row (63, a bridged deploy's
attribution), the interop rule that the implementations, not the prose, are the oracle (64), and the
printer's guard against a value with no faithful literal (65).

| # | Law | Where it lives |
|---|---|---|
| 55 | **Bounded work per step.** Every step costs at most a quantity the protocol bounds, and the bound is a **hypothesis of the step**, not a check after it (`spendStep` charges only while `spent < limit`); a spent budget carries **nothing**, so the bound is a resource policy and not a consensus change | `Rchain/Bounded.lean` (`Work`, `spendStep`, `workSystem`, and the defect's inverted `lateGuardStep`/`lateGuardSystem` with `the_late_guard_overspends`; `boundedRun` for the schedule half) — the merge search's `SearchBudget` (`sdk/src/dag/merging.rs`), the census (`casper/src/merging.rs`), and the ingress queues (`casper/src/blocks/block_receiver.rs`, `casper/src/blocks/block_processor.rs`) |
| 56 | **The rooted namespace.** `publish(path, value)` is accepted iff the path's owner prefix is the caller's **derived** REV address, so a write outside your own root cannot be spelled; versions are append-only, `seal` closes a path, and only the alias tier is a mutable pointer | `Rchain/Namespace.lean` (`NsPath`, `NsState`, `Verdict`, `publish`, `setAlias`, and the defect's `unguardedPublish`) — `casper/src/genesis/resources/rgov/MasterDictionary.rho`, `casper/src/genesis/rgov.rs`, `casper/src/genesis/mod.rs` |
| 57 | **The delegator's stake is the operator's bond, attributed.** A delegation moves a principal into the staking vault and adds it to the operator's pool entry (the aggregate), so it counts in the draw and in what a slash reaches; the epoch reward splits pro-rata, the operator keeping the integer remainder, so a validator with delegators commits what it would alone and the split is the identity with none | `Rchain/Pos.lean` (`PosDelegation`, `PosUndelegation`, `delegatedTotal`, `proRata`, `delegate`, `undelegate`, `split_sums_to_the_reward`, `a_second_delegation_accumulates`, `a_slash_clears_the_delegations`) — `rholang/src/native_state.rs`, `rholang/src/system_processes.rs` |
| 58 | **The gate publishes on a fully-participating set** — the forward dual Law 52 lacks. Law 52's gate is a biconditional over a *free* support map and never says one obtains; a participation-closed view is the whole bonded set, hence a full partition, hence the gate publishes. The schedule is a named hypothesis, not a theorem | `Rchain/Casper/Rounds.lean` (`participation_all_current`, `gate_publishes_of_a_full_partition`, `nextFringe_publishes_of_a_full_partition`, `the_licence_ends_at_the_horizon`) — `casper/src/blocks/proposer/proposer.rs`, `block-storage/src/dag/liveness.rs`, `block-storage/src/dag/finalizer.rs` |
| 59 | **The CapTP bridge round-trips its domain, and its refusals are named.** A value the bridge decoded is one it encodes and decodes again; the decode is injective on the domain; and the encoder never emits a shape outside it — no silent partiality applied to a wire | `Rchain/Syrup.lean` (`parToSy`, `syToPar`, `wireable`, `syrup_decode_encode`, `syToPar_injective`, `parToSy_wireable`) — `ocapn/src/par_value.rs`, tied by the `syrup` corpus (`node/tests/lean_syrup_corpus.rs`) |
| 60 | **An install is total on a channel.** An install that would replace a *different* installed continuation is refused (the space keeps one per channel, so a second `insert` used to replace the first silently), an idempotent re-install is a no-op, and what a channel carries after any permitted run is the continuation it was first given | `Rchain/Install.lean` (`installed`, `InstallStep`, `ReplacingStep`, `a_different_install_has_no_step`, `a_permitted_run_keeps_the_first`) — `rspace/src/hot_store.rs`, `rholang/src/system_processes.rs` |
| 61 | **A delivery in flight does not stall the loop.** With an answer outstanding the session's loop still reads the next delivery, and the outstanding answer is still written — deferring is not dropping | `Rchain/Session.lean` (`LoopState`, `Step`, `BlockingStep`, `a_pending_answer_does_not_block_the_next_delivery`, `a_deferred_answer_is_still_written`) — `ocapn/src/conn.rs` (`Reply::Deferred`), `ocapn/src/owner.rs`, `ocapn/src/bootstrap.rs` |
| 62 | **A peer cannot extend this node's reach.** The node never dials, on a peer's word, anything that peer could not dial itself, so a remote peer cannot aim this node at its own loopback services while a loopback peer still can | `Rchain/Perimeter.lean` (`Origin`, `Target`, `ownReach`, `permits`, and the defect's `originBlind`) — `ocapn/src/dial_policy.rs`, `ocapn/src/netlayer.rs`, `ocapn/src/enliven.rs`, `ocapn/src/fixtures.rs` |
| 63 | **A bridged delivery's footprint is attributable to a payer** — the state a session causes is one the peer that caused it pays for. **This row is `open`:** the close is a cross-implementation relay (the node hands the peer the exact `DeployData` to sign), which Endo cannot speak yet | `Rchain/Attribution.lean` (`Payer`, `Attributable`, `bridgedWrite`, `a_bridged_write_is_not_attributable`, `the_relay_would_satisfy_it`) — the sites it is about: `casper/src/shard_invoke.rs`, `node/src/api/ocapn.rs` |
| 64 | **Where the references disagree, the port speaks the implementations' reading and accepts every reading they produce.** The draft is not the oracle — the implementations are; and where they differ from each other the port accepts the union, because there is no single reading to speak | `Rchain/Interop.lean` (`Source`, `Form`, `Disagreement`, `speaks`, `accepts`, `the_port_speaks_the_implementations_reading`, `every_reference_reading_is_accepted`) — `ocapn/src/captp.rs`, `ocapn/src/session.rs`, `ocapn/src/bootstrap.rs`, `casper/src/shard_invoke.rs` |
| 65 | **A value with no faithful literal is refused, not written.** The printer writes a value into a term that is parsed back as code, and Rholang's literal grammar has no escape — so `check_renderable` refuses what it cannot write faithfully rather than signing a term whose body a peer chose | `Rchain/Literal.lean` (`Literal`, `Faithful`, `mayWrite`, `the_printer_writes_only_what_reads_back`, `an_unfaithful_literal_is_refused`, `the_quoted_string_case_is_refused`) — `rholang/src/pretty_printer.rs`, `casper/src/shard_invoke.rs` |

## The distributed-system set (Laws 66–71)

**What they are about.** Six rows that treat the node as a distributed system rather than as a language
plus a chain: the fringe record store, and the sync that carries state between nodes. Law 66 is an
*impossibility about the design* — the value a node stores at a fringe key is not determined by that
key, because the merge that produced it also started from a base the key never mentions, so two writers
under one key store different states and the join's tie-break silently settles it. Law 67 is the join
itself: a commutative, idempotent semigroup at one key (though not a monoid — it has no identity the
model fabricates), so the value at a key is a function of the set of arriving records. Law 68 is the
store property that makes a read well-defined: a read is a function of the version it names, and what a
store must be for that is **append-only, not content-addressed**. Laws 69–71 are the joiner's sync: the
state-page walk's termination and its give-up rule (69), that importing a walk of a root yields the trie
rooted there and the checker is sound (70), and that a catch-up window bounds what a joining node holds
so its progress does not depend on the network standing still (71).

| # | Law | Where it lives |
|---|---|---|
| 66 | **The value at a fringe key is not determined by that key.** The record holds a `state_hash` the key never mentions (the merge started from a base), and the join's `min` tie-break settles the collision — so a writer that had computed the other value finds its own block failing its own validation | `Rchain/Casper/Record.lean` (`the_fringe_only_key_is_incomplete`, `the_join_rewrites_a_writers_value`) — `models/src/fringe_data.rs`, `casper/src/dag.rs`, `casper/src/multi_parent_casper.rs` |
| 67 | **The record join is a commutative idempotent semigroup at one key** — so the value at a key is a function of the set of arriving records — **and it is not a monoid** (no identity). Order-independence does not come from the arrivals agreeing: `min` is commutative, so two records that *disagree* still fold to one | `Rchain/Casper/RecordJoin.lean` (`joinRecord_assoc`/`_comm`/`_idempotent`, `joinRecord_fold_perm`, `the_joined_value_is_a_function_of_the_set`, `the_join_changes_nothing_iff_the_report_adds_nothing`) — `casper/src/dag.rs`'s `join_fringe_records` |
| 68 | **A read is a function of the version it names**, and what a store must be for that is **append-only, not content-addressed**. Two content-addressed reads of one version agree because a collision is impossible; a store that only appends has the same property without hashing — the record store today is neither | `Rchain/Casper/ReadVersion.lean` (`two_content_addressed_reads_of_one_version_agree`, `a_read_of_an_append_only_store_is_stable`, `a_non_content_addressed_read_is_not_a_function_of_its_version`) — `rspace/src/history/radix_tree.rs`, `models/src/fringe_data.rs`, `tools/reconcile-network.sh` |
| 69 | **A joiner's state-page walk need not terminate, and with a give-up rule it is paced.** The walk as written exits only on an error or `is_finished`, so a peer that simply never answers leaves an infinite run; the idle give-up the block leg already has makes every turn spend fuel, giving a bounded exit | `Rchain/Sync/Walk.lean` (`the_page_walk_as_written_spins_for_ever`, `the_fixed_walk_cannot_run_for_ever`, `the_page_walk_with_the_give_up_rule_is_paced`) — `casper/src/engine/lfs_tuple_space_requester.rs`, `casper/src/engine/lfs_block_requester.rs`, `casper/src/engine/node_syncing.rs` |
| 70 | **Importing the walk of a root yields exactly the trie rooted there, and the checker is sound.** The walk delivers the reachable nodes however the pages are cut; a page-wise import whose pages pass their checks reads at the root what the peer reads there — the joiner is checking the peer, not trusting it | `Rchain/Sync/Transfer.lean` (`import_reads_the_rooted_trie`, `reads_determined`, `WalkListing.pages_are_the_nodes`) — `rspace/src/state/mod.rs`, `rspace/src/history/export.rs`, `rspace/src/history/radix_tree.rs` |
| 71 | **A catch-up window bounds what a joining node holds**, so its progress does not depend on the network standing still. The frontier is monotone along any run, stalls included; an answered window advances it one width; and the pending set is a function of the window and the frontier, never of how far the peer's tip has run | `Rchain/Sync/Progress.lean` (`run_monotone`, `answered_run_reaches`, `windowPending_independent_of_tip`, `gapPending_exceeds_the_window`) — `casper/src/engine/catchup.rs`, `casper/src/engine/node_running.rs` |

---

## The durable reset, the stop, and the read construction (Laws 72–77)

**What they are about.** Six rows about what survives a crash and what a reset means. Laws 72, 73 and 75
are the ordering of a durable act and its record: the reset's marker is written only once **every** effect
it brackets has been applied (72 — the DAG rewind, the body purge, the runtime's durable root, with the
boot repair re-applying the whole set from any half-applied state); a raised request is cleared by, and
only by, the step that applies the reset's effects (73), so a raised request is always still actionable;
and a dependent action is taken only on a committed record (75), so the request's premise is always
durable. Law 74 is the RSpace half of the same concern and it is a *negative*: a checkpoint does not
drain an overlay it has not folded, so an interrupted checkpoint loses nothing it has not committed — no
reachable state has an overlay empty while its changes are unfolded. Law 76 concludes it: a fault the
node cannot serve through, once reported, leaves it not serving and unable to return, because the report
and the stop are one step.

**Law 77 is the read path's, and it is the one a performance change was derived from.** A reset to a
root keeps the root it names and drops the overlay, so an install made *before* a reset to that root is
**unobservable** after it — and the construction that installs the system processes, resets to the
block's root and installs them again is the *same function* as the one that resets and installs once.
Its falsifier is the shape a **folding** reset would have, which is not a straw man: folding an overlay
into the trie rather than dropping it is what a checkpoint does, and Law 74 is the law that it must fold
what it drains. Under that rule the pre-reset install survives, so the law is about the reset's rule
rather than a restatement of a container (`spec/Rchain/Reset.lean`). The read path took the first shape;
the law is what made removing its second pass a derivation rather than a guess, and the measurement
afterwards is what says what it bought (`spec/audit/evidence/read-path-364/`).

## The genesis's developer allocation (Law 78)

**A row the oracle does not have, and the one economic term the chain decides at birth.** The Scala
ceremony installs exactly the vaults its wallets file names; this port derives one more from their total
— `max(20 000 000 REV, 2% of the allocated total)` — and credits it to a configured address **as a
vesting account, not a spendable vault**: Law 79 is the schedule it unlocks on. The two
arms cross at exactly one billion REV, so below that allocation the floor governs and at or above it the
rate does; on the allocation this network has (908 962 714 REV, dated 13 May 2026) the floor is the arm
that applies, and the wallets-plus-allocation total becomes 928 962 714 REV. The arithmetic is stated in
`spec/Rchain/Genesis.lean`, and the [economics page](../node/validator-economics.md) carries what the
allocation is and where it sits in the genesis.

**Why it is a law rather than a comment.** The balance is installed outside the genesis block's deploys
and content-addressed into its post-state hash, so a creator and a node **replaying** the genesis that
derive different amounts compute different hashes, and the replay refuses the block (AUDIT C46). *One
function of one file* is therefore the property that matters — and of the file's **installed** total,
since a repeated address is installed once — and it is the property the row states: the amount is never a
setting, only the address is, and an address that is set must parse, refused at startup rather than at
genesis. A node that restores the chain over LFS does not replay genesis; the restore-completion guard
enforces its local claim when it has one and trusts the bootstrap when it has none. That boundary
belongs to the restore path and is stated in [the economics page](../node/validator-economics.md). The deviation from the oracle is recorded with it
(`spec/audit/passes.md` §138). The falsifier is that the two crossover directions would contradict each
other if the rule were mis-stated, and the non-vacuity witness is that the rate strictly exceeds the floor
above the crossover — without which both directions would hold of any constant.

## The allocation's schedule (Law 79)

**A second row for the same deal, because "how much" and "when" are different claims.** Law 78 fixes the
amount; on its own that is an allocation credited to a vault at genesis and therefore *spendable* at
genesis — an immediate claim on the whole thing, which is the shape a reviewer called a sell overhang.
So the genesis credits it to a **vesting account** (`pos:dev-vesting`: the recipient, the total, and the
unreleased balance in one record), and each epoch boundary moves what has vested into the recipient's own
vault. Once there it is an ordinary balance, and no other rule in the chain has to know where it came
from.

**The rule is linear and runs one way.** The vested fraction is the schedule position: zero at genesis,
total at the end, truncated toward the network so the released amount never runs ahead. The release moves
`vested − released`, so the two theorems that matter are the bound — `devTaxVested_le_allocation`, which
keeps the recipient's vault bounded by the allocation however long the chain runs — and the monotonicity,
which keeps that difference from going negative. A duration of `0` is *no schedule at all* rather than a
zero-length one, and that is what a params record written before the field decodes to.

**The term is a parameter, not a constant.** `dev-tax-vesting-blocks` is a genesis parameter carrying 36
months at the cadence `defaults.conf` states (5 s/block, the conservative end of the measured band) —
the arithmetic is [`spec/Rchain/Vesting.lean`](../../../spec/Rchain/Vesting.lean), and the term is the
deal's. That split is deliberate: this tree records that **a block cadence is not a constant**
([scaling](../node/scaling.md)), so a "month" is only ever an assumption, and a parameter makes the
assumption visible and settable at launch rather than buried in the release rule. The release happens at
**epoch boundaries** rather than every block, because native state is written at boundaries — stated
because a reader comparing the schedule to a balance needs to know which clock the balance follows.

## Reading the formalization

- **Lean** (`spec/Rchain/*.lean`) owns the algebraic/order laws and the type-system fundamentals. Build:
  `cd spec && lake build`. An `axiom` with a named reason is a *stated* obligation, not a hole: the gate
  refuses a `sorry`, so an unproven claim has to be visible in the source as an axiom and in
  `spec/INVENTORY.md` as a status.
- **Coq** (`spec/coq/*.v`) owns the substitution / α-equivalence metatheory (Laws 2–6). Build:
  `make -C spec/coq`.
- **K** (`legacy/rholang/src/main/k/rholang/*.k`) is the executable reference semantics of the language.
- **Rust** carries each invariant *structurally* (refinement newtypes, no silent partiality).
- **The corpora** (`spec/conformance/*.tsv`) are the bridge: they are *emitted* from the Lean definitions
  (`lake exe rchain-corpus --layer <l>`), committed, and read by a Rust consumer that runs the same cases
  through the node and must agree 1:1. A law whose definition changes without its corpus being re-emitted
  fails the gate rather than drifting quietly.

### The gates

| Command | What it refuses |
|---|---|
| `tools/check-lean-conformance.sh` | a failed Lean or Coq build, a `sorry`/`admit`, a module `Rchain.lean` does not import, a **stale or untracked corpus**, a corpus with no consumer, a consumer that disagrees with its corpus, and a law-39 catalog urn with no row in `spec/API-SCHEMA.md`. It is the `formal` job in the nightly. |
| `tools/audit-type-system.sh` | production `panic!`/`unsafe`/silent conversion — the no-silent-partiality discipline |
| `tools/audit-status.sh` | a check-off that disagrees with `spec/findings.tsv`, a `todo` that names nothing, a `done` row whose evidence resolves to nothing |

**A third gate used to be here** — `tools/audit-test-register.sh`, which refused a register that
overstated the tree, a named test that did not exist, and a law row claiming coverage without naming a
Lean module, a corpus and a consumer that exist. It was deleted on 2026-09-27: 1,453 lines, 78.5
seconds, and every failure meant a document disagreed with another document.

The gates answer "would this have been caught?"; the question *"which law would have caught the
eleventh failure?"* is answered by `spec/audit/passes.md` §20, which maps every incident to its law
and its case.

Per-law status, source-of-truth pointers, and Rust realization are in
[`spec/INVENTORY.md`](../../../spec/INVENTORY.md); the machine-readable rows are
[`spec/laws.tsv`](../../../spec/laws.tsv).
