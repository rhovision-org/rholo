#!/usr/bin/env python3
"""RC-1 — truthful settlement: a confirmed settlement is a retained finalised write.

Test: `python3 tools/release-check.py run --claim RC-1 --env local`

The test runs the acceptance drill (one marker deploy, retained through two further epoch
boundaries), then reads the settlement route for the same deploy id on every validator and the
marker at every validator's finalised head. It also scans the blocks after inclusion for the deploy
id in `rejectedDeploys`. A settlement that says `settlementConfirmed` while the effect is gone is a
fail, and that is the check-1 defect this test exists to catch.
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

CLAIM_ID = "RC-1"
TITLE = "Truthful settlement"


def _fixture_baseline(arm: str = "local") -> dict:
    environment = "public" if arm == "public" else "local"
    return {
        "schema": 1, "claim_id": CLAIM_ID, "claim_title": TITLE, "arm": arm,
        "environment": environment,
        "locality": "public-live" if environment == "public" else "local-devnet",
        "provenance": {"source_sha": "0" * 40, "binary_sha256": "1" * 64},
        "preconditions": [], "verdict": "pass", "reasons": [], "errors": [],
        "harness": {"name": "consensus-acceptance-drill", "exit_code": 0},
        "observations": {
            "drill_result": "pass",
            "deploy_id": "ab" * 32,
            "marker": "acceptance:fixture",
            "value": 42,
            "control_absent_before": True,
            "settlements": [
                {"validator": i, "stage": "finalizedSuccess",
                 "settlementConfirmed": True, "execution": "succeeded",
                 "blockHash": "cd" * 32, "blockNumber": 14}
                for i in range(4)
            ],
            "finalised": [
                {"validator": i, "height": 34, "hash": "ef" * 32,
                 "marker_present": True, "inclusion_finalized": True}
                for i in range(4)
            ],
            "rejected": [{"validator": i, "found": False} for i in range(4)],
            "traces": [{
                "kind": "merge", "validator": 0, "parents": ["ab" * 32],
                "mode": "fringe",
                "claims": [{"height": 3, "id": "ab" * 32, "hash": "cd" * 32}],
                "certified_base": None, "base_source": "agreed",
                "conflict_scope": ["ef" * 32], "final_scope": ["ef" * 32],
                "fork_blocks": [], "rejected_hosts": [], "rejected_user_hosts": [],
                "report": {"conflict_chains": 1, "kept_chains": 0, "rejected_chains": 1},
            }],
        },
    }


def _check_baseline(record: dict) -> tuple[str, list[str]]:
    if record.get("verdict") == "not run":
        return "not run", list(record.get("preconditions") or record.get("reasons") or [])
    problems: list[str] = []
    observations = record.get("observations")
    if not isinstance(observations, dict):
        return "fail", ["observations: missing or not an object"]
    if observations.get("drill_result") != "pass":
        problems.append("the acceptance drill did not pass (marker retention)")
    if observations.get("control_absent_before") is not True:
        problems.append("the absent-marker control did not hold before the write")
    if not observations.get("deploy_id") or not observations.get("marker"):
        problems.append("the record carries no deploy id and marker")
    settlements = observations.get("settlements")
    finalised = observations.get("finalised")
    rejected = observations.get("rejected")
    if not isinstance(settlements, list) or not settlements:
        problems.append("settlements: missing or empty")
        settlements = []
    if not isinstance(finalised, list) or not finalised:
        problems.append("finalised: missing or empty")
        finalised = []
    if not isinstance(rejected, list):
        problems.append("rejected: missing or not a list")
        rejected = []
    confirmed = [row for row in settlements
                 if isinstance(row, dict) and row.get("stage") == "finalizedSuccess"
                 and row.get("settlementConfirmed") is True]
    if not confirmed:
        problems.append("no settlement reported finalizedSuccess with settlementConfirmed true")
    identities = {(row.get("blockHash"), row.get("blockNumber")) for row in settlements
                  if isinstance(row, dict)}
    if len(identities) > 1:
        problems.append(f"validators disagree on the settlement inclusion block: {identities}")
    inclusion_height = max((row.get("blockNumber") for row in settlements
                            if isinstance(row, dict) and type(row.get("blockNumber")) is int),
                           default=None)
    if inclusion_height is None:
        problems.append("settlements carry no inclusion height")
    validators = {row.get("validator") for row in finalised if isinstance(row, dict)}
    if len(validators) < 4:
        problems.append(f"finalised heads read on {len(validators)} validators, expected 4")
    for row in finalised:
        if not isinstance(row, dict):
            problems.append("finalised row is not an object")
            continue
        name = row.get("validator")
        height = row.get("height")
        if inclusion_height is not None and (type(height) is not int or height < inclusion_height):
            problems.append(f"validator {name}: finalised height {height} is below inclusion {inclusion_height}")
        if row.get("marker_present") is not True:
            problems.append(f"validator {name}: the marker is absent at its finalised head")
        if row.get("inclusion_finalized") is not True:
            problems.append(f"validator {name}: the inclusion block is not finalised")
    if len(rejected) < 4:
        problems.append("rejectedDeploys was not scanned on every validator")
    for row in rejected:
        if isinstance(row, dict) and row.get("found"):
            problems.append(f"validator {row.get('validator')}: the deploy appears in rejectedDeploys")

    # **Read the failure against the proven phase space.** The predicates are recomputed from the
    # trace coordinates; the node's own labels are not trusted. A failure with every predicate true
    # is a model gap, not a guessed cause.
    bad_outcome = any(isinstance(row, dict) and row.get("marker_present") is not True
                      for row in finalised) or any(
        isinstance(row, dict) and row.get("found") for row in rejected)
    problems.extend(common.phase_space_reading(observations, bad_outcome))
    return common.evaluate(record, problems)


FIXTURES = {
    "local": lambda: _fixture_baseline("local"),
    "public": lambda: _fixture_baseline("public"),
    "bond-leave": lambda: common.bond_leave_fixture(CLAIM_ID, TITLE),
}
ARMS = {
    "local": _check_baseline,
    "public": _check_baseline,
    "bond-leave": common.check_bond_leave,
}


def fixture(arm: str = "local") -> dict:
    return FIXTURES[arm]()


def check(record: dict) -> tuple[str, list[str]]:
    if record.get("verdict") == "not run":
        return "not run", list(record.get("preconditions") or record.get("reasons") or [])
    return common.dispatch_arm(ARMS, record)


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--env", choices=("local", "public"))
    parser.add_argument("--arm")
    parser.add_argument("--out", required=True)
    parser.add_argument("--public-manifest")
    parser.add_argument("--keep", action="store_true")
    parser.add_argument("--window", type=int, default=240)
    parser.add_argument("--rehearse", action="store_true")
    args = parser.parse_args(argv)
    arm = args.arm or args.env or "local"
    meta = common.claim_arm(CLAIM_ID, arm)
    environment = meta["environment"]
    record = common.new_record(CLAIM_ID, TITLE, environment, meta["locality"], arm=arm,
                               rehearsal=args.rehearse)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
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
        deploy = manifest.get("deploy") or {}
        target = common.public_target(manifest)
        value = int(deploy.get("value", common.MARKER_VALUE))
        try:
            _control_absent(target, deploy["marker"], value)
        except common.ClaimError as error:
            return common.not_run(out, record, str(error))[1]
        fleeted, code = _fleet("deploy", args, out / "fleet-deploy")
        if code == 3:
            return common.not_run(
                out, record,
                _reason(fleeted, "the deploy did not reach every validator"))[1]
        if code != 0:
            common.finish(record, "fail",
                          [_reason(fleeted, "the fleet deploy failed")])
            return common.write_record(out, record)[1]
        submitted = fleeted.get("deploy") or {}
        record["provenance"] = manifest.get("provenance") or {}
        record["harness"] = {"name": "fleet-run deploy", "exit_code": code,
                             "out": str(out / "fleet-deploy")}
        record["observations"] = {
            "drill_result": None,
            "control_absent_before": True,
            "deploy_id": submitted.get("deploy_id"),
            "marker": submitted.get("marker") or deploy.get("marker"),
            "value": value,
        }
        return _observe(target, record, submitted, out, len(target.nodes),
                        actions=manifest.get("actions") or {},
                        traces_loader=lambda: _fleet_traces(args, out))

    # The drill reads the default HTTP/admin ports; the gRPC host ports are separate and one of
    # them can be held by an unrelated process on a shared machine, so they move.
    devnet = common.Devnet("acceptance", validators=4, network="acceptance", grpc_base=45402,
                           extra_up=("--no-autopropose", "--epoch-length", "10"))
    try:
        devnet.up()
        drill_out = out / "drill"
        command = [sys.executable, str(ROOT / "tools" / "consensus-acceptance-drill.py"),
                   "--validators", "4", "--window", str(args.window),
                   "--out", str(drill_out), "--local"]
        result = subprocess.run(command, cwd=ROOT, env=devnet.env(), text=True,
                                capture_output=True, timeout=args.window + 600)
        (out / "drill.log").write_text(result.stdout + result.stderr)
        record["harness"] = {"name": "consensus-acceptance-drill", "command": command,
                             "exit_code": result.returncode, "out": str(drill_out)}
        record["provenance"] = devnet.provenance()
        summary_path = drill_out / "workload-result.json"
        summary = json.loads(summary_path.read_text()) if summary_path.exists() else {}
        drill_ok = result.returncode == 0 and summary.get("result") == "pass"
        record["observations"] = {
            "drill_result": "pass" if drill_ok else "fail",
            "drill_error": None if drill_ok else (result.stdout + result.stderr)[-1500:],
            "deploy_id": summary.get("deploy_id"),
            "marker": summary.get("marker"),
            "value": summary.get("value", common.MARKER_VALUE),
            "control_absent_before": drill_ok,
        }
        if not drill_ok:
            # The refused merge is the observation. Copy the decision traces before the devnet is
            # torn down, or the failure reads as evidence-insufficient and names no predicate.
            record["observations"]["traces"] = _collect_merge_traces(devnet, out)
            common.finish(record, "fail", ["the acceptance drill failed: " +
                                           str(record["observations"]["drill_error"])])
            return common.write_record(out, record)[1]
        return _observe(devnet.target(), record, summary, out, 4, devnet=devnet)
    except Exception as error:  # noqa: BLE001 - the record is the report
        common.finish(record, "fail", [f"{type(error).__name__}: {error}"])
        return common.write_record(out, record)[1]
    finally:
        if not args.keep:
            devnet.down()


def _fleet(action: str, args, out: Path) -> tuple[dict, int]:
    """Run one driver action against the campaign manifest; return its result and exit code."""
    out.mkdir(parents=True, exist_ok=True)
    command = [sys.executable, str(ROOT / "tools" / "fleet-run.py"), action,
               "--manifest", args.public_manifest, "--out", str(out), "--claim", CLAIM_ID]
    if args.rehearse:
        command.append("--rehearse")
    try:
        done = subprocess.run(command, cwd=ROOT, text=True, capture_output=True, timeout=3600)
    except subprocess.TimeoutExpired as error:
        return {"reasons": [f"fleet-run {action} timed out: {error}"]}, 3
    path = out / "fleet-result.json"
    try:
        document = json.loads(path.read_text())
    except (OSError, ValueError):
        document = {"reasons": [(done.stdout + done.stderr)[-800:]]}
    return document, done.returncode


def _fleet_traces(args, out: Path) -> list[dict]:
    document, code = _fleet("traces", args, out / "fleet-traces")
    if code != 0:
        return []
    return [row for row in (document.get("traces") or []) if isinstance(row, dict)]


def _reason(document: dict, fallback: str) -> str:
    reasons = document.get("reasons") or document.get("errors") or []
    return "; ".join(str(reason) for reason in reasons) or fallback


def _control_absent(target, marker: str, value: int) -> None:
    """The marker must be absent before the write; a pre-existing marker is not a control."""
    for index in range(len(target.nodes)):
        _, block_hash = target.finalized_head(index)
        if common.marker_present(target.probe(index, marker, block_hash), value):
            raise common.ClaimError(
                f"the marker {marker!r} is already present on {target.name(index)} before the "
                "deploy; the absent-marker control did not hold")


def _retention_samples(target, settlements: list[dict], marker: str, value: int,
                       actions: dict, validators: int) -> list[dict]:
    """The marker at the finalised head across two further epoch boundaries."""
    epoch = int(actions.get("epoch_length") or 0)
    epochs = int(actions.get("retention_epochs", 2))
    inclusion = min((row.get("blockNumber") for row in settlements
                     if type(row.get("blockNumber")) is int), default=None)
    if not epoch or inclusion is None:
        raise common.ClaimError(
            "retention: the settlement carries no inclusion height or the manifest no epoch length")
    first = ((inclusion // epoch) + 1) * epoch
    boundaries = [first + epoch * index for index in range(epochs)]
    samples: list[dict] = []
    deadline = time.monotonic() + 900
    for boundary in boundaries:
        while True:
            heads = [target.finalized_head(index) for index in range(validators)]
            if all(height >= boundary for height, _ in heads):
                break
            if time.monotonic() >= deadline:
                raise common.ClaimError(
                    f"retention: no finalised head reached the epoch boundary {boundary}")
            time.sleep(5)
        for index, (height, block_hash) in enumerate(heads):
            samples.append({
                "validator": index, "boundary": boundary, "height": height,
                "hash": block_hash,
                "marker_present": common.marker_present(
                    target.probe(index, marker, block_hash), value),
            })
    return samples


def _collect_merge_traces(devnet, out: Path, validators: int = 4) -> list[dict]:
    """Every node's merge traces, copied before the devnet is torn down."""
    traces: list[dict] = []
    for i in range(validators):
        dest = out / "traces" / f"validator-{i}"
        devnet.copy_traces(i, dest)
        for path in sorted(dest.glob("merge-*.json")):
            try:
                row = json.loads(path.read_text())
            except ValueError:
                continue
            traces.append({"validator": i, "file": path.name, **row})
    return traces


def _observe(target, record: dict, source: dict, out: Path, validators: int,
             devnet=None, actions: dict | None = None, traces_loader=None) -> int:
    deploy_id = source["deploy_id"]
    marker = source["marker"]
    value = int(source.get("value", common.MARKER_VALUE))
    settlements, finalised, rejected = [], [], []

    def read_settlements() -> list[dict]:
        rows = []
        for i in range(validators):
            settlement = target.settlement(i, deploy_id)
            rows.append({"validator": i, **{key: settlement.get(key) for key in
                         ("stage", "settlementConfirmed", "execution",
                          "blockHash", "blockNumber")}})
        return rows

    try:
        settlements = read_settlements()
        inclusion = {row["blockHash"] for row in settlements if row.get("blockHash")}
        # Finality is asynchronous; the drill stops as soon as its own barrier passes. Drive a
        # bounded catch-up so the inclusion block is finalised on every validator before the
        # cross-validator read, rather than reading one validator a block behind.
        if inclusion:
            deadline = time.monotonic() + 240
            while time.monotonic() < deadline:
                if all(target.is_finalized(i, h) for i in range(validators)
                       for h in inclusion):
                    break
                if devnet is not None:
                    try:
                        devnet.deploy("Nil", timeout=30)
                    except Exception:  # noqa: BLE001 - the wait is the observation
                        pass
                time.sleep(5)
        # The settlement read can race the finality advance: it named includedUnfinalized while
        # the inclusion block was one finality step behind. Re-read after the wait, and give the
        # route a bounded window to observe its own finalised fringe.
        deadline = time.monotonic() + 120
        while True:
            settlements = read_settlements()
            if all(row.get("stage") == "finalizedSuccess"
                   and row.get("settlementConfirmed") is True for row in settlements):
                break
            if time.monotonic() >= deadline:
                break
            time.sleep(3)
        for i in range(validators):
            height, block_hash = target.finalized_head(i)
            response = target.probe(i, marker, block_hash)
            finalised.append({
                "validator": i, "height": height, "hash": block_hash,
                "marker_present": common.marker_present(response, value),
                "inclusion_finalized": all(target.is_finalized(i, h) for h in inclusion if h),
            })
        if actions is not None:
            # The public arm has no drill summary: retention is sampled from the live chain, at
            # two further epoch boundaries, and `drill_result` is derived from those reads.
            retention = _retention_samples(target, settlements, marker, value, actions, validators)
            record["observations"]["retention"] = retention
            record["observations"]["drill_result"] = (
                "pass" if retention and all(row["marker_present"] for row in retention)
                else "fail")
        inclusion_height = min((row["blockNumber"] for row in settlements
                                if type(row.get("blockNumber")) is int), default=0)
        for i in range(validators):
            height, _ = target.finalized_head(i)
            end = min(height, inclusion_height + 512)
            found = False
            if end > inclusion_height:
                blocks = target.get(i, f"/api/blocks/{inclusion_height}/{end}")
                rows = blocks if isinstance(blocks, list) else (blocks or {}).get("blocks") or []
                found = any(deploy_id in (row.get("rejectedDeploys") or [])
                            for row in rows if isinstance(row, dict))
            rejected.append({"validator": i, "found": found, "scanned_to": end,
                             "truncated": end < height})
        traces = []
        if devnet is not None:
            for i in range(validators):
                dest = out / "traces" / f"validator-{i}"
                devnet.copy_traces(i, dest)
                for path in sorted(dest.glob("merge-*.json")):
                    try:
                        row = json.loads(path.read_text())
                    except ValueError:
                        continue
                    traces.append({"validator": i, "file": path.name, **row})
        elif traces_loader is not None:
            try:
                traces = traces_loader() or []
            except Exception as error:  # noqa: BLE001 - a missing trace is evidence-insufficient
                record["observations"]["trace_error"] = f"{type(error).__name__}: {error}"
        record["observations"].update({
            "deploy_id": deploy_id, "marker": marker, "value": value,
            "settlements": settlements, "finalised": finalised, "rejected": rejected,
            "traces": traces,
            "control_absent_before": True,
        })
    except Exception as error:  # noqa: BLE001
        if isinstance(error, common.ClaimError) and str(error).startswith("retention:"):
            common.finish(record, "not run", [str(error)])
        else:
            common.finish(record, "fail", [f"{type(error).__name__}: {error}"])
        return common.write_record(out, record)[1]
    verdict, reasons = check(record)
    common.finish(record, verdict, reasons)
    return common.write_record(out, record)[1]


if __name__ == "__main__":
    raise SystemExit(run(sys.argv[1:]))
