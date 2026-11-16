//! Law 104's conformance corpus: the `checkpoint` layer.
//!
//! Each row is a `(supporting, total, expected)` triple for the checkpoint quorum rule. The Rust
//! consumer calls `rchain_sdk::consensus::is_super_majority` — the exact function
//! `verify_checkpoint_certificate` uses — so a rule that admits exactly two thirds fails the boundary
//! rows rather than drifting silently.

#[test]
fn the_quorum_rule_is_the_lean_models() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../spec/conformance/checkpoint.tsv");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "read {}: {e}\n(run tools/emit-lean-corpus.sh)",
            path.display()
        )
    });

    let mut seen = 0;
    let mut admitted = 0;
    let mut refused = 0;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let cols: Vec<&str> = line.split('\t').collect();
        assert_eq!(
            cols.len(),
            4,
            "a checkpoint row is kind/supporting/total/expected: {line:?}"
        );
        assert_eq!(cols[0], "checkpoint", "{line:?}");
        let supporting: i128 = cols[1].parse().expect("supporting stake");
        let total: i128 = cols[2].parse().expect("total stake");
        let expected: bool = cols[3].parse().expect("expected");
        assert_eq!(
            rchain_sdk::consensus::is_super_majority(supporting, total),
            expected,
            "case {line:?}"
        );
        if expected {
            admitted += 1;
        } else {
            refused += 1;
        }
        seen += 1;
    }

    assert_eq!(
        seen, 17,
        "the corpus carried 17 cases and no unknown row was skipped"
    );
    assert!(
        admitted > 0 && refused > 0,
        "the corpus must exercise both verdicts"
    );
}
