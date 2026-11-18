#!/usr/bin/env python3
"""Collect one observation document for any observer count, read-only.

The #584 runbook names four validators and fronts its data collection with a private plan. This
tool is the generic instrument: a JSON config names aliases, endpoints, routes and a window; the
observer count is whatever the config lists. It does not submit a deploy, restart a node, sign
anything or touch a key. It reuses the `live-gate.py` sampler and the
`release-pin-probe.py observe` path rather than growing a second collector.

Usage:
  python3 tools/gate-collect.py sample --config plan.json --out target/run
  python3 tools/gate-collect.py pin --config plan.json --out target/pins

Config (schema 1):

  { "schema": 1,
    "subject": {"deploy_id": "..."},
    "observers": [{"alias": "B", "http": "https://...", "admin": "https://..."}],
    "routes": ["health", "status", "last_finalized", "finality",
               "deploy_status", "deploy_settlement"],
    "window": {"interval": 5, "duration": 900, "timeout": 5},
    "records": {"preflight": "/secure/preflight.json",
                "submission": "/secure/submission.json",
                "claims": "/secure/claims.json",
                "recovery": "/secure/recovery.json"},
    "pin": {"local": true, "service": "rnode", "data_dir": "/var/lib/rnode"} }

`sample` writes the sampler's run directory and an `observations.json` in the evaluator's input
shape. `pin` writes the per-host records and the merged preflight. The public record keeps
aliases and endpoint digests; host addresses stay in the private config.
"""
from __future__ import annotations

import argparse
import gzip
import importlib.util
import json
import subprocess
import sys
import tempfile
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
SCHEMA = 1
ROUTES = ("health", "status", "last_finalized", "finality",
          "deploy_status", "deploy_settlement")


def _module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


live_gate = _module("gate_collect_live_gate", ROOT / "tools" / "live-gate.py")


def read_config(path: Path) -> dict:
    config = json.loads(path.read_text())
    if config.get("schema") != SCHEMA:
        raise SystemExit(f"gate-collect: config schema {config.get('schema')!r} is not {SCHEMA}")
    observers = config.get("observers")
    if not isinstance(observers, list) or not observers:
        raise SystemExit("gate-collect: the config must name at least one observer")
    aliases = set()
    for entry in observers:
        if not isinstance(entry, dict) or not entry.get("alias") or not entry.get("http"):
            raise SystemExit("gate-collect: every observer needs alias and http")
        if entry["alias"] in aliases:
            raise SystemExit(f"gate-collect: duplicate observer alias {entry['alias']!r}")
        aliases.add(entry["alias"])
    return config


def _plan(config: dict) -> dict:
    subject = config.get("subject") or {}
    deploy_id = subject.get("deploy_id")
    if not isinstance(deploy_id, str) or not deploy_id.strip():
        raise SystemExit("gate-collect: subject.deploy_id is required for sampling")
    return {
        "schema": SCHEMA,
        "deploy_id": deploy_id,
        "validators": [{"alias": entry["alias"], "base": entry["http"]}
                       for entry in config["observers"]],
    }


def _read_samples(run_dir: Path) -> list[dict]:
    path = run_dir / "samples.jsonl.gz"
    if path.is_file():
        with gzip.open(path, "rt", encoding="utf-8") as stream:
            return [json.loads(line) for line in stream if line.strip()]
    path = run_dir / "samples.jsonl"
    if path.is_file():
        return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
    return []


def _record(document: dict, name: str, path: str | None) -> None:
    if not path:
        return
    target = Path(path)
    if not target.is_absolute():
        target = ROOT / target
    if target.is_file():
        document[name] = json.loads(target.read_text())


def cmd_sample(config: dict, out: Path) -> int:
    plan = _plan(config)
    window = config.get("window") or {}
    with tempfile.NamedTemporaryFile("w", suffix=".json", dir=ROOT / "target",
                                     delete=False) as handle:
        json.dump(plan, handle)
        plan_path = Path(handle.name)
    try:
        code = live_gate.cmd_sample(SimpleNamespace(
            plan=str(plan_path), out=str(out),
            interval=float(window.get("interval", 5)),
            duration=float(window.get("duration", 900)),
            timeout=float(window.get("timeout", 5)),
            observers=len(config["observers"]),
        ))
    finally:
        plan_path.unlink(missing_ok=True)
    run_path = out / "run.json"
    run = json.loads(run_path.read_text()) if run_path.is_file() else None
    events = _read_samples(out)
    routes = config.get("routes")
    if isinstance(routes, list) and routes:
        events = [event for event in events if event.get("route") in set(routes)]
    document = {
        "schema": SCHEMA,
        "subject": config.get("subject") or {},
        "observers": [
            {"alias": entry["alias"],
             "endpoint_sha256": live_gate.endpoint_digest(entry["http"])}
            for entry in config["observers"]
        ],
        "run": run,
        "events": events,
        "record": {"dir": str(out)},
    }
    records = config.get("records") or {}
    for name in ("preflight", "submission", "claims", "recovery"):
        _record(document, name, records.get(name))
    (out / "observations.json").write_text(json.dumps(document, indent=1) + "\n")
    return code


def cmd_pin(config: dict, out: Path) -> int:
    out.mkdir(parents=True, exist_ok=True)
    pin = config.get("pin") or {}
    for entry in config["observers"]:
        command = [sys.executable, str(ROOT / "tools" / "release-pin-probe.py"), "observe",
                   "--out", str(out), "--name", entry["alias"], "--http", entry["http"]]
        if entry.get("admin"):
            command.extend(["--admin", entry["admin"]])
        for flag, key in (("--service", "service"), ("--binary", "binary"),
                         ("--data-dir", "data_dir"), ("--genesis-dir", "genesis_dir"),
                         ("--config-file", "config_file")):
            if pin.get(key):
                command.extend([flag, str(pin[key])])
        if pin.get("local"):
            command.append("--local")
        code = subprocess.run(command, check=False).returncode
        if code != 0:
            return code
    merge = [sys.executable, str(ROOT / "tools" / "release-pin-probe.py"), "merge",
             "--out", str(out), "--expect", str(len(config["observers"])), "--check"]
    if config.get("operator_json"):
        merge.extend(["--operator-json", str(config["operator_json"])])
    if config.get("redact"):
        merge.append("--redact")
    return subprocess.run(merge, check=False).returncode


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("sample", "pin"))
    parser.add_argument("--config", required=True)
    parser.add_argument("--out", required=True)
    args = parser.parse_args(argv)
    config = read_config(Path(args.config))
    out = Path(args.out)
    if args.command == "sample":
        return cmd_sample(config, out)
    return cmd_pin(config, out)


if __name__ == "__main__":
    sys.exit(main())
