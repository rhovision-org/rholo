//! Block metadata store — in-memory DAG state.
//!
//! Mirrors the pure logic of
//! `block-storage/src/main/scala/coop/rchain/blockstorage/dag/BlockMetadataStore.scala`. The
//! `F[_]`/`Ref`/store wrapper is deferred to the async layer; these pure functions implement
//! `addBlockToDagState` / `validateDagState` / `recreateInMemoryState` (Law 18: height-map
//! contiguity).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use rchain_models::block_hash::BlockHash;
use rchain_models::block_metadata::BlockMetadata;
use rchain_shared::refined::{BlockHeight, NonNegI64};

/// The in-memory DAG state.
///
/// The three maps are `Arc`-shared with the DAG representation that reads them (AUDIT C56's owed
/// paragraph): the store keeps one index and the representation points at the same allocations,
/// instead of every insert cloning the whole index into a second copy. `Arc::make_mut` keeps the
/// pure functions pure — a state that someone else still holds is copied before it is extended —
/// while an unshared state is extended in place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DagState {
    pub dag_set: Arc<BTreeSet<BlockHash>>,
    pub child_map: Arc<BTreeMap<BlockHash, BTreeSet<BlockHash>>>,
    pub height_map: Arc<BTreeMap<BlockHeight, BTreeSet<BlockHash>>>,
}

impl DagState {
    pub fn empty() -> Self {
        Self {
            dag_set: Arc::new(BTreeSet::new()),
            child_map: Arc::new(BTreeMap::new()),
            height_map: Arc::new(BTreeMap::new()),
        }
    }
}

/// A projection of `BlockMetadata` used to rebuild in-memory state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockInfo {
    pub hash: BlockHash,
    pub parents: BTreeSet<BlockHash>,
    pub block_num: BlockHeight,
    pub validation_failed: bool,
}

pub fn block_metadata_to_info(meta: &BlockMetadata) -> BlockInfo {
    BlockInfo {
        hash: meta.block_hash,
        parents: meta.justifications.clone(),
        block_num: meta.block_num,
        validation_failed: meta.validation_failed,
    }
}

/// Project stored metadata into the DAG index without cloning its justification set.
///
/// Moves the justification set from an owned record rather than cloning it.
/// The caller still loads all metadata through to_map() before projection;
/// this is not a bounded-memory streaming reconstruction.
pub fn block_metadata_into_info(meta: BlockMetadata) -> BlockInfo {
    BlockInfo {
        hash: meta.block_hash,
        parents: meta.justifications,
        block_num: meta.block_num,
        validation_failed: meta.validation_failed,
    }
}

pub fn add_block_to_dag_state(block: &BlockInfo, state: &DagState) -> DagState {
    let mut next = state.clone();
    add_block_to_dag_state_mut(block, &mut next);
    next
}

/// Extend a DAG state with one block, in place — the live path's form of
/// [`add_block_to_dag_state`].
///
/// The maps are `Arc`-shared, so this copies a map only if some other holder (a DAG representation
/// snapshot a reader took) still points at it; the storage's own insert is serialized, and the
/// representation is refreshed from the store's new `Arc`s at the end of that insert.
pub fn add_block_to_dag_state_mut(block: &BlockInfo, state: &mut DagState) {
    Arc::make_mut(&mut state.dag_set).insert(block.hash);

    let child_map = Arc::make_mut(&mut state.child_map);
    for parent in &block.parents {
        child_map.entry(*parent).or_default().insert(block.hash);
    }
    child_map.entry(block.hash).or_default();

    if !block.validation_failed {
        Arc::make_mut(&mut state.height_map)
            .entry(block.block_num)
            .or_default()
            .insert(block.hash);
    }
}

/// Validate that the height-map keys form a contiguous range (Law 18).
pub fn validate_dag_state(state: &DagState) -> Result<(), String> {
    check_height_extent(height_extent(state))
}

/// **The same check as [`validate_dag_state`], for the state `block` would produce** — so a caller can
/// refuse an add *before* it has written anything or mutated anything.
///
/// This exists because the mutating form cannot be used as a gate: `add_block_to_dag_state_mut`
/// extends the live state in place, so validating *after* it has run returns `Err` with the state
/// already extended. That is not hypothetical — a block whose height leaves a gap in the height map
/// (the reachable case is a failed block, which is not counted into `height_map`, with a later block
/// above it) made the index report a block the store never received, permanently, because the store
/// write sits after the `?`. AUDIT C172.
///
/// The equivalence with the mutating form is **pinned rather than trusted**
/// (`validate_after_agrees_with_validating_the_extended_state`), for the same reason
/// `recreate_in_memory_state`'s in-place rebuild is: a gate that disagrees with the check it replaces
/// is a worse defect than the one it fixes.
pub fn validate_dag_state_after(state: &DagState, block: &BlockInfo) -> Result<(), String> {
    check_height_extent(height_extent_after(state, block))
}

/// `(min, max_exclusive, len)` — the three numbers the contiguity check compares.
fn height_extent(state: &DagState) -> (BlockHeight, BlockHeight, i64) {
    let m = &state.height_map;
    let (min, max) = match (m.keys().next(), m.keys().next_back()) {
        (Some(first), Some(last)) => (*first, *last + NonNegI64::one()),
        _ => (BlockHeight::zero(), BlockHeight::zero()),
    };
    (min, max, i64::try_from(m.len()).unwrap_or(i64::MAX))
}

/// The extent the state would have with `block` in it.
///
/// Two cases are the ones to be careful with, and both are `height_map`'s **keys** rather than its
/// blocks:
///
/// - **An empty map is `(0, 0, 0)` by the convention above**, and inserting the *first* key resets the
///   minimum to that key rather than keeping the conventional zero — a first block at height 5 is a
///   valid single-key state, and treating `0` as the minimum would refuse it.
/// - **A second block at a height the map already has does not add a key**, so the length must not
///   grow: the extent is over key positions, not over blocks. (This one was wrong until the
///   equivalence test below caught it, on the state `[(0, false)]` with a candidate at height 0 — a
///   case the mutating form accepts and the arithmetic refused.)
fn height_extent_after(state: &DagState, block: &BlockInfo) -> (BlockHeight, BlockHeight, i64) {
    let m = &state.height_map;
    // A validation-failed block is recorded in `dag_set` and `child_map` but not at its height.
    if block.validation_failed {
        return height_extent(state);
    }
    let len = i64::try_from(m.len()).unwrap_or(i64::MAX);
    let adds_a_key = !m.contains_key(&block.block_num);
    let len = if adds_a_key { len + 1 } else { len };
    match (m.keys().next(), m.keys().next_back()) {
        (Some(first), Some(last)) => {
            let min = (*first).min(block.block_num);
            let max = (*last + NonNegI64::one()).max(block.block_num + NonNegI64::one());
            (min, max, len)
        }
        _ => (block.block_num, block.block_num + NonNegI64::one(), len),
    }
}

fn check_height_extent((min, max, len): (BlockHeight, BlockHeight, i64)) -> Result<(), String> {
    if max - min != len {
        return Err("DAG store height map has numbers not in sequence.".to_string());
    }
    Ok(())
}

/// Rebuild in-memory state from a block-info map, then validate.
///
/// **In place, and that is the whole of its cost fix.** It used to fold through
/// [`add_block_to_dag_state`], which is `state.clone()` and then the mutating form — so each of N
/// blocks deep-cloned all three maps while the previous binding was still alive, and rebuilding a
/// chain was O(N²) in allocations on the **start-up** path, where N is every block ever accepted.
/// [`add_block_to_dag_state_mut`] is what the live path uses — the module doc above says why an
/// unshared state is extended in place — and this is an unshared state by construction.
///
/// The equivalence is pinned rather than trusted: `the_in_place_rebuild_equals_the_cloning_one`
/// asserts the two forms produce the same state, which is what made the change safe to make.
pub fn recreate_in_memory_state(
    blocks: &BTreeMap<BlockHash, BlockInfo>,
) -> Result<DagState, String> {
    let mut state = DagState::empty();
    for block in blocks.values() {
        add_block_to_dag_state_mut(block, &mut state);
    }
    validate_dag_state(&state)?;
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(byte: u8) -> BlockHash {
        let mut bytes = [0u8; 32];
        bytes[0] = byte;
        BlockHash::new(bytes)
    }

    fn info(hash: BlockHash, parents: &[BlockHash], block_num: i64) -> BlockInfo {
        BlockInfo {
            hash,
            parents: parents.iter().copied().collect(),
            block_num: BlockHeight::try_from(block_num).unwrap(),
            validation_failed: false,
        }
    }

    #[test]
    fn law18_contiguous_height_map_is_valid() {
        let mut blocks = BTreeMap::new();
        let h0 = info(hash(0), &[], 0);
        let h1 = info(hash(1), &[hash(0)], 1);
        let h2 = info(hash(2), &[hash(1)], 2);
        blocks.insert(hash(0), h0);
        blocks.insert(hash(1), h1);
        blocks.insert(hash(2), h2);
        recreate_in_memory_state(&blocks).unwrap(); // no error
    }

    #[test]
    fn law18_height_map_with_holes_errors() {
        let mut blocks = BTreeMap::new();
        blocks.insert(hash(0), info(hash(0), &[], 0));
        // block_num 2 with no block_num 1 -> hole.
        blocks.insert(hash(2), info(hash(2), &[hash(0)], 2));
        assert_eq!(
            recreate_in_memory_state(&blocks).unwrap_err(),
            "DAG store height map has numbers not in sequence."
        );
    }

    /// **The gate agrees with the check it replaces**, over the states and blocks that matter: empty,
    /// a single high block, a contiguous chain, a gap, a *failed* block (which is not counted into the
    /// height map), and a candidate that is itself failed.
    ///
    /// `validate_dag_state_after` exists so a caller can refuse an add **before** the in-place
    /// mutation; if it disagreed with `validate_dag_state` on the extended state, it would be a worse
    /// defect than the one it fixes, and nothing else would catch it — the two are compared here
    /// rather than argued about (AUDIT C172).
    #[test]
    fn validate_after_agrees_with_validating_the_extended_state() {
        let states: Vec<Vec<(i64, bool)>> = vec![
            vec![],
            vec![(0, false)],
            vec![(0, false), (1, false)],
            vec![(0, false), (1, true), (2, false)],
            vec![(0, false), (2, false)],
            vec![(5, false)],
            vec![(0, false), (5, false)],
        ];
        let candidates: Vec<(i64, bool)> = vec![
            (0, false),
            (1, false),
            (2, false),
            (3, false),
            (5, false),
            (0, true),
            (3, true),
            (6, true),
        ];
        for state_blocks in &states {
            for (height, failed) in &candidates {
                let mut state = DagState::empty();
                for (i, (h, f)) in state_blocks.iter().enumerate() {
                    let block = BlockInfo {
                        hash: hash(i as u8 + 1),
                        parents: BTreeSet::new(),
                        block_num: BlockHeight::try_from(*h).unwrap(),
                        validation_failed: *f,
                    };
                    add_block_to_dag_state_mut(&block, &mut state);
                }
                let candidate = BlockInfo {
                    hash: hash(200),
                    parents: BTreeSet::new(),
                    block_num: BlockHeight::try_from(*height).unwrap(),
                    validation_failed: *failed,
                };
                assert_eq!(
                    validate_dag_state_after(&state, &candidate).is_ok(),
                    validate_dag_state(&add_block_to_dag_state(&candidate, &state)).is_ok(),
                    "disagreement on state {state_blocks:?} with a candidate at {height} \
                     (failed={failed})"
                );
            }
        }
    }

    #[test]
    fn add_block_to_dag_state_builds_child_map() {
        let parent = hash(0);
        let child = hash(1);
        let state = DagState::empty();
        let s1 = add_block_to_dag_state(&info(parent, &[], 0), &state);
        let s2 = add_block_to_dag_state(&info(child, &[parent], 1), &s1);
        assert_eq!(s2.child_map[&parent], [child].into_iter().collect());
        assert!(s2.child_map[&child].is_empty());
        assert_eq!(*s2.dag_set, [parent, child].into_iter().collect());
    }

    /// **The in-place rebuild is the same state as the cloning one, which is what made the change
    /// safe to make.** `recreate_in_memory_state` folded through `add_block_to_dag_state` — `clone`
    /// then mutate — so an N-block rebuild deep-cloned the whole `DagState` N times on the start-up
    /// path; it now calls the mutating form directly. The two must agree exactly, over a chain with a
    /// **fork**, because the maps built here are the ones the start-up path hands to the finalizer,
    /// and a difference would surface as a finality disagreement rather than as a compile error.
    ///
    /// The fork is the load-bearing part of the fixture: a linear chain exercises `child_map` and
    /// `height_map` one entry at a time, while two children under one parent is where a per-block
    /// rebuild could put a set in the wrong place and still produce a state that validates.
    #[test]
    fn the_in_place_rebuild_equals_the_cloning_one() {
        let mut blocks = BTreeMap::new();
        let (h0, h1, h2, h2b, h3) = (hash(0), hash(1), hash(2), hash(3), hash(4));
        blocks.insert(h0, info(h0, &[], 0));
        blocks.insert(h1, info(h1, &[h0], 1));
        blocks.insert(h2, info(h2, &[h1], 2));
        blocks.insert(h2b, info(h2b, &[h1], 2));
        blocks.insert(h3, info(h3, &[h2, h2b], 3));

        let in_place = recreate_in_memory_state(&blocks).expect("the fork validates");

        // The reference: the cloning fold this function used to be.
        let mut cloning = DagState::empty();
        for block in blocks.values() {
            cloning = add_block_to_dag_state(block, &cloning);
        }

        assert_eq!(
            in_place, cloning,
            "the in-place rebuild must equal the cloning one, or the optimisation changed the state"
        );
        assert_eq!(
            in_place.child_map[&h1],
            [h2, h2b].into_iter().collect(),
            "and the fork is what makes the equality above mean something"
        );
    }
}
