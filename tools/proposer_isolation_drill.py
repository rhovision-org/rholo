#!/usr/bin/env python3
"""C343's end-to-end proposer-isolation drill: a surviving validator that outruns finality halts,
says why, refuses to produce, and re-halts after the operator resume while finality is still frozen.

Why this exists. The unit tests pin the arithmetic (`MAX_PROPOSAL_FINALITY_LAG = 32`) and the phase
gates, and `U20-01/07/09/11` still name the operational arm as owed. This drill drives the real
routes on a disposable devnet:

  1. wait for four validators with advancing finality;
  2. stop validators 1..3 so only the bootstrap (validator 0) remains, and drive proposals until the
     local tip is more than 32 heights beyond the local finalized height;
  3. assert `/health` reports `proposalHalted: true`, `ready: false`, `readyReasons` contains
     `proposal_isolated`, and `proposalHaltReason` names the bound; assert the operator route carries
     the same gate; assert the tip stays flat and an explicit propose adds no block;
  4. `POST /api/v1/proposer/resume` clears the latch; the next proposal re-halts because finality is
     still frozen.

The tool asserts; it does not author a verdict. It writes `raw/result.json`, the route snapshots and
the journals. A failed assertion is recorded and exits non-zero; the record is then INCONCLUSIVE or
FAIL, not a pass.

Usage:
  python3 tools/proposer_isolation_drill.py --out target/c343-proposer-isolation
  python3 tools/proposer_isolation_drill.py --skip-up --out target/c343-proposer-isolation
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

HTTP_BASE = int(os.environ.get("DEVNET_HTTP_BASE", "40403"))
ADMIN_BASE = int(os.environ.get("DEVNET_ADMIN_BASE", "40405"))
PREFIX = os.environ.get("DEVNET_PREFIX", "devnet")
FINALITY_BOUND = 32  # MAX_PROPOSAL_FINALITY_LAG


def http_json(port: int, path: str, method: str = "GET", timeout: float = 10.0) -> dict:
    request = urllib.request.Request(f"http://127.0.0.1:{port}{path}", method=method)
    with urllib.request.urlopen(request, timeout=timeout) as response:
        body = response.read()
        return json.loads(body) if body else {}


def wait_for(port: int, path: str, predicate, timeout_s: float, poll_s: float, label: str) -> dict:
    deadline = time.monotonic() + timeout_s
    last: dict = {}
    while time.monotonic() < deadline:
        try:
            last = http_json(port, path)
            if predicate(last):
                return last
        except (urllib.error.URLError, TimeoutError, json.JSONDecodeError, ConnectionError) as exc:
            last = {"error": str(exc)}
        time.sleep(poll_s)
    raise TimeoutError(f"{label}: no qualifying answer in {timeout_s:.0f}s; last={last}")


def run(argv: list[str]) -> subprocess.CompletedProcess:
    return subprocess.run(argv, check=True, text=True, capture_output=True)


def find_key(value, key: str):
    """Find the first `key` anywhere in a JSON value (the gate is nested on the operator route)."""
    if isinstance(value, dict):
        if key in value:
            return value[key]
        for child in value.values():
            found = find_key(child, key)
            if found is not None:
                return found
    elif isinstance(value, list):
        for child in value:
            found = find_key(child, key)
            if found is not None:
                return found
    return None


def capture(path: Path, value) -> None:
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, help="run directory (gitignored target/... )")
    parser.add_argument("--skip-up", action="store_true", help="use the devnet already running")
    parser.add_argument("--validators", type=int, default=4, help="devnet validators (default 4)")
    parser.add_argument("--proposals", type=int, default=120, help="max explicit proposals to send")
    parser.add_argument("--poll", type=float, default=0.5, help="poll interval in seconds")
    parser.add_argument("--timeout", type=float, default=30 * 60, help="overall bounded wait (s)")
    parser.add_argument("--flat-window", type=float, default=15.0,
                        help="seconds the halted tip must stay flat (default 15)")
    args = parser.parse_args()

    out = Path(args.out)
    raw = out / "raw"
    raw.mkdir(parents=True, exist_ok=False)

    if not args.skip_up:
        run(["tools/devnet.sh", "up", "--validators", str(args.validators), "--fresh",
             "--autopropose", "--admin"])

    checks: dict[str, bool] = {}
    observations: dict = {}

    # 1. A healthy four-validator network with advancing finality.
    first = wait_for(HTTP_BASE, "/health",
                     lambda body: body.get("ready") is True
                     and body.get("finalizedHeight") not in (None, 0),
                     args.timeout, args.poll, "four-validator network ready")
    time.sleep(2)
    second = http_json(HTTP_BASE, "/health")
    checks["finality_advancing_before_stop"] = (
        second.get("finalizedHeight") or 0) > (first.get("finalizedHeight") or 0)
    observations["before_stop"] = second
    capture(raw / "health-before-stop.json", second)

    # 2. Leave only the bootstrap producing. Validators 1..N-1 are separate containers.
    for i in range(1, args.validators):
        run(["tools/devnet.sh", "stop", str(i)])

    latest = http_json(HTTP_BASE, "/health")
    proposals_sent = 0
    halted = False
    while proposals_sent < args.proposals:
        lag = latest.get("finalizedLag")
        if latest.get("proposalHalted") or (lag is not None and lag > FINALITY_BOUND):
            halted = True
            break
        try:
            http_json(ADMIN_BASE, "/api/v1/propose", method="POST")
        except urllib.error.HTTPError:
            # A refused propose is an observation, not a reason to abort: the next poll reads the
            # gate the refusal came from.
            pass
        proposals_sent += 1
        time.sleep(args.poll)
        latest = http_json(HTTP_BASE, "/health")
    observations["proposals_sent"] = proposals_sent
    observations["halt_health"] = latest
    capture(raw / "health-halted.json", latest)

    checks["halt_latched"] = bool(latest.get("proposalHalted"))
    checks["ready_false"] = latest.get("ready") is False
    checks["ready_reason_names_isolation"] = "proposal_isolated" in (latest.get("readyReasons") or [])
    reason = latest.get("proposalHaltReason") or ""
    checks["halt_reason_names_bound"] = str(FINALITY_BOUND) in reason or "beyond" in reason.lower()
    checks["lag_exceeds_bound"] = (latest.get("finalizedLag") or 0) > FINALITY_BOUND

    # 3. The operator route carries the same gate.
    try:
        operator = http_json(ADMIN_BASE, "/api/v1/status/operator")
        capture(raw / "operator-status-halted.json", operator)
        checks["operator_reports_halt"] = (
            find_key(operator, "proposal_halted") is True
            or find_key(operator, "proposalHalted") is True
        )
        checks["operator_names_isolation"] = bool(
            find_key(operator, "proposal_halt_reason") or find_key(operator, "proposalHaltReason"))
    except (urllib.error.URLError, urllib.error.HTTPError, json.JSONDecodeError) as exc:
        observations["operator_error"] = str(exc)
        checks["operator_reports_halt"] = False
        checks["operator_names_isolation"] = False

    # 4. The halted tip is flat, and an explicit propose adds no block.
    flat_start = latest.get("maxObservedHeight")
    time.sleep(args.flat_window)
    flat_end = http_json(HTTP_BASE, "/health").get("maxObservedHeight")
    checks["tip_flat_while_halted"] = flat_start == flat_end
    try:
        http_json(ADMIN_BASE, "/api/v1/propose", method="POST")
    except urllib.error.HTTPError:
        pass
    time.sleep(2)
    after_propose = http_json(HTTP_BASE, "/health").get("maxObservedHeight")
    checks["explicit_propose_adds_no_block"] = after_propose == flat_end
    observations["flat"] = {"start": flat_start, "end": flat_end, "after_propose": after_propose}

    # 5. The operator resume clears the latch; the next proposal re-halts while finality is frozen.
    http_json(ADMIN_BASE, "/api/v1/proposer/resume", method="POST")
    time.sleep(1)
    after_resume = http_json(HTTP_BASE, "/health")
    checks["resume_clears_latch"] = after_resume.get("proposalHalted") is False
    captured_relatch = False
    for _ in range(args.proposals):
        if after_resume.get("proposalHalted"):
            captured_relatch = True
            break
        try:
            http_json(ADMIN_BASE, "/api/v1/propose", method="POST")
        except urllib.error.HTTPError:
            pass
        time.sleep(args.poll)
        after_resume = http_json(HTTP_BASE, "/health")
    checks["rehalts_after_resume"] = captured_relatch
    observations["after_resume"] = after_resume
    capture(raw / "health-after-resume.json", after_resume)

    for name, container in [(f"{PREFIX}-bootstrap", "bootstrap.log")]:
        try:
            log = subprocess.run(["docker", "logs", "--timestamps", name], text=True,
                                 stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=True)
            (raw / container).write_text(log.stdout)
        except subprocess.CalledProcessError as exc:
            observations[f"log_error_{container}"] = str(exc)

    verdict_hint = "PASS" if all(checks.values()) else "FAIL"
    result = {
        "tool": "proposer_isolation_drill.py",
        "source_sha": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
        "bound": FINALITY_BOUND,
        "checks": checks,
        "observations": observations,
        "verdict_hint": verdict_hint,
        "execution_proven": False,
    }
    capture(raw / "result.json", result)
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if all(checks.values()) else 1


if __name__ == "__main__":
    raise SystemExit(main())
