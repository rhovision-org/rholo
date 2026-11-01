"""The structured failure envelope shared by the acceptance harnesses.

A failed arm used to leave one string — `{"verdict":"FAIL","error":str(error)}` — so triaging it meant
opening the node logs and guessing which of several causes it was. This writes a `failure.json` that
names the class, the arm and phase, the first error, the health rows at the failure, the last events
and what is missing. The class vocabulary is **closed**; when the evidence does not decide, the class
is `UNDETERMINED` and `missing` says which datum would have decided it. A guess is worse than the
string it replaced.

It is output-only: it never changes a verdict, never retries, never touches the node.
"""

from __future__ import annotations

import json
from pathlib import Path

SCHEMA = 1

CLASSES = (
    "receiver_capacity",
    "pipeline_fault",
    "orphan_body",
    "not_ready",
    "deploy_unresolved",
    "agreement",
    "timeout",
    "harness_error",
    "UNDETERMINED",
)

# A named verdict is the strongest evidence there is; map it before guessing from text.
VERDICT_CLASSES = (
    ("FAIL_PRECONDITION_pipelineFaultsOutstanding", "pipeline_fault"),
    ("FAIL_PRECONDITION_proposalHalted", "not_ready"),
    ("FAIL_PRECONDITION_", "not_ready"),
    ("FAIL_PERMANENT_DIVERGENCE", "orphan_body"),
    ("FAIL_FORK", "agreement"),
    ("FAIL_FINALIZED_DISAGREEMENT", "agreement"),
    ("FAIL_STALLED", "timeout"),
    ("INCONCLUSIVE_", "timeout"),
)


def classify(error_text: str, health_rows=None, phase: str = "") -> tuple[str, list[str]]:
    """(class, missing). `missing` is non-empty only for UNDETERMINED, and names the datum."""
    text = f"{error_text} {phase}"
    lowered = text.lower() if isinstance(text, str) else ""
    for marker, name in VERDICT_CLASSES:
        if marker in text:
            return name, []
    if "block receiver state full" in lowered or "receiver_capacity" in lowered:
        return "receiver_capacity", []
    if "missing justification" in lowered or "missing parent" in lowered:
        return "deploy_unresolved", []
    if "orphan" in lowered:
        return "orphan_body", []
    if "not ready" in lowered or "readyreasons" in lowered:
        return "not_ready", []
    for row in health_rows or []:
        if not isinstance(row, dict):
            continue
        outstanding = row.get("pipeline_faults_outstanding")
        if type(outstanding) is int and outstanding > 0:
            return "pipeline_fault", []
        orphans = row.get("orphan_bodies")
        if type(orphans) is int and orphans > 0:
            return "orphan_body", []
        if row.get("ready") is False:
            return "not_ready", []
    if "timeout" in lowered or "did not" in lowered or "window" in lowered:
        return "timeout", []
    return "UNDETERMINED", ["no failure.json, result reason or health row names a cause"]


def envelope(
    verdict: str,
    error: str,
    *,
    phase: str = "",
    arm: str | None = None,
    node=None,
    health=None,
    events=None,
    missing=None,
    artifact_refs=None,
    capture_error=None,
) -> dict:
    health_rows = health or []
    failure_class, class_missing = classify(str(error), health_rows, phase)
    body = {
        "schema": SCHEMA,
        "verdict": "FAIL" if str(verdict).upper().startswith("FAIL") else verdict,
        "failure": {
            "class": failure_class,
            "arm": arm,
            "phase": phase,
            "node": node,
            "first_error": str(error),
            "health": health_rows,
            "last_events": events or [],
            "missing": list(missing or []) + class_missing,
            "artifact_refs": artifact_refs or [],
        },
    }
    if capture_error is not None:
        # A capture failure is recorded beside the primary error, never instead of it.
        body["failure"]["capture_error"] = str(capture_error)
    return body


def write(directory, verdict: str, error: str, **kwargs) -> Path:
    """Write `failure.json` under `directory`; returns the path. Output-only."""
    out = Path(directory)
    out.mkdir(parents=True, exist_ok=True)
    path = out / "failure.json"
    path.write_text(json.dumps(envelope(verdict, error, **kwargs), indent=2) + "\n")
    return path


def best_effort_health(reader, validators) -> tuple[list, str | None]:
    """Collect health rows for the envelope; a failed read is `capture_error`, never a verdict."""
    rows = []
    for index in validators:
        try:
            rows.append(reader(index))
        except Exception as error:  # noqa: BLE001 - a capture error is data, not a verdict
            return rows, f"{type(error).__name__}: {error}"
    return rows, None
