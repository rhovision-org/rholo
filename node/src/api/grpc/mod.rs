//! gRPC service adapters (port of `api/DeployGrpcServiceV1.scala`,
//! `api/ProposeGrpcServiceV1.scala`, `api/ReplGrpcService.scala`, and `runtime/GrpcServices.scala`).
//!
//! The monix-gRPC transport is mapped to plain async service structs over `BlockApi`/`RhoRuntime`;
//! the protobuf `oneof` responses collapse to `Result<_, ServiceError>` (the Scala `Either`). The
//! tonic (gRPC) bindings for these adapters live in [`tonic`].

mod deploy_grpc_service_v1;
mod propose_grpc_service_v1;
mod repl_grpc_service;
pub mod tonic;

pub use deploy_grpc_service_v1::DeployGrpcServiceV1;
pub use propose_grpc_service_v1::ProposeGrpcServiceV1;
pub use repl_grpc_service::{CmdRequest, EvalRequest, ReplGrpcService, ReplResponse};

use std::sync::Arc;

use rchain_casper::api::block_api::BlockApi;
use rchain_casper::api::block_report_api::BlockReportApi;
use rchain_rholang::runtime::RhoRuntime;
use rchain_shared::rate_limiter::KeyedRateLimiter;

/// The trio of node gRPC services (port of `GrpcServices`).
pub struct GrpcServices {
    pub deploy: DeployGrpcServiceV1,
    pub propose: ProposeGrpcServiceV1,
    pub repl: ReplGrpcService,
}

impl GrpcServices {
    /// Build the service trio from the block APIs + runtime (port of `GrpcServices.build`).
    pub fn build(
        block_api: Arc<dyn BlockApi>,
        block_report_api: Arc<BlockReportApi>,
        runtime: Arc<RhoRuntime>,
        enable_reporting: bool,
    ) -> GrpcServices {
        let repl = ReplGrpcService::new(runtime);
        let deploy =
            DeployGrpcServiceV1::new(block_api.clone(), block_report_api, enable_reporting);
        let propose = ProposeGrpcServiceV1::new(block_api);
        GrpcServices {
            deploy,
            propose,
            repl,
        }
    }
}

/// The deploy request rate limit (requests/second) applied to the external (unauthenticated) gRPC
/// server. Documented Scala deviation: Scala binds the deploy service to `0.0.0.0` with no limit.
pub const DEFAULT_API_RATE_LIMIT_PER_SEC: u64 = 100;

/// Distinct peers the deploy service's ingress bound tracks at once — the same rule as the HTTP
/// listener's `MAX_TRACKED_PEERS`, and for the same reason: the per-peer map is itself a resource an
/// unauthenticated caller sizes by opening connections.
pub const MAX_TRACKED_PEERS: usize = 4_096;

/// The subject of an ingress bound on the deploy gRPC service.
///
/// **The peer, and it is all the interceptor can have.** tonic runs an interceptor *after* the request
/// has been decoded, and what it hands over is an empty body: the deployer is inside that body, so it
/// does not exist yet at the point this bound runs. That is not a defect in this bound but the reason
/// it is a *different* bound from the deployer's: this one bounds what a connection may make the node
/// decode, and the deployer's own rate and pool share are charged downstream at `add_deploy`, where
/// the identity the node can hold a submitter to finally exists.
#[derive(Clone, Copy, Debug)]
pub struct PeerAddress(pub Option<std::net::SocketAddr>);

/// The key the deploy service's ingress bound is charged to: the peer, or **one shared bucket** when
/// the address could not be named at accept time.
///
/// A shared bucket rather than a bypass, and deliberately: a key that meant "do not charge this" would
/// be indistinguishable from the defect this bound exists to close.
fn ingress_key(peer: Option<std::net::SocketAddr>) -> Vec<u8> {
    match peer {
        Some(addr) => addr.ip().to_string().into_bytes(),
        None => b"unattributed-peer".to_vec(),
    }
}

/// **The deploy service's ingress bound**, extracted from `serve_deploy` so the refusal is testable
/// without a listener. It is the peer-keyed limiter and nothing else: the deployer's own rate and
/// pool share are charged downstream, where the identity exists.
#[derive(Clone)]
struct DeployIngressGuard {
    limiter: Arc<KeyedRateLimiter>,
}

impl ::tonic::service::interceptor::Interceptor for DeployIngressGuard {
    fn call(
        &mut self,
        request: ::tonic::Request<()>,
    ) -> Result<::tonic::Request<()>, ::tonic::Status> {
        let key = ingress_key(request.extensions().get::<PeerAddress>().and_then(|p| p.0));
        if self.limiter.allow(&key) {
            Ok(request)
        } else {
            Err(::tonic::Status::resource_exhausted(
                rchain_casper::pool::RATE_LIMITED_MESSAGE,
            ))
        }
    }
}

/// A plaintext inbound connection carrying its peer address, for tonic's `Connected`.
struct PeerAddressIo {
    stream: tokio::net::TcpStream,
    peer: Option<std::net::SocketAddr>,
}

impl ::tonic::transport::server::Connected for PeerAddressIo {
    type ConnectInfo = PeerAddress;
    /// Total: the address is captured at accept time rather than looked up here, because
    /// `connect_info` is infallible and a lookup that could fail would have to be answered with
    /// something invented.
    fn connect_info(&self) -> Self::ConnectInfo {
        PeerAddress(self.peer)
    }
}

// The `AsyncRead`/`AsyncWrite` half of the wrapper, forwarding to the socket — the same shape
// `comm`'s `TlsIo` uses, and the reason `Connected` is not enough on its own: tonic needs an IO it
// can read and write, and this is one with an address attached.
impl tokio::io::AsyncRead for PeerAddressIo {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for PeerAddressIo {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.stream).poll_write(cx, buf)
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

/// Serve the external gRPC service (deploy) on `addr` (port of the external `GrpcServer`).
pub async fn serve_deploy(
    deploy: DeployGrpcServiceV1,
    addr: std::net::SocketAddr,
    max_message_size: usize,
    stop: tokio::sync::watch::Receiver<bool>,
) -> Result<(), String> {
    use ::tonic::service::interceptor::InterceptedService;
    use futures_util::StreamExt;
    use rchain_models::proto::casper::deploy_service_server::DeployServiceServer;

    // **The bound's subject is the peer** (W-N2-01, W-N2-08). It was one process-wide bucket, so one
    // caller's burst exhausted the allowance and refused every other caller's deploys — the bound
    // held, and the wrong counterparty paid for it.
    let limiter = Arc::new(KeyedRateLimiter::new(
        DEFAULT_API_RATE_LIMIT_PER_SEC,
        MAX_TRACKED_PEERS,
    ));
    let server = DeployServiceServer::new(deploy).max_decoding_message_size(max_message_size);
    let service = InterceptedService::new(server, DeployIngressGuard { limiter });

    // **Served from an accepted stream, so the peer address reaches the interceptor.** `serve` binds
    // its own listener and hands the service nothing but the request; `Connected::connect_info` is the
    // only way an address gets into a request's extensions, and it needs the socket.
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("cannot bind {addr}: {e}"))?;
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener).map(|accepted| {
        accepted.map(|stream| PeerAddressIo {
            peer: stream.peer_addr().ok(),
            stream,
        })
    });

    // Graceful shutdown (AUDIT C144): on the operator's word this stops accepting and lets the RPCs
    // already in flight finish, instead of the process being killed with them outstanding.
    ::tonic::transport::Server::builder()
        .add_service(service)
        .serve_with_incoming_shutdown(incoming, crate::runtime::shutdown::stop_requested(stop))
        .await
        .map_err(|e| e.to_string())
}

/// Serve the internal gRPC services (propose + repl) on `addr` (port of the internal `GrpcServer`).
pub async fn serve_internal(
    propose: ProposeGrpcServiceV1,
    repl: ReplGrpcService,
    addr: std::net::SocketAddr,
    max_message_size: usize,
    stop: tokio::sync::watch::Receiver<bool>,
) -> Result<(), String> {
    use rchain_models::proto::casper::propose_service_server::ProposeServiceServer;
    use rchain_models::proto::repl::repl_server::ReplServer;

    ::tonic::transport::Server::builder()
        .add_service(ProposeServiceServer::new(propose).max_decoding_message_size(max_message_size))
        .add_service(ReplServer::new(repl).max_decoding_message_size(max_message_size))
        .serve_with_shutdown(addr, crate::runtime::shutdown::stop_requested(stop))
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The gRPC ingress bound is charged to the peer, not to the node** (W-N2-01, W-N2-08).
    ///
    /// It was one process-wide bucket, so one caller's burst exhausted the allowance and every other
    /// caller's deploys were refused — the bound held, and the wrong counterparty paid for it. This is
    /// the assertion that can tell a keyed bound from an unkeyed one; the single-peer case cannot.
    ///
    /// Falsified by keying on anything constant, under which `bob`'s request is refused too.
    #[test]
    fn the_deploy_ingress_is_charged_to_the_peer() {
        let alice: std::net::SocketAddr = "192.0.2.1:5000".parse().expect("addr");
        let bob: std::net::SocketAddr = "192.0.2.2:5000".parse().expect("addr");
        let limiter = KeyedRateLimiter::new(1, MAX_TRACKED_PEERS);

        assert!(limiter.allow(&ingress_key(Some(alice))));
        assert!(
            !limiter.allow(&ingress_key(Some(alice))),
            "alice is over her allowance"
        );
        assert!(
            limiter.allow(&ingress_key(Some(bob))),
            "bob's allowance is his own; one caller's burst must not refuse every caller"
        );

        // A socket whose peer could not be named shares one bucket rather than being let through —
        // a key meaning "do not charge this" would be indistinguishable from the defect.
        let unattributed = ingress_key(None);
        assert!(limiter.allow(&unattributed));
        assert!(!limiter.allow(&unattributed));
    }

    /// The guard itself answers over-allowance with the gRPC spelling of 429: `RESOURCE_EXHAUSTED`
    /// carrying the pool's own phrase. Falsified by returning `Unauthenticated`, by an empty
    /// message, or by letting the second call through.
    #[test]
    fn the_ingress_guard_refuses_over_allowance_as_resource_exhausted() {
        use ::tonic::service::interceptor::Interceptor as _;
        let alice: std::net::SocketAddr = "192.0.2.9:5000".parse().expect("addr");
        let mut guard = DeployIngressGuard {
            limiter: Arc::new(KeyedRateLimiter::new(1, MAX_TRACKED_PEERS)),
        };
        let request = || {
            let mut request = ::tonic::Request::new(());
            request.extensions_mut().insert(PeerAddress(Some(alice)));
            request
        };
        assert!(
            guard.call(request()).is_ok(),
            "the first call is within the allowance"
        );
        let error = guard
            .call(request())
            .expect_err("the second call is over it");
        assert_eq!(error.code(), ::tonic::Code::ResourceExhausted);
        assert_eq!(error.message(), rchain_casper::pool::RATE_LIMITED_MESSAGE);
    }
}
