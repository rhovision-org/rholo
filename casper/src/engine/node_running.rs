//! NodeRunning engine (port of `engine/NodeRunning.scala`).
//!
//! The message handlers wire the transport layer to the block store / DAG / block retriever: block
//! hash broadcasts and has-block messages feed the retriever, block requests are served from the
//! store, fork-choice-tip / finalized-fringe requests are served from the DAG, and store-items
//! (LFS state-sync) requests are served from the RSpace exporter.

use rchain_shared::lock::Unpoison;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rchain_block_storage::block_store::BlockStore;
use rchain_block_storage::dag::dag_storage::BlockDagStorage;
use rchain_comm::peer_node::PeerNode;
use rchain_comm::rp::rp_conf::RPConf;
use rchain_comm::transport::transport_layer::TransportLayer;
use rchain_comm::transport::transport_layer_syntax;
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_models::block::state_hash::StateHash;
use rchain_models::block_hash::BlockHash;
use rchain_models::casper::protocol::casper_message::{
    BlockFringe, BlockMessage, BlockRange, BlockRangeRequest, BlockRequest, CasperMessage,
    FinalizedFringe, FinalizedFringeRequest, HasBlock, HasBlockRequest, StoreItemsMessage,
    StoreItemsMessageRequest,
};
use rchain_models::casper::protocol::packet_type_tag::ToPacket;
use rchain_rspace::state::RSpaceExporter;
use rchain_shared::log::{Log, LogSource};
use rchain_shared::refined::BlockHeight;

use crate::blocks::block_receiver::not_validated;
use crate::blocks::block_retriever::{AdmitHashReason, BlockRetriever};
use crate::engine::catchup::CatchupWindow;
use crate::protocol::casper_message_protocol::{
    BlockMessageSerde, BlockRangeSerde, FinalizedFringeSerde, HasBlockSerde, StoreItemsMessageSerde,
};
use crate::validator_identity::ValidatorIdentity;

/// Handle a peer-broadcast block hash (port of `handleBlockHashMessage`).
pub async fn handle_block_hash_message(
    block_retriever: &BlockRetriever,
    log: &dyn Log,
    source: LogSource,
    peer: &PeerNode,
    hash: &BlockHash,
    ignore: bool,
) {
    if ignore {
        log.debug(
            source,
            &format!("Ignoring {} hash broadcast", hash.to_hex()),
        );
    } else {
        log.debug(
            source,
            &format!(
                "Incoming BlockHashMessage {} from {}",
                hash.to_hex(),
                peer.endpoint.host
            ),
        );
        let _ = block_retriever
            .admit_hash(hash, Some(peer), AdmitHashReason::HashBroadcastRecieved)
            .await;
    }
}

/// Handle a peer reporting that it has a particular block (port of `handleHasBlockMessage`).
pub async fn handle_has_block_message(
    block_retriever: &BlockRetriever,
    log: &dyn Log,
    source: LogSource,
    peer: &PeerNode,
    hash: &BlockHash,
    ignore: bool,
) {
    if ignore {
        log.debug(
            source,
            &format!("Ignoring {} HasBlockMessage", hash.to_hex()),
        );
    } else {
        log.debug(
            source,
            &format!(
                "Incoming HasBlockMessage {} from {}",
                hash.to_hex(),
                peer.endpoint.host
            ),
        );
        let _ = block_retriever
            .admit_hash(hash, Some(peer), AdmitHashReason::HasBlockMessageReceived)
            .await;
    }
}

/// Per-peer block-request limit (requests/second). Generous enough for sync bursts while bounding
/// the outbound bandwidth a single peer can pull.
const DEFAULT_BLOCK_REQUEST_LIMIT_PER_SEC: u32 = 100;

/// Upper bound on `StoreItemsMessageRequest.take` — a peer-controlled state-sync walk is capped so
/// a `take=i32::MAX` cannot traverse/serialize the whole trie.
const MAX_STORE_ITEMS_TAKE: usize = 10_000;

/// Byte cap on a store-items page this node will serve (AUDIT C73). `MAX_STORE_ITEMS_TAKE` bounds how
/// many nodes a request may name; this bounds how much data the walk may hand back, because a node's
/// *value* size is not bounded by the request at all — the maximal `take` with fat values is tens of
/// megabytes of the responder's memory per request, repeated per peer.
///
/// **Refused, never truncated.** The requester recomputes the page it expects and requires the received
/// keys to match (`validate_state_items`), so a short page is not a smaller answer — it is a wrong state
/// claim the importer would then validate its own traversal against (the same reason the unreadable-store
/// path drops rather than serving an empty page, AUDIT C63). This handler has no error reply to send, so
/// the policy is the one it already applies to an over-large `take`: drop, log, and let the requester
/// time out and retry. The oracle has no such cap (it serves whatever it is asked), which makes this a
/// registered deviation — the responder's only defence that is not a lie.
///
/// 32 MiB is ~10× the largest *legitimate* page (`PAGE_SIZE = 750` nodes × 4 KiB items ≈ 3 MB) and below
/// the ~41 MB the maximal `take` produces at that item size, so the two caps bracket the same hostile
/// request from both sides.
pub const MAX_STORE_ITEMS_BYTES: usize = 32 * 1024 * 1024;

/// Per-peer fixed-window rate limiter for block requests (H4): bounds the outbound bandwidth a
/// single peer can pull by requesting blocks. Documented Scala deviation: Scala serves every block
/// request with no limit.
pub struct PeerRateLimiter {
    max_per_sec: u32,
    state: Mutex<BTreeMap<Vec<u8>, (Instant, u32)>>,
}

impl PeerRateLimiter {
    pub fn new(max_per_sec: u32) -> Self {
        PeerRateLimiter {
            max_per_sec,
            state: Mutex::new(BTreeMap::new()),
        }
    }

    /// Admit a request from `peer` if it is within the per-peer one-second window.
    pub fn allow(&self, peer_key: &[u8]) -> bool {
        let now = Instant::now();
        let mut state = self.state.lock().unpoison();
        // Prune peers whose window expired long ago, so a connection churn with many distinct node
        // ids (Kademlia-injectable) cannot grow this map without bound (R30).
        state.retain(|_, (last, _)| now.duration_since(*last) < Duration::from_secs(60));
        let entry = state.entry(peer_key.to_vec()).or_insert((now, 0));
        if now.duration_since(entry.0) >= Duration::from_secs(1) {
            entry.0 = now;
            entry.1 = 0;
        }
        entry.1 += 1;
        entry.1 <= self.max_per_sec
    }
}

/// Serve a peer's request for a block (port of `handleBlockRequest`), throttled per-peer.
pub async fn handle_block_request(
    transport: &dyn TransportLayer,
    conf: &RPConf,
    block_store: &BlockStore,
    log: &dyn Log,
    source: LogSource,
    peer: &PeerNode,
    br: &BlockRequest,
    limiter: &PeerRateLimiter,
) {
    let hash = br.hash;
    if !limiter.allow(peer.key()) {
        log.info(
            source,
            &format!(
                "Received request for block {} from {peer}. Dropped: per-peer block-request rate limit exceeded.",
                hash.to_hex()
            ),
        );
        return;
    }
    // A store that cannot be read skips the answer: an unanswered request is retryable, where a
    // `false` answer would be a state claim (AUDIT C67).
    let has_block = match block_is_known(block_store, &hash).await {
        Ok(known) => known,
        Err(e) => {
            log.error(
                source,
                &format!(
                    "Received request for block {} from {peer}. Dropped: the block store could not be read: {e}",
                    hash.to_hex()
                ),
            );
            return;
        }
    };
    if has_block {
        let stored = match block_by_hash(block_store, &hash).await {
            Ok(block) => block,
            Err(e) => {
                log.error(
                    source,
                    &format!(
                        "Received request for block {} from {peer}. Dropped: the block store could not be read: {e}",
                        hash.to_hex()
                    ),
                );
                return;
            }
        };
        if let Some(block) = stored {
            transport_layer_syntax::stream_to_peer(
                transport,
                conf,
                peer,
                BlockMessageSerde.mk_packet(&block),
            )
            .await;
        }
        log.info(
            source,
            &format!(
                "Received request for block {} from {peer}. Response sent.",
                hash.to_hex()
            ),
        );
    } else {
        log.info(
            source,
            &format!(
                "Received request for block {} from {peer}. No response given since block not found.",
                hash.to_hex()
            ),
        );
    }
}

/// Respond to a peer's has-block query (port of `handleHasBlockRequest`).
pub async fn handle_has_block_request(
    transport: &dyn TransportLayer,
    conf: &RPConf,
    log: &dyn Log,
    source: LogSource,
    peer: &PeerNode,
    hbr: &HasBlockRequest,
    has_block: bool,
) {
    if has_block {
        if let Err(e) = transport_layer_syntax::send_to_peer(
            transport,
            conf,
            peer,
            HasBlockSerde.mk_packet(&HasBlock { hash: hbr.hash }),
        )
        .await
        {
            // **Loud, because the caller cannot be** (C254's E6b): this handler returns nothing, so a
            // packet that never left the node used to leave no trace anywhere. The response to a peer's
            // request either went or it did not, and now the log says which.
            log.warn(
                source,
                &format!(
                    "could not tell {peer} this node has block {}: {e} (AUDIT C254's E6b)",
                    hbr.hash.to_hex()
                ),
            );
        }
    }
}

/// Respond to a peer's fork-choice-tip request (port of `handleForkChoiceTipRequest`).
pub async fn handle_fork_choice_tip_request(
    transport: &dyn TransportLayer,
    conf: &RPConf,
    dag: &dyn BlockDagStorage,
    log: &dyn Log,
    source: LogSource,
    peer: &PeerNode,
) {
    log.info(
        source,
        &format!("Received ForkChoiceTipRequest from {}", peer.endpoint.host),
    );
    let repr = dag.get_representation().await;
    let tips: Vec<BlockHash> = repr
        .dag_message_state
        .latest_msgs
        .values()
        .map(|m| m.id)
        .collect();
    for tip in &tips {
        if let Err(e) = transport_layer_syntax::send_to_peer(
            transport,
            conf,
            peer,
            HasBlockSerde.mk_packet(&HasBlock { hash: *tip }),
        )
        .await
        {
            log.warn(
                source,
                &format!(
                    "could not tell {peer} about the tip {} this node finalized: {e} (AUDIT C254's E6b)",
                    tip.to_hex()
                ),
            );
        }
    }
    log.info(
        source,
        &format!(
            "Sending tips {} to {}",
            tips.iter()
                .map(|t| t.to_hex())
                .collect::<Vec<_>>()
                .join(" "),
            peer.endpoint.host
        ),
    );
}

/// Answer a height-window request with the hashes at those heights, in topological order (C259's
/// catch-up).
///
/// **The window is bounded by this node's own limit, not by the requester's word** — a peer asking for
/// a hundred thousand heights gets the limit's worth, exactly as the block API's own depth limit
/// refuses a wider range (`casper/src/api/block_api_impl.rs::get_blocks_by_heights`, the worked example
/// of `topo_sort_unsafe`). The answer is hashes rather than blocks: a window is tens of blocks, the
/// requester already has `BlockRequest`, and keeping the payload to a kilobyte is what lets the window
/// be small enough to pace.
///
/// **A height with no blocks is simply absent from the answer**, which is why the response echoes the
/// bounds instead of a count: the requester advances by `to`, not by what it received.
pub async fn handle_block_range_request(
    transport: &dyn TransportLayer,
    conf: &RPConf,
    dag: &dyn BlockDagStorage,
    log: &dyn Log,
    source: LogSource,
    peer: &PeerNode,
    req: BlockRangeRequest,
) {
    let Some(response) = block_range_response(dag, log, source, &req).await else {
        return;
    };
    log.info(
        source,
        &format!(
            "answering a block-range request {}..={} from {} with {} hash(es)",
            response.from,
            response.to,
            peer.endpoint.host,
            response.hashes.len()
        ),
    );
    if let Err(e) = transport_layer_syntax::send_to_peer(
        transport,
        conf,
        peer,
        BlockRangeSerde.mk_packet(&response),
    )
    .await
    {
        log.warn(
            source,
            &format!(
                "could not answer a block-range request from {}: {e}",
                peer.endpoint.host
            ),
        );
    }
}

/// The answer to a height-window request, or `None` when the range cannot be ordered — the silence the
/// requester degrades on, and the same answer an unaware responder gives (C259's catch-up).
///
/// A free function of the DAG so the choice has a test that does not need a whole engine, which is the
/// shape `finalized_fringe_response` already has for its own (C259a).
pub async fn block_range_response(
    dag: &dyn BlockDagStorage,
    log: &dyn Log,
    source: LogSource,
    req: &BlockRangeRequest,
) -> Option<BlockRange> {
    let to = req.to.min(req.from.saturating_add(MAX_BLOCK_RANGE as i64));
    let repr = dag.get_representation().await;
    // The requester cannot tell "you have nothing above my frontier" from "you have nothing in this
    // window" from the hashes alone, and the difference is whether a walk against a *producing* peer
    // ever ends. So the answer says where this node's own frontier is.
    let tip = repr.latest_block_number() - 1;
    match repr.topo_sort_unsafe(req.from, Some(to)) {
        Ok(topo) => {
            let mut hashes = Vec::new();
            for level in &topo {
                hashes.extend(level.iter().copied());
            }
            Some(BlockRange {
                from: req.from,
                to,
                hashes,
                tip,
            })
        }
        Err(e) => {
            // A range this node cannot order — below its own floor, or malformed — is answered with
            // nothing rather than with a guess: a wrong window would have the requester validate the
            // wrong blocks, where silence costs it one empty step and it stops.
            log.warn(
                source,
                &format!("refusing a block-range request {}..={}: {e}", req.from, to),
            );
            None
        }
    }
}

/// Stream a finalized fringe to a peer (port of `handleFinalizedFringeRequest`).
/// **The per-block fringe state of a fringe's ancestry** (AUDIT C188, #139).
///
/// A node that *restores* a chain rather than validating it cannot derive this for itself: `fringe`
/// and `fringeStateHash` are the node's own recomputation, produced when it validates or creates a
/// block, and they are not block fields — so a restored node has no fringe to replay from, and every
/// block whose `close_block` anchors the epoch seed to the fringe state replays to a different
/// post-state. Measured before the fix: the joiner takes the LFS path and then stops at the height it
/// synced to, logging `regenerated mergeable channels for block … but replay computed … instead of …`.
///
/// The walk follows the message map's **parent links from the fringe itself**, which is the same set
/// the joiner restores — its block walk follows justifications from the same place. A block reached
/// without metadata is skipped and named in the log rather than silently omitted: the joiner then
/// falls back to its own derivation for that block, which is the pre-#139 behaviour for that block
/// alone, and the operator can see which one it was.
async fn collect_fringe_ancestry(
    dag: &dyn BlockDagStorage,
    fringe_hashes: &BTreeSet<BlockHash>,
    log: &dyn Log,
    source: LogSource,
) -> Vec<BlockFringe> {
    let mut seen: BTreeSet<BlockHash> = BTreeSet::new();
    let mut queue: Vec<BlockHash> = fringe_hashes.iter().copied().collect();
    let mut out: Vec<BlockFringe> = Vec::new();
    while let Some(h) = queue.pop() {
        if !seen.insert(h) {
            continue;
        }
        match dag.lookup(&h).await {
            Ok(Some(meta)) => {
                out.push(BlockFringe {
                    block_hash: h,
                    fringe: meta.fringe.clone(),
                    fringe_state_hash: meta.fringe_state_hash,
                });
                queue.extend(meta.justifications.iter().copied());
            }
            Ok(None) => log.warn(
                source,
                &format!(
                    "fringe ancestry (#139): no metadata for {} — the joiner will derive its own \
                     fringe for this block, which is the pre-#139 behaviour for it alone",
                    h.to_hex()
                ),
            ),
            Err(e) => log.warn(
                source,
                &format!(
                    "fringe ancestry (#139): lookup failed for {}: {e}",
                    h.to_hex()
                ),
            ),
        }
    }
    // Deterministic order (the message is serialized and compared in tests): by block hash.
    out.sort_by_key(|b| b.block_hash);
    out
}

pub async fn handle_finalized_fringe_request(
    transport: &dyn TransportLayer,
    conf: &RPConf,
    log: &dyn Log,
    source: LogSource,
    peer: &PeerNode,
    fringe: &FinalizedFringe,
) {
    log.info(
        source,
        &format!("Received FinalizedFringeRequest from {peer}"),
    );
    transport_layer_syntax::stream_to_peer(
        transport,
        conf,
        peer,
        FinalizedFringeSerde.mk_packet(fringe),
    )
    .await;
    log.info(source, &format!("FinalizedFringe sent to {peer}"));
}

/// **The answer to a finalized-fringe request** — this node's own latest fringe, or the block a
/// *recovery sync* named (C259, C259a).
///
/// Naming the block in the request rather than letting the joiner fetch the block and patch a seed
/// together locally is what carries the `ancestry`: the per-block fringe state of every block at and
/// below the root, which a restoring node cannot derive for itself and which #139 exists to send.
/// Without it the joiner derives its own, that derivation needs a sidecar the state transfer does not
/// include, and the node **stalls at its anchor** — silently near the tip, loudly on an older one.
///
/// `None` means "nothing to answer with". That is a shard with no finalized fringe and no genesis
/// block, and it is also **an anchor this node does not hold**: answering that one with the ordinary
/// fringe would hand the joiner a root it did not ask for — on a frozen net, the genesis block — which
/// is the substitution C259 is about. Silence is retryable and names itself in the log.
pub(crate) async fn finalized_fringe_response(
    dag: &dyn BlockDagStorage,
    block_store: &BlockStore,
    log: &dyn Log,
    log_source: LogSource,
    req: &FinalizedFringeRequest,
) -> Option<FinalizedFringe> {
    let repr = dag.get_representation().await;
    let latest_fringe_hashes: BTreeSet<BlockHash> =
        repr.latest_fringe().iter().map(|m| m.id).collect();
    let ordinary = if latest_fringe_hashes.is_empty() {
        // Fresh genesis: the shard-choice fringe is empty (no finalized messages), so the "chosen"
        // state is the genesis block (block 0). Hand it over so the syncing validator downloads
        // block 0 + its post-state (bonds) instead of ending up unbonded with an empty DAG.
        let genesis_hash = repr
            .height_map
            .get(&BlockHeight::zero())
            .and_then(|s| s.iter().next().copied());
        match genesis_hash {
            Some(genesis_hash) => {
                let genesis = match block_store.get(&[genesis_hash]).await {
                    Ok(mut v) => v.pop().flatten(),
                    Err(e) => {
                        // **A read that fails is not "there is no genesis block"** (C249's class).
                        // This branch exists so a joining validator downloads the genesis and its
                        // bonds instead of ending up unbonded with an empty DAG — which is exactly
                        // what it would do on a silent `None`.
                        log.error(
                            log_source,
                            &format!(
                                "Failed to read the genesis block {}: {e}",
                                genesis_hash.to_hex()
                            ),
                        );
                        None
                    }
                };
                genesis.map(|b| FinalizedFringe {
                    hashes: vec![genesis_hash],
                    state_hash: b.post_state_hash,
                    ancestry: Vec::new(),
                })
            }
            None => None,
        }
    } else {
        // **The latest finalised block's claim, read from that block's metadata** (Law 66/68; C250's
        // residue, C270). The `fringe-data` record this used to read retired — and with it the last
        // reason this handler needed a runtime or a replay, because a block's metadata needs neither.
        // The claim is that block's own `fringe_state_hash`; the version a reader names is the block
        // (Law 68), and the latest finalised block is the max `(height, hash)` member of the latest
        // fringe, which is a function of the fringe so every node holding it names the same block.
        //
        // The tie-break here is the same rule the reader in `multi_parent_casper.rs` uses for the
        // previous fringe's state, so the root a joiner is handed and the root a proposer starts from
        // agree.
        let mut latest: Option<(BlockHeight, BlockHash, StateHash)> = None;
        for h in &latest_fringe_hashes {
            // `lookup` is a metadata read, not a replay; a member this node cannot read is skipped
            // rather than served as a bogus state.
            if let Ok(Some(meta)) = dag.lookup(h).await {
                let candidate = (meta.block_num, *h, meta.fringe_state_hash);
                let better = latest
                    .as_ref()
                    .is_none_or(|(height, hash, _)| (candidate.0, candidate.1) > (*height, *hash));
                if better {
                    latest = Some(candidate);
                }
            }
        }
        latest.map(|(_, _, state_hash)| FinalizedFringe {
            hashes: latest_fringe_hashes.iter().copied().collect(),
            state_hash,
            ancestry: Vec::new(),
        })
    };
    // **A recovery sync names its own root** (C259a). The joiner asks for the block the operator's
    // reconciliation chose, and the answer is that block's post-state — the same pair the genesis
    // branch above hands a fresh shard — with the ancestry filled below by the same
    // `include_fringe_metadata` path. Naming it here rather than at the joiner is what carries the
    // ancestry: without it every restored block at or below the anchor had no fringe state to be
    // replayed against.
    //
    // **An anchor this node does not hold is answered with nothing, not with the ordinary fringe.**
    // Falling back would hand the joiner a root it did not ask for — and on a net with finality frozen
    // that root is the genesis block, which is precisely the substitution C259 is about: the joiner
    // would sync to block 0, reject everything after it, and never re-enter the sync path, with the
    // operator's anchor silently ignored. No answer is retryable and visible in both logs, and a sync
    // that cannot be answered is a thing the joiner already has a terminal state for (AUDIT C181).
    let response = match req.anchor {
        Some(anchor) => match block_store.get(&[anchor]).await {
            Ok(mut v) => v.pop().flatten().map(|b| FinalizedFringe {
                hashes: vec![anchor],
                state_hash: b.post_state_hash,
                ancestry: Vec::new(),
            }),
            Err(e) => {
                log.error(
                    log_source,
                    &format!(
                        "Failed to read the anchored block {} a sync asked for: {e}",
                        anchor.to_hex()
                    ),
                );
                None
            }
        },
        None => ordinary,
    };
    // **Filled only when the requester asks** (#139): a bounded addition to a message that already
    // exists, so a requester or responder that does not know the field simply gets the pre-#139
    // exchange rather than a failed sync.
    match response {
        Some(mut f) => {
            if req.include_fringe_metadata {
                let hashes: BTreeSet<BlockHash> = f.hashes.iter().copied().collect();
                f.ancestry = collect_fringe_ancestry(dag, &hashes, log, log_source).await;
                log.info(
                    log_source,
                    &format!(
                        "Included {} block fringe(s) in the fringe response (#139).",
                        f.ancestry.len()
                    ),
                );
            }
            Some(f)
        }
        None => None,
    }
}

/// Serve a peer's store-items (state-sync) request from the exporter (port of
/// `handleStoreItemsRequest`), unless the operator disabled the exporter.
///
/// `disable_state_exporter` is the one thing that turns this node into a non-server of state: the
/// Scala gates the whole handler on it (`NodeRunning.scala:314-320`) after logging
/// "the node is configured to not respond to StoreItemsMessage", and does nothing else. The flag's
/// point is an operator's choice about who may pull this node's trie (a state-exfiltration and
/// bandwidth-amplification surface), so the refusal must be *before* the walk and before any send.
#[allow(clippy::too_many_arguments)]
pub async fn handle_store_items_request<E: RSpaceExporter>(
    transport: &dyn TransportLayer,
    conf: &RPConf,
    exporter: &E,
    log: &dyn Log,
    log_source: LogSource,
    peer: &PeerNode,
    req: &StoreItemsMessageRequest,
    disable_state_exporter: bool,
) {
    if disable_state_exporter {
        log.info(
            log_source,
            &format!(
                "Received StoreItemsMessage request but the node is configured to not respond to \
                 StoreItemsMessage, from {}.",
                peer.endpoint.host
            ),
        );
        return;
    }
    // Validate-on-ingress: a negative skip/take would wrap to a huge `usize` via `as` and make
    // `get_nodes` iterate the whole trie.
    let (Ok(skip), Ok(take)) = (usize::try_from(req.skip), usize::try_from(req.take)) else {
        log.info(
            log_source,
            "Dropping store-items request with negative skip/take",
        );
        return;
    };
    // Bound the walk: a peer-controlled `take` of i32::MAX would traverse and serialize the whole
    // trie (state exfiltration + CPU/IO/bandwidth amplification, repeatable per peer).
    if take > MAX_STORE_ITEMS_TAKE {
        log.info(
            log_source,
            &format!("Dropping store-items request with take {take} > {MAX_STORE_ITEMS_TAKE}"),
        );
        return;
    }
    // A store the exporter cannot read is logged and dropped, the way an over-large `take` is above:
    // this handler has no error reply to send, and serving an *empty* page would be a state claim —
    // the importer would then validate its own traversal against it (AUDIT C63).
    let nodes = match exporter.get_nodes(&req.start_path, skip, take) {
        Ok(nodes) => nodes,
        Err(e) => {
            log.error(
                log_source,
                &format!("Dropping store-items request: the trie could not be read: {e}"),
            );
            return;
        }
    };
    let history_keys: Vec<Blake2b256Hash> = nodes
        .iter()
        .filter(|n| !n.is_leaf)
        .map(|n| n.hash)
        .collect();
    let data_keys: Vec<Blake2b256Hash> =
        nodes.iter().filter(|n| n.is_leaf).map(|n| n.hash).collect();
    let history_items = match exporter.get_history_items(&history_keys, |b: &[u8]| b.to_vec()) {
        Ok(items) => items,
        Err(e) => {
            log.error(
                log_source,
                &format!("Dropping store-items request: history items unreadable: {e}"),
            );
            return;
        }
    };
    let data_items = match exporter.get_data_items(&data_keys, |b: &[u8]| b.to_vec()) {
        Ok(items) => items,
        Err(e) => {
            log.error(
                log_source,
                &format!("Dropping store-items request: data items unreadable: {e}"),
            );
            return;
        }
    };
    // The byte cap (AUDIT C73): see `MAX_STORE_ITEMS_BYTES` for why this refuses rather than truncates.
    // It is checked here, after assembly and before the response exists, so an oversized page is never
    // serialised and never sent.
    let page_bytes: usize = history_items
        .iter()
        .map(|(_, v)| v.len())
        .chain(data_items.iter().map(|(_, v)| v.len()))
        .sum();
    if page_bytes > MAX_STORE_ITEMS_BYTES {
        log.error(
            log_source,
            &format!(
                "Dropping store-items request: the page is {page_bytes} bytes > \
                 {MAX_STORE_ITEMS_BYTES} — refusing rather than truncating, since a short page is a \
                 wrong state claim (the requester recomputes it)"
            ),
        );
        return;
    }
    let last_path = nodes.last().map(|n| n.path.clone()).unwrap_or_default();

    let response = StoreItemsMessage {
        start_path: req.start_path.clone(),
        last_path,
        history_items,
        data_items,
    };
    log.info(
        log_source,
        &format!(
            "Sending {} history and {} data store items to {}",
            response.history_items.len(),
            response.data_items.len(),
            peer.endpoint.host
        ),
    );
    // Stream the response (matching Scala's `streamToPeer`); the unary `send_to_peer` path does
    // not deliver to the syncing peer, which stalls LFS state sync.
    transport_layer_syntax::stream_to_peer(
        transport,
        conf,
        peer,
        StoreItemsMessageSerde.mk_packet(&response),
    )
    .await;
}

/// Bound on the inbound block queue. The channel is created upstream (node crate) with
/// `tokio::sync::mpsc::channel(MAX_PENDING_BLOCKS)`; `NodeRunning` only holds the bounded sender and
/// drops blocks (via `try_send`) rather than blocking the inbound task when it is full.
pub const MAX_PENDING_BLOCKS: usize = 1024;

/// How many heights one `BlockRange` answer may cover (C259's catch-up). **A window, not a limit on
/// what a node may hold**: the requester validates one window before asking for the next, so a
/// catch-up of any length stays far below `MAX_PENDING_BLOCKS` instead of saturating it. Eight heights
/// is at most 32 blocks on a four-validator shard and under a kilobyte of hashes.
pub const MAX_BLOCK_RANGE: usize = 8;

/// Whether the block store holds `hash` — the `contains` half of the receiver's presence reads.
///
/// **A store that cannot be read is an error, not "unknown".** The oracle reads presence inside `F`
/// (`BlockStore[F].contains`, `NodeRunning.scala:127,231`), so a store error is an error there and its
/// caller logs it and moves on; the port's `unwrap_or_default()` answered `false` — "unknown" — and
/// each handler acted on that negative, silently. A store answering fewer presence bits than keys is
/// refused too, rather than read as "not known".
pub(crate) async fn block_is_known(
    block_store: &BlockStore,
    hash: &BlockHash,
) -> Result<bool, String> {
    let contains = block_store.contains(&[*hash]).await?;
    contains.first().copied().ok_or_else(|| {
        format!(
            "the block store answered no presence bit for {}",
            hash.to_hex()
        )
    })
}

/// The block at `hash`, if the store holds it — the `get` half of the same reads.
///
/// `Ok(None)` is a block the store genuinely does not hold; an unreadable store is an `Err`, because
/// the oracle's `getUnsafe` (`BlockStoreSyntax.scala:33-35`) lifts both into an error in `F`.
pub(crate) async fn block_by_hash(
    block_store: &BlockStore,
    hash: &BlockHash,
) -> Result<Option<BlockMessage>, String> {
    Ok(block_store
        .get(&[*hash])
        .await?
        .into_iter()
        .flatten()
        .next())
}

/// The running-state engine (port of the `NodeRunning` class): message handling and the
/// store-items (LFS state-sync) serving are ported.
pub struct NodeRunning<E: RSpaceExporter> {
    transport: Arc<dyn TransportLayer>,
    conf: RPConf,
    block_store: BlockStore,
    dag: Arc<dyn BlockDagStorage>,
    block_retriever: Arc<BlockRetriever>,
    log: Arc<dyn Log>,
    log_source: LogSource,
    validator_id: Option<ValidatorIdentity>,
    incoming_blocks: tokio::sync::mpsc::Sender<BlockMessage>,
    block_request_limit: Arc<PeerRateLimiter>,
    exporter: E,
    /// Operator switch: refuse store-items (state-sync) requests (port of the Scala's
    /// `disableStateExporter`).
    disable_state_exporter: bool,
    /// The temporary ingest window a catch-up holds (C259). Released — admitting every height — unless
    /// a driver is walking, so an ordinary node's ingest is unchanged by this field.
    catchup: Arc<CatchupWindow>,
    /// Where a `BlockRange` answer goes: the catch-up driver owns the receiver. A full channel is not
    /// an error — it means the walk that asked has ended.
    ///
    /// **Behind a cell since #320**, because a walk can now happen more than once: a node that resets
    /// itself rewinds and walks again from the anchor it rewound to, and the second walk owns a second
    /// receiver. `apply` swaps the sender in — dropping the previous one, which is what tells the walk
    /// that ended to stop.
    block_range_tx: BlockRangeDoor,
}

/// **The catch-up walk's door** — see [`NodeRunning::block_range_tx`]. Shared with `apply`, which swaps
/// a fresh sender in once per running phase.
pub type BlockRangeDoor = Arc<tokio::sync::Mutex<tokio::sync::mpsc::Sender<BlockRange>>>;

impl<E: RSpaceExporter> NodeRunning<E> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        transport: Arc<dyn TransportLayer>,
        conf: RPConf,
        block_store: BlockStore,
        dag: Arc<dyn BlockDagStorage>,
        block_retriever: Arc<BlockRetriever>,
        log: Arc<dyn Log>,
        validator_id: Option<ValidatorIdentity>,
        incoming_blocks: tokio::sync::mpsc::Sender<BlockMessage>,
        exporter: E,
        disable_state_exporter: bool,
        catchup: Arc<CatchupWindow>,
        block_range_tx: BlockRangeDoor,
    ) -> Self {
        NodeRunning {
            transport,
            conf,
            block_store,
            dag,
            block_retriever,
            log,
            log_source: LogSource::new("casper.engine.NodeRunning"),
            validator_id,
            incoming_blocks,
            catchup,
            block_range_tx,
            block_request_limit: Arc::new(PeerRateLimiter::new(
                DEFAULT_BLOCK_REQUEST_LIMIT_PER_SEC,
            )),
            exporter,
            disable_state_exporter,
        }
    }

    /// Handle an incoming casper message from a peer (port of `handle`).
    pub async fn handle(&self, peer: &PeerNode, msg: &CasperMessage) {
        match msg {
            CasperMessage::BlockHashMessage(bhm) => {
                let hash = bhm.block_hash;
                let ignore = match block_is_known(&self.block_store, &hash).await {
                    Ok(known) => known,
                    Err(e) => {
                        self.log.error(
                            self.log_source,
                            &format!(
                                "Dropping a block-hash message for {}: the block store could not be read: {e}",
                                hash.to_hex()
                            ),
                        );
                        return;
                    }
                };
                handle_block_hash_message(
                    &self.block_retriever,
                    self.log.as_ref(),
                    self.log_source,
                    peer,
                    &hash,
                    ignore,
                )
                .await;
            }
            CasperMessage::BlockMessage(b) => {
                if let Some(id) = &self.validator_id {
                    if b.sender.as_bytes().as_slice() == id.public_key.bytes() {
                        self.log.warn(
                            self.log_source,
                            &format!(
                                "There is another node {peer} proposing using the same private key as you. \
                                 Or did you restart your node?"
                            ),
                        );
                    }
                }
                let known = match block_is_known(&self.block_store, &b.block_hash).await {
                    Ok(known) => known,
                    Err(e) => {
                        self.log.error(
                            self.log_source,
                            &format!(
                                "Dropping a block message for {}: the block store could not be read: {e}",
                                b.block_hash.to_hex()
                            ),
                        );
                        return;
                    }
                };
                if known {
                    self.log.debug(
                        self.log_source,
                        &format!(
                            "Ignoring BlockMessage #{} from {}",
                            b.block_number, peer.endpoint.host
                        ),
                    );
                } else {
                    // **The catch-up window** (C259, `engine::catchup`). While a walk is catching this
                    // node up, a block above the window is one whose parents are not validated yet, and
                    // pending it is exactly what fills `MAX_PENDING_BLOCKS` and freezes the node
                    // (measured: 173 drops, one block validated, height 121 against a master's 577).
                    // This one is left **un-acked** rather than refused, so the retriever offers it
                    // again once the frontier has moved past it: a pace, not a loss.
                    if !self.catchup.admits(i64::from(b.block_number)) {
                        self.log.debug(
                            self.log_source,
                            &format!(
                                "Block #{} from {} is above the catch-up window; leaving it for the \
                                 retriever",
                                b.block_number, peer.endpoint.host
                            ),
                        );
                        return;
                    }
                    match self.incoming_blocks.try_send(b.clone()) {
                        Ok(()) => {
                            self.log.debug(
                                self.log_source,
                                &format!(
                                    "Incoming BlockMessage #{} from {}",
                                    b.block_number, peer.endpoint.host
                                ),
                            );
                        }
                        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                            // **No silent loss** (F-U8-02): the retriever already knows how to ask a
                            // peer for a hash again, and the block stays unacknowledged until it is
                            // queued. Re-admit it instead of dropping it.
                            self.log.warn(
                                self.log_source,
                                &format!(
                                    "Block ingress queue full; re-requesting block {} from {peer}",
                                    b.block_hash.to_hex()
                                ),
                            );
                            let _ = self
                                .block_retriever
                                .admit_hash(
                                    &b.block_hash,
                                    Some(peer),
                                    AdmitHashReason::BlockReceived,
                                )
                                .await;
                        }
                        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                            self.log.warn(
                                self.log_source,
                                &format!(
                                    "Block ingress queue closed; dropping block {} from {peer}",
                                    b.block_hash.to_hex()
                                ),
                            );
                        }
                    }
                }
            }
            CasperMessage::BlockRequest(br) => {
                handle_block_request(
                    self.transport.as_ref(),
                    &self.conf,
                    &self.block_store,
                    self.log.as_ref(),
                    self.log_source,
                    peer,
                    br,
                    self.block_request_limit.as_ref(),
                )
                .await;
            }
            CasperMessage::HasBlockRequest(hbr) => {
                let repr = self.dag.get_representation().await;
                let hash = hbr.hash;
                let has_block = repr.contains(&hash);
                handle_has_block_request(
                    self.transport.as_ref(),
                    &self.conf,
                    self.log.as_ref(),
                    self.log_source,
                    peer,
                    hbr,
                    has_block,
                )
                .await;
            }
            CasperMessage::HasBlock(hb) => {
                let hash = hb.hash;
                let known = match block_is_known(&self.block_store, &hash).await {
                    Ok(known) => known,
                    Err(e) => {
                        self.log.error(
                            self.log_source,
                            &format!(
                                "Dropping a has-block message for {}: the block store could not be read: {e}",
                                hash.to_hex()
                            ),
                        );
                        return;
                    }
                };
                if known {
                    let validated = match not_validated(&self.block_store, self.dag.as_ref(), &hash)
                        .await
                    {
                        Ok(validated) => validated,
                        Err(e) => {
                            self.log.error(
                                self.log_source,
                                &format!("Dropping a has-block message for {}: {e}", hash.to_hex()),
                            );
                            return;
                        }
                    };
                    if validated {
                        let stored = match block_by_hash(&self.block_store, &hash).await {
                            Ok(block) => block,
                            Err(e) => {
                                self.log.error(
                                    self.log_source,
                                    &format!(
                                        "Dropping a has-block message for {}: the block store could not be read: {e}",
                                        hash.to_hex()
                                    ),
                                );
                                return;
                            }
                        };
                        if let Some(block) = stored {
                            match self.incoming_blocks.try_send(block) {
                                Ok(()) => {}
                                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                                    self.log.warn(
                                        self.log_source,
                                        &format!(
                                            "Block ingress queue full; re-requesting block {} from {peer}",
                                            hash.to_hex()
                                        ),
                                    );
                                    let _ = self
                                        .block_retriever
                                        .admit_hash(
                                            &hash,
                                            Some(peer),
                                            AdmitHashReason::BlockReceived,
                                        )
                                        .await;
                                }
                                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                    self.log.warn(
                                        self.log_source,
                                        &format!(
                                            "Block ingress queue closed; dropping block {} from {peer}",
                                            hash.to_hex()
                                        ),
                                    );
                                }
                            }
                        }
                    }
                } else {
                    self.log.debug(
                        self.log_source,
                        &format!(
                            "Incoming HasBlockMessage {} from {}",
                            hash.to_hex(),
                            peer.endpoint.host
                        ),
                    );
                    let _ = self
                        .block_retriever
                        .admit_hash(&hash, Some(peer), AdmitHashReason::HasBlockMessageReceived)
                        .await;
                }
            }
            CasperMessage::ForkChoiceTipRequest(_) => {
                handle_fork_choice_tip_request(
                    self.transport.as_ref(),
                    &self.conf,
                    self.dag.as_ref(),
                    self.log.as_ref(),
                    self.log_source,
                    peer,
                )
                .await;
            }
            CasperMessage::BlockRangeRequest(req) => {
                handle_block_range_request(
                    self.transport.as_ref(),
                    &self.conf,
                    self.dag.as_ref(),
                    self.log.as_ref(),
                    self.log_source,
                    peer,
                    req.clone(),
                )
                .await;
            }
            CasperMessage::BlockRange(answer) => {
                // The answer to a window this node asked for: hand it to the catch-up driver, which
                // owns the receiver. A closed or full channel is not an error — it means the walk that
                // asked for it has ended, and the answer is then simply stale.
                // Logged at `info` rather than `debug`: the only traffic on this message is a
                // catch-up's own windows, so the line is rare by construction, and "the answer arrived
                // but the walk never moved" is otherwise invisible from outside — which is exactly the
                // shape a wiring mistake takes (found by running it, 2026-10-10).
                self.log.info(
                    self.log_source,
                    &format!(
                        "received a block-range answer {}..={} with {} hash(es), tip {}",
                        answer.from,
                        answer.to,
                        answer.hashes.len(),
                        answer.tip
                    ),
                );
                // The door is read per answer rather than held, because `apply` swaps it when a walk
                // ends (a full channel is not an error: it means nobody is walking).
                if self
                    .block_range_tx
                    .lock()
                    .await
                    .try_send(answer.clone())
                    .is_err()
                {
                    self.log.warn(
                        self.log_source,
                        "a block-range answer arrived with no catch-up waiting for it",
                    );
                }
            }
            CasperMessage::FinalizedFringeRequest(req) => {
                // The response construction — the peer's own fringe, or the block a recovery sync
                // named, with the ancestry either way — is a free function of its own so the choice
                // has a test that does not need a whole engine (C259a).
                if let Some(fringe_response) = finalized_fringe_response(
                    self.dag.as_ref(),
                    &self.block_store,
                    self.log.as_ref(),
                    self.log_source,
                    req,
                )
                .await
                {
                    handle_finalized_fringe_request(
                        self.transport.as_ref(),
                        &self.conf,
                        self.log.as_ref(),
                        self.log_source,
                        peer,
                        &fringe_response,
                    )
                    .await;
                    self.log.info(
                        self.log_source,
                        &format!(
                            "Sent fringe response ({}).",
                            fringe_response
                                .hashes
                                .iter()
                                .map(|h| h.to_hex())
                                .collect::<Vec<_>>()
                                .join(" ")
                        ),
                    );
                }
            }
            CasperMessage::StoreItemsMessageRequest(req) => {
                handle_store_items_request(
                    self.transport.as_ref(),
                    &self.conf,
                    &self.exporter,
                    self.log.as_ref(),
                    self.log_source,
                    peer,
                    req,
                    self.disable_state_exporter,
                )
                .await;
            }
            CasperMessage::StoreItemsMessage(_) => {}
            CasperMessage::FinalizedFringe(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// **A store that cannot be read is not "the block is not known"** (AUDIT C67).
    ///
    /// The oracle reads presence inside `F` (`BlockStore[F].contains`, `NodeRunning.scala:127,231`),
    /// so a store error is an error there; the port's `unwrap_or_default()` answered `false` — "not
    /// known" — and the *consequence* differs per handler (a duplicate block admitted, a block
    /// re-saved, a response never sent), while the silence is the same everywhere: nothing tells the
    /// operator the block store is failing.
    ///
    /// Falsifier, both forms. Pre-fix (witnessing): the failing store was answered with `false` and
    /// `None`, and those assertions **passed on exactly that** (run 2026-09-24 before the change).
    /// Post-fix: both are `Err`, naming the failure.
    #[tokio::test]
    async fn a_store_that_cannot_be_read_is_not_an_unknown_block() {
        let block_store: BlockStore = Arc::new(FailingBlockStore);
        let hash = BlockHash::new([0x11; 32]);

        let err = block_is_known(&block_store, &hash)
            .await
            .expect_err("a block store that cannot be read must not answer \"unknown block\"");
        assert!(
            err.contains("the block store is down"),
            "and the refusal must name the failure, got: {err}"
        );
        let err = block_by_hash(&block_store, &hash)
            .await
            .expect_err("…nor \"no block\" for the read that returns it");
        assert!(
            err.contains("the block store is down"),
            "and the refusal must name the failure, got: {err}"
        );
    }
    use async_trait::async_trait;
    use rchain_block_storage::dag::codecs::{BlockHashCodec, BlockMessageCodec};
    use rchain_comm::errors::CommErr;
    use rchain_comm::peer_node::NodeIdentifier;
    use rchain_comm::rp::rp_conf::{ClearConnectionsConf, RPConf};
    use rchain_comm::transport::chunker::Blob;
    use rchain_comm::transport::transport_layer::TransportLayer;
    use rchain_models::comm::protocol::Protocol;
    use rchain_models::validator::Validator;
    use rchain_shared::log::NopLog;
    use rchain_shared::store::InMemoryKeyValueStore;
    use rchain_shared::typed_store::KeyValueTypedStoreCodec;
    use std::collections::{BTreeMap, BTreeSet};
    use std::time::Duration;

    use crate::protocol::comm_util::{CommUtil, ConnectionsCell};

    fn peer(name: &str, port: u16) -> PeerNode {
        PeerNode::from(
            NodeIdentifier::new(name.as_bytes().to_vec()),
            "host".to_string(),
            rchain_shared::refined::Port::new(port),
            rchain_shared::refined::Port::new(port),
        )
    }

    fn hash(byte: u8) -> BlockHash {
        BlockHash::new([byte; 32])
    }

    fn block(hash: BlockHash) -> BlockMessage {
        BlockMessage {
            version: 1,
            shard_id: "root".to_string(),
            block_hash: hash,
            block_number: 0.try_into().unwrap(),
            sender: Validator::new([1u8; 65]),
            seq_num: 0.try_into().unwrap(),
            pre_state_hash: StateHash::new([0u8; 32]),
            post_state_hash: StateHash::new([0u8; 32]),
            justifications: vec![],
            bonds: BTreeMap::new(),
            rejected_deploys: BTreeSet::new(),
            rejected_blocks: BTreeSet::new(),
            rejected_senders: BTreeSet::new(),
            state: rchain_models::casper::protocol::casper_message::RholangState::default(),
            sig_algorithm: "secp256k1".to_string(),
            sig: vec![],
            timestamp: 0,
        }
    }

    async fn block_store(blocks: Vec<BlockMessage>) -> BlockStore {
        let store: BlockStore = Arc::new(KeyValueTypedStoreCodec::new(
            Arc::new(tokio::sync::Mutex::new(Box::new(
                InMemoryKeyValueStore::default(),
            ))),
            Arc::new(BlockHashCodec),
            Arc::new(BlockMessageCodec),
        ));
        let pairs: Vec<(BlockHash, BlockMessage)> =
            blocks.into_iter().map(|b| (b.block_hash, b)).collect();
        store.put(&pairs).await.unwrap();
        store
    }

    fn conf(local: &PeerNode) -> RPConf {
        RPConf {
            local: local.clone(),
            network_id: "testnet".to_string(),
            bootstrap: None,
            default_timeout: Duration::from_secs(10),
            max_num_of_connections: 10,
            clear_connections: ClearConnectionsConf {
                num_of_connections_pinged: 10,
            },
        }
    }

    #[derive(Default)]
    struct MockTransport {
        sends: std::sync::Mutex<Vec<(PeerNode, Protocol)>>,
        streams: std::sync::Mutex<Vec<(Vec<PeerNode>, Blob)>>,
    }

    #[async_trait]
    impl TransportLayer for MockTransport {
        async fn send(&self, peer: &PeerNode, msg: Protocol) -> CommErr<()> {
            self.sends.lock().unwrap().push((peer.clone(), msg));
            Ok(())
        }
        async fn broadcast(&self, peers: &[PeerNode], msg: Protocol) -> Vec<CommErr<()>> {
            for peer in peers {
                self.sends.lock().unwrap().push((peer.clone(), msg.clone()));
            }
            peers.iter().map(|_| Ok(())).collect()
        }
        async fn stream(&self, peers: &[PeerNode], blob: Blob) {
            self.streams.lock().unwrap().push((peers.to_vec(), blob));
        }
    }

    /// A store-items server for the gate test: one leaf and one internal node, so the handler has
    /// something to send when it is allowed to.
    struct MockExporter;

    impl rchain_shared::state::TrieExporter<Blake2b256Hash> for MockExporter {
        fn get_nodes(
            &self,
            _start_path: &[(Blake2b256Hash, Option<u8>)],
            _skip: usize,
            _take: usize,
        ) -> Result<Vec<rchain_shared::state::TrieNode<Blake2b256Hash>>, String> {
            let leaf = Blake2b256Hash::from_bytes([7u8; 32]);
            let branch = Blake2b256Hash::from_bytes([8u8; 32]);
            Ok(vec![
                rchain_shared::state::TrieNode {
                    hash: leaf,
                    is_leaf: true,
                    path: vec![],
                },
                rchain_shared::state::TrieNode {
                    hash: branch,
                    is_leaf: false,
                    path: vec![(branch, None)],
                },
            ])
        }

        fn get_history_items<Value>(
            &self,
            keys: &[Blake2b256Hash],
            from_buffer: impl Fn(&[u8]) -> Value,
        ) -> Result<Vec<(Blake2b256Hash, Value)>, String> {
            Ok(keys.iter().map(|k| (*k, from_buffer(&[1u8]))).collect())
        }

        fn get_data_items<Value>(
            &self,
            keys: &[Blake2b256Hash],
            from_buffer: impl Fn(&[u8]) -> Value,
        ) -> Result<Vec<(Blake2b256Hash, Value)>, String> {
            Ok(keys.iter().map(|k| (*k, from_buffer(&[2u8]))).collect())
        }
    }

    impl RSpaceExporter for MockExporter {
        fn get_root(&self) -> Result<Option<Blake2b256Hash>, String> {
            Ok(Some(Blake2b256Hash::from_bytes([8u8; 32])))
        }
    }

    /// A store-items server whose page is `count` nodes of `payload` bytes each — what an LFS
    /// state-sync request gets from a real trie exporter.
    struct PageExporter {
        count: usize,
        payload: usize,
    }

    impl PageExporter {
        fn node(&self, i: usize) -> rchain_shared::state::TrieNode<Blake2b256Hash> {
            let mut bytes = [0u8; 32];
            bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
            let hash = Blake2b256Hash::from_bytes(bytes);
            rchain_shared::state::TrieNode {
                hash,
                is_leaf: i % 2 == 0,
                path: vec![(hash, None)],
            }
        }

        fn items<Value>(
            &self,
            count: usize,
            keys: &[Blake2b256Hash],
            from_buffer: impl Fn(&[u8]) -> Value,
        ) -> Result<Vec<(Blake2b256Hash, Value)>, String> {
            let bytes = vec![1u8; self.payload];
            Ok(keys
                .iter()
                .take(count)
                .map(|k| (*k, from_buffer(&bytes)))
                .collect())
        }
    }

    impl rchain_shared::state::TrieExporter<Blake2b256Hash> for PageExporter {
        fn get_nodes(
            &self,
            _start_path: &[(Blake2b256Hash, Option<u8>)],
            _skip: usize,
            take: usize,
        ) -> Result<Vec<rchain_shared::state::TrieNode<Blake2b256Hash>>, String> {
            Ok((0..take.min(self.count)).map(|i| self.node(i)).collect())
        }

        fn get_history_items<Value>(
            &self,
            keys: &[Blake2b256Hash],
            from_buffer: impl Fn(&[u8]) -> Value,
        ) -> Result<Vec<(Blake2b256Hash, Value)>, String> {
            self.items(self.count, keys, from_buffer)
        }

        fn get_data_items<Value>(
            &self,
            keys: &[Blake2b256Hash],
            from_buffer: impl Fn(&[u8]) -> Value,
        ) -> Result<Vec<(Blake2b256Hash, Value)>, String> {
            self.items(self.count, keys, from_buffer)
        }
    }

    impl RSpaceExporter for PageExporter {
        fn get_root(&self) -> Result<Option<Blake2b256Hash>, String> {
            Ok(Some(Blake2b256Hash::from_bytes([8u8; 32])))
        }
    }

    /// **A measurement, not a tripwire** (AUDIT C61): what one store-items page costs the shard's
    /// dispatch loop.
    ///
    /// `handle_store_items_request` is awaited *inline* — `node_launch.rs` serves a shard's messages
    /// from one `while let Some(pm) = packet_rx.recv().await { engine.handle(..).await }` loop, and
    /// this handler walks the exporter, materialises the page, serialises it and hands it to the
    /// transport before returning (`node_running.rs`). Nothing else for that shard — a block, a
    /// fork-choice tip, another peer's request — is handled meanwhile, and the routing loop above it
    /// (`node_runtime.rs`'s `for tx in targets { tx.send(..).await }` into a 50-deep channel)
    /// backpressures on the same stall. The plan records this rather than fixing it: moving the
    /// handler off the loop is a behaviour change, and a bounded page is the cheaper answer.
    ///
    /// Measured here in a debug test build, against a page whose items are 4 KiB (a node with
    /// children), with the *response* the handler produced (so the number is the page's, not a
    /// synthetic chunker call): an LFS page (`PAGE_SIZE = 750`) and the largest page the cap admits
    /// (`MAX_STORE_ITEMS_TAKE = 10_000`). `--nocapture` prints both. The gRPC write itself is not
    /// included — `MockTransport` records the blob — so this is the shard-side cost, which is the
    /// part that holds the loop.
    #[tokio::test]
    async fn a_store_items_page_costs_the_dispatch_loop_this_long() {
        const ITEM_BYTES: usize = 4096;

        // The second case is the largest page the *byte* cap admits at this item size, not the largest
        // `take`: 10,000 nodes of 4 KiB is ~41 MB, which `MAX_STORE_ITEMS_BYTES` now refuses outright
        // rather than truncating (AUDIT C73, and its own test). Measuring a refused page would measure
        // nothing, so this is 6,000 nodes ≈ 24 MB — the biggest loop turn a peer can actually buy.
        for (label, nodes) in [("LFS page", 750usize), ("byte-capped page", 6_000)] {
            let local = peer("src", 40400);
            let remote = peer("peer", 40400);
            let transport = Arc::new(MockTransport::default());
            let exporter = PageExporter {
                count: nodes,
                payload: ITEM_BYTES,
            };

            let started = std::time::Instant::now();
            handle_store_items_request(
                transport.as_ref(),
                &conf(&local),
                &exporter,
                &NopLog,
                LogSource::new("test"),
                &remote,
                &StoreItemsMessageRequest {
                    start_path: vec![],
                    skip: 0,
                    take: i32::try_from(nodes).expect("nodes fit i32"),
                },
                false,
            )
            .await;
            let served = started.elapsed();

            let streams = transport.streams.lock().unwrap();
            let blob = &streams.last().expect("the page was streamed").1;
            let page_bytes = blob.packet.content.len();
            // The measured page is the page: one item per requested node, `ITEM_BYTES` each plus
            // the keys and the protobuf framing.
            assert!(
                page_bytes >= nodes * ITEM_BYTES,
                "{label}: served {page_bytes} B for {nodes} nodes, expected at least {}",
                nodes * ITEM_BYTES
            );
            // Chunking is part of the same loop turn (`stream_to_peer` → `TransportLayer::stream`
            // → `chunk_it`). At the configured stream limit (256 MiB) this page is one data chunk,
            // so this is what the page's copy costs inside the loop.
            let chunked = std::time::Instant::now();
            let chunks = rchain_comm::transport::chunker::chunk_it(
                "testnet",
                blob,
                usize::try_from(268_435_456i64).expect("fits usize"),
            )
            .expect("chunks");
            let chunking = chunked.elapsed();
            assert_eq!(chunks.len(), 2, "{label}: header + one data chunk");
            println!(
                "{label}: {nodes} nodes, {page_bytes} B served in {served:?} + chunked in \
                 {chunking:?} — one dispatch loop turn"
            );
        }
    }

    /// A store-items request is answered when the operator has left the exporter enabled.
    ///
    /// The two tests below are the pair the Scala's flag is for (`NodeRunning.scala:314-320`): the
    /// node either serves its trie to a peer or refuses before touching it. The refusal has to be
    /// *before* the walk and before any send — a node that refuses after traversing has already
    /// spent the bandwidth the flag exists to save.
    #[tokio::test]
    async fn store_items_request_is_served_when_the_exporter_is_enabled() {
        let local = peer("src", 40400);
        let remote = peer("peer", 40400);
        let transport = Arc::new(MockTransport::default());

        handle_store_items_request(
            transport.as_ref(),
            &conf(&local),
            &MockExporter,
            &NopLog,
            LogSource::new("test"),
            &remote,
            &StoreItemsMessageRequest {
                start_path: vec![],
                skip: 0,
                take: 10,
            },
            false,
        )
        .await;

        let streams = transport.streams.lock().unwrap();
        assert_eq!(streams.len(), 1, "one state-sync response");
        assert_eq!(streams[0].0, vec![remote.clone()]);
        assert_eq!(streams[0].1.packet.type_id, "StoreItemsMessage");
    }

    /// …and is refused, silently, when the operator has disabled the exporter — no response at all.
    #[tokio::test]
    async fn store_items_request_is_refused_when_the_exporter_is_disabled() {
        let local = peer("src", 40400);
        let remote = peer("peer", 40400);
        let transport = Arc::new(MockTransport::default());

        handle_store_items_request(
            transport.as_ref(),
            &conf(&local),
            &MockExporter,
            &NopLog,
            LogSource::new("test"),
            &remote,
            &StoreItemsMessageRequest {
                start_path: vec![],
                skip: 0,
                take: 10,
            },
            true,
        )
        .await;

        assert!(
            transport.streams.lock().unwrap().is_empty(),
            "a node with `disable-state-exporter` must not stream its trie to a peer"
        );
    }

    #[tokio::test]
    async fn handle_block_request_streams_block_when_present() {
        let local = peer("src", 40400);
        let remote = peer("peer", 40400);
        let transport = Arc::new(MockTransport::default());
        let h = hash(1);
        let store = block_store(vec![block(h)]).await;
        let log = NopLog;

        handle_block_request(
            transport.as_ref(),
            &conf(&local),
            &store,
            &log,
            LogSource::new("test"),
            &remote,
            &BlockRequest { hash: h },
            &PeerRateLimiter::new(100),
        )
        .await;

        let streams = transport.streams.lock().unwrap();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].0, vec![remote.clone()]);
        assert_eq!(streams[0].1.packet.type_id, "BlockMessage");
    }

    #[tokio::test]
    async fn handle_block_request_does_not_stream_absent_block() {
        let local = peer("src", 40400);
        let remote = peer("peer", 40400);
        let transport = Arc::new(MockTransport::default());
        let store = block_store(vec![]).await;
        let log = NopLog;

        handle_block_request(
            transport.as_ref(),
            &conf(&local),
            &store,
            &log,
            LogSource::new("test"),
            &remote,
            &BlockRequest { hash: hash(1) },
            &PeerRateLimiter::new(100),
        )
        .await;

        assert!(transport.streams.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn handle_has_block_request_sends_has_block_when_present() {
        let local = peer("src", 40400);
        let remote = peer("peer", 40400);
        let transport = Arc::new(MockTransport::default());

        handle_has_block_request(
            transport.as_ref(),
            &conf(&local),
            &NopLog,
            LogSource::new("test"),
            &remote,
            &HasBlockRequest { hash: hash(1) },
            true,
        )
        .await;

        let sends = transport.sends.lock().unwrap();
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0].0, remote);
        let packet = rchain_comm::rp::protocol_helper::to_packet(&sends[0].1).unwrap();
        assert_eq!(packet.type_id, "HasBlock");
    }

    #[tokio::test]
    async fn handle_has_block_request_sends_nothing_when_absent() {
        let local = peer("src", 40400);
        let remote = peer("peer", 40400);
        let transport = Arc::new(MockTransport::default());

        handle_has_block_request(
            transport.as_ref(),
            &conf(&local),
            &NopLog,
            LogSource::new("test"),
            &remote,
            &HasBlockRequest { hash: hash(1) },
            false,
        )
        .await;

        assert!(transport.sends.lock().unwrap().is_empty());
    }

    /// An empty DAG over in-memory stores — the argument `NodeRunning::new` requires and what the
    /// `HasBlock`/`HasBlockRequest` arms read (`repr.contains(hash)`). `casper/src/dag.rs`'s tests build
    /// the same storage for themselves; a `#[cfg(test)]` helper in another module is not reachable
    /// here, so it is duplicated rather than widened into a production constructor.
    async fn build_dag() -> Arc<dyn BlockDagStorage> {
        use crate::block_metadata_store::BlockMetadataStore;
        use rchain_block_storage::dag::codecs::{BlockMetadataCodec, SignedDeployDataCodec};
        use rchain_models::casper::protocol::casper_message::SignedDeployData;
        use rchain_shared::typed_store::{BytesCodec, KeyValueTypedStore, SharedStore};

        let fresh = || -> SharedStore {
            Arc::new(tokio::sync::Mutex::new(Box::new(
                InMemoryKeyValueStore::default(),
            )))
        };
        let metadata = Arc::new(
            BlockMetadataStore::create(Arc::new(KeyValueTypedStoreCodec::new(
                fresh(),
                Arc::new(BlockHashCodec),
                Arc::new(BlockMetadataCodec),
            )))
            .await
            .expect("metadata store"),
        );
        type DeployId = rchain_block_storage::dag::dag_storage::DeployId;
        let deploy_index: Arc<dyn KeyValueTypedStore<DeployId, BlockHash>> = Arc::new(
            KeyValueTypedStoreCodec::new(fresh(), Arc::new(BytesCodec), Arc::new(BlockHashCodec)),
        );
        let deploy_store: Arc<dyn KeyValueTypedStore<DeployId, SignedDeployData>> =
            Arc::new(KeyValueTypedStoreCodec::new(
                fresh(),
                Arc::new(BytesCodec),
                Arc::new(SignedDeployDataCodec),
            ));
        Arc::new(
            crate::dag::BlockDagKeyValueStorage::create(metadata, deploy_index, deploy_store)
                .await
                .expect("dag storage"),
        )
    }

    /// **A block above the catch-up window is left for the retriever, not pended** (C259).
    ///
    /// This is the gate that keeps the pending set from filling with a whole gap: while a walk is
    /// catching a node up, anything above `frontier + window` is left **un-acked** — the retriever
    /// offers it again once the frontier has moved past, so nothing is lost — and the control below
    /// shows the same node queueing a block *inside* its window, so the assertion is about the height
    /// and not about the message being ignored for some other reason.
    #[tokio::test]
    async fn a_block_above_the_catchup_window_is_left_for_the_retriever_and_one_inside_is_queued() {
        let local = peer("src", 40400);
        let remote = peer("peer", 40400);
        let transport = Arc::new(MockTransport::default());
        let connections: ConnectionsCell = Arc::new(tokio::sync::RwLock::new(Vec::new()));
        let comm_util = Arc::new(CommUtil::new(
            transport.clone(),
            conf(&local),
            connections,
            Arc::new(NopLog),
        ));
        let retriever = Arc::new(BlockRetriever::new(comm_util, Arc::new(NopLog)));
        let store = block_store(vec![]).await;
        let (incoming_tx, mut incoming_rx) = tokio::sync::mpsc::channel(4);
        let (block_range_tx, _block_range_rx) = tokio::sync::mpsc::channel(4);
        // The door, as `apply` installs it: these tests never walk, so the receiver is dropped here.
        let block_range_tx = Arc::new(tokio::sync::Mutex::new(block_range_tx));
        let window = Arc::new(CatchupWindow::new(crate::engine::catchup::WINDOW_HEIGHTS));

        let running = NodeRunning::new(
            transport.clone(),
            conf(&local),
            store.clone(),
            build_dag().await,
            retriever,
            Arc::new(NopLog),
            None,
            incoming_tx,
            MockExporter,
            false,
            window.clone(),
            block_range_tx,
        );

        // A block far above the node's frontier: the gap, not the window.
        let far = chain_block(9, 500, &[]);
        window.hold(100); // admits up to 108
        running
            .handle(&remote, &CasperMessage::BlockMessage(far.clone()))
            .await;
        assert!(
            incoming_rx.try_recv().is_err(),
            "height 500 is above a window held at 100 and must not be pended"
        );

        // The control: a block *inside* the window is queued as usual, so the drop above is about the
        // height and not about the message, the sender or the store.
        let inside = chain_block(8, 105, &[]);
        running
            .handle(&remote, &CasperMessage::BlockMessage(inside.clone()))
            .await;
        let taken = incoming_rx
            .try_recv()
            .expect("a block inside the catch-up window is queued");
        assert_eq!(taken.block_hash, inside.block_hash);

        // Released, the same far block is queued: the gate is a pace, not a filter.
        window.release();
        running
            .handle(&remote, &CasperMessage::BlockMessage(far.clone()))
            .await;
        let taken = incoming_rx
            .try_recv()
            .expect("with the window released the gap block is ordinary ingress again");
        assert_eq!(taken.block_hash, far.block_hash);
    }

    /// **F-U8-02: a full ingress queue re-requests instead of dropping.** With a one-slot queue, the
    /// second unseen block cannot be pended; the handler must hand its hash back to the retriever,
    /// which asks the peer for it. The test observes that request on the mock transport.
    #[tokio::test]
    async fn a_full_ingress_queue_re_requests_the_block() {
        let local = peer("src", 40400);
        let remote = peer("peer", 40400);
        let transport = Arc::new(MockTransport::default());
        let connections: ConnectionsCell = Arc::new(tokio::sync::RwLock::new(Vec::new()));
        let comm_util = Arc::new(CommUtil::new(
            transport.clone(),
            conf(&local),
            connections,
            Arc::new(NopLog),
        ));
        let retriever = Arc::new(BlockRetriever::new(comm_util, Arc::new(NopLog)));
        let store = block_store(vec![]).await;
        let (incoming_tx, _incoming_rx) = tokio::sync::mpsc::channel(1);
        let (block_range_tx, _block_range_rx) = tokio::sync::mpsc::channel(4);
        let block_range_tx = Arc::new(tokio::sync::Mutex::new(block_range_tx));
        let running = NodeRunning::new(
            transport.clone(),
            conf(&local),
            store,
            build_dag().await,
            retriever,
            Arc::new(NopLog),
            None,
            incoming_tx,
            MockExporter,
            false,
            Arc::new(CatchupWindow::new(crate::engine::catchup::WINDOW_HEIGHTS)),
            block_range_tx,
        );

        let first = block(hash(11));
        let second = block(hash(12));
        running
            .handle(&remote, &CasperMessage::BlockMessage(first.clone()))
            .await;
        // Leave the one slot occupied: the second block must take the re-request path.
        running
            .handle(&remote, &CasperMessage::BlockMessage(second.clone()))
            .await;
        let sends = transport.sends.lock().unwrap();
        assert!(
            sends.iter().any(|(peer, _)| *peer == remote),
            "a full queue must ask the peer to re-offer the block: {sends:?}"
        );
    }

    /// **`NodeRunning::handle`'s dispatch, and the hand-offs it owns.** Every arm calls a free handler
    /// that has its own test, so what is uncovered is the *routing* — which arm a message takes — and
    /// the two hand-offs into the node's own machinery: an unseen block goes into `incoming_blocks`,
    /// and an unseen hash goes to the retriever. A mis-routed message is a consensus-level bug, and
    /// the store guards this pins are the ones AUDIT C67 added (a read that cannot answer must drop
    /// the message *with a reason*, not act on a false negative).
    #[tokio::test]
    async fn handle_routes_each_message_and_hands_off_to_the_right_queue() {
        let local = peer("src", 40400);
        let remote = peer("peer", 40400);
        let transport = Arc::new(MockTransport::default());
        let connections: ConnectionsCell = Arc::new(tokio::sync::RwLock::new(Vec::new()));
        let comm_util = Arc::new(CommUtil::new(
            transport.clone(),
            conf(&local),
            connections,
            Arc::new(NopLog),
        ));
        let retriever = Arc::new(BlockRetriever::new(comm_util, Arc::new(NopLog)));

        let known = block(hash(1));
        let unseen = block(hash(2));
        let store = block_store(vec![known.clone()]).await;
        let (incoming_tx, mut incoming_rx) = tokio::sync::mpsc::channel(4);
        let (block_range_tx, _block_range_rx) = tokio::sync::mpsc::channel(4);
        // The door, as `apply` installs it: these tests never walk, so the receiver is dropped here.
        let block_range_tx = Arc::new(tokio::sync::Mutex::new(block_range_tx));

        let running = NodeRunning::new(
            transport.clone(),
            conf(&local),
            store.clone(),
            build_dag().await,
            retriever,
            Arc::new(NopLog),
            None,
            incoming_tx,
            MockExporter,
            false,
            Arc::new(CatchupWindow::new(crate::engine::catchup::WINDOW_HEIGHTS)),
            block_range_tx,
        );

        // An unseen block is handed to the block-processing queue — this is the node's ingress.
        running
            .handle(&remote, &CasperMessage::BlockMessage(unseen.clone()))
            .await;
        let taken = incoming_rx.try_recv().expect("the unseen block is queued");
        assert_eq!(
            taken.block_hash, unseen.block_hash,
            "and it is the block that arrived"
        );

        // A block we already have is *not* queued: the same message twice must not process twice.
        running
            .handle(&remote, &CasperMessage::BlockMessage(known.clone()))
            .await;
        assert!(
            incoming_rx.try_recv().is_err(),
            "a known block is dropped rather than queued again"
        );

        // An unseen *hash* goes to the retriever, which asks the peer for it (the free handler's own
        // behaviour, asserted here to show the arm is routed to it).
        running
            .handle(
                &remote,
                &CasperMessage::BlockHashMessage(
                    rchain_models::casper::protocol::casper_message::BlockHashMessage {
                        block_hash: hash(3),
                        block_creator: Vec::new(),
                    },
                ),
            )
            .await;
        {
            let sends = transport.sends.lock().unwrap();
            assert_eq!(sends.len(), 1, "the retriever asked for the block");
            assert_eq!(sends[0].0, remote);
        }

        // `HasBlockRequest` reads the **dag**, not the block store: an empty DAG answers nothing.
        running
            .handle(
                &remote,
                &CasperMessage::HasBlockRequest(
                    rchain_models::casper::protocol::casper_message::HasBlockRequest {
                        hash: hash(4),
                    },
                ),
            )
            .await;
        assert_eq!(
            transport.sends.lock().unwrap().len(),
            1,
            "no HasBlock answer for a hash the dag does not hold (the store has it, the dag does not)"
        );
    }

    #[tokio::test]
    async fn handle_has_block_message_requests_unknown_block_from_peer() {
        let local = peer("src", 40400);
        let remote = peer("peer", 40400);
        let transport = Arc::new(MockTransport::default());
        let connections: ConnectionsCell = Arc::new(tokio::sync::RwLock::new(Vec::new()));
        let comm_util = Arc::new(CommUtil::new(
            transport.clone(),
            conf(&local),
            connections,
            Arc::new(NopLog),
        ));
        let retriever = BlockRetriever::new(comm_util, Arc::new(NopLog));

        handle_has_block_message(
            &retriever,
            &NopLog,
            LogSource::new("test"),
            &remote,
            &hash(1),
            false,
        )
        .await;

        let sends = transport.sends.lock().unwrap();
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0].0, remote);
        let packet = rchain_comm::rp::protocol_helper::to_packet(&sends[0].1).unwrap();
        assert_eq!(packet.type_id, "BlockRequest");
    }
    /// **AUDIT C73**: a page over the byte cap is refused — dropped, never truncated.
    ///
    /// `MAX_STORE_ITEMS_TAKE` bounds how many nodes a request may name; nothing bounded how much data
    /// they carry, so the maximal `take` with fat values is tens of megabytes of the responder's memory
    /// per request, per peer. The page is now dropped rather than shortened: the requester recomputes
    /// the page it expects and requires the received keys to match (`validate_state_items`), so a short
    /// page is a wrong state claim rather than a smaller answer — the same policy this handler already
    /// applies to an unreadable store (C63) and to an over-large `take`.
    ///
    /// Both directions in one test, so neither can pass vacuously: the over-cap page must not be sent,
    /// and a legitimate page must still be served (the drop cannot be "this handler stopped working").
    /// Falsified against this tree: with the cap's check removed, the over-cap page is streamed and the
    /// first assertion fails.
    #[tokio::test]
    async fn a_page_over_the_byte_cap_is_dropped_not_truncated() {
        let local = peer("src", 40400);
        let remote = peer("peer", 40400);

        // Over the cap: 200 nodes × 200 KiB items, split about evenly into history and data.
        const FAT_NODES: usize = 200;
        const FAT_ITEM: usize = 200 * 1024;
        let fat_bytes = FAT_NODES
            .checked_mul(FAT_ITEM)
            .expect("the fixture's page size is small");
        assert!(
            fat_bytes > MAX_STORE_ITEMS_BYTES,
            "the fixture must exceed the cap ({fat_bytes} bytes), or this test proves nothing"
        );
        let transport = Arc::new(MockTransport::default());
        handle_store_items_request(
            transport.as_ref(),
            &conf(&local),
            &PageExporter {
                count: FAT_NODES,
                payload: FAT_ITEM,
            },
            &NopLog,
            LogSource::new("test"),
            &remote,
            &StoreItemsMessageRequest {
                start_path: vec![],
                skip: 0,
                take: i32::try_from(FAT_NODES).expect("nodes fit i32"),
            },
            false,
        )
        .await;
        assert!(
            transport.streams.lock().unwrap().is_empty(),
            "an over-cap page must be dropped: a truncated page is a wrong state claim, and the peer \
             cannot tell it from a real one"
        );

        // Under the cap: the largest legitimate page (`PAGE_SIZE = 750` × 4 KiB ≈ 3 MB) is served.
        let transport = Arc::new(MockTransport::default());
        handle_store_items_request(
            transport.as_ref(),
            &conf(&local),
            &PageExporter {
                count: 750,
                payload: 4096,
            },
            &NopLog,
            LogSource::new("test"),
            &remote,
            &StoreItemsMessageRequest {
                start_path: vec![],
                skip: 0,
                take: 750,
            },
            false,
        )
        .await;
        assert_eq!(
            transport.streams.lock().unwrap().len(),
            1,
            "a legitimate page must still be served — otherwise the drop above is the handler \
             refusing everything"
        );
    }

    /// A block at a height with parents, for the ancestry test below.
    fn chain_block(id: u8, height: i64, parents: &[BlockHash]) -> BlockMessage {
        let mut b = block(hash(id));
        b.block_number = height.try_into().unwrap();
        b.sender = Validator::new([id + 1; 65]);
        b.justifications = parents.to_vec();
        b.post_state_hash = StateHash::new([id; 32]);
        b
    }

    /// **A window is answered in topological order, and only as wide as this node allows.** The
    /// requester walks the gap upward from its own frontier, so the order is the contract: ascending
    /// height, every parent before its children. And the *width* is the responder's limit, not the
    /// requester's word — a peer asking for a hundred thousand heights gets a window.
    #[tokio::test]
    async fn a_block_range_request_is_answered_in_topological_order_within_the_responder_limit() {
        use rchain_models::block_metadata::BlockMetadata;

        let dag = build_dag().await;
        let g = chain_block(0, 0, &[]);
        let a = chain_block(1, 1, &[g.block_hash]);
        let anchor = chain_block(2, 2, &[a.block_hash]);
        for b in [&g, &a, &anchor] {
            dag.insert(BlockMetadata::from_block(b), b.clone())
                .await
                .expect("the block inserts");
        }

        let req = BlockRangeRequest { from: 0, to: 2 };
        let answer = block_range_response(dag.as_ref(), &NopLog, LogSource::new("test"), &req)
            .await
            .expect("a range this node holds is answered");
        assert_eq!(
            answer.hashes,
            vec![g.block_hash, a.block_hash, anchor.block_hash],
            "ascending height, parent before child"
        );
        assert_eq!((answer.from, answer.to), (0, 2));
        assert_eq!(
            answer.tip, 2,
            "and it says where its own frontier is, so a walk against a producing peer can end"
        );

        // A window above anything this node holds is an *empty answer*, not a refusal: the requester's
        // walk reads that as "the peer has nothing above my frontier" and stops.
        let above = BlockRangeRequest { from: 3, to: 5 };
        let empty = block_range_response(dag.as_ref(), &NopLog, LogSource::new("test"), &above)
            .await
            .expect("an empty window is still an answer");
        assert!(empty.hashes.is_empty());

        // The requester's `to` is a request, not a promise: the answer never exceeds one window.
        let wide = BlockRangeRequest {
            from: 0,
            to: i64::MAX,
        };
        let bounded = block_range_response(dag.as_ref(), &NopLog, LogSource::new("test"), &wide)
            .await
            .expect("a wide range is bounded, not refused");
        assert_eq!(
            bounded.to, MAX_BLOCK_RANGE as i64,
            "the responder bounds the window by its own limit"
        );
    }

    /// **A recovery sync's root, and the ancestry that makes it replayable** (C259a).
    ///
    /// A joiner with an empty DAG cannot derive the fringe state of the blocks it restores — that is a
    /// node's own recomputation and not a block field — so a restore that does not carry it stalls at
    /// its anchor. This is the responder's half: an anchored request is answered with **that block**
    /// and the per-block fringe state of its whole ancestry, rather than with this node's own latest
    /// fringe and an empty vector.
    ///
    /// **Falsifier.** With the anchored branch removed, the response falls back to the ordinary
    /// answer — here the genesis block, because this DAG holds no fringe record — so the anchor is
    /// never the root and the first assertion fails. Verified by reverting the branch (C259a).
    #[tokio::test]
    async fn an_anchored_fringe_request_is_answered_with_that_block_and_its_ancestry() {
        use rchain_models::block_metadata::BlockMetadata;

        let dag = build_dag().await;

        // A chain of three, each block's *receiver-derived* fringe state set explicitly — this is the
        // data a restoring node must be handed and cannot recompute.
        let g = chain_block(0, 0, &[]);
        let a = chain_block(1, 1, &[g.block_hash]);
        let anchor = chain_block(2, 2, &[a.block_hash]);
        for (b, fringe, state) in [
            (&g, vec![], 9u8),
            (&a, vec![g.block_hash], 8),
            (&anchor, vec![a.block_hash], 7),
        ] {
            let mut m = BlockMetadata::from_block(b);
            m.fringe = fringe.into_iter().collect();
            m.fringe_state_hash = StateHash::new([state; 32]);
            dag.insert(m, b.clone()).await.expect("the block inserts");
        }

        let store = block_store(vec![g.clone(), a.clone(), anchor.clone()]).await;
        let req = FinalizedFringeRequest {
            identifier: String::new(),
            trim_state: true,
            include_fringe_metadata: true,
            anchor: Some(anchor.block_hash),
        };
        let response =
            finalized_fringe_response(dag.as_ref(), &store, &NopLog, LogSource::new("test"), &req)
                .await
                .expect("an anchored request is answered");

        assert_eq!(
            response.hashes,
            vec![anchor.block_hash],
            "the anchor is the sync's root — not this node's own latest fringe"
        );
        assert_eq!(
            response.state_hash, anchor.post_state_hash,
            "and the state the joiner restores is the anchor's own post-state"
        );

        // The ancestry carries the anchor *and* its ancestors, each with the fringe state a restoring
        // node cannot derive. That is the whole of C259a's first half: with it a restored block is
        // replayed against the fringe its proposer used; without it the joiner derives its own, needs a
        // sidecar the state transfer does not include, and stops at the anchor.
        let carried: BTreeMap<BlockHash, (BTreeSet<BlockHash>, StateHash)> = response
            .ancestry
            .iter()
            .map(|f| (f.block_hash, (f.fringe.clone(), f.fringe_state_hash)))
            .collect();
        assert_eq!(
            carried.get(&anchor.block_hash),
            Some(&(BTreeSet::from([a.block_hash]), StateHash::new([7u8; 32]))),
            "the anchor's own fringe state travels"
        );
        assert_eq!(
            carried.get(&a.block_hash),
            Some(&(BTreeSet::from([g.block_hash]), StateHash::new([8u8; 32]))),
            "and so does its parent's — the blocks a restore has to replay, not only the root"
        );
        assert_eq!(carried.len(), 3, "g -> a -> anchor: the whole ancestry");

        // **An anchor this node does not hold is answered with nothing**, not with the ordinary
        // fringe: substituting one would send the joiner to a root it did not ask for — on a frozen
        // net, the genesis block — which is exactly the conflation C259 records. The refusal is the
        // behaviour, so it is pinned rather than left to the retry loop.
        let absent = FinalizedFringeRequest {
            anchor: Some(hash(0xEE)),
            ..req.clone()
        };
        assert!(
            finalized_fringe_response(
                dag.as_ref(),
                &store,
                &NopLog,
                LogSource::new("test"),
                &absent
            )
            .await
            .is_none(),
            "an unknown anchor must not be answered with a root the requester did not ask for"
        );
    }
}
