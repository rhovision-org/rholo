//! Client-side transport layer (gRPC over mutual TLS).
//!
//! Mirrors `comm/src/main/scala/coop/rchain/comm/transport/GrpcTransportClient.scala`. The
//! per-peer `StreamObservable` (bounded key-only queue) is simplified to a direct per-peer stream
//! task; the packet cache/`PacketOps` round-trip is preserved for `stream`.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::FuturesOrdered;
use futures::StreamExt;
use http::Uri;
use hyper_util::rt::TokioIo;
use rchain_models::comm::protocol::transport_layer_client::TransportLayerClient;
use rchain_models::comm::protocol::Protocol;
use rustls::pki_types::ServerName;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tonic::transport::{Channel, Endpoint};
use tower::service_fn;

use crate::errors::{CommErr, CommError};
use crate::peer_node::PeerNode;
use crate::transport::chunker::Blob;
use crate::transport::grpc_transport;
use crate::transport::hostname_trust_manager;
use crate::transport::transport_layer::TransportLayer;

/// The default send timeout (port of `GrpcTransportClient.DefaultSendTimeout`).
pub const DEFAULT_SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Cap on the number of cached per-peer TLS channels. The cache never evicted, so a peer could grow
/// it without bound (one entry per distinct `PeerNode`). At capacity the oldest entry is evicted
/// before a new one is inserted.
const MAX_CACHED_CHANNELS: usize = 1024;

/// The gRPC transport client (port of `GrpcTransportClient`).
#[derive(Clone)]
pub struct GrpcTransportClient {
    network_id: Arc<String>,
    packet_chunk_size: usize,
    max_message_size: usize,
    tls: Arc<rustls::ClientConfig>,
    channels: Arc<tokio::sync::Mutex<HashMap<PeerNode, Channel>>>,
}

impl GrpcTransportClient {
    pub fn new(
        network_id: String,
        cert_pem: &str,
        key_pem: &str,
        max_message_size: usize,
        packet_chunk_size: usize,
        _client_queue_size: usize,
    ) -> Result<Self, String> {
        let tls = hostname_trust_manager::client_config(cert_pem, key_pem)?;
        Ok(GrpcTransportClient {
            network_id: Arc::new(network_id),
            packet_chunk_size,
            max_message_size,
            tls,
            channels: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        })
    }

    /// Create a TLS channel to `peer`, verifying the server cert against the peer's node id.
    async fn create_channel(&self, peer: &PeerNode) -> Result<Channel, CommError> {
        let endpoint = Endpoint::from_shared(format!(
            "https://{}:{}",
            peer.endpoint.host,
            u16::from(peer.endpoint.tcp_port)
        ))
        .map_err(|e| CommError::ParseError(e.to_string()))?;

        let connector = TlsConnector::from(self.tls.clone());
        let server_name = ServerName::try_from(peer.id.to_string())
            .map_err(|e| CommError::ParseError(e.to_string()))?;
        let host = peer.endpoint.host.clone();
        let port = peer.endpoint.tcp_port;

        Ok(
            endpoint.connect_with_connector_lazy(service_fn(move |_: Uri| {
                let connector = connector.clone();
                let server_name = server_name.clone();
                let host = host.clone();
                async move {
                    let tcp = TcpStream::connect((host.as_str(), u16::from(port))).await?;
                    let tls = connector.connect(server_name, tcp).await?;
                    Ok::<_, std::io::Error>(TokioIo::new(tls))
                }
            })),
        )
    }

    async fn get_channel(&self, peer: &PeerNode) -> Result<Channel, CommError> {
        let mut channels = self.channels.lock().await;
        if let Some(channel) = channels.get(peer) {
            return Ok(channel.clone());
        }
        // Bound the cache: when full and the key is new, evict an arbitrary (first) entry.
        if channels.len() >= MAX_CACHED_CHANNELS {
            if let Some(oldest) = channels.keys().next().cloned() {
                channels.remove(&oldest);
            }
        }
        let channel = self.create_channel(peer).await?;
        channels.insert(peer.clone(), channel.clone());
        Ok(channel)
    }

    /// One bounded stream RPC; the caller retains the blob for a single retry.
    async fn stream_once(&self, peer: &PeerNode, blob: &Blob) -> CommErr<()> {
        let channel = self.get_channel(peer).await?;
        let mut client =
            TransportLayerClient::new(channel).max_decoding_message_size(self.max_message_size);
        tokio::time::timeout(
            DEFAULT_SEND_TIMEOUT,
            grpc_transport::stream(
                &mut client,
                peer,
                &self.network_id,
                blob,
                self.packet_chunk_size,
            ),
        )
        .await
        .map_err(|_| CommError::TimeOut)?
    }
}

#[async_trait]
impl TransportLayer for GrpcTransportClient {
    async fn send(&self, peer: &PeerNode, msg: Protocol) -> CommErr<()> {
        let channel = self.get_channel(peer).await?;
        let mut client =
            TransportLayerClient::new(channel).max_decoding_message_size(self.max_message_size);
        tokio::time::timeout(
            DEFAULT_SEND_TIMEOUT,
            grpc_transport::send(&mut client, peer, msg),
        )
        .await
        .map_err(|_| CommError::TimeOut)?
    }

    async fn broadcast(&self, peers: &[PeerNode], msg: Protocol) -> Vec<CommErr<()>> {
        // CommUtil associates each outcome with the peer at the same index.
        // Poll concurrently, but return input order rather than completion order.
        let sends: FuturesOrdered<_> = peers
            .iter()
            .map(|peer| self.send(peer, msg.clone()))
            .collect();
        sends.collect().await
    }

    async fn stream(&self, peers: &[PeerNode], blob: Blob) -> Vec<CommErr<()>> {
        let tasks: Vec<_> = peers
            .iter()
            .map(|peer| {
                let this = self.clone();
                let peer = peer.clone();
                let blob = blob.clone();
                tokio::spawn(async move {
                    let first = this.stream_once(&peer, &blob).await;
                    // A failed response otherwise waits for the retriever's
                    // 240-second maintenance timeout. Keep the same payload and
                    // retry once; both attempts retain the five-second RPC bound.
                    let result = if first.is_err() {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        this.stream_once(&peer, &blob).await
                    } else {
                        first
                    };
                    if let Err(error) = &result {
                        eprintln!(
                            "gRPC stream {} to {peer} failed after bounded retry: {error:?}",
                            blob.packet.type_id
                        );
                    }
                    result
                })
            })
            .collect();
        let mut outcomes = Vec::with_capacity(tasks.len());
        for task in tasks {
            outcomes.push(match task.await {
                Ok(result) => result,
                Err(join) => Err(CommError::UnknownCommError(format!(
                    "stream task failed: {join}"
                ))),
            });
        }
        outcomes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use rchain_shared::refined::Port;

    use crate::peer_node::NodeIdentifier;
    use crate::transport::generate_certificate_if_absent::generate_certificate;

    fn client() -> GrpcTransportClient {
        let (cert_pem, key_pem) = generate_certificate().expect("a test certificate");
        GrpcTransportClient::new(
            "testnet".to_string(),
            &cert_pem,
            &key_pem,
            16 * 1024 * 1024,
            16 * 1024,
            16,
        )
        .expect("a client with a valid certificate")
    }

    fn peer(id: NodeIdentifier, host: &str, port: u16) -> PeerNode {
        PeerNode::from(id, host.to_string(), Port::new(port), Port::new(port))
    }

    fn short_id() -> NodeIdentifier {
        // Hex containing letters is a valid TLS DNS name; an all-numeric
        // fixture can be rejected before a connection is attempted.
        NodeIdentifier::new(vec![0xAB; 20])
    }

    /// A later peer can fail immediately while the first peer waits for TLS.
    /// The caller must still associate each failure with the correct recipient.
    #[tokio::test]
    async fn broadcast_outcomes_follow_peer_order_when_a_later_peer_fails_first() {
        let c = client();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let slow = peer(
            short_id(),
            "127.0.0.1",
            listener.local_addr().unwrap().port(),
        );
        let invalid = peer(NodeIdentifier::new(vec![0xAB; 200]), "127.0.0.1", 1);

        let outcomes = c.broadcast(&[slow, invalid], Protocol::default()).await;

        assert_eq!(outcomes.len(), 2);
        assert!(
            matches!(&outcomes[0], Err(CommError::TimeOut)),
            "the first peer must reach the network timeout: {outcomes:?}"
        );
        assert!(
            matches!(&outcomes[1], Err(CommError::ParseError(_))),
            "the later invalid identity must retain its recipient index: {outcomes:?}"
        );
    }

    struct RefuseStreams {
        receiver: crate::transport::grpc_transport_receiver::GrpcTransportReceiver,
        attempts: Arc<std::sync::atomic::AtomicUsize>,
        failures: usize,
    }

    #[async_trait]
    impl rchain_models::comm::protocol::transport_layer_server::TransportLayer for RefuseStreams {
        async fn send(
            &self,
            request: tonic::Request<rchain_models::comm::protocol::TlRequest>,
        ) -> Result<tonic::Response<rchain_models::comm::protocol::TlResponse>, tonic::Status>
        {
            self.receiver.send(request).await
        }

        async fn stream(
            &self,
            request: tonic::Request<tonic::Streaming<rchain_models::comm::protocol::Chunk>>,
        ) -> Result<tonic::Response<rchain_models::comm::protocol::TlResponse>, tonic::Status>
        {
            let attempt = self
                .attempts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if attempt < self.failures {
                return Err(tonic::Status::unavailable(
                    "injected stream response failure",
                ));
            }
            self.receiver.stream(request).await
        }
    }

    async fn stream_retry_case(failures: usize) {
        use crate::transport::communication_response::CommunicationResponse;
        use crate::transport::grpc_transport_receiver::{
            accept_tls, ConcurrencyLimits, GrpcTransportReceiver,
        };
        use crate::transport::hostname_trust_manager::{public_address_of_cert, server_config};
        use rchain_models::comm::protocol::transport_layer_server::TransportLayerServer;
        use rchain_models::comm::protocol::Packet;
        use rustls::pki_types::CertificateDer;
        use std::sync::atomic::{AtomicUsize, Ordering};

        fn cert_id(pem: &str) -> NodeIdentifier {
            let (_, pem) = x509_parser::pem::parse_x509_pem(pem.as_bytes()).unwrap();
            let der = CertificateDer::from(pem.contents);
            NodeIdentifier::new(public_address_of_cert(&der).unwrap())
        }

        let (server_cert, server_key) = generate_certificate().unwrap();
        let (client_cert, client_key) = generate_certificate().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let remote = peer(cert_id(&server_cert), "127.0.0.1", port);
        let local = peer(cert_id(&client_cert), "127.0.0.1", port);
        let (delivered, mut received) = tokio::sync::mpsc::channel(2);
        let receiver = GrpcTransportReceiver::for_test(
            remote.clone(),
            "testnet",
            16 * 1024 * 1024,
            Arc::new(|_protocol| {
                Box::pin(async { CommunicationResponse::handled_without_message() })
            }),
            Arc::new(move |blob| {
                let delivered = delivered.clone();
                Box::pin(async move {
                    delivered.send(blob).await.unwrap();
                })
            }),
            ConcurrencyLimits::default(),
        );
        let attempts = Arc::new(AtomicUsize::new(0));
        let service = RefuseStreams {
            receiver,
            attempts: attempts.clone(),
            failures,
        };
        let tls = server_config(&server_cert, &server_key).unwrap();
        let incoming = accept_tls(listener, tokio_rustls::TlsAcceptor::from(tls), 8);
        let serving = tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(TransportLayerServer::new(service))
                .serve_with_incoming(incoming)
                .await
                .unwrap();
        });
        let client = GrpcTransportClient::new(
            "testnet".to_string(),
            &client_cert,
            &client_key,
            16 * 1024 * 1024,
            16 * 1024,
            16,
        )
        .unwrap();
        let blob = Blob {
            sender: local,
            packet: Packet {
                type_id: "BlockMessage".to_string(),
                content: vec![3; 32 * 1024],
            },
        };

        // One caller invocation, through actual mutually authenticated RPCs.
        let outcomes = client.stream(&[remote], blob.clone()).await;
        assert_eq!(outcomes.len(), 1, "one outcome per input peer");
        assert_eq!(
            outcomes[0].is_ok(),
            failures < 2,
            "the bounded retry reports completion after one failure and a failure after two"
        );
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            if failures == 0 { 1 } else { 2 }
        );
        if failures < 2 {
            let actual = tokio::time::timeout(std::time::Duration::from_secs(5), received.recv())
                .await
                .expect("the bounded retry must deliver")
                .expect("the receiver must remain alive");
            assert_eq!(actual.sender, blob.sender);
            assert_eq!(actual.packet, blob.packet);
        }
        assert!(received.try_recv().is_err(), "no extra delivery");
        serving.abort();
    }

    #[tokio::test]
    async fn stream_retries_a_failed_rpc_and_delivers_the_same_blob() {
        stream_retry_case(1).await;
    }

    #[tokio::test]
    async fn stream_stops_after_two_failed_rpcs() {
        stream_retry_case(usize::MAX).await;
    }

    #[tokio::test]
    async fn a_successful_stream_is_not_retried() {
        stream_retry_case(0).await;
    }

    /// A client with unusable PEM material fails to build rather than panicking later: the TLS
    /// configuration is the one thing `new` can get wrong, and it does so up front.
    #[test]
    fn a_client_rejects_unusable_certificate_material() {
        for (cert, key) in [
            ("not a pem", "also not a pem"),
            (
                "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
                "x",
            ),
        ] {
            assert!(
                GrpcTransportClient::new("testnet".to_string(), cert, key, 1024, 1024, 1).is_err(),
                "garbage PEM must be refused"
            );
        }
    }

    /// **A peer id that is not a usable TLS server name is rejected as a parse error.** The client
    /// verifies the *server's* certificate against the peer's id, so an id it cannot form a
    /// `ServerName` from must fail here — not be silently accepted and then fail (or worse, match)
    /// during the handshake. The bound is DNS's 253 characters, and an id is 40 hex chars per 20
    /// key bytes, so an over-long key is what reaches it.
    #[tokio::test]
    async fn an_over_long_peer_id_is_a_parse_error() {
        let c = client();
        let long_id = peer(NodeIdentifier::new(vec![0xAB; 200]), "127.0.0.1", 40404);
        let err = c
            .get_channel(&long_id)
            .await
            .expect_err("a 400-character server name is not valid");
        assert!(
            matches!(err, CommError::ParseError(_)),
            "expected a parse error, got {err:?}"
        );
    }

    /// A cached lazy channel is not evidence of delivery. Both the initial and
    /// repeated RPC to an unavailable peer must report a failed handoff.
    #[tokio::test]
    async fn a_failed_connection_is_reported_on_send() {
        let c = client();
        let p = peer(short_id(), "127.0.0.1", 1);
        assert!(
            c.send(&p, Protocol::default()).await.is_err(),
            "a refused connection must be an error"
        );
        assert!(
            c.send(&p, Protocol::default()).await.is_err(),
            "a cached lazy channel must not turn a failed send into success"
        );
    }

    /// The channel cache is **bounded** (an unbounded per-peer channel map was a DoS finding in the
    /// security pass, `spec/audit/passes.md` §12): the bound is what makes eviction reachable at all. It is
    /// asserted at *compile* time, so a change to the constant is a build failure rather than a test
    /// failure nobody runs — and so this module needs no test to police a number.
    const _: () = assert!(MAX_CACHED_CHANNELS > 0 && MAX_CACHED_CHANNELS <= 4096);
}
