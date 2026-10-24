//! HTTP server routes (port of the http4s `service` methods in the `web` package and
//! `NewPrometheusReporter.service`).
//!
//! The public server mounts `/version`, `/metrics`, `/status`, the `/api` + `/api/v1` JSON routes,
//! the `/api/v1/openapi.json` OpenAPI document, and the reporting routes (`/reporting/trace` +
//! `/api/trace`). The admin server mounts the `/api`/`/api/v1` admin routes (propose) and its own
//! OpenAPI document. CORS and a per-request timeout are applied to both servers.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{middleware, Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{watch, OwnedSemaphorePermit, Semaphore};
use tower_http::cors::CorsLayer;
use tower_http::timeout::TimeoutLayer;

use rchain_casper::api::block_api::BlockApi;
use rchain_casper::api::block_report_api::BlockReportApi;
use rchain_casper::gateway::GatewayTxn;
use rchain_casper::protocol::comm_util::ConnectionsCell;
use rchain_comm::discovery::NodeDiscovery;
use rchain_comm::rp::rp_conf::RPConf;
use rchain_models::block_hash::BlockHash;
use rchain_models::validator::Validator;
use rchain_shared::base16;
use rchain_shared::rate_limiter::KeyedRateLimiter;
use rchain_shared::refined::{Port, ShardId};

use crate::api::admin_web_api::AdminWebApi;
use crate::api::dto::{
    BlockApiException, DataAtNameByBlockHashRequest, DataAtNameRequest, DeployRequest,
    ExploreDeployRequest, FaucetRequest, TxnRecordDto, TxnRequest,
};
use crate::api::grpc::{DEFAULT_API_RATE_LIMIT_PER_SEC, MAX_TRACKED_PEERS};
use crate::api::web_api::WebApi;
use crate::diagnostics::effects::MetricsRegistry;
use crate::diagnostics::NewPrometheusReporter;
use crate::resources::admission::PressureGate;
use crate::resources::ResourceGuard;
use crate::runtime::shutdown::stop_requested;
use crate::web::pos_read::PosReadApi;
use crate::web::reporting::transform_result;
use crate::web::status_info;
use crate::web::version_info;

/// Rate limit for the devnet faucet endpoint. Much stricter than the deploy route because each
/// request transfers real (dev) REV.
const FAUCET_RATE_LIMIT_PER_SEC: u64 = 1;

/// The most legs one cross-shard transaction may name (AUDIT C121).
///
/// **Why a bound here at all.** A leg is a shard membership, so a request naming more legs than the
/// gateway has memberships is asking for something the coordinator cannot do — `GatewayTxn::run`
/// rejects each non-member leg individually, but only *after* the request has been parsed into
/// node-signed deploys. `legs.len()` was unbounded, so one request bought as many deploys as the
/// caller cared to list.
///
/// **Why a constant and not a config key.** The register has recorded three times that a knob which is
/// parsed and then not honoured is worse than no knob (the metrics reporters, `disable-state-exporter`,
/// the nine in C143), and no operator needs to tune this: the number is a property of the topology, not
/// of a deployment. A wallet that legitimately spans more shards than this is a shape the coordinator's
/// own per-leg membership check would refuse anyway.
const MAX_TXN_LEGS: usize = 32;

/// Rate limit for the transaction routes. See [`AdminState::txn_rate_limiter`]; deliberately far
/// stricter than the deploy API's 100/s, because one admitted request signs up to [`MAX_TXN_LEGS`]
/// deploys with the node's own key and holds a 2PC transaction open across phase timeouts.
const TXN_RATE_LIMIT_PER_SEC: u64 = 10;

/// Rate limit for the node-started OCapN dial (issue #249). Far below the deploy API's 100/s: each
/// admitted request makes the node open (or reuse) a session to a caller-named peer and fetch an
/// object over it — an **egress** primitive, and one whose cost is a connection to someone else's
/// host, so it is bounded like the transaction routes rather than like a read.
const OCAPN_DIAL_RATE_LIMIT_PER_SEC: u64 = 2;

/// Rate limit for the autopropose-resume route (W-N10-01, F-U10-02). Deliberately tiny: the route
/// clears a durable halt and wakes a parked timer, which is an operator act performed once, not a
/// control loop. A bound here is not for throughput — it is so that a page in a browser or a retrying
/// script cannot hammer the node's own actuator, the same reason the sibling admin routes have one.
const RESUME_AUTOPROPOSE_RATE_LIMIT_PER_SEC: u64 = 1;

/// The resource-fallback resume is an operator action like the autopropose resume: one per second.
const RESUME_RESOURCE_RATE_LIMIT_PER_SEC: u64 = 1;

/// The subject an ingress bound uses when the request carries no connection information.
///
/// That happens when a router is called directly rather than served — which is what the tests do. One
/// shared bucket is the honest reading: the bound still applies and its subject is simply "everyone",
/// exactly as it was before the bound was keyed. Deliberately **not** a bypass, because a bypass would
/// be indistinguishable from the defect.
const UNATTRIBUTED_PEER: &[u8] = b"unattributed-peer";

/// An opt-in bound on executing HTTP requests, separate from the per-peer rate limit.
/// One zero-delay admission attempt is made before JSON extraction; refusal is 503.
/// No default number is guessed for a validator without resource measurements.
const MAX_CONFIGURED_HTTP_INFLIGHT: usize = 1024;

#[derive(Clone)]
pub struct InFlightAdmission {
    limit: usize,
    semaphore: Arc<Semaphore>,
}

impl InFlightAdmission {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            semaphore: Arc::new(Semaphore::new(limit)),
        }
    }
}

fn parse_inflight_limit(
    raw: Option<&str>,
    name: &str,
) -> Result<Option<InFlightAdmission>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let limit = raw.parse::<usize>().map_err(|_| {
        format!("{name} must be an integer from 1 to {MAX_CONFIGURED_HTTP_INFLIGHT}")
    })?;
    if !(1..=MAX_CONFIGURED_HTTP_INFLIGHT).contains(&limit) {
        return Err(format!(
            "{name} must be an integer from 1 to {MAX_CONFIGURED_HTTP_INFLIGHT}"
        ));
    }
    Ok(Some(InFlightAdmission::new(limit)))
}

fn inflight_from_env(name: &str) -> Result<Option<InFlightAdmission>, String> {
    match std::env::var(name) {
        Ok(value) => parse_inflight_limit(Some(&value), name),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(_) => Err(format!("{name} is not valid Unicode")),
    }
}

/// Comm state needed by `GET /status` (port of the `ConnectionsCell`/`NodeDiscovery`/`RPConfAsk`
/// arguments of `StatusInfo.service`).
#[derive(Clone)]
pub struct StatusProvider {
    pub connections: ConnectionsCell,
    pub rp_conf: RPConf,
    pub discovery: Arc<dyn NodeDiscovery>,
}

/// The shards a node validates for, primary first, each with the API that reads its head. Served by
/// `GET /api/v1/shards` — deliberately *not* folded into `/api/status`, whose `shardId` field
/// existing tooling extracts with a single greedy match.
#[derive(Clone)]
pub struct ShardRegistry {
    pub primary: ShardId,
    pub members: Vec<(ShardId, Arc<dyn BlockApi>)>,
}

/// State shared by the public HTTP server (port of the `webApi` + `prometheusReporter` +
/// `blockReportAPI` arguments of `acquireHttpServer`).
#[derive(Clone)]
pub struct HttpState {
    pub reporter: Arc<NewPrometheusReporter>,
    /// The node's metric registry, so `/metrics` can publish what the node has accumulated. Nothing
    /// else in production called `report_period_snapshot`, so `/metrics` served the placeholder
    /// `EMPTY_SCRAPE_DATA` forever (`prometheus_reporter.rs`): the surface existed, the wire did not.
    pub metrics: Arc<MetricsRegistry>,
    pub web_api: Arc<dyn WebApi>,
    pub block_report_api: Arc<BlockReportApi>,
    pub shards: Arc<ShardRegistry>,
    pub status_provider: Option<StatusProvider>,
    /// The primary shard's PoS read (`GET /api/v1/pos`, AUDIT C148): epoch, boundary distance,
    /// active validator set, pending withdrawals. Separate from `web_api` because it reads native
    /// state rather than the block API.
    pub pos: Arc<dyn PosReadApi>,
    pub enable_reporting: bool,
    /// Rate limiter for the unauthenticated deploy/explore-deploy routes (documented Scala
    /// deviation: the Scala HTTP deploy routes are unlimited).
    pub deploy_rate_limiter: Arc<KeyedRateLimiter>,
    /// Rate limiter for `/api/v1/explore-deploy` and its by-block-hash sibling — **separate from
    /// `deploy_rate_limiter` since AUDIT R36**, because they are different resources.
    ///
    /// An exploratory deploy *runs a term*; a deploy only validates one and pools it. Sharing one
    /// budget meant an explore flood spent the deploy budget, so the two routes starved each other
    /// for a resource only one of them uses heavily. The faucet already had its own limiter, which is
    /// the precedent; this is the same shape one route over.
    ///
    /// The *rate* is deliberately unchanged at `DEFAULT_API_RATE_LIMIT_PER_SEC`. R36 is about the
    /// sharing, and "explore is more expensive per request, so it deserves a stricter number" is a
    /// policy question with no oracle behind it — the Scala's HTTP deploy routes are unlimited — so it
    /// is left where it was rather than answered by a guess.
    pub explore_rate_limiter: Arc<KeyedRateLimiter>,
    /// Optional process-wide in-flight bounds. They do not use a memory
    /// estimate or claim to prevent OOM; the operator must set each separately.
    pub deploy_admission: Option<InFlightAdmission>,
    pub explore_admission: Option<InFlightAdmission>,
    /// **The memory gate** (unit B34): evaluated before a deploy or explore body is decoded.
    ///
    /// It reads the watchdog's newest sample and refuses when headroom is below the admission floor,
    /// when the sample is stale, or when the facts could not be read. The optional semaphores above
    /// are a *concurrency* bound; this is a *memory* bound — they answer different questions, and
    /// neither substitutes for the other.
    pub pressure: PressureGate,
    /// Rate limiter for the devnet faucet endpoint.
    pub faucet_rate_limiter: Arc<KeyedRateLimiter>,
    /// Whether the faucet routes are mounted at all — dev mode **and** a deployer key, which is the
    /// handler's own condition, carried here so the mount and the handler cannot disagree (AUDIT
    /// R34). When false the routes do not exist and the documented **404** is what a caller gets.
    pub faucet_enabled: bool,
}

/// State shared by the admin HTTP server (port of the `adminWebApiRoutes` argument of
/// `acquireAdminHttpServer`).
///
/// **Why the cross-shard coordinator lives here and not on the public state.** It used to be a field
/// of `HttpState`, and the transaction routes with it — on a server that binds `api-server.host`
/// (`0.0.0.0`) by default. That made `POST /api/txn` a *"spend this node's validator REV to an address
/// I choose"* primitive with no caller identity and no rate limit: the coordinator signs every phase
/// deploy with the node's own key, and `rho:txn prepare` derives the escrow source from that signing
/// identity, so the funds moved out of the operator's account (AUDIT C121). The admin server is the
/// one this codebase already treats as privileged: it binds loopback unless
/// `api-server.enable-devnet-admin-public` is set, the same opt-in that protects the unauthenticated
/// `/api/propose` (C112). Moving the capability rather than guarding the route means the public state
/// no longer *holds* a gateway, so the fund-moving routes cannot be mounted there by accident later.
#[derive(Clone)]
pub struct AdminState {
    pub admin_web_api: Arc<dyn AdminWebApi>,
    pub enable_devnet_cors: bool,
    /// The on-node cross-shard 2PC coordinator, present only when this node is a multi-shard
    /// gateway (several memberships and a signing key).
    pub gateway: Option<Arc<GatewayTxn>>,
    /// Whether the cross-shard transaction routes are enabled (`api-server.enable-txn-api`).
    ///
    /// It remains a gate *in addition to* the bind: a gateway operator who does not serve
    /// transactions never mounts them, and one who does still has to publish the admin port to reach
    /// them from another host.
    pub enable_txn_api: bool,
    /// Rate limiter for the transaction routes. Every sibling state-mutating route consults one
    /// (`api_deploy`, `api_faucet`, both explore-deploy handlers) and this was the only exception —
    /// the asymmetry AUDIT C121 rests on. The limit is lower than the deploy API's 100/s on purpose:
    /// one admitted request can open up to [`MAX_TXN_LEGS`] node-signed deploys and drives a 2PC
    /// transaction that holds per-phase timeouts.
    pub txn_rate_limiter: Arc<KeyedRateLimiter>,
    /// The node's **outbound** OCapN surface (issue #249), published by the listener once its
    /// transports are bound. An empty slot means no transport is bound — the listener has not started
    /// or has none configured — and the dial route answers 503 rather than dialing with no layer.
    pub ocapn_dial: crate::api::ocapn::OcapnDialSlot,
    /// Whether the node-started dial route is enabled (`api-server.enable-ocapn-dial`). Off by
    /// default: it is an egress primitive, so it gets an explicit switch, as `enable_txn_api` does.
    pub enable_ocapn_dial: bool,
    /// Rate limiter for the dial route. See [`OCAPN_DIAL_RATE_LIMIT_PER_SEC`].
    pub ocapn_dial_rate_limiter: Arc<KeyedRateLimiter>,
    /// **The PoS read, which is where a delegator's positions are served from** (W-N3-03).
    ///
    /// It was on `HttpState` and the route was on the public router, so `/api/v1/pos/delegations`
    /// answered any caller's question about any delegator's portfolio. It is a *store query* keyed on
    /// a caller-supplied key, not chain state a peer would read, and the admin listener is where this
    /// codebase puts the surfaces that act with the operator's authority.
    pub pos: Arc<dyn PosReadApi>,
    /// Rate limiter for the autopropose-resume route (W-N10-01). Every sibling actuator on this
    /// listener consults one; the resume route gets the same treatment because it is what pages an
    /// operator's browser (and a retrying script) would otherwise hit unbounded. See
    /// [`RESUME_AUTOPROPOSE_RATE_LIMIT_PER_SEC`].
    pub resume_rate_limiter: Arc<KeyedRateLimiter>,
}

/// `GET /version` (port of `VersionInfo.service`): the node version string.
pub async fn version() -> String {
    version_info::node_version()
}

/// `GET /metrics` (port of `NewPrometheusReporter.service`): the Prometheus scrape data.
///
/// The snapshot is published *before* the render, so a scrape reflects the registry's current
/// values. This is the wire that was missing: without it `/metrics` returned the reporter's initial
/// placeholder string, because the reporter only renders inside `report_period_snapshot` and no
/// production path called it.
pub async fn metrics(State(state): State<HttpState>) -> String {
    // **Rendered, not accumulated.** `report_period_snapshot` merges into a period accumulator, which
    // is right for kamon's periodic reporter and wrong for a per-request endpoint over a *cumulative*
    // registry: each scrape added the running totals to themselves, so the merge histogram read
    // `_count 7617` against a census of 101 and the factor was how often someone scraped. See
    // `NewPrometheusReporter::render`.
    state.reporter.render(&state.metrics.snapshot())
}

/// `GET /status` (port of `StatusInfo.service`): the node address, version, and peer/node counts.
pub async fn status(State(state): State<HttpState>) -> Response {
    match &state.status_provider {
        Some(provider) => {
            let connections = provider.connections.read().await;
            let discovered = provider.discovery.peers();
            let version = version_info::node_version();
            let status =
                status_info::status(&version, &connections, &discovered, &provider.rp_conf);
            (StatusCode::OK, Json(status)).into_response()
        }
        None => (StatusCode::NOT_FOUND, ()).into_response(),
    }
}

/// Map a `WebApi`/`AdminWebApi` result to an HTTP response: `200` with a JSON body on success,
/// `400` with a JSON error string on `BlockApiException` (port of the `handleResponseError`
/// handler in `WebApiRoutes`).
fn json_result<T: Serialize>(result: Result<T, BlockApiException>) -> Response {
    match result {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(err) => (StatusCode::BAD_REQUEST, Json(err.0)).into_response(),
    }
}

// --- Web API routes (port of `WebApiRoutes.service`) ---

async fn api_status(State(state): State<HttpState>) -> Response {
    json_result(state.web_api.status().await)
}

/// `GET /api/v1/status/operator` — **the operator half of the status, on the admin listener**
/// (W-N3-01).
///
/// A **distinct path** rather than the same path with a second schema, for two reasons: OpenAPI cannot
/// hold two schemas at one path, and tooling that greps `/api/status` keeps working. The public
/// `/api/status` no longer carries a single operator field.
async fn api_operator_status(State(state): State<AdminState>) -> Response {
    json_result(state.admin_web_api.operator_status().await)
}

async fn api_deploys(State(state): State<HttpState>) -> Response {
    json_result(state.web_api.pooled_deploys().await)
}

async fn api_capabilities(State(state): State<HttpState>) -> Response {
    json_result(state.web_api.capabilities().await)
}

/// `GET /api/v1/shards` — the shards this node is a member of (a gateway is a member of several),
/// primary first, each with the height of its own chain. A client that needs to address a specific
/// shard reads the ids here; a deploy already names its shard in `DeployData.shardId`.
async fn api_shards(State(state): State<HttpState>) -> Response {
    let mut shards = Vec::with_capacity(state.shards.members.len());
    for (shard_id, api) in &state.shards.members {
        let status = api.status().await;
        shards.push(json!({
            "shardId": shard_id.to_string(),
            "primary": *shard_id == state.shards.primary,
            "latestBlockNumber": status.latest_block_number,
        }));
    }
    Json(json!({
        "primaryShard": state.shards.primary.to_string(),
        "shardCount": shards.len(),
        "shards": shards,
    }))
    .into_response()
}

/// `GET /api/v1/pos` — the PoS reads this node had no surface for (AUDIT C148): the epoch, how far
/// the next boundary is, the active validator set, and every staged withdrawal with its countdown.
///
/// A failed read is the node's own store rather than the caller's request, so it answers **500 with
/// the reason** instead of the 400 the block-API routes use for a refusal.
async fn api_pos_status(State(state): State<HttpState>) -> Response {
    match state.pos.pos_status().await {
        Ok(status) => Json(status).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e })),
        )
            .into_response(),
    }
}

/// **The delegator-scoped PoS read** (#193): one key's positions, across every operator it has staked
/// with. Separate from `/api/v1/pos` rather than a field on its response, because that read is a
/// single object about a shard and the delegation ledger is unbounded in delegator count — hanging it
/// off the status read would make one call's size a function of how many people have ever delegated.
#[derive(Deserialize)]
struct PosDelegationsQuery {
    delegator: String,
}

async fn api_pos_delegations(
    State(state): State<AdminState>,
    Query(query): Query<PosDelegationsQuery>,
) -> Response {
    // `400` rather than an empty list for a malformed key: an empty list is a *true answer* about a
    // delegator with no positions, and a caller that mistyped its own key must not be told that.
    let Some(bytes) = base16::decode(&query.delegator) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "delegator must be a hex-encoded 65-byte public key" })),
        )
            .into_response();
    };
    let Ok(delegator) = Validator::try_from(bytes.as_slice()) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "delegator must be a hex-encoded 65-byte public key" })),
        )
            .into_response();
    };
    match state.pos.delegator_positions(&delegator).await {
        Ok(positions) => Json(positions).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e })),
        )
            .into_response(),
    }
}

/// The gateway, or the standard not-available response.
///
/// Mirrors the `enable-reporting` convention: the route is mounted unconditionally and answers 404
/// when the feature is off, so a single-shard node's surface is unchanged and a client can tell
/// "not a gateway" from "bad request".
///
/// **The state is the admin server's, and that is the authorization boundary** (AUDIT C121): the routes
/// live on the loopback-by-default listener, so the 404 here means "this node does not serve
/// transactions" rather than "this node will spend its REV for anyone who asks".
fn gateway_or_not_found(state: &AdminState) -> Result<Arc<GatewayTxn>, Response> {
    match (&state.gateway, state.enable_txn_api) {
        (Some(gateway), true) => Ok(gateway.clone()),
        _ => Err((StatusCode::NOT_FOUND, ()).into_response()),
    }
}

/// The dial route's gate: **404** when the feature is off, **503** when the listener has bound no
/// transport.
///
/// Mirrors [`gateway_or_not_found`]: mounted unconditionally so a node without the feature has an
/// unchanged surface, and a client can tell "this node does not dial" (404) from "not ready yet"
/// (503) rather than reading one status for both.
fn dialer_or_not_found(state: &AdminState) -> Result<crate::api::ocapn::OcapnDialer, Response> {
    if !state.enable_ocapn_dial {
        return Err((StatusCode::NOT_FOUND, ()).into_response());
    }
    match state.ocapn_dial.get() {
        Some(dialer) => Ok(dialer.clone()),
        // Enabled, but the OCapN listener has bound no transport: there is no layer to dial with, so
        // say so rather than attempting a dial that would fail with a less useful reason.
        None => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(
                "the OCapN listener has no transport bound; set api-server.ocapn-listen or \
                 api-server.ocapn-listen-unix"
                    .to_string(),
            ),
        )
            .into_response()),
    }
}

/// The body of `POST /api/v1/ocapn/dial`: the peer to dial and the object to fetch there.
#[derive(Deserialize)]
pub struct OcapnDialRequest {
    /// The peer's designator — the name it advertises, and half of its OCapN identity.
    pub designator: String,
    /// The transport to reach it on: `tcp-testing-only`, `unix`, `noise` or `websocket`. It is passed
    /// through to the dialing dispatcher verbatim, which refuses an unknown name by listing the ones
    /// this node speaks.
    pub transport: String,
    /// The transport's hints — `host` and `port` for tcp, `path` for unix.
    #[serde(default)]
    pub hints: std::collections::BTreeMap<String, String>,
    /// The swiss number of the object to fetch, base16-encoded.
    pub swiss: String,
}

/// `POST /api/v1/ocapn/dial` — the node dials the peer the request names and fetches the object at the
/// swiss number it gives (issue #249).
///
/// **This is the node starting a session of its own.** Until here the node dialled only when a peer's
/// sturdyref or handoff give asked it to; this route is the local surface that makes it originate one.
/// It reuses `Enlivener::dial_and_fetch`, so a node-started dial and a peer-asked one are the same
/// code rather than two that could drift.
///
/// **The guard is the target policy, not an origin.** A node-started dial carries no peer origin — the
/// session slot it is built over is empty — so Law 62's origin rule has nothing to judge. What decides
/// whether a target may be dialled is `api-server.ocapn-deny-local-dial` (applied by the transport's
/// policy), plus this route's own `enable-ocapn-dial` gate, the loopback-by-default admin bind,
/// `admin_origin_guard`, and the rate limiter. It is served on the admin listener for the reason
/// `/api/propose` and `/api/v1/txn` are: it makes the node act, on the caller's word, against a peer
/// of the caller's choosing.
async fn admin_ocapn_dial(
    State(state): State<AdminState>,
    Json(req): Json<OcapnDialRequest>,
) -> Response {
    let dialer = match dialer_or_not_found(&state) {
        Ok(dialer) => dialer,
        Err(response) => return response,
    };
    let Some(swiss) = base16::decode(&req.swiss) else {
        return (
            StatusCode::BAD_REQUEST,
            Json("`swiss` is not base16".to_string()),
        )
            .into_response();
    };
    let peer = rchain_ocapn::locator::PeerLocator {
        designator: req.designator,
        transport: req.transport,
        hints: req.hints,
    };
    match dialer.dial_and_fetch(&peer, &swiss).await {
        // The fetch succeeded. The answer is a **descriptor** on that peer's table — the object the
        // swiss number named — which is what the node now holds a live reference to.
        Ok((_session, to, _value)) => Json(json!({
            "peer": format!("{}.{}", peer.designator, peer.transport),
            "fetched": format!("{to:?}"),
        }))
        .into_response(),
        // 502: the node could not complete the dial-and-fetch. The reason is the one the transport or
        // the policy gave — a refused dial, a peer that did not answer, a policy refusal — so a
        // link-local target is refused *with its reason* rather than a bare status.
        Err(reason) => (StatusCode::BAD_GATEWAY, Json(reason)).into_response(),
    }
}

/// `POST /api/v1/txn` — open (or resume) a cross-shard transaction and drive it to a terminal state.
///
/// The legs are ordinary deploys on this node's own shards, so the call returns once each leg has
/// been included in a block — seconds, not microseconds — and can fail by timeout. It is
/// idempotent under `txnId`: re-issuing a completed transaction returns its record and moves no
/// funds.
///
/// **Served on the admin server, and the caller's legs are bounded** (AUDIT C121). The coordinator
/// signs every phase deploy with the node's validator key and the escrow is taken from that key's own
/// vault, so this handler is a spend surface for the operator's account; it sits behind the same
/// loopback-by-default bind as `/api/propose` (C112), consults a rate limiter like every sibling
/// state-mutating route, and refuses a leg list longer than [`MAX_TXN_LEGS`] before the gateway is
/// driven.
async fn api_txn_run(State(state): State<AdminState>, Json(req): Json<TxnRequest>) -> Response {
    let gateway = match gateway_or_not_found(&state) {
        Ok(gateway) => gateway,
        Err(response) => return response,
    };
    if req.legs.len() > MAX_TXN_LEGS {
        return (
            StatusCode::BAD_REQUEST,
            Json(format!(
                "a transaction may name at most {MAX_TXN_LEGS} legs, this one names {}",
                req.legs.len()
            )),
        )
            .into_response();
    }
    let txn_id = match base16::decode(&req.txn_id) {
        Some(bytes) if !bytes.is_empty() && bytes.len() <= 64 => bytes,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json("txnId must be non-empty hex of at most 64 bytes".to_string()),
            )
                .into_response()
        }
    };
    let mut legs = Vec::with_capacity(req.legs.len());
    for leg in &req.legs {
        match rchain_shared::refined::ShardId::try_from(leg.shard_id.clone()) {
            // An empty `to` is the one field of a leg with no check anywhere behind this handler: the
            // amount is caught by the ledger's `NonNegI64` refinement in `GatewayTxn::run`, and the
            // shard id above. A commit that credits `""` is not a request the coordinator should open
            // (AUDIT §8's validate-on-ingress note, corrected: everything else *is* stopped, just later
            // than the boundary).
            Ok(_) if leg.to.trim().is_empty() => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(format!("leg on shard '{}' has an empty 'to'", leg.shard_id)),
                )
                    .into_response()
            }
            Ok(shard_id) => legs.push(rchain_casper::gateway::GatewayLeg {
                shard_id,
                amount: leg.amount,
                to: leg.to.clone(),
            }),
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(format!("invalid shardId '{}': {e}", leg.shard_id)),
                )
                    .into_response()
            }
        }
    }

    match gateway.run(&txn_id, &legs).await {
        Ok(record) => (StatusCode::OK, Json(TxnRecordDto::from_record(&record))).into_response(),
        Err(err) => (StatusCode::BAD_REQUEST, Json(err)).into_response(),
    }
}

/// `GET /api/v1/txn/:txnId` — the durable record of a transaction, or 404 when this node has none.
async fn api_txn_status(State(state): State<AdminState>, Path(txn_id): Path<String>) -> Response {
    let gateway = match gateway_or_not_found(&state) {
        Ok(gateway) => gateway,
        Err(response) => return response,
    };
    let Some(txn_id) = base16::decode(&txn_id) else {
        return (
            StatusCode::BAD_REQUEST,
            Json("txnId must be hex".to_string()),
        )
            .into_response();
    };
    match gateway.status(&txn_id).await {
        Ok(Some(record)) => {
            (StatusCode::OK, Json(TxnRecordDto::from_record(&record))).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, ()).into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, Json(err)).into_response(),
    }
}

/// `GET /api/v1/txn` — the transactions this node is still coordinating.
async fn api_txn_list(State(state): State<AdminState>) -> Response {
    let gateway = match gateway_or_not_found(&state) {
        Ok(gateway) => gateway,
        Err(response) => return response,
    };
    match gateway.list().await {
        Ok(records) => {
            let records: Vec<TxnRecordDto> =
                records.iter().map(TxnRecordDto::from_record).collect();
            Json(json!({ "inFlight": records })).into_response()
        }
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, Json(err)).into_response(),
    }
}

async fn api_deploy(State(state): State<HttpState>, Json(req): Json<DeployRequest>) -> Response {
    json_result(state.web_api.deploy(&req).await)
}

async fn api_faucet(State(state): State<HttpState>, Json(req): Json<FaucetRequest>) -> Response {
    json_result(state.web_api.faucet(&req.address).await)
}

async fn api_explore_deploy(State(state): State<HttpState>, Json(term): Json<String>) -> Response {
    json_result(state.web_api.exploratory_deploy(&term, None, false).await)
}

async fn api_explore_deploy_by_block_hash(
    State(state): State<HttpState>,
    Json(req): Json<ExploreDeployRequest>,
) -> Response {
    let block_hash = if req.block_hash.is_empty() {
        None
    } else {
        Some(req.block_hash.as_str())
    };
    json_result(
        state
            .web_api
            .exploratory_deploy(&req.term, block_hash, req.use_pre_state_hash)
            .await,
    )
}

async fn api_data_at_name(
    State(state): State<HttpState>,
    Json(req): Json<DataAtNameRequest>,
) -> Response {
    json_result(state.web_api.listen_for_data_at_name(&req).await)
}

async fn api_data_at_name_by_block_hash(
    State(state): State<HttpState>,
    Json(req): Json<DataAtNameByBlockHashRequest>,
) -> Response {
    json_result(state.web_api.get_data_at_par(&req).await)
}

async fn api_last_finalized_block(State(state): State<HttpState>) -> Response {
    json_result(state.web_api.last_finalized_block().await)
}

async fn api_get_block(State(state): State<HttpState>, Path(hash): Path<String>) -> Response {
    json_result(state.web_api.get_block(&hash).await)
}

/// `GET /api/v1/block/{hash}/raw` — the preimage of the block's own hash (AUDIT C261).
///
/// This exists because the summary route cannot be verified: `LightBlockInfo` is built from a stored
/// block and the protobuf it came from is not reconstructible out of it, so a client reading a height
/// or a justification from it is reading the node's word. With the preimage, the client hashes it,
/// compares to `blockHash`, and then reads the fields off bytes it has verified.
async fn api_get_raw_block(State(state): State<HttpState>, Path(hash): Path<String>) -> Response {
    json_result(state.web_api.raw_block(&hash).await)
}

/// `GET /api/v1/finality-attestation?nonce=…` — what this node has finalised, signed by its validator
/// key (AUDIT C261). A node-local claim cannot be weighed; an attestation signed by a bonded key can.
async fn api_finality_attestation(
    State(state): State<HttpState>,
    Query(query): Query<FinalityAttestationQuery>,
) -> Response {
    json_result(state.web_api.finality_attestation(&query.nonce).await)
}

/// The attestation's query (`nonce`, required — see the handler's route documentation).
#[derive(Deserialize)]
struct FinalityAttestationQuery {
    nonce: String,
}

async fn api_get_blocks(State(state): State<HttpState>) -> Response {
    json_result(state.web_api.get_blocks(1).await)
}

async fn api_get_blocks_by_heights(
    State(state): State<HttpState>,
    Path((start, end)): Path<(i64, i64)>,
) -> Response {
    json_result(state.web_api.get_blocks_by_heights(start, end).await)
}

async fn api_get_blocks_by_depth(
    State(state): State<HttpState>,
    Path(depth): Path<i32>,
) -> Response {
    json_result(state.web_api.get_blocks(depth).await)
}

async fn api_find_deploy(
    State(state): State<HttpState>,
    Path(deploy_id): Path<String>,
) -> Response {
    json_result(state.web_api.find_deploy(&deploy_id).await)
}

/// `GET /api/v1/deployer/{key}` — has this key signed a deploy in a block, i.e. is it public? `key`
/// is the hex `blake2b256` hash of the public key (so asking does not reveal it) or the key itself.
/// Serves the wallet's quantum key-hygiene check (rholo post-quantum plan §16.1) in one
/// lookup instead of a scan of every block.
async fn api_find_deployer(State(state): State<HttpState>, Path(key): Path<String>) -> Response {
    json_result(state.web_api.find_deployer(&key).await)
}

async fn api_is_finalized(State(state): State<HttpState>, Path(hash): Path<String>) -> Response {
    json_result(state.web_api.is_finalized(&hash).await)
}

async fn api_get_transaction(State(state): State<HttpState>, Path(hash): Path<String>) -> Response {
    // Reporting is disabled by default (`api-server.enable-reporting = false`); the route answers
    // 404 unless explicitly enabled (M6 — the flag was read but never enforced for the transaction
    // route).
    if !state.enable_reporting {
        return (StatusCode::NOT_FOUND, ()).into_response();
    }
    json_result(state.web_api.get_transaction(&hash).await)
}

// --- Web API v1 routes (port of `WebApiRoutesV1`; the OpenAPI schema is served below) ---

async fn api_v1_deploy_status(
    State(state): State<HttpState>,
    Path(deploy_signature): Path<String>,
) -> Response {
    json_result(state.web_api.deploy_status(&deploy_signature).await)
}

/// `GET /api/v1/openapi.json` — the OpenAPI 3.0 document describing the v1 API. Hand-written from the
/// endpoint DTOs (the Scala derives the same schema from its endpoints4s algebra).
/// The OpenAPI document this node serves at `GET /api/v1/openapi` — the schema a client generates its
/// types from. It is hand-written, so nothing but the corpus keeps it equal to what the endpoints
/// actually serialize; `node/tests/lean_envelope_corpus.rs` (law 43) holds it to
/// `spec/Rchain/Envelope.lean`'s catalog, which the DTOs are held to as well.
pub const OPENAPI_JSON: &str = r##"{
  "openapi": "3.0.0",
  "info": { "title": "RNode API", "version": "1.0" },
  "servers": [ { "url": "/api/v1" } ],
  "paths": {
    "/status": {
      "get": {
        "summary": "Node status",
        "responses": {
          "200": {
            "description": "Node version, address and peer/node counts",
            "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiStatus" } } }
          }
        }
      }
    },
    "/admission": {
      "get": {
        "summary": "Configured HTTP in-flight admission bounds",
        "description": "Deploy and explore concurrency budget, not a memory, finality or readiness signal. Unset budgets return configured=false.",
        "responses": {
          "200": { "description": "HTTP concurrency-only admission state", "content": { "application/json": { "schema": { "type": "object", "properties": {
            "deploy": { "$ref": "#/components/schemas/HttpAdmissionBudget" },
            "explore": { "$ref": "#/components/schemas/HttpAdmissionBudget" }
          } } } } }
        }
      }
    },
    "/capabilities": {
      "get": {
        "summary": "What this node's API can do",
        "description": "The `ApiStatus` boolean fields, plus the faucet. A client reads this to decide whether to offer `propose`, `deploy`, or the devnet faucet.",
        "responses": {
          "200": { "description": "Capabilities", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/NodeCapabilities" } } } }
        }
      }
    },
    "/deploys": {
      "get": {
        "summary": "The deploys waiting in the deploy pool",
        "responses": {
          "200": { "description": "Pooled deploys", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/PooledDeploys" } } } }
        }
      }
    },
    "/faucet": {
      "post": {
        "summary": "Dev-mode faucet",
        "description": "Transfers dev REV to an address. Present only in dev mode; answers 404 otherwise.",
        "responses": {
          "200": { "description": "The transfer's deploy id, amount and recipient", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/FaucetResponse" } } } },
          "404": { "description": "The faucet is disabled on this node" }
        }
      }
    },
    "/shards": {
      "get": {
        "summary": "The shards this node is a member of",
        "description": "Primary shard first, each with the height of its own chain. A gateway node is a member of several. Deploys name their target shard in `DeployRequest.data.shardId`.",
        "responses": {
          "200": { "description": "Membership list", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ShardsResponse" } } } }
        }
      }
    },
    "/txn": {
      "post": {
        "summary": "Open or resume a cross-shard transaction",
        "description": "Runs a two-phase commit across this node's own member shards: each leg is an ordinary deploy on the shard that owns it, so the call returns once every leg is in a block (seconds) and can fail by timeout. Idempotent under `txnId`: re-issuing a completed transaction returns its record and moves no funds. 404 when this node is not a multi-shard gateway.",
        "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/TxnRequest" } } } },
        "responses": {
          "200": { "description": "The durable coordinator record", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/TxnRecord" } } } },
          "400": { "description": "Invalid txnId, a non-member shard, or a failed leg" },
          "404": { "description": "This node is not a gateway (`api-server.enable-txn-api` is off, or it has a single shard)" }
        }
      },
      "get": {
        "summary": "Transactions this node is still coordinating",
        "responses": {
          "200": { "description": "The in-flight records", "content": { "application/json": { "schema": { "type": "object", "properties": { "inFlight": { "type": "array", "items": { "$ref": "#/components/schemas/TxnRecord" } } } } } } },
          "404": { "description": "This node is not a gateway" }
        }
      }
    },
    "/txn/{txnId}": {
      "get": {
        "summary": "A cross-shard transaction's durable record",
        "parameters": [ { "name": "txnId", "in": "path", "required": true, "schema": { "type": "string" }, "description": "Hex-encoded transaction id" } ],
        "responses": {
          "200": { "description": "The coordinator record", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/TxnRecord" } } } },
          "400": { "description": "txnId is not hex" },
          "404": { "description": "Unknown transaction, or this node is not a gateway" }
        }
      }
    },
    "/deploy": {
      "post": {
        "summary": "Deploy a signed rholang term",
        "servers": [ { "url": "/api" }, { "url": "/api/v1" } ],
        "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/DeployRequest" } } } },
        "responses": {
          "200": { "description": "Deploy accepted", "content": { "application/json": { "schema": { "type": "string" } } } },
          "400": { "description": "Invalid deploy" },
          "503": { "$ref": "#/components/responses/HttpAdmissionExhausted" }
        }
      }
    },
    "/deploy-status/{deploySignature}": {
      "get": {
        "summary": "Deploy execution status",
        "parameters": [ { "name": "deploySignature", "in": "path", "required": true, "schema": { "type": "string" } } ],
        "responses": {
          "200": { "description": "Deploy execution status", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/DeployExecStatus" } } } },
          "400": { "description": "Invalid deploy signature" }
        }
      }
    },
    "/explore-deploy": {
      "post": {
        "summary": "Run an exploratory deploy",
        "servers": [ { "url": "/api" }, { "url": "/api/v1" } ],
        "requestBody": { "required": true, "content": { "application/json": { "schema": { "type": "string" } } } },
        "responses": {
          "200": { "description": "Result expression", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ExploratoryDeployResponse" } } } },
          "400": { "description": "Deploy failed" },
          "503": { "$ref": "#/components/responses/HttpAdmissionExhausted" }
        }
      }
    },
    "/explore-deploy-by-block-hash": {
      "post": {
        "summary": "Exploratory deploy at a block hash",
        "servers": [ { "url": "/api" }, { "url": "/api/v1" } ],
        "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ExploreDeployRequest" } } } },
        "responses": {
          "200": { "description": "Result expression", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ExploratoryDeployResponse" } } } },
          "400": { "description": "Deploy failed" },
          "503": { "$ref": "#/components/responses/HttpAdmissionExhausted" }
        }
      }
    },
    "/data-at-name-by-block-hash": {
      "post": {
        "summary": "Data at a name, at a block hash",
        "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/DataAtNameByBlockHashRequest" } } } },
        "responses": {
          "200": { "description": "Data at the name", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/RhoDataResponse" } } } },
          "400": { "description": "Invalid request" }
        }
      }
    },
    "/blocks": {
      "get": {
        "summary": "Recent blocks",
        "responses": {
          "200": { "description": "Recent lightweight block info", "content": { "application/json": { "schema": { "type": "array", "items": { "$ref": "#/components/schemas/LightBlockInfo" } } } } }
        }
      }
    },
    "/block/{hash}": {
      "get": {
        "summary": "A block by hash",
        "parameters": [ { "name": "hash", "in": "path", "required": true, "schema": { "type": "string" } } ],
        "responses": {
          "200": { "description": "Block and its deploys", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/BlockInfo" } } } },
          "400": { "description": "Invalid block hash" }
        }
      }
    },
    "/block/{hash}/raw": {
      "get": {
        "summary": "The preimage of the block's own hash, so a client verifies the block instead of trusting a summary of it",
        "parameters": [ { "name": "hash", "in": "path", "required": true, "schema": { "type": "string" } } ],
        "responses": {
          "200": { "description": "The bytes the block hash is taken over, plus the signature over that hash", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/RawBlock" } } } },
          "400": { "description": "Invalid block hash, or a block this node has not added" }
        }
      }
    },
    "/finality-attestation": {
      "get": {
        "summary": "This node's finalised block, signed by its validator key and bound to the caller's nonce",
        "parameters": [ { "name": "nonce", "in": "query", "required": true, "schema": { "type": "string" } } ],
        "responses": {
          "200": { "description": "The attestation", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/FinalityAttestation" } } } },
          "400": { "description": "Missing or too-short nonce, or this node has no validator identity" }
        }
      }
    },
    "/propose": {
      "post": {
        "summary": "Propose a block",
        "responses": {
          "200": { "description": "Proposal result", "content": { "application/json": { "schema": { "type": "string" } } } }
        }
      }
    }
  },
  "components": {
    "responses": {
      "HttpAdmissionExhausted": {
        "description": "Work refused before the request body is decoded: memory headroom below the node's admission floor, the resource sample stale or unreadable, or the operator's HTTP in-flight budget exhausted",
        "headers": {
          "Retry-After": {
            "description": "Delay in seconds before retry; currently 1",
            "schema": { "type": "integer", "minimum": 1, "example": 1 }
          }
        },
        "content": { "application/json": {
          "schema": { "type": "string" },
          "example": "memory headroom below the admission floor: available memory is 10 MiB (cgroup-aware), below the 819 MiB admission floor"
        } }
      }
    },
    "schemas": {
      "VersionInfo": {
        "type": "object",
        "properties": {
          "api": { "type": "string" },
          "node": { "type": "string" }
        }
      },
      "TxnLeg": {
        "type": "object",
        "properties": {
          "shardId": { "type": "string", "description": "The member shard that escrows this leg" },
          "amount": { "type": "integer", "description": "REV to escrow" },
          "to": { "type": "string", "description": "The REV address a commit credits" }
        }
      },
      "TxnRequest": {
        "type": "object",
        "properties": {
          "txnId": { "type": "string", "description": "Hex; caller-supplied so a retry is the same transaction" },
          "legs": { "type": "array", "items": { "$ref": "#/components/schemas/TxnLeg" } }
        }
      },
      "TxnVote": {
        "type": "object",
        "properties": {
          "shardId": { "type": "string" },
          "vote": { "type": "string", "enum": ["ready", "abort"] }
        }
      },
      "TxnRecord": {
        "type": "object",
        "properties": {
          "txnId": { "type": "string" },
          "state": { "type": "string", "enum": ["proposed", "prepared", "committed", "aborted"] },
          "coordinator": { "type": "string", "description": "The key the participants gate commit/abort on" },
          "recordHash": { "type": "string", "description": "The record's content address" },
          "legs": { "type": "array", "items": { "$ref": "#/components/schemas/TxnLeg" } },
          "votes": { "type": "array", "items": { "$ref": "#/components/schemas/TxnVote" } },
          "reason": { "type": "string", "nullable": true, "description": "Why an abort happened, when one did" }
        }
      },
      "ShardsResponse": {
        "type": "object",
        "properties": {
          "primaryShard": { "type": "string", "description": "The default target for requests that do not name a shard" },
          "shardCount": { "type": "integer" },
          "shards": {
            "type": "array",
            "items": {
              "type": "object",
              "properties": {
                "shardId": { "type": "string" },
                "primary": { "type": "boolean" },
                "latestBlockNumber": { "type": "integer" }
              }
            }
          }
        }
      },
      "ApiStatus": {
        "type": "object",
        "description": "**The chain-public half of the node's status** (W-N3-01). The operator half — divergence records, the safety halt, the report counters, the sync task's state, the durable reset marker — is `OperatorStatus`, served on the admin listener at /api/v1/status/operator and never here.",
        "properties": {
          "version": { "$ref": "#/components/schemas/VersionInfo" },
          "address": { "type": "string" },
          "networkId": { "type": "string" },
          "shardId": { "type": "string" },
          "peers": { "type": "integer", "format": "int32" },
          "nodes": { "type": "integer", "format": "int32" },
          "minPhloPrice": { "type": "integer", "format": "int64" },
          "latestBlockNumber": { "type": "integer", "format": "int64" },
          "autopropose": { "type": "boolean", "description": "Continuous block production" },
          "proposeOnDeploy": { "type": "boolean", "description": "Propose immediately after a deploy is accepted" },
          "manualPropose": { "type": "boolean", "description": "Blocks only by an explicit propose" },
          "devMode": { "type": "boolean", "description": "Dev mode is on" },
          "degraded": { "type": "string", "description": "The pressure fallback state: normal | throttled | critical | unavailable (a stale or unreadable sample reports unavailable). A word, not a number; the numbers are on the operator surface" },
          "acceptsDeploys": { "type": "boolean", "description": "Whether the node is accepting deploys right now. True until the admission/fallback units land" }
        }
      },
      "OperatorStatus": {
        "type": "object",
        "description": "**The operator half of the node's status**, served on the admin listener. A superset of `ApiStatus` by construction: every public property appears here too, so a client can move between the two routes without a shape change (W-N3-01).",
        "properties": {
          "version": { "$ref": "#/components/schemas/VersionInfo" },
          "address": { "type": "string" },
          "networkId": { "type": "string" },
          "shardId": { "type": "string" },
          "peers": { "type": "integer", "format": "int32" },
          "nodes": { "type": "integer", "format": "int32" },
          "minPhloPrice": { "type": "integer", "format": "int64" },
          "latestBlockNumber": { "type": "integer", "format": "int64" },
          "autopropose": { "type": "boolean", "description": "Continuous block production" },
          "proposeOnDeploy": { "type": "boolean", "description": "Propose immediately after a deploy is accepted" },
          "manualPropose": { "type": "boolean", "description": "Blocks only by an explicit propose" },
          "devMode": { "type": "boolean", "description": "Dev mode is on" },
          "degraded": { "type": "string", "description": "The pressure fallback state: normal | throttled | critical | unavailable. A word, not a number; the numbers are on the operator surface" },
          "acceptsDeploys": { "type": "boolean", "description": "Whether the node is accepting deploys right now. True until the admission/fallback units land" },
          "adminHttp": { "type": "boolean", "description": "The admin HTTP surface is published" },
          "consecutiveSelfValidationFailures": { "type": "integer", "format": "int64", "description": "Self-validation failures the proposer has recorded in a row; cleared by a successful propose" },
          "autoproposeTimerHalted": { "type": "boolean", "description": "The autopropose timer has stopped. Two things restart it: the process (the halt is durable, re-seeded at boot), and POST /api/v1/autopropose/resume on the admin listener, which clears the record and wakes the parked timer in-process. Not the same as block production having stopped: the tap and POST /api/propose keep running" },
          "autoproposeTimerHaltEpisodes": { "type": "integer", "format": "int64", "description": "How many times this shard's autopropose timer has stopped. Counting transitions, so a second note of the same halt does not move it; seeded from the durable record at boot, so it is the shard's count and not this process's (W-N10-01)" },
          "autoproposeHalt": { "type": "object", "description": "The autopropose halt on record: when it happened and whether the record is trustworthy. A null `at` with `durable: true` means no halt is on record; `durable: false` means the record could not be read or written, so the node cannot vouch for it — 'unknown' and 'never halted' are the two readings this field exists to keep apart (W-N10-01, F-U10-02)", "properties": { "at": { "type": "integer", "format": "int64", "nullable": true, "description": "When the shard last halted, in milliseconds since the unix epoch; null is no halt on record, unless durable is false" }, "durable": { "type": "boolean", "description": "Whether this reading reflects the store: false means the halt record could not be read or written, so the node cannot vouch for `at`" } } },
          "autoproposeTapSuppressed": { "type": "integer", "format": "int64", "description": "Autopropose requests the validated-block tap refused because the timer is halted. The halt is only a halt if the tap obeys it, and this is the measurement of what it cost (W-N4-08)" },
          "staleSnapshotSelfEquivocations": { "type": "integer", "format": "int64", "description": "Times the node's own block collided with its already-synced block at a sequence number derived from a stale parent set. Not a self-validation failure, so it does not halt the timer" },
          "finalityStall": { "type": "string", "nullable": true, "description": "The merge gate's verdict on the last block this node validated, with the numbers that make it a diagnosis (supporting stake, partitions, candidates, and the partition size it measured against). Null when that derivation advanced. NOT an alarm: on a producing chain most blocks are refused by the stake gate, because a block justifies only the messages that existed when it was built — read finalityStalled for that" },
          "finalityStalled": { "type": "boolean", "description": "Whether this node is stalled right now: the derivation has been refused for longer than a round (twice the partition, in consecutive blocks), so the refusals are no longer the ordinary partial layers of a producing chain. This is the field to alarm on; finalityStall above is its diagnosis" },
          "finalityStallEpisodes": { "type": "integer", "format": "int64", "description": "Times this shard has entered a finality stall — the sustained state finalityStalled reports — monotone, so 'stalled now' is distinguishable from 'stalled and recovered'. Persisted, so it survives the restart an operator's alarm would otherwise be cleared by; read it with finalityCountersDurable, which is false when the store could not be reached and the count is this process's instead (W-N10-02)" },
          "finalityCountersDurable": { "type": "boolean", "description": "Whether finalityStallEpisodes, nonQuietMergeReports and unrestorableRecords are the *shard's* history (true) or this process's running totals (false). False means the shard's record could not be read or written, so a restarted node would otherwise read as one that never stalled (W-N10-02)" },
          "resetMarker": { "type": "string", "nullable": true, "description": "The reset marker this node is standing under: the durable record a self-reset writes, rendered with its phase, anchor and height. Null when the node is not mid-reset. The marker is cleared only when the catch-up frontier passes its anchor, so one that stands while the height does not move says the repair is not converging (F-U12-02)" },
          "nonQuietMergeReports": { "type": "integer", "format": "int64", "description": "Merges whose report was not quiet: a chain dropped, or an invariant violated. The log carries the detail; this is the count the live incident would have moved. Persisted with the stall episodes above, so the merge history is the shard's rather than the process's (W-N10-02)" },
          "sync": { "type": "object", "nullable": true, "description": "The LFS sync task's own state while this node is in NodeSyncing — how many attempts it has started and why the last one failed. Null means the sync task has returned, which covers both a restored state and a node that never had to sync; it does NOT mean 'synced fine'", "properties": { "attempts": { "type": "integer", "format": "int64", "description": "Attempts the sync task has started" }, "attemptsScope": { "type": "string", "description": "Always 'SinceProcessStart': the count is the sync task's own and the task restarts with the process, so persisting it would claim a continuity it does not have (W-N10-02). The *scope* half of that row deliberately does not apply here — LFS sync is shard-blind, so a per-shard reading of it would be an invention" }, "lastFailure": { "type": "string", "nullable": true, "description": "The last named sync failure, verbatim; null while the attempt now running has not failed" } } },
          "poisonRecoveries": { "type": "integer", "format": "int64", "description": "Times a poisoned lock was recovered from, which means a panic happened while shared state was held. Process-wide and monotone" },
          "poisonRecoveriesScope": { "type": "string", "description": "Always 'SinceProcessStart': the count is this process's, so a restart correctly zeroes it. Named so a reader does not compare it with the shard-history counters above as though the two had one lifetime (W-N10-02)" },
          "unrestorableRecords": { "type": "integer", "format": "int64", "description": "Blocks this shard has given up on: a divergence record whose revalidation budget is spent and which still fails here, so the node refuses it and its descendants for the process lifetime. Persisted with the two counters above, so the count is the shard's (W-N10-02). Nothing resets a running node yet; this is the counter a reset would act on" },
          "unrestorableBlock": { "type": "string", "nullable": true, "description": "The block the node last gave up on, as '<hash> (seq <n>)'. Null when it never has" },
          "tasksPanicked": { "type": "integer", "format": "int64", "description": "Supervised background tasks that have panicked. Before the supervisor these deaths were invisible: a bare tokio::spawn whose handle was dropped" },
          "tasksPanickedScope": { "type": "string", "description": "Always 'SinceProcessStart' — a supervised task's death is this process's event (W-N10-02)" },
          "tasksExited": { "type": "integer", "format": "int64", "description": "Supervised background tasks that have returned normally, counted apart from panics because a return is the quieter death — nothing announces it anywhere" },
          "tasksExitedScope": { "type": "string", "description": "Always 'SinceProcessStart' — see tasksPanickedScope (W-N10-02)" },
          "undeliveredSends": { "type": "integer", "format": "int64", "description": "Sends a closed channel swallowed. Every production send goes through rchain_shared::chan, which keeps the decision and counts it; monotone and process-wide, so a message dropped into a closed channel is a number a probe can read rather than a silence" },
          "undeliveredSendsScope": { "type": "string", "description": "Always 'SinceProcessStart' — the count lives in a process-wide cell, and a restart correctly zeroes it (W-N10-02)" },
          "shards": { "type": "object", "nullable": true, "description": "**Every member shard's health, keyed by shard id** (W-N10-04), and total over the node's members. The flat fields above are the *primary's* reading; a non-primary shard whose autopropose timer has halted is visible here and nowhere else. Null when the caller is not a node with members", "additionalProperties": { "$ref": "#/components/schemas/ShardHealthDto" } },
          "resources": { "$ref": "#/components/schemas/ResourceStatus" }
        }
      },
      "ResourceStatus": {
        "type": "object",
        "description": "The sampled host-resource snapshot (A1), on the operator surface only. A reading with a source and an age, not a certificate: state=normal means the node did not see pressure at the sample, not that it is healthy",
        "properties": {
          "state": { "type": "string", "description": "The raw read: normal | unavailable" },
          "fallbackState": { "type": "string", "description": "The pressure state machine: normal | throttled | critical | unavailable. Only normal accepts new deploy/explore work; critical also stops proposing and attesting" },
          "fallbackReason": { "type": "string", "description": "Why the fallback state is what it is" },
          "fallbackSinceMs": { "type": "integer", "format": "int64", "description": "When the fallback state last changed (ms since the Unix epoch)" },
          "memAvailableMb": { "type": "integer", "format": "int64" },
          "memTotalMb": { "type": "integer", "format": "int64" },
          "cgroupVersion": { "type": "string", "description": "v1 | v2 | unavailable" },
          "cgroupLimitMb": { "type": "integer", "format": "int64", "nullable": true },
          "cgroupCurrentMb": { "type": "integer", "format": "int64", "nullable": true },
          "cgroupOomKills": { "type": "integer", "format": "int64" },
          "cpuQuota": { "type": "array", "nullable": true, "description": "(quota, period) from cpu.max/cpu.cfs_quota_us", "items": { "type": "integer", "format": "int64" } },
          "dataDirFreeMb": { "type": "integer", "format": "int64", "nullable": true },
          "rnodeProcesses": { "type": "integer", "format": "int64", "description": "Best-effort count of rnode processes in this PID namespace, including this one; advisory only" },
          "sampledAtMs": { "type": "integer", "format": "int64" },
          "sampleAgeMs": { "type": "integer", "format": "int64" },
          "source": { "type": "string", "description": "Which readers contributed, e.g. meminfo+cgroup+disk" }
        }
      },
      "ShardHealthDto": {
        "type": "object",
        "description": "One member shard's health, the value of an entry in OperatorStatus.shards. The map's key is the shard id, which is the scope — there is deliberately no scope field here, because a second place to say what the key says is a second place for the two to disagree (W-N10-04)",
        "properties": {
          "proposer": { "$ref": "#/components/schemas/ProposerHealthDto" },
          "finality": { "$ref": "#/components/schemas/FinalityHealthDto" }
        }
      },
      "ProposerHealthDto": {
        "type": "object",
        "description": "One shard's proposer health, as the per-shard map carries it (W-N10-04). The same facts as the flat fields, attributable to a named member",
        "properties": {
          "consecutiveSelfValidationFailures": { "type": "integer", "format": "int64" },
          "autoproposeTimerHalted": { "type": "boolean" },
          "autoproposeTimerHaltEpisodes": { "type": "integer", "format": "int64" },
          "autoproposeHalt": { "$ref": "#/components/schemas/HaltStatusDto" },
          "autoproposeTapSuppressed": { "type": "integer", "format": "int64" },
          "staleSnapshotSelfEquivocations": { "type": "integer", "format": "int64" }
        }
      },
      "FinalityHealthDto": {
        "type": "object",
        "description": "One shard's finality-and-merge health, as the per-shard map carries it (W-N10-03/04)",
        "properties": {
          "stallReason": { "type": "string", "nullable": true },
          "stallSustained": { "type": "boolean" },
          "stallEpisodes": { "type": "integer", "format": "int64" },
          "nonQuietMergeReports": { "type": "integer", "format": "int64" },
          "unrestorableRecords": { "type": "integer", "format": "int64" },
          "unrestorableBlock": { "type": "string", "nullable": true },
          "countersDurable": { "type": "boolean", "description": "Whether the three counts are the shard's history (true) or this process's totals (false) — see OperatorStatus.finalityCountersDurable (W-N10-02)" }
        }
      },
      "HaltStatusDto": {
        "type": "object",
        "description": "The autopropose halt on record: when it happened and whether the record is trustworthy (W-N10-01, F-U10-02)",
        "properties": {
          "at": { "type": "integer", "format": "int64", "nullable": true },
          "durable": { "type": "boolean" }
        }
      },
      "DeployData": {
        "type": "object",
        "properties": {
          "term": { "type": "string" },
          "timestamp": { "type": "integer", "format": "int64" },
          "phloPrice": { "type": "integer", "format": "int64" },
          "phloLimit": { "type": "integer", "format": "int64" },
          "validAfterBlockNumber": { "type": "integer", "format": "int64" },
          "shardId": { "type": "string" }
        }
      },
      "DeployRequest": {
        "type": "object",
        "properties": {
          "data": { "$ref": "#/components/schemas/DeployData" },
          "deployer": { "type": "string" },
          "signature": { "type": "string" },
          "sigAlgorithm": { "type": "string" }
        }
      },
      "BondInfo": {
        "type": "object",
        "properties": {
          "validator": { "type": "string" },
          "stake": { "type": "integer", "format": "int64" }
        }
      },
      "LightBlockInfo": {
        "type": "object",
        "properties": {
          "version": { "type": "integer", "format": "int32" },
          "shardId": { "type": "string" },
          "blockHash": { "type": "string" },
          "blockNumber": { "type": "integer", "format": "int64" },
          "sender": { "type": "string" },
          "seqNum": { "type": "integer", "format": "int64" },
          "preStateHash": { "type": "string" },
          "postStateHash": { "type": "string" },
          "justifications": { "type": "array", "items": { "type": "string" } },
          "bonds": { "type": "array", "items": { "$ref": "#/components/schemas/BondInfo" } },
          "sigAlgorithm": { "type": "string" },
          "sig": { "type": "string" },
          "blockSize": { "type": "string" },
          "deployCount": { "type": "integer", "format": "int32" },
          "rejectedDeploys": { "type": "array", "items": { "type": "string" } },
          "timestamp": { "type": "integer", "format": "int64", "description": "Informational block header timestamp (ms since the epoch); not a consensus input" }
        }
      },
      "RawBlock": {
        "type": "object",
        "properties": {
          "blockHash": { "type": "string", "description": "The block's content-addressed hash, hex" },
          "encoded": { "type": "string", "description": "Base16 of the exact bytes the hash is taken over (the block with its hash and signature cleared); hashing it reproduces blockHash" },
          "sender": { "type": "string", "description": "The proposer's public key, hex" },
          "sigAlgorithm": { "type": "string" },
          "sig": { "type": "string", "description": "Base16 signature over the 32 bytes of blockHash, by sender" }
        }
      },
      "FinalityAttestation": {
        "type": "object",
        "properties": {
          "blockHash": { "type": "string", "description": "The block this validator has finalised, hex" },
          "nonce": { "type": "string", "description": "The caller's nonce, hex; it is signed, so the attestation is good only for the conversation that asked for it" },
          "sender": { "type": "string", "description": "The attesting validator's public key, hex" },
          "sigAlgorithm": { "type": "string" },
          "sig": { "type": "string", "description": "Base16 signature over blake2b256(\"rchain-finality-attestation-v1\" || nonce || blockHash)" }
        }
      },
      "DeployInfo": {
        "type": "object",
        "properties": {
          "deployer": { "type": "string" },
          "term": { "type": "string" },
          "timestamp": { "type": "integer", "format": "int64" },
          "sig": { "type": "string" },
          "sigAlgorithm": { "type": "string" },
          "phloPrice": { "type": "integer", "format": "int64" },
          "phloLimit": { "type": "integer", "format": "int64" },
          "validAfterBlockNumber": { "type": "integer", "format": "int64" },
          "cost": { "type": "integer", "format": "int64" },
          "errored": { "type": "boolean" },
          "systemDeployError": { "type": "string" }
        }
      },
      "BlockInfo": {
        "type": "object",
        "properties": {
          "blockInfo": { "$ref": "#/components/schemas/LightBlockInfo" },
          "deploys": { "type": "array", "items": { "$ref": "#/components/schemas/DeployInfo" } }
        }
      },
      "ExploreDeployRequest": {
        "type": "object",
        "properties": {
          "term": { "type": "string" },
          "blockHash": { "type": "string" },
          "usePreStateHash": { "type": "boolean" }
        }
      },
      "DataAtNameByBlockHashRequest": {
        "type": "object",
        "properties": {
          "name": { "type": "object", "description": "A rholang expression" },
          "blockHash": { "type": "string" },
          "usePreStateHash": { "type": "boolean" }
        }
      },
      "RhoDataResponse": {
        "type": "object",
        "properties": {
          "expr": { "type": "array", "items": { "$ref": "#/components/schemas/RhoExpr" } },
          "block": { "$ref": "#/components/schemas/LightBlockInfo" }
        }
      },
      "ExploratoryDeployResponse": {
        "type": "object",
        "properties": {
          "expr": { "type": "array", "items": { "$ref": "#/components/schemas/RhoExpr" } },
          "block": { "$ref": "#/components/schemas/LightBlockInfo" },
          "replySource": { "type": "string", "enum": ["firstPrivateName", "out", "none"], "description": "Which channel the reply was read from: the term's first `new`-bound name, `@\"out\"`, or neither (the term produced nothing on either). A client can tell an empty `expr` from a reply it cannot see." }
        }
      },
      "RhoExpr": {
        "description": "A rholang value, in the reference document's shape: every arm wraps its payload in a field named `data` (legacy/docs/rnode-api/rnode-openapi.json).",
        "oneOf": [
          { "type": "object", "properties": { "ExprPar": { "type": "object", "properties": { "data": { "type": "array", "items": { "$ref": "#/components/schemas/RhoExpr" } } } } } },
          { "type": "object", "properties": { "ExprTuple": { "type": "object", "properties": { "data": { "type": "array", "items": { "$ref": "#/components/schemas/RhoExpr" } } } } } },
          { "type": "object", "properties": { "ExprList": { "type": "object", "properties": { "data": { "type": "array", "items": { "$ref": "#/components/schemas/RhoExpr" } } } } } },
          { "type": "object", "properties": { "ExprSet": { "type": "object", "properties": { "data": { "type": "array", "items": { "$ref": "#/components/schemas/RhoExpr" } } } } } },
          { "type": "object", "properties": { "ExprMap": { "type": "object", "properties": { "data": { "type": "object", "additionalProperties": { "$ref": "#/components/schemas/RhoExpr" } } } } } },
          { "type": "object", "properties": { "ExprBool": { "type": "object", "properties": { "data": { "type": "boolean" } } } } },
          { "type": "object", "properties": { "ExprInt": { "type": "object", "properties": { "data": { "type": "integer" } } } } },
          { "type": "object", "properties": { "ExprString": { "type": "object", "properties": { "data": { "type": "string" } } } } },
          { "type": "object", "properties": { "ExprUri": { "type": "object", "properties": { "data": { "type": "string" } } } } },
          { "type": "object", "properties": { "ExprBytes": { "type": "object", "properties": { "data": { "type": "string" } } } } },
          { "type": "object", "properties": { "ExprUnforg": { "type": "object", "properties": { "data": { "$ref": "#/components/schemas/RhoUnforg" } } } } }
        ]
      },
      "RhoUnforg": {
        "oneOf": [
          { "type": "object", "properties": { "UnforgPrivate": { "type": "object", "properties": { "data": { "type": "string" } } } } },
          { "type": "object", "properties": { "UnforgDeploy": { "type": "object", "properties": { "data": { "type": "string" } } } } },
          { "type": "object", "properties": { "UnforgDeployer": { "type": "object", "properties": { "data": { "type": "string" } } } } }
        ]
      },
      "DeployExecStatus": {
        "oneOf": [
          { "type": "object", "properties": { "deployResult": { "type": "array", "items": { "type": "object" } }, "block": { "$ref": "#/components/schemas/LightBlockInfo" } } },
          { "type": "object", "properties": { "deployError": { "type": "string" }, "block": { "$ref": "#/components/schemas/LightBlockInfo" } } },
          { "type": "object", "properties": { "status": { "type": "string" } } }
        ]
      },
      "HttpAdmissionBudget": {
        "type": "object",
        "properties": {
          "configured": { "type": "boolean" },
          "limit": { "type": "integer", "minimum": 1 },
          "inFlight": { "type": "integer", "minimum": 0 },
          "available": { "type": "integer", "minimum": 0 }
        },
        "required": ["configured"]
      },
      "NodeCapabilities": {
        "type": "object",
        "properties": {
          "autopropose": { "type": "boolean" },
          "proposeOnDeploy": { "type": "boolean" },
          "manualPropose": { "type": "boolean" },
          "adminHttp": { "type": "boolean" },
          "devMode": { "type": "boolean" },
          "faucet": { "type": "boolean" },
        "faucetRemaining": { "type": "integer", "format": "int64" }
        }
      },
      "PooledDeploy": {
        "type": "object",
        "properties": {
          "deployId": { "type": "string" },
          "timestamp": { "type": "integer", "format": "int64" },
          "deployer": { "type": "string" },
          "term": { "type": "string" },
          "phloPrice": { "type": "integer", "format": "int64" },
          "phloLimit": { "type": "integer", "format": "int64" },
          "validAfterBlockNumber": { "type": "integer", "format": "int64" }
        }
      },
      "PooledDeploys": {
        "type": "object",
        "properties": {
          "deploys": { "type": "array", "items": { "$ref": "#/components/schemas/PooledDeploy" } }
        }
      },
      "FaucetResponse": {
        "type": "object",
        "properties": {
          "deployId": { "type": "string" },
          "amount": { "type": "integer", "format": "int64" },
          "to": { "type": "string" },
          "status": { "type": "string", "enum": ["pending", "resubmitted", "failed"] },
          "deployError": { "type": ["string", "null"] }
        }
      }
    }
  }
}"##;

async fn api_v1_openapi() -> Response {
    let doc: serde_json::Value =
        serde_json::from_str(OPENAPI_JSON).unwrap_or_else(|_| serde_json::Value::Null);
    (StatusCode::OK, Json(doc)).into_response()
}

// --- Reporting routes (port of `ReportingRoutes.service`) ---

/// `GET /reporting/trace` query params (`blockHash` + optional `forceReplay`).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReportingQuery {
    block_hash: String,
    force_replay: Option<bool>,
}

async fn reporting_trace(
    State(state): State<HttpState>,
    Query(query): Query<ReportingQuery>,
) -> Response {
    // Reporting is disabled by default (`api-server.enable-reporting = false`); the route answers
    // 404 unless explicitly enabled (M6 — the flag was read but never enforced).
    if !state.enable_reporting {
        return (StatusCode::NOT_FOUND, ()).into_response();
    }
    // Validate-on-ingress: a malformed block hash (non-hex / wrong length) must be a 400, not a
    // panic in `BlockHash::from_hex`.
    let hash = match BlockHash::try_from_hex(&query.block_hash) {
        Ok(h) => h,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json("invalid block hash".to_string()),
            )
                .into_response()
        }
    };
    let result = state
        .block_report_api
        .block_report(&hash, query.force_replay.unwrap_or(false))
        .await;
    (StatusCode::OK, Json(transform_result(result))).into_response()
}

// --- Admin Web API routes (port of `AdminWebApiRoutes.service`) ---

async fn admin_propose(State(state): State<AdminState>) -> Response {
    json_result(state.admin_web_api.propose().await)
}

/// `POST /api/v1/autopropose/resume` — clear the durable autopropose halt and wake the parked timer
/// (W-N10-01, F-U10-02).
///
/// The one way out of a halt that survives a restart. Served on the admin listener, behind
/// `admin_origin_guard` and the resume rate limiter, for the reason every actuator here is: it changes
/// what the node does on the caller's word.
async fn admin_resume_autopropose(State(state): State<AdminState>) -> Response {
    json_result(state.admin_web_api.resume_autopropose().await)
}

/// `POST /api/v1/resources/resume` — clear a latched resource fallback state (unit C, B37).
///
/// It takes a **fresh sample** and clears only when that sample is clear; a pressured node answers
/// `409` with the measured state, so the actuator cannot override live pressure. It never touches the
/// autopropose halt: that is an integrity decision, not a resource one.
async fn admin_resume_resources(State(state): State<AdminState>) -> Response {
    match state.admin_web_api.resume_resource_fallback().await {
        Ok(word) if word == "normal" => (
            StatusCode::OK,
            Json(json!({ "state": word, "message": "resource fallback cleared" })),
        )
            .into_response(),
        Ok(word) => (
            StatusCode::CONFLICT,
            Json(json!({
                "state": word,
                "message": "the node is still under resource pressure; the resume did not override it"
            })),
        )
            .into_response(),
        Err(BlockApiException(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!(e))).into_response(),
    }
}

/// **The ingress bound, keyed on the peer and applied before the body is decoded.**
///
/// Two defects in one function. Its subject was the *node* — one process-wide bucket per route — so
/// one caller's burst refused every other caller's request and each route starved the others'
/// budget: the bound held, and the wrong counterparty paid (W-N2-01, W-N2-02, W-N2-08). And it ran
/// *inside* the handler, after axum's `Json` extractor had already decoded the body, so a caller
/// could make the node decode as many payloads as it liked and only then be told it was over the
/// limit (W-N2-06, W-N2-09).
///
/// **What this bounds is the decode, not the deploys.** The deployer's own bound is downstream at
/// `BlockDagKeyValueStorage::add_deploy`, keyed on the deployer public key — the only identity the
/// node can hold a submitter to, and one that does not exist until the body has been read. This layer
/// is what stops one peer spending another's decode budget in the window before that, and it is why
/// keying it on the address is not a substitute for the downstream bound but a different bound over a
/// different resource.
///
/// As a `route_layer` it runs before the handler's extractors, which is the whole point of it being a
/// layer rather than three lines at the top of each handler.
async fn peer_rate_limit(
    State((limiter, message)): State<(Arc<KeyedRateLimiter>, &'static str)>,
    request: Request,
    next: Next,
) -> Response {
    let key: Vec<u8> = match request.extensions().get::<ConnectInfo<SocketAddr>>() {
        Some(ConnectInfo(addr)) => addr.ip().to_string().into_bytes(),
        // No connection information: this router was built without
        // `into_make_service_with_connect_info`, which is what the tests do when they call it
        // directly. One shared bucket — see [`UNATTRIBUTED_PEER`].
        None => UNATTRIBUTED_PEER.to_vec(),
    };
    if !limiter.allow(&key) {
        return (StatusCode::TOO_MANY_REQUESTS, Json(message.to_string())).into_response();
    }
    next.run(request).await
}

/// The one `503` shape both gates answer with: a reason and a `Retry-After`, before any body work.
fn admission_refused(message: &str) -> Response {
    let mut response = (StatusCode::SERVICE_UNAVAILABLE, Json(message.to_string())).into_response();
    response.headers_mut().insert(
        axum::http::header::RETRY_AFTER,
        axum::http::HeaderValue::from_static("1"),
    );
    response
}

/// **The two gates on the deploy/explore path** (B34 memory, B35 concurrency).
///
/// The memory gate is always present and is checked first: it reads the watchdog's newest sample and
/// refuses when headroom is below the floor, the sample is stale, or the facts could not be read. A
/// permit is a *concurrency* bound and cannot substitute for it. The permit is optional (the operator's
/// configured in-flight budget) and lives through the whole handler future, including exploratory
/// Rholang execution, so a busy route fails quickly without decoding a body or queuing unbounded work.
/// Other routes (status, finality, admin) borrow neither gate.
async fn in_flight_admission(
    State((budget, pressure)): State<(Option<InFlightAdmission>, PressureGate)>,
    request: Request,
    next: Next,
) -> Response {
    if let Some((reason, detail)) = pressure.evaluate_now().refusal() {
        return admission_refused(&format!("{reason}: {detail}"));
    }
    let _permit: Option<OwnedSemaphorePermit> = match budget {
        Some(budget) => match budget.semaphore.try_acquire_owned() {
            Ok(permit) => Some(permit),
            Err(_) => return admission_refused("HTTP in-flight capacity exhausted"),
        },
        None => None,
    };
    next.run(request).await
}

fn admission_field(budget: &Option<InFlightAdmission>) -> serde_json::Value {
    match budget {
        Some(budget) => {
            let available = budget.semaphore.available_permits();
            json!({
                "configured": true,
                "limit": budget.limit,
                "inFlight": budget.limit - available,
                "available": available
            })
        }
        None => json!({"configured": false}),
    }
}

/// Describes only the HTTP concurrency bound, not memory pressure, validator
/// health, transport responsiveness, finality progress or readiness.
async fn api_admission_status(State(state): State<HttpState>) -> Response {
    Json(json!({
        "deploy": admission_field(&state.deploy_admission),
        "explore": admission_field(&state.explore_admission),
    }))
    .into_response()
}

/// Build the public HTTP routes (port of `acquireHttpServer`'s route map: `/version`, `/metrics`,
/// `/status`, the `/api` JSON routes, `/reporting` + `/api/trace`, the `/api/v1` routes, and the
/// `/api/v1/openapi.json` OpenAPI document).
pub fn router(state: HttpState) -> Router {
    let faucet_enabled = state.faucet_enabled;

    // **Each ingress bound is a layer over exactly its own routes.** Applied to the whole router, one
    // limiter would serve every route — which is the AUDIT R36 defect in a new spelling, and the
    // reason these are sub-routers rather than one `.layer()` at the end. The layer runs *before* the
    // handler's extractors, so a request is refused before its body is decoded.
    let deploy_routes = Router::new()
        .route("/api/deploy", post(api_deploy))
        .route("/api/v1/deploy", post(api_deploy))
        .route_layer(middleware::from_fn_with_state(
            (state.deploy_admission.clone(), state.pressure.clone()),
            in_flight_admission,
        ))
        .route_layer(middleware::from_fn_with_state(
            (
                state.deploy_rate_limiter.clone(),
                "deploy rate limit exceeded",
            ),
            peer_rate_limit,
        ));
    let explore_routes = Router::new()
        .route("/api/explore-deploy", post(api_explore_deploy))
        .route(
            "/api/explore-deploy-by-block-hash",
            post(api_explore_deploy_by_block_hash),
        )
        .route("/api/v1/explore-deploy", post(api_explore_deploy))
        .route(
            "/api/v1/explore-deploy-by-block-hash",
            post(api_explore_deploy_by_block_hash),
        )
        .route_layer(middleware::from_fn_with_state(
            (state.explore_admission.clone(), state.pressure.clone()),
            in_flight_admission,
        ))
        .route_layer(middleware::from_fn_with_state(
            (
                state.explore_rate_limiter.clone(),
                "explore-deploy rate limit exceeded",
            ),
            peer_rate_limit,
        ));
    // The reporting routes share the deploy limiter, as they did when it was a handler check: they
    // are a different resource from a deploy, but the same bound covered them and this change is not
    // the place to re-price them.
    let reporting_routes = Router::new()
        .route("/reporting/trace", get(reporting_trace))
        .route("/api/trace", get(reporting_trace))
        .route("/api/transactions/{hash}", get(api_get_transaction))
        .route_layer(middleware::from_fn_with_state(
            (
                state.deploy_rate_limiter.clone(),
                "deploy rate limit exceeded",
            ),
            peer_rate_limit,
        ));

    let mut routes = Router::new()
        .route("/version", get(version))
        .route("/metrics", get(metrics))
        .route("/status", get(status))
        .route("/api/status", get(api_status))
        .route("/api/v1/admission", get(api_admission_status))
        .route("/api/capabilities", get(api_capabilities))
        .route("/api/shards", get(api_shards))
        .route("/api/deploys", get(api_deploys))
        .route("/api/data-at-name", post(api_data_at_name))
        .route(
            "/api/data-at-name-by-block-hash",
            post(api_data_at_name_by_block_hash),
        )
        .route("/api/last-finalized-block", get(api_last_finalized_block))
        .route("/api/block/{hash}", get(api_get_block))
        .route("/api/block/{hash}/raw", get(api_get_raw_block))
        .route("/api/finality-attestation", get(api_finality_attestation))
        .route("/api/blocks", get(api_get_blocks))
        .route("/api/blocks/{start}/{end}", get(api_get_blocks_by_heights))
        .route("/api/blocks/{depth}", get(api_get_blocks_by_depth))
        .route("/api/deploy/{deploy_id}", get(api_find_deploy))
        .route("/api/is-finalized/{hash}", get(api_is_finalized))
        .route("/api/v1/status", get(api_status))
        .route("/api/v1/capabilities", get(api_capabilities))
        .route("/api/v1/shards", get(api_shards))
        .route("/api/v1/pos", get(api_pos_status))
        .route("/api/v1/deployer/{key}", get(api_find_deployer))
        .route("/api/v1/deploys", get(api_deploys))
        .route(
            "/api/v1/deploy-status/{deploy_signature}",
            get(api_v1_deploy_status),
        )
        .route(
            "/api/v1/data-at-name-by-block-hash",
            post(api_data_at_name_by_block_hash),
        )
        .route("/api/v1/blocks", get(api_get_blocks))
        .route("/api/v1/block/{hash}", get(api_get_block))
        .route("/api/v1/block/{hash}/raw", get(api_get_raw_block))
        .route(
            "/api/v1/finality-attestation",
            get(api_finality_attestation),
        )
        .route("/api/v1/openapi.json", get(api_v1_openapi))
        .merge(deploy_routes)
        .merge(explore_routes)
        .merge(reporting_routes);

    // **The faucet is mounted only when it is switched on** (AUDIT R34).
    //
    // It used to be mounted unconditionally with the gate inside the handler, and that made the
    // published contract wrong: `GET /api/v1/openapi.json` documents this route's refusal as
    // **404** ("The faucet is disabled on this node"), while `json_result` maps every
    // `BlockApiException` to 400 — so a client reading the schema and a client reading the wire
    // disagreed about the same event. Mounting it only when enabled makes the documented 404 the
    // actual answer, and takes a funded route off a production node's surface rather than leaving it
    // there to say no.
    //
    // The condition is the handler's own — dev mode plus a deployer key — carried here by
    // `acquire_http_server` rather than re-derived, so the two cannot disagree about when the faucet
    // is on.
    if faucet_enabled {
        routes = routes.merge(
            Router::new()
                .route("/api/faucet", post(api_faucet))
                .route("/api/v1/faucet", post(api_faucet))
                .route_layer(middleware::from_fn_with_state(
                    (
                        state.faucet_rate_limiter.clone(),
                        "faucet rate limit exceeded",
                    ),
                    peer_rate_limit,
                )),
        );
    }

    routes.layer(CorsLayer::permissive()).with_state(state)
}

/// Refuse a **cross-origin** request to the admin surface — the residual AUDIT C133 named and did not
/// close, closed here.
///
/// **Why this is a layer and not a check in each handler.** Every route on this router makes the node
/// act on the caller's word with its own authority: `/api/propose` produces a block, the `/api/txn`
/// family spends out of the validator's account, and `/api/v1/ocapn/dial` makes the node open a
/// connection to a peer the caller names. A guard written into the handlers would be a bound enforced
/// by the callers that happen to exist — the shape this register keeps recording — and the next admin
/// route added would not have it. A layer applies to every route on the router, including later ones.
///
/// **What it allows, and why each exception is principled rather than convenient:**
///
/// * **No `Origin` header at all.** CSRF requires a browser, and every browser sends `Origin` on a
///   `POST`; a client that sends none — `curl`, the CLI, this repository's own devnet scripts — is not
///   making a cross-site request, and refusing those would break the tooling while buying nothing.
/// * **Same-origin**, i.e. an `Origin` whose authority equals the request's `Host`. Compared
///   scheme-insensitively on purpose: the admin port is plain HTTP on loopback and `https` behind an
///   operator's TLS proxy, and what this decides is *which site* is asking, not which scheme it used.
/// * **Anything, when `api-server.enable-devnet-cors` is set.** That flag *is* the operator asking for
///   cross-origin access, and the browser-wallet path that the C112 bind exists to allow depends on
///   it. A guard that ignored the flag would break the feature it is meant to protect.
///
/// `Origin: null` — a sandboxed iframe, or a redirect from a `data:` URL — matches no host and is
/// refused, which is what comparing rather than pattern-matching buys.
async fn admin_origin_guard(
    State(state): State<AdminState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if !state.enable_devnet_cors {
        if let Some(origin) = request
            .headers()
            .get(axum::http::header::ORIGIN)
            .and_then(|v| v.to_str().ok())
        {
            let host = request
                .headers()
                .get(axum::http::header::HOST)
                .and_then(|v| v.to_str().ok());
            // `scheme://authority` → `authority`. An origin that does not parse keeps its whole
            // value, which then matches no host and is refused.
            let authority = origin.split_once("://").map_or(origin, |(_, rest)| rest);
            if host != Some(authority) {
                return (
                    StatusCode::FORBIDDEN,
                    "cross-origin request refused: this listener acts with the node's own key, so \
                     only same-origin or origin-less requests are accepted. A browser on another \
                     origin needs `api-server.enable-devnet-cors`.\n",
                )
                    .into_response();
            }
        }
    }
    next.run(request).await
}

/// Build the admin HTTP routes (port of `acquireAdminHttpServer`'s `/api` + `/api/v1` admin routes).
pub fn admin_router(state: AdminState) -> Router {
    // Restrictive CORS (no allowed origins) by default; devnet / browser-wallet access opts into
    // permissive CORS via `api-server.enable-devnet-cors`.
    //
    // **What that does and does not buy, stated because this comment claimed more (AUDIT C133).** It
    // used to say the restrictive layer stopped "a browser on another origin … trigger[ing] block
    // production". `CorsLayer` never rejects a request: only `OPTIONS` takes the preflight branch and
    // every other method is forwarded to the inner service with response headers added
    // (`tower-http/src/cors/mod.rs`). `admin_propose` takes no extractor and checks no origin, so a
    // bodyless cross-origin `POST` is a CORS **simple request** — no preflight is sent, the handler
    // runs, and CORS only stops the page *reading* the reply. Loopback does not help either: the
    // request comes from the operator's own browser (CSRF), and DNS rebinding resolves an attacker's
    // name to `127.0.0.1`.
    //
    // **What protects the route is two things, and this comment first named only the first.** The
    // bind and the opt-in above (C112) are the boundary against a *remote* attacker. They are no
    // boundary against a page the operator visits, which is what `admin_origin_guard` below is for: a
    // cross-origin request to this router is refused before any handler runs, so the residual this
    // comment used to name is closed rather than documented. The CORS layer remains what the
    // paragraph above says it is — a header policy, not a boundary.
    let cors = if state.enable_devnet_cors {
        CorsLayer::permissive()
    } else {
        CorsLayer::new()
    };
    // The cross-shard transaction surface (AUDIT C121). It used to be mounted on the **public**
    // router, where the coordinator's fund-moving handler had no caller identity, no rate limit and
    // no bound on the leg vector — on a server that binds `0.0.0.0` by default. It belongs on the
    // privileged listener for the same reason `/api/propose` does: both act with the node's own key,
    // and this one spends from its account. `gateway_or_not_found` still answers 404 on a node that
    // is not a gateway or has the feature off, so the 404 a client sees has not changed meaning.
    //
    // Its bound is a `route_layer` for the same two reasons as the public routes': the subject is the
    // peer rather than the node, and it runs before the body is decoded (W-N2-01, W-N2-06). This
    // listener is loopback unless the operator publishes it, so the peer is usually local — but
    // `enable-devnet-admin-public` is exactly the case where it is not, and a bound whose subject is
    // the node is the wrong bound in that case too.
    let txn_routes = Router::new()
        .route("/api/txn", get(api_txn_list).post(api_txn_run))
        .route("/api/txn/{txn_id}", get(api_txn_status))
        .route("/api/v1/txn", get(api_txn_list).post(api_txn_run))
        .route("/api/v1/txn/{txn_id}", get(api_txn_status))
        .route_layer(middleware::from_fn_with_state(
            (state.txn_rate_limiter.clone(), "txn rate limit exceeded"),
            peer_rate_limit,
        ));
    // The node-started OCapN dial (issue #249). Served on the privileged listener for the same
    // reason the txn routes are: it makes the node act on the caller's word — dial a peer the
    // caller names and fetch what it points at. `dialer_or_not_found` answers 404 when
    // `enable-ocapn-dial` is off, so a node without the feature has an unchanged surface.
    let dial_routes = Router::new()
        .route("/api/v1/ocapn/dial", post(admin_ocapn_dial))
        .route_layer(middleware::from_fn_with_state(
            (
                state.ocapn_dial_rate_limiter.clone(),
                "ocapn dial rate limit exceeded",
            ),
            peer_rate_limit,
        ));

    // The autopropose-resume actuator (W-N10-01, F-U10-02). A durable halt is *obeyed*, so the
    // operator's way out cannot be "restart and hope" — this route clears the record on every member
    // shard and wakes the parked timer. It is an actuator on the privileged listener for the same
    // reason `/api/propose` is: it changes what the node does, on the caller's word. Bounded like its
    // siblings rather than left open, because a page the operator visits is exactly the caller that
    // should not be able to hammer it.
    let resume_routes = Router::new()
        .route("/api/v1/autopropose/resume", post(admin_resume_autopropose))
        .route_layer(middleware::from_fn_with_state(
            (
                state.resume_rate_limiter.clone(),
                "autopropose resume rate limit exceeded",
            ),
            peer_rate_limit,
        ))
        // The resource resume has its own bucket: an operator clearing the two actuators must not
        // spend one's allowance on the other.
        .merge(
            Router::new()
                .route("/api/v1/resources/resume", post(admin_resume_resources))
                .route_layer(middleware::from_fn_with_state(
                    (
                        Arc::new(KeyedRateLimiter::new(
                            RESUME_RESOURCE_RATE_LIMIT_PER_SEC,
                            MAX_TRACKED_PEERS,
                        )),
                        "resource resume rate limit exceeded",
                    ),
                    peer_rate_limit,
                )),
        );

    Router::new()
        .route("/api/propose", post(admin_propose))
        .route("/api/v1/propose", post(admin_propose))
        .route("/api/v1/status/operator", get(api_operator_status))
        .route("/api/v1/pos/delegations", get(api_pos_delegations))
        .route("/api/v1/openapi.json", get(api_v1_openapi))
        .merge(txn_routes)
        .merge(dial_routes)
        .merge(resume_routes)
        .layer(cors)
        // Outermost, so a cross-origin request is refused before any handler on this router runs —
        // including the ones that spend the node's REV. See `admin_origin_guard` for why it is a layer
        // rather than a check in each handler.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            admin_origin_guard,
        ))
        .with_state(state)
}

/// Bind and serve the public HTTP routes (port of `web/acquireHttpServer`), with a CORS layer and a
/// per-request timeout (`api-server.max-connection-idle`).
///
/// **No gateway parameter, and that is deliberate** (AUDIT C121): the cross-shard coordinator is not
/// reachable from this listener at all, so the fund-moving routes cannot be mounted here by a later
/// change without moving the capability back into `HttpState`.
#[allow(clippy::too_many_arguments)]
pub async fn acquire_http_server(
    host: &str,
    port: Port,
    reporter: Arc<NewPrometheusReporter>,
    metrics: Arc<MetricsRegistry>,
    web_api: Arc<dyn WebApi>,
    block_report_api: Arc<BlockReportApi>,
    shards: Arc<ShardRegistry>,
    status_provider: Option<StatusProvider>,
    pos: Arc<dyn PosReadApi>,
    resources: Arc<ResourceGuard>,
    max_connection_idle: Duration,
    enable_reporting: bool,
    // Whether to mount the faucet routes (AUDIT R34): dev mode **and** a deployer key, resolved by
    // the caller because that is where both are known.
    faucet_enabled: bool,
    stop: watch::Receiver<bool>,
) -> Result<(), String> {
    let port = u16::from(port); // single discharge at the bind boundary
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .map_err(|e| format!("invalid bind address {host}:{port}: {e}"))?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| e.to_string())?;
    // Read configuration once at startup. An invalid explicit limit stops the
    // listener instead of silently disabling admission control.
    let deploy_admission = inflight_from_env("RNODE_HTTP_MAX_INFLIGHT_DEPLOY")?;
    let explore_admission = inflight_from_env("RNODE_HTTP_MAX_INFLIGHT_EXPLORE")?;
    // The memory gate is not opt-in: a node that cannot read its own headroom is exactly the node
    // this programme exists for. The policy is a documented floor (resources/admission.rs), not a
    // guessed concurrency number.
    let pressure = PressureGate::new(resources, crate::resources::AdmissionPolicy::default());
    let app = router(HttpState {
        reporter,
        metrics,
        web_api,
        block_report_api,
        shards,
        status_provider,
        pos,
        enable_reporting,
        deploy_rate_limiter: Arc::new(KeyedRateLimiter::new(
            DEFAULT_API_RATE_LIMIT_PER_SEC,
            MAX_TRACKED_PEERS,
        )),
        explore_rate_limiter: Arc::new(KeyedRateLimiter::new(
            DEFAULT_API_RATE_LIMIT_PER_SEC,
            MAX_TRACKED_PEERS,
        )),
        deploy_admission,
        explore_admission,
        pressure,
        faucet_rate_limiter: Arc::new(KeyedRateLimiter::new(
            FAUCET_RATE_LIMIT_PER_SEC,
            MAX_TRACKED_PEERS,
        )),
        faucet_enabled,
    })
    .layer(TimeoutLayer::with_status_code(
        StatusCode::REQUEST_TIMEOUT,
        max_connection_idle,
    ));
    // Graceful shutdown (AUDIT C144): on the operator's word this stops accepting and lets the
    // connections already in flight finish, so a stop is not a truncation of somebody's request.
    //
    // **Served with connection information**, which is what the ingress bound is keyed on: without
    // `into_make_service_with_connect_info` the peer address never reaches the request and
    // `peer_rate_limit` would fall back to one shared bucket — the bound this change removed.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(stop_requested(stop))
    .await
    .map_err(|e| e.to_string())
}

/// Bind and serve the admin HTTP routes (port of `web/acquireAdminHttpServer`).
///
/// This listener carries the surfaces that make the node act on a caller's word with its own
/// authority: `POST /api/propose`, since AUDIT C121 the cross-shard transaction routes, and since
/// issue #249 the node-started OCapN dial. The caller chooses the bind host (`admin_bind_host`),
/// which is loopback unless the operator opts in — that choice is what makes those routes' absence
/// from the public server an authorization boundary rather than a rearrangement.
pub async fn acquire_admin_http_server(
    host: &str,
    port: Port,
    admin_web_api: Arc<dyn AdminWebApi>,
    enable_devnet_cors: bool,
    gateway: Option<Arc<GatewayTxn>>,
    enable_txn_api: bool,
    ocapn_dial: crate::api::ocapn::OcapnDialSlot,
    enable_ocapn_dial: bool,
    // **The PoS read: this listener is where a delegator's positions are served** (W-N3-03).
    pos: Arc<dyn PosReadApi>,
    max_connection_idle: Duration,
    stop: watch::Receiver<bool>,
) -> Result<(), String> {
    let port = u16::from(port); // single discharge at the bind boundary
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .map_err(|e| format!("invalid bind address {host}:{port}: {e}"))?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| e.to_string())?;
    let app = admin_router(AdminState {
        admin_web_api,
        enable_devnet_cors,
        gateway,
        enable_txn_api,
        txn_rate_limiter: Arc::new(KeyedRateLimiter::new(
            TXN_RATE_LIMIT_PER_SEC,
            MAX_TRACKED_PEERS,
        )),
        ocapn_dial,
        enable_ocapn_dial,
        ocapn_dial_rate_limiter: Arc::new(KeyedRateLimiter::new(
            OCAPN_DIAL_RATE_LIMIT_PER_SEC,
            MAX_TRACKED_PEERS,
        )),
        pos,
        resume_rate_limiter: Arc::new(KeyedRateLimiter::new(
            RESUME_AUTOPROPOSE_RATE_LIMIT_PER_SEC,
            MAX_TRACKED_PEERS,
        )),
    })
    .layer(TimeoutLayer::with_status_code(
        StatusCode::REQUEST_TIMEOUT,
        max_connection_idle,
    ));
    // The same stop path as the public server (AUDIT C144), and the same connection information for
    // the same reason: this listener's bounds are keyed on the peer too.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(stop_requested(stop))
    .await
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::dto::{
        DataAtNameResponse, DeployExecStatus, ExploratoryDeployResponse, FaucetResponse,
        HaltStatusDto, NodeCapabilities, OperatorStatus, PooledDeploy, PooledDeploys, PublicStatus,
        RhoDataResponse, VersionInfo,
    };
    use crate::diagnostics::scrape_data_builder::Configuration;
    use crate::resources::{AdmissionPolicy, ResourceSnapshot, ResourceState};
    use crate::web::transaction::TransactionResponse;
    use async_trait::async_trait;
    use axum::body::to_bytes;
    use rchain_block_storage::dag::codecs::{BlockHashCodec, BlockMessageCodec};
    use rchain_casper::api::block_api::{FinalityHealth, ShardHealthEntry};
    use rchain_casper::reporting::noop;
    use rchain_casper::runtime_manager::CapturedReply;
    use rchain_comm::peer_node::{NodeIdentifier, PeerNode};
    use rchain_comm::rp::rp_conf::ClearConnectionsConf;
    use rchain_models::casper::protocol::deploy_service::{BlockInfo, LightBlockInfo};
    use rchain_models::casper::protocol::report::BlockEventInfo;
    use rchain_shared::metrics::Metrics as _;
    use rchain_shared::store::InMemoryKeyValueStore;
    use rchain_shared::typed_store::{Codec, KeyValueTypedStoreCodec, SharedStore};
    use std::collections::BTreeMap;
    use std::marker::PhantomData;

    struct JsonCodec<T>(PhantomData<T>);

    struct NoopDiscovery;
    #[async_trait]
    impl NodeDiscovery for NoopDiscovery {
        async fn discover(&self) {}
        fn peers(&self) -> Vec<PeerNode> {
            Vec::new()
        }
    }

    impl<T: Serialize + serde::de::DeserializeOwned + Send + Sync> Codec<T> for JsonCodec<T> {
        fn encode(&self, value: &T) -> Vec<u8> {
            serde_json::to_vec(value).expect("json encode")
        }

        fn decode(&self, bytes: &[u8]) -> Result<T, String> {
            serde_json::from_slice(bytes).map_err(|e| e.to_string())
        }
    }

    fn test_block_report_api() -> Arc<BlockReportApi> {
        let store: SharedStore = Arc::new(tokio::sync::Mutex::new(Box::new(
            InMemoryKeyValueStore::default(),
        )));
        let block_store = Arc::new(KeyValueTypedStoreCodec::new(
            store.clone(),
            Arc::new(BlockHashCodec),
            Arc::new(BlockMessageCodec),
        ));
        let report_store = Arc::new(KeyValueTypedStoreCodec::new(
            store,
            Arc::new(BlockHashCodec),
            Arc::new(JsonCodec::<BlockEventInfo>(PhantomData)),
        ));
        Arc::new(BlockReportApi::new(
            block_store,
            Arc::new(noop()),
            report_store,
            None,
        ))
    }

    fn test_status() -> PublicStatus {
        PublicStatus {
            version: VersionInfo {
                api: "1.0".to_string(),
                node: "2.0".to_string(),
            },
            address: "addr".to_string(),
            network_id: "testnet".to_string(),
            shard_id: "root".to_string(),
            peers: 1,
            nodes: 2,
            min_phlo_price: 3,
            latest_block_number: 4,
            autopropose: true,
            propose_on_deploy: true,
            manual_propose: false,
            dev_mode: true,
            degraded: "normal".to_string(),
            accepts_deploys: true,
        }
    }

    /// The operator envelope, for `MockAdminWebApi`: the public core above plus the half that moved to
    /// the admin listener (W-N3-01). The values are the "nothing to report" arm of each field, which is
    /// what a healthy node answers.
    fn test_operator_status() -> OperatorStatus {
        OperatorStatus {
            core: test_status(),
            admin_http: true,
            consecutive_self_validation_failures: 0,
            autopropose_timer_halted: false,
            autopropose_timer_halt_episodes: 0,
            autopropose_halt: HaltStatusDto {
                at: None,
                durable: false,
            },
            autopropose_tap_suppressed: 0,
            stale_snapshot_self_equivocations: 0,
            finality_stall: None,
            finality_stalled: false,
            finality_stall_episodes: 0,
            finality_counters_durable: true,
            reset_marker: None,
            non_quiet_merge_reports: 0,
            sync: None,
            poison_recoveries: 0,
            poison_recoveries_scope: "SinceProcessStart".to_string(),
            unrestorable_records: 0,
            unrestorable_block: None,
            tasks_panicked: 0,
            tasks_panicked_scope: "SinceProcessStart".to_string(),
            tasks_exited: 0,
            tasks_exited_scope: "SinceProcessStart".to_string(),
            undelivered_sends: 0,
            undelivered_sends_scope: "SinceProcessStart".to_string(),
            shards: None,
            resources: Default::default(),
        }
    }

    struct MockWebApi {
        status: PublicStatus,
        pooled_deploys: PooledDeploys,
    }

    #[async_trait]
    impl WebApi for MockWebApi {
        async fn status(&self) -> Result<PublicStatus, BlockApiException> {
            Ok(self.status.clone())
        }

        async fn deploy(&self, _: &DeployRequest) -> Result<String, BlockApiException> {
            // Not `unimplemented!()`. It was, and that made the mock usable only for the paths that
            // refuse *before* reaching it — so a test could assert "you never got here" but never
            // "you got here". AUDIT R36's separator needs the second: an explore flood must leave the
            // deploy route still answering, which means the deploy route has to have an answer.
            Ok("mock-deploy-id".to_string())
        }

        async fn deploy_status(&self, _: &str) -> Result<DeployExecStatus, BlockApiException> {
            unimplemented!()
        }

        async fn pooled_deploys(&self) -> Result<PooledDeploys, BlockApiException> {
            Ok(self.pooled_deploys.clone())
        }

        async fn capabilities(&self) -> Result<NodeCapabilities, BlockApiException> {
            unimplemented!()
        }

        async fn faucet(&self, _: &str) -> Result<FaucetResponse, BlockApiException> {
            // Not `unimplemented!()`, for the same reason `deploy` is not (AUDIT R34/R36): a mock that
            // can only panic tests that a route was *not* reached, and the mount test needs both
            // arms — absent, and present-and-answering.
            Err(BlockApiException("stub faucet".to_string()))
        }

        async fn listen_for_data_at_name(
            &self,
            _: &DataAtNameRequest,
        ) -> Result<DataAtNameResponse, BlockApiException> {
            unimplemented!()
        }

        async fn get_data_at_par(
            &self,
            _: &DataAtNameByBlockHashRequest,
        ) -> Result<RhoDataResponse, BlockApiException> {
            unimplemented!()
        }

        async fn last_finalized_block(&self) -> Result<BlockInfo, BlockApiException> {
            unimplemented!()
        }

        async fn get_block(&self, _: &str) -> Result<BlockInfo, BlockApiException> {
            unimplemented!()
        }

        async fn get_blocks(&self, _: i32) -> Result<Vec<LightBlockInfo>, BlockApiException> {
            unimplemented!()
        }

        async fn find_deploy(&self, _: &str) -> Result<LightBlockInfo, BlockApiException> {
            unimplemented!()
        }

        async fn exploratory_deploy(
            &self,
            _: &str,
            _: Option<&str>,
            _: bool,
        ) -> Result<ExploratoryDeployResponse, BlockApiException> {
            unimplemented!()
        }

        async fn get_blocks_by_heights(
            &self,
            _: i64,
            _: i64,
        ) -> Result<Vec<LightBlockInfo>, BlockApiException> {
            unimplemented!()
        }

        async fn is_finalized(&self, _: &str) -> Result<bool, BlockApiException> {
            unimplemented!()
        }

        async fn get_transaction(&self, _: &str) -> Result<TransactionResponse, BlockApiException> {
            unimplemented!()
        }
    }

    /// A snapshot sampled now with headroom: the default for the router tests, so an unrelated route
    /// test is not refused by the memory gate. A pressured or stale arm replaces it.
    fn healthy_snapshot() -> ResourceSnapshot {
        ResourceSnapshot {
            state: "normal",
            fallback: ResourceState::Normal,
            mem_available_mb: 4_096,
            mem_total_mb: 8_192,
            sampled_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
                .unwrap_or(0),
            source: "test".to_string(),
            ..ResourceSnapshot::default()
        }
    }

    fn gate_over(snapshot: ResourceSnapshot) -> PressureGate {
        PressureGate::new(ResourceGuard::inert(snapshot), AdmissionPolicy::default())
    }

    fn state() -> HttpState {
        HttpState {
            reporter: Arc::new(NewPrometheusReporter::new(Configuration::default())),
            metrics: Arc::new(MetricsRegistry::new()),
            web_api: Arc::new(MockWebApi {
                status: test_status(),
                pooled_deploys: PooledDeploys {
                    deploys: Vec::new(),
                },
            }),
            block_report_api: test_block_report_api(),
            // The HTTP tests drive the routes, not the shard list; one member stands in.
            shards: Arc::new(ShardRegistry {
                primary: rchain_shared::refined::ShardId::try_from("/root".to_string()).unwrap(),
                members: Vec::new(),
            }),
            status_provider: None,
            // `GET /api/v1/pos` (AUDIT C148) is pinned on its own below; the other routes never read
            // this, so a value the assertions can name stands in.
            pos: Arc::new(StubPosRead),
            enable_reporting: true,
            deploy_rate_limiter: Arc::new(KeyedRateLimiter::new(
                DEFAULT_API_RATE_LIMIT_PER_SEC,
                MAX_TRACKED_PEERS,
            )),
            explore_rate_limiter: Arc::new(KeyedRateLimiter::new(
                DEFAULT_API_RATE_LIMIT_PER_SEC,
                MAX_TRACKED_PEERS,
            )),
            faucet_rate_limiter: Arc::new(KeyedRateLimiter::new(
                FAUCET_RATE_LIMIT_PER_SEC,
                MAX_TRACKED_PEERS,
            )),
            deploy_admission: None,
            explore_admission: None,
            pressure: gate_over(healthy_snapshot()),
            // On by default in the tests, because the router tests drive the faucet route; the
            // off case is pinned by its own test below.
            faucet_enabled: true,
        }
    }

    #[test]
    fn http_inflight_config_is_opt_in_and_rejects_invalid_values() {
        assert!(parse_inflight_limit(None, "TEST_LIMIT").unwrap().is_none());
        for invalid in ["0", "1025", "-1", "nan", "8 ", ""] {
            assert!(parse_inflight_limit(Some(invalid), "TEST_LIMIT").is_err());
        }
        let configured = parse_inflight_limit(Some("2"), "TEST_LIMIT")
            .unwrap()
            .expect("configured");
        assert_eq!(configured.limit, 2);
        assert_eq!(configured.semaphore.available_permits(), 2);
    }

    #[tokio::test]
    async fn in_flight_admission_refuses_explore_aliases_before_body_decode() {
        use tower::ServiceExt;
        let mut s = state();
        s.explore_admission = Some(InFlightAdmission::new(1));
        s.deploy_admission = Some(InFlightAdmission::new(2));
        let occupied = s
            .explore_admission
            .as_ref()
            .unwrap()
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .unwrap();
        for uri in [
            "/api/explore-deploy",
            "/api/v1/explore-deploy",
            "/api/explore-deploy-by-block-hash",
            "/api/v1/explore-deploy-by-block-hash",
        ] {
            let request = axum::http::Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(axum::body::Body::from("{ invalid JSON"))
                .unwrap();
            let reply = router(s.clone()).oneshot(request).await.unwrap();
            assert_eq!(reply.status(), StatusCode::SERVICE_UNAVAILABLE, "{uri}");
            assert_eq!(
                reply
                    .headers()
                    .get(axum::http::header::RETRY_AFTER)
                    .unwrap(),
                "1"
            );
        }
        let status = router(s.clone())
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/admission")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(status.status(), StatusCode::OK);
        let body = axum::body::to_bytes(status.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["explore"]["inFlight"], 1);
        assert_eq!(json["deploy"]["inFlight"], 0);
        assert_eq!(json["deploy"]["limit"], 2);
        drop(occupied);
        let reply = router(s)
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/admission")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let data = axum::body::to_bytes(reply.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&data).unwrap();
        assert_eq!(json["explore"]["available"], 1);
    }

    #[tokio::test]
    async fn deploy_budget_separate_and_status_stays_accessible() {
        use tower::ServiceExt;
        let mut s = state();
        s.deploy_admission = Some(InFlightAdmission::new(1));
        s.explore_admission = Some(InFlightAdmission::new(3));
        let held = s
            .deploy_admission
            .as_ref()
            .unwrap()
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .unwrap();
        for uri in ["/api/deploy", "/api/v1/deploy"] {
            let reply = router(s.clone())
                .oneshot(
                    axum::http::Request::builder()
                        .method("POST")
                        .uri(uri)
                        .header("content-type", "application/json")
                        .body(axum::body::Body::from("invalid json"))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(reply.status(), StatusCode::SERVICE_UNAVAILABLE);
        }
        let status = router(s.clone())
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/status")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(status.status(), StatusCode::OK);
        drop(held);
        let reply = router(s)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/deploy")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from("invalid json"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(reply.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    /// A real handler holds the permit across its await; completion and
    /// cancellation both return capacity without a waiting request queue.
    #[tokio::test]
    async fn in_flight_permit_covers_async_handler_and_releases_on_completion() {
        use tower::ServiceExt;
        let budget = InFlightAdmission::new(1);
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let entered = Arc::new(std::sync::Mutex::new(Some(entered_tx)));
        let release = Arc::new(tokio::sync::Notify::new());
        let app = Router::new()
            .route(
                "/slow",
                get({
                    let entered = entered.clone();
                    let release = release.clone();
                    move || {
                        let entered = entered.clone();
                        let release = release.clone();
                        async move {
                            if let Some(tx) = entered.lock().unwrap().take() {
                                let _ = tx.send(());
                            }
                            release.notified().await;
                            "done"
                        }
                    }
                }),
            )
            .route_layer(middleware::from_fn_with_state(
                (Some(budget.clone()), gate_over(healthy_snapshot())),
                in_flight_admission,
            ));

        let running = tokio::spawn({
            let app = app.clone();
            async move {
                app.oneshot(
                    axum::http::Request::builder()
                        .uri("/slow")
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        });
        tokio::time::timeout(Duration::from_secs(2), entered_rx)
            .await
            .expect("handler reached")
            .expect("handler notified");
        assert_eq!(budget.semaphore.available_permits(), 0);
        let refused = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/slow")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
        release.notify_one();
        assert_eq!(running.await.unwrap().status(), StatusCode::OK);
        assert_eq!(budget.semaphore.available_permits(), 1);

        let cancelled = tokio::spawn({
            let app = app.clone();
            async move {
                app.oneshot(
                    axum::http::Request::builder()
                        .uri("/slow")
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while budget.semaphore.available_permits() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("second handler acquired permit");
        cancelled.abort();
        let _ = cancelled.await;
        assert_eq!(budget.semaphore.available_permits(), 1);
    }

    #[tokio::test]
    async fn disabled_admission_does_not_claim_validator_health() {
        use tower::ServiceExt;
        let response = router(state())
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/admission")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["deploy"]["configured"], false);
        assert_eq!(json["explore"]["configured"], false);
        assert!(json.get("degraded").is_none());
    }

    /// **A node below its admission floor refuses the work before it decodes the body** (unit B34,
    /// B34/H-RS-02).
    ///
    /// The body is deliberately invalid JSON: a `503` proves the gate answered before axum's `Json`
    /// extractor ran, and the `Retry-After` proves a caller is told how to come back rather than
    /// being left to guess. Every deploy and explore alias is covered, because a gate on one alias is
    /// a gate an attacker walks around; `/api/status` stays reachable, because shedding work must not
    /// blind the operator.
    ///
    /// Falsifier: with a healthy snapshot the same requests reach the extractor and the replies are
    /// `400`s (the control assertion below).
    #[tokio::test]
    async fn a_deploy_is_refused_503_before_the_body_is_decoded() {
        use tower::ServiceExt;
        let mut pressured = healthy_snapshot();
        pressured.mem_available_mb = 10;

        let mut shed = state();
        shed.pressure = gate_over(pressured);

        for uri in [
            "/api/deploy",
            "/api/v1/deploy",
            "/api/explore-deploy",
            "/api/v1/explore-deploy",
            "/api/explore-deploy-by-block-hash",
            "/api/v1/explore-deploy-by-block-hash",
        ] {
            let reply = router(shed.clone())
                .oneshot(
                    axum::http::Request::builder()
                        .method("POST")
                        .uri(uri)
                        .header("content-type", "application/json")
                        .body(axum::body::Body::from("{ not json"))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                reply.status(),
                StatusCode::SERVICE_UNAVAILABLE,
                "{uri}: a pressured node must refuse before decode, not answer a parse error"
            );
            assert_eq!(
                reply
                    .headers()
                    .get(axum::http::header::RETRY_AFTER)
                    .expect("Retry-After"),
                "1"
            );
        }

        // The operator can still see the node while admission is shed.
        let status = router(shed)
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/status")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(status.status(), StatusCode::OK);

        // The control: on a healthy sample the same body reaches the extractor and is a `400`.
        let healthy = router(state())
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/deploy")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from("{ not json"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(
            healthy.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "a healthy node must not refuse the request"
        );
    }

    /// **A stale sample is a refusal, not a pass** (B33/B34): a stopped watchdog must not leave
    /// admission open on a reading nobody refreshed.
    #[tokio::test]
    async fn a_stale_resource_sample_refuses_deploys() {
        use tower::ServiceExt;
        let mut stale = healthy_snapshot();
        stale.sampled_at_ms = 1;
        let mut s = state();
        s.pressure = gate_over(stale);
        let reply = router(s)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/deploy")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reply.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    /// **AUDIT C148**: the PoS read has a route, and the route renders what it was handed. The
    /// numbers here are deliberately *not* round — an epoch of 3 with 40 blocks to the boundary and
    /// a withdrawal with 60 left are values a handler that dropped or transposed a field would fail
    /// on, where `0`s and `1`s would pass.
    struct StubPosRead;

    #[async_trait]
    impl PosReadApi for StubPosRead {
        async fn pos_status(&self) -> Result<crate::web::pos_read::PosStatus, String> {
            use crate::web::pos_read::{PendingWithdrawal, PosStatus};
            use rchain_models::validator::Validator;
            Ok(PosStatus {
                latest_block_number: 260,
                epoch_length: 100,
                quarantine_length: 100,
                epoch: 2,
                blocks_until_epoch_boundary: 40,
                active_validators: vec![
                    Validator::try_from([3u8; 65].as_slice()).expect("65 bytes")
                ],
                pending_withdrawals: vec![PendingWithdrawal {
                    validator: Validator::try_from([9u8; 65].as_slice()).expect("65 bytes"),
                    deadline: 260,
                    blocks_remaining: 60,
                }],
            })
        }

        /// The delegation read the route table also mounts (#193). Non-round values for the same
        /// reason `pos_status`'s are: a handler that dropped or transposed a field fails on these and
        /// would pass on zeros.
        async fn delegator_positions(
            &self,
            delegator: &Validator,
        ) -> Result<Vec<crate::web::pos_read::DelegatorPosition>, String> {
            use crate::web::pos_read::{DelegatorPosition, PendingUndelegation};
            // Empty for any key but the one the route's own test asks about, so a handler that
            // ignored `?delegator=` and returned a hard-coded list would be visible as a non-empty
            // answer about a key that has never delegated.
            if *delegator != Validator::try_from([7u8; 65].as_slice()).expect("65 bytes") {
                return Ok(Vec::new());
            }
            Ok(vec![DelegatorPosition {
                operator: Validator::try_from([8u8; 65].as_slice()).expect("65 bytes"),
                amount: 40,
                accrued_rewards: 17,
                pending_undelegation: Some(PendingUndelegation {
                    deadline: 260,
                    blocks_remaining: 60,
                }),
            }])
        }
    }

    /// The admin web API is not what these tests drive; `propose` is on the admin router and pinned
    /// elsewhere. A stub is enough to build `AdminState`, which the transaction handlers take
    /// (AUDIT C121).
    struct MockAdminWebApi;

    #[async_trait]
    impl AdminWebApi for MockAdminWebApi {
        async fn propose(&self) -> Result<String, BlockApiException> {
            unimplemented!()
        }

        async fn propose_result(&self) -> Result<String, BlockApiException> {
            unimplemented!()
        }

        async fn operator_status(&self) -> Result<OperatorStatus, BlockApiException> {
            Ok(test_operator_status())
        }

        async fn resume_autopropose(&self) -> Result<String, BlockApiException> {
            Ok("resumed".to_string())
        }

        async fn resume_resource_fallback(&self) -> Result<String, BlockApiException> {
            Ok("normal".to_string())
        }
    }

    /// **The public router builds under axum 0.8's path syntax.**
    ///
    /// axum 0.8 replaced `:param` segments with `{param}`, and `Router::route` **panics while the
    /// router is constructed** when it meets the old form. That panic fires inside the spawned HTTP
    /// task, so the node is otherwise healthy: it reaches `Running`, creates its genesis, and serves
    /// nothing — port 40403 never binds, so `/health`, `/api/*` and the deploy endpoint are all
    /// absent, and the only trace is one `tokio-rt-worker panicked ... Path segments must not start
    /// with :` line in the journal.
    ///
    /// The 2026-09-26 dependency bump compiled with every parameterised route still spelled
    /// `:param`; a CI *artifact build* cannot see this, and the first thing that could was a running
    /// node (found on the testnet, node A, 15:01 UTC — the node reached `Running` and never opened
    /// the API). `node/tests/api_surface.rs` boots a real node and would also fail on it, but it
    /// costs a boot; constructing the router here pins the route table for the price of a call.
    #[test]
    fn the_router_builds_under_axum08_path_syntax() {
        let _ = router(state());
    }

    /// Stage 6 / AUDIT C61 — the `/metrics` wire exists.
    ///
    /// The Prometheus surface was mounted and the reporter could render, but nothing in production
    /// called `report_period_snapshot`, so every scrape returned the reporter's initial placeholder
    /// (`prometheus_reporter.rs`'s `EMPTY_SCRAPE_DATA`). The handler now publishes a snapshot of the
    /// node's `MetricsRegistry` before rendering, so the surface reports the node's own numbers.
    ///
    /// Falsified against this tree (2026-09-24): with that one call removed — the shape before this
    /// stage — the body is exactly the placeholder string and both assertions below fail.
    #[tokio::test]
    async fn the_metrics_route_serves_the_registrys_own_numbers() {
        let state = state();
        state.metrics.set_gauge(
            &rchain_shared::metrics::Source::base().sub("dag"),
            "messages",
            42,
        );
        state.metrics.set_gauge(
            &rchain_shared::metrics::Source::base().sub("dag"),
            "logical_bytes",
            7,
        );

        let body = metrics(State(state)).await;
        assert!(
            body.contains("rchain_dag_messages 42"),
            "the scrape reports the registry's gauge, not the placeholder: {body}"
        );
        // The unit-bearing name is normalized on the way out (`_bytes`), which is the surface's own
        // rule rather than this stage's.
        assert!(body.contains("rchain_dag_logical_bytes 7"), "{body}");
        assert!(
            !body.contains("didn't receive any data just yet"),
            "the placeholder must be gone once a snapshot has been published"
        );
    }

    #[tokio::test]
    async fn the_metrics_route_serves_queue_depth_and_sampled_peak() {
        let state = state();
        let observe = state.metrics.queue_observer(
            rchain_shared::metrics::Source::base()
                .sub("block_pipeline")
                .sub("shard_0")
                .sub("validated"),
        );
        observe(9, true);
        let body = metrics(State(state.clone())).await;
        assert!(
            body.contains("rchain_block_pipeline_shard_0_validated_depth 9"),
            "{body}"
        );
        observe(2, true);
        let body = metrics(State(state.clone())).await;
        assert!(
            body.contains("rchain_block_pipeline_shard_0_validated_depth 2"),
            "{body}"
        );
        assert!(
            body.contains("rchain_block_pipeline_shard_0_validated_sampled_peak 9"),
            "{body}"
        );
        observe(0, false);
        let body = metrics(State(state)).await;
        assert!(
            body.contains("rchain_block_pipeline_shard_0_validated_depth 0"),
            "{body}"
        );
        assert!(
            body.contains("rchain_block_pipeline_shard_0_validated_consumer_active 0"),
            "{body}"
        );
        assert!(
            body.contains("rchain_block_pipeline_shard_0_validated_sampled_peak 9"),
            "{body}"
        );
    }

    /// The route serves the version **with its build commit**, because that is the surface a room
    /// member reads to learn which binary produced an attestation (issue #32) — and `commit #
    /// unknown` on a git checkout is exactly the failure this pins. `version_info`'s own test pins
    /// that the build script's value is compiled in; this one pins that the *route* reads it rather
    /// than formatting a version of its own, which is how the two drifted apart in the first place.
    #[tokio::test]
    async fn version_returns_node_version_with_its_build_commit() {
        let v = version().await;
        assert!(v.starts_with("rholo node "), "{v}");
        assert!(
            !v.contains("commit # unknown"),
            "the route must serve the build commit (built outside a git checkout? that is the one \
             legitimate cause), got {v}"
        );
    }

    #[tokio::test]
    async fn metrics_returns_scrape_data() {
        // The route renders the registry's snapshot, so the reporter's "no snapshot has been
        // reported yet" placeholder can no longer be a served node's body: an empty registry
        // renders no series at all (AUDIT C61). That the *content* is the registry's is pinned by
        // `the_metrics_route_serves_the_registrys_own_numbers`.
        let out = metrics(State(state())).await;
        assert_ne!(
            out, "# The kamon-prometheus module didn't receive any data just yet.\n",
            "a scrape publishes a snapshot, so the placeholder is never the body"
        );
        assert!(
            out.is_empty(),
            "an empty registry renders no series: {out:?}"
        );
    }

    #[tokio::test]
    async fn api_status_returns_json() {
        let response = api_status(State(state())).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["address"], "addr");
        assert_eq!(json["version"]["api"], "1.0");
    }

    #[tokio::test]
    async fn api_deploys_returns_wrapped_pool() {
        let mut s = state();
        s.web_api = Arc::new(MockWebApi {
            status: test_status(),
            pooled_deploys: PooledDeploys {
                deploys: vec![PooledDeploy {
                    deploy_id: "deadbeef".to_string(),
                    timestamp: 1724500000000,
                    deployer: "00".to_string(),
                    term: "Nil".to_string(),
                    phlo_price: 1,
                    phlo_limit: 100,
                    valid_after_block_number: -1,
                }],
            },
        });
        let response = api_deploys(State(s)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["deploys"][0]["deployId"], "deadbeef");
        assert_eq!(json["deploys"][0]["timestamp"], 1724500000000i64);
    }

    #[tokio::test]
    async fn api_deploys_empty_pool_returns_empty_array() {
        let response = api_deploys(State(state())).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["deploys"], serde_json::json!([]));
    }

    #[test]
    fn openapi_json_is_valid() {
        let doc: serde_json::Value =
            serde_json::from_str(OPENAPI_JSON).expect("OPENAPI_JSON parses");
        assert_eq!(doc["openapi"], "3.0.0");
        assert!(doc["paths"].is_object());
        assert!(doc["components"]["schemas"].is_object());
    }

    /// The served schema must describe the same refusal on every legacy/v1
    /// deploy/explore alias that the middleware actually enforces.
    #[tokio::test]
    async fn openapi_http_admission_refusal_matches_all_six_live_aliases() {
        use tower::ServiceExt;

        let doc: serde_json::Value =
            serde_json::from_str(OPENAPI_JSON).expect("served OpenAPI parses");
        assert_eq!(doc["servers"], json!([{ "url": "/api/v1" }]));
        assert!(doc["paths"]["/admission"]["get"]["servers"].is_null());

        let documented = &doc["components"]["responses"]["HttpAdmissionExhausted"];
        assert!(documented.is_object(), "shared 503 response missing");
        assert_eq!(
            documented["headers"]["Retry-After"]["schema"]["type"],
            "integer"
        );
        assert_eq!(documented["headers"]["Retry-After"]["schema"]["example"], 1);
        assert_eq!(
            documented["content"]["application/json"]["schema"]["type"],
            "string"
        );
        let example = documented["content"]["application/json"]["example"]
            .as_str()
            .expect("the 503 example is a string");
        assert!(
            example.contains("admission floor"),
            "the served example must describe the memory gate's refusal, got {example}"
        );

        let mut s = state();
        s.deploy_admission = Some(InFlightAdmission::new(1));
        s.explore_admission = Some(InFlightAdmission::new(1));
        let _held_deploy = s
            .deploy_admission
            .as_ref()
            .unwrap()
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .unwrap();
        let _held_explore = s
            .explore_admission
            .as_ref()
            .unwrap()
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .unwrap();

        for (logical_path, route) in [
            ("/deploy", "/api/deploy"),
            ("/deploy", "/api/v1/deploy"),
            ("/explore-deploy", "/api/explore-deploy"),
            ("/explore-deploy", "/api/v1/explore-deploy"),
            (
                "/explore-deploy-by-block-hash",
                "/api/explore-deploy-by-block-hash",
            ),
            (
                "/explore-deploy-by-block-hash",
                "/api/v1/explore-deploy-by-block-hash",
            ),
        ] {
            let operation = &doc["paths"][logical_path]["post"];
            assert_eq!(
                operation["responses"]["503"]["$ref"],
                "#/components/responses/HttpAdmissionExhausted",
                "{route} has no documented 503"
            );
            let servers = operation["servers"]
                .as_array()
                .expect("legacy/v1 aliases must be explicit in operation servers");
            assert_eq!(
                servers,
                &vec![json!({"url": "/api"}), json!({"url": "/api/v1"})]
            );
            assert!(
                servers.iter().any(|server| {
                    let prefix = server["url"].as_str().unwrap();
                    format!("{prefix}{logical_path}") == route
                }),
                "missing OpenAPI mapping for {route}"
            );

            let request = axum::http::Request::builder()
                .method("POST")
                .uri(route)
                .header("content-type", "application/json")
                .body(axum::body::Body::from("{ invalid JSON"))
                .unwrap();
            let reply = router(s.clone()).oneshot(request).await.unwrap();
            assert_eq!(reply.status(), StatusCode::SERVICE_UNAVAILABLE, "{route}");
            assert_eq!(
                reply
                    .headers()
                    .get(axum::http::header::RETRY_AFTER)
                    .unwrap(),
                "1",
                "{route}"
            );
            let body = to_bytes(reply.into_body(), usize::MAX).await.unwrap();
            let error_text: String = serde_json::from_slice(&body).unwrap();
            assert_eq!(error_text, "HTTP in-flight capacity exhausted", "{route}");
        }
    }

    fn status_provider() -> StatusProvider {
        let local = PeerNode::from(
            NodeIdentifier::new(vec![1]),
            "localhost".to_string(),
            rchain_shared::refined::Port::new(40400),
            rchain_shared::refined::Port::new(40404),
        );
        StatusProvider {
            connections: Arc::new(tokio::sync::RwLock::new(vec![local])),
            rp_conf: RPConf {
                local: PeerNode::from(
                    NodeIdentifier::new(vec![2]),
                    "localhost".to_string(),
                    rchain_shared::refined::Port::new(40400),
                    rchain_shared::refined::Port::new(40404),
                ),
                network_id: "testnet".to_string(),
                bootstrap: None,
                default_timeout: std::time::Duration::from_secs(10),
                max_num_of_connections: 100,
                clear_connections: ClearConnectionsConf {
                    num_of_connections_pinged: 10,
                },
            },
            discovery: Arc::new(NoopDiscovery),
        }
    }

    #[tokio::test]
    async fn status_returns_comm_state() {
        let mut s = state();
        s.status_provider = Some(status_provider());
        let response = status(State(s)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["peers"], 1);
        assert_eq!(json["nodes"], 0);
    }

    /// The deploy route is rate-limited, and an exhausted limiter is a 429 — not a silent drop and
    /// not an unbounded accept. (A limiter configured to zero is closed, so this needs no sleep.)
    ///
    /// **Driven through the router**, which is where the bound now lives: it is a `route_layer`, so it
    /// runs before the handler and before the body is decoded. Calling the handler directly would test
    /// nothing — the check is not in it.
    #[tokio::test]
    async fn api_deploy_returns_429_when_the_limiter_is_exhausted() {
        use tower::ServiceExt;

        let mut s = state();
        s.deploy_rate_limiter = Arc::new(KeyedRateLimiter::new(0, MAX_TRACKED_PEERS));
        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/api/deploy")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "data": {
                        "term": "Nil",
                        "timestamp": 0,
                        "phloPrice": 1,
                        "phloLimit": 1,
                        "validAfterBlockNumber": 0,
                        "shardId": "/root"
                    },
                    "deployer": "",
                    "signature": "",
                    "sigAlgorithm": "secp256k1"
                }))
                .expect("json"),
            ))
            .expect("a request");
        let response = router(s).oneshot(request).await.expect("a response");
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    /// **The limiter is consulted before the deploy body is decoded** (W-N2-06).
    ///
    /// The row: the rate check ran *inside* the handler, so axum's `Json` extractor had already
    /// decoded the body — and a deploy may be 16 MiB on the wire — before the caller was told it was
    /// over the limit. An over-limit flood therefore still paid the full decode, and the bound the
    /// limiter exists to be is a bound on admitted work, not on work already done.
    ///
    /// The order is observable from outside because a **body that cannot be decoded at all** has to
    /// be answered by whichever ran first: the limiter (429) or the extractor (400). The second arm
    /// is what makes the first a fact about the *order* rather than about the route — the same
    /// malformed body, with the bound slack, is refused by the extractor, so the route does read
    /// bodies and arm 1's 429 can only mean it never got there.
    ///
    /// Falsified by moving the check back into the handler: arm 1 answers 400 and the assertion on
    /// the status reddens.
    #[tokio::test]
    async fn the_limiter_is_consulted_before_the_deploy_body_is_decoded() {
        use tower::ServiceExt;

        // Malformed *and* not small: nothing here is JSON, and the size is what the row's flood pays.
        let body = vec![b'x'; 64 * 1024];
        let build = || {
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/deploy")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.clone()))
                .expect("a request")
        };

        let mut s = state();
        s.deploy_rate_limiter = Arc::new(KeyedRateLimiter::new(0, MAX_TRACKED_PEERS));
        let response = router(s).oneshot(build()).await.expect("a response");
        assert_eq!(
            response.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "an over-limit request is refused before its body is read, so a body that cannot be \
             decoded must not turn the refusal into the extractor's 400 (W-N2-06)"
        );

        let response = router(state()).oneshot(build()).await.expect("a response");
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "and the same body is the extractor's refusal once the bound lets it through — without \
             this arm the 429 above would be consistent with a route that never reads a body"
        );
    }

    /// **One peer's burst does not refuse another peer's request** (W-N2-01, W-N2-02, W-N2-08).
    ///
    /// The bound's subject was the *node*: one process-wide bucket per route, so an exhausted limiter
    /// refused every caller — the bound held, and the wrong counterparty paid. It is the peer address
    /// now, so one peer's burst must leave another's allowance untouched. This is the assertion that
    /// distinguishes a keyed bound from an unkeyed one; nothing about the single-peer case can.
    ///
    /// Falsified by keying on anything constant — the node, or the empty key — under which the third
    /// request is a 429 as well.
    #[tokio::test]
    async fn one_peers_burst_does_not_refuse_another_peers_request() {
        use tower::ServiceExt;

        fn deploy_from(addr: SocketAddr) -> axum::http::Request<axum::body::Body> {
            let mut request = axum::http::Request::builder()
                .method("POST")
                .uri("/api/deploy")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::to_vec(&serde_json::json!({
                        "data": {
                            "term": "Nil", "timestamp": 0, "phloPrice": 1, "phloLimit": 1,
                            "validAfterBlockNumber": 0, "shardId": "/root"
                        },
                        "deployer": "", "signature": "", "sigAlgorithm": "secp256k1"
                    }))
                    .expect("json"),
                ))
                .expect("a request");
            // What `into_make_service_with_connect_info` puts there in production.
            request.extensions_mut().insert(ConnectInfo(addr));
            request
        }

        let mut s = state();
        // One token each, so a single request exhausts a peer's allowance.
        s.deploy_rate_limiter = Arc::new(KeyedRateLimiter::new(1, MAX_TRACKED_PEERS));
        let alice: SocketAddr = "192.0.2.1:5000".parse().expect("addr");
        let bob: SocketAddr = "192.0.2.2:5000".parse().expect("addr");

        let first = router(s.clone())
            .oneshot(deploy_from(alice))
            .await
            .expect("a response");
        assert_ne!(
            first.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "a peer's first request is within its allowance"
        );
        let second = router(s.clone())
            .oneshot(deploy_from(alice))
            .await
            .expect("a response");
        assert_eq!(
            second.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "and its second is over it"
        );
        let bobs = router(s)
            .oneshot(deploy_from(bob))
            .await
            .expect("a response");
        assert_ne!(
            bobs.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "another peer's allowance is its own — one caller's burst must not refuse every caller, \
             which is the defect this keying removes"
        );
    }

    /// **The bound runs before the body is decoded** (W-N2-06, W-N2-09).
    ///
    /// It used to be three lines at the top of the handler, *after* axum's `Json` extractor had
    /// already decoded the body — so a caller could make the node decode as many payloads as it liked
    /// and only then be told it was over the limit. The body here is **not valid JSON**: if the
    /// extractor ran first the answer would be a 4xx about the body, and a 429 is the proof that the
    /// layer refused the request before anything read it.
    #[tokio::test]
    async fn the_ingress_bound_refuses_a_body_it_never_decoded() {
        use tower::ServiceExt;

        let mut s = state();
        s.deploy_rate_limiter = Arc::new(KeyedRateLimiter::new(0, MAX_TRACKED_PEERS));
        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/api/deploy")
            .header("content-type", "application/json")
            // Deliberately not JSON, and deliberately large enough that decoding it is real work.
            .body(axum::body::Body::from("not json at all"))
            .expect("a request");
        let response = router(s).oneshot(request).await.expect("a response");
        assert_eq!(
            response.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "a refused peer must be refused before its body is decoded, not after"
        );
    }

    /// **A node without the faucet does not mount the route, so the documented 404 is the answer**
    /// (AUDIT R34).
    ///
    /// This one drives the *router* rather than a handler, because the thing under test is the mount
    /// — and the reason it matters is the schema: `GET /api/v1/openapi.json` documents this route's
    /// refusal as **404** ("The faucet is disabled on this node"), while the handler's
    /// `BlockApiException` maps to **400**. A client reading the schema and a client reading the wire
    /// disagreed about the same event, which is what "the gate is only inside the handler" cost.
    ///
    /// Two arms: disabled is a 404, and enabled is not — so the mount condition cannot be a router
    /// that drops everything.
    #[tokio::test]
    async fn a_node_without_the_faucet_does_not_mount_the_route() {
        use tower::ServiceExt;

        let request = || {
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/faucet")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    r#"{"address":"rBdXnotARealAddress"}"#,
                ))
                .expect("a request")
        };

        let mut off = state();
        off.faucet_enabled = false;
        let resp = router(off).oneshot(request()).await.expect("a response");
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "the route is not mounted, so the schema's 404 is what a caller gets"
        );

        let on = state();
        let resp = router(on).oneshot(request()).await.expect("a response");
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "with the faucet on the route exists and answers (here, refusing a bad address)"
        );
    }

    /// **The explore routes have their own budget, and exhausting it leaves deploy's alone** (AUDIT
    /// R36). They shared one limiter until 2026-09-27, so an explore flood spent the deploy budget —
    /// the two routes starved each other for a resource only one of them uses heavily. An exploratory
    /// deploy *runs a term*; a deploy validates one and pools it.
    ///
    /// The arm that matters is the second: deploy still answers, so what changed is that there are two
    /// budgets rather than that one of them got stricter. **Falsified by pointing both fields at the
    /// same limiter** — the state the row describes — and then the deploy request is a 429.
    #[tokio::test]
    async fn an_explore_flood_does_not_spend_the_deploy_budget() {
        use tower::ServiceExt;

        let mut s = state();
        s.explore_rate_limiter = Arc::new(KeyedRateLimiter::new(0, MAX_TRACKED_PEERS));

        let explore = axum::http::Request::builder()
            .method("POST")
            .uri("/api/explore-deploy")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(r#""Nil""#))
            .expect("a request");
        let response = router(s.clone())
            .oneshot(explore)
            .await
            .expect("a response");
        assert_eq!(
            response.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "explore is closed"
        );

        let deploy = axum::http::Request::builder()
            .method("POST")
            .uri("/api/deploy")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "data": {
                        "term": "Nil",
                        "timestamp": 0,
                        "phloPrice": 1,
                        "phloLimit": 1,
                        "validAfterBlockNumber": 0,
                        "shardId": "/root"
                    },
                    "deployer": "",
                    "signature": "",
                    "sigAlgorithm": "secp256k1"
                }))
                .expect("json"),
            ))
            .expect("a request");
        let response = router(s).oneshot(deploy).await.expect("a response");
        assert_ne!(
            response.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "a closed explore budget must not refuse a deploy — they are separate budgets"
        );
    }

    // --- the shard list and the cross-shard transaction routes (Laws 26–29) ---

    /// A `BlockApi` for the transaction-route tests: only the three methods the gateway's phase
    /// path touches are real, because these tests drive *validation* and the gate, not a live 2PC.
    struct TxnStubApi {
        shard_id: String,
    }

    #[async_trait]
    impl BlockApi for TxnStubApi {
        async fn deploy(
            &self,
            _: &rchain_models::casper::protocol::casper_message::SignedDeployData,
        ) -> Result<String, String> {
            Err("no phase deploys in these tests".to_string())
        }
        async fn get_latest_message(
            &self,
        ) -> Result<rchain_models::block_metadata::BlockMetadata, String> {
            Err("no head in these tests".to_string())
        }
        /// Delegates to the floored read: this double has no unfinalised view of its own, and the
        /// method is required rather than defaulted so that is a decision rather than an oversight
        /// (`ShardRoutingBlockApi` is the implementation that had to forward it for real).
        async fn clear_autopropose_halt(&self) -> Result<(), String> {
            Ok(())
        }

        /// A stub has no finality cell to read, so it reports **nothing observed** — which this
        /// trait's docs distinguish from "observed healthy". See [`Self::finality_health`].
        async fn finality_health(&self) -> FinalityHealth {
            FinalityHealth::default()
        }

        /// A stub is not a node, so it has no member shards to map.
        async fn health_by_shard(&self) -> BTreeMap<String, ShardHealthEntry> {
            BTreeMap::new()
        }

        async fn get_unfinalised_name_data_response(
            &self,
            depth: i32,
            listening_name: &rchain_models::ast::Par,
        ) -> Result<
            (
                Vec<rchain_models::casper::protocol::deploy_service::DataWithBlockInfo>,
                i32,
            ),
            String,
        > {
            self.get_listening_name_data_response(depth, listening_name)
                .await
        }

        async fn get_listening_name_data_response(
            &self,
            depth: i32,
            _: &rchain_models::ast::Par,
        ) -> Result<
            (
                Vec<rchain_models::casper::protocol::deploy_service::DataWithBlockInfo>,
                i32,
            ),
            String,
        > {
            Ok((Vec::new(), depth))
        }
        async fn status(&self) -> rchain_models::casper::protocol::deploy_service::Status {
            rchain_models::casper::protocol::deploy_service::Status {
                version: rchain_models::casper::protocol::deploy_service::VersionInfo {
                    api: "1".to_string(),
                    node: "test".to_string(),
                },
                address: self.shard_id.clone(),
                network_id: "testnet".to_string(),
                shard_id: self.shard_id.clone(),
                peers: 0,
                nodes: 0,
                min_phlo_price: 1,
                latest_block_number: if self.shard_id == "/root" { 7 } else { 3 },
            }
        }
        async fn deploy_status(
            &self,
            _: &rchain_block_storage::dag::dag_storage::DeployId,
        ) -> Result<rchain_models::casper::protocol::deploy_service::DeployExecStatus, String>
        {
            Err("unused".to_string())
        }
        async fn pooled_deploys(
            &self,
        ) -> Result<Vec<rchain_models::casper::protocol::casper_message::SignedDeployData>, String>
        {
            Err("unused".to_string())
        }
        async fn capabilities(&self) -> rchain_casper::api::block_api::Capabilities {
            unreachable!("unused in these tests")
        }
        async fn create_block(&self, _: bool) -> Result<String, String> {
            Err("unused".to_string())
        }
        async fn get_propose_result(&self) -> Result<String, String> {
            Err("unused".to_string())
        }
        async fn get_listening_name_continuation_response(
            &self,
            _: i32,
            _: &[rchain_models::ast::Par],
        ) -> Result<
            (
                Vec<rchain_models::casper::protocol::deploy_service::ContinuationsWithBlockInfo>,
                i32,
            ),
            String,
        > {
            Err("unused".to_string())
        }
        async fn get_blocks_by_heights(
            &self,
            _: i64,
            _: i64,
        ) -> Result<Vec<LightBlockInfo>, String> {
            Err("unused".to_string())
        }
        async fn visualize_dag(&self, _: i32, _: i32, _: bool) -> Result<Vec<String>, String> {
            Err("unused".to_string())
        }
        async fn machine_verifiable_dag(&self, _: i32) -> Result<String, String> {
            Err("unused".to_string())
        }
        async fn get_blocks(&self, _: i32) -> Result<Vec<LightBlockInfo>, String> {
            Err("unused".to_string())
        }
        async fn find_deploy(
            &self,
            _: &rchain_block_storage::dag::dag_storage::DeployId,
        ) -> Result<LightBlockInfo, String> {
            Err("unused".to_string())
        }
        async fn get_block(&self, _: &str) -> Result<BlockInfo, String> {
            Err("unused".to_string())
        }
        async fn bond_status(&self, _: &[u8]) -> Result<bool, String> {
            Err("unused".to_string())
        }
        async fn exploratory_deploy(
            &self,
            _: &str,
            _: Option<&str>,
            _: bool,
        ) -> Result<(CapturedReply, LightBlockInfo), String> {
            Err("unused".to_string())
        }
        async fn get_data_at_par(
            &self,
            _: &rchain_models::ast::Par,
            _: &str,
            _: bool,
        ) -> Result<(Vec<rchain_models::ast::Par>, LightBlockInfo), String> {
            Err("unused".to_string())
        }
        async fn last_finalized_block(&self) -> Result<BlockInfo, String> {
            Err("unused".to_string())
        }
        async fn is_finalized(&self, _: &str) -> Result<bool, String> {
            Err("unused".to_string())
        }
    }

    /// A [`KeyValueStoreManager`] whose stores reject every read — how `api_txn_status`'s 500 arm is
    /// reached (a ledger that opens and then cannot be read).
    struct UnreadableStoreManager;

    struct UnreadableStore;

    impl rchain_shared::store::KeyValueStore for UnreadableStore {
        fn get(&self, _: &[Vec<u8>]) -> Result<Vec<Option<Vec<u8>>>, String> {
            Err("store read failed".to_string())
        }
        fn put(&mut self, _: Vec<(Vec<u8>, Vec<u8>)>) -> Result<(), String> {
            Err("store write failed".to_string())
        }
        fn delete(&mut self, _: &[Vec<u8>]) -> Result<usize, String> {
            Err("store delete failed".to_string())
        }
        fn entries(&self) -> Result<Vec<(Vec<u8>, Vec<u8>)>, String> {
            Err("store scan failed".to_string())
        }
    }

    #[async_trait]
    impl rchain_shared::store_manager::KeyValueStoreManager for UnreadableStoreManager {
        async fn store(&self, _: &str) -> Result<SharedStore, String> {
            let store: Box<dyn rchain_shared::store::KeyValueStore + Send + Sync> =
                Box::new(UnreadableStore);
            Ok(Arc::new(tokio::sync::Mutex::new(store)))
        }
        async fn shutdown(&self) {}
    }

    fn txn_shards() -> Vec<(rchain_shared::refined::ShardId, Arc<dyn BlockApi>)> {
        let shard = |id: &str| rchain_shared::refined::ShardId::try_from(id.to_string()).unwrap();
        vec![
            (
                shard("/root"),
                Arc::new(TxnStubApi {
                    shard_id: "/root".to_string(),
                }) as Arc<dyn BlockApi>,
            ),
            (
                shard("/root/child"),
                Arc::new(TxnStubApi {
                    shard_id: "/root/child".to_string(),
                }) as Arc<dyn BlockApi>,
            ),
        ]
    }

    /// A gateway over the stub shards, with a real ledger.
    async fn gateway() -> Arc<GatewayTxn> {
        gateway_over(Arc::new(
            rchain_shared::store_manager::InMemoryStoreManager::default(),
        ))
        .await
    }

    async fn gateway_over(
        manager: Arc<dyn rchain_shared::store_manager::KeyValueStoreManager>,
    ) -> Arc<GatewayTxn> {
        let (key, pub_key) = rchain_casper::construct_deploy::default_key_pair().unwrap();
        let mut shards = std::collections::BTreeMap::new();
        for (shard_id, api) in txn_shards() {
            shards.insert(
                shard_id.clone(),
                rchain_casper::gateway::LocalShard {
                    shard_id,
                    block_api: api,
                    max_listen_depth: 50,
                },
            );
        }
        Arc::new(rchain_casper::gateway::GatewayTxn::new(
            Arc::new(rchain_casper::gateway::LocalShardDeployService::new(shards)),
            Arc::new(
                rchain_casper::gateway::ledger::TxnLedger::open(manager.as_ref())
                    .await
                    .expect("ledger"),
            ),
            key,
            pub_key,
            Duration::from_millis(50),
        ))
    }

    /// **Admin** state with an optional gateway — the listener the transaction routes are mounted on
    /// (AUDIT C121). It was the public state until that finding: the coordinator spends from the node's
    /// own REV account, so it is privileged like `/api/propose`, not public like `/api/deploy`.
    fn txn_state(gateway: Option<Arc<GatewayTxn>>, enable_txn_api: bool) -> AdminState {
        AdminState {
            admin_web_api: Arc::new(MockAdminWebApi),
            enable_devnet_cors: false,
            gateway,
            enable_txn_api,
            txn_rate_limiter: Arc::new(KeyedRateLimiter::new(
                TXN_RATE_LIMIT_PER_SEC,
                MAX_TRACKED_PEERS,
            )),
            pos: Arc::new(StubPosRead),
            ocapn_dial: Arc::new(std::sync::OnceLock::new()),
            enable_ocapn_dial: false,
            ocapn_dial_rate_limiter: Arc::new(KeyedRateLimiter::new(
                OCAPN_DIAL_RATE_LIMIT_PER_SEC,
                MAX_TRACKED_PEERS,
            )),
            resume_rate_limiter: Arc::new(KeyedRateLimiter::new(
                RESUME_AUTOPROPOSE_RATE_LIMIT_PER_SEC,
                MAX_TRACKED_PEERS,
            )),
        }
    }

    /// Public state whose shard list is populated. `api_shards` is the only handler that reads
    /// `HttpState::shards`, and it stayed on the public router while the transaction routes moved, so
    /// this helper is separate from [`txn_state`] on purpose.
    fn state_with_shards(
        members: Vec<(rchain_shared::refined::ShardId, Arc<dyn BlockApi>)>,
    ) -> HttpState {
        let mut s = state();
        s.shards = Arc::new(ShardRegistry {
            primary: rchain_shared::refined::ShardId::try_from("/root".to_string()).unwrap(),
            members,
        });
        s
    }

    fn txn_body(txn_id: &str, legs: serde_json::Value) -> crate::api::dto::TxnRequest {
        serde_json::from_value(serde_json::json!({ "txnId": txn_id, "legs": legs })).unwrap()
    }

    /// The two ingress checks that are *not* at the boundary — pinned here because a client sees the
    /// 400 either way, and the difference is only which layer says it. The amount's check is the
    /// ledger's `NonNegI64` refinement (`GatewayTxn::run`), the shard's is `ShardId::try_from`, and the
    /// empty `to` is the one this handler has to make itself (AUDIT §8).
    #[tokio::test]
    async fn a_leg_with_an_empty_to_is_rejected_before_the_gateway_runs() {
        let response = api_txn_run(
            State(txn_state(Some(gateway().await), true)),
            Json(txn_body(
                "6161",
                serde_json::json!([{ "shardId": "/root", "amount": 10, "to": "   " }]),
            )),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(
            String::from_utf8_lossy(&body).contains("empty 'to'"),
            "{}",
            String::from_utf8_lossy(&body)
        );
    }

    #[tokio::test]
    async fn a_negative_leg_amount_is_rejected_by_the_ledgers_refinement() {
        let response = api_txn_run(
            State(txn_state(Some(gateway().await), true)),
            Json(txn_body(
                "6262",
                serde_json::json!([{ "shardId": "/root", "amount": -1, "to": "1111abc" }]),
            )),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(
            String::from_utf8_lossy(&body).contains("amount"),
            "{}",
            String::from_utf8_lossy(&body)
        );
    }

    #[tokio::test]
    async fn api_shards_lists_every_member_primary_first() {
        let response = api_shards(State(state_with_shards(txn_shards()))).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["primaryShard"], "/root");
        assert_eq!(json["shardCount"], 2);
        assert_eq!(json["shards"][0]["shardId"], "/root");
        assert_eq!(json["shards"][0]["primary"], true);
        assert_eq!(json["shards"][0]["latestBlockNumber"], 7);
        assert_eq!(json["shards"][1]["shardId"], "/root/child");
        assert_eq!(json["shards"][1]["primary"], false);
        assert_eq!(json["shards"][1]["latestBlockNumber"], 3);
    }

    #[tokio::test]
    async fn api_shards_with_no_members_is_an_empty_list() {
        let response = api_shards(State(state_with_shards(Vec::new()))).await;
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["shardCount"], 0);
        assert_eq!(json["shards"].as_array().unwrap().len(), 0);
    }

    /// **AUDIT C148's surface, rendered.** Every field the row says had no read is asserted by name,
    /// and the two that carry an *answer* rather than a value — how far the next boundary is, and how
    /// long a staged withdrawal has left — are the ones a transposed field would silently swap.
    #[tokio::test]
    async fn api_v1_pos_answers_the_epoch_the_validator_set_and_the_staged_withdrawals() {
        let response = api_pos_status(State(state())).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["latestBlockNumber"], 260);
        assert_eq!(json["epochLength"], 100);
        assert_eq!(json["epoch"], 2);
        assert_eq!(json["blocksUntilEpochBoundary"], 40);
        assert_eq!(json["quarantineLength"], 100);
        assert_eq!(
            json["activeValidators"].as_array().unwrap().len(),
            1,
            "the active set is the consensus set, not the bond pool: {json}"
        );
        // The validator renders as hex, which is what an operator copies into `bond-status`.
        assert!(json["activeValidators"][0]
            .as_str()
            .unwrap()
            .starts_with("0303"));
        // `deadline`, not `stagedAtBlock`: the stored value is law 47's deadline, which already
        // contains the quarantine (AUDIT C206 — the rename is the fix's user-visible half).
        assert_eq!(json["pendingWithdrawals"][0]["deadline"], 260);
        assert_eq!(json["pendingWithdrawals"][0]["blocksRemaining"], 60);
    }

    /// **The operator status is on the admin listener and nowhere else** (W-N3-01).
    ///
    /// The two halves are asserted together, and the *public* half is the one that matters: a route
    /// mounted by mistake is a leak, and the 404 is what says it is not mounted. Falsified by adding
    /// the route to `router()` — which is exactly what the change removes.
    #[tokio::test]
    async fn the_operator_status_is_served_on_the_admin_listener_only() {
        use tower::ServiceExt;

        let request = || {
            axum::http::Request::builder()
                .method("GET")
                .uri("/api/v1/status/operator")
                .body(axum::body::Body::empty())
                .expect("a request")
        };

        let public = router(state())
            .oneshot(request())
            .await
            .expect("a response");
        assert_eq!(
            public.status(),
            StatusCode::NOT_FOUND,
            "the public listener must not serve the operator half"
        );

        let served = admin_router(txn_state(None, false))
            .oneshot(request())
            .await
            .expect("a response");
        assert_eq!(served.status(), StatusCode::OK, "the admin listener does");
    }

    /// **The resource-fallback resume is an admin actuator** (unit C, B37): mounted on the privileged
    /// listener and answering the resulting state. The pressure semantics are the state machine's,
    /// pinned in `resources::fallback`; this pins the route and its answer shape.
    #[tokio::test]
    async fn the_resource_resume_route_answers_the_state() {
        use tower::ServiceExt;
        let reply = admin_router(txn_state(None, false))
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/resources/resume")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reply.status(), StatusCode::OK);
        let body = axum::body::to_bytes(reply.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["state"], "normal");
    }

    /// **And the delegations route moved with it** (W-N3-03): it answered any caller's question about
    /// any delegator's portfolio from the public port, and it is a store query keyed on a
    /// caller-supplied key rather than chain state a peer would read.
    #[tokio::test]
    async fn the_delegations_route_is_served_on_the_admin_listener_only() {
        use tower::ServiceExt;

        let request = || {
            axum::http::Request::builder()
                .method("GET")
                .uri("/api/v1/pos/delegations?delegator=07")
                .body(axum::body::Body::empty())
                .expect("a request")
        };

        let public = router(state())
            .oneshot(request())
            .await
            .expect("a response");
        assert_eq!(public.status(), StatusCode::NOT_FOUND);

        let served = admin_router(txn_state(None, false))
            .oneshot(request())
            .await
            .expect("a response");
        assert_ne!(
            served.status(),
            StatusCode::NOT_FOUND,
            "the route exists on the admin listener (here, refusing a key that is not 65 bytes)"
        );
    }

    /// **#193's read, rendered.** The stub answers for exactly one key (65 bytes of `0x07`) and
    /// returns an empty list for any other, so a handler that ignored `?delegator=` and returned a
    /// fixed list is visible here rather than passing.
    #[tokio::test]
    async fn api_v1_pos_delegations_answers_one_delegators_positions() {
        // **The admin state**, because the route moved to the admin listener (W-N3-03): it was
        // answering any caller's question about any delegator's portfolio from the public port.
        let response = api_pos_delegations(
            State(txn_state(None, false)),
            Query(PosDelegationsQuery {
                delegator: rchain_shared::base16::encode(&[7u8; 65]),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 1, "{json}");
        assert_eq!(json[0]["amount"], 40);
        assert_eq!(json[0]["accruedRewards"], 17);
        assert_eq!(json[0]["pendingUndelegation"]["deadline"], 260);
        assert_eq!(json[0]["pendingUndelegation"]["blocksRemaining"], 60);
        assert!(json[0]["operator"].as_str().unwrap().starts_with("0808"));

        // And a key that has never delegated gets an empty list — the scoping is the handler's, not
        // the stub's, so asking about a *different* key must not return the first one's position.
        let other = api_pos_delegations(
            State(txn_state(None, false)),
            Query(PosDelegationsQuery {
                delegator: rchain_shared::base16::encode(&[6u8; 65]),
            }),
        )
        .await;
        let body = to_bytes(other.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 0, "{json}");
    }

    /// A key that cannot be parsed is a **`400`, not an empty list**: an empty list is a *true
    /// answer* about a delegator with no positions, and a caller that mistyped its own key must not
    /// be told that.
    #[tokio::test]
    async fn api_v1_pos_delegations_refuses_a_key_it_cannot_parse() {
        for bad in ["nothex", "aabb", ""] {
            let response = api_pos_delegations(
                State(txn_state(None, false)),
                Query(PosDelegationsQuery {
                    delegator: bad.to_string(),
                }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{bad:?}");
        }
    }

    /// The gate: without a gateway, or with the feature switched off, the transaction routes answer
    /// 404 — the same convention the reporting routes use. The `(Some, false)` case is the one no
    /// test reached before, and it is what stops a gateway node from serving the routes before an
    /// operator enables them.
    #[tokio::test]
    async fn txn_routes_404_when_the_gateway_is_off_or_disabled() {
        for (gateway, enabled) in [(None, false), (Some(gateway().await), false), (None, true)] {
            let state = || txn_state(gateway.clone(), enabled);
            assert_eq!(
                api_txn_list(State(state())).await.status(),
                StatusCode::NOT_FOUND
            );
            assert_eq!(
                api_txn_status(State(state()), Path("aabb".to_string()))
                    .await
                    .status(),
                StatusCode::NOT_FOUND
            );
            assert_eq!(
                api_txn_run(
                    State(state()),
                    Json(txn_body("/root", serde_json::json!([])))
                )
                .await
                .status(),
                StatusCode::NOT_FOUND
            );
        }
    }

    /// **The dial route is off unless asked for, and says so before any transport is bound**
    /// (issue #249). Two distinct answers, because they mean different things: 404 is "this node does
    /// not dial" (the feature is off), 503 is "not ready yet" (enabled, but the listener has bound no
    /// transport). One status for both would leave a client unable to tell them apart.
    #[tokio::test]
    async fn the_dial_route_is_404_unless_asked_for_and_503_before_transports_bind() {
        let body = |swiss: &str| {
            Json(OcapnDialRequest {
                designator: "peer".to_string(),
                transport: "tcp-testing-only".to_string(),
                hints: Default::default(),
                swiss: swiss.to_string(),
            })
        };

        // Off by default: the gate answers 404, the same shape `/api/v1/txn` uses.
        let off = txn_state(None, false);
        assert!(!off.enable_ocapn_dial);
        assert_eq!(
            admin_ocapn_dial(State(off), body("aabb")).await.status(),
            StatusCode::NOT_FOUND
        );

        // Enabled but nothing bound: 503, not a dial into "this node speaks nothing".
        let mut on = txn_state(None, false);
        on.enable_ocapn_dial = true;
        assert_eq!(
            admin_ocapn_dial(State(on), body("aabb")).await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );

        // A dialer bound, and a swiss number that is not base16: 400 — and the decode is before the
        // dial, so no layer is ever asked to connect.
        let mut bound = txn_state(None, false);
        bound.enable_ocapn_dial = true;
        let published = bound.ocapn_dial.set(crate::api::ocapn::OcapnDialer::new(
            Arc::new(
                rchain_ocapn::tcp_testing_only::TcpTestingOnly::bind("127.0.0.1:0")
                    .await
                    .expect("bind a throwaway layer"),
            ),
            Arc::new(rchain_ocapn::owner::SessionRegistry::default()),
            rchain_ocapn::locator::PeerLocator {
                designator: "self".to_string(),
                transport: "tcp-testing-only".to_string(),
                hints: Default::default(),
            },
            Arc::new(tokio::sync::Semaphore::new(
                rchain_ocapn::capacity::MAX_DIALED_SESSIONS,
            )),
        ));
        assert!(published.is_ok(), "the slot starts empty");
        assert_eq!(
            admin_ocapn_dial(State(bound), body("not-hex!!"))
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }

    /// The leg count is bounded before the coordinator is driven (AUDIT C121). One request used to buy
    /// as many node-signed deploys as the caller cared to list, from a coordinator that spends the
    /// node's own REV account — so `legs.len()` is now checked against [`MAX_TXN_LEGS`] at the handler,
    /// before the per-leg parse loop and before `GatewayTxn::run`.
    #[tokio::test]
    async fn a_transaction_naming_too_many_legs_is_rejected() {
        let leg = serde_json::json!({ "shardId": "/root", "amount": 1, "to": "1111abc" });
        let legs = |n: usize| serde_json::Value::Array(vec![leg.clone(); n]);

        let response = api_txn_run(
            State(txn_state(Some(gateway().await), true)),
            Json(txn_body("7373", legs(MAX_TXN_LEGS + 1))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(
            String::from_utf8_lossy(&body).contains(&MAX_TXN_LEGS.to_string()),
            "the refusal must name the bound: {}",
            String::from_utf8_lossy(&body)
        );

        // The differential: exactly the bound is *not* refused on the count. Whatever the gateway then
        // answers for those legs, it must not be this refusal — otherwise the test above would pass on
        // a handler that refused every list at all.
        let response = api_txn_run(
            State(txn_state(Some(gateway().await), true)),
            Json(txn_body("7474", legs(MAX_TXN_LEGS))),
        )
        .await;
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(
            !String::from_utf8_lossy(&body).contains("at most"),
            "the bound itself must reach the gateway, not the refusal: {}",
            String::from_utf8_lossy(&body)
        );
    }

    /// **N4/MORE — the spend route shares the produce route's one credentialless gate.** The admin
    /// router's only caller check is `admin_origin_guard` (`web::http`, the layer at the bottom of
    /// [`admin_router`]), which reads no identity and admits any request carrying no `Origin` (CSRF
    /// requires a browser). `/api/txn` signs every phase deploy with the node's validator key and
    /// escrows REV from that key's own vault, so a caller that reaches the listener past the bind and
    /// sends no `Origin` is admitted to a *spend* on the same terms as a `propose` — the guard does
    /// not scale with the authority behind it.
    ///
    /// Fires when the origin-less POST reaches the spend handler (a `400` from the handler, not the
    /// guard's `403`). Refuted only if the guard refused a credentialless caller to the spend route
    /// while admitting the same caller to propose (one identity for one authority).
    #[tokio::test]
    async fn the_spend_route_shares_the_produce_routes_credentialless_gate() {
        use tower::ServiceExt;

        let body = || axum::body::Body::from(r#"{"txnId":"aabb","legs":[]}"#);

        // The one thing the guard refuses: a cross-origin request. It is refused on the spend route
        // exactly as on propose, which is what makes the arm below a differential rather than a
        // guard that waves everything through.
        let cross_origin = axum::http::Request::builder()
            .method("POST")
            .uri("/api/txn")
            .header("origin", "http://evil.example")
            .header("host", "127.0.0.1:40404")
            .header("content-type", "application/json")
            .body(body())
            .unwrap();
        let refused = admin_router(txn_state(Some(gateway().await), true))
            .oneshot(cross_origin)
            .await
            .expect("a response");
        assert_eq!(
            refused.status(),
            StatusCode::FORBIDDEN,
            "a cross-origin POST must be refused before any handler runs"
        );

        // A no-Origin, no-credential caller is admitted past the guard to the *spend* handler, which
        // reads no identity either: the answer is the handler's own refusal, not the guard's.
        let credentialless = axum::http::Request::builder()
            .method("POST")
            .uri("/api/txn")
            .header("content-type", "application/json")
            .body(body())
            .unwrap();
        let admitted = admin_router(txn_state(Some(gateway().await), true))
            .oneshot(credentialless)
            .await
            .expect("a response");
        assert_ne!(
            admitted.status(),
            StatusCode::FORBIDDEN,
            "the guard admits a credentialless caller to the route that spends the validator's REV"
        );
        assert_eq!(
            admitted.status(),
            StatusCode::BAD_REQUEST,
            "the response is the handler's (empty leg list), so the spend handler was reached"
        );
    }

    #[tokio::test]
    async fn api_txn_run_rejects_a_malformed_transaction_id() {
        for bad in ["", "zz", "abc", &"aa".repeat(65)] {
            let response = api_txn_run(
                State(txn_state(Some(gateway().await), true)),
                Json(txn_body(bad, serde_json::json!([]))),
            )
            .await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "txnId {bad:?}");
        }
    }

    #[tokio::test]
    async fn api_txn_run_rejects_an_invalid_leg_shard_or_an_empty_leg_list() {
        let response = api_txn_run(
            State(txn_state(Some(gateway().await), true)),
            Json(txn_body(
                "aabb",
                serde_json::json!([{ "shardId": "", "amount": 1, "to": "d" }]),
            )),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("invalid shardId"));

        let response = api_txn_run(
            State(txn_state(Some(gateway().await), true)),
            Json(txn_body("aabb", serde_json::json!([]))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let err = String::from_utf8_lossy(&body);
        assert!(err.contains("at least one leg"), "{err}");
    }

    /// A leg on a shard this node does not validate is a caller error, and the membership is
    /// reported back.
    #[tokio::test]
    async fn api_txn_run_reports_a_non_member_shard() {
        let response = api_txn_run(
            State(txn_state(Some(gateway().await), true)),
            Json(txn_body(
                "aabb",
                serde_json::json!([{ "shardId": "/elsewhere", "amount": 1, "to": "d" }]),
            )),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let err = String::from_utf8_lossy(&body);
        assert!(err.contains("/elsewhere"), "{err}");
        assert!(err.contains("/root, /root/child"), "{err}");
    }

    #[tokio::test]
    async fn api_txn_status_400_non_hex_and_404_unknown_and_500_unreadable() {
        let g = gateway().await;
        assert_eq!(
            api_txn_status(
                State(txn_state(Some(g.clone()), true)),
                Path("zz".to_string())
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        // A well-formed id this node has never seen.
        assert_eq!(
            api_txn_status(State(txn_state(Some(g), true)), Path("aabb".to_string()))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );

        // A ledger that cannot be read is a server error, not a 404.
        let unreadable = gateway_over(Arc::new(UnreadableStoreManager)).await;
        assert_eq!(
            api_txn_status(
                State(txn_state(Some(unreadable), true)),
                Path("aabb".to_string())
            )
            .await
            .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[tokio::test]
    async fn api_txn_list_is_empty_for_a_fresh_gateway() {
        let response = api_txn_list(State(txn_state(Some(gateway().await), true))).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["inFlight"].as_array().unwrap().len(), 0);
    }
}
