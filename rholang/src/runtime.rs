//! The rholang runtime façade (port of `RhoRuntime.scala`, core).

use rchain_shared::lock::Unpoison;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
use rchain_models::ast::{Bundle, Expr, Par, Var};
use rchain_models::par_ops::from_expr;
use rchain_models::runtime::{BindPattern, ListParWithRandom, TaggedContinuation};
use rchain_models::sorted::SortedProc;
use rchain_models::types::Closed;
use rchain_rspace::checkpoint::{Checkpoint, SoftCheckpoint};
use rchain_rspace::errors::RSpaceError;
use rchain_rspace::i_replay_space::IReplaySpace;
use rchain_rspace::i_space::ISpace;
use rchain_rspace::internal::{Datum, Row, WaitingContinuation};
use rchain_rspace::native_store::{InMemNativeStore, NativeStoreAction};
use rchain_rspace::replay_rspace::ReplayRSpace;
use rchain_rspace::rspace::RSpace;
use rchain_rspace::trace::Log;
use rchain_rspace::tuple_space::Tuplespace as RSpaceTuplespace;
use rchain_rspace::util::ReplayException;

use crate::accounting::CostAccounting;
use crate::dispatch::RholangAndScalaDispatcher;
use crate::env::Env;
use crate::errors::RholangError;
use crate::evaluate_result::EvaluateResult;
use crate::native_state::NativeSystemState;
use crate::reduce::DebruijnInterpreter;
use crate::scheduler::EffectMode;
use crate::storage::{ChargingRSpace, RhoHistoryRepository, RhoTuplespace};
use crate::system_processes::{BlockData, SystemProcesses};

/// The concrete rspace type the runtime operates on.
pub type RhoSpace = Arc<RSpace<SortedProc, BindPattern, ListParWithRandom, TaggedContinuation>>;

/// The replay rspace type the replay runtime operates on (port of `RhoReplayISpace`).
pub type RhoReplaySpace =
    Arc<ReplayRSpace<SortedProc, BindPattern, ListParWithRandom, TaggedContinuation>>;

/// The reducer type wired to the charging space and dispatcher.
pub type RhoReducer = DebruijnInterpreter<ChargingRSpace, Arc<RholangAndScalaDispatcher>>;

/// Wire the reducer and dispatcher together (breaking their mutual recursion) and return the
/// reducer.
pub fn setup_reducer(
    charging_space: ChargingRSpace,
    cost: Arc<CostAccounting>,
    mergeable_tag_name: SortedProc,
) -> Arc<RhoReducer> {
    let dispatcher = Arc::new(RholangAndScalaDispatcher::new(BTreeMap::new()));
    let reducer = Arc::new(DebruijnInterpreter::new(
        charging_space,
        dispatcher.clone(),
        BTreeMap::new(),
        mergeable_tag_name,
    ));
    let reducer_for_eval = Arc::downgrade(&reducer);
    dispatcher.set_eval(Box::new(move |par, env, rand, path| {
        let reducer = match reducer_for_eval.upgrade() {
            Some(r) => r,
            None => {
                return Box::pin(async move {
                    Err(RholangError::BugFoundError(
                        "reducer has been dropped".to_string(),
                    ))
                })
            }
        };
        let cost = cost.clone();
        Box::pin(async move { reducer.reduce_par(par, env, rand, cost, path).await })
    }));
    reducer
}

/// The rholang runtime (port of `RhoRuntime`): `evaluate`/`inj`, checkpointing, and tuplespace reads.
pub struct RhoRuntime {
    reducer: Arc<RhoReducer>,
    space: RhoSpace,
    cost: Arc<CostAccounting>,
    block_data: Arc<Mutex<BlockData>>,
    _history: RhoHistoryRepository,
    proc_defs: Arc<Vec<(Par, i32, bool, i64)>>,
}

/// A write-only bundle over `channel` (port of `Bundle(channel, writeFlag = true)`).
pub(crate) fn write_bundle(channel: Par) -> Par {
    Par {
        bundles: vec![Bundle {
            body: Box::new(channel),
            write_flag: true,
            read_flag: false,
        }],
        ..Default::default()
    }
}

/// Install each system-contract definition as a persistent join on its fixed channel (port of
/// `RhoRuntime.introduceSystemProcesses`). The `space` is a `Tuplespace` so both the play `RSpace`
/// and the `ReplayRSpace` can be installed into.
async fn install_system_processes(
    space: &RhoTuplespace,
    proc_defs: &[(Par, i32, bool, i64)],
) -> Result<(), RholangError> {
    let started = crate::read_path_census::Clock::start();
    for (name, arity, remainder, body_ref) in proc_defs {
        let patterns = vec![BindPattern {
            patterns: (0..*arity)
                .map(|i| SortedProc::new(from_expr(Expr::EVar(Box::new(Var::FreeVar(i))))))
                .collect(),
            remainder: if *remainder {
                Some(Var::FreeVar(*arity))
            } else {
                None
            },
            free_count: if *remainder { *arity + 1 } else { *arity },
        }];
        let continuation = TaggedContinuation::ScalaBodyRef(*body_ref);
        space
            .install(&[SortedProc::new(name.clone())], &patterns, continuation)
            .await
            .map_err(|e| RholangError::ReduceError(e.to_string()))?;
    }
    crate::read_path_census::record_install(proc_defs.len(), started.elapsed());
    Ok(())
}

/// The shared reducer/system-process wiring built over a `Tuplespace` (port of `createRhoEnv` +
/// `setupReducer`). The play `RhoRuntime`, the replay `ReplayRhoRuntime`, and the reporting
/// `ReportingRuntime` reuse this core; the only difference is the concrete space each retains for
/// `ISpace`/`IReplaySpace` operations.
pub(crate) struct RuntimeCore {
    pub(crate) reducer: Arc<RhoReducer>,
    pub(crate) cost: Arc<CostAccounting>,
    pub(crate) block_data: Arc<Mutex<BlockData>>,
    pub(crate) proc_defs: Vec<(Par, i32, bool, i64)>,
}

pub(crate) async fn build_runtime_core(
    space: &RhoTuplespace,
    mergeable_tag_name: SortedProc,
    native_store: Arc<InMemNativeStore>,
    concurrent: bool,
    mode: EffectMode,
) -> std::io::Result<RuntimeCore> {
    let cost = Arc::new(CostAccounting::from_initial(
        crate::accounting::Costs::unsafe_max(),
    ));
    let charging_space = ChargingRSpace::new(space.clone(), cost.clone());
    let block_data = Arc::new(Mutex::new(BlockData::empty()));
    let native_state = Arc::new(NativeSystemState::new(native_store));

    // Build the dispatcher (empty), then the system processes, then wire them together.
    let dispatcher = Arc::new(RholangAndScalaDispatcher::new(BTreeMap::new()));
    let system_processes = SystemProcesses::new(
        charging_space.clone(),
        dispatcher.clone(),
        block_data.clone(),
        native_state,
    );

    let mut dispatch_table = BTreeMap::new();
    let mut urn_map = BTreeMap::new();
    let mut proc_defs: Vec<(Par, i32, bool, i64)> = Vec::new();
    for d in system_processes.definitions() {
        dispatch_table.insert(d.body_ref, d.handler);
        urn_map.insert(d.urn, write_bundle(d.fixed_channel.clone()));
        proc_defs.push((d.fixed_channel, d.arity, d.remainder, d.body_ref));
    }

    dispatcher.set_dispatch_table(dispatch_table);
    // Minted vault handles are persisted as continuations but their handlers are not: a replay's
    // dispatcher has only the fixed table above. Attach the resolver that reads the handle's durable
    // dispatch record, so play and replay can both run a handle minted in an earlier block (#669).
    dispatcher.set_native_resolver(system_processes.native_body_resolver());
    install_system_processes(space, &proc_defs)
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

    let mut reducer = DebruijnInterpreter::new(
        charging_space,
        dispatcher.clone(),
        urn_map,
        mergeable_tag_name,
    );
    reducer.set_concurrent(concurrent);
    reducer.set_effect_mode(mode);
    let reducer = Arc::new(reducer);
    // Weak, not Arc: the dispatcher is stored inside the reducer, so a strong capture here would
    // form a reducer→dispatcher→reducer cycle and leak the whole runtime (issues #18/#23).
    let reducer_for_eval = Arc::downgrade(&reducer);
    let cost_for_eval = cost.clone();
    dispatcher.set_eval(Box::new(move |par, env, rand, path| {
        let reducer = match reducer_for_eval.upgrade() {
            Some(r) => r,
            None => {
                return Box::pin(async move {
                    Err(RholangError::BugFoundError(
                        "reducer has been dropped".to_string(),
                    ))
                })
            }
        };
        let cost = cost_for_eval.clone();
        Box::pin(async move { reducer.reduce_par(par, env, rand, cost, path).await })
    }));

    Ok(RuntimeCore {
        reducer,
        cost,
        block_data,
        proc_defs,
    })
}

impl RhoRuntime {
    pub async fn create(
        space: RhoSpace,
        history: RhoHistoryRepository,
        mergeable_tag_name: SortedProc,
    ) -> std::io::Result<RhoRuntime> {
        Self::create_with_concurrency(space, history, mergeable_tag_name, true).await
    }

    pub async fn create_with_concurrency(
        space: RhoSpace,
        history: RhoHistoryRepository,
        mergeable_tag_name: SortedProc,
        concurrent: bool,
    ) -> std::io::Result<RhoRuntime> {
        Self::create_with_effect_mode(
            space,
            history,
            mergeable_tag_name,
            concurrent,
            EffectMode::Sequential,
        )
        .await
    }

    /// Like [`RhoRuntime::create_with_concurrency`], with an explicit effect-scheduler mode
    /// (Laws 20–22): `Sequential` is the sound reference; `Gate` (Law 21) and `Relaxed` (Law 20,
    /// off-chain only) replace its effect loop while preserving its per-channel DFS order.
    pub async fn create_with_effect_mode(
        space: RhoSpace,
        history: RhoHistoryRepository,
        mergeable_tag_name: SortedProc,
        concurrent: bool,
        mode: EffectMode,
    ) -> std::io::Result<RhoRuntime> {
        let tuplespace: RhoTuplespace = space.clone();
        let native_store = space.native_store();
        let core = build_runtime_core(
            &tuplespace,
            mergeable_tag_name,
            native_store,
            concurrent,
            mode,
        )
        .await?;
        Ok(RhoRuntime {
            reducer: core.reducer,
            space,
            cost: core.cost,
            block_data: core.block_data,
            _history: history,
            proc_defs: Arc::new(core.proc_defs),
        })
    }

    /// Set the per-block data exposed to the `rho:block:data` contract (port of `setBlockData`).
    pub fn set_block_data(&self, block_data: BlockData) {
        *self.block_data.lock().unpoison() = block_data;
    }

    /// The per-block data as it stands — the getter's pair, added for the producer's payment
    /// (B2, #150), which needs the block's **sender** while the deploy fold runs.
    ///
    /// A copy rather than a guard: the block data is four small fields and holding the lock across
    /// the fold would serialise anything else that wants it, for no benefit.
    pub fn block_data(&self) -> BlockData {
        self.block_data.lock().unpoison().clone()
    }

    /// Set the effect-scheduler mode (forwards to the reducer, which also arms the Law 24
    /// per-commit certificate for `RelaxedValidated`). Interior-mutable so the casper block
    /// path can switch around the per-deploy sequential fallback re-run.
    pub fn set_effect_mode(&self, mode: EffectMode) {
        self.reducer.set_effect_mode(mode);
    }

    /// The current effect-scheduler mode.
    pub fn effect_mode(&self) -> EffectMode {
        self.reducer.effect_mode()
    }

    /// Whether the current evaluation observed a scheduling skew (the S.3 enqueue window). Read by
    /// the casper block path after evaluation to trigger the sequential fallback.
    pub fn observed_skew(&self) -> bool {
        self.reducer.observed_skew()
    }

    /// Execute a `Closed` process in the given environment (port of `inj`). The `Closed` proof is
    /// discharged at this boundary; reduction then operates on the flat `Par` sub-terms.
    pub async fn inj(
        &self,
        par: &Closed,
        env: &Env<Par>,
        rand: &Blake2b512Random,
    ) -> Result<(), RholangError> {
        self.reducer
            .clone()
            .eval(&Par::from(par.clone()), env, rand, &self.cost)
            .await
    }

    /// Parse + run a rholang term (port of `evaluate`).
    pub async fn evaluate(
        &self,
        term: &str,
        rand: &Blake2b512Random,
    ) -> Result<EvaluateResult, RholangError> {
        self.evaluate_with_env(term, &BTreeMap::new(), rand).await
    }

    /// Parse + run a rholang term with an explicit normalizer environment (port of `evaluate`).
    pub async fn evaluate_with_env(
        &self,
        term: &str,
        env: &BTreeMap<String, Par>,
        rand: &Blake2b512Random,
    ) -> Result<EvaluateResult, RholangError> {
        let before = self.cost.total_charged();
        // Charge the parse *before* doing it (AUDIT F-2). It used to happen after `before` was
        // sampled, so the parse was neither charged against the deploy's budget nor counted in the
        // reported cost — which let a `phlo_limit = 0` deploy run a full parse and normalize for free.
        // `parsing_cost` was already in the cost table and called nowhere; the Scala charges
        // `parsingCost` ahead of `sourceToADT`, and this is that.
        //
        // A deploy that cannot afford its own parse is a **failed deploy, not a node fault**, so the
        // error is collected and returned as `Ok` — the same shape the reduction's own exhaustion takes
        // in the match below. Returning `Err` here would have escaped the genesis and block paths as a
        // hard fault, and `Err` from this function has to keep meaning what the `SpeculationInvalid` arm
        // says it means.
        if let Err(e) = self
            .cost
            .charge(crate::accounting::Costs::parsing_cost(term))
        {
            let cost = self.cost.total_charged() - before;
            return Ok(EvaluateResult {
                cost: crate::accounting::Cost::new(cost, "evaluate"),
                errors: vec![e],
                mergeable: BTreeSet::new(),
            });
        }
        let par = crate::normalizer::source_to_adt_with_env(term, env)?;
        let errors = match self.inj(&par, &Env::new(), rand).await {
            Ok(()) => Vec::new(),
            // Laws 23–25: a per-commit validation failure must escape as an error — it is the
            // fail-fast signal the block path reads to fall back to the sequential reference.
            // Landing it in the errors vec would mark the deploy failed instead.
            Err(e @ RholangError::SpeculationInvalid { .. }) => return Err(e),
            Err(e) => vec![e],
        };
        let cost = self.cost.total_charged() - before;
        Ok(EvaluateResult {
            cost: crate::accounting::Cost::new(cost, "evaluate"),
            errors,
            mergeable: BTreeSet::new(),
        })
    }

    /// The empty-state hash: reset to the empty root, bootstrap the registry, and checkpoint (port
    /// of `emptyStateHash`).
    pub async fn empty_state_hash(&self) -> Result<Blake2b256Hash, String> {
        self.space
            .reset(rchain_rspace::history::history::empty_root_hash_value())
            .await
            .map_err(|e| e.to_string())?;
        // Re-install the system processes (stdout/crypto/registry/…): the reset above wipes the
        // installed continuations, so without this the genesis contracts would send to fixed channels
        // with no consumer (the silent-empty-registry failure mode). The registry handlers are native
        // `Definition`s, so there is no separate bootstrap AST to install.
        let ts: RhoTuplespace = self.space.clone();
        install_system_processes(&ts, &self.proc_defs)
            .await
            .map_err(|e| e.to_string())?;
        let checkpoint = self
            .space
            .create_checkpoint()
            .await
            .map_err(|e| e.to_string())?;
        Ok(checkpoint.root)
    }

    pub fn space(&self) -> &RhoSpace {
        &self.space
    }

    /// The native system-contract store (shared with the system processes).
    pub fn native_store(&self) -> Arc<InMemNativeStore> {
        self.space.native_store()
    }

    /// The native mutations folded into the most recent checkpoint. The block path accumulates these
    /// across the user-deploy checkpoint and each block-level system-deploy checkpoint, and the
    /// block index carries the block's total so a merge can re-apply it (issue #74).
    pub fn last_native_changes(&self) -> Vec<NativeStoreAction> {
        self.space.last_native_changes()
    }

    /// The same drain, **grouped by the deploy that wrote each slot** — what the block's *sidecar* is
    /// built from, since the merge re-derives cost accounting from the accepted deploys (AUDIT C207)
    /// and a write outside any deploy's window has no deploy to travel under (#280). The checkpoint
    /// still folds the whole set; this is only what is attributable.
    pub fn last_native_drain(&self) -> rchain_rspace::native_store::NativeDrain {
        self.space.last_native_drain()
    }

    pub fn cost(&self) -> &CostAccounting {
        self.cost.as_ref()
    }

    /// Set the reducer's per-evaluation reduction-step budget (see `DEFAULT_MAX_REDUCE_STEPS`).
    /// The exploratory runtime can lower this to bound non-terminating reads more tightly than
    /// block-production deploys.
    pub fn set_max_reduce_steps(&self, steps: i64) {
        self.reducer.set_max_reduce_steps(steps);
    }

    /// Cooperatively cancel an in-flight evaluation: the reducer's next step check fails with a
    /// cancellation error, unwinding the spawned task tree (issue #12).
    pub fn cancel_reduce(&self) {
        self.reducer.cancel();
    }

    pub async fn create_checkpoint(&self) -> Result<Checkpoint, String> {
        self.space.create_checkpoint().await
    }

    pub async fn reset(&self, root: Blake2b256Hash) -> Result<(), String> {
        self.space.reset(root).await?;
        // Minted native handlers belong to the block that minted them. Drop them so play, like a
        // replay, resolves the handle from its durable record (#669). This is a **handler-parity**
        // guard, not a cost one: resolution charges nothing, and the handle block's play/replay
        // costs agree either way (`vault_handle_across_blocks` asserts both). What differs is *which*
        // `ContractCall`/native reader a handler closes over — a stale in-memory one after the store
        // is reset versus the one a fresh replay builds — so both paths are made to use the same
        // construction from durable state.
        self.reducer.clear_minted_handlers();
        // The reset replaces the hot store, dropping the in-memory system-process installs (they
        // are not checkpointed). Re-install so deploys/evaluations that run against this root still
        // reach the fixed-channel system contracts.
        let ts: RhoTuplespace = self.space.clone();
        let install_started = crate::read_path_census::Clock::start();
        let install = install_system_processes(&ts, &self.proc_defs).await;
        crate::read_path_census::record_reset_install(install_started.elapsed());
        install.map_err(|e| e.to_string())
    }

    /// Capture a soft (in-memory) checkpoint for rollback (port of `createSoftCheckpoint`).
    pub async fn create_soft_checkpoint(
        &self,
    ) -> SoftCheckpoint<SortedProc, BindPattern, ListParWithRandom, TaggedContinuation> {
        self.space.create_soft_checkpoint().await
    }

    /// Roll back to a soft checkpoint (port of `revertToSoftCheckpoint`).
    pub async fn revert_to_soft_checkpoint(
        &self,
        checkpoint: SoftCheckpoint<SortedProc, BindPattern, ListParWithRandom, TaggedContinuation>,
    ) -> std::result::Result<(), rchain_rspace::errors::RSpaceError> {
        self.space.revert_to_soft_checkpoint(checkpoint).await
    }

    pub async fn get_data(
        &self,
        channel: &SortedProc,
    ) -> Result<Vec<Datum<ListParWithRandom>>, RSpaceError> {
        self.space.get_data(channel).await
    }

    pub async fn get_joins(
        &self,
        channel: &SortedProc,
    ) -> Result<Vec<Vec<SortedProc>>, RSpaceError> {
        self.space.get_joins(channel).await
    }

    pub async fn get_continuation(
        &self,
        channels: &[SortedProc],
    ) -> Result<Vec<WaitingContinuation<BindPattern, TaggedContinuation>>, RSpaceError> {
        self.space.get_waiting_continuations(channels).await
    }

    /// Read all `Par`s at a channel (port of `getDataPar`).
    pub async fn get_data_par(&self, channel: &SortedProc) -> Result<Vec<Par>, RSpaceError> {
        let data = self.space.get_data(channel).await?;
        Ok(data
            .into_iter()
            .flat_map(|d| d.a.pars.into_iter().map(|p| p.as_par().clone()))
            .collect())
    }

    /// Read the waiting `ParBody` continuations as `(patterns, body)` (port of
    /// `getContinuationPar`).
    pub async fn get_continuation_par(
        &self,
        channels: &[SortedProc],
    ) -> Result<Vec<(Vec<BindPattern>, Par)>, RSpaceError> {
        let conts = self.space.get_waiting_continuations(channels).await?;
        Ok(conts
            .into_iter()
            .filter_map(|wc| match wc.continuation {
                TaggedContinuation::ParBody(pwr) => Some((wc.patterns, pwr.body.as_par().clone())),
                _ => None,
            })
            .collect())
    }

    /// Consume the result at a channel with a pattern (port of `consumeResult`).
    pub async fn consume_result(
        &self,
        channels: &[SortedProc],
        patterns: &[BindPattern],
    ) -> Result<Option<(TaggedContinuation, Vec<ListParWithRandom>)>, RSpaceError> {
        let result = self
            .space
            .consume(
                channels,
                patterns,
                TaggedContinuation::Empty,
                false,
                BTreeSet::new(),
            )
            .await?;
        Ok(result.map(|(cont, data)| {
            (
                cont.continuation,
                data.into_iter().map(|d| d.matched_datum).collect(),
            )
        }))
    }

    pub async fn get_hot_changes(
        &self,
    ) -> BTreeMap<Vec<SortedProc>, Row<BindPattern, ListParWithRandom, TaggedContinuation>> {
        self.space.to_map().await
    }
}

/// The replay runtime (port of `ReplayRhoRuntime`). Wraps a `ReplayRSpace` so that `inj`/`evaluate`
/// re-execute against the recorded COMM trace, and exposes `rig`/`check_replay_data` (Law 11).
/// **Whether a replay treats a recorded-cost mismatch as a failure** (#564).
///
/// Production is [`ReplayCostCheck::Enforce`]: the recorded cost is consensus data and the oracle
/// refuses a mismatch. [`ReplayCostCheck::Ignore`] is the diagnostic shadow: it lets the replay reach
/// the checkpoint so the caller can learn whether the declared post-state hash matched, which the
/// enforcing path never computes. The mismatches it saw are kept on the runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayCostCheck {
    Enforce,
    Ignore,
}

/// One cost mismatch the diagnostic shadow let through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IgnoredCostMismatch {
    pub deploy_id: Vec<u8>,
    pub recorded: i64,
    pub replayed: i64,
    /// The replay-side breakdown for this deploy, if the census was on.
    pub replay_census: Option<crate::accounting::CostCensusSnapshot>,
}

pub struct ReplayRhoRuntime {
    reducer: Arc<RhoReducer>,
    space: RhoReplaySpace,
    cost: Arc<CostAccounting>,
    block_data: Arc<Mutex<BlockData>>,
    _history: RhoHistoryRepository,
    proc_defs: Arc<Vec<(Par, i32, bool, i64)>>,
    /// `true` = `Ignore`. The enforcing path is the default and the only one a block is accepted on.
    ignore_cost_mismatch: AtomicBool,
    ignored_cost_mismatch: Mutex<Option<IgnoredCostMismatch>>,
}

impl ReplayRhoRuntime {
    pub async fn create(
        space: RhoReplaySpace,
        history: RhoHistoryRepository,
        mergeable_tag_name: SortedProc,
    ) -> std::io::Result<ReplayRhoRuntime> {
        let tuplespace: RhoTuplespace = space.clone();
        let native_store = space.native_store();
        // Replay is the Law 11 determinism carrier: it must re-derive the recorded COMM trace, so
        // only the sequential DFS effect loop may drive it (never Gate/Relaxed).
        let core = build_runtime_core(
            &tuplespace,
            mergeable_tag_name,
            native_store,
            true,
            EffectMode::Sequential,
        )
        .await?;
        Ok(ReplayRhoRuntime {
            reducer: core.reducer,
            space,
            cost: core.cost,
            block_data: core.block_data,
            _history: history,
            proc_defs: Arc::new(core.proc_defs),
            ignore_cost_mismatch: AtomicBool::new(false),
            ignored_cost_mismatch: Mutex::new(None),
        })
    }

    /// Set the diagnostic shadow mode. Production never calls this with `Ignore`; the self-built
    /// validation path uses it for one bounded replay after a cost refusal.
    pub fn set_replay_cost_check(&self, check: ReplayCostCheck) {
        self.ignore_cost_mismatch
            .store(check == ReplayCostCheck::Ignore, Ordering::Relaxed);
    }

    pub fn replay_cost_check(&self) -> ReplayCostCheck {
        if self.ignore_cost_mismatch.load(Ordering::Relaxed) {
            ReplayCostCheck::Ignore
        } else {
            ReplayCostCheck::Enforce
        }
    }

    /// Record a mismatch the shadow let through. The first one wins: a shadow replay is for one
    /// diagnosis, not a list.
    pub fn note_ignored_cost_mismatch(&self, mismatch: IgnoredCostMismatch) {
        let mut slot = self.ignored_cost_mismatch.lock().unpoison();
        if slot.is_none() {
            *slot = Some(mismatch);
        }
    }

    pub fn take_ignored_cost_mismatch(&self) -> Option<IgnoredCostMismatch> {
        self.ignored_cost_mismatch.lock().unpoison().take()
    }

    /// Set the per-block data exposed to the `rho:block:data` contract (port of `setBlockData`).
    pub fn set_block_data(&self, block_data: BlockData) {
        *self.block_data.lock().unpoison() = block_data;
    }

    /// The per-block data as it stands — the getter's pair, added for the producer's payment
    /// (B2, #150), which needs the block's **sender** while the deploy fold runs.
    ///
    /// A copy rather than a guard: the block data is four small fields and holding the lock across
    /// the fold would serialise anything else that wants it, for no benefit.
    pub fn block_data(&self) -> BlockData {
        self.block_data.lock().unpoison().clone()
    }

    /// Execute a `Closed` process in the given environment (port of `inj`). The `Closed` proof is
    /// discharged at this boundary; reduction then operates on the flat `Par` sub-terms.
    pub async fn inj(
        &self,
        par: &Closed,
        env: &Env<Par>,
        rand: &Blake2b512Random,
    ) -> Result<(), RholangError> {
        self.reducer
            .clone()
            .eval(&Par::from(par.clone()), env, rand, &self.cost)
            .await
    }

    /// Parse + run a rholang term (port of `evaluate`).
    pub async fn evaluate(
        &self,
        term: &str,
        rand: &Blake2b512Random,
    ) -> Result<EvaluateResult, RholangError> {
        self.evaluate_with_env(term, &BTreeMap::new(), rand).await
    }

    /// Parse + run a rholang term with an explicit normalizer environment (port of `evaluate`).
    pub async fn evaluate_with_env(
        &self,
        term: &str,
        env: &BTreeMap<String, Par>,
        rand: &Blake2b512Random,
    ) -> Result<EvaluateResult, RholangError> {
        let before = self.cost.total_charged();
        // Charge the parse *before* doing it (AUDIT F-2). It used to happen after `before` was
        // sampled, so the parse was neither charged against the deploy's budget nor counted in the
        // reported cost — which let a `phlo_limit = 0` deploy run a full parse and normalize for free.
        // `parsing_cost` was already in the cost table and called nowhere; the Scala charges
        // `parsingCost` ahead of `sourceToADT`, and this is that.
        //
        // A deploy that cannot afford its own parse is a **failed deploy, not a node fault**, so the
        // error is collected and returned as `Ok` — the same shape the reduction's own exhaustion takes
        // in the match below. Returning `Err` here would have escaped the genesis and block paths as a
        // hard fault, and `Err` from this function has to keep meaning what the `SpeculationInvalid` arm
        // says it means.
        if let Err(e) = self
            .cost
            .charge(crate::accounting::Costs::parsing_cost(term))
        {
            let cost = self.cost.total_charged() - before;
            return Ok(EvaluateResult {
                cost: crate::accounting::Cost::new(cost, "evaluate"),
                errors: vec![e],
                mergeable: BTreeSet::new(),
            });
        }
        let par = crate::normalizer::source_to_adt_with_env(term, env)?;
        let errors = match self.inj(&par, &Env::new(), rand).await {
            Ok(()) => Vec::new(),
            Err(e) => vec![e],
        };
        let cost = self.cost.total_charged() - before;
        Ok(EvaluateResult {
            cost: crate::accounting::Cost::new(cost, "evaluate"),
            errors,
            mergeable: BTreeSet::new(),
        })
    }

    pub fn space(&self) -> &RhoReplaySpace {
        &self.space
    }

    /// The native system-contract store (shared with the wrapped play space).
    pub fn native_store(&self) -> Arc<InMemNativeStore> {
        self.space.native_store()
    }

    /// The native mutations folded into the most recent replay checkpoint (one per replayed block).
    pub fn last_native_changes(&self) -> Vec<NativeStoreAction> {
        self.space.last_native_changes()
    }

    /// The same drain, **grouped by the deploy that wrote each slot** — what the block's *sidecar* is
    /// built from (#280).
    pub fn last_native_drain(&self) -> rchain_rspace::native_store::NativeDrain {
        self.space.last_native_drain()
    }

    pub fn cost(&self) -> &CostAccounting {
        self.cost.as_ref()
    }

    pub async fn create_checkpoint(&self) -> Result<Checkpoint, String> {
        self.space.create_checkpoint().await
    }

    pub async fn reset(&self, root: Blake2b256Hash) -> Result<(), String> {
        self.space.reset(root).await?;
        self.reducer.clear_minted_handlers();
        // Re-install the system processes after the reset (see `RhoRuntime::reset`).
        let ts: RhoTuplespace = self.space.clone();
        install_system_processes(&ts, &self.proc_defs)
            .await
            .map_err(|e| e.to_string())
    }

    /// Capture a soft (in-memory) checkpoint for rollback (port of `createSoftCheckpoint`).
    pub async fn create_soft_checkpoint(
        &self,
    ) -> SoftCheckpoint<SortedProc, BindPattern, ListParWithRandom, TaggedContinuation> {
        self.space.create_soft_checkpoint().await
    }

    /// Roll back to a soft checkpoint (port of `revertToSoftCheckpoint`).
    pub async fn revert_to_soft_checkpoint(
        &self,
        checkpoint: SoftCheckpoint<SortedProc, BindPattern, ListParWithRandom, TaggedContinuation>,
    ) -> std::result::Result<(), rchain_rspace::errors::RSpaceError> {
        self.space.revert_to_soft_checkpoint(checkpoint).await
    }

    pub async fn get_data(
        &self,
        channel: &SortedProc,
    ) -> Result<Vec<Datum<ListParWithRandom>>, RSpaceError> {
        self.space.get_data(channel).await
    }

    /// Read all `Par`s at a channel (port of `getDataPar`).
    pub async fn get_data_par(&self, channel: &SortedProc) -> Result<Vec<Par>, RSpaceError> {
        let data = self.space.get_data(channel).await?;
        Ok(data
            .into_iter()
            .flat_map(|d| d.a.pars.into_iter().map(|p| p.as_par().clone()))
            .collect())
    }

    /// Consume the result at a channel with a pattern (port of `consumeResult`).
    pub async fn consume_result(
        &self,
        channels: &[SortedProc],
        patterns: &[BindPattern],
    ) -> Result<Option<(TaggedContinuation, Vec<ListParWithRandom>)>, RSpaceError> {
        let result = self
            .space
            .consume(
                channels,
                patterns,
                TaggedContinuation::Empty,
                false,
                BTreeSet::new(),
            )
            .await?;
        Ok(result.map(|(cont, data)| {
            (
                cont.continuation,
                data.into_iter().map(|d| d.matched_datum).collect(),
            )
        }))
    }

    /// Load the replay trace (port of `rig`).
    pub async fn rig(&self, log: Log) {
        self.space.rig(log).await;
    }

    /// Verify every recorded COMM was consumed by the replay (port of `checkReplayData`).
    pub async fn check_replay_data(&self) -> Result<(), ReplayException> {
        self.space.check_replay_data().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use rchain_rspace::tuple_space::{ContResult, Result as RSpaceResult};
    use std::sync::Mutex;

    struct MockSpace {
        produced: Mutex<Vec<(SortedProc, ListParWithRandom, bool)>>,
    }

    #[async_trait]
    impl RSpaceTuplespace<SortedProc, BindPattern, ListParWithRandom, TaggedContinuation>
        for MockSpace
    {
        async fn consume(
            &self,
            _channels: &[SortedProc],
            _patterns: &[BindPattern],
            _continuation: TaggedContinuation,
            _persist: bool,
            _peeks: BTreeSet<usize>,
        ) -> Result<
            Option<(
                ContResult<SortedProc, BindPattern, TaggedContinuation>,
                Vec<RSpaceResult<SortedProc, ListParWithRandom>>,
            )>,
            RSpaceError,
        > {
            Ok(None)
        }

        async fn produce(
            &self,
            channel: SortedProc,
            data: ListParWithRandom,
            persist: bool,
        ) -> Result<
            Option<(
                ContResult<SortedProc, BindPattern, TaggedContinuation>,
                Vec<RSpaceResult<SortedProc, ListParWithRandom>>,
            )>,
            RSpaceError,
        > {
            self.produced
                .lock()
                .unpoison()
                .push((channel, data, persist));
            Ok(None)
        }

        async fn install(
            &self,
            _channels: &[SortedProc],
            _patterns: &[BindPattern],
            _continuation: TaggedContinuation,
        ) -> Result<Option<(TaggedContinuation, Vec<ListParWithRandom>)>, RSpaceError> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn inj_send_produces_through_bridge() {
        let mock = Arc::new(MockSpace {
            produced: Mutex::new(Vec::new()),
        });
        let cost = Arc::new(CostAccounting::from_initial(
            crate::accounting::Costs::unsafe_max(),
        ));
        let charging = ChargingRSpace::new(mock.clone(), cost.clone());
        let reducer = setup_reducer(charging, cost.clone(), SortedProc::default());

        let send = rchain_models::ast::Send {
            chan: Box::new(
                rchain_models::par_ops::from_expr(rchain_models::ast::Expr::GInt(1)).quote(),
            ),
            data: vec![
                rchain_models::par_ops::from_expr(rchain_models::ast::Expr::GInt(2)).quote(),
            ],
            persistent: false,
            locally_free: rchain_models::ast::AlwaysEqual(vec![]),
            connective_used: false,
        };
        let par = Par {
            sends: vec![send],
            ..Default::default()
        };
        let rand = Blake2b512Random::new_random(128);
        reducer
            .clone()
            .eval(&par, &Env::new(), &rand, &cost)
            .await
            .unwrap();

        let produced = mock.produced.lock().unpoison();
        assert_eq!(produced.len(), 1);
        assert_eq!(
            produced[0].1.pars,
            vec![SortedProc::new(rchain_models::par_ops::from_expr(
                rchain_models::ast::Expr::GInt(2)
            ))]
        );
    }
}
