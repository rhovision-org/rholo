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
from threading import Barrier, Event, Thread
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


HTTP_BASE = int(__import__("os").environ.get("DEVNET_HTTP_BASE", "40403"))
ADMIN_BASE = int(__import__("os").environ.get("DEVNET_ADMIN_BASE", "40405"))


def get(i, path, body=None, admin=False, timeout=10):
    """The devnet's HTTP path, honouring the port bases the network was actually started with."""
    port = (ADMIN_BASE if admin else HTTP_BASE) + i * 1000
    return drill.fuzz.http_json(f"http://127.0.0.1:{port}{path}",
                                "POST" if body is not None else "GET", body, timeout=timeout)[1]


def health(i):
    return get(i, "/health")


def docker(*args, timeout=60):
    return subprocess.run(["docker", *args], text=True, capture_output=True, timeout=timeout)


def record(path, **event):
    with path.open("a") as f:
        f.write(json.dumps({"utc": time.time(), **event}) + "\n")


def boot_id(i):
    """The process identity `/health` publishes (`<pid>-<start-millis>`)."""
    value = health(i).get("bootId")
    if not isinstance(value, str) or not value:
        raise RuntimeError(f"validator {i} reports no bootId")
    return value


def wait_new_boot(i, previous, deadline, events):
    """Wait until the restarted victim is a *different process*, not merely a started container."""
    last = previous
    while time.monotonic() < deadline:
        try:
            value = boot_id(i)
            if value != previous:
                record(events, node=i, previous_boot_id=previous, new_boot_id=value)
                return value
            last = value
        except (OSError, ValueError) as error:
            last = repr(error)
        time.sleep(0.5)
    record(events, wait_new_boot=i, previous=previous, last=last)
    # **A silent /health is not the same as a slow one.** The restarted process may have exited
    # with a named refusal; capture its own log and exit code so the failure names the reason.
    node = node_name(i)
    logs = docker("logs", "--tail", "80", node)
    exit_code = docker("inspect", "-f", "{{.State.ExitCode}}", node)
    detail = (logs.stdout + logs.stderr)[-1500:]
    raise RuntimeError(
        f"validator {i} did not restart with a new bootId "
        f"(exit={exit_code.stdout.strip()}, last={last}); log tail: {detail}")


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
    status = get(i, "/api/v1/status")
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
    return drill.finalized_identity(get(i, "/api/last-finalized-block"))


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
                    status = get(i, f"/api/v1/deploy-status/{deploy_id}")
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
    victim_boot = boot_id(victim)
    record(events, cycle=cycle, baseline=baseline, victim=victim, victim_boot_id=victim_boot,
           streams=4)

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
        restarted_boot = wait_new_boot(victim, victim_boot, deadline, events)
        record(events, cycle=cycle, killed=victim, killed_at=started_utc, restarted_at=restarted_utc,
               container=before.stdout.strip(), restarted_boot_id=restarted_boot)

        # **The victim's catch-up coordinates, sampled while it rejoins** (RC-4 item A). The phase
        # must be consistent whenever it says `catching-up`, and a catch-up that does not close is
        # the silent freeze the continuous supervisor exists to remove. The survivor height is the
        # validated-height proxy (`height` is one past the observed tip); the predicate only uses it
        # as a sanity floor on the victim's own validated tip.
        catchup_samples = []
        stop_sampling = Event()

        def sample_catchup():
            while not stop_sampling.is_set():
                body = health(victim)
                peer_heights = []
                for i in range(args.validators):
                    if i == victim:
                        continue
                    peer = health(i)
                    if isinstance(peer, dict) and type(peer.get("height")) is int:
                        peer_heights.append(peer["height"] - 1)
                catchup_samples.append(
                    {"body": body, "peer_height": max(peer_heights, default=-1)}
                )
                time.sleep(1)

        sampler = Thread(target=sample_catchup, daemon=True)
        sampler.start()
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

    stop_sampling.set()
    sampler.join(timeout=5)
    reading = []
    for index, sample in enumerate(catchup_samples):
        body = sample["body"]
        verdict = controls.catchup_verdict(body)
        if verdict is not None:
            raise RuntimeError(f"victim {victim} catch-up sample {index}: {verdict}")
        catchup = body.get("catchUp") if isinstance(body, dict) else None
        reading.append(
            {
                "phase": body.get("nodePhase") if isinstance(body, dict) else None,
                "own": catchup.get("own", -1) if isinstance(catchup, dict) else -1,
                "peer_height": sample["peer_height"],
            }
        )
    progress = controls.catchup_progress_verdict(reading)
    if progress is not None:
        raise RuntimeError(f"victim {victim}: {progress}")
    record(events, cycle=cycle, catchup_samples=reading)

    pipelines = {}
    for i in range(args.validators):
        body = health(i)
        verdict = controls.health_verdict(body)
        if verdict is not None:
            raise RuntimeError(f"validator {i}: {verdict}")
        pipeline = body.get("blockPipeline") if isinstance(body, dict) else None
        if pipeline is None:
            # Absent is unknown, never zero: the arm must not claim a capacity reading it did not take.
            record(events, cycle=cycle, node=i, block_pipeline_absent=True)
        else:
            pipeline_verdict = controls.receiver_verdict(body)
            if pipeline_verdict is not None:
                raise RuntimeError(f"validator {i}: {pipeline_verdict}")
            pipelines[i] = pipeline
            record(events, cycle=cycle, node=i, block_pipeline=pipeline)
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

    return {
        "cycle": cycle,
        "result": "PASS",
        "victim": victim,
        "base_height": base_height,
        "final": identities,
        "boot_ids": [victim_boot, restarted_boot],
        "stream_counts": counts,
        "block_pipeline": pipelines,
    }


NETWORK = __import__("os").environ.get("DEVNET_NETWORK", "devnet")
IMAGE = __import__("os").environ.get("RNODE_IMAGE", "rnode:local")


def control_node_spec():
    """A running validator's argv, minus its validator identity.

    The control node joins the same Docker network and bootstraps from the same container, but a
    negative-control node must not sign blocks: it is not a validator and must never be counted as
    one. The image's entrypoint is used unchanged.
    """
    # A validator, not the genesis master: only a validator carries `--bootstrap`, and the control
    # node must join the existing chain rather than create a second genesis of its own.
    raw = docker("inspect", node_name(1))
    if raw.returncode:
        raise RuntimeError(f"cannot inspect validator 1: {raw.stderr}")
    config = json.loads(raw.stdout)[0]["Config"]
    entrypoint = config.get("Entrypoint") or []
    cmd = list(config.get("Cmd") or [])
    # A control node must not sign, propose, deploy or reuse a peer hostname.
    drop = {
        "--validator-private-key", "--validator-private-key-path", "--validator-public-key",
        "--validator-key-file", "--deployer-private-key", "--autopropose", "--propose-on-deploy",
        "--host",
    }
    cleaned = []
    skip = False
    for item in cmd:
        if skip:
            skip = False
            continue
        if item in drop:
            skip = True
            continue
        if any(item.startswith(f + "=") for f in drop):
            continue
        cleaned.append(item)
    if not entrypoint:
        entrypoint = ["rnode"]
    return entrypoint, cleaned


def genesis_hash():
    blocks = get(0, "/api/blocks/1")
    if not isinstance(blocks, list) or not blocks:
        raise RuntimeError("the bootstrap serves no anchor block")
    value = blocks[0].get("blockHash")
    if not isinstance(value, str) or len(value) != 64:
        raise RuntimeError("the bootstrap's anchor is not a 32-byte block hash")
    return value


def flipped(hash_hex):
    """A deterministic, absent hash of the same shape: flip the first nibble."""
    first = "0" if hash_hex[0] != "0" else "1"
    return first + hash_hex[1:]


def shared_mounts():
    """The genesis and contracts binds the running validators use, by destination."""
    raw = docker("inspect", "-f", "{{json .Mounts}}", node_name(1))
    if raw.returncode:
        raise RuntimeError(f"cannot inspect validator 1 mounts: {raw.stderr}")
    out = []
    for mount in json.loads(raw.stdout):
        if mount.get("Destination") in ("/genesis", "/contracts"):
            mode = "ro" if not mount.get("RW") else "rw"
            out += ["-v", f"{mount['Source']}:{mount['Destination']}:{mode}"]
    return out


def control_container(name, extra, volume):
    entrypoint, cmd = control_node_spec()
    docker("rm", "-f", name)
    docker("volume", "rm", "-f", volume)
    launched = docker(
        "run", "-d", "--name", name, "--network", NETWORK,
        "-v", f"{volume}:/var/lib/rnode", *shared_mounts(),
        "--entrypoint", entrypoint[0], IMAGE, *entrypoint[1:], *cmd, "--host", name, *extra,
    )
    if launched.returncode:
        raise RuntimeError(f"cannot start control node {name}: {launched.stderr}")
    return launched.stdout.strip()


def wait_control_exit(name, deadline):
    """Wait for a control container to stop; return (exit_code, log, entered_running)."""
    last = None
    while time.monotonic() < deadline:
        state = docker("inspect", "-f", "{{.State.Running}} {{.State.ExitCode}}", name)
        if state.returncode:
            last = state.stderr
        else:
            running, code = state.stdout.split()
            logs = docker("logs", name)
            text = logs.stdout + logs.stderr
            entered = "Making a transition to Running state" in text
            if running == "false":
                return int(code), text, entered
            last = f"running; exit={code}"
        time.sleep(1)
    raise RuntimeError(f"control node {name} did not stop before the deadline: {last}")


def run_control(args, out, events):
    name = f"{PREFIX}-control-{args.control}"
    volume = f"{PREFIX}-control-{args.control}-data"
    if args.control == "interrupted-restart":
        verdict, detail = interrupted_restart(args, events)
        record(events, control=args.control, verdict=verdict, **detail)
        (out / f"{args.control}.json").write_text(json.dumps(
            {"control": args.control, "result": "PASS" if verdict is None else "FAIL",
             "error": verdict, **detail}, indent=2))
        return verdict
    try:
        if args.control == "wrong-genesis":
            wrong = flipped(genesis_hash())
            control_container(name, ["--expected-genesis-hash", wrong], volume)
            code, log, entered = wait_control_exit(name, time.monotonic() + args.control_window)
            verdict = controls.wrong_genesis_verdict(code, log, entered)
            detail = {"wrong_genesis": wrong, "exit_code": code, "entered_running": entered}
        else:
            absent = flipped(genesis_hash())
            control_container(name, ["--sync-anchor", absent], volume)
            code, log, entered = wait_control_exit(name, time.monotonic() + args.control_window)
            verdict = controls.incomplete_ancestry_verdict(code, log, entered)
            detail = {"absent_anchor": absent, "exit_code": code, "entered_running": entered}
    finally:
        # Preserve the whole control log and inspect before removing the container — including on a
        # timeout, so a control that never refuses leaves its evidence rather than a stray container.
        logs = docker("logs", name)
        (out / f"{args.control}.log").write_text(logs.stdout + logs.stderr)
        inspect = docker("inspect", name)
        (out / f"{args.control}-inspect.json").write_text(inspect.stdout)
        docker("rm", "-f", name)
        docker("volume", "rm", "-f", volume)
    record(events, control=args.control, verdict=verdict, **detail)
    (out / f"{args.control}.json").write_text(json.dumps(
        {"control": args.control, "result": "PASS" if verdict is None else "FAIL",
         "error": verdict, **detail}, indent=2))
    return verdict


def interrupted_restart(args, events):
    """Kill the victim, start it, kill it again before it can recover, then start it."""
    for i in range(args.validators):
        wait_ready(i, time.monotonic() + args.control_window, events)
    baseline = wait_baseline(args.validators, time.monotonic() + args.control_window, events)
    base_height = baseline[0][0]
    last = dict(enumerate(baseline))
    victim = 1
    node = node_name(victim)
    boot_ids = [boot_id(victim)]

    first_kill = docker("kill", "--signal", "KILL", node)
    if first_kill.returncode:
        raise RuntimeError(f"first kill failed: {first_kill.stderr}")
    first_start = docker("start", node)
    if first_start.returncode:
        raise RuntimeError(f"first start failed: {first_start.stderr}")
    deadline = time.monotonic() + args.control_window
    boot_ids.append(wait_new_boot(victim, boot_ids[-1], deadline, events))

    second_kill = docker("kill", "--signal", "KILL", node)
    if second_kill.returncode:
        raise RuntimeError(f"second kill failed: {second_kill.stderr}")

    ready_error = None
    if args.interrupt_expect == "detected":
        try:
            wait_ready(victim, time.monotonic() + min(15, args.control_window), events)
        except RuntimeError as error:
            ready_error = str(error)

    second_start = docker("start", node)
    if second_start.returncode:
        raise RuntimeError(f"second start failed: {second_start.stderr}")
    deadline = time.monotonic() + args.control_window
    boot_ids.append(wait_new_boot(victim, boot_ids[-1], deadline, events))
    wait_ready(victim, deadline, events)
    identities = wait_agreement(args.validators, base_height + 3, deadline, events, last)
    detail = {"boot_ids": boot_ids, "final": identities, "ready_error": ready_error,
              "interrupt_expect": args.interrupt_expect}
    return controls.interrupted_restart_verdict(boot_ids, args.interrupt_expect, ready_error), detail


def provenance(validators, args):
    source = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    binary = docker("exec", node_name(0), "sha256sum", "/usr/local/bin/rnode")
    if binary.returncode:
        raise RuntimeError(f"cannot hash the running binary: {binary.stderr}")
    image = docker("inspect", "-f", "{{.Id}}", node_name(0))
    return {
        "source_sha": source,
        "binary_sha256": binary.stdout.split()[0],
        "container_image": image.stdout.strip() if image.returncode == 0 else None,
        "genesis_hash": genesis_hash(),
        "validators": validators,
        "window": args.window,
        "load_seconds": args.load_seconds,
        "prefix": PREFIX,
        "network": NETWORK,
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--validators", type=int, default=4)
    parser.add_argument("--cycles", type=int, default=2)
    parser.add_argument("--window", type=int, default=240)
    parser.add_argument("--load-seconds", type=int, default=20)
    parser.add_argument("--out", default="target/restart-under-load")
    parser.add_argument("--control",
                        choices=["wrong-genesis", "incomplete-ancestry", "interrupted-restart"])
    parser.add_argument("--control-window", type=int, default=240)
    parser.add_argument("--interrupt-expect", choices=["recovered", "detected"],
                        default="recovered")
    args = parser.parse_args()

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    events = out / "observations.jsonl"
    events.write_text("")

    if args.control:
        if args.validators < 2:
            raise SystemExit("a control needs validator 1 to supply the node argv")
        try:
            verdict = run_control(args, out, events)
            result = {"verdict": "PASS" if verdict is None else "FAIL",
                      "control": args.control, "error": verdict}
        except Exception as error:  # noqa: BLE001 - the verdict is the output
            rows, capture_error = drill.failure_envelope.best_effort_health(
                health, range(args.validators))
            try:
                last_events = events.read_text().splitlines()[-20:]
            except OSError:
                last_events = []
            drill.failure_envelope.write(
                out, "FAIL", error, phase="control", arm=args.control,
                health=rows, events=last_events, capture_error=capture_error)
            result = {"verdict": "FAIL", "control": args.control, "error": str(error)}
        (out / "campaign.json").write_text(json.dumps(
            {"gate_id": f"n4-restart-under-load-{args.control}",
             "cycles": [{"control": args.control, "result": result["verdict"]}]}, indent=2))
        (out / "result.json").write_text(json.dumps(result, indent=2))
        print(json.dumps(result))
        return 0 if result["verdict"] == "PASS" else 1

    # The container walk is only valid for the devnet naming.
    if args.cycles != 2 or args.load_seconds < 8 or args.load_seconds >= args.window:
        raise SystemExit("issue #522 requires two cycles and 8 <= load-seconds < window")
    if args.validators < 3:
        raise SystemExit("a restart-under-load arm needs at least three validators")

    try:
        pin = provenance(args.validators, args)
    except Exception as error:  # noqa: BLE001 - a provenance failure is recorded, not a verdict
        pin = {"error": f"{type(error).__name__}: {error}"}
        record(events, provenance_error=pin["error"])

    arms = []
    try:
        for cycle in range(args.cycles):
            victim = victim_for_cycle(cycle)
            arms.append(run_cycle(args, victim, cycle, events))
        result = {"verdict": "PASS", "cycles": args.cycles, "validators": args.validators}
        (out / "campaign.json").write_text(json.dumps(
            {"gate_id": "n4-restart-under-load", "provenance": pin, "cycles": arms}, indent=2))
    except Exception as error:  # noqa: BLE001 - the verdict is the output
        # **The structured envelope** (M): class, arm, phase, the health rows at the failure and the
        # last observations. A capture failure is data, never a replacement for the primary error.
        rows, capture_error = drill.failure_envelope.best_effort_health(
            health, range(args.validators))
        try:
            last_events = events.read_text().splitlines()[-20:]
        except OSError:
            last_events = []
        drill.failure_envelope.write(
            out, "FAIL", error, phase="restart-under-load", arm="restart-under-load",
            health=rows, events=last_events, capture_error=capture_error)
        result = {"verdict": "FAIL", "error": str(error)}
    (out / "result.json").write_text(json.dumps(result, indent=2))
    print(json.dumps(result))
    return 0 if result["verdict"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
