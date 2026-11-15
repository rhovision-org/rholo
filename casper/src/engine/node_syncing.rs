//! Node syncing state machine (port of `engine/NodeSyncing.scala`).
//!
//! Drives the Last Finalized State sync (blocks + tuple space) from the bootstrap node: it
//! accepts the finalized fringe from the bootstrap, runs the `LfsBlockRequester` and
//! `LfsTupleSpaceRequester` streams, then populates the DAG from the received blocks.

use rchain_shared::chan;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use rchain_block_storage::approved_store::{
    ApprovedStore, FINALIZED_FRINGE_KEY, RECOVERY_PENDING_KEY,
};
use rchain_block_storage::block_store::BlockStore;
use rchain_block_storage::dag::dag_storage::BlockDagStorage;
use rchain_comm::peer_node::PeerNode;
use rchain_comm::rp::rp_conf::RPConf;
use rchain_comm::transport::transport_layer::TransportLayer;
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_models::block::state_hash::StateHash;
use rchain_models::block_hash::BlockHash;
use rchain_models::block_metadata::BlockMetadata;
use rchain_models::casper::protocol::casper_message::{
    BlockFringe, BlockMessage, BlockMetadataMessage, CasperMessage, FinalizedFringe,
    StoreItemsMessage,
};
use rchain_rspace::state::RSpaceImporter;
use rchain_shared::lock::Unpoison;
use rchain_shared::log::{Log, LogSource};

use super::lfs_block_requester::BlockRequestLog;
use super::lfs_tuple_space_requester::{
    request_tuple_space, request_tuple_space_roots, REQUEST_TIMEOUT,
};
use super::recovery::RecoveryReplay;
use crate::checkpoint::{check_bond_set_unchanged, verify_genesis_rooted_certificate};
use crate::protocol::comm_util::CommUtil;
use crate::validator_identity::ValidatorIdentity;

/// The node-syncing engine (port of the `NodeSyncing` class).
pub struct NodeSyncing<I: RSpaceImporter> {
    transport: Arc<dyn TransportLayer>,
    conf: RPConf,
    block_store: BlockStore,
    dag: Arc<dyn BlockDagStorage>,
    approved_store: ApprovedStore,
    comm_util: Arc<CommUtil>,
    log: Arc<dyn Log>,
    log_source: LogSource,
    #[allow(dead_code)] // reserved (Scala stores it; unused in the syncing path)
    validator_id: Option<ValidatorIdentity>,
    #[allow(dead_code)] // reserved (Scala stores it; consumed by the caller)
    trim_state: bool,
    importer: Option<I>,
    replay: Option<RecoveryReplay>,
    /// **The certificate the sync target was chosen from** (sync/verification separation). A joiner
    /// picks the certified anchor *before* asking for the fringe; re-fetching a certificate at sync
    /// time can name a newer anchor than the served fringe, and the two must be the same statement.
    prefetched_certificate:
        Option<rchain_models::casper::protocol::checkpoint::CheckpointCertificate>,
    incoming_blocks_tx: tokio::sync::mpsc::Sender<BlockMessage>,
    incoming_blocks_rx: Option<tokio::sync::mpsc::Receiver<BlockMessage>>,
    /// **The per-hash block-request log** (#457): which peers each outstanding hash was requested
    /// from (written by the walk) and which delivered it (written here, where the sending `PeerNode`
    /// is known). Cleared at each attempt's start. The give-up error reports the difference.
    block_requests: Arc<tokio::sync::Mutex<BlockRequestLog>>,
    tuple_space_tx: tokio::sync::mpsc::Sender<StoreItemsMessage>,
    tuple_space_rx: Option<tokio::sync::mpsc::Receiver<StoreItemsMessage>>,
    /// Metadata answers for the header walk (sync/verification separation). A sibling of the block
    /// and tuple-space paths: the walker owns the receiver, this engine forwards what arrives.
    metadata_tx: tokio::sync::mpsc::Sender<BlockMetadataMessage>,
    metadata_rx: Option<tokio::sync::mpsc::Receiver<BlockMetadataMessage>>,
    start_requester: bool,
    initial_fringe_received: Arc<tokio::sync::Notify>,
    finished: Arc<tokio::sync::Notify>,
    /// **The shared latest-fringe slot** (AUDIT C181, #125). The retry a failed attempt needs is a
    /// *newer* fringe to sync to, and this is where it comes from: `on_finalized_fringe_message`
    /// writes every bootstrap answer here, so a later one is kept rather than discarded, and the
    /// spawned task reads it between attempts.
    latest_fringe: Arc<tokio::sync::Mutex<Option<FinalizedFringe>>>,
    /// Signalled whenever the slot above is written, so the retry wakes on a new fringe instead of
    /// polling for one.
    fringe_arrived: Arc<tokio::sync::Notify>,
    /// **The terminal action's signal** (AUDIT C181). `finished` means "the state was restored";
    /// this means "the retries are spent and this node cannot be synced" — a state the protocol can
    /// otherwise never leave, which is the defect. It is a signal rather than a `std::process::exit`
    /// so the *caller* owns what to do, and so an exit inside the sync task cannot take a test
    /// binary with it.
    terminal: Arc<tokio::sync::Notify>,
}

/// How many times one sync attempt is retried before the node gives up and signals terminal.
///
/// The count is a bound and not a policy: each attempt already gives up on its own after
/// `lfs_block_requester::MAX_IDLE_ROUNDS` idle rounds, so this caps the *retries*, and the total
/// wait is bounded by the product of the two. **Ten rather than three** because the certified path
/// fails fast when the bootstrap has not yet assembled a certificate (its collector ticks every two
/// seconds after a finality advance), and a loaded host can take a minute to reach that point: three
/// attempts spent before the first certificate exists would stop a joiner that had done nothing
/// wrong. The terminal state — and its operator signal — is unchanged.
pub const MAX_SYNC_ATTEMPTS: u32 = 10;

/// How long a retry waits for a newer fringe before re-attempting the one it has.
///
/// A new fringe is the trigger worth waiting for — a later bootstrap answer names a state nearer the
/// tip — but waiting for one *only* would turn a bootstrap that has stopped answering into the same
/// latch this fixes, one level up.
const SYNC_RETRY_DELAY: Duration = Duration::from_secs(10);

/// The sync task's current state, or `None` when it is not syncing.
///
/// **`None` is the ordinary case**: it is written while the retry loop is running and **cleared when
/// an attempt restores the state**, so a non-`None` reading means "this node is in `NodeSyncing`
/// right now" rather than "this node once synced". A running node therefore reports nothing here,
/// which is the honest reading — the sync task has returned.
///
/// **Why a surface at all.** The attempt count and the failure reason lived only in the task's own
/// locals and in a line on stderr, so a node whose sync was looping — or whose retries had been spent
/// and had stopped serving — answered `GET /api/v1/status` exactly like a healthy one. The measured
/// incident (`spec/audit/evidence/n-anchor-drill/c268-joiner-page-walk-loop.txt`) was joiners at
/// height 0 for ten minutes with no way for a probe to ask why. This is the writer half;
/// `BlockApi::sync_health` reads it, so the log line and the status surface carry the same fact.
static SYNC_STATE: std::sync::Mutex<Option<SyncState>> = std::sync::Mutex::new(None);

/// What the LFS sync task is doing, as this process last observed it — the reading [`sync_state`]
/// returns and [`crate::api::block_api::SyncHealth`] republishes on the status surface.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncState {
    /// Attempts **started**. Counted when an attempt begins rather than when it fails, so a reading
    /// taken while one is in flight names the attempt now running rather than the last one that ended.
    /// This is *not* the count the retry bound uses, which is a failure count (`attempts` in the loop
    /// below); the two agree at every failure and differ only mid-attempt.
    pub attempts: u32,
    /// The last named failure's message, verbatim — the same `e` the retry loop logs. `None` while the
    /// attempt the count names has not failed (and before any attempt has).
    pub last_failure: Option<String>,
}

/// Record that an attempt has **started**, and drop the previous one's failure.
///
/// The failure goes with the count it belonged to: `attempts` and `last_failure` are read as a pair,
/// and a reason left beside a *newer* count would report attempt N failing with attempt N-1's error —
/// a worse lie than reporting no reason at all.
fn note_sync_attempt(started: u32) {
    *SYNC_STATE.lock().unpoison() = Some(SyncState {
        attempts: started,
        last_failure: None,
    });
}

/// Record the failure of the attempt the current reading names.
fn note_sync_failure(message: &str) {
    let mut state = SYNC_STATE.lock().unpoison();
    if let Some(state) = state.as_mut() {
        state.last_failure = Some(message.to_string());
    }
}

/// Clear the reading: the sync task has stopped — the state was restored, or the retries were spent —
/// so this node is not syncing and has nothing to report here.
fn clear_sync_state() {
    *SYNC_STATE.lock().unpoison() = None;
}

/// The LFS sync task's current state, as this process last observed it (the reader half; see
/// [`SYNC_STATE`]).
///
/// `None` means the task is not running — **not** that the last sync succeeded, which this accessor
/// cannot see and does not claim.
pub fn sync_state() -> Option<SyncState> {
    SYNC_STATE.lock().unpoison().clone()
}

impl<I: RSpaceImporter + Send + 'static> NodeSyncing<I> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        transport: Arc<dyn TransportLayer>,
        conf: RPConf,
        block_store: BlockStore,
        dag: Arc<dyn BlockDagStorage>,
        approved_store: ApprovedStore,
        comm_util: Arc<CommUtil>,
        log: Arc<dyn Log>,
        validator_id: Option<ValidatorIdentity>,
        trim_state: bool,
        importer: I,
    ) -> Self {
        let (incoming_blocks_tx, incoming_blocks_rx) = tokio::sync::mpsc::channel(50);
        let (tuple_space_tx, tuple_space_rx) = tokio::sync::mpsc::channel(50);
        let (metadata_tx, metadata_rx) = tokio::sync::mpsc::channel(50);
        NodeSyncing {
            transport,
            conf,
            block_store,
            dag,
            approved_store,
            comm_util,
            log,
            log_source: LogSource::new("casper.engine.NodeSyncing"),
            validator_id,
            trim_state,
            importer: Some(importer),
            replay: None,
            prefetched_certificate: None,
            incoming_blocks_tx,
            incoming_blocks_rx: Some(incoming_blocks_rx),
            block_requests: Arc::new(tokio::sync::Mutex::new(BlockRequestLog::default())),
            tuple_space_tx,
            tuple_space_rx: Some(tuple_space_rx),
            metadata_tx,
            metadata_rx: Some(metadata_rx),
            start_requester: true,
            initial_fringe_received: Arc::new(tokio::sync::Notify::new()),
            finished: Arc::new(tokio::sync::Notify::new()),
            latest_fringe: Arc::new(tokio::sync::Mutex::new(None)),
            fringe_arrived: Arc::new(tokio::sync::Notify::new()),
            terminal: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// A sync without a replay context fails closed before publishing any DAG metadata.
    pub fn with_replay(mut self, replay: RecoveryReplay) -> Self {
        self.replay = Some(replay);
        self
    }

    /// Record the certificate the launch phase used to choose the certified anchor. The sync task
    /// verifies **this** statement rather than fetching a newer one: the fringe it is about to walk
    /// was requested for this anchor.
    pub fn set_checkpoint_certificate(
        &mut self,
        certificate: rchain_models::casper::protocol::checkpoint::CheckpointCertificate,
    ) {
        self.prefetched_certificate = Some(certificate);
    }

    /// A future that completes when syncing finishes (port of `finished.get`).
    pub async fn wait(&self) {
        self.finished.notified().await;
    }

    /// Signalled once, only after bootstrap identity and nonempty-target admission.
    pub fn initial_fringe_received_handle(&self) -> Arc<tokio::sync::Notify> {
        self.initial_fringe_received.clone()
    }

    /// A cloneable handle to the syncing-finished notification, for waiting concurrently with the
    /// `handle` loop without holding the engine's mutex.
    pub fn finished_handle(&self) -> Arc<tokio::sync::Notify> {
        self.finished.clone()
    }

    /// A cloneable handle to the **terminal** signal: the retries are spent and this node cannot be
    /// synced (AUDIT C181). `node_launch` selects on it beside `finished`, because without it the
    /// node sits in `NodeSyncing` for good with a serving API — the state C68's sequencing was
    /// careful to keep *out* of `NodeRunning` but never gave a way *out of*.
    ///
    /// **Deliberately not `finished`.** That one means "the state was restored", and firing it here
    /// would move the node into `NodeRunning` on a partial DAG, which is the outcome C68 exists to
    /// prevent. The two are different facts and they get different signals.
    pub fn terminal_handle(&self) -> Arc<tokio::sync::Notify> {
        self.terminal.clone()
    }

    /// Handle an incoming casper message (port of `handle`).
    pub async fn handle(&mut self, peer: &PeerNode, msg: &CasperMessage) -> Result<(), String> {
        match msg {
            CasperMessage::FinalizedFringe(fringe) => {
                self.on_finalized_fringe_message(peer, fringe).await
            }
            CasperMessage::StoreItemsMessage(s) => {
                self.log.info(
                    self.log_source,
                    &format!(
                        "Received StoreItems(history: {}, data: {}) from {peer}.",
                        s.history_items.len(),
                        s.data_items.len()
                    ),
                );
                chan::send(&self.tuple_space_tx, s.clone()).await;
                Ok(())
            }
            CasperMessage::BlockMetadataMessage(m) => {
                chan::send(&self.metadata_tx, m.clone()).await;
                Ok(())
            }
            CasperMessage::BlockMessage(b) => {
                self.log.info(
                    self.log_source,
                    &format!("BlockMessage received #{} from {peer}.", b.block_number),
                );
                // Record which hash this peer delivered before the payload loses its source
                // (#457's diagnostic). An answer for a different hash does not clear another's entry.
                self.block_requests
                    .lock()
                    .await
                    .record_answer(&b.block_hash, &peer.id);
                chan::send(&self.incoming_blocks_tx, b.clone()).await;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Handle a finalized-fringe message, starting the LFS sync once from the bootstrap node (port
    /// of `onFinalizedFringeMessage`).
    async fn on_finalized_fringe_message(
        &mut self,
        sender: &PeerNode,
        fringe: &FinalizedFringe,
    ) -> Result<(), String> {
        // The transport binds this id to the client's certificate (C115). Host and ports are
        // mutable routing metadata, not authority: a hostname/IP or advertised-port change must
        // not lock out the same bootstrap key (C334, #386). Reject other keys before touching the
        // retry target or one-shot latch; only a trusted sender's empty fringe is an announcement.
        let sender_is_bootstrap = self
            .conf
            .bootstrap
            .as_ref()
            .map(|b| b.id == sender.id)
            .unwrap_or(false);
        if !sender_is_bootstrap {
            let expected = self
                .conf
                .bootstrap
                .as_ref()
                .map(|b| b.id.to_string())
                .unwrap_or_else(|| "<not configured>".to_string());
            self.log.warn(
                self.log_source,
                &format!(
                    "Fringe message ignored: sender {sender} does not match bootstrap node id \
                     {expected}."
                ),
            );
            return Ok(());
        }

        // **A fringe with no hashes names no state to sync to, and it must not consume the one-shot
        // trigger** (issue #100). The genesis master broadcasts exactly that — `hashes: Vec::new()`
        // paired with the genesis *pre*-state hash (`node_launch.rs`'s `create_store_broadcast_genesis`)
        // — as an announcement that the approved state is the genesis, not as a sync target. On a fresh
        // network that broadcast can arrive before the answer to our own request: measured on a devnet,
        // it landed 34 ms after the master sent it and 83 ms *before* the master had even seen the
        // request, so the node consumed the trigger on it, "restored" nothing, and discarded the
        // correct answer in silence (a later fringe from the bootstrap logs nothing at all).
        if fringe.hashes.is_empty() {
            self.log.info(
                self.log_source,
                &format!(
                    "Ignoring an empty fringe from {sender}: it is the genesis master's announcement, \
                     not an answer to our request. Still waiting for a fringe that names a block \
                     to sync to."
                ),
            );
            return Ok(());
        }

        // **Every bootstrap answer reaches the slot, not only the first** (AUDIT C181, #125). The
        // old code kept the fringe only when it was about to start a request, so a later — and
        // necessarily *newer* — answer was dropped in silence, which is precisely the thing a retry
        // has to have. The slot is written before the start check, so a fringe arriving mid-attempt
        // is already there when the attempt fails and the loop looks for one.
        {
            let mut slot = self.latest_fringe.lock().await;
            let is_new = slot.as_ref() != Some(fringe);
            *slot = Some(fringe.clone());
            drop(slot);
            if is_new {
                self.fringe_arrived.notify_waiters();
            }
        }

        // The task is spawned once; the *retry* lives inside it, because the importer and both
        // receivers are moved into it and nothing downstream can hand them back (AUDIT C181).
        let start = self.start_requester;
        self.start_requester = false;
        if start {
            self.initial_fringe_received.notify_one();
        }

        // **Every bootstrap fringe is logged, not only the one that starts an attempt.** This line
        // used to live inside the `if start` branch, so the second and later answers produced no log
        // line at all — the repo's own note calls that "a later fringe from the bootstrap logs nothing
        // at all", and it is half of the silence this row is about: an operator cannot see a fringe
        // that was received and dropped (AUDIT C181).
        //
        // **The oracle puts the state hash in the parentheses and the hashes after**
        // (`NodeSyncing.scala`: `s"Received finalized fringe from bootstrap node
        // ($fringeStateHashStr) $fringeHashesStr."`). The port had the hashes in the parentheses and
        // the state hash nowhere — and the state hash is the one field that tells the genesis master's
        // *announcement* from the *answer* to a request, because the announcement carries the genesis
        // **pre**-state and the response the genesis **post**-state. Diagnosing issue #100 meant
        // distinguishing exactly those two, and this line could not show it.
        self.log.info(
            self.log_source,
            &format!(
                "Received finalized fringe from bootstrap node ({}) {}.",
                rchain_shared::base16::encode(fringe.state_hash.as_bytes()),
                fringe
                    .hashes
                    .iter()
                    .map(|h| h.to_hex())
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        );

        if start {
            // Spawn the LFS sync in the background. Awaiting it here deadlocks: the sync drains
            // `tuple_space_rx`/`incoming_blocks_rx`, which are only fed by `handle` (the
            // StoreItemsMessage/BlockMessage branches) running in this same dispatch loop, which is
            // currently blocked inside this call. Spawning lets `handle` return and keep routing.
            let transport = self.transport.clone();
            let conf = self.conf.clone();
            let block_store = self.block_store.clone();
            let dag = self.dag.clone();
            let approved_store = self.approved_store.clone();
            let comm_util = self.comm_util.clone();
            let log = self.log.clone();
            let finished = self.finished.clone();
            let latest_fringe = self.latest_fringe.clone();
            let fringe_arrived = self.fringe_arrived.clone();
            let terminal = self.terminal.clone();
            let importer = match self.importer.take() {
                Some(importer) => importer,
                None => {
                    self.log.error(
                        self.log_source,
                        "LFS sync requested twice; importer already taken",
                    );
                    return Err("LFS sync already started: importer already taken".to_string());
                }
            };
            let incoming_blocks_rx = match self.incoming_blocks_rx.take() {
                Some(rx) => rx,
                None => {
                    self.log.error(
                        self.log_source,
                        "LFS sync requested twice; incoming-blocks receiver already taken",
                    );
                    return Err(
                        "LFS sync already started: incoming-blocks receiver already taken"
                            .to_string(),
                    );
                }
            };
            let tuple_space_rx = match self.tuple_space_rx.take() {
                Some(rx) => rx,
                None => {
                    self.log.error(
                        self.log_source,
                        "LFS sync requested twice; tuple-space receiver already taken",
                    );
                    return Err(
                        "LFS sync already started: tuple-space receiver already taken".to_string(),
                    );
                }
            };
            let metadata_rx = match self.metadata_rx.take() {
                Some(rx) => rx,
                None => {
                    self.log.error(
                        self.log_source,
                        "LFS sync requested twice; metadata receiver already taken",
                    );
                    return Err(
                        "LFS sync already started: metadata receiver already taken".to_string()
                    );
                }
            };
            // **The retry lives inside the task, because nothing outside it can re-arm the attempt.**
            // The importer and both receivers are moved in here and have no inverse: `handle` holds
            // only the senders, `self` never reaches the task, and `finished` carries no payload.
            // So the loop takes them by `&mut` across attempts instead of re-taking them (AUDIT C181).
            let block_requests = self.block_requests.clone();
            let replay = self.replay.clone();
            let prefetched_certificate = self.prefetched_certificate.clone();
            tokio::spawn(async move {
                let source = LogSource::new("casper.engine.NodeSyncing");
                let mut importer = importer;
                let mut incoming_blocks_rx = incoming_blocks_rx;
                let mut tuple_space_rx = tuple_space_rx;
                let mut metadata_rx = metadata_rx;
                let mut attempts: u32 = 0;
                // The attempt count the *status surface* names, separate from `attempts` above, which
                // is the retry bound's failure count. They agree at every failure and differ only
                // while an attempt is in flight — exactly the window a reader asking "is this node
                // trying right now" is asking about.
                let mut started: u32 = 0;

                loop {
                    // The target is re-read every attempt: a later bootstrap answer is newer, and
                    // syncing to the newest known state is the point of retrying at all.
                    let target = latest_fringe.lock().await.clone();
                    let Some(target) = target else {
                        // Nothing to sync to at all. This is not a failure to signal terminal for:
                        // the slot is written before the task is spawned, so it is unreachable in
                        // practice, and a node with no fringe is one that never received a request.
                        log.error(source, "LFS sync task started with no fringe to sync to");
                        // A returning task must not leave a reading behind: `None` here means "not
                        // syncing", and this task is about to stop.
                        clear_sync_state();
                        return;
                    };

                    started += 1;
                    note_sync_attempt(started);
                    // A new attempt starts from an empty log: the diagnostic names the peers that
                    // answered *this* walk, not one that helped a previous one.
                    *block_requests.lock().await = BlockRequestLog::default();

                    let outcome = run_approved_state_sync(
                        &target,
                        transport.clone(),
                        conf.clone(),
                        block_store.clone(),
                        dag.clone(),
                        comm_util.clone(),
                        log.clone(),
                        &mut importer,
                        &mut incoming_blocks_rx,
                        &mut tuple_space_rx,
                        &mut metadata_rx,
                        prefetched_certificate.as_ref(),
                        replay.as_ref(),
                        &approved_store,
                    )
                    .await;
                    let outcome = match outcome {
                        Ok(()) => {
                            report_restored_checkpoint(&approved_store, &target, log.as_ref()).await
                        }
                        Err(error) => Err(error),
                    };

                    match &outcome {
                        Ok(()) => {
                            notify_when_restored(&outcome, &finished);
                            // The state is restored, so this task is returning and the node is no
                            // longer syncing: the reading is cleared rather than left naming the loop
                            // that is now over.
                            clear_sync_state();
                            return;
                        }
                        Err(e) => {
                            attempts += 1;
                            // The reason travels with the count: the status surface and the terminal
                            // line both read it, and it is the same `e` the log below formats.
                            note_sync_failure(e);
                            log.error(
                                source,
                                &format!(
                                    "LFS state sync failed (attempt {attempts} of \
                                     {MAX_SYNC_ATTEMPTS}): {e}"
                                ),
                            );
                            if attempts >= MAX_SYNC_ATTEMPTS {
                                // **The terminal state** (AUDIT C181). The retries are spent and this
                                // node cannot reach the approved state, so it stops rather than
                                // serving from a DAG it never populated. The signal is the caller's,
                                // not a `process::exit` here, so the test binary survives and the
                                // node owns the policy.
                                log.error(
                                    source,
                                    "LFS state sync is terminal: no attempt restored the approved \
                                     state and the retries are spent. Signalling shutdown — a node in \
                                     this state cannot serve a chain it never synced.",
                                );
                                terminal.notify_waiters();
                                return;
                            }
                            // Wait for a newer fringe, or a bounded pause, then try again. The two
                            // are both needed: a new fringe is the better target, and waiting only
                            // for one turns a bootstrap that has stopped answering into the same
                            // latch one level up.
                            let _ =
                                tokio::time::timeout(SYNC_RETRY_DELAY, fringe_arrived.notified())
                                    .await;
                        }
                    }
                }
            });
        }
        Ok(())
    }
}

/// The pending marker precedes publication into the durable DAG. A restart must
/// re-enter recovery until the accepted checkpoint has been read back and the
/// marker removed; a partially published DAG is not evidence of completion.
async fn mark_recovery_pending(
    store: &ApprovedStore,
    fringe: &FinalizedFringe,
) -> Result<(), String> {
    store.put(&[(RECOVERY_PENDING_KEY, fringe.clone())]).await?;
    if store.get(&[RECOVERY_PENDING_KEY]).await? != vec![Some(fringe.clone())] {
        return Err("recovery pending marker read-back mismatch".to_string());
    }
    Ok(())
}

async fn report_restored_checkpoint(
    approved_store: &ApprovedStore,
    fringe: &FinalizedFringe,
    log: &dyn Log,
) -> Result<(), String> {
    let result = async {
        approved_store
            .put(&[(FINALIZED_FRINGE_KEY, fringe.clone())])
            .await?;
        if approved_store.get(&[FINALIZED_FRINGE_KEY]).await? != vec![Some(fringe.clone())] {
            return Err("approved-fringe checkpoint read-back mismatch".to_string());
        }
        approved_store.delete(&[RECOVERY_PENDING_KEY]).await?;
        if approved_store.get(&[RECOVERY_PENDING_KEY]).await? != vec![None] {
            return Err("recovery pending marker was not cleared".to_string());
        }
        Ok(())
    }
    .await;
    let source = LogSource::new("casper.engine.NodeSyncing");
    match &result {
        Ok(()) => log.info(source, "LFS state is successfully restored."),
        Err(error) => log.error(
            source,
            &format!("Recovery checkpoint incomplete; refusing completion: {error}"),
        ),
    }
    result
}

/// Signal the syncing-finished handle **only when the state was actually restored** (AUDIT C68).
///
/// The oracle sequences `finished.complete(())` *after* the sync's drain, so a failed attempt leaves
/// the node in `NodeSyncing` — and its own comment says why the approved block is stored only after
/// the state is received ("to restart requesting if interrupted with incomplete state"). Notifying
/// unconditionally means a failed sync moves the node to `NodeRunning` with an incomplete DAG, which
/// is the one outcome the sequencing exists to prevent.
fn notify_when_restored(outcome: &Result<(), String>, finished: &tokio::sync::Notify) {
    if outcome.is_ok() {
        finished.notify_waiters();
    }
}

/// Download the approved (last finalized) state — blocks + tuple space in parallel — and populate the
/// DAG (port of `requestApprovedState`). Free function so it can be spawned off the dispatch loop.
#[allow(clippy::too_many_arguments)]
async fn run_approved_state_sync<I: RSpaceImporter + Send + 'static>(
    fringe: &FinalizedFringe,
    transport: Arc<dyn TransportLayer>,
    conf: RPConf,
    block_store: BlockStore,
    dag: Arc<dyn BlockDagStorage>,
    comm_util: Arc<CommUtil>,
    log: Arc<dyn Log>,
    // **Borrowed, not moved** (AUDIT C181): the retry loop owns the importer and both receivers and
    // an attempt must leave them usable for the next one. They have no inverse — `handle` holds only
    // the senders — so an attempt that consumed them would latch the node exactly as the old code did.
    importer: &mut I,
    incoming_blocks_rx: &mut tokio::sync::mpsc::Receiver<BlockMessage>,
    tuple_space_rx: &mut tokio::sync::mpsc::Receiver<StoreItemsMessage>,
    metadata_rx: &mut tokio::sync::mpsc::Receiver<BlockMetadataMessage>,
    prefetched_certificate: Option<
        &rchain_models::casper::protocol::checkpoint::CheckpointCertificate,
    >,
    replay: Option<&RecoveryReplay>,
    approved_store: &ApprovedStore,
) -> Result<(), String> {
    let source = LogSource::new("casper.engine.NodeSyncing");
    let replay =
        replay.ok_or_else(|| "LFS recovery has no independent replay context".to_string())?;

    // **The certificate comes first** (sync/verification separation). The fringe a bootstrap offers
    // is an unsigned claim, and a state root is not acceptable on the strength of a message this
    // node cannot attribute. `verified` is the statement plus the stake that signed it, checked
    // against the genesis active bonds before a single block or page is requested; a source that
    // cannot produce one fails the attempt with the reason, and no fallback to the old path exists.
    let trusted_bonds = replay.runtime.genesis_pos().active_bonds();
    let verified = certified_checkpoint(
        replay.checkpoint_source.as_deref(),
        &trusted_bonds,
        &replay.shard_id,
        fringe,
        replay.checkpoint_store.as_ref(),
        prefetched_certificate,
    )
    .await?;
    let checkpoint_root = checkpoint_state_root(&verified.checkpoint)?;
    let anchor = decode_hash(&verified.checkpoint.anchor, "certificate anchor")?;

    // **The history is walked by headers** (sync/verification separation). The certificate made
    // this history acceptable; the walk learns its shape — every block's parents, fringe claim and
    // state roots — and no body is downloaded for it. The walk's local checks are the ones a header
    // can support: the hash it was requested by, one parentless genesis, parents older than
    // children, and agreement between duplicates.
    let walk = super::lfs_metadata_requester::request_metadata(
        anchor,
        metadata_rx,
        transport.as_ref(),
        &conf,
        comm_util.as_ref(),
        log.as_ref(),
    )
    .await?;
    let (entries, fringe_state_root) =
        certified_metadata_entries(&walk, &verified.checkpoint, checkpoint_root)?;

    // **Two roots are hydrated, and only two.** The checkpoint's post-state is the state this node
    // starts from; the max fringe member's `fringe_state_hash` is the state the proposer's next
    // block starts from. Both are claims of the same signed statement. The old path walked the
    // whole trie for every ancestry claim and every block's pre/post root — O(height) full walks,
    // measured at ~2.5 minutes for 316 roots (C339) — and replay then recomputed all of it.
    let mut required_roots = BTreeSet::from([checkpoint_root, fringe_state_root]);
    required_roots.insert(Blake2b256Hash::from_byte_array(
        fringe.state_hash.as_bytes(),
    ));
    // **The close block reads a short history.** A post-anchor block runs the PoS close for the
    // block finalized a few heights behind it; `validateBlockCheckpoint` regenerates that block's
    // mergeable channels from the block's **post-state**, and the checkpoint state alone does not
    // carry it. The last few walked headers' post-states are hydrated (the measured 24-root
    // pre/post/fringe set cost ~6 minutes; the close needs the post-states only). The walk is
    // content-addressed and bounded.
    const CLOSE_HISTORY_BLOCKS: usize = 5;
    let mut recent: Vec<_> = walk.headers.values().collect();
    recent.sort_by_key(|header| i64::from(header.block_num));
    for header in recent.iter().rev().take(CLOSE_HISTORY_BLOCKS) {
        let root = header.post_state_hash;
        if root != StateHash::new([0u8; 32]) {
            required_roots.insert(Blake2b256Hash::from_byte_array(root.as_bytes()));
        }
    }
    crate::engine::sync_metrics::census().add_state_roots(required_roots.len() as u64);

    // The anchored fringe's own root is the checkpoint root (`node_running` answers an anchored
    // request with the anchor's post-state), so this walk hydrates it; any other root is a second
    // walk. `request_tuple_space` is the first because its exit state is the one the C271 guard
    // below reads.
    // **The fan-out read is bounded**: the walk must not freeze on a connections cell a writer is
    // holding. The bootstrap is still asked by the requester; the connected set is only the hedge.
    let page_peers = match tokio::time::timeout(Duration::from_secs(2), comm_util.connected_peers())
        .await
    {
        Ok(peers) => peers,
        Err(_) => {
            log.warn(
                source,
                "the connections read did not return within 2s; the state walk asks the bootstrap only",
            );
            Vec::new()
        }
    };
    let tuple_st = request_tuple_space(
        fringe,
        tuple_space_rx,
        REQUEST_TIMEOUT,
        transport.as_ref(),
        &conf,
        &page_peers,
        importer,
        log.as_ref(),
    )
    .await
    .map_err(|e| e.to_string())?;
    // **An unfinished page walk is not a restored state** (C271): what was expected, what was
    // found. `request_tuple_space_roots` refuses its own non-completion exits by name (Law 69), so
    // this is the consumer's independent reading of the same contract.
    if !tuple_st.is_finished() {
        return Err(format!(
            "the tuple-space walk finished without completing every page: {} key(s) still \
             outstanding (expected the walk to reach `is_finished` before the state is called \
             restored)",
            tuple_st.outstanding_count()
        ));
    }

    let fringe_root = Blake2b256Hash::from_byte_array(fringe.state_hash.as_bytes());
    let extra_roots: Vec<Blake2b256Hash> = required_roots
        .into_iter()
        .filter(|root| *root != fringe_root)
        .collect();
    if !extra_roots.is_empty() {
        log.info(
            source,
            &format!(
                "Requesting tuple-space data for {} approved block state roots.",
                extra_roots.len()
            ),
        );
        // This walk's returned state is not bound here, and unlike the joined leg above that is safe
        // (C271): `request_tuple_space_roots` now returns `Ok` **only** for a walk that reached
        // `is_finished`, so there is no partial state left to read — the check that mattered is the one
        // inside the walk, and the one above is its twin at the call site.
        request_tuple_space_roots(
            &extra_roots,
            tuple_space_rx,
            REQUEST_TIMEOUT,
            transport.as_ref(),
            &conf,
            &page_peers,
            importer,
            log.as_ref(),
        )
        .await
        .map_err(|e| e.to_string())?;
    }

    // **The genesis is the one body this path fetches** — the plan's exception. It anchors the
    // operator's genesis pin and is the only block whose body this node will need before a
    // backfill; fetching it here also *content-verifies* it, which no header can be.
    let genesis = walk
        .headers
        .values()
        .find(|h| h.justifications.is_empty())
        .map(|h| h.block_hash)
        .ok_or_else(|| "the metadata walk produced no genesis".to_string())?;
    if let Some(pin) = replay.expected_genesis.filter(|pin| *pin != genesis) {
        return Err(format!(
            "the certified history's genesis {} differs from the pinned genesis {}",
            genesis.to_hex(),
            pin.to_hex()
        ));
    }
    fetch_genesis_body(
        &block_store,
        genesis,
        incoming_blocks_rx,
        comm_util.as_ref(),
        log.as_ref(),
    )
    .await?;

    // **The bond drift check is the last gate.** The signatures were weighed against the genesis
    // active bonds; if the anchor state's active bonds differ, those signatures were cast under a
    // different partition and counting them here would weigh the wrong stake. This runs after the
    // state walk because the bond set is read from the state the certificate names.
    let anchor_bonds = replay
        .runtime
        .compute_bonds(&StateHash::from_slice(checkpoint_root.as_bytes()))
        .await
        .map_err(|e| format!("the checkpoint state's bonds are unavailable: {e}"))?;
    check_bond_set_unchanged(&trusted_bonds, &anchor_bonds).map_err(|e| e.to_string())?;

    mark_recovery_pending(approved_store, fringe).await?;
    super::recovery::publish_certified_metadata(dag.as_ref(), &entries).await?;
    // **Record the first height whose body is missing** (sync/verification separation). Only the
    // genesis body was fetched, so height 1 is the backfill's starting point (0 when the certified
    // anchor is genesis itself: nothing is missing). The backfill advances this marker, and a reader
    // must read "not indexed below this height", not "no such deploy".
    if let Some(store) = replay.checkpoint_store.as_ref() {
        let floor = if verified.checkpoint.anchor_height > 0 {
            1
        } else {
            0
        };
        store.set_history_from(floor).await?;
        // **Adopt only after the state, metadata and floor are accepted.** The head is the
        // statement this node will re-verify at boot and serve to joiners, so it must not move
        // ahead of the state it names. `allow_sibling` is true because this certificate was already
        // tied to the fringe the sync walked: a different anchor at the same height is this
        // network's finality movement, not a foreign statement.
        if let Some(certificate) = store.get(&verified.digest).await? {
            match store.adopt(&verified.digest, &certificate, true).await? {
                rchain_block_storage::checkpoint_store::Adoption::Adopted
                | rchain_block_storage::checkpoint_store::Adoption::AlreadyHead => {}
                rchain_block_storage::checkpoint_store::Adoption::Conflict {
                    head_digest,
                    incoming_digest,
                } => {
                    crate::engine::sync_metrics::census().add_checkpoint_conflict();
                    log.warn(
                        source,
                        &format!(
                            "two checkpoint statements at one anchor: head {}, accepted {}; the \
                             head was not replaced",
                            head_digest
                                .map(|digest| digest.to_hex())
                                .unwrap_or_else(|| "unknown".to_string()),
                            incoming_digest.to_hex()
                        ),
                    );
                }
                rchain_block_storage::checkpoint_store::Adoption::Sibling {
                    head_digest,
                    head_anchor_height,
                } => log.warn(
                    source,
                    &format!(
                        "the accepted certificate's anchor ({}) is a sibling of the served head at \
                         height {head_anchor_height} (head digest {}); the head was not replaced",
                        verified.checkpoint.anchor_height,
                        head_digest
                            .map(|digest| digest.to_hex())
                            .unwrap_or_else(|| "unknown".to_string())
                    ),
                ),
                rchain_block_storage::checkpoint_store::Adoption::Stale {
                    head_anchor_height,
                } => log.warn(
                    source,
                    &format!(
                        "the accepted certificate's anchor ({}) is older than the served head at \
                         height {head_anchor_height}; the head was not replaced",
                        verified.checkpoint.anchor_height
                    ),
                ),
                rchain_block_storage::checkpoint_store::Adoption::ForeignShard {
                    head_shard,
                    incoming_shard,
                } => log.warn(
                    source,
                    &format!(
                        "the accepted certificate names shard {incoming_shard}, but the served head \
                         is {head_shard}; the head was not replaced"
                    ),
                ),
            }
        }
    }
    log.info(
        source,
        &format!(
            "LFS state accepted under the checkpoint certificate at anchor {} ({} of {} stake \
             signed): published {} header(s), downloaded one body (genesis), replayed none.",
            verified.checkpoint.anchor_height,
            verified.supporting_stake,
            verified.total_stake,
            entries.len()
        ),
    );
    Ok(())
}

/// **Fetch, verify and tie the certificate to the fringe the bootstrap served.**
///
/// Extracted so the gate is testable without a runtime: `trusted_bonds` is the genesis active bond
/// set the caller read from its own state, and this function owns every check that does not need
/// that state. A `None` source is a refusal, not a fallback: an unsigned fringe is not a state root.
async fn certified_checkpoint(
    source: Option<&dyn crate::engine::checkpoint_source::CheckpointSource>,
    trusted_bonds: &BTreeMap<
        rchain_models::validator::Validator,
        rchain_shared::refined::NonNegI64,
    >,
    shard_id: &str,
    fringe: &FinalizedFringe,
    store: Option<&rchain_block_storage::checkpoint_store::CheckpointStore>,
    prefetched: Option<&rchain_models::casper::protocol::checkpoint::CheckpointCertificate>,
) -> Result<crate::checkpoint::VerifiedCheckpoint, String> {
    let source = source.ok_or_else(|| {
        "LFS sync has no checkpoint source: this build does not accept an unsigned fringe as a \
         state root"
            .to_string()
    })?;
    // **The statement the anchor was chosen from** is verified here; a fresh fetch can name a newer
    // anchor than the fringe that was requested for the older one, and the tie below would (rightly)
    // refuse it.
    let certificate = match prefetched {
        Some(certificate) => certificate.clone(),
        None => source
            .fetch_certificate()
            .await
            .map_err(|e| format!("the bootstrap's checkpoint certificate was not fetched: {e}"))?,
    };
    let verified = verify_genesis_rooted_certificate(&certificate, trusted_bonds)
        .map_err(|e| format!("the bootstrap's checkpoint certificate was refused: {e}"))?;
    if verified.checkpoint.shard_id != shard_id {
        return Err(format!(
            "the bootstrap's certificate names shard {:?}, not this node's {:?}",
            verified.checkpoint.shard_id, shard_id
        ));
    }
    tie_certificate_to_fringe(&verified.checkpoint, fringe)?;
    // **Record before the state walk**: the certificate is what makes the accepted fringe
    // re-verifiable at the next boot, and a node that crashes after hydrating the state but before
    // storing the statement would restart with a history it cannot attribute. This is the
    // digest-keyed record, not adoption: the head moves only after the state, metadata and history
    // floor are accepted (see the end of the state sync).
    if let Some(store) = store {
        if matches!(
            store.record(&verified.digest, &certificate).await?,
            rchain_block_storage::checkpoint_store::Recorded::Conflict { .. }
        ) {
            // The adoption after the state is published reports the conflict with the head it
            // protected; counting it here keeps the record-time sighting visible even if the walk
            // fails.
            crate::engine::sync_metrics::census().add_checkpoint_conflict();
        }
    }
    Ok(verified)
}

/// Decode the checkpoint's state root.
fn checkpoint_state_root(
    checkpoint: &rchain_models::casper::protocol::checkpoint::Checkpoint,
) -> Result<Blake2b256Hash, String> {
    let bytes = rchain_shared::base16::decode(&checkpoint.anchor_post_state)
        .ok_or_else(|| "the checkpoint's anchor post-state is not base16".to_string())?;
    if bytes.len() != 32 {
        return Err(format!(
            "the checkpoint's anchor post-state is {} bytes, not 32",
            bytes.len()
        ));
    }
    Ok(Blake2b256Hash::from_byte_array(&bytes))
}

fn decode_hash(hex: &str, what: &str) -> Result<BlockHash, String> {
    let bytes =
        rchain_shared::base16::decode(hex).ok_or_else(|| format!("{what} {hex} is not base16"))?;
    BlockHash::try_from(bytes.as_slice()).map_err(|e| format!("{what} {hex} is not a hash: {e}"))
}

/// **The certificate must describe the fringe the sync is walking.** Two statements otherwise: the
/// bootstrap could sign one history and serve another, and the joiner would accept the state of the
/// first while publishing the blocks of the second.
pub(super) fn tie_certificate_to_fringe(
    checkpoint: &rchain_models::casper::protocol::checkpoint::Checkpoint,
    fringe: &FinalizedFringe,
) -> Result<(), String> {
    let transport: BTreeSet<BlockHash> = fringe.hashes.iter().copied().collect();
    if transport.len() != fringe.hashes.len() {
        return Err("the served fringe names a block twice".to_string());
    }
    let certified: BTreeSet<BlockHash> = checkpoint
        .fringe_hashes
        .iter()
        .map(|h| decode_hash(h, "certificate fringe hash"))
        .collect::<Result<_, _>>()?;
    // **The anchored request is the ordinary case for a joiner**: `node_launch` asks the bootstrap
    // for the fringe anchored at the certificate's anchor, so the answer names that anchor alone and
    // carries its ancestry. The statement and the served root are then the same by construction.
    if transport.len() == 1 {
        if let Some(&served) = transport.iter().next() {
            let anchor = decode_hash(&checkpoint.anchor, "certificate anchor")?;
            if served == anchor && certified.contains(&anchor) {
                return Ok(());
            }
        }
    }
    if certified != transport {
        return Err(format!(
            "the certificate's fringe ({} block(s)) is not the fringe the bootstrap served ({} \
             block(s)): a signed statement and a served history must be the same history",
            certified.len(),
            transport.len()
        ));
    }
    let anchor = decode_hash(&checkpoint.anchor, "certificate anchor")?;
    if !certified.contains(&anchor) {
        return Err(format!(
            "the certificate's anchor {} is not a member of its own fringe",
            anchor.to_hex()
        ));
    }
    Ok(())
}

/// **Metadata for a history accepted under the quorum**, plus the fringe state root to hydrate.
///
/// The per-block validator is deliberately not run: that is the audit's job, and running it here is
/// the replay this design removes. What this checks is what a header supports — the anchor's height
/// and post-state against the certificate, the fringe's max `(height, hash)` rule, and the walk's
/// own shape. `validated` means "accepted into this node's DAG under the certificate", not "this
/// node executed it".
pub(super) fn certified_metadata_entries(
    walk: &super::lfs_metadata_requester::MetadataWalk,
    checkpoint: &rchain_models::casper::protocol::checkpoint::Checkpoint,
    checkpoint_root: Blake2b256Hash,
) -> Result<(Vec<(BlockHash, BlockMetadata)>, Blake2b256Hash), String> {
    let anchor = decode_hash(&checkpoint.anchor, "certificate anchor")?;
    let anchor_header = walk.headers.get(&anchor).ok_or_else(|| {
        format!(
            "the metadata walk did not reach the certificate's anchor {}",
            anchor.to_hex()
        )
    })?;
    if i64::from(anchor_header.block_num) != checkpoint.anchor_height {
        return Err(format!(
            "the certificate says the anchor is at height {}, but the header is at {}",
            checkpoint.anchor_height, anchor_header.block_num
        ));
    }
    if Blake2b256Hash::from_byte_array(anchor_header.post_state_hash.as_bytes()) != checkpoint_root
    {
        return Err(format!(
            "the certificate names post-state {}, but the anchor header's post-state is {}",
            checkpoint_root.to_hex(),
            rchain_shared::base16::encode(anchor_header.post_state_hash.as_bytes())
        ));
    }

    // **The anchor rule is max `(height, hash)` among the fringe members the walk holds.** A
    // sibling fringe member is not an ancestor of the anchor and is not walked; the certificate is
    // still signed over it, and the anchor's state is what this node executes from.
    let certified: BTreeSet<BlockHash> = checkpoint
        .fringe_hashes
        .iter()
        .map(|h| decode_hash(h, "certificate fringe hash"))
        .collect::<Result<_, _>>()?;
    let mut best: Option<(i64, BlockHash)> = None;
    for header in walk
        .headers
        .values()
        .filter(|h| certified.contains(&h.block_hash))
    {
        let candidate = (i64::from(header.block_num), header.block_hash);
        if best.is_none_or(|b| candidate > b) {
            best = Some(candidate);
        }
    }
    match best {
        Some((height, hash)) if height == checkpoint.anchor_height && hash == anchor => {}
        Some((height, hash)) => {
            return Err(format!(
                "the certificate's anchor is {} at height {}, but the fringe's max (height, hash) \
                 member the walk holds is {} at height {}",
                anchor.to_hex(),
                checkpoint.anchor_height,
                hash.to_hex(),
                height
            ))
        }
        None => {
            return Err(
                "none of the certificate's fringe members was walked, so the anchor rule cannot \
                 be checked"
                    .to_string(),
            )
        }
    }

    // **The certificate's commitment to the headers the walk holds.** A statement without one is a
    // legacy certificate, refused by the sync path; a mismatch means the headers this peer served
    // are not the headers the quorum signed over. The replay's job is the audit's; this is what a
    // header can prove.
    crate::checkpoint::verify_metadata_root(checkpoint, walk.headers.values())
        .map_err(|e| e.to_string())?;

    let mut entries = Vec::with_capacity(walk.headers.len());
    for header in walk.headers.values() {
        entries.push((
            header.block_hash,
            BlockMetadata {
                block_hash: header.block_hash,
                block_num: header.block_num,
                sender: header.sender,
                seq_num: header.seq_num,
                justifications: header.justifications.clone(),
                bonds_map: header.bonds_map.clone(),
                validated: true,
                validation_failed: false,
                slashable: false,
                failure_cause: None,
                slash_severity: rchain_models::block_metadata::SlashSeverity::Unspecified,
                restore_attempts: 0,
                fringe: header.fringe.clone(),
                fringe_state_hash: header.fringe_state_hash,
                pre_state_hash: header.pre_state_hash,
                post_state_hash: header.post_state_hash,
                sig: header.sig.clone(),
                sig_algorithm: header.sig_algorithm.clone(),
                member_of_fringe: None,
            },
        ));
    }
    // Parents before children for the insert; the DAG refuses a block whose justifications are
    // absent, and the walk's own shape check has already guaranteed the order exists.
    entries.sort_by_key(|(hash, meta)| (i64::from(meta.block_num), *hash));
    Ok((
        entries,
        Blake2b256Hash::from_byte_array(anchor_header.fringe_state_hash.as_bytes()),
    ))
}

/// **Fetch and content-verify the genesis body.** The one body the certified path downloads: the
/// operator's genesis pin is a claim about *this* block, and a hash of a header is not a check of
/// it (`hash_block` covers deploy payloads, which a header does not carry).
async fn fetch_genesis_body(
    block_store: &BlockStore,
    genesis: BlockHash,
    incoming_blocks_rx: &mut tokio::sync::mpsc::Receiver<BlockMessage>,
    comm_util: &CommUtil,
    log: &dyn Log,
) -> Result<(), String> {
    // **The send is inside the deadline.** A blocked transport used to sit outside the receive
    // deadline, so the one body the certified path downloads could stall before the clock started.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let requested = tokio::time::timeout(
        deadline.saturating_duration_since(tokio::time::Instant::now()),
        comm_util.request_lfs_block_from(&genesis, &[]),
    )
    .await;
    if requested.is_err() {
        return Err(format!(
            "the genesis body {} was requested but the request could not be sent within 30s",
            genesis.to_hex()
        ));
    }
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(format!(
                "the genesis body {} was requested but did not arrive within 30s",
                genesis.to_hex()
            ));
        }
        match tokio::time::timeout(remaining, incoming_blocks_rx.recv()).await {
            Ok(Some(block)) => {
                if block.block_hash != genesis {
                    continue;
                }
                if i64::from(block.block_number) != 0
                    || !crate::validate::format_of_fields(&block)
                    || !crate::validate::block_hash(&block)
                {
                    return Err(format!(
                        "the body served for genesis {} is not that block (hash, height or field \
                         format)",
                        genesis.to_hex()
                    ));
                }
                block_store.put(&[(genesis, block)]).await?;
                log.info(
                    LogSource::new("casper.engine.NodeSyncing"),
                    &format!(
                        "genesis body {} fetched and content-verified",
                        genesis.to_hex()
                    ),
                );
                return Ok(());
            }
            Ok(None) => {
                return Err("the block ingress closed before the genesis arrived".to_string())
            }
            Err(_) => {
                return Err(format!(
                    "the genesis body {} did not arrive before the deadline",
                    genesis.to_hex()
                ))
            }
        }
    }
}

/// **The ancestry's shape** (Law 83, C338): one entry per downloaded block and no duplicates. A
/// missing entry stays legal because independent replay reconstructs it, but
/// an entry naming a block the walk did not download is a claim the joiner cannot check at all.
pub(super) fn check_ancestry_shape(
    ancestry: &[BlockFringe],
    downloaded: &BTreeSet<BlockHash>,
) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for bf in ancestry {
        if !downloaded.contains(&bf.block_hash) {
            return Err(format!(
                "the bootstrap sent fringe metadata for block {}, which the walk did not download",
                bf.block_hash.to_hex()
            ));
        }
        if !seen.insert(bf.block_hash) {
            return Err(format!(
                "the bootstrap sent two ancestry entries for block {}",
                bf.block_hash.to_hex()
            ));
        }
        for h in &bf.fringe {
            if !downloaded.contains(h) {
                return Err(format!(
                    "the ancestry for block {} names fringe member {}, which the walk did not \
                     download; a fringe names ancestors, so this is not a history the joiner can \
                     check",
                    bf.block_hash.to_hex(),
                    h.to_hex()
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use rchain_block_storage::dag::codecs::{
        BlockHashCodec, BlockMessageCodec, BlockMetadataCodec, SignedDeployDataCodec,
    };
    use rchain_block_storage::dag::dag_storage::DeployId;
    use rchain_comm::errors::CommErr;
    use rchain_comm::peer_node::{NodeIdentifier, PeerNode};
    use rchain_comm::rp::rp_conf::{ClearConnectionsConf, RPConf};
    use rchain_comm::transport::chunker::Blob;
    use rchain_comm::transport::transport_layer::TransportLayer;
    use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
    use rchain_models::block::state_hash::StateHash;
    use rchain_models::block_metadata::BlockMetadata;
    use rchain_models::casper::protocol::casper_message::{
        BlockHeader, BlockMessage, RholangState, SignedDeployData,
    };
    use rchain_models::comm::protocol::Protocol;
    use rchain_models::validator::Validator;
    use rchain_rspace::state::RSpaceImporter;
    use rchain_shared::log::{Log, LogSource};
    use rchain_shared::refined::BlockHeight;
    use rchain_shared::state::TrieImporter;
    use rchain_shared::store::{InMemoryKeyValueStore, KeyValueStore};
    use rchain_shared::typed_store::{BytesCodec, KeyValueTypedStore, KeyValueTypedStoreCodec};
    use std::time::Duration;

    use crate::protocol::comm_util::{CommUtil, ConnectionsCell};
    use rchain_block_storage::approved_store::{
        ApprovedStore, FINALIZED_FRINGE_KEY, RECOVERY_PENDING_KEY,
    };
    use rchain_block_storage::dag::codecs::{ByteCodec, FringeCodec};

    use crate::block_metadata_store::BlockMetadataStore;
    use crate::dag::BlockDagKeyValueStorage;

    /// **The process-global [`SYNC_STATE`] is written by every test that lets a sync task run**, so a
    /// reader that means to observe *its own* writes is polluted by whichever sibling is mid-flight.
    /// The tests that touch it take this guard for their whole body; the direct test it protects
    /// asserts on values no other test produces. A `tokio` mutex rather than a `std` one because the
    /// guard is held across `await` — each `#[tokio::test]` owns its own runtime, and this primitive
    /// is the one that serializes tasks *across* runtimes.
    static SYNC_STATE_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    type Shared = Arc<tokio::sync::Mutex<Box<dyn KeyValueStore + Send + Sync>>>;

    fn in_memory() -> Shared {
        Arc::new(tokio::sync::Mutex::new(Box::new(
            InMemoryKeyValueStore::default(),
        )))
    }

    fn hash(byte: u8) -> BlockHash {
        let mut bytes = [0u8; 32];
        bytes[0] = byte;
        BlockHash::new(bytes)
    }

    fn chain_block(block_num: i64, justification: Option<BlockHash>) -> BlockMessage {
        let justifications: Vec<BlockHash> = justification.into_iter().collect();
        BlockMessage {
            version: 1,
            shard_id: "root".to_string(),
            block_hash: hash(block_num as u8),
            block_number: BlockHeight::try_from(block_num).unwrap(),
            sender: Validator::new([0u8; 65]),
            seq_num: block_num.try_into().unwrap(),
            pre_state_hash: StateHash::new([0u8; 32]),
            post_state_hash: StateHash::new([0u8; 32]),
            justifications,
            bonds: BTreeMap::new(),
            rejected_deploys: BTreeSet::new(),
            rejected_blocks: BTreeSet::new(),
            rejected_senders: BTreeSet::new(),
            state: RholangState::default(),
            sig_algorithm: "secp256k1".to_string(),
            sig: vec![],
            timestamp: 0,
        }
    }

    async fn build_dag() -> Arc<BlockDagKeyValueStorage> {
        let metadata_store = Arc::new(
            BlockMetadataStore::create(Arc::new(KeyValueTypedStoreCodec::new(
                in_memory(),
                Arc::new(BlockHashCodec),
                Arc::new(BlockMetadataCodec),
            )))
            .await
            .unwrap(),
        );
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
        Arc::new(
            BlockDagKeyValueStorage::create(metadata_store, deploy_index, deploy_store)
                .await
                .unwrap(),
        )
    }

    fn peer(name: &str) -> PeerNode {
        PeerNode::from(
            NodeIdentifier::new(name.as_bytes().to_vec()),
            "host".to_string(),
            rchain_shared::refined::Port::new(40400),
            rchain_shared::refined::Port::new(40404),
        )
    }

    /// A logger that keeps its lines: the end-to-end test needs the *evidence that the attempt ran and
    /// failed*, without which "no notification arrived" could pass because nothing happened at all.
    #[derive(Default)]
    struct RecordingLog {
        lines: std::sync::Mutex<Vec<String>>,
    }

    impl RecordingLog {
        fn contains(&self, needle: &str) -> bool {
            self.lines
                .lock()
                .expect("log lock")
                .iter()
                .any(|l| l.contains(needle))
        }
    }

    impl Log for RecordingLog {
        fn is_trace_enabled(&self, _source: LogSource) -> bool {
            false
        }
        fn trace(&self, _source: LogSource, _msg: &str) {}
        fn debug(&self, _source: LogSource, _msg: &str) {}
        fn info(&self, _source: LogSource, msg: &str) {
            self.lines.lock().expect("log lock").push(msg.to_string());
        }
        fn warn(&self, _source: LogSource, msg: &str) {
            self.lines
                .lock()
                .expect("log lock")
                .push(format!("[warn] {msg}"));
        }
        fn error(&self, _source: LogSource, msg: &str) {
            self.lines.lock().expect("log lock").push(msg.to_string());
        }
    }

    /// A transport that answers nothing: this fixture's failure comes from the stores, not the wire.
    struct SilentTransport;

    #[async_trait]
    impl TransportLayer for SilentTransport {
        async fn send(&self, _peer: &PeerNode, _msg: Protocol) -> CommErr<()> {
            Ok(())
        }
        async fn broadcast(&self, _peers: &[PeerNode], _msg: Protocol) -> Vec<CommErr<()>> {
            Vec::new()
        }
        async fn stream(&self, _peers: &[PeerNode], _blob: Blob) {}
    }

    /// A block store whose reads fail — the block walk's first store touch is a `contains`, so the
    /// attempt fails immediately rather than waiting out the walk's idle timeout.
    struct UnreadableBlockStore {
        inner: BlockStore,
    }

    #[async_trait]
    impl KeyValueTypedStore<BlockHash, BlockMessage> for UnreadableBlockStore {
        async fn get(&self, keys: &[BlockHash]) -> Result<Vec<Option<BlockMessage>>, String> {
            self.inner.get(keys).await
        }
        async fn put(&self, pairs: &[(BlockHash, BlockMessage)]) -> Result<(), String> {
            self.inner.put(pairs).await
        }
        async fn delete(&self, keys: &[BlockHash]) -> Result<usize, String> {
            self.inner.delete(keys).await
        }
        async fn contains(&self, _keys: &[BlockHash]) -> Result<Vec<bool>, String> {
            Err("block store is unreadable".to_string())
        }
        async fn to_map(&self) -> Result<BTreeMap<BlockHash, BlockMessage>, String> {
            self.inner.to_map().await
        }
    }

    /// An importer that refuses to open a root, so the tuple-space leg fails at once instead of
    /// waiting out its 120 s timeout — `set_root` is fallible since AUDIT C67's sibling work.
    struct RefusingImporter;

    impl TrieImporter<Blake2b256Hash> for RefusingImporter {
        fn set_history_items<Value>(
            &mut self,
            _data: &[(Blake2b256Hash, Value)],
            _to_buffer: impl Fn(&Value) -> Vec<u8>,
        ) -> Result<(), String> {
            Ok(())
        }
        fn set_data_items<Value>(
            &mut self,
            _data: &[(Blake2b256Hash, Value)],
            _to_buffer: impl Fn(&Value) -> Vec<u8>,
        ) -> Result<(), String> {
            Ok(())
        }
        fn set_root(&mut self, _root: Blake2b256Hash) -> Result<(), String> {
            Err("state store unavailable".to_string())
        }
    }

    impl RSpaceImporter for RefusingImporter {
        fn get_history_item(&self, _hash: Blake2b256Hash) -> Result<Option<Vec<u8>>, String> {
            Ok(None)
        }
    }

    /// The fixture the retry tests share (AUDIT C181): a block store whose reads fail — the walk's
    /// first store touch is a `contains`, so an attempt fails at once and for a reason the log names,
    /// rather than waiting out the walk's idle timeout — an importer that refuses every root, and a
    /// transport that answers nothing.
    async fn failing_engine(
        log: &Arc<RecordingLog>,
    ) -> (NodeSyncing<RefusingImporter>, PeerNode, ApprovedStore) {
        let bootstrap = peer("bootstrap");
        let inner_store: BlockStore = Arc::new(KeyValueTypedStoreCodec::new(
            in_memory(),
            Arc::new(BlockHashCodec),
            Arc::new(BlockMessageCodec),
        ));
        let block_store: BlockStore = Arc::new(UnreadableBlockStore { inner: inner_store });
        let dag = build_dag().await;
        let approved_store: ApprovedStore = Arc::new(KeyValueTypedStoreCodec::new(
            in_memory(),
            Arc::new(ByteCodec),
            Arc::new(FringeCodec),
        ));

        let transport: Arc<dyn TransportLayer> = Arc::new(SilentTransport);
        let conf = RPConf {
            local: peer("local"),
            network_id: "testnet".to_string(),
            bootstrap: Some(bootstrap.clone()),
            default_timeout: Duration::from_secs(10),
            max_num_of_connections: 10,
            clear_connections: ClearConnectionsConf {
                num_of_connections_pinged: 10,
            },
        };
        let connections: ConnectionsCell =
            Arc::new(tokio::sync::RwLock::new(vec![bootstrap.clone()]));
        let comm_util = Arc::new(CommUtil::new(
            transport.clone(),
            conf.clone(),
            connections,
            log.clone(),
        ));

        let engine = NodeSyncing::new(
            transport,
            conf,
            block_store,
            dag,
            approved_store.clone(),
            comm_util,
            log.clone(),
            None,
            false,
            RefusingImporter,
        );
        (engine, bootstrap, approved_store)
    }

    /// Regression test for the LFS-sync "justification not present in message map" failure: the
    /// DAG must be populated parents-before-children, i.e. in ascending block height order. The
    /// prior code reversed the height map and inserted the newest block first, whose justification
    /// was not yet in the message map.
    #[tokio::test]
    async fn populate_dag_inserts_parents_before_children() {
        let n = 5i64;
        // A single-validator chain, each block justifying its predecessor.
        let blocks: Vec<BlockMessage> = (0..n)
            .map(|i| chain_block(i, (i > 0).then(|| hash(i as u8 - 1))))
            .collect();

        let block_store: BlockStore = Arc::new(KeyValueTypedStoreCodec::new(
            in_memory(),
            Arc::new(BlockHashCodec),
            Arc::new(BlockMessageCodec),
        ));
        for b in &blocks {
            block_store.put(&[(b.block_hash, b.clone())]).await.unwrap();
        }

        let dag = build_dag().await;
        let entries: Vec<_> = blocks
            .iter()
            .map(|block| (block.block_hash, BlockMetadata::from_block(block)))
            .collect();
        super::super::recovery::publish_verified_blocks(dag.as_ref(), &block_store, &entries)
            .await
            .expect("verified metadata must be published parents-first");

        // The whole chain is in the DAG, so the latest block number equals the chain length.
        assert_eq!(dag.get_representation().await.latest_block_number(), n);
    }

    /// **Law 83, C338: a fringe row for a block the walk did not download** is a claim the joiner
    /// cannot check at all, so it is refused before anything is written.
    #[test]
    fn an_ancestry_entry_for_an_unknown_block_is_refused() {
        let downloaded: BTreeSet<BlockHash> = [hash(0)].into_iter().collect();
        let entry = BlockFringe {
            block_hash: hash(9),
            fringe: BTreeSet::new(),
            fringe_state_hash: StateHash::new([0u8; 32]),
        };
        let err = check_ancestry_shape(&[entry], &downloaded)
            .expect_err("an entry for an undownloaded block must be refused");
        assert!(err.contains("did not download"), "{err}");
    }

    #[test]
    fn two_ancestry_entries_for_one_block_are_refused() {
        let downloaded: BTreeSet<BlockHash> = [hash(0)].into_iter().collect();
        let entry = BlockFringe {
            block_hash: hash(0),
            fringe: BTreeSet::new(),
            fringe_state_hash: StateHash::new([0u8; 32]),
        };
        let err = check_ancestry_shape(&[entry.clone(), entry], &downloaded)
            .expect_err("duplicate ancestry rows must be refused");
        assert!(err.contains("two ancestry entries"), "{err}");
    }

    /// **The derivable field is refused when it lies**: a block's `fringe` is recomputed from its
    /// justifications (`latest_fringe`), so a bootstrap cannot choose it.
    #[tokio::test]
    async fn an_ancestry_fringe_that_is_not_the_derived_fringe_is_refused() {
        let n = 3i64;
        let blocks: Vec<BlockMessage> = (0..n)
            .map(|i| chain_block(i, (i > 0).then(|| hash(i as u8 - 1))))
            .collect();
        let block_store: BlockStore = Arc::new(KeyValueTypedStoreCodec::new(
            in_memory(),
            Arc::new(BlockHashCodec),
            Arc::new(BlockMessageCodec),
        ));
        for b in &blocks {
            block_store.put(&[(b.block_hash, b.clone())]).await.unwrap();
        }
        let mut height_map: BTreeMap<i64, BTreeSet<BlockHash>> = BTreeMap::new();
        for b in &blocks {
            height_map
                .entry(i64::from(b.block_number))
                .or_default()
                .insert(b.block_hash);
        }
        // Block 1 justifies genesis, whose own fringe is empty, so the derived fringe is empty.
        // Claiming genesis as the fringe is a lie the joiner can see.
        let mut lying = BTreeSet::new();
        lying.insert(blocks[0].block_hash);
        let ancestry = [BlockFringe {
            block_hash: blocks[1].block_hash,
            fringe: lying,
            fringe_state_hash: StateHash::new([0u8; 32]),
        }];

        // The replay result is authoritative; the supplied fringe must match it.
        let local = BlockMetadata::from_block(&blocks[1]);
        let err = super::super::recovery::compare_claim(&local, &ancestry[0])
            .expect_err("a fringe that disagrees with replay must be refused");
        assert!(err.contains("disagrees"), "{err}");
    }

    /// A non-bootstrap peer cannot consume the one-shot latch or poison the next retry target.
    #[tokio::test]
    async fn initial_fringe_signal_requires_an_authorized_nonempty_target() {
        let log = Arc::new(RecordingLog::default());
        let (mut engine, bootstrap, _approved) = failing_engine(&log).await;
        let signal = engine.initial_fringe_received_handle();
        let mut fringe = FinalizedFringe {
            hashes: vec![],
            state_hash: StateHash::new([7u8; 32]),
            ancestry: vec![],
        };
        engine
            .handle(&bootstrap, &CasperMessage::FinalizedFringe(fringe.clone()))
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(1), signal.notified())
                .await
                .is_err()
        );
        fringe.hashes = vec![hash(7)];
        engine
            .handle(
                &peer("outsider"),
                &CasperMessage::FinalizedFringe(fringe.clone()),
            )
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(1), signal.notified())
                .await
                .is_err()
        );
        engine
            .handle(&bootstrap, &CasperMessage::FinalizedFringe(fringe))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), signal.notified())
            .await
            .expect("admitted target stops solicitation");
    }

    #[tokio::test]
    async fn a_non_bootstrap_fringe_cannot_start_or_replace_lfs_sync() {
        let log = Arc::new(RecordingLog::default());
        let (mut engine, bootstrap, _approved) = failing_engine(&log).await;
        let outsider = peer("outsider");
        assert_eq!(outsider.endpoint, bootstrap.endpoint);
        assert_ne!(outsider.id, bootstrap.id);
        let fringe = FinalizedFringe {
            hashes: vec![hash(7)],
            state_hash: StateHash::new([7u8; 32]),
            ancestry: Vec::new(),
        };

        engine
            .handle(&outsider, &CasperMessage::FinalizedFringe(fringe.clone()))
            .await
            .expect("unexpected peer must be ignored without error");

        assert!(
            engine.start_requester,
            "an outsider consumed the sync trigger"
        );
        assert!(
            engine.latest_fringe.lock().await.is_none(),
            "an outsider replaced the trusted retry target"
        );
        assert!(!log.contains("Received finalized fringe from bootstrap node"));
        assert!(log.contains("[warn] Fringe message ignored"));

        // Once a trusted target exists, a matching endpoint with a different key still cannot
        // replace it. Keep the worker out of this rejection test by staging its consumed latch.
        engine.start_requester = false;
        *engine.latest_fringe.lock().await = Some(fringe.clone());
        let mut forged = fringe.clone();
        forged.state_hash = StateHash::new([8u8; 32]);
        engine
            .handle(&outsider, &CasperMessage::FinalizedFringe(forged))
            .await
            .expect("an outsider's update must be ignored");
        assert_eq!(*engine.latest_fringe.lock().await, Some(fringe));
        assert!(!log.contains("Received finalized fringe from bootstrap node"));
    }

    /// A hostname/IP or advertised-port change does not change the TLS-authenticated node key.
    #[tokio::test]
    async fn a_bootstrap_endpoint_change_does_not_block_lfs_sync() {
        let _guard = SYNC_STATE_TESTS.lock().await;
        for (host, tcp, udp) in [
            ("127.0.0.1", 40400, 40404),
            ("bootstrap.example", 50400, 40404),
            ("bootstrap.example", 40400, 50404),
            ("127.0.0.1", 50400, 50404),
        ] {
            let log = Arc::new(RecordingLog::default());
            let (mut engine, mut bootstrap, _approved) = failing_engine(&log).await;
            bootstrap.endpoint.host = "bootstrap.example".to_string();
            engine.conf.bootstrap = Some(bootstrap.clone());
            let sender = PeerNode::from(
                bootstrap.id.clone(),
                host.to_string(),
                rchain_shared::refined::Port::new(tcp),
                rchain_shared::refined::Port::new(udp),
            );
            assert_ne!(sender.endpoint, bootstrap.endpoint);
            let fringe = FinalizedFringe {
                hashes: vec![hash(7)],
                state_hash: StateHash::new([7u8; 32]),
                ancestry: Vec::new(),
            };
            assert!(engine.importer.is_some());
            engine
                .handle(&sender, &CasperMessage::FinalizedFringe(fringe.clone()))
                .await
                .expect("the bootstrap identity remains trusted after an endpoint change");
            assert!(
                !engine.start_requester,
                "the trusted fringe did not start sync"
            );
            assert!(engine.importer.is_none(), "the sync worker was not started");
            assert_eq!(*engine.latest_fringe.lock().await, Some(fringe));

            // Observe the spawned worker, then stop this deliberately failing fixture before the
            // next case or another test reads the process-wide sync state.
            wait_for_sync_log(&log, "LFS state sync failed").await;
            *engine.latest_fringe.lock().await = None;
            engine.fringe_arrived.notify_one();
            wait_for_sync_log(&log, "LFS sync task started with no fringe").await;
        }
    }

    async fn wait_for_sync_log(log: &RecordingLog, needle: &str) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !log.contains(needle) {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("the sync fixture did not reach its observable worker state");
    }

    #[tokio::test]
    async fn a_fringe_without_a_configured_bootstrap_is_refused() {
        let log = Arc::new(RecordingLog::default());
        let (mut engine, sender, _approved) = failing_engine(&log).await;
        engine.conf.bootstrap = None;
        engine
            .handle(
                &sender,
                &CasperMessage::FinalizedFringe(FinalizedFringe {
                    hashes: vec![hash(7)],
                    state_hash: StateHash::new([7u8; 32]),
                    ancestry: Vec::new(),
                }),
            )
            .await
            .expect("a missing trust anchor must not authorize a peer");
        assert!(engine.start_requester);
        assert!(engine.latest_fringe.lock().await.is_none());
        assert!(log.contains("[warn] Fringe message ignored"));
    }

    #[tokio::test]
    async fn a_non_bootstrap_empty_fringe_is_not_a_genesis_announcement() {
        let log = Arc::new(RecordingLog::default());
        let (mut engine, _bootstrap, _approved) = failing_engine(&log).await;
        engine
            .handle(
                &peer("outsider"),
                &CasperMessage::FinalizedFringe(FinalizedFringe {
                    hashes: Vec::new(),
                    state_hash: StateHash::new([7u8; 32]),
                    ancestry: Vec::new(),
                }),
            )
            .await
            .expect("an outsider's empty fringe must be ignored");
        assert!(engine.start_requester);
        assert!(engine.latest_fringe.lock().await.is_none());
        assert!(log.contains("[warn] Fringe message ignored"));
        assert!(!log.contains("the genesis master's announcement"));
    }

    /// A failed durable checkpoint must keep the completion notification closed.
    #[tokio::test]
    async fn checkpoint_write_failure_refuses_completion() {
        struct RejectCheckpointWrite {
            inner: ApprovedStore,
        }

        #[async_trait]
        impl KeyValueTypedStore<u8, FinalizedFringe> for RejectCheckpointWrite {
            async fn get(&self, keys: &[u8]) -> Result<Vec<Option<FinalizedFringe>>, String> {
                self.inner.get(keys).await
            }
            async fn put(&self, _pairs: &[(u8, FinalizedFringe)]) -> Result<(), String> {
                Err("checkpoint store unavailable".to_string())
            }
            async fn delete(&self, keys: &[u8]) -> Result<usize, String> {
                self.inner.delete(keys).await
            }
            async fn contains(&self, keys: &[u8]) -> Result<Vec<bool>, String> {
                self.inner.contains(keys).await
            }
            async fn to_map(&self) -> Result<BTreeMap<u8, FinalizedFringe>, String> {
                self.inner.to_map().await
            }
        }

        let working: ApprovedStore = Arc::new(KeyValueTypedStoreCodec::new(
            in_memory(),
            Arc::new(ByteCodec),
            Arc::new(FringeCodec),
        ));
        let failing: ApprovedStore = Arc::new(RejectCheckpointWrite {
            inner: working.clone(),
        });
        let fringe = FinalizedFringe {
            hashes: vec![hash(7)],
            state_hash: StateHash::new([7u8; 32]),
            ancestry: Vec::new(),
        };

        let failure_log = RecordingLog::default();
        mark_recovery_pending(&working, &fringe).await.unwrap();
        let failure = report_restored_checkpoint(&failing, &fringe, &failure_log).await;
        assert!(failure.is_err());
        let finished = tokio::sync::Notify::new();
        let notified = finished.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        notify_when_restored(&failure, &finished);
        assert!(tokio::time::timeout(Duration::from_millis(20), notified)
            .await
            .is_err());
        assert!(failure_log.contains("refusing completion"));
        assert_eq!(
            working.get(&[RECOVERY_PENDING_KEY]).await.unwrap(),
            vec![Some(fringe.clone())]
        );
        assert!(failure_log.contains("checkpoint store unavailable"));
        assert!(!failure_log.contains("LFS state is successfully restored."));
        assert_eq!(
            working.get(&[FINALIZED_FRINGE_KEY]).await.unwrap(),
            vec![None]
        );

        let success_log = RecordingLog::default();
        report_restored_checkpoint(&working, &fringe, &success_log)
            .await
            .unwrap();
        assert_eq!(
            working.get(&[RECOVERY_PENDING_KEY]).await.unwrap(),
            vec![None]
        );
        assert!(success_log.contains("LFS state is successfully restored."));
        assert_eq!(
            working.get(&[FINALIZED_FRINGE_KEY]).await.unwrap(),
            vec![Some(fringe)]
        );
    }

    #[tokio::test]
    async fn checkpoint_readback_and_pending_marker_removal_are_required() {
        struct FaultyCheckpoint {
            inner: ApprovedStore,
            discard: Option<u8>,
            keep_pending: bool,
        }
        #[async_trait]
        impl KeyValueTypedStore<u8, FinalizedFringe> for FaultyCheckpoint {
            async fn get(&self, keys: &[u8]) -> Result<Vec<Option<FinalizedFringe>>, String> {
                self.inner.get(keys).await
            }
            async fn put(&self, pairs: &[(u8, FinalizedFringe)]) -> Result<(), String> {
                let retained: Vec<_> = pairs
                    .iter()
                    .filter(|(key, _)| Some(*key) != self.discard)
                    .cloned()
                    .collect();
                self.inner.put(&retained).await
            }
            async fn delete(&self, keys: &[u8]) -> Result<usize, String> {
                if self.keep_pending {
                    Ok(0)
                } else {
                    self.inner.delete(keys).await
                }
            }
            async fn contains(&self, keys: &[u8]) -> Result<Vec<bool>, String> {
                self.inner.contains(keys).await
            }
            async fn to_map(&self) -> Result<BTreeMap<u8, FinalizedFringe>, String> {
                self.inner.to_map().await
            }
        }
        let fringe = FinalizedFringe {
            hashes: vec![hash(7)],
            state_hash: StateHash::new([7; 32]),
            ancestry: Vec::new(),
        };
        for (discard, keep_pending) in [
            (Some(RECOVERY_PENDING_KEY), false),
            (Some(FINALIZED_FRINGE_KEY), false),
            (None, true),
        ] {
            let inner: ApprovedStore = Arc::new(KeyValueTypedStoreCodec::new(
                in_memory(),
                Arc::new(ByteCodec),
                Arc::new(FringeCodec),
            ));
            let faulty: ApprovedStore = Arc::new(FaultyCheckpoint {
                inner: inner.clone(),
                discard,
                keep_pending,
            });
            let started = mark_recovery_pending(&faulty, &fringe).await;
            if discard == Some(RECOVERY_PENDING_KEY) {
                assert!(started.unwrap_err().contains("read-back"));
                assert_eq!(
                    inner.get(&[FINALIZED_FRINGE_KEY]).await.unwrap(),
                    vec![None]
                );
            } else {
                started.unwrap();
                let outcome =
                    report_restored_checkpoint(&faulty, &fringe, &RecordingLog::default()).await;
                assert!(
                    outcome.is_err(),
                    "acknowledging a write or delete is not a durable completion proof"
                );
                assert_eq!(
                    inner.get(&[RECOVERY_PENDING_KEY]).await.unwrap(),
                    vec![Some(fringe.clone())]
                );
                report_restored_checkpoint(&inner, &fringe, &RecordingLog::default())
                    .await
                    .unwrap();
                assert_eq!(
                    inner.get(&[RECOVERY_PENDING_KEY]).await.unwrap(),
                    vec![None]
                );
            }
        }
    }

    /// **AUDIT C68**: a failed LFS sync must not signal the node out of `NodeSyncing`.
    ///
    /// Both directions in one test, because the fix is a decision and the decision has two outcomes:
    /// `Err` must leave the waiter untouched (the oracle's `complete` is sequenced after the drain), and
    /// `Ok` must wake it (otherwise the node would never leave syncing at all, which the second half
    /// rules out). Falsified against this tree: with the unconditional `finished.notify_waiters()`
    /// restored, the first half fails — the waiter is woken by a failed attempt.
    ///
    /// What this pins is the *decision*, not the transition: that a failed attempt leaves the node in
    /// syncing end to end needs a `NodeSyncing` fixture with a failing store and a mock transport,
    /// which is the next unit's work rather than this one's.
    #[tokio::test]
    async fn a_failed_sync_does_not_signal_the_node_out_of_syncing() {
        // A failed attempt: the waiter must stay asleep.
        let finished = tokio::sync::Notify::new();
        let mut waiter = std::pin::pin!(finished.notified());
        notify_when_restored(&Err("LFS state sync failed".to_string()), &finished);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut waiter)
                .await
                .is_err(),
            "a failed sync signalled the node to leave syncing"
        );

        // A successful attempt: the waiter must wake — so the assertion above cannot pass by the
        // signal never being wired at all.
        let finished = tokio::sync::Notify::new();
        let mut waiter = std::pin::pin!(finished.notified());
        notify_when_restored(&Ok(()), &finished);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut waiter)
                .await
                .is_ok(),
            "a restored state must signal the node out of syncing"
        );
    }

    /// **No silence, law 69's other half: the sync task's state is a reading, and it is *cleared* when
    /// there is nothing to read.**
    ///
    /// Both directions in one test, following [`notify_when_restored`]'s pair above: a failed attempt
    /// leaves a reading that names the attempt it belongs to and the reason it failed, and a restored
    /// state clears it — because a reading left behind ("not syncing, but here is why it last failed")
    /// is indistinguishable from a node that is failing *right now*, which is the silence this row is
    /// about. `None` is the ordinary case, so the second half is also what keeps the first from passing
    /// as "the surface always says something".
    ///
    /// The statics are process-wide and the tests here run in parallel, so this holds
    /// [`SYNC_STATE_TESTS`] and so do the tests that spawn a sync task: without it a sibling mid-flight
    /// would write between the calls below and the assertion would be about its write, not this one's.
    #[tokio::test]
    async fn a_failed_attempt_leaves_a_reading_and_a_restore_clears_it() {
        let _guard = SYNC_STATE_TESTS.lock().await;

        note_sync_attempt(2);
        note_sync_failure("the state-page walk was abandoned after 3 idle rounds");
        let reading = sync_state().expect("a running sync reports a reading");
        assert_eq!(
            reading.attempts, 2,
            "the reading names the attempt that started"
        );
        assert_eq!(
            reading.last_failure.as_deref(),
            Some("the state-page walk was abandoned after 3 idle rounds"),
            "the reading must carry the failure verbatim, not a paraphrase of it"
        );

        clear_sync_state();
        assert_eq!(
            sync_state(),
            None,
            "a node that is not syncing must report nothing, not the last failure it saw"
        );
    }

    /// **U16 / AUDIT C68 *and* C181, end to end: a failed attempt is retried, the retries are bounded,
    /// and the bound has a terminal state.**
    ///
    /// Two properties, and the old version of this test pinned only the first — under a name that
    /// asserted the *defect*. C68: `node_launch` waits on the `finished` handle as the only signal to
    /// leave `NodeSyncing` for `NodeRunning`, so "the node stayed out of running" is exactly "that
    /// handle was never notified"; the attempt must really run and really fail (the recording log
    /// proves it, so the assertion cannot pass because nothing ran). C181: the node must not sit there
    /// for ever — the retries end in a **terminal** signal, which is the state this row is about ("a
    /// serving API and no error line").
    ///
    /// Falsified against this tree by restoring the pre-fix latch (the attempt takes the importer and
    /// both receivers with no retry): the attempt counter never reaches [`MAX_SYNC_ATTEMPTS`] and the
    /// terminal assertion hangs out its bound. The second half — that `finished` still never fires —
    /// is what keeps the fix from being "signal success on failure", which is the C68 regression.
    #[tokio::test]
    async fn a_failed_attempt_is_retried_and_the_retries_end_in_a_terminal_signal() {
        // Held for the whole test: this test's sync task writes the process-global `SYNC_STATE`, and
        // the direct reading test must not observe it mid-flight.
        let _guard = SYNC_STATE_TESTS.lock().await;
        let bootstrap = peer("bootstrap");
        let log = Arc::new(RecordingLog::default());

        let inner_store: BlockStore = Arc::new(KeyValueTypedStoreCodec::new(
            in_memory(),
            Arc::new(BlockHashCodec),
            Arc::new(BlockMessageCodec),
        ));
        let block_store: BlockStore = Arc::new(UnreadableBlockStore { inner: inner_store });
        let dag = build_dag().await;
        let approved_store: ApprovedStore = Arc::new(KeyValueTypedStoreCodec::new(
            in_memory(),
            Arc::new(ByteCodec),
            Arc::new(FringeCodec),
        ));

        let transport: Arc<dyn TransportLayer> = Arc::new(SilentTransport);
        let conf = RPConf {
            local: peer("local"),
            network_id: "testnet".to_string(),
            bootstrap: Some(bootstrap.clone()),
            default_timeout: Duration::from_secs(10),
            max_num_of_connections: 10,
            clear_connections: ClearConnectionsConf {
                num_of_connections_pinged: 10,
            },
        };
        let connections: ConnectionsCell =
            Arc::new(tokio::sync::RwLock::new(vec![bootstrap.clone()]));
        let comm_util = Arc::new(CommUtil::new(
            transport.clone(),
            conf.clone(),
            connections,
            log.clone(),
        ));

        let mut engine = NodeSyncing::new(
            transport,
            conf,
            block_store,
            dag,
            approved_store.clone(),
            comm_util,
            log.clone(),
            None,
            false,
            RefusingImporter,
        );
        let finished = engine.finished_handle();
        // Both waiters exist before anything can signal: `Notify::notify_waiters` only wakes waiters
        // already registered, so a tripwire created after the fact observes nothing (the comment
        // below is this test's own history of getting exactly that wrong).
        let terminal = engine.terminal_handle();
        let mut terminal_waiter = std::pin::pin!(terminal.notified());

        let fringe = FinalizedFringe {
            hashes: vec![hash(7)],
            state_hash: StateHash::new([7u8; 32]),
            ancestry: Vec::new(),
        };

        // The waiter is registered *before* the attempt can signal — polled by the same `select!` that
        // drives the handler — because `Notify::notify_waiters` only wakes waiters already registered.
        // The first version of this test created the waiter *after* the failure and passed with the
        // defect restored: the notification had already fired into an empty waiter set. This is the
        // pass's own rule again (a falsifier must fail against the tree with the defect in it), and the
        // reason the structure below is a `select!` rather than a sleep-then-check.
        // The waiter captures the `notify_waiters` counter **at creation** (tokio's `notified()`
        // reads it then and completes on the next poll if it moved), so creating it before the attempt
        // can signal is what makes the observation independent of *when* the future is polled — no
        // grace period, and no dependence on the scheduler. The first version created it *after* the
        // failure and was blind to the defect for exactly that reason; the second bounded the
        // fixture's progress with a 2 s sleep, which a loaded parallel runner can miss (a flaky
        // failure rather than a missed defect). Both are the same lesson: what a tripwire observes,
        // and when it starts observing, are part of its calibration.
        let mut waiter = std::pin::pin!(finished.notified());
        // A generous bound, because this waits on the *fixture* (a spawned task's failure path), not
        // on anything about the code under test — and it is declared here because the retry drive
        // below shares it.
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        // **Scoped on purpose.** The pinned future borrows `engine` mutably for as long as it lives,
        // and the retry below has to call `handle` again on the same engine; a future that outlives
        // its driving loop would keep the borrow open.
        {
            let handle = async {
                engine
                    .handle(&bootstrap, &CasperMessage::FinalizedFringe(fringe))
                    .await
            };
            tokio::pin!(handle);
            let mut handled = false;
            while !handled {
                tokio::select! {
                    result = &mut handle => {
                        result.expect("the fringe message is handled");
                        handled = true;
                    }
                    _ = tokio::time::sleep(Duration::from_millis(10)) => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "the fringe handler never returned"
                        );
                    }
                }
            }
        }
        // The attempt must actually run and fail: without this the "no notification" assertion below
        // would pass on a node that never tried, which is the vacuous shape this test exists to avoid.
        while !log.contains("LFS state sync failed") {
            assert!(
                std::time::Instant::now() < deadline,
                "the sync attempt did not fail as the fixture intends, so this test proves nothing"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // …and with the attempt failed, the node must stay where it is. One `biased` poll of the
        // waiter: it is ready if any `notify_waiters` happened after the waiter was created, which is
        // checked without a time bound.
        let notified = tokio::select! {
            biased;
            _ = &mut waiter => true,
            _ = tokio::task::yield_now() => false,
        };
        assert!(
            !notified,
            "a failed sync notified the sync-finished handle: `node_launch` would take the node into \
             NodeRunning on an empty or partial DAG"
        );

        // The approved block is not recorded either — the oracle sequences that after the state, and a
        // recorded fringe is what a restart would treat as a restored state.
        assert_eq!(
            approved_store
                .get(&[FINALIZED_FRINGE_KEY])
                .await
                .expect("approved store readable")[0],
            None,
            "a failed sync must not record the fringe as approved"
        );

        // --- C181: the attempt is retried, and the retries are bounded. ------------------------
        //
        // Each further bootstrap fringe triggers the next attempt without waiting out
        // `SYNC_RETRY_DELAY`, which is also the property the slot exists for: a later answer is a
        // *newer* target, so it is both the retry's trigger and the better thing to sync to. The
        // **timing assertion at the end of this test is what pins that**: retries driven by the
        // timeout alone would spend two `SYNC_RETRY_DELAY`s here, so a fringe that was received and
        // thrown away — the pre-fix behaviour — cannot get under the bound.
        let retry_started = std::time::Instant::now();
        for attempt in 1..=MAX_SYNC_ATTEMPTS {
            while !log.contains(&format!("attempt {attempt} of {MAX_SYNC_ATTEMPTS}")) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the fixture never reached attempt {attempt}: the attempt was not retried, which \
                     is the latch this row is about"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            if attempt < MAX_SYNC_ATTEMPTS {
                engine
                    .handle(
                        &bootstrap,
                        &CasperMessage::FinalizedFringe(FinalizedFringe {
                            hashes: vec![hash(7 + attempt as u8)],
                            state_hash: StateHash::new([7 + attempt as u8; 32]),
                            ancestry: Vec::new(),
                        }),
                    )
                    .await
                    .expect("a later bootstrap fringe is handled");
            }
        }

        // The bound is reached and it has a state to reach: `terminal` fires.
        assert!(
            tokio::time::timeout(Duration::from_secs(5), &mut terminal_waiter)
                .await
                .is_ok(),
            "the retries were spent and nothing signalled terminal: the node is left in the state \
             this row names — a serving API and no way out"
        );
        assert!(
            log.contains("is terminal"),
            "the terminal state must be logged, not only signalled"
        );

        // **The retries were driven by the fringes, not by the timeout.** Two `SYNC_RETRY_DELAY`s
        // would be 20 s; anything under one of them means the later fringes were received *and kept*,
        // which is the half the log line alone cannot show. A 2× margin over a path that fails in
        // milliseconds is wide enough for a loaded runner and far too narrow to be satisfied by the
        // timeout.
        assert!(
            retry_started.elapsed() < SYNC_RETRY_DELAY,
            "the retries took {:?}: they waited out the retry delay instead of waking on the later \
             fringes, which means the fringes were not kept",
            retry_started.elapsed()
        );
    }

    /// **AUDIT C181: a later bootstrap fringe is not discarded in silence — the *silence* half.**
    ///
    /// The pre-fix code logged the received fringe *inside* the `if start` branch, so once the trigger
    /// was consumed a later — and necessarily newer — answer produced no log line at all. The repo's
    /// own note calls that "a later fringe from the bootstrap logs nothing at all", and an operator
    /// cannot diagnose a fringe that was received and dropped: this pins that half, and only that half.
    ///
    /// **The "and kept" half is the retry test's timing assertion**, not this one: a log line says the
    /// fringe reached the handler, not that the sync task can still see it. Falsified against this
    /// tree by moving the log back inside the start branch: the second state hash appears nowhere.
    #[tokio::test]
    async fn a_later_bootstrap_fringe_is_not_discarded_in_silence() {
        // This test's sync task also writes the process-global `SYNC_STATE` (see the note on the
        // direct reading test).
        let _guard = SYNC_STATE_TESTS.lock().await;
        let log = Arc::new(RecordingLog::default());
        let (mut engine, bootstrap, _approved) = failing_engine(&log).await;

        let first = FinalizedFringe {
            hashes: vec![hash(1)],
            state_hash: StateHash::new([1u8; 32]),
            ancestry: Vec::new(),
        };
        engine
            .handle(&bootstrap, &CasperMessage::FinalizedFringe(first))
            .await
            .expect("the first fringe is handled");

        let second = FinalizedFringe {
            hashes: vec![hash(2)],
            state_hash: StateHash::new([2u8; 32]),
            ancestry: Vec::new(),
        };
        let second_state = rchain_shared::base16::encode(second.state_hash.as_bytes());
        engine
            .handle(&bootstrap, &CasperMessage::FinalizedFringe(second))
            .await
            .expect("the second fringe is handled");

        assert!(
            log.contains(&second_state),
            "the second bootstrap fringe was not even logged: it was discarded in silence, which is \
             exactly what a retry has to have and what this row is about"
        );
    }

    /// **Issue #100: the genesis master's empty fringe must not consume the one-shot sync trigger.**
    ///
    /// The master broadcasts `FinalizedFringe { hashes: Vec::new(), state_hash: genesis.pre_state }` as
    /// an *announcement* (`node_launch.rs`'s `create_store_broadcast_genesis`), not as a sync target, and
    /// on a fresh network it arrives **before** the answer to the joiner's own request — measured on a
    /// devnet at 34 ms after the master sent it and 83 ms before the master had even seen the request.
    /// Consuming the trigger on it made the node "restore" nothing and then discard the real answer in
    /// silence, because a second fringe from the bootstrap logs nothing at all.
    ///
    /// The evidence is a two-phase log observation, and the *first* phase is the discriminator: the
    /// ignore line is written synchronously inside `handle` before it returns, so its presence is
    /// exactly "the empty fringe was seen and refused" and cannot depend on scheduling. Phase 2 is the
    /// positive control — a store that cannot be read makes a *started* sync fail at once and say so, so
    /// the failure line can only appear if the trigger was still available. (Without Unit 1 the sync
    /// starts on the empty fringe in phase 1, and phase 2 produces nothing.)
    #[tokio::test]
    async fn an_empty_fringe_does_not_consume_the_sync_trigger() {
        // This test's sync task also writes the process-global `SYNC_STATE` (see the note on the
        // direct reading test).
        let _guard = SYNC_STATE_TESTS.lock().await;
        let bootstrap = peer("bootstrap");
        let log = Arc::new(RecordingLog::default());

        let inner_store: BlockStore = Arc::new(KeyValueTypedStoreCodec::new(
            in_memory(),
            Arc::new(BlockHashCodec),
            Arc::new(BlockMessageCodec),
        ));
        let block_store: BlockStore = Arc::new(UnreadableBlockStore { inner: inner_store });
        let dag = build_dag().await;
        let approved_store: ApprovedStore = Arc::new(KeyValueTypedStoreCodec::new(
            in_memory(),
            Arc::new(ByteCodec),
            Arc::new(FringeCodec),
        ));

        let transport: Arc<dyn TransportLayer> = Arc::new(SilentTransport);
        let conf = RPConf {
            local: peer("local"),
            network_id: "testnet".to_string(),
            bootstrap: Some(bootstrap.clone()),
            default_timeout: Duration::from_secs(10),
            max_num_of_connections: 10,
            clear_connections: ClearConnectionsConf {
                num_of_connections_pinged: 10,
            },
        };
        let connections: ConnectionsCell =
            Arc::new(tokio::sync::RwLock::new(vec![bootstrap.clone()]));
        let comm_util = Arc::new(CommUtil::new(
            transport.clone(),
            conf.clone(),
            connections,
            log.clone(),
        ));

        let mut engine = NodeSyncing::new(
            transport,
            conf,
            block_store,
            dag,
            approved_store,
            comm_util,
            log.clone(),
            None,
            false,
            RefusingImporter,
        );

        // Phase 1: the master's announcement, exactly as `create_store_broadcast_genesis` sends it.
        let announcement = FinalizedFringe {
            hashes: Vec::new(),
            state_hash: StateHash::new([0u8; 32]),
            ancestry: Vec::new(),
        };
        engine
            .handle(&bootstrap, &CasperMessage::FinalizedFringe(announcement))
            .await
            .expect("the empty fringe is handled");

        assert!(
            log.contains("Ignoring an empty fringe"),
            "an empty fringe must be refused as a sync target — it is the genesis master's \
             announcement, not an answer. If this is absent the fringe was consumed as one."
        );

        // Phase 2: a fringe that names a block. It must still be able to start the sync.
        let fringe = FinalizedFringe {
            hashes: vec![hash(7)],
            state_hash: StateHash::new([7u8; 32]),
            ancestry: Vec::new(),
        };
        engine
            .handle(&bootstrap, &CasperMessage::FinalizedFringe(fringe))
            .await
            .expect("the named fringe is handled");

        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while !log.contains("LFS state sync failed") {
            assert!(
                std::time::Instant::now() < deadline,
                "the fringe that named a block did not start a sync: the one-shot trigger was \
                 consumed by the empty announcement, which is issue #100"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    // --- Sync/verification separation: the certified path -------------------------------
    //
    // The certificate is the acceptance rule now. These tests pin the gate's refusals: no source
    // (the old unsigned path), a quorum short of the rule, a statement for another shard, a
    // certificate over a different fringe, and the metadata walk's requirement that every body be
    // the requested block and every non-genesis block carry the claim the quorum signed.

    use crate::checkpoint::checkpoint_digest;
    use crate::engine::checkpoint_source::CheckpointSource;
    use crate::validator_identity::checkpoint_attestation_prehash;
    use rchain_crypto::private_key::PrivateKey;
    use rchain_crypto::signatures::signatures_alg::SignaturesAlg;
    use rchain_models::casper::protocol::checkpoint::{
        Checkpoint, CheckpointAttestation, CheckpointCertificate,
    };
    use rchain_shared::base16;
    use rchain_shared::refined::NonNegI64;

    /// A body that passes `format_of_fields` and `block_hash`: the sync path checks both before it
    /// accepts a block, and `chain_block`'s fabricated hash would fail on every one.
    fn sealed_block(height: i64, parent: Option<BlockHash>) -> BlockMessage {
        let mut b = chain_block(height, parent);
        b.sig_algorithm = "secp256k1".to_string();
        b.sig = vec![1u8; 64];
        b.block_hash = crate::proto_util::hash_block(&b);
        b
    }

    fn signed_certificate(
        checkpoint: &Checkpoint,
    ) -> (CheckpointCertificate, BTreeMap<Validator, NonNegI64>) {
        let key = PrivateKey::new(vec![7u8; 32]);
        let public = rchain_crypto::signatures::secp256k1::Secp256k1
            .to_public(&key)
            .expect("public key");
        let validator = Validator::try_from(public.bytes()).expect("validator");
        let digest = checkpoint_digest(checkpoint).expect("digest");
        let sig = rchain_crypto::signatures::secp256k1::Secp256k1
            .sign(&checkpoint_attestation_prehash(&digest), key.bytes())
            .expect("sign");
        let attestation = CheckpointAttestation {
            checkpoint_digest: digest.to_hex(),
            sender: base16::encode(public.bytes()),
            sig_algorithm: "secp256k1".to_string(),
            sig: base16::encode(&sig),
        };
        let bonds = BTreeMap::from([(validator, NonNegI64::try_from(100).expect("stake"))]);
        (
            CheckpointCertificate {
                checkpoint: checkpoint.clone(),
                attestations: vec![attestation],
            },
            bonds,
        )
    }

    fn statement(anchor: BlockHash, fringe: Vec<BlockHash>) -> Checkpoint {
        Checkpoint {
            shard_id: "root".to_string(),
            anchor: base16::encode(anchor.as_bytes()),
            anchor_height: 1,
            anchor_post_state: base16::encode(anchor.as_bytes()),
            fringe_hashes: fringe
                .iter()
                .map(|h| base16::encode(h.as_bytes()))
                .collect(),
            parent_digest: None,
            epoch: 0,
            metadata_root: None,
        }
    }

    struct FixedCertificate(CheckpointCertificate);

    #[async_trait]
    impl CheckpointSource for FixedCertificate {
        async fn fetch_certificate(&self) -> Result<CheckpointCertificate, String> {
            Ok(self.0.clone())
        }
    }

    fn fringe_of(hashes: Vec<BlockHash>) -> FinalizedFringe {
        FinalizedFringe {
            hashes,
            state_hash: StateHash::new([0u8; 32]),
            ancestry: Vec::new(),
        }
    }

    #[tokio::test]
    async fn an_unsigned_sync_has_no_fallback_path() {
        let bonds = BTreeMap::new();
        let err = certified_checkpoint(None, &bonds, "root", &fringe_of(vec![hash(1)]), None, None)
            .await
            .expect_err("a sync with no certificate source must fail closed");
        assert!(err.contains("no checkpoint source"), "{err}");
        assert!(
            err.contains("unsigned fringe"),
            "the refusal must say what it refused: {err}"
        );
    }

    #[tokio::test]
    async fn a_quorum_short_of_the_rule_is_refused() {
        let checkpoint = statement(hash(1), vec![hash(1)]);
        let (_certificate, bonds) = signed_certificate(&checkpoint);
        let empty = CheckpointCertificate {
            checkpoint: checkpoint.clone(),
            attestations: Vec::new(),
        };
        let err = certified_checkpoint(
            Some(&FixedCertificate(empty)),
            &bonds,
            "root",
            &fringe_of(vec![hash(1)]),
            None,
            None,
        )
        .await
        .expect_err("a certificate with no signatures must be refused");
        assert!(err.contains("not quorate"), "{err}");

        // The same statement with its signer is accepted: the refusal above is the quorum, not a
        // broken fixture.
        let (certificate, bonds) = signed_certificate(&checkpoint);
        certified_checkpoint(
            Some(&FixedCertificate(certificate)),
            &bonds,
            "root",
            &fringe_of(vec![hash(1)]),
            None,
            None,
        )
        .await
        .expect("a quorate certificate over the served fringe is accepted");
    }

    #[tokio::test]
    async fn a_certificate_for_another_shard_is_refused() {
        let mut checkpoint = statement(hash(1), vec![hash(1)]);
        checkpoint.shard_id = "other".to_string();
        let (certificate, bonds) = signed_certificate(&checkpoint);
        let err = certified_checkpoint(
            Some(&FixedCertificate(certificate)),
            &bonds,
            "root",
            &fringe_of(vec![hash(1)]),
            None,
            None,
        )
        .await
        .expect_err("a certificate for another shard must be refused");
        assert!(err.contains("names shard"), "{err}");
    }

    #[tokio::test]
    async fn a_certificate_over_another_fringe_is_refused() {
        let checkpoint = statement(hash(1), vec![hash(1)]);
        let (certificate, bonds) = signed_certificate(&checkpoint);
        let err = certified_checkpoint(
            Some(&FixedCertificate(certificate)),
            &bonds,
            "root",
            &fringe_of(vec![hash(2)]),
            None,
            None,
        )
        .await
        .expect_err("a signed statement and a served history must be the same history");
        assert!(err.contains("not the fringe the bootstrap served"), "{err}");
    }

    fn header_of(height: i64, parent: Option<BlockHash>) -> BlockHeader {
        let block = sealed_block(height, parent);
        rchain_models::casper::protocol::casper_message::BlockHeader {
            block_hash: block.block_hash,
            block_num: block.block_number,
            sender: block.sender,
            seq_num: block.seq_num,
            justifications: block.justifications.iter().copied().collect(),
            bonds_map: block.bonds.clone(),
            fringe: BTreeSet::new(),
            fringe_state_hash: block.post_state_hash,
            pre_state_hash: block.pre_state_hash,
            post_state_hash: block.post_state_hash,
            body_available: false,
            sig: block.sig.clone(),
            sig_algorithm: block.sig_algorithm.clone(),
        }
    }

    fn walk_of(headers: Vec<BlockHeader>) -> crate::engine::lfs_metadata_requester::MetadataWalk {
        crate::engine::lfs_metadata_requester::MetadataWalk {
            headers: headers.into_iter().map(|h| (h.block_hash, h)).collect(),
        }
    }

    fn certified_checkpoint_over(
        anchor: BlockHash,
        post: StateHash,
        fringe: Vec<BlockHash>,
    ) -> Checkpoint {
        Checkpoint {
            shard_id: "root".to_string(),
            anchor: base16::encode(anchor.as_bytes()),
            anchor_height: 2,
            anchor_post_state: base16::encode(post.as_bytes()),
            fringe_hashes: fringe
                .iter()
                .map(|h| base16::encode(h.as_bytes()))
                .collect(),
            parent_digest: None,
            epoch: 0,
            metadata_root: None,
        }
    }

    #[test]
    fn a_certified_metadata_history_carries_the_quorums_claim_on_each_block() {
        let genesis = header_of(0, None);
        let child = header_of(1, Some(genesis.block_hash));
        let mut grandchild = header_of(2, Some(child.block_hash));
        grandchild.fringe = BTreeSet::from([child.block_hash]);
        let walk = walk_of(vec![genesis.clone(), child.clone(), grandchild.clone()]);
        let mut checkpoint = certified_checkpoint_over(
            grandchild.block_hash,
            grandchild.post_state_hash,
            vec![grandchild.block_hash],
        );
        // The statement commits to the headers the walk is about to accept.
        checkpoint.metadata_root = Some(
            rchain_models::casper::protocol::checkpoint::metadata_root(walk.headers.values())
                .to_hex(),
        );
        let (entries, fringe_root) = certified_metadata_entries(
            &walk,
            &checkpoint,
            Blake2b256Hash::from_byte_array(grandchild.post_state_hash.as_bytes()),
        )
        .expect("a certified header set publishes");
        assert_eq!(entries.len(), 3);
        assert!(
            entries
                .windows(2)
                .all(|w| w[0].1.block_num <= w[1].1.block_num),
            "parents are published before children"
        );
        let (_, anchor_meta) = entries.last().expect("anchor entry");
        assert!(anchor_meta.validated, "accepted under the certificate");
        assert!(!anchor_meta.validation_failed);
        assert_eq!(anchor_meta.fringe, BTreeSet::from([child.block_hash]));
        assert_eq!(anchor_meta.sig, grandchild.sig, "the signature survives");
        assert_eq!(
            fringe_root,
            Blake2b256Hash::from_byte_array(grandchild.fringe_state_hash.as_bytes())
        );
    }

    /// A legacy statement has no metadata commitment; the sync path refuses it rather than treating
    /// an uncommitted header set as authenticated.
    #[test]
    fn a_metadata_history_without_a_commitment_is_refused() {
        let genesis = header_of(0, None);
        let child = header_of(1, Some(genesis.block_hash));
        let grandchild = header_of(2, Some(child.block_hash));
        let walk = walk_of(vec![genesis.clone(), child.clone(), grandchild.clone()]);
        let checkpoint = certified_checkpoint_over(
            grandchild.block_hash,
            grandchild.post_state_hash,
            vec![grandchild.block_hash],
        );
        let err = certified_metadata_entries(
            &walk,
            &checkpoint,
            Blake2b256Hash::from_byte_array(grandchild.post_state_hash.as_bytes()),
        )
        .expect_err("a legacy statement has no metadata commitment");
        assert!(err.contains("does not commit"), "got {err}");
    }

    /// **Metadata tampering under an unchanged valid certificate.** The certificate's root is
    /// computed over the original headers; one committed field of one middle header changes, and
    /// the joiner refuses the set. This is the arm the audit asked for.
    #[test]
    fn a_tampered_metadata_header_is_refused_under_the_same_certificate() {
        let genesis = header_of(0, None);
        let child = header_of(1, Some(genesis.block_hash));
        let grandchild = header_of(2, Some(child.block_hash));
        let mut walk = walk_of(vec![genesis.clone(), child.clone(), grandchild.clone()]);
        let mut checkpoint = certified_checkpoint_over(
            grandchild.block_hash,
            grandchild.post_state_hash,
            vec![grandchild.block_hash],
        );
        checkpoint.metadata_root = Some(
            rchain_models::casper::protocol::checkpoint::metadata_root(walk.headers.values())
                .to_hex(),
        );
        let tampered = walk
            .headers
            .get_mut(&child.block_hash)
            .expect("the child header is in the walk");
        tampered.pre_state_hash = StateHash::new([0x9A; 32]);
        let err = certified_metadata_entries(
            &walk,
            &checkpoint,
            Blake2b256Hash::from_byte_array(grandchild.post_state_hash.as_bytes()),
        )
        .expect_err("the headers are not the set the certificate committed to");
        assert!(err.contains("commits"), "got {err}");
    }

    #[test]
    fn a_metadata_anchor_must_be_the_fringes_maximum_member() {
        let genesis = header_of(0, None);
        let child = header_of(1, Some(genesis.block_hash));
        let grandchild = header_of(2, Some(child.block_hash));
        let walk = walk_of(vec![genesis.clone(), child.clone(), grandchild.clone()]);
        let checkpoint = Checkpoint {
            anchor: base16::encode(child.block_hash.as_bytes()),
            anchor_height: 1,
            anchor_post_state: base16::encode(child.post_state_hash.as_bytes()),
            fringe_hashes: vec![
                base16::encode(child.block_hash.as_bytes()),
                base16::encode(grandchild.block_hash.as_bytes()),
            ],
            parent_digest: None,
            epoch: 0,
            shard_id: "root".to_string(),
            metadata_root: None,
        };
        let err = certified_metadata_entries(
            &walk,
            &checkpoint,
            Blake2b256Hash::from_byte_array(child.post_state_hash.as_bytes()),
        )
        .expect_err("the anchor rule is max (height, hash)");
        assert!(err.contains("max (height, hash)"), "{err}");
    }

    #[test]
    fn a_metadata_anchor_must_carry_the_certified_post_state() {
        let genesis = header_of(0, None);
        let child = header_of(1, Some(genesis.block_hash));
        let walk = walk_of(vec![genesis.clone(), child.clone()]);
        let checkpoint = Checkpoint {
            anchor: base16::encode(child.block_hash.as_bytes()),
            anchor_height: 1,
            anchor_post_state: base16::encode(&[0xAB; 32]),
            fringe_hashes: vec![base16::encode(child.block_hash.as_bytes())],
            parent_digest: None,
            epoch: 0,
            shard_id: "root".to_string(),
            metadata_root: None,
        };
        let err = certified_metadata_entries(
            &walk,
            &checkpoint,
            Blake2b256Hash::from_byte_array(&[0xAB; 32]),
        )
        .expect_err("the header's post-state must be the certificate's");
        assert!(err.contains("post-state"), "{err}");
    }

    #[test]
    fn a_metadata_walk_needs_exactly_one_genesis_and_ordered_parents() {
        let genesis = header_of(0, None);
        let child = header_of(1, Some(genesis.block_hash));
        crate::engine::lfs_metadata_requester::check_walk_shape(&walk_of(vec![
            genesis.clone(),
            child.clone(),
        ]))
        .expect("one genesis, ordered parents");

        // A second parentless header is not a history.
        let mut another_root = header_of(0, None);
        another_root.block_hash = hash(9);
        let err = crate::engine::lfs_metadata_requester::check_walk_shape(&walk_of(vec![
            genesis.clone(),
            another_root,
        ]))
        .expect_err("two parentless blocks are refused");
        assert!(err.contains("exactly one parentless"), "{err}");

        // A missing parent is refused by name.
        let orphan = header_of(2, Some(hash(8)));
        let err = crate::engine::lfs_metadata_requester::check_walk_shape(&walk_of(vec![
            genesis, child, orphan,
        ]))
        .expect_err("a header naming an absent parent is refused");
        assert!(err.contains("did not receive"), "{err}");
    }
}
