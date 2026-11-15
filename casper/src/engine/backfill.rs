//! **The background body backfill** (sync/verification separation).
//!
//! A joiner that accepts a checkpoint holds its **headers** only. This task walks the missing
//! heights from genesis upward, one height at a time, requests each body, content-verifies it and
//! attaches it to the DAG record. It never replays: execution is the audit's job (`rnode audit`),
//! and the body's only claim here is that it hashes to the accepted header.
//!
//! **Progress is the store's `history_from` marker**: it names the lowest height whose body is not
//! held, and it is advanced only after every block at that height is attached. A crash resumes from
//! it, and the status surface reads it (`historyAvailability`).
//!
//! **A failure is a stall, not a fallback.** A body that does not arrive is retried; the marker is
//! never advanced past a height that is not complete, and the node keeps serving from the checkpoint
//! state while the gap remains. That is the light/full distinction: the node is correct either way,
//! and the marker is how it says which it is.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rchain_block_storage::block_store::BlockStore;
use rchain_block_storage::checkpoint_store::CheckpointStore;
use rchain_block_storage::dag::dag_storage::BlockDagStorage;
use rchain_models::block_hash::BlockHash;
use rchain_models::casper::protocol::casper_message::BlockMessage;
use rchain_shared::log::{Log, LogSource};
use rchain_shared::refined::BlockHeight;

use crate::protocol::comm_util::CommUtil;

/// How long one body request may take before it is re-asked.
const BODY_TIMEOUT: Duration = Duration::from_secs(30);

/// Re-asks for one body before the height is left incomplete for this pass.
const MAX_BODY_ATTEMPTS: u32 = 3;

/// How long to wait when there is nothing to backfill (or the last pass failed). A checkpoint can
/// arrive at any time, so the task does not exit; it looks again.
const IDLE_PAUSE: Duration = Duration::from_secs(30);

/// **Whether a backfill is walking right now**, read by `/api/v1/status`. It is a gauge, not a
/// fact about the store: a node with a floor above zero and no active walk is `unavailable`, not
/// `backfilling`.
static BACKFILL_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn backfill_active() -> bool {
    BACKFILL_ACTIVE.load(Ordering::Relaxed)
}

pub struct Backfill {
    dag: Arc<dyn BlockDagStorage>,
    block_store: BlockStore,
    checkpoint_store: CheckpointStore,
    comm_util: Arc<CommUtil>,
    shard_id: String,
    log: Arc<dyn Log>,
}

/// Spawn the backfill loop for one shard. Its ingress is the channel the running engine diverts
/// bodies into when it already holds the block's metadata.
pub fn spawn(
    dag: Arc<dyn BlockDagStorage>,
    block_store: BlockStore,
    checkpoint_store: CheckpointStore,
    comm_util: Arc<CommUtil>,
    shard_id: String,
    log: Arc<dyn Log>,
    mut ingress: tokio::sync::mpsc::Receiver<BlockMessage>,
) -> tokio::task::JoinHandle<()> {
    let backfill = Backfill {
        dag,
        block_store,
        checkpoint_store,
        comm_util,
        shard_id,
        log,
    };
    tokio::spawn(async move {
        let source = LogSource::new("casper.engine.Backfill");
        loop {
            match backfill.tick(&mut ingress).await {
                Ok(true) => {}
                Ok(false) => {
                    BACKFILL_ACTIVE.store(false, Ordering::Relaxed);
                    tokio::time::sleep(IDLE_PAUSE).await;
                }
                Err(e) => {
                    BACKFILL_ACTIVE.store(false, Ordering::Relaxed);
                    backfill.log.warn(
                        source,
                        &format!(
                            "{}: body backfill paused: {e} (the history floor is unchanged; the \
                             node keeps serving from the checkpoint state)",
                            backfill.shard_id
                        ),
                    );
                    tokio::time::sleep(IDLE_PAUSE).await;
                }
            }
        }
    })
}

impl Backfill {
    /// One pass at the current floor. `Ok(true)` = progress (call again immediately);
    /// `Ok(false)` = nothing to do (or the certificate is not yet at a floor).
    async fn tick(
        &self,
        ingress: &mut tokio::sync::mpsc::Receiver<BlockMessage>,
    ) -> Result<bool, String> {
        let Some(floor) = self.checkpoint_store.history_from().await? else {
            return Ok(false);
        };
        if floor <= 0 {
            return Ok(false);
        }
        let Some(anchor_height) = self
            .checkpoint_store
            .latest()
            .await?
            .map(|certificate| certificate.checkpoint.anchor_height)
        else {
            return Ok(false);
        };
        if floor > anchor_height {
            // The floor is past the certified anchor: nothing below it is outstanding.
            self.checkpoint_store.set_history_from(0).await?;
            return Ok(true);
        }
        BACKFILL_ACTIVE.store(true, Ordering::Relaxed);

        let height = BlockHeight::try_from(floor)
            .map_err(|e| format!("history floor {floor} is not a height: {e}"))?;
        let hashes: Vec<BlockHash> = self
            .dag
            .get_representation()
            .await
            .height_map
            .get(&height)
            .map(|set| set.iter().copied().collect())
            .unwrap_or_default();

        for hash in hashes {
            let Some(meta) = self.dag.lookup(&hash).await? else {
                continue;
            };
            if self
                .block_store
                .contains(&[hash])
                .await?
                .first()
                .copied()
                .unwrap_or(false)
            {
                continue;
            }
            let body = Self::fetch(&self.comm_util, hash, ingress, &self.log).await?;
            if !crate::validate::block_hash(&body) || body.shard_id != self.shard_id {
                return Err(format!(
                    "the body served for {} is not that block on this shard",
                    hash.to_hex()
                ));
            }
            self.block_store.put(&[(hash, body.clone())]).await?;
            self.dag.attach_body(&meta, &body).await?;
            crate::engine::sync_metrics::census().add_backfilled(1);
            self.log.info(
                LogSource::new("casper.engine.Backfill"),
                &format!(
                    "{}: attached body {} at height {floor}",
                    self.shard_id,
                    hash.to_hex()
                ),
            );
        }

        let floor = next_history_floor(floor, anchor_height);
        self.checkpoint_store.set_history_from(floor).await?;
        if floor == 0 {
            BACKFILL_ACTIVE.store(false, Ordering::Relaxed);
            self.log.info(
                LogSource::new("casper.engine.Backfill"),
                &format!(
                    "{}: body backfill complete; deploy indexing covers the certified history",
                    self.shard_id
                ),
            );
        }
        Ok(true)
    }

    /// Ask for one body and wait for it on the backfill ingress. Other blocks that arrive are
    /// ignored (the running engine only diverts bodies whose metadata is known, but a stale answer
    /// for another height may still be in flight).
    async fn fetch(
        comm_util: &CommUtil,
        hash: BlockHash,
        ingress: &mut tokio::sync::mpsc::Receiver<BlockMessage>,
        log: &Arc<dyn Log>,
    ) -> Result<BlockMessage, String> {
        for attempt in 1..=MAX_BODY_ATTEMPTS {
            crate::engine::sync_metrics::census().add_backfill_request();
            // The send is bounded by the same body deadline the receive uses; a blocked transport
            // is this attempt's failure, not a stall.
            let deadline = tokio::time::Instant::now() + BODY_TIMEOUT;
            let requested = tokio::time::timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
                comm_util.request_lfs_block_from(&hash, &[]),
            )
            .await;
            if requested.is_err() {
                log.warn(
                    LogSource::new("casper.engine.Backfill"),
                    &format!(
                        "the body request for {} did not complete within {BODY_TIMEOUT:?}",
                        hash.to_hex()
                    ),
                );
                continue;
            }
            loop {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match tokio::time::timeout(remaining, ingress.recv()).await {
                    Ok(Some(block)) => {
                        if block.block_hash == hash {
                            return Ok(block);
                        }
                    }
                    Ok(None) => return Err("the backfill ingress closed".to_string()),
                    Err(_) => break,
                }
            }
            log.warn(
                LogSource::new("casper.engine.Backfill"),
                &format!(
                    "no body for {} on attempt {attempt} of {MAX_BODY_ATTEMPTS}",
                    hash.to_hex()
                ),
            );
        }
        Err(format!(
            "the body for {} was requested {MAX_BODY_ATTEMPTS} times and did not arrive",
            hash.to_hex()
        ))
    }
}

/// **The marker after one completed height.** `floor` is the height just attached; the next height
/// is one above it, and once the certified anchor is covered the history is complete and the
/// marker is `0` (full coverage). Pure so the arithmetic has a test without a node.
pub fn next_history_floor(completed: i64, anchor_height: i64) -> i64 {
    if completed >= anchor_height {
        0
    } else {
        completed.saturating_add(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_history_floor_advances_one_height_and_ends_at_zero() {
        assert_eq!(next_history_floor(1, 10), 2);
        assert_eq!(
            next_history_floor(9, 10),
            10,
            "the anchor itself is still outstanding"
        );
        assert_eq!(
            next_history_floor(10, 10),
            0,
            "the anchor completes the history"
        );
        assert_eq!(
            next_history_floor(0, 0),
            0,
            "a genesis-rooted history needs no backfill"
        );
    }

    /// The gauge is what separates `backfilling` from `unavailable` on the status surface; it starts
    /// false and is only set by a tick.
    #[test]
    fn the_backfill_gauge_is_false_until_a_tick_runs() {
        assert!(!backfill_active());
    }
}
