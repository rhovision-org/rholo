//! Block proposer (port of `blocks/proposer/Proposer.scala`).
//!
//! `Proposer.apply` builds the dependency closures from the DAG/runtime; the `proposeEffect`
//! (broadcast via `CommUtil`) is supplied by the caller.

use rchain_shared::chan;
use rchain_shared::lock::Unpoison;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rchain_block_storage::block_store::BlockStore;
use rchain_block_storage::dag::dag_storage::{BlockDagStorage, DeployId};
use rchain_block_storage::dag::liveness;
use rchain_block_storage::syntax::put_block;
use rchain_crypto::private_key::PrivateKey;
use rchain_crypto::signatures::secp256k1::Secp256k1;
use rchain_crypto::signatures::signed::Signed;
use rchain_models::block::state_hash::StateHash;
use rchain_models::block_hash::BlockHash;
use rchain_models::block_metadata::SlashSeverity;
use rchain_models::casper::protocol::casper_message::{BlockMessage, DeployData, SignedDeployData};
use rchain_models::validator::Validator;
use rchain_sdk::consensus::is_super_majority;
use rchain_shared::log::{Log, LogSource};
use rchain_shared::refined::{BlockHeight, NonNegI64};

use super::block_creator::BlockCreator;
use super::propose_result::{BlockCreatorResult, ProposeResult, ProposeStatus};
use crate::interpreter_util::ShardHealth;
use crate::merging::BlockIndex;
use crate::multi_parent_casper::{get_pre_state_for_new_block, ValidateError, DEPLOY_LIFESPAN};
use crate::runtime_manager::RuntimeManager;
use crate::validator_identity::ValidatorIdentity;
use rchain_models::block_metadata::FailureCause;

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// A proposal result signaled to the caller (port of `ProposerResult`).
#[derive(Clone, Debug)]
pub enum ProposerResult {
    Empty,
    Success {
        status: ProposeStatus,
        block: BlockMessage,
    },
    Failure {
        status: ProposeStatus,
        seq_number: i64,
        message: String,
    },
    Started {
        seq_number: i64,
    },
}

/// Who asked for a proposal. This is **provenance**, not the caller's `is_async` (which only decides
/// whether an acknowledgement is sent before the work runs) — and it is what decides whether C171's
/// pace bound applies.
///
/// C171's bound is the #70 attestation storm's: a node that reacts to **every** remote block — each
/// one an attestation that is in turn a remote block for the next — attests to a net that is already
/// live. What it paces is therefore a proposal the node raised **on its own**: the autopropose tap and
/// timer, the attest-on-new-blocks tap, and the follow-up a colliding request resolves to. A caller's
/// proposal is not a reaction, and C171 must not silence it: `tools/devnet.sh` documents that "an
/// explicit `propose`/`POST /api/v1/propose`" creates a block on a node started `--no-autopropose`,
/// and `tools/devnet-test.sh` asserts `POST /api/propose` answers 200 on a node whose last deploy is
/// already in a block. Both are the contract `Explicit` preserves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProposeSource {
    /// A caller asked — admin HTTP, the gRPC propose service, the CLI, and `--propose-on-deploy`
    /// (which a deployer's request drives and which always carries that deploy, so the pace bound
    /// never applies to it anyway). Carries the caller's `is_async`: whether to acknowledge with
    /// `Started` before the work runs. This is the only source that is acknowledged, because it is
    /// the only one with a caller left to read it.
    Explicit { is_async: bool },
    /// The node asked itself: the autopropose tap and timer, the attest-on-new-blocks tap, and the
    /// follow-up a colliding request resolves to. Nobody waits for the result, so nothing is
    /// acknowledged — and this is the traffic C171 paces.
    Automatic,
}

impl ProposeSource {
    /// Whether the caller is quick to ask for a `Started` acknowledgement before the work runs.
    /// Automatic proposals have no live receiver to read one.
    fn acknowledges(&self) -> bool {
        matches!(self, ProposeSource::Explicit { is_async: true })
    }
}

/// Whether `sender` is in the bonds map the DAG reports — extracted from the
/// `check_active_validator` closure so that its failure behaviour is testable without a proposer
/// fixture (the `load_node`/`load_node_from_store` split, applied to a DAG read).
///
/// **Fallible, because the oracle is.** `Proposer.scala:240-243` takes the bonds map through
/// `BlockDagStorage[F].lookupUnsafe`, which lifts an errored *or absent* lookup into an error in
/// `F` — so the Scala never answers "not bonded" for a DAG it could not read. The port's pre-fix body
/// flattened both into an empty bonds map, and an empty map answers `false` for every sender: a node
/// whose DAG could not be read would stop proposing and report `NotBonded`, with no error anywhere
/// (AUDIT C67, found by the U14 sweep; the falsifier's witnessing form is noted in the test below).
///
/// An *absent* block under a height-map key is an inconsistency, not an empty map: the height map
/// names blocks the message map holds, which is exactly what `lookupUnsafe` refuses.
pub async fn is_active_validator(
    dag: &Arc<dyn BlockDagStorage>,
    sender: &Validator,
) -> Result<bool, String> {
    let dag_repr = dag.get_representation().await;
    // The *newest* block's view of the active set - not the fringe's bond map, and not the lowest height's,
    // which is what this read before.
    //
    // Both of those are historical. The fringe's is the set as of the last finalised block, and the lowest
    // height's is the genesis's. A validator that bonds after either was fixed is in the chain's active set
    // but absent from them, and this check then refused to let it propose - forever, because nothing
    // refreshes those maps except a fringe that cannot advance without the very validators it expects to
    // speak. That is the whole of the "only the genesis validator ever proposes" symptom: the pool grows,
    // the active set grows, and everyone who joined afterwards is stuck read-only (#70).
    //
    // A block-carried map is safe *here* where it is not safe in the finaliser: this decides only whether
    // this node proposes, which changes nobody's state. The finaliser, which decides what finalises, reads
    // the bond map from the state.
    let bonds_map = match dag_repr.height_map.iter().next_back() {
        Some((height, hashes)) => match hashes.iter().next() {
            Some(h) => {
                dag.lookup(h)
                    .await?
                    .ok_or_else(|| {
                        format!(
                            "the DAG's height map names {} at height {}, but it is not in the DAG",
                            h.to_hex(),
                            i64::from(*height)
                        )
                    })?
                    .bonds_map
            }
            None => Default::default(),
        },
        None => Default::default(),
    };
    // A zero-stake entry is not a validator with a say; `select_active` never produces one.
    Ok(bonds_map
        .get(sender)
        .map(|stake| i64::from(*stake) > 0)
        .unwrap_or(false))
}

/// The block proposer (port of `Proposer`).
pub struct Proposer {
    get_latest_seq_number: Arc<dyn Fn(Validator) -> BoxFuture<i64> + Send + Sync>,
    check_active_validator:
        Arc<dyn Fn(&ValidatorIdentity) -> BoxFuture<Result<bool, String>> + Send + Sync>,
    create_block: Arc<
        dyn Fn(&ValidatorIdentity, ProposeSource) -> BoxFuture<Result<BlockCreatorResult, String>>
            + Send
            + Sync,
    >,
    validate_block:
        Arc<dyn Fn(&BlockMessage) -> BoxFuture<Result<(), ValidateError>> + Send + Sync>,
    propose_effect: Arc<dyn Fn(&BlockMessage) -> BoxFuture<()> + Send + Sync>,
    validator: ValidatorIdentity,
    log: Arc<dyn Log>,
    consecutive_failures: Arc<AtomicU64>,
    stale_snapshot_equivocations: Arc<AtomicU64>,
    /// Broadcast a twin of every block this node creates (see `CasperConf::equivocation_injection`).
    equivocation_injection: bool,
    /// The block store, kept here for one purpose: **persisting the injected twin's body**. A peer
    /// answers a hash announcement by asking for the block, so a twin that is announced and not held is
    /// a twin nobody ever receives — which is what the first version of this arm measured (45 twins
    /// broadcast, no refusal anywhere, no slash). `validate_block` stores the real block's body and
    /// nothing validates the twin, so the twin needs its own write.
    block_store: BlockStore,
}

impl Proposer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        get_latest_seq_number: Arc<dyn Fn(Validator) -> BoxFuture<i64> + Send + Sync>,
        check_active_validator: Arc<
            dyn Fn(&ValidatorIdentity) -> BoxFuture<Result<bool, String>> + Send + Sync,
        >,
        create_block: Arc<
            dyn Fn(
                    &ValidatorIdentity,
                    ProposeSource,
                ) -> BoxFuture<Result<BlockCreatorResult, String>>
                + Send
                + Sync,
        >,
        validate_block: Arc<
            dyn Fn(&BlockMessage) -> BoxFuture<Result<(), ValidateError>> + Send + Sync,
        >,
        propose_effect: Arc<dyn Fn(&BlockMessage) -> BoxFuture<()> + Send + Sync>,
        validator: ValidatorIdentity,
        log: Arc<dyn Log>,
        consecutive_failures: Arc<AtomicU64>,
        stale_snapshot_equivocations: Arc<AtomicU64>,
        equivocation_injection: bool,
        block_store: BlockStore,
    ) -> Self {
        Proposer {
            get_latest_seq_number,
            check_active_validator,
            create_block,
            validate_block,
            propose_effect,
            validator,
            log,
            consecutive_failures,
            stale_snapshot_equivocations,
            equivocation_injection,
            block_store,
        }
    }

    async fn do_propose(
        &self,
        source: ProposeSource,
    ) -> Result<(ProposeResult, Option<BlockMessage>), String> {
        // A DAG that cannot be read is an error, not `NotBonded` (AUDIT C67): the oracle's
        // `lookupUnsafe` raises, and reporting "not bonded" would stop this node proposing on a wrong
        // reason with nothing in the log.
        match (self.check_active_validator)(&self.validator).await {
            Ok(true) => {}
            Ok(false) => {
                // The decision, logged where it is made — the same reason `close_block` logs its
                // boundary decision. `NotBonded` covers two states a validator can be in and an
                // operator cannot tell apart from the outside: *not in the pool*, and *in the pool but
                // not in the active set this epoch*. The second is new with the randomised draw
                // (`spec/RUST-VS-SCALA.md` §3 item 12) and it is the one that stops a node proposing
                // for a whole epoch with nothing else in the log: measured on a 3-validator devnet with
                // a cap of 2, where the drawn-out validator was the only one that could propose, the
                // chain halted and said nothing. The check itself reads the newest block's carried
                // bonds map, which is a state read, so every node agrees on the answer.
                eprintln!(
                    "[pos] not proposing: {} is not in the active set this node reads, \
                     which is the newest block's bond cache — either it is not bonded, or the \
                     draw left it out for this epoch",
                    rchain_shared::base16::encode(self.validator.public_key.bytes())
                );
                return Ok((
                    ProposeResult {
                        propose_status: ProposeStatus::NotBonded,
                    },
                    None,
                ));
            }
            Err(e) => return Err(format!("cannot decide whether this node is bonded: {e}")),
        }

        match (self.create_block)(&self.validator, source).await? {
            BlockCreatorResult::NoNewDeploys => Ok((
                ProposeResult {
                    propose_status: ProposeStatus::NoNewDeploys,
                },
                None,
            )),
            // Not a failure and not counted as one: the validator is not due yet, and the round closes as
            // soon as the other bonded senders have advanced — or the 15-second stall timer expires.
            // Attempts are telemetry, not an early escape licence (issue #339).
            BlockCreatorResult::AlreadyProposedThisRound => Ok((
                ProposeResult {
                    propose_status: ProposeStatus::NotEnoughNewBlocks,
                },
                None,
            )),
            BlockCreatorResult::Created(block) => match (self.validate_block)(&block).await {
                Ok(()) => {
                    self.consecutive_failures.store(0, Ordering::Relaxed);
                    (self.propose_effect)(&block).await;
                    // **The equivocation injection**, when armed: a second, equally valid block at the
                    // same `(sender, seq_num)`. Loud on every block, because a node that is doing this
                    // has been asked to and should never look healthy while it does.
                    if self.equivocation_injection {
                        match equivocation_twin(&block, &self.validator) {
                            Ok(twin) => {
                                // **The body as well as the hash.** A peer answers a hash announcement
                                // by asking for the block, so a twin that is announced and not held is
                                // a twin no peer ever receives — and the first version of this arm did
                                // exactly that: 45 twins broadcast, no refusal anywhere, no slash, and
                                // an empty ledger of refusals that looked like the fix failing.
                                // `validate_block` stores the real block's body; nothing validates the
                                // twin, so nothing else would store its.
                                if let Err(e) = put_block(&self.block_store, twin.clone()).await {
                                    self.log.error(
                                        LogSource::new("casper.blocks.Proposer"),
                                        &format!("could not store the injected twin's body: {e}"),
                                    );
                                }
                                self.log.warn(
                                    LogSource::new("casper.blocks.Proposer"),
                                    &format!(
                                        "EQUIVOCATION INJECTION: broadcasting a second block #{} \
                                         (seq {}) from this node's own key. Every peer will record \
                                         this node as having equivocated and may slash it. This is \
                                         the devnet-only `--equivocation-injection`.",
                                        twin.block_number, twin.seq_num
                                    ),
                                );
                                (self.propose_effect)(&twin).await;
                            }
                            Err(e) => self.log.error(
                                LogSource::new("casper.blocks.Proposer"),
                                &format!("could not build the injected twin: {e}"),
                            ),
                        }
                    }
                    Ok((
                        ProposeResult {
                            propose_status: ProposeStatus::ProposeSuccess,
                        },
                        Some(block),
                    ))
                }
                // **A divergence is not this node's accounting being broken** (C270, pass §92).
                //
                // `InvalidPreStateHash` — and its class — is `FailureCause::Divergence`: the same block
                // is valid for a node whose view of the parents is the one the block was built under,
                // and what changed is *this* node's view, because a fringe key it had already read was
                // recorded with the other state (the join keeps the smaller and unions the rejection
                // sets, which is what makes the map deterministic — C215). Counting that as a failure
                // halts autopropose on a block this node **built and would have accepted a moment
                // earlier**, and it was measured doing exactly that: a four-validator devnet lost three
                // of four producers at one height, and the four heads never met again
                // (`spec/audit/evidence/n-anchor-drill/self-reject-stall.txt`).
                //
                // Not counted toward the halt, and not due: the next tick re-derives the pre-state from
                // the now-current records, exactly as the stale-snapshot arm below does. The warn line
                // is the visibility — deliberately not a new gauge: this arm does not silence anything,
                // it declines to convert a divergence into a halt, and a node that keeps re-deriving
                // without succeeding says so on every tick.
                Err(ValidateError::ValidationFailed(_, status))
                    if status.failure_cause() == FailureCause::Divergence =>
                {
                    self.log.warn(
                        LogSource::new("casper.blocks.Proposer"),
                        &format!(
                            "Self-created block #{} (seq {}) no longer validates against this node's own \
                             view: {status} — a divergence about the merge rather than about the block's \
                             deploys, so it is not counted toward the autopropose halt. Re-deriving next \
                             round (C270).",
                            block.block_number, block.seq_num
                        ),
                    );
                    Ok((
                        ProposeResult {
                            propose_status: ProposeStatus::NotEnoughNewBlocks,
                        },
                        None,
                    ))
                }
                Err(ValidateError::ValidationFailed(_, status)) => {
                    // Defence in depth: a self-created block that fails its own validation means the
                    // node's state accounting is inconsistent — surface it loudly (with the exact
                    // status + block/seq) and count it so the caller can halt autopropose.
                    self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
                    self.log.error(
                        LogSource::new("casper.blocks.Proposer"),
                        &format!(
                            "Self-created block #{} (seq {}) failed validation: {status} — node state accounting is inconsistent.",
                            block.block_number, block.seq_num
                        ),
                    );
                    Err(format!(
                        "the node rejected its own block #{block} (seq {seq}): {status}. \
                         This is a node-side bug, not a problem with your request.",
                        block = block.block_number,
                        seq = block.seq_num,
                    ))
                }
                Err(ValidateError::SelfEquivocation) => {
                    // A stale-snapshot self-equivocation (§48): the DAG advanced between the
                    // parent-set read and the insert, so the node's own earlier block already holds
                    // the sequence number it derived. Not a self-validation failure — do NOT count it
                    // toward the halt — and not due: the next tick re-derives from the now-current DAG.
                    // Counted separately so a node that never recovers is still visible on `/metrics`.
                    self.stale_snapshot_equivocations
                        .fetch_add(1, Ordering::Relaxed);
                    self.log.warn(
                        LogSource::new("casper.blocks.Proposer"),
                        &format!(
                            "Self-created block #{} (seq {}) collided with this node's own \
                             already-synced block — a stale parent-set snapshot, not a validation \
                             failure. Re-deriving next round (AUDIT §48).",
                            block.block_number, block.seq_num
                        ),
                    );
                    Ok((
                        ProposeResult {
                            propose_status: ProposeStatus::NotEnoughNewBlocks,
                        },
                        None,
                    ))
                }
                Err(ValidateError::Internal(e)) => {
                    self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
                    self.log.error(
                        LogSource::new("casper.blocks.Proposer"),
                        &format!(
                            "Self-created block #{} (seq {}) failed validation with internal error: {e}",
                            block.block_number, block.seq_num
                        ),
                    );
                    Err(format!(
                        "the node rejected its own block #{block} (seq {seq}) with an internal \
                         error: {e}. This is a node-side bug, not a problem with your request.",
                        block = block.block_number,
                        seq = block.seq_num,
                    ))
                }
            },
        }
    }

    pub async fn propose(
        &self,
        source: ProposeSource,
        propose_id: tokio::sync::oneshot::Sender<ProposerResult>,
    ) -> Result<(ProposeResult, Option<BlockMessage>), String> {
        let validator = Validator::from_slice(self.validator.public_key.bytes());
        let next_seq = (self.get_latest_seq_number)(validator).await + 1;

        if source.acknowledges() {
            chan::oneshot_send(
                propose_id,
                ProposerResult::Started {
                    seq_number: next_seq,
                },
            );
            self.do_propose(source).await
        } else {
            let result = self.do_propose(source).await;
            let proposer_result = match &result {
                Ok((result, Some(block))) => ProposerResult::Success {
                    status: result.propose_status.clone(),
                    block: block.clone(),
                },
                Ok((result, None)) => ProposerResult::Failure {
                    status: result.propose_status.clone(),
                    seq_number: next_seq,
                    message: result.propose_status.to_string(),
                },
                Err(e) => ProposerResult::Failure {
                    status: ProposeStatus::BugError(e.clone()),
                    seq_number: next_seq,
                    message: e.clone(),
                },
            };
            chan::oneshot_send(propose_id, proposer_result);
            result
        }
    }

    /// Build a `Proposer` from its dependencies (port of `Proposer.apply`). The DAG/block-store/
    /// runtime/block-index are captured by `Arc` so the returned proposer is `'static` and can be
    /// driven from a spawned task.
    #[allow(clippy::too_many_arguments)]
    pub fn apply<F, Fut>(
        validator_identity: ValidatorIdentity,
        shard_id: String,
        min_phlo_price: i64,
        max_number_of_parents: i32,
        epoch_length: i32,
        dummy_deploy_opt: Option<(PrivateKey, String)>,
        dag: Arc<dyn BlockDagStorage>,
        block_store: BlockStore,
        runtime: Arc<RuntimeManager>,
        block_index: F,
        propose_effect: Arc<dyn Fn(&BlockMessage) -> BoxFuture<()> + Send + Sync>,
        log: Arc<dyn Log>,
        consecutive_failures: Arc<AtomicU64>,
        stale_snapshot_equivocations: Arc<AtomicU64>,
        equivocation_injection: bool,
        // **This shard's finality-and-merge health** (W-N10-02/03), held like the two counters above:
        // the proposer drives `validate` on its own blocks, so it is one of the writers.
        health: ShardHealth,
    ) -> Proposer
    where
        F: Fn(BlockHash) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Arc<BlockIndex>, String>> + Send + 'static,
    {
        let block_index = Arc::new(block_index);

        let get_latest_seq_number: Arc<dyn Fn(Validator) -> BoxFuture<i64> + Send + Sync> = {
            let dag = dag.clone();
            Arc::new(move |sender| {
                let dag = dag.clone();
                Box::pin(async move {
                    let dag_repr = dag.get_representation().await;
                    dag_repr
                        .dag_message_state
                        .latest_msgs
                        .get(&sender)
                        .map(|m| i64::from(m.sender_seq))
                        .unwrap_or(-1)
                })
            })
        };

        let check_active_validator: Arc<
            dyn Fn(&ValidatorIdentity) -> BoxFuture<Result<bool, String>> + Send + Sync,
        > = {
            let dag = dag.clone();
            Arc::new(move |vi: &ValidatorIdentity| {
                let sender = Validator::from_slice(vi.public_key.bytes());
                let dag = dag.clone();
                Box::pin(async move { is_active_validator(&dag, &sender).await })
            })
        };

        let create_block: Arc<
            dyn Fn(
                    &ValidatorIdentity,
                    ProposeSource,
                ) -> BoxFuture<Result<BlockCreatorResult, String>>
                + Send
                + Sync,
        > = {
            let runtime = runtime.clone();
            let dag = dag.clone();
            let block_store = block_store.clone();
            let block_index = block_index.clone();
            let shard_id = shard_id.clone();
            let log = log.clone();
            let dummy_deploy_opt = dummy_deploy_opt.clone();
            // **How many proposals this validator has declined waiting for the round to close.** This
            // counter is telemetry only; the stall clock below is the sole escape licence (#339).
            let health = health.clone();
            let blocked_since_advance: Arc<std::sync::atomic::AtomicI64> =
                Arc::new(std::sync::atomic::AtomicI64::new(0));
            // The first blocked observation is process-local monotonic time. None means the
            // round is healthy; restarts begin a fresh observation, never import a timer.
            let blocked_since: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
            Arc::new(move |vi: &ValidatorIdentity, source: ProposeSource| {
                let runtime = runtime.clone();
                let dag = dag.clone();
                let block_store = block_store.clone();
                let block_index = block_index.clone();
                let vi = vi.clone();
                let shard_id = shard_id.clone();
                let blocked_since_advance = blocked_since_advance.clone();
                let blocked_since = blocked_since.clone();
                let dummy_deploy_opt = dummy_deploy_opt.clone();
                let log = log.clone();
                let health = health.clone();
                let max_number_of_parents = max_number_of_parents;
                Box::pin(async move {
                    create_block(
                        runtime.as_ref(),
                        dag.as_ref(),
                        &block_store,
                        block_index.as_ref(),
                        &vi,
                        &shard_id,
                        min_phlo_price,
                        &log,
                        max_number_of_parents,
                        epoch_length,
                        dummy_deploy_opt.as_ref(),
                        &blocked_since_advance,
                        &blocked_since,
                        source,
                        &health,
                    )
                    .await
                })
            })
        };

        let validate_block: Arc<
            dyn Fn(&BlockMessage) -> BoxFuture<Result<(), ValidateError>> + Send + Sync,
        > = {
            let runtime = runtime.clone();
            let dag = dag.clone();
            let block_store = block_store.clone();
            let block_index = block_index.clone();
            let shard_id = shard_id.clone();
            let log = log.clone();
            let health = health.clone();
            Arc::new(move |block: &BlockMessage| {
                let runtime = runtime.clone();
                let dag = dag.clone();
                let block_store = block_store.clone();
                let block_index = block_index.clone();
                let block = block.clone();
                let shard_id = shard_id.clone();
                let log = log.clone();
                let health = health.clone();
                let max_number_of_parents = max_number_of_parents;
                Box::pin(async move {
                    match crate::multi_parent_casper::validate(
                        dag.as_ref(),
                        &block_store,
                        runtime.as_ref(),
                        &block,
                        &shard_id,
                        min_phlo_price,
                        max_number_of_parents,
                        block_index.as_ref(),
                        &log,
                        &health,
                    )
                    .await
                    {
                        Ok(meta) => {
                            let accepted_post = block.post_state_hash;
                            let accepted_height = meta.block_num;
                            // Persist the block body BEFORE recording it in the DAG. `insert`
                            // writes the deploy index + DAG state but not the full block, so a
                            // read landing between the two would otherwise hit "missing block".
                            put_block(&block_store, block.clone()).await.map_err(|e| {
                                ValidateError::Internal(format!("failed to store block body: {e}"))
                            })?;
                            dag.insert(meta, block.clone()).await.map_err(|e| {
                                // A stale-snapshot self-equivocation (§48): the DAG advanced between
                                // the parent-set read and this insert, so the node's own earlier block
                                // already holds the sequence number it derived. Distinguish it from a
                                // genuine insert failure so `do_propose` does not count it toward the
                                // halt — it is "not due", not "invalid".
                                if e.contains(crate::dag::EQUIVOCATION_PREFIX) {
                                    ValidateError::SelfEquivocation
                                } else {
                                    ValidateError::Internal(format!(
                                        "failed to insert block into DAG: {e}"
                                    ))
                                }
                            })?;
                            // **The block is accepted: publish its state** (law 80, B40). The
                            // checkpoint recorded the root as known; this moves `CURRENT_ROOT`.
                            crate::blocks::block_processor::publish_accepted_root(
                                dag.as_ref(),
                                runtime.as_ref(),
                                accepted_height,
                                accepted_post,
                                &log,
                            )
                            .await;
                            Ok(())
                        }
                        Err(err) => Err(err),
                    }
                })
            })
        };

        Proposer::new(
            get_latest_seq_number,
            check_active_validator,
            create_block,
            validate_block,
            propose_effect,
            validator_identity,
            log,
            consecutive_failures,
            stale_snapshot_equivocations,
            equivocation_injection,
            block_store,
        )
    }
}

async fn get_block(
    block_store: &BlockStore,
    hash: &BlockHash,
) -> Result<Option<BlockMessage>, String> {
    let mut vals = block_store.get(&[*hash]).await?;
    Ok(vals.pop().flatten())
}

/// The most deploys one block may carry, before the slashes it also has to seed (AUDIT C123).
///
/// **The bound is the per-deploy randomness seed, which is indexed in a `u8`.** `BlockCreator::create`
/// derives each deploy's seed with `rand.split_byte(u8::try_from(selected.len() + i)?)` and the
/// `close_block` seed with `split_byte(u8::try_from(selected.len() + to_slash.len())?)`
/// (`block_creator.rs`), so a block's deploy count **plus** its slash count must fit in 255. Nothing
/// bounded the selection, and the pool's own cap is 10,000 — set for a different reason entirely — so a
/// pool of 256 valid deploys made `create` fail *before any state work*: the node could not produce a
/// block at all, on any shard, until the pool drained below 256.
///
/// **The cap is on the selection and not on the seed**: widening the index would hide the bound rather
/// than respect it, and the deploys a block cannot carry are not lost — the pool keeps them and the next
/// block takes them. The slash count needs no cap of its own: `to_slash` comes from the block's
/// justifications, which are bounded by `max-number-of-parents`, so it is small by construction.
pub const MAX_BLOCK_DEPLOYS: usize = 255; // `u8::MAX` — the width of the seed index

/// The deploys one block may carry, given how many validators it also has to seed — see
/// [`MAX_BLOCK_DEPLOYS`] for why the *sum* is what has to fit.
fn per_block_deploy_budget(slashes: usize) -> usize {
    MAX_BLOCK_DEPLOYS.saturating_sub(slashes)
}

/// The deploys this block will carry: the pool's valid entries in their canonical order, truncated to
/// `budget` (AUDIT C123).
///
/// "Valid" is the Scala proposer's filter — not future, not expired, not already in the DAG (the replay
/// guard). The order is canonical because the pool is a `BTreeMap` keyed by `DeployId`, so the prefix the
/// budget takes is the same for every node holding the same pool. The loop stops *reading* at the budget
/// as well as collecting, so an oversized pool costs no more DAG lookups than the block can carry.
async fn select_deploys(
    dag: &dyn BlockDagStorage,
    next_block_num: BlockHeight,
    budget: usize,
) -> Result<Vec<DeployId>, String> {
    let pooled = dag.pooled_deploys().await?;
    let mut deploys: Vec<DeployId> = Vec::new();
    for (id, d) in pooled {
        let in_the_dag = dag.lookup_by_deploy_id(&id).await?.is_some();
        if not_selectable(
            d.data.valid_after_block_number,
            i64::from(next_block_num),
            in_the_dag,
        )
        .is_none()
        {
            deploys.push(id);
            if deploys.len() == budget {
                break;
            }
        }
    }
    Ok(deploys)
}

/// **Why a pooled deploy cannot go into a block at `next_block_num`** — the Scala proposer's filter
/// (`select_deploys`'s own doc: not future, not expired, not already in the DAG), in **one** place so
/// that the filter and the report of it cannot disagree.
///
/// Extracted for issue #356, where a deploy the node could never include was silent end to end:
/// accepted by the API, held by the pool, dropped here, and no line anywhere saying so. A report that
/// counted with its own copy of the arithmetic would be a second implementation of the filter, and this
/// codebase's own warning is that two implementations of one rule disagree (see the note on `NoAdvance`
/// in `block-storage/src/dag/finalizer.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotSelectable {
    /// `valid_after > next_block_num`: not yet — and it will be, as the chain advances.
    Future,
    /// `valid_after <= next_block_num - DEPLOY_LIFESPAN`: never again, whatever happens next.
    Expired,
    /// Already in the DAG: including it again would be a replay.
    Replay,
}

fn not_selectable(
    valid_after: i64,
    next_block_num: i64,
    already_in_the_dag: bool,
) -> Option<NotSelectable> {
    if crate::validate::deploy_is_future_at_block(valid_after, next_block_num) {
        Some(NotSelectable::Future)
    } else if crate::validate::deploy_expired_at_block(valid_after, next_block_num, DEPLOY_LIFESPAN)
    {
        Some(NotSelectable::Expired)
    } else if already_in_the_dag {
        Some(NotSelectable::Replay)
    } else {
        None
    }
}

/// **What a block left out, when it had nothing to include** (issue #356).
///
/// Called only on that path — a selection that came back empty — so it costs one pool read and one DAG
/// lookup per pooled deploy, and only where the alternative is silence. This is the line that would
/// have told the playground's operator, in words, why an accepted deploy was never proposed; without
/// it the node answers `Success!`, produces nothing, and says nothing, which is the report.
///
/// A pool that is empty is not reported: that is an attestation round, which is the ordinary case and
/// the one the guard is about (`attestation_suppressed`).
async fn report_nothing_to_include(
    dag: &dyn BlockDagStorage,
    next_block_num: BlockHeight,
    log: &Arc<dyn Log>,
) {
    let Ok(pooled) = dag.pooled_deploys().await else {
        return;
    };
    if pooled.is_empty() {
        return;
    }
    let (mut expired, mut future, mut replay) = (0usize, 0usize, 0usize);
    let mut lookup_failed = false;
    for (id, d) in pooled {
        match dag.lookup_by_deploy_id(&id).await {
            Ok(seen) => match not_selectable(
                d.data.valid_after_block_number,
                i64::from(next_block_num),
                seen.is_some(),
            ) {
                Some(NotSelectable::Expired) => expired += 1,
                Some(NotSelectable::Future) => future += 1,
                Some(NotSelectable::Replay) => replay += 1,
                // Selectable and not selected: only the per-block budget can leave one of these behind,
                // and that is the ordinary "the next block takes it" case rather than a finding.
                None => {}
            },
            Err(_) => lookup_failed = true,
        }
    }
    let total = expired + future + replay;
    if total == 0 {
        return;
    }
    log.warn(
        LogSource::new("casper.blocks.Proposer"),
        &format!(
            "nothing to include in block {next_block_num}: the pool holds {total} deploy(s) that cannot \
             be selected — {expired} expired (anchored at or below {next_block_num} minus the \
             {DEPLOY_LIFESPAN}-block lifespan, so never again), {future} not valid yet, {replay} already \
             in the DAG{}",
            if lookup_failed {
                " (and a deploy lookup failed, so these counts are a lower bound)"
            } else {
                ""
            }
        ),
    );
}

/// **How long a validator may be blocked by the round gate before it takes the escape.**
///
/// A round may remain blocked when a bonded sender is silent (#213). The round's DAG-derived clock
/// is frozen in that state, so it cannot release the gate. The proposer records the first blocked
/// monotonic sighting and the existing one-shot automatic retry supplies an attempt after this
/// bound, including with `--no-autopropose` (C210).
///
/// In the N=8 acceptance trace (#339), six attempts arrived in 316–3,388 ms and escaped the
/// round early, bypassing the shared parent snapshot before the intended 15-second stall. Only
/// elapsed blocked time may authorise escape; the attempt count remains diagnostic telemetry.
pub const ROUND_STALL_ESCAPE: Duration = Duration::from_secs(15);

/// Elapsed time since this process first observed the blocked round. A host wall-clock
/// correction cannot advance or delay this timer. The first sighting always returns zero;
/// a clock value before the stored sighting also returns zero (defensive test case).
fn stall_ms_since(first_sighting: &Mutex<Option<Instant>>, now: Instant) -> i64 {
    let mut since = first_sighting.lock().unpoison();
    match *since {
        None => {
            *since = Some(now);
            0
        }
        Some(start) => now
            .checked_duration_since(start)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64,
    }
}

/// Whether the round gate may be escaped: only an elapsed stall reaches the bound.
/// Attempt counts are diagnostic telemetry, intentionally absent from this decision (#339).
fn round_escape_owed(stalled_ms: i64) -> bool {
    stalled_ms >= ROUND_STALL_ESCAPE.as_millis() as i64
}

#[allow(clippy::too_many_arguments)]
async fn create_block<'a, F, Fut>(
    runtime: &'a RuntimeManager,
    dag: &'a dyn BlockDagStorage,
    block_store: &'a BlockStore,
    block_index: &'a F,
    validator_identity: &ValidatorIdentity,
    shard_id: &str,
    min_phlo_price: i64,
    log: &'a Arc<dyn Log>,
    max_number_of_parents: i32,
    epoch_length: i32,
    dummy_deploy_opt: Option<&(PrivateKey, String)>,
    blocked_since_advance: &std::sync::atomic::AtomicI64,
    blocked_since: &Mutex<Option<Instant>>,
    source: ProposeSource,
    health: &ShardHealth,
) -> Result<BlockCreatorResult, String>
where
    F: Fn(BlockHash) -> Fut + Sync,
    Fut: Future<Output = Result<Arc<BlockIndex>, String>>,
{
    let creators_validator_for_parents =
        Validator::from_slice(validator_identity.public_key.bytes());

    // **C190's repair, before anything is derived from the parent set.** A node holding a failure record of
    // its *own* block at a sequence number its arithmetic cannot see (H-2 keeps the record out of
    // `latest_msgs`) will build a block reusing that number, and the H-1 gate will refuse it — three times,
    // and the autopropose timer halts. Nothing else clears such a record: the restoring rule fires on an
    // *incoming* block's justifications and no inbound block can justify the node's own failed record.
    //
    // The result is deliberately ignored. If the clear succeeded, the parent set read below simply sees the
    // block and the arithmetic advances; if it did not, the refusal that follows is the *counted* one
    // (`consecutive_failures`), so the halt stays visible on `/api/status` and `/metrics` rather than
    // becoming a silent decline.
    let _ = crate::multi_parent_casper::clear_own_failure_record(
        dag,
        block_store,
        runtime,
        shard_id,
        min_phlo_price,
        max_number_of_parents,
        block_index,
        &creators_validator_for_parents,
        log,
        health,
    )
    .await;
    // **One block per validator per round, with a way out.** Every validator enforces the rule the numbers
    // imply: `validate.rs:236` requires `max(justifications) + 1 == block_number` and `:259` requires
    // `creator_latest_seq + 1 == seq_num`, so a second proposal against the same snapshot is an
    // equivocation. Refusing for ever deadlocks a round whose quiet sender cannot age out — the round's
    // clock is measured from a tip the refusal freezes — so the wait is bounded by
    // [`ROUND_STALL_ESCAPE`] only. A retry scheduled past that bound supplies the next attempt
    // even without autopropose; frequent attempts must not escape the round early (#339).
    let escape = {
        let dag_repr = dag.get_representation().await;
        if dag_repr
            .dag_message_state
            .has_advanced_past_the_round(&creators_validator_for_parents)
        {
            let waited =
                blocked_since_advance.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            let stalled_ms = stall_ms_since(blocked_since, Instant::now());
            if !round_escape_owed(stalled_ms) {
                return Ok(BlockCreatorResult::AlreadyProposedThisRound);
            }
            log.warn(
                LogSource::new("casper.blocks.Proposer"),
                &format!(
                    "round gate escaped after {waited} attempt(s) and {stalled_ms} ms blocked — the round is \
                     not closing (AUDIT #213)."
                ),
            );
            true
        } else {
            blocked_since_advance.store(0, std::sync::atomic::Ordering::Relaxed);
            *blocked_since.lock().unpoison() = None;
            false
        }
    };
    let pre_state = get_pre_state_for_new_block(
        dag,
        block_store,
        runtime,
        block_index,
        &creators_validator_for_parents,
        escape,
    )
    .await?;
    let pre_state_hash = pre_state.pre_state_hash;
    let creators_validator = Validator::from_slice(validator_identity.public_key.bytes());
    let next_block_num = pre_state
        .justifications
        .iter()
        .map(|m| m.block_num)
        .max()
        .map(|m| m + NonNegI64::one())
        .unwrap_or_else(BlockHeight::zero);
    let parent_hashes: Vec<BlockHash> = pre_state
        .justifications
        .iter()
        .map(|m| m.block_hash)
        .collect();

    // **The proposer's half of #153's bound.** A justification set wider than the protocol allows is
    // refused by every validator (`validate::justification_count`), so emitting one would be a block
    // this node cannot get accepted — it would burn its own round and, worse, look like a validation
    // failure to everyone else. Refusing here is the proposer respecting the same bound the receivers
    // enforce, which is what `MAX_BLOCK_DEPLOYS` does for the deploy count on the other side of the same
    // arithmetic.
    //
    // It should not be reachable on a sane chain: the set is `round_parents`, one message per sender, so
    // it takes more than `max_number_of_parents` distinct senders to get here. That is exactly why the
    // refusal is loud — reaching it means the DAG holds more senders than the network is configured for,
    // which is a fact the operator needs rather than a proposal to quietly skip.
    if max_number_of_parents > 0 && parent_hashes.len() > max_number_of_parents as usize {
        return Err(format!(
            "the round's parent set holds {} justifications, above this network's \
             max-number-of-parents of {} — every validator would refuse a block built on it, so this \
             node is not proposing one",
            parent_hashes.len(),
            max_number_of_parents
        ));
    }

    let pre_state_bonds = runtime
        .compute_bonds(&StateHash::from_slice(pre_state_hash.as_bytes()))
        .await?;
    let bonded: BTreeSet<Validator> = pre_state_bonds
        .iter()
        .filter(|(_, b)| i64::from(**b) > 0)
        .map(|(v, _)| *v)
        .collect();
    let mut to_slash: BTreeMap<Validator, ProposedSlash> =
        slashable_offenders(&pre_state.justifications, &bonded);
    // **And the equivocations this node has itself seen** (AUDIT C200).
    //
    // **The instrument is AUDIT C201's, and it is here because the row asked for a measurement rather
    // than a fixture.** The live A2 arm showed this fold taking a `Slash` for an offender the *block's
    // own* bonds map had already dropped, on every block — 59 of them. The merge is exonerated (C201's
    // fixture passes: a slashing branch's native write does reach a merged root), so what is left is
    // **which hash this fold reads**, and that is what these two lines report: the pre-state it asked,
    // how many validators that answer held, and — for every equivocation this node has recorded —
    // whether the `bonded` filter admitted it. A run that takes the slash while `admitted` names the
    // offender is the bug; one that never admits it means the repeated slash has another cause.
    //
    // Logged only when there is something recorded, so a chain that has never seen an equivocation pays
    // nothing for it.
    let recorded_equivocations = dag.recorded_equivocations().await;
    if !recorded_equivocations.is_empty() {
        let admitted: Vec<String> = recorded_equivocations
            .iter()
            .filter(|(v, _)| bonded.contains(v))
            .map(|(v, _)| rchain_shared::base16::encode(v.as_bytes()))
            .collect();
        eprintln!(
            "[pos] c201: pre_state={} bonded={} recorded={} admitted={:?} justifications={} \
             fringe={} rejected={} parents={:?}",
            pre_state_hash.to_hex(),
            bonded.len(),
            recorded_equivocations.len(),
            admitted,
            // **The other half of the question**: if the pre-state is constant while the chain
            // advances, the interesting fact is what the *parents* were. Taken from the same
            // `ParentsMergedState` the pre-state came from, so the two cannot disagree about which
            // merge produced which.
            pre_state.justifications.len(),
            // **And the two things that could pin it.** `fringe` is eight bytes of the fringe the merge
            // used: if it never moves, the base half is pinned. `rejected` is how many deploy ids the
            // merge *refused* — a non-zero value here means the conflict scope's work was thrown away,
            // which is the only remaining way a growing conflict scope contributes nothing.
            &rchain_shared::base16::encode(pre_state.fringe_state.as_bytes())[..8],
            pre_state.rejected_deploys.len(),
            pre_state
                .justifications
                .iter()
                .map(|m| format!(
                    "{}@{}",
                    &rchain_shared::base16::encode(m.block_hash.as_bytes())[..8],
                    m.block_num
                ))
                .collect::<Vec<_>>()
        );
    }
    add_recorded_equivocations(&mut to_slash, recorded_equivocations, &bonded);
    if !to_slash.is_empty() {
        // The consequence, logged where it is decided. The validation failure that caused it is already
        // logged by the block processor; nothing connected the two, so a slashing used to be visible only as
        // the pool getting smaller later on (`getBonds` counting one fewer validator). The tier is logged
        // with it, because it decides how much of the bond leaves — and an operator reading a slash wants
        // to know both.
        eprintln!(
            "[pos] slashing {} bonded validator(s) whose block failed validation here (tier and share): {}",
            to_slash.len(),
            to_slash
                .iter()
                .map(|(v, slash)| format!(
                    "{} {:?}/{}bps{}",
                    rchain_shared::base16::encode(v.as_bytes())
                        .chars()
                        .take(8)
                        .collect::<String>(),
                    slash.severity,
                    slash.severity.basis_points(),
                    if slash.evidence.is_some() {
                        " (equivocation evidence)"
                    } else {
                        ""
                    }
                ))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }

    // An epoch boundary is a block whose height is a positive multiple of `epoch_length` (matching
    // the PoS contract's `blockNumber % epochLength == 0`). A non-positive `epoch_length` disables
    // the epoch trigger (the native PoS lifecycle is otherwise block-driven).
    let change_epoch = epoch_length > 0
        && i64::from(next_block_num) != 0
        && i64::from(next_block_num) % i64::from(epoch_length) == 0;

    // Attestation suppression: no new state transitions, or not yet a super-majority.
    let dag_repr = dag.get_representation().await;
    // The ids are what this needs (the two consumers below are a union and a difference), so it
    // yields them directly rather than cloning the message that holds them — a `Message` clone used
    // to drag its whole ancestor set along (AUDIT C56).
    let seen = |h: &BlockHash| -> Vec<BlockHash> {
        dag_repr
            .dag_message_state
            .msg_map
            .get(h)
            .map(|m| m.seen.iter().copied().collect())
            .unwrap_or_default()
    };
    // **What is left to finalise is read from everything this node has seen, not from the round's
    // parents** (AUDIT C209). The parents are the round snapshot, so a deploy-bearing block another
    // validator made this round is in `latest_msgs` and not in any parent: read from the parents, every
    // other validator saw "nothing to finalise" and stayed silent, the round never closed, and the deploy
    // was never finalised. That is the live report on #214: deploys sent to one node never finalise, and
    // sending them in rotation does.
    //
    // **The fringe is the one the parents carry, not the one this block would** (#223). `pre_state.fringe`
    // is the fringe this block *would* publish; counting a deploy as finalised against it withheld exactly
    // the block that would have published it, so the round came to rest one fringe short and the last
    // deploy was never finalised: live, a deploy at height 34 with the chain quiet at 37 and the
    // published fringe at 32, every validator reading `nothing_to_finalize` against a fringe no block
    // carried. Against the parents' fringe the round that computes the covering fringe still proposes and
    // publishes it, and the next round reads it and goes quiet — one round more, never unbounded.
    let fringe_seen: BTreeSet<BlockHash> =
        pre_state.prev_fringe.iter().flat_map(|h| seen(h)).collect();
    let mut conflict_set: Vec<(BlockHeight, BlockHash)> = dag_repr
        .dag_message_state
        .latest_msgs
        .values()
        .flat_map(|m| m.seen.iter().copied())
        .collect::<BTreeSet<BlockHash>>()
        .difference(&fringe_seen)
        .filter_map(|h| {
            dag_repr
                .dag_message_state
                .msg_map
                .get(h)
                .map(|m| (m.height, *h))
        })
        .collect();
    // Oldest first, so the scan stops at the oldest deploy-bearing block, which is the one whose age
    // decides the attestation horizon below.
    conflict_set.sort();

    let has_deploys =
        |b: &BlockMessage| !b.state.system_deploys.is_empty() || !b.state.deploys.is_empty();

    let mut oldest_unfinalized: Option<BlockHeight> = None;
    for (height, h) in &conflict_set {
        if let Some(b) = get_block(block_store, h).await? {
            if has_deploys(&b) {
                oldest_unfinalized = Some(*height);
                break;
            }
        }
    }

    // Attestation timing (the port's `newlySeen`/`waitingForSupermajority` guard, corrected).
    //
    // The port derived "newly seen" as `seen(our own latest message's justifications) \ seen(parents)`.
    // Our own message is one of the parents and everything it justifies is an ancestor of the parents, so
    // that difference is empty by construction: `new_state_transition` was always false and
    // `attestation_stake` always zero, hence `waiting_for_supermajority` always true. The empty-attestation
    // branch in `block_creator.rs` was therefore unreachable, and the dev-mode dummy deploy was the only
    // thing that ever got past this guard — which is why `--autopropose` could not produce a block on its
    // own. See #70.
    //
    // What the guard needs is (a) whether an unfinalized state transition exists to attest to — the scan
    // above, over what this node has seen — and (b) how much stake is moving. (b) is the senders of the
    // latest messages this node has seen, excluding ourselves: our own message is the attestation we are
    // about to add, and it is counted on the other side of the comparison.
    //
    // **Every input is read from the same view, the node's seen view, and not from the parents** (C209,
    // second half). The parents are the round snapshot, and before the first round closes that snapshot
    // is the genesis alone — a message from the genesis signer, at the tip. Read from it, the signer has
    // "just spoken" and nobody else is moving, so its cadence is never due and its quorum never reachable,
    // and it never speaks again: measured live on #219 with every deploy sent to another validator (the
    // signer's block count stayed at 1). The parents decide what the block is built on; they do not
    // decide whether this node should speak.
    //
    // The guard's inputs are named functions rather than inline sums, so that its behaviour is
    // falsifiable without a DAG: `moving_attestation_stake` (who counts as moving),
    // `attestation_suppressed` (the decision) and `cadence_due` (the pace bound). `tip` is the newest
    // height this node can see, which is the datum the recency checks need.
    let seen_heights: BTreeMap<Validator, BlockHeight> = liveness::latest_heights(
        dag_repr
            .dag_message_state
            .latest_msgs
            .values()
            .map(|m| (m.sender.clone(), m.height)),
    );
    let tip = seen_heights
        .values()
        .copied()
        .max()
        .unwrap_or_else(BlockHeight::zero);
    // (a), as the two inputs the decision takes — see `attestation_inputs` for why the second is now
    // bounded by age rather than read from the parents.
    // The fringe this block extends, as a height: the work's age is counted from here when the work lies
    // below it (#223) — see `work_age_floor`.
    let fringe_top = pre_state
        .prev_fringe
        .iter()
        .filter_map(|h| dag_repr.dag_message_state.msg_map.get(h).map(|m| m.height))
        .max();
    let (nothing_to_finalize, new_state_transition) =
        attestation_inputs(work_age_floor(oldest_unfinalized, fringe_top), tip);
    // The live weight set: the one liveness rule, shared with the finalizer through `liveness` (#70
    // increment 2). It decides who counts as *moving*; the quorum itself stays measured against the
    // whole bonded map below, so a minority cannot attest its way to a supermajority.
    let live_bonds = liveness::live_weight_set(
        &pre_state_bonds,
        &seen_heights,
        liveness::quorum_supported_height(&pre_state_bonds, &seen_heights),
        liveness::LIVENESS_WINDOW,
    );
    let pre_state_bonds_stake: i128 = pre_state_bonds
        .values()
        .map(|s| i128::from(i64::from(*s)))
        .sum();
    let own_stake: i128 = pre_state_bonds
        .iter()
        .filter(|(v, _)| **v == creators_validator)
        .map(|(_, s)| i128::from(i64::from(*s)))
        .sum();
    let attestation_stake = moving_attestation_stake(&live_bonds, &creators_validator);
    let quorum_reachable =
        attestation_reaches_supermajority(attestation_stake, own_stake, pre_state_bonds_stake);

    let cadence = cadence_due(&seen_heights, &creators_validator, tip);
    // Only a proposal the node raised on its own is paced (C171): the storm is a node reacting to remote
    // blocks, and a caller's explicit propose is not a reaction.
    let paced = source == ProposeSource::Automatic;
    let suppress_attestation = attestation_suppressed(
        nothing_to_finalize,
        new_state_transition,
        quorum_reachable,
        cadence,
        paced,
    );
    // **Why this node did or did not attest, in its own log** (#213's diagnostic). Without it a validator
    // that stays silent looks exactly like one with nothing to do: the R3 arm on #219 showed the genesis
    // signer making no block after genesis, and no run could say which input held it.
    let decision = describe_attestation_decision(&AttestationDecision {
        nothing_to_finalize,
        new_state_transition,
        quorum_reachable,
        cadence_due: cadence,
        paced,
        round_senders: pre_state.justifications.iter().map(|m| &m.sender).collect(),
        blocked_attempts: blocked_since_advance.load(std::sync::atomic::Ordering::Relaxed),
        escape,
        suppressed: suppress_attestation,
    });
    if attestation_line_due(&decision, i64::from(tip)) {
        log.info(
            LogSource::new("casper.blocks.Proposer"),
            &format!("attestation at tip {}: {decision}", i64::from(tip)),
        );
    }

    // User deploys: filter future / expired / replayed, then cap at what the block can seed — the pool
    // may hold far more than one block can carry, and the leftover stays pooled (AUDIT C123).
    let mut deploys =
        select_deploys(dag, next_block_num, per_block_deploy_budget(to_slash.len())).await?;
    // **A block with nothing to include says why** (issue #356). Reported here rather than at the
    // callers because this is where the reasons are known, and only on this path.
    if deploys.is_empty() {
        report_nothing_to_include(dag, next_block_num, log).await;
    }

    // Dev-mode dummy deploy: when there is nothing pooled to include, inject a signed `Nil` deploy so
    // `--autopropose` can keep producing blocks (port of Scala `Proposer.dummyDeployOpt`).
    if deploys.is_empty() {
        if let Some((key, term)) = dummy_deploy_opt {
            eprintln!(
                "No pooled deploys; injecting dummy deploy for block #{}",
                i64::from(next_block_num)
            );
            let data = DeployData {
                attachments: Vec::new(),
                term: term.clone(),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0),
                phlo_price: 1,
                phlo_limit: 90000,
                valid_after_block_number: i64::from(next_block_num) - 1,
                shard_id: shard_id.to_string(),
            };
            let signed_deploy = {
                // `Signed` holds a `&'static dyn SignaturesAlg` (not `Sync`); drop it before the
                // `.await` below so the future stays `Send`.
                let signed = Signed::new(data, &Secp256k1, key).map_err(|e| e.to_string())?;
                SignedDeployData {
                    data: signed.data,
                    deployer: signed.pk.bytes().to_vec(),
                    sig: signed.sig,
                    sig_algorithm: signed.sig_algorithm.name().to_string(),
                }
            };
            let id = crate::multi_parent_casper::add_deploy(dag, &signed_deploy).await?;
            deploys.push(id);
        }
    }

    BlockCreator {
        id: validator_identity.clone(),
        shard_id: shard_id.to_string(),
    }
    .create(
        runtime,
        dag,
        &pre_state,
        &deploys,
        &to_slash,
        change_epoch,
        suppress_attestation,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The clock begins at the first blocked observation, and resetting the round
    /// discards the observation. Wall time is deliberately absent from this API.
    #[test]
    fn a_stall_uses_monotonic_elapsed_time_and_resets() {
        let since = Mutex::new(None);
        let first = Instant::now();
        assert_eq!(stall_ms_since(&since, first), 0);
        assert_eq!(
            stall_ms_since(&since, first + Duration::from_millis(14_999)),
            14_999
        );
        assert!(!round_escape_owed(stall_ms_since(
            &since,
            first + Duration::from_millis(14_999)
        )));
        assert_eq!(
            stall_ms_since(&since, first + Duration::from_millis(15_000)),
            15_000
        );
        assert!(round_escape_owed(stall_ms_since(
            &since,
            first + Duration::from_millis(15_000)
        )));
        *since.lock().unpoison() = None;
        assert_eq!(stall_ms_since(&since, first + Duration::from_secs(60)), 0);
    }

    /// A host-clock correction must not appear in the elapsed calculation. This
    /// regression would fail the old wall-clock implementation at a 60-second step.
    #[test]
    fn wall_clock_steps_cannot_license_or_delay_round_escape() {
        let since = Mutex::new(None);
        let first = Instant::now();
        let _ = stall_ms_since(&since, first);
        // The former SystemTime subtraction would license escape after a forward
        // step, and would delay it after a backward step. Neither offset is read.
        assert!(round_escape_owed(60_000));
        assert!(!round_escape_owed(stall_ms_since(
            &since,
            first + Duration::from_secs(1)
        )));
        assert!(!round_escape_owed(-60_000));
        assert!(round_escape_owed(stall_ms_since(
            &since,
            first + ROUND_STALL_ESCAPE
        )));
        assert_eq!(stall_ms_since(&since, first - Duration::from_secs(1)), 0);
    }

    /// **A burst of attempts is not a licence to escape before the stall bound.**
    ///
    /// The N=8 trace observed attempts six and beyond in under four seconds. The timed retry
    /// already supplies another proposal after 15 seconds on a quiet network (C210).
    #[test]
    fn the_round_escape_requires_elapsed_stall_even_after_many_attempts() {
        // The predicate accepts elapsed time only; attempt bursts cannot influence it.
        let stall_ms = ROUND_STALL_ESCAPE.as_millis() as i64;

        assert!(!round_escape_owed(0), "first refusal starts the clock");
        assert!(!round_escape_owed(967), "N=8 premature escape witness");
        assert!(!round_escape_owed(3_388), "N=8 maximum recorded escape");
        assert!(
            !round_escape_owed(stall_ms - 1),
            "the interval is strictly below the 15-second bound"
        );
        assert!(
            !round_escape_owed(-1),
            "negative elapsed time cannot escape"
        );
        assert!(
            round_escape_owed(stall_ms),
            "the precise stall boundary licenses escape"
        );
        assert!(
            round_escape_owed(stall_ms + 1),
            "the gate remains open after the bound"
        );
    }

    /// A `BlockDagStorage` whose `lookup` fails, so a DAG read error is observable as one, and whose
    /// representation has an **empty fringe with a non-empty height map** — the shape that sends
    /// `is_active_validator` to the `lookup` branch.
    struct FailingLookupDag {
        first_hash: BlockHash,
    }

    #[async_trait::async_trait]
    impl BlockDagStorage for FailingLookupDag {
        async fn get_representation(
            &self,
        ) -> Arc<rchain_block_storage::dag::representation::DagRepresentation> {
            Arc::new(
                rchain_block_storage::dag::representation::DagRepresentation {
                    dag_set: Arc::new(BTreeSet::new()),
                    child_map: Arc::new(std::collections::BTreeMap::new()),
                    height_map: Arc::new(std::collections::BTreeMap::from([(
                        BlockHeight::try_from(1).unwrap(),
                        BTreeSet::from([self.first_hash]),
                    )])),
                    dag_message_state:
                        rchain_block_storage::dag::message_state::DagMessageState::empty(),
                },
            )
        }

        /// The failing read: a DAG that cannot answer is the whole point of this stub.
        async fn lookup(
            &self,
            _block_hash: &BlockHash,
        ) -> Result<Option<rchain_models::block_metadata::BlockMetadata>, String> {
            Err("the DAG store is down".to_string())
        }

        // The rest of the trait is not on this path.
        async fn insert(
            &self,
            _block_metadata: rchain_models::block_metadata::BlockMetadata,
            _block: BlockMessage,
        ) -> Result<(), String> {
            todo!("not on the is_active_validator path")
        }
        async fn lookup_by_deploy_id(&self, _d: &DeployId) -> Result<Option<BlockHash>, String> {
            todo!("not on the is_active_validator path")
        }
        async fn add_deploy(&self, _d: SignedDeployData) -> Result<(), String> {
            todo!("not on the is_active_validator path")
        }
        async fn pooled_deploys(
            &self,
        ) -> Result<std::collections::BTreeMap<DeployId, SignedDeployData>, String> {
            todo!("not on the is_active_validator path")
        }
        async fn contains_deploy_in_pool(&self, _d: &DeployId) -> Result<bool, String> {
            todo!("not on the is_active_validator path")
        }
    }

    /// **A DAG read that fails is not "this node is not bonded"** (AUDIT C67).
    ///
    /// The oracle is `Proposer.scala:240-243`, whose `else` branch takes the bonds map through
    /// `BlockDagStorage[F].lookupUnsafe` — an errored lookup is an error in `F`, so the Scala never
    /// answers "not bonded" for it. The port flattened the error into an empty bonds map, and an
    /// empty map answers `false` for every sender, so a node whose DAG could not be read would stop
    /// proposing and report `NotBonded` — a consensus-adjacent wrong answer with no error anywhere.
    ///
    /// Falsifier, both forms. Pre-fix (witnessing): `is_active_validator` answered `false` for a
    /// failing `lookup`, and the assertion `!is_active_validator(..)` **passed on exactly that**
    /// (run 2026-09-24 before the change). Post-fix: the same call is an `Err` naming the failure.
    #[tokio::test]
    async fn a_dag_read_that_fails_is_not_an_inactive_validator() {
        let dag: Arc<dyn BlockDagStorage> = Arc::new(FailingLookupDag {
            first_hash: BlockHash::new([0x11; 32]),
        });
        let sender = Validator::new([0x22; rchain_models::validator::LENGTH]);

        let err = is_active_validator(&dag, &sender)
            .await
            .expect_err("a DAG that cannot be read must not answer \"not an active validator\"");
        assert!(
            err.contains("the DAG store is down"),
            "and the refusal must name the failure, got: {err}"
        );
    }

    /// **A DAG read that fails is not "this node is not bonded" — on the attestation path either**
    /// (W-N5-07).
    ///
    /// The propose path's version of this distinction is AUDIT C67, tested above. The attestation path
    /// had no such gate at all, so a node outside the active set — or one whose DAG could not be read —
    /// would still sign "the fringe is final" with its own name on it, and the network counts
    /// attestations toward quorum.
    ///
    /// The two refusals are asserted apart, which is the whole reason this is a separate function: a
    /// flattened version answering `Err` for both would pass the first assertion and fail the second
    /// on its message.
    ///
    /// It lives in this module's tests because `FailingLookupDag` does; the function under test is
    /// `api::block_api_impl::attestation_is_admissible`, which is what the attestation handler calls.
    #[tokio::test]
    async fn a_dag_read_that_fails_is_not_an_unbonded_attester() {
        let dag: Arc<dyn BlockDagStorage> = Arc::new(FailingLookupDag {
            first_hash: BlockHash::new([0x11; 32]),
        });
        let key = [0x22u8; rchain_models::validator::LENGTH];

        let err = crate::api::block_api_impl::attestation_is_admissible(&dag, &key)
            .await
            .expect_err("a DAG that cannot be read must not answer \"not in the active set\"");
        assert!(
            err.contains("cannot decide whether this node is bonded"),
            "the refusal must say it could not *tell*, not that the node is unbonded: {err}"
        );
        assert!(
            err.contains("the DAG store is down"),
            "and it must name the failure underneath: {err}"
        );
    }

    /// A DAG that answers cleanly and does not list the key: the other arm of the same refusal.
    struct UnbondedDag {
        newest: BlockHash,
    }

    #[async_trait::async_trait]
    impl BlockDagStorage for UnbondedDag {
        async fn get_representation(
            &self,
        ) -> Arc<rchain_block_storage::dag::representation::DagRepresentation> {
            Arc::new(
                rchain_block_storage::dag::representation::DagRepresentation {
                    dag_set: Arc::new(BTreeSet::new()),
                    child_map: Arc::new(std::collections::BTreeMap::new()),
                    height_map: Arc::new(std::collections::BTreeMap::from([(
                        BlockHeight::try_from(1).unwrap(),
                        BTreeSet::from([self.newest]),
                    )])),
                    dag_message_state:
                        rchain_block_storage::dag::message_state::DagMessageState::empty(),
                },
            )
        }

        /// The answer that is *not* an error: a block whose bond map does not list the sender.
        async fn lookup(
            &self,
            _block_hash: &BlockHash,
        ) -> Result<Option<rchain_models::block_metadata::BlockMetadata>, String> {
            Ok(Some(rchain_models::block_metadata::BlockMetadata {
                block_hash: self.newest,
                block_num: BlockHeight::try_from(1).unwrap(),
                sender: Validator::new([9u8; 65]),
                seq_num: rchain_shared::refined::SeqNum::zero(),
                justifications: BTreeSet::new(),
                bonds_map: std::collections::BTreeMap::new(),
                validated: true,
                validation_failed: false,
                slashable: false,
                failure_cause: None,
                slash_severity: SlashSeverity::Unspecified,
                restore_attempts: 0,
                fringe: BTreeSet::new(),
                fringe_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
                member_of_fringe: None,
            }))
        }

        async fn insert(
            &self,
            _block_metadata: rchain_models::block_metadata::BlockMetadata,
            _block: BlockMessage,
        ) -> Result<(), String> {
            todo!("not on the is_active_validator path")
        }
        async fn lookup_by_deploy_id(&self, _d: &DeployId) -> Result<Option<BlockHash>, String> {
            todo!("not on the is_active_validator path")
        }
        async fn add_deploy(&self, _d: SignedDeployData) -> Result<(), String> {
            todo!("not on the is_active_validator path")
        }
        async fn pooled_deploys(
            &self,
        ) -> Result<std::collections::BTreeMap<DeployId, SignedDeployData>, String> {
            todo!("not on the is_active_validator path")
        }
        async fn contains_deploy_in_pool(&self, _d: &DeployId) -> Result<bool, String> {
            todo!("not on the is_active_validator path")
        }
    }

    /// **A DAG that answers "not bonded" gets its own refusal** (W-N5-07).
    ///
    /// The row asks that `finality_attestation` refuse an identity that is not a bonded validator. The
    /// test above pins the *error* arm — a DAG that cannot be read must not answer "unbonded" — and
    /// this is the other half: a DAG that answers cleanly and does not list the key must be refused
    /// with the sentence about the active set, **not** the one about not being able to tell. A caller
    /// that cannot tell the two apart reads a store fault as a bonding fact, which is AUDIT C67's class
    /// one arm over.
    ///
    /// Falsified by folding the arms back together: answer the "cannot decide" sentence for `Ok(false)`
    /// and this fails while the test above still passes.
    #[tokio::test]
    async fn a_dag_that_answers_not_bonded_is_refused_with_its_own_sentence() {
        let dag: Arc<dyn BlockDagStorage> = Arc::new(UnbondedDag {
            newest: BlockHash::new([0x33; 32]),
        });
        let key = [0x22u8; rchain_models::validator::LENGTH];

        let err = crate::api::block_api_impl::attestation_is_admissible(&dag, &key)
            .await
            .expect_err("an identity the active set does not list must be refused");
        assert!(
            err.contains("not in the active validator set"),
            "the refusal must say the node is not in the set: {err}"
        );
        assert!(
            !err.contains("cannot decide"),
            "and it must not read as a fault, which is the other arm's sentence: {err}"
        );
    }

    /// A DAG whose pool is whatever the test hands it, and which has never seen a deploy — so the replay
    /// guard never fires. Every other trait method is off the selection path.
    struct PoolDag {
        pooled: std::collections::BTreeMap<DeployId, SignedDeployData>,
    }

    #[async_trait::async_trait]
    impl BlockDagStorage for PoolDag {
        async fn get_representation(
            &self,
        ) -> Arc<rchain_block_storage::dag::representation::DagRepresentation> {
            todo!("not on the selection path")
        }
        async fn insert(
            &self,
            _m: rchain_models::block_metadata::BlockMetadata,
            _b: BlockMessage,
        ) -> Result<(), String> {
            todo!("not on the selection path")
        }
        async fn lookup(
            &self,
            _h: &BlockHash,
        ) -> Result<Option<rchain_models::block_metadata::BlockMetadata>, String> {
            todo!("not on the selection path")
        }
        async fn lookup_by_deploy_id(&self, _d: &DeployId) -> Result<Option<BlockHash>, String> {
            Ok(None)
        }
        async fn add_deploy(&self, _d: SignedDeployData) -> Result<(), String> {
            todo!("not on the selection path")
        }
        async fn pooled_deploys(
            &self,
        ) -> Result<std::collections::BTreeMap<DeployId, SignedDeployData>, String> {
            Ok(self.pooled.clone())
        }
        async fn contains_deploy_in_pool(&self, _d: &DeployId) -> Result<bool, String> {
            todo!("not on the selection path")
        }
    }

    /// A pooled deploy valid at the block being proposed (`add_deploy`'s shape, minimal).
    fn pooled(sig: u16, valid_after: i64) -> SignedDeployData {
        SignedDeployData {
            data: DeployData {
                attachments: Vec::new(),
                term: "Nil".to_string(),
                timestamp: 0,
                phlo_price: 1,
                phlo_limit: 1,
                valid_after_block_number: valid_after,
                shard_id: "root".to_string(),
            },
            deployer: vec![0u8; 65],
            sig: sig.to_le_bytes().to_vec(),
            sig_algorithm: "secp256k1".to_string(),
        }
    }

    /// **The regression test for AUDIT C123's bound**, and it is arithmetic on purpose: the invariant the
    /// per-deploy seed needs is `deploys + slashes ≤ 255`. A test that only exercised a pool of 300 would
    /// pass while the *bound itself* drifted, so this asserts the bound.
    ///
    /// `budget(0) == 255` is the half that matters: it is what makes a selection of 256 unreachable, and
    /// 256 pooled deploys were exactly what stopped block production — `BlockCreator::create` computed
    /// `u8::try_from(selected.len() + to_slash.len())` and returned `Err` *before any state work*, so the
    /// node could not produce a block at all until the pool drained.
    #[test]
    fn the_per_block_budget_leaves_room_for_every_seed_the_block_needs() {
        assert_eq!(
            per_block_deploy_budget(0),
            MAX_BLOCK_DEPLOYS,
            "a block with no slashes may still not select 256 deploys"
        );
        for slashes in 0..=MAX_BLOCK_DEPLOYS {
            assert!(
                per_block_deploy_budget(slashes) + slashes <= MAX_BLOCK_DEPLOYS,
                "the seed index is a u8, so {slashes} slashes leave {} deploys",
                per_block_deploy_budget(slashes)
            );
        }
        // Past the bound the budget saturates rather than wrapping — a slash count that large is
        // unreachable (`to_slash` comes from the block's justifications), and a wrap here would be the
        // arithmetic that got C123 into trouble in the first place.
        assert_eq!(per_block_deploy_budget(MAX_BLOCK_DEPLOYS + 1), 0);
    }

    /// **The other half of C123**: the selection is a bounded, deterministic prefix of the pool's valid
    /// entries, so a pool larger than one block can carry produces a block instead of an error.
    #[tokio::test]
    async fn the_selection_is_capped_and_takes_the_pools_canonical_prefix() {
        // Two bytes per key, little-endian so the map's order is the index order — no cast and no
        // division, which also keeps this fixture out of the partiality gate's `div`/`cast` classes.
        let key = |i: u16| i.to_le_bytes().to_vec();
        let pooled_map: std::collections::BTreeMap<DeployId, SignedDeployData> =
            (0..300u16).map(|i| (key(i), pooled(i, 0))).collect();
        let dag = PoolDag { pooled: pooled_map };
        let next = BlockHeight::try_from(1).unwrap();

        let selected = select_deploys(&dag, next, per_block_deploy_budget(0))
            .await
            .unwrap();
        assert_eq!(
            selected.len(),
            MAX_BLOCK_DEPLOYS,
            "a pool three times the block's capacity must yield a block-sized selection"
        );
        // The prefix is the pool's **canonical order**, and the assertion takes that order from the map
        // rather than from a hand-written list: the first draft of this test spelled the expected keys
        // out and got them wrong (`Vec<u8>` keys sort lexicographically, so `[0,1]` precedes `[1,0]` and
        // the prefix is not the first N indices). What has to hold is the *property* — the selection is
        // the pool's own order, truncated — and that is what this states.
        let mut all_keys: Vec<DeployId> = dag.pooled.keys().cloned().collect();
        all_keys.sort();
        let expected: Vec<DeployId> = all_keys[..MAX_BLOCK_DEPLOYS].to_vec();
        assert_eq!(selected, expected);

        // A slash takes a seed as well, so it takes a deploy's place.
        let with_one_slash = select_deploys(&dag, next, per_block_deploy_budget(1))
            .await
            .unwrap();
        assert_eq!(with_one_slash.len(), MAX_BLOCK_DEPLOYS - 1);

        // And the pool's own filter still runs at the exact validation boundary. At candidate
        // block 51 with lifespan 50, valid_after=1 is already expired (`1 <= 51 - 50`), while 2 is
        // still valid. This is the edge that used to let the proposer construct a block it rejected
        // itself during validation.
        let mut with_expired = std::collections::BTreeMap::new();
        with_expired.insert(vec![0u8, 8u8], pooled(8, 2));
        with_expired.insert(vec![0u8, 9u8], pooled(9, 1));
        with_expired.insert(vec![0u8, 10u8], pooled(10, 0));
        let dag = PoolDag {
            pooled: with_expired,
        };
        let expiry_edge = BlockHeight::try_from(51).unwrap();
        assert_eq!(
            select_deploys(&dag, expiry_edge, per_block_deploy_budget(0))
                .await
                .unwrap(),
            vec![vec![0u8, 8u8]],
            "the proposer must exclude both an old deploy and the exact first-height expiry boundary"
        );
    }

    /// **The reason a deploy was left out, and the arithmetic it is read off** (issue #356).
    ///
    /// [`not_selectable`] is the one place the filter's three reasons live, and it is what
    /// [`report_nothing_to_include`] counts with — so a report that says "3 expired" counts the same
    /// boundary the selection drops by. The boundaries are asserted on both sides, because an
    /// off-by-one here would either lose a deploy that was still usable or claim one is expired that
    /// is not.
    #[test]
    fn the_reason_a_deploy_is_left_out_is_the_filters_own_arithmetic() {
        let next = 51; // the boundary Aria's test pins: at 51, an anchor of 1 is exactly expired
        assert_eq!(
            not_selectable(1, next, false),
            Some(NotSelectable::Expired),
            "valid_after == next_block - 50 is expired: the filter drops it, so the report says so"
        );
        assert_eq!(
            not_selectable(2, next, false),
            None,
            "…and one height later it is still usable"
        );
        assert_eq!(
            not_selectable(52, next, false),
            Some(NotSelectable::Future),
            "an anchor above the block being built is not yet valid"
        );
        assert_eq!(
            not_selectable(51, next, false),
            None,
            "an anchor at exactly the block being built is fine"
        );
        assert_eq!(
            not_selectable(10, next, true),
            Some(NotSelectable::Replay),
            "already in the DAG: a second inclusion is a replay, and it is only reached when the \
             anchor itself is fine"
        );
        // The reason is *a* reason and not the set of them: a deploy can be both expired and already
        // in the DAG, and the report only needs to name one.
        assert_eq!(
            not_selectable(1, next, true),
            Some(NotSelectable::Expired),
            "the anchor is checked first, so an expired deploy reads as expired rather than as a replay"
        );
    }

    /// The operational wedge from #284 was a disagreement between selection and validation at
    /// the exact expiry boundary. Exercise both sides together across the boundary so the proposer
    /// cannot manufacture a block that the validator rejects, and so a deploy that is still valid
    /// remains proposable on the following height.
    #[tokio::test]
    async fn expiry_boundary_selection_and_validation_advance_together() {
        use rchain_models::casper::protocol::casper_message::{PCost, ProcessedDeploy};

        let processed = |d: SignedDeployData| ProcessedDeploy {
            deploy: d,
            cost: PCost { cost: 0 },
            deploy_log: Vec::new(),
            is_failed: false,
            system_deploy_error: None,
        };

        // At block 51, valid_after=1 is exactly expired and valid_after=2 is still usable.
        let mut pool_51 = std::collections::BTreeMap::new();
        pool_51.insert(vec![0u8, 1u8], pooled(1, 1));
        pool_51.insert(vec![0u8, 2u8], pooled(2, 2));
        let dag_51 = PoolDag {
            pooled: pool_51.clone(),
        };
        let height_51 = BlockHeight::try_from(51).unwrap();
        let selected_51 = select_deploys(&dag_51, height_51, per_block_deploy_budget(0))
            .await
            .unwrap();
        assert_eq!(selected_51, vec![vec![0u8, 2u8]]);

        let mut candidate_51 = block();
        candidate_51.block_number = height_51;
        candidate_51.state.deploys = selected_51
            .iter()
            .map(|id| processed(pool_51[id].clone()))
            .collect();
        assert_eq!(
            crate::validate::transaction_expiration(&candidate_51, DEPLOY_LIFESPAN),
            crate::block_status::BlockStatus::Valid,
            "anything selected for block 51 must pass block 51's expiry validation"
        );
        assert_eq!(
            crate::validate::future_transaction(&candidate_51),
            crate::block_status::BlockStatus::Valid,
            "anything selected for block 51 must also pass its future-deploy validation"
        );

        // Move one height forward. valid_after=2 is now exactly expired; a fresh valid_after=3 deploy
        // must take its place. This is the liveness half: the boundary advances rather than wedging
        // forever on the stale deploy that used to be selected and self-rejected.
        let mut pool_52 = std::collections::BTreeMap::new();
        pool_52.insert(vec![0u8, 2u8], pooled(2, 2));
        pool_52.insert(vec![0u8, 3u8], pooled(3, 3));
        let dag_52 = PoolDag {
            pooled: pool_52.clone(),
        };
        let height_52 = BlockHeight::try_from(52).unwrap();
        let selected_52 = select_deploys(&dag_52, height_52, per_block_deploy_budget(0))
            .await
            .unwrap();
        assert_eq!(selected_52, vec![vec![0u8, 3u8]]);

        let mut candidate_52 = block();
        candidate_52.block_number = height_52;
        candidate_52.state.deploys = selected_52
            .iter()
            .map(|id| processed(pool_52[id].clone()))
            .collect();
        assert_eq!(
            crate::validate::transaction_expiration(&candidate_52, DEPLOY_LIFESPAN),
            crate::block_status::BlockStatus::Valid,
            "the next candidate remains valid after crossing the original block-51 wedge"
        );
        assert_eq!(
            crate::validate::future_transaction(&candidate_52),
            crate::block_status::BlockStatus::Valid
        );
    }

    fn proposer(create: BlockCreatorResult) -> Proposer {
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
            let create = create.clone();
            Box::pin(async move { Ok(create) })
        });
        let validate: Arc<
            dyn Fn(&BlockMessage) -> BoxFuture<Result<(), ValidateError>> + Send + Sync,
        > = Arc::new(|_b| Box::pin(async { Ok(()) }));
        let effect: Arc<dyn Fn(&BlockMessage) -> BoxFuture<()> + Send + Sync> =
            Arc::new(|_b| Box::pin(async {}));
        let validator = ValidatorIdentity::from_hex(
            "67e56582298859ddae725f972992a07c6c4fb9f62a8fff58ce3ca926a1063530",
        )
        .unwrap();
        let log: Arc<dyn Log> = Arc::new(rchain_shared::log::NopLog);
        let consecutive_failures = Arc::new(AtomicU64::new(0));
        let stale_snapshot_equivocations = Arc::new(AtomicU64::new(0));
        Proposer::new(
            get_seq,
            check_active,
            create_block,
            validate,
            effect,
            validator,
            log,
            consecutive_failures,
            stale_snapshot_equivocations,
            false,
            block_store(),
        )
    }

    /// A proposer whose own validation refuses the block it just built, with `status` — the rig the two
    /// tests above need and the existing helpers do not provide (`proposer(..)` fixes the *creation*
    /// outcome, not the validation one).
    fn proposer_failing_validation(status: crate::block_status::BlockStatus) -> Proposer {
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
        > = Arc::new(|_v, _source| Box::pin(async { Ok(BlockCreatorResult::Created(block())) }));
        let validate: Arc<
            dyn Fn(&BlockMessage) -> BoxFuture<Result<(), ValidateError>> + Send + Sync,
        > = Arc::new(move |b| {
            let status = status;
            let b = b.clone();
            Box::pin(async move {
                Err(ValidateError::ValidationFailed(
                    rchain_models::block_metadata::BlockMetadata::from_block(&b),
                    status,
                ))
            })
        });
        let effect: Arc<dyn Fn(&BlockMessage) -> BoxFuture<()> + Send + Sync> =
            Arc::new(|_b| Box::pin(async {}));
        let validator = ValidatorIdentity::from_hex(
            "67e56582298859ddae725f972992a07c6c4fb9f62a8fff58ce3ca926a1063530",
        )
        .unwrap();
        let log: Arc<dyn Log> = Arc::new(rchain_shared::log::NopLog);
        let consecutive_failures = Arc::new(AtomicU64::new(0));
        let stale_snapshot_equivocations = Arc::new(AtomicU64::new(0));
        Proposer::new(
            get_seq,
            check_active,
            create_block,
            validate,
            effect,
            validator,
            log,
            consecutive_failures,
            stale_snapshot_equivocations,
            false,
            block_store(),
        )
    }

    /// A block store for tests that never read it — this file's tests run with the injection off.
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

    fn block() -> BlockMessage {
        BlockMessage {
            version: 1,
            shard_id: "root".to_string(),
            block_hash: BlockHash::new([1u8; 32]),
            block_number: 0.try_into().unwrap(),
            sender: Validator::new([0u8; 65]),
            seq_num: 0.try_into().unwrap(),
            pre_state_hash: StateHash::new([0u8; 32]),
            post_state_hash: StateHash::new([0u8; 32]),
            justifications: vec![],
            bonds: std::collections::BTreeMap::new(),
            rejected_deploys: std::collections::BTreeSet::new(),
            rejected_blocks: std::collections::BTreeSet::new(),
            rejected_senders: std::collections::BTreeSet::new(),
            state: rchain_models::casper::protocol::casper_message::RholangState::default(),
            sig_algorithm: "secp256k1".to_string(),
            sig: vec![],
            timestamp: 0,
        }
    }

    #[tokio::test]
    async fn no_new_deploys_returns_failure() {
        let p = proposer(BlockCreatorResult::NoNewDeploys);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (result, block_opt) = p
            .propose(ProposeSource::Explicit { is_async: false }, tx)
            .await
            .unwrap();
        assert_eq!(result.propose_status, ProposeStatus::NoNewDeploys);
        assert!(block_opt.is_none());
        assert!(matches!(rx.await.unwrap(), ProposerResult::Failure { .. }));
    }

    #[tokio::test]
    async fn created_block_returns_success() {
        let p = proposer(BlockCreatorResult::Created(block()));
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (result, block_opt) = p
            .propose(ProposeSource::Explicit { is_async: false }, tx)
            .await
            .unwrap();
        assert_eq!(result.propose_status, ProposeStatus::ProposeSuccess);
        assert!(block_opt.is_some());
        assert!(matches!(rx.await.unwrap(), ProposerResult::Success { .. }));
    }

    /// **A divergence about a fringe key's value must not halt this node's production** (C270, pass §92).
    ///
    /// `InvalidPreStateHash` is `FailureCause::Divergence`: the block is valid for a node whose view of
    /// the parents is the one it was built under. Counting it toward the halt is what stopped three of
    /// four nodes at one height on a devnet and left four heads that never met
    /// (`spec/audit/evidence/n-anchor-drill/self-reject-stall.txt`). So: counted *not at all*, read as
    /// "not due" so the next tick re-derives, and **no block broadcast** — the block that this node's own
    /// view rejected must not go out. The control below shows the halt still works for a status that is
    /// the block's own fault, so this is a carve-out and not a disabled safety property.
    #[tokio::test]
    async fn a_divergence_self_rejection_is_not_counted_toward_the_autopropose_halt() {
        let p = proposer_failing_validation(crate::block_status::BlockStatus::InvalidPreStateHash);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (result, block_opt) = p
            .propose(ProposeSource::Explicit { is_async: false }, tx)
            .await
            .unwrap();
        assert_eq!(
            result.propose_status,
            ProposeStatus::NotEnoughNewBlocks,
            "a divergence is 'not due', not a failure of this node's accounting"
        );
        assert!(
            block_opt.is_none(),
            "and the block this node no longer agrees with is not broadcast"
        );
        assert_eq!(
            p.consecutive_failures.load(Ordering::Relaxed),
            0,
            "the counter the autopropose timer halts on must not move — this is the whole claim"
        );
        assert!(matches!(rx.await.unwrap(), ProposerResult::Failure { .. }));
    }

    /// The control: a status that is the **block's own fault** is still counted, so the halt is a
    /// carve-out for divergence rather than a disabled property.
    #[tokio::test]
    async fn a_block_fault_self_rejection_is_still_counted() {
        let p =
            proposer_failing_validation(crate::block_status::BlockStatus::InvalidDeploySignature);
        assert_eq!(
            p.consecutive_failures.load(Ordering::Relaxed),
            0,
            "the counter starts at zero"
        );
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let _ = p
            .propose(ProposeSource::Explicit { is_async: false }, tx)
            .await;
        assert_eq!(
            p.consecutive_failures.load(Ordering::Relaxed),
            1,
            "a block-fault failure still counts toward the halt"
        );
    }

    /// **The falsifier for the §48 race fix.** A self-created block whose insert collides as an
    /// equivocation is a stale-snapshot race, not a self-validation failure: it must *not* count toward
    /// the timer halt (`consecutive_failures` stays 0), it must be counted on its own gauge, and it
    /// must read as "not due" (`NotEnoughNewBlocks`) so the next tick re-derives and succeeds.
    #[tokio::test]
    async fn a_stale_snapshot_self_equivocation_is_not_counted_as_a_failure() {
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
        > = Arc::new(|_v, _source| Box::pin(async { Ok(BlockCreatorResult::Created(block())) }));
        let validate: Arc<
            dyn Fn(&BlockMessage) -> BoxFuture<Result<(), ValidateError>> + Send + Sync,
        > = Arc::new(|_b| Box::pin(async { Err(ValidateError::SelfEquivocation) }));
        let effect: Arc<dyn Fn(&BlockMessage) -> BoxFuture<()> + Send + Sync> =
            Arc::new(|_b| Box::pin(async {}));
        let validator = ValidatorIdentity::from_hex(
            "67e56582298859ddae725f972992a07c6c4fb9f62a8fff58ce3ca926a1063530",
        )
        .unwrap();
        let log: Arc<dyn Log> = Arc::new(rchain_shared::log::NopLog);
        let consecutive_failures = Arc::new(AtomicU64::new(0));
        let stale_snapshot_equivocations = Arc::new(AtomicU64::new(0));

        let p = Proposer::new(
            get_seq,
            check_active,
            create_block,
            validate,
            effect,
            validator,
            log,
            consecutive_failures.clone(),
            stale_snapshot_equivocations.clone(),
            false,
            block_store(),
        );

        let (tx, rx) = tokio::sync::oneshot::channel();
        let (result, block_opt) = p
            .propose(ProposeSource::Explicit { is_async: false }, tx)
            .await
            .unwrap();
        assert_eq!(
            result.propose_status,
            ProposeStatus::NotEnoughNewBlocks,
            "a self-equivocation is 'not due', not 'invalid'"
        );
        assert!(block_opt.is_none());
        assert_eq!(
            consecutive_failures.load(Ordering::Relaxed),
            0,
            "the race must not count toward the timer halt"
        );
        assert_eq!(
            stale_snapshot_equivocations.load(Ordering::Relaxed),
            1,
            "the race is counted on its own gauge, so a stuck node is still visible"
        );
        assert!(matches!(rx.await.unwrap(), ProposerResult::Failure { .. }));
    }
}

/// Whether this validator should attest now: its own weight plus the stake already moving on the fringe
/// reaches a supermajority.
///
/// The stake that counts as "moving" toward the quorum: the **live** weight set minus ourselves.
///
/// The set is the shared liveness rule ([`liveness::live_weight_set`], #70 increment 2) — the same
/// function the finalizer's partition ranges over, so "who is speaking" cannot differ between the two
/// consumers. The exclusion is ours to make here: our own message is the attestation we are about to
/// add, and [`attestation_reaches_supermajority`] counts it on the other side of the comparison.
///
/// **What this replaced, and why it was a defect (F4).** The stake used to be summed over the senders
/// of `pre_state.justifications`, which is `latest_msgs` — a map that keeps a silent sender's last
/// message **indefinitely** — so "moving" meant *has ever spoken*. Three equal validators with one dead
/// summed 200 (the live peer's message *plus* the dead one's stale one) and, with `own_stake`, reached
/// 300 of 300: a supermajority that does not exist. Pinned by
/// `a_silent_validators_stale_message_does_not_carry_the_quorum` in `attestation_suppression_tests`.
///
/// **The denominator is not this set.** The quorum is measured against the whole bonded map
/// (`pre_state_bonds`), not the live one: a quorum over the live stake would let any self-consistent
/// subset finalise, and under a partition each side would finalise its own view — two conflicting
/// finalisations. The issue is that an absent validator must not *block* the partition; it is not that
/// an absent validator's stake stops counting. See `liveness`' module doc and the §6 row.
fn moving_attestation_stake(live_bonds: &BTreeMap<Validator, NonNegI64>, own: &Validator) -> i128 {
    live_bonds
        .iter()
        .filter(|(v, _)| *v != own)
        .map(|(_, s)| i128::from(i64::from(*s)))
        .sum()
}

/// Whether this node has been quiet long enough to speak again, read from the height of its own latest
/// message among the senders' `latest` heights — so the pace bound needs no new state, no counter, and no
/// persistence.
fn cadence_due(
    latest: &BTreeMap<Validator, BlockHeight>,
    own: &Validator,
    tip: BlockHeight,
) -> bool {
    match latest.get(own) {
        Some(mine) => liveness::heights_behind(tip, *mine) > liveness::LIVENESS_WINDOW,
        // Nothing from us in the DAG yet: there is no quiet to have broken.
        None => true,
    }
}

/// **How long a deploy-bearing block may go unfinalised and still license a prompt attestation**, in
/// heights. Measured in the in-process network (`quiet_chain_tests`): a healthy round finalises a deploy
/// within 3 heights of it, and with one validator killed within 7, because the dead one has to age out of
/// the partition (`LIVENESS_WINDOW`) first. Three windows leaves room above both.
///
/// It is the storm bound, and it is what lets the licence below be read from what the node has seen
/// without bringing C171 back: if finality stops advancing for any other reason, prompt attestation stops
/// this many heights after the deploy, instead of every validator attesting at every height for ever.
const ATTESTATION_HORIZON: i64 = 3 * liveness::LIVENESS_WINDOW;

/// The guard's two inputs, from the **oldest deploy-bearing block this node has seen that the fringe it is
/// building on does not yet cover** (`None`: there is none).
///
/// - `nothing_to_finalize`: there is no such block.
/// - `new_state_transition`: there is one, and it is within [`ATTESTATION_HORIZON`] of the tip.
///
/// **The licence used to be "a parent carries deploys", and that is AUDIT C209.** With no autopropose the
/// only proposal attempts are the deploy itself and the attestation tap, and each round's parents are the
/// snapshot taken when the previous round closed. One round after a deploy no parent carries it any more,
/// so every validator was paced to its cadence, the cadence is never due on a chain that is not moving,
/// and the fringe stopped one layer short of the deploy: measured live (#214, criterion 1: 693 samples,
/// finality `none` throughout) and in-process. A deploy-bearing block needs every bonded sender to speak
/// for a few rounds after it, not one, and this is what licenses that.
fn attestation_inputs(oldest_unfinalized: Option<BlockHeight>, tip: BlockHeight) -> (bool, bool) {
    match oldest_unfinalized {
        None => (true, false),
        Some(h) => (
            false,
            liveness::heights_behind(tip, h) <= ATTESTATION_HORIZON,
        ),
    }
}

/// **The height the unfinalised work's age is counted from**: the work's own height, or the fringe's when
/// the work lies below it.
///
/// The horizon bounds a *finality stall* — work that has waited `ATTESTATION_HORIZON` heights without the
/// fringe reaching it. Work below a fringe that has since moved on has not waited at all: it arrived late.
/// That is a returning validator (#223): while catching up it proposed on each block it replayed, so its
/// chain holds blocks at the heights it was absent for, two of them epoch boundaries carrying a
/// `CloseBlock`. Every survivor read that block, twenty heights below the tip, as twenty-height-old work,
/// withheld its attestation, and the work could only be finalised by the attestations it withheld:
/// finality stopped for good, live, with the survivors' fringe three heights behind the tip. Counting from
/// the fringe licenses them while the fringe keeps up, and a stalled fringe still ends the licence on the
/// same bound.
fn work_age_floor(
    oldest_unfinalized: Option<BlockHeight>,
    fringe_top: Option<BlockHeight>,
) -> Option<BlockHeight> {
    oldest_unfinalized.map(|h| match fringe_top {
        Some(f) if f > h => f,
        _ => h,
    })
}

/// The guard's inputs and its decision, for the diagnostic line.
struct AttestationDecision<'a> {
    nothing_to_finalize: bool,
    new_state_transition: bool,
    quorum_reachable: bool,
    cadence_due: bool,
    paced: bool,
    /// The senders of the round snapshot this block is built on.
    round_senders: BTreeSet<&'a Validator>,
    /// Proposals declined since this node last advanced past the round (the escape's attempt counter).
    blocked_attempts: i64,
    escape: bool,
    suppressed: bool,
}

/// The decision as one line. **The tip is not in it**, so that a line that only moved height compares
/// equal and the rate limit holds; the caller prefixes it.
fn describe_attestation_decision(d: &AttestationDecision<'_>) -> String {
    let senders: Vec<String> = d
        .round_senders
        .iter()
        .map(|v| {
            v.as_bytes()[..4]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        })
        .collect();
    format!(
        "{} — nothing_to_finalize={} new_state_transition={} quorum_reachable={} cadence_due={} \
         paced={} round=[{}] blocked_attempts={} escape={}",
        if d.suppressed { "withheld" } else { "licensed" },
        d.nothing_to_finalize,
        d.new_state_transition,
        d.quorum_reachable,
        d.cadence_due,
        d.paced,
        senders.join(" "),
        d.blocked_attempts,
        d.escape,
    )
}

/// The last attestation line logged, and the tip it was logged at: a line is due when it **changes**, or
/// every `ATTESTATION_LOG_INTERVAL` heights while it does not — the rate limit the finality stall line uses
/// (`interpreter_util.rs`), for the same reason: the change is the event, and a steady state is not.
static LAST_ATTESTATION_LINE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
static LAST_ATTESTATION_TIP: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(i64::MIN);
const ATTESTATION_LOG_INTERVAL: i64 = 100;

fn attestation_line_due(line: &str, tip: i64) -> bool {
    line_due(&LAST_ATTESTATION_LINE, &LAST_ATTESTATION_TIP, line, tip)
}

/// The rate limit itself, over state the caller owns — so a test can hold its own and not race the
/// node's statics, which every `create_block` in the test binary writes.
fn line_due(
    last_line: &std::sync::Mutex<Option<String>>,
    last_tip: &std::sync::atomic::AtomicI64,
    line: &str,
    tip: i64,
) -> bool {
    let changed = {
        let mut last = last_line.lock().unpoison();
        let changed = last.as_deref() != Some(line);
        if changed {
            *last = Some(line.to_string());
        }
        changed
    };
    let due =
        changed || tip.saturating_sub(last_tip.load(Ordering::Relaxed)) >= ATTESTATION_LOG_INTERVAL;
    if due {
        last_tip.store(tip, Ordering::Relaxed);
    }
    due
}

/// Whether this node withholds its attestation for the block it is building.
///
/// A pair of bounds, and the order matters:
///
/// - nothing to finalise → suppress. An idle chain must not grow.
/// - the quorum is reachable → attest a deploy-bearing block promptly (it needs the quorum), and an
///   attestation only while this node is itself behind (C171: the #70 storm is attesting to every
///   remote block, each an attestation that is in turn a remote block for the next). **Only a
///   `paced` proposal** — one the node raised on its own — is bounded this way; a caller's explicit
///   propose is not a reaction to a remote block and is not silenced (`ProposeSource`).
/// - the quorum is unreachable, but a state transition exists to attest to **and** this node has itself
///   been quiet for longer than `ATTESTATION_WINDOW` → attest anyway, at that reduced cadence.
/// - otherwise → suppress.
///
/// **This used to be `nothing_to_finalize || !(new_state_transition || quorum)`, and that was #70's
/// F3.** `new_state_transition` ("any parent carries deploys", the ordinary case on a chain with
/// traffic) was OR-ed *inside* the quorum test, so it short-circuited it to `false`; suppression then
/// collapsed to `nothing_to_finalize`, which was also `false` because the deploy-bearing block was
/// unfinalized. A node that had lost over a third of its stake attested at **every** height — the
/// 276-blocks-in-a-minute run on #70 — and the contract stated in the attestation-tap comment at
/// `node_runtime.rs` ("a chain that has lost over a third of its stake does not spin") was not what the
/// code did. `new_state_transition` still licenses an attestation; it no longer licenses an unbounded
/// rate of them. Pinned by
/// `an_unreachable_supermajority_suppresses_even_with_a_deploy_bearing_parent`, and the cadence half by
/// `but_a_node_quiet_past_the_window_speaks_again`.
fn attestation_suppressed(
    nothing_to_finalize: bool,
    new_state_transition: bool,
    quorum_reachable: bool,
    cadence_due: bool,
    paced: bool,
) -> bool {
    if nothing_to_finalize {
        return true;
    }
    if quorum_reachable {
        // A deploy licenses a prompt attestation — it is the thing that needs the quorum — but an
        // attestation *without* a deploy is the #70 storm (C171): one empty block per remote block,
        // each an attestation that is in turn a remote block for the next. Gate that case on our own
        // quiet, so a caught-up node does not re-attest to its peers' attestations while a node left
        // behind still catches up.
        //
        // **The pace bound is the storm's, so it applies only to a proposal the node raised on its
        // own** (`paced`). A caller's explicit propose is not a reaction to a remote block, and
        // silencing it would break the documented contract that an explicit propose produces a block
        // on a `--no-autopropose` node (`tools/devnet.sh`; `tools/devnet-test.sh`'s step 5). This is
        // the pre-C171 rule for that case.
        if !paced {
            return false;
        }
        return !(new_state_transition || cadence_due);
    }
    !(new_state_transition && cadence_due)
}

/// Counting our own weight is the point. Without it, a validator whose attestation is exactly what would
/// complete the quorum can never be the one to add it — it is always "waiting for a supermajority" that
/// only its own message could create. With it, four validators at equal stake attest as soon as one of
/// them has moved (25% + 25% … 75% > 2/3), while a lone 10% validator still waits (10% + 0 < 2/3), which
/// is the anti-spam property the original rule was protecting.
fn attestation_reaches_supermajority(
    moving_stake: i128,
    own_stake: i128,
    total_stake: i128,
) -> bool {
    is_super_majority(moving_stake + own_stake, total_stake)
}

#[cfg(test)]
mod attestation_guard_tests {
    use super::attestation_reaches_supermajority;

    #[test]
    fn a_validator_counts_its_own_weight_toward_the_quorum() {
        // Four validators at 100 each: one other has moved, we hold 100 -> 200 of 400 is not > 2/3.
        assert!(!attestation_reaches_supermajority(100, 100, 400));
        // Two others have moved (the deploy-holder plus one) -> 300 of 400 is > 2/3, so we attest.
        assert!(attestation_reaches_supermajority(200, 100, 400));
        // A single node holding everything attests on its own.
        assert!(attestation_reaches_supermajority(0, 100, 100));
        // A lone 10% validator on a net where nobody has moved still waits.
        assert!(!attestation_reaches_supermajority(0, 10, 100));
        assert!(!attestation_reaches_supermajority(10, 10, 100));
    }
}

/// The two defects in the attestation guard, pinned as tests that are **red against this tree** rather
/// than described in prose — F4 (`moving_attestation_stake` cannot see recency) and F3
/// (`attestation_suppressed`'s quorum clause is short-circuited by a deploy-bearing parent).
///
/// Both were found by the #70 design pass on 2026-09-29; the issue's own comments do not contain
/// either, and three of them are stale against this tree. See the plan at
/// `~/.claude/plans/from-open-issues-and-nested-eclipse.md`.
#[cfg(test)]
mod attestation_suppression_tests {
    use super::{
        attestation_reaches_supermajority, attestation_suppressed, cadence_due,
        describe_attestation_decision, line_due, moving_attestation_stake, AttestationDecision,
    };
    use rchain_block_storage::dag::liveness;
    use rchain_models::block_hash::BlockHash;
    use rchain_models::block_metadata::BlockMetadata;
    use rchain_models::block_metadata::SlashSeverity;
    use rchain_models::validator::Validator;
    use rchain_shared::refined::{BlockHeight, NonNegI64, SeqNum};
    use std::collections::{BTreeMap, BTreeSet};

    fn validator(byte: u8) -> Validator {
        Validator::new([byte; 65])
    }

    /// A sender's latest message, at `block_num`. `latest_msgs` keeps one of these per sender
    /// indefinitely, so an old `block_num` is the whole of the staleness signal a guard could read.
    fn latest(sender: Validator, block_num: i64) -> BlockMetadata {
        BlockMetadata {
            block_hash: BlockHash::new([0u8; 32]),
            block_num: BlockHeight::try_from(block_num).unwrap(),
            sender,
            seq_num: SeqNum::zero(),
            justifications: BTreeSet::new(),
            bonds_map: BTreeMap::new(),
            validated: true,
            validation_failed: false,
            slashable: false,
            failure_cause: None,
            slash_severity: SlashSeverity::Unspecified,
            restore_attempts: 0,
            fringe: BTreeSet::new(),
            fringe_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
            member_of_fringe: None,
        }
    }

    fn bonds(entries: &[(Validator, i64)]) -> BTreeMap<Validator, NonNegI64> {
        entries
            .iter()
            .map(|(v, s)| (v.clone(), NonNegI64::try_from(*s).unwrap()))
            .collect()
    }

    /// **F4: a silent validator's stale message must not carry the quorum.**
    ///
    /// Three equal validators. `b` has gone silent, so its latest message sits 7 heights behind `c`'s —
    /// and both stay in `latest_msgs` forever. Only `c` is moving, so the quorum is
    /// `100 + our own 100 = 200` of 300, which is *exactly* 2/3 and therefore **not** a supermajority
    /// (`two_thirds_is_not_supermajority`, `sdk/src/consensus.rs:24`). Counting `b`'s stale message
    /// makes it 300 of 300 and has this node attest at every height — the spin #70 reports.
    ///
    /// Red before the recency filter — and still red if it is deleted from `live_weight_set`: the
    /// moving stake then reads 200, which `own_stake` turns into 300 of 300.
    #[test]
    fn a_silent_validators_stale_message_does_not_carry_the_quorum() {
        let (a, b, c) = (validator(1), validator(2), validator(3));
        let bonded = bonds(&[(a.clone(), 100), (b.clone(), 100), (c.clone(), 100)]);
        let justifications = [latest(b.clone(), 3), latest(c.clone(), 10)];
        let tip = BlockHeight::try_from(10).unwrap();

        let live = liveness::live_weight_set(
            &bonded,
            &liveness::latest_heights(
                justifications
                    .iter()
                    .map(|m| (m.sender.clone(), m.block_num)),
            ),
            tip,
            liveness::LIVENESS_WINDOW,
        );
        let moving = moving_attestation_stake(&live, &a);
        assert_eq!(
            moving, 100,
            "only `c` is moving — `b`'s message is 7 heights stale and must not count toward quorum"
        );
        assert!(
            !attestation_reaches_supermajority(moving, 100, 300),
            "200 of 300 is exactly 2/3, which is not a supermajority"
        );
    }

    /// **The quorum is measured against the whole bonded map, not the live set** — the difference
    /// between "an absent validator must not block the partition" and "an absent validator's stake
    /// stops counting", and the second is what would cost safety: a quorum over the live stake is
    /// trivially reached by any self-consistent subset, so under a partition each side would finalise
    /// its own view. Pinned as the arithmetic that separates the two readings.
    #[test]
    fn the_quorum_is_measured_against_the_whole_bonded_map_not_the_live_one() {
        // Three equal validators, one silent: the live set is 200, the bonded map 300.
        assert!(
            attestation_reaches_supermajority(100, 100, 200),
            "against the live set alone this is a quorum — which is the shape that loses safety"
        );
        assert!(
            !attestation_reaches_supermajority(100, 100, 300),
            "against the whole bonded map it is exactly 2/3, and 2/3 is not a supermajority"
        );
    }

    /// **F3: a deploy-bearing parent must not defeat the quorum clause.**
    ///
    /// Four validators at 100, one peer moving: `100 + our own 100 = 200` of 400, unreachable. A parent
    /// carrying deploys is the ordinary case on a chain with traffic, and it used to short-circuit the
    /// quorum test entirely, so this node attested at **every** height — the 276-blocks-in-a-minute
    /// run. This pins the contract stated in the attestation-tap comment at `node_runtime.rs`: *a chain
    /// that has lost over a third of its stake does not spin.*
    #[test]
    fn an_unreachable_supermajority_suppresses_even_with_a_deploy_bearing_parent() {
        assert!(
            attestation_suppressed(
                false, // nothing_to_finalize: there *is* an unfinalized transition to attest to
                true,  // new_state_transition: a parent carries deploys — the ordinary case
                false, // quorum_reachable: 200 of 400 is not a supermajority
                false, // cadence_due: we spoke within the window
                true,  // paced: the node's own proposal
            ),
            "the quorum is unreachable (200 of 400), so suppression must not be defeated by a \
             deploy-bearing parent"
        );
    }

    /// **The other half of F3, and the reason the repair is not a blanket suppress.**
    ///
    /// A hard suppress would trap liveness: no messages → no tip movement → no fresh justifications →
    /// the quorum stays unreachable forever, and a peer that came back is never seen. So a node quiet
    /// for longer than the window attests anyway, which bounds the spin to one block per window instead
    /// of one per height.
    #[test]
    fn but_a_node_quiet_past_the_window_speaks_again() {
        assert!(
            !attestation_suppressed(false, true, false, true, true),
            "a node that has been quiet past the window must attest even while the quorum is out of \
             reach, or a stalled chain can never discover that a peer returned"
        );
    }

    /// **The C171 fix.** A deploy-bearing block is attested promptly even with the quorum reachable — it
    /// is the thing that needs the quorum — but an attestation *without* a deploy is the #70 storm and
    /// is gated by our own quiet: a caught-up node withholds, a node left behind still speaks.
    #[test]
    fn a_reachable_quorum_attests_a_deploy_promptly_but_gates_the_storm() {
        // A deploy licenses an immediate attestation.
        assert!(!attestation_suppressed(false, true, true, false, true));
        // An attestation without a deploy, while caught up, is the storm and is withheld.
        assert!(attestation_suppressed(false, false, true, false, true));
        // But a node left behind still attests, so a stalled chain can discover a peer returned.
        assert!(!attestation_suppressed(false, false, true, true, true));
    }

    /// **And the pace bound is the storm's, not the caller's.** An explicit propose is not the node
    /// reacting to a remote block, so it attests whenever the quorum is reachable — the pre-C171 rule.
    /// This is the contract `tools/devnet.sh` documents ("an explicit `propose`/`POST /api/v1/propose`"
    /// creates a block on a `--no-autopropose` node) and `tools/devnet-test.sh`'s step 5 asserts
    /// (`POST /api/propose` answers 200). Silencing it is the regression this pins: an operator asking
    /// a caught-up node for a block gets one; a caught-up node asking *itself* (the `paced` arm above)
    /// is the storm and is withheld.
    #[test]
    fn an_explicit_propose_is_not_paced_by_the_storm_bound() {
        assert!(
            !attestation_suppressed(false, false, true, false, false),
            "a caller's explicit propose must not be silenced by the C171 pace bound"
        );
    }

    /// And an idle chain still produces nothing: suppression outranks every other term.
    #[test]
    fn an_idle_chain_suppresses_whatever_else_is_true() {
        assert!(attestation_suppressed(true, true, true, true, true));
    }

    /// **The diagnostic names every input, and fires on a change rather than on a height.** The tip is
    /// kept out of the rendered line, so a validator that stays withheld for the same reason logs once and
    /// then once per interval, not once per block.
    #[test]
    fn the_attestation_line_names_every_input_and_fires_on_a_change() {
        let (a, b) = (validator(1), validator(2));
        let decision = |suppressed: bool| {
            describe_attestation_decision(&AttestationDecision {
                nothing_to_finalize: false,
                new_state_transition: true,
                quorum_reachable: false,
                cadence_due: false,
                paced: true,
                round_senders: BTreeSet::from([&a, &b]),
                blocked_attempts: 2,
                escape: false,
                suppressed,
            })
        };
        let withheld = decision(true);
        for field in [
            "withheld",
            "nothing_to_finalize=false",
            "new_state_transition=true",
            "quorum_reachable=false",
            "cadence_due=false",
            "paced=true",
            "round=[01010101 02020202]",
            "blocked_attempts=2",
            "escape=false",
        ] {
            assert!(withheld.contains(field), "{field} missing from: {withheld}");
        }

        let (line, tip) = (
            std::sync::Mutex::new(None),
            std::sync::atomic::AtomicI64::new(i64::MIN),
        );
        let attestation_line_due = |l: &str, t: i64| line_due(&line, &tip, l, t);
        assert!(
            attestation_line_due(&withheld, 1_000),
            "the first line is logged"
        );
        assert!(
            !attestation_line_due(&withheld, 1_001),
            "the same line one height later is not"
        );
        assert!(
            attestation_line_due(&decision(false), 1_002),
            "a changed decision is logged at once"
        );
        assert!(
            attestation_line_due(&decision(false), 1_102),
            "an unchanged line is logged again after the interval"
        );
    }

    /// The pace bound is read from the DAG, so it needs no new state: our own latest message's height
    /// against the tip. Nothing from us at all counts as due — there is no quiet to have broken.
    #[test]
    fn the_cadence_is_read_from_our_own_latest_message() {
        let (a, b) = (validator(1), validator(2));
        let tip = BlockHeight::try_from(20).unwrap();

        let at = |v: &Validator, h: i64| {
            BTreeMap::from([(v.clone(), BlockHeight::try_from(h).unwrap())])
        };

        assert!(
            cadence_due(&BTreeMap::new(), &a, tip),
            "nothing from us: speak"
        );
        assert!(
            !cadence_due(&at(&a, 19), &a, tip),
            "we spoke one height ago"
        );
        assert!(
            cadence_due(&at(&a, 20 - liveness::LIVENESS_WINDOW - 1), &a, tip),
            "quiet past the window"
        );
        // Someone else's message is not ours, and must not reset our cadence.
        assert!(cadence_due(&at(&b, 20), &a, tip));
    }
}

#[cfg(test)]
mod equivocation_injection_tests {
    use super::equivocation_twin;
    use super::ProposeSource;
    use crate::proto_util::hash_block;
    use crate::validate::block_signature;
    use crate::validator_identity::ValidatorIdentity;
    use rchain_models::block::state_hash::StateHash;
    use rchain_models::block_hash::BlockHash;
    use rchain_models::casper::protocol::casper_message::{BlockMessage, RholangState};
    use rchain_models::validator::Validator;
    use rchain_shared::refined::{BlockHeight, SeqNum};

    fn identity() -> ValidatorIdentity {
        ValidatorIdentity::from_hex(
            "67e56582298859ddae725f972992a07c6c4fb9f62a8fff58ce3ca926a1063530",
        )
        .expect("a known secp256k1 key")
    }

    /// A block signed by the identity, at a real sequence number.
    fn block(id: &ValidatorIdentity) -> BlockMessage {
        let base = BlockMessage {
            version: 1,
            shard_id: "root".to_string(),
            block_hash: BlockHash::new([0u8; 32]),
            block_number: BlockHeight::try_from(7).expect("a height"),
            sender: Validator::from_slice(id.public_key.bytes()),
            seq_num: SeqNum::try_from(3).expect("a sequence number"),
            pre_state_hash: StateHash::new([1u8; 32]),
            post_state_hash: StateHash::new([2u8; 32]),
            justifications: Vec::new(),
            bonds: std::collections::BTreeMap::new(),
            rejected_deploys: std::collections::BTreeSet::new(),
            rejected_blocks: std::collections::BTreeSet::new(),
            rejected_senders: std::collections::BTreeSet::new(),
            state: RholangState::default(),
            sig_algorithm: "secp256k1".to_string(),
            sig: Vec::new(),
            timestamp: 1_700_000_000_000,
        };
        id.sign_block(&base).expect("signing with a real key")
    }

    /// **The twin is the same block in every way the protocol reads, and a different block in the one
    /// way that makes it an equivocation.**
    ///
    /// The injection is only a *measurement* of the equivocation rules if the second block is otherwise
    /// beyond reproach: same sender, same sequence number, same state claim, a valid content-addressed
    /// hash and a valid signature — and a different hash, which is the whole offence. A twin that
    /// differed anywhere else would be refused for some other reason and the arm would measure nothing.
    ///
    /// The field-by-field assertion below is that claim spelled out: everything equal but `timestamp`,
    /// `block_hash` and `sig`.
    #[test]
    fn the_injected_twin_differs_only_where_an_equivocation_demands() {
        let id = identity();
        let first = block(&id);
        let twin = equivocation_twin(&first, &id).expect("the twin signs");

        assert_ne!(
            first.block_hash, twin.block_hash,
            "the offence: two distinct signed blocks, so the hashes must differ"
        );
        assert_eq!(
            first.sender, twin.sender,
            "same sender — that is what makes it one validator's equivocation"
        );
        assert_eq!(first.seq_num, twin.seq_num, "and the same sequence number");
        assert_eq!(first.block_number, twin.block_number);
        assert_eq!(first.pre_state_hash, twin.pre_state_hash);
        assert_eq!(first.post_state_hash, twin.post_state_hash);
        assert_eq!(first.justifications, twin.justifications);
        assert_eq!(first.state, twin.state);

        // Both are *valid*: a real content-addressed hash and a signature that verifies against the
        // sender. Without this the arm measures the wrong refusal.
        assert_eq!(twin.block_hash, hash_block(&twin));
        assert!(block_signature(&twin));
        assert_eq!(first.block_hash, hash_block(&first));
        assert!(block_signature(&first));

        assert_eq!(
            twin.timestamp,
            first.timestamp + 1,
            "the one field the twin moves — informational, and covered by the hash"
        );
    }

    /// **The injection reaches the wire and the store** (W-N9-04).
    ///
    /// The test above pins what the twin *is*; this pins that a `Proposer` with the flag armed emits
    /// one. The row: with `--equivocation-injection` the node signs a second, content-distinct block at
    /// the same `(sender, seq_num)` and broadcasts it — the safety split reached from the honest
    /// signing path, on an operator's word. That flag is outside the bounded-adversary model (the
    /// operator is trusted), which is why this is a reverse arm rather than an in-model finding.
    ///
    /// **Two assertions, because announcing a twin is not enough.** A peer answers a hash announcement
    /// by *asking for the block*, so a twin announced and not held is one nobody ever receives — which
    /// is what the flag's first version measured (45 twins broadcast, no refusal anywhere, no slash).
    /// The effect must see both blocks and the store must hold the twin's body.
    ///
    /// Falsified by dropping the `put_block` write: the store assertion reddens while the effect still
    /// sees two blocks, which is the defect the comment at the write names.
    #[tokio::test]
    async fn the_injection_puts_a_second_block_on_the_wire_and_in_the_store() {
        use crate::multi_parent_casper::ValidateError;
        use rchain_block_storage::block_store::BlockStore;
        use rchain_block_storage::dag::codecs::{BlockHashCodec, BlockMessageCodec};
        use rchain_shared::log::NopLog;
        use rchain_shared::store::InMemoryKeyValueStore;
        use rchain_shared::typed_store::KeyValueTypedStoreCodec;
        use std::sync::atomic::AtomicU64;
        use std::sync::Arc;

        let store: BlockStore = Arc::new(KeyValueTypedStoreCodec::new(
            Arc::new(tokio::sync::Mutex::new(Box::new(
                InMemoryKeyValueStore::default(),
            ))),
            Arc::new(BlockHashCodec),
            Arc::new(BlockMessageCodec),
        ));
        let id = identity();
        let created = block(&id);

        let get_seq: Arc<dyn Fn(Validator) -> super::BoxFuture<i64> + Send + Sync> =
            Arc::new(|_v| Box::pin(async { 0 }));
        let check_active: Arc<
            dyn Fn(&ValidatorIdentity) -> super::BoxFuture<Result<bool, String>> + Send + Sync,
        > = Arc::new(|_id| Box::pin(async { Ok(true) }));
        let create: Arc<
            dyn Fn(
                    &ValidatorIdentity,
                    super::ProposeSource,
                ) -> super::BoxFuture<Result<super::BlockCreatorResult, String>>
                + Send
                + Sync,
        > = {
            let created = created.clone();
            Arc::new(move |_v, _s| {
                let created = created.clone();
                Box::pin(async move { Ok(super::BlockCreatorResult::Created(created)) })
            })
        };
        let validate: Arc<
            dyn Fn(&BlockMessage) -> super::BoxFuture<Result<(), ValidateError>> + Send + Sync,
        > = Arc::new(|_b| Box::pin(async { Ok(()) }));
        let seen: Arc<std::sync::Mutex<Vec<BlockMessage>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let effect: Arc<dyn Fn(&BlockMessage) -> super::BoxFuture<()> + Send + Sync> = {
            let seen = seen.clone();
            Arc::new(move |b| {
                seen.lock().unwrap().push(b.clone());
                Box::pin(async {})
            })
        };

        let proposer = super::Proposer::new(
            get_seq,
            check_active,
            create,
            validate,
            effect,
            id.clone(),
            Arc::new(NopLog),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
            true, // **the flag under test**
            store.clone(),
        );

        let (tx, _rx) = tokio::sync::oneshot::channel();
        let (_result, block_opt) = proposer
            .propose(ProposeSource::Explicit { is_async: false }, tx)
            .await
            .expect("the proposal runs");
        let emitted = block_opt.expect("a block was created");

        let broadcast = seen.lock().unwrap().clone();
        assert_eq!(
            broadcast.len(),
            2,
            "**the injection put a second block on the wire**: the block and its twin, both emitted \
             to the propose effect (W-N9-04)"
        );
        assert_eq!(
            broadcast[0].block_hash, emitted.block_hash,
            "the real block first"
        );
        let twin = &broadcast[1];
        assert_eq!(
            twin.sender, emitted.sender,
            "the twin is the same validator's"
        );
        assert_eq!(twin.seq_num, emitted.seq_num, "at the same sequence number");
        assert_ne!(twin.block_hash, emitted.block_hash, "and a different block");

        // And the *body* is held, which is what makes the announcement answerable: a peer asks for
        // the block by hash, so a twin nobody stored is a twin nobody receives.
        let stored = store
            .get(&[twin.block_hash.clone()])
            .await
            .expect("the store answers");
        assert_eq!(
            stored[0].as_ref().map(|b| b.block_hash.clone()),
            Some(twin.block_hash.clone()),
            "the twin's body is in the block store, not only its hash on the wire"
        );
    }
}

/// **A twin of `block`** — the same block with a different `timestamp`, re-hashed and re-signed by the
/// same identity.
///
/// This is the *only* difference between the two blocks, and that is deliberate. The timestamp is a
/// header field the block hash covers, so the twin is a distinct block; it is also **not a consensus
/// input** — no rule reads it, it is exposed to rholang as `rho:block:data` for applications — so the
/// twin passes every check the original passes. That matters for what the injection measures: a peer's
/// refusal of the second block must be the equivocation gate and nothing else, and a twin that were
/// invalid in any other way would leave that ambiguous.
///
/// It exists for the devnet arm of A2 (#150): the fault it produces is the one no honest node
/// produces, so it cannot be produced by driving the node honestly — it has to be asked for, here,
/// behind a flag whose own documentation says it is self-harm on a real network.
fn equivocation_twin(block: &BlockMessage, id: &ValidatorIdentity) -> Result<BlockMessage, String> {
    let twin = BlockMessage {
        timestamp: block.timestamp + 1,
        ..block.clone()
    };
    id.sign_block(&twin).map_err(|e| e.to_string())
}

/// **A slash the proposer is about to attach**: how much the offence takes (AUDIT C199), and — when
/// the offence is an **equivocation** rather than a failed block — the evidence a receiver needs to
/// check it (AUDIT C200).
///
/// The two kinds are one type because they travel the same way: the tier sizes the confiscation in
/// both, and the evidence is what makes the second kind *justifiable* by a node that never saw the
/// block the proposer is complaining about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposedSlash {
    pub severity: SlashSeverity,
    /// The offender's second block at a `(sender, seq_num)` the DAG already holds a different block
    /// for, serialized. `None` for the metadata-justified kind.
    pub evidence: Option<Vec<u8>>,
}

/// The validators a proposer may slash: bonded, and the sender of a justification whose failure is
/// attributable to the block rather than to this node's inability to replay it.
///
/// `validation_failed` alone is too broad - it is also set when this node cannot run the replay at all,
/// which says nothing about the sender (#70). `BlockMetadata::slashable` is the narrower, attributable
/// signal.
///
/// **The rule itself is not defined here** (AUDIT C110). It is
/// [`crate::validate::slashable_senders`], and it is shared with the validators who *check* the slash
/// — which is the point: while the rule existed only on this side, a proposer could name any bonded
/// validator and the receiving nodes had nothing to compare the name against. What stays local to the
/// proposer is the `bonded` filter, because "only take stake from someone who has some" is a policy
/// about who is worth slashing, not part of what makes a slash justified.
fn slashable_offenders(
    justifications: &[rchain_models::block_metadata::BlockMetadata],
    bonded: &BTreeSet<Validator>,
) -> BTreeMap<Validator, ProposedSlash> {
    crate::validate::slashable_senders(justifications)
        .into_iter()
        .filter(|(sender, _)| bonded.contains(sender))
        .map(|(sender, severity)| {
            (
                sender,
                ProposedSlash {
                    severity,
                    evidence: None,
                },
            )
        })
        .collect()
}

/// Fold **the equivocations this node has itself seen** into the set it is about to slash (AUDIT C200).
///
/// A validator that signed two blocks at one `(sender, seq_num)` has committed the one offence no
/// honest node can commit, so it is slashed whatever else is true — at the harshest tier, and with the
/// evidence **attached**, so that every other node checks the offence against its own DAG rather than
/// trusting this one. Refusing such a block was already free of charge; this is the half that costs the
/// offender.
///
/// The `bonded` filter is the same policy the metadata arm applies: only take stake from someone who
/// has some. An insert over a metadata-justified entry is the right direction — `Malicious` is the
/// harshest tier there is, so the fold can only harden the sentence, never soften it.
fn add_recorded_equivocations(
    to_slash: &mut BTreeMap<Validator, ProposedSlash>,
    recorded: Vec<(Validator, Vec<u8>)>,
    bonded: &BTreeSet<Validator>,
) {
    for (offender, evidence) in recorded {
        if !bonded.contains(&offender) {
            continue;
        }
        to_slash.insert(
            offender,
            ProposedSlash {
                severity: SlashSeverity::Malicious,
                evidence: Some(evidence),
            },
        );
    }
}

#[cfg(test)]
mod slashable_offenders_tests {
    use super::add_recorded_equivocations;
    use super::slashable_offenders;
    use super::ProposedSlash;
    use rchain_models::block_hash::BlockHash;
    use rchain_models::block_metadata::BlockMetadata;
    use rchain_models::block_metadata::SlashSeverity;
    use rchain_models::validator::Validator;
    use rchain_shared::refined::{BlockHeight, SeqNum};
    use std::collections::{BTreeMap, BTreeSet};

    fn meta(sender: Validator, validation_failed: bool, slashable: bool) -> BlockMetadata {
        BlockMetadata {
            block_hash: BlockHash::new([0u8; 32]),
            block_num: BlockHeight::try_from(1).unwrap(),
            sender,
            seq_num: SeqNum::zero(),
            justifications: BTreeSet::new(),
            bonds_map: BTreeMap::new(),
            validated: true,
            validation_failed,
            slashable,
            failure_cause: None,
            // A slashable record always carries a real tier (`mark_failed` sets it from the status), so
            // the fixture does too; a test that wants a different one overwrites it.
            slash_severity: if slashable {
                SlashSeverity::Malicious
            } else {
                SlashSeverity::Unspecified
            },
            restore_attempts: 0,
            fringe: BTreeSet::new(),
            fringe_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
            member_of_fringe: None,
        }
    }

    /// The metadata-justified slash this rule produces for `v`: the tier the record carries, and no
    /// evidence — the equivocation arm is a different source (AUDIT C200).
    fn slashed(tier: SlashSeverity) -> ProposedSlash {
        ProposedSlash {
            severity: tier,
            evidence: None,
        }
    }

    fn rule(
        justifications: &[BlockMetadata],
        bonded: &[Validator],
    ) -> BTreeMap<Validator, ProposedSlash> {
        slashable_offenders(justifications, &bonded.iter().copied().collect())
    }

    #[test]
    fn a_bonded_sender_of_a_disagreeing_block_is_slashable() {
        let v = Validator::new([1u8; 65]);
        assert_eq!(
            rule(&[meta(v, true, true)], &[v]),
            BTreeMap::from([(v, slashed(SlashSeverity::Malicious))])
        );
    }

    /// The point of the narrowing: a block this node could not replay says nothing about its sender, so a
    /// local problem must not cost an honest validator its stake.
    #[test]
    fn a_replay_that_could_not_run_is_not_slashable() {
        let v = Validator::new([1u8; 65]);
        assert!(
            rule(&[meta(v, true, false)], &[v]).is_empty(),
            "an unattributable failure must not cost the sender its stake"
        );
    }

    #[test]
    fn only_bonded_senders_are_slashable() {
        let bonded = Validator::new([1u8; 65]);
        let observer = Validator::new([2u8; 65]);
        assert_eq!(
            rule(
                &[meta(bonded, true, true), meta(observer, true, true)],
                &[bonded]
            ),
            BTreeMap::from([(bonded, slashed(SlashSeverity::Malicious))])
        );
    }

    /// **A validator answers for the worst thing it did** (AUDIT C199). Two failed blocks from one
    /// sender — a tenth and a quarter — cost a quarter: not the sum, and not the milder one. The order
    /// of the justifications must not matter, since two nodes can see them in different orders.
    #[test]
    fn a_validator_answers_for_its_worst_tier() {
        let v = Validator::new([1u8; 65]);
        let mut mild = meta(v, true, true);
        mild.slash_severity = SlashSeverity::HonestMistake;
        let mut bad = meta(v, true, true);
        bad.slash_severity = SlashSeverity::Misdemeanour;

        let expected = BTreeMap::from([(v, slashed(SlashSeverity::Misdemeanour))]);
        assert_eq!(rule(&[mild.clone(), bad.clone()], &[v]), expected);
        assert_eq!(rule(&[bad, mild], &[v]), expected);
    }

    /// **The equivocation arm, and what it must carry** (AUDIT C200).
    ///
    /// A recorded equivocation is slashed at the harshest tier **with its evidence attached** — the
    /// evidence is the whole point: a receiver refuses a slash it cannot check, so a slash without one
    /// would be worse than useless. Red before: `to_slash` held only the metadata arm, so an
    /// equivocation was refused and never punished.
    #[test]
    fn a_recorded_equivocation_is_slashed_with_its_evidence() {
        let v = Validator::new([1u8; 65]);
        let evidence = vec![0xab, 0xcd];
        let mut to_slash = BTreeMap::new();
        add_recorded_equivocations(
            &mut to_slash,
            vec![(v, evidence.clone())],
            &[v].into_iter().collect(),
        );
        assert_eq!(
            to_slash,
            BTreeMap::from([(
                v,
                ProposedSlash {
                    severity: SlashSeverity::Malicious,
                    evidence: Some(evidence),
                }
            )])
        );
    }

    /// An equivocation by an **unbonded** key costs nothing, exactly as the metadata arm: there is no
    /// stake to take, and a slash naming a validator with no bond is one every receiver refuses.
    #[test]
    fn an_equivocation_by_an_unbonded_sender_is_not_slashed() {
        let v = Validator::new([1u8; 65]);
        let mut to_slash = BTreeMap::new();
        add_recorded_equivocations(&mut to_slash, vec![(v, vec![1, 2, 3])], &BTreeSet::new());
        assert!(to_slash.is_empty());
    }

    /// The fold can only **harden** a sentence: a validator already facing a milder metadata-justified
    /// tier answers for the equivocation instead, at `Malicious`.
    #[test]
    fn an_equivocation_hardens_a_milder_metadata_tier() {
        let v = Validator::new([1u8; 65]);
        let mut mild = meta(v, true, true);
        mild.slash_severity = SlashSeverity::HonestMistake;
        let mut to_slash = rule(&[mild], &[v]);
        add_recorded_equivocations(
            &mut to_slash,
            vec![(v, vec![7])],
            &[v].into_iter().collect(),
        );
        assert_eq!(
            to_slash[&v].severity,
            SlashSeverity::Malicious,
            "the equivocation is the worst thing this validator did, at either order"
        );
        assert_eq!(to_slash[&v].evidence, Some(vec![7]));
    }
}

#[cfg(test)]
mod active_validator_tests {
    use super::*;
    use rchain_block_storage::dag::dag_storage::BlockDagStorage;
    use rchain_block_storage::dag::representation::DagRepresentation;
    use rchain_models::block_metadata::BlockMetadata;
    use rchain_models::validator::Validator;
    use rchain_shared::refined::{BlockHeight, NonNegI64, SeqNum};
    use std::collections::{BTreeMap, BTreeSet};

    /// A DAG with two blocks: the genesis at height 1, whose `bonds_map` is the validator set the chain
    /// started with, and a newer block at height 9, whose map includes a validator that has bonded since.
    /// An empty fringe, which is the state a chain is in while its fringe is stalled.
    struct TwoHeightDag {
        genesis: BlockHash,
        newest: BlockHash,
        newcomer: Validator,
    }

    fn meta(block_hash: BlockHash, bonds: BTreeMap<Validator, NonNegI64>) -> BlockMetadata {
        BlockMetadata {
            block_hash,
            block_num: BlockHeight::try_from(1).unwrap(),
            sender: Validator::new([9u8; 65]),
            seq_num: SeqNum::zero(),
            justifications: BTreeSet::new(),
            bonds_map: bonds,
            validated: true,
            validation_failed: false,
            slashable: false,
            failure_cause: None,
            slash_severity: SlashSeverity::Unspecified,
            restore_attempts: 0,
            fringe: BTreeSet::new(),
            fringe_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
            member_of_fringe: None,
        }
    }

    #[async_trait::async_trait]
    impl BlockDagStorage for TwoHeightDag {
        async fn get_representation(&self) -> Arc<DagRepresentation> {
            Arc::new(DagRepresentation {
                dag_set: Arc::new(BTreeSet::new()),
                child_map: Arc::new(BTreeMap::new()),
                height_map: Arc::new(BTreeMap::from([
                    (
                        BlockHeight::try_from(1).unwrap(),
                        BTreeSet::from([self.genesis]),
                    ),
                    (
                        BlockHeight::try_from(9).unwrap(),
                        BTreeSet::from([self.newest]),
                    ),
                ])),
                dag_message_state: rchain_block_storage::dag::message_state::DagMessageState::empty(
                ),
            })
        }

        async fn lookup(&self, block_hash: &BlockHash) -> Result<Option<BlockMetadata>, String> {
            if *block_hash == self.genesis {
                // The set the chain started with: the newcomer is not in it.
                Ok(Some(meta(self.genesis, BTreeMap::new())))
            } else if *block_hash == self.newest {
                Ok(Some(meta(
                    self.newest,
                    BTreeMap::from([(self.newcomer, NonNegI64::try_from(100).unwrap())]),
                )))
            } else {
                Ok(None)
            }
        }

        async fn insert(&self, _m: BlockMetadata, _b: BlockMessage) -> Result<(), String> {
            todo!("not on the is_active_validator path")
        }
        async fn lookup_by_deploy_id(&self, _d: &DeployId) -> Result<Option<BlockHash>, String> {
            todo!("not on the is_active_validator path")
        }
        async fn add_deploy(&self, _d: SignedDeployData) -> Result<(), String> {
            todo!("not on the is_active_validator path")
        }
        async fn pooled_deploys(&self) -> Result<BTreeMap<DeployId, SignedDeployData>, String> {
            todo!("not on the is_active_validator path")
        }
        async fn contains_deploy_in_pool(&self, _d: &DeployId) -> Result<bool, String> {
            todo!("not on the is_active_validator path")
        }
    }

    /// **A validator that bonded after the chain started is active, and may propose.**
    ///
    /// This is the whole of the "only the first validator ever proposes" symptom (#70). The check used to
    /// read the bond map of the *lowest* height in the DAG - the genesis's - so a validator admitted later
    /// was never active here, no matter what the chain's own active set said. Since the fringe cannot
    /// advance without the validators it expects to speak, the node stayed read-only forever, and the
    /// network could not actually grow its validator set even though every admission step succeeded.
    #[tokio::test]
    async fn a_validator_bonded_after_the_genesis_is_active() {
        let newcomer = Validator::new([1u8; 65]);
        let dag: Arc<dyn BlockDagStorage> = Arc::new(TwoHeightDag {
            genesis: BlockHash::new([0u8; 32]),
            newest: BlockHash::new([7u8; 32]),
            newcomer,
        });
        assert!(
            is_active_validator(&dag, &newcomer).await.unwrap(),
            "the newest block's view of the active set is what counts, not the genesis's"
        );
        let stranger = Validator::new([2u8; 65]);
        assert!(
            !is_active_validator(&dag, &stranger).await.unwrap(),
            "and a validator in no block's set is still not active"
        );
    }
}

/// **An in-process network of validators with no autopropose**, running the proposer's own decisions —
/// the round gate and its escape, the pre-state fringe, the attestation guard, the epoch trigger — over
/// one shared `DagMessageState`. Every block reaches every live validator at once; each arrival is one
/// attestation-tap attempt (one per sender per height, as `attest_warranted` allows); a deploy is one
/// attempt on the validator it was sent to and stays pooled until a block of that validator's carries it.
/// The network runs until no attempt anywhere would produce a block, which is what "quiet" means here.
///
/// What it does not model: delivery delay and reordering, and the store reads `create_block` makes.
#[cfg(test)]
mod quiet_chain_tests {
    use super::{attestation_inputs, attestation_suppressed, work_age_floor, ATTESTATION_HORIZON};
    use rchain_block_storage::dag::finalizer::Message;
    use rchain_block_storage::dag::liveness;
    use rchain_block_storage::dag::message_map;
    use rchain_block_storage::dag::message_state::DagMessageState;
    use rchain_shared::refined::{BlockHeight, NonNegI64, SeqNum};
    use std::collections::{BTreeMap, BTreeSet, VecDeque};

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum View {
        /// The guard reads the round snapshot (the parents) — the rule on dev.
        Round,
        /// What is left to finalise is read from `latest_msgs`, and who is moving from the parents: the
        /// first cut of C209, which a live run refuted (a deploy sent to a validator that did not sign the
        /// genesis is never finalised).
        SeenWork,
        /// Every guard input is read from everything the node has seen (`latest_msgs`).
        Seen,
    }

    #[derive(Clone, Copy, Debug)]
    enum Step {
        Deploy(usize),
        Kill(usize),
        Revive(usize),
        /// Bond a validator into every later block's bond map (it has not spoken yet).
        Bond(usize, i64),
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Ev {
        /// `--propose-on-deploy`: the deploy's own attempt.
        Deploy,
        /// The attest-on-new-blocks tap.
        Tap,
    }

    struct Cfg {
        stakes: Vec<i64>,
        genesis_sender: u8,
        view: View,
        epoch: i64,
        cap: usize,
    }

    #[derive(Debug)]
    #[allow(dead_code)] // `tip` and `escapes` are read through `Debug`, in the failure messages.
    struct Outcome {
        blocks: usize,
        deploys: usize,
        finalized: usize,
        stranded: usize,
        tip: i64,
        hit_cap: bool,
        escapes: usize,
        /// The oldest an unfinalised deploy-bearing block got, in heights behind the tip.
        max_age: i64,
    }

    fn seen(ms: &BTreeSet<Message<u64, u8>>) -> BTreeSet<u64> {
        ms.iter().flat_map(|m| m.seen.iter().copied()).collect()
    }

    fn run(cfg: &Cfg, script: &[Step]) -> Outcome {
        let mut bonds: BTreeMap<u8, NonNegI64> = cfg
            .stakes
            .iter()
            .enumerate()
            .map(|(i, s)| (i as u8, NonNegI64::try_from(*s).unwrap()))
            .collect();
        let n_max = 16usize;
        let mut st: DagMessageState<u64, u8> = DagMessageState::empty();
        let genesis = st.create_message(
            0,
            BlockHeight::zero(),
            cfg.genesis_sender,
            SeqNum::zero(),
            bonds.clone(),
            &BTreeSet::new(),
        );
        st.insert_msg_mut(&genesis);
        let mut next_id = 1u64;
        // Genesis carries the genesis deploys, so it is deploy-bearing to the guard, as on a node.
        let mut with_deploy: BTreeSet<u64> = BTreeSet::from([0]);
        let mut deploy_blocks: Vec<u64> = Vec::new();
        let mut answered: BTreeMap<(usize, u8), i64> = BTreeMap::new();
        let mut pool = vec![0usize; n_max];
        let mut dead: BTreeSet<usize> = BTreeSet::new();
        let mut waited = vec![0i64; n_max];
        let mut blocked_since: Vec<Option<usize>> = vec![None; n_max];
        let (mut blocks, mut escapes, mut deploys, mut hit_cap) = (0usize, 0usize, 0usize, false);
        let mut max_age = 0i64;

        for (step, s) in script.iter().enumerate() {
            let mut queue: VecDeque<(usize, Ev)> = VecDeque::new();
            match *s {
                Step::Deploy(v) => {
                    deploys += 1;
                    pool[v] += 1;
                    queue.push_back((v, Ev::Deploy));
                }
                Step::Kill(v) => {
                    dead.insert(v);
                }
                Step::Revive(v) => {
                    dead.remove(&v);
                    answered.retain(|(u, _), _| *u != v);
                    waited[v] = 0;
                    blocked_since[v] = None;
                }
                Step::Bond(v, stake) => {
                    bonds.insert(v as u8, NonNegI64::try_from(stake).unwrap());
                }
            }
            while let Some((v, ev)) = queue.pop_front() {
                if blocks >= cfg.cap {
                    hit_cap = true;
                    break;
                }
                if dead.contains(&v) {
                    continue;
                }
                let me = v as u8;
                let escape = if st.has_advanced_past_the_round(&me) {
                    waited[v] += 1;
                    // The stall bound: a step is a later moment in wall-clock time than the one before.
                    let since = *blocked_since[v].get_or_insert(step);
                    // Mirror the production elapsed-stall licence (#379), not the old
                    // attempt-count shortcut. A later step represents elapsed time here.
                    if since >= step {
                        continue;
                    }
                    escapes += 1;
                    true
                } else {
                    waited[v] = 0;
                    blocked_since[v] = None;
                    false
                };
                let parents = if escape {
                    st.parents_for_new_block_escaping(&me)
                } else {
                    st.parents_for_new_block()
                };
                let tip = parents.iter().map(|m| m.height).max().unwrap();
                let fringe = message_map::latest_fringe(&st.msg_map, &parents);
                let latest: BTreeSet<Message<u64, u8>> = st.latest_msgs.values().cloned().collect();
                let work_view = match cfg.view {
                    View::Round => &parents,
                    View::SeenWork | View::Seen => &latest,
                };
                let viewed = match cfg.view {
                    View::Round | View::SeenWork => &parents,
                    View::Seen => &latest,
                };
                // The movement inputs are read from one view, and recency is measured from its tip.
                let seen_tip = viewed.iter().map(|m| m.height).max().unwrap();
                let work_tip = work_view.iter().map(|m| m.height).max().unwrap();
                let conflict: BTreeSet<u64> = seen(work_view)
                    .difference(&seen(&fringe))
                    .copied()
                    .collect();
                let nothing_to_finalize = conflict.iter().all(|h| !with_deploy.contains(h));
                let oldest_pending = conflict
                    .iter()
                    .filter(|h| with_deploy.contains(h))
                    .map(|h| st.msg_map[h].height)
                    .min();
                if let Some(o) = oldest_pending {
                    max_age = max_age.max(liveness::heights_behind(work_tip, o));
                }
                let (nothing_to_finalize, new_state_transition) = match cfg.view {
                    View::Round => (
                        nothing_to_finalize,
                        parents.iter().any(|p| with_deploy.contains(&p.id)),
                    ),
                    View::SeenWork | View::Seen => attestation_inputs(oldest_pending, work_tip),
                };
                let live = liveness::live_weight_set(
                    &bonds,
                    &liveness::latest_heights(viewed.iter().map(|m| (m.sender, m.height))),
                    seen_tip,
                    liveness::LIVENESS_WINDOW,
                );
                let total: i128 = bonds.values().map(|s| i128::from(i64::from(*s))).sum();
                let own: i128 = bonds.get(&me).map_or(0, |s| i128::from(i64::from(*s)));
                let moving: i128 = live
                    .iter()
                    .filter(|(s, _)| **s != me)
                    .map(|(_, s)| i128::from(i64::from(*s)))
                    .sum();
                let quorum_reachable = 3 * (moving + own) > 2 * total;
                let cadence_due = match viewed.iter().find(|m| m.sender == me) {
                    Some(mine) => {
                        liveness::heights_behind(seen_tip, mine.height) > liveness::LIVENESS_WINDOW
                    }
                    None => true,
                };
                let next = i64::from(tip) + 1;
                let change_epoch = cfg.epoch > 0 && next % cfg.epoch == 0;
                let has_deploy = pool[v] > 0;
                let suppressed = attestation_suppressed(
                    nothing_to_finalize,
                    new_state_transition,
                    quorum_reachable,
                    cadence_due,
                    ev != Ev::Deploy,
                );
                if !has_deploy && !change_epoch && suppressed {
                    continue;
                }
                let seq = parents
                    .iter()
                    .find(|m| m.sender == me)
                    .map(|m| m.sender_seq + NonNegI64::one())
                    .unwrap_or_else(SeqNum::zero);
                let id = next_id;
                next_id += 1;
                let msg =
                    st.create_message(id, tip + NonNegI64::one(), me, seq, bonds.clone(), &parents);
                st.insert_msg_mut(&msg);
                blocks += 1;
                if has_deploy || change_epoch {
                    with_deploy.insert(id);
                }
                if has_deploy {
                    deploy_blocks.extend(std::iter::repeat_n(id, pool[v]));
                    pool[v] = 0;
                }
                let height = i64::from(msg.height);
                for u in 0..n_max {
                    if u == v || !bonds.contains_key(&(u as u8)) {
                        continue;
                    }
                    let last = answered.get(&(u, me)).copied();
                    if last.map_or(true, |l| height > l) {
                        answered.insert((u, me), height);
                        queue.push_back((u, Ev::Tap));
                    }
                }
            }
            if hit_cap {
                break;
            }
        }
        let latest: BTreeSet<_> = st.latest_msgs.values().cloned().collect();
        let fin_seen = seen(&message_map::latest_fringe(&st.msg_map, &latest));
        Outcome {
            blocks,
            deploys,
            finalized: deploy_blocks
                .iter()
                .filter(|d| fin_seen.contains(d))
                .count(),
            stranded: pool.iter().sum(),
            tip: latest.iter().map(|m| i64::from(m.height)).max().unwrap(),
            hit_cap,
            escapes,
            max_age,
        }
    }

    fn cfg(stakes: &[i64], genesis_sender: u8, view: View) -> Cfg {
        Cfg {
            stakes: stakes.to_vec(),
            genesis_sender,
            view,
            epoch: 10,
            cap: 3000,
        }
    }

    /// The scenarios a no-autopropose net with a changing validator set has to survive, each with a
    /// supermajority still live: one validator, then several, then validators lost, returning, and joining.
    fn live_quorum_scenarios() -> Vec<(&'static str, Vec<i64>, Vec<Step>)> {
        use Step::*;
        vec![
            (
                "two validators, one deploy",
                vec![100, 100],
                vec![Deploy(0)],
            ),
            (
                "three, every deploy to one",
                vec![100, 100, 50],
                vec![Deploy(0), Deploy(0), Deploy(0)],
            ),
            (
                "three, every deploy to a validator that did not sign the genesis",
                vec![100, 100, 50],
                vec![Deploy(1), Deploy(1), Deploy(1)],
            ),
            (
                "three, every deploy to the 50",
                vec![100, 100, 50],
                vec![Deploy(2), Deploy(2)],
            ),
            (
                "three, deploys in rotation",
                vec![100, 100, 50],
                vec![Deploy(0), Deploy(1), Deploy(2)],
            ),
            (
                "five, every deploy to one",
                vec![100; 5],
                vec![Deploy(0), Deploy(0), Deploy(0)],
            ),
            (
                "eight, scattered",
                vec![100; 8],
                vec![Deploy(0), Deploy(3), Deploy(7), Deploy(0)],
            ),
            (
                "three, the 50 killed",
                vec![100, 100, 50],
                vec![Deploy(0), Kill(2), Deploy(0), Deploy(1), Deploy(0)],
            ),
            (
                "three, the 50 killed and back",
                vec![100, 100, 50],
                vec![
                    Deploy(0),
                    Kill(2),
                    Deploy(0),
                    Deploy(1),
                    Revive(2),
                    Deploy(0),
                    Deploy(2),
                ],
            ),
            (
                "four, one killed",
                vec![100; 4],
                vec![Deploy(0), Kill(3), Deploy(0), Deploy(1), Deploy(2)],
            ),
            (
                "three, a fourth bonds and speaks",
                vec![100, 100, 50],
                vec![Deploy(0), Bond(3, 50), Deploy(0), Deploy(3), Deploy(1)],
            ),
            (
                "two, a third bonds and never speaks",
                vec![100, 100],
                vec![Deploy(0), Bond(2, 10), Deploy(0), Deploy(1), Deploy(0)],
            ),
        ]
    }

    fn bonded_count(stakes: &[i64], script: &[Step]) -> usize {
        stakes.len()
            + script
                .iter()
                .filter(|s| matches!(s, Step::Bond(..)))
                .count()
    }

    /// **C209: with a supermajority live, every deploy finalises, at a bounded cost, and the net then
    /// stops.** Whichever validator the deploy is sent to, whoever is killed, returns or joins, and
    /// whichever sender signed the genesis.
    ///
    /// "Stops" is the run itself returning: the network is simulated until no attempt anywhere would
    /// produce a block, and the cap is far above the bound, so reaching it is a storm.
    #[test]
    fn every_deploy_finalises_and_then_the_chain_is_quiet() {
        for (name, stakes, script) in live_quorum_scenarios() {
            for genesis_sender in [255u8, 0] {
                let o = run(&cfg(&stakes, genesis_sender, View::Seen), &script);
                let bound = 10 * bonded_count(&stakes, &script) * o.deploys;
                assert!(
                    !o.hit_cap,
                    "{name} (genesis by {genesis_sender}): never went quiet — {o:?}"
                );
                assert_eq!(o.stranded, 0, "{name}: a deploy was left in a pool — {o:?}");
                assert_eq!(
                    o.finalized, o.deploys,
                    "{name} (genesis by {genesis_sender}): {o:?}"
                );
                assert!(
                    o.blocks <= bound,
                    "{name}: {} blocks, over {bound} — {o:?}",
                    o.blocks
                );
                assert!(
                    o.max_age <= ATTESTATION_HORIZON,
                    "{name}: a deploy went {} heights unfinalised, past the horizon — {o:?}",
                    o.max_age
                );
            }
        }
    }

    /// **The control: on the rule this replaces, a deploy sent to one validator is never finalised.**
    /// The genesis is signed by a bonded validator, as on the live net. If this starts passing, the
    /// in-process network no longer reproduces the defect and the test above proves nothing.
    #[test]
    fn on_the_round_snapshot_a_deploy_sent_to_one_validator_is_never_finalised() {
        use Step::*;
        let o = run(
            &cfg(&[100, 100, 50], 0, View::Round),
            &[Deploy(0), Deploy(0), Deploy(0)],
        );
        assert!(!o.hit_cap, "{o:?}");
        assert_eq!(o.finalized, 0, "{o:?}");
    }

    /// **The second control: reading only the work from the seen view is not enough.** The genesis is a
    /// message from its signer, so read from the round's parents the signer has "just spoken" at the tip
    /// and nobody else is moving: its cadence is never due and its quorum never reachable, and it never
    /// speaks again. A deploy sent to any other validator is then never finalised — measured live on #219
    /// (the signer's block count stayed at 1 while the others made 5 and 1) and here.
    #[test]
    fn reading_only_the_work_from_the_seen_view_strands_the_genesis_signer() {
        use Step::*;
        let script = [Deploy(1), Deploy(1), Deploy(1)];
        let o = run(&cfg(&[100, 100, 50], 0, View::SeenWork), &script);
        assert!(!o.hit_cap, "{o:?}");
        assert_eq!(o.finalized, 0, "{o:?}");
        let o = run(&cfg(&[100, 100, 50], 0, View::Seen), &script);
        assert_eq!(o.finalized, o.deploys, "{o:?}");
    }

    /// **Without a supermajority nothing can finalise, and the net must not spin trying.** A validator
    /// holding 100 of 250 is killed, or one of two: production stays bounded.
    #[test]
    fn a_lost_quorum_does_not_storm() {
        use Step::*;
        for (stakes, script) in [
            (
                vec![100, 100, 50],
                vec![Deploy(0), Kill(0), Deploy(1), Deploy(2), Deploy(1)],
            ),
            (
                vec![100, 100],
                vec![Deploy(0), Kill(1), Deploy(0), Deploy(0)],
            ),
        ] {
            for genesis_sender in [255u8, 0] {
                let o = run(&cfg(&stakes, genesis_sender, View::Seen), &script);
                assert!(!o.hit_cap, "{o:?}");
                assert!(o.blocks <= 10 * stakes.len() * o.deploys, "{o:?}");
            }
        }
    }

    /// **Late work below a fringe that has moved on keeps its licence; a stalled fringe still ends it**
    /// (#223). A returning validator's catch-up chain put a `CloseBlock` at height 10 into the survivors'
    /// view at tip 33 with their fringe at 30: counted from its own height it was 23 heights old, past the
    /// horizon, and every survivor withheld the attestation that alone could finalise it.
    #[test]
    fn late_work_below_the_fringe_is_counted_from_the_fringe() {
        let h = |n: i64| BlockHeight::try_from(n).unwrap();
        let tip = h(33);
        // Live, before: counted from the work, the licence had lapsed.
        assert_eq!(attestation_inputs(Some(h(10)), tip), (false, false));
        // Counted from the fringe it lies below, the work is 3 heights old.
        assert_eq!(work_age_floor(Some(h(10)), Some(h(30))), Some(h(30)));
        assert_eq!(
            attestation_inputs(work_age_floor(Some(h(10)), Some(h(30))), tip),
            (false, true)
        );
        // Work above the fringe is unchanged, so a stall still ends the licence on the same bound: the
        // fringe stuck at 10 with work at 12 and the tip at 12 + horizon + 1.
        let stalled_tip = h(12 + ATTESTATION_HORIZON + 1);
        assert_eq!(work_age_floor(Some(h(12)), Some(h(10))), Some(h(12)));
        assert_eq!(
            attestation_inputs(work_age_floor(Some(h(12)), Some(h(10))), stalled_tip),
            (false, false)
        );
        // No work is still no work, whatever the fringe.
        assert_eq!(work_age_floor(None, Some(h(30))), None);
    }

    /// **The horizon is where the licence ends, and only the licence.** A deploy-bearing block that is
    /// still unfinalised past it is still something to finalise, so the guard falls back to the cadence
    /// rather than to silence.
    ///
    /// **This is also W-N9-01's arm, and it is the row's refutation.** The row read the horizon as a
    /// stall: `attestation_inputs` returns `new_state_transition = false` past it, and the guard was
    /// said to suppress "unconditionally" on that alone — an honest validator silencing finality with
    /// no fault of its own. The assertions below are the two decisions the horizon actually leaves,
    /// and only one of them withholds.
    #[test]
    fn the_licence_ends_at_the_horizon_and_the_work_does_not() {
        let tip = BlockHeight::try_from(100).unwrap();
        let at = |behind: i64| Some(BlockHeight::try_from(100 - behind).unwrap());
        assert_eq!(
            attestation_inputs(None, tip),
            (true, false),
            "nothing seen is unfinalised"
        );
        assert_eq!(attestation_inputs(at(0), tip), (false, true));
        assert_eq!(
            attestation_inputs(at(ATTESTATION_HORIZON), tip),
            (false, true)
        );
        assert_eq!(
            attestation_inputs(at(ATTESTATION_HORIZON + 1), tip),
            (false, false),
            "past the horizon the deploy no longer licenses a prompt attestation"
        );

        // **What that `false` does *not* mean, which is the whole of W-N9-01.** The row read it as
        // silence: "with quorum_reachable false (or new_state_transition false) attestation_suppressed
        // returns true unconditionally", so an honest validator holding one deploy-bearing fringe past
        // the horizon could stop finality by itself — the shape the statement says cannot happen. The
        // first arm is the refutation. Past the horizon the node has lost the *licence to attest
        // promptly*, not the ability: with a quorum in reach and the cadence due it attests anyway,
        // and an explicit propose always does.
        assert!(
            !attestation_suppressed(false, false, true, true, true),
            "a deploy past the horizon still attests while a quorum is reachable and the cadence is \
             due — the row that read the horizon as silence is refuted (W-N9-01)"
        );
        assert!(
            !attestation_suppressed(false, false, true, false, false),
            "and a caller's explicit propose is never silenced by it"
        );
        assert!(
            attestation_suppressed(false, false, true, false, true),
            "with the cadence not due it is quiet — which is the anti-storm rule, not the horizon"
        );
        assert!(
            attestation_suppressed(false, false, false, true, true),
            "with no quorum in reach it withholds, and that is the half the row mistook for a stall: \
             there is nothing reachable to attest to"
        );
    }

    /// The table behind the bounds above. `cargo test -p rchain-casper --lib quiet_chain_tests::table --
    /// --ignored --nocapture`.
    #[test]
    #[ignore]
    fn table() {
        for (name, stakes, script) in live_quorum_scenarios() {
            for genesis_sender in [255u8, 0] {
                for view in [View::Round, View::SeenWork, View::Seen] {
                    let o = run(&cfg(&stakes, genesis_sender, view), &script);
                    eprintln!("{name:<36} genesis by {genesis_sender:<3} {view:?}: {o:?}");
                }
            }
        }
    }
}
