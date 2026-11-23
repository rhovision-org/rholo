#!/usr/bin/env python3
"""RC-6 — join and audit: a restored node reaches the chain and audit reproduces its claim.

Test: `python3 tools/release-check.py run --claim RC-6 --env local`

The mature-join arm wipes validator 3's store and restarts it from a bootstrap; the six-stage chain
is correlated from the two logs. `rnode audit` then replays the joiner's anchor from genesis and must
return AGREE with the same derived root the certificate claimed. For a genesis anchor there is no
certificate digest to compare, and the claim says so.
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location(
    "release_claim_common", ROOT / "tests" / "release_claim_common.py")
common = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(common)

CLAIM_ID = "RC-6"
TITLE = "Join and audit"


def _fixture_baseline(arm: str = "local") -> dict:
    environment = "public" if arm == "public" else "local"
    return {
        "schema": 1, "claim_id": CLAIM_ID, "claim_title": TITLE, "arm": arm,
        "environment": environment,
        "locality": "public-live" if environment == "public" else "local-devnet",
        "provenance": {"source_sha": "0" * 40, "binary_sha256": "1" * 64},
        "preconditions": [], "verdict": "pass", "reasons": [], "errors": [],
        "harness": {"name": "n457-mature-join-arm", "exit_code": 0},
        "observations": {
            "join_result": "pass",
            "chain_problems": [],
            "joiner_checkpoint": {"kind": "checkpoint", "digest": "ab" * 32,
                                  "anchor": "cd" * 32, "anchorHeight": 28},
            "audit": {"exit": 0, "verdict": "AGREE", "checkpointDigest": "ab" * 32,
                      "claimedRoot": "ef" * 32, "derivedRoot": "ef" * 32, "blocks": 29},
            "digest_matches": True,
            "derived_root_matches": True,
            "audit_traces": [{
                "kind": "audit", "digest": "ab" * 32,
                "anchor": "cd" * 32, "anchor_in_fringe": True,
                "target_hashes": ["cd" * 32], "target_state": "ef" * 32,
                "anchor_post_state": "ef" * 32, "verdict": "AGREE",
                "derived_root": "ef" * 32, "claimed_root": "ef" * 32,
            }],
        },
    }


def _check_baseline(record: dict) -> tuple[str, list[str]]:
    if record.get("verdict") == "not run":
        return "not run", list(record.get("preconditions") or record.get("reasons") or [])
    problems: list[str] = []
    observations = record.get("observations")
    if not isinstance(observations, dict):
        return "fail", ["observations: missing or not an object"]
    if observations.get("join_result") != "pass":
        problems.append(f"the mature join did not pass: {observations.get('join_result')!r}")
    if observations.get("chain_problems"):
        problems.append(f"the six-stage chain has problems: {observations['chain_problems']}")
    audit = observations.get("audit")
    if not isinstance(audit, dict):
        problems.append("audit: missing or not an object")
    else:
        if audit.get("exit") != 0:
            problems.append(f"rnode audit exited {audit.get('exit')}: {audit.get('error')}")
        if audit.get("verdict") != "AGREE":
            detail = f": {audit['reason']}" if audit.get("reason") else ""
            problems.append(f"rnode audit verdict is {audit.get('verdict')!r}, not AGREE{detail}")
        if audit.get("derivedRoot") != audit.get("claimedRoot"):
            problems.append("the audit's derived root differs from the claim")
        if audit.get("blocks") in (0, None):
            problems.append("the audit consumed no bodies")
    checkpoint = observations.get("joiner_checkpoint") or {}
    if checkpoint.get("kind") == "checkpoint":
        if observations.get("digest_matches") is not True:
            problems.append("the audit's digest does not equal the joiner's accepted digest")
    problems.extend(common.phase_space_reading(observations, bool(problems)))
    return common.evaluate(record, problems)


FIXTURES = {
    "local": lambda: _fixture_baseline("local"),
    "public": lambda: _fixture_baseline("public"),
    "off-host": lambda: common.offhost_fixture(CLAIM_ID, TITLE),
    "availability": lambda: common.availability_fixture(CLAIM_ID, TITLE),
}
ARMS = {
    "local": _check_baseline,
    "public": _check_baseline,
    "off-host": common.check_offhost,
    "availability": common.check_availability,
}


def fixture(arm: str = "local") -> dict:
    return FIXTURES[arm]()


def check(record: dict) -> tuple[str, list[str]]:
    if record.get("verdict") == "not run":
        return "not run", list(record.get("preconditions") or record.get("reasons") or [])
    return common.dispatch_arm(ARMS, record)


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--env", choices=("local", "public"))
    parser.add_argument("--arm")
    parser.add_argument("--out", required=True)
    parser.add_argument("--public-manifest")
    parser.add_argument("--campaign-result")
    parser.add_argument("--head", help="the source SHA the operator run pins")
    parser.add_argument("--keep", action="store_true")
    parser.add_argument("--min-height", type=int, default=28)
    args = parser.parse_args(argv)
    arm = args.arm or args.env or "local"
    meta = common.claim_arm(CLAIM_ID, arm)
    environment = meta["environment"]
    record = common.new_record(CLAIM_ID, TITLE, environment, meta["locality"], arm=arm)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    if arm == "off-host":
        if not args.campaign_result:
            return common.not_run(
                out, record, "the off-host arm needs --campaign-result <M3 run dir>")[1]
        return common.run_offhost_arm(out, record, args.campaign_result, head=args.head)
    if arm not in ("local", "public"):
        return common.not_run(
            out, record,
            f"the {arm} arm's harness lands in {meta['next_pr']}")[1]
    failure = common.preflight_failure(CLAIM_ID, environment, out, record,
                                       getattr(args, "public_manifest", None))
    if failure is not None:
        return failure

    if environment == "public":
        manifest = common.load_manifest(args.public_manifest)
        if not manifest:
            return common.not_run(out, record, "no --public-manifest for the public fleet")[1]
        target = common.public_target(manifest)
        record["provenance"] = manifest.get("provenance") or {}
        record["observations"]["join_result"] = manifest.get("join_result")
        if manifest.get("join_result") != "pass":
            return common.not_run(
                out, record,
                "the public run needs the operator's completed join record")[1]
        return _audit(target, record, out, manifest.get("audit_bootstrap"))

    # The mature-join arm replays a chain that keeps producing during the sync; the node's
    # preflight floor scales with persisted blocks, so this arm gets 4 GiB per validator.
    devnet = common.Devnet("n457join", validators=4, http_base=55403,
                           admin_base=55405, grpc_base=55402, network="n457net",
                           memory="4g", extra_up=("--epoch-length", "10"))
    try:
        devnet.up()
        record["provenance"] = devnet.provenance()
        join_out = out / "join"
        command = [sys.executable, str(ROOT / "tools" / "n457-mature-join-arm.py"),
                   "--supported-recovery", "--min-height", str(args.min_height),
                   "--timeout", "900", "--out", str(join_out)]
        result = subprocess.run(command, cwd=ROOT, env=devnet.env(), text=True,
                                capture_output=True, timeout=1800)
        (out / "join.log").write_text(result.stdout + result.stderr)
        record["harness"] = {"name": "n457-mature-join-arm", "command": command,
                             "exit_code": result.returncode, "out": str(join_out)}
        record["observations"]["join_result"] = "pass" if result.returncode == 0 else "fail"
        if result.returncode:
            record["observations"]["join_error"] = (result.stdout + result.stderr)[-1500:]
            common.finish(record, "fail", [record["observations"]["join_error"]])
            return common.write_record(out, record)[1]
        chain_path = join_out / "chain.json"
        chain = subprocess.run(
            [sys.executable, str(ROOT / "tools" / "m3-join-chain.py"),
             "check-chain", "--chain", str(chain_path)],
            cwd=ROOT, text=True, capture_output=True, timeout=120)
        (out / "chain.log").write_text(chain.stdout + chain.stderr)
        record["observations"]["chain_problems"] = (
            [] if chain.returncode == 0 else (chain.stdout + chain.stderr).strip().splitlines())
        # The joiner synced at a checkpoint and does not hold the pre-anchor bodies, so the replay
        # must read the certificate and history from the bootstrap, not from the joiner's own API.
        return _audit(devnet.target(), record, out, f"http://{devnet.node(0)}:40403",
                      joiner_index=3, container=devnet.node(3))
    except Exception as error:  # noqa: BLE001
        common.finish(record, "fail", [f"{type(error).__name__}: {error}"])
        return common.write_record(out, record)[1]
    finally:
        if not args.keep:
            devnet.down()


def _audit(target, record: dict, out: Path, public_bootstrap, joiner_index: int = 3,
           container: str | None = None) -> int:
    observations = record["observations"]
    try:
        status = target.get(joiner_index, "/api/v1/status/operator", admin=True)
        checkpoint = (status or {}).get("checkpoint") or {}
        observations["joiner_checkpoint"] = {
            "kind": checkpoint.get("kind"),
            "digest": checkpoint.get("digest"),
            "anchor": checkpoint.get("anchor"),
            "anchorHeight": checkpoint.get("anchorHeight"),
        }
        bootstrap = public_bootstrap or "http://127.0.0.1:40403"
        if container:
            # The audit runs inside the joiner; its scratch paths are container paths and the
            # report is copied out, not read through the host's temp directory.
            data_dir = "/tmp/rc6-audit-data"
            audit_out = "/tmp/rc6-audit-out"
            trace_dir = "/tmp/rc6-audit-traces"
            command = ["rnode", "audit", "--bootstrap", bootstrap,
                       "--data-dir", data_dir, "--out", audit_out,
                       # The replay must use the audited chain's genesis, not defaults.conf.
                       "--bonds-file", "/genesis/bonds.txt",
                       "--wallets-file", "/genesis/wallets.txt",
                       "--dev-mode", "--epoch-length", "10"]
            if checkpoint.get("kind") == "checkpoint" and checkpoint.get("digest"):
                command += ["--checkpoint", checkpoint["digest"]]
            subprocess.run(
                ["docker", "exec", container, "rm", "-rf", data_dir, audit_out, trace_dir],
                text=True, capture_output=True, timeout=60)
            result = subprocess.run(
                ["docker", "exec", "-e", f"RNODE_TRACE_DIR={trace_dir}", container, *command],
                cwd=ROOT, text=True, capture_output=True, timeout=900)
            trace_host = out / "traces" / "audit"
            subprocess.run(["docker", "cp", f"{container}:{trace_dir}/.", str(trace_host)],
                           text=True, capture_output=True, timeout=60)
            _collect_audit_traces(trace_host, observations)
            (out / "audit.log").write_text(result.stdout + result.stderr)
            report_host = out / "audit.json"
            copy = subprocess.run(
                ["docker", "cp", f"{container}:{audit_out}/audit.json", str(report_host)],
                text=True, capture_output=True, timeout=60)
            report = _read_json(report_host) if copy.returncode == 0 else {}
        else:
            with tempfile.TemporaryDirectory(prefix="rc6-audit-") as scratch:
                command = ["rnode", "audit", "--bootstrap", bootstrap,
                           "--data-dir", str(Path(scratch) / "data"),
                           "--out", str(Path(scratch) / "out")]
                if checkpoint.get("kind") == "checkpoint" and checkpoint.get("digest"):
                    command += ["--checkpoint", checkpoint["digest"]]
                trace_dir = Path(scratch) / "traces"
                env = {**os.environ, "RNODE_TRACE_DIR": str(trace_dir)}
                result = subprocess.run(command, cwd=ROOT, text=True, env=env,
                                        capture_output=True, timeout=900)
                (out / "audit.log").write_text(result.stdout + result.stderr)
                report = _read_json(Path(scratch) / "out" / "audit.json") or {}
                _collect_audit_traces(trace_dir, observations)
        text = result.stdout + result.stderr
        named = [line for line in text.splitlines()
                 if line.startswith("Configuration error")
                 or line.lower().startswith("audit failed")
                 or "error:" in line.lower()]
        observations["audit"] = {
            "exit": result.returncode,
            "verdict": report.get("verdict"),
            "checkpointDigest": report.get("checkpointDigest"),
            "claimedRoot": report.get("claimedRoot"),
            "derivedRoot": report.get("derivedRoot"),
            "blocks": report.get("blocks"),
            "reason": report.get("reason"),
            "error": None if result.returncode == 0 else
            ("\n".join(named[:5]) if named else text[:800]),
        }
        observations["digest_matches"] = (
            observations["audit"].get("checkpointDigest") == observations["joiner_checkpoint"].get("digest")
            if observations["joiner_checkpoint"].get("kind") == "checkpoint" else None)
        observations["derived_root_matches"] = (
            observations["audit"].get("derivedRoot") is not None
            and observations["audit"].get("derivedRoot") == observations["audit"].get("claimedRoot"))
    except Exception as error:  # noqa: BLE001
        common.finish(record, "fail", [f"{type(error).__name__}: {error}"])
        return common.write_record(out, record)[1]
    verdict, reasons = check(record)
    common.finish(record, verdict, reasons)
    return common.write_record(out, record)[1]


def _collect_audit_traces(directory: Path, observations: dict) -> None:
    """Copy the audit process's `AuditTrace` files into the record."""
    rows = []
    if directory.is_dir():
        for path in sorted(directory.glob("audit-*.json")):
            try:
                row = json.loads(path.read_text())
            except ValueError:
                continue
            rows.append({"file": path.name, **row})
    observations["audit_traces"] = rows


def _read_json(path: Path):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return None


if __name__ == "__main__":
    raise SystemExit(run(sys.argv[1:]))
