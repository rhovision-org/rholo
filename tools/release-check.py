#!/usr/bin/env python3
"""Run, inspect and record the seven release-claim tests.

    python3 tools/release-check.py list
    python3 tools/release-check.py run --claim RC-1 --env local
    python3 tools/release-check.py run --claim RC-1 --env public --public-manifest plan.json
    python3 tools/release-check.py check --claim RC-1 --record target/release-claims/RC-1
    python3 tools/release-check.py record --claim RC-1 --env local --record target/release-claims/RC-1
    python3 tools/release-check.py --assert-ready

A claim's status is `pass`, `fail` or `not run`, per environment. It is derived from the newest
committed record under `spec/audit/evidence/` by re-running the claim test's own `check(record)`;
`target/` runs are shown as scratch and never make the tracker ready. Nothing about a status is
authored.
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
AUDIT = ROOT / "spec" / "audit"
CLAIMS = AUDIT / "release-claims.json"
EVIDENCE = AUDIT / "evidence"
SCRATCH = ROOT / "target" / "release-claims"
ENVIRONMENTS = ("local", "public")


def _load(path: Path, name: str):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def builder():
    return _load(ROOT / "tools" / "build-mainnet-gates.py", "build_mainnet_gates")


def claims() -> list[dict]:
    return json.loads(CLAIMS.read_text())["claims"]


def claim_row(ident: str) -> dict:
    for row in claims():
        if row["id"] == ident:
            return row
    raise SystemExit(f"unknown claim {ident}")


def module_for(row: dict):
    return builder().claim_module(row)


def newest(rows: list[dict], ident: str, environment: str, root: Path):
    candidates = [row for row in rows
                  if row.get("claim_id") == ident and row.get("environment") == environment
                  and Path(row.get("_path", "")).is_relative_to(root)]
    if not candidates:
        return None
    return max(candidates, key=lambda row: str(row.get("finished_at") or ""))


def statuses(roots=(EVIDENCE,)) -> dict:
    """Per claim and environment: the newest record's verdict, from its own predicate."""
    derive = builder().derive_claim
    out = {}
    for row in claims():
        derived = derive(row, roots)
        out[row["id"]] = derived
    return out


def cmd_list(args) -> int:
    rows = builder().claim_records((EVIDENCE, SCRATCH))
    committed = statuses((EVIDENCE,))
    scratch = statuses((EVIDENCE, SCRATCH))
    print(f"{'claim':6} {'local':14} {'public':14}  test")
    ready = True
    for row in claims():
        idents = row["id"]
        cells = []
        for environment in ENVIRONMENTS:
            entry = committed[idents][environment]
            state = entry.get("status") or "not run"
            scratch_entry = scratch[idents][environment]
            if (scratch_entry.get("record") or "").startswith("target/"):
                state += "*"
            if state != "pass":
                ready = False
            pin = (entry.get("source_sha") or "")[:7]
            cells.append(f"{state}{('@' + pin) if pin and state.startswith('pass') else ''}")
        print(f"{idents:6} {cells[0]:14} {cells[1]:14}  {row['test']}")
    print(f"\n{'RELEASE_CLAIMS_READY' if ready else 'RELEASE_CLAIMS_NOT_READY'} "
          f"(* = a newer scratch record exists; it does not count)")
    if args.json:
        print(json.dumps({"committed": committed, "scratch": scratch,
                          "ready": ready}, indent=2, sort_keys=True))
    return 0 if ready else 3


def cmd_run(args) -> int:
    row = claim_row(args.claim)
    out = Path(args.out or (SCRATCH / args.claim / args.env))
    command = [sys.executable, str(ROOT / row["test"]), "--env", args.env,
               "--out", str(out)]
    if args.public_manifest:
        command += ["--public-manifest", args.public_manifest]
    if args.old_image:
        command += ["--old-image", args.old_image]
    if args.keep:
        command.append("--keep")
    env = dict(os.environ)
    if getattr(args, "allow_dirty", False):
        env["RELEASE_ALLOW_DIRTY"] = "1"
    result = subprocess.run(command, cwd=ROOT, text=True, env=env)
    print(f"record: {out / 'claim.json'}")
    return result.returncode


def cmd_check(args) -> int:
    row = claim_row(args.claim)
    path = Path(args.record)
    record_path = path / "claim.json" if path.is_dir() else path
    record = json.loads(record_path.read_text())
    verdict, reasons = module_for(row).check(record)
    print(json.dumps({"claim": row["id"], "verdict": verdict, "reasons": reasons,
                      "record": str(record_path)}, indent=2, sort_keys=True))
    return {"pass": 0, "fail": 1, "not run": 3}.get(verdict, 1)


def cmd_record(args) -> int:
    row = claim_row(args.claim)
    run_dir = Path(args.record)
    record_path = run_dir / "claim.json" if run_dir.is_dir() else run_dir
    record = json.loads(record_path.read_text())
    verdict, reasons = module_for(row).check(record)
    # A `not run` record is packed as INCONCLUSIVE: the reason it could not run is itself a
    # retained observation, and the tracker must be able to show which precondition blocked it.
    evidence_verdict = {"pass": "PASS", "fail": "FAIL"}.get(verdict, "INCONCLUSIVE")
    provenance = record.get("provenance") or {}
    destination = Path(args.into) if args.into else (
        EVIDENCE / f"{row['id'].lower().replace('-', '')}-{args.env}-"
                  f"{(record.get('finished_at') or '')[:10].replace('-', '')}-"
                  f"{(provenance.get('source_sha') or '0000000')[:7]}")
    if destination.exists():
        print(json.dumps({"result": "REFUSED", "error": f"{destination} already exists"},
                         sort_keys=True))
        return 2
    command = [sys.executable, str(ROOT / "tools" / "evidence-pack.py"), "pack",
               "--run-dir", str(run_dir if run_dir.is_dir() else run_dir.parent),
               "--record", str(destination), "--gate-id", row["id"],
               "--verdict", evidence_verdict,
               "--locality", record.get("locality") or "local-devnet"]
    if provenance.get("source_sha"):
        command += ["--source-sha", provenance["source_sha"]]
    if provenance.get("binary_sha256"):
        command += ["--binary-sha256", provenance["binary_sha256"]]
    result = subprocess.run(command, cwd=ROOT, text=True)
    if result.returncode:
        return result.returncode
    print(f"recorded: {destination} ({verdict})")
    return 0


def cmd_assert_ready(args) -> int:
    current = statuses((EVIDENCE,))
    not_ready = []
    for row in claims():
        for environment in ENVIRONMENTS:
            entry = current[row["id"]][environment]
            if (entry.get("status") or "not run") != "pass":
                not_ready.append(f"{row['id']}/{environment}={entry.get('status') or 'not run'}")
    token = "RELEASE_CLAIMS_NOT_READY" if not_ready else "RELEASE_CLAIMS_READY"
    print(token)
    for line in not_ready:
        print(f"  {line}")
    return 3 if not_ready else 0


def main(argv: list[str]) -> int:
    if "--assert-ready" in argv:
        return cmd_assert_ready(argparse.Namespace())
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    listing = sub.add_parser("list", help="show the derived statuses")
    listing.add_argument("--json", action="store_true")
    listing.set_defaults(func=cmd_list)
    run = sub.add_parser("run", help="run one claim's test")
    run.add_argument("--claim", required=True)
    run.add_argument("--env", choices=ENVIRONMENTS, required=True)
    run.add_argument("--out")
    run.add_argument("--public-manifest")
    run.add_argument("--old-image")
    run.add_argument("--keep", action="store_true")
    run.add_argument("--allow-dirty", action="store_true",
                     help="the image is known to contain this working tree")
    run.set_defaults(func=cmd_run)
    check = sub.add_parser("check", help="re-run a record's predicate")
    check.add_argument("--claim", required=True)
    check.add_argument("--record", required=True)
    check.set_defaults(func=cmd_check)
    record = sub.add_parser("record", help="pack a run into an evidence record")
    record.add_argument("--claim", required=True)
    record.add_argument("--env", choices=ENVIRONMENTS, required=True)
    record.add_argument("--record", required=True)
    record.add_argument("--into")
    record.set_defaults(func=cmd_record)
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
