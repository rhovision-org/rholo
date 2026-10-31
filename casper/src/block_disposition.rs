//! **The durable disposition of a received block** (HAZOP U19, Law 88a).
//!
//! The live private-chain incident had a state the node could not represent: a block body in the
//! block store, no DAG entry, no lease, no retry and no fault. The receiver's pending set and the
//! processor's drop arms were both in memory, so a restart forgot the question and the body stayed
//! an orphan for ever (RCA §9; F-U19-01/F-U19-05/F-U19-07).
//!
//! This ledger is the durable half. It holds only the **unsettled** states:
//!
//! * [`BlockDisposition::Pending`] — the body is stored and the node still owes it an outcome:
//!   waiting on a parent, queued for validation, or scheduled for a retry. Written by the receiver
//!   as soon as the body is in the store, so a crash cannot lose the obligation.
//! * [`BlockDisposition::Faulted`] — a node fault (a panicked validator, an internal replay error,
//!   a timed-out validation, a failed DAG insert) left the block without an outcome. It carries the
//!   stage, the reason, the first and last attempt and the attempt count, so the retry loop is a
//!   query and the operator sees what happened.
//!
//! **`Accepted` and `Refused` are not stored here.** Both are DAG metadata
//! (`BlockMetadata::validated`/`validation_failed`), which is the durable record of the verdict; a
//! second row would be a second truth. Settling — either outcome — deletes the row, so the ledger
//! is bounded by the number of blocks the node currently owes an outcome, not by chain height.
//!
//! **What this cannot do, said rather than implied:** rows are written only by this build. A body
//! stored by an older binary has no row, and finding it would mean decoding the whole block store at
//! boot — the memory cost the resource work exists to avoid. The one-time legacy scan
//! (`scan_legacy_bodies`, F-U19-07) walks the store's **keys** and adopts each body the DAG does not
//! hold, without decoding a single body; it runs once per store, marker-gated, and re-runs on a
//! `LEGACY_BODY_SCAN_VERSION` bump — which is what carried the ledger across the C350 move into the
//! body's LMDB environment.
//!
//! **The body and the first row are one write** (C350): [`BodyDispositionWriter`] commits both in one
//! LMDB transaction, which is why the ledger shares the `blockstorage` environment.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rchain_block_storage::block_store::BlockStore;
use rchain_block_storage::dag::codecs::{BlockHashCodec, BlockMessageCodec};
use rchain_block_storage::dag::dag_storage::BlockDagStorage;
use rchain_block_storage::dag::dag_storage::LEGACY_BODY_SCAN_VERSION;
use rchain_models::block_hash::BlockHash;
use rchain_models::casper::protocol::casper_message::BlockMessage;
use rchain_shared::chan;
use rchain_shared::log::{Log, LogSource};
use rchain_shared::refined::BlockHeight;
use rchain_shared::typed_store::{Codec, KeyValueTypedStore};
use tokio::sync::mpsc;

use crate::blocks::block_receiver::{BlockReceiverState, ReofferClaim};
use crate::engine::node_phase::NodePhaseCell;

/// Where a pending body is in the pipeline. Diagnostic only: every pending row is equally
/// re-offerable at boot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingStage {
    /// The body is in the store and the receiver is resolving dependencies.
    Stored,
    /// The block was released to the processor; validation has not returned.
    Validating,
    /// A faulted block was re-offered by the retry loop.
    Retry,
}

impl PendingStage {
    fn as_byte(self) -> u8 {
        match self {
            PendingStage::Stored => 0,
            PendingStage::Validating => 1,
            PendingStage::Retry => 2,
        }
    }

    fn from_byte(byte: u8) -> Result<Self, String> {
        match byte {
            0 => Ok(PendingStage::Stored),
            1 => Ok(PendingStage::Validating),
            2 => Ok(PendingStage::Retry),
            other => Err(format!("unknown pending stage {other}")),
        }
    }
}

/// The unsettled state of one stored block. See the module docs for why only these two exist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockDisposition {
    Pending {
        block_number: BlockHeight,
        stage: PendingStage,
        since_unix_ms: u64,
    },
    Faulted {
        block_number: BlockHeight,
        stage: String,
        reason: String,
        since_unix_ms: u64,
        last_attempt_unix_ms: u64,
        attempts: u32,
    },
}

impl BlockDisposition {
    pub fn block_number(&self) -> BlockHeight {
        match self {
            BlockDisposition::Pending { block_number, .. }
            | BlockDisposition::Faulted { block_number, .. } => *block_number,
        }
    }

    pub fn attempts(&self) -> u32 {
        match self {
            BlockDisposition::Pending { .. } => 0,
            BlockDisposition::Faulted { attempts, .. } => *attempts,
        }
    }

    pub fn stage_description(&self) -> String {
        match self {
            BlockDisposition::Pending { stage, .. } => format!("pending/{stage:?}"),
            BlockDisposition::Faulted { stage, .. } => stage.clone(),
        }
    }
}

/// **The retry budget before a block is quarantined.** A fault that repeats is not transient; after
/// this many attempts the row stays `Faulted`, the pipeline fault stays visible, and only an
/// operator restart/reset or a peer's re-delivery can move it. The number is policy.
pub const MAX_FAULT_RETRIES: u32 = 8;

/// The first retry delay. Doubles per attempt up to [`MAX_RETRY_DELAY_MS`], so the eight attempts
/// span roughly half an hour without hammering a failing store or replay.
pub const FIRST_RETRY_DELAY_MS: u64 = 30_000;

pub const MAX_RETRY_DELAY_MS: u64 = 600_000;

fn retry_delay_ms(attempts: u32) -> u64 {
    let shift = attempts.saturating_sub(1).min(5);
    FIRST_RETRY_DELAY_MS
        .saturating_mul(1u64 << shift)
        .min(MAX_RETRY_DELAY_MS)
}

/// The clock the retry windows are measured on. Public so the retry loop and its tests read the
/// same source.
pub fn now_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// The on-disk codec. Hand-rolled rather than protobuf: the record is five fields, and a wire format
/// nobody else reads does not earn a schema. Lengths are bounded on encode so a pathological reason
/// string cannot make a row unreadable.
#[derive(Default)]
pub struct BlockDispositionCodec;

const TAG_PENDING: u8 = 0;
const TAG_FAULTED: u8 = 1;
const MAX_TEXT_BYTES: usize = 60_000;

fn bounded_text(text: &str) -> String {
    if text.len() <= MAX_TEXT_BYTES {
        return text.to_string();
    }
    let mut end = MAX_TEXT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

impl Codec<BlockDisposition> for BlockDispositionCodec {
    fn encode(&self, value: &BlockDisposition) -> Vec<u8> {
        let mut out = Vec::with_capacity(64);
        match value {
            BlockDisposition::Pending {
                block_number,
                stage,
                since_unix_ms,
            } => {
                out.push(TAG_PENDING);
                out.extend_from_slice(&i64::from(*block_number).to_le_bytes());
                out.extend_from_slice(&since_unix_ms.to_le_bytes());
                out.push(stage.as_byte());
            }
            BlockDisposition::Faulted {
                block_number,
                stage,
                reason,
                since_unix_ms,
                last_attempt_unix_ms,
                attempts,
            } => {
                out.push(TAG_FAULTED);
                out.extend_from_slice(&i64::from(*block_number).to_le_bytes());
                out.extend_from_slice(&since_unix_ms.to_le_bytes());
                out.extend_from_slice(&last_attempt_unix_ms.to_le_bytes());
                out.extend_from_slice(&attempts.to_le_bytes());
                let stage = bounded_text(stage);
                let reason = bounded_text(reason);
                let stage_len = u16::try_from(stage.len()).unwrap_or(u16::MAX);
                let reason_len = u16::try_from(reason.len()).unwrap_or(u16::MAX);
                out.extend_from_slice(&stage_len.to_le_bytes());
                out.extend_from_slice(stage.as_bytes());
                out.extend_from_slice(&reason_len.to_le_bytes());
                out.extend_from_slice(reason.as_bytes());
            }
        }
        out
    }

    fn decode(&self, bytes: &[u8]) -> Result<BlockDisposition, String> {
        let mut cursor = Cursor { bytes, at: 0 };
        let tag = cursor.byte()?;
        let block_number = BlockHeight::try_from(cursor.i64()?)
            .map_err(|e| format!("negative block height in disposition: {e}"))?;
        match tag {
            TAG_PENDING => {
                let since_unix_ms = cursor.u64()?;
                let stage = PendingStage::from_byte(cursor.byte()?)?;
                cursor.finish()?;
                Ok(BlockDisposition::Pending {
                    block_number,
                    stage,
                    since_unix_ms,
                })
            }
            TAG_FAULTED => {
                let since_unix_ms = cursor.u64()?;
                let last_attempt_unix_ms = cursor.u64()?;
                let attempts = cursor.u32()?;
                let stage = cursor.text()?;
                let reason = cursor.text()?;
                cursor.finish()?;
                Ok(BlockDisposition::Faulted {
                    block_number,
                    stage,
                    reason,
                    since_unix_ms,
                    last_attempt_unix_ms,
                    attempts,
                })
            }
            other => Err(format!("unknown block disposition tag {other}")),
        }
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self
            .at
            .checked_add(n)
            .ok_or_else(|| "disposition record length overflow".to_string())?;
        let slice = self
            .bytes
            .get(self.at..end)
            .ok_or_else(|| format!("disposition record truncated at {}", self.at))?;
        self.at = end;
        Ok(slice)
    }

    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, String> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, String> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u64(&mut self) -> Result<u64, String> {
        let bytes = self.take(8)?;
        let mut array = [0u8; 8];
        array.copy_from_slice(bytes);
        Ok(u64::from_le_bytes(array))
    }

    fn i64(&mut self) -> Result<i64, String> {
        let bytes = self.take(8)?;
        let mut array = [0u8; 8];
        array.copy_from_slice(bytes);
        Ok(i64::from_le_bytes(array))
    }

    fn text(&mut self) -> Result<String, String> {
        let len = usize::from(self.u16()?);
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|e| format!("disposition text is not UTF-8: {e}"))
    }

    fn finish(&self) -> Result<(), String> {
        if self.at == self.bytes.len() {
            Ok(())
        } else {
            Err(format!(
                "disposition record has {} trailing bytes",
                self.bytes.len() - self.at
            ))
        }
    }
}

/// The per-shard ledger over an LMDB-backed typed store.
#[derive(Clone)]
pub struct BlockDispositionStore {
    store: Arc<dyn KeyValueTypedStore<BlockHash, BlockDisposition>>,
    mutation: Arc<tokio::sync::Mutex<()>>,
}

/// **The body and its disposition, in one transaction** (C350).
///
/// The receiver used to store the body and then write the `Pending` row: two databases, two
/// transactions, and a crash between them left a body nothing owed. Both live in the block-storage
/// environment now, so `LmdbDirStoreManager::put_across` writes them together — either the body is
/// stored *and* owed, or neither changed.
///
/// **What this does not do**: it does not make the later stage transitions atomic. Those are row-only
/// writes over a body that is already stored and already owed, which is exactly the state the ledger
/// exists to make durable.
#[derive(Clone)]
pub struct BodyDispositionWriter {
    manager: rchain_shared::lmdb::LmdbDirStoreManager,
}

impl BodyDispositionWriter {
    pub fn new(manager: rchain_shared::lmdb::LmdbDirStoreManager) -> Self {
        Self { manager }
    }

    /// Store `block`'s body and record that this shard owes it an outcome, in one write.
    ///
    /// The row keeps an existing `Faulted` record (a fault is a stronger fact than a re-delivery) and
    /// an existing `Pending` row's first-sighting stamp, exactly as [`BlockDispositionStore::mark_pending`]
    /// does — the difference is that the read happens inside the transaction that writes the body, so
    /// there is no window for a second writer to interleave.
    pub async fn store_body_and_mark_pending(
        &self,
        block: &BlockMessage,
        stage: PendingStage,
    ) -> Result<(), String> {
        let key = BlockHashCodec.encode(&block.block_hash);
        let body = BlockMessageCodec.encode(block);
        let block_number = block.block_number;
        let since_unix_ms = now_unix_millis();
        self.manager
            .put_across(
                rchain_block_storage::block_store::BLOCKS_DB,
                &[(key, body)],
                crate::storage::BLOCK_DISPOSITION_DB,
                move |_key, existing| {
                    let next = match existing
                        .map(|bytes| BlockDispositionCodec.decode(bytes))
                        .transpose()?
                    {
                        Some(BlockDisposition::Faulted { .. }) => return Ok(None),
                        Some(BlockDisposition::Pending { since_unix_ms, .. }) => {
                            BlockDisposition::Pending {
                                block_number,
                                stage,
                                since_unix_ms,
                            }
                        }
                        None => BlockDisposition::Pending {
                            block_number,
                            stage,
                            since_unix_ms,
                        },
                    };
                    Ok(Some(BlockDispositionCodec.encode(&next)))
                },
            )
            .await
    }
}

impl BlockDispositionStore {
    pub fn from_store(store: Arc<dyn KeyValueTypedStore<BlockHash, BlockDisposition>>) -> Self {
        Self {
            store,
            mutation: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// **Record that this node owes the block an outcome.** Written when the body is in the store,
    /// before dependency resolution, so a crash or a receiver error cannot lose the obligation. An
    /// existing row is kept, so a re-delivery does not reset the fault's first sighting.
    pub async fn mark_pending(
        &self,
        hash: BlockHash,
        block_number: BlockHeight,
        stage: PendingStage,
    ) -> Result<(), String> {
        let _mutation = self.mutation.lock().await;
        let since_unix_ms = match self.get(&hash).await? {
            // A fault is a stronger record than "pending"; a re-delivery must not erase it.
            Some(BlockDisposition::Faulted { .. }) => return Ok(()),
            Some(BlockDisposition::Pending { since_unix_ms, .. }) => since_unix_ms,
            None => now_unix_millis(),
        };
        self.store
            .put(&[(
                hash,
                BlockDisposition::Pending {
                    block_number,
                    stage,
                    since_unix_ms,
                },
            )])
            .await
    }

    /// Record a node fault and return the attempt count after the increment.
    ///
    /// A fault on a row that is already `Faulted` preserves the first sighting and increments; a
    /// fault on a `Pending` row starts the fault record at attempt 1.
    pub async fn mark_faulted(
        &self,
        hash: BlockHash,
        block_number: BlockHeight,
        stage: &str,
        reason: &str,
    ) -> Result<u32, String> {
        let _mutation = self.mutation.lock().await;
        let now = now_unix_millis();
        let (since_unix_ms, attempts) = match self.get(&hash).await? {
            Some(BlockDisposition::Faulted {
                since_unix_ms,
                attempts,
                ..
            }) => (since_unix_ms, attempts.saturating_add(1)),
            Some(BlockDisposition::Pending { since_unix_ms, .. }) => (since_unix_ms, 1),
            None => (now, 1),
        };
        self.store
            .put(&[(
                hash,
                BlockDisposition::Faulted {
                    block_number,
                    stage: bounded_text(stage),
                    reason: bounded_text(reason),
                    since_unix_ms,
                    last_attempt_unix_ms: now,
                    attempts,
                },
            )])
            .await?;
        Ok(attempts)
    }

    /// A fault that cannot succeed on retry (self-equivocation is a proposer-only outcome on a
    /// received block) is recorded with the budget already spent, so no retry loop picks it up.
    /// It stays visible until an operator resets or repairs.
    pub async fn mark_terminal_fault(
        &self,
        hash: BlockHash,
        block_number: BlockHeight,
        stage: &str,
        reason: &str,
    ) -> Result<(), String> {
        let _mutation = self.mutation.lock().await;
        let now = now_unix_millis();
        let since_unix_ms = match self.get(&hash).await? {
            Some(disposition) => match disposition {
                BlockDisposition::Faulted { since_unix_ms, .. } => since_unix_ms,
                BlockDisposition::Pending { since_unix_ms, .. } => since_unix_ms,
            },
            None => now,
        };
        self.store
            .put(&[(
                hash,
                BlockDisposition::Faulted {
                    block_number,
                    stage: bounded_text(stage),
                    reason: bounded_text(reason),
                    since_unix_ms,
                    last_attempt_unix_ms: now,
                    attempts: MAX_FAULT_RETRIES,
                },
            )])
            .await
    }

    /// Mark that a retry is now in flight, so the loop does not re-send it until the next window.
    pub async fn mark_retry_started(&self, hash: BlockHash) -> Result<(), String> {
        let _mutation = self.mutation.lock().await;
        match self.get(&hash).await? {
            Some(BlockDisposition::Faulted {
                block_number,
                stage,
                reason,
                since_unix_ms,
                attempts,
                ..
            }) => {
                self.store
                    .put(&[(
                        hash,
                        BlockDisposition::Faulted {
                            block_number,
                            stage,
                            reason,
                            since_unix_ms,
                            last_attempt_unix_ms: now_unix_millis(),
                            attempts,
                        },
                    )])
                    .await
            }
            Some(BlockDisposition::Pending {
                block_number,
                since_unix_ms,
                ..
            }) => {
                self.store
                    .put(&[(
                        hash,
                        BlockDisposition::Pending {
                            block_number,
                            stage: PendingStage::Retry,
                            since_unix_ms,
                        },
                    )])
                    .await
            }
            None => Ok(()),
        }
    }

    /// Claim an aged Stored row only if it is still eligible now. Ledger snapshots
    /// can outlive a concurrent verdict, validation handoff or terminal fault.
    /// The shared mutation lock makes that check and transition one operation.
    pub async fn reserve_stored_retry(&self, hash: BlockHash, now_ms: u64) -> Result<bool, String> {
        let _mutation = self.mutation.lock().await;
        let Some(disposition) = self.get(&hash).await? else {
            return Ok(false);
        };
        if !eligible_stored_pending(&disposition, now_ms) {
            return Ok(false);
        }
        let BlockDisposition::Pending {
            block_number,
            since_unix_ms,
            ..
        } = disposition
        else {
            return Ok(false);
        };
        self.store
            .put(&[(
                hash,
                BlockDisposition::Pending {
                    block_number,
                    stage: PendingStage::Retry,
                    since_unix_ms,
                },
            )])
            .await?;
        Ok(true)
    }

    /// **A verdict, or a fault the node has stopped retrying, settles the obligation.** The DAG
    /// metadata is the record for an accepted or refused block; a quarantined fault stays visible
    /// until an operator acts, so it is not deleted here.
    pub async fn settle(&self, hash: BlockHash) -> Result<usize, String> {
        let _mutation = self.mutation.lock().await;
        self.store.delete(&[hash]).await
    }

    pub async fn get(&self, hash: &BlockHash) -> Result<Option<BlockDisposition>, String> {
        Ok(self.store.get(&[*hash]).await?.into_iter().next().flatten())
    }

    /// Every row the node still owes an outcome, oldest first by block height.
    pub async fn unsettled(&self) -> Result<Vec<(BlockHash, BlockDisposition)>, String> {
        let mut rows: Vec<(BlockHash, BlockDisposition)> =
            self.store.to_map().await?.into_iter().collect();
        rows.sort_by_key(|(_, disposition)| disposition.block_number());
        Ok(rows)
    }

    /// The rows whose retry window has opened and whose budget is not spent.
    pub async fn due_retries(
        &self,
        now_unix_ms: u64,
    ) -> Result<Vec<(BlockHash, BlockDisposition)>, String> {
        let mut due: Vec<(BlockHash, BlockDisposition)> = self
            .store
            .to_map()
            .await?
            .into_iter()
            .filter(|(_, disposition)| match disposition {
                BlockDisposition::Faulted {
                    last_attempt_unix_ms,
                    attempts,
                    ..
                } => {
                    *attempts < MAX_FAULT_RETRIES
                        && now_unix_ms.saturating_sub(*last_attempt_unix_ms)
                            >= retry_delay_ms(*attempts)
                }
                BlockDisposition::Pending { .. } => false,
            })
            .collect();
        due.sort_by_key(|(_, disposition)| disposition.block_number());
        Ok(due)
    }
}

/// How often the retry loop asks the ledger whether a fault's window has opened.
pub const RETRY_SWEEP_INTERVAL: Duration = Duration::from_secs(15);

/// **Re-offer every body this node still owes an outcome for.** Runs once at pipeline start, before
/// the node reports ready, so a crash between "body stored" and "DAG entry" is a retry rather than a
/// permanent orphan.
///
/// Rows are processed oldest first, and a row is skipped (not faulted) when a justification is not
/// yet in the DAG: the parent is also in the ledger and is sent earlier, so the processor's serial
/// order inserts it first; a genuinely missing parent leaves the row `Pending` for the next start
/// rather than spending its retry budget on a question that cannot be answered yet.
pub async fn reoffer_unsettled(
    ledger: BlockDispositionStore,
    block_store: BlockStore,
    dag: Arc<dyn BlockDagStorage>,
    to_processor: mpsc::Sender<BlockMessage>,
    log: Arc<dyn Log>,
) -> Result<ReofferReport, String> {
    let source = LogSource::new("casper.blocks.BlockDisposition");
    let mut report = ReofferReport::default();
    let rows = ledger
        .unsettled()
        .await
        .map_err(|e| format!("could not read the disposition ledger at start-up: {e}"))?;
    for (hash, disposition) in rows {
        if disposition.attempts() >= MAX_FAULT_RETRIES {
            report.quarantined = report.quarantined.saturating_add(1);
            log.error(
                source,
                &format!(
                    "block {} at height {} is quarantined after {} faults at stage {}; only an \
                     operator reset or an explicit repair clears it",
                    hash.to_hex(),
                    i64::from(disposition.block_number()),
                    disposition.attempts(),
                    disposition.stage_description()
                ),
            );
            continue;
        }
        let repr = dag.get_representation().await;
        if repr.contains(&hash) {
            // The crash landed after the insert, before the settle. The metadata is the verdict.
            report.settled_already_inserted = report.settled_already_inserted.saturating_add(1);
            if let Err(e) = ledger.settle(hash).await {
                log.warn(
                    source,
                    &format!(
                        "could not settle the already-inserted block {}: {e}",
                        hash.to_hex()
                    ),
                );
            }
            continue;
        }
        let stored = match block_store.get(&[hash]).await {
            Ok(stored) => stored,
            Err(e) => {
                report.read_errors = report.read_errors.saturating_add(1);
                log.error(
                    source,
                    &format!(
                        "could not read the stored body of block {}: {e}",
                        hash.to_hex()
                    ),
                );
                continue;
            }
        };
        let Some(block) = stored.into_iter().next().flatten() else {
            report.body_missing = report.body_missing.saturating_add(1);
            log.error(
                source,
                &format!(
                    "block {} has a {} disposition but no stored body; only an operator reset can \
                     settle it",
                    hash.to_hex(),
                    disposition.stage_description()
                ),
            );
            continue;
        };
        if !block
            .justifications
            .iter()
            .all(|parent| repr.contains(parent))
        {
            report.skipped_missing_parents = report.skipped_missing_parents.saturating_add(1);
            log.info(
                source,
                &format!(
                    "block {} still waits on a justification not in the DAG; keeping its {} \
                     disposition for a later attempt",
                    hash.to_hex(),
                    disposition.stage_description()
                ),
            );
            continue;
        }
        if let Err(e) = ledger.mark_retry_started(hash).await {
            report.ledger_errors = report.ledger_errors.saturating_add(1);
            log.error(
                source,
                &format!("could not mark block {} as re-offered: {e}", hash.to_hex()),
            );
        }
        if !chan::send(&to_processor, block).await {
            report.channel_closed = true;
            break;
        }
        report.offered.push(hash);
        report.reoffered = report.reoffered.saturating_add(1);
    }
    log.info(
        source,
        &format!(
            "start-up disposition audit: {} unsettled block(s) re-offered, {} skipped for missing \
             parents, {} already inserted, {} quarantined, {} unreadable",
            report.reoffered,
            report.skipped_missing_parents,
            report.settled_already_inserted,
            report.quarantined,
            report.body_missing.saturating_add(report.read_errors)
        ),
    );
    Ok(report)
}

/// What the start-up re-offer did. The `offered` hashes are the only rows the bounded drain waits
/// for: a row skipped for a missing parent was never sent, and must not hold boot.
#[derive(Debug, Default)]
pub struct ReofferReport {
    pub offered: Vec<BlockHash>,
    pub reoffered: usize,
    pub settled_already_inserted: usize,
    pub skipped_missing_parents: usize,
    pub quarantined: usize,
    pub body_missing: usize,
    pub read_errors: usize,
    pub ledger_errors: usize,
    pub channel_closed: bool,
}

/// **The start-up audit's liveness bound.** Long enough for in-process validation of the rows just
/// re-offered, far below `BLOCK_VALIDATION_TIMEOUT` (600 s): the point is to let the ledger drain,
/// not to wait out a replay. A row still `Pending` when the window closes stays visible.
pub const STARTUP_AUDIT_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);
pub const STARTUP_AUDIT_POLL_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Default)]
pub struct DrainOutcome {
    pub settled: usize,
    pub still_pending: usize,
    pub timed_out: bool,
}

/// Wait, bounded, for the rows the re-offer sent to leave the ledger. Only a **Pending** row in
/// `offered` holds the drain: a settled row is a verdict, and a faulted row is the retry loop's
/// question, not the boot's.
pub async fn wait_for_reoffered_rows(
    ledger: &BlockDispositionStore,
    offered: &[BlockHash],
    timeout: Duration,
    log: &Arc<dyn Log>,
) -> DrainOutcome {
    let source = LogSource::new("casper.blocks.BlockDisposition");
    if offered.is_empty() {
        return DrainOutcome::default();
    }
    let wanted: std::collections::BTreeSet<BlockHash> = offered.iter().copied().collect();
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let still = match ledger.unsettled().await {
            Ok(rows) => rows
                .iter()
                .filter(|(hash, disposition)| {
                    wanted.contains(hash) && matches!(disposition, BlockDisposition::Pending { .. })
                })
                .count(),
            Err(e) => {
                log.error(
                    source,
                    &format!("could not read the disposition ledger during the drain: {e}"),
                );
                return DrainOutcome {
                    settled: 0,
                    still_pending: wanted.len(),
                    timed_out: true,
                };
            }
        };
        if still == 0 {
            return DrainOutcome {
                settled: wanted.len(),
                still_pending: 0,
                timed_out: false,
            };
        }
        if tokio::time::Instant::now() >= deadline {
            return DrainOutcome {
                settled: wanted.len().saturating_sub(still),
                still_pending: still,
                timed_out: true,
            };
        }
        tokio::time::sleep(STARTUP_AUDIT_POLL_INTERVAL).await;
    }
}

/// **The one-time scan for bodies written before the disposition ledger** (U19/F-U19-07).
///
/// It enumerates the block store's **keys only** — the body is decoded one orphan at a time, never
/// the whole store — adopts each hash the DAG does not hold as a `Pending` row, and writes its
/// durable marker last. The re-offer that follows is what sends them; the scan never sends.
///
/// The marker is written only after every decodable orphan has a row, so a crash before it rescans
/// next boot and a crash after it leaves rows the re-offer handles. An **undecodable** body is
/// logged by hash and does not block the marker: walking again cannot decode it, and it remains the
/// periodic count audit's signal (the operator reset is its repair).
pub const LEGACY_SCAN_LOG_LIMIT: usize = 64;

#[derive(Debug, Default)]
pub struct LegacyScanReport {
    pub scanned: bool,
    pub keys: usize,
    pub orphans: usize,
    pub adopted: usize,
    pub unreadable_bodies: usize,
    pub marker_written: bool,
    pub reported_hashes: Vec<BlockHash>,
}

pub async fn scan_legacy_bodies(
    ledger: &BlockDispositionStore,
    block_store: &BlockStore,
    dag: &Arc<dyn BlockDagStorage>,
    log: &Arc<dyn Log>,
) -> Result<LegacyScanReport, String> {
    scan_unaccounted_bodies(ledger, block_store, dag, log, false).await
}

/// A later interrupted store/disposition write can create an unaccounted body
/// *after* the one-time legacy marker has been written. The runtime orphan
/// audit may recheck that durable obligation without rewinding the marker.
async fn scan_unaccounted_bodies(
    ledger: &BlockDispositionStore,
    block_store: &BlockStore,
    dag: &Arc<dyn BlockDagStorage>,
    log: &Arc<dyn Log>,
    ignore_legacy_marker: bool,
) -> Result<LegacyScanReport, String> {
    let source = LogSource::new("casper.blocks.BlockDisposition");
    if !ignore_legacy_marker {
        match dag.read_legacy_body_scan_marker().await {
            Ok(Some(version)) if version >= LEGACY_BODY_SCAN_VERSION => {
                return Ok(LegacyScanReport::default());
            }
            Ok(_) => {}
            Err(e) => {
                log.warn(
                    source,
                    &format!("legacy body scan skipped: its marker could not be read: {e}"),
                );
                return Err(e);
            }
        }
    }
    let keys = block_store.keys().await?;
    let repr = dag.get_representation().await;
    let mut report = LegacyScanReport {
        scanned: true,
        keys: keys.len(),
        ..LegacyScanReport::default()
    };
    for hash in keys {
        if repr.dag_set.contains(&hash) {
            continue;
        }
        report.orphans = report.orphans.saturating_add(1);
        if ledger.get(&hash).await?.is_some() {
            continue;
        }
        match block_store.get(&[hash]).await {
            Ok(stored) => match stored.into_iter().next().flatten() {
                Some(block) => match ledger
                    .mark_pending(hash, block.block_number, PendingStage::Stored)
                    .await
                {
                    Ok(()) => {
                        report.adopted = report.adopted.saturating_add(1);
                        if report.reported_hashes.len() < LEGACY_SCAN_LOG_LIMIT {
                            report.reported_hashes.push(hash);
                        }
                    }
                    Err(e) => log.error(
                        source,
                        &format!(
                            "legacy body scan could not adopt block {}: {e}; the marker is not \
                             written and the next boot retries",
                            hash.to_hex()
                        ),
                    ),
                },
                None => {
                    report.unreadable_bodies = report.unreadable_bodies.saturating_add(1);
                    log.error(
                        source,
                        &format!(
                            "legacy body scan: key {} has no readable body; it stays an orphan and \
                             only an operator reset can repair it",
                            hash.to_hex()
                        ),
                    );
                }
            },
            Err(e) => {
                report.unreadable_bodies = report.unreadable_bodies.saturating_add(1);
                log.error(
                    source,
                    &format!(
                        "legacy body scan could not read block {}: {e}; it stays an orphan",
                        hash.to_hex()
                    ),
                );
            }
        }
    }
    if report.orphans > 0 {
        log.warn(
            source,
            &format!(
                "legacy body scan: {} key(s), {} orphan(s), {} adopted, {} unreadable; first \
                 adopted: {:?}",
                report.keys,
                report.orphans,
                report.adopted,
                report.unreadable_bodies,
                report
                    .reported_hashes
                    .iter()
                    .map(|h| h.to_hex())
                    .collect::<Vec<_>>()
            ),
        );
    }
    if !ignore_legacy_marker {
        match dag
            .write_legacy_body_scan_marker(LEGACY_BODY_SCAN_VERSION)
            .await
        {
            Ok(()) => report.marker_written = true,
            Err(e) => log.error(
                source,
                &format!(
                    "legacy body scan completed but could not write its marker; the next boot will \
                     rescan: {e}"
                ),
            ),
        }
    }
    Ok(report)
}

#[derive(Debug, Default)]
pub struct StartupAuditOutcome {
    pub scan: Option<LegacyScanReport>,
    pub reoffer: ReofferReport,
    pub drain: DrainOutcome,
}

/// **The start-up audit, then `Running`.** Scan (unit 4), re-offer, drain bounded, then move the
/// shard's phase. The phase transition is here rather than in launch so the law's `startup` and the
/// code's one path are the same shape. A ledger/store error is logged and does not refuse boot: the
/// rows are durable and the periodic count audit still withholds readiness for a real orphan.
pub async fn startup_audit_then_running(
    phase: &NodePhaseCell,
    ledger: &BlockDispositionStore,
    block_store: &BlockStore,
    dag: &Arc<dyn BlockDagStorage>,
    to_processor: &mpsc::Sender<BlockMessage>,
    drain_timeout: Duration,
    log: &Arc<dyn Log>,
) -> StartupAuditOutcome {
    let source = LogSource::new("casper.blocks.BlockDisposition");
    let scan = match scan_legacy_bodies(ledger, block_store, dag, log).await {
        Ok(report) => Some(report),
        Err(e) => {
            log.error(source, &e);
            None
        }
    };
    let reoffer = match reoffer_unsettled(
        ledger.clone(),
        block_store.clone(),
        dag.clone(),
        to_processor.clone(),
        log.clone(),
    )
    .await
    {
        Ok(report) => report,
        Err(e) => {
            log.error(source, &e);
            ReofferReport::default()
        }
    };
    let drain = wait_for_reoffered_rows(ledger, &reoffer.offered, drain_timeout, log).await;
    if drain.timed_out {
        log.warn(
            source,
            &format!(
                "start-up audit drain closed after {:?} with {} row(s) still pending; they stay \
                 visible and the periodic stored-body audit still gates readiness",
                drain_timeout, drain.still_pending
            ),
        );
    }
    phase.set_phase(crate::engine::node_phase::NodePhase::Running);
    StartupAuditOutcome {
        scan,
        reoffer,
        drain,
    }
}

/// **How long a `Stored` row may sit before the runtime re-offer sweep will take it.** Normal
/// ingress hands a body on within milliseconds; the floor keeps the sweep from racing the
/// receiver's own release and from duplicating the boot sweep's first pass.
pub const PENDING_PARENT_RECHECK_AGE_MS: u64 = 60_000;

/// Whether a row is a `Stored` pending body old enough for the re-offer sweep. `Validating` and
/// `Retry` rows are already in flight; `Faulted` rows belong to the backoff loop.
pub fn eligible_stored_pending(disposition: &BlockDisposition, now_ms: u64) -> bool {
    matches!(
        disposition,
        BlockDisposition::Pending {
            stage: PendingStage::Stored,
            since_unix_ms,
            ..
        } if now_ms.saturating_sub(*since_unix_ms) >= PENDING_PARENT_RECHECK_AGE_MS
    )
}

/// **One re-offer sweep** (Law 88c), extracted so a test can drive it deterministically.
///
/// For each eligible stored row: settle it if it is already in the DAG; otherwise load the body,
/// require every justification in the DAG, claim the receiver's in-memory state (so a concurrent
/// release cannot dispatch it twice), mark the row in flight, and hand the body to the processor.
/// Returns the number of bodies dispatched.
#[allow(clippy::too_many_arguments)]
pub async fn reoffer_ready_stored_pending_once(
    ledger: &BlockDispositionStore,
    block_store: &BlockStore,
    dag: &Arc<dyn BlockDagStorage>,
    receiver_state: &tokio::sync::Mutex<BlockReceiverState<BlockHash>>,
    to_processor: &mpsc::Sender<BlockMessage>,
    now_ms: u64,
    log: &dyn Log,
) -> Result<usize, String> {
    let source = LogSource::new("casper.blocks.BlockDisposition");
    let rows = ledger.unsettled().await?;
    let mut dispatched = 0;
    for (hash, disposition) in rows {
        if !eligible_stored_pending(&disposition, now_ms) {
            continue;
        }
        let repr = dag.get_representation().await;
        if repr.contains(&hash) {
            if let Err(e) = ledger.settle(hash).await {
                log.warn(
                    source,
                    &format!(
                        "could not settle inserted pending block {}: {e}",
                        hash.to_hex()
                    ),
                );
            }
            continue;
        }
        let stored = match block_store.get(&[hash]).await {
            Ok(stored) => stored,
            Err(e) => {
                log.error(
                    source,
                    &format!("could not load stored pending body {}: {e}", hash.to_hex()),
                );
                continue;
            }
        };
        let Some(block) = stored.into_iter().next().flatten() else {
            log.error(
                source,
                &format!(
                    "pending body {} is missing from storage; retaining the obligation",
                    hash.to_hex()
                ),
            );
            continue;
        };
        if !block
            .justifications
            .iter()
            .all(|parent| repr.contains(parent))
        {
            continue;
        }
        // The row moves in flight before the claim, so a claim that turns out `Owned` (the
        // receiver is dispatching it) is still covered by the processor's settle/fault.
        match ledger.reserve_stored_retry(hash, now_ms).await {
            Ok(true) => {}
            Ok(false) => continue,
            Err(e) => {
                log.error(
                    source,
                    &format!("could not reserve pending replay {}: {e}", hash.to_hex()),
                );
                continue;
            }
        }
        if receiver_state.lock().await.claim_reoffer(&hash) == ReofferClaim::Owned {
            continue;
        }
        log.info(
            source,
            &format!(
                "re-offering stored pending body {} after its parents entered the DAG",
                hash.to_hex()
            ),
        );
        if !chan::send(to_processor, block).await {
            return Ok(dispatched);
        }
        dispatched += 1;
    }
    Ok(dispatched)
}

/// The runtime loop over [`reoffer_ready_stored_pending_once`], on the same cadence as the fault
/// retry and gated on the shard being `Running` (the #499 phase cell). A sweep that cannot read
/// the ledger logs and waits for the next tick; it never advances a row it did not dispatch.
pub async fn retry_ready_stored_pending(
    phase: NodePhaseCell,
    ledger: BlockDispositionStore,
    block_store: BlockStore,
    dag: Arc<dyn BlockDagStorage>,
    receiver_state: Arc<tokio::sync::Mutex<BlockReceiverState<BlockHash>>>,
    to_processor: mpsc::Sender<BlockMessage>,
    log: Arc<dyn Log>,
) {
    let source = LogSource::new("casper.blocks.BlockDisposition");
    phase.wait_until_running().await;
    let mut interval = tokio::time::interval(RETRY_SWEEP_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // The boot sweep gets the first immediate tick; this loop starts after one interval.
    interval.tick().await;
    // A confirmed runtime orphan may have been created after the one-time
    // legacy scan. Walk the store once per detected gap episode, not per tick.
    let mut orphan_scan_attempted = false;
    loop {
        interval.tick().await;
        if orphan_bodies() == 0 {
            orphan_scan_attempted = false;
        } else if !orphan_scan_attempted {
            match block_store.count().await {
                Ok(stored) if stored > dag.get_representation().await.dag_set.len() => {
                    match scan_unaccounted_bodies(&ledger, &block_store, &dag, &log, true).await {
                        Ok(report) => {
                            orphan_scan_attempted = true;
                            if report.adopted > 0 {
                                log.warn(
                                    source,
                                    &format!(
                                        "runtime orphan audit adopted {} stored bodies without \
                                         disposition rows; durable retry remains subject to the \
                                         existing DAG/parent and readiness gates",
                                        report.adopted
                                    ),
                                );
                            }
                        }
                        Err(e) => log.error(
                            source,
                            &format!("runtime orphan adoption failed; will retry: {e}"),
                        ),
                    }
                }
                Ok(_) => {}
                Err(e) => log.error(
                    source,
                    &format!("runtime orphan audit could not count stored bodies: {e}"),
                ),
            }
        }
        if let Err(e) = reoffer_ready_stored_pending_once(
            &ledger,
            &block_store,
            &dag,
            &receiver_state,
            &to_processor,
            now_unix_millis(),
            log.as_ref(),
        )
        .await
        {
            log.error(
                source,
                &format!("stored-pending re-offer sweep could not read the ledger: {e}"),
            );
        }
    }
}

/// The retry loop: every [`RETRY_SWEEP_INTERVAL`], re-offer the faulted blocks whose backoff has
/// elapsed. The processor's fault handlers update the row, so a repeated fault counts and a
/// successful one settles it. A terminal fault stays in the ledger and keeps the pipeline fault
/// surface loud; nothing here clears it without an operator.
pub async fn retry_faulted_blocks(
    phase: NodePhaseCell,
    ledger: BlockDispositionStore,
    block_store: BlockStore,
    to_processor: mpsc::Sender<BlockMessage>,
    log: Arc<dyn Log>,
) {
    let source = LogSource::new("casper.blocks.BlockDisposition");
    phase.wait_until_running().await;
    let mut interval = tokio::time::interval(RETRY_SWEEP_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let due = match ledger.due_retries(now_unix_millis()).await {
            Ok(due) => due,
            Err(e) => {
                log.error(
                    source,
                    &format!("could not read the disposition ledger; retry sweep skipped: {e}"),
                );
                continue;
            }
        };
        for (hash, disposition) in due {
            let stored = match block_store.get(&[hash]).await {
                Ok(stored) => stored,
                Err(e) => {
                    log.error(
                        source,
                        &format!(
                            "could not read the stored body of block {}: {e}",
                            hash.to_hex()
                        ),
                    );
                    continue;
                }
            };
            let Some(block) = stored.into_iter().next().flatten() else {
                log.error(
                    source,
                    &format!(
                        "faulted block {} has no stored body; the fault stays and only an \
                         operator reset can settle it",
                        hash.to_hex()
                    ),
                );
                continue;
            };
            // **Update the window before the send**, so a send that faults and returns to this loop
            // does not re-offer the same block on the next tick.
            if let Err(e) = ledger.mark_retry_started(hash).await {
                log.error(
                    source,
                    &format!("could not mark block {} as retried: {e}", hash.to_hex()),
                );
            }
            log.warn(
                source,
                &format!(
                    "re-offering faulted block #{} {} (attempt {} of {}, stage {})",
                    block.block_number,
                    hash.to_hex(),
                    disposition.attempts().saturating_add(1),
                    MAX_FAULT_RETRIES,
                    disposition.stage_description()
                ),
            );
            if !chan::send(&to_processor, block).await {
                return;
            }
        }
    }
}

/// How often the stored-body audit samples the two counts.
pub const ORPHAN_AUDIT_INTERVAL: Duration = Duration::from_secs(60);

/// **How many consecutive samples a store/DAG gap must persist before it is an orphan.** Every
/// healthy node has a body stored for a moment before its DAG entry exists; treating one sample as
/// an orphan would flap the readiness gate on every block. Three minutes at the default interval is
/// long past any legitimate in-flight block and short enough that the live incident's first hour
/// would have been loud.
pub const ORPHAN_AUDIT_SAMPLES: u32 = 3;

/// The confirmed gap — bodies in the block store that the DAG does not account for — and how many
/// times a sustained gap has been entered. Process-wide (the gate and `/health` are per node); a
/// shard-local audit writes the sum, which is the conservative direction for readiness.
static ORPHAN_BODIES: AtomicU64 = AtomicU64::new(0);
static ORPHAN_DETECTIONS: AtomicU64 = AtomicU64::new(0);

// A boot-time store/DAG mismatch is known before readiness is exposed. The
// periodic three-sample rule only protects against transient *runtime* writes;
// it must not erase an already observed startup discrepancy.
pub fn observe_boot_orphan_gap(stored: usize, in_dag: usize) -> u64 {
    let gap = u64::try_from(stored.saturating_sub(in_dag)).unwrap_or(u64::MAX);
    if gap > 0 {
        let previous = ORPHAN_BODIES.swap(gap, Ordering::Relaxed);
        if previous == 0 {
            ORPHAN_DETECTIONS.fetch_add(1, Ordering::Relaxed);
        }
    }
    gap
}

/// The currently observed orphan count. A mismatch already known at boot blocks readiness
/// immediately; a transient runtime write counts only after three consecutive audit samples.
pub fn orphan_bodies() -> u64 {
    ORPHAN_BODIES.load(Ordering::Relaxed)
}

/// Clear the readiness blocker only when the store/DAG discrepancy is actually gone.
fn clear_orphan_gate_if_resolved(gap: u64) -> bool {
    gap == 0 && ORPHAN_BODIES.swap(0, Ordering::Relaxed) > 0
}

/// How many distinct sustained orphan episodes this process has seen. Monotone, so an operator can
/// tell "the gate is clear because the gap closed" from "the gate never tripped".
pub fn orphan_detections() -> u64 {
    ORPHAN_DETECTIONS.load(Ordering::Relaxed)
}

/// The consecutive-sample rule, separated so it can be tested without a node: returns `Some(gap)`
/// exactly when a gap has been observed `ORPHAN_AUDIT_SAMPLES` times running.
#[derive(Default)]
pub struct OrphanStreak {
    samples: u32,
}

impl OrphanStreak {
    pub fn observe(&mut self, gap: u64) -> Option<u64> {
        if gap == 0 {
            self.samples = 0;
            return None;
        }
        self.samples = self.samples.saturating_add(1);
        if self.samples >= ORPHAN_AUDIT_SAMPLES {
            Some(gap)
        } else {
            None
        }
    }
}

/// **A body the DAG does not account for is a fault even when no ledger row names it** (U19,
/// F-U19-07). This is the periodic audit the start-up re-offer cannot be: it compares the two
/// counts a healthy shard keeps equal — one record in the block store per block in the DAG index —
/// and reports a *sustained* difference. A body written by a pre-ledger build lands here; so does
/// any future accounting bug. The audit does not delete or repair: it makes the state loud and
/// withholds readiness, and the operator reset remains the repair.
pub async fn audit_stored_bodies(
    phase: NodePhaseCell,
    block_store: BlockStore,
    dag: Arc<dyn BlockDagStorage>,
    log: Arc<dyn Log>,
) {
    let source = LogSource::new("casper.blocks.BlockDisposition");
    phase.wait_until_running().await;
    let mut interval = tokio::time::interval(ORPHAN_AUDIT_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut streak = OrphanStreak::default();
    loop {
        interval.tick().await;
        let stored = match block_store.count().await {
            Ok(count) => u64::try_from(count).unwrap_or(u64::MAX),
            Err(e) => {
                log.error(
                    source,
                    &format!("stored-body audit could not count the block store: {e}"),
                );
                continue;
            }
        };
        let in_dag =
            u64::try_from(dag.get_representation().await.dag_set.len()).unwrap_or(u64::MAX);
        let gap = stored.saturating_sub(in_dag);
        match streak.observe(gap) {
            Some(gap) => {
                let previous = ORPHAN_BODIES.swap(gap, Ordering::Relaxed);
                if previous == 0 {
                    ORPHAN_DETECTIONS.fetch_add(1, Ordering::Relaxed);
                    log.error(
                        source,
                        &format!(
                            "{gap} stored block body/bodies have no DAG entry for \
                             {ORPHAN_AUDIT_SAMPLES} consecutive samples ({stored} stored, {in_dag} \
                             in the DAG); the readiness gate is now blocked. The start-up re-offer \
                             covers bodies with a disposition row; a body without one is a \
                             pre-ledger orphan and needs an operator reset or a peer re-delivery"
                        ),
                    );
                }
            }
            None => {
                if clear_orphan_gate_if_resolved(gap) {
                    log.info(
                        source,
                        &format!(
                            "the stored-body/DAG gap closed ({stored} stored, {in_dag} in the DAG)"
                        ),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_block_storage::dag::codecs::BlockHashCodec;
    use rchain_shared::store_manager::{database, InMemoryStoreManager};

    fn hash(byte: u8) -> BlockHash {
        BlockHash::new([byte; 32])
    }

    fn height(n: i64) -> BlockHeight {
        BlockHeight::try_from(n).unwrap()
    }

    #[tokio::test]
    async fn stale_pending_snapshots_cannot_duplicate_a_claim_or_retry_a_terminal_fault() {
        let ledger = store().await;
        let block = hash(70);
        ledger
            .mark_pending(block, height(8), PendingStage::Stored)
            .await
            .unwrap();
        let now = now_unix_millis().saturating_add(PENDING_PARENT_RECHECK_AGE_MS);
        let first = ledger.clone();
        let second = ledger.clone();
        let (a, b) = tokio::join!(
            first.reserve_stored_retry(block, now),
            second.reserve_stored_retry(block, now),
        );
        assert_eq!(usize::from(a.unwrap()) + usize::from(b.unwrap()), 1);

        // A sweep may still hold its old Stored snapshot after a fault or
        // verdict. Neither outcome may be turned back into an eligible retry.
        ledger
            .mark_terminal_fault(block, height(8), "terminal", "quarantined")
            .await
            .unwrap();
        assert!(!ledger.reserve_stored_retry(block, now).await.unwrap());
        assert_eq!(
            ledger.get(&block).await.unwrap().unwrap().attempts(),
            MAX_FAULT_RETRIES
        );
        ledger.settle(block).await.unwrap();
        assert!(!ledger.reserve_stored_retry(block, now).await.unwrap());
        assert!(ledger.get(&block).await.unwrap().is_none());
    }

    #[test]
    fn aged_stored_pending_is_reoffered_but_inflight_and_faulted_are_not() {
        let pending = BlockDisposition::Pending {
            block_number: height(8),
            stage: PendingStage::Stored,
            since_unix_ms: 1000,
        };
        assert!(!eligible_stored_pending(&pending, 60_999));
        assert!(eligible_stored_pending(&pending, 61_000));
        assert!(!eligible_stored_pending(&pending, 0));
        for stage in [PendingStage::Validating, PendingStage::Retry] {
            let in_flight = BlockDisposition::Pending {
                block_number: height(8),
                stage,
                since_unix_ms: 1000,
            };
            assert!(!eligible_stored_pending(&in_flight, 600_000));
        }
        let faulted = BlockDisposition::Faulted {
            block_number: height(8),
            stage: "validation".to_string(),
            reason: "injected".to_string(),
            since_unix_ms: 1000,
            last_attempt_unix_ms: 5000,
            attempts: MAX_FAULT_RETRIES,
        };
        assert!(!eligible_stored_pending(&faulted, 600_000));
    }

    async fn store() -> BlockDispositionStore {
        let manager = InMemoryStoreManager::default();
        let kv = database(
            &manager,
            "block-disposition",
            Arc::new(BlockHashCodec),
            Arc::new(BlockDispositionCodec),
        )
        .await
        .unwrap();
        BlockDispositionStore::from_store(Arc::new(kv))
    }

    /// An LMDB manager whose body and disposition databases share an environment, as the node's own
    /// mapping makes them (C350).
    fn shared_env_manager() -> rchain_shared::lmdb::LmdbDirStoreManager {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "rchain-disposition-c350-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        rchain_shared::lmdb::LmdbDirStoreManager::new(
            &dir,
            vec![
                (
                    rchain_shared::store_manager::Db::new(
                        rchain_block_storage::block_store::BLOCKS_DB,
                    ),
                    rchain_shared::store_manager::LmdbEnvConfig::new(
                        "blockstorage",
                        64 * 1024 * 1024,
                    ),
                ),
                (
                    rchain_shared::store_manager::Db::new(crate::storage::BLOCK_DISPOSITION_DB),
                    rchain_shared::store_manager::LmdbEnvConfig::new(
                        "blockstorage",
                        64 * 1024 * 1024,
                    ),
                ),
            ],
        )
    }

    /// **The body and its pending row are one write, and a fault outranks re-pending** (C350).
    ///
    /// Before this, the row was a second transaction in a second LMDB environment: a crash between
    /// the two left a stored body nothing owed. The pair is now committed together, and a row that
    /// already records a fault is kept — a re-delivery is not evidence the fault was handled.
    #[tokio::test]
    async fn the_writer_stores_the_body_and_its_row_together() {
        let manager = shared_env_manager();
        let writer = BodyDispositionWriter::new(manager.clone());
        let ledger = BlockDispositionStore::from_store(Arc::new(
            database(
                &manager,
                crate::storage::BLOCK_DISPOSITION_DB,
                Arc::new(BlockHashCodec),
                Arc::new(BlockDispositionCodec),
            )
            .await
            .expect("open the ledger"),
        ));
        let bodies = block_store::create(&manager)
            .await
            .expect("open the block store");
        let block = sample_block(hash(41));

        writer
            .store_body_and_mark_pending(&block, PendingStage::Stored)
            .await
            .expect("one write");
        assert_eq!(
            bodies.get(&[block.block_hash]).await.expect("read body"),
            vec![Some(block.clone())],
            "the body is in the store"
        );
        assert!(
            matches!(
                ledger.get(&block.block_hash).await.expect("read row"),
                Some(BlockDisposition::Pending {
                    stage: PendingStage::Stored,
                    ..
                })
            ),
            "and so is the obligation"
        );

        // A fault is a stronger record than a re-delivery's pending row; the body is still written.
        ledger
            .mark_faulted(block.block_hash, block.block_number, "retry", "boom")
            .await
            .expect("fault");
        writer
            .store_body_and_mark_pending(&block, PendingStage::Stored)
            .await
            .expect("re-delivery");
        assert!(matches!(
            ledger.get(&block.block_hash).await.expect("read row"),
            Some(BlockDisposition::Faulted { .. })
        ));
    }

    #[test]
    fn the_disposition_codec_round_trips_both_states() {
        let codec = BlockDispositionCodec;
        for disposition in [
            BlockDisposition::Pending {
                block_number: height(7),
                stage: PendingStage::Stored,
                since_unix_ms: 1_700_000_000_000,
            },
            BlockDisposition::Faulted {
                block_number: height(9),
                stage: "validation internal".to_string(),
                reason: "merge search budget refused".to_string(),
                since_unix_ms: 1_700_000_000_001,
                last_attempt_unix_ms: 1_700_000_000_002,
                attempts: 3,
            },
        ] {
            let bytes = codec.encode(&disposition);
            assert_eq!(codec.decode(&bytes).unwrap(), disposition);
        }
    }

    #[test]
    fn a_transient_gap_is_not_an_orphan_but_a_sustained_one_is() {
        let mut streak = OrphanStreak::default();
        assert_eq!(streak.observe(1), None, "one in-flight body is normal");
        assert_eq!(streak.observe(1), None);
        assert_eq!(
            streak.observe(1),
            Some(1),
            "the third consecutive sample is an orphan"
        );
        // A zero sample is evidence the normal case returned; the count starts again.
        assert_eq!(streak.observe(0), None);
        assert_eq!(streak.observe(1), None);
    }

    #[test]
    fn a_known_boot_gap_is_not_hidden_behind_the_periodic_sampling_window() {
        ORPHAN_BODIES.store(0, Ordering::Relaxed);
        // This exact shape occurred in the isolated legacy testnet snapshot.
        assert_eq!(observe_boot_orphan_gap(1947, 21), 1926);
        assert_eq!(orphan_bodies(), 1926);
        // One transient sample cannot clear a mismatch already found at boot.
        let mut streak = OrphanStreak::default();
        assert_eq!(streak.observe(1926), None);
        assert!(!clear_orphan_gate_if_resolved(1926));
        assert_eq!(orphan_bodies(), 1926);
        assert!(clear_orphan_gate_if_resolved(0));
        assert_eq!(orphan_bodies(), 0);
        assert_eq!(observe_boot_orphan_gap(21, 1947), 0);
        assert_eq!(orphan_bodies(), 0);
    }

    #[test]
    fn a_truncated_or_trailing_record_is_an_error_not_a_default() {
        let codec = BlockDispositionCodec;
        let bytes = codec.encode(&BlockDisposition::Pending {
            block_number: height(1),
            stage: PendingStage::Stored,
            since_unix_ms: 5,
        });
        assert!(codec.decode(&bytes[..bytes.len() - 1]).is_err());
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(codec.decode(&longer).is_err());
    }

    #[tokio::test]
    async fn a_pending_row_survives_redelivery_and_settles_only_on_a_verdict() {
        let store = store().await;
        let h = hash(1);
        store
            .mark_pending(h, height(3), PendingStage::Stored)
            .await
            .unwrap();
        let since = match store.get(&h).await.unwrap().unwrap() {
            BlockDisposition::Pending { since_unix_ms, .. } => since_unix_ms,
            other => panic!("expected a pending row, got {other:?}"),
        };
        store
            .mark_pending(h, height(3), PendingStage::Validating)
            .await
            .unwrap();
        match store.get(&h).await.unwrap().unwrap() {
            BlockDisposition::Pending {
                stage,
                since_unix_ms,
                ..
            } => {
                assert_eq!(stage, PendingStage::Validating);
                assert_eq!(
                    since_unix_ms, since,
                    "a re-delivery does not restart the clock"
                );
            }
            other => panic!("expected a pending row, got {other:?}"),
        }
        store.settle(h).await.unwrap();
        assert_eq!(store.get(&h).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_redelivered_faulted_block_is_not_downgraded_to_pending() {
        let store = store().await;
        let h = hash(4);
        store
            .mark_faulted(h, height(6), "internal", "boom")
            .await
            .unwrap();
        store
            .mark_pending(h, height(6), PendingStage::Stored)
            .await
            .unwrap();
        assert!(matches!(
            store.get(&h).await.unwrap(),
            Some(BlockDisposition::Faulted { attempts: 1, .. })
        ));
    }

    #[tokio::test]
    async fn faults_increment_and_preserve_the_first_sighting() {
        let store = store().await;
        let h = hash(2);
        store
            .mark_pending(h, height(4), PendingStage::Stored)
            .await
            .unwrap();
        assert_eq!(
            store
                .mark_faulted(h, height(4), "internal", "boom")
                .await
                .unwrap(),
            1
        );
        let attempts = store
            .mark_faulted(h, height(4), "internal", "boom again")
            .await
            .unwrap();
        assert_eq!(attempts, 2);
        match store.get(&h).await.unwrap().unwrap() {
            BlockDisposition::Faulted {
                attempts, reason, ..
            } => {
                assert_eq!(attempts, 2);
                assert_eq!(reason, "boom again");
            }
            other => panic!("expected a fault, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_retry_is_not_due_before_its_backoff_and_not_after_the_budget() {
        let store = store().await;
        let h = hash(3);
        store
            .mark_faulted(h, height(5), "insert", "store error")
            .await
            .unwrap();
        // The first attempt's last_attempt is now; nothing is due immediately.
        let now = now_unix_millis();
        assert!(store
            .due_retries(now.saturating_sub(FIRST_RETRY_DELAY_MS / 2))
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            store
                .due_retries(now.saturating_add(MAX_RETRY_DELAY_MS))
                .await
                .unwrap()
                .len(),
            1
        );
        for _ in 0..MAX_FAULT_RETRIES {
            store
                .mark_faulted(h, height(5), "insert", "store error")
                .await
                .unwrap();
        }
        assert!(store
            .due_retries(now.saturating_add(MAX_RETRY_DELAY_MS))
            .await
            .unwrap()
            .is_empty());
    }

    // --- the start-up audit: scan, re-offer, bounded drain (units 3–4) ---

    use rchain_block_storage::block_store::{self, BlockStore};
    use rchain_block_storage::dag::codecs::{BlockMetadataCodec, SignedDeployDataCodec};
    use rchain_block_storage::dag::dag_storage::DeployId;
    use rchain_models::block::state_hash::StateHash;
    use rchain_models::casper::protocol::casper_message::{RholangState, SignedDeployData};
    use rchain_models::validator::Validator;
    use rchain_shared::refined::SeqNum;
    use rchain_shared::store::InMemoryKeyValueStore;
    use rchain_shared::typed_store::{BytesCodec, KeyValueTypedStoreCodec, SharedStore};
    use std::collections::{BTreeMap, BTreeSet};

    use crate::block_metadata_store::BlockMetadataStore as CasperMetadataStore;
    use crate::dag::BlockDagKeyValueStorage;
    use crate::engine::node_phase::NodePhaseCell;

    fn in_memory() -> SharedStore {
        Arc::new(tokio::sync::Mutex::new(Box::new(
            InMemoryKeyValueStore::default(),
        )))
    }

    fn sample_block(hash: BlockHash) -> BlockMessage {
        BlockMessage {
            version: 1,
            shard_id: "root".to_string(),
            block_hash: hash,
            block_number: height(3),
            sender: Validator::new([0x11; 65]),
            seq_num: SeqNum::try_from(3i64).expect("seq"),
            pre_state_hash: StateHash::new([1u8; 32]),
            post_state_hash: StateHash::new([2u8; 32]),
            justifications: Vec::new(),
            bonds: BTreeMap::new(),
            rejected_deploys: BTreeSet::new(),
            rejected_blocks: BTreeSet::new(),
            rejected_senders: BTreeSet::new(),
            state: RholangState::default(),
            sig_algorithm: "secp256k1".to_string(),
            sig: vec![1],
            timestamp: 0,
        }
    }

    async fn empty_dag() -> Arc<dyn BlockDagStorage> {
        let metadata = CasperMetadataStore::create(Arc::new(KeyValueTypedStoreCodec::new(
            in_memory(),
            Arc::new(BlockHashCodec),
            Arc::new(BlockMetadataCodec),
        )))
        .await
        .expect("metadata store");
        let deploy_index: Arc<dyn KeyValueTypedStore<DeployId, BlockHash>> =
            Arc::new(KeyValueTypedStoreCodec::new(
                in_memory(),
                Arc::new(BytesCodec),
                Arc::new(BlockHashCodec),
            ));
        let deploy_store: Arc<dyn KeyValueTypedStore<DeployId, SignedDeployData>> =
            Arc::new(KeyValueTypedStoreCodec::new(
                in_memory(),
                Arc::new(BytesCodec),
                Arc::new(SignedDeployDataCodec),
            ));
        let markers: Arc<dyn KeyValueTypedStore<Vec<u8>, Vec<u8>>> = Arc::new(
            KeyValueTypedStoreCodec::new(in_memory(), Arc::new(BytesCodec), Arc::new(BytesCodec)),
        );
        Arc::new(
            BlockDagKeyValueStorage::create(Arc::new(metadata), deploy_index, deploy_store)
                .await
                .expect("dag")
                .with_reset_marker(markers),
        )
    }

    async fn block_store_with(block: &BlockMessage) -> BlockStore {
        let store = block_store::create(&InMemoryStoreManager::default())
            .await
            .expect("block store");
        store
            .put(&[(block.block_hash, block.clone())])
            .await
            .expect("store body");
        store
    }

    /// **A stored body with no DAG entry is re-offered** rather than left for the next restart.
    #[tokio::test]
    async fn a_stored_body_without_a_dag_entry_is_reoffered() {
        let block = sample_block(hash(20));
        let block_store = block_store_with(&block).await;
        let ledger = store().await;
        ledger
            .mark_pending(block.block_hash, block.block_number, PendingStage::Stored)
            .await
            .expect("pending row");
        let dag = empty_dag().await;
        let (tx, mut rx) = mpsc::channel(4);
        let log: Arc<dyn Log> = Arc::new(rchain_shared::log::NopLog);
        let report = reoffer_unsettled(ledger, block_store, dag, tx, log)
            .await
            .expect("re-offer");
        assert_eq!(report.offered, vec![block.block_hash]);
        assert_eq!(report.reoffered, 1);
        let received = rx.try_recv().expect("the processor receives the body");
        assert_eq!(received.block_hash, block.block_hash);
    }

    #[tokio::test]
    async fn the_startup_drain_waits_for_reoffered_rows_but_not_for_missing_parents() {
        let ledger = store().await;
        let offered = hash(21);
        let waiting = hash(22);
        ledger
            .mark_pending(offered, height(3), PendingStage::Stored)
            .await
            .expect("offered row");
        ledger
            .mark_pending(waiting, height(4), PendingStage::Stored)
            .await
            .expect("waiting row");
        let settler = ledger.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            settler.settle(offered).await.expect("settle");
        });
        let log: Arc<dyn Log> = Arc::new(rchain_shared::log::NopLog);
        let outcome =
            wait_for_reoffered_rows(&ledger, &[offered], Duration::from_secs(2), &log).await;
        assert!(!outcome.timed_out);
        assert_eq!(outcome.settled, 1);
        assert_eq!(outcome.still_pending, 0);
        assert!(
            ledger.get(&waiting).await.expect("read").is_some(),
            "a row that was not offered (missing parent) never holds the drain"
        );
    }

    #[tokio::test]
    async fn a_reoffered_row_that_faults_does_not_hold_the_drain() {
        let ledger = store().await;
        let faulted = hash(23);
        ledger
            .mark_faulted(faulted, height(3), "internal", "boom")
            .await
            .expect("faulted row");
        let log: Arc<dyn Log> = Arc::new(rchain_shared::log::NopLog);
        let outcome =
            wait_for_reoffered_rows(&ledger, &[faulted], Duration::from_millis(50), &log).await;
        assert!(!outcome.timed_out, "a fault is the retry loop's question");
        assert_eq!(outcome.still_pending, 0);
    }

    /// **The one-time scan names pre-ledger bodies without decoding the whole store** (U19/F-U19-07).
    /// It adopts each orphan as `Pending`, writes its marker, and a second pass is a no-op.
    #[tokio::test]
    async fn the_legacy_scan_adopts_every_unaccounted_body_and_writes_its_marker() {
        let block = sample_block(hash(30));
        let block_store = block_store_with(&block).await;
        let ledger = store().await;
        let dag = empty_dag().await;
        let log: Arc<dyn Log> = Arc::new(rchain_shared::log::NopLog);

        let report = scan_legacy_bodies(&ledger, &block_store, &dag, &log)
            .await
            .expect("scan");
        assert!(report.scanned);
        assert_eq!(report.keys, 1);
        assert_eq!(report.orphans, 1);
        assert_eq!(report.adopted, 1);
        assert_eq!(report.reported_hashes, vec![block.block_hash]);
        assert!(report.marker_written);
        assert!(
            ledger.get(&block.block_hash).await.expect("row").is_some(),
            "an orphan body is adopted as a visible row"
        );
        assert_eq!(
            dag.read_legacy_body_scan_marker().await.expect("marker"),
            Some(rchain_block_storage::dag::dag_storage::LEGACY_BODY_SCAN_VERSION)
        );

        let again = scan_legacy_bodies(&ledger, &block_store, &dag, &log)
            .await
            .expect("second scan");
        assert!(!again.scanned, "the marker skips the walk");
        assert_eq!(again.adopted, 0);

        // Simulate the post-marker crash window: the body survives but its
        // disposition row is missing. A runtime scan must adopt it again,
        // without rewriting the one-time legacy marker or bypassing validation.
        ledger.settle(block.block_hash).await.expect("remove row");
        let recovery = scan_unaccounted_bodies(&ledger, &block_store, &dag, &log, true)
            .await
            .expect("runtime scan");
        assert!(recovery.scanned);
        assert_eq!(recovery.adopted, 1);
        assert!(!recovery.marker_written);
        assert!(matches!(
            ledger.get(&block.block_hash).await.expect("recovered row"),
            Some(BlockDisposition::Pending {
                stage: PendingStage::Stored,
                ..
            })
        ));
    }

    /// **The phase does not move before the drain closes** (Law 88a's `startup`).
    #[tokio::test]
    async fn the_phase_stays_starting_until_the_drain_closes() {
        let block = sample_block(hash(24));
        let block_store = block_store_with(&block).await;
        let ledger = store().await;
        ledger
            .mark_pending(block.block_hash, block.block_number, PendingStage::Stored)
            .await
            .expect("pending row");
        let dag = empty_dag().await;
        let (tx, _rx) = mpsc::channel(4);
        let phase = NodePhaseCell::new();
        assert_eq!(
            phase.phase(),
            crate::engine::node_phase::NodePhase::Starting
        );

        let settler = ledger.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            settler.settle(block.block_hash).await.expect("settle");
        });
        let runner = phase.clone();
        let log: Arc<dyn Log> = Arc::new(rchain_shared::log::NopLog);
        let task = tokio::spawn(async move {
            startup_audit_then_running(
                &runner,
                &ledger,
                &block_store,
                &dag,
                &tx,
                Duration::from_secs(2),
                &log,
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            phase.phase(),
            crate::engine::node_phase::NodePhase::Starting,
            "the phase must not move before the re-offered row settles"
        );
        let outcome = task.await.expect("audit task");
        assert!(phase.is_running(), "the audit closes into Running");
        assert_eq!(outcome.drain.still_pending, 0);
    }

    // --- the runtime reader for stored pending bodies (Law 88c) ---

    /// **The overflow half.** The receiver's capacity refusal stores the body and writes the
    /// `Pending` row before it gives the in-memory claim up; nothing in the runtime used to read
    /// that row again. Here the state is the post-refusal one (no claim), the row is 60 s old, and
    /// the sweep must dispatch it — the assertion fails on the pre-fix tree because no sweep
    /// exists.
    #[tokio::test]
    async fn a_refused_stored_body_is_reoffered_by_the_runtime_sweep() {
        let block = sample_block(hash(40));
        let block_store = block_store_with(&block).await;
        let ledger = store().await;
        ledger
            .mark_pending(block.block_hash, block.block_number, PendingStage::Stored)
            .await
            .expect("pending row");
        let dag = empty_dag().await;
        // Exactly the state `abort_stored` leaves: no in-memory claim.
        let receiver = tokio::sync::Mutex::new(BlockReceiverState::<BlockHash>::new());
        let (tx, mut rx) = mpsc::channel(4);
        let log: Arc<dyn Log> = Arc::new(rchain_shared::log::NopLog);
        let since = now_unix_millis() + PENDING_PARENT_RECHECK_AGE_MS;

        let dispatched = reoffer_ready_stored_pending_once(
            &ledger,
            &block_store,
            &dag,
            &receiver,
            &tx,
            since,
            log.as_ref(),
        )
        .await
        .expect("sweep");
        assert_eq!(dispatched, 1, "the orphan is handed to the processor");
        assert_eq!(rx.try_recv().expect("body").block_hash, block.block_hash);
        assert!(matches!(
            ledger.get(&block.block_hash).await.expect("row"),
            Some(BlockDisposition::Pending {
                stage: PendingStage::Retry,
                ..
            })
        ));
    }

    /// **The restart half.** A body whose parent is still absent is not dispatched, and the row
    /// stays `Pending` — the obligation survives, so a later sweep (when the parent arrives) can
    /// take it.
    #[tokio::test]
    async fn a_pending_body_waits_for_its_parent() {
        let mut block = sample_block(hash(41));
        block.justifications.push(hash(99));
        let block_store = block_store_with(&block).await;
        let ledger = store().await;
        ledger
            .mark_pending(block.block_hash, block.block_number, PendingStage::Stored)
            .await
            .expect("pending row");
        let dag = empty_dag().await;
        let receiver = tokio::sync::Mutex::new(BlockReceiverState::<BlockHash>::new());
        let (tx, mut rx) = mpsc::channel(4);
        let log: Arc<dyn Log> = Arc::new(rchain_shared::log::NopLog);
        let since = now_unix_millis() + PENDING_PARENT_RECHECK_AGE_MS;

        let dispatched = reoffer_ready_stored_pending_once(
            &ledger,
            &block_store,
            &dag,
            &receiver,
            &tx,
            since,
            log.as_ref(),
        )
        .await
        .expect("sweep");
        assert_eq!(dispatched, 0);
        assert!(rx.try_recv().is_err(), "nothing is dispatched");
        assert!(matches!(
            ledger.get(&block.block_hash).await.expect("row"),
            Some(BlockDisposition::Pending {
                stage: PendingStage::Stored,
                ..
            })
        ));
    }

    /// The row is moved in flight before dispatch, so a second sweep does not duplicate it.
    #[tokio::test]
    async fn the_sweep_is_idempotent() {
        let block = sample_block(hash(42));
        let block_store = block_store_with(&block).await;
        let ledger = store().await;
        ledger
            .mark_pending(block.block_hash, block.block_number, PendingStage::Stored)
            .await
            .expect("pending row");
        let dag = empty_dag().await;
        let receiver = tokio::sync::Mutex::new(BlockReceiverState::<BlockHash>::new());
        let (tx, mut rx) = mpsc::channel(4);
        let log: Arc<dyn Log> = Arc::new(rchain_shared::log::NopLog);
        let since = now_unix_millis() + PENDING_PARENT_RECHECK_AGE_MS;

        for expected in [1, 0] {
            let dispatched = reoffer_ready_stored_pending_once(
                &ledger,
                &block_store,
                &dag,
                &receiver,
                &tx,
                since,
                log.as_ref(),
            )
            .await
            .expect("sweep");
            assert_eq!(dispatched, expected);
        }
        assert_eq!(
            rx.try_recv().expect("one dispatch").block_hash,
            block.block_hash
        );
        assert!(rx.try_recv().is_err(), "the second sweep sent nothing");
    }
}
