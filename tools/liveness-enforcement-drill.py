#!/usr/bin/env python3
"""C344's liveness-enforcement devnet arm: jail an absent validator, slash it by a passing stake
vote, and (in the stall arm) show that a chain below the two-thirds quorum creates no liveness
record.

Why this exists. `U21-01/04/09` and `H-RS-17` name the live/devnet arm as owed. The unit tests and
the Lean model pin the rules; this drill drives the real PoS surface on a disposable devnet:

  * `jail-vote` — stakes 700,100,100,100; stop validator 3; wait two advancing boundaries; read
    `getLiveness` until its record shows a jail, and `getBonds` to show the bond is untouched; then
    submit a signed `voteSlash` from validator 0, cross a boundary, and show the bond is removed.
  * `stall` — stakes 400,300,200,100; stop validators 1..3 so only 400/1000 of the bonded stake
    remains (below 2/3); assert the epoch does not advance and no liveness record is created.

The tool asserts and records; it does not author a verdict. It writes `raw/result.json`, the route
replies and the bootstrap journal. A failed assertion exits non-zero and the record is FAIL /
INCONCLUSIVE, never a pass.

Usage:
  python3 tools/liveness-enforcement-drill.py --arm jail-vote --out target/c344-jail-vote
  python3 tools/liveness-enforcement-drill.py --arm stall --out target/c344-stall
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

HTTP_BASE = int(os.environ.get("DEVNET_HTTP_BASE", "40403"))
ADMIN_BASE = int(os.environ.get("DEVNET_ADMIN_BASE", "40405"))
PREFIX = os.environ.get("DEVNET_PREFIX", "devnet")
ROOT = Path(__file__).resolve().parents[1]
EPOCH_LENGTH = 3


def http_json(port: int, path: str, method: str = "GET", body=None, timeout: float = 20.0):
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(
        f"http://127.0.0.1:{port}{path}", data=data, method=method,
        headers={"Content-Type": "application/json"} if data is not None else {},
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        raw = response.read()
        return json.loads(raw) if raw else {}


def wait_until(fn, timeout_s: float, poll_s: float, label: str):
    deadline = time.monotonic() + timeout_s
    last = None
    while time.monotonic() < deadline:
        try:
            last = fn()
            if last is not None:
                return last
        except (urllib.error.URLError, TimeoutError, json.JSONDecodeError, ConnectionError) as exc:
            last = {"error": str(exc)}
        time.sleep(poll_s)
    raise TimeoutError(f"{label}: no qualifying answer in {timeout_s:.0f}s; last={last}")


def run(argv: list[str], timeout: float | None = None) -> subprocess.CompletedProcess:
    return subprocess.run(argv, check=True, text=True, capture_output=True, timeout=timeout)


def docker(argv: list[str]) -> subprocess.CompletedProcess:
    return subprocess.run(["docker", *argv], check=True, text=True,
                          stdout=subprocess.PIPE, stderr=subprocess.STDOUT)


def decode(node):
    """Decode the node's RhoExpr wire shape into Python values."""
    if isinstance(node, list):
        return [decode(item) for item in node]
    if not isinstance(node, dict) or len(node) != 1:
        return node
    (tag, payload), = node.items()
    data = payload.get("data") if isinstance(payload, dict) else payload
    if tag in ("ExprPar", "ExprList", "ExprSet"):
        return [decode(item) for item in data]
    if tag == "ExprTuple":
        return tuple(decode(item) for item in data)
    if tag == "ExprMap":
        return {key: decode(value) for key, value in data.items()}
    if tag == "ExprInt":
        return data
    if tag == "ExprBool":
        return data
    if tag in ("ExprString", "ExprUri", "ExprBytes"):
        return data
    return node


def explore(term: str):
    reply = http_json(HTTP_BASE, "/api/v1/explore-deploy", method="POST", body=term)
    return [decode(item) for item in reply.get("expr", [])]


def read_map(term: str):
    values = explore(term)
    if not values:
        return {}
    value = values[0]
    if isinstance(value, list) and len(value) == 1 and isinstance(value[0], dict):
        value = value[0]
    return value if isinstance(value, dict) else {}


def liveness() -> dict:
    return read_map(
        'new pos(`rho:rchain:pos`), ret in { pos!("getLiveness", [*ret]) | '
        'for (@m <- ret) { @"out"!(*m) } }'
    )


def bonds() -> dict:
    return read_map(
        'new pos(`rho:rchain:pos`), ret in { pos!("getBonds", [*ret]) | '
        'for (@m <- ret) { @"out"!(*m) } }'
    )


def pos_status() -> dict:
    return http_json(HTTP_BASE, "/api/v1/pos")


def validator_keys() -> list[str]:
    text = (ROOT / "tools" / "devnet.sh").read_text()
    block = re.search(r"VALIDATOR_PUB=\(\s*(.*?)\s*\)", text, re.S)
    if not block:
        raise RuntimeError("devnet.sh has no VALIDATOR_PUB table")
    return re.findall(r'"([0-9a-f]{130})"', block.group(1))


def target_record(records: dict, target_hex: str):
    for key, value in records.items():
        if target_hex in key or key in target_hex:
            return value
    return None


def submit_signed(term: str, height: int) -> str:
    """Deploy a term signed by the bootstrap's own validator key (validator 0 / funded deployer)."""
    with tempfile.NamedTemporaryFile("w", suffix=".rho", delete=False) as handle:
        handle.write(term)
        host_path = handle.name
    container = "c344-term.rho"
    docker(["cp", host_path, f"{PREFIX}-bootstrap:/tmp/{container}"])
    deployed = docker([
        "exec", f"{PREFIX}-bootstrap", "rnode", "deploy",
        "--phlo-limit", "1000000", "--phlo-price", "1",
        "--private-key-path", "/var/lib/rnode/validator.hex",
        "--shard-id", "root", "--valid-after-block-number", str(max(height - 1, 0)),
        f"/tmp/{container}",
    ])
    match = re.search(r"DeployId is:\s*([0-9a-f]+)", deployed.stdout)
    if not match:
        raise RuntimeError(f"signed deploy rejected: {deployed.stdout[-1200:]}")
    return match.group(1)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--arm", choices=["jail-vote", "stall"], default="jail-vote")
    parser.add_argument("--out", required=True)
    parser.add_argument("--skip-up", action="store_true")
    parser.add_argument("--poll", type=float, default=1.0)
    parser.add_argument("--timeout", type=float, default=20 * 60)
    args = parser.parse_args()

    out = Path(args.out)
    raw = out / "raw"
    raw.mkdir(parents=True, exist_ok=False)
    keys = validator_keys()
    checks: dict[str, bool] = {}
    observations: dict = {}

    if not args.skip_up:
        stakes = "700,100,100,100" if args.arm == "jail-vote" else "400,300,200,100"
        run(["tools/devnet.sh", "up", "--validators", "4", "--fresh", "--autopropose", "--admin",
             "--epoch-length", str(EPOCH_LENGTH), "--stakes", stakes,
             "--liveness-absence-threshold", "1",
             "--liveness-jail-after-boundaries", "2",
             "--liveness-jail-epochs", "5",
             "--liveness-vote-window", "3",
             "--liveness-slash-severity", "misdemeanour"])

    def ready():
        body = http_json(HTTP_BASE, "/health")
        return body if body.get("ready") and body.get("finalizedHeight") else None

    wait_until(ready, args.timeout, args.poll, "devnet ready")

    if args.arm == "jail-vote":
        target = keys[3]
        baseline_bonds = bonds()
        observations["bonds_before"] = baseline_bonds
        baseline_stake = target_record(baseline_bonds, target)

        run(["tools/devnet.sh", "stop", "3"])
        jailed = wait_until(
            lambda: (lambda record: record if record and int(record[1]) > 0 else None)(
                target_record(liveness(), target)),
            args.timeout, args.poll, "validator 3 jailed")
        observations["jail_record"] = jailed
        checks["jailed"] = int(jailed[1]) > 0
        checks["bond_untouched_by_jail"] = target_record(bonds(), target) == baseline_stake

        vote = (
            'new pos(`rho:rchain:pos`), deployerId(`rho:rchain:deployerId`), ret in { '
            f'pos!("voteSlash", [*deployerId, "{target}".hexToBytes(), true, *ret]) | '
            'for (@r <- ret) { @"out"!(*r) } }'
        )
        submit_signed(vote, http_json(HTTP_BASE, "/api/status").get("latestBlockNumber", 1))
        http_json(ADMIN_BASE, "/api/v1/propose", method="POST")
        slashed = wait_until(
            lambda: True if target_record(bonds(), target) in (None, 0) else None,
            args.timeout, args.poll, "passing vote slashes the bond")
        observations["bonds_after_vote"] = bonds()
        checks["vote_slashed_the_bond"] = slashed is True
    else:
        for i in (1, 2, 3):
            run(["tools/devnet.sh", "stop", str(i)])
        time.sleep(5)
        before = pos_status()
        time.sleep(3 * EPOCH_LENGTH * 2)
        after = pos_status()
        observations["pos_before"] = before
        observations["pos_after"] = after
        checks["epoch_frozen_below_quorum"] = before.get("epoch") == after.get("epoch")
        # A stalled chain creates no new liveness record for a validator that never produced.
        checks["no_liveness_record_created"] = liveness() == {}

    try:
        log = docker(["logs", "--timestamps", f"{PREFIX}-bootstrap"])
        (raw / "bootstrap.log").write_text(log.stdout)
    except subprocess.CalledProcessError as exc:
        observations["log_error"] = str(exc)

    verdict_hint = "PASS" if all(checks.values()) else "FAIL"
    result = {
        "tool": "liveness-enforcement-drill.py",
        "arm": args.arm,
        "source_sha": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
        "epoch_length": EPOCH_LENGTH,
        "checks": checks,
        "observations": observations,
        "verdict_hint": verdict_hint,
        "execution_proven": False,
    }
    (raw / "result.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if all(checks.values()) else 1


if __name__ == "__main__":
    raise SystemExit(main())
