#!/usr/bin/env python3
"""Criterion checks for the M4 rejoin and M5 merge-accounting records.

These check a campaign record, not the campaign. `rejoin` requires a fixed
anchor, a stable dwell and every-validator finalization inside the measured
healing window. `merge` requires the accounting fields, a per-deploy effect
list and a state readback; an unavailable historical bundle is PRECONDITION,
never PASS. A missing record is PRECONDITION.
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

HEX64 = re.compile(r"^[0-9a-f]{64}$")
FLOOR = 270.0
CEILING = 600.0


def nonneg(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


def check_evidence(data: dict, gate_id: str, token: object) -> tuple[str, list[str]]:
    evidence = data.get("evidence")
    if not isinstance(evidence, dict):
        return "PRECONDITION", ["no evidence record"]
    record = evidence.get("record")
    path = Path(record) if isinstance(record, str) else None
    if path is not None and not path.is_absolute():
        path = ROOT / path
    if path is None or not path.is_dir():
        return "FAIL", ["evidence.record is not a retained directory"]
    if not ((path / "manifest.json").is_file() and (path / "SHA256SUMS.json").is_file()):
        return "FAIL", ["evidence.record needs manifest.json and SHA256SUMS.json"]
    proc = subprocess.run(
        [sys.executable, str(ROOT / "tools/evidence-pack.py"), "verify",
         "--record", str(path)],
        capture_output=True, text=True, timeout=120, check=False)
    if proc.returncode != 0:
        return "FAIL", ["evidence.record does not verify"]
    manifest = json.loads((path / "manifest.json").read_text())
    errors: list[str] = []
    if manifest.get("gate_id") != gate_id:
        errors.append(f"evidence names {manifest.get('gate_id')!r}, not {gate_id}")
    if manifest.get("execution_proven") is not False:
        errors.append("execution_proven must be false")
    readme = path / "README.md"
    text = readme.read_text(errors="ignore") if readme.is_file() else ""
    if not isinstance(token, str) or not token or token not in text:
        errors.append("the criterion token is not in the evidence record")
    if not (isinstance(evidence.get("digest"), str)
            and HEX64.fullmatch(evidence["digest"])):
        errors.append("evidence.digest must be a 64-hex digest")
    return ("FAIL" if errors else "PASS"), errors


def check_rejoin(data: dict, floor: float = FLOOR, ceiling: float = CEILING,
                 min_dwell: int = 3) -> tuple[str, list[str], dict]:
    errors: list[str] = []
    anchor = data.get("anchor") or {}
    evidence_status, evidence_errors = check_evidence(data, "G428-M4", anchor.get("hash"))
    if evidence_status != "PASS":
        return evidence_status, evidence_errors, {}
    if not (isinstance(anchor.get("hash"), str) and HEX64.fullmatch(anchor["hash"])):
        errors.append("anchor.hash must be a 64-hex digest")
    if not nonneg(anchor.get("height")):
        errors.append("anchor.height must be a non-negative integer")
    dwell = data.get("dwell") or {}
    if not (isinstance(dwell.get("samples"), int) and dwell["samples"] >= min_dwell):
        errors.append(f"dwell.samples must be at least {min_dwell}")
    if dwell.get("stable") is not True:
        errors.append("dwell.stable must be true")
    window = data.get("window") or {}
    seconds = window.get("seconds")
    if not (isinstance(seconds, (int, float)) and not isinstance(seconds, bool)):
        errors.append("window.seconds must be a number")
    elif not (floor <= float(seconds) <= ceiling):
        errors.append(f"window.seconds {seconds} is outside [{floor}, {ceiling}]")
    validators = data.get("validators") or []
    if not isinstance(validators, list) or not validators:
        errors.append("validators must be a non-empty list")
        validators = []
    for validator in validators:
        if not isinstance(validator, dict):
            errors.append("every validator must be an object")
        elif validator.get("finalized_anchor") is not True:
            errors.append(f"validator {validator.get('id')!r} did not finalize the fixed anchor")
    return ("FAIL" if errors else "PASS"), errors, {"validators": len(validators)}


def check_merge(data: dict) -> tuple[str, list[str], dict]:
    errors: list[str] = []
    bundle = data.get("bundle") or {}
    if bundle.get("available") is not True:
        return "PRECONDITION", ["the authenticated historical bundle is unavailable"], {}
    capture_path = bundle.get("record")
    path = Path(capture_path) if isinstance(capture_path, str) else None
    if path is not None and not path.is_absolute():
        path = ROOT / path
    if path is None or not path.is_file():
        return "FAIL", ["a bundle claimed available has no capture record"], {}
    try:
        capture = json.loads(path.read_text())
    except (OSError, ValueError):
        return "FAIL", ["the bundle capture record is unreadable"], {}
    if capture.get("complete") is not True:
        return "FAIL", ["the bundle capture record is not complete"], {}
    preimages = capture.get("preimages") or []
    if not isinstance(preimages, list) or not preimages:
        return "FAIL", ["the bundle capture record has no preimages"], {}
    if not all(isinstance(item, dict) and item.get("verify_ok") is True
               for item in preimages):
        return "FAIL", ["a bundle preimage did not verify"], {}
    digest = bundle.get("digest")
    if not (isinstance(digest, str) and HEX64.fullmatch(digest)):
        return "FAIL", ["bundle.digest must be a 64-hex digest"], {}
    evidence_status, evidence_errors = check_evidence(data, "G428-M5", digest)
    if evidence_status != "PASS":
        return evidence_status, evidence_errors, {}
    for field in ("final_scope_rejected_chains", "rejections_without_a_conflict",
                  "unapplied_kept_writes"):
        if not nonneg(data.get(field)):
            errors.append(f"{field} must be a non-negative integer")
    if nonneg(data.get("unapplied_kept_writes")) and data["unapplied_kept_writes"] != 0:
        errors.append("a kept write was not applied")
    if (nonneg(data.get("rejections_without_a_conflict"))
            and data["rejections_without_a_conflict"] != 0):
        errors.append("a rejection has no conflicting transition")
    effects = data.get("per_deploy_effects") or []
    if not isinstance(effects, list) or not effects:
        errors.append("per_deploy_effects must be a non-empty list")
        effects = []
    for effect in effects:
        if not isinstance(effect, dict):
            errors.append("every effect must be an object")
            continue
        if not (isinstance(effect.get("deploy_id"), str) and effect["deploy_id"].strip()):
            errors.append("every effect needs a deploy_id")
        if effect.get("verdict") not in ("accepted", "rejected", "final_rejected"):
            errors.append(f"unknown effect verdict {effect.get('verdict')!r}")
    if data.get("kept_readback") is not True:
        errors.append("kept_readback must be true; the report is not the state")
    return ("FAIL" if errors else "PASS"), errors, {
        "effects": len(effects),
        "final_scope_rejected_chains": data.get("final_scope_rejected_chains"),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("rejoin", "merge"):
        command = sub.add_parser(name)
        command.add_argument("--record", required=True)
    args = parser.parse_args()
    path = Path(args.record)
    if not path.is_file():
        print(json.dumps({"result": "PRECONDITION", "detail": f"{path} is absent"}, sort_keys=True))
        return 3
    try:
        data = json.loads(path.read_text())
    except (OSError, ValueError) as exc:
        print(json.dumps({"result": "FAIL", "errors": [f"unreadable record: {exc}"]}, sort_keys=True))
        return 1
    checker = {"rejoin": check_rejoin, "merge": check_merge}[args.command]
    result, errors, detail = checker(data)
    print(json.dumps({"result": result, "errors": errors, "detail": detail,
                      "record_check": True}, sort_keys=True))
    return 0 if result == "PASS" else (3 if result == "PRECONDITION" else 1)


if __name__ == "__main__":
    raise SystemExit(main())
