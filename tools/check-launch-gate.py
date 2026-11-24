#!/usr/bin/env python3
"""Check the aggregate launch gate: claims ready and external obligations dispositioned.

The `launch-gate` action in `spec/audit/release-claims.json` is a programme action, not an arm.
Its two halves are different registers:

  * the claims half is `tools/release-check.py --assert-ready` — every required arm must read
    `safe` over a committed record;
  * the external half is `spec/audit/mainnet-gates.external.json` — rows that are not
    `met`/`superseded` are open obligations, and each open row needs exactly one dated
    disposition in `spec/audit/launch-gate.dispositions.json`.

This checker never authors a status. The open set is derived from the external register; the
dispositions file carries decisions only. A disposition is a release-owner record, not a pass:
`--gate` enforces the stated clause ("refuse ready while any external item is open without a
dated disposition"), while `--assert-ready` requires closure — every external row must be
`met`/`superseded`. M5 blocked by an owner decision is a continuing blocker, not a waiver.

Exit codes: 0 pass, 2 malformed or anti-rot, 3 not ready.
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
EXTERNAL = ROOT / "spec" / "audit" / "mainnet-gates.external.json"
DISPOSITIONS = ROOT / "spec" / "audit" / "launch-gate.dispositions.json"
RELEASE_CHECK = ROOT / "tools" / "release-check.py"

RESOLVED = {"met", "superseded"}
DISPOSITION_VOCABULARY = {
    "accepted-risk", "deferred", "waived", "closed-elsewhere", "not-a-launch-condition",
}
DATE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
DISPOSITION_KEYS = {"gate_id", "disposition", "dated", "authority", "reference", "note"}
TOP_LEVEL_KEYS = {"schema", "as_of", "rule", "dispositions"}


class GateError(ValueError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise GateError(message)


def load_json(path: Path) -> dict:
    try:
        document = json.loads(path.read_text())
    except OSError as error:
        raise GateError(f"cannot read {path}: {error}") from error
    except ValueError as error:
        raise GateError(f"{path} is not JSON: {error}") from error
    require(isinstance(document, dict), f"{path} is not a JSON object")
    return document


def external_rows(document: dict) -> list[dict]:
    gates = document.get("gates")
    require(isinstance(gates, list) and gates, "the external register carries no gates list")
    ids = set()
    for gate in gates:
        require(isinstance(gate, dict), "an external gate row is not an object")
        ident = gate.get("id")
        require(isinstance(ident, str) and ident, "an external gate row has no id")
        require(ident not in ids, f"external gate {ident!r} appears twice")
        ids.add(ident)
        require(isinstance(gate.get("status"), str) and gate["status"],
                f"external gate {ident!r} has no status")
    return gates


def open_rows(gates: list[dict]) -> list[dict]:
    return [gate for gate in gates if gate["status"] not in RESOLVED]


def _reference_ok(reference: str) -> bool:
    return (reference.startswith("https://")
            or reference.startswith("spec/")
            or reference.startswith("docs/"))


def validate(gates: list[dict], document: dict, today: dt.date) -> dict:
    require(document.get("schema") == 1, "the dispositions register schema must be 1")
    extra = set(document) - TOP_LEVEL_KEYS
    require(not extra, f"the dispositions register has unknown key(s): {sorted(extra)}")
    rows = document.get("dispositions")
    require(isinstance(rows, list), "dispositions must be a list")

    by_id: dict[str, dict] = {}
    status_by_id = {gate["id"]: gate["status"] for gate in gates}
    for row in rows:
        require(isinstance(row, dict), "a disposition row is not an object")
        extra = set(row) - DISPOSITION_KEYS
        require(not extra, f"a disposition row has unknown key(s): {sorted(extra)}")
        ident = row.get("gate_id")
        require(isinstance(ident, str) and ident, "a disposition row has no gate_id")
        require(ident in status_by_id, f"disposition for unknown external gate {ident!r}")
        require(ident not in by_id, f"duplicate disposition for {ident!r}")
        require(status_by_id[ident] not in RESOLVED,
                f"disposition for {ident!r}, which is already {status_by_id[ident]}")
        disposition = row.get("disposition")
        require(disposition in DISPOSITION_VOCABULARY,
                f"{ident}: disposition {disposition!r} is not one of "
                f"{sorted(DISPOSITION_VOCABULARY)}")
        dated = row.get("dated")
        require(isinstance(dated, str) and DATE.fullmatch(dated),
                f"{ident}: dated must be YYYY-MM-DD")
        try:
            parsed = dt.date.fromisoformat(dated)
        except ValueError as error:
            raise GateError(f"{ident}: dated is not a date: {error}") from error
        require(parsed <= today, f"{ident}: dated {dated} is in the future")
        for field in ("authority", "reference"):
            value = row.get(field)
            require(isinstance(value, str) and value.strip(), f"{ident}: {field} is empty")
        require(_reference_ok(row["reference"]),
                f"{ident}: reference must be https:// or an in-repo spec/ or docs/ path")
        by_id[ident] = row

    open_ids = sorted(gate["id"] for gate in open_rows(gates))
    missing = [ident for ident in open_ids if ident not in by_id]
    return {
        "open": open_ids,
        "dispositioned": sorted(by_id),
        "missing": missing,
    }


def claims_ready() -> tuple[bool, str]:
    result = subprocess.run(
        [sys.executable, str(RELEASE_CHECK), "--assert-ready"],
        cwd=ROOT, text=True, capture_output=True)
    return result.returncode == 0, (result.stdout + result.stderr).strip()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--gate", action="store_true",
                        help="exit 3 unless every open external row has a dated disposition")
    parser.add_argument("--assert-ready", action="store_true",
                        help="exit 0 only when the claims are ready and every external row is "
                             "met or superseded")
    parser.add_argument("--json", action="store_true", help="print the report as JSON")
    parser.add_argument("--today", help="evaluate dates against this YYYY-MM-DD (tests)")
    parser.add_argument("--external", default=str(EXTERNAL))
    parser.add_argument("--dispositions", default=str(DISPOSITIONS))
    args = parser.parse_args(argv)

    today = dt.date.fromisoformat(args.today) if args.today else dt.date.today()
    try:
        gates = external_rows(load_json(Path(args.external)))
        register = load_json(Path(args.dispositions))
        report = validate(gates, register, today)
    except GateError as error:
        print(json.dumps({"result": "INVALID", "error": str(error)}, sort_keys=True))
        return 2

    open_ids = report["open"]
    missing = report["missing"]
    if args.assert_ready:
        ready, detail = claims_ready()
        blockers = list(missing)
        if open_ids:
            blockers = [f"external:{ident}" for ident in open_ids]
        result = {
            "result": "READY" if ready and not open_ids else "NOT_READY",
            "claims_ready": ready,
            "claims_detail": detail,
            "open": open_ids,
            "missing_dispositions": missing,
            "blockers": blockers,
        }
        if args.json:
            print(json.dumps(result, indent=2, sort_keys=True))
        else:
            print("LAUNCH_GATE_READY" if result["result"] == "READY"
                  else "LAUNCH_GATE_NOT_READY")
            if not ready:
                print("  claims: " + (detail.splitlines()[0] if detail else "not ready"))
            for ident in open_ids:
                suffix = "" if ident not in missing else " (no dated disposition)"
                print(f"  external: {ident}{suffix}")
        return 0 if result["result"] == "READY" else 3

    result = {
        "result": "DISPOSITIONED" if not missing else "NOT_DISPOSITIONED",
        "as_of": register.get("as_of"),
        "open": open_ids,
        "dispositioned": report["dispositioned"],
        "missing": missing,
    }
    if args.json:
        print(json.dumps(result, indent=2, sort_keys=True))
    else:
        print("LAUNCH_GATE_DISPOSITIONED" if not missing
              else "LAUNCH_GATE_NOT_DISPOSITIONED")
        for ident in open_ids:
            state = "dispositioned" if ident not in missing else "MISSING"
            print(f"  {ident}: {state}")
    if args.gate:
        return 0 if not missing else 3
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
