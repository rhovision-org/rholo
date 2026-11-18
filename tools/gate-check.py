#!/usr/bin/env python3
"""Evaluate one gate criterion against one observation document.

`spec/audit/gate-criteria.json` names the principle, parameters, required observations and
negative controls for each executable external gate. This CLI is the uniform entry point:
it loads the entry, checks the required observations, and returns PASS (0), FAIL (1) or
PRECONDITION (3). A missing observation is never PASS.

This is a record measurement, not a `met` promotion. The canonical register checker
(`tools/check-mainnet-gates.py`) remains the authority on gate status.

Usage:
  python3 tools/gate-check.py --list
  python3 tools/gate-check.py --gate G584-G2 --observations target/n584/observations.json
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def _module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


principles = _module("gate_check_principles", ROOT / "tools" / "gate-principles.py")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--list", action="store_true", help="list the registered criteria")
    parser.add_argument("--gate", help="gate id, for example G584-G2")
    parser.add_argument("--observations", help="normalised observation document (JSON)")
    parser.add_argument("--criteria", default=str(principles.CRITERIA))
    args = parser.parse_args(argv)
    entries = principles.criteria_by_gate(Path(args.criteria))
    if args.list:
        for gate_id, entry in sorted(entries.items()):
            print(f"{gate_id:10} {entry['principle']:24} {entry.get('adapter', '')}")
        return 0
    if not args.gate or not args.observations:
        parser.error("--gate and --observations are required unless --list is used")
    entry = entries.get(args.gate)
    if entry is None:
        print(json.dumps({"status": "PRECONDITION", "reasons": [f"no criterion for {args.gate}"]},
                         sort_keys=True))
        return 3
    observations = json.loads(Path(args.observations).read_text())
    result = principles.evaluate_entry(entry, observations)
    print(json.dumps(result, indent=1, sort_keys=True))
    return {"PASS": 0, "FAIL": 1, "PRECONDITION": 3}[result["status"]]


if __name__ == "__main__":
    sys.exit(main())
