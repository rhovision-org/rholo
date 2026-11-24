//! System-contract message unapplying (port of `ContractCall.scala`).

use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
use rchain_models::ast::{Expr, Par, Var};
use rchain_models::par_ops::from_expr;
use rchain_models::rholang::RhoType::RhoName;
use rchain_models::runtime::{BindPattern, ListParWithRandom, TaggedContinuation};
use rchain_models::sorted::SortedProc;

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::accounting::CostAccounting;
use crate::errors::RholangError;
use crate::reduce::{Dispatch, Tuplespace};
use crate::scheduler::DfsPath;

/// Unapplies a message sent to a system contract, returning the producer and the message content
/// (port of `ContractCall`). The producer is separated into [`ContractCall::produce`] plus the
/// recovered random state, so callers can send a reply without capturing the store.
#[derive(Clone)]
pub struct ContractCall<T: Tuplespace, D: Dispatch> {
    space: T,
    dispatcher: D,
    cost: Arc<CostAccounting>,
}

impl<T: Tuplespace, D: Dispatch> ContractCall<T, D> {
    /// The cost cell comes first so a caller can build it from the space it is about to move in
    /// (`ContractCall::new(space.cost().clone(), space, dispatcher)`), which is how the one production
    /// call site does it — argument evaluation is left to right, so any later position would borrow
    /// the space after it had moved.
    pub fn new(cost: Arc<CostAccounting>, space: T, dispatcher: D) -> Self {
        ContractCall {
            space,
            dispatcher,
            cost,
        }
    }

    /// The cost cell this call charges against (audit F-3).
    ///
    /// **Why a handler needs it.** Cost accounting charges on the *storage* path, proportional to the
    /// serialized size of what moves — the right charge for a move, and the wrong one for a fold.
    /// `rho:gov:censure` is cubic in its arguments and was charging nothing at all, so the per-block
    /// phlo cap could not contain it: a cap only bounds work when a phlo buys bounded work. A handler
    /// whose work outgrows its output has to charge for the work.
    pub fn cost(&self) -> &Arc<CostAccounting> {
        &self.cost
    }

    /// Send `values` through `ch`, dispatching any matched continuation (port of `produce`).
    /// `path` addresses the handler invocation this reply is a child of; the dispatched
    /// continuation is addressed at `path.child(0)` (the same convention as `apply_effect`).
    pub async fn produce(
        &self,
        rand: &Blake2b512Random,
        values: &[Par],
        ch: &Par,
        path: DfsPath,
    ) -> Result<(), RholangError> {
        let result = self
            .space
            .produce(
                &SortedProc::new(ch.clone()),
                ListParWithRandom {
                    pars: values.iter().map(|p| SortedProc::new(p.clone())).collect(),
                    random_state: rand.clone(),
                },
                false,
            )
            .await?;
        if let Some((continuation, data_list, _)) = result {
            let data: Vec<ListParWithRandom> = data_list
                .iter()
                .map(|(_, matched, _, _)| matched.clone())
                .collect();
            self.dispatcher
                .dispatch(continuation, data, path.child(0))
                .await?;
        }
        Ok(())
    }

    /// Extract the message content and its random state if there is exactly one argument (port of
    /// `unapply`).
    pub fn unapply(
        &self,
        contract_args: &[ListParWithRandom],
    ) -> Option<(Vec<Par>, Blake2b512Random)> {
        if let [single] = contract_args {
            Some((
                single.pars.iter().map(|p| p.as_par().clone()).collect(),
                single.random_state.clone(),
            ))
        } else {
            None
        }
    }

    /// The dispatch id for one **arity** of a minted channel: a function of the channel's bytes and
    /// that arity.
    ///
    /// Negative, so it can never collide with a `BodyRefs` constant (those are small non-negative
    /// ids), and derived from the name rather than from a counter — a counter would depend on how many
    /// handles *this dispatcher* has installed, which is not the same on play and on replay (replay
    /// builds a fresh dispatcher), so the continuation would dispatch nowhere on the second run. A
    /// hash of the name is a function of the name, and the name is a function of the deploy's RNG.
    ///
    /// **The arity is part of the id, and it is the arity of the *pattern* the channel carries.** A
    /// channel now carries exactly one installed continuation (Law 60), so the arity distinguishes a
    /// contract installed at `arity: 1, remainder: true` from the same name installed flat — which is
    /// what a byte-identical re-install has to be recognised as (`locked_install` skips it by
    /// comparing the consume, and this id is what that consume is built from).
    pub fn native_body_ref(name_bytes: &[u8], arity: i32) -> i64 {
        let digest = rchain_crypto::hash::blake2b256_hash::Blake2b256Hash::create_many(&[
            name_bytes,
            &arity.to_le_bytes(),
        ]);
        let mut first_eight = [0u8; 8];
        first_eight.copy_from_slice(digest.as_bytes().split_at(8).0);
        i64::from_be_bytes(first_eight) | i64::MIN
    }

    /// Bind `handler` to one pattern of a channel minted from `name_bytes`, returning that channel as
    /// a `Par`, ready to be handed out as a capability.
    ///
    /// This is the whole of the minted-channel primitive: build the name the way `reduce::alloc` does
    /// for a rholang `new` ([`RhoName::apply_bytes`]), register the handler against the id derived
    /// from those bytes and that arity, and install the persistent continuation the space will match a
    /// send against. `remainder` makes the last field a rest pattern (`arity: 1, remainder: true` is
    /// the shape a method-taking contract uses), exactly as `install_system_processes` builds it for a
    /// `Definition`.
    ///
    /// **One pattern per channel, not several** (AUDIT C219, Law 60). The space keeps one installed
    /// continuation per channel, so a second install of a *different* pattern on the same name is
    /// **refused** by `install_continuation` rather than replacing the first — calling this twice for
    /// two arities of one name used to leave only the second matching, silently. A contract with
    /// several methods is one continuation at `arity: 1, remainder: true` dispatching on the method
    /// inside it.
    ///
    /// **Register before installing**, so a failure leaves a handler nobody can reach rather than a
    /// continuation that dispatches into an empty table — the second is an error inside the reducer,
    /// the first is a name the caller is told about.
    ///
    /// **A persistent consume, not an `install`** (#669). `install` writes the hot store's
    /// `installed_continuations`, which is deliberately absent from `changes()`: it is the right
    /// shape for the fixed system processes, which are re-installed from compile-time definitions at
    /// every reset, and the wrong shape for a handle minted by a deploy. A consume with
    /// `persist: true` is a checkpointed `WaitingContinuation` and stays installed after it fires, so
    /// the continuation is still in the store in the next block and on replay. Its handler is
    /// re-built by the dispatcher's native-body resolver from the record
    /// `NativeSystemState::set_vault_handle_dispatch` writes at mint time.
    ///
    /// **The name is fresh**, so a consume that matches immediately (*`Some`*) means something else
    /// already produced on it; that is a bug, not a request to dispatch.
    pub async fn install_native(
        &self,
        name_bytes: Vec<u8>,
        arity: i32,
        remainder: bool,
        handler: crate::dispatch::ScalaBodyFn,
    ) -> Result<Par, RholangError> {
        let channel = RhoName::apply_bytes(name_bytes.clone());
        let body_ref = Self::native_body_ref(&name_bytes, arity);
        self.dispatcher.register(body_ref, handler)?;

        let patterns = vec![BindPattern {
            patterns: (0..arity)
                .map(|i| SortedProc::new(from_expr(Expr::EVar(Box::new(Var::FreeVar(i))))))
                .collect(),
            remainder: if remainder {
                Some(Var::FreeVar(arity))
            } else {
                None
            },
            free_count: if remainder { arity + 1 } else { arity },
        }];
        let matched = self
            .space
            .consume(
                &[SortedProc::new(channel.clone())],
                &patterns,
                TaggedContinuation::ScalaBodyRef(body_ref),
                true,
                BTreeSet::new(),
            )
            .await?;
        if matched.is_some() {
            return Err(RholangError::BugFoundError(format!(
                "a freshly minted vault handle ({body_ref}) already carried data; refusing to \
                 dispatch against a name the caller has not been handed yet"
            )));
        }
        Ok(channel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};

    use rchain_models::ast::Expr;
    use rchain_models::par_ops::from_expr;
    use rchain_models::runtime::{BindPattern, TaggedContinuation};

    use crate::reduce::Application;

    /// A space that records what was produced and returns a canned `Application`, so the two arms
    /// (matched / not matched) are chosen by the test rather than by a real store.
    #[derive(Default)]
    struct RecordingSpace {
        produced: Mutex<Vec<(SortedProc, ListParWithRandom, bool)>>,
        /// `None` = nothing waiting; `Some` = a match to return.
        reply: Application,
        /// When set, `produce` fails with this message.
        fail: Option<String>,
    }

    #[async_trait::async_trait]
    impl Tuplespace for RecordingSpace {
        async fn produce(
            &self,
            channel: &SortedProc,
            data: ListParWithRandom,
            persist: bool,
        ) -> Result<Application, RholangError> {
            self.produced
                .lock()
                .unwrap()
                .push((channel.clone(), data, persist));
            match &self.fail {
                Some(msg) => Err(RholangError::ReduceError(msg.clone())),
                None => Ok(self.reply.clone()),
            }
        }
        async fn consume(
            &self,
            _channels: &[SortedProc],
            _patterns: &[BindPattern],
            _continuation: TaggedContinuation,
            _persist: bool,
            _peeks: BTreeSet<usize>,
        ) -> Result<Application, RholangError> {
            unreachable!("a system-contract reply never consumes")
        }
    }

    /// `ContractCall` takes its space and dispatcher by value, so the test needs to read them back
    /// after the call — hence the `Arc` forwarding impls (the same shape `dispatch.rs` uses for
    /// `Arc<RholangAndScalaDispatcher>`).
    #[async_trait::async_trait]
    impl Tuplespace for Arc<RecordingSpace> {
        async fn produce(
            &self,
            channel: &SortedProc,
            data: ListParWithRandom,
            persist: bool,
        ) -> Result<Application, RholangError> {
            self.as_ref().produce(channel, data, persist).await
        }
        async fn consume(
            &self,
            channels: &[SortedProc],
            patterns: &[BindPattern],
            continuation: TaggedContinuation,
            persist: bool,
            peeks: BTreeSet<usize>,
        ) -> Result<Application, RholangError> {
            self.as_ref()
                .consume(channels, patterns, continuation, persist, peeks)
                .await
        }
    }

    /// Records the dispatch's three arguments.
    #[derive(Default)]
    struct RecordingDispatch {
        dispatched: Mutex<Vec<(TaggedContinuation, Vec<ListParWithRandom>, DfsPath)>>,
    }

    #[async_trait::async_trait]
    impl Dispatch for RecordingDispatch {
        async fn dispatch(
            &self,
            continuation: TaggedContinuation,
            data_list: Vec<ListParWithRandom>,
            path: DfsPath,
        ) -> Result<(), RholangError> {
            self.dispatched
                .lock()
                .unwrap()
                .push((continuation, data_list, path));
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl Dispatch for Arc<RecordingDispatch> {
        async fn dispatch(
            &self,
            continuation: TaggedContinuation,
            data_list: Vec<ListParWithRandom>,
            path: DfsPath,
        ) -> Result<(), RholangError> {
            self.as_ref().dispatch(continuation, data_list, path).await
        }
    }

    /// A cost cell for the tests below. `ContractCall` always carries one in production (the
    /// system-process layer passes the `ChargingRSpace`'s); the tests only need a live cell so a
    /// handler that charges has something to charge against (audit F-3).
    fn test_cost() -> Arc<CostAccounting> {
        Arc::new(CostAccounting::from_initial(
            crate::accounting::Costs::unsafe_max(),
        ))
    }

    fn lpw(values: &[i64]) -> ListParWithRandom {
        ListParWithRandom {
            pars: values
                .iter()
                .map(|v| SortedProc::new(from_expr(Expr::GInt(*v))))
                .collect(),
            random_state: Blake2b512Random::from_init(&[3u8; 32]),
        }
    }

    fn pars(values: &[i64]) -> Vec<SortedProc> {
        values
            .iter()
            .map(|v| SortedProc::new(from_expr(Expr::GInt(*v))))
            .collect()
    }

    fn ch(name: &str) -> Par {
        from_expr(Expr::GString(name.to_string()))
    }

    /// `unapply` is the *exactly one argument* pattern (Scala's `case List(single) =>`): a system
    /// contract's reply takes one message, so anything else must not be unwrapped — the second
    /// element of a two-argument reply would otherwise be silently dropped.
    #[test]
    fn unapply_requires_exactly_one_argument() {
        let call = ContractCall::new(
            test_cost(),
            Arc::new(RecordingSpace::default()),
            Arc::new(RecordingDispatch::default()),
        );

        let (values, rand) = call.unapply(&[lpw(&[7])]).expect("one argument unwraps");
        assert_eq!(values, vec![from_expr(Expr::GInt(7))]);
        assert_eq!(
            rand,
            Blake2b512Random::from_init(&[3u8; 32]),
            "the argument's random state comes back with it"
        );

        assert!(call.unapply(&[]).is_none(), "no argument does not unwrap");
        assert!(
            call.unapply(&[lpw(&[1]), lpw(&[2])]).is_none(),
            "two arguments do not unwrap"
        );
    }

    /// A reply with nothing waiting produces and dispatches nothing — the common case for a
    /// fire-and-forget system process.
    #[tokio::test]
    async fn a_reply_with_no_match_dispatches_nothing() {
        let space = Arc::new(RecordingSpace::default());
        let dispatch = Arc::new(RecordingDispatch::default());
        let call = ContractCall::new(test_cost(), space.clone(), dispatch.clone());

        call.produce(
            &Blake2b512Random::from_init(&[1u8; 32]),
            &[from_expr(Expr::GInt(9))],
            &ch("out"),
            DfsPath::root(),
        )
        .await
        .expect("produce");

        let produced = space.produced.lock().unwrap();
        assert_eq!(produced.len(), 1);
        assert_eq!(produced[0].0, SortedProc::new(ch("out")));
        assert_eq!(produced[0].1.pars, pars(&[9]));
        assert!(!produced[0].2, "a reply is not persistent");
        assert!(
            dispatch.dispatched.lock().unwrap().is_empty(),
            "no match, no continuation"
        );
    }

    /// A matched reply dispatches at **`path.child(0)`** — the same convention `apply_effect` uses,
    /// so the continuation's own path is a child of the handler invocation that produced it. The
    /// path is what the scheduler keys its per-path bookkeeping on, so getting it wrong is a
    /// scheduling bug rather than a cosmetic one.
    #[tokio::test]
    async fn a_matched_reply_dispatches_at_the_child_path() {
        let space = Arc::new(RecordingSpace {
            produced: Mutex::new(Vec::new()),
            // The 4-tuple is `(channel, matched data, removed data, persistent)`; the matched and
            // removed values are deliberately *different* here (the peek shape), so the assertion
            // below distinguishes the field the continuation receives from the one that left the
            // space.
            reply: Some((
                TaggedContinuation::Empty,
                vec![(SortedProc::new(ch("out")), lpw(&[4]), lpw(&[99]), false)],
                false,
            )),
            fail: None,
        });
        let dispatch = Arc::new(RecordingDispatch::default());
        let call = ContractCall::new(test_cost(), space, dispatch.clone());

        call.produce(
            &Blake2b512Random::from_init(&[1u8; 32]),
            &[from_expr(Expr::GInt(9))],
            &ch("out"),
            DfsPath(vec![2, 5]),
        )
        .await
        .expect("produce");

        let dispatched = dispatch.dispatched.lock().unwrap();
        assert_eq!(dispatched.len(), 1);
        assert_eq!(
            dispatched[0].2,
            DfsPath(vec![2, 5, 0]),
            "the continuation is addressed at the *child* of the invoking path"
        );
        assert_eq!(
            dispatched[0].1.len(),
            1,
            "exactly the matched datum is dispatched"
        );
        assert_eq!(
            dispatched[0].1[0].pars,
            pars(&[4]),
            "the *matched* datum is what the continuation receives, not the removed one"
        );
    }

    /// A space error is propagated, not swallowed: a system contract whose reply could not be
    /// produced must fail the deploy rather than report success.
    #[tokio::test]
    async fn a_produce_error_is_propagated() {
        let call = ContractCall::new(
            test_cost(),
            Arc::new(RecordingSpace {
                fail: Some("store is down".to_string()),
                ..RecordingSpace::default()
            }),
            Arc::new(RecordingDispatch::default()),
        );

        let err = call
            .produce(
                &Blake2b512Random::from_init(&[1u8; 32]),
                &[],
                &ch("out"),
                DfsPath::root(),
            )
            .await
            .expect_err("the space's error must escape");
        assert!(format!("{err}").contains("store is down"), "{err}");
    }
}
