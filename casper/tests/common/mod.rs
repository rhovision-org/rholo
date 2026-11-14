//! Shared harness for the casper consensus-pipeline integration tests.
//!
//! Each `casper/tests/*.rs` binary compiles this module separately, so helpers a given binary does
//! not use would warn there — hence the module-wide `dead_code` allowance below.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::Arc;

use rchain_casper::runtime_manager::{
    CapturedReply, MergeableStore, NativeChangesStore, RuntimeManager,
};
use rchain_models::runtime::{BindPattern, ListParWithRandom, TaggedContinuation};
use rchain_models::sorted::SortedProc;
use rchain_rholang::merging::{DeployMergeableDataCodec, NativeSidecarCodec};
use rchain_rholang::runtime::{ReplayRhoRuntime, RhoRuntime};
use rchain_rholang::scheduler::EffectMode;
use rchain_rholang::storage::RhoMatch;
use rchain_rspace::factory::create_history_repository;
use rchain_rspace::hot_store::InMemHotStore;
use rchain_rspace::rspace::RSpace;
use rchain_shared::store_manager::{database, InMemoryStoreManager};
use rchain_shared::typed_store::BytesCodec;

/// Assemble a full `RuntimeManager` (play + replay runtimes + mergeable store) over an in-memory
/// store, running the plain sequential scheduler.
pub async fn build_runtime_manager() -> RuntimeManager {
    build_runtime_manager_with_mode(EffectMode::Sequential).await
}

/// `build_runtime_manager` under the given effect-scheduler mode (Laws 20–22): the play runtime is
/// created via `create_with_effect_mode`, the replay runtime stays sequential (already enforced by
/// `ReplayRhoRuntime::create`), and the manager records the mode for its block-path hard-reject.
pub async fn build_runtime_manager_with_mode(mode: EffectMode) -> RuntimeManager {
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
    let (play, replay) = RSpace::create_with_replay(history.clone(), hot, Arc::new(RhoMatch));
    let rho = RhoRuntime::create_with_effect_mode(
        play,
        history.clone(),
        SortedProc::default(),
        true,
        mode,
    )
    .await
    .expect("rho runtime");
    let replay = ReplayRhoRuntime::create(Arc::new(replay), history.clone(), SortedProc::default())
        .await
        .expect("replay runtime");
    let mergeable: MergeableStore = Arc::new(
        database(
            &manager,
            "mergeable",
            Arc::new(BytesCodec),
            Arc::new(DeployMergeableDataCodec),
        )
        .await
        .expect("mergeable store"),
    );
    let native_changes: NativeChangesStore = Arc::new(
        database(
            &manager,
            "native-changes",
            Arc::new(BytesCodec),
            Arc::new(NativeSidecarCodec),
        )
        .await
        .expect("native changes store"),
    );
    RuntimeManager::new(rho, replay, history, mergeable, native_changes, mode)
}

// Imports for the shared gateway harness below.
use std::sync::Mutex;

use async_trait::async_trait;

use rchain_block_storage::dag::dag_storage::DeployId;
use rchain_casper::api::block_api::{
    ApiErr, BlockApi, Capabilities, FinalityHealth, ShardHealthEntry,
};
use rchain_casper::gateway::GatewayLeg;
use rchain_models::ast::Par;
use rchain_models::block_metadata::BlockMetadata;
use rchain_models::block_metadata::SlashSeverity;
use rchain_models::casper::protocol::casper_message::SignedDeployData;
use rchain_models::casper::protocol::deploy_service::{
    BlockInfo, ContinuationsWithBlockInfo, DataWithBlockInfo, DeployExecStatus, LightBlockInfo,
    Status,
};
use rchain_models::normalizer_env::NormalizerEnv;
use rchain_rholang::native_state::NativeSystemState;
use rchain_shared::refined::{NonNegI64, ShardId};

// --- Shared gateway test harness (Laws 26–29) --------------------------------
//
// The multi-shard gateway's tests live in two files — `gateway_txn.rs` (the happy paths and the
// failure paths reachable through a well-behaved shard) and `gateway_faults.rs` (the ones that need
// a shard that fails, times out, or never answers). Both need the same shard double, so it lives
// here rather than being duplicated or widened into the crate's production surface with a
// `#[cfg(test)] pub` accessor.

/// A `BlockApi` for one in-process shard: "deploying" evaluates the term on that shard's runtime,
/// and a reply is read back from the runtime's own channel — enough to exercise the coordinator
/// without standing up the whole node.
pub struct TestShardApi {
    shard_id: String,
    runtime: Arc<rchain_casper::runtime_manager::RuntimeManager>,
    /// The height this shard reports (a leg is anchored at it).
    height: Mutex<i64>,
    faults: ShardFaults,
    /// The `valid_after_block_number` of each accepted deploy, in order — how a test observes that a
    /// leg was anchored at the shard's head rather than at 0.
    deploy_anchors: Mutex<Vec<i64>>,
    /// The term of each accepted deploy, in order — a phase is named in its term, so this is how a
    /// test tells a `prepare` from a `commit`.
    deploy_terms: Mutex<Vec<String>>,
    /// The depth of each reply listen, in order — how a test observes the clamp.
    listen_depths: Mutex<Vec<i32>>,
}

/// How a test shard misbehaves. The default is a well-behaved shard; each knob isolates one failure
/// the coordinator has to handle, which is cheaper than a second full `BlockApi` double.
#[derive(Clone, Default)]
pub struct ShardFaults {
    /// Reject every deploy (a participant that cannot take the phase at all).
    pub deploy_error: Option<String>,
    /// Fail every reply listen.
    pub listen_error: Option<String>,
    /// Fail `get_latest_message`. The coordinator must refuse the phase rather than anchor it at
    /// genesis (AUDIT C64/C67).
    pub head_error: bool,
    /// Report this height as the shard's head instead of its deploy counter, so a test can tell
    /// "anchored at the head" from "anchored at 0" on a shard that has not produced blocks.
    pub reported_height: Option<i64>,
    /// Reject any deploy whose term contains this string — e.g. `"commit"`, to fail phase two while
    /// phase one succeeds.
    pub deploy_error_matching: Option<String>,
    /// Answer a reply listen successfully but with **no data** — which is what drives `await_reply`
    /// to its timeout, as distinct from an immediate transport error.
    pub empty_listen: bool,
}

impl TestShardApi {
    /// The height this shard reports — the count of deploys it accepted.
    pub fn height(&self) -> i64 {
        self.height.lock().map(|h| *h).unwrap_or(0)
    }

    /// How many deploys this shard accepted.
    pub fn deploy_count(&self) -> usize {
        self.deploy_anchors.lock().map(|d| d.len()).unwrap_or(0)
    }

    /// The anchor of the most recent deploy, if any.
    pub fn last_anchor(&self) -> Option<i64> {
        self.deploy_anchors.lock().ok()?.last().copied()
    }

    /// The term of every deploy this shard accepted, in order.
    pub fn deploy_terms(&self) -> Vec<String> {
        self.deploy_terms
            .lock()
            .map(|t| t.clone())
            .unwrap_or_default()
    }

    /// The depth of the most recent reply listen, if any.
    pub fn last_listen_depth(&self) -> Option<i32> {
        self.listen_depths.lock().ok()?.last().copied()
    }

    pub fn new(
        shard_id: &str,
        runtime: Arc<rchain_casper::runtime_manager::RuntimeManager>,
    ) -> Self {
        TestShardApi::with_faults(shard_id, runtime, ShardFaults::default())
    }

    pub fn with_faults(
        shard_id: &str,
        runtime: Arc<rchain_casper::runtime_manager::RuntimeManager>,
        faults: ShardFaults,
    ) -> Self {
        TestShardApi {
            shard_id: shard_id.to_string(),
            runtime,
            height: Mutex::new(0),
            faults,
            deploy_anchors: Mutex::new(Vec::new()),
            deploy_terms: Mutex::new(Vec::new()),
            listen_depths: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl BlockApi for TestShardApi {
    async fn status(&self) -> Status {
        unreachable!("the gateway does not call status")
    }

    async fn deploy(&self, d: &SignedDeployData) -> ApiErr<String> {
        if let Some(err) = &self.faults.deploy_error {
            return Err(err.clone());
        }
        if let Some(needle) = &self.faults.deploy_error_matching {
            if d.data.term.contains(needle) {
                return Err(format!("deploy rejected (matched {needle})"));
            }
        }
        let env = NormalizerEnv::new(d);
        let rand = rchain_crypto::hash::blake2b512_random::Blake2b512Random::from_init(&[0u8; 32]);
        let result = self
            .runtime
            .runtime()
            .evaluate_with_env(&d.data.term, env.to_env(), &rand)
            .await
            .map_err(|e| e.to_string())?;
        if !result.errors.is_empty() {
            return Err(result
                .errors
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("; "));
        }
        // A deploy that arrived means this shard produced a block for it.
        if let Ok(mut height) = self.height.lock() {
            *height += 1;
        }
        if let Ok(mut anchors) = self.deploy_anchors.lock() {
            anchors.push(d.data.valid_after_block_number);
        }
        if let Ok(mut terms) = self.deploy_terms.lock() {
            terms.push(d.data.term.clone());
        }
        Ok(rchain_shared::base16::encode(&d.sig))
    }

    /// Delegates to the floored read: this double has no unfinalised view of its own, and the method
    /// is required rather than defaulted so that is a decision rather than an oversight
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
        listening_name: &Par,
    ) -> ApiErr<(Vec<DataWithBlockInfo>, i32)> {
        self.get_listening_name_data_response(depth, listening_name)
            .await
    }

    async fn get_listening_name_data_response(
        &self,
        depth: i32,
        listening_name: &Par,
    ) -> ApiErr<(Vec<DataWithBlockInfo>, i32)> {
        if let Ok(mut depths) = self.listen_depths.lock() {
            depths.push(depth);
        }
        if let Some(err) = &self.faults.listen_error {
            return Err(err.clone());
        }
        if self.faults.empty_listen {
            // A successful listen with no data: `await_reply` polls until its timeout.
            return Ok((Vec::new(), depth));
        }
        let data = self
            .runtime
            .runtime()
            .get_data_par(&SortedProc::new(listening_name.clone()))
            .await
            .map_err(|e| e.to_string())?;
        Ok((
            vec![DataWithBlockInfo {
                post_block_data: data,
                block: light_block(&self.shard_id),
            }],
            0,
        ))
    }

    async fn get_latest_message(&self) -> ApiErr<BlockMetadata> {
        if self.faults.head_error {
            return Err("head unavailable (ShardFaults::head_error)".to_string());
        }
        let height = self
            .faults
            .reported_height
            .unwrap_or_else(|| self.height.lock().map(|h| *h).unwrap_or(0));
        Ok(BlockMetadata {
            block_hash: rchain_models::block_hash::BlockHash::new([0u8; 32]),
            block_num: rchain_shared::refined::BlockHeight::try_from(height)
                .map_err(|e| e.to_string())?,
            sender: rchain_models::validator::Validator::from_slice(&[0u8; 65]),
            seq_num: rchain_shared::refined::SeqNum::zero(),
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
            fringe_state_hash: rchain_crypto::hash::blake2b256_hash::Blake2b256Hash::from_bytes(
                [0u8; 32],
            )
            .into(),
            pre_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
            post_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
            sig: Vec::new(),
            sig_algorithm: String::new(),
        })
    }

    async fn deploy_status(&self, _: &DeployId) -> ApiErr<DeployExecStatus> {
        Err("not used by the gateway".to_string())
    }
    async fn pooled_deploys(&self) -> ApiErr<Vec<SignedDeployData>> {
        Err("not used by the gateway".to_string())
    }
    async fn capabilities(&self) -> Capabilities {
        unreachable!("the gateway does not call capabilities")
    }
    async fn create_block(&self, _: bool) -> ApiErr<String> {
        Err("not used by the gateway".to_string())
    }
    async fn get_propose_result(&self) -> ApiErr<String> {
        Err("not used by the gateway".to_string())
    }
    async fn get_listening_name_continuation_response(
        &self,
        _: i32,
        _: &[Par],
    ) -> ApiErr<(Vec<ContinuationsWithBlockInfo>, i32)> {
        Err("not used by the gateway".to_string())
    }
    async fn get_blocks_by_heights(&self, _: i64, _: i64) -> ApiErr<Vec<LightBlockInfo>> {
        Err("not used by the gateway".to_string())
    }
    async fn visualize_dag(&self, _: i32, _: i32, _: bool) -> ApiErr<Vec<String>> {
        Err("not used by the gateway".to_string())
    }
    async fn machine_verifiable_dag(&self, _: i32) -> ApiErr<String> {
        Err("not used by the gateway".to_string())
    }
    async fn get_blocks(&self, _: i32) -> ApiErr<Vec<LightBlockInfo>> {
        Err("not used by the gateway".to_string())
    }
    async fn find_deploy(&self, _: &DeployId) -> ApiErr<LightBlockInfo> {
        Err("not used by the gateway".to_string())
    }
    async fn get_block(&self, _: &str) -> ApiErr<BlockInfo> {
        Err("not used by the gateway".to_string())
    }
    async fn bond_status(&self, _: &[u8]) -> ApiErr<bool> {
        Err("not used by the gateway".to_string())
    }
    async fn exploratory_deploy(
        &self,
        _: &str,
        _: Option<&str>,
        _: bool,
    ) -> ApiErr<(CapturedReply, LightBlockInfo)> {
        Err("not used by the gateway".to_string())
    }
    async fn get_data_at_par(
        &self,
        _: &Par,
        _: &str,
        _: bool,
    ) -> ApiErr<(Vec<Par>, LightBlockInfo)> {
        Err("not used by the gateway".to_string())
    }
    async fn last_finalized_block(&self) -> ApiErr<BlockInfo> {
        Err("not used by the gateway".to_string())
    }
    async fn is_finalized(&self, _: &str) -> ApiErr<bool> {
        Err("not used by the gateway".to_string())
    }
}

pub fn light_block(shard_id: &str) -> LightBlockInfo {
    LightBlockInfo {
        version: 1,
        shard_id: shard_id.to_string(),
        block_hash: String::new(),
        block_number: 0,
        sender: String::new(),
        seq_num: 0,
        pre_state_hash: String::new(),
        post_state_hash: String::new(),
        justifications: Vec::new(),
        bonds: Vec::new(),
        sig_algorithm: String::new(),
        sig: String::new(),
        block_size: "0".to_string(),
        deploy_count: 0,
        rejected_deploys: Vec::new(),
        timestamp: 0,
    }
}

pub fn shard(id: &str) -> ShardId {
    ShardId::try_from(id.to_string()).unwrap()
}

/// Fund an address's vault on a shard.
pub fn fund(runtime: &RhoRuntime, address: &str, amount: i64) {
    NativeSystemState::new(runtime.native_store())
        .set_vault_balance(address, NonNegI64::try_from(amount).expect("non-negative"));
}

pub fn txn_legs(destination: &str) -> Vec<GatewayLeg> {
    vec![
        GatewayLeg {
            shard_id: shard("/root"),
            amount: 30,
            to: destination.to_string(),
        },
        GatewayLeg {
            shard_id: shard("/root/child"),
            amount: 40,
            to: destination.to_string(),
        },
    ]
}

/// The state hash of the last finalised fringe as of a synthetic block — the value a block's close
/// system deploy anchors the next epoch's active-set seed to.
///
/// A test supplies this to *both* the play and the replay of a block, exactly as a real node derives
/// it once from its own DAG and passes it to both paths. It only has to be deterministic and, where a
/// test means to move it, distinct.
pub fn fringe_state(seed: u8) -> rchain_crypto::hash::blake2b256_hash::Blake2b256Hash {
    rchain_crypto::hash::blake2b256_hash::Blake2b256Hash::create(&[seed])
}
