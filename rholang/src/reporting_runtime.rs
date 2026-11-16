//! The reporting replay runtime (port of `ReportingRuntime` in `ReportingCasper.scala`).
//!
//! A `ReplayRhoRuntime` analogue whose space is a [`ReportingRspace`], so produce/consume/COMM
//! events are recorded during replay. Exposes `get_report` to drain the recorded report.

use rchain_shared::lock::Unpoison;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
use rchain_models::ast::Par;
use rchain_models::runtime::{BindPattern, ListParWithRandom, TaggedContinuation};
use rchain_models::sorted::SortedProc;
use rchain_models::types::Closed;
use rchain_rspace::checkpoint::{Checkpoint, SoftCheckpoint};
use rchain_rspace::errors::RSpaceError;
use rchain_rspace::i_replay_space::IReplaySpace;
use rchain_rspace::i_space::ISpace;
use rchain_rspace::internal::Datum;
use rchain_rspace::native_store::InMemNativeStore;
use rchain_rspace::reporting_rspace::{ReportingEvent, ReportingRspace};
use rchain_rspace::trace::Log;
use rchain_rspace::tuple_space::Tuplespace;
use rchain_rspace::util::ReplayException;
use rchain_shared::store_manager::KeyValueStoreManager;

use crate::accounting::CostAccounting;
use crate::env::Env;
use crate::errors::RholangError;
use crate::evaluate_result::EvaluateResult;
use crate::runtime::{build_runtime_core, RhoReducer};
use crate::scheduler::EffectMode;
use crate::storage::RhoTuplespace;
use crate::system_processes::BlockData;

/// The concrete rholang reporting space (port of `RhoReportingRspace`).
pub type RhoReportingRspace =
    ReportingRspace<SortedProc, BindPattern, ListParWithRandom, TaggedContinuation>;

/// A single recorded reporting event (port of `RhoReportingEvent`).
pub type RhoReportingEvent =
    ReportingEvent<SortedProc, BindPattern, ListParWithRandom, TaggedContinuation>;

/// Build a reporting space from the store manager + the rholang matcher (port of
/// `ReportingRuntime.createReportingRSpace`).
pub async fn create_reporting_rspace(
    manager: &dyn KeyValueStoreManager,
) -> Result<Arc<RhoReportingRspace>, String> {
    rchain_rspace::factory::create_reporting_rspace(manager, Arc::new(crate::storage::RhoMatch))
        .await
        .map(Arc::new)
}

/// The reporting runtime (port of `ReportingRuntime`).
pub struct ReportingRuntime {
    reducer: Arc<RhoReducer>,
    space: Arc<RhoReportingRspace>,
    cost: Arc<CostAccounting>,
    block_data: Arc<Mutex<BlockData>>,
}

impl ReportingRuntime {
    pub async fn create(
        space: Arc<RhoReportingRspace>,
        mergeable_tag_name: SortedProc,
    ) -> std::io::Result<ReportingRuntime> {
        let tuplespace: RhoTuplespace = space.clone();
        let native_store = space.native_store();
        // Replay-side (event-recording) runtime: like `ReplayRhoRuntime`, it must re-derive the
        // recorded trace, so only the sequential DFS effect loop may drive it.
        let core = build_runtime_core(
            &tuplespace,
            mergeable_tag_name,
            native_store,
            true,
            EffectMode::Sequential,
        )
        .await?;
        Ok(ReportingRuntime {
            reducer: core.reducer,
            space,
            cost: core.cost,
            block_data: core.block_data,
        })
    }

    /// Drain the recorded report (port of `getReport`).
    pub fn get_report(&self) -> Vec<Vec<RhoReportingEvent>> {
        self.space.get_report()
    }

    /// The cost-accounting cell (exposed so replay can seed the per-deploy phlo budget).
    pub fn cost(&self) -> &CostAccounting {
        self.cost.as_ref()
    }

    /// The native system-contract store (shared with the wrapped reporting/replay/play spaces).
    pub fn native_store(&self) -> Arc<InMemNativeStore> {
        self.space.native_store()
    }

    pub fn set_block_data(&self, block_data: BlockData) {
        *self.block_data.lock().unpoison() = block_data;
    }

    /// The per-block data as it stands (the getter's pair; see `RhoRuntime::block_data`, B2, #150).
    pub fn block_data(&self) -> BlockData {
        self.block_data.lock().unpoison().clone()
    }

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

    pub async fn evaluate(
        &self,
        term: &str,
        rand: &Blake2b512Random,
    ) -> Result<EvaluateResult, RholangError> {
        self.evaluate_with_env(term, &BTreeMap::new(), rand).await
    }

    pub async fn evaluate_with_env(
        &self,
        term: &str,
        env: &BTreeMap<String, Par>,
        rand: &Blake2b512Random,
    ) -> Result<EvaluateResult, RholangError> {
        let before = self.cost.total_charged();
        // As `RhoRuntime::evaluate_with_env`: charge the parse before doing it (AUDIT F-2). This type
        // does not delegate to that one, so the change has to be made in both — including the shape of
        // the failure, which is a collected deploy error rather than a returned fault.
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

    pub async fn create_checkpoint(&self) -> Result<Checkpoint, String> {
        self.space.create_checkpoint().await
    }

    pub async fn reset(&self, root: Blake2b256Hash) -> Result<(), String> {
        self.space.reset(root).await
    }

    pub async fn create_soft_checkpoint(
        &self,
    ) -> SoftCheckpoint<SortedProc, BindPattern, ListParWithRandom, TaggedContinuation> {
        self.space.create_soft_checkpoint().await
    }

    pub async fn revert_to_soft_checkpoint(
        &self,
        checkpoint: SoftCheckpoint<SortedProc, BindPattern, ListParWithRandom, TaggedContinuation>,
    ) -> std::result::Result<(), rchain_rspace::errors::RSpaceError> {
        self.space.revert_to_soft_checkpoint(checkpoint).await
    }

    pub async fn rig(&self, log: Log) {
        self.space.rig(log).await;
    }

    pub async fn check_replay_data(&self) -> Result<(), ReplayException> {
        self.space.check_replay_data().await
    }

    pub async fn get_data(
        &self,
        channel: &SortedProc,
    ) -> Result<Vec<Datum<ListParWithRandom>>, RSpaceError> {
        self.space.get_data(channel).await
    }

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
    use rchain_shared::store_manager::InMemoryStoreManager;

    use crate::accounting::Cost;

    fn rand() -> Blake2b512Random {
        Blake2b512Random::from_init(&[0u8; 32])
    }

    async fn runtime() -> ReportingRuntime {
        let manager = InMemoryStoreManager::default();
        let space = create_reporting_rspace(&manager)
            .await
            .expect("reporting space");
        ReportingRuntime::create(space, SortedProc::default())
            .await
            .expect("reporting runtime")
    }

    /// A term that reduces cleanly is reported with no errors and a charged cost.
    #[tokio::test]
    async fn a_clean_evaluation_reports_no_errors_and_charges_a_cost() {
        let rt = runtime().await;
        let result = rt
            .evaluate(r#"new c in { c!(1) | for (@x <- c) { Nil } }"#, &rand())
            .await
            .expect("a parsable term");
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert!(result.cost.value > 0, "the evaluation must be charged");
    }

    /// **The error-capture arm**: a term that parses but fails to *reduce* comes back as an
    /// `EvaluateResult` with the error recorded — not as an `Err`. That distinction is what a
    /// caller reports as a failed deploy rather than as a malformed request.
    #[tokio::test]
    async fn a_reduce_error_is_captured_in_the_result() {
        let rt = runtime().await;
        // The divide has to be *reached*: a top-level expression that is not a send or a receive is
        // discarded by the reducer, so `1 / 0` alone would evaluate cleanly. Sending it forces the
        // evaluation.
        let result = rt
            .evaluate(r#"@"out"!(1 / 0)"#, &rand())
            .await
            .expect("division by zero is a reduce error, not a parse error");
        assert!(
            !result.errors.is_empty(),
            "the failure must be reported in the result"
        );
        assert!(result.failed());
    }

    /// A term that does not even parse *is* an `Err` — the other half of the same distinction.
    #[tokio::test]
    async fn a_parse_error_is_returned_as_an_error() {
        let rt = runtime().await;
        assert!(
            rt.evaluate("new in {", &rand()).await.is_err(),
            "an unparsable term is an error, not a result with errors"
        );
    }

    /// **The cost is a per-evaluation delta.** The accounting cell is shared for the runtime's life
    /// (replay seeds the per-deploy budget on it), so a result that reported the *total* would grow
    /// without bound and every deploy after the first would look over budget. Pre-charging a large
    /// amount and then asserting the result is *smaller* pins the subtraction.
    #[tokio::test]
    async fn the_reported_cost_is_this_evaluations_delta() {
        let rt = runtime().await;
        rt.cost()
            .charge(Cost::new(1_000_000, "pre-charge"))
            .expect("charge");

        let result = rt.evaluate("Nil", &rand()).await.expect("evaluate");
        assert!(
            result.cost.value < 1_000_000,
            "the result must be the delta, not the running total: {}",
            result.cost.value
        );
        assert!(
            rt.cost().total_charged() >= 1_000_000,
            "the cell itself still holds the pre-charge"
        );
    }

    /// The report records what the evaluation *did*: a COMM leaves a reporting event, and
    /// `get_report` drains them (the second call is empty), so a caller reading the report cannot
    /// see an event twice.
    #[tokio::test]
    async fn the_report_records_events_and_drains() {
        let rt = runtime().await;
        rt.evaluate(r#"new c in { c!(1) | for (@x <- c) { Nil } }"#, &rand())
            .await
            .expect("evaluate");

        let first = rt.get_report();
        assert!(
            first.iter().flatten().next().is_some(),
            "the COMM must leave a reporting event: {first:?}"
        );
        assert!(rt.get_report().is_empty(), "the report drains on read");
    }

    /// `consume_result` reports `None` when nothing is waiting.
    ///
    /// **And it also reports `None` when something *is* waiting on the channel.** With a datum that
    /// `get_data` reports as present, a probe with a binder pattern — the same pattern and datum that
    /// `RhoMatch::get` matches in isolation (`rholang/src/storage.rs`'s `rho_match_binds_free_vars`)
    /// — still comes back `None`, and the datum is still in the space afterwards. So the probe
    /// neither matches nor consumes. That is pinned as observed behaviour, and recorded in
    /// `spec/audit/passes.md` §16 as an **open question** rather than a confirmed defect: the matching arm is
    /// unreachable through this entry point, and I could not establish whether that is intended
    /// (`consume_result` may be a reporting placeholder) or a wiring gap.
    #[tokio::test]
    async fn an_unmatched_consume_result_is_none_and_leaves_a_waiter() {
        let rt = runtime().await;
        let channel = |name: &str| {
            SortedProc::new(rchain_models::par_ops::from_expr(
                rchain_models::ast::Expr::GString(name.to_string()),
            ))
        };
        // A binder pattern matches any datum (the `RhoMatch` matcher binds the free variable), which
        // is what makes this a "is anything waiting here" probe rather than a value comparison.
        let binder = || BindPattern {
            patterns: vec![SortedProc::new(Par {
                exprs: vec![rchain_models::ast::Expr::EVar(Box::new(
                    rchain_models::ast::Var::FreeVar(0),
                ))],
                connective_used: true,
                ..Default::default()
            })],
            remainder: None,
            free_count: 1,
        };

        assert!(
            rt.consume_result(&[channel("c")], &[binder()])
                .await
                .expect("no error")
                .is_none(),
            "an unmatched consume reports None"
        );

        // A datum is present — verified directly — and the probe still reports nothing, leaving the
        // datum in place.
        rt.evaluate(r#"@"c"!(42)"#, &rand())
            .await
            .expect("evaluate");
        assert_eq!(
            rt.get_data(&channel("c")).await.expect("get_data").len(),
            1,
            "the produce landed in the space"
        );
        assert!(
            rt.consume_result(&[channel("c")], &[binder()])
                .await
                .expect("no error")
                .is_none(),
            "the probe reports nothing even with the datum present"
        );
        assert_eq!(
            rt.get_data(&channel("c")).await.expect("get_data").len(),
            1,
            "…and it did not consume it either"
        );
    }
}
