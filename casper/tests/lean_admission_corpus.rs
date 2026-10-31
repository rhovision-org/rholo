//! **The `admission` corpus, Law 86a/86d, driven through the Rust policy.**
//!
//! `spec/conformance/admission.tsv` is emitted from `spec/Rchain/Admission.lean`: each row is a shape
//! the Lean pool or permit model decides. This consumer runs the same shape through the Rust the node
//! uses — `casper::pool` (the policy `casper/src/dag.rs` calls) and `casper::budget` (the permit
//! arithmetic `node/src/resources/budget.rs` calls) — so a drift between the model and the code fails
//! here rather than in a note.

use rchain_casper::budget::{permits_for, split};
use rchain_casper::pool::{check_admission, check_commit, PoolError, PoolLimits};

fn limits() -> PoolLimits {
    PoolLimits {
        max_deploys: 1,
        max_bytes: 100,
        per_deployer: 10,
    }
}

#[test]
fn the_lean_admission_cases_hold_on_the_rust_policy() {
    let corpus = include_str!("../../spec/conformance/admission.tsv");
    let mut seen = 0usize;
    for line in corpus.lines() {
        let mut fields = line.split('\t');
        assert_eq!(fields.next(), Some("admission"), "layer: {line}");
        let case = fields.next().expect("a case name");
        let expected = fields.next().expect("the Lean verdict");
        assert_eq!(
            expected, "true",
            "{case}: the Lean case says the shape holds"
        );

        match case {
            "free_slot" => assert_eq!(
                check_admission(&limits(), 0, 0, true),
                Ok(()),
                "an empty pool with a free slot admits"
            ),
            "count_bound" => assert_eq!(
                check_admission(&limits(), 1, 0, true),
                Err(PoolError::Full),
                "a full pool refuses by name"
            ),
            "duplicate_bound" => assert_eq!(
                check_commit(&limits(), 0, 0, 0, true),
                Err(PoolError::Duplicate),
                "a duplicate is refused before the count/byte checks"
            ),
            "one_permit" => {
                let total = permits_for(410, 409, 64, 8);
                assert!(total >= 1, "one MiB over the floor is one permit");
                assert!(
                    split(total, 2).exec >= 1,
                    "execution takes the floor of the split"
                );
            }
            "zero_headroom" => assert_eq!(
                permits_for(409, 409, 64, 8),
                0,
                "at the floor there is no execution permit"
            ),
            other => panic!("the Lean corpus emitted an unknown admission case: {other}"),
        }
        seen += 1;
    }
    assert_eq!(
        seen, 5,
        "the layer is pinned at five cases; a corpus that silently shrinks is a check that stopped"
    );
}
