#!/usr/bin/env python3
"""RC-2 — safety: no conflicting finalised histories across a producing partition.

Test: `python3 tools/release-check.py run --claim RC-2 --env local`

The test runs the two-cycle partition/rejoin harness with `--producing-victim` (the victim keeps
proposing while isolated), then asks the only finality oracle: for every finalised block one
validator holds, is it finalised by every validator whose finalised height is at or above it?
Three consecutive polls must agree, and no finalised height may regress.
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location(
    "release_claim_common", ROOT / "tests" / "release_claim_common.py")
common = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(common)

CLAIM_ID = "RC-2"
TITLE = "Safety"
PASS_TOKEN = "PASS_BOUNDED_WORKLOAD_PARTITION_REJOIN"


def fixture() -> dict:
    return {
        "schema": 1, "claim_id": CLAIM_ID, "claim_title": TITLE,
        "environment": "local", "locality": "local-devnet",
        "provenance": {"source_sha": "0" * 40, "binary_sha256": "1" * 64},
        "preconditions": [], "verdict": "pass", "reasons": [], "errors": [],
        "harness": {"name": "consensus-partition-rejoin", "exit_code": 0},
        "observations": {
            "harness_result": PASS_TOKEN,
            "cycles": [
                {"cycle": 1, "victim_index": 3, "producing_victim": True,
                 "anchor_retention": [{"node": i, "finalized_anchor": True} for i in range(4)]},
                {"cycle": 2, "victim_index": 1, "producing_victim": True,
                 "anchor_retention": [{"node": i, "finalized_anchor": True} for i in range(4)]},
            ],
            "containment": [
                {"validator": v, "height": 40, "hash": "ab" * 32,
                 "answers": [{"validator": w, "finalized": True} for w in range(4)]}
                for v in range(4)
            ],
            "regression": [],
            "finality_traces": [{
                "kind": "finality", "validator": 0,
                "bonded_total": 400, "live_total": 400, "quorum_total": 400,
                "supporting": 300, "full_partitions": 3, "candidates": 3,
                "supported_tip": 40,
                "partition": ["aa", "bb", "cc", "dd"],
                "bonded_senders": ["aa", "bb", "cc", "dd"],
                "prev_fringe": [], "new_fringe": ["aa"], "advanced": True,
                "reason_kind": None,
            }],
        },
    }


def check(record: dict) -> tuple[str, list[str]]:
    if record.get("verdict") == "not run":
        return "not run", list(record.get("preconditions") or record.get("reasons") or [])
    problems: list[str] = []
    observations = record.get("observations")
    if not isinstance(observations, dict):
        return "fail", ["observations: missing or not an object"]
    if observations.get("harness_result") != PASS_TOKEN:
        problems.append(f"the partition harness did not pass: {observations.get('harness_result')!r}")
    cycles = observations.get("cycles")
    if not isinstance(cycles, list) or len(cycles) < 2:
        problems.append("the harness did not record two partition cycles")
        cycles = []
    if len({row.get("victim_index") for row in cycles if isinstance(row, dict)}) < 2:
        problems.append("the two cycles did not rotate the victim")
    for row in cycles:
        if not isinstance(row, dict):
            problems.append("cycle row is not an object")
            continue
        if row.get("producing_victim") is not True:
            problems.append(f"cycle {row.get('cycle')}: the victim was not producing")
        retention = row.get("anchor_retention")
        if not isinstance(retention, list) or not retention:
            problems.append(f"cycle {row.get('cycle')}: no anchor-retention rows")
            continue
        for entry in retention:
            if not isinstance(entry, dict) or entry.get("finalized_anchor") is not True:
                problems.append(
                    f"cycle {row.get('cycle')}: node {entry.get('node') if isinstance(entry, dict) else entry} "
                    "did not retain the anchor")
    containment = observations.get("containment")
    if not isinstance(containment, list) or not containment:
        problems.append("containment: missing or empty")
        containment = []
    for row in containment:
        if not isinstance(row, dict):
            problems.append("containment row is not an object")
            continue
        answers = row.get("answers")
        if not isinstance(answers, list) or not answers:
            problems.append(f"validator {row.get('validator')}: no containment answers")
            continue
        required = {entry.get("validator") for entry in answers if isinstance(entry, dict)}
        if len(required) < 4:
            problems.append(f"validator {row.get('validator')}: containment read on "
                            f"{len(required)} validators, expected 4")
        for entry in answers:
            if isinstance(entry, dict) and entry.get("finalized") is not True:
                problems.append(
                    f"validator {entry.get('validator')} does not finalise "
                    f"{row.get('hash')} held finalised by {row.get('validator')}")
    if observations.get("regression"):
        problems.append(f"finalised regression observed: {observations['regression']}")
    problems.extend(common.phase_space_reading(observations, bool(problems)))
    return common.evaluate(record, problems)


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--env", choices=("local", "public"), required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--public-manifest")
    parser.add_argument("--keep", action="store_true")
    args = parser.parse_args(argv)
    record = common.new_record(CLAIM_ID, TITLE, args.env,
                               "local-devnet" if args.env == "local" else "public-live")
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    failure = common.preflight_failure(CLAIM_ID, args.env, out, record,
                                       getattr(args, "public_manifest", None))
    if failure is not None:
        return failure

    if args.env == "public":
        manifest = common.load_manifest(args.public_manifest)
        if not manifest:
            return common.not_run(out, record, "no --public-manifest for the public fleet")[1]
        if not manifest.get("fault", {}).get("kind"):
            return common.not_run(
                out, record,
                "the public run needs the operator's declared partition window and victim")[1]
        target = common.public_target(manifest)
        record["provenance"] = manifest.get("provenance") or {}
        record["observations"]["harness_result"] = manifest.get("harness_result")
        record["observations"]["cycles"] = manifest.get("cycles") or []
        if manifest.get("harness_result") != PASS_TOKEN:
            return common.not_run(
                out, record,
                "the public manifest does not carry a completed partition/rejoin result")[1]
        return _containment(target, record, out)

    devnet = common.Devnet("acceptance", validators=4, network="acceptance", grpc_base=45402,
                           extra_up=("--epoch-length", "10"))
    try:
        devnet.up()
        common.wait_for_advance(devnet.target())
        harness_out = out / "partition"
        command = [sys.executable, str(ROOT / "tests" / "consensus-partition-rejoin.py"),
                   "--validators", "4", "--epoch-length", "10", "--out", str(harness_out),
                   "--local", "--producing-victim"]
        result = subprocess.run(command, cwd=ROOT, env=devnet.env(), text=True,
                                capture_output=True, timeout=3600)
        (out / "partition.log").write_text(result.stdout + result.stderr)
        record["harness"] = {"name": "consensus-partition-rejoin", "command": command,
                             "exit_code": result.returncode, "out": str(harness_out)}
        record["provenance"] = devnet.provenance()
        typed = _read_json(harness_out / "partition-rejoin.json") or {}
        result_doc = _read_json(harness_out / "partition-result.json") or {}
        if result.returncode and not typed.get("cycles"):
            # The harness's first phase is the shared acceptance drill. On this build that drill
            # stops at the RC-1 retention defect, before a partition is installed. That is a
            # precondition of this test, not evidence about safety; say so instead of calling the
            # missing partition a safety failure.
            reason = ("the shared acceptance drill did not reach the partition phase: "
                      + (result.stdout + result.stderr)[-1200:])
            record["observations"]["harness_error"] = reason
            record["preconditions"].append(
                "RC-1's settlement-retention precondition failed; no partition was installed")
            common.finish(record, "not run", [reason])
            return common.write_record(out, record)[1]
        record["observations"].update({
            "harness_result": result_doc.get("result") or (
                PASS_TOKEN if result.returncode == 0 else "FAIL"),
            "cycles": typed.get("cycles") or [],
            "harness_error": None if result.returncode == 0 else
            (result.stdout + result.stderr)[-1500:],
        })
        return _containment(devnet.target(), record, out, devnet)
    except Exception as error:  # noqa: BLE001
        common.finish(record, "fail", [f"{type(error).__name__}: {error}"])
        return common.write_record(out, record)[1]
    finally:
        if not args.keep:
            devnet.down()


def _read_json(path: Path):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return None


def _containment(target, record: dict, out: Path, devnet=None) -> int:
    observations = record["observations"]
    try:
        first = [target.finalized_head(i) for i in range(len(target.nodes))]
        time.sleep(2)
        second = [target.finalized_head(i) for i in range(len(target.nodes))]
        regression = [
            {"validator": i, "before": first[i], "after": second[i]}
            for i in range(len(target.nodes))
            if second[i][0] < first[i][0]
            or (second[i][0] == first[i][0] and second[i][1] != first[i][1])
        ]
        heads = [(i, max(first[i], second[i], key=lambda pair: pair[0])) for i in range(len(target.nodes))]
        containment = []
        for v, (height, block_hash) in heads:
            answers = []
            for w, (w_height, _) in heads:
                if w_height < height:
                    continue
                seen = []
                for _ in range(3):
                    try:
                        seen.append(target.is_finalized(w, block_hash))
                    except Exception as error:  # noqa: BLE001
                        seen.append(f"{type(error).__name__}: {error}")
                    time.sleep(1)
                answers.append({"validator": w, "finalized": all(x is True for x in seen),
                                "samples": seen})
            containment.append({"validator": v, "height": height, "hash": block_hash,
                                "answers": answers})
        observations["containment"] = containment
        observations["regression"] = regression
        if devnet is not None:
            traces = []
            for i in range(len(target.nodes)):
                dest = out / "traces" / f"validator-{i}"
                devnet.copy_traces(i, dest)
                for path in sorted(dest.glob("finality-*.json")):
                    try:
                        row = json.loads(path.read_text())
                    except ValueError:
                        continue
                    traces.append({"validator": i, "file": path.name, **row})
            observations["finality_traces"] = traces
    except Exception as error:  # noqa: BLE001
        common.finish(record, "fail", [f"{type(error).__name__}: {error}"])
        return common.write_record(out, record)[1]
    verdict, reasons = check(record)
    common.finish(record, verdict, reasons)
    return common.write_record(out, record)[1]


if __name__ == "__main__":
    raise SystemExit(run(sys.argv[1:]))
