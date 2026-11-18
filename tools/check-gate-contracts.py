#!/usr/bin/env python3
"""Docker-free checks for the three #51 release-blocker contracts.

These are record checks, not proof that an operator export or a rollout ran.
`vault` asserts conservation and uniqueness; `state` asserts that completeness
is the manifest's counts and roots, not an assertion; `attachment` asserts the
selected pins, lockstep activation and old/new client refusal. A missing input
is PRECONDITION, never PASS.
"""
from __future__ import annotations

import argparse
import datetime as dt
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PROVENANCE_RECORD = "spec/audit/evidence/n564-binary-qualification-20261113"

HEX40 = re.compile(r"^[0-9a-f]{40}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
KINDS = ("vault", "bond", "reward", "pot")


def nonneg(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


def check_provenance(data: dict) -> tuple[str, list[str]]:
    """An operator/evidence envelope is the external input; a synthetic record is not."""
    provenance = data.get("provenance")
    if not isinstance(provenance, dict):
        return "PRECONDITION", ["no operator attestation/evidence record"]
    errors: list[str] = []
    if not (isinstance(provenance.get("operator"), str) and provenance["operator"].strip()):
        errors.append("provenance.operator is required")
    try:
        signed = dt.datetime.fromisoformat(
            str(provenance.get("signed_at") or "").replace("Z", "+00:00"))
        if signed.tzinfo is None:
            errors.append("provenance.signed_at must be timezone-aware")
    except ValueError:
        errors.append("provenance.signed_at is not a UTC timestamp")
    if not (isinstance(provenance.get("digest"), str)
            and HEX64.fullmatch(provenance["digest"])):
        errors.append("provenance.digest must be a 64-hex digest")
    record = provenance.get("record")
    path = Path(record) if isinstance(record, str) else None
    if path is not None and not path.is_absolute():
        path = ROOT / path
    if path is None or not path.is_dir():
        errors.append("provenance.record must be a retained evidence directory")
    elif not ((path / "manifest.json").is_file() and (path / "SHA256SUMS.json").is_file()):
        errors.append("provenance.record needs manifest.json and SHA256SUMS.json")
    else:
        proc = subprocess.run(
            [sys.executable, str(ROOT / "tools/evidence-pack.py"), "verify",
             "--record", str(path)],
            capture_output=True, text=True, timeout=120, check=False)
        if proc.returncode != 0:
            errors.append("provenance.record does not verify")
    return ("FAIL" if errors else "PASS"), errors


def check_vault(data: dict) -> tuple[str, list[str], dict]:
    provenance, provenance_errors = check_provenance(data)
    if provenance != "PASS":
        return provenance, provenance_errors, {}
    errors: list[str] = []
    if data.get("schema") != 1:
        errors.append("schema 1 required")
    checkpoint = data.get("checkpoint") or {}
    if not nonneg(checkpoint.get("height")):
        errors.append("checkpoint.height must be a non-negative integer")
    if not (isinstance(checkpoint.get("hash"), str) and HEX64.fullmatch(checkpoint["hash"])):
        errors.append("checkpoint.hash must be a 64-hex digest")
    entries = data.get("entries") or []
    if not isinstance(entries, list) or not entries:
        errors.append("entries must be a non-empty list")
        entries = []
    seen: set[str] = set()
    sums = {kind: 0 for kind in KINDS}
    for entry in entries:
        if not isinstance(entry, dict):
            errors.append("every entry must be an object")
            continue
        ident = entry.get("id")
        if not (isinstance(ident, str) and ident.strip()):
            errors.append("every entry needs an id")
        elif ident in seen:
            errors.append(f"duplicate entry id {ident}")
        else:
            seen.add(ident)
        kind = entry.get("kind")
        if kind not in KINDS:
            errors.append(f"unknown entry kind {kind!r}")
        balance = entry.get("balance")
        if not nonneg(balance):
            errors.append(f"entry {ident!r} has a negative or non-integer balance")
        elif kind in KINDS:
            sums[kind] += balance
    totals = data.get("totals") or {}
    for key in ("supply", "vaults", "bonded", "rewards", "pot"):
        if not nonneg(totals.get(key)):
            errors.append(f"totals.{key} must be a non-negative integer")
    if not errors:
        if totals["vaults"] != sums["vault"]:
            errors.append(f"vault total {totals['vaults']} != entries {sums['vault']}")
        if totals["bonded"] != sums["bond"]:
            errors.append(f"bond total {totals['bonded']} != entries {sums['bond']}")
        if totals["rewards"] != sums["reward"]:
            errors.append(f"reward total {totals['rewards']} != entries {sums['reward']}")
        if totals["pot"] != sums["pot"]:
            errors.append(f"pot total {totals['pot']} != entries {sums['pot']}")
        conserved = totals["vaults"] + totals["bonded"] + totals["rewards"] + totals["pot"]
        if totals["supply"] != conserved:
            errors.append(f"supply {totals['supply']} != vaults+bonded+rewards+pot {conserved}")
    return ("FAIL" if errors else "PASS"), errors, {"entries": len(entries), "totals": totals}


def check_state(data: dict) -> tuple[str, list[str], dict]:
    provenance, provenance_errors = check_provenance(data)
    if provenance != "PASS":
        return provenance, provenance_errors, {}
    errors: list[str] = []
    if data.get("schema") != 1:
        errors.append("schema 1 required")
    checkpoint = data.get("checkpoint") or {}
    if not nonneg(checkpoint.get("height")):
        errors.append("checkpoint.height must be a non-negative integer")
    if not (isinstance(checkpoint.get("hash"), str) and HEX64.fullmatch(checkpoint["hash"])):
        errors.append("checkpoint.hash must be a 64-hex digest")
    roots = data.get("roots") or {}
    for key in ("native", "rspace", "history"):
        if not (isinstance(roots.get(key), str) and HEX64.fullmatch(roots[key])):
            errors.append(f"roots.{key} must be a 64-hex digest")
    counts = data.get("counts") or {}
    for key in ("native", "rspace", "history"):
        if not nonneg(counts.get(key)):
            errors.append(f"counts.{key} must be a non-negative integer")
    if data.get("complete") is not True:
        errors.append("complete must be true; an assertion is not completeness")
    entries = data.get("entries")
    if not isinstance(entries, dict):
        errors.append("entries is required for completeness")
    else:
        for key in ("native", "rspace", "history"):
            listed = entries.get(key)
            if not isinstance(listed, list):
                errors.append(f"entries.{key} must be a list")
            elif nonneg(counts.get(key)) and len(listed) != counts[key]:
                errors.append(f"entries.{key} has {len(listed)} items, count says {counts[key]}")
    for key in ("native", "rspace", "history"):
        if roots.get(key) == "0" * 64:
            errors.append(f"roots.{key} is an all-zero placeholder")
    return ("FAIL" if errors else "PASS"), errors, {"counts": counts}


def _utc(value: object) -> dt.datetime | None:
    if not isinstance(value, str):
        return None
    try:
        parsed = dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None
    return parsed if parsed.tzinfo is not None else None


def check_attachment(data: dict) -> tuple[str, list[str], dict]:
    provenance, provenance_errors = check_provenance(data)
    if provenance != "PASS":
        return provenance, provenance_errors, {}
    errors: list[str] = []
    if data.get("schema") != 1:
        errors.append("schema 1 required")
    selected = data.get("selected") or {}
    if not (isinstance(selected.get("source_sha"), str)
            and HEX40.fullmatch(selected["source_sha"])):
        errors.append("selected.source_sha must be a commit")
    for key in ("artifact_digest", "binary_sha256"):
        if not (isinstance(selected.get(key), str) and HEX64.fullmatch(selected[key])):
            errors.append(f"selected.{key} must be a 64-hex digest")
    validators = data.get("validators") or []
    if not isinstance(validators, list) or not validators:
        errors.append("validators must be a non-empty list")
        validators = []
    versions = {v.get("version") for v in validators if isinstance(v, dict)}
    if len(versions) > 1:
        errors.append(f"validators run different versions: {sorted(map(str, versions))}")
    times = []
    for validator in validators:
        if not isinstance(validator, dict):
            errors.append("every validator must be an object")
            continue
        if validator.get("binary_sha256") != selected.get("binary_sha256"):
            errors.append(f"validator {validator.get('id')!r} does not run the selected binary")
        if validator.get("still_serving_old") is not False:
            errors.append(f"validator {validator.get('id')!r} still serves the old version")
        parsed = _utc(validator.get("activated_utc"))
        if parsed is None:
            errors.append(f"validator {validator.get('id')!r} has no usable activation time")
        else:
            times.append(parsed)
    window = data.get("window_seconds")
    if not nonneg(window) or window == 0:
        errors.append("window_seconds must be a positive integer")
    elif times and (max(times) - min(times)).total_seconds() > window:
        span = (max(times) - min(times)).total_seconds()
        errors.append(f"activation span {span:.0f}s exceeds the {window}s window")
    clients = data.get("clients")
    if not clients:
        return "PRECONDITION", errors + ["no old/new client refusal evidence"], {"validators": len(validators)}
    if not any(c.get("supports_attachments") is True for c in clients if isinstance(c, dict)):
        errors.append("no client supports attachments")
    if not any(c.get("refuses_old") is True for c in clients if isinstance(c, dict)):
        errors.append("no old client refusal is recorded")
    return ("FAIL" if errors else "PASS"), errors, {"validators": len(validators), "clients": len(clients)}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("vault", "state", "attachment"):
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
    checker = {"vault": check_vault, "state": check_state, "attachment": check_attachment}[args.command]
    result, errors, detail = checker(data)
    print(json.dumps({"result": result, "errors": errors, "detail": detail,
                      "record_check": True}, sort_keys=True))
    return 0 if result == "PASS" else (3 if result == "PRECONDITION" else 1)


if __name__ == "__main__":
    raise SystemExit(main())
