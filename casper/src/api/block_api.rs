//! Block API surface (port of `BlockApi.scala`): the `BlockApi` trait plus the block-info
//! constructors shared with `BlockApiImpl`.

use async_trait::async_trait;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rchain_block_storage::dag::dag_storage::DeployId;
use rchain_models::ast::Par;
use rchain_models::block_metadata::BlockMetadata;
use rchain_models::casper::protocol::casper_message::{BlockMessage, SignedDeployData};
use rchain_models::casper::protocol::deploy_service::{
    BlockInfo, BondInfo, ContinuationsWithBlockInfo, DataWithBlockInfo, DeployExecStatus,
    FinalityAttestation, LightBlockInfo, RawBlock, Status,
};
use rchain_models::validator::Validator;
use rchain_shared::base16;
use rchain_shared::refined::NonNegI64;
use serde::{Deserialize, Serialize};

use rchain_block_storage::dag::dag_storage::{BlockDagStorage, HaltRecord};
use rchain_shared::lock::Unpoison;

use crate::runtime_manager::CapturedReply;

/// Milliseconds since the unix epoch, or `0` for a clock before it.
///
/// Total rather than fallible: a halt whose date could not be read is still a halt, and the count
/// beside it is what a reader acts on.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A block-api error (the Scala `BlockApi.Error = String`).
pub type ApiErr<A> = Result<A, String>;

/// What the node knows about one deployer key (`GET /api/v1/deployer/{pubkey}`): a block that
/// includes a deploy it signed — which means the key is public — and how far down the index reaches.
/// `block: None` is "not in any block" only when `indexed_from_height` is 0; above 0 the node is
/// still backfilling the blocks below that height after an upgrade, or (after last-finalized-state
/// sync) does not hold them at all. A multi-shard node searches every member shard and reports the
/// highest member's `indexed_from_height`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployerInfo {
    pub block: Option<LightBlockInfo>,
    pub indexed_from_height: i64,
}

/// The node's block-creation mode + deploy-gating capabilities, exposed to apps so a wallet can
/// decide whether to surface `propose` / the faucet instead of hardcoding a devnet flag.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    /// Continuous block production (`--autopropose`).
    pub autopropose: bool,
    /// Propose immediately after a deploy is accepted (`--propose-on-deploy`).
    pub propose_on_deploy: bool,
    /// Blocks are produced only by an explicit `propose` (neither of the above).
    pub manual_propose: bool,
    /// The admin HTTP surface (`POST /api/v1/propose` on 40405) is published and reachable from a
    /// browser (`--admin` + `--api-enable-devnet-cors`).
    pub admin_http: bool,
    /// Dev mode is on (`--dev-mode`).
    pub dev_mode: bool,
    /// **This node holds a validator identity for the shard and can propose at all.** Not a claim of
    /// being bonded or in the active set — that is a consensus read, and `manual_propose` cannot
    /// stand in for it: an unkeyed observer with no `--autopropose` computes
    /// `manual_propose = true`, exactly like a manual-mode validator. A client that wants to know
    /// whether a node can include a deploy reads this, not `manual_propose`.
    pub proposer: bool,
}

/// **The shard's autopropose halt, as the surface reads it** (W-N10-01, F-U10-02).
///
/// Three facts rather than the one boolean that used to stand for them: when the shard last halted,
/// how many times it has, and whether the record of the latest one reached the store. The flag alone
/// could not tell "halted" from "never halted" after a restart, and the operator's *prescribed* remedy
/// for a halt is a restart — so the remedy was what destroyed the record of the fault.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HaltStatus {
    /// When the shard last halted, in milliseconds since the unix epoch. `None` is **no halt on
    /// record**, which is a different fact from "halted at the epoch".
    pub at: Option<i64>,
    /// How many times this shard's timer has halted — the durable count, so it climbs across restarts
    /// where a per-process one would restart at zero.
    pub episodes: u64,
    /// Whether the latest halt reached the store. `false` means this process is honouring a halt the
    /// next one will not know about — said rather than implied.
    pub durable: bool,
}

/// **A shard's proposer health, as the API reports it.**
///
/// **The flag names its trigger, and that is the point.** Three paths feed the proposer queue — the
/// autopropose tap, the attest-on-new-blocks tap, and the admin `POST /api/propose` — and only the
/// autopropose *timer* stops when the failure count reaches `AUTOPROPOSE_MAX_CONSECUTIVE_FAILURES`
/// (`node/src/runtime/node_runtime.rs`). The other two keep running, so a node whose timer has halted
/// can still produce blocks on inbound traffic or on demand. A field called `halted` would claim more
/// than the code does; this one says which trigger stopped, and the counter is reported beside it
/// because that is what an operator needs to tell "quiet" from "broken" (#148's probe needs exactly
/// that witness — the API could not tell the three causes apart).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProposerHealth {
    /// Self-validation failures recorded in a row. The proposer clears it on a successful propose —
    /// "consecutive" is the whole quantity, since one success in between is a node that recovered.
    pub consecutive_self_validation_failures: u64,
    /// The shard's autopropose timer has stopped. The value is sticky — a later success does **not**
    /// clear it — and two things restart it: the process (the halt is durable, re-seeded at boot) and
    /// the operator's `POST /api/v1/autopropose/resume`. Not the same as production having stopped:
    /// the tap and the admin `POST /api/propose` keep running.
    pub autopropose_timer_halted: bool,
    /// **The shard's autopropose halt** — when, how many, and whether it is on record.
    ///
    /// This replaces an in-memory episode count whose own doc said it did *not* survive a restart. It
    /// did not, and saying so is what left F-U10-02 open; the record is durable now, and **obeyed**: a
    /// shard whose record stands does not start its timer, so a restart cannot silently resume
    /// production on a fault nobody has cleared. See [`HaltStatus`].
    pub autopropose_halt: HaltStatus,
    /// Autopropose requests the validated-block tap refused because the timer is halted (W-N4-08).
    pub autopropose_tap_suppressed: u64,
    /// Stale-snapshot self-equivocations (§48): the node's own block collided with its already-synced
    /// block at the sequence number it derived. Not a self-validation failure — the DAG advanced under
    /// the proposal — so it is counted separately and does **not** halt the timer.
    pub stale_snapshot_self_equivocations: u64,
}

/// The per-shard cell behind [`ProposerHealth`]: the proposer bumps the counter, the autopropose loop
/// records the halt, and the API reads both. Shared by `Clone` (every field is an `Arc`), so all three
/// holders see one cell rather than three copies.
#[derive(Clone, Debug, Default)]
pub struct ProposeHealth {
    failures: Arc<AtomicU64>,
    timer_halted: Arc<AtomicBool>,
    /// Monotone, and bumped only when the flag *transitions* to set — see [`ProposeHealth::note_timer_halted`].
    /// **Seeded from the durable record at boot**, so it is the shard's count and not this process's.
    timer_halt_episodes: Arc<AtomicU64>,
    /// Autopropose requests the validated-block tap refused because the timer is halted (W-N4-08).
    /// Counted rather than silently dropped: the halt is only a halt if the tap obeys it, and a
    /// refusal nobody can see is the shape that row is about.
    tap_suppressed_halted: Arc<AtomicU64>,
    /// **The halt on record**, as this process knows it: what the store held at boot, updated by
    /// [`ProposeHealth::note_timer_halted`]. The mirror is what keeps the surface from drifting from
    /// the store it reports — the same rule `note_reset_marker` follows for the reset marker.
    halt_record: Arc<Mutex<Option<HaltRecord>>>,
    /// **Whether the latest halt reached the store.** `false` means this process honours a halt the
    /// next one will not know about, which the surface says rather than implying durability.
    halt_durable: Arc<AtomicBool>,
    /// **The clear's wake.** A halted timer parks here instead of breaking, so the admin route resumes
    /// it in-process rather than the operator needing a second restart to get a live timer back.
    resume: Arc<tokio::sync::Notify>,
    stale_snapshot_equivocations: Arc<AtomicU64>,
}

impl ProposeHealth {
    pub fn new() -> Self {
        Self::default()
    }

    /// The counter the proposer bumps on a self-validation failure and clears on success — the same
    /// `Arc<AtomicU64>` `Proposer::apply` already takes, so the proposer needs no metrics handle of
    /// its own to be observable.
    pub fn failures(&self) -> Arc<AtomicU64> {
        self.failures.clone()
    }

    /// The counter the proposer bumps on a stale-snapshot self-equivocation (§48) — observable so a
    /// node that never recovers from the race is still visible, without counting toward the halt.
    pub fn stale_snapshot_equivocations(&self) -> Arc<AtomicU64> {
        self.stale_snapshot_equivocations.clone()
    }

    /// The counter the validated-block tap bumps when it refuses to propose (W-N4-08).
    pub fn tap_suppressed_halted(&self) -> &Arc<AtomicU64> {
        &self.tap_suppressed_halted
    }

    /// Whether the autopropose timer has stopped. The cell the timer stops on, and the one the
    /// validated-block tap has to consult: a halt that the tap ignores is not a halt (W-N4-08).
    pub fn timer_halted(&self) -> bool {
        self.timer_halted.load(Ordering::Relaxed)
    }

    /// **Seed the cell from the durable record at boot** (W-N10-01) — the read that lets a halt
    /// outlive the process that took it.
    ///
    /// Called after the shard's store exists and **before its timer is spawned**; a seed placed after
    /// the spawn races the first tick, which is the way this is most likely to be got wrong.
    pub fn seed_halt(&self, record: Option<HaltRecord>) {
        if record.is_some() {
            self.timer_halted.store(true, Ordering::Relaxed);
        }
        self.timer_halt_episodes
            .store(record.map(|r| r.episodes).unwrap_or(0), Ordering::Relaxed);
        self.halt_durable.store(record.is_some(), Ordering::Relaxed);
        *self.halt_record.lock().unpoison() = record;
    }

    /// **Read this shard's halt record from the store and seed the cell from it** — the boot read that
    /// lets a halt outlive the process that took it (W-N10-01, F-U10-02).
    ///
    /// Extracted from `setup_shard_runtime` so the seed — the line most likely to be dropped without a
    /// test noticing — is a function a test drives through the *real* store rather than a
    /// re-implementation of it. It must be called after the shard's store exists and **before its timer
    /// is spawned**; a seed placed after the spawn races the first tick.
    ///
    /// **A read that fails is not a boot refusal**, and the error is returned rather than swallowed so
    /// the caller can say so: this is the record of an observability fact, and refusing to start over
    /// it would trade a silence for an outage. The cell is seeded empty either way, which reads as
    /// `durable: false` — **"unknown", not "never halted"**.
    pub async fn seed_halt_from_store(&self, dag: &dyn BlockDagStorage) -> Result<(), String> {
        match dag.read_halt_record().await {
            Ok(record) => {
                self.seed_halt(record);
                Ok(())
            }
            Err(e) => {
                self.seed_halt(None);
                Err(e)
            }
        }
    }

    /// Record that the autopropose timer has stopped, and hand back **the record the caller must
    /// persist** — the timer cannot write the store itself, and it has already decided to stop, so it
    /// cannot refuse either.
    ///
    /// **The episode is counted on the transition, not on the call**: `swap` returns the previous
    /// value, so a second call in one process is not a second halt. The count starts from what the
    /// store held, so it is the shard's count across restarts.
    pub fn note_timer_halted(&self) -> HaltRecord {
        let first = !self.timer_halted.swap(true, Ordering::Relaxed);
        let episodes = if first {
            self.timer_halt_episodes.fetch_add(1, Ordering::Relaxed) + 1
        } else {
            self.timer_halt_episodes.load(Ordering::Relaxed)
        };
        // `durable: false` until the caller's write returns — the pessimistic half, because a reading
        // that claims durability the write has not established is the defect this record exists for.
        let record = HaltRecord {
            at_ms: now_ms(),
            episodes,
            durable: false,
        };
        self.halt_durable.store(false, Ordering::Relaxed);
        *self.halt_record.lock().unpoison() = Some(record);
        record
    }

    /// Record whether [`Self::note_timer_halted`]'s write reached the store.
    pub fn mark_halt_durable(&self, durable: bool) {
        self.halt_durable.store(durable, Ordering::Relaxed);
        if let Some(record) = self.halt_record.lock().unpoison().as_mut() {
            record.durable = durable;
        }
    }

    /// **The one way out of a halt**: forget it, and wake a parked timer so production resumes without
    /// a restart. Reached by `POST /api/v1/autopropose/resume` on the admin listener.
    ///
    /// **The failure counter is cleared with the flag, or the resume is a formality.** The halt fires on
    /// `consecutive_self_validation_failures >= MAX`, and a halted timer is not proposing — so nothing
    /// clears that counter while the halt stands. Waking the timer with it still at the threshold would
    /// re-halt on the very next tick, before any proposal could succeed and clear it, which would make
    /// the operator's one way out a no-op. Clearing it is what lets the retry the halt was deferring
    /// actually happen; a successful propose then clears it the ordinary way if the fault is gone, and a
    /// persisting fault re-halts after `MAX` more — which is the honest outcome, not a lost cause.
    pub fn clear_halt(&self) {
        self.timer_halted.store(false, Ordering::Relaxed);
        self.failures.store(0, Ordering::Relaxed);
        self.halt_durable.store(true, Ordering::Relaxed);
        *self.halt_record.lock().unpoison() = None;
        // `notify_one`, not `notify_waiters`: a permit is *stored* when nobody is parked, so a clear
        // that lands between the timer's halt and its park is not lost. `notify_waiters` wakes only
        // waiters already registered, which is a lost-wakeup race on the one path out of a halt.
        self.resume.notify_one();
    }

    /// The clear's wake, for a halted timer to park on.
    pub fn resumed(&self) -> Arc<tokio::sync::Notify> {
        self.resume.clone()
    }

    /// **Park until this shard is not halted** — the autopropose timer's own gate (W-N10-01).
    ///
    /// Extracted from the timer loop so the obedience is a function a test can drive rather than five
    /// lines buried in a spawned task. Its whole content is the flag check: the timer used to consult
    /// only `consecutive_self_validation_failures`, so a halt *seeded from the store at boot* would be
    /// reported by the surface while the loop kept proposing — the tap obeying the halt and the timer
    /// not, which is W-N4-08's defect one level up. Deleting the check (returning immediately) is the
    /// mutation that lets a node report a halt it does not honour.
    ///
    /// It parks on the notify rather than returning an error, so the operator's
    /// `POST /api/v1/autopropose/resume` resumes this shard **in-process**.
    pub async fn park_while_halted(&self) {
        while self.timer_halted() {
            self.resume.notified().await;
        }
    }

    /// All three values as one observation, so a reader cannot see a count from one instant and a flag
    /// from another.
    pub fn snapshot(&self) -> ProposerHealth {
        ProposerHealth {
            consecutive_self_validation_failures: self.failures.load(Ordering::Relaxed),
            autopropose_timer_halted: self.timer_halted.load(Ordering::Relaxed),
            autopropose_halt: HaltStatus {
                at: self.halt_record.lock().unpoison().as_ref().map(|r| r.at_ms),
                episodes: self.timer_halt_episodes.load(Ordering::Relaxed),
                durable: self.halt_durable.load(Ordering::Relaxed),
            },
            autopropose_tap_suppressed: self.tap_suppressed_halted.load(Ordering::Relaxed),
            stale_snapshot_self_equivocations: self
                .stale_snapshot_equivocations
                .load(Ordering::Relaxed),
        }
    }
}

/// The merge and finality observations a node can report, in the [`ProposerHealth`] shape.
///
/// **Why this is a surface at all.** The finality-stall *reason* and the merge's report both reached the
/// **log** and nothing else: a node that had stopped advancing said so only to whoever was reading
/// stderr, and the incident that started the failure-mode audit produced no counter and no line
/// anywhere. These are the values the status surface carries (C249's F-U10-01 and F-U4-02); the statics
/// that back them live beside their writers in `crate::interpreter_util`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FinalityHealth {
    /// The gate's verdict on the **last block validated**, rendered — `None` when that derivation
    /// advanced. **Not** a health verdict, and not an alarm: on a producing chain this is usually
    /// `Some(..)` while the chain is perfectly healthy, because a block justifies only the messages
    /// that existed when it was built (#318). Read [`stall_sustained`](Self::stall_sustained) for the
    /// distinction.
    pub stall_reason: Option<String>,
    /// **Whether the derivation has been refused long enough to be stalled** — a run of refusals
    /// reaching twice the partition, so it has outlasted a round. This is the state to alarm on; the
    /// reason beside it is the diagnosis for whichever state the node is in.
    pub stall_sustained: bool,
    /// How many times this process has **entered** a stall — the sustained state above, monotone — so a
    /// reader can tell "stalled now" from "stalled and recovered" without watching.
    pub stall_episodes: u64,
    /// Merges whose report was not quiet: a dropped chain, or an invariant violation (#280). The log
    /// carries the detail; this is the number the live incident would have moved.
    pub non_quiet_merge_reports: u64,
    /// **How many blocks this node has given up on** (C249's R3 gap): a `Divergence` record whose
    /// revalidation budget is spent and which still fails, so the node refuses it and its descendants
    /// for the process lifetime. Nothing acts on this yet — it is the denominator a reset would be
    /// reviewed against, shipped dark.
    pub unrestorable_records: u64,
    /// …and *which* block, as `<hash> (seq <n>)` — the count says it happened, this says where.
    pub unrestorable_block: Option<String>,
    /// **Whether the three counters above are the shard's** (W-N10-02): `true` when they were seeded
    /// from (or written to) the shard's record, `false` when the store could not be reached — in which
    /// case they are this process's and the surface says so rather than letting a restart read as a
    /// shard that never stalled.
    pub counters_durable: bool,
}

/// The LFS sync task's own state, in the [`FinalityHealth`] shape — what `GET /api/v1/status` carries
/// under `sync`.
///
/// **Why a surface at all.** A node stuck in `NodeSyncing` was invisible: the retries logged to
/// stderr and nothing else moved, and the status API carried no trace of it, so a probe could not
/// tell a node that was syncing from one that was fine. The measured incident
/// (`spec/audit/evidence/n-anchor-drill/c268-joiner-page-walk-loop.txt`) was joiners answering the
/// status API at height 0 for ten minutes with no way to see why. The statics that back this live
/// beside their writer in [`crate::engine::node_syncing`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncHealth {
    /// Attempts the sync task has **started**.
    pub attempts: u32,
    /// The last named failure's message, verbatim. `None` while the attempt now running has not
    /// failed.
    pub last_failure: Option<String>,
}

/// **One member shard's health**, as the per-shard map carries it (W-N10-04): the shard's proposer
/// (including its autopropose halt) and its finality observations, together, because a consumer
/// reading one shard's health wants both and two maps for two facts is two lookups that can disagree.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShardHealthEntry {
    pub proposer: ProposerHealth,
    pub finality: FinalityHealth,
}

/// **A deploy refusal that is a bound, not a malformed request** (Law 86a). The pool's count/byte
/// bounds, a deployer's share and the per-deployer rate are all "come back", not "your request is
/// wrong"; the HTTP and gRPC surfaces must say so with 429/`RESOURCE_EXHAUSTED` and a `Retry-After`,
/// never with a 400. The strings are the pool's own (`casper/src/dag.rs`), and this is the one place
/// the two surfaces agree on the classification.
pub fn deploy_refusal_is_bounded(message: &str) -> bool {
    message.starts_with("deploy pool is full")
        || message.contains("holds its share of the deploy pool")
        || message.contains("deploy rate limit exceeded")
}

/// **The deploy pool's capacity**, as the public status surface reads it (Law 86a).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeployPoolStats {
    pub deploys: usize,
    pub max_deploys: usize,
    pub bytes: usize,
    pub max_bytes: usize,
}

impl DeployPoolStats {
    /// Room under both the count and the term-byte bound. Per-deployer shares and the rate limiter
    /// are submitter bounds, not pool capacity; a node with room may still refuse one deployer.
    pub fn has_room(&self) -> bool {
        self.deploys < self.max_deploys && self.bytes < self.max_bytes
    }
}

impl Default for DeployPoolStats {
    fn default() -> Self {
        DeployPoolStats {
            deploys: 0,
            max_deploys: usize::MAX,
            bytes: 0,
            max_bytes: usize::MAX,
        }
    }
}

/// The block API (port of `BlockApi[F]`). Implementations read from the block store/DAG and drive
/// propose via the runtime.
#[async_trait]
pub trait BlockApi: Send + Sync {
    async fn status(&self) -> Status;

    async fn deploy(&self, deploy: &SignedDeployData) -> ApiErr<String>;

    async fn deploy_status(&self, deploy_id: &DeployId) -> ApiErr<DeployExecStatus>;

    /// The currently-pooled (not-yet-included) deploys.
    async fn pooled_deploys(&self) -> ApiErr<Vec<SignedDeployData>>;

    /// **The pool's capacity, for the public `acceptsDeploys` answer** (Law 86a). Admission is the
    /// pool's decision, so the status surface reads the pool's own bounds rather than the advisory
    /// resource word. The default is unbounded for stubs that model an API, not a pool.
    async fn deploy_pool_stats(&self) -> ApiErr<DeployPoolStats> {
        Ok(DeployPoolStats::default())
    }

    /// The node's block-creation mode + deploy-gating capabilities.
    async fn capabilities(&self) -> Capabilities;

    /// #157: the shard's proposer health — the consecutive self-validation failure count and whether
    /// the autopropose timer has stopped.
    ///
    /// **The default is for stubs**, which model an API surface rather than a proposer and have no
    /// failing proposer to report; a reader must not take it as "observed healthy". The one
    /// implementation that matters is `BlockApiImpl`'s, which reads the live cell the proposer writes.
    async fn proposer_health(&self) -> ProposerHealth {
        ProposerHealth::default()
    }

    /// C249's F-U10-01: the merge's and finality's observations, **of this implementation's primary
    /// shard**. Every shard's reading is [`health_by_shard`](Self::health_by_shard).
    ///
    /// **A required method now, and the change is the finding.** It used to default to reading seven
    /// process-global statics, with a deliberate argument that a stub "reports the truth rather than a
    /// polite zero". The statics were the defect: a refusal in one shard appeared in every shard's
    /// health (W-N10-03), and a restart zeroed counters the shard's own history had moved (W-N10-02).
    /// With one cell per shard there is no process-wide reading left for a default to return, and a
    /// defaulted method is the #352 defect — a wrapper that does not forward it silently answers the
    /// old thing. So it is required, and a stub's honest answer is `FinalityHealth::default()`, which
    /// this trait's docs say is "nothing observed" and never "observed healthy".
    async fn finality_health(&self) -> FinalityHealth;

    /// **Every member shard's proposer-and-finality health, keyed by shard id** (W-N10-04).
    ///
    /// A node's `proposer_health` and `finality_health` are its **primary's**; a shard whose timer has
    /// halted off the primary was invisible before this. The map is **total over the node's members** —
    /// a partial map beside the total flat fields is a trap for a consumer that zips them — and the
    /// router builds it by merging its members, so a wrapper that failed to forward it would be a
    /// compile error rather than a silent omission.
    async fn health_by_shard(&self) -> BTreeMap<String, ShardHealthEntry>;

    /// Law 69's other half: the LFS sync task's own state — how many attempts it has started and why
    /// the last one failed. **`None` is a reading too**: it means the task has returned, which covers
    /// a restored state *and* a node that never had to sync. It does not mean "the last sync
    /// succeeded", and a surface that reported `Some(default)` here would say exactly that.
    ///
    /// **The default reads this process's own static rather than returning `None` unconditionally**,
    /// for the reason [`finality_health`](Self::finality_health) gives: the value is process-wide state
    /// the sync task writes, so reading it is correct for every implementation and a stub reports the
    /// truth rather than a polite absence.
    async fn sync_health(&self) -> Option<SyncHealth> {
        crate::engine::node_syncing::sync_state().map(|state| SyncHealth {
            attempts: state.attempts,
            last_failure: state.last_failure,
        })
    }

    async fn create_block(&self, is_async: bool) -> ApiErr<String>;

    async fn get_propose_result(&self) -> ApiErr<String>;

    /// [`Self::get_listening_name_data_response`], **without the finality floor** — for reading the
    /// reply channel of a deploy this node submitted itself.
    ///
    /// The floor is a rule about what a *caller* may read: a block above the last finalised one may
    /// yet be rolled back, so a value read out of it is a value from a chain that does not exist yet
    /// (W-N3-04). The OCapN bridge is not that caller. It waits on the reply channel of a deploy it
    /// has just had included, and the value it is waiting for is in a block **this node produced** —
    /// which, on a chain whose finality lags and on every single-validator fixture, where finality
    /// never advances past genesis at all, is above the floor by construction. Waiting for that block
    /// to finalise before answering would make the bridge unable to answer, which is a worse failure
    /// than the one the floor prevents, and it is measured: the three multi-delivery OCapN
    /// integration tests fail on the floored read and pass on this one.
    ///
    /// **Required rather than defaulted, and that is the fix rather than the ceremony.** A default
    /// that forwarded to the floored read did exactly the wrong thing for the one implementation that
    /// mattered — `ShardRoutingBlockApi` routes each call to the shard that owns the data, and a
    /// wrapper that does not forward this one silently answers with the floored read, so the bridge
    /// went back to timing out on chains whose finality lags. Measured: the three multi-delivery OCapN
    /// integration tests fail with the default and pass with the forwarding. An implementation that
    /// does not say what its unfinalised view is now does not compile.
    /// **Clear an autopropose halt and let this shard produce again** (W-N10-01, F-U10-02) — the one
    /// way out of a halt that is now durable, reached by `POST /api/v1/autopropose/resume`.
    ///
    /// **Required rather than defaulted**, for the reason `get_unfinalised_name_data_response` records:
    /// a default would let a wrapper answer this for one shard and silently do nothing for the rest,
    /// which is the same defect as the halt itself — a surface that says one thing and a node that does
    /// another. An implementation that does not say what it clears does not compile.
    async fn clear_autopropose_halt(&self) -> Result<(), String>;

    async fn get_unfinalised_name_data_response(
        &self,
        depth: i32,
        listening_name: &Par,
    ) -> ApiErr<(Vec<DataWithBlockInfo>, i32)>;

    async fn get_listening_name_data_response(
        &self,
        depth: i32,
        listening_name: &Par,
    ) -> ApiErr<(Vec<DataWithBlockInfo>, i32)>;

    async fn get_listening_name_continuation_response(
        &self,
        depth: i32,
        listening_names: &[Par],
    ) -> ApiErr<(Vec<ContinuationsWithBlockInfo>, i32)>;

    async fn get_blocks_by_heights(
        &self,
        start_block_number: i64,
        end_block_number: i64,
    ) -> ApiErr<Vec<LightBlockInfo>>;

    async fn visualize_dag(
        &self,
        depth: i32,
        start_block_number: i32,
        show_justification_lines: bool,
    ) -> ApiErr<Vec<String>>;

    async fn machine_verifiable_dag(&self, depth: i32) -> ApiErr<String>;

    async fn get_blocks(&self, depth: i32) -> ApiErr<Vec<LightBlockInfo>>;

    async fn find_deploy(&self, id: &DeployId) -> ApiErr<LightBlockInfo>;

    /// Whether the key whose `blake2b256` hash is `deployer_hash` has signed a deploy in a block (the
    /// deployer index). The default refuses: a surface without the index cannot say a key is unseen.
    async fn find_deployer(&self, _deployer_hash: &[u8]) -> ApiErr<DeployerInfo> {
        Err("this node keeps no deployer index".to_string())
    }

    async fn get_block(&self, hash: &str) -> ApiErr<BlockInfo>;

    async fn bond_status(&self, public_key: &[u8]) -> ApiErr<bool>;

    /// The reply *and where it was read from*: a bare `Vec<Par>` cannot say whether the term
    /// produced nothing or replied on a channel the node does not read (AUDIT C38).
    async fn exploratory_deploy(
        &self,
        term: &str,
        block_hash: Option<&str>,
        use_pre_state_hash: bool,
    ) -> ApiErr<(CapturedReply, LightBlockInfo)>;

    async fn get_data_at_par(
        &self,
        par: &Par,
        block_hash: &str,
        use_pre_state_hash: bool,
    ) -> ApiErr<(Vec<Par>, LightBlockInfo)>;

    async fn last_finalized_block(&self) -> ApiErr<BlockInfo>;

    /// **The preimage of a block's hash, so a client can verify what it was given** (AUDIT C261's
    /// missing artefact). `get_block` answers with a summary a client cannot recompute the hash of;
    /// this answers with the bytes the hash *is* taken over. Default refuses, so a surface that has no
    /// block store need not implement it.
    async fn get_raw_block(&self, _hash: &str) -> ApiErr<RawBlock> {
        Err("this surface serves no raw block".to_string())
    }

    /// **The node's own finalised block, signed by its validator key** — the artefact that makes a
    /// finality claim attributable to a bonded validator rather than a node-local assertion. Default
    /// refuses: a surface with no validator identity cannot attest for anyone.
    async fn finality_attestation(&self, _nonce: &str) -> ApiErr<FinalityAttestation> {
        Err("this surface holds no validator identity to attest with".to_string())
    }

    async fn is_finalized(&self, hash: &str) -> ApiErr<bool>;

    async fn get_latest_message(&self) -> ApiErr<BlockMetadata>;
}

/// Build a bond info (port of `bondToBondInfo`).
pub fn bond_to_bond_info(bond: (&Validator, NonNegI64)) -> BondInfo {
    BondInfo {
        validator: base16::encode(bond.0.as_bytes()),
        stake: i64::from(bond.1),
    }
}

/// Build the full block info (port of `getFullBlockInfo`).
pub fn get_full_block_info(block: &BlockMessage) -> BlockInfo {
    construct_block_info(block)
}

/// Build the light block info (port of `getLightBlockInfo`).
pub fn get_light_block_info(block: &BlockMessage) -> LightBlockInfo {
    construct_light_block_info(block)
}

fn construct_block_info(block: &BlockMessage) -> BlockInfo {
    let light_block_info = construct_light_block_info(block);
    let deploys = block
        .state
        .deploys
        .iter()
        .map(|d| d.to_deploy_info())
        .collect();
    BlockInfo {
        block_info: light_block_info,
        deploys,
    }
}

fn construct_light_block_info(block: &BlockMessage) -> LightBlockInfo {
    LightBlockInfo {
        version: block.version,
        shard_id: block.shard_id.clone(),
        block_hash: base16::encode(block.block_hash.as_bytes()),
        block_number: i64::from(block.block_number),
        sender: base16::encode(block.sender.as_bytes()),
        seq_num: i64::from(block.seq_num),
        pre_state_hash: base16::encode(block.pre_state_hash.as_bytes()),
        post_state_hash: base16::encode(block.post_state_hash.as_bytes()),
        justifications: block
            .justifications
            .iter()
            .map(|h| base16::encode(h.as_bytes()))
            .collect(),
        bonds: block
            .bonds
            .iter()
            .map(|(v, s)| bond_to_bond_info((v, *s)))
            .collect(),
        sig_algorithm: block.sig_algorithm.clone(),
        sig: base16::encode(&block.sig),
        block_size: block.to_bytes().len().to_string(),
        deploy_count: block.state.deploys.len() as i32,
        rejected_deploys: block
            .rejected_deploys
            .iter()
            .map(|d| base16::encode(d))
            .collect(),
        timestamp: block.timestamp,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_models::block_hash::BlockHash;
    use rchain_models::casper::protocol::casper_message::RholangState;
    use std::collections::{BTreeMap, BTreeSet};

    fn block() -> BlockMessage {
        BlockMessage {
            version: 1,
            shard_id: "root".to_string(),
            block_hash: BlockHash::new([1u8; 32]),
            block_number: 5.try_into().unwrap(),
            sender: Validator::new([2u8; 65]),
            seq_num: 3.try_into().unwrap(),
            pre_state_hash: rchain_models::block::state_hash::StateHash::new([0xab; 32]),
            post_state_hash: rchain_models::block::state_hash::StateHash::new([0xcd; 32]),
            justifications: vec![BlockHash::new([9u8; 32])],
            bonds: BTreeMap::from([(Validator::new([2u8; 65]), 100.try_into().unwrap())]),
            rejected_deploys: BTreeSet::new(),
            rejected_blocks: BTreeSet::new(),
            rejected_senders: BTreeSet::new(),
            state: RholangState::default(),
            sig_algorithm: "secp256k1".to_string(),
            sig: vec![0xee],
            timestamp: 1_700_000_000_000,
        }
    }

    #[test]
    fn light_block_info_renders_hashes_as_hex() {
        let info = get_light_block_info(&block());
        assert_eq!(info.block_number, 5);
        assert_eq!(info.seq_num, 3);
        assert_eq!(info.deploy_count, 0);
        assert_eq!(info.bonds.len(), 1);
        assert_eq!(info.bonds[0].stake, 100);
        assert!(info.block_hash.starts_with("0101"));
        // The new block format's informational timestamp is exposed to clients.
        assert_eq!(info.timestamp, 1_700_000_000_000);
    }

    #[test]
    fn full_block_info_carries_deploys() {
        let info = get_full_block_info(&block());
        assert_eq!(info.block_info.block_number, 5);
        assert!(info.deploys.is_empty());
    }

    /// **The witness #157 is about, at the seam where its two halves meet.**
    ///
    /// The value the API reads is the cell the proposer writes — not a copy taken at setup, which
    /// would read zero for ever — and the two halves of the snapshot keep their distinct semantics: the
    /// count is *consecutive* (a success clears it), while the halt is *sticky* (the timer really has
    /// stopped, and a later success is exactly the case where a reader must not conclude the node is
    /// healthy).
    #[test]
    fn propose_health_is_the_live_cell_and_the_halt_outlives_a_success() {
        let health = ProposeHealth::new();
        assert_eq!(health.snapshot(), ProposerHealth::default());

        // The proposer's handle and the reader's view are the same cell.
        let proposer_handle = health.failures();
        proposer_handle.fetch_add(1, Ordering::Relaxed);
        proposer_handle.fetch_add(1, Ordering::Relaxed);
        assert_eq!(health.snapshot().consecutive_self_validation_failures, 2);
        assert!(!health.snapshot().autopropose_timer_halted);

        health.note_timer_halted();
        // The node recovers and proposes: the count clears, the halt does not.
        proposer_handle.store(0, Ordering::Relaxed);
        let snap = health.snapshot();
        assert_eq!(snap.consecutive_self_validation_failures, 0);
        assert!(
            snap.autopropose_timer_halted,
            "a success clears the count, not the halt"
        );
        assert_eq!(snap.autopropose_halt.episodes, 1, "one halt is one episode");
        assert!(
            snap.autopropose_halt.at.is_some(),
            "a halt on record names when it happened"
        );
        assert!(
            !snap.autopropose_halt.durable,
            "the write has not returned yet, so the record is not yet durable"
        );
        assert_eq!(snap.autopropose_tap_suppressed, 0);
        assert_eq!(snap.stale_snapshot_self_equivocations, 0);
    }

    /// **The count is of halts, not of calls** (W-N10-01).
    ///
    /// A second `note_timer_halted` in the same process — a timer that stops twice, or a caller that
    /// records the same halt twice — must not move it. Falsified by dropping the `swap` guard and
    /// storing unconditionally, under which the count is 2 and this says so.
    ///
    /// **This test does not assert anything about a restart, and the first version of this comment
    /// claimed it did.** `autopropose_timer_halted`'s real ambiguity — a restarted node reporting
    /// `false`, indistinguishable from one that never halted — was untouched by the in-memory counter
    /// this test exercises. That case is F-U10-02, and it is answered one level up: the durable record
    /// and the boot seed (see [`ProposeHealth::seed_halt`] and
    /// `casper/tests/reset.rs::a_halt_written_before_the_restart_is_read_at_the_next_boot`). The
    /// in-process count here still counts *transitions*, which is correct whether the seed came from
    /// the store or from zero.
    #[test]
    fn a_second_halt_in_one_process_is_not_a_second_episode() {
        let health = ProposeHealth::new();
        assert_eq!(health.snapshot().autopropose_halt.episodes, 0);

        health.note_timer_halted();
        assert_eq!(
            health.snapshot().autopropose_halt.episodes,
            1,
            "the first halt is an episode"
        );
        health.note_timer_halted();
        health.note_timer_halted();
        assert_eq!(
            health.snapshot().autopropose_halt.episodes,
            1,
            "and further notes of the *same* halt are not: the episode is the transition, not the call"
        );

        // The tap's own count is separate and monotone, so "the halt cost this many proposals" is a
        // fact beside "the halt happened once" (W-N4-08).
        health
            .tap_suppressed_halted()
            .fetch_add(3, Ordering::Relaxed);
        assert_eq!(health.snapshot().autopropose_tap_suppressed, 3);
    }
}
