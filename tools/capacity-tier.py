#!/usr/bin/env python3
"""Assemble the G428-M1 typed capacity-tier record from the resource arms' own output.

The declaration is the operator's claim: the tier, its per-validator minima, the service budgets,
the served height, the claimed host-failure tolerance, and the validator -> host/container/stake
map. The measurements come from `n420-soak.py` (soak.json) and `resource-restart-arm.py`
(restart.json). This tool never calls Docker: the run may be gone by the time the record is
assembled, and a re-read would be a different observation.

It refuses rather than fabricating. A missing, null or wrong-typed measurement, a container the
soak did not sample, a container with no declaration, a duplicate mapping, or an unreadable
command line exits non-zero and leaves no output file.

The run is classified `supported-tier` only when no run used the under-minimum override and every
measured memory/CPU/disk share meets the declaration; otherwise it is `fault-injection`, which the
criterion treats as unkeyable evidence. The classification never depends on an OOM outcome: an
OOM on an adequate host is a failure of the run, not a different kind of run.

Usage:
  python3 tools/capacity-tier.py collect --declaration tier.json --out <run>/observations.json
"""
from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

SCHEMA = 1
TIERS = ("minimal", "comfortable")
MEASURED_KEYS = ("rss_mb", "mem_total_mb", "cgroup_limit_mb", "data_dir_free_mb",
                 "rnode_processes", "chain_height", "fallback_state")


def refuse(message: str) -> None:
    raise SystemExit(f"capacity-tier: {message}")


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


def positive_int(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value > 0


def number(value: object) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def read_declaration(path: Path) -> dict:
    declaration = load(path, "the declaration")
    if declaration.get("schema") != SCHEMA:
        refuse(f"declaration schema {declaration.get('schema')!r} is not {SCHEMA}")
    if declaration.get("tier") not in TIERS:
        refuse("declaration.tier is not a known tier")
    minima = declaration.get("per_validator")
    if not isinstance(minima, dict):
        refuse("declaration.per_validator is absent")
    for field in ("cores", "ram_gib", "disk_gib"):
        if not positive_int(minima.get(field)):
            refuse(f"declaration.per_validator.{field} is not a positive integer")
    for field in ("max_peak_rss_mb", "max_replay_seconds", "served_height",
                  "host_failure_tolerance"):
        if not positive_int(declaration.get(field)):
            refuse(f"declaration.{field} is not a positive integer")
    validators = declaration.get("validators")
    if not isinstance(validators, list) or not validators:
        refuse("declaration.validators is absent or empty")
    ids: set[str] = set()
    containers: set[str] = set()
    for index, entry in enumerate(validators):
        where = f"declaration.validators[{index}]"
        if not isinstance(entry, dict):
            refuse(f"{where} is not an object")
        ident = entry.get("id")
        if not isinstance(ident, str) or not ident:
            refuse(f"{where}.id is not a non-empty string")
        if ident in ids:
            refuse(f"{where}.id is listed more than once")
        ids.add(ident)
        if not isinstance(entry.get("host"), str) or not entry["host"]:
            refuse(f"{where}.host is not a non-empty string")
        container = entry.get("container")
        if not isinstance(container, str) or not container:
            refuse(f"{where}.container is not a non-empty string")
        if container in containers:
            refuse(f"{where}.container is listed more than once")
        containers.add(container)
        if not positive_int(entry.get("stake")):
            refuse(f"{where}.stake is not a positive integer")
        for field in ("soak", "restart"):
            if not isinstance(entry.get(field), str) or not entry[field]:
                refuse(f"{where}.{field} is not a path")
    return declaration


def resolve(base: Path, text: str) -> Path:
    path = Path(text)
    return (path if path.is_absolute() else base / path).resolve()


def read_soak(path: Path, containers: set[str]) -> dict:
    document = load(path, f"the soak record {path}")
    samples = document.get("samples")
    if not isinstance(samples, list) or not samples:
        refuse(f"the soak record {path} has no samples")
    seen: set[str] = set()
    for index, sample in enumerate(samples):
        nodes = sample.get("nodes") if isinstance(sample, dict) else None
        if not isinstance(nodes, dict):
            refuse(f"the soak record {path} sample {index} has no nodes")
        seen.update(nodes)
    if seen != containers:
        missing = sorted(containers - seen)
        extra = sorted(seen - containers)
        refuse(f"the soak record {path} does not match the declaration: "
               f"missing {missing}, extra {extra}")
    return document


def node_series(soak: dict, container: str) -> list[dict]:
    series = []
    for index, sample in enumerate(soak["samples"]):
        node = sample["nodes"].get(container)
        if not isinstance(node, dict) or "error" in node:
            refuse(f"the soak sample {index} for {container} is unreadable")
        for key in MEASURED_KEYS:
            if key not in node:
                refuse(f"the soak sample {index} for {container} has no {key}")
        series.append(node)
    return series


def consistent(series: list[dict], key: str, container: str):
    values = [node[key] for node in series]
    if any(value != values[0] for value in values):
        refuse(f"the {key} for {container} changes across the soak samples")
    return values[0]


def peak(series: list[dict], key: str, container: str) -> int:
    values = [node[key] for node in series]
    if not all(isinstance(value, int) and not isinstance(value, bool) for value in values):
        refuse(f"the {key} for {container} is not an integer")
    return max(values)


def minimum(series: list[dict], key: str, container: str) -> int:
    values = [node[key] for node in series]
    if not all(isinstance(value, int) and not isinstance(value, bool) for value in values):
        refuse(f"the {key} for {container} is not an integer")
    return min(values)


def cpu_cores(series: list[dict], container: str, host_cores: object,
              validators_on_host: int) -> float:
    quota = series[-1].get("cpu_quota")
    if isinstance(quota, list) and len(quota) == 2 and all(
            positive_int(part) for part in quota):
        return quota[0] / quota[1]
    if not positive_int(host_cores):
        refuse(f"neither a CPU quota nor a host core count is available for {container}")
    return host_cores / validators_on_host


def deltas(soak: dict, container: str) -> tuple[int, int]:
    initial = soak.get("initial", {}).get(container)
    final = soak.get("final", {}).get(container)
    if not isinstance(initial, dict) or not isinstance(final, dict):
        refuse(f"the soak record has no container facts for {container}")
    for facts in (initial, final):
        if not isinstance(facts.get("restarts"), int):
            refuse(f"the soak record has no restart count for {container}")
    oom = int(final.get("oom_kill", 0)) - int(initial.get("oom_kill", 0))
    if final.get("oom_killed") and not initial.get("oom_killed"):
        oom = max(oom, 1)
    return oom, final["restarts"] - initial["restarts"]


def runtime_facts(soak: dict, container: str) -> dict:
    runtime = soak.get("runtime", {}).get(container)
    if not isinstance(runtime, dict):
        refuse(f"the soak record has no runtime facts for {container}")
    argv = runtime.get("argv")
    if not isinstance(argv, list) or not argv or not all(
            isinstance(word, str) for word in argv):
        refuse(f"the recorded command line for {container} is unreadable")
    cmd = runtime.get("cmd")
    if not isinstance(cmd, list) or not all(isinstance(word, str) for word in cmd):
        refuse(f"the recorded container command for {container} is unreadable")
    flags = runtime.get("override_flags")
    if not isinstance(flags, list) or not all(isinstance(flag, str) for flag in flags):
        refuse(f"the recorded override scan for {container} is unreadable")
    host_cores = runtime.get("host_cores")
    if not positive_int(host_cores):
        refuse(f"the recorded host core count for {container} is unreadable")
    return {"argv": argv, "cmd": cmd, "override": bool(flags),
            "host_cores": host_cores}


def read_restart(path: Path) -> dict:
    document = load(path, f"the restart record {path}")
    if not number(document.get("replay_seconds")) or document["replay_seconds"] <= 0:
        refuse(f"the restart record {path} has no replay time")
    for field in ("replay_height", "replay_peak_rss_mb"):
        value = document.get(field)
        if not isinstance(value, int) or isinstance(value, bool) or value < 0:
            refuse(f"the restart record {path} has no {field}")
    return document


def collect(declaration_path: Path, out: Path) -> dict:
    declaration = read_declaration(declaration_path)
    base = declaration_path.parent
    minima = declaration["per_validator"]
    validators = declaration["validators"]

    by_host: dict[str, list[dict]] = {}
    by_soak: dict[Path, set[str]] = {}
    for entry in validators:
        by_host.setdefault(entry["host"], []).append(entry)
        by_soak.setdefault(resolve(base, entry["soak"]), set()).add(entry["container"])
    soaks = {path: read_soak(path, containers) for path, containers in by_soak.items()}

    records = []
    for entry in validators:
        container = entry["container"]
        soak = soaks[resolve(base, entry["soak"])]
        series = node_series(soak, container)
        runtime = runtime_facts(soak, container)
        on_host = len(by_host[entry["host"]])
        rss = series[-1]["rss_mb"]
        if not isinstance(rss, int) or isinstance(rss, bool):
            refuse(f"the last RSS sample for {container} is not an integer")
        mem_total = consistent(series, "mem_total_mb", container)
        if not isinstance(mem_total, int) or isinstance(mem_total, bool):
            refuse(f"the memory total for {container} is not an integer")
        limit = consistent(series, "cgroup_limit_mb", container)
        if limit is not None and (not isinstance(limit, int) or isinstance(limit, bool)):
            refuse(f"the cgroup limit for {container} is not an integer or null")
        measured = {
            "rss_mb": rss,
            "peak_rss_mb": peak(series, "rss_mb", container),
            "mem_total_mb": mem_total,
            "cgroup_limit_mb": limit,
            "cpu_cores": cpu_cores(series, container, runtime["host_cores"], on_host),
            "host_cores": runtime["host_cores"],
            "data_dir_free_mb": minimum(series, "data_dir_free_mb", container),
            "rnode_processes": peak(series, "rnode_processes", container),
            "chain_height": peak(series, "chain_height", container),
        }
        restart = read_restart(resolve(base, entry["restart"]))
        measured.update({
            "replay_seconds": restart["replay_seconds"],
            "replay_height": restart["replay_height"],
            "replay_peak_rss_mb": restart["replay_peak_rss_mb"],
            "runtime_override": runtime["override"],
            "argv": runtime["argv"],
            "container_config": {"cmd": runtime["cmd"]},
        })
        oom, restarts = deltas(soak, container)
        measured["oom_kills_delta"] = oom
        measured["restart_count_delta"] = restarts
        measured["fallback_states"] = sorted(
            {node["fallback_state"] for node in series})
        if not all(isinstance(state, str) for state in measured["fallback_states"]):
            refuse(f"the fallback state for {container} is unreadable")
        records.append({"id": entry["id"], "host": entry["host"],
                        "container": container, "stake": entry["stake"],
                        "measured": measured})

    # The declaration's own capacity can be below the documented tier; that is the criterion's
    # floor check, not this tool's. This classification only distinguishes a run at the declared
    # tier from a below-tier injection or an override run.
    below = []
    for record in records:
        measured = record["measured"]
        if measured["runtime_override"]:
            below.append(f"{record['id']}: the run used the under-minimum override")
            continue
        share = measured["cgroup_limit_mb"]
        if share is None:
            share = measured["mem_total_mb"]
        if share < minima["ram_gib"] * 1024:
            below.append(f"{record['id']}: the memory share is below the declaration")
        if measured["cpu_cores"] < minima["cores"]:
            below.append(f"{record['id']}: the CPU share is below the declaration")
        if measured["data_dir_free_mb"] < minima["disk_gib"] * 1024:
            below.append(f"{record['id']}: the disk is below the declaration")

    kind = "fault-injection" if below else "supported-tier"
    for problem in below:
        print(f"capacity-tier: {problem}", file=sys.stderr)
    document = {"record": {
        "schema": SCHEMA,
        "kind": kind,
        "declared": {
            "tier": declaration["tier"],
            "per_validator": dict(minima),
            "max_peak_rss_mb": declaration["max_peak_rss_mb"],
            "max_replay_seconds": declaration["max_replay_seconds"],
            "served_height": declaration["served_height"],
            "host_failure_tolerance": declaration["host_failure_tolerance"],
            "validator_count": len(records),
            "total_stake": sum(record["stake"] for record in records),
        },
        "validators": records,
    }}
    out.parent.mkdir(parents=True, exist_ok=True)
    temporary = out.with_suffix(out.suffix + ".tmp")
    temporary.write_text(json.dumps(document, indent=1) + "\n")
    os.replace(temporary, out)
    return document


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["collect"])
    parser.add_argument("--declaration", required=True)
    parser.add_argument("--out", required=True)
    args = parser.parse_args()
    declaration = Path(args.declaration)
    if not declaration.is_file():
        refuse(f"the declaration {declaration} does not exist")
    document = collect(declaration, Path(args.out))
    print(f"capacity-tier: {document['record']['kind']} record at {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
