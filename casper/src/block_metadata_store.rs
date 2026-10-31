//! Block metadata store — the in-memory DAG index over the persisted metadata store (port of
//! `block-storage/dag/BlockMetadataStore.scala`).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use rchain_block_storage::dag::metadata_store::{
    add_block_to_dag_state_mut, block_metadata_into_info, block_metadata_to_info,
    recreate_in_memory_state, validate_dag_state_after, BlockInfo, DagState,
};
use rchain_models::block_hash::BlockHash;
use rchain_models::block_metadata::BlockMetadata;
use rchain_shared::refined::BlockHeight;
use rchain_shared::typed_store::KeyValueTypedStore;

/// The block metadata store: a persisted `KeyValueTypedStore` plus an in-memory [`DagState`] index
/// rebuilt on startup.
pub struct BlockMetadataStore {
    store: Arc<dyn KeyValueTypedStore<BlockHash, BlockMetadata>>,
    dag_state: tokio::sync::RwLock<DagState>,
}

impl BlockMetadataStore {
    /// Rebuild the in-memory DAG index from the persisted store (port of `BlockMetadataStore.apply`).
    pub async fn create(
        store: Arc<dyn KeyValueTypedStore<BlockHash, BlockMetadata>>,
    ) -> Result<Self, String> {
        // Move justification sets out of owned records instead of cloning them.
        // to_map() still materializes the full decoded history before this pass;
        // only per-record duplication is reduced, not total startup or replay memory.
        let info_map: BTreeMap<BlockHash, BlockInfo> = store
            .to_map()
            .await?
            .into_iter()
            .map(|(hash, meta)| (hash, block_metadata_into_info(meta)))
            .collect();
        let dag_state = recreate_in_memory_state(&info_map)?;
        Ok(BlockMetadataStore {
            store,
            dag_state: tokio::sync::RwLock::new(dag_state),
        })
    }

    /// Insert a block's metadata into both the persisted store and the in-memory index.
    ///
    /// **The store first, then the index, and the transition is checked before either.** The order is
    /// the whole of this shape, and [`validate_dag_state_after`] says what the old one cost: it
    /// extended the live index *before* validating and before writing the store, so a refused add —
    /// a block whose height leaves a gap in the height map, which a validation-failed block makes
    /// reachable — left the index reporting a block the store never received. The two are read by
    /// different callers (`DagRepresentation::contains` reads the index, `dag.lookup` reads the
    /// store), so a disagreement is a block that satisfies `has_all_deps`, is queued for validation,
    /// then fails with `missing justification` and is dropped with nothing to re-queue it. AUDIT
    /// C172, issue #103.
    ///
    /// The check runs twice on purpose. The first is what normally decides, because callers hold
    /// `BlockDagKeyValueStorage`'s insert lock; the second runs under the write guard that performs
    /// the mutation, so the state it validated cannot change between the check and the extension,
    /// and a lost race leaves the store holding a block the index does not — the safe direction,
    /// since `contains` false is exactly what "not in the DAG yet" means to both of its callers.
    pub async fn add(&self, block: BlockMetadata) -> Result<(), String> {
        let info = block_metadata_to_info(&block);
        {
            let state = self.dag_state.read().await;
            validate_dag_state_after(&state, &info)?;
        }
        self.store.put(&[(block.block_hash, block)]).await?;
        let mut state = self.dag_state.write().await;
        validate_dag_state_after(&state, &info)?;
        // In place: the index is `Arc`-shared with the DAG representation, so this copies a map only
        // if a reader still holds the previous one (AUDIT C56's owed paragraph).
        add_block_to_dag_state_mut(&info, &mut state);
        Ok(())
    }

    pub async fn get(&self, hash: &BlockHash) -> Result<Option<BlockMetadata>, String> {
        let vals = self.store.get(&[*hash]).await?;
        Ok(vals.into_iter().next().flatten())
    }

    /// Look up a block's metadata, failing if it is absent (port of `getUnsafe`).
    pub async fn get_unchecked(&self, hash: &BlockHash) -> Result<BlockMetadata, String> {
        self.get(hash)
            .await?
            .ok_or_else(|| format!("BlockMetadataStore is missing key {}", hash.to_hex()))
    }

    pub async fn contains(&self, hash: &BlockHash) -> bool {
        self.dag_state.read().await.dag_set.contains(hash)
    }

    /// The index's own allocation, not a copy of it: the caller shares the map the store updates
    /// (AUDIT C56's owed paragraph — this used to hand out a full clone per insert).
    pub async fn dag_set(&self) -> Arc<BTreeSet<BlockHash>> {
        self.dag_state.read().await.dag_set.clone()
    }

    pub async fn child_map_data(&self) -> Arc<BTreeMap<BlockHash, BTreeSet<BlockHash>>> {
        self.dag_state.read().await.child_map.clone()
    }

    pub async fn height_map(&self) -> Arc<BTreeMap<BlockHeight, BTreeSet<BlockHash>>> {
        self.dag_state.read().await.height_map.clone()
    }

    /// **Drop every block above `height`** — from the persisted store and from the in-memory index —
    /// and return the hashes that went (C249's node-side reset, #287).
    ///
    /// A node that cannot locally heal has exactly one way back today: an operator stops it and deletes
    /// the shard data dir by hand. That path exists as a comment (`casper/src/dag.rs`) and nowhere as
    /// code. This is the node-side primitive that replaces it: rewind the DAG to a height the node can
    /// replay from, and start again from there.
    ///
    /// **The order is the store first, then the rebuild, and the rebuild is a rebuild rather than an
    /// edit.** The store-first order is the one [`Self::add`] uses and for the same reason (AUDIT C172):
    /// a refused or failed delete leaves the index untouched. Building the surviving index from what the
    /// store holds, rather than removing entries from the live one, is what makes the two agree *by
    /// construction* instead of by two implementations of the same rule — and
    /// [`recreate_in_memory_state`] re-runs the height-map contiguity check on the way, so a rewind that
    /// would leave a gap is refused rather than published.
    ///
    /// **`height` is kept, not dropped**: it is the state the node replays from. The caller's anchor and
    /// this primitive's argument have to be the same block, which is why the anchor is persisted
    /// separately rather than re-derived from a DAG the reset is about to shorten.
    ///
    /// The hashes are returned rather than a count because the reset is not the only thing that has to
    /// be rewound: the DAG's *derived* indices (the deploy index, the deployer index) hold block hashes
    /// too, and a caller that only learned "2 blocks went" could not clean them.
    pub async fn drop_above(&self, height: BlockHeight) -> Result<Vec<BlockHash>, String> {
        let blocks = self.store.to_map().await?;
        let doomed: Vec<BlockHash> = blocks
            .iter()
            .filter(|(_, meta)| meta.block_num > height)
            .map(|(hash, _)| *hash)
            .collect();
        if doomed.is_empty() {
            return Ok(doomed);
        }
        // The store reports what it actually removed. The index below is rebuilt from what *survives*,
        // not from what was asked for, so a key that was already gone changes nothing either way.
        let _removed = self.store.delete(&doomed).await?;

        let surviving: BTreeMap<BlockHash, BlockInfo> = blocks
            .iter()
            .filter(|(_, meta)| meta.block_num <= height)
            .map(|(hash, meta)| (*hash, block_metadata_to_info(meta)))
            .collect();
        let rebuilt = recreate_in_memory_state(&surviving)?;
        *self.dag_state.write().await = rebuilt;
        Ok(doomed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_block_storage::dag::codecs::{BlockHashCodec, BlockMetadataCodec};
    use rchain_models::block_metadata::SlashSeverity;
    use rchain_shared::store::{InMemoryKeyValueStore, KeyValueStore};
    use rchain_shared::typed_store::KeyValueTypedStoreCodec;

    type Shared = Arc<tokio::sync::Mutex<Box<dyn KeyValueStore + Send + Sync>>>;

    fn metadata_store() -> Arc<dyn KeyValueTypedStore<BlockHash, BlockMetadata>> {
        metadata_store_over(Box::new(InMemoryKeyValueStore::default()))
    }

    fn metadata_store_over(
        store: Box<dyn KeyValueStore + Send + Sync>,
    ) -> Arc<dyn KeyValueTypedStore<BlockHash, BlockMetadata>> {
        let shared: Shared = Arc::new(tokio::sync::Mutex::new(store));
        Arc::new(KeyValueTypedStoreCodec::new(
            shared,
            Arc::new(BlockHashCodec),
            Arc::new(BlockMetadataCodec),
        ))
    }

    /// A store whose writes always fail: every read still works, so the only thing under test is what
    /// `add` does with the index when its write does not land.
    struct FailingPutStore {
        inner: InMemoryKeyValueStore,
    }

    impl KeyValueStore for FailingPutStore {
        fn get(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Vec<u8>>>, String> {
            self.inner.get(keys)
        }
        fn put(&mut self, _pairs: Vec<(Vec<u8>, Vec<u8>)>) -> Result<(), String> {
            Err("the block store is down".to_string())
        }
        fn delete(&mut self, keys: &[Vec<u8>]) -> Result<usize, String> {
            self.inner.delete(keys)
        }
        fn entries(&self) -> Result<Vec<(Vec<u8>, Vec<u8>)>, String> {
            self.inner.entries()
        }
    }

    fn hash(byte: u8) -> BlockHash {
        let mut bytes = [0u8; 32];
        bytes[0] = byte;
        BlockHash::new(bytes)
    }

    fn meta(hash: BlockHash, parents: &[BlockHash], block_num: i64) -> BlockMetadata {
        BlockMetadata {
            block_hash: hash,
            block_num: rchain_shared::refined::BlockHeight::try_from(block_num).unwrap(),
            sender: rchain_models::validator::Validator::new([0u8; 65]),
            seq_num: 0.try_into().unwrap(),
            justifications: parents.iter().copied().collect(),
            bonds_map: BTreeMap::new(),
            validated: true,
            validation_failed: false,
            slashable: false,
            failure_cause: None,
            slash_severity: SlashSeverity::Unspecified,
            restore_attempts: 0,
            fringe: BTreeSet::new(),
            fringe_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
            member_of_fringe: None,
        }
    }

    #[test]
    fn owned_metadata_projection_matches_the_borrowed_projection() {
        let mut block = meta(hash(2), &[hash(0), hash(1)], 2);
        block.validation_failed = true;
        block.slashable = true;
        let expected = block_metadata_to_info(&block);
        let actual = block_metadata_into_info(block);
        assert_eq!(
            actual, expected,
            "moving fields must preserve every DAG value"
        );
        assert_eq!(actual.parents.len(), 2);
        assert!(actual.validation_failed);
    }

    /// A restored index must preserve height, children, and persisted contents
    /// after taking ownership of the decoded metadata records.
    #[tokio::test]
    async fn restored_dag_projection_is_identical_to_the_live_index() {
        let backing = metadata_store();
        let live = BlockMetadataStore::create(backing.clone()).await.unwrap();
        live.add(meta(hash(0), &[], 0)).await.unwrap();
        live.add(meta(hash(1), &[hash(0)], 1)).await.unwrap();
        live.add(meta(hash(2), &[hash(0), hash(1)], 2))
            .await
            .unwrap();

        let expected_set = live.dag_set().await;
        let expected_children = live.child_map_data().await;
        let expected_heights = live.height_map().await;
        let restored = BlockMetadataStore::create(backing).await.unwrap();
        assert_eq!(*restored.dag_set().await, *expected_set);
        assert_eq!(*restored.child_map_data().await, *expected_children);
        assert_eq!(*restored.height_map().await, *expected_heights);
        assert!(restored.get_unchecked(&hash(2)).await.is_ok());
    }

    #[tokio::test]
    async fn add_and_lookup_round_trip() {
        let store = BlockMetadataStore::create(metadata_store()).await.unwrap();
        let genesis = meta(hash(0), &[], 0);
        store.add(genesis.clone()).await.unwrap();

        assert!(store.contains(&hash(0)).await);
        assert_eq!(store.get(&hash(0)).await.unwrap(), Some(genesis.clone()));
        assert_eq!(store.get(&hash(1)).await.unwrap(), None);

        // get_unchecked panics (errors) on a missing key.
        assert!(store.get_unchecked(&hash(9)).await.is_err());
    }

    /// **A refused add changes nothing — neither side of the pair.** The block whose height leaves a
    /// gap in the height map is the reachable refusal (a *validation-failed* block is not counted into
    /// `height_map`, so its child's height is a gap), and the index is what `has_all_deps` reads while
    /// the store is what the summary path reads. Leaving the index extended is a block that passes the
    /// dependency gate, is queued, and then fails with `missing justification` — permanently, since
    /// nothing re-queues it (AUDIT C172, issue #103).
    ///
    /// Red before the reorder: `contains` answered `true` for a block `get` could not find.
    #[tokio::test]
    async fn a_refused_height_gap_is_not_left_in_the_index() {
        let store = BlockMetadataStore::create(metadata_store()).await.unwrap();
        store.add(meta(hash(0), &[], 0)).await.unwrap();

        // Height 5 with nothing at 1..4: the height map would stop being contiguous.
        let refusal = store.add(meta(hash(5), &[hash(0)], 5)).await;
        assert!(refusal.is_err(), "a height gap must be refused");
        assert!(
            !store.contains(&hash(5)).await,
            "the index must not report a block the add refused"
        );
        assert_eq!(store.get(&hash(5)).await.unwrap(), None);
    }

    /// **A reset drops the blocks above the anchor — from the store *and* from the index — and a
    /// restart over the same store sees the dropped state** (C249's node-side half, #287).
    ///
    /// This is the primitive an operator performs by hand today: stop the node, delete the shard data
    /// dir — a path that exists as a comment and nowhere as code. The direction that matters is the
    /// restart. The in-memory index is only an index; a rewind that lived only in memory would be
    /// undone by the next boot, putting the node back on exactly the blocks it gave up on, which is
    /// the wedge the reset exists to break.
    ///
    /// Falsifier: the method does not exist on the pre-fix tree, so this test does not **build** there.
    /// That is the R1 form of a red-before — the capability is absent, and an absent capability has no
    /// behaviour to be wrong.
    #[tokio::test]
    async fn dropping_above_the_anchor_takes_the_higher_blocks_with_it() {
        let store = metadata_store();
        let index = BlockMetadataStore::create(store.clone()).await.unwrap();
        // A contiguous chain 0..=4, each block justified by the one below it.
        let mut parents: Vec<BlockHash> = Vec::new();
        for n in 0..=4u8 {
            let h = hash(n);
            index.add(meta(h, &parents, i64::from(n))).await.unwrap();
            parents = vec![h];
        }
        assert_eq!(
            index.height_map().await.len(),
            5,
            "the chain is in the index to begin with"
        );

        let anchor = rchain_shared::refined::BlockHeight::try_from(2).unwrap();
        let removed = index.drop_above(anchor).await.unwrap();
        assert_eq!(
            removed.iter().copied().collect::<BTreeSet<_>>(),
            BTreeSet::from([hash(3), hash(4)]),
            "exactly the blocks above the anchor, and the anchor is not one of them"
        );
        assert_eq!(
            index.get(&hash(4)).await.unwrap(),
            None,
            "gone from the persisted store"
        );
        assert!(
            !index.contains(&hash(3)).await,
            "…and from the index, which is what `has_all_deps` reads"
        );
        assert_eq!(
            index.height_map().await.len(),
            3,
            "the anchor and everything below it survive"
        );

        // The restart the design cares about: a *fresh* index over the same store.
        let rebooted = BlockMetadataStore::create(store).await.unwrap();
        assert_eq!(
            rebooted.height_map().await.len(),
            3,
            "a rewind that lived only in memory would be undone by the next boot"
        );
        assert_eq!(rebooted.get(&hash(4)).await.unwrap(), None);
        assert!(
            rebooted.get(&hash(2)).await.unwrap().is_some(),
            "the anchor itself is kept — it is the state the node replays from"
        );
    }

    /// **And the other side of the same pair: a write that fails does not leave the index claiming the
    /// block.** `has_all_deps` asks the index; the summary path asks the store; a block in the first
    /// and not the second is the divergence from the store's direction. Red before the reorder:
    /// the index entry was made before the write, so the failed `put` left it behind.
    #[tokio::test]
    async fn a_failed_store_write_is_not_left_in_the_index() {
        let store = BlockMetadataStore::create(metadata_store_over(Box::new(FailingPutStore {
            inner: InMemoryKeyValueStore::default(),
        })))
        .await
        .unwrap();

        assert!(store.add(meta(hash(0), &[], 0)).await.is_err());
        assert!(
            !store.contains(&hash(0)).await,
            "the index must not report a block the store never took"
        );
        assert_eq!(store.get(&hash(0)).await.unwrap(), None);
    }

    /// **The regression test for AUDIT C122**, written against the *store* because that is the path C110's
    /// slash rule actually reads, and it asserts both halves of the finding: the flag survives, and the
    /// rule can therefore see it.
    ///
    /// `slashable` is what `validate::slashable_senders` consults to decide which validators a block's
    /// evidence holds responsible. Every read of a stored metadata goes `get` → codec `decode` →
    /// `BlockMetadata::from_proto`, and that conversion hard-coded `false` — so the flag was false for
    /// *every* metadata any caller could see, not only after a restart as the field's doc claimed, and
    /// the slash rule was unreachable: the proposer's `to_slash` was always empty and `slash_is_unjustified`
    /// treated every `Slash` as unjustified.
    ///
    /// The existing round-trip test above cannot see it: its fixture is built with `slashable: false`, so
    /// `false` round-trips to `false`. This one sets the flag — the only difference — and then hands the
    /// read-back metadata to the rule, because "the flag survives" is only interesting as "the rule is
    /// reachable".
    #[tokio::test]
    async fn the_slashable_flag_survives_the_store_round_trip_and_reaches_the_slash_rule() {
        let store = BlockMetadataStore::create(metadata_store()).await.unwrap();
        let mut attributable = meta(hash(0), &[], 0);
        attributable.validation_failed = true;
        attributable.slashable = true;
        store.add(attributable.clone()).await.unwrap();

        let read_back = store.get(&hash(0)).await.unwrap().expect("stored");
        assert!(
            read_back.slashable,
            "the flag the slash rule reads must survive the store — without it nothing in this tree can \
             take a bonded validator's stake (AUDIT C122)"
        );
        assert_eq!(
            read_back, attributable,
            "the whole metadata round-trips, not only the flag"
        );

        // The consequence, which is the finding: a stored attributable failure is evidence — with the
        // **tier** it earned, which is what decides how much of the bond the slash takes (AUDIT C199).
        assert_eq!(
            crate::validate::slashable_senders(std::slice::from_ref(&read_back)),
            std::collections::BTreeMap::from([(
                attributable.sender,
                read_back.slash_severity
            )]),
            "a validator that stored an attributable failure must be whom the slash rule names, at \
             the tier the record carries"
        );
    }

    #[tokio::test]
    async fn dag_state_tracks_child_and_height_maps() {
        let store = BlockMetadataStore::create(metadata_store()).await.unwrap();
        let genesis = meta(hash(0), &[], 0);
        let child = meta(hash(1), &[hash(0)], 1);
        store.add(genesis).await.unwrap();
        store.add(child).await.unwrap();

        let child_map = store.child_map_data().await;
        assert_eq!(child_map[&hash(0)], [hash(1)].into_iter().collect());
        assert!(child_map[&hash(1)].is_empty());

        let height_map = store.height_map().await;
        assert_eq!(
            height_map[&rchain_shared::refined::BlockHeight::try_from(0).unwrap()],
            [hash(0)].into_iter().collect()
        );
        assert_eq!(
            height_map[&rchain_shared::refined::BlockHeight::try_from(1).unwrap()],
            [hash(1)].into_iter().collect()
        );
    }
}
