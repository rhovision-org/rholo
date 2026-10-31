#!/usr/bin/env python3
"""Bounded passive live join proof. Writes only a new, private output directory.

No validator keys, deploys, proposals, service mutations or firewall changes.
Run on the authorized host that can read all four validator HTTP endpoints.
"""
import argparse
import collections
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import time
import urllib.request


def get(base, path):
    with urllib.request.urlopen(base + path, timeout=4) as response:
        return json.load(response)


def snapshot(apis, genesis):
    rows = []
    for api in apis:
        st = get(api, "/api/status")
        fin = get(api, "/api/last-finalized-block")["blockInfo"]
        gens = [b["blockHash"] for b in get(api, "/api/blocks/0/0")]
        if gens != [genesis]:
            raise RuntimeError("live genesis disagrees with operator trust pin")
        rows.append(dict(api=api, tip=st["latestBlockNumber"],
                         finalized_height=fin["blockNumber"], finalized_hash=fin["blockHash"],
                         genesis=gens, version=st["version"]["node"], address=st["address"]))
    for key in ("tip", "finalized_height", "finalized_hash", "genesis"):
        if len({json.dumps(r[key]) for r in rows}) != 1:
            raise RuntimeError("live validators disagree on " + key)
    return rows


def safety_readings():
    result = {}
    for key, cmd in (
        ("local_validator_services", ["systemctl", "show", "rnode", "rnode-d", "-p", "MainPID",
                                     "-p", "ExecMainStartTimestamp", "-p", "NRestarts"]),
        ("local_firewall_sha256", ["sudo", "-n", "iptables-save"]),
    ):
        p = subprocess.run(cmd, text=True, capture_output=True, timeout=8)
        policy = "\n".join(re.sub(r"\[\d+:\d+\]", "[0:0]", line)
                           for line in p.stdout.splitlines() if line and not line.startswith("#"))
        result[key] = (hashlib.sha256(policy.encode()).hexdigest()
                       if key.endswith("sha256") and p.returncode == 0 else p.stdout.strip())
        result[key + "_exit"] = p.returncode
    return result


def mem_available():
    for line in Path("/proc/meminfo").read_text().splitlines():
        if line.startswith("MemAvailable:"):
            return int(line.split()[1])
    return 0


def case(args, report, label, db, port, wrong=False):
    baseline = snapshot(args.network_api, args.trusted_genesis)
    if not wrong and label.startswith("fresh"):
        db.mkdir(mode=0o700)
        assert not list(db.iterdir()), "fresh store must actually be empty"
    pin = "00" * 32 if wrong else args.trusted_genesis
    bootstrap = (f"rnode://{baseline[0]['address']}@{args.bootstrap_host}"
                 "?protocol=40400&discovery=40404")
    command = [str(args.binary), "--profile", "docker", "run", "--dev-mode", "--no-upnp",
               "--network-id", "testnet", "--shard-name", "testnet", "--parent-shard-id", "/",
               "--host", args.bootstrap_host, "--bootstrap", bootstrap,
               "--api-host", "127.0.0.1", "--data-dir", str(db),
               "--expected-genesis-hash", pin, "--thread-pool-size", "2",
               "--epoch-length", "10", "--quarantine-length", "10",
               "--bond-minimum", "1", "--bond-maximum", "250"]
    command.extend(args.genesis_flags)
    for flag, offset in (("--protocol-port", 0), ("--api-port-grpc-external", 1),
                         ("--api-port-grpc-internal", 2), ("--api-port-http", 3),
                         ("--discovery-port", 4), ("--api-port-admin-http", 5)):
        command.extend([flag, str(port + offset)])
    # Refuse a collision before launching. The sockets are closed without sending payloads.
    for offset in range(6):
        with socket.socket() as s:
            s.bind(("0.0.0.0", port + offset))
    logpath = args.out / (label + ".log")
    row = dict(label=label, command=command, before=baseline, initially_empty=label.startswith("fresh"),
               started_utc=datetime.datetime.now(datetime.timezone.utc).isoformat(),
               port=port, max_rss_kib=0, min_available_kib=mem_available(), pass_=False)
    started = time.monotonic()
    ready_since = None
    proc = None
    unit = "rholo-n457-" + args.out.name + "-" + label + ".service"
    assert re.fullmatch(r"rholo-n457-[a-z0-9.-]+\.service", unit)
    row["disposable_unit"] = unit
    limit = 40 if wrong else args.timeout
    try:
        with logpath.open("w") as log:
            proc = subprocess.Popen(
                ["sudo", "-n", "systemd-run", "--quiet", "--wait", "--pipe", "--collect",
                 "--unit=" + unit, "--uid=" + str(os.getuid()), "--gid=" + str(os.getgid()),
                 "--setenv=RUST_LOG=info", "--property=MemoryMax=768M", "--property=CPUQuota=100%",
                 "--property=MemorySwapMax=256M", "--property=TasksMax=96", "--property=Nice=10", "--property=KillSignal=SIGINT",
                 "--property=TimeoutStopSec=15", "--property=RuntimeMaxSec=" + str(limit+20), "--"]
                + command, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            while time.monotonic() - started < limit:
                avail = mem_available()
                row["min_available_kib"] = min(row["min_available_kib"], avail)
                if avail < 700000:
                    raise RuntimeError("host available memory crossed the safety floor")
                if proc.poll() is not None:
                    row["natural_exit_code"] = proc.returncode
                    break
                pid = subprocess.run(["systemctl", "show", unit, "-p", "MainPID", "--value"],
                                     text=True, capture_output=True, timeout=4).stdout.strip()
                process_status = Path(f"/proc/{pid}/status")
                if pid.isdigit() and process_status.exists():
                    for line in process_status.read_text().splitlines():
                        if line.startswith("VmRSS:"):
                            row["max_rss_kib"] = max(row["max_rss_kib"], int(line.split()[1]))
                if not wrong:
                    try:
                        api = f"http://127.0.0.1:{port+3}"
                        status = get(api, "/api/status")
                        row["last_status"] = status
                        if status["latestBlockNumber"] >= baseline[0]["tip"]:
                            fin = get(api, "/api/last-finalized-block")["blockInfo"]
                            gens = [b["blockHash"] for b in get(api, "/api/blocks/0/0")]
                            if fin["blockHash"] != baseline[0]["finalized_hash"] or gens != [args.trusted_genesis]:
                                raise RuntimeError("recovered node disagrees with trusted chain")
                            row["finalized"] = fin
                            row["genesis"] = gens
                            operator = get(f"http://127.0.0.1:{port+5}", "/api/v1/status/operator")
                            row["operator_sync"] = operator.get("sync", "missing")
                            text = logpath.read_text(errors="replace")
                            if "Making a transition to Running state." in text and row["operator_sync"] is None:
                                ready_since = ready_since or time.monotonic()
                                if time.monotonic() - ready_since >= 12:
                                    row["recovery_s"] = round(ready_since - started, 3)
                                    row["post_running_observation_s"] = round(time.monotonic() - ready_since, 3)
                                    break
                    except (OSError, ValueError, KeyError):
                        pass
                time.sleep(1)
    except Exception as e:
        row["error"] = repr(e)
    finally:
        if proc is not None:
            if proc.poll() is None:
                subprocess.run(["sudo", "-n", "systemctl", "kill", "--signal=SIGINT", unit],
                               capture_output=True, timeout=5)
                try:
                    proc.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    subprocess.run(["sudo", "-n", "systemctl", "stop", unit],
                                   capture_output=True, timeout=20)
                    proc.wait(timeout=5)
            row["exit_code"] = proc.returncode
        row["elapsed_s"] = round(time.monotonic() - started, 3)
    text = logpath.read_text(errors="replace")
    row["entered_running"] = "Making a transition to Running state." in text
    row["restored_lfs"] = "LFS state is successfully restored" in text
    row["active_bonds_error"] = "active-bonds leaf not found" in text
    row["receiver_overflow"] = "block receiver state full" in text
    row["sync_failure"] = "LFS state sync failed" in text
    row["log_sha256"] = hashlib.sha256(logpath.read_bytes()).hexdigest()
    row["deliveries_by_peer"] = dict(collections.Counter(re.findall(
        r"BlockMessage received #\d+ from (rnode://\S+)\.", text)))
    row["unneeded_deliveries"] = text.count("walk does not need")
    row["idle_warnings"] = text.count("No block responses")
    mismatch = bool(re.search(r"genesis.*(?:mismatch|not match|differs|different)", text, re.I))
    row["wrong_pin_rejected"] = wrong and row.get("natural_exit_code", 0) != 0 and mismatch and not row["entered_running"]
    row["pass"] = (row["wrong_pin_rejected"] if wrong else
                   "recovery_s" in row and not any(row[k] for k in ("active_bonds_error", "receiver_overflow", "sync_failure"))
                   and row["exit_code"] == 0
                   and args.expected_source in row.get("last_status", {}).get("version", {}).get("node", "")
                   and (not label.startswith("fresh") or row["restored_lfs"])) and "error" not in row
    row.pop("pass_")
    row["after"] = snapshot(args.network_api, args.trusted_genesis)
    report["cases"].append(row)
    (args.out / "acceptance.json").write_text(json.dumps(report, indent=2, sort_keys=True)+"\n")
    print(label, "PASS" if row["pass"] else "FAIL", row.get("recovery_s"), row["elapsed_s"], flush=True)
    return row["pass"]


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--binary", type=Path, required=True)
    p.add_argument("--expected-source", required=True)
    p.add_argument("--trusted-genesis", required=True)
    p.add_argument("--bootstrap-host", required=True)
    p.add_argument("--network-api", action="append", required=True)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--port-base", type=int, default=43600)
    p.add_argument("--timeout", type=int, default=600)
    args = p.parse_args()
    args.binary = args.binary.resolve(strict=True)
    if len(args.network_api) != 4:
        p.error("exactly four protected validator API endpoints are required")
    version = subprocess.check_output([str(args.binary), "--version"], text=True).strip()
    if args.expected_source not in version:
        raise RuntimeError("binary does not identify the requested source")
    args.out.mkdir(mode=0o700)
    before = snapshot(args.network_api, args.trusted_genesis)
    report = dict(source_sha=args.expected_source, binary_sha256=hashlib.sha256(args.binary.read_bytes()).hexdigest(),
                  binary_version=version, live_before=before, safety_before=safety_readings(), cases=[])
    (args.out / "acceptance.json").write_text(json.dumps(report, indent=2, sort_keys=True)+"\n")
    db = args.out / "empty-store-1"
    ok = case(args, report, "fresh-1", db, args.port_base)
    if ok:
        ok = case(args, report, "restart", db, args.port_base+10)
    if ok:
        ok = case(args, report, "wrong-genesis", db, args.port_base+20, wrong=True)
    if ok:
        ok = case(args, report, "fresh-2", args.out / "empty-store-2", args.port_base+30)
    report["live_after"] = snapshot(args.network_api, args.trusted_genesis)
    report["safety_after"] = safety_readings()
    report["local_services_and_firewall_unchanged"] = report["safety_before"] == report["safety_after"]
    report["live_versions_unchanged"] = [r["version"] for r in before] == [r["version"] for r in report["live_after"]]
    report["live_heights_not_regressed"] = all(
        b["tip"] <= a["tip"] and b["finalized_height"] <= a["finalized_height"]
        for b, a in zip(before, report["live_after"]))
    report["pass"] = (ok and len(report["cases"]) == 4 and report["local_services_and_firewall_unchanged"]
                      and report["live_versions_unchanged"] and report["live_heights_not_regressed"])
    (args.out / "acceptance.json").write_text(json.dumps(report, indent=2, sort_keys=True)+"\n")
    return 0 if report["pass"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
