//! The in-memory DAG view.
//!
//! Mirrors `block-storage/src/main/scala/coop/rchain/blockstorage/dag/DagRepresentation.scala`.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound::{Excluded, Unbounded};
use std::sync::Arc;

use rchain_models::block_hash::BlockHash;
use rchain_models::validator::Validator;
use rchain_shared::base16;
use rchain_shared::refined::{BlockHeight, NonNegI64};

use crate::errors::StorageError;

use super::finalizer::Message;
use super::message_state::DagMessageState;

/// What a rewind to this node's own last finalized block would cost.
///
/// Produced by [`DagRepresentation::rewind_plan`]. It was reported and not acted on while the per-node
/// reset shipped dark (C249's R3); since #320 it is also the **input to the reset**, and its three
/// shapes decide whether the node can act at all — see `multi_parent_casper::reset_decision`. The
/// middle shape (`anchor` without a height) is why the two arms were never merged into one answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RewindPlan {
    /// The block a reset would replay from — the highest block in the latest fringe, or `None` when
    /// this view has no fringe at all.
    pub anchor: Option<BlockHash>,
    /// Where that block sits, if there is one.
    pub anchor_height: Option<BlockHeight>,
    /// How many blocks this view holds above the anchor (or above genesis when there is none).
    /// Counted from the height map, so validation-failed blocks are not in it — a floor, not a count.
    pub blocks_above: usize,
}

/// The in-memory state of the DAG — an index of the block metadata store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DagRepresentation {
    /// The DAG index — the *same* allocations `BlockMetadataStore` holds (AUDIT C56's owed
    /// paragraph). Keyed by `Arc` so the store's accessors and this view are one index rather than
    /// two copies of it, and an insert updates it by moving three pointers.
    pub dag_set: Arc<BTreeSet<BlockHash>>,
    pub child_map: Arc<BTreeMap<BlockHash, BTreeSet<BlockHash>>>,
    pub height_map: Arc<BTreeMap<BlockHeight, BTreeSet<BlockHash>>>,
    pub dag_message_state: DagMessageState<BlockHash, Validator>,
    // **The `fringe_states` map retired with the `fringe-data` store** (Law 66/68; C250's residue,
    // C270). It was a `BTreeMap<fringe_hash, FringeData>` — one derived value per fringe key, joined
    // with a `min` tie-break when two writers disagreed — and the ambiguity Law 66 describes was
    // *created by it*: one key, many bases. The claim it cached is a per-block fact now
    // (`BlockMetadata.fringe_state_hash`, `BlockMetadata.fringe`, the block's `rejected_deploys`, and
    // `BlockMetadata.member_of_fringe`), so the readers derive from `dag_set`/`child_map` and a
    // metadata lookup instead, and there is no second index here to drift from the store.
}

impl DagRepresentation {
    pub fn latest_fringe(&self) -> BTreeSet<Message<BlockHash, Validator>> {
        self.dag_message_state.latest_fringe()
    }

    /// The finalized blocks are the seen-closure of the latest fringe.
    pub fn finalized_blocks_set(&self) -> BTreeSet<BlockHash> {
        self.latest_fringe()
            .iter()
            .flat_map(|m| m.seen.iter().copied())
            .collect()
    }

    pub fn latest_block_number(&self) -> i64 {
        self.height_map
            .keys()
            .last()
            .map(|h| i64::from(*h + NonNegI64::one()))
            .unwrap_or(0)
    }

    pub fn last_finalized_block_hash(&self) -> Option<BlockHash> {
        self.latest_fringe()
            .iter()
            .map(|m| (m.height, m.id))
            .max()
            .map(|(_, id)| id)
    }

    /// The last finalized block hash, or an error if no fringe is available (port of
    /// `DagRepresentationSyntax.lastFinalizedBlockUnsafe`).
    pub fn last_finalized_block_unsafe(&self) -> Result<BlockHash, String> {
        self.last_finalized_block_hash()
            .ok_or_else(|| "Finalized fringe is not available.".to_string())
    }

    pub fn contains(&self, block_hash: &BlockHash) -> bool {
        self.dag_set.contains(block_hash)
    }

    /// Which height this view files `hash` under, or `None` if it does not hold it.
    ///
    /// A linear scan of the height map, which is a `BTreeMap` of *sets*: the mapping from a block to
    /// its height is not stored in that direction. It is used by the rewind plan, which runs at most
    /// once per refused block, so the scan is deliberate rather than accidental.
    pub fn block_height_of(&self, hash: &BlockHash) -> Option<BlockHeight> {
        self.height_map
            .iter()
            .find(|(_, set)| set.contains(hash))
            .map(|(height, _)| *height)
    }

    /// **What a per-node reset would cost, as this view sees it** (C249's node-side half, #287).
    ///
    /// The anchor is the tree's existing definition of the last finalized block — the highest block in
    /// the latest fringe — because a reset has to replay from a state the node is sure of, and this is
    /// the only such state the running node names. A view with no fringe yet has **no anchor**, which
    /// is a different situation from "nothing to drop": a node that early in a chain has nothing to
    /// rewind *to*.
    ///
    /// `blocks_above` is counted from the height map, so it leaves out validation-failed blocks (the
    /// height map does not count them). A caller that has to be exact should read that as a floor:
    /// `BlockMetadataStore::drop_above` works from the store and takes those blocks too.
    pub fn rewind_plan(&self) -> RewindPlan {
        let anchor = self.last_finalized_block_hash();
        let anchor_height = anchor.and_then(|hash| self.block_height_of(&hash));
        // With no anchor the floor is genesis: everything above it is what a replay from scratch would
        // have to redo, which is the honest reading of "we cannot name a safe state yet".
        let floor = anchor_height.unwrap_or_else(BlockHeight::zero);
        let blocks_above: usize = self
            .height_map
            .range((Excluded(floor), Unbounded))
            .map(|(_, set)| set.len())
            .sum();
        RewindPlan {
            anchor,
            anchor_height,
            blocks_above,
        }
    }

    pub fn children(&self, block_hash: &BlockHash) -> Option<&BTreeSet<BlockHash>> {
        self.child_map.get(block_hash)
    }

    pub fn is_finalized(&self, block_hash: &BlockHash) -> bool {
        self.finalized_blocks_set().contains(block_hash)
    }

    /// Blocks grouped by height in the requested range (or `None` for an invalid range).
    pub fn topo_sort(
        &self,
        start_block_number: i64,
        maybe_end_block_number: Option<i64>,
    ) -> Option<Vec<Vec<BlockHash>>> {
        let max_number = self.latest_block_number();
        let start_number = 0.max(start_block_number);
        let end_number = maybe_end_block_number
            .map(|e| e.min(max_number))
            .unwrap_or(max_number);
        let valid_range = start_number >= 0 && start_number <= end_number;
        if valid_range {
            Some(
                self.height_map
                    .iter()
                    .filter(|(h, _)| i64::from(**h) >= start_number && i64::from(**h) <= end_number)
                    .map(|(_, v)| v.iter().copied().collect())
                    .collect(),
            )
        } else {
            None
        }
    }

    /// Blocks grouped by height in the requested range, or an error for an invalid range (port of
    /// `DagRepresentationSyntax.topoSortUnsafe`).
    pub fn topo_sort_unsafe(
        &self,
        start_block_number: i64,
        maybe_end_block_number: Option<i64>,
    ) -> Result<Vec<Vec<BlockHash>>, StorageError> {
        self.topo_sort(start_block_number, maybe_end_block_number)
            .ok_or(StorageError::TopoSortFragmentParameterError {
                start_block_number,
                end_block_number: i64::MAX,
            })
    }

    /// The number of messages in the DAG's message state — one per block (`msg_map`).
    pub fn message_count(&self) -> usize {
        self.dag_message_state.msg_map.len()
    }

    /// Σ over messages of `seen.len()`: the entries of H6's Θ(N²) residency.
    ///
    /// Θ(N) to *sum* — each `len()` is O(1), so this is a pass over the map, not over the sets —
    /// which is why it can be published per block rather than cached and left to drift.
    pub fn seen_entries(&self) -> usize {
        self.dag_message_state
            .msg_map
            .values()
            .map(|m| m.seen.len())
            .sum()
    }

    /// The DAG index's entries: known blocks, parent→child links, and height groups.
    pub fn index_entries(&self) -> usize {
        self.dag_set.len() + self.child_map.len() + self.height_map.len()
    }

    /// The logical bytes the message state holds, as the pass's own accounting defines it: for each
    /// message the struct itself, the three sets of 32-byte hashes it carries (`seen`, `parents`,
    /// `fringe`), and its bond weights.
    ///
    /// This is the number that makes H6's residency measurable rather than argued: Σ|seen| × 32 B is
    /// the Θ(N²) floor, and every other term is a copy of it. It is a *value* accounting, so nothing
    /// about how the maps are keyed or shared (AUDIT C56's owed paragraph: the index's `Arc`s) can
    /// move it — `logical_bytes_is_a_value_not_a_representation` pins that.
    pub fn logical_bytes(&self) -> usize {
        self.dag_message_state
            .msg_map
            .values()
            .map(|m| {
                std::mem::size_of::<Message<BlockHash, Validator>>()
                    + (m.seen.len() + m.parents.len() + m.fringe.len()) * 32
                    + m.bonds_map.len() * 73
            })
            .sum()
    }

    /// Find a block hash by (possibly truncated) hex prefix.
    pub fn find(&self, truncated_hash: &str) -> Option<BlockHash> {
        // Validate-on-ingress: reject non-hex input (`decode` returns `None`) rather than silently
        // dropping invalid chars via `unsafe_decode`.
        if truncated_hash.len().is_multiple_of(2) {
            let bytes = base16::decode(truncated_hash)?;
            self.dag_set.iter().find(|h| h.starts_with(&bytes)).copied()
        } else {
            let bytes = base16::decode(&truncated_hash[..truncated_hash.len() - 1])?;
            self.dag_set
                .iter()
                .filter(|h| h.starts_with(&bytes))
                .find(|h| h.to_hex().starts_with(truncated_hash))
                .copied()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(byte: u8) -> BlockHash {
        BlockHash::new([byte; 32])
    }

    /// A representation with a small chain 0 → 1 → 2 at heights 0..=2.
    fn chain() -> DagRepresentation {
        let (b0, b1, b2) = (hash(0), hash(1), hash(2));
        DagRepresentation {
            dag_set: Arc::new([b0, b1, b2].into_iter().collect()),
            child_map: Arc::new(
                [
                    (b0, [b1].into_iter().collect()),
                    (b1, [b2].into_iter().collect()),
                    (b2, BTreeSet::new()),
                ]
                .into_iter()
                .collect(),
            ),
            height_map: Arc::new(
                [
                    (
                        BlockHeight::try_from(0).unwrap(),
                        [b0].into_iter().collect(),
                    ),
                    (
                        BlockHeight::try_from(1).unwrap(),
                        [b1].into_iter().collect(),
                    ),
                    (
                        BlockHeight::try_from(2).unwrap(),
                        [b2].into_iter().collect(),
                    ),
                ]
                .into_iter()
                .collect(),
            ),
            dag_message_state: DagMessageState::empty(),
        }
    }

    /// The same three blocks, with a message state so the view has a fringe: one sender, a chain, and
    /// each message's `seen` is its whole ancestry.
    ///
    /// **`fringe` is what carries the anchor, not `seen`.** `latest_fringe` takes the highest message
    /// in the map and returns *its* `fringe` set — so a message with an empty fringe (which is what the
    /// first draft of this fixture had) leaves the view with no last finalized block at all, which is
    /// the sibling test's case and not this one.
    fn chain_with_a_tip() -> DagRepresentation {
        let mut dag = chain();
        let mut state: DagMessageState<BlockHash, Validator> = DagMessageState::empty();
        for (seq, id, seen) in [
            (0i64, hash(0), vec![hash(0)]),
            (1, hash(1), vec![hash(0), hash(1)]),
            (2, hash(2), vec![hash(0), hash(1), hash(2)]),
        ] {
            state.insert_msg_mut(&Message {
                id,
                height: BlockHeight::try_from(seq).unwrap(),
                sender: Validator::new([0u8; 65]),
                sender_seq: rchain_shared::refined::SeqNum::try_from(seq).unwrap(),
                bonds_map: BTreeMap::new(),
                parents: BTreeSet::new(),
                fringe: [id].into_iter().collect(),
                seen: Arc::new(seen.into_iter().collect()),
            });
        }
        dag.dag_message_state = state;
        dag
    }

    /// An empty DAG has no last finalized block, and the `_unsafe` variant says so by name rather
    /// than panicking — the arm a caller reads when the fringe is not available yet.
    #[test]
    fn an_empty_dag_has_no_last_finalized_block() {
        let dag = DagRepresentation {
            dag_set: Arc::new(BTreeSet::new()),
            child_map: Arc::new(BTreeMap::new()),
            height_map: Arc::new(BTreeMap::new()),
            dag_message_state: DagMessageState::empty(),
        };
        assert_eq!(dag.last_finalized_block_hash(), None);
        assert_eq!(
            dag.last_finalized_block_unsafe().expect_err("no fringe"),
            "Finalized fringe is not available."
        );
        assert_eq!(dag.latest_block_number(), 0);
        assert!(dag.latest_fringe().is_empty());
        assert!(dag.finalized_blocks_set().is_empty());
    }

    /// **Law 125's Rust witness.** The finalized read is the seen closure of the latest fringe, not
    /// the fringe's own ids: a block below the tip is finalized because the tip has seen it, while a
    /// block outside the closure is not. The fixture's tip (height 2) carries `fringe = {hash(2)}`
    /// and `seen = {hash(0), hash(1), hash(2)}`.
    #[test]
    fn the_finalized_read_is_the_seen_closure_not_the_fringe_ids() {
        let dag = chain_with_a_tip();
        assert_eq!(dag.last_finalized_block_hash(), Some(hash(2)));
        assert!(
            dag.is_finalized(&hash(0)),
            "hash(0) is below the fringe but the tip has seen it"
        );
        assert!(
            dag.is_finalized(&hash(1)),
            "hash(1) is in the tip's seen set"
        );
        assert!(dag.is_finalized(&hash(2)), "hash(2) is the fringe member");
        assert!(
            !dag.is_finalized(&hash(9)),
            "a block outside the seen closure is not finalized"
        );
    }

    /// **A rewind plan with no anchor counts everything above genesis, and says there is no anchor.**
    ///
    /// This is the state a node is in before its first finalization: it holds blocks, it has no block
    /// it is *sure* of, and a reset would have to replay from genesis. "No anchor" and "nothing to
    /// drop" are different answers and the plan keeps them apart — the first is loud, the second is
    /// silent, and conflating them is how a node with nothing to rewind to looks like a node with
    /// nothing to do.
    #[test]
    fn a_view_without_a_fringe_has_no_anchor_and_counts_from_genesis() {
        let plan = chain().rewind_plan();
        assert_eq!(plan.anchor, None, "an empty message state has no fringe");
        assert_eq!(plan.anchor_height, None);
        assert_eq!(
            plan.blocks_above, 2,
            "the two blocks above genesis: what a replay from scratch would redo"
        );
    }

    /// **With a fringe, the anchor is the fringe's highest block and the plan counts above it.**
    ///
    /// The tip is finalized here, so the plan is the silent answer — zero blocks above the anchor —
    /// which is the control for the test above: same three blocks, different fringe, different plan.
    #[test]
    fn a_finalized_tip_leaves_nothing_above_the_anchor() {
        let plan = chain_with_a_tip().rewind_plan();
        assert_eq!(
            plan.anchor,
            Some(hash(2)),
            "the highest block in the fringe"
        );
        assert_eq!(plan.anchor_height, Some(BlockHeight::try_from(2).unwrap()));
        assert_eq!(plan.blocks_above, 0);
    }

    /// `block_height_of` answers in one direction only: the height map is a map of *sets*, so a block
    /// not in the view must come back `None` rather than colliding with the block filed at height 0.
    #[test]
    fn block_height_of_resolves_and_refuses() {
        let dag = chain();
        assert_eq!(dag.block_height_of(&hash(0)), Some(BlockHeight::zero()));
        assert_eq!(
            dag.block_height_of(&hash(1)),
            Some(BlockHeight::try_from(1).unwrap())
        );
        assert_eq!(dag.block_height_of(&hash(9)), None, "not in this view");
    }

    /// The height index drives `latest_block_number` (one past the highest height) and the range
    /// query: the end bound is clamped to the DAG's height, so a caller asking beyond the tip gets
    /// the tip's group rather than an error.
    #[test]
    fn the_height_index_drives_the_range_query() {
        let dag = chain();
        assert_eq!(dag.latest_block_number(), 3, "one past height 2");

        let all = dag.topo_sort(0, None).expect("a valid range");
        assert_eq!(all, vec![vec![hash(0)], vec![hash(1)], vec![hash(2)]]);

        // A start beyond the tip is an *invalid* range (`start > end`), not an empty answer.
        assert_eq!(dag.topo_sort(9, None), None);
        // A negative start is clamped to zero, so it is valid.
        assert_eq!(dag.topo_sort(-5, None).expect("clamped").len(), 3);
        // An end beyond the tip is clamped.
        assert_eq!(dag.topo_sort(0, Some(99)).expect("clamped").len(), 3);
        // A sub-range.
        assert_eq!(
            dag.topo_sort(1, Some(1)).expect("valid"),
            vec![vec![hash(1)]]
        );
        let err = dag
            .topo_sort_unsafe(9, Some(1))
            .expect_err("an invalid range is an error");
        assert!(format!("{err}").contains("topo-sort"), "{err}");
    }

    /// `find` resolves a hex prefix, and **rejects non-hex input rather than dropping the bad
    /// characters** (the validate-on-ingress rule): an odd-length prefix still resolves, but a
    /// malformed one finds nothing instead of matching a different block.
    #[test]
    fn find_resolves_a_hex_prefix_and_rejects_junk() {
        let dag = chain();
        let full = hash(2).to_hex();

        assert_eq!(dag.find(&full), Some(hash(2)));
        assert_eq!(dag.find(&full[..8]), Some(hash(2)), "an even-length prefix");
        // Odd length: the last nibble is compared as text.
        assert_eq!(dag.find(&full[..9]), Some(hash(2)));
        let odd_junk = format!("{}z", &full[..8]);
        assert_eq!(dag.find(&odd_junk), None, "not hex");
        assert_eq!(
            dag.find(""),
            Some(hash(0)),
            "the empty prefix matches the first block"
        );
        assert_eq!(dag.find("ffffff"), None, "no block starts with that");
    }

    /// `contains`/`children`/`is_finalized` are the DAG queries the block processor uses.
    #[test]
    fn membership_and_children_queries() {
        let dag = chain();
        assert!(dag.contains(&hash(1)));
        assert!(!dag.contains(&hash(9)));
        assert_eq!(
            dag.children(&hash(0)),
            Some(&[hash(1)].into_iter().collect())
        );
        assert_eq!(
            dag.children(&hash(9)),
            None,
            "an unknown block has no children"
        );
        // With no fringe state, nothing is finalized.
        assert!(!dag.is_finalized(&hash(0)));
    }
}
