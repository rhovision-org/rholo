#!/usr/bin/env python3
"""RC-1 diagnostic: which merge path dropped a finalised write?

The release claim RC-1 reproduces a finalised marker that is absent at a later finalised head and
named in that block's `rejectedDeploys`. The record says *what* happened; this tool collects the
discriminants that say *which* path did it, without changing node behaviour:

* the drill's marker events (finalised present, then absent);
* every validator's node log, parsed for `[fringe-divergence]`, the `merge:` report
  (`rejected as forks`, `rode on a final-scope rejected chain`), `state-hash disagreement` and
  `replay failed`;
* the losing block's `rejectedDeploys`, including a boundary `blockHash ++ 02` CloseBlock id;
* each validator's finalised head and the losing block's header.

The output is `diagnosis.json` with one branch: R1 stale/missing base claim, R2 fork
reclassification, R3 rejection-map attribution, R4/R5 replay/view divergence, R6 boundary
CloseBlock cascade, or undetermined. The classification is pure and unit-tested offline; the live
arm only collects.

    python3 tools/rc1-merge-retention-diagnose.py --validators 4 --window 240 \
        --out spec/audit/evidence/rc1-merge-retention-20261120
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import re
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location(
    "release_claim_common", ROOT / "tests" / "release_claim_common.py")
common = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(common)

CLOSE_ID = re.compile(r"^[0-9a-f]{64}02$")


def classify(lines: list[str], rejected: list[str]) -> dict:
    """Rank the branch from the log signals and the losing block's rejection set.

    Precedence follows the only paths that can drop a tuple-space write: a divergent base claim
    (R1) or a finalised block reclassified as a fork (R2). R6 explains a boundary CloseBlock id
    riding beside the user id. R3 alone explains persistence, not loss, so it ranks below R2.
    """
    text = "\n".join(lines)
    signals = {
        "fringe_divergence": "[fringe-divergence]" in text,
        "fork_rejected": bool(re.search(r"\brejected as forks\b", text)),
        "final_scope_rejected": "rode on a final-scope rejected chain" in text,
        "state_hash_disagreement": "state-hash disagreement" in text,
        "replay_failed": "replay failed" in text,
        "close_block_id": any(CLOSE_ID.match(identifier) for identifier in rejected),
        "marker_rejected": bool(rejected),
    }
    if signals["fringe_divergence"]:
        branch = "R1"
        why = "a finaliser's fringe_state_hash claims disagree; the chosen base may predate the write"
    elif signals["fork_rejected"]:
        branch = "R2"
        why = "the merge rejected a chain as a non-descendant fork; a finalised block was reclassified"
    elif signals["close_block_id"]:
        branch = "R6"
        why = "the losing block's rejection set carries a boundary CloseBlock id beside the user id"
    elif signals["final_scope_rejected"]:
        branch = "R3"
        why = "the final-scope rejection ledger names the deploy; attribution, not yet the loss"
    elif signals["state_hash_disagreement"] or signals["replay_failed"]:
        branch = "R4/R5"
        why = "a replay or view disagreement is logged; the base or accepted set differs by node"
    else:
        branch = "undetermined"
        why = "none of the discriminants is present in the captured logs"
    return {"branch": branch, "why": why, "signals": signals}


def select_loss(events: dict) -> tuple[dict, dict]:
    """The first absent read after the write, and the first one that names a rejection.

    The drill's absent-marker control precedes the write (height 0/1), so it cannot be the loss.
    The state can be gone one block before the block that records the rejection; both are kept.
    """
    present_heights = [row["height"] for row in events["present"]
                       if isinstance(row.get("height"), int)]
    floor = max(present_heights) if present_heights else 0
    post = [row for row in events["absent"]
            if isinstance(row.get("height"), int) and row["height"] >= floor]
    first = post[0] if post else (events["absent"][-1] if events["absent"] else {})
    attribution = next((row for row in post if row.get("rejectedDeploys")), first)
    return first, attribution


def marker_events(events_path: Path) -> dict:
    """The drill's finalised-marker reads, split into present and absent."""
    present, absent = [], []
    try:
        lines = events_path.read_text().splitlines()
    except OSError:
        return {"present": present, "absent": absent}
    for line in lines:
        try:
            event = json.loads(line)
        except ValueError:
            continue
        response = event.get("response")
        block = (response or {}).get("block") or {}
        row = {
            "node": event.get("node"),
            "height": block.get("blockNumber"),
            "hash": event.get("finalized_marker_at") or block.get("blockHash"),
            "rejectedDeploys": block.get("rejectedDeploys") or [],
        }
        expr = (response or {}).get("expr")
        if expr == [{"ExprInt": {"data": 42}}]:
            present.append(row)
        elif expr == []:
            absent.append(row)
    return {"present": present, "absent": absent}


def collect(args) -> int:
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    devnet = common.Devnet("acceptance", validators=args.validators, network="acceptance",
                           grpc_base=45402,
                           extra_up=("--no-autopropose", "--epoch-length", "10"))
    try:
        devnet.up()
        drill_out = out / "drill"
        command = [sys.executable, str(ROOT / "tools" / "consensus-acceptance-drill.py"),
                   "--validators", str(args.validators), "--window", str(args.window),
                   "--out", str(drill_out), "--local"]
        result = subprocess.run(command, cwd=ROOT, env=devnet.env(), text=True,
                                capture_output=True, timeout=args.window + 600)
        (out / "drill.log").write_text(result.stdout + result.stderr)
        events = marker_events(drill_out / "observations.jsonl")
        loss, attribution = select_loss(events)
        logs = []
        for i in range(args.validators):
            log = subprocess.run(["docker", "logs", devnet.node(i)],
                                 text=True, capture_output=True, timeout=120)
            path = out / f"node-{i}.log"
            path.write_text(log.stdout + log.stderr)
            logs.extend((log.stdout + log.stderr).splitlines())
        diagnosis = classify(logs, attribution.get("rejectedDeploys") or [])
        diagnosis.update({
            "generated": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            "drill_exit": result.returncode,
            "markers": events,
            "first_absent": loss,
            "attribution": attribution,
            "evidence": {
                "fork_lines": [line for line in logs if "rejected as forks" in line][-3:],
                "final_scope_lines": [line for line in logs
                                      if "rode on a final-scope rejected chain" in line][-3:],
            },
            "provenance": devnet.provenance(),
            "boundary": "finite local witness; no universal cause theorem",
        })
        (out / "diagnosis.json").write_text(json.dumps(diagnosis, indent=2, sort_keys=True) + "\n")
        print(json.dumps({"branch": diagnosis["branch"], "why": diagnosis["why"],
                          "first_absent": loss, "attribution": attribution,
                          "record": str(out / "diagnosis.json")}))
        return 0
    finally:
        devnet.down()


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--validators", type=int, choices=(4, 8), default=4)
    parser.add_argument("--window", type=int, default=240)
    parser.add_argument("--out", required=True)
    return collect(parser.parse_args(argv))


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
