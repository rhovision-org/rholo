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

use std::collections::{BTreeMap, BTreeSet};
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
pub async fn request_metadata(
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
        ask_peers(transport, conf, comm_util, &batch, log).await;

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

/// Ask the bootstrap and a bounded fan-out of connected peers for one batch. An unreachable peer
/// is absent for this round; the next round re-asks the same frontier.
async fn ask_peers(
    transport: &dyn TransportLayer,
    conf: &RPConf,
    comm_util: &CommUtil,
    batch: &[BlockHash],
    log: &dyn Log,
) {
    let request = BlockMetadataRequest {
        hashes: batch.to_vec(),
    };
    let mut chosen: Vec<PeerNode> = Vec::new();
    if let Some(bootstrap) = &conf.bootstrap {
        chosen.push(bootstrap.clone());
    }
    let connected = comm_util.connected_peers().await;
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
