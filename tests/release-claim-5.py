#!/usr/bin/env python3
"""RC-5 — overload refusal: a configured bound refuses explicitly and loses nothing.

Test: `python3 tools/release-check.py run --claim RC-5 --env local`

Two devnet lifetimes (capped and unset) drive the M2 pressure arm; the Rust gRPC observation is the
third input; the resulting record is judged by the existing `tools/m2-pressure.py::check_pressure`.
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location(
    "release_claim_common", ROOT / "tests" / "release_claim_common.py")
common = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(common)

CLAIM_ID = "RC-5"
TITLE = "Overload refusal"


def fixture() -> dict:
    return {
        "schema": 1, "claim_id": CLAIM_ID, "claim_title": TITLE,
        "environment": "local", "locality": "local-devnet",
        "provenance": {"source_sha": "0" * 40, "binary_sha256": "1" * 64},
        "preconditions": [], "verdict": "pass", "reasons": [], "errors": [],
        "harness": {"name": "m2-pressure", "exit_code": 0},
        "observations": {
            "check_exit": 0, "verdict": "PASS",
            "pressure": "record/pressure.json", "not_processed": [],
        },
    }


def check(record: dict) -> tuple[str, list[str]]:
    if record.get("verdict") == "not run":
        return "not run", list(record.get("preconditions") or record.get("reasons") or [])
    problems: list[str] = []
    observations = record.get("observations")
    if not isinstance(observations, dict):
        return "fail", ["observations: missing or not an object"]
    if observations.get("verdict") != "PASS":
        problems.append(f"check_pressure returned {observations.get('verdict')!r}")
    if observations.get("check_exit") != 0:
        problems.append(f"the pressure check exited {observations.get('check_exit')}")
    if observations.get("not_processed"):
        problems.append(f"accepted deploy(s) not resolved: {observations['not_processed']}")
    return common.evaluate(record, problems)


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--env", choices=("local", "public"), required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--public-manifest")
    parser.add_argument("--keep", action="store_true")
    args = parser.parse_args(argv)
    record = common.new_record(CLAIM_ID, TITLE, args.env,
                               "local-devnet" if args.env == "local" else "public-live")
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)

    if args.env == "public":
        manifest = common.load_manifest(args.public_manifest)
        if not manifest:
            return common.not_run(out, record, "no --public-manifest for the public fleet")[1]
        record["provenance"] = manifest.get("provenance") or {}
        supplied = manifest.get("pressure_record")
        if not supplied:
            return common.not_run(
                out, record,
                "the public run needs the operator's capped/unset pressure record")[1]
        return _judge(Path(supplied), record, out, "public-live")

    capped = common.Devnet("m2cap", validators=1, http_base=56403, admin_base=56405,
                           grpc_base=56402, network="m2net", memory="2g",
                           node_env="RNODE_HTTP_MAX_INFLIGHT_DEPLOY=2")
    unset = common.Devnet("m2cap", validators=1, http_base=56403, admin_base=56405,
                          grpc_base=56402, network="m2net", memory="2g")
    arm = ROOT / "tools" / "m2-pressure-arm.py"
    try:
        record["provenance"] = {}
        capped.up()
        record["provenance"] = capped.provenance()
        # The criterion's candidate is the binary's own source pin, not the checkout's HEAD:
        # `rnode --version` names the commit the image was built from, and an evidence-only
        # commit can sit on top of it.
        version = subprocess.check_output(
            ["docker", "exec", "m2cap-bootstrap", "rnode", "--version"],
            text=True, stderr=subprocess.DEVNULL).strip()
        match = re.search(r"\(([0-9a-f]{40})\)", version)
        record["provenance"]["binary_source_sha"] = match.group(1) if match else None
        record["provenance"]["binary_version"] = version
        _run([sys.executable, str(arm), "phase", "--mode", "capped",
              "--out", str(out / "capped"), "--container", "m2cap-bootstrap",
              "--port", "56403", "--cap", "2"], record, out / "capped.log")
        capped.down()
        unset.up()
        _run([sys.executable, str(arm), "phase", "--mode", "unset",
              "--out", str(out / "unset"), "--container", "m2cap-bootstrap",
              "--port", "56403"], record, out / "unset.log")
        unset.down()
        _run([sys.executable, str(arm), "grpc", "--out", str(out / "grpc")],
             record, out / "grpc.log")
        _run([sys.executable, str(arm), "assemble",
              "--capped", str(out / "capped"), "--unset", str(out / "unset"),
              "--grpc", str(out / "grpc"), "--out", str(out / "record"),
              "--locality", "local-devnet", "--port", "56403",
              "--source-sha", record["provenance"].get("binary_source_sha")
              or record["provenance"].get("source_sha") or "",
              "--binary-sha256", record["provenance"].get("binary_sha256") or ""],
             record, out / "assemble.log")
        return _judge(out / "record", record, out, "local-devnet")
    except Exception as error:  # noqa: BLE001
        common.finish(record, "fail", [f"{type(error).__name__}: {error}"])
        return common.write_record(out, record)[1]
    finally:
        if not args.keep:
            capped.down()
            unset.down()


def _run(command: list[str], record: dict, log: Path) -> None:
    result = subprocess.run(command, cwd=ROOT, text=True, capture_output=True, timeout=1800)
    log.write_text(result.stdout + result.stderr)
    if result.returncode:
        raise common.ClaimError(
            f"{Path(command[1]).name} {' '.join(command[2:4])} failed ({result.returncode}): "
            f"{(result.stdout + result.stderr)[-1200:]}")


def _judge(record_dir: Path, record: dict, out: Path, locality: str) -> int:
    check_command = [sys.executable, str(ROOT / "tools" / "m2-pressure.py"),
                     "check", "--record", str(record_dir), "--locality", locality]
    result = subprocess.run(check_command, cwd=ROOT, text=True, capture_output=True, timeout=600)
    (out / "check.log").write_text(result.stdout + result.stderr)
    pressure = _read_json(record_dir / "pressure.json") or {}
    observations = record["observations"]
    observations.update({
        "check_exit": result.returncode,
        "verdict": (pressure.get("verdict") if isinstance(pressure, dict) else None)
        or ("PASS" if result.returncode == 0 else "FAIL"),
        "pressure": str(record_dir / "pressure.json"),
        "not_processed": [row for row in (pressure.get("accepted") or [])
                          if isinstance(row, dict) and row.get("status") == "NotProcessed"],
        "check_output": (result.stdout + result.stderr)[-2000:],
    })
    record["harness"] = {"name": "m2-pressure", "command": check_command,
                         "exit_code": result.returncode, "out": str(record_dir)}
    verdict, reasons = check(record)
    common.finish(record, verdict, reasons)
    return common.write_record(out, record)[1]


def _read_json(path: Path):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return None


if __name__ == "__main__":
    raise SystemExit(run(sys.argv[1:]))
