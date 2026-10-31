//! Interpreter utilities (port of `rholang/InterpreterUtil.scala`).

use rchain_shared::lock::Unpoison;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use rchain_block_storage::block_store::BlockStore;
use rchain_block_storage::dag::dag_storage::{BlockDagStorage, HealthCounters, ResetMarker};
use rchain_block_storage::dag::finalizer::NoAdvance;
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
use rchain_models::ast::Par;
use rchain_models::block::state_hash::StateHash;
use rchain_models::block_hash::BlockHash;
use rchain_models::block_metadata::{BlockMetadata, FailureCause, SlashSeverity};
use rchain_models::casper::protocol::casper_message::{BlockMessage, SignedDeployData};
use rchain_models::validator::Validator;
use rchain_rholang::errors::RholangError;
use rchain_rholang::runtime::ReplayRhoRuntime;
use rchain_rholang::system_processes::BlockData;
use rchain_shared::base16;
use rchain_shared::log::{Log, LogSource};
use rchain_shared::refined::BlockHeight;

use crate::block_random_seed::BlockRandomSeed;
use crate::block_status::BlockStatus;
use crate::merging::{BlockIndex, ParentsMergedState};
use crate::multi_parent_casper::get_pre_state_for_parents;
use crate::rholang::{ReplayFailure, SystemDeployRuntimeResult, UserDeployRuntimeResult};
use crate::runtime_manager::RuntimeManager;
use crate::system_deploy::SystemDeploy;

/// Parse + normalize a rholang term (port of `mkTerm`).
pub fn mk_term(rho: &str, env: &BTreeMap<String, Par>) -> Result<Par, RholangError> {
    rchain_rholang::normalizer::source_to_adt_with_env(rho, env).map(Par::from)
}

/// Replay a block's deploys and return the computed state hash (port of `replayBlock`). The replay
/// mutates only `replay_runtime` (a per-block fork); the mergeable-channel save uses `runtime`.
/// `fringe_state_hash` is the state hash of the last finalised fringe as of `block` — the value its
/// close system deploy anchored the next epoch's seed to. The caller computes it from the DAG
/// (`pre_state.fringe_state`) rather than reading it off the block, because the block's header does
/// not carry it: a field the validator recomputes anyway is a second derivation waiting to fall out
/// of step, and one that a proposer would be choosing if it were on the wire.
pub async fn replay_block(
    runtime: &RuntimeManager,
    replay_runtime: &ReplayRhoRuntime,
    block: &BlockMessage,
    fringe_state_hash: &Blake2b256Hash,
    participation: &BTreeMap<Validator, BlockHeight>,
    rand: &Blake2b512Random,
) -> Result<Blake2b256Hash, ReplayFailure> {
    let start_hash = Blake2b256Hash::from_byte_array(block.pre_state_hash.as_bytes());
    let block_data = BlockData::from_block(block);
    let with_cost_accounting = !block.justifications.is_empty();
    let (state_hash, _mergeable) = runtime
        .replay_compute_state_with(
            replay_runtime,
            &start_hash,
            &block.state.deploys,
            &block.state.system_deploys,
            rand,
            block_data,
            fringe_state_hash,
            participation,
            with_cost_accounting,
            // Genesis PoS descriptors (pool/trusted/params) come from the network's genesis
            // configuration; the trie is authoritative for every non-genesis block, so this value is
            // only consumed on the genesis replay path.
            runtime.genesis_pos(),
            // The genesis vault balances, and only for the genesis: they are not carried on the block
            // (they are installed at genesis) and the pre-state of a later block already holds them,
            // so re-installing there would clobber post-genesis balances (AUDIT C46).
            if is_genesis_pre_state(&start_hash) {
                runtime.genesis_vaults()
            } else {
                &[]
            },
        )
        .await?;
    Ok(state_hash)
}

/// Map a replay result into an `Option` of the matching state hash (port of `handleErrors`).
pub fn handle_errors(
    ts_hash: &Blake2b256Hash,
    result: Result<Blake2b256Hash, ReplayFailure>,
) -> Result<Option<Blake2b256Hash>, String> {
    match result {
        Ok(computed) => {
            if *ts_hash == computed {
                Ok(Some(computed))
            } else {
                Ok(None)
            }
        }
        Err(ReplayFailure::InternalError(cause)) => Err(format!(
            "Internal errors encountered while processing deploy: {cause}"
        )),
        Err(_) => Ok(None),
    }
}

/// Compute the post-state + processed deploys from a deploy sequence (port of
/// `computeDeploysCheckpoint`).
#[allow(clippy::too_many_arguments)]
pub async fn compute_deploys_checkpoint(
    runtime: &RuntimeManager,
    deploys: &[SignedDeployData],
    system_deploys: &[SystemDeploy],
    rand: &Blake2b512Random,
    block_data: BlockData,
    pre_state_hash: &Blake2b256Hash,
    fringe_state_hash: &Blake2b256Hash,
    batch_limit: Option<usize>,
) -> Result<
    (
        Blake2b256Hash,
        Vec<UserDeployRuntimeResult>,
        Vec<SystemDeployRuntimeResult>,
    ),
    String,
> {
    runtime
        .compute_state_with_limit(
            pre_state_hash,
            deploys,
            system_deploys,
            rand,
            block_data,
            fringe_state_hash,
            batch_limit,
        )
        .await
}

/// The hard-coded empty-state (genesis pre-state) hash (port of `emptyStateHashFixed`).
///
/// Recomputed after the native system-process install + de-blessed genesis (the empty state is now
/// "system processes installed + empty native state", with no registry-bootstrap echo).
pub fn empty_state_hash_fixed() -> Blake2b256Hash {
    Blake2b256Hash::from_byte_array(&base16::unsafe_decode(
        "0e5751c026e543b2e8ab2eb06099daa1d1e5df47778f7787faab45cdf12fe3a8",
    ))
}

/// Whether `pre_state_hash` is the genesis block's pre-state — the empty state.
///
/// This is the one replay that must re-install the network's genesis descriptors (the native PoS
/// state and the initial REV vault balances), because `compute_genesis` installs both as native
/// state *outside* the block's deploys, so no deploy in the genesis recreates them (AUDIT C46).
/// Every other block's pre-state already carries those balances, so re-installing there would
/// clobber a post-genesis one — the condition is what keeps the re-install from being unconditional.
pub fn is_genesis_pre_state(pre_state_hash: &Blake2b256Hash) -> bool {
    *pre_state_hash == empty_state_hash_fixed()
}

/// Does this block slash anyone its own justifications do not hold responsible (AUDIT C110)?
///
/// **Why the check lives here and not in the pure-check list.** The pure checks read the block and
/// nothing else, and the *evidence* for a slash is not in the block: `BlockMessage.justifications` is
/// a list of `BlockHash`, and the `slashable` flag is `BlockMetadata`'s — in memory only, explicitly
/// "neither [] carried in the protobuf" (see its doc). So a validator can only ask the question
/// against **its own DAG view**, which means the answer depends on what *this node* observed about
/// those justifications, and that is exactly the property wanted: the proposer's opinion of the victim
/// carries no weight, and a node that never saw the offending block cannot be made to rubber-stamp
/// the punishment.
///
/// A block with no slashes short-circuits before any lookup, so the cost is paid only by blocks that
/// take someone's stake. A justification this node does not have is simply absent from the metadata
/// list — it then contributes no offender, so an unknown justification cannot license a slash, which
/// is the safe direction.
async fn slash_is_unjustified(
    dag: &dyn BlockDagStorage,
    block: &BlockMessage,
) -> Result<bool, String> {
    let slashed = crate::validate::slashed_validators(block);
    if slashed.is_empty() {
        return Ok(false);
    }
    let mut metadata = Vec::with_capacity(block.justifications.len());
    for j in &block.justifications {
        if let Some(meta) = dag.lookup(j).await? {
            metadata.push(meta);
        }
    }
    let justified = crate::validate::slashable_senders(&metadata);
    let evidence = crate::validate::slashed_evidence(block);
    // **A victim must be justified one of two ways, and both are re-derived here** (AUDIT C199, C200).
    //
    // The tier is *checked, not taken*: a victim must be an offender **at the tier this node derives**,
    // or a proposer would size the confiscation freely and the tiers would be advice. A slash recorded
    // before the tiers existed carries `Unspecified` — which takes everything — and a legacy record
    // derives the same, so an old block replays unchanged.
    //
    // The second way is evidence: a block the proposer attaches as proof of an **equivocation**, which
    // this node re-checks against **its own DAG** rather than believing. That is what lets the one
    // unambiguous Byzantine fault be punished without any node having to store the refused block.
    for (victim, tier) in &slashed {
        if justified.get(victim) == Some(tier) {
            continue;
        }
        let Some(bytes) = evidence.get(victim) else {
            return Ok(true);
        };
        if !crate::validate::equivocation_is_proved(dag, victim, bytes).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The tip height at which a finality stall was last logged. A `static` because the alternative is
/// threading a counter through every call for one log line — and the gate itself is stateless.
///
/// `i64::MIN` is the "never logged" sentinel, and the comparison is a `saturating_sub` for that reason:
/// plain subtraction would overflow on the first stall of a process's life, which is a panic in debug
/// and a wrapped negative in release — a rate limiter that never fires, on the path it exists for.
static LAST_STALL_LOG: AtomicI64 = AtomicI64::new(i64::MIN);
/// The last stall *kind* logged, so a **change of reason** is reported immediately.
///
/// **Why this exists, and it is the reason the discriminator has never been read.** The gate was
/// height-only — one line per `STALL_LOG_INTERVAL` (100) heights — and a short run's tip never leaves the
/// low thirties, so the *only* line a devnet can emit is the genesis-tip one. The 2026-09-30 campaign
/// measured finality pinning in 3 of 3 attempts and could not say why, because the instrument that
/// explains it was gated out of range. A stall that changes reason is the event worth reporting; the
/// height gate stays as the rate limit for a stall that does *not* change.
static LAST_STALL_DESC: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// How many heights may pass between two stall lines: one per hundred blocks keeps a stall that lasts
/// hundreds of blocks to a readable number of lines (#70's measurement ran ~160 blocks at finality 44).
const STALL_LOG_INTERVAL: i64 = 100;

/// **One shard's health cells** (W-N10-02, W-N10-03, C249's F-U10-01).
///
/// **Why one cell per shard rather than the process-global statics these once were.** The surface
/// answered a *node-wide* question with a *shard-local* fact: a refusal in one shard appeared in every
/// shard's finality health (W-N10-03), and a restart zeroed counters whose events are the shard's own
/// history (W-N10-02). It is threaded exactly as `ProposeHealth` is — created in `setup_shard_runtime`
/// beside it, handed into `setup_shard`, held by `BlockApiImpl` beside its `shard_id`, read as a field.
///
/// **Not a registry, and the reason is the reset lane's own.** `RESET_REQUEST`/`RESET_WAKE` are keyed by
/// `ShardLane` because the reset *detector* is shard-blind — it has the id and nothing else. Every
/// writer here is shard-*bearing* and already holds the shard it is validating (`validate_checks` takes
/// `shard_id`), so a process-wide map would put a lock on the block-validation path to re-derive a key
/// that was about to be dropped anyway.
#[derive(Clone, Debug, Default)]
pub struct ShardHealth {
    /// The last finality verdict this shard observed, for the **machine-readable** surface — C249's
    /// F-U10-01, and the reason this cell is not `LAST_STALL_DESC`.
    ///
    /// That one is the *log's* rate limiter: it holds the last line it let through, and it is only
    /// written when the limit allows one. A surface reading it would report a stale reason for up to
    /// `STALL_LOG_INTERVAL` heights — precisely while an operator is looking. This is written on
    /// **every** observation, so it is the current verdict or nothing at all, and an observation that
    /// advanced clears it.
    ///
    /// **It is the per-observation verdict, not an alarm.** On a producing chain most blocks are
    /// refused by the stake gate, because a block justifies only the messages that existed when it was
    /// built (#318); what makes a *stall* is that the refusals keep coming, which is
    /// [`Self::finality_stall_sustained`] and [`Self::stall_episodes`]. The description on the wire says
    /// so; before #318 it read as "why finality is not advancing", which made a healthy net look broken.
    /// Every field is an `Arc`, so `Clone` **shares the cells** rather than copying them — the same
    /// property `ProposeHealth` relies on, and the reason a cloned handle is the same shard's health.
    current_stall_reason: Arc<std::sync::Mutex<Option<String>>>,
    /// Refusals seen **consecutively** since the derivation last advanced. Reset by an advance, and by a
    /// verdict that is not a refusal at all (see [`ShardHealth::note_finality_stall`]).
    consecutive_refusals: Arc<AtomicU64>,
    /// Whether the run above has reached the threshold — i.e. whether this shard is stalled **now**, as
    /// opposed to having been refused on the block it is validating this instant.
    current_stall_sustained: Arc<AtomicBool>,
    /// How many times this shard has **entered** a finality stall (a not-stalled → stalled transition),
    /// monotone, where "stalled" is the sustained state [`stall_threshold`] defines and not a single
    /// refusal. Counting observations would count every block of a long stall; counting *single*
    /// refusals counts the ordinary partial layers of a producing round, which is what #318 measured (a
    /// four-validator net "entered a stall" 136-152 times over ~212 blocks while finality advanced).
    /// **Persisted** ([`HealthCounters`]), seeded from the store at boot.
    stall_episodes: Arc<AtomicU64>,
    /// How many merges have returned a report that was not quiet (#280) — a dropped chain, or a
    /// violated invariant. The log carries the detail; this makes the event **countable**, which is the
    /// half the live incident lacked: `reject_whole_blocks` fired with no counter and no line anywhere
    /// moving. **Persisted**, like the episodes above.
    non_quiet_merge_reports: Arc<AtomicU64>,
    /// **How many blocks this shard has given up on** — C249's R3 gap, made countable.
    ///
    /// When a `Divergence` record's revalidation budget is spent and it is *still* failed, the node has
    /// stopped trying: it will refuse that block, and every descendant of it, for the process lifetime
    /// (`casper/src/multi_parent_casper.rs:restore_is_warranted`). That transition used to be silent —
    /// the scan simply `continue`d — which is the disposition pass's finding at its far end: the
    /// failure upstream is loud and the *state* it leaves has no surface, no count and no inverse.
    ///
    /// **Nothing acts on this.** It is the denominator a reset would act on, shipped dark so an operator
    /// can see what a reset *would* have done before one is wired (#287/#294 own the net-wide half).
    /// **Persisted**, like the two above.
    unrestorable_records: Arc<AtomicU64>,
    /// The block this shard last gave up on, as `<hash> (seq <n>)`. The count answers "has this
    /// happened"; this answers "on what". An operator needs both, and the count alone is not
    /// actionably specific. **Not persisted**: it is current state, and a restarted shard that has
    /// given up on nothing has nothing to name — the honest answer rather than a stale one.
    current_unrestorable: Arc<std::sync::Mutex<Option<String>>>,
    /// **Whether the three counters above are the shard's or this process's** (W-N10-02) — the honest
    /// half of "persisted": `true` once a boot read answered or a write succeeded, `false` when the
    /// store could not be reached, in which case the surface says so rather than letting a restart read
    /// as a shard that never stalled.
    counters_durable: Arc<AtomicBool>,
}

impl ShardHealth {
    pub fn new() -> Self {
        Self::default()
    }

    /// **Seed the three persisted counters from the shard's own record at boot** (W-N10-02) — the read
    /// that stops a restart from understating them. The reason and sustained cells are **not** seeded:
    /// they are current state, and a restarted shard that has observed nothing has no reason, which is
    /// the honest answer rather than a stale one.
    ///
    /// **`read` is the boot read itself, and its three shapes are three different facts** — which is
    /// why this takes a `Result<Option<_>>` rather than an `Option<_>`:
    ///
    /// * `Ok(Some(c))` — the shard's history, and it is durable;
    /// * `Ok(None)` — **nothing on record, which IS durable**: a fresh shard's history is zero, and
    ///   saying "cannot vouch" here would be as wrong as saying "never stalled" about a lost record.
    ///   A live node that has never transitioned hits exactly this arm, which is how the first version
    ///   of this signature was caught reading `false` on a healthy two-shard node;
    /// * `Err(_)` — the store could not be read, so the zero is *unknown* and `counters_durable` says so.
    pub fn seed_counters(&self, read: Result<Option<HealthCounters>, String>) {
        let (counters, durable) = match read {
            Ok(record) => (record, true),
            Err(_) => (None, false),
        };
        self.counters_durable.store(durable, Ordering::Relaxed);
        let c = counters.unwrap_or_default();
        self.stall_episodes
            .store(c.stall_episodes, Ordering::Relaxed);
        self.non_quiet_merge_reports
            .store(c.non_quiet_merge_reports, Ordering::Relaxed);
        self.unrestorable_records
            .store(c.unrestorable_records, Ordering::Relaxed);
    }

    /// The three persisted counters as one value, for the writer that stores them.
    pub fn counters(&self) -> HealthCounters {
        HealthCounters {
            stall_episodes: self.stall_episodes.load(Ordering::Relaxed),
            non_quiet_merge_reports: self.non_quiet_merge_reports.load(Ordering::Relaxed),
            unrestorable_records: self.unrestorable_records.load(Ordering::Relaxed),
        }
    }

    /// **Record one observation of the gate's verdict**: the reason when the derivation was refused,
    /// `None` when it advanced. Called on every block's validation, so the surface follows the chain
    /// rather than the log.
    ///
    /// * **advanced** (`None`) — the run resets, the sustained flag clears;
    /// * **refused** (`Coverage`/`Support`) — the run grows, and once it reaches [`stall_threshold`]
    ///   the shard is *stalled*: the flag sets and, on the transition, an episode is counted;
    /// * **nothing to publish** (`AlreadyPublished`) — neither. The derivation has no fringe to publish
    ///   because the published one is current, which is the state a chain that is not moving *backwards*
    ///   lands in; counting it as a refusal would make a quiet net look stalled.
    ///
    /// Returns **`true` when an episode began**, so the caller — which holds the store — persists it.
    /// The write is on the transition, not on the observation, because a block is validated every round
    /// and most are refused by the stake gate.
    pub fn note_finality_stall(&self, reason: Option<&NoAdvance<Validator>>) -> bool {
        let rendered = reason.map(describe_no_advance);
        let partition = reason.and_then(verdict_partition);
        let sustained = self.current_stall_sustained.load(Ordering::Relaxed);
        let mut episode = false;
        match partition {
            None => {
                self.consecutive_refusals.store(0, Ordering::Relaxed);
                self.current_stall_sustained.store(false, Ordering::Relaxed);
            }
            Some(partition) => {
                let run = self.consecutive_refusals.fetch_add(1, Ordering::Relaxed) + 1;
                let stalled = run >= stall_threshold(partition);
                self.current_stall_sustained
                    .store(stalled, Ordering::Relaxed);
                if stalled && !sustained {
                    self.stall_episodes.fetch_add(1, Ordering::Relaxed);
                    episode = true;
                }
            }
        }
        *self.current_stall_reason.lock().unpoison() = rendered;
        episode
    }

    /// The reason the gate last refused the derivation, as this shard observed it.
    ///
    /// `None` means the last observation saw a merge that advanced — **not** "finality is healthy",
    /// which is a judgement the acceptance spec makes, not this accessor. On a producing chain this is
    /// usually `Some(..)` while the chain is perfectly healthy; read
    /// [`Self::finality_stall_sustained`] for the distinction.
    pub fn finality_stall_reason(&self) -> Option<String> {
        self.current_stall_reason.lock().unpoison().clone()
    }

    /// **Whether this shard is stalled right now** — the derivation has been refused
    /// [`stall_threshold`] times in a row, so the run of refusals has outlasted a round. This is what
    /// `finalityStallEpisodes` counts, and what an operator should alarm on; the reason string beside it
    /// is the diagnosis for whichever state the shard is in.
    pub fn finality_stall_sustained(&self) -> bool {
        self.current_stall_sustained.load(Ordering::Relaxed)
    }

    /// How many finality stalls this shard has entered (monotone, persisted).
    pub fn stall_episodes(&self) -> u64 {
        self.stall_episodes.load(Ordering::Relaxed)
    }

    /// How many non-quiet merge reports this shard has logged (monotone, persisted).
    pub fn non_quiet_merge_reports(&self) -> u64 {
        self.non_quiet_merge_reports.load(Ordering::Relaxed)
    }

    /// Record one non-quiet merge report. Rare by construction — a dropped chain or a violated
    /// invariant — so the caller persists rather than the write being rate-limited.
    pub fn note_non_quiet_merge_report(&self) {
        self.non_quiet_merge_reports.fetch_add(1, Ordering::Relaxed);
    }

    /// How many blocks this shard has given up on (monotone, persisted).
    pub fn unrestorable_records(&self) -> u64 {
        self.unrestorable_records.load(Ordering::Relaxed)
    }

    /// The last block this shard gave up on, or `None` if it never has. **Not** a health verdict: a
    /// shard that has given up on one block may still be serving, and this says what it is stuck on,
    /// not how bad that is.
    pub fn unrestorable_block(&self) -> Option<String> {
        self.current_unrestorable.lock().unpoison().clone()
    }

    /// Record that this shard has given up on a block. See the caller in `multi_parent_casper.rs` for
    /// what that means and what it deliberately does not do.
    pub fn note_unrestorable(&self, block_hex: &str, seq_num: i64) {
        self.unrestorable_records.fetch_add(1, Ordering::Relaxed);
        *self.current_unrestorable.lock().unpoison() = Some(format!("{block_hex} (seq {seq_num})"));
    }

    /// **Write the three persisted counters to the shard's record.** Called by a writer *after* a
    /// transition, never on every observation. The error is returned rather than logged here so the
    /// caller, which holds its own `Log`, can say which shard will under-count in the next process.
    pub async fn persist_counters(&self, dag: &dyn BlockDagStorage) -> Result<(), String> {
        dag.write_health_counters(self.counters()).await?;
        self.counters_durable.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// **Record one finality observation, and persist only when it moved a counter** (W-N10-02) — the
    /// rule the validation path applies to every block, in one place rather than as an `if` at a call
    /// site a test cannot reach.
    ///
    /// A block is validated every round and most are refused by the stake gate, so persisting on the
    /// *observation* would be a store write per validated block; what is worth a write is the moment the
    /// shard **enters** a stall, which is what [`Self::note_finality_stall`] reports. The falsifier is
    /// the one that used to be green: make this persist unconditionally and
    /// `casper/tests/reset.rs::the_shard_health_counters_survive_a_restart` finds a record where the
    /// shard had nothing to record.
    ///
    /// The error is the caller's to report, as with [`Self::persist_counters`].
    pub async fn record_finality_stall(
        &self,
        reason: Option<&NoAdvance<Validator>>,
        dag: &dyn BlockDagStorage,
    ) -> Result<(), String> {
        if self.note_finality_stall(reason) {
            self.persist_counters(dag).await
        } else {
            Ok(())
        }
    }

    /// Whether [`Self::counters`] is the shard's history (`true`) or this process's running totals
    /// (`false`) — the surface says which rather than implying durability it does not have.
    pub fn counters_durable(&self) -> bool {
        self.counters_durable.load(Ordering::Relaxed)
    }
}

/// **How many consecutive refusals make a stall.** Not a tuned constant: the derivation advances at
/// least once per round on a healthy net — that is what a round *is*, every bonded sender speaks and
/// the layer completes — so the longest run of refusals a healthy net can show is bounded by the
/// partition, one per block that arrives before the round completes. **Twice the partition** is
/// therefore a run no healthy net reaches, and it scales with the net instead of with a rig: eight
/// bonded validators move the threshold, rather than quietly making every stall look like pacing.
///
/// The partition comes from the refusal itself ([`NoAdvance::Coverage`]/[`NoAdvance::Support`] carry
/// it), so the rule reads the gate's own arithmetic rather than a second copy of it (`Split`, law 51b).
fn stall_threshold(partition: usize) -> u64 {
    2 * partition.max(1) as u64
}

/// The partition a verdict was taken against, when it carries one.
fn verdict_partition(reason: &NoAdvance<Validator>) -> Option<usize> {
    match reason {
        NoAdvance::Coverage { partition, .. } | NoAdvance::Support { partition, .. } => {
            Some(*partition)
        }
        // The progress guard is not a refusal at all: the derivation would publish what is already
        // finalised, so there is nothing to advance *to*. It carries no partition because it decided
        // nothing about one.
        NoAdvance::AlreadyPublished => None,
    }
}

/// **A per-node reset this node has decided it needs** (F-U5-01's acting half, #320).
///
/// The condition is the one `note_unrestorable` counts — a `Divergence` record whose revalidation
/// budget is spent, so the node will refuse that block and its descendants for the process lifetime —
/// **and** a state it can safely rewind to: `DagRepresentation::rewind_plan`'s first arm, the last
/// finalised block. The second arm (no fringe) is deliberately **not** a request: a node with nothing
/// finalised cannot name a safe state, which is C259's state, and "nothing to rewind to" must never
/// become "rewind to genesis".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResetRequest {
    /// The last finalised block — the height the rewind drops above.
    pub anchor_height: BlockHeight,
    /// The same block, for the log and for the state the runtime is reset to.
    pub anchor: BlockHash,
    /// How many blocks the *view* says will go (a floor: validation-failed blocks are not in the height
    /// map and the reset works from the store — the same direction of error as the dry run).
    pub blocks_above: usize,
    /// The block this node gave up on, which is what makes the reset necessary.
    pub refused: BlockHash,
}

/// **A shard's lane key** — the shard id as both halves derive it, `ShardId::to_string()`.
///
/// A node may be a member of several shards (`casper.shards`), each with its own DAG, block store and
/// runtime, while these cells are process-wide. Without a key, a request raised by one shard's validator
/// path reaches whichever shard installed its waker last, and that shard drives *its own* stores with
/// the other's anchor — refused, because the anchor is not in its block store, and once a failed reset
/// stops the node that misdelivery is a whole-node kill. Keying makes "shard B takes shard A's request"
/// unrepresentable rather than guarded (2026-10-13, §107's D4).
pub type ShardLane = String;

static RESET_REQUEST: std::sync::Mutex<BTreeMap<ShardLane, ResetRequest>> =
    std::sync::Mutex::new(BTreeMap::new());
static RESET_WAKE: std::sync::Mutex<BTreeMap<ShardLane, Arc<tokio::sync::Notify>>> =
    std::sync::Mutex::new(BTreeMap::new());

/// **Install the waker the engine's drive loop waits on**, once per shard, at boot. The engine owns
/// where a reset would be performed — it holds the transport, the block store and the runtime — and the
/// detection happens deep in the validator path, which has none of them. So the two halves talk through
/// this cell rather than through a new dependency: the detector records the request, and the driver
/// wakes.
///
/// Installing or replacing a driver also delivers a request already in *that shard's* lane. Lock the
/// request before the waker, as submission does, so a concurrent request either reaches this driver or
/// is observed here as already pending.
pub fn install_reset_waker(shard: ShardLane, notify: Arc<tokio::sync::Notify>) {
    let pending = RESET_REQUEST.lock().unpoison();
    RESET_WAKE
        .lock()
        .unpoison()
        .insert(shard.clone(), notify.clone());
    if pending.contains_key(&shard) {
        notify.notify_one();
    }
}

/// Record a shard's reset request and wake its driver, if one is installed. Returns whether this call
/// *set* it — a second request for the same shard while one is pending changes nothing, and that shard's
/// driver takes the first.
pub fn request_reset(shard: ShardLane, request: ResetRequest) -> bool {
    let mut slot = RESET_REQUEST.lock().unpoison();
    if slot.contains_key(&shard) {
        return false;
    }
    slot.insert(shard.clone(), request);
    let wake = RESET_WAKE.lock().unpoison().get(&shard).cloned();
    drop(slot);
    if let Some(notify) = wake {
        notify.notify_one();
    }
    true
}

/// Take a shard's pending request, if any. The driver takes it exactly once, which is what makes the
/// reset idempotent under a burst of refusals: the first exhaustion acts, the rest of the scan finds
/// the lane empty. **Only the shard that raised a request can take it**, because the lane is the key.
pub fn take_reset_request(shard: &str) -> Option<ResetRequest> {
    RESET_REQUEST.lock().unpoison().remove(shard)
}

/// A shard's pending request without taking it, for the status surface and for tests.
pub fn reset_request(shard: &str) -> Option<ResetRequest> {
    RESET_REQUEST.lock().unpoison().get(shard).cloned()
}

/// **The reset marker this node is standing under** — the durable record, read onto the surface (F-U12-02,
/// §111). The record is cleared only when the catch-up frontier passes its anchor, so a node that resets
/// and never catches up keeps re-entering the boot guard, and **until now nothing said so anywhere a probe
/// could ask**: the marker's lifetime was unbounded and silent. A surface is the R2 answer the ladder asks
/// for, and it is the whole finding — a marker that stands is a node mid-repair, which an operator reading
/// a stalled node needs to see.
///
/// Written by the marker store's own three methods (`casper/src/dag.rs`), so a future writer cannot miss
/// it, and read by the status surface. `None` means this node is not mid-reset.
static CURRENT_RESET_MARKER: std::sync::Mutex<Option<ResetMarker>> = std::sync::Mutex::new(None);

/// Record what the marker store now holds. Called by every marker write, read and clear, so the surface
/// cannot drift from the store it reports.
pub fn note_reset_marker(marker: Option<ResetMarker>) {
    *CURRENT_RESET_MARKER.lock().unpoison() = marker;
}

/// The standing marker, for the status surface.
pub fn reset_marker_state() -> Option<ResetMarker> {
    CURRENT_RESET_MARKER.lock().unpoison().clone()
}

/// The merge search's census, reported on the node's own log (#117). Wall-clock-gated rather than
/// height-gated: the instrument exists to be read off a *run*, and a run is measured in seconds. The
/// clock starts at the first report rather than at process start, so the first line is immediate.
static CENSUS_CLOCK: OnceLock<Instant> = OnceLock::new();
static LAST_CENSUS_LOG_MS: AtomicU64 = AtomicU64::new(0);
const CENSUS_LOG_INTERVAL_MS: u64 = 5000;

/// A validator id, short enough for a log line: its first four bytes in hex.
///
/// The slice cannot be short — a `Validator` is a fixed 65-byte array (`models/src/validator.rs:10`,
/// an uncompressed secp256k1 key), so this is a *display* truncation and not a guard. Said plainly
/// because the four bytes are the only handle an operator has on which validator the gate is waiting
/// for, and a reader who thinks the length is a risk will "fix" it into a lossy form that hides one.
fn short_id(v: &Validator) -> String {
    v.as_bytes()[..4]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The gate's reason for not advancing, as one line an operator can act on.
///
/// The three are the gate's three exits and they imply different faults, which is the whole point of
/// logging them rather than "finality stalled": a partition no layer covers is a different defect from a
/// layer whose stake is short, and both differ from "the derivation would publish what is already
/// finalised".
/// **The deploy identity of a block, for a state-hash disagreement** (#139).
///
/// The two hash values say a disagreement happened; this says *which deploy* was in flight when it
/// did, which is what the issue's close condition asks for ("names the deploy and the two hashes").
/// On this path the deploy that consumes the fringe is the `CloseBlock` system deploy — it anchors the
/// next epoch's seed to the fringe state — so whether one is present is said explicitly rather than
/// left for a reader to infer from a list of kinds.
fn describe_deploys(block: &BlockMessage) -> String {
    let user: Vec<String> = block
        .state
        .deploys
        .iter()
        .map(|d| {
            format!(
                "sig={} deployer={}",
                rchain_shared::base16::encode(&d.deploy.sig),
                rchain_shared::base16::encode(&d.deploy.deployer)
            )
        })
        .collect();
    let kinds: Vec<String> = block
        .state
        .system_deploys
        .iter()
        .map(|sd| format!("{:?}", sd))
        .collect();
    let close_block = block
        .state
        .system_deploys
        .iter()
        .any(|sd| format!("{sd:?}").contains("CloseBlock"));
    format!(
        "user_deploys={} [{}] system_deploys=[{}] close_block={}",
        user.len(),
        user.join(", "),
        kinds.join(", "),
        if close_block { "yes" } else { "no" }
    )
}

/// One line naming a state-hash disagreement and every input that could explain it (#139).
///
/// **Why this exists.** The failure reached an operator as
/// `Block <hex> failed validation: InvalidStateHash` — the variant name, no hashes, and no way to tell
/// a pre-state disagreement (this node's merge produced a different state) from a post-state one (the
/// merge agreed and the replay diverged). Those point at different causes, so a reproduction could not
/// "name the deploy and the two hashes" until this line existed.
///
/// The fringe fields are the load-bearing ones: the replay's `close_block` anchors the next epoch's
/// seed to the fringe state, and a node derives that state from **its own** DAG — it is not on the
/// wire (`validate_block_checkpoint`'s doc). So an empty `prev_fringe` with a `prev_fringe_lookup` of
/// the empty fringe is the signature of blocks that were *restored* rather than validated, and it
/// belongs in the line that is read when two nodes disagree.
#[allow(clippy::too_many_arguments)]
fn describe_state_mismatch(
    block: &BlockMessage,
    pre_state: &ParentsMergedState,
    predicate: &str,
    declared: &Blake2b256Hash,
    recomputed: Option<&Blake2b256Hash>,
) -> String {
    format!(
        "state-hash disagreement on {predicate}: block #{} {} by {} — declared {} vs recomputed {} \
         (pre_state={} fringe_state={} prev_fringe_lookup={} prev_fringe={:?}) {}",
        block.block_number,
        block.block_hash.to_hex(),
        short_id(&block.sender),
        declared.to_hex(),
        match recomputed {
            Some(h) => h.to_hex(),
            None => "none (the replay did not reach a state)".to_string(),
        },
        pre_state.pre_state_hash.to_hex(),
        pre_state.fringe_state.to_hex(),
        pre_state.prev_fringe_lookup.to_hex(),
        pre_state
            .prev_fringe
            .iter()
            .map(|h| h.to_hex())
            .collect::<Vec<_>>(),
        describe_deploys(block),
    )
}

/// One line naming a **`rejected_deploys`** disagreement — the one refusal on this path that used to say
/// nothing at all (#139, #140).
///
/// **Why it is its own line and not a `describe_state_mismatch` call.** The other refusals compare two
/// *hashes* and can name both sides as a pair. This one compares two **sets of rejected deploy ids** —
/// what this node's own merge filtered out against what the block says was filtered out — so the
/// evidence is not a scalar but *which ids are in one set and not the other*. The line therefore carries
/// the symmetric difference, capped at four ids a side, because these sets are peer-sized and this is a
/// `warn` per failing block.
///
/// The arm itself returned `Err(BlockStatus::InvalidRejectedDeploy)` with no log, so an operator saw a
/// status and no counts: the claimed set could be empty, or could be every deploy in the block, and the
/// log could not tell those apart.
fn describe_rejected_deploy_mismatch(
    block: &BlockMessage,
    pre_state: &ParentsMergedState,
) -> String {
    /// The ids present in `set` and absent from `other`, as hex, first four then a count of the rest.
    fn only(set: &BTreeSet<Vec<u8>>, other: &BTreeSet<Vec<u8>>) -> String {
        let diff: Vec<String> = set
            .difference(other)
            .map(|id| rchain_shared::base16::encode(id))
            .collect();
        match diff.len() {
            0 => "none".to_string(),
            n if n <= 4 => diff.join(", "),
            n => format!("{}, +{} more", diff[..4].join(", "), n - 4),
        }
    }
    format!(
        "state-hash disagreement on rejected-deploys: block #{} {} by {} — this node's merge computed {} \
         rejected ({} only here), the block claims {} ({} only there); fringe_state={} \
         prev_fringe_lookup={} {}",
        block.block_number,
        block.block_hash.to_hex(),
        short_id(&block.sender),
        pre_state.fringe_rejected_deploys.len(),
        only(&pre_state.fringe_rejected_deploys, &block.rejected_deploys),
        block.rejected_deploys.len(),
        only(&block.rejected_deploys, &pre_state.fringe_rejected_deploys),
        pre_state.fringe_state.to_hex(),
        pre_state.prev_fringe_lookup.to_hex(),
        describe_deploys(block),
    )
}

/// What a failed replay failed **on**, before `handle_errors` collapses it (#139, #140).
///
/// **Why the variant is worth a line of its own.** `handle_errors` keeps only two outcomes out of a
/// [`ReplayFailure`]: `InternalError` becomes an `Err` — the node's own fault, so no block is recorded
/// failed — and **every other variant becomes `Ok(None)`**, which is the same `InvalidStateHash` a plain
/// post-state mismatch produces. So "the replay disagreed about the post-state" and "the replay failed a
/// status, cost or comm check" reach an operator as one status, and the variant with its payload is what
/// separates them. It is read here, before that collapse.
///
/// The payloads are included rather than elided: a cost pair and a pair of booleans *are* the diagnosis,
/// and the error strings are already in this process's memory at this point. They can be peer-sized,
/// which is the same exposure `handle_errors` already has when it formats an `InternalError`'s cause into
/// an error.
fn describe_replay_failure(failure: &ReplayFailure) -> String {
    match failure {
        ReplayFailure::InternalError(cause) => format!("kind=InternalError cause={cause}"),
        ReplayFailure::ReplayStatusMismatch {
            initial_failed,
            replay_failed,
        } => format!(
            "kind=ReplayStatusMismatch initial_failed={initial_failed} replay_failed={replay_failed}"
        ),
        ReplayFailure::UnusedCommEvent(cause) => format!("kind=UnusedCommEvent cause={cause}"),
        ReplayFailure::ReplayCostMismatch {
            initial_cost,
            replay_cost,
        } => format!(
            "kind=ReplayCostMismatch initial_cost={initial_cost} replay_cost={replay_cost}"
        ),
        ReplayFailure::SystemDeployErrorMismatch {
            play_error,
            replay_error,
        } => format!(
            "kind=SystemDeployErrorMismatch play_error={play_error} replay_error={replay_error}"
        ),
    }
}

fn describe_no_advance(reason: &NoAdvance<Validator>) -> String {
    match reason {
        NoAdvance::Coverage {
            missing,
            senders,
            partition,
        } => format!(
            "the unfinalized region has no layer covering the partition — {} of its {partition} \
             senders have a minimum message and {} do not ({})",
            senders.len(),
            missing.len(),
            missing
                .iter()
                .map(short_id)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        NoAdvance::Support {
            supporting,
            total,
            full_partitions,
            candidates,
            partition,
        } => format!(
            "a layer exists but its supporting stake is not a supermajority — {supporting} of {total} \
             ({full_partitions} full partition(s) among {candidates} candidate(s) of a {partition}-sender \
             partition)"
        ),
        NoAdvance::AlreadyPublished => {
            "the derivation would publish the fringe that is already finalized".to_string()
        }
    }
}

/// Validate a block by recomputing its pre-state and replaying its deploys (port of
/// `validateBlockCheckpoint`). Returns the block metadata plus a `bool` (valid) / `BlockStatus`
/// (rejectable) outcome.
pub async fn validate_block_checkpoint<F, Fut>(
    runtime: &RuntimeManager,
    dag: &dyn BlockDagStorage,
    block_store: &BlockStore,
    block: &BlockMessage,
    block_index: &F,
    log: &Arc<dyn Log>,
    // **The shard's health cells** (W-N10-02, W-N10-03). The two observations below used to write
    // process-global statics, so a refusal in one shard appeared in every shard's finality health and a
    // restart zeroed counters the shard's own history had moved. Threaded, not registered — see
    // [`ShardHealth`] for why.
    health: &ShardHealth,
) -> Result<(BlockMetadata, Result<bool, BlockStatus>), String>
where
    F: Fn(BlockHash) -> Fut,
    Fut: std::future::Future<Output = Result<Arc<BlockIndex>, String>>,
{
    // Non-failed parent hashes.
    let mut parents: Vec<BlockHash> = Vec::new();
    for j in &block.justifications {
        if let Some(meta) = dag.lookup(j).await? {
            if !meta.validation_failed {
                parents.push(meta.block_hash);
            }
        }
    }
    let parents_set: BTreeSet<BlockHash> = parents.iter().copied().collect();

    let source = LogSource::new("casper.interpreter.validate");
    let pre_state = if !parents_set.is_empty() {
        let pre_state =
            get_pre_state_for_parents(dag, block_store, runtime, &parents_set, block_index).await?;
        // **What the merge's conflict search was handed, from the running node (#117).** The search's
        // cost is a deterministic function of the conflict map, and the map's real shape — how wide a
        // scope gets, how dense its conflicts are — had never been observed, so the defect was argued
        // about with an assumed shape. `warn`, not `info`, so it is visible at the default level; the
        // five-second gate keeps a long run to a readable number of lines. See
        // `crate::merging::search_census`, and remove both with the fix.
        let elapsed_ms = CENSUS_CLOCK.get_or_init(Instant::now).elapsed().as_millis() as u64;
        if elapsed_ms.saturating_sub(LAST_CENSUS_LOG_MS.load(Ordering::Relaxed))
            >= CENSUS_LOG_INTERVAL_MS
        {
            LAST_CENSUS_LOG_MS.store(elapsed_ms, Ordering::Relaxed);
            log.warn(source, &crate::merging::search_census::summary());
        }
        // **Why finality is not advancing, in this node's own log.** On 2026-09-29 a two-validator
        // chain sat at finality 44 while the height ran to 202 and nothing in the logs said why (#70).
        // The reason comes back from the gate itself, so it cannot disagree with the decision it
        // explains, and it is rate-limited to one line per `STALL_LOG_INTERVAL` heights.
        // **The surface's copy, written on every observation** (C249's F-U10-01). The line below is
        // rate-limited so a long stall stays readable; a status surface that inherited that limit would
        // report a stale reason for up to `STALL_LOG_INTERVAL` heights. This one is current or nothing.
        // **The episode counter is persisted only when it moves** (W-N10-02) — the rule is
        // [`ShardHealth::record_finality_stall`], one call rather than an `if` here, so it can be driven
        // with a store and no runtime. This path validates a block every round and most are refused.
        if let Err(e) = health
            .record_finality_stall(pre_state.finality_stall.as_ref(), dag)
            .await
        {
            log.warn(
                source,
                &format!(
                    "a finality-stall episode was counted but not recorded: {e} — this shard will \
                     under-report its stall history in the next process"
                ),
            );
        }
        if let Some(reason) = &pre_state.finality_stall {
            let tip = pre_state.max_block_num;
            // **Compare the rendered line, not the variant name.** The first version of this gate keyed on
            // the variant, and the devnet pin is `Support` from genesis onward — so the kind never changed
            // and the line never re-fired: the 2026-09-30 run produced three genesis-tip lines and nothing
            // about the pin. What moves is the *numbers* in the line (0 of 250 becomes 200 of 250 becomes
            // …), and those are the diagnosis, so the line firing on any change is what makes it readable.
            let line = describe_no_advance(reason);
            let changed = {
                let mut last = LAST_STALL_DESC.lock().unpoison();
                let changed = last.as_deref() != Some(line.as_str());
                if changed {
                    *last = Some(line.clone());
                }
                changed
            };
            if changed
                || tip.saturating_sub(LAST_STALL_LOG.load(Ordering::Relaxed)) >= STALL_LOG_INTERVAL
            {
                LAST_STALL_LOG.store(tip, Ordering::Relaxed);
                log.warn(
                    source,
                    &format!("finality did not advance at tip {tip}: {line}"),
                );
            }
        }
        // **What the merge did, when it did anything at all** (#280). This is the line the live
        // incident could not produce: a merge rejected a boundary chain, a user deploy died with it,
        // and no counter and no line anywhere moved. `is_quiet` is the guard rather than a rate limit
        // because a merge that rejects nothing is the ordinary case, and the interesting merges — the
        // ones that drop a chain, or that violate an invariant — are exactly the ones worth a line.
        if !pre_state.merge_report.is_quiet() {
            health.note_non_quiet_merge_report();
            // Rare by construction — a dropped chain or a violated invariant — so it is persisted
            // every time rather than rate-limited (W-N10-02).
            if let Err(e) = health.persist_counters(dag).await {
                log.warn(
                    source,
                    &format!(
                        "a non-quiet merge report was counted but not recorded: {e} — this shard will \
                         under-report its merge history in the next process"
                    ),
                );
            }
            log.warn(source, &pre_state.merge_report.describe());
        }
        pre_state
    } else {
        // Genesis block: no parents.
        let genesis_pre_state_hash = empty_state_hash_fixed();
        ParentsMergedState {
            finality_stall: None,
            merge_report: crate::merging::MergeReport::default(),
            justifications: Vec::new(),
            max_block_num: 0,
            max_seq_nums: BTreeMap::from([(block.sender, 0)]),
            fringe: BTreeSet::new(),
            fringe_state: genesis_pre_state_hash,
            // The genesis has no parents, so the fringe it begins from is the empty one — the same
            // key a restored node's blocks carry, which is why the two are worth telling apart by the
            // *block number* in the log rather than by this field alone (#139).
            prev_fringe_lookup: rchain_models::block_metadata::fringe_hash_of(&BTreeSet::new()),
            prev_fringe: BTreeSet::new(),
            fringe_bonds_map: block.bonds.clone(),
            // Genesis has no parents and therefore no fringe, so it has no participation: an empty map,
            // which is the honest reading rather than one invented from the bonds. A boundary's rule
            // reads the participation of *its own* merge, not this one.
            participation: BTreeMap::new(),
            fringe_rejected_deploys: BTreeSet::new(),
            pre_state_hash: genesis_pre_state_hash,
            rejected_deploys: BTreeSet::new(),
        }
    };

    let incoming_pre_state_hash = Blake2b256Hash::from_byte_array(block.pre_state_hash.as_bytes());
    // **Its own status, not a second way to say `InvalidStateHash`** (#139). `Ok(false)` reaches
    // `multi_parent_casper`'s `InvalidStateHash`, and so did this arm until it was split: an operator
    // could not tell a *pre*-state disagreement — this node's merge over the block's justifications
    // produced a different state — from a *post*-state one, where the merge agreed and the replay then
    // diverged. Those point at different causes, and naming which one fired is the first thing #139's
    // close condition asks for.
    let result: Result<bool, BlockStatus> = if incoming_pre_state_hash != pre_state.pre_state_hash {
        log.warn(
            source,
            &describe_state_mismatch(
                block,
                &pre_state,
                "pre-state",
                &incoming_pre_state_hash,
                Some(&pre_state.pre_state_hash),
            ),
        );
        Err(BlockStatus::InvalidPreStateHash)
    } else if pre_state.fringe_rejected_deploys != block.rejected_deploys {
        // **The one arm on this path that used to say nothing at all.** The other refusals carry a hash
        // disagreement and go through `describe_state_mismatch`; this one compared two sets and returned
        // `InvalidRejectedDeploy` with no log, so the claimed set could be empty or could be every deploy
        // in the block and the log could not tell those apart (Aria's #140 named the arm; this is it,
        // rewritten against the status split that landed after it).
        log.warn(
            source,
            &describe_rejected_deploy_mismatch(block, &pre_state),
        );
        Err(BlockStatus::InvalidRejectedDeploy)
    } else if slash_is_unjustified(dag, block).await? {
        Err(BlockStatus::UnjustifiedSlash)
    } else {
        let rand =
            BlockRandomSeed::random_generator_from_block(block).map_err(|e| e.to_string())?;
        let post_state_hash = Blake2b256Hash::from_byte_array(block.post_state_hash.as_bytes());
        // Fork a fresh replay runtime at the block's pre-state (read-only history fork) so block
        // validation is self-contained and can run concurrently with other blocks.
        let forked = runtime
            .fork_replay_runtime(pre_state.pre_state_hash)
            .await?;
        let replay_result = replay_block(
            runtime,
            &forked,
            block,
            &pre_state.fringe_state,
            &pre_state.participation,
            &rand,
        )
        .await;
        // Read the recomputed hash *before* `handle_errors` consumes the result, so the disagreement
        // can be reported with both sides (#139).
        let recomputed = replay_result.as_ref().ok().cloned();
        // **A replay that failed and a replay that disagreed are different faults**, and `handle_errors`
        // collapses them: every variant but `InternalError` becomes `Ok(None)`, which is the same
        // `InvalidStateHash` a plain post-state mismatch produces. The variant and its payload are read
        // here, before that collapse (Aria's #140; the hash-side arms of that PR are already covered by
        // the disagreement lines above and below).
        if let Err(failure) = &replay_result {
            log.warn(
                source,
                &format!(
                    "replay failed on block #{} {} by {} — {} (declared post-state {} pre_state={} \
                     fringe_state={} prev_fringe_lookup={}) {}",
                    block.block_number,
                    block.block_hash.to_hex(),
                    short_id(&block.sender),
                    describe_replay_failure(failure),
                    post_state_hash.to_hex(),
                    pre_state.pre_state_hash.to_hex(),
                    pre_state.fringe_state.to_hex(),
                    pre_state.prev_fringe_lookup.to_hex(),
                    describe_deploys(block),
                ),
            );
        }
        let handled = handle_errors(&post_state_hash, replay_result)?;
        if handled.is_none() {
            log.warn(
                source,
                &describe_state_mismatch(
                    block,
                    &pre_state,
                    "post-state",
                    &post_state_hash,
                    recomputed.as_ref(),
                ),
            );
        }
        Ok(handled.is_some())
    };

    let validation_failed = match &result {
        Err(_) => true,
        Ok(valid) => !*valid,
    };
    let bmd = BlockMetadata {
        validated: true,
        validation_failed,
        // **The cause is not `mark_failed`'s to decide here.** Everything this function can refuse for
        // is a disagreement between the block and *this node's* state or replay — a pre-state hash it
        // did not derive, a rejected-deploy set from its own merge, a post-state it recomputed — so
        // the cause is `Divergence` and the failure is not the block's own fault (AUDIT C173). The
        // attribution is left unset (`mark_failed` sets both `slashable` and the cause from the
        // status; setting it here as well is what once made the flag survive a disagreement it did
        // not describe).
        failure_cause: validation_failed.then_some(FailureCause::Divergence),
        slash_severity: SlashSeverity::Unspecified,
        slashable: false,
        fringe: pre_state.fringe,
        fringe_state_hash: StateHash::from_slice(pre_state.fringe_state.as_bytes()),
        ..BlockMetadata::from_block(block)
    };

    Ok((bmd, result))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(byte: u8) -> Blake2b256Hash {
        Blake2b256Hash::from_bytes([byte; 32])
    }

    /// **The reset request's contract, which the engine's running loop depends on** (#320).
    ///
    /// Three properties, and each is load-bearing for the loop rather than decorative:
    ///
    /// * **the first request wins.** A single validation pass can give up on several blocks, and the
    ///   reset it needs is one reset — so a second request while one is pending must not replace the
    ///   anchor the driver is about to rewind to;
    /// * **taking it consumes it.** The loop takes exactly once, which is what makes acting idempotent
    ///   under a burst of refusals;
    /// * **the waker fires**, or a loop parked on the packet stream would never look — the whole reason
    ///   the request travels through a cell instead of the validator path performing the reset itself.
    ///
    /// One test rather than three, for the same reason the stall surface's is one: the statics are
    /// process-wide and libtest runs a module's tests concurrently.
    #[tokio::test]
    async fn a_reset_request_is_raised_once_taken_once_and_wakes_the_driver() {
        let lane = || "test-shard".to_string();
        let notify = Arc::new(tokio::sync::Notify::new());
        install_reset_waker(lane(), notify.clone());
        // Whatever a previous test left: this test owns the lane from here.
        let _ = take_reset_request(&lane());

        let first = ResetRequest {
            anchor: BlockHash::new([1u8; 32]),
            anchor_height: BlockHeight::try_from(10i64).expect("a height"),
            blocks_above: 3,
            refused: BlockHash::new([2u8; 32]),
        };
        let second = ResetRequest {
            anchor: BlockHash::new([9u8; 32]),
            ..first.clone()
        };

        assert!(
            request_reset(lane(), first.clone()),
            "the first request is recorded"
        );
        assert!(
            !request_reset(lane(), second),
            "a second request while one is pending is refused rather than replacing it"
        );
        assert_eq!(
            reset_request(&lane()).as_ref(),
            Some(&first),
            "the anchor the driver will rewind to is the first one asked for"
        );

        tokio::time::timeout(std::time::Duration::from_secs(1), notify.notified())
            .await
            .expect("the request wakes a driver parked on the packet stream");

        // **Another shard cannot take it** (2026-10-13, §107's D4). The lane is the key, so a request
        // raised for one shard is invisible to every other: the misdelivery that would drive a second
        // shard's stores with the first shard's anchor is unrepresentable rather than refused.
        assert_eq!(
            take_reset_request("another-shard"),
            None,
            "a request is only taken by the shard that raised it"
        );

        assert_eq!(
            take_reset_request(&lane()),
            Some(first.clone()),
            "the driver takes it"
        );
        assert_eq!(
            take_reset_request(&lane()),
            None,
            "and takes it exactly once — a burst of refusals is one reset"
        );

        // A request can precede installation. The stored request, rather than
        // the old notification, must make the newly installed driver runnable.
        RESET_WAKE.lock().unpoison().remove(&lane());
        assert!(request_reset(lane(), first.clone()));
        let installed = Arc::new(tokio::sync::Notify::new());
        install_reset_waker(lane(), installed.clone());
        tokio::time::timeout(std::time::Duration::from_secs(1), installed.notified())
            .await
            .expect("installing a waker delivers the request already in the lane");
        assert_eq!(reset_request(&lane()), Some(first.clone()));

        // Handover while pending keeps the first request and wakes the new
        // driver, even after the previous driver consumed its notification.
        let replacement = Arc::new(tokio::sync::Notify::new());
        install_reset_waker(lane(), replacement.clone());
        tokio::time::timeout(std::time::Duration::from_secs(1), replacement.notified())
            .await
            .expect("a replacement driver observes the same pending request");
        assert_eq!(take_reset_request(&lane()), Some(first));
        assert_eq!(take_reset_request(&lane()), None);

        let idle = Arc::new(tokio::sync::Notify::new());
        install_reset_waker(lane(), idle.clone());
        assert!(
            tokio::time::timeout(std::time::Duration::ZERO, idle.notified())
                .await
                .is_err(),
            "installing on an empty lane must not request a spurious reset"
        );
    }

    /// **The standing-marker surface follows the store** (F-U12-02, §111). The marker's lifetime is
    /// unbounded — it is cleared only when the catch-up frontier passes its anchor — so a node that
    /// resets and cannot catch up stands under one for ever, and before this cell nothing on any surface
    /// said so. The cell is process-wide, so this test owns it; the *wiring* (every marker write, read
    /// and clear driving it) is pinned in `casper/tests/reset.rs`'s crash test, which writes a real
    /// marker through the storage trait.
    #[test]
    fn the_reset_marker_surface_follows_the_store() {
        note_reset_marker(None);
        assert!(
            reset_marker_state().is_none(),
            "no marker, nothing reported: a healthy node is not mid-reset"
        );
        let marker = ResetMarker {
            anchor: BlockHash::new([7u8; 32]),
            height: BlockHeight::try_from(5i64).expect("a height"),
            phase: rchain_block_storage::dag::dag_storage::ResetPhase::Rewinding,
        };
        note_reset_marker(Some(marker.clone()));
        assert_eq!(
            reset_marker_state(),
            Some(marker),
            "a standing marker is reportable without reading the DAG"
        );
        note_reset_marker(None);
        assert!(
            reset_marker_state().is_none(),
            "and the clear is reported too — a surface that kept reporting would alarm for ever"
        );
    }

    /// **The disagreement line names both hashes, the predicate, and whether a `CloseBlock` is
    /// present** (#139). Those three are the whole of what the issue's close condition asks a
    /// reproduction to produce, so the line that carries them is pinned rather than assumed: a later
    /// campaign reads *this* string out of a node's log, and a field silently dropped here would make
    /// every run's evidence unreadable in the same way, months after the change.
    ///
    /// The `close_block=yes` arm is deliberate: that is the system deploy that anchors the next
    /// epoch's seed to the fringe state, so it is the deploy a fringe-derived divergence appears on.
    /// A block and the pre-state it was merged against, with every field the mismatch lines read set to a
    /// distinct value. Shared so that a change to `BlockMessage` or `ParentsMergedState` breaks one
    /// fixture rather than one per line under test.
    fn fixture() -> (BlockMessage, ParentsMergedState) {
        let block = BlockMessage {
            version: 1,
            shard_id: "root".to_string(),
            block_hash: BlockHash::new([0xab; 32]),
            block_number: 42.try_into().unwrap(),
            sender: Validator::new([1u8; 65]),
            seq_num: 0.try_into().unwrap(),
            pre_state_hash: rchain_models::block::state_hash::StateHash::new([0x11; 32]),
            post_state_hash: rchain_models::block::state_hash::StateHash::new([0x22; 32]),
            justifications: vec![],
            bonds: BTreeMap::new(),
            rejected_deploys: BTreeSet::new(),
            rejected_blocks: BTreeSet::new(),
            rejected_senders: BTreeSet::new(),
            state: rchain_models::casper::protocol::casper_message::RholangState {
                deploys: vec![],
                system_deploys: vec![
                    rchain_models::casper::protocol::casper_message::ProcessedSystemDeploy::Succeeded {
                        event_list: vec![],
                        system_deploy:
                            rchain_models::casper::protocol::casper_message::SystemDeployData::CloseBlock,
                    },
                ],
            },
            sig_algorithm: "secp256k1".to_string(),
            sig: vec![1],
            timestamp: 0,
        };

        let pre_state = ParentsMergedState {
            finality_stall: None,
            merge_report: crate::merging::MergeReport::default(),
            justifications: vec![],
            max_block_num: 41,
            max_seq_nums: BTreeMap::new(),
            fringe: BTreeSet::new(),
            fringe_state: hash(0x33),
            prev_fringe_lookup: hash(0x44),
            prev_fringe: BTreeSet::new(),
            fringe_bonds_map: BTreeMap::new(),
            participation: BTreeMap::new(),
            fringe_rejected_deploys: BTreeSet::new(),
            pre_state_hash: hash(0x11),
            rejected_deploys: BTreeSet::new(),
        };
        (block, pre_state)
    }

    /// A `BTreeSet<Vec<u8>>` of rejected deploy ids, for the fixture's overrides.
    fn ids(items: impl IntoIterator<Item = u8>) -> BTreeSet<Vec<u8>> {
        items.into_iter().map(|b| vec![b]).collect()
    }

    #[test]
    fn the_disagreement_line_names_both_hashes_and_the_predicate() {
        let (block, pre_state) = fixture();

        let line = describe_state_mismatch(
            &block,
            &pre_state,
            "post-state",
            &hash(0x22),
            Some(&hash(0x55)),
        );

        // Both sides of the disagreement.
        assert!(line.contains(&hash(0x22).to_hex()), "declared: {line}");
        assert!(line.contains(&hash(0x55).to_hex()), "recomputed: {line}");
        // Which comparison failed, and which block it was.
        assert!(line.contains("post-state"), "{line}");
        assert!(line.contains("#42"), "{line}");
        // The fringe inputs, without which a fringe-derived divergence cannot be read.
        assert!(line.contains(&hash(0x33).to_hex()), "fringe_state: {line}");
        assert!(
            line.contains(&hash(0x44).to_hex()),
            "prev_fringe_lookup: {line}"
        );
        // The deploy that consumes the fringe.
        assert!(line.contains("close_block=yes"), "{line}");

        // A replay that never produced a value is still a readable line rather than a panic or a
        // missing field — `handle_errors` returns `Ok(None)` for a soft failure as well as for a
        // mismatch.
        let none = describe_state_mismatch(&block, &pre_state, "post-state", &hash(0x22), None);
        assert!(none.contains("did not reach a state"), "{none}");
    }

    #[test]
    fn handle_errors_accepts_matching_hash() {
        assert_eq!(handle_errors(&hash(1), Ok(hash(1))).unwrap(), Some(hash(1)));
    }

    #[test]
    fn handle_errors_rejects_mismatching_hash() {
        assert_eq!(handle_errors(&hash(1), Ok(hash(2))).unwrap(), None);
    }

    #[test]
    fn handle_errors_raises_internal_error() {
        let r = handle_errors(&hash(1), Err(ReplayFailure::internal_error("boom")));
        assert!(r.is_err());
    }

    /// The condition the genesis-vault re-install hangs on (AUDIT C46). It has to be *exactly* the
    /// genesis: re-installing the balances on a later block's replay would clobber post-genesis
    /// balances, which is why the fix is conditional rather than unconditional.
    #[test]
    fn is_genesis_pre_state_is_true_only_for_the_empty_state() {
        assert!(
            is_genesis_pre_state(&empty_state_hash_fixed()),
            "the genesis pre-state is the empty state"
        );
        assert!(
            !is_genesis_pre_state(&hash(0xab)),
            "a post-genesis pre-state must not re-install the genesis vaults"
        );
    }

    #[test]
    fn handle_errors_soft_fails_on_replay_status_mismatch() {
        let r = handle_errors(
            &hash(1),
            Err(ReplayFailure::replay_status_mismatch(true, false)),
        );
        assert_eq!(r.unwrap(), None);
    }

    /// **The arm that used to leave no trace** (#140). A `rejected_deploys` disagreement has to name both
    /// sets — so an empty claimed set is distinguishable from one holding every deploy — and, because the
    /// evidence is *which ids differ*, the difference between them.
    #[test]
    fn the_rejected_deploy_line_names_both_sets_and_what_differs() {
        let (mut block, mut pre_state) = fixture();
        // This node's merge filtered out `01` and `02`; the block claims `02` and `03`.
        pre_state.fringe_rejected_deploys = ids([0x01, 0x02]);
        block.rejected_deploys = ids([0x02, 0x03]);

        let line = describe_rejected_deploy_mismatch(&block, &pre_state);

        assert!(line.contains("rejected-deploys"), "{line}");
        assert!(line.contains("#42"), "{line}");
        // Both counts, which is what the arm logged nothing of before.
        assert!(line.contains("computed 2 rejected"), "computed: {line}");
        assert!(line.contains("claims 2"), "claimed: {line}");
        // The ids each side has and the other does not — the actual evidence.
        assert!(
            line.contains(&rchain_shared::base16::encode(&[0x01u8])),
            "only here: {line}"
        );
        assert!(
            line.contains(&rchain_shared::base16::encode(&[0x03u8])),
            "only there: {line}"
        );
        // The deploy that consumes the fringe, as the sibling disagreement line carries it.
        assert!(line.contains("close_block=yes"), "{line}");
    }

    /// …and a side with no ids of its own says `none` rather than printing an empty list, because that is
    /// what tells an operator the other set is a strict superset.
    #[test]
    fn the_rejected_deploy_line_says_none_when_one_side_is_a_superset() {
        let (mut block, mut pre_state) = fixture();
        pre_state.fringe_rejected_deploys = ids([0x01]);
        block.rejected_deploys = ids([0x01, 0x02]);

        let line = describe_rejected_deploy_mismatch(&block, &pre_state);

        assert!(line.contains("(none only here)"), "{line}");
        assert!(
            line.contains(&rchain_shared::base16::encode(&[0x02u8])),
            "only there: {line}"
        );
    }

    /// **Read before `handle_errors` collapses it** (#140). Four of the five variants become `Ok(None)` —
    /// the same status a plain post-state mismatch produces — so the variant is the only thing that
    /// separates "the replay disagreed" from "the replay failed a status, cost or comm check".
    #[test]
    fn the_replay_failure_line_names_every_variant_and_its_payload() {
        let cases = [
            (ReplayFailure::internal_error("boom"), "kind=InternalError"),
            (
                ReplayFailure::replay_status_mismatch(true, false),
                "initial_failed=true",
            ),
            (
                ReplayFailure::unused_comm_event("c"),
                "kind=UnusedCommEvent",
            ),
            (ReplayFailure::replay_cost_mismatch(7, 9), "replay_cost=9"),
            (
                ReplayFailure::system_deploy_error_mismatch("a", "b"),
                "play_error=a",
            ),
        ];

        let mut lines = BTreeSet::new();
        for (failure, needle) in &cases {
            let line = describe_replay_failure(failure);
            assert!(line.contains(needle), "{needle} missing from {line}");
            lines.insert(line);
        }
        // All five must be *distinguishable*: a line that named only the kind would pass the assertions
        // above while leaving the four soft-failed variants reading alike.
        assert_eq!(lines.len(), cases.len(), "one line per variant: {lines:?}");
    }

    /// **The stall reason reaches a surface, and the surface follows the gate rather than the log**
    /// (C249's F-U10-01).
    ///
    /// The line beside this one in the validation path is rate-limited to one per `STALL_LOG_INTERVAL`
    /// heights so a long stall stays readable. A surface reading *that* cell would report a stale reason
    /// for up to a hundred heights — precisely while an operator is looking — so the two are separate.
    /// This pins the separate one: written on every observation, cleared when the gate stops reporting,
    /// and counting **episodes** rather than blocks.
    ///
    /// Falsifier: before this change there was no cell and no accessor — the reason existed only as a
    /// formatted log line, so a test could not ask for it at all. That is what "log-only" meant.
    ///
    /// The assertions are exact, **because the cell is a local `ShardHealth` now** (W-N10-02/03). They
    /// used to be `>`/`>=` against a captured reading: the cells were process-wide statics and libtest
    /// runs a module's tests concurrently. One instance per test makes both the flakiness and the fuzzy
    /// comparisons go away — and the exactness is the point, because the whole contract is a *sequence*
    /// (refuse, refuse, advance, refuse) that a second test touching the same cells would have made
    /// unassertable.
    ///
    /// What it pins, in #318's terms: a single refusal on a producing chain is **pacing**, not a stall —
    /// the derivation advances at least once per round, so a run shorter than twice the partition is
    /// what a healthy net shows and what used to be counted here.
    #[test]
    fn a_stall_is_a_run_of_refusals_and_the_run_scales_with_the_partition() {
        // The measurement #318 recorded, as a verdict: one validator's candidate, seen by the whole
        // partition, on a four-sender net. On the surface this is "100 of 400" — the ordinary state
        // between the first and the last block of a round.
        let partial = |partition: usize, total: i128| NoAdvance::Support {
            supporting: 100,
            total,
            full_partitions: 1,
            candidates: 1,
            partition,
        };
        let health = ShardHealth::new();

        assert!(
            !health.note_finality_stall(None),
            "an advance is not an episode"
        );
        assert_eq!(
            health.finality_stall_reason(),
            None,
            "an advance clears the verdict"
        );
        assert!(!health.finality_stall_sustained());
        assert_eq!(health.stall_episodes(), 0);

        // Seven refusals on a four-sender net: below `2 * 4`. Pacing, and the counter must not move —
        // this is the assertion the old definition failed, where each round added an "episode".
        for _ in 0..7 {
            health.note_finality_stall(Some(&partial(4, 400)));
            assert!(
                !health.finality_stall_sustained(),
                "a run shorter than two rounds is not a stall"
            );
            assert_eq!(
                health.stall_episodes(),
                0,
                "and it is not an episode: this is #318's defect"
            );
        }
        // The verdict is still reported while the run is short — the surface shows the *diagnosis*
        // whether or not the state has become a stall, which is what F-U10-01 asked of it.
        let reason = health
            .finality_stall_reason()
            .expect("the per-observation verdict is reported");
        assert!(
            reason.contains("100 of 400") && reason.contains("of a 4-sender partition"),
            "the reason carries the layer and the partition it was measured against: {reason}"
        );

        // The eighth consecutive refusal reaches `2 * 4`, and *that* is the stall — and the transition
        // is what the writer persists (W-N10-02).
        assert!(
            health.note_finality_stall(Some(&partial(4, 400))),
            "entering a stall is the transition the counter moves on"
        );
        assert!(
            health.finality_stall_sustained(),
            "the run has outlasted a round"
        );
        assert_eq!(
            health.stall_episodes(),
            1,
            "entering a stall is one episode"
        );

        // Further refusals are the same stall, however many — and none of them is another transition,
        // so none of them is another store write.
        for _ in 0..20 {
            assert!(
                !health.note_finality_stall(Some(&partial(4, 400))),
                "a refusal inside a running stall is not a transition, so it is not persisted"
            );
        }
        assert_eq!(
            health.stall_episodes(),
            1,
            "one stall is one episode however many observations it takes"
        );

        // An advance ends it, and the next run is a second episode.
        health.note_finality_stall(None);
        assert!(
            !health.finality_stall_sustained(),
            "an advance is a recovery"
        );
        assert_eq!(
            health.finality_stall_reason(),
            None,
            "and the reason goes with it"
        );
        for _ in 0..8 {
            health.note_finality_stall(Some(&partial(4, 400)));
        }
        assert_eq!(
            health.stall_episodes(),
            2,
            "a stall after a recovery is a new episode"
        );

        // **The threshold is the net's, not the rig's.** Eight refusals are a stall on four senders and
        // pacing on thirty, because a round on thirty is longer — the number the rule reads is the
        // gate's own partition, so this needs no constant to be right about both.
        health.note_finality_stall(None);
        for _ in 0..8 {
            health.note_finality_stall(Some(&partial(30, 3000)));
        }
        assert!(
            !health.finality_stall_sustained(),
            "eight refusals on a thirty-sender net are still inside a round"
        );

        // **`AlreadyPublished` is neither a refusal nor an advance**: the derivation has nothing to
        // publish because what is published is current. Counting it would make a quiet chain look
        // stalled, and it carries no partition because it decided nothing about one.
        health.note_finality_stall(None);
        let before_guard = health.stall_episodes();
        for _ in 0..50 {
            assert!(!health.note_finality_stall(Some(&NoAdvance::AlreadyPublished)));
        }
        assert!(
            !health.finality_stall_sustained(),
            "nothing to publish is not a stall"
        );
        assert_eq!(
            health.stall_episodes(),
            before_guard,
            "and the progress guard counts no episode"
        );
        assert!(
            health.finality_stall_reason().is_some(),
            "the guard's verdict is still the last verdict observed"
        );
    }

    /// **A shard's counters are the shard's** (W-N10-03) — two instances observe independently, which is
    /// the property the process-global statics could not have. This reddens on the tree before the fix:
    /// with one set of statics, beta's refusal moves alpha's `finalityStallEpisodes`.
    #[test]
    fn one_shards_stall_is_not_another_shards() {
        let alpha = ShardHealth::new();
        let beta = ShardHealth::new();
        let partial = NoAdvance::Support {
            supporting: 100,
            total: 400,
            full_partitions: 1,
            candidates: 1,
            partition: 4,
        };

        for _ in 0..8 {
            beta.note_finality_stall(Some(&partial));
        }
        assert!(
            beta.finality_stall_sustained(),
            "beta reached its threshold"
        );
        assert_eq!(beta.stall_episodes(), 1);

        assert!(
            !alpha.finality_stall_sustained(),
            "alpha saw no refusals of its own, so alpha is not stalled"
        );
        assert_eq!(
            alpha.stall_episodes(),
            0,
            "and beta's refusal is not alpha's episode"
        );
        assert_eq!(alpha.finality_stall_reason(), None);
    }

    /// **The three persisted counters survive a restart** (W-N10-02) — a restart's shape is `counters()`
    /// out, `seed_counters()` into a fresh cell, which is what the boot seed does.
    #[test]
    fn the_persisted_counters_round_trip_through_a_new_cell() {
        let before = ShardHealth::new();
        for _ in 0..8 {
            before.note_finality_stall(Some(&NoAdvance::Support {
                supporting: 100,
                total: 400,
                full_partitions: 1,
                candidates: 1,
                partition: 4,
            }));
        }
        before.note_non_quiet_merge_report();
        before.note_unrestorable("deadbeef", 42);

        let record = before.counters();
        assert_eq!(record.stall_episodes, 1);
        assert_eq!(record.non_quiet_merge_reports, 1);
        assert_eq!(record.unrestorable_records, 1);

        // The restart: a fresh cell seeded from the record.
        let after = ShardHealth::new();
        assert_eq!(after.stall_episodes(), 0, "a fresh cell starts at zero");
        after.seed_counters(Ok(Some(record)));
        assert_eq!(
            after.stall_episodes(),
            1,
            "the shard's stall history survives"
        );
        assert_eq!(after.non_quiet_merge_reports(), 1, "and its merge history");
        assert_eq!(
            after.unrestorable_records(),
            1,
            "and the blocks it gave up on"
        );
        // **The current-state cells are NOT seeded**, and that is the decision rather than an omission:
        // a restarted shard that has observed nothing has no *current* verdict, and reporting the
        // previous process's would be a stale reason presented as a live one.
        assert_eq!(after.finality_stall_reason(), None);
        assert!(!after.finality_stall_sustained());
        assert_eq!(after.unrestorable_block(), None);
    }

    /// **A node that has given up on a block says so, and says where** — C249's R3 gap, counted.
    ///
    /// Falsifier: before this change the moment a node stopped trying was **silent** — the restore scan
    /// simply `continue`d — so there was no cell, no accessor and no way for a test (or a probe) to ask
    /// whether it had happened at all. That is the shape the disposition pass calls "fails loudly and
    /// then does nothing", and this is the *doing nothing* half made legible.
    ///
    /// The cell is a local [`ShardHealth`] now (W-N10-02/03), so the assertions are exact rather than
    /// `>`/`>=` against a captured reading.
    #[test]
    fn the_unrestorable_surface_names_the_block_it_gave_up_on() {
        let health = ShardHealth::new();
        assert_eq!(health.unrestorable_records(), 0);

        health.note_unrestorable("deadbeef", 42);

        assert_eq!(
            health.unrestorable_records(),
            1,
            "giving up on a block is counted"
        );
        let named = health
            .unrestorable_block()
            .expect("the block it gave up on is named");
        assert!(
            named.ends_with("deadbeef (seq 42)"),
            "the name carries the hash and the sequence number an operator needs to act: {named}"
        );

        // Monotone: a later give-up replaces the *name* and adds to the count, so a shard that has
        // given up on two blocks reads 2 and names the second — never 1.
        health.note_unrestorable("cafebabe", 7);
        assert_eq!(health.unrestorable_records(), 2, "and again");
        let named = health.unrestorable_block().expect("named");
        assert!(
            named.ends_with("cafebabe (seq 7)"),
            "the surface names the most recent, not the first: {named}"
        );
    }
}
