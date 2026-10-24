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

/// The memory gate's answer on the gRPC surface: `resource_exhausted` with the gate's reason.
///
/// Named rather than inline so the surface's refusal is testable without a listener: the verdict
/// itself is `resources::admission::evaluate`'s, and this is the one line that maps it to a gRPC
/// status. Tonic's interceptor runs after the request frame is decoded; the pre-decode bound here is
/// `max_decoding_message_size`, and this is the memory bound that follows it and precedes the handler.
fn refuse_under_pressure(pressure: &crate::resources::PressureGate) -> Result<(), ::tonic::Status> {
    match pressure.evaluate_now().refusal() {
        Some((reason, detail)) => Err(::tonic::Status::resource_exhausted(format!(
            "{reason}: {detail}"
        ))),
        None => Ok(()),
    }
}

/// Serve the external gRPC service (deploy) on `addr` (port of the external `GrpcServer`).
pub async fn serve_deploy(
    deploy: DeployGrpcServiceV1,
    addr: std::net::SocketAddr,
    max_message_size: usize,
    pressure: crate::resources::PressureGate,
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
    let service = InterceptedService::new(server, move |req: ::tonic::Request<()>| {
        // **The memory gate** (B34). It runs before the handler, so a pressured node refuses the
        // deploy before it is parsed or pooled. A stale or unreadable sample refuses, like HTTP.
        refuse_under_pressure(&pressure)?;
        let key = ingress_key(req.extensions().get::<PeerAddress>().and_then(|p| p.0));
        if limiter.allow(&key) {
            Ok(req)
        } else {
            Err(::tonic::Status::resource_exhausted(
                "deploy rate limit exceeded",
            ))
        }
    });

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

    /// **A node below its admission floor refuses gRPC deploys with `resource_exhausted`** (B34).
    ///
    /// The HTTP half is pinned in `web/http.rs`; this is the same verdict mapped to the gRPC status a
    /// client library will surface, and the control shows the same gate admitting on a healthy sample
    /// — a gate that refused everything would look like this test passing while the node was broken.
    #[test]
    fn a_pressured_node_refuses_grpc_deploys_with_resource_exhausted() {
        use crate::resources::admission::AdmissionPolicy;
        use crate::resources::{PressureGate, ResourceGuard, ResourceSnapshot, ResourceState};

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0);
        let snapshot = |available_mb: u64| ResourceSnapshot {
            state: "normal",
            fallback: ResourceState::Normal,
            mem_available_mb: available_mb,
            mem_total_mb: 8_192,
            sampled_at_ms: now_ms,
            source: "test".to_string(),
            ..ResourceSnapshot::default()
        };
        let gate = |available_mb: u64| {
            PressureGate::new(
                ResourceGuard::inert(snapshot(available_mb)),
                AdmissionPolicy::default(),
            )
        };

        let status = refuse_under_pressure(&gate(10))
            .expect_err("10 MiB is below the 819 MiB floor on an 8 GiB host");
        assert_eq!(status.code(), ::tonic::Code::ResourceExhausted);
        assert!(
            status.message().contains("admission floor"),
            "the status must name the gate: {}",
            status.message()
        );
        assert!(refuse_under_pressure(&gate(4_096)).is_ok());
    }
}
