#!/usr/bin/env python3
"""The restart/replay arm (barrier B31/B41, safety-case H-RS-10).

A validator on a below-minimum host is SIGKILLed and restarted. The restart must replay from the
durable state, come back serving, and not be killed again — the failure this guards against is the
supervisor restarting a node into the same OOM, which is why the start-up preflight (A2) and the
fallback state (C) exist. The arm records the replay window, the container's OOM/restart state, and
whether the node's accepted chain survived.

The node is restarted with the same flags the devnet gave it (including the operator's explicit
`--allow-under-minimum` on a below-minimum host). The preflight's *refusal* without the override is
the A2 arming instrument's subject, not this arm's.

Requires a running devnet whose containers answer (see `tools/n420-soak.py` for the 1 GiB start).

Usage: tools/resource-restart-arm.py [--container n420soak-validator-1] [--out DIR] [--timeout 600]
"""

import argparse
import json
import os
import subprocess
import sys
import time

PREFIX = os.environ.get("DEVNET_PREFIX", "n420soak")
DEFAULT_CONTAINER = f"{PREFIX}-validator-1"
SHARD = "/root"
DEPLOYER_KEY = "a68a6e6cca30f81bd24a719f3145d20e8424bd7b396309b0708a16c7d8000b76"


def run(cmd, check=True):
    return subprocess.run(cmd, check=check, text=True, capture_output=True)


def docker(*args, check=True):
    return run(["docker", *args], check=check)


def get(container, path):
    out = run(["docker", "exec", container, "curl", "-sS", f"http://127.0.0.1:40403{path}"]).stdout
    return json.loads(out)


def facts(container):
    state = run(
        ["docker", "inspect", "--format", "{{.State.OOMKilled}} {{.RestartCount}}", container]
    ).stdout.split()
    return {"oom_killed": state[0] == "true", "restarts": int(state[1])}


def tip(container):
    """The highest block a node holds — the anchor the restarted node must replay to. A read, so it
    works on a node whose fallback state is `critical` and which correctly refuses new work."""
    infos = get(container, "/api/blocks/50")
    return max(infos, key=lambda b: b.get("blockNumber", 0)) if infos else None


def first_normal(containers, timeout=120):
    """The first container that will accept work. Under a 1 GiB burst a node can be `critical` when
    the arm starts; the deploy must go to a node the fallback admits, while the restart target is
    chosen separately."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        for container in containers:
            try:
                status = get(container, "/api/status")
                if status.get("degraded") == "normal" and status.get("acceptsDeploys"):
                    return container
            except (subprocess.CalledProcessError, json.JSONDecodeError, KeyError):
                pass
        time.sleep(2)
    raise SystemExit("no node is accepting deploys; the arm needs one normal node to anchor a block")


def accepted_deploy(container):
    """Deploy one term and wait for a block that carries it, returning that block."""
    run(
        [
            "docker",
            "exec",
            container,
            "sh",
            "-c",
            "printf 'Nil' > /tmp/n420-restart.rho",
        ]
    )
    before = get(container, "/api/status")["latestBlockNumber"]
    deploy = run(
        [
            "docker",
            "exec",
            container,
            "rnode",
            "deploy",
            "--phlo-limit",
            "1000000",
            "--phlo-price",
            "1",
            "--private-key",
            DEPLOYER_KEY,
            "--shard-id",
            SHARD,
            "/tmp/n420-restart.rho",
        ],
        check=False,
    )
    if deploy.returncode != 0:
        lines = [
            line
            for line in (deploy.stdout + deploy.stderr).splitlines()
            if line.strip() and "jemalloc" not in line
        ]
        raise SystemExit("the anchor deploy was refused: " + " | ".join(lines[-4:]))
    deadline = time.time() + 120
    while time.time() < deadline:
        height = get(container, "/api/status")["latestBlockNumber"]
        if height > before:
            for info in get(container, "/api/blocks/50"):
                if info.get("blockNumber") == height:
                    return info
        time.sleep(1)
    raise SystemExit("no block carried the anchor deploy")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--container", default=DEFAULT_CONTAINER)
    parser.add_argument(
        "--deploy-to",
        default=None,
        help="the node that anchors the block; default: the first node the fallback admits",
    )
    parser.add_argument("--out", default="target/issue420-evidence/restart")
    parser.add_argument("--timeout", type=int, default=600)
    args = parser.parse_args()

    if docker("inspect", args.container, check=False).returncode != 0:
        raise SystemExit(f"{args.container} is not running")

    before_facts = facts(args.container)
    anchor_source = args.deploy_to or f"{PREFIX}-bootstrap"
    anchor = tip(anchor_source)
    if anchor is None:
        raise SystemExit(f"{anchor_source} holds no block to anchor on")
    height = anchor["blockNumber"]
    print(
        f"anchor block {anchor['blockHash'][:12]} at height {height} (from {anchor_source}); "
        f"restart target {args.container}",
        flush=True,
    )

    # SIGKILL: the same class as the kernel's OOM kill, without waiting for an OOM.
    started = time.time()
    docker("kill", "--signal", "KILL", args.container)
    docker("start", args.container)

    deadline = time.time() + args.timeout
    recovered = None
    while time.time() < deadline:
        try:
            status = get(args.container, "/api/status")
            if status["latestBlockNumber"] >= height:
                recovered = status
                break
        except (subprocess.CalledProcessError, json.JSONDecodeError, KeyError):
            pass
        time.sleep(2)
    elapsed = time.time() - started
    if recovered is None:
        raise SystemExit(f"the node did not replay to height {height} within {args.timeout}s")

    after_facts = facts(args.container)
    anchor_after = get(args.container, "/api/block/" + anchor["blockHash"]).get(
        "blockInfo", {}
    )
    record = {
        "boundary": "finite restart/replay witness on a below-minimum host; not a supported-tier claim",
        "container": args.container,
        "anchor_block": anchor["blockHash"],
        "anchor_height": height,
        "replay_seconds": elapsed,
        "before": before_facts,
        "after": after_facts,
        "recovered_height": recovered["latestBlockNumber"],
        "anchor_still_present": bool(anchor_after),
        "fallback_state": recovered.get("degraded"),
        "manual_restart": True,
        "non_claims": [
            "one node, one SIGKILL; not a universal recovery theorem",
            "restarted with the operator's --allow-under-minimum on a below-minimum host",
        ],
    }
    os.makedirs(args.out, exist_ok=True)
    with open(os.path.join(args.out, "restart.json"), "w") as handle:
        json.dump(record, handle, indent=2)

    problems = []
    if after_facts["oom_killed"] and not before_facts["oom_killed"]:
        problems.append("the restarted container was OOM-killed")
    if not record["anchor_still_present"]:
        problems.append("the anchor block is missing after replay")
    print(
        f"replay {elapsed:.1f}s to height {recovered['latestBlockNumber']} "
        f"(degraded={record['fallback_state']})",
        flush=True,
    )
    if problems:
        for problem in problems:
            print(f"FAIL: {problem}", file=sys.stderr)
        sys.exit(1)
    print(f"ok: evidence at {args.out}/restart.json")


if __name__ == "__main__":
    main()
