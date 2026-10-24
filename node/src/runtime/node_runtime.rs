//! Node runtime assembly (port of `runtime/Setup.scala` + `runtime/NodeRuntime.scala`).
//!
//! Assembles the store manager → RSpace → RhoRuntime → RuntimeManager → BlockApiImpl →
//! GrpcServices/WebApi/AdminWebApi chain and serves it over gRPC + HTTP, including the
//! comm/transport/discovery layer, the proposer, the block receiver/processor streams, the
//! NodeLaunch state machines, and the report-store codec.

use rchain_shared::chan;
use rchain_shared::lock::Unpoison;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use prost::Message;
use tokio::sync::mpsc;
use tokio::sync::watch;

use rchain_block_storage::approved_store::{self, ApprovedStore};
use rchain_block_storage::block_store::{self, BlockStore};
use rchain_block_storage::dag::codecs::{
    BlockHashCodec, BlockMetadataCodec, SignedDeployDataCodec,
};
use rchain_block_storage::dag::dag_storage::{BlockDagStorage, DeployId};
use rchain_casper::api::block_api::{BlockApi, ProposeHealth, ProposerHealth};
use rchain_casper::api::block_api_impl::{
    BlockApiImpl, NetworkStatus, NetworkStatusFn, ProposeFunction,
};
use rchain_casper::api::block_report_api::BlockReportApi;
use rchain_casper::block_metadata_store::BlockMetadataStore;
use rchain_casper::block_random_seed::BlockRandomSeed;
use rchain_casper::blocks::block_processor;
use rchain_casper::blocks::block_receiver::{self, BlockReceiverState};
use rchain_casper::blocks::block_retriever::BlockRetriever;
use rchain_casper::blocks::proposer::proposer::{ProposeSource, Proposer, ProposerResult};
use rchain_casper::conf::ShardSpec;
use rchain_casper::dag::BlockDagKeyValueStorage;
use rchain_casper::engine::node_launch::{self, PeerMessage};
use rchain_casper::interpreter_util::ShardHealth;
use rchain_casper::merging::BlockIndex;
use rchain_casper::protocol::comm_util::{CommUtil, ConnectionsCell};
use rchain_casper::reporting::{rho_reporter, ReportingCasper};
use rchain_casper::runtime_manager::RuntimeManager;
use rchain_casper::state::ProposerState;
use rchain_casper::storage::{
    rnode_key_value_store_manager, shard_data_dir, RESET_MARKER_DB, SHARD_ID_MARKER,
    STORE_FORMAT_MARKER, STORE_FORMAT_VERSION,
};
use rchain_casper::validator_identity::ValidatorIdentity;
use rchain_comm::discovery::grpc_kademlia_rpc::GrpcKademliaRpc;
use rchain_comm::discovery::grpc_kademlia_rpc_server::{
    serve as kademlia_serve, GrpcKademliaRpcServer,
};
use rchain_comm::discovery::kademlia_handle_rpc::{handle_lookup, handle_ping};
use rchain_comm::discovery::kademlia_store::table as kademlia_table;
use rchain_comm::discovery::node_discovery::KademliaNodeDiscovery;
use rchain_comm::discovery::{KademliaRpc, NodeDiscovery};
use rchain_comm::peer_node::{NodeIdentifier, PeerNode};
use rchain_comm::rp::connect::{add_conn, clear_connections, find_and_connect, remove_conn};
use rchain_comm::rp::handle_messages::{self, RoutingMessage};
use rchain_comm::rp::rp_conf::{ClearConnectionsConf, RPConf};
use rchain_comm::transport::chunker::Blob;
use rchain_comm::transport::communication_response::CommunicationResponse;
use rchain_comm::transport::grpc_transport_client::GrpcTransportClient;
use rchain_comm::transport::grpc_transport_receiver::BoxFuture;
use rchain_comm::transport::grpc_transport_server::TransportLayerServer;
use rchain_comm::transport::transport_layer::TransportLayer;
use rchain_comm::who_am_i;
use rchain_crypto::private_key::PrivateKey;
use rchain_models::block_hash::BlockHash;
use rchain_models::block_metadata::BlockMetadata;
use rchain_models::casper::protocol::casper_message::{
    BlockMessage, CasperMessage, SignedDeployData,
};
use rchain_models::casper::protocol::casper_message_protocol::to_casper_message_proto;
use rchain_models::casper::protocol::report::BlockEventInfo;
use rchain_models::comm::protocol::Protocol;
use rchain_models::runtime::{BindPattern, ListParWithRandom, TaggedContinuation};
use rchain_models::sorted::SortedProc;
use rchain_rholang::merging::{DeployMergeableDataCodec, NativeSidecarCodec};
use rchain_rholang::reporting_runtime::create_reporting_rspace;
use rchain_rholang::runtime::{ReplayRhoRuntime, RhoRuntime};
use rchain_rholang::scheduler::EffectMode;
use rchain_rholang::storage::RhoMatch;
use rchain_rspace::factory::create_history_repository;
use rchain_rspace::hot_store::InMemHotStore;
use rchain_rspace::rspace::RSpace;
use rchain_rspace::state::instances::{RSpaceExporterStore, RSpaceImporterStore};
use rchain_shared::base16;
use rchain_shared::lmdb::LmdbDirStoreManager;
use rchain_shared::log::{Log, LogSource};
use rchain_shared::metrics::{Metrics, Source};
use rchain_shared::refined::{Port, ShardId};
use rchain_shared::store_manager::database;
use rchain_shared::typed_store::{BytesCodec, Codec, KeyValueTypedStore};

use crate::api::admin_web_api::AdminWebApi;
use crate::api::admin_web_api_impl::AdminWebApiImpl;
use crate::api::grpc::{serve_deploy, serve_internal, GrpcServices};
use crate::api::ocapn::{serve_ocapn, ChainCapability, OcapnDialSlot, OcapnListeners};
use crate::api::shard_routing::ShardRoutingBlockApi;
use crate::api::web_api::WebApi;
use crate::api::web_api_impl::WebApiImpl;
use crate::configuration::model::NodeConf;
use crate::diagnostics::effects::MetricsRegistry;
use crate::diagnostics::NewPrometheusReporter;
use crate::instances::proposer_instance;
use crate::resources::ResourceGuard;
use crate::runtime::shutdown::{stop_requested, SHUTDOWN_DRAIN_TIMEOUT};
use crate::web::http::{
    acquire_admin_http_server, acquire_http_server, ShardRegistry, StatusProvider,
};
use crate::web::pos_read::{PosReadApi, ShardPosRead};
use crate::web::transaction::TransactionAPIImpl;
use rchain_casper::gateway::ledger::TxnLedger;
use rchain_casper::gateway::{GatewayTxn, LocalShard, LocalShardDeployService};
use rchain_ocapn::conn::Export;

/// Interval between `--autopropose` timer ticks. Together with the dev-mode dummy deploy this makes a
/// fresh devnet produce blocks on its own (a lone validator has no peer/deploy to kick the
/// event-driven propose, so a timer is the missing trigger).
///
/// **The growth this produces is unbounded, and that is a decision recorded in `docs/src/node/devnet.md`
/// ("Growth, and the measurement volumes"), not an oversight** (2026-09-24). Every tick mints a block with
/// no ceiling: a devnet left running grows ~1,800 blocks an hour, and one reached 5,844 blocks — the chain
/// that made the DAG's costs visible and through which C55/C56 were found. Capping it would change the
/// instrument (on a local testnet the chain length *is* the variable), so the decision is to document it
/// and expose the cost: `/metrics` carries the DAG's four gauges — `rchain_dag_messages`,
/// `rchain_dag_seen_entries`, `rchain_dag_index_entries` and `rchain_dag_logical_bytes` (a fifth,
/// `rchain_dag_fringe_states`, retired with the `fringe-data` store; Law 66/68, C250's residue,
/// C270) — and that doc also states the measurement volumes' disposition.
const AUTOPROPOSE_INTERVAL: Duration = Duration::from_secs(2);

/// After this many consecutive self-validation failures the autopropose **timer** halts, so a node with
/// inconsistent state accounting stops proposing instead of silently spinning on `BugError`.
///
/// **Three things about this constant that a reader should not have to infer (#157).**
///
/// - **What stops is the timer, and only the timer.** Three paths feed the proposer queue — the
///   autopropose tap above, the attest-on-new-blocks tap below, and the admin `POST /api/propose` — and
///   each of them keeps running. So a halted node is *not* a wedged one: it still proposes on inbound
///   blocks and on demand, and because the proposer stores `0` on a success it can even clear the
///   counter while the timer stays stopped. That is why the API and `/metrics` report
///   `autopropose_timer_halted` rather than a bare `halted` — the flag would otherwise claim the whole
///   node had stopped when one of its three triggers had.
/// - **Nothing restarts it.** The task `break`s and no `JoinHandle` is kept, so the halt lasts until the
///   process does. That is a decision rather than an oversight: a resume path that quietly restarts a
///   proposer whose state accounting is inconsistent is the failure this constant exists to prevent.
/// - **It is observable as of #157.** Before that the halt was one ERROR line, and on a chain whose
///   steady state is a node that produces nothing until a deploy arrives, "quiet" and "broken" looked
///   identical to every API client — which is also why #148's probe could not be read without a witness
///   that says which of the two a node is in.
const AUTOPROPOSE_MAX_CONSECUTIVE_FAILURES: u64 = 3;

/// Where the **admin** HTTP server binds (AUDIT C112).
///
/// Loopback unless the operator opted in. That server carries an unauthenticated `POST /api/propose`
/// — it triggers block production — and it used to bind `api-server.host` unconditionally, which
/// defaults to `0.0.0.0`. CORS was the only thing in front of it and CORS is not authentication: a
/// non-browser client ignores it, so on a published port any host on the network could make the node
/// propose. The internal gRPC propose service was already loopback-bound, so this makes the two
/// propose surfaces agree.
///
/// Extracted from the spawn so the choice is testable: as an inline `if` inside an `async move` block
/// the only way to observe it was to start a server and connect to it, which is why the public bind
/// survived as long as it did.
fn admin_bind_host(public_host: &str, enable_devnet_admin_public: bool) -> String {
    if enable_devnet_admin_public {
        public_host.to_string()
    } else {
        "127.0.0.1".to_string()
    }
}

/// Build the real block-reporting casper: each `trace` constructs a fresh, isolated reporting
/// `ReplayRSpace` over the persistent store (the factory clones the store manager, which shares the
/// underlying LMDB environments).
fn reporting_casper(
    store_manager: &LmdbDirStoreManager,
    shard_id: &str,
    dag: Arc<dyn BlockDagStorage>,
) -> Result<impl ReportingCasper, String> {
    let store_manager = store_manager.clone();
    let mergeable_tag_name = SortedProc::new(
        BlockRandomSeed::non_negative_mergeable_tag_name(shard_id).map_err(|e| e.to_string())?,
    );
    Ok(rho_reporter(
        move || {
            let manager = store_manager.clone();
            async move { create_reporting_rspace(&manager).await }
        },
        mergeable_tag_name,
        // A reporter replays from a block message and has no DAG of its own, so the close deploy's two
        // derived inputs are supplied here: the fringe state hash a boundary block anchored its
        // successor's seed to (from the metadata validation stored for that block, which is the
        // recomputed value), and the participation its absence rule reads (derived from the DAG, by the
        // same function the validator uses).
        move |block: BlockMessage| {
            let dag = dag.clone();
            async move {
                let fringe_state_hash = fringe_state_of(&*dag, &block.block_hash).await?;
                let participation =
                    rchain_casper::multi_parent_casper::participation_for_block(&*dag, &block)
                        .await?;
                Ok((fringe_state_hash, participation))
            }
        },
    ))
}

/// The state hash of the last finalised fringe as of a block — the value that block's close system
/// deploy anchored the next epoch's active-set seed to.
///
/// Read from the DAG's block metadata, which for a validated block is built with
/// `pre_state.fringe_state`: exactly the value the block's own play and validation used, so a replay
/// driven by it reaches the same seed leaf. A missing metadata is an error rather than a default,
/// because a zero would replay a boundary block to a different state.
async fn fringe_state_of(
    dag: &dyn BlockDagStorage,
    hash: &BlockHash,
) -> Result<rchain_crypto::hash::blake2b256_hash::Blake2b256Hash, String> {
    dag.lookup(hash)
        .await
        .map_err(|e| e.to_string())?
        .map(|m| {
            rchain_crypto::hash::blake2b256_hash::Blake2b256Hash::from_byte_array(
                m.fringe_state_hash.as_bytes(),
            )
        })
        .ok_or_else(|| format!("no block metadata for {}: cannot index it", hash.to_hex()))
}

/// The `BlockEventInfo` report-store codec (prost wire round-trip).
struct BlockEventInfoCodec;

impl Codec<BlockEventInfo> for BlockEventInfoCodec {
    fn encode(&self, value: &BlockEventInfo) -> Vec<u8> {
        crate::api::grpc::tonic::block_event_info_to_wire(value).encode_to_vec()
    }
    fn decode(&self, bytes: &[u8]) -> Result<BlockEventInfo, String> {
        let wire = <rchain_models::proto::casper::BlockEventInfo as prost::Message>::decode(bytes)
            .map_err(|e| e.to_string())?;
        crate::api::grpc::tonic::block_event_info_from_wire(&wire)
    }
}

/// The assembled comm/discovery state (port of the `NodeRuntime` transport/comm-state setup).
pub struct CommState {
    pub transport: Arc<dyn TransportLayer>,
    pub connections: ConnectionsCell,
    pub rp_conf: RPConf,
    pub comm_util: Arc<CommUtil>,
    pub block_retriever: Arc<BlockRetriever>,
    pub local_peer: PeerNode,
    pub discovery: Arc<dyn NodeDiscovery>,
}

/// Create the comm/discovery state (port of `NodeRuntime.main`'s transport + comm-state setup).
pub async fn create_comm_state(
    conf: &NodeConf,
    id: &NodeIdentifier,
    log: Arc<dyn Log>,
) -> Result<CommState, String> {
    let source = LogSource::new("coop.rchain.node.runtime.NodeRuntime");

    // Fetch the local peer node (blocking external-IP/UPnP discovery at startup).
    let protocol_port = Port::try_from(conf.protocol_server.port).map_err(|e| e.to_string())?;
    let discovery_port = Port::try_from(conf.peers_discovery.port).map_err(|e| e.to_string())?;
    let mut log_buffer = Vec::new();
    let local_peer = who_am_i::fetch_local_peer_node(
        conf.protocol_server.host.clone(),
        protocol_port,
        discovery_port,
        conf.protocol_server.no_upnp,
        id.clone(),
        &mut |msg| log_buffer.push(msg),
    );
    for msg in log_buffer {
        log.info(source, &msg);
    }

    // Transport client (mutual TLS).
    let cert = std::fs::read_to_string(&conf.tls.certificate_path).map_err(|e| e.to_string())?;
    let key = std::fs::read_to_string(&conf.tls.key_path).map_err(|e| e.to_string())?;
    let transport: Arc<dyn TransportLayer> = Arc::new(GrpcTransportClient::new(
        conf.protocol_client.network_id.clone(),
        &cert,
        &key,
        usize::try_from(conf.protocol_client.grpc_max_recv_message_size)
            .map_err(|e| e.to_string())?,
        usize::try_from(conf.protocol_client.grpc_stream_chunk_size).map_err(|e| e.to_string())?,
        100,
    )?);

    // Comm state (connections cell + RPConf).
    let connections: ConnectionsCell = Arc::new(tokio::sync::RwLock::new(Vec::new()));
    let bootstrap = if conf.standalone {
        None
    } else {
        Some(conf.protocol_client.bootstrap.clone())
    };
    let rp_conf = RPConf {
        local: local_peer.clone(),
        network_id: conf.protocol_client.network_id.clone(),
        bootstrap,
        default_timeout: conf.protocol_client.network_timeout,
        max_num_of_connections: usize::try_from(conf.protocol_client.batch_max_connections)
            .map_err(|e| e.to_string())?,
        clear_connections: ClearConnectionsConf {
            num_of_connections_pinged: usize::try_from(conf.peers_discovery.heartbeat_batch_size)
                .map_err(|e| e.to_string())?,
        },
    };

    let comm_util = Arc::new(CommUtil::new(
        transport.clone(),
        rp_conf.clone(),
        connections.clone(),
        log.clone(),
    ));
    let block_retriever = Arc::new(BlockRetriever::new(comm_util.clone(), log.clone()));

    // Kademlia discovery: routing-table store + gRPC RPC client + RPC server + iterative loop.
    let kademlia_store = kademlia_table(id);
    // Kademlia runs over the same mutual TLS as the transport (AUDIT C116): the certificate is the
    // node's identity on both, so discovery cannot be a way in that the transport is not.
    let kademlia_rpc: Arc<dyn KademliaRpc> = Arc::new(
        GrpcKademliaRpc::new(
            local_peer.clone(),
            conf.protocol_client.network_id.clone(),
            conf.protocol_client.network_timeout,
            &cert,
            &key,
        )
        .map_err(|e| format!("kademlia client TLS: {e}"))?,
    );

    let discovery_addr: std::net::SocketAddr = format!("0.0.0.0:{}", u16::from(discovery_port))
        .parse::<std::net::SocketAddr>()
        .map_err(|e| e.to_string())?;
    {
        let store_ping = kademlia_store.clone();
        let store_lookup = kademlia_store.clone();
        let ping_handler = move |peer: PeerNode| -> BoxFuture<()> {
            let store = store_ping.clone();
            Box::pin(async move { handle_ping(store.as_ref(), peer) })
        };
        let lookup_handler = move |peer: PeerNode, key: Vec<u8>| -> BoxFuture<Vec<PeerNode>> {
            let store = store_lookup.clone();
            Box::pin(async move { handle_lookup(store.as_ref(), peer, &key) })
        };
        let server = GrpcKademliaRpcServer::new(
            conf.protocol_client.network_id.clone(),
            ping_handler,
            lookup_handler,
        );
        let kademlia_tls =
            rchain_comm::transport::hostname_trust_manager::server_config(&cert, &key)
                .map_err(|e| format!("kademlia server TLS: {e}"))?;
        // The same cap the peer transport honours: the discovery service is reached by the same
        // certificate-bearing peers over the same accept path, so it is bounded by the same operator
        // key (W-N1-01). Read here, not in the task, so the future captures a `usize` and not `conf`.
        let kademlia_max_recv =
            usize::try_from(conf.protocol_server.grpc_max_recv_message_size).unwrap_or(262144);
        rchain_shared::supervise::spawn_supervised("kademlia-serve", async move {
            if let Err(e) =
                kademlia_serve(discovery_addr, server, kademlia_max_recv, kademlia_tls).await
            {
                log.error(source, &format!("Kademlia RPC server failed: {e}"));
            }
        });
    }

    // Seed the bootstrap peer into the routing table.
    if let Some(bootstrap) = &rp_conf.bootstrap {
        kademlia_store.update_last_seen(bootstrap.clone());
    }

    let discovery: Arc<dyn NodeDiscovery> = Arc::new(KademliaNodeDiscovery::new(
        id.clone(),
        kademlia_store,
        kademlia_rpc,
    ));

    // Periodic discovery + connect loop: discover peers, then connect to the newly-found ones.
    {
        let discovery = discovery.clone();
        let transport = transport.clone();
        let rp_conf = rp_conf.clone();
        let connections = connections.clone();
        let interval = conf.peers_discovery.lookup_interval;
        rchain_shared::supervise::spawn_supervised("peer-discovery", async move {
            loop {
                discovery.discover().await;
                let current = connections.read().await.clone();
                let new_peers =
                    find_and_connect(discovery.as_ref(), &rp_conf, transport.as_ref(), &current)
                        .await;
                if !new_peers.is_empty() {
                    let mut guard = connections.write().await;
                    *guard = add_conn(&guard, &new_peers);
                }
                tokio::time::sleep(interval).await;
            }
        });
    }

    // Periodic clear-connections loop: ping the oldest peers and drop non-responders. The pings run
    // over a snapshot; the mutation applies to the current connections under a brief write lock.
    {
        let transport = transport.clone();
        let rp_conf = rp_conf.clone();
        let connections = connections.clone();
        let interval = conf.peers_discovery.cleanup_interval;
        rchain_shared::supervise::spawn_supervised("clear-connections", async move {
            loop {
                let snapshot = connections.read().await.clone();
                let (to_ping, successful, _failed) =
                    clear_connections(transport.as_ref(), &rp_conf, &snapshot).await;
                {
                    let mut guard = connections.write().await;
                    let rest = remove_conn(&guard, &to_ping);
                    *guard = add_conn(&rest, &successful);
                }
                tokio::time::sleep(interval).await;
            }
        });
    }

    Ok(CommState {
        transport,
        connections,
        rp_conf,
        comm_util,
        block_retriever,
        local_peer,
        discovery,
    })
}

/// The transport (protocol) server and its inbound-message dispatch closures (port of
/// `NetworkServers.protocolServer`).
pub struct ProtocolServer {
    server: TransportLayerServer,
    dispatch: Box<dyn Fn(Protocol) -> BoxFuture<CommunicationResponse> + Send + Sync>,
    handle_streamed: Box<dyn Fn(Blob) -> BoxFuture<()> + Send + Sync>,
}

/// The assembled node program (port of the `setupNodeProgram` result).
pub struct NodeProgram {
    grpc_services: GrpcServices,
    web_api: Arc<dyn WebApi>,
    /// The PoS read surface (`GET /api/v1/pos`, AUDIT C148).
    pos_read: Arc<dyn PosReadApi>,
    admin_web_api: Arc<dyn AdminWebApi>,
    /// The shards this node validates for, for the `GET /api/v1/shards` route.
    shards: Arc<ShardRegistry>,
    /// The on-node cross-shard 2PC coordinator, when this node is a multi-shard gateway.
    gateway: Option<Arc<GatewayTxn>>,
    block_report_api: Arc<BlockReportApi>,
    reporter: Arc<NewPrometheusReporter>,
    metrics: Arc<MetricsRegistry>,
    host: String,
    port_http: Port,
    port_admin_http: Port,
    port_grpc_external: Port,
    port_grpc_internal: Port,
    grpc_max_recv_message_size: usize,
    max_connection_idle: Duration,
    enable_reporting: bool,
    // Whether the faucet routes are mounted (AUDIT R34): dev mode and a deployer key, resolved where
    // both are known and passed through to `acquire_http_server`.
    faucet_enabled: bool,
    enable_txn_api: bool,
    enable_devnet_cors: bool,
    enable_devnet_admin_public: bool,
    /// The transports to bind the OCapN listener on — `tcp-testing-only` and/or `unix`, or neither
    /// for a dial-only node (issue #249).
    ocapn_listeners: OcapnListeners,
    /// The node's Noise identity, when it has one — required exactly when `ocapn_listeners.noise` is
    /// set, and also what makes the designator a name a peer can verify (issue #249).
    ocapn_identity: Option<rchain_ocapn::noise::NoiseIdentity>,
    /// Where the listener publishes the [`OcapnDialSlot`]'s dialer, and the admin server reads it to
    /// serve a node-started dial (issue #249).
    ocapn_dial: OcapnDialSlot,
    /// Whether the node-started dial route is mounted (`api-server.enable-ocapn-dial`).
    enable_ocapn_dial: bool,
    /// Refuse to dial loopback and private addresses on a peer's word (HAZOP row B4).
    ocapn_deny_local_dial: bool,
    /// The name this node advertises in every session (C224 item 2).
    ocapn_designator: String,
    /// The host peers are told to dial, when the bound address is not one they can reach (HAZOP row
    /// C237). `None` means "the bound address", which `serve_ocapn` refuses for `0.0.0.0`/`::`.
    ocapn_advertised_host: Option<String>,
    /// The node's log, for the one surface that had none: the OCapN listener served, refused and
    /// spent in silence, so an operator could not tell "nobody is calling" from "calls are failing"
    /// (HAZOP row E6).
    log: Arc<dyn Log>,
    /// The chain-backed capability the listener publishes, or `None` when there is no key.
    /// The chain-backed capabilities the OCapN bridge publishes, each under its swiss number.
    ocapn_chain: Vec<(Vec<u8>, Arc<dyn Export>)>,
    protocol_server: Option<ProtocolServer>,
    status_provider: Option<StatusProvider>,
    /// The host-resource watchdog's sampled snapshot (A1). The status surfaces read it, and the
    /// ingress gates (B34) evaluate their admission floor against it — one signal, so the answer a
    /// client is refused with and the state an operator sees cannot come from two readings.
    resources: Arc<ResourceGuard>,
    /// The primary shard's runtime manager. Exposed so an integration test can read the native
    /// state the running node actually installed (the developer-allocation leaf, the PoS params)
    /// without a second reader against the node's LMDB directory while it holds it.
    pub primary_runtime: Arc<RuntimeManager>,
}

impl NodeProgram {
    /// Serve the gRPC + HTTP + protocol servers (port of `NetworkServers.create`).
    ///
    /// `stop` is the operator's word (AUDIT C144): every listener is handed a receiver, so the same
    /// event stops them all, and this returns once they have finished draining — or once
    /// [`SHUTDOWN_DRAIN_TIMEOUT`] has passed, whichever comes first.
    pub async fn serve(self, stop: watch::Receiver<bool>) -> Result<(), String> {
        let NodeProgram {
            grpc_services,
            web_api,
            pos_read,
            admin_web_api,
            faucet_enabled,
            shards,
            block_report_api,
            reporter,
            metrics,
            host,
            port_http,
            port_admin_http,
            port_grpc_external,
            port_grpc_internal,
            grpc_max_recv_message_size,
            max_connection_idle,
            enable_reporting,
            enable_txn_api,
            enable_devnet_cors,
            enable_devnet_admin_public,
            ocapn_listeners,
            ocapn_identity,
            ocapn_dial,
            enable_ocapn_dial,
            ocapn_deny_local_dial,
            ocapn_designator,
            ocapn_advertised_host,
            log,
            ocapn_chain,
            protocol_server,
            status_provider,
            gateway,
            resources,
            primary_runtime: _,
        } = self;

        let GrpcServices {
            deploy,
            propose,
            repl,
        } = grpc_services;

        let grpc_external_addr: std::net::SocketAddr =
            format!("{}:{}", host, u16::from(port_grpc_external))
                .parse::<std::net::SocketAddr>()
                .map_err(|e| e.to_string())?;
        // The internal (propose + repl) server binds to loopback only (documented deviation from
        // Scala's `0.0.0.0` bind) so the unauthenticated propose/repl endpoints are not reachable
        // from the network.
        let grpc_internal_addr: std::net::SocketAddr =
            format!("127.0.0.1:{}", u16::from(port_grpc_internal))
                .parse::<std::net::SocketAddr>()
                .map_err(|e| e.to_string())?;

        let mut grpc_external = tokio::spawn(serve_deploy(
            deploy,
            grpc_external_addr,
            grpc_max_recv_message_size,
            crate::resources::PressureGate::new(
                resources.clone(),
                crate::resources::AdmissionPolicy::default(),
            ),
            stop.clone(),
        ));
        let mut grpc_internal = tokio::spawn(serve_internal(
            propose,
            repl,
            grpc_internal_addr,
            grpc_max_recv_message_size,
            stop.clone(),
        ));

        let mut http = tokio::spawn({
            let host = host.clone();
            let stop = stop.clone();
            // **Cloned before the closure, because the admin listener needs the same read.** The
            // delegations route moved to the admin port (W-N3-03) and takes the PoS read with it; an
            // `async move` that captures it here would take it from there.
            let pos_read = pos_read.clone();
            async move {
                // No gateway here: the cross-shard transaction routes are on the admin listener, which
                // is loopback by default (AUDIT C121), and `HttpState` no longer carries the capability.
                acquire_http_server(
                    &host,
                    port_http,
                    reporter,
                    metrics,
                    web_api,
                    block_report_api,
                    shards,
                    status_provider,
                    pos_read.clone(),
                    resources.clone(),
                    max_connection_idle,
                    enable_reporting,
                    faucet_enabled,
                    stop,
                )
                .await
            }
        });

        let mut admin = tokio::spawn({
            let host = host.clone();
            let stop = stop.clone();
            // The listener publishes the dialer through this slot; the admin route reads it. Cloned
            // here so the listener task below can still take the slot by value.
            let ocapn_dial = ocapn_dial.clone();
            async move {
                // The admin HTTP server hosts the **unauthenticated** `/api/propose`, which triggers
                // block production. It used to bind `api-server.host` — `0.0.0.0` — unconditionally,
                // "so a browser wallet can reach it through a published port"; CORS was the only
                // thing in front of it, and CORS is not authentication (a non-browser client ignores
                // it entirely). Any host on the network could make the node propose (AUDIT C112).
                //
                // So the default is loopback, which is where the *other* propose surface already
                // lives: the internal gRPC service binds `127.0.0.1:{port_grpc_internal}`
                // (`grpc/mod.rs::serve_internal`). A wallet that genuinely needs a published admin
                // port asks for it with `api-server.enable-devnet-admin-public`, the same
                // ask-for-it shape as `enable_devnet_cors` above — the capability is preserved, the
                // default is not.
                let admin_host = admin_bind_host(&host, enable_devnet_admin_public);
                acquire_admin_http_server(
                    &admin_host,
                    port_admin_http,
                    admin_web_api,
                    enable_devnet_cors,
                    gateway,
                    enable_txn_api,
                    ocapn_dial,
                    enable_ocapn_dial,
                    pos_read.clone(),
                    max_connection_idle,
                    stop,
                )
                .await
            }
        });

        // The OCapN listener (issue #249). Spawned even when it is not configured — see
        // `serve_ocapn` — so its select arm and drain slot are unconditional.
        let mut ocapn = tokio::spawn(serve_ocapn(
            ocapn_listeners,
            ocapn_chain,
            ocapn_designator,
            ocapn_deny_local_dial,
            ocapn_dial,
            ocapn_identity,
            ocapn_advertised_host,
            log.clone(),
            stop.clone(),
        ));

        // **The first moment a node can say the expensive part is over, and the expensive part is
        // replay rather than the bind** (issue #60's observability half). Every costly step — the store
        // rebuilds, the metadata and DAG indices, the block-index replay — completes *before* this
        // function is called, so reaching here is the signal an operator greps for when a node looks
        // hung on a long chain. It deliberately does not claim the API is answering: a bind failure is
        // reported by `listener_stopped` below (AUDIT C142), and a line here claiming more than this
        // could become a lie.
        eprintln!("replay complete — starting listeners; the API answers once they are up");

        // Every one of these is an accept loop: it returns only when something has already gone
        // wrong (a failed bind, a panicking task, a listener closing), so the **first** completion is
        // the fact the operator needs. `join!` waited for all five instead, and since four of them
        // never return, a listener whose bind failed — its task ending with `Err` at once — left the
        // join pending forever while the node ran with a dead component and logged nothing at all
        // (AUDIT C142). `select!` reports the first exit, and `listener_stopped` names it.
        //
        // The last arm is the operator's stop (AUDIT C144): the listeners were handed the same word
        // through `stop`, so `None` here means "every one of them is draining", not "something went
        // wrong". It is also the **bound**: without it this wait could only end when a listener
        // returned, and a listener that never finishes draining would hold the process open forever
        // — the termination grace this path exists to stop paying. The protocol listener is not among
        // the drained ones: the transport's `serve` has no shutdown path to hand it (its
        // `grpc_transport_receiver` is the layer that would need one, AUDIT C144's residue), so it is
        // the one this arm is really for.
        let stopping = stop.clone();
        // **Which listeners the `select!` has already seen finish**, so the drain below does not poll
        // them a second time. A `tokio::task::JoinHandle` **panics** when it is polled after returning
        // `Ready` ("JoinHandle polled after completion"), and those arms do exactly that — they poll
        // `&mut handle` and take the output. The race is real rather than theoretical: the listeners
        // are handed the *same* stop word, so one of them can drain and finish **before**
        // `stop_requested` resolves and wins the select, and the drain would then await the handle the
        // arm already consumed. `node/tests/shutdown.rs` asserts the serve task must not panic, which
        // is how this surfaced (CI, 2026-09-28); it does not reproduce under a light local load,
        // because there `stop_requested` wins.
        let mut drained = [false; 5];
        let stopped = if let Some(protocol) = protocol_server {
            let mut protocol = tokio::spawn(async move {
                protocol
                    .server
                    .serve(protocol.dispatch, protocol.handle_streamed)
                    .await
            });
            tokio::select! {
                r = &mut grpc_external => { drained[0] = true; Some(listener_stopped("deploy gRPC listener", r, *stopping.borrow())) },
                r = &mut grpc_internal => { drained[1] = true; Some(listener_stopped("internal gRPC listener", r, *stopping.borrow())) },
                r = &mut http => { drained[2] = true; Some(listener_stopped("HTTP listener", r, *stopping.borrow())) },
                r = &mut admin => { drained[3] = true; Some(listener_stopped("admin HTTP listener", r, *stopping.borrow())) },
                r = &mut ocapn => { drained[4] = true; Some(listener_stopped("OCapN listener", r, *stopping.borrow())) },
                r = &mut protocol => Some(listener_stopped("protocol listener", r, *stopping.borrow())),
                _ = stop_requested(stop) => None,
            }
        } else {
            tokio::select! {
                r = &mut grpc_external => { drained[0] = true; Some(listener_stopped("deploy gRPC listener", r, *stopping.borrow())) },
                r = &mut grpc_internal => { drained[1] = true; Some(listener_stopped("internal gRPC listener", r, *stopping.borrow())) },
                r = &mut http => { drained[2] = true; Some(listener_stopped("HTTP listener", r, *stopping.borrow())) },
                r = &mut admin => { drained[3] = true; Some(listener_stopped("admin HTTP listener", r, *stopping.borrow())) },
                r = &mut ocapn => { drained[4] = true; Some(listener_stopped("OCapN listener", r, *stopping.borrow())) },
                _ = stop_requested(stop) => None,
            }
        };
        // Both `None` (the operator's word arrived first) and `Some(Ok(()))` (a listener drained
        // because of it) mean the same thing here: stop waiting for a fault and wait out the drain.
        match stopped {
            None | Some(Ok(())) => {}
            Some(Err(e)) => return Err(e),
        }
        // Wait for the listeners so the requests already in flight are answered rather than cut off —
        // bounded, because a listener that will not finish must not hold the process past the
        // orchestrator's grace. The ones the `select!` already completed are skipped: they have
        // nothing left to drain, and polling them again is the panic above.
        let drain = async {
            if !drained[0] {
                let _ = grpc_external.await;
            }
            if !drained[1] {
                let _ = grpc_internal.await;
            }
            if !drained[2] {
                let _ = http.await;
            }
            if !drained[3] {
                let _ = admin.await;
            }
            if !drained[4] {
                let _ = ocapn.await;
            }
        };
        let _ = tokio::time::timeout(SHUTDOWN_DRAIN_TIMEOUT, drain).await;
        Ok(())
    }
}

/// The store/runtime handles for **one** shard, extracted from [`setup_shard`] so that shard's
/// block-processing streams can be wired in a separate step.
pub struct ShardParts {
    /// Which shard these handles belong to.
    pub spec: ShardSpec,
    pub block_store: BlockStore,
    pub dag: Arc<dyn BlockDagStorage>,
    pub runtime_manager: Arc<RuntimeManager>,
    pub approved_store: ApprovedStore,
    /// This shard's store manager, over its own data directory ([`shard_data_dir`]).
    pub store_manager: LmdbDirStoreManager,
    pub validator_identity_opt: Option<ValidatorIdentity>,
    pub proposer: Option<ProposerParts>,
    /// This shard's client-facing APIs. Held here rather than in `NodeProgram` so a multi-shard
    /// node can dispatch a request to the shard that owns it (`ShardRoutingBlockApi`).
    pub block_api: Arc<dyn BlockApi>,
    pub block_report_api: Arc<BlockReportApi>,
    pub transaction_api: Arc<TransactionAPIImpl>,
}

/// The proposer request queue + shared state (port of the `proposerQueue`/`proposerStateRefOpt` in
/// `Setup.setupNodeProgram`). Built in [`setup`] (so `BlockApiImpl` can get the trigger + state);
/// consumed in [`setup_node_program`] to drive the proposer stream.
pub struct ProposerParts {
    pub queue_tx: mpsc::Sender<(ProposeSource, tokio::sync::oneshot::Sender<ProposerResult>)>,
    pub queue_rx: mpsc::Receiver<(ProposeSource, tokio::sync::oneshot::Sender<ProposerResult>)>,
    pub state: Arc<tokio::sync::Mutex<ProposerState>>,
}

/// Wire the block receiver + processor streams (port of the `BlockReceiver`/`BlockProcessor` part of
/// `Setup.setupNodeProgram`). Returns the `(incoming_blocks, validated_blocks)` channel senders so the
/// transport and proposer can plug into the pipeline. `NodeLaunch.apply` and the proposer are wired
/// separately.
/// Feed each validated hash's block from the store into the processor queue (the
/// `blockReceiverStream.evalMap(blockStore.getUnsafe)` half of the receiver).
///
/// Extracted so its failure behaviour is testable without a node fixture — the
/// `load_node`/`load_node_from_store` split, applied to a block-store read.
///
/// **A read that fails is reported, not skipped.** The oracle's `getUnsafe`
/// (`block-storage/.../BlockStoreSyntax.scala:33-35`) lifts an absent *or errored* read into
/// `BlockStoreInconsistencyError` in `F`; the port's `.ok()` dropped the block from the
/// validate→process path with nothing recorded, so a node could leave a peer's block unprocessed and
/// say nothing at all. A spawned task has no reply to send, so the honest answer is the log line —
/// naming the hash, because the hash is what makes the gap actionable.
pub(crate) async fn pump_validated_blocks(
    block_store: &BlockStore,
    validation_rx: &mut mpsc::UnboundedReceiver<BlockHash>,
    processor_input_tx: &mpsc::Sender<BlockMessage>,
    log: &dyn Log,
) {
    while let Some(hash) = validation_rx.recv().await {
        match block_store.get(&[hash]).await {
            Ok(mut v) => {
                if let Some(block) = v.pop().flatten() {
                    chan::send(&processor_input_tx, block).await;
                }
            }
            Err(e) => log.error(
                LogSource::new("node.runtime.block_processing"),
                &format!(
                    "a validated block could not be read back from the block store: {}: {e}",
                    hash.to_hex()
                ),
            ),
        }
    }
}

/// How many validated blocks may wait for the block receiver's round/fringe consumer, and — through the
/// taps — for the proposer and attestation callbacks (AUDIT C175).
///
/// **A policy number, not a measured envelope.** The queue observer landed in #120 and read depth **0.0 at
/// every sample** on the frozen reproduction while the node's anonymous memory climbed to its cgroup
/// ceiling, so nothing measured here says this queue needs *this much* room. What says it needs a bound at
/// all is that it had none, on the half of the pipeline a peer streams valid-signed blocks into: a peer
/// that fills it faster than the CPU-bound replay validation drains it grew the node's heap with block
/// messages. 1024 is the depth the processor input beside it already uses
/// (`rchain_casper::engine::node_running::MAX_PENDING_BLOCKS`); the point of the number is that the queue
/// is bounded and its producer *waits*, not that 1024 is the correct depth.
const MAX_VALIDATED_BLOCKS: usize = 1024;

pub fn wire_block_processing(
    comm_state: &CommState,
    parts: &ShardParts,
    shard_id: &str,
    min_phlo_price: i64,
    max_number_of_parents: i32,
    log: Arc<dyn Log>,
    autopropose: Option<Arc<dyn Fn() + Send + Sync>>,
    attest_on_new_blocks: Option<Arc<dyn Fn(&BlockMessage) + Send + Sync>>,
    health: ShardHealth,
) -> (mpsc::Sender<BlockMessage>, mpsc::Sender<BlockMessage>) {
    wire_block_processing_observed(
        comm_state,
        parts,
        shard_id,
        min_phlo_price,
        max_number_of_parents,
        log,
        autopropose,
        attest_on_new_blocks,
        None,
        health,
    )
}

fn wire_block_processing_observed(
    comm_state: &CommState,
    parts: &ShardParts,
    shard_id: &str,
    min_phlo_price: i64,
    max_number_of_parents: i32,
    log: Arc<dyn Log>,
    autopropose: Option<Arc<dyn Fn() + Send + Sync>>,
    attest_on_new_blocks: Option<Arc<dyn Fn(&BlockMessage) + Send + Sync>>,
    queue_metrics: Option<(Arc<MetricsRegistry>, usize)>,
    health: ShardHealth,
) -> (mpsc::Sender<BlockMessage>, mpsc::Sender<BlockMessage>) {
    let (incoming_blocks_tx, incoming_blocks_rx) =
        mpsc::channel(rchain_casper::engine::node_running::MAX_PENDING_BLOCKS);
    let (validated_blocks_tx, validated_blocks_rx) = mpsc::channel(MAX_VALIDATED_BLOCKS);

    // Tap the validated-blocks stream: for autopropose (propose on each validated block) and, when
    // `--attest-on-new-blocks` is on, for attestation (propose on each *remote* block).
    // The taps compose — each forwards the stream after firing.
    let autopropose_tap: Option<Arc<dyn Fn(&BlockMessage) + Send + Sync>> =
        autopropose.map(|tap| {
            Arc::new(move |_: &BlockMessage| tap()) as Arc<dyn Fn(&BlockMessage) + Send + Sync>
        });
    let queue_observer = |stage: &str| {
        queue_metrics.as_ref().map(|(metrics, index)| {
            metrics.queue_observer(
                rchain_shared::metrics::Source::base()
                    .sub("block_pipeline")
                    .sub(&format!("shard_{index}"))
                    .sub(stage),
            )
        })
    };
    let mut validated_blocks_rx = validated_blocks_rx;
    let mut observer = queue_observer("validated");
    if autopropose_tap.is_some() {
        validated_blocks_rx = tap_validated_blocks(validated_blocks_rx, autopropose_tap, observer);
        observer = queue_observer("autopropose");
    }
    if attest_on_new_blocks.is_some() {
        validated_blocks_rx =
            tap_validated_blocks(validated_blocks_rx, attest_on_new_blocks, observer);
        observer = queue_observer("attestation");
    }

    // Block receiver: incoming + validated blocks → a queue of dependency-free block hashes.
    let receiver_state = Arc::new(tokio::sync::Mutex::new(
        BlockReceiverState::<BlockHash>::new(),
    ));
    let put_to_incoming_queue: Arc<dyn Fn(BlockMessage) + Send + Sync> = Arc::new({
        let incoming_blocks_tx = incoming_blocks_tx.clone();
        move |block| {
            chan::try_send(&incoming_blocks_tx, block);
        }
    });
    let validation_rx = block_receiver::apply_with_queue_observer(
        receiver_state,
        incoming_blocks_rx,
        validated_blocks_rx,
        shard_id.to_string(),
        parts.block_store.clone(),
        parts.dag.clone(),
        comm_state.block_retriever.clone(),
        put_to_incoming_queue,
        log.clone(),
        observer,
    );

    // Load each validated hash's block from the store and feed the processor (port of
    // `blockReceiverStream.evalMap(blockStore.getUnsafe)`). Bounded so a peer streaming valid-signed
    // blocks applies backpressure to `load_blocks` instead of growing an unbounded in-memory queue
    // ahead of CPU-bound replay validation (R15).
    let (processor_input_tx, processor_input_rx) =
        mpsc::channel(rchain_casper::engine::node_running::MAX_PENDING_BLOCKS);
    let load_blocks = {
        let block_store = parts.block_store.clone();
        let log = log.clone();
        let mut validation_rx = validation_rx;
        async move {
            pump_validated_blocks(&block_store, &mut validation_rx, &processor_input_tx, &*log)
                .await;
        }
    };
    tokio::spawn(load_blocks);

    // Block processor: validate + insert into the DAG, then notify the validated queue.
    let block_index = {
        let runtime = parts.runtime_manager.clone();
        let dag = parts.dag.clone();
        let block_store = parts.block_store.clone();
        let log = log.clone();
        move |hash: BlockHash| {
            let runtime = runtime.clone();
            let dag = dag.clone();
            let block_store = block_store.clone();
            let log = log.clone();
            async move {
                // A missing metadata is reported as the index error it is — the caller retries the
                // lookup on the next index request, and the message names the block.
                let fringe_state_hash = fringe_state_of(&*dag, &hash).await?;
                let result = BlockIndex::get_block_index(
                    &runtime,
                    &*dag,
                    &block_store,
                    hash,
                    fringe_state_hash,
                )
                .await;
                // Indexing every stored block is the expensive half of a restart, and until now it was
                // silent (#60): a node replaying its whole DAG looked exactly like a hung one, with the
                // API down and nothing in the log. Report progress while it happens, so both the cost
                // and its cause (a replay fallback per block) are visible.
                let stats = rchain_casper::merging::IndexStats::read();
                if stats.calls > 0 && stats.calls % 250 == 0 {
                    log.info(
                        LogSource::new("coop.rchain.node.runtime.BlockIndex"),
                        &format!("block indexing: {}", stats.summary()),
                    );
                }
                result
            }
        }
    };
    let processor = block_processor::apply(
        processor_input_rx,
        validated_blocks_tx.clone(),
        shard_id.to_string(),
        min_phlo_price,
        max_number_of_parents,
        parts.dag.clone(),
        parts.block_store.clone(),
        parts.runtime_manager.clone(),
        comm_state.comm_util.clone(),
        block_index,
        log,
        health,
    );
    tokio::spawn(processor);

    (incoming_blocks_tx, validated_blocks_tx)
}

/// Build the RSpace importer over the on-chain state stores (port of
/// `HistoryRepository.lmdbRepository`'s `RSpaceImporterStore(history, cold, roots)`).
async fn create_rspace_importer(
    store_manager: &LmdbDirStoreManager,
) -> Result<RSpaceImporterStore, String> {
    let history_store = store_manager.store_sync("rspace-history").await?;
    let value_store = store_manager.store_sync("rspace-cold").await?;
    let roots_store = store_manager.store_sync("rspace-roots").await?;
    Ok(RSpaceImporterStore::new(
        history_store,
        value_store,
        roots_store,
    ))
}

/// Build the RSpace exporter over the on-chain state stores (port of
/// `RSpaceExporterStore(history, cold, roots)`).
async fn create_rspace_exporter(
    store_manager: &LmdbDirStoreManager,
) -> Result<RSpaceExporterStore, String> {
    let history_store = store_manager.store_sync("rspace-history").await?;
    let value_store = store_manager.store_sync("rspace-cold").await?;
    let roots_store = store_manager.store_sync("rspace-roots").await?;
    Ok(RSpaceExporterStore::new(
        history_store,
        value_store,
        roots_store,
    ))
}

/// Parse routing messages into peer messages and deliver each to the member shard it belongs to
/// (port of the `peerMessageStream` in `Setup.setupNodeProgram`, extended to route by shard).
///
/// A `BlockMessage` names its shard, so it goes to exactly one member — or is dropped with a log
/// line if the node is not a member (relaying to a shard this node does not validate is out of
/// scope by design). The hash-keyed messages fan out to every member, which is *self-selecting*:
/// a shard answers a `BlockRequest` only if the hash is in its own block store, so exactly the
/// owner replies and no hash→shard index is needed anywhere in the comm path. That is sound because
/// block hashes are shard-disjoint by construction — the shard id is a signed field of
/// `BlockMessage` and is committed by its hash.
fn spawn_peer_message_router(
    mut routing_rx: mpsc::Receiver<RoutingMessage>,
    shards: BTreeMap<String, mpsc::Sender<PeerMessage>>,
    log: Arc<dyn Log>,
) {
    let source = LogSource::new("coop.rchain.node.runtime.Setup");
    tokio::spawn(async move {
        while let Some(rm) = routing_rx.recv().await {
            let peer = rm.peer.clone();
            match to_casper_message_proto(&rm.packet)
                .and_then(|proto| CasperMessage::from_proto(&proto))
            {
                Ok(message) => {
                    let targets: Vec<&mpsc::Sender<PeerMessage>> = match &message {
                        CasperMessage::BlockMessage(block) => match shards.get(&block.shard_id) {
                            Some(tx) => vec![tx],
                            None => {
                                log.info(
                                    source,
                                    &format!(
                                        "Ignored block for shard {}, which this node is not a member of",
                                        block.shard_id
                                    ),
                                );
                                Vec::new()
                            }
                        },
                        // Hash-keyed requests: every member looks, only the owner answers.
                        _ => shards.values().collect(),
                    };
                    if targets.is_empty() {
                        // **The silent branch.** A block for a shard this node is not a member of is
                        // logged above; every *other* message is sent to every member, so an empty
                        // `shards` map dropped it with no line anywhere — and a node with no shard
                        // task answers nothing while looking healthy (issue #100's class).
                        log.warn(
                            source,
                            &format!(
                                "Dropped a message from {peer}: this node has no shard task to \
                                 deliver it to"
                            ),
                        );
                    }
                    for tx in targets {
                        // `send().await` already applies backpressure when the shard is busy (this is
                        // not the drop-on-full site that AUDIT C105 is about) — but the *error* was
                        // discarded, and the only error left is a **closed** channel: that shard's task
                        // has exited, so the message cannot be delivered at all. A drop nobody logs is
                        // how a node "handles" packets it never processes.
                        if tx
                            .send(PeerMessage {
                                peer: peer.clone(),
                                message: message.clone(),
                            })
                            .await
                            .is_err()
                        {
                            log.warn(
                                source,
                                &format!(
                                    "Could not deliver a message from {peer}: the shard's task has exited"
                                ),
                            );
                        }
                    }
                }
                Err(err) => {
                    log.warn(
                        source,
                        &format!(
                            "Could not extract casper message from packet sent by {peer}: {err}"
                        ),
                    );
                }
            }
        }
    });
}

/// Build the transport (protocol) server and its inbound-message dispatch closures (port of
/// `NetworkServers.protocolServer`).
fn build_protocol_server(
    conf: &NodeConf,
    comm_state: &CommState,
    routing_tx: mpsc::Sender<RoutingMessage>,
    log: Arc<dyn Log>,
) -> Result<ProtocolServer, String> {
    let cert = std::fs::read_to_string(&conf.tls.certificate_path).map_err(|e| e.to_string())?;
    let key = std::fs::read_to_string(&conf.tls.key_path).map_err(|e| e.to_string())?;

    let server = TransportLayerServer::new(
        comm_state.local_peer.clone(),
        conf.protocol_server.network_id.clone(),
        u16::try_from(conf.protocol_server.port).map_err(|e| e.to_string())?,
        &cert,
        &key,
        conf.protocol_server.grpc_max_recv_stream_message_size,
    )?
    // The inbound unary decode cap (AUDIT C113). This configuration key was read on the *client*
    // side only, so an inbound `send` was accepted up to tonic's 4 MiB default while the operator's
    // 256 KiB sat unused — a limit 16x looser than the one the config names.
    .with_max_recv_message_size(
        usize::try_from(conf.protocol_server.grpc_max_recv_message_size).unwrap_or(262144),
    );

    let dispatch: Box<dyn Fn(Protocol) -> BoxFuture<CommunicationResponse> + Send + Sync> = {
        let transport = comm_state.transport.clone();
        let rp_conf = comm_state.rp_conf.clone();
        let connections = comm_state.connections.clone();
        let routing_tx = routing_tx.clone();
        let log = log.clone();
        Box::new(move |proto: Protocol| {
            let transport = transport.clone();
            let rp_conf = rp_conf.clone();
            let connections = connections.clone();
            let routing_tx = routing_tx.clone();
            let log = log.clone();
            Box::pin(async move {
                handle_messages::handle(
                    proto,
                    &rp_conf,
                    transport.as_ref(),
                    connections.as_ref(),
                    &routing_tx,
                    log.as_ref(),
                )
                .await
            })
        })
    };

    let handle_streamed: Box<dyn Fn(Blob) -> BoxFuture<()> + Send + Sync> = {
        let routing_tx = routing_tx.clone();
        let log = log.clone();
        Box::new(move |blob: Blob| {
            let routing_tx = routing_tx.clone();
            let log = log.clone();
            Box::pin(async move {
                // **The streamed path had no inbound record at all** (issue #100). The one added to
                // `handle_messages::handle` covers the *unary* dispatch, and every streamed message —
                // the finalized fringe, every store-items page, every `stream_to_peers` broadcast —
                // bypasses it. That is why a second fringe arriving and being ignored was invisible
                // during #100's diagnosis, which had to be settled from the *other* node's timestamps.
                // `type_id` is the serde tag, so it names the message without a decode.
                log.debug(
                    LogSource::new("coop.rchain.comm.inbound"),
                    &format!(
                        "Received {} (streamed) from {}",
                        blob.packet.type_id, blob.sender.id
                    ),
                );
                // The result was discarded as well: the only error left is a closed channel, i.e. the
                // router task has exited, in which case the message cannot be delivered at all and a
                // node that "handled" it would be lying about it.
                if routing_tx
                    .send(RoutingMessage {
                        peer: blob.sender,
                        packet: blob.packet,
                    })
                    .await
                    .is_err()
                {
                    log.warn(
                        LogSource::new("coop.rchain.node.runtime.Setup"),
                        "Could not deliver a streamed message: the peer-message router is not running",
                    );
                }
            })
        })
    };

    Ok(ProtocolServer {
        server,
        dispatch,
        handle_streamed,
    })
}

/// Assemble the full running node program (port of `Setup.setupNodeProgram` +
/// `NodeRuntime.main`): comm/discovery state, block receiver/processor, peer-message stream,
/// transport server, `NodeLaunch.apply`, the proposer stream, and the request-missing-dependencies
/// loop.
pub async fn setup_node_program(
    conf: &NodeConf,
    id: &NodeIdentifier,
    log: Arc<dyn Log>,
) -> Result<NodeProgram, String> {
    let comm_state = create_comm_state(conf, id, log.clone()).await?;

    // The node's metric registry: one instance, shared by every shard's DAG (which publishes its
    // gauges) and by the HTTP server (whose `/metrics` publishes a snapshot of it).
    let metrics = Arc::new(MetricsRegistry::new());

    // The host-resource watchdog (A1). A dedicated thread, started before any store is opened, so the
    // signal is available to the status surface from the first request and is not carried by the tokio
    // executor it is meant to protect. It only reads; admission and fallback states arrive later.
    let resources = ResourceGuard::start(
        conf.storage.data_dir.clone(),
        std::time::Duration::from_secs(5),
    )
    .map_err(|e| format!("resource watchdog: {e}"))?;

    // Node-level resources, built once: the validator identity, and the REPL eval runtime (an
    // isolated `eval-*` store set, deliberately chain-independent — port of Scala's `evalStores`).
    let validator_opt: Option<ValidatorIdentity> = conf
        .casper
        .validator_private_key
        .as_deref()
        .and_then(ValidatorIdentity::from_hex);
    let eval_runtime = build_eval_runtime(&conf.storage.data_dir).await?;

    // Assemble every member shard: its own data directory, stores, runtime manager, block pipeline
    // and `NodeLaunch`.
    let mut shards: BTreeMap<ShardId, ShardRuntime> = BTreeMap::new();
    for (index, spec) in conf.casper.shards.iter().enumerate() {
        let runtime = setup_shard_runtime(
            conf,
            spec,
            index,
            id,
            &comm_state,
            &validator_opt,
            resources.clone(),
            &log,
            metrics.clone(),
        )
        .await?;
        shards.insert(spec.shard_id.clone(), runtime);
    }

    // Routing queue → peer-message router → each member shard's `NodeLaunch`.
    let (routing_tx, routing_rx) = mpsc::channel::<RoutingMessage>(50);
    let router_targets: BTreeMap<String, mpsc::Sender<PeerMessage>> = shards
        .iter()
        .map(|(shard_id, rt)| (shard_id.to_string(), rt.peer_tx.clone()))
        .collect();
    spawn_peer_message_router(routing_rx, router_targets, log.clone());

    // Request-missing-dependencies loop (port of `requestDependencies` in `Setup.setupNodeProgram`).
    let request_deps = {
        let block_retriever = comm_state.block_retriever.clone();
        let timeout = conf.casper.requested_blocks_timeout;
        let interval = conf.casper.casper_loop_interval;
        async move {
            loop {
                block_retriever.request_all(timeout).await;
                tokio::time::sleep(interval).await;
            }
        }
    };
    rchain_shared::supervise::spawn_supervised("block-retriever", request_deps);

    // The client surface is one set of servers over all the members: `ShardRoutingBlockApi` sends
    // each request to the shard that owns it, so the gRPC/HTTP services above it need no shard
    // awareness of their own.
    let primary_id = conf.casper.shards.primary().shard_id.clone();
    let shard_apis: BTreeMap<ShardId, Arc<dyn BlockApi>> = shards
        .iter()
        .map(|(shard_id, rt)| (shard_id.clone(), rt.parts.block_api.clone()))
        .collect();
    let routing: Arc<dyn BlockApi> =
        Arc::new(ShardRoutingBlockApi::new(shard_apis, primary_id.clone())?);
    let primary = shards
        .get(&primary_id)
        .ok_or_else(|| "the primary shard was not assembled".to_string())?;
    let primary_parts = &primary.parts;

    // The faucet signs transfers with the dev deployer key (only present in dev mode; `None`
    // disables the faucet). The funds come from the deployer vault seeded at genesis via wallets.txt.
    let faucet_deployer_key = conf
        .dev
        .deployer_private_key
        .as_deref()
        .and_then(|hex| base16::decode(hex))
        .map(PrivateKey::new);
    // **Resolved before the key is moved into the API**, because this is the one place both halves
    // are in hand (AUDIT R34). `WebApiImpl::capabilities` derives the same predicate from the block
    // API's `dev_mode` and its own key, asynchronously, which a synchronously-built router cannot
    // consult; the two values are the same pair the faucet handler refuses without.
    let faucet_enabled = conf.dev_mode && faucet_deployer_key.is_some();
    // **The listeners, resolved once** (issue #249): the tcp address, the unix path, and the noise
    // address, any of them or none — a node with none is dial-only.
    let ocapn_listeners = OcapnListeners {
        tcp: conf.api_server.ocapn_listen.clone(),
        unix: conf.api_server.ocapn_listen_unix.clone(),
        noise: conf.api_server.ocapn_listen_noise.clone(),
        websocket: conf.api_server.ocapn_listen_websocket.clone(),
    };
    // **The node's OCapN identity**, loaded or generated at the configured path. Required exactly by
    // the two transports that authenticate with it — `noise` names the node by the key, and
    // `websocket` signs its challenge response with it — and this is also where the node stops being
    // nameless: `node_designator` below is a *hash* of the deployer key and cannot sign, while this is
    // a key a peer can hold in advance. The three cases, including the one where a configured key is
    // validated without being used, are `ocapn_identity_for`'s (and are tested there).
    let ocapn_identity = crate::api::ocapn::ocapn_identity_for(
        &ocapn_listeners,
        conf.api_server.ocapn_identity_key.as_deref(),
    )?;
    // **This node's OCapN designator** — its name. When the node has a Noise identity the designator
    // **is** the Ed25519 verifying key, because that is the name the handshake actually checks: a peer
    // must put it in the SYN's cleartext prefix, and a responder refuses a SYN naming another node.
    // Without one, it falls back to the derivation that predates Noise (C224 item 2), which is a hash
    // of the deployer key and is stable across restarts but is not a key anything can sign with.
    let ocapn_designator = match &ocapn_identity {
        Some(identity) => rchain_shared::base16::encode(&identity.verifying_key()),
        None => crate::api::ocapn::node_designator(faucet_deployer_key.as_ref(), id),
    };
    // **The host a peer is told to dial** (HAZOP row C237). `local_addr()` is what a listener binds,
    // and `0.0.0.0`/`::` is not an address another host can reach — a remote peer following it reaches
    // itself — so the operator says what to advertise, and `serve_ocapn` refuses a listener bound to
    // an unspecified address that has not been told.
    let ocapn_advertised_host = conf.api_server.ocapn_advertised_host.clone();
    // Where the listener publishes the dialer it builds once its transports are bound; the admin
    // route reads it to start a dial of the node's own (issue #249).
    let ocapn_dial: OcapnDialSlot = Arc::new(std::sync::OnceLock::new());
    // The OCapN bridge (issue #249): a delivery to a chain-backed capability becomes a signed deploy.
    // Built only when the node both listens and has a key to sign with — today the node's own dev
    // deployer key, because binding a CapTP session to a caller's identity is the work
    // `docs/src/node/ocapn.md` lists as future.
    let ocapn_chain: Vec<(Vec<u8>, Arc<dyn Export>)> =
        match (ocapn_listeners.any(), faucet_deployer_key.as_ref()) {
            (true, Some(key)) => {
                // **One limiter for the node's whole OCapN surface**, shared by every capability and
                // every session: each bridged delivery is a signed deploy paid for out of the node's own
                // REV, so the *total* has to be on the node — a limiter per capability instead of this
                // is a bound of N times the number (HAZOP row A3).
                let limiter = Arc::new(rchain_shared::rate_limiter::RateLimiter::new(
                    crate::api::ocapn::BRIDGED_DEPLOYMENTS_PER_SEC,
                ));
                // **And a share of it per capability** (W-N8-02). The total alone is unfair rather
                // than unsafe: the one object a peer loops on can spend the whole allowance while
                // every other exported capability is refused. Keyed on the capability's designator,
                // so the share is per object and the node's total spend is unchanged.
                let capability_limiter =
                    Arc::new(rchain_shared::rate_limiter::KeyedRateLimiter::new(
                        crate::api::ocapn::BRIDGED_DEPLOYMENTS_PER_CAPABILITY_PER_SEC,
                        crate::api::ocapn::MAX_TRACKED_CAPABILITIES,
                    ));
                vec![
                    (
                        crate::api::ocapn::REV_VAULT_BALANCE_SWISS.to_vec(),
                        Arc::new(ChainCapability::rev_vault_balance(
                            routing.clone(),
                            key.clone(),
                            primary_id.to_string(),
                            limiter.clone(),
                            capability_limiter.clone(),
                        )) as Arc<dyn Export>,
                    ),
                    (
                        crate::api::ocapn::ERTP_SWISS.to_vec(),
                        Arc::new(crate::api::ocapn::ertp_capability(
                            routing.clone(),
                            key.clone(),
                            primary_id.to_string(),
                            limiter,
                            capability_limiter,
                        )),
                    ),
                ]
            }
            _ => Vec::new(),
        };
    let web_api: Arc<dyn WebApi> = Arc::new(
        WebApiImpl::new(
            routing.clone(),
            primary_parts.transaction_api.clone(),
            faucet_deployer_key,
            primary_id.to_string(),
            resources.clone(),
        )
        .with_persistent_faucet_ledger(conf.storage.data_dir.join("faucet-ledger.json"))?,
    );
    // The PoS read (AUDIT C148): the primary shard's live native state through the runtime manager
    // that owns it, plus the status API for the head's height — one definition of "latest block".
    let pos_read: Arc<dyn PosReadApi> = Arc::new(ShardPosRead::new(
        primary_parts.runtime_manager.clone(),
        web_api.clone(),
    ));
    let admin_web_api: Arc<dyn AdminWebApi> =
        Arc::new(AdminWebApiImpl::new(routing.clone(), resources.clone()));
    let grpc_services = GrpcServices::build(
        routing.clone(),
        primary_parts.block_report_api.clone(),
        eval_runtime,
        conf.api_server.enable_reporting,
    );
    // The membership, for the `GET /api/v1/shards` route: what the node validates for, primary
    // first, each with the API that reads its head.
    let registry = Arc::new(ShardRegistry {
        primary: primary_id.clone(),
        members: shards
            .iter()
            .map(|(shard_id, rt)| (shard_id.clone(), rt.parts.block_api.clone()))
            .collect(),
    });

    // The 2PC gateway (Laws 26–29): a node that is a member of several shards can drive a
    // cross-shard transaction itself. It exists only where it can act — more than one membership
    // and a signing key — so a single-shard or key-less node is untouched.
    let gateway = build_gateway(conf, &shards, &primary_parts.store_manager, &log).await?;

    Ok(NodeProgram {
        grpc_services,
        web_api,
        pos_read,
        admin_web_api,
        shards: registry,
        block_report_api: primary_parts.block_report_api.clone(),
        reporter: Arc::new(NewPrometheusReporter::new(prometheus_scrape_config())),
        metrics,
        host: conf.api_server.host.clone(),
        port_http: Port::try_from(conf.api_server.port_http).map_err(|e| e.to_string())?,
        port_admin_http: Port::try_from(conf.api_server.port_admin_http)
            .map_err(|e| e.to_string())?,
        port_grpc_external: Port::try_from(conf.api_server.port_grpc_external)
            .map_err(|e| e.to_string())?,
        port_grpc_internal: Port::try_from(conf.api_server.port_grpc_internal)
            .map_err(|e| e.to_string())?,
        grpc_max_recv_message_size: usize::try_from(conf.api_server.grpc_max_recv_message_size)
            .map_err(|e| e.to_string())?,
        max_connection_idle: conf.api_server.max_connection_idle,
        enable_reporting: conf.api_server.enable_reporting,
        faucet_enabled,
        enable_txn_api: conf.api_server.enable_txn_api,
        enable_devnet_cors: conf.api_server.enable_devnet_cors,
        enable_devnet_admin_public: conf.api_server.enable_devnet_admin_public,
        ocapn_listeners,
        ocapn_identity,
        ocapn_dial,
        enable_ocapn_dial: conf.api_server.enable_ocapn_dial,
        ocapn_deny_local_dial: conf.api_server.ocapn_deny_local_dial,
        ocapn_designator,
        ocapn_advertised_host,
        log: log.clone(),
        ocapn_chain,
        protocol_server: Some(build_protocol_server(
            conf,
            &comm_state,
            routing_tx,
            log.clone(),
        )?),
        status_provider: Some(StatusProvider {
            connections: comm_state.connections.clone(),
            rp_conf: comm_state.rp_conf.clone(),
            discovery: comm_state.discovery.clone(),
        }),
        gateway,
        resources,
        primary_runtime: primary_parts.runtime_manager.clone(),
    })
}

/// Build the on-node 2PC gateway, if this node can act as one.
///
/// It needs **more than one membership** (a single-shard node has nothing to coordinate across) and
/// a signing key (every participant gates `commit`/`abort` on the coordinator's key, and the legs
/// must be able to pay phlo on each shard). Without either, the gateway is absent rather than
/// present-and-failing.
///
/// On success the in-flight records are resumed in the background: a prepared participant holds its
/// escrow until the coordinator finishes, so a restart must finish or abort what it started. It is
/// spawned rather than awaited so a slow leg cannot stall startup.
async fn build_gateway(
    conf: &NodeConf,
    shards: &BTreeMap<ShardId, ShardRuntime>,
    store_manager: &LmdbDirStoreManager,
    log: &Arc<dyn Log>,
) -> Result<Option<Arc<GatewayTxn>>, String> {
    if shards.len() < 2 {
        return Ok(None);
    }
    let Some(identity) = shards
        .values()
        .next()
        .and_then(|rt| rt.parts.validator_identity_opt.clone())
    else {
        log.warn(
            LogSource::new("coop.rchain.node.runtime.Setup"),
            "This node is a member of several shards but has no validator key, so it cannot act as \
             a cross-shard coordinator",
        );
        return Ok(None);
    };
    let key = match conf
        .casper
        .validator_private_key
        .as_deref()
        .and_then(|hex| base16::decode(hex))
    {
        Some(bytes) => PrivateKey::new(bytes),
        None => return Ok(None),
    };

    let mut locals: BTreeMap<ShardId, LocalShard> = BTreeMap::new();
    for (shard_id, rt) in shards {
        locals.insert(
            shard_id.clone(),
            LocalShard {
                shard_id: shard_id.clone(),
                block_api: rt.parts.block_api.clone(),
                max_listen_depth: conf.api_server.max_blocks_limit,
            },
        );
    }
    let ledger = Arc::new(TxnLedger::open(store_manager).await?);
    let gateway = Arc::new(GatewayTxn::new(
        Arc::new(LocalShardDeployService::new(locals)),
        ledger,
        key,
        identity.public_key.clone(),
        Duration::from_secs(30),
    ));

    let recovery = gateway.clone();
    let recovery_log = log.clone();
    tokio::spawn(async move {
        match recovery.recover_in_flight().await {
            Ok(records) if records.is_empty() => {}
            Ok(records) => {
                let ids: Vec<String> = records.iter().map(|r| base16::encode(&r.txn_id)).collect();
                recovery_log.info(
                    LogSource::new("coop.rchain.node.runtime.Setup"),
                    &format!(
                        "Resumed {} cross-shard transaction(s) left in flight: {}",
                        records.len(),
                        ids.join(", ")
                    ),
                );
            }
            Err(err) => recovery_log.error(
                LogSource::new("coop.rchain.node.runtime.Setup"),
                &format!("Could not resume in-flight cross-shard transactions: {err}"),
            ),
        }
    });

    Ok(Some(gateway))
}

/// The first of the node's listeners to stop, named for the operator — [`NodeProgram::serve`].
///
/// Every listener is an accept loop, so *any* completion is fatal: an `Ok(())` means a loop that was
/// supposed to run forever has returned, and an `Err` is the failed bind this exists for. The name is
/// the point. A node that loses a listener has to say *which* one, because "the node is up but not
/// answering on the API port" is exactly the failure an operator cannot diagnose from silence
/// (AUDIT C142: the running node answered its health check from the *other* node on the port, and its
/// own log ended at `Making a transition to Running state.` with no error, panic or bind line).
///
/// `JoinError` is separated from the listener's own `Err` on purpose: a task that panicked and a bind
/// that was refused are different faults, and reporting the second as the first hides the panic.
///
/// **`stopping` is what keeps a clean stop from being reported as a fault** (AUDIT C144). Once the
/// operator's word has gone out, the listeners end *because they were told to* — that `Ok(())` is the
/// drain working, and it reads as "an accept loop returned" only if the reader is not told which of
/// the two it is looking at. Without it the race is real rather than theoretical: the stop arm and a
/// draining listener's arm both become ready, `select!` picks among ready arms in arbitrary order, and
/// `docker stop` would sometimes exit 1 on a perfectly clean shutdown.
fn listener_stopped(
    name: &str,
    joined: Result<Result<(), String>, tokio::task::JoinError>,
    stopping: bool,
) -> Result<(), String> {
    match joined {
        // The listener finished what it was doing and stopped because it was asked to: not a fault,
        // and the signal for `serve` to wait out the others.
        Ok(Ok(())) if stopping => Ok(()),
        Ok(Ok(())) => Err(format!(
            "the {name} stopped serving — an accept loop returned, so this node is not answering there"
        )),
        Ok(Err(e)) => Err(format!("the {name} failed: {e}")),
        Err(e) => Err(format!("the {name} task ended: {e}")),
    }
}

/// One member shard's assembled state: its handles plus the channel the peer-message router feeds.
struct ShardRuntime {
    parts: ShardParts,
    peer_tx: mpsc::Sender<PeerMessage>,
}

/// A registry for the test call sites of [`setup_shard`].
#[cfg(test)]
fn metrics_for_test() -> Arc<MetricsRegistry> {
    Arc::new(MetricsRegistry::new())
}

/// Build the node-level REPL eval runtime (an isolated `eval-*` store set over `data_dir`).
async fn build_eval_runtime(data_dir: &std::path::Path) -> Result<Arc<RhoRuntime>, String> {
    let store_manager = rnode_key_value_store_manager(data_dir);
    let eval_history = create_history_repository::<
        SortedProc,
        BindPattern,
        ListParWithRandom,
        TaggedContinuation,
    >(&store_manager, "eval")
    .await
    .map_err(|e| e.to_string())?;
    let eval_reader = eval_history.get_history_reader(eval_history.root()).await;
    let eval_hot = Arc::new(InMemHotStore::new(eval_reader.base()));
    let (eval_play, _) =
        RSpace::create_with_replay(eval_history.clone(), eval_hot, Arc::new(RhoMatch));
    let eval_runtime = RhoRuntime::create(eval_play, eval_history, SortedProc::default())
        .await
        .map_err(|e| e.to_string())?;
    Ok(Arc::new(eval_runtime))
}

/// **Publish a shard's proposer health (#157).** Two gauges, under one source per shard:
/// `consecutive_failures` and `autopropose_timer_halted`.
///
/// **Why it is a push and not a getter.** `/metrics` renders `state.metrics.snapshot()`
/// (`node/src/web/http.rs::metrics`), and this registry has no mechanism to pull a live value at scrape
/// time — `snapshot()` can only emit what some `set_gauge` previously wrote. So the value has to be
/// pushed by whatever can change it, which is why this is called from the autopropose tap and the
/// autopropose timer rather than once from setup. A gauge pushed only at setup would read `0` for ever
/// and look like a healthy node, which is the failure this exists to prevent.
fn push_proposer_health(metrics: &MetricsRegistry, source: &Source, health: &ProposeHealth) {
    let health: ProposerHealth = health.snapshot();
    metrics.set_gauge(
        source,
        "consecutive_failures",
        i64::try_from(health.consecutive_self_validation_failures).unwrap_or(i64::MAX),
    );
    metrics.set_gauge(
        source,
        "autopropose_timer_halted",
        i64::from(health.autopropose_timer_halted),
    );
    metrics.set_gauge(
        source,
        "stale_snapshot_self_equivocations",
        i64::try_from(health.stale_snapshot_self_equivocations).unwrap_or(i64::MAX),
    );
}

/// Wire one shard's block pipeline, `NodeLaunch` and proposer stream (port of the per-shard part of
/// `Setup.setupNodeProgram`), returning its handles and the router's delivery channel.
#[allow(clippy::too_many_arguments)]
async fn setup_shard_runtime(
    conf: &NodeConf,
    spec: &ShardSpec,
    index: usize,
    id: &NodeIdentifier,
    comm_state: &CommState,
    validator_opt: &Option<ValidatorIdentity>,
    resources: Arc<ResourceGuard>,
    log: &Arc<dyn Log>,
    metrics: Arc<MetricsRegistry>,
) -> Result<ShardRuntime, String> {
    // #157: created **here**, before `setup_shard`, because that call builds the `BlockApiImpl` that
    // reports it and the autopropose loop below is what writes it. One cell per shard, shared by
    // `Clone`.
    let propose_health = ProposeHealth::new();
    // **This shard's finality-and-merge cells** (W-N10-02, W-N10-03) — created beside the proposer's,
    // for the same reason: the validation path writes them and the `BlockApiImpl` this shard builds
    // reports them, so the cell exists before either. `Clone` shares the cells rather than copying
    // them. See [`ShardHealth`].
    let shard_health = ShardHealth::new();
    let mut parts = setup_shard(
        conf,
        spec,
        index,
        id,
        comm_state.connections.clone(),
        comm_state.discovery.clone(),
        validator_opt.clone(),
        metrics.clone(),
        propose_health.clone(),
        shard_health.clone(),
        log.as_ref(),
    )
    .await?;
    // LFS sync is shard-blind: the fringe exchange carries no shard id, so a multi-shard node could
    // not tell which chain a synced fringe belongs to and might import another shard's state. A
    // single-shard node syncs exactly as before; a gateway must start from a local chain
    // (reconnecting) or as its own genesis master.
    if conf.casper.shards.len() > 1
        && !conf.standalone
        && parts.dag.get_representation().await.dag_set.is_empty()
    {
        return Err(format!(
            "shard '{}' has no local chain and LFS sync is not shard-aware; start the node with \
             --standalone (genesis master) or give this shard existing state",
            spec.shard_id
        ));
    }
    // **The halt this shard took before this process** (W-N10-01, F-U10-02).
    //
    // Read here — after the shard's store exists and **before the timer is spawned below** — because a
    // seed placed after the spawn races the first tick, and the ordering is the part of this most
    // likely to be got wrong silently. `read_halt_record` is on the DAG's trait, so this reaches the
    // same store the reset marker writes to, under its own reserved key.
    //
    // **A read that fails is not a boot refusal.** Unlike the reset marker's read — where a failure
    // means an un-repaired rewind, so propagating `?` is right — this is the record of an
    // observability fact, and refusing to start over it would trade a silence for an outage. It seeds
    // the empty record with `durable: false`, which is the surface saying it cannot vouch for the
    // reading: "unknown", not "never halted".
    if let Err(e) = propose_health
        .seed_halt_from_store(parts.dag.as_ref())
        .await
    {
        log.warn(
            LogSource::new("coop.rchain.node.runtime.Setup"),
            &format!(
                "could not read the autopropose halt record for shard {}: {e} — this shard's halt \
                 history is unknown, not empty",
                spec.shard_id
            ),
        );
    }

    // **The shard's own health counters, seeded the same way** (W-N10-02). Read here — after the store
    // exists and before the validation path starts — so the surface reports the shard's history rather
    // than this process's. A read that fails seeds zeros rather than refusing to boot: these are
    // observations, and refusing to start over one would trade a silence for an outage. The two
    // *current-state* cells are deliberately not seeded — see [`ShardHealth::seed_counters`].
    let counters = parts.dag.read_health_counters().await;
    if let Err(e) = &counters {
        log.warn(
            LogSource::new("coop.rchain.node.runtime.Setup"),
            &format!(
                "could not read the health counters for shard {}: {e} — this shard's stall and \
                 merge history is unknown, not empty",
                spec.shard_id
            ),
        );
    }
    // Seeded from the read itself, so "nothing on record" (a fresh shard, which IS durable) and
    // "could not read" (unknown) are told apart rather than both reading as zero.
    shard_health.seed_counters(counters);

    let importer = create_rspace_importer(&parts.store_manager).await?;
    let exporter = create_rspace_exporter(&parts.store_manager).await?;
    let shard_id = spec.shard_id.to_string();

    // Extract the proposer queue/state before wiring block processing, so the autopropose tap can
    // enqueue a propose on each validated block.
    let proposer_parts = parts.proposer.take();

    // This shard's proposer health: the proposer resets the counter on success and bumps it on a
    // self-validation failure, and the shard's autopropose timer halts after a burst of failures. Per
    // shard, so a shard that cannot self-validate does not stop the others producing blocks.
    //
    // Since #157 it is also **published**, on two surfaces — the halt used to be one ERROR line and
    // nothing else, so a node that had stopped producing looked exactly like a node with nothing to do,
    // which is the ambiguity #148's probe cannot resolve without a witness. `push_proposer_health` is
    // called from both triggers below, because `/metrics` renders a snapshot of a push-populated
    // registry: a value pushed only at setup would read `0` for ever. No cell is created here: the one
    // handed to `setup_shard` above is the same cell, which is why it is created before that call.
    let consecutive_failures = propose_health.failures();
    let stale_snapshot_equivocations = propose_health.stale_snapshot_equivocations();
    let health_source = Source::base()
        .sub("proposer")
        .sub(&format!("shard_{index}"));

    // Autopropose tap: fire an (async) propose on each validated block.
    let autopropose: Option<Arc<dyn Fn() + Send + Sync>> = if conf.autopropose {
        match &proposer_parts {
            Some(pp) => {
                let tap_log = log.clone();
                let tap_tx = pp.queue_tx.clone();
                // #157: this tap is one of the two paths that can still change the counter after the
                // timer halts — a proposer that recovers clears it (`proposer.rs`'s success arm), and
                // without a push from here the gauge would keep reporting the pre-recovery count.
                let tap_metrics = metrics.clone();
                let tap_source = health_source.clone();
                let tap_health = propose_health.clone();
                // The tap runs on every validated block; if the propose queue is full or the
                // proposer stream is gone, the request is dropped — and nothing else would say so.
                let tap: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
                    // **A halted timer halts the tap too** (W-N4-08), and the decision is
                    // [`fire_autopropose_tap`]'s so that a test can drive it.
                    //
                    // The push comes first because *both* paths used to make it — the suppression was
                    // counted beside a fresh gauge reading, and an unhalted tap pushed before it
                    // enqueued — so hoisting it here is the same writes in the same order, with the
                    // guard now standing between them and the queue instead of around one of them.
                    push_proposer_health(&tap_metrics, &tap_source, &tap_health);
                    fire_autopropose_tap(&tap_health, || {
                        let (otx, _orx) = tokio::sync::oneshot::channel();
                        if let Err(e) = tap_tx.try_send((ProposeSource::Automatic, otx)) {
                            tap_log.warn(
                                LogSource::new("coop.rchain.node.runtime.Setup"),
                                &format!(
                                    "autopropose request not queued ({e}) — no block will be \
                                     proposed for this trigger"
                                ),
                            );
                        }
                    });
                });

                // Periodic timer: the event-driven tap only fires on a validated block or a deploy,
                // and a lone validator has neither after genesis. Tick every AUTOPROPOSE_INTERVAL so
                // `--autopropose` (with the dev-mode dummy deploy) produces blocks on its own. Halt
                // after a burst of consecutive self-validation failures instead of spinning forever.
                let timer_tx = pp.queue_tx.clone();
                let timer_failures = consecutive_failures.clone();
                let timer_log = log.clone();
                let timer_shard = shard_id.clone();
                // #157: the timer is the one trigger that stops, so it is the one that must leave the
                // halt readable — pushed at the tick *and* once more after the flag is set, because the
                // tick that breaks is the last write this task will ever make.
                let timer_metrics = metrics.clone();
                let timer_source = health_source.clone();
                let timer_health = propose_health.clone();
                let timer_dag = parts.dag.clone();
                rchain_shared::supervise::spawn_supervised("autopropose-timer", async move {
                    // **A record that says halted means this shard does not produce** (W-N10-01).
                    //
                    // The gate is `park_while_halted`, which checks the *flag* — the thing nothing
                    // consulted at boot before this, where only the failure counter below was read.
                    // Without it a halt seeded from the store would be *reported* by the surface while
                    // this loop kept proposing: the tap obeying the halt and the timer not, which is
                    // W-N4-08's defect one level up. It parks before the first tick as well as each tick
                    // after, so a seeded halt never produces once.
                    timer_health.park_while_halted().await;
                    let mut interval = tokio::time::interval(AUTOPROPOSE_INTERVAL);
                    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    loop {
                        interval.tick().await;
                        timer_health.park_while_halted().await;
                        let failures = timer_failures.load(Ordering::Relaxed);
                        if failures >= AUTOPROPOSE_MAX_CONSECUTIVE_FAILURES {
                            // **The record is written here, and the write is what makes the halt
                            // outlive this process.** The timer cannot refuse it — it has already
                            // decided to stop, so there is nothing left to abort — so a failed write is
                            // reported rather than raised, and the surface keeps `durable: false`: the
                            // node saying that this process honours a halt the next one will not know
                            // about. `mark_halt_durable(true)` is the only thing that clears it.
                            let record = timer_health.note_timer_halted();
                            match timer_dag.write_halt_record(record).await {
                                Ok(()) => timer_health.mark_halt_durable(true),
                                Err(e) => timer_log.error(
                                    LogSource::new("coop.rchain.node.runtime.Setup"),
                                    &format!(
                                        "shard {timer_shard} halted but its record could not be \
                                         written: {e} — this process honours the halt and the next one \
                                         will not know about it"
                                    ),
                                ),
                            }
                            push_proposer_health(&timer_metrics, &timer_source, &timer_health);
                            timer_log.error(
                                LogSource::new("coop.rchain.node.runtime.Setup"),
                                &format!(
                                    "block production for shard {timer_shard} halted after \
                                     {failures} consecutive self-validation failures; clear it with \
                                     POST /api/v1/autopropose/resume"
                                ),
                            );
                            // The next turn of the loop parks here, because `note_timer_halted` set
                            // the flag this gate reads.
                            continue;
                        }
                        push_proposer_health(&timer_metrics, &timer_source, &timer_health);
                        let (otx, _orx) = tokio::sync::oneshot::channel();
                        if let Err(e) = timer_tx.try_send((ProposeSource::Automatic, otx)) {
                            timer_log.warn(
                                LogSource::new("coop.rchain.node.runtime.Setup"),
                                &format!(
                                    "autopropose tick for shard {timer_shard} not queued ({e}) — \
                                     no block will be proposed"
                                ),
                            );
                        }
                    }
                });

                Some(tap)
            }
            None => None,
        }
    } else {
        None
    };

    // Attest-on-new-blocks tap: propose when a *remote* block is validated. With nothing of our own to
    // include, that proposal becomes an empty attestation (`block_creator.rs`'s attestation branch) — the
    // way a validator holding no deploys moves its latest message, and therefore the way a finality quorum
    // forms when every deploy arrives at one node. It is **on by default** (`--no-attest-on-new-blocks`
    // opts out) so a validator needs no `--autopropose` to be live (#70).
    //
    // It reacts to any remote block, including other validators' attestations, because the fringe rule
    // needs a *full partition*: every justification sender's message seen by every bonded sender. Reacting
    // only to deploy-bearing blocks gave exactly one round of attestations and the fringe never advanced.
    // The proposer's `suppress_attestation` is the pace rule; this tap only bounds its own queue, answering
    // each remote height at most once so a burst at one height cannot enqueue a request per block.
    let attest_on_new_blocks: Option<Arc<dyn Fn(&BlockMessage) + Send + Sync>> = match (
        &proposer_parts,
        conf.attest_on_new_blocks && !conf.no_attest_on_new_blocks,
        validator_opt,
    ) {
        (Some(pp), true, Some(identity)) => {
            let tap_log = log.clone();
            let tap_tx = pp.queue_tx.clone();
            // Our own sender bytes: `Validator` is exactly `Validator::from_slice(public_key.bytes())`
            // (see `block_creator.rs`), so comparing bytes identifies our own blocks.
            let me: Vec<u8> = identity.public_key.bytes().to_vec();
            // The highest height this tap has answered, **per sender** (AUDIT C192). A single height
            // gate answers only the first block of a round that comes to rest at one height — one block
            // per validator, all at the same height — and seals it, because nothing above that height
            // exists or can now be produced. Per-sender answers each peer's block at the resting height,
            // so the round advances; the gate still bounds a burst to one request per sender per height.
            let answered: Arc<Mutex<BTreeMap<Vec<u8>, i64>>> =
                Arc::new(Mutex::new(BTreeMap::new()));
            Some(Arc::new(move |block: &BlockMessage| {
                let height = i64::from(block.block_number);
                let sender = block.sender.as_bytes().to_vec();
                // History being replayed is not a round to answer (#223) — see `ATTEST_MAX_BLOCK_AGE_MS`.
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                if !attest_is_live(block.timestamp, now) {
                    return;
                }
                {
                    let mut answered = answered.lock().unpoison();
                    let last_for_sender = answered.get(&sender).copied();
                    if !attest_warranted(&me, &sender, height, last_for_sender) {
                        return;
                    }
                    answered.insert(sender, height);
                }
                let (otx, _orx) = tokio::sync::oneshot::channel();
                if let Err(e) = tap_tx.try_send((ProposeSource::Automatic, otx)) {
                    tap_log.warn(
                        LogSource::new("coop.rchain.node.runtime.Setup"),
                        &format!(
                            "attest request not queued ({e}) — this validator will not attest \
                             to the new block"
                        ),
                    );
                }
            }))
        }
        _ => None,
    };

    // Block receiver + processor streams (spawned internally).
    let (incoming_blocks_tx, _validated_blocks_tx) = wire_block_processing_observed(
        comm_state,
        &parts,
        &shard_id,
        conf.casper.min_phlo_price,
        conf.casper.max_number_of_parents,
        log.clone(),
        autopropose,
        attest_on_new_blocks,
        Some((metrics, index)),
        shard_health.clone(),
    );

    // This shard's slice of the peer-message stream (fed by the router).
    let (peer_tx, peer_message_rx) = mpsc::channel::<PeerMessage>(50);

    // Node launch mode dispatch (genesis → syncing → running over the peer-message stream).
    let node_launch = node_launch::apply(
        peer_message_rx,
        incoming_blocks_tx,
        spec.clone(),
        !conf.protocol_client.disable_lfs,
        conf.casper.sync_anchor.clone(),
        // F-U5-01's acting half (#320): the operator's switch, off unless they turned it on.
        conf.casper.self_reset,
        conf.protocol_server.disable_state_exporter,
        parts.validator_identity_opt.clone(),
        conf.standalone,
        conf.dev_mode,
        comm_state.transport.clone(),
        comm_state.comm_util.clone(),
        comm_state.block_retriever.clone(),
        comm_state.connections.clone(),
        comm_state.rp_conf.clone(),
        parts.runtime_manager.clone(),
        parts.block_store.clone(),
        parts.approved_store.clone(),
        parts.dag.clone(),
        importer,
        exporter,
        log.clone(),
    );
    let node_launch_log = log.clone();
    tokio::spawn(async move {
        if let Err(err) = node_launch.await {
            node_launch_log.error(
                LogSource::new("coop.rchain.node.runtime.Setup"),
                &format!("NodeLaunch exited with error: {err}"),
            );
            // **And the node actually stops** (§106's owed D3). `apply`'s error is a *concluded* stop
            // — a failed reset, or the LFS sync terminal, each of which says "this node is stopping" in
            // its own words — and a logged line is not a stop: this task ending leaves the block
            // receiver, the processor, the proposer and the API serving a node whose stores may have
            // moved under a view that was not rebuilt. The latch is what `main`'s wait selects on; the
            // first reporter wins, so two shards that both fail still produce one reason.
            crate::runtime::shutdown::report_fatal(format!("NodeLaunch exited with error: {err}"));
        }
    });

    // Proposer stream (port of `proposerStream` in `Setup.setupNodeProgram`). Runs only when a
    // validator identity is configured; the propose trigger + state were wired into `BlockApiImpl`
    // by [`setup_shard`].
    let validator = parts.validator_identity_opt.clone();
    if let (Some(proposer_parts), Some(validator)) = (proposer_parts, validator) {
        let block_index = {
            let runtime = parts.runtime_manager.clone();
            let dag = parts.dag.clone();
            let block_store = parts.block_store.clone();
            move |hash: BlockHash| {
                let runtime = runtime.clone();
                let dag = dag.clone();
                let block_store = block_store.clone();
                async move {
                    let fringe_state_hash = fringe_state_of(&*dag, &hash).await?;
                    BlockIndex::get_block_index(
                        &runtime,
                        &*dag,
                        &block_store,
                        hash,
                        fringe_state_hash,
                    )
                    .await
                }
            }
        };
        let propose_effect: Arc<
            dyn Fn(&BlockMessage) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>>
                + Send
                + Sync,
        > = {
            // The block body is persisted by `Proposer::validate_block` (before the DAG insert);
            // here we only broadcast the block hash to peers.
            let comm_util = comm_state.comm_util.clone();
            Arc::new(move |block: &BlockMessage| {
                let comm_util = comm_util.clone();
                let block = block.clone();
                Box::pin(async move {
                    comm_util
                        .send_block_hash(&block.block_hash, block.sender.as_bytes())
                        .await;
                })
            })
        };
        // Dev-mode dummy deploy: inject a signed `Nil` deploy whenever the pool is empty, so a proposal
        // always has something to include.
        //
        // Gated on `--autopropose` as well as the deployer key, and that gate is the point: the key on
        // its own is what enables `/api/faucet` (below), so a node that wants a faucet is not thereby
        // forced to keep proposing empty blocks. Proposing with nothing to include is otherwise a no-op
        // (`block_creator.rs`), which is the behaviour the network wants — a chain that advances on
        // content rather than on wall-clock time (see #70).
        let dummy_deploy_opt =
            dummy_deploy_key(conf.autopropose, conf.dev.deployer_private_key.as_deref())
                .map(|key| (key, "Nil".to_string()));

        let proposer = Proposer::apply(
            validator,
            shard_id.clone(),
            conf.casper.min_phlo_price,
            conf.casper.max_number_of_parents,
            spec.genesis_block_data.epoch_length,
            dummy_deploy_opt,
            parts.dag.clone(),
            parts.block_store.clone(),
            parts.runtime_manager.clone(),
            block_index,
            propose_effect,
            log.clone(),
            consecutive_failures.clone(),
            stale_snapshot_equivocations.clone(),
            conf.casper.equivocation_injection,
            shard_health.clone(),
        );
        let proposer_stream = proposer_instance::create(
            proposer_parts.queue_rx,
            proposer_parts.queue_tx,
            proposer,
            proposer_parts.state,
            resources.clone(),
            log.clone(),
        );
        tokio::spawn(async move {
            use futures_util::StreamExt;
            let mut stream = Box::pin(proposer_stream);
            while stream.next().await.is_some() {}
        });
    }

    Ok(ShardRuntime { parts, peer_tx })
}

/// **Fire the autopropose tap for one validated block, unless the timer is halted** (W-N4-08).
///
/// The row: the halt stopped the periodic timer and *only* the timer. The tap went on proposing on
/// every validated block, so a node whose status surface read `autoproposeTimerHalted: true` kept
/// producing — on inbound traffic, with no operator — which is the exact failure the halt exists to
/// prevent. The tap was the missing half; a halt that the surface reports and the producer ignores
/// is a report that cannot become untrue.
///
/// **Why it is a function rather than four lines inside the closure.** The two things the row is
/// about are "is a request handed to the queue" and "is the refusal counted", and neither is visible
/// from outside a `move` closure that owns its queue, its log and its health cell. Lifting the
/// decision out is what makes the arm drivable: `enqueue` is a parameter, so a test can pass one that
/// counts, and the halt is a fact about [`ProposeHealth`], so a test can set it.
///
/// Returns `true` when the request was handed over, `false` when the halt suppressed it — in which
/// case the suppression has been counted, because a refusal nobody can see is the same defect as a
/// report that cannot become untrue.
fn fire_autopropose_tap(health: &ProposeHealth, enqueue: impl FnOnce()) -> bool {
    if health.timer_halted() {
        health
            .tap_suppressed_halted()
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        return false;
    }
    enqueue();
    true
}

/// Assemble the node program (port of `Setup.setupNodeProgram`, minus the comm/discovery/proposer/
/// block-stream pieces).
/// Assemble one shard's stores, runtime manager, native state and client-facing APIs.
///
/// `index` is the membership's position in `conf.casper.shards` and selects its data directory
/// ([`shard_data_dir`]): the primary shard keeps the data-directory root, every additional shard
/// nests under `shard/…`. Node-level resources (the transport, the ports, the eval runtime, the
/// validator identity) are assembled once by [`setup_node_program`], not here.
pub async fn setup_shard(
    conf: &NodeConf,
    spec: &ShardSpec,
    index: usize,
    id: &NodeIdentifier,
    connections: ConnectionsCell,
    discovery: Arc<dyn NodeDiscovery>,
    validator_opt: Option<ValidatorIdentity>,
    metrics: Arc<MetricsRegistry>,
    propose_health: ProposeHealth,
    shard_health: ShardHealth,
    log: &dyn Log,
) -> Result<ShardParts, String> {
    let data_dir = shard_data_dir(&conf.storage.data_dir, index, &spec.shard_id);
    let store_manager = rnode_key_value_store_manager(&data_dir);

    // Block store + DAG storage.
    let block_store = block_store::create(&store_manager).await?;
    let approved_store = approved_store::create(&store_manager).await?;
    let block_metadata_kv: Arc<dyn KeyValueTypedStore<BlockHash, BlockMetadata>> = Arc::new(
        database(
            &store_manager,
            "block-metadata",
            Arc::new(BlockHashCodec),
            Arc::new(BlockMetadataCodec),
        )
        .await?,
    );
    // **Before the expensive metadata rebuild** (A2): the store's record count is cheap, and the
    // start-up resource floor is decided here rather than after a replay that a small host cannot
    // finish. A refusal returns a setup error; an observer's warnings are logged.
    let chain_blocks = u64::try_from(block_metadata_kv.count().await.map_err(|e| e.to_string())?)
        .map_err(|_| "the persisted block count does not fit u64".to_string())?;
    let startup_facts = crate::resources::preflight::gather(&data_dir);
    crate::resources::preflight::enforce(conf, &startup_facts, chain_blocks, log)?;
    let block_metadata_store = Arc::new(
        BlockMetadataStore::create(block_metadata_kv)
            .await
            .map_err(|e| e.to_string())?,
    );
    // **No `fringe-data` store** (Law 66/68; C250's residue, C270): the claim it keyed by fringe is
    // per-block metadata now, so the DAG is built over the metadata store alone. A shard that still
    // holds `fringe-data` rows from an earlier build leaves them on disk, unread.
    let deploy_index: Arc<dyn KeyValueTypedStore<DeployId, BlockHash>> = Arc::new(
        database(
            &store_manager,
            "deploy-index",
            Arc::new(BytesCodec),
            Arc::new(BlockHashCodec),
        )
        .await?,
    );
    let deploy_store: Arc<dyn KeyValueTypedStore<DeployId, SignedDeployData>> = Arc::new(
        database(
            &store_manager,
            "deploy-pool",
            Arc::new(BytesCodec),
            Arc::new(SignedDeployDataCodec),
        )
        .await?,
    );
    // Deployer key (hashed) → a block it signed in, for `GET /api/v1/deployer/{pubkey}`.
    let deployer_index: Arc<dyn KeyValueTypedStore<Vec<u8>, Vec<u8>>> = Arc::new(
        database(
            &store_manager,
            "deployer-index",
            Arc::new(BytesCodec),
            Arc::new(BytesCodec),
        )
        .await?,
    );
    // **Where a reset records itself** (F-U5-01, register C274): one literal key holding the anchor and
    // whether the purge finished. Without it a reset refuses to run, because a crash between its two
    // store operations would leave bodies the receiver re-injects — see `ResetMarker`.
    let reset_marker: Arc<dyn KeyValueTypedStore<Vec<u8>, Vec<u8>>> = Arc::new(
        database(
            &store_manager,
            RESET_MARKER_DB,
            Arc::new(BytesCodec),
            Arc::new(BytesCodec),
        )
        .await?,
    );
    let dag_kv = Arc::new(
        BlockDagKeyValueStorage::create(block_metadata_store, deploy_index, deploy_store)
            .await
            .map_err(|e| e.to_string())?
            // The shard's DAG publishes its gauges into the node's registry (`/metrics`).
            .with_metrics(metrics)
            // `0` (the default, and the only value a non-dev node can hold — see
            // `check_merge_divergence_injection`) injects nothing.
            .with_merge_divergence_injection(conf.casper.merge_divergence_injection)
            .with_deployer_index(deployer_index)
            .await?
            .with_reset_marker(reset_marker),
    );
    let block_dag_storage: Arc<dyn BlockDagStorage> = dag_kv.clone();

    // Runtime manager (play + replay runtimes + mergeable store). The configured effect-scheduler
    // mode (Laws 20–22) applies to the play runtime and is recorded on the manager for the
    // block-path hard-reject of `relaxed`; the replay runtime stays sequential.
    let effect_mode = conf
        .casper
        .effect_mode
        .parse::<EffectMode>()
        .map_err(|e| format!("invalid casper.effect-scheduler: {e}"))?;
    let history = create_history_repository::<
        SortedProc,
        BindPattern,
        ListParWithRandom,
        TaggedContinuation,
    >(&store_manager, "rspace")
    .await
    .map_err(|e| e.to_string())?;
    let reader = history.get_history_reader(history.root()).await;
    let hot = Arc::new(InMemHotStore::new(reader.base()));
    let (play, replay) = RSpace::create_with_replay(history.clone(), hot, Arc::new(RhoMatch));
    let rho_runtime = RhoRuntime::create_with_effect_mode(
        play.clone(),
        history.clone(),
        SortedProc::default(),
        true,
        effect_mode,
    )
    .await
    .map_err(|e| e.to_string())?;
    let replay_runtime =
        ReplayRhoRuntime::create(Arc::new(replay), history.clone(), SortedProc::default())
            .await
            .map_err(|e| e.to_string())?;
    let mergeable_store = Arc::new(
        database(
            &store_manager,
            "mergeable-channel-cache",
            Arc::new(BytesCodec),
            Arc::new(DeployMergeableDataCodec),
        )
        .await?,
    );
    // The per-block native-changes sidecar (issue #74): the native state mutations each block's
    // deploys and system deploys folded into its post-state, so a multi-parent merge can re-apply them
    // instead of silently reverting them.
    let native_changes_store = Arc::new(
        database(
            &store_manager,
            "native-changes-cache",
            Arc::new(BytesCodec),
            Arc::new(NativeSidecarCodec),
        )
        .await?,
    );
    // The network's genesis descriptors, read from the configured genesis files on **any** node that
    // has them, not only a bootstrap. A joining validator replays the genesis when it first indexes
    // it — that is where AUDIT C46 bit, because the genesis's PoS state and REV vault balances are
    // installed natively *outside* the block's deploys and so cannot be recovered from the block.
    // Distinct from `standalone`, which is only about whether this node runs the ceremony: a
    // non-ceremony node reads the bonds file strictly (it may not mint a validator set), the ceremony
    // may generate it.
    //
    // A *failure* here is fatal, in both directions: an unreadable bonds file must not fall back to a
    // default PoS genesis, because the node would then replay the genesis against the wrong validator
    // set. `Ok(None)` is not that case — it means the files are genuinely absent, so the node has no
    // genesis config and cannot replay the genesis at all; it keeps the defaults and the failure is
    // left to the replay rather than papered over here.
    let genesis_descriptors =
        rchain_casper::genesis::genesis_descriptors_from_config(spec, conf.standalone)?;
    let (genesis_pos, genesis_vaults) = match genesis_descriptors {
        Some(d) => (d.pos_genesis, d.vaults),
        None => (Default::default(), Vec::new()),
    };
    let runtime_manager = Arc::new(
        RuntimeManager::new(
            rho_runtime,
            replay_runtime,
            history,
            mergeable_store,
            native_changes_store,
            effect_mode,
        )
        .with_genesis_pos(genesis_pos)
        .with_genesis_vaults(genesis_vaults),
    );

    // The eval runtime for the Repl service is node-level (it holds no chain state), and the
    // validator identity is the node's, so both are built once by `setup_node_program`.

    // Proposer queue + trigger + state (port of the `proposerQueue`/`triggerProposeFOpt`/
    // `proposerStateRefOpt` in `Setup.setupNodeProgram`). The proposer stream itself is driven in
    // `setup_node_program`, but the trigger + state must be available to `BlockApiImpl` here.
    let (proposer_queue_tx, proposer_queue_rx) =
        mpsc::channel::<(ProposeSource, tokio::sync::oneshot::Sender<ProposerResult>)>(100);
    let proposer_state: Option<Arc<tokio::sync::Mutex<ProposerState>>> = validator_opt
        .as_ref()
        .map(|_| Arc::new(tokio::sync::Mutex::new(ProposerState::default())));
    let trigger_propose: Option<ProposeFunction> = if validator_opt.is_some() {
        let tx = proposer_queue_tx.clone();
        let f: ProposeFunction = Box::new(
            move |is_async: bool| -> Pin<Box<dyn Future<Output = ProposerResult> + Send + 'static>> {
                let tx = tx.clone();
                Box::pin(async move {
                    let (otx, orx) = tokio::sync::oneshot::channel();
                    // A caller asked, and waits: `Explicit`, so C171's pace bound does not apply.
                    chan::send(&tx, (ProposeSource::Explicit { is_async }, otx)).await;
                    orx.await.unwrap_or(ProposerResult::Empty)
                })
            },
        );
        Some(f)
    } else {
        None
    };
    let proposer_parts: Option<ProposerParts> =
        proposer_state.as_ref().map(|state| ProposerParts {
            queue_tx: proposer_queue_tx.clone(),
            queue_rx: proposer_queue_rx,
            state: state.clone(),
        });

    let network_id = conf.protocol_server.network_id.clone();
    let shard_id = spec.shard_id.to_string();
    let network_status: NetworkStatusFn = Box::new({
        let id = id.clone();
        let connections = connections.clone();
        let discovery = discovery.clone();
        move || {
            let id = id.clone();
            let connections = connections.clone();
            let discovery = discovery.clone();
            Box::pin(async move {
                let peers = connections.read().await.len() as i32;
                let nodes = discovery.peers().len() as i32;
                NetworkStatus {
                    address: id.to_string(),
                    peers,
                    nodes,
                }
            })
        }
    });

    let block_api: Arc<dyn rchain_casper::api::block_api::BlockApi> = Arc::new(BlockApiImpl::new(
        block_dag_storage.clone(),
        block_store.clone(),
        runtime_manager.clone(),
        validator_opt.clone(),
        network_id,
        shard_id.clone(),
        conf.casper.min_phlo_price,
        crate::web::version_info::node_version(),
        network_status,
        conf.casper.validator_private_key.is_none(),
        conf.api_server.max_blocks_limit,
        conf.dev_mode,
        trigger_propose,
        proposer_state.clone(),
        conf.autopropose,
        conf.propose_on_deploy,
        conf.api_server.enable_devnet_cors,
        std::collections::BTreeSet::new(),
        propose_health,
        shard_health,
    ));

    let report_store: Arc<dyn KeyValueTypedStore<BlockHash, BlockEventInfo>> = Arc::new(
        database(
            &store_manager,
            "reporting-cache",
            Arc::new(BlockHashCodec),
            Arc::new(BlockEventInfoCodec),
        )
        .await?,
    );
    let block_report_api = Arc::new(BlockReportApi::new(
        block_store.clone(),
        Arc::new(reporting_casper(
            &store_manager,
            &shard_id,
            block_dag_storage.clone(),
        )?),
        report_store,
        validator_opt.clone(),
    ));

    // The transaction API is shard-scoped: its `transfer_unforgeable` is derived from the shard's
    // genesis random, so each shard reads its own REV transfers.
    let transfer_unforgeable =
        BlockRandomSeed::transfer_unforgeable(&shard_id).map_err(|e| e.to_string())?;
    let transaction_api = Arc::new(TransactionAPIImpl::new(
        block_report_api.clone(),
        transfer_unforgeable,
    ));

    // Claim this shard's data directory before anything else touches it: a directory that belonged
    // to a different shard must fail startup, not read as an empty chain.
    check_shard_data_dir(&data_dir, spec, block_dag_storage.as_ref(), &block_store).await?;

    // **Repair the derived indexes before the backfill declares the deployer index complete** (B39,
    // HAZOP F-U17-11). The audit is independent of the reset marker: an upgraded store may carry rows
    // from a crash on an older build, or from a rewind whose empty metadata drop skipped the cleanup.
    // A failure is logged and the shard runs: the rows make the proposer's replay guard skip a pooled
    // deploy, which the next boot retries, rather than a reason to refuse to serve the shard.
    match dag_kv.repair_derived_indexes().await {
        Ok((0, 0)) => {}
        Ok((deploy_rows, deployer_rows)) => log.warn(
            LogSource::new("coop.rchain.node.runtime.DeployerIndex"),
            &format!(
                "{}: derived-index audit repaired {deploy_rows} deploy-index and {deployer_rows} \
                 deployer-index row(s) naming a block this DAG does not hold",
                spec.shard_id
            ),
        ),
        Err(e) => log.warn(
            LogSource::new("coop.rchain.node.runtime.DeployerIndex"),
            &format!("{}: derived-index audit failed: {e}", spec.shard_id),
        ),
    }

    // A node upgraded onto an existing chain indexes the blocks it already holds, once, in the
    // background; until it finishes, `GET /api/v1/deployer` reports the height it has reached.
    {
        let dag_kv = dag_kv.clone();
        let block_store = block_store.clone();
        let shard_id = spec.shard_id.to_string();
        tokio::spawn(async move {
            let log = rchain_shared::log::StderrLog::default();
            let source = LogSource::new("coop.rchain.node.runtime.DeployerIndex");
            match dag_kv.backfill_deployer_index(&block_store).await {
                Ok(0) => {}
                Ok(n) => log.info(
                    source,
                    &format!("{shard_id}: deployer index backfilled from {n} stored blocks"),
                ),
                Err(e) => log.warn(
                    source,
                    &format!("{shard_id}: deployer index backfill failed: {e}"),
                ),
            }
        });
    }

    Ok(ShardParts {
        spec: spec.clone(),
        block_store,
        dag: block_dag_storage,
        runtime_manager,
        approved_store,
        store_manager,
        validator_identity_opt: validator_opt,
        proposer: proposer_parts,
        block_api,
        block_report_api,
        transaction_api,
    })
}

/// Record — or verify — which shard owns a data directory.
///
/// The primary membership keeps the data-directory root while every additional shard nests under
/// `shard/…`, so reordering or renaming memberships would otherwise silently re-point a shard at
/// another shard's chain. The marker file turns that into a startup error.
///
/// On a node that predates the marker and already holds a chain, the stored block's shard id is
/// checked before the directory is claimed, so an upgraded node cannot adopt a foreign chain either.
async fn check_shard_data_dir(
    dir: &std::path::Path,
    spec: &ShardSpec,
    dag: &dyn BlockDagStorage,
    block_store: &BlockStore,
) -> Result<(), String> {
    // **The store's format is checked before its shard** (W-N11-03). A store written by a *newer*
    // build is a format this one does not know, and the shard-id check below would not notice: the
    // id marker is a plain file and reads the same. Before this, the only guard on a data directory
    // was which shard owned it, so an older binary — or an image at a base pinned only by tag — ran
    // over a newer store and was accepted, which is the reverse of the advance the upgrade is.
    check_store_format(dir)?;

    let marker = dir.join(SHARD_ID_MARKER);
    let expected = spec.shard_id.to_string();
    let recorded = std::fs::read_to_string(&marker)
        .ok()
        .map(|id| id.trim().to_string());

    match recorded {
        Some(id) if id == expected => Ok(()),
        Some(id) => Err(format!(
            "data directory {} belongs to shard '{id}', but this node is configured for shard \
             '{expected}'; remove the directory or fix `casper.shards`",
            dir.display()
        )),
        None => {
            let representation = dag.get_representation().await;
            if let Some(hash) = representation.dag_set.iter().next().copied() {
                let stored = block_store
                    .get(&[hash])
                    .await
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .next()
                    .flatten();
                if let Some(block) = stored {
                    if block.shard_id != expected {
                        return Err(format!(
                            "data directory {} holds shard '{}', but this node is configured for \
                             shard '{expected}'; remove the directory or fix `casper.shards`",
                            dir.display(),
                            block.shard_id
                        ));
                    }
                }
            }
            std::fs::write(&marker, &expected).map_err(|e| {
                format!(
                    "cannot write the shard-id marker to {}: {e}",
                    marker.display()
                )
            })
        }
    }
}

/// **Refuse a store written by a newer build, and stamp the ones this build opens** (W-N11-03).
///
/// Split out of [`check_shard_data_dir`] because it is a property of the *directory*, not of the
/// shard: it takes no store handle, so the decision is testable without a node — and the falsifier
/// below drives all four states (absent, equal, older, newer, malformed) directly.
///
/// The three answers, and only the third is a refusal:
///
/// * **no marker** — a store from a build that has no stamp, or a fresh directory. It is stamped with
///   this build's version and read. An absent stamp is not evidence of an old store, and inventing a
///   refusal for it would refuse every existing deployment on upgrade;
/// * **this build's version or a lower one** — the ordinary case, and the *intended* direction of an
///   upgrade (an older store is advanced by this build);
/// * **a higher one** — the store was written by a newer build, whose rows this one does not know how
///   to read. Refused with both numbers, the same shape the shard-id refusal uses.
///
/// A marker that is not a number is refused rather than guessed at: it was written by something, and
/// "something wrote this and it is not a version" is not a state to read a store from.
fn check_store_format(dir: &std::path::Path) -> Result<(), String> {
    let marker = dir.join(STORE_FORMAT_MARKER);
    let recorded = std::fs::read_to_string(&marker)
        .ok()
        .map(|v| v.trim().to_string());
    let Some(recorded) = recorded else {
        return std::fs::write(&marker, STORE_FORMAT_VERSION.to_string()).map_err(|e| {
            format!(
                "cannot write the store-format marker to {}: {e}",
                marker.display()
            )
        });
    };
    let version: u32 = recorded.parse().map_err(|_| {
        format!(
            "the data directory {} carries a store-format marker this build cannot read: '{recorded}' \
             (this build writes {STORE_FORMAT_VERSION})",
            dir.display()
        )
    })?;
    if version > STORE_FORMAT_VERSION {
        return Err(format!(
            "the data directory {} was written by a newer build (store format {version}, this build \
             writes {STORE_FORMAT_VERSION}); reading a format it does not know is how a downgrade \
             silently becomes an advance — start the newer build, or point this one at a copy",
            dir.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_models::block_metadata::SlashSeverity;

    /// A block store whose every read fails, so a read error is observable as one.
    struct FailingBlockStore;

    #[async_trait::async_trait]
    impl rchain_shared::typed_store::KeyValueTypedStore<BlockHash, BlockMessage> for FailingBlockStore {
        async fn get(&self, _keys: &[BlockHash]) -> Result<Vec<Option<BlockMessage>>, String> {
            Err("the block store is down".to_string())
        }
        async fn put(&self, _pairs: &[(BlockHash, BlockMessage)]) -> Result<(), String> {
            Err("the block store is down".to_string())
        }
        async fn delete(&self, _keys: &[BlockHash]) -> Result<usize, String> {
            Err("the block store is down".to_string())
        }
        async fn contains(&self, _keys: &[BlockHash]) -> Result<Vec<bool>, String> {
            Err("the block store is down".to_string())
        }
        async fn to_map(
            &self,
        ) -> Result<std::collections::BTreeMap<BlockHash, BlockMessage>, String> {
            Err("the block store is down".to_string())
        }
    }

    /// A log that records what it was told, so "nothing was reported" is falsifiable.
    #[derive(Default)]
    struct RecordingLog {
        messages: std::sync::Mutex<Vec<String>>,
    }

    impl RecordingLog {
        fn messages(&self) -> Vec<String> {
            self.messages.lock().expect("not poisoned").clone()
        }
    }

    impl Log for RecordingLog {
        fn is_trace_enabled(&self, _source: LogSource) -> bool {
            false
        }
        fn trace(&self, _source: LogSource, _msg: &str) {}
        fn debug(&self, _source: LogSource, _msg: &str) {}
        fn info(&self, _source: LogSource, _msg: &str) {}
        fn warn(&self, _source: LogSource, msg: &str) {
            self.messages
                .lock()
                .expect("not poisoned")
                .push(msg.to_string());
        }
        fn error(&self, _source: LogSource, msg: &str) {
            self.messages
                .lock()
                .expect("not poisoned")
                .push(msg.to_string());
        }
    }

    /// **A block that cannot be read back is reported, not skipped** (AUDIT C67).
    ///
    /// The oracle's `getUnsafe` (`block-storage/.../BlockStoreSyntax.scala:33-35`) lifts an absent
    /// *or errored* read into `BlockStoreInconsistencyError` in `F`; the port's `.ok()` dropped the
    /// block from the validate→process path with nothing recorded, so a node could leave a peer's
    /// block unprocessed and say nothing.
    ///
    /// Falsifier, both forms. Pre-fix (witnessing): the read fails and **nothing is reported**, and
    /// the assertion `log.messages().is_empty()` **passed on exactly that** (run 2026-09-24 before the
    /// change). Post-fix: one line, naming the hash and the store's error.
    #[tokio::test]
    async fn a_block_that_cannot_be_read_back_is_reported() {
        let block_store: BlockStore = Arc::new(FailingBlockStore);
        let log = RecordingLog::default();
        let (processor_input_tx, _processor_input_rx) = mpsc::channel(1);
        let (validation_tx, mut validation_rx) = mpsc::unbounded_channel();
        validation_tx
            .send(BlockHash::new([0x11; 32]))
            .expect("send");
        drop(validation_tx);

        pump_validated_blocks(&block_store, &mut validation_rx, &processor_input_tx, &log).await;

        let messages = log.messages();
        assert_eq!(
            messages.len(),
            1,
            "a block-store read failure must be reported once: {messages:?}"
        );
        assert!(
            messages[0].contains("11".repeat(32).as_str()),
            "and the line must name the block, got: {messages:?}"
        );
        assert!(
            messages[0].contains("the block store is down"),
            "and the store's own error, got: {messages:?}"
        );
    }
    use crate::configuration::configuration::parse_defaults;
    use crate::configuration::hocon::node_conf_from_hocon;

    struct NoopDiscovery;
    #[async_trait::async_trait]
    impl NodeDiscovery for NoopDiscovery {
        async fn discover(&self) {}
        fn peers(&self) -> Vec<rchain_comm::peer_node::PeerNode> {
            Vec::new()
        }
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rchain-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A standalone config over `dir`, for the assembly tests.
    fn test_conf(dir: &std::path::Path) -> NodeConf {
        let defaults = parse_defaults(dir.to_str().unwrap()).unwrap();
        let mut conf = node_conf_from_hocon(&defaults).unwrap();
        conf.storage.data_dir = dir.to_path_buf();
        conf.api_server.host = "127.0.0.1".to_string();
        conf
    }

    fn noop_comm() -> (ConnectionsCell, Arc<dyn NodeDiscovery>) {
        (
            Arc::new(tokio::sync::RwLock::new(Vec::new())),
            Arc::new(NoopDiscovery),
        )
    }

    #[tokio::test]
    async fn setup_assembles_shard_over_lmdb() {
        let dir = temp_dir("node-runtime");
        let conf = test_conf(&dir);
        let id = NodeIdentifier::new(vec![1u8]);
        let (connections, discovery) = noop_comm();

        let spec = conf.casper.shards.primary().clone();
        let parts = setup_shard(
            &conf,
            &spec,
            0,
            &id,
            connections,
            discovery,
            None,
            metrics_for_test(),
            ProposeHealth::new(),
            ShardHealth::new(),
            &rchain_shared::log::NopLog,
        )
        .await
        .expect("setup_shard should assemble");

        // The primary shard keeps the data-directory root, and the directory is claimed by marker.
        assert_eq!(parts.spec.shard_id, spec.shard_id);
        assert_eq!(parts.spec.shard_id.to_string(), "/root");
        assert_eq!(
            std::fs::read_to_string(dir.join(SHARD_ID_MARKER)).unwrap(),
            "/root"
        );
        // A shard reports itself as its own id, not the node's default.
        assert_eq!(parts.block_api.status().await.shard_id, "/root");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A second membership nests under `shard/<id segments>` and is stamped with its own shard id,
    /// so two shards in one process never share a store.
    #[tokio::test]
    async fn setup_assembles_a_second_shard_in_its_own_directory() {
        let dir = temp_dir("node-runtime-two-shards");
        let mut conf = test_conf(&dir);
        let primary = conf.casper.shards.primary().clone();
        conf.casper.shards = rchain_casper::conf::ShardMemberships::new(vec![
            primary.clone(),
            ShardSpec::new(
                "child".to_string(),
                "/root".to_string(),
                primary.genesis_block_data.clone(),
                primary.autogen_shard_size,
            )
            .unwrap(),
        ])
        .unwrap();

        let id = NodeIdentifier::new(vec![1u8]);
        let (c1, d1) = noop_comm();
        let (c2, d2) = noop_comm();
        let child = conf.casper.shards.iter().nth(1).unwrap().clone();

        let _primary_parts = setup_shard(
            &conf,
            &primary,
            0,
            &id,
            c1,
            d1,
            None,
            metrics_for_test(),
            ProposeHealth::new(),
            ShardHealth::new(),
            &rchain_shared::log::NopLog,
        )
        .await
        .expect("primary shard");
        let child_parts = setup_shard(
            &conf,
            &child,
            1,
            &id,
            c2,
            d2,
            None,
            metrics_for_test(),
            ProposeHealth::new(),
            ShardHealth::new(),
            &rchain_shared::log::NopLog,
        )
        .await
        .expect("child shard");

        assert_eq!(child_parts.spec.shard_id.to_string(), "/root/child");
        assert_eq!(child_parts.block_api.status().await.shard_id, "/root/child");
        let child_dir = dir.join("shard").join("root").join("child");
        assert!(child_dir.is_dir(), "{} should exist", child_dir.display());
        assert_eq!(
            std::fs::read_to_string(child_dir.join(SHARD_ID_MARKER)).unwrap(),
            "/root/child"
        );
        // The primary's stores live at the root, not under the child's directory.
        assert_eq!(
            std::fs::read_to_string(dir.join(SHARD_ID_MARKER)).unwrap(),
            "/root"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A directory that belongs to another shard is refused rather than read as an empty chain.
    #[tokio::test]
    async fn setup_refuses_a_directory_owned_by_another_shard() {
        let dir = temp_dir("node-runtime-foreign-dir");
        let conf = test_conf(&dir);
        std::fs::write(dir.join(SHARD_ID_MARKER), "/something-else").unwrap();

        let id = NodeIdentifier::new(vec![1u8]);
        let (connections, discovery) = noop_comm();
        let spec = conf.casper.shards.primary().clone();
        let err = match setup_shard(
            &conf,
            &spec,
            0,
            &id,
            connections,
            discovery,
            None,
            metrics_for_test(),
            ProposeHealth::new(),
            ShardHealth::new(),
            &rchain_shared::log::NopLog,
        )
        .await
        {
            Ok(_) => panic!("a foreign data directory must be refused"),
            Err(err) => err,
        };
        assert!(err.contains("/something-else"), "{err}");
        assert!(err.contains("configured for shard '/root'"), "{err}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- the peer-message router (Law 26) ------------------------------------

    /// A block whose shard id is `shard_id`, minimal but well-formed.
    fn block_for(shard_id: &str) -> rchain_models::casper::protocol::casper_message::BlockMessage {
        use rchain_models::block::state_hash::StateHash;
        use rchain_models::casper::protocol::casper_message::RholangState;
        use rchain_models::validator::Validator;
        use rchain_shared::refined::{BlockHeight, SeqNum};

        rchain_casper::proto_util::unsigned_block_proto(
            1,
            shard_id.to_string(),
            BlockHeight::try_from(1).expect("height"),
            Validator::from_slice(&[0u8; 65]),
            SeqNum::zero(),
            StateHash::from(
                rchain_crypto::hash::blake2b256_hash::Blake2b256Hash::from_bytes([0u8; 32]),
            ),
            StateHash::from(
                rchain_crypto::hash::blake2b256_hash::Blake2b256Hash::from_bytes([1u8; 32]),
            ),
            Vec::new(),
            std::collections::BTreeMap::new(),
            std::collections::BTreeSet::new(),
            RholangState {
                deploys: Vec::new(),
                system_deploys: Vec::new(),
            },
            0,
        )
    }

    fn a_peer() -> rchain_comm::peer_node::PeerNode {
        rchain_comm::peer_node::PeerNode::from(
            NodeIdentifier::new(vec![1u8]),
            "127.0.0.1".to_string(),
            Port::try_from(40400).expect("port"),
            Port::try_from(40404).expect("port"),
        )
    }

    /// Drive the router once with `message` and report which shard channels received it.
    async fn route_once(
        packet: rchain_models::comm::protocol::Packet,
        members: &[&str],
    ) -> Vec<(String, bool)> {
        let mut shards: std::collections::BTreeMap<String, mpsc::Sender<PeerMessage>> =
            std::collections::BTreeMap::new();
        let mut receivers = Vec::new();
        for member in members {
            let (tx, rx) = mpsc::channel::<PeerMessage>(5);
            shards.insert(member.to_string(), tx);
            receivers.push((member.to_string(), rx));
        }

        let (routing_tx, routing_rx) = mpsc::channel::<RoutingMessage>(5);
        spawn_peer_message_router(
            routing_rx,
            shards,
            Arc::new(rchain_shared::log::StderrLog::default()),
        );
        routing_tx
            .send(RoutingMessage {
                peer: a_peer(),
                packet,
            })
            .await
            .expect("route");
        drop(routing_tx);

        // Give the router a moment, then read whatever was delivered.
        tokio::time::sleep(Duration::from_millis(50)).await;
        receivers
            .into_iter()
            .map(|(member, mut rx)| (member, rx.try_recv().is_ok()))
            .collect()
    }

    /// A block goes to the member that owns its shard — and only that member.
    #[tokio::test]
    async fn the_router_delivers_a_block_to_its_shard() {
        use rchain_casper::protocol::casper_message_protocol::BlockMessageSerde;
        use rchain_models::casper::protocol::packet_type_tag::ToPacket;
        let delivered = route_once(
            BlockMessageSerde.mk_packet(&block_for("/root/child")),
            &["/root", "/root/child"],
        )
        .await;
        assert_eq!(
            delivered,
            vec![
                ("/root".to_string(), false),
                ("/root/child".to_string(), true)
            ],
            "a block must reach exactly the shard it names"
        );
    }

    /// A block for a shard this node is not a member of is dropped, not relayed — relaying is an
    /// explicit non-goal, and delivering it to a member would be a cross-shard corruption.
    #[tokio::test]
    async fn the_router_drops_a_block_for_a_foreign_shard() {
        use rchain_casper::protocol::casper_message_protocol::BlockMessageSerde;
        use rchain_models::casper::protocol::packet_type_tag::ToPacket;
        let delivered = route_once(
            BlockMessageSerde.mk_packet(&block_for("/somewhere-else")),
            &["/root", "/root/child"],
        )
        .await;
        assert!(
            delivered.iter().all(|(_, got)| !got),
            "a foreign block must reach no member: {delivered:?}"
        );
    }

    /// Hash-keyed messages fan out to every member, which is self-selecting: only the shard holding
    /// the hash answers. Sending them to one member would leave the block unfindable from the others.
    #[tokio::test]
    async fn the_router_fans_out_a_hash_keyed_message() {
        use rchain_casper::protocol::casper_message_protocol::BlockHashMessageSerde;
        use rchain_models::casper::protocol::packet_type_tag::ToPacket;
        let packet = BlockHashMessageSerde.mk_packet(
            &rchain_models::casper::protocol::casper_message::BlockHashMessage {
                block_hash: rchain_models::block_hash::BlockHash::new([9u8; 32]),
                block_creator: vec![0u8; 65],
            },
        );
        let delivered = route_once(packet, &["/root", "/root/child"]).await;
        assert_eq!(
            delivered,
            vec![
                ("/root".to_string(), true),
                ("/root/child".to_string(), true)
            ],
            "a hash-keyed request must reach every member"
        );
    }

    /// The guard's other half: a directory whose marker is *missing* but which already holds a
    /// chain must still be checked, so an upgraded node (or a directory copied between shards) cannot
    /// adopt a foreign chain just because the marker was not there yet.
    #[tokio::test]
    async fn a_directory_holding_a_foreign_chain_is_refused_without_a_marker() {
        use rchain_models::block_metadata::BlockMetadata;
        use rchain_models::validator::Validator;
        use rchain_shared::refined::{BlockHeight, SeqNum};

        let dir = temp_dir("node-runtime-foreign-chain");
        let conf = test_conf(&dir);
        let id = NodeIdentifier::new(vec![1u8]);
        let spec = conf.casper.shards.primary().clone();

        // Assemble the shard once, then plant a block belonging to a different shard.
        let (connections, discovery) = noop_comm();
        let parts = setup_shard(
            &conf,
            &spec,
            0,
            &id,
            connections,
            discovery,
            None,
            metrics_for_test(),
            ProposeHealth::new(),
            ShardHealth::new(),
            &rchain_shared::log::NopLog,
        )
        .await
        .expect("first assembly");
        let foreign = block_for("/someone-elses-shard");
        rchain_block_storage::syntax::put_block(&parts.block_store, foreign.clone())
            .await
            .expect("store the block body");
        parts
            .dag
            .insert(
                BlockMetadata {
                    block_hash: foreign.block_hash,
                    block_num: BlockHeight::try_from(1).expect("height"),
                    sender: Validator::from_slice(foreign.sender.as_bytes()),
                    seq_num: SeqNum::zero(),
                    justifications: std::collections::BTreeSet::new(),
                    bonds_map: std::collections::BTreeMap::new(),
                    validated: true,
                    validation_failed: false,
                    slashable: false,
                    failure_cause: None,
                    slash_severity: SlashSeverity::Unspecified,
                    restore_attempts: 0,
                    member_of_fringe: None,
                    fringe: std::collections::BTreeSet::new(),
                    fringe_state_hash: foreign.post_state_hash,
                },
                foreign,
            )
            .await
            .expect("insert into the dag");
        std::fs::remove_file(dir.join(SHARD_ID_MARKER)).expect("remove the marker");

        let (connections, discovery) = noop_comm();
        let err = match setup_shard(
            &conf,
            &spec,
            0,
            &id,
            connections,
            discovery,
            None,
            metrics_for_test(),
            ProposeHealth::new(),
            ShardHealth::new(),
            &rchain_shared::log::NopLog,
        )
        .await
        {
            Ok(_) => panic!("a directory holding a foreign chain must be refused"),
            Err(err) => err,
        };
        assert!(err.contains("holds shard"), "{err}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn create_rspace_importer_round_trips_over_lmdb() {
        use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
        use rchain_rspace::state::RSpaceImporter;
        use rchain_shared::state::TrieImporter;

        let dir = temp_dir("rspace-importer");
        let manager = rnode_key_value_store_manager(&dir);
        let mut importer = create_rspace_importer(&manager)
            .await
            .expect("importer should build");

        let hash = Blake2b256Hash::from_bytes([0x33; 32]);
        let value = vec![1u8, 2, 3];
        importer
            .set_history_items(&[(hash, value.clone())], |v: &Vec<u8>| v.clone())
            .expect("in-memory history store");
        importer.set_root(hash).expect("in-memory roots store");

        assert_eq!(
            importer
                .get_history_item(hash)
                .expect("in-memory history store"),
            Some(value)
        );

        drop(importer);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A store from a newer build is refused; the ones this build opens are stamped** (W-N11-03).
    ///
    /// The row: the only guard on a data directory was which shard owned it, and the mapping's own
    /// rule is that an upgraded node "leaves any existing rows on disk and ignores them" — so an older
    /// binary, or an image at a base pinned only by tag, ran over a newer store and was accepted,
    /// which is a downgrade read as an advance. All four states are driven here, because three of
    /// them must **not** refuse: an absent marker is stamped (or every existing deployment would
    /// refuse to boot on upgrade), an equal one is the ordinary boot, and a lower one is the intended
    /// direction of an advance. Only a higher one — and a marker that is not a number at all — refuse.
    ///
    /// Falsified by dropping the comparison (accept every version) and by inverting it (refuse the
    /// older store, which is the bug this is not allowed to have).
    #[test]
    fn a_store_from_a_newer_build_is_refused_and_this_build_stamps_what_it_opens() {
        let dir = temp_dir("node-runtime-store-format");
        let marker = dir.join(STORE_FORMAT_MARKER);

        // Absent: stamped with this build's version, and read.
        check_store_format(&dir).expect("a fresh directory is stamped, not refused");
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap(),
            STORE_FORMAT_VERSION.to_string(),
            "the store this build opens says which format it is"
        );

        // Equal, and older: both are boots of a store this build may read.
        check_store_format(&dir).expect("its own version");
        std::fs::write(&marker, "0").unwrap();
        check_store_format(&dir).expect("an older store is the advance an upgrade is for");

        // Newer: refused, naming both numbers.
        let newer = STORE_FORMAT_VERSION + 1;
        std::fs::write(&marker, newer.to_string()).unwrap();
        let err = check_store_format(&dir).expect_err("a newer store is not this build's to read");
        assert!(err.contains("newer build"), "{err}");
        assert!(err.contains(&newer.to_string()), "both versions: {err}");
        assert!(
            err.contains(&STORE_FORMAT_VERSION.to_string()),
            "both versions: {err}"
        );

        // And a marker that is not a version is refused rather than guessed at.
        std::fs::write(&marker, "not-a-version").unwrap();
        let err = check_store_format(&dir).expect_err("a marker that is not a number");
        assert!(err.contains("not-a-version"), "{err}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The dummy-deploy key, when this node should inject one: `--autopropose` **and** a deployer key.
///
/// The key alone is deliberately not enough. The same `dev.deployer-private-key` enables the
/// `/api/faucet` endpoint, so gating the injector on it alone would force every node that wants a
/// faucet to keep proposing empty blocks — the opposite of what the network wants. With nothing to
/// include, a proposal is already a no-op (`block_creator.rs`), so a chain should advance on content
/// rather than on wall-clock time (see #70).
///
/// The injector is a **dev/CI tool**, not a liveness mechanism: it is off unless `--autopropose` is
/// passed, and liveness now comes from attestation, which is on by default (`--no-attest-on-new-blocks`
/// opts out). A validator therefore needs neither `--autopropose` nor a deployer key (#70).
fn dummy_deploy_key(autopropose: bool, deployer_private_key: Option<&str>) -> Option<PrivateKey> {
    if !autopropose {
        return None;
    }
    deployer_private_key
        .and_then(base16::decode)
        .map(PrivateKey::new)
}

#[cfg(test)]
mod dummy_deploy_tests {
    use super::dummy_deploy_key;

    #[test]
    fn the_dummy_deploy_needs_autopropose_and_not_just_a_key() {
        let key = "0a".repeat(32);

        // The intentional case: continuous production is opted into explicitly.
        assert!(dummy_deploy_key(true, Some(&key)).is_some());
        // The case that matters: a faucet key alone must not imply empty blocks.
        assert!(dummy_deploy_key(false, Some(&key)).is_none());
        // And with no key there is nothing to sign with, either way.
        assert!(dummy_deploy_key(true, None).is_none());
        assert!(dummy_deploy_key(false, None).is_none());
    }
}

/// The scrape configuration: the default, plus the merge shape's own histogram buckets.
///
/// **Why the shape needs its own** (C182). The default bucket set runs 0.005 .. 10, and a scope width is
/// 1 .. 43 chains — so in Stage 1's first run every width sample landed in `+Inf`, the distribution
/// collapsed to its total, and the artifact said so while the code looked right. The defect was in *this
/// configuration*, so the configuration is a named function with a test rather than an argument to a
/// constructor.
///
/// The edges are the shape's own: the census's width edges, and a logarithmic set for the cost, whose
/// 10^6 bucket is the order the heap profile reached. The writer appends `+Inf`.
///
/// **The key is the source's own name, dots and all.** `Source::sub` joins with `.`, so the registry
/// looks the bucket set up under `rchain.merge.scope_width`; the underscores a reader sees on `/metrics`
/// are `normalize_metric_name`'s work at *render* time, one step later. The first fix of this defect used
/// the rendered form and changed nothing — which the second measurement caught, and which the test below
/// now cannot miss because it builds the key the same way the publisher does.
fn metric_key(name: &str) -> String {
    rchain_shared::metrics::Source::base()
        .sub("merge")
        .sub(name)
        .0
}

/// The merge shape's own boundaries, **derived from the census's constants rather than retyped**.
///
/// The two renderings of one quantity are the census's log line and this endpoint, and until now the
/// endpoint's edges were a hand-written superset (`8,16,32,64,128,256`) of the census's
/// (`WIDTH_EDGES`). Nothing compared them, so nothing could notice them drifting — and they had
/// already drifted in a way that made C182's own close condition unsatisfiable: the census's
/// open-ended bucket is *published at* its last edge, so a width of 200 and a width of 128 both arrive
/// as `128`, which is the value the configured edge list must contain for the two to agree. Deriving
/// the list from the constant is what makes "the endpoint agrees with the census" a claim a test can
/// state. (`search_census`'s buckets are five over four edges: the last is the open one.)
fn prometheus_scrape_config() -> crate::diagnostics::scrape_data_builder::Configuration {
    use rchain_casper::merging::search_census::{EXPANDED_EDGES, WIDTH_EDGES};

    let edges = |e: &[usize]| e.iter().map(|x| *x as f64).collect::<Vec<f64>>();
    let mut config = crate::diagnostics::scrape_data_builder::Configuration::default();
    config
        .custom_buckets
        .insert(metric_key("scope_width"), edges(&WIDTH_EDGES));
    config
        .custom_buckets
        .insert(metric_key("states_expanded"), edges(&EXPANDED_EDGES));
    config
}

/// Forward a validated-blocks stream, running `tap` on each block first (when there is one).
fn tap_validated_blocks(
    rx: mpsc::Receiver<BlockMessage>,
    tap: Option<Arc<dyn Fn(&BlockMessage) + Send + Sync>>,
    observer: Option<block_receiver::QueueObserver>,
) -> mpsc::Receiver<BlockMessage> {
    let Some(tap) = tap else {
        return rx;
    };
    // **The tap's own hand-off is bounded for the same reason the queue it reads is** (C175): a tap is a
    // proposer/attestation callback that does real work, so leaving this hop unbounded would move the
    // unboundedness one step downstream instead of removing it from the path.
    let (tap_tx, tap_rx) = mpsc::channel(MAX_VALIDATED_BLOCKS);
    tokio::spawn(block_receiver::consume_observed_queue(
        rx,
        observer,
        move |block| {
            let tap = tap.clone();
            let tap_tx = tap_tx.clone();
            async move {
                tap(&block);
                chan::send(&tap_tx, block).await;
            }
        },
    ));
    tap_rx
}

/// **The oldest a remote block may be and still prompt an attestation**, by its own timestamp.
///
/// An attestation answers a live round: a peer's block that the network is waiting on this node to see.
/// A block older than this is history this node is replaying — a validator that was down and is catching
/// up validates every block it missed, and the tap answered each one (#223). Live, a validator killed for
/// thirty heights came back and proposed twenty blocks in two seconds, one at every height it had missed,
/// two of them at epoch boundaries carrying a second `CloseBlock` for an epoch already closed; the
/// survivors then disagreed about the merged pre-state of the blocks built on them. On a live net a block
/// reaches its peers in well under a second, and the round gate's own stall bound is
/// `ROUND_STALL_ESCAPE`, so a block older than that is not part of any round still open. Only the tap is
/// gated: a deploy and the not-due retry still propose, so a catching-up node that is handed work does it.
const ATTEST_MAX_BLOCK_AGE_MS: i64 =
    rchain_casper::blocks::proposer::proposer::ROUND_STALL_ESCAPE.as_millis() as i64;

/// How far ahead of this node's clock a remote block may be stamped and still be a live round.
///
/// **The forward half of the same window, and it was missing.** A block whose timestamp is in the
/// future is live — the proposer's clock being ahead of ours must not silence a validator — but that
/// was implemented as `now.saturating_sub(block)` being *small*, and `saturating_sub` clamps at
/// `i64::MIN`… it clamps at *zero*: a block stamped in the year 3000 subtracts to `0`, which is well
/// under the age bound, so **every far-future-stamped block read as perfectly live** (W-N14-02). The
/// tap then fires on every replay of it, which is the amplification the age bound exists to prevent.
///
/// Symmetric with the age bound on purpose, and for the same reason: a block is a live round when its
/// own timestamp is within one round-stall of now, in either direction. A validator whose clock drifts
/// by more than fifteen seconds is not one whose blocks this node should be attesting to anyway.
const ATTEST_MAX_CLOCK_SKEW_MS: i64 = ATTEST_MAX_BLOCK_AGE_MS;

/// Whether a remote block with timestamp `block_ms` is still a live round at `now_ms`.
///
/// Both directions are bounded, and both are computed with saturating arithmetic: `block_ms` is wire
/// data and may be `i64::MIN` or `i64::MAX`, so neither subtraction may be trusted to stay in range.
fn attest_is_live(block_ms: i64, now_ms: i64) -> bool {
    block_ms <= now_ms.saturating_add(ATTEST_MAX_CLOCK_SKEW_MS)
        && now_ms.saturating_sub(block_ms) <= ATTEST_MAX_BLOCK_AGE_MS
}

/// Whether a validated block is a reason for this node to attest: any block from someone else.
///
/// Deliberately not restricted to blocks that carry deploys. The fringe rule requires a *full partition* —
/// every justification sender's message seen by every bonded sender — so the round that finalises a state
/// transition is the one in which the validators' attestations see each other. Restricting this to
/// deploy-bearing blocks produced exactly one round, and the fringe never advanced ([#70]).
///
/// The traffic is bounded by the proposer's guard, not here: `suppress_attestation` refuses to attest while
/// nothing unfinalized carries deploys (so an idle chain produces nothing), and while a supermajority is out
/// of reach it attests only once per `ATTESTATION_WINDOW` heights rather than at every one (so a chain that
/// has lost over a third of its stake does not spin). Each remote block can also prompt at most one proposal
/// in response.
///
/// **What these bounds do *not* cover, said plainly because this comment used to overstate them.** When the
/// quorum *is* reachable, none of the above limits the rate: a node attests promptly, its attestation is a
/// remote block for its peers, and they attest in turn. On an all-live net that is the `--attest-on-new-blocks`
/// storm recorded on #70 — 276 blocks in about a minute, finalised only to block 11 — and it is still
/// unbound: #70 is rolled into #126, whose open half is exactly this. The per-sender rule above is not
/// a bound at all while the height itself keeps advancing, so the pace
/// half belongs on *our own* quiet (`our latest message at least k heights behind`) and is not here yet. See
/// `docs/src/node/running-a-public-testnet.md`, "Attesting on every remote block is a block storm".
///
/// [#70]: https://github.com/rhovision-org/rholo/issues/70
fn attest_warranted(
    me: &[u8],
    sender: &[u8],
    height: i64,
    last_attested_height_for_sender: Option<i64>,
) -> bool {
    sender != me && last_attested_height_for_sender.map_or(true, |last| height > last)
}

/// The regression guard for C182's first defect: the shape's metrics must carry buckets that reach the
/// values they record. The default set tops out at 10, which is what put every sample in `+Inf`.
#[cfg(test)]
mod prometheus_scrape_config_tests {
    use super::{metric_key, prometheus_scrape_config};
    use crate::diagnostics::effects::MetricsRegistry;
    use crate::diagnostics::prometheus_reporter::NewPrometheusReporter;
    use rchain_shared::metrics::{Metrics, Source};

    /// **The test that decides whether the fix is a fix.** It renders the artifact an operator reads —
    /// through the same registry, the same `record` path and the same configuration production uses —
    /// instead of asserting that the configuration contains the key the configuration used, which is what
    /// certified the first attempt while every sample sat in `+Inf`.
    #[test]
    fn a_merge_width_renders_into_the_shapes_own_buckets() {
        let registry = MetricsRegistry::new();
        // Exactly the publisher's call shape: the base source, then the metric's name.
        let merge = Source::base().sub("merge");
        registry.record(&merge, "scope_width", 20, 1); // one merge whose conflict set was 20 chains
        registry.record(&merge, "states_expanded", 50_000, 1); // expanding 50,000 states

        let reporter = NewPrometheusReporter::new(prometheus_scrape_config());
        // Through the endpoint's own path (`render`), not the periodic accumulator: the two render the
        // same bytes for one snapshot, but only one of them is what an operator's scrape sees.
        let rendered = reporter.render(&registry.snapshot());

        // 20 chains is above the 16 edge and below the 32 — the shape's own boundaries, not the
        // registry's defaults (0.005 .. 10), which is the defect this asserts against.
        assert!(
            rendered.contains(r#"rchain_merge_scope_width_bucket{le="16.0"} 0.0"#),
            "the width must be bucketed by the shape's edges, not the default set:\n{rendered}"
        );
        assert!(
            rendered.contains(r#"rchain_merge_scope_width_bucket{le="32.0"} 1.0"#),
            "a 20-chain scope lands in the 32 bucket:\n{rendered}"
        );
        assert!(
            !rendered.contains(r#"rchain_merge_scope_width_bucket{le="0.005"}"#),
            "the default set's edges must be gone from the shape's metric:\n{rendered}"
        );
        // And the cost, on its own logarithmic edges.
        assert!(
            rendered.contains(r#"rchain_merge_states_expanded_bucket{le="100000.0"} 1.0"#),
            "a 50,000-state merge lands in the 10^5 bucket:\n{rendered}"
        );
    }

    /// **Idempotence: a scrape must not change what the next scrape says.** The devnet campaign of
    /// 2026-09-30 found the census and `/metrics` disagreeing on one quantity — the census rendered `101
    /// merges` while the endpoint rendered `_count 7617`. The factor was the **scrape count**, not a
    /// reporting period: the endpoint pushes a snapshot per request into a five-year accumulator, and
    /// this registry's histograms are *cumulative*, so each request added the running total to itself.
    /// The campaign's own sampler curled `/metrics` once a second, so the instrument was inflated by the
    /// measurement. Counters were summed the same way; gauges used `insert` and were the one channel that
    /// read correctly, which is what caught it.
    ///
    /// The blast radius was exactly the two metrics C182 added — `casper/src/dag.rs:272,283` are the sole
    /// `Metrics::record` call sites in the workspace, and the queue depths are gauges.
    #[test]
    fn re_reporting_a_snapshot_does_not_double_a_histogram_count() {
        let registry = MetricsRegistry::new();
        let merge = Source::base().sub("merge");
        registry.record(&merge, "scope_width", 20, 1);

        // `render` is the path `/metrics` takes; `report_period_snapshot` is the *periodic* entry and
        // merges by design (kamon's contract, and its own tests pin that). The defect was the trigger,
        // so the guard exercises the trigger's path — twice, because the whole class is "the second
        // scrape disagrees with the first".
        let reporter = NewPrometheusReporter::new(prometheus_scrape_config());
        let first = reporter.render(&registry.snapshot());
        let second = reporter.render(&registry.snapshot());

        assert!(
            first.contains("rchain_merge_scope_width_count 1.0"),
            "one merge, one count:\n{first}"
        );
        assert!(
            second.contains("rchain_merge_scope_width_count 1.0"),
            "a re-reported snapshot doubled the histogram's count — which is what inflates the \
             devnet's histogram ~75x over the census:\n{second}"
        );
    }

    /// **Two observations, two buckets — the guard the registry needed and never had.** Without a
    /// recorded value map, `snapshot()` emitted a single `Bucket { value: max, frequency: count }`, so
    /// every `_bucket{le=…}` line was a function of the **running maximum** and the scrape schedule
    /// rather than of the data. Every histogram fixture in this tree recorded exactly one sample, which
    /// is why neither this nor the accumulation defect was visible: a one-sample distribution is
    /// correctly rendered by a single-bucket accumulator.
    #[test]
    fn a_histogram_renders_every_observation_it_recorded() {
        let registry = MetricsRegistry::new();
        let merge = Source::base().sub("merge");
        registry.record(&merge, "scope_width", 20, 1); // one merge whose conflict set was 20 chains
        registry.record(&merge, "scope_width", 40, 1); // and one at 40

        let reporter = NewPrometheusReporter::new(prometheus_scrape_config());
        // Through the endpoint's own path (`render`), not the periodic accumulator: the two render the
        // same bytes for one snapshot, but only one of them is what an operator's scrape sees.
        let rendered = reporter.render(&registry.snapshot());

        assert!(
            rendered.contains("rchain_merge_scope_width_count 2.0"),
            "both observations are counted:\n{rendered}"
        );
        // Cumulative, as a Prometheus histogram is: 20 lands in le=32, and 40 in le=64.
        assert!(
            rendered.contains(r#"rchain_merge_scope_width_bucket{le="32.0"} 1.0"#),
            "the 20-chain observation is in the 32 bucket, not the top one:\n{rendered}"
        );
        assert!(
            rendered.contains(r#"rchain_merge_scope_width_bucket{le="64.0"} 2.0"#),
            "and the 40-chain one joins it at 64:\n{rendered}"
        );
    }

    #[test]
    fn the_merge_shape_metrics_carry_their_own_buckets() {
        let config = prometheus_scrape_config();
        let width = config
            .custom_buckets
            .get(&metric_key("scope_width"))
            .expect("the width metric has its own buckets");
        let cost = config
            .custom_buckets
            .get(&metric_key("states_expanded"))
            .expect("the cost metric has its own buckets");
        // And the key must be the *distribution's* name, not the rendered one: `Source::sub` joins with
        // a dot and the renderer turns those into underscores one step later, so a set keyed on the
        // rendered form is looked up by nothing and changes nothing (measured: the first fix of this
        // defect did exactly that and left every sample in `+Inf`).
        assert!(
            metric_key("scope_width").contains('.'),
            "the registry's name for a metric keeps the source separator: {}",
            metric_key("scope_width")
        );
        // **Equality with the census's own constants, not a superset check.** The previous assertion was
        // existential — `width.iter().any(|edge| *edge >= 64.0)` — which passes for the correct list and
        // for `[64]` and for any hand-written superset, which is what this was. Nothing compared the two
        // renderings of one quantity, and they had already drifted into a state that made C182's close
        // condition unsatisfiable (see `prometheus_scrape_config`). Deriving the list makes this an
        // equality; asserting the equality is what keeps it derived.
        use rchain_casper::merging::search_census::{EXPANDED_EDGES, WIDTH_EDGES};
        let expect = |constant: &[usize]| constant.iter().map(|e| *e as f64).collect::<Vec<f64>>();
        assert_eq!(
            *width,
            expect(&WIDTH_EDGES),
            "the width boundaries are the census's own, not a hand-written set: {width:?}"
        );
        assert_eq!(
            *cost,
            expect(&EXPANDED_EDGES),
            "the cost boundaries are the census's own: {cost:?}"
        );
        // And the top edge is the one the census *publishes its open bucket at*, so a width past it
        // arrives as this value and must land on this edge rather than past the last one.
        assert_eq!(
            width.last(),
            Some(&(WIDTH_EDGES[WIDTH_EDGES.len() - 1] as f64))
        );
    }
}

/// #157: the halt, read off the artifact an operator reads.
#[cfg(test)]
mod proposer_health_metric_tests {
    use super::{prometheus_scrape_config, push_proposer_health};
    use crate::diagnostics::effects::MetricsRegistry;
    use crate::diagnostics::prometheus_reporter::NewPrometheusReporter;
    use rchain_casper::api::block_api::ProposeHealth;
    use rchain_shared::metrics::Source;
    use std::sync::atomic::Ordering;

    fn scrape(health: &ProposeHealth, source: &Source) -> String {
        let registry = MetricsRegistry::new();
        push_proposer_health(&registry, source, health);
        NewPrometheusReporter::new(prometheus_scrape_config()).render(&registry.snapshot())
    }

    /// **The witness itself, through the reporter rather than through the registry.** A node that has
    /// halted publishes a non-zero count *and* the flag; a node that is merely quiet publishes neither,
    /// and that contrast is the whole reason #148's probe can now tell the two apart.
    #[test]
    fn a_halted_timer_is_readable_on_the_scrape() {
        let source = Source::base().sub("proposer").sub("shard_0");
        let health = ProposeHealth::new();

        // Quiet: nothing has failed, so nothing reads as halted.
        let quiet = scrape(&health, &source);
        assert!(
            quiet.contains("rchain_proposer_shard_0_consecutive_failures 0"),
            "{quiet}"
        );
        assert!(
            quiet.contains("rchain_proposer_shard_0_autopropose_timer_halted 0"),
            "{quiet}"
        );
        assert!(
            quiet.contains("rchain_proposer_shard_0_stale_snapshot_self_equivocations 0"),
            "{quiet}"
        );

        // Three self-validation failures, then the timer's own halt — and, separately, one
        // stale-snapshot self-equivocation, which must NOT read as a failure.
        let counter = health.failures();
        counter.fetch_add(1, Ordering::Relaxed);
        counter.fetch_add(1, Ordering::Relaxed);
        counter.fetch_add(1, Ordering::Relaxed);
        health
            .stale_snapshot_equivocations()
            .fetch_add(1, Ordering::Relaxed);
        health.note_timer_halted();

        let halted = scrape(&health, &source);
        assert!(
            halted.contains("rchain_proposer_shard_0_consecutive_failures 3"),
            "the count that caused the halt is reported beside it:\n{halted}"
        );
        assert!(
            halted.contains("rchain_proposer_shard_0_autopropose_timer_halted 1"),
            "{halted}"
        );
        assert!(
            halted.contains("rchain_proposer_shard_0_stale_snapshot_self_equivocations 1"),
            "the race is reported on its own gauge, beside the failure count that caused the halt:\n{halted}"
        );

        // **The case that makes the pair worth having.** The node recovers and proposes: the count
        // clears, and the flag must NOT — the timer is still stopped, and it is the flag that says so.
        counter.store(0, Ordering::Relaxed);
        let recovered = scrape(&health, &source);
        assert!(
            recovered.contains("rchain_proposer_shard_0_consecutive_failures 0"),
            "{recovered}"
        );
        assert!(
            recovered.contains("rchain_proposer_shard_0_autopropose_timer_halted 1"),
            "a recovered node must still report its stopped timer:\n{recovered}"
        );
    }

    /// **A halted shard does not tick until the halt is cleared** (W-N10-01) — the *obedience* half of
    /// F-U10-02, driven through the timer's own gate rather than a copy of it.
    ///
    /// A halt *seeded from the store at boot* must stop production, not merely be reported. The earlier
    /// code consulted only `consecutive_self_validation_failures`, so a node could report halted on the
    /// surface while the timer kept proposing — the tap obeying the halt and the timer not, which is
    /// W-N4-08's defect one level up.
    ///
    /// **The falsifier.** Make `park_while_halted` return immediately (drop the `timer_halted` check)
    /// and the task below sets `ticked` while the halt still stands, which the first assertion catches.
    #[tokio::test]
    async fn a_halted_timer_does_not_tick_until_the_halt_is_cleared() {
        use rchain_block_storage::dag::dag_storage::HaltRecord;
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;
        use std::time::Duration;

        let health = ProposeHealth::new();
        // The state a restart leaves: the record was read at boot and seeded into the cell.
        health.seed_halt(Some(HaltRecord {
            at_ms: 1,
            episodes: 1,
            durable: true,
        }));

        let ticked = Arc::new(AtomicBool::new(false));
        let gate_health = health.clone();
        let gate_ticked = ticked.clone();
        let handle = tokio::spawn(async move {
            gate_health.park_while_halted().await;
            gate_ticked.store(true, Ordering::SeqCst);
        });

        // It must not get past the gate while the halt stands.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !ticked.load(Ordering::SeqCst),
            "a halted shard does not tick — the halt is obeyed, not just reported"
        );

        // The operator's clear resumes it in-process, without a restart.
        health.clear_halt();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("the clear releases the gate")
            .expect("the parked task did not panic");
        assert!(
            ticked.load(Ordering::SeqCst),
            "and production resumes on the operator's word"
        );
    }
}

#[cfg(test)]
mod autopropose_tap_tests {
    use super::fire_autopropose_tap;
    use rchain_casper::api::block_api::ProposeHealth;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    /// **A halted timer halts the tap too** (W-N4-08).
    ///
    /// The row: the halt stopped the timer and nothing else, so the tap kept proposing on every
    /// validated block — the status surface said `autoproposeTimerHalted: true` while the node went
    /// on producing on inbound traffic. Two directions, and both are asserted: an unhalted tap hands
    /// the request over, a halted one hands over nothing **and counts what it refused**.
    ///
    /// Falsified by dropping the guard (the second arm's enqueue fires) or by counting before the
    /// check rather than after it (the first arm's counter moves off zero) — the counter is what
    /// distinguishes a refusal the operator can see from one the node swallows.
    #[test]
    fn a_halted_timer_suppresses_the_tap_and_counts_what_it_refused() {
        let health = ProposeHealth::new();
        let enqueued = Arc::new(AtomicU64::new(0));
        let note = {
            let enqueued = enqueued.clone();
            move || {
                enqueued.fetch_add(1, Ordering::Relaxed);
            }
        };

        // Not halted: the tap fires, and nothing was suppressed.
        assert!(fire_autopropose_tap(&health, note.clone()));
        assert_eq!(
            enqueued.load(Ordering::Relaxed),
            1,
            "an unhalted tap hands its request over"
        );
        assert_eq!(
            health.tap_suppressed_halted().load(Ordering::Relaxed),
            0,
            "and counts no suppression on the way"
        );

        // Halted: nothing is handed over, and the refusal is visible.
        health.note_timer_halted();
        assert!(!fire_autopropose_tap(&health, note.clone()));
        assert_eq!(
            enqueued.load(Ordering::Relaxed),
            1,
            "**a halted tap enqueues nothing** — this is the row"
        );
        assert_eq!(
            health.tap_suppressed_halted().load(Ordering::Relaxed),
            1,
            "and the request it did not send is counted rather than dropped in silence"
        );

        // Twice, so the cell is a count of what the halt cost rather than a flag saying it happened.
        assert!(!fire_autopropose_tap(&health, note));
        assert_eq!(enqueued.load(Ordering::Relaxed), 1);
        assert_eq!(health.tap_suppressed_halted().load(Ordering::Relaxed), 2);
    }
}

#[cfg(test)]
mod attest_warranted_tests {
    use super::{attest_is_live, attest_warranted, ATTEST_MAX_BLOCK_AGE_MS};
    use std::collections::BTreeMap;

    /// **A replayed block is not answered (#223)**: one older than the round's stall bound is history a
    /// returning validator is catching up on, and answering it put a block at every height it had missed.
    #[test]
    fn a_block_older_than_the_round_is_not_answered() {
        let now = 1_000_000_000;
        assert!(attest_is_live(now, now));
        assert!(attest_is_live(now - ATTEST_MAX_BLOCK_AGE_MS, now));
        assert!(!attest_is_live(now - ATTEST_MAX_BLOCK_AGE_MS - 1, now));
        assert!(
            !attest_is_live(now - 60_000, now),
            "a minute-old block is history"
        );
        assert!(
            attest_is_live(now + 5_000, now),
            "a peer's clock ahead of ours must not silence us"
        );
    }

    /// **A block stamped in the far future is not live** (W-N14-02).
    ///
    /// The forward bound is the half that was missing: `now.saturating_sub(block_ms)` gives `0` for any
    /// stamp ahead of `now`, and `0 <= ATTEST_MAX_BLOCK_AGE_MS` is true, so a block dated in the year
    /// 3000 was indistinguishable from one dated now — the tap answered every replay of it, which is
    /// precisely the amplification the backstop bound exists to prevent.
    ///
    /// Falsified by restoring the one-sided form, under which every assertion below is `true`.
    #[test]
    fn a_block_stamped_far_in_the_future_is_not_answered() {
        let now = 1_000_000_000;
        assert!(
            !attest_is_live(now + ATTEST_MAX_BLOCK_AGE_MS + 1, now),
            "a block past the forward window is not a live round"
        );
        assert!(
            !attest_is_live(now + 86_400_000, now),
            "a block dated a day ahead must not prompt an attestation"
        );
        // The wire extremes must not wrap: `i64::MAX` saturates rather than overflowing into the
        // window, and `i64::MIN` is history as far as the age bound is concerned.
        assert!(!attest_is_live(i64::MAX, now), "the far end of the range");
        assert!(!attest_is_live(i64::MIN, now), "and the near end");
    }

    #[test]
    fn any_remote_block_at_a_new_height_is_a_reason_to_attest() {
        let me = vec![1u8; 65];
        let other = vec![2u8; 65];

        // Another validator's block — whether a state transition or its attestation — is a reason for us
        // to add ours: the fringe needs the attestations to see each other.
        assert!(attest_warranted(&me, &other, 7, None));
        assert!(attest_warranted(&me, &other, 7, Some(6)));
        // Our own block already attests to itself.
        assert!(!attest_warranted(&me, &me, 7, None));
        assert!(!attest_warranted(&me, &me, 7, Some(6)));
    }

    /// A height this node has already answered is not answered again: the tap bounds itself to one
    /// request per remote height, so a burst of blocks at one height (the fan-out that made three
    /// validators produce 276 blocks in a minute, #70) enqueues one proposal, not one per block.
    #[test]
    fn a_height_already_answered_is_not_answered_again() {
        let me = vec![1u8; 65];
        let other = vec![2u8; 65];

        assert!(!attest_warranted(&me, &other, 7, Some(7)));
        assert!(!attest_warranted(&me, &other, 6, Some(7)));
        // A strictly newer height still is.
        assert!(attest_warranted(&me, &other, 8, Some(7)));
    }

    /// **The falsifier for the C192 fix.** A per-*height* gate answers only the first block of a round
    /// that comes to rest at one height — the measured `n149` shape (genesis plus N blocks, all at
    /// height 1, no height 2) — and seals it, because nothing above that height can be produced once
    /// every node's tap has refused the height. Keyed per-*sender* instead, the gate answers each
    /// peer's block at the resting height, so the round advances; a burst is still bounded to one
    /// request per sender per height.
    #[test]
    fn a_round_that_comes_to_rest_at_one_height_is_answered_for_every_peer() {
        let me = vec![1u8; 65];
        let peers: Vec<Vec<u8>> = (2..9u8).map(|i| vec![i; 65]).collect();

        // Three or more validators: seven peers' attestations, all at height 1. Per-sender, every one
        // is a reason to attest, so the round advances instead of sealing.
        let mut answered: BTreeMap<Vec<u8>, i64> = BTreeMap::new();
        let count = peers
            .iter()
            .filter(|p| {
                let last = answered.get(*p).copied();
                let w = attest_warranted(&me, p, 1, last);
                if w {
                    answered.insert((*p).clone(), 1);
                }
                w
            })
            .count();
        assert_eq!(
            count, 7,
            "every peer's block at the resting height is answered, so the round is not sealed"
        );

        // And the bound survives: the same peer at the same height is not answered twice, while its
        // strictly newer height still is.
        assert!(
            !attest_warranted(&me, &peers[0], 1, answered.get(&peers[0]).copied()),
            "a peer at a height already answered is not answered again"
        );
        assert!(
            attest_warranted(&me, &peers[0], 2, answered.get(&peers[0]).copied()),
            "a peer's strictly newer height still is"
        );

        // The control: the same number of blocks, one height apart — a two-validator net's shape. Still
        // all answered; the per-sender gate does not change this.
        let mut last: Option<i64> = None;
        let answered = (1..=7)
            .filter(|h| {
                let w = attest_warranted(&me, &peers[0], *h, last);
                if w {
                    last = Some(*h);
                }
                w
            })
            .count();
        assert_eq!(
            answered, 7,
            "when the height keeps advancing, every block is a reason to attest"
        );
    }
}

#[cfg(test)]
mod admin_bind_tests {
    use super::admin_bind_host;

    /// **The regression test for AUDIT C112.** The admin HTTP server carries an unauthenticated
    /// `POST /api/propose`, so its bind address is a security property and not a preference: with the
    /// default configuration every node in the network used to publish it.
    ///
    /// Both arms are asserted. Only testing the default would leave the opt-in free to become a no-op
    /// — an operator who sets the flag and still gets loopback has lost the browser-wallet access the
    /// flag promises, and that failure is silent.
    #[test]
    fn the_admin_server_binds_loopback_unless_the_operator_asks_otherwise() {
        assert_eq!(
            admin_bind_host("0.0.0.0", false),
            "127.0.0.1",
            "the default must not publish the unauthenticated propose endpoint on a wildcard address"
        );
        assert_eq!(
            admin_bind_host("0.0.0.0", true),
            "0.0.0.0",
            "the opt-in must actually publish, or the browser-wallet path it exists for is broken"
        );
        // A host that is already loopback stays where it is either way.
        assert_eq!(admin_bind_host("127.0.0.1", false), "127.0.0.1");
        assert_eq!(admin_bind_host("127.0.0.1", true), "127.0.0.1");
        // A specific interface is preserved under the opt-in, not widened to the wildcard.
        assert_eq!(admin_bind_host("10.0.0.5", true), "10.0.0.5");
    }
}
