//! Replay a `MergeTrace` through the pure merge guards.
//!
//! The evaluator (`tools/trace-evaluate.py`) recomputes the predicates from the trace coordinates.
//! This test does the Rust half: the trace's claims, certified base, conflict scope and rejected
//! hosts are fed back through `choose_base_claim` and `finalised_floor`, so a serialised trace is
//! enough to reproduce the decision without a DAG or a devnet.

use rchain_casper::merging::finalised_floor;
use rchain_casper::multi_parent_casper::{
    choose_base_claim, BaseError, BaseSource, Claim, MergeTrace,
};
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_models::block_hash::BlockHash;
use std::collections::BTreeSet;

fn h(byte: u8) -> String {
    format!("{byte:02x}").repeat(32)
}

fn trace_json() -> serde_json::Value {
    serde_json::json!({
        "kind": "merge",
        "parents": [h(0xaa), h(0xbb)],
        "mode": "fringe",
        "merge_fringe": [h(0xaa), h(0xbb)],
        "final_fringe": [h(0xcc)],
        "claims": [
            {"height": 3, "id": h(0xaa), "hash": h(0x11)},
            {"height": 4, "id": h(0xbb), "hash": h(0x11)}
        ],
        "certified_base": null,
        "base_source": "agreed",
        "base_state": h(0x11),
        "pre_state": h(0x11),
        "post_state": h(0x42),
        "conflict_scope": [h(0xdd)],
        "final_scope": [],
        "fork_blocks": [],
        "rejected_deploys": [],
        "rejected_hosts": [],
        "report": {
            "conflict_chains": 1,
            "kept_chains": 1,
            "rejected_chains": 0,
            "fork_rejected_chains": 0,
            "final_scope_rejected_chains": 0
        },
        "predicates": {
            "base_ok": true,
            "fork_ok": true,
            "ledger_ok": true,
            "scope_partition_ok": true
        }
    })
}

fn parse_claim(row: &rchain_casper::multi_parent_casper::TraceClaim) -> Claim {
    Claim {
        height: row.height,
        id: BlockHash::try_from_hex(&row.id).expect("block hash"),
        state: Blake2b256Hash::from_hex_either(&row.hash).expect("state hash"),
    }
}

fn hashes(values: &serde_json::Value) -> BTreeSet<BlockHash> {
    values
        .as_array()
        .expect("array")
        .iter()
        .map(|v| BlockHash::try_from_hex(v.as_str().expect("hash")).expect("hex"))
        .collect()
}

#[test]
fn an_agreeing_trace_replays_to_the_agreed_base() {
    let trace: MergeTrace = serde_json::from_value(trace_json()).expect("trace");
    assert_eq!(trace.kind, "merge");
    assert_eq!(trace.parents.len(), 2);
    let claims = trace
        .claims
        .iter()
        .map(|c| Claim {
            height: c.height,
            id: BlockHash::try_from_hex(&c.id).expect("id"),
            state: Blake2b256Hash::from_hex_either(&c.hash).expect("hash"),
        })
        .collect();
    let base = choose_base_claim(claims, None).expect("agreement has a base");
    match base {
        BaseSource::Agreed { state, claims } => {
            assert_eq!(claims.len(), 2);
            assert_eq!(state.to_hex(), h(0x11));
        }
        BaseSource::Certified { .. } => panic!("an agreed trace must not be certified"),
    }
    assert!(trace.predicates.base_ok);
}

#[test]
fn disagreeing_claims_are_refused_by_the_guard_not_tie_broken() {
    let mut value = trace_json();
    value["claims"][1]["hash"] = serde_json::json!(h(0x22));
    // The node's label deliberately lies; the guard must still refuse.
    value["predicates"]["base_ok"] = serde_json::json!(true);
    let trace: MergeTrace = serde_json::from_value(value).expect("trace");
    let claims: Vec<Claim> = trace.claims.iter().map(parse_claim).collect();
    let error = choose_base_claim(claims, None).expect_err("disagreement has no base");
    assert!(matches!(error, BaseError::Disagree(rows) if rows.len() == 2));
    let hashes: BTreeSet<&String> = trace.claims.iter().map(|c| &c.hash).collect();
    assert!(
        hashes.len() > 1,
        "the replayed coordinate is the disagreement"
    );
}

#[test]
fn a_certified_base_still_wins_over_disagreeing_claims() {
    let mut value = trace_json();
    value["claims"][1]["hash"] = serde_json::json!(h(0x22));
    value["certified_base"] = serde_json::json!(h(0x33));
    let trace: MergeTrace = serde_json::from_value(value).expect("trace");
    let claims: Vec<Claim> = trace.claims.iter().map(parse_claim).collect();
    let certified = Blake2b256Hash::from_hex_either(&h(0x33)).expect("hash");
    let base = choose_base_claim(claims, Some(certified)).expect("certified wins");
    assert!(matches!(base, BaseSource::Certified { .. }));
    assert_eq!(base.state().to_hex(), h(0x33));
}

#[test]
fn a_rejected_host_in_the_conflict_scope_is_refused() {
    let mut value = trace_json();
    value["rejected_hosts"] = serde_json::json!([h(0xdd)]);
    let trace: MergeTrace = serde_json::from_value(value).expect("trace");
    let finalised = hashes(&serde_json::json!(trace.conflict_scope));
    let rejected = hashes(&serde_json::json!(trace.rejected_hosts));
    let error = finalised_floor(&finalised, &rejected).expect_err("overlap is refused");
    assert!(error.contains("finalised deploy cannot be rejected"));
}

#[test]
fn a_rejected_host_outside_the_conflict_scope_is_allowed() {
    let mut value = trace_json();
    value["rejected_hosts"] = serde_json::json!([h(0xee)]);
    let trace: MergeTrace = serde_json::from_value(value).expect("trace");
    let finalised = hashes(&serde_json::json!(trace.conflict_scope));
    let rejected = hashes(&serde_json::json!(trace.rejected_hosts));
    finalised_floor(&finalised, &rejected).expect("disjoint sets are allowed");
}

#[test]
fn a_fringe_merge_with_fork_blocks_is_the_guarded_projection() {
    let mut value = trace_json();
    value["fork_blocks"] = serde_json::json!([h(0xdd)]);
    let trace: MergeTrace = serde_json::from_value(value).expect("trace");
    let fork_ok = trace.mode != "fringe" || trace.fork_blocks.is_empty();
    assert!(
        !fork_ok,
        "the replay recomputes the predicate from the coordinates"
    );
}
