//! Command-line options (port of `Options.scala`).

use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};
use rchain_comm::peer_node::PeerNode;
use rchain_rholang::scheduler::EffectMode;

use super::super::hocon::parse_duration;

fn parse_duration_arg(s: &str) -> Result<Duration, String> {
    parse_duration(s).ok_or_else(|| {
        format!("'{s}': finite duration is expected, e.g. 20 seconds, 4 minutes, etc.")
    })
}

fn parse_peer_node_arg(s: &str) -> Result<PeerNode, String> {
    PeerNode::from_address(s).map_err(|_| "Can not parse the bootstrap address".to_string())
}

fn parse_base16(s: &str) -> Result<Vec<u8>, String> {
    rchain_shared::base16::decode(s)
        .ok_or_else(|| format!("Error parsing value. Invalid base16 encoding: {s}"))
}

/// A base16-decoded byte string (clap parses it via `FromStr`). A plain `Vec<u8>` field with
/// `value_parser = parse_base16` does not work: clap reads `Vec<T>` as "many occurrences of `T`",
/// so the parser's `Vec<u8>` output and the field's element type `u8` disagree and clap panics on
/// the downcast instead of reporting the mismatch (issue #14).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Base16(pub Vec<u8>);

impl std::str::FromStr for Base16 {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_base16(s).map(Base16)
    }
}

/// The effect-scheduler mode (Laws 20–25): `dfs` → [`EffectMode::Sequential`], `gate` →
/// [`EffectMode::Gate`], `relaxed` → [`EffectMode::Relaxed`], `relaxed-validated` →
/// [`EffectMode::RelaxedValidated`]. Parsed by clap via `FromStr` (the `Base16` newtype pattern).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectScheduler(pub EffectMode);

impl EffectScheduler {
    /// The flag/config-file name of the mode (the inverse of `FromStr`).
    pub fn as_str(&self) -> &'static str {
        match self.0 {
            EffectMode::Sequential | EffectMode::ForkJoin => "dfs",
            EffectMode::Gate => "gate",
            EffectMode::Relaxed => "relaxed",
            EffectMode::RelaxedValidated => "relaxed-validated",
        }
    }
}

impl std::str::FromStr for EffectScheduler {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        EffectMode::from_str(s).map(EffectScheduler)
    }
}

/// The CLI surface (port of the Scala `Options` scallop config).
// `name` is the binary's own, not `rchain`: clap prints it before the version — `rnode 0.1.0
// (<commit>)` — and the usage line already reads `rnode` because that is `argv[0]`, so `rchain` made
// `--version` name a program the operator did not run.
//
// **This note is a `//` comment and not a `///` doc comment on purpose.** clap's derive turns a
// multi-line doc comment into `long_about`, which `--help` prints — so writing the rationale as doc
// text leaked it into the operator-facing help, and it had to be moved here. The `version` argument
// is `cli_version()` rather than `node_version()` for the same class of reason: see that function.
#[derive(Parser, Debug)]
#[command(
    name = "rnode",
    version = crate::web::version_info::cli_version(),
    about = "rholo node | gRPC client",
    disable_help_flag = true
)]
pub struct Options {
    // Long-only on purpose: `-h` is a *real* flag in this CLI (`--grpc-host` at this level,
    // `--api-port-http` under `run`), so help is spelled out rather than abbreviated. `global = true`
    // is what gives every subcommand a `--help` of its own: the top-level `disable_help_flag = true`
    // (which is what frees `-h`) also means clap adds no help flag to a subcommand, so `rnode run
    // --help` was "unexpected argument '--help'" and the options of a subcommand were not
    // discoverable from the CLI at all (AUDIT C81). The action prints the help of the command it is
    // found in, so `rnode run --help` prints `run`'s options and exits 0. The help text states the
    // `-h` convention, because it is the one place an operator looks for it.
    #[arg(
        long = "help",
        global = true,
        action = clap::ArgAction::Help,
        help = "Print help for this command (`-h` is not help in this CLI: `--grpc-host` at this level, `--api-port-http` under `run`)"
    )]
    help: Option<bool>,
    /// Remote gRPC host for client calls.
    #[arg(short = 'h', long = "grpc-host", default_value = "localhost")]
    pub grpc_host: String,

    /// Remote gRPC port for client calls. Defaults to 40401 (external) for `deploy`, and 40402
    /// (internal) for `repl`/`propose`.
    #[arg(short = 'p', long = "grpc-port")]
    pub grpc_port: Option<i32>,

    /// Max inbound gRPC message size for client calls.
    #[arg(short = 's', long = "grpc-max-recv-message-size", default_value_t = 16 * 1024 * 1024)]
    pub grpc_max_recv_message_size: i32,

    /// Predefined set of defaults to use: default or docker.
    #[arg(long = "profile")]
    pub profile: Option<String>,

    #[command(subcommand)]
    pub subcommand: Commands,
}

#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Commands {
    /// Start RNode server.
    Run(Run),
    /// Generates a public/private key pair.
    Keygen {
        /// Folder to save keyfiles. Defaults to './'
        #[arg(default_value = "")]
        location: PathBuf,
    },
    /// View properties of the last finalized block.
    LastFinalizedBlock,
    /// Check if the given block has been finalized.
    IsFinalized {
        /// The hash value of the block to check.
        hash: String,
    },
    /// Starts a thin client REPL.
    Repl,
    /// Evaluate rholang in a file on an existing running node.
    Eval {
        #[arg(required = true)]
        file_names: Vec<String>,
        #[arg(long = "print-unmatched-sends-only")]
        print_unmatched_sends_only: bool,
    },
    /// Deploy a Rholang source file.
    Deploy {
        #[arg(long = "phlo-limit", required = true)]
        phlo_limit: i64,
        #[arg(long = "phlo-price", required = true)]
        phlo_price: i64,
        #[arg(long = "valid-after-block-number")]
        valid_after_block_number: Option<i64>,
        #[arg(long = "private-key")]
        private_key: Option<String>,
        #[arg(long = "private-key-path")]
        private_key_path: Option<PathBuf>,
        #[arg(long = "shard-id", default_value = "")]
        shard_id: String,
        #[arg(required = true)]
        location: String,
    },
    /// Returns the status of the deploy with provided signature.
    DeployStatus {
        #[arg(long = "deploy-signature", required = true, value_parser = clap::value_parser!(Base16))]
        deploy_signature: Base16,
    },
    /// View properties of a block.
    ShowBlock {
        /// The hash value of the block.
        hash: String,
    },
    /// View list of blocks.
    ShowBlocks {
        /// Lists blocks to the given depth.
        #[arg(long = "depth")]
        depth: Option<i32>,
    },
    /// DAG in DOT format.
    Vdag {
        #[arg(long = "depth")]
        depth: Option<i32>,
        #[arg(long = "showJustificationlines")]
        show_justification_lines: bool,
    },
    /// Machine Verifiable DAG.
    Mvdag,
    /// Listen for data at the specified name.
    ListenDataAtName {
        #[arg(short = 't', long = "type", required = true)]
        type_of_name: String,
        #[arg(short = 'c', long = "content", required = true)]
        content: Vec<String>,
    },
    /// Listen for continuation at the specified name.
    ListenContAtName {
        #[arg(short = 't', long = "type", required = true)]
        type_of_name: String,
        #[arg(short = 'c', long = "content", required = true)]
        content: Vec<String>,
    },
    /// Searches for a block containing the deploy with provided id.
    FindDeploy {
        #[arg(long = "deploy-id", required = true, value_parser = clap::value_parser!(Base16))]
        deploy_id: Base16,
    },
    /// Force Casper to propose a block.
    Propose {
        #[arg(long = "print-unmatched-sends")]
        print_unmatched_sends: bool,
    },
    /// Check bond status for a validator public key.
    BondStatus {
        #[arg(value_parser = clap::value_parser!(Base16))]
        validator_public_key: Base16,
    },
    /// Get RNode status information.
    Status,
    /// Verify a block's raw preimage from a node's `/api/block/{hash}/raw` response on stdin, printing
    /// the fields it proves (AUDIT C261). The reconciliation tool calls this rather than re-deriving
    /// the node's hash and signature rules itself.
    VerifyBlock,
    /// Verify a node's `/api/finality-attestation` response on stdin against the nonce this run asked
    /// for, printing the block and signer it proves (AUDIT C261).
    VerifyAttestation {
        /// The nonce this run asked for, hex. It is *not* read from the response: a verifier that
        /// took it from there would accept a captured attestation replayed from an earlier run.
        #[arg(long = "nonce", required = true)]
        nonce: String,
    },
}

/// The `run` subcommand options (port of `Options.run`).
#[derive(Parser, Debug)]
pub struct Run {
    /// Path to the configuration file for RNode server.
    #[arg(short = 'c', long = "config-file")]
    pub config_file: Option<PathBuf>,

    /// Number of threads allocated for the main scheduler (the tokio worker threads; defaults to
    /// the number of CPUs).
    #[arg(long = "thread-pool-size")]
    pub thread_pool_size: Option<i32>,

    /// Verbosity of the node's own log lines on stderr: `error`, `warn`, `info` (the default),
    /// `debug` or `trace`. **AUDIT C145:** before this there was no way to raise verbosity short of
    /// rebuilding — `debug` and `trace` were empty bodies in the only logger the node installs — so
    /// every diagnostic behind them was unreachable in a shipped binary.
    #[arg(long = "log-level")]
    pub log_level: Option<String>,

    /// The effect scheduler (Laws 20–25): `dfs` (default, the sequential DFS loop), `gate` (the
    /// DFS gate), `relaxed` (per-channel claim queues; off-chain only — a relaxed node refuses
    /// block-path deploy execution), or `relaxed-validated` (Laws 23–25: relaxed on the block
    /// path, validated against the sequential reference with sequential fallback). Falls back to
    /// the config file's `casper.effect-scheduler` (default `dfs`) when not given.
    #[arg(long = "effect-scheduler", value_parser = clap::value_parser!(EffectScheduler))]
    pub effect_scheduler: Option<EffectScheduler>,

    /// Start a stand-alone node.
    #[arg(short = 's', long = "standalone")]
    pub standalone: bool,

    /// Address of RNode to bootstrap from.
    #[arg(short = 'b', long = "bootstrap", value_parser = parse_peer_node_arg)]
    pub bootstrap: Option<PeerNode>,

    /// ID of the RChain network to connect to.
    #[arg(long = "network-id")]
    pub network_id: Option<String>,

    /// Make node automatically propose blocks.
    #[arg(long = "autopropose")]
    pub autopropose: bool,

    /// Propose a block immediately after a deploy is accepted.
    #[arg(long = "propose-on-deploy")]
    pub propose_on_deploy: bool,

    /// Attest to remote blocks, by proposing.
    ///
    /// With nothing of our own to include, that proposal is an empty attestation (`block_creator.rs`):
    /// the way a validator holding no deploys moves its latest message, and therefore the way a finality
    /// quorum forms when every deploy arrives at one node. **On by default** — a validator needs no
    /// `--autopropose` to be live.
    ///
    /// It fires on **any** remote block, not only deploy-bearing ones. The fringe rule needs a full
    /// partition — every justification sender's message seen by every bonded sender — which takes more
    /// than one round, because the attestations have to see each other. What bounds the traffic is the
    /// proposer's own guard, not this predicate: `suppress_attestation` refuses to attest once nothing
    /// unfinalized carries deploys, or while a supermajority is out of reach.
    #[arg(long = "attest-on-new-blocks")]
    pub attest_on_new_blocks: bool,

    /// Do not attest to remote blocks.
    ///
    /// Attestation is on by default; this turns it off for a validator that must not add blocks
    /// (a host too small to carry the attestation traffic, or a deliberately passive observer with a
    /// validator key).
    #[arg(long = "no-attest-on-new-blocks")]
    pub no_attest_on_new_blocks: bool,

    /// Start below the resource-guard floor. Explicit and logged; the chain-length floor is a policy
    /// table, and this flag is the operator's acceptance of starting under it.
    #[arg(long = "allow-under-minimum")]
    pub allow_under_minimum: bool,

    /// Disable UPnP.
    #[arg(long = "no-upnp")]
    pub no_upnp: bool,

    /// Host IP address changes dynamically.
    #[arg(long = "dynamic-ip")]
    pub dynamic_ip: bool,

    /// Number of validator identities to generate.
    #[arg(long = "autogen-shard-size")]
    pub autogen_shard_size: Option<i32>,

    /// Disable start from Last Finalized State.
    #[arg(long = "disable-lfs")]
    pub disable_lfs: bool,

    /// Address to bind RChain Protocol server.
    #[arg(long = "host")]
    pub host: Option<String>,

    /// Use random ports if configured ports are not free.
    #[arg(long = "use-random-ports")]
    pub use_random_ports: bool,

    /// Allow connections to peers with private network addresses (unused).
    #[arg(long = "allow-private-addresses")]
    pub allow_private_addresses: bool,

    /// Disable the node respond to export state requests.
    #[arg(long = "disable-state-exporter")]
    pub disable_state_exporter: bool,

    /// Default timeout for network calls.
    #[arg(long = "network-timeout", value_parser = parse_duration_arg)]
    pub network_timeout: Option<Duration>,

    /// Port used for node discovery (Kademlia).
    #[arg(long = "discovery-port")]
    pub discovery_port: Option<i32>,

    /// Peer discovery interval.
    #[arg(long = "discovery-lookup-interval", value_parser = parse_duration_arg)]
    pub discovery_lookup_interval: Option<Duration>,

    /// Peer discovery cleanup interval.
    #[arg(long = "discovery-cleanup-interval", value_parser = parse_duration_arg)]
    pub discovery_cleanup_interval: Option<Duration>,

    /// Check for first connection loop interval.
    #[arg(long = "discovery-init-wait-loop-interval", value_parser = parse_duration_arg)]
    pub discovery_init_wait_loop_interval: Option<Duration>,

    /// Peer discovery heartbeat batch size.
    #[arg(long = "discovery-heartbeat-batch-size")]
    pub discovery_heartbeat_batch_size: Option<i32>,

    /// gRPC port serving RChain Protocol messages.
    #[arg(short = 'p', long = "protocol-port")]
    pub protocol_port: Option<i32>,

    /// Maximum message size for gRPC transport server.
    #[arg(long = "protocol-grpc-max-recv-message-size")]
    pub protocol_grpc_max_recv_message_size: Option<i64>,

    /// Maximum size of messages received via transport streams.
    #[arg(long = "protocol-grpc-max-recv-stream-message-size")]
    pub protocol_grpc_max_recv_stream_message_size: Option<i64>,

    /// Chunk size for streaming packets between nodes.
    #[arg(long = "protocol-grpc-stream-chunk-size")]
    pub protocol_grpc_stream_chunk_size: Option<i32>,

    /// Number of connected peers picked for broadcasting/streaming.
    #[arg(long = "protocol-max-connections")]
    pub protocol_max_connections: Option<i32>,

    /// Number of incoming message consumers.
    #[arg(long = "protocol-max-message-consumers")]
    pub protocol_max_message_consumers: Option<i32>,

    /// Path to private key for TLS.
    #[arg(short = 'k', long = "tls-key-path")]
    pub tls_key_path: Option<PathBuf>,

    /// Path to X.509 certificate for TLS.
    #[arg(long = "tls-certificate-path")]
    pub tls_certificate_path: Option<PathBuf>,

    /// Use a non blocking secure random instance.
    #[arg(long = "tls-secure-random-non-blocking")]
    pub tls_secure_random_non_blocking: bool,

    /// Address to bind API servers.
    #[arg(long = "api-host")]
    pub api_host: Option<String>,

    /// Bind an OCapN listener on `host:port` (e.g. `127.0.0.1:22045`); off when unset. The only
    /// netlayer implemented is the OCapN project's `tcp-testing-only` — plain TCP, no encryption and
    /// no authentication (issue #249).
    #[arg(long = "ocapn-listen")]
    pub ocapn_listen: Option<String>,

    /// Bind the OCapN **`unix`** listener at this socket path; off when unset. Unlike
    /// `--ocapn-listen`, this transport authenticates by the socket's file mode (`0600`) rather than
    /// by a network address (issue #249).
    #[arg(long = "ocapn-listen-unix")]
    pub ocapn_listen_unix: Option<String>,

    /// Bind the OCapN **`noise`** listener on this `host:port`; off when unset. The transport a peer
    /// not on this host can reach: an authenticated, encrypted handshake with no certificate
    /// authority. Needs `--ocapn-identity-key` (issue #249).
    #[arg(long = "ocapn-listen-noise")]
    pub ocapn_listen_noise: Option<String>,

    /// Bind the OCapN **`websocket`** listener on this `host:port`; off when unset. The transport
    /// `@endo/ocapn` speaks, and weaker than `--ocapn-listen-noise`. Needs `--ocapn-identity-key`.
    #[arg(long = "ocapn-listen-websocket")]
    pub ocapn_listen_websocket: Option<String>,

    /// Where this node's OCapN identity is kept (64 bytes, mode `0600`); created on first use. Required
    /// by the `noise` and `websocket` listeners, which authenticate with it.
    #[arg(long = "ocapn-identity-key")]
    pub ocapn_identity_key: Option<String>,

    /// Port for external gRPC API.
    #[arg(short = 'e', long = "api-port-grpc-external")]
    pub api_port_grpc_external: Option<i32>,

    /// Port for internal gRPC API.
    #[arg(short = 'i', long = "api-port-grpc-internal")]
    pub api_port_grpc_internal: Option<i32>,

    /// Maximum message size for gRPC API server.
    #[arg(long = "api-grpc-max-recv-message-size")]
    pub api_grpc_max_recv_message_size: Option<i32>,

    /// Port for HTTP services.
    #[arg(short = 'h', long = "api-port-http")]
    pub api_port_http: Option<i32>,

    /// Port for admin HTTP services.
    #[arg(short = 'a', long = "api-port-admin-http")]
    pub api_port_admin_http: Option<i32>,

    /// The max block numbers you can acquire from api.
    #[arg(long = "api-max-blocks-limit")]
    pub api_max_blocks_limit: Option<i32>,

    /// Enable reporting endpoints.
    #[arg(long = "api-enable-reporting")]
    pub api_enable_reporting: bool,

    /// Relax CORS on the admin HTTP API (devnet / browser-wallet access only).
    #[arg(long = "api-enable-devnet-cors")]
    pub api_enable_devnet_cors: bool,

    /// Publish the admin HTTP API (`/api/propose`) on `--api-host` instead of loopback.
    ///
    /// Off by default: that API has no authentication and triggers block production, so binding it
    /// to a published interface is a deliberate act (AUDIT C112). Devnet and browser-wallet setups
    /// that need to reach it from off-host set this.
    #[arg(long = "api-enable-devnet-admin-public")]
    pub api_enable_devnet_admin_public: bool,

    /// Custom keepalive time.
    #[arg(long = "api-keep-alive-time", value_parser = parse_duration_arg)]
    pub api_keep_alive_time: Option<Duration>,

    /// Custom keepalive timeout.
    #[arg(long = "api-keep-alive-timeout", value_parser = parse_duration_arg)]
    pub api_keep_alive_timeout: Option<Duration>,

    /// Most aggressive keep-alive time clients are permitted.
    #[arg(long = "api-permit-keep-alive-time", value_parser = parse_duration_arg)]
    pub api_permit_keep_alive_time: Option<Duration>,

    /// Custom max connection idle time.
    #[arg(long = "api-max-connection-idle", value_parser = parse_duration_arg)]
    pub api_max_connection_idle: Option<Duration>,

    /// Custom max connection age.
    #[arg(long = "api-max-connection-age", value_parser = parse_duration_arg)]
    pub api_max_connection_age: Option<Duration>,

    /// Custom grace time for connection termination.
    #[arg(long = "api-max-connection-age-grace", value_parser = parse_duration_arg)]
    pub api_max_connection_age_grace: Option<Duration>,

    /// Path to data directory.
    #[arg(long = "data-dir")]
    pub data_dir: Option<PathBuf>,

    /// Name of the shard this node is connected to.
    #[arg(long = "shard-name")]
    pub shard_name: Option<String>,

    /// ID of the parent shard (`/` for the root). The full shard id is `{parent-shard-id}/{shard-name}`.
    #[arg(long = "parent-shard-id")]
    pub parent_shard_id: Option<String>,

    /// Base16 encoding of the public key for signing proposed blocks.
    #[arg(long = "validator-public-key")]
    pub validator_public_key: Option<String>,

    /// Path to the base16 encoded private key for signing proposed blocks.
    ///
    /// **The flag this replaced is gone, deliberately** (W-N11-05). `--validator-private-key <hex>`
    /// put the node's signing identity in `/proc/<pid>/cmdline`, where any local process may read it;
    /// it was hidden from the help text, which hides it from the operator and not from the host. The
    /// three other routes are unchanged — this path, the config file's `casper.validator-private-key`
    /// (not on a command line), and `RNODE_VALIDATOR_PASSWORD` for an encrypted PEM — so nothing on
    /// the command line carries the scalar, and a command line that still passes the old flag is
    /// refused at parse time rather than silently seeding a node whose secret is public.
    #[arg(long = "validator-private-key-path")]
    pub validator_private_key_path: Option<PathBuf>,

    /// Interval for the casper loop.
    #[arg(long = "casper-loop-interval", value_parser = parse_duration_arg)]
    pub casper_loop_interval: Option<Duration>,

    /// Timeout for blocks requests.
    #[arg(long = "requested-blocks-timeout", value_parser = parse_duration_arg)]
    pub requested_blocks_timeout: Option<Duration>,

    /// Maximum number of block parents.
    #[arg(long = "max-number-of-parents")]
    pub max_number_of_parents: Option<i32>,

    /// Fork choice stale threshold.
    #[arg(long = "fork-choice-stale-threshold", value_parser = parse_duration_arg)]
    pub fork_choice_stale_threshold: Option<Duration>,

    /// Interval for checking if fork choice tip is stale.
    #[arg(long = "fork-choice-check-if-stale-interval", value_parser = parse_duration_arg)]
    pub fork_choice_check_if_stale_interval: Option<Duration>,

    /// Synchrony constraint threshold (fraction of stake).
    #[arg(long = "synchrony-constraint-threshold")]
    pub synchrony_constraint_threshold: Option<f64>,

    /// How far ahead of the last finalized block the node is allowed to propose.
    #[arg(long = "height-constraint-threshold")]
    pub height_constraint_threshold: Option<i64>,

    /// Bonds file (genesis).
    #[arg(long = "bonds-file")]
    pub bonds_file: Option<String>,

    /// Wallets file (genesis).
    #[arg(long = "wallets-file")]
    pub wallets_file: Option<String>,

    /// Minimum bond accepted by the PoS contract.
    #[arg(long = "bond-minimum")]
    pub bond_minimum: Option<i64>,

    /// Genesis block number for hard fork.
    #[arg(long = "genesis-block-number")]
    pub genesis_block_number: Option<i64>,

    /// Maximum bond accepted by the PoS contract.
    #[arg(long = "bond-maximum")]
    pub bond_maximum: Option<i64>,

    /// Length of the validation epoch in blocks.
    #[arg(long = "epoch-length")]
    pub epoch_length: Option<i32>,

    /// Length of the quarantine time in blocks.
    ///
    /// Wired: the value reaches the native PoS contract's `PosParams`
    /// (`genesis/contracts.rs` → `native_state.rs`), where `withdraw`'s eligibility window is
    /// `block_number + quarantine_length`. (This comment said "not yet wired" until it was checked;
    /// the value flows through `node_launch.rs` to genesis.)
    #[arg(long = "quarantine-length")]
    pub quarantine_length: Option<i32>,

    /// Number of active validators.
    #[arg(long = "number-of-active-validators")]
    pub number_of_active_validators: Option<i32>,

    /// **The block producer's share of a deploy's burned phlo, in basis points** (B2, #150): 2500 pays
    /// the validator that signed a block a quarter of the fees that block consumed, before the rest of
    /// the epoch's burned phlo reaches the pot. A *genesis* parameter, so every node on a network must
    /// agree on it — see `docs/src/node/running-a-public-testnet.md`.
    #[arg(long = "executor-share")]
    pub executor_share: Option<i32>,

    /// **How long a validator may go without signing a block before an epoch stops paying it** (B4,
    /// #150), in heights; `0` is the contract's behaviour and the shipped default. It is the *knee* of
    /// the participation weight: the lag at which the weight reaches zero.
    #[arg(long = "absence-slack")]
    pub absence_slack: Option<i32>,

    /// **How far behind the last finalised fringe a validator's latest message may sit and still be
    /// paid its whole share** (B4, #150), in heights — the flat part of the weight, with the ramp
    /// running from here to `absence-slack`.
    ///
    /// **Unstated means the slack's binary rule, not zero.** A network whose config predates this key was
    /// running `grace == slack`, so that is what an omitted flag resolves to — a ramp from zero would be
    /// a rule change wearing a default, and it would land on exactly the networks that had armed the
    /// rule. `--absence-slack 0` disables the rule whatever this says.
    #[arg(long = "participation-grace")]
    pub participation_grace: Option<i32>,

    /// Public key for transfers from the PoS vault.
    ///
    /// Reserved: parsed and validated, but not yet wired to the native PoS contract
    /// (see `spec/RUST-FIRST.md`).
    #[arg(long = "pos-vault-pub-key")]
    pub pos_vault_pub_key: Option<String>,

    /// Public key to manage system contract updates.
    #[arg(long = "system-contract-pub-key")]
    pub system_contract_pub_key: Option<String>,

    /// Accepted for config compatibility: this port always serves Prometheus text at `GET /metrics`,
    /// so the setting is already satisfied and does not gate the endpoint.
    #[arg(long = "prometheus")]
    pub prometheus: bool,

    /// Refused at startup: this port has no InfluxDB metrics reporter to enable.
    #[arg(long = "influxdb")]
    pub influxdb: bool,

    /// Refused at startup: this port has no InfluxDB UDP metrics reporter to enable.
    #[arg(long = "influxdb-udp")]
    pub influxdb_udp: bool,

    /// Refused at startup: this port has no Zipkin span reporter to enable.
    #[arg(long = "zipkin")]
    pub zipkin: bool,

    /// Refused at startup: this port has no Sigar host-metrics collector to enable.
    #[arg(long = "sigar")]
    pub sigar: bool,

    /// Enable all developer tools.
    #[arg(long = "dev-mode")]
    pub dev_mode: bool,

    /// Private key for dummy deploys.
    #[arg(long = "deployer-private-key")]
    pub deployer_private_key: Option<String>,

    /// MinPhloPrice.
    #[arg(long = "min-phlo-price")]
    pub min_phlo_price: Option<i64>,

    /// **Broadcast a twin of every block this node creates** — the same block, equally signed, at the
    /// same `(sender, seq_num)`. A measurement instrument, not an operating mode: every peer will
    /// record this node as having equivocated and may take its bond (AUDIT C200).
    #[arg(long = "equivocation-injection")]
    pub equivocation_injection: bool,

    /// **Let this node reset itself when it can no longer extend the chain** (#320). When a divergence
    /// record's revalidation budget is spent the node refuses that block and its descendants for the
    /// process lifetime; with this switch it instead rewinds to its last finalised block — one a strict
    /// supermajority of stake finalised — resets its runtime to that state and re-syncs from there.
    /// **Off by default**, and it needs a bootstrap: rewinding leaves the node behind its peers on
    /// purpose, and the blocks above the anchor are re-requested from one.
    #[arg(long = "self-reset")]
    pub self_reset: bool,

    /// **Perturb this node's fringe records, so its merge answers differently from its peers'** — `0`
    /// (the default) injects nothing. A measurement instrument for the divergence class, and the
    /// staging instrument for the reconciliation drill: it makes four nodes reach four different merged
    /// states at one height, which is the shape of a chain that halts and cannot recover. Requires
    /// `--dev-mode`, and every perturbed write prints a line saying what it did.
    #[arg(long = "merge-divergence-injection")]
    pub merge_divergence_injection: Option<u8>,

    /// **A block hash to sync to when this node has no chain** — the anchor a reconciliation computes
    /// when a net has no finalised fringe to resync from. Empty is the ordinary path.
    ///
    /// The hash is named **in the fringe request** (C259a), and the bootstrap answers with that block's
    /// post-state **and the per-block fringe state of its ancestry** — the piece #139 added so a restored
    /// block can be replayed rather than re-derived. The earlier form of this flag fetched the block and
    /// patched a seed together locally, which left the ancestry empty: the node then restored the state
    /// and **stopped at the anchor**, silently near the tip and loudly on an older anchor
    /// (`regenerated mergeable channels…`). That is measured in
    /// `spec/audit/evidence/n-anchor-drill/run.md`.
    ///
    /// **What it still depends on.** A peer that does not know the request's `anchor` field answers with
    /// its own latest fringe — or, with finality frozen, the genesis block — so the recovery degrades to
    /// the ordinary path rather than failing. Every node in a shard runs the same binary, so in practice
    /// the responder is one that knows it.
    #[arg(long = "sync-anchor")]
    pub sync_anchor: Option<String>,

    /// Full genesis block hash of the network to join.
    #[arg(long = "expected-genesis-hash")]
    pub expected_genesis_hash: Option<String>,

    /// Public keys of the Coop multisig vault.
    ///
    /// **Wired, but not to what the name suggests.** In the blessed contract these keys own the Coop
    /// multisig vault that receives slashed stake (`Pos.rhox:122-128`); this port's native model has no
    /// multisig vault, so they are read as the initial *trusted stakeholder* set instead — the keys
    /// allowed to admit validators (`casper/src/genesis/mod.rs::build_pos_genesis`, and see
    /// `spec/audit/passes.md` §6). An operator setting them for slashing-vault control is also granting
    /// admission rights.
    #[arg(long = "pos-multi-sig-public-keys", num_args = 1..)]
    pub pos_multi_sig_public_keys: Option<Vec<String>>,

    /// How many confirmations are necessary to use multi-sig vault.
    ///
    /// Reserved: parsed and validated, but not yet wired to the native PoS contract
    /// (see `spec/RUST-FIRST.md`).
    #[arg(long = "pos-multi-sig-quorum")]
    pub pos_multi_sig_quorum: Option<i32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Options, clap::Error> {
        Options::try_parse_from(std::iter::once("rnode").chain(args.iter().copied()))
    }

    /// **The validator key cannot be given on the command line** (W-N11-05).
    ///
    /// The row: `--validator-private-key <hex>` put the node's signing identity in
    /// `/proc/<pid>/cmdline`, readable by any local process, and `hide = true` hid the flag from the
    /// help text rather than from the host. The route is gone — a secret reaches the node through a
    /// file path, the config file's own key, or an encrypted PEM's passphrase, none of which is argv —
    /// and this is the falsifier: the flag is a parse error, and the error points at the route that
    /// replaces it rather than leaving an operator to guess.
    ///
    /// It does not assert that no secret can appear in a *typed* command line (nothing can prevent
    /// that); it asserts the node refuses to take one from there, which is the half that is the
    /// node's to decide.
    #[test]
    fn the_validator_key_cannot_be_given_on_the_command_line() {
        let err = parse(&[
            "run",
            "--validator-private-key",
            "67e56582298859ddae725f972992a07c6c4fb9f62a8fff58ce3ca926a1063530",
        ])
        .expect_err("the headerless key route must be refused");
        let rendered = err.to_string();
        assert!(
            rendered.contains("--validator-private-key"),
            "the refusal names the flag it refuses: {rendered}"
        );
        assert!(
            rendered.contains("--validator-private-key-path"),
            "and points at the route that replaces it: {rendered}"
        );
    }

    #[test]
    fn find_deploy_parses_base16_id() {
        let opts = parse(&["find-deploy", "--deploy-id", "30440220"]).expect("parse");
        match &opts.subcommand {
            Commands::FindDeploy { deploy_id } => {
                assert_eq!(deploy_id.0, vec![0x30, 0x44, 0x02, 0x20]);
            }
            other => panic!("wrong subcommand: {other:?}"),
        }
    }

    #[test]
    fn deploy_status_parses_base16_signature() {
        let opts = parse(&["deploy-status", "--deploy-signature", "30440220"]).expect("parse");
        match &opts.subcommand {
            Commands::DeployStatus { deploy_signature } => {
                assert_eq!(deploy_signature.0, vec![0x30, 0x44, 0x02, 0x20]);
            }
            other => panic!("wrong subcommand: {other:?}"),
        }
    }

    #[test]
    fn bond_status_parses_base16_public_key() {
        let opts = parse(&["bond-status", "04f700a4"]).expect("parse");
        match &opts.subcommand {
            Commands::BondStatus {
                validator_public_key,
            } => {
                assert_eq!(validator_public_key.0, vec![0x04, 0xf7, 0x00, 0xa4]);
            }
            other => panic!("wrong subcommand: {other:?}"),
        }
    }

    #[test]
    fn base16_parser_rejects_invalid_input() {
        // Invalid base16 must be a parse error, not a panic (issue #14).
        assert!(parse(&["find-deploy", "--deploy-id", "zz"]).is_err());
        assert!(parse(&["deploy-status", "--deploy-signature", "zz"]).is_err());
        assert!(parse(&["bond-status", "zz"]).is_err());
    }

    /// AUDIT C81: every command accepts `--help`, and it prints *that* command's options.
    ///
    /// Before this, only the top-level `Options` carried a help flag — `disable_help_flag = true` is
    /// what frees `-h` for `--grpc-host`, and it also left every subcommand without one — so
    /// `rnode run --help` was `unexpected argument '--help' found` (exit 2) and a subcommand's
    /// options could not be discovered from the CLI at all. `global = true` on the long-only help arg
    /// is the fix: it reaches every subcommand, including the payload-less ones.
    #[test]
    fn every_command_accepts_help_for_its_own_options() {
        for argv in [
            vec!["--help"],
            vec!["run", "--help"],
            vec!["deploy", "--help"],
            vec!["status", "--help"],
        ] {
            let err = parse(&argv).expect_err("help is a DisplayHelp error, which is what exits 0");
            assert_eq!(
                err.kind(),
                clap::error::ErrorKind::DisplayHelp,
                "{argv:?} must be help, not a usage error"
            );
        }
        // The help shown is the *command's own*: `run`'s options are in `run --help`, not the parent's.
        let run_help = parse(&["run", "--help"])
            .expect_err("DisplayHelp")
            .to_string();
        assert!(run_help.contains("--api-port-http"), "{run_help}");
        assert!(run_help.contains("Usage: rnode run"), "{run_help}");
    }

    /// `-h` is not help in this CLI: it is `--grpc-host` at the top level and `--api-port-http` under
    /// `run`. That binding is the port's convention and is kept — renaming a short flag is a CLI
    /// contract change for anyone scripting it — so the help text states the convention where an
    /// operator looks for help, and this pins that the binding still parses.
    #[test]
    fn short_h_is_a_real_flag_and_the_help_says_so() {
        let top_help = parse(&["--help"]).expect_err("DisplayHelp").to_string();
        assert!(top_help.contains("-h, --grpc-host"), "{top_help}");
        assert!(top_help.contains("`-h` is not help"), "{top_help}");

        let opts = parse(&["-h", "example.invalid", "status"]).expect("`-h` takes a value");
        assert_eq!(opts.grpc_host, "example.invalid");

        let err = parse(&["run", "-h"]).expect_err("`-h` is `--api-port-http` under `run`");
        assert!(err.to_string().contains("--api-port-http"), "{err}");
    }
}
