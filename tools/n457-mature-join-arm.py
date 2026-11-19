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

**Two recovery shapes, and only one of them is supported.** By default the reset node is started with
`--bootstrap` *and* the devnet's validator key, so it signs from the moment it starts. That is the
shape #576 diagnosed: the node's parent set does not yet contain its own pre-reset blocks, so the
proposer can derive a sequence number the chain already used and sign a genuine second block. With
`--supported-recovery` the node is staged as a passive replicator instead (`DEVNET_RESET_PASSIVE=1`),
allowed to restore and catch up with **no** key, and only then activated with its key from a DAG that
holds its own history (`tools/devnet.sh activate`). The supported shape additionally requires that
the reset node's log contains no `equivocation detected` line. With `--unsupported-shape` the arm
runs the key-present shape and requires the fail-closed guard: if the reset node's own key
equivocates, it must log `this validator's key equivocated` and produce no further block; a run
where the race does not happen and the node converges also passes, because a race that did not
happen cannot falsify the guard.

Usage: tools/n457-mature-join-arm.py [--min-height 28] [--timeout 600] [--out DIR]
                                    [--supported-recovery | --unsupported-shape]
"""

import argparse
import importlib.util
import json
import os
import re
import subprocess
import sys
import time
from datetime import datetime

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

PREFIX = os.environ.get("DEVNET_PREFIX", "n457join")
CONTAINERS = [f"{PREFIX}-bootstrap"] + [f"{PREFIX}-validator-{i}" for i in (1, 2, 3)]
RESET_TARGET = f"{PREFIX}-validator-3"
RESET_INDEX = "3"
# The fail-closed sentence the node logs when this validator's own key has equivocated (#576).
HALT_LINE = "this validator's key equivocated"


def container_log(container):
    seen = run(["docker", "logs", container], check=False)
    return seen.stdout + seen.stderr


def run(cmd, check=True, env=None):
    return subprocess.run(cmd, check=check, text=True, capture_output=True, env=env)


def docker(*args, check=True):
    return run(["docker", *args], check=check)


def chain_tool():
    """Load the M3 join-chain checker without making the arm's import depend on it."""
    spec = importlib.util.spec_from_file_location(
        "n457_m3_join_chain", os.path.join(ROOT, "tools", "m3-join-chain.py")
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def build_join_chain(joiner_text, responder_text, *, joiner_source=None, responder_source=None):
    return chain_tool().build_chain(
        joiner_log=joiner_text, responder_log=responder_text,
        locality="local-devnet", joiner_container=RESET_TARGET,
        responder_container=CONTAINERS[0],
        joiner_source_path=joiner_source, responder_source_path=responder_source,
    )


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


def proposal_sequences(log):
    """`(block number, sequence)` for every successful local proposal in `log`.

    **Both groups stay whole.** The sequences are two digits from height 10 on, and a helper that
    unpacked the match into `(_, seq)` over one capture group would split "15" into "1" and "5" —
    a witness that reads 5 for a block that used 15. The arm records the raw lines beside these
    pairs so the two cannot disagree silently (found exactly that way, 2026-11-13)."""
    return [
        (int(block), int(seq))
        for block, seq in re.findall(
            r"proposed and added block #(\d+) \(seq (\d+)\)", log
        )
    ]


def equivocation_refusals(log):
    """The block processor's refusal lines — one per refused block, not one per mention.

    The own-key halt sentence quotes the DAG's reason, and the proposer status quotes the halt, so
    a bare substring count reports one offence three times. The processor's line is the event."""
    return [
        line
        for line in log.splitlines()
        if "equivocation detected" in line and "casper.blocks.BlockProcessor" in line
    ]


# How long after the halt an in-flight proposal may still complete. Autopropose produces roughly
# every two seconds, so a gate that did not hold would show a block well inside this window.
HALT_SETTLE_SECONDS = 10

TS = re.compile(r"^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3})Z")


def late_proposals_after_halt(log, settle_seconds=HALT_SETTLE_SECONDS):
    """Proposals that **complete more than `settle_seconds` after** the key-equivocation halt.

    A propose attempt already in flight when the halt is recorded completes just after it — in the
    measured shape the second block is 107 ms after the halt, and it *started* before it. What the
    halt must stop is production continuing: a block arriving seconds later means an attempt began
    after the gate was armed. The settle window is what separates the two."""
    halt_at = log.find(HALT_LINE)
    if halt_at < 0:
        return []
    halt_line_start = log.rfind("\n", 0, halt_at) + 1
    halt_match = TS.match(log[halt_line_start:])
    if not halt_match:
        return []
    halt_ts = datetime.fromisoformat(halt_match.group(1))
    late = []
    for line in log[halt_line_start:].splitlines():
        if "proposed and added block" not in line:
            continue
        stamp = TS.match(line)
        if stamp and (datetime.fromisoformat(stamp.group(1)) - halt_ts).total_seconds() > settle_seconds:
            late.append(line)
    return late


def wait_recovered(container, minimum, deadline, what):
    """Wait until a node has restored, left the sync surface, and reached `minimum`.

    A passive replica is held to the same height as a validator here: it receives and validates
    blocks like any peer; what it must not do is propose, which is the property the supported shape
    buys."""
    last_seen = {}
    while time.time() < deadline:
        try:
            st = status(container)
            fin = finalized(container)
            last_seen = {"status": st, "finalized": fin}
            if (
                st.get("latestBlockNumber", 0) >= minimum
                and st.get("sync") is None
                and fin.get("blockHash")
            ):
                return st, fin, last_seen
        except (subprocess.CalledProcessError, json.JSONDecodeError, KeyError):
            pass
        time.sleep(2)
    raise SystemExit(f"{what} within the deadline; last seen: {json.dumps(last_seen)[:600]}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--min-height", type=int, default=28)
    parser.add_argument("--timeout", type=int, default=600)
    parser.add_argument("--out", default="target/issue457-evidence/mature-join")
    parser.add_argument(
        "--supported-recovery",
        action="store_true",
        help="stage the reset validator passively (no key), wait for its catch-up, then activate "
        "its key and require convergence: the supported shape (#576). Without this the node signs "
        "from the moment it starts, which is the shape the issue diagnosed.",
    )
    parser.add_argument(
        "--unsupported-shape",
        action="store_true",
        help="run the key-present-from-boot shape and require the fail-closed guard: if the reset "
        "node's own key equivocates, it must refuse to propose with the named halt and produce no "
        "further block; a benign run that converges without an offence also passes (#576).",
    )
    args = parser.parse_args()
    if args.supported_recovery and args.unsupported_shape:
        parser.error("--supported-recovery and --unsupported-shape are different runs; pick one")

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
    if args.supported_recovery:
        reset_env["DEVNET_RESET_PASSIVE"] = "1"
        print(
            f"== reset {RESET_TARGET} (empty store, passive replicator, no key)",
            flush=True,
        )
    else:
        print(f"== reset {RESET_TARGET} (empty store, --bootstrap)", flush=True)
    # The responder-side half of the M3 chain is only the exchange that follows this reset. Docker
    # `--since` takes a Unix second; one second early, so the first line after the reset is never
    # cut off.
    reset_since = str(max(0, int(time.time()) - 1))
    run(["tools/devnet.sh", "reset", RESET_INDEX], env=reset_env)

    # **The supported recovery is two steps** (#576): restore and catch up with no signing key, so
    # the proposer cannot derive a sequence from a parent set that lacks the node's own pre-reset
    # blocks, then give the key back from a DAG that holds them.
    passive_seconds = None
    passive_text = None
    if args.supported_recovery:
        passive_started = time.time()
        _, _, last_seen = wait_recovered(
            RESET_TARGET,
            boot_status.get("latestBlockNumber", 0),
            deadline,
            "the passive replica did not restore and catch up to the bootstrap's height",
        )
        passive_seconds = time.time() - passive_started
        # **The passive phase's log dies with the container** (`activate` recreates it), so it is
        # captured here: it is the evidence that the node restored and followed with no signing key.
        passive_log = run(["docker", "logs", RESET_TARGET], check=False)
        passive_text = passive_log.stdout + passive_log.stderr
        os.makedirs(args.out, exist_ok=True)
        with open(os.path.join(args.out, "passive.log"), "w") as handle:
            handle.write(passive_text)
        print(
            f"== passive replica caught up in {passive_seconds:.1f}s; activating its key",
            flush=True,
        )
        run(["tools/devnet.sh", "activate", RESET_INDEX], env=reset_env)

    started = time.time()
    recovered = {}
    restored_anchor = {}
    halt_observed = False
    if args.unsupported_shape:
        # **The property is the guard, not convergence** (#576): wait for the reset node to refuse a
        # block by its own key — a genuine equivocation — and then for the named halt. A benign run
        # that converges without an offence passes too: a race that did not happen cannot falsify
        # the guard, and claiming it did would be a tuned green.
        while time.time() < deadline:
            seen = container_log(RESET_TARGET)
            if "equivocation detected" in seen:
                halt_by = min(deadline, time.time() + 120)
                while time.time() < halt_by:
                    if HALT_LINE in seen:
                        halt_observed = True
                        break
                    time.sleep(2)
                    seen = container_log(RESET_TARGET)
                # Let any in-flight proposal complete, and give a broken gate time to show itself.
                time.sleep(HALT_SETTLE_SECONDS + 2)
                break
            try:
                st = status(RESET_TARGET)
                if (
                    st.get("latestBlockNumber", 0)
                    >= boot_status.get("latestBlockNumber", 0)
                    and st.get("sync") is None
                ):
                    recovered = st
                    break
            except (subprocess.CalledProcessError, json.JSONDecodeError, KeyError):
                pass
            time.sleep(2)
    else:
        # **The claim under test is the restore plus the follow, not equality with a moving tip.** The
        # chain keeps producing while this runs, so the node is required to reach the height captured
        # before the reset, leave `NodeSyncing`, and hold a finalized block the bootstrap also holds.
        recovered, restored_anchor, last_seen = wait_recovered(
            RESET_TARGET,
            boot_status.get("latestBlockNumber", 0),
            deadline,
            "the reset node did not catch up to the bootstrap's height and finalized block",
        )
    elapsed = time.time() - started

    # The restored anchor must be a block the bootstrap holds: a node that left `NodeSyncing` on a
    # state the bootstrap never had is not a join.
    anchor_hex = str(restored_anchor.get("blockHash"))
    held_by_bootstrap = False
    if anchor_hex:
        try:
            held = get(CONTAINERS[0], f"/api/block/{anchor_hex}")
            held_by_bootstrap = bool(held.get("blockInfo"))
        except (subprocess.CalledProcessError, json.JSONDecodeError):
            held_by_bootstrap = False

    # **A second proposal makes the sequence witness non-vacuous** (a reactivated validator must not
    # reuse or regress its own sequence). Best-effort within two minutes: one proposal already
    # witnesses that the key serves, and the sequence check simply has nothing to compare below two.
    if args.supported_recovery:
        second_by = min(deadline, time.time() + 120)
        while time.time() < second_by:
            seen = run(["docker", "logs", RESET_TARGET], check=False)
            if (seen.stdout + seen.stderr).count("proposed and added block") >= 2:
                break
            time.sleep(2)

    # **One capture, written down, and every count read from it** — a witness counted from a second
    # `docker logs` call can disagree with the log beside it in the pack, which is a contradiction an
    # auditor cannot resolve. The container's identity travels with it for the same reason.
    logs = run(["docker", "logs", RESET_TARGET], check=False)
    combined = (logs.stdout + logs.stderr)
    os.makedirs(args.out, exist_ok=True)
    with open(os.path.join(args.out, "activated.log"), "w") as handle:
        handle.write(combined)

    # **The M3 six-stage chain** (#428). Correlate the joiner's restore with the responder's
    # segment since the reset. Best-effort and outside this arm's exit semantics: an incomplete
    # chain is written with its reasons and the checker (`tools/m3-join-chain.py`) decides.
    responder_log = docker("logs", "--since", reset_since, CONTAINERS[0], check=False)
    responder_text = responder_log.stdout + responder_log.stderr
    chain_log_method = "since"
    if responder_log.returncode != 0 or not responder_text.strip():
        responder_text = container_log(CONTAINERS[0])
        chain_log_method = "full"
    # Preserve both exact inputs to the M3 correlation; responder-side raw
    # log is necessary for independent stage 2-4 verification after packing.
    joiner_text = passive_text if args.supported_recovery and passive_text else combined
    # **The chain names these files and the checker binds every quoted line to them.** The
    # responder segment is otherwise lost when the container is recreated, and the joiner's own
    # name distinguishes the supported-recovery replay from the ordinary activation log.
    joiner_name = "passive.log" if args.supported_recovery and passive_text else "activated.log"
    with open(os.path.join(args.out, joiner_name), "w") as handle:
        handle.write(joiner_text)
    with open(os.path.join(args.out, "responder.log"), "w") as handle:
        handle.write(responder_text)
    chain = build_join_chain(
        joiner_text, responder_text,
        joiner_source=f"raw/{joiner_name}", responder_source="raw/responder.log",
    )
    with open(os.path.join(args.out, "chain.json"), "w") as handle:
        json.dump(chain, handle, indent=2, sort_keys=True)
    container = run(
        ["docker", "inspect", "-f", "{{.Id}} {{.Created}}", RESET_TARGET], check=False
    ).stdout.strip()
    sync_failures = combined.count("LFS state sync failed")
    give_ups = combined.count("no block finished in")
    # **Count refusals, not the phrase**: the halt sentence quotes the DAG's reason, so a bare
    # substring count reports one offence three times (the refusal, the halt, the proposer status).
    equivocations = len(equivocation_refusals(combined))
    # **The fail-closed witness** (#576): the halt sentence, and whether any proposal follows it.
    halt_at = combined.find(HALT_LINE)
    halt_in_log = halt_at >= 0
    proposals_after_halt = (
        combined[halt_at:].count("proposed and added block") if halt_in_log else 0
    )
    late_proposals = late_proposals_after_halt(combined)
    # **The sequence witness the reactivation test already uses**: a validator that re-signs one of
    # its own sequences shows it here even if the DAG refusal happened to be quiet.
    proposals = proposal_sequences(combined)
    proposed = len(proposals)
    sequences = [seq for _, seq in proposals]
    proposed_lines = [
        f"proposed and added block #{block} (seq {seq})" for block, seq in proposals
    ]

    record = {
        "boundary": "finite local devnet witness; not a live-testnet join and not a certification",
        "chain_height": heights,
        "bootstrap_finalized": {
            "number": boot_final.get("blockNumber"),
            "hash": boot_final.get("blockHash"),
        },
        "reset_target": RESET_TARGET,
        "recovery_shape": (
            "passive-then-activate"
            if args.supported_recovery
            else "key-present-fail-closed"
            if args.unsupported_shape
            else "key-present-from-boot"
        ),
        "passive_catch_up_seconds": passive_seconds,
        "own_key_halt_in_log": halt_in_log,
        "proposals_after_halt": proposals_after_halt,
        "late_proposals_after_halt": late_proposals,
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
        "reset_container_id_created": container,
        "self_equivocations_in_log": equivocations,
        "blocks_proposed_in_log": proposed,
        "proposed_sequences": sequences,
        "proposed_lines": proposed_lines,
        "manual_reset": True,
        "chain": "chain.json",
        "chain_complete": chain["complete"],
        "chain_log_method": chain_log_method,
        "non_claims": [
            "one local four-validator devnet; a finite witness, not a universal join theorem",
            "the chain is a log correlation, not a cryptographic delivery receipt",
            "a local-devnet chain is not an M3 off-host positive",
            "the chain is this build's own; the live testnet replay is #421/#429's",
            (
                "the reset node is staged as a passive replicator and then activated, the "
                "supported recovery (#576)"
                if args.supported_recovery
                else "the reset node is started with `--bootstrap` and the devnet's validator key, "
                "the unsupported shape #576 diagnosed"
            ),
        ],
    }
    os.makedirs(args.out, exist_ok=True)
    with open(os.path.join(args.out, "mature-join.json"), "w") as handle:
        json.dump(record, handle, indent=2)

    problems = []
    if sync_failures:
        problems.append(f"the reset node logged {sync_failures} LFS sync failure(s)")
    if args.unsupported_shape:
        # **The guard is the claim** (#576): an offence must be followed by the named halt and no
        # further proposal; a run with no offence must have converged, or the arm measured nothing.
        if halt_in_log:
            # A propose attempt in flight when the halt lands completes just after it; production
            # continuing past the settle window is the gate not holding.
            if late_proposals:
                problems.append(
                    "the node produced block(s) more than "
                    f"{HALT_SETTLE_SECONDS}s after the key-equivocation halt: {late_proposals}"
                )
        elif equivocations:
            problems.append(
                f"the node equivocated {equivocations} time(s) without the named halt"
            )
        elif not recovered:
            problems.append(
                "neither convergence nor a key-equivocation halt was observed within the deadline"
            )
    else:
        if recovered.get("sync") is not None:
            problems.append("the sync surface is not cleared")
        if not held_by_bootstrap:
            problems.append("the restored anchor is not a block the bootstrap holds")
        if args.supported_recovery and equivocations:
            problems.append(
                f"the supported shape logged {equivocations} equivocation line(s); the staging did "
                "not keep the proposer off a sequence the chain already used (#576)"
            )
        if args.supported_recovery and proposed == 0:
            problems.append(
                "the activated validator proposed no block, so the arm does not witness it serving"
            )
        if args.supported_recovery and any(
            b <= a for a, b in zip(sequences, sequences[1:])
        ):
            problems.append(
                f"the activated validator reused or regressed its own sequence: {sequences}"
            )
        if args.supported_recovery and halt_in_log:
            problems.append(
                "the supported shape halted on a key equivocation, which it must not produce"
            )
    print(
        f"({record['recovery_shape']}) in {elapsed:.1f}s: "
        f"round={recovered.get('latestBlockNumber')}, "
        f"anchor {restored_anchor.get('blockNumber')} "
        f"({anchor_hex[:12] or 'none'}, held by bootstrap={held_by_bootstrap}); "
        f"sync={recovered.get('sync')}; sync failures={sync_failures}; "
        f"self-equivocations={equivocations}; halted={halt_in_log}; "
        f"proposed={proposed}; after-halt={proposals_after_halt}; "
        f"late-after-halt={len(late_proposals)}",
        flush=True,
    )
    if problems:
        for problem in problems:
            print(f"FAIL: {problem}", file=sys.stderr)
        sys.exit(1)
    print(f"ok: evidence at {args.out}/mature-join.json")


if __name__ == "__main__":
    main()
