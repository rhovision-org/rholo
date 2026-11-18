//! Transport-layer convenience methods.
//!
//! Mirrors `comm/src/main/scala/coop/rchain/comm/transport/TransportLayerSyntax.scala`. The cats-mtl
//! `RPConfAsk` reader and the casper `ToPacket` typeclass are simplified to explicit `&RPConf` +
//! `Packet` parameters.

use rchain_models::comm::protocol::Packet;

use crate::errors::CommErr;
use crate::peer_node::PeerNode;
use crate::rp::protocol_helper;
use crate::rp::rp_conf::RPConf;
use crate::transport::chunker::Blob;
use crate::transport::transport_layer::TransportLayer;

/// Stream a blob to a single peer (port of `stream1`), returning that peer's outcome.
pub async fn stream1<T: TransportLayer + ?Sized>(
    transport: &T,
    peer: &PeerNode,
    blob: Blob,
) -> CommErr<()> {
    transport
        .stream(std::slice::from_ref(peer), blob)
        .await
        .into_iter()
        .next()
        .unwrap_or(Ok(()))
}

/// Send a packet to a peer, wrapped in a protocol message (port of `sendToPeer`).
///
/// **The error is returned rather than discarded** (C254's E6b). It used to be
/// `let _ = transport.send(peer, msg).await`, which made a packet that never left this node — a peer
/// that had gone, a TLS session that failed — indistinguishable from one that was delivered, at the
/// one call site whose entire job is delivery.
pub async fn send_to_peer<T: TransportLayer + ?Sized>(
    transport: &T,
    conf: &RPConf,
    peer: &PeerNode,
    packet: Packet,
) -> CommErr<()> {
    let msg = protocol_helper::packet(&conf.local, &conf.network_id, packet);
    transport.send(peer, msg).await
}

/// Stream a packet to a peer in chunks (port of `streamToPeer`), returning the peer's outcome.
pub async fn stream_to_peer<T: TransportLayer + ?Sized>(
    transport: &T,
    conf: &RPConf,
    peer: &PeerNode,
    packet: Packet,
) -> CommErr<()> {
    let blob = Blob {
        sender: conf.local.clone(),
        packet,
    };
    stream1(transport, peer, blob).await
}

/// Send a packet to the configured bootstrap peer (port of `sendToBootstrap`).
pub async fn send_to_bootstrap<T: TransportLayer + ?Sized>(
    transport: &T,
    conf: &RPConf,
    packet: Packet,
) -> CommErr<()> {
    match &conf.bootstrap {
        Some(bootstrap) => send_to_peer(transport, conf, bootstrap, packet).await,
        // Nothing to do is not a failure: there is no peer that could have refused the packet.
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    use async_trait::async_trait;
    use rchain_models::comm::protocol::Protocol;
    use rchain_shared::refined::Port;

    use crate::errors::{CommErr, CommError};
    use crate::peer_node::{NodeIdentifier, PeerNode};
    use crate::rp::rp_conf::ClearConnectionsConf;

    /// What the transport was asked to do, recorded rather than performed — the same shape as
    /// `casper`'s `MockTransport`, kept here because this file is the only user of these three
    /// helpers and casper depends on this crate, not the other way round.
    #[derive(Default)]
    struct RecordingTransport {
        sends: Mutex<Vec<(PeerNode, Protocol)>>,
        streams: Mutex<Vec<(Vec<PeerNode>, Blob)>>,
    }

    #[async_trait]
    impl TransportLayer for RecordingTransport {
        async fn send(&self, peer: &PeerNode, msg: Protocol) -> CommErr<()> {
            self.sends.lock().unwrap().push((peer.clone(), msg));
            Ok(())
        }

        async fn broadcast(&self, peers: &[PeerNode], msg: Protocol) -> Vec<CommErr<()>> {
            self.sends
                .lock()
                .unwrap()
                .extend(peers.iter().map(|p| (p.clone(), msg.clone())));
            peers.iter().map(|_| Ok(())).collect()
        }

        async fn stream(&self, peers: &[PeerNode], blob: Blob) -> Vec<CommErr<()>> {
            self.streams.lock().unwrap().push((peers.to_vec(), blob));
            peers.iter().map(|_| Ok(())).collect()
        }
    }

    /// A transport with a scripted outcome vector: the single-peer helper must select this peer's
    /// outcome, whatever the rest of the script says.
    struct ScriptedTransport {
        outcomes: Vec<CommErr<()>>,
    }

    #[async_trait]
    impl TransportLayer for ScriptedTransport {
        async fn send(&self, _peer: &PeerNode, _msg: Protocol) -> CommErr<()> {
            unreachable!("the scripted transport only answers stream")
        }

        async fn broadcast(&self, _peers: &[PeerNode], _msg: Protocol) -> Vec<CommErr<()>> {
            unreachable!("the scripted transport only answers stream")
        }

        async fn stream(&self, _peers: &[PeerNode], _blob: Blob) -> Vec<CommErr<()>> {
            self.outcomes.clone()
        }
    }

    fn peer(name: &str, port: u16) -> PeerNode {
        PeerNode::from(
            NodeIdentifier::new(name.as_bytes().to_vec()),
            "host".to_string(),
            Port::new(port),
            Port::new(port),
        )
    }

    fn conf(local: PeerNode, bootstrap: Option<PeerNode>) -> RPConf {
        RPConf {
            local,
            network_id: "testnet".to_string(),
            bootstrap,
            default_timeout: Duration::from_secs(10),
            max_num_of_connections: 8,
            clear_connections: ClearConnectionsConf {
                num_of_connections_pinged: 10,
            },
        }
    }

    fn packet() -> Packet {
        Packet {
            type_id: "BlockMessage".to_string(),
            content: vec![1, 2, 3],
        }
    }

    /// `send_to_peer` wraps the packet in a protocol message carrying **this node's** identity and
    /// network id — a message stamped with the wrong sender would be dropped (or worse, attributed)
    /// by the receiver — and sends it to exactly the peer asked for.
    #[tokio::test]
    async fn send_to_peer_wraps_the_packet_with_the_local_identity() {
        let transport = RecordingTransport::default();
        let local = peer("local", 40400);
        let remote = peer("remote", 40401);
        let conf = conf(local.clone(), None);

        send_to_peer(&transport, &conf, &remote, packet())
            .await
            .expect("the mock transport accepts a send");

        let sends = transport.sends.lock().unwrap();
        assert_eq!(sends.len(), 1, "exactly one send");
        assert_eq!(sends[0].0, remote, "to the peer asked for");
        let sent = &sends[0].1;
        let header = sent.header.as_ref().expect("a header");
        assert_eq!(
            header.sender,
            Some(local.to_node()),
            "stamped with the local peer"
        );
        assert_eq!(header.network_id, "testnet");
        match &sent.message {
            Some(rchain_models::comm::protocol::protocol::Message::Packet(p)) => {
                assert_eq!(p, &packet(), "the packet is carried unchanged");
            }
            other => panic!("expected a packet protocol, got {other:?}"),
        }
        assert!(transport.streams.lock().unwrap().is_empty(), "no stream");
    }

    /// `stream1` streams to a **one-element** slice of the peer given, not to a broadcast list: it
    /// is the single-peer form, and a blob sent to the wrong set would be a very quiet bug.
    #[tokio::test]
    async fn stream1_streams_to_exactly_one_peer() {
        let transport = RecordingTransport::default();
        let target = peer("target", 40402);
        let blob = Blob {
            sender: peer("local", 40400),
            packet: packet(),
        };

        assert!(stream1(&transport, &target, blob.clone()).await.is_ok());

        let streams = transport.streams.lock().unwrap();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].0, vec![target], "a one-element peer list");
        // `Blob` carries no `Debug`, so the parts are compared rather than the whole.
        assert_eq!(streams[0].1.sender, blob.sender);
        assert_eq!(
            streams[0].1.packet, blob.packet,
            "the blob is carried unchanged"
        );
    }

    /// `stream_to_peer` builds the blob from the configuration: the sender is the local peer, so a
    /// receiver can attribute the stream, and the packet rides inside it.
    #[tokio::test]
    async fn stream_to_peer_stamps_the_blob_with_the_local_sender() {
        let transport = RecordingTransport::default();
        let local = peer("local", 40400);
        let remote = peer("remote", 40401);
        let conf = conf(local.clone(), None);

        assert!(stream_to_peer(&transport, &conf, &remote, packet())
            .await
            .is_ok());

        let streams = transport.streams.lock().unwrap();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].0, vec![remote]);
        assert_eq!(
            streams[0].1.sender, local,
            "the blob's sender is `conf.local`"
        );
        assert_eq!(streams[0].1.packet, packet());
        assert!(
            transport.sends.lock().unwrap().is_empty(),
            "streaming, not sending"
        );
    }

    /// `send_to_bootstrap` sends **nothing** when no bootstrap is configured — the `if let` is the
    /// whole function, and a node with no bootstrap (a validator starting from genesis) must not
    /// panic or invent a peer.
    #[tokio::test]
    async fn send_to_bootstrap_does_nothing_without_a_bootstrap_peer() {
        let transport = RecordingTransport::default();
        let conf = conf(peer("local", 40400), None);
        send_to_bootstrap(&transport, &conf, packet())
            .await
            .expect("nothing to send is not a failure");
        assert!(transport.sends.lock().unwrap().is_empty());
        assert!(transport.streams.lock().unwrap().is_empty());
    }

    /// …and sends to the bootstrap peer when there is one, wrapped like any other send.
    #[tokio::test]
    async fn send_to_bootstrap_sends_to_the_configured_peer() {
        let transport = RecordingTransport::default();
        let local = peer("local", 40400);
        let bootstrap = peer("bootstrap", 40403);
        let conf = conf(local.clone(), Some(bootstrap.clone()));

        send_to_bootstrap(&transport, &conf, packet())
            .await
            .expect("nothing to send is not a failure");

        let sends = transport.sends.lock().unwrap();
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0].0, bootstrap, "to the bootstrap peer");
        let sent = &sends[0].1;
        assert_eq!(
            sent.header.as_ref().expect("a header").sender,
            Some(local.to_node())
        );
        match &sent.message {
            Some(rchain_models::comm::protocol::protocol::Message::Packet(p)) => {
                assert_eq!(p, &packet());
            }
            other => panic!("expected a packet protocol, got {other:?}"),
        }
    }

    /// For every scripted outcome vector, the single-peer helper returns the **first** peer's
    /// outcome (and an empty vector is `Ok`, because no peer refused). Extras and order are not
    /// silently reinterpreted.
    #[tokio::test]
    async fn stream1_selects_the_first_peers_outcome_for_every_script() {
        let remote = peer("remote", 40401);
        let failure = Err(CommError::TimeOut);
        let scripts = vec![
            vec![],
            vec![Ok(())],
            vec![failure.clone()],
            vec![Ok(()), failure.clone()],
            vec![failure.clone(), Ok(())],
            vec![failure.clone(), failure.clone(), Ok(())],
        ];
        for outcomes in scripts {
            let expected = outcomes.first().cloned().unwrap_or(Ok(()));
            let transport = ScriptedTransport {
                outcomes: outcomes.clone(),
            };
            let blob = Blob {
                sender: peer("local", 40400),
                packet: packet(),
            };
            assert_eq!(
                stream1(&transport, &remote, blob).await,
                expected,
                "script {outcomes:?}"
            );
        }
    }
}
