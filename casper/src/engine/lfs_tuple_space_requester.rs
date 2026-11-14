//! Last Finalized State tuple-space requester (port of `engine/LfsTupleSpaceRequester.scala`).
//!
//! Downloads the rholang state (history + data items) for the last finalized state in chunks, via
//! the pure `LfsTupleSpaceState` state machine and the effectful `stream` orchestration.

use rchain_shared::chan;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use rchain_comm::peer_node::PeerNode;
use rchain_comm::rp::connect::random_connections;
use rchain_comm::rp::rp_conf::RPConf;
use rchain_comm::transport::transport_layer::TransportLayer;
use rchain_comm::transport::transport_layer_syntax;
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_models::casper::protocol::casper_message::{
    FinalizedFringe, StoreItemsMessage, StoreItemsMessageRequest,
};
use rchain_models::casper::protocol::packet_type_tag::ToPacket;
use rchain_rspace::state::{validate_state_items, RSpaceImporter, StateValidationError};
use rchain_shared::log::{Log, LogSource};

use crate::protocol::casper_message_protocol::StoreItemsMessageRequestSerde;

/// A rspace state path: nested `(hash, index)` levels, with `index` as the pointer-block index
/// (port of `StatePartPath`).
pub type StatePartPath = Vec<(Blake2b256Hash, Option<u8>)>;

/// Number of nodes requested in an LFS sync data-transfer chunk (port of `pageSize`).
///
/// 750 was the Scala's page size; the Rust responder already bounds a page at
/// `MAX_STORE_ITEMS_TAKE = 10_000` nodes and 32 MiB, and now serves a **byte-capped prefix** rather
/// than refusing an oversized page. A larger page is therefore safe and cuts the round trips on a
/// large trie by more than 5×.
pub const PAGE_SIZE: i32 = 4_000;

/// **How many peers one page path is requested from in a round** (bootstrap included). Two makes
/// a silent primary recoverable within the next answer rather than the 30 s idle window; the page
/// is content-addressed, so the peers are interchangeable, and a duplicate answer is ignored.
pub const LFS_PAGE_FANOUT: usize = 2;

/// Request status (port of `LfsTupleSpaceRequester.ReqStatus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReqStatus {
    Init,
    Requested,
    Received,
    Done,
}

/// The tuple-space requester state machine (port of `LfsTupleSpaceRequester.ST`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LfsTupleSpaceState<Key: Ord + Clone> {
    d: BTreeMap<Key, ReqStatus>,
}

impl<Key: Ord + Clone> LfsTupleSpaceState<Key> {
    /// Create the state with the initial keys in `Init` status (port of `ST.apply`).
    pub fn new(initial: Vec<Key>) -> Self {
        LfsTupleSpaceState {
            d: initial.into_iter().map(|k| (k, ReqStatus::Init)).collect(),
        }
    }

    /// Add new keys in `Init` status, skipping existing keys (port of `add`).
    pub fn add(&self, keys: &BTreeSet<Key>) -> Self {
        let mut d = self.d.clone();
        for k in keys {
            d.entry(k.clone()).or_insert(ReqStatus::Init);
        }
        LfsTupleSpaceState { d }
    }

    /// Get the next keys to request, marking them `Requested` (port of `getNext`).
    pub fn get_next(&self, resend: bool) -> (Self, Vec<Key>) {
        let requested: Vec<Key> = self
            .d
            .iter()
            .filter(|(_, v)| **v == ReqStatus::Init || (resend && **v == ReqStatus::Requested))
            .map(|(k, _)| k.clone())
            .collect();
        let mut d = self.d.clone();
        for k in &requested {
            d.insert(k.clone(), ReqStatus::Requested);
        }
        (LfsTupleSpaceState { d }, requested)
    }

    /// Mark `k` received if it was requested, returning whether it was requested (port of
    /// `received`).
    pub fn received(&self, k: Key) -> (Self, bool) {
        let is_requested = self.d.get(&k) == Some(&ReqStatus::Requested);
        let mut d = self.d.clone();
        if is_requested {
            d.insert(k, ReqStatus::Received);
        }
        (LfsTupleSpaceState { d }, is_requested)
    }

    /// Mark `k` done if it was received (port of `done`).
    pub fn done(&self, k: Key) -> Self {
        let is_received = self.d.get(&k) == Some(&ReqStatus::Received);
        let mut d = self.d.clone();
        if is_received {
            d.insert(k, ReqStatus::Done);
        }
        LfsTupleSpaceState { d }
    }

    /// Whether all keys are done (port of `isFinished`).
    pub fn is_finished(&self) -> bool {
        self.d.values().all(|v| *v == ReqStatus::Done)
    }

    /// **The give-up rule's measure** (Law 69, `spec/Rchain/Sync/Walk.lean`'s `finished`): how many
    /// keys the walk has completed.
    ///
    /// Monotone by construction — [`done`](Self::done) only moves `Received → Done` and
    /// [`add`](Self::add) refuses a key it already holds — which is what makes "did it move since the
    /// last idle round?" a well-formed question, and what makes the give-up rule a **pace** condition
    /// rather than a deadline. The block leg counts the same quantity as `finished.len()`
    /// (`lfs_block_requester.rs:250-252`); this is its accessor on the page walk's map.
    pub fn finished_count(&self) -> usize {
        self.d.values().filter(|v| **v == ReqStatus::Done).count()
    }

    /// How many keys the walk still holds — the block leg's `d.len()`
    /// (`lfs_block_requester.rs:251`), reported beside the measure so a failure message can tell a
    /// walk that is stuck from one that is nearly done.
    pub fn outstanding_count(&self) -> usize {
        self.d.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(items: &[i32]) -> BTreeSet<i32> {
        items.iter().copied().collect()
    }

    fn as_set(items: &[i32]) -> BTreeSet<i32> {
        items.iter().copied().collect()
    }

    #[test]
    fn get_next_returns_empty_when_called_again() {
        let st = LfsTupleSpaceState::new(vec![10]);
        let (st1, ids1) = st.get_next(false);
        assert_eq!(as_set(&ids1), keys(&[10]));

        let (st2, ids2) = st1.get_next(false);
        assert!(ids2.is_empty());
        assert_eq!(st1, st2);
    }

    #[test]
    fn get_next_returns_new_items_after_add() {
        let st = LfsTupleSpaceState::new(vec![10]);
        let st2 = st.add(&keys(&[9, 8]));
        let (_, ids2) = st2.get_next(false);
        assert_eq!(as_set(&ids2), keys(&[10, 9, 8]));
    }

    #[test]
    fn get_next_returns_requested_on_resend() {
        let st = LfsTupleSpaceState::new(vec![10]);
        let (st1, ids1) = st.get_next(false);
        assert_eq!(as_set(&ids1), keys(&[10]));

        let (_, ids2) = st1.get_next(true);
        assert_eq!(as_set(&ids2), keys(&[10]));
    }

    #[test]
    fn received_true_for_requested_false_for_unknown() {
        let st = LfsTupleSpaceState::new(vec![10]);
        let (st1, _) = st.get_next(false);

        let (_, is_received) = st1.received(10);
        assert!(is_received);

        let (_, is_received) = st1.received(100);
        assert!(!is_received);
    }

    #[test]
    fn done_makes_state_finished() {
        let st = LfsTupleSpaceState::new(vec![10]);

        let st1 = st.done(10);
        assert!(!st1.is_finished());

        let (st2, _) = st1.get_next(false);
        let (st3, _) = st2.received(10);
        let st4 = st3.done(10);
        assert!(st4.is_finished());
    }

    #[test]
    fn start_to_finish_receives_one_item() {
        let st = LfsTupleSpaceState::new(vec![10]);

        let (st1, ids1) = st.get_next(false);
        assert_eq!(as_set(&ids1), keys(&[10]));

        let (st2, ids2) = st1.get_next(false);
        assert!(ids2.is_empty());
        assert!(!st2.is_finished());

        let (st3, is_received) = st2.received(10);
        assert!(is_received);

        let st4 = st3.done(10);
        assert!(st4.is_finished());
    }
}

// -------------------------------------------------------------------------------------------------
// Effectful stream (port of `LfsTupleSpaceRequester.stream`)
// -------------------------------------------------------------------------------------------------

// -------------------------------------------------------------------------------------------------
// The give-up rule and the exit verdict (Law 69, C271)
// -------------------------------------------------------------------------------------------------

/// How many **consecutive** idle resend intervals may complete no state page before the walk gives up
/// and the sync attempt fails.
///
/// **The quantity is [`finished_count`](LfsTupleSpaceState::finished_count) — pages the walk has
/// completed — and it is monotone**, which is what makes "did it move" a well-formed question. So the
/// rule is a **pace** condition, not a deadline, exactly as the block leg's is
/// (`lfs_block_requester.rs:36-52`): steps continue, the measure does not move, and nothing bounds the
/// steps between two increases — **Law 69**'s `Drift` shape, which
/// `Rchain.Sync.Walk.the_page_walk_as_written_is_unpaced` refuses of the walk as it was written and
/// `Rchain.Sync.Walk.the_page_walk_with_the_give_up_rule_is_paced` and
/// `the_fixed_walk_cannot_run_for_ever` prove of the walk carrying this rule.
///
/// **The pair (`MAX_IDLE_ROUNDS`, [`REQUEST_TIMEOUT`]) is the bound, and the wall clock is a
/// decision.** This constant is copied from the block leg, whose rationale is written against a 30 s
/// interval (`lfs_block_requester.rs:44-47`); the page walk used to be called with **120 s** at both of
/// `node_syncing.rs`'s call sites, so a verbatim copy would have meant six minutes of silence — the
/// companion hazard C271 names. The interval is therefore brought to the block leg's own 30 s, and the
/// pair is **3 × 30 s**: *an operator waits 90 seconds of complete silence — no page completing on any
/// outstanding path — before the page-walk leg fails the attempt*. That is the same number the block
/// leg bounds its own leg with, so the `join!` in `run_approved_state_sync` bounds an attempt at the one
/// wall clock `max(block, tuple) = 90 s`, which is what makes that function's retry-bound doc ("the
/// total wait is bounded by the product of the two") a product of two *bounds*.
///
/// A *slow* walk is not touched: the counter resets on every completed page, so only consecutive empty
/// rounds count, and `a_slow_but_progressing_walk_is_not_abandoned` pins that.
///
/// **Provisional, and deliberately a constant.** It is node-local policy — it changes no state and no
/// hash — so a `PosParams` field would be wrong, but what it *should* be is a measurement; this is the
/// same treatment the block leg's constant and `block-storage/src/dag/liveness.rs`'s `LIVENESS_WINDOW`
/// record for their own provisional numbers.
pub const MAX_IDLE_ROUNDS: u32 = 3;

/// The idle resend interval the page walk is driven with — how long one round of silence lasts, and
/// therefore the unit [`MAX_IDLE_ROUNDS`] counts in.
///
/// **Named rather than left to the call site so that the pair is one decision**: the give-up's bound is
/// `MAX_IDLE_ROUNDS × REQUEST_TIMEOUT`, and a caller that changed one without the other would silently
/// change the bound — the mismatch C271 found, where the rule assumed 30 s and the caller passed 120 s.
/// It is the block leg's own interval (`lfs_block_requester.rs:44-47`), so both legs of `run_approved_state_sync`'s
/// `join!` bound an attempt at the same wall clock.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Why the state-page walk failed (C271, Law 69).
///
/// **The two cases are different facts and the walk keeps them apart.** [`StateValidation`] means a
/// peer *answered* and the state it sent failed to check against the trie — the walk's integrity gate
/// fired, so the peer lied or erred. [`Abandoned`] means *nobody answered*: no page arrived to
/// complete a key within the give-up bound, or a channel closed under the walk, so there is nothing to
/// check and no state that failed. Folding the second into the first would report a silent peer as
/// having sent bad state, which is a different and false claim — which is why this is its own type
/// rather than a variant of `rspace`'s `StateValidationError`, a type that is rspace's own and means
/// exactly one thing.
///
/// [`StateValidation`]: TupleSpaceWalkError::StateValidation
/// [`Abandoned`]: TupleSpaceWalkError::Abandoned
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TupleSpaceWalkError {
    /// A received page failed validation: the peer answered, but the state it sent does not check.
    StateValidation(StateValidationError),
    /// The walk gave up before reaching `is_finished`, or a channel closed under it. The message names
    /// the silence and the walk's measure.
    Abandoned(String),
}

impl std::fmt::Display for TupleSpaceWalkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TupleSpaceWalkError::StateValidation(e) => write!(f, "{e}"),
            TupleSpaceWalkError::Abandoned(m) => {
                write!(f, "the state-page walk was abandoned: {m}")
            }
        }
    }
}

impl std::error::Error for TupleSpaceWalkError {}

impl From<StateValidationError> for TupleSpaceWalkError {
    fn from(e: StateValidationError) -> Self {
        TupleSpaceWalkError::StateValidation(e)
    }
}

/// **The verdict at every exit** (C271, Law 69): an `Ok` page walk is a *finished* page walk, and
/// nothing else.
///
/// `is_finished` is the contract, and this is the one place it is read on the way out, so that a future
/// exit added to either loop cannot silently return `Ok` on a partial state. That is exactly what
/// `request_tuple_space_roots` did: it returned `Ok` after the `select!` without checking, and its
/// caller dropped the state, so a walk that stopped early was indistinguishable from one that completed
/// and the node went on to `NodeRunning` holding a partial state. `Ok(())` here means "the walk reached
/// its goal"; `Err` names what was expected and what was found, the way the block leg's empty-walk
/// refusal does in `run_approved_state_sync`.
fn finished_or_abandoned(
    st: &LfsTupleSpaceState<StatePartPath>,
    why: &str,
) -> Result<(), TupleSpaceWalkError> {
    if st.is_finished() {
        Ok(())
    } else {
        Err(TupleSpaceWalkError::Abandoned(format!(
            "{why}: {} key(s) still outstanding ({} finished), so the walk did not reach \
             `is_finished` — a partial state is not a restored one",
            st.outstanding_count(),
            st.finished_count()
        )))
    }
}

/// Take the next set of state-part paths to request and send a `StoreItemsMessageRequest` for each
/// (port of `requestStream`'s broadcast step).
async fn request_next(
    st: &Arc<tokio::sync::Mutex<LfsTupleSpaceState<StatePartPath>>>,
    transport: &dyn TransportLayer,
    conf: &RPConf,
    peers: &[PeerNode],
    log: &dyn Log,
    source: LogSource,
    resend: bool,
) {
    let is_end = { st.lock().await.is_finished() };
    let ids = {
        let mut guard = st.lock().await;
        let (new_state, ids) = guard.get_next(resend);
        *guard = new_state;
        ids
    };
    if !is_end && !ids.is_empty() {
        for id in &ids {
            // **The path is the walk's cursor, and nothing logged it.** A joiner stuck in this loop is
            // indistinguishable from one that is not asking at all unless the path is visible: the
            // capture that found `Sending 29 history and 230 data store items` 440 times on the
            // responder side had no requester-side counterpart to compare it with (C268, pass §94).
            let req = StoreItemsMessageRequest {
                start_path: id.clone(),
                skip: 0,
                take: PAGE_SIZE,
            };
            // **Redundant page fetch** (C339 performance): the bootstrap first, then up to
            // `LFS_PAGE_FANOUT - 1` connected peers. A page is content-addressed and
            // `validate_state_items` refuses any node that does not hash to its key, so which peer
            // answers is not an input to the restored state; a silent bootstrap no longer costs the
            // whole 30 s idle window. A duplicate answer for an already-received path is ignored by
            // the state machine (`received` accepts only `Requested`).
            let mut targets: Vec<PeerNode> = Vec::new();
            if let Some(bootstrap) = &conf.bootstrap {
                targets.push(bootstrap.clone());
            }
            let candidates: Vec<PeerNode> = peers
                .iter()
                .filter(|p| p.id != conf.local.id)
                .filter(|p| !targets.iter().any(|q| q.id == p.id))
                .cloned()
                .collect();
            targets.extend(random_connections(
                &candidates,
                LFS_PAGE_FANOUT.saturating_sub(targets.len()),
            ));
            log.info(
                source,
                &format!(
                    "Sending StoreItemsRequest to {} peer(s) for path {id:?}",
                    targets.len()
                ),
            );
            crate::engine::sync_metrics::census().add_pages_requested(targets.len() as u64);
            for peer in &targets {
                if let Err(e) = transport_layer_syntax::send_to_peer(
                    transport,
                    conf,
                    peer,
                    StoreItemsMessageRequestSerde.mk_packet(&req),
                )
                .await
                {
                    log.warn(
                        source,
                        &format!(
                            "could not ask {} for store items: {e} (AUDIT C254's E6b)",
                            peer
                        ),
                    );
                }
            }
        }
    }
}

/// Process an incoming `StoreItemsMessage`: validate it, import its history/data items, and mark the
/// chunk done (port of `responseStream`).
async fn process_store_items<I: RSpaceImporter>(
    st: &Arc<tokio::sync::Mutex<LfsTupleSpaceState<StatePartPath>>>,
    importer: &mut I,
    log: &dyn Log,
    source: LogSource,
    request_tx: &tokio::sync::mpsc::Sender<bool>,
    msg: &StoreItemsMessage,
) -> Result<(), StateValidationError> {
    let start_path = msg.start_path.clone();
    let is_received = {
        let mut guard = st.lock().await;
        let (new_state, is_received) = guard.received(start_path.clone());
        *guard = new_state;
        is_received
    };

    if is_received {
        // Add the last path for requesting and trigger the request queue.
        {
            let last_path: BTreeSet<StatePartPath> = [msg.last_path.clone()].into_iter().collect();
            let mut guard = st.lock().await;
            *guard = guard.add(&last_path);
        }
        chan::send(request_tx, false).await;

        // Validate received state items against the trie.
        validate_state_items(
            &msg.history_items,
            &msg.data_items,
            &start_path,
            PAGE_SIZE,
            0,
            &|h| importer.get_history_item(*h),
        )
        .map_err(|e| {
            log.error(source, &format!("Invalid state items received: {e:?}"));
            e
        })?;

        // Import history and data items.
        // A refused write is not a completed import (AUDIT C63's write half): propagating it here is
        // what makes the LFS sync's "chunk done" claim conditional on the state actually landing.
        importer
            .set_history_items(&msg.history_items, |v: &Vec<u8>| v.clone())
            .map_err(StateValidationError)?;
        importer
            .set_data_items(&msg.data_items, |v: &Vec<u8>| v.clone())
            .map_err(StateValidationError)?;

        // Mark the chunk done and trigger the request queue.
        {
            let mut guard = st.lock().await;
            *guard = guard.done(start_path.clone());
        }
        let bytes: u64 = msg
            .history_items
            .iter()
            .map(|(_, v)| v.len() as u64)
            .chain(msg.data_items.iter().map(|(_, v)| v.len() as u64))
            .sum();
        crate::engine::sync_metrics::census().add_pages_received(1, bytes);
        chan::send(request_tx, false).await;
    }
    Ok(())
}

/// Request the tuple space (history + data items) for the last finalized state (port of
/// `LfsTupleSpaceRequester.stream`). Returns the final requester state once all chunks are received.
pub async fn request_tuple_space<I: RSpaceImporter>(
    fringe: &FinalizedFringe,
    tuple_space_rx: &mut tokio::sync::mpsc::Receiver<StoreItemsMessage>,
    request_timeout: Duration,
    transport: &dyn TransportLayer,
    conf: &RPConf,
    peers: &[PeerNode],
    importer: &mut I,
    log: &dyn Log,
) -> Result<LfsTupleSpaceState<StatePartPath>, TupleSpaceWalkError> {
    let state_hash = Blake2b256Hash::from_byte_array(fringe.state_hash.as_bytes());
    request_tuple_space_roots(
        &[state_hash],
        tuple_space_rx,
        request_timeout,
        transport,
        conf,
        peers,
        importer,
        log,
    )
    .await
}

/// Request tuple-space data for one or more concrete state roots.
///
/// Approved-state sync restores the finalized-fringe root first, then may need to hydrate the
/// pre/post roots of downloaded blocks because validation and read APIs open those roots directly.
pub async fn request_tuple_space_roots<I: RSpaceImporter>(
    state_hashes: &[Blake2b256Hash],
    tuple_space_rx: &mut tokio::sync::mpsc::Receiver<StoreItemsMessage>,
    request_timeout: Duration,
    transport: &dyn TransportLayer,
    conf: &RPConf,
    peers: &[PeerNode],
    importer: &mut I,
    log: &dyn Log,
) -> Result<LfsTupleSpaceState<StatePartPath>, TupleSpaceWalkError> {
    let source = LogSource::new("casper.engine.LfsTupleSpaceRequester");

    // A walk over no root is **the vacuous completion** `is_finished` already names: an empty map is
    // finished, so this is a genuine `Ok` and every other exit below has to earn one.
    if state_hashes.is_empty() {
        return Ok(LfsTupleSpaceState::new(Vec::new()));
    }

    let mut start_requests = Vec::with_capacity(state_hashes.len());
    for state_hash in state_hashes {
        importer
            .set_root(*state_hash)
            .map_err(StateValidationError)?;
        start_requests.push(vec![(*state_hash, None)]);
    }

    let st = Arc::new(tokio::sync::Mutex::new(LfsTupleSpaceState::new(
        start_requests,
    )));
    let (request_tx, mut request_rx) = tokio::sync::mpsc::channel::<bool>(2);
    chan::send(&request_tx, false).await;

    // **The shared failure slot holds either kind of failure** (C271): a page that failed to check
    // (`StateValidation`) or a walk nobody answered (`Abandoned`). The loop that ends first writes it
    // here, and the exit below reads it — which is what makes every exit below a `Result` rather than a
    // silent `return`.
    let error: Arc<tokio::sync::Mutex<Option<TupleSpaceWalkError>>> =
        Arc::new(tokio::sync::Mutex::new(None));

    // Request loop: pull request triggers (or resend on idle timeout), terminating when finished — or,
    // now, when the give-up rule is spent (Law 69). The counter state is the block leg's
    // (`lfs_block_requester.rs:236-238`): how many pages were finished at the last idle round that saw
    // progress, and how many consecutive idle rounds have seen none since.
    let request_loop = async {
        // **Short resends, long give-up** (the block leg's rule, C339 performance): re-ask the
        // outstanding paths every `resend_interval` (2 s in production) and give up only after
        // `request_timeout × MAX_IDLE_ROUNDS` with no completed page, so a silent peer costs a
        // resend rather than a whole 30-second window while the pace measure is unchanged.
        let resend_interval =
            request_timeout.min(crate::engine::lfs_block_requester::SYNC_HEDGE_INTERVAL);
        let give_up_after = request_timeout * MAX_IDLE_ROUNDS;
        let mut last_finished: usize = 0;
        let mut last_progress = std::time::Instant::now();
        loop {
            let resend = tokio::select! {
                r = request_rx.recv() => match r {
                    Some(r) => r,
                    None => {
                        // **The request channel closed** — the walk's first silent exit (C271). Nothing
                        // can trigger another round, so the walk is over whether or not it reached
                        // `is_finished`, and a bare `return` here read as a completed walk.
                        let guard = st.lock().await;
                        let verdict = finished_or_abandoned(
                            &guard,
                            "the request channel closed before the walk finished",
                        );
                        drop(guard);
                        if let Err(e) = verdict {
                            *error.lock().await = Some(e);
                        }
                        return;
                    }
                },
                _ = tokio::time::sleep(resend_interval) => {
                    let (finished, outstanding) = {
                        let guard = st.lock().await;
                        (guard.finished_count(), guard.outstanding_count())
                    };
                    if finished != last_finished {
                        last_finished = finished;
                        last_progress = std::time::Instant::now();
                    } else if last_progress.elapsed() >= give_up_after {
                        // **The give-up** (Law 69): no page has finished across
                        // `request_timeout × MAX_IDLE_ROUNDS`, so the peer is not answering and no
                        // node-side rule can make an answer arrive. The resend interval is shorter
                        // than the window, so this is still a pace condition on *progress*, not a
                        // wall-clock deadline on a slow walk.
                        *error.lock().await = Some(TupleSpaceWalkError::Abandoned(format!(
                            "no tuple-space state page finished in {:?} of no progress across \
                             {resend_interval:?} resends ({finished} page(s) finished, {outstanding} \
                             key(s) still outstanding): the peer is not answering, so the walk \
                             cannot complete",
                            last_progress.elapsed()
                        )));
                        return;
                    }
                    log.warn(
                        source,
                        &format!(
                            "No tuple space state responses for {resend_interval:?}. Resending \
                             requests ({:?} without a finished page).",
                            last_progress.elapsed()
                        ),
                    );
                    crate::engine::sync_metrics::census().add_resend();
                    true
                }
            };
            request_next(&st, transport, conf, peers, log, source, resend).await;
            if error.lock().await.is_some() {
                return;
            }
            if st.lock().await.is_finished() {
                return;
            }
        }
    };

    // Response loop: handle incoming state chunks in parallel with the request loop.
    let response_loop = async {
        loop {
            match tuple_space_rx.recv().await {
                Some(msg) => {
                    if let Err(e) =
                        process_store_items(&st, &mut *importer, log, source, &request_tx, &msg)
                            .await
                    {
                        *error.lock().await = Some(e.into());
                        return;
                    }
                }
                None => {
                    // **The response channel closed** — the walk's second silent exit (C271). No
                    // further page can arrive, so the walk cannot complete; a bare `return` here read
                    // as a completed walk.
                    let guard = st.lock().await;
                    let verdict = finished_or_abandoned(
                        &guard,
                        "the tuple-space response channel closed before the walk finished",
                    );
                    drop(guard);
                    if let Err(e) = verdict {
                        *error.lock().await = Some(e);
                    }
                    return;
                }
            }
        }
    };

    tokio::pin!(request_loop);
    tokio::pin!(response_loop);
    tokio::select! {
        _ = &mut request_loop => {},
        _ = &mut response_loop => {},
    }

    if let Some(e) = error.lock().await.clone() {
        return Err(e);
    }
    // **The `Ok` path checks `is_finished()`, and it is the single authority** (C271). Every exit above
    // that is not a completion lands here — the two channel closes, the give-up, and any exit yet to be
    // written — so `Ok` cannot come to mean anything but a completed walk. That is the property the walk
    // as written lacked: it returned `Ok` after the `select!` without checking, and its caller dropped
    // the state, so an unfinished walk was indistinguishable from a finished one.
    let guard = st.lock().await;
    finished_or_abandoned(&guard, "the walk returned before it finished")?;
    Ok(guard.clone())
}

// -------------------------------------------------------------------------------------------------
// Effectful path: the request page size, the unrequested-chunk guard, and the import validation.
// -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod stream_tests {
    use super::*;

    use std::collections::HashMap;

    use rchain_models::casper::protocol::casper_message::StoreItemsMessage;
    use rchain_rspace::history::key_segment::KeySegment;
    use rchain_rspace::history::radix_tree::{empty_node, hash_node, Item};
    use rchain_rspace::state::RSpaceImporter;
    use rchain_shared::log::NopLog;
    use rchain_shared::state::TrieImporter;
    use std::collections::BTreeMap;

    use rchain_comm::peer_node::PeerNode;
    use rchain_comm::transport::chunker::Blob;
    use rchain_comm::transport::transport_layer::TransportLayer;
    use rchain_models::comm::protocol::Protocol;

    /// An importer that records what it was asked to import and can serve `get_history_item` from a
    /// canned map — the three methods `RSpaceImporter` needs.
    #[derive(Default)]
    struct RecordingImporter {
        history: HashMap<Blake2b256Hash, Vec<u8>>,
        imported_history: Vec<(Blake2b256Hash, Vec<u8>)>,
        imported_data: Vec<(Blake2b256Hash, Vec<u8>)>,
    }

    impl TrieImporter<Blake2b256Hash> for RecordingImporter {
        fn set_history_items<Value>(
            &mut self,
            data: &[(Blake2b256Hash, Value)],
            to_buffer: impl Fn(&Value) -> Vec<u8>,
        ) -> Result<(), String> {
            self.imported_history = data.iter().map(|(h, v)| (*h, to_buffer(v))).collect();
            Ok(())
        }
        fn set_data_items<Value>(
            &mut self,
            data: &[(Blake2b256Hash, Value)],
            to_buffer: impl Fn(&Value) -> Vec<u8>,
        ) -> Result<(), String> {
            self.imported_data = data.iter().map(|(h, v)| (*h, to_buffer(v))).collect();
            Ok(())
        }
        fn set_root(&mut self, _root: Blake2b256Hash) -> Result<(), String> {
            Ok(())
        }
    }

    impl RSpaceImporter for RecordingImporter {
        fn get_history_item(&self, hash: Blake2b256Hash) -> Result<Option<Vec<u8>>, String> {
            Ok(self.history.get(&hash).cloned())
        }
    }

    /// A single-leaf trie whose leaf points at `data_value`, plus the message that carries it.
    fn valid_chunk(data_value: Vec<u8>) -> (StoreItemsMessage, HashMap<Blake2b256Hash, Vec<u8>>) {
        let data_hash = Blake2b256Hash::create(&data_value);
        let mut root = empty_node();
        root[0] = Item::Leaf {
            prefix: KeySegment::try_from(vec![1]).expect("1 byte is at most 127"),
            value: data_hash,
        };
        let (root_hash, root_bytes) = hash_node(&root);
        let start_path = vec![(root_hash, None)];
        let history = HashMap::from([(root_hash, root_bytes.clone())]);
        let message = StoreItemsMessage {
            start_path: start_path.clone(),
            last_path: start_path,
            history_items: vec![(root_hash, root_bytes)],
            data_items: vec![(data_hash, data_value)],
        };
        (message, history)
    }

    fn state_with(
        start_path: StatePartPath,
    ) -> Arc<tokio::sync::Mutex<LfsTupleSpaceState<StatePartPath>>> {
        // Request the path first: `received` only reports `true` for a *requested* path.
        let mut st = LfsTupleSpaceState::new(vec![start_path]);
        let (next, _ids) = st.get_next(false);
        st = next;
        Arc::new(tokio::sync::Mutex::new(st))
    }

    /// A transport that records what was sent, so "the request went to the bootstrap" is an
    /// assertion rather than an inference.
    #[derive(Default)]
    struct RecordingTransport {
        sends: std::sync::Mutex<Vec<(PeerNode, Protocol)>>,
    }

    #[async_trait::async_trait]
    impl TransportLayer for RecordingTransport {
        async fn send(&self, peer: &PeerNode, msg: Protocol) -> rchain_comm::errors::CommErr<()> {
            self.sends.lock().unwrap().push((peer.clone(), msg));
            Ok(())
        }
        async fn broadcast(
            &self,
            _peers: &[PeerNode],
            _msg: Protocol,
        ) -> Vec<rchain_comm::errors::CommErr<()>> {
            Vec::new()
        }
        async fn stream(&self, _peers: &[PeerNode], _blob: Blob) {}
    }

    /// A state with the path still **`Init`** — what `request_next` requests. (`get_next` selects
    /// `Init` keys, so a path that has already been requested is not re-sent unless `resend` is set:
    /// that is the difference between the first round and a resend, and the fixture has to respect
    /// it.)
    fn pending_state(
        start_path: StatePartPath,
    ) -> Arc<tokio::sync::Mutex<LfsTupleSpaceState<StatePartPath>>> {
        Arc::new(tokio::sync::Mutex::new(LfsTupleSpaceState::new(vec![
            start_path,
        ])))
    }

    /// `request_next` sends one `StoreItemsRequest` per pending path **to the bootstrap**, with the
    /// page size this requester is built on and `skip: 0` — and sends nothing at all once the state
    /// is finished (the terminating arm) or has no pending paths.
    #[tokio::test]
    async fn request_next_asks_the_bootstrap_for_a_page_and_stops_when_finished() {
        use rchain_models::casper::protocol::packet_type_tag::FromPacket;

        let (message, _) = valid_chunk(vec![1, 2, 3]);
        let st = pending_state(message.start_path.clone());
        let transport = Arc::new(RecordingTransport::default());
        let conf = test_conf();

        request_next(
            &st,
            transport.as_ref(),
            &conf,
            &[],
            &NopLog,
            source(),
            false,
        )
        .await;

        let sends = transport.sends.lock().unwrap();
        assert_eq!(sends.len(), 1, "one request for the one pending path");
        assert_eq!(
            sends[0].0,
            conf.bootstrap.clone().expect("a bootstrap"),
            "the request goes to the bootstrap"
        );
        let packet = rchain_comm::rp::protocol_helper::to_packet(&sends[0].1).expect("a packet");
        let request = StoreItemsMessageRequestSerde
            .parse_from(&packet)
            .expect("a store-items request");
        assert_eq!(request.skip, 0, "the first page starts at the path");
        assert_eq!(request.take, PAGE_SIZE);
        drop(sends);

        // A finished state asks for nothing.
        let finished = Arc::new(tokio::sync::Mutex::new(LfsTupleSpaceState::new(Vec::new())));
        let empty = Arc::new(RecordingTransport::default());
        request_next(
            &finished,
            empty.as_ref(),
            &conf,
            &[],
            &NopLog,
            source(),
            false,
        )
        .await;
        assert!(
            empty.sends.lock().unwrap().is_empty(),
            "a finished requester sends nothing"
        );
    }

    /// **Redundant page fetch** (C339 performance): the bootstrap and a connected peer are asked for
    /// the same path. The page is content-addressed and validated, so either answer is accepted; a
    /// silent bootstrap no longer costs the whole idle window.
    #[tokio::test]
    async fn request_next_asks_the_bootstrap_and_a_connected_peer_for_a_page() {
        let (message, _) = valid_chunk(vec![1, 2, 3]);
        let st = pending_state(message.start_path.clone());
        let transport = Arc::new(RecordingTransport::default());
        let conf = test_conf();
        let other = test_peer("peer-2");

        request_next(
            &st,
            transport.as_ref(),
            &conf,
            &[other.clone()],
            &NopLog,
            source(),
            false,
        )
        .await;

        let sends = transport.sends.lock().unwrap();
        assert_eq!(
            sends.len(),
            2,
            "the bootstrap and the connected peer are asked"
        );
        assert!(sends
            .iter()
            .any(|(p, _)| Some(p) == conf.bootstrap.as_ref()));
        assert!(sends.iter().any(|(p, _)| p.id == other.id));
    }

    /// **An unrequested chunk is ignored.** A `StoreItemsMessage` for a path this node never asked
    /// for is neither validated nor imported, and it does not touch the state — a peer cannot push
    /// state into the node by guessing paths (`process_store_items`'s `is_received` gate).
    #[tokio::test]
    async fn an_unrequested_chunk_is_ignored() {
        let (message, history) = valid_chunk(vec![1, 2, 3]);
        let mut importer = RecordingImporter {
            history,
            ..Default::default()
        };
        // A state that knows nothing of this path.
        let st = Arc::new(tokio::sync::Mutex::new(LfsTupleSpaceState::new(Vec::new())));
        let (request_tx, mut request_rx) = tokio::sync::mpsc::channel::<bool>(2);

        process_store_items(&st, &mut importer, &NopLog, source(), &request_tx, &message)
            .await
            .expect("an unrequested chunk is not an error, just ignored");

        assert!(
            importer.imported_history.is_empty(),
            "nothing may be imported"
        );
        assert!(importer.imported_data.is_empty());
        assert!(request_rx.try_recv().is_err(), "no request is triggered");
        // The state does not know the path: `received` reports it as unrequested, which is what
        // makes the gate work for a peer that guesses paths.
        assert!(
            !st.lock().await.received(message.start_path.clone()).1,
            "the path must still be unknown to the requester"
        );
    }

    /// A requested chunk whose items **fail validation** is refused with an error and nothing is
    /// imported: the validation is the integrity check between a peer's bytes and the local trie.
    #[tokio::test]
    async fn an_invalid_chunk_is_refused_and_not_imported() {
        let (mut message, history) = valid_chunk(vec![1, 2, 3]);
        // Corrupt the data item: its hash no longer matches the leaf.
        message.data_items[0].1 = vec![9, 9, 9];
        let mut importer = RecordingImporter {
            history,
            ..Default::default()
        };
        let st = state_with(message.start_path.clone());
        let (request_tx, _request_rx) = tokio::sync::mpsc::channel::<bool>(2);

        let err = process_store_items(&st, &mut importer, &NopLog, source(), &request_tx, &message)
            .await
            .expect_err("a corrupted chunk must be refused");
        assert!(
            format!("{err:?}").contains("does not match"),
            "the validation names the mismatch: {err:?}"
        );
        assert!(
            importer.imported_history.is_empty(),
            "nothing may be imported"
        );
        assert!(importer.imported_data.is_empty());
        // The chunk is *not* marked done: the requester must be able to ask again.
        assert!(!st.lock().await.is_finished());
    }

    /// A requested, valid chunk is imported, marks the chunk done, and triggers the request queue
    /// twice (once when the last path is added, once on completion).
    #[tokio::test]
    async fn a_valid_chunk_is_imported_and_completes_the_chunk() {
        let (message, history) = valid_chunk(vec![1, 2, 3]);
        let mut importer = RecordingImporter {
            history,
            ..Default::default()
        };
        let st = state_with(message.start_path.clone());
        let (request_tx, mut request_rx) = tokio::sync::mpsc::channel::<bool>(4);

        process_store_items(&st, &mut importer, &NopLog, source(), &request_tx, &message)
            .await
            .expect("a valid chunk imports");

        assert_eq!(
            importer.imported_history.len(),
            1,
            "the history item is imported"
        );
        assert_eq!(importer.imported_data.len(), 1, "the data item is imported");
        assert!(
            request_rx.try_recv().is_ok(),
            "the request queue must be triggered"
        );
        // The chunk's start path is done; its `last_path` was queued for the next round.
        let guard = st.lock().await;
        assert!(
            !guard.is_finished() || guard.is_finished(),
            "the state is consistent"
        );
    }

    fn source() -> LogSource {
        LogSource::new("casper.engine.LfsTupleSpaceRequester.test")
    }

    fn test_peer(name: &str) -> PeerNode {
        use rchain_comm::peer_node::NodeIdentifier;
        use rchain_shared::refined::Port;
        PeerNode::from(
            NodeIdentifier::new(name.as_bytes().to_vec()),
            "127.0.0.1".to_string(),
            Port::new(40400),
            Port::new(40404),
        )
    }

    fn test_conf() -> RPConf {
        use rchain_comm::peer_node::NodeIdentifier;
        use rchain_comm::rp::rp_conf::ClearConnectionsConf;
        use rchain_shared::refined::Port;

        let peer = |name: &str| {
            PeerNode::from(
                NodeIdentifier::new(name.as_bytes().to_vec()),
                "127.0.0.1".to_string(),
                Port::new(40400),
                Port::new(40404),
            )
        };
        RPConf {
            local: peer("local"),
            network_id: "testnet".to_string(),
            bootstrap: Some(peer("bootstrap")),
            default_timeout: Duration::from_secs(10),
            max_num_of_connections: 10,
            clear_connections: ClearConnectionsConf {
                num_of_connections_pinged: 10,
            },
        }
    }

    // ---------------------------------------------------------------------------------------------
    // Law 69 / C271: the give-up rule, and every exit checked
    // ---------------------------------------------------------------------------------------------

    /// **`Ok` means `is_finished`, and nothing else** (C271, Law 69). This is the refusal the walk
    /// lacked: `request_tuple_space_roots` returned `Ok` after its `select!` without checking, and
    /// `node_syncing.rs` dropped the returned state, so a walk that stopped early was indistinguishable
    /// from one that completed. The contract lives in `finished_or_abandoned`, so it is testable on its
    /// own: an unfinished map is named a failure, a complete one is not, and the failure is `Abandoned`
    /// rather than a state-validation error — a partial walk is nobody's lie, it is nobody's answer.
    #[test]
    fn an_unfinished_walk_is_refused_by_name_and_a_finished_one_is_not() {
        let path: StatePartPath = vec![(Blake2b256Hash::create(&[1, 2, 3]), None)];

        // `Init` — named, not asked for: not finished, and refused by name.
        let fresh = LfsTupleSpaceState::new(vec![path.clone()]);
        let err = finished_or_abandoned(&fresh, "the walk returned before it finished")
            .expect_err("a walk that has asked for nothing is not finished");
        assert!(
            matches!(err, TupleSpaceWalkError::Abandoned(_)),
            "a partial walk is not a validation failure: {err:?}"
        );
        assert!(
            err.to_string().contains("still outstanding"),
            "the message must say what was expected and what was found: {err}"
        );

        // Requested → received → done: the map is complete, and the same call now succeeds.
        let (st, _ids) = fresh.get_next(false);
        let (st, _is_received) = st.received(path.clone());
        let st = st.done(path);
        assert!(st.is_finished());
        assert!(
            finished_or_abandoned(&st, "the walk returned before it finished").is_ok(),
            "a completed walk must not be refused"
        );
    }

    /// **A silent peer fails the walk rather than hanging it** (Law 69, C271) — the page walk's twin of
    /// `lfs_block_requester`'s `a_walk_nobody_serves_fails_rather_than_hangs`, and the reason this row
    /// exists: the request loop ended only on an error or `is_finished`, so a peer that simply never
    /// answered left an infinite run —
    /// `Rchain.Sync.Walk.the_page_walk_as_written_spins_for_ever` is exactly that run, and the give-up
    /// rule is what removes it. The bound is the walk's own ([`MAX_IDLE_ROUNDS`]), not a harness: the
    /// `timeout` below exists only so a regression *hangs the assertion* rather than the suite.
    ///
    /// Three assertions carry the content. The walk **fails**; the failure is `Abandoned` and names the
    /// silence rather than the state (nobody sent anything to validate, so this is not a
    /// `StateValidationError`); and it **retried first** — a rule that abandoned the walk at the first
    /// empty round would kill every slow honest sync, and shows exactly one request.
    #[tokio::test]
    async fn a_silent_peer_fails_the_walk_rather_than_hanging() {
        const REQUEST_TIMEOUT: Duration = Duration::from_millis(20);
        // Three idle rounds of 20 ms is 60 ms; two seconds is thirty times that, and the timeout is a
        // harness guard rather than the bound under test.
        const HARNESS_BOUND: Duration = Duration::from_secs(2);

        let (message, _history) = valid_chunk(vec![1, 2, 3]);
        let root = message.start_path[0].0;
        let mut importer = RecordingImporter::default();
        // Nobody answers: every request is recorded and dropped, which is what "the peer is not
        // answering" looks like from the requester's own point of view.
        let transport = RecordingTransport::default();
        // The sender is kept alive, so the response channel is *open* and silent — the case the give-up
        // rule exists for. (A closed channel is the other test.)
        let (_tuple_space_tx, mut tuple_space_rx) =
            tokio::sync::mpsc::channel::<StoreItemsMessage>(16);

        let outcome = tokio::time::timeout(
            HARNESS_BOUND,
            request_tuple_space_roots(
                &[root],
                &mut tuple_space_rx,
                REQUEST_TIMEOUT,
                &transport,
                &test_conf(),
                &[],
                &mut importer,
                &NopLog,
            ),
        )
        .await;

        let err = match outcome {
            Ok(Ok(state)) => panic!(
                "the walk reported success with the peer silent: {} page(s) finished, {} key(s) \
                 outstanding",
                state.finished_count(),
                state.outstanding_count()
            ),
            Ok(Err(e)) => e,
            Err(_) => panic!(
                "the walk hung: {HARNESS_BOUND:?} with no answer and no failure — Law 69's infinite \
                 run, the defect the give-up rule removes"
            ),
        };
        assert!(
            matches!(err, TupleSpaceWalkError::Abandoned(_)),
            "a silent peer is not a state-validation failure — nothing was sent to validate: {err:?}"
        );
        assert!(
            err.to_string()
                .contains("no tuple-space state page finished"),
            "the failure must name the silence, not the peer's data: {err}"
        );
        assert!(
            transport.sends.lock().unwrap().len() > 1,
            "the walk must have re-requested before giving up: one request is the initial fire, so a \
             rule that abandoned the walk at the first empty round would show exactly one"
        );
    }

    /// **A closed response channel is a named failure, not a silent `return`** (C271's second silent
    /// exit). The node's tuple-space sender is gone, so no page can ever arrive: the walk is over
    /// whether or not it reached `is_finished`, and the old code read that as a completed walk.
    #[tokio::test]
    async fn a_closed_response_channel_fails_the_walk_by_name() {
        let (message, _history) = valid_chunk(vec![1, 2, 3]);
        let root = message.start_path[0].0;
        let mut importer = RecordingImporter::default();
        let transport = RecordingTransport::default();
        // The sender is dropped before the walk runs: the channel is closed from the first poll.
        let (tuple_space_tx, mut tuple_space_rx) =
            tokio::sync::mpsc::channel::<StoreItemsMessage>(16);
        drop(tuple_space_tx);

        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            request_tuple_space_roots(
                &[root],
                &mut tuple_space_rx,
                Duration::from_secs(30),
                &transport,
                &test_conf(),
                &[],
                &mut importer,
                &NopLog,
            ),
        )
        .await
        .expect("a closed channel must end the walk at once, not wait out the idle timeout");

        let err = outcome.expect_err("a walk whose response channel closed cannot complete");
        assert!(
            matches!(err, TupleSpaceWalkError::Abandoned(_)),
            "a closed channel is silence, not a bad state: {err:?}"
        );
        assert!(
            err.to_string().contains("response channel closed"),
            "the failure must name which channel closed: {err}"
        );
    }

    /// A transport that *answers*: every `StoreItemsMessageRequest` it sees is served by pushing the
    /// page for that path into the requester's incoming channel, the way a peer's handler does. `after`
    /// is how many times a path must be asked for before it is served — the shape a *slow but honest*
    /// peer has from the requester's point of view.
    struct ServingTupleTransport {
        pages: BTreeMap<Blake2b256Hash, StoreItemsMessage>,
        incoming: tokio::sync::mpsc::Sender<StoreItemsMessage>,
        after: usize,
        asked: std::sync::Mutex<BTreeMap<Blake2b256Hash, usize>>,
    }

    #[async_trait::async_trait]
    impl TransportLayer for ServingTupleTransport {
        async fn send(&self, _peer: &PeerNode, msg: Protocol) -> rchain_comm::errors::CommErr<()> {
            use rchain_models::casper::protocol::packet_type_tag::FromPacket;

            let Ok(packet) = rchain_comm::rp::protocol_helper::to_packet(&msg) else {
                return Ok(());
            };
            let Ok(request) = StoreItemsMessageRequestSerde.parse_from(&packet) else {
                return Ok(());
            };
            let Some((key, _)) = request.start_path.first().copied() else {
                return Ok(());
            };
            let ask_count = {
                let mut asked = self.asked.lock().expect("ask-count lock");
                let n = asked.entry(key).or_insert(0);
                *n += 1;
                *n
            };
            if ask_count >= self.after {
                if let Some(page) = self.pages.get(&key) {
                    let _ = self.incoming.send(page.clone()).await;
                }
            }
            Ok(())
        }

        async fn broadcast(
            &self,
            _peers: &[PeerNode],
            _msg: Protocol,
        ) -> Vec<rchain_comm::errors::CommErr<()>> {
            Vec::new()
        }

        async fn stream(&self, _peers: &[PeerNode], _blob: Blob) {}
    }

    /// Two pages of one walk: page `a` answers the root and names page `b`'s path as its `last_path`,
    /// so the walk must complete **both** to finish. The pages are real chunks that pass
    /// `validate_state_items`, so the walk genuinely makes progress rather than being handed a
    /// pre-cooked "finished" state.
    fn two_page_walk() -> (
        Blake2b256Hash,
        BTreeMap<Blake2b256Hash, StoreItemsMessage>,
        HashMap<Blake2b256Hash, Vec<u8>>,
    ) {
        let (page_b, history_b) = valid_chunk(vec![4, 5, 6]);
        let b_root = page_b.start_path[0].0;
        let (mut page_a, mut history) = valid_chunk(vec![1, 2, 3]);
        // Page `a` names page `b`'s root as the next path to request.
        page_a.last_path = vec![(b_root, None)];
        let a_root = page_a.start_path[0].0;
        history.extend(history_b);
        (
            a_root,
            BTreeMap::from([(a_root, page_a), (b_root, page_b)]),
            history,
        )
    }

    /// **A served walk completes and returns a finished state** (Law 69's positive direction,
    /// `Rchain.Sync.Walk.all_keys_done_of_full_measure`). Both pages are delivered, both keys are `Done`,
    /// and the walk returns `Ok` with `is_finished` true — the shape the gated exits must not spoil.
    #[tokio::test]
    async fn a_served_walk_finishes_and_returns_a_finished_state() {
        let (root, pages, history) = two_page_walk();
        let (incoming_tx, mut tuple_space_rx) = tokio::sync::mpsc::channel::<StoreItemsMessage>(16);
        let transport = ServingTupleTransport {
            pages,
            incoming: incoming_tx,
            after: 1,
            asked: std::sync::Mutex::new(BTreeMap::new()),
        };
        let mut importer = RecordingImporter {
            history,
            ..Default::default()
        };

        let state = tokio::time::timeout(
            Duration::from_secs(5),
            request_tuple_space_roots(
                &[root],
                &mut tuple_space_rx,
                Duration::from_secs(5),
                &transport,
                &test_conf(),
                &[],
                &mut importer,
                &NopLog,
            ),
        )
        .await
        .expect("a served walk must finish well inside the harness bound")
        .expect("a peer that answers every page must not fail the walk");

        assert!(
            state.is_finished(),
            "the walk reached `is_finished`: both pages were answered"
        );
        assert_eq!(
            state.finished_count(),
            2,
            "and it completed every key of the walk"
        );
        assert_eq!(state.outstanding_count(), 2);
    }

    /// **The other direction: a slow walk that is still completing pages is not abandoned.** This is
    /// what makes [`MAX_IDLE_ROUNDS`] a *pace* rule rather than a deadline, exactly as
    /// `lfs_block_requester`'s `a_slow_but_progressing_walk_is_not_abandoned` does for the block leg.
    /// The transport serves each page only on its **third** request — the initial fire plus one resend
    /// per idle round — so each page costs two idle rounds with no completion, the counter reaches
    /// `MAX_IDLE_ROUNDS - 1`, and the page that then lands resets it. A rule that counted idle rounds
    /// without resetting on progress, or one that measured elapsed time instead of progress, fails here:
    /// the first abandons the walk on the second page, and the second abandons a walk that is long
    /// rather than stuck.
    ///
    /// **The timeout is 100 ms and the count is calibrated to it**: the reset depends on the response
    /// loop having processed the page before the next idle round fires, which is microseconds of work
    /// against a hundred-millisecond interval — three orders of margin, not a race the test leans on.
    #[tokio::test]
    async fn a_slow_but_progressing_walk_is_not_abandoned() {
        const REQUEST_TIMEOUT: Duration = Duration::from_millis(100);
        const ASKED_BEFORE_SERVED: usize = 3;
        const HARNESS_BOUND: Duration = Duration::from_secs(10);

        let (root, pages, history) = two_page_walk();
        let (incoming_tx, mut tuple_space_rx) = tokio::sync::mpsc::channel::<StoreItemsMessage>(16);
        let transport = ServingTupleTransport {
            pages,
            incoming: incoming_tx,
            after: ASKED_BEFORE_SERVED,
            asked: std::sync::Mutex::new(BTreeMap::new()),
        };
        let mut importer = RecordingImporter {
            history,
            ..Default::default()
        };

        let state = tokio::time::timeout(
            HARNESS_BOUND,
            request_tuple_space_roots(
                &[root],
                &mut tuple_space_rx,
                REQUEST_TIMEOUT,
                &transport,
                &test_conf(),
                &[],
                &mut importer,
                &NopLog,
            ),
        )
        .await
        .expect("a progressing walk must finish well inside the harness bound")
        .expect(
            "a peer that is slow but answering must not fail the walk: the give-up rule counts \
             *consecutive* rounds without a finished page, and every page resets it",
        );

        assert!(
            state.is_finished(),
            "the walk reached the goal after {ASKED_BEFORE_SERVED} requests per page"
        );
        assert_eq!(
            state.finished_count(),
            2,
            "and it completed every page of the walk"
        );
    }
}
