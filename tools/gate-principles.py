#!/usr/bin/env python3
"""Principle-level gate criteria.

The canonical register (`spec/audit/mainnet-gates.json`) states each gate as prose plus an
owner. The checkers that support the external gates each know one instance: the four #584
validators, the 270-600 s M4 window, the block-6 shape, the C357 cap. This module names the
**principle** instead, and `spec/audit/gate-criteria.json` supplies the instance parameters.

`evaluate(principle, observations, parameters)` returns one of:

    PASS          the observation document satisfies the principle
    FAIL          the observation contradicts the principle
    PRECONDITION  a required observation is absent or insufficient

The deep semantics stay in the existing checkers (`live-gate.py`, `check-gate-criterion.py`,
`c343-live.py`, `c357-acceptance.py`, `check-evidence-locality.py`,
`check-bounded-ingress-queues.sh`); the functions here adapt one normalised observation
document to those checkers. No predicate receives a gate id, person, host or endpoint.

A PASS is a record measurement, not a `met` promotion. The register still requires its
witness/control/evidence/pin binding, and the register checker remains the authority on status.
"""
from __future__ import annotations

import datetime as dt
import hashlib
import importlib.util
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CRITERIA = ROOT / "spec" / "audit" / "gate-criteria.json"
HEX40 = re.compile(r"^[0-9a-f]{40}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
STATUSES = ("PASS", "FAIL", "PRECONDITION")
ATTESTATION_VERDICTS = ("PASS", "FAIL", "INCONCLUSIVE", "category-A-accepted")


def verdict(status: str, reasons: list[str] | None = None, details: dict | None = None) -> dict:
    if status not in STATUSES:
        raise ValueError(f"unknown verdict {status!r}")
    return {"status": status, "reasons": list(reasons or []), "details": dict(details or {})}


def _module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


_live = _module("gate_principles_live_gate", ROOT / "tools" / "live-gate.py")
_criterion = _module("gate_principles_criterion", ROOT / "tools" / "check-gate-criterion.py")
_c343 = _module("gate_principles_c343", ROOT / "tools" / "c343-live.py")
_c357 = _module("gate_principles_c357", ROOT / "tools" / "c357-acceptance.py")
_locality = _module("gate_principles_locality", ROOT / "tools" / "check-evidence-locality.py")
_m3chain = _module("gate_principles_m3_chain", ROOT / "tools" / "m3-join-chain.py")
_m2pressure = _module("gate_principles_m2_pressure", ROOT / "tools" / "m2-pressure.py")


def _present(value: object) -> bool:
    if value is None:
        return False
    if isinstance(value, (str, list, dict)) and not value:
        return False
    return True


def _get(document: object, path: str):
    """Read a JSON-Pointer-style path (`/` separators, `~1` escapes a literal `/`)."""
    current = document
    for part in path.split("/"):
        part = part.replace("~1", "/").replace("~0", "~")
        if isinstance(current, list):
            try:
                current = current[int(part)]
            except (ValueError, IndexError):
                return None
        elif isinstance(current, dict) and part in current:
            current = current[part]
        else:
            return None
    return current


def _events(observations: dict) -> list[dict]:
    events = observations.get("events")
    if not isinstance(events, list):
        return []
    return [event for event in events if isinstance(event, dict)]


def _report_status(report: dict) -> str:
    status = report.get("status") if isinstance(report, dict) else None
    if status == "PASS":
        return "PASS"
    if status == "FAIL":
        return "FAIL"
    return "PRECONDITION"


def _live_report(report: dict) -> dict:
    if not isinstance(report, dict):
        return verdict("PRECONDITION", ["no worksheet report"])
    reasons = [str(reason) for reason in report.get("reasons") or []]
    status = _report_status(report)
    return verdict(status, reasons, {"worksheet": status})


def _criterion_report(result: tuple) -> dict:
    status, errors, detail = result
    return verdict(status, [str(error) for error in errors], detail if isinstance(detail, dict) else {})


def _materialise(files: dict) -> tempfile.TemporaryDirectory:
    directory = tempfile.TemporaryDirectory(dir=ROOT / "target")
    base = Path(directory.name)
    for relative, text in files.items():
        path = base / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        if isinstance(text, bytes):
            path.write_bytes(text)
        else:
            path.write_text(str(text))
    return directory


# --------------------------------------------------------------------------------------------------
# principles


def identity_agreement(observations: dict, parameters: dict) -> dict:
    report = _live.check_g1(
        observations.get("preflight"), parameters.get("min_observers", 4))
    expected = parameters.get("min_observers")
    if report.get("status") == "PASS" and isinstance(expected, int):
        validators = (observations.get("preflight") or {}).get("validators") or []
        if len(validators) < expected:
            return verdict("PRECONDITION",
                           [f"{len(validators)} observer record(s); at least {expected} required"])
    return _live_report(report)


def staged_lifecycle(observations: dict, parameters: dict) -> dict:
    configured = set(parameters.get("stages") or [])
    required = {"pooled", "includedUnfinalized", "finalizedSuccess"}
    if configured and configured != required:
        return verdict("FAIL", [f"unsupported stage set {sorted(configured)}; the principle "
                                f"asserts {sorted(required)}"])
    if parameters.get("per_observer") is False:
        return verdict("FAIL", ["the staged lifecycle is per-observer by definition"])
    subject = observations.get("subject")
    declared = subject.get("deploy_id") if isinstance(subject, dict) else None
    if not isinstance(declared, str) or not declared:
        return verdict("PRECONDITION", ["subject.deploy_id is absent"])
    run = observations.get("run")
    sampled = run.get("deploy_id") if isinstance(run, dict) else None
    if not isinstance(sampled, str) or not sampled:
        return verdict("PRECONDITION", ["run.deploy_id is absent"])
    if declared != sampled:
        return verdict(
            "FAIL", [f"the declared subject {declared!r} is not the sampled deploy {sampled!r}"])
    events = _events(observations)
    minimum = parameters.get("min_observers")
    if isinstance(minimum, int):
        aliases = {event.get("alias") for event in events if event.get("alias")}
        if len(aliases) < minimum:
            return verdict("PRECONDITION",
                           [f"{len(aliases)} observer(s) contributed; at least {minimum} required"])
    report = _live.check_g2(
        observations.get("submission"), run, _events(observations),
        minimum if isinstance(minimum, int) else 4)
    return _live_report(report)


def no_false_settlement(observations: dict, parameters: dict) -> dict:
    report = _live.check_g3(
        observations.get("claims"), _events(observations),
        parameters.get("min_observers", 4))
    return _live_report(report)


def recovery_no_loss(observations: dict, parameters: dict) -> dict:
    report = _live.check_g4(observations.get("recovery"), _events(observations))
    return _live_report(report)


def independent_observations(observations: dict, parameters: dict) -> dict:
    record = observations.get("record") or {}
    directory = record.get("dir")
    if not isinstance(directory, str) or not directory:
        return verdict("PRECONDITION", ["record.dir is absent"])
    path = Path(directory)
    if not path.is_absolute():
        path = ROOT / path
    if not path.is_dir():
        return verdict("PRECONDITION", [f"record.dir {directory!r} is absent"])
    report = _live.check_g5(
        observations.get("run"), _events(observations), path,
        parameters.get("min_observers", 4))
    return _live_report(report)


def human_attestation(observations: dict, parameters: dict) -> dict:
    attestation = observations.get("attestation")
    if not isinstance(attestation, dict):
        return verdict("PRECONDITION", ["no attestation record"])
    errors: list[str] = []
    for field in ("reviewer", "relationship", "independent", "signed_at", "verdict",
                  "evidence_sha256", "verdict_url", "evidence_record"):
        if field not in attestation:
            errors.append(f"attestation.{field} is required")
    if attestation.get("independent") is not True:
        errors.append("attestation.independent must be true")
    reviewer = str(attestation.get("reviewer") or "").strip()
    owner = str(parameters.get("owner") or "").strip()
    if not reviewer:
        errors.append("attestation.reviewer is required")
    if owner and (reviewer.casefold() in owner.casefold()
                  or owner.casefold() in reviewer.casefold()):
        errors.append("the reviewer must not be the gate owner")
    try:
        signed = dt.datetime.fromisoformat(
            str(attestation.get("signed_at") or "").replace("Z", "+00:00"))
        if signed.tzinfo is None:
            errors.append("attestation.signed_at must be timezone-aware")
    except ValueError:
        errors.append("attestation.signed_at is not a UTC timestamp")
    digest = attestation.get("evidence_sha256")
    if not (isinstance(digest, str) and HEX64.fullmatch(digest)):
        errors.append("attestation.evidence_sha256 must be a 64-hex digest")
    url = attestation.get("verdict_url")
    if not (isinstance(url, str) and url.startswith("https://")):
        errors.append("attestation.verdict_url must be https")
    record = attestation.get("evidence_record")
    if not _present(record):
        errors.append("attestation.evidence_record is required")
    else:
        path = Path(str(record))
        if not path.is_absolute():
            path = ROOT / path
        path = path.resolve()
        if not path.is_dir():
            errors.append("attestation.evidence_record is not a record directory")
        elif not path.is_relative_to(ROOT.resolve()):
            errors.append("attestation.evidence_record is outside the repository")
        elif not (path / "manifest.json").is_file():
            errors.append("attestation.evidence_record has no manifest.json")
        elif (isinstance(digest, str) and HEX64.fullmatch(digest)
              and hashlib.sha256((path / "manifest.json").read_bytes()).hexdigest() != digest):
            # **The signed digest answers "which bytes did the reviewer see?"** A 64-hex value
            # that matches nothing is the same as no digest at all; the promotion checker already
            # binds it to the evidence record's manifest.
            errors.append(
                "attestation.evidence_sha256 does not match the cited record's manifest")
    if errors:
        return verdict("FAIL", errors)
    recorded = attestation.get("verdict")
    if recorded not in ATTESTATION_VERDICTS:
        return verdict("FAIL", [f"unknown attestation verdict {recorded!r}"])
    if recorded == "FAIL":
        return verdict("FAIL", ["the independent reviewer recorded FAIL"])
    if recorded == "INCONCLUSIVE":
        return verdict("PRECONDITION", ["the independent reviewer recorded INCONCLUSIVE"])
    return verdict("PASS", [], {"verdict": recorded})


def healing_anchor(observations: dict, parameters: dict) -> dict:
    data = observations.get("record_data")
    if not isinstance(data, dict):
        return verdict("PRECONDITION", ["record_data is absent"])
    return _criterion_report(_criterion.check_rejoin(
        data, parameters.get("floor", _criterion.FLOOR),
        parameters.get("ceiling", _criterion.CEILING),
        parameters.get("min_dwell", 3)))


def merge_accounting(observations: dict, parameters: dict) -> dict:
    data = observations.get("record_data")
    if not isinstance(data, dict):
        return verdict("PRECONDITION", ["record_data is absent"])
    return _criterion_report(_criterion.check_merge(data))


def phase_diagnosis(observations: dict, parameters: dict) -> dict:
    health = observations.get("health")
    operator = observations.get("operator")
    if not isinstance(health, dict) or not isinstance(operator, dict):
        return verdict("PRECONDITION", ["health and operator observations are required"])
    wanted = parameters.get("locality")
    if wanted:
        declared = observations.get("locality")
        if not _present(declared):
            return verdict("PRECONDITION", ["the observation declares no locality"])
        if declared != wanted:
            return verdict("FAIL", [
                f"the observation declares {declared!r}; the criterion requires {wanted!r}"
            ])
    table_path = Path(parameters.get("table") or _c343.DEFAULT_TABLE)
    if not table_path.is_absolute():
        table_path = ROOT / table_path
    if not table_path.is_file():
        return verdict("PRECONDITION", [f"phase table {table_path} is absent"])
    table = _c343.load_table(table_path)
    block_text = observations.get("block6")
    evaluation = _c343.host_evaluation(
        health, operator, block_text if isinstance(block_text, str) else "", table)
    if evaluation["verdict"] == "PASS":
        return verdict("PASS", [], {
            "diagnosis": evaluation["diagnosis"],
            "block6": evaluation["cells"]["block6-disposition"]["detail"],
        })
    reasons = [
        f"{name}: {value['detail']}"
        for name, value in evaluation["cells"].items() if value["verdict"] != "PASS"
    ]
    return verdict(evaluation["verdict"], reasons)


def lockstep_acceptance(observations: dict, parameters: dict) -> dict:
    record = observations.get("record_data")
    if not isinstance(record, dict):
        return verdict("PRECONDITION", ["record_data is absent"])
    table_path = Path(parameters.get("table") or _c357.DEFAULT_TABLE)
    if not table_path.is_absolute():
        table_path = ROOT / table_path
    if not table_path.is_file():
        return verdict("PRECONDITION", [f"acceptance table {table_path} is absent"])
    requirements = _c357.load_table(table_path)
    report = _c357.check_record(record, requirements,
                                bool(parameters.get("require_original_trace")),
                                max_window=parameters.get("max_window", _c357.MAX_WINDOW))
    status = report.get("result")
    if status not in STATUSES:
        return verdict("FAIL", [f"unknown acceptance result {status!r}"])
    reasons = [
        f"{name}: {check.get('detail')}"
        for name, check in (report.get("checks") or {}).items()
        if check.get("verdict") != "PASS"
    ]
    if report.get("detail"):
        reasons.append(str(report["detail"]))
    return verdict(status, reasons, {"result": status})


def bounded_admission(observations: dict, parameters: dict) -> dict:
    source = observations.get("source")
    if not isinstance(source, dict):
        return verdict("PRECONDITION", ["source is absent"])
    directory = None
    try:
        if isinstance(source.get("text"), str):
            directory = _materialise({"source.rs": source["text"]})
            path = Path(directory.name) / "source.rs"
        else:
            text_path = source.get("path")
            if not isinstance(text_path, str) or not text_path:
                return verdict("PRECONDITION", ["source.path and source.text are absent"])
            path = Path(text_path)
            if not path.is_absolute():
                path = ROOT / path
        if not path.is_file():
            return verdict("PRECONDITION", [f"source {path} is absent"])
        command = ["bash", str(ROOT / "tools" / "check-bounded-ingress-queues.sh"),
                   "--source", str(path), "--gate"]
        queues = parameters.get("queues")
        if isinstance(queues, list) and queues:
            command.extend(["--queues", ",".join(str(queue) for queue in queues)])
        proc = subprocess.run(command, capture_output=True, text=True, timeout=120,
                              check=False)
        if proc.returncode == 0:
            return verdict("PASS", [], {"source": str(path)})
        return verdict("FAIL", [line for line in (proc.stderr or proc.stdout).splitlines() if line][-4:])
    finally:
        if directory is not None:
            directory.cleanup()


def pressure_refusal(observations: dict, parameters: dict) -> dict:
    """M2's two halves: bounded queues (source) and deterministic overload refusal (record).

    The source half stays `bounded_admission`; this principle composes it and then measures the
    typed pressure record. An unset-default run that refuses, a 5xx, a refusal without a retry
    signal or a summary that disagrees with its transcripts are all failures; a missing surface or
    record is a precondition, never a pass.
    """
    source = bounded_admission(observations, {"queues": parameters.get("queues", [])})
    if source["status"] != "PASS":
        return source
    pressure = observations.get("pressure")
    if not isinstance(pressure, dict):
        return verdict("PRECONDITION", ["pressure is absent"])
    status, problems = _m2pressure.check_pressure(pressure, parameters)
    return verdict(status, problems)


def off_host_claim(observations: dict, parameters: dict) -> dict:
    record = observations.get("record")
    if not isinstance(record, dict):
        return verdict("PRECONDITION", ["record is absent"])
    directory = None
    try:
        if isinstance(record.get("files"), dict) and record["files"]:
            directory = _materialise(record["files"])
            path = Path(directory.name)
        else:
            claimed = record.get("dir")
            if not isinstance(claimed, str) or not claimed:
                return verdict("PRECONDITION", ["record.files and record.dir are absent"])
            path = Path(claimed)
            if not path.is_absolute():
                path = ROOT / path
        if not path.is_dir():
            return verdict("PRECONDITION", [f"record {path} is absent"])
        problems = _locality.check(path)
        if problems:
            return verdict("FAIL", problems)
        return verdict("PASS", [], {"record": str(path)})
    finally:
        if directory is not None:
            directory.cleanup()


def join_chain(observations: dict, parameters: dict) -> dict:
    """The M3 six-stage chain, plus the locality policy that record claims.

    `record.chain` is the inline form the generic fixture mutates; a real record carries
    `chain.json` beside its README. The chain checker refuses a send-log-only, missing-stage or
    hash-mismatched chain; the locality half is the same `off_host_claim` invariant, so an
    off-host policy cannot pass on a local-devnet chain.
    """
    record = observations.get("record")
    if not isinstance(record, dict):
        return verdict("PRECONDITION", ["record is absent"])
    policy = parameters.get("locality")
    if policy not in ("off-host", "local-devnet", "any"):
        return verdict("FAIL", [f"unknown locality policy {policy!r}"])
    chain = record.get("chain")
    directory = None
    try:
        if isinstance(record.get("files"), dict) and record["files"]:
            directory = _materialise(record["files"])
            path = Path(directory.name)
        else:
            claimed = record.get("dir")
            if not isinstance(claimed, str) or not claimed:
                if chain is None:
                    return verdict("PRECONDITION", ["record.files, record.dir and record.chain are absent"])
                path = None
            else:
                path = Path(claimed)
                if not path.is_absolute():
                    path = ROOT / path
        problems = []
        label = None
        if path is not None:
            if not path.is_dir():
                return verdict("PRECONDITION", [f"record {path} is absent"])
            label = _locality.declared_locality(path)
            if policy in ("off-host", "local-devnet"):
                problems.extend(_locality.check(path))
            if chain is None:
                resolved = _m3chain.chain_path(path)
                if resolved is None:
                    return verdict(
                        "PRECONDITION", [f"{path}: chain.json is absent (root or raw/)"])
                try:
                    chain = json.loads(resolved.read_text())
                except ValueError as error:
                    return verdict("FAIL", [f"{resolved} is not valid JSON: {error}"])
        if chain is None:
            return verdict("PRECONDITION", ["record.chain is absent"])
        problems.extend(_m3chain.check_chain(chain))
        if policy == "off-host":
            if label != "off-host-claim":
                problems.append(
                    f"the record declares {label or 'no locality'}; an off-host criterion "
                    "requires 'off-host-claim'")
            host = _m3chain.declared_host(
                (chain.get("correlation") or {}).get("advertised_peer")
                if isinstance(chain, dict) else None)
            if host is not None and _locality.local_address(host):
                problems.append(f"the advertised peer {host!r} is local, not routable")
        elif policy == "local-devnet" and label not in ("local-devnet", "single-host"):
            problems.append(
                f"the record declares {label or 'no locality'}; a local policy requires "
                "'local-devnet' or 'single-host'")
        if (
            label is not None
            and isinstance(chain, dict)
            and chain.get("locality") not in (None, label)
        ):
            problems.append(
                f"the chain declares {chain.get('locality')!r} but the record declares {label!r}")
        # **The quoted lines must exist in the captured logs, not only inside the chain.** An
        # off-host record must also be digest-pinned, however it was supplied (`files` or `dir`);
        # a local fixture or bare run directory is bound by inclusion and, when it carries sums,
        # checked against them.
        if (
            path is not None
            and isinstance(chain, dict)
            and isinstance(chain.get("logs"), dict)
        ):
            problems.extend(_m3chain.raw_problems(path, chain))
            if policy == "off-host" or (path / "SHA256SUMS.json").is_file():
                problems.extend(_m3chain.pin_problems(path, chain))
        if problems:
            return verdict("FAIL", problems)
        return verdict("PASS", [], {"locality": label, "state_hash": chain.get("correlation", {}).get("state_hash")})
    finally:
        if directory is not None:
            directory.cleanup()


def resource_attribution(observations: dict, parameters: dict) -> dict:
    """The observed process table, counted by name.

    `declared_count` is the record's own claim, so this is a self-consistency check, not
    independent evidence. It is still required: without it the declared-versus-measured rule has
    nothing to compare, and a record could omit the number it is supposed to prove. A pid must be a
    positive integer and appear once; the Rust witness counts numeric `/proc/<pid>` entries only.
    """
    record = observations.get("record")
    if not isinstance(record, dict):
        return verdict("PRECONDITION", ["record is absent"])
    processes = record.get("host_processes")
    if not isinstance(processes, list) or not processes:
        return verdict("PRECONDITION", ["record.host_processes is absent or empty"])
    declared = record.get("declared_count")
    if declared is None:
        return verdict("PRECONDITION", ["record.declared_count is absent"])
    if not isinstance(declared, int) or isinstance(declared, bool) or declared < 0:
        return verdict("FAIL", ["record.declared_count must be a non-negative integer"])
    name = str(parameters.get("process_name") or "rnode")
    counted: list[int] = []
    for entry in processes:
        if not isinstance(entry, dict) or not isinstance(entry.get("name"), str):
            return verdict("FAIL", ["every process entry must be an object with a name"])
        pid = entry.get("pid")
        if not isinstance(pid, int) or isinstance(pid, bool) or pid <= 0:
            return verdict("FAIL", ["every process entry must carry a positive integer pid"])
        if entry["name"] == name:
            counted.append(pid)
    if not counted:
        return verdict("PRECONDITION", [f"no process named {name!r} was observed"])
    if len(set(counted)) != len(counted):
        return verdict("FAIL", [f"a {name} process pid is listed more than once"])
    if declared != len(counted):
        return verdict(
            "FAIL", [f"declared count {declared} != observed {len(counted)} {name} process(es)"])
    return verdict("PASS", [], {"count": len(counted), "process_name": name})


PRINCIPLES = {
    "identity_agreement": identity_agreement,
    "staged_lifecycle": staged_lifecycle,
    "no_false_settlement": no_false_settlement,
    "recovery_no_loss": recovery_no_loss,
    "independent_observations": independent_observations,
    "human_attestation": human_attestation,
    "healing_anchor": healing_anchor,
    "merge_accounting": merge_accounting,
    "phase_diagnosis": phase_diagnosis,
    "lockstep_acceptance": lockstep_acceptance,
    "bounded_admission": bounded_admission,
    "pressure_refusal": pressure_refusal,
    "off_host_claim": off_host_claim,
    "join_chain": join_chain,
    "resource_attribution": resource_attribution,
}

PARAMETER_SCHEMA = {
    "identity_agreement": {"min_observers": int},
    "staged_lifecycle": {"stages": list, "per_observer": bool, "min_observers": int},
    "no_false_settlement": {"min_observers": int},
    "recovery_no_loss": {},
    "independent_observations": {"min_observers": int},
    "human_attestation": {"owner": str},
    "healing_anchor": {"floor": (int, float), "ceiling": (int, float), "min_dwell": int},
    "merge_accounting": {},
    "phase_diagnosis": {"table": str, "locality": str},
    "lockstep_acceptance": {"table": str, "require_original_trace": bool,
                            "max_window": (int, float)},
    "bounded_admission": {"queues": list},
    "pressure_refusal": {"queues": list, "locality": str, "rate_ceiling": int},
    "off_host_claim": {},
    "join_chain": {"locality": str},
    "resource_attribution": {"process_name": str},
}


def validate_parameters(principle: str, parameters: object) -> list[str]:
    schema = PARAMETER_SCHEMA.get(principle)
    if schema is None:
        return [f"unknown principle {principle!r}"]
    if parameters is None:
        parameters = {}
    if not isinstance(parameters, dict):
        return ["parameters must be an object"]
    errors = []
    for key, expected in schema.items():
        if key not in parameters:
            errors.append(f"parameters.{key} is required for {principle}")
            continue
        value = parameters[key]
        if expected is int and (not isinstance(value, int) or isinstance(value, bool)):
            errors.append(f"parameters.{key} must be an integer")
        elif expected is str and not isinstance(value, str):
            errors.append(f"parameters.{key} must be a string")
        elif expected is list and not isinstance(value, list):
            errors.append(f"parameters.{key} must be a list")
        elif expected is bool and not isinstance(value, bool):
            errors.append(f"parameters.{key} must be a boolean")
        elif isinstance(expected, tuple) and not isinstance(value, expected):
            errors.append(f"parameters.{key} must be numeric")
    for key in parameters:
        if key not in schema:
            errors.append(f"parameters.{key} is not used by {principle}")
    return errors


def evaluate(principle: str, observations: dict, parameters: dict) -> dict:
    function = PRINCIPLES.get(principle)
    if function is None:
        return verdict("FAIL", [f"unknown principle {principle!r}"])
    try:
        return function(observations, parameters or {})
    except Exception as error:  # a criterion that raises is a criterion that failed
        return verdict("FAIL", [f"the principle raised {type(error).__name__}: {error}"])


def evaluate_entry(entry: dict, observations: dict) -> dict:
    missing = [path for path in entry.get("requires") or [] if not _present(_get(observations, path))]
    if missing:
        return verdict("PRECONDITION",
                       [f"missing required observation: {path}" for path in missing])
    try:
        return evaluate(entry.get("principle"), observations, entry.get("parameters") or {})
    except Exception as error:  # a criterion that raises is a criterion that failed
        return verdict("FAIL", [f"the principle raised {type(error).__name__}: {error}"])


def load_criteria(path: Path = CRITERIA) -> dict:
    document = json.loads(Path(path).read_text())
    if document.get("schema") != 1:
        raise ValueError("gate-criteria schema 1 required")
    entries = document.get("criteria")
    if not isinstance(entries, list) or not entries:
        raise ValueError("gate-criteria has no criteria")
    return document


def criteria_by_gate(path: Path = CRITERIA) -> dict[str, dict]:
    return {entry["gate_id"]: entry for entry in load_criteria(path)["criteria"]}


def main(argv: list[str] | None = None) -> int:
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gate", required=True)
    parser.add_argument("--observations", required=True)
    parser.add_argument("--criteria", default=str(CRITERIA))
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    entries = criteria_by_gate(Path(args.criteria))
    entry = entries.get(args.gate)
    if entry is None:
        print(json.dumps({"status": "PRECONDITION", "reasons": [f"no criterion for {args.gate}"]}))
        return 3
    observations = json.loads(Path(args.observations).read_text())
    result = evaluate_entry(entry, observations)
    print(json.dumps(result, indent=1, sort_keys=True))
    return {"PASS": 0, "FAIL": 1, "PRECONDITION": 3}[result["status"]]


if __name__ == "__main__":
    raise SystemExit(main())
