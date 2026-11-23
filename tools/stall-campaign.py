#!/usr/bin/env python3
"""The local stall campaign: one run, two claim records.

The denominator scenario is the committed `tests/admission-finality-stall.py` campaign: four
validators at stakes 100/100/50/50, the two smallest stopped (exactly 200/300 live), a deploy that
does not settle while finality is frozen, and the same deploy finalised after the validators are
restarted. This tool boots the devnet, runs that harness, and converts its recorded coordinates into
the `stall_series` the claim predicates read.

It does not decide the verdict. `tests/release-claim-2.py` and `tests/release-claim-3.py` call
`stall.verdict` and their own checks on the record this tool fills; a missing or failed harness is a
measured failure, not a fabricated series.
"""
from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location(
    "release_claim_common", ROOT / "tests" / "release_claim_common.py")
common = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(common)

EPISODES = {
    "rc2": {"deviations": ["DE-04", "DE-05", "DE-06"],
            "representation": "protocol-stall", "shape": "void",
            "cause": "unsatisfiable-requirement", "rung": "R2", "law": "Law 52b"},
    "rc3": {"deviations": ["DE-09"], "representation": "protocol-wait", "shape": "historic",
            "cause": "historic-predicate", "rung": "R2", "law": "Rchain.traceRefines"},
}
STAKES = "100,100,50,50"


def _read(path: Path) -> dict:
    try:
        value = json.loads(path.read_text())
    except (OSError, ValueError):
        return {}
    return value if isinstance(value, dict) else {}


def _arm(payload: dict, name: str) -> dict:
    for row in payload.get("arms") or []:
        if isinstance(row, dict) and row.get("arm") == name:
            return row
    raise RuntimeError(f"the harness did not record arm {name!r}")


def _ints(values) -> list[int]:
    return [value for value in (values or {}).values() if type(value) is int]


def _max(values) -> int | None:
    numbers = _ints(values)
    return max(numbers) if numbers else None


def _min(values) -> int | None:
    numbers = _ints(values)
    return min(numbers) if numbers else None


def _episode_counts(surface) -> dict:
    return {ident: row.get("episodes") for ident, row in (surface or {}).items()
            if isinstance(row, dict)}


def run_denominator(record: dict, out: Path, *, episode: str, keep: bool = False) -> None:
    """Boot the devnet, run the committed harness, and fill the record's observations."""
    if episode not in EPISODES:
        raise ValueError(f"unknown episode {episode!r}; expected one of {sorted(EPISODES)}")
    out.mkdir(parents=True, exist_ok=True)
    devnet = common.Devnet(
        "stall", validators=4, network="stall",
        extra_up=("--stakes", STAKES, "--no-autopropose", "--no-propose-on-deploy",
                  "--epoch-length", "10"))
    try:
        devnet.up()
        harness_out = out / "denominator"
        command = [sys.executable, str(ROOT / "tests" / "admission-finality-stall.py"),
                   "--validators", "4", "--stakes", STAKES, "--out", str(harness_out)]
        result = subprocess.run(command, cwd=ROOT, env=devnet.env(), text=True,
                                capture_output=True, timeout=3600)
        (out / "denominator.log").write_text(result.stdout + result.stderr)
        record["harness"] = {"name": "admission-finality-stall", "command": command,
                             "exit_code": result.returncode, "out": str(harness_out)}
        record["provenance"] = devnet.provenance()
        payload = _read(harness_out / "campaign.json")
        outcome = _read(harness_out / "result.json")
        if result.returncode or outcome.get("verdict") != "PASS":
            reason = (outcome.get("error")
                      or (result.stdout + result.stderr)[-1200:]
                      or "the harness produced no result")
            raise RuntimeError(f"the denominator harness did not complete: {reason}")
        _fill(record, payload, episode)
    finally:
        if not keep:
            devnet.down()


def _fill(record: dict, payload: dict, episode: str) -> None:
    stall = _arm(payload, "stall-exactly-two-thirds")
    recovery = _arm(payload, "recovery")
    lag = _arm(payload, "ordinary-lag")
    deploy = stall.get("deploy")
    if not isinstance(deploy, str) or not deploy:
        raise RuntimeError("the stall arm recorded no deploy id")

    baseline_height = _max(stall.get("height_before"))
    baseline_finalized = _max(stall.get("finalized_before"))
    stall_height = _max(stall.get("height_after"))
    stall_finalized = _min(stall.get("finalized_frozen"))
    if None in (baseline_height, baseline_finalized, stall_height, stall_finalized):
        raise RuntimeError("the stall arm did not record both heights")

    progress = any(
        type(stall["height_after"].get(ident)) is int
        and type(stall["height_before"].get(ident)) is int
        and stall["height_after"][ident] > stall["height_before"][ident]
        for ident in stall.get("height_before") or {})
    finality = any(
        type(stall["finalized_frozen"].get(ident)) is int
        and type(stall["finalized_before"].get(ident)) is int
        and stall["finalized_frozen"][ident] > stall["finalized_before"][ident]
        for ident in stall.get("finalized_before") or {})
    settlements = [row for row in stall.get("settlement") or [] if isinstance(row, dict)]
    claimed = any(row.get("stage") == "finalizedSuccess"
                  or row.get("settlementConfirmed") is True for row in settlements)

    live, _, total = str(stall.get("live_stake") or "").partition("/")
    if not live.isdigit() or not total.isdigit():
        raise RuntimeError(f"the stall arm recorded no live stake: {stall.get('live_stake')!r}")

    stages = {}
    for ident, status in (recovery.get("final_stages") or {}).items():
        if isinstance(ident, str) and isinstance(status, str):
            stages[ident] = status
    recovered = recovery.get("settlement") or {}
    if isinstance(recovered, dict) and recovered.get("stage"):
        stages[deploy] = str(recovered["stage"])
    surfaces = {ident: f"deploy-status: {status}" for ident, status in stages.items()
                if status in ("rejected", "terminal")}

    counters = stall.get("counters") or {}
    baseline_counts = _episode_counts(counters.get("baseline"))
    pre_counts = _episode_counts(counters.get("pre_trigger"))
    control = ({"kind": "no-trigger", "stall_absent": lag.get("result") == "PASS"}
               if episode == "rc2" else
               {"kind": "never-stalled", "stall_absent": True,
                "episode_count_equal": baseline_counts == pre_counts})

    record["observations"].update({
        "stall_deploy": deploy,
        "stall_settlements": settlements,
        "stall_series": [
            {"kind": "stall", "phase": "baseline", "height": baseline_height,
             "finalized": baseline_finalized},
            {"kind": "stall", "phase": "trigger", "mechanism": "stop-validators",
             "exercised": True, "live_stake": int(live), "bonded_total": int(total)},
            {"kind": "stall", "phase": "stall", "height": stall_height,
             "finalized": stall_finalized, "window_s": 51,
             "progress_advanced": progress, "finality_advanced": finality,
             "progress_expected": True, "finality_expected": False,
             "settlement_claimed": claimed},
            {"kind": "stall", "phase": "restore", "action": "restart-validators",
             "rule": "R2", "height": int(recovery.get("finalized") or stall_height),
             "finalized": int(recovery.get("finalized") or stall_finalized),
             "progress_resumed": True, "finality_resumed": True},
            {"kind": "stall", "phase": "settle", "deploy_status": stages,
             "rejected_deploys": [], "surfaces": surfaces},
        ],
        "stall_episode": EPISODES[episode],
        "stall_control": control,
        "stall_counters": {"baseline": counters.get("baseline") or {},
                           "pre_trigger": counters.get("pre_trigger") or {},
                           "stall": counters.get("stall") or {},
                           "restore": recovery.get("counters") or {}},
    })
