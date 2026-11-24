#!/usr/bin/env python3
"""Failover mechanism diagnostic: a dead first seed, a live second seed, on loopback.

This proves the ordered bootstrap seed list end to end: a joiner is given a dead first seed and
the live bootstrap as its second, and it must connect through the second and reach the bootstrap's
finalised head. It is a **loopback-diagnostic**, never the RC-4/failover arm record: that arm's
locality is `off-host-claim` and needs two routable non-private hosts.

The run is one devnet with one observer. `DEVNET_OBSERVER_SEEDS` places the dead seed before the
live bootstrap; the observer's log must carry the fallback witness. The report lands in
`--out` (default `target/failover-diagnostic`) and is not packable evidence.

    python3 tools/failover-diagnostic.py --image rnode:campaign-20261123
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import os
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
_COMMON_SPEC = importlib.util.spec_from_file_location(
    "release_claim_common", ROOT / "tests" / "release_claim_common.py")
common = importlib.util.module_from_spec(_COMMON_SPEC)
_COMMON_SPEC.loader.exec_module(common)

DEAD_SEED = "rnode://" + "11" * 32 + "@127.0.0.1:49999?protocol=49997&discovery=49999"
LIVE_SEED = "rnode://<bootstrap-id>@failover-bootstrap?protocol=40400&discovery=40404"


def observer_logs(name: str) -> str:
    result = subprocess.run(["docker", "logs", name], text=True, capture_output=True)
    return result.stdout + result.stderr


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", default=os.environ.get("RNODE_IMAGE", "rnode:local"),
                        help="the node image the devnet runs")
    parser.add_argument("--out", default=str(ROOT / "target" / "failover-diagnostic"))
    parser.add_argument("--keep", action="store_true", help="leave the devnet running")
    args = parser.parse_args()

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    os.environ["RNODE_IMAGE"] = args.image
    os.environ["DEVNET_OBSERVER_SEEDS"] = DEAD_SEED

    devnet = common.Devnet("failover", validators=1, http_base=61403, admin_base=61405,
                           grpc_base=61402, extra_up=("--observers", "1"))
    bootstrap = common.Target([
        {"name": "bootstrap", "http": "http://127.0.0.1:61403",
         "admin": "http://127.0.0.1:61405"}])
    observer = common.Target([
        {"name": "observer", "http": "http://127.0.0.1:63403",
         "admin": "http://127.0.0.1:63405"}])
    try:
        devnet.up()
        log = observer_logs("failover-observer-1")
        (out / "observer.log").write_text(log)
        witness = None
        for line in log.splitlines():
            if "bootstrap seed " in line and "connected:" in line:
                witness = line.strip()
                break
        fallback = bool(
            witness and "bootstrap seed 2/2 connected:" in witness
            and "failover-bootstrap" in witness)

        agree = False
        bootstrap_head = observer_head = None
        deadline = time.time() + 300
        while time.time() < deadline:
            try:
                bootstrap_head = bootstrap.finalized_head(0)
                observer_head = observer.finalized_head(0)
                if observer_head == bootstrap_head and observer_head[0] > 0:
                    agree = True
                    break
            except Exception:
                pass
            time.sleep(2)

        report = {
            "schema": 1,
            "locality": "loopback-diagnostic",
            "seeds": [DEAD_SEED, LIVE_SEED],
            "seed_status": ["down", "live"],
            "joined": agree,
            "joined_via": "failover-bootstrap" if fallback else None,
            "finalised_head_agrees": agree,
            "fallback_witness": witness,
            "bootstrap_head": list(bootstrap_head) if bootstrap_head else None,
            "observer_head": list(observer_head) if observer_head else None,
            "image": args.image,
            "non_claims": [
                "a loopback diagnostic is not the RC-4/failover arm",
                "the arm needs two routable non-private hosts and a down-then-live window",
            ],
        }
        (out / "failover.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report, indent=2))
        return 0 if fallback and agree else 1
    finally:
        if not args.keep:
            devnet.down()


if __name__ == "__main__":
    raise SystemExit(main())
