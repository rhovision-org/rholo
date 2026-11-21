//! Block proposer (port of `blocks/proposer/Proposer.scala`).
//!
//! `Proposer.apply` builds the dependency closures from the DAG/runtime; the `proposeEffect`
//! (broadcast via `CommUtil`) is supplied by the caller.

use rchain_shared::chan;
use rchain_shared::lock::Unpoison;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rchain_block_storage::block_store::BlockStore;
use rchain_block_storage::dag::dag_storage::{
    BlockDagStorage, DeployId, SelfReplayRecord, SelfReplayStage, SELF_REPLAY_BODY_MAX,
    SELF_REPLAY_IDS_MAX, SELF_REPLAY_RECORD_VERSION,
};
use rchain_block_storage::dag::liveness;
use rchain_block_storage::dag::representation::DagRepresentation;
use rchain_block_storage::syntax::put_block;
use rchain_block_storage::work_seen::WorkSeenStore;
use rchain_crypto::private_key::PrivateKey;
use rchain_crypto::signatures::secp256k1::Secp256k1;
use rchain_crypto::signatures::signed::Signed;
use rchain_models::block::state_hash::StateHash;
use rchain_models::block_hash::BlockHash;
use rchain_models::block_metadata::SlashSeverity;
use rchain_models::casper::protocol::casper_message::{BlockMessage, DeployData, SignedDeployData};
use rchain_models::validator::Validator;
use rchain_rholang::native_state::NativeSystemState;
use rchain_sdk::consensus::is_super_majority;
use rchain_shared::log::{Log, LogSource};
use rchain_shared::refined::{BlockHeight, NonNegI64};

use super::block_creator::BlockCreator;
use super::propose_result::{BlockCreatorResult, ProposalMode, ProposeResult, ProposeStatus};
use crate::api::block_api::SelfValidationRefusalSink;
use crate::conf::SelfReplayInjection;
use crate::interpreter_util::{ShardHealth, ValidationOrigin};
use crate::merging::BlockIndex;
use crate::multi_parent_casper::{
    get_pre_state_for_new_block, validate_with_origin, ValidateError, DEPLOY_LIFESPAN,
};
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

/// How many refused deploy ids the operator surface carries. The record also carries the full count,
/// so a large block is bounded on the wire without hiding that it was large.
const SELF_REPLAY_IDS_SHOWN: usize = 16;

/// **The proposer's local memory of a self-replay refusal** (#564).
///
/// The incident's loop was deterministic: the same deploy set rebuilt the same block, replay refused
/// it at the same cost pair, and nothing changed the next tick. This is the state that makes the
/// retry bounded — *which* set was refused, how many times, and which deploy ids are now held out of
/// selection. It is **proposer-local operating state, not consensus**: another validator with a
/// correct replay may include the same deploy, and the block it produces is valid here too unless
/// this node's replay is broken for it (that is RC1, and this type does not pretend to fix it). The
/// chain must not stop because one node cannot rebuild one set.
///
/// `last_selected` is recorded from what `select_deploys` actually chose, before the dev-mode dummy
/// deploy is injected. A refusal is only deploy-scoped when at least one refused id was a pooled
/// deploy; a dummy or attestation-only block has nothing to hold out, and that case stays on the
/// counted/fail-loudly path.
#[derive(Debug, Default)]
struct SelfReplayTracker {
    /// The exact refusal key: the pre-state it happened at, the deploy set, and the failure code.
    /// A *different pre-state* is a different input, not a retry of the same one.
    last: Option<(Vec<u8>, BTreeSet<DeployId>, &'static str)>,
    attempts: u64,
    /// Held out at **every** pre-state: the set was refused at two different pre-states, so it is
    /// not the pre-state's fault.
    excluded: BTreeSet<DeployId>,
    /// The pre-state that first refused each id. It is held out at that pre-state only; a later
    /// pre-state retries it once. This is what keeps the first refusal from censoring a deploy.
    suspect: BTreeMap<DeployId, Vec<u8>>,
    /// **The pool selection keyed by the block it produced** (#564): `(height, sender, seq,
    /// pre_state_hash)`. The association is per block, not a single "last" cell, so a second caller
    /// that creates a block between this one's creation and its refusal cannot misattribute the set.
    /// Bounded; the key exists only until the block is accepted or refused.
    selected: BTreeMap<SelfReplayBlockKey, BTreeSet<DeployId>>,
    /// The pool this proposer last read, so exclusions can be pruned when a deploy leaves it.
    pooled: BTreeSet<DeployId>,
}

/// The identity of a created block that carries a selection: height, sender and pre-state. One block
/// per validator per round is the protocol rule, and the pre-state distinguishes a re-derivation from
/// the block it replaces; the sequence is derived from the parent set and is not needed in the key.
type SelfReplayBlockKey = (i64, Vec<u8>, Vec<u8>);

/// **What the reset closure decided** (#564). The distinction that matters is `Raised` versus
/// `AlreadyAttempted`: a self-replay fault gets **one** reset per finalised anchor, so a recurrence
/// at the same anchor halts instead of rewinding again. Without the distinction a deterministic
/// fault would be a reset loop once the post-catch-up resume is automatic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelfReplayResetOutcome {
    /// The request was raised (or found already pending) at this finalised anchor.
    Raised([u8; 32]),
    /// This anchor has already had its one reset; the caller halts without raising another.
    AlreadyAttempted([u8; 32]),
    /// No finalised anchor exists, so no reset can be named; the caller halts.
    NoAnchor,
    /// The prior durable record could not be read, so the reset history is unknown. A destructive
    /// reset is refused: a failed read followed by a successful overwrite could erase the fact that
    /// this anchor already had its attempt.
    HistoryUnknown,
    /// The `ResetRequested` attempt record could not be written. A reset is refused unless its one
    /// attempt is durably recorded; the log says which write failed.
    AttemptNotDurable,
}

impl SelfReplayResetOutcome {
    /// The anchor this outcome names, if it names one.
    pub fn anchor(&self) -> Option<[u8; 32]> {
        match self {
            Self::Raised(anchor) | Self::AlreadyAttempted(anchor) => Some(*anchor),
            Self::NoAnchor | Self::HistoryUnknown | Self::AttemptNotDurable => None,
        }
    }

    /// Whether this is the recurrence arm — the halt reason says so.
    pub fn is_recurrence(&self) -> bool {
        matches!(self, Self::AlreadyAttempted(_))
    }

    /// A storage failure that must be named in the halt reason, rather than read as "no anchor".
    pub fn storage_refusal(&self) -> Option<&'static str> {
        match self {
            Self::HistoryUnknown => Some(
                "the prior durable self-replay record could not be read, so the reset history is \
                 unknown; no reset was raised",
            ),
            Self::AttemptNotDurable => {
                Some("the reset attempt could not be recorded durably, so no reset was raised")
            }
            Self::Raised(_) | Self::AlreadyAttempted(_) | Self::NoAnchor => None,
        }
    }
}

/// **Has this finalised anchor already had its one reset?** (#564, law 94). The pure decision the
/// reset closure applies; the anchor equality is what makes it about *this* anchor rather than the
/// fault in general.
fn reset_is_recurrence(record: &SelfReplayRecord, anchor: &BlockHash) -> bool {
    record.reset_attempted && record.reset_anchor == Some(*anchor.as_bytes())
}

/// **The attempt record and the decision to raise, in order** (#564). Extracted so the ordering is
/// a test rather than a comment: a destructive reset is raised only after the attempt record's write
/// returned `Ok`, and a recurrence never reaches the write. A failed write is `WriteFailed`, which
/// the caller turns into a halt — never into a reset.
enum ResetAttempt {
    /// The attempt is durably on record; the caller may raise the request.
    Durable,
    /// This anchor already had its one reset; the caller halts without another write or request.
    Recurrence,
    /// The attempt record could not be written; the caller must not raise the request.
    WriteFailed(String),
}

async fn record_reset_attempt<W>(
    record: &SelfReplayRecord,
    anchor: &BlockHash,
    write: W,
) -> ResetAttempt
where
    W: FnOnce(SelfReplayRecord) -> BoxFuture<Result<(), String>>,
{
    if reset_is_recurrence(record, anchor) {
        return ResetAttempt::Recurrence;
    }
    let mut requested = record.clone();
    requested.stage = SelfReplayStage::ResetRequested;
    requested.reset_anchor = Some(*anchor.as_bytes());
    requested.reset_attempted = true;
    match write(requested).await {
        Ok(()) => ResetAttempt::Durable,
        Err(e) => ResetAttempt::WriteFailed(e),
    }
}

/// **Should the devnet injector refuse this block?** (#564). Pure except for the one-shot budget:
/// `Peer` is always false (defensive — the proposer's closure is never the inbound path), `Off` is
/// false, `Deploy` spares the attestation-only fallback, and `:N` spends itself so the build after
/// the recovery is clean.
fn injection_refuses(
    mode: SelfReplayInjection,
    origin: ValidationOrigin,
    has_user_deploys: bool,
    anchored: bool,
    injected: &AtomicUsize,
) -> bool {
    if !matches!(origin, ValidationOrigin::SelfBuilt)
        || !mode.refuses(has_user_deploys)
        // **The drill's fault lands on a node with a finalised anchor.** That is the incident's
        // precondition: the node-level rung's whole safety argument is the reset to a finalised
        // block, and a fault on a fresh chain has no anchor to name. Gating the instrument there
        // keeps each arm deterministic without changing the recovery path itself.
        || !anchored
    {
        return false;
    }
    match mode.limit() {
        None => true,
        Some(limit) => injected
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                (n < limit).then_some(n + 1)
            })
            .is_ok(),
    }
}

const MAX_SELECTED_BLOCKS: usize = 8;

/// The cap on held-out deploy ids. A whole pool can be at most `MAX_POOLED_DEPLOYS`; the cap is a
/// process-memory bound, and reaching it is a fault condition, not a reason to grow for ever.
const MAX_SELF_REPLAY_EXCLUSIONS: usize = 256;

/// The identity of a created block, as the selection is keyed.
fn self_replay_block_key(block: &BlockMessage) -> SelfReplayBlockKey {
    (
        i64::from(block.block_number),
        block.sender.as_bytes().to_vec(),
        block.pre_state_hash.as_bytes().to_vec(),
    )
}

/// **Build the durable record from an in-memory refusal** (#564). The body is included when it fits
/// under the record's cap; otherwise only its length is kept, so a large block cannot make the
/// record unbounded. This is the artifact the incident did not have: the refused block, the verdict,
/// the suspect ids and the stage, before any recovery transition.
#[allow(clippy::too_many_arguments)]
fn self_replay_record(
    block: &BlockMessage,
    failure: &crate::rholang::ReplayFailure,
    cost_pair: Option<(i64, i64)>,
    state_hash_matched: Option<bool>,
    replayed_state_hash: Option<String>,
    refused: &BTreeSet<DeployId>,
    deploy_count: usize,
    stage: SelfReplayStage,
) -> SelfReplayRecord {
    let body_bytes = block.to_bytes();
    let body_len = body_bytes.len();
    let body = (body_len <= SELF_REPLAY_BODY_MAX).then_some(body_bytes);
    SelfReplayRecord {
        version: SELF_REPLAY_RECORD_VERSION,
        at_ms: rchain_shared::time::current_millis(),
        block_hash: *block.block_hash.as_bytes(),
        height: i64::from(block.block_number),
        seq_num: i64::from(block.seq_num),
        code: failure.code().to_string(),
        initial_cost: cost_pair.map(|(initial, _)| initial),
        replay_cost: cost_pair.map(|(_, replay)| replay),
        state_hash_matched,
        replayed_state_hash: replayed_state_hash
            .as_deref()
            .and_then(rchain_shared::base16::decode)
            .and_then(|v| <[u8; 32]>::try_from(v.as_slice()).ok()),
        deploy_ids: refused.iter().take(SELF_REPLAY_IDS_MAX).cloned().collect(),
        deploy_count: deploy_count as u32,
        stage,
        reset_anchor: None,
        reset_attempted: false,
        body,
        body_len,
        durable: false,
    }
}

impl SelfReplayTracker {
    /// Record one refusal keyed by `(pre_state, ids, code)`. Returns the run length for that exact
    /// key and whether this set has now been refused at a **different** pre-state — the condition
    /// that moves it from "suspect at one pre-state" to "held out everywhere". An unchanged retry is
    /// not reachable through this API: the caller's next build is `AttestOnly`, and a deploy-bearing
    /// build only happens at a new pre-state.
    fn record_refusal(
        &mut self,
        pre_state: &[u8],
        ids: &BTreeSet<DeployId>,
        code: &'static str,
    ) -> (u64, bool) {
        let same = self
            .last
            .as_ref()
            .is_some_and(|(last_pre, last_ids, last_code)| {
                last_pre.as_slice() == pre_state && last_ids == ids && *last_code == code
            });
        if same {
            self.attempts = self.attempts.saturating_add(1);
        } else {
            self.last = Some((pre_state.to_vec(), ids.clone(), code));
            self.attempts = 1;
        }
        let mut quarantined = false;
        for id in ids {
            match self.suspect.get(id) {
                Some(previous) if previous.as_slice() != pre_state => quarantined = true,
                _ => {
                    self.suspect.insert(id.clone(), pre_state.to_vec());
                }
            }
        }
        (self.attempts, quarantined)
    }

    /// The ids selection must skip at `pre_state`: the globally excluded set plus the ids first
    /// refused at this same pre-state.
    fn effective_exclusion(&self, pre_state: &[u8]) -> BTreeSet<DeployId> {
        let mut out = self.excluded.clone();
        for (id, refused_at) in &self.suspect {
            if refused_at.as_slice() == pre_state {
                out.insert(id.clone());
            }
        }
        out
    }

    /// Hold the union of these ids out of selection at every pre-state. Returns the number held and
    /// whether the cap was reached; a caller that sees `true` treats the fault as node-level rather
    /// than growing for ever.
    fn exclude(&mut self, ids: &BTreeSet<DeployId>) -> (u64, bool) {
        for id in ids {
            if self.excluded.len() >= MAX_SELF_REPLAY_EXCLUSIONS {
                return (self.excluded.len() as u64, true);
            }
            self.excluded.insert(id.clone());
            self.suspect.remove(id);
        }
        (self.excluded.len() as u64, false)
    }

    /// Record the pool the last selection read, and drop every held-out id that has left it. A deploy
    /// that was included, expired or otherwise gone cannot be censored by this node, and the operator
    /// surface then agrees with the pool rather than carrying a stale exclusion.
    fn set_pooled(&mut self, ids: BTreeSet<DeployId>) {
        self.pooled = ids;
        self.prune();
    }

    /// Drop held-out ids that have left the pool. Returns the number still held.
    fn prune(&mut self) -> u64 {
        self.excluded.retain(|id| self.pooled.contains(id));
        self.suspect.retain(|id, _| self.pooled.contains(id));
        (self.excluded.len() + self.suspect.len()) as u64
    }

    /// The globally held-out ids. Test-only: production reads [`Self::effective_exclusion`].
    #[cfg(test)]
    fn excluded(&self) -> BTreeSet<DeployId> {
        self.excluded.clone()
    }

    /// Remember what this block selected, before the dummy-deploy fallback.
    fn set_selected(&mut self, key: SelfReplayBlockKey, ids: BTreeSet<DeployId>) {
        if self.selected.len() >= MAX_SELECTED_BLOCKS {
            if let Some(oldest) = self.selected.keys().next().cloned() {
                self.selected.remove(&oldest);
            }
        }
        self.selected.insert(key, ids);
    }

    /// The deploy ids the block with this identity selected from the pool.
    fn selected(&self, key: &SelfReplayBlockKey) -> BTreeSet<DeployId> {
        self.selected.get(key).cloned().unwrap_or_default()
    }

    /// Drop the association once the block has been accepted or refused.
    fn forget_selected(&mut self, key: &SelfReplayBlockKey) {
        self.selected.remove(key);
    }

    /// A success clears the run — "consecutive" is the quantity — but **not** the exclusions: the
    /// deploy that could not be rebuilt must not return to selection on the next block.
    fn clear_attempts(&mut self) {
        self.last = None;
        self.attempts = 0;
    }
}

/// The block proposer (port of `Proposer`).
pub struct Proposer {
    get_latest_seq_number: Arc<dyn Fn(Validator) -> BoxFuture<i64> + Send + Sync>,
    check_active_validator:
        Arc<dyn Fn(&ValidatorIdentity) -> BoxFuture<Result<bool, String>> + Send + Sync>,
    create_block: Arc<
        dyn Fn(
                &ValidatorIdentity,
                ProposeSource,
                ProposalMode,
                Option<usize>,
            ) -> BoxFuture<Result<BlockCreatorResult, String>>
            + Send
            + Sync,
    >,
    validate_block:
        Arc<dyn Fn(&BlockMessage) -> BoxFuture<Result<(), ValidateError>> + Send + Sync>,
    propose_effect: Arc<dyn Fn(&BlockMessage) -> BoxFuture<()> + Send + Sync>,
    validator: ValidatorIdentity,
    log: Arc<dyn Log>,
    consecutive_failures: Arc<AtomicU64>,
    /// **Why the counted failure happened** (#485). `new` starts with an isolated sink; the node
    /// wires the shared one with [`Self::with_self_validation_refusal`].
    self_validation_refusal: SelfValidationRefusalSink,
    stale_snapshot_equivocations: Arc<AtomicU64>,
    /// Broadcast a twin of every block this node creates (see `CasperConf::equivocation_injection`).
    equivocation_injection: bool,
    /// The block store, kept here for one purpose: **persisting the injected twin's body**. A peer
    /// answers a hash announcement by asking for the block, so a twin that is announced and not held is
    /// a twin nobody ever receives — which is what the first version of this arm measured (45 twins
    /// broadcast, no refusal anywhere, no slash). `validate_block` stores the real block's body and
    /// nothing validates the twin, so the twin needs its own write.
    block_store: BlockStore,
    /// The largest execution batch this proposer may run: 1 when cross-deploy batching is off,
    /// otherwise the configured max. The node acquires at most this many execution permits.
    batch_cap: usize,
    /// **The bounded-retry state for self-replay refusals** (#564). Shared with the `create_block`
    /// closure so the exclusions reach selection; `new` starts an empty tracker and `apply` installs
    /// it with [`Self::with_self_replay_tracker`].
    self_replay: Arc<Mutex<SelfReplayTracker>>,
    /// **The reset request a self-replay fault raises** after the attestation fallback also fails
    /// (#564). `apply` wires it to the DAG's finalised anchor and the reset-request cell; `new`
    /// gets a no-op so the proposer unit tests need no DAG. It takes the record so the request and
    /// the `ResetRequested` stage are written together, and says whether this anchor has already
    /// had its one reset.
    self_replay_reset:
        Arc<dyn Fn(SelfReplayRecord) -> BoxFuture<SelfReplayResetOutcome> + Send + Sync>,
    /// **Persist the self-replay fault and its stage** (#564). `apply` wires it to the DAG's
    /// durable record; `new` gets a no-op. A failed write is reported as `durable: false`; the
    /// fault is real whether or not its evidence reached the store.
    self_replay_persist:
        Arc<dyn Fn(SelfReplayRecord) -> BoxFuture<Result<(), String>> + Send + Sync>,
    /// **Read the durable record before a refusal overwrites it** (#564). The refusal arm writes a
    /// fresh `Refused` record, but "this finalised anchor has already had its one reset" lives on
    /// the prior record, so the attempt history must be carried across. **The error is preserved**:
    /// `Ok(None)` is "no record", `Err` is "unknown history", and the two must not be conflated —
    /// an unreadable prior record must not be overwritten, and its anchor must not be reset again.
    /// `new` reads nothing; `apply` wires the DAG.
    self_replay_load:
        Arc<dyn Fn() -> BoxFuture<Result<Option<SelfReplayRecord>, String>> + Send + Sync>,
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
                    ProposalMode,
                    Option<usize>,
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
            self_validation_refusal: SelfValidationRefusalSink::default(),
            stale_snapshot_equivocations,
            equivocation_injection,
            block_store,
            batch_cap: 1,
            self_replay: Arc::new(Mutex::new(SelfReplayTracker::default())),
            self_replay_reset: Arc::new(|_| Box::pin(async { SelfReplayResetOutcome::NoAnchor })),
            self_replay_persist: Arc::new(|_| Box::pin(async { Ok(()) })),
            self_replay_load: Arc::new(|| Box::pin(async { Ok(None) })),
        }
    }

    /// Set the execution batch cap (a node builder; `new` defaults to 1).
    pub fn with_batch_cap(mut self, cap: usize) -> Self {
        self.batch_cap = cap.max(1);
        self
    }

    /// Share the tracker the `create_block` closure also holds, so the exclusions this proposer
    /// records are the ones selection applies.
    fn with_self_replay_tracker(mut self, tracker: Arc<Mutex<SelfReplayTracker>>) -> Self {
        self.self_replay = tracker;
        self
    }

    /// Share the node's refusal sink (the one `ProposeHealth` snapshots) rather than a private one.
    pub fn with_self_validation_refusal(mut self, sink: SelfValidationRefusalSink) -> Self {
        self.self_validation_refusal = sink;
        self
    }

    /// Wire the finalised-anchor reset the self-replay fault raises (#564).
    pub fn with_self_replay_reset(
        mut self,
        effect: Arc<dyn Fn(SelfReplayRecord) -> BoxFuture<SelfReplayResetOutcome> + Send + Sync>,
    ) -> Self {
        self.self_replay_reset = effect;
        self
    }

    /// Wire the durable-record read that carries a reset attempt across a refusal (#564).
    pub fn with_self_replay_load(
        mut self,
        load: Arc<dyn Fn() -> BoxFuture<Result<Option<SelfReplayRecord>, String>> + Send + Sync>,
    ) -> Self {
        self.self_replay_load = load;
        self
    }

    /// **Write the durable halt after the reset decision** (#564). `reset_attempted` is true when
    /// this fault named a finalised anchor — whether the reset was raised now or already attempted
    /// there. With no anchor the carried history is preserved, never zeroed.
    async fn record_self_replay_halt(
        &self,
        record: &SelfReplayRecord,
        outcome: SelfReplayResetOutcome,
    ) {
        let mut halted = record.clone();
        halted.stage = SelfReplayStage::Halted;
        if let Some(anchor) = outcome.anchor() {
            halted.reset_anchor = Some(anchor);
            halted.reset_attempted = true;
        }
        self.self_validation_refusal.note_self_replay_reset(
            halted.reset_attempted,
            halted
                .reset_anchor
                .map(|anchor| rchain_shared::base16::encode(&anchor)),
        );
        if let Err(e) = (self.self_replay_persist)(halted).await {
            self.log.error(
                LogSource::new("casper.blocks.Proposer"),
                &format!(
                    "the self-replay halt could not be recorded durably: {e} — a restart cannot \
                     tell that the reset was already attempted (#564)"
                ),
            );
        }
    }

    /// Wire the durable record the self-replay fault writes (#564).
    pub fn with_self_replay_persist(
        mut self,
        effect: Arc<dyn Fn(SelfReplayRecord) -> BoxFuture<Result<(), String>> + Send + Sync>,
    ) -> Self {
        self.self_replay_persist = effect;
        self
    }

    pub fn batch_cap(&self) -> usize {
        self.batch_cap
    }

    async fn do_propose(
        &self,
        source: ProposeSource,
        batch_limit: Option<usize>,
    ) -> Result<(ProposeResult, Option<BlockMessage>), String> {
        // **A key that has equivocated does not propose again** (#576). The proof is a durable
        // verdict: this node's own key signed a second block at a sequence the DAG already held, so
        // every further proposal would deepen the fork. Nothing resumes it — the operator rotates
        // the key or unbonds — which is why the check sits above the resumable self-replay halt.
        if let Some(reason) = crate::blocks::block_processor::own_key_equivocation() {
            return Ok((
                ProposeResult {
                    propose_status: ProposeStatus::KeyEquivocationHalted(reason),
                },
                None,
            ));
        }
        // **The self-replay fault latch stops every trigger** (#564). The autopropose timer, the
        // attest-on-new-blocks tap, the not-due retry and an explicit propose all arrive here, so
        // one check is what makes the halt total; the surface reads the reason beside it.
        if self.self_validation_refusal.self_replay_halted() {
            return Ok((
                ProposeResult {
                    propose_status: ProposeStatus::SelfReplayFaultHalted,
                },
                None,
            ));
        }
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

        match (self.create_block)(&self.validator, source, ProposalMode::Normal, batch_limit)
            .await?
        {
            BlockCreatorResult::AwaitingFinality => Ok((
                ProposeResult {
                    propose_status: ProposeStatus::AwaitingFinality,
                },
                None,
            )),
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
            // **A transient wait, not an isolation.** The node has observed blocks its own message
            // state has not caught up to; the next tick re-derives and proposes once it has. The
            // status is the existing "wait for more blocks" one, so an operator sees a wait rather
            // than a fault.
            BlockCreatorResult::ViewNotCaughtUp => Ok((
                ProposeResult {
                    propose_status: ProposeStatus::NotEnoughNewBlocks,
                },
                None,
            )),
            // **No private chain.** A node that has fallen more than the policy bound behind
            // finality must not keep extending a chain no peer has accepted. The status is the one
            // that already existed but had no producer; the node-level gate turns it into a halt
            // of propose *and* attest, with an operator resume.
            BlockCreatorResult::TooFarAheadOfLastFinalized => Ok((
                ProposeResult {
                    propose_status: ProposeStatus::TooFarAheadOfLastFinalized,
                },
                None,
            )),
            BlockCreatorResult::SelfReplayFaultHalted => Ok((
                ProposeResult {
                    propose_status: ProposeStatus::SelfReplayFaultHalted,
                },
                None,
            )),
            BlockCreatorResult::Created(block) => match (self.validate_block)(&block).await {
                Ok(()) => {
                    self.consecutive_failures.store(0, Ordering::Relaxed);
                    self.self_validation_refusal.clear();
                    // A success ends the run; the exclusions stay, because a deploy this node could
                    // not rebuild must not return to selection on the next block (#564).
                    {
                        let mut tracker = self.self_replay.lock().unpoison();
                        tracker.clear_attempts();
                        tracker.forget_selected(&self_replay_block_key(&block));
                    }
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
                // **A self-built block failed its own replay** (#564). Two things happen, in this
                // order: the refusal is recorded as evidence, and the node immediately builds a
                // *different* block — an attestation over the same pre-state with no user deploys.
                // A replay refusal is deterministic for the block it refused; retrying that block
                // cannot succeed, and a node that cannot move its latest message caps finality at
                // its last broadcast height. That is the incident this arm exists to end.
                Err(ValidateError::SelfReplayFailed(failure)) => {
                    let refused: BTreeSet<DeployId> = block
                        .state
                        .deploys
                        .iter()
                        .map(|d| d.deploy.sig.clone())
                        .collect();
                    let block_key = self_replay_block_key(&block);
                    let (attempts, pool_ids, quarantined) = {
                        let mut tracker = self.self_replay.lock().unpoison();
                        let selected = tracker.selected(&block_key);
                        let pool_ids: BTreeSet<DeployId> =
                            refused.intersection(&selected).cloned().collect();
                        let pre_state = block.pre_state_hash.as_bytes().to_vec();
                        let (attempts, refused_elsewhere) =
                            tracker.record_refusal(&pre_state, &refused, failure.code());
                        let mut quarantined = false;
                        if !pool_ids.is_empty() && refused_elsewhere {
                            let (total, overflowed) = tracker.exclude(&pool_ids);
                            quarantined = true;
                            if overflowed {
                                // The cap is a node-level fault: stop growing and halt every
                                // trigger rather than working through the pool for ever.
                                drop(tracker);
                                self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
                                self.self_validation_refusal.set_self_replay_halted(format!(
                                    "self-replay exclusion cap ({MAX_SELF_REPLAY_EXCLUSIONS}) reached"
                                ));
                                self.log.error(
                                    LogSource::new("casper.blocks.Proposer"),
                                    &format!(
                                        "Self-created block #{} (seq {}) reached the self-replay \
                                         exclusion cap ({MAX_SELF_REPLAY_EXCLUSIONS}); proposing \
                                         and attesting are halted until an operator resumes (#564).",
                                        block.block_number, block.seq_num
                                    ),
                                );
                                return Err(format!(
                                    "the node rejected its own block #{} (seq {}): the self-replay \
                                     exclusion cap was reached. This is a node-side bug.",
                                    block.block_number, block.seq_num
                                ));
                            }
                            self.self_validation_refusal.set_quarantined(total);
                        }
                        (attempts, pool_ids, quarantined)
                    };
                    // **The reset history is three-valued** (#564): `Ok(None)` is "no record",
                    // `Ok(Some)` carries the attempt, and `Err` is unknown history. The last two
                    // must not be conflated: an unreadable record must not be overwritten, and the
                    // node must not raise a destructive reset whose anchor may already have had
                    // its one attempt.
                    let prior = (self.self_replay_load)().await;
                    let (history_known, carried_attempted, carried_anchor) = match &prior {
                        Ok(Some(prior)) => (true, prior.reset_attempted, prior.reset_anchor),
                        Ok(None) => (true, false, None),
                        Err(e) => {
                            self.log.error(
                                LogSource::new("casper.blocks.Proposer"),
                                &format!(
                                    "the self-replay record could not be read before a new \
                                     refusal: {e} — the existing record will not be overwritten, \
                                     and no reset will be raised while the reset history is \
                                     unknown (#564)"
                                ),
                            );
                            (false, false, None)
                        }
                    };
                    let cost_pair = failure.cost_pair();
                    let (state_hash_matched, replayed_state_hash) = match &failure {
                        crate::rholang::ReplayFailure::ReplayCostMismatchWithDetail {
                            detail,
                            ..
                        } => (
                            detail.state_hash_matched,
                            detail.replayed_state_hash.clone(),
                        ),
                        _ => (None, None),
                    };
                    let deploy_count = refused.len();
                    let deploy_ids: Vec<String> = refused
                        .iter()
                        .take(SELF_REPLAY_IDS_SHOWN)
                        .map(|id| rchain_shared::base16::encode(id))
                        .collect();
                    let held_out = quarantined || !pool_ids.is_empty();
                    // **Evidence before the transition** (#564): the refused block, its verdict and
                    // the suspect ids are durable before anything acts on them. This is the artifact
                    // the incident did not have. A failed write is reported and the recovery
                    // proceeds — the fault is real whether or not its evidence reached the store.
                    let mut record = self_replay_record(
                        &block,
                        &failure,
                        cost_pair,
                        state_hash_matched,
                        replayed_state_hash.clone(),
                        &refused,
                        deploy_count,
                        SelfReplayStage::Refused,
                    );
                    record.reset_attempted = carried_attempted;
                    record.reset_anchor = carried_anchor;

                    // **The write is what makes the evidence durable**, so it runs before the surface
                    // claims any of it. With the history unknown the prior record is **not**
                    // overwritten: the fresh `Refused` would erase an attempt this process could not
                    // read, which is the failure mode the review found.
                    let persisted = if history_known {
                        match (self.self_replay_persist)(record.clone()).await {
                            Ok(()) => true,
                            Err(e) => {
                                self.log.error(
                                    LogSource::new("casper.blocks.Proposer"),
                                    &format!(
                                        "the self-replay refusal could not be recorded durably: \
                                         {e} — the recovery proceeds, but the refused body is not \
                                         durably available and a restart cannot reconstruct this \
                                         fault (#564)"
                                    ),
                                );
                                false
                            }
                        }
                    } else {
                        false
                    };
                    self.self_validation_refusal.note_self_replay(
                        failure.code(),
                        i64::from(block.block_number),
                        i64::from(block.seq_num),
                        cost_pair,
                        deploy_ids,
                        deploy_count,
                        attempts,
                        held_out,
                        state_hash_matched,
                        replayed_state_hash,
                        // **Durable availability, not in-memory presence** (#564): a body the
                        // write did not persist is not available to a restart, so the surface must
                        // not claim it is. The length is still the known in-memory length.
                        persisted && record.body.is_some(),
                        record.body_len,
                    );
                    self.log.error(
                        LogSource::new("casper.blocks.Proposer"),
                        &format!(
                            "Self-created block #{} (seq {}) failed its own replay with {} — \
                             rebuilding once as an attestation-only block over the same pre-state; \
                             the refused set stays pooled and is retried on a later pre-state \
                             (#564; the replay's own cause is RC1).",
                            block.block_number,
                            block.seq_num,
                            failure.describe()
                        ),
                    );

                    // **The one transition the incident lacked** (#564). The second build has no
                    // user deploy, so it is a different input by construction; it carries the
                    // system deploys the pre-state requires and moves this validator's latest
                    // message, which is what finality needs.
                    match (self.create_block)(
                        &self.validator,
                        source,
                        ProposalMode::AttestOnly,
                        batch_limit,
                    )
                    .await
                    {
                        Ok(BlockCreatorResult::Created(attestation)) => {
                            match (self.validate_block)(&attestation).await {
                                Ok(()) => {
                                    self.consecutive_failures.store(0, Ordering::Relaxed);
                                    // The refusal record is **not** cleared: the evidence stands
                                    // until the suspect set is resolved. Only the run counter is
                                    // cleared, because the node produced a block.
                                    {
                                        let mut tracker = self.self_replay.lock().unpoison();
                                        tracker.clear_attempts();
                                        tracker
                                            .forget_selected(&self_replay_block_key(&attestation));
                                    }
                                    (self.propose_effect)(&attestation).await;
                                    let mut recovered = record.clone();
                                    recovered.stage = SelfReplayStage::Recovered;
                                    self.self_validation_refusal.note_self_replay_reset(
                                        recovered.reset_attempted,
                                        recovered
                                            .reset_anchor
                                            .map(|a| rchain_shared::base16::encode(&a)),
                                    );
                                    if history_known {
                                        let _ = (self.self_replay_persist)(recovered).await;
                                    } else {
                                        self.log.warn(
                                            LogSource::new("casper.blocks.Proposer"),
                                            "recovered by broadcasting an attestation-only block, \
                                             but the prior self-replay record could not be read: it \
                                             is left untouched rather than overwritten with a \
                                             history this process cannot see (#564)",
                                        );
                                    }
                                    self.log.warn(
                                        LogSource::new("casper.blocks.Proposer"),
                                        &format!(
                                            "Recovered from the self-replay refusal by broadcasting \
                                             attestation-only block #{} (seq {}). The refused deploy \
                                             set is still pooled and is retried on a later pre-state; \
                                             finality is not capped by this validator (#564).",
                                            attestation.block_number, attestation.seq_num
                                        ),
                                    );
                                    Ok((
                                        ProposeResult {
                                            propose_status: ProposeStatus::SelfReplayRecovered,
                                        },
                                        Some(attestation),
                                    ))
                                }
                                Err(ValidateError::SelfReplayFailed(second)) => {
                                    // Even a block with no user deploy cannot replay: the fault is
                                    // node-level. Ask for the finalised reset (the engine acts only
                                    // when the reason's switch is on) and halt every trigger.
                                    let outcome = if history_known {
                                        (self.self_replay_reset)(record.clone()).await
                                    } else {
                                        SelfReplayResetOutcome::HistoryUnknown
                                    };
                                    if history_known {
                                        self.record_self_replay_halt(&record, outcome).await;
                                    }
                                    let reason = if let Some(storage) = outcome.storage_refusal() {
                                        format!(
                                            "the attestation fallback also failed its own replay: \
                                             {}; {storage}",
                                            second.describe()
                                        )
                                    } else if outcome.is_recurrence() {
                                        format!(
                                            "the attestation fallback also failed its own replay \
                                             after this finalised anchor's one reset: {}",
                                            second.describe()
                                        )
                                    } else {
                                        format!(
                                            "the attestation fallback also failed its own replay: {}",
                                            second.describe()
                                        )
                                    };
                                    self.self_validation_refusal
                                        .set_self_replay_halted(reason.clone());
                                    self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
                                    self.log.error(
                                        LogSource::new("casper.blocks.Proposer"),
                                        &format!(
                                            "Self-created attestation block #{} (seq {}) also \
                                             failed its own replay — {reason}. Proposing and \
                                             attesting are halted until an operator resumes (#564).",
                                            attestation.block_number, attestation.seq_num
                                        ),
                                    );
                                    Err(format!(
                                        "the node rejected its own attestation block #{} (seq \
                                         {}): {reason}. This is a node-side bug; proposing and \
                                         attesting are halted until an operator resumes.",
                                        attestation.block_number, attestation.seq_num
                                    ))
                                }
                                Err(e) => {
                                    self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
                                    self.log.error(
                                        LogSource::new("casper.blocks.Proposer"),
                                        &format!(
                                            "the recovery attestation for block #{} (seq {}) failed \
                                             validation: {e:?}",
                                            attestation.block_number, attestation.seq_num
                                        ),
                                    );
                                    Err(format!(
                                        "the recovery attestation failed validation: {e:?}"
                                    ))
                                }
                            }
                        }
                        Ok(other) => {
                            // No attestation was produced: the node cannot build any block at this
                            // pre-state. Retrying the deploy block is not an exit.
                            let outcome = if history_known {
                                (self.self_replay_reset)(record.clone()).await
                            } else {
                                SelfReplayResetOutcome::HistoryUnknown
                            };
                            if history_known {
                                self.record_self_replay_halt(&record, outcome).await;
                            }
                            let reason = if let Some(storage) = outcome.storage_refusal() {
                                format!(
                                    "no attestation could be built after the refusal ({other:?}); \
                                     {storage}"
                                )
                            } else if outcome.is_recurrence() {
                                format!(
                                    "no attestation could be built after the refusal, and this \
                                     finalised anchor's one reset was already attempted ({other:?})"
                                )
                            } else {
                                format!(
                                    "no attestation could be built after the refusal ({other:?})"
                                )
                            };
                            self.self_validation_refusal
                                .set_self_replay_halted(reason.clone());
                            self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
                            self.log.error(
                                LogSource::new("casper.blocks.Proposer"),
                                &format!(
                                    "Self-created block #{} (seq {}) failed its own replay and no \
                                     attestation could be built — {reason}. Proposing and attesting \
                                     are halted until an operator resumes (#564).",
                                    block.block_number, block.seq_num
                                ),
                            );
                            Err(reason)
                        }
                        Err(e) => {
                            let outcome = if history_known {
                                (self.self_replay_reset)(record.clone()).await
                            } else {
                                SelfReplayResetOutcome::HistoryUnknown
                            };
                            if history_known {
                                self.record_self_replay_halt(&record, outcome).await;
                            }
                            let reason = if let Some(storage) = outcome.storage_refusal() {
                                format!("the attestation build failed: {e}; {storage}")
                            } else if outcome.is_recurrence() {
                                format!(
                                    "the attestation build failed, and this finalised anchor's one \
                                     reset was already attempted: {e}"
                                )
                            } else {
                                format!("the attestation build failed: {e}")
                            };
                            self.self_validation_refusal
                                .set_self_replay_halted(reason.clone());
                            self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
                            self.log.error(
                                LogSource::new("casper.blocks.Proposer"),
                                &format!(
                                    "Self-created block #{} (seq {}) failed its own replay and the \
                                     attestation build failed — {reason}. Proposing and attesting \
                                     are halted until an operator resumes (#564).",
                                    block.block_number, block.seq_num
                                ),
                            );
                            Err(reason)
                        }
                    }
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
                    self.self_validation_refusal.note(
                        status.code(),
                        i64::from(block.block_number),
                        i64::from(block.seq_num),
                    );
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
                    self.self_validation_refusal.note(
                        "internal-error",
                        i64::from(block.block_number),
                        i64::from(block.seq_num),
                    );
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
        self.propose_with_limit(source, propose_id, None).await
    }

    /// Propose with an explicit execution-batch cap. `None` keeps the runtime's configured policy;
    /// the node passes the number of execution permits it acquired.
    pub async fn propose_with_limit(
        &self,
        source: ProposeSource,
        propose_id: tokio::sync::oneshot::Sender<ProposerResult>,
        batch_limit: Option<usize>,
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
            self.do_propose(source, batch_limit).await
        } else {
            let result = self.do_propose(source, batch_limit).await;
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
        // **The local first-seen record for unfinalised work** (#583). Aged from observation, not
        // from the advancing fringe, so the prompt-attestation licence cannot be renewed for ever.
        work_seen: WorkSeenStore,
        runtime: Arc<RuntimeManager>,
        block_index: F,
        propose_effect: Arc<dyn Fn(&BlockMessage) -> BoxFuture<()> + Send + Sync>,
        log: Arc<dyn Log>,
        consecutive_failures: Arc<AtomicU64>,
        self_validation_refusal: SelfValidationRefusalSink,
        stale_snapshot_equivocations: Arc<AtomicU64>,
        equivocation_injection: bool,
        // **The devnet-only self-replay injector** (#564). Refused outside `--dev-mode` by
        // `check_self_replay_injection`; it is applied only to this node's own blocks.
        self_replay_injection: SelfReplayInjection,
        // **This shard's finality-and-merge health** (W-N10-02/03), held like the two counters above:
        // the proposer drives `validate` on its own blocks, so it is one of the writers.
        health: ShardHealth,
        // **The runtime's admissible pre-state window** (review R2): this node's own validated
        // blocks derive roots that its peers' children may build on, so the proposer notes them.
        accepted_window: crate::engine::anchor::SharedAcceptedWindow,
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
                    let latest = dag_repr
                        .dag_message_state
                        .latest_msgs
                        .get(&sender)
                        .map(|m| i64::from(m.sender_seq))
                        .unwrap_or(-1);
                    // **A restarted node must not reuse a sequence it already signed.** After a
                    // store wipe the node's own earlier blocks arrive through the catch-up and can
                    // sit in `msg_map` without being the key's `latest_msgs` entry; proposing
                    // `latest + 1` then collides with one of them and the DAG's own-key gate refuses
                    // (AUDIT C190's warning: "in msg_map but not latest_msgs"). The next sequence is
                    // past every message this node's key has in the DAG, not just the latest map.
                    let anywhere = dag_repr
                        .dag_message_state
                        .msg_map
                        .values()
                        .filter(|m| m.sender == sender)
                        .map(|m| i64::from(m.sender_seq))
                        .max()
                        .unwrap_or(-1);
                    latest.max(anywhere)
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

        // **The proposer-local bounded-retry state** (#564), shared with selection below and with
        // this proposer's refusal arm. Nothing here is consensus state: another validator's block
        // carrying the same deploy is still judged on its own replay.
        let self_replay: Arc<Mutex<SelfReplayTracker>> =
            Arc::new(Mutex::new(SelfReplayTracker::default()));
        // The selection closure and this proposer share one tracker; the clone keeps the original
        // available for the builder below.
        let self_replay_for_create = self_replay.clone();

        let create_block: Arc<
            dyn Fn(
                    &ValidatorIdentity,
                    ProposeSource,
                    ProposalMode,
                    Option<usize>,
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
            // Independent monotonic retry for a quiet chain with unfinalized work.
            // It belongs to this shard/proposer, not a process-global clock.
            let quiet_retry_since: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
            // The finalized hash whose work-seen rows were last pruned: the sweep runs only when
            // finality advances, so the local map stays bounded by the unfinalized region (#583).
            let work_seen_pruned: Arc<Mutex<Option<BlockHash>>> = Arc::new(Mutex::new(None));
            let accepted_window_for_create = accepted_window.clone();
            Arc::new(
                move |vi: &ValidatorIdentity,
                      source: ProposeSource,
                      mode: ProposalMode,
                      batch_limit: Option<usize>| {
                    let runtime = runtime.clone();
                    let dag = dag.clone();
                    let block_store = block_store.clone();
                    let block_index = block_index.clone();
                    let vi = vi.clone();
                    let shard_id = shard_id.clone();
                    let blocked_since_advance = blocked_since_advance.clone();
                    let blocked_since = blocked_since.clone();
                    let quiet_retry_since = quiet_retry_since.clone();
                    let work_seen = work_seen.clone();
                    let work_seen_pruned = work_seen_pruned.clone();
                    let dummy_deploy_opt = dummy_deploy_opt.clone();
                    let log = log.clone();
                    let health = health.clone();
                    let self_replay = self_replay_for_create.clone();
                    let accepted_window = accepted_window_for_create.clone();
                    let max_number_of_parents = max_number_of_parents;
                    Box::pin(async move {
                        create_block(
                            runtime.as_ref(),
                            dag.as_ref(),
                            &block_store,
                            &work_seen,
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
                            &quiet_retry_since,
                            source,
                            mode,
                            &health,
                            &self_replay,
                            &work_seen_pruned,
                            batch_limit,
                            Some(&accepted_window),
                        )
                        .await
                    })
                },
            )
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
            // **The injector's one-shot budget** (#564), shared by every call in this process.
            let injected = Arc::new(AtomicUsize::new(0));
            Arc::new(move |block: &BlockMessage| {
                let runtime = runtime.clone();
                let dag = dag.clone();
                let block_store = block_store.clone();
                let block_index = block_index.clone();
                let block = block.clone();
                let shard_id = shard_id.clone();
                let log = log.clone();
                let health = health.clone();
                let injected = injected.clone();
                // The async block is `move`; the outer closure is `Fn`, so hand the future its own
                // `Arc` rather than moving the captured one.
                let accepted_window = accepted_window.clone();
                let max_number_of_parents = max_number_of_parents;
                Box::pin(async move {
                    // **The devnet injector** (#564): a refusal on this node's own block, through the
                    // ordinary path. `Peer` can never reach this closure, but the predicate refuses it
                    // independently; the block is never inserted, stored or broadcast, exactly like
                    // the real fault.
                    let anchored = dag
                        .get_representation()
                        .await
                        .rewind_plan()
                        .anchor
                        .is_some();
                    if injection_refuses(
                        self_replay_injection,
                        ValidationOrigin::SelfBuilt,
                        !block.state.deploys.is_empty(),
                        anchored,
                        &injected,
                    ) {
                        log.error(
                            LogSource::new("casper.blocks.Proposer"),
                            &format!(
                                "[SELF-REPLAY INJECTION] refusing this node's own block #{} \
                                 (seq {}) with a synthetic replay-cost-mismatch — devnet-only \
                                 `--self-replay-injection` (#564)",
                                block.block_number, block.seq_num
                            ),
                        );
                        return Err(ValidateError::SelfReplayFailed(
                            crate::rholang::ReplayFailure::replay_cost_mismatch(1, 0),
                        ));
                    }
                    // **This block is this node's own** (#564): a replay failure is a local fault,
                    // not a peer-view divergence. The inbound path keeps `validate`, unchanged.
                    match validate_with_origin(
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
                        Some(&accepted_window),
                        ValidationOrigin::SelfBuilt,
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
                            let accepted_meta = meta.clone();
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
                            // **This node's own validated block derives roots its peers' children
                            // may build on.** Noted only after the insert committed.
                            {
                                let mut guard = accepted_window.write().await;
                                if let Some(window) = guard.as_mut() {
                                    window.note_roots(&accepted_meta);
                                }
                            }
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

        // **The self-replay fault's reset transition** (#564). It runs only after the attestation
        // fallback also failed, so it is the node-level rung: the anchor is the last finalised block,
        // which is the only state that is safe to rewind to, and the request carries the refused
        // block so the engine can log it. The `ResetRequested` stage and the anchor are written
        // **before** the request is raised, so a crash between the two leaves a record that names the
        // one attempt this fault is allowed. A view with no finalised anchor cannot name one and the
        // proposer halts instead — "nothing to rewind to" never becomes "rewind to genesis".
        let self_replay_reset: Arc<
            dyn Fn(SelfReplayRecord) -> BoxFuture<SelfReplayResetOutcome> + Send + Sync,
        > = {
            let dag = dag.clone();
            let shard_id = shard_id.clone();
            let log = log.clone();
            Arc::new(move |record: SelfReplayRecord| {
                let dag = dag.clone();
                let shard_id = shard_id.clone();
                let log = log.clone();
                Box::pin(async move {
                    let plan = dag.get_representation().await.rewind_plan();
                    match (plan.anchor, plan.anchor_height) {
                        (Some(anchor), Some(anchor_height)) => {
                            // **One reset per finalised anchor** (#564, law 94). The record carried
                            // across the refusal names whether this anchor has already been reset;
                            // a recurrence halts instead of rewinding again, so the automatic
                            // post-catch-up resume cannot turn a deterministic fault into a loop.
                            // The attempt's write gates the request: no durable attempt, no reset.
                            match record_reset_attempt(&record, &anchor, |requested| {
                                let dag = dag.clone();
                                Box::pin(
                                    async move { dag.write_self_replay_record(requested).await },
                                )
                            })
                            .await
                            {
                                ResetAttempt::Recurrence => {
                                    // Halt durably before the caller writes the same stage: a crash
                                    // in this window must not leave the non-latching `Refused`
                                    // record as the last word.
                                    let mut halted = record.clone();
                                    halted.stage = SelfReplayStage::Halted;
                                    halted.reset_anchor = Some(*anchor.as_bytes());
                                    halted.reset_attempted = true;
                                    if let Err(e) = dag.write_self_replay_record(halted).await {
                                        log.error(
                                            LogSource::new("casper.blocks.Proposer"),
                                            &format!(
                                                "the self-replay recurrence record could not be \
                                                 written: {e} — a restart may retry the unchanged \
                                                 input (#564)"
                                            ),
                                        );
                                    }
                                    log.error(
                                        LogSource::new("casper.blocks.Proposer"),
                                        &format!(
                                            "self-replay fault at block {} (seq {}) at height {}: \
                                             already reset once at the finalised anchor {} — halting \
                                             instead of a second reset (#564, law 94)",
                                            rchain_shared::base16::encode(&record.block_hash),
                                            record.seq_num,
                                            record.height,
                                            anchor.to_hex(),
                                        ),
                                    );
                                    return SelfReplayResetOutcome::AlreadyAttempted(
                                        *anchor.as_bytes(),
                                    );
                                }
                                ResetAttempt::WriteFailed(e) => {
                                    log.error(
                                        LogSource::new("casper.blocks.Proposer"),
                                        &format!(
                                            "the self-replay reset attempt could not be recorded: \
                                             {e} — **no reset is raised**: a destructive rewind is \
                                             only admissible when its one attempt per anchor is \
                                             durably on record, or a crash could turn it into a \
                                             second attempt (#564)"
                                        ),
                                    );
                                    return SelfReplayResetOutcome::AttemptNotDurable;
                                }
                                ResetAttempt::Durable => {}
                            }
                            let raised = crate::interpreter_util::request_reset(
                                shard_id.clone(),
                                crate::interpreter_util::ResetRequest {
                                    anchor,
                                    anchor_height,
                                    blocks_above: plan.blocks_above,
                                    refused: BlockHash::new(record.block_hash),
                                    reason: crate::interpreter_util::ResetReason::SelfReplayFault,
                                },
                            );
                            log.error(
                                LogSource::new("casper.blocks.Proposer"),
                                &format!(
                                    "self-replay fault at block {} (seq {}) at height {}: the \
                                     attestation fallback also failed, so this node {} a reset to the \
                                     last finalised block {} at height {} (#564)",
                                    rchain_shared::base16::encode(&record.block_hash),
                                    record.seq_num,
                                    record.height,
                                    if raised { "requested" } else { "found pending" },
                                    anchor.to_hex(),
                                    i64::from(anchor_height)
                                ),
                            );
                            SelfReplayResetOutcome::Raised(*anchor.as_bytes())
                        }
                        _ => {
                            log.error(
                                LogSource::new("casper.blocks.Proposer"),
                                &format!(
                                    "self-replay fault at block {} (seq {}) at height {}: no \
                                     finalised anchor exists, so no reset can be named; the node \
                                     halts proposing and attesting (#564)",
                                    rchain_shared::base16::encode(&record.block_hash),
                                    record.seq_num,
                                    record.height
                                ),
                            );
                            SelfReplayResetOutcome::NoAnchor
                        }
                    }
                })
            })
        };

        // **The durable half** (#564): the same DAG store as the reset marker and halt record. A write
        // that fails is surfaced by the caller as `durable: false`; it never cancels the recovery.
        let self_replay_persist: Arc<
            dyn Fn(SelfReplayRecord) -> BoxFuture<Result<(), String>> + Send + Sync,
        > = {
            let dag = dag.clone();
            Arc::new(move |record: SelfReplayRecord| {
                let dag = dag.clone();
                Box::pin(async move { dag.write_self_replay_record(record).await })
            })
        };

        // **The reset attempt survives the next refusal** (#564): a refusal writes a fresh `Refused`
        // record, so the "this anchor has had its one reset" fact is read first and carried.
        let self_replay_load: Arc<
            dyn Fn() -> BoxFuture<Result<Option<SelfReplayRecord>, String>> + Send + Sync,
        > = {
            let dag = dag.clone();
            Arc::new(move || {
                let dag = dag.clone();
                // **The error is returned, not flattened to `None`.** The caller must be able to
                // tell "no record" from "unknown history": a read failure followed by a successful
                // overwrite is exactly how the one-reset-per-anchor bound would be erased.
                Box::pin(async move { dag.read_self_replay_record().await })
            })
        };

        let batch = runtime.batch_policy();
        let batch_cap = if batch.enabled {
            batch.max_batch.max(1)
        } else {
            1
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
        .with_batch_cap(batch_cap)
        .with_self_validation_refusal(self_validation_refusal)
        .with_self_replay_tracker(self_replay)
        .with_self_replay_reset(self_replay_reset)
        .with_self_replay_persist(self_replay_persist)
        .with_self_replay_load(self_replay_load)
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
///
/// `excluded` is the proposer-local set held out after repeated self-replay refusals (#564). It is not a
/// validity rule: the deploy stays in the pool and another validator may include it. It is the bound
/// that stops this node rebuilding the one block its own replay refuses.
/// Returns the selected ids and the pool ids it read, so the caller can prune exclusions against the
/// pool without a second DAG read.
async fn select_deploys(
    dag: &dyn BlockDagStorage,
    next_block_num: BlockHeight,
    budget: usize,
    excluded: &BTreeSet<DeployId>,
) -> Result<(Vec<DeployId>, BTreeSet<DeployId>), String> {
    let pooled = dag.pooled_deploys().await?;
    let pooled_ids: BTreeSet<DeployId> = pooled.keys().cloned().collect();
    let mut deploys: Vec<DeployId> = Vec::new();
    for (id, d) in pooled {
        if excluded.contains(&id) {
            continue;
        }
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
    Ok((deploys, pooled_ids))
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

/// **How far above the last finalized block a validator may propose.**
///
/// This is the isolation bound the proposer never had: `TooFarAheadOfLastFinalized` existed as a
/// status variant but no code produced it, so a validator whose peers had stopped accepting its
/// blocks could extend a private chain indefinitely. Thirty-two heights is policy, not a measured
/// optimum — a healthy four-validator net trails finality by a handful of heights — and it is
/// deliberately a height gap, not a wall-clock stall: finality-stall counters fire on a healthy
/// net (see `ShardHealth`'s own episodes), while a tip 32 blocks beyond the last finalized block
/// is unambiguously a node that cannot get its own chain accepted.
pub const MAX_PROPOSAL_FINALITY_LAG: i64 = 32;

/// The height-gap half of the isolation gate, as a pure predicate so the boundary (`== bound`
/// passes, `bound + 1` refuses) is testable without a DAG.
pub fn within_proposal_finality_bound(tip_height: i64, finalized_height: i64) -> bool {
    tip_height.saturating_sub(finalized_height) <= MAX_PROPOSAL_FINALITY_LAG
}

/// **How far the node's own quorum-supported view may trail the tip it has observed** before it
/// proposes — the shared catch-up window (`crate::engine::catchup::within_window`, Law 121).
///
/// Finality advances only on a closed fringe of the **latest messages**. A validator that proposes
/// while its DAG has not yet accepted the other validators' recent blocks publishes a message whose
/// seen set is an old fringe: no fence above it can close until that message ages out, so one
/// alive-but-stale latest message caps the **whole** partition's finality. Measured on the RC-4 join
/// drill (2026-11-21): a rejoined validator proposed from a view ~30 heights behind the blocks it had
/// already observed; the fence froze at 223 while tips reached 255, and
/// [`MAX_PROPOSAL_FINALITY_LAG`] then stopped every proposer — the network deadlocked, and stopping
/// the rejoined validator afterwards did not recover it.
///
/// The supported height is the same primitive the liveness partition uses
/// ([`liveness::quorum_supported_height`]): the greatest height reached by strictly more than two
/// thirds of the **active** bond map. A silent peer therefore cannot hold this gate closed — the live
/// majority's messages set the supported height — while a node whose own message state is behind the
/// blocks it has received is held until it catches up.
pub fn view_is_caught_up(view_tip: i64, supported: BlockHeight) -> bool {
    let tip = BlockHeight::try_from(view_tip).unwrap_or_else(|_| BlockHeight::zero());
    crate::engine::catchup::within_window(tip, supported)
}

/// **The height the gate measures from when the view has no finalised block** (C351).
///
/// `last_finalized_block_hash()` is `None` on a view with no fringe, and the gate used to be wrapped
/// in `if let Some(finalized)`, so *both* the bound and the isolation latch were skipped entirely —
/// the shape the Lean model refuses with `a_gate_without_the_bound_admits_a_private_chain`. A node
/// that early in a chain has no safe state to name, and "no anchor" is not "no bound": genesis is
/// the floor, which is the same answer `RewindPlan` already gives (`block-storage/src/dag/
/// representation.rs::rewind_plan`), and it is what the model's `finalizedFloor` states.
pub fn finalized_floor(finalized: Option<BlockHeight>) -> BlockHeight {
    finalized.unwrap_or_else(BlockHeight::zero)
}

/// The gate's decision, as a pure function so the **no-finality arm** is testable without a DAG.
///
/// This is the property C351 is about: `proposal_is_within_bound(n, None)` is `false` beyond the
/// bound. The gate this replaced could be written `finalized.is_some_and(|f| within(n, f))`, which
/// answers `true` for `None` — no bound at all — and that is the falsifier the test exhibits.
pub fn proposal_is_within_bound(
    next_block_num: BlockHeight,
    finalized: Option<BlockHeight>,
) -> bool {
    within_proposal_finality_bound(
        i64::from(next_block_num),
        i64::from(finalized_floor(finalized)),
    )
}

/// Set when the height-gap gate refuses a proposal. It is a process-local fail-safe latch: it
/// stops proposing and attesting and tells a peer/operator why; a restart (or an explicit
/// operator resume) is the recovery. No proposal path may clear it just because one later tick
/// looks closer to finality — the node has already shown it cannot get its chain accepted.
static PROPOSAL_ISOLATED: AtomicBool = AtomicBool::new(false);
static PROPOSAL_ISOLATION_REASON: Mutex<Option<String>> = Mutex::new(None);

pub fn proposal_isolated() -> bool {
    PROPOSAL_ISOLATED.load(Ordering::Relaxed)
}

pub fn proposal_isolation_reason() -> Option<String> {
    PROPOSAL_ISOLATION_REASON.lock().unpoison().clone()
}

pub fn clear_proposal_isolation() {
    PROPOSAL_ISOLATED.store(false, Ordering::Relaxed);
    *PROPOSAL_ISOLATION_REASON.lock().unpoison() = None;
}

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

/// **The pacing rule as a pure function** (Law 51d), so the conformance corpus can tie the model
/// to the node. Given the first blocked sighting (if any) and `now`, returns `(escape?, new
/// sighting)`: `None` starts the clock; a full interval returns `true` and **consumes** the permit
/// by moving the sighting to `now`; anything else returns `false` and leaves the sighting alone.
/// The mutex form below is this function's one implementation — a banked permit would make a
/// still-open round escape on every tick after the first interval.
pub fn take_escape_permit(first_ms: Option<u64>, now_ms: u64) -> (bool, Option<u64>) {
    let bound = ROUND_STALL_ESCAPE.as_millis() as u64;
    match first_ms {
        None => (false, Some(now_ms)),
        Some(first) if now_ms.saturating_sub(first) >= bound => (true, Some(now_ms)),
        Some(first) => (false, Some(first)),
    }
}

/// Consume one escape permit after a full monotonic stall interval.
/// A still-open round must wait another interval before escaping again;
/// otherwise a fast producer can exhaust the finality-lag bound while its
/// peers are still assembling the next shared justification layer.
fn take_round_escape(since: &Mutex<Option<Instant>>, now: Instant) -> Option<i64> {
    let elapsed = stall_ms_since(since, now);
    if !round_escape_owed(elapsed) {
        return None;
    }
    *since.lock().unpoison() = Some(now);
    Some(elapsed)
}

/// **The epoch trigger is the chain's, not the node's config file** (C301's C5).
///
/// `close_block` applies the epoch transition when the on-chain `PosParams.epoch_length` says the
/// height is a boundary. A proposer that used a different local value would emit boundary blocks
/// with and without the close at the same heights — a silent fork between nodes that agree on
/// everything else. The node's configured value is used only when the chain has no `PosParams`
/// record at all (tests and bare surfaces); on a chain that has one, a divergence is refused rather
/// than proposed, so the operator sees it instead of the network.
fn epoch_trigger(
    local_epoch_length: i32,
    on_chain_epoch_length: Option<i64>,
    next_block_num: i64,
) -> Result<bool, String> {
    let epoch_length = match on_chain_epoch_length {
        Some(on_chain) if i64::from(local_epoch_length) != on_chain => {
            return Err(format!(
                "the configured epoch length ({local_epoch_length}) differs from the on-chain \
                 PosParams ({on_chain}); refusing to propose — the block's boundary trigger would \
                 disagree with the transition `close_block` applies"
            ))
        }
        Some(on_chain) => on_chain,
        None => i64::from(local_epoch_length),
    };
    Ok(epoch_length > 0 && next_block_num != 0 && next_block_num % epoch_length == 0)
}

#[allow(clippy::too_many_arguments)]
async fn create_block<'a, F, Fut>(
    runtime: &'a RuntimeManager,
    dag: &'a dyn BlockDagStorage,
    block_store: &'a BlockStore,
    work_seen: &WorkSeenStore,
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
    quiet_retry_since: &Mutex<Option<Instant>>,
    source: ProposeSource,
    mode: ProposalMode,
    health: &ShardHealth,
    self_replay: &Mutex<SelfReplayTracker>,
    work_seen_pruned: &Mutex<Option<BlockHash>>,
    batch_limit: Option<usize>,
    accepted_window: Option<&crate::engine::anchor::SharedAcceptedWindow>,
) -> Result<BlockCreatorResult, String>
where
    F: Fn(BlockHash) -> Fut + Sync,
    Fut: Future<Output = Result<Arc<BlockIndex>, String>>,
{
    // The isolation latch is a fail-safe, not a per-tick verdict: once set, this process does
    // not propose again until an operator repairs/restarts (or calls the resume hook). Check it
    // before the parent merge so a latched node does no proposal work at all.
    if proposal_isolated() {
        return Ok(BlockCreatorResult::TooFarAheadOfLastFinalized);
    }

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
        accepted_window,
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
            let Some(stalled_ms) = take_round_escape(blocked_since, Instant::now()) else {
                return Ok(BlockCreatorResult::AlreadyProposedThisRound);
            };
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

    // **The view gate** (join ladder, RC-4): see `view_is_caught_up`. It comes *before* the
    // private-chain bound on purpose: a node that is catching up must **wait**, not latch the
    // isolation halt that stops propose and attest until an operator resumes. That latch, firing
    // for every proposer once finality froze, was the deadlock the drill measured.
    {
        let bonds = runtime
            .compute_bonds(&StateHash::from_slice(pre_state_hash.as_bytes()))
            .await?;
        let repr = dag.get_representation().await;
        // **Only the active set's messages set the tip**, for the same reason they set the quorum: a
        // jailed or unbonded sender's last message is frozen where it stopped, and letting it stand
        // as the highest latest message would hold every proposer closed with nothing to advance it.
        let (view_tip, supported) = crate::engine::catchup::view(
            repr.dag_message_state
                .latest_msgs
                .values()
                .map(|message| (message.sender.clone(), message.height)),
            &bonds,
        );
        if !crate::engine::catchup::within_window(view_tip, supported) {
            log.warn(
                LogSource::new("casper.blocks.Proposer"),
                &format!(
                    "waiting to propose at height {}: the node's highest latest message is {} while \
                     a quorum supports {} (more than {} heights behind); a proposal from this view \
                     would publish a fringe no closed set can include",
                    i64::from(next_block_num),
                    i64::from(view_tip),
                    i64::from(supported),
                    liveness::LIVENESS_WINDOW
                ),
            );
            return Ok(BlockCreatorResult::ViewNotCaughtUp);
        }
    }

    // **The private-chain bound.** A node whose tip is this far beyond its own last finalized
    // block has no evidence that any peer accepts its chain; continuing to propose is how A ran
    // 267 heights against finality 3. Return the pre-existing status variant and leave the block
    // uncreated. The node-level gate turns this into a propose-and-attest halt with an operator
    // resume; the proposer alone at least stops extending the chain.
    let last_finalized_height = {
        let repr = dag.get_representation().await;
        repr.last_finalized_block_hash()
            .and_then(|hash| repr.block_height_of(&hash))
    };
    // **The gate is total** (C351): a view with no finalised block measures from genesis rather than
    // skipping the bound and the latch. Before this, `if let Some(finalized)` made "no anchor" mean
    // "no bound", which is `Disposition.a_gate_without_the_bound_admits_a_private_chain`.
    let finalized = finalized_floor(last_finalized_height);
    let gap = i64::from(next_block_num) - i64::from(finalized);
    if !proposal_is_within_bound(next_block_num, last_finalized_height) {
        let reason = format!(
            "tip {} is {gap} heights beyond last finalized {} (bound {MAX_PROPOSAL_FINALITY_LAG})",
            i64::from(next_block_num),
            i64::from(finalized)
        );
        PROPOSAL_ISOLATED.store(true, Ordering::Relaxed);
        *PROPOSAL_ISOLATION_REASON.lock().unpoison() = Some(reason.clone());
        log.warn(
            LogSource::new("casper.blocks.Proposer"),
            &format!(
                "refusing to propose at height {}: {reason}; this node cannot show that its chain \
                 is accepted",
                i64::from(next_block_num)
            ),
        );
        return Ok(BlockCreatorResult::TooFarAheadOfLastFinalized);
    }

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

    // An epoch boundary is a block whose height is a positive multiple of the **on-chain**
    // `PosParams.epoch_length` (matching the PoS contract's `blockNumber % epochLength == 0`); the
    // node's configured value is only a fallback for a surface with no params record, and a
    // divergence is refused rather than proposed (C301's C5).
    let on_chain_epoch_length = NativeSystemState::new(runtime.runtime().native_store())
        .params_opt()
        .await
        .map_err(|e| format!("cannot read the on-chain PosParams for the epoch trigger: {e}"))?
        .map(|params| params.epoch_length);
    let change_epoch = epoch_trigger(
        epoch_length,
        on_chain_epoch_length,
        i64::from(next_block_num),
    )?;

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

    // A merge can publish a rejection without executing a new deploy. Keep
    // attesting until that decision is covered by the published fringe.
    // Subtract decisions already in its causal past so repeated rejection
    // ledgers on empty attestations cannot keep a quiet chain running.
    let mut finalized_rejections = None;

    let mut newest_work: Option<(BlockHash, BlockHeight)> = None;
    // **Newest first**: the licence ages from the newest unfinalised work-bearing block, so an
    // expired side branch cannot mask work still inside its own opportunity (#583).
    for (height, h) in conflict_set.iter().rev() {
        if let Some(b) = get_block(block_store, h).await? {
            if b.state.system_deploys.is_empty()
                && b.state.deploys.is_empty()
                && !b.rejected_deploys.is_empty()
                && finalized_rejections.is_none()
            {
                let mut covered = BTreeSet::new();
                for h in fringe_seen.iter().chain(pre_state.prev_fringe.iter()) {
                    if let Some(finalized) = get_block(block_store, h).await? {
                        covered.extend(finalized.rejected_deploys);
                    }
                }
                finalized_rejections = Some(covered);
            }
            if block_needs_finalization(
                &b,
                finalized_rejections.as_ref().unwrap_or(&BTreeSet::new()),
            ) {
                newest_work = Some((h.clone(), *height));
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
    // **The licence is aged from this node's first observation of the work, not from the moving
    // fringe** (#583). The work's own height is not a substitute: #223's returning validator observed
    // its boundary work twenty-three heights late.
    let licence_age = work_licence_age(work_seen, newest_work.as_ref(), tip, log).await;
    let (nothing_to_finalize, new_state_transition) = attestation_inputs(licence_age);
    prune_work_seen(work_seen, dag_repr.as_ref(), work_seen_pruned, log).await;
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
    // A stalled chain can keep a tip at the same height indefinitely. Height-based
    // cadence alone then never becomes due, while the bounded deploy horizon has
    // already expired. With >2/3 of the *whole bonded stake* still reachable,
    // retry at most once per monotonic interval to supply the missing attestation.
    // This is a local proposal schedule, not a change to finality or quorum rules.
    let quiet_retry = quiet_finality_retry_due(
        quiet_retry_since,
        Instant::now(),
        !nothing_to_finalize && !new_state_transition && quorum_reachable,
        source == ProposeSource::Automatic,
    );
    // Only a proposal the node raised on its own is paced (C171): the storm is a node reacting to remote
    // blocks, and a caller's explicit propose is not a reaction.
    let paced = source == ProposeSource::Automatic;
    // **The recovery transition forces the attestation** (#564). `suppress_attestation` is the
    // empty-block storm's pace rule; an `AttestOnly` build exists precisely because the node must
    // move its latest message after a deploy-bearing refusal, so the pace rule must not withhold it.
    let suppress_attestation = suppress_attestation_for(
        mode,
        nothing_to_finalize,
        new_state_transition,
        quorum_reachable,
        cadence || quiet_retry,
        paced,
    );
    // **Why this node did or did not attest, in its own log** (#213's diagnostic). Without it a validator
    // that stays silent looks exactly like one with nothing to do: the R3 arm on #219 showed the genesis
    // signer making no block after genesis, and no run could say which input held it.
    let decision = describe_attestation_decision(&AttestationDecision {
        nothing_to_finalize,
        new_state_transition,
        quorum_reachable,
        cadence_due: cadence || quiet_retry,
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
    // **And skip the set this node's own replay refused three times** (#564): the exclusion is
    // proposer-local, so the deploy stays valid for every other node.
    let pre_state_bytes = pre_state_hash.as_bytes().to_vec();
    let excluded = self_replay
        .lock()
        .unpoison()
        .effective_exclusion(&pre_state_bytes);
    let (mut deploys, pooled_ids) = select_deploys(
        dag,
        next_block_num,
        per_block_deploy_budget(to_slash.len()),
        &excluded,
    )
    .await?;
    // **The recovery transition carries no user deploy** (#564). The block is still built over the
    // same pre-state and still carries the system deploys the pre-state requires; the refused set
    // stays pooled and is retried on a later pre-state. Clearing the id list (rather than passing a
    // different selection) is what makes the second build a *different input* by construction.
    if mode == ProposalMode::AttestOnly {
        deploys.clear();
    }
    // Remember what selection actually chose, **before** the dev-mode dummy fallback: a refusal of a
    // dummy-only block is not deploy-set-scoped, and the refusal arm distinguishes the two by this set.
    // Keyed by the block it will produce, so a later create/validate cannot overwrite it.
    let selection_key = (
        i64::from(next_block_num),
        creators_validator.as_bytes().to_vec(),
        pre_state_hash.as_bytes().to_vec(),
    );
    {
        let mut tracker = self_replay.lock().unpoison();
        tracker.set_pooled(pooled_ids);
        tracker.set_selected(selection_key, deploys.iter().cloned().collect());
    }
    // **A block with nothing to include says why** (issue #356). Reported here rather than at the
    // callers because this is where the reasons are known, and only on this path.
    if deploys.is_empty() {
        report_nothing_to_include(dag, next_block_num, log).await;
    }

    // Dev-mode dummy deploy: when there is nothing pooled to include, inject a signed `Nil` deploy so
    // `--autopropose` can keep producing blocks (port of Scala `Proposer.dummyDeployOpt`). The
    // recovery transition is **not** a dummy-deploy path: its block must carry no user deploy at all.
    if mode == ProposalMode::Normal && deploys.is_empty() {
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

    // A suppressed attestation with outstanding work is not an idle proposer.
    // Without this distinction the stream receives NoNewDeploys and never wakes
    // the monotonic quiet-finality timer on a no-autopropose network.
    if mode == ProposalMode::Normal
        && deploys.is_empty()
        && to_slash.is_empty()
        && !change_epoch
        && suppress_attestation
        && !nothing_to_finalize
        && quorum_reachable
        && paced
    {
        return Ok(BlockCreatorResult::AwaitingFinality);
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
        batch_limit,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The private-chain boundary is inclusive at the bound and refuses one height beyond it.**
    /// `TooFarAheadOfLastFinalized` existed without a producer; this pins the arithmetic the
    /// `create_block` gate now uses.
    #[test]
    fn the_proposal_finality_bound_passes_at_the_bound_and_refuses_beyond_it() {
        assert!(within_proposal_finality_bound(MAX_PROPOSAL_FINALITY_LAG, 0));
        assert!(!within_proposal_finality_bound(
            MAX_PROPOSAL_FINALITY_LAG + 1,
            0
        ));
        assert!(within_proposal_finality_bound(4, 0));
    }

    /// **The view gate passes at the window and refuses one height beyond it** (join ladder,
    /// RC-4). The arm the drill measured — a 30-height gap between the observed tip and the
    /// node's own quorum-supported height — must refuse; the boundary itself must not.
    #[test]
    fn the_view_gate_refuses_a_view_a_window_behind_the_observed_tip() {
        let height = |v: i64| BlockHeight::try_from(v).expect("non-negative");
        let tip = 60;
        assert!(view_is_caught_up(
            tip,
            height(tip - liveness::LIVENESS_WINDOW)
        ));
        assert!(!view_is_caught_up(
            tip,
            height(tip - liveness::LIVENESS_WINDOW - 1)
        ));
        // The drill's arm: the highest latest message at 253, a quorum stuck at 223.
        assert!(!view_is_caught_up(253, height(223)));
        // A fresh chain is not held: nothing observed, nothing supported.
        assert!(view_is_caught_up(0, BlockHeight::zero()));
    }

    /// **The epoch trigger is the on-chain length, and a divergent local value is refused**
    /// (C301's C5). A node configured with 5 on a chain whose `PosParams` say 10 would have
    /// proposed a boundary block at 10 without the close its own transition applies — the silent
    /// fork. The configured value is only the fallback for a surface with no params record.
    #[test]
    fn the_epoch_trigger_is_the_on_chain_length_and_a_divergence_is_refused() {
        assert!(epoch_trigger(5, None, 10).expect("no params record"));
        assert!(!epoch_trigger(5, None, 11).expect("no params record"));

        let err = epoch_trigger(5, Some(10), 10).expect_err("a divergence is refused");
        assert!(err.contains("differs from the on-chain"), "{err}");

        assert!(epoch_trigger(10, Some(10), 10).expect("agreement"));
        assert!(!epoch_trigger(10, Some(10), 11).expect("agreement"));

        assert!(!epoch_trigger(0, None, 10).expect("a non-positive length disables"));
        assert!(!epoch_trigger(10, Some(10), 0).expect("height 0 is genesis"));
    }

    /// **A view with no finalised block is still bounded** (C351). The gate used to be wrapped in
    /// `if let Some(finalized)`, so `None` skipped the bound *and* the isolation latch: a node whose
    /// fringe had not formed could extend a private chain without limit — the Lean model's own
    /// `Disposition.a_gate_without_the_bound_admits_a_private_chain`. Genesis is the floor, the same
    /// answer `rewind_plan` gives, so the no-anchor arm refuses exactly where the anchored arm does.
    #[test]
    fn a_view_without_a_finalized_block_still_trips_the_bound() {
        let height = |v: i64| BlockHeight::try_from(v).expect("non-negative");
        let bound_plus_one = height(MAX_PROPOSAL_FINALITY_LAG + 1);
        // The floor is genesis, and it is a floor rather than a sentinel: an anchor is passed through.
        assert_eq!(finalized_floor(None), BlockHeight::zero());
        assert_eq!(finalized_floor(Some(height(0))), BlockHeight::zero());
        assert_eq!(finalized_floor(Some(height(7))), height(7));
        // The falsifier: `finalized.is_some_and(|f| within(n, f))` answers `true` here.
        assert!(!proposal_is_within_bound(bound_plus_one, None));
        assert!(!proposal_is_within_bound(bound_plus_one, Some(height(0))));
        // And the arm still admits a tip at the bound, so the gate did not become a refuse-all.
        assert!(proposal_is_within_bound(
            height(MAX_PROPOSAL_FINALITY_LAG),
            None
        ));
        assert!(proposal_is_within_bound(height(1), None));
    }

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

    #[test]
    fn round_escape_permit_is_consumed_even_when_the_round_stays_open() {
        let since = Mutex::new(None);
        let t = Instant::now();
        assert_eq!(take_round_escape(&since, t), None);
        assert_eq!(
            take_round_escape(&since, t + ROUND_STALL_ESCAPE),
            Some(15_000)
        );
        // The archived failure took many escapes inside one interval.
        // Both a duplicate tick and a burst after it must now wait.
        for ms in [0, 1, 100, 967, 3_388, 14_999] {
            assert_eq!(
                take_round_escape(&since, t + ROUND_STALL_ESCAPE + Duration::from_millis(ms)),
                None
            );
        }
        assert_eq!(
            take_round_escape(&since, t + ROUND_STALL_ESCAPE * 2),
            Some(15_000)
        );
        assert_eq!(take_round_escape(&since, t + ROUND_STALL_ESCAPE * 2), None);
        // Closing the round starts a fresh observation, never an old permit.
        *since.lock().unpoison() = None;
        assert_eq!(take_round_escape(&since, t + ROUND_STALL_ESCAPE * 10), None);
        assert_eq!(
            take_round_escape(&since, t + ROUND_STALL_ESCAPE * 11),
            Some(15_000)
        );
    }

    #[test]
    fn late_escape_and_repeated_ticks_do_not_bank_unused_permits() {
        let since = Mutex::new(None);
        let t = Instant::now();
        assert_eq!(take_round_escape(&since, t), None);
        assert_eq!(
            take_round_escape(&since, t + Duration::from_secs(90)),
            Some(90_000)
        );
        for ms in 0..15_000 {
            assert_eq!(
                take_round_escape(&since, t + Duration::from_millis(90_000 + ms)),
                None
            );
        }
        assert_eq!(
            take_round_escape(&since, t + Duration::from_secs(105)),
            Some(15_000)
        );
    }

    #[test]
    fn quiet_chain_retry_is_bounded_without_weakening_quorum() {
        let since = Mutex::new(None);
        let t = Instant::now();
        assert!(!quiet_finality_retry_due(&since, t, true, true));
        assert!(!quiet_finality_retry_due(
            &since,
            t + ROUND_STALL_ESCAPE - Duration::from_millis(1),
            true,
            true
        ));
        assert!(quiet_finality_retry_due(
            &since,
            t + ROUND_STALL_ESCAPE,
            true,
            true
        ));
        assert!(!quiet_finality_retry_due(
            &since,
            t + ROUND_STALL_ESCAPE + Duration::from_millis(1),
            true,
            true
        ));
        assert!(quiet_finality_retry_due(
            &since,
            t + ROUND_STALL_ESCAPE * 2,
            true,
            true
        ));
        // A lost quorum must not use a retry to mint a different finality rule.
        assert!(!quiet_finality_retry_due(
            &since,
            t + ROUND_STALL_ESCAPE * 3,
            false,
            true
        ));
        assert!(!quiet_finality_retry_due(
            &since,
            t + ROUND_STALL_ESCAPE * 4,
            true,
            true
        ));
        assert!(quiet_finality_retry_due(
            &since,
            t + ROUND_STALL_ESCAPE * 5,
            true,
            true
        ));
        // Explicit proposals have their own ingress and cannot arm the timer.
        assert!(!quiet_finality_retry_due(
            &since,
            t + ROUND_STALL_ESCAPE * 6,
            true,
            false
        ));
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
                pre_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
                post_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
                sig: Vec::new(),
                sig_algorithm: String::new(),
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

        let selected = select_deploys(&dag, next, per_block_deploy_budget(0), &BTreeSet::new())
            .await
            .unwrap()
            .0;
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

        // **The self-replay exclusion is a selection filter, not a pool deletion** (#564): the
        // excluded id is skipped and the budget is still filled from the next valid entry.
        let excluded = BTreeSet::from([key(0)]);
        let without = select_deploys(&dag, next, per_block_deploy_budget(0), &excluded)
            .await
            .unwrap()
            .0;
        assert!(
            !without.contains(&key(0)),
            "an excluded deploy must not be selected"
        );
        assert_eq!(
            without.len(),
            MAX_BLOCK_DEPLOYS,
            "the exclusion must not shrink the block below its budget"
        );
        assert_eq!(
            without,
            all_keys[1..=MAX_BLOCK_DEPLOYS].to_vec(),
            "the next canonical entry replaces the excluded one"
        );

        // A slash takes a seed as well, so it takes a deploy's place.
        let with_one_slash =
            select_deploys(&dag, next, per_block_deploy_budget(1), &BTreeSet::new())
                .await
                .unwrap()
                .0;
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
            select_deploys(&dag, expiry_edge, per_block_deploy_budget(0), &BTreeSet::new())
                .await
                .unwrap()
                .0,
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
        let selected_51 = select_deploys(
            &dag_51,
            height_51,
            per_block_deploy_budget(0),
            &BTreeSet::new(),
        )
        .await
        .unwrap()
        .0;
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
        let selected_52 = select_deploys(
            &dag_52,
            height_52,
            per_block_deploy_budget(0),
            &BTreeSet::new(),
        )
        .await
        .unwrap()
        .0;
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
                    ProposalMode,
                    Option<usize>,
                ) -> BoxFuture<Result<BlockCreatorResult, String>>
                + Send
                + Sync,
        > = Arc::new(move |_v, _source, _mode, _limit| {
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
                    ProposalMode,
                    Option<usize>,
                ) -> BoxFuture<Result<BlockCreatorResult, String>>
                + Send
                + Sync,
        > = Arc::new(|_v, _source, _mode, _limit| {
            Box::pin(async { Ok(BlockCreatorResult::Created(block())) })
        });
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

    /// A proposer whose own validation returns a self-replay failure (#564), sharing `tracker` so the
    /// test can inspect the bounded-retry state the arm writes.
    fn proposer_failing_self_replay(
        failure: crate::rholang::ReplayFailure,
        built: BlockMessage,
        tracker: Arc<Mutex<SelfReplayTracker>>,
    ) -> Proposer {
        let get_seq: Arc<dyn Fn(Validator) -> BoxFuture<i64> + Send + Sync> =
            Arc::new(|_v| Box::pin(async { 0i64 }));
        let check_active: Arc<
            dyn Fn(&ValidatorIdentity) -> BoxFuture<Result<bool, String>> + Send + Sync,
        > = Arc::new(|_v| Box::pin(async { Ok(true) }));
        let create_block: Arc<
            dyn Fn(
                    &ValidatorIdentity,
                    ProposeSource,
                    ProposalMode,
                    Option<usize>,
                ) -> BoxFuture<Result<BlockCreatorResult, String>>
                + Send
                + Sync,
        > = {
            let built = built.clone();
            Arc::new(move |_v, _source, _mode, _limit| {
                let built = built.clone();
                Box::pin(async move { Ok(BlockCreatorResult::Created(built)) })
            })
        };
        let validate: Arc<
            dyn Fn(&BlockMessage) -> BoxFuture<Result<(), ValidateError>> + Send + Sync,
        > = {
            let failure = failure.clone();
            Arc::new(move |_b| {
                let failure = failure.clone();
                Box::pin(async move { Err(ValidateError::SelfReplayFailed(failure)) })
            })
        };
        let effect: Arc<dyn Fn(&BlockMessage) -> BoxFuture<()> + Send + Sync> =
            Arc::new(|_b| Box::pin(async {}));
        let validator = ValidatorIdentity::from_hex(
            "67e56582298859ddae725f972992a07c6c4fb9f62a8fff58ce3ca926a1063530",
        )
        .unwrap();
        Proposer::new(
            get_seq,
            check_active,
            create_block,
            validate,
            effect,
            validator,
            Arc::new(rchain_shared::log::NopLog),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
            false,
            block_store(),
        )
        .with_self_replay_tracker(tracker)
    }

    /// A proposer that **recovers** (#564): `Normal` builds a deploy-bearing block, `AttestOnly`
    /// builds an empty one, and validation fails only the deploy-bearing block. The modes and the
    /// broadcasts are recorded so a test can assert the second build is a different input and the
    /// refused one is never broadcast. (The halt rig above returns the same block for both modes and
    /// fails both, which is the node-level fault.)
    fn proposer_recovering_a_self_replay(
        failure: crate::rholang::ReplayFailure,
        tracker: Arc<Mutex<SelfReplayTracker>>,
        modes: Arc<Mutex<Vec<ProposalMode>>>,
        broadcast: Arc<Mutex<Vec<BlockMessage>>>,
    ) -> Proposer {
        let get_seq: Arc<dyn Fn(Validator) -> BoxFuture<i64> + Send + Sync> =
            Arc::new(|_v| Box::pin(async { 0i64 }));
        let check_active: Arc<
            dyn Fn(&ValidatorIdentity) -> BoxFuture<Result<bool, String>> + Send + Sync,
        > = Arc::new(|_v| Box::pin(async { Ok(true) }));
        let create_block: Arc<
            dyn Fn(
                    &ValidatorIdentity,
                    ProposeSource,
                    ProposalMode,
                    Option<usize>,
                ) -> BoxFuture<Result<BlockCreatorResult, String>>
                + Send
                + Sync,
        > = {
            let modes = modes.clone();
            let deploy_block = block_with_deploy_sig(7);
            let attestation = block();
            Arc::new(move |_v, _source, mode, _limit| {
                modes.lock().unpoison().push(mode);
                let built = if mode == ProposalMode::AttestOnly {
                    attestation.clone()
                } else {
                    deploy_block.clone()
                };
                Box::pin(async move { Ok(BlockCreatorResult::Created(built)) })
            })
        };
        let validate: Arc<
            dyn Fn(&BlockMessage) -> BoxFuture<Result<(), ValidateError>> + Send + Sync,
        > = {
            let failure = failure.clone();
            Arc::new(move |b| {
                let failure = failure.clone();
                let carries_deploys = !b.state.deploys.is_empty();
                Box::pin(async move {
                    if carries_deploys {
                        Err(ValidateError::SelfReplayFailed(failure))
                    } else {
                        Ok(())
                    }
                })
            })
        };
        let effect: Arc<dyn Fn(&BlockMessage) -> BoxFuture<()> + Send + Sync> = {
            let broadcast = broadcast.clone();
            Arc::new(move |b| {
                broadcast.lock().unpoison().push(b.clone());
                Box::pin(async {})
            })
        };
        let validator = ValidatorIdentity::from_hex(
            "67e56582298859ddae725f972992a07c6c4fb9f62a8fff58ce3ca926a1063530",
        )
        .unwrap();
        Proposer::new(
            get_seq,
            check_active,
            create_block,
            validate,
            effect,
            validator,
            Arc::new(rchain_shared::log::NopLog),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
            false,
            block_store(),
        )
        .with_self_replay_tracker(tracker)
    }

    /// A block carrying one user deploy whose id is the signature, so the refusal arm has ids to
    /// intersect with `last_selected`.
    fn block_with_deploy_sig(sig: u8) -> BlockMessage {
        use rchain_models::casper::protocol::casper_message::{
            DeployData, PCost, ProcessedDeploy, SignedDeployData,
        };
        let mut b = block();
        b.state.deploys = vec![ProcessedDeploy {
            deploy: SignedDeployData {
                data: DeployData {
                    attachments: Vec::new(),
                    term: "Nil".to_string(),
                    timestamp: 0,
                    phlo_price: 1,
                    phlo_limit: 500_000,
                    valid_after_block_number: 0,
                    shard_id: "root".to_string(),
                },
                deployer: vec![0u8; 65],
                sig: vec![sig; 65],
                sig_algorithm: "secp256k1".to_string(),
            },
            cost: PCost { cost: 1 },
            deploy_log: Vec::new(),
            is_failed: false,
            system_deploy_error: None,
        }];
        b
    }

    /// A proposer whose validator returns an internal error — the second counted arm (#485).
    fn proposer_failing_internal() -> Proposer {
        let get_seq: Arc<dyn Fn(Validator) -> BoxFuture<i64> + Send + Sync> =
            Arc::new(|_v| Box::pin(async { 0i64 }));
        let check_active: Arc<
            dyn Fn(&ValidatorIdentity) -> BoxFuture<Result<bool, String>> + Send + Sync,
        > = Arc::new(|_v| Box::pin(async { Ok(true) }));
        let create_block: Arc<
            dyn Fn(
                    &ValidatorIdentity,
                    ProposeSource,
                    ProposalMode,
                    Option<usize>,
                ) -> BoxFuture<Result<BlockCreatorResult, String>>
                + Send
                + Sync,
        > = Arc::new(|_v, _source, _mode, _limit| {
            Box::pin(async { Ok(BlockCreatorResult::Created(block())) })
        });
        let validate: Arc<
            dyn Fn(&BlockMessage) -> BoxFuture<Result<(), ValidateError>> + Send + Sync,
        > = Arc::new(|_b| Box::pin(async { Err(ValidateError::Internal("boom".to_string())) }));
        let effect: Arc<dyn Fn(&BlockMessage) -> BoxFuture<()> + Send + Sync> =
            Arc::new(|_b| Box::pin(async {}));
        let validator = ValidatorIdentity::from_hex(
            "67e56582298859ddae725f972992a07c6c4fb9f62a8fff58ce3ca926a1063530",
        )
        .unwrap();
        Proposer::new(
            get_seq,
            check_active,
            create_block,
            validate,
            effect,
            validator,
            Arc::new(rchain_shared::log::NopLog),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
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
        assert!(
            p.self_validation_refusal.snapshot().is_none(),
            "a divergence is not a counted refusal, so it must not name one"
        );
        assert!(matches!(rx.await.unwrap(), ProposerResult::Failure { .. }));
    }

    /// **The incident's arm** (#564): a self-built block fails its own replay, and the node does
    /// **not** rebuild it. The next build is an attestation over the same pre-state with no user
    /// deploy; that is a different input by construction, it is the block that is broadcast, and the
    /// refused set stays pooled for a later pre-state. No counter is touched.
    #[tokio::test]
    async fn a_self_replay_failure_is_bounded_and_not_counted_as_divergence() {
        let tracker = Arc::new(Mutex::new(SelfReplayTracker::default()));
        let id = vec![7u8; 65];
        {
            let mut guard = tracker.lock().unpoison();
            guard.set_pooled(BTreeSet::from([id.clone()]));
            guard.set_selected(
                (0, vec![0u8; 65], vec![0u8; 32]),
                BTreeSet::from([id.clone()]),
            );
        }
        let modes = Arc::new(Mutex::new(Vec::new()));
        let broadcast = Arc::new(Mutex::new(Vec::new()));
        let p = proposer_recovering_a_self_replay(
            crate::rholang::ReplayFailure::replay_cost_mismatch(5576, 5302),
            tracker.clone(),
            modes.clone(),
            broadcast.clone(),
        );

        let (tx, _rx) = tokio::sync::oneshot::channel();
        let (result, block_opt) = p
            .propose(ProposeSource::Explicit { is_async: false }, tx)
            .await
            .expect("the recovery is a success, not an error");

        assert_eq!(result.propose_status, ProposeStatus::SelfReplayRecovered);
        let attestation = block_opt.expect("the attestation is the proposed block");
        assert!(
            attestation.state.deploys.is_empty(),
            "the recovery block carries no user deploy"
        );
        assert_eq!(
            *modes.lock().unpoison(),
            vec![ProposalMode::Normal, ProposalMode::AttestOnly],
            "one refusal, one different input — never the same block twice"
        );
        assert_eq!(
            broadcast.lock().unpoison().len(),
            1,
            "only the attestation is broadcast; the refused block never leaves"
        );
        assert!(
            broadcast.lock().unpoison()[0].state.deploys.is_empty(),
            "and the broadcast block is the attestation, not the refused block"
        );
        assert_eq!(
            p.consecutive_failures.load(Ordering::Relaxed),
            0,
            "the recovery produced a block, so it is not a counted failure"
        );
        let refusal = p
            .self_validation_refusal
            .snapshot()
            .expect("the refusal is recorded and is not cleared by the recovery");
        assert_eq!(refusal.code, "replay-cost-mismatch");
        assert_eq!(refusal.initial_cost, Some(5576));
        assert_eq!(refusal.replay_cost, Some(5302));
        assert_eq!(refusal.deploy_ids, vec![rchain_shared::base16::encode(&id)]);
        assert_eq!(refusal.deploy_count, 1);
        assert!(
            refusal.excluded,
            "the refused set is held out at the pre-state that refused it"
        );

        let guard = tracker.lock().unpoison();
        assert!(
            guard.effective_exclusion(&[0u8; 32]).contains(&id),
            "the refusing pre-state does not re-select the set"
        );
        assert!(
            guard.effective_exclusion(&[9u8; 32]).is_empty(),
            "a later pre-state is a different input and retries the set once"
        );
        assert!(
            !p.self_validation_refusal.self_replay_halted(),
            "a recovered refusal does not halt the node"
        );
    }

    /// **The attestation also fails**: a block with no user deploy still cannot replay, so the fault
    /// is node-level. The node counts it once, latches the fault, and every proposal trigger — not
    /// just this one — reads the latch (#564). This is the state the incident lacked.
    #[tokio::test]
    async fn a_self_replay_failure_with_no_pool_deploy_is_counted() {
        let tracker = Arc::new(Mutex::new(SelfReplayTracker::default()));
        let p = proposer_failing_self_replay(
            crate::rholang::ReplayFailure::replay_status_mismatch(false, true),
            block_with_deploy_sig(9),
            tracker.clone(),
        );
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let result = p
            .propose(ProposeSource::Explicit { is_async: false }, tx)
            .await;
        assert!(
            result.is_err(),
            "the attestation fallback also failed, so the node halts with an error"
        );
        assert_eq!(
            p.consecutive_failures.load(Ordering::Relaxed),
            1,
            "the node-level fault is counted"
        );
        let refusal = p
            .self_validation_refusal
            .snapshot()
            .expect("the refusal is recorded");
        assert_eq!(refusal.code, "replay-status-mismatch");
        assert!(!refusal.excluded);
        assert_eq!(p.self_validation_refusal.quarantined(), 0);
        assert!(
            p.self_validation_refusal.self_replay_halted(),
            "the fault latch is set"
        );
        assert!(
            p.self_validation_refusal
                .self_replay_halt_reason()
                .is_some_and(|r| r.contains("attestation fallback")),
            "the halt names the failed fallback"
        );

        // Every trigger arrives through `do_propose`, so the latch stops the next one too.
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let (halted, none) = p
            .propose(ProposeSource::Explicit { is_async: false }, tx)
            .await
            .expect("a halted proposer answers a status, not an error");
        assert_eq!(halted.propose_status, ProposeStatus::SelfReplayFaultHalted);
        assert!(none.is_none());
    }

    /// **The recovery attestation is licensed even when the pace rule would withhold it** (#564,
    /// F-U23-07). `nothing_to_finalize` is exactly the state a failed deploy-bearing block leaves:
    /// the normal rule withholds an empty attestation then, and `AttestOnly` must not — that is the
    /// transition the incident lacked.
    #[test]
    fn an_attestation_only_build_is_not_withheld_by_the_pace_rule() {
        assert!(
            super::suppress_attestation_for(ProposalMode::Normal, true, false, true, false, true),
            "the ordinary pace rule withholds an empty attestation when there is nothing to finalise"
        );
        assert!(
            !super::suppress_attestation_for(
                ProposalMode::AttestOnly,
                true,
                false,
                true,
                false,
                true
            ),
            "the recovery build must not be withheld by the rule it exists to bypass"
        );
        assert!(
            !super::suppress_attestation_for(ProposalMode::Normal, false, true, true, true, true),
            "the ordinary rule is unchanged for a licensed attestation"
        );
    }

    /// **The node-level fault reaches the reset exactly once and records the halt** (#564). The
    /// refusal is persisted first, the reset names one finalised anchor, and the halted stage is
    /// written after it — the ordering that lets a restart tell "already attempted" from "never
    /// tried" and stops a reset storm.
    #[tokio::test]
    async fn a_self_replay_fault_raises_one_reset_and_records_the_halt() {
        let tracker = Arc::new(Mutex::new(SelfReplayTracker::default()));
        let resets = Arc::new(Mutex::new(Vec::new()));
        let records = Arc::new(Mutex::new(Vec::new()));
        let p = proposer_failing_self_replay(
            crate::rholang::ReplayFailure::replay_status_mismatch(false, true),
            block_with_deploy_sig(9),
            tracker,
        )
        .with_self_replay_reset({
            let resets = resets.clone();
            Arc::new(move |record: SelfReplayRecord| {
                resets.lock().unpoison().push(record);
                Box::pin(async { SelfReplayResetOutcome::Raised([4u8; 32]) })
            })
        })
        .with_self_replay_persist({
            let records = records.clone();
            Arc::new(move |record: SelfReplayRecord| {
                records.lock().unpoison().push(record);
                Box::pin(async { Ok(()) })
            })
        });

        let (tx, _rx) = tokio::sync::oneshot::channel();
        assert!(
            p.propose(ProposeSource::Explicit { is_async: false }, tx)
                .await
                .is_err(),
            "the attestation also failed, so the fault is terminal for this attempt"
        );

        let resets = resets.lock().unpoison();
        assert_eq!(resets.len(), 1, "one reset attempt per fault, not a storm");
        assert_eq!(resets[0].stage, SelfReplayStage::Refused);
        assert_eq!(resets[0].code, "replay-status-mismatch");

        let records = records.lock().unpoison();
        assert_eq!(
            records.iter().map(|r| r.stage).collect::<Vec<_>>(),
            vec![SelfReplayStage::Refused, SelfReplayStage::Halted],
            "the evidence is written before the transition, and the halt after it"
        );
        assert_eq!(records[1].reset_anchor, Some([4u8; 32]));
        assert!(records[1].reset_attempted);
        assert!(p.self_validation_refusal.self_replay_halted());
    }

    /// A prior record that already attempted a reset at `anchor` (#564).
    fn prior_attempt(anchor: [u8; 32]) -> SelfReplayRecord {
        SelfReplayRecord {
            version: SELF_REPLAY_RECORD_VERSION,
            at_ms: 1,
            block_hash: [7u8; 32],
            height: 1115,
            seq_num: 3,
            code: "replay-cost-mismatch".to_string(),
            initial_cost: Some(1),
            replay_cost: Some(0),
            state_hash_matched: None,
            replayed_state_hash: None,
            deploy_ids: Vec::new(),
            deploy_count: 0,
            stage: SelfReplayStage::Halted,
            reset_anchor: Some(anchor),
            reset_attempted: true,
            body: None,
            body_len: 0,
            durable: true,
        }
    }

    /// **The one-reset-per-anchor rule**, pure (#564, law 94): attempted at *this* anchor is a
    /// recurrence; attempted at another anchor is a new anchor's one attempt.
    #[test]
    fn a_reset_at_an_anchor_already_attempted_is_refused() {
        let mut record = prior_attempt([4u8; 32]);
        assert!(reset_is_recurrence(&record, &BlockHash::new([4u8; 32])));
        assert!(
            !reset_is_recurrence(&record, &BlockHash::new([5u8; 32])),
            "a different finalised anchor has its own one attempt"
        );
        record.reset_attempted = false;
        assert!(
            !reset_is_recurrence(&record, &BlockHash::new([4u8; 32])),
            "no attempt yet means the anchor may be reset once"
        );
    }

    /// **The injector is SelfBuilt-only and its budget is one-shot** (#564). The first assertion is
    /// the witness that a peer block can never be refused by this instrument; the last is what makes
    /// the build after the recovery clean on a devnet.
    #[test]
    fn the_self_replay_injection_refuses_only_self_built_blocks_and_honours_its_limit() {
        let counter = AtomicUsize::new(0);
        assert!(
            !injection_refuses(
                SelfReplayInjection::All { limit: None },
                ValidationOrigin::Peer,
                true,
                true,
                &counter
            ),
            "the injector never touches a peer block"
        );
        assert!(
            !injection_refuses(
                SelfReplayInjection::All { limit: None },
                ValidationOrigin::SelfBuilt,
                true,
                false,
                &counter
            ),
            "no finalised anchor means the node-level rung has no safe reset to name"
        );
        assert!(injection_refuses(
            SelfReplayInjection::All { limit: None },
            ValidationOrigin::SelfBuilt,
            true,
            true,
            &counter
        ));
        assert!(
            !injection_refuses(
                SelfReplayInjection::Deploy { limit: None },
                ValidationOrigin::SelfBuilt,
                false,
                true,
                &counter
            ),
            "the attestation-only fallback has no user deploy and is spared by `deploy`"
        );
        assert!(!injection_refuses(
            SelfReplayInjection::Off,
            ValidationOrigin::SelfBuilt,
            true,
            true,
            &counter
        ));

        let counter = AtomicUsize::new(0);
        let mode = SelfReplayInjection::All { limit: Some(1) };
        assert!(injection_refuses(
            mode,
            ValidationOrigin::SelfBuilt,
            true,
            true,
            &counter
        ));
        assert!(
            !injection_refuses(mode, ValidationOrigin::SelfBuilt, true, true, &counter),
            "the one-shot budget is spent, so the post-recovery build is clean"
        );
    }

    /// **The attempt history survives the refusal record's overwrite** (#564). Without the carry, a
    /// recurrence at the same anchor would look like a first fault and reset again — the loop the
    /// automatic resume would otherwise make live.
    #[tokio::test]
    async fn the_prior_reset_attempt_is_carried_across_the_refusal_record() {
        let tracker = Arc::new(Mutex::new(SelfReplayTracker::default()));
        let records = Arc::new(Mutex::new(Vec::new()));
        let p = proposer_failing_self_replay(
            crate::rholang::ReplayFailure::replay_status_mismatch(false, true),
            block_with_deploy_sig(9),
            tracker,
        )
        .with_self_replay_load(Arc::new(|| {
            Box::pin(async { Ok(Some(prior_attempt([4u8; 32]))) })
        }))
        .with_self_replay_persist({
            let records = records.clone();
            Arc::new(move |record: SelfReplayRecord| {
                records.lock().unpoison().push(record);
                Box::pin(async { Ok(()) })
            })
        });

        let (tx, _rx) = tokio::sync::oneshot::channel();
        assert!(p
            .propose(ProposeSource::Explicit { is_async: false }, tx)
            .await
            .is_err());
        let records = records.lock().unpoison();
        assert_eq!(records[0].stage, SelfReplayStage::Refused);
        assert!(
            records[0].reset_attempted,
            "the refusal record carries the earlier attempt"
        );
        assert_eq!(records[0].reset_anchor, Some([4u8; 32]));
        assert_eq!(records[1].stage, SelfReplayStage::Halted);
        assert!(records[1].reset_attempted);
    }

    /// **A record-read error is "unknown history", not "no record"** (#564). The prior record must
    /// not be overwritten, no reset may be raised while its anchor is unknown, and the surface must
    /// not claim a refused body is durably available when nothing was written.
    #[tokio::test]
    async fn a_self_replay_record_read_error_does_not_overwrite_the_history_or_reset() {
        let tracker = Arc::new(Mutex::new(SelfReplayTracker::default()));
        let resets = Arc::new(Mutex::new(0usize));
        let records = Arc::new(Mutex::new(Vec::new()));
        let p = proposer_failing_self_replay(
            crate::rholang::ReplayFailure::replay_status_mismatch(false, true),
            block_with_deploy_sig(9),
            tracker,
        )
        .with_self_replay_load(Arc::new(|| {
            Box::pin(async { Err("the durable-record store is unavailable".to_string()) })
        }))
        .with_self_replay_reset({
            let resets = resets.clone();
            Arc::new(move |_record: SelfReplayRecord| {
                *resets.lock().unpoison() += 1;
                Box::pin(async { SelfReplayResetOutcome::Raised([4u8; 32]) })
            })
        })
        .with_self_replay_persist({
            let records = records.clone();
            Arc::new(move |record: SelfReplayRecord| {
                records.lock().unpoison().push(record);
                Box::pin(async { Ok(()) })
            })
        });

        let (tx, _rx) = tokio::sync::oneshot::channel();
        assert!(p
            .propose(ProposeSource::Explicit { is_async: false }, tx)
            .await
            .is_err());

        assert!(
            records.lock().unpoison().is_empty(),
            "an unreadable prior record must not be overwritten with a fresh `Refused`"
        );
        assert_eq!(
            *resets.lock().unpoison(),
            0,
            "no destructive reset is raised while the history is unknown"
        );
        assert!(p.self_validation_refusal.self_replay_halted());
        assert!(
            p.self_validation_refusal
                .self_replay_halt_reason()
                .is_some_and(|r| r.contains("reset history is unknown")),
            "the halt names the storage failure, not a missing anchor"
        );
        let refusal = p
            .self_validation_refusal
            .snapshot()
            .expect("the in-memory refusal is recorded");
        assert!(
            !refusal.refused_body_available,
            "a body the write did not persist is not durably available"
        );
    }

    /// **A record-write failure is reported, not papered over** (#564). The refusal evidence can be
    /// in memory, but its durable availability must be false while the store refuses the write.
    #[tokio::test]
    async fn a_failed_record_write_does_not_claim_the_body_is_durably_available() {
        let tracker = Arc::new(Mutex::new(SelfReplayTracker::default()));
        let p = proposer_failing_self_replay(
            crate::rholang::ReplayFailure::replay_status_mismatch(false, true),
            block_with_deploy_sig(9),
            tracker,
        )
        .with_self_replay_persist(Arc::new(|_record: SelfReplayRecord| {
            Box::pin(async { Err("the durable-record store is unavailable".to_string()) })
        }));

        let (tx, _rx) = tokio::sync::oneshot::channel();
        assert!(p
            .propose(ProposeSource::Explicit { is_async: false }, tx)
            .await
            .is_err());
        let refusal = p
            .self_validation_refusal
            .snapshot()
            .expect("the in-memory refusal is recorded");
        assert!(
            !refusal.refused_body_available,
            "a failed write cannot make the body durably available"
        );
        assert!(
            refusal.refused_body_len > 0,
            "the in-memory length is still reported"
        );
    }

    /// **The write gates the reset** (#564): a durable attempt admits the request, a failed write
    /// does not, and a recurrence never reaches the write at all.
    #[tokio::test]
    async fn a_reset_attempt_is_raised_only_after_its_record_is_written() {
        let anchor = BlockHash::new([4u8; 32]);
        let mut fresh = prior_attempt([9u8; 32]);
        fresh.reset_attempted = false;
        fresh.reset_anchor = None;

        let writes = Arc::new(AtomicUsize::new(0));
        let durable = record_reset_attempt(&fresh, &anchor, {
            let writes = writes.clone();
            move |_record| {
                writes.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok::<(), String>(()) })
            }
        })
        .await;
        assert!(matches!(durable, ResetAttempt::Durable));
        assert_eq!(writes.load(Ordering::SeqCst), 1);

        let failed = record_reset_attempt(&fresh, &anchor, |_record| {
            Box::pin(async { Err("disk full".to_string()) })
        })
        .await;
        assert!(
            matches!(failed, ResetAttempt::WriteFailed(ref e) if e == "disk full"),
            "a failed attempt-record write is named, and the caller raises nothing"
        );

        let mut attempted = fresh.clone();
        attempted.reset_attempted = true;
        attempted.reset_anchor = Some(*anchor.as_bytes());
        let writes = Arc::new(AtomicUsize::new(0));
        let recurrence = record_reset_attempt(&attempted, &anchor, {
            let writes = writes.clone();
            move |_record| {
                writes.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok::<(), String>(()) })
            }
        })
        .await;
        assert!(matches!(recurrence, ResetAttempt::Recurrence));
        assert_eq!(
            writes.load(Ordering::SeqCst),
            0,
            "a recurrence is decided before any write"
        );
    }

    /// The tracker's identity rule: a run is per `(pre-state, deploy set, failure kind)`. A different
    /// set or kind starts again; a **different pre-state is a different input**, so the set is
    /// suspect only at the pre-state that refused it and a second pre-state quarantines it. A
    /// success clears the run, and an exclusion survives the clear.
    #[test]
    fn the_tracker_counts_only_identical_sets_and_keeps_exclusions_on_success() {
        let mut tracker = SelfReplayTracker::default();
        let a: BTreeSet<DeployId> = BTreeSet::from([vec![1u8]]);
        let b: BTreeSet<DeployId> = BTreeSet::from([vec![2u8]]);
        let pre_a = [1u8; 32];
        let pre_b = [2u8; 32];

        assert_eq!(
            tracker.record_refusal(&pre_a, &a, "replay-cost-mismatch").0,
            1
        );
        assert_eq!(
            tracker.record_refusal(&pre_a, &a, "replay-cost-mismatch").0,
            2,
            "the same pre-state, set and kind is one run"
        );
        assert_eq!(
            tracker.record_refusal(&pre_a, &b, "replay-cost-mismatch").0,
            1,
            "a different deploy set is a different attempt"
        );
        assert_eq!(
            tracker
                .record_refusal(&pre_a, &a, "replay-status-mismatch")
                .0,
            1,
            "a different failure kind is a different attempt"
        );

        // `a` was first refused at `pre_a`: it is held out there, not at `pre_b`.
        assert!(tracker.effective_exclusion(&pre_a).contains(&vec![1u8]));
        assert!(
            tracker.effective_exclusion(&pre_b).is_empty(),
            "a later pre-state is a different input and retries the set once"
        );
        let (_, refused_elsewhere) = tracker.record_refusal(&pre_b, &a, "replay-cost-mismatch");
        assert!(
            refused_elsewhere,
            "a second pre-state is what moves the set from suspect to quarantined"
        );
        tracker.exclude(&a);
        assert!(
            tracker.excluded().contains(&vec![1u8]),
            "the quarantine holds the id out at every pre-state"
        );

        tracker.set_pooled(BTreeSet::from([vec![1u8]]));
        assert_eq!(tracker.prune(), 1, "an id still in the pool stays held");
        tracker.set_pooled(BTreeSet::new());
        assert_eq!(
            tracker.prune(),
            0,
            "an id that left the pool is no longer held"
        );

        // A success clears the run but must not put an unrebuildable deploy back into selection.
        let mut after_success = SelfReplayTracker::default();
        after_success.exclude(&a);
        after_success.clear_attempts();
        assert_eq!(
            after_success
                .record_refusal(&pre_a, &a, "replay-cost-mismatch")
                .0,
            1
        );
        assert!(after_success.excluded().contains(&vec![1u8]));

        // The cap is a fault condition, not a reason to grow for ever.
        let mut capped = SelfReplayTracker::default();
        let many: BTreeSet<DeployId> = (0..=MAX_SELF_REPLAY_EXCLUSIONS)
            .map(|i| vec![(i & 0xff) as u8, (i >> 8) as u8])
            .collect();
        let (held, overflow) = capped.exclude(&many);
        assert!(overflow, "the cap must be reported, not silently exceeded");
        assert_eq!(held, MAX_SELF_REPLAY_EXCLUSIONS as u64);
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

    /// **#485: the counted refusal carries *why* and *which block*.** The counter alone could not
    /// tell an operator that the height-3122 wedge was a justification regression; only the log line
    /// could. This asserts the diagnostic names it, without parsing stderr.
    #[tokio::test]
    async fn a_self_validation_refusal_records_its_code_and_block() {
        let p =
            proposer_failing_validation(crate::block_status::BlockStatus::JustificationRegression);
        assert!(
            p.self_validation_refusal.snapshot().is_none(),
            "nothing stands before the attempt"
        );
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let _ = p
            .propose(ProposeSource::Explicit { is_async: false }, tx)
            .await;
        let refusal = p
            .self_validation_refusal
            .snapshot()
            .expect("the refusal is recorded");
        assert_eq!(refusal.code, "justification-regression");
        assert_eq!(refusal.block_number, 0);
        assert_eq!(refusal.seq_num, 0);
        assert_eq!(p.consecutive_failures.load(Ordering::Relaxed), 1);
    }

    /// The same success that clears the counter clears the reason: "consecutive" is the whole
    /// quantity, so a recovered node must not keep naming the failure it recovered from.
    #[tokio::test]
    async fn a_successful_propose_clears_the_refusal() {
        let p = proposer(BlockCreatorResult::Created(block()));
        p.self_validation_refusal
            .note("justification-regression", 0, 0);
        assert!(p.self_validation_refusal.snapshot().is_some());
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let _ = p
            .propose(ProposeSource::Explicit { is_async: false }, tx)
            .await;
        assert!(
            p.self_validation_refusal.snapshot().is_none(),
            "a successful propose clears the reason beside the counter"
        );
    }

    /// The internal-error arm has no `BlockStatus`, so it records its own closed code.
    #[tokio::test]
    async fn an_internal_error_records_internal_error() {
        let p = proposer_failing_internal();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let _ = p
            .propose(ProposeSource::Explicit { is_async: false }, tx)
            .await;
        let refusal = p.self_validation_refusal.snapshot().expect("recorded");
        assert_eq!(refusal.code, "internal-error");
        assert_eq!(p.consecutive_failures.load(Ordering::Relaxed), 1);
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
                    ProposalMode,
                    Option<usize>,
                ) -> BoxFuture<Result<BlockCreatorResult, String>>
                + Send
                + Sync,
        > = Arc::new(|_v, _source, _mode, _limit| {
            Box::pin(async { Ok(BlockCreatorResult::Created(block())) })
        });
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
        let refusal = SelfValidationRefusalSink::default();

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
        )
        .with_self_validation_refusal(refusal.clone());

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
        assert!(
            refusal.snapshot().is_none(),
            "a self-equivocation is not a counted self-validation refusal"
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

/// Retry an outstanding finality obligation when the DAG's height stops moving.
///
/// Once the three-window attestation horizon expires, a quorum can be alive but
/// unable to close the fringe: the tip stays fixed, and height-only cadence can
/// never become due. This is a per-proposer monotonic *scheduling* hint. It is
/// never used in the finality algorithm or in the bonded-stake denominator.
/// The caller must require an actual unfinalized transition and reachable quorum.
/// Re-entering a prompt/deploy-bearing round clears the timer so it never
/// accidentally licenses an attestation to an idle chain.
fn quiet_finality_retry_due(
    since: &Mutex<Option<Instant>>,
    now: Instant,
    pending_and_reachable: bool,
    automatic: bool,
) -> bool {
    let mut slot = since.lock().unpoison();
    if !pending_and_reachable || !automatic {
        *slot = None;
        return false;
    }
    match *slot {
        None => {
            *slot = Some(now);
            false
        }
        Some(first) if now.saturating_duration_since(first) >= ROUND_STALL_ESCAPE => {
            *slot = Some(now);
            true
        }
        Some(_) => false,
    }
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

/// Whether a block carries execution work or a rejection decision not yet
/// covered by the published fringe. A merge-only decision also needs finality.
fn block_needs_finalization(
    block: &BlockMessage,
    finalized_rejections: &BTreeSet<Vec<u8>>,
) -> bool {
    !block.state.system_deploys.is_empty()
        || !block.state.deploys.is_empty()
        || !block.rejected_deploys.is_subset(finalized_rejections)
}

/// The guard's two inputs, from the **newest work-bearing block this node has seen that the fringe it is
/// building on does not yet cover**. The height passed in is the work's **age in heights**, measured
/// from this node's first observation of it (#583), not the work's own height.
///
/// - `nothing_to_finalize`: there is no such block.
/// - `new_state_transition`: there is one, and its first observation is within
///   [`ATTESTATION_HORIZON`] heights of the tip.
///
/// The input is an age rather than a height so that a fail-closed store error can be represented at
/// every tip, including below the horizon.
///
/// **The licence used to be "a parent carries deploys", and that is AUDIT C209.** With no autopropose the
/// only proposal attempts are the deploy itself and the attestation tap, and each round's parents are the
/// snapshot taken when the previous round closed. One round after a deploy no parent carries it any more,
/// so every validator was paced to its cadence, the cadence is never due on a chain that is not moving,
/// and the fringe stopped one layer short of the deploy: measured live (#214, criterion 1: 693 samples,
/// finality `none` throughout) and in-process. A deploy-bearing block needs every bonded sender to speak
/// for a few rounds after it, not one, and this is what licenses that.
fn attestation_inputs(newest_unfinalized_age: Option<i64>) -> (bool, bool) {
    match newest_unfinalized_age {
        None => (true, false),
        Some(age) => (false, age <= ATTESTATION_HORIZON),
    }
}

/// **The age of the prompt-attestation licence** (#583): heights from this node's first observation
/// of the newest unfinalised work to the current tip.
///
/// The recorded first-seen tip is never rewritten while that work is unfinalised. An advancing
/// fringe therefore cannot renew it, and the store is durable, so a restart cannot either. An absent
/// row is a first observation; a read or write error is **fail-closed** (an age past the horizon),
/// which withholds promptness only — the cadence path still attests when a quorum is reachable
/// (W-N9-01). The work's own height is not a substitute: #223's returning validator observed its
/// boundary work twenty-three heights late and had to be licensed immediately.
///
/// The scan takes the **newest** work-bearing block, not the oldest: an expired side-branch block
/// must not mask newer work that is still inside its own opportunity, which would be premature
/// quiet finality. Each work identity is still aged once and for ever; a fresh sighting is genuinely
/// new work, not the same work renewed.
async fn work_licence_age(
    work_seen: &WorkSeenStore,
    newest_work: Option<&(BlockHash, BlockHeight)>,
    tip: BlockHeight,
    log: &Arc<dyn Log>,
) -> Option<i64> {
    let (hash, _height) = newest_work?;
    let source = LogSource::new("casper.blocks.Proposer");
    let first_seen = match work_seen.get(std::slice::from_ref(hash)).await {
        Ok(mut rows) => match rows.pop().flatten() {
            Some(seen) if seen >= 0 => BlockHeight::try_from(seen).ok(),
            Some(seen) => {
                log.warn(
                    source,
                    &format!(
                        "work {} has a negative first-seen height ({seen}); withholding the prompt \
                         attestation",
                        hash.to_hex()
                    ),
                );
                None
            }
            None => match work_seen.put(&[(hash.clone(), i64::from(tip))]).await {
                Ok(()) => Some(tip),
                Err(e) => {
                    log.warn(
                        source,
                        &format!(
                            "could not record the first sighting of work {}: {e}; withholding the \
                             prompt attestation (fail-closed)",
                            hash.to_hex()
                        ),
                    );
                    None
                }
            },
        },
        Err(e) => {
            log.warn(
                source,
                &format!(
                    "could not read the first sighting of work {}: {e}; withholding the prompt \
                     attestation (fail-closed)",
                    hash.to_hex()
                ),
            );
            None
        }
    };
    match first_seen {
        Some(first) => Some(liveness::heights_behind(tip, first)),
        // Fail-closed: an age past the horizon, representable at every tip. (A synthetic *height*
        // would clamp to zero below the horizon and read as licensed.)
        None => Some(ATTESTATION_HORIZON + 1),
    }
}

/// Delete first-seen rows for work at or below the last finalized block.
///
/// The sweep runs only when finality advances, so the map stays bounded by the unfinalized region.
/// Work below finality cannot re-enter the unfinalized region without a safety violation, so dropping
/// its row cannot renew a licence.
async fn prune_work_seen(
    work_seen: &WorkSeenStore,
    dag_repr: &DagRepresentation,
    last_pruned: &Mutex<Option<BlockHash>>,
    log: &Arc<dyn Log>,
) {
    let source = LogSource::new("casper.blocks.Proposer");
    let Some(finalized) = dag_repr.last_finalized_block_hash() else {
        return;
    };
    {
        let mut guard = last_pruned.lock().unpoison();
        if guard.as_ref() == Some(&finalized) {
            return;
        }
        *guard = Some(finalized.clone());
    }
    let Some(finalized_height) = dag_repr
        .dag_message_state
        .msg_map
        .get(&finalized)
        .map(|m| m.height)
    else {
        return;
    };
    let keys = match work_seen.keys().await {
        Ok(keys) => keys,
        Err(e) => {
            log.warn(
                source,
                &format!("could not read the work-seen store to prune it: {e}"),
            );
            return;
        }
    };
    let stale: Vec<BlockHash> = keys
        .into_iter()
        .filter(|hash| {
            dag_repr
                .dag_message_state
                .msg_map
                .get(hash)
                .is_some_and(|m| m.height <= finalized_height)
        })
        .collect();
    if stale.is_empty() {
        return;
    }
    if let Err(e) = work_seen.delete(&stale).await {
        log.warn(
            source,
            &format!("could not prune {} work-seen rows: {e}", stale.len()),
        );
    }
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
/// **The pace rule with the recovery mode's override** (#564). `AttestOnly` exists to move this
/// validator's latest message after a refusal, so the empty-block storm's rule must not withhold it;
/// every other mode keeps `attestation_suppressed` unchanged. Extracted so the override is a
/// function a test drives, not an `if` buried in `create_block` (F-U23-07).
fn suppress_attestation_for(
    mode: ProposalMode,
    nothing_to_finalize: bool,
    new_state_transition: bool,
    quorum_reachable: bool,
    cadence_due: bool,
    paced: bool,
) -> bool {
    mode != ProposalMode::AttestOnly
        && attestation_suppressed(
            nothing_to_finalize,
            new_state_transition,
            quorum_reachable,
            cadence_due,
            paced,
        )
}

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
            pre_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
            post_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
            sig: Vec::new(),
            sig_algorithm: String::new(),
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

    #[test]
    fn merge_only_rejections_need_finality_until_the_fringe_covers_them() {
        let mut merged = block(&identity());
        let a = vec![0xa1];
        let b = vec![0xb2];
        merged.rejected_deploys = [a.clone(), b.clone()].into_iter().collect();
        // Differential witness for the former deploy-only guard.
        assert!(merged.state.deploys.is_empty() && merged.state.system_deploys.is_empty());
        assert!(super::block_needs_finalization(
            &merged,
            &Default::default()
        ));
        for covered in [a.clone(), b.clone()] {
            let partial = [covered].into_iter().collect();
            assert!(super::block_needs_finalization(&merged, &partial));
        }
        // A repeated ledger is not fresh work after finalization.
        assert!(!super::block_needs_finalization(
            &merged,
            &merged.rejected_deploys
        ));
        let superset = [a, b, vec![0xc3]].into_iter().collect();
        assert!(!super::block_needs_finalization(&merged, &superset));
        merged.rejected_deploys.clear();
        assert!(!super::block_needs_finalization(
            &merged,
            &Default::default()
        ));
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
                    super::ProposalMode,
                    Option<usize>,
                ) -> super::BoxFuture<Result<super::BlockCreatorResult, String>>
                + Send
                + Sync,
        > = {
            let created = created.clone();
            Arc::new(move |_v, _s, _mode, _limit| {
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
            pre_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
            post_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
            sig: Vec::new(),
            sig_algorithm: String::new(),
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
            pre_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
            post_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
            sig: Vec::new(),
            sig_algorithm: String::new(),
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

/// **The #583 falsifier: work is licensed from its first observation, not from the moving fringe.**
///
/// The #223 live shape is the positive control: work first seen at tip 33 is licensed at once and at
/// the horizon boundary. Advancing the tip past one horizon from that observation — with the fringe
/// still three heights behind — must end the licence, and re-observing the same work at a later tip
/// must not renew it.
#[cfg(test)]
mod attestation_age_tests {
    use super::{attestation_inputs, work_licence_age, ATTESTATION_HORIZON};
    use rchain_block_storage::work_seen::{self, WorkSeenStore};
    use rchain_models::block_hash::BlockHash;
    use rchain_shared::log::{Log, NopLog};
    use rchain_shared::refined::BlockHeight;
    use rchain_shared::store_manager::InMemoryStoreManager;
    use rchain_shared::typed_store::KeyValueTypedStore;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn h(n: i64) -> BlockHeight {
        BlockHeight::try_from(n).expect("height")
    }

    fn hash(byte: u8) -> BlockHash {
        BlockHash::from_slice(&[byte; 32])
    }

    async fn licence(
        store: &WorkSeenStore,
        work: &(BlockHash, BlockHeight),
        tip: BlockHeight,
        log: &Arc<dyn Log>,
    ) -> (bool, bool) {
        attestation_inputs(work_licence_age(store, Some(work), tip, log).await)
    }

    #[tokio::test]
    async fn late_work_is_licensed_from_its_first_observation_and_expires_at_the_horizon() {
        let store = work_seen::create(&InMemoryStoreManager::default())
            .await
            .expect("open the work-seen store");
        let log: Arc<dyn Log> = Arc::new(NopLog);
        // The #223 live shape: the work is at its own height 10, and this node first saw it at tip
        // 33. The store is the record; the fringe is not an input any more.
        let work = (hash(1), h(10));
        store
            .put(&[(work.0.clone(), 33)])
            .await
            .expect("record the first sighting");

        assert_eq!(
            licence(&store, &work, h(33), &log).await,
            (false, true),
            "work first seen at 33 is licensed immediately"
        );
        assert_eq!(
            licence(&store, &work, h(33 + ATTESTATION_HORIZON), &log).await,
            (false, true),
            "and at the horizon boundary"
        );

        // Past one horizon from the observation the licence ends; advancing the tip is exactly what
        // the old fringe floor used to convert into renewal (W-N9-07).
        for tip in [33 + ATTESTATION_HORIZON + 1, 97, 997] {
            assert_eq!(
                licence(&store, &work, h(tip), &log).await,
                (false, false),
                "work first seen at 33 is still licensed at tip {tip}"
            );
        }
        // Re-observing the same work at a later tip does not rewrite the first-seen height.
        assert_eq!(licence(&store, &work, h(1000), &log).await, (false, false));

        // The positive control: different work first seen at 49 is licensed at 49, even though its
        // own height is 10. This is the C211/C214 late-work path, kept alive within its window.
        let fresh = (hash(2), h(10));
        store
            .put(&[(fresh.0.clone(), 49)])
            .await
            .expect("record the fresh sighting");
        assert_eq!(licence(&store, &fresh, h(49), &log).await, (false, true));
    }

    /// **An expired side branch does not mask newer work** (#583): the licence is decided by the
    /// newest work-bearing block, so old work outside its window cannot withhold promptness for work
    /// that is still inside its own.
    #[tokio::test]
    async fn expired_work_does_not_mask_newer_work() {
        let store = work_seen::create(&InMemoryStoreManager::default())
            .await
            .expect("open the work-seen store");
        let log: Arc<dyn Log> = Arc::new(NopLog);
        let old = (hash(1), h(10));
        let newer = (hash(2), h(35));
        store.put(&[(old.0.clone(), 10)]).await.expect("record old");
        store
            .put(&[(newer.0.clone(), 35)])
            .await
            .expect("record newer");
        // Newest-at-height wins the scan; its own first observation is 35, so at tip 40 it is five
        // heights old and licensed.
        assert_eq!(licence(&store, &newer, h(40), &log).await, (false, true));
        // The old work alone is 30 heights old at that tip and would withhold.
        assert_eq!(licence(&store, &old, h(40), &log).await, (false, false));
    }

    /// **A store that cannot answer is fail-closed.** An unreadable first-seen row must not be read
    /// as a fresh sighting; that would turn a local store error into exactly the renewal the fix
    /// removes. Promptness is withheld, and the cadence path is unaffected.
    #[tokio::test]
    async fn an_unreadable_store_withholds_the_prompt_licence() {
        struct FailingStore;

        #[async_trait::async_trait]
        impl KeyValueTypedStore<BlockHash, i64> for FailingStore {
            async fn get(&self, _keys: &[BlockHash]) -> Result<Vec<Option<i64>>, String> {
                Err("unreadable".to_string())
            }
            async fn put(&self, _pairs: &[(BlockHash, i64)]) -> Result<(), String> {
                Err("unwritable".to_string())
            }
            async fn delete(&self, _keys: &[BlockHash]) -> Result<usize, String> {
                Err("unwritable".to_string())
            }
            async fn contains(&self, _keys: &[BlockHash]) -> Result<Vec<bool>, String> {
                Err("unreadable".to_string())
            }
            async fn to_map(&self) -> Result<BTreeMap<BlockHash, i64>, String> {
                Err("unreadable".to_string())
            }
        }

        let store: WorkSeenStore = Arc::new(FailingStore);
        let log: Arc<dyn Log> = Arc::new(NopLog);
        let work = (hash(1), h(10));
        assert_eq!(
            licence(&store, &work, h(50), &log).await,
            (false, false),
            "a failed read is fail-closed, not a fresh sighting"
        );
        // The edge the age input exists for: below the horizon, a synthetic "expired" *height*
        // would clamp to zero and read as licensed.
        assert_eq!(
            licence(&store, &work, h(5), &log).await,
            (false, false),
            "a failed read withholds promptness at a low tip too"
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
    use super::{attestation_inputs, attestation_suppressed, ATTESTATION_HORIZON};
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
        // **First observation per work block** (#583): the height at which it first entered the
        // conflict set. It is never rewritten, so no later step can renew the licence.
        let mut first_seen: BTreeMap<u64, BlockHeight> = BTreeMap::new();
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
                let work: Vec<u64> = conflict
                    .iter()
                    .filter(|h| with_deploy.contains(h))
                    .copied()
                    .collect();
                for h in &work {
                    first_seen.entry(*h).or_insert(work_tip);
                }
                // The licence ages from the first observation of the **newest** work-bearing block
                // (#583), so an expired side branch cannot mask work still inside its opportunity.
                let newest_first_seen = work
                    .iter()
                    .max_by_key(|h| st.msg_map[h].height)
                    .map(|h| first_seen[h]);
                let oldest_pending = work.iter().map(|h| st.msg_map[h].height).min();
                if let Some(o) = oldest_pending {
                    max_age = max_age.max(liveness::heights_behind(work_tip, o));
                }
                let (nothing_to_finalize, new_state_transition) = match cfg.view {
                    View::Round => (
                        nothing_to_finalize,
                        parents.iter().any(|p| with_deploy.contains(&p.id)),
                    ),
                    View::SeenWork | View::Seen => attestation_inputs(
                        newest_first_seen.map(|first| liveness::heights_behind(work_tip, first)),
                    ),
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
        let age = |behind: i64| Some(behind);
        assert_eq!(
            attestation_inputs(None),
            (true, false),
            "nothing seen is unfinalised"
        );
        assert_eq!(attestation_inputs(age(0)), (false, true));
        assert_eq!(attestation_inputs(age(ATTESTATION_HORIZON)), (false, true));
        assert_eq!(
            attestation_inputs(age(ATTESTATION_HORIZON + 1)),
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
