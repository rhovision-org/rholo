//! Web API request/response data types (port of the DTOs in `api/WebApi.scala`).

use std::fmt;

use rchain_models::casper::protocol::casper_message::DeployData;
use rchain_models::casper::protocol::deploy_service::LightBlockInfo;
use serde::{Deserialize, Serialize};

use super::rho_expr::{RhoExpr, RhoUnforg};

/// A deploy request (port of `DeployRequest`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployRequest {
    pub data: DeployData,
    pub deployer: String,
    pub signature: String,
    pub sig_algorithm: String,
}

/// An exploratory-deploy request (port of `ExploreDeployRequest`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExploreDeployRequest {
    pub term: String,
    pub block_hash: String,
    pub use_pre_state_hash: bool,
}

/// A data-at-name request (port of `DataAtNameRequest`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataAtNameRequest {
    pub name: RhoUnforg,
    pub depth: i32,
}

/// A data-at-name-by-block-hash request (port of `DataAtNameByBlockHashRequest`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataAtNameByBlockHashRequest {
    pub name: RhoExpr,
    pub block_hash: String,
    pub use_pre_state_hash: bool,
}

/// A faucet request: the REV address to fund (devnet-only endpoint).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaucetRequest {
    pub address: String,
}

/// **What the node knows about a drip when it answers** (#247).
///
/// Submission is not delivery, and the endpoint used to return the same bare `{deployId, amount, to}`
/// whether the drip was on its way or had already failed its phlo pre-charge — which is how an *unfunded*
/// faucet signer stayed invisible on the live testnet while every client reported success.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FaucetDripStatus {
    /// A deploy was submitted and its outcome is not yet known; poll `deploy-status/{deployId}`.
    Pending,
    /// The same, except the deploy is the *original* signed drip replayed after a crash, so the id is
    /// unchanged and nothing new was signed.
    Resubmitted,
    /// The deploy identified by `deployId` was processed **with an error**: nothing was delivered. The
    /// reservation has been refunded, so a later call may retry.
    Failed,
}

/// A faucet response: the deploy id of the transfer (poll `deploy-status/{deployId}` for the result).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaucetResponse {
    pub deploy_id: String,
    pub amount: i64,
    pub to: String,
    /// What the node knows about this drip *at answer time* — see [`FaucetDripStatus`].
    pub status: FaucetDripStatus,
    /// The replay error when `status` is `failed`, `null` otherwise.
    ///
    /// **The key is always present, deliberately.** The envelope register compares key *sets*, so a
    /// `skip_serializing_if` here would drop `deployError` from the serialized shape and fail the corpus
    /// check — and would also mean a client could not distinguish "no error" from "this node does not
    /// report errors".
    pub deploy_error: Option<String>,
}

/// API/node version info (port of `VersionInfo`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    pub api: String,
    pub node: String,
}

/// **What any caller may read**: the chain-public half of the node's status (W-N3-01).
///
/// It was one struct. The other half — divergence records, the safety halt, poison recoveries, the
/// sync task's state — is operator state, and it was served to anyone who could reach the public port.
/// Split rather than gated, and split *structurally*: `to_public_status` takes no health, finality or
/// reporter argument, so a field added to one of those cannot leak here by someone forgetting to edit
/// a serializer — it does not compile. That is the difference between this and one struct with two
/// serializers, and it is the reason the split is two types.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicStatus {
    pub version: VersionInfo,
    pub address: String,
    pub network_id: String,
    pub shard_id: String,
    pub peers: i32,
    pub nodes: i32,
    pub min_phlo_price: i64,
    pub latest_block_number: i64,
    /// Continuous block production (`--autopropose`).
    pub autopropose: bool,
    /// Propose immediately after a deploy is accepted (`--propose-on-deploy`).
    pub propose_on_deploy: bool,
    /// Blocks are produced only by an explicit `propose`.
    pub manual_propose: bool,
    /// Dev mode is on. Public: it is a configuration fact, and a client that must not be handed
    /// dev-only answers has to be able to see it.
    pub dev_mode: bool,
    /// **The sampled resource state** (A1): `normal` while the host facts are readable, `unavailable`
    /// when they are not. A word, not a number — numeric host data stays on the operator surface. The
    /// pressure states (`throttled`/`critical`) arrive with the fallback-state unit.
    pub degraded: String,
    /// Whether the node is accepting deploys right now. True until the admission/fallback units land;
    /// the field is the contract, and a node under pressure will make it false.
    pub accepts_deploys: bool,
}

/// **The shard's autopropose halt**, beside the episode count the surface already carried — which
/// this makes durable (W-N10-01, F-U10-02).
///
/// **Three facts the one boolean could not carry.** `autoproposeTimerHalted` is this process's flag;
/// this says *when* the shard last halted, and whether the reading can be trusted. The scheduled
/// remedy for a halt is a restart, so the flag alone was an all-clear that erased the fault.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HaltStatusDto {
    /// When the shard last halted, in milliseconds since the unix epoch. `null` is **no halt on
    /// record** — unless `durable` is `false`, in which case it is *unknown*.
    pub at: Option<i64>,
    /// Whether this reading reflects the store. `false` means the halt record could not be read or
    /// written, so the node cannot vouch for `at` — said rather than implied, because "unknown" and
    /// "never halted" are the two readings this whole row is about confusing.
    pub durable: bool,
}

/// **The one shard's proposer health**, as the per-shard map carries it (W-N10-04).
///
/// The same facts the flat fields beside it carry, but attributable to a named member: a node whose
/// non-primary shard has halted reports a healthy `autoproposeTimerHalted` at the top level, because
/// that field is the **primary's**. This is where the other members' halts are.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposerHealthDto {
    pub consecutive_self_validation_failures: u64,
    pub autopropose_timer_halted: bool,
    pub autopropose_timer_halt_episodes: u64,
    pub autopropose_halt: HaltStatusDto,
    pub autopropose_tap_suppressed: u64,
    pub stale_snapshot_self_equivocations: u64,
}

/// **The one shard's finality-and-merge health**, as the per-shard map carries it (W-N10-03/04).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalityHealthDto {
    pub stall_reason: Option<String>,
    pub stall_sustained: bool,
    /// The **shard's** stall count — persisted across a restart (W-N10-02). `countersDurable: false`
    /// means the store could not be read or written, in which case this is this *process's* count and
    /// the node says so rather than letting it read as the shard's history.
    pub stall_episodes: u64,
    pub non_quiet_merge_reports: u64,
    pub unrestorable_records: u64,
    pub unrestorable_block: Option<String>,
    /// Whether the three counts above are the shard's history (`true`) or this process's running
    /// totals (`false`) — the same honesty rule [`HaltStatusDto::durable`] keeps.
    pub counters_durable: bool,
}

/// **One member shard's health, keyed by shard id** (W-N10-04) — the value of `OperatorStatus::shards`.
///
/// **No `scope` field on this object, and that is deliberate**: the map's key *is* the scope. A
/// per-object string would be a second place to say what the key already says, and the two could
/// disagree. The scopes that need saying are on the **flat** status, where fields of different
/// lifetimes sit beside one another.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShardHealthDto {
    pub proposer: ProposerHealthDto,
    pub finality: FinalityHealthDto,
}

/// **What an operator's own surface may read, and nothing else does**: the operator half of the
/// node's status.
///
/// `#[serde(flatten)]` keeps it a superset of [`PublicStatus`], so a client that reads the operator
/// route sees every public field under the same name and can move between the two without a shape
/// change. The fields are the ones W-N3-01 named: a divergence record, the safety halt, the counters
/// that say whether a subsystem has stopped, the sync task's state, and the durable reset marker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorStatus {
    #[serde(flatten)]
    pub core: PublicStatus,
    /// The admin HTTP surface is published and CORS-enabled. **Operator state**, and the reason it is
    /// not public: it tells an unauthenticated caller that the actuator port is reachable.
    pub admin_http: bool,
    /// #157: self-validation failures the proposer has recorded in a row. `0` on a healthy node, and
    /// cleared by a successful propose — this is what tells "quiet" from "broken" on a chain whose
    /// steady state is a node that produces nothing until work arrives.
    pub consecutive_self_validation_failures: u64,
    /// #157: the shard's autopropose **timer** has stopped. Deliberately not called `halted`: the tap
    /// and the admin `POST /api/propose` keep running, so a node can have a stopped timer and still be
    /// producing blocks.
    ///
    /// **Two things restart it now**: the process (the halt is durable and re-seeded at boot), and the
    /// operator's `POST /api/v1/autopropose/resume` on the admin listener, which clears the record and
    /// wakes the parked timer in-process.
    pub autopropose_timer_halted: bool,
    /// **How many times this shard's autopropose timer has stopped** (W-N10-01), counted on the
    /// transition rather than on the call and **seeded from the durable record at boot**, so it is the
    /// shard's count and not this process's.
    ///
    /// A flat field rather than nested inside [`HaltStatusDto`] because consumers of this surface are
    /// written against it. The durability of the value is carried by the `durable` flag beside it, not
    /// by this field's position.
    pub autopropose_timer_halt_episodes: u64,
    /// **The shard's autopropose halt**: when, and whether the record is trustworthy. See
    /// [`HaltStatusDto`].
    pub autopropose_halt: HaltStatusDto,
    /// Autopropose requests the validated-block **tap** refused because the timer is halted (W-N4-08).
    /// Beside the episode count because it is the measurement of what the halt cost: without it, the
    /// halt was a flag the tap ignored.
    pub autopropose_tap_suppressed: u64,
    /// #156: stale-snapshot self-equivocations — the node's own block collided with its already-synced
    /// block at a sequence number derived from a stale parent set. Not a self-validation failure, so it
    /// does not count toward the timer halt; it is reported so a node that never recovers from the race
    /// is still visible.
    pub stale_snapshot_self_equivocations: u64,
    /// C249's F-U10-01: **the gate's verdict on the last block this node validated**. The reason existed
    /// only on stderr — a node that had stopped advancing said so to whoever was reading the log, and to
    /// nothing a probe could ask. `None` when that derivation advanced.
    ///
    /// **Not an alarm, and not "why finality is not advancing"** (#318): on a producing chain most
    /// blocks are refused by the stake gate, because a block justifies only the messages that existed
    /// when it was built — so this is usually `Some(..)` while the chain is healthy. The state to alarm
    /// on is `finalityStalled`, and this is its diagnosis.
    pub finality_stall: Option<String>,
    /// **Whether this node is stalled right now** — the derivation has been refused for longer than a
    /// round (twice the partition, in consecutive blocks), so the refusals are no longer the ordinary
    /// partial layers of a producing round.
    pub finality_stalled: bool,
    /// …and how many times this **shard** has entered a stall, so a reader can tell "stalled now" from
    /// "stalled and recovered" without watching. Counts the sustained state above, which is what makes
    /// the two distinguishable — before #318 it counted single refusals and climbed once per round on a
    /// healthy net. **Persisted** (W-N10-02): the count is the shard's history and survives the restart
    /// an operator's alarm would otherwise be cleared by. Read it with `finalityCountersDurable`, which
    /// is false when the store could not be reached and the count is therefore this process's.
    pub finality_stall_episodes: u64,
    /// **Whether the three shard-history counters are the shard's** (W-N10-02) — `stallEpisodes`,
    /// `nonQuietMergeReports` and `unrestorableRecords` below. `false` means the store could not be read
    /// or written, so they are this *process's* running totals; saying so keeps a restarted node from
    /// reading as one that never stalled.
    pub finality_counters_durable: bool,
    /// **The reset marker this node is standing under**, rendered — `None` when it is not mid-reset
    /// (F-U12-02). The marker is cleared only when the catch-up frontier passes its anchor, so a node
    /// that resets and never catches up keeps re-entering the boot guard; that lifetime was unbounded
    /// **and silent**, and this is the surface the ladder's R2 asks for. Read it with `unrestorableRecords`:
    /// a standing marker says the node is repairing, and one that stands while the height does not move
    /// says the repair is not converging.
    pub reset_marker: Option<String>,
    /// #280's ledger, counted: merges that returned a report which was not quiet — a dropped chain, or a
    /// violated invariant. The live incident produced no counter and no line anywhere moving; this is
    /// the number it would have moved, and the log still carries the detail.
    pub non_quiet_merge_reports: u64,
    /// **Law 69's other half: the LFS sync task's own state**, so a node stuck in `NodeSyncing` is
    /// visible on the status surface instead of silent. **Null when the node is not syncing** — which
    /// means "the sync task has returned", **not** "it synced fine": a node that never had to sync and
    /// one that finished cleanly both report null here.
    ///
    /// **Nullable rather than omitted**, which is what the sibling `finalityStall` does and what law
    /// 43's envelope catalog requires: that law checks a struct's key *set* against the DTO's own
    /// serialization, so a field that disappears when it is empty would make the catalog either
    /// incomplete (the AUDIT C29 drift) or wrong depending on the payload. A key that is always
    /// present and sometimes null is the shape the law can state.
    #[serde(default)]
    pub sync: Option<SyncStatus>,
    /// A poisoned lock was recovered from, which means a panic happened while shared state was held.
    /// Monotone and process-wide, because the poisoning is: the panic happened in whichever thread held
    /// the guard (`rspace::lock`). Counted rather than silently absorbed, which is F-U9-03.
    pub poison_recoveries: u64,
    /// **Since process start**, and no other scope is possible: the poisoning is the process's, so a
    /// restart correctly zeroes it. Named rather than left implicit so a reader does not compare it with
    /// the shard-history counters above as though the two had one lifetime (W-N10-02).
    pub poison_recoveries_scope: String,
    /// **How many blocks this node has given up on** (C249's R3 gap): a divergence record whose
    /// revalidation budget is spent and which still fails here, so the node refuses it and its
    /// descendants for the process lifetime. Nothing resets a running node yet — this is the
    /// denominator a reset would be reviewed against, shipped dark.
    pub unrestorable_records: u64,
    /// …and which block, as `<hash> (seq <n>)`. `None` when the node has never given one up.
    pub unrestorable_block: Option<String>,
    /// **How many supervised background tasks have panicked** (C254). The node has no supervisor: a
    /// long-lived task was a bare `tokio::spawn` whose handle was dropped, so nothing could tell one
    /// that was running from one that died at startup. A panic reached stderr; a plain early return
    /// reached nothing at all.
    pub tasks_panicked: u64,
    /// **Since process start** — a supervised task's death is this process's event (see
    /// `poison_recoveries_scope`).
    pub tasks_panicked_scope: String,
    /// …and how many have **returned**, which is the quieter death and the reason the two are counted
    /// apart: a panic announces itself once, a return does not announce itself anywhere.
    pub tasks_exited: u64,
    /// **Since process start** — see `poison_recoveries_scope`.
    pub tasks_exited_scope: String,
    /// **How many sends a closed channel swallowed** (C254). Every production `send` goes through
    /// `rchain_shared::chan`, which keeps the decision *and* counts it, so the count is the other half
    /// of the closed-channel door — the door made the event loud at the call site, this makes it
    /// countable on a surface. Monotone and process-wide. It was counted from the day the door landed
    /// and published by nothing, which its own module doc said out loud.
    pub undelivered_sends: u64,
    /// **Since process start** — the count lives in `rchain_shared::chan`'s process-wide cell, and a
    /// restart correctly zeroes it. See `poison_recoveries_scope`.
    pub undelivered_sends_scope: String,

    /// **Every member shard's health, keyed by shard id** (W-N10-04) — **total** over the node's
    /// members, because a partial map beside the total flat fields is a trap for a consumer that zips
    /// them. The flat fields above are the **primary's** reading; a non-primary shard whose autopropose
    /// timer has halted is visible here and nowhere else. `null` when the caller is not a node with
    /// members (a stub), which law 43's catalog carries as a nullable-but-always-present key.
    #[serde(default)]
    pub shards: Option<std::collections::BTreeMap<String, ShardHealthDto>>,

    /// **The sampled resource snapshot** (A1): the operator surface carries the numbers and their
    /// provenance; the public surface carries only `degraded`/`acceptsDeploys`.
    pub resources: ResourceStatusDto,
}

/// The numeric host-resource reading, on the operator surface only.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceStatusDto {
    /// The **raw read**: `normal` | `unavailable`.
    pub state: String,
    /// The fallback state machine's word (unit C): `normal` | `throttled` | `critical` |
    /// `unavailable`.
    pub fallback_state: String,
    /// Why the fallback state is what it is, in a stable phrase.
    pub fallback_reason: String,
    /// When the fallback state last changed (ms since the Unix epoch).
    pub fallback_since_ms: u64,
    pub mem_available_mb: u64,
    pub mem_total_mb: u64,
    /// `v1` | `v2` | `unavailable`.
    pub cgroup_version: String,
    #[serde(default)]
    pub cgroup_limit_mb: Option<u64>,
    #[serde(default)]
    pub cgroup_current_mb: Option<u64>,
    pub cgroup_oom_kills: u64,
    /// `(quota, period)` from `cpu.max`/`cpu.cfs_quota_us`; `null` when unquoted or unknown.
    #[serde(default)]
    pub cpu_quota: Option<(u64, u64)>,
    #[serde(default)]
    pub data_dir_free_mb: Option<u64>,
    pub rnode_processes: u64,
    /// When the snapshot was taken (ms since the Unix epoch) and how old it is now.
    pub sampled_at_ms: u64,
    pub sample_age_ms: u64,
    /// Which readers contributed, e.g. `meminfo+cgroup+disk`.
    pub source: String,
}

/// The LFS sync task's state, carried under `sync` on the status response (law 69's other half).
///
/// **One object, not two fields**: the attempt count and the reason are one observation and travel
/// together, so a reader never pairs a count from one attempt with a reason from another.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    /// Attempts the sync task has **started**.
    pub attempts: u32,
    /// **Since process start**, and no other scope is possible: the count is the sync task's own, and
    /// the task restarts with the process — persisting it would claim a continuity the task does not
    /// have. Named rather than left implicit, for the same reason as `poisonRecoveriesScope` (W-N10-02).
    ///
    /// The **scope** half of W-N10-02 deliberately does not apply to this object: LFS sync is
    /// shard-blind (`node_runtime`'s `setup_shard` refuses to sync a multi-shard node for exactly that
    /// reason), so a per-shard reading of it would be an invention.
    pub attempts_scope: String,
    /// The last named failure's message, verbatim. `null` while the attempt now running has not failed.
    pub last_failure: Option<String>,
}

/// The node's capabilities, returned by `GET /api/v1/capabilities` (the app-facing "can I propose /
/// is the faucet available" surface).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeCapabilities {
    /// Continuous block production (`--autopropose`).
    pub autopropose: bool,
    /// Propose immediately after a deploy is accepted (`--propose-on-deploy`).
    pub propose_on_deploy: bool,
    /// Blocks are produced only by an explicit `propose` (neither of the above).
    pub manual_propose: bool,
    /// The admin HTTP surface (`POST /api/v1/propose` on 40405) is published and CORS-enabled.
    pub admin_http: bool,
    /// Dev mode is on (`--dev-mode`).
    pub dev_mode: bool,
    /// The `/api/v1/faucet` endpoint is available (dev mode + a deployer key + remaining budget).
    pub faucet: bool,
    /// Faucet transfer budget still available, in REV drops. Clients can hide a dry faucet.
    pub faucet_remaining: i64,
}

/// A pooled (not-yet-included) deploy (an entry in the `/api/v1/deploys` response).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PooledDeploy {
    /// The deploy id (base16 signature) — identical to what `POST /api/v1/deploy` returns and
    /// `deploy-status` accepts.
    pub deploy_id: String,
    pub timestamp: i64,
    pub deployer: String,
    pub term: String,
    pub phlo_price: i64,
    pub phlo_limit: i64,
    pub valid_after_block_number: i64,
}

/// The `/api/v1/deploys` response: the currently-pooled deploys, most-recent-first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PooledDeploys {
    pub deploys: Vec<PooledDeploy>,
}

/// Exception thrown by the Block API (port of `BlockApiException`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockApiException(pub String);

impl fmt::Display for BlockApiException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for BlockApiException {}

/// A deploy-signature error (port of `SignatureException`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureException(pub String);

impl fmt::Display for SignatureException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for SignatureException {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::rho_expr::{RhoExpr, RhoUnforg};

    #[test]
    fn dto_fields_are_accessible() {
        let deploy = DeployRequest {
            data: DeployData {
                attachments: Vec::new(),
                term: "Nil".to_string(),
                timestamp: 0,
                phlo_price: 1,
                phlo_limit: 1,
                valid_after_block_number: 0,
                shard_id: "root".to_string(),
            },
            deployer: "de".to_string(),
            signature: "si".to_string(),
            sig_algorithm: "secp256k1".to_string(),
        };
        assert_eq!(deploy.sig_algorithm, "secp256k1");

        let data_at_name = DataAtNameRequest {
            name: RhoUnforg::UnforgPrivate("ab".to_string()),
            depth: 1,
        };
        assert_eq!(data_at_name.depth, 1);

        let by_hash = DataAtNameByBlockHashRequest {
            name: RhoExpr::ExprString("x".to_string()),
            block_hash: "h".to_string(),
            use_pre_state_hash: false,
        };
        assert!(!by_hash.use_pre_state_hash);

        let status = PublicStatus {
            version: VersionInfo {
                api: "1.0".to_string(),
                node: "2.0".to_string(),
            },
            address: "addr".to_string(),
            network_id: "testnet".to_string(),
            shard_id: "root".to_string(),
            peers: 1,
            nodes: 2,
            min_phlo_price: 3,
            latest_block_number: 4,
            autopropose: true,
            propose_on_deploy: true,
            manual_propose: false,
            dev_mode: true,
            degraded: "normal".to_string(),
            accepts_deploys: true,
        };
        assert_eq!(status.min_phlo_price, 3);
        assert_eq!(status.latest_block_number, 4);
    }

    #[test]
    fn exceptions_carry_messages() {
        let e = BlockApiException("boom".to_string());
        assert_eq!(e.to_string(), "boom");
        let e = SignatureException("bad sig".to_string());
        assert_eq!(e.to_string(), "bad sig");
    }

    /// Every field of a coordinator record survives the DTO mapping — including the aborted shape,
    /// which is the one an operator reads when a transaction fails.
    #[test]
    fn txn_record_dto_maps_every_field() {
        use rchain_casper::gateway::ledger::{CoordRecord, CoordState, LegRecord, Vote};
        use rchain_crypto::public_key::PublicKey;
        use rchain_shared::refined::{NonNegI64, ShardId};

        let shard = |id: &str| ShardId::try_from(id.to_string()).unwrap();
        let mut record = CoordRecord {
            txn_id: vec![0xAA, 0xBB],
            state: CoordState::Proposed,
            coordinator: PublicKey::new(vec![7u8; 65]),
            legs: vec![LegRecord {
                shard_id: shard("/root"),
                amount: NonNegI64::try_from(30).unwrap(),
                to: "dest".to_string(),
            }],
            votes: Vec::new(),
            reason: None,
        };

        // Proposed: no votes, no reason.
        let dto = TxnRecordDto::from_record(&record);
        assert_eq!(dto.txn_id, "aabb");
        assert_eq!(dto.state, "proposed");
        assert_eq!(dto.coordinator.len(), 130, "65 bytes as hex");
        assert_eq!(dto.legs.len(), 1);
        assert_eq!(dto.legs[0].shard_id, "/root");
        assert_eq!(dto.legs[0].amount, 30);
        assert_eq!(dto.legs[0].to, "dest");
        assert!(dto.votes.is_empty());
        assert!(dto.reason.is_none());
        assert_eq!(dto.record_hash.len(), 64, "the record's content address");

        // Aborted with a reason: the terminal shape a client has to interpret.
        record.record_vote(shard("/root"), Vote::Abort, Some("short".to_string()));
        let dto = TxnRecordDto::from_record(&record);
        assert_eq!(dto.state, "aborted");
        assert_eq!(dto.votes.len(), 1);
        assert_eq!(dto.votes[0].shard_id, "/root");
        assert_eq!(dto.votes[0].vote, "abort");
        assert_eq!(dto.reason.as_deref(), Some("short"));
    }

    /// The API speaks `camelCase` both ways, so a client's request and the response it reads back
    /// use the same names.
    #[test]
    fn txn_dtos_round_trip_through_json() {
        let request: TxnRequest = serde_json::from_str(
            r#"{"txnId":"aabb","legs":[{"shardId":"/root","amount":30,"to":"dest"}]}"#,
        )
        .expect("request parses");
        assert_eq!(request.txn_id, "aabb");
        assert_eq!(request.legs[0].shard_id, "/root");
        assert_eq!(request.legs[0].amount, 30);

        let record = TxnRecordDto {
            txn_id: "aabb".to_string(),
            state: "committed".to_string(),
            coordinator: "00".to_string(),
            record_hash: "11".to_string(),
            legs: request.legs.clone(),
            votes: vec![TxnVoteDto {
                shard_id: "/root".to_string(),
                vote: "ready".to_string(),
            }],
            reason: None,
        };
        let json = serde_json::to_string(&record).expect("serializes");
        assert!(json.contains("\"recordHash\""), "{json}");
        assert!(json.contains("\"txnId\""), "{json}");
        assert!(json.contains("\"shardId\""), "{json}");
        let back: TxnRecordDto = serde_json::from_str(&json).expect("round trip");
        assert_eq!(back, record);
    }
}

/// A deploy execution status (port of the `DeployExecStatus` ADT in `WebApi.scala`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all_fields = "camelCase")]
pub enum DeployExecStatus {
    ProcessedWithSuccess {
        deploy_result: Vec<RhoExpr>,
        block: LightBlockInfo,
    },
    ProcessedWithError {
        deploy_error: String,
        block: LightBlockInfo,
    },
    NotProcessed {
        status: String,
    },
}

/// A rholang expression plus the block it was found in (port of `RhoExprWithBlock`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RhoExprWithBlock {
    pub expr: RhoExpr,
    pub block: LightBlockInfo,
}

/// A data-at-name response (port of `DataAtNameResponse`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataAtNameResponse {
    pub exprs: Vec<RhoExprWithBlock>,
    pub length: i32,
}

/// An exploratory-deploy response (port of `ExploratoryDeployResponse`) — what
/// `POST /api/v1/explore-deploy` returns, and **the type the served document has always named for
/// that path** (`node/src/web/http.rs`): the handler returned `RhoDataResponse` instead, which was
/// invisible while the two were field-for-field identical.
///
/// `reply_source` is the part a client could not get before: **which channel the reply was read
/// from**, per the rule in `spec/API-SCHEMA.md`. Without it an empty `expr` meant both "your term
/// produced nothing" and "your reply is on a channel the node does not read" — a dropped reply with
/// nothing to catch it (AUDIT C38).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExploratoryDeployResponse {
    pub expr: Vec<RhoExpr>,
    pub block: LightBlockInfo,
    /// `firstPrivateName`, `out`, or `none` — the channel that answered, named by the rule.
    pub reply_source: String,
}

/// A rho data response (port of `RhoDataResponse`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RhoDataResponse {
    pub expr: Vec<RhoExpr>,
    pub block: LightBlockInfo,
}

// --- Cross-shard transactions (the multi-shard gateway, Laws 26–29) ---

/// `POST /api/v1/txn` — open (or resume) a cross-shard transaction on this node.
///
/// `txn_id` is caller-supplied rather than generated: a retried request must be the *same*
/// transaction, so the coordinator's idempotency (Law 28 at the API boundary) applies to it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TxnRequest {
    pub txn_id: String,
    pub legs: Vec<TxnLegDto>,
}

/// One leg: which shard escrows, how much REV, and where a commit credits it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TxnLegDto {
    pub shard_id: String,
    pub amount: i64,
    pub to: String,
}

/// A recorded vote, as reported.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TxnVoteDto {
    pub shard_id: String,
    pub vote: String,
}

/// The coordinator's durable record, as reported by `POST /api/v1/txn` and `GET /api/v1/txn/:id`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TxnRecordDto {
    pub txn_id: String,
    /// `proposed` | `prepared` | `committed` | `aborted`.
    pub state: String,
    /// The coordinator key the participants gate `commit`/`abort` on.
    pub coordinator: String,
    /// The record's content address — the same decision on two nodes hashes the same, and nothing
    /// about it is consensus state.
    pub record_hash: String,
    pub legs: Vec<TxnLegDto>,
    pub votes: Vec<TxnVoteDto>,
    /// Why an abort happened (a participant error or a timeout), when one did.
    pub reason: Option<String>,
}

impl TxnRecordDto {
    /// Render a coordinator record for the API.
    pub fn from_record(record: &rchain_casper::gateway::ledger::CoordRecord) -> Self {
        TxnRecordDto {
            txn_id: rchain_shared::base16::encode(&record.txn_id),
            state: record.state.as_str().to_string(),
            coordinator: rchain_shared::base16::encode(record.coordinator.bytes()),
            record_hash: rchain_shared::base16::encode(
                rchain_casper::gateway::ledger::record_hash(record).as_bytes(),
            ),
            legs: record
                .legs
                .iter()
                .map(|leg| TxnLegDto {
                    shard_id: leg.shard_id.to_string(),
                    amount: i64::from(leg.amount),
                    to: leg.to.clone(),
                })
                .collect(),
            votes: record
                .votes
                .iter()
                .map(|(shard_id, vote)| TxnVoteDto {
                    shard_id: shard_id.to_string(),
                    vote: vote.as_str().to_string(),
                })
                .collect(),
            reason: record.reason.clone(),
        }
    }
}
