#!/usr/bin/env python3
"""Issue #447: two real validators race to update one linear Rholang cell.

The node, network, stakes and keys must belong to a disposable Docker devnet.
A split is not a safety proof; report observed counter state and rejected ids.
"""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import sys
import time
import traceback

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))
SPEC = importlib.util.spec_from_file_location("fork_point_447", TOOLS / "n420-fork-point.py")
fork = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(fork)
from n447_partition import DockerBackend, SplitNetwork

CONTAINERS = fork.CONTAINERS
CELL = '@"n447_linear_cell"'
A = 'for (@old <- ' + CELL + ') { ' + CELL + '!(old + 1) | @"n447_effect_a"!("A") }'
B = 'for (@old <- ' + CELL + ') { ' + CELL + '!(old + 2) | @"n447_effect_b"!("B") }'


def conflict_terms(costlier_writer=None):
    # Additional runtime arithmetic changes cost, not the resulting cell value.
    # Cost and actor placement vary the fixtures without prescribing a global winner.
    terms = {"A": A, "B": B}
    if costlier_writer is not None:
        if costlier_writer not in terms:
            raise ValueError("unknown costlier writer")
        old = "old + 1" if costlier_writer == "A" else "old + 2"
        terms[costlier_writer] = terms[costlier_writer].replace(
            old, old + " + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0")
    return terms["A"], terms["B"]


def find_signed_carrier(node, term, minimum_height):
    """Find the containing block by signed term, not by the current tip."""
    # The block-list API rejects depths > 50 with HTTP 400. The generic
    # fork.blocks helper would turn its error envelope into an empty list,
    # falsely reporting that a signed deploy had not entered any block.
    # Keep the request in range and reject error envelopes explicitly.
    inventory = fork.request(node, "/api/blocks/25")
    if isinstance(inventory, dict):
        inventory = inventory.get("blocks") if "error" not in inventory else None
    if not isinstance(inventory, list):
        raise AssertionError("block-list API returned an error or invalid inventory")
    candidates = []
    for entry in inventory:
        if isinstance(entry, dict) and "blockHash" in entry:
            candidates.append(entry)
        elif isinstance(entry, str) and len(entry) == 64 and all(
                c in "0123456789abcdefABCDEF" for c in entry):
            candidates.append(fork.block_info(node, entry))
        # Any other item is non-block diagnostic material and cannot count
        # toward inclusion. The surrounding bounded deadline still fails.
    for block in sorted(candidates, key=lambda b: int(b["blockNumber"])):
        if (int(block["blockNumber"]) <= minimum_height
                or int(block.get("deployCount", 0)) < 1):
            continue
        body = fork.request(node, "/api/block/" + block["blockHash"])
        if not isinstance(body, dict) or not isinstance(body.get("deploys"), list):
            raise AssertionError(f"malformed block deploy inventory: {body!r}")
        matches = [d["sig"] for d in body["deploys"]
                   if isinstance(d, dict) and d.get("term") == term]
        if len(matches) == 1:
            return block, matches[0]
        if len(matches) > 1:
            raise AssertionError("duplicate matching signed terms in one block")
    return None, None


def expr(container, channel, block_hash):
    """Read all tuple-space data at the pinned name, not one consumed value.

    An exploratory Rholang for consumes one arbitrary datum and exposes
    just that one reply. It cannot prove cell cardinality: a merged state
    containing both 0 and 1 could appear to contain just 1.
    The named-data API returns the entire collection at the exact block
    post-state, including duplicate produces, without consuming anything.
    """
    if channel not in ("n447_linear_cell", "n447_effect_a", "n447_effect_b"):
        raise AssertionError(f"unexpected application channel: {channel!r}")
    response = fork.request(container, "/api/v1/data-at-name-by-block-hash", {
        "name": {"ExprString": {"data": channel}},
        "blockHash": block_hash,
        "usePreStateHash": False,
    })
    if not isinstance(response, dict) or not isinstance(response.get("expr"), list):
        raise AssertionError(f"invalid pinned name read: {response!r}")
    if not isinstance(response.get("block"), dict) or (
            response["block"].get("blockHash") != block_hash):
        raise AssertionError(
            f"pinned name read did not return the requested block {block_hash}"
        )
    return response["expr"]


def require_finalized_agreement(views, merge_hash, expected_state):
    if set(views) != set(CONTAINERS):
        raise AssertionError("missing validator finalized observation")
    hashes = {v.get("blockHash") for v in views.values()}
    if len(hashes) != 1 or None in hashes:
        raise AssertionError("validators have different finalized blocks")
    block_hash = next(iter(hashes))
    fork.merge_state_agreement(views, CONTAINERS, block_hash)
    for node in CONTAINERS:
        if block_hash != merge_hash:
            verify_branch_descends_from_seed(
                lambda h: fork.block_info(node, h), block_hash, merge_hash)
        if pinned_application_snapshot(node, block_hash) != expected_state:
            raise AssertionError("finalized application state differs from accepted winner")
    return block_hash


def contains_marker(expressions, marker):
    """A surviving effect must be exactly one marker, with no extra produces."""
    if not isinstance(expressions, list):
        raise AssertionError("invalid effect readback")
    if not expressions:
        return False
    expected = [{"ExprString": {"data": marker}}]
    if expressions != expected:
        raise AssertionError(
            f"effect channel has duplicate, unexpected, or malformed values: {expressions!r}"
        )
    return True


def classify_effects(a, b, rejected, id_a, id_b):
    """Return the sole surviving writer, refusing impossible partial-effect mixes."""
    if len(rejected) != 1 or not rejected.issubset({id_a, id_b}):
        raise AssertionError(f"expected exactly one of two conflicting signatures: {rejected!r}")
    if a == b or not (a or b):
        raise AssertionError(f"exactly one application effect must remain (a={a}, b={b})")
    kept = "A" if a else "B"
    expected_rejected = id_b if a else id_a
    if rejected != {expected_rejected}:
        raise AssertionError("rejectedDeploys does not match the missing application effect")
    return kept


def require_single_cell_value(expressions, survivor):
    """Prove the surviving update, not just the presence of one marker."""
    expected = {"A": 1, "B": 2}.get(survivor)
    if expected is None or not isinstance(expressions, list) or len(expressions) != 1:
        raise AssertionError("the conflict left a missing or non-single cell value")
    item = expressions[0]
    actual = item.get("ExprInt", {}).get("data") if isinstance(item, dict) else None
    if type(actual) is not int or actual != expected:
        raise AssertionError(f"linear cell value {actual!r} differs from expected {expected}")
    return actual


def require_divergent_signed_tips(tip_a, tip_b, seed_height, sig_a, sig_b):
    """A real partition can advance groups at different local block heights.

    Height equality is not a requirement for two causally independent forks.
    The signed blocks must both follow a common seed, be different blocks,
    and carry different signed deploy identities; partition checks separately
    ensure no route between the two groups.
    """
    if not all(isinstance(tip, dict) for tip in (tip_a, tip_b)):
        raise AssertionError("missing signed fork tip")
    if not (int(tip_a["blockNumber"]) > int(seed_height) and
            int(tip_b["blockNumber"]) > int(seed_height)):
        raise AssertionError("conflicting signed tips did not advance beyond common seed")
    if tip_a["blockHash"] == tip_b["blockHash"]:
        raise AssertionError("conflicting deploys unexpectedly shared one block")
    if not sig_a or not sig_b or sig_a == sig_b:
        raise AssertionError("fork signatures missing or duplicated")
    return {"same_height": tip_a["blockNumber"] == tip_b["blockNumber"],
            "heights": {"A": tip_a["blockNumber"], "B": tip_b["blockNumber"]},
            "pre_roots_equal": tip_a.get("preStateHash") == tip_b.get("preStateHash")}


def verify_branch_descends_from_seed(fetch_block, branch, seed, max_blocks=400):
    """An independent signed fork must descend from the actual seeded block.

    Numeric height > seed height is not proof of common ancestry. Check the
    exact block-hash justification graph before claiming a shared cell was
    consumed on both sides of a split.
    """
    from collections import deque
    if not branch or not seed or branch == seed:
        raise AssertionError("invalid branch/seed ancestry target")
    queue = deque([(branch, [branch])])
    visited = set()
    while queue:
        block_hash, path = queue.popleft()
        if block_hash in visited:
            continue
        if len(visited) >= max_blocks:
            raise AssertionError("seed ancestry exceeds bounded block traversal")
        visited.add(block_hash)
        block = fetch_block(block_hash)
        if not isinstance(block, dict) or block.get("blockHash") != block_hash:
            raise AssertionError(f"invalid block in common-seed ancestry: {block_hash}")
        parents = block.get("justifications")
        if not isinstance(parents, list) or any(
                not isinstance(h, str) or not h for h in parents):
            raise AssertionError("invalid justification set in common-seed ancestry")
        if block_hash == seed:
            return path
        for parent in parents:
            if parent not in visited and parent not in path:
                queue.append((parent, path + [parent]))
    raise AssertionError("signed fork does not descend from the common seed")


def pinned_application_snapshot(node, block_hash):
    """Capture the actual committed cell and both markers at one block root."""
    return {
        "cell": expr(node, "n447_linear_cell", block_hash),
        "a": expr(node, "n447_effect_a", block_hash),
        "b": expr(node, "n447_effect_b", block_hash),
    }


def require_branch_tip_state(snapshot, side):
    """A merge cannot be blamed if a branch never executed its own write."""
    if side not in ("A", "B"):
        raise AssertionError("invalid branch label")
    require_single_cell_value(snapshot["cell"], side)
    expected = side
    wrong = "B" if side == "A" else "A"
    if not contains_marker(snapshot[side.lower()], expected):
        raise AssertionError("branch missing its own application marker")
    if contains_marker(snapshot[wrong.lower()], wrong):
        raise AssertionError("branch already contains competing marker")
    return True


def wait_for_block_on_all(block_hash, seconds=120):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        try:
            views = {name: fork.block_info(name, block_hash) for name in CONTAINERS}
            fork.merge_state_agreement(views, CONTAINERS, block_hash)
            return views
        except Exception:
            time.sleep(2)
    raise AssertionError("four-validator merged-block/state agreement not observed")


def brief_block_evidence(info, fork_tips):
    """Keep justifications and rejection IDs without repeating bond tables."""
    justifications = set(info.get("justifications") or [])
    return {
        "height": info.get("blockNumber"),
        "hash": info.get("blockHash"),
        "parents": len(justifications),
        "fork_tips_directly_justified": sorted(justifications & set(fork_tips)),
        "rejected_deploys": info.get("rejectedDeploys", [])[:12],
        "post_state_hash": info.get("postStateHash"),
    }


def causal_ancestor_paths(fetch_block, head, targets, max_blocks=1200):
    """Bounded DAG ancestry witness: each fork tip must be an ancestor of head.

    A later merge need not list the original fork tips as immediate parents.
    Traversal follows the block's actual justification hashes, never height
    proximity or a rejected-deploy field in isolation. No complete ancestry,
    no accepted causal-merge witness.
    """
    from collections import deque

    targets = set(targets)
    if not isinstance(head, str) or not head or len(targets) != 2:
        raise AssertionError("causal witness needs one block and two distinct tips")
    queue = deque([(head, (head,))])
    visited = set()
    found = {}
    while queue and len(found) < 2:
        block_hash, trail = queue.popleft()
        if block_hash in visited:
            continue
        if len(visited) >= max_blocks:
            raise AssertionError("causal ancestry traversal exceeded its block bound")
        visited.add(block_hash)
        if block_hash in targets:
            found[block_hash] = list(trail)
            continue
        block = fetch_block(block_hash)
        if not isinstance(block, dict) or block.get("blockHash") != block_hash:
            raise AssertionError(f"invalid block response in ancestry path: {block_hash}")
        parents = block.get("justifications")
        if not isinstance(parents, list) or any(
                not isinstance(item, str) or not item for item in parents):
            raise AssertionError(f"malformed DAG justifications at {block_hash}")
        for parent in parents:
            if parent not in visited and parent not in trail:
                queue.append((parent, trail + (parent,)))
    return found


def scan_causal_conflict(node, minimum_height, tips, ids, fetch_block=None,
                         inventory=None):
    """Find a later block that both descends from both tips and rejects one ID.

    Reject-only or height-only hints are insufficient; the ancestry chain
    must be independently traversable from the candidate's own block hash.
    """
    fetch_block = fetch_block or (lambda h: fork.block_info(node, h))
    inventory = inventory if inventory is not None else fork.blocks(node, 50)
    if not isinstance(inventory, list):
        raise AssertionError("invalid post-heal block list")
    cache = {}

    def cached(h):
        if h not in cache:
            cache[h] = fetch_block(h)
        return cache[h]

    suspects = []
    for item in sorted(inventory, key=lambda v: v.get("blockNumber", -1)
                       if isinstance(v, dict) else -1):
        if not isinstance(item, dict):
            continue
        if int(item.get("blockNumber", -1)) <= minimum_height:
            continue
        matching = set(item.get("rejectedDeploys") or []) & set(ids)
        if len(matching) != 1:
            continue
        block_hash = item.get("blockHash")
        if not isinstance(block_hash, str) or not block_hash:
            continue
        try:
            paths = causal_ancestor_paths(cached, block_hash, tips)
        except (AssertionError, KeyError) as error:
            suspects.append({"hash": block_hash, "invalid_ancestry": str(error)})
            continue
        if set(paths) == set(tips):
            return item, paths, suspects
        suspects.append({"hash": block_hash,
                         "reachable_tips": sorted(paths),
                         "matching_rejections": sorted(matching)})
    return None, {}, suspects


def post_heal_diagnostics(fork_tips, minimum_height):
    """A missing direct merge needs node-level evidence, not a generic timeout."""
    result = {}
    for node in CONTAINERS:
        try:
            inventory = [
                brief_block_evidence(item, fork_tips)
                for item in fork.blocks(node, 50)
                if isinstance(item, dict)
                and int(item.get("blockNumber", -1)) > minimum_height
            ]
            inventory.sort(key=lambda item: (item["height"], item["hash"] or ""))
            result[node] = {
                "observed_block_count": len(inventory),
                "direct_justification_count": sum(
                    len(item["fork_tips_directly_justified"]) == 2 for item in inventory
                ),
                "recent_blocks": inventory[-20:],
                "status": {
                    key: fork.status(node).get(key)
                    for key in ("latestBlockNumber", "peers", "acceptsDeploys")
                },
            }
        except Exception as error:
            result[node] = {"diagnostic_error": f"{type(error).__name__}: {error}"}
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True)
    parser.add_argument("--timeout", type=int, default=480)
    parser.add_argument("--costlier-writer", choices=("A", "B"))
    parser.add_argument("--expect-winner", choices=("A", "B"))
    parser.add_argument("--swap-writers", action="store_true")
    args = parser.parse_args(argv)
    term_a, term_b = conflict_terms(args.costlier_writer)
    actor_a, actor_b = ((CONTAINERS[2], CONTAINERS[0]) if args.swap_writers
                        else (CONTAINERS[0], CONTAINERS[2]))
    p = Path(args.out)
    p.parent.mkdir(parents=True, exist_ok=True)
    record = {"result": "INCOMPLETE",
              "boundary": "finite 2+2 conflict on a disposable Docker devnet",
              "steps": []}

    def step(name, data):
        record["steps"].append({"stage": name, "value": data})
        p.write_text(json.dumps(record, indent=2, sort_keys=True) + "\n")
        print("STAGE:", name, flush=True)

    try:
        fork.wait_all(CONTAINERS, args.timeout)
        baseline = fork.tip(CONTAINERS[0])
        step("genesis", baseline)
        step("fixture", {"costlier_writer": args.costlier_writer,
                         "expected_winner": args.expect_winner,
                         "actor_a": actor_a, "actor_b": actor_b,
                         "term_a": term_a, "term_b": term_b})

        # Seed one ordinary linear datum, which BOTH sides will later consume.
        seed_term = CELL + "!(0)"
        fork.deploy(CONTAINERS[0], seed_term, baseline["blockNumber"])
        deadline = time.monotonic() + min(args.timeout, 90)
        seed = None
        while time.monotonic() < deadline:
            fork.propose(CONTAINERS[0])
            seed, seed_sig = find_signed_carrier(
                CONTAINERS[0], seed_term, baseline["blockNumber"])
            if seed:
                break
            time.sleep(2)
        if seed is None:
            candidates = []
            for block in fork.blocks(CONTAINERS[0], 25):
                if isinstance(block, dict) and int(block.get("deployCount", 0)) > 0:
                    body = fork.request(CONTAINERS[0], "/api/block/" + block["blockHash"])
                    candidates.append({"blockHash": block["blockHash"],
                                       "blockNumber": block["blockNumber"],
                                       "deployCount": block["deployCount"],
                                       "deploys": [{"term": dep.get("term"),
                                                    "sig": dep.get("sig"),
                                                    "errored": dep.get("errored")}
                                                   for dep in body.get("deploys", [])]})
            step("seed_inclusion_diagnostics", candidates)
            raise AssertionError("unable to seed a linear cell in a carrying block")
        step("seed_signature", seed_sig)
        step("seed", seed)
        for node in CONTAINERS:
            end = time.monotonic() + 60
            while time.monotonic() < end:
                try:
                    assert fork.block_info(node, seed["blockHash"])["blockHash"] == seed["blockHash"]
                    break
                except Exception:
                    time.sleep(2)
            else:
                raise AssertionError(f"seed block not replicated before split: {node}")
        step("seed_replicated", True)
        seed_cell = expr(CONTAINERS[0], "n447_linear_cell", seed["blockHash"])
        if seed_cell != [{"ExprInt": {"data": 0}}]:
            raise AssertionError(f"the common pre-fork linear cell is not exactly zero: {seed_cell}")
        seed_markers = {
            "a": expr(CONTAINERS[0], "n447_effect_a", seed["blockHash"]),
            "b": expr(CONTAINERS[0], "n447_effect_b", seed["blockHash"]),
        }
        if seed_markers["a"] or seed_markers["b"]:
            raise AssertionError("pre-fork seed already contains a conflicting effect")
        step("seed_state", {"cell": seed_cell, "effects": seed_markers})

        # Observing a seed block is not the same as adopting it: concurrent
        # seed carriers can leave a pair on another causal history. Finalize
        # the shared seed before partitioning, retaining the ancestry gate.
        seed_expected = {"cell": seed_cell, **seed_markers}
        seed_deadline = time.monotonic() + min(args.timeout, 180)
        seed_errors = []
        seed_round = 0
        while time.monotonic() < seed_deadline:
            try:
                seed_final_views = {}
                for node in CONTAINERS:
                    response = fork.request(node, "/api/last-finalized-block")
                    seed_final_views[node] = response.get("blockInfo", response)
                seed_final_hash = require_finalized_agreement(
                    seed_final_views, seed["blockHash"], seed_expected)
                step("seed_finalized", {"hash": seed_final_hash,
                                        "views": seed_final_views, "readback": seed_expected})
                break
            except Exception as error:
                seed_errors.append(f"{type(error).__name__}: {error}")
            fork.propose(CONTAINERS[seed_round % len(CONTAINERS)])
            seed_round += 1
            time.sleep(2)
        else:
            step("seed_finality_failure", {"errors": seed_errors[-12:]})
            raise AssertionError("common seed was not finalized before partition")

        with SplitNetwork(DockerBackend(), prefix=fork.PREFIX, main=fork.NETWORK) as topology:
            pair_reachability = [
                fork.probe_transport(CONTAINERS[0], CONTAINERS[1]),
                fork.probe_transport(CONTAINERS[1], CONTAINERS[0]),
                fork.probe_transport(CONTAINERS[2], CONTAINERS[3]),
                fork.probe_transport(CONTAINERS[3], CONTAINERS[2]),
            ]
            across = [
                fork.probe_transport(CONTAINERS[0], CONTAINERS[2]),
                fork.probe_transport(CONTAINERS[2], CONTAINERS[0]),
            ]
            step("partition", {"topology": topology.require_partition(),
                               "intra_pair_tcp": pair_reachability,
                               "cross_pair_tcp": across})
            if not all(pair_reachability) or any(across):
                raise AssertionError("network topology is not a true 2+2 split")
            fork.deploy(actor_a, term_a, seed["blockNumber"])
            fork.deploy(actor_b, term_b, seed["blockNumber"])
            fork.propose(actor_a)
            fork.propose(actor_b)
            end = time.monotonic() + args.timeout
            while time.monotonic() < end:
                tip_a, sig_a = find_signed_carrier(
                    actor_a, term_a, seed["blockNumber"])
                tip_b, sig_b = find_signed_carrier(
                    actor_b, term_b, seed["blockNumber"])
                if tip_a and tip_b and tip_a["blockHash"] != tip_b["blockHash"]:
                    break
                fork.propose(actor_a)
                fork.propose(actor_b)
                time.sleep(2)
            else:
                step("signed_fork_not_observed", {
                    "last_A": tip_a, "last_B": tip_b,
                    "latest_A": fork.tip(actor_a),
                    "latest_B": fork.tip(actor_b),
                })
                raise AssertionError("both conflicting signed deploys did not produce independent fork tips")
            fork_distinction = require_divergent_signed_tips(
                tip_a, tip_b, seed["blockNumber"], sig_a, sig_b
            )
            step("tips", {"a": tip_a, "b": tip_b,
                          "id_a": sig_a, "id_b": sig_b,
                          "fork_distinction": fork_distinction})
            # Prove that both supposedly independent branches actually
            # descend from the same seeded state. An unequal block height
            # cannot substitute for a DAG ancestry witness.
            branch_parents = {
                "A": verify_branch_descends_from_seed(
                    lambda h: fork.block_info(actor_a, h),
                    tip_a["blockHash"], seed["blockHash"]),
                "B": verify_branch_descends_from_seed(
                    lambda h: fork.block_info(actor_b, h),
                    tip_b["blockHash"], seed["blockHash"]),
            }
            step("branch_common_seed_ancestry", branch_parents)
            # An executed marker alone is insufficient: capture the *cell*
            # produced by each signed branch before restoring gossip.
            # The previously observed inconsistency was A-marker present
            # while merged cell retained the seed value zero.
            branch_state = {
                "A": pinned_application_snapshot(actor_a, tip_a["blockHash"]),
                "B": pinned_application_snapshot(actor_b, tip_b["blockHash"]),
            }
            step("branch_tip_state", {
                "tips": {"A": tip_a["blockHash"], "B": tip_b["blockHash"]},
                "states": branch_state,
            })
            for branch, snapshot in branch_state.items():
                require_branch_tip_state(snapshot, branch)
            step("branch_tip_assertions", {"A": True, "B": True})

        step("healed", {node: sorted(DockerBackend().members(node)) for node in CONTAINERS})
        target = [tip_a["blockHash"], tip_b["blockHash"]]
        end = time.monotonic() + min(args.timeout, 120)
        while time.monotonic() < end:
            views = fork.observe_cross_tip_gossip(CONTAINERS, target)
            if fork.all_tips_visible(views):
                break
            time.sleep(2)
        else:
            step("gossip_missing", views)
            raise AssertionError("disconnected groups did not exchange their tips after healing")
        step("gossip", views)

        # The fork tips may become *ancestors*, not direct parents, of the
        # later merge. Accept only a real rejected-deploy block for which
        # both causal paths can be independently traced through the DAG.
        end = time.monotonic() + args.timeout
        merge = None
        ancestry = {}
        attempts = []
        rejected_hints = []
        while time.monotonic() < end and merge is None:
            actor = CONTAINERS[len(attempts) % len(CONTAINERS)]
            proposed = fork.propose(actor)
            attempts.append({
                "validator": actor,
                "exit": proposed.returncode,
                "stdout_tail": proposed.stdout[-240:],
                "stderr_tail": proposed.stderr[-240:],
            })
            merge, ancestry, rejected_hints = scan_causal_conflict(
                CONTAINERS[0], max(tip_a["blockNumber"], tip_b["blockNumber"]),
                target, [sig_a, sig_b])
            if merge is None:
                time.sleep(2)
        step("post_heal_proposals", {
            "attempts": len(attempts), "by_validator": attempts[-24:],
        })
        if merge is None:
            diagnostics = post_heal_diagnostics(target, max(tip_a["blockNumber"], tip_b["blockNumber"]))
            step("merge_not_observed", diagnostics)
            step("rejection_hints_without_full_ancestry", rejected_hints[-30:])
            raise AssertionError(
                "no causally witnessed rejection block after healing; "
                "matching rejection alone cannot prove a valid conflict merge"
            )
        merge_hash = merge["blockHash"]
        step("causal_merge", {
            "hash": merge_hash,
            "height": merge["blockNumber"],
            "original_fork_tips": target,
            "ancestry_paths": ancestry,
            "matching_rejected_deploys": sorted(
                set(merge.get("rejectedDeploys") or []) & {sig_a, sig_b}),
        })
        views = wait_for_block_on_all(merge_hash)
        step("merge", {"hash": merge_hash, "views": views})
        rejected = set(merge["rejectedDeploys"]) & {sig_a, sig_b}
        step("rejections", {"all": merge["rejectedDeploys"], "matching": sorted(rejected)})

        states = {}
        for node in CONTAINERS:
            states[node] = {
                "a": expr(node, "n447_effect_a", merge_hash),
                "b": expr(node, "n447_effect_b", merge_hash),
                "cell": expr(node, "n447_linear_cell", merge_hash),
            }
        step("readback", states)
        if len({json.dumps(state, sort_keys=True) for state in states.values()}) != 1:
            raise AssertionError("validator readbacks differ")
        first = states[CONTAINERS[0]]
        survived = classify_effects(
            contains_marker(first["a"], "A"),
            contains_marker(first["b"], "B"),
            rejected, sig_a, sig_b,
        )
        cell_value = require_single_cell_value(first["cell"], survived)
        step("winner", {"survivor": survived, "cell_value": cell_value})
        if args.expect_winner is not None and survived != args.expect_winner:
            raise AssertionError("the weighted conflict did not retain the expected winner")

        final_deadline = time.monotonic() + args.timeout
        final_errors = []
        finalized_hash = None
        final_attempt = 0
        while time.monotonic() < final_deadline:
            try:
                final_views = {}
                for node in CONTAINERS:
                    response = fork.request(node, "/api/last-finalized-block")
                    final_views[node] = response.get("blockInfo", response)
                finalized_hash = require_finalized_agreement(final_views, merge_hash, first)
                step("finalized_agreement", {"hash": finalized_hash, "views": final_views,
                                            "readback": first})
                break
            except Exception as error:
                final_errors.append(f"{type(error).__name__}: {error}")
            fork.propose(CONTAINERS[final_attempt % len(CONTAINERS)])
            final_attempt += 1
            time.sleep(2)
        if finalized_hash is None:
            step("finality_failure", {"errors": final_errors[-12:],
                                     "views": final_views,
                                     "merge_hash": merge_hash,
                                     "proposals": final_attempt,
                                     "diagnostics": post_heal_diagnostics(
                                         target, max(tip_a["blockNumber"], tip_b["blockNumber"]))})
            raise AssertionError("four-validator finalized block/state agreement not observed")

        # Re-open the exact same accepted block and application state after a
        # hard validator container restart. The block hash is pinned: reading
        # an unrelated new tip would not demonstrate durable recovery.
        restarted = CONTAINERS[-1]
        fork.docker("restart", restarted)
        recovered = False
        recovery_errors = []
        recovery_deadline = time.monotonic() + 90
        while time.monotonic() < recovery_deadline:
            try:
                observed = fork.block_info(restarted, merge_hash)
                fork.merge_state_agreement(
                    {restarted: observed, **{
                        node: views[node] for node in CONTAINERS if node != restarted
                    }}, CONTAINERS, merge_hash
                )
                replay = {
                    "a": expr(restarted, "n447_effect_a", merge_hash),
                    "b": expr(restarted, "n447_effect_b", merge_hash),
                    "cell": expr(restarted, "n447_linear_cell", merge_hash),
                }
                if replay != first:
                    raise AssertionError("restart changed the pinned application state")
                recovered_final = fork.block_info(restarted, finalized_hash)
                fork.merge_state_agreement(
                    {**final_views, restarted: recovered_final}, CONTAINERS, finalized_hash)
                if pinned_application_snapshot(restarted, finalized_hash) != first:
                    raise AssertionError("restart changed the finalized application state")
                recovered = True
                step("durable_restart", {
                    "validator": restarted, "block_hash": merge_hash,
                    "finalized_block_hash": finalized_hash,
                    "pre_state_hash": observed["preStateHash"],
                    "post_state_hash": observed["postStateHash"],
                    "readback": replay,
                })
                break
            except Exception as error:
                recovery_errors.append(f"{type(error).__name__}: {error}")
                time.sleep(2)
        if not recovered:
            step("restart_failure", {"errors": recovery_errors[-8:]})
            raise AssertionError("validator restart did not restore the pinned merge block")
        record["result"] = "PASS"
    except Exception as error:
        record["error"] = f"{type(error).__name__}: {error}"
        record["traceback"] = traceback.format_exc()
        try:
            record["last_block_inventory"] = fork.blocks(CONTAINERS[0], 15)
        except Exception as diagnostic_error:
            record["inventory_error"] = f"{type(diagnostic_error).__name__}: {diagnostic_error}"
        raise
    finally:
        p.write_text(json.dumps(record, indent=2, sort_keys=True) + "\n")
        print("same-channel verdict", record["result"], "evidence", p, flush=True)


if __name__ == "__main__":
    try:
        main()
    except Exception:
        traceback.print_exc()
        sys.exit(1)
