//! The multi-parent DAG finalizer / fringe estimator.
//!
//! Mirrors `block-storage/src/main/scala/coop/rchain/blockstorage/dag/Finalizer.scala`. The
//! finality threshold is [`rchain_sdk::consensus::is_super_majority`] (Law 14: strictly > 2/3).

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use rchain_sdk::consensus::is_super_majority;
use rchain_shared::refined::{BlockHeight, NonNegI64, SeqNum};

use super::message_map;

/// A message (view) carrying all data the finalizer needs.
///
/// `hashCode` is overridden to `id.hashCode()` in the Scala (identity for set/map membership);
/// the Rust `Hash` impl mirrors that. `Eq`/`Ord` remain full structural comparison.
///
/// `height`/`sender_seq` carry their non-negativity structurally (`BlockHeight`/`SeqNum`), and the
/// `bonds_map` value is a *stake* amount (non-negative by the PoS invariant), carried as `NonNegI64`
/// (the `Bond` refinement).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Message<M, S> {
    pub id: M,
    pub height: BlockHeight,
    pub sender: S,
    pub sender_seq: SeqNum,
    pub bonds_map: BTreeMap<S, NonNegI64>,
    pub parents: BTreeSet<M>,
    pub fringe: BTreeSet<M>,
    /// Cache of seen message ids.
    ///
    /// Shared behind an `Arc` because this is the set that makes a `Message` expensive to copy: for
    /// a chain of N blocks it holds the block's whole ancestry, so `clone`-ing a `Message` used to
    /// copy Θ(N) ids — which the per-block and per-request paths did constantly (AUDIT C56), and
    /// which the DAG restore did once per block (C55). A clone is now a refcount bump.
    ///
    /// The *value* is unchanged, and that matters: law 15 pins this set as the construction
    /// `seenOf js id = (js.map (·.seen)).join ++ [id]` (`spec/Rchain/Casper/Fringe.lean:90`), with
    /// both inclusion halves proved. The representation is not what the law constrains, which is why
    /// this is not a register event — the Θ(N²) *residency* it implies is H6, still registered as an
    /// accepted-faithful residual, and still what a future pass would have to decide about.
    pub seen: Arc<BTreeSet<M>>,
}

impl<M: Hash, S> Hash for Message<M, S> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

/// **Why the derivation did not advance** — the gate's three exits, as data.
///
/// A stall is otherwise indistinguishable from the outside: the chain grows, nothing finalises, and
/// the logs say nothing (the 2026-09-29 measurement that stopped at finality 44 had exactly this
/// problem — see #70). Each variant carries the numbers that name the fault, and they are *returned
/// by the gate itself* rather than recomputed by an explainer, because two implementations of one
/// rule can disagree (`Split`, Law 51b) and this is the rule the node's finality runs on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoAdvance<S> {
    /// `check_min_messages` refused: the minimum messages' **senders** are not the partition's.
    /// `missing` is the partition minus those senders — a validator with no message anywhere in the
    /// non-finalized region (C174's shape), or a region no single layer covers.
    Coverage {
        missing: BTreeSet<S>,
        senders: BTreeSet<S>,
        /// How many senders the partition held. Carried so a reader can tell a **broad** region — most
        /// of the partition silent — from a narrow one, which the two sets above do not say on their
        /// own: one sender missing out of four is a validator that has not spoken yet this round, and
        /// one out of thirty is the same sentence about a different net.
        partition: usize,
    },
    /// `calculate_fringe` refused: a layer exists whose candidates were seen by the whole
    /// partition, but the stake behind them is not a supermajority.
    Support {
        supporting: i128,
        total: i128,
        full_partitions: usize,
        /// How many candidate messages the layer had. Compare with `partition`: on a healthy net this
        /// is smaller than the partition for most of a round, because a block justifies only the
        /// messages that existed when it was built (see [`partition`](NoAdvance::Support::partition)
        /// and #318).
        candidates: usize,
        /// How many senders the layer had to cover. **#318's missing number**: a `Support` refusal that
        /// reads `200 of 400 (1 full partition among 1 candidate)` is a *partial* layer — one
        /// validator's message seen by everyone — which is the ordinary state between the first and the
        /// last block of a round rather than a fault. Without the partition in the payload, that
        /// reading and a genuinely stuck one were the same string.
        partition: usize,
    },
    /// The progress guard: the derivation would publish exactly what is already finalised.
    AlreadyPublished,
}

/// Multi-parent finalization over a cache of all messages. The message map is borrowed (never
/// cloned) — the finalizer only reads it, so a per-call full-map clone is unnecessary (H6).
#[derive(Clone, Debug)]
pub struct Finalizer<'a, M, S> {
    pub msg_map: &'a BTreeMap<M, Message<M, S>>,
}

impl<'a, M, S> Finalizer<'a, M, S>
where
    M: Ord + Clone + Eq + Hash,
    S: Ord + Clone + Eq + Hash,
{
    pub fn new(msg_map: &'a BTreeMap<M, Message<M, S>>) -> Self {
        Self { msg_map }
    }

    fn msg(&self, id: &M) -> Option<Message<M, S>> {
        self.msg_map.get(id).cloned()
    }

    /// Iterate the sender's own parent chain (nearest to oldest), stopping at `finalized` messages.
    fn self_parents(
        &self,
        mv: &Message<M, S>,
        finalized: &BTreeSet<Message<M, S>>,
    ) -> Vec<Message<M, S>> {
        let mut chain = Vec::new();
        let mut next: Vec<Message<M, S>> = mv
            .parents
            .iter()
            .filter_map(|p| self.msg(p))
            .filter(|x| x.sender == mv.sender && !finalized.contains(x))
            .collect();
        while let Some(m) = next.pop() {
            chain.push(m.clone());
            next = m
                .parents
                .iter()
                .filter_map(|p| self.msg(p))
                .filter(|x| x.sender == mv.sender && !finalized.contains(x))
                .collect();
        }
        chain
    }

    /// Whether the minimum messages are enough for the next-fringe calculation: their **sender set**
    /// must be the bonded set, not merely as many senders as there are bonds.
    ///
    /// A count comparison — which is what the Scala oracle has, with the epoch TODO saying so
    /// (`Finalizer.scala:64-66`, *"simple comparison for senders count is not enough"*) — admitted
    /// `[A, A, B]` for bonds `{A, B, C}`: `calculate_next_layer` collapses the duplicate sender into one
    /// entry, so the published fringe **omitted bonded validator `C`** while presenting A's stake
    /// twice. **This is a deliberate departure from the oracle** — upstream has not made the decision —
    /// and it is registered with its reason in `spec/audit/passes.md` §6; `spec/Rchain/Casper/Dag.lean`
    /// models the same gate, and law 14b's statement of "one message per bonded validator" rests on it.
    /// The length clause is kept because it is the oracle's own first conjunct; the set comparison
    /// subsumes it.
    pub fn check_min_messages(
        &self,
        min_msgs: &[Message<M, S>],
        bonds_map: &BTreeMap<S, NonNegI64>,
    ) -> bool {
        let minimum_senders: BTreeSet<&S> =
            min_msgs.iter().map(|message| &message.sender).collect();
        let bonded_senders: BTreeSet<&S> = bonds_map.keys().collect();
        min_msgs.len() == bonds_map.len() && minimum_senders == bonded_senders
    }

    /// Find the top (most recent) message referenced from the minimum messages, per sender.
    pub fn calculate_next_layer(&self, min_msgs: &[Message<M, S>]) -> BTreeMap<S, Message<M, S>> {
        let mut min_messages_map: BTreeMap<S, Message<M, S>> = min_msgs
            .iter()
            .map(|x| (x.sender.clone(), x.clone()))
            .collect();
        let mut candidates: Vec<Message<M, S>> = min_msgs
            .iter()
            .flat_map(|x| x.parents.iter().filter_map(|p| self.msg(p)))
            .collect();
        candidates.retain(|x| min_messages_map.contains_key(&x.sender));
        for m in candidates {
            if let Some(curr) = min_messages_map.get(&m.sender) {
                if m.sender_seq > curr.sender_seq {
                    min_messages_map.insert(m.sender.clone(), m);
                }
            }
        }
        min_messages_map
    }

    /// For each justification sender, which next-layer messages each parent sees.
    pub fn calculate_next_fringe_support_map(
        &self,
        parents: &BTreeSet<Message<M, S>>,
        next_layer: &BTreeMap<S, Message<M, S>>,
        finalized: &BTreeSet<Message<M, S>>,
    ) -> BTreeMap<S, BTreeMap<S, BTreeSet<S>>> {
        let next_layer_ids: BTreeSet<M> = next_layer.values().map(|m| m.id.clone()).collect();
        let mut result = BTreeMap::new();
        for mv in parents {
            let mut seen_by: BTreeMap<S, BTreeSet<S>> = BTreeMap::new();
            for (s, min_msg) in next_layer {
                let parents_of_parent: BTreeSet<M> =
                    mv.parents.difference(&next_layer_ids).cloned().collect();
                let mut see_min_msg: BTreeSet<S> = BTreeSet::new();
                for p_id in &parents_of_parent {
                    if let Some(p) = self.msg(p_id) {
                        let mut self_msgs = vec![p.clone()];
                        self_msgs.extend(self.self_parents(&p, finalized));
                        if self_msgs.iter().any(|m| m.seen.contains(&min_msg.id)) {
                            see_min_msg.insert(p.sender.clone());
                        }
                    }
                }
                if !see_min_msg.is_empty() {
                    seen_by.insert(s.clone(), see_min_msg);
                }
            }
            if !seen_by.is_empty() {
                result.insert(mv.sender.clone(), seen_by);
            }
        }
        result
    }

    /// Whether the supporting stake is a supermajority (Law 14).
    ///
    /// **Two maps, because the gate asks two questions** (see [`super::liveness`]): `partition_bonds`'
    /// *keys* are the senders a candidate message must have been seen by, and `quorum_bonds`' *values*
    /// are what the supermajority is measured against. They were one map until 2026-09-29, which is why
    /// a bonded validator that produced no message capped finality however little stake it held; passing
    /// the same map twice is exactly the old behaviour, and is what the identity case of the model
    /// recovers (`Rchain.calculateFringe`).
    pub fn calculate_fringe(
        &self,
        next_fringe_support_map: &BTreeMap<S, BTreeMap<S, BTreeSet<S>>>,
        partition_bonds: &BTreeMap<S, NonNegI64>,
        quorum_bonds: &BTreeMap<S, NonNegI64>,
    ) -> bool {
        let must_be_seen: BTreeSet<S> = partition_bonds.keys().cloned().collect();
        let mut full_partition_stake: i128 = 0;
        for (sender, seen_by) in next_fringe_support_map {
            let all_bonded = sees_the_whole_partition(seen_by, &must_be_seen);
            // Only bonded senders contribute stake. A non-bonded justification sender must not
            // index the bonds map (it would panic) — skip it instead.
            if all_bonded {
                if let Some(stake) = quorum_bonds.get(sender) {
                    full_partition_stake += i128::from(i64::from(*stake));
                }
            }
        }
        let total_stake: i128 = quorum_bonds
            .values()
            .map(|v| i128::from(i64::from(*v)))
            .sum();
        is_super_majority(full_partition_stake, total_stake)
    }

    /// **The gate's arithmetic, once** — what `calculate_fringe` decides on, returned rather than only
    /// compared: `(full_partition_stake, total_stake, full_partitions, candidates)`.
    ///
    /// A refusal has to name the numbers it was made of ([`NoAdvance::Support`]), and computing them
    /// *beside* the gate would be two implementations of one rule — the disagreement `Split` (Law 51b)
    /// names, in the one place the node's finality needs a single answer. `calculate_fringe` is this
    /// function's comparison, and the refusal site reads the same tuple.
    pub fn calculate_fringe_numbers(
        &self,
        next_fringe_support_map: &BTreeMap<S, BTreeMap<S, BTreeSet<S>>>,
        partition_bonds: &BTreeMap<S, NonNegI64>,
        quorum_bonds: &BTreeMap<S, NonNegI64>,
    ) -> (i128, i128, usize, usize) {
        let must_be_seen: BTreeSet<S> = partition_bonds.keys().cloned().collect();
        let mut full_partition_stake: i128 = 0;
        let mut full_partitions: usize = 0;
        for (sender, seen_by) in next_fringe_support_map {
            let all_bonded = sees_the_whole_partition(seen_by, &must_be_seen);
            if all_bonded {
                full_partitions += 1;
                if let Some(stake) = quorum_bonds.get(sender) {
                    full_partition_stake += i128::from(i64::from(*stake));
                }
            }
        }
        let total_stake: i128 = quorum_bonds
            .values()
            .map(|v| i128::from(i64::from(*v)))
            .sum();
        (
            full_partition_stake,
            total_stake,
            full_partitions,
            next_fringe_support_map.len(),
        )
    }

    pub fn next_fringe_detailed(
        &self,
        justifications: &BTreeSet<Message<M, S>>,
        partition_bonds: &BTreeMap<S, NonNegI64>,
        quorum_bonds: &BTreeMap<S, NonNegI64>,
        prev_fringe: &BTreeSet<Message<M, S>>,
    ) -> Result<BTreeSet<Message<M, S>>, NoAdvance<S>> {
        // Minimum (oldest non-finalized) message from each justification sender.
        let mut min_msgs = Vec::new();
        for p in justifications {
            let mut chain = vec![p.clone()];
            chain.extend(self.self_parents(p, prev_fringe));
            // `chain` is non-empty by construction (seeded with `p`).
            min_msgs.push(chain.into_iter().last().unwrap_or_else(|| p.clone()));
        }
        if !self.check_min_messages(&min_msgs, partition_bonds) {
            let senders: BTreeSet<S> = min_msgs.iter().map(|m| m.sender.clone()).collect();
            let missing = partition_bonds
                .keys()
                .filter(|v| !senders.contains(*v))
                .cloned()
                .collect();
            return Err(NoAdvance::Coverage {
                missing,
                senders,
                partition: partition_bonds.len(),
            });
        }
        let next_layer = self.calculate_next_layer(&min_msgs);
        let fringe_support_map =
            self.calculate_next_fringe_support_map(justifications, &next_layer, prev_fringe);
        if !self.calculate_fringe(&fringe_support_map, partition_bonds, quorum_bonds) {
            // The numbers the refusal is made of, from the gate's own arithmetic — never a second
            // implementation of it (`Split`, Law 51b). Paid for only on a refusal, which is the path a
            // capture exists to explain rather than the path a chain takes.
            let (supporting, total, full_partitions, candidates) =
                self.calculate_fringe_numbers(&fringe_support_map, partition_bonds, quorum_bonds);
            return Err(NoAdvance::Support {
                supporting,
                total,
                full_partitions,
                candidates,
                partition: partition_bonds.len(),
            });
        }
        Ok(next_layer.values().cloned().collect())
    }

    /// Compute the fringe from joined justifications and any newly detected fringe.
    ///
    /// `partition_bonds` is the set a candidate message must have been seen by — in the node, the
    /// **live weight set** ([`super::liveness::calculate_finalization`]) — and `quorum_bonds` is the
    /// whole bonded map the supermajority is measured against. Passing the same map twice is the
    /// pre-2026-09-29 behaviour; the tests below do exactly that, which is what keeps them a statement
    /// about the gate rather than about the liveness rule layered over it.
    pub fn calculate_finalization(
        &self,
        justifications: &BTreeSet<Message<M, S>>,
        partition_bonds: &BTreeMap<S, NonNegI64>,
        quorum_bonds: &BTreeMap<S, NonNegI64>,
    ) -> (BTreeSet<Message<M, S>>, Option<BTreeSet<Message<M, S>>>) {
        let (parent, new_fringe, _why) =
            self.calculate_finalization_detailed(justifications, partition_bonds, quorum_bonds);
        (parent, new_fringe)
    }

    /// [`Self::calculate_finalization`], with the reason it did not advance — see [`NoAdvance`].
    ///
    /// The plain form delegates to this one, so the gate has a single implementation and the reason
    /// cannot drift from the decision it explains.
    pub fn calculate_finalization_detailed(
        &self,
        justifications: &BTreeSet<Message<M, S>>,
        partition_bonds: &BTreeMap<S, NonNegI64>,
        quorum_bonds: &BTreeMap<S, NonNegI64>,
    ) -> (
        BTreeSet<Message<M, S>>,
        Option<BTreeSet<Message<M, S>>>,
        Option<NoAdvance<S>>,
    ) {
        let parent_fringe = message_map::latest_fringe(self.msg_map, justifications);
        let mut current = parent_fringe.clone();
        let mut new_fringe_opt: Option<BTreeSet<Message<M, S>>> = None;
        // The reason the *last* attempt failed — the one that stopped the walk.
        let why: NoAdvance<S> = loop {
            match self.next_fringe_detailed(justifications, partition_bonds, quorum_bonds, &current)
            {
                Err(reason) => break reason,
                Ok(nf) => {
                    // Progress guard: a non-advancing fringe would loop forever. Only record a
                    // strictly new fringe.
                    if nf == current {
                        break NoAdvance::AlreadyPublished;
                    }
                    new_fringe_opt = Some(nf.clone());
                    current = nf;
                }
            }
        };
        // A walk that advanced somewhere has no reason to report; one that never published anything
        // reports why its last attempt failed.
        let reason = if new_fringe_opt.is_some() {
            None
        } else {
            Some(why)
        };
        (parent_fringe, new_fringe_opt, reason)
    }
}

/// Whether every next-layer message in `seen_by` was seen by the **whole partition** — the
/// full-partition filter `calculate_fringe` and `calculate_fringe_numbers` share.
///
/// **Containment, not equality.** `calculate_next_fringe_support_map` resolves seers through the full
/// message map, so a bonded validator outside the live partition that built on a candidate lands in its
/// seer set. Equality then dropped a candidate the whole partition had seen, and the survivors of a kill
/// were credited 0 of their stake (#213's `0 of 250`). With one map for both questions — the gate before
/// 2026-09-29, and the Scala's `forall(_ == bondedSenders)` — a seer outside the partition could only be a
/// sender with no bond, so the two readings almost always agreed; the live partition is what made them
/// differ routinely. A seer set that misses a partition member is still refused (law 52b), and the
/// quorum is still the whole bonded map, so a minority still cannot finalise.
fn sees_the_whole_partition<S: Ord>(
    seen_by: &BTreeMap<S, BTreeSet<S>>,
    must_be_seen: &BTreeSet<S>,
) -> bool {
    !seen_by.is_empty() && seen_by.values().all(|v| must_be_seen.is_subset(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn n8_support_requires_six_distinct_bonded_senders_and_complete_coverage() {
        let map: BTreeMap<i32, Message<i32, i32>> = BTreeMap::new();
        let finalizer = Finalizer::new(&map);
        let bonds: BTreeMap<_, _> = (0..8)
            .map(|v| (v, NonNegI64::try_from(100).unwrap()))
            .collect();
        let live: BTreeMap<_, _> = bonds
            .iter()
            .filter(|(v, _)| **v != 7)
            .map(|(v, s)| (*v, *s))
            .collect();
        let seers: BTreeSet<_> = (0..7).collect();
        let layer: BTreeMap<_, _> = (0..7).map(|v| (v, seers.clone())).collect();
        let mut support: BTreeMap<_, _> = (0..5).map(|v| (v, layer.clone())).collect();
        assert_eq!(
            finalizer
                .calculate_fringe_numbers(&support, &live, &bonds)
                .0,
            500
        );
        assert!(!finalizer.calculate_fringe(&support, &live, &bonds));
        support.insert(5, layer.clone());
        assert!(finalizer.calculate_fringe(&support, &live, &bonds));
        // A missing seer on even one candidate removes this sender's support.
        support.get_mut(&5).unwrap().get_mut(&0).unwrap().remove(&6);
        assert!(!finalizer.calculate_fringe(&support, &live, &bonds));
        // An unbonded sender cannot replace the missing sixth bonded supporter.
        support.insert(99, layer);
        assert!(!finalizer.calculate_fringe(&support, &live, &bonds));
        assert_eq!(
            finalizer
                .calculate_fringe_numbers(&support, &live, &bonds)
                .1,
            800
        );
    }

    fn msg(
        id: i32,
        sender: i32,
        sender_seq: i64,
        parents: &[i32],
        seen: &[i32],
    ) -> Message<i32, i32> {
        Message {
            id,
            height: BlockHeight::zero(),
            sender,
            sender_seq: SeqNum::try_from(sender_seq).unwrap(),
            bonds_map: BTreeMap::new(),
            parents: parents.iter().copied().collect(),
            fringe: BTreeSet::new(),
            seen: Arc::new(seen.iter().copied().collect()),
        }
    }

    fn bonded() -> BTreeMap<i32, NonNegI64> {
        [(0, 1), (1, 1), (2, 1)]
            .into_iter()
            .map(|(k, v)| (k, NonNegI64::try_from(v).unwrap()))
            .collect()
    }

    /// A support map in which every sender in `see_full` sees the full bonded partition.
    fn support_with_full(
        see_full: &[i32],
        bonded: &BTreeSet<i32>,
    ) -> BTreeMap<i32, BTreeMap<i32, BTreeSet<i32>>> {
        see_full
            .iter()
            .map(|&s| {
                let seen_by = bonded.iter().map(|&b| (b, bonded.clone())).collect();
                (s, seen_by)
            })
            .collect()
    }

    #[test]
    fn law14_fringe_requires_supermajority() {
        let map: BTreeMap<i32, Message<i32, i32>> = BTreeMap::new();
        let finalizer: Finalizer<i32, i32> = Finalizer::new(&map);
        let bonds = bonded();
        let bonded_senders: BTreeSet<i32> = bonds.keys().copied().collect();

        // All 3 validators see the full partition -> 3/3 > 2/3 -> finalizes.
        assert!(finalizer.calculate_fringe(
            &support_with_full(&[0, 1, 2], &bonded_senders),
            &bonds,
            &bonds
        ));

        // Only 2 of 3 -> 2/3 is NOT > 2/3 -> does not finalize.
        assert!(!finalizer.calculate_fringe(
            &support_with_full(&[0, 1], &bonded_senders),
            &bonds,
            &bonds
        ));
    }

    #[test]
    fn calculate_fringe_ignores_non_bonded_sender() {
        let map: BTreeMap<i32, Message<i32, i32>> = BTreeMap::new();
        let finalizer: Finalizer<i32, i32> = Finalizer::new(&map);
        let bonds = bonded();
        let bonded_senders: BTreeSet<i32> = bonds.keys().copied().collect();

        // A support map that includes a non-bonded sender (99) seeing the full partition. This
        // must not panic on `bonds_map[sender]` and must still finalize on the bonded 3/3 stake.
        let mut support = support_with_full(&[0, 1, 2], &bonded_senders);
        support.insert(
            99,
            bonded_senders
                .iter()
                .map(|&b| (b, bonded_senders.clone()))
                .collect(),
        );
        assert!(finalizer.calculate_fringe(&support, &bonds, &bonds));
    }

    #[test]
    fn check_min_messages_needs_all_bonded_senders() {
        let map: BTreeMap<i32, Message<i32, i32>> = BTreeMap::new();
        let finalizer: Finalizer<i32, i32> = Finalizer::new(&map);
        let bonds = bonded();
        assert!(finalizer.check_min_messages(
            &[
                msg(0, 0, 0, &[], &[]),
                msg(1, 1, 0, &[], &[]),
                msg(2, 2, 0, &[], &[])
            ],
            &bonds
        ));

        // Matching the bond count is insufficient: duplicate sender 0 leaves
        // bonded sender 2 uncovered.
        assert!(!finalizer.check_min_messages(
            &[
                msg(0, 0, 0, &[], &[]),
                msg(3, 0, 1, &[], &[]),
                msg(1, 1, 0, &[], &[])
            ],
            &bonds
        ));

        // A non-bonded sender cannot substitute for a missing bonded sender.
        assert!(!finalizer.check_min_messages(
            &[
                msg(0, 0, 0, &[], &[]),
                msg(1, 1, 0, &[], &[]),
                msg(99, 99, 0, &[], &[])
            ],
            &bonds
        ));

        assert!(!finalizer.check_min_messages(&[msg(0, 0, 0, &[], &[])], &bonds));
    }

    #[test]
    fn calculate_next_layer_picks_max_sender_seq() {
        // m0's parent is m0_later (same sender, higher sender_seq).
        let m0_later = msg(3, 0, 1, &[], &[]);
        let m0 = msg(0, 0, 0, &[3], &[]);
        let m1 = msg(1, 1, 0, &[], &[]);
        let m2 = msg(2, 2, 0, &[], &[]);
        let map: BTreeMap<i32, Message<i32, i32>> = [
            (0, m0.clone()),
            (1, m1.clone()),
            (2, m2.clone()),
            (3, m0_later.clone()),
        ]
        .into_iter()
        .collect();
        let finalizer: Finalizer<i32, i32> = Finalizer::new(&map);
        let min_msgs = [m0, m1, m2];
        let next = finalizer.calculate_next_layer(&min_msgs);
        assert_eq!(next[&0].id, 3); // validator 0's later parent wins
        assert_eq!(next[&1].id, 1);
        assert_eq!(next[&2].id, 2);
    }

    #[test]
    fn calculate_finalization_requires_exact_sender_coverage() {
        // Genesis by a non-bonded sender (99); three bonded senders 0/1/2.
        let genesis = msg(99, 99, 0, &[], &[99]);
        // Layer 1: a three-way fork — each sees only genesis.
        let a1 = msg(10, 0, 1, &[99], &[99, 10]);
        let b1 = msg(11, 1, 1, &[99], &[99, 11]);
        let c1 = msg(12, 2, 1, &[99], &[99, 12]);
        // Layer 2 + 3: convergence.
        let a2 = msg(20, 0, 2, &[10, 11, 12], &[99, 10, 11, 12, 20]);
        let b2 = msg(21, 1, 2, &[10, 11, 12], &[99, 10, 11, 12, 21]);
        let c2 = msg(22, 2, 2, &[10, 11, 12], &[99, 10, 11, 12, 22]);
        let a3 = msg(30, 0, 3, &[20, 21, 22], &[99, 10, 11, 12, 20, 21, 22, 30]);
        let b3 = msg(31, 1, 3, &[20, 21, 22], &[99, 10, 11, 12, 20, 21, 22, 31]);
        let c3 = msg(32, 2, 3, &[20, 21, 22], &[99, 10, 11, 12, 20, 21, 22, 32]);
        let a3_duplicate = msg(33, 0, 4, &[20, 21, 22], &[99, 10, 11, 12, 20, 21, 22, 33]);

        let map: BTreeMap<i32, Message<i32, i32>> = [
            genesis.clone(),
            a1.clone(),
            b1.clone(),
            c1.clone(),
            a2.clone(),
            b2.clone(),
            c2.clone(),
            a3.clone(),
            b3.clone(),
            c3.clone(),
            a3_duplicate.clone(),
        ]
        .into_iter()
        .map(|m| (m.id, m))
        .collect();
        let bonds: BTreeMap<i32, NonNegI64> = [(0, 80), (1, 10), (2, 10)]
            .into_iter()
            .map(|(k, v)| (k, NonNegI64::try_from(v).unwrap()))
            .collect();

        let finalizer: Finalizer<i32, i32> = Finalizer::new(&map);

        // Count-only coverage accepted these three messages even though sender 0
        // appeared twice and bonded sender 2 was absent. With 90/100 represented
        // stake, the malformed candidate could advance the fringe.
        let duplicate_justifications: BTreeSet<Message<i32, i32>> =
            [a3.clone(), a3_duplicate, b3.clone()].into_iter().collect();
        let (_parent, duplicate_fringe, why) =
            finalizer.calculate_finalization_detailed(&duplicate_justifications, &bonds, &bonds);
        assert!(
            duplicate_fringe.is_none(),
            "duplicate sender must not substitute for a missing bonded sender"
        );
        // **And the reason names the sender that went missing** — the assertion #70's second stop
        // needs: `Coverage { missing }` is what a validator with no message in the unfinalized region
        // looks like, and it must not be reported as the stake gate or the progress guard.
        match why {
            Some(NoAdvance::Coverage { missing, .. }) => {
                assert_eq!(
                    missing,
                    [2].into_iter().collect(),
                    "bonded sender 2 has no message"
                )
            }
            other => panic!("expected Coverage, got {other:?}"),
        }

        // Exact coverage by all bonded senders still advances the fringe.
        let justifications: BTreeSet<Message<i32, i32>> = [a3, b3, c3].into_iter().collect();
        let (_parent, new_fringe) =
            finalizer.calculate_finalization(&justifications, &bonds, &bonds);
        let ids: BTreeSet<i32> = new_fringe
            .expect("fringe should advance")
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(ids, [10, 11, 12].into_iter().collect());
    }

    /// **A seer outside the live partition does not void a candidate the whole partition has seen**
    /// (#213, the `0 of 250` stall).
    ///
    /// Bonds 100/100/50, and sender 2 has stopped: the partition is the live pair {0, 1}, the quorum the
    /// whole bonded map. Sender 2's last message `c2` saw layer 1 and is justified by the survivors, so
    /// `calculate_next_fringe_support_map` records 2 as a seer of `a1` and `b1` beside 0 and 1 — it
    /// resolves parents through the full message map, not the partition. The gate used to demand that
    /// the seer set *equal* the partition, so the extra seer dropped both candidates and the survivors'
    /// 200 of 250 was credited as 0. The partition is what must have seen a candidate; a validator
    /// outside it having seen it too takes nothing away.
    ///
    /// The control is the same chain without `c2`, which finalises under either reading — so the
    /// difference is the extra seer and nothing else.
    #[test]
    fn a_seer_outside_the_live_partition_does_not_void_a_candidate() {
        let genesis = msg(99, 99, 0, &[], &[99]);
        let a1 = msg(10, 0, 1, &[99], &[99, 10]);
        let b1 = msg(11, 1, 1, &[99], &[99, 11]);
        let c1 = msg(12, 2, 1, &[99], &[99, 12]);
        let partition: BTreeMap<i32, NonNegI64> = [(0, 100), (1, 100)]
            .into_iter()
            .map(|(k, v)| (k, NonNegI64::try_from(v).unwrap()))
            .collect();
        let quorum: BTreeMap<i32, NonNegI64> = [(0, 100), (1, 100), (2, 50)]
            .into_iter()
            .map(|(k, v)| (k, NonNegI64::try_from(v).unwrap()))
            .collect();
        let fringe_of = |messages: Vec<Message<i32, i32>>, tips: [i32; 2]| {
            let map: BTreeMap<i32, Message<i32, i32>> =
                messages.into_iter().map(|m| (m.id, m)).collect();
            let justifications: BTreeSet<Message<i32, i32>> =
                tips.iter().map(|id| map[id].clone()).collect();
            let finalizer: Finalizer<i32, i32> = Finalizer::new(&map);
            let (_parent, new_fringe, why) =
                finalizer.calculate_finalization_detailed(&justifications, &partition, &quorum);
            (
                new_fringe.map(|f| f.into_iter().map(|m| m.id).collect::<BTreeSet<i32>>()),
                why,
            )
        };

        // Control: sender 2 is never built on after layer 1, so every seer is in the partition.
        let control = vec![
            genesis.clone(),
            a1.clone(),
            b1.clone(),
            c1.clone(),
            msg(20, 0, 2, &[10, 11, 12], &[99, 10, 11, 12, 20]),
            msg(21, 1, 2, &[10, 11, 12], &[99, 10, 11, 12, 21]),
            msg(30, 0, 3, &[20, 21], &[99, 10, 11, 12, 20, 21, 30]),
            msg(31, 1, 3, &[20, 21], &[99, 10, 11, 12, 20, 21, 31]),
        ];
        let (fringe, why) = fringe_of(control, [30, 31]);
        assert_eq!(fringe, Some([10, 11].into_iter().collect()), "{why:?}");

        // The question: sender 2 spoke once more (`c2`, which saw layer 1) and then stopped, and the
        // survivors justify that last message.
        let with_c2 = vec![
            genesis,
            a1,
            b1,
            c1,
            msg(22, 2, 2, &[10, 11, 12], &[99, 10, 11, 12, 22]),
            msg(20, 0, 2, &[10, 11, 22], &[99, 10, 11, 12, 22, 20]),
            msg(21, 1, 2, &[10, 11, 22], &[99, 10, 11, 12, 22, 21]),
            msg(30, 0, 3, &[20, 21, 22], &[99, 10, 11, 12, 20, 21, 22, 30]),
            msg(31, 1, 3, &[20, 21, 22], &[99, 10, 11, 12, 20, 21, 22, 31]),
        ];
        let (fringe, why) = fringe_of(with_c2, [30, 31]);
        assert_eq!(
            fringe,
            Some([10, 11].into_iter().collect()),
            "the survivors hold 200 of 250 and both saw layer 1; the gate said {why:?}"
        );
    }

    /// **The seer test is containment, and it still refuses a seer set that misses a partition member**
    /// — law 52b's clause, at the level of the support map. Containment is what lets a seer outside the
    /// partition through (the test above); it must not also let through a candidate some partition
    /// member never saw.
    #[test]
    fn the_seer_test_is_containment_of_the_partition() {
        let map: BTreeMap<i32, Message<i32, i32>> = BTreeMap::new();
        let finalizer: Finalizer<i32, i32> = Finalizer::new(&map);
        let partition: BTreeMap<i32, NonNegI64> = [(0, 100), (1, 100)]
            .into_iter()
            .map(|(k, v)| (k, NonNegI64::try_from(v).unwrap()))
            .collect();
        let quorum: BTreeMap<i32, NonNegI64> = [(0, 100), (1, 100), (2, 50)]
            .into_iter()
            .map(|(k, v)| (k, NonNegI64::try_from(v).unwrap()))
            .collect();
        let support = |seers: &[i32]| -> BTreeMap<i32, BTreeMap<i32, BTreeSet<i32>>> {
            let seers: BTreeSet<i32> = seers.iter().copied().collect();
            [0, 1]
                .into_iter()
                .map(|candidate| {
                    let seen_by = [(0, seers.clone()), (1, seers.clone())]
                        .into_iter()
                        .collect();
                    (candidate, seen_by)
                })
                .collect()
        };
        // Exactly the partition, and the partition plus a non-live bonded seer: both 200 of 250.
        assert!(finalizer.calculate_fringe(&support(&[0, 1]), &partition, &quorum));
        assert!(finalizer.calculate_fringe(&support(&[0, 1, 2]), &partition, &quorum));
        assert_eq!(
            finalizer.calculate_fringe_numbers(&support(&[0, 1, 2]), &partition, &quorum),
            (200, 250, 2, 2)
        );
        // A partition member missing from the seers: refused, with or without the extra seer.
        assert!(!finalizer.calculate_fringe(&support(&[0]), &partition, &quorum));
        assert!(!finalizer.calculate_fringe(&support(&[0, 2]), &partition, &quorum));
        // And the quorum is still the whole bonded map: one survivor's 100 of 250 is not enough even
        // when it is the whole partition.
        let alone: BTreeMap<i32, NonNegI64> = [(0, NonNegI64::try_from(100).unwrap())].into();
        let one = [(0, [(0, [0, 2].into_iter().collect())].into_iter().collect())].into();
        assert!(!finalizer.calculate_fringe(&one, &alone, &quorum));
    }

    #[test]
    fn calculate_finalization_returns_none_without_supermajority_support() {
        // A lockstep chain: only one sender's message per layer, so no full-partition support.
        let genesis = msg(99, 99, 0, &[], &[99]);
        let a1 = msg(10, 0, 1, &[99], &[99, 10]);
        let b1 = msg(11, 1, 1, &[10], &[99, 10, 11]);
        let c1 = msg(12, 2, 1, &[11], &[99, 10, 11, 12]);
        let map: BTreeMap<i32, Message<i32, i32>> = [genesis, a1, b1, c1]
            .into_iter()
            .map(|m| (m.id, m))
            .collect();
        let bonds: BTreeMap<i32, NonNegI64> = [(0, 10), (1, 10), (2, 10)]
            .into_iter()
            .map(|(k, v)| (k, NonNegI64::try_from(v).unwrap()))
            .collect();
        let justifications: BTreeSet<Message<i32, i32>> =
            [map[&10].clone(), map[&11].clone(), map[&12].clone()]
                .into_iter()
                .collect();
        let finalizer: Finalizer<i32, i32> = Finalizer::new(&map);
        let (_parent, new_fringe, why) =
            finalizer.calculate_finalization_detailed(&justifications, &bonds, &bonds);
        assert!(new_fringe.is_none(), "lockstep chain must not finalize");
        // **The reason is the stake gate, not the coverage gate** — the distinction #70's measurement
        // needed: a lockstep chain has min messages covering the partition (every sender speaks), and
        // what stops it is that no candidate was seen by all of them.
        assert!(
            matches!(why, Some(NoAdvance::Support { .. })),
            "expected the stake gate, got {why:?}"
        );
    }
    /// **C69's second half, settled: the fringe walk is bounded by the non-finalized chain length.**
    ///
    /// The progress guard (`if nf == current { break }`, `calculate_finalization` above) catches an
    /// *immediate* repeat only, which the §6 row recorded as "weaker than a cycle guard: a longer cycle is
    /// unimpeded — nothing has measured one". The measure is what makes that vacuous rather than
    /// unmeasured: each iteration passes the *previous fringe* as the cutoff, and `self_parents` stops at
    /// it, so for every sender the minimum message can only move along that sender's own chain in one
    /// direction. A sender's unfinalized chain is finite and acyclic, and a step that is not the fixed
    /// point moves strictly, so the walk terminates in at most (messages on the longest unfinalized chain)
    /// steps — a cycle of any length would have to move strictly forever on a finite chain.
    ///
    /// This drives the loop the way `calculate_finalization` does (reproduced here so the *count* is
    /// observable; `next_fringe` is private to this module) over a **generated `L`-layer fork**, so the
    /// walk is long rather than trivially short: `L` layers of three bonded senders, each layer
    /// justifying the one below. The measured steps grow with `L` and stay within the non-finalized
    /// length; the assertion sits *inside* the loop, so a hypothetical cycle fails the test rather than
    /// hanging it.
    #[test]
    fn the_fringe_walk_is_bounded_by_the_non_finalized_chain_length() {
        const LAYERS: i64 = 8;

        // Layer 0 is a genesis by a non-bonded sender; layers 1..=L are a three-way fork that converges
        // to the full previous layer at each step (the shape `calculate_finalization_advances_fringe_on_fork`
        // uses, extended so the walk has somewhere to go).
        let genesis = msg(99, 99, 0, &[], &[99]);
        let mut map: BTreeMap<i32, Message<i32, i32>> = [(genesis.id, genesis.clone())].into();
        let mut seen: Vec<i32> = vec![99];
        let mut layer_ids: Vec<i32> = vec![99];
        let mut messages = 1usize;
        for layer in 1..=LAYERS {
            let mut next_ids = Vec::new();
            for sender in 0..3i32 {
                let id = i32::try_from(layer * 10).expect("small ids") + sender;
                let m = msg(id, sender, layer, &layer_ids, &seen);
                seen.push(id);
                next_ids.push(id);
                map.insert(id, m);
                messages += 1;
            }
            layer_ids = next_ids;
        }
        let bonds: BTreeMap<i32, NonNegI64> = [(0, 10), (1, 10), (2, 10)]
            .into_iter()
            .map(|(k, v)| (k, NonNegI64::try_from(v).unwrap()))
            .collect();
        let justifications: BTreeSet<Message<i32, i32>> =
            layer_ids.iter().map(|id| map[id].clone()).collect();

        let finalizer: Finalizer<i32, i32> = Finalizer::new(&map);
        let mut current = message_map::latest_fringe(finalizer.msg_map, &justifications);
        let mut steps = 0usize;
        loop {
            let Ok(nf) = finalizer.next_fringe_detailed(&justifications, &bonds, &bonds, &current)
            else {
                break;
            };
            if nf == current {
                break;
            }
            steps += 1;
            assert!(
                steps <= messages,
                "the fringe walk took more than {messages} steps ({steps}) over a {LAYERS}-layer \
                 fixture without reaching a fixed point — the walk is not bounded by the \
                 non-finalized chain"
            );
            current = nf;
        }
        println!(
            "a {LAYERS}-layer fork: the fringe walk took {steps} step(s), bounded by {messages} \
             messages of non-finalized chain"
        );
        assert!(
            steps > 1,
            "the fixture must make the walk advance more than once, or it is not exercising the bound"
        );
    }
}
