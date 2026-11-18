#!/usr/bin/env python3
"""live-gate — sample the #584 live four-validator run and check G1–G5 without deciding.

The #584 gate closes only when a named independent reviewer records **G6** against **G1–G5**
(`spec/audit/runbooks/n487-live-postdeployment-20261108.md`). This tool is the instrument between
the runbook's arms and that review:

* `sample` polls four validator endpoints **read-only** at a fixed cadence and records every answer,
  including 503s, timeouts and connection errors, as timestamped events with the exact response body
  and its digest. It never submits a deploy, restarts a node, signs anything or touches a key. The
  single authorized submission is the operator's, recorded separately in `submission.json`.
* `check` reads a run directory and reports each of G1–G5 as PASS / FAIL / INCOMPLETE with the
  checks that produced it. A supplied `--verdict` record is validated (reviewer, named reviewer,
  verdict enum, evidence digest, https verdict URL, UTC timestamp and a cited evidence record) and
  its G6 status is reported. The tool **never** records G6 and never emits an acceptance verdict:
  `overall` is always `NOT_A_VERDICT`. A zero exit means the worksheet (and, if supplied, the G6
  record) is complete, not that the gate passed.

Inputs and outputs stay separate: the private sampling plan and the private #572 preflight manifest
are never written into the run directory, and the run directory records validator **aliases** and
endpoint digests rather than host addresses. Raw response bodies may still carry node identities;
they are private until a human redacts and packs the public record.

Usage:
  python3 tools/live-gate.py sample --plan /secure/n584-plan.json --out target/n584-live \\
      [--interval 5] [--duration 900] [--timeout 5]
  python3 tools/live-gate.py check --run-dir target/n584-live --preflight /secure/preflight.json \\
      [--verdict /secure/g6-verdict.json] [--report target/n584-live/gate-report.json]

Exit codes:
  sample  0 every alias answered at least one deploy-status/settlement sample; 3 an alias never
          answered; 2 usage/IO error.
  check   0 all of G1–G5 PASS (the worksheet is complete); 1 any FAIL; 3 any INCOMPLETE and no
          FAIL; 2 usage/IO error.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import importlib.util
import json
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCHEMA = 1
TOOL = "live-gate"
ISSUE = 584
MAX_BODY_BYTES = 1 << 20
ALIAS_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
DIGEST_RE = re.compile(r"^[0-9a-f]{64}$")
SOURCE_RE = re.compile(r"^[0-9a-f]{40}$")
URL_RE = re.compile(r"https?://", re.IGNORECASE)
IP_RE = re.compile(r"\b\d{1,3}(?:\.\d{1,3}){3}\b")

ROUTES = (
    ("health", "/health"),
    ("status", "/api/status"),
    ("last_finalized", "/api/last-finalized-block"),
    ("finality", "/api/v1/finality-observation"),
    ("deploy_status", "/api/v1/deploy-status/{deploy_id}"),
    ("deploy_settlement", "/api/v1/deploy-settlement/{deploy_id}"),
)

OBSERVED_FIELDS = (
    "name",
    "version",
    "source_sha",
    "binary_sha256",
    "binary_path",
    "network_id",
    "shard_id",
    "genesis_hash",
    "genesis_raw_sha256",
    "bonds_sha256",
    "total_stake",
    "height",
    "finalized_height",
    "finalized_hash",
    "finalized_lag",
    "ready",
)
AGREEMENT_FIELDS = (
    "source_sha",
    "binary_sha256",
    "network_id",
    "shard_id",
    "genesis_hash",
    "genesis_raw_sha256",
    "bonds_sha256",
    "total_stake",
)
DIGEST_FIELDS = ("binary_sha256", "genesis_raw_sha256", "bonds_sha256")
OPERATOR_FIELDS = (
    "release_tag",
    "release_branch",
    "selected_source_sha",
    "selected_artifact_digest",
    "selected_binary_sha256",
    "window_start_utc",
    "window_end_utc",
    "operator",
    "observers",
    "independent_reviewer",
    "rollback_owner",
    "test_transfer",
    "rollback",
    "evidence_destination",
)
SETTLED = "finalizedSuccess"
UNSETTLED_STAGES = {"pooled", "notProcessed", "includedUnfinalized", "finalityUnknown"}
PROCESSED_TAGS = ("ProcessedWithSuccess", "ProcessedWithError")


def _load_controls():
    path = ROOT / "tests/admission_finality_stall_controls.py"
    spec = importlib.util.spec_from_file_location("finality_controls", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


controls = _load_controls()


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def parse_utc(value):
    if not isinstance(value, str) or not value:
        return None
    text = value[:-1] + "+00:00" if value.endswith("Z") else value
    try:
        stamp = datetime.fromisoformat(text)
    except ValueError:
        return None
    if stamp.tzinfo is None:
        stamp = stamp.replace(tzinfo=timezone.utc)
    return stamp.astimezone(timezone.utc)


def fail(message: str):
    print(f"FAIL  {message}", file=sys.stderr)
    raise SystemExit(2)


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def read_json(path: Path):
    try:
        return json.loads(path.read_text()), None
    except (OSError, ValueError) as error:
        return None, str(error)


def find_file(run_dir: Path, name: str):
    """A run directory may be raw, or a packed record whose files live under `raw/`."""
    for candidate in (run_dir / name, run_dir / "raw" / name):
        if candidate.is_file():
            return candidate
    return None


class Finding:
    """One criterion's worksheet: reasons are tagged, and FAIL outranks INCOMPLETE."""

    def __init__(self):
        self.reasons: list[tuple[str, str]] = []

    def fail(self, message: str) -> None:
        self.reasons.append(("FAIL", message))

    def incomplete(self, message: str) -> None:
        self.reasons.append(("INCOMPLETE", message))

    def status(self) -> str:
        if any(tag == "FAIL" for tag, _ in self.reasons):
            return "FAIL"
        if self.reasons:
            return "INCOMPLETE"
        return "PASS"

    def report(self) -> dict:
        return {
            "status": self.status(),
            "reasons": [f"{tag}: {message}" for tag, message in self.reasons],
        }


def present(value) -> bool:
    if value is None:
        return False
    if isinstance(value, (str, list, dict)) and not value:
        return False
    return True


def distinct(values):
    seen = set()
    for value in values:
        if isinstance(value, list):
            value = tuple(value)
        seen.add(value)
    return seen


# --------------------------------------------------------------------------------------------------
# sample


def endpoint_digest(base: str) -> str:
    parts = urllib.parse.urlsplit(base)
    path = parts.path.rstrip("/")
    normal = f"{parts.scheme.lower()}://{parts.netloc.lower()}{path}"
    return sha256_bytes(normal.encode())


def load_plan(path: Path, expected_observers: int = 4) -> dict:
    plan, error = read_json(path)
    if not isinstance(plan, dict):
        fail(f"sampling plan is not a JSON object: {error or path}")
    if plan.get("schema") != SCHEMA:
        fail(f"sampling plan schema {plan.get('schema')!r} is not {SCHEMA}")
    deploy_id = plan.get("deploy_id")
    if not isinstance(deploy_id, str) or not deploy_id.strip():
        fail("sampling plan has no deploy_id")
    validators = plan.get("validators")
    if not isinstance(validators, list) or len(validators) != expected_observers:
        fail(f"the sampling plan must name exactly {expected_observers} validators")
    aliases, digests = set(), set()
    for entry in validators:
        if not isinstance(entry, dict):
            fail("every validator entry must be an object")
        alias, base = entry.get("alias"), entry.get("base")
        if not isinstance(alias, str) or not ALIAS_RE.match(alias):
            fail(f"validator alias {alias!r} is not a safe alias")
        if alias in aliases:
            fail(f"duplicate validator alias {alias!r}")
        aliases.add(alias)
        if not isinstance(base, str):
            fail(f"validator {alias} has no base URL")
        parts = urllib.parse.urlsplit(base)
        if parts.scheme not in ("http", "https") or not parts.netloc:
            fail(f"validator {alias} base URL is not http(s): {base!r}")
        digest = endpoint_digest(base)
        if digest in digests:
            fail("two aliases resolve to the same endpoint: four names are not four validators")
        digests.add(digest)
    return plan


def http_get(base: str, path: str, timeout: float):
    url = base.rstrip("/") + path
    request = urllib.request.Request(
        url, method="GET", headers={"Accept": "application/json", "User-Agent": f"{TOOL}/1"}
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            raw = response.read(MAX_BODY_BYTES + 1)
            truncated = len(raw) > MAX_BODY_BYTES
            return response.status, raw[:MAX_BODY_BYTES], None, truncated
    except urllib.error.HTTPError as error:
        raw = error.read(MAX_BODY_BYTES + 1)
        truncated = len(raw) > MAX_BODY_BYTES
        return error.code, raw[:MAX_BODY_BYTES], None, truncated
    except (urllib.error.URLError, TimeoutError, OSError) as error:
        return None, b"", str(error), False


def body_fields(raw: bytes):
    text = raw.decode("utf-8", errors="replace")
    try:
        parsed = json.loads(text)
    except ValueError:
        parsed = None
    return text, parsed


def cmd_sample(args) -> int:
    plan_path = Path(args.plan)
    plan = load_plan(plan_path, getattr(args, "observers", 4))
    out = Path(args.out)
    if out.exists() and not out.is_dir():
        fail(f"{out} exists and is not a directory")
    if out.exists() and any(out.iterdir()):
        fail(f"{out} is not empty; a live run never overwrites a previous collection")
    out.mkdir(parents=True, exist_ok=True)

    interval = float(args.interval)
    duration = float(args.duration)
    timeout = float(args.timeout)
    if interval <= 0 or duration <= 0 or timeout <= 0:
        fail("interval, duration and timeout must be positive")

    deploy_id = plan["deploy_id"]
    validators = plan["validators"]
    started = utc_now()
    counts = {
        entry["alias"]: {name: {"events": 0, "ok": 0} for name, _ in ROUTES}
        for entry in validators
    }
    tick = 0
    stopped = False
    samples_path = out / "samples.jsonl.gz"
    try:
        with gzip.open(samples_path, "wt", encoding="utf-8") as stream:
            began = time.monotonic()
            while time.monotonic() - began < duration:
                tick_started = time.monotonic()
                for entry in validators:
                    alias, base = entry["alias"], entry["base"]
                    for name, template in ROUTES:
                        path = template.format(deploy_id=deploy_id)
                        started_at = utc_now()
                        monotonic_ns = time.monotonic_ns()
                        request_started = time.monotonic()
                        status, raw, error, truncated = http_get(base, path, timeout)
                        elapsed_ms = int((time.monotonic() - request_started) * 1000)
                        body_raw, body = body_fields(raw)
                        event = {
                            "schema": SCHEMA,
                            "tool": TOOL,
                            "event": "sample",
                            "tick": tick,
                            "alias": alias,
                            "route": name,
                            "path": path,
                            "utc": started_at,
                            "monotonic_ns": monotonic_ns,
                            "http_status": status,
                            "elapsed_ms": elapsed_ms,
                            "body_raw": body_raw,
                            "body_sha256": sha256_bytes(body_raw.encode()),
                            "body_truncated": truncated,
                            "body": body,
                            "error": error,
                        }
                        stream.write(json.dumps(event, sort_keys=True) + "\n")
                        counts[alias][name]["events"] += 1
                        if status is not None and 200 <= status < 300:
                            counts[alias][name]["ok"] += 1
                stream.flush()
                tick += 1
                remaining = interval - (time.monotonic() - tick_started)
                if remaining > 0:
                    time.sleep(remaining)
    except KeyboardInterrupt:
        stopped = True

    ended = utc_now()
    public_plan = {
        "schema": SCHEMA,
        "tool": TOOL,
        "deploy_id": deploy_id,
        "validators": [
            {"alias": entry["alias"], "endpoint_sha256": endpoint_digest(entry["base"])}
            for entry in validators
        ],
    }
    (out / "plan.public.json").write_text(json.dumps(public_plan, indent=2, sort_keys=True) + "\n")
    run = {
        "schema": SCHEMA,
        "tool": TOOL,
        "version": 1,
        "deploy_id": deploy_id,
        "started_utc": started,
        "ended_utc": ended,
        "interval_seconds": interval,
        "duration_seconds": duration,
        "ticks": tick,
        "interrupted": stopped,
        "validators": public_plan["validators"],
        "counts": counts,
        "execution_proven": False,
    }
    (out / "run.json").write_text(json.dumps(run, indent=2, sort_keys=True) + "\n")
    (out / "SHA256SUMS").write_text(
        "".join(
            f"{sha256_file(out / name)}  {name}\n"
            for name in ("samples.jsonl.gz", "plan.public.json", "run.json")
        )
    )

    missing = [
        alias
        for alias, routes in counts.items()
        if routes["deploy_status"]["ok"] + routes["deploy_settlement"]["ok"] == 0
    ]
    print(f"sampled {tick} tick(s) into {out}")
    for alias in sorted(counts):
        answered = sum(route["ok"] for route in counts[alias].values())
        total = sum(route["events"] for route in counts[alias].values())
        print(f"  {alias}: {answered}/{total} successful responses")
    if missing:
        print(f"  no deploy observation from: {', '.join(sorted(missing))}", file=sys.stderr)
        return 3
    return 0


# --------------------------------------------------------------------------------------------------
# check


def load_samples(run_dir: Path):
    path = find_file(run_dir, "samples.jsonl.gz")
    opener = gzip.open
    if path is None:
        path = find_file(run_dir, "samples.jsonl")
        opener = open
    if path is None:
        return [], "no samples.jsonl(.gz) in the run directory"
    events = []
    try:
        with opener(path, "rt", encoding="utf-8") as stream:
            for line in stream:
                line = line.strip()
                if line:
                    events.append(json.loads(line))
    except (OSError, ValueError) as error:
        return [], f"samples could not be read: {error}"
    return events, None


def settlement_events(events):
    return [
        event
        for event in events
        if event.get("route") == "deploy_settlement"
        and event.get("http_status") == 200
        and isinstance(event.get("body"), dict)
    ]


def finality_events(events):
    return [
        event
        for event in events
        if event.get("route") == "finality"
        and event.get("http_status") in (200, 503)
        and isinstance(event.get("body"), dict)
    ]


def execution_tag(body):
    if not isinstance(body, dict):
        return None
    for tag in PROCESSED_TAGS:
        if tag in body:
            return tag
    if "deployResult" in body:
        return "ProcessedWithSuccess"
    if "deployError" in body:
        return "ProcessedWithError"
    if "status" in body:
        return "NotProcessed"
    return None


def check_g1(preflight, expected_observers: int = 4) -> dict:
    finding = Finding()
    if not isinstance(preflight, dict):
        finding.incomplete("the #572 preflight manifest is absent or unreadable")
        return finding.report()
    if preflight.get("schema") != SCHEMA:
        finding.fail(f"preflight schema {preflight.get('schema')!r} is not {SCHEMA}")
        return finding.report()
    validators = preflight.get("validators")
    if not isinstance(validators, list):
        finding.incomplete("preflight has no validator records")
        return finding.report()
    if len(validators) < expected_observers:
        finding.incomplete(
            f"preflight has {len(validators)} validator record(s), not {expected_observers}")
    elif len(validators) > expected_observers:
        finding.fail(
            f"preflight has {len(validators)} validator records; the gate is {expected_observers}")

    names = [v.get("name") for v in validators if isinstance(v, dict) and present(v.get("name"))]
    boot_ids = [v.get("boot_id") for v in validators if isinstance(v, dict) and present(v.get("boot_id"))]
    if len(set(names)) != len(names):
        finding.fail("two validator records share a name")
    if not boot_ids:
        finding.incomplete("no boot IDs: four filenames do not prove four validators")
    elif len(set(boot_ids)) != len(boot_ids):
        finding.fail("two validator records share a boot ID")

    for index, record in enumerate(validators):
        label = (record.get("name") if isinstance(record, dict) else None) or f"validator[{index}]"
        if not isinstance(record, dict):
            finding.incomplete(f"{label} is not an object")
            continue
        for field in OBSERVED_FIELDS:
            if not present(record.get(field)):
                finding.incomplete(f"{label} has no {field}")
            elif field in DIGEST_FIELDS and not DIGEST_RE.match(str(record[field])):
                finding.fail(f"{label}.{field} is not a 64-hex digest")
        unset = record.get("unset")
        if isinstance(unset, list) and unset:
            finding.incomplete(f"{label} lists unset fields: {', '.join(map(str, unset))}")
        stake = record.get("total_stake")
        if isinstance(stake, int) and stake <= 0:
            finding.fail(f"{label} carries no bonded stake")

    for field in AGREEMENT_FIELDS:
        values = [v.get(field) for v in validators if isinstance(v, dict) and present(v.get(field))]
        if len(values) < len(validators):
            finding.incomplete(f"{field} is absent from {len(validators) - len(values)} record(s)")
        elif len(distinct(values)) > 1:
            finding.fail(f"validators disagree on {field}")

    for key in ("unset_observed", "unset_operator"):
        values = preflight.get(key)
        if isinstance(values, list) and values:
            finding.incomplete(f"preflight {key} lists {len(values)} missing field(s)")
    checks = preflight.get("checks") or {}
    if checks.get("ok") is not True:
        finding.incomplete("the collector's own checks.ok is not true (independent checks still apply)")
    if checks.get("validator_count") != expected_observers:
        finding.incomplete(
            f"the collector did not record {expected_observers} validators")

    operator = preflight.get("operator_fields")
    if not isinstance(operator, dict):
        finding.incomplete("preflight has no operator_fields")
        return finding.report()
    for field in OPERATOR_FIELDS:
        if not present(operator.get(field)):
            finding.incomplete(f"operator_fields.{field} is absent")
    selected_source = operator.get("selected_source_sha")
    selected_binary = operator.get("selected_binary_sha256")
    if present(selected_source) and not SOURCE_RE.match(str(selected_source)):
        finding.fail("operator_fields.selected_source_sha is not a commit")
    if present(selected_binary) and not DIGEST_RE.match(str(selected_binary)):
        finding.fail("operator_fields.selected_binary_sha256 is not a 64-hex digest")
    if present(selected_source) and present(selected_binary):
        for record in validators:
            if not isinstance(record, dict):
                continue
            if (record.get("source_sha") != selected_source
                    or record.get("binary_sha256") != selected_binary):
                finding.fail(
                    f"{record.get('name') or 'a validator'} is not the selected candidate")
                break
    start, end = parse_utc(operator.get("window_start_utc")), parse_utc(operator.get("window_end_utc"))
    if start is None or end is None:
        finding.incomplete("the approved UTC window is absent or unparseable")
    elif start >= end:
        finding.fail("the approved window does not end after it starts")
    if present(operator.get("operator")) and operator.get("operator") == operator.get("independent_reviewer"):
        finding.fail("the independent reviewer is the operator")
    if isinstance(operator.get("observers"), list) and not operator["observers"]:
        finding.incomplete("no observers are named")
    for field in ("test_transfer", "rollback"):
        if not isinstance(operator.get(field), dict) or not operator[field]:
            finding.incomplete(f"operator_fields.{field} is empty; approval is not a placeholder")
    return finding.report()


def check_g2(submission, run, events) -> dict:
    finding = Finding()
    if not isinstance(submission, dict):
        finding.incomplete("submission.json (the operator's accepted-deploy receipt) is absent")
    else:
        if submission.get("http_status") != 200:
            finding.incomplete(f"the submission receipt is HTTP {submission.get('http_status')!r}")
        payload = submission.get("payload_sha256")
        if not isinstance(payload, str) or not DIGEST_RE.match(payload):
            finding.incomplete("the submission receipt has no signed-payload digest")
        deploy_id = submission.get("deploy_id")
        if not present(deploy_id):
            finding.incomplete("the submission receipt has no deploy id")
        elif isinstance(run, dict) and run.get("deploy_id") != deploy_id:
            finding.fail("the submission deploy id differs from the sampled run")
        elif not present(submission.get("utc")) or parse_utc(submission.get("utc")) is None:
            finding.incomplete("the submission receipt has no parseable UTC time")

    if not events:
        finding.incomplete("there are no samples")
        return finding.report()
    aliases = sorted({event.get("alias") for event in events if event.get("alias")})
    for alias in aliases:
        routes = {(event.get("route"), event.get("http_status")) for event in events if event.get("alias") == alias}
        if not any(route == "deploy_status" and status == 200 for route, status in routes):
            finding.incomplete(f"{alias} has no successful deploy-status sample")
        if not any(route == "deploy_settlement" and status == 200 for route, status in routes):
            finding.incomplete(f"{alias} has no successful deploy-settlement sample")

    epoch = datetime.min.replace(tzinfo=timezone.utc)
    ordered = sorted(
        settlement_events(events),
        key=lambda event: (parse_utc(event.get("utc")) or epoch, event.get("tick", 0)),
    )
    stages = {}
    for event in ordered:
        stage = event["body"].get("stage")
        if isinstance(stage, str):
            stages.setdefault(stage, event)
    included = any(stage in ("includedUnfinalized", "finalityUnknown") for stage in stages)
    if not included:
        included = any(
            execution_tag(event.get("body")) in PROCESSED_TAGS
            for event in events
            if event.get("route") == "deploy_status"
        )
    if "pooled" not in stages:
        finding.incomplete("the pooled stage was not directly observed")
    if not included:
        finding.incomplete("no included/executed observation (includedUnfinalized, finalityUnknown or a Processed tag)")
    if SETTLED not in stages:
        finding.incomplete("the deploy was not observed finalized within the window")

    rank = {"pooled": 0, "includedUnfinalized": 1, "finalityUnknown": 1, SETTLED: 2}
    per_alias = {}
    for event in ordered:
        stage = event["body"].get("stage")
        if stage in rank:
            per_alias.setdefault(event.get("alias"), []).append((rank[stage], event.get("utc")))
    for alias, sequence in per_alias.items():
        for (before, _), (after, _) in zip(sequence, sequence[1:]):
            if after < before:
                finding.fail(f"{alias} reports the lifecycle out of order")
    for alias in aliases:
        stages_for_alias = {
            event["body"].get("stage")
            for event in settlement_events(events) if event.get("alias") == alias
        }
        included_for_alias = any(
            stage in ("includedUnfinalized", "finalityUnknown") for stage in stages_for_alias
        ) or any(
            execution_tag(event.get("body")) in PROCESSED_TAGS
            for event in events
            if event.get("alias") == alias and event.get("route") == "deploy_status"
        )
        if "pooled" not in stages_for_alias:
            finding.incomplete(f"{alias} never observed the pooled stage")
        if not included_for_alias:
            finding.incomplete(f"{alias} never observed an included/executed stage")
        if SETTLED not in stages_for_alias:
            finding.incomplete(f"{alias} never observed the finalized stage")

    identities = set()
    for event in ordered:
        body = event["body"]
        if body.get("blockHash") is not None and body.get("blockNumber") is not None:
            identities.add((body.get("blockHash"), body.get("blockNumber")))
    if len(identities) > 1:
        finding.fail(f"the deploy is reported in {len(identities)} different inclusion blocks")
    elif not identities:
        finding.incomplete("no inclusion block identity (hash and height) was observed")

    tags = {
        execution_tag(event.get("body"))
        for event in events
        if event.get("route") == "deploy_status" and execution_tag(event.get("body")) in PROCESSED_TAGS
    }
    if "ProcessedWithSuccess" in tags and "ProcessedWithError" in tags:
        finding.fail("the deploy is reported both succeeded and failed")
    return finding.report()


def check_g3(claims, events) -> dict:
    finding = Finding()
    if not isinstance(claims, dict):
        finding.incomplete("claims.json (the record's own settlement claim) is absent")
    else:
        settled = claims.get("settled_claim")
        if not isinstance(settled, bool):
            finding.incomplete("claims.json has no boolean settled_claim")
        elif settled:
            basis = claims.get("basis")
            if basis != SETTLED:
                finding.fail(f"the record claims settlement on {basis!r}, not on finalized inclusion")
            if not settlement_events(events) or not any(
                event["body"].get("stage") == SETTLED
                and event["body"].get("settlementConfirmed") is True
                and event["body"].get("finalized") is True
                for event in settlement_events(events)
            ):
                finding.fail("the record claims settlement without a finalizedSuccess observation")

    if not any(
        event["body"].get("finalized") is not True
        and event["body"].get("settlementConfirmed") is not True
        for event in settlement_events(events)
    ):
        finding.incomplete("no pre-finality not-finalized settlement sample")

    observations = finality_events(events)
    for event in observations:
        reason = controls.finality_observation_verdict(event.get("http_status"), event.get("body"))
        if reason:
            finding.fail(f"{event.get('alias')} finality-observation: {reason}")

    for event in settlement_events(events):
        body = event["body"]
        reason = controls.settlement_verdict(
            body.get("stage"), body.get("settlementConfirmed"), body.get("finalized")
        )
        if reason:
            finding.fail(f"{event.get('alias')} settlement: {reason}")
        if body.get("settlementConfirmed") is True:
            same_tick = [
                observation
                for observation in observations
                if observation.get("alias") == event.get("alias")
                and observation.get("tick") == event.get("tick")
            ]
            if not same_tick:
                finding.incomplete(
                    f"{event.get('alias')} confirms settlement without a same-tick finality observation"
                )
                continue
            finalized_height = same_tick[0]["body"].get("finalizedBlockNumber")
            inclusion_height = body.get("blockNumber")
            if not isinstance(finalized_height, int) or not isinstance(inclusion_height, int):
                finding.incomplete("settlement and finality do not carry comparable block heights")
            elif finalized_height < inclusion_height:
                finding.fail(
                    f"{event.get('alias')} confirms settlement at finalized height {finalized_height}, "
                    f"behind inclusion {inclusion_height}"
                )

    by_height = {}
    for event in observations:
        body = event["body"]
        height = body.get("finalizedBlockNumber")
        if isinstance(height, int) and present(body.get("finalizedBlockHash")):
            by_height.setdefault(height, set()).add(body["finalizedBlockHash"])
    for height, hashes in by_height.items():
        if len(hashes) > 1:
            finding.fail(f"validators disagree on the finalized hash at height {height}")

    if not settlement_events(events):
        finding.incomplete("no settlement sample to assess for false settlement")
    elif not observations:
        finding.incomplete("no finality-observation sample to agree with the settlement")
    return finding.report()


def check_g4(recovery, events) -> dict:
    finding = Finding()
    if not isinstance(recovery, dict):
        finding.incomplete("recovery.json (the chosen G4 arm) is absent; A4 read-only alone is not G4")
        return finding.report()
    required = (
        "arm",
        "start_utc",
        "end_utc",
        "same_deploy_finalized",
        "accepted_deploys_before",
        "accepted_deploys_after",
        "final_stages",
        "lost_deploys",
        "own_key_halt_observed",
        "self_replay_observed",
        "commands",
    )
    for field in required:
        if field not in recovery:
            finding.incomplete(f"recovery.json has no {field}")
    if any(field not in recovery for field in required):
        return finding.report()
    start, end = parse_utc(recovery.get("start_utc")), parse_utc(recovery.get("end_utc"))
    if start is None or end is None:
        finding.incomplete("the recovery window is absent or unparseable")
    elif start >= end:
        finding.fail("the recovery window does not end after it starts")
    if not isinstance(recovery.get("accepted_deploys_before"), list):
        finding.incomplete("accepted_deploys_before is not a list")
    if not isinstance(recovery.get("accepted_deploys_after"), list):
        finding.incomplete("accepted_deploys_after is not a list")
    if not isinstance(recovery.get("final_stages"), dict):
        finding.incomplete("final_stages is not a map from deploy id to terminal stage")
    if not isinstance(recovery.get("lost_deploys"), list):
        finding.incomplete("lost_deploys is not a list")
    if recovery.get("own_key_halt_observed") is True:
        finding.fail("an own-key halt fired on the healthy key")
    if recovery.get("self_replay_observed") is True:
        finding.fail("a spurious self-replay fired on the healthy key")
    if not recovery.get("commands"):
        finding.incomplete("no recovery commands were recorded")
    if recovery.get("lost_deploys"):
        finding.fail(f"the recovery lost accepted work: {recovery['lost_deploys']}")
    before = set(recovery.get("accepted_deploys_before") or [])
    after = set(recovery.get("accepted_deploys_after") or [])
    if before - after:
        finding.fail(f"accepted deploys disappeared across recovery: {sorted(before - after)}")
    final_stages = recovery.get("final_stages")
    if isinstance(final_stages, dict) and isinstance(recovery.get("accepted_deploys_before"), list):
        reason = controls.no_silent_loss_verdict(sorted(before), final_stages)
        if reason:
            finding.fail(f"recovery: {reason}")
    if recovery.get("same_deploy_finalized") is False:
        finding.fail("the same deploy did not finalize after recovery")
    elif recovery.get("same_deploy_finalized") is not True:
        finding.incomplete("same_deploy_finalized is not a boolean")
    finalized_after = [
        event
        for event in settlement_events(events)
        if end is not None
        and (parse_utc(event.get("utc")) or start or end) >= end
        and event["body"].get("stage") == SETTLED
    ]
    if not finalized_after:
        finding.incomplete("no finalizedSuccess sample after the recovery window")
    post = recovery.get("post_recovery_settlement")
    if isinstance(post, dict):
        reason = controls.settlement_verdict(
            post.get("stage"), post.get("settlementConfirmed"), post.get("finalized")
        )
        if reason:
            finding.fail(f"post-recovery settlement: {reason}")
    elif post is not None:
        finding.incomplete("post_recovery_settlement is not an object")
    return finding.report()


def verify_sums_json(path: Path) -> str | None:
    mapping, error = read_json(path)
    if not isinstance(mapping, dict):
        return f"{path.name} is unreadable: {error or 'not an object'}"
    for relative, digest in mapping.items():
        target = path.parent / relative
        if not target.is_file():
            return f"missing file listed in {path.name}: {relative}"
        if sha256_file(target) != digest:
            return f"digest mismatch in {path.name}: {relative}"
    return None


def verify_sums_text(path: Path):
    """Return (listed names, problem). A plain sums file must cover every evidence file present."""
    try:
        lines = path.read_text().splitlines()
    except OSError as error:
        return set(), f"{path.name} is unreadable: {error}"
    listed = set()
    for line in lines:
        line = line.strip()
        if not line:
            continue
        parts = line.split(None, 1)
        if len(parts) != 2 or not DIGEST_RE.match(parts[0]):
            return listed, f"malformed line in {path.name}: {line!r}"
        relative = parts[1].lstrip("*")
        listed.add(relative)
        target = path.parent / relative
        if not target.is_file():
            return listed, f"missing file listed in {path.name}: {relative}"
        if sha256_file(target) != parts[0]:
            return listed, f"digest mismatch in {path.name}: {relative}"
    return listed, None


def check_g5(run, events, run_dir: Path, expected_observers: int = 4) -> dict:
    finding = Finding()
    validators = run.get("validators") if isinstance(run, dict) else None
    aliases = []
    if not isinstance(validators, list) or len(validators) != expected_observers:
        finding.incomplete(f"run.json does not name {expected_observers} validators")
    else:
        aliases = [entry.get("alias") for entry in validators if isinstance(entry, dict)]
        digests = [entry.get("endpoint_sha256") for entry in validators if isinstance(entry, dict)]
        if len(set(aliases)) != len(aliases):
            finding.fail("two aliases share a name")
        if any(not isinstance(digest, str) or not DIGEST_RE.match(digest) for digest in digests):
            finding.incomplete("an endpoint digest is absent or malformed")
        elif len(set(digests)) != len(digests):
            finding.fail("two aliases resolve to the same endpoint digest")
    if isinstance(run, dict):
        if parse_utc(run.get("started_utc")) is None or parse_utc(run.get("ended_utc")) is None:
            finding.incomplete("run.json does not record parseable start/end UTC times")

    if not events:
        finding.incomplete("there are no samples")
        return finding.report()
    for alias in aliases:
        if not any(event.get("alias") == alias for event in events):
            finding.incomplete(f"{alias} contributed no observation")
        for route in ("deploy_status", "deploy_settlement", "finality"):
            if not any(
                event.get("alias") == alias and event.get("route") == route and event.get("http_status") == 200
                for event in events
            ):
                finding.incomplete(f"{alias} has no successful {route} observation")
    for event in events:
        stamp = event.get("utc")
        if parse_utc(stamp) is None:
            if stamp is None:
                finding.incomplete("a sample has no UTC time")
            else:
                finding.fail(f"a sample carries an unparseable UTC time: {stamp!r}")
        raw = event.get("body_raw")
        digest = event.get("body_sha256")
        if not isinstance(raw, str) or not isinstance(digest, str):
            finding.incomplete("a sample is missing its body or body digest")
        elif sha256_bytes(raw.encode()) != digest:
            finding.fail(f"a sample body does not match its digest ({event.get('alias')}, {event.get('route')})")

    manifest = find_file(run_dir, "manifest.json")
    if manifest is not None:
        document, error = read_json(manifest)
        if not isinstance(document, dict):
            finding.fail(f"manifest.json is unreadable: {error or 'not an object'}")
        elif document.get("execution_proven") is not False:
            finding.fail("manifest.json must record execution_proven=false")
        sums_json = find_file(run_dir, "SHA256SUMS.json")
        if sums_json is None:
            finding.incomplete("the packed record has no SHA256SUMS.json")
        else:
            problem = verify_sums_json(sums_json)
            if problem:
                finding.fail(problem)
    else:
        sums = find_file(run_dir, "SHA256SUMS")
        if sums is None:
            finding.incomplete("the run is not hashed; pack it with tools/evidence-pack.py")
        else:
            listed, problem = verify_sums_text(sums)
            if problem:
                finding.fail(problem)
            else:
                expected = {
                    name
                    for name in (
                        "samples.jsonl.gz",
                        "samples.jsonl",
                        "plan.public.json",
                        "run.json",
                        "submission.json",
                        "claims.json",
                        "recovery.json",
                    )
                    if (run_dir / name).is_file()
                }
                missing = expected - listed
                if missing:
                    finding.incomplete(
                        f"evidence file(s) not covered by SHA256SUMS: {', '.join(sorted(missing))}"
                    )

    for name in ("run.json", "plan.public.json"):
        path = find_file(run_dir, name)
        if path is None:
            continue
        try:
            text = path.read_text()
        except OSError:
            continue
        if URL_RE.search(text) or IP_RE.search(text):
            finding.fail(f"{name} leaks an endpoint, URL or address")
    for name in ("submission.json", "claims.json", "recovery.json"):
        path = find_file(run_dir, name)
        if path is None:
            continue
        try:
            text = path.read_text()
        except OSError:
            continue
        if URL_RE.search(text) or IP_RE.search(text):
            finding.incomplete(f"{name} carries an endpoint, URL or address; redact before publishing")
    return finding.report()


def check_g6(verdict, preflight) -> dict:
    finding = Finding()
    if not isinstance(verdict, dict):
        finding.incomplete("G6 is not recorded")
        return finding.report()
    required = ("reviewer", "relationship", "independent", "verdict",
                "evidence_sha256", "verdict_url", "signed_at", "evidence_record")
    for field in required:
        if not present(verdict.get(field)):
            finding.incomplete(f"verdict.json has no {field}")
    if any(not present(verdict.get(field)) for field in required):
        return finding.report()
    if verdict.get("independent") is not True:
        finding.fail("the reviewer must be independent")
    if verdict.get("verdict") not in ("PASS", "FAIL", "INCONCLUSIVE"):
        finding.fail("the verdict must be PASS, FAIL or INCONCLUSIVE")
    if not DIGEST_RE.match(str(verdict.get("evidence_sha256"))):
        finding.fail("evidence_sha256 is not a 64-hex digest")
    if not str(verdict.get("verdict_url")).startswith("https://"):
        finding.fail("verdict_url must be https")
    if parse_utc(verdict.get("signed_at")) is None:
        finding.fail("signed_at is not a UTC timestamp")
    operator = (preflight or {}).get("operator_fields") or {}
    if verdict.get("reviewer") == operator.get("operator"):
        finding.fail("the reviewer is the operator")
    named = operator.get("independent_reviewer")
    if named and verdict.get("reviewer") != named:
        finding.fail("the reviewer is not the named independent reviewer")
    if not Path(str(verdict.get("evidence_record"))).exists():
        finding.incomplete("the cited evidence record does not exist")
    return finding.report()


def evaluate(run_dir: Path, preflight_path: Path, verdict_path: Path | None = None,
             expected_observers: int = 4):
    preflight, preflight_error = read_json(preflight_path)
    if not isinstance(preflight, dict):
        preflight = None
    submission_path = find_file(run_dir, "submission.json")
    submission = read_json(submission_path)[0] if submission_path else None
    claims_path = find_file(run_dir, "claims.json")
    claims = read_json(claims_path)[0] if claims_path else None
    recovery_path = find_file(run_dir, "recovery.json")
    recovery = read_json(recovery_path)[0] if recovery_path else None
    run_path = find_file(run_dir, "run.json")
    run = read_json(run_path)[0] if run_path else None
    events, samples_error = load_samples(run_dir)
    verdict = None
    verdict_error = None
    if verdict_path is not None:
        verdict, verdict_error = read_json(verdict_path)
        if not isinstance(verdict, dict):
            verdict = None

    criteria = {
        "G1": check_g1(preflight, expected_observers),
        "G2": check_g2(submission, run, events),
        "G3": check_g3(claims, events),
        "G4": check_g4(recovery, events),
        "G5": check_g5(run, events, run_dir, expected_observers),
        "G6": check_g6(verdict, preflight),
    }
    details = {}
    if preflight_error and preflight is None:
        details["preflight_error"] = preflight_error
    if samples_error:
        details["samples_error"] = samples_error
    source_sha = None
    binary_sha256 = None
    if isinstance(preflight, dict):
        source = preflight.get("source")
        if isinstance(source, dict):
            source_sha = source.get("sha")
        records = preflight.get("validators")
        if isinstance(records, list) and records and isinstance(records[0], dict):
            binary_sha256 = records[0].get("binary_sha256")
    report = {
        "schema": SCHEMA,
        "tool": TOOL,
        "version": 1,
        "issue": ISSUE,
        "generated_at_utc": utc_now(),
        "run_dir": run_dir.name,
        "deploy_id": run.get("deploy_id") if isinstance(run, dict) else None,
        "source_sha": source_sha,
        "binary_sha256": binary_sha256,
        "criteria": criteria,
        "G6": {
            "status": criteria["G6"]["status"] if verdict_path is not None else "PENDING",
            "reviewer": verdict.get("reviewer") if isinstance(verdict, dict) else None,
            "verdict": verdict.get("verdict") if isinstance(verdict, dict) else None,
            "reasons": criteria["G6"]["reasons"] if verdict_path is not None else [],
            "rule": "recorded by the named independent reviewer against G1-G5",
        },
        "overall": "NOT_A_VERDICT",
        "acceptance": False,
        "execution_proven": False,
        "details": details,
    }
    return report


def cmd_check(args) -> int:
    run_dir = Path(args.run_dir)
    if not run_dir.is_dir():
        fail(f"no run directory at {run_dir}")
    preflight = Path(args.preflight)
    if not preflight.is_file():
        fail(f"no preflight manifest at {preflight} (pass the private #572 merge record)")
    verdict_path = Path(args.verdict) if getattr(args, "verdict", None) else None
    report = evaluate(run_dir, preflight, verdict_path, getattr(args, "observers", 4))
    destination = Path(args.report) if args.report else run_dir / "gate-report.json"
    destination.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")

    print(f"live-gate worksheet for #584 ({run_dir})")
    for name in ("G1", "G2", "G3", "G4", "G5"):
        criterion = report["criteria"][name]
        print(f"  {name}: {criterion['status']}")
        for reason in criterion["reasons"][:4]:
            print(f"      {reason}")
    g6 = report["G6"]
    if g6["status"] == "PENDING":
        print("  G6: PENDING - independent reviewer")
    else:
        print(f"  G6: {g6['status']} record")
        for reason in g6["reasons"][:4]:
            print(f"      {reason}")
    print("  NOT ACCEPTANCE: this tool cannot record #584's G6 verdict.")
    statuses = {
        criterion["status"] for name, criterion in report["criteria"].items()
        if name != "G6" or verdict_path is not None
    }
    if "FAIL" in statuses:
        return 1
    if "INCOMPLETE" in statuses:
        return 3
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    sample = sub.add_parser("sample", help="read-only four-validator sampling")
    sample.add_argument("--plan", required=True, help="private sampling plan (never committed)")
    sample.add_argument("--out", required=True, help="new run directory")
    sample.add_argument("--interval", type=float, default=5.0)
    sample.add_argument("--duration", type=float, default=900.0)
    sample.add_argument("--timeout", type=float, default=5.0)
    sample.add_argument("--observers", type=int, default=4,
                        help="exact validator count the plan must name (default 4)")
    sample.set_defaults(func=cmd_sample)

    check = sub.add_parser("check", help="G1-G5 worksheet; optional G6 record; never a verdict")
    check.add_argument("--run-dir", required=True)
    check.add_argument("--preflight", required=True, help="the private #572 merge record")
    check.add_argument("--verdict", help="optional typed G6 record (reviewer, verdict, evidence)")
    check.add_argument("--report", help="where to write gate-report.json")
    check.add_argument("--observers", type=int, default=4,
                       help="expected validator count (default 4)")
    check.set_defaults(func=cmd_check)

    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
