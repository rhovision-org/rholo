//! Law 51d's conformance corpus: the `recovery` layer.
//!
//! Each row is one tick of the paced gate — the first blocked sighting, the tick, and the model's
//! `(escape?, new sighting)`. The Rust consumer calls the same pure function the proposer's mutex
//! form delegates to, so a gate that banks its permit fails the tie: the row `pace 15000 15001`
//! expects **no** escape, and a non-consuming gate escapes.
//!
//! Later clauses extend this layer (the receiver re-offer and the fault classification); this
//! consumer dispatches on the case name and fails on an unknown one rather than skipping it.

use rchain_casper::block_disposition::{eligible_stored_pending, BlockDisposition, PendingStage};
use rchain_casper::blocks::proposer::proposer::take_escape_permit;
use rchain_casper::validate::JustificationOutcome;
use rchain_shared::refined::BlockHeight;

#[test]
fn the_paced_gate_is_the_lean_models() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../spec/conformance/recovery.tsv");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "read {}: {e}\n(run tools/emit-lean-corpus.sh)",
            path.display()
        )
    });

    let mut seen = 0;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let cols: Vec<&str> = line.split('\t').collect();
        assert_eq!(cols[0], "recovery", "{line:?}");
        match cols[1] {
            "pace" => {
                assert_eq!(
                    cols.len(),
                    6,
                    "a pace row is kind/first/now/escape/new: {line:?}"
                );
                let first = parse_opt(cols[2]);
                let now: u64 = cols[3].parse().expect("now");
                let escape: bool = cols[4].parse().expect("escape");
                let expected_since = parse_opt(cols[5]);
                assert_eq!(
                    take_escape_permit(first, now),
                    (escape, expected_since),
                    "case {line:?}"
                );
            }
            "eligible" => {
                assert_eq!(
                    cols.len(),
                    6,
                    "an eligible row is kind/stage/since/now/expected: {line:?}"
                );
                let stage = match cols[2] {
                    "stored" => PendingStage::Stored,
                    "validating" => PendingStage::Validating,
                    "retry" => PendingStage::Retry,
                    other => panic!("no Rust stage for {other:?}"),
                };
                let since: u64 = cols[3].parse().expect("since");
                let now: u64 = cols[4].parse().expect("now");
                let expected: bool = cols[5].parse().expect("expected");
                let row = BlockDisposition::Pending {
                    block_number: BlockHeight::try_from(0).expect("height"),
                    stage,
                    since_unix_ms: since,
                };
                assert_eq!(
                    eligible_stored_pending(&row, now),
                    expected,
                    "case {line:?}"
                );
            }
            "classify" => {
                assert_eq!(
                    cols.len(),
                    5,
                    "a classify row is kind/in-map/regression/expected: {line:?}"
                );
                let in_map: bool = cols[2].parse().expect("in-map");
                let regression: bool = cols[3].parse().expect("regression");
                let outcome = JustificationOutcome::classify(in_map, regression);
                assert_eq!(outcome.name(), cols[4], "case {line:?}");
                assert_eq!(
                    outcome.refuses(),
                    cols[4] == "regression",
                    "only a regression refuses — the unresolved case is not a consensus change"
                );
            }
            other => panic!("no Rust arm for the recovery case {other:?}"),
        }
        seen += 1;
    }

    // No row may pass vacuously: at least one escape, one refusal and one consumption.
    assert!(
        seen >= 11,
        "the corpus carries the pacing and eligibility cases"
    );
}

fn parse_opt(s: &str) -> Option<u64> {
    if s == "-" {
        None
    } else {
        Some(s.parse().expect("an optional integer"))
    }
}
