//! Block processing (port of `blocks/BlockProcessor.scala`).

use rchain_shared::chan;
use std::sync::Arc;

use rchain_block_storage::block_store::BlockStore;
use rchain_block_storage::dag::dag_storage::BlockDagStorage;
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_models::block::state_hash::StateHash;
use rchain_models::block_hash::BlockHash;
use rchain_models::casper::protocol::casper_message::BlockMessage;
use rchain_shared::log::{Log, LogSource};
use rchain_shared::refined::BlockHeight;
use tokio::sync::mpsc;
use tokio::task::{JoinError, JoinSet};

use crate::block_status::BlockStatus;
use crate::interpreter_util::ShardHealth;
use crate::merging::BlockIndex;
use crate::multi_parent_casper::ValidateError;
use crate::protocol::comm_util::CommUtil;
use crate::runtime_manager::RuntimeManager;

/// Cap on the number of blocks validated concurrently (per batch). The receiver only emits
/// dependency-free blocks, so a batch of siblings is mutually independent and replayable in parallel.
fn max_parallel_block_validation() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

/// Bound pending hash-only announcements independently of replay validation.
/// An unavailable peer may consume the transport timeout for every announcement;
/// awaiting that timeout after every DAG insert serializes the whole receiver.
const MAX_PENDING_HASH_ANNOUNCEMENTS: usize = 32;

struct BlockHashAnnouncements {
    tasks: JoinSet<()>,
    limit: usize,
    log: Arc<dyn Log>,
}

impl BlockHashAnnouncements {
    fn new(log: Arc<dyn Log>) -> Self {
        Self {
            tasks: JoinSet::new(),
            limit: MAX_PENDING_HASH_ANNOUNCEMENTS,
            log,
        }
    }

    fn report(&self, result: Result<(), JoinError>) {
        if let Err(error) = result {
            self.log.error(
                LogSource::new("casper.blocks.BlockProcessor"),
                &format!("block hash announcement task failed: {error}"),
            );
        }
    }

    async fn enqueue<F>(&mut self, announcement: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        while let Some(result) = self.tasks.try_join_next() {
            self.report(result);
        }
        while self.tasks.len() >= self.limit {
            if let Some(result) = self.tasks.join_next().await {
                self.report(result);
            }
        }
        self.tasks.spawn(announcement);
    }

    async fn finish(&mut self) {
        while let Some(result) = self.tasks.join_next().await {
            self.report(result);
        }
    }
}

/// **Publish an accepted block's post-state as `CURRENT_ROOT`** (law 80, B40, HAZOP F-U17-10).
///
/// Only a block at the DAG's top publishes: `latest_block_number()` is one past the highest height,
/// so `block_num + 1 == latest` is the tip (a same-height sibling is also accepted; either
/// post-state is a state an accepted block carried). An older acceptance does not move the pointer
/// backwards.
///
/// A refusal (`Ok(false)`: no checkpoint recorded the root) or a store error is **logged, not
/// returned** — the insert has already succeeded, and the boot reconciliation republishes the latest
/// accepted root on the next start.
pub(crate) async fn publish_accepted_root(
    dag: &dyn BlockDagStorage,
    runtime: &RuntimeManager,
    block_num: BlockHeight,
    post_state: StateHash,
    log: &Arc<dyn Log>,
) {
    let latest = dag.get_representation().await.latest_block_number();
    if i64::from(block_num) + 1 < latest {
        return;
    }
    let root = Blake2b256Hash::from_byte_array(post_state.as_bytes());
    let source = LogSource::new("casper.block-processor");
    match runtime.publish_accepted_root(root).await {
        Ok(true) => {}
        Ok(false) => log.warn(
            source,
            &format!(
                "accepted block at height {block_num} names a post-state no checkpoint recorded; \
                 CURRENT_ROOT stays where it was (law 80)"
            ),
        ),
        Err(e) => log.warn(
            source,
            &format!(
                "could not publish the accepted root at height {block_num}: {e}; the boot \
                 reconciliation retries"
            ),
        ),
    }
}

/// Validate a block and insert it into the DAG (port of `validateAndAddToDag`).
pub async fn validate_and_add_to_dag<F, Fut>(
    dag: &dyn BlockDagStorage,
    block_store: &BlockStore,
    runtime: &RuntimeManager,
    block: BlockMessage,
    shard_id: &str,
    min_phlo_price: i64,
    max_number_of_parents: i32,
    block_index: &F,
    log: &Arc<dyn Log>,
    health: &ShardHealth,
) -> Result<Result<(), BlockStatus>, String>
where
    F: Fn(BlockHash) -> Fut,
    Fut: std::future::Future<Output = Result<Arc<BlockIndex>, String>>,
{
    let result = crate::multi_parent_casper::validate(
        dag,
        block_store,
        runtime,
        &block,
        shard_id,
        min_phlo_price,
        max_number_of_parents,
        block_index,
        log,
        health,
    )
    .await;
    let (block_meta, status) = match result {
        Ok(meta) => (meta, Ok(())),
        Err(ValidateError::ValidationFailed(meta, status)) => (meta, Err(status)),
        Err(ValidateError::SelfEquivocation) => {
            return Err("self-equivocation is a proposer-only outcome".to_string())
        }
        Err(ValidateError::Internal(e)) => return Err(e),
    };
    let accepted_post = block.post_state_hash;
    let accepted_height = block_meta.block_num;
    dag.insert(block_meta, block).await?;
    // **Only an accepted block publishes its state** (law 80, B40). A `ValidationFailed` block is
    // inserted into the DAG with its failure recorded, but its post-state is not the node's state and
    // must not become `CURRENT_ROOT`.
    if status.is_ok() {
        publish_accepted_root(dag, runtime, accepted_height, accepted_post, log).await;
    }
    Ok(status)
}

// **Why nothing here defers a `missing justification` refusal** (AUDIT C211, #223).
//
// This loop used to hold a bounded list of blocks refused for a justification that had not arrived,
// retrying them with the next delivered batch. It was removed on 2026-10-04, after C211's cause was
// fixed at its source, because the refusal it protected against **cannot reach this loop at all**:
//
// * the **only** producer into this processor is `pump_validated_blocks`, fed by the receiver's
//   `out_tx` (`node/src/runtime/node_runtime.rs:852-863`), and `out_tx` is sent a hash **only when
//   that block's dependency set is empty** — `block_receiver.rs`'s `has_all_deps` and `finished`'s
//   release set, both now computed as "every justification is in the DAG", under the receiver's lock
//   (`end_stored_awaiting`, #223's fix). The receiver's *other* outlet, `send_to_validate`'s
//   `put_to_incoming_queue`, goes back into the **receiver** (`incoming_blocks_tx`), not here.
// * the DAG index those two read only ever grows, and `BlockMetadataStore::add` writes the **store
//   first** and the index second (`block_metadata_store.rs:57-70`, AUDIT C172), so
//   `index contains j ⟹ store contains j` for every `j`.
// * `validate::block_number` / `sequence_number` read the **store** (`dag.lookup`), so a released
//   block's every justification is readable and the summary cannot fail this way.
//
// Measured too: the two `fixed-run1`/`fixed-run2` rejoins in `spec/audit/evidence/n223-rejoin-blocks/`
// record **zero** refusals across a full rejoin burst. A deferral here would therefore be dead code —
// and not inert if it ever did fire: a parked block rides **every** subsequent batch, so an
// unresolvable one would be replayed up to the cap on each delivery, for ever.

/// Process incoming blocks: validate a batch concurrently, insert serially in topological order,
/// notify the validated queue, and broadcast the block hash (port of `BlockProcessor.apply`).
pub async fn apply<F, Fut>(
    mut input_blocks: mpsc::Receiver<BlockMessage>,
    validated_tx: mpsc::Sender<BlockMessage>,
    shard_id: String,
    min_phlo_price: i64,
    max_number_of_parents: i32,
    dag: Arc<dyn BlockDagStorage>,
    block_store: BlockStore,
    runtime: Arc<RuntimeManager>,
    comm_util: Arc<CommUtil>,
    block_index: F,
    log: Arc<dyn Log>,
    health: ShardHealth,
) where
    F: Fn(BlockHash) -> Fut + Clone + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Arc<BlockIndex>, String>> + Send + 'static,
{
    let source = LogSource::new("casper.blocks.BlockProcessor");
    let mut announcements = BlockHashAnnouncements::new(log.clone());
    while let Some(first) = input_blocks.recv().await {
        // Drain a batch of dependency-free blocks, bounded by the concurrency cap.
        let mut batch = vec![first];
        while batch.len() < max_parallel_block_validation() {
            match input_blocks.try_recv() {
                Ok(block) => batch.push(block),
                Err(_) => break,
            }
        }

        // Validate each block concurrently. Validation is verify-only (replay) and forks its own
        // replay runtime per block, so blocks in the batch are independent.
        let mut handles = Vec::with_capacity(batch.len());
        for block in &batch {
            let dag = dag.clone();
            let block_store = block_store.clone();
            let runtime = runtime.clone();
            let shard_id = shard_id.clone();
            let block_index = block_index.clone();
            let block = block.clone();
            let log = log.clone();
            let health = health.clone();
            handles.push(tokio::spawn(async move {
                crate::multi_parent_casper::validate(
                    dag.as_ref(),
                    &block_store,
                    runtime.as_ref(),
                    &block,
                    &shard_id,
                    min_phlo_price,
                    max_number_of_parents,
                    &block_index,
                    &log,
                    &health,
                )
                .await
            }));
        }

        // Insert serially in drained order (a valid topological order: parents are emitted before
        // children), then forward/broadcast only blocks that validated successfully.
        for (block, handle) in batch.into_iter().zip(handles) {
            let result = match handle.await {
                Ok(r) => r,
                Err(e) => {
                    log.error(source, &format!("validator task panicked: {e}"));
                    continue;
                }
            };
            let (block_meta, status) = match result {
                Ok(meta) => (meta, Ok(())),
                Err(ValidateError::ValidationFailed(meta, status)) => (meta, Err(status)),
                Err(ValidateError::SelfEquivocation) => {
                    log.error(
                        source,
                        &format!(
                            "Block {} processing error: self-equivocation is a proposer-only outcome",
                            block.block_hash.to_hex()
                        ),
                    );
                    continue;
                }
                Err(ValidateError::Internal(e)) => {
                    // An `Internal` failure is a store or runtime failure this loop has no answer for,
                    // so the block is dropped and the error is logged. **A `missing justification`
                    // cannot arrive here** — see the note on `apply` — so there is nothing to defer.
                    log.error(
                        source,
                        &format!("Block {} processing error: {e}", block.block_hash.to_hex()),
                    );
                    continue;
                }
            };
            if let Err(e) = dag.insert(block_meta, block.clone()).await {
                log.error(
                    source,
                    &format!("Block {} insert error: {e}", block.block_hash.to_hex()),
                );
                continue;
            }
            match status {
                Ok(()) => {
                    // **Awaiting, so a full queue backpressures this processor instead of growing**
                    // (C175). The queue is what the block receiver consumes to advance the round/fringe
                    // state, and the producer here is CPU-bound replay validation — exactly the shape R15
                    // bounded on the other half of this pipeline. `let _ =` stays: a send fails only when
                    // every receiver is gone, which is shutdown, and the block is already in the DAG.
                    chan::send(&validated_tx, block.clone()).await;
                    // Keep the completion notification and DAG insertion on the
                    // acceptance path. Network timeouts belong to a separate,
                    // bounded set of hash-only tasks, not to the next replay.
                    let comm = comm_util.clone();
                    let hash = block.block_hash;
                    let creator = block.sender.as_bytes().to_vec();
                    announcements
                        .enqueue(async move {
                            comm.send_block_hash(&hash, &creator).await;
                        })
                        .await;
                }
                // **The `Display` sentence, not the `Debug` variant name** (#139). `{status:?}` printed
                // `InvalidStateHash` and nothing else, so an operator could not tell a pre-state
                // disagreement from a post-state one — the two are now different statuses — nor read
                // what either means. The hash-carrying line is the interpreter's
                // (`casper.interpreter.validate`); this one says which block and who sent it.
                Err(status) => log.warn(
                    source,
                    &format!(
                        "Block #{} {} from {} failed validation: {status}",
                        block.block_number,
                        block.block_hash.to_hex(),
                        rchain_shared::base16::encode(&block.sender.as_bytes()[..8])
                    ),
                ),
            }
        }
    }
    announcements.finish().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_stalled_hash_announcement_does_not_hold_the_next_one() {
        let mut announcements = BlockHashAnnouncements::new(Arc::new(rchain_shared::log::NopLog));
        let (release, held) = tokio::sync::oneshot::channel();
        announcements
            .enqueue(async move {
                held.await.unwrap();
            })
            .await;
        let (accepted, observed) = tokio::sync::oneshot::channel();
        announcements
            .enqueue(async move {
                accepted.send(()).unwrap();
            })
            .await;

        tokio::time::timeout(std::time::Duration::from_secs(1), observed)
            .await
            .expect("a stalled peer must not serialize later announcements")
            .unwrap();
        release.send(()).unwrap();
        announcements.finish().await;
    }

    #[tokio::test]
    async fn hash_announcement_capacity_backpressures_and_shutdown_drains() {
        let mut announcements = BlockHashAnnouncements::new(Arc::new(rchain_shared::log::NopLog));
        announcements.limit = 2;
        let mut releases = Vec::new();
        for _ in 0..2 {
            let (release, held) = tokio::sync::oneshot::channel();
            releases.push(release);
            announcements
                .enqueue(async move {
                    held.await.unwrap();
                })
                .await;
        }
        let (accepted, observed) = tokio::sync::oneshot::channel();
        let mut next = Box::pin(announcements.enqueue(async move {
            accepted.send(()).unwrap();
        }));
        std::future::poll_fn(|cx| {
            assert!(std::future::Future::poll(next.as_mut(), cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        releases.remove(0).send(()).unwrap();
        next.await;
        releases.remove(0).send(()).unwrap();
        announcements.finish().await;
        observed.await.unwrap();
        assert!(announcements.tasks.is_empty());
    }

    /// The concurrency cap is never zero: a zero would mean no block is ever validated concurrently —
    /// or, if it were used as a batch size, that a batch of blocks is never processed at all. The
    /// host fallback is the only arm that can go wrong here, and it cannot return zero.
    #[test]
    fn the_parallel_validation_cap_is_never_zero() {
        let cap = max_parallel_block_validation();
        assert!(cap >= 1, "a zero cap would stall block validation: {cap}");
        assert_eq!(
            cap,
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4),
            "the cap follows the host's parallelism"
        );
    }
}
