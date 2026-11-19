#!/usr/bin/env python3
"""m2-pressure — the typed overload witness for #428 M2.

M2's prose is a statement about behaviour under load: concurrent requests, bounded queues,
explicit backpressure, deterministic acceptance or rejection, and no 5xx. The source-shape check
(`check-bounded-ingress-queues.sh`) is a necessary half, but a source fixture cannot show that a
saturated node refuses rather than silently drops, and it cannot show that the unset default lets
the same burst through.

This tool makes the second half executable:

  * `check_pressure` validates a typed pressure record: paired capped/unset runs, the refusal class
    recomputed from the raw transcripts, no server errors, an explicit retry signal, a handler
    reachable after release, and a rate under the per-peer limiter's ceiling;
  * `check --record DIR --locality ...` binds the record to its raw transcript bytes and to its
    declared locality.

The record is a **correlation of captured responses**, not a cryptographic proof that the run
happened. `execution_proven` is false; a coordinated operator can still lie. What the binding does
prevent is an accidental local run, an edited summary, or a summary that disagrees with its own
transcripts from passing as the public-live positive.

Usage:
  tools/m2-pressure.py check --record DIR --locality public-live|local-devnet|any
  tools/m2-pressure.py check-chain --record FILE

Exit codes mirror `gate-check.py`: 0 PASS, 1 FAIL, 3 PRECONDITION (a required input is absent).
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import math
import re
import sys
import tempfile
from pathlib import Path
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parents[1]
HEX40 = re.compile(r"^[0-9a-f]{40}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
CAP_KEY = "RNODE_HTTP_MAX_INFLIGHT_DEPLOY"
SURFACES = ("http", "direct_http", "grpc")
POLICIES = ("public-live", "local-devnet", "any")
LABELS = ("public-live", "local-devnet", "single-host")
REFUSAL_MESSAGE = "HTTP in-flight capacity exhausted"
STATUS_429 = "HTTP/1.1 429"
STATUS_400 = "HTTP/1.1 400"


def _module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


_locality = _module("m2_pressure_locality", ROOT / "tools" / "check-evidence-locality.py")


def _nonempty(value: object) -> bool:
    return isinstance(value, str) and bool(value)


def _int(value: object, minimum: int | None = None) -> bool:
    if not isinstance(value, int) or isinstance(value, bool):
        return False
    return minimum is None or value >= minimum


def materialise(spec: object) -> tuple[Path | None, tempfile.TemporaryDirectory | None]:
    """A record directory from an inline `files` map or a `dir` reference."""
    if not isinstance(spec, dict):
        return None, None
    files = spec.get("files")
    if isinstance(files, dict) and files:
        # A Python-only job on a fresh checkout has no target/; a fixture must not depend on a
        # build artefact.
        (ROOT / "target").mkdir(parents=True, exist_ok=True)
        directory = tempfile.TemporaryDirectory(dir=ROOT / "target")
        base = Path(directory.name)
        for relative, text in files.items():
            path = base / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            if isinstance(text, bytes):
                path.write_bytes(text)
            else:
                path.write_text(str(text))
        return base, directory
    claimed = spec.get("dir")
    if not _nonempty(claimed):
        return None, None
    path = Path(claimed)
    if not path.is_absolute():
        path = ROOT / path
    return path, None


def read_pressure(base: Path) -> tuple[dict | None, str | None]:
    for name in ("pressure.json", "raw/pressure.json"):
        path = base / name
        if path.is_file():
            try:
                document = json.loads(path.read_text())
            except ValueError as error:
                return None, f"{path}: not valid JSON: {error}"
            if not isinstance(document, dict):
                return None, f"{path}: not an object"
            return document, None
    return None, f"{base}: pressure.json is absent (root or raw/)"


def resolve(base: Path, name: object) -> Path | None:
    """A named file at the record root or under `raw/` (the packer's layout)."""
    if not _nonempty(name):
        return None
    for candidate in (base / name, base / "raw" / name):
        if candidate.is_file():
            return candidate
    return None


def _lines(base: Path, name: object) -> tuple[list[str] | None, str | None]:
    if not _nonempty(name):
        return None, "transcript path is absent"
    path = resolve(base, name)
    if path is None:
        return None, f"{name} is absent from the record"
    return path.read_text(errors="replace").splitlines(), None


def _jsonl(lines: list[str], name: str, problems: list[str]) -> list[dict]:
    records = []
    for number, line in enumerate(lines, 1):
        if not line.strip():
            continue
        try:
            record = json.loads(line)
        except ValueError as error:
            problems.append(f"{name}:{number}: not JSON: {error}")
            continue
        if not isinstance(record, dict):
            problems.append(f"{name}:{number}: not an object")
            continue
        records.append(record)
    return records


def _read_version(base: Path, run: dict) -> str | None:
    path = resolve(base, run.get("version"))
    return path.read_text(errors="replace") if path is not None else None


def _read_inspect(base: Path, run: dict) -> list[str] | None:
    path = resolve(base, run.get("inspect"))
    if path is None:
        return None
    try:
        document = json.loads(path.read_text())
    except ValueError:
        return None
    if isinstance(document, list) and document and isinstance(document[0], dict):
        config = document[0].get("Config")
        if isinstance(config, dict) and isinstance(config.get("Env"), list):
            return [str(item) for item in config["Env"]]
    if isinstance(document, dict) and isinstance(document.get("env"), list):
        return [str(item) for item in document["env"]]
    return None


def _cap_value(run: dict) -> int | None:
    env = run.get("env")
    if not isinstance(env, dict):
        return None
    value = env.get(CAP_KEY)
    return value if _int(value, 1) else None


def _recompute(records: list[dict], row: dict, label: str, problems: list[str]) -> None:
    """Recompute the summary counts from the transcript; a disagreement is a failure."""
    probes = [record for record in records if record.get("kind") == "probe"]
    held = [record for record in records if record.get("kind") == "held"]
    after = [record for record in records if record.get("kind") == "after"]
    refused = [record for record in probes if record.get("status") == 429]
    admitted = [record for record in probes if record.get("status") == 400]
    server = [record for record in probes if isinstance(record.get("status"), int)
              and 500 <= record["status"] <= 599]
    retry = [record for record in refused if record.get("retry_after") == 1]
    if row.get("probe") != len(probes):
        problems.append(f"{label}: probe count {row.get('probe')!r} != transcript {len(probes)}")
    if row.get("held") is not None and row.get("held") != len(held):
        problems.append(f"{label}: held count {row.get('held')!r} != transcript {len(held)}")
    if row.get("refused") != len(refused):
        problems.append(f"{label}: refused count {row.get('refused')!r} != transcript {len(refused)}")
    if row.get("admitted") is not None and row.get("admitted") != len(admitted):
        problems.append(f"{label}: admitted count {row.get('admitted')!r} != transcript {len(admitted)}")
    if row.get("server_errors") != len(server):
        problems.append(
            f"{label}: server_errors {row.get('server_errors')!r} != transcript {len(server)}"
        )
    if refused and row.get("retry_after") is not None and row.get("retry_after") != len(retry):
        problems.append(
            f"{label}: retry_after count {row.get('retry_after')!r} != transcript {len(retry)}"
        )
    if after:
        last = max((int(record.get("t_ms") or 0) for record in probes), default=0)
        if int(after[-1].get("t_ms") or 0) < last:
            problems.append(f"{label}: the after-release request precedes the probes")


def _check_http(surface: dict, base: Path, source: str, problems: list[str],
                preconditions: list[str], rate_ceiling: int) -> None:
    capped = surface.get("capped")
    unset = surface.get("unset")
    if not isinstance(capped, dict) or not isinstance(unset, dict):
        preconditions.append("http: capped and unset runs are both required")
        return
    cap = capped.get("cap")
    if not _int(cap, 1) or cap > 64:
        problems.append(f"http.capped.cap {cap!r} is not an integer in 1..64")
    held = capped.get("held")
    if held != cap:
        problems.append(f"http.capped.held {held!r} != cap {cap!r}; not every permit was held")
    refusal = capped.get("refusal")
    if not isinstance(refusal, dict):
        problems.append("http.capped.refusal is absent")
        refusal = {}
    if refusal.get("class") != "429":
        problems.append(f"http.capped.refusal.class {refusal.get('class')!r} is not '429'")
    message = refusal.get("message")
    if not _nonempty(message):
        problems.append("http.capped.refusal.message is absent")
    elif message not in source:
        problems.append("http.capped.refusal.message does not appear in the source")

    lines, error = _lines(base, capped.get("transcript"))
    if error:
        problems.append(f"http.capped: {error}")
        return
    records = _jsonl(lines, str(capped.get("transcript")), problems)
    _recompute(records, capped, "http.capped", problems)
    probes = [record for record in records if record.get("kind") == "probe"]
    refused = [record for record in probes if record.get("status") == 429]
    admitted = [record for record in probes if record.get("status") == 400]
    if not refused:
        problems.append("http.capped: no probe was refused; the cap did not saturate")
    if admitted:
        problems.append("http.capped: a probe reached the handler while every permit was held")
    for record in refused:
        if record.get("retry_after") != 1:
            problems.append("http.capped: a 429 carries no retry-after: 1")
        if record.get("body") != message:
            problems.append("http.capped: a 429 body is not the admission refusal message")
    after = [record for record in records if record.get("kind") == "after"]
    if not after or after[-1].get("status") != 400:
        problems.append("http.capped: no after-release request reached the handler (400)")
    rate = capped.get("requests_per_second")
    if not isinstance(rate, (int, float)) or isinstance(rate, bool) or rate <= 0:
        problems.append("http.capped.requests_per_second is absent or non-positive")
    elif rate > rate_ceiling:
        problems.append(
            f"http.capped.requests_per_second {rate} exceeds the per-peer ceiling "
            f"{rate_ceiling}; the refusal may be the rate limiter, not admission"
        )

    lines, error = _lines(base, unset.get("transcript"))
    if error:
        problems.append(f"http.unset: {error}")
        return
    records = _jsonl(lines, str(unset.get("transcript")), problems)
    _recompute(records, unset, "http.unset", problems)
    probes = [record for record in records if record.get("kind") == "probe"]
    refused = [record for record in probes if record.get("status") == 429]
    admitted = [record for record in probes if record.get("status") == 400]
    if refused:
        problems.append("http.unset: the unset default refused a probe")
    if not admitted:
        problems.append("http.unset: no probe reached the handler; the control did not run")
    rate = unset.get("requests_per_second")
    if not isinstance(rate, (int, float)) or isinstance(rate, bool) or rate <= 0:
        problems.append("http.unset.requests_per_second is absent or non-positive")
    elif rate > rate_ceiling:
        problems.append(
            f"http.unset.requests_per_second {rate} exceeds the per-peer ceiling {rate_ceiling}"
        )


def _check_direct(surface: dict, base: Path, problems: list[str],
                  preconditions: list[str]) -> None:
    capped = surface.get("capped")
    unset = surface.get("unset")
    if not isinstance(capped, dict) or not isinstance(unset, dict):
        preconditions.append("direct_http: capped and unset runs are both required")
        return
    lines, error = _lines(base, capped.get("transcript"))
    if error:
        problems.append(f"direct_http.capped: {error}")
        return
    text = "\n".join(lines)
    if STATUS_429 not in text:
        problems.append("direct_http.capped: no 'HTTP/1.1 429' status line")
    if not re.search(r"(?im)^retry-after:\s*1\s*$", text):
        problems.append("direct_http.capped: no 'retry-after: 1' header")
    if STATUS_400 not in text:
        problems.append("direct_http.capped: no after-release 'HTTP/1.1 400'")
    if re.search(r"(?m)^HTTP/1\.1 5\d\d", text):
        problems.append("direct_http.capped: a 5xx status line")
    lines, error = _lines(base, unset.get("transcript"))
    if error:
        problems.append(f"direct_http.unset: {error}")
        return
    text = "\n".join(lines)
    if STATUS_400 not in text:
        problems.append("direct_http.unset: the control did not reach the handler (400)")
    if STATUS_429 in text:
        problems.append("direct_http.unset: the unset default refused a request")


def _check_grpc(surface: dict, base: Path, problems: list[str],
                preconditions: list[str]) -> None:
    capped = surface.get("capped")
    permissive = surface.get("permissive")
    if not isinstance(capped, dict) or not isinstance(permissive, dict):
        preconditions.append("grpc: capped and permissive runs are both required")
        return
    arms = capped.get("arms")
    if not isinstance(arms, dict):
        preconditions.append("grpc.capped.arms is absent")
        return
    for arm in ("ingress_rate", "pool_full"):
        if not isinstance(arms.get(arm), dict):
            preconditions.append(f"grpc.capped.arms.{arm} is absent")
    lines, error = _lines(base, capped.get("transcript"))
    if error:
        problems.append(f"grpc.capped: {error}")
        return
    records = _jsonl(lines, str(capped.get("transcript")), problems)
    for arm in ("ingress_rate", "pool_full"):
        row = arms.get(arm)
        if not isinstance(row, dict):
            continue
        observed = [record for record in records if record.get("arm") == arm]
        refused = [record for record in observed if record.get("code") == 8]
        admitted = [record for record in observed if record.get("code") == 0]
        if row.get("refused") is not None and row.get("refused") != len(refused):
            problems.append(f"grpc.{arm}: refused count disagrees with the transcript")
        if row.get("admitted") is not None and row.get("admitted") != len(admitted):
            problems.append(f"grpc.{arm}: admitted count disagrees with the transcript")
        if arm == "ingress_rate" and not admitted:
            problems.append("grpc.ingress_rate: no admitted call; the arm did not exercise the bound")
        if not refused:
            problems.append(f"grpc.{arm}: no RESOURCE_EXHAUSTED refusal")
        if any(record.get("code") not in (0, 8) for record in observed):
            problems.append(f"grpc.{arm}: a status outside the accepted/refused classes")
        if arm == "pool_full" and row.get("retry_after") != 1:
            problems.append("grpc.pool_full: the refusal carries no retry-after: 1")
        if arm == "ingress_rate":
            for record in refused:
                if record.get("retry_after") not in (None, 1):
                    problems.append("grpc.ingress_rate: malformed retry-after")
    lines, error = _lines(base, permissive.get("transcript"))
    if error:
        problems.append(f"grpc.permissive: {error}")
        return
    records = _jsonl(lines, str(permissive.get("transcript")), problems)
    refused = [record for record in records if record.get("code") == 8]
    admitted = [record for record in records if record.get("code") == 0]
    if refused:
        problems.append("grpc.permissive: the guard refused unconditionally")
    if not admitted:
        problems.append("grpc.permissive: no admitted call; the control did not run")


def check_transcripts(record: dict, base: Path, source: str,
                      rate_ceiling: int) -> tuple[list[str], list[str]]:
    problems: list[str] = []
    preconditions: list[str] = []
    surfaces = record.get("surfaces")
    if not isinstance(surfaces, dict):
        return problems, ["surfaces is absent"]
    for name in SURFACES:
        if not isinstance(surfaces.get(name), dict):
            preconditions.append(f"surfaces.{name} is absent")
    if isinstance(surfaces.get("http"), dict):
        _check_http(surfaces["http"], base, source, problems, preconditions, rate_ceiling)
    if isinstance(surfaces.get("direct_http"), dict):
        _check_direct(surfaces["direct_http"], base, problems, preconditions)
    if isinstance(surfaces.get("grpc"), dict):
        _check_grpc(surfaces["grpc"], base, problems, preconditions)
    return problems, preconditions


def check_binding(record: dict, base: Path) -> list[str]:
    """Every named raw file exists; a packed record's sums cover it."""
    problems: list[str] = []
    named: set[str] = set()

    def walk(value: object) -> None:
        if isinstance(value, dict):
            for key, item in value.items():
                if key in ("transcript", "inspect", "version") and _nonempty(item):
                    named.add(item)
                else:
                    walk(item)
        elif isinstance(value, list):
            for item in value:
                walk(item)

    walk(record)
    for candidate in ("pressure.json", "raw/pressure.json"):
        if (base / candidate).is_file():
            named.add(candidate)
            break
    for name in sorted(named):
        if resolve(base, name) is None:
            problems.append(f"{name} is absent from the record")
    sums_path = base / "SHA256SUMS.json"
    if not sums_path.is_file():
        sums_path = base / "raw" / "SHA256SUMS.json"
    if not sums_path.is_file():
        # A named transcript that is not pinned is not evidence: the record can be edited after
        # the run. The same rule the M3 record applies, for the same reason.
        if named:
            problems.append("the record has no SHA256SUMS.json to pin its raw files")
    else:
        try:
            sums = json.loads(sums_path.read_text())
        except ValueError as error:
            return problems + [f"{sums_path}: not valid JSON: {error}"]
        if not isinstance(sums, dict):
            return problems + [f"{sums_path}: not an object"]
        for name in sorted(named):
            path = resolve(base, name)
            if path is None:
                continue
            relative = path.relative_to(base).as_posix()
            digest = sums.get(relative, sums.get(name))
            if digest is None:
                problems.append(f"{name} is not digest-pinned")
            elif digest != hashlib.sha256(path.read_bytes()).hexdigest():
                problems.append(f"{name} digest does not match SHA256SUMS.json")
    return problems


def check_pressure(spec: dict, parameters: dict, source: str = "") -> tuple[str, list[str]]:
    """The typed-record half. Returns (PASS|FAIL|PRECONDITION, problems)."""
    base, cleanup = materialise(spec)
    try:
        if base is None:
            return "PRECONDITION", ["pressure.dir and pressure.files are absent"]
        if not base.is_dir():
            return "PRECONDITION", [f"pressure record {base} is absent"]
        if isinstance(spec.get("record"), dict):
            record, error = spec["record"], None
        else:
            record, error = read_pressure(base)
        if error:
            return "PRECONDITION", [error]
        problems: list[str] = []
        if record.get("schema") != 1:
            problems.append(f"schema {record.get('schema')!r} is not 1")
        if record.get("gate_id") != "G428-M2":
            problems.append(f"gate_id {record.get('gate_id')!r} is not 'G428-M2'")
        locality = record.get("locality")
        if not _nonempty(locality):
            problems.append("locality is absent")
        declared = _locality.declared_locality(base)
        if declared is None:
            problems.append("the record has no single README locality label")
        elif declared != locality:
            problems.append(f"README declares {declared!r} but pressure.json declares {locality!r}")
        wanted = parameters.get("locality")
        if wanted not in POLICIES:
            problems.append(f"parameter locality {wanted!r} is unknown")
        elif wanted != "any" and locality != wanted:
            problems.append(
                f"the record declares {locality!r}; the criterion requires {wanted!r}"
            )
        endpoint = record.get("endpoint")
        if not isinstance(endpoint, dict):
            problems.append("endpoint is absent")
        else:
            if endpoint.get("proxy") != "none":
                problems.append(f"endpoint.proxy {endpoint.get('proxy')!r} is not 'none'")
            url = endpoint.get("url")
            if not _nonempty(url):
                problems.append("endpoint.url is absent")
            else:
                parsed = urlparse(url)
                if parsed.scheme not in ("http", "https") or not parsed.hostname:
                    problems.append(f"endpoint.url {url!r} is not an http(s) URL")
                elif wanted == "public-live" and _locality.local_address(parsed.hostname):
                    problems.append(f"endpoint host {parsed.hostname!r} is local, not public-live")
        candidate = record.get("candidate")
        if not isinstance(candidate, dict):
            problems.append("candidate is absent")
            candidate = {}
        source_sha = candidate.get("source_sha")
        binary = candidate.get("binary_sha256")
        if not _nonempty(source_sha) or not HEX40.match(str(source_sha)):
            problems.append("candidate.source_sha is not 40-hex")
        if not _nonempty(binary) or not HEX64.match(str(binary)):
            problems.append("candidate.binary_sha256 is not 64-hex")
        runs = record.get("runs")
        cap = None
        if not isinstance(runs, dict) or not isinstance(runs.get("capped"), dict) \
                or not isinstance(runs.get("unset"), dict):
            problems.append("runs.capped and runs.unset are both required")
        else:
            cap = _cap_value(runs["capped"])
            if cap is None:
                problems.append(f"runs.capped.env.{CAP_KEY} is not an integer")
            elif cap > 64:
                problems.append(f"runs.capped.env.{CAP_KEY} exceeds 64")
            if _cap_value(runs["unset"]) is not None:
                problems.append(f"runs.unset.env.{CAP_KEY} is set; the control is not unset")
            for role, run in (("capped", runs["capped"]), ("unset", runs["unset"])):
                env = _read_inspect(base, run)
                if env is None:
                    problems.append(f"runs.{role}.inspect is absent or unreadable")
                elif role == "capped" and not any(
                        item.startswith(f"{CAP_KEY}=") for item in env):
                    problems.append(f"runs.capped.inspect does not carry {CAP_KEY}")
                elif role == "unset" and any(item.startswith(f"{CAP_KEY}=") for item in env):
                    problems.append(f"runs.unset.inspect carries {CAP_KEY}")
                version = _read_version(base, run)
                if version is None:
                    problems.append(f"runs.{role}.version is absent")
                elif _nonempty(source_sha) and str(source_sha) not in version:
                    problems.append(f"runs.{role}.version does not name candidate.source_sha")
        surfaces = record.get("surfaces")
        if isinstance(surfaces, dict) and isinstance(surfaces.get("http"), dict):
            http = surfaces["http"]
            surface_cap = (http.get("capped") or {}).get("cap") \
                if isinstance(http.get("capped"), dict) else None
            if cap is not None and surface_cap != cap:
                problems.append(
                    f"http.capped.cap {surface_cap!r} != runs.capped env {cap!r}"
                )
        rate_ceiling = parameters.get("rate_ceiling", 100)
        if not _int(rate_ceiling, 1):
            problems.append(f"rate_ceiling {rate_ceiling!r} is not a positive integer")
            rate_ceiling = 100
        if not _nonempty(source):
            http_source = ROOT / "node" / "src" / "web" / "http.rs"
            source = http_source.read_text(errors="replace") if http_source.is_file() else ""
        if not _nonempty(source):
            problems.append("the source text is absent; the refusal class cannot be bound")
        transcript_problems, preconditions = check_transcripts(
            record, base, source, rate_ceiling
        )
        problems.extend(transcript_problems)
        problems.extend(check_binding(record, base))
        if preconditions:
            return "PRECONDITION", preconditions + problems
        return ("FAIL", problems) if problems else ("PASS", [])
    finally:
        if cleanup is not None:
            cleanup.cleanup()


def check_record(directory: Path, policy: str) -> tuple[int, list[str]]:
    if policy not in POLICIES:
        return 3, [f"unknown locality policy {policy!r}"]
    status, problems = check_pressure({"dir": str(directory)}, {"locality": policy})
    return {"PASS": 0, "FAIL": 1, "PRECONDITION": 3}[status], problems


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    check = sub.add_parser("check", help="check a pressure record")
    check.add_argument("--record", required=True, type=Path)
    check.add_argument("--locality", default="any", choices=POLICIES)
    chain = sub.add_parser("check-chain", help="check pressure.json only")
    chain.add_argument("--record", required=True, type=Path)
    args = parser.parse_args(argv)
    if args.command == "check-chain":
        status, problems = check_pressure({"dir": str(args.record)}, {"locality": "any"})
        for problem in problems:
            print(problem, file=sys.stderr)
        return {"PASS": 0, "FAIL": 1, "PRECONDITION": 3}[status]
    code, problems = check_record(args.record, args.locality)
    for problem in problems:
        print(problem, file=sys.stderr)
    return code


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
