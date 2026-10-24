//! Per-network DAG message state.
//!
//! Mirrors `block-storage/src/main/scala/coop/rchain/blockstorage/dag/DagMessageState.scala`.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::Hash;
use std::sync::Arc;

use rchain_shared::refined::{BlockHeight, NonNegI64, SeqNum};

use super::finalizer::{Finalizer, Message};
use super::message_map;
use crate::errors::StorageError;

/// The per-sender latest messages and the full message map.
///
/// Invariant: `latest_msgs` is keyed by sender, so the one-message-per-sender invariant is
/// structural (a map cannot hold two entries for one sender); every entry is also present in
/// `msg_map`. `insert_msg_mut` enforces the subset invariant with a `debug_assert!`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DagMessageState<M, S> {
    pub latest_msgs: BTreeMap<S, Message<M, S>>,
    pub msg_map: BTreeMap<M, Message<M, S>>,
    /// **The parent set a new block justifies** — the `latest_msgs` as of the last round boundary, and
    /// not `latest_msgs` itself.
    ///
    /// This is not a heuristic: the fringe gate refuses `latest_msgs` outright. `calculate_fringe`
    /// counts a candidate only if the whole live partition has seen every next-layer message *through
    /// messages beyond that layer* (`calculate_next_fringe_support_map` reads `parents ∖ next_layer`),
    /// and a parent set that is every sender's newest message has no such remainder — the head of each
    /// round has an empty one and the later movers have seen a prefix. Measured in
    /// `casper/tests/finalization.rs`: the chain built from `latest_msgs` never finalises, and the same
    /// chain with a per-round snapshot finalises at a constant 12-height lag (48/60, 168/180, 348/360).
    ///
    /// A **round boundary** is the first message at which every sender of the live weight set has a
    /// message above the previous boundary. That is a function of the DAG alone — the bond map rides on
    /// every message — so every node computes the same boundary from the same history, and a validator
    /// that stops proposing drops out of the live set on the usual `LIVENESS_WINDOW` and stops holding
    /// the boundary back.
    pub round_parents: BTreeMap<S, Message<M, S>>,
    /// The height `round_parents` was taken at — the last round boundary. Zero before the first.
    pub round_height: BlockHeight,
}

impl<M, S> DagMessageState<M, S>
where
    M: Ord + Clone + Eq + Hash,
    S: Ord + Clone + Eq + Hash,
{
    pub fn empty() -> Self {
        Self::from_parts(BTreeMap::new(), BTreeMap::new())
    }

    /// Build a state from its two parts, with the round boundary **derived** rather than left for the
    /// caller to get right — the fields are public, and a state whose `round_parents` disagrees with its
    /// `latest_msgs` would hand a proposer parents the gate cannot use.
    pub fn from_parts(
        latest_msgs: BTreeMap<S, Message<M, S>>,
        msg_map: BTreeMap<M, Message<M, S>>,
    ) -> Self {
        let mut state = Self {
            latest_msgs,
            msg_map,
            round_parents: BTreeMap::new(),
            round_height: BlockHeight::zero(),
        };
        state.advance_round();
        state
    }

    /// Take a new round boundary if the current tip closes one.
    ///
    /// Called after every insert ([`Self::insert_msg_mut`]), so the boundary is always current when a
    /// proposer asks. The bonds come off any message (they ride on all of them).
    ///
    /// **A bonded sender closes the boundary when it has a message above the last one — and it stops
    /// holding it back once [`super::liveness::LIVENESS_WINDOW`] heights have passed without one.** The
    /// two halves are both needed and neither alone is right:
    ///
    /// - *waiting for a sender that has not spoken yet* is what makes a round a round. Closing on the
    ///   first message of a fresh chain puts `round_parents` at that single message, and the fringe then
    ///   refuses with `150 of 250` — the snapshot has to be a **set**, not a stack;
    /// - *waiting for ever* is #70 again: a validator that never comes back would freeze the parent set,
    ///   so a sender silent past the window is retired and the boundary closes without it. That is the
    ///   same window and the same rule the fringe gate uses, which is the point of reusing it.
    fn advance_round(&mut self) {
        let Some(tip) = self.latest_msgs.values().map(|m| m.height).max() else {
            return;
        };
        if self.round_parents.is_empty() {
            self.round_parents = self.latest_msgs.clone();
            self.round_height = tip;
            return;
        }
        if tip <= self.round_height {
            return;
        }
        let Some(bonds) = self
            .latest_msgs
            .values()
            .next()
            .map(|m| m.bonds_map.clone())
        else {
            return;
        };
        let window = super::liveness::LIVENESS_WINDOW;
        let heights = super::liveness::latest_heights(
            self.latest_msgs
                .values()
                .map(|m| (m.sender.clone(), m.height)),
        );
        // With no strict bonded supermajority, supported progress remains zero;
        // absent/stale senders cannot be retired by a minority tip alone.
        // This can leave the boundary frozen until quorum-backed progress resumes.
        // One fast proposer may not age out six still-active validators.
        // Use the same stake-backed progress clock as the finalizer.
        let supported_tip = super::liveness::quorum_supported_height(&bonds, &heights);
        let closed = bonds.keys().all(|s| {
            match (self.latest_msgs.get(s), self.round_parents.get(s)) {
                // Progress is relative to THIS sender's previous boundary message.
                // The quorum clock can be below another sender's boundary height:
                // comparing to one scalar round_height then reuses an old message
                // as if that sender had spoken in the new round.
                (Some(latest), Some(boundary)) if latest.sender_seq > boundary.sender_seq => true,
                // A sender new to the snapshot has spoken since the boundary.
                (Some(_), None) => true,
                // A sender which has not advanced retires only after the
                // stake-backed liveness window, never after one fast proposer.
                (Some(latest), Some(_)) => {
                    super::liveness::heights_behind(supported_tip, latest.height) > window
                }
                // Never spoken: allow the initial grace once, measured from
                // chain origin by the whole-stake quorum clock. Measuring from
                // round_height rearmed this absent sender after every boundary,
                // freezing each fresh quiet round for another full window.
                (None, _) => i64::from(supported_tip) > window,
            }
        });
        if closed {
            self.round_parents = self.latest_msgs.clone();
            // Next round must be measured from collective progress, not the
            // outlier's height (or it would wait for everyone to reach that tip).
            self.round_height = supported_tip;
        }
    }

    /// **The parent set for a new block** — the pure round snapshot, which is what the fringe gate can
    /// advance on.
    ///
    /// Unconditional, and it has to be. A validator must not propose twice inside one round, because
    /// `block_creator.rs:58-75` derives `block_num` and `seq_num` from this set and **every validator checks
    /// both against it** — `validate.rs:236` requires `max(justifications) + 1 == block_number` and `:259`
    /// requires `creator_latest_seq + 1 == seq_num`, so a second proposal against the same snapshot is
    /// refused as an equivocation, correctly. The proposer is what enforces the rule
    /// ([`Self::has_advanced_past_the_round`]); this function stays pure so that every proposal that is
    /// admitted has the parents the gate wants.
    pub fn parents_for_new_block(&self) -> BTreeSet<Message<M, S>> {
        if self.round_parents.is_empty() {
            return self.latest_msgs.values().cloned().collect();
        }
        self.round_parents.values().cloned().collect()
    }

    /// **[`Self::parents_for_new_block`] with the sender's own entry replaced by its newest message** — the
    /// escape, for a proposer that has already spoken this round and has been waiting past
    /// the elapsed `ROUND_STALL_ESCAPE` interval for the round to close.
    ///
    /// It exists because refusing has no way out. The round closes only when every bonded sender has
    /// advanced past the boundary, a quiet sender stops holding it back only once `LIVENESS_WINDOW` heights
    /// have passed above its last message, and that is measured from the tip — which a refusal freezes. So a
    /// validator killed while inside the window is never retired and the round never closes: measured on the
    /// node (flat height for the whole window after a kill, no error) and in-process
    /// (`the_round_closes_when_a_validator_goes_quiet_inside_the_window`). This parent set moves the tip, so
    /// the window ages the quiet sender out and the round closes on its own — and the next proposal is an
    /// ordinary one again.
    ///
    /// **It may not advance the fringe, and that is the accepted cost.** It is taken only after the
    /// elapsed-stall wait, so in a healthy round it is never taken at all; the rate matters more
    /// than this block's own finality, because a chain that cannot move cannot finalise anything.
    ///
    /// Replaced, not added: a parent set with two messages from one sender makes the block creator's
    /// `find(|m| m.sender == me)` ambiguous, it picks the older one, and the tip freezes with the numbers
    /// stuck — measured, and the reason the first version of this escape admitted 74 proposals to a tip that
    /// never left 2.
    pub fn parents_for_new_block_escaping(&self, sender: &S) -> BTreeSet<Message<M, S>> {
        if self.round_parents.is_empty() {
            return self.latest_msgs.values().cloned().collect();
        }
        let mut parents = self.round_parents.clone();
        if let Some(mine) = self.latest_msgs.get(sender) {
            parents.insert(sender.clone(), mine.clone());
        }
        parents.values().cloned().collect()
    }

    /// **Whether `sender` has already produced a block since the last round boundary** — and so must not
    /// propose again until the round closes, on pain of equivocating with itself.
    ///
    /// The proposer's veto, not this module's: the escape above is what keeps the veto from deadlocking.
    pub fn has_advanced_past_the_round(&self, sender: &S) -> bool {
        if self.round_parents.is_empty() {
            return false;
        }
        // **Against the snapshot's entry for this sender, not against `round_height`.** A boundary set at a
        // tip *above* this sender's own last block leaves `latest.height > round_height` false while the
        // snapshot's entry for it is older still — and the pure snapshot then hands back a sequence number
        // already used. Measured: one node logged ten `equivocation detected` lines in three seconds that
        // way, in one of three attempts, before the round closed and it recovered on its own.
        match (self.latest_msgs.get(sender), self.round_parents.get(sender)) {
            (Some(latest), Some(at_boundary)) => latest.sender_seq > at_boundary.sender_seq,
            // Spoke after the boundary and was not in it.
            (Some(_), None) => true,
            (None, _) => false,
        }
    }

    /// Create a new message, generating its finalization fringe.
    #[allow(clippy::too_many_arguments)]
    pub fn create_message(
        &self,
        id: M,
        height: BlockHeight,
        sender: S,
        sender_seq: SeqNum,
        fin_bonds_map: BTreeMap<S, NonNegI64>,
        justifications: &BTreeSet<Message<M, S>>,
    ) -> Message<M, S> {
        let finalizer = Finalizer::new(&self.msg_map);
        // Through the liveness rule, not the raw gate: the creator writes this fringe into the block,
        // and a validator that derived a different one would refuse the block it was handed — so both
        // sides must come through here (see `super::liveness`).
        let (parent_fringe, new_fringe_opt) =
            super::liveness::calculate_finalization(&finalizer, justifications, &fin_bonds_map);

        let new_fringe = new_fringe_opt.unwrap_or(parent_fringe);
        let new_fringe_ids: BTreeSet<M> = new_fringe.iter().map(|m| m.id.clone()).collect();

        let mut new_seen: BTreeSet<M> = justifications
            .iter()
            .flat_map(|j| j.seen.iter().cloned())
            .collect();
        new_seen.insert(id.clone());

        let justification_keys: BTreeSet<M> = justifications.iter().map(|j| j.id.clone()).collect();

        Message {
            id,
            height,
            sender,
            sender_seq,
            bonds_map: fin_bonds_map,
            parents: justification_keys,
            fringe: new_fringe_ids,
            seen: Arc::new(new_seen),
        }
    }

    /// Insert a message **in place** (no-op if its id is already present). Only a higher
    /// `sender_seq` replaces the sender's latest message (the Law 15 monotonicity invariant).
    ///
    /// The in-place form is what a *bulk* caller must use. The Scala's `insertMsg` returns a state
    /// built with `Map + (k -> v)` over an **immutable** map, whose structural sharing makes that
    /// O(log N); the Rust `BTreeMap` has no structural sharing, so a persistent-shaped
    /// implementation that clones the map per insert costs O(N) per call — and O(N²) when each
    /// entry's own `seen` set is deep-copied with it. Rebuilding a stored chain through
    /// [`Self::insert_msg`] that way was Θ(N³): a 5,844-block devnet restart spent longer than the
    /// devnet's own 120 s healthcheck inside `BlockDagKeyValueStorage::create` and looked like a
    /// hang (AUDIT C55).
    pub fn insert_msg_mut(&mut self, msg: &Message<M, S>) {
        if self.msg_map.contains_key(&msg.id) {
            return;
        }
        let replace = self
            .latest_msgs
            .get(&msg.sender)
            .map(|cur| msg.sender_seq > cur.sender_seq)
            .unwrap_or(true);
        self.msg_map.insert(msg.id.clone(), msg.clone());
        if replace {
            self.latest_msgs.insert(msg.sender.clone(), msg.clone());
        }

        debug_assert!(
            self.latest_msgs
                .values()
                .all(|m| self.msg_map.contains_key(&m.id)),
            "latest_msgs must be a subset of msg_map"
        );
        // The boundary is recomputed here, after the insert, so a proposer reading `round_parents`
        // cannot see a stale one.
        self.advance_round();
    }

    /// [`Self::insert_msg_mut`] as a value: one clone, then the insert. Kept for the callers that
    /// hand the next state on (the per-block path and `create_msg_and_update_sender`), so the
    /// acceptance rule lives in exactly one place.
    pub fn insert_msg(&self, msg: &Message<M, S>) -> Self {
        let mut next = self.clone();
        next.insert_msg_mut(msg);
        next
    }

    /// [`Self::insert_msg_without_latest`] in place.
    ///
    /// Used for validation-failed blocks: they must be recorded in the map (so
    /// `neglectedInvalidBlock` and justification-regression can see them) but must not become a
    /// proposer's parent, otherwise a single failed block would wedge block production (H-2).
    pub fn insert_msg_without_latest_mut(&mut self, msg: &Message<M, S>) {
        if self.msg_map.contains_key(&msg.id) {
            return;
        }
        self.msg_map.insert(msg.id.clone(), msg.clone());
    }

    /// **[`Self::insert_msg_mut`] for a message the map already holds** — the promotion a *cleared*
    /// failure record needs (AUDIT C193).
    ///
    /// H-2 records a validation-failed block through [`Self::insert_msg_without_latest_mut`], which is
    /// what keeps it out of a proposer's parent set — and therefore out of `latest_msgs`, and therefore
    /// out of the arithmetic `block_creator.rs` derives `seq_num` from. When the restoring rule clears
    /// that record the block becomes usable again, and this is the other half of the exclusion:
    /// `insert_msg_mut` returns early for an id it already holds (right for its own contract — a message
    /// is inserted once), so without this the block stays out of `latest_msgs` for ever, and the
    /// proposer keeps deriving a sequence number it has already spent.
    ///
    /// A no-op for an id the map does *not* hold: that is an insertion, and `insert_msg_mut` is the
    /// function for it.
    pub fn promote_msg_mut(&mut self, msg: &Message<M, S>) {
        if !self.msg_map.contains_key(&msg.id) {
            return;
        }
        let replace = self
            .latest_msgs
            .get(&msg.sender)
            .map(|cur| msg.sender_seq > cur.sender_seq)
            .unwrap_or(true);
        if replace {
            self.latest_msgs.insert(msg.sender.clone(), msg.clone());
        }
        self.advance_round();
    }

    /// [`Self::insert_msg_without_latest_mut`] as a value: one clone, then the insert.
    pub fn insert_msg_without_latest(&self, msg: &Message<M, S>) -> Self {
        let mut next = self.clone();
        next.insert_msg_without_latest_mut(msg);
        next
    }

    /// Create a new message for `creator` and insert it.
    pub fn create_msg_and_update_sender<F>(
        &self,
        creator: &S,
        gen_msg_id: F,
    ) -> Result<(Self, Message<M, S>), StorageError>
    where
        F: FnOnce(&S, BlockHeight) -> M,
    {
        let max_height = self
            .latest_msgs
            .values()
            .map(|m| m.height)
            .max()
            .ok_or(StorageError::EmptyLatestMessages)?;
        let new_height = max_height + NonNegI64::one();
        let seq_num = self
            .latest_msgs
            .get(creator)
            .map(|m| m.sender_seq)
            .unwrap_or_else(SeqNum::zero);
        let new_seq_num = seq_num + NonNegI64::one();
        let justifications: BTreeSet<Message<M, S>> = self.parents_for_new_block();
        let bonds_map = self
            .latest_msgs
            .values()
            .next()
            .ok_or(StorageError::EmptyLatestMessages)?
            .bonds_map
            .clone();

        let msg_id = gen_msg_id(creator, new_height);
        let new_msg = self.create_message(
            msg_id,
            new_height,
            creator.clone(),
            new_seq_num,
            bonds_map,
            &justifications,
        );
        Ok((self.insert_msg(&new_msg), new_msg))
    }

    /// The latest fringe, using the latest messages as parents.
    pub fn latest_fringe(&self) -> BTreeSet<Message<M, S>> {
        let latest: BTreeSet<Message<M, S>> = self.latest_msgs.values().cloned().collect();
        message_map::latest_fringe(&self.msg_map, &latest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn msg(id: i32, sender: i32, sender_seq: i64) -> Message<i32, i32> {
        Message {
            id,
            height: BlockHeight::zero(),
            sender,
            sender_seq: SeqNum::try_from(sender_seq).unwrap(),
            bonds_map: BTreeMap::new(),
            parents: BTreeSet::new(),
            fringe: BTreeSet::new(),
            seen: Arc::new([id].into_iter().collect()),
        }
    }

    #[test]
    fn law15_insert_msg_is_monotone() {
        let state: DagMessageState<i32, i32> = DagMessageState::empty();
        let genesis = msg(0, 0, 0);
        let s1 = state.insert_msg(&genesis);
        assert_eq!(s1.latest_msgs.get(&0), Some(&genesis));

        // Same sender_seq does NOT replace the latest (monotone, no regression).
        let stale = msg(1, 0, 0);
        let s2 = s1.insert_msg(&stale);
        assert_eq!(s2.latest_msgs.get(&0), Some(&genesis));

        // A higher sender_seq replaces the latest.
        let newer = msg(2, 0, 1);
        let s3 = s2.insert_msg(&newer);
        assert_eq!(s3.latest_msgs.get(&0), Some(&newer));

        // The message map still holds every message.
        assert_eq!(s3.msg_map.len(), 3);
    }

    #[test]
    fn insert_msg_is_idempotent() {
        let state: DagMessageState<i32, i32> = DagMessageState::empty();
        let genesis = msg(0, 0, 0);
        let s1 = state.insert_msg(&genesis);
        let s2 = s1.insert_msg(&genesis);
        assert_eq!(s1, s2);
    }

    #[test]
    fn create_message_seen_is_parents_seen_plus_id() {
        let state: DagMessageState<i32, i32> = DagMessageState::empty();
        let genesis = msg(0, 0, 0);
        let justifications: BTreeSet<_> = [genesis].into_iter().collect();
        let new_msg = state.create_message(
            1,
            BlockHeight::try_from(1).unwrap(),
            1,
            SeqNum::try_from(1).unwrap(),
            BTreeMap::new(),
            &justifications,
        );
        assert_eq!(*new_msg.seen, [0, 1].into_iter().collect());
        assert_eq!(new_msg.parents, [0].into_iter().collect());
    }

    fn at(id: i32, sender: i32, seq: i64, height: i64, bonds: &[(i32, i64)]) -> Message<i32, i32> {
        Message {
            id,
            height: BlockHeight::try_from(height).unwrap(),
            sender,
            sender_seq: SeqNum::try_from(seq).unwrap(),
            bonds_map: bonds
                .iter()
                .map(|(s, b)| (*s, NonNegI64::try_from(*b).unwrap()))
                .collect(),
            parents: BTreeSet::new(),
            fringe: BTreeSet::new(),
            seen: Arc::new([id].into_iter().collect()),
        }
    }

    /// The quorum clock is a liveness measure, not a per-sender round marker.
    /// Stake 100/100/50 yields a quorum-supported height 2 while the
    /// boundary contains a sender at height 3 (and another at height 4).
    /// Those OLD messages cannot count as new-round proposals.
    #[test]
    fn quorum_clock_below_a_senders_boundary_does_not_reuse_that_message() {
        let bonds = [(0, 100), (1, 100), (2, 50)];
        let mut state: DagMessageState<i32, i32> = DagMessageState::empty();
        for sender in 0..3 {
            state.insert_msg_mut(&at(sender, sender, 0, sender as i64 + 1, &bonds));
        }
        // The first boundary was just sender 0, so sender 0 must
        // advance once before all three can enter the next snapshot.
        state.insert_msg_mut(&at(10, 0, 1, 4, &bonds));
        assert_eq!(state.round_height, BlockHeight::try_from(2).unwrap());
        assert_eq!(state.round_parents.len(), 3);
        assert_eq!(
            state.round_parents.get(&0).unwrap().sender_seq,
            SeqNum::try_from(1).unwrap()
        );
        assert_eq!(
            state.round_parents.get(&2).unwrap().sender_seq,
            SeqNum::zero()
        );

        state.insert_msg_mut(&at(11, 0, 2, 5, &bonds));
        state.insert_msg_mut(&at(12, 1, 1, 6, &bonds));
        assert_eq!(
            state.round_parents.get(&0).unwrap().sender_seq,
            SeqNum::try_from(1).unwrap(),
            "no new snapshot before validator 2 speaks again"
        );
        assert_eq!(
            state.round_height,
            BlockHeight::try_from(2).unwrap(),
            "validator 2's height 3 at the previous boundary is not fresh progress"
        );

        state.insert_msg_mut(&at(13, 2, 1, 7, &bonds));
        assert_eq!(
            state.round_parents.get(&0).unwrap().sender_seq,
            SeqNum::try_from(2).unwrap()
        );
        assert_eq!(
            state.round_parents.get(&1).unwrap().sender_seq,
            SeqNum::try_from(1).unwrap()
        );
        assert_eq!(
            state.round_parents.get(&2).unwrap().sender_seq,
            SeqNum::try_from(1).unwrap()
        );
    }

    /// With fewer than strictly 2/3 of total stake represented, no
    /// quorum-backed progress height exists. One fast proposer cannot
    /// silently close the round; the boundary may move again once a
    /// full-stake supermajority resumes sending messages.
    #[test]
    fn a_never_spoken_validator_does_not_rearm_its_grace_every_round() {
        let bonds = [(0, 100), (1, 100), (2, 100), (3, 100)];
        let window = super::super::liveness::LIVENESS_WINDOW;
        let mut state: DagMessageState<i32, i32> = DagMessageState::empty();
        state.insert_msg_mut(&at(0, 0, 0, 1, &bonds));
        for step in 1..=window + 1 {
            for sender in 0..3 {
                let seq = if sender == 0 { step } else { step - 1 };
                state.insert_msg_mut(&at(
                    100 + step as i32 * 10 + sender,
                    sender,
                    seq,
                    step + 1,
                    &bonds,
                ));
            }
        }
        let first_boundary = window + 2;
        assert_eq!(i64::from(state.round_height), first_boundary);
        assert!(!state.latest_msgs.contains_key(&3));
        for sender in 0..3 {
            let seq = i64::from(state.latest_msgs[&sender].sender_seq) + 1;
            state.insert_msg_mut(&at(1000 + sender, sender, seq, first_boundary + 1, &bonds));
        }
        assert_eq!(
            i64::from(state.round_height),
            first_boundary + 1,
            "a never-spoken validator rearmed its grace after the quorum already retired it"
        );
    }

    #[test]
    fn missing_supermajority_freezes_round_until_quorum_returns() {
        let bonds = [(0, 100), (1, 100), (2, 100)];
        let mut state: DagMessageState<i32, i32> = DagMessageState::empty();
        state.insert_msg_mut(&at(0, 0, 0, 1, &bonds));
        assert_eq!(state.round_height, BlockHeight::try_from(1).unwrap());

        for height in 2..=12 {
            state.insert_msg_mut(&at(100 + height as i32, 0, height - 1, height, &bonds));
        }
        let bonded = state.latest_msgs.values().next().unwrap().bonds_map.clone();
        let latest = super::super::liveness::latest_heights(
            state.latest_msgs.values().map(|m| (m.sender, m.height)),
        );
        assert_eq!(
            super::super::liveness::quorum_supported_height(&bonded, &latest),
            BlockHeight::zero(),
            "one sender alone is not a quorum"
        );
        assert_eq!(state.round_height, BlockHeight::try_from(1).unwrap());
        assert_eq!(state.round_parents.len(), 1);

        state.insert_msg_mut(&at(201, 1, 0, 13, &bonds));
        // Exactly 2/3 is still not the strict supermajority.
        assert_eq!(state.round_height, BlockHeight::try_from(1).unwrap());
        assert_eq!(state.round_parents.len(), 1);

        state.insert_msg_mut(&at(202, 2, 0, 14, &bonds));
        assert_eq!(state.round_height, BlockHeight::try_from(12).unwrap());
        assert_eq!(state.round_parents.len(), 3);
        for sender in 0..3 {
            assert!(
                !state.has_advanced_past_the_round(&sender),
                "returning quorum must form one fresh boundary"
            );
        }
    }

    #[test]
    fn fast_proposer_cannot_retire_an_online_quorum_or_advance_round_alone() {
        let bonds: Vec<(i32, i64)> = (0..8).map(|v| (v, 100)).collect();
        let mut state: DagMessageState<i32, i32> = DagMessageState::empty();
        for sender in 0..8 {
            let msg = at(100 + sender, sender, 0, 60, &bonds);
            state.insert_msg_mut(&msg);
        }
        // The first inserted message establishes a one-sender boundary.
        // Complete a real round before testing the next round's outlier.
        for sender in 0..8 {
            state.insert_msg_mut(&at(200 + sender, sender, 1, 61, &bonds));
        }
        // Sender 0 opened this boundary before the other senders reached 61.
        // It must speak again to complete a snapshot of their new messages.
        state.insert_msg_mut(&at(300, 0, 2, 62, &bonds));
        assert_eq!(state.round_parents.len(), 8);
        assert_eq!(state.round_height, BlockHeight::try_from(61).unwrap());

        for height in 63..=106 {
            // Explicit repeated deploys may let a single creator keep climbing.
            // Other senders have still produced their height-60 message.
            let msg = at(1000 + height as i32, 0, height - 60, height, &bonds);
            state.insert_msg_mut(&msg);
        }
        assert_eq!(state.round_height, BlockHeight::try_from(61).unwrap());
        assert_eq!(state.round_parents.len(), 8);

        // Six peers catch up; the seventh is partitioned. The real quorum
        // (7/8, not a minority) moves the supported clock and retires the
        // partitioned peer without waiting for everyone to reach height 106.
        for sender in 1..=6 {
            state.insert_msg_mut(&at(1200 + sender, sender, 2, 68, &bonds));
        }
        assert_eq!(state.round_height, BlockHeight::try_from(68).unwrap());
        assert_eq!(state.round_parents.len(), 8);
        let heights = super::super::liveness::latest_heights(
            state.latest_msgs.values().map(|m| (m.sender, m.height)),
        );
        let bonded = state.latest_msgs.values().next().unwrap().bonds_map.clone();
        let supported = super::super::liveness::quorum_supported_height(&bonded, &heights);
        let live = super::super::liveness::live_weight_set(
            &bonded,
            &heights,
            supported,
            super::super::liveness::LIVENESS_WINDOW,
        );
        assert_eq!(supported, BlockHeight::try_from(68).unwrap());
        assert_eq!(live.len(), 7);
        assert!(!live.contains_key(&7));
    }

    /// **The round is what makes the per-deploy cost linear in the validator count** (#149).
    ///
    /// The issue asks for a node-local rule that stops a validator attesting when its own latest message
    /// already covers the unfinalised set. That rule exists, in the stronger form one-block-per-validator
    /// -per-round ([`Self::has_advanced_past_the_round`], consulted by the proposer at
    /// `casper/src/blocks/proposer/proposer.rs:531`), and this test pins the part that survives it: the
    /// round closes **only when every bonded sender has a message above the boundary**, so a deploy
    /// cannot be finalised without all N of them speaking — once per round, for as many rounds as
    /// finality takes. No node-local rule removes that N, because the sender with nothing to say is
    /// exactly the one the round is waiting for; that is why #149's step 2 would be a no-op and its
    /// step 3 is the consensus change.
    #[test]
    fn a_round_closes_only_when_every_bonded_sender_has_spoken() {
        let bonds = [(0, 100), (1, 100), (2, 100)];
        let state: DagMessageState<i32, i32> = DagMessageState::empty();

        // Genesis: sender 0 alone forms the first boundary, which is the set the next round builds on.
        // The veto reads *since* the boundary, so the message that **is** the boundary does not count as
        // having passed it — each validator speaks once per round, and this is the licensing half.
        let s1 = state.insert_msg(&at(0, 0, 0, 0, &bonds));
        assert_eq!(s1.round_parents.len(), 1);
        assert!(
            !s1.has_advanced_past_the_round(&0),
            "the message that set the boundary has not passed it, so sender 0 may still speak"
        );

        // Two of the three speak. The round does **not** close: sender 0 is at the boundary and has
        // nothing above it yet, so the snapshot — a set, not a stack — is still incomplete.
        let s2 = s1.insert_msg(&at(1, 1, 0, 1, &bonds));
        let s3 = s2.insert_msg(&at(2, 2, 0, 1, &bonds));
        assert_eq!(
            s3.round_height,
            BlockHeight::zero(),
            "the round closed with one of three bonded senders still at the boundary"
        );
        assert_eq!(s3.round_parents.len(), 1);

        // The third speaks: the round closes and the new snapshot holds all three.
        let s4 = s3.insert_msg(&at(3, 0, 1, 1, &bonds));
        assert_eq!(s4.round_height, BlockHeight::try_from(1).unwrap());
        assert_eq!(s4.round_parents.len(), 3);
        // The new boundary resets the veto for everyone: the next round is theirs to speak in.
        for v in [0, 1, 2] {
            assert!(
                !s4.has_advanced_past_the_round(&v),
                "sender {v} is clear to speak in the round the boundary just opened"
            );
        }
    }

    /// The falsifier twin of the test above: the wait is for **bonded** senders, and for nothing else.
    ///
    /// Same three messages, same order — only the bond map differs, dropping sender 0. The round now
    /// closes at the second insertion. Without this the test above would pass just as well for a rule
    /// that waits for any sender, or for none, and the N it attributes to the bond set would be
    /// unearned.
    #[test]
    fn and_it_closes_at_once_when_the_quiet_sender_is_not_bonded() {
        let bonds = [(1, 100), (2, 100)];
        let state: DagMessageState<i32, i32> = DagMessageState::empty();

        let s1 = state.insert_msg(&at(0, 0, 0, 0, &bonds));
        let s2 = s1.insert_msg(&at(1, 1, 0, 1, &bonds));
        assert_eq!(
            s2.round_height,
            BlockHeight::zero(),
            "one bonded sender has spoken and the other has not"
        );
        let s3 = s2.insert_msg(&at(2, 2, 0, 1, &bonds));
        assert_eq!(
            s3.round_height,
            BlockHeight::try_from(1).unwrap(),
            "both bonded senders are above the boundary — the unbonded one is not waited for"
        );
        // The *wait* is over the bonded set; the snapshot is every sender's latest message, the unbonded
        // one included. The two are different sets, and conflating them is how this assertion was wrong
        // on the first run: 3, not 2.
        assert_eq!(s3.round_parents.len(), 3);
    }
}
