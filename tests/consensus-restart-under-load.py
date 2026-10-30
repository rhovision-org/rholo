#!/usr/bin/env python3
"""Restart-under-load: kill one validator mid-burst, restart it, require exact agreement.

This is the end-to-end falsifier for Law 51d (a stalled escape consumes its permit) and Law 88c
(a stored pending body is re-offered at runtime). It is a **disposable Docker devnet** arm: it does
not authorise a public-net deployment, and it changes no consensus parameter.

PASS requires, for each cycle, within `--window` seconds of the victim's restart:

* all validators report the **same finalized `(height, hash)`**, at least three heights above the
  pre-kill baseline;
* every deploy submitted in the window is resolved `ProcessedWithSuccess` on every validator;
* no finalized height regresses and no fixed height changes hash across samples;
* every `/health` reports `pipelineFaults == 0`, `orphanBodies == 0`, `proposalHalted == false`,
  and no log contains `block receiver state full`;
* the victim proposes again (its own `proposed` counter advances after restart).

The predicates live in `restart_under_load_controls.py` so they can be tested offline; this script
only collects and feeds them.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
from threading import Barrier
import importlib.util
import json
import re
import subprocess
import time
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def _load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


drill = _load("acceptance_drill", ROOT / "tools/consensus-acceptance-drill.py")
controls = _load("restart_controls", ROOT / "tests/restart_under_load_controls.py")
fuzz = drill.fuzz

PREFIX = __import__("os").environ.get("DEVNET_PREFIX", "restartload")


def node_name(i):
    return f"{PREFIX}-bootstrap" if i == 0 else f"{PREFIX}-validator-{i}"


def health(i):
    return drill.get(i, "/health")


def docker(*args, timeout=60):
    return subprocess.run(["docker", *args], text=True, capture_output=True, timeout=timeout)


def record(path, **event):
    with path.open("a") as f:
        f.write(json.dumps({"utc": time.time(), **event}) + "\n")


def wait_ready(i, deadline, events):
    last = None
    while time.monotonic() < deadline:
        try:
            value = health(i)
            if value.get("ready") is True:
                return value
            last = value
        except (OSError, ValueError) as error:
            last = repr(error)
        time.sleep(1)
    record(events, wait_ready=i, last=last)
    raise RuntimeError(f"validator {i} did not become ready before the deadline")


def deploy_on(i, term):
    """Submit a signed deploy through validator `i`'s own CLI; return its deploy id."""
    status = drill.get(i, "/api/v1/status")
    node = node_name(i)
    path = f"/tmp/restart-under-load-{uuid.uuid4().hex}.rho"
    write = subprocess.run(
        ["docker", "exec", "-i", node, "sh", "-c", f"cat > {path}"],
        input=term,
        text=True,
        capture_output=True,
        timeout=10,
    )
    if write.returncode:
        raise RuntimeError(f"deploy file write failed on node {i}: {write.stderr}")
    result = subprocess.run(
        [
            "docker", "exec", node, "rnode", "--grpc-host", "localhost", "deploy",
            "--phlo-limit", "1000000", "--phlo-price", "1",
            "--private-key", fuzz.DEPLOYER_PRIV, "--shard-id", status["shardId"],
            "--valid-after-block-number", str(status["latestBlockNumber"]), path,
        ],
        text=True,
        capture_output=True,
        timeout=30,
    )
    if result.returncode:
        raise RuntimeError(f"deploy on node {i} failed: {result.stderr}")
    match = re.search(r"DeployId is: ([0-9a-fA-F]+)", result.stdout)
    if not match:
        raise RuntimeError(f"deploy on node {i} returned no deploy id: {result.stdout!r}")
    return match.group(1).lower()


def finalized(i):
    return drill.finalized_identity(drill.get(i, "/api/last-finalized-block"))


def agreement(validators):
    identities = [finalized(i) for i in range(validators)]
    return identities


def observed_finality(validators, last, events):
    """Reject regressions and finalized hash conflicts, even between final samples."""
    identities = agreement(validators)
    by_height = {}
    for i, identity in enumerate(identities):
        height, block_hash = identity
        old = last.get(i)
        if old is not None:
            if height < old[0]:
                raise RuntimeError(f"validator {i} finalized height regressed {old[0]} -> {height}")
            if height == old[0] and block_hash != old[1]:
                raise RuntimeError(f"validator {i} changed finalized hash at height {height}")
        if height in by_height and by_height[height] != block_hash:
            raise RuntimeError(f"conflicting finalized hashes at height {height}")
        by_height[height] = block_hash
        last[i] = identity
    record(events, finalized=identities)
    return identities


def wait_agreement(validators, min_height, deadline, events, last):
    while time.monotonic() < deadline:
        try:
            identities = observed_finality(validators, last, events)
            verdict = controls.agreement_verdict(identities, min_height)
            if verdict is None:
                return identities
            record(events, wait_agreement=verdict, identities=identities)
        except (OSError, ValueError) as error:
            record(events, wait_agreement_error=repr(error))
        time.sleep(2)
    raise RuntimeError(f"no agreement at height >= {min_height} by the recovery deadline")


def wait_deploys_resolved(validators, submitted, deadline, events, last):
    """Give accepted deployments until the same recovery deadline to resolve."""
    unresolved = []
    while time.monotonic() < deadline:
        observed_finality(validators, last, events)
        unresolved = []
        for i in range(validators):
            for deploy_id, _, _, _ in submitted:
                try:
                    status = drill.get(i, f"/api/v1/deploy-status/{deploy_id}")
                except (OSError, ValueError) as error:
                    unresolved.append((i, deploy_id, repr(error)))
                    continue
                if not controls.deploy_is_success(status):
                    unresolved.append((i, deploy_id, status))
        if not unresolved:
            record(events, resolved=len(submitted), validators=validators)
            return
        record(events, unresolved_count=len(unresolved), example=unresolved[:3])
        time.sleep(2)
    raise RuntimeError(f"{len(unresolved)} deploy statuses unresolved by recovery deadline")


def wait_baseline(validators, deadline, events):
    """Wait for the initial finalized block on *all* nodes before fault injection.

    A newly booted node may temporarily answer HTTP 400 to last-finalized-block;
    this is not a receiver-saturation outcome. The refusal is recorded, never
    counted as a PASS, and cannot outlive the bounded startup window.
    """
    last = "no finalized observation"
    while time.monotonic() < deadline:
        try:
            identities = agreement(validators)
            verdict = controls.baseline_verdict(identities)
            if verdict is None:
                record(events, baseline_ready=identities)
                return identities
            last = verdict
            record(events, baseline_wait=verdict, identities=identities)
        except (OSError, ValueError) as error:
            last = repr(error)
            record(events, baseline_read_error=last)
        time.sleep(2)
    raise RuntimeError(f"initial finalized baseline unavailable before deadline: {last}")


def victim_for_cycle(cycle):
    return 1 if cycle == 0 else 0  # non-master, then genesis master


def run_cycle(args, victim, cycle, events):
    for i in range(args.validators):
        wait_ready(i, time.monotonic() + args.window, events)
    baseline = wait_baseline(args.validators, time.monotonic() + args.window, events)
    base_height = baseline[0][0]
    last = dict(enumerate(baseline))
    record(events, cycle=cycle, baseline=baseline, victim=victim, streams=4)

    # Four independent CLI streams must be active when the SIGKILL occurs.
    # Target the three surviving validators: a killed node is not a valid
    # submission endpoint, and a refusal must never masquerade as an accepted write.
    gate = Barrier(5)
    stop_at = time.monotonic() + args.load_seconds

    def submit_stream(stream):
        gate.wait(timeout=15)
        accepted = []
        target = (victim + 1 + stream) % args.validators
        if target == victim:
            target = (target + 1) % args.validators
        while time.monotonic() < stop_at:
            marker = f"restart-under-load:{cycle}:{stream}:{uuid.uuid4().hex}"
            deploy_id = deploy_on(target, f'@"{marker}"!(1)')
            accepted.append((deploy_id, marker, target, stream))
            record(events, cycle=cycle, stream=stream, deploy=deploy_id,
                   marker=marker, target=target)
            time.sleep(0.2)
        return accepted

    with ThreadPoolExecutor(max_workers=4) as executor:
        futures = [executor.submit(submit_stream, stream) for stream in range(4)]
        gate.wait(timeout=15)
        time.sleep(5)
        node = node_name(victim)
        before = docker("inspect", "-f", "{{.Id}}", node)
        if before.returncode or not before.stdout.strip():
            raise RuntimeError(f"cannot inspect victim {victim}: {before.stderr}")
        started_utc = time.time()
        killed = docker("kill", "--signal", "KILL", node)
        if killed.returncode:
            raise RuntimeError(f"docker kill failed for victim {victim}: {killed.stderr}")
        restarted = docker("start", node)
        restarted_utc = time.time()
        if restarted.returncode:
            raise RuntimeError(f"docker start failed for victim {victim}: {restarted.stderr}")
        after = docker("inspect", "-f", "{{.Id}}", node)
        if after.returncode or before.stdout.strip() != after.stdout.strip():
            raise RuntimeError("restart did not reuse the original container and store")
        deadline = time.monotonic() + args.window
        record(events, cycle=cycle, killed=victim, killed_at=started_utc, restarted_at=restarted_utc,
               container=before.stdout.strip())
        submitted = []
        for future in futures:
            submitted.extend(future.result(timeout=max(1, deadline - time.monotonic())))
    counts = {stream: sum(1 for _, _, _, owner in submitted if owner == stream)
              for stream in range(4)}
    if any(count == 0 for count in counts.values()):
        raise RuntimeError(f"a concurrent deploy stream submitted no accepted deploys: {counts}")
    record(events, cycle=cycle, stream_counts=counts)

    wait_ready(victim, deadline, events)
    identities = wait_agreement(args.validators, base_height + 3, deadline, events, last)
    record(events, cycle=cycle, agreed=identities)
    wait_deploys_resolved(args.validators, submitted, deadline, events, last)
    identities = wait_agreement(args.validators, base_height + 3, deadline, events, last)
    record(events, cycle=cycle, final_agreement=identities)

    for i in range(args.validators):
        body = health(i)
        verdict = controls.health_verdict(body)
        if verdict is not None:
            raise RuntimeError(f"validator {i}: {verdict}")
        record(events, cycle=cycle, node=i, health=body)
        all_logs = docker("logs", node_name(i))
        if all_logs.returncode:
            raise RuntimeError(f"cannot inspect validator {i} logs: {all_logs.stderr}")
        if "block receiver state full" in all_logs.stdout + all_logs.stderr:
            raise RuntimeError(f"validator {i} logged a receiver capacity refusal")

    since = docker("logs", "--since", str(restarted_utc), node)
    if since.returncode:
        raise RuntimeError(f"cannot inspect post-restart proposal evidence: {since.stderr}")
    if not controls.proposed_after_restart(since.stdout + since.stderr):
        raise RuntimeError("the victim did not propose after restart")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--validators", type=int, default=4)
    parser.add_argument("--cycles", type=int, default=2)
    parser.add_argument("--window", type=int, default=240)
    parser.add_argument("--load-seconds", type=int, default=20)
    parser.add_argument("--out", default="target/restart-under-load")
    args = parser.parse_args()

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    events = out / "observations.jsonl"
    events.write_text("")
    # The container walk is only valid for the devnet naming.
    if args.cycles != 2 or args.load_seconds < 8 or args.load_seconds >= args.window:
        raise SystemExit("issue #522 requires two cycles and 8 <= load-seconds < window")
    if args.validators < 3:
        raise SystemExit("a restart-under-load arm needs at least three validators")

    try:
        for cycle in range(args.cycles):
            victim = victim_for_cycle(cycle)
            run_cycle(args, victim, cycle, events)
        result = {"verdict": "PASS", "cycles": args.cycles, "validators": args.validators}
    except Exception as error:  # noqa: BLE001 - the verdict is the output
        result = {"verdict": "FAIL", "error": str(error)}
    (out / "result.json").write_text(json.dumps(result, indent=2))
    print(json.dumps(result))
    return 0 if result["verdict"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
