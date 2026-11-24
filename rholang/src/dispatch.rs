//! Continuation dispatch (port of `dispatch.scala`).

use rchain_shared::lock::Unpoison;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};

use async_trait::async_trait;
use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
use rchain_models::ast::Par;
use rchain_models::runtime::{ListParWithRandom, TaggedContinuation};

use crate::env::Env;
use crate::errors::RholangError;
use crate::reduce::Dispatch;
use crate::scheduler::DfsPath;

/// A built-in continuation handler (port of the dispatch-table function), invoked at the DFS path
/// of its `ScalaBodyRef` continuation. Handlers that reply through [`ContractCall`] pass the path
/// on so the reply's matched continuation keeps its position in the scheduler's order (Laws 20–22).
pub type ScalaBodyFn = Box<
    dyn Fn(
            Vec<ListParWithRandom>,
            DfsPath,
        ) -> Pin<Box<dyn Future<Output = Result<(), RholangError>> + Send>>
        + Send
        + Sync,
>;

/// The `ParBody` continuation evaluator: evals a body in the env built from the matched data with
/// the merged random state, at the continuation's DFS path.
pub type EvalBodyFn = Box<
    dyn Fn(
            Par,
            Env<Par>,
            Blake2b512Random,
            DfsPath,
        ) -> Pin<Box<dyn Future<Output = Result<(), RholangError>> + Send>>
        + Send
        + Sync,
>;

/// Resolves a **minted** native continuation id that this dispatcher has not registered (#669).
///
/// A vault handle's continuation is checkpointed, but the dispatcher is not: play reuses a table
/// built in an earlier block and replay starts with only the fixed system processes. The resolver
/// reads the id's durable record and re-builds the handler, so both paths run the same code. It is
/// asked only for negative (`ScalaBodyRef` minted) ids; a non-negative `BodyRef` that is missing is
/// a bug, not a lookup.
#[async_trait]
pub trait NativeBodyResolver: Send + Sync {
    async fn resolve(&self, body_ref: i64) -> Result<Option<ScalaBodyFn>, RholangError>;
}

/// Build an environment from the data captured by a match (port of `Dispatch.buildEnv`).
pub fn build_env(data_list: &[ListParWithRandom]) -> Env<Par> {
    Env::make_env(
        data_list
            .iter()
            .flat_map(|d| d.pars.iter().map(|p| p.as_par().clone())),
    )
}

/// Dispatches a continuation: eval `ParBody`, invoke the built-in handler for `ScalaBodyRef`, or
/// no-op for `Empty` (port of `RholangAndScalaDispatcher`).
///
/// `eval` is set after construction to break the reducer↔dispatcher cycle.
pub struct RholangAndScalaDispatcher {
    /// `Arc` around each handler so a resolved id can be both registered and called: a
    /// `ScalaBodyFn` is a boxed closure and cannot be cloned, and a resolution that used the
    /// handler without keeping it would ask the resolver again on every send.
    dispatch_table: Mutex<BTreeMap<i64, Arc<ScalaBodyFn>>>,
    eval: Mutex<Option<EvalBodyFn>>,
    native_resolver: Mutex<Option<Arc<dyn NativeBodyResolver>>>,
}

impl RholangAndScalaDispatcher {
    pub fn new(dispatch_table: BTreeMap<i64, ScalaBodyFn>) -> Self {
        RholangAndScalaDispatcher {
            dispatch_table: Mutex::new(
                dispatch_table
                    .into_iter()
                    .map(|(id, handler)| (id, Arc::new(handler)))
                    .collect(),
            ),
            eval: Mutex::new(None),
            native_resolver: Mutex::new(None),
        }
    }

    pub fn set_eval(&self, eval: EvalBodyFn) {
        *self.eval.lock().unpoison() = Some(eval);
    }

    pub fn set_dispatch_table(&self, table: BTreeMap<i64, ScalaBodyFn>) {
        *self.dispatch_table.lock().unpoison() = table
            .into_iter()
            .map(|(id, handler)| (id, Arc::new(handler)))
            .collect();
    }

    /// Attach the resolver a fresh dispatcher uses for minted native ids (#669). Called once per
    /// runtime, after the system-process table is built and before any deploy runs.
    pub fn set_native_resolver(&self, resolver: Arc<dyn NativeBodyResolver>) {
        *self.native_resolver.lock().unpoison() = Some(resolver);
    }

    /// Bind one handler to one id, leaving every other entry alone — the entry point the vault
    /// capability needs (see [`Dispatch::register`](crate::reduce::Dispatch::register)).
    ///
    /// **Re-registering the same id is a no-op, not a replacement.** The install path is re-run on
    /// replay and a deploy may legitimately call `findOrCreate` twice for one address within a block;
    /// the second call must reach the same handler rather than swap in a new closure under a
    /// continuation the space already holds. The check-then-insert is under the table's own lock, so
    /// two concurrent installs of one id cannot both win.
    pub fn register_handler(
        &self,
        body_ref: i64,
        handler: ScalaBodyFn,
    ) -> Result<(), RholangError> {
        self.register_handler_arc(body_ref, Arc::new(handler));
        Ok(())
    }

    fn register_handler_arc(&self, body_ref: i64, handler: Arc<ScalaBodyFn>) {
        let mut table = self.dispatch_table.lock().unpoison();
        if table.contains_key(&body_ref) {
            return;
        }
        table.insert(body_ref, handler);
    }
}

#[async_trait]
impl Dispatch for RholangAndScalaDispatcher {
    async fn dispatch(
        &self,
        continuation: TaggedContinuation,
        data_list: Vec<ListParWithRandom>,
        path: DfsPath,
    ) -> Result<(), RholangError> {
        match &continuation {
            TaggedContinuation::ParBody(pwr) => {
                let env = build_env(&data_list);
                // Order-sensitive merge, Scala-faithful (`dispatch.scala:33`:
                // `parWithRand.randomState +: dataList.map(_.randomState)`): continuation first, then
                // the matched data in receive-bind order. `data_list` order is canonical (extracted in
                // pattern order by `extract_data_candidates`), so this is deterministic — do NOT sort
                // here (unlike the mergeable-channel merge, which merges an unordered set of branches).
                let mut randoms: Vec<Blake2b512Random> = vec![pwr.random_state.clone()];
                randoms.extend(data_list.iter().map(|d| d.random_state.clone()));
                let merged = Blake2b512Random::merge(&randoms);
                let fut = {
                    let eval = self.eval.lock().unpoison();
                    let f = eval.as_ref().ok_or_else(|| {
                        RholangError::BugFoundError("dispatcher eval not set".to_string())
                    })?;
                    f(pwr.body.as_par().clone(), env, merged, path)
                };
                fut.await
            }
            TaggedContinuation::ScalaBodyRef(r) => {
                let r = *r;
                let known = {
                    let table = self.dispatch_table.lock().unpoison();
                    table.get(&r).cloned()
                };
                let handler = match known {
                    Some(handler) => handler,
                    // A minted id this dispatcher has not seen: replay starts empty, and the live
                    // runtime's table dies with its first block. Ask the durable-state resolver.
                    None if r < 0 => {
                        let resolver = {
                            let attached = self.native_resolver.lock().unpoison();
                            attached.clone()
                        }
                        .ok_or_else(|| {
                            RholangError::BugFoundError(format!(
                                "dispatch: no native-body resolver for minted continuation {r}"
                            ))
                        })?;
                        // **Not under the table lock.** The resolver reads native state, which may
                        // await the store; holding the dispatcher's mutex across that would serialize
                        // every other dispatch behind one I/O.
                        let resolved = resolver.resolve(r).await?.ok_or_else(|| {
                            RholangError::ReduceError(format!(
                                "dispatch: minted continuation {r} has no durable handler record"
                            ))
                        })?;
                        let handler = Arc::new(resolved);
                        self.register_handler_arc(r, handler.clone());
                        handler
                    }
                    None => {
                        return Err(RholangError::ReduceError(format!(
                            "dispatch: no function for {r}"
                        )))
                    }
                };
                handler(data_list, path).await
            }
            TaggedContinuation::Empty => Ok(()),
        }
    }

    /// Forward to the concrete table: the `Arc` impl exists so handlers can hold the dispatcher
    /// cheaply, and a registration that stopped at the wrapper would be a capability that works for
    /// one holder and silently not for another.
    fn register(&self, body_ref: i64, handler: ScalaBodyFn) -> Result<(), RholangError> {
        self.register_handler(body_ref, handler)
    }

    /// Drop minted handlers; the fixed system processes are the non-negative ids and stay.
    fn clear_minted_handlers(&self) {
        let mut table = self.dispatch_table.lock().unpoison();
        table.retain(|id, _| *id >= 0);
    }
}

#[async_trait]
impl Dispatch for Arc<RholangAndScalaDispatcher> {
    async fn dispatch(
        &self,
        continuation: TaggedContinuation,
        data_list: Vec<ListParWithRandom>,
        path: DfsPath,
    ) -> Result<(), RholangError> {
        self.as_ref().dispatch(continuation, data_list, path).await
    }

    fn register(&self, body_ref: i64, handler: ScalaBodyFn) -> Result<(), RholangError> {
        self.as_ref().register_handler(body_ref, handler)
    }

    fn clear_minted_handlers(&self) {
        self.as_ref().clear_minted_handlers();
    }
}

#[async_trait]
impl Dispatch for Weak<RholangAndScalaDispatcher> {
    /// Upgrade the weak reference for the duration of the dispatch. System-process handlers hold the
    /// dispatcher weakly so the dispatcher does not keep itself (and the whole forked runtime) alive
    /// through its own dispatch table (issues #18/#23).
    async fn dispatch(
        &self,
        continuation: TaggedContinuation,
        data_list: Vec<ListParWithRandom>,
        path: DfsPath,
    ) -> Result<(), RholangError> {
        let dispatcher = self.upgrade().ok_or_else(|| {
            RholangError::BugFoundError("system dispatcher has been dropped".to_string())
        })?;
        dispatcher.dispatch(continuation, data_list, path).await
    }

    /// The same upgrade-then-forward, and it is load-bearing rather than symmetry: a system-process
    /// handler holds the dispatcher **weakly** (issues #18/#23), so the handler that installs a
    /// minted channel's continuation is exactly the holder that would otherwise fall back to the
    /// refusing default.
    fn register(&self, body_ref: i64, handler: ScalaBodyFn) -> Result<(), RholangError> {
        let dispatcher = self.upgrade().ok_or_else(|| {
            RholangError::BugFoundError("system dispatcher has been dropped".to_string())
        })?;
        dispatcher.register_handler(body_ref, handler)
    }

    fn clear_minted_handlers(&self) {
        if let Some(dispatcher) = self.upgrade() {
            dispatcher.clear_minted_handlers();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_models::runtime::ParWithRandom;
    use rchain_models::sorted::SortedProc;

    fn data(pars: Vec<Par>) -> ListParWithRandom {
        ListParWithRandom {
            pars: pars.into_iter().map(SortedProc::new).collect(),
            random_state: Blake2b512Random::new_random(128),
        }
    }

    fn int(n: i64) -> Par {
        Par {
            exprs: vec![rchain_models::ast::Expr::GInt(n)],
            ..Default::default()
        }
    }

    /// The environment is built positionally from the matched data, and `make_env` folds with
    /// `put`, which **prepends**: the *last* datum of the receive is index 0 and the first is the
    /// highest index — the De Bruijn convention the normalizer numbers a receive's variables with.
    /// Asserted explicitly because a change to the fold order is a change of *bindings*, not a
    /// cosmetic one (the execution corpus would catch it end-to-end, this catches it in one place).
    #[test]
    fn build_env_binds_the_data_by_de_bruijn_index() {
        let env = build_env(&[data(vec![int(10), int(11)]), data(vec![int(12)])]);
        let value = |i: i32| env.get(i).and_then(|p| p.exprs.first().cloned());
        assert_eq!(value(0), Some(rchain_models::ast::Expr::GInt(12)));
        assert_eq!(value(1), Some(rchain_models::ast::Expr::GInt(11)));
        assert_eq!(value(2), Some(rchain_models::ast::Expr::GInt(10)));
        assert_eq!(value(3), None, "nothing beyond the data");
    }

    /// **The minted-channel primitive's handler half (vault capability, stage 1).** A registration
    /// is per-entry — it must not disturb the constants `install_system_processes` put in the table —
    /// and re-registering one id is a no-op rather than a swap, because the install path runs again
    /// on replay and a deploy may mint the same handle twice; a second closure under a continuation
    /// the space already holds would make the two runs disagree about what the channel does.
    #[tokio::test]
    async fn a_registration_is_per_entry_and_idempotent() {
        let mut table: BTreeMap<i64, ScalaBodyFn> = BTreeMap::new();
        table.insert(7, Box::new(|_data, _path| Box::pin(async { Ok(()) })));
        let dispatcher = RholangAndScalaDispatcher::new(table);

        let first: ScalaBodyFn = Box::new(|_data, _path| {
            Box::pin(async { Err(RholangError::ReduceError("first".to_string())) })
        });
        dispatcher
            .register_handler(-5, first)
            .expect("a free id registers");

        // The constant that was already there is untouched, and the new id is now occupied.
        {
            let table = dispatcher.dispatch_table.lock().unpoison();
            assert_eq!(table.len(), 2, "one entry added, not a replacement");
            assert!(table.contains_key(&7), "the constant survives");
            assert!(table.contains_key(&-5));
        }

        // A second registration of the same id is a no-op: the *first* handler is still the one the
        // id dispatches to, which is the property replay depends on.
        let second: ScalaBodyFn = Box::new(|_data, _path| {
            Box::pin(async { Err(RholangError::ReduceError("second".to_string())) })
        });
        dispatcher
            .register_handler(-5, second)
            .expect("re-registering is not an error");
        let err = dispatcher
            .dispatch(
                TaggedContinuation::ScalaBodyRef(-5),
                vec![],
                DfsPath::default(),
            )
            .await
            .expect_err("the first handler errors");
        assert_eq!(err.to_string(), "first");
    }

    #[test]
    fn build_env_of_no_data_is_empty() {
        let env = build_env(&[]);
        assert_eq!(env.get(0), None);
    }

    /// The `ParBody` path needs its evaluator: the reducer↔dispatcher cycle is broken deliberately
    /// (the eval closure is set after construction), so a dispatch that arrives before it is wired
    /// must **say so** rather than silently no-op and drop a continuation.
    #[tokio::test]
    async fn a_par_body_without_an_evaluator_is_a_bug_not_a_silent_no_op() {
        let dispatcher = RholangAndScalaDispatcher::new(BTreeMap::new());
        let err = dispatcher
            .dispatch(
                TaggedContinuation::ParBody(ParWithRandom {
                    body: SortedProc::new(int(1)),
                    random_state: Blake2b512Random::new_random(128),
                }),
                // One datum, so the random merge has the two inputs it asserts on (see the
                // zero-data test below, which pins that assertion).
                vec![data(vec![int(1)])],
                DfsPath::root(),
            )
            .await
            .expect_err("an unset evaluator must be reported");
        assert!(
            matches!(err, RholangError::BugFoundError(_)),
            "expected a bug report, got {err:?}"
        );
    }

    /// **Recorded, not endorsed** (AUDIT.md C5): a `ParBody` continuation dispatched with **no
    /// matched data** panics inside `Blake2b512Random::merge`, which asserts at least two inputs —
    /// the dispatcher always prepends the continuation's own random, so zero data means one input.
    /// The reducer does not currently dispatch a `ParBody` with zero data (a receive always matches
    /// at least the datum that triggered it, and a match with nothing to run becomes
    /// `TaggedContinuation::Empty`), so this is a latent panic on an unreachable path. Pinned so that
    /// if the reachability ever changes — or someone adds the guard — the change is deliberate
    /// rather than discovered by a node crashing.
    #[tokio::test]
    #[should_panic(expected = "at least 2 inputs")]
    async fn a_par_body_with_no_matched_data_panics_in_merge() {
        let dispatcher = RholangAndScalaDispatcher::new(BTreeMap::new());
        let _ = dispatcher
            .dispatch(
                TaggedContinuation::ParBody(ParWithRandom {
                    body: SortedProc::new(int(1)),
                    random_state: Blake2b512Random::new_random(128),
                }),
                vec![],
                DfsPath::root(),
            )
            .await;
    }

    /// A `ScalaBodyRef` naming a body that is not installed is an error naming the ref — a system
    /// process reached for but never registered must not fail silently.
    #[tokio::test]
    async fn an_unregistered_body_ref_names_the_ref() {
        let dispatcher = RholangAndScalaDispatcher::new(BTreeMap::new());
        let err = dispatcher
            .dispatch(
                TaggedContinuation::ScalaBodyRef(42),
                vec![],
                DfsPath::root(),
            )
            .await
            .expect_err("an unregistered body ref must be reported");
        let rendered = format!("{err:?}");
        assert!(rendered.contains("42"), "{rendered}");
        assert!(rendered.contains("no function"), "{rendered}");
    }

    /// An `Empty` continuation is the deliberate no-op (a match that produced nothing to run), so it
    /// must succeed rather than be reported as a missing handler.
    #[tokio::test]
    async fn an_empty_continuation_is_a_no_op() {
        let dispatcher = RholangAndScalaDispatcher::new(BTreeMap::new());
        assert!(dispatcher
            .dispatch(TaggedContinuation::Empty, vec![], DfsPath::root())
            .await
            .is_ok());
    }

    /// A registered handler is invoked, and the `ParBody` evaluator receives the environment built
    /// from the data and the random state merged from the continuation **and** the data.
    #[tokio::test]
    async fn a_registered_handler_and_a_set_evaluator_are_both_reached() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let calls = Arc::new(AtomicUsize::new(0));
        let handler_calls = calls.clone();
        let mut table: BTreeMap<i64, ScalaBodyFn> = BTreeMap::new();
        table.insert(
            7,
            Box::new(move |_data, _path| {
                let calls = handler_calls.clone();
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            }),
        );
        let dispatcher = RholangAndScalaDispatcher::new(table);

        dispatcher
            .dispatch(TaggedContinuation::ScalaBodyRef(7), vec![], DfsPath::root())
            .await
            .expect("the handler is registered");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // The evaluator, once set, is reached with the built environment.
        let seen = Arc::new(Mutex::new(None));
        let seen_env = seen.clone();
        dispatcher.set_eval(Box::new(move |_body, env, _rand, _path| {
            let seen = seen_env.clone();
            Box::pin(async move {
                *seen.lock().unwrap() = Some(env.get(0).is_some());
                Ok(())
            })
        }));
        dispatcher
            .dispatch(
                TaggedContinuation::ParBody(ParWithRandom {
                    body: SortedProc::new(int(1)),
                    random_state: Blake2b512Random::new_random(128),
                }),
                vec![data(vec![int(9)])],
                DfsPath::root(),
            )
            .await
            .expect("the evaluator is set");
        assert_eq!(
            *seen.lock().unwrap(),
            Some(true),
            "the env passed to the evaluator carries the matched data"
        );
    }

    /// The weak dispatcher exists so a system process does not keep its runtime alive through the
    /// dispatch table (issues #18/#23). Once the dispatcher is dropped, dispatching through the weak
    /// handle is a bug report — not a panic and not a silent success.
    #[tokio::test]
    async fn a_dropped_dispatcher_is_reported_through_the_weak_handle() {
        let dispatcher = Arc::new(RholangAndScalaDispatcher::new(BTreeMap::new()));
        let weak: Weak<RholangAndScalaDispatcher> = Arc::downgrade(&dispatcher);
        drop(dispatcher);

        let err = weak
            .dispatch(TaggedContinuation::Empty, vec![], DfsPath::root())
            .await
            .expect_err("a dropped dispatcher must be reported");
        assert!(
            matches!(err, RholangError::BugFoundError(_)),
            "expected a bug report, got {err:?}"
        );
    }
}
