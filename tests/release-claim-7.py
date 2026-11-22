#!/usr/bin/env python3
"""RC-7 — upgrade compatibility: populated-store migration, downgrade refusal, mixed fleet.

Test: `python3 tools/release-check.py run --claim RC-7 --env local [--old-image rnode:old]`

Three arms:

1. the G520 populated-store copy/rollback/restart drill;
2. a store stamped with a store-format version this build does not know must be refused;
3. two binaries in one network: the block produced during the window must have the same state
   triples on both, or the old node must refuse on the consensus path; after the swap the upgraded
   node must reach the same finalised head.

The mixed arm needs a separately built prior image. Without one it is `not run`, not a fabricated
pass; the other two arms still run and are recorded.
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location(
    "release_claim_common", ROOT / "tests" / "release_claim_common.py")
common = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(common)

CLAIM_ID = "RC-7"
TITLE = "Upgrade compatibility"
STORE_FORMAT_VERSION = 4  # casper/src/storage.rs; the test stamps one higher


def _fixture_baseline(arm: str = "local") -> dict:
    environment = "public" if arm == "public" else "local"
    return {
        "schema": 1, "claim_id": CLAIM_ID, "claim_title": TITLE, "arm": arm,
        "environment": environment,
        "locality": "public-live" if environment == "public" else "local-devnet",
        "provenance": {"source_sha": "0" * 40, "binary_sha256": "1" * 64},
        "preconditions": [], "verdict": "pass", "reasons": [], "errors": [],
        "harness": {"name": "populated-store-migration-drill", "exit_code": 0},
        "observations": {
            "migration_result": "PASS",
            "downgrade": {"refused": True, "stderr": "written by a newer build (store format 5"},
            "mixed": {
                "old_image": "rnode:old", "old_binary_sha256": "2" * 64,
                "new_binary_sha256": "3" * 64,
                "agreement": True, "refusal": None,
                "upgrade": {"heads_agree": True, "marker_present": True},
            },
            "format_traces": [{
                "kind": "format", "stage": "probe", "present": str(STORE_FORMAT_VERSION + 1),
                "build": STORE_FORMAT_VERSION, "on_disk": STORE_FORMAT_VERSION + 1,
                "decision": "refuse_newer", "stamped": False, "durable": False,
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
    if observations.get("migration_result") != "PASS":
        problems.append(f"the populated-store migration drill returned "
                        f"{observations.get('migration_result')!r}")
    downgrade = observations.get("downgrade")
    if not isinstance(downgrade, dict) or downgrade.get("refused") is not True:
        problems.append("a store written by a newer build was not refused")
    mixed = observations.get("mixed")
    if not isinstance(mixed, dict):
        problems.append("mixed: missing or not an object")
        return "fail", problems
    if mixed.get("old_binary_sha256") == mixed.get("new_binary_sha256"):
        problems.append("the mixed arm did not run two distinct binaries")
    if mixed.get("agreement") is not True and not mixed.get("refusal"):
        problems.append("the mixed fleet neither agreed on the block nor refused on the consensus path")
    upgrade = mixed.get("upgrade") or {}
    if upgrade.get("heads_agree") is not True:
        problems.append("the upgraded node did not reach the same finalised head")
    if upgrade.get("marker_present") is not True:
        problems.append("the marker is absent after the upgrade")
    problems.extend(common.phase_space_reading(observations, bool(problems)))
    return common.evaluate(record, problems)


FIXTURES = {
    "local": lambda: _fixture_baseline("local"),
    "public": lambda: _fixture_baseline("public"),
}
ARMS = {
    "local": _check_baseline,
    "public": _check_baseline,
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
    parser.add_argument("--keep", action="store_true")
    parser.add_argument("--old-image", default=os.environ.get("RNODE_OLD_IMAGE"))
    args = parser.parse_args(argv)
    arm = args.arm or args.env or "local"
    meta = common.claim_arm(CLAIM_ID, arm)
    environment = meta["environment"]
    record = common.new_record(CLAIM_ID, TITLE, environment, meta["locality"], arm=arm)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    failure = common.preflight_failure(CLAIM_ID, environment, out, record,
                                       getattr(args, "public_manifest", None))
    if failure is not None:
        return failure

    if environment == "public":
        manifest = common.load_manifest(args.public_manifest)
        if not manifest:
            return common.not_run(out, record, "no --public-manifest for the public fleet")[1]
        supplied = manifest.get("upgrade_record")
        if not supplied:
            return common.not_run(
                out, record,
                "the public run needs the operator's migration and mixed-version record")[1]
        observations = json.loads(Path(supplied).read_text())
        record["observations"] = observations
        record["provenance"] = manifest.get("provenance") or {}
        verdict, reasons = check(record)
        common.finish(record, verdict, reasons)
        return common.write_record(out, record)[1]

    try:
        record["provenance"] = _provenance()
        _migration(record, out)
        _downgrade(record, out)
        if not args.old_image:
            record["preconditions"].append(
                "no --old-image/RNODE_OLD_IMAGE: the mixed-version arm needs a separately built "
                "prior image")
            common.finish(record, "not run", list(record["preconditions"]))
            return common.write_record(out, record)[1]
        _mixed(record, out, args.old_image, args.keep)
    except Exception as error:  # noqa: BLE001
        common.finish(record, "fail", [f"{type(error).__name__}: {error}"])
        return common.write_record(out, record)[1]
    verdict, reasons = check(record)
    common.finish(record, verdict, reasons)
    return common.write_record(out, record)[1]


def _provenance() -> dict:
    def run(*args: str):
        try:
            return subprocess.check_output(args, cwd=ROOT, text=True,
                                           stderr=subprocess.DEVNULL).strip()
        except Exception:
            return None
    current = os.environ.get("RNODE_IMAGE", "rnode:local")
    image = run("docker", "inspect", "-f", "{{.Id}}", current)
    binary = run("docker", "run", "--rm", "--entrypoint", "sha256sum", current,
                 "/usr/local/bin/rnode")
    return {"source_sha": run("git", "rev-parse", "HEAD"),
            "source_dirty": bool(run("git", "status", "--porcelain")),
            "binary_sha256": binary.split()[0] if binary else None,
            "image_id": image, "profile": "local-4", "devnet_prefix": "rc7"}


def _migration(record: dict, out: Path) -> None:
    migration_out = out / "migration"
    command = [sys.executable, str(ROOT / "tools" / "populated-store-migration-drill.py"),
               "--gate", "--out", str(migration_out)]
    result = subprocess.run(command, cwd=ROOT, text=True, capture_output=True, timeout=3600)
    (out / "migration.log").write_text(result.stdout + result.stderr)
    doc = _read_json(migration_out / "result.json") or {}
    record["harness"] = {"name": "populated-store-migration-drill", "command": command,
                         "exit_code": result.returncode, "out": str(migration_out)}
    record["observations"]["migration_result"] = doc.get("result") or (
        "PASS" if result.returncode == 0 else "FAIL")
    record["observations"]["migration_reasons"] = doc.get("reasons") or []


def _downgrade(record: dict, out: Path) -> None:
    with tempfile.TemporaryDirectory(prefix="rc7-store-") as scratch:
        (Path(scratch) / "store-format").write_text(f"{STORE_FORMAT_VERSION + 1}\n")
        trace_dir = Path(scratch) / "traces"
        trace_dir.mkdir(mode=0o777, exist_ok=True)
        # The image entrypoint is `rnode`; `--profile` is a top-level option before the
        # subcommand. Repeating the binary name makes the CLI read it as a subcommand.
        command = ["docker", "run", "--rm", "-v", f"{scratch}:/data",
                   "-e", "RNODE_TRACE_DIR=/data/traces",
                   os.environ.get("RNODE_IMAGE", "rnode:local"),
                   "--profile", "docker", "run", "--data-dir", "/data"]
        try:
            result = subprocess.run(command, cwd=ROOT, text=True, capture_output=True,
                                    timeout=120)
            stderr = result.stdout + result.stderr
        except subprocess.TimeoutExpired as error:
            stderr = (error.stdout or "") + (error.stderr or "")
        rows = record["observations"].setdefault("format_traces", [])
        for path in sorted(trace_dir.glob("format-*.json")):
            try:
                rows.append({"file": path.name, **json.loads(path.read_text())})
            except ValueError:
                continue
    (out / "downgrade.log").write_text(stderr)
    expected = f"written by a newer build (store format {STORE_FORMAT_VERSION + 1}"
    record["observations"]["downgrade"] = {
        "refused": expected in stderr,
        "stderr": stderr[-2000:],
        "command": command,
    }


def _mixed(record: dict, out: Path, old_image: str, keep: bool) -> None:
    devnet = common.Devnet("rc7mix", validators=4, http_base=44403, admin_base=44405,
                           grpc_base=44402, node_images={3: old_image})
    try:
        devnet.up()
        _collect_format_traces(devnet, out / "traces" / "mixed", record)
        target = devnet.target()
        marker = "release-claim:" + str(time.time_ns())
        value = common.MARKER_VALUE
        deploy_id = devnet.deploy(f'@{json.dumps(marker)}!({value})')
        common.wait_until(
            "the mixed fleet finalises the marker on every validator",
            lambda: all(common.marker_present(
                target.probe(i, marker, target.finalized_head(i)[1]), value)
                for i in range(4)), timeout=360)
        height, block_hash = target.finalized_head(0)
        old_sha = _binary_sha(devnet.node(3))
        new_sha = _binary_sha(devnet.node(0))
        agreement, refusal = False, None
        try:
            old_info = target.block_info(3, block_hash)
            new_info = target.block_info(0, block_hash)
            triple = lambda row: (row.get("preStateHash"), row.get("postStateHash"),
                                  tuple(row.get("rejectedDeploys") or []))
            agreement = triple(old_info) == triple(new_info)
            record["observations"]["mixed_block"] = {
                "height": height, "hash": block_hash,
                "old": triple(old_info), "new": triple(new_info)}
        except Exception as error:  # noqa: BLE001
            refusal = f"{type(error).__name__}: {error}"
        logs = subprocess.run(["docker", "logs", devnet.node(3)], text=True,
                              capture_output=True, timeout=60)
        lines = [line for line in (logs.stdout + logs.stderr).splitlines()
                 if any(word in line.lower() for word in
                        ("reject", "refus", "fork", "equivocat", "incompatible"))]
        if lines and not agreement:
            refusal = "\n".join(lines[-20:])
        record["observations"]["mixed"] = {
            "old_image": old_image, "old_binary_sha256": old_sha,
            "new_binary_sha256": new_sha, "deploy_id": deploy_id,
            "agreement": agreement, "refusal": refusal,
            "upgrade": {"heads_agree": False, "marker_present": False},
        }
        # Rolling swap: recreate validator 3 on the current image, keeping its store.
        subprocess.run(["docker", "rm", "-f", devnet.node(3)], text=True,
                       capture_output=True, timeout=120)
        upgraded = common.Devnet("rc7mix", validators=4, http_base=44403, admin_base=44405,
                                 grpc_base=44402)
        result = upgraded.shell("up", "--validators", "4", timeout=900)
        (out / "upgrade.log").write_text(result.stdout + result.stderr)
        common.wait_until(
            "every validator answers after the swap",
            lambda: all(upgraded._ready(i) for i in range(4)), timeout=360)
        upgraded_target = upgraded.target()
        common.wait_until(
            "the upgraded node reaches the same finalised head",
            lambda: len({upgraded_target.finalized_head(i) for i in range(4)}) == 1,
            timeout=600)
        _collect_format_traces(upgraded, out / "traces" / "upgraded", record)
        # The upgraded node reaches the head as soon as its store opens; the state read at that
        # head can lag the finality read. Give the marker a bounded window rather than a single
        # snapshot (the same race the RC-1 finalised-head read has).
        deadline = time.monotonic() + 120
        while True:
            head_height, head_hash = upgraded_target.finalized_head(3)
            present = common.marker_present(upgraded_target.probe(3, marker, head_hash), value)
            if present or time.monotonic() >= deadline:
                break
            time.sleep(3)
        record["observations"]["mixed"]["upgrade"] = {
            "heads_agree": len({upgraded_target.finalized_head(i) for i in range(4)}) == 1,
            "marker_present": present, "height": head_height, "hash": head_hash}
    finally:
        if not keep:
            devnet.down()


def _collect_format_traces(devnet, dest: Path, record: dict) -> None:
    """Copy each node's `FormatTrace` files before its container is torn down."""
    rows = record["observations"].setdefault("format_traces", [])
    for i in range(4):
        node_dest = dest / f"node-{i}"
        try:
            devnet.copy_traces(i, node_dest)
        except Exception:  # noqa: BLE001 - an old image may not write traces
            continue
        for path in sorted(node_dest.glob("format-*.json")):
            try:
                rows.append({"validator": i, "file": path.name, **json.loads(path.read_text())})
            except ValueError:
                continue


def _binary_sha(container: str):
    try:
        return subprocess.check_output(
            ["docker", "exec", container, "sha256sum", "/usr/local/bin/rnode"],
            text=True, stderr=subprocess.DEVNULL).split()[0]
    except Exception:
        return None


def _read_json(path: Path):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return None


if __name__ == "__main__":
    raise SystemExit(run(sys.argv[1:]))
