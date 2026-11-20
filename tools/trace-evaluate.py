#!/usr/bin/env python3
"""Evaluate a claim record's decision traces against the phase-space predicates.

The predicates are recomputed from the trace coordinates (the node's own boolean labels are not
trusted). The exit status is the reading:

    0  every captured trace satisfies its predicates
    1  a predicate is false: the implementation allowed an unsafe point (the printed coordinates
       and the predicate's Rust anchor say where)
    2  every predicate is true and the claim still failed: the point is outside the proven model,
       and the trace is where the model must extend
    3  the failure carries no trace: the evidence is insufficient to decide

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
    "base_ok": {
        "rust": "casper/src/multi_parent_casper.rs:choose_base_claim",
        "lean": "Rchain.FinalizedMerge.pickBase_agrees",
    },
    "fork_ok": {
        "rust": "casper/src/merging.rs:MergeScope::from_fringes_guarded",
        "lean": "Rchain.MergeScope.finalising_merge_has_no_forks",
    },
    "ledger_ok": {
        "rust": "casper/src/merging.rs:finalised_floor",
        "lean": "Rchain.FinalizedMerge.a_finalised_deploy_is_not_in_the_ledger",
    },
    "scope_partition_ok": {
        "rust": "casper/src/merging.rs:MergeScope::merge",
        "lean": "Rchain.MergeScope.KeptRule",
    },
}


def traces_from(record: dict) -> list[dict]:
    """The merge traces in a record. A boot trace has no `mode` and is not one of these."""
    observations = record.get("observations")
    if isinstance(observations, dict):
        traces = observations.get("traces")
        if isinstance(traces, list):
            return [row for row in traces if isinstance(row, dict) and "mode" in row]
    traces = record.get("traces")
    return ([row for row in traces if isinstance(row, dict) and "mode" in row]
            if isinstance(traces, list) else [])


def evaluate(record: dict) -> dict:
    traces = traces_from(record)
    results = []
    for trace in traces:
        evaluation = common.evaluate_merge_trace(trace)
        results.append({
            "validator": trace.get("validator"),
            "file": trace.get("file"),
            "mode": trace.get("mode"),
            "base_source": trace.get("base_source"),
            "false": evaluation["false"],
            "predicates": evaluation["predicates"],
            "coordinates": {
                "claims": len(trace.get("claims") or []),
                "rejected_hosts": trace.get("rejected_hosts") or [],
                "conflict_scope": trace.get("conflict_scope") or [],
                "fork_blocks": trace.get("fork_blocks") or [],
                "report": trace.get("report") or {},
            },
            "anchors": [ANCHORS[name] for name in evaluation["false"] if name in ANCHORS],
        })
    false = [row for row in results if row["false"]]
    verdict = record.get("verdict")
    if not traces:
        reading = "evidence-insufficient"
        code = 3
    elif false:
        reading = "unsafe-point"
        code = 1
    elif verdict == "fail":
        reading = "model-incomplete"
        code = 2
    else:
        reading = "safe"
        code = 0
    return {
        "claim": record.get("claim_id"),
        "verdict": verdict,
        "traces": len(results),
        "reading": reading,
        "results": results,
    }, code


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--record", required=True,
                        help="a claim.json, or the run directory containing it")
    args = parser.parse_args(argv)
    path = Path(args.record)
    record_path = path / "claim.json" if path.is_dir() else path
    record = json.loads(record_path.read_text())
    report, code = evaluate(record)
    print(json.dumps(report, indent=2, sort_keys=True))
    return code


if __name__ == "__main__":
    raise SystemExit(main(__import__("sys").argv[1:]))
