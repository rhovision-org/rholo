#!/usr/bin/env python3
"""Verify a fresh r-cash installer state without copying private keys to CI artifacts.

This is the inclusion-checked installer, not the unscheduled 17-deploy burst.
"""

import argparse
import json
from pathlib import Path
import re
import sys

HEX = re.compile(r"^[0-9a-fA-F]{64}$")


def assess(state, expected=17):
    steps = state.get("steps")
    if not isinstance(steps, dict) or len(steps) != expected:
        raise AssertionError(f"required {expected} installer steps, got {len(steps) if isinstance(steps, dict) else 'invalid'}")
    if not isinstance(state.get("root_uri"), str) or not state["root_uri"].startswith("rho:id:"):
        raise AssertionError("installed root URI missing")
    findings = []
    for name, datum in steps.items():
        if not isinstance(datum, dict):
            raise AssertionError(f"{name}: malformed installer step")
        deploy_id = datum.get("deploy_id")
        block_hash = datum.get("block")
        uris = datum.get("uris")
        if datum.get("reused"):
            raise AssertionError(f"{name}: step was reused instead of freshly installed")
        if not isinstance(deploy_id, str) or not deploy_id.strip():
            raise AssertionError(f"{name}: no deploy ID")
        if not isinstance(block_hash, str) or not HEX.fullmatch(block_hash):
            raise AssertionError(f"{name}: no pinned carrying block")
        if not isinstance(uris, list) or not uris or any(
                not isinstance(x, str) or not x.startswith("rho:id:")
                for x in uris):
            raise AssertionError(f"{name}: missing or unverified Root key URI")
        findings.append({
            "step": name, "deploy_id": deploy_id, "block_hash": block_hash,
            "resolved_keys": len(uris),
        })
    return {"result": "PASS", "steps_verified": len(findings),
            "root_uri": state["root_uri"], "steps": findings,
            "boundary": "source-pinned r-cash verified installer; NOT unpaced 17-deploy burst"}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--state", required=True)
    p.add_argument("--out", required=True)
    args = p.parse_args()
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    result = {"result": "INCOMPLETE"}
    try:
        state = json.loads(Path(args.state).read_text())
        result = assess(state)
    except Exception as error:
        result = {"result": "FAIL", "error": f"{type(error).__name__}: {error}"}
        raise
    finally:
        out.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
        print("r-cash installer evidence", result["result"], flush=True)


if __name__ == "__main__":
    try:
        main()
    except Exception as exc:
        print(f"FAIL: {exc}", file=sys.stderr)
        sys.exit(1)
