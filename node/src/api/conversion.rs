//! Web API protobuf conversion functions (port of the conversion fns in `api/WebApi.scala`).

use rchain_casper::api::block_api::{
    Capabilities, FinalityHealth, ProposerHealth, ShardHealthEntry, SyncHealth,
};
use rchain_casper::runtime_manager::CapturedReply;
use rchain_crypto::public_key::PublicKey;
use rchain_crypto::signatures::signatures_alg::from_algorithm;
use rchain_crypto::signatures::signed::Signed;
use rchain_models::ast::Par;
use rchain_models::casper::protocol::casper_message::{DeployData, SignedDeployData};
use rchain_models::casper::protocol::deploy_service::{
    DataWithBlockInfo, DeployExecStatus as CasperDeployExecStatus, LightBlockInfo, Status,
};
use rchain_shared::base16;

use super::dto::{
    DataAtNameResponse, DeployExecStatus as ApiDeployExecStatus, DeployRequest,
    ExploratoryDeployResponse, FinalityHealthDto, HaltStatusDto, NodeCapabilities, OperatorStatus,
    PooledDeploy, ProposerHealthDto, PublicStatus, ResourceStatusDto, RhoDataResponse,
    RhoExprWithBlock, ShardHealthDto, SignatureException, SyncStatus, VersionInfo,
};
use super::rho_expr::{expr_from_par, RhoExpr};
use crate::resources::ResourceSnapshot;

/// **The scope of a counter that is the process's own running total** (W-N10-02). A string on the wire
/// rather than a boolean, because it names *what* the number counts — a reader comparing it with the
/// shard-history counters beside it does not have to guess why one restarts at zero and the other does
/// not.
pub const SINCE_PROCESS_START: &str = "SinceProcessStart";

/// Map one casper [`ShardHealthEntry`] to its DTO (W-N10-04).
fn to_shard_health_dto(entry: &ShardHealthEntry) -> ShardHealthDto {
    ShardHealthDto {
        proposer: ProposerHealthDto {
            consecutive_self_validation_failures: entry
                .proposer
                .consecutive_self_validation_failures,
            autopropose_timer_halted: entry.proposer.autopropose_timer_halted,
            autopropose_timer_halt_episodes: entry.proposer.autopropose_halt.episodes,
            autopropose_halt: HaltStatusDto {
                at: entry.proposer.autopropose_halt.at,
                durable: entry.proposer.autopropose_halt.durable,
            },
            autopropose_tap_suppressed: entry.proposer.autopropose_tap_suppressed,
            stale_snapshot_self_equivocations: entry.proposer.stale_snapshot_self_equivocations,
        },
        finality: FinalityHealthDto {
            stall_reason: entry.finality.stall_reason.clone(),
            stall_sustained: entry.finality.stall_sustained,
            stall_episodes: entry.finality.stall_episodes,
            non_quiet_merge_reports: entry.finality.non_quiet_merge_reports,
            unrestorable_records: entry.finality.unrestorable_records,
            unrestorable_block: entry.finality.unrestorable_block.clone(),
            counters_durable: entry.finality.counters_durable,
        },
    }
}

/// **The public half, and it takes no operator data — which is the point of the split** (W-N3-01).
///
/// There is no `health`, `finality`, `reset_marker` or reporter argument here. A field added to one of
/// those cannot appear on the public port because this function could not read it: the leak that a
/// single struct with two serializers invites is a *compile error* rather than a review miss.
pub fn to_public_status(
    status: &Status,
    caps: &Capabilities,
    resource: &ResourceSnapshot,
) -> PublicStatus {
    PublicStatus {
        version: VersionInfo {
            api: status.version.api.clone(),
            node: status.version.node.clone(),
        },
        address: status.address.clone(),
        network_id: status.network_id.clone(),
        shard_id: status.shard_id.clone(),
        peers: status.peers,
        nodes: status.nodes,
        min_phlo_price: status.min_phlo_price,
        latest_block_number: status.latest_block_number,
        autopropose: caps.autopropose,
        propose_on_deploy: caps.propose_on_deploy,
        manual_propose: caps.manual_propose,
        dev_mode: caps.dev_mode,
        degraded: resource.fallback.as_str().to_string(),
        // The contract, now backed by the state machine: only `normal` accepts new work.
        accepts_deploys: !resource.fallback.refuses_ingress(),
    }
}

/// The operator half, on top of the public core the caller already built.
#[allow(clippy::too_many_arguments)]
pub fn to_operator_status(
    core: PublicStatus,
    caps: &Capabilities,
    health: &ProposerHealth,
    finality: &FinalityHealth,
    sync: Option<&SyncHealth>,
    // F-U12-02: the reset marker's own reading, threaded in like `sync` rather than fetched here, so a
    // caller that has no store still maps a status deterministically.
    reset_marker: Option<String>,
    poison_recoveries: u64,
    tasks_panicked: u64,
    tasks_exited: u64,
    undelivered_sends: u64,
    // **Every member shard's health** (W-N10-04), gathered by the caller from `health_by_shard`. Threaded
    // like `sync` rather than fetched here, so a caller that has no members maps a status deterministically.
    shards: Option<std::collections::BTreeMap<String, ShardHealthEntry>>,
    resource: &ResourceSnapshot,
) -> OperatorStatus {
    OperatorStatus {
        core,
        admin_http: caps.admin_http,
        consecutive_self_validation_failures: health.consecutive_self_validation_failures,
        autopropose_timer_halted: health.autopropose_timer_halted,
        // The durable count, which is what the key always claimed to be (W-N10-01): it was an
        // in-memory count when this line was written, and a restart reset it.
        autopropose_timer_halt_episodes: health.autopropose_halt.episodes,
        autopropose_halt: HaltStatusDto {
            at: health.autopropose_halt.at,
            durable: health.autopropose_halt.durable,
        },
        autopropose_tap_suppressed: health.autopropose_tap_suppressed,
        stale_snapshot_self_equivocations: health.stale_snapshot_self_equivocations,
        finality_stall: finality.stall_reason.clone(),
        finality_stalled: finality.stall_sustained,
        finality_stall_episodes: finality.stall_episodes,
        finality_counters_durable: finality.counters_durable,
        reset_marker,
        non_quiet_merge_reports: finality.non_quiet_merge_reports,
        // Law 69's other half: the sync task's own state, threaded in by the caller exactly as
        // `finality` is. **`None` travels as `null` rather than as a zeroed object** — a reader must be
        // able to tell "this node is not syncing" from "this node is syncing and its first attempt has
        // not failed yet", and only the absence of the object says the first.
        sync: sync.map(|s| SyncStatus {
            attempts: s.attempts,
            attempts_scope: SINCE_PROCESS_START.to_string(),
            last_failure: s.last_failure.clone(),
        }),
        poison_recoveries,
        poison_recoveries_scope: SINCE_PROCESS_START.to_string(),
        unrestorable_records: finality.unrestorable_records,
        unrestorable_block: finality.unrestorable_block.clone(),
        tasks_panicked,
        tasks_panicked_scope: SINCE_PROCESS_START.to_string(),
        tasks_exited,
        tasks_exited_scope: SINCE_PROCESS_START.to_string(),
        undelivered_sends,
        undelivered_sends_scope: SINCE_PROCESS_START.to_string(),
        shards: shards.map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), to_shard_health_dto(v)))
                .collect()
        }),
        resources: ResourceStatusDto {
            state: resource.state.to_string(),
            fallback_state: resource.fallback.as_str().to_string(),
            fallback_reason: resource.fallback_reason.to_string(),
            fallback_since_ms: resource.fallback_since_ms,
            mem_available_mb: resource.mem_available_mb,
            mem_total_mb: resource.mem_total_mb,
            cgroup_version: resource.cgroup_version.to_string(),
            cgroup_limit_mb: resource.cgroup_limit_mb,
            cgroup_current_mb: resource.cgroup_current_mb,
            cgroup_oom_kills: resource.cgroup_oom_kills,
            cpu_quota: resource.cpu_quota,
            data_dir_free_mb: resource.data_dir_free_mb,
            rnode_processes: u64::try_from(resource.rnode_processes).unwrap_or(u64::MAX),
            sampled_at_ms: resource.sampled_at_ms,
            sample_age_ms: now_ms().saturating_sub(resource.sampled_at_ms),
            source: resource.source.clone(),
        },
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Map a casper `Capabilities` + the faucet availability flag to the app-facing `NodeCapabilities`.
pub fn to_node_capabilities(
    caps: &Capabilities,
    faucet: bool,
    faucet_remaining: i64,
) -> NodeCapabilities {
    NodeCapabilities {
        autopropose: caps.autopropose,
        propose_on_deploy: caps.propose_on_deploy,
        manual_propose: caps.manual_propose,
        admin_http: caps.admin_http,
        dev_mode: caps.dev_mode,
        faucet,
        faucet_remaining,
    }
}

/// Map a pooled `SignedDeployData` to a `PooledDeploy` (an entry in the `/api/v1/deploys` response).
pub fn to_pooled_deploy(signed: &SignedDeployData) -> PooledDeploy {
    PooledDeploy {
        deploy_id: base16::encode(&signed.sig),
        timestamp: signed.data.timestamp,
        deployer: base16::encode(&signed.deployer),
        term: signed.data.term.clone(),
        phlo_price: signed.data.phlo_price,
        phlo_limit: signed.data.phlo_limit,
        valid_after_block_number: signed.data.valid_after_block_number,
    }
}

/// Map a casper `DeployExecStatus` to the API `DeployExecStatus` (port of `toDeployExecStatus`).
pub fn to_deploy_exec_status(status: &CasperDeployExecStatus) -> Option<ApiDeployExecStatus> {
    match status {
        CasperDeployExecStatus::ProcessedWithSuccess {
            deploy_result,
            block,
        } => {
            let result: Vec<RhoExpr> = deploy_result.iter().filter_map(expr_from_par).collect();
            Some(ApiDeployExecStatus::ProcessedWithSuccess {
                deploy_result: result,
                block: block.clone(),
            })
        }
        CasperDeployExecStatus::ProcessedWithError {
            deploy_error,
            block,
        } => Some(ApiDeployExecStatus::ProcessedWithError {
            deploy_error: deploy_error.clone(),
            block: block.clone(),
        }),
        CasperDeployExecStatus::NotProcessed { status } => {
            Some(ApiDeployExecStatus::NotProcessed {
                status: status.clone(),
            })
        }
    }
}

/// Map post-block `Par`s plus a block to a `RhoDataResponse` (port of `toRhoDataResponse`).
pub fn to_rho_data_response(pars: &[Par], block: &LightBlockInfo) -> RhoDataResponse {
    RhoDataResponse {
        expr: pars.iter().filter_map(expr_from_par).collect(),
        block: block.clone(),
    }
}

/// Map an exploratory deploy's reply — its data *and* the channel it was read from — to an
/// `ExploratoryDeployResponse`. `to_rho_data_response` is for the endpoints where the *client* names
/// the channel (`/data-at-name-by-block-hash`), so no source is needed there.
pub fn to_exploratory_deploy_response(
    reply: &CapturedReply,
    block: &LightBlockInfo,
) -> ExploratoryDeployResponse {
    ExploratoryDeployResponse {
        expr: reply.data.iter().filter_map(expr_from_par).collect(),
        block: block.clone(),
        reply_source: reply.source.as_str().to_string(),
    }
}

/// Map post-block data plus a length to a `DataAtNameResponse` (port of `toDataAtNameResponse`).
pub fn to_data_at_name_response(dbs: &[DataWithBlockInfo], length: i32) -> DataAtNameResponse {
    let mut exprs_with_block = Vec::new();
    for data in dbs {
        let exprs: Vec<RhoExpr> = data
            .post_block_data
            .iter()
            .filter_map(expr_from_par)
            .collect();
        // Implements the semantic of Par with Unit: P | Nil ==> P.
        let expr = if let [single] = exprs.as_slice() {
            single.clone()
        } else {
            RhoExpr::ExprPar(exprs)
        };
        exprs_with_block.insert(
            0,
            RhoExprWithBlock {
                expr,
                block: data.block.clone(),
            },
        );
    }
    DataAtNameResponse {
        exprs: exprs_with_block,
        length,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::ResourceState;
    use rchain_casper::api::block_api::HaltStatus;
    use rchain_models::ast::Expr;
    use rchain_models::casper::protocol::deploy_service::{
        BondInfo, Status, VersionInfo as CasperVersionInfo,
    };

    /// **The public contract follows the fallback state** (unit C, B38): `degraded` is the state
    /// machine's word, and `acceptsDeploys` is true only in `normal`. A throttled or critical node must
    /// not advertise that it will take work.
    #[test]
    fn accepts_deploys_is_false_under_pressure() {
        let status = Status {
            version: CasperVersionInfo {
                api: "1".to_string(),
                node: "n".to_string(),
            },
            address: "a".to_string(),
            network_id: "root".to_string(),
            shard_id: "root".to_string(),
            peers: 0,
            nodes: 0,
            min_phlo_price: 0,
            latest_block_number: 0,
        };
        let caps = Capabilities {
            autopropose: false,
            propose_on_deploy: false,
            manual_propose: false,
            admin_http: false,
            dev_mode: false,
        };
        for (state, word, accepts) in [
            (ResourceState::Normal, "normal", true),
            (ResourceState::Throttled, "throttled", false),
            (ResourceState::Critical, "critical", false),
            (ResourceState::Unavailable, "unavailable", false),
        ] {
            let snapshot = ResourceSnapshot {
                fallback: state,
                ..ResourceSnapshot::default()
            };
            let public = to_public_status(&status, &caps, &snapshot);
            assert_eq!(public.degraded, word);
            assert_eq!(public.accepts_deploys, accepts, "{word}");
        }
    }

    fn light_block_info() -> LightBlockInfo {
        LightBlockInfo {
            version: 1,
            shard_id: "root".to_string(),
            block_hash: "h".to_string(),
            block_number: 1,
            sender: "s".to_string(),
            seq_num: 1,
            pre_state_hash: "pre".to_string(),
            post_state_hash: "post".to_string(),
            justifications: vec![],
            bonds: vec![BondInfo {
                validator: "v".to_string(),
                stake: 100,
            }],
            sig_algorithm: "secp256k1".to_string(),
            sig: "sig".to_string(),
            block_size: "0".to_string(),
            deploy_count: 0,
            rejected_deploys: vec![],
            timestamp: 0,
        }
    }

    fn par_int(n: i64) -> Par {
        Par {
            exprs: vec![Expr::GInt(n)],
            ..Default::default()
        }
    }

    #[test]
    fn to_api_status_maps_fields() {
        let status = Status {
            version: CasperVersionInfo {
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
        };
        let caps = Capabilities {
            autopropose: true,
            propose_on_deploy: true,
            manual_propose: false,
            admin_http: true,
            dev_mode: true,
        };
        let health = ProposerHealth {
            consecutive_self_validation_failures: 3,
            autopropose_timer_halted: true,
            autopropose_halt: HaltStatus {
                at: Some(1_700_000_000_000),
                episodes: 1,
                durable: true,
            },
            autopropose_tap_suppressed: 4,
            stale_snapshot_self_equivocations: 5,
        };
        let finality = FinalityHealth {
            stall_reason: Some(
                "a layer exists but its supporting stake is not a supermajority".to_string(),
            ),
            stall_sustained: true,
            stall_episodes: 7,
            non_quiet_merge_reports: 2,
            unrestorable_records: 1,
            unrestorable_block: Some("ab12 (seq 9)".to_string()),
            counters_durable: true,
        };
        // The sync reading is a parameter like `finality`, and the `None` arm is the ordinary one:
        // this process's sync task has returned.
        // **Two constructors, and which one a field lives on is the subject of the split** (W-N3-01):
        // `to_public_status` cannot see the health, the finality counters or the reporter counts, so a
        // field moved into the operator half by mistake is a compile error rather than a leak.
        let core = to_public_status(&status, &caps, &ResourceSnapshot::default());
        let api = to_operator_status(
            core.clone(),
            &caps,
            &health,
            &finality,
            None,
            None,
            11,
            2,
            40,
            7,
            None,
            &ResourceSnapshot::default(),
        );
        assert_eq!(core.version.api, "1.0");
        assert_eq!(core.address, "addr");
        assert_eq!(core.min_phlo_price, 3);
        assert!(core.autopropose);
        assert!(!core.manual_propose);
        // The operator half is a **superset** — `#[serde(flatten)]` on the wire and the same `core` in
        // the struct — so a client can move between the two routes without a shape change.
        assert_eq!(api.core, core);
        // …and `adminHttp` is the one field that *moved* rather than being added: it tells an
        // unauthenticated caller that the actuator port is published.
        assert!(api.admin_http);
        // #157: the health is carried through, and it is the third source rather than a capability —
        // the configured mode above says nothing about whether the proposer is failing.
        assert_eq!(api.consecutive_self_validation_failures, 3);
        assert!(api.autopropose_timer_halted);
        // W-N10-01: the **episode count** beside the sticky flag, counted on the transition. It is the
        // durable count now — seeded from the store at boot — so unlike the flag it does not start at
        // zero on a node that restarted after halting.
        assert_eq!(api.autopropose_timer_halt_episodes, 1);
        // …and the halt on record: *when* it happened and whether the record is trustworthy. `at` is a
        // different fact from the flag — `null` means no halt on record, which a restarted node with a
        // lost record reads as `durable: false` rather than as "never halted".
        assert_eq!(api.autopropose_halt.at, Some(1_700_000_000_000));
        assert!(api.autopropose_halt.durable);
        // W-N4-08: and what the halt cost, counted where it was refused. Without the tap's count the
        // halt was a flag the tap ignored.
        assert_eq!(api.autopropose_tap_suppressed, 4);
        // #156: the stale-snapshot count is carried too, distinct from the halt.
        assert_eq!(api.stale_snapshot_self_equivocations, 5);
        // C249's F-U10-01: the *reason* finality is not advancing reaches the surface rather than only
        // the log. It is a `String` and not a flag because the diagnosis is in the numbers it carries
        // ("400 of 800", "4 full partitions among 4 candidates") — which is why the log line was worth
        // having and why a probe could not ask for it.
        assert_eq!(
            api.finality_stall.as_deref(),
            Some("a layer exists but its supporting stake is not a supermajority")
        );
        // …with the sustained flag and the episode count beside it, so "stalled now" is
        // distinguishable from "recovered" — and, since #318, from the ordinary per-round refusal a
        // healthy producing chain reports through the reason above.
        assert!(api.finality_stalled);
        assert_eq!(api.finality_stall_episodes, 7);
        // #280's ledger, counted: the number the live incident would have moved, and now a fifth source
        // rather than the third — the merge's own report, not the proposer's health.
        assert_eq!(api.non_quiet_merge_reports, 2);
        // F-U9-03: a poisoned-lock recovery is reported rather than absorbed.
        assert_eq!(api.poison_recoveries, 11);
        // C249's R3 gap: a node that has given up on a block says so, and says *which* one. Nothing
        // acts on it yet — the surface exists so the reset that later does act on it is reviewable
        // against a real denominator.
        assert_eq!(api.unrestorable_records, 1);
        assert_eq!(api.unrestorable_block.as_deref(), Some("ab12 (seq 9)"));
        // C254: the supervisor's two counters, carried through as their own source. A panic and a
        // return are different facts — the return is the one nothing else reports.
        assert_eq!(api.tasks_panicked, 2);
        assert_eq!(api.tasks_exited, 40);
        // C254's other residual: the closed-channel door's count reaches the surface. The door has
        // counted every undelivered send since it landed and nothing published the count, so this is
        // the increment its own module doc named — the assertion is that the parameter travels.
        assert_eq!(api.undelivered_sends, 7);
        // Law 69's other half: a `None` reading travels as a null `sync`, which is what an operator
        // sees on a node that is not syncing — the ordinary reading, and the one that must not be
        // confused with "synced fine". The `Some` shape needs a running sync task, and its decision is
        // pinned in `node_syncing`'s own test where the statics live.
        assert_eq!(api.sync, None);
        // W-N10-02: the durable flag travels with the shard-history counters, so a node whose store
        // could not be reached says its counts are this process's rather than the shard's.
        assert!(api.finality_counters_durable);
        // …and the four counters that are the process's own are **labelled** rather than left to read
        // as though they shared the counters' lifetime.
        assert_eq!(api.poison_recoveries_scope, "SinceProcessStart");
        assert_eq!(api.tasks_panicked_scope, "SinceProcessStart");
        assert_eq!(api.tasks_exited_scope, "SinceProcessStart");
        assert_eq!(api.undelivered_sends_scope, "SinceProcessStart");
        // W-N10-04: the per-shard map travels, and `None` here is the caller's choice — the `Some`
        // shape is pinned by `admin_web_api_impl`'s own test, where the router supplies the members.
        assert_eq!(api.shards, None);
    }

    /// **The sync reading names its scope** (W-N10-02).
    ///
    /// The `Some` arm is the one an operator sees on a node latched in `NodeSyncing`, and it is the
    /// arm the durable-counter question does *not* apply to: the count is the sync task's own and the
    /// task restarts with the process. This pins that the label travels, so a later change cannot drop
    /// it silently — the counter beside three shard-history counters with no label would read as though
    /// it shared their lifetime.
    #[test]
    fn the_sync_reading_labels_its_scope() {
        let core = PublicStatus {
            version: VersionInfo {
                api: "1.0".into(),
                node: "n".into(),
            },
            address: "addr".into(),
            network_id: "testnet".into(),
            shard_id: "/root".into(),
            peers: 0,
            nodes: 0,
            min_phlo_price: 0,
            latest_block_number: 0,
            autopropose: false,
            propose_on_deploy: false,
            manual_propose: true,
            dev_mode: false,
            degraded: "normal".to_string(),
            accepts_deploys: true,
        };
        let sync = SyncHealth {
            attempts: 4,
            last_failure: Some("no peers answered".to_string()),
        };
        let api = to_operator_status(
            core,
            &Capabilities {
                autopropose: false,
                propose_on_deploy: false,
                manual_propose: true,
                admin_http: false,
                dev_mode: false,
            },
            &ProposerHealth::default(),
            &FinalityHealth::default(),
            Some(&sync),
            None,
            0,
            0,
            0,
            0,
            None,
            &ResourceSnapshot::default(),
        );
        let s = api.sync.expect("the reading travels");
        assert_eq!(s.attempts, 4);
        assert_eq!(s.last_failure.as_deref(), Some("no peers answered"));
        assert_eq!(
            s.attempts_scope, "SinceProcessStart",
            "the count is the sync task's own, and the task restarts with the process"
        );
    }

    /// **The shard's history reaches the wire under the names the row uses** (W-N10-02).
    ///
    /// The row states its defect by wire name — `finalityStallEpisodes`, `nonQuietMergeReports` — because
    /// that is what an operator reads and what the row's evidence quotes. Making the counters durable
    /// changes nothing anyone can see unless the counting value *arrives*, so this pins the last link in
    /// a chain the other falsifiers pin in pieces: `casper/tests/reset.rs` pins store → cell, and the
    /// live arm in `node/tests/gateway.rs` pins cell → surface on a running node. This one reads the
    /// **serialized** form, because a *key name* is not a property of the struct: a dropped field, a
    /// rename, or a lost `rename_all` shows up here and nowhere else.
    ///
    /// Falsified by moving any of the three keys or the durability flag: the assertions below are the
    /// operator's reading, and a counter no surface names is not a report.
    #[test]
    fn the_shard_history_counts_reach_the_wire_under_the_rows_names() {
        let core = PublicStatus {
            version: VersionInfo {
                api: "1.0".into(),
                node: "n".into(),
            },
            address: "addr".into(),
            network_id: "testnet".into(),
            shard_id: "/root".into(),
            peers: 0,
            nodes: 0,
            min_phlo_price: 0,
            latest_block_number: 0,
            autopropose: true,
            propose_on_deploy: false,
            manual_propose: false,
            dev_mode: false,
            degraded: "normal".to_string(),
            accepts_deploys: true,
        };
        // The shard's history as a restarted node reads it back: non-zero, and the store answered.
        let finality = FinalityHealth {
            stall_episodes: 9,
            non_quiet_merge_reports: 4,
            unrestorable_records: 1,
            counters_durable: true,
            ..FinalityHealth::default()
        };
        let api = to_operator_status(
            core,
            &Capabilities {
                autopropose: true,
                propose_on_deploy: false,
                manual_propose: false,
                admin_http: false,
                dev_mode: false,
            },
            &ProposerHealth::default(),
            &finality,
            None,
            None,
            0,
            0,
            0,
            0,
            None,
            &ResourceSnapshot::default(),
        );
        let json = serde_json::to_value(&api).expect("the operator status serializes");
        for (key, want) in [
            ("finalityStallEpisodes", 9u64),
            ("nonQuietMergeReports", 4),
            ("unrestorableRecords", 1),
        ] {
            assert_eq!(
                json[key].as_u64(),
                Some(want),
                "the row's own name for this count, carrying the shard's history: {key} in {json}"
            );
        }
        assert_eq!(
            json["finalityCountersDurable"].as_bool(),
            Some(true),
            "and the flag that says the counts are the shard's travels beside them: {json}"
        );
    }

    /// **The per-shard map carries a non-primary shard's halt** (W-N10-04).
    ///
    /// The row: `ShardRoutingBlockApi::proposer_health` reads `primary_api()` alone, so a non-primary
    /// shard whose timer has halted is invisible — the top-level `autoproposeTimerHalted` is the
    /// primary's, and on a healthy primary it says `false` while another member is stopped. This drives
    /// the conversion with two members, one halted, and asserts the map says which.
    #[test]
    fn the_per_shard_map_carries_each_members_halt() {
        use std::collections::BTreeMap;
        let core = PublicStatus {
            version: VersionInfo {
                api: "1.0".into(),
                node: "n".into(),
            },
            address: "addr".into(),
            network_id: "testnet".into(),
            shard_id: "/root".into(),
            peers: 0,
            nodes: 0,
            min_phlo_price: 0,
            latest_block_number: 0,
            autopropose: true,
            propose_on_deploy: false,
            manual_propose: false,
            dev_mode: false,
            degraded: "normal".to_string(),
            accepts_deploys: true,
        };
        let caps = Capabilities {
            autopropose: true,
            propose_on_deploy: false,
            manual_propose: false,
            admin_http: true,
            dev_mode: false,
        };
        let halted = ProposerHealth {
            autopropose_timer_halted: true,
            autopropose_halt: HaltStatus {
                at: Some(1_700_000_000_000),
                episodes: 2,
                durable: true,
            },
            ..ProposerHealth::default()
        };
        let shards = BTreeMap::from([
            (
                "/root".to_string(),
                ShardHealthEntry {
                    proposer: ProposerHealth::default(),
                    finality: FinalityHealth::default(),
                },
            ),
            (
                "/root/child".to_string(),
                ShardHealthEntry {
                    proposer: halted,
                    finality: FinalityHealth {
                        stall_episodes: 9,
                        counters_durable: true,
                        ..FinalityHealth::default()
                    },
                },
            ),
        ]);
        let api = to_operator_status(
            core,
            &caps,
            &ProposerHealth::default(),
            &FinalityHealth::default(),
            None,
            None,
            0,
            0,
            0,
            0,
            Some(shards),
            &ResourceSnapshot::default(),
        );
        // The flat field is the **primary's**, and the primary is healthy.
        assert!(
            !api.autopropose_timer_halted,
            "the flat field is the primary's reading, which is the row's whole point"
        );
        let map = api.shards.expect("the map travels");
        assert_eq!(map.len(), 2, "**total** over the node's members");
        assert!(!map["/root"].proposer.autopropose_timer_halted);
        assert!(
            map["/root/child"].proposer.autopropose_timer_halted,
            "the non-primary's halt is visible here and nowhere else"
        );
        assert_eq!(
            map["/root/child"].proposer.autopropose_halt.at,
            Some(1_700_000_000_000)
        );
        assert_eq!(map["/root/child"].finality.stall_episodes, 9);
    }

    /// **The public status carries no operator key** (W-N3-01), asserted on the *serialized* form
    /// rather than on the struct: a `#[serde(flatten)]` mistake, a rename, or a field added to
    /// `PublicStatus` that belongs on the other half would all show up as a key here and nowhere else
    /// in the type system.
    ///
    /// The keys are the ones the row names. Falsified by moving any of them back: this is the test
    /// that fails, and the compiler is what fails first for the two the constructors separate.
    #[test]
    fn the_public_status_serializes_no_operator_key() {
        let status = Status {
            version: CasperVersionInfo {
                api: "1".to_string(),
                node: "2".to_string(),
            },
            address: "addr".to_string(),
            network_id: "testnet".to_string(),
            shard_id: "root".to_string(),
            peers: 0,
            nodes: 0,
            min_phlo_price: 0,
            latest_block_number: 0,
        };
        let caps = Capabilities {
            autopropose: false,
            propose_on_deploy: false,
            manual_propose: false,
            admin_http: true,
            dev_mode: false,
        };
        let json = serde_json::to_value(to_public_status(
            &status,
            &caps,
            &ResourceSnapshot::default(),
        ))
        .expect("serializes");
        let keys: Vec<&str> = json
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        for forbidden in [
            "adminHttp",
            "resetMarker",
            "unrestorableBlock",
            "unrestorableRecords",
            "autoproposeTimerHalted",
            "autoproposeTimerHaltEpisodes",
            "autoproposeHalt",
            "autoproposeTapSuppressed",
            "finalityCountersDurable",
            "poisonRecoveriesScope",
            "tasksPanickedScope",
            "tasksExitedScope",
            "undeliveredSendsScope",
            "shards",
            "consecutiveSelfValidationFailures",
            "staleSnapshotSelfEquivocations",
            "finalityStall",
            "finalityStalled",
            "finalityStallEpisodes",
            "nonQuietMergeReports",
            "sync",
            "poisonRecoveries",
            "tasksPanicked",
            "tasksExited",
            "undeliveredSends",
        ] {
            assert!(
                !keys.contains(&forbidden),
                "the public status must not carry operator state: {forbidden} is in {keys:?}"
            );
        }
        // And the public fields *are* there, so the assertion above is not passing on an empty object.
        for expected in ["version", "address", "networkId", "shardId", "devMode"] {
            assert!(
                keys.contains(&expected),
                "the public status must still carry {expected}: {keys:?}"
            );
        }
    }

    #[test]
    fn to_deploy_exec_status_maps_oneof() {
        let not_processed = to_deploy_exec_status(&CasperDeployExecStatus::NotProcessed {
            status: "pending".to_string(),
        })
        .unwrap();
        assert_eq!(
            not_processed,
            ApiDeployExecStatus::NotProcessed {
                status: "pending".to_string()
            }
        );

        let err = to_deploy_exec_status(&CasperDeployExecStatus::ProcessedWithError {
            deploy_error: "boom".to_string(),
            block: light_block_info(),
        })
        .unwrap();
        assert!(matches!(
            err,
            ApiDeployExecStatus::ProcessedWithError { .. }
        ));
    }

    #[test]
    fn to_rho_data_response_maps_pars() {
        let resp = to_rho_data_response(&[par_int(42)], &light_block_info());
        assert_eq!(resp.expr, vec![RhoExpr::ExprInt(42)]);
        assert_eq!(resp.block.block_hash, "h");
    }

    #[test]
    fn to_data_at_name_response_maps_data() {
        let db = DataWithBlockInfo {
            post_block_data: vec![par_int(1)],
            block: light_block_info(),
        };
        let resp = to_data_at_name_response(&[db], 5);
        assert_eq!(resp.length, 5);
        assert_eq!(resp.exprs.len(), 1);
        assert_eq!(resp.exprs[0].expr, RhoExpr::ExprInt(1));
    }
}

/// Build a signed deploy from a deploy request (port of `toSignedDeploy`).
pub fn to_signed_deploy(sd: &DeployRequest) -> Result<Signed<DeployData>, SignatureException> {
    let pk_bytes = base16::decode(&sd.deployer)
        .ok_or_else(|| SignatureException("Public key is not valid base16 format.".to_string()))?;
    let sig_bytes = base16::decode(&sd.signature)
        .ok_or_else(|| SignatureException("Signature is not valid base16 format.".to_string()))?;
    let pk = PublicKey::new(pk_bytes);
    let sig_alg = from_algorithm(&sd.sig_algorithm)
        .ok_or_else(|| SignatureException("Signature algorithm not supported.".to_string()))?;
    Signed::from_signed_data(sd.data.clone(), pk, sig_bytes, sig_alg)
        .ok_or_else(|| SignatureException("Invalid signature.".to_string()))
}

#[cfg(test)]
mod to_signed_deploy_tests {
    use super::*;

    fn deploy_request(deployer: &str, sig_algorithm: &str) -> DeployRequest {
        DeployRequest {
            data: DeployData {
                attachments: Vec::new(),
                term: "Nil".to_string(),
                timestamp: 0,
                phlo_price: 1,
                phlo_limit: 1,
                valid_after_block_number: 0,
                shard_id: "root".to_string(),
            },
            deployer: deployer.to_string(),
            signature: "00".to_string(),
            sig_algorithm: sig_algorithm.to_string(),
        }
    }

    #[test]
    fn rejects_invalid_deployer_hex() {
        let err = to_signed_deploy(&deploy_request("zz", "secp256k1"))
            .err()
            .unwrap();
        assert_eq!(err.to_string(), "Public key is not valid base16 format.");
    }

    #[test]
    fn rejects_unsupported_algorithm() {
        let err = to_signed_deploy(&deploy_request("00", "unknown"))
            .err()
            .unwrap();
        assert_eq!(err.to_string(), "Signature algorithm not supported.");
    }
}
