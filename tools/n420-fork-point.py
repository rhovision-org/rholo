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

from n447_partition import DockerBackend, SplitNetwork
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


def probe_transport(source, target):
    """An actual HTTP/TCP connection, not merely common Docker membership."""
    result = run([
        "docker", "exec", source, "curl", "-fsS",
        "--connect-timeout", "2", "--max-time", "4",
        f"http://{target}:40403/api/status",
    ], check=False)
    return result.returncode == 0


def observe_cross_tip_gossip(containers, tips):
    """Report whether both independently proposed blocks reached every node."""
    observed = {}
    for node in containers:
        found = {}
        for block_hash in tips:
            try:
                info = block_info(node, block_hash)
                found[block_hash] = (info.get("blockHash") == block_hash)
            except Exception:
                found[block_hash] = False
        observed[node] = found
    return observed


def all_tips_visible(observed):
    return bool(observed) and all(all(visibilities.values()) for visibilities in observed.values())


def merge_state_agreement(infos, nodes, merge_hash):
    """All validators must serve the exact same merge block and state hashes."""
    if set(infos) != set(nodes):
        raise AssertionError("merge block has not reached all four validators")
    hashes = {info.get("blockHash") for info in infos.values()}
    pre = {info.get("preStateHash") for info in infos.values()}
    post = {info.get("postStateHash") for info in infos.values()}
    if hashes != {merge_hash} or len(pre) != 1 or None in pre or len(post) != 1 or None in post:
        raise AssertionError(
            f"validator disagreement on merge block/pre/post state: "
            f"hashes={hashes} pre={pre} post={post}"
        )
    return next(iter(pre)), next(iter(post))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", default="target/issue420-evidence/fork-point")
    parser.add_argument("--timeout", type=int, default=600)
    parser.add_argument("--gossip-timeout", type=int, default=120)
    args = parser.parse_args()

    record = {
        "boundary": "finite devnet witness; no universal safety theorem",
        "result": "INCOMPLETE",
        "steps": [],
    }

    def persist():
        # A failed/disconnected test must leave its actual observations behind.
        # Write atomically so a terminated run cannot leave malformed JSON.
        os.makedirs(args.out, exist_ok=True)
        destination = os.path.join(args.out, "fork-point.json")
        temporary = destination + ".tmp"
        with open(temporary, "w") as handle:
            json.dump(record, handle, indent=2)
            handle.write("\n")
        os.replace(temporary, destination)

    def step(name, value):
        record["steps"].append({"step": name, "value": value})
        persist()
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

    # A 2+2 split requires TWO live internal bridges. Merely disconnecting
    # validators 2 and 3 from the only bridge also disconnects them from each other.
    # SplitNetwork audits actual Docker membership and always restores it.
    with SplitNetwork(DockerBackend(), prefix=PREFIX, main=NETWORK) as topology:
        step("partition", {
            "group_a": topology.left,
            "group_b": topology.right,
            "networks": topology.require_partition(),
        })
        # A shared bridge is not proof that the peers can communicate. Require
        # actual HTTP reachability within each pair and no route between pairs.
        checks = {
            "left_0_to_1": probe_transport(CONTAINERS[0], CONTAINERS[1]),
            "left_1_to_0": probe_transport(CONTAINERS[1], CONTAINERS[0]),
            "right_2_to_3": probe_transport(CONTAINERS[2], CONTAINERS[3]),
            "right_3_to_2": probe_transport(CONTAINERS[3], CONTAINERS[2]),
            "cross_0_to_2": probe_transport(CONTAINERS[0], CONTAINERS[2]),
            "cross_2_to_0": probe_transport(CONTAINERS[2], CONTAINERS[0]),
        }
        step("partition_transport", checks)
        if not all(checks[k] for k in (
            "left_0_to_1", "left_1_to_0", "right_2_to_3", "right_3_to_2"
        )):
            raise SystemExit("invalid 2+2 partition: intra-group HTTP connectivity failed")
        if checks["cross_0_to_2"] or checks["cross_2_to_0"]:
            raise SystemExit("invalid 2+2 partition: cross-group route is still reachable")

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
        if (tip_a is None or tip_b is None or tip_a["blockNumber"] <= height
                or tip_b["blockNumber"] <= height):
            raise SystemExit("the partition did not produce a tip on each side; no fork to merge")
        if tip_a["blockHash"] == tip_b["blockHash"]:
            raise SystemExit("both sides produced the same block: no fork")
        if tip_a["blockNumber"] != tip_b["blockNumber"]:
            raise SystemExit("the partition produced unequal tip heights; same-height merge arm unproven")

        tips = [tip_a["blockHash"], tip_b["blockHash"]]
        step(
            "tips",
            {"height": tip_a["blockNumber"], "a": tip_a["blockHash"], "b": tip_b["blockHash"]},
        )

    step("healed", {node: sorted(topology.backend.members(node)) for node in CONTAINERS})

    # A missing merge is not a merge-state failure when the validators have
    # not even received both fork tips. Diagnose gossip before proposal gates.
    gossip_deadline = time.time() + args.gossip_timeout
    visibility = observe_cross_tip_gossip(CONTAINERS, tips)
    while not all_tips_visible(visibility) and time.time() < gossip_deadline:
        time.sleep(2)
        visibility = observe_cross_tip_gossip(CONTAINERS, tips)
    step("healed_tip_gossip", visibility)
    if not all_tips_visible(visibility):
        raise SystemExit(
            "healed network did not exchange both fork tips; "
            "gossip recovery failed before multi-parent merge validation"
        )

    deadline = time.time() + args.timeout
    merge = None
    while time.time() < deadline and merge is None:
        propose(CONTAINERS[0])
        merge = find_merge(CONTAINERS[0], tip_a["blockNumber"], tips, time.time() + 20)
    if merge is None:
        observations = {}
        for node in CONTAINERS:
            try:
                observations[node] = status(node)
            except Exception as error:
                observations[node] = {"read_error": f"{type(error).__name__}: {error}"}
        step("merge_not_observed", {
            "expected_tips": tips,
            "per_node_status": observations,
            "timeout_seconds": args.timeout,
        })
        raise SystemExit(
            "no multi-parent merge appeared; inspect merge_not_observed in fork-point.json"
        )
    merge_hash = merge["blockHash"]
    step("merge", merge_hash)

    # Wait for the actual merged block to become readable from all FOUR
    # validators, not just the two on the first side of the earlier split.
    # Separate absent replication from divergent state-hash observations.
    replication_deadline = time.time() + min(args.timeout, 120)
    infos = {}
    errors = {}
    while True:
        for node in CONTAINERS:
            try:
                infos[node] = block_info(node, merge_hash)
                errors.pop(node, None)
            except Exception as error:
                errors[node] = f"{type(error).__name__}: {error}"
        if len(infos) == len(CONTAINERS):
            break
        if time.time() >= replication_deadline:
            step("merge_replication_missing", {
                "available": infos,
                "errors": errors,
                "target": merge_hash,
            })
            raise SystemExit("merge block did not propagate to all four validators")
        time.sleep(2)
    step("merge_views", infos)

    # (a) all FOUR validators agree on the exact block and pre/post state.
    pre_state, merged_post = merge_state_agreement(infos, CONTAINERS, merge_hash)

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
    # The endpoint takes a **block hash** and reads the state the block declares (`usePreStateHash`
    # selects pre vs post), not a state hash.
    per_node_reads = {
        node: json.dumps(explore(node, read, merge_hash, False).get("expr", []))
        for node in CONTAINERS
    }
    at_base = explore(CONTAINERS[0], read, base["blockHash"], False)
    post_results = set(per_node_reads.values())
    base_expr = json.dumps(at_base.get("expr", []))
    assert len(post_results) == 1, f"application state differs across validators: {per_node_reads}"
    post_expr = next(iter(post_results))
    assert '"A"' in post_expr and '"B"' in post_expr, f"the merged state lost a writer: {post_expr}"
    assert '"A"' not in base_expr and '"B"' not in base_expr, (
        f"the common base already carried a child value: {base_expr}"
    )
    step("reads", {"by_validator": per_node_reads, "base": base_expr})

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

    record["result"] = "PASS"
    persist()
    print(f"ok: evidence at {args.out}/fork-point.json")


if __name__ == "__main__":
    try:
        main()
    except AssertionError as error:
        print(f"FAIL: {error}", file=sys.stderr)
        sys.exit(1)
