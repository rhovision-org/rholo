#!/usr/bin/env python3
"""Assemble the G428-M4 finality-campaign record from the campaign's own artefacts.

The declaration is the operator's claim: the campaign name, the selected candidate, the declared
hosts, the bootstrap, the rerun commands, and each workload's validator-id-to-node map, approved
root and artefact paths. The measurements come from the partition/rejoin harness, the M1 capacity
collector, the restart arm, the read-only root and host-loss observers, and the chosen recovery
arm. This tool never calls a node or Docker: the run may be gone, and a re-read would be a
different observation.

It refuses rather than fabricating. A missing or wrong-typed artefact, a summary that disagrees
with the raw rounds, a candidate or approved root mismatch, a below-tier capacity record, a lost
or spurious recovery, or a validator set that does not match the declaration exits non-zero and
leaves no output file.

Usage:
  python3 tools/m4-campaign.py collect --declaration campaign.json --out <run>/observations.json
  python3 tools/m4-campaign.py observe-root --approved-root HEX --phase before \
      --validator v1=https://host-a --validator v2=https://host-b --out root-before.json
  python3 tools/m4-campaign.py observe-host-loss --approved-root HEX \
      --lost host-d=https://host-d --survivor v1=https://host-a --out host-loss.json
"""
from __future__ import annotations

import argparse
import json
import os
import re
import sys
import urllib.error
import urllib.request
from pathlib import Path
from urllib.parse import urlparse

SCHEMA = 1
GATE = "G428-M4"
HEX40 = re.compile(r"^[0-9a-f]{40}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
HARNESS = "consensus-partition-rejoin.py"
AGGREGATE = "PASS_BOUNDED_WORKLOAD_PARTITION_REJOIN"
COUNTS = {"n4": 4, "n8": 8}
RECOVERY_FIELDS = ("arm", "start_utc", "end_utc", "same_deploy_finalized",
                   "accepted_deploys_before", "accepted_deploys_after", "final_stages",
                   "lost_deploys", "own_key_halt_observed", "self_replay_observed",
                   "commands")


def refuse(message: str) -> None:
    raise SystemExit(f"m4-campaign: {message}")


def string(value: object) -> bool:
    return isinstance(value, str) and bool(value.strip())


def positive_int(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value > 0


def nonneg(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


def number(value: object) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def load(path: Path, what: str) -> dict:
    try:
        document = json.loads(path.read_text())
    except OSError as error:
        refuse(f"{what} cannot be read: {error}")
    except json.JSONDecodeError as error:
        refuse(f"{what} is not JSON: {error}")
    if not isinstance(document, dict):
        refuse(f"{what} is not an object")
    return document


def resolve(base: Path, text: object, where: str) -> Path:
    if not string(text):
        refuse(f"{where} is not a path")
    path = Path(text)
    return (path if path.is_absolute() else base / path).resolve()


def read_declaration(path: Path) -> dict:
    declaration = load(path, "the declaration")
    if declaration.get("schema") != SCHEMA:
        refuse(f"declaration schema {declaration.get('schema')!r} is not {SCHEMA}")
    campaign = declaration.get("campaign")
    if not isinstance(campaign, dict):
        refuse("declaration.campaign is absent")
    if not string(campaign.get("name")):
        refuse("declaration.campaign.name is absent")
    if campaign.get("locality") != "multi-host":
        refuse("declaration.campaign.locality is not 'multi-host'")
    if not string(campaign.get("bootstrap")):
        refuse("declaration.campaign.bootstrap is absent")
    selected = campaign.get("selected")
    if not isinstance(selected, dict):
        refuse("declaration.campaign.selected is absent")
    if not (isinstance(selected.get("source_sha"), str)
            and HEX40.fullmatch(selected["source_sha"])):
        refuse("declaration.campaign.selected.source_sha is not 40-hex")
    if not (isinstance(selected.get("binary_sha256"), str)
            and HEX64.fullmatch(selected["binary_sha256"])):
        refuse("declaration.campaign.selected.binary_sha256 is not 64-hex")
    hosts = campaign.get("hosts")
    if not isinstance(hosts, list) or not hosts or not all(string(host) for host in hosts):
        refuse("declaration.campaign.hosts is absent or carries an empty host")
    if len(set(hosts)) != len(hosts):
        refuse("declaration.campaign.hosts lists a host more than once")
    rerun = campaign.get("rerun")
    if not isinstance(rerun, dict):
        refuse("declaration.campaign.rerun is absent")
    for key, count in COUNTS.items():
        command = rerun.get(key)
        if not string(command) or HARNESS not in command \
                or f"--validators {count}" not in command:
            refuse(f"declaration.campaign.rerun.{key} does not name the harness "
                   f"and --validators {count}")

    workloads = declaration.get("workloads")
    if not isinstance(workloads, dict) or set(workloads) != set(COUNTS):
        refuse("declaration.workloads must name exactly n4 and n8")
    for key, count in COUNTS.items():
        workload = workloads[key]
        if not isinstance(workload, dict):
            refuse(f"declaration.workloads.{key} is not an object")
        if not string(workload.get("dir")):
            refuse(f"declaration.workloads.{key}.dir is not a path")
        for field in ("capacity", "restart", "root_before", "root_after",
                      "host_loss", "recovery"):
            if not string(workload.get(field)):
                refuse(f"declaration.workloads.{key}.{field} is not a path")
        approved = workload.get("approved_root")
        if not (isinstance(approved, str) and HEX64.fullmatch(approved)):
            refuse(f"declaration.workloads.{key}.approved_root is not 64-hex")
        nodes = workload.get("nodes")
        if not isinstance(nodes, list) or len(nodes) != count \
                or not all(string(node) for node in nodes):
            refuse(f"declaration.workloads.{key}.nodes is not {count} validator ids")
        if len(set(nodes)) != count:
            refuse(f"declaration.workloads.{key}.nodes lists an id more than once")
        if nodes[0] != campaign["bootstrap"]:
            refuse(f"declaration.workloads.{key}.nodes[0] is not the bootstrap")
    return declaration


def capacity_record(path: Path) -> dict:
    document = load(path, f"the capacity record {path}")
    record = document.get("record", document)
    if not isinstance(record, dict):
        refuse(f"the capacity record {path} is not an object")
    if record.get("kind") != "supported-tier":
        refuse(f"the capacity record {path} is not a supported-tier measurement")
    validators = record.get("validators")
    if not isinstance(validators, list) or not validators:
        refuse(f"the capacity record {path} has no validators")
    return record


def capacity_ids(record: dict) -> dict[str, dict]:
    mapping = {}
    for entry in record["validators"]:
        if not isinstance(entry, dict) or not string(entry.get("id")):
            refuse("the capacity record has a validator without an id")
        if not string(entry.get("host")):
            refuse(f"the capacity record's validator {entry['id']!r} has no host")
        mapping[entry["id"]] = entry
    return mapping


def check_root(document: dict, phase: str, approved: str,
               expected_ids: set[str], where: str) -> None:
    if document.get("schema") != SCHEMA or document.get("gate_id") != GATE:
        refuse(f"{where} is not a G428-M4 schema-1 observation")
    if document.get("phase") != phase:
        refuse(f"{where}.phase is {document.get('phase')!r}, not {phase!r}")
    if document.get("approved_root") != approved:
        refuse(f"{where}.approved_root is not the declared approved root")
    rows = document.get("per_validator")
    if not isinstance(rows, list):
        refuse(f"{where}.per_validator is absent")
    seen = set()
    for row in rows:
        if not isinstance(row, dict) or not string(row.get("id")):
            refuse(f"{where} has a row without a validator id")
        ident = row["id"]
        if ident not in expected_ids:
            refuse(f"{where} names {ident!r}, which the workload does not declare")
        if ident in seen:
            refuse(f"{where} lists {ident!r} more than once")
        seen.add(ident)
        if row.get("finalized") is not True:
            refuse(f"{where}: validator {ident!r} did not finalize the approved root")
    if seen != expected_ids:
        refuse(f"{where} does not cover every declared validator")


def check_host_loss(document: dict, approved: str, declared_hosts: set[str],
                    id_host: dict[str, str], tolerance: int,
                    where: str) -> None:
    if document.get("schema") != SCHEMA or document.get("gate_id") != GATE:
        refuse(f"{where} is not a G428-M4 schema-1 observation")
    if document.get("approved_root") != approved:
        refuse(f"{where}.approved_root is not the declared approved root")
    lost = document.get("lost_hosts")
    if not isinstance(lost, list) or not lost or not all(string(host) for host in lost):
        refuse(f"{where}.lost_hosts is absent")
    if len(lost) != tolerance:
        refuse(f"{where} lost {len(lost)} host(s), not the declared tolerance {tolerance}")
    for host in lost:
        if host not in declared_hosts:
            refuse(f"{where} lost host {host!r} is not declared")
    survivors = {ident for ident, host in id_host.items() if host not in set(lost)}
    listed = document.get("survivors")
    if not isinstance(listed, list) or {item for item in listed if string(item)} != survivors:
        refuse(f"{where}.survivors is not the surviving validator set")
    rows = document.get("per_validator")
    if not isinstance(rows, list):
        refuse(f"{where}.per_validator is absent")
    seen = set()
    for row in rows:
        if not isinstance(row, dict) or not string(row.get("id")):
            refuse(f"{where} has a row without a validator id")
        ident = row["id"]
        if ident not in survivors:
            refuse(f"{where} names a non-survivor")
        if ident in seen:
            refuse(f"{where} lists {ident!r} more than once")
        seen.add(ident)
        if row.get("finalized_anchor") is not True:
            refuse(f"{where}: survivor {ident!r} did not finalize the anchor")
        if row.get("root") != approved:
            refuse(f"{where}: survivor {ident!r} reports a different root")
    if seen != survivors:
        refuse(f"{where} does not cover every survivor")
    if document.get("root_before") != approved or document.get("root_after") != approved:
        refuse(f"{where} does not preserve the approved root")
    if document.get("retained") is not True:
        refuse(f"{where} did not retain the anchor")


def read_restart(path: Path) -> dict:
    document = load(path, f"the restart record {path}")
    if not string(document.get("container")):
        refuse(f"the restart record {path} names no container")
    seconds = document.get("replay_seconds")
    if not number(seconds) or seconds <= 0:
        refuse(f"the restart record {path} has no readable replay time")
    for field in ("replay_height", "recovered_height"):
        if not nonneg(document.get(field)):
            refuse(f"the restart record {path} has no readable {field}")
    if document["recovered_height"] < document["replay_height"]:
        refuse(f"the restart record {path} recovered below its replay height")
    if document.get("anchor_still_present") is not True:
        refuse(f"the restart record {path} did not retain the anchor")
    if document.get("manual_restart") is not True:
        refuse(f"the restart record {path} is not a manual restart arm")
    return document


def read_recovery(path: Path) -> dict:
    document = load(path, f"the recovery record {path}")
    for field in RECOVERY_FIELDS:
        if field not in document:
            refuse(f"the recovery record {path} has no {field}")
    for field in ("accepted_deploys_before", "accepted_deploys_after",
                  "lost_deploys", "commands"):
        if not isinstance(document.get(field), list):
            refuse(f"the recovery record {path} has no {field} list")
    if not document["accepted_deploys_before"]:
        refuse(f"the recovery record {path} accepted no work to recover")
    if not document["commands"]:
        refuse(f"the recovery record {path} records no recovery command")
    if document.get("same_deploy_finalized") is not True:
        refuse(f"the recovery record {path} did not finalize the same deploy")
    if document.get("lost_deploys"):
        refuse(f"the recovery record {path} lost accepted work: {document['lost_deploys']}")
    if set(document["accepted_deploys_before"]) - set(document["accepted_deploys_after"]):
        refuse(f"the recovery record {path} lost accepted deploys")
    if document.get("self_replay_observed") is not False:
        refuse(f"the recovery record {path} reports a spurious self-replay")
    if document.get("own_key_halt_observed") is not False:
        refuse(f"the recovery record {path} reports an own-key halt")
    return document


def collect(declaration_path: Path, out: Path) -> dict:
    declaration = read_declaration(declaration_path)
    base = declaration_path.parent
    campaign = declaration["campaign"]
    selected = campaign["selected"]
    hosts = list(campaign["hosts"])
    workloads = {}
    for key, count in COUNTS.items():
        spec = declaration["workloads"][key]
        nodes = list(spec["nodes"])
        approved = spec["approved_root"]
        where = f"workloads.{key}"

        capacity = capacity_record(resolve(base, spec["capacity"], f"{where}.capacity"))
        by_id = capacity_ids(capacity)
        if set(by_id) != set(nodes):
            refuse(f"{where}: the capacity ids do not match the declared nodes")
        declared = capacity.get("declared") if isinstance(capacity.get("declared"), dict) else {}
        tolerance = declared.get("host_failure_tolerance")
        if not positive_int(tolerance):
            refuse(f"{where}: the capacity record declares no host failure tolerance")
        id_host = {ident: entry["host"] for ident, entry in by_id.items()}
        if not all(host in hosts for host in id_host.values()):
            refuse(f"{where}: a capacity host is not in campaign.hosts")
        container_to_id = {entry.get("container"): ident
                           for ident, entry in by_id.items() if string(entry.get("container"))}

        run_dir = resolve(base, spec["dir"], f"{where}.dir")
        harness_campaign = load(run_dir / "campaign.json", f"{where} campaign.json")
        if harness_campaign.get("gate_id") != "n8-partition-rejoin":
            refuse(f"{where}: campaign.json is not the partition/rejoin aggregate")
        if harness_campaign.get("aggregate") != AGGREGATE:
            refuse(f"{where}: campaign.json aggregate is {harness_campaign.get('aggregate')!r}")
        if harness_campaign.get("validators") != count:
            refuse(f"{where}: campaign.json names {harness_campaign.get('validators')!r} validators")
        if harness_campaign.get("source_sha") != selected["source_sha"]:
            refuse(f"{where}: campaign.json source_sha is not the selected source")
        if harness_campaign.get("binary_sha256") != selected["binary_sha256"]:
            refuse(f"{where}: campaign.json binary is not the selected binary")

        partition_result = load(run_dir / "partition-result.json", f"{where} partition-result.json")
        if partition_result.get("result") != AGGREGATE:
            refuse(f"{where}: partition-result.json is not a PASS aggregate")
        rounds = partition_result.get("rounds")
        if not isinstance(rounds, list) or len(rounds) < 2:
            refuse(f"{where}: partition-result.json has fewer than two cycles")
        if partition_result.get("validators") != count:
            refuse(f"{where}: partition-result.json names a different validator count")

        workload_result = load(run_dir / "workload-result.json", f"{where} workload-result.json")
        if workload_result.get("result") != "pass":
            refuse(f"{where}: workload-result.json is not a pass")
        if workload_result.get("validators") != count:
            refuse(f"{where}: workload-result.json names a different validator count")
        target = workload_result.get("target_height")
        if not nonneg(target):
            refuse(f"{where}: workload-result.json has no finalized target height")

        typed = load(run_dir / "partition-rejoin.json", f"{where} partition-rejoin.json")
        if typed.get("schema") != SCHEMA or typed.get("gate_id") != GATE:
            refuse(f"{where}: partition-rejoin.json is not a G428-M4 schema-1 record")
        cycles = typed.get("cycles")
        if not isinstance(cycles, list) or len(cycles) != len(rounds):
            refuse(f"{where}: partition-rejoin.json does not match the rounds")

        root_before = load(resolve(base, spec["root_before"], f"{where}.root_before"),
                           f"{where} root_before")
        check_root(root_before, "before", approved, set(nodes), f"{where}.root_before")

        host_loss = load(resolve(base, spec["host_loss"], f"{where}.host_loss"),
                         f"{where} host_loss")
        check_host_loss(host_loss, approved, set(hosts), id_host, tolerance,
                        f"{where}.host_loss")
        lost = set(host_loss["lost_hosts"])
        survivors = [ident for ident in nodes if id_host[ident] not in lost]

        root_after = load(resolve(base, spec["root_after"], f"{where}.root_after"),
                          f"{where} root_after")
        check_root(root_after, "after", approved, set(survivors), f"{where}.root_after")

        restart = read_restart(resolve(base, spec["restart"], f"{where}.restart"))
        restart_id = container_to_id.get(restart["container"])
        if restart_id is None:
            refuse(f"{where}: the restart container is not a declared validator")

        recovery = read_recovery(resolve(base, spec["recovery"], f"{where}.recovery"))

        typed_cycles = []
        victims = []
        for raw, cycle in zip(rounds, cycles):
            if not isinstance(raw, dict) or not isinstance(cycle, dict):
                refuse(f"{where}: a cycle is not an object")
            if cycle.get("cycle") != raw.get("cycle"):
                refuse(f"{where}: typed cycle numbering disagrees with the rounds")
            victim_index = cycle.get("victim_index")
            if not positive_int(victim_index) or victim_index >= count:
                refuse(f"{where}: a cycle names no non-bootstrap victim index")
            victim = nodes[victim_index]
            if victim in victims:
                refuse(f"{where}: a victim is cut in more than one cycle")
            victims.append(victim)
            anchor = cycle.get("anchor") if isinstance(cycle.get("anchor"), dict) else {}
            raw_anchor = raw.get("healing_anchor") if isinstance(raw.get("healing_anchor"), dict) else {}
            if anchor.get("hash") != raw_anchor.get("hash") \
                    or anchor.get("height") != raw_anchor.get("height"):
                refuse(f"{where}: a typed anchor disagrees with the raw round")
            if not (isinstance(anchor.get("hash"), str) and HEX64.fullmatch(anchor["hash"])):
                refuse(f"{where}: a cycle has no 64-hex anchor")
            if not nonneg(anchor.get("height")):
                refuse(f"{where}: a cycle has no readable anchor height")
            window = cycle.get("window") if isinstance(cycle.get("window"), dict) else {}
            if window.get("seconds") != raw.get("healing_window_seconds"):
                refuse(f"{where}: a typed healing window disagrees with the raw round")
            dwell = cycle.get("dwell") if isinstance(cycle.get("dwell"), dict) else {}
            if not nonneg(dwell.get("samples")) or dwell["samples"] < 3:
                refuse(f"{where}: a cycle's dwell has fewer than three samples")
            if dwell.get("stable") is not True:
                refuse(f"{where}: a cycle's dwell is not stable")
            if raw.get("rejoined") is None:
                refuse(f"{where}: a cycle did not rejoin")
            if raw.get("healing_failure_class") is not None:
                refuse(f"{where}: a cycle recorded {raw.get('healing_failure_class')!r}")
            if raw.get("epoch_boundary_crossed") is not True:
                refuse(f"{where}: a cycle's missing range did not cross an epoch boundary")
            rows = cycle.get("validators")
            if not isinstance(rows, list):
                refuse(f"{where}: a cycle has no validator retention rows")
            seen = set()
            for row in rows:
                if not isinstance(row, dict) or not nonneg(row.get("node")):
                    refuse(f"{where}: a cycle has a retention row without a node")
                node = row["node"]
                if node >= count or node in seen:
                    refuse(f"{where}: a cycle's retention rows are not one per validator")
                seen.add(node)
                if row.get("finalized_anchor") is not True:
                    refuse(f"{where}: a validator did not retain the anchor")
            if seen != set(range(count)):
                refuse(f"{where}: a cycle's retention rows do not cover every validator")
            typed_cycles.append({
                "cycle": cycle.get("cycle"),
                "victim": victim,
                "anchor": {"hash": anchor["hash"], "height": anchor["height"]},
                "dwell": {"samples": dwell["samples"], "stable": True},
                "window": {"seconds": window["seconds"]},
                "validators": [
                    {"id": nodes[row["node"]], "finalized_anchor": True}
                    for row in sorted(rows, key=lambda item: item["node"])
                ],
                "root_before": approved,
                "root_after": approved,
                "approved_root_finalized": [
                    {"id": row["id"], "finalized": True}
                    for row in root_before["per_validator"]
                ],
            })

        final_nodes = workload_result.get("final_finalized")
        if not isinstance(final_nodes, list):
            refuse(f"{where}: workload-result.json has no final_finalized list")
        final_by_node = {}
        for row in final_nodes:
            if not isinstance(row, dict) or not nonneg(row.get("node")):
                refuse(f"{where}: a finalization row has no node")
            node = row["node"]
            if node >= count or node in final_by_node:
                refuse(f"{where}: finalization rows are not one per validator")
            if not nonneg(row.get("height")):
                refuse(f"{where}: a finalization row has no height")
            final_by_node[node] = row["height"]
        if set(final_by_node) != set(range(count)):
            refuse(f"{where}: finalization rows do not cover every validator")

        workloads[key] = {
            "validators": count,
            "approved_root": approved,
            "candidate": dict(selected),
            "capacity": capacity,
            "arms": {
                "deploy_finalization": {
                    "term": f"finalized-marker-through-height-{target}",
                    "finalized_on_all": True,
                    "approved_root_finalized_on_all": True,
                    "per_validator": [
                        {"id": nodes[node], "finalized": True, "height": final_by_node[node]}
                        for node in range(count)
                    ],
                },
                "partition_rejoin": {"cycles": typed_cycles},
                "host_loss": {
                    "lost_hosts": list(host_loss["lost_hosts"]),
                    "survivors": survivors,
                    "per_validator": [
                        {"id": ident, "finalized_anchor": True, "root": approved}
                        for ident in survivors
                    ],
                    "root_before": approved,
                    "root_after": approved,
                    "retained": True,
                },
                "restart_replay": {
                    "validator": restart_id,
                    "replay_seconds": restart["replay_seconds"],
                    "replay_height": restart["replay_height"],
                    "recovered_height": restart["recovered_height"],
                    "anchor_still_present": True,
                    "fallback_state": restart.get("fallback_state"),
                    "manual_restart": True,
                    "root_before": approved,
                    "root_after": approved,
                },
                "safe_recovery": {
                    "accepted_work_lost": len(recovery["lost_deploys"]),
                    "self_replay": bool(recovery["self_replay_observed"]),
                    "own_key_halt": bool(recovery["own_key_halt_observed"]),
                    "approved_root": approved,
                    "retained": True,
                    "per_validator": [
                        {"id": row["id"], "root_after": approved}
                        for row in root_after["per_validator"]
                        if row["id"] in set(survivors)
                    ],
                },
            },
        }

    document = {"record_data": {
        "schema": SCHEMA,
        "gate_id": GATE,
        "campaign": {
            "name": campaign["name"],
            "locality": campaign["locality"],
            "bootstrap": campaign["bootstrap"],
            "selected": dict(selected),
            "hosts": hosts,
            "rerun": dict(campaign["rerun"]),
        },
        "workloads": workloads,
    }}
    out.parent.mkdir(parents=True, exist_ok=True)
    temporary = out.with_suffix(out.suffix + ".tmp")
    temporary.write_text(json.dumps(document, indent=1) + "\n")
    os.replace(temporary, out)
    return document


def http_bool(url: str) -> bool:
    parsed = urlparse(url)
    if parsed.scheme not in ("http", "https") or not parsed.hostname:
        refuse(f"{url}: not an http(s) URL")
    try:
        with urllib.request.urlopen(url, timeout=15) as response:
            body = response.read().decode(errors="replace").strip()
    except urllib.error.HTTPError as error:
        refuse(f"{url}: answered HTTP {error.code}")
    except (urllib.error.URLError, OSError) as error:
        raise ConnectionError(str(error)) from error
    try:
        value = json.loads(body)
    except ValueError:
        refuse(f"{url}: not JSON: {body[:80]}")
    if not isinstance(value, bool):
        refuse(f"{url}: the answer is not a boolean: {body[:80]}")
    return value


def pairs(values: list[str], flag: str) -> list[tuple[str, str]]:
    result = []
    for value in values:
        ident, separator, url = value.partition("=")
        if not separator or not ident or not url:
            refuse(f"{flag} {value!r} is not ID=URL")
        result.append((ident, url))
    if not result:
        refuse(f"{flag} is absent")
    if len({ident for ident, _ in result}) != len(result):
        refuse(f"{flag} repeats an id")
    return result


def observe_root(args: argparse.Namespace) -> int:
    rows = []
    for ident, endpoint in pairs(args.validator, "--validator"):
        url = endpoint.rstrip("/") + "/api/is-finalized/" + args.approved_root
        rows.append({"id": ident, "url": endpoint,
                     "finalized": http_bool(url)})
    document = {"schema": SCHEMA, "gate_id": GATE, "phase": args.phase,
                "approved_root": args.approved_root, "per_validator": rows}
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(document, indent=1) + "\n")
    if not all(row["finalized"] for row in rows):
        print("m4-campaign: a validator did not finalize the approved root", file=sys.stderr)
        return 1
    return 0


def observe_host_loss(args: argparse.Namespace) -> int:
    lost = pairs(args.lost, "--lost")
    survivors = pairs(args.survivor, "--survivor")
    lost_hosts = [host for host, _ in lost]
    for host, endpoint in lost:
        url = endpoint.rstrip("/") + "/api/is-finalized/" + args.approved_root
        try:
            http_bool(url)
        except ConnectionError:
            continue
        refuse(f"the declared lost host {host!r} answered at {endpoint}")
    rows = []
    for ident, endpoint in survivors:
        url = endpoint.rstrip("/") + "/api/is-finalized/" + args.approved_root
        rows.append({"id": ident, "url": endpoint, "finalized_anchor": True,
                     "root": args.approved_root})
        if not http_bool(url):
            refuse(f"survivor {ident!r} did not finalize the approved root")
    document = {"schema": SCHEMA, "gate_id": GATE, "approved_root": args.approved_root,
                "lost_hosts": lost_hosts, "survivors": [ident for ident, _ in survivors],
                "per_validator": rows, "root_before": args.approved_root,
                "root_after": args.approved_root, "retained": True}
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(document, indent=1) + "\n")
    return 0


def observe_equivocation(args: argparse.Namespace) -> int:
    """Read both twin hashes on every validator and **record** what each finalised.

    This observer never refuses a validator that finalised both blocks: that is the unsafe
    observation the RC-2 predicate exists to read. Only a missing input is refused.
    """
    validators = pairs(args.validator, "--validator")
    if len(validators) < 4:
        refuse("--validator needs at least four validators for the <=1/3 bound")
    if args.injector not in {ident for ident, _ in validators}:
        refuse(f"the injector {args.injector!r} is not one of the declared validators")
    blocks = list(args.block or [])
    if len(blocks) != 2 or len(set(blocks)) != 2:
        refuse("--block must name two distinct twin hashes")
    for block in blocks:
        if not isinstance(block, str) or not HEX64.fullmatch(block):
            refuse(f"block {block!r} is not 64-hex")
    detection = load(Path(args.detection), "the detection file")
    refusals = detection.get("refusals") if isinstance(detection, dict) else None
    if not isinstance(refusals, list):
        refuse("the detection file has no refusals list")
    detected = any(
        isinstance(row, dict) and string(row.get("validator"))
        and row.get("validator") != args.injector
        for row in refusals)
    policy = load(Path(args.policy), "the policy file")
    if not isinstance(policy, dict) or type(policy.get("slashing_enabled")) is not bool:
        refuse("the policy file has no boolean slashing_enabled")
    finalised_by = {}
    for ident, endpoint in validators:
        finalised = []
        for block in blocks:
            url = endpoint.rstrip("/") + "/api/is-finalized/" + block
            if http_bool(url):
                finalised.append(block)
        finalised_by[ident] = finalised
    document = {
        "schema": SCHEMA,
        "gate_id": GATE,
        "kind": "equivocation",
        "injector": args.injector,
        "height": args.height,
        "blocks": blocks,
        "validators": [ident for ident, _ in validators],
        "finalised_by": finalised_by,
        "detected": detected,
        "slashing_enabled": policy["slashing_enabled"],
        "refusals": refusals[:64],
    }
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(document, indent=1) + "\n")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    collect_cmd = sub.add_parser("collect", help="assemble the campaign record")
    collect_cmd.add_argument("--declaration", required=True)
    collect_cmd.add_argument("--out", required=True)
    root_cmd = sub.add_parser("observe-root", help="read the approved root on each validator")
    root_cmd.add_argument("--approved-root", required=True)
    root_cmd.add_argument("--phase", required=True, choices=("before", "after"))
    root_cmd.add_argument("--validator", action="append", default=[], metavar="ID=URL")
    root_cmd.add_argument("--out", required=True)
    loss_cmd = sub.add_parser("observe-host-loss", help="read the survivors after a host stop")
    loss_cmd.add_argument("--approved-root", required=True)
    loss_cmd.add_argument("--lost", action="append", default=[], metavar="HOST=URL")
    loss_cmd.add_argument("--survivor", action="append", default=[], metavar="ID=URL")
    loss_cmd.add_argument("--out", required=True)
    equiv_cmd = sub.add_parser("observe-equivocation",
                               help="read both twin hashes on every validator")
    equiv_cmd.add_argument("--injector", required=True)
    equiv_cmd.add_argument("--validator", action="append", default=[], metavar="ID=URL")
    equiv_cmd.add_argument("--block", action="append", default=[], metavar="HEX64")
    equiv_cmd.add_argument("--height", type=int, required=True)
    equiv_cmd.add_argument("--detection", required=True)
    equiv_cmd.add_argument("--policy", required=True)
    equiv_cmd.add_argument("--out", required=True)
    args = parser.parse_args()
    if args.command == "collect":
        declaration = Path(args.declaration)
        if not declaration.is_file():
            refuse(f"the declaration {declaration} does not exist")
        document = collect(declaration, Path(args.out))
        print(f"m4-campaign: {len(document['record_data']['workloads'])} workloads at {args.out}")
        return 0
    if args.command == "observe-root":
        return observe_root(args)
    if args.command == "observe-equivocation":
        return observe_equivocation(args)
    return observe_host_loss(args)


if __name__ == "__main__":
    raise SystemExit(main())
