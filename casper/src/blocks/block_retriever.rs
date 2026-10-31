//! Block retriever (port of `blocks/BlockRetriever.scala`).
//!
//! Makes sure a block is received once casper requests it. A block stays in scope until it is added
//! to the casper buffer (acknowledged via [`BlockRetriever::ack_received`]).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use rchain_comm::peer_node::PeerNode;
use rchain_models::block_hash::BlockHash;
use rchain_shared::lock::Unpoison;
use rchain_shared::log::{Log, LogSource};
use rchain_shared::time::current_millis;

use crate::protocol::comm_util::CommUtil;

/// Maximum number of distinct hashes the retriever tracks at once (H5): bounds the `requested` map
/// against a peer flooding `BlockHash`/`HasBlock` messages for bogus hashes.
const MAX_REQUESTED_BLOCKS: usize = 10_000;
/// Maximum waiting-list length per requested hash (H5).
const MAX_WAITING_LIST_PER_HASH: usize = 32;
/// How many capacity-refused hashes wait for a slot to free (F-U8-03). A bound on the overflow
/// itself: an adversary cannot turn a full request map into an unbounded second map.
const MAX_CAPACITY_OVERFLOW: usize = 4_096;
/// Renewed child demand may retry an unresolved dependency once per default
/// maintenance cadence, without waiting for the 240-second request-age timeout.
const MISSING_DEPENDENCY_RETRY_MILLIS: i64 = 30_000;

enum MissingDependencyAction {
    Admit,
    Wait,
    Request(PeerNode),
    Discover,
}

fn reserve_missing_dependency(
    requests: &mut BTreeMap<BlockHash, RequestState>,
    hash: &BlockHash,
    now: i64,
) -> MissingDependencyAction {
    match requests.get_mut(hash) {
        None => MissingDependencyAction::Admit,
        Some(request) => {
            let age = now.saturating_sub(request.timestamp);
            if age < MISSING_DEPENDENCY_RETRY_MILLIS {
                MissingDependencyAction::Wait
            } else {
                request.timestamp = now;
                if request.waiting_list.is_empty() {
                    request.peers.clear();
                    MissingDependencyAction::Discover
                } else {
                    let peer = request.waiting_list.remove(0);
                    if !request.peers.contains(&peer)
                        && request.peers.len() < MAX_WAITING_LIST_PER_HASH
                    {
                        request.peers.push(peer.clone());
                    }
                    MissingDependencyAction::Request(peer)
                }
            }
        }
    }
}

/// Reason a hash was admitted (port of `AdmitHashReason`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmitHashReason {
    HasBlockMessageReceived,
    HashBroadcastRecieved,
    MissingDependencyRequested,
    BlockReceived,
}

/// Status of an admitted hash (port of `AdmitHashStatus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmitHashStatus {
    NewSourcePeerAddedToRequest,
    NewRequestAdded,
    /// The `requested` map is at capacity; the new hash was not admitted.
    CapacityReached,
    Ignore,
}

/// Result of admitting a hash (port of `AdmitHashResult`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmitHashResult {
    pub status: AdmitHashStatus,
    pub broadcast_request: bool,
    pub request_block: bool,
}

/// Per-hash request state (port of `RequestState`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestState {
    /// Last time the block was requested.
    pub timestamp: i64,
    /// Peers that were queried for this block.
    pub peers: Vec<PeerNode>,
    /// Peers that reportedly have the block and are yet to be queried.
    pub waiting_list: Vec<PeerNode>,
}

impl RequestState {
    fn new(now: i64, source_peer: Option<PeerNode>) -> Self {
        RequestState {
            timestamp: now,
            peers: Vec::new(),
            waiting_list: source_peer.into_iter().collect(),
        }
    }
}

/// Add a new request if the hash is unknown (port of `addNewRequest`).
fn add_new_request(
    state: &BTreeMap<BlockHash, RequestState>,
    hash: &BlockHash,
    now: i64,
    source_peer: Option<PeerNode>,
) -> BTreeMap<BlockHash, RequestState> {
    if state.contains_key(hash) {
        state.clone()
    } else {
        let mut new_state = state.clone();
        new_state.insert(*hash, RequestState::new(now, source_peer));
        new_state
    }
}

/// Append a peer to an existing request's waiting list (port of `addSourcePeerToRequest`).
fn add_source_peer_to_request(
    state: &BTreeMap<BlockHash, RequestState>,
    hash: &BlockHash,
    peer: &PeerNode,
) -> BTreeMap<BlockHash, RequestState> {
    match state.get(hash) {
        None => state.clone(),
        Some(request_state) => {
            if request_state.waiting_list.len() >= MAX_WAITING_LIST_PER_HASH {
                return state.clone();
            }
            let mut updated = request_state.clone();
            updated.waiting_list.push(peer.clone());
            let mut new_state = state.clone();
            new_state.insert(*hash, updated);
            new_state
        }
    }
}

/// The pure state transition for `admit_hash` (port of the `RequestedBlocks.modify` block).
pub fn admit_hash_state(
    state: &BTreeMap<BlockHash, RequestState>,
    hash: &BlockHash,
    now: i64,
    peer: Option<&PeerNode>,
) -> (BTreeMap<BlockHash, RequestState>, AdmitHashResult) {
    let unknown_hash = !state.contains_key(hash);
    if unknown_hash {
        if state.len() >= MAX_REQUESTED_BLOCKS {
            return (
                state.clone(),
                AdmitHashResult {
                    status: AdmitHashStatus::CapacityReached,
                    broadcast_request: false,
                    request_block: false,
                },
            );
        }
        let new_state = add_new_request(state, hash, now, peer.cloned());
        let result = AdmitHashResult {
            status: AdmitHashStatus::NewRequestAdded,
            broadcast_request: peer.is_none(),
            request_block: peer.is_some(),
        };
        (new_state, result)
    } else {
        match peer {
            Some(peer) => {
                let already_known = state[hash].waiting_list.contains(peer);
                if already_known {
                    (
                        state.clone(),
                        AdmitHashResult {
                            status: AdmitHashStatus::Ignore,
                            broadcast_request: false,
                            request_block: false,
                        },
                    )
                } else {
                    let was_empty = state[hash].waiting_list.is_empty();
                    let new_state = add_source_peer_to_request(state, hash, peer);
                    let result = AdmitHashResult {
                        status: AdmitHashStatus::NewSourcePeerAddedToRequest,
                        broadcast_request: false,
                        // Request from the first peer in the waiting list; otherwise requests are
                        // triggered by the casper loop.
                        request_block: was_empty,
                    };
                    (new_state, result)
                }
            }
            None => (
                state.clone(),
                AdmitHashResult {
                    status: AdmitHashStatus::Ignore,
                    broadcast_request: false,
                    request_block: false,
                },
            ),
        }
    }
}

/// Block retriever (port of `BlockRetriever[F]`).
pub struct BlockRetriever {
    requested: tokio::sync::Mutex<BTreeMap<BlockHash, RequestState>>,
    /// Hashes refused when the request map was at capacity. They retain their source peer and are
    /// re-admitted as `ack_received` frees slots; bounded by [`MAX_CAPACITY_OVERFLOW`].
    overflow: std::sync::Mutex<std::collections::VecDeque<(BlockHash, Option<PeerNode>)>>,
    capacity_refusals: std::sync::atomic::AtomicU64,
    comm_util: Arc<CommUtil>,
    log: Arc<dyn Log>,
    log_source: LogSource,
}

impl BlockRetriever {
    pub fn new(comm_util: Arc<CommUtil>, log: Arc<dyn Log>) -> Self {
        BlockRetriever {
            requested: tokio::sync::Mutex::new(BTreeMap::new()),
            overflow: std::sync::Mutex::new(std::collections::VecDeque::new()),
            capacity_refusals: std::sync::atomic::AtomicU64::new(0),
            comm_util,
            log,
            log_source: LogSource::new("casper.blocks.BlockRetriever"),
        }
    }

    /// Make the retriever process an incoming hash (port of `admitHash`).
    pub async fn admit_hash(
        &self,
        hash: &BlockHash,
        peer: Option<&PeerNode>,
        reason: AdmitHashReason,
    ) -> AdmitHashResult {
        let now = current_millis();
        let result = {
            let mut state = self.requested.lock().await;
            let (new_state, result) = admit_hash_state(&state, hash, now, peer);
            *state = new_state;
            result
        };

        match result.status {
            AdmitHashStatus::NewSourcePeerAddedToRequest => {
                self.log.debug(
                    self.log_source,
                    &format!(
                        "Adding {} to waiting list of {} request. Reason: {:?}",
                        peer.map(|p| p.endpoint.host.as_str()).unwrap_or(""),
                        hash.to_hex(),
                        reason
                    ),
                );
            }
            AdmitHashStatus::NewRequestAdded => {
                self.log.info(
                    self.log_source,
                    &format!(
                        "Adding {} hash to RequestedBlocks because of {:?}.",
                        hash.to_hex(),
                        reason
                    ),
                );
            }
            AdmitHashStatus::CapacityReached => {
                self.capacity_refusals
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let retained = {
                    let mut overflow = self.overflow.lock().unpoison();
                    if overflow.len() < MAX_CAPACITY_OVERFLOW {
                        overflow.push_back((*hash, peer.cloned()));
                        true
                    } else {
                        false
                    }
                };
                self.log.warn(
                    self.log_source,
                    &format!(
                        "Block-retriever request map is at capacity ({}); {} hash {}",
                        MAX_REQUESTED_BLOCKS,
                        if retained {
                            "retained for re-admission:"
                        } else {
                            "overflow is full, dropping"
                        },
                        hash.to_hex()
                    ),
                );
            }
            AdmitHashStatus::Ignore => {}
        }

        if result.broadcast_request {
            self.comm_util.broadcast_has_block_request(hash).await;
        }
        if result.request_block {
            if let Some(peer) = peer {
                self.comm_util.request_for_block(peer, hash).await;
            }
        }
        result
    }

    /// Request an absent parent, including one whose earlier reply was lost.
    /// Reserve the retry under the request lock before any network await, so
    /// concurrent children cannot each resend the same unresolved dependency.
    pub async fn request_missing_dependency(&self, hash: &BlockHash) {
        let now = current_millis();
        let action = {
            let mut requests = self.requested.lock().await;
            reserve_missing_dependency(&mut requests, hash, now)
        };
        self.perform_missing_dependency_action(hash, action).await;
    }

    /// One overdue request per valid received block. A missing ancestor can be
    /// hidden behind children already stored before the partition, so direct
    /// parent demand alone is insufficient. Never retry the body just received.
    pub async fn retry_oldest_pending_request(&self, received: &BlockHash) {
        let now = current_millis();
        let pending = {
            let mut requests = self.requested.lock().await;
            let oldest = requests
                .iter()
                .filter(|(hash, request)| {
                    *hash != received
                        && now.saturating_sub(request.timestamp) >= MISSING_DEPENDENCY_RETRY_MILLIS
                })
                .min_by_key(|(_, request)| request.timestamp)
                .map(|(hash, _)| *hash);
            oldest.map(|hash| {
                let action = reserve_missing_dependency(&mut requests, &hash, now);
                (hash, action)
            })
        };
        if let Some((hash, action)) = pending {
            self.perform_missing_dependency_action(&hash, action).await;
        }
    }

    async fn perform_missing_dependency_action(
        &self,
        hash: &BlockHash,
        action: MissingDependencyAction,
    ) {
        match action {
            MissingDependencyAction::Admit => {
                self.admit_hash(hash, None, AdmitHashReason::MissingDependencyRequested)
                    .await;
            }
            MissingDependencyAction::Wait => {}
            MissingDependencyAction::Request(peer) => {
                self.log.info(
                    self.log_source,
                    &format!(
                        "Retrying unresolved dependency {} from {} after renewed block demand.",
                        hash.to_hex(),
                        peer.endpoint.host
                    ),
                );
                self.comm_util.request_for_block(&peer, hash).await;
            }
            MissingDependencyAction::Discover => {
                self.comm_util.broadcast_has_block_request(hash).await;
            }
        }
    }

    /// How many hashes the request map refused for capacity. A visible counter for F-U8-03.
    pub fn capacity_refusals(&self) -> u64 {
        self.capacity_refusals
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Acknowledge that a block was received (port of `ackReceived`).
    pub async fn ack_received(&self, hash: &BlockHash) {
        let is_received = {
            let mut state = self.requested.lock().await;
            let old = state.clone();
            state.remove(hash);
            *state != old
        };
        if !is_received {
            return;
        }
        self.log.info(
            self.log_source,
            &format!("Block {} marked as received.", hash.to_hex()),
        );
        // The freed slot is spent on the oldest capacity-refused hash, if any (F-U8-03). The
        // admission happens outside the request-map guard so it cannot deadlock on it.
        let next = self.overflow.lock().unpoison().pop_front();
        if let Some((hash, peer)) = next {
            self.log.info(
                self.log_source,
                &format!(
                    "Re-admitting capacity-refused hash {} after a slot freed.",
                    hash.to_hex()
                ),
            );
            self.admit_hash(
                &hash,
                peer.as_ref(),
                AdmitHashReason::MissingDependencyRequested,
            )
            .await;
        }
    }

    /// Try to re-request all pending hashes whose latest request is older than `age_threshold`
    /// (port of `requestAll`).
    pub async fn request_all(&self, age_threshold: Duration) {
        let state = { self.requested.lock().await.clone() };
        let keys: Vec<BlockHash> = state.keys().copied().collect();
        if !keys.is_empty() {
            self.log.debug(
                self.log_source,
                &format!(
                    "Running BlockRetriever maintenance ({} items unexpired).",
                    keys.len()
                ),
            );
        }
        for hash in keys {
            let requested = state[&hash].clone();
            let expired = current_millis() - requested.timestamp > age_threshold.as_millis() as i64;
            if expired {
                self.log.debug(
                    self.log_source,
                    &format!(
                        "Casper loop: checking if should re-request {}.",
                        hash.to_hex()
                    ),
                );
                self.try_rerequest(hash, requested).await;
            }
        }
    }

    /// Re-request a single expired hash from the next waiting peer, or broadcast for new sources
    /// (port of `tryRerequest`).
    async fn try_rerequest(&self, hash: BlockHash, requested: RequestState) {
        match requested.waiting_list.split_first() {
            Some((next_peer, waiting_list_tail)) => {
                self.log.debug(
                    self.log_source,
                    &format!(
                        "Trying {} to query for {} block. Remain waiting: {}.",
                        next_peer.endpoint.host,
                        hash.to_hex(),
                        waiting_list_tail
                            .iter()
                            .map(|p| p.endpoint.host.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
                self.comm_util.request_for_block(next_peer, &hash).await;
                let mut peers = requested.peers.clone();
                if !peers.contains(next_peer) {
                    peers.push(next_peer.clone());
                }
                let updated = RequestState {
                    timestamp: current_millis(),
                    peers,
                    waiting_list: waiting_list_tail.to_vec(),
                };
                let mut state = self.requested.lock().await;
                if state.get(&hash) == Some(&requested) {
                    state.insert(hash, updated);
                }
            }
            None => {
                self.log.warn(
                    self.log_source,
                    &format!(
                        "Could not retrieve requested block {} from {}. Asking peers again.",
                        hash.to_hex(),
                        requested
                            .peers
                            .iter()
                            .map(|p| p.endpoint.host.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
                {
                    let mut state = self.requested.lock().await;
                    if state.get(&hash) != Some(&requested) {
                        return;
                    }
                    state.remove(&hash);
                }
                self.comm_util.broadcast_has_block_request(&hash).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use rchain_comm::errors::CommErr;
    use rchain_comm::peer_node::NodeIdentifier;
    use rchain_comm::rp::rp_conf::{ClearConnectionsConf, RPConf};
    use rchain_comm::transport::transport_layer::TransportLayer;
    use rchain_models::comm::protocol::Protocol;
    use rchain_shared::log::NopLog;

    use crate::protocol::comm_util::{CommUtil, ConnectionsCell};

    fn hash(byte: u8) -> BlockHash {
        BlockHash::new([byte; 32])
    }

    /// Distinct hashes for the capacity test (the u8 helper only gives 256).
    fn numbered(value: usize) -> BlockHash {
        let mut bytes = [0u8; 32];
        bytes[..8].copy_from_slice(&(value as u64).to_le_bytes());
        BlockHash::new(bytes)
    }

    fn peer(name: &str, port: u16) -> PeerNode {
        PeerNode::from(
            NodeIdentifier::new(name.as_bytes().to_vec()),
            "host".to_string(),
            rchain_shared::refined::Port::new(port),
            rchain_shared::refined::Port::new(port),
        )
    }

    fn set(items: &[u8]) -> BTreeMap<BlockHash, RequestState> {
        items
            .iter()
            .map(|b| (hash(*b), RequestState::new(0, None)))
            .collect()
    }

    #[test]
    fn admit_hash_state_adds_unknown_hash_and_broadcasts_without_peer() {
        let state = set(&[]);
        let (new_state, result) = admit_hash_state(&state, &hash(1), 0, None);
        assert!(new_state.contains_key(&hash(1)));
        assert_eq!(result.status, AdmitHashStatus::NewRequestAdded);
        assert!(result.broadcast_request);
        assert!(!result.request_block);
    }

    #[test]
    fn admit_hash_state_adds_unknown_hash_and_requests_with_peer() {
        let p = peer("peer", 40400);
        let state = set(&[]);
        let (new_state, result) = admit_hash_state(&state, &hash(1), 0, Some(&p));
        assert!(new_state.contains_key(&hash(1)));
        assert_eq!(result.status, AdmitHashStatus::NewRequestAdded);
        assert!(!result.broadcast_request);
        assert!(result.request_block);
    }

    #[test]
    fn admit_hash_state_ignores_known_hash_without_peer() {
        let state = set(&[1]);
        let (_, result) = admit_hash_state(&state, &hash(1), 0, None);
        assert_eq!(result.status, AdmitHashStatus::Ignore);
        assert!(!result.broadcast_request);
        assert!(!result.request_block);
    }

    #[test]
    fn admit_hash_state_requests_from_first_peer_when_waiting_list_empty() {
        let p = peer("peer", 40400);
        let state = set(&[1]); // known hash, empty waiting list
        let (_, result) = admit_hash_state(&state, &hash(1), 0, Some(&p));
        assert_eq!(result.status, AdmitHashStatus::NewSourcePeerAddedToRequest);
        assert!(result.request_block);
    }

    #[test]
    fn admit_hash_state_ignores_peer_already_in_waiting_list() {
        let p = peer("peer", 40400);
        let mut state = set(&[1]);
        state
            .get_mut(&hash(1))
            .unwrap()
            .waiting_list
            .push(p.clone());
        let (_, result) = admit_hash_state(&state, &hash(1), 0, Some(&p));
        assert_eq!(result.status, AdmitHashStatus::Ignore);
    }

    #[test]
    fn admit_hash_state_adds_peer_without_requesting_when_list_nonempty() {
        let p = peer("peer", 40400);
        let p2 = peer("secondPeer", 40400);
        let mut state = set(&[1]);
        state.get_mut(&hash(1)).unwrap().waiting_list.push(p);
        let (new_state, result) = admit_hash_state(&state, &hash(1), 0, Some(&p2));
        assert_eq!(result.status, AdmitHashStatus::NewSourcePeerAddedToRequest);
        assert!(!result.request_block);
        assert_eq!(new_state[&hash(1)].waiting_list.len(), 2);
    }

    // --- effectful integration (mock transport) -------------------------------------------

    #[derive(Default)]
    struct MockTransport {
        sends: std::sync::Mutex<Vec<(PeerNode, Protocol)>>,
        broadcasts: std::sync::Mutex<Vec<(Vec<PeerNode>, Protocol)>>,
    }

    #[async_trait]
    impl TransportLayer for MockTransport {
        async fn send(&self, peer: &PeerNode, msg: Protocol) -> CommErr<()> {
            self.sends.lock().unwrap().push((peer.clone(), msg));
            Ok(())
        }
        async fn broadcast(&self, peers: &[PeerNode], msg: Protocol) -> Vec<CommErr<()>> {
            self.broadcasts.lock().unwrap().push((peers.to_vec(), msg));
            peers.iter().map(|_| Ok(())).collect()
        }
        async fn stream(&self, _peers: &[PeerNode], _blob: rchain_comm::transport::chunker::Blob) {}
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

    fn retriever(transport: Arc<MockTransport>, local: &PeerNode) -> BlockRetriever {
        let connections: ConnectionsCell = Arc::new(tokio::sync::RwLock::new(Vec::new()));
        let comm_util = Arc::new(CommUtil::new(
            transport,
            conf(local),
            connections,
            Arc::new(NopLog),
        ));
        BlockRetriever::new(comm_util, Arc::new(NopLog))
    }

    #[tokio::test]
    async fn admit_hash_with_peer_sends_block_request() {
        let transport = Arc::new(MockTransport::default());
        let local = peer("src", 40400);
        let remote = peer("peer", 40400);
        let retriever = retriever(transport.clone(), &local);

        let result = retriever
            .admit_hash(
                &hash(1),
                Some(&remote),
                AdmitHashReason::HasBlockMessageReceived,
            )
            .await;
        assert!(result.request_block);
        assert!(!result.broadcast_request);

        let sends = transport.sends.lock().unwrap();
        assert_eq!(sends.len(), 1);
        let (recipient, msg) = &sends[0];
        assert_eq!(recipient, &remote);
        let packet = rchain_comm::rp::protocol_helper::to_packet(msg).unwrap();
        assert_eq!(packet.type_id, "BlockRequest");
    }

    #[tokio::test]
    async fn admit_hash_without_peer_broadcasts_has_block_request() {
        let transport = Arc::new(MockTransport::default());
        let local = peer("src", 40400);
        let retriever = retriever(transport.clone(), &local);

        let result = retriever
            .admit_hash(&hash(1), None, AdmitHashReason::HashBroadcastRecieved)
            .await;
        assert!(result.broadcast_request);
        assert!(!result.request_block);

        let broadcasts = transport.broadcasts.lock().unwrap();
        assert_eq!(broadcasts.len(), 1);
        let (_, msg) = &broadcasts[0];
        let packet = rchain_comm::rp::protocol_helper::to_packet(msg).unwrap();
        assert_eq!(packet.type_id, "HasBlockRequest");
    }

    /// **F-U8-03: a capacity-refused hash is retained and re-admitted when a slot frees.** Before the
    /// overflow queue, `CapacityReached` returned the state unchanged and the hash was forgotten.
    #[tokio::test]
    async fn a_capacity_refused_hash_is_retained_and_readmitted() {
        let transport = Arc::new(MockTransport::default());
        let local = peer("src", 40400);
        let remote = peer("peer", 40400);
        let retriever = retriever(transport.clone(), &local);

        {
            let mut requested = retriever.requested.lock().await;
            for value in 0..MAX_REQUESTED_BLOCKS {
                requested.insert(numbered(value), RequestState::new(0, None));
            }
        }
        let refused = numbered(MAX_REQUESTED_BLOCKS);
        let result = retriever
            .admit_hash(
                &refused,
                Some(&remote),
                AdmitHashReason::HasBlockMessageReceived,
            )
            .await;
        assert_eq!(result.status, AdmitHashStatus::CapacityReached);
        assert_eq!(retriever.capacity_refusals(), 1);
        assert_eq!(retriever.overflow.lock().unpoison().len(), 1);

        retriever.ack_received(&numbered(0)).await;
        assert!(
            retriever.requested.lock().await.contains_key(&refused),
            "the freed slot re-admits the retained hash"
        );
        assert!(retriever.overflow.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn renewed_child_demand_retries_a_lost_dependency_without_flooding() {
        let transport = Arc::new(MockTransport::default());
        let local = peer("src", 40400);
        let remote = peer("peer", 40401);
        let retriever = retriever(transport.clone(), &local);
        retriever
            .admit_hash(
                &hash(1),
                Some(&remote),
                AdmitHashReason::HashBroadcastRecieved,
            )
            .await;

        // No reply arrived, but repeated children inside the retry lease must
        // not multiply the original request.
        for _ in 0..100 {
            retriever.request_missing_dependency(&hash(1)).await;
        }
        assert_eq!(transport.sends.lock().unwrap().len(), 1);
        retriever
            .requested
            .lock()
            .await
            .get_mut(&hash(1))
            .unwrap()
            .timestamp = current_millis() - MISSING_DEPENDENCY_RETRY_MILLIS - 1;
        retriever.request_missing_dependency(&hash(1)).await;
        retriever.request_missing_dependency(&hash(1)).await;
        {
            let sends = transport.sends.lock().unwrap();
            assert_eq!(sends.len(), 2);
            assert_eq!(sends[1].0, remote);
            let first = rchain_comm::rp::protocol_helper::to_packet(&sends[0].1).unwrap();
            let retry = rchain_comm::rp::protocol_helper::to_packet(&sends[1].1).unwrap();
            assert_eq!(first, retry, "retry the same missing block request");
        }
        assert_eq!(retriever.requested.lock().await.len(), 1);
        retriever.ack_received(&hash(1)).await;
        assert!(retriever.requested.lock().await.is_empty());
    }

    #[tokio::test]
    async fn renewed_dependency_demand_tries_the_next_known_source() {
        let transport = Arc::new(MockTransport::default());
        let local = peer("src", 40400);
        let first = peer("first", 40401);
        let second = peer("second", 40402);
        let retriever = retriever(transport.clone(), &local);
        for source in [&first, &second] {
            retriever
                .admit_hash(
                    &hash(1),
                    Some(source),
                    AdmitHashReason::HashBroadcastRecieved,
                )
                .await;
        }
        for _ in 0..2 {
            retriever
                .requested
                .lock()
                .await
                .get_mut(&hash(1))
                .unwrap()
                .timestamp = current_millis() - MISSING_DEPENDENCY_RETRY_MILLIS - 1;
            retriever.request_missing_dependency(&hash(1)).await;
        }
        let sends = transport.sends.lock().unwrap();
        assert_eq!(sends.len(), 3);
        assert_eq!(sends[1].0, first);
        assert_eq!(sends[2].0, second);
    }

    #[tokio::test]
    async fn renewed_dependency_demand_rediscovers_sources_with_a_retry_lease() {
        let transport = Arc::new(MockTransport::default());
        let local = peer("src", 40400);
        let retriever = retriever(transport.clone(), &local);
        retriever.request_missing_dependency(&hash(1)).await;
        assert_eq!(transport.broadcasts.lock().unwrap().len(), 1);
        retriever
            .requested
            .lock()
            .await
            .get_mut(&hash(1))
            .unwrap()
            .timestamp = current_millis() - MISSING_DEPENDENCY_RETRY_MILLIS - 1;
        retriever.request_missing_dependency(&hash(1)).await;
        retriever.request_missing_dependency(&hash(1)).await;
        assert_eq!(transport.broadcasts.lock().unwrap().len(), 2);
        assert_eq!(retriever.requested.lock().await.len(), 1);
    }
    #[tokio::test]
    async fn received_blocks_retry_one_oldest_request_and_exclude_the_received_hash() {
        let transport = Arc::new(MockTransport::default());
        let local = peer("src", 40400);
        let remote = peer("peer", 40401);
        let retriever = retriever(transport.clone(), &local);
        for id in [1, 2] {
            retriever
                .admit_hash(
                    &hash(id),
                    Some(&remote),
                    AdmitHashReason::HashBroadcastRecieved,
                )
                .await;
            retriever
                .requested
                .lock()
                .await
                .get_mut(&hash(id))
                .unwrap()
                .timestamp = current_millis() - (3 - i64::from(id)) * 60_000;
        }
        retriever.retry_oldest_pending_request(&hash(1)).await;
        retriever.retry_oldest_pending_request(&hash(1)).await;
        {
            let sends = transport.sends.lock().unwrap();
            assert_eq!(
                sends.len(),
                3,
                "one retry, then the fresh lease suppresses it"
            );
            let retry = rchain_comm::rp::protocol_helper::to_packet(&sends[2].1).unwrap();
            let second = rchain_comm::rp::protocol_helper::to_packet(&sends[1].1).unwrap();
            assert_eq!(retry, second, "the received older hash must be excluded");
        }
        retriever.retry_oldest_pending_request(&hash(3)).await;
        assert_eq!(transport.sends.lock().unwrap().len(), 4);
        retriever.ack_received(&hash(1)).await;
        retriever.ack_received(&hash(2)).await;
        assert!(retriever.requested.lock().await.is_empty());
    }

    #[tokio::test]
    async fn stale_maintenance_cannot_restore_an_acknowledged_request() {
        let transport = Arc::new(MockTransport::default());
        let local = peer("src", 40400);
        let remote = peer("peer", 40401);
        let retriever = retriever(transport, &local);
        let stale = RequestState::new(0, Some(remote));
        retriever
            .requested
            .lock()
            .await
            .insert(hash(1), stale.clone());

        // Maintenance may snapshot before the receiver acknowledges the body.
        retriever.ack_received(&hash(1)).await;
        retriever.try_rerequest(hash(1), stale).await;
        assert!(retriever.requested.lock().await.is_empty());
    }
}
