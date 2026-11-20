//! Replay an `AdmissionTrace` through the pool's pure policy.
//!
//! The Python evaluator recomputes the predicates from the trace coordinates. This test does the
//! Rust half for the two pool sites: the recorded counts and class are fed back through
//! `check_admission`/`check_commit`, so a serialised trace reproduces the decision without a node.

use rchain_casper::pool::{check_admission, check_commit, AdmissionTrace, PoolError, PoolLimits};

fn limits() -> PoolLimits {
    PoolLimits {
        max_deploys: 8,
        max_bytes: 1024,
        per_deployer: 2,
    }
}

#[test]
fn a_count_refusal_replays_to_the_same_class() {
    let trace: AdmissionTrace = serde_json::from_value(serde_json::json!({
        "kind": "admission", "site": "pool-admit", "surface": "internal",
        "refused": true, "class": "full",
        "pool_count": 8, "pool_limit": 8, "deployer_held": 0
    }))
    .expect("trace");
    let error = check_admission(
        &limits(),
        trace.pool_count.expect("count"),
        trace.deployer_held.expect("held"),
        true,
    )
    .expect_err("a full pool refuses");
    assert_eq!(error.class_name(), "full");
    assert_eq!(trace.class.as_deref(), Some(error.class_name()));
}

#[test]
fn a_rate_refusal_replays_to_the_same_class() {
    let trace: AdmissionTrace = serde_json::from_value(serde_json::json!({
        "kind": "admission", "site": "pool-admit", "surface": "internal",
        "refused": true, "class": "rate-limited",
        "pool_count": 0, "pool_limit": 8, "deployer_held": 0
    }))
    .expect("trace");
    let error = check_admission(
        &limits(),
        trace.pool_count.expect("count"),
        trace.deployer_held.expect("held"),
        false,
    )
    .expect_err("a denied rate refuses");
    assert_eq!(error.class_name(), "rate-limited");
}

#[test]
fn a_byte_refusal_replays_to_the_same_class() {
    let trace: AdmissionTrace = serde_json::from_value(serde_json::json!({
        "kind": "admission", "site": "pool-commit", "surface": "internal",
        "refused": true, "class": "byte-full",
        "pool_count": 1, "pool_limit": 8, "pool_bytes": 1020, "byte_limit": 1024
    }))
    .expect("trace");
    let error = check_commit(
        &limits(),
        trace.pool_count.expect("count"),
        trace.pool_bytes.expect("bytes"),
        8,
        false,
    )
    .expect_err("a byte overrun refuses");
    assert_eq!(error.class_name(), "byte-full");
}

#[test]
fn a_duplicate_is_refused_but_is_not_a_bound() {
    let domain = PoolError::Duplicate;
    assert!(!domain.is_bounded());
    assert_eq!(domain.class_name(), "duplicate");
}
