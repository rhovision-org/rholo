#!/usr/bin/env python3
"""Admission vs finality: a four-validator campaign at <= 2/3 live bonded stake (#487).

This is a **disposable Docker devnet** arm. It does not authorise a public-net deployment, does not
rewrite a live store, and changes no consensus parameter.

It expects a running devnet (start it with `tools/devnet.sh up --validators 4
--stakes 100,100,50,50 --fresh --no-autopropose --no-propose-on-deploy`) and proves:

* a deploy is `pooled` before a proposal and `includedUnfinalized`/`finalityUnknown` after one, never
  `finalizedSuccess` while finality is frozen;
* with 200 of 300 bonded stake live (exactly 2/3, not a strict supermajority) the height advances
  while the finalized height does not, `/health` still says ready and acceptsDeploys, and the
  per-deploy settlement never claims settlement;
* restoring the two stopped validators finalizes the same deploy without resubmission, and no
  accepted deploy is left unsettled;
* a partitioned validator answers `availability: "unknown"` and never a false settlement.

The predicates live in `admission_finality_stall_controls.py` so they are testable offline; this
script only collects and feeds them.
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import os
import re
import subprocess
import time
import traceback
import urllib.error
import urllib.request
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


drill = _load("acceptance_drill", ROOT / "tools/consensus-acceptance-drill.py")
controls = _load("finality_controls", ROOT / "tests/admission_finality_stall_controls.py")
fuzz = drill.fuzz

PREFIX = os.environ.get("DEVNET_PREFIX", "admissionstall")
NETWORK = os.environ.get("DEVNET_NETWORK", PREFIX)
HTTP_BASE = int(os.environ.get("DEVNET_HTTP_BASE", "40403"))
ADMIN_BASE = int(os.environ.get("DEVNET_ADMIN_BASE", "40405"))
IMAGE = os.environ.get("RNODE_IMAGE", "rnode:local")


def node_name(i: int) -> str:
    return f"{PREFIX}-bootstrap" if i == 0 else f"{PREFIX}-validator-{i}"


def port(i: int, admin: bool = False) -> int:
    return (ADMIN_BASE if admin else HTTP_BASE) + i * 1000


def http_json(url: str, method: str = "GET", body: dict | None = None, timeout: int = 10):
    data = json.dumps(body).encode() if body is not None else None
    headers = {"Content-Type": "application/json"} if data is not None else {}
    request = urllib.request.Request(url, data=data, headers=headers, method=method)
    with urllib.request.urlopen(request, timeout=timeout) as response:
        raw = response.read().decode()
        return response.status, (json.loads(raw) if raw.strip() else None)


def http_allow_error(url: str, timeout: int = 10) -> tuple[int, dict | None]:
    """A 503 is an answer, not an exception: the finality route uses it for 'unknown'."""
    try:
        return http_json(url, timeout=timeout)
    except urllib.error.HTTPError as error:
        raw = error.read().decode()
        try:
            return error.code, (json.loads(raw) if raw.strip() else None)
        except ValueError:
            return error.code, None


def get(i: int, path: str):
    _, body = http_json(f"http://127.0.0.1:{port(i)}{path}")
    return body


def container_get_allow_error(name: str, path: str, timeout: int = 10) -> tuple[int, dict | None]:
    """Read a node's own API through its container loopback.

    A partitioned victim is isolated from its peers, not from itself: `docker network disconnect`
    removes the published host route, so the only witness is the container's own loopback. Returns
    the status code as well, because `/api/v1/finality-observation` answers 503 for unknown.
    """
    result = subprocess.run(
        ["docker", "exec", name, "curl", "-sS", "-w", "\n%{http_code}",
         f"http://127.0.0.1:40403{path}"],
        text=True,
        capture_output=True,
        timeout=timeout,
    )
    body, _, code = result.stdout.rpartition("\n")
    return int(code or 0), (json.loads(body) if body.strip() else None)


def health(i: int) -> dict:
    return get(i, "/health")


def status(i: int) -> dict:
    return get(i, "/api/v1/status")


def docker(*args: str, timeout: int = 90) -> subprocess.CompletedProcess:
    return subprocess.run(["docker", *args], text=True, capture_output=True, timeout=timeout)


def record(path: Path, **event) -> None:
    event.setdefault("utc", time.time())
    with path.open("a") as handle:
        handle.write(json.dumps(event) + "\n")


def finalized_height(i: int) -> int:
    body = get(i, "/api/last-finalized-block")
    return int(body["blockInfo"]["blockNumber"])


def observed_height(i: int) -> int | None:
    value = health(i).get("maxObservedHeight")
    return None if value is None else int(value)


def operator_status(i: int) -> dict:
    """The operator half: whether the persisted stall counters are the shard's history."""
    try:
        _, body = http_json(f"http://127.0.0.1:{port(i, admin=True)}/api/v1/status/operator")
    except Exception:  # noqa: BLE001 - a stopped node has no reading; the caller records it
        return {}
    return body if isinstance(body, dict) else {}


def stall_surface(i: int) -> dict:
    """The episode surface: stalled now, the persisted count, and whether it is the shard's."""
    body = health(i)
    operator = operator_status(i)
    return {
        "stalled": body.get("finalityStalled"),
        "episodes": body.get("finalityStallEpisodes"),
        "durable": operator.get("finalityCountersDurable"),
    }


def settlement(deploy_id: str, node: int = 0) -> dict:
    return get(node, f"/api/v1/deploy-settlement/{deploy_id}")


def deploy_status(deploy_id: str, node: int = 0) -> dict:
    return get(node, f"/api/v1/deploy-status/{deploy_id}")


def deploy_on(i: int, term: str) -> str:
    """Submit a signed deploy through validator `i`'s own CLI; return its deploy id."""
    current = status(i)
    node = node_name(i)
    path = f"/tmp/admission-finality-{uuid.uuid4().hex}.rho"
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
            "--private-key", fuzz.DEPLOYER_PRIV, "--shard-id", current["shardId"],
            "--valid-after-block-number", str(current["latestBlockNumber"]), path,
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


def propose(i: int) -> None:
    http_json(
        f"http://127.0.0.1:{port(i, admin=True)}/api/v1/propose",
        method="POST",
        body={},
        timeout=15,
    )


def wait_ready(i: int, deadline: float, events: Path) -> None:
    last = None
    while time.monotonic() < deadline:
        try:
            value = health(i)
            if value.get("ready") is True:
                return
            last = value
        except (OSError, ValueError, urllib.error.URLError) as error:
            last = repr(error)
        time.sleep(1)
    record(events, wait_ready=i, last=last)
    raise RuntimeError(f"validator {i} did not become ready before the deadline")


def wait_finalized(indices, min_height: int, deadline: float, events: Path) -> None:
    last = None
    indices = list(indices)
    while time.monotonic() < deadline:
        heights = []
        for i in indices:
            try:
                heights.append(finalized_height(i))
            except (OSError, ValueError, KeyError, urllib.error.URLError) as error:
                last = repr(error)
        if heights and len(heights) == len(indices) and min(heights) >= min_height:
            return
        time.sleep(1)
    record(events, wait_finalized=min_height, last=last)
    raise RuntimeError(f"finality did not reach {min_height} before the deadline")


def drive_proposals(indices, rounds: int = 3) -> None:
    for _ in range(rounds):
        for i in indices:
            try:
                propose(i)
            except (OSError, urllib.error.URLError):
                pass
        time.sleep(1)


def advance_with_deploy(indices, rounds: int = 1) -> None:
    """A quiet chain's autopropose has no new state transition; a fresh deploy supplies one.

    `POST /api/v1/propose` with no new deploy answers `NoNewDeploys`, so the stall arm advances the
    height by submitting and proposing, not by waiting on the timer. Each live validator gets its own
    deploy because a pool is node-local.
    """
    for _ in range(rounds):
        for i in indices:
            try:
                deploy_on(i, "Nil")
                propose(i)
            except (OSError, RuntimeError, urllib.error.URLError):
                pass
        time.sleep(1)


def wait_deploy_processed(deploy_id: str, deadline: float, events: Path, node: int = 0) -> dict:
    last = None
    while time.monotonic() < deadline:
        last = deploy_status(deploy_id, node)
        if "ProcessedWithSuccess" in last:
            return last
        if "ProcessedWithError" in last:
            raise RuntimeError(f"deploy {deploy_id} was processed with an error: {last}")
        time.sleep(1)
    record(events, wait_deploy_processed=deploy_id, last=last)
    raise RuntimeError(f"deploy {deploy_id} was not processed before the deadline")


def processed_block_number(status_body: dict) -> int:
    entry = status_body.get("ProcessedWithSuccess") or status_body.get("ProcessedWithError")
    if not entry:
        raise RuntimeError(f"deploy status has no included block: {status_body}")
    return int(entry["block"]["blockNumber"])


def wait_settlement(deploy_id: str, deadline: float, events: Path) -> dict:
    """Poll until the inclusion block is final, or the deadline; return the last reading."""
    last = None
    while time.monotonic() < deadline:
        last = settlement(deploy_id)
        if last.get("stage") == "finalizedSuccess":
            return last
        time.sleep(1)
    record(events, wait_settlement=deploy_id, last=last)
    return last


def provenance(validators: int, stakes: list[int]) -> dict:
    source = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True, capture_output=True, timeout=10
    ).stdout.strip()
    image_id = docker("image", "inspect", "--format", "{{.Id}}", IMAGE).stdout.strip()
    binary = docker(
        "exec", node_name(0), "sha256sum", "/usr/local/bin/rnode"
    ).stdout.split()[0]
    genesis = {
        # The devnet mounts its genesis directory at `/genesis`, not under the data directory.
        "bonds": docker("exec", node_name(0), "cat", "/genesis/bonds.txt").stdout,
        "wallets": docker("exec", node_name(0), "cat", "/genesis/wallets.txt").stdout,
    }
    return {
        "source_sha": source,
        "image": IMAGE,
        "image_id": image_id,
        "binary_sha256": binary,
        "validators": validators,
        "stakes": stakes,
        "genesis": genesis,
        "prefix": PREFIX,
        "network": NETWORK,
    }


def sample_settlement(
    deploy_id: str, out: Path, tag: str, seconds: int = 5, node: int = 0
) -> list[dict]:
    samples = []
    deadline = time.time() + seconds
    while time.time() < deadline:
        value = settlement(deploy_id, node)
        samples.append(value)
        record(out / "observations.jsonl", settlement=tag, deploy=deploy_id, node=node, value=value)
        time.sleep(0.5)
    return samples


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--validators", type=int, default=4)
    parser.add_argument("--stakes", default="100,100,50,50")
    parser.add_argument("--out", default="target/admission-finality-stall")
    parser.add_argument("--window", type=int, default=240)
    parser.add_argument("--recovery-window", type=int, default=360)
    args = parser.parse_args()

    stakes = [int(value) for value in args.stakes.split(",")]
    if len(stakes) != args.validators:
        raise SystemExit("--stakes needs one value per bonded validator")
    total = sum(stakes)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    events = out / "observations.jsonl"
    events.write_text("")

    arms: list[dict] = []
    accepted: list[str] = []
    result: dict

    try:
        pin = provenance(args.validators, stakes)
        (out / "provenance.json").write_text(json.dumps(pin, indent=2))
        deadline = time.monotonic() + args.window
        for i in range(args.validators):
            wait_ready(i, deadline, events)
        # A finalizing baseline: the campaign is meaningless on a chain that never finalized.
        drive_proposals(range(args.validators))
        wait_finalized(range(args.validators), 3, deadline, events)
        baseline_counters = {i: stall_surface(i) for i in range(args.validators)}

        # Arm 1: accepted, and never advertised as settled before its inclusion block finalizes. On
        # an autopropose chain the deploy can already be included when the first sample is read, so
        # the stage is recorded rather than required; the `pooled` stage itself is unit-tested in
        # `issue_487_finalized_execution_error_is_not_successful_settlement`.
        accepted_id = deploy_on(0, "Nil")
        accepted.append(accepted_id)
        first = settlement(accepted_id)
        error = controls.settlement_verdict(
            first.get("stage"), first.get("settlementConfirmed"), first.get("finalized")
        )
        arms.append({"arm": "accepted-not-settled", "deploy": accepted_id,
                     "observed_stage": first.get("stage"),
                     "result": "PASS" if error is None else "FAIL",
                     "error": error, "sample": first})
        if error is not None:
            raise RuntimeError(error)

        # Arm 2: ordinary lag on >2/3. The deploy must settle.
        lag_id = deploy_on(0, "Nil")
        accepted.append(lag_id)
        processed = wait_deploy_processed(lag_id, deadline, events)
        included = processed_block_number(processed)
        lag_samples = sample_settlement(lag_id, out, "ordinary-lag", seconds=3)
        drive_proposals(range(args.validators))
        wait_finalized(range(args.validators), included, deadline, events)
        lag_samples.append(wait_settlement(lag_id, deadline, events))
        # **The never-stalled control.** The ordinary lag is not a sustained stall, so the
        # persisted episode count must not have moved before the trigger.
        pre_trigger_counters = {i: stall_surface(i) for i in range(args.validators)}
        error = controls.ordinary_lag_verdict(lag_samples)
        arms.append({"arm": "ordinary-lag", "deploy": lag_id, "result": "PASS" if error is None else "FAIL",
                     "error": error})
        if error is not None:
            raise RuntimeError(error)

        # Arm 3: the stall at exactly 2/3. Stop the two smallest-stake validators.
        stopped = sorted(range(args.validators), key=lambda i: stakes[i])[: 2]
        live = total - sum(stakes[i] for i in stopped)
        error = controls.stake_fraction_verdict(total, live, must_exceed_two_thirds=False)
        if error is not None:
            raise RuntimeError(error)
        for i in stopped:
            record(events, stop=i, node=node_name(i))
            docker("stop", node_name(i))
        live_nodes = [i for i in range(args.validators) if i not in stopped]
        # Let the finality already in flight from the stopped validators settle before the baseline.
        # A step that completes from messages sent before the stop is not production at <= 2/3; a
        # step after this window is.
        time.sleep(5)
        before = {i: finalized_height(i) for i in live_nodes}
        stall_id = deploy_on(live_nodes[0], "Nil")
        accepted.append(stall_id)
        stall_processed = wait_deploy_processed(stall_id, time.monotonic() + args.window, events)
        stall_inclusion = processed_block_number(stall_processed)
        height_before = {i: observed_height(i) for i in live_nodes}
        # The round gate can hold a proposer for its 15 s escape; give the live side a window rather
        # than one attempt. Production continuing is the claim, not production at a fixed cadence.
        advance_deadline = time.monotonic() + 45
        height_after = dict(height_before)
        while time.monotonic() < advance_deadline:
            advance_with_deploy(live_nodes)
            height_after = {i: observed_height(i) for i in live_nodes}
            if any(
                height_after[i] is not None
                and height_before[i] is not None
                and height_after[i] > height_before[i]
                for i in live_nodes
            ):
                break
            time.sleep(2)
        frozen = {i: finalized_height(i) for i in live_nodes}
        stall_counters = {i: stall_surface(i) for i in live_nodes}
        samples = sample_settlement(stall_id, out, "stall", seconds=6)
        for sample in samples:
            error = controls.settlement_verdict(
                sample.get("stage"), sample.get("settlementConfirmed"), sample.get("finalized")
            )
            if error is not None:
                raise RuntimeError(error)
        if any(sample.get("stage") == "finalizedSuccess" for sample in samples):
            raise RuntimeError("a deploy settled while only 2/3 of bonded stake was live")
        # Production continues on the live side; it need not advance on every surviving validator
        # (the round gate can leave one blocked while another produces).
        progressed = any(
            height_after[i] is not None
            and height_before[i] is not None
            and height_after[i] > height_before[i]
            for i in live_nodes
        )
        if not progressed:
            raise RuntimeError(
                f"height did not advance on any live validator: before={height_before} after={height_after}"
            )
        if any(frozen[i] > before[i] for i in live_nodes):
            raise RuntimeError(
                f"finality advanced with {live}/{total} live stake: before={before} frozen={frozen}"
            )
        bootstrap = health(live_nodes[0])
        error = controls.health_contrast_verdict(bootstrap)
        if error is not None:
            raise RuntimeError(error)
        arms.append({
            "arm": "stall-exactly-two-thirds",
            "deploy": stall_id,
            "live_stake": f"{live}/{total}",
            "height_before": height_before,
            "height_after": height_after,
            "finalized_before": before,
            "finalized_frozen": frozen,
            "settlement": samples,
            "counters": {"baseline": baseline_counters,
                         "pre_trigger": pre_trigger_counters,
                         "stall": stall_counters},
            "result": "PASS",
        })

        # Recovery: restore >2/3 and require the same deploy to finalize.
        for i in stopped:
            record(events, start=i, node=node_name(i))
            docker("start", node_name(i))
        recovery_deadline = time.monotonic() + args.recovery_window
        # A rejoined node can carry a pipeline/orphan obligation and report not-ready while it
        # catches up; the criterion is finality recovery on a strict supermajority of **ready**
        # stake, not every node's readiness. A not-ready node is recorded, not hidden.
        recovery_nodes: list[int] = []
        while time.monotonic() < recovery_deadline:
            ready: list[int] = []
            for i in range(args.validators):
                try:
                    if health(i).get("ready") is True:
                        ready.append(i)
                except (OSError, ValueError, KeyError, urllib.error.URLError):
                    pass
            if len(ready) >= 3 and 3 * sum(stakes[i] for i in ready) > 2 * total:
                recovery_nodes = ready
                break
            time.sleep(2)
        if not recovery_nodes:
            raise RuntimeError(
                "no strict supermajority of ready bonded stake returned before the deadline"
            )
        not_ready = []
        for i in range(args.validators):
            if i not in recovery_nodes:
                try:
                    not_ready.append({"node": i, "reasons": health(i).get("readyReasons")})
                except (OSError, ValueError, urllib.error.URLError) as error:
                    not_ready.append({"node": i, "reasons": repr(error)})
        inclusion = stall_inclusion
        drive_proposals(recovery_nodes, rounds=8)
        wait_finalized(recovery_nodes, max(inclusion, 1), recovery_deadline, events)
        recovered = wait_settlement(stall_id, recovery_deadline, events)
        restore_counters = {i: stall_surface(i) for i in recovery_nodes}
        error = controls.recovery_verdict(
            recovered, inclusion, finalized_height(recovery_nodes[0])
        )
        final_stages = {deploy_id: settlement(deploy_id).get("stage") for deploy_id in accepted}
        loss = controls.no_silent_loss_verdict(accepted, final_stages)
        error = error or loss
        arms.append({
            "arm": "recovery",
            "deploy": stall_id,
            "inclusion": inclusion,
            "finalized": finalized_height(recovery_nodes[0]),
            "recovery_nodes": recovery_nodes,
            "not_ready": not_ready,
            "settlement": recovered,
            "counters": restore_counters,
            "accepted": accepted,
            "final_stages": final_stages,
            "result": "PASS" if error is None else "FAIL",
            "error": error,
        })
        if error is not None:
            raise RuntimeError(error)

        # Arm 4: a partitioned node may say unknown, never settled.
        victim = args.validators - 1
        victim_id = deploy_on(0, "Nil")
        accepted.append(victim_id)
        wait_deploy_processed(victim_id, time.monotonic() + args.window, events)
        # The victim must hold the inclusion block before it is cut off; otherwise its settlement
        # says notProcessed for a reason that has nothing to do with finality.
        wait_deploy_processed(victim_id, time.monotonic() + args.window, events, node=victim)
        docker("network", "disconnect", NETWORK, node_name(victim))
        partition_samples = []
        sample_deadline = time.time() + 4
        while time.time() < sample_deadline:
            code, body = container_get_allow_error(
                node_name(victim), f"/api/v1/deploy-settlement/{victim_id}"
            )
            if body is not None:
                partition_samples.append(body)
                record(out / "observations.jsonl", settlement="partition",
                       deploy=victim_id, node=victim, http_status=code, value=body)
            time.sleep(0.5)
        observation = container_get_allow_error(
            node_name(victim), "/api/v1/finality-observation"
        )
        error = controls.partition_stale_verdict(partition_samples, observation)
        docker("network", "connect", NETWORK, node_name(victim))
        drive_proposals(range(args.validators), rounds=5)
        arms.append({"arm": "partition-stale", "deploy": victim_id,
                     "result": "PASS" if error is None else "FAIL", "error": error,
                     "settlement": partition_samples, "observation": observation})
        if error is not None:
            raise RuntimeError(error)

        result = {"verdict": "PASS", "validators": args.validators, "stakes": stakes,
                  "live_at_stall": f"{live}/{total}"}
    except Exception as error:  # noqa: BLE001 - the verdict is the output
        try:
            rows = [health(i) for i in range(args.validators)]
        except Exception:  # noqa: BLE001
            rows = []
        try:
            last_events = events.read_text().splitlines()[-20:]
        except OSError:
            last_events = []
        drill.failure_envelope.write(
            out, "FAIL", error, phase="admission-finality-stall", arm="campaign",
            health=rows, events=last_events, capture_error=None,
        )
        result = {"verdict": "FAIL", "error": str(error), "traceback": traceback.format_exc()}

    (out / "campaign.json").write_text(json.dumps(
        {"gate_id": "n487-admission-finality-stall", "arms": arms}, indent=2))
    (out / "result.json").write_text(json.dumps(result, indent=2))
    print(json.dumps(result))
    return 0 if result["verdict"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
