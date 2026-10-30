//! Inbound protocol message dispatch.
//!
//! Mirrors `comm/src/main/scala/coop/rchain/comm/rp/HandleMessages.scala`. The fs2 routing-message
//! queue becomes a `tokio::sync::mpsc::Sender`.

use std::net::IpAddr;

use rchain_models::comm::protocol::{protocol, Packet, Protocol};
use rchain_shared::log::{Log, LogSource};

use crate::errors::CommError;
use crate::peer_node::PeerNode;
use crate::rp::connect::{add_conn, remove_conn};
use crate::rp::protocol_helper;
use crate::rp::rp_conf::RPConf;
use crate::transport::communication_response::CommunicationResponse;
use crate::transport::transport_layer::TransportLayer;

/// The log source for the inbound path, so a diagnostic run can find these lines by name.
const INBOUND: LogSource = LogSource::new("coop.rchain.comm.inbound");

/// A short name for an inbound message, for the inbound line.
fn message_name(proto: &Protocol) -> &'static str {
    match &proto.message {
        Some(protocol::Message::Heartbeat(_)) => "Heartbeat",
        Some(protocol::Message::ProtocolHandshake(_)) => "ProtocolHandshake",
        Some(protocol::Message::ProtocolHandshakeResponse(_)) => "ProtocolHandshakeResponse",
        Some(protocol::Message::Disconnect(_)) => "Disconnect",
        Some(protocol::Message::Packet(_)) => "Packet",
        None => "no message",
    }
}

/// A routing packet addressed from a peer (port of `RoutingMessage`).
#[derive(Clone, Debug)]
pub struct RoutingMessage {
    pub peer: PeerNode,
    pub packet: Packet,
}

/// Whether a host is a local/private address (port of `HandleMessages.isLocalAddress`). Classifies
/// both IPv4 and IPv6 literals. Hostnames are not resolved (see the residual note below).
pub fn is_local_address(host: &str) -> bool {
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            let o = ip.octets();
            ip.is_unspecified() // 0.0.0.0
                || ip.is_loopback() // 127/8
                || ip.is_multicast() // 224/4
                || (o[0] == 169 && o[1] == 254) // link-local 169.254/16
                || o[0] == 10 // 10/8
                || (o[0] == 172 && (16..=31).contains(&o[1])) // 172.16/12
                || (o[0] == 192 && o[1] == 168) // 192.168/16
        }
        Ok(IpAddr::V6(ip)) => {
            ip.is_unspecified() // ::
                || ip.is_loopback() // ::1
                || ip.is_multicast() // ff00::/8
                || ip.is_unique_local() // fc00::/7
                || ip.is_unicast_link_local() // fe80::/10
        }
        // A hostname is not an address, so this cannot classify it. Callers that accept a host from
        // a peer must use [`is_local_address_resolved`], which is the same rule with the name
        // resolved first (AUDIT C117).
        Err(_) => false,
    }
}

/// Whether a host is local/private, **resolving a hostname first** (AUDIT C117).
///
/// [`is_local_address`] classifies IP literals and nothing else, and that was the whole guard on the
/// Kademlia ingress — so a peer could supply `localhost`, or any DNS name whose A record points at
/// `127.0.0.1` or `10.0.0.5`, and the routing table would take it and the node would later **dial it**.
/// The residual was recorded honestly ("a hostname that resolves to a private/loopback address is not
/// classified here") and the risk was real all the same: the guard's job is to stop a peer aiming the
/// node at its own network, and a name is a way of aiming.
///
/// **Fail-closed at both ends.** Any resolved address being local/private refuses the host — a name
/// with one public and one private record is not mostly safe, it is a name that reaches a private
/// address. And a name that does not resolve at all is refused too: it is not a place to dial, and
/// treating "could not find out" as "probably fine" is the reading that cannot be defended.
///
/// **What this does not close, stated rather than implied**: the check and the dial are separate
/// resolutions, so an attacker controlling a name with a short TTL can answer public here and private
/// there. Closing that means pinning the resolved address into the routing table and dialling *that*,
/// which is a change to what a `PeerNode` carries rather than to this function. The window is
/// narrowed from "any name works, always" to "the name must lie at exactly the right moment".
pub async fn is_local_address_resolved(host: &str) -> bool {
    if host.parse::<IpAddr>().is_ok() {
        return is_local_address(host);
    }
    match tokio::net::lookup_host((host, 0)).await {
        Ok(addrs) => {
            let mut seen = false;
            for addr in addrs {
                seen = true;
                if is_local_address(&addr.ip().to_string()) {
                    return true;
                }
            }
            !seen
        }
        Err(_) => true,
    }
}

/// Whether `peer` is on the same subnetwork class (public vs local) as the local node (port of
/// `checkPeerOnSameNetwork`).
pub fn check_peer_on_same_network(conf: &RPConf, peer: &PeerNode) -> bool {
    is_local_address(&conf.local.endpoint.host) == is_local_address(&peer.endpoint.host)
}

/// Dispatch an inbound protocol message (port of `handle`).
///
/// The connections cell is an `RwLock` rather than `&mut Vec` so the write lock is held only for the
/// brief mutation, never across the outbound `send` in the handshake path (H3).
///
/// **The spoofing residual that used to be recorded here is closed** (AUDIT C115). The protocol
/// `sender` was taken from the message header and never compared to the TLS peer certificate, so a
/// peer presenting any self-signed certificate could assert an arbitrary node id. The receiver now
/// carries the identity the client certificate proves into the request
/// (`grpc_transport_receiver::PeerId`) and refuses a message whose header claims a different one, on
/// the unary and streamed paths alike. This function still reads the sender from the header — that is
/// what the field is for — but the value has been checked against the certificate by the time it
/// arrives, so what it names is what connected.
pub async fn handle<T: TransportLayer + ?Sized>(
    proto: Protocol,
    conf: &RPConf,
    transport: &T,
    connections: &tokio::sync::RwLock<Vec<PeerNode>>,
    routing_queue: &tokio::sync::mpsc::Sender<RoutingMessage>,
    log: &dyn Log,
) -> CommunicationResponse {
    let sender = match protocol_helper::sender(&proto) {
        Ok(s) => s,
        Err(e) => return CommunicationResponse::not_handled(e),
    };

    // **What this node received, before any decision about it.** The inbound path used to log nothing
    // at all, which made three different outcomes indistinguishable from outside: a message that
    // never arrived, one that arrived and was dropped by a gate below, and one that arrived and was
    // routed somewhere with no consumer. Diagnosing issue #100 (a joining validator's
    // `FinalizedFringeRequest` reaching the bootstrap and vanishing) cost a devnet run per hypothesis
    // for exactly this reason. One line per message, at debug.
    log.debug(
        INBOUND,
        &format!("Received {} from {}", message_name(&proto), sender.id),
    );
    match proto.message {
        Some(protocol::Message::Heartbeat(_)) => {
            // A receiver can restart while its peers still consider the old connection live.
            // A certificate-verified, same-network heartbeat is fresh evidence of the sender:
            // restore the connection just as an inbound handshake would, rather than only
            // refreshing a record that was lost at restart.
            if check_peer_on_same_network(conf, &sender) {
                let mut conns = connections.write().await;
                *conns = add_conn(&*conns, &[sender]);
            }
            CommunicationResponse::handled_without_message()
        }
        Some(protocol::Message::ProtocolHandshake(_)) => {
            handle_protocol_handshake(transport, conf, connections, &sender, log).await
        }
        Some(protocol::Message::ProtocolHandshakeResponse(_)) => {
            let mut conns = connections.write().await;
            *conns = add_conn(&*conns, &[sender]);
            CommunicationResponse::handled_without_message()
        }
        Some(protocol::Message::Disconnect(_)) => {
            let mut conns = connections.write().await;
            *conns = remove_conn(&*conns, &[sender]);
            CommunicationResponse::handled_without_message()
        }
        Some(protocol::Message::Packet(packet)) => {
            // **Backpressure, not a silent drop** (AUDIT C105). `try_send` returns `Full` when the
            // routing queue is busy, and the error was *discarded* — so the packet was lost while this
            // handler answered `HandledWithoutMessage` and the transport acked the peer as though the
            // packet had been taken. Awaiting is the policy the streamed path already uses
            // (`node_runtime.rs`'s `handle_streamed`), one hop earlier: a peer's packet is not thrown
            // away because the node is momentarily busy.
            //
            // **The alternative — making the ack mean "processed" rather than "received" — was
            // rejected on shape, not on principle**: the receiver `tokio::spawn`s the dispatch, so
            // acting on its result means awaiting it inline, which would put a *peer round-trip* (the
            // handshake path's outbound send) inside the RPC handler's critical path. What a packet
            // needs is not to be lost; what an ack means for every other message on this path is
            // "taken into the pipeline", and that is what it still means.
            match routing_queue
                .send(RoutingMessage {
                    peer: sender.clone(),
                    packet,
                })
                .await
            {
                Ok(()) => {
                    // A peer with an already-established outbound connection will not resend
                    // a handshake when this node restarts. Its authenticated inbound packet
                    // proves the same sender identity, and the routing queue accepted it.
                    // Recover the *in-memory* connection without altering the transport
                    // identity check, locality policy or the bounded peer-table capacity.
                    if check_peer_on_same_network(conf, &sender) {
                        let mut conns = connections.write().await;
                        *conns = add_conn(&*conns, &[sender]);
                    }
                    CommunicationResponse::handled_without_message()
                }
                // The router task is gone, so this packet cannot be delivered *at all*. Answering
                // "handled" would be the same lie one layer down, so say so instead.
                Err(_) => {
                    CommunicationResponse::not_handled(CommError::InternalCommunicationError(
                        "the peer-message router is not running".to_string(),
                    ))
                }
            }
        }
        other => {
            CommunicationResponse::not_handled(CommError::UnexpectedMessage(format!("{other:?}")))
        }
    }
}

/// Handle an inbound protocol handshake (port of `handleProtocolHandshake`): accept only peers on
/// the same subnetwork class, respond with a handshake response, and record the connection. The
/// response `send` runs *before* the connections lock is taken, so a slow peer cannot stall the
/// connection table (H3).
pub async fn handle_protocol_handshake<T: TransportLayer + ?Sized>(
    transport: &T,
    conf: &RPConf,
    connections: &tokio::sync::RwLock<Vec<PeerNode>>,
    peer: &PeerNode,
    log: &dyn Log,
) -> CommunicationResponse {
    if check_peer_on_same_network(conf, peer) {
        // **The peer is recorded when its handshake arrives, not when the reply to it succeeds**
        // (issue #100). Those used to be the same event, and they are not:
        //
        // A joining node dials its bootstrap *before its own protocol server has bound*, so the
        // bootstrap's reply is refused at the TCP layer — measured: `Connection refused (os error
        // 111)`, 157 ms before the joining node logged that it had started syncing. Under the old
        // shape that refusal lost the peer **permanently**: `add_conn` was inside the reply's
        // `is_ok()` arm, the dialler meanwhile considered itself connected because *its own* send had
        // been acked, and nothing retried the handshake. The network then had two nodes that each
        // thought the other was a peer, one connection table with the other in it and one empty — and
        // because the empty side drops what arrives from a peer it has not recorded, the joining
        // node's `FinalizedFringeRequest` reached this node and was routed nowhere. That is the whole
        // of #100: a deadlock from genesis, with no panic and no error.
        //
        // A peer that has just proved possession of its identity (the transport checked that against
        // its certificate before this was called) is a peer. The reply is how the *dialler* learns it
        // was accepted, and losing it costs the dialler a retry — not the connection. The brief lock
        // is released before any I/O, which is what H3 asks for.
        {
            let mut conns = connections.write().await;
            *conns = add_conn(&*conns, std::slice::from_ref(peer));
        }
        let response = protocol_helper::protocol_handshake_response(&conf.local, &conf.network_id);
        if let Err(err) = transport.send(peer, response).await {
            // Bounded and expected: the dialler may not be listening yet. It is a warning rather than
            // an error because the connection is recorded either way, and the dialler retries.
            log.warn(
                INBOUND,
                &format!(
                    "Could not answer {peer}'s handshake ({err:?}); it is recorded as a peer anyway \
                     and will retry"
                ),
            );
        }
    } else {
        // **Refused on the subnetwork class, and it used to say nothing.** A peer on a different
        // class gets no response and is not recorded, so from its side the handshake succeeded (the
        // RPC is acked) while from this side nothing happened at all.
        log.warn(
            INBOUND,
            &format!(
                "Refusing {peer}: it is on a different subnetwork class than this node \
                 (local host {}, peer host {})",
                conf.local.endpoint.host, peer.endpoint.host
            ),
        );
    }
    CommunicationResponse::handled_without_message()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_shared::log::NopLog;

    /// A transport that is never called: the packet path enqueues for the router and touches no peer,
    /// so a mock that refuses everything is the honest shape for these tests.
    struct NoTransport;

    #[async_trait::async_trait]
    impl crate::transport::transport_layer::TransportLayer for NoTransport {
        async fn send(&self, _peer: &PeerNode, _msg: Protocol) -> crate::errors::CommErr<()> {
            Err(CommError::InternalCommunicationError(
                "NoTransport: the test does not send".to_string(),
            ))
        }

        async fn broadcast(
            &self,
            _peers: &[PeerNode],
            _msg: Protocol,
        ) -> Vec<crate::errors::CommErr<()>> {
            Vec::new()
        }

        async fn stream(&self, _peers: &[PeerNode], _blob: crate::transport::chunker::Blob) {}
    }

    fn conf_for(local: &PeerNode) -> RPConf {
        RPConf {
            local: local.clone(),
            network_id: "testnet".to_string(),
            bootstrap: None,
            default_timeout: std::time::Duration::from_secs(10),
            max_num_of_connections: 10,
            clear_connections: crate::rp::rp_conf::ClearConnectionsConf {
                num_of_connections_pinged: 10,
            },
        }
    }

    /// **A packet is not dropped when the routing queue is full** (AUDIT C105).
    ///
    /// `try_send` returned `Full` under load and the error was discarded — the packet was lost while
    /// this handler answered `HandledWithoutMessage`, so the transport acked the peer for a packet the
    /// node had thrown away. The tell is that the handler must *wait* for room: with the old code it
    /// returns immediately (the first `select!` arm below fires), with the fix it delivers what it was
    /// holding as soon as a slot frees.
    #[tokio::test]
    async fn a_packet_is_not_dropped_when_the_routing_queue_is_full() {
        let local = peer("local", "127.0.0.1");
        let conf = conf_for(&local);
        let (tx, mut rx) = tokio::sync::mpsc::channel::<RoutingMessage>(1);

        // Fill the queue, so the handler's send *must* wait.
        let filler_peer = peer("filler", "10.0.0.9");
        tx.send(RoutingMessage {
            peer: filler_peer.clone(),
            packet: Default::default(),
        })
        .await
        .expect("the filler goes in");

        let conns = tokio::sync::RwLock::new(Vec::new());
        let proto = protocol_helper::packet(&local, &conf.network_id, Default::default());
        let mut handler = Box::pin(handle(proto, &conf, &NoTransport, &conns, &tx, &NopLog));

        // With the queue full the handler must still be *waiting*. On the old code it has already
        // returned — having dropped the packet and answered "handled" — which is the defect.
        if let Ok(response) =
            tokio::time::timeout(std::time::Duration::from_millis(150), &mut handler).await
        {
            panic!(
                "the handler answered {response:?} while the queue was full: the packet was dropped \
                 and the peer was told it was handled (AUDIT C105)"
            );
        }

        // Free one slot **while polling the handler** — a future that is not polled makes no progress,
        // which is what the first draft of this test got wrong. The packet it was holding must arrive,
        // and it must be the *peer's*, not the filler.
        let drain = async {
            let first = rx.recv().await.expect("the filler is in the queue");
            assert_eq!(first.peer.id, filler_peer.id, "the filler came first");
            tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
                .await
                .expect(
                    "the handler's packet must arrive — with a discarded `try_send` it never does",
                )
                .expect("a message")
        };
        let (response, delivered) = tokio::join!(&mut handler, drain);
        assert_eq!(
            delivered.peer.id, local.id,
            "the delivered packet is the peer's own, not the filler"
        );
        assert!(
            matches!(response, CommunicationResponse::HandledWithoutMessage),
            "a packet that was accepted into the pipeline is handled"
        );
    }

    /// After a bootstrap restart the connection table is empty, but the other
    /// validators keep their established connection and send ordinary packets.
    /// An accepted authenticated packet must restore the sender to the table.
    #[tokio::test]
    async fn an_authenticated_packet_relearns_a_peer_after_restart() {
        let local = peer("bootstrap", "10.0.0.1");
        let sender = peer("validator-1", "10.0.0.2");
        let conf = conf_for(&local);
        let conns = tokio::sync::RwLock::new(Vec::new());
        let (tx, mut rx) = tokio::sync::mpsc::channel::<RoutingMessage>(2);
        let packet = protocol_helper::packet(&sender, &conf.network_id, Default::default());
        let result = handle(packet, &conf, &NoTransport, &conns, &tx, &NopLog).await;
        assert!(matches!(
            result,
            CommunicationResponse::HandledWithoutMessage
        ));
        assert_eq!(rx.recv().await.expect("routed").peer.id, sender.id);
        assert_eq!(conns.read().await.len(), 1);
        assert_eq!(conns.read().await[0].id, sender.id);
    }

    /// Prevent an out-of-class authenticated packet from becoming a broadcast target.
    #[tokio::test]
    async fn an_inbound_remote_class_packet_does_not_become_a_peer() {
        let local = peer("bootstrap", "10.0.0.1");
        let remote = peer("remote", "8.8.8.8");
        let conf = conf_for(&local);
        let conns = tokio::sync::RwLock::new(Vec::new());
        let (tx, mut rx) = tokio::sync::mpsc::channel::<RoutingMessage>(2);
        let packet = protocol_helper::packet(&remote, &conf.network_id, Default::default());
        let _ = handle(packet, &conf, &NoTransport, &conns, &tx, &NopLog).await;
        assert_eq!(
            rx.recv().await.expect("previous routing behavior").peer.id,
            remote.id
        );
        assert!(
            conns.read().await.is_empty(),
            "out-of-class peer must not enter the table"
        );
    }

    /// The consumer has disappeared: a packet that was not routed cannot establish
    /// a connection. This preserves the original fail-closed backpressure contract.
    #[tokio::test]
    async fn a_closed_router_does_not_relearn_a_peer() {
        let local = peer("bootstrap", "10.0.0.1");
        let sender = peer("validator-1", "10.0.0.2");
        let conf = conf_for(&local);
        let conns = tokio::sync::RwLock::new(Vec::new());
        let (tx, rx) = tokio::sync::mpsc::channel::<RoutingMessage>(1);
        drop(rx);
        let packet = protocol_helper::packet(&sender, &conf.network_id, Default::default());
        let _ = handle(packet, &conf, &NoTransport, &conns, &tx, &NopLog).await;
        assert!(conns.read().await.is_empty());
    }

    #[test]
    fn classifies_private_addresses_as_local() {
        for host in [
            "0.0.0.0",
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
        ] {
            assert!(is_local_address(host), "{host} should be local");
        }
    }

    #[test]
    fn classifies_public_addresses_as_remote() {
        for host in ["8.8.8.8", "1.2.3.4", "172.32.0.1", "192.169.0.1"] {
            assert!(!is_local_address(host), "{host} should be public");
        }
    }

    /// **The same-network check is about *locality class*, not equality.** A peer is on the same
    /// network when both ends are local (a private/devnet address) or both are public — so a
    /// localhost node accepts a LAN peer, and refuses a public one. The `handle` path uses this to
    /// decide whether to answer a handshake, so an inversion here would make a devnet unreachable
    /// (or a public node accept only public peers).
    #[test]
    fn the_same_network_check_compares_locality_class() {
        let conf = |local_host: &str| RPConf {
            local: peer("local", local_host),
            network_id: "testnet".to_string(),
            bootstrap: None,
            default_timeout: std::time::Duration::from_secs(10),
            max_num_of_connections: 10,
            clear_connections: crate::rp::rp_conf::ClearConnectionsConf {
                num_of_connections_pinged: 10,
            },
        };

        let local_conf = conf("127.0.0.1");
        assert!(
            check_peer_on_same_network(&local_conf, &peer("lan", "192.168.1.5")),
            "a local node accepts another private peer"
        );
        assert!(
            !check_peer_on_same_network(&local_conf, &peer("public", "8.8.8.8")),
            "…and refuses a public one"
        );

        let public_conf = conf("8.8.8.8");
        assert!(
            check_peer_on_same_network(&public_conf, &peer("other", "1.1.1.1")),
            "a public node accepts a public peer"
        );
        assert!(
            !check_peer_on_same_network(&public_conf, &peer("private", "10.0.0.1")),
            "…and refuses a private one"
        );
    }

    /// The same-network check is *symmetric* in its two arguments' roles, since it compares the two
    /// classifications — the property a caller relies on when it applies the check to either side.
    #[test]
    fn the_same_network_check_is_symmetric() {
        let conf = |host: &str| RPConf {
            local: peer("local", host),
            network_id: "testnet".to_string(),
            bootstrap: None,
            default_timeout: std::time::Duration::from_secs(10),
            max_num_of_connections: 10,
            clear_connections: crate::rp::rp_conf::ClearConnectionsConf {
                num_of_connections_pinged: 10,
            },
        };
        for (a, b) in [
            ("127.0.0.1", "10.0.0.1"),
            ("127.0.0.1", "8.8.8.8"),
            ("8.8.8.8", "1.1.1.1"),
        ] {
            assert_eq!(
                check_peer_on_same_network(&conf(a), &peer("p", b)),
                check_peer_on_same_network(&conf(b), &peer("p", a)),
                "({a}, {b}) must classify the same way as ({b}, {a})"
            );
        }
    }

    fn peer(name: &str, host: &str) -> PeerNode {
        use rchain_shared::refined::Port;
        PeerNode::from(
            crate::peer_node::NodeIdentifier::new(name.as_bytes().to_vec()),
            host.to_string(),
            Port::new(40400),
            Port::new(40404),
        )
    }
}
