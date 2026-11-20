//! Replay a recorded store-format trace through the pure decision.
//!
//! The trace is the node's `FormatTrace` shape (written under `RNODE_TRACE_DIR`). This test reads
//! the coordinates back and asserts `decide_store_format` reproduces the decision, so a serialised
//! trace is enough to reproduce the gate without opening a store.

use rchain_casper::storage::{decide_store_format, StoreFormatDecision};

#[test]
fn every_recorded_format_trace_replays_to_its_decision() {
    let traces: Vec<serde_json::Value> = serde_json::from_str(
        r#"[
          {"kind":"format","present":null,"build":4,"decision":"stamp"},
          {"kind":"format","present":"4","build":4,"decision":"accept"},
          {"kind":"format","present":"3","build":4,"decision":"advance"},
          {"kind":"format","present":"5","build":4,"decision":"refuse_newer"},
          {"kind":"format","present":"four","build":4,"decision":"refuse_unreadable"}
        ]"#,
    )
    .expect("traces");
    for trace in traces {
        let present = trace["present"].as_str();
        let build = trace["build"].as_u64().expect("build") as u32;
        let decision = decide_store_format(present, build);
        assert_eq!(
            decision.as_str(),
            trace["decision"].as_str().expect("decision"),
            "trace {trace}"
        );
    }
}

#[test]
fn a_newer_store_is_never_opened() {
    let decision = decide_store_format(Some("5"), 4);
    assert!(decision.refused());
    assert_eq!(decision, StoreFormatDecision::RefuseNewer { on_disk: 5 });
}

#[test]
fn an_unreadable_marker_is_never_guessed() {
    let decision = decide_store_format(Some("  "), 4);
    assert!(decision.refused());
    assert!(matches!(
        decision,
        StoreFormatDecision::RefuseUnreadable { .. }
    ));
}

#[test]
fn a_stamp_is_written_only_for_an_absent_or_older_marker() {
    assert!(decide_store_format(None, 4).needs_stamp());
    assert!(decide_store_format(Some("2"), 4).needs_stamp());
    assert!(!decide_store_format(Some("4"), 4).needs_stamp());
    assert!(!decide_store_format(Some("5"), 4).needs_stamp());
}
