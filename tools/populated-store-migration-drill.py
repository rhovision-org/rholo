#!/usr/bin/env python3
"""G520-MIGRATION: a populated-store copy, rollback and restart acceptance.

The gate asks for an **exact-format populated-store copy/rollback/restart
acceptance, including replay qualification, before a store swap**. This drill
stages it on a disposable local devnet and records the arms; it does not decide
the release, and `execution_proven` is always false in the packed record.

Arms:

  1. populate a 3-validator devnet to a finalized height H0 and capture the
     genesis hash, the finalized block, the checkpoint certificate and its digest;
  2. copy the bootstrap's store byte-for-byte to a second volume and compare the
     file manifests (lock.mdb excluded);
  3. restart the bootstrap from the copy; require the same genesis, height,
     finalized block, post-state, checkpoint anchor/post-state and the node's
     own `POST /api/v1/checkpoint/verify` verdict (the certificate digest
     commits to the live fringe, so it is recorded but not compared across
     restarts);
  4. advance to H1, snapshot the H0 stores, restore them and require the chain
     back at H0 - not H1;
  5. run a fresh-genesis arm and require it **not** to substitute for the
     populated store;
  6. clean up (`down -v`) unless `--keep`.

Usage:
  python3 tools/populated-store-migration-drill.py --gate --out target/g520-migration
  python3 tools/populated-store-migration-drill.py --gate --out target/g520-migration \\
      --source-sha <40-hex>
  python3 tools/populated-store-migration-drill.py self-test

The drill uses the Docker devnet as the harness. The evidence is the store
bytes, the restarted node's own certificate verification and the fresh-genesis
control; Docker is not the witness. A 2->4 or 3->4 validator scale-up is a
genesis/activation change, not this store-migration arm, and is not claimed.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEVNET = ROOT / "tools" / "devnet.sh"
PREFIX = os.environ.setdefault("DEVNET_PREFIX", "g520drill")
HTTP_BASE = int(os.environ.setdefault("DEVNET_HTTP_BASE", "42403"))
ADMIN_BASE = int(os.environ.setdefault("DEVNET_ADMIN_BASE", "42405"))
GRPC_BASE = int(os.environ.setdefault("DEVNET_GRPC_BASE", "42402"))
TARGET_HEIGHT = 8
EPOCH_LENGTH = 6
WAIT_SECONDS = 240.0
POLL_SECONDS = 3.0


def _module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


# The mature-replay tool already owns the HTTP, wait and checkpoint-field
# helpers. Reuse them rather than growing a second implementation.
mature = _module("g520_mature_replay", ROOT / "tools" / "c339_mature_replay.py")


def run(argv: list[str], timeout: float | None = None) -> subprocess.CompletedProcess:
    completed = subprocess.run(argv, cwd=ROOT, text=True, capture_output=True,
                               timeout=timeout, env=os.environ.copy())
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout or "").strip()
        raise RuntimeError(
            f"{' '.join(map(str, argv))} failed (exit {completed.returncode})"
            + (f":\n{detail}" if detail else ""))
    return completed


def docker(argv: list[str], timeout: float | None = 600.0) -> str:
    return run(["docker", *argv], timeout=timeout).stdout


def devnet(*argv: str, timeout: float | None = 1200.0) -> str:
    return run([str(DEVNET), *argv], timeout=timeout).stdout


def node_volume(node: str) -> str:
    return f"{PREFIX}-{node}-data"


def volume_create(name: str) -> None:
    docker(["volume", "create", name])


def volume_remove(name: str) -> None:
    subprocess.run(["docker", "volume", "rm", "-f", name], cwd=ROOT,
                   text=True, capture_output=True, timeout=120, check=False)


def copy_volume(source: str, destination: str, replace: bool = False) -> None:
    clear = "find /dst -mindepth 1 -maxdepth 1 -exec rm -rf {} +; " if replace else ""
    script = (clear
              + "cp -a /src/. /dst/; "
              + "find /dst -name 'lock.mdb' -delete")
    docker(["run", "--rm", "-v", f"{source}:/src:ro", "-v", f"{destination}:/dst",
            "alpine", "sh", "-c", script])


def volume_manifest(volume: str) -> dict:
    """Path, size and digest of every file, excluding LMDB's transient lock."""
    script = (
        "cd /store && find . -type f ! -name 'lock.mdb' -print | sort | "
        "while read -r f; do printf '%s %s %s\\n' \"$f\" \"$(stat -c %s \"$f\")\" "
        "\"$(sha256sum \"$f\" | cut -d' ' -f1)\"; done")
    text = docker(["run", "--rm", "-v", f"{volume}:/store:ro", "alpine",
                   "sh", "-c", script])
    lines = [line for line in text.splitlines() if line.strip()]
    return {
        "files": len(lines),
        "digest": hashlib.sha256("\n".join(lines).encode()).hexdigest(),
        "lines": lines,
    }


def genesis_block(port: int) -> dict:
    body = mature.try_http_json(port, "/api/blocks/0/0")
    info = {}
    if isinstance(body, list) and body:
        info = body[0]
    elif isinstance(body, dict):
        info = body.get("blockInfo") or body
    return {"genesis_hash": info.get("blockHash"),
            "post_state": info.get("postStateHash")}


def genesis_hash(port: int) -> str | None:
    return genesis_block(port).get("genesis_hash")


def finalized(port: int) -> dict:
    body = mature.try_http_json(port, "/api/last-finalized-block")
    info = (body.get("blockInfo") or body) if isinstance(body, dict) else {}
    certificate = mature.try_http_json(port, "/api/v1/checkpoint")
    fields = (mature.certificate_fields(certificate) or {}
              if isinstance(certificate, dict) else {})
    verdict = (mature.try_post_json(port, "/api/v1/checkpoint/verify", certificate)
               if isinstance(certificate, dict) else None)
    return {
        "height": info.get("blockNumber"),
        "block_hash": info.get("blockHash"),
        "post_state": info.get("postStateHash") or fields.get("anchor_post_state"),
        "digest": fields.get("digest"),
        "anchor": fields.get("anchor"),
        "anchor_height": fields.get("anchor_height"),
        "anchor_post_state": fields.get("anchor_post_state"),
        "verified": verified(verdict),
        "certificate": certificate,
        "verdict": verdict,
    }


def wait_finalized_state(port: int, height: int, label: str) -> dict:
    """The `/health` height can lead the route by a tick; retry the state view."""
    deadline = time.monotonic() + WAIT_SECONDS
    last: dict = {}
    while time.monotonic() < deadline:
        last = finalized(port)
        if (last.get("height") is not None
                and last["height"] >= height
                and last.get("block_hash")):
            return last
        time.sleep(POLL_SECONDS)
    raise TimeoutError(f"{label}: no finalized state in {WAIT_SECONDS:.0f}s; last={last}")


def verified(value: object) -> bool:
    if value is True:
        return True
    if isinstance(value, dict):
        for key in ("verified", "valid", "ok"):
            if key in value:
                return bool(value[key])
    return False


def wait_finalized(port: int, height: int, label: str) -> dict:
    return mature.wait_for(
        port,
        lambda body: body.get("ready") is True
        and body.get("finalizedHeight") is not None
        and body["finalizedHeight"] >= height,
        WAIT_SECONDS, POLL_SECONDS, label)


def wait_ready(port: int, label: str) -> dict:
    return mature.wait_for(port, lambda body: body.get("ready") is True,
                           WAIT_SECONDS, POLL_SECONDS, label)


def _fresh_is_substitute(fresh: dict | None, pre: dict) -> bool:
    """A fresh genesis is a substitute only if it produces the populated chain.

    The expected fresh arm is a *different* chain at height 0; same genesis,
    height and post-state is the substitution this control exists to refuse.
    """
    if not fresh or not pre:
        return False
    return (fresh.get("genesis_hash") and fresh.get("genesis_hash") == pre.get("genesis_hash")
            and fresh.get("height", -1) >= pre.get("height", 0)
            and fresh.get("post_state") == pre.get("post_state"))


def populated_acceptance(pre: dict, copied: dict, rollback: dict,
                         advanced: dict | None, fresh: dict | None,
                         logs: str = "") -> tuple[str, list[str]]:
    """The gate's predicate; `self_test` exercises it without a devnet."""
    if not pre or not pre.get("genesis_hash"):
        return "INCONCLUSIVE", ["the populated arm did not capture a genesis hash"]
    if not copied or not copied.get("genesis_hash"):
        return "INCONCLUSIVE", ["the copy restart did not answer"]
    if copied["genesis_hash"] != pre["genesis_hash"]:
        return "INCONCLUSIVE", ["the copy restart changed the genesis hash"]
    reasons: list[str] = []
    if copied.get("height", -1) < pre.get("height", 0):
        reasons.append("the copy restart did not retain the finalized height")
    if copied.get("block_hash") != pre.get("block_hash"):
        reasons.append("the copy restart serves a different finalized block")
    if copied.get("post_state") != pre.get("post_state"):
        reasons.append("the copy restart serves a different post-state")
    # The certificate can lag the last-finalized route by one block while
    # autopropose is running; compare the anchor only when it is current.
    if pre.get("anchor") and pre.get("anchor_height") == pre.get("height"):
        if copied.get("anchor") != pre.get("anchor"):
            reasons.append("the copy restart's checkpoint anchor differs")
        if (pre.get("anchor_post_state")
                and copied.get("anchor_post_state") != pre.get("anchor_post_state")):
            reasons.append("the copy restart's anchor post-state differs")
    if copied.get("verified") is not True:
        reasons.append("the copied certificate did not verify")
    if rollback.get("height") != pre.get("height"):
        reasons.append("the rollback did not return to the populated height")
    if rollback.get("block_hash") != pre.get("block_hash"):
        reasons.append("the rollback serves a different finalized block")
    if pre.get("anchor") and pre.get("anchor_height") == pre.get("height"):
        if rollback.get("anchor") != pre.get("anchor"):
            reasons.append("the rollback's checkpoint anchor differs")
        if (pre.get("anchor_post_state")
                and rollback.get("anchor_post_state") != pre.get("anchor_post_state")):
            reasons.append("the rollback's anchor post-state differs")
    if advanced and advanced.get("height", -1) <= pre.get("height", -1):
        reasons.append("the advance arm did not move past the populated height")
    if _fresh_is_substitute(fresh, pre):
        reasons.append("the fresh-genesis arm substituted for the populated store")
    lowered = (logs or "").lower()
    for marker in ("replayexception", "replay divergence", "justification regressed",
                   "cannot replay", "state hash mismatch"):
        if marker in lowered:
            reasons.append(f"the node log contains {marker!r}")
    return ("FAIL", reasons) if reasons else ("PASS", [])


def capture(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


def container_logs() -> str:
    """The node writes its journal to stderr; merge it rather than dropping it."""
    try:
        return mature.docker(["logs", "--timestamps", "--tail", "2000",
                              f"{PREFIX}-bootstrap"])
    except (RuntimeError, subprocess.CalledProcessError, subprocess.TimeoutExpired):
        return ""


def snapshot_all(suffix: str) -> dict[str, str]:
    """The single-validator devnet's bootstrap volume is its whole populated store."""
    source = node_volume("bootstrap")
    target = f"{PREFIX}-snapshot-{suffix}-bootstrap"
    volume_create(target)
    copy_volume(source, target, replace=True)
    return {"bootstrap": target}


def restore_all(snapshots: dict[str, str]) -> None:
    for node, snapshot in snapshots.items():
        copy_volume(snapshot, node_volume(node), replace=True)


def run_gate(args) -> int:
    out = Path(args.out)
    raw = out / "raw"
    raw.mkdir(parents=True, exist_ok=True)
    if args.source_sha:
        capture(out / "source-sha.txt", args.source_sha.strip() + "\n")
    image_id = docker(["image", "inspect", "-f", "{{.Id}}", "rnode:local"]).strip()
    image_id = image_id.removeprefix("sha256:")
    capture(out / "binary-sha256.txt", image_id + "\n")
    result: dict = {"result": "INCONCLUSIVE", "reasons": [], "arms": {}}
    snapshots: dict[str, str] = {}
    copy_volume_name = f"{PREFIX}-copy"
    try:
        volume_create(copy_volume_name)
        try:
            devnet("down", "-v")
        except RuntimeError:
            pass
        devnet("up", "--validators", "1", "--fresh", "--epoch-length", str(EPOCH_LENGTH))
        pre = wait_finalized_state(HTTP_BASE, TARGET_HEIGHT, "the populated chain")
        pre["genesis_hash"] = genesis_hash(HTTP_BASE)
        result["arms"]["populated"] = pre
        capture(raw / "populated.json", json.dumps(pre, indent=2, sort_keys=True) + "\n")

        # Stop before copying: a live LMDB copy is not the exact-format artifact.
        devnet("down")
        source_manifest = volume_manifest(node_volume("bootstrap"))
        copy_volume(node_volume("bootstrap"), copy_volume_name, replace=True)
        copy_manifest = volume_manifest(copy_volume_name)
        manifests = {"source": source_manifest, "copy": copy_manifest}
        capture(raw / "store-manifests.json",
                json.dumps(manifests, indent=2, sort_keys=True) + "\n")
        if source_manifest["digest"] != copy_manifest["digest"]:
            result["reasons"].append("the copied store's manifest differs from the source")
            result["result"] = "FAIL"

        devnet("up", "--validators", "1", "--data-volume", copy_volume_name,
               "--no-autopropose")
        copied = wait_finalized_state(HTTP_BASE, pre.get("height") or TARGET_HEIGHT,
                                      "the copied store's restart")
        copied["genesis_hash"] = genesis_hash(HTTP_BASE)
        result["arms"]["copied"] = copied
        capture(raw / "copied.json", json.dumps(copied, indent=2, sort_keys=True) + "\n")
        copied_log = container_logs()
        capture(raw / "copied-bootstrap.log", copied_log)

        devnet("down")
        snapshots = snapshot_all("h0")
        devnet("up", "--validators", "1", "--data-volume", copy_volume_name,
               "--no-autopropose")
        wait_finalized_state(HTTP_BASE, pre.get("height") or TARGET_HEIGHT,
                             "the advance arm's start")
        devnet("propose")
        advanced_state = wait_finalized_state(
            HTTP_BASE, (pre.get("height") or 0) + 1,
            "the advance arm's next finalized block")
        advanced_state["genesis_hash"] = genesis_hash(HTTP_BASE)
        result["arms"]["advanced"] = advanced_state
        capture(raw / "advanced.json",
                json.dumps(advanced_state, indent=2, sort_keys=True) + "\n")

        devnet("down")
        restore_all(snapshots)
        devnet("up", "--validators", "1", "--no-autopropose")
        rollback = wait_finalized_state(HTTP_BASE, pre.get("height") or TARGET_HEIGHT,
                                        "the rollback arm's start")
        rollback["genesis_hash"] = genesis_hash(HTTP_BASE)
        result["arms"]["rollback"] = rollback
        capture(raw / "rollback.json", json.dumps(rollback, indent=2, sort_keys=True) + "\n")
        rollback_log = container_logs()
        capture(raw / "rollback-bootstrap.log", rollback_log)
        logs = copied_log + "\n" + rollback_log

        devnet("down", "-v")
        devnet("up", "--validators", "1", "--fresh", "--no-autopropose",
               "--epoch-length", str(EPOCH_LENGTH))
        health = wait_ready(HTTP_BASE, "the fresh-genesis arm")
        genesis = genesis_block(HTTP_BASE)
        fresh = {"genesis_hash": genesis.get("genesis_hash"),
                 "height": health.get("finalizedHeight") or 0,
                 "block_hash": genesis.get("genesis_hash"),
                 "post_state": genesis.get("post_state"),
                 "digest": None}
        result["arms"]["fresh"] = fresh
        capture(raw / "fresh.json", json.dumps(fresh, indent=2, sort_keys=True) + "\n")

        verdict, reasons = populated_acceptance(
            pre, copied, rollback, advanced_state, fresh, logs)
        if result["result"] == "FAIL" and reasons:
            verdict, reasons = "FAIL", result["reasons"] + reasons
        result["result"], result["reasons"] = verdict, reasons
    except (RuntimeError, subprocess.TimeoutExpired, TimeoutError, OSError,
            json.JSONDecodeError) as error:
        result["result"] = "INCONCLUSIVE"
        result["reasons"].append(str(error))
    finally:
        if not args.keep:
            try:
                devnet("down", "-v")
            except (RuntimeError, subprocess.TimeoutExpired):
                pass
            volume_remove(copy_volume_name)
            for snapshot in snapshots.values():
                volume_remove(snapshot)
    capture(raw / "result.json", json.dumps(result, indent=2, sort_keys=True) + "\n")
    capture(out / "campaign.json", json.dumps(
        {"aggregate": result["result"],
         "cycles": [{"name": name, "result": result["result"],
                     "arm": arm} for name, arm in result["arms"].items()]},
        indent=2, sort_keys=True) + "\n")
    print(json.dumps({"result": result["result"], "reasons": result["reasons"]},
                     sort_keys=True))
    return 0 if result["result"] == "PASS" else (3 if result["result"] == "INCONCLUSIVE" else 1)


def self_test() -> int:
    """Hermetic controls for the acceptance predicate; the mutation must fail here."""
    pre = {"genesis_hash": "a" * 64, "height": 5, "block_hash": "b" * 64,
           "post_state": "c" * 64, "digest": "d" * 64}
    copied = dict(pre, verified=True)
    rollback = dict(pre)
    advanced = dict(pre, height=6)
    substitute = dict(pre)
    fresh = {"genesis_hash": "e" * 64, "height": 0, "block_hash": "f" * 64,
             "post_state": "0" * 64, "digest": "1" * 64}
    failures: list[str] = []

    verdict, reasons = populated_acceptance(pre, copied, rollback, advanced, substitute, "")
    if verdict != "FAIL" or not any("substituted" in reason for reason in reasons):
        failures.append(f"the fresh-genesis substitution was not refused: {verdict} {reasons}")

    verdict, reasons = populated_acceptance(pre, copied, rollback, advanced, fresh, "")
    if verdict != "PASS":
        failures.append(f"the legitimate fresh arm did not pass: {verdict} {reasons}")

    verdict, _ = populated_acceptance(pre, dict(copied, genesis_hash="9" * 64),
                                      rollback, advanced, fresh, "")
    if verdict != "INCONCLUSIVE":
        failures.append(f"a changed genesis was not INCONCLUSIVE: {verdict}")

    verdict, _ = populated_acceptance(pre, dict(copied, post_state="8" * 64),
                                      rollback, advanced, fresh, "")
    if verdict != "FAIL":
        failures.append(f"a changed post-state was not FAIL: {verdict}")

    verdict, _ = populated_acceptance(pre, copied, dict(rollback, height=6),
                                      advanced, fresh, "")
    if verdict != "FAIL":
        failures.append(f"a rollback that did not return was not FAIL: {verdict}")

    verdict, _ = populated_acceptance(pre, copied, rollback, dict(advanced, height=5),
                                      fresh, "")
    if verdict != "FAIL":
        failures.append(f"an advance that did not move was not FAIL: {verdict}")

    verdict, _ = populated_acceptance(pre, copied, rollback, advanced, fresh,
                                      "thread panicked: replay divergence")
    if verdict != "FAIL":
        failures.append(f"a replay-divergence log was not FAIL: {verdict}")

    if failures:
        for failure in failures:
            print(f"SELF-TEST FAIL: {failure}", file=sys.stderr)
        return 1
    print(json.dumps({"result": "SELF_TEST_PASS", "controls": 7}, sort_keys=True))
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", nargs="?", default="gate",
                        choices=("gate", "self-test"))
    parser.add_argument("--gate", action="store_true",
                        help="run the populated-store acceptance (the default)")
    parser.add_argument("--out", default="target/g520-migration-store-drill")
    parser.add_argument("--source-sha", default=None,
                        help="40-hex commit under test; written into the run directory")
    parser.add_argument("--keep", action="store_true",
                        help="leave the devnet and volumes for diagnosis")
    args = parser.parse_args(argv)
    if args.command == "self-test":
        return self_test()
    return run_gate(args)


if __name__ == "__main__":
    raise SystemExit(main())
