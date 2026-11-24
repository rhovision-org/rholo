#!/usr/bin/env python3
"""The G655 local-devnet witness: forkless developer rewards, inactive → activated → claimed.

**The verdict engine is the part under test here.** A gate may only be promoted when every required
arm has actually passed in this run. An arm that was not run is `not-run`, not `pass` — the failure
mode this file exists to make impossible is a green verdict produced from missing measurements.

Usage:
  tools/devrewards-experiment.py self-test
  tools/devrewards-experiment.py run --image rnode:devrewards --out /tmp/g655 [--skip-build]
      [--validators 4] [--developer-share 500] [--treasury-amount 1000000]

The full programme (§7 of the #655 plan) is: production genesis → operate → inactive arm → activate →
beneficiary versioning → independent observer recomputation → accept/claim + negative arms → restart
drills → compatibility → benchmarks → verdict. `run` drives the phases it can from this CLI and
records the rest as `not-run` with the reason. The gate stays `owes` unless every arm is `pass`.
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import subprocess
import sys
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent
DEVNET = ROOT / "tools" / "devnet.sh"
INSTALL = ROOT / "tools" / "devrewards_install.py"
RENDERED = ROOT / "examples" / "devrewards" / "install.rendered.rho"

REQUIRED_ARMS = (
    "binary-recorded",
    "production-genesis",
    "finality",
    "install",
    "inactive-pay-refused",
    "activation",
    "claim-once",
    "replayed-claim-refused",
    "independent-observer-manifest",
    "conflicting-manifest-refused",
    "restart-neither-duplicates-nor-loses",
    "benchmarks",
)


def verdict(arms: dict[str, dict]) -> dict:
    """`pass` only when every required arm is `pass`; `fail` if any failed; else `not-run`."""
    missing = [arm for arm in REQUIRED_ARMS if arm not in arms]
    failed = [arm for arm in REQUIRED_ARMS if arms.get(arm, {}).get("status") == "fail"]
    not_run = {arm for arm in REQUIRED_ARMS if arms.get(arm, {}).get("status") != "pass"}
    if failed:
        return {
            "result": "fail",
            "failed": sorted(failed),
            "not_run": sorted(not_run - set(failed)),
        }
    if missing or not_run:
        return {"result": "not-run", "failed": [], "not_run": sorted(not_run | set(missing))}
    return {"result": "pass", "failed": [], "not_run": []}


def self_test() -> None:
    all_pass = {arm: {"status": "pass"} for arm in REQUIRED_ARMS}
    v = verdict(all_pass)
    assert v["result"] == "pass", v

    one_failed = dict(all_pass)
    one_failed["claim-once"] = {"status": "fail", "reason": "paid twice"}
    v = verdict(one_failed)
    assert v["result"] == "fail" and v["failed"] == ["claim-once"], v

    missing = dict(all_pass)
    del missing["independent-observer-manifest"]
    v = verdict(missing)
    assert v["result"] == "not-run" and "independent-observer-manifest" in v["not_run"], v

    skipped = dict(all_pass)
    skipped["benchmarks"] = {"status": "not-run", "reason": "not reached"}
    assert verdict(skipped)["result"] == "not-run"

    # An extra arm cannot rescue a missing required one.
    extra = dict(missing)
    extra["bonus"] = {"status": "pass"}
    assert verdict(extra)["result"] == "not-run"

    # The manifest tool's own refusals are part of this witness's surface.
    import importlib.util

    spec = importlib.util.spec_from_file_location("manifest", ROOT / "tools" / "devrewards_manifest.py")
    manifest = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(manifest)
    golden = json.loads((ROOT / "tests" / "fixtures" / "devrewards_manifest_golden.json").read_text())
    keys = [bytes.fromhex(k) for k in golden["keys"]]
    root, _ = manifest.build_tree(keys, golden["amounts"])
    assert root.hex() == golden["root"]
    assert manifest.digest(golden["epoch"], bytes.fromhex(golden["anchor_hash"]), root).hex() == golden["digest"]
    print("devrewards-experiment self-test: ok")


def run(cmd: list[str], **kwargs) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, text=True, capture_output=True, **kwargs)


def devnet(*args: str, env: dict | None = None) -> subprocess.CompletedProcess:
    return run(["bash", str(DEVNET), *args], env=env, cwd=ROOT)


def wait_for_finality(deadline: float) -> tuple[bool, str]:
    """Poll the bootstrap's last finalized block. Its absence is `not-run`, never a pass."""
    while time.time() < deadline:
        result = devnet("cli", "devnet-validator-1", "last-finalized-block")
        if result.returncode == 0 and result.stdout.strip():
            return True, result.stdout.strip()
        time.sleep(5)
    return False, "no finalized block before the deadline"


def run_gate(args: argparse.Namespace) -> int:
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    arms: dict[str, dict] = {}

    # 1. Build once and record the artefact.
    if not args.skip_build:
        built = devnet("build", env={**os.environ, "RNODE_IMAGE": args.image})
        arms["binary-recorded"] = {
            "status": "pass" if built.returncode == 0 else "fail",
            "reason": built.stderr[-500:] if built.returncode else "built",
        }
    digest = run(["docker", "image", "inspect", args.image, "--format", "{{.Id}}"])
    record = {"image": args.image, "image_id": digest.stdout.strip() if digest.returncode == 0 else ""}
    if digest.returncode != 0:
        arms.setdefault("binary-recorded", {"status": "fail", "reason": "image does not exist"})
        record["arms"] = arms
        record["verdict"] = verdict(arms)
        (out / "verdict.json").write_text(json.dumps(record, indent=2, sort_keys=True) + "\n")
        return 2

    # 2. A production genesis (no `--dev-mode`) with a non-zero reserve, funded deployer, four
    #    validators whose first two keys are the 2-of-2 reserve signers.
    keys = "04f700a417754b775d95421973bdbdadb2d23c8a5af46f1829b1431f5c136e549e8a0d61aa0c793f1a614f8e437711c7758473c6ceb0859ac7e9e07911ca66b5c4,04dbe32c2062240a4ba0bcad01d7edd98c78b51c77765d5e1e5e9fa3743d2f12a1f82f42cd7dc4f41445979117d790f23e9b3d08d0aa06d527c236172043e747fc"
    env = {**os.environ, "RNODE_IMAGE": args.image}
    up = devnet(
        "up", "--validators", str(args.validators), "--fresh", "--production-genesis",
        "--developer-share", str(args.developer_share),
        "--developer-reserve-keys", keys, "--developer-reserve-quorum", "2",
        env=env,
    )
    arms["production-genesis"] = {
        "status": "pass" if up.returncode == 0 else "fail",
        "reason": (up.stdout + up.stderr)[-800:] if up.returncode else "genesis accepted",
    }
    if up.returncode != 0:
        record["arms"] = arms
        record["verdict"] = verdict(arms)
        (out / "verdict.json").write_text(json.dumps(record, indent=2, sort_keys=True) + "\n")
        return 2

    ok, detail = wait_for_finality(time.time() + args.finality_timeout)
    arms["finality"] = {"status": "pass" if ok else "fail", "reason": detail}

    # 3. The install is an ordinary deploy. Attestors are the first two validator keys.
    render = run(
        ["python3", str(INSTALL), "--attestor", keys.split(",")[0], "--attestor", keys.split(",")[1],
         "--threshold", "2"]
    )
    if render.returncode != 0:
        arms["install"] = {"status": "fail", "reason": render.stderr[-500:]}
    else:
        RENDERED.write_text(render.stdout)
        deployed = devnet("deploy", "devrewards/install.rendered.rho")
        arms["install"] = {
            "status": "pass" if deployed.returncode == 0 else "fail",
            "reason": (deployed.stdout + deployed.stderr)[-800:],
        }

    # 4. Every arm that needs the block-event export or a second observer is recorded honestly.
    for arm in REQUIRED_ARMS:
        if arm in arms:
            continue
        arms[arm] = {
            "status": "not-run",
            "reason": (
                "the block-event export (CommEvent.consume hash) has no CLI surface in this build; "
                "the independent observer and the arms derived from it are owed a gRPC report adapter"
            ),
        }

    record["arms"] = arms
    record["verdict"] = verdict(arms)
    (out / "verdict.json").write_text(json.dumps(record, indent=2, sort_keys=True) + "\n")
    return 0 if record["verdict"]["result"] == "pass" else 1


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("self-test")
    p_run = sub.add_parser("run")
    p_run.add_argument("--image", default="rnode:devrewards")
    p_run.add_argument("--out", default="/tmp/g655")
    p_run.add_argument("--skip-build", action="store_true")
    p_run.add_argument("--validators", type=int, default=4)
    p_run.add_argument("--developer-share", type=int, default=500)
    p_run.add_argument("--treasury-amount", type=int, default=1_000_000)
    p_run.add_argument("--finality-timeout", type=int, default=300)
    args = parser.parse_args(argv)
    if args.command == "self-test":
        self_test()
        return 0
    return run_gate(args)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
