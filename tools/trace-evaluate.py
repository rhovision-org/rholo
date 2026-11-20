#!/usr/bin/env python3
"""Evaluate a claim record's decision traces against the phase-space predicates.

The predicates are recomputed from the trace coordinates (the node's own boolean labels are not
trusted). The exit status is the reading:

    0  every captured trace satisfies its predicates
    1  a predicate is false: the implementation allowed an unsafe point (the printed coordinates
       and the predicate's Rust anchor say where)
    2  every predicate is true and the claim still failed: the point is outside the proven model,
       and the trace is where the model must extend
    3  the failure carries no trace, or a trace lacks the coordinates to decide

    python3 tools/trace-evaluate.py --record target/release-claims/RC-1/local/claim.json
"""
from __future__ import annotations

import argparse
import importlib.util
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location(
    "release_claim_common", ROOT / "tests" / "release_claim_common.py")
common = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(common)

# The predicate's Rust anchor and Lean declaration. Printed with a false predicate so the
# deduction starts at the code that should have prevented the state.
ANCHORS = {
    ("merge", "base_ok"): {
        "rust": "casper/src/multi_parent_casper.rs:choose_base_claim",
        "lean": "Rchain.FinalizedMerge.pickBase_agrees",
    },
    ("merge", "fork_ok"): {
        "rust": "casper/src/merging.rs:MergeScope::from_fringes_guarded",
        "lean": "Rchain.MergeScope.finalising_merge_has_no_forks",
    },
    ("merge", "ledger_ok"): {
        "rust": "casper/src/merging.rs:finalised_floor",
        "lean": "Rchain.FinalizedMerge.a_finalised_deploy_is_not_in_the_ledger",
    },
    ("merge", "scope_partition_ok"): {
        "rust": "casper/src/merging.rs:MergeScope::merge",
        "lean": "Rchain.MergeScope.KeptRule",
    },
    ("finality", "denominator_ok"): {
        "rust": "block-storage/src/dag/liveness.rs:calculate_finalization_reported",
        "lean": "Rchain.finalitySafe",
    },
    ("finality", "support_ok"): {
        "rust": "block-storage/src/dag/finalizer.rs:next_fringe_reported",
        "lean": "Rchain.a_safe_advance_has_a_supermajority",
    },
    ("finality", "partition_ok"): {
        "rust": "block-storage/src/dag/liveness.rs:live_weight_set",
        "lean": "Rchain.finality_safe_partition",
    },
    ("state", "pre_ok"): {
        "rust": "casper/src/interpreter_util.rs:validate_block_checkpoint",
        "lean": "Rchain.Sync.ReplayState.stateSafe",
    },
    ("state", "post_ok"): {
        "rust": "casper/src/interpreter_util.rs:validate_block_checkpoint",
        "lean": "Rchain.Sync.ReplayState.stateSafe",
    },
    ("state", "rejected_ok"): {
        "rust": "casper/src/interpreter_util.rs:validate_block_checkpoint",
        "lean": "Rchain.Sync.ReplayState.stateSafe",
    },
    ("state", "agreement_ok"): {
        "rust": "casper/src/multi_parent_casper.rs:get_pre_state_for_parents",
        "lean": "Rchain.Sync.ReplayState.same_parents_same_state",
    },
    ("boot", "genesis_ok"): {
        "rust": "casper/src/engine/node_launch.rs:boot_outcome",
        "lean": "Rchain.Sync.Anchor.bootSafe",
    },
    ("boot", "acquisition_ok"): {
        "rust": "casper/src/engine/node_launch.rs:acquire_boot_certificate",
        "lean": "Rchain.Sync.Anchor.a_refusal_names_a_missing_coordinate",
    },
    ("boot", "verification_ok"): {
        "rust": "casper/src/engine/node_launch.rs:acquire_boot_certificate",
        "lean": "Rchain.Sync.Anchor.bootSafe",
    },
    ("boot", "anchor_ok"): {
        "rust": "casper/src/engine/node_launch.rs:rebuild_window_for_boot",
        "lean": "Rchain.Sync.Anchor.running_is_anchored",
    },
    ("admission", "bounded_ok"): {
        "rust": "casper/src/pool.rs:classify",
        "lean": "Rchain.Admission.admissionSafe",
    },
    ("admission", "retry_signal_ok"): {
        "rust": "node/src/web/http.rs:in_flight_refused",
        "lean": "Rchain.Admission.a_safe_refusal_is_bounded_and_visible",
    },
    ("admission", "permit_ok"): {
        "rust": "node/src/web/http.rs:in_flight_admission",
        "lean": "Rchain.Admission.admissionSafe",
    },
    ("admission", "pool_bound_ok"): {
        "rust": "casper/src/pool.rs:check_admission",
        "lean": "Rchain.Admission.a_full_pool_refuses",
    },
    ("audit", "anchor_membership_ok"): {
        "rust": "casper/src/checkpoint.rs:checkpoint_digest",
        "lean": "Rchain.Sync.ReplayState.targetShapeSafe",
    },
    ("audit", "target_shape_ok"): {
        "rust": "node/src/runtime/audit.rs:anchored_target",
        "lean": "Rchain.Sync.ReplayState.an_anchored_target_is_the_only_accepted_shape",
    },
    ("audit", "replay_agrees_ok"): {
        "rust": "casper/src/engine/recovery.rs:check_target",
        "lean": "Rchain.Sync.ReplayState.an_anchored_target_is_the_only_accepted_shape",
    },
    ("format", "format_ok"): {
        "rust": "casper/src/storage.rs:decide_store_format",
        "lean": "Rchain.Casper.StoreFormat.a_newer_store_is_never_opened",
    },
    ("format", "stamp_ok"): {
        "rust": "node/src/runtime/node_runtime.rs:write_marker_durably",
        "lean": "Rchain.Casper.StoreFormat.storeSafe",
    },
}

COORDINATE_KEYS = (
    "validator", "file", "site", "mode", "stage", "block", "rule",
    "claims", "conflict_scope", "fork_blocks", "rejected_hosts", "report",
    "partition", "bonded_total", "quorum_total", "supporting", "reason_kind",
    "parents", "claimed_pre", "computed_pre", "claimed_post", "replayed_post",
    "acquisition", "genesis_only", "served_supporting_stake", "served_total_stake",
    "class", "status", "grpc_code", "retry_after", "inflight", "inflight_limit",
    "pool_count", "pool_limit", "pool_bytes", "byte_limit",
    "anchor", "target_hashes", "target_state", "anchor_post_state",
    "verdict", "derived_root", "claimed_root",
    "on_disk", "build", "decision", "stamped", "durable",
)


def evaluate(record: dict) -> tuple[dict, int]:
    traces = common.traces_from(record)
    results = []
    for trace in traces:
        evaluation = common.evaluate_trace(trace)
        results.append({
            "kind": evaluation["kind"],
            "validator": trace.get("validator"),
            "file": trace.get("file"),
            "site": trace.get("site"),
            "predicates": evaluation["predicates"],
            "false": evaluation["false"],
            "missing": evaluation["missing"],
            "coordinates": {key: trace.get(key) for key in COORDINATE_KEYS
                            if key in trace},
            "anchors": [ANCHORS[(evaluation["kind"], name)]
                        for name in evaluation["false"]
                        if (evaluation["kind"], name) in ANCHORS],
        })
    false = [row for row in results if row["false"]]
    missing = [row for row in results if row["missing"]]
    disagreed = common.evaluate_state_agreement(traces)
    verdict = record.get("verdict")
    if not traces:
        reading, code = "evidence-insufficient", 3
    elif false or disagreed:
        reading, code = "unsafe-point", 1
    elif missing:
        reading, code = "evidence-insufficient", 3
    elif verdict == "fail":
        reading, code = "model-incomplete", 2
    else:
        reading, code = "safe", 0
    report = {
        "claim": record.get("claim_id"),
        "verdict": verdict,
        "traces": len(results),
        "reading": reading,
        "state_disagreement": disagreed,
        "results": results,
    }
    if disagreed:
        report["anchors"] = [ANCHORS[("state", "agreement_ok")]]
    return report, code


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--record", required=True,
                        help="a claim.json, or the run directory containing it")
    args = parser.parse_args(argv)
    path = Path(args.record)
    if path.is_dir():
        # A packed evidence record keeps its payload under `raw/`.
        record_path = path / "claim.json"
        if not record_path.exists():
            record_path = path / "raw" / "claim.json"
    else:
        record_path = path
    record = json.loads(record_path.read_text())
    report, code = evaluate(record)
    print(json.dumps(report, indent=2, sort_keys=True))
    return code


if __name__ == "__main__":
    raise SystemExit(main(__import__("sys").argv[1:]))
