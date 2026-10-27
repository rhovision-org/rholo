#!/usr/bin/env python3
"""#457 disposable negative control: only the bootstrap may dial the joining node.

Build a mature N=4 Docker chain first. This command resets validator-3 into an empty
store, disconnects it, configures inbound packet drops *inside its own network
namespace*, and reconnects it. Other validators and their Docker bridges are
untouched. The positive control is tools/n457-mature-join-arm.py on the same chain.

A failure of the negative arm is preserved as evidence, not retried until green.
This is not a public-testnet acceptance run or a claim that peer reachability can
be inferred solely from a connected-peer count.
"""

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import time

PREFIX = os.environ.get("DEVNET_PREFIX", "n457reach")
NETWORK = os.environ.get("DEVNET_NETWORK", "n457-reachability")
BOOTSTRAP = f"{PREFIX}-bootstrap"
TARGET = f"{PREFIX}-validator-3"
PEERS = (f"{PREFIX}-validator-1", f"{PREFIX}-validator-2")
BLOCKED_PORTS = (("tcp", 40400), ("tcp", 40404), ("udp", 40404))
GIVE_UP = "no block finished in"
ATTRIBUTION = "no block was received from the peers they were requested from"


def command(argv, *, check=True, env=None, timeout=30):
    return subprocess.run(
        argv, check=check, text=True, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, env=env, timeout=timeout
    )


def docker(*args, check=True, timeout=30):
    return command(["docker", *args], check=check, timeout=timeout)


def details(name):
    value = json.loads(docker("inspect", name).stdout)
    if len(value) != 1:
        raise RuntimeError(f"unexpected Docker inspect count: {name}")
    return value[0]


def address(name):
    return details(name)["NetworkSettings"]["Networks"][NETWORK]["IPAddress"]


def api(name, path):
    raw = docker("exec", name, "curl", "-sf", "--max-time", "3",
                 f"http://127.0.0.1:40403{path}", timeout=8).stdout
    value = json.loads(raw)
    return value if isinstance(value, dict) else {}


def wait_for_heights(minimum, deadline):
    names = (BOOTSTRAP, *PEERS)
    last = {}
    while time.monotonic() < deadline:
        try:
            last = {name: api(name, "/api/status").get("latestBlockNumber", 0)
                    for name in names}
            if all(height >= minimum for height in last.values()):
                return last
        except (subprocess.CalledProcessError, subprocess.TimeoutExpired,
                ValueError, KeyError):
            pass
        time.sleep(3)
    raise RuntimeError(f"mature chain not reached (min={minimum}): {last}")


def block_rule(pid, ip, proto, port):
    # This is limited to the reset container's network namespace. The host and
    # existing validator namespaces do not acquire any firewall changes.
    return [
        "sudo", "nsenter", "-t", str(pid), "-n", "iptables", "-w",
        "-I", "INPUT", "1", "-s", ip, "-p", proto, "--dport", str(port),
        "-j", "DROP",
    ]


def dropped_packets(counter_text, ips):
    """Count observed DROP packets from the *non-bootstrap* peer IPs only."""
    total = 0
    for line in counter_text.splitlines():
        columns = line.split()
        if (len(columns) >= 8 and columns[0].isdigit()
                and columns[2] == "DROP"
                and any(ip in columns for ip in ips)):
            total += int(columns[0])
    return total


def classify_negative(log, latest_height):
    """Return independent falsifiers rather than a generic CI success flag."""
    problems = []
    if "Making a transition to Running state." in log:
        problems.append("joiner entered Running despite the inbound filter")
    if latest_height > 0:
        problems.append(f"joiner advanced to height {latest_height}")
    if "LFS state sync failed" not in log or GIVE_UP not in log:
        problems.append("the LFS timeout/failure was not observed")
    if ATTRIBUTION not in log:
        problems.append("the per-hash missing-answer attribution was not observed")
    return problems


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--min-height", type=int, default=28)
    p.add_argument("--mature-timeout", type=int, default=600)
    p.add_argument("--negative-timeout", type=int, default=300)
    p.add_argument("--out", default="target/issue457/asymmetric")
    args = p.parse_args()
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)

    record = {
        "claim": "#457 asymmetric blocked-inbound negative control",
        "scope": "disposable Docker N4; not a public/live testnet run",
        "prefix": PREFIX, "network": NETWORK, "blocked_ports": BLOCKED_PORTS,
        "status": "INCOMPLETE",
    }
    try:
        pre = wait_for_heights(args.min_height,
                               time.monotonic() + args.mature_timeout)
        record["source_heights"] = pre
        record["source_finalized"] = api(BOOTSTRAP, "/api/last-finalized-block")
        record["peer_ips"] = {peer: address(peer) for peer in PEERS}
        record["bootstrap_ip"] = address(BOOTSTRAP)
        print(f"mature chain: {pre}; bootstrap={record['bootstrap_ip']}",
              flush=True)

        env = os.environ.copy()
        env["DEVNET_RESET_NETWORK_PAUSE"] = "1"
        reset = command(["tools/devnet.sh", "reset", "3"],
                        env=env, timeout=150)
        (out / "reset.log").write_text(reset.stdout + reset.stderr)
        target_info = details(TARGET)
        pid = target_info["State"]["Pid"]
        if not target_info["State"]["Running"] or pid <= 0:
            raise RuntimeError("reset joiner not running while network disconnected")
        if NETWORK in target_info["NetworkSettings"]["Networks"]:
            raise RuntimeError("reset joiner remained attached during fault staging")
        record["reset_pid"] = pid

        installed = []
        for peer, ip in record["peer_ips"].items():
            for proto, port in BLOCKED_PORTS:
                argv = block_rule(pid, ip, proto, port)
                command(argv, timeout=15)
                installed.append({"peer": peer, "ip": ip, "protocol": proto,
                                  "port": port})
        record["installed_rules"] = installed
        record["pre_connect_rules"] = command([
            "sudo", "nsenter", "-t", str(pid), "-n", "iptables", "-S", "INPUT"
        ]).stdout
        print(f"installed {len(installed)} inbound drops in reset node only",
              flush=True)

        docker("network", "connect", NETWORK, TARGET)
        record["reconnected_ip"] = address(TARGET)
        print(f"reconnected {TARGET}: {record['reconnected_ip']}", flush=True)

        deadline = time.monotonic() + args.negative_timeout
        last_status = {}
        log = ""
        while time.monotonic() < deadline:
            combined = docker("logs", TARGET, check=False)
            log = combined.stdout + combined.stderr
            try:
                last_status = api(TARGET, "/api/status")
            except (subprocess.CalledProcessError, subprocess.TimeoutExpired,
                    ValueError):
                pass
            if "LFS state sync failed" in log or "Making a transition to Running state." in log:
                break
            time.sleep(3)

        record["last_status"] = last_status
        record["timeout_observed"] = "LFS state sync failed" in log
        record["per_hash_attribution_observed"] = ATTRIBUTION in log
        record["last_height"] = last_status.get("latestBlockNumber", 0)
        record["rule_counters"] = command([
            "sudo", "nsenter", "-t", str(pid), "-n", "iptables",
            "-n", "-v", "-x", "-L", "INPUT"
        ]).stdout
        record["dropped_packets"] = dropped_packets(
            record["rule_counters"], record["peer_ips"].values()
        )
        (out / "joiner.log").write_text(log)
        (out / "iptables-counters.txt").write_text(record["rule_counters"])
        issues = classify_negative(log, record["last_height"])
        if record["dropped_packets"] == 0:
            issues.append("no peer-to-joiner packets matched the inbound drop rules")
        record["falsifiers"] = issues
        record["status"] = "PASS" if not issues else "FAIL"
        for issue in issues:
            print(f"FAIL: {issue}", file=sys.stderr, flush=True)
        if issues:
            return 1
        print("PASS: bootstrap-only inbound caused bounded LFS refusal "
              "with per-hash attribution; matched isolated negative control",
              flush=True)
        return 0
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired,
            RuntimeError, KeyError, ValueError) as e:
        record["status"] = "ERROR"
        record["error"] = str(e)
        print(f"ERROR: {e}", file=sys.stderr, flush=True)
        return 2
    finally:
        (out / "asymmetric.json").write_text(json.dumps(record, indent=2) + "\n")


if __name__ == "__main__":
    sys.exit(main())
