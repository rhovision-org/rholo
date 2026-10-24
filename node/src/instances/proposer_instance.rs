//! Proposer instance (port of `node/instances/ProposerInstance.scala`).
//!
//! Drains propose requests, serializing actual proposal through a semaphore; concurrent attempts
//! resolve to `ProposerResult::Empty` and set the `trigger` flag, so a propose that arrives while
//! one is running is re-enqueued once the running one finishes (the Scala's trigger re-enqueue).
//!
//! **A "not due" outcome is retried by the node itself** ([`NOT_DUE_RETRY`]). The round gate's wall-clock
//! escape is only evaluated when something asks this node to propose, and with `--no-autopropose` nothing
//! does until the next deploy or remote block arrives — so a deploy refused by the gate on a quiet net sat
//! in the pool until someone else spoke (#213, #219).
//!
//! **Every propose outcome is logged here, where it is produced.** Most propose requests discard
//! their result — the `proposeOnDeploy` path (`let _ = trigger(true).await`) and both autopropose
//! taps (`try_send` with the reply receiver dropped) — so a proposer that fails on every attempt
//! stops block production *silently*: `/api/status` keeps serving, the container stays healthy, and
//! the log stays empty while the height never moves. Logging at the point of production makes every
//! one of those paths visible at once, including the ones with no caller left to report to.

use rchain_shared::chan;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::{mpsc, oneshot, Semaphore};
use tokio_stream::wrappers::ReceiverStream;

use rchain_casper::blocks::proposer::propose_result::{ProposeResult, ProposeStatus};
use rchain_casper::blocks::proposer::proposer::{ProposeSource, Proposer, ProposerResult};
use rchain_casper::state::ProposerState;
use rchain_models::casper::protocol::casper_message::BlockMessage;
use rchain_shared::log::{Log, LogSource};

use crate::resources::ResourceGuard;

/// Log one propose outcome. Routine outcomes (no new deploys, not bonded, a propose already in
/// flight) are `debug`; every outcome that means *a block was expected and was not produced* is
/// louder, so a stalled proposer cannot hide behind a serving node. A `BugError` is an internal
/// failure — the class the wedged devnet hit with "Fringe state not available in state cache" —
/// and is logged at `error` with the reason, because it never resolves on its own.
fn log_propose_result(log: &Arc<dyn Log>, result: &(ProposeResult, Option<BlockMessage>)) {
    let source = LogSource::new("coop.rchain.node.instances.ProposerInstance");
    let (propose_result, block) = result;
    match block {
        Some(block) => log.info(
            source,
            &format!(
                "proposed and added block #{} (seq {})",
                block.block_number, block.seq_num
            ),
        ),
        None => match &propose_result.propose_status {
            ProposeStatus::ProposeSuccess => log.info(source, "propose succeeded (no new block)"),
            ProposeStatus::BugError(reason) => log.error(
                source,
                &format!(
                    "propose failed with an internal error: {reason} — no block was produced, so \
                     the chain will not advance until this is fixed"
                ),
            ),
            ProposeStatus::ResourcePressure(state) => log.warn(
                source,
                &format!(
                    "propose withheld: the node is {state} and is not producing (resource fallback,                      unit C)"
                ),
            ),
            status @ (ProposeStatus::TooFarAheadOfLastFinalized
            | ProposeStatus::InternalDeployError) => {
                log.warn(source, &format!("propose failed: {status}"))
            }
            status => log.debug(source, &format!("propose produced no block: {status}")),
        },
    }
}

/// How long after a "not due" outcome the node asks itself to propose again.
///
/// `NotEnoughNewBlocks` is what the proposer returns when the round gate refuses (this validator already
/// spoke this round) and when its own block lost a stale-snapshot race (§48). Both say "the next attempt
/// re-derives and succeeds", and the gate's stall escape fires on the first attempt made at least
/// `ROUND_STALL_ESCAPE` after the refusal began. The retry is therefore set just past that bound, so one
/// retry is enough to take the escape when the round is genuinely stuck, and a round that closed in the
/// meantime simply proposes normally.
///
/// It cannot become a storm: at most one retry is pending at a time, and the retry is an `Automatic`
/// propose, so with an empty pool it goes through the attestation guard like any tap and is withheld when
/// there is nothing to finalise or no quorum to reach.
pub const NOT_DUE_RETRY: Duration = Duration::from_millis(
    rchain_casper::blocks::proposer::proposer::ROUND_STALL_ESCAPE.as_millis() as u64 + 1_000,
);

/// Whether a propose outcome is "not due yet", which the node retries on its own.
fn not_due(result: &(ProposeResult, Option<BlockMessage>)) -> bool {
    result.1.is_none() && result.0.propose_status == ProposeStatus::NotEnoughNewBlocks
}

/// Create the proposer stream (port of `ProposerInstance.create`).
pub fn create(
    propose_requests_rx: mpsc::Receiver<(ProposeSource, oneshot::Sender<ProposerResult>)>,
    propose_requests_tx: mpsc::Sender<(ProposeSource, oneshot::Sender<ProposerResult>)>,
    proposer: Proposer,
    state: Arc<tokio::sync::Mutex<ProposerState>>,
    resources: Arc<ResourceGuard>,
    log: Arc<dyn Log>,
) -> impl tokio_stream::Stream<Item = (ProposeResult, Option<BlockMessage>)> + Send + 'static {
    create_with_retry(
        propose_requests_rx,
        propose_requests_tx,
        proposer,
        state,
        resources,
        log,
        NOT_DUE_RETRY,
    )
}

fn create_with_retry(
    propose_requests_rx: mpsc::Receiver<(ProposeSource, oneshot::Sender<ProposerResult>)>,
    propose_requests_tx: mpsc::Sender<(ProposeSource, oneshot::Sender<ProposerResult>)>,
    proposer: Proposer,
    state: Arc<tokio::sync::Mutex<ProposerState>>,
    resources: Arc<ResourceGuard>,
    log: Arc<dyn Log>,
    not_due_retry: Duration,
) -> impl tokio_stream::Stream<Item = (ProposeResult, Option<BlockMessage>)> + Send + 'static {
    let input = ReceiverStream::new(propose_requests_rx);
    let lock = Arc::new(Semaphore::new(1));
    let trigger = Arc::new(AtomicBool::new(false));
    let retry_pending = Arc::new(AtomicBool::new(false));
    let proposer = Arc::new(proposer);

    input
        .map(move |(source, propose_id_def)| {
            let lock = lock.clone();
            let trigger = trigger.clone();
            let retry_pending = retry_pending.clone();
            let state = state.clone();
            let tx = propose_requests_tx.clone();
            let proposer = proposer.clone();
            let resources = resources.clone();
            let log = log.clone();
            async move {
                let permit = match lock.clone().try_acquire_owned() {
                    Ok(p) => p,
                    Err(_) => {
                        chan::oneshot_send(propose_id_def, ProposerResult::Empty);
                        trigger.store(true, Ordering::SeqCst);
                        return None;
                    }
                };

                // **The fallback gate** (unit C, B37): a node that has stopped producing does not
                // call the proposer at all. The requester gets a refusal naming the state, and no
                // retry is scheduled — the autopropose timer asks again, so recovery needs no restart.
                let fallback = resources.snapshot().fallback;
                if fallback.stops_production() {
                    let result = (
                        ProposeResult {
                            propose_status: ProposeStatus::ResourcePressure(
                                fallback.as_str().to_string(),
                            ),
                        },
                        None,
                    );
                    chan::oneshot_send(
                        propose_id_def,
                        ProposerResult::Failure {
                            status: result.0.propose_status.clone(),
                            seq_number: 0,
                            message: format!(
                                "the node is {} ({}); it is not proposing",
                                fallback.as_str(),
                                resources.snapshot().fallback_reason
                            ),
                        },
                    );
                    log_propose_result(&log, &result);
                    state.lock().await.latest_propose_result = Some(result.clone());
                    drop(permit);
                    return Some(result);
                }

                let (r_tx, r_rx) = oneshot::channel();
                {
                    state.lock().await.curr_propose_result = Some(r_rx);
                }
                let r = proposer.propose(source, propose_id_def).await;
                let r = match r {
                    Ok(r) => r,
                    Err(e) => (
                        ProposeResult {
                            propose_status: ProposeStatus::BugError(e),
                        },
                        None,
                    ),
                };
                chan::oneshot_send(r_tx, r.clone());
                // The requester may be long gone (both autopropose taps drop the receiver), so this
                // is the only place every outcome is guaranteed to be seen.
                log_propose_result(&log, &r);
                {
                    let mut s = state.lock().await;
                    s.latest_propose_result = Some(r.clone());
                    s.curr_propose_result = None;
                }
                drop(permit);

                // Re-enqueue a follow-up propose if a request arrived while this one was running.
                // `Automatic`: the colliding caller already got `Empty` (a 400, "another propose is
                // in progress"), so this follow-up serves no caller — it is the node retrying on its
                // own, and it is paced like the taps it usually came from.
                if trigger.swap(false, Ordering::SeqCst) {
                    let (d_tx, d_rx) = oneshot::channel();
                    chan::send(&tx, (ProposeSource::Automatic, d_tx)).await;
                    // Keep the receiver alive until the re-queued propose completes it.
                    std::mem::forget(d_rx);
                }

                // A "not due" outcome on a quiet net has nobody left to ask again — see `NOT_DUE_RETRY`.
                if not_due(&r) && !retry_pending.swap(true, Ordering::SeqCst) {
                    let tx = tx.clone();
                    let retry_pending = retry_pending.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(not_due_retry).await;
                        retry_pending.store(false, Ordering::SeqCst);
                        let (d_tx, d_rx) = oneshot::channel();
                        chan::send(&tx, (ProposeSource::Automatic, d_tx)).await;
                        std::mem::forget(d_rx);
                    });
                }
                Some(r)
            }
        })
        .buffer_unordered(100)
        .filter_map(|r| async move { r })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicU64, AtomicUsize};

    use rchain_block_storage::block_store::BlockStore;
    use rchain_casper::blocks::proposer::propose_result::BlockCreatorResult;
    use rchain_casper::multi_parent_casper::ValidateError;
    use rchain_casper::validator_identity::ValidatorIdentity;
    use rchain_models::validator::Validator;

    type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

    fn block_store() -> BlockStore {
        use rchain_block_storage::dag::codecs::{BlockHashCodec, BlockMessageCodec};
        use rchain_shared::store::InMemoryKeyValueStore;
        use rchain_shared::typed_store::KeyValueTypedStoreCodec;
        Arc::new(KeyValueTypedStoreCodec::new(
            Arc::new(tokio::sync::Mutex::new(Box::new(
                InMemoryKeyValueStore::default(),
            ))),
            Arc::new(BlockHashCodec),
            Arc::new(BlockMessageCodec),
        ))
    }

    /// A proposer whose block creator always answers `outcome`, counting how often it is asked.
    fn proposer(outcome: BlockCreatorResult, calls: Arc<AtomicUsize>) -> Proposer {
        let get_seq: Arc<dyn Fn(Validator) -> BoxFuture<i64> + Send + Sync> =
            Arc::new(|_v| Box::pin(async { 0i64 }));
        let check_active: Arc<
            dyn Fn(&ValidatorIdentity) -> BoxFuture<Result<bool, String>> + Send + Sync,
        > = Arc::new(|_v| Box::pin(async { Ok(true) }));
        let create_block: Arc<
            dyn Fn(
                    &ValidatorIdentity,
                    ProposeSource,
                ) -> BoxFuture<Result<BlockCreatorResult, String>>
                + Send
                + Sync,
        > = Arc::new(move |_v, _source| {
            calls.fetch_add(1, Ordering::SeqCst);
            let outcome = outcome.clone();
            Box::pin(async move { Ok(outcome) })
        });
        let validate: Arc<
            dyn Fn(&BlockMessage) -> BoxFuture<Result<(), ValidateError>> + Send + Sync,
        > = Arc::new(|_b| Box::pin(async { Ok(()) }));
        let effect: Arc<dyn Fn(&BlockMessage) -> BoxFuture<()> + Send + Sync> =
            Arc::new(|_b| Box::pin(async {}));
        Proposer::new(
            get_seq,
            check_active,
            create_block,
            validate,
            effect,
            ValidatorIdentity::from_hex(
                "67e56582298859ddae725f972992a07c6c4fb9f62a8fff58ce3ca926a1063530",
            )
            .unwrap(),
            Arc::new(rchain_shared::log::NopLog),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
            false,
            block_store(),
        )
    }

    /// A guard whose fallback state is `Normal`: the propose gate is open.
    fn healthy() -> Arc<ResourceGuard> {
        ResourceGuard::inert(crate::resources::ResourceSnapshot {
            fallback: crate::resources::ResourceState::Normal,
            ..crate::resources::ResourceSnapshot::default()
        })
    }

    /// Send one request, drive the stream for `window`, and report how often the creator was asked.
    async fn asks_after_one_request(
        outcome: BlockCreatorResult,
        retry: Duration,
        window: Duration,
    ) -> usize {
        let calls = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = mpsc::channel(16);
        let stream = create_with_retry(
            rx,
            tx.clone(),
            proposer(outcome, calls.clone()),
            Arc::new(tokio::sync::Mutex::new(ProposerState::default())),
            healthy(),
            Arc::new(rchain_shared::log::NopLog),
            retry,
        );
        let (otx, _orx) = oneshot::channel();
        tx.send((ProposeSource::Automatic, otx)).await.unwrap();
        let mut stream = Box::pin(stream);
        let _ =
            tokio::time::timeout(window, async { while stream.next().await.is_some() {} }).await;
        calls.load(Ordering::SeqCst)
    }

    /// **The falsifier for the quiet-net strand.** A propose the round gate refuses is asked again by the
    /// node itself, with no deploy, remote block or timer to prompt it — the gate's stall escape is only
    /// evaluated on an attempt, and on a `--no-autopropose` net nothing else makes one.
    #[tokio::test]
    async fn a_refused_propose_is_retried_with_nobody_asking() {
        let retry = Duration::from_millis(40);
        let asks = asks_after_one_request(
            BlockCreatorResult::AlreadyProposedThisRound,
            retry,
            Duration::from_millis(300),
        )
        .await;
        assert!(asks >= 2, "the refusal was never retried: {asks} ask(s)");
        // One pending retry at a time: ~300/40 ≈ 7 at most, never a burst.
        assert!(
            asks <= 9,
            "the retry stormed: {asks} asks in 300 ms at a 40 ms retry"
        );
    }

    /// Only "not due" is retried. A proposer with nothing to do stays quiet.
    #[tokio::test]
    async fn nothing_to_do_is_not_retried() {
        let asks = asks_after_one_request(
            BlockCreatorResult::NoNewDeploys,
            Duration::from_millis(40),
            Duration::from_millis(300),
        )
        .await;
        assert_eq!(asks, 1);
    }

    /// The retry is set just past the gate's stall bound, so the first retry takes the escape.
    #[test]
    fn the_retry_lands_past_the_stall_escape() {
        assert!(NOT_DUE_RETRY > rchain_casper::blocks::proposer::proposer::ROUND_STALL_ESCAPE);
    }

    /// **A node that has stopped producing does not call the proposer** (unit C, B37). The request is
    /// answered with a `Failure` naming the state — not a silent `Empty` — and no "not due" retry is
    /// scheduled: the timer asks again, so recovery needs no restart.
    #[tokio::test]
    async fn a_refused_propose_is_logged_and_not_retried() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = mpsc::channel(16);
        let critical = ResourceGuard::inert(crate::resources::ResourceSnapshot {
            fallback: crate::resources::ResourceState::Critical,
            fallback_reason: "headroom below the critical floor",
            ..crate::resources::ResourceSnapshot::default()
        });
        let stream = create_with_retry(
            rx,
            tx.clone(),
            proposer(BlockCreatorResult::NoNewDeploys, calls.clone()),
            Arc::new(tokio::sync::Mutex::new(ProposerState::default())),
            critical,
            Arc::new(rchain_shared::log::NopLog),
            Duration::from_millis(50),
        );
        let (otx, orx) = oneshot::channel();
        tx.send((ProposeSource::Automatic, otx)).await.unwrap();
        let mut stream = Box::pin(stream);
        let first = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("the request is answered")
            .expect("one result");
        assert!(matches!(
            first.0.propose_status,
            ProposeStatus::ResourcePressure(ref state) if state == "critical"
        ));
        assert!(first.1.is_none(), "no block is produced under pressure");

        let refusal = orx.await.expect("the requester is answered");
        assert!(
            matches!(
                refusal,
                ProposerResult::Failure {
                    status: ProposeStatus::ResourcePressure(_),
                    ..
                }
            ),
            "the caller learns why: {refusal:?}"
        );

        // The retry window passes with the proposer still never called, and no second result.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(100), stream.next())
                .await
                .is_err(),
            "a withheld propose must not schedule the not-due retry"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
