#!/usr/bin/env python3
"""release-pin-probe — collect the #559 §1 preflight pins for the four public validators.

The #487 live acceptance runbook (`spec/audit/runbooks/n487-live-postdeployment-20261108.md`) needs
one manifest that names, before any test deploy: the installed artifact and binary digest, the
validator identities and stakes, the genesis/bond identity, the observed deployment state, and the
operator-supplied window/people/transfer/rollback fields. Collecting that by hand from four hosts is
where the pin step goes wrong; this tool makes it one record per host and one merged manifest.

**What it is not.** A digest proves the bytes are the bytes, not that the run happened, and nothing
here is acceptance. The tool records what it observes and *lists every field it could not fill*:
`--check` fails on any missing §1 field, any disagreement between validators, or too few records.
The private operator JSON carries the selected-candidate pins, the deploy-distribution plan, the
window, people, transfer and rollback; it is supplied by the operator and never committed;
`--redact` removes endpoints and host identifiers from the public copy.

Usage:
  # on each validator host (the HTTP/admin endpoints must be reachable from it)
  tools/release-pin-probe.py observe --out target/release-pin --name validator-1 \
      --http http://127.0.0.1:40403 --admin http://127.0.0.1:40405 \
      [--local --service rnode --binary /usr/local/bin/rnode --data-dir /var/lib/rnode]

  # once the four records are copied into one directory
  tools/release-pin-probe.py merge --out target/release-pin --expect 4 --check \
      [--operator-json /secure/preflight-operator.json] [--redact]

Exit codes: 0 ok, 1 the check failed, 2 usage/IO error.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import subprocess
import sys
import time
from datetime import datetime, timezone
from email.utils import parsedate_to_datetime
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

SCHEMA = 1
USER_AGENT = "rholo-release-pin-probe/1"

# The §1 fields the probe can observe. A field the node does not expose is listed as unset, never
# guessed from an earlier run or the release tag.
OBSERVED_FIELDS = [
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
    "boot_id",
]

# The §1 fields only the operator can supply. They come from --operator-json and are private.
# This tuple is mirrored by `tools/live-gate.py::OPERATOR_FIELDS` and the preflight schema; a test
# keeps the three in step. `source_sha` is deliberately absent: the approved commit is
# `selected_source_sha`, and the manifest's `source` block is derived from it, so the collector
# cannot infer an approval from what it observed.
OPERATOR_FIELDS = [
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
    "deploy_relay",
]

REQUIRED_FIELDS = OBSERVED_FIELDS + OPERATOR_FIELDS

# Per-validator fields that must agree across the four (where present); a disagreement is a NO-GO.
AGREEMENT_FIELDS = ["source_sha", "network_id", "shard_id", "genesis_hash", "genesis_raw_sha256",
                    "binary_sha256"]

SOURCE_RE = re.compile(r"^[0-9a-f]{40}$")
DIGEST_RE = re.compile(r"^[0-9a-f]{64}$")
RELAY_MODES = ("relay", "four-ingress")


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def parse_utc(value):
    """A timezone-aware timestamp, or None. Naive times are refused (the window is UTC)."""
    if not isinstance(value, str) or not value:
        return None
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None
    return parsed if parsed.tzinfo is not None else None


def relay_problem(relay, record_names) -> str | None:
    """The typed deploy-distribution pin, or a reason it is not one.

    The relay is a *declared* plan, not a network measurement: `merge --check` proves the shape and
    that the named aliases are the records, and the operator's reachability check is recorded in
    `reachability_checked_utc` / `unreachable_aliases`.
    """
    if not isinstance(relay, dict):
        return "deploy_relay is not an object"
    mode = relay.get("mode")
    if mode not in RELAY_MODES:
        return f"deploy_relay.mode {mode!r} is not one of {RELAY_MODES}"
    aliases = relay.get("aliases")
    if not isinstance(aliases, list) or not all(isinstance(a, str) and a for a in aliases):
        return "deploy_relay.aliases must be a list of non-empty names"
    if len(set(aliases)) != len(aliases):
        return "deploy_relay.aliases contains duplicates"
    if record_names and set(aliases) != set(record_names):
        return "deploy_relay.aliases do not name the validator records"
    if mode == "relay":
        port = relay.get("destination_port")
        if not isinstance(port, int) or isinstance(port, bool) or not 1 <= port <= 65535:
            return "deploy_relay.destination_port must be a port for mode relay"
    elif relay.get("destination_port") is not None:
        return "deploy_relay.destination_port is only for mode relay"
    if parse_utc(relay.get("reachability_checked_utc")) is None:
        return "deploy_relay.reachability_checked_utc is absent or unparseable"
    unreachable = relay.get("unreachable_aliases")
    if not isinstance(unreachable, list):
        return "deploy_relay.unreachable_aliases must be a list"
    if unreachable:
        return f"deploy_relay.unreachable_aliases is not empty: {unreachable}"
    return None


def operator_problems(operator: dict, record_names: list[str]) -> list[str]:
    """Format and shape problems in the operator-supplied §1 fields."""
    problems = []
    selected_source = operator.get("selected_source_sha")
    if selected_source and not SOURCE_RE.match(str(selected_source)):
        problems.append("operator_fields.selected_source_sha is not a 40-hex commit")
    for field in ("selected_artifact_digest", "selected_binary_sha256"):
        value = operator.get(field)
        if value and not DIGEST_RE.match(str(value)):
            problems.append(f"operator_fields.{field} is not a 64-hex digest")
    relay = operator.get("deploy_relay")
    if relay is not None:
        problem = relay_problem(relay, record_names)
        if problem:
            problems.append(f"operator_fields.{problem}")
    return problems


def canonical_bonds(bonds) -> tuple[list[dict], str, int]:
    """Normalise a genesis bond list and hash it canonically.

    The canonical form sorts by the validator identity and keeps only the validator/stake pair, so
    two nodes that serialise the list differently still hash the same. Anything unparseable is kept
    as its JSON form, which still hashes deterministically — an odd bond list must not be silently
    dropped from the preflight.
    """
    normalised: list[dict] = []
    for bond in bonds or []:
        if not isinstance(bond, dict):
            normalised.append({"raw": bond})
            continue
        validator = bond.get("validator") or bond.get("validatorId") or bond.get("id")
        stake = bond.get("stake")
        if validator is None:
            normalised.append(dict(sorted(bond.items())))
        else:
            normalised.append({"validator": str(validator), "stake": stake})
    normalised.sort(key=lambda item: json.dumps(item, sort_keys=True, separators=(",", ":")))
    encoded = json.dumps(normalised, sort_keys=True, separators=(",", ":")).encode()
    total = 0
    for item in normalised:
        stake = item.get("stake")
        if isinstance(stake, int):
            total += stake
        elif isinstance(stake, str) and stake.isdigit():
            total += int(stake)
    return normalised, sha256_bytes(encoded), total


def dig(obj, *path, default=None):
    current = obj
    for key in path:
        if isinstance(current, dict):
            current = current.get(key)
        elif isinstance(current, list) and isinstance(key, int) and key < len(current):
            current = current[key]
        else:
            return default
        if current is None:
            return default
    return current


def fetch_json(base: str, path: str, timeout: float):
    """Return (parsed, error, server_date). A failed read is a recorded error, not a crash."""
    url = base.rstrip("/") + path
    request = Request(url, headers={"Accept": "application/json", "User-Agent": USER_AGENT})
    try:
        with urlopen(request, timeout=timeout) as response:
            body = response.read()
            server_date = response.headers.get("Date")
        return json.loads(body), None, server_date
    except (HTTPError, URLError, TimeoutError, ValueError, OSError) as exc:
        return None, f"{path}: {exc}", None


def fetch_bytes(base: str, path: str, timeout: float):
    url = base.rstrip("/") + path
    request = Request(url, headers={"User-Agent": USER_AGENT})
    try:
        with urlopen(request, timeout=timeout) as response:
            return response.read(), None
    except (HTTPError, URLError, TimeoutError, OSError) as exc:
        return None, f"{path}: {exc}"


def clock_offset_ms(server_date: str | None) -> int | None:
    if not server_date:
        return None
    try:
        server = parsedate_to_datetime(server_date).astimezone(timezone.utc)
    except (TypeError, ValueError):
        return None
    # The Date header has one-second resolution; this is a coarse skew indicator, not a measurement.
    return int((datetime.now(timezone.utc) - server).total_seconds() * 1000)


def first_block(backups):
    if isinstance(backups, list) and backups:
        return backups[0]
    if isinstance(backups, dict):
        for key in ("blockInfo", "block", "data"):
            if isinstance(backups.get(key), dict):
                return backups[key]
        if "blockHash" in backups:
            return backups
    return None


def exec_start_path(service: str) -> str | None:
    try:
        output = subprocess.run(
            ["systemctl", "show", "-p", "ExecStart", service],
            check=True,
            capture_output=True,
            text=True,
            timeout=10,
        ).stdout
    except (OSError, subprocess.SubprocessError):
        return None
    match = re.search(r"path=([^ ;]+)", output)
    return match.group(1) if match else None


def source_sha_from_version(version: str | None) -> str | None:
    """The build's self-reported commit, if the version string carries one."""
    if not version:
        return None
    match = re.search(r"\(([0-9a-f]{7,40})\)", version)
    return match.group(1) if match else None


def binary_version(path: str) -> tuple[str | None, str | None]:
    """Return (version line, source commit if the build self-reports one)."""
    try:
        output = subprocess.run(
            [path, "--version"], check=True, capture_output=True, text=True, timeout=10
        ).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return None, None
    version = output.splitlines()[0] if output else None
    return version, source_sha_from_version(version)


def local_observation(args) -> dict:
    """Host-local fields. Every failure is recorded; none is fatal."""
    observed: dict = {"local_errors": []}
    binary_path = args.binary or exec_start_path(args.service)
    if binary_path:
        observed["binary_path"] = binary_path
        candidate = Path(binary_path)
        if candidate.is_file():
            observed["binary_sha256"] = sha256_file(candidate)
        else:
            observed["local_errors"].append(f"binary path {binary_path} is not a file")
        version, commit = binary_version(binary_path)
        observed["version"] = version
        if commit:
            observed["source_sha"] = commit
    else:
        observed["local_errors"].append("could not resolve the rnode binary path")

    boot_id = Path("/proc/sys/kernel/random/boot_id")
    if boot_id.is_file():
        observed["boot_id"] = boot_id.read_text().strip()
        observed["boot_id_source"] = "kernel"
    observed["host"] = platform.node()
    try:
        observed["cpu_count"] = os.cpu_count()
    except OSError:
        pass
    meminfo = Path("/proc/meminfo")
    if meminfo.is_file():
        values = {}
        for line in meminfo.read_text().splitlines():
            key, _, rest = line.partition(":")
            if key in {"MemTotal", "MemAvailable"}:
                values[key] = rest.strip()
        observed["memory"] = values
    data_dir = Path(args.data_dir)
    if data_dir.is_dir():
        usage = os.statvfs(data_dir)
        observed["disk_free_bytes"] = usage.f_bavail * usage.f_frsize
        observed["disk_total_bytes"] = usage.f_blocks * usage.f_frsize
    genesis_dir = Path(args.genesis_dir)
    for name in ("bonds.txt", "wallets.txt"):
        path = genesis_dir / name
        if path.is_file():
            observed[f"genesis_{name}_sha256"] = sha256_file(path)
    config = Path(args.config_file) if args.config_file else None
    if config and config.is_file():
        observed["config_file"] = str(config)
        observed["config_sha256"] = sha256_file(config)
    return observed


def observe(args) -> int:
    record: dict = {
        "name": args.name,
        "observed_at_utc": utc_now(),
        "http": args.http,
        "admin": args.admin,
        "errors": [],
    }
    status, error, server_date = fetch_json(args.http, "/api/status", args.timeout)
    if status is None:
        record["errors"].append(error or "no /api/status response")
    else:
        record["version"] = dig(status, "version", "node")
        record["api_version"] = dig(status, "version", "api")
        # **A self-report, not artifact provenance.** The build embeds its commit in the version
        # string; recording it is useful, but the operator's release-artifact pin is what §1 means.
        if source_sha_from_version(record["version"]):
            record["source_sha"] = source_sha_from_version(record["version"])
            record["source_sha_source"] = "version-string"
        record["network_id"] = status.get("networkId")
        record["shard_id"] = status.get("shardId")
        record["address"] = status.get("address")
        record["height"] = status.get("latestBlockNumber")
        record["autopropose"] = status.get("autopropose")
        record["propose_on_deploy"] = status.get("proposeOnDeploy")
        record["dev_mode"] = status.get("devMode")
        record["degraded"] = status.get("degraded")
        record["accepts_deploys"] = status.get("acceptsDeploys")
        record["pool"] = status.get("pool")
        record["clock_offset_ms"] = clock_offset_ms(server_date)

    health, error, _ = fetch_json(args.http, "/health", args.timeout)
    if health is None:
        record["errors"].append(error or "no /health response")
    else:
        record["ready"] = health.get("ready")
        record["ready_reasons"] = health.get("readyReasons")
        record["health_height"] = health.get("height")
        record["max_observed_height"] = health.get("maxObservedHeight")
        record["finalized_height"] = health.get("finalizedHeight")
        record["finalized_lag"] = health.get("finalizedLag")
        record["finalized_height_error"] = health.get("finalizedHeightError")
        record["last_block_age_ms"] = health.get("lastBlockAgeMs")
        record["last_finality_age_ms"] = health.get("lastFinalityAgeMs")
        if record.get("height") is None:
            record["height"] = health.get("height")

    last_finalized, error, _ = fetch_json(args.http, "/api/last-finalized-block", args.timeout)
    if last_finalized is None:
        record["errors"].append(error or "no /api/last-finalized-block response")
    else:
        block = first_block(last_finalized)
        if block:
            record["finalized_hash"] = block.get("blockHash")
            if record.get("finalized_height") is None:
                record["finalized_height"] = block.get("blockNumber")

    genesis, error, _ = fetch_json(args.http, "/api/blocks/0/0", args.timeout)
    genesis_block = first_block(genesis)
    if genesis_block is None:
        record["errors"].append(error or "no genesis block at /api/blocks/0/0")
    else:
        record["genesis_hash"] = genesis_block.get("blockHash")
        bonds, bonds_sha, total = canonical_bonds(genesis_block.get("bonds"))
        record["bonds"] = bonds
        record["bonds_sha256"] = bonds_sha
        record["total_stake"] = total
        if record["genesis_hash"]:
            raw, error = fetch_bytes(
                args.http, f"/api/block/{record['genesis_hash']}/raw", args.timeout
            )
            if raw is None:
                record["errors"].append(error or "no raw genesis block")
            else:
                record["genesis_raw_sha256"] = sha256_bytes(raw)
                record["genesis_raw_bytes"] = len(raw)

    if args.admin:
        operator, error, _ = fetch_json(
            args.admin, "/api/v1/status/operator", args.timeout
        )
        if operator is None:
            record["errors"].append(error or "no operator status response")
        else:
            record["operator"] = {
                "height": operator.get("latestBlockNumber"),
                "peers": operator.get("peers"),
                "orphan_bodies": operator.get("orphanBodies"),
                "finality_stalled": operator.get("finalityStalled"),
                "resources": operator.get("resources"),
                "reset_marker": operator.get("resetMarker"),
            }

    if args.local:
        record["local"] = local_observation(args)
        for key in ("version", "source_sha", "binary_sha256", "binary_path", "boot_id",
                    "boot_id_source"):
            if record.get(key) is None and record["local"].get(key) is not None:
                record[key] = record["local"][key]

    record["unset"] = [field for field in OBSERVED_FIELDS if record.get(field) in (None, "")]
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    path = out / f"validator-{args.name}.json"
    path.write_text(json.dumps(record, indent=2, sort_keys=True) + "\n")
    print(f"wrote {path}")
    if record["errors"]:
        print(f"  {len(record['errors'])} read error(s); see the record", file=sys.stderr)
    return 0


def load_records(out: Path) -> list[dict]:
    records = []
    for path in sorted(out.glob("validator-*.json")):
        try:
            records.append(json.loads(path.read_text()))
        except (OSError, ValueError) as exc:
            raise SystemExit(f"cannot read {path}: {exc}")
    return records


def build_manifest(records: list[dict], operator: dict | None) -> dict:
    operator = operator or {}
    manifest = {
        "schema": SCHEMA,
        "generated_at_utc": utc_now(),
        "source": None,
        "operator_fields": {},
        "validators": records,
        "unset_observed": [],
        "unset_operator": [],
        "checks": {},
        "errors": [],
    }
    for field in OPERATOR_FIELDS:
        value = operator.get(field)
        if value not in (None, "", [], {}):
            manifest["operator_fields"][field] = value
        else:
            manifest["unset_operator"].append(field)
    for record in records:
        if not isinstance(record, dict):
            manifest["errors"].append("a validator record is not an object")
            continue
        for field in record.get("unset", []):
            entry = f"{record.get('name')}:{field}"
            if entry not in manifest["unset_observed"]:
                manifest["unset_observed"].append(entry)

    record_names = [
        record.get("name") for record in records
        if isinstance(record, dict) and record.get("name")
    ]
    operator_errors = operator_problems(operator, record_names)
    manifest["errors"].extend(operator_errors)
    if operator.get("selected_source_sha"):
        manifest["source"] = {
            "sha": operator.get("selected_source_sha"),
            "tag": operator.get("release_tag"),
            "branch": operator.get("release_branch"),
        }

    checks = manifest["checks"]
    checks["validator_count"] = len(records)
    for field in AGREEMENT_FIELDS:
        values = {
            record.get(field)
            for record in records
            if isinstance(record, dict) and record.get(field) not in (None, "")
        }
        checks[f"{field}_agreement"] = len(values) <= 1
        if len(values) > 1:
            manifest["errors"].append(f"{field} disagrees across validators: {sorted(values)}")
    boot_ids = [
        record.get("boot_id") for record in records
        if isinstance(record, dict) and record.get("boot_id")
    ]
    manifest["synthetic_boot_ids"] = any(
        isinstance(record, dict) and record.get("boot_id_source") not in (None, "kernel")
        for record in records
    )
    selected_source = operator.get("selected_source_sha")
    selected_binary = operator.get("selected_binary_sha256")
    checks["operator_fields_typed"] = not operator_errors
    checks["distinct_names"] = (
        len(record_names) == len(records) and len(set(record_names)) == len(record_names)
    )
    checks["distinct_boot_ids"] = (
        len(boot_ids) == len(records) and len(set(boot_ids)) == len(boot_ids)
    )
    checks["selected_candidate_matches_validators"] = all(
        isinstance(record, dict)
        and (not selected_source or record.get("source_sha") == selected_source)
        and (not selected_binary or record.get("binary_sha256") == selected_binary)
        for record in records
    )
    checks["all_observed_fields"] = not manifest["unset_observed"]
    checks["all_operator_fields"] = not manifest["unset_operator"]
    checks["ok"] = (
        checks["all_observed_fields"]
        and checks["all_operator_fields"]
        and checks["operator_fields_typed"]
        and checks["distinct_names"]
        and checks["distinct_boot_ids"]
        and checks["selected_candidate_matches_validators"]
        and all(checks.get(f"{field}_agreement", True) for field in AGREEMENT_FIELDS)
        and checks["validator_count"] > 0
    )
    return manifest


def redact_manifest(manifest: dict) -> dict:
    redacted = json.loads(json.dumps(manifest))
    for record in redacted.get("validators", []):
        for field in ("http", "admin"):
            if record.get(field):
                record[field] = "<redacted>"
        local = record.get("local")
        if isinstance(local, dict) and local.get("host"):
            local["host"] = "<redacted>"
    for field in ("operator", "observers", "rollback_owner", "independent_reviewer"):
        if field in redacted.get("operator_fields", {}):
            redacted["operator_fields"][field] = "<redacted>"
    if isinstance(redacted.get("operator_fields", {}).get("test_transfer"), dict):
        redacted["operator_fields"]["test_transfer"] = "<redacted>"
    return redacted


def merge(args) -> int:
    out = Path(args.out)
    records = load_records(out)
    operator = None
    if args.operator_json:
        try:
            operator = json.loads(Path(args.operator_json).read_text())
        except (OSError, ValueError) as exc:
            raise SystemExit(f"cannot read the operator JSON: {exc}")
    manifest = build_manifest(records, operator)
    if len(records) != args.expect:
        manifest["errors"].append(
            f"expected {args.expect} validator record(s), found {len(records)}"
        )
        manifest["checks"]["ok"] = False
    if args.redact:
        manifest = redact_manifest(manifest)
    destination = out / ("manifest-redacted.json" if args.redact else "manifest.json")
    destination.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print(f"wrote {destination}")
    print(
        "validators={count} observed_unset={observed} operator_unset={operator} ok={ok}".format(
            count=len(records),
            observed=len(manifest["unset_observed"]),
            operator=len(manifest["unset_operator"]),
            ok=manifest["checks"]["ok"],
        )
    )
    for error in manifest["errors"]:
        print(f"  {error}", file=sys.stderr)
    if args.check and not manifest["checks"]["ok"]:
        print("check failed: the preflight is not complete (NO-GO)", file=sys.stderr)
        return 1
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)

    observe_parser = sub.add_parser("observe", help="record one validator's preflight fields")
    observe_parser.add_argument("--out", required=True, help="directory for validator-<name>.json")
    observe_parser.add_argument("--name", required=True, help="stable validator name/alias")
    observe_parser.add_argument("--http", required=True, help="public HTTP base URL")
    observe_parser.add_argument("--admin", help="admin HTTP base URL (operator status)")
    observe_parser.add_argument("--timeout", type=float, default=10.0)
    observe_parser.add_argument("--local", action="store_true", help="also read host-local pins")
    observe_parser.add_argument("--service", default="rnode", help="systemd unit for ExecStart")
    observe_parser.add_argument("--binary", help="rnode binary path (overrides ExecStart)")
    observe_parser.add_argument("--data-dir", default="/var/lib/rnode")
    observe_parser.add_argument("--genesis-dir", default="/var/lib/rnode/genesis")
    observe_parser.add_argument("--config-file")
    observe_parser.set_defaults(func=observe)

    merge_parser = sub.add_parser("merge", help="merge validator records into the #559 manifest")
    merge_parser.add_argument("--out", required=True, help="directory holding validator-*.json")
    merge_parser.add_argument("--expect", type=int, default=4)
    merge_parser.add_argument("--check", action="store_true", help="exit 1 unless the preflight is complete")
    merge_parser.add_argument("--operator-json", help="private operator-supplied §1 fields")
    merge_parser.add_argument("--redact", action="store_true", help="write manifest-redacted.json")
    merge_parser.set_defaults(func=merge)
    return parser


def main(argv: list[str]) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
