"""One stall series and its predicate.

A stall arm's record carries an ordered observation:

    baseline -> trigger -> stall -> restore -> settle

Each row is also a `stall` decision trace: the predicates are recomputed from the coordinates by
`release_claim_common.evaluate_stall_trace`, never read from a stored label. The episode (the
representation / shape / cause / rung reading) is a separate object and is checked against the
emitted `spec/progress-vocabulary.tsv`.

Two rules make this a measurement rather than a narrative:

* A trigger that was not exercised is **`not run`**, never `pass`. A campaign that did not
  reproduce the deviation must not close its row.
* The stored verdict is not trusted. `verdict` re-derives the reading from the series.
"""
from __future__ import annotations

import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location(
    "release_claim_common", ROOT / "tests" / "release_claim_common.py")
common = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(common)

PHASES = ("baseline", "trigger", "stall", "restore", "settle")


def strict_supermajority(live: int, total: int) -> bool:
    """The denominator rule, in one place: exactly two thirds is not a supermajority."""
    return 3 * live > 2 * total


def fixture(deploy_id: str, *, episode: dict, expected: dict, control: dict,
            rule: str = "R2", mechanism: str = "stop-validators",
            live_stake: int = 200, bonded_total: int = 300,
            status: str = "finalizedSuccess", surfaces: dict | None = None) -> dict:
    """The observations half of a complete passing stall record.

    The live harness fills the same shape from its measured coordinates; the claim tests use this
    for the offline fixture and one decisive mutation.
    """
    return {
        "stall_series": [
            {"kind": "stall", "phase": "baseline", "height": 40, "finalized": 38},
            {"kind": "stall", "phase": "trigger", "mechanism": mechanism, "exercised": True,
             "live_stake": live_stake, "bonded_total": bonded_total},
            {"kind": "stall", "phase": "stall", "height": 44, "finalized": 38, "window_s": 60,
             "progress_advanced": expected["progress"],
             "finality_advanced": expected["finality"],
             "progress_expected": expected["progress"],
             "finality_expected": expected["finality"],
             "settlement_claimed": False},
            {"kind": "stall", "phase": "restore", "action": "restore", "rule": rule,
             "height": 46, "finalized": 44, "progress_resumed": True,
             "finality_resumed": True},
            {"kind": "stall", "phase": "settle", "deploy_status": {deploy_id: status},
             "rejected_deploys": [], "surfaces": surfaces or {}},
        ],
        "stall_episode": episode,
        "stall_control": control,
    }


def _reasons_for(row: dict) -> list[str]:
    evaluation = common.evaluate_stall_trace(row)
    reasons = [f"{name} is false at phase {row.get('phase')!r}"
               for name in evaluation["false"]]
    reasons += [f"{name} has no coordinate at phase {row.get('phase')!r}"
                for name in evaluation["missing"]]
    return reasons


def verdict(record: dict, expect: dict) -> tuple[str, list[str]]:
    """Return ("pass" | "fail" | "not run", reasons) for one record and one arm contract."""
    if record.get("rehearsal") is True:
        return "not run", ["a rehearsal record is not evidence; run the arm on the devnet"]
    if record.get("verdict") == "not run":
        return "not run", list(record.get("preconditions") or record.get("reasons") or [])
    observations = record.get("observations")
    if not isinstance(observations, dict):
        return "fail", ["observations: missing or not an object"]
    rows = observations.get("stall_series")
    if not isinstance(rows, list) or not rows:
        return "fail", ["stall_series: missing or empty"]

    problems: list[str] = []
    by_phase: dict[str, dict] = {}
    order: list[str] = []
    for row in rows:
        if not isinstance(row, dict):
            problems.append("stall_series: a row is not an object")
            continue
        phase = row.get("phase")
        if phase not in PHASES:
            problems.append(f"stall_series: unknown phase {phase!r}")
            continue
        if phase in by_phase:
            problems.append(f"stall_series: phase {phase!r} appears twice")
        by_phase[phase] = row
        order.append(phase)
    if order != list(PHASES):
        problems.append(f"stall_series: phases are {order!r}, want {list(PHASES)!r}")
    missing = [phase for phase in PHASES if phase not in by_phase]
    if missing:
        problems.append(f"stall_series: missing phase(s): {', '.join(missing)}")

    trigger = by_phase.get("trigger")
    if trigger is not None and (
            trigger.get("exercised") is False
            or observations.get("trigger_exercised") is False):
        return "not run", ["the campaign did not exercise the trigger: "
                           + str(trigger.get("mechanism") or "no mechanism named")]

    for row in rows:
        if isinstance(row, dict):
            problems.extend(_reasons_for(row))

    episode = observations.get("stall_episode")
    if not isinstance(episode, dict):
        problems.append("stall_episode: missing or not an object")
    else:
        problems.extend(_reasons_for({"kind": "stall", "phase": "episode", **episode}))
        for key, axis in (("representation", "representation"), ("shape", "shape"),
                          ("cause", "root-cause"), ("rung", "rung")):
            if episode.get(key) != expect.get(key):
                problems.append(
                    f"stall_episode: {key} {episode.get(key)!r} is not the expected "
                    f"{expect.get(key)!r}")
        observed = set(episode.get("deviations") or [])
        wanted = set(expect.get("deviations") or [])
        if observed != wanted:
            problems.append(
                f"stall_episode: deviations {sorted(observed)} are not the expected "
                f"{sorted(wanted)}")

    expected_control = expect.get("control")
    if expected_control:
        control = observations.get("stall_control")
        if not isinstance(control, dict) or control.get("kind") != expected_control:
            problems.append(
                "stall_control: missing or not the expected "
                f"{expected_control!r}: {control!r}")
        elif expected_control == "halt":
            if control.get("operator_surface") is not True:
                problems.append("stall_control: the halt control has no operator surface")
        elif expected_control == "never-stalled":
            if control.get("stall_absent") is not True \
                    or control.get("episode_count_equal") is not True:
                problems.append("stall_control: the never-stalled control is not equal "
                                "at baseline")
        elif control.get("stall_absent") is not True:
            problems.append("stall_control: the no-trigger control did not stay live")

    settle = by_phase.get("settle")
    if settle is not None:
        statuses = settle.get("deploy_status")
        statuses = statuses if isinstance(statuses, dict) else {}
        for ident in expect.get("deploys") or []:
            if ident not in statuses:
                problems.append(f"settle: deploy {ident} has no disposition")

    return ("pass", []) if not problems else ("fail", problems)
