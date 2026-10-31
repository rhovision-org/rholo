//! Block processing (port of `blocks/BlockProcessor.scala`).

use rchain_shared::chan;
use std::collections::BTreeSet;
use std::sync::Arc;

use rchain_block_storage::block_store::BlockStore;
use rchain_block_storage::dag::dag_storage::BlockDagStorage;
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_models::block::state_hash::StateHash;
use rchain_models::block_hash::BlockHash;
use rchain_models::casper::protocol::casper_message::BlockMessage;
use rchain_shared::lock::Unpoison;
use rchain_shared::log::{Log, LogSource};
use rchain_shared::refined::BlockHeight;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::{JoinError, JoinHandle, JoinSet};

use crate::block_disposition::{BlockDispositionStore, MAX_FAULT_RETRIES};
use crate::interpreter_util::ShardHealth;
use crate::merging::BlockIndex;
use crate::multi_parent_casper::ValidateError;
use crate::protocol::comm_util::CommUtil;
use crate::runtime_manager::RuntimeManager;

/// **How many received blocks this process has dropped before a DAG entry**, and the most recent
/// one. The live private-chain incident had no counter for this state: the only trace was one
/// `log.error`, and the node's surfaces still reported a healthy height. The process-wide cell is
/// the P0 loudness half; the disposition ledger (U19) is the durable per-shard half.
static PIPELINE_FAULTS: AtomicU64 = AtomicU64::new(0);
static LAST_PIPELINE_FAULT: Mutex<Option<String>> = Mutex::new(None);

pub fn pipeline_faults() -> u64 {
    PIPELINE_FAULTS.load(Ordering::Relaxed)
}

pub fn last_pipeline_fault() -> Option<String> {
    LAST_PIPELINE_FAULT.lock().unpoison().clone()
}

pub(crate) fn note_pipeline_fault(block: &BlockMessage, stage: &str, error: &str) {
    PIPELINE_FAULTS.fetch_add(1, Ordering::Relaxed);
    *LAST_PIPELINE_FAULT.lock().unpoison() = Some(format!(
        "{} at {}: {error}",
        block.block_hash.to_hex(),
        stage
    ));
}

/// **The deadline on one block's validation** (HAZOP F-U19-09, RCA S2/S7).
///
/// The processor awaits each validation future serially. A replay that stops making progress — a
/// stalled store read, a detached reducer task, a merge search that never returns — therefore holds
/// every later block behind it, and the node's height stands still with no fault anywhere. There was
/// no deadline at all; this is the bound.
///
/// The value is policy, not a measured optimum: a block at the largest deploy count on a slow host
/// can legitimately take minutes, and a false timeout is a fault on a healthy node. Ten minutes is
/// deliberately far above any observed validation on the testnet; an operator can only shorten the
/// wedge by restarting, never lengthen this without a rebuild. Aborting is **not** assumed to leave a
/// clean replay: the aborted task may have written checkpoints or roots, so the timeout arms the
/// same `pipelineFaults` latch that withholds proposals and deploy admission and requires an operator
/// restart or resume. The alternative — waiting for ever behind a silent task — is the failure this
/// bound exists to refuse.
pub const BLOCK_VALIDATION_TIMEOUT: Duration = Duration::from_secs(600);

/// Await one validation task, or fault the pipeline and abort it at the deadline.
///
/// Returns `None` on timeout (the caller skips the block); `Some(joined)` otherwise, leaving the
/// panic arm to the caller so both failure shapes are recorded the same way.
async fn validation_within_deadline<T: Send + 'static>(
    mut handle: JoinHandle<T>,
    timeout: Duration,
    block: &BlockMessage,
    log: &Arc<dyn Log>,
    source: LogSource,
) -> Option<Result<T, JoinError>> {
    match tokio::time::timeout(timeout, &mut handle).await {
        Ok(joined) => Some(joined),
        Err(_) => {
            // **Abort before returning, not instead of it.** Dropping the handle would leave the task
            // running detached and silent — the very shape this bound exists to make impossible. The
            // caller records the fault (process cell + durable ledger).
            handle.abort();
            log.error(
                source,
                &format!(
                    "Block #{} {} validation did not return within {timeout:?}; the replay task \
                     was aborted and the pipeline is faulted. Restart or resume the node only after \
                     deciding whether the aborted replay left partial runtime state",
                    block.block_number,
                    block.block_hash.to_hex()
                ),
            );
            None
        }
    }
}

/// Record a retryable node fault in the durable ledger **and** the process-wide fault cell.
///
/// Both are needed and neither substitutes for the other: the ledger is what survives a restart and
/// what the retry loop reads; the cell is what `/health` and the propose/deploy gates read now. A
/// ledger write that fails is itself logged as a fault, because the alternative is a stored body
/// with no record — the orphan state U19 exists to forbid.
/// **The stage a validation `Internal` failure is recorded under** (Law 88d). The height and
/// sequence checks produce `missing justification <hex>` for a justification the node cannot
/// resolve; that failure is named, so an operator (and the retry sweep) can tell it from a store or
/// runtime fault. The verdict is unchanged — the block still fails and is still retried.
fn internal_fault_stage(reason: &str) -> &'static str {
    if reason.starts_with("missing justification") {
        "missing justification"
    } else {
        "validation internal"
    }
}

async fn record_block_fault(
    disposition: &BlockDispositionStore,
    block: &BlockMessage,
    stage: &str,
    reason: &str,
    log: &Arc<dyn Log>,
    source: LogSource,
) {
    note_pipeline_fault(block, stage, reason);
    match disposition
        .mark_faulted(block.block_hash, block.block_number, stage, reason)
        .await
    {
        Ok(attempts) if attempts >= MAX_FAULT_RETRIES => log.error(
            source,
            &format!(
                "Block #{} {} faulted at {stage} ({reason}); the retry budget is spent after \
                 {attempts} attempts and the block is quarantined until an operator reset",
                block.block_number,
                block.block_hash.to_hex()
            ),
        ),
        Ok(attempts) => log.error(
            source,
            &format!(
                "Block #{} {} faulted at {stage} ({reason}); recorded for retry {attempts}/{MAX_FAULT_RETRIES}",
                block.block_number,
                block.block_hash.to_hex()
            ),
        ),
        Err(e) => log.error(
            source,
            &format!(
                "Block #{} {} faulted at {stage} ({reason}) but the disposition could not be \
                 recorded: {e}; if the node restarts before the next attempt the body may be \
                 orphaned",
                block.block_number,
                block.block_hash.to_hex()
            ),
        ),
    }
}

/// A fault no retry can fix is parked with the budget already spent; it stays loud and only an
/// operator clears it.
async fn record_terminal_fault(
    disposition: &BlockDispositionStore,
    block: &BlockMessage,
    stage: &str,
    reason: &str,
    log: &Arc<dyn Log>,
    source: LogSource,
) {
    note_pipeline_fault(block, stage, reason);
    if let Err(e) = disposition
        .mark_terminal_fault(block.block_hash, block.block_number, stage, reason)
        .await
    {
        log.error(
            source,
            &format!(
                "Block {} terminal fault could not be recorded: {e}",
                block.block_hash.to_hex()
            ),
        );
    }
}

/// **A verdict settles the obligation.** Accepted and refused blocks are both DAG metadata now; the
/// ledger row is deleted. A failed delete is not fatal — the next start reads the DAG first and
/// settles a row whose block is already inserted — but it is logged because the row would otherwise
/// look like an open obligation.
async fn settle_block(
    disposition: &BlockDispositionStore,
    block: &BlockMessage,
    log: &Arc<dyn Log>,
    source: LogSource,
) {
    if let Err(e) = disposition.settle(block.block_hash).await {
        log.warn(
            source,
            &format!(
                "could not settle the disposition of block {} after its verdict: {e}; the next \
                 start reconciles it from the DAG",
                block.block_hash.to_hex()
            ),
        );
    }
}

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

/// The receiver and disposition sweeps may offer the same stored body concurrently.
/// Keep exact hash duplicates out of replay; different hashes must still reach
/// validation even when they share a sender and sequence number.
fn unique_received_blocks(batch: Vec<BlockMessage>) -> Vec<BlockMessage> {
    let mut seen = BTreeSet::new();
    batch
        .into_iter()
        .filter(|block| seen.insert(block.block_hash))
        .collect()
}

/// Process incoming blocks: validate a batch concurrently, insert serially in topological order,
/// notify the validated queue, and broadcast the block hash (port of `BlockProcessor.apply`).
pub async fn apply<F, Fut>(
    mut input_blocks: mpsc::Receiver<BlockMessage>,
    validated_tx: mpsc::Sender<BlockMessage>,
    released_tx: mpsc::Sender<BlockMessage>,
    shard_id: String,
    min_phlo_price: i64,
    max_number_of_parents: i32,
    dag: Arc<dyn BlockDagStorage>,
    block_store: BlockStore,
    disposition: BlockDispositionStore,
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

        // A disposition sweep can race the receiver's handoff. Deduplicate within
        // this batch, then discard deliveries committed by an earlier batch. All
        // validation tasks in the previous batch have finished before this point.
        let mut uninserted = Vec::new();
        for block in unique_received_blocks(batch) {
            if dag.get_representation().await.contains(&block.block_hash) {
                settle_block(&disposition, &block, &log, source).await;
                // A late receiver handoff may still hold dependency state.
                // Release that state without replay, rebroadcast or proposer taps.
                chan::send(&released_tx, block).await;
                continue;
            }
            uninserted.push(block);
        }
        let batch = uninserted;

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
            let Some(joined) =
                validation_within_deadline(handle, BLOCK_VALIDATION_TIMEOUT, &block, &log, source)
                    .await
            else {
                record_block_fault(
                    &disposition,
                    &block,
                    "validation timeout",
                    "no verdict within the deadline",
                    &log,
                    source,
                )
                .await;
                continue;
            };
            let result = match joined {
                Ok(r) => r,
                Err(e) => {
                    record_block_fault(
                        &disposition,
                        &block,
                        "validator task panicked",
                        &e.to_string(),
                        &log,
                        source,
                    )
                    .await;
                    continue;
                }
            };
            let (block_meta, status) = match result {
                Ok(meta) => (meta, Ok(())),
                Err(ValidateError::ValidationFailed(meta, status)) => (meta, Err(status)),
                Err(ValidateError::SelfEquivocation) => {
                    record_terminal_fault(
                        &disposition,
                        &block,
                        "self-equivocation",
                        "a received block cannot self-equivocate; retry cannot change this",
                        &log,
                        source,
                    )
                    .await;
                    continue;
                }
                Err(ValidateError::Internal(e)) => {
                    // An `Internal` failure is retryable by the disposition ledger's schedule, not
                    // droppable. **A `missing justification` should not arrive here** — the
                    // receiver releases a block only when every parent is in the DAG — but if one
                    // does, it is named for what it is rather than folded into "validation
                    // internal", where it is indistinguishable from a store or runtime fault
                    // (Law 88d).
                    record_block_fault(
                        &disposition,
                        &block,
                        internal_fault_stage(&e),
                        &e,
                        &log,
                        source,
                    )
                    .await;
                    continue;
                }
            };
            if let Err(e) = dag.insert(block_meta, block.clone()).await {
                record_block_fault(&disposition, &block, "dag insert", &e, &log, source).await;
                continue;
            }
            // The insert committed, so either verdict is a DAG record; the durable obligation is
            // discharged before the notification below.
            settle_block(&disposition, &block, &log, source).await;
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
                Err(status) => {
                    // **A failed insert is still a DAG entry and must release its waiting
                    // children.** Only the accepted arm uses `validated_tx` (and its taps);
                    // this separate stream feeds the receiver's `finished_if_present` without
                    // proposing or attesting to a refused block.
                    chan::send(&released_tx, block.clone()).await;
                    log.warn(
                        source,
                        &format!(
                            "Block #{} {} from {} failed validation: {status}",
                            block.block_number,
                            block.block_hash.to_hex(),
                            rchain_shared::base16::encode(&block.sender.as_bytes()[..8])
                        ),
                    )
                }
            }
        }
    }
    announcements.finish().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A missing justification is recorded under its own stage** (Law 88d): not "validation
    /// internal", where it would be indistinguishable from a store or runtime fault.
    #[test]
    fn a_missing_justification_is_recorded_under_its_own_stage() {
        assert_eq!(
            internal_fault_stage("missing justification abcd"),
            "missing justification"
        );
        assert_eq!(
            internal_fault_stage("block summary failed: store read"),
            "validation internal"
        );
    }

    /// A minimal block for the fault/deadline tests. `block-storage`'s shared fixture is
    /// `#[cfg(test)]` inside that crate, so it is not reachable from here (CI caught exactly that).
    fn sample_block() -> BlockMessage {
        use rchain_models::block::state_hash::StateHash;
        use rchain_models::casper::protocol::casper_message::RholangState;
        use rchain_models::validator::Validator;
        use rchain_shared::refined::SeqNum;
        use std::collections::{BTreeMap, BTreeSet};

        BlockMessage {
            version: 1,
            shard_id: "root".to_string(),
            block_hash: BlockHash::new([0xab; 32]),
            block_number: BlockHeight::zero(),
            sender: Validator::new([0x11; 65]),
            seq_num: SeqNum::zero(),
            pre_state_hash: StateHash::new([1u8; 32]),
            post_state_hash: StateHash::new([2u8; 32]),
            justifications: vec![],
            bonds: BTreeMap::new(),
            rejected_deploys: BTreeSet::new(),
            rejected_blocks: BTreeSet::new(),
            rejected_senders: BTreeSet::new(),
            state: RholangState {
                deploys: vec![],
                system_deploys: vec![],
            },
            sig_algorithm: "secp256k1".to_string(),
            sig: vec![1],
            timestamp: 0,
        }
    }

    #[test]
    fn concurrent_receiver_and_retry_deliveries_are_replayed_once() {
        let first = sample_block();
        let mut sibling = first.clone();
        sibling.block_hash = BlockHash::new([0xcd; 32]);
        let selected = unique_received_blocks(vec![
            first.clone(),
            first.clone(),
            sibling.clone(),
            first.clone(),
            sibling.clone(),
        ]);
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].block_hash, first.block_hash);
        assert_eq!(selected[1].block_hash, sibling.block_hash);
        // A conflicting block at the same sender/sequence is not an exact
        // duplicate. It must still pass through the existing equivocation checks.
        assert_eq!(selected[0].sender, selected[1].sender);
        assert_eq!(selected[0].seq_num, selected[1].seq_num);
    }

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

    /// **A validation that never returns is a fault, not a wait.** The processor used to await the
    /// task with no deadline, so one hung replay held every later block and the height stood still
    /// with nothing recorded (HAZOP F-U19-09). The deadline aborts the task and increments the
    /// process-wide fault the readiness/refusal gates read.
    #[tokio::test]
    async fn a_hung_validation_times_out_and_faults_the_pipeline() {
        use crate::block_disposition::{BlockDisposition, BlockDispositionCodec, PendingStage};
        use rchain_block_storage::dag::codecs::BlockHashCodec;
        use rchain_shared::store_manager::{database, InMemoryStoreManager};

        let block = sample_block();
        let log: Arc<dyn Log> = Arc::new(rchain_shared::log::NopLog);
        let kv = database(
            &InMemoryStoreManager::default(),
            "block-disposition",
            Arc::new(BlockHashCodec),
            Arc::new(BlockDispositionCodec),
        )
        .await
        .expect("in-memory ledger");
        let store = BlockDispositionStore::from_store(Arc::new(kv));
        store
            .mark_pending(block.block_hash, block.block_number, PendingStage::Stored)
            .await
            .expect("pending row");
        let before = pipeline_faults();
        let handle = tokio::spawn(std::future::pending::<()>());
        let outcome = validation_within_deadline(
            handle,
            Duration::from_millis(10),
            &block,
            &log,
            LogSource::new("casper.blocks.BlockProcessor"),
        )
        .await;
        assert!(
            outcome.is_none(),
            "a hung validation must not be awaited to completion"
        );
        record_block_fault(
            &store,
            &block,
            "validation timeout",
            "no verdict within the deadline",
            &log,
            LogSource::new("casper.blocks.BlockProcessor"),
        )
        .await;
        assert!(
            pipeline_faults() > before,
            "a timeout must be visible as a fault"
        );
        assert!(
            last_pipeline_fault().is_some_and(
                |f| f.contains("validation timeout") && f.contains(&block.block_hash.to_hex())
            ),
            "the fault must name the stage and the block"
        );
        assert!(matches!(
            store.get(&block.block_hash).await.expect("ledger read"),
            Some(BlockDisposition::Faulted { attempts: 1, .. })
        ));
    }

    /// The deadline is a *deadline*, not a fixed delay: a validation that returns first is
    /// unaffected.
    #[tokio::test]
    async fn a_validation_that_returns_before_the_deadline_is_delivered() {
        let block = sample_block();
        let log: Arc<dyn Log> = Arc::new(rchain_shared::log::NopLog);
        let handle = tokio::spawn(async { 7u8 });
        let outcome = validation_within_deadline(
            handle,
            Duration::from_secs(5),
            &block,
            &log,
            LogSource::new("casper.blocks.BlockProcessor"),
        )
        .await;
        assert_eq!(outcome.map(|joined| joined.unwrap()), Some(7));
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
