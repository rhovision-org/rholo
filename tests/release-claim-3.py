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
_stall_spec = importlib.util.spec_from_file_location(
    "stall_series", ROOT / "tests" / "stall_series.py")
stall = importlib.util.module_from_spec(_stall_spec)
_stall_spec.loader.exec_module(stall)

CLAIM_ID = "RC-3"
TITLE = "State agreement across a merge"
MINIMUM_BLOCKS = 10


def _fixture_baseline(arm: str = "local") -> dict:
    environment = "public" if arm == "public" else "local"
    return {
        "schema": 1, "claim_id": CLAIM_ID, "claim_title": TITLE, "arm": arm,
        "environment": environment,
        "locality": "public-live" if environment == "public" else "local-devnet",
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


def _check_baseline(record: dict) -> tuple[str, list[str]]:
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


STALL_DEPLOY = "cd" * 32
STALL_EXPECT = {
    "deviations": {"DE-09"},
    "representation": "protocol-wait",
    "shape": "historic",
    "cause": "historic-predicate",
    "rung": "R2",
    "control": "never-stalled",
    "deploys": [STALL_DEPLOY],
}


def _fixture_stall_episode() -> dict:
    observations = stall.fixture(
        STALL_DEPLOY,
        episode={"deviations": ["DE-09"], "representation": "protocol-wait",
                 "shape": "historic", "cause": "historic-predicate", "rung": "R2",
                 "law": "Rchain.traceRefines"},
        expected={"progress": True, "finality": False},
        control={"kind": "never-stalled", "stall_absent": True,
                 "episode_count_equal": True})
    observations["stall_counters"] = {
        "baseline": {str(i): {"stalled": False, "episodes": 0, "durable": True}
                     for i in range(4)},
        "pre_trigger": {str(i): {"stalled": False, "episodes": 0, "durable": True}
                        for i in range(4)},
        "stall": {"0": {"stalled": True, "episodes": 1, "durable": True},
                  "1": {"stalled": True, "episodes": 1, "durable": True}},
        "restore": {str(i): {"stalled": False, "episodes": 1, "durable": True}
                    for i in range(4)},
    }
    return {
        "schema": 1, "claim_id": CLAIM_ID, "claim_title": TITLE,
        "arm": "stall-episode", "environment": "local", "locality": "local-devnet",
        "provenance": {"source_sha": "0" * 40, "binary_sha256": "1" * 64},
        "preconditions": [], "verdict": "pass", "reasons": [], "errors": [],
        "harness": {"name": "admission-finality-stall", "exit_code": 0},
        "observations": observations,
    }


def _check_stall_episode(record: dict) -> tuple[str, list[str]]:
    observations = record.get("observations") or {}
    expect = dict(STALL_EXPECT)
    expect["deploys"] = [observations.get("stall_deploy") or STALL_DEPLOY]
    verdict, reasons = stall.verdict(record, expect)
    if verdict != "pass":
        return verdict, reasons
    counters = observations.get("stall_counters")
    counters = counters if isinstance(counters, dict) else {}
    problems: list[str] = []
    stall_rows = counters.get("stall")
    stall_rows = stall_rows if isinstance(stall_rows, dict) else {}
    restore = counters.get("restore")
    restore = restore if isinstance(restore, dict) else {}
    baseline = counters.get("baseline")
    baseline = baseline if isinstance(baseline, dict) else {}
    pre = counters.get("pre_trigger")
    pre = pre if isinstance(pre, dict) else {}
    # The episode is visible either as the live boolean or as a persisted count that moved. DE-09
    # is exactly the case where the boolean has cleared by the time the operator reads; the count
    # is the surface that must survive that.
    entered = []
    for ident, row in stall_rows.items():
        if not isinstance(row, dict):
            continue
        before = (baseline.get(ident) or {}).get("episodes")
        during = row.get("episodes")
        if row.get("stalled") is True or (
                type(before) is int and type(during) is int and during > before):
            entered.append(ident)
    if not entered:
        problems.append("no live validator entered a stall episode during the episode window "
                        "(neither finalityStalled nor finalityStallEpisodes moved)")
    for ident, row in restore.items():
        if not isinstance(row, dict):
            problems.append(f"validator {ident}: restore counter row is not an object")
            continue
        if row.get("stalled") is True:
            problems.append(f"validator {ident}: finalityStalled persists after the restore")
        if row.get("durable") is not True:
            problems.append(f"validator {ident}: finalityCountersDurable is not true")
        if type(row.get("episodes")) is not int or row["episodes"] <= 0:
            problems.append(f"validator {ident}: no stall episode was retained")
        during = (stall_rows.get(ident) or {}).get("episodes")
        if type(during) is int and type(row.get("episodes")) is int \
                and row["episodes"] < during:
            problems.append(
                f"validator {ident}: the retained count fell ({during} -> {row['episodes']})")
    for ident in baseline:
        before = (baseline.get(ident) or {}).get("episodes")
        after = (pre.get(ident) or {}).get("episodes")
        if type(before) is int and type(after) is int and after != before:
            problems.append(
                f"validator {ident}: the stall counter advanced on a live chain "
                f"({before} -> {after})")
    return common.evaluate(record, problems)


FIXTURES = {
    "local": lambda: _fixture_baseline("local"),
    "public": lambda: _fixture_baseline("public"),
    "availability": lambda: common.availability_fixture(CLAIM_ID, TITLE),
    "stall-episode": _fixture_stall_episode,
}
ARMS = {
    "local": _check_baseline,
    "public": _check_baseline,
    "availability": common.check_availability,
    "stall-episode": _check_stall_episode,
}


def fixture(arm: str = "local") -> dict:
    return FIXTURES[arm]()


def check(record: dict) -> tuple[str, list[str]]:
    if record.get("verdict") == "not run":
        return "not run", list(record.get("preconditions") or record.get("reasons") or [])
    return common.dispatch_arm(ARMS, record)


def _load_campaign():
    path = ROOT / "tools" / "stall-campaign.py"
    spec = importlib.util.spec_from_file_location("stall_campaign", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _run_stall(args, out: Path, record: dict) -> int:
    try:
        campaign = _load_campaign()
        campaign.run_denominator(record, out, episode="rc3", keep=args.keep)
    except common.ClaimError as error:
        common.finish(record, "not run", [str(error)])
        return common.write_record(out, record)[1]
    except Exception as error:  # noqa: BLE001 - a failed harness is a measured failure
        common.finish(record, "fail", [f"{type(error).__name__}: {error}"])
        return common.write_record(out, record)[1]
    verdict, reasons = check(record)
    common.finish(record, verdict, reasons)
    return common.write_record(out, record)[1]


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--env", choices=("local", "public"))
    parser.add_argument("--arm")
    parser.add_argument("--out", required=True)
    parser.add_argument("--public-manifest")
    parser.add_argument("--keep", action="store_true")
    args = parser.parse_args(argv)
    arm = args.arm or args.env or "local"
    meta = common.claim_arm(CLAIM_ID, arm)
    environment = meta["environment"]
    record = common.new_record(CLAIM_ID, TITLE, environment, meta["locality"], arm=arm)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    if arm == "stall-episode":
        return _run_stall(args, out, record)
    if arm not in ("local", "public"):
        return common.not_run(
            out, record,
            f"the {arm} arm's harness lands in {meta['next_pr']}")[1]
    failure = common.preflight_failure(CLAIM_ID, environment, out, record,
                                       getattr(args, "public_manifest", None))
    if failure is not None:
        return failure

    if environment == "public":
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
