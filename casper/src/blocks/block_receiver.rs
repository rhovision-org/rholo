//! Block receiver (port of `blocks/BlockReceiver.scala`).
//!
//! The pure `BlockReceiverState` state machine (begin/end storing + finished), the `not_validated`
//! helper, and the `apply` stream wiring (incoming + validated block streams → validation queue)
//! are ported.

use rchain_shared::chan;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use rchain_block_storage::block_store::BlockStore;
use rchain_block_storage::dag::dag_storage::BlockDagStorage;
use rchain_models::block_hash::BlockHash;
use rchain_models::casper::protocol::casper_message::BlockMessage;
use rchain_shared::log::{Log, LogSource};
use tokio::sync::mpsc;

use crate::block_disposition::{BlockDispositionStore, BodyDispositionWriter, PendingStage};
use crate::blocks::block_processor;
use crate::blocks::block_retriever::BlockRetriever;
use crate::engine::node_running::MAX_PENDING_BLOCKS;
use crate::validate::{block_hash, block_signature, format_of_fields};

/// Reserved capacity for missing parents of blocks already pending in the receiver.
/// Finite: an attacker can never turn pending dependencies into unbounded state.
const MAX_DEPENDENCY_HEADROOM: usize = 32;

/// Block-receive status (port of `RecvStatus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecvStatus {
    /// Begin checking and storing block.
    BeginStoreBlock,
    /// Block stored in the block store, waiting for validation and DAG insertion.
    EndStoreBlock,
    /// Block sent to validation.
    PendingValidation,
    /// Requested missing dependencies.
    Requested,
}

/// The result of [`BlockReceiverState::claim_reoffer`]: whether the runtime sweep may dispatch a
/// stored pending body, and whether the claim moved the receiver's own state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReofferClaim {
    /// The receiver held the body at `EndStoreBlock`; the sweep now owns the dispatch.
    Claimed,
    /// The receiver holds no in-memory claim; the sweep owns the dispatch.
    NotHeld,
    /// The receiver already has it in flight (or in a state the sweep must not touch).
    Owned,
}

/// Block receiver state (port of `BlockReceiverState`).
///
/// It consists of three events: two to store blocks (begin and end) to prevent a race when storing
/// blocks, and `finished` when a block is validated and added to the DAG (end of processing).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockReceiverState<MId: Ord + Clone + std::fmt::Debug> {
    /// Blocks received and stored in BlockStore (not validated), each with the parents it still waits
    /// for: those not yet validated into the DAG (#223).
    blocks_st: BTreeMap<MId, BTreeSet<MId>>,
    /// Blocks receiving status.
    receive_st: BTreeMap<MId, RecvStatus>,
    /// Blocks mapping with children relations.
    child_relations: BTreeMap<MId, BTreeSet<MId>>,
}

impl<MId: Ord + Clone + std::fmt::Debug> BlockReceiverState<MId> {
    /// Create an empty receiver state (port of `BlockReceiverState.apply`).
    pub fn new() -> Self {
        BlockReceiverState {
            blocks_st: BTreeMap::new(),
            receive_st: BTreeMap::new(),
            child_relations: BTreeMap::new(),
        }
    }

    /// Begin storing a block, marking it to prevent duplicate threads storing the same block. The
    /// returned flag is `true` when storing should proceed (port of `beginStored`).
    pub fn begin_stored(&self, id: MId) -> (Self, bool) {
        // If state is not known or pending request, it's expected, so continue with receiving.
        let expected_receive = match self.receive_st.get(&id) {
            Some(RecvStatus::Requested) => true,
            Some(_) => false,
            None => true,
        };
        if expected_receive {
            let mut receive_st = self.receive_st.clone();
            receive_st.insert(id, RecvStatus::BeginStoreBlock);
            (
                BlockReceiverState {
                    receive_st,
                    ..self.clone()
                },
                true,
            )
        } else {
            (self.clone(), false)
        }
    }

    /// Undo a begin-stored reservation when a store read or capacity check fails.
    /// A dependency which children still need stays Requested; an unrelated
    /// refusal becomes unknown again. Neither is permanently stuck at BeginStoreBlock.
    pub fn abort_stored(&self, id: &MId) -> Self {
        let mut next = self.clone();
        if next.receive_st.get(id) == Some(&RecvStatus::BeginStoreBlock) {
            if next
                .child_relations
                .get(id)
                .is_some_and(|children| !children.is_empty())
            {
                next.receive_st.insert(id.clone(), RecvStatus::Requested);
            } else {
                next.receive_st.remove(id);
            }
        }
        next
    }

    /// **Claim a stored body for the runtime re-offer sweep** (Law 88c), under the receiver's own
    /// lock, so a sweep cannot dispatch a body the receiver is about to release.
    ///
    /// * `NotHeld` — the receiver has no in-memory claim (the overflow refusal path dropped it, or
    ///   this is a post-restart row). The sweep owns the dispatch.
    /// * `Claimed` — the receiver holds it at `EndStoreBlock`; this moves it to
    ///   `PendingValidation`, which is the state `begin_stored` refuses, so a concurrent delivery
    ///   cannot dispatch it a second time.
    /// * `Owned` — already in flight, requested, or otherwise not the sweep's to take.
    pub fn claim_reoffer(&mut self, id: &MId) -> ReofferClaim {
        if !self.blocks_st.contains_key(id) {
            return ReofferClaim::NotHeld;
        }
        match self.receive_st.get(id) {
            Some(RecvStatus::EndStoreBlock) => {
                self.receive_st
                    .insert(id.clone(), RecvStatus::PendingValidation);
                ReofferClaim::Claimed
            }
            _ => ReofferClaim::Owned,
        }
    }

    /// Storing of the block is done, waiting validation. Returns the updated state and the unseen
    /// parent dependencies (port of `endStored`). The Scala `assert` is a `Result` error here so an
    /// invariant violation is logged and the block skipped rather than panicking the task.
    pub fn end_stored(
        &self,
        id: MId,
        parents: Vec<(MId, bool)>,
    ) -> Result<(Self, BTreeSet<MId>), String> {
        let not_stored: BTreeSet<MId> = parents
            .iter()
            .filter(|(_, not_stored)| *not_stored)
            .map(|(parent, _)| parent.clone())
            .collect();
        self.end_stored_awaiting(id, parents, not_stored)
    }

    /// [`end_stored`](Self::end_stored), with the block's dependencies given explicitly: `awaiting` is every
    /// parent that is **not yet validated into the DAG**, which is what the block must wait for.
    ///
    /// **Waiting on "not stored" was not enough (#223).** `end_stored` records only the parents missing
    /// from the block store, so a block whose parents are *stored but not yet validated* — every block a
    /// rejoining validator receives in a burst — has an empty dependency set. The first of those parents
    /// to finish then released it, its validation read the second parent from the DAG, found nothing
    /// (`block summary failed: missing justification`), and the block was dropped in `PendingValidation`,
    /// where a re-delivery is refused: the node never advanced again. Here the dependency set is the
    /// parents not yet in the DAG, so a block is released only when the last of them finishes. The caller
    /// computes `awaiting` while holding the state lock, so no parent can finish between the read and
    /// this call: a parent's `finished` needs the same lock and runs only after its DAG insert.
    pub fn end_stored_awaiting(
        &self,
        id: MId,
        parents: Vec<(MId, bool)>,
        awaiting: BTreeSet<MId>,
    ) -> Result<(Self, BTreeSet<MId>), String> {
        let cur_state_opt = self.receive_st.get(&id);
        if cur_state_opt != Some(&RecvStatus::BeginStoreBlock) {
            return Err(format!(
                "Received should be called only in begin received state, actual: {:?}, hash: {:?}",
                cur_state_opt, id
            ));
        }

        // R29 bounds unsolicited blocks at MAX_PENDING_BLOCKS. Reserve a small,
        // finite allowance for missing parents of blocks already admitted.
        // Refusing those parents when the ordinary bound is full deadlocks
        // the receiver: none of their waiting children can be validated.
        // Never evict a stored-but-unvalidated child to make room.
        if self.blocks_st.len() >= MAX_PENDING_BLOCKS && !self.blocks_st.contains_key(&id) {
            let needed = self
                .child_relations
                .get(&id)
                .is_some_and(|children| !children.is_empty());
            if !needed || self.blocks_st.len() >= MAX_PENDING_BLOCKS + MAX_DEPENDENCY_HEADROOM {
                return Err(format!(
                    "block receiver state full ({} blocks); cannot admit {:?}",
                    self.blocks_st.len(),
                    id
                ));
            }
        }

        // Update blocks state, keep unseen parents only.
        let parents_not_stored: BTreeSet<MId> = parents
            .iter()
            .filter(|(_, not_stored)| *not_stored)
            .map(|(parent, _)| parent.clone())
            .collect();
        let mut unseen_parents = parents_not_stored;
        unseen_parents.retain(|parent| {
            !self.blocks_st.contains_key(parent)
                && !self.receive_st.contains_key(parent)
                && parent != &id
        });
        let mut new_blocks_st = self.blocks_st.clone();
        // A parent the block must wait for: every unseen parent (not stored, so not validated) and every
        // stored parent that is not yet in the DAG. `parent != id` as above, so a malformed self-reference
        // cannot make a block wait on itself.
        let mut deps = awaiting;
        deps.retain(|parent| parent != &id);
        deps.extend(unseen_parents.iter().cloned());
        new_blocks_st.insert(id.clone(), deps);

        // Update block status to received and set unseen parents to Requested.
        let mut new_receive_st = self.receive_st.clone();
        new_receive_st.insert(id.clone(), RecvStatus::EndStoreBlock);
        for parent in &unseen_parents {
            new_receive_st.insert(parent.clone(), RecvStatus::Requested);
        }

        // Update children relations of the received block.
        let mut new_child_relations = self.child_relations.clone();
        for (parent, _) in &parents {
            new_child_relations
                .entry(parent.clone())
                .or_default()
                .insert(id.clone());
        }

        let new_state = BlockReceiverState {
            blocks_st: new_blocks_st,
            receive_st: new_receive_st,
            child_relations: new_child_relations,
        };
        Ok((new_state, unseen_parents))
    }

    /// Finish block validation, updating state and returning the next blocks with validated
    /// dependencies (port of `finished`). The Scala `assert` is a `Result` error here so an
    /// invariant violation is logged and the block skipped rather than panicking the task.
    pub fn finished(
        &self,
        id: MId,
        parents: BTreeSet<MId>,
    ) -> Result<(Self, BTreeSet<MId>), String> {
        let parents_in_state = self.blocks_st.contains_key(&id);
        let is_received = matches!(
            self.receive_st.get(&id),
            Some(RecvStatus::EndStoreBlock) | Some(RecvStatus::PendingValidation)
        );
        // To finish a block it must be present in the state (parents relations and at least stored).
        if !(parents_in_state && is_received) {
            return Err(format!(
                "Calling finished on unexpected block hash {:?}.",
                id
            ));
        }

        // Remove the finished block from its children's dependencies and from the blocks state.
        let childs: Vec<MId> = self
            .child_relations
            .get(&id)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default();
        let updated_blocks: BTreeMap<MId, BTreeSet<MId>> = childs
            .iter()
            .map(|child| {
                let mut deps = self.blocks_st.get(child).cloned().unwrap_or_default();
                deps.remove(&id);
                (child.clone(), deps)
            })
            .collect();
        let mut new_blocks_st = self.blocks_st.clone();
        for (child, deps) in &updated_blocks {
            new_blocks_st.insert(child.clone(), deps.clone());
        }
        new_blocks_st.remove(&id);

        // Next blocks with all dependencies validated and not already in pending validation state.
        let deps_validated: BTreeSet<MId> = updated_blocks
            .iter()
            .filter(|(bid, parents)| {
                let pending = matches!(
                    self.receive_st.get(*bid),
                    Some(RecvStatus::PendingValidation)
                );
                parents.is_empty() && !pending
            })
            .map(|(bid, _)| bid.clone())
            .collect();

        // Set next blocks to pending validation and remove the finished block.
        let mut new_receive_st = self.receive_st.clone();
        for dep in &deps_validated {
            new_receive_st.insert(dep.clone(), RecvStatus::PendingValidation);
        }
        new_receive_st.remove(&id);

        // Remove the finished block from children relations.
        let mut new_child_relations: BTreeMap<MId, BTreeSet<MId>> = BTreeMap::new();
        for (parent, childs) in &self.child_relations {
            if parents.contains(parent) {
                let mut childs = childs.clone();
                childs.remove(&id);
                if !childs.is_empty() {
                    new_child_relations.insert(parent.clone(), childs);
                }
            } else {
                new_child_relations.insert(parent.clone(), childs.clone());
            }
        }

        let new_state = BlockReceiverState {
            blocks_st: new_blocks_st,
            receive_st: new_receive_st,
            child_relations: new_child_relations,
        };
        Ok((new_state, deps_validated))
    }

    /// Release a block if the receiver still holds it; a no-op when it was never end-stored.
    ///
    /// A validation-failed block is inserted into the DAG but is not an `Ok(())`; it must still
    /// release children that were already waiting on its parent relation. A faulted-before-
    /// `end_stored` block is not in `blocks_st`, so the release is an idempotent no-op rather than
    /// an error that hides the original fault.
    pub fn finished_if_present(
        &self,
        id: MId,
        parents: BTreeSet<MId>,
    ) -> Result<(Self, BTreeSet<MId>), String> {
        if !self.blocks_st.contains_key(&id) {
            return Ok((self.clone(), BTreeSet::new()));
        }
        self.finished(id, parents)
    }
}

impl<MId: Ord + Clone + std::fmt::Debug> Default for BlockReceiverState<MId> {
    fn default() -> Self {
        Self::new()
    }
}

/// Check whether a block is stored but not yet validated into the DAG (port of
/// `BlockReceiver.notValidated`).
///
/// **A store that cannot be read is an error, not "not validated".** The oracle reads presence inside
/// `F` (`BlockStore[F].contains`, `NodeRunning.scala:127,231`); the port's `unwrap_or_default()`
/// answered `false`, and `false` is *also* what an already-validated block answers — two meanings its
/// callers cannot tell apart, read silently as "fetch this block again" (AUDIT C67). The read is the
/// same checked one the receiver's other presence sites use, so it is shared rather than re-derived.
pub async fn not_validated(
    block_store: &BlockStore,
    dag: &dyn BlockDagStorage,
    hash: &BlockHash,
) -> Result<bool, String> {
    if !crate::engine::node_running::block_is_known(block_store, hash).await? {
        return Ok(false);
    }
    let repr = dag.get_representation().await;
    Ok(!repr.contains(hash))
}

// -------------------------------------------------------------------------------------------------
// Stream wiring (port of `BlockReceiver.apply`)
// -------------------------------------------------------------------------------------------------

/// Each justification paired with whether it still needs requesting — the `parents` list
/// `BlockReceiverState::end_stored` takes.
///
/// Extracted so the read's failure behaviour is testable without a stream fixture (the
/// `load_node`/`load_node_from_store` split, applied to a block-store read).
///
/// **A read that fails is an error, not "not stored".** The oracle reads presence inside `F`
/// (`BlockStore[F].contains`, `NodeRunning.scala:127,231`), so a store error is an error there and the
/// caller logs it; the port's `unwrap_or_default()` answered `true` — "not stored" — which only causes
/// a redundant request, **but silently**: the operator never learns the block store is failing (AUDIT
/// C67's owed log line). A store that answers fewer presence bits than keys is refused too, rather
/// than read as "not stored".
pub(crate) async fn parents_not_stored(
    block_store: &BlockStore,
    justifications: &[BlockHash],
) -> Result<Vec<(BlockHash, bool)>, String> {
    let mut parents = Vec::with_capacity(justifications.len());
    for hash in justifications {
        let contains = block_store.contains(&[*hash]).await?;
        let stored = contains.first().copied().ok_or_else(|| {
            format!(
                "the block store answered no presence bit for {}",
                hash.to_hex()
            )
        })?;
        parents.push((*hash, !stored));
    }
    Ok(parents)
}

/// Check that a block is cryptographically safe and part of the same shard (port of
/// `checkIfOfInterest`).
async fn check_if_of_interest(
    block: &BlockMessage,
    conf_shard_name: &str,
    log: &dyn Log,
    source: LogSource,
) -> bool {
    let valid_shard = conf_shard_name == block.shard_id;
    if !valid_shard {
        log.info(
            source,
            &format!(
                "Ignored block with invalid shard, expected: {conf_shard_name}, received: {}",
                block.shard_id
            ),
        );
    }
    valid_shard && format_of_fields(block) && block_hash(block) && block_signature(block)
}

/// Check that a block is older than the current DAG's lowest height (port of `checkIfKnown`).
async fn check_if_known(block: &BlockMessage, dag: &dyn BlockDagStorage) -> bool {
    let repr = dag.get_representation().await;
    repr.height_map
        .first_key_value()
        .map(|(h, _)| i64::from(*h))
        .unwrap_or(-1)
        > i64::from(block.block_number)
}

/// Request the missing dependencies of a block (port of `requestMissingDependencies`).
async fn request_missing_dependencies(
    deps: &BTreeSet<BlockHash>,
    block_retriever: &BlockRetriever,
) {
    for hash in deps {
        block_retriever.request_missing_dependency(hash).await;
    }
}

/// Re-send stored blocks back to the incoming queue for validation (port of `sendToValidate`).
///
/// **A store read that fails is not "there is nothing to re-send".** The port's earlier form read with
/// `.ok()` and dropped the error on the floor: the hash left the batch, nothing was logged, and no
/// other path would offer it again — one of only two genuinely silent paths the failure-mode HAZOP
/// found (C249). It is the **third site of AUDIT C67's class**, after `parents_not_stored` and
/// `not_validated`, both of which answer `Err` and are logged by the caller; this one was the site
/// that was missed, and it is fixed the same way rather than a third way.
///
/// **What stays owed is the recovery, and it is named rather than faked.** A parent that is never
/// re-sent is a parent whose `finished` never re-runs, so the block waiting on it stays pending — a
/// wedge, which belongs with the node-wedge work (C249's F-U5-01) rather than here. The obvious
/// re-request does not recover it: re-admitting the hash makes a peer send a block the store already
/// holds, and the receiver's `begin_stored` sees a block it has already processed and drops it. A
/// recovery that does not recover is worse than the missing one, so the honest change is the loud half.
async fn send_to_validate(
    hashes: &BTreeSet<BlockHash>,
    block_store: &BlockStore,
    put_to_incoming_queue: &(dyn Fn(BlockMessage) + Send + Sync),
) -> Result<(), String> {
    for hash in hashes {
        let mut stored = block_store
            .get(&[*hash])
            .await
            .map_err(|e| format!("could not read stored block {}: {e}", hash.to_hex()))?;
        if let Some(block) = stored.pop().flatten() {
            put_to_incoming_queue(block);
        }
    }
    Ok(())
}

/// Process incoming blocks (port of `incomingBlocks`): filter, store, resolve dependencies, and
/// forward dependency-free blocks to the output queue.
async fn incoming_blocks(
    mut incoming_blocks_rx: mpsc::Receiver<BlockMessage>,
    state: Arc<tokio::sync::Mutex<BlockReceiverState<BlockHash>>>,
    conf_shard_name: Arc<str>,
    block_store: BlockStore,
    disposition: BlockDispositionStore,
    body_writer: BodyDispositionWriter,
    dag: Arc<dyn BlockDagStorage>,
    block_retriever: Arc<BlockRetriever>,
    put_to_incoming_queue: Arc<dyn Fn(BlockMessage) + Send + Sync>,
    out_tx: mpsc::UnboundedSender<BlockHash>,
    log: Arc<dyn Log>,
) {
    let source = LogSource::new("casper.blocks.BlockReceiver");
    while let Some(block) = incoming_blocks_rx.recv().await {
        // Filter out blocks that are not of interest.
        if !check_if_of_interest(&block, &conf_shard_name, log.as_ref(), source).await {
            log.info(
                source,
                &format!("Block {} is malformed. Dropped", block.block_hash.to_hex()),
            );
            continue;
        }

        // Renew one overdue request even when its direct children are already
        // stored and will not traverse dependency admission again.
        block_retriever
            .retry_oldest_pending_request(&block.block_hash)
            .await;

        // Ignore blocks older than the DAG before reserving their hash.
        // The old order left rejected hashes stuck in BeginStoreBlock.
        if check_if_known(&block, dag.as_ref()).await {
            log.info(
                source,
                &format!(
                    "Block {} is not of interest. Dropped",
                    block.block_hash.to_hex()
                ),
            );
            continue;
        }

        let should_check = {
            let mut guard = state.lock().await;
            let (new_state, should_check) = guard.begin_stored(block.block_hash);
            *guard = new_state;
            should_check
        };
        if !should_check {
            continue;
        }

        // Store the block and resolve its parent dependencies.
        let block_stored = match block_store.contains(&[block.block_hash]).await {
            Ok(present) => present.first().copied().unwrap_or(false),
            Err(e) => {
                // **A read that fails is not "not stored" *silently*.** This erasure is the same shape
                // `send_to_validate` had (C249's class, AUDIT C67's family): "not stored" is a decision
                // about the block, and a store that cannot answer has not made one. The put below would
                // fail too, but only after this line had pretended the store replied.
                log.error(
                    source,
                    &format!(
                        "Failed to read whether block {} is stored, skipping: {e}",
                        block.block_hash.to_hex()
                    ),
                );
                let mut guard = state.lock().await;
                *guard = guard.abort_stored(&block.block_hash);
                continue;
            }
        };
        // **The body and the obligation are one write** (C350, U19, F-U19-01/05/07). The row used to
        // be a second transaction in another LMDB environment, so a crash between them left a body
        // nothing owed. One transaction cannot span environments, so the ledger shares the body's and
        // `store_body_and_mark_pending` commits both together: either both are durable or neither is.
        if !block_stored {
            if let Err(e) = body_writer
                .store_body_and_mark_pending(&block, PendingStage::Stored)
                .await
            {
                // The transaction aborted, so the body is *not* stored: the block is not ours to
                // settle and the peer or retriever offers it again.
                log.error(
                    source,
                    &format!(
                        "Failed to store block {} (or to record its disposition), skipping: {e}",
                        block.block_hash.to_hex()
                    ),
                );
                let mut guard = state.lock().await;
                *guard = guard.abort_stored(&block.block_hash);
                continue;
            }
        } else if let Err(e) = disposition
            .mark_pending(block.block_hash, block.block_number, PendingStage::Stored)
            .await
        {
            // A re-delivery of a body already in the store: there is no body write to pair this with
            // (and re-marking an existing row is idempotent), so a failure here is the fault surface.
            block_processor::note_pipeline_fault(&block, "disposition write", &e);
            log.error(
                source,
                &format!(
                    "Could not record the disposition of stored block {}: {e}",
                    block.block_hash.to_hex()
                ),
            );
        }

        // The loop's own failure idiom (as for `end_stored` below): log with the block, skip it, and
        // let the peer or the retriever offer it again.
        let parents = match parents_not_stored(&block_store, &block.justifications).await {
            Ok(parents) => parents,
            Err(e) => {
                log.error(
                    source,
                    &format!(
                        "Failed to read whether block {}'s parents are stored, skipping: {e}",
                        block.block_hash.to_hex()
                    ),
                );
                let mut guard = state.lock().await;
                *guard = guard.abort_stored(&block.block_hash);
                continue;
            }
        };

        // **The parents not yet in the DAG are read under the state lock (#223)**, and they are the block's
        // dependencies: a parent's `finished` takes the same lock after its DAG insert, so a parent read as
        // absent here is one whose `finished` has not run yet and will release this block when it does.
        let missing_dependencies: BTreeSet<BlockHash> = parents
            .iter()
            .filter(|(_, not_stored)| *not_stored)
            .map(|(hash, _)| *hash)
            .collect();
        let (mut pending_requests, has_all_deps) = {
            let mut guard = state.lock().await;
            let repr = dag.get_representation().await;
            let awaiting: BTreeSet<BlockHash> = block
                .justifications
                .iter()
                .filter(|h| !repr.contains(h))
                .copied()
                .collect();
            let has_all_deps = awaiting.is_empty();
            match guard.end_stored_awaiting(block.block_hash, parents, awaiting) {
                Ok((new_state, unseen)) => {
                    *guard = new_state;
                    (unseen, has_all_deps)
                }
                Err(e) => {
                    *guard = guard.abort_stored(&block.block_hash);
                    log.error(source, &e);
                    continue;
                }
            }
        };

        // An earlier request may have lost its reply during a partition. A new
        // child still needs that parent even when receiver state already marks
        // it Requested. The retriever reserves and rate-limits renewed demand.
        pending_requests.extend(missing_dependencies);

        block_retriever.ack_received(&block.block_hash).await;

        let mut parents_to_validate = BTreeSet::new();
        for hash in &block.justifications {
            let validated = match not_validated(&block_store, dag.as_ref(), hash).await {
                Ok(validated) => validated,
                Err(e) => {
                    log.error(
                        source,
                        &format!(
                            "Failed to read whether block {} is validated, skipping: {e}",
                            hash.to_hex()
                        ),
                    );
                    continue;
                }
            };
            if validated {
                parents_to_validate.insert(*hash);
            }
        }

        if has_all_deps {
            chan::unbounded_send(&out_tx, block.block_hash);
        } else {
            if !pending_requests.is_empty() {
                request_missing_dependencies(&pending_requests, block_retriever.as_ref()).await;
            }
            if !parents_to_validate.is_empty() {
                if let Err(e) = send_to_validate(
                    &parents_to_validate,
                    &block_store,
                    put_to_incoming_queue.as_ref(),
                )
                .await
                {
                    log.error(
                        source,
                        &format!(
                            "Failed to re-send block {}'s stored parents for re-validation: {e}",
                            block.block_hash.to_hex()
                        ),
                    );
                }
            }
        }
    }
}

/// A sampled queue depth and whether its consumer is alive. Depth excludes the
/// item currently being processed and does not measure the payload's heap bytes.
pub type QueueObserver = Arc<dyn Fn(usize, bool) + Send + Sync>;

struct QueueObservation(QueueObserver);

impl Drop for QueueObservation {
    fn drop(&mut self) {
        // The receiver is being dropped: any remaining queued items are discarded.
        (self.0)(0, false);
    }
}

/// The two things [`consume_observed_queue`] asks of a queue: how deep it is, and the next item.
///
/// **Why this exists rather than a concrete receiver type.** `tokio::mpsc::Receiver` and
/// `UnboundedReceiver` share no public trait — `recv` and `len` are inherent on each — and the ingress path
/// uses both: C175 bounded the validated-blocks queue and the tap beside it, while the queue observers'
/// own unit tests drive unbounded ones. Without this, the observation loop would be written twice and the
/// two copies would drift, which is the failure mode the observer exists to prevent one level up.
///
/// The trait is public because a public generic function cannot name a private bound.
pub trait ObservedQueue<T> {
    /// How many items are waiting: a *sample*, not a guarantee.
    fn depth(&self) -> usize;
    /// The next item, or `None` once every sender is gone.
    fn recv(&mut self) -> impl std::future::Future<Output = Option<T>> + Send;
}

impl<T: Send> ObservedQueue<T> for mpsc::Receiver<T> {
    fn depth(&self) -> usize {
        self.len()
    }
    async fn recv(&mut self) -> Option<T> {
        mpsc::Receiver::recv(self).await
    }
}

impl<T: Send> ObservedQueue<T> for mpsc::UnboundedReceiver<T> {
    fn depth(&self) -> usize {
        self.len()
    }
    async fn recv(&mut self) -> Option<T> {
        mpsc::UnboundedReceiver::recv(self).await
    }
}

/// Consume the original queue without introducing a forwarding queue or copying
/// items. Sample every 100 ms, including while asynchronous processing waits.
/// Synchronous work that blocks this executor can delay sampling; the maximum
/// reported by the observer must therefore be described as a sampled maximum.
pub async fn consume_observed_queue<R, T, F, Fut>(
    mut rx: R,
    observer: Option<QueueObserver>,
    mut consume: F,
) where
    R: ObservedQueue<T> + Send,
    T: Send,
    F: FnMut(T) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let Some(observer) = observer else {
        while let Some(item) = rx.recv().await {
            consume(item).await;
        }
        return;
    };
    let _observation = QueueObservation(observer.clone());
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        observer(rx.depth(), true);
        let item = tokio::select! {
            item = rx.recv() => item,
            _ = interval.tick() => continue,
        };
        let Some(item) = item else {
            break;
        };
        let work = consume(item);
        tokio::pin!(work);
        loop {
            tokio::select! {
                _ = &mut work => break,
                _ = interval.tick() => observer(rx.depth(), true),
            }
        }
    }
}

/// Process validated blocks (port of `validatedBlocks`): update state and forward the next
/// dependency-free blocks to the output queue.
async fn validated_blocks(
    finished_processing_rx: mpsc::Receiver<BlockMessage>,
    state: Arc<tokio::sync::Mutex<BlockReceiverState<BlockHash>>>,
    out_tx: mpsc::UnboundedSender<BlockHash>,
    log: Arc<dyn Log>,
    observer: Option<QueueObserver>,
) {
    let source = LogSource::new("casper.blocks.BlockReceiver");
    consume_observed_queue(finished_processing_rx, observer, move |block| {
        let state = state.clone();
        let out_tx = out_tx.clone();
        let log = log.clone();
        async move {
            let parents: BTreeSet<BlockHash> = block.justifications.iter().copied().collect();
            let next = {
                let mut guard = state.lock().await;
                match guard.finished_if_present(block.block_hash, parents) {
                    Ok((new_state, next)) => {
                        *guard = new_state;
                        next
                    }
                    Err(e) => {
                        log.error(source, &e);
                        return;
                    }
                }
            };
            for hash in next {
                chan::unbounded_send(&out_tx, hash);
            }
        }
    })
    .await;
}

/// Wire the incoming and validated block streams to a shared validation queue (port of
/// `BlockReceiver.apply`). Returns the queue of block hashes ready for validation.
pub fn apply(
    state: Arc<tokio::sync::Mutex<BlockReceiverState<BlockHash>>>,
    incoming_blocks_rx: mpsc::Receiver<BlockMessage>,
    finished_processing_rx: mpsc::Receiver<BlockMessage>,
    released_processing_rx: mpsc::Receiver<BlockMessage>,
    conf_shard_name: String,
    block_store: BlockStore,
    disposition: BlockDispositionStore,
    body_writer: BodyDispositionWriter,
    dag: Arc<dyn BlockDagStorage>,
    block_retriever: Arc<BlockRetriever>,
    put_to_incoming_queue: Arc<dyn Fn(BlockMessage) + Send + Sync>,
    log: Arc<dyn Log>,
) -> mpsc::UnboundedReceiver<BlockHash> {
    apply_with_queue_observer(
        state,
        incoming_blocks_rx,
        finished_processing_rx,
        released_processing_rx,
        conf_shard_name,
        block_store,
        disposition,
        body_writer,
        dag,
        block_retriever,
        put_to_incoming_queue,
        log,
        None,
    )
}

/// The existing receiver wiring, with optional validated-queue depth observation.
pub fn apply_with_queue_observer(
    state: Arc<tokio::sync::Mutex<BlockReceiverState<BlockHash>>>,
    incoming_blocks_rx: mpsc::Receiver<BlockMessage>,
    finished_processing_rx: mpsc::Receiver<BlockMessage>,
    released_processing_rx: mpsc::Receiver<BlockMessage>,
    conf_shard_name: String,
    block_store: BlockStore,
    disposition: BlockDispositionStore,
    body_writer: BodyDispositionWriter,
    dag: Arc<dyn BlockDagStorage>,
    block_retriever: Arc<BlockRetriever>,
    put_to_incoming_queue: Arc<dyn Fn(BlockMessage) + Send + Sync>,
    log: Arc<dyn Log>,
    observer: Option<QueueObserver>,
) -> mpsc::UnboundedReceiver<BlockHash> {
    let (out_tx, out_rx) = mpsc::unbounded_channel::<BlockHash>();

    tokio::spawn(incoming_blocks(
        incoming_blocks_rx,
        state.clone(),
        Arc::from(conf_shard_name),
        block_store,
        disposition,
        body_writer,
        dag.clone(),
        block_retriever,
        put_to_incoming_queue,
        out_tx.clone(),
        log.clone(),
    ));
    // Accepted blocks release through the validated stream (and its taps). A refused insert is
    // not a validation success, but it is still a DAG entry and must release children that were
    // already waiting on its parent relation; give that stream its own consumer so it cannot
    // trigger autopropose/attestation taps.
    tokio::spawn(validated_blocks(
        finished_processing_rx,
        state.clone(),
        out_tx.clone(),
        log.clone(),
        observer,
    ));
    tokio::spawn(validated_blocks(
        released_processing_rx,
        state,
        out_tx,
        log,
        None,
    ));

    out_rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn queue_observer_sees_backlog_while_processing_waits_and_keeps_order() {
        use std::sync::Mutex;
        use std::time::Duration;
        let (tx, rx) = mpsc::unbounded_channel::<usize>();
        let samples = Arc::new(Mutex::new(Vec::new()));
        let backlog_seen = Arc::new(tokio::sync::Notify::new());
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let received = Arc::new(Mutex::new(Vec::new()));
        let observer: QueueObserver = Arc::new({
            let samples = samples.clone();
            let backlog_seen = backlog_seen.clone();
            move |depth, active| {
                samples.lock().unwrap().push((depth, active));
                if depth == 5 && active {
                    backlog_seen.notify_one();
                }
            }
        });
        let task = tokio::spawn(consume_observed_queue(rx, Some(observer), {
            let started = started.clone();
            let release = release.clone();
            let received = received.clone();
            move |item| {
                let started = started.clone();
                let release = release.clone();
                let received = received.clone();
                async move {
                    if item == 0 {
                        started.notify_one();
                        let permit = release.acquire().await.unwrap();
                        permit.forget();
                    }
                    received.lock().unwrap().push(item);
                }
            }
        }));
        tx.send(0).unwrap();
        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .unwrap();
        for item in 1..=5 {
            tx.send(item).unwrap();
        }
        // The consumer is still blocked; this must be a timer sample, not a dequeue sample.
        tokio::time::timeout(Duration::from_secs(2), backlog_seen.notified())
            .await
            .unwrap();
        assert!(received.lock().unwrap().is_empty());
        release.add_permits(1);
        drop(tx);
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(*received.lock().unwrap(), vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(samples.lock().unwrap().last(), Some(&(0, false)));
    }

    #[tokio::test]
    async fn queue_observer_marks_cancelled_consumer_inactive() {
        use std::sync::Mutex;
        use std::time::Duration;
        let (tx, rx) = mpsc::unbounded_channel::<usize>();
        let latest = Arc::new(Mutex::new((0, false)));
        let started = Arc::new(tokio::sync::Notify::new());
        let observer: QueueObserver = Arc::new({
            let latest = latest.clone();
            move |depth, active| {
                *latest.lock().unwrap() = (depth, active);
            }
        });
        let task = tokio::spawn(consume_observed_queue(rx, Some(observer), {
            let started = started.clone();
            move |_| {
                let started = started.clone();
                async move {
                    started.notify_one();
                    std::future::pending::<()>().await;
                }
            }
        }));
        tx.send(1).unwrap();
        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(*latest.lock().unwrap(), (0, false));
        assert!(tx.send(2).is_err());
    }

    #[tokio::test]
    async fn queue_observer_disabled_preserves_delivery() {
        let (tx, rx) = mpsc::unbounded_channel();
        tx.send(1).unwrap();
        tx.send(2).unwrap();
        drop(tx);
        let received = Arc::new(std::sync::Mutex::new(Vec::new()));
        consume_observed_queue(rx, None, {
            let received = received.clone();
            move |item| {
                let received = received.clone();
                async move {
                    received.lock().unwrap().push(item);
                }
            }
        })
        .await;
        assert_eq!(*received.lock().unwrap(), vec![1, 2]);
    }

    /// **The bounded half of `ObservedQueue`, which no other test in this file reaches.** Every other test
    /// here drives `consume_observed_queue` with an unbounded channel, so without this the `mpsc::Receiver`
    /// arm added for C175 would run only in production — the shape of green test that measures the path the
    /// node does not take.
    ///
    /// The assertion is the *producer's* wait, because that is the whole content of the bound: a queue that
    /// never makes its sender wait would be bounded in name only. The consumer is held inside its closure
    /// while the queue is filled, and the send past the bound must not complete.
    #[tokio::test]
    async fn a_bounded_queue_makes_its_producer_wait() {
        use std::sync::Mutex;
        use std::time::Duration;

        let (tx, rx) = mpsc::channel::<usize>(2);
        let received = Arc::new(Mutex::new(Vec::new()));
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let task = tokio::spawn(consume_observed_queue(rx, None, {
            let received = received.clone();
            let started = started.clone();
            let release = release.clone();
            move |item| {
                let received = received.clone();
                let started = started.clone();
                let release = release.clone();
                async move {
                    if item == 0 {
                        started.notify_one();
                        let permit = release.acquire().await.unwrap();
                        permit.forget();
                    }
                    received.lock().unwrap().push(item);
                }
            }
        }));

        tx.send(0).await.unwrap(); // taken by the consumer, which then blocks inside the closure
        started.notified().await;
        tx.send(1).await.unwrap(); // one slot left
        tx.send(2).await.unwrap(); // and now the queue is full
        assert!(
            tokio::time::timeout(Duration::from_millis(200), tx.send(3))
                .await
                .is_err(),
            "a full queue must make its producer wait; this one did not"
        );

        release.add_permits(1);
        drop(tx);
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(*received.lock().unwrap(), vec![0, 1, 2]);
    }

    /// A block store whose every read fails, so a read error is observable as one.
    struct FailingBlockStore;

    #[async_trait::async_trait]
    impl rchain_shared::typed_store::KeyValueTypedStore<BlockHash, BlockMessage> for FailingBlockStore {
        async fn get(&self, _keys: &[BlockHash]) -> Result<Vec<Option<BlockMessage>>, String> {
            Err("the block store is down".to_string())
        }
        async fn put(&self, _pairs: &[(BlockHash, BlockMessage)]) -> Result<(), String> {
            Err("the block store is down".to_string())
        }
        async fn delete(&self, _keys: &[BlockHash]) -> Result<usize, String> {
            Err("the block store is down".to_string())
        }
        async fn contains(&self, _keys: &[BlockHash]) -> Result<Vec<bool>, String> {
            Err("the block store is down".to_string())
        }
        async fn to_map(
            &self,
        ) -> Result<std::collections::BTreeMap<BlockHash, BlockMessage>, String> {
            Err("the block store is down".to_string())
        }
    }

    /// A `BlockDagStorage` that is never reached: `not_validated` returns on the failing block-store
    /// read before it looks at the DAG, so this stub only has to exist.
    struct UnusedDag;

    #[async_trait::async_trait]
    impl BlockDagStorage for UnusedDag {
        async fn get_representation(
            &self,
        ) -> Arc<rchain_block_storage::dag::representation::DagRepresentation> {
            todo!("not_validated returns before this on a failing block store")
        }
        async fn insert(
            &self,
            _block_metadata: rchain_models::block_metadata::BlockMetadata,
            _block: BlockMessage,
        ) -> Result<(), String> {
            todo!("unreachable")
        }
        async fn lookup(
            &self,
            _block_hash: &BlockHash,
        ) -> Result<Option<rchain_models::block_metadata::BlockMetadata>, String> {
            todo!("unreachable")
        }
        async fn lookup_by_deploy_id(
            &self,
            _deploy_id: &rchain_block_storage::dag::dag_storage::DeployId,
        ) -> Result<Option<BlockHash>, String> {
            todo!("unreachable")
        }
        async fn add_deploy(
            &self,
            _deploy: rchain_models::casper::protocol::casper_message::SignedDeployData,
        ) -> Result<(), String> {
            todo!("unreachable")
        }
        async fn pooled_deploys(
            &self,
        ) -> Result<
            std::collections::BTreeMap<
                rchain_block_storage::dag::dag_storage::DeployId,
                rchain_models::casper::protocol::casper_message::SignedDeployData,
            >,
            String,
        > {
            todo!("unreachable")
        }
        async fn contains_deploy_in_pool(
            &self,
            _deploy_id: &rchain_block_storage::dag::dag_storage::DeployId,
        ) -> Result<bool, String> {
            todo!("unreachable")
        }
    }

    /// **A store that cannot be read is not "not validated".**
    ///
    /// `false` is what an *already validated* block answers too, so a flattened store error is a
    /// meaning the caller cannot tell from a legitimate one — it re-fetches a block it has, silently
    /// (AUDIT C67). The oracle's `contains` is in `F`, so the Scala's failure reaches its caller.
    ///
    /// Falsifier, both forms. Pre-fix this site carried the *same* `unwrap_or_default()` expression
    /// `block_is_known` carried, and that one's witnessing form was run pre-fix
    /// (`a_store_that_cannot_be_read_is_not_an_unknown_block`, 2026-09-24); after the fix
    /// `not_validated` **delegates** to that checked read, so the two cannot drift. Here the refusal is
    /// asserted through this function's own signature.
    #[tokio::test]
    async fn a_store_that_cannot_be_read_is_not_an_unvalidated_block() {
        let block_store: BlockStore = Arc::new(FailingBlockStore);
        let dag = UnusedDag;
        let hash = BlockHash::new([0x11; 32]);

        let err = not_validated(&block_store, &dag, &hash)
            .await
            .expect_err("a block store that cannot be read must not answer \"not validated\"");
        assert!(
            err.contains("the block store is down"),
            "and the refusal must name the failure, got: {err}"
        );
    }

    /// **A store that cannot be read is not "the parent is not stored".**
    ///
    /// The oracle reads presence inside `F` (`BlockStore[F].contains`, `NodeRunning.scala:127,231`),
    /// so a store error is an error there; the port's `unwrap_or_default()` answered `true` — "not
    /// stored" — which only causes a redundant request (the *conservative* direction), but silently:
    /// the operator never learns that the block store is failing (AUDIT C67's owed log line).
    ///
    /// Falsifier, both forms. Pre-fix (witnessing): a failing store was answered with
    /// `vec![(hash, true)]` — "not stored" — silently, and that assertion **passed on exactly that**
    /// (run 2026-09-24 before the change). Post-fix: an `Err` naming the failure.
    #[tokio::test]
    async fn a_store_that_cannot_be_read_is_not_an_unstored_parent() {
        let block_store: BlockStore = Arc::new(FailingBlockStore);
        let hash = BlockHash::new([0x11; 32]);

        let err = parents_not_stored(&block_store, &[hash])
            .await
            .expect_err("a block store that cannot be read must not answer \"not stored\"");
        assert!(
            err.contains("the block store is down"),
            "and the refusal must name the failure, got: {err}"
        );
    }

    /// **A store that cannot be read is not "there is nothing to re-send"** — the third site of AUDIT
    /// C67's class, beside the two tests above.
    ///
    /// `send_to_validate` re-injects a stored, validated parent so the receiver's state machine can run
    /// `finished` for it and release the block waiting on it. It read the store with `.ok()`, so a
    /// store failure was answered with *silence*: the hash left the batch, nothing was logged, and
    /// nothing re-offered it (C249's F-U8-01).
    ///
    /// Falsifier, both forms. **Pre-fix, witnessed**: the pre-fix body was run verbatim against this
    /// store during this change and printed `forwarded=0` — no error, no log line, nothing re-offered —
    /// so the `expect_err` below fails on exactly the old behaviour. Post-fix: an `Err` naming the
    /// failure — and still nothing forwarded, because a failed read is not a re-send.
    #[tokio::test]
    async fn a_store_that_cannot_be_read_is_not_a_parent_with_nothing_to_re_send() {
        let block_store: BlockStore = Arc::new(FailingBlockStore);
        let hash = BlockHash::new([0x22; 32]);
        let hashes: BTreeSet<BlockHash> = std::iter::once(hash).collect();
        let forwarded = Arc::new(std::sync::Mutex::new(0usize));
        let sink = {
            let forwarded = forwarded.clone();
            move |_: BlockMessage| *forwarded.lock().unwrap() += 1
        };

        let err = send_to_validate(&hashes, &block_store, &sink)
            .await
            .expect_err("a store that cannot be read must not answer \"nothing to re-send\"");
        assert!(
            err.contains("the block store is down"),
            "and the refusal must name the failure, got: {err}"
        );
        assert_eq!(
            *forwarded.lock().unwrap(),
            0,
            "a failed read forwards nothing, so the error is the only signal there is"
        );
    }

    type MId = String;

    fn parents(items: &[(&str, bool)]) -> Vec<(MId, bool)> {
        items
            .iter()
            .map(|(id, stored)| (id.to_string(), *stored))
            .collect()
    }

    #[test]
    fn begin_stored_true_if_unknown() {
        let (st, is_receiving) = BlockReceiverState::<MId>::new().begin_stored("A1".to_string());
        assert_eq!(
            st.receive_st,
            BTreeMap::from([("A1".to_string(), RecvStatus::BeginStoreBlock)])
        );
        assert!(is_receiving);
    }

    #[test]
    fn begin_stored_false_if_not_requested() {
        let (st, _) = BlockReceiverState::<MId>::new().begin_stored("A1".to_string());
        let (new_st, is_receiving) = st.begin_stored("A1".to_string());
        assert_eq!(
            new_st.receive_st,
            BTreeMap::from([("A1".to_string(), RecvStatus::BeginStoreBlock)])
        );
        assert!(!is_receiving);
    }

    #[test]
    fn begin_stored_true_if_requested() {
        let (st, _) = BlockReceiverState::<MId>::new().begin_stored("A2".to_string());
        let (new_st, _) = st
            .end_stored("A2".to_string(), parents(&[("A1", true)]))
            .unwrap();
        // Unseen parent A1 now has Requested status.
        let (_, is_receiving) = new_st.begin_stored("A1".to_string());
        assert!(is_receiving);
    }

    #[test]
    fn a_full_receiver_admits_the_dependency_that_can_drain_it() {
        let parent = "missing-parent".to_string();
        let mut state = BlockReceiverState::<MId>::new();
        for n in 0..MAX_PENDING_BLOCKS {
            let id = format!("orphan-{n:04}");
            state
                .blocks_st
                .insert(id.clone(), BTreeSet::from([parent.clone()]));
            state
                .receive_st
                .insert(id.clone(), RecvStatus::EndStoreBlock);
            state
                .child_relations
                .entry(parent.clone())
                .or_default()
                .insert(id);
        }
        state
            .receive_st
            .insert(parent.clone(), RecvStatus::Requested);

        let (begun, accepted) = state.begin_stored(parent.clone());
        assert!(accepted);
        let (admitted, _) = begun.end_stored(parent.clone(), Vec::new()).unwrap();
        assert_eq!(admitted.blocks_st.len(), MAX_PENDING_BLOCKS + 1);
        assert!(admitted.blocks_st.contains_key(&parent));
        assert_eq!(admitted.child_relations[&parent].len(), MAX_PENDING_BLOCKS);

        let (released, ready) = admitted.finished(parent.clone(), BTreeSet::new()).unwrap();
        assert_eq!(ready.len(), MAX_PENDING_BLOCKS);
        assert!(!released.blocks_st.contains_key(&parent));
    }

    /// **A re-offer claim is taken once** (Law 88c). A held `EndStoreBlock` body moves to
    /// `PendingValidation`, which `begin_stored` refuses, so a concurrent delivery cannot dispatch
    /// it a second time; an unheld body is the sweep's (`NotHeld`), and an in-flight one is not.
    #[test]
    fn a_reoffer_claim_is_taken_once_and_never_steals_an_inflight_block() {
        let state = BlockReceiverState::<MId>::new();
        let (state, _) = state.begin_stored("held".to_string());
        let (mut state, _) = state.end_stored("held".to_string(), Vec::new()).unwrap();
        assert_eq!(
            state.claim_reoffer(&"held".to_string()),
            ReofferClaim::Claimed
        );
        assert_eq!(
            state.receive_st.get("held"),
            Some(&RecvStatus::PendingValidation),
            "the claim moves the receiver's own state"
        );
        assert_eq!(
            state.claim_reoffer(&"held".to_string()),
            ReofferClaim::Owned
        );
        assert_eq!(
            state.claim_reoffer(&"absent".to_string()),
            ReofferClaim::NotHeld,
            "an overflow-refused body has no in-memory claim"
        );
    }

    #[test]
    fn dependency_headroom_is_finite_and_unrelated_blocks_are_refused() {
        let mut state = BlockReceiverState::<MId>::new();
        for n in 0..MAX_PENDING_BLOCKS {
            let id = format!("orphan-{n:04}");
            state
                .blocks_st
                .insert(id.clone(), BTreeSet::from(["missing".to_string()]));
            state.receive_st.insert(id, RecvStatus::EndStoreBlock);
        }
        for n in 0..MAX_DEPENDENCY_HEADROOM {
            let id = format!("parent-{n:02}");
            state
                .child_relations
                .insert(id.clone(), BTreeSet::from(["orphan-0000".to_string()]));
            let (begun, can_start) = state.begin_stored(id.clone());
            assert!(can_start);
            let (next, _) = begun.end_stored(id, Vec::new()).unwrap();
            state = next;
        }
        assert_eq!(
            state.blocks_st.len(),
            MAX_PENDING_BLOCKS + MAX_DEPENDENCY_HEADROOM
        );
        let needed = "one-more-parent".to_string();
        state
            .child_relations
            .insert(needed.clone(), BTreeSet::from(["orphan-0000".to_string()]));
        let (begun, _) = state.begin_stored(needed.clone());
        assert!(begun.end_stored(needed, Vec::new()).is_err());
        let unrelated = "unrelated".to_string();
        let (begun, _) = state.begin_stored(unrelated.clone());
        assert!(begun.end_stored(unrelated, Vec::new()).is_err());
    }

    #[test]
    fn capacity_refusal_does_not_poison_the_hash_for_later_retry() {
        let mut state = BlockReceiverState::<MId>::new();
        for n in 0..MAX_PENDING_BLOCKS {
            let id = format!("ready-{n:04}");
            state.blocks_st.insert(id.clone(), BTreeSet::new());
            state.receive_st.insert(id, RecvStatus::PendingValidation);
        }
        let hash = "new-head".to_string();
        let (begun, accepted) = state.begin_stored(hash.clone());
        assert!(accepted);
        assert!(begun.end_stored(hash.clone(), Vec::new()).is_err());
        let recovered = begun.abort_stored(&hash);
        assert!(!recovered.receive_st.contains_key(&hash));
        assert!(recovered.begin_stored(hash).1);
    }

    #[test]
    fn end_stored_errors_if_not_begin_store_block() {
        let (st, _) = BlockReceiverState::<MId>::new().begin_stored("A1".to_string());
        let (new_st, _) = st.end_stored("A1".to_string(), Vec::new()).unwrap();
        // A1 is now EndStoreBlock but should be BeginStoreBlock.
        assert!(new_st.end_stored("A1".to_string(), Vec::new()).is_err());
    }

    #[test]
    fn end_stored_updates_state_and_child_relations() {
        let (st, _) = BlockReceiverState::<MId>::new().begin_stored("A2".to_string());
        let (new_st, unseen_parents) = st
            .end_stored("A2".to_string(), parents(&[("A1", true)]))
            .unwrap();

        assert_eq!(st.receive_st.get("A2"), Some(&RecvStatus::BeginStoreBlock));
        assert_eq!(
            new_st.receive_st.get("A2"),
            Some(&RecvStatus::EndStoreBlock)
        );

        assert!(!st.receive_st.contains_key("A1"));
        assert_eq!(new_st.receive_st.get("A1"), Some(&RecvStatus::Requested));
        assert_eq!(unseen_parents, BTreeSet::from(["A1".to_string()]));

        assert_eq!(
            new_st.blocks_st,
            BTreeMap::from([("A2".to_string(), BTreeSet::from(["A1".to_string()]))])
        );
        assert_eq!(
            new_st.child_relations,
            BTreeMap::from([("A1".to_string(), BTreeSet::from(["A2".to_string()]))])
        );
    }

    #[test]
    fn finished_errors_if_block_not_in_state() {
        assert!(BlockReceiverState::<MId>::new()
            .finished("A1".to_string(), BTreeSet::new())
            .is_err());
    }

    /// A refused-before-end block has no `blocks_st` entry; releasing it must be a no-op, while a
    /// refused-but-inserted block still releases its waiting child.
    #[test]
    fn finished_if_present_releases_a_waiting_child_and_ignores_an_unknown_block() {
        let state = BlockReceiverState::<MId>::new();
        let (next, released) = state
            .finished_if_present("unknown".to_string(), BTreeSet::new())
            .unwrap();
        assert_eq!(next, state);
        assert!(released.is_empty());

        let (parent, _) = state.begin_stored("parent".to_string());
        let (parent, _) = parent.end_stored("parent".to_string(), Vec::new()).unwrap();
        let (child, _) = parent.begin_stored("child".to_string());
        let (child, _) = child
            .end_stored("child".to_string(), parents(&[("parent", true)]))
            .unwrap();
        let (after, released) = child
            .finished_if_present("parent".to_string(), BTreeSet::new())
            .unwrap();
        assert_eq!(released, BTreeSet::from(["child".to_string()]));
        assert_eq!(
            after.receive_st.get("child"),
            Some(&RecvStatus::PendingValidation)
        );
    }

    #[test]
    fn finished_errors_if_block_not_received() {
        let (st, _) = BlockReceiverState::<MId>::new().begin_stored("A1".to_string());
        assert!(st.finished("A1".to_string(), BTreeSet::new()).is_err());
    }

    #[test]
    fn finished_returns_empty_state_if_all_processed() {
        let (st1, _) = BlockReceiverState::<MId>::new().begin_stored("A1".to_string());
        let (st2, _) = st1.end_stored("A1".to_string(), Vec::new()).unwrap();
        // A1 has no dependencies; finishing it removes it from the state.
        let (st3, _) = st2.finished("A1".to_string(), BTreeSet::new()).unwrap();
        assert!(st3.blocks_st.is_empty());
        assert!(st3.receive_st.is_empty());
        assert!(st3.child_relations.is_empty());
    }

    /// **The falsifier for #223.** A block whose two parents are stored but not yet validated — what a
    /// rejoining validator receives in a burst — must not be released when the *first* parent finishes: its
    /// validation would read the second from the DAG, find nothing, and drop it for good. On the
    /// not-stored dependency set it was released at once; on the not-validated set it waits for both.
    #[test]
    fn a_block_waits_for_every_parent_not_yet_validated() {
        let st = BlockReceiverState::<MId>::new();
        let mut st = st;
        for p in ["P1", "P2"] {
            let (s, _) = st.begin_stored(p.to_string());
            let (s, _) = s
                .end_stored_awaiting(p.to_string(), Vec::new(), BTreeSet::new())
                .unwrap();
            st = s;
        }
        let (st, _) = st.begin_stored("C".to_string());
        let both = BTreeSet::from(["P1".to_string(), "P2".to_string()]);
        let (st, unseen) = st
            .end_stored_awaiting(
                "C".to_string(),
                parents(&[("P1", false), ("P2", false)]),
                both,
            )
            .unwrap();
        assert!(
            unseen.is_empty(),
            "both parents are stored, so nothing is requested"
        );

        let (st, next) = st.finished("P1".to_string(), BTreeSet::new()).unwrap();
        assert!(
            next.is_empty(),
            "C was released with P2 still unvalidated: {next:?}"
        );
        let (_, next) = st.finished("P2".to_string(), BTreeSet::new()).unwrap();
        assert_eq!(next, BTreeSet::from(["C".to_string()]));
    }

    /// The control: on the old dependency set (only the parents missing from the store), the same sequence
    /// releases C when P1 finishes — the order #223's two `missing justification` refusals came from.
    #[test]
    fn on_the_not_stored_set_the_first_parent_releases_the_block() {
        let mut st = BlockReceiverState::<MId>::new();
        for p in ["P1", "P2"] {
            let (s, _) = st.begin_stored(p.to_string());
            let (s, _) = s.end_stored(p.to_string(), Vec::new()).unwrap();
            st = s;
        }
        let (st, _) = st.begin_stored("C".to_string());
        let (st, _) = st
            .end_stored("C".to_string(), parents(&[("P1", false), ("P2", false)]))
            .unwrap();
        let (_, next) = st.finished("P1".to_string(), BTreeSet::new()).unwrap();
        assert_eq!(next, BTreeSet::from(["C".to_string()]));
    }

    #[test]
    fn finished_removes_resolved_deps_and_returns_next() {
        let (st1, _) = BlockReceiverState::<MId>::new().begin_stored("A2".to_string());
        assert_eq!(st1.receive_st.get("A2"), Some(&RecvStatus::BeginStoreBlock));

        let (st2, a2_unseen) = st1
            .end_stored("A2".to_string(), parents(&[("A1", true)]))
            .unwrap();
        assert_eq!(
            st2.blocks_st.get("A2"),
            Some(&BTreeSet::from(["A1".to_string()]))
        );
        assert_eq!(st2.receive_st.get("A2"), Some(&RecvStatus::EndStoreBlock));
        assert_eq!(st2.receive_st.get("A1"), Some(&RecvStatus::Requested));
        assert_eq!(
            st2.child_relations.get("A1"),
            Some(&BTreeSet::from(["A2".to_string()]))
        );
        assert_eq!(a2_unseen, BTreeSet::from(["A1".to_string()]));

        let (st3, _) = st2.begin_stored("A1".to_string());
        assert_eq!(st3.receive_st.get("A1"), Some(&RecvStatus::BeginStoreBlock));

        let (st4, a1_unseen) = st3.end_stored("A1".to_string(), Vec::new()).unwrap();
        assert_eq!(st4.blocks_st.get("A1"), Some(&BTreeSet::new()));
        assert_eq!(st4.receive_st.get("A1"), Some(&RecvStatus::EndStoreBlock));
        assert!(a1_unseen.is_empty());

        // Finishing A1 removes it from receive state; child A2 becomes PendingValidation.
        let (st5, deps_validated) = st4.finished("A1".to_string(), BTreeSet::new()).unwrap();
        assert!(!st5.receive_st.contains_key("A1"));
        assert_eq!(
            st5.receive_st.get("A2"),
            Some(&RecvStatus::PendingValidation)
        );
        assert_eq!(deps_validated, BTreeSet::from(["A2".to_string()]));
    }
}
