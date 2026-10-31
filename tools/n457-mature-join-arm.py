#!/usr/bin/env python3
"""The #457 mature-join arm: a reset validator must LFS-sync a chain that is already 28+ blocks long.

What this asserts, and what it does not. `tools/devnet.sh reset 3` discards validator-3's container and
store and starts it again with `--bootstrap`, so the node must restore the bootstrap's finalized state
from scratch. Before #457 the walk stopped at 3 finished of 28 and failed after three idle rounds; this
arm runs the same shape on a disposable four-validator network and requires the reset node to reach the
bootstrap's height and finalized block, with the sync surface cleared and no `LFS state sync failed` in
its log.

**Fault/acceptance boundary.** A pass is a finite witness on a local Docker bridge: it says the walk now
restores this chain, not that the live testnet join is fixed. The live chain is the observation that
counts, and it is not run here.

Network (a port base and bridge of its own, so it can run beside another devnet):

  DEVNET_PREFIX=n457join DEVNET_NETWORK=n457net \\
  DEVNET_GRPC_BASE=55402 DEVNET_HTTP_BASE=55403 DEVNET_ADMIN_BASE=55405 \\
  tools/devnet.sh up --validators 4 --fresh

Usage: tools/n457-mature-join-arm.py [--min-height 28] [--timeout 600] [--out DIR]
"""

import argparse
import json
import os
import subprocess
import sys
import time

PREFIX = os.environ.get("DEVNET_PREFIX", "n457join")
CONTAINERS = [f"{PREFIX}-bootstrap"] + [f"{PREFIX}-validator-{i}" for i in (1, 2, 3)]
RESET_TARGET = f"{PREFIX}-validator-3"
RESET_INDEX = "3"


def run(cmd, check=True, env=None):
    return subprocess.run(cmd, check=check, text=True, capture_output=True, env=env)


def docker(*args, check=True):
    return run(["docker", *args], check=check)


def get(container, path):
    out = run(
        ["docker", "exec", container, "curl", "-sS", f"http://127.0.0.1:40403{path}"]
    ).stdout
    payload = json.loads(out)
    # A node that has just been started answers a plain string (`"No block ..."`) on some endpoints
    # until its state is loaded; the arm treats that as "not ready", not as a malformed record.
    return payload if isinstance(payload, dict) else {}


def status(container):
    return get(container, "/api/status")


def finalized(container):
    data = get(container, "/api/last-finalized-block")
    block_info = data.get("blockInfo", {})
    return block_info if isinstance(block_info, dict) else {}


def wait_heights(containers, minimum, deadline):
    while time.time() < deadline:
        try:
            heights = {c: status(c)["latestBlockNumber"] for c in containers}
            if all(h >= minimum for h in heights.values()):
                return heights
        except (subprocess.CalledProcessError, json.JSONDecodeError, KeyError):
            pass
        time.sleep(2)
    raise SystemExit(f"the devnet did not reach height {minimum} on every node")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--min-height", type=int, default=28)
    parser.add_argument("--timeout", type=int, default=600)
    parser.add_argument("--out", default="target/issue457-evidence/mature-join")
    args = parser.parse_args()

    missing = [c for c in CONTAINERS if docker("inspect", c, check=False).returncode != 0]
    if missing:
        raise SystemExit(
            "missing containers "
            + ", ".join(missing)
            + "; start the network first (see the module docstring)"
        )

    deadline = time.time() + args.timeout
    # The reset target is about to be wiped, so it is not part of the pre-check: waiting on a dead
    # container would turn "the chain is mature" into "the node we are resetting is healthy".
    heights = wait_heights([c for c in CONTAINERS if c != RESET_TARGET], args.min_height, deadline)
    boot_final = finalized(CONTAINERS[0])
    boot_status = status(CONTAINERS[0])
    print(
        f"chain at {heights}; bootstrap finalized {boot_final.get('blockNumber')} "
        f"({str(boot_final.get('blockHash'))[:12]})",
        flush=True,
    )
    if not boot_final.get("blockHash"):
        raise SystemExit("the bootstrap has no finalized block to sync to")

    reset_env = os.environ.copy()
    print(f"== reset {RESET_TARGET} (empty store, --bootstrap)", flush=True)
    run(["tools/devnet.sh", "reset", RESET_INDEX], env=reset_env)

    started = time.time()
    recovered = None
    restored_anchor = {}
    last_seen = {}
    while time.time() < deadline:
        try:
            st = status(RESET_TARGET)
            fin = finalized(RESET_TARGET)
            last_seen = {"status": st, "finalized": fin}
            # **The claim under test is the restore, not convergence.** The LFS sync is what left the
            # walk stranded; once it returns, the node is in NodeRunning and the windowed catch-up
            # owns the gap above the restored anchor. The chain keeps producing while this runs, so
            # equality with the tip is not the property — the restored anchor being a block the
            # bootstrap holds, and the node leaving NodeSyncing, is.
            if (
                st.get("latestBlockNumber", 0) >= boot_status.get("latestBlockNumber", 0)
                and st.get("sync") is None
                and fin.get("blockHash")
            ):
                recovered = st
                restored_anchor = fin
                break
        except (subprocess.CalledProcessError, json.JSONDecodeError, KeyError):
            pass
        time.sleep(2)
    elapsed = time.time() - started
    if recovered is None:
        raise SystemExit(
            "the reset node did not catch up to the bootstrap's height and finalized block within "
            f"{args.timeout}s; last seen: {json.dumps(last_seen)[:600]}"
        )

    # The restored anchor must be a block the bootstrap holds: a node that left `NodeSyncing` on a
    # state the bootstrap never had is not a join.
    anchor_hex = str(restored_anchor.get("blockHash"))
    held_by_bootstrap = False
    try:
        held = get(CONTAINERS[0], f"/api/block/{anchor_hex}")
        held_by_bootstrap = bool(held.get("blockInfo"))
    except (subprocess.CalledProcessError, json.JSONDecodeError):
        held_by_bootstrap = False

    logs = run(["docker", "logs", RESET_TARGET], check=False)
    combined = (logs.stdout + logs.stderr)
    sync_failures = combined.count("LFS state sync failed")
    give_ups = combined.count("no block finished in")

    record = {
        "boundary": "finite local devnet witness; not a live-testnet join and not a certification",
        "chain_height": heights,
        "bootstrap_finalized": {
            "number": boot_final.get("blockNumber"),
            "hash": boot_final.get("blockHash"),
        },
        "reset_target": RESET_TARGET,
        "recovered": {
            "latest_block_number": recovered.get("latestBlockNumber"),
            "restored_anchor_number": restored_anchor.get("blockNumber"),
            "restored_anchor_hash": restored_anchor.get("blockHash"),
            "anchor_held_by_bootstrap": held_by_bootstrap,
            "sync": recovered.get("sync"),
        },
        "catch_up_seconds": elapsed,
        "sync_failures_in_log": sync_failures,
        "give_ups_in_log": give_ups,
        "manual_reset": True,
        "non_claims": [
            "one local four-validator devnet; a finite witness, not a universal join theorem",
            "the chain is this build's own; the live testnet replay is #421/#429's",
            "the reset node is started with `--bootstrap` and the devnet's validator key",
        ],
    }
    os.makedirs(args.out, exist_ok=True)
    with open(os.path.join(args.out, "mature-join.json"), "w") as handle:
        json.dump(record, handle, indent=2)

    problems = []
    if sync_failures:
        problems.append(f"the reset node logged {sync_failures} LFS sync failure(s)")
    if recovered.get("sync") is not None:
        problems.append("the sync surface is not cleared")
    if not held_by_bootstrap:
        problems.append("the restored anchor is not a block the bootstrap holds")
    print(
        f"recovered in {elapsed:.1f}s to round {recovered.get('latestBlockNumber')}, anchor "
        f"{restored_anchor.get('blockNumber')} ({anchor_hex[:12]}, held by bootstrap={held_by_bootstrap}); "
        f"sync={recovered.get('sync')}; sync failures={sync_failures}",
        flush=True,
    )
    if problems:
        for problem in problems:
            print(f"FAIL: {problem}", file=sys.stderr)
        sys.exit(1)
    print(f"ok: evidence at {args.out}/mature-join.json")


if __name__ == "__main__":
    main()
