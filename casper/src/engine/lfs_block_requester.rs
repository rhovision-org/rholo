//! Last Finalized State block requester (port of `engine/LfsBlockRequester.scala`).
//!
//! Downloads the blocks needed to reconstruct the last finalized state, following justifications
//! from the finalized fringe. The pure requester state is [`super::LfsState`]; the `request_blocks`
//! stream orchestration (request loop + response loop with an idle-resend timeout) is ported here
//! onto tokio channels, mirroring the fs2 `requestStream concurrently responseStream` structure.
//!
//! **One deliberate departure from the oracle, and it is [`MAX_IDLE_ROUNDS`]**: the walk gives up
//! when it has completed no block across several consecutive idle rounds. The Scala's only
//! termination condition is `isFinished` (`LfsBlockRequester.scala:309-312`, the `terminateAfter`
//! after `evalOnIdle`), so a fringe naming a block no peer serves retries forever — and the port did
//! the same. Issue #102 records what that costs: the `join!` in `run_approved_state_sync` waits for
//! both legs, so the sync attempt never finishes, `notify_when_restored` never fires, and the node
//! sits in `NodeSyncing` for good **with a serving API and no error line**.

use rchain_shared::chan;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use rchain_block_storage::block_store::BlockStore;
use rchain_comm::peer_node::{NodeIdentifier, PeerNode};
use rchain_models::block_hash::BlockHash;
use rchain_models::casper::protocol::casper_message::{BlockMessage, FinalizedFringe};
use rchain_shared::log::{Log, LogSource};

use super::{LfsState, ReceiveInfo};
use crate::protocol::comm_util::CommUtil;
use crate::validate;

/// The block-requester state keyed by block hash (port of `ST[BlockHash]`).
type St = LfsState<BlockHash>;

/// How many **consecutive** idle resend intervals may complete no block before the walk gives up and
/// the sync attempt fails.
///
/// **The quantity is `finished.len()` — blocks the walk has completed — and it is monotone**
/// ([`LfsState::done`] only ever adds to it, and [`LfsState::add`] refuses a key already there), which
/// is what makes "did it move" a well-formed question. So the rule is a **pace** condition, not a
/// deadline: it is **Law 51a**'s `Drift` shape — steps continue, the measure never moves, and nothing
/// bounds the steps between two increases, which is what `Paced` refuses. **A wall-clock deadline
/// would be the wrong instrument**: a long chain legitimately takes longer than any fixed number, and
/// it is silence the walk must not tolerate, not duration.
///
/// Three rounds is `3 × request_timeout` — the production 30 s makes it 90 s of complete silence,
/// which no honest peer produces and which a partial partition reaches in three lost responses. A
/// *slow* walk is not touched: the counter resets on every block, so only consecutive empty rounds
/// count, and `a_slow_but_progressing_walk_is_not_abandoned` pins that.
///
/// **Provisional, and deliberately a constant.** It is node-local policy — it changes no block and no
/// hash — so a `PosParams` field would be wrong, but what it *should* be is a measurement; this is the
/// same treatment `block-storage/src/dag/liveness.rs`'s `LIVENESS_WINDOW` records for its own
/// provisional number.
pub const MAX_IDLE_ROUNDS: u32 = 3;

/// **The per-hash request log** (#457's attribution, after review). A connected peer is not
/// necessarily asked — `request_next` broadcasts to a random subset (the walk uses `Some(1)`) — and a
/// `BlockMessage` is not necessarily an answer (it may be gossip for another hash). So the give-up
/// diagnostic reports what was *observed*: for each outstanding hash, the peers it was actually
/// requested from that never delivered **that hash**.
#[derive(Default)]
pub struct BlockRequestLog {
    /// The peers each hash was requested from, keyed by peer id and kept for display.
    sent: BTreeMap<BlockHash, BTreeMap<NodeIdentifier, PeerNode>>,
    /// The peers that delivered each hash.
    answered: BTreeMap<BlockHash, BTreeSet<NodeIdentifier>>,
}

impl BlockRequestLog {
    pub(crate) fn record_sent(&mut self, hash: &BlockHash, peers: &[PeerNode]) {
        self.sent
            .entry(*hash)
            .or_default()
            .extend(peers.iter().map(|p| (p.id.clone(), p.clone())));
    }

    pub(crate) fn record_answer(&mut self, hash: &BlockHash, peer: &NodeIdentifier) {
        self.answered.entry(*hash).or_default().insert(peer.clone());
    }

    /// The peers `hash` was requested from that never delivered it, named. An answer for a
    /// **different** hash does not clear this one, and a connected-but-unselected peer never appears.
    pub(crate) fn unanswered(&self, hash: &BlockHash) -> Vec<String> {
        self.sent
            .get(hash)
            .into_iter()
            .flat_map(|peers| peers.iter())
            .filter(|(id, _)| {
                !self
                    .answered
                    .get(hash)
                    .is_some_and(|answered| answered.contains(*id))
            })
            .map(|(_, peer)| peer.to_string())
            .collect()
    }
}

/// Validate a received block and, if accepted, request its justifications. Returns whether the
/// block was requested and its hash valid (port of `validateReceivedBlock`).
///
/// **The Scala's `lowerBound` acceptance cutoff is inert, so this is not a divergence** (AUDIT C64).
/// The port used to say it omitted the cutoff; in fact the oracle's production path never seeds it
/// either — `LfsBlockRequester.scala:125` defaults it to `0` and the only construction in the tree
/// (`:318`) passes `latest` and `extraHeights` but not `lowerBound`, so
/// `blockNumber >= minimumHeight` (`:228`) is `>= 0` for every block. `extraHeights` is not a bound
/// on the walk: it *reduces* the bound further (`max(0, min(height - 1, lowerBound) - extraHeights)`).
/// Both trees therefore walk the full ancestry to genesis.
/// `a_walk_longer_than_deploy_lifespan_reaches_genesis` pins it.
async fn validate_received_block(
    st: &Arc<tokio::sync::Mutex<St>>,
    block: &BlockMessage,
    log: &dyn Log,
    source: LogSource,
) -> bool {
    let block_number = i64::from(block.block_number);

    // **#457: check what the walk owes, then check the body, and only then consume the key.**
    //
    // The order used to be `received` → `validate::block_hash`: a body that failed the check had
    // already moved the key to `Received`, and `get_next` never selected `Received` again, so the key
    // was stranded and `is_finished` (which requires `d` empty) could never hold. Nothing about a
    // block is known until its content hash is re-derived, so nothing is written until then.
    let known_status = {
        let guard = st.lock().await;
        guard.d.get(&block.block_hash).copied()
    };
    let Some(status) = known_status else {
        log.warn(
            source,
            &format!(
                "Received block #{} with hash {} that the walk does not need; dropped.",
                block.block_number,
                block.block_hash.to_hex()
            ),
        );
        return false;
    };
    if !validate::block_hash(block) {
        log.warn(
            source,
            &format!(
                "Received block #{} with a hash that does not match its content: claimed {}, \
                 recomputed {}. Key left {:?}; it will be re-requested rather than consumed.",
                block.block_number,
                block.block_hash.to_hex(),
                crate::proto_util::hash_block(block).to_hex(),
                status
            ),
        );
        return false;
    }

    let info = {
        let mut guard = st.lock().await;
        let (new_state, info) = guard.received(&block.block_hash, block_number);
        *guard = new_state;
        info
    };
    let ReceiveInfo { last_latest, .. } = info;
    if last_latest {
        log.info(source, "Latest blocks downloaded.");
    }
    // Always request the block's justifications: `dag.insert` requires every justification to be
    // present in the message map, so the requester must reconstruct the full ancestry chain down
    // to the genesis block (a syncing node's DAG is always empty).
    let justifications: BTreeSet<BlockHash> = block.justifications.iter().copied().collect();
    let mut guard = st.lock().await;
    *guard = guard.add(&justifications);
    true
}

/// Save a received block to the store and mark it done (port of `saveBlock`).
///
/// **The write is the point of this function, so its failure is not discarded** (AUDIT C65): the
/// oracle's `saveBlock` is `containsBlock` then `putBlockToStore` in `F`, and a failed write fails
/// the sync attempt rather than being swallowed. Discarding it and marking the block done anyway
/// would leave the requester believing it holds a block it never persisted, and — because `done`
/// removes the key from the request map — it would never be requested again: a permanently missing
/// block in a "synced" state.
async fn save_block(
    st: &Arc<tokio::sync::Mutex<St>>,
    block_store: &BlockStore,
    block: &BlockMessage,
) -> Result<(), String> {
    let already_saved = block_store
        .contains(&[block.block_hash])
        .await?
        .first()
        .copied()
        .unwrap_or(false);
    if !already_saved {
        block_store
            .put(&[(block.block_hash, block.clone())])
            .await?;
    }
    let mut guard = st.lock().await;
    *guard = guard.done(&block.block_hash);
    Ok(())
}

/// Process an incoming block: validate it, save it, and trigger the next request (port of
/// `processBlock`).
async fn process_block(
    st: &Arc<tokio::sync::Mutex<St>>,
    block_store: &BlockStore,
    log: &dyn Log,
    source: LogSource,
    request_tx: &tokio::sync::mpsc::Sender<bool>,
    block: &BlockMessage,
) -> Result<(), String> {
    let is_valid = validate_received_block(st, block, log, source).await;
    if is_valid {
        save_block(st, block_store, block).await?;
        // Only a completed block can expose new ancestry. A delayed duplicate or
        // an unsolicited/invalid body must not resend every outstanding hash:
        // those replies create more duplicates and reset the idle failure timer.
        // Owed keys with invalid bodies remain outstanding for the bounded idle
        // retry; refusing a body never advances or completes the walk.
        chan::send(request_tx, false).await;
    }
    Ok(())
}

/// Take the next set of hashes to request, enqueue existing ones for processing, and broadcast
/// requests for the missing ones (port of `requestNext`).
async fn request_next(
    st: &Arc<tokio::sync::Mutex<St>>,
    response_hash_tx: &tokio::sync::mpsc::UnboundedSender<BlockHash>,
    block_store: &BlockStore,
    comm_util: &CommUtil,
    resend: bool,
    block_requests: &Arc<tokio::sync::Mutex<BlockRequestLog>>,
) -> Result<(), String> {
    let is_end = { st.lock().await.is_finished() };
    let hashes = {
        let mut guard = st.lock().await;
        let (new_state, hashes) = guard.get_next(resend);
        *guard = new_state;
        hashes
    };

    let hashes_vec: Vec<BlockHash> = hashes.iter().copied().collect();
    // A failed read must not read as "nothing to request" (AUDIT C65): `unwrap_or_default()` yields
    // an empty `Vec<bool>`, the `zip` below yields nothing, and every hash is silently neither
    // reported as existing nor requested — the walk then does nothing until the idle resend nudges
    // it. The oracle's `filterA(containsBlock)` propagates instead.
    let contains = block_store.contains(&hashes_vec).await?;
    let mut existing = Vec::new();
    let mut missing = Vec::new();
    for (h, c) in hashes_vec.into_iter().zip(contains) {
        if c {
            existing.push(h);
        } else {
            missing.push(h);
        }
    }

    for h in &existing {
        chan::unbounded_send(response_hash_tx, *h);
    }
    if !is_end && !missing.is_empty() {
        for h in &missing {
            let peers = comm_util.request_lfs_block(h).await;
            block_requests.lock().await.record_sent(h, &peers);
        }
    }
    Ok(())
}

/// Request all blocks needed for the last finalized state (port of `LfsBlockRequester.stream`).
///
/// Returns the final requester state (the Scala stream's `.last` element) once all blocks are
/// received.
pub async fn request_blocks(
    fringe: &FinalizedFringe,
    incoming_blocks: &mut tokio::sync::mpsc::Receiver<BlockMessage>,
    request_timeout: Duration,
    block_store: &BlockStore,
    comm_util: &CommUtil,
    log: &dyn Log,
    block_requests: &Arc<tokio::sync::Mutex<BlockRequestLog>>,
) -> Result<St, String> {
    let source = LogSource::new("casper.engine.LfsBlockRequester");

    // Finalized block hashes from which LFS sync starts.
    let finalized_hashes: BTreeSet<BlockHash> = fringe.hashes.iter().copied().collect();
    let st = Arc::new(tokio::sync::Mutex::new(LfsState::new(
        finalized_hashes.clone(),
        finalized_hashes,
    )));

    // `true` triggers a resend of already-requested blocks.
    let (request_tx, mut request_rx) = tokio::sync::mpsc::channel::<bool>(2);
    let (response_hash_tx, mut response_hash_rx) =
        tokio::sync::mpsc::unbounded_channel::<BlockHash>();

    // "Light the fire!" / start the first request for blocks.
    chan::send(&request_tx, false).await;

    // Request loop: pull request triggers (or resend on idle timeout) and request next blocks,
    // terminating once all blocks are finished.
    let request_loop = async {
        // The give-up rule's state: how many blocks the walk had completed at the last idle round that
        // saw progress, and how many consecutive idle rounds have seen none since.
        let mut last_finished: usize = 0;
        let mut idle_rounds: u32 = 0;
        loop {
            let resend = tokio::select! {
                r = request_rx.recv() => match r {
                    Some(r) => r,
                    None => return Ok::<(), String>(()),
                },
                _ = tokio::time::sleep(request_timeout) => {
                    // An idle round: a whole resend interval with no block response. Whether it is
                    // *stuck* is a different question from whether it is *quiet*, and `finished` is
                    // the measure that answers it.
                    let (finished, (init, requested, received), sample) = {
                        let guard = st.lock().await;
                        let sample: Vec<String> = guard
                            .d
                            .iter()
                            .take(8)
                            .map(|(k, s)| format!("{}={s:?}", k.to_hex()))
                            .collect();
                        (guard.finished.len(), guard.outstanding_breakdown(), sample)
                    };
                    if finished == last_finished {
                        idle_rounds += 1;
                        if idle_rounds >= MAX_IDLE_ROUNDS {
                            let (outstanding_hashes, connected) = {
                                let guard = st.lock().await;
                                (
                                    guard.d.keys().copied().take(8).collect::<Vec<_>>(),
                                    guard.d.len(),
                                )
                            };
                            let log = block_requests.lock().await;
                            let attributed: Vec<String> = outstanding_hashes
                                .iter()
                                .filter_map(|hash| {
                                    let silent = log.unanswered(hash);
                                    (!silent.is_empty()).then(|| {
                                        format!("{}: [{}]", hash.to_hex(), silent.join(", "))
                                    })
                                })
                                .collect();
                            let attribution = if attributed.is_empty() {
                                "no recorded request to a peer is unanswered".to_string()
                            } else {
                                attributed.join("; ")
                            };
                            let peers = comm_util.connected_peers().await.len();
                            return Err(format!(
                                "no block finished in {idle_rounds} consecutive idle rounds of \
                                 {request_timeout:?} ({finished} block(s) finished; outstanding: \
                                 {init} init, {requested} requested, {received} received; \
                                 first outstanding: [{}]; {connected} outstanding hash(es), \
                                 {peers} connected peer(s)). For the outstanding hashes, no block \
                                 was received from the peers they were requested from: [{attribution}]. \
                                 The fringe names state no peer is serving, every answer is being \
                                 refused, or the peers cannot answer this node: a joiner whose \
                                 protocol and discovery ports admit only its bootstrap gets \
                                 exactly this — they gossip to it and cannot dial in \
                                 (#457/#458)",
                                sample.join(", ")
                            ));
                        }
                    } else {
                        idle_rounds = 0;
                        last_finished = finished;
                    }
                    log.warn(
                        source,
                        &format!(
                            "No block responses for {request_timeout:?}. Resending requests \
                             ({idle_rounds}/{MAX_IDLE_ROUNDS} idle rounds without a finished block)."
                        ),
                    );
                    true
                }
            };
            request_next(
                &st,
                &response_hash_tx,
                block_store,
                comm_util,
                resend,
                block_requests,
            )
            .await?;
            if st.lock().await.is_finished() {
                return Ok(());
            }
        }
    };

    // Response loop: handle incoming blocks and existing-block hashes in parallel with the request
    // loop.
    let response_loop = async {
        loop {
            tokio::select! {
                block = incoming_blocks.recv() => {
                    match block {
                        Some(block) => {
                            process_block(&st, block_store, log, source, &request_tx, &block)
                                .await?;
                        }
                        None => return Ok::<(), String>(()),
                    }
                }
                hash = response_hash_rx.recv() => {
                    match hash {
                        Some(hash) => {
                            let block = match block_store.get(&[hash]).await {
                                Ok(v) => v.into_iter().flatten().next(),
                                Err(e) => {
                                    // A read that fails is not "there is no such block" (C249's class):
                                    // the loop below would move on believing the store had answered.
                                    log.error(
                                        source,
                                        &format!("Failed to read block {}: {e}", hash.to_hex()),
                                    );
                                    None
                                }
                            };
                            if let Some(block) = block {
                                log.info(
                                    source,
                                    &format!("Process existing block #{}", block.block_number),
                                );
                                process_block(&st, block_store, log, source, &request_tx, &block)
                                    .await?;
                            }
                        }
                        None => return Ok(()),
                    }
                }
            }
        }
    };

    tokio::pin!(request_loop);
    tokio::pin!(response_loop);
    // Either loop ending ends the walk; an error from either fails the sync attempt, which is what
    // the oracle's `compile.drain` does with a store failure (AUDIT C65).
    tokio::select! {
        outcome = &mut request_loop => outcome?,
        outcome = &mut response_loop => outcome?,
    }

    let guard = st.lock().await;
    Ok(guard.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use rchain_block_storage::block_store::BlockStore;
    use rchain_block_storage::dag::codecs::{BlockHashCodec, BlockMessageCodec};
    use rchain_models::block::state_hash::StateHash;
    use rchain_models::block_hash::BlockHash;
    use rchain_models::casper::protocol::casper_message::RholangState;
    use rchain_models::validator::Validator;
    use rchain_shared::log::{LogSource, NopLog};
    use rchain_shared::store::InMemoryKeyValueStore;
    use rchain_shared::typed_store::{KeyValueTypedStore, KeyValueTypedStoreCodec};

    use crate::proto_util::hash_block;
    use async_trait::async_trait;
    use rchain_comm::errors::CommErr;
    use rchain_comm::peer_node::{NodeIdentifier, PeerNode};
    use rchain_comm::rp::rp_conf::{ClearConnectionsConf, RPConf};
    use rchain_comm::transport::chunker::Blob;
    use rchain_comm::transport::transport_layer::TransportLayer;
    use rchain_models::casper::protocol::packet_type_tag::FromPacket;
    use rchain_models::comm::protocol::Protocol;
    use rchain_shared::refined::{BlockHeight, SeqNum};
    use std::time::{Duration, Instant};

    use crate::engine::ReqStatus;
    use crate::protocol::casper_message_protocol::BlockRequestSerde;
    use crate::protocol::comm_util::{CommUtil, ConnectionsCell};

    fn no_requests() -> Arc<tokio::sync::Mutex<BlockRequestLog>> {
        Arc::new(tokio::sync::Mutex::new(BlockRequestLog::default()))
    }

    /// #457's attribution, after review: the error names the peers a **hash** was requested from
    /// that never delivered it. An unselected connected peer is never blamed, and an answer for a
    /// different hash (gossip) does not clear the request.
    #[test]
    fn the_request_log_names_only_requested_peers_that_did_not_deliver_the_hash() {
        let mut log = BlockRequestLog::default();
        let a = peer("validator-1");
        let b = peer("validator-2");
        let unselected = peer("validator-3");
        let requested = BlockHash::new([1u8; 32]);
        let gossip = BlockHash::new([2u8; 32]);

        // The walk requests `requested` from A and B (it broadcasts to a subset, not the connection set).
        log.record_sent(&requested, &[a.clone(), b.clone()]);
        log.record_answer(&requested, &a.id);
        // A block for another hash is not an answer to this request.
        log.record_answer(&gossip, &b.id);

        let silent = log.unanswered(&requested);
        assert_eq!(silent, vec![b.to_string()], "only B is named");
        assert!(
            !silent.iter().any(|s| s == &a.to_string()),
            "A delivered the hash"
        );
        assert!(
            !silent.iter().any(|s| s == &unselected.to_string()),
            "a connected peer that was never asked must not be blamed: {silent:?}"
        );

        log.record_answer(&requested, &b.id);
        assert!(
            log.unanswered(&requested).is_empty(),
            "a delivery from B clears the hash"
        );
    }

    fn source() -> LogSource {
        LogSource::new("casper.engine.LfsBlockRequester.test")
    }

    /// A block with the given justifications and a placeholder hash.
    fn block(justifications: &[BlockHash]) -> BlockMessage {
        BlockMessage {
            version: 1,
            shard_id: "root".to_string(),
            block_hash: BlockHash::new([0u8; 32]),
            block_number: 0.try_into().unwrap(),
            sender: Validator::new([1u8; 65]),
            seq_num: 0.try_into().unwrap(),
            pre_state_hash: StateHash::new([0u8; 32]),
            post_state_hash: StateHash::new([0u8; 32]),
            justifications: justifications.to_vec(),
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

    /// A block whose `block_hash` really is its content hash, so `validate::block_hash` accepts it.
    fn valid_block(justifications: &[BlockHash]) -> BlockMessage {
        let mut b = block(justifications);
        b.block_hash = hash_block(&b);
        b
    }

    /// A store holding one in-memory typed map.
    async fn store() -> BlockStore {
        Arc::new(KeyValueTypedStoreCodec::new(
            Arc::new(tokio::sync::Mutex::new(Box::new(
                InMemoryKeyValueStore::default(),
            ))),
            Arc::new(BlockHashCodec),
            Arc::new(BlockMessageCodec),
        ))
    }

    /// The state a node is in once it has *asked* for `hash`. The request status no longer gates
    /// acceptance (#457 — `known` does), but a fixture that skipped the request would exercise the
    /// early-answer path rather than the requested one.
    fn requested(hash: BlockHash) -> St {
        let seeded = St::new([hash].into_iter().collect(), BTreeSet::new());
        seeded.get_next(false).0
    }

    /// A block nobody asked for is dropped without even checking its hash: the requester only
    /// accepts what it requested, so an unsolicited block cannot grow the download set (or the
    /// store) from a peer's say-so.
    #[tokio::test]
    async fn an_unrequested_block_is_rejected() {
        let st = Arc::new(tokio::sync::Mutex::new(St::new(
            BTreeSet::new(),
            BTreeSet::new(),
        )));
        // Perfectly valid content — rejected purely for not having been requested.
        assert!(!validate_received_block(&st, &valid_block(&[]), &NopLog, source()).await);
    }

    /// A requested block whose hash does not match its content is rejected: the requester re-derives
    /// the hash rather than trusting the message, so a peer cannot substitute a different block for
    /// the one requested. Its justifications are not added either.
    ///
    /// **The key is not consumed** (#457). `validate_received_block` used to call `st.received`
    /// *before* checking the hash, so a forged body moved the key to `Received`; `get_next(resend)`
    /// only re-requested `Init`/`Requested`, so nothing ever asked for it again and `is_finished`
    /// (which requires `d` empty) could never hold. One forged answer stalled the whole walk. The
    /// state is now untouched on rejection and the resend path re-asks the key.
    #[tokio::test]
    async fn a_forged_body_does_not_consume_the_key_and_is_re_requested() {
        let forged = block(&[]);
        let st = Arc::new(tokio::sync::Mutex::new(requested(forged.block_hash)));

        assert!(
            !validate_received_block(&st, &forged, &NopLog, source()).await,
            "a hash that does not match the content must not be accepted"
        );
        let guard = st.lock().await;
        assert_eq!(
            guard.d.get(&forged.block_hash),
            Some(&ReqStatus::Requested),
            "the key is left as it was, not consumed to `Received`"
        );
        assert!(
            guard.get_next(true).1.contains(&forged.block_hash),
            "…and the resend path asks for it again"
        );
        assert!(
            !guard.is_finished(),
            "…the key is still outstanding, as it should be"
        );
    }

    /// #457 at the requester's edge: an answer for a key the walk still owes is accepted even when
    /// its request turn has not fired. The content hash is the verification; the request slot is
    /// scheduling.
    #[tokio::test]
    async fn a_known_needed_block_is_accepted_before_its_request_is_sent() {
        let answer = valid_block(&[]);
        let st = Arc::new(tokio::sync::Mutex::new(St::new(
            [answer.block_hash].into_iter().collect(),
            BTreeSet::new(),
        )));

        assert!(
            validate_received_block(&st, &answer, &NopLog, source()).await,
            "the walk owed this key, so the verified answer is the walk's"
        );
        let guard = st.lock().await;
        assert_eq!(guard.d.get(&answer.block_hash), Some(&ReqStatus::Received));
        assert!(!guard.is_finished());
    }

    /// An accepted block contributes its **justifications** to the download set — that is how the
    /// ancestry chain is walked — and the acceptance flag is reported back to the caller.
    #[tokio::test]
    async fn an_accepted_block_adds_its_justifications() {
        let justification = valid_block(&[]);
        let block = valid_block(&[justification.block_hash]);
        let st = Arc::new(tokio::sync::Mutex::new(requested(block.block_hash)));

        assert!(
            validate_received_block(&st, &block, &NopLog, source()).await,
            "a requested block with a matching hash is accepted"
        );
        let next = { st.lock().await.get_next(false) }.1;
        assert!(
            next.contains(&justification.block_hash),
            "the block's justifications become the next requests: {next:?}"
        );
    }

    /// Saving is idempotent: a block that arrives twice is stored once and finishes once. `done`
    /// only moves a `Received` key, so the two saves must leave the state finished rather than
    /// leaving the key behind — a requester that never reports finished stalls the sync.
    #[tokio::test]
    async fn saving_is_idempotent() {
        let block = valid_block(&[]);
        let block_store = store().await;
        let st = Arc::new(tokio::sync::Mutex::new(requested(block.block_hash)));

        // Receive it first (`done` requires the `Received` status), then save twice.
        assert!(validate_received_block(&st, &block, &NopLog, source()).await);
        save_block(&st, &block_store, &block).await.unwrap();
        save_block(&st, &block_store, &block).await.unwrap();

        assert_eq!(
            block_store
                .contains(&[block.block_hash])
                .await
                .expect("contains"),
            vec![true]
        );
        assert!(
            st.lock().await.is_finished(),
            "the only requested block was saved, so the requester is finished"
        );
    }

    /// A delayed duplicate is not a failed answer to every other outstanding request.
    /// On the live chain, each duplicate used to trigger a full resend and generate
    /// further duplicates, starving the ancestry walk behind its own reply traffic.
    #[tokio::test]
    async fn duplicate_delivery_does_not_resend_unrelated_outstanding_blocks() {
        let blocks = chain(2);
        let tip = &blocks[1];
        let store = store().await;
        let st = Arc::new(tokio::sync::Mutex::new(requested(tip.block_hash)));
        let (request_tx, mut request_rx) = tokio::sync::mpsc::channel(2);
        process_block(&st, &store, &NopLog, source(), &request_tx, tip)
            .await
            .unwrap();
        assert_eq!(request_rx.try_recv(), Ok(false));
        let before = {
            let mut guard = st.lock().await;
            *guard = guard.get_next(false).0;
            guard.clone()
        };
        assert_eq!(before.d[&blocks[0].block_hash], ReqStatus::Requested);

        process_block(&st, &store, &NopLog, source(), &request_tx, tip)
            .await
            .unwrap();
        assert_eq!(
            *st.lock().await,
            before,
            "a duplicate changes no walk state"
        );
        assert!(
            request_rx.try_recv().is_err(),
            "a duplicate must not trigger requests for the still-pending parent"
        );
    }

    /// Continuous unsolicited traffic is not progress. It must neither create
    /// requests nor postpone the bounded failure of a genuinely unserved fringe.
    #[tokio::test]
    async fn unsolicited_blocks_do_not_reset_the_idle_failure_bound() {
        let blocks = chain(2);
        let tip = blocks[1].block_hash;
        let store = store().await;
        let (incoming_tx, mut incoming_rx) = tokio::sync::mpsc::channel(64);
        let noise = blocks[0].clone(); // not known until the unserved tip arrives
        let spam = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(2));
            loop {
                interval.tick().await;
                if incoming_tx.send(noise.clone()).await.is_err() {
                    break;
                }
            }
        });
        let transport = Arc::new(RecordingTransport::default());
        let conf = RPConf {
            local: peer("local"),
            network_id: "testnet".to_string(),
            bootstrap: None,
            default_timeout: Duration::from_secs(10),
            max_num_of_connections: 10,
            clear_connections: ClearConnectionsConf {
                num_of_connections_pinged: 10,
            },
        };
        let connections = Arc::new(tokio::sync::RwLock::new(vec![peer("bootstrap")]));
        let comm_util = CommUtil::new(transport.clone(), conf, connections, Arc::new(NopLog));
        let fringe = FinalizedFringe {
            hashes: vec![tip],
            state_hash: StateHash::new([0; 32]),
            ancestry: Vec::new(),
        };
        let outcome = tokio::time::timeout(
            Duration::from_millis(500),
            request_blocks(
                &fringe,
                &mut incoming_rx,
                Duration::from_millis(20),
                &store,
                &comm_util,
                &NopLog,
                &no_requests(),
            ),
        )
        .await;
        spam.abort();
        let error = outcome
            .expect("unsolicited traffic must not keep an incomplete walk alive")
            .expect_err("no peer supplied the fringe block");
        assert!(error.contains("no block finished"), "{error}");
        assert_eq!(
            transport.broadcasts.lock().unwrap().len(),
            MAX_IDLE_ROUNDS as usize,
            "only the initial request and bounded idle retries are sent"
        );
    }
    fn peer(name: &str) -> PeerNode {
        PeerNode::from(
            NodeIdentifier::new(name.as_bytes().to_vec()),
            "host".to_string(),
            rchain_shared::refined::Port::new(40400),
            rchain_shared::refined::Port::new(40404),
        )
    }

    /// A chain of `n` blocks — block `i` justifies block `i - 1` — each with a real
    /// content-addressed hash, so the requester's `validate::block_hash` accepts it.
    fn chain(n: usize) -> Vec<BlockMessage> {
        let mut blocks: Vec<BlockMessage> = Vec::with_capacity(n);
        for i in 0..n {
            let justifications: Vec<BlockHash> =
                blocks.last().map(|b| b.block_hash).into_iter().collect();
            let mut b = valid_block(&justifications);
            b.block_number = BlockHeight::try_from(i as i64).expect("a small chain");
            b.seq_num = SeqNum::try_from(i as i64).expect("a small chain");
            b.block_hash = hash_block(&b);
            blocks.push(b);
        }
        blocks
    }

    /// A transport that records every broadcast, so a test can assert that a walk asked for nothing.
    #[derive(Default)]
    struct RecordingTransport {
        broadcasts: std::sync::Mutex<Vec<usize>>,
    }

    #[async_trait]
    impl TransportLayer for RecordingTransport {
        async fn send(&self, _peer: &PeerNode, _msg: Protocol) -> CommErr<()> {
            Ok(())
        }

        async fn broadcast(&self, peers: &[PeerNode], _msg: Protocol) -> Vec<CommErr<()>> {
            self.broadcasts
                .lock()
                .expect("broadcast log lock")
                .push(peers.len());
            vec![Ok(()); peers.len()]
        }

        async fn stream(&self, _peers: &[PeerNode], _blob: Blob) {}
    }

    /// A transport that *answers*: every `BlockRequest` it sees is served by pushing the requested
    /// block into the requester's incoming channel, the way a peer's `handle_block_request` does.
    struct ServingTransport {
        blocks: BTreeMap<BlockHash, BlockMessage>,
        incoming: tokio::sync::mpsc::Sender<BlockMessage>,
    }

    #[async_trait]
    impl TransportLayer for ServingTransport {
        async fn send(&self, _peer: &PeerNode, _msg: Protocol) -> CommErr<()> {
            Ok(())
        }

        async fn broadcast(&self, _peers: &[PeerNode], msg: Protocol) -> Vec<CommErr<()>> {
            let Some(rchain_models::comm::protocol::protocol::Message::Packet(packet)) =
                msg.message
            else {
                return Vec::new();
            };
            if packet.type_id != "BlockRequest" {
                return Vec::new();
            }
            let Ok(request) = BlockRequestSerde.parse(&packet.content) else {
                return Vec::new();
            };
            let hash = request.hash;
            if let Some(block) = self.blocks.get(&hash) {
                chan::send(&self.incoming, block.clone()).await;
            }
            Vec::new()
        }

        async fn stream(&self, _peers: &[PeerNode], _blob: Blob) {}
    }

    /// The admitted bootstrap can answer; discovered peers accept requests but
    /// cannot deliver replies to the joining node's advertised protocol port.
    struct BootstrapOnlyTransport {
        bootstrap: PeerNode,
        blocks: BTreeMap<BlockHash, BlockMessage>,
        incoming: tokio::sync::mpsc::Sender<BlockMessage>,
        asked: std::sync::Mutex<Vec<PeerNode>>,
    }

    #[async_trait]
    impl TransportLayer for BootstrapOnlyTransport {
        async fn send(&self, peer: &PeerNode, msg: Protocol) -> CommErr<()> {
            self.asked.lock().unwrap().push(peer.clone());
            if peer.id == self.bootstrap.id {
                if let Some(rchain_models::comm::protocol::protocol::Message::Packet(packet)) =
                    msg.message
                {
                    if let Ok(request) = BlockRequestSerde.parse(&packet.content) {
                        if let Some(block) = self.blocks.get(&request.hash) {
                            chan::send(&self.incoming, block.clone()).await;
                        }
                    }
                }
            }
            Ok(()) // transport acknowledgement does not establish reverse reachability
        }

        async fn broadcast(&self, peers: &[PeerNode], msg: Protocol) -> Vec<CommErr<()>> {
            let mut outcomes = Vec::new();
            for peer in peers {
                outcomes.push(self.send(peer, msg.clone()).await);
            }
            outcomes
        }

        async fn stream(&self, _peers: &[PeerNode], _blob: Blob) {}
    }

    /// The fringe and state-page source is already known before discovery adds
    /// it to the connection set. Randomly selecting other peers strands this
    /// otherwise serviceable ancestry behind their unreachable reply paths.
    #[tokio::test]
    async fn an_admitted_bootstrap_serves_the_walk_without_reverse_reachability_to_other_peers() {
        let blocks = chain(28);
        let bootstrap = peer("bootstrap");
        let store = store().await;
        let (incoming_tx, mut incoming_rx) = tokio::sync::mpsc::channel(64);
        let transport = Arc::new(BootstrapOnlyTransport {
            bootstrap: bootstrap.clone(),
            blocks: blocks.iter().map(|b| (b.block_hash, b.clone())).collect(),
            incoming: incoming_tx,
            asked: std::sync::Mutex::new(Vec::new()),
        });
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
        // Deliberately exclude the bootstrap: it has admitted the fringe, but
        // discovery has not added it; the other peers cannot deliver replies.
        let connections = Arc::new(tokio::sync::RwLock::new(vec![peer("unreachable")]));
        let comm = CommUtil::new(transport.clone(), conf, connections, Arc::new(NopLog));
        let fringe = FinalizedFringe {
            hashes: vec![blocks.last().unwrap().block_hash],
            state_hash: StateHash::new([0; 32]),
            ancestry: Vec::new(),
        };
        let state = tokio::time::timeout(
            Duration::from_secs(2),
            request_blocks(
                &fringe,
                &mut incoming_rx,
                Duration::from_millis(20),
                &store,
                &comm,
                &NopLog,
                &no_requests(),
            ),
        )
        .await
        .expect("a reachable source must not hang")
        .expect("the authenticated bootstrap serves every required block");
        assert!(state.is_finished());
        assert_eq!(state.finished.len(), blocks.len());
        let asked = transport.asked.lock().unwrap();
        assert_eq!(asked.len(), blocks.len(), "one request per required block");
        assert!(asked.iter().all(|p| p.id == bootstrap.id));
    }

    /// **U13's instrument (AUDIT C62's serving term): the LFS block walk advances on *responses*, not
    /// on the idle resend.**
    ///
    /// The devnet showed a fresh validator pulling blocks at ~1.5 per minute, with the requester's own
    /// `No block responses for 30s. Resending requests.` warning between them. On a chain a block's
    /// only justification is its parent, so the walk is one generation per block — which makes that
    /// rate **one idle timeout per block** rather than one round trip. This test isolates the
    /// requester: a chain of `N` blocks, a transport that answers every request immediately from the
    /// requester's own point of view, and the production `request_timeout` of 30 s. A
    /// response-driven walk finishes in milliseconds; a walk that waits for the resend takes
    /// `N × 30 s`. The bound sits at 5 s — three orders of magnitude above the healthy shape and
    /// below even one timeout, so it cannot pass by being lucky.
    #[tokio::test]
    async fn the_walk_advances_on_responses_not_on_the_idle_timeout() {
        const N: usize = 6;
        const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
        const BOUND: Duration = Duration::from_secs(5);

        let blocks = chain(N);
        let by_hash: BTreeMap<BlockHash, BlockMessage> =
            blocks.iter().map(|b| (b.block_hash, b.clone())).collect();
        let tip = blocks.last().expect("a non-empty chain").block_hash;
        let store = store().await;
        let (incoming_tx, mut incoming_rx) = tokio::sync::mpsc::channel(64);
        let transport = Arc::new(ServingTransport {
            blocks: by_hash,
            incoming: incoming_tx,
        });
        let connections: ConnectionsCell =
            Arc::new(tokio::sync::RwLock::new(vec![peer("bootstrap")]));
        let conf = RPConf {
            local: peer("local"),
            network_id: "testnet".to_string(),
            bootstrap: None,
            default_timeout: Duration::from_secs(10),
            max_num_of_connections: 10,
            clear_connections: ClearConnectionsConf {
                num_of_connections_pinged: 10,
            },
        };
        let comm_util = CommUtil::new(transport, conf, connections, Arc::new(NopLog));
        let fringe = FinalizedFringe {
            hashes: vec![tip],
            state_hash: StateHash::new([0u8; 32]),
            ancestry: Vec::new(),
        };

        let started = Instant::now();
        let state = request_blocks(
            &fringe,
            &mut incoming_rx,
            REQUEST_TIMEOUT,
            &store,
            &comm_util,
            &NopLog,
            &no_requests(),
        )
        .await
        .expect("a healthy store");
        let elapsed = started.elapsed();
        println!(
            "walk of {N} blocks: {elapsed:?} (idle timeout {REQUEST_TIMEOUT:?}), finished: {}",
            state.is_finished()
        );

        assert!(state.is_finished(), "the walk completed");
        let stored = store
            .contains(
                &blocks
                    .iter()
                    .map(|b| b.block_hash)
                    .collect::<Vec<BlockHash>>(),
            )
            .await
            .expect("store readable");
        assert!(
            stored.iter().all(|c| *c),
            "every block in the chain was fetched and saved"
        );
        assert!(
            elapsed < BOUND,
            "a {N}-block walk against a peer that answers every request took {elapsed:?} (bound \
             {BOUND:?}, idle timeout {REQUEST_TIMEOUT:?}): the walk is advancing on the idle resend \
             rather than on the response, so each block costs one request timeout"
        );
    }

    /// **The walk is unbounded: it reaches genesis, and the oracle's `lowerBound` cutoff does not
    /// bound it there either** (AUDIT C64).
    ///
    /// `lfs_block_requester.rs:26` and `engine/mod.rs:50` both say the port omits the Scala's
    /// `lowerBound`/`extraHeights` cutoff and walks the full ancestry instead, and AUDIT C64 carried
    /// "adopt the Scala's lowerBound cutoff" as the remedy. **That remedy is a no-op, and the
    /// divergence it exists to close does not exist.** In `LfsBlockRequester.scala` the bound is a
    /// parameter of `ST.apply` defaulting to `0` (`:125`) and the production construction is the only
    /// one in the tree — `ST(initialHashes, latest = finalizedHashes, extraHeights =
    /// blockHeightsBeforeFringe)` (`:318`) — which **never passes `lowerBound`**. So `minimumHeight`
    /// is `0` for the whole sync, and both gates are vacuously true: `blockIsAccepted =
    /// isReceivedLatest || isReceived && blockNumber >= minimumHeight` (`:228`) and `populateDag`'s
    /// `blockHeightOk = blockHeight >= minHeight` (`NodeSyncing.scala`, fed `st.lowerBound`). The
    /// `lowerBound` machinery is exercised only by `LfsBlockRequesterStateSpec`, which passes
    /// `lowerBound = 200` itself.
    ///
    /// `extraHeights` is not a bound on the walk either — `deployLifespan = 50`
    /// (`MultiParentCasper.scala:35`) arrives as `extraHeights`, and it *reduces* the bound further
    /// (`max(0, min(height - 1, lowerBound) - extraHeights)`), so it lengthens the walk rather than
    /// shortening it. The `N > 50` chain is therefore the discriminating fixture: a cutoff anywhere
    /// near `deployLifespan` would leave the genesis of this chain unrequested.
    ///
    /// This is the witness the register said did not exist ("nothing in this register would catch a
    /// wrong one", C94). It pins the port's own behaviour — every block of an over-`deployLifespan`
    /// chain, genesis included, is fetched and saved — so a future pass that re-adds the cutoff
    /// without first defining the truncated-ancestry DAG semantics fails here.
    #[tokio::test]
    async fn a_walk_longer_than_deploy_lifespan_reaches_genesis() {
        // The oracle's `MultiParentCasper.deployLifespan`.
        const DEPLOY_LIFESPAN: usize = 50;
        const N: usize = DEPLOY_LIFESPAN + 1;

        let blocks = chain(N);
        let genesis = blocks.first().expect("a non-empty chain");
        assert_eq!(
            i64::from(genesis.block_number),
            0,
            "the chain's first block is its genesis, which is what a cutoff would strand"
        );
        let by_hash: BTreeMap<BlockHash, BlockMessage> =
            blocks.iter().map(|b| (b.block_hash, b.clone())).collect();
        let tip = blocks.last().expect("a non-empty chain").block_hash;
        let store = store().await;
        let (incoming_tx, mut incoming_rx) = tokio::sync::mpsc::channel(128);
        let transport = Arc::new(ServingTransport {
            blocks: by_hash,
            incoming: incoming_tx,
        });
        let connections: ConnectionsCell =
            Arc::new(tokio::sync::RwLock::new(vec![peer("bootstrap")]));
        let conf = RPConf {
            local: peer("local"),
            network_id: "testnet".to_string(),
            bootstrap: None,
            default_timeout: Duration::from_secs(10),
            max_num_of_connections: 10,
            clear_connections: ClearConnectionsConf {
                num_of_connections_pinged: 10,
            },
        };
        let comm_util = CommUtil::new(transport, conf, connections, Arc::new(NopLog));
        let fringe = FinalizedFringe {
            hashes: vec![tip],
            state_hash: StateHash::new([0u8; 32]),
            ancestry: Vec::new(),
        };

        let state = request_blocks(
            &fringe,
            &mut incoming_rx,
            Duration::from_secs(30),
            &store,
            &comm_util,
            &NopLog,
            &no_requests(),
        )
        .await
        .expect("a healthy store");

        assert!(state.is_finished(), "the walk completed");
        // The genesis block's height is the low end of the ancestry, so it is the block a
        // `lowerBound` cutoff at or near `deployLifespan` would never ask for.
        assert!(
            store
                .contains(&[genesis.block_hash])
                .await
                .expect("store readable")[0],
            "a {N}-block walk must reach the genesis block: the port has no cutoff, and neither \
             does the oracle's production path (`lowerBound` is never seeded off its `0` default)"
        );
    }
    /// A block store that fails what it is told to fail — the fault injection AUDIT C65's two
    /// falsifiers need, since both defects live on error paths that a healthy store never reaches.
    struct FailingStore {
        inner: BlockStore,
        fail_puts: bool,
        fail_reads: bool,
    }

    #[async_trait]
    impl KeyValueTypedStore<BlockHash, BlockMessage> for FailingStore {
        async fn get(&self, keys: &[BlockHash]) -> Result<Vec<Option<BlockMessage>>, String> {
            if self.fail_reads {
                return Err("store is unreadable".to_string());
            }
            self.inner.get(keys).await
        }

        async fn put(&self, pairs: &[(BlockHash, BlockMessage)]) -> Result<(), String> {
            if self.fail_puts {
                return Err("store refuses writes".to_string());
            }
            self.inner.put(pairs).await
        }

        async fn delete(&self, keys: &[BlockHash]) -> Result<usize, String> {
            self.inner.delete(keys).await
        }

        async fn contains(&self, keys: &[BlockHash]) -> Result<Vec<bool>, String> {
            if self.fail_reads {
                return Err("store is unreadable".to_string());
            }
            self.inner.contains(keys).await
        }

        async fn to_map(&self) -> Result<BTreeMap<BlockHash, BlockMessage>, String> {
            self.inner.to_map().await
        }
    }

    /// The comm state a walk runs with, pointed at a transport that answers every request.
    fn comm_state(
        blocks: &[BlockMessage],
        incoming: tokio::sync::mpsc::Sender<BlockMessage>,
    ) -> CommUtil {
        let by_hash: BTreeMap<BlockHash, BlockMessage> =
            blocks.iter().map(|b| (b.block_hash, b.clone())).collect();
        let connections: ConnectionsCell =
            Arc::new(tokio::sync::RwLock::new(vec![peer("bootstrap")]));
        let conf = RPConf {
            local: peer("local"),
            network_id: "testnet".to_string(),
            bootstrap: None,
            default_timeout: Duration::from_secs(10),
            max_num_of_connections: 10,
            clear_connections: ClearConnectionsConf {
                num_of_connections_pinged: 10,
            },
        };
        CommUtil::new(
            Arc::new(ServingTransport {
                blocks: by_hash,
                incoming,
            }),
            conf,
            connections,
            Arc::new(NopLog),
        )
    }

    /// **AUDIT C65, first half: a failed block write fails the walk, and the block is not marked
    /// done.**
    ///
    /// `save_block` read `already_saved` through `unwrap_or_default()` (a store error reading as "not
    /// saved"), **discarded** the result of `put`, and marked the block `done` regardless — so on a
    /// write failure the requester believed it held a block it never persisted, and `done` removes the
    /// key from the request map, which means it would never be requested again. The oracle's
    /// `saveBlock` is `containsBlock` then `putBlockToStore` in `F`: a failed write fails the sync
    /// attempt.
    ///
    /// Falsified in the witnessing form: with the discard restored (`let _ = block_store.put(..)`)
    /// this returns `Ok`, the block is marked done, and the store is empty — success reported over a
    /// silent loss, which is what the assertion below refuses.
    #[tokio::test]
    async fn a_failed_block_write_fails_the_walk_instead_of_marking_the_block_done() {
        let blocks = chain(3);
        let tip = blocks.last().expect("a non-empty chain").block_hash;
        let inner = store().await;
        let failing: BlockStore = Arc::new(FailingStore {
            inner: inner.clone(),
            fail_puts: true,
            fail_reads: false,
        });
        let (incoming_tx, mut incoming_rx) = tokio::sync::mpsc::channel(64);
        let comm_util = comm_state(&blocks, incoming_tx);
        let fringe = FinalizedFringe {
            hashes: vec![tip],
            state_hash: StateHash::new([0u8; 32]),
            ancestry: Vec::new(),
        };

        let outcome = request_blocks(
            &fringe,
            &mut incoming_rx,
            Duration::from_secs(5),
            &failing,
            &comm_util,
            &NopLog,
            &no_requests(),
        )
        .await;

        assert!(
            outcome.is_err(),
            "a store that refuses the write must fail the walk; reporting success would leave the \
             block marked done with nothing persisted"
        );
        assert!(
            !inner.to_map().await.expect("readable").contains_key(&tip),
            "nothing was persisted, so nothing may be reported as saved"
        );
    }

    /// **AUDIT C65, second half: a failed `contains` must not read as "nothing to request".**
    ///
    /// `request_next` computed the request set from
    /// `block_store.contains(&hashes).await.unwrap_or_default()`: an error yields an **empty**
    /// `Vec<bool>`, the `zip` yields nothing, and every hash is silently neither reported as existing
    /// nor requested. The walk then does nothing until the idle resend nudges it — a stall that looks
    /// exactly like a pacing property. The oracle's `filterA(containsBlock)` propagates the error.
    ///
    /// Falsified in the witnessing form: with `unwrap_or_default()` restored the call neither fails
    /// nor progresses, so the bounded wait below fires and the assertion reports the stall. The bound
    /// is 5 s against a 1 s idle timeout, so a *timeout-driven* walk would have had five chances to
    /// do something and done nothing.
    #[tokio::test]
    async fn a_failed_store_read_fails_the_walk_instead_of_requesting_nothing() {
        let blocks = chain(3);
        let tip = blocks.last().expect("a non-empty chain").block_hash;
        let inner = store().await;
        let failing: BlockStore = Arc::new(FailingStore {
            inner,
            fail_puts: false,
            fail_reads: true,
        });
        let (incoming_tx, mut incoming_rx) = tokio::sync::mpsc::channel(64);
        let comm_util = comm_state(&blocks, incoming_tx);
        let fringe = FinalizedFringe {
            hashes: vec![tip],
            state_hash: StateHash::new([0u8; 32]),
            ancestry: Vec::new(),
        };

        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            request_blocks(
                &fringe,
                &mut incoming_rx,
                Duration::from_secs(1),
                &failing,
                &comm_util,
                &NopLog,
                &no_requests(),
            ),
        )
        .await;

        match outcome {
            Ok(result) => assert!(
                result.is_err(),
                "a store whose reads fail must fail the walk, not report an empty request set"
            ),
            Err(_) => panic!(
                "the walk neither failed nor progressed: a failed `contains` read as \"nothing to \
                 request\", so it waited on the idle resend instead of surfacing the error"
            ),
        }
    }

    /// **The premise of `node_syncing`'s empty-restore guard (issue #100): an empty fringe finishes
    /// the walk at once, successfully, with nothing in it.**
    ///
    /// All three halves are the point. It returns `Ok` — so a caller that reads only the `Result`
    /// cannot tell "the state was restored" from "there was nothing to restore", which is exactly how
    /// a node came to log `LFS state is successfully restored.` having restored nothing and then run on
    /// an empty DAG. The `height_map` is empty, which is what `run_approved_state_sync` now refuses.
    /// And not one request goes out, which the recording transport asserts rather than assumes: the
    /// emptiness check belongs to the *consumer*, because this function's own contract — "walk what
    /// the fringe names" — is honestly satisfied by an empty fringe.
    #[tokio::test]
    async fn an_empty_fringe_finishes_the_walk_at_once_with_nothing_in_it() {
        const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
        const BOUND: Duration = Duration::from_secs(5);

        let store = store().await;
        let (_incoming_tx, mut incoming_rx) = tokio::sync::mpsc::channel(64);
        let transport = Arc::new(RecordingTransport::default());
        let connections: ConnectionsCell =
            Arc::new(tokio::sync::RwLock::new(vec![peer("bootstrap")]));
        let conf = RPConf {
            local: peer("local"),
            network_id: "testnet".to_string(),
            bootstrap: None,
            default_timeout: Duration::from_secs(10),
            max_num_of_connections: 10,
            clear_connections: ClearConnectionsConf {
                num_of_connections_pinged: 10,
            },
        };
        let comm_util = CommUtil::new(transport.clone(), conf, connections, Arc::new(NopLog));
        let fringe = FinalizedFringe {
            hashes: Vec::new(),
            state_hash: StateHash::new([0u8; 32]),
            ancestry: Vec::new(),
        };

        let started = Instant::now();
        let state = request_blocks(
            &fringe,
            &mut incoming_rx,
            REQUEST_TIMEOUT,
            &store,
            &comm_util,
            &NopLog,
            &no_requests(),
        )
        .await
        .expect("an empty fringe is not an error for this function");
        let elapsed = started.elapsed();

        assert!(
            state.is_finished(),
            "an empty fringe is finished before it starts: `LfsState::is_finished` is \
             `latest.is_empty() && d.is_empty()`, and both start empty"
        );
        assert!(
            state.height_map.is_empty(),
            "and it recorded no block, which is the shape `run_approved_state_sync` refuses"
        );
        assert!(
            transport
                .broadcasts
                .lock()
                .expect("broadcast log lock")
                .is_empty(),
            "and it asked the peer for nothing: a walk over no hashes sends no request"
        );
        assert!(
            elapsed < BOUND,
            "and it did not wait out the idle resend ({REQUEST_TIMEOUT:?}): took {elapsed:?}"
        );
    }

    /// A transport that answers only after it has been asked for the same hash `after` times — the
    /// shape a *slow but honest* peer has from the requester's point of view: one that needs several
    /// resends before it can serve.
    struct SlowServingTransport {
        blocks: BTreeMap<BlockHash, BlockMessage>,
        incoming: tokio::sync::mpsc::Sender<BlockMessage>,
        after: usize,
        asked: std::sync::Mutex<BTreeMap<BlockHash, usize>>,
    }

    #[async_trait]
    impl TransportLayer for SlowServingTransport {
        async fn send(&self, _peer: &PeerNode, _msg: Protocol) -> CommErr<()> {
            Ok(())
        }

        async fn broadcast(&self, _peers: &[PeerNode], msg: Protocol) -> Vec<CommErr<()>> {
            let Some(rchain_models::comm::protocol::protocol::Message::Packet(packet)) =
                msg.message
            else {
                return Vec::new();
            };
            if packet.type_id != "BlockRequest" {
                return Vec::new();
            }
            let Ok(request) = BlockRequestSerde.parse(&packet.content) else {
                return Vec::new();
            };
            let hash = request.hash;
            let ask_count = {
                let mut asked = self.asked.lock().expect("ask-count lock");
                let n = asked.entry(hash).or_insert(0);
                *n += 1;
                *n
            };
            if ask_count >= self.after {
                if let Some(block) = self.blocks.get(&hash) {
                    chan::send(&self.incoming, block.clone()).await;
                }
            }
            Vec::new()
        }

        async fn stream(&self, _peers: &[PeerNode], _blob: Blob) {}
    }

    /// **Issue #102's owed falsifier: a walk whose fringe block is served by nobody fails rather than
    /// hangs.**
    ///
    /// The oracle's only termination condition is `isFinished` (`LfsBlockRequester.scala:309-312`), so
    /// a fringe naming state no peer has retries forever — and `run_approved_state_sync` `join!`s the
    /// block walk with the tuple-space request, so "forever" is the whole sync attempt: the spawned
    /// task never returns, `notify_when_restored` never fires, and the node sits in `NodeSyncing` with
    /// a serving API and no error line. The bound here is the walk's own rule
    /// ([`MAX_IDLE_ROUNDS`]), not a test harness: the `timeout` below exists only so that a
    /// regression *hangs the assertion* rather than the suite.
    ///
    /// Two assertions carry the content. The walk **fails**, and the error names the silence rather
    /// than the fringe. And it **retried first** — `broadcasts` is non-empty past the initial fire —
    /// because a rule that gave up on the first empty interval would kill every slow honest sync; the
    /// companion test below is what separates the two.
    #[tokio::test]
    async fn a_walk_nobody_serves_fails_rather_than_hangs() {
        const REQUEST_TIMEOUT: Duration = Duration::from_millis(20);
        // Three idle rounds of 20 ms is 60 ms; two seconds is thirty times that, and the timeout is
        // a harness guard rather than the bound under test.
        const HARNESS_BOUND: Duration = Duration::from_secs(2);

        let blocks = chain(3);
        let tip = blocks.last().expect("a non-empty chain").block_hash;
        let store = store().await;
        let (_incoming_tx, mut incoming_rx) = tokio::sync::mpsc::channel(64);
        // Nobody answers: every request is recorded and dropped, which is what "the fringe names
        // state no peer has" looks like from the requester's own point of view.
        let transport = Arc::new(RecordingTransport::default());
        let connections: ConnectionsCell =
            Arc::new(tokio::sync::RwLock::new(vec![peer("bootstrap")]));
        let conf = RPConf {
            local: peer("local"),
            network_id: "testnet".to_string(),
            bootstrap: None,
            default_timeout: Duration::from_secs(10),
            max_num_of_connections: 10,
            clear_connections: ClearConnectionsConf {
                num_of_connections_pinged: 10,
            },
        };
        let comm_util = CommUtil::new(transport.clone(), conf, connections, Arc::new(NopLog));
        let fringe = FinalizedFringe {
            hashes: vec![tip],
            state_hash: StateHash::new([0u8; 32]),
            ancestry: Vec::new(),
        };

        let outcome = tokio::time::timeout(
            HARNESS_BOUND,
            request_blocks(
                &fringe,
                &mut incoming_rx,
                REQUEST_TIMEOUT,
                &store,
                &comm_util,
                &NopLog,
                &no_requests(),
            ),
        )
        .await;

        let err = match outcome {
            Ok(Ok(state)) => panic!(
                "the walk reported success with the fringe unserved: {} block(s) finished, fringe \
                 {} hash(es)",
                state.finished.len(),
                fringe.hashes.len()
            ),
            Ok(Err(e)) => e,
            Err(_) => panic!(
                "the walk hung: {HARNESS_BOUND:?} with no answer and no failure. This is #102's \
                 defect — the node stays in `NodeSyncing` for good, serving an API over an empty DAG"
            ),
        };
        assert!(
            err.contains("no block finished"),
            "the failure must name the silence, not the fringe: {err}"
        );
        assert!(
            err.contains("outstanding:"),
            "the failure must report the status breakdown (#457): a count alone did not say whether \
             keys were init, requested or stranded as received: {err}"
        );
        assert!(
            err.contains("no block was received from the peers they were requested from"),
            "the failure must attribute the missing hash to the peers it was requested from, not to \
             the whole connection set: {err}"
        );
        assert!(
            transport
                .broadcasts
                .lock()
                .expect("broadcast log lock")
                .len()
                > 1,
            "the walk must have re-requested before giving up: one broadcast is the initial fire, so \
             a rule that abandoned the walk at the first empty interval would show exactly one"
        );
    }

    /// **The other direction: a slow walk that is still completing blocks is not abandoned.**
    ///
    /// This is what makes [`MAX_IDLE_ROUNDS`] a *pace* rule rather than a deadline. The transport here
    /// serves each block only on its **third** request — the walk's initial fire plus one resend per
    /// idle round — so every block costs two idle rounds with no completion, the counter reaches
    /// `MAX_IDLE_ROUNDS - 1`, and the block that then lands resets it. A rule that counted idle rounds
    /// without resetting on progress, or one that measured elapsed time instead of progress, fails
    /// here: the first abandons the walk on the second block, and the second abandons a chain that is
    /// long rather than stuck.
    ///
    /// **The timeout is 100 ms and the count is calibrated to it**: the reset depends on the response
    /// loop having processed the block before the next idle round fires, which is microseconds of work
    /// against a hundred-millisecond interval — three orders of margin, not a race the test leans on.
    #[tokio::test]
    async fn a_slow_but_progressing_walk_is_not_abandoned() {
        const REQUEST_TIMEOUT: Duration = Duration::from_millis(100);
        const N: usize = 3;
        const ASKED_BEFORE_SERVED: usize = 3;
        const HARNESS_BOUND: Duration = Duration::from_secs(10);

        let blocks = chain(N);
        let tip = blocks.last().expect("a non-empty chain").block_hash;
        let by_hash: BTreeMap<BlockHash, BlockMessage> =
            blocks.iter().map(|b| (b.block_hash, b.clone())).collect();
        let store = store().await;
        let (incoming_tx, mut incoming_rx) = tokio::sync::mpsc::channel(64);
        let transport = Arc::new(SlowServingTransport {
            blocks: by_hash,
            incoming: incoming_tx,
            after: ASKED_BEFORE_SERVED,
            asked: std::sync::Mutex::new(BTreeMap::new()),
        });
        let connections: ConnectionsCell =
            Arc::new(tokio::sync::RwLock::new(vec![peer("bootstrap")]));
        let conf = RPConf {
            local: peer("local"),
            network_id: "testnet".to_string(),
            bootstrap: None,
            default_timeout: Duration::from_secs(10),
            max_num_of_connections: 10,
            clear_connections: ClearConnectionsConf {
                num_of_connections_pinged: 10,
            },
        };
        let comm_util = CommUtil::new(transport, conf, connections, Arc::new(NopLog));
        let fringe = FinalizedFringe {
            hashes: vec![tip],
            state_hash: StateHash::new([0u8; 32]),
            ancestry: Vec::new(),
        };

        let state = tokio::time::timeout(
            HARNESS_BOUND,
            request_blocks(
                &fringe,
                &mut incoming_rx,
                REQUEST_TIMEOUT,
                &store,
                &comm_util,
                &NopLog,
                &no_requests(),
            ),
        )
        .await
        .expect("a progressing walk must finish well inside the harness bound")
        .expect(
            "a peer that is slow but answering must not fail the walk: the give-up rule counts \
             *consecutive* rounds without a finished block, and every block resets it",
        );

        assert!(
            state.is_finished(),
            "the walk reached the fringe after {ASKED_BEFORE_SERVED} requests per block"
        );
        assert_eq!(
            state.finished.len(),
            N,
            "and it completed every block of the chain"
        );
    }

    /// **The mature-chain join #457 needs, in the tree.** The live walk stopped at 3 finished of 28
    /// blocks against a bootstrap that *was* answering; the unit tests around this one use chains of
    /// one to three blocks, so nothing exercised the shape the defect appeared in. This serves a
    /// 28-block chain whose every block justifies its parent — the topology the requester grows the
    /// request set along — and requires the walk to finish every block.
    #[tokio::test]
    async fn a_mature_chain_join_completes() {
        const REQUEST_TIMEOUT: Duration = Duration::from_millis(100);
        const N: usize = 28;
        const HARNESS_BOUND: Duration = Duration::from_secs(10);

        let blocks = chain(N);
        let tip = blocks.last().expect("a non-empty chain").block_hash;
        let by_hash: BTreeMap<BlockHash, BlockMessage> =
            blocks.iter().map(|b| (b.block_hash, b.clone())).collect();
        let store = store().await;
        let (incoming_tx, mut incoming_rx) = tokio::sync::mpsc::channel(64);
        let transport = Arc::new(SlowServingTransport {
            blocks: by_hash,
            incoming: incoming_tx,
            after: 1,
            asked: std::sync::Mutex::new(BTreeMap::new()),
        });
        let connections: ConnectionsCell =
            Arc::new(tokio::sync::RwLock::new(vec![peer("bootstrap")]));
        let conf = RPConf {
            local: peer("local"),
            network_id: "testnet".to_string(),
            bootstrap: None,
            default_timeout: Duration::from_secs(10),
            max_num_of_connections: 10,
            clear_connections: ClearConnectionsConf {
                num_of_connections_pinged: 10,
            },
        };
        let comm_util = CommUtil::new(transport, conf, connections, Arc::new(NopLog));
        let fringe = FinalizedFringe {
            hashes: vec![tip],
            state_hash: StateHash::new([0u8; 32]),
            ancestry: Vec::new(),
        };

        let state = tokio::time::timeout(
            HARNESS_BOUND,
            request_blocks(
                &fringe,
                &mut incoming_rx,
                REQUEST_TIMEOUT,
                &store,
                &comm_util,
                &NopLog,
                &no_requests(),
            ),
        )
        .await
        .expect("a served mature chain must finish inside the harness bound")
        .expect("every block of the chain is served, so the walk must not fail");

        assert!(state.is_finished(), "the walk reached the fringe");
        assert_eq!(
            state.finished.len(),
            N,
            "every block of the chain completed"
        );
        assert_eq!(
            state.outstanding_breakdown(),
            (0, 0, 0),
            "and nothing was left outstanding — the stranded-key shape that stopped the live join"
        );
    }
}
