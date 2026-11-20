#!/usr/bin/env python3
"""RC-3 — state agreement across a merge.

Test: `python3 tools/release-check.py run --claim RC-3 --env local`

The test drives a short workload on one devnet and, for every block hash all validators hold,
compares `preStateHash`, `postStateHash` and `rejectedDeploys`. It also reads one marker written
before the window at the same block hash on every validator, so agreement is not only about hashes
the nodes chose to report.
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location(
    "release_claim_common", ROOT / "tests" / "release_claim_common.py")
common = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(common)

CLAIM_ID = "RC-3"
TITLE = "State agreement across a merge"
MINIMUM_BLOCKS = 10


def fixture() -> dict:
    return {
        "schema": 1, "claim_id": CLAIM_ID, "claim_title": TITLE,
        "environment": "local", "locality": "local-devnet",
        "provenance": {"source_sha": "0" * 40, "binary_sha256": "1" * 64},
        "preconditions": [], "verdict": "pass", "reasons": [], "errors": [],
        "harness": {"name": "release-claim-3"},
        "observations": {
            "marker": "release-claim:fixture", "value": 42,
            "blocks": [
                {"hash": f"{i:064x}", "height": i,
                 "preStateHash": "aa" * 32, "postStateHash": "bb" * 32,
                 "rejectedDeploys": [],
                 "validators": [0, 1, 2, 3]}
                for i in range(1, 12)
            ],
            "marker_block": f"{11:064x}",
            "marker_present": [{"validator": i, "present": True} for i in range(4)],
            "state_traces": [{
                "kind": "state", "validator": 0, "block": f"{11:064x}",
                "parents": [f"{10:064x}"],
                "claimed_pre": "aa" * 32, "computed_pre": "aa" * 32,
                "claimed_post": "bb" * 32, "replayed_post": "bb" * 32,
                "claimed_rejected": [], "computed_rejected": [], "verdict": "matched",
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
    blocks = observations.get("blocks")
    if not isinstance(blocks, list):
        problems.append("blocks: missing or not a list")
        blocks = []
    common_blocks = [row for row in blocks
                     if isinstance(row, dict) and set(row.get("validators") or []) >= {0, 1, 2, 3}]
    if len(common_blocks) < MINIMUM_BLOCKS:
        problems.append(f"only {len(common_blocks)} blocks were held by all validators; "
                        f"the floor is {MINIMUM_BLOCKS}")
    for row in common_blocks:
        if not row.get("preStateHash") or not row.get("postStateHash"):
            problems.append(f"block {row.get('height')}: a state hash is missing")
        if not isinstance(row.get("rejectedDeploys"), list):
            problems.append(f"block {row.get('height')}: rejectedDeploys is not a list")
        if row.get("disagreement"):
            problems.append(f"block {row.get('height')}: validators disagree on the state or ledger")
    marker = observations.get("marker_present")
    if not isinstance(marker, list) or len(marker) < 4:
        problems.append("the same-block marker was not read on every validator")
    else:
        for entry in marker:
            if not isinstance(entry, dict) or entry.get("present") is not True:
                problems.append(
                    f"validator {entry.get('validator') if isinstance(entry, dict) else entry}: "
                    "the marker is absent at the common block")
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
        target = common.public_target(manifest)
        record["provenance"] = manifest.get("provenance") or {}
        return _observe(target, record, out)

    devnet = common.Devnet("releaserc3", validators=4, http_base=43403,
                           admin_base=43405, grpc_base=43402)
    try:
        devnet.up()
        record["provenance"] = devnet.provenance()
        return _observe(devnet.target(), record, out, devnet)
    except Exception as error:  # noqa: BLE001
        common.finish(record, "fail", [f"{type(error).__name__}: {error}"])
        return common.write_record(out, record)[1]
    finally:
        if not args.keep:
            devnet.down()


def _observe(target, record: dict, out: Path, devnet=None) -> int:
    observations = record["observations"]
    marker = "release-claim:" + str(time.time_ns())
    value = common.MARKER_VALUE
    try:
        if devnet is not None:
            deploy_id = devnet.deploy(f'@{json.dumps(marker)}!({value})')
        else:
            deploy_id = None
        observations.update({"marker": marker, "value": value, "deploy_id": deploy_id})

        def visible():
            heights = []
            for i in range(len(target.nodes)):
                blocks = target.blocks(i, 1)
                if not blocks:
                    return None
                height, block_hash = common.block_identity(blocks[0])
                if not common.marker_present(target.probe(i, marker, block_hash), value):
                    return None
                heights.append(height)
            return max(heights)

        height = common.wait_until("the marker is visible on every validator", visible, timeout=240)
        common.wait_until(
            "the finalised head is two heights above the marker",
            lambda: all(target.finalized_head(i)[0] >= height + 2
                        for i in range(len(target.nodes))), timeout=240)

        def inclusion():
            settlement = target.settlement(0, deploy_id) if deploy_id else {}
            if settlement.get("stage") == "finalizedSuccess" and settlement.get("blockHash"):
                return settlement["blockHash"]
            return None

        inclusion_hash = common.wait_until(
            "the settlement names the marker's inclusion block", inclusion, timeout=300)
        common.wait_until(
            "the inclusion block is finalised on every validator",
            lambda: all(target.is_finalized(i, inclusion_hash)
                        for i in range(len(target.nodes))), timeout=300)

        per_block: dict[str, dict] = {}
        for i in range(len(target.nodes)):
            for row in target.blocks(i, 48):
                if not isinstance(row, dict):
                    continue
                block_hash = row.get("blockHash")
                if not isinstance(block_hash, str):
                    continue
                entry = per_block.setdefault(block_hash, {
                    "hash": block_hash, "height": row.get("blockNumber"),
                    "preStateHash": row.get("preStateHash"),
                    "postStateHash": row.get("postStateHash"),
                    "rejectedDeploys": row.get("rejectedDeploys"),
                    "validators": [],
                })
                triple = (row.get("preStateHash"), row.get("postStateHash"),
                          tuple(row.get("rejectedDeploys") or []))
                if (entry["preStateHash"], entry["postStateHash"],
                        tuple(entry["rejectedDeploys"] or [])) != triple:
                    entry["disagreement"] = True
                entry["validators"].append(i)
        common_hashes = [h for h, row in per_block.items() if len(set(row["validators"])) ==
                         len(target.nodes)]
        common_hashes.sort(key=lambda h: per_block[h]["height"] or 0)
        observations["blocks"] = [per_block[h] for h in common_hashes]
        observations["marker_block"] = inclusion_hash
        observations["marker_present"] = [
            {"validator": i,
             "present": common.marker_present(
                 target.probe(i, marker, inclusion_hash), value)}
            for i in range(len(target.nodes))
        ]
        if devnet is not None:
            common_set = set(common_hashes)
            traces = []
            for i in range(len(target.nodes)):
                dest = out / "traces" / f"validator-{i}"
                devnet.copy_traces(i, dest)
                for path in sorted(dest.glob("state-*.json")):
                    try:
                        row = json.loads(path.read_text())
                    except ValueError:
                        continue
                    if row.get("block") in common_set:
                        traces.append({"validator": i, "file": path.name, **row})
            observations["state_traces"] = traces
    except Exception as error:  # noqa: BLE001
        common.finish(record, "fail", [f"{type(error).__name__}: {error}"])
        return common.write_record(out, record)[1]
    verdict, reasons = check(record)
    common.finish(record, verdict, reasons)
    return common.write_record(out, record)[1]


if __name__ == "__main__":
    raise SystemExit(run(sys.argv[1:]))
