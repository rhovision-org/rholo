//! Casper comm utilities (port of `protocol/CommUtil.scala`).
//!
//! `CommUtil` is a thin facade over the transport layer that broadcasts packets to (a random
//! subset of) the current connections, streams blobs, retries sends, and requests blocks. The
//! Scala `ConnectionsCell.random` becomes [`random_connections`](rchain_comm::rp::connect::random_connections).

use std::sync::Arc;
use std::time::Duration;

use rchain_comm::peer_node::PeerNode;
use rchain_comm::rp::connect::random_connections;
use rchain_comm::rp::protocol_helper;
use rchain_comm::rp::rp_conf::RPConf;
use rchain_comm::transport::chunker::Blob;
use rchain_comm::transport::transport_layer::TransportLayer;
use rchain_comm::transport::transport_layer_syntax;
use rchain_models::block_hash::BlockHash;
use rchain_models::casper::protocol::casper_message::{
    BlockHashMessage, BlockRangeRequest, BlockRequest, FinalizedFringeRequest,
    ForkChoiceTipRequest, HasBlockRequest,
};
use rchain_models::casper::protocol::packet_type_tag::ToPacket;
use rchain_models::comm::protocol::{Packet, Protocol};
use rchain_shared::log::{Log, LogSource};

use crate::protocol::casper_message_protocol::{
    BlockHashMessageSerde, BlockRangeRequestSerde, BlockRequestSerde, FinalizedFringeRequestSerde,
    ForkChoiceTipRequestSerde, HasBlockRequestSerde,
};

/// A shared, mutable list of current connections (port of `ConnectionsCell[F]`).
pub type ConnectionsCell = Arc<tokio::sync::RwLock<Vec<PeerNode>>>;

/// Maximum number of retries for a bootstrap request before giving up (M8). Documented deviation:
/// Scala's `keepOnRequestingTillRunning` retries forever.
const MAX_BOOTSTRAP_RETRIES: u32 = 10;

/// A standalone (bootstrap) node tried to send to the bootstrap node (port of
/// `StandaloneNodeSendToBootstrapError`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StandaloneNodeSendToBootstrapError;

impl std::fmt::Display for StandaloneNodeSendToBootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("standalone node cannot send to the bootstrap node")
    }
}

impl std::error::Error for StandaloneNodeSendToBootstrapError {}

/// Comm utilities (port of `CommUtil[F]`).
pub struct CommUtil {
    transport: Arc<dyn TransportLayer>,
    conf: RPConf,
    connections: ConnectionsCell,
    log: Arc<dyn Log>,
    log_source: LogSource,
}

impl CommUtil {
    pub fn new(
        transport: Arc<dyn TransportLayer>,
        conf: RPConf,
        connections: ConnectionsCell,
        log: Arc<dyn Log>,
    ) -> Self {
        CommUtil {
            transport,
            conf,
            connections,
            log,
            log_source: LogSource::new("casper.protocol.CommUtil"),
        }
    }

    /// The peers this node is connected to. The block walk broadcasts its requests to this set, and
    /// the give-up diagnostic lists the members that never answered — the reachability shape of a
    /// joiner whose inbound protocol/discovery ports admit only its bootstrap.
    pub async fn connected_peers(&self) -> Vec<PeerNode> {
        self.connections.read().await.clone()
    }

    /// Broadcast a packet (in one piece) to up to `scope_size` random peers (port of `sendToPeers`).
    pub async fn send_to_peers(&self, message: &Packet, scope_size: Option<usize>) {
        let max = scope_size.unwrap_or(self.conf.max_num_of_connections);
        let peers = {
            let connections = self.connections.read().await;
            random_connections(&connections, max)
        };
        let msg = protocol_helper::packet(&self.conf.local, &self.conf.network_id, message.clone());
        self.transport.broadcast(&peers, msg).await;
    }

    /// Broadcast a packet in chunks (stream) to up to `scope_size` random peers (port of
    /// `streamToPeers`).
    pub async fn stream_to_peers(&self, packet: &Packet, scope_size: Option<usize>) {
        let max = scope_size.unwrap_or(self.conf.max_num_of_connections);
        let peers = {
            let connections = self.connections.read().await;
            random_connections(&connections, max)
        };
        let blob = Blob {
            sender: self.conf.local.clone(),
            packet: packet.clone(),
        };
        self.transport.stream(&peers, blob).await;
    }

    /// Send a packet with retry until it succeeds (port of `sendWithRetry`).
    pub async fn send_with_retry(
        &self,
        message: &Packet,
        peer: &PeerNode,
        retry_after: Duration,
        msg_type_name: &str,
    ) {
        let msg = protocol_helper::packet(&self.conf.local, &self.conf.network_id, message.clone());
        self.log.info(
            self.log_source,
            &format!("Starting to request {msg_type_name}"),
        );
        self.keep_on_requesting_till_running(peer, &msg, retry_after, msg_type_name)
            .await;
    }

    /// Retry sending `msg` to `peer` until it succeeds (port of `keepOnRequestingTillRunning`).
    ///
    /// Bounded to [`MAX_BOOTSTRAP_RETRIES`] attempts (M8): a dead/unreachable bootstrap must not
    /// block node startup forever (documented deviation — Scala retries indefinitely).
    async fn keep_on_requesting_till_running(
        &self,
        peer: &PeerNode,
        msg: &Protocol,
        retry_after: Duration,
        msg_type_name: &str,
    ) {
        let mut attempts = 0u32;
        loop {
            match self.transport.send(peer, msg.clone()).await {
                Ok(_) => {
                    self.log.info(
                        self.log_source,
                        &format!("Successfully sent {msg_type_name} to {peer}"),
                    );
                    break;
                }
                Err(error) => {
                    attempts += 1;
                    if attempts >= MAX_BOOTSTRAP_RETRIES {
                        self.log.error(
                            self.log_source,
                            &format!(
                                "Giving up sending {msg_type_name} to {peer} after {attempts} attempts (last error: {error:?})."
                            ),
                        );
                        break;
                    }
                    self.log.warn(
                        self.log_source,
                        &format!(
                            "Failed to send {msg_type_name} to {peer} because of {error:?}. Retrying in {retry_after:?}..."
                        ),
                    );
                    tokio::time::sleep(retry_after).await;
                }
            }
        }
    }

    /// Request a block from a peer (port of `requestForBlock`).
    pub async fn request_for_block(&self, peer: &PeerNode, hash: &BlockHash) {
        self.log.debug(
            self.log_source,
            &format!("Requesting {} from {}.", hash.to_hex(), peer.endpoint.host),
        );
        let packet = BlockRequestSerde.mk_packet(&BlockRequest { hash: *hash });
        if let Err(e) =
            transport_layer_syntax::send_to_peer(self.transport.as_ref(), &self.conf, peer, packet)
                .await
        {
            self.log.warn(
                self.log_source,
                &format!(
                    "could not request block {} from {}: {e} (AUDIT C254's E6b)",
                    hash.to_hex(),
                    peer.endpoint.host
                ),
            );
        }
    }

    /// Fetch LFS ancestry from the configured source of the admitted fringe and
    /// state pages. A discovered peer can acknowledge a request while its reply
    /// path to the joiner is blocked; choosing one at random then costs an idle
    /// retry for each ancestry frontier. The bootstrap is usable before it enters
    /// the discovery connection set. Block hashes are still verified by the walk,
    /// and an unserved block still fails under its existing bounded retry policy.
    pub async fn request_lfs_block(&self, hash: &BlockHash) -> Vec<PeerNode> {
        if let Some(bootstrap) = &self.conf.bootstrap {
            self.request_for_block(bootstrap, hash).await;
            vec![bootstrap.clone()]
        } else {
            // Standalone callers have no admitted bootstrap; retain their
            // peer-based block walk rather than inventing a state source.
            self.broadcast_request_for_block(hash, Some(1)).await
        }
    }

    /// Ask a peer for the blocks at heights `[from, to]` (C259's catch-up — see
    /// [`BlockRangeRequest`]).
    ///
    /// One peer, not a broadcast: the answer is a window of the peer's DAG, and the walk depends on
    /// knowing *which* peer's chain it is walking up. A peer that does not know the message answers
    /// nothing, which the driver reads as "no progress" and stops on — the same degraded exchange the
    /// anchored fringe request gets.
    pub async fn request_block_range(&self, peer: &PeerNode, req: &BlockRangeRequest) {
        self.log.debug(
            self.log_source,
            &format!(
                "Requesting heights {}..={} from {}.",
                req.from, req.to, peer.endpoint.host
            ),
        );
        let packet = BlockRangeRequestSerde.mk_packet(req);
        if let Err(e) =
            transport_layer_syntax::send_to_peer(self.transport.as_ref(), &self.conf, peer, packet)
                .await
        {
            self.log.warn(
                self.log_source,
                &format!(
                    "could not request heights {}..={} from {}: {e}",
                    req.from, req.to, peer.endpoint.host
                ),
            );
        }
    }

    // --- CommUtil syntax extensions (port of `CommUtilOps`) --------------------------------

    /// Advertise an accepted block hash to the current peers.
    ///
    /// Unlike general best-effort broadcasts, a block-hash advertisement affects
    /// whether peers can acquire the next justification. The transport returns one
    /// outcome per target; silently discarding those results and logging "sent"
    /// turns a failed handoff into a false liveness signal. Retry each failed or
    /// unreported target once, with a bounded timeout, without altering the
    /// block, sender, network id, quorum or any finality rule.
    pub async fn send_block_hash(&self, hash: &BlockHash, block_creator: &[u8]) {
        let block_hash = hash.to_hex();
        let packet = BlockHashMessageSerde.mk_packet(&BlockHashMessage {
            block_hash: *hash,
            block_creator: block_creator.to_vec(),
        });
        let peers = {
            let connections = self.connections.read().await;
            random_connections(&connections, self.conf.max_num_of_connections)
        };
        let message = protocol_helper::packet(&self.conf.local, &self.conf.network_id, packet);
        let outcomes = self.transport.broadcast(&peers, message.clone()).await;

        if outcomes.len() != peers.len() {
            self.log.warn(
                self.log_source,
                &format!(
                    "Block hash {block_hash}: broadcast returned {} outcomes for {} peers; missing outcomes require retry, not inferred success",
                    outcomes.len(),
                    peers.len()
                ),
            );
        }
        let mut initially_accepted = 0usize;
        let mut retry_accepted = 0usize;
        let mut not_delivered = 0usize;
        // Failed peers are independent: awaiting retries serially would turn
        // N dead peers into N * 2 seconds of processor head-of-line blocking.
        // A bounded JoinSet keeps the worst-case recovery delay at two seconds.
        let mut retries = tokio::task::JoinSet::new();
        for (index, peer) in peers.iter().enumerate() {
            if matches!(outcomes.get(index), Some(Ok(()))) {
                initially_accepted += 1;
                continue;
            }
            if let Some(Err(error)) = outcomes.get(index) {
                self.log.warn(
                    self.log_source,
                    &format!(
                        "Block hash {block_hash}: initial handoff to {} failed: {error}",
                        peer.endpoint.host
                    ),
                );
            }
            let transport = self.transport.clone();
            let retry_peer = peer.clone();
            let retry_message = message.clone();
            retries.spawn(async move {
                let outcome = tokio::time::timeout(
                    Duration::from_secs(2),
                    transport.send(&retry_peer, retry_message),
                )
                .await;
                (retry_peer, outcome)
            });
        }
        while let Some(joined) = retries.join_next().await {
            match joined {
                Ok((_, Ok(Ok(())))) => retry_accepted += 1,
                Ok((peer, Ok(Err(error)))) => {
                    not_delivered += 1;
                    self.log.warn(
                        self.log_source,
                        &format!(
                            "Block hash {block_hash}: retry to {} failed: {error}",
                            peer.endpoint.host
                        ),
                    );
                }
                Ok((peer, Err(_))) => {
                    not_delivered += 1;
                    self.log.warn(
                        self.log_source,
                        &format!(
                            "Block hash {block_hash}: retry to {} timed out",
                            peer.endpoint.host
                        ),
                    );
                }
                Err(error) => {
                    not_delivered += 1;
                    self.log.warn(
                        self.log_source,
                        &format!("Block hash {block_hash}: retry task failed: {error}"),
                    );
                }
            }
        }
        self.log.info(
            self.log_source,
            &format!(
                "Block hash {block_hash}: transport accepted {initially_accepted} initial, {retry_accepted} retry; {not_delivered} failed of {} peers (transport acceptance is not remote DAG admission)",
                peers.len()
            ),
        );
    }

    /// Broadcast a has-block request to peers (port of `broadcastHasBlockRequest`).
    pub async fn broadcast_has_block_request(&self, hash: &BlockHash) {
        let packet = HasBlockRequestSerde.mk_packet(&HasBlockRequest { hash: *hash });
        self.send_to_peers(&packet, None).await;
    }

    /// Broadcast a request for a block and report **which peers it was sent to** — the per-hash
    /// attribution the join diagnostic needs (#457). `send_to_peers` picks a random subset, so the
    /// connection set is not the asked set; this returns the subset it actually used.
    pub async fn broadcast_request_for_block(
        &self,
        hash: &BlockHash,
        scope_size: Option<usize>,
    ) -> Vec<PeerNode> {
        let packet = BlockRequestSerde.mk_packet(&BlockRequest { hash: *hash });
        let max = scope_size.unwrap_or(self.conf.max_num_of_connections);
        let peers = {
            let connections = self.connections.read().await;
            random_connections(&connections, max)
        };
        let msg = protocol_helper::packet(&self.conf.local, &self.conf.network_id, packet);
        self.transport.broadcast(&peers, msg).await;
        peers
    }

    /// Request the fork-choice tip from peers (port of `sendForkChoiceTipRequest`).
    pub async fn send_fork_choice_tip_request(&self) {
        let packet = ForkChoiceTipRequestSerde.mk_packet(&ForkChoiceTipRequest);
        self.send_to_peers(&packet, None).await;
        self.log
            .info(self.log_source, "Requested fork tip from peers");
    }

    /// Retry until an authenticated nonempty LFS reply is admitted, not just sent.
    /// Startup must keep routing packets concurrently with this bounded future.
    pub async fn request_initial_fringe(
        &self,
        trim_state: bool,
        anchor: Option<BlockHash>,
        received: Arc<tokio::sync::Notify>,
    ) -> Result<(), String> {
        self.request_initial_fringe_with_policy(
            trim_state,
            anchor,
            received,
            Duration::from_secs(10),
            MAX_BOOTSTRAP_RETRIES,
        )
        .await
    }

    async fn request_initial_fringe_with_policy(
        &self,
        trim_state: bool,
        anchor: Option<BlockHash>,
        received: Arc<tokio::sync::Notify>,
        interval: Duration,
        attempts: u32,
    ) -> Result<(), String> {
        let bootstrap = self
            .conf
            .bootstrap
            .as_ref()
            .ok_or_else(|| StandaloneNodeSendToBootstrapError.to_string())?;
        let packet = FinalizedFringeRequestSerde.mk_packet(&FinalizedFringeRequest {
            identifier: String::new(),
            trim_state,
            include_fringe_metadata: true,
            anchor,
        });
        let msg = protocol_helper::packet(&self.conf.local, &self.conf.network_id, packet);
        // A stored notify_one permit also covers admission before this future is polled.
        let response = received.notified();
        tokio::pin!(response);
        for attempt in 1..=attempts {
            tokio::select! {
                biased;
                _ = &mut response => return Ok(()),
                outcome = tokio::time::timeout(interval, self.transport.send(bootstrap, msg.clone())) => {
                    match outcome {
                        Ok(Ok(())) => self.log.info(self.log_source, &format!(
                            "LFS fringe request {attempt}/{attempts} sent; awaiting admitted reply from {bootstrap}"
                        )),
                        Ok(Err(error)) => self.log.warn(self.log_source, &format!(
                            "LFS fringe request {attempt}/{attempts} failed: {error:?}"
                        )),
                        Err(_) => self.log.warn(self.log_source, &format!(
                            "LFS fringe request {attempt}/{attempts} send timed out"
                        )),
                    }
                }
            }
            tokio::select! {
                biased;
                _ = &mut response => return Ok(()),
                _ = tokio::time::sleep(interval) => {}
            }
        }
        Err(format!(
            "LFS bootstrap supplied no authenticated nonempty fringe after {attempts} requests; refusing to enter Running"
        ))
    }

    /// Request the finalized fringe from the bootstrap node (port of `requestFinalizedFringe`).
    ///
    /// `include_fringe_metadata` asks the responder to attach the per-block fringe state of the
    /// fringe's ancestry (AUDIT C188, #139). A joining node cannot derive it — it is a node's own
    /// recomputation and not a block field — so without it every block it restores whose
    /// `close_block` anchors the epoch seed to the fringe state replays to a different post-state,
    /// and the node never catches up. A responder that does not know the field ignores it, so a
    /// mixed pair falls back to the pre-#139 behaviour rather than failing to sync.
    ///
    /// `anchor` names the block the sync is to be rooted at (C259). `None` is the ordinary request.
    /// `Some(h)` is a recovery sync: the responder builds the fringe, and its ancestry, from block `h`
    /// instead of from its own latest fringe — which a peer with finality frozen does not have, and
    /// which it silently replaces with the genesis block. Without the anchor a wiped joiner syncs to
    /// block 0 and can never re-enter the sync path.
    pub async fn request_finalized_fringe(
        &self,
        trim_state: bool,
        include_fringe_metadata: bool,
        anchor: Option<BlockHash>,
    ) -> Result<(), StandaloneNodeSendToBootstrapError> {
        let bootstrap = self
            .conf
            .bootstrap
            .clone()
            .ok_or(StandaloneNodeSendToBootstrapError)?;
        let msg = FinalizedFringeRequest {
            identifier: String::new(),
            trim_state,
            include_fringe_metadata,
            anchor,
        };
        let packet = FinalizedFringeRequestSerde.mk_packet(&msg);
        self.send_with_retry(
            &packet,
            &bootstrap,
            Duration::from_secs(10),
            "FinalizedFringeRequest",
        )
        .await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use rchain_comm::errors::CommErr;
    use rchain_comm::peer_node::NodeIdentifier;
    use rchain_comm::rp::rp_conf::ClearConnectionsConf;
    use rchain_models::casper::protocol::packet_type_tag::FromPacket;
    use rchain_shared::log::NopLog;

    /// Records every call and fails `send` while `fail_sends` is positive. The retry bound is a
    /// deviation from Scala (which retries forever), so it needs a transport that really fails —
    /// a mock that always succeeds cannot reach the loop's give-up arm at all.
    #[derive(Default)]
    struct MockTransport {
        sends: std::sync::Mutex<Vec<(PeerNode, Protocol)>>,
        broadcasts: std::sync::Mutex<Vec<(Vec<PeerNode>, Protocol)>>,
        fail_broadcast_to: std::sync::Mutex<Option<Vec<u8>>>,
        streams: std::sync::Mutex<Vec<(Vec<PeerNode>, Blob)>>,
        fail_sends: std::sync::atomic::AtomicUsize,
        reply_on_send: usize,
        reply: Option<Arc<tokio::sync::Notify>>,
        hang_send: bool,
    }

    impl MockTransport {
        fn failing(times: usize) -> Self {
            let t = MockTransport::default();
            t.fail_sends
                .store(times, std::sync::atomic::Ordering::SeqCst);
            t
        }
        fn send_count(&self) -> usize {
            self.sends.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl TransportLayer for MockTransport {
        async fn send(&self, peer: &PeerNode, msg: Protocol) -> CommErr<()> {
            self.sends.lock().unwrap().push((peer.clone(), msg));
            // `usize::MAX` is the "always fails" setting, and the only one that reaches the bound.
            if self.hang_send {
                std::future::pending::<()>().await;
            }
            if self.send_count() == self.reply_on_send {
                if let Some(reply) = &self.reply {
                    reply.notify_one();
                }
            }
            let remaining = self.fail_sends.load(std::sync::atomic::Ordering::SeqCst);
            if remaining > 0 {
                self.fail_sends
                    .store(remaining - 1, std::sync::atomic::Ordering::SeqCst);
                Err(rchain_comm::errors::CommError::PeerUnavailable(
                    peer.clone(),
                    "the test transport was told to fail".to_string(),
                ))
            } else {
                Ok(())
            }
        }
        async fn broadcast(&self, peers: &[PeerNode], msg: Protocol) -> Vec<CommErr<()>> {
            self.broadcasts.lock().unwrap().push((peers.to_vec(), msg));
            let failing = self.fail_broadcast_to.lock().unwrap().clone();
            peers
                .iter()
                .map(|peer| {
                    if failing.as_deref() == Some(peer.id.key()) {
                        Err(rchain_comm::errors::CommError::PeerUnavailable(
                            peer.clone(),
                            "simulated broadcast failure".to_string(),
                        ))
                    } else {
                        Ok(())
                    }
                })
                .collect()
        }
        async fn stream(&self, peers: &[PeerNode], blob: Blob) {
            self.streams.lock().unwrap().push((peers.to_vec(), blob));
        }
    }

    fn peer(name: &str, port: u16) -> PeerNode {
        PeerNode::from(
            NodeIdentifier::new(name.as_bytes().to_vec()),
            "host".to_string(),
            rchain_shared::refined::Port::new(port),
            rchain_shared::refined::Port::new(port),
        )
    }

    fn conf(local: &PeerNode, bootstrap: Option<PeerNode>, max: usize) -> RPConf {
        RPConf {
            local: local.clone(),
            network_id: "testnet".to_string(),
            bootstrap,
            default_timeout: Duration::from_secs(10),
            max_num_of_connections: max,
            clear_connections: ClearConnectionsConf {
                num_of_connections_pinged: 10,
            },
        }
    }

    /// `CommUtil` over the mock, with `n` connections already established.
    fn comm_util_with(
        transport: Arc<MockTransport>,
        conf: RPConf,
        connections: Vec<PeerNode>,
    ) -> CommUtil {
        CommUtil::new(
            transport,
            conf,
            Arc::new(tokio::sync::RwLock::new(connections)),
            Arc::new(NopLog),
        )
    }

    fn packet(type_id: &str) -> Packet {
        Packet {
            type_id: type_id.to_string(),
            content: Vec::new(),
        }
    }

    /// Decode a protocol back to its packet, so an assertion can name the wire type the caller
    /// actually sent rather than trusting the wrapper.
    fn unpack(proto: &Protocol) -> Packet {
        protocol_helper::to_packet(proto).expect("the broadcast carries a packet")
    }

    fn hash(byte: u8) -> BlockHash {
        BlockHash::new([byte; 32])
    }

    #[tokio::test]
    async fn initial_fringe_retries_an_acknowledged_request_whose_reply_was_lost() {
        let local = peer("local", 40400);
        let bootstrap = peer("bootstrap", 40401);
        let received = Arc::new(tokio::sync::Notify::new());
        let transport = Arc::new(MockTransport {
            reply_on_send: 2,
            reply: Some(received.clone()),
            ..MockTransport::default()
        });
        let comm = comm_util_with(
            transport.clone(),
            conf(&local, Some(bootstrap.clone()), 10),
            vec![],
        );
        tokio::time::timeout(
            Duration::from_secs(2),
            comm.request_initial_fringe_with_policy(
                true,
                Some(hash(7)),
                received,
                Duration::from_millis(1),
                3,
            ),
        )
        .await
        .expect("bounded retry")
        .expect("second reply admitted");
        let sends = transport.sends.lock().unwrap();
        assert_eq!(sends.len(), 2, "send success alone must not stop retry");
        for (peer, proto) in sends.iter() {
            assert_eq!(peer, &bootstrap);
            let req = FinalizedFringeRequestSerde
                .parse_from(&unpack(proto))
                .expect("fringe request");
            assert!(req.trim_state);
            assert!(req.include_fringe_metadata);
            assert_eq!(
                req.anchor,
                Some(hash(7)),
                "retry must preserve the operator anchor"
            );
        }
    }

    #[tokio::test]
    async fn initial_fringe_missing_reply_is_terminal_even_when_all_sends_succeed() {
        let local = peer("local", 40400);
        let transport = Arc::new(MockTransport::default());
        let comm = comm_util_with(
            transport.clone(),
            conf(&local, Some(peer("bootstrap", 40401)), 10),
            vec![],
        );
        let outcome = tokio::time::timeout(
            Duration::from_secs(2),
            comm.request_initial_fringe_with_policy(
                false,
                None,
                Arc::new(tokio::sync::Notify::new()),
                Duration::from_millis(1),
                3,
            ),
        )
        .await
        .expect("no indefinite startup");
        assert!(outcome
            .expect_err("no reply is not success")
            .contains("after 3 requests"));
        assert_eq!(transport.send_count(), 3);
    }

    #[tokio::test]
    async fn initial_fringe_admission_before_polling_is_not_lost_and_sends_nothing() {
        let local = peer("local", 40400);
        let transport = Arc::new(MockTransport::default());
        let comm = comm_util_with(
            transport.clone(),
            conf(&local, Some(peer("bootstrap", 40401)), 10),
            vec![],
        );
        let received = Arc::new(tokio::sync::Notify::new());
        received.notify_one();
        comm.request_initial_fringe_with_policy(false, None, received, Duration::from_millis(1), 3)
            .await
            .expect("stored admission permit");
        assert_eq!(transport.send_count(), 0);
    }

    #[tokio::test]
    async fn initial_fringe_hung_sends_exhaust_the_same_bound() {
        let local = peer("local", 40400);
        let transport = Arc::new(MockTransport {
            hang_send: true,
            ..MockTransport::default()
        });
        let comm = comm_util_with(
            transport.clone(),
            conf(&local, Some(peer("bootstrap", 40401)), 10),
            vec![],
        );
        let outcome = tokio::time::timeout(
            Duration::from_secs(2),
            comm.request_initial_fringe_with_policy(
                false,
                None,
                Arc::new(tokio::sync::Notify::new()),
                Duration::from_millis(1),
                3,
            ),
        )
        .await
        .expect("hung send must be bounded");
        assert!(outcome.is_err());
        assert_eq!(transport.send_count(), 3);
    }

    #[tokio::test]
    async fn initial_fringe_without_bootstrap_refuses_before_sending() {
        let local = peer("local", 40400);
        let transport = Arc::new(MockTransport::default());
        let comm = comm_util_with(transport.clone(), conf(&local, None, 10), vec![]);
        assert!(comm
            .request_initial_fringe(false, None, Arc::new(tokio::sync::Notify::new()))
            .await
            .is_err());
        assert_eq!(transport.send_count(), 0);
    }

    #[tokio::test]
    async fn request_finalized_fringe_is_an_error_when_there_is_no_bootstrap() {
        let local = peer("local", 40400);
        let transport = Arc::new(MockTransport::default());
        let comm = comm_util_with(transport.clone(), conf(&local, None, 10), Vec::new());

        let err = comm
            .request_finalized_fringe(false, false, None)
            .await
            .expect_err("a standalone node has no bootstrap to ask");
        assert_eq!(err, StandaloneNodeSendToBootstrapError);
        assert_eq!(
            err.to_string(),
            "standalone node cannot send to the bootstrap node"
        );
        // The guard is a precondition, not a filter: nothing was put on the wire.
        assert_eq!(transport.send_count(), 0);
    }

    #[tokio::test]
    async fn request_finalized_fringe_asks_the_bootstrap_for_the_fringe() {
        let local = peer("local", 40400);
        let bootstrap = peer("bootstrap", 40401);
        let transport = Arc::new(MockTransport::default());
        let comm = comm_util_with(
            transport.clone(),
            conf(&local, Some(bootstrap.clone()), 10),
            Vec::new(),
        );

        comm.request_finalized_fringe(true, false, None)
            .await
            .expect("a bootstrap");
        let sends = transport.sends.lock().unwrap();
        assert_eq!(sends.len(), 1, "one successful attempt, then stop");
        assert_eq!(sends[0].0, bootstrap, "the request goes to the bootstrap");
        assert_eq!(unpack(&sends[0].1).type_id, "FinalizedFringeRequest");
        // The retry loop's own behaviour is covered by the zero-delay tests below; driving it from
        // here would sleep the hard-coded ten seconds between attempts.
    }

    /// The give-up arm: a transport that never succeeds must not hang the caller. Scala's
    /// `keepOnRequestingTillRunning` retries forever (AUDIT.md), so this bound is a deviation and
    /// the test is what keeps it from silently disappearing.
    #[tokio::test]
    async fn send_with_retry_gives_up_after_the_bounded_number_of_attempts() {
        let local = peer("local", 40400);
        let remote = peer("remote", 40402);
        let transport = Arc::new(MockTransport::failing(usize::MAX));
        let comm = comm_util_with(transport.clone(), conf(&local, None, 10), Vec::new());

        // A zero retry delay keeps the ten attempts instantaneous; the bound, not the wait, is
        // what is under test.
        comm.send_with_retry(
            &packet("BlockRequest"),
            &remote,
            Duration::ZERO,
            "BlockRequest",
        )
        .await;
        assert_eq!(
            transport.send_count(),
            MAX_BOOTSTRAP_RETRIES as usize,
            "the loop is bounded, and the bound is the named constant"
        );
    }

    #[tokio::test]
    async fn send_with_retry_recovers_after_a_transient_failure() {
        let local = peer("local", 40400);
        let remote = peer("remote", 40402);
        let transport = Arc::new(MockTransport::failing(2));
        let comm = comm_util_with(transport.clone(), conf(&local, None, 10), Vec::new());

        comm.send_with_retry(
            &packet("BlockRequest"),
            &remote,
            Duration::ZERO,
            "BlockRequest",
        )
        .await;
        assert_eq!(
            transport.send_count(),
            3,
            "two retries then the attempt that succeeds"
        );
    }

    #[tokio::test]
    async fn send_with_retry_stops_at_the_first_success() {
        let local = peer("local", 40400);
        let remote = peer("remote", 40402);
        let transport = Arc::new(MockTransport::default());
        let comm = comm_util_with(transport.clone(), conf(&local, None, 10), Vec::new());

        comm.send_with_retry(
            &packet("BlockRequest"),
            &remote,
            Duration::ZERO,
            "BlockRequest",
        )
        .await;
        assert_eq!(transport.send_count(), 1, "one attempt, then stop");
    }

    #[tokio::test]
    async fn send_to_peers_caps_the_scope_and_defaults_to_the_configured_maximum() {
        let local = peer("local", 40400);
        let transport = Arc::new(MockTransport::default());
        let connections: Vec<PeerNode> = (0..12)
            .map(|i| peer(&format!("p{i}"), 40400 + i as u16))
            .collect();
        let comm = comm_util_with(
            transport.clone(),
            conf(&local, None, 10),
            connections.clone(),
        );

        comm.send_to_peers(&packet("ForkChoiceTipRequest"), Some(2))
            .await;
        comm.send_to_peers(&packet("ForkChoiceTipRequest"), None)
            .await;

        let broadcasts = transport.broadcasts.lock().unwrap();
        assert_eq!(broadcasts.len(), 2);
        assert_eq!(broadcasts[0].0.len(), 2, "an explicit scope is honoured");
        assert_eq!(
            broadcasts[1].0.len(),
            10,
            "no scope means the configured maximum, not every connection"
        );
        // The peers are drawn from the connection table (randomly ordered, hence the set compare).
        for sent in broadcasts[0].0.iter() {
            assert!(connections.contains(sent), "{sent:?} is not a connection");
        }
    }

    /// A node with no connections broadcasts to nobody. The call still reaches the transport
    /// (matching Scala, which does not special-case the empty list) — what matters is that the
    /// peer list is empty, so no send is attempted.
    #[tokio::test]
    async fn send_to_peers_over_no_connections_addresses_nobody() {
        let local = peer("local", 40400);
        let transport = Arc::new(MockTransport::default());
        let comm = comm_util_with(transport.clone(), conf(&local, None, 10), Vec::new());

        comm.send_to_peers(&packet("BlockHashMessage"), None).await;
        let broadcasts = transport.broadcasts.lock().unwrap();
        assert_eq!(broadcasts.len(), 1);
        assert!(broadcasts[0].0.is_empty(), "no connections, no recipients");
    }

    #[tokio::test]
    async fn stream_to_peers_streams_a_blob_to_the_scoped_peers() {
        let local = peer("local", 40400);
        let transport = Arc::new(MockTransport::default());
        let connections: Vec<PeerNode> = (0..5)
            .map(|i| peer(&format!("p{i}"), 40400 + i as u16))
            .collect();
        let comm = comm_util_with(
            transport.clone(),
            conf(&local, None, 2),
            connections.clone(),
        );

        comm.stream_to_peers(&packet("BlockResponse"), None).await;
        let streams = transport.streams.lock().unwrap();
        assert_eq!(streams.len(), 1);
        assert_eq!(
            streams[0].0.len(),
            2,
            "the configured maximum applies to a stream too, not just a broadcast"
        );
        for sent in &streams[0].0 {
            assert!(connections.contains(sent), "{sent:?} is not a connection");
        }
        // The blob carries the sender and the packet: a stream that lost either would be
        // undecodable at the receiver.
        assert_eq!(streams[0].1.sender, local);
        assert_eq!(streams[0].1.packet.type_id, "BlockResponse");
    }

    /// The syntax wrappers each name themselves on the wire; a copy-paste that reuses the wrong
    /// serde would still "broadcast something", so the type id is asserted, not the count alone.
    #[tokio::test]
    async fn the_syntax_wrappers_broadcast_their_own_packet_type() {
        let local = peer("local", 40400);
        let transport = Arc::new(MockTransport::default());
        let comm = comm_util_with(
            transport.clone(),
            conf(&local, None, 10),
            vec![peer("p", 40401)],
        );

        comm.send_block_hash(&hash(1), &[9u8; 65]).await;
        comm.broadcast_has_block_request(&hash(2)).await;
        comm.broadcast_request_for_block(&hash(3), Some(1)).await;
        comm.send_fork_choice_tip_request().await;

        let broadcasts = transport.broadcasts.lock().unwrap();
        let ids: Vec<String> = broadcasts
            .iter()
            .map(|(peers, proto)| {
                assert_eq!(peers.len(), 1, "one connection, one recipient");
                unpack(proto).type_id
            })
            .collect();
        assert_eq!(
            ids,
            [
                "BlockHashMessage",
                "HasBlockRequest",
                "BlockRequest",
                "ForkChoiceTipRequest"
            ],
            "each wrapper names itself on the wire"
        );
    }

    /// A failure in a multi-peer broadcast must not be logged as blanket
    /// success. Only the failed recipient is retried, with the same signed
    /// network envelope and hash announcement.
    #[tokio::test]
    async fn block_hash_broadcast_retries_only_the_failed_peer() {
        let local = peer("local", 40400);
        let p0 = peer("p0", 40401);
        let p1 = peer("p1", 40402);
        let p2 = peer("p2", 40403);
        let transport = Arc::new(MockTransport::default());
        *transport.fail_broadcast_to.lock().unwrap() = Some(p1.id.key().to_vec());
        let comm = comm_util_with(
            transport.clone(),
            conf(&local, None, 8),
            vec![p0, p1.clone(), p2],
        );

        comm.send_block_hash(&hash(9), &[3u8; 65]).await;

        let broadcasts = transport.broadcasts.lock().unwrap();
        assert_eq!(broadcasts.len(), 1);
        assert_eq!(broadcasts[0].0.len(), 3);
        let sends = transport.sends.lock().unwrap();
        assert_eq!(sends.len(), 1, "retry only the failed target");
        assert_eq!(sends[0].0, p1);
        let retried = BlockHashMessageSerde
            .parse_from(&unpack(&sends[0].1))
            .expect("same wire type");
        assert_eq!(retried.block_hash, hash(9));
        assert_eq!(retried.block_creator, vec![3u8; 65]);
        assert_eq!(
            sends[0].1.header.as_ref().expect("envelope").network_id,
            "testnet"
        );
    }

    /// A permanently unavailable peer receives at most one bounded retry;
    /// no other validator is dropped from the broadcast.
    #[tokio::test]
    async fn block_hash_broadcast_does_not_retry_forever() {
        let local = peer("local", 40400);
        let failing = peer("failing", 40401);
        let healthy = peer("healthy", 40402);
        let transport = Arc::new(MockTransport::failing(usize::MAX));
        *transport.fail_broadcast_to.lock().unwrap() = Some(failing.id.key().to_vec());
        let comm = comm_util_with(
            transport.clone(),
            conf(&local, None, 8),
            vec![failing.clone(), healthy],
        );

        comm.send_block_hash(&hash(8), &[1u8; 65]).await;
        assert_eq!(transport.broadcasts.lock().unwrap().len(), 1);
        let sends = transport.sends.lock().unwrap();
        assert_eq!(sends.len(), 1, "exactly one failed retry");
        assert_eq!(sends[0].0, failing);
    }

    #[tokio::test]
    async fn request_for_block_targets_one_peer_and_names_the_block() {
        let local = peer("local", 40400);
        let remote = peer("remote", 40402);
        let transport = Arc::new(MockTransport::default());
        let comm = comm_util_with(transport.clone(), conf(&local, None, 10), Vec::new());

        comm.request_for_block(&remote, &hash(7)).await;
        let sends = transport.sends.lock().unwrap();
        assert_eq!(sends.len(), 1, "a block request is unicast, not broadcast");
        assert_eq!(sends[0].0, remote);
        assert_eq!(unpack(&sends[0].1).type_id, "BlockRequest");
    }

    /// `protocol_helper::packet` is what stamps the sender and network id on every outgoing
    /// message, so a message that skipped it would be rejected by a peer's routing layer.
    #[tokio::test]
    async fn an_outgoing_block_hash_carries_the_block_hash_and_its_creator() {
        let local = peer("local", 40400);
        let transport = Arc::new(MockTransport::default());
        let comm = comm_util_with(
            transport.clone(),
            conf(&local, None, 10),
            vec![peer("p", 40401)],
        );

        comm.send_block_hash(&hash(3), &[9u8; 65]).await;
        let broadcasts = transport.broadcasts.lock().unwrap();
        let decoded = BlockHashMessageSerde
            .parse_from(&unpack(&broadcasts[0].1))
            .expect("the advertised hash round-trips");
        assert_eq!(decoded.block_hash, hash(3));
        assert_eq!(decoded.block_creator, vec![9u8; 65]);
        // The header is what the receiver authenticates: it must name this node and network.
        let header = broadcasts[0]
            .1
            .header
            .clone()
            .expect("a packet always carries a header");
        assert_eq!(
            header.sender.clone().expect("sender").id,
            local.id.key().to_vec()
        );
        assert_eq!(header.network_id, "testnet");
    }
}
