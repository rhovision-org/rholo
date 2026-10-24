//! The runtime manager façade (port of `casper/rholang/RuntimeManager.scala`, read-only surface).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
use rchain_crypto::public_key::PublicKey;
use rchain_models::ast::{Expr, Par, Var};
use rchain_models::block::state_hash::StateHash;
use rchain_models::casper::protocol::casper_message::{
    Event, PCost, ProcessedDeploy, ProcessedSystemDeploy, SignedDeployData, SystemDeployData,
};
use rchain_models::normalizer_env::NormalizerEnv;
use rchain_models::par_ops::from_expr;
use rchain_models::rholang::RhoType::RhoName;
use rchain_models::runtime::{BindPattern, ListParWithRandom, TaggedContinuation};
use rchain_models::sorted::SortedProc;
use rchain_models::types::count_free_vars;
use rchain_models::validator::Validator;
use rchain_rholang::accounting::Cost;
use rchain_rholang::errors::RholangError;
use rchain_rholang::evaluate_result::EvaluateResult;
use rchain_rholang::merging::SidecarRecord;
use rchain_rholang::merging::{
    calculate_num_channel_diff, encode_mergeable_key, get_number_with_rnd, DeployMergeableData,
    NumberChannel,
};
use rchain_rholang::native_state::{
    decode_bonds, decode_params, pos_active_key, pos_dev_vesting_key, pos_params_key, DevVesting,
    NativeSystemState, PosGenesis,
};
use rchain_rholang::runtime::{ReplayRhoRuntime, RhoRuntime};
use rchain_rholang::scheduler::EffectMode;
use rchain_rholang::storage::{RhoHistoryRepository, RhoMatch};
use rchain_rholang::system_processes::BlockData;
use rchain_rspace::checkpoint::SoftCheckpoint;
use rchain_rspace::hot_store::InMemHotStore;
use rchain_rspace::merger::event_log_index::NumberChannelsDiff;
use rchain_rspace::native_store::{BlockNativeEffects, InMemNativeStore, NativeWriter, PREFIX_POS};
use rchain_rspace::rspace::RSpace;
use rchain_shared::refined::{BlockHeight, NonNegI64};
use rchain_shared::typed_store::KeyValueTypedStore;

use crate::event_converter::{comm_multisets_match, to_casper_event};
use crate::genesis::contracts::Vault;
use crate::interpreter_util::is_genesis_pre_state;
use crate::rholang::{ReplayFailure, SystemDeployRuntimeResult, UserDeployRuntimeResult};
use crate::runtime_replay::RuntimeReplayOps;
use crate::system_deploy::{
    process_bool_result, EvalCollector, NativeSystemDeployOp, SystemDeploy, SystemDeployUserError,
};

/// The runtime manager (port of `RuntimeManager`). Deploy execution (user deploys + system
/// deploys), genesis/state computation, replay, and bond/validator queries are implemented.
///
/// The mergeable-channel store (port of `RuntimeManager.MergeableStore`).
pub type MergeableStore = Arc<dyn KeyValueTypedStore<Vec<u8>, Vec<DeployMergeableData>>>;

/// The per-block native-changes store: the native state mutations a block's deploys and system
/// deploys folded into its post-state, keyed exactly like [`MergeableStore`]
/// (`(post_state_hash, sender, seq_num)`).
///
/// The tuple-space merge reconstructs branch state from deploy effects; native state has no such
/// effect log, so a merge that re-applies tuple space alone silently reverts every native write in the
/// merged branches (issue #74). This sidecar is that missing effect log, written wherever a block is
/// played or replayed and carried into `BlockIndex` for `MergeScope::merge` to apply.
pub type NativeChangesStore = Arc<dyn KeyValueTypedStore<Vec<u8>, SidecarRecord>>;

/// The phlo (gas) limit for a single exploratory deploy (documented Scala deviation: Scala runs
/// exploratory deploys with no limit). Mirrors the Repl bound in
/// `node/src/api/grpc/repl_grpc_service.rs`.
const EXPLORATORY_PHLO_LIMIT: i64 = 1_000_000_000;
/// The wall-clock deadline for a single exploratory deploy (documented Scala deviation).
const EXPLORATORY_EVAL_TIMEOUT: Duration = Duration::from_secs(60);
/// The reduction-step budget for exploratory deploys. Lower than the reducer default so the legible
/// "reduction step budget exceeded" fires well inside [`EXPLORATORY_EVAL_TIMEOUT`] (measured ~460
/// steps/s at depth 16k, so 10k steps completes in ~22s worst case — issue #12). The wall clock is
/// the backstop, not the operative bound.
const EXPLORATORY_MAX_REDUCE_STEPS: i64 = 10_000;

/// Stage-1 instrument for #144: how much time the block play path spends taking rollback/log
/// snapshots, and where those snapshots come from. This deliberately changes no scheduling or
/// rollback behaviour; it only counts and times the calls that already exist.
///
/// The counters are process-wide for the same reason as the merge-search census: the async runtime
/// manager has no metrics handle. `BlockDagKeyValueStorage` publishes a snapshot of them on the DAG's
/// normal observability tick, so a devnet campaign can read the numbers from `/metrics` without
/// adding logging to the hot deploy loop.
pub mod soft_checkpoint_census {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Site {
        DeployFallback,
        DeployLog,
        CostUnitFallback,
        PreChargeLog,
        SystemDeployLog,
    }

    impl Site {
        pub const ALL: [Site; 5] = [
            Site::DeployFallback,
            Site::DeployLog,
            Site::CostUnitFallback,
            Site::PreChargeLog,
            Site::SystemDeployLog,
        ];

        pub const fn index(self) -> usize {
            match self {
                Site::DeployFallback => 0,
                Site::DeployLog => 1,
                Site::CostUnitFallback => 2,
                Site::PreChargeLog => 3,
                Site::SystemDeployLog => 4,
            }
        }

        pub const fn metric_stem(self) -> &'static str {
            match self {
                Site::DeployFallback => "deploy_fallback",
                Site::DeployLog => "deploy_log",
                Site::CostUnitFallback => "cost_unit_fallback",
                Site::PreChargeLog => "pre_charge_log",
                Site::SystemDeployLog => "system_deploy_log",
            }
        }
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct SiteSnapshot {
        pub count: u64,
        pub total_ns: u64,
        pub max_ns: u64,
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Snapshot {
        pub deploy_units: u64,
        pub deploy_unit_total_ns: u64,
        pub deploy_unit_max_ns: u64,
        pub sites: [SiteSnapshot; 5],
    }

    static COUNTS: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];
    static TOTAL_NS: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];
    static MAX_NS: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];
    static DEPLOY_UNITS: AtomicU64 = AtomicU64::new(0);
    static DEPLOY_UNIT_TOTAL_NS: AtomicU64 = AtomicU64::new(0);
    static DEPLOY_UNIT_MAX_NS: AtomicU64 = AtomicU64::new(0);

    fn ns(duration: Duration) -> u64 {
        u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
    }

    pub fn record(site: Site, duration: Duration) {
        let i = site.index();
        let n = ns(duration);
        COUNTS[i].fetch_add(1, Ordering::Relaxed);
        TOTAL_NS[i].fetch_add(n, Ordering::Relaxed);
        MAX_NS[i].fetch_max(n, Ordering::Relaxed);
    }

    pub fn record_deploy_unit(duration: Duration) {
        let n = ns(duration);
        DEPLOY_UNITS.fetch_add(1, Ordering::Relaxed);
        DEPLOY_UNIT_TOTAL_NS.fetch_add(n, Ordering::Relaxed);
        DEPLOY_UNIT_MAX_NS.fetch_max(n, Ordering::Relaxed);
    }

    pub fn snapshot() -> Snapshot {
        let mut sites = [SiteSnapshot::default(); 5];
        for site in Site::ALL {
            let i = site.index();
            sites[i] = SiteSnapshot {
                count: COUNTS[i].load(Ordering::Relaxed),
                total_ns: TOTAL_NS[i].load(Ordering::Relaxed),
                max_ns: MAX_NS[i].load(Ordering::Relaxed),
            };
        }
        Snapshot {
            deploy_units: DEPLOY_UNITS.load(Ordering::Relaxed),
            deploy_unit_total_ns: DEPLOY_UNIT_TOTAL_NS.load(Ordering::Relaxed),
            deploy_unit_max_ns: DEPLOY_UNIT_MAX_NS.load(Ordering::Relaxed),
            sites,
        }
    }

    /// Human-readable form for an evidence capture or a diagnostic line. The campaign should use
    /// the raw `/metrics` counters for arithmetic; this exists so a captured log still names the
    /// same quantities without reverse-engineering metric names.
    pub fn summary() -> String {
        let s = snapshot();
        let mut parts = Vec::new();
        for site in Site::ALL {
            let x = s.sites[site.index()];
            parts.push(format!(
                "{}={} snapshots/{} ns (max {} ns)",
                site.metric_stem(),
                x.count,
                x.total_ns,
                x.max_ns
            ));
        }
        format!(
            "soft checkpoints: {} deploy units / {} ns (max {} ns); {}",
            s.deploy_units,
            s.deploy_unit_total_ns,
            s.deploy_unit_max_ns,
            parts.join("; ")
        )
    }
}

/// Per-request attribution of the **read path** (issue #364).
///
/// A read is `POST /api/explore-deploy` with no hash: the node forks a runtime at the last finalized
/// block's state and evaluates the term. The read-latency probe's control arm shows the cost is
/// per-request *setup* rather than the term — a no-op term pays what the read term pays — so this
/// census times the setup stage by stage, to say which term dominates instead of guessing.
///
/// The two stages that matter and are easy to conflate: `Reset` is `RhoRuntime::reset` as a whole,
/// and it *contains* the system-process install leg that `rholang::read_path_census` times
/// separately — so `Reset − Install` is the space reset. Splitting them that way needs no change to
/// `reset`'s signature.
///
/// Statics rather than return values, for the same reason as [`soft_checkpoint_census`]: the async
/// runtime manager has no metrics handle, and the one that publishes them is the DAG's tick.
/// **Relaxed ordering throughout** — a progress report, not a barrier.
///
/// **The numbers are means, not per-request traces.** The counters are process-wide and cumulative,
/// so the decomposition to read is `mean_total ≈ mean_fork + mean_reset + mean_eval + mean_replies`,
/// with the fork's internals as their own means. The stage counters are recorded inside
/// `fork_play_runtime_with_mode`, which serves *every* play fork (explore-deploy, `get_data`,
/// `get_continuation`), so a stage count above `reads` is forks-per-read being visible rather than a
/// defect.
///
/// **It changes no behaviour.** Every `Instant` here wraps a call that already exists.
pub mod read_path_census {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use super::ReplySource;

    /// The stages of one read, in the order the request pays them.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Stage {
        /// The whole `fork_play_runtime_with_mode` call, beside its four internals below.
        Fork,
        /// `get_history_reader(root)` — a fresh trie plus one root-node load.
        HistoryReader,
        /// `InMemHotStore::new(reader.base())` — a fresh store with a cold read cache.
        HotStore,
        /// `RSpace::create_with_replay` — the play space and the replay half.
        SpaceAndReplay,
        /// `RhoRuntime::create_with_effect_mode` — the runtime core, including its own install pass.
        RuntimeCreate,
        /// `runtime.reset(root)` — the space reset *and* the re-install.
        Reset,
        /// `evaluate(term, rand)`.
        Evaluate,
        /// The reply read from the term's first `new`-bound name.
        DerivedReply,
        /// The reply read from `@"out"`, which is only read when the derived name was empty.
        OutReply,
    }

    impl Stage {
        pub const ALL: [Stage; 9] = [
            Stage::Fork,
            Stage::HistoryReader,
            Stage::HotStore,
            Stage::SpaceAndReplay,
            Stage::RuntimeCreate,
            Stage::Reset,
            Stage::Evaluate,
            Stage::DerivedReply,
            Stage::OutReply,
        ];

        pub const fn index(self) -> usize {
            match self {
                Stage::Fork => 0,
                Stage::HistoryReader => 1,
                Stage::HotStore => 2,
                Stage::SpaceAndReplay => 3,
                Stage::RuntimeCreate => 4,
                Stage::Reset => 5,
                Stage::Evaluate => 6,
                Stage::DerivedReply => 7,
                Stage::OutReply => 8,
            }
        }

        pub const fn metric_stem(self) -> &'static str {
            match self {
                Stage::Fork => "fork",
                Stage::HistoryReader => "history_reader",
                Stage::HotStore => "hot_store",
                Stage::SpaceAndReplay => "space_and_replay",
                Stage::RuntimeCreate => "runtime_create",
                Stage::Reset => "reset",
                Stage::Evaluate => "evaluate",
                Stage::DerivedReply => "derived_reply",
                Stage::OutReply => "out_reply",
            }
        }
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct StageSnapshot {
        pub count: u64,
        pub total_ns: u64,
        pub max_ns: u64,
    }

    impl StageSnapshot {
        /// The mean, in nanoseconds, or `None` when the stage never ran.
        pub fn mean_ns(&self) -> Option<u64> {
            (self.count > 0).then(|| self.total_ns / self.count)
        }
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Snapshot {
        /// Reads served: one per `play_exploratory_deploy`.
        pub reads: u64,
        pub total_ns: u64,
        pub max_ns: u64,
        pub stages: [StageSnapshot; 9],
        /// How many replies came from each channel — turns "this node returns `EMPTY expr`" from an
        /// observation about a build into a number measured on it.
        pub reply_first_private_name: u64,
        pub reply_out: u64,
        pub reply_none: u64,
    }

    impl Snapshot {
        /// The mean whole-request time, in nanoseconds.
        pub fn mean_total_ns(&self) -> Option<u64> {
            (self.reads > 0).then(|| self.total_ns / self.reads)
        }

        /// One line per stage, with the mean, for a log or a captured run.
        pub fn summary(&self) -> String {
            let mut parts = Vec::new();
            for stage in Stage::ALL {
                let s = self.stages[stage.index()];
                parts.push(format!(
                    "{}={} calls/{} ns mean (max {} ns)",
                    stage.metric_stem(),
                    s.count,
                    s.mean_ns().unwrap_or(0),
                    s.max_ns
                ));
            }
            format!(
                "read path: {} reads, mean {} ns (max {} ns); replies first={} out={} none={}; {}",
                self.reads,
                self.mean_total_ns().unwrap_or(0),
                self.max_ns,
                self.reply_first_private_name,
                self.reply_out,
                self.reply_none,
                parts.join("; ")
            )
        }
    }

    static STAGE_COUNTS: [AtomicU64; 9] = [const { AtomicU64::new(0) }; 9];
    static STAGE_TOTAL_NS: [AtomicU64; 9] = [const { AtomicU64::new(0) }; 9];
    static STAGE_MAX_NS: [AtomicU64; 9] = [const { AtomicU64::new(0) }; 9];
    static READS: AtomicU64 = AtomicU64::new(0);
    static REQUEST_TOTAL_NS: AtomicU64 = AtomicU64::new(0);
    static REQUEST_MAX_NS: AtomicU64 = AtomicU64::new(0);
    static REPLY_FIRST: AtomicU64 = AtomicU64::new(0);
    static REPLY_OUT: AtomicU64 = AtomicU64::new(0);
    static REPLY_NONE: AtomicU64 = AtomicU64::new(0);

    fn ns(duration: Duration) -> u64 {
        u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
    }

    /// Record one stage of one read.
    pub fn record(stage: Stage, duration: Duration) {
        let i = stage.index();
        let n = ns(duration);
        STAGE_COUNTS[i].fetch_add(1, Ordering::Relaxed);
        STAGE_TOTAL_NS[i].fetch_add(n, Ordering::Relaxed);
        STAGE_MAX_NS[i].fetch_max(n, Ordering::Relaxed);
    }

    /// Record one served read end to end.
    pub fn record_request(duration: Duration) {
        let n = ns(duration);
        READS.fetch_add(1, Ordering::Relaxed);
        REQUEST_TOTAL_NS.fetch_add(n, Ordering::Relaxed);
        REQUEST_MAX_NS.fetch_max(n, Ordering::Relaxed);
    }

    /// Record where a read's reply was found.
    pub fn record_reply(source: ReplySource) {
        match source {
            ReplySource::FirstPrivateName => &REPLY_FIRST,
            ReplySource::Out => &REPLY_OUT,
            ReplySource::None => &REPLY_NONE,
        }
        .fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot() -> Snapshot {
        let mut stages = [StageSnapshot::default(); 9];
        for stage in Stage::ALL {
            let i = stage.index();
            stages[i] = StageSnapshot {
                count: STAGE_COUNTS[i].load(Ordering::Relaxed),
                total_ns: STAGE_TOTAL_NS[i].load(Ordering::Relaxed),
                max_ns: STAGE_MAX_NS[i].load(Ordering::Relaxed),
            };
        }
        Snapshot {
            reads: READS.load(Ordering::Relaxed),
            total_ns: REQUEST_TOTAL_NS.load(Ordering::Relaxed),
            max_ns: REQUEST_MAX_NS.load(Ordering::Relaxed),
            stages,
            reply_first_private_name: REPLY_FIRST.load(Ordering::Relaxed),
            reply_out: REPLY_OUT.load(Ordering::Relaxed),
            reply_none: REPLY_NONE.load(Ordering::Relaxed),
        }
    }

    pub fn summary() -> String {
        snapshot().summary()
    }
}

async fn measured_soft_checkpoint(
    runtime: &RhoRuntime,
    site: soft_checkpoint_census::Site,
) -> SoftCheckpoint<SortedProc, BindPattern, ListParWithRandom, TaggedContinuation> {
    let started = Instant::now();
    let checkpoint = runtime.create_soft_checkpoint().await;
    soft_checkpoint_census::record(site, started.elapsed());
    checkpoint
}

pub struct RuntimeManager {
    runtime: RhoRuntime,
    replay_runtime: ReplayRhoRuntime,
    history_repo: RhoHistoryRepository,
    mergeable_store: MergeableStore,
    /// The per-block native-changes sidecar (see [`NativeChangesStore`]).
    native_changes_store: NativeChangesStore,
    /// The effect-scheduler mode the manager was created with (Laws 20–22). The block paths
    /// hard-reject `Relaxed` — a relaxed schedule never reaches consensus, so relaxed deploys run
    /// off-chain only (explore-deploy).
    effect_mode: EffectMode,
    /// The network's genesis PoS descriptors (pool, trusted stakeholder set, params). Used to
    /// re-install the genesis native state during the trusted genesis replay; the trie is
    /// authoritative for every non-genesis block.
    genesis_pos: PosGenesis,
    /// The network's genesis REV vault balances. Like the PoS descriptors these are installed as
    /// native state *outside* the genesis block's deploys (`compute_genesis`'s `set_vault_balance`
    /// loop), so a genesis replay cannot recover them from the block and has to be handed them
    /// (AUDIT C46). Empty on a node with no genesis config, which is the only case in which the
    /// genesis is not replayable at all.
    genesis_vaults: Vec<Vault>,
}

/// Errors private to the runtime-manager play helpers. Distinguishes the Law 24 certificate
/// failure — which triggers the sequential fallback (Law 25, whole-run form) — from ordinary
/// failures that must surface to the caller.
enum RuntimeRunError {
    SpeculationInvalidated,
    Other(String),
}

impl From<String> for RuntimeRunError {
    fn from(e: String) -> Self {
        RuntimeRunError::Other(e)
    }
}

impl From<RuntimeRunError> for String {
    fn from(e: RuntimeRunError) -> Self {
        match e {
            RuntimeRunError::SpeculationInvalidated => {
                "speculation invalidated (Law 24 certificate)".to_string()
            }
            RuntimeRunError::Other(e) => e,
        }
    }
}

/// Where an exploratory deploy's reply was found, named by the rule rather than by the unforgeable's
/// bytes: a client can act on "the first private name answered" or "`@"out"` answered", and cannot do
/// anything useful with a hex string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplySource {
    /// The term's first `new`-bound name — the derived channel, and the only one read before this
    /// was made explicit.
    FirstPrivateName,
    /// `@"out"` — the channel the corpus, the examples and the system-process conformance tests use.
    Out,
    /// Neither channel held a datum: the term really did produce nothing *there*.
    None,
}

impl ReplySource {
    /// The name a client sees, stable and part of the scheme (`spec/API-SCHEMA.md`).
    pub fn as_str(self) -> &'static str {
        match self {
            ReplySource::FirstPrivateName => "firstPrivateName",
            ReplySource::Out => "out",
            ReplySource::None => "none",
        }
    }
}

/// An exploratory deploy's reply: the data, and where it was read from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapturedReply {
    pub source: ReplySource,
    pub data: Vec<Par>,
}

impl RuntimeManager {
    pub fn new(
        runtime: RhoRuntime,
        replay_runtime: ReplayRhoRuntime,
        history_repo: RhoHistoryRepository,
        mergeable_store: MergeableStore,
        native_changes_store: NativeChangesStore,
        effect_mode: EffectMode,
    ) -> Self {
        RuntimeManager {
            runtime,
            replay_runtime,
            history_repo,
            mergeable_store,
            native_changes_store,
            effect_mode,
            genesis_pos: PosGenesis::default(),
            genesis_vaults: Vec::new(),
        }
    }

    /// Set the network's genesis PoS descriptors (builder form).
    pub fn with_genesis_pos(mut self, genesis_pos: PosGenesis) -> Self {
        self.genesis_pos = genesis_pos;
        self
    }

    /// Set the network's genesis REV vault balances (builder form).
    pub fn with_genesis_vaults(mut self, genesis_vaults: Vec<Vault>) -> Self {
        self.genesis_vaults = genesis_vaults;
        self
    }

    /// The network's genesis PoS descriptors.
    pub fn genesis_pos(&self) -> &PosGenesis {
        &self.genesis_pos
    }

    /// The network's genesis REV vault balances (empty when the node has no genesis config).
    pub fn genesis_vaults(&self) -> &[Vault] {
        &self.genesis_vaults
    }

    pub fn get_history_repo(&self) -> &RhoHistoryRepository {
        &self.history_repo
    }

    /// Move `CURRENT_ROOT` to `post_state` if a checkpoint recorded it (law 80, B40).
    ///
    /// This is the **acceptance** half of the split: `RSpace::create_checkpoint` records a candidate
    /// root as known, and this publishes it once the block carrying it is in the DAG. `Ok(false)`
    /// means the store does not know the root, so it is not publishable — never write a pointer to a
    /// state no checkpoint produced.
    pub async fn publish_accepted_root(&self, post_state: Blake2b256Hash) -> Result<bool, String> {
        self.history_repo.publish_if_known(post_state).await
    }

    pub fn get_mergeable_store(&self) -> &MergeableStore {
        &self.mergeable_store
    }

    pub fn get_native_changes_store(&self) -> &NativeChangesStore {
        &self.native_changes_store
    }

    pub fn runtime(&self) -> &RhoRuntime {
        &self.runtime
    }

    pub fn replay_runtime(&self) -> &ReplayRhoRuntime {
        &self.replay_runtime
    }

    /// Fork a fresh replay runtime seeded at `root` via the read-only history fork, for parallel
    /// block validation (replay is verify-only; each block replays from its own pre-state root).
    pub async fn fork_replay_runtime(
        &self,
        root: Blake2b256Hash,
    ) -> Result<ReplayRhoRuntime, String> {
        let reader = self.history_repo.get_history_reader(root).await;
        let hot = Arc::new(InMemHotStore::new(reader.base()));
        let (_play, replay) =
            RSpace::create_with_replay(self.history_repo.clone(), hot, Arc::new(RhoMatch));
        ReplayRhoRuntime::create(
            Arc::new(replay),
            self.history_repo.clone(),
            SortedProc::default(),
        )
        .await
        .map_err(|e| e.to_string())
    }

    /// Fork a fresh, isolated play runtime seeded at `root` (the play counterpart of
    /// `fork_replay_runtime`), running the plain sequential scheduler. Read paths (data-at-name,
    /// data-at-continuation) run on this so they never mutate the shared play runtime the proposer
    /// is concurrently using to create blocks.
    pub async fn fork_play_runtime(&self, root: Blake2b256Hash) -> Result<RhoRuntime, String> {
        self.fork_play_runtime_with_mode(root, EffectMode::Sequential)
            .await
    }

    /// `fork_play_runtime` under the given effect-scheduler mode (the explore-deploy path passes
    /// the manager's own mode, so a relaxed node explores off-chain under the relaxed scheduler).
    pub async fn fork_play_runtime_with_mode(
        &self,
        root: Blake2b256Hash,
        mode: EffectMode,
    ) -> Result<RhoRuntime, String> {
        let reader_started = Instant::now();
        let reader = self.history_repo.get_history_reader(root).await;
        let native_reader = self.history_repo.get_native_reader(root).await;
        read_path_census::record(
            read_path_census::Stage::HistoryReader,
            reader_started.elapsed(),
        );

        let store_started = Instant::now();
        let hot = Arc::new(InMemHotStore::new(reader.base()));
        read_path_census::record(read_path_census::Stage::HotStore, store_started.elapsed());

        let space_started = Instant::now();
        // **The fork carries its own native reader** (Law 77, `spec/Rchain/Reset.lean`). The system
        // contracts read bonds and balances through the native store, and `InMemNativeStore::empty()`
        // is a `NoopNativeReader` whose reads answer `None` — so a fork built empty is readable only
        // *after* a reset, which is why every read path used to reset the fork it had just made at the
        // same root, and paid a second system-process install into the store that reset replaced.
        // Built here, the reset becomes the identity and the read paths do not make one.
        let native_store = Arc::new(InMemNativeStore::new(native_reader));
        let (play, _replay) = RSpace::create_with_replay_and_native_store(
            self.history_repo.clone(),
            hot,
            Arc::new(RhoMatch),
            native_store,
        );
        read_path_census::record(
            read_path_census::Stage::SpaceAndReplay,
            space_started.elapsed(),
        );

        let core_started = Instant::now();
        let created = RhoRuntime::create_with_effect_mode(
            play,
            self.history_repo.clone(),
            SortedProc::default(),
            true,
            mode,
        )
        .await;
        read_path_census::record(
            read_path_census::Stage::RuntimeCreate,
            core_started.elapsed(),
        );
        created.map_err(|e| e.to_string())
    }

    /// Load mergeable channels from the store (port of `loadMergeableChannels`).
    pub async fn load_mergeable_channels(
        &self,
        state_hash: &[u8],
        creator: &[u8],
        seq_num: i64,
    ) -> Result<Vec<NumberChannelsDiff>, String> {
        let state_hash = Blake2b256Hash::from_byte_array(state_hash);
        let key = encode_mergeable_key(&state_hash, creator, seq_num);
        let vals = self.mergeable_store.get(&[key]).await?;
        let res = vals
            .into_iter()
            .next()
            .flatten()
            .ok_or_else(|| format!("Mergeable store invalid state hash {:?}.", state_hash))?;
        Ok(res
            .into_iter()
            .map(|d| d.channels.into_iter().map(|c| (c.hash, c.diff)).collect())
            .collect())
    }

    /// Convert final mergeable-channel values to diffs and persist them (port of
    /// `saveMergeableChannels`).
    pub async fn save_mergeable_channels(
        &self,
        post_state_hash: Blake2b256Hash,
        creator: &[u8],
        seq_num: i64,
        channels_data: &[NumberChannelsDiff],
        pre_state_hash: Blake2b256Hash,
    ) -> Result<(), String> {
        let diffs = self
            .convert_number_channels_to_diff(channels_data, pre_state_hash)
            .await?;
        let deploy_channels: Vec<DeployMergeableData> = diffs
            .into_iter()
            .map(|data| DeployMergeableData {
                channels: data
                    .into_iter()
                    .map(|(hash, diff)| NumberChannel { hash, diff })
                    .collect(),
            })
            .collect();
        let key = encode_mergeable_key(&post_state_hash, creator, seq_num);
        self.mergeable_store.put(&[(key, deploy_channels)]).await?;
        Ok(())
    }

    /// Load a block's native-changes record from the sidecar, or `None` when the block has no record
    /// (never played or replayed by this node, or written before this sidecar existed).
    ///
    /// **Three states, told apart here and not at the caller** (#280): no record, a record that
    /// attributes every effect to the deploy that made it, and one written before attribution existed
    /// (`SidecarRecord::LegacyUnattributed`). The last is *not* the same as an empty record, and the
    /// caller refuses a nonempty one rather than reading it as a block-level set — which is the
    /// representation the defect lived in.
    pub async fn load_native_changes(
        &self,
        state_hash: &[u8],
        creator: &[u8],
        seq_num: i64,
    ) -> Result<Option<SidecarRecord>, String> {
        let state_hash = Blake2b256Hash::from_byte_array(state_hash);
        let key = encode_mergeable_key(&state_hash, creator, seq_num);
        let vals = self.native_changes_store.get(&[key]).await?;
        Ok(vals.into_iter().next().flatten())
    }

    /// Persist a block's native state mutations beside its mergeable channels (same key). Called
    /// wherever the block is played or replayed, so the merge never has to reconstruct them.
    pub async fn save_native_changes(
        &self,
        post_state_hash: Blake2b256Hash,
        creator: &[u8],
        seq_num: i64,
        native_changes: &BlockNativeEffects,
    ) -> Result<(), String> {
        let key = encode_mergeable_key(&post_state_hash, creator, seq_num);
        self.native_changes_store
            .put(&[(key, SidecarRecord::Attributed(native_changes.clone()))])
            .await?;
        Ok(())
    }

    /// Convert final number-channel values to per-deploy diffs (port of
    /// `convertNumberChannelsToDiff`).
    async fn convert_number_channels_to_diff(
        &self,
        channels_data: &[NumberChannelsDiff],
        pre_state_hash: Blake2b256Hash,
    ) -> Result<Vec<NumberChannelsDiff>, String> {
        let history_reader = self.history_repo.get_history_reader(pre_state_hash).await;
        let mut keys: BTreeSet<Blake2b256Hash> = BTreeSet::new();
        for m in channels_data {
            keys.extend(m.keys().copied());
        }
        let mut init_values: BTreeMap<Blake2b256Hash, i64> = BTreeMap::new();
        for k in &keys {
            let data = history_reader
                .get_data(*k)
                .await
                .map_err(|e| e.to_string())?;
            let num = match data.first() {
                Some(d) => get_number_with_rnd(&d.a)?.0,
                None => 0,
            };
            init_values.insert(*k, num);
        }
        calculate_num_channel_diff(channels_data, &init_values)
    }

    /// Read the `Par`s at a channel in the state identified by `hash` (port of `getData`). Runs on a
    /// forked runtime so the read cannot reset the shared play runtime out from under the proposer.
    pub async fn get_data(&self, hash: &StateHash, channel: &Par) -> Result<Vec<Par>, String> {
        // **No reset.** The fork is built *at* this root and carries its own native reader
        // (`RSpace::with_native_store`), so a reset here would be the identity: it would replace a
        // store already at the root and re-install what the runtime core already installed, the second
        // pass landing in the store the replacement discarded. Law 77 — a reset is a projection onto
        // its root, so work done before it is unobservable (`spec/Rchain/Reset.lean`).
        let runtime = self.fork_play_runtime(to_blake(hash)).await?;
        runtime
            .get_data_par(&SortedProc::new(channel.clone()))
            .await
            .map_err(|e| e.to_string())
    }

    /// Read the `ParBody` continuations at `channels` in the state identified by `hash` (port of
    /// `getContinuation`). Runs on a forked runtime for the same reason as `get_data`.
    pub async fn get_continuation(
        &self,
        hash: &StateHash,
        channels: &[Par],
    ) -> Result<Vec<(Vec<BindPattern>, Par)>, String> {
        // **No reset.** The fork is built *at* this root and carries its own native reader
        // (`RSpace::with_native_store`), so a reset here would be the identity: it would replace a
        // store already at the root and re-install what the runtime core already installed, the second
        // pass landing in the store the replacement discarded. Law 77 — a reset is a projection onto
        // its root, so work done before it is unobservable (`spec/Rchain/Reset.lean`).
        let runtime = self.fork_play_runtime(to_blake(hash)).await?;
        let channels: Vec<SortedProc> = channels
            .iter()
            .map(|c| SortedProc::new(c.clone()))
            .collect();
        runtime
            .get_continuation_par(&channels)
            .await
            .map_err(|e| e.to_string())
    }

    /// Execute a single deploy, returning its `ProcessedDeploy` + evaluation result (port of
    /// `processDeploy`).
    pub async fn process_deploy(
        &self,
        deploy: &SignedDeployData,
        rand: &Blake2b512Random,
    ) -> Result<(ProcessedDeploy, EvaluateResult), String> {
        // Hard-reject the pure relaxed scheduler on the block path (Laws 20–22): a relaxed
        // schedule never reaches consensus, so a relaxed node refuses block work (off-chain
        // only). `RelaxedValidated` (Laws 23–25) passes: `compute_state` validates its run
        // against the sequential reference and falls back on divergence.
        if self.effect_mode == EffectMode::Relaxed {
            return Err(
                "relaxed effect scheduling is off-chain only: refusing block-path deploy execution"
                    .to_string(),
            );
        }
        match Self::process_deploy_with(&self.runtime, deploy, rand).await {
            Ok(v) => Ok(v),
            Err(RuntimeRunError::SpeculationInvalidated) => {
                // Law 25, deploy boundary: `process_deploy_with` already reverted to its pre-deploy
                // checkpoint; just switch to the sequential reference and re-run. Sequential cannot
                // invalidate, so the retry always returns a plain result.
                let mode = self.runtime.effect_mode();
                self.runtime.set_effect_mode(EffectMode::Sequential);
                let retried = Self::process_deploy_with(&self.runtime, deploy, rand).await;
                self.runtime.set_effect_mode(mode);
                retried.map_err(Into::into)
            }
            Err(RuntimeRunError::Other(e)) => Err(e),
        }
    }

    /// The body of `process_deploy`, parameterized on the runtime so the validation oracle can run
    /// the same deploy sequence on a forked sequential runtime.
    async fn process_deploy_with(
        runtime: &RhoRuntime,
        deploy: &SignedDeployData,
        rand: &Blake2b512Random,
    ) -> Result<(ProcessedDeploy, EvaluateResult), RuntimeRunError> {
        let fallback =
            measured_soft_checkpoint(runtime, soft_checkpoint_census::Site::DeployFallback).await;
        // Bind `rho:rchain:deployerId` (and `rho:rchain:deployId`) so the deploy's free URI names
        // resolve during normalization (port of `NormalizerEnv(deploy).toEnv`).
        let normalizer_env = NormalizerEnv::new(deploy);
        // Enforce the deploy's declared phlo budget (port of Scala `set(phloLimit)`). The cost
        // balance is otherwise a single i32::MAX pool shared across the runtime's lifetime; seeding
        // it here both caps this deploy at `phlo_limit` and resets it between deploys, so one deploy
        // can no longer drain the pool and brick every subsequent deploy until restart.
        runtime.cost().set(rchain_rholang::accounting::Cost::new(
            deploy.data.phlo_limit,
            "deploy",
        ));
        let eval_result = match runtime
            .evaluate_with_env(&deploy.data.term, normalizer_env.to_env(), rand)
            .await
        {
            Ok(v) => v,
            Err(RholangError::SpeculationInvalid { .. }) => {
                // Revert the speculative partial state before the error escapes, so the caller's
                // sequential re-run starts from the pre-deploy state (not the aborted run's commits).
                runtime.revert_to_soft_checkpoint(fallback).await;
                return Err(RuntimeRunError::SpeculationInvalidated);
            }
            Err(e) => return Err(RuntimeRunError::Other(e.to_string())),
        };
        // Laws 23–25: the write-record certificate fails fast on a DFS-later read; the skew signal
        // is the complementary heuristic for the certificate's blind spot (a DFS-earlier writer
        // landing while a DFS-later op held the channel). Either way, roll back and let the caller
        // fall back to the sequential reference.
        if runtime.observed_skew() {
            runtime.revert_to_soft_checkpoint(fallback).await;
            return Err(RuntimeRunError::SpeculationInvalidated);
        }
        let checkpoint =
            measured_soft_checkpoint(runtime, soft_checkpoint_census::Site::DeployLog).await;
        let succeeded = eval_result.errors.is_empty();
        // The deploy-level outcome, next to the pos call's own log. They can disagree: a deploy whose
        // state changes are reverted below still logs `[pos] ok` from inside the call, which is how a
        // `trust` that "reported success" left the trusted set unchanged (#74).
        eprintln!(
            "[deploy] deployer={} {} cost={} {}",
            rchain_shared::base16::encode(&deploy.deployer)
                .chars()
                .take(16)
                .collect::<String>(),
            if succeeded {
                "ok"
            } else {
                "FAILED (state changes reverted)"
            },
            eval_result.cost.value,
            eval_result
                .errors
                .first()
                .map(|e| e.to_string())
                .unwrap_or_default()
        );
        // Surface the reducer's failure reason in the processed deploy (issue #15): a failed user
        // deploy previously recorded only `errored: true` + cost, leaving the deployer no message.
        // `system_deploy_error` is the per-deploy error field the block already carries; explore-deploy
        // returns this same error text directly.
        let system_deploy_error = if succeeded {
            None
        } else {
            eval_result.errors.first().map(|e| e.to_string())
        };
        let deploy_log: Vec<Event> = checkpoint.log.iter().map(to_casper_event).collect();
        let processed = ProcessedDeploy {
            deploy: deploy.clone(),
            cost: PCost {
                // `PCost.cost` is a protobuf `uint64`; a negative (over-charged) cost is an
                // accounting anomaly. Reject it rather than silently clamping to 0.
                cost: u64::try_from(eval_result.cost.value).map_err(|_| {
                    RuntimeRunError::Other(format!(
                        "deploy cost is negative: {}",
                        eval_result.cost.value
                    ))
                })?,
            },
            deploy_log,
            is_failed: !succeeded,
            system_deploy_error,
        };
        if !succeeded {
            runtime.revert_to_soft_checkpoint(fallback).await;
        }
        Ok((processed, eval_result))
    }

    /// Run deploys from `start_hash` and return the post-state hash + processed deploys (port of
    /// `playDeploys`).
    pub async fn play_deploys(
        &self,
        start_hash: &Blake2b256Hash,
        terms: &[SignedDeployData],
        rand: &Blake2b512Random,
    ) -> Result<(Blake2b256Hash, Vec<UserDeployRuntimeResult>), String> {
        // Same hard-reject as `process_deploy` (belt and braces: the entry points of both block
        // paths refuse the pure relaxed scheduler; `RelaxedValidated` passes and is validated by
        // `compute_state`).
        if self.effect_mode == EffectMode::Relaxed {
            return Err(
                "relaxed effect scheduling is off-chain only: refusing block-path deploy execution"
                    .to_string(),
            );
        }
        self.runtime.reset(*start_hash).await.map_err(|e| e)?;
        let mut results = Vec::new();
        for (i, d) in terms.iter().enumerate() {
            // The user-deploy split index (1) matches `processDeployWithMergeableData`, so the
            // genesis play random agrees with the replay (`RuntimeReplayOps`).
            let r = rand
                .split_byte(u8::try_from(i).map_err(|e| e.to_string())?)
                .split_byte(1);
            let (processed, eval_result) = self
                .process_deploy(d, &r)
                .await
                .map_err(|e| format!("deploy #{i} failed: {e}"))?;
            results.push(UserDeployRuntimeResult {
                deploy: processed,
                mergeable: BTreeMap::new(),
                eval_result,
            });
        }
        let checkpoint = self.runtime.create_checkpoint().await.map_err(|e| e)?;
        Ok((checkpoint.root, results))
    }

    /// Execute a user deploy with the pre-charge/refund system deploys (port of
    /// `playDeployWithCostAccounting`).
    pub async fn play_deploy_with_cost_accounting(
        &self,
        deploy: &SignedDeployData,
        rand: &Blake2b512Random,
    ) -> Result<UserDeployRuntimeResult, String> {
        Self::play_deploy_with_cost_accounting_with(&self.runtime, deploy, rand)
            .await
            .map_err(Into::into)
    }

    /// Whether the cost-accounting *unit* needs a rollback snapshot before it starts.
    ///
    /// Only the relaxed schedulers can invalidate a speculative commit. Sequential is the reference
    /// execution and Gate preserves DFS commit order, so neither has a path that can return
    /// `SpeculationInvalidated` from this unit. Taking the O(hot-state) snapshot there bought no
    /// recovery path; #144 measured it anyway on every successful deploy. Keep the snapshot for both
    /// relaxed modes so Law 25's whole-unit rollback remains unchanged wherever invalidation exists.
    fn cost_unit_needs_fallback(mode: EffectMode) -> bool {
        matches!(mode, EffectMode::Relaxed | EffectMode::RelaxedValidated)
    }

    /// The body of `play_deploy_with_cost_accounting`, parameterized on the runtime (the
    /// validation oracle runs it on a forked sequential runtime), with the Law 25 per-deploy
    /// fallback: if the certificate invalidates any commit of this deploy's unit, the whole
    /// unit — pre-charge, user deploy, refund — reverts to the pre-unit soft checkpoint and
    /// re-runs sequentially, then the validated mode is restored.
    async fn play_deploy_with_cost_accounting_with(
        runtime: &RhoRuntime,
        deploy: &SignedDeployData,
        rand: &Blake2b512Random,
    ) -> Result<UserDeployRuntimeResult, RuntimeRunError> {
        let mode = runtime.effect_mode();
        if !Self::cost_unit_needs_fallback(mode) {
            // Sequential/Gate cannot invalidate this unit. Running it directly is behaviorally the
            // same success/error path without cloning the full hot store only to drop the clone.
            return Self::play_deploy_with_cost_accounting_once(runtime, deploy, rand).await;
        }

        // Capture the pre-unit state BEFORE the speculative run, so an invalidation can roll back
        // the whole unit (pre-charge → deploy → refund). The event log is empty at each unit start
        // (`play_deploys_with_cost_accounting_with` resets to `start_hash` first), so this drain is
        // a no-op on the success path.
        let pre =
            measured_soft_checkpoint(runtime, soft_checkpoint_census::Site::CostUnitFallback).await;
        match Self::play_deploy_with_cost_accounting_once(runtime, deploy, rand).await {
            Err(RuntimeRunError::SpeculationInvalidated) => {
                runtime.revert_to_soft_checkpoint(pre).await;
                runtime.set_effect_mode(EffectMode::Sequential);
                let retried =
                    Self::play_deploy_with_cost_accounting_once(runtime, deploy, rand).await;
                runtime.set_effect_mode(mode);
                retried
            }
            other => other,
        }
    }

    /// One speculative attempt at the cost-accounting unit (pre-charge → user deploy → refund).
    async fn play_deploy_with_cost_accounting_once(
        runtime: &RhoRuntime,
        deploy: &SignedDeployData,
        rand: &Blake2b512Random,
    ) -> Result<UserDeployRuntimeResult, RuntimeRunError> {
        let mut collector = EvalCollector::default();

        let pre_charge = SystemDeploy::pre_charge(
            deploy
                .data
                .total_phlo_charge()
                .ok_or_else(|| "phlo charge overflow".to_string())?,
            &PublicKey::new(deploy.deployer.clone()),
            rand.split_byte(0),
        );
        // **Everything inside this window is cost accounting's** (AUDIT C207): its writes are the
        // ones the merge re-derives from the accepted deploys rather than carrying in the block's
        // sidecar. Nothing else can run here — these are system deploys evaluated on this call's own
        // stack — so the attribution is exact rather than heuristic.
        runtime
            .native_store()
            .begin_writer(NativeWriter::CostAccounting);
        let (pre_result, pre_eval) = Self::eval_system_deploy_with(runtime, &pre_charge).await?;
        runtime.native_store().end_writer();
        let pre_checkpoint =
            measured_soft_checkpoint(runtime, soft_checkpoint_census::Site::PreChargeLog).await;
        collector = collector.add(
            &pre_checkpoint
                .log
                .iter()
                .map(to_casper_event)
                .collect::<Vec<_>>(),
            &pre_eval.mergeable,
        );

        if let Err(e) = pre_result {
            let failed = ProcessedDeploy {
                deploy: deploy.clone(),
                cost: PCost { cost: 0 },
                deploy_log: collector.event_log.clone(),
                is_failed: true,
                system_deploy_error: Some(e.0),
            };
            return Ok(UserDeployRuntimeResult {
                deploy: failed,
                mergeable: BTreeMap::new(),
                eval_result: EvaluateResult {
                    cost: rchain_rholang::accounting::Cost::new(0, "pre-charge"),
                    errors: Vec::new(),
                    mergeable: BTreeSet::new(),
                },
            });
        }

        let (mut processed, eval_result) =
            Self::process_deploy_with(runtime, deploy, &rand.split_byte(1)).await?;
        collector = collector.add(&processed.deploy_log, &eval_result.mergeable);

        let refund = SystemDeploy::refund(
            &PublicKey::new(deploy.deployer.clone()),
            processed.refund_amount(),
            rand.split_byte(2),
        );
        runtime
            .native_store()
            .begin_writer(NativeWriter::CostAccounting);
        let _ = Self::eval_system_deploy_with(runtime, &refund).await?;

        // **And the block's producer is paid for the work** (B2, #150). Last, because the amount is a
        // share of what the deploy *burned* — which is only known once the refund has returned the
        // unconsumed phlo — and because the vault has to hold the burned amount before it can pay out
        // of it. The executor is the block's own signed `sender`, read from the runtime the block
        // data was set on, so the play and replay paths pay the same address without either of them
        // carrying it any further than this.
        let pay_executor = SystemDeploy::pay_executor(
            &runtime.block_data().sender,
            processed.burned_amount(),
            rand.split_byte(3),
        );
        let _ = Self::eval_system_deploy_with(runtime, &pay_executor).await?;
        // The cost-accounting window closes with the last of the three, before the block's own
        // bookkeeping resumes.
        runtime.native_store().end_writer();

        processed.deploy_log = collector.event_log.clone();
        Ok(UserDeployRuntimeResult {
            deploy: processed,
            mergeable: BTreeMap::new(),
            eval_result,
        })
    }

    /// Run deploys with cost accounting from `start_hash` (port of `playDeploys` with
    /// `playDeployWithCostAccounting`).
    pub async fn play_deploys_with_cost_accounting(
        &self,
        start_hash: &Blake2b256Hash,
        terms: &[SignedDeployData],
        rand: &Blake2b512Random,
    ) -> Result<(Blake2b256Hash, Vec<UserDeployRuntimeResult>), String> {
        Self::play_deploys_with_cost_accounting_with(&self.runtime, start_hash, terms, rand)
            .await
            .map_err(Into::into)
    }

    /// The body of `play_deploys_with_cost_accounting`, parameterized on the runtime.
    async fn play_deploys_with_cost_accounting_with(
        runtime: &RhoRuntime,
        start_hash: &Blake2b256Hash,
        terms: &[SignedDeployData],
        rand: &Blake2b512Random,
    ) -> Result<(Blake2b256Hash, Vec<UserDeployRuntimeResult>), RuntimeRunError> {
        runtime
            .reset(*start_hash)
            .await
            .map_err(RuntimeRunError::Other)?;
        let mut results = Vec::new();
        for (i, d) in terms.iter().enumerate() {
            let r = rand
                .split_byte(u8::try_from(i).map_err(|e| RuntimeRunError::Other(e.to_string()))?);
            let started = Instant::now();
            // The deploy's **ordinal** in the block's list is the name its native writes travel
            // under (#280), so it is threaded down to the window rather than re-derived there.
            let ordinal = u32::try_from(i).map_err(|_| {
                RuntimeRunError::Other("more deploys than a u32 ordinal can name".to_string())
            })?;
            // The whole cost-accounting unit runs inside the deploy's own window, so the inner
            // cost windows (which the unit opens and closes itself) nest inside it rather than
            // replacing it — see `InMemNativeStore::begin_writer`'s stack.
            runtime
                .native_store()
                .begin_writer(NativeWriter::Deploy(ordinal));
            let played = Self::play_deploy_with_cost_accounting_with(runtime, d, &r).await;
            runtime.native_store().end_writer();
            soft_checkpoint_census::record_deploy_unit(started.elapsed());
            results.push(played?);
        }
        let checkpoint = runtime
            .create_checkpoint()
            .await
            .map_err(|e| RuntimeRunError::Other(e.to_string()))?;
        Ok((checkpoint.root, results))
    }

    /// Compute the genesis state from deploys (port of `computeGenesis`).
    ///
    /// Rust-first: the PoS bond pool, trusted stakeholder set, parameters and the derived active
    /// validator set are installed as native state (not derived from an interpreted `Pos.rhox`), and
    /// `terms` is the (possibly empty) list of pure-library deploys.
    pub async fn compute_genesis(
        &self,
        terms: &[SignedDeployData],
        rand: &Blake2b512Random,
        block_data: BlockData,
        pos_genesis: &PosGenesis,
        vaults: &[Vault],
    ) -> Result<(Blake2b256Hash, Blake2b256Hash, Vec<UserDeployRuntimeResult>), String> {
        let creator = block_data.sender.bytes().to_vec();
        let seq_num = i64::from(block_data.seq_num);
        self.runtime.set_block_data(block_data);
        let pre_state_hash = self.runtime.empty_state_hash().await?;
        self.runtime.reset(pre_state_hash).await.map_err(|e| e)?;
        // **The genesis installation is its own writer** (#280). Everything from here to the final
        // checkpoint is block 0's own native state — the PoS pool, the vault balances and the
        // registry aliases — written before any deploy list exists to attribute it to. It is named
        // rather than left unattributed, and it does not travel in a sidecar
        // (`NativeWriter::Genesis` says why).
        self.runtime
            .native_store()
            .begin_writer(NativeWriter::Genesis);
        let native = NativeSystemState::new(self.runtime.native_store());
        // The native-channel aliases have no deploy to wait for; seed them before the loop so a
        // blessed deploy could look them up, and re-seed after each deploy below.
        crate::genesis::seed_registry_aliases(&native).await?;
        let mut results = Vec::new();
        for (i, d) in terms.iter().enumerate() {
            let r = rand
                .split_byte(u8::try_from(i).map_err(|e| e.to_string())?)
                .split_byte(1);
            let (processed, eval_result) = self
                .process_deploy(d, &r)
                .await
                .map_err(|e| format!("deploy #{i} failed: {e}"))?;
            results.push(UserDeployRuntimeResult {
                deploy: processed,
                mergeable: BTreeMap::new(),
                eval_result,
            });
            // Publish the aliases this deploy's registration made available, before the next deploy
            // runs (MakeMint resolves `rho:lang:nonNegativeNumber` at deploy time; the governance
            // contracts resolve what an earlier governance deploy published).
            crate::genesis::seed_registry_aliases(&native).await?;
            crate::genesis::seed_rgov_aliases(self).await?;
        }
        // Install the native system-contract state before the final checkpoint so it is
        // content-addressed into the post-state hash.
        native.install_genesis(pos_genesis)?;
        // Seed the initial REV vault balances from the genesis wallets file so pre-charge can
        // deduct phlo (the native vault map is otherwise empty, and every deploy fails pre-charge).
        for vault in vaults {
            native.set_vault_balance(&vault.rev_address.to_base58(), vault.initial_balance);
        }
        // Re-seed once more so an alias whose source was the last deploy still lands before the
        // checkpoint. The *completeness* check belongs to the genesis ceremony
        // (`create_genesis_block`): `compute_genesis` is also called with arbitrary term lists (in
        // tests, and for a chain that installs a different contract set), and demanding the full
        // manifest here would reject those.
        crate::genesis::seed_registry_aliases(&native).await?;
        self.runtime.native_store().end_writer();
        let checkpoint = self.runtime.create_checkpoint().await.map_err(|e| e)?;
        let mergeable_chs: Vec<NumberChannelsDiff> =
            results.iter().map(|r| r.mergeable.clone()).collect();
        self.save_mergeable_channels(
            checkpoint.root,
            &creator,
            seq_num,
            &mergeable_chs,
            pre_state_hash,
        )
        .await?;
        Ok((pre_state_hash, checkpoint.root, results))
    }

    /// Run a system deploy's source (port of `evaluateSystemSource`).
    async fn evaluate_system_source_with(
        runtime: &RhoRuntime,
        deploy: &SystemDeploy,
    ) -> Result<EvaluateResult, RuntimeRunError> {
        runtime
            .evaluate_with_env(deploy.source, &deploy.normalizer_env, &deploy.rand)
            .await
            .map_err(|e| match e {
                RholangError::SpeculationInvalid { .. } => RuntimeRunError::SpeculationInvalidated,
                e => RuntimeRunError::Other(e.to_string()),
            })
    }

    /// Consume the result produced on the system deploy's return channel (port of
    /// `consumeSystemResult`).
    async fn consume_system_result_with(
        runtime: &RhoRuntime,
        deploy: &SystemDeploy,
    ) -> Result<Option<(TaggedContinuation, Vec<ListParWithRandom>)>, String> {
        let patterns = vec![SortedProc::new(from_expr(Expr::EVar(Box::new(
            Var::FreeVar(0),
        ))))];
        let pattern = BindPattern {
            free_count: count_free_vars(patterns[0].as_par()),
            patterns,
            remainder: None,
        };
        runtime
            .consume_result(
                &[SortedProc::new(deploy.return_channel.clone())],
                &[pattern],
            )
            .await
            .map_err(|e| e.to_string())
    }

    /// Evaluate a system deploy and extract its result (port of `evalSystemDeploy`).
    pub async fn eval_system_deploy(
        &self,
        deploy: &SystemDeploy,
    ) -> Result<(Result<(), SystemDeployUserError>, EvaluateResult), String> {
        Self::eval_system_deploy_with(&self.runtime, deploy)
            .await
            .map_err(Into::into)
    }

    /// The body of `eval_system_deploy`, parameterized on the runtime.
    async fn eval_system_deploy_with(
        runtime: &RhoRuntime,
        deploy: &SystemDeploy,
    ) -> Result<(Result<(), SystemDeployUserError>, EvaluateResult), RuntimeRunError> {
        if let Some(op) = &deploy.op {
            return Self::eval_native_system_deploy_with(runtime, op)
                .await
                .map_err(RuntimeRunError::Other);
        }
        let eval_result = Self::evaluate_system_source_with(runtime, deploy).await?;
        if !eval_result.errors.is_empty() {
            return Err(RuntimeRunError::Other(format!(
                "Unexpected system errors: {:?}",
                eval_result.errors
            )));
        }
        let consumed = Self::consume_system_result_with(runtime, deploy)
            .await
            .map_err(RuntimeRunError::Other)?;
        match consumed {
            Some((_, data)) => match data.as_slice() {
                [single] if single.pars.len() == 1 => {
                    let result = process_bool_result(single.pars[0].as_par());
                    Ok((result, eval_result))
                }
                _ => Err(RuntimeRunError::Other(
                    "Unexpected system-deploy result".to_string(),
                )),
            },
            None => Err(RuntimeRunError::Other(
                "Unable to consume results of system deploy".to_string(),
            )),
        }
    }

    /// Evaluate a native system deploy (rust-first replacement for the rholang PoS/registry sources).
    async fn eval_native_system_deploy_with(
        runtime: &RhoRuntime,
        op: &NativeSystemDeployOp,
    ) -> Result<(Result<(), SystemDeployUserError>, EvaluateResult), String> {
        let native = NativeSystemState::new(runtime.native_store());
        let result = match op {
            NativeSystemDeployOp::PreCharge { deployer, amount } => {
                native.pre_charge(deployer, *amount).await?
            }
            NativeSystemDeployOp::Refund { deployer, amount } => {
                native.refund(deployer, *amount).await?
            }
            NativeSystemDeployOp::PayExecutor { executor, burned } => {
                native.pay_executor(executor, *burned).await?
            }
            NativeSystemDeployOp::CloseBlock {
                block_number,
                fringe_state_hash,
                participation,
            } => {
                native
                    .close_block(*block_number, *fringe_state_hash, participation)
                    .await?
            }
            NativeSystemDeployOp::Slash {
                validator,
                severity,
                evidence: _,
            } => native.slash(validator, *severity).await?,
        };
        let eval_result = EvaluateResult {
            cost: rchain_rholang::accounting::Cost::new(0, "native-system-deploy"),
            errors: Vec::new(),
            mergeable: BTreeSet::new(),
        };
        Ok((result.map_err(SystemDeployUserError), eval_result))
    }

    /// Play a single block-level system deploy from `state_hash` (port of `playSystemDeploy`).
    pub async fn play_system_deploy(
        &self,
        state_hash: &Blake2b256Hash,
        deploy: &SystemDeploy,
    ) -> Result<(Blake2b256Hash, SystemDeployRuntimeResult), String> {
        Self::play_system_deploy_with(&self.runtime, state_hash, deploy)
            .await
            .map_err(Into::into)
    }

    /// The body of `play_system_deploy`, parameterized on the runtime.
    async fn play_system_deploy_with(
        runtime: &RhoRuntime,
        state_hash: &Blake2b256Hash,
        deploy: &SystemDeploy,
    ) -> Result<(Blake2b256Hash, SystemDeployRuntimeResult), RuntimeRunError> {
        runtime
            .reset(*state_hash)
            .await
            .map_err(RuntimeRunError::Other)?;
        let (result, _eval_result) = Self::eval_system_deploy_with(runtime, deploy).await?;
        let checkpoint =
            measured_soft_checkpoint(runtime, soft_checkpoint_census::Site::SystemDeployLog).await;
        let event_list: Vec<Event> = checkpoint.log.iter().map(to_casper_event).collect();
        let final_hash = runtime
            .create_checkpoint()
            .await
            .map_err(|e| RuntimeRunError::Other(e.to_string()))?
            .root;
        match result {
            Ok(()) => {
                let system_deploy = match &deploy.op {
                    Some(NativeSystemDeployOp::CloseBlock { .. }) => SystemDeployData::CloseBlock,
                    Some(NativeSystemDeployOp::Slash {
                        validator,
                        severity,
                        evidence,
                    }) => SystemDeployData::Slash {
                        validator: *validator,
                        severity: *severity,
                        evidence: evidence.clone(),
                    },
                    _ => SystemDeployData::Empty,
                };
                let processed = ProcessedSystemDeploy::Succeeded {
                    event_list,
                    system_deploy,
                };
                Ok((
                    final_hash,
                    SystemDeployRuntimeResult {
                        deploy: processed,
                        mergeable: BTreeMap::new(),
                    },
                ))
            }
            Err(e) => Err(RuntimeRunError::Other(format!(
                "System deploy failed: {}",
                e.0
            ))),
        }
    }

    /// Compute the post-state from user deploys + block-level system deploys (port of
    /// `computeState`).
    pub async fn compute_state(
        &self,
        start_hash: &Blake2b256Hash,
        terms: &[SignedDeployData],
        system_deploys: &[SystemDeploy],
        rand: &Blake2b512Random,
        block_data: BlockData,
        fringe_state_hash: &Blake2b256Hash,
    ) -> Result<
        (
            Blake2b256Hash,
            Vec<UserDeployRuntimeResult>,
            Vec<SystemDeployRuntimeResult>,
        ),
        String,
    > {
        let creator = block_data.sender.bytes().to_vec();
        let seq_num = i64::from(block_data.seq_num);
        // **The close deploy's participation is in the deploy itself on this path** (B4, #150): the play
        // op carries the map a validator derived from its fringe, so the sequential backstop and the
        // law-11 rig-replay below must use *that* map rather than an empty one — otherwise the two
        // sides of the comparison would disagree about a boundary's rewards, which is the divergence
        // this whole net exists to catch.
        let participation: BTreeMap<Validator, BlockHeight> = system_deploys
            .iter()
            .find_map(|sd| match &sd.op {
                Some(NativeSystemDeployOp::CloseBlock { participation, .. }) => {
                    Some(participation.clone())
                }
                _ => None,
            })
            .unwrap_or_default();
        // Laws 23–25: the speculative run, with the whole-set safety net. A certificate
        // invalidation escaping the per-deploy fallback (a block-level system deploy's
        // speculative commit) re-runs the whole deploy set sequentially on a forked runtime
        // from start_hash — the fork reset is the inverse-replay, the sequential fold the gate
        // re-run — and the shipped result is the sequential reference's by construction.
        let mut fell_back = false;
        let (
            mut state_hash,
            mut processed_deploys,
            mut processed_system_deploys,
            mut native_changes,
        ) = match Self::block_on(
            &self.runtime,
            start_hash,
            terms,
            system_deploys,
            rand,
            &block_data,
        )
        .await
        {
            Ok(v) => v,
            Err(RuntimeRunError::SpeculationInvalidated) => {
                let oracle = self.fork_play_runtime(*start_hash).await?;
                let v = Self::block_on(
                    &oracle,
                    start_hash,
                    terms,
                    system_deploys,
                    rand,
                    &block_data,
                )
                .await
                .map_err(String::from)?;
                fell_back = true;
                v
            }
            Err(RuntimeRunError::Other(e)) => return Err(e),
        };
        if self.effect_mode == EffectMode::RelaxedValidated && !fell_back {
            // Laws 23–25 (on-chain validated speculation): validate the speculative run against
            // the sequential DFS reference — post-state hash, per-channel COMM multisets, and
            // Law 11 rig-replay of the shipped trace — and fall back to the reference's results
            // on any divergence (the block then carries the sequential trace).
            let (hash, user, sys, native) = self
                .validate_relaxed_block(
                    start_hash,
                    terms,
                    system_deploys,
                    rand,
                    &block_data,
                    fringe_state_hash,
                    &participation,
                    &processed_deploys,
                    &processed_system_deploys,
                    state_hash,
                    native_changes,
                )
                .await?;
            state_hash = hash;
            processed_deploys = user;
            processed_system_deploys = sys;
            native_changes = native;
        }
        let mut mergeable_chs: Vec<NumberChannelsDiff> = Vec::new();
        mergeable_chs.extend(processed_deploys.iter().map(|r| r.mergeable.clone()));
        mergeable_chs.extend(processed_system_deploys.iter().map(|r| r.mergeable.clone()));
        self.save_mergeable_channels(state_hash, &creator, seq_num, &mergeable_chs, *start_hash)
            .await?;
        // The block's native mutations, beside its mergeable channels, so a merge can re-apply them
        // instead of silently reverting every native write in the merged branches (issue #74).
        self.save_native_changes(state_hash, &creator, seq_num, &native_changes)
            .await?;
        Ok((state_hash, processed_deploys, processed_system_deploys))
    }

    /// Run user deploys + block-level system deploys on the given runtime, returning the
    /// post-state hash and the processed results — the shared play-and-checkpoint fold of the
    /// speculative run and the validation oracle.
    async fn block_on(
        runtime: &RhoRuntime,
        start_hash: &Blake2b256Hash,
        terms: &[SignedDeployData],
        system_deploys: &[SystemDeploy],
        rand: &Blake2b512Random,
        block_data: &BlockData,
    ) -> Result<
        (
            Blake2b256Hash,
            Vec<UserDeployRuntimeResult>,
            Vec<SystemDeployRuntimeResult>,
            BlockNativeEffects,
        ),
        RuntimeRunError,
    > {
        runtime.set_block_data(block_data.clone());
        let (mut state_hash, processed_deploys) =
            Self::play_deploys_with_cost_accounting_with(runtime, start_hash, terms, rand).await?;
        // The user deploys' native mutations, drained into the checkpoint that call just made — and
        // **the sidecar's half only**: cost accounting's writes are re-derived by the merge from the
        // accepted deploys (AUDIT C207), so carrying them here would make every user-deploy block
        // overlap every concurrent sibling on `pos:vault` and lose its own writes to a whole-block
        // rejection. The checkpoint has already folded both halves.
        let mut native_effects = BlockNativeEffects::from_drain(&runtime.last_native_drain())
            .map_err(RuntimeRunError::Other)?;
        let mut processed_system_deploys = Vec::new();
        for (i, sd) in system_deploys.iter().enumerate() {
            let ordinal = u32::try_from(terms.len() + i).map_err(|_| {
                RuntimeRunError::Other("more deploys than a u32 ordinal can name".to_string())
            })?;
            runtime
                .native_store()
                .begin_writer(NativeWriter::Deploy(ordinal));
            let played = Self::play_system_deploy_with(runtime, &state_hash, sd).await;
            runtime.native_store().end_writer();
            let (new_hash, processed) = played?;
            state_hash = new_hash;
            // Each block-level system deploy resets and checkpoints its own state, so its native
            // mutations (activation, rewards, withdrawal processing, slashing) are read here and
            // accumulated — one `last_native_changes` per checkpoint, and reset does not clear it.
            // A block-level system deploy is not cost accounting, so the own-set is the right read.
            let effects = BlockNativeEffects::from_drain(&runtime.last_native_drain())
                .map_err(RuntimeRunError::Other)?;
            native_effects.merge(effects);
            processed_system_deploys.push(processed);
        }
        Ok((
            state_hash,
            processed_deploys,
            processed_system_deploys,
            native_effects,
        ))
    }

    /// Validate a relaxed-validated block run against the sequential reference (Laws 23–25):
    /// re-run the deploy set on a forked sequential runtime, require the post-state hash and the
    /// per-channel COMM subsequences to agree, and Law 11-rig-replay the shipped trace. On any
    /// divergence the reference's results are returned — the fallback the block carries.
    #[allow(clippy::too_many_arguments)]
    async fn validate_relaxed_block(
        &self,
        start_hash: &Blake2b256Hash,
        terms: &[SignedDeployData],
        system_deploys: &[SystemDeploy],
        rand: &Blake2b512Random,
        block_data: &BlockData,
        fringe_state_hash: &Blake2b256Hash,
        participation: &BTreeMap<Validator, BlockHeight>,
        relaxed_user: &[UserDeployRuntimeResult],
        relaxed_sys: &[SystemDeployRuntimeResult],
        relaxed_hash: Blake2b256Hash,
        relaxed_native: BlockNativeEffects,
    ) -> Result<
        (
            Blake2b256Hash,
            Vec<UserDeployRuntimeResult>,
            Vec<SystemDeployRuntimeResult>,
            BlockNativeEffects,
        ),
        String,
    > {
        let oracle = self.fork_play_runtime(*start_hash).await?;
        let (oracle_hash, oracle_user, oracle_sys, oracle_native) =
            Self::block_on(&oracle, start_hash, terms, system_deploys, rand, block_data).await?;
        let logs_match = relaxed_user
            .iter()
            .zip(&oracle_user)
            .all(|(r, o)| comm_multisets_match(&r.deploy.deploy_log, &o.deploy.deploy_log))
            && relaxed_sys
                .iter()
                .zip(&oracle_sys)
                .all(|(r, o)| match (&r.deploy, &o.deploy) {
                    (
                        ProcessedSystemDeploy::Succeeded { event_list: a, .. },
                        ProcessedSystemDeploy::Succeeded { event_list: b, .. },
                    ) => comm_multisets_match(a, b),
                    _ => false,
                });
        let mut accepted = oracle_hash == relaxed_hash && logs_match;
        if accepted {
            // Law 11 backstop: the shipped (relaxed) trace must rig-replay to the shipped hash.
            let processed: Vec<ProcessedDeploy> =
                relaxed_user.iter().map(|r| r.deploy.clone()).collect();
            let processed_sys: Vec<ProcessedSystemDeploy> =
                relaxed_sys.iter().map(|r| r.deploy.clone()).collect();
            accepted = match self.fork_replay_runtime(*start_hash).await {
                Ok(replay) => self
                    .replay_compute_state_with(
                        &replay,
                        start_hash,
                        &processed,
                        &processed_sys,
                        rand,
                        block_data.clone(),
                        fringe_state_hash,
                        participation,
                        true,
                        // No PoS override on the backstop replay; the manager's genesis
                        // descriptors stand in for the (HEAD-era) empty bonds map.
                        &self.genesis_pos,
                        // Same condition as the two validation/indexing call sites: a genesis
                        // replay needs the genesis vault balances, because `compute_genesis`
                        // installs them natively outside the block's deploys (AUDIT C46). Today
                        // the genesis does not take this path — the relaxed-validated mode is a
                        // block-path mode for proposed blocks — but the `&[]` that stood here was
                        // the same unstated "this is always non-genesis" assumption the audit
                        // falsified once, and a relaxed-validated node receiving block #0 would
                        // have reached it.
                        if is_genesis_pre_state(start_hash) {
                            self.genesis_vaults()
                        } else {
                            &[]
                        },
                    )
                    .await
                    .map(|(h, _)| h == relaxed_hash)
                    .unwrap_or(false),
                Err(_) => false,
            };
        }
        if accepted {
            Ok((
                relaxed_hash,
                relaxed_user.to_vec(),
                relaxed_sys.to_vec(),
                relaxed_native,
            ))
        } else {
            Ok((oracle_hash, oracle_user, oracle_sys, oracle_native))
        }
    }

    /// Replay processed deploys + system deploys and verify the replayed state hash + mergeable
    /// channels (port of `replayComputeState`).
    #[allow(clippy::too_many_arguments)]
    pub async fn replay_compute_state(
        &self,
        start_hash: &Blake2b256Hash,
        terms: &[ProcessedDeploy],
        system_deploys: &[ProcessedSystemDeploy],
        rand: &Blake2b512Random,
        block_data: BlockData,
        fringe_state_hash: &Blake2b256Hash,
        participation: &BTreeMap<Validator, BlockHeight>,
        with_cost_accounting: bool,
        pos_genesis: &PosGenesis,
        vaults: &[Vault],
    ) -> Result<(Blake2b256Hash, Vec<NumberChannelsDiff>), ReplayFailure> {
        self.replay_compute_state_with(
            &self.replay_runtime,
            start_hash,
            terms,
            system_deploys,
            rand,
            block_data,
            fringe_state_hash,
            participation,
            with_cost_accounting,
            pos_genesis,
            vaults,
        )
        .await
    }

    /// Replay using an explicit replay runtime (the per-block fork for parallel validation). The
    /// replay mutates only `replay_runtime`; the mergeable-channel save uses the shared store.
    #[allow(clippy::too_many_arguments)]
    pub async fn replay_compute_state_with(
        &self,
        replay_runtime: &ReplayRhoRuntime,
        start_hash: &Blake2b256Hash,
        terms: &[ProcessedDeploy],
        system_deploys: &[ProcessedSystemDeploy],
        rand: &Blake2b512Random,
        block_data: BlockData,
        fringe_state_hash: &Blake2b256Hash,
        participation: &BTreeMap<Validator, BlockHeight>,
        with_cost_accounting: bool,
        pos_genesis: &PosGenesis,
        vaults: &[Vault],
    ) -> Result<(Blake2b256Hash, Vec<NumberChannelsDiff>), ReplayFailure> {
        let creator = block_data.sender.bytes().to_vec();
        let seq_num = i64::from(block_data.seq_num);
        let (state_hash, mergeable_chs) = RuntimeReplayOps::new(replay_runtime)
            .replay_compute_state(
                start_hash,
                rand,
                terms,
                system_deploys,
                block_data,
                fringe_state_hash,
                participation,
                with_cost_accounting,
                pos_genesis,
                vaults,
            )
            .await?;
        self.save_mergeable_channels(state_hash, &creator, seq_num, &mergeable_chs, *start_hash)
            .await
            .map_err(ReplayFailure::internal_error)?;
        // The replayed block's native mutations, from the single checkpoint `replay_deploys` made at
        // the end of the run. Recorded here so the validator that replayed the block does not have to
        // replay it again to index it, and so the merge can re-apply them (issue #74).
        // **The sidecar's half**, as on the play path: cost accounting is re-derived by the merge
        // (AUDIT C207), so only the block's own writes travel. The checkpoint has folded both.
        let native_effects = BlockNativeEffects::from_drain(&replay_runtime.last_native_drain())
            .map_err(ReplayFailure::internal_error)?;
        self.save_native_changes(state_hash, &creator, seq_num, &native_effects)
            .await
            .map_err(ReplayFailure::internal_error)?;
        Ok((state_hash, mergeable_chs))
    }

    /// Run a read-only exploratory deploy and capture its result (port of `playExploratoryDeploy`).
    pub async fn play_exploratory_deploy(
        &self,
        term: &str,
        hash: &StateHash,
    ) -> Result<CapturedReply, String> {
        let rand = Blake2b512Random::default_random();
        let mut return_rand = rand.copy();
        let return_channel = RhoName::apply_bytes(return_rand.next());
        self.capture_results(hash, term, &rand, &return_channel)
            .await
    }

    async fn capture_results(
        &self,
        start: &StateHash,
        term: &str,
        rand: &Blake2b512Random,
        return_channel: &Par,
    ) -> Result<CapturedReply, String> {
        let request_started = Instant::now();
        // Fork a fresh, isolated play runtime at `start`: exploration must never mutate the shared
        // runtime the proposer uses to create blocks (a concurrent explore-deploy would otherwise
        // reset/re-evaluate the shared space mid-block and corrupt the block's post-state hash).
        let fork_started = Instant::now();
        let runtime = self
            .fork_play_runtime_with_mode(to_blake(start), self.effect_mode)
            .await?;
        read_path_census::record(read_path_census::Stage::Fork, fork_started.elapsed());

        // **No reset, and that is the fix Law 77 licenses** (`spec/Rchain/Reset.lean`). This used to
        // fork at `start` and then reset to `start` — a reset to the root the fork already named,
        // which replaced a store already at that root and re-installed the system processes the
        // runtime core had already installed, the second pass landing in the discarded store. The fork
        // now carries its own native reader, so that reset would be the identity, and the stage is no
        // longer recorded: its count is now "resets this read performed", which is zero (#364).
        // Bound the exploratory evaluation (documented Scala deviation): a phlo cap + a
        // reduction-step budget (the operative bound; the legible "step budget exceeded" error must
        // fire well inside the wall-clock deadline) + a wall-clock backstop.
        runtime
            .cost()
            .set(Cost::new(EXPLORATORY_PHLO_LIMIT, "exploratory"));
        runtime.set_max_reduce_steps(EXPLORATORY_MAX_REDUCE_STEPS);
        let eval_started = Instant::now();
        let eval_outcome =
            tokio::time::timeout(EXPLORATORY_EVAL_TIMEOUT, runtime.evaluate(term, rand)).await;
        read_path_census::record(read_path_census::Stage::Evaluate, eval_started.elapsed());
        let eval = match eval_outcome {
            Ok(eval) => eval.map_err(|e| e.to_string())?,
            Err(_elapsed) => {
                // Dropping the outer evaluation future does NOT stop the spawned continuation tasks
                // (dropping a `JoinHandle` never aborts its task). Signal the reducer cooperatively:
                // the next step check in the detached task tree fails and the whole tree unwinds,
                // releasing the forked runtime (issue #12).
                runtime.cancel_reduce();
                return Err(format!(
                    "exploratory deploy timed out after {EXPLORATORY_EVAL_TIMEOUT:?}"
                ));
            }
        };
        if !eval.errors.is_empty() {
            return Err(format!("{:?}", eval.errors));
        }
        // Where a term's reply is read from, in the order the scheme states (`spec/API-SCHEMA.md`,
        // law 43's catalog). The first is the derived name — the term's **first `new`-bound name**,
        // because the normalizer consumes this RNG stream for that binding — and the second is the
        // channel every example, corpus and client writes to. Reading only the first meant a term
        // that replied on `@"out"` returned an empty `expr`, which is indistinguishable from a term
        // that produced nothing: a reply dropped silently, with nothing to catch it. See AUDIT C38.
        let reply = 'reply: {
            let derived_started = Instant::now();
            let derived = runtime
                .get_data_par(&SortedProc::new(return_channel.clone()))
                .await
                .map_err(|e| e.to_string())?;
            read_path_census::record(
                read_path_census::Stage::DerivedReply,
                derived_started.elapsed(),
            );
            if !derived.is_empty() {
                break 'reply CapturedReply {
                    source: ReplySource::FirstPrivateName,
                    data: derived,
                };
            }
            let out_started = Instant::now();
            let out = runtime
                .get_data_par(&SortedProc::new(Self::out_channel()))
                .await
                .map_err(|e| e.to_string())?;
            read_path_census::record(read_path_census::Stage::OutReply, out_started.elapsed());
            if !out.is_empty() {
                break 'reply CapturedReply {
                    source: ReplySource::Out,
                    data: out,
                };
            }
            CapturedReply {
                source: ReplySource::None,
                data: Vec::new(),
            }
        };
        read_path_census::record_reply(reply.source);
        read_path_census::record_request(request_started.elapsed());
        Ok(reply)
    }

    /// The `@"out"` channel, as the runtime keys it: the name `@"out"` is the quoted string, which is
    /// what every `@"out"!(…)` in the corpus, the examples and the system-process conformance tests
    /// writes to and what `node/tests/lean_*_corpus.rs` reads back with `chan("out")`.
    fn out_channel() -> Par {
        rchain_models::par_ops::from_expr(rchain_models::ast::Expr::GString("out".to_string()))
    }

    /// Query the current active validators at `hash` (native PoS read, port of `getActiveValidators`).
    pub async fn get_active_validators(&self, hash: &StateHash) -> Result<Vec<Validator>, String> {
        // Active validators = the consensus set (`pos:active`), which may be a strict subset of the
        // bond pool once `number_of_active_validators` caps it.
        Ok(self.compute_bonds(hash).await?.into_keys().collect())
    }

    /// Query the current *active* bonds at `hash` (native PoS read, port of `computeBonds`).
    /// Consensus (supermajority, finality fringe, block bonds) uses this active set.
    pub async fn compute_bonds(
        &self,
        hash: &StateHash,
    ) -> Result<BTreeMap<Validator, NonNegI64>, String> {
        let reader = self.history_repo.get_history_reader(to_blake(hash)).await;
        let bytes = reader
            .get_native(PREFIX_POS, pos_active_key())
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "active-bonds leaf not found in state".to_string())?;
        decode_bonds(&bytes)
    }

    /// Read the immutable validity bounds from a particular committed state. Legacy records have
    /// no bound fields; an explicitly committed zero is a value, never a legacy sentinel. Missing,
    /// malformed and unsupported records are errors rather than permission to fall back to config.
    pub async fn committed_validation_bounds(
        &self,
        hash: &StateHash,
    ) -> Result<Option<(i64, i64)>, String> {
        let reader = self.history_repo.get_history_reader(to_blake(hash)).await;
        let bytes = reader
            .get_native(PREFIX_POS, pos_params_key())
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "PoS params leaf not found in committed state".to_string())?;
        let params = decode_params(&bytes)?;
        // decode_params has already checked the exact supported shape. Only the unversioned
        // layouts lack the bound fields; inspecting their decoded zero values loses that fact.
        Ok((bytes.len() % 8 == 1).then_some((
            i64::from(params.min_phlo_price),
            params.max_number_of_parents,
        )))
    }

    /// Read and decode the PoS params leaf from a particular committed state.
    pub async fn committed_params(
        &self,
        hash: &StateHash,
    ) -> Result<rchain_rholang::native_state::PosParams, String> {
        let reader = self.history_repo.get_history_reader(to_blake(hash)).await;
        let bytes = reader
            .get_native(PREFIX_POS, pos_params_key())
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "PoS params leaf not found in committed state".to_string())?;
        decode_params(&bytes)
    }

    /// Read the developer allocation's vesting leaf from a particular committed state. `None` means
    /// the chain carries no allocation (a legacy genesis); a malformed leaf is an error.
    pub async fn committed_dev_vesting(
        &self,
        hash: &StateHash,
    ) -> Result<Option<DevVesting>, String> {
        let reader = self.history_repo.get_history_reader(to_blake(hash)).await;
        match reader
            .get_native(PREFIX_POS, pos_dev_vesting_key())
            .await
            .map_err(|e| e.to_string())?
        {
            Some(bytes) => DevVesting::decode(&bytes).map(Some),
            None => Ok(None),
        }
    }

    /// Guard startup/restore before the node uses its configured proposal and admission settings.
    /// Validation reads the committed leaf independently. The bounds are immutable in this release;
    /// refusing a mismatch also keeps this node from producing blocks under different local rules.
    pub async fn check_configured_validation_bounds(
        &self,
        hash: &StateHash,
        min_phlo_price: i64,
        max_number_of_parents: i32,
    ) -> Result<(), String> {
        if let Some(committed) = self.committed_validation_bounds(hash).await? {
            let configured = (min_phlo_price, i64::from(max_number_of_parents));
            if configured != committed {
                return Err(format!(
                    "configured validity bounds {configured:?} differ from committed PoS bounds \
                     {committed:?} at state {}; refusing to enter Running",
                    to_blake(hash).to_hex()
                ));
            }
        }
        Ok(())
    }
}

fn to_blake(hash: &StateHash) -> Blake2b256Hash {
    Blake2b256Hash::from_byte_array(hash.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_models::runtime::{BindPattern, ListParWithRandom, TaggedContinuation};
    use rchain_rholang::merging::{DeployMergeableDataCodec, NativeSidecarCodec};
    use rchain_rspace::factory::create_history_repository;
    use rchain_shared::store_manager::{database, InMemoryStoreManager};
    use rchain_shared::typed_store::BytesCodec;

    /// A manager over fresh in-memory stores — the fixture the tests below share.
    async fn manager() -> RuntimeManager {
        let manager = InMemoryStoreManager::default();
        let history = create_history_repository::<
            SortedProc,
            BindPattern,
            ListParWithRandom,
            TaggedContinuation,
        >(&manager, "rspace")
        .await
        .unwrap();
        let empty_root = history.root();
        let reader = history.get_history_reader(empty_root).await;
        let hot = Arc::new(InMemHotStore::new(reader.base()));
        let (play, replay) = RSpace::create_with_replay(history.clone(), hot, Arc::new(RhoMatch));
        let rho = RhoRuntime::create(play, history.clone(), SortedProc::default())
            .await
            .unwrap();
        let replay =
            ReplayRhoRuntime::create(Arc::new(replay), history.clone(), SortedProc::default())
                .await
                .unwrap();
        let mergeable_store = Arc::new(
            database(
                &manager,
                "mergeable",
                Arc::new(BytesCodec),
                Arc::new(DeployMergeableDataCodec),
            )
            .await
            .unwrap(),
        );
        let native_changes_store = Arc::new(
            database(
                &manager,
                "native-changes",
                Arc::new(BytesCodec),
                Arc::new(NativeSidecarCodec),
            )
            .await
            .unwrap(),
        );
        RuntimeManager::new(
            rho,
            replay,
            history,
            mergeable_store,
            native_changes_store,
            EffectMode::Sequential,
        )
    }

    #[test]
    fn only_speculative_modes_pay_for_the_cost_unit_fallback_snapshot() {
        assert!(!RuntimeManager::cost_unit_needs_fallback(
            EffectMode::Sequential
        ));
        assert!(!RuntimeManager::cost_unit_needs_fallback(EffectMode::Gate));
        assert!(RuntimeManager::cost_unit_needs_fallback(
            EffectMode::Relaxed
        ));
        assert!(RuntimeManager::cost_unit_needs_fallback(
            EffectMode::RelaxedValidated
        ));
    }

    /// A forked replay runtime (used for parallel block validation) must be constructible at the
    /// empty root and able to replay an empty deploy set.
    #[tokio::test]
    async fn fork_replay_runtime_replays_empty() {
        let runtime = manager().await;
        let empty_root = runtime
            .runtime
            .empty_state_hash()
            .await
            .expect("the empty state hash");

        let forked = runtime.fork_replay_runtime(empty_root).await.unwrap();
        let result = runtime
            .replay_compute_state_with(
                &forked,
                &empty_root,
                &[],
                &[],
                &Blake2b512Random::new_random(128),
                BlockData::empty(),
                &Blake2b256Hash::from_bytes([0u8; 32]),
                &BTreeMap::new(),
                false,
                &PosGenesis::default(),
                &[],
            )
            .await;
        assert!(result.is_ok());
    }

    /// **The deploy fold pays the block's own sender** (B2, #150) — the integration half of the
    /// native unit test, and the half the compiler cannot check: `pay_executor` being correct says
    /// nothing about whether anything calls it, or whether it is called with the **block's** sender
    /// rather than the deployer's.
    ///
    /// The deployer and the producer are different keys on purpose, so a payment to the wrong one is
    /// visible rather than accidentally right. The share is read back from the manager's own runtime,
    /// which is where the block's native store actually lives — a `fork_play_runtime` carries the
    /// tuple space and not the native store, so reading through one would report the empty map.
    #[tokio::test]
    async fn the_deploy_fold_pays_the_blocks_sender() {
        use rchain_models::casper::protocol::casper_message::{DeployData, SignedDeployData};
        use rchain_rholang::native_state::PosParams;
        use rchain_rholang::util::rev_address::RevAddress;
        use rchain_shared::refined::{BlockHeight, SeqNum};

        let rm = manager().await;
        let rand = Blake2b512Random::from_init(&[0u8; 32]);
        let deployer = PublicKey::new(vec![7u8; 65]);
        let producer = PublicKey::new(vec![5u8; 65]);
        let producer_address = RevAddress::from_public_key(&producer)
            .expect("producer address")
            .to_base58();

        let bonded = Validator::from_slice(&[1u8; 65]);
        let pos = PosGenesis {
            bonds: BTreeMap::from([(bonded, NonNegI64::try_from(1_000).expect("a stake"))]),
            trusted: BTreeSet::new(),
            params: PosParams {
                executor_share: NonNegI64::try_from(2_500).expect("a quarter"),
                ..PosParams::default()
            },
            dev_vesting: None,
        };
        let vaults = [Vault {
            rev_address: RevAddress::from_public_key(&deployer).expect("deployer address"),
            initial_balance: NonNegI64::try_from(1_000_000_000).expect("a funded deployer"),
        }];
        let (_pre, genesis_post, _) = rm
            .compute_genesis(&[], &rand, BlockData::empty(), &pos, &vaults)
            .await
            .expect("compute_genesis");

        let block_data = BlockData {
            block_number: BlockHeight::zero(),
            sender: producer.clone(),
            seq_num: SeqNum::zero(),
            timestamp: 0,
        };
        let deploy = SignedDeployData {
            data: DeployData {
                attachments: Vec::new(),
                term: r#"new deployerId(`rho:rchain:deployerId`) in { @"marker"!(true) }"#
                    .to_string(),
                timestamp: 0,
                phlo_price: 1,
                phlo_limit: 500_000,
                valid_after_block_number: 0,
                shard_id: "root".to_string(),
            },
            deployer: deployer.bytes().to_vec(),
            sig: Vec::new(),
            sig_algorithm: "secp256k1".to_string(),
        };
        let (_post, user_results, _) = rm
            .compute_state(
                &genesis_post,
                &[deploy],
                &[],
                &rand,
                block_data,
                &Blake2b256Hash::from_bytes([3u8; 32]),
            )
            .await
            .expect("play compute_state");
        let burned = i64::from(user_results[0].deploy.burned_amount());
        assert!(burned > 0, "the control: this deploy burns phlo");

        let native = NativeSystemState::new(rm.runtime.native_store());
        assert_eq!(
            i64::from(
                native
                    .vault_balance(&producer_address)
                    .await
                    .expect("read the producer's balance")
                    .unwrap_or(NonNegI64::zero())
            ),
            burned * 2_500 / 10_000,
            "the producer's own vault holds a quarter of what the deploy burned"
        );
        assert_eq!(
            native.bonds().await.expect("read the pool").get(&bonded),
            Some(&NonNegI64::try_from(1_000).expect("a stake")),
            "and the payment does not come out of anyone's bond"
        );
    }

    /// **A boundary's participation reaches the epoch's arithmetic, and play and replay agree about
    /// it** (B4, #150).
    ///
    /// The participation is the one input to `close_block` that is **not** carried on the wire: every
    /// node derives it from its own DAG, so what matters is that each path feeds the same value into
    /// the same arithmetic. On the play path it travels *inside* the op; on the replay path it is an
    /// argument. This test is that seam.
    ///
    /// **The replay-with-a-different-map arm is the load-bearing one**, not the agreement: without it,
    /// the first assertion is satisfied by a replay that ignores the map it was handed, which is the
    /// shape of the defect the fringe read replaced — a value that looks like it is used and is not.
    ///
    /// It replaces `a_block_records_its_own_producer_and_the_replay_agrees`, whose subject — the
    /// producer's own activity record in `pos:last_spoke` — was retired with the primitive.
    #[tokio::test]
    async fn a_boundarys_participation_reaches_play_and_replay_alike() {
        use rchain_models::casper::protocol::casper_message::{DeployData, SignedDeployData};
        use rchain_rholang::native_state::PosParams;
        use rchain_rholang::util::rev_address::RevAddress;
        use rchain_shared::refined::{BlockHeight, SeqNum};

        let rm = manager().await;
        let rand = Blake2b512Random::from_init(&[0u8; 32]);
        let deployer = PublicKey::new(vec![7u8; 65]);
        let validator = Validator::from_slice(&[1u8; 65]);
        let boundary = 20i64;
        let fringe = Blake2b256Hash::from_bytes([4u8; 32]);

        // Epoch length 1, so this block is a boundary; the weight is armed and graded, and the two
        // maps below sit either side of it — one lag inside the grace, one past the knee.
        let pos = PosGenesis {
            bonds: BTreeMap::from([(validator, NonNegI64::try_from(1_000).expect("a stake"))]),
            trusted: BTreeSet::new(),
            params: PosParams {
                epoch_length: 1,
                minimum_bond: NonNegI64::try_from(1).expect("a minimum"),
                absence_slack: NonNegI64::try_from(10).expect("a knee"),
                participation_grace: NonNegI64::try_from(2).expect("a grace"),
                ..PosParams::default()
            },
            dev_vesting: None,
        };
        let vaults = [Vault {
            rev_address: RevAddress::from_public_key(&deployer).expect("deployer address"),
            initial_balance: NonNegI64::try_from(1_000_000_000).expect("a funded deployer"),
        }];
        let (_pre, genesis_post, _) = rm
            .compute_genesis(&[], &rand, BlockData::empty(), &pos, &vaults)
            .await
            .expect("compute_genesis");

        let block_data = BlockData {
            block_number: BlockHeight::try_from(boundary).expect("a height"),
            sender: deployer.clone(),
            seq_num: SeqNum::zero(),
            timestamp: 0,
        };
        // The deploy is what makes a pot: it burns phlo through the vault on its way in.
        let deploy = SignedDeployData {
            data: DeployData {
                attachments: Vec::new(),
                term: "Nil".to_string(),
                timestamp: 0,
                phlo_price: 1,
                phlo_limit: 500_000,
                valid_after_block_number: 0,
                shard_id: "root".to_string(),
            },
            deployer: deployer.bytes().to_vec(),
            sig: Vec::new(),
            sig_algorithm: "secp256k1".to_string(),
        };
        // Behind the boundary by one height (inside the grace of 2) and by nineteen (past the knee).
        let near = BTreeMap::from([(
            validator,
            BlockHeight::try_from(boundary - 1).expect("a height"),
        )]);
        let far = BTreeMap::from([(validator, BlockHeight::try_from(1).expect("a height"))]);

        let near_deploys = [SystemDeploy::close_block(
            boundary,
            fringe,
            near.clone(),
            rand.split_byte(0),
        )];
        let (post_state, user_results, sys_results) = rm
            .compute_state(
                &genesis_post,
                &[deploy.clone()],
                &near_deploys,
                &rand,
                block_data.clone(),
                &fringe,
            )
            .await
            .expect("play with a validator inside the grace");
        let processed: Vec<ProcessedDeploy> = user_results.into_iter().map(|r| r.deploy).collect();
        let processed_sys: Vec<ProcessedSystemDeploy> =
            sys_results.into_iter().map(|r| r.deploy).collect();

        // **The control first**: the other map really does move the post-state, so the replay arm
        // below is testing the map and not a no-op.
        let far_deploys = [SystemDeploy::close_block(
            boundary,
            fringe,
            far.clone(),
            rand.split_byte(0),
        )];
        let (other_post_state, _, _) = rm
            .compute_state(
                &genesis_post,
                &[deploy.clone()],
                &far_deploys,
                &rand,
                block_data.clone(),
                &fringe,
            )
            .await
            .expect("play with a validator past the knee");
        assert_ne!(
            post_state, other_post_state,
            "the participation changes what the boundary commits, or the arm below proves nothing"
        );

        let (agreed, _) = rm
            .replay_compute_state(
                &genesis_post,
                &processed,
                &processed_sys,
                &rand,
                block_data.clone(),
                &fringe,
                &near,
                true,
                &pos,
                &vaults,
            )
            .await
            .expect("replay with the map the block was played with");
        assert_eq!(
            post_state, agreed,
            "the participation is consensus data: both paths must reach the same post-state"
        );

        let (mismatched, _) = rm
            .replay_compute_state(
                &genesis_post,
                &processed,
                &processed_sys,
                &rand,
                block_data,
                &fringe,
                &far,
                true,
                &pos,
                &vaults,
            )
            .await
            .expect("replay with a map the block was not played with");
        assert_ne!(
            post_state, mismatched,
            "a replay that ignores the participation it was handed reaches the wrong state, which is \
             what this arm exists to catch"
        );
    }

    /// **AUDIT C201: after a block slashes a validator, is it gone from the bonds the *next* proposer
    /// reads?**
    ///
    /// The live A2 arm (`spec/audit/evidence/a2-live-equivocation-run.sh`) turned this up: the proposer
    /// emitted a `Slash` for the offender on **every** block after the first, 59 of them and still
    /// counting, while every one of those blocks carried a bonds map with the offender already gone.
    /// `add_recorded_equivocations` filters on `bonded`, which is `compute_bonds(pre_state_hash)` —
    /// `pos:active` read at the pre-state — so either that read is stale, or the offender is somehow
    /// still in the active set, and the two cannot both be true.
    ///
    /// This test splits the question at the one place it can be split cheaply: it slashes a bonded
    /// validator in a block and then asks `compute_bonds` at that block's **post-state**. A pass here
    /// means the read is right and the live behaviour comes from *which hash* the proposer reads (the
    /// merged pre-state, and the native sidecar that reconstructs it — #74's territory); a failure here
    /// is the bug itself, named.
    #[tokio::test]
    async fn a_slashed_validator_is_absent_from_the_bonds_at_the_post_state() {
        use rchain_models::block_metadata::SlashSeverity;
        use rchain_rholang::native_state::PosParams;
        use rchain_shared::refined::{BlockHeight, SeqNum};

        let rm = manager().await;
        let rand = Blake2b512Random::from_init(&[0u8; 32]);
        let operator = Validator::from_slice(&[1u8; 65]);
        let victim = Validator::from_slice(&[2u8; 65]);
        let pos = PosGenesis {
            bonds: BTreeMap::from([
                (operator, NonNegI64::try_from(100).expect("a stake")),
                (victim, NonNegI64::try_from(100).expect("a stake")),
            ]),
            trusted: BTreeSet::new(),
            params: PosParams::default(),
            dev_vesting: None,
        };
        let (_pre, genesis_post, _) = rm
            .compute_genesis(&[], &rand, BlockData::empty(), &pos, &[])
            .await
            .expect("compute_genesis");

        let before = rm
            .compute_bonds(&StateHash::from_slice(genesis_post.as_bytes()))
            .await
            .expect("bonds at the genesis post-state");
        assert!(
            before.contains_key(&victim),
            "the control: the victim is bonded before anything is slashed"
        );

        let block_data = BlockData {
            block_number: BlockHeight::try_from(1).expect("a height"),
            sender: PublicKey::new(vec![1u8; 65]),
            seq_num: SeqNum::zero(),
            timestamp: 0,
        };
        let slashing = [SystemDeploy::slash(
            &victim,
            SlashSeverity::Malicious,
            None,
            rand.split_byte(0),
        )];
        let (post, _, _) = rm
            .compute_state(
                &genesis_post,
                &[],
                &slashing,
                &rand,
                block_data,
                &Blake2b256Hash::from_bytes([5u8; 32]),
            )
            .await
            .expect("a block that slashes the victim");

        let after = rm
            .compute_bonds(&StateHash::from_slice(post.as_bytes()))
            .await
            .expect("bonds at the slashing block's post-state");
        assert!(
            after.contains_key(&operator),
            "the control: the operator is untouched by the slash"
        );
        assert!(
            !after.contains_key(&victim),
            "a slashed validator must be gone from `pos:active` at the post-state — if it is still \
             here, this is C201's defect and the live run's repeated `Slash` is explained by it"
        );

        // **And the two roots this block sits between are two different denominators** (W-N9-03).
        //
        // The row: the attestation guard measures reachable stake against
        // `compute_bonds(pre_state_hash)` (`proposer.rs`'s `pre_state_bonds` — the conflict-merge
        // state the block is built on) while the finaliser measures it against
        // `compute_bonds(prev_fringe_state)` (`multi_parent_casper.rs` — the root the previous fringe
        // finalised). Those are the same map only while no bond change lands between them, and this
        // fixture is exactly that shape: the genesis post-state *is* the fringe, and the slashing block
        // sits above it and has not been finalised through it. So a node proposing on top of that
        // block reads the guard's root (the block's pre-state, both stakes) while a finaliser closing
        // the fringe reads the fringe's root (one stake left).
        //
        // Nothing here decides which denominator is the right one for the quorum — that is a
        // consensus-rule change and its own unit. What this pins is the row's premise, measured rather
        // than argued: the two numbers exist for one decision, and they move the denominator with
        // them.
        let at_the_fringe = before.clone();
        let above_it = after.clone();
        let stake =
            |m: &BTreeMap<Validator, NonNegI64>| -> i64 { m.values().map(|s| i64::from(*s)).sum() };
        assert_eq!(
            stake(&at_the_fringe),
            200,
            "the control: the genesis post-state — the fringe this block is above — carries both stakes"
        );
        assert_ne!(
            at_the_fringe, above_it,
            "**the two roots disagree once a bond change is between them** (W-N9-03): the guard's \
             `compute_bonds(pre_state_hash)` and the finaliser's `compute_bonds(prev_fringe_state)` \
             are the same map only while nothing changes the bond set, and this block does"
        );
        assert_eq!(
            stake(&above_it),
            100,
            "and the denominator a quorum is measured against halves — 200 against 100 is the same \
             quorum read as a different fraction of the same chain"
        );
    }
}
