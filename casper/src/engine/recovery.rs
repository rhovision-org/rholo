//! Replay downloaded history before admitting its metadata (C339).
//!
//! The staging DAG contains only locally checked parents. Wire fringe roots are
//! comparison inputs, never inputs to the merge that checks them. Finality of the
//! selected target remains a bootstrap/operator assumption (Law 83).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use rchain_block_storage::block_store::BlockStore;
use rchain_block_storage::dag::codecs::{
    BlockHashCodec, BlockMetadataCodec, SignedDeployDataCodec,
};
use rchain_block_storage::dag::dag_storage::BlockDagStorage;
use rchain_block_storage::syntax::insert_genesis;
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_models::block_hash::BlockHash;
use rchain_models::block_metadata::BlockMetadata;
use rchain_models::casper::protocol::casper_message::{BlockFringe, FinalizedFringe};
use rchain_shared::log::{Log, LogSource};
use rchain_shared::store_manager::{database, InMemoryStoreManager};
use rchain_shared::typed_store::BytesCodec;

use crate::block_metadata_store::BlockMetadataStore;
use crate::dag::BlockDagKeyValueStorage;
use crate::interpreter_util::ShardHealth;
use crate::merging::BlockIndex;
use crate::runtime_manager::RuntimeManager;

/// Production replay context; built by NodeLaunch from this shard's runtime/config.
#[derive(Clone)]
pub struct RecoveryReplay {
    pub runtime: Arc<RuntimeManager>,
    pub shard_id: String,
    pub min_phlo_price: i64,
    pub max_number_of_parents: i32,
    pub expected_genesis: Option<BlockHash>,
    pub anchor: Option<BlockHash>,
}

/// Only replay can construct this value. Publishing never consumes wire metadata.
#[derive(Debug)]
pub struct VerifiedRecovery {
    entries: Vec<(BlockHash, BlockMetadata)>,
}

async fn staging_dag() -> Result<BlockDagKeyValueStorage, String> {
    let manager = InMemoryStoreManager::default();
    let metadata = database(
        &manager,
        "metadata",
        Arc::new(BlockHashCodec),
        Arc::new(BlockMetadataCodec),
    )
    .await?;
    let index = database(
        &manager,
        "index",
        Arc::new(BytesCodec),
        Arc::new(BlockHashCodec),
    )
    .await?;
    let deploys = database(
        &manager,
        "deploys",
        Arc::new(BytesCodec),
        Arc::new(SignedDeployDataCodec),
    )
    .await?;
    BlockDagKeyValueStorage::create(
        Arc::new(BlockMetadataStore::create(Arc::new(metadata)).await?),
        Arc::new(index),
        Arc::new(deploys),
    )
    .await
}

pub(super) fn compare_claim(local: &BlockMetadata, claimed: &BlockFringe) -> Result<(), String> {
    if local.fringe != claimed.fringe || local.fringe_state_hash != claimed.fringe_state_hash {
        return Err(format!("recovery replay disagrees with bootstrap metadata for {}: local fringe {:?}, local state {}; claimed fringe {:?}, claimed state {}", local.block_hash.to_hex(), local.fringe, rchain_shared::base16::encode(local.fringe_state_hash.as_bytes()), claimed.fringe, rchain_shared::base16::encode(claimed.fringe_state_hash.as_bytes())));
    }
    Ok(())
}

/// Law 85: ordinary replies serve the max member's locally derived claim;
/// explicit anchors and the fresh-genesis reply serve the named block's post-state.
async fn check_target(
    dag: &dyn BlockDagStorage,
    store: &BlockStore,
    fringe: &FinalizedFringe,
    anchor: Option<BlockHash>,
) -> Result<(), String> {
    let mut members = Vec::new();
    for hash in &fringe.hashes {
        let meta = dag.lookup(hash).await?.ok_or_else(|| {
            format!(
                "recovery target {} is not in verified history",
                hash.to_hex()
            )
        })?;
        members.push(meta);
    }
    let latest = members
        .iter()
        .max_by_key(|m| (m.block_num, m.block_hash))
        .ok_or_else(|| "recovery target is empty".to_string())?;
    let expected = if let Some(anchor) = anchor {
        if fringe.hashes != vec![anchor] {
            return Err("recovery reply does not name the requested anchor".to_string());
        }
        let block = store
            .get(&[anchor])
            .await?
            .into_iter()
            .flatten()
            .next()
            .ok_or_else(|| "recovery anchor body missing".to_string())?;
        block.post_state_hash
    } else if members.len() == 1 && i64::from(latest.block_num) == 0 {
        let block = store
            .get(&[latest.block_hash])
            .await?
            .into_iter()
            .flatten()
            .next()
            .ok_or_else(|| "recovery genesis body missing".to_string())?;
        block.post_state_hash
    } else {
        latest.fringe_state_hash
    };
    if fringe.state_hash != expected {
        return Err(format!(
            "recovery target state disagrees with local replay: claimed {}, expected {}",
            rchain_shared::base16::encode(fringe.state_hash.as_bytes()),
            rchain_shared::base16::encode(expected.as_bytes())
        ));
    }
    Ok(())
}

impl RecoveryReplay {
    pub async fn verify(
        &self,
        store: &BlockStore,
        heights: &BTreeMap<i64, BTreeSet<BlockHash>>,
        target: &FinalizedFringe,
        log: &Arc<dyn Log>,
    ) -> Result<VerifiedRecovery, String> {
        let downloaded: BTreeSet<_> = heights
            .values()
            .flat_map(|hashes| hashes.iter().copied())
            .collect();
        super::node_syncing::check_ancestry_shape(&target.ancestry, &downloaded)?;
        let target_set: BTreeSet<_> = target.hashes.iter().copied().collect();
        if target_set.len() != target.hashes.len() || target_set.is_empty() {
            return Err("recovery target must name unique nonempty block hashes".to_string());
        }
        let genesis = heights
            .get(&0)
            .filter(|s| s.len() == 1)
            .and_then(|s| s.first())
            .ok_or_else(|| "recovery requires exactly one height-zero genesis".to_string())?;
        if let Some(pin) = self.expected_genesis.filter(|pin| pin != genesis) {
            return Err(format!(
                "recovery genesis {} differs from pinned genesis {}",
                genesis.to_hex(),
                pin.to_hex()
            ));
        }
        let dag = staging_dag().await?;
        let health = ShardHealth::new();
        let claims: BTreeMap<_, _> = target
            .ancestry
            .iter()
            .map(|claim| (claim.block_hash, claim))
            .collect();
        let mut entries = Vec::new();
        for (height, hashes) in heights {
            for hash in hashes {
                let block = store
                    .get(&[*hash])
                    .await?
                    .into_iter()
                    .flatten()
                    .next()
                    .ok_or_else(|| format!("recovery body missing: {}", hash.to_hex()))?;
                if block.block_hash != *hash
                    || i64::from(block.block_number) != *height
                    || block.shard_id != self.shard_id
                    || !crate::validate::format_of_fields(&block)
                    || !crate::validate::block_hash(&block)
                {
                    return Err(format!(
                        "recovery block content/index mismatch: {}",
                        hash.to_hex()
                    ));
                }
                let meta = if *height == 0 {
                    if !block.justifications.is_empty() {
                        return Err("recovery genesis has parents".to_string());
                    }
                    insert_genesis(&dag, block.clone()).await?;
                    dag.lookup(hash)
                        .await?
                        .ok_or_else(|| "staged genesis missing".to_string())?
                } else {
                    if !crate::validate::block_signature(&block) {
                        return Err(format!("recovery signature invalid: {}", hash.to_hex()));
                    }
                    for parent in &block.justifications {
                        let meta = dag.lookup(parent).await?.ok_or_else(|| {
                            format!("recovery parent {} is not verified", parent.to_hex())
                        })?;
                        if meta.block_num >= block.block_number {
                            return Err("recovery parent is not older than its child".to_string());
                        }
                    }
                    let block_index = |hash| {
                        let dag = &dag;
                        async move {
                            let meta = dag.lookup(&hash).await?.ok_or_else(|| {
                                "recovery index needs verified metadata".to_string()
                            })?;
                            BlockIndex::get_block_index(
                                &self.runtime,
                                dag,
                                store,
                                hash,
                                Blake2b256Hash::from_byte_array(meta.fringe_state_hash.as_bytes()),
                            )
                            .await
                        }
                    };
                    let meta = crate::multi_parent_casper::validate(
                        &dag,
                        store,
                        &self.runtime,
                        &block,
                        &self.shard_id,
                        self.min_phlo_price,
                        self.max_number_of_parents,
                        &block_index,
                        log,
                        &health,
                    )
                    .await
                    .map_err(|e| format!("recovery replay failed for {}: {e:?}", hash.to_hex()))?;
                    if meta.validation_failed || !meta.validated {
                        return Err("recovery validator did not accept the block".to_string());
                    }
                    dag.insert(meta.clone(), block).await?;
                    meta
                };
                if let Some(claim) = claims.get(hash) {
                    compare_claim(&meta, claim)?;
                }
                entries.push((*hash, meta));
            }
        }
        check_target(&dag, store, target, self.anchor).await?;
        // Exercise the actual bonds read against a verified committed post-state.
        let newest = entries
            .last()
            .ok_or_else(|| "recovery history is empty".to_string())?
            .0;
        let block = store
            .get(&[newest])
            .await?
            .into_iter()
            .flatten()
            .next()
            .ok_or_else(|| "verified tip body missing".to_string())?;
        self.runtime.compute_bonds(&block.post_state_hash).await?;
        log.info(
            LogSource::new("casper.engine.RecoveryReplay"),
            &format!(
                "Independently replayed {} recovery blocks; all supplied fringe claims match.",
                entries.len()
            ),
        );
        Ok(VerifiedRecovery { entries })
    }
}

impl VerifiedRecovery {
    pub async fn publish(
        self,
        dag: &dyn BlockDagStorage,
        store: &BlockStore,
    ) -> Result<(), String> {
        publish_verified_blocks(dag, store, &self.entries).await
    }
}

pub(super) async fn publish_verified_blocks(
    dag: &dyn BlockDagStorage,
    store: &BlockStore,
    entries: &[(BlockHash, BlockMetadata)],
) -> Result<(), String> {
    let verified_hashes: BTreeSet<_> = entries.iter().map(|(hash, _)| *hash).collect();
    if !dag
        .get_representation()
        .await
        .dag_set
        .is_subset(&verified_hashes)
    {
        return Err(
            "existing DAG is not a prefix of the independently verified recovery history"
                .to_string(),
        );
    }
    for (hash, meta) in entries {
        let block = store
            .get(&[*hash])
            .await?
            .into_iter()
            .flatten()
            .next()
            .ok_or_else(|| format!("verified body missing: {}", hash.to_hex()))?;
        if let Some(known) = dag.lookup(hash).await? {
            if known.fringe != meta.fringe
                || known.fringe_state_hash != meta.fringe_state_hash
                || !known.validated
                || known.validation_failed
            {
                return Err(format!(
                    "existing recovery metadata disagrees with verified history: {}",
                    hash.to_hex()
                ));
            }
        }
        dag.insert(meta.clone(), block).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_models::block::state_hash::StateHash;

    #[test]
    fn a_fringe_state_claim_must_equal_the_local_replay() {
        let block = rchain_models::casper::protocol::casper_message::BlockMessage {
            version: 1,
            shard_id: "root".to_string(),
            block_hash: BlockHash::new([1; 32]),
            block_number: rchain_shared::refined::BlockHeight::try_from(1).unwrap(),
            sender: rchain_models::validator::Validator::new([0; 65]),
            seq_num: 1.try_into().unwrap(),
            pre_state_hash: StateHash::new([0; 32]),
            post_state_hash: StateHash::new([0; 32]),
            justifications: Vec::new(),
            bonds: BTreeMap::new(),
            rejected_deploys: BTreeSet::new(),
            rejected_blocks: BTreeSet::new(),
            rejected_senders: BTreeSet::new(),
            state: rchain_models::casper::protocol::casper_message::RholangState::default(),
            sig_algorithm: "secp256k1".to_string(),
            sig: Vec::new(),
            timestamp: 0,
        };
        let local = BlockMetadata::from_block(&block);
        let mut claim = BlockFringe {
            block_hash: block.block_hash,
            fringe: local.fringe.clone(),
            fringe_state_hash: local.fringe_state_hash,
        };
        compare_claim(&local, &claim).unwrap();
        claim.fringe_state_hash = StateHash::new([7; 32]);
        assert!(compare_claim(&local, &claim)
            .unwrap_err()
            .contains("disagrees"));
    }
}
