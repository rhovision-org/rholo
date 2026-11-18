#!/usr/bin/env python3
"""c343-live — the read-only B/C/D snapshot and its verifier.

C343's remaining evidence is a **read-only** snapshot of the live hosts B/C/D: `GET /health`,
`GET /api/v1/status/operator`, and the block-6 journal lines. This tool has two halves:

* `collect` only issues GETs on the two routes, writes the raw answers and a public plan (aliases
  and endpoint digests, never addresses or tokens), and never deploys, proposes, resumes, restarts
  or signs. An alias that never answers is a partial run, not a pass.
* `verify` reads the committed Lean table (`spec/audit/phase-diagnosis.tsv`) and classifies each
  host. The verdict vocabulary is `PASS` / `FAIL` / `PRECONDITION`: a pre-fix host that lacks a
  field is `PRECONDITION`, never `FAIL` or `PASS`; a contradictory observation is `FAIL`.

The tool asserts; it does not author the incident's verdict. `execution_proven` is always false.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
import tempfile
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_TABLE = ROOT / "spec/audit/phase-diagnosis.tsv"
HEALTH_ROUTE = "/health"
OPERATOR_ROUTE = "/api/v1/status/operator"
HEALTH_FIELDS = [
    "ready", "readyReasons", "phase", "nodePhase", "shardPhases", "pipelineFaults",
    "pipelineFaultsOutstanding", "lastPipelineFault", "orphanBodies", "orphanDetections",
    "proposalHalted", "proposalHaltReason", "finalizedLag", "blockPipeline",
]
OPERATOR_FIELDS = [
    "pipelineFaults", "pipelineFaultsOutstanding", "lastPipelineFault", "orphanBodies",
    "orphanDetections", "proposalHalted", "proposalHaltReason", "syncPhase", "sync",
    "resetMarker", "finalityStalled", "shards",
]
CONDITIONS = {
    "fault-outstanding", "phase-blocked", "proposal-halted", "phase-syncing",
    "sync-phase-syncing", "sync-active", "reset-marker", "any-syncing", "ready",
    "ready-reasons-empty", "all-shards-running", "finality-stalled", "no-fault-outstanding",
    "no-proposal-halted", "no-syncing", "not-finality-stalled",
}
MUTATION_VERBS = ("POST ", "PUT ", "PATCH ", "DELETE ", "/deploy", "/propose", "/resume")


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_text(text: str) -> str:
    return sha256_bytes(text.encode())


def health_get(url: str, timeout: float, token: str | None) -> dict[str, Any]:
    request = urllib.request.Request(url, method="GET")
    if token:
        request.add_header("Authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            body = response.read()
            return {"status": response.status, "body": body.decode("utf-8", "replace"),
                    "body_sha256": sha256_bytes(body)}
    except urllib.error.HTTPError as error:
        body = error.read()
        return {"status": error.code, "body": body.decode("utf-8", "replace"),
                "body_sha256": sha256_bytes(body)}
    except Exception as error:  # noqa: BLE001 - a collector records every failure
        return {"status": None, "body": "", "body_sha256": None, "error": str(error)}


def collect(args: argparse.Namespace) -> int:
    plan = json.loads(Path(args.plan).read_text())
    hosts = plan.get("hosts", [])
    if not hosts:
        print("c343-live: the plan has no hosts", file=sys.stderr)
        return 2
    out = Path(args.out)
    raw = out / "raw"
    raw.mkdir(parents=True, exist_ok=True)
    run: dict[str, Any] = {
        "schema": 1,
        "campaign": plan.get("campaign", "c343-live"),
        "read_only": True,
        "execution_proven": False,
        "source_sha": args.source_sha or plan.get("source_sha"),
        "binary_sha256": args.binary_sha256 or plan.get("binary_sha256"),
        "answers": {},
        "journals": {},
    }
    public_hosts = []
    events = []
    all_ok = True
    for host in hosts:
        alias = host["alias"]
        base = host["base"].rstrip("/")
        operator_base = host.get("operator_base", base).rstrip("/")
        token = os.environ.get(host["auth_env"]) if host.get("auth_env") else None
        public_hosts.append({
            "alias": alias,
            "base_sha256": sha256_text(base),
            "operator_base_sha256": sha256_text(operator_base),
            "source_sha": host.get("source_sha"),
            "binary_sha256": host.get("binary_sha256"),
        })
        answers = {}
        for route, url, name in (
            (HEALTH_ROUTE, base + HEALTH_ROUTE, "health"),
            (OPERATOR_ROUTE, operator_base + OPERATOR_ROUTE, "operator"),
        ):
            answer = health_get(url, args.timeout, token)
            answers[name] = {"status": answer["status"], "body_sha256": answer["body_sha256"],
                             "error": answer.get("error")}
            events.append({"alias": alias, "route": route, "method": "GET",
                           "status": answer["status"],
                           "body_sha256": answer["body_sha256"],
                           "error": answer.get("error")})
            target = raw / f"{alias}-{name}.json"
            if answer["status"] == 200:
                try:
                    parsed = json.loads(answer["body"])
                    target.write_text(json.dumps(parsed, indent=1, sort_keys=True) + "\n")
                except json.JSONDecodeError:
                    target.write_text(answer["body"])
            else:
                target.write_text(json.dumps(
                    {"_status": answer["status"], "_error": answer.get("error"),
                     "_body_sha256": answer["body_sha256"]}, indent=1) + "\n")
            if answer["status"] != 200:
                all_ok = False
        run["answers"][alias] = answers
        journal = (plan.get("journals") or {}).get(alias) or (
            str(Path(args.journal_dir) / f"{alias}.log") if args.journal_dir else None)
        if journal and Path(journal).is_file():
            data = Path(journal).read_bytes()
            (raw / f"{alias}-journal.log").write_bytes(data)
            run["journals"][alias] = {"path": f"raw/{alias}-journal.log",
                                      "sha256": sha256_bytes(data)}
        elif journal:
            run["journals"][alias] = {"path": journal, "error": "not found"}
    (raw / "run.json").write_text(json.dumps(run, indent=1, sort_keys=True) + "\n")
    (raw / "plan.public.json").write_text(json.dumps(
        {"schema": 1, "campaign": run["campaign"], "hosts": public_hosts}, indent=1,
        sort_keys=True) + "\n")
    with (raw / "snapshots.jsonl").open("w") as handle:
        for event in events:
            handle.write(json.dumps(event, sort_keys=True) + "\n")
    sums = {}
    for path in sorted(raw.rglob("*")):
        if path.is_file() and path.name != "SHA256SUMS":
            sums[str(path.relative_to(raw))] = sha256_bytes(path.read_bytes())
    (raw / "SHA256SUMS").write_text("".join(
        f"{digest}  {name}\n" for name, digest in sums.items()))
    print(f"c343-live: collected {len(hosts)} host(s) into {out}; "
          f"{'all answered' if all_ok else 'a host did not answer both routes'}")
    return 0 if all_ok else 3


def load_table(path: Path) -> list[dict[str, Any]]:
    rows = []
    lines = path.read_text().splitlines()
    if not lines or not lines[0].startswith("priority\t"):
        raise SystemExit(f"c343-live: {path} is not the phase-diagnosis table")
    for line in lines[1:]:
        if not line.strip():
            continue
        priority, diagnosis, conditions = line.split("\t")
        conds = [c for c in conditions.split(",") if c]
        unknown = set(conds) - CONDITIONS
        if unknown:
            raise SystemExit(f"c343-live: unknown condition(s) {sorted(unknown)}")
        rows.append({"priority": int(priority), "diagnosis": diagnosis, "conditions": conds})
    priorities = [row["priority"] for row in rows]
    if priorities != sorted(priorities) or len(set(priorities)) != len(priorities):
        raise SystemExit("c343-live: the table priorities are not sorted and unique")
    return rows


def phase_label(value: Any) -> str:
    if isinstance(value, dict) and "blocked" in value:
        return "blocked"
    return str(value) if value is not None else "starting"


def observation(health: dict[str, Any], operator: dict[str, Any]) -> dict[str, Any]:
    phase = phase_label(health.get("phase", health.get("nodePhase", "starting")))
    sync_phase = operator.get("syncPhase")
    reasons = health.get("readyReasons") or []
    shard_phases = health.get("shardPhases") or {}
    if isinstance(shard_phases, dict):
        shard_phases = list(shard_phases.values())
    height = health.get("maxObservedHeight")
    finalized = health.get("finalizedHeight")
    if height is None or finalized is None:
        gap = health.get("finalizedLag", 0)
    else:
        gap = max(0, int(height) - int(finalized))
    return {
        "healthPhase": phase,
        "syncPhase": sync_phase,
        "syncActive": operator.get("sync") is not None,
        "resetMarker": operator.get("resetMarker") is not None,
        "pipelineFaultsOutstanding": int(health.get("pipelineFaultsOutstanding",
                                                   operator.get("pipelineFaultsOutstanding", 0)) or 0),
        "proposalHalted": bool(health.get("proposalHalted",
                                          operator.get("proposalHalted", False))),
        "finalityStalled": bool(health.get("finalityStalled",
                                           operator.get("finalityStalled", False))),
        "heightGap": gap,
        "shardPhases": [phase_label(p) for p in shard_phases],
        "orphanBodies": int(health.get("orphanBodies", 0) or 0),
        "ready": bool(health.get("ready", False)),
        "readyReasons": reasons,
        "finalizedLag": health.get("finalizedLag"),
        "pipelineFaults": health.get("pipelineFaults", operator.get("pipelineFaults")),
        "lastPipelineFault": health.get("lastPipelineFault", operator.get("lastPipelineFault")),
        "orphanDetections": health.get("orphanDetections", operator.get("orphanDetections")),
        "proposalHaltReason": health.get("proposalHaltReason", operator.get("proposalHaltReason")),
    }


def holds(condition: str, o: dict[str, Any]) -> bool:
    if condition == "fault-outstanding":
        return o["pipelineFaultsOutstanding"] > 0
    if condition == "phase-blocked":
        return o["healthPhase"] == "blocked"
    if condition == "proposal-halted":
        return o["proposalHalted"]
    if condition == "phase-syncing":
        return o["healthPhase"] == "syncing"
    if condition == "sync-phase-syncing":
        return o["syncPhase"] == "syncing"
    if condition == "sync-active":
        return o["syncActive"]
    if condition == "reset-marker":
        return o["resetMarker"]
    if condition == "any-syncing":
        return (o["healthPhase"] == "syncing" or o["syncPhase"] == "syncing"
                or o["syncActive"] or o["resetMarker"])
    if condition == "ready":
        return o["ready"]
    if condition == "ready-reasons-empty":
        return o["readyReasons"] == []
    if condition == "all-shards-running":
        return bool(o["shardPhases"]) and all(p == "running" for p in o["shardPhases"])
    if condition == "finality-stalled":
        return o["finalityStalled"]
    if condition == "no-fault-outstanding":
        return o["pipelineFaultsOutstanding"] == 0
    if condition == "no-proposal-halted":
        return not o["proposalHalted"]
    if condition == "no-syncing":
        return not holds("any-syncing", o)
    if condition == "not-finality-stalled":
        return not o["finalityStalled"]
    raise SystemExit(f"c343-live: unknown condition {condition}")


def diagnose(table: list[dict[str, Any]], o: dict[str, Any]) -> str:
    for row in table:
        if all(holds(c, o) for c in row["conditions"]):
            return row["diagnosis"]
    return "unmodelled"


def consistent(o: dict[str, Any]) -> bool:
    if o["ready"] != (o["readyReasons"] == []):
        return False
    if (o["healthPhase"] == "blocked") != (o["pipelineFaultsOutstanding"] > 0):
        return False
    if o["syncPhase"] is not None and o["healthPhase"] != "blocked" and not (
            o["healthPhase"] == o["syncPhase"] or o["syncPhase"] in ("syncing", "starting")):
        return False
    return True


def cell(verdict: str, detail: str) -> dict[str, str]:
    return {"verdict": verdict, "detail": detail}


BLOCK6 = re.compile(
    r"(?<!\d)(?:block[- ]?#?|height[- ]?#?|#)\s*0*6(?!\d)", re.I)
BLOCK6_DISPOSITIONS = {
    "orphan": ("FAIL", "block 6 is an orphan"),
    "held": ("FAIL", "block 6 is held"),
    "faulted": ("FAIL", "block 6 is faulted"),
    "pending": ("PRECONDITION", "block 6 is pending"),
    "unknown": ("PRECONDITION", "block 6 is unknown"),
    "accepted": ("PASS", "block 6 is accepted"),
    "included": ("PASS", "block 6 is included"),
    "finalized": ("PASS", "block 6 is finalized"),
    "proposed": ("PASS", "block 6 is proposed"),
    "added": ("PASS", "block 6 is added"),
}


def block6_disposition(text: str) -> tuple[str, str]:
    """Read a structured block-6 disposition, not a substring.

    A line that names block 6 without a disposition (``block 6 unknown``) is a
    precondition, never a pass. A JSON record with a block/height and a
    disposition/status is read first; free text is matched next.
    """
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("{"):
            try:
                record = json.loads(stripped)
            except ValueError:
                record = None
            if isinstance(record, dict) and (record.get("block") == 6
                                             or record.get("height") == 6):
                token = str(record.get("disposition") or record.get("status") or "").lower()
                for name, (verdict, detail) in BLOCK6_DISPOSITIONS.items():
                    if name in token:
                        return verdict, detail
                return "PRECONDITION", "block 6 is named without a disposition"
        if not BLOCK6.search(line):
            continue
        low = line.lower()
        for name, (verdict, detail) in BLOCK6_DISPOSITIONS.items():
            if re.search(rf"\b{name}\b", low):
                return verdict, detail
        return "PRECONDITION", "block 6 is named without a disposition"
    return "PRECONDITION", "no block-6 line"


def verify_run(args: argparse.Namespace) -> tuple[int, dict[str, Any]]:
    run_dir = Path(args.run_dir)
    raw = run_dir / "raw"
    run = json.loads((raw / "run.json").read_text())
    table = load_table(Path(args.table))
    required = [h.strip() for h in (args.require_hosts or "").split(",") if h.strip()]
    aliases = required or sorted(run.get("answers", {}))
    report: dict[str, Any] = {
        "schema": 1, "gate_id": "C343-LIVE", "purpose": args.purpose,
        "pins": {"source_sha": run.get("source_sha"),
                 "binary_sha256": run.get("binary_sha256")},
        "hosts": {}, "contradictions": [], "execution_proven": False,
    }
    any_fail = False
    any_pre = False
    if run.get("read_only") is not True:
        report["contradictions"].append("run.json does not assert read_only")
        any_fail = True
    for name in ("run.json", "plan.public.json", "snapshots.jsonl"):
        path = raw / name
        if path.is_file():
            text = path.read_text(errors="replace")
            for verb in MUTATION_VERBS:
                if verb in text:
                    report["contradictions"].append(f"{name} contains {verb!r}")
                    any_fail = True
    for alias in aliases:
        answers = (run.get("answers") or {}).get(alias)
        if not answers:
            report["hosts"][alias] = {"cells": {"health-route": cell(
                "PRECONDITION", "the alias did not answer; collect exited 3"),
                "operator-route": cell("PRECONDITION", "the alias did not answer")},
                "diagnosis": "unmodelled"}
            any_pre = True
            continue
        health = {}
        operator = {}
        if answers.get("health", {}).get("status") == 200:
            try:
                health = json.loads((raw / f"{alias}-health.json").read_text())
            except Exception:  # noqa: BLE001
                health = {}
        if answers.get("operator", {}).get("status") == 200:
            try:
                operator = json.loads((raw / f"{alias}-operator.json").read_text())
            except Exception:  # noqa: BLE001
                operator = {}
        missing = [f for f in HEALTH_FIELDS if f not in health]
        op_missing = [f for f in OPERATOR_FIELDS if f not in operator]
        cells: dict[str, dict[str, str]] = {}
        cells["health-route"] = (cell("PASS", "the 14-field contract is present")
                                 if not missing else
                                 cell("PRECONDITION", f"missing {missing}"))
        cells["operator-route"] = (cell("PASS", "the gates and C343 fields are present")
                                   if not op_missing else
                                   cell("PRECONDITION", f"missing {op_missing}"))
        o = observation(health, operator)
        is_consistent = consistent(o)
        diagnosis = diagnose(table, o) if not missing and not op_missing else "unmodelled"
        if missing or op_missing:
            cells["phase-discriminator"] = cell(
                "PRECONDITION", "the observation cannot be completed")
        elif not is_consistent:
            cells["phase-discriminator"] = cell("FAIL", "the observation is inconsistent")
        elif diagnosis == "unmodelled":
            cells["phase-discriminator"] = cell(
                "PRECONDITION", "the observation matches no named shape")
        else:
            cells["phase-discriminator"] = cell("PASS", f"diagnosis {diagnosis}")
        journal = (run.get("journals") or {}).get(alias)
        if not journal or "error" in journal:
            cells["block6-disposition"] = cell("PRECONDITION", "no journal line")
        else:
            text = (raw / Path(journal["path"]).name).read_text(errors="replace")
            verdict, detail = block6_disposition(text)
            cells["block6-disposition"] = cell(verdict, detail)
        if "maxObservedHeight" in health and "finalizedHeight" in health:
            expected = max(0, int(health["maxObservedHeight"]) - int(health["finalizedHeight"]))
            if expected != int(health.get("finalizedLag", -1)):
                cells["height-gap"] = cell(
                    "FAIL", f"maxObservedHeight-finalizedHeight={expected} "
                            f"but finalizedLag={health.get('finalizedLag')}")
            else:
                cells["height-gap"] = cell("PASS", f"lag {expected}")
        else:
            cells["height-gap"] = cell("PRECONDITION", "maxObservedHeight/finalizedHeight absent")
        if o["ready"] != (o["readyReasons"] == []):
            cells["readiness-conjunction"] = cell(
                "FAIL", "ready/reasons disagree")
        else:
            cells["readiness-conjunction"] = cell("PASS", "ready agrees with its reasons")
        if o["pipelineFaultsOutstanding"] > 0:
            ok = (o["healthPhase"] == "blocked" and not o["ready"]
                  and "pipeline_fault_outstanding" in o["readyReasons"])
            cells["fault-latch"] = cell("PASS" if ok else "FAIL",
                                        "the fault reports blocked" if ok else
                                        "an outstanding fault is not fail-closed")
        else:
            cells["fault-latch"] = cell("PASS", "no outstanding fault")
        if o["proposalHalted"]:
            reason = o.get("proposalHaltReason") or ""
            ok = (not o["ready"] and "proposal_isolated" in o["readyReasons"]
                  and ("bound" in reason or "height" in reason))
            cells["isolation"] = cell("PASS" if ok else "FAIL",
                                      "the isolation names its bound" if ok else
                                      "the isolation is not named on both surfaces")
        else:
            cells["isolation"] = cell("PASS", "not isolated")
        if o["resetMarker"]:
            cells["reset-marker"] = cell(
                "PASS" if diagnosis == "syncing-recovery" else "FAIL",
                "the marker reads as recovery" if diagnosis == "syncing-recovery" else
                "a reset marker is not syncing recovery")
        else:
            cells["reset-marker"] = cell("PASS", "no reset marker")
        if args.purpose == "current-build":
            pins_ok = (args.expected_source_sha in (None, run.get("source_sha"))
                       and args.expected_binary_sha256 in (None, run.get("binary_sha256")))
            cells["current-build"] = cell(
                "PASS" if pins_ok else "PRECONDITION",
                "the selected pins match" if pins_ok else "the build is not the selected one")
        else:
            cells["current-build"] = cell("PRECONDITION",
                                          "the incident purpose does not gate the current build")
        if args.purpose == "current-build":
            prior = [v["verdict"] for k, v in cells.items() if k != "closeout"]
            close = "PASS" if all(v == "PASS" for v in prior) else (
                "FAIL" if "FAIL" in prior else "PRECONDITION")
        else:
            close = "PASS" if diagnosis != "unmodelled" else "PRECONDITION"
        cells["closeout"] = cell(close, f"purpose {args.purpose}, diagnosis {diagnosis}")
        report["hosts"][alias] = {"cells": cells, "diagnosis": diagnosis,
                                  "consistent": is_consistent, "observation": o}
        verdicts = [v["verdict"] for v in cells.values() if v["verdict"] != "PASS"]
        if "FAIL" in verdicts:
            any_fail = True
        elif "PRECONDITION" in verdicts:
            any_pre = True
        if not is_consistent:
            report["contradictions"].append(f"{alias}: the observation is inconsistent")
    result = "FAIL" if any_fail else ("PRECONDITION" if any_pre else "PASS")
    report["result"] = result
    if args.report:
        Path(args.report).write_text(json.dumps(report, indent=1, sort_keys=True) + "\n")
    if args.assert_closeable and result != "PASS":
        return 3, report
    return (1 if result == "FAIL" else (3 if result == "PRECONDITION" else 0)), report


def _fixture(alias: str, health: dict[str, Any], operator: dict[str, Any],
             journal: str | None = None) -> dict[str, Any]:
    return {"alias": alias, "health": health, "operator": operator, "journal": journal}


def _base_health(**overrides: Any) -> dict[str, Any]:
    health = {
        "ready": True, "readyReasons": [], "phase": "running", "nodePhase": "running",
        "shardPhases": {"/root": "running"}, "pipelineFaults": 0,
        "pipelineFaultsOutstanding": 0, "lastPipelineFault": None, "orphanBodies": 0,
        "orphanDetections": 0, "proposalHalted": False, "proposalHaltReason": None,
        "finalizedLag": 1, "maxObservedHeight": 5, "finalizedHeight": 4,
        "finalityStalled": False, "blockPipeline": {"capacity": 1, "pending": 0,
                                                    "sampledPeak": 0, "validatedQueue": 0,
                                                    "incomingQueue": 0},
    }
    health.update(overrides)
    return health


def _base_operator(**overrides: Any) -> dict[str, Any]:
    operator = {
        "pipelineFaults": 0, "pipelineFaultsOutstanding": 0, "lastPipelineFault": None,
        "orphanBodies": 0, "orphanDetections": 0, "proposalHalted": False,
        "proposalHaltReason": None, "syncPhase": "running", "sync": None,
        "resetMarker": None, "finalityStalled": False, "shards": {},
    }
    operator.update(overrides)
    return operator


def self_test() -> int:
    table = load_table(DEFAULT_TABLE)
    cases = [
        ("healthy", _fixture("A", _base_health(), _base_operator()), "healthy", "PASS"),
        ("processor-fault", _fixture(
            "A", _base_health(ready=False, readyReasons=["pipeline_fault_outstanding"],
                              phase="blocked", pipelineFaultsOutstanding=2),
            _base_operator(pipelineFaultsOutstanding=2)), "processor-fault-blocked", "PASS"),
        ("isolation", _fixture(
            "A", _base_health(ready=False, readyReasons=["proposal_isolated"],
                              proposalHalted=True, proposalHaltReason="tip 36 is 33 heights beyond last finalized 3 (bound 32)"),
            _base_operator(proposalHalted=True)), "proposal-isolation", "PASS"),
        ("syncing", _fixture(
            "A", _base_health(ready=False, readyReasons=["shard_not_running"], phase="syncing"),
            _base_operator(syncPhase="syncing", sync={"attempts": 1})), "syncing-recovery", "PASS"),
        ("unmodelled", _fixture(
            "A", _base_health(ready=False, readyReasons=["unknown"], finalityStalled=True),
            _base_operator(finalityStalled=True)), "unmodelled", "PRECONDITION"),
    ]
    failures = []
    for name, fixture, expected, expected_result in cases:
        o = observation(fixture["health"], fixture["operator"])
        actual = diagnose(table, o)
        if actual != expected:
            failures.append(f"{name}: diagnosis {actual}, expected {expected}")
        if expected_result == "PRECONDITION" and actual != "unmodelled":
            failures.append(f"{name}: expected the unmodelled precondition")
    # The mutant: remove the processor row and the processor case must stop being named.
    mutant = [row for row in table if row["diagnosis"] != "processor-fault-blocked"]
    o = observation(
        _base_health(ready=False, readyReasons=["pipeline_fault_outstanding"], phase="blocked",
                     pipelineFaultsOutstanding=2), _base_operator(pipelineFaultsOutstanding=2))
    if diagnose(mutant, o) == "processor-fault-blocked":
        failures.append("mutant table still names the processor fault")
    # A contradictory ready/reasons pair must fail the consistency check.
    bad = observation(_base_health(ready=True, readyReasons=["x"]), _base_operator())
    if consistent(bad):
        failures.append("a ready/reasons contradiction passed the consistency check")
    block6_cases = [
        ("block 6 accepted", "PASS"),
        ("block 6 unknown", "PRECONDITION"),
        ("block 6 orphan", "FAIL"),
        ("block 16 accepted", "PRECONDITION"),
        ("proposed and added block #6", "PASS"),
        ('{"block": 6, "disposition": "unknown"}', "PRECONDITION"),
        ('{"block": 6, "disposition": "finalized"}', "PASS"),
    ]
    for text, expected in block6_cases:
        actual, _ = block6_disposition(text)
        if actual != expected:
            failures.append(f"block-6 {text!r}: {actual}, expected {expected}")
    if failures:
        for failure in failures:
            print(f"c343-live self-test: FAIL {failure}", file=sys.stderr)
        return 1
    print("c343-live self-test: ok")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    collect_p = sub.add_parser("collect")
    collect_p.add_argument("--plan", required=True)
    collect_p.add_argument("--out", required=True)
    collect_p.add_argument("--journal-dir")
    collect_p.add_argument("--source-sha")
    collect_p.add_argument("--binary-sha256")
    collect_p.add_argument("--timeout", type=float, default=10.0)
    verify_p = sub.add_parser("verify")
    verify_p.add_argument("--run-dir", required=True)
    verify_p.add_argument("--report")
    verify_p.add_argument("--table", default=str(DEFAULT_TABLE))
    verify_p.add_argument("--purpose", choices=("incident", "current-build"), required=True)
    verify_p.add_argument("--expected-source-sha")
    verify_p.add_argument("--expected-binary-sha256")
    verify_p.add_argument("--require-hosts")
    verify_p.add_argument("--assert-closeable", action="store_true")
    sub.add_parser("self-test")
    args = parser.parse_args()
    if args.command == "self-test":
        return self_test()
    if args.command == "collect":
        return collect(args)
    if args.command == "verify":
        code, report = verify_run(args)
        print(f"c343-live: {report['result']} "
              f"({', '.join(sorted(report['hosts'])) or 'no hosts'})")
        return code
    return 2


if __name__ == "__main__":
    sys.exit(main())
