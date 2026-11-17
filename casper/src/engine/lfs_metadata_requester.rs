//! **The metadata-only walk** (sync/verification separation).
//!
//! A joiner does not download the chain's bodies. Starting at the checkpoint's anchor, it asks
//! connected peers for the **headers** of a frontier, learns each header's justifications, and asks
//! for those, until it reaches a block with no parents. The result is a contiguous header set the
//! certificate already made acceptable; bodies are backfilled separately.
//!
//! Three bounds make the walk safe against a silent peer:
//!
//! * a per-round timeout, after which the same frontier is re-asked (the request is idempotent);
//! * a cap on idle rounds — a walk that learns nothing for that many rounds fails **by name**,
//!   reporting how many hashes are still missing, rather than waiting for ever;
//! * a cap on total headers, so a peer that answers one request with a fabricated infinity cannot
//!   exhaust this node's memory.
//!
//! A header is not content-verifiable against its hash (`hash_block` covers `state.deploys`), so
//! this module checks only what a header can be checked for locally: the hash it was requested by,
//! the parent closure's shape, and that duplicate answers agree. Acceptability comes from the
//! certificate; the audit is what recomputes it from bodies.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

use rchain_comm::peer_node::PeerNode;
use rchain_comm::rp::rp_conf::RPConf;
use rchain_comm::transport::transport_layer::TransportLayer;
use rchain_models::block_hash::BlockHash;
use rchain_models::casper::protocol::casper_message::{
    BlockHeader, BlockMetadataMessage, BlockMetadataRequest,
};
use rchain_models::casper::protocol::packet_type_tag::ToPacket;
use rchain_shared::log::{Log, LogSource};

use crate::protocol::casper_message_protocol::BlockMetadataRequestSerde;
use crate::protocol::comm_util::CommUtil;

/// Hashes asked for in one request. A header is a few hundred bytes; a batch keeps a round trip
/// productive without making one answer exceed a packet.
const METADATA_BATCH: usize = 128;

/// Heights covered by one range request. The requester asks a tip for its ancestry in this window;
/// a peer answers at most this many (bounded by its own answer cap), and the walk continues from
/// the window's lower edge.
const METADATA_RANGE_WINDOW: i64 = 256;

/// How long one round waits for answers before re-asking the same frontier.
const METADATA_ROUND_TIMEOUT: Duration = Duration::from_secs(2);

/// Rounds with **no new header** before the walk gives up and names what is missing.
const METADATA_MAX_IDLE_ROUNDS: u32 = 6;

/// The largest history this walk will hold. A fabricated branch cannot exceed it.
const MAX_METADATA_HEADERS: usize = 2_000_000;

/// Peers asked for each frontier beyond the bootstrap (the bootstrap is always asked).
const METADATA_FANOUT: usize = 2;

/// The headers a completed walk produced, in the order they were learned.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MetadataWalk {
    pub headers: BTreeMap<BlockHash, BlockHeader>,
}

/// **Walk the header closure of `anchor`.** Returns only when every branch reaches a parentless
/// block (genesis) or the walk fails with the number of hashes still outstanding.
///
/// A mature single-parent chain would otherwise cost one round trip per generation. When the
/// anchor's height is known and above zero, the walk first asks peers for **height windows** of the
/// anchor's ancestry; a peer that does not understand the range answers nothing, and the walk falls
/// back to the hash frontier. Neither path changes acceptability: the resulting set is checked
/// against the certificate's `metadataRoot` and the walk's shape.
pub async fn request_metadata(
    anchor: BlockHash,
    anchor_height: i64,
    metadata_rx: &mut tokio::sync::mpsc::Receiver<BlockMetadataMessage>,
    transport: &dyn TransportLayer,
    conf: &RPConf,
    comm_util: &CommUtil,
    log: &dyn Log,
) -> Result<MetadataWalk, String> {
    if anchor_height > 0 {
        match range_walk(
            anchor,
            anchor_height,
            metadata_rx,
            transport,
            conf,
            comm_util,
            log,
        )
        .await?
        {
            Some(walk) => return Ok(walk),
            None => log.info(
                LogSource::new("casper.engine.LfsMetadataRequester"),
                "metadata walk: no peer answered a height range; falling back to the hash-frontier \
                 walk",
            ),
        }
    }
    hash_walk(anchor, metadata_rx, transport, conf, comm_util, log).await
}

/// **Fetch named headers only.** The closure walk follows justifications from the anchor, so it
/// never reaches a same-height sibling; a v3 certificate commits those siblings' headers, and this
/// asks for exactly the signed hashes. No justification is followed, nothing outside `hashes` is
/// admitted, and an incomplete answer is a refusal (the caller is verifying a signed commitment).
pub async fn request_headers(
    hashes: &BTreeSet<BlockHash>,
    metadata_rx: &mut tokio::sync::mpsc::Receiver<BlockMetadataMessage>,
    transport: &dyn TransportLayer,
    conf: &RPConf,
    comm_util: &CommUtil,
    log: &dyn Log,
) -> Result<BTreeMap<BlockHash, BlockHeader>, String> {
    let source = LogSource::new("casper.engine.LfsMetadataRequester");
    let mut headers: BTreeMap<BlockHash, BlockHeader> = BTreeMap::new();
    let mut outstanding: BTreeSet<BlockHash> = hashes.clone();
    let mut idle_rounds: u32 = 0;
    while !outstanding.is_empty() {
        let batch: Vec<BlockHash> = outstanding.iter().copied().take(METADATA_BATCH).collect();
        ask_peers(
            transport,
            conf,
            comm_util,
            &BlockMetadataRequest::hashes(batch.clone()),
            log,
        )
        .await;
        let deadline = tokio::time::Instant::now() + METADATA_ROUND_TIMEOUT;
        let before = headers.len();
        while !outstanding.is_empty() {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let Ok(Some(answer)) = tokio::time::timeout(remaining, metadata_rx.recv()).await else {
                break;
            };
            for header in answer.headers {
                if !hashes.contains(&header.block_hash) {
                    continue;
                }
                if let Some(known) = headers.get(&header.block_hash) {
                    if known != &header {
                        return Err(format!(
                            "two peers sent different headers for block {}: a header cannot be \
                             content-verified, so a disagreement about one is not resolvable here",
                            header.block_hash.to_hex()
                        ));
                    }
                } else {
                    headers.insert(header.block_hash, header);
                }
                // Answers may arrive in any order; drop every hash that is now held.
                outstanding.retain(|hash| !headers.contains_key(hash));
            }
        }
        if headers.len() == before {
            idle_rounds += 1;
            if idle_rounds >= METADATA_MAX_IDLE_ROUNDS {
                let mut missing: Vec<String> =
                    outstanding.iter().take(8).map(|h| h.to_hex()).collect();
                if outstanding.len() > missing.len() {
                    missing.push(format!("… {} more", outstanding.len() - missing.len()));
                }
                return Err(format!(
                    "the fringe-header fetch learned nothing for {idle_rounds} rounds; {} hash(es) \
                     are still outstanding: {}",
                    outstanding.len(),
                    missing.join(", ")
                ));
            }
            log.warn(
                source,
                &format!(
                    "fringe-header fetch: no answer this round; re-asking {} hash(es)",
                    outstanding.len()
                ),
            );
        } else {
            idle_rounds = 0;
        }
    }
    Ok(headers)
}

/// The original hash-frontier walk: one batch of named hashes per round.
async fn hash_walk(
    anchor: BlockHash,
    metadata_rx: &mut tokio::sync::mpsc::Receiver<BlockMetadataMessage>,
    transport: &dyn TransportLayer,
    conf: &RPConf,
    comm_util: &CommUtil,
    log: &dyn Log,
) -> Result<MetadataWalk, String> {
    let source = LogSource::new("casper.engine.LfsMetadataRequester");
    let mut headers: BTreeMap<BlockHash, BlockHeader> = BTreeMap::new();
    let mut asked: BTreeSet<BlockHash> = BTreeSet::new();
    let mut frontier: BTreeSet<BlockHash> = BTreeSet::from([anchor]);
    let mut idle_rounds: u32 = 0;

    while !frontier.is_empty() {
        if headers.len() + frontier.len() > MAX_METADATA_HEADERS {
            return Err(format!(
                "the metadata walk reached {MAX_METADATA_HEADERS} headers without completing: \
                 refusing a history this large"
            ));
        }
        let batch: Vec<BlockHash> = frontier.iter().copied().take(METADATA_BATCH).collect();
        for h in &batch {
            frontier.remove(h);
            asked.insert(*h);
        }
        ask_peers(
            transport,
            conf,
            comm_util,
            &BlockMetadataRequest::hashes(batch.clone()),
            log,
        )
        .await;

        let deadline = tokio::time::Instant::now() + METADATA_ROUND_TIMEOUT;
        let mut pending: BTreeSet<BlockHash> = batch.iter().copied().collect();
        let mut learned = 0usize;
        while !pending.is_empty() {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let Ok(Some(answer)) = tokio::time::timeout(remaining, metadata_rx.recv()).await else {
                break;
            };
            for header in answer.headers {
                // Only an answer to something this walk asked for is admitted; an unsolicited
                // header is not evidence for anything.
                if !asked.contains(&header.block_hash) {
                    continue;
                }
                if let Some(known) = headers.get(&header.block_hash) {
                    if known != &header {
                        return Err(format!(
                            "two peers sent different headers for block {}: a header cannot be \
                             content-verified, so a disagreement about one is not resolvable here",
                            header.block_hash.to_hex()
                        ));
                    }
                    pending.remove(&header.block_hash);
                    continue;
                }
                pending.remove(&header.block_hash);
                for j in &header.justifications {
                    if !headers.contains_key(j) && !asked.contains(j) && !pending.contains(j) {
                        frontier.insert(*j);
                    }
                }
                headers.insert(header.block_hash, header);
                learned += 1;
            }
        }
        // A batch still outstanding goes back to the frontier for the next round (a re-ask is
        // idempotent); an answer that arrived for an earlier round is already in `headers`.
        frontier.extend(pending.into_iter().filter(|h| !headers.contains_key(h)));
        // A progress line every eight rounds: a mature walk is thousands of headers, and a
        // silently advancing walk is otherwise indistinguishable from a stalled one.
        if headers.len() % (METADATA_BATCH * 8) < METADATA_BATCH {
            log.info(
                source,
                &format!(
                    "metadata walk: {} header(s), {} hash(es) outstanding",
                    headers.len(),
                    frontier.len()
                ),
            );
        }
        if learned == 0 {
            idle_rounds += 1;
            if idle_rounds >= METADATA_MAX_IDLE_ROUNDS {
                let mut missing: Vec<String> =
                    frontier.iter().take(8).map(|h| h.to_hex()).collect();
                if frontier.len() > missing.len() {
                    missing.push(format!("… {} more", frontier.len() - missing.len()));
                }
                return Err(format!(
                    "the metadata walk learned nothing for {idle_rounds} rounds; {} hash(es) are \
                     still outstanding: {}",
                    frontier.len(),
                    missing.join(", ")
                ));
            }
            log.warn(
                source,
                &format!(
                    "metadata walk: no answer this round; re-asking {} hash(es) (idle round \
                     {idle_rounds} of {METADATA_MAX_IDLE_ROUNDS})",
                    frontier.len()
                ),
            );
        } else {
            idle_rounds = 0;
        }
    }

    let walk = MetadataWalk { headers };
    check_walk_shape(&walk)?;
    log.info(
        source,
        &format!(
            "metadata walk complete: {} header(s) from anchor {} to genesis",
            walk.headers.len(),
            anchor.to_hex()
        ),
    );
    Ok(walk)
}

/// **Walk the anchor's header closure in height windows.**
///
/// Each round names the current frontier (the missing parents) and asks for their ancestry inside a
/// `METADATA_RANGE_WINDOW`-high window. A peer that understands the range returns the tips and
/// every ancestor in the window; one that does not answers nothing, and the caller falls back to
/// the hash walk. The returned set is not trusted: `check_walk_shape` and the certificate's
/// `metadataRoot` decide whether it is the history the quorum signed.
async fn range_walk(
    anchor: BlockHash,
    anchor_height: i64,
    metadata_rx: &mut tokio::sync::mpsc::Receiver<BlockMetadataMessage>,
    transport: &dyn TransportLayer,
    conf: &RPConf,
    comm_util: &CommUtil,
    log: &dyn Log,
) -> Result<Option<MetadataWalk>, String> {
    let source = LogSource::new("casper.engine.LfsMetadataRequester");
    let mut headers: BTreeMap<BlockHash, BlockHeader> = BTreeMap::new();
    // A missing ancestor to an upper bound on its height: its child's height minus one.
    let mut frontier: BTreeMap<BlockHash, i64> = BTreeMap::from([(anchor, anchor_height)]);
    let mut idle_rounds: u32 = 0;
    let mut answered_any = false;

    while !frontier.is_empty() {
        if headers.len() + frontier.len() > MAX_METADATA_HEADERS {
            return Err(format!(
                "the metadata walk reached {MAX_METADATA_HEADERS} headers without completing: \
                 refusing a history this large"
            ));
        }
        let batch: Vec<(BlockHash, i64)> = frontier
            .iter()
            .take(METADATA_BATCH)
            .map(|(hash, height)| (*hash, *height))
            .collect();
        for (hash, _) in &batch {
            frontier.remove(hash);
        }
        let to = batch
            .iter()
            .map(|(_, height)| *height)
            .max()
            .unwrap_or(anchor_height);
        let from = (to - (METADATA_RANGE_WINDOW - 1)).max(0);
        let tips: Vec<BlockHash> = batch.iter().map(|(hash, _)| *hash).collect();
        ask_peers(
            transport,
            conf,
            comm_util,
            &BlockMetadataRequest::range(tips.clone(), from, to),
            log,
        )
        .await;

        let deadline = tokio::time::Instant::now() + METADATA_ROUND_TIMEOUT;
        let mut outstanding: BTreeSet<BlockHash> = tips.iter().copied().collect();
        let mut learned = 0usize;
        while !outstanding.is_empty() {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let Ok(Some(answer)) = tokio::time::timeout(remaining, metadata_rx.recv()).await else {
                break;
            };
            // Admit the requested tips present in the answer, then their ancestors that arrived
            // with them. A header with no admitted descendant is unsolicited and not evidence.
            let by_hash: BTreeMap<BlockHash, BlockHeader> = answer
                .headers
                .into_iter()
                .map(|header| (header.block_hash, header))
                .collect();
            let mut admit: BTreeSet<BlockHash> = BTreeSet::new();
            let mut queue: VecDeque<BlockHash> = tips
                .iter()
                .copied()
                .filter(|hash| by_hash.contains_key(hash))
                .collect();
            while let Some(hash) = queue.pop_front() {
                if !admit.insert(hash) {
                    continue;
                }
                if let Some(header) = by_hash.get(&hash) {
                    for parent in &header.justifications {
                        if by_hash.contains_key(parent) && !admit.contains(parent) {
                            queue.push_back(*parent);
                        }
                    }
                }
            }
            for hash in admit {
                outstanding.remove(&hash);
                let Some(header) = by_hash.get(&hash) else {
                    continue;
                };
                if let Some(known) = headers.get(&hash) {
                    if known != header {
                        return Err(format!(
                            "two peers sent different headers for block {}: a header cannot be \
                             content-verified, so a disagreement about one is not resolvable here",
                            hash.to_hex()
                        ));
                    }
                    continue;
                }
                let height = i64::from(header.block_num);
                let bound = height.saturating_sub(1);
                for parent in &header.justifications {
                    if !headers.contains_key(parent) {
                        let entry = frontier.entry(*parent).or_insert(bound);
                        if bound < *entry {
                            *entry = bound;
                        }
                    }
                }
                answered_any = true;
                learned += 1;
                headers.insert(hash, header.clone());
            }
        }
        // A first round that learned nothing means the peers answered no range at all (an older
        // peer ignores the fields). Fall back rather than waiting out the idle budget.
        if !answered_any {
            return Ok(None);
        }
        // A tip nobody answered is re-asked next round; the walk is idempotent.
        for (hash, bound) in batch {
            if !headers.contains_key(&hash) {
                frontier.entry(hash).or_insert(bound);
            }
        }
        if learned == 0 {
            idle_rounds += 1;
            if idle_rounds >= METADATA_MAX_IDLE_ROUNDS {
                let mut missing: Vec<String> =
                    frontier.keys().take(8).map(|hash| hash.to_hex()).collect();
                if frontier.len() > missing.len() {
                    missing.push(format!("… {} more", frontier.len() - missing.len()));
                }
                return Err(format!(
                    "the metadata walk learned nothing for {idle_rounds} rounds; {} hash(es) are \
                     still outstanding: {}",
                    frontier.len(),
                    missing.join(", ")
                ));
            }
            log.warn(
                source,
                &format!(
                    "metadata walk: no answer this round; re-asking {} hash(es) (idle round \
                     {idle_rounds} of {METADATA_MAX_IDLE_ROUNDS})",
                    frontier.len()
                ),
            );
        } else {
            idle_rounds = 0;
        }
        if headers.len() % (METADATA_BATCH * 8) < METADATA_BATCH {
            log.info(
                source,
                &format!(
                    "metadata walk: {} header(s), {} hash(es) outstanding",
                    headers.len(),
                    frontier.len()
                ),
            );
        }
    }

    let walk = MetadataWalk { headers };
    check_walk_shape(&walk)?;
    log.info(
        source,
        &format!(
            "metadata walk complete: {} header(s) from anchor {} to genesis (height windows)",
            walk.headers.len(),
            anchor.to_hex()
        ),
    );
    Ok(Some(walk))
}

/// **The walk's shape, checked locally.** Exactly one parentless block, at height zero; every
/// parent present; every parent older than its child. These are the claims a header set can be
/// checked for without a body, and a set that fails them cannot be published as a DAG.
pub fn check_walk_shape(walk: &MetadataWalk) -> Result<(), String> {
    let roots: Vec<&BlockHeader> = walk
        .headers
        .values()
        .filter(|h| h.justifications.is_empty())
        .collect();
    if roots.len() != 1 {
        return Err(format!(
            "a metadata history must have exactly one parentless block, found {}",
            roots.len()
        ));
    }
    let genesis = roots[0];
    if i64::from(genesis.block_num) != 0 {
        return Err(format!(
            "the parentless block {} is at height {}, not 0",
            genesis.block_hash.to_hex(),
            genesis.block_num
        ));
    }
    for header in walk.headers.values() {
        for parent in &header.justifications {
            let Some(parent_header) = walk.headers.get(parent) else {
                return Err(format!(
                    "block {} names parent {}, which the walk did not receive",
                    header.block_hash.to_hex(),
                    parent.to_hex()
                ));
            };
            if parent_header.block_num >= header.block_num {
                return Err(format!(
                    "block {} names parent {} at height {} — a parent must be older than its child",
                    header.block_hash.to_hex(),
                    parent.to_hex(),
                    parent_header.block_num
                ));
            }
        }
    }
    Ok(())
}

/// Ask the bootstrap and a bounded fan-out of connected peers for one request. An unreachable peer
/// is absent for this round; the next round re-asks the same frontier.
async fn ask_peers(
    transport: &dyn TransportLayer,
    conf: &RPConf,
    comm_util: &CommUtil,
    request: &BlockMetadataRequest,
    log: &dyn Log,
) {
    let mut chosen: Vec<PeerNode> = Vec::new();
    if let Some(bootstrap) = &conf.bootstrap {
        chosen.push(bootstrap.clone());
    }
    // **A connections read that never returns must not stop the walk.** The bootstrap is known from
    // the configuration; the connected set is only the fan-out. A writer holding the connections
    // cell (a discovery cycle) left the walker awaiting this read with no round timeout and no log,
    // which is how a 2,175-header walk froze silently.
    let connected =
        match tokio::time::timeout(METADATA_ROUND_TIMEOUT, comm_util.connected_peers()).await {
            Ok(peers) => peers,
            Err(_) => {
                log.warn(
                    LogSource::new("casper.engine.LfsMetadataRequester"),
                    &format!(
                        "metadata walk: the connections read did not return within \
                     {METADATA_ROUND_TIMEOUT:?}; asking the bootstrap only this round"
                    ),
                );
                Vec::new()
            }
        };
    for peer in connected {
        if chosen.len() > METADATA_FANOUT {
            break;
        }
        if peer.id == conf.local.id || chosen.iter().any(|p| p.id == peer.id) {
            continue;
        }
        chosen.push(peer);
    }
    if chosen.is_empty() {
        log.warn(
            LogSource::new("casper.engine.LfsMetadataRequester"),
            "metadata walk: no peer to ask (no bootstrap and no connections)",
        );
        return;
    }
    for peer in chosen {
        // **A blocked transport send must not stall the walk.** The send sits outside the round's
        // timeout; a bootstrap whose inbound path is saturated would otherwise stop the walk with no
        // log line at all (measured on a 2,175-header walk). The request is idempotent, so a timed-out
        // send is re-asked next round.
        let send = rchain_comm::transport::transport_layer_syntax::send_to_peer(
            transport,
            conf,
            &peer,
            BlockMetadataRequestSerde.mk_packet(&request),
        );
        match tokio::time::timeout(METADATA_ROUND_TIMEOUT, send).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => log.warn(
                LogSource::new("casper.engine.LfsMetadataRequester"),
                &format!(
                    "metadata walk: request to {peer} failed: {e}; another peer and the next round \
                     may still answer"
                ),
            ),
            Err(_) => log.warn(
                LogSource::new("casper.engine.LfsMetadataRequester"),
                &format!(
                    "metadata walk: request to {peer} did not leave the node within \
                     {METADATA_ROUND_TIMEOUT:?}; the next round re-asks the same hashes"
                ),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_comm::errors::CommErr;
    use rchain_comm::peer_node::NodeIdentifier;
    use rchain_comm::rp::rp_conf::ClearConnectionsConf;
    use rchain_comm::transport::chunker::Blob;
    use rchain_models::block::state_hash::StateHash;
    use rchain_models::casper::protocol::packet_type_tag::FromPacket;
    use rchain_models::comm::protocol::Protocol;
    use rchain_models::validator::Validator;
    use rchain_shared::log::NopLog;
    use std::sync::{Arc, Mutex};

    use crate::protocol::casper_message_protocol::BlockMetadataRequestSerde;
    use crate::protocol::comm_util::CommUtil;

    fn peer(name: &str) -> PeerNode {
        PeerNode::from(
            NodeIdentifier::new(name.as_bytes().to_vec()),
            "host".to_string(),
            rchain_shared::refined::Port::new(40400),
            rchain_shared::refined::Port::new(40400),
        )
    }

    fn conf(local: &PeerNode, bootstrap: &PeerNode) -> RPConf {
        RPConf {
            local: local.clone(),
            network_id: "testnet".to_string(),
            bootstrap: Some(bootstrap.clone()),
            default_timeout: Duration::from_secs(10),
            max_num_of_connections: 10,
            clear_connections: ClearConnectionsConf {
                num_of_connections_pinged: 10,
            },
        }
    }

    fn block_hash(height: i64) -> BlockHash {
        let mut bytes = [0u8; 32];
        bytes[..8].copy_from_slice(&height.to_le_bytes());
        bytes[31] = 0xA5;
        BlockHash::new(bytes)
    }

    fn header(height: i64, parent: Option<BlockHash>) -> BlockHeader {
        BlockHeader {
            block_hash: block_hash(height),
            block_num: height.try_into().expect("a height"),
            sender: Validator::new([1u8; 65]),
            seq_num: 0.try_into().expect("a sequence"),
            justifications: parent.into_iter().collect(),
            bonds_map: BTreeMap::new(),
            fringe: BTreeSet::new(),
            fringe_state_hash: StateHash::new([0u8; 32]),
            pre_state_hash: StateHash::new([0u8; 32]),
            post_state_hash: StateHash::new([0u8; 32]),
            body_available: false,
            sig: Vec::new(),
            sig_algorithm: "secp256k1".to_string(),
        }
    }

    /// A chain of headers `0..=top`, each naming its predecessor.
    fn chain(top: i64) -> Vec<BlockHeader> {
        let mut chain = Vec::new();
        for height in 0..=top {
            let parent = (height > 0).then(|| block_hash(height - 1));
            chain.push(header(height, parent));
        }
        chain
    }

    /// A peer that answers a range request the way the handler does: the named tip and its
    /// ancestors inside the window. It records every request, so a test can count round trips.
    struct ChainTransport {
        chain: Vec<BlockHeader>,
        tx: tokio::sync::mpsc::Sender<BlockMetadataMessage>,
        requests: Mutex<Vec<BlockMetadataRequest>>,
    }

    #[async_trait::async_trait]
    impl TransportLayer for ChainTransport {
        async fn send(&self, _peer: &PeerNode, msg: Protocol) -> CommErr<()> {
            let Some(rchain_models::comm::protocol::protocol::Message::Packet(packet)) =
                &msg.message
            else {
                return Ok(());
            };
            let Ok(request) = BlockMetadataRequestSerde.parse_from(packet) else {
                return Ok(());
            };
            self.requests.lock().unwrap().push(request.clone());
            let mut headers = Vec::new();
            if let Some(range) = request.range {
                for tip in &request.hashes {
                    let Some(tip_header) = self.chain.iter().find(|h| h.block_hash == *tip) else {
                        continue;
                    };
                    let tip_height = i64::from(tip_header.block_num);
                    for candidate in self
                        .chain
                        .iter()
                        .filter(|h| {
                            let height = i64::from(h.block_num);
                            height >= range.from && height <= range.to.min(tip_height)
                        })
                        .rev()
                    {
                        headers.push(candidate.clone());
                    }
                }
            }
            let _ = self.tx.send(BlockMetadataMessage { headers }).await;
            Ok(())
        }

        async fn broadcast(&self, _peers: &[PeerNode], _msg: Protocol) -> Vec<CommErr<()>> {
            Vec::new()
        }

        async fn stream(&self, _peers: &[PeerNode], _blob: Blob) {}
    }

    /// **A mature chain is fetched in height windows, not one generation per round.** The walk's
    /// result is the same set the hash-frontier walk produces; only the number of round trips
    /// changes. The request count is the witness: 601 headers in at most four requests, where the
    /// per-generation walk would have needed 601.
    #[tokio::test]
    async fn a_height_range_walk_fetches_a_mature_chain_in_windows() {
        let local = peer("local");
        let bootstrap = peer("bootstrap");
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let transport = Arc::new(ChainTransport {
            chain: chain(600),
            tx,
            requests: Mutex::new(Vec::new()),
        });
        let conf = conf(&local, &bootstrap);
        let connections: crate::protocol::comm_util::ConnectionsCell =
            Arc::new(tokio::sync::RwLock::new(Vec::new()));
        let comm_util = CommUtil::new(
            transport.clone(),
            conf.clone(),
            connections,
            Arc::new(NopLog),
        );

        let walk = request_metadata(
            block_hash(600),
            600,
            &mut rx,
            transport.as_ref(),
            &conf,
            &comm_util,
            &NopLog,
        )
        .await
        .expect("the window walk completes");

        assert_eq!(walk.headers.len(), 601, "every header, genesis included");
        assert!(walk.headers.contains_key(&block_hash(0)), "genesis arrived");
        let rounds = transport.requests.lock().unwrap().len();
        assert!(
            rounds <= 4,
            "601 headers must not cost 601 round trips; the walk used {rounds}"
        );
    }

    /// A peer that answers nothing to a range request is reported as unsupported, so the caller
    /// falls back to the hash-frontier walk. (The answer to an ignored request is not an error; it
    /// is simply not an answer, which is how an older peer behaves.)
    #[tokio::test]
    async fn a_peer_that_ignores_the_range_is_reported_as_unsupported() {
        struct SilentTransport;

        #[async_trait::async_trait]
        impl TransportLayer for SilentTransport {
            async fn send(&self, _peer: &PeerNode, _msg: Protocol) -> CommErr<()> {
                Ok(())
            }
            async fn broadcast(&self, _peers: &[PeerNode], _msg: Protocol) -> Vec<CommErr<()>> {
                Vec::new()
            }
            async fn stream(&self, _peers: &[PeerNode], _blob: Blob) {}
        }

        let local = peer("local");
        let bootstrap = peer("bootstrap");
        let (_tx, mut rx) = tokio::sync::mpsc::channel(1);
        let transport = Arc::new(SilentTransport);
        let conf = conf(&local, &bootstrap);
        let connections: crate::protocol::comm_util::ConnectionsCell =
            Arc::new(tokio::sync::RwLock::new(Vec::new()));
        let comm_util = CommUtil::new(
            transport.clone(),
            conf.clone(),
            connections,
            Arc::new(NopLog),
        );

        let answer = range_walk(
            block_hash(600),
            600,
            &mut rx,
            transport.as_ref(),
            &conf,
            &comm_util,
            &NopLog,
        )
        .await
        .expect("a silence is not an error here");
        assert!(
            answer.is_none(),
            "the caller must fall back to the hash walk"
        );
    }
}
