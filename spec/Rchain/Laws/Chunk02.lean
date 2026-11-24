import Rchain.Laws.Base

/-!
# The law register, chunk 02 (laws 15–29)

One module of the register catalog. The register is split across modules because elaborating it as a
single module peaked at ~15 GiB on the 16 GiB CI runner (measured 2026-11-23); a module boundary is a
process boundary, so each chunk's elaboration memory is bounded. The register's numbering check
(`1..lawCeiling`) and `entryCeiling` make a dropped or duplicated row across chunk boundaries a build
failure. See `Rchain/Laws.lean` for the assembly and the measurement.
-/

set_option autoImplicit false
set_option maxHeartbeats 4000000

namespace Rchain
namespace Laws

def lawsChunk02 : List Law := [
  { number := 15, layer := "Casper",
    rustWitness := ["block-storage/src/property_tests.rs:law15_adding_blocks_only_grows_the_state"],
    statement := "The fringe is monotone by height **per sender** and the seen set is monotone (no \
      regression) — the **derived** fringe and the **constructed** seen set, the height claim **under the \
      ingress rules the port enforces at admission** (fork-freedom and the sequence rule, both refused \
      rather than observed: H-1's equivocation gate, `sequence_number`, \
      `check_justification_regression`); over bare values both claims are false and their refutations are \
      proved, and the **cross-sender** reading of the height claim is false too, of this tree *and* of the \
      oracle (`cross_sender_height_monotone_is_false`)",
    status := .provedModel,
    declarations := [`Rchain.Message, `Rchain.seenOf, `Rchain.Reaches,
      `Rchain.seen_monotone_of_reaches, `Rchain.seen_monotone_is_false,
      `Rchain.fringe_monotone_is_false, `Rchain.seenOf_contains_justifications, `Rchain.mem_seenOf_self,
      `Rchain.seen_subset_of_mem_seen, `Rchain.selfParents_skips_finalized,
      `Rchain.cross_sender_height_monotone_is_false, `Rchain.chain3,
      `Rchain.the_walk_is_oldest_first, `Rchain.a_chain_of_three_picks_the_oldest,
      `Rchain.ReachesF, `Rchain.NoFork, `Rchain.ReachesF.step, `Rchain.ReachesF.trans,
      `Rchain.selfParents_reaches, `Rchain.selfParents_height_lt, `Rchain.nofork_of_unfiltered,
      `Rchain.nofork_ancestors_go_through_the_parent, `Rchain.selfParents_above_a_finalized_ancestor,
      `Rchain.fork4, `Rchain.the_comparison_is_false_without_fork_freedom, `Rchain.fold4,
      `Rchain.minMsgs_fold4, `Rchain.fold4_is_fork_free,
      `Rchain.fold5, `Rchain.fold5_is_fork_free, `Rchain.fold5_satisfies_the_sequence_rule,
      `Rchain.the_fold_can_publish_below_the_previous_fringe, `Rchain.seedLayer,
      `Rchain.insertCandidate, `Rchain.seedLayer_nodup, `Rchain.insertCandidate_nodup,
      `Rchain.the_guard_keeps_the_newer_message, `Rchain.the_comparison_holds_on_fold5, `Rchain.SeqUnique,
      `Rchain.SeqStep, `Rchain.seqNum_lt_height_lt, `Rchain.entryFor, `Rchain.find?_sender_eq,
      `Rchain.insertCandidate_height_le, `Rchain.find?_filter_ne, `Rchain.entryFor_layerInsert_ne,
      `Rchain.entryFor_insertCandidate_ne, `Rchain.entryFor_insertCandidate_self,
      `Rchain.entryFor_insertCandidate_refused, `Rchain.entryFor_insertCandidate_some,
      `Rchain.entryFor_insertCandidate_none, `Rchain.entryFor_insertCandidate_le,
      `Rchain.mem_layerInsert, `Rchain.mem_insertCandidate, `Rchain.seedLayer_mem, `Rchain.parents_mem,
      `Rchain.foldl_insertCandidate_height_le, `Rchain.nextLayer_height_le_seed,
      `Rchain.mem_of_head?, `Rchain.reachesF_sender, `Rchain.selfParents_sender, `Rchain.minMsg_sender,
      `Rchain.minMsgs_mem_d, `Rchain.minMsgs_above, `Rchain.find?_sender_eq_nat,
      `Rchain.foldl_layerInsert_mem, `Rchain.seedLayer_entryFor_mem, `Rchain.seedLayer_entryFor_sender,
      `Rchain.foldl_insertCandidate_entryFor_none, `Rchain.nextLayer_above_the_previous_fringe,
      `Rchain.derivedFringe_above_the_previous_fringe, `Rchain.descendsB, `Rchain.descendsB_iff],
    axioms := [],
    rust := ["block-storage/src/dag/message_state.rs", "block-storage/src/dag/finalizer.rs"],
    witness := [`Rchain.fringe_monotone_is_false, `Rchain.seen_monotone_is_false,
      `Rchain.cross_sender_height_monotone_is_false, `Rchain.a_chain_of_three_picks_the_oldest,
      `Rchain.selfParents_above_a_finalized_ancestor,
      `Rchain.the_comparison_is_false_without_fork_freedom, `Rchain.fold4_is_fork_free,
      `Rchain.the_fold_can_publish_below_the_previous_fringe, `Rchain.fold5_is_fork_free,
      `Rchain.fold5_satisfies_the_sequence_rule, `Rchain.the_guard_keeps_the_newer_message,
      `Rchain.the_comparison_holds_on_fold5, `Rchain.seqNum_lt_height_lt,
      `Rchain.insertCandidate_height_le, `Rchain.minMsgs_above, `Rchain.nextLayer_height_le_seed,
      `Rchain.nextLayer_above_the_previous_fringe, `Rchain.derivedFringe_above_the_previous_fringe,
      `Rchain.d15_the_layer, `Rchain.d15_the_published_entry, `Rchain.d15_descends,
      `Rchain.d15_is_fork_free, `Rchain.d15_is_sequence_unique, `Rchain.d15_satisfies_the_sequence_rule,
      `Rchain.d15_the_comparison_holds],
    falsifiable := some "`fringe_monotone_is_false` exhibits two overlapping fringes (one at 5 and 1, one \
      at 3) where both arms of the disjunction fail — so the axiom was false as written; \
      `seen_monotone_is_false` exhibits two unrelated messages where `b` sees `a` and `a` sees `2` but \
      `b` does not. The constructive half is falsifiable too: `seenOf_contains_justifications` fails for \
      a `seenOf` that dropped the justifications' sets, which is the port's `new_seen` \
      (`message_state.rs:54-59`). **The comparison's own hypothesis is falsified rather than assumed**: \
      `the_comparison_is_false_without_fork_freedom` (`fork4`: `p` with a finalized parent at height 5 \
      and a second same-sender branch that reaches height 1 without passing through it) makes the \
      statement **false** of a DAG with a fork, so the theorem is about fork-free DAGs and `NoFork` is \
      what the ingress refusal (H-1, AUDIT C82/C84) supplies. **And the lift's own hypotheses are \
      falsified where they are dropped**: `the_fold_can_publish_below_the_previous_fringe` states the \
      conclusion's negation on `fold4`, a **fork-free** DAG whose `101` justifies the older `99` — so \
      fork-freedom alone does not carry the comparison, and the sequence rule is what refuses that shape; \
      and the fold's guard is load-bearing by a **one-word mutation** — `insertCandidate`'s \
      `if cur.seqNum < m.seqNum` replaced by `true` — under which `fold5` publishes the stale `c` where \
      the port publishes `z` (`the_guard_keeps_the_newer_message`)",
    note := "**two more false axioms, both refuted in the tree.** The content is the *derivation*: a \
      message's seen set is **constructed** as the union of its justifications' seen sets plus its own id \
      (`message_state.rs:54-59`), which the model now has (`seenOf`, with both halves proved: \
      `seenOf_contains_justifications` and `mem_seenOf_self`); and height monotonicity relates \
      *successive* fringes of one validator, which the finalizer's advance gate produces \
      (`finalizer.rs:275-317`, `:361-363`). **The transitive closure is proved, along the relation that does not need a lookup** (2026-09-24): \
      `Reaches a b` (a is a justification of b, or of a justification that reaches it) gives \
      `a.seen ⊆ b.seen` by induction (`seen_monotone_of_reaches`), and the hypothesis it needs — that a \
      message's seen set *is* `seenOf` of its justifications — is the port's own construction \
      (`message_state.rs:54-59`), stated rather than assumed. Falsified by mutation: a `seenOf` that \
      drops the union breaks both the one-step lemma and this proof's two steps. **And both things this \
      form did not cover arrived with the DAG model** (2026-09-24, `Rchain/Casper/Dag.lean`): the \
      literal id-based form is `seen_subset_of_mem_seen`, over `Dag.msg` — the id→message map this note \
      said the model would have to bring — with the port's construction (`Constructed`) and the DAG's \
      ordering facts (`Descends`: a parent resolves in the DAG and is strictly lower) as **hypotheses \
      rather than axioms**, because resolving an id back to a value needs uniqueness and a finite \
      descent. **And the descent is *enforced* rather than assumed** (H1b, `46c35b545`): `blockNumber` \
      refuses a block that names a resolved parent at or above its own number, the **failed** ones \
      included (`casper/src/validate.rs:226-230`), so `Descends` holds of **every state the port \
      admits** — where the reference validator, which filters failed parents out of its own \
      `blockNumber`, admits the violating block, the §6 deviation AUDIT C83 carries. The earlier plan \
      to weaken this to *unfailed* parents is therefore **superseded**: a weakened `Descends` forces \
      `unfailed` into `seen_subset_of_mem_seen`'s descent step, while the strong form is what the \
      ingress check guarantees. The per-sender reading of the height claim rests on `selfParents_skips_finalized`: the \
      walk filters by `!finalized.contains(x)` **and never traverses through an excluded message** (the \
      worklist is rebuilt from the survivors), so a min message is the previous sentinel's direct \
      successor rather than a descendant of something older. **And that settled which reading of the \
      height claim is true**: the cross-sender one is **false**, of this tree and of the oracle — \
      `prev = {m3 (sender 0, height 3)}` against a published `{q2 (sender 1, height 2)}`, because the \
      layer is built from the *justifications* and need not cover the senders `prev` covers, and the \
      gate cannot see it: what `calculate_fringe` reads is the **support map**, while \
      `Finalizer.scala:144-178` compares fringes *never* \
      (`LazyList.unfold(parentFringe)(nextFringe(_).map(nf => (nf, nf))).lastOption`). It is kept as \
      `cross_sender_height_monotone_is_false` rather than dropped, so the next reader sees which \
      reading was false and on what; AUDIT C69 carries the measurements, and §6 the port's own \
      termination guard — which the oracle does not have, and which is weaker than a cycle guard. \
      **What remains owed** is the last conjunct: the per-sender height *comparison* itself, the \
      mechanism proved and the arithmetic across a chain not — a small named remainder rather than the \
      two-sided gap this row used to carry. **And the statement it needs is sharper than it looks**: as \
      written the comparison is *false*, because the walk takes **every** same-sender parent, so a \
      same-sender fork on `mv`'s frontier puts a message from the other branch in its output, and that \
      branch is not below any given sentinel. **And the port does not merely happen to be fork-free — it \
      *refuses* a same-sender fork at ingress**, which is stronger than the datum this sentence used to \
      assert and is what the proof's hypothesis can lean on: H-1's equivocation detection rejects a \
      second distinct block by one sender reusing a `seq_num`, **before any partial write** \
      (`casper/src/dag.rs:244-252`, test `insert_rejects_equivocation_same_seq_num` at `:568`); \
      `sequence_number` requires a block to justify a same-sender block exactly one `seq_num` lower \
      (`casper/src/validate.rs:245-262`, test `sequence_number_must_be_creator_latest_plus_one` at \
      `:813`); and `check_justification_regression` admits at most one justification per sender and \
      demands that it be the latest (`casper/src/validate.rs:267-279`). **Why the port refuses it, and \
      why that refusal is a deviation the oracle does not share, is AUDIT C84** — the Scala has no \
      equivocation gate anywhere and its `validateDagState` checks only height contiguity, so the \
      premise this proof needs is **guaranteed here and only observed there**. H-1 landed **2026-08-20** \
      (`76415d6c6`), a month *before* the sentence it corrects was written — so the premise was carried \
      in good faith rather than checked, which is this programme's recurring class. The hypothesis is \
      therefore \"at most one same-sender parent per message\" — **guaranteed by an ingress refusal** \
      rather than observed of the data — and with it the comparison \
      follows from the boundary plus the descent. That is the shape of the last conjunct, named so \
      the next pass proves a true statement rather than a forked counterexample. The old row's claim that the seen set is monotone \"(no \
      regression)\" was true of the port and false of the value the axiom quantified over. **And the \
      owed conjunct's *step* was wrong until 2026-09-25** (AUDIT C93): `minMsgs` takes the *oldest* \
      non-finalized same-sender ancestor — the port's `chain.into_iter().last()` over \
      `[p] ++ self_parents p` — and the model read `getLast?` of `selfParents`, whose list is \
      **oldest-first** (the model prepends into its accumulator where the port `push`es its visit order, \
      so the port's chain is newest-first and its `.last()` is the oldest). Measured on a chain \
      `10 (h 0) ← 11 (h 1) ← 12 (h 2)`: the model's `selfParents` is `[10, 11]`, the port's chain is \
      `[11, 10]`, and the model answered `11` where the port answers `10` — the *newest* for the oldest. \
      **No test could see it**: every `decide`d instance in the file gave a sender at most one \
      same-sender ancestor, and with one element both ends agree, so a step of the derivation read the \
      opposite end of the chain while every check stayed green. The fix is `.head?`, pinned by \
      `chain3`'s two theorems (`the_walk_is_oldest_first` and `a_chain_of_three_picks_the_oldest`, the \
      instance that fails under `getLast?` — verified by reverting it), and the antichain above is \
      unaffected: which of two same-sender messages the layer fold keeps does not change its keys. \
      **What remains owed is still the comparison itself**, and it is now **one step narrower, with the \
      step named** (2026-09-25). A sentence here briefly said the obstacle was a *missing model* — that \
      `nextFringe` takes the next layer as an argument (`Fringe.lean:110`) so no two successive fringes \
      are related; **that was wrong**, a reading of `Fringe.lean` alone: `Rchain/Casper/Dag.lean`'s \
      `derivedFringe` derives the layer from the DAG *and* takes `prev` (`:140`), so the successive-fringe \
      relation exists. What is now **proved**, about the walk that derivation rests on, is the **sentinel \
      half**: `selfParents_above_a_finalized_ancestor` — for a justification, `minMsgs` returns a message \
      **strictly above every finalized same-sender ancestor of it** — under `NoFork`, the fork-freedom \
      ingress *refuses* rather than assumes (H-1 at the DAG insert, `sequence_number`, \
      `check_justification_regression`; AUDIT C82 and C84). Two things make it more than a spelling: the \
      walk's descent is proved first (`selfParents_height_lt`: everything the walk returns is reached \
      from the seed along same-sender parent edges, so `Descends` makes it strictly lower), and the \
      hypothesis is **falsified where it is dropped** — `the_comparison_is_false_without_fork_freedom` \
      exhibits a four-message fork where the walk steps *around* the finalized ancestor and returns a \
      height-1 message beneath a height-5 one. **What is still owed is the lift through the fold, and \
      *which* ingress rule it needs is a question two refutations narrowed without settling** — both \
      machine-checked rather than argued. `nextLayer` adds the min messages' *candidate parents* whose \
      sender is already a key (`:129-135`), and **neither fork-freedom nor the sequence rule excludes a \
      published message below the previous fringe**: `fold4` (two sender-0 blocks, `99` at height 2 and \
      `100` at height 5, neither with a parent, and `101` justifying `99`) is **fork-free** and still \
      publishes `99`, and `fold5` answers the next guess by adding `z` (seq 6) justifying `q` (seq 5) — so \
      every same-sender edge has consecutive `seqNum`s and the sequence rule holds — and it *still* \
      publishes the lower `c`, because `cands` is folded right-to-left and a candidate from an earlier \
      justification wins the sender's slot over the min message \
      (`the_fold_can_publish_below_the_previous_fringe`, with \
      `fold4_is_fork_free`, `fold5_is_fork_free` and `fold5_satisfies_the_sequence_rule` proving each \
      instance *has* the hypothesis it tests — the two refutation instances, and the two checks on the \
      fold that now mirrors the port). **And the fold itself was the thing in the way, which is now fixed rather than argued about.** The port's `calculate_next_layer` gives a candidate its sender's slot **only when its `sender_seq` is strictly greater** (`block-storage/src/dag/finalizer.rs:160-166`), and the port's own comment calls that guard *\"the Law 15 monotonicity invariant\"* (`message_state.rs:77`). `layerInsert` did not: it is the *seeding* rule (the port's `BTreeMap::collect`, last wins) and the model used it for the candidates too, justified by *\"the antichain does not depend on that choice\"* — true of law 14b's **keys**, false of the **message** law 15 is about. `seedLayer` and `insertCandidate` now carry both rules, and the difference shows on this file's own instance: `dag3`'s published layer was [10, 12] and is now [11, 12] — the model was publishing a **stale** same-sender message, which no law-14b check could see because the *keys* were right either way. **With the faithful fold the counterexample is `fold4` and nothing else**, stated per sender as law 15 states it: `the_fold_can_publish_below_the_previous_fringe` holds of a fork-free DAG whose `101` justifies the older `99` while `100` is finalized — a shape the **sequence rule** refuses, since `101`'s same-sender justification is not its sender's latest. `fold5` refutes nothing now: with the guard it publishes `z` (height 6) for sender 0 and the comparison holds (`the_guard_keeps_the_newer_message`, `the_comparison_holds_on_fold5`). **So the lift's hypothesis is the sequence rule, on a fold that mirrors the port** — which is what this row said before two of its own corrections, and it is now what the machine says rather than what a guess said. **And the lift is proved** (2026-09-25), so the row is no longer owed anything: the last two ingredients \
      are `seqNum_lt_height_lt` (on a sender, a higher `seqNum` is a higher height — from `SeqStep`, the \
      port's `sequence_number`, with `SeqUnique`, H-1's one-message-per-`seq_num`) and \
      `insertCandidate_height_le` (**the guarded insertion never lowers a sender's entry** — the port's \
      *\"Law 15 monotonicity invariant\"* as a theorem about this model's own fold). The composition is \
      `foldl_insertCandidate_height_le`: the step over the **whole** candidate list, whose accumulator is \
      generalized because the entry between two insertions is neither the seed's nor the layer's and it is \
      the one the next step's guard is about — and `nextLayer_height_le_seed` reads it where the row \
      needs it: a sender's entry in the layer `nextLayer` **publishes** is at a height no lower than the \
      **seeded** one. The comparison against `prev` is `minMsgs_above`, and the relation it needs is \
      exactly the one this row had named: every justification of a sender descends from the previous \
      fringe's message for that sender — the port's `sequence_number` plus \
      `check_justification_regression`, stated as the **hypothesis** it is rather than assumed, because \
      it is a fact about what a block may justify and not about the DAG value. \
      `nextLayer_above_the_previous_fringe` composes the two halves (per sender, the published layer lies \
      **strictly above** the previous fringe's message), and \
      `derivedFringe_above_the_previous_fringe` reads it at the `Fringe` `derivedFringe` returns — law \
      15's sentence with its hypothesis named. **What makes the hypotheses more than spelling**: `d15` \
      carries all four and a derivation that publishes (`d15_descends`, through a `Bool` mirror because \
      `Descends` quantifies over every message; `d15_is_fork_free`, `d15_is_sequence_unique`, \
      `d15_satisfies_the_sequence_rule`, each `decide`d), `d15_the_published_entry` pins the entry the \
      layer holds for the sender, and `d15_the_comparison_holds` is the theorem **applied** to it — \
      height 1 against height 2 — so no hypothesis is idle. The `rustWitness` is unchanged and stays a \
      **property test** (`block-storage/src/property_tests.rs:law15_adding_blocks_only_grows_the_state`): \
      this row's statement is the derivation-level fact, and the tie to the state it is tested against is \
      the port's own guard, which the fold above is now the model of rather than an approximation to" },
  { number := 16, clause := "a", layer := "Casper",
    rustWitness := ["casper/src/validate.rs:block_number_must_be_parent_max_plus_one"],
    statement := "Block number = max(parent) + 1 — as the port's check, which **rejects** a block whose \
      number is not one more than the maximum of its non-failed justifications (`0` when there is none \
      live)",
    status := .provedTied,
    corpus := some "block",
    declarations := [`Rchain.Parent, `Rchain.maxParentNumber, `Rchain.BlockNumberValid,
      `Rchain.block_number_max_parent_plus_one, `Rchain.block_number_rejects,
      `Rchain.block_number_universal_is_false],
    axioms := [],
    rust := ["casper/src/validate.rs"],
    witness := [`Rchain.block_number_rejects, `Rchain.block_number_universal_is_false],
    falsifiable := some "`block_number_rejects` is the case the port returns `InvalidBlockNumber` for \
      (`validate.rs:236-240`): an off-by-one — `max + 2`, or `max` itself — fails it. The `-1` seed is \
      falsifiable on its own: a block with no live justification must be numbered `0`, so a model that \
      folded a maximum from `0` would demand `1` and reject the genesis-shaped case. And \
      `block_number_universal_is_false` exhibits the refutation of the axiom that stood here — which \
      quantified over every `Block` with no hypothesis at all",
    note := "**the axiom was false, not merely unproven**: it quantified over every `Block`, and a \
      `Block` is freely constructed, so one line refutes it (`block_number_universal_is_false`). The law \
      is re-scoped to the check the code has — a fold over the block's justifications, skipping the \
      failed ones and seeded `-1` (`validate.rs:222-234`) — and the proof is that predicate's \
      elimination, which is the honest shape: the port enforces this by *refusing blocks*, not by \
      maintaining an invariant it states. The model's `Block` carries `justifications` because the check \
      reads them; the `parents : List Nat` field this row's model used does not exist in the port. \
      **Witness column (AUDIT C140).** This witness was strengthened on 2026-09-27. It asserted only \
      that the seen set does not shrink and that the counts do not fall, and a DAG state that never \
      changes satisfies both: making `add_block_to_dag_state` return its input left it green \
      (measured), while five of law 18's tests and `add_block_to_dag_state_builds_child_map` went red. \
      It now requires the block that was added to be *in* the new seen set and its height to be *in* \
      the index — the growth the law is about, rather than its direction" },
  { number := 16, clause := "b", layer := "Casper",
    rustWitness := ["casper/src/validate.rs:sequence_number_must_be_creator_latest_plus_one"],
    statement := "`seqNum` strictly increases **under the sender's justification**: the port requires the \
      block's `seqNum` to be one more than the maximum `seqNum` among the justifications whose sender is \
      this block's sender (`0` when there are none)",
    status := .provedTied,
    corpus := some "block",
    declarations := [`Rchain.senderLatestSeq, `Rchain.SeqNumValid, `Rchain.seq_num_strictly_increases,
      `Rchain.seq_num_universal_is_false],
    axioms := [],
    rust := ["casper/src/validate.rs"],
    witness := [`Rchain.seq_num_universal_is_false, `Rchain.seq_num_strictly_increases],
    falsifiable := some "a block whose `seqNum` skips or repeats the sender's latest justification is \
      rejected (`InvalidSequenceNumber`, `validate.rs:259-262`), and the `-1` seed is a case of its own: \
      a sender's first block must be `0`. `seq_num_universal_is_false` is the published refutation of \
      the axiom this replaces — and it refutes it **for a single sender**, which is why the re-scoping \
      is the justification relation and not the sender relation",
    note := "**the axiom was false as stated** (`prev.seqNum + 1 = next.seqNum` over any two blocks), and \
      the diagnosis in the row it replaces — \"the sender relation is missing\" — was wrong: a same-sender \
      pair with a non-consecutive `seqNum` refutes it just as well \
      (`seq_num_universal_is_false`). What the check folds over is the block's justifications **whose \
      sender matches**, against their maximum (`validate.rs:245-262`); the law is re-scoped to that, and \
      the proof is the predicate's elimination. The old model also carried a `seqNum`-ordering axiom over \
      any two blocks, which no port rule states" },
  { number := 16, clause := "c", layer := "Casper",
    rustWitness := ["models/src/casper/protocol/casper_message.rs:law16_to_proto_sorts_justifications"],
    statement := "Content addressing: `hash_block` clears `block_hash` and `sig` and hashes every other \
      proto field canonically, so equal hashes determine equal bodies **that the serializer can \
      represent** (`Canonical`: numbers inside `int64`, justifications in the port's sorted order)",
    status := .provedTied,
    corpus := some "body",
    declarations := [`Rchain.Block, `Rchain.BlockBody, `Rchain.Block.body, `Rchain.Parent.key,
      `Rchain.Canonical, `Rchain.sortParents, `Rchain.sortParents_of_pairwise, `Rchain.encodeParent,
      `Rchain.decodeParent, `Rchain.decodeParent_encodeParent, `Rchain.decodeParents,
      `Rchain.decodeParents_length_encodings, `Rchain.encodeBody, `Rchain.canonicalise,
      `Rchain.taggedVarint, `Rchain.taggedVarint_varint, `Rchain.taggedVarint_varintField,
      `Rchain.taggedVarint_bytesField, `Rchain.decodeBody, `Rchain.decodeBody_encodeBody,
      `Rchain.encodeBody_injective,
      `Rchain.the_body_encoder_is_not_injective_without_canonical,
      `Rchain.blockHash, `Rchain.content_addressing, `Rchain.blockHash_changes_with_header,
      `Rchain.a_body_encoder_that_truncates_is_not_injective,
      `Rchain.a_body_encoder_that_canonicalises_is_not_injective],
    axioms := [],
    rust := ["casper/src/proto_util.rs", "models/src/casper/protocol/casper_message.rs"],
    witness := [`Rchain.the_body_encoder_is_not_injective_without_canonical, `Rchain.a_body_encoder_that_truncates_is_not_injective, `Rchain.a_body_encoder_that_canonicalises_is_not_injective, `Rchain.content_addressing, `Rchain.blockHash_changes_with_header],
    falsifiable := some "`content_addressing` composes Law 19's `blake2b256_collision_free` with \
      `encodeBody_injective`, so a serializer that dropped a field would falsify it — and \
      `blockHash_changes_with_header` is the code's own `hash_block_changes_with_timestamp` \
      (`proto_util.rs:155-162`) derived: two blocks differing in **any** hashed field must hash \
      differently, which is why the informational timestamp is in the body even though no consensus rule \
      reads it. A `hash_block` that stopped clearing `sig` would falsify \
      `hash_block_is_deterministic_and_ignores_sig` (`:138-144`) and, transitively, this. **And the \
      canonicity axiom was falsified before it was narrowed**, two ways, because the model's body is \
      wider than the bytes the port writes: \
      `a_body_encoder_that_truncates_is_not_injective` (the proto's `blockNumber` is an `int64`, \
      `models/proto/casper.proto:49`, so any encoder that mirrors it identifies `2^63` with `2^63 + 2^64`) \
      and `a_body_encoder_that_canonicalises_is_not_injective` (`to_proto` sorts the justifications \
      before hashing, `casper_message.rs:636`, so it cannot distinguish a body from the same body \
      permuted). **And the axiom is gone**, so the refutation is now about the *definition the port \
      runs* rather than about a hypothesis: `the_body_encoder_is_not_injective_without_canonical` \
      exhibits `2^63` and `2^63 + 2^64` as one image of `encodeBody` itself, and `Canonical`'s number \
      bound is what recovers the value from the residue (`int64_eq_of_lt`, `Rchain/Proto.lean`). Drop \
      `Canonical` from `encodeBody_injective` and the proof breaks at `decodeBody_encodeBody`, which \
      returns `canonicalise b` — a different body exactly when the hypothesis fails",
    note := "**the old row was a postulate about a field no function computed** — `hash = \
      Blake2b256(block − {hash, sig})` over `Block.hash : Nat`, which the note below it admitted was not \
      true even of its own model (an injective `Nat → Nat` does not exist). `Block.hash` is gone; the \
      hash is *computed* (`blockHash`), and the content addressing is a **theorem**: the composition of \
      Law 19's collision-freedom with the serializer's canonicity. The two axioms that remain are Law \
      10's shape and for the same stated reason — a hash can make an encoding collision-resistant but \
      never canonical, so `encodeBody`'s injectivity is the serializer's assumption, not the hash's. \
      The body is the **hashed** body: the port hashes every field except `block_hash` and `sig` \
      (`proto_util.rs:58-64`, `BlockMessage` at `casper_message.rs:563-585`), so the model carries the \
      timestamp and the rest of the header, and `blockHash_changes_with_header` is the statement a \
      narrower body could not make. **What the third pass found (2026-09-23)**: the canonicity axiom \
      was still stated over types wider than the port's — a `Nat` block number against the proto's \
      `int64`, and `justifications` in arrival order against the sorted order `to_proto` writes — so it \
      was false of any encoder the port could be using, and the two refutation theorems above say so \
      with witnesses. The statement was narrowed to `Canonical` rather than the model widened (the \
      widths and the sorted order are properties of the *code*, which the model's validation rules \
      deliberately do not carry, since they fold over the list in any order). **The fourth pass \
      (2026-09-23) discharged it**: `encodeBody` is now a **definition** (`Rchain/Casper/Validate.lean`) \
      over protobuf's wire format (`Rchain/Proto.lean`: the base-128 `varint` and its self-delimiting \
      decode, `varintField`, `bytesField`, the 64-bit narrowing), with the round trip \
      (`decodeBody_encodeBody` — the encoder's own inverse returns `canonicalise b`, which is the \
      hypothesis's whole content) and injectivity (`encodeBody_injective`) as theorems about it, so this \
      row's axiom citation is empty. **What is still prose, and is named rather than implied**: `prost`'s \
      `encode_to_vec` is an external crate (0.13.5, `Cargo.lock`), so \"these bytes are the node's \
      bytes\" is a tie no Lean theorem can make — and it is now a tie the row **carries**: `corpus := \
      some \"body\"`, the layer that was the follow-up when this sentence was written (`Corpus.lean`'s \
      `bodyCases`, `emit-lean-corpus.sh`'s `LAYERS`, `spec/conformance/body.tsv`, the consumer \
      `node/tests/lean_body_corpus.rs`, the gate's layer-to-consumer map). The row is `proved-tied` on \
      it. **What the layer had to overcome was measured before it was built rather than after** (AUDIT \
      C57): the model's `encodeBody` and `prost` disagree in three ways that a byte comparison hits on \
      its first case — the model writes tags 4, 6, 5, 17, 9, 14 where `prost` writes the `.proto`'s \
      ascending order; `prost` omits default-valued fields and the model writes unconditionally; and the \
      model collapses nine fields \
      into one opaque `header` blob at tag 14, which `prost` reads as `state`. **What the layer covers is \
      the subset where the two structures correspond** — six fields, with the fields it does not model \
      and `justifications`' shape named as its boundary in the consumer's own doc — so what remains is \
      C57's \
      *option (a)*: mirroring `prost` over the full field list is a modelling change and not a plumbing \
      one, and it would put `decodeBody_encodeBody`/`encodeBody_injective` in play for those fields too. \
      C57 records the three \
      divergences, the compile-time hazard below, and the bounded alternative. **A \
      measurement worth keeping**: the first spelling of `decodeBody` was a `guard`-per-tag do-block, \
      and it did not slow the *elaborator* down — it slowed the **compiler** down: `lean --profile` \
      reports 128 ms of elaboration and `compilation of Rchain.decodeBody took 98.6s`, with the build \
      swelling past 5 GB until it aborted. One `taggedVarint` per field compiles in 224 ms" },
  { number := 16, clause := "d", layer := "Casper",
    statement := "The bonds cache equals the PoS state",
    status := .provedModel,
    declarations := [`Rchain.ActiveBonds, `Rchain.bondsOfState, `Rchain.Justification,
      `Rchain.newestJustification, `Rchain.bondsFromNewestState, `Rchain.bondsFromCarried,
      `Rchain.the_sources_agree, `Rchain.a_disagreeing_set_is_refused,
      `Rchain.a_bond_change_between_justifications_is_refused,
      `Rchain.a_bond_does_not_move_the_map_the_gates_read,
      `Rchain.the_pool_derived_map_moves_where_the_states_does_not],
    rust := ["casper/src/multi_parent_casper.rs", "casper/src/runtime_manager.rs",
      "rholang/src/native_state.rs"],
    rustWitness := ["casper/tests/consensus.rs:the_bond_cache_is_the_active_pos_state_and_a_differing_one_is_refused"],
    witness := [`Rchain.a_bond_change_between_justifications_is_refused,
      `Rchain.a_bond_does_not_move_the_map_the_gates_read],
    falsifiable := some "the two sources of the bonds map disagree exactly when the bond set moves across \
      the justifications, and there the fallback **refuses** rather than picking one \
      (`a_bond_change_between_justifications_is_refused`: validator 7 at stake 1 in the older state, 2 \
      in the newer — what law 44's gate lets happen between boundaries — gives `none` from the carried \
      maps and `some [(7, 2)]` from the newest state's). So the equality is a claim about an **honest, \
      unmoved** set of justifications, not a tautology: drop either hypothesis and the statement is \
      false, which is why the port reads the state wherever it can. The **state's** side is falsifiable \
      the same way, and it was measured: make `a_bond_does_not_move_the_map_the_gates_read` read `pool` \
      instead of `active` and it stops being `rfl` — the build fails, which is what the theorem's \
      partner asserts",
    note := "**the state's side is closed: the model now carries the map it was missing, and the map \
      the gates read is derived from it** (2026-09-25, closing AUDIT C92). The row was `owed` because \
      the model could not *hold* the map — `active` was a `List Validator`, ids with no stakes, so the \
      only stake-carrying field was `pool`, and a derivation from `pool` answers with the *current* \
      stakes, which for a validator that bonded after the last boundary is a different map from the \
      port's. `Rchain/Pos.lean`'s `active` is now `List (Validator × Nat)`, the pairs `select_active` \
      returns, and `reselect` keeps them instead of dropping them (`.map (·.1)` was the loss). With \
      that, the state's side is statable, and it is stated as **a pair, because either half alone is a \
      spelling**: `a_bond_does_not_move_the_map_the_gates_read` — a bond writes `pool` and leaves the \
      map the gates read exactly as it was, which is law 44's gate seen from the finalizer's side — and \
      `the_pool_derived_map_moves_where_the_states_does_not`, where the same bond moves a pool-derived \
      reading by one entry and no hypothesis is needed, because `bond` prepends unconditionally. \
      Together they say the port's reading of `pos:active` rather than `pos:bonds` is load-bearing \
      rather than cosmetic. **The sync site is unchanged**: `the_sources_agree` and \
      `a_disagreeing_set_is_refused` still mirror the port's three sources. **Law 44's \
      `a_boundary_activates_the_pool` is strictly stronger for the same reason** — it assumed \
      `s'.claims = []` only because an id-only active set could equal step 4's output when the filter \
      removed nobody, and carrying the stakes makes the conclusion the filter itself, so that \
      hypothesis became *inert* rather than the claim becoming weaker. **What this row still does not \
      say**, stated rather than implied: nothing here proves the *port* reads `pos:active` — that is \
      the Rust's own shape (`runtime_manager.rs:1374-1380`) — so this is a `proved-model` tie, and the \
      model change carried laws 44-47 with it" },
  { number := 17, clause := "a", layer := "Casper",
    rustWitness := [
      "sdk/src/property_tests.rs:law17_deploys_without_conflicts_need_no_rejection",
      "sdk/src/property_tests.rs:law17_the_chosen_rejection_is_one_of_the_options",
      "sdk/src/property_tests.rs:law17_the_chosen_rejection_minimizes_the_total_cost",
      "sdk/src/property_tests.rs:law17_the_survivors_of_a_rejection_option_are_conflict_free"],
    statement := "Merge determinism: a rejection resolves to a unique minimum-cost candidate — the \
      rejection option is the minimum of `(total cost, size, the sorted set)`, and a minimum of a set \
      is unique, so the resolution is a function of the conflict set and not of the iteration order",
    status := .provedModel,
    declarations := [`Rchain.RejectionOption, `Rchain.totalCost, `Rchain.optionKey,
      `Rchain.optionKeyComparator, `Rchain.optionComparator, `Rchain.pickRejection,
      `Rchain.pickRejection_eq_none_iff, `Rchain.pickRejection_mem, `Rchain.pickRejection_minimal,
      `Rchain.equal_cost_and_size_do_not_make_equal_options, `Rchain.the_minimum_is_unique,
      `Rchain.the_resolution_does_not_depend_on_the_iteration_order, `Rchain.Comparator.le_of_not_lt],
    axioms := [],
    rust := ["sdk/src/dag/merging.rs", "casper/src/merging.rs"],
    witness := [`Rchain.equal_cost_and_size_do_not_make_equal_options, `Rchain.the_minimum_is_unique, `Rchain.the_resolution_does_not_depend_on_the_iteration_order],
    falsifiable := some "the linearity the word *unique* needs is itself checked: \
      `equal_cost_and_size_do_not_make_equal_options` exhibits two options that agree on the key's \
      first two components (`[1, 2]` and `[1, 3]`, both cost 2 and length 2 under a unit cost) that the \
      comparator still separates (`cmp = lt`) — drop the set from the key and they compare equal, \
      `min_by` returns whichever the `BTreeSet` yielded first, and \
      `the_resolution_does_not_depend_on_the_iteration_order` is false. The port's own test asserts the \
      same case (`compute_optimal_rejection_minimizes_cost_then_size`: `{1}` must win)",
    note := "**The note this row carried was wrong, and the code was right** (found 2026-09-23, \
      Programme D unit 10). It said the port \"does not choose among candidates, so a claim about a \
      unique minimum-cost candidate has nothing in the code to be stated against\". It does choose: \
      `resolve_conflict_set` (`sdk/src/dag/merging.rs:395`) closes the conflict map under \
      dependencies, computes the rejection options, extends them with what an overflow forces, and \
      `compute_optimal_rejection` (`:279-295`) picks one — `options.iter().min_by(|a, b| (cost(a), \
      a.len(), a).cmp(&(cost(b), b.len(), b)))`. The row's word *unique* is therefore load-bearing \
      rather than decorative, and it is exactly the property the model proves: the key is linear, so \
      no two distinct options compare equal, so the minimum is unique, so `min_by` over a `BTreeSet` \
      cannot be observed to depend on iteration order. The model is of the *selection*; the conflict \
      predicate and the branch sets it runs over are Law 9's (`are_conflicting`, \
      `rspace/src/merger/event_log_merging_logic.rs:100-158`), which is modelled there" },
  { number := 17, clause := "b", layer := "Casper",
    rustWitness := ["rholang/src/merging.rs:calculate_diff_rejects_i64_overflow_instead_of_wrapping"],
    statement := "The merge's arithmetic is the checked 64-bit one — a value that would leave `i64` is \
      **refused, not wrapped** — and the merged RNG is a function of the *set* of branch generators",
    status := .provedModel,
    declarations := [`Rchain.checkedAdd, `Rchain.checkedSub, `Rchain.mergeRandoms,
      `Rchain.checkedAdd_refuses_overflow, `Rchain.checkedSub_refuses_overflow,
      `Rchain.merge_diff_round_trip, `Rchain.mergeRandoms_perm],
    rust := ["rholang/src/merging.rs", "rspace/src/merger/event_log_index.rs"],
    axioms := [],
    witness := [`Rchain.checkedAdd_refuses_overflow, `Rchain.checkedSub_refuses_overflow, `Rchain.mergeRandoms_perm],
    falsifiable := some "the refusal is a witness rather than a remark: `checkedAdd i64Max 1 = none` \
      and `checkedAdd i64Min (-1) = none` (`checkedAdd_refuses_overflow`), which a `checkedAdd` that \
      wrapped would fail; `mergeRandoms_perm` is the statement that the merged RNG does not depend on \
      the order branches arrived in, false the moment the caller's sort is removed",
    note := "**the law that stood here was false**: `numeric_channels_nonneg` claimed numeric channels \
      are non-negative, and they are signed `i64` with ordinary negative diffs \
      (`rholang/src/merging.rs:161-166`, the `NumberChannel` struct; the tests are \
      `mergeable_data_round_trips` (`:414`, whose `diff: -5` literal is at `:426`) and \
      `calculate_diff_handles_negative_and_absent_keys` (`:389`)). The non-negativity that \
      *is* true belongs to Law 14's bonds and to `NonNegI64` (`shared/src/refined.rs:64`), which types \
      bonds and heights, never numeric channels. **And the arithmetic is only half checked**: the merge \
      result uses `checked_add` (`rholang/src/merging.rs:102`) while the diff accumulator *was* a plain \
      `i64 +=` (`EventLogIndex::combine`, `rspace/src/merger/event_log_index.rs:156`, and the merge's \
      own fold at `casper/src/merging.rs:891` — both `checked_add` now) — a debug panic, a \
      release wrap. That half was a code finding (AUDIT C41) and is **fixed**: the accumulation is
      checked now and its error reaches the merge, with `combining_refuses_a_diff_that_leaves_i64`
      (`rspace/src/merger/event_log_index.rs`) failing on a `wrapping_add`" },
  { number := 17, clause := "c", layer := "Casper",
    rustWitness := [
      "casper/src/merging.rs:a_boundary_round_does_not_lose_a_contained_deploys_charge",
      "casper/src/merging.rs:a_rejected_boundary_chain_does_not_take_its_blocks_other_chains",
      "casper/src/merging.rs:the_merge_report_names_every_dropped_native_write",
      "rspace/src/native_store.rs:a_drained_write_names_the_deploy_that_made_it",
      "rspace/src/native_store.rs:a_write_outside_any_window_is_not_attributed_to_a_block"],
    statement := "The native relation is on the **chain**, not on the block: two concurrent native \
      writers of one slot conflict, and a chain that wrote nothing contended is not a conflict \
      partner at all — so a user deploy's effects cannot die with a boundary its block also carried",
    status := .provedModel,
    declarations := [`Rchain.Chain, `Rchain.chainConflict, `Rchain.hostSlots,
      `Rchain.hostConflict, `Rchain.incidentChains, `Rchain.foldRefuses, `Rchain.resolves,
      `Rchain.host_keys_conflict_where_the_chains_do_not,
      `Rchain.the_resolution_leaves_the_fold_nothing_to_refuse,
      `Rchain.the_fold_refuses_concurrent_writers_of_one_slot],
    axioms := [],
    rust := ["casper/src/merging.rs", "rspace/src/native_store.rs", "rholang/src/merging.rs"],
    witness := [`Rchain.host_keys_conflict_where_the_chains_do_not,
      `Rchain.the_resolution_leaves_the_fold_nothing_to_refuse,
      `Rchain.the_fold_refuses_concurrent_writers_of_one_slot],
    falsifiable := some "Two halves, and the falsifier is the first. \
      `host_keys_conflict_where_the_chains_do_not` exhibits the incident's own shape — one block \
      carrying a boundary chain that wrote `{1,2}` and a user chain that wrote ∅, plus a concurrent \
      sibling writing `{1,2}` — and shows the **block-level** relation holding of the user chain while \
      the **chain-level** one does not: that is the rule that stood, refuted on the pair of chains it \
      rejected, and it fails the moment the relation is keyed on the host again. \
      `the_resolution_leaves_the_fold_nothing_to_refuse` is the positive half and is a *bridge*, not a \
      restatement: `resolves` is stated on `chainConflict` and `foldRefuses` on its own \
      shared-slot-and-unsequenced shape, so drop the overlap conjunct from `chainConflict` and two \
      concurrent writers of one slot are kept, the fold's own `Err` fires, and the theorem is false. \
      `the_fold_refuses_concurrent_writers_of_one_slot` stops the refusal being an empty predicate — \
      without it the positive half would be a statement about a shape nothing can have.",
    note := "**What this row does not model, and why that is deliberate.** The *selection* over the \
      rejection options is law 17a's and is unchanged; the **option enumeration** over a chain-level \
      conflict graph — the maximal conflict-free sets and the budget that bounds them — is still not \
      modelled here, because it would be a second model of `compute_rejection_options` \
      (`sdk/src/dag/merging.rs`) and its subject is not what the defect falsified. What is modelled is \
      the relation and the acceptance predicate, which is. **The defect it answers** is the live one: \
      `spec/audit/evidence/n280-merge-loses-a-write-results.md` records a four-validator net losing a \
      deployed write at an epoch boundary, unanimously, with the boundary blocks' `CloseBlock` ids in \
      the rejected set and no node refusing a peer's block. The Rust side is structural as well as \
      proved — `spec/TYPE-SYSTEM.md` — because the block-level *set* is not constructible any more: \
      `BlockIndex.native_changes` is gone, `DeployChainIndex::native_effects` carries each chain's own \
      writes, and `reject_whole_blocks` is deleted rather than narrowed, so 'reject every chain of a \
      block because of a write the block made' has no operation left to express it. **Its `slots` are \
      law 17e's effective writes**: a chain's slot set is the writes that changed a value, so this \
      row's relation and 17e's rule are two halves of one fact and neither restates the other." },
  { number := 17, clause := "d", layer := "Casper",
    rustWitness := [
      "rholang/src/merging.rs:a_legacy_sidecar_decodes_as_unattributed_and_not_as_empty",
      "rholang/src/merging.rs:an_attributed_sidecar_round_trips_and_a_damaged_one_is_not_read_as_empty",
      "rspace/src/native_store.rs:a_genesis_write_is_named_and_does_not_travel_in_a_sidecar"],
    statement := "A block's native effects are **attributed to the deploy that made them, by \
      construction**: an unattributed, block-level native-effect set is not a value — the sidecar's \
      shape has no arm for it, and a write outside any deploy's window is refused rather than filed \
      under the block",
    status := .vacuous,
    declarations := [`Rchain.Chain],
    axioms := [],
    rust := ["rspace/src/native_store.rs", "rholang/src/merging.rs"],
    falsifiable := some "the *negative* half is the machine half, as in law 22: the two decode tests \
      pin that a record written before attribution existed reads as `LegacyUnattributed` with its \
      actions intact and **not** as `Attributed(empty)` — collapsing either way is how a block's \
      effects come to be filed under the block or to vanish — and that a truncated or \
      trailing-byte record is an `Err` rather than an empty one. What would reopen the row is a second \
      constructor taking a bare action list, which is the shape the type exists to remove.",
    note := "**`vacuous` in the register's own sense, and the reason is law 22's.** The positive half — \
      \"an unattributed native-effect set is not a value\" — is a fact about a **type**, not a \
      theorem: `BlockNativeEffects` has a private field, no `Deref` and no getter, and its keys are \
      deploy ordinals, so there is no key that could mean \"the block\". Stating it in Lean would \
      prove that a type ignores a constructor nobody writes, which is the deleted \
      `next_step_closure_computable` shape this register already refused once. The artifact is \
      `rspace/src/native_store.rs`'s `BlockNativeEffects` + `NativeWriter` (whose `OutsideAnyDeploy` \
      arm is refused by `from_drain` rather than attributed to the block, and whose `Genesis` arm is \
      *named* rather than unattributed, and does not travel), the codec's three-way `SidecarRecord` \
      in `rholang/src/merging.rs`, and the gate that keeps the discipline: \
      `tools/audit-type-system.sh`'s construction scan. **Not a refinement in that gate's roster**, \
      deliberately: the gate's `escape` class is for types carrying a *predicate* (a validator the \
      constructor must run), and this type carries none — its invariant is its key type." },
  { number := 17, clause := "e", layer := "Casper",
    rustWitness := [
      "casper/src/merging.rs:a_finalized_boundary_noop_does_not_reject_a_concurrent_ballot",
      "casper/src/merging.rs:a_concurrent_boundary_noop_does_not_overwrite_a_ballot",
      "casper/src/merging.rs:a_boundary_that_changes_the_ballot_map_still_conflicts",
      "casper/src/merging.rs:a_noop_native_write_is_not_a_chain_slot"],
    statement := "**A native write that leaves the slot as it found it is not a write.** Law 17c's \
      relation is on the slots a chain *wrote*, and `NativeStore::put` records a `Put` even when the \
      bytes equal the pre-state's. The armed liveness prelude rewrites `pos:slash-votes`, \
      `pos:vote-windows` and `pos:liveness` at every advancing boundary, so counting an unchanged \
      rewrite as a slot made a finalised boundary chain a conflict partner of a concurrent user \
      chain — and a conflict-scope chain that conflicts with a finally-accepted one is forced out. \
      A no-op write therefore contributes no slot: the chain index is built from the **effective** \
      writes, those whose value changed, walked in deploy order",
    status := .provedModel,
    formal := `Rchain.law17e_statement,
    falsifier := `Rchain.law17e,
    declarations := [`Rchain.NativeWrite, `Rchain.NativeWrite.isNoop, `Rchain.effectiveSlots,
      `Rchain.allSlots, `Rchain.law17e_statement, `Rchain.law17e,
      `Rchain.all_writes_conflict_where_the_effective_ones_do_not],
    axioms := [],
    rust := ["casper/src/merging.rs", "casper/src/runtime_manager.rs",
      "rspace/src/native_store.rs"],
    witness := [`Rchain.law17e, `Rchain.all_writes_conflict_where_the_effective_ones_do_not],
    falsifiable := "`all_writes_conflict_where_the_effective_ones_do_not` exhibits the same pair of \
      chains under both rules: counted by its slot, the no-op writer conflicts with a chain that \
      really writes the leaf; counted by its effective set, it does not. Drop the value comparison \
      from `effectiveSlots` and the port is back to rejecting a user chain for bytes it did not \
      change. The Rust side is the red-then-green fixture \
      `a_finalized_boundary_noop_does_not_reject_a_concurrent_ballot`: before the fix the vote \
      deploy's id appeared in `rejected_deploys` and its slot in `dropped_native_slots`.",
    note := "**Why this is not a restatement of 17c.** 17c says *which chains* are conflict partners \
      (per chain, not per block). This row says *which of a chain's writes count* (the ones that \
      changed a value). Both are needed: the C207 fix made the relation chain-level and a user \
      chain that wrote nothing contended stopped dying; the boundary still counted an unchanged \
      rewrite as something it wrote, so a user chain that *did* write the leaf could still be \
      vetoed by a finalised boundary that wrote it back byte-for-byte. **Where the port applies \
      it:** `effective_native_effects` at index construction (`BlockIndex::apply`, and the batched \
      play path), not in the store — `apply` trusts a stored sidecar, so a store-level filter would \
      make an old sidecar and a regenerated one disagree about the same block's slots after a \
      rolling upgrade, and the filter here is idempotent across both. **Hard-fork class:** two \
      binaries can reject different chains at a merge; no deployment, and mainnet arming remains \
      #428. **What it does not model:** the value is an `Option Nat` and the write list is flat, so \
      the model states the comparison, not the port's byte codecs; the deploy-order walk is \
      modelled by the list order." },
  { number := 18, layer := "Storage",
    rustWitness := [
      "block-storage/src/dag/metadata_store.rs:law18_contiguous_height_map_is_valid",
      "block-storage/src/dag/metadata_store.rs:law18_height_map_with_holes_errors",
      "block-storage/src/property_tests.rs:law18_a_contiguous_chain_validates",
      "block-storage/src/property_tests.rs:law18_a_chain_with_a_hole_in_the_middle_is_refused",
      "block-storage/src/property_tests.rs:law18_a_missing_lowest_height_is_not_a_hole",
      "block-storage/src/property_tests.rs:law18_the_empty_dag_and_a_lone_block_are_contiguous",
      "block-storage/src/property_tests.rs:law18_the_state_does_not_depend_on_insertion_order",
      "models/src/block_metadata.rs:law18_fringe_hash_is_order_independent"],
    statement := "The store's own invariants: the height map is **contiguous** — no holes in block \
      heights — and the fringe identity is **order-independent**, because what the code keys on is a \
      `BTreeSet`, not a list",
    status := .provedModel,
    declarations := [`Rchain.HeightSet, `Rchain.Contiguous, `Rchain.contiguous_insert_succ,
      `Rchain.contiguous_skip_leaves_hole, `Rchain.height_map_universal_is_false, `Rchain.Fringe,
      `Rchain.fringeId, `Rchain.fringeId_perm,
      `Rchain.fringe_identity_order_independent_is_false],
    axioms := [],
    rust := ["block-storage/src/dag/metadata_store.rs", "models/src/block_metadata.rs",
      "block-storage/src/dag/finalizer.rs"],
    witness := [`Rchain.height_map_universal_is_false, `Rchain.fringe_identity_order_independent_is_false, `Rchain.contiguous_skip_leaves_hole, `Rchain.fringeId_perm],
    falsifiable := some "`contiguous_skip_leaves_hole` is the negative case the store's check exists for: \
      a block whose number is not the successor of the current maximum leaves a hole (what \
      `validate_dag_state` reports, `metadata_store.rs:83-86`) — and the store **cannot derive** the \
      positive direction for itself, since it inspects only the keys it is handed: that step runs through \
      Law 16a's block-number check. `fringeId_perm` fails for an identity that dropped the sort, which \
      is what `fringe_hash_of` would be without its `BTreeSet` input (`models/src/block_metadata.rs:31-34`), \
      and `height_map_universal_is_false` / `fringe_identity_order_independent_is_false` publish the \
      refutations of the two axioms this row replaces",
    note := "**two axioms gone, and neither was a law.** `height_map_contiguous` claimed a property of \
      *any* `List Block` — false, and refuted in the tree (a one-element list numbered 1 has an element \
      above 0 and no element at 0); what the port has is an invariant its *store* checks \
      (`metadata_store.rs:77-87`), so the model states contiguity as the store's meaning and proves the \
      two steps that matter: a successor insert preserves it, a skipped number breaks it. \
      `fringe_identity_order_independent` claimed `Perm → f = g`, also false of a hand-built `Fringe` \
      (refuted in the tree) — in the port the property is **structural**: `fringe` is a \
      `BTreeSet<BlockHash>` wherever it appears, so there is no list order to be invariant under, and the \
      model keeps a list only so the claim can be stated at all, then sorts (`fringeId`) and proves the \
      invariance the type supplies. **Clause split removed**: 18a and 18b now share a status, an empty \
      axiom set and a file" },
  { number := 19, layer := "Crypto",
    rustWitness := [
      "crypto/src/hash/blake2b512_random.rs:empty_gives_a_predictable_result",
      "crypto/src/hash/blake2b512_random.rs:merge_with_two_children",
      "crypto/src/hash/blake2b512_random.rs:merge_with_many_children",
      "crypto/src/hash/blake2b512_random.rs:merge_is_order_sensitive",
      "crypto/src/signatures/secp256k1.rs:creates_known_ecdsa_signature",
      "crypto/src/signatures/secp256k1.rs:verifies_known_signature",
      "crypto/src/encryption/curve25519.rs:decrypts",
      "crypto/src/encryption/x25519.rs:rfc7748_section_5_2_first_vector",
      "crypto/src/encryption/x25519.rs:rfc7748_section_5_2_second_vector",
      "crypto/src/encryption/x25519.rs:rfc7748_section_6_1_diffie_hellman_agrees_both_ways"],
    statement := "Blake2b256 is canonical and collision-free; the `Blake2b512Random` merge is n-ary \
      and **order-sensitive**; signatures verify what they sign; Curve25519 round-trips",
    status := .axiomByDesign,
    declarations := [`Rchain.blake2b256, `Rchain.blake2b256_output_is_32_bytes,
      `Rchain.blake2b256_collision_free, `Rchain.sign, `Rchain.verify,
      `Rchain.sign_verify_roundtrip, `Rchain.sharedSecret, `Rchain.curve25519_roundtrip,
      `Rchain.mergeRandom],
    rust := ["crypto/src/hash/blake2b256_hash.rs", "crypto/src/hash/blake2b512_random.rs"],
    axioms := [`Rchain.blake2b256, `Rchain.blake2b256_output_is_32_bytes,
      `Rchain.blake2b256_collision_free, `Rchain.sign, `Rchain.verify,
      `Rchain.sign_verify_roundtrip, `Rchain.sharedSecret, `Rchain.curve25519_roundtrip,
      `Rchain.mergeRandom],
    falsifiable := some "the Rust side is pinned by known-answer tests rather than by these axioms: \
      `crypto/`'s `Blake2b512Random`/`Secp256k1`/`Curve25519` vectors are what would catch a wrong \
      primitive, so the axioms are a *boundary* of the model, not a claim the model establishes",
    note := "the only `axiomByDesign` law, and where a **false axiom** was found: `mergeRandom_comm` \
      claimed the RNG merge commutes and the code's own test refutes it \
      (`crypto/src/hash/blake2b512_random.rs:548`, `merge_is_order_sensitive`). The merge is modelled \
      n-ary (`List Random → Random`) because that is its Rust signature, and *no* algebraic law of it \
      is claimed — the code has none to claim, so `mergeRandom_assoc` went with the commutativity. Law \
      17's RNG clause duplicated this one and is merged into it. **The types were retyped to mirror the \
      code's**: `Msg`/`Hash` are byte strings rather than opaque `Nat` wrappers, and the 32-byte width \
      is `blake2b256_output_is_32_bytes` (`Hash32`, `shared/src/refined.rs:302-311`) — which is what \
      lets Law 7's and Law 10's models be built out of hashes instead of guessed numbers. **And the \
      nine are not all load-bearing — measured 2026-09-25** with `Lean.collectAxioms` over every \
      declaration the register names: `blake2b256` is used by 7 of them (the block hash, the Merkle \
      node hash, content addressing), `blake2b256_collision_free` by 4, `mergeRandom` by 2, and Law 7's \
      `hashHashes` by 1 — while `blake2b256_output_is_32_bytes`, `sign`, `verify`, \
      `sign_verify_roundtrip`, `sharedSecret` and `curve25519_roundtrip` are used by **none**. Those \
      six are the crypto API stated as the model's boundary, so that laws about it are statable, not \
      assumptions any proof leaned on; the register now says which is which instead of leaving nine \
      axioms looking equally exercised. **The tie is a witness the gate runs** (`rustWitness`): the \
      known-answer vectors — the RNG's fixed empty-input stream and its two merge cases, a known ECDSA \
      signature and its verification, the `Curve25519` **sealed-box** round-trip \
      (`crypto/src/encryption/curve25519.rs:decrypts`, a `crypto_box` vector), and — since the OCapN \
      Noise transport needed the raw primitive rather than a box — RFC 7748's own `X25519` vectors, \
      §5.2's two and §6.1's Diffie-Hellman example (`crypto/src/encryption/x25519.rs`) — because they \
      are what catches a wrong \
      primitive. No test \
      can witness the idealization itself: `blake2b256_collision_free` states collision-*resistance* as \
      injectivity, which the pigeonhole refutes as a fact about the real function \
      (`spec/Rchain/Crypto/Spec.lean:41-49`)" },

  -- ── Scheduler: the effect scheduler (Laws 20–25) ─────────────────────────────────────────────────
  { number := 20, layer := "Scheduler",
    rustWitness := ["rspace/src/property_tests.rs:law20_per_channel_path_order"],
    statement := "Channel-task linearization (\"1 channel = 1 logical task\"): same-channel ops commit \
      in DFS path order through a per-channel claim queue, and the path-smallest pending claim is \
      always committable",
    status := .provedModel,
    declarations := [`Rchain.queue_commit_path_ordered, `Rchain.pathSorted_head_minimal],
    rust := ["rspace/src/concurrent/channel_queue.rs"],
    witness := [`Rchain.queue_commit_path_ordered, `Rchain.pathSorted_head_minimal],
    falsifiable := some "`queue_commit_path_ordered` is an induction on the run — a commit rule that \
      removed a claim which was not head-on-all would falsify it — and `pathSorted_head_minimal` is \
      the bakery argument's core: an unordered queue, or a head with a smaller path behind it, would \
      falsify that",
    note := "the axiom that stood here (`law20_deadlock_freedom`) is **deleted**, and the reason is a \
      finding: it quantified over every `Chan → Queue` and claimed some claim is head-on-all, which is \
      unprovable as stated — `PathLt` is not well-founded on paths (`[0] > [0,0] > [0,0,0] > …`), so an \
      unbounded pending set need have no minimum. The real system's paths are bounded by the depth of \
      the term being reduced and its pending set is finite, so the corrected statement carries that \
      finiteness as a hypothesis; what is proved here is the queue-level core, which needs none. The \
      Rust's liveness evidence is stress tests plus the queue's own deferral of cross-channel cycles \
      (`rspace/src/concurrent/channel_queue.rs:24-28`, risk R3) — a tested property, not a theorem. \
      **Also found while proving it**: `queue_commit_path_ordered` carried an unused \
      `hinit : ∀ ch, PathSorted (q0 ch)` — the same unused-hypothesis shape as Law 24's — and it is \
      gone, because the suffix property comes from the commit rule rather than from the initial sorting" },
  { number := 21, layer := "Scheduler",
    rustWitness := ["rholang/src/property_tests.rs:law21_the_gate_scheduler_refines_the_sequential_reference"],
    statement := "DFS-gate linearization: the gate scheduler refines sequential `Effect.apply`; one-hop \
      next-step pruning is unsound",
    status := .provedModel,
    declarations := [`Rchain.gate_await_closure_orders, `Rchain.one_hop_depth2_diverges],
    rust := ["rholang/src/reduce.rs", "rholang/src/scheduler.rs"],
    witness := [`Rchain.one_hop_depth2_diverges, `Rchain.gate_await_closure_orders],
    falsifiable := some "`one_hop_depth2_diverges` is a proved depth-2 counterexample to the pruning \
      rule the law forbids — the law and the disproof of its tempting weakening are published together \
      — and `gate_await_closure_orders` is false for a chain with a missing link (a dependency of \
      `j + 2 = i` would leave odd-indexed tasks unordered)",
    note := "`gate_exec_refines_apply` is **gone**: it defined `gateApply` as the sequential fold and \
      then proved the fold is the fold. The Rust's own comment above the gate says the same — 'Not a \
      speedup — the sound, sequential-equivalent carrier' (`reduce.rs:2568-2569`, in the gate's own comment) — so the content was \
      never in the identification. It is in the **dependency structure**, and that is what \
      `gate_await_closure_orders` proves: the immediate-predecessor await chain is transitively \
      complete, which is exactly the Rust's 'a linear chain of awaits, not the quadratic \
      all-predecessors join' (`reduce.rs:2570-2575`)" },
  { number := 22, layer := "Scheduler",
    rustWitness := ["rholang/src/reduce.rs:law22_the_next_step_closure_is_computable_at_dispatch"],
    statement := "Next-step closure is computable at dispatch (the matched datum is concrete); \
      computability does not make cross-channel pruning sound",
    status := .vacuous,
    declarations := [`Rchain.depth2_next_step_disjoint],
    rust := ["rholang/src/reduce.rs"],
    falsifiable := some "`depth2_next_step_disjoint` is the half with content: the closure is \
      computable yet cross-channel pruning by it is still unsound, and that is proved rather than \
      asserted",
    note := "the positive half is a fact about a **signature**, not a theorem, which is why this row is \
      `vacuous` rather than a proof claim: `resolve_children` takes no store \
      (`reduce.rs:resolve_match`, whose doc comment names the purity), so stating it in Lean \
      would prove that \
      a function ignores a parameter nobody passes. The theorem that stood here \
      (`next_step_closure_computable`) was `by rfl` and is **deleted** — and **that is the reason not to \
      close this row**: a future proof of the positive half would be `rfl`-shaped again, re-introducing \
      exactly the vacuity the deletion removed. **And the tempting positive law \
      is false**: resolving `p | q` term-wise does *not* give `resolve p ++ resolve q`, because the Rust \
      flattens all sends before all receives — one send and one receive in each half resolves as \
      `[sp,sq,rp,rq]` merged but `[sp,rp,sq,rq]` concatenated. Found by trying to prove it, which is \
      the argument for proving things" },
  { number := 23, layer := "Scheduler",
    rustWitness := ["rspace/src/property_tests.rs:law23_read_state_determines_outcome"],
    statement := "Read-determinism: an effect's chosen candidate, commit outcome and event trace are a \
      deterministic function of the state it reads",
    status := .provedModel,
    declarations := [`Rchain.read_state_determines_outcome],
    rust := ["rspace/src/space_matcher.rs", "rspace/src/rspace.rs"],
    witness := [`Rchain.certificate_blind_late_writer_diverges],
    falsifiable := some "stated as an implication from two states agreeing on the effect's closure, so \
      it is falsifiable by a state pair that agrees on the closure yet yields different traces — the \
      depth-2 stale read of Law 24 is exactly the shape that would produce one; \
      `certificate_blind_late_writer_diverges` is the instance",
    note := "the property test `law23_read_state_determines_outcome` (`rspace/src/property_tests.rs`) \
      names it on the Rust side (commit 35dd13b62)" },
  { number := 24, layer := "Scheduler",
    rustWitness := ["rspace/src/property_tests.rs:law24_record_layer_and_validation",
      "rholang/src/reduce.rs:law24_the_effect_mode_is_what_enables_the_certificate"],
    statement := "DFS-order serializability: a concurrent execution is sound for the block path iff \
      every commit read exactly the state the DFS-earlier effects produced (the versioned \
      write-record layer)",
    status := .provedModel,
    declarations := [`Rchain.prefixVisible, `Rchain.ValidCommit, `Rchain.DFSSerializable,
      `Rchain.s3_pair_fails_validation, `Rchain.serializable_writer_chain,
      `Rchain.pinned_run_publication, `Rchain.certificate_blind_late_writer_diverges],
    rust := ["rspace/src/concurrent/channel_queue.rs", "casper/src/runtime_manager.rs"],
    witness := [`Rchain.certificate_blind_late_writer_diverges, `Rchain.s3_pair_fails_validation, `Rchain.serializable_writer_chain],
    falsifiable := some "the certificate's blind spot is *published* as a theorem \
      (`certificate_blind_late_writer_diverges`): a late-writer run passes the certificate and still \
      diverges, which is why the oracle backstop stays load-bearing",
    note := "the unused `DFSSerializable` hypothesis is **gone**, and with it the \
      `set_option linter.unusedVariables false` that suppressed the warning — the theorem \
      (`pinned_run_publication`, renamed to state what it proves) is dispatched + path-nodup + pinned \
      ⇒ the gate fold, with **no** certificate hypothesis, because a pinned run *is* the \
      serializability that matters. The certificate *finds* such runs; it is not why the fold is reached" },
  { number := 25, layer := "Scheduler",
    rustWitness := ["rholang/src/property_tests.rs:law25_the_validated_relaxed_scheduler_refines_sequential"],
    statement := "Validated speculation: commits may reorder iff each validates Law 24; invalidated \
      runs fall back to the whole-run gate re-run, so the published state is the sequential fold's",
    status := .provedModel,
    declarations := [`Rchain.published, `Rchain.published_state_is_the_oracles,
      `Rchain.fallback_rerun_published, `Rchain.Published],
    rust := ["casper/src/runtime_manager.rs", "rspace/src/concurrent/channel_queue.rs"],
    witness := [`Rchain.published_state_is_the_oracles, `Rchain.fallback_rerun_published],
    falsifiable := some "the rule is the Rust's own (`validate_relaxed_block`, \
      `casper/src/runtime_manager.rs:1095`; its acceptance is `comm_multisets_match` over both logs \
      with `oracle_hash == relaxed_hash`, `:1113-1132`): accept \
      the speculative run only when it agrees with the oracle, and otherwise ship the oracle's result. \
      A publication rule that shipped the speculative state unconditionally would falsify \
      `published_state_is_the_oracles`, which is why the theorem is stated over the *rule* rather than \
      over the fold. `fallback_rerun_published` separately pins that the path-sorted re-run commits \
      each path at most once",
    note := "`validated_speculation_refines_apply` is **gone**, and so is `gate_replay_terminates`: the \
      first was a disjunction whose second arm held for *every* run by the definition of `gateFold` (so \
      it said nothing about the certificate, the fallback, or the code), and the second was \
      `⟨gateRerun ops st, rfl⟩` — totality of a Lean function, which is why `∃ st', f st = st'` is \
      never a law. What replaces them is the port's actual publication rule, which is what makes the \
      sequential-oracle backstop load-bearing rather than decorative" },

  -- ── Cross-shard: two-phase commit (Laws 26–29) ───────────────────────────────────────────────────
  { number := 26, clause := "a", layer := "Cross-shard",
    rustWitness := [
      "node/src/web/http.rs:api_txn_run_rejects_an_invalid_leg_shard_or_an_empty_leg_list",
      "node/src/web/http.rs:a_leg_with_an_empty_to_is_rejected_before_the_gateway_runs",
      "casper/src/property_tests.rs:law26_a_shard_id_is_accepted_exactly_when_nonempty_ascii",
      "casper/src/property_tests.rs:law26_invalid_shard_names_are_refused",
      "casper/src/property_tests.rs:law26_a_child_id_nests_under_its_parent"],
    statement := "A deploy/block's effects bind to exactly one shard, and the leg a gateway admits \
      carries a validated shard id",
    status := .provedModel,
    declarations := [`Rchain.shard_scope_deterministic_is_false, `Rchain.ValidShardId, `Rchain.Leg,
      `Rchain.isAscii, `Rchain.isBlank, `Rchain.validShardId, `Rchain.IncomingLeg, `Rchain.AdmittedLeg,
      `Rchain.admitLeg, `Rchain.validShardId_empty, `Rchain.validShardId_implies_ne,
      `Rchain.the_admitted_leg_carries_a_validated_shard_id, `Rchain.admitLeg_rejects_an_empty_shard_id,
      `Rchain.admitLeg_rejects_a_non_ascii_shard_id, `Rchain.admitLeg_rejects_a_blank_recipient,
      `Rchain.admitLeg_admits_a_negative_amount],
    rust := ["node/src/web/http.rs", "shared/src/refined.rs"],
    witness := [`Rchain.admitLeg_rejects_a_non_ascii_shard_id,
      `Rchain.admitLeg_rejects_a_blank_recipient, `Rchain.shard_scope_deterministic_is_false],
    falsifiable := "`admitLeg_rejects_an_empty_shard_id` / `..._a_non_ascii_shard_id` / \
      `..._a_blank_recipient` are the three refusals the boundary performs, each `decide`d, so dropping \
      a check from `admitLeg` fails them (measured: dropping the ASCII and blank checks makes the build \
      fail on both witnesses *and* on `the_admitted_leg_carries_a_validated_shard_id`, whose three \
      conjuncts are the two `ShardId::try_from` checks plus the recipient one)",
    note := "**the axiom this row used to cite was FALSE** — `∀ l : Leg, ValidShardId l.shard` over a \
      freely constructible record, refuted by `Leg.mk \"\" 0 0` (`shard_scope_deterministic_is_false`, \
      2026-09-23). The axiom is deleted rather than kept beside its own refutation (a false axiom makes \
      everything provable). What the law is about is the *ingress*: the port rejects an invalid shard id \
      at the boundary (`ShardId::try_from` on `TxnLegDto.shard_id`, `node/src/web/http.rs:305-331`, with \
      the boundary test that pins the 400), so the narrowed statement is about the function that admits \
      a leg — which needed that function modelled. **That function is modelled now (2026-09-24, \
      Programme F) and the row is proved**: `admitLeg` mirrors the boundary's decision exactly — \
      `validShardId` (non-empty, ASCII) and a non-blank recipient — and \
      `the_admitted_leg_carries_a_validated_shard_id` states the law's content, with the three refusals \
      as `decide`d witnesses. Two things are named rather than implied. (1) **The amount is not checked \
      at the ingress** — the port's own comment records it is left to the ledger's `NonNegI64` \
      refinement in `GatewayTxn::run`, and `admitLeg_admits_a_negative_amount` is a *theorem* so a model \
      that added the check would fail it. (2) **The recipient check is modelled over code points**: \
      `String.trim` does not reduce in Lean, so `isBlank` (all characters `Char.isWhitespace`) is the \
      computable counterpart of `l.to.trim().is_empty()`, which is what keeps the witnesses `decide`d" },
  { number := 26, clause := "b", layer := "Cross-shard",
    statement := "The shard id is a validated, ordered value — the order is the **derived** one (the \
      port's `#[derive(…, Ord …)]` over the inner `String`), and naming a child under a valid shard \
      yields a valid one whose path **extends** its parent's",
    status := .provedModel,
    declarations := [`Rchain.shardChild, `Rchain.validShardId_child, `Rchain.shardChild_prefix,
      `Rchain.a_child_sorts_after_its_parent, `Rchain.a_grandchild_sorts_after_its_grandparent,
      `Rchain.the_child_of_an_invalid_parent_can_be_valid],
    axioms := [],
    rust := ["shared/src/refined.rs", "casper/src/gateway/mod.rs", "node/src/runtime/node_runtime.rs"],
    witness := [`Rchain.validShardId_child, `Rchain.shardChild_prefix,
      `Rchain.a_child_sorts_after_its_parent, `Rchain.the_child_of_an_invalid_parent_can_be_valid],
    falsifiable := some "**a mutation for each half.** The order half: `shardChild_prefix` fails for a `child` that \
      returned its parent, and the two `decide`d instances fail for one that prepended instead of \
      appending — while the *general* monotonicity is the named boundary (a library gap, not a port \
      claim), and the guard it would need (`n ≠ \"\"` at the root, where `child` appends only the name) \
      is a property of the order rather than of the prefix fact. The validity half: `validShardId_child` fails if the characterisation \
      drops its `isAscii n` term, and `the_child_of_an_invalid_parent_can_be_valid` is the `decide`d \
      witness that the row's original identity is **false** — `shardChild \"\" \"x\"` is `/x`, valid, \
      while `\"\"` is not",
    note := "**the order is derived, so the model needs no instance of its own** (2026-09-24): \
      `shared/src/refined.rs:387` is \
      `#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)] pub struct ShardId(String)` — the \
      lexicographic order on the id string, which `child`'s prefixing makes *structurally meaningful* (a \
      shard sorts before all of its descendants, and `is_descendant_of` is the separate structural \
      predicate that agrees with it). Where the order is observable is the two `BTreeMap<ShardId, _>` \
      sites — `shards` at `casper/src/gateway/mod.rs:65` and at \
      `node/src/runtime/node_runtime.rs:958` — deterministic \
      iteration, exactly where a silently different order would bite — and the model's `ShardId` *is* a \
      `String`, so `decide` and `omega` see the same order the port derives. **And the witness this row \
      carried needed correcting, which is the part worth keeping**: `validShardId (s.child n) = \
      validShardId s` is **false in both directions** — the *model's* `shardChild` joins the path with \
      no check, so a non-ASCII name yields an id `TryFrom` would have refused; and an invalid parent can \
      have a valid child (`shardChild \"\" \"x\" = \"/x\"`), which holds of the port too. **The port's \
      `child` did the same when this row was written** — it constructed the newtype **directly** \
      (`refined.rs:398-425`), bypassing `TryFrom`, which is AUDIT C78 — **and no longer does**: it \
      refuses an empty or non-ASCII name and returns `Result` (`shared/src/refined.rs:411-425`, whose \
      own doc names C78). So the port-side reading of that identity is false for a different reason now, \
      the constructor *refusing* where the model joins. What holds instead is the characterisation in \
      `validShardId_child`: given a **valid** parent, the child is valid exactly when the name is ASCII. \
      AUDIT C78 was a finding about the port's own refinement discipline rather than a deviation from \
      the oracle, and it is **closed**; this row states the corrected form rather than the convenient \
      one, with the refuted version kept beside it so the next reader does not re-derive it. **And the \
      order half's boundary is named with its cost**: \
      what the `BTreeMap` sites observe is `shardChild_prefix` (the child's path carries the parent's) \
      plus the two `decide`d instances; the general \"a child sorts after its parent\" is *statable*, \
      follows from this plus the lexicographic order's prefix property, and is **not a theorem here** \
      because this tree compares no `String`s at all (`Cmp.lean` orders by `GString` with no \
      string-order lemma) — proving it would be the tree's **first** string-order lemma, a modelling \
      decision rather than a proof step. That is a library gap and not a claim about the port, which is \
      why the row is `provedModel` on what is proved rather than `owed` on what the toolchain lacks" },
  { number := 26, clause := "c", layer := "Cross-shard",
    statement := "The RNG seed and unforgeable names are shard-scoped",
    status := .retired,
    rust := ["rholang/src/reduce.rs", "casper/src/tools.rs"],
    falsifiable := some "the law could have been true in exactly one way, and the reading below shows \
      the port does not take it: put a shard in the seed derivation — the unforgeable draw \
      (`rholang/src/reduce.rs`'s `alloc`), `unforgeable_name_rng` and the deploy's \
      `Blake2b512Random::from_init` (`casper/src/tools.rs`) — and the same deploy bytes in a second \
      shard would draw *different* names. Nothing in either tree does, so the claim has no instance to \
      be false of, which is why the row is retired rather than open",
    note := "**retired 2026-09-25, on the reading below — the port has no rule of this shape** \
      (`retired`'s words), and the evidence is the measurement that was already here: \
      **determined by reading the port, not by judgement** (2026-09-24), and the answer is that \
      the node scopes **neither** to a shard — so this is a **design claim** (law 48's shape), not a \
      port gap. The unforgeable names *are* the deploy's RNG stream: the `GPrivate` constructor over a draw: \
      `GUnforgeable::GPrivate(GPrivate { id: bytes })` with `bytes = rand.next()` (`rholang/src/reduce.rs:2198`, \
      in `alloc`); and \
      every seed site is shard-free — `unforgeable_name_rng` over the deployer and the timestamp \
      (`casper/src/tools.rs:11`), `Blake2b512Random::from_init(&deploy.to_bytes())` (`:26`), \
      `rng` over the signature (`:31`) — with no `shard` in any rand or seed site anywhere in `casper/src/`. \
      **The oracle matches**, which settles the disposition: `ProtoUtil.scala:36,49` carries `shardId` \
      only as the *deploy record's own field*, and nothing under `legacy/casper/…/runtime/` mentions a \
      shard at all — so the shard id is a deploy-level field, checked at the ingress (26a's `admitLeg`) \
      and binding the effect to one shard, not an input to the derivation. **Its uniqueness across \
      shards is a consequence of a deploy executing once, not of the derivation: the same deploy bytes \
      in a second shard would draw the same names.** Closing the row would need the shard in the seed \
      derivation *plus* a two-shard non-collision theorem — a design decision shared with the Scala" },
  { number := 27, layer := "Cross-shard",
    rustWitness := [
      "casper/src/property_tests.rs:law27_a_legless_record_cannot_commit",
      "casper/src/property_tests.rs:law27_an_abort_vote_prevents_a_later_commit",
      "casper/src/property_tests.rs:law27_and_law29_the_state_agrees_with_the_votes",
      "casper/tests/cross_shard_txn.rs:two_shard_2pc_commits_all",
      "casper/tests/cross_shard_txn.rs:two_shard_2pc_aborts_all_when_a_leg_fails"],
    statement := "Cross-shard atomicity (2PC): every leg that *prepared* reaches the one decision the \
      coordinator made — commit on all of them or abort on all of them",
    status := .provedModel,
    declarations := [`Rchain.txn_atomic_is_false, `Rchain.uniform, `Rchain.ShardOutcome,
      `Rchain.voteFromReply, `Rchain.decisionOf, `Rchain.phaseTwoWith, `Rchain.phaseTwoWith_cons,
      `Rchain.runPhaseTwo, `Rchain.phaseTwoWith_all_prepared, `Rchain.every_leg_reaches_the_one_decision,
      `Rchain.all_prepared_legs_commit, `Rchain.an_already_committed_leg_still_commits,
      `Rchain.an_aborted_reply_moves_the_decision],
    rust := ["casper/src/txn_coordinator.rs"],
    witness := [`Rchain.txn_atomic_is_false, `Rchain.an_already_committed_leg_still_commits, `Rchain.an_aborted_reply_moves_the_decision, `Rchain.every_leg_reaches_the_one_decision, `Rchain.all_prepared_legs_commit],
    falsifiable := some "`every_leg_reaches_the_one_decision` says every leg's phase-two outcome is the \
      *single* decision or `\"not prepared\"` — so a leg that voted abort cannot carry an outcome that \
      disagrees with a prepared leg's, and `all_prepared_legs_commit` is the all-ready case. The retry \
      path is pinned by `an_already_committed_leg_still_commits` (a `committed` reply plus a `ready` one \
      still commit) against `an_aborted_reply_moves_the_decision` (an `aborted` reply, or an error, is an \
      abort vote) — the two together are what `vote_from_reply`'s own comment argues for, and reading \
      `committed` as not-ready would flip the first into an abort and leave one shard committed and \
      another aborted",
    note := "**the axiom this row used to cite was FALSE** — `∀ r : Run, uniform r` over `Run := List \
      Outcome`, refuted by `[committed, aborted]` (`txn_atomic_is_false`, 2026-09-23). The narrowed \
      statement is the port's own: `run_2pc` decides once (`all_ready`), and phase two applies that \
      decision to every leg that voted ready — a leg that did not prepare never locked, so it has no \
      outcome to be uniform about (`casper/src/txn_coordinator.rs:152-192`). The Rust's comment on \
      `vote_from_reply` (`:209-217`) is the differential reference for the retry path: a re-run \
      answering `committed` must count as ready, or the other legs abort and the run stops being \
      uniform on exactly the path recovery makes reachable. **The run is modelled now** (2026-09-23, \
      Programme D unit 6): `runPhaseTwo` is the port's phase two, `decisionOf` its two decision lines \
      over `voteFromReply`'s booleans, and the three theorems above are the narrowed statement — so the \
      row is a model claim rather than a claim about the Rust evidenced by tests" },
  { number := 28, layer := "Cross-shard",
    rustWitness := ["rholang/src/native_state.rs:law28_txn_prepare_rejects_overdraw_and_is_idempotent"],
    statement := "Leg idempotency: `txn_prepare`/`txn_commit`/`txn_abort` are idempotent under the \
      transaction id — a retried leg returns the record it already has — and the two terminal verbs \
      refuse each other",
    status := .provedModel,
    declarations := [`Rchain.applyEffect, `Rchain.leg_idempotent, `Rchain.TxnRecord, `Rchain.Ledger,
      `Rchain.txnPrepare, `Rchain.txnCommit, `Rchain.txnAbort, `Rchain.txnPrepare_idempotent,
      `Rchain.txnCommit_idempotent, `Rchain.txnAbort_idempotent, `Rchain.commit_after_abort_is_an_error,
      `Rchain.abort_after_commit_is_an_error, `Rchain.prepare_refuses_overdraft],
    axioms := [],
    rust := ["rholang/src/native_state.rs"],
    witness := [`Rchain.commit_after_abort_is_an_error, `Rchain.abort_after_commit_is_an_error, `Rchain.prepare_refuses_overdraft, `Rchain.txnPrepare_idempotent],
    falsifiable := some "the negatives are the witnesses: a second `prepare` that re-escrowed would fail \
      `txnPrepare_idempotent`, and the port's own test pins the balance after a repeated \
      `txn_prepare`/`txn_commit` (`native_state.rs:txn_prepare`, `txn_prepare` and \
      `txn_commit` at `:1454`; the test is \
      `law28_txn_prepare_rejects_overdraw_and_is_idempotent`, `native_state.rs` (the symbol form: this test has moved twice now)); dropping the \
      early return \
      would let a retry fail on insufficient balance *after* the first call had already succeeded, which \
      the port's ordering (`native_state.rs:txn_prepare`, before the balance check) forbids. \
      `commit_after_abort_is_an_error` \
      fails for a verb that allowed the transition — the port returns \
      `Err(\"txn commit: already aborted\")` (`native_state.rs:txn_commit`), and `abort_after_commit_is_an_error` the mirror \
      (`native_state.rs:txn_prepare`). `prepare_refuses_overdraft` is the refusal with the port's own message (`native_state.rs:txn_prepare`)",
    note := "`leg_idempotent` is **proved now** — `funext` on a pointwise update, which is all it ever \
      needed — but it is the per-*shard-state* view, and the port's verbs are not pointwise updates: \
      they read a record, decide, and write a vault balance *and* a record. So the law is re-modelled on \
      the ledger the port keeps, where idempotence is the **early return on an existing record** \
      (`native_state.rs:txn_prepare`) rather than a coincidence of the arithmetic, and where the **fences** \
      are stated too — commit after abort is an error and abort after commit is an error \
      (`native_state.rs:txn_commit`, `native_state.rs:txn_abort`), which idempotence alone would permit. A fidelity note: the \
      model's `TxnState` carried a \
      fourth constructor (`proposed`) that the code does not have (`native_state.rs:vault_key`); a transaction with no \
      record is `none`, which is how the verbs spell it" },
  { number := 29, layer := "Cross-shard",
    rustWitness := [
      "casper/src/property_tests.rs:law29_a_terminal_record_never_changes_again",
      "casper/src/property_tests.rs:law27_an_abort_vote_prevents_a_later_commit",
      "casper/src/property_tests.rs:law27_a_legless_record_cannot_commit",
      "casper/src/property_tests.rs:law27_and_law29_the_state_agrees_with_the_votes"],
    statement := "The coordinator's decision is a *function* of its votes — `committed` iff every \
      participant voted ready — and it is a durable record a prepared participant can recover",
    status := .provedModel,
    declarations := [`Rchain.commit_record_deterministic_is_false, `Rchain.allReady,
      `Rchain.coordinatorDecision, `Rchain.allReady_eq_true,
      `Rchain.coordinator_decision_committed_iff, `Rchain.TxnState.IsTerminal,
      `Rchain.CoordRecord.recordVote, `Rchain.an_abort_is_absorbing, `Rchain.a_commit_is_absorbing,
      `Rchain.an_abort_vote_aborts_a_prepared_record, `Rchain.a_committed_record_stays_committed,
      `Rchain.Ledger.record_setRecord, `Rchain.txnCommit_fixes],
    rust := ["casper/src/txn_coordinator.rs"],
    witness := [`Rchain.commit_record_deterministic_is_false, `Rchain.coordinator_decision_committed_iff, `Rchain.an_abort_is_absorbing, `Rchain.a_commit_is_absorbing, `Rchain.a_committed_record_stays_committed, `Rchain.an_abort_vote_aborts_a_prepared_record],
    falsifiable := some "`coordinator_decision_committed_iff` is the decision half, proved against the \
      port's own two lines; the witness that the *old* statement was false is \
      `commit_record_deterministic_is_false` — a record whose state is `committed` while a vote is \
      `abort` — which contains no false claim and could not, since the record is a free type",
    note := "**the axiom this row used to cite was FALSE** — a property of *every* `CoordRecord` (`:77` \
      is freely constructible), refuted by `{state := committed, votes := [\"\", abort]}` \
      (`commit_record_deterministic_is_false`). Split in two: the **decision half is now proved** — \
      `allReady`/`coordinatorDecision` are the Rust's `all_ready`/`decision` lines, and \
      `coordinator_decision_committed_iff` says `committed` iff every vote is ready — while the \
      **durability half is now modelled too** (2026-09-23, Programme D unit 6): `CoordRecord.recordVote` \
      is the port's `record_vote` (`casper/src/gateway/ledger.rs:158-178`), whose first line — a terminal record \
      ignores later votes — is the fix for AUDIT C160, and the four theorems are the port's own \
      `an_abort_is_absorbing` / `a_commit_is_absorbing` plus the contrast that shows the guard is a \
      choice rather than a fact about votes (`an_abort_vote_aborts_a_prepared_record`). On the \
      participant's side the durability is `Ledger.record_setRecord` (a written record reads back) with \
      `txnCommit_fixes`; the row is `owed` no longer" },
]

end Laws
end Rchain
