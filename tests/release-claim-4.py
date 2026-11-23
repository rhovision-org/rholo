#!/usr/bin/env python3
"""RC-4 — recovery without loss: accepted work survives a validator restart.

Test: `python3 tools/release-check.py run --claim RC-4 --env local`

The test writes a marker, waits for it to settle, runs the restart-under-load harness (which kills
one validator mid-burst and restarts it), then reads the marker at the post-recovery finalised head
on every survivor and on the victim. `ProcessedWithSuccess` alone is not settlement; the marker at
the finalised head is.
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location(
    "release_claim_common", ROOT / "tests" / "release_claim_common.py")
common = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(common)

CLAIM_ID = "RC-4"
TITLE = "Recovery without loss"


def _fixture_baseline(arm: str = "local") -> dict:
    environment = "public" if arm == "public" else "local"
    record = {
        "schema": 1, "claim_id": CLAIM_ID, "claim_title": TITLE, "arm": arm,
        "environment": environment,
        "locality": "public-live" if environment == "public" else "local-devnet",
        "provenance": {"source_sha": "0" * 40, "binary_sha256": "1" * 64},
        "preconditions": [], "verdict": "pass", "reasons": [], "errors": [],
        "harness": {"name": "consensus-restart-under-load", "exit_code": 0},
        "observations": {
            "marker": "release-claim:fixture", "value": 42, "deploy_id": "ab" * 32,
            "harness_result": "PASS",
            "post_heads": [
                {"validator": i, "height": 40, "hash": "cd" * 32,
                 "marker_present": True, "inclusion_finalized": True}
                for i in range(4)
            ],
            "post_common_head": {
                "height": 40, "hash": "cd" * 32, "on_every_validator": True,
            },
            "boot_ids": {"before": {str(i): "a" for i in range(4)},
                         "after": {str(i): ("b" if i == 1 else "a") for i in range(4)}},
            "victim": 1,
            "certificates_after": {str(i): {"kind": "checkpoint"} for i in range(4)},
            "boot_traces": [{
                "kind": "boot", "stage": "acquisition", "validator": 1,
                "acquisition": "certificate", "accepted": True, "genesis_only": False,
                "outcome": "certified", "dag_len": 40, "store_head": None,
                "source_configured": True, "shard_id": "root",
                "served_supporting_stake": 300, "served_total_stake": 400,
            }],
        },
    }
    if environment == "public":
        record["observations"].update({
            "baseline": {"per_node": [], "common_height": 30, "common_hash": "cd" * 32},
            "rejoin": {"height": 40, "hash": "cd" * 32, "same_head": True,
                       "common_height": 40, "heights_above_baseline": 10},
            "survivor_deploy": {"deploy_id": "ab" * 32, "resolved": True},
            "own_key_halt": False, "self_replay": False, "regressions": [],
            "stop": {"exit_code": 0}, "start": {"exit_code": 0},
        })
    return record


def _check_baseline(record: dict) -> tuple[str, list[str]]:
    if record.get("verdict") == "not run":
        return "not run", list(record.get("preconditions") or record.get("reasons") or [])
    problems: list[str] = []
    observations = record.get("observations")
    if not isinstance(observations, dict):
        return "fail", ["observations: missing or not an object"]
    if observations.get("harness_result") != "PASS":
        problems.append(f"the restart harness did not pass: {observations.get('harness_result')!r}")
    heads = observations.get("post_heads")
    if not isinstance(heads, list) or len(heads) < 4:
        problems.append("post-recovery heads were not read on every validator")
        heads = []
    identities = {(row.get("height"), row.get("hash")) for row in heads if isinstance(row, dict)}
    if len(identities) > 1:
        # **A live chain advances between four sequential reads.** The claim is that the validators
        # agree on a finalised head, not that they are sampled at the same instant: the lowest head
        # read must be reported finalised by every validator. A differing *higher* head is a node
        # that has finalised one more block while the reads were in flight, not a split.
        common_head = observations.get("post_common_head") or {}
        if common_head.get("on_every_validator") is not True:
            problems.append(
                f"validators disagree on the post-recovery finalised head: {identities} "
                f"(the lowest head {common_head.get('height')}:{common_head.get('hash')} is not "
                "finalised on every validator)")
    for row in heads:
        if not isinstance(row, dict):
            problems.append("post-head row is not an object")
            continue
        if row.get("marker_present") is not True:
            problems.append(f"validator {row.get('validator')}: the marker is absent")
        if row.get("inclusion_finalized") is not True:
            problems.append(f"validator {row.get('validator')}: the inclusion block is not finalised")
    boot = observations.get("boot_ids") or {}
    before, after = boot.get("before") or {}, boot.get("after") or {}
    victim = str(observations.get("victim", 1))
    if before.get(victim) and before.get(victim) == after.get(victim):
        problems.append("the victim was not restarted (its boot time did not change)")
    certificates = observations.get("certificates_after")
    if isinstance(certificates, dict):
        if (certificates.get(victim) or {}).get("kind") != "checkpoint":
            problems.append(
                f"victim {victim} has no verified checkpoint certificate after restart")
    boot_traces = observations.get("boot_traces")
    victim_traces = ([t for t in boot_traces
                      if isinstance(t, dict) and common.trace_kind(t) == "boot"
                      and str(t.get("validator")) == victim]
                     if isinstance(boot_traces, list) else [])
    if victim_traces:
        # **The decision trace, not merely the last filename.** The boot writes an acquisition
        # trace and then a decision trace; the decision is the one that says whether the accepted
        # fringe was anchored. Trace filenames carry a per-process sequence, so "last by name" can
        # pick the acquisition trace once the sequence grows past a digit boundary.
        trace = next((t for t in reversed(victim_traces)
                      if t.get("stage") == "decision"), victim_traces[-1])
        if str(trace.get("acquisition", "")).startswith("refused"):
            problems.append(
                f"victim {victim}: the boot decision refused: {trace['acquisition']} "
                f"(dag {trace.get('dag_len')} block(s), store head {trace.get('store_head')}, "
                f"source configured {trace.get('source_configured')})")
        elif trace.get("accepted") is not True:
            problems.append(
                f"victim {victim}: the boot decision did not accept (acquisition "
                f"{trace.get('acquisition')})")
    if record.get("environment") == "public" and record.get("rehearsal") is not True:
        _public_problems(record, observations, boot, victim, victim_traces, problems)
    problems.extend(common.phase_space_reading(observations, bool(problems)))
    return common.evaluate(record, problems)


def _public_problems(record, observations, boot, victim, victim_traces, problems):
    """The coordinates only the fleet run can supply; a public record must carry them all."""
    for key in ("baseline", "rejoin", "survivor_deploy"):
        value = observations.get(key)
        if not isinstance(value, dict) or not value:
            problems.append(f"public recovery: {key} is missing")
    if "own_key_halt" not in observations:
        problems.append("public recovery: the own-key halt counter was not captured")
    if "self_replay" not in observations:
        problems.append("public recovery: the self-replay counter was not captured")
    if (boot.get("before") or {}).get(victim) is None or \
            (boot.get("after") or {}).get(victim) is None:
        problems.append(f"public recovery: victim {victim} has no before/after boot id")
    baseline = observations.get("baseline") or {}
    rejoin = observations.get("rejoin") or {}
    base_height = baseline.get("common_height")
    rejoin_height = rejoin.get("height")
    if type(base_height) is not int or type(rejoin_height) is not int:
        problems.append("public recovery: the baseline or rejoin height is missing")
    elif rejoin_height < base_height + 3:
        problems.append(
            f"the victim rejoined at {rejoin_height}, less than {base_height} + 3")
    if rejoin.get("same_head") is not True:
        problems.append("the victim did not rejoin the common finalised head")
    if (observations.get("survivor_deploy") or {}).get("resolved") is not True:
        problems.append("the survivor deploy did not resolve")
    if observations.get("own_key_halt") is not False:
        problems.append("a validator reports an own-key halt")
    if observations.get("self_replay") is not False:
        problems.append("a validator reports a self-replay refusal")
    if observations.get("regressions"):
        problems.append(f"finalised heads regressed: {observations['regressions']}")
    if not victim_traces:
        problems.append(
            f"evidence-insufficient: victim {victim} carries no boot trace")


FIXTURES = {
    "local": lambda: _fixture_baseline("local"),
    "public": lambda: _fixture_baseline("public"),
    "multi-host": lambda: common.multi_host_recovery_fixture(CLAIM_ID, TITLE),
    "failover": lambda: common.failover_fixture(CLAIM_ID, TITLE),
    "returner": lambda: common.returner_fixture(CLAIM_ID, TITLE),
}
ARMS = {
    "local": _check_baseline,
    "public": _check_baseline,
    "multi-host": common.check_multi_host_recovery,
    "failover": common.check_failover,
    "returner": common.check_returner,
}


def fixture(arm: str = "local") -> dict:
    return FIXTURES[arm]()


def check(record: dict) -> tuple[str, list[str]]:
    if record.get("verdict") == "not run":
        return "not run", list(record.get("preconditions") or record.get("reasons") or [])
    return common.dispatch_arm(ARMS, record)


def _run_returner(args, out: Path, record: dict) -> int:
    """The RC-4 half of one bond-leave campaign."""
    if getattr(args, "rehearse", False):
        return common.not_run(out, record,
                              "a rehearsal is not evidence for the returner arm")[1]
    failure = common.preflight_failure(CLAIM_ID, "local", out, record)
    if failure is not None:
        return failure
    try:
        common.bond_leave_arm_run(out, record, "returner",
                                  getattr(args, "campaign_result", None),
                                  keep=getattr(args, "keep", False))
    except common.ClaimError as error:
        common.finish(record, "not run", [str(error)])
        return common.write_record(out, record)[1]
    except Exception as error:  # noqa: BLE001 - a failed harness is a measured failure
        common.finish(record, "fail", [f"{type(error).__name__}: {error}"])
        return common.write_record(out, record)[1]
    verdict, reasons = check(record)
    common.finish(record, verdict, reasons)
    return common.write_record(out, record)[1]


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--env", choices=("local", "public"))
    parser.add_argument("--arm")
    parser.add_argument("--out", required=True)
    parser.add_argument("--public-manifest")
    parser.add_argument("--campaign-result")
    parser.add_argument("--head", help="the source SHA the operator run pins")
    parser.add_argument("--keep", action="store_true")
    parser.add_argument("--cycles", type=int, default=2)
    parser.add_argument("--rehearse", action="store_true")
    args = parser.parse_args(argv)
    arm = args.arm or args.env or "local"
    meta = common.claim_arm(CLAIM_ID, arm)
    environment = meta["environment"]
    record = common.new_record(CLAIM_ID, TITLE, environment, meta["locality"], arm=arm,
                               rehearsal=args.rehearse)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    if arm == "returner":
        return _run_returner(args, out, record)
    if arm == "failover":
        if not args.campaign_result:
            return common.not_run(
                out, record, "the failover arm needs --campaign-result <two-host run dir>")[1]
        return common.run_failover_arm(out, record, args.campaign_result, head=args.head)
    if arm == "multi-host":
        if not args.campaign_result:
            return common.not_run(
                out, record, "the multi-host arm needs --campaign-result <M4 observations>")[1]
        return common.run_multi_host_arm(out, record, args.campaign_result, head=args.head)
    if arm not in ("local", "public"):
        return common.not_run(
            out, record,
            f"the {arm} arm's harness lands in {meta['next_pr']}")[1]
    failure = common.preflight_failure(CLAIM_ID, environment, out, record,
                                       getattr(args, "public_manifest", None),
                                       rehearsal=args.rehearse)
    if failure is not None:
        return failure

    if environment == "public":
        manifest = common.load_manifest(args.public_manifest)
        if not manifest:
            return common.not_run(out, record, "no --public-manifest for the public fleet")[1]
        target = common.public_target(manifest)
        fleet_result, code = _recover(args, out / "fleet-recovery")
        record["provenance"] = manifest.get("provenance") or {}
        record["harness"] = {"name": "fleet-run recovery", "exit_code": code,
                             "out": str(out / "fleet-recovery")}
        _map_recovery(record, fleet_result, target)
        if code == 3:
            return common.not_run(
                out, record,
                "; ".join(fleet_result.get("reasons") or ["the recovery precondition failed"]))[1]
        if code != 0:
            common.finish(
                record, "fail",
                ["; ".join(fleet_result.get("reasons") or ["the recovery run failed"])])
            return common.write_record(out, record)[1]
        return _post_reads(target, record, out)

    # **The floor scales with the store.** The arm used 4 GiB until 2026-11-21; at 1,139 persisted
    # blocks a 4 GiB node reads 4,087 MiB available against the 4,096 floor and refuses the resource
    # preflight, which would confound the boot decision with a memory refusal. 6 GiB leaves headroom
    # for the restart's populated store; 2 GiB fails the preflight outright.
    devnet = common.Devnet("restartload", validators=4, memory="6g")
    try:
        devnet.up()
        record["provenance"] = devnet.provenance()
        marker = "release-claim:" + str(time.time_ns())
        value = common.MARKER_VALUE
        deploy_id = devnet.deploy(f'@{json.dumps(marker)}!({value})')
        target = devnet.target()
        try:
            common.wait_until(
                "the marker is present at every finalised head before the restart",
                lambda: all(common.marker_present(
                    target.probe(i, marker, target.finalized_head(i)[1]), value)
                    for i in range(4)), timeout=300)
        except common.ClaimError as error:
            record["preconditions"].append(
                "RC-1's finalised-marker retention failed; no restart was attempted")
            common.finish(record, "not run", [str(error)])
            return common.write_record(out, record)[1]
        # **A node with blocks but no verified checkpoint certificate refuses to restart by
        # design.** Record which validators hold one before the kill. The probe is bounded and does
        # not block the run: the harness's own restart is the observation of interest.
        certificates = _certificates(target, timeout=300)
        record["observations"]["checkpoint_certificates"] = certificates
        before = _boot_ids(devnet)
        harness_out = out / "restart"
        command = [sys.executable, str(ROOT / "tests" / "consensus-restart-under-load.py"),
                   "--validators", "4", "--cycles", str(args.cycles), "--window", "240",
                   "--load-seconds", "20", "--out", str(harness_out)]
        result = subprocess.run(command, cwd=ROOT, env=devnet.env(), text=True,
                                capture_output=True, timeout=2400)
        (out / "restart.log").write_text(result.stdout + result.stderr)
        after = _boot_ids(devnet)
        result_doc = _read_json(harness_out / "result.json") or {}
        # The harness's first victim is validator 1, not a constant; derive it from the run.
        victim = _last_victim(harness_out)
        if victim is None:
            victim = 1
        certificates_after = _certificates(target, timeout=0)
        boot_traces = []
        for i in range(4):
            dest = out / "traces" / f"validator-{i}"
            devnet.copy_traces(i, dest)
            for path in sorted(dest.glob("boot-*.json")):
                try:
                    row = json.loads(path.read_text())
                except ValueError:
                    continue
                boot_traces.append({"validator": i, "file": path.name, **row})
        record["observations"]["boot_traces"] = boot_traces
        record["harness"] = {"name": "consensus-restart-under-load", "command": command,
                             "exit_code": result.returncode, "out": str(harness_out)}
        record["observations"].update({
            "marker": marker, "value": value, "deploy_id": deploy_id,
            "harness_result": result_doc.get("verdict") or (
                "PASS" if result.returncode == 0 else "FAIL"),
            "boot_ids": {"before": before, "after": after},
            "victim": victim,
            "certificates_after": certificates_after,
        })
        if result.returncode:
            victim_log = subprocess.run(
                ["docker", "logs", devnet.node(victim)],
                text=True, capture_output=True, timeout=60)
            text = victim_log.stdout + victim_log.stderr
            # **The refusal is the line that names the decision, and the terminal-sync sentence
            # is the one that names a boot that never served.** The old scan looked for
            # "checkpoint certificate" and so printed the collector's per-tick warning while the
            # actual stop — "LFS state sync is terminal: the approved state could not be restored
            # and the retries are spent" — was filtered out. Read the stop, not the tick.
            refusal = [line for line in text.splitlines()
                       if any(word in line.lower() for word in
                              ("checkpoint certificate", "setup error", "refuses to run",
                               "cannot be attributed", "fatal:", "state sync is terminal",
                               "retries are spent", "could not be restored", "blocked"))]
            record["observations"]["harness_error"] = (
                (result.stdout + result.stderr)[-1500:])
            record["observations"]["victim_refusal"] = "\n".join(refusal[-6:])
            reason = ("the restart harness failed: " +
                      str((result_doc or {}).get("error")
                          or record["observations"]["harness_error"]))
            if refusal:
                reason += " — " + refusal[-1]
            else:
                # A failed restart that names no refusal is still an observation: keep the tail so
                # the next reader does not have to re-run the drill to see where the node stopped.
                tail = [line for line in text.splitlines() if line.strip()][-8:]
                if tail:
                    record["observations"]["victim_log_tail"] = "\n".join(tail)
                    reason += " — victim log tail: " + " | ".join(tail[-3:])
            if (certificates_after.get(str(victim)) or {}).get("kind") != "checkpoint":
                reason += f"; victim {victim} restarted without a verified checkpoint certificate"
            victim_trace = next(
                (t for t in reversed(boot_traces)
                 if str(t.get("validator")) == str(victim)
                 and t.get("stage") == "decision"),
                next((t for t in reversed(boot_traces)
                      if str(t.get("validator")) == str(victim)), None))
            if victim_trace:
                if str(victim_trace.get("acquisition", "")).startswith("refused"):
                    reason += (f"; victim {victim} boot trace: {victim_trace['acquisition']} "
                               f"(dag {victim_trace.get('dag_len')} block(s), store head "
                               f"{victim_trace.get('store_head')}, source configured "
                               f"{victim_trace.get('source_configured')})")
                elif victim_trace.get("accepted") is not True:
                    reason += (f"; victim {victim} boot trace: accepted=false (acquisition "
                               f"{victim_trace.get('acquisition')})")
            else:
                reason += f"; evidence-insufficient: victim {victim} carries no boot trace"
            common.finish(record, "fail", [reason])
            return common.write_record(out, record)[1]
        return _post_reads(target, record, out)
    except Exception as error:  # noqa: BLE001
        common.finish(record, "fail", [f"{type(error).__name__}: {error}"])
        return common.write_record(out, record)[1]
    finally:
        if not args.keep:
            devnet.down()


def _recover(args, out: Path) -> tuple[dict, int]:
    """Run the driver's recovery; the driver performs the stop and captures the outcome."""
    out.mkdir(parents=True, exist_ok=True)
    command = [sys.executable, str(ROOT / "tools" / "fleet-run.py"), "recovery",
               "--manifest", args.public_manifest, "--out", str(out), "--claim", CLAIM_ID]
    if args.rehearse:
        command.append("--rehearse")
    try:
        done = subprocess.run(command, cwd=ROOT, text=True, capture_output=True, timeout=7200)
    except subprocess.TimeoutExpired as error:
        return {"reasons": [f"fleet-run recovery timed out: {error}"]}, 3
    path = out / "fleet-result.json"
    try:
        return json.loads(path.read_text()), done.returncode
    except (OSError, ValueError):
        return {"reasons": [(done.stdout + done.stderr)[-800:]]}, done.returncode or 3


def _map_recovery(record: dict, result: dict, target) -> None:
    deploy = result.get("deploy") or {}
    observations = record["observations"]
    observations.update({
        "marker": deploy.get("marker"),
        "value": int(deploy.get("value", common.MARKER_VALUE)),
        "deploy_id": deploy.get("deploy_id"),
        "harness_result": result.get("harness_result"),
        "boot_ids": result.get("boot_ids") or {},
        "victim": result.get("victim"),
        "certificates_after": result.get("certificates_after") or {},
        "baseline": result.get("baseline") or {},
        "rejoin": result.get("rejoin") or {},
        "survivor_deploy": result.get("survivor_deploy") or {},
        "own_key_halt": result.get("own_key_halt"),
        "self_replay": result.get("self_replay"),
        "regressions": result.get("regressions") or [],
        "stop": result.get("stop"),
        "start": result.get("start"),
    })
    traces = []
    for row in result.get("boot_traces") or []:
        if common.trace_kind(row) != "boot":
            continue
        index = row.get("validator")
        name = (target.name(index)
                if isinstance(index, int) and 0 <= index < len(target.nodes) else str(index))
        traces.append({**row, "validator": name})
    observations["boot_traces"] = traces
    observations["fleet_errors"] = result.get("errors") or []


def _certificates(target, timeout: float = 300) -> dict:
    """Which validators hold a verified checkpoint anchor, after a bounded wait.

    The bootstrap mints the certificate; the test records which nodes hold it rather than
    assuming. A validator without one cannot attribute its restored state, and its restart will
    say so — that is an observation, not a reason to skip the run.
    """
    deadline = time.monotonic() + timeout
    found: dict = {}
    while True:
        found = {}
        for i in range(len(target.nodes)):
            try:
                status = target.get(i, "/api/v1/status/operator", admin=True)
            except Exception:  # noqa: BLE001
                status = {}
            checkpoint = (status or {}).get("checkpoint") or {}
            found[str(i)] = {
                "kind": checkpoint.get("kind"),
                "digest": checkpoint.get("digest"),
                "anchorHeight": checkpoint.get("anchorHeight"),
            }
        if all(entry.get("kind") == "checkpoint" for entry in found.values()):
            return found
        if time.monotonic() >= deadline:
            return found
        time.sleep(5)


def _last_victim(harness_out: Path) -> int | None:
    """The victim index the harness was working on when it stopped.

    The harness records the cycle's victim in its event stream before the kill; a failed run does
    not reach `campaign.json`, so the event stream is the only place the index survives.
    """
    victim = None
    try:
        for line in (harness_out / "observations.jsonl").read_text().splitlines():
            try:
                event = json.loads(line)
            except ValueError:
                continue
            if isinstance(event.get("victim"), int):
                victim = event["victim"]
    except OSError:
        return None
    return victim


def _read_json(path: Path):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return None


def _boot_ids(devnet) -> dict:
    ids = {}
    for i in range(devnet.validators):
        try:
            ids[str(i)] = subprocess.check_output(
                ["docker", "inspect", "-f", "{{.State.StartedAt}}", devnet.node(i)],
                text=True, stderr=subprocess.DEVNULL).strip()
        except Exception:
            ids[str(i)] = None
    return ids


def _post_reads(target, record: dict, out: Path) -> int:
    observations = record["observations"]
    try:
        heads = []
        for i in range(len(target.nodes)):
            height, block_hash = target.finalized_head(i)
            response = target.probe(i, observations["marker"], block_hash)
            heads.append({
                "validator": i, "height": height, "hash": block_hash,
                "marker_present": common.marker_present(
                    response, int(observations.get("value", common.MARKER_VALUE))),
                "inclusion_finalized": target.is_finalized(i, block_hash),
            })
        observations["post_heads"] = heads
        # **The common finalised head.** The four reads are sequential on a producing chain; the
        # lowest one is the height every node must at least have finalised. Verify it on every node
        # so a genuine split (a head that later differs) is still caught, while a node that simply
        # advanced between reads is not reported as disagreement.
        if heads:
            lowest = min(heads, key=lambda row: row["height"])
            observations["post_common_head"] = {
                "height": lowest["height"],
                "hash": lowest["hash"],
                "on_every_validator": all(
                    target.is_finalized(i, lowest["hash"])
                    for i in range(len(target.nodes))
                ),
            }
    except Exception as error:  # noqa: BLE001
        common.finish(record, "fail", [f"{type(error).__name__}: {error}"])
        return common.write_record(out, record)[1]
    verdict, reasons = check(record)
    common.finish(record, verdict, reasons)
    return common.write_record(out, record)[1]


if __name__ == "__main__":
    raise SystemExit(run(sys.argv[1:]))
