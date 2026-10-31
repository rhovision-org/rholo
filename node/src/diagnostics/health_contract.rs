//! **The node diagnostics contract** — `/health` v2, its key set, and the readiness vocabulary.
//!
//! Why this module exists. The node's self-diagnostics grew field by field: `pipelineFaults` here,
//! `finalityStallEpisodes` there, and the operator surface learned a different subset. Nothing
//! stated the standard, so a field could be renamed, dropped, or published on one listener and not
//! the other without a test noticing — the class the failure record is full of (a joining node that
//! reports `Running`, an 80%-stake wedge with no field saying so, a stall counter that fired on a
//! healthy chain). This module is the standard's machine side:
//!
//! - [`DIAG_VERSION`] — the schema version, bumped only additively-or-with-a-register-row.
//! - [`PUBLIC_HEALTH_KEYS`] — the **exact** key set of `GET /health`. The route's own test compares
//!   the served JSON against this list, so adding or removing a field without updating the contract
//!   fails the build rather than drifting in the docs.
//! - [`OPERATOR_GATE_KEYS`] — the process gates an operator surface must carry beside the public
//!   ones. They used to exist only on `/health`; an operator reading the admin surface could not see
//!   the outstanding pipeline fault that withheld readiness.
//! - [`ready_reasons`] — a **closed** reason vocabulary for `ready = false`. A boolean is a verdict
//!   with no cause; a probe that can only say "not ready" sends its reader to the logs.
//!
//! What this contract is not. It does not change behaviour and it is not a second copy of the
//! values: the route assembles the values, this module names the set and the vocabulary. The rules
//! the names follow are in `docs/src/spec/node-diagnostics.md` §2; the fields this standard still
//! owes (the gaps the review found) are in §5 and are counted there rather than implied.

/// **The schema version of `GET /health`.** v1 was the unversioned body; v2 adds `diagVersion`,
/// `readyReasons`, `finalizedHeight` and `finalizedLag`. Every change is additive: a v1 reader sees
/// the fields it knew, with their old names and types.
pub const DIAG_VERSION: u32 = 2;

/// **The exact key set of `GET /health`.** Kept sorted so the comparison in the route's test has no
/// ordering to argue about.
pub const PUBLIC_HEALTH_KEYS: &[&str] = &[
    "acceptsDeploys",
    "degraded",
    "diagVersion",
    "finalityStallEpisodes",
    "finalityStalled",
    "finalizedHeight",
    "finalizedHeightError",
    "finalizedLag",
    "height",
    "lastPipelineFault",
    "maxObservedHeight",
    "nodePhase",
    "ok",
    "orphanBodies",
    "orphanDetections",
    "phase",
    "pipelineFaults",
    "pipelineFaultsOutstanding",
    "pool",
    "proposalHaltReason",
    "proposalHalted",
    "ready",
    "readyReasons",
    "shardPhases",
];

/// **The process gates an operator surface must carry.** These are the fields that decide
/// `ready`; publishing them only on the public route made the admin route unable to explain a
/// readiness it could already see from `PublicStatus`. Sorted.
pub const OPERATOR_GATE_KEYS: &[&str] = &[
    "lastPipelineFault",
    "orphanBodies",
    "orphanDetections",
    "pipelineFaults",
    "pipelineFaultsOutstanding",
    "proposalHaltReason",
    "proposalHalted",
];

/// **Why the node is not ready — a closed vocabulary.** The order is the order in the readiness
/// conjunction, so a reader can map the list back to the route's expression. An empty list beside
/// `ready = true` is the only successful reading; an empty list beside `ready = false` is a defect
/// the test below the function forbids.
pub fn ready_reasons(
    all_shards_running: bool,
    pipeline_faults_outstanding: u64,
    proposal_isolated: bool,
    orphan_bodies: u64,
) -> Vec<&'static str> {
    let mut reasons = Vec::new();
    if !all_shards_running {
        reasons.push("shard_not_running");
    }
    if pipeline_faults_outstanding > 0 {
        reasons.push("pipeline_fault_outstanding");
    }
    if proposal_isolated {
        reasons.push("proposal_isolated");
    }
    if orphan_bodies > 0 {
        reasons.push("orphan_body");
    }
    reasons
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_public_key_contract_is_sorted_and_unique() {
        let mut sorted = PUBLIC_HEALTH_KEYS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted, PUBLIC_HEALTH_KEYS,
            "keep the contract sorted and deduplicated"
        );
    }

    #[test]
    fn the_operator_gate_contract_is_sorted_and_unique() {
        let mut sorted = OPERATOR_GATE_KEYS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted, OPERATOR_GATE_KEYS,
            "keep the contract sorted and deduplicated"
        );
    }

    #[test]
    fn every_operator_gate_is_also_a_public_health_key() {
        for key in OPERATOR_GATE_KEYS {
            assert!(
                PUBLIC_HEALTH_KEYS.contains(key),
                "{key} is an operator gate but not a /health field; the two surfaces disagree"
            );
        }
    }

    #[test]
    fn ready_reasons_is_a_closed_vocabulary() {
        assert!(ready_reasons(true, 0, false, 0).is_empty());
        assert_eq!(
            ready_reasons(false, 2, true, 1),
            vec![
                "shard_not_running",
                "pipeline_fault_outstanding",
                "proposal_isolated",
                "orphan_body"
            ]
        );
        // Every reason a false `ready` can carry is one of four codes; there is no fifth.
        let all = ready_reasons(false, 1, true, 1);
        for reason in all {
            assert!(
                [
                    "shard_not_running",
                    "pipeline_fault_outstanding",
                    "proposal_isolated",
                    "orphan_body"
                ]
                .contains(&reason),
                "unregistered readiness reason {reason}"
            );
        }
    }

    #[test]
    fn a_not_ready_node_always_names_a_reason() {
        // The conjunction in `api_health` is exactly these four terms; if `blocked` is true at
        // least one of them is true, so the list cannot be empty beside `ready = false`.
        for all_running in [false, true] {
            for faults in [0u64, 1] {
                for isolated in [false, true] {
                    for orphans in [0u64, 1] {
                        let blocked = !all_running || faults > 0 || isolated || orphans > 0;
                        let reasons = ready_reasons(all_running, faults, isolated, orphans);
                        assert_eq!(blocked, !reasons.is_empty());
                    }
                }
            }
        }
    }
}
