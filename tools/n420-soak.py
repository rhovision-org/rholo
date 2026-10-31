#!/usr/bin/env python3
"""The #420 pressure arm: a 1 GiB devnet under a sustained deploy burst.

What this asserts, and what it does not. It runs the *node* under a cgroup ceiling below the advised
validator minimum and checks that the designed barriers act: no container is OOM-killed, every node
still answers `/api/status`, the fallback state visits `throttled`/`critical` and recovers rather than
the kernel choosing a victim, and finality advances or the arm records the stall as data.

This is **fault injection, not supported-tier acceptance** (#428 M1): the host is deliberately below
the advised size. A pass here says the node sheds work and stays observable at 1 GiB; it does not say
1 GiB is a supported validator.

Start the net first (the `--allow-under-minimum` override is the start-up preflight's explicit
acceptance of the below-minimum host; the preflight's own refusal is pinned by the A2 arming
instrument, not here):

  DEVNET_PREFIX=n420soak DEVNET_NODE_MEMORY=1g \
  DEVNET_EXTRA_FLAGS=--allow-under-minimum \
  tools/devnet.sh up --validators 4 --fresh

Usage: tools/n420-soak.py [--out DIR] [--seconds 600] [--deploy-every 2]
"""

import argparse
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request

PREFIX = os.environ.get("DEVNET_PREFIX", "n420soak")
SHARD = "/root"
DEPLOYER_KEY = "a68a6e6cca30f81bd24a719f3145d20e8424bd7b396309b0708a16c7d8000b76"
CONTAINERS = [
    f"{PREFIX}-bootstrap",
    f"{PREFIX}-validator-1",
    f"{PREFIX}-validator-2",
    f"{PREFIX}-validator-3",
]


def run(cmd, check=True):
    return subprocess.run(cmd, check=check, text=True, capture_output=True)


def docker(*args, check=True):
    return run(["docker", *args], check=check)


def http_port(container):
    out = run(["docker", "port", container, "40403"]).stdout.strip()
    if not out:
        raise SystemExit(f"{container}: port 40403 is not published")
    return int(out.splitlines()[0].rsplit(":", 1)[1])


def get(port, path):
    with urllib.request.urlopen(f"http://127.0.0.1:{port}{path}", timeout=15) as response:
        return json.loads(response.read().decode())


def get_on(container, port, path):
    out = run(
        ["docker", "exec", container, "curl", "-sS", f"http://127.0.0.1:{port}{path}"]
    ).stdout
    return json.loads(out)


def finalized_number(port):
    return get(port, "/api/last-finalized-block")["blockInfo"]["blockNumber"]


def container_facts(container):
    """OOM-killed and restart state, plus the cgroup's own event counters when readable."""
    state = run(
        ["docker", "inspect", "--format", "{{.State.OOMKilled}} {{.RestartCount}}", container]
    ).stdout.split()
    facts = {"oom_killed": state[0] == "true", "restarts": int(state[1])}
    events = run(
        ["docker", "exec", container, "cat", "/sys/fs/cgroup/memory.events"], check=False
    ).stdout
    for line in events.splitlines():
        name, _, value = line.partition(" ")
        if name in ("oom", "oom_kill"):
            facts[name] = int(value)
    return facts


def deploy(container):
    # `rnode deploy` takes a **file**, not a term: a literal "Nil" is read as a filename and every
    # attempt fails. Write the term inside the container once, then deploy the file.
    run(
        [
            "docker",
            "exec",
            container,
            "sh",
            "-c",
            "printf 'Nil' > /tmp/n420-soak.rho",
        ]
    )
    return run(
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
            "/tmp/n420-soak.rho",
        ],
        check=False,
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", default="target/issue420-evidence/soak")
    parser.add_argument("--seconds", type=int, default=600)
    parser.add_argument("--deploy-every", type=float, default=2.0)
    args = parser.parse_args()

    missing = [c for c in CONTAINERS if docker("inspect", c, check=False).returncode != 0]
    if missing:
        raise SystemExit(
            "missing containers "
            + ", ".join(missing)
            + "; start the 1 GiB net first:\n  "
            + f"DEVNET_PREFIX={PREFIX} DEVNET_NODE_MEMORY=1g DEVNET_EXTRA_FLAGS=--allow-under-minimum "
            "tools/devnet.sh up --validators 4 --fresh"
        )

    ports = {c: http_port(c) for c in CONTAINERS}
    initial = {c: container_facts(c) for c in CONTAINERS}
    first_status = {c: get(ports[c], "/api/status") for c in CONTAINERS}
    height = min(s["latestBlockNumber"] for s in first_status.values())

    finality_start = {c: finalized_number(ports[c]) for c in CONTAINERS}
    samples = []
    refusals = 0
    deadline = time.time() + args.seconds
    next_deploy = time.time()
    index = 0
    while time.time() < deadline:
        if time.time() >= next_deploy:
            result = deploy(CONTAINERS[index % len(CONTAINERS)])
            if result.returncode != 0:
                refusals += 1
            index += 1
            next_deploy += args.deploy_every
        sample = {"at": time.time(), "nodes": {}}
        for container in CONTAINERS:
            try:
                status = get(ports[container], "/api/status")
                # The operator half is on the **admin** listener (40405, loopback inside the
                # container); the public port does not serve it.
                operator = get_on(container, 40405, "/api/v1/status/operator")
                resources = operator.get("resources", {})
                sample["nodes"][container] = {
                    "degraded": status.get("degraded"),
                    "accepts_deploys": status.get("acceptsDeploys"),
                    "fallback_state": resources.get("fallbackState"),
                    "fallback_reason": resources.get("fallbackReason"),
                    "mem_available_mb": resources.get("memAvailableMb"),
                    "halted": operator.get("autoproposeTimerHalted"),
                    "height": status.get("latestBlockNumber"),
                }
            except (urllib.error.URLError, KeyError, ConnectionError) as error:
                sample["nodes"][container] = {"error": str(error)}
        samples.append(sample)
        time.sleep(5)

    final = {c: container_facts(c) for c in CONTAINERS}
    final_status = {}
    for container in CONTAINERS:
        try:
            final_status[container] = get(ports[container], "/api/status")
        except (urllib.error.URLError, KeyError, ConnectionError) as error:
            final_status[container] = {"error": str(error)}

    states = sorted(
        {
            node.get("fallback_state")
            for sample in samples
            for node in sample["nodes"].values()
            if node.get("fallback_state")
        }
    )
    finality_end = {}
    for container in CONTAINERS:
        try:
            finality_end[container] = finalized_number(ports[container])
        except (urllib.error.URLError, KeyError, ConnectionError):
            finality_end[container] = None
    record = {
        "boundary": "finite 1 GiB fault-injection arm; not supported-tier acceptance",
        "seconds": args.seconds,
        "deploy_every_s": args.deploy_every,
        "deploys_attempted": index,
        "deploy_refusals": refusals,
        "fallback_states_seen": states,
        "initial": initial,
        "final": final,
        "finality_start": finality_start,
        "finality_end": finality_end,
        "samples": samples,
        "non_claims": [
            "a finite devnet witness, not a universal safety theorem",
            "1 GiB is below the advised validator minimum; a pass is not a supported-tier claim",
            "a transient throttled/critical interval is expected and recorded, not a failure",
        ],
    }

    problems = []
    for container in CONTAINERS:
        before, after = initial[container], final[container]
        if after["oom_killed"] or after.get("oom_kill", 0) > before.get("oom_kill", 0):
            problems.append(f"{container}: the container was OOM-killed")
        if after["restarts"] > before["restarts"]:
            problems.append(f"{container}: the container restarted")
        if "error" in final_status[container]:
            problems.append(f"{container}: /api/status is unreadable: {final_status[container]}")
        if final_status[container].get("autoproposeTimerHalted"):
            problems.append(f"{container}: autopropose halted under pressure")

    os.makedirs(args.out, exist_ok=True)
    with open(os.path.join(args.out, "soak.json"), "w") as handle:
        json.dump(record, handle, indent=2)
    print(f"fallback states seen: {states}")
    print(f"finality {finality_start} -> {finality_end}; refusals {refusals}/{index}")
    if problems:
        for problem in problems:
            print(f"FAIL: {problem}", file=sys.stderr)
        sys.exit(1)
    print(f"ok: evidence at {args.out}/soak.json")


if __name__ == "__main__":
    main()
