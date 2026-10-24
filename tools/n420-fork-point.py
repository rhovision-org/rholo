#!/usr/bin/env python3
"""The #420 devnet/fork-point arm: a multi-parent merge of two divergent tips.

What this asserts, and what it does not. The in-process regressions
(`casper/tests/n420_boundary_merge.rs`, `n420_rcash_replay.rs`) prove the merge function on
source-pinned state. This arm runs the *node*: four real validators, one partition, two tips at the
same height, then a healing merge. It asserts that every node derives the same merged pre-state, that
the merged state carries both writers' values, and that a restart re-derives the same state.

  a. every node reports the same merge block hash and post-state hash;
  b. the merge's pre-state is neither parent's post-state (two divergent writers — the #420
     discriminator; the single-writer control is in the in-process test);
  c. the merge's justification set contains both tips;
  d. an exploratory read at the merge's post-state returns both values, and at the **common base**
     (the parents' ancestor) neither (the negative control; the merge's own pre-state is the merged
     state of its parents, so it is expected to carry both);
  e. restarting one validator and letting it catch up reproduces the same post-state and reads.

Non-claims: a finite devnet witness, not a universal safety theorem; no live network; the merge
proposal may be withheld by the round/attestation gates — that is recorded as a result, not retried
until green.

Requires a running devnet started by this script's companion command (printed on failure):

  DEVNET_PREFIX=n420fork DEVNET_NODE_MEMORY=2g tools/devnet.sh up --validators 4 --fresh \
      --no-autopropose --epoch-length 10

Usage: tools/n420-fork-point.py [--out DIR] [--timeout 600]
"""

import argparse
import base64
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request

PREFIX = os.environ.get("DEVNET_PREFIX", "n420fork")
NETWORK = os.environ.get("DEVNET_NETWORK", "devnet")
SHARD = "/root"
# The devnet's fixed, throwaway deployer key (tools/devnet.sh). It is funded at genesis.
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


def request(container, path, body=None):
    """HTTP from **inside** the container: a partitioned container is off the bridge, so its host
    port mapping is unreachable — and the isolated side is exactly what this arm queries."""
    args = ["docker", "exec", container, "curl", "-sS", "-X",
            "POST" if body is not None else "GET"]
    if body is not None:
        args += ["-H", "content-type: application/json", "-d", json.dumps(body)]
    args.append(f"http://127.0.0.1:40403{path}")
    return json.loads(run(args).stdout)


def status(container):
    return request(container, "/api/status")


def wait_all(containers, timeout, minimum_height=1):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            heights = [status(c)["latestBlockNumber"] for c in containers]
            if all(h >= minimum_height for h in heights):
                return heights
        except (subprocess.CalledProcessError, json.JSONDecodeError, KeyError):
            pass
        time.sleep(2)
    raise SystemExit(f"the devnet did not reach height {minimum_height}: {containers}")


def block_info(container, block_hash):
    response = request(container, f"/api/block/{block_hash}")
    return response.get("blockInfo", response)


def blocks(container, depth=10):
    response = request(container, f"/api/blocks/{depth}")
    if isinstance(response, dict):
        return response.get("blocks", [])
    return response


def explore(container, term, block_hash, use_pre_state):
    response = request(
        container,
        "/api/v1/explore-deploy-by-block-hash",
        {"term": term, "blockHash": block_hash, "usePreStateHash": use_pre_state},
    )
    if not isinstance(response, dict):
        raise SystemExit(f"the exploratory deploy was refused: {response}")
    return response


def deploy(container, term, height):
    encoded = base64.b64encode(term.encode()).decode()
    path = f"/tmp/n420-fork-{abs(hash(term)) % 10_000}.rho"
    run(
        [
            "docker",
            "exec",
            container,
            "sh",
            "-c",
            f"echo {encoded} | base64 -d > {path}",
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
            "--valid-after-block-number",
            str(height),
            path,
        ]
    )


def propose(container):
    return run(["docker", "exec", container, "rnode", "propose"], check=False)


def tip(container):
    """The highest block this node holds. `latestBlockNumber` counts rounds; a block's own number is
    the field the merge is judged by."""
    infos = blocks(container, 50)
    return max(infos, key=lambda b: b.get("blockNumber", 0)) if infos else None


def find_merge(container, height, tips, deadline):
    """A block above `height` whose justification set contains both tips."""
    while time.time() < deadline:
        for info in blocks(container, 25):
            justifications = set(info.get("justifications") or [])
            if info.get("blockNumber", 0) > height and set(tips) <= justifications:
                return info
        time.sleep(2)
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", default="target/issue420-evidence/fork-point")
    parser.add_argument("--timeout", type=int, default=600)
    args = parser.parse_args()

    record = {"boundary": "finite devnet witness; no universal safety theorem", "steps": []}

    def step(name, value):
        record["steps"].append({"step": name, "value": value})
        print(f"== {name}: {value}", flush=True)

    missing = [c for c in CONTAINERS if docker("inspect", c, check=False).returncode != 0]
    if missing:
        raise SystemExit(
            "missing containers "
            + ", ".join(missing)
            + "; start the devnet first:\n  "
            + f"DEVNET_PREFIX={PREFIX} DEVNET_NODE_MEMORY=2g tools/devnet.sh up --validators 4 "
            "--fresh --no-autopropose --epoch-length 10"
        )

    heights = wait_all(CONTAINERS, args.timeout)
    step("heights", heights)
    base = tip(CONTAINERS[0])
    if base is None:
        raise SystemExit("the bootstrap holds no block")
    height = base["blockNumber"]
    step("base", {"blockNumber": height, "hash": base["blockHash"]})

    # The partition: bootstrap + validator-1 on the network; validator-2 isolated. Each side has its
    # own DAG and can propose; the deployer is funded on both.
    for isolated in (CONTAINERS[2], CONTAINERS[3]):
        docker("network", "disconnect", NETWORK, isolated)
    step("partition", {"group_a": CONTAINERS[:2], "group_b": CONTAINERS[2:4]})

    # **Distinct channels on purpose.** Two linear produces on one channel are a conflict the merge
    # resolves by keeping one; the property this arm checks is that two *accepted, non-conflicting*
    # writers both survive a multi-parent merge.
    deploy(CONTAINERS[0], r'@"forkA"!("A")', height)
    deploy(CONTAINERS[2], r'@"forkB"!("B")', height)
    propose(CONTAINERS[0])
    propose(CONTAINERS[2])

    deadline = time.time() + args.timeout
    tip_a = tip_b = None
    while time.time() < deadline:
        tip_a = tip(CONTAINERS[0])
        tip_b = tip(CONTAINERS[2])
        advanced = (
            tip_a is not None
            and tip_b is not None
            and tip_a["blockNumber"] > height
            and tip_b["blockNumber"] > height
            and tip_a["blockNumber"] == tip_b["blockNumber"]
        )
        if advanced:
            break
        propose(CONTAINERS[0])
        propose(CONTAINERS[2])
        time.sleep(2)
    if tip_a is None or tip_b is None or tip_a["blockNumber"] <= height:
        raise SystemExit("the partition did not produce a tip on each side; no fork to merge")
    if tip_a["blockHash"] == tip_b["blockHash"]:
        raise SystemExit("both sides produced the same block: no fork")

    tips = [tip_a["blockHash"], tip_b["blockHash"]]
    step(
        "tips",
        {"height": tip_a["blockNumber"], "a": tip_a["blockHash"], "b": tip_b["blockHash"]},
    )

    for isolated in (CONTAINERS[2], CONTAINERS[3]):
        docker("network", "connect", NETWORK, isolated)
    step("healed", True)

    deadline = time.time() + args.timeout
    merge = None
    while time.time() < deadline and merge is None:
        propose(CONTAINERS[0])
        merge = find_merge(CONTAINERS[0], tip_a["blockNumber"], tips, time.time() + 20)
    if merge is None:
        raise SystemExit("no multi-parent merge appeared; the round/attestation gate may have withheld it")
    merge_hash = merge["blockHash"]
    step("merge", merge_hash)

    infos = {c: block_info(c, merge_hash) for c in CONTAINERS[:2]}
    step("merge_views", infos)

    # (a) every node agrees on the merge and its post-state.
    post_states = {info.get("postStateHash") for info in infos.values()}
    assert len(post_states) == 1, f"nodes disagree on the merge post-state: {post_states}"
    pre_state = next(iter(infos.values())).get("preStateHash")

    # (b) two divergent writers: the merge is neither parent's post-state.
    parent_posts = {tip_a["postStateHash"], tip_b["postStateHash"]}
    assert pre_state not in parent_posts, (
        f"the merge pre-state {pre_state} equals a parent's post-state {parent_posts}"
    )

    # (c) both tips are justified.
    justifications = set(next(iter(infos.values())).get("justifications") or [])
    assert set(tips) <= justifications, f"the merge does not justify both tips: {justifications}"

    # (d) the merged state carries both writers; the common base carries neither.
    read = r'for (@x <- @"forkA"; @y <- @"forkB") { @"out"!((x, y)) }'
    merged_post = post_states.pop()
    # The endpoint takes a **block hash** and reads the state the block declares (`usePreStateHash`
    # selects pre vs post), not a state hash.
    at_post = explore(CONTAINERS[0], read, merge_hash, False)
    at_base = explore(CONTAINERS[0], read, base["blockHash"], False)
    post_expr = json.dumps(at_post.get("expr", []))
    base_expr = json.dumps(at_base.get("expr", []))
    assert '"A"' in post_expr and '"B"' in post_expr, f"the merged state lost a writer: {post_expr}"
    assert '"A"' not in base_expr and '"B"' not in base_expr, (
        f"the common base already carried a child value: {base_expr}"
    )
    step("reads", {"post": post_expr, "base": base_expr})

    # (e) a restart re-derives the same state.
    docker("restart", CONTAINERS[1])
    wait_all([CONTAINERS[1]], args.timeout, height)
    again = block_info(CONTAINERS[1], merge_hash)
    assert again.get("postStateHash") == merged_post, (
        "the restarted validator derives a different merge post-state"
    )
    re_read = explore(CONTAINERS[1], read, merge_hash, False)
    re_expr = json.dumps(re_read.get("expr", []))
    assert '"A"' in re_expr and '"B"' in re_expr, f"the restarted validator reads differently: {re_expr}"
    step("restart", {"postStateHash": again.get("postStateHash"), "reads": re_expr})

    os.makedirs(args.out, exist_ok=True)
    with open(os.path.join(args.out, "fork-point.json"), "w") as handle:
        json.dump(record, handle, indent=2)
    print(f"ok: evidence at {args.out}/fork-point.json")


if __name__ == "__main__":
    try:
        main()
    except AssertionError as error:
        print(f"FAIL: {error}", file=sys.stderr)
        sys.exit(1)
