//! Shared harness for node-level integration tests.
//!
//! Assembles a real standalone node in-process (mirroring `main.rs`: `Configuration::build` →
//! `node_environment::create` → `setup_node_program` → `serve`) over an ephemeral data dir and
//! loopback ports, then drives its gRPC + HTTP surfaces.
//!
//! Each `node/tests/*.rs` binary compiles this module separately, so helpers a given binary does not
//! use would warn there — hence the module-wide allowance.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rchain_casper::conf::{ShardMemberships, ShardSpec};
use rchain_casper::runtime_manager::RuntimeManager;
use rchain_casper::validator_identity::ValidatorIdentity;
use rchain_comm::peer_node::{NodeIdentifier, PeerNode};
use rchain_models::block::state_hash::StateHash;
use rchain_node::configuration::configuration::parse_defaults;
use rchain_node::configuration::hocon::node_conf_from_hocon;
use rchain_node::configuration::model::NodeConf;
use rchain_node::runtime::node_environment;
use rchain_node::runtime::node_runtime::{setup_node_program, NodeProgram};
use rchain_rholang::native_state::{DevVesting, NativeSystemState, PosParams};
use rchain_shared::base16;
use rchain_shared::log::{Log, LogSource};

/// The default secp256k1 validator private key (hex), port of `ConstructDeploy.defaultSec`.
pub const VALIDATOR_PRIV_HEX: &str =
    "a68a6e6cca30f81bd24a719f3145d20e8424bd7b396309b0708a16c7d8000b76";

/// A second valid secp256k1 scalar, for the two-validator restore tests. It is a different key
/// from [`VALIDATOR_PRIV_HEX`], and a chain that must accept its proposal has to bond it at genesis.
pub const SECOND_VALIDATOR_PRIV_HEX: &str =
    "1111111111111111111111111111111111111111111111111111111111111111";

fn state_hash_from_hex(hex: &str) -> StateHash {
    let bytes = base16::decode(hex).expect("state hash is base16");
    StateHash::from_slice(&bytes)
}

/// The base16 public key for a private-key hex string.
pub fn validator_pubkey_hex(private_hex: &str) -> String {
    let identity = ValidatorIdentity::from_hex(private_hex).expect("validator identity");
    base16::encode(identity.public_key.bytes())
}

/// The deployer (validator-0) REV address, funded in the genesis wallets file. Derived from
/// `VALIDATOR_PRIV_HEX`'s secp256k1 pubkey; a deploy signed with `VALIDATOR_PRIV_HEX` pays phlo from
/// this vault.
pub const DEPLOYER_REV_ADDR: &str = "11112VYAt8rUGNRRZX3eJdgagaAhtWTK8Js7F7X5iqddMVqyDTtYau";

/// Build a standalone `NodeConf` bound to 5 loopback ports `[http, admin-http, grpc-internal,
/// protocol, grpc-external]`, with a bonded validator (`VALIDATOR_PRIV_HEX`) and a funded deployer
/// wallet so signed deploys can pay phlo.
pub fn deploy_conf(dir: &Path, ports: &[u16]) -> NodeConf {
    assert!(
        ports.len() >= 5,
        "need [http, admin-http, grpc-internal, protocol, grpc-external]"
    );
    let mut conf = standalone_conf(dir, &ports[0..4], Some(VALIDATOR_PRIV_HEX));
    conf.api_server.port_grpc_external = ports[4] as i32;
    let wallets = conf
        .casper
        .shards
        .primary()
        .genesis_block_data
        .wallets_file
        .clone();
    std::fs::write(&wallets, format!("{DEPLOYER_REV_ADDR},1000000000000\n"))
        .expect("write wallets");
    conf
}

/// A multi-threaded tokio runtime with a large per-worker stack. The genesis blessed terms recurse
/// deeper than the default 2 MiB worker stack allows (matching the node binary's runtime).
pub fn test_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .thread_stack_size(32 * 1024 * 1024)
        .enable_all()
        .build()
        .expect("build test runtime")
}

/// Create a temporary directory for a test (caller removes it after dropping the node).
pub fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rchain-it-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Allocate `n` free loopback TCP ports. The ports are released when the returned listeners drop.
pub fn free_ports(n: usize) -> Vec<u16> {
    let mut listeners = Vec::with_capacity(n);
    let mut ports = Vec::with_capacity(n);
    for _ in 0..n {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        ports.push(listener.local_addr().expect("local addr").port());
        listeners.push(listener);
    }
    ports
}

/// Build a standalone `NodeConf` bound to loopback with the given `[http, admin-http, grpc-internal,
/// protocol]` ports. When `validator_hex` is set, a matching bonds file is written and wired into
/// genesis so the validator is bonded.
pub fn standalone_conf(dir: &Path, ports: &[u16], validator_hex: Option<&str>) -> NodeConf {
    assert!(
        ports.len() >= 4,
        "need [http, admin-http, grpc-internal, protocol] ports"
    );
    let defaults = parse_defaults(dir.to_str().unwrap()).expect("parse defaults");
    let mut conf = node_conf_from_hocon(&defaults).expect("node conf from hocon");

    conf.storage.data_dir = dir.to_path_buf();
    conf.api_server.host = "127.0.0.1".to_string();
    conf.api_server.port_http = ports[0] as i32;
    conf.api_server.port_admin_http = ports[1] as i32;
    conf.api_server.port_grpc_internal = ports[2] as i32;
    conf.protocol_server.port = ports[3] as i32;

    conf.standalone = true;
    // **A test node is a dev network**, exactly as `tools/devnet.sh` treats every node it starts.
    // This is what exempts it from the production-genesis gate: Law 78 refuses to create a genesis
    // outside dev mode without a `dev-tax-address`, and a test chain is not a network that owes one
    // (its allocation is covered by `casper`'s own tests, not here).
    conf.dev_mode = true;
    conf.protocol_server.no_upnp = true;
    conf.protocol_server.host = Some("127.0.0.1".to_string());

    if let Some(hex) = validator_hex {
        conf.casper.validator_private_key = Some(hex.to_string());
        let identity = ValidatorIdentity::from_hex(hex).expect("validator identity");
        let pub_hex = base16::encode(identity.public_key.bytes());
        let bonds = dir.join("bonds.txt");
        std::fs::write(&bonds, format!("{pub_hex} 100\n")).expect("write bonds file");
        conf.casper
            .shards
            .primary_mut()
            .genesis_block_data
            .bonds_file = bonds.to_string_lossy().into_owned();
    }

    // `create_genesis_block` calls `vault_parser::parse` (not `parse_if_exists`), so the wallets
    // file must exist — an empty one yields no initial vaults.
    let wallets = dir.join("wallets.txt");
    if !wallets.exists() {
        std::fs::write(&wallets, "").expect("write wallets file");
    }
    conf.casper
        .shards
        .primary_mut()
        .genesis_block_data
        .wallets_file = wallets.to_string_lossy().into_owned();

    conf
}

/// A joining node's config: the same loopback setup as [`standalone_conf`], but `standalone =
/// false`, a bootstrap peer, and its **own** discovery port — the default 40404 is a real listener,
/// so two in-process nodes otherwise collide.
pub fn joiner_conf(dir: &Path, ports: &[u16], bootstrap: &PeerNode) -> NodeConf {
    assert!(
        ports.len() >= 5,
        "need [http, admin-http, grpc-internal, protocol, discovery] ports"
    );
    let mut conf = standalone_conf(dir, &ports[0..4], None);
    conf.standalone = false;
    // A joiner does not create a genesis, so the production-genesis gate is never reached.
    conf.dev_mode = false;
    conf.protocol_client.bootstrap = bootstrap.clone();
    conf.peers_discovery.port = ports[4] as i32;
    if ports.len() >= 6 {
        conf.api_server.port_grpc_external = ports[5] as i32;
    }
    conf
}

/// What a joiner's local genesis descriptors claim, for the LFS restore matrix.
#[derive(Clone, Debug)]
pub struct JoinerSpec {
    /// The local `dev-tax-address`, or `None` for no local claim.
    pub recipient: Option<String>,
    /// The local wallets file's rows, or `None` to leave the harness's empty file.
    pub wallets_rows: Option<String>,
    /// Whether a bonds file is present — `genesis_descriptors_from_config` needs it to derive.
    pub bonds: bool,
    /// Whether this joiner is a bonded validator (second key; the genesis bonds include it).
    pub bonded: bool,
}

impl JoinerSpec {
    pub fn apply(&self, conf: &mut NodeConf, dir: &Path) {
        if self.bonded {
            conf.casper.validator_private_key = Some(SECOND_VALIDATOR_PRIV_HEX.to_string());
        }
        let gbd = &mut conf.casper.shards.primary_mut().genesis_block_data;
        gbd.dev_tax_address = self.recipient.clone().unwrap_or_default();
        if self.bonds {
            let mut rows = format!("{} 100\n", validator_pubkey_hex(VALIDATOR_PRIV_HEX));
            if self.bonded {
                rows.push_str(&format!(
                    "{} 100\n",
                    validator_pubkey_hex(SECOND_VALIDATOR_PRIV_HEX)
                ));
            }
            let path = dir.join("bonds.txt");
            std::fs::write(&path, rows).expect("write joiner bonds");
            gbd.bonds_file = path.to_string_lossy().into_owned();
        }
        if let Some(rows) = &self.wallets_rows {
            let path = dir.join("wallets.txt");
            std::fs::write(&path, rows).expect("write joiner wallets");
            gbd.wallets_file = path.to_string_lossy().into_owned();
        }
    }
}

/// Poll `/api/status` until `latestBlockNumber` reaches `height`.
pub async fn wait_for_height(base: &str, height: i64, timeout: Duration) {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Ok(resp) = client.get(format!("{base}/api/status")).send().await {
            if let Ok(status) = resp.json::<serde_json::Value>().await {
                let reached = status
                    .get("latestBlockNumber")
                    .and_then(|n| n.as_i64())
                    .unwrap_or(-1);
                if reached >= height {
                    return;
                }
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the node at {base} never reached height {height}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Poll the admin status until the LFS sync task has returned (`sync == null`). A named failure is
/// reported immediately rather than waiting for the timeout.
pub async fn wait_for_sync_cleared(admin_base: &str, timeout: Duration) {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Ok(resp) = client
            .get(format!("{admin_base}/api/v1/status/operator"))
            .send()
            .await
        {
            if let Ok(status) = resp.json::<serde_json::Value>().await {
                match status.get("sync") {
                    Some(serde_json::Value::Null) => return,
                    Some(sync) => {
                        if let Some(failure) = sync.get("lastFailure").and_then(|f| f.as_str()) {
                            panic!("the LFS sync task failed: {failure}");
                        }
                    }
                    None => {}
                }
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the LFS sync task at {admin_base} never returned"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// A log that records every line, so a test can observe a refusal the node latches rather than
/// returns (the launch task's error is logged and the process is asked to stop).
#[derive(Default)]
pub struct RecordingLog {
    messages: std::sync::Mutex<Vec<String>>,
}

impl RecordingLog {
    pub fn messages(&self) -> Vec<String> {
        self.messages.lock().expect("not poisoned").clone()
    }
}

impl Log for RecordingLog {
    fn is_trace_enabled(&self, _source: LogSource) -> bool {
        false
    }
    fn trace(&self, _source: LogSource, _msg: &str) {}
    fn debug(&self, _source: LogSource, _msg: &str) {}
    fn info(&self, _source: LogSource, msg: &str) {
        self.messages
            .lock()
            .expect("not poisoned")
            .push(msg.to_string());
    }
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

/// A running in-process node: the served-program task handle and the node identifier.
pub struct TestNode {
    pub handle: tokio::task::JoinHandle<Result<(), String>>,
    pub id: NodeIdentifier,
    pub grpc_port: u16,
    pub http_port: u16,
    /// The admin HTTP port — the listener that binds loopback by default and carries the surfaces
    /// that act with the node's own key (`/api/propose`, and since AUDIT C121 the cross-shard
    /// transaction routes). Read from the config rather than passed in, so a test that needs it does
    /// not have to thread another argument through `start`.
    pub admin_port: u16,
    /// The primary shard's runtime manager, from the running program. A test reads the native
    /// state the node actually installed through it, rather than opening the node's LMDB directory
    /// behind a live process.
    pub runtime: Arc<RuntimeManager>,
    /// Every line this node's logger was given, including a launch refusal that is logged rather
    /// than returned through `serve`.
    pub log: Arc<RecordingLog>,
    /// The node's stop path (AUDIT C144), held for the node's lifetime. It is a field rather than a
    /// local in `start` because a **dropped sender resolves every listener's `stop_requested` at
    /// once** — letting it fall out of scope would stop the node the instant it started, and the
    /// symptom (every test timing out on a node that answers nothing) would point at everything
    /// except this. Tests that do not stop the node never touch it.
    stop: tokio::sync::watch::Sender<bool>,
    /// **One heavyweight node program at a time** — held from setup until the node is dropped.
    ///
    /// See [`heavy_node_lock`]. It is a field for the same reason `stop` is: the lock has to live
    /// exactly as long as the node, and a local in `start` would release it while the node ran.
    ///
    /// An `Arc` because a test that runs **two** nodes ([`start_pair`]) takes the lock *once* and
    /// hands the same guard to both, so the pair holds it until the last of them drops.
    _heavy: Arc<tokio::sync::OwnedMutexGuard<()>>,
}

/// **One heavyweight node program at a time.**
///
/// Every [`start`] builds a full node: an LMDB environment with a large map, a Kademlia gRPC server,
/// an HTTP API, and the OCapN listener. `cargo test` runs one binary's tests on parallel threads, so
/// the OCapN listener's nine of them start nine nodes at once — and two things go wrong that are
/// **not** defects in the node, which is what makes them worth a lock rather than a retry:
///
/// * the runner runs out of address space for the maps, and the failure is
///   `open LMDB environment reporting: Cannot allocate memory` — measured in CI on 2026-10-07, and
///   locally;
/// * [`free_ports`] *releases* the ports it probed, so two tests that start at the same moment can be
///   handed the same one: the second fails with `Kademlia RPC server failed: Address already in use`.
///
/// Both make a red build say something untrue. Holding the lock for the node's whole life means a
/// test that starts a node runs alone from setup to teardown. It serialises within one test binary —
/// which is where both failures happen, because the binaries themselves run in sequence — and a test
/// that needs two nodes takes the lock once and shares the guard ([`start_pair`]), so there is
/// nothing for it to deadlock against.
fn heavy_node_lock() -> Arc<tokio::sync::Mutex<()>> {
    static LOCK: std::sync::OnceLock<Arc<tokio::sync::Mutex<()>>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

impl TestNode {
    /// The node's live native state, through the primary shard's runtime manager.
    pub fn native_state(&self) -> NativeSystemState {
        NativeSystemState::new(self.runtime.runtime().native_store())
    }

    /// The `pos:dev-vesting` leaf the node installed, if any.
    pub async fn dev_vesting(&self) -> Option<DevVesting> {
        self.native_state()
            .dev_vesting()
            .await
            .expect("read the dev vesting leaf")
    }

    /// The node's live `PosParams` record.
    pub async fn pos_params(&self) -> PosParams {
        self.native_state().params().await.expect("read PosParams")
    }

    /// The allocation leaf at a specific committed state, named by the base16 `postStateHash` a
    /// block-info response carries. This is the read a restored joiner needs: its live hot store is
    /// not guaranteed to be folded onto the restored root yet.
    pub async fn dev_vesting_at_hex(&self, hex: &str) -> Option<DevVesting> {
        self.runtime
            .committed_dev_vesting(&state_hash_from_hex(hex))
            .await
            .expect("read the dev vesting leaf at the committed state")
    }

    /// The params record at a specific committed state.
    pub async fn params_at_hex(&self, hex: &str) -> PosParams {
        self.runtime
            .committed_params(&state_hash_from_hex(hex))
            .await
            .expect("read PosParams at the committed state")
    }

    /// Abort the server task outright — the blunt stop, for a test that wants the process killed
    /// the way a `SIGKILL` does.
    ///
    /// Most tests want [`TestNode::shutdown`] instead: this leaves the listeners' detached tasks
    /// running on the test's runtime, and a caller that removes the data directory immediately
    /// afterwards can race a writer still holding LMDB.
    pub fn kill(self) {
        self.handle.abort();
    }

    /// Stop the node the way an operator's `SIGTERM` does, **waiting for `serve` to return**.
    ///
    /// Every listener is told to drain; the handle is awaited so the caller may remove the data
    /// directory without a detached task touching the store after it is gone. The timeout is a
    /// tripwire for a listener that never drains, not a performance budget; a stuck node is aborted
    /// and still awaited so the record is a stop, not a panic.
    pub async fn shutdown(mut self) {
        let _ = self.stop.send(true);
        if tokio::time::timeout(Duration::from_secs(30), &mut self.handle)
            .await
            .is_err()
        {
            self.handle.abort();
            let _ = (&mut self.handle).await;
        }
    }

    /// Ask the node to stop the way an operator's `SIGTERM` does: every listener is told, they
    /// drain, and `serve` returns. Unlike [`TestNode::shutdown`] this is the graceful path, so a
    /// test can await the handle afterwards and see what the node reported.
    pub fn request_stop(&self) {
        let _ = self.stop.send(true);
    }
}

/// Initialize the environment, assemble the node, and start serving it.
///
/// **Taken one at a time** — see [`heavy_node_lock`], which is acquired first and held until the
/// returned node is dropped.
pub async fn start(conf: &NodeConf, grpc_port: u16, http_port: u16) -> TestNode {
    let heavy = Arc::new(heavy_node_lock().lock_owned().await);
    build(conf, grpc_port, http_port, heavy).await
}

/// Start a standalone node with its ports allocated **inside** the heavyweight lock.
///
/// [`free_ports`] releases the listeners it probes, so a caller that allocates before [`start`]
/// takes the lock can be handed a port an earlier node already bound — the collision class the
/// harness's own lock exists to prevent, left open by the allocation happening outside it. Here the
/// allocation happens while every earlier node is still alive and holding the lock, so the ephemeral
/// range cannot return one of their ports. The tests that boot a node without a custom layout should
/// use this rather than `free_ports` + `standalone_conf` + `start`.
pub async fn start_standalone(dir: &Path, validator_hex: Option<&str>) -> (TestNode, Vec<u16>) {
    let heavy = Arc::new(heavy_node_lock().lock_owned().await);
    let ports = free_ports(4);
    let conf = standalone_conf(dir, &ports, validator_hex);
    let node = build(&conf, ports[2], ports[0], heavy).await;
    (node, ports)
}

/// **Wait for the node's HTTP surface, and name the reason if the node is already gone.**
///
/// The readiness loops in the node tests reported only "HTTP server did not come up" — the same
/// message for a port collision, a store error, a returned `serve` error and a slow machine.
/// `serve` returns its error on the join handle, so a finished task is checked every iteration and
/// its reason becomes the failure; a node that is merely slow still gets the deadline. The deadline
/// is a tripwire (a local boot is under a second), not a performance budget.
pub async fn wait_for_http(node: &mut TestNode, base: &str) -> String {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(resp) = client.get(format!("{base}/version")).send().await {
            if resp.status().is_success() {
                return resp.text().await.unwrap_or_default();
            }
        }
        if node.handle.is_finished() {
            match (&mut node.handle).await {
                Ok(Ok(())) => panic!("the node stopped before its HTTP server came up"),
                Ok(Err(e)) => panic!("the node failed to start: {e}"),
                Err(e) => panic!("the node task panicked: {e}"),
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "HTTP server did not come up within 30s"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// **Two nodes at once**, for the tests that need one node to *reach* another rather than a test
/// client to reach one.
///
/// [`start`] takes the heavy lock for the node's whole life, so calling it twice in one test would
/// wait for ever on the first node. This takes the lock once and shares the guard between the pair,
/// which is the same serialisation against every other test with neither node waiting on the other.
///
/// The two nodes must be given **disjoint** ports and data directories — `free_ports` and
/// `temp_dir` twice, not once.
pub async fn start_pair(
    first: (&NodeConf, u16, u16),
    second: (&NodeConf, u16, u16),
) -> (TestNode, TestNode) {
    let heavy = Arc::new(heavy_node_lock().lock_owned().await);
    let a = build(first.0, first.1, first.2, heavy.clone()).await;
    let b = build(second.0, second.1, second.2, heavy).await;
    (a, b)
}

/// **Two nodes, with the second started only after the first is ready.** The LFS restore tests need
/// the bootstrap's genesis (and, for a snapshot, its finalized fringe) to exist before the joiner
/// dials; starting both immediately is the empty-fringe race (#100). The readiness closure runs
/// while the shared heavy guard is held, so nothing else can start a node in between.
pub async fn start_pair_after<F, Fut>(
    first: (&NodeConf, u16, u16),
    ready: F,
    second: (&NodeConf, u16, u16),
) -> (TestNode, TestNode)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let heavy = Arc::new(heavy_node_lock().lock_owned().await);
    let a = build(first.0, first.1, first.2, heavy.clone()).await;
    ready().await;
    let b = build(second.0, second.1, second.2, heavy).await;
    (a, b)
}

/// The body of [`start`], with the heavy guard already in hand so a pair can share it.
async fn build(
    conf: &NodeConf,
    grpc_port: u16,
    http_port: u16,
    heavy: Arc<tokio::sync::OwnedMutexGuard<()>>,
) -> TestNode {
    let id = node_environment::create(conf).expect("node environment");
    let log = Arc::new(RecordingLog::default());
    // **Setup runs on a worker, not the caller's future.** `test_runtime` gives its workers the
    // node binary's 32 MiB stack because genesis normalization recurses deeply; before this, the
    // pre-await part of `setup_node_program` executed on the libtest thread (2 MiB) whenever a test
    // used `test_runtime().block_on(...)`. The node's own runtime contract is the stack the work
    // runs on, not the stack of whoever happened to start it.
    let setup_conf = conf.clone();
    let setup_id = id.clone();
    let setup_log = log.clone();
    let program: NodeProgram =
        tokio::spawn(async move { setup_node_program(&setup_conf, &setup_id, setup_log).await })
            .await
            .expect("setup task")
            .expect("setup node program");
    let runtime = program.primary_runtime.clone();
    let (stop, stop_rx) = tokio::sync::watch::channel(false);
    let handle = tokio::spawn(program.serve(stop_rx));
    TestNode {
        handle,
        id,
        grpc_port,
        http_port,
        admin_port: conf.api_server.port_admin_http as u16,
        runtime,
        log,
        stop,
        _heavy: heavy,
    }
}

/// A standalone **gateway** `NodeConf`: one node that is a member of two shards (`/root` and
/// `/root/child`), each with its own genesis — bonds and a wallet funding the validator's REV
/// address, so the cross-shard escrow can pay phlo and hold a balance on both.
///
/// `propose-on-deploy` is on because a cross-shard leg is an ordinary deploy: it takes effect when a
/// block containing it is produced, which is what the gateway's coordinator waits for.
pub fn gateway_conf(dir: &Path, ports: &[u16], validator_hex: &str) -> NodeConf {
    gateway_conf_with_txn_api(dir, ports, validator_hex, true)
}

/// As [`gateway_conf`], with the cross-shard transaction API switched on or off — the flag an
/// operator controls, and therefore a gate a test has to drive from a real config.
pub fn gateway_conf_with_txn_api(
    dir: &Path,
    ports: &[u16],
    validator_hex: &str,
    enable_txn_api: bool,
) -> NodeConf {
    assert!(
        ports.len() >= 4,
        "need [http, admin-http, grpc-internal, protocol]"
    );
    let mut conf = standalone_conf(dir, ports, Some(validator_hex));
    conf.propose_on_deploy = true;
    conf.api_server.enable_txn_api = enable_txn_api;

    let identity = ValidatorIdentity::from_hex(validator_hex).expect("validator identity");
    let pub_hex = base16::encode(identity.public_key.bytes());
    let primary = conf.casper.shards.primary().clone();

    let mut specs = Vec::new();
    for (index, (name, parent)) in [("root", "/"), ("child", "/root")].into_iter().enumerate() {
        // Each shard gets its own genesis files: its own bonds, and a wallet funding the validator
        // (the node's coordinator key) so it can escrow on that shard.
        let shard_dir = dir.join(format!("genesis-{name}"));
        std::fs::create_dir_all(&shard_dir).expect("genesis dir");
        let bonds = shard_dir.join("bonds.txt");
        std::fs::write(&bonds, format!("{pub_hex} 100\n")).expect("write bonds");
        let wallets = shard_dir.join("wallets.txt");
        std::fs::write(&wallets, format!("{DEPLOYER_REV_ADDR},1000000000000\n"))
            .expect("write wallets");

        let mut genesis = primary.genesis_block_data.clone();
        genesis.bonds_file = bonds.to_string_lossy().into_owned();
        genesis.wallets_file = wallets.to_string_lossy().into_owned();
        let _ = index;
        specs.push(
            ShardSpec::new(name.to_string(), parent.to_string(), genesis, 5).expect("shard spec"),
        );
    }
    conf.casper.shards = ShardMemberships::new(specs).expect("memberships");
    conf
}

/// Assemble a play runtime over a fresh in-memory store — the same shape `rholang/tests/common`
/// uses, for tests that need the interpreter but not a node. The corpus consumers in this crate call
/// the API's own codecs over terms, so they need a store to run a term in and nothing more.
pub async fn rho_runtime() -> rchain_rholang::runtime::RhoRuntime {
    use rchain_models::runtime::{BindPattern, ListParWithRandom, TaggedContinuation};
    use rchain_models::sorted::SortedProc;
    use rchain_rholang::storage::RhoMatch;
    use rchain_rspace::factory::create_history_repository;
    use rchain_rspace::hot_store::InMemHotStore;
    use rchain_rspace::rspace::RSpace;
    use rchain_shared::store_manager::InMemoryStoreManager;

    let manager = InMemoryStoreManager::default();
    let history = create_history_repository::<
        SortedProc,
        BindPattern,
        ListParWithRandom,
        TaggedContinuation,
    >(&manager, "rspace")
    .await
    .expect("history repository");
    let reader = history.get_history_reader(history.root()).await;
    let hot = Arc::new(InMemHotStore::new(reader.base()));
    let (play, _replay) = RSpace::create_with_replay(history.clone(), hot, Arc::new(RhoMatch));
    rchain_rholang::runtime::RhoRuntime::create(play, history, SortedProc::default())
        .await
        .expect("rho runtime")
}
