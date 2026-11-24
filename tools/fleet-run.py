#!/usr/bin/env python3
"""fleet-run — the rc-public-fleet driver.

The campaign reads a manifest (`tools/fleet_manifest.py`), verifies the four hosts against it, and
then *performs* the run: it submits the deploy, stops and restarts the victim, captures the process
boot ids, the decision traces and the health counters. The claim maps this result into its record
and re-derives the verdict with its own predicates; it never reads the outcome from the manifest.

Transports:

  ssh-systemd   the public fleet. Reads are direct where the endpoint is public and proxied over
                SSH where it is loopback (admin). systemd owns the unit lifecycle.
  docker        the local dress rehearsal. The same actions against a four-validator devnet, so the
                driver and the claim's public path are exercised before the window.

Process boot id (`/health.bootId`) is the restart witness. The kernel boot id
(`/proc/sys/kernel/random/boot_id`) does not change on `systemctl restart`; it is used only to
establish that the four public hosts are distinct.

Exit codes: 0 pass, 1 unsafe, 2 usage/IO, 3 precondition. Every action writes
`<out>/fleet-result.json`.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import ipaddress
import json
import os
import re
import shlex
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEPLOY_ID_RE = re.compile(r"DeployId is: ([0-9a-fA-F]+)")
MARKER_VALUE = 42
ADMITTED_STATUSES = {
    "pooled", "included", "includedunfinalized", "finalizedsuccess", "processed",
    "processedwithsuccess", "includedunfinalised",
}
REJECTED_STATUSES = {"rejected", "rejecteddeploy", "failed", "processederror"}
DEFAULT_TIMEOUTS = {
    "preflight_s": 120, "deploy_s": 120, "settle_s": 300, "stop_s": 90,
    "advance_s": 300, "start_s": 90, "rejoin_s": 600, "trace_s": 120,
}
DOCKER_DEPLOYER_KEY = "a68a6e6cca30f81bd24a719f3145d20e8424bd7b396309b0708a16c7d8000b76"


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


fleet_manifest = _load("fleet_manifest", ROOT / "tools" / "fleet_manifest.py")
pin_probe = _load("release_pin_probe", ROOT / "tools" / "release-pin-probe.py")


class FleetError(RuntimeError):
    """A named failure; the message is the reason in the result."""


class Precondition(FleetError):
    """The run cannot be made; no claim failure is attributed."""


class Unsafe(FleetError):
    """The run happened and a predicate failed."""


def utc_now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    return sha256_bytes(path.read_bytes())


def _local_hostname(hostname: str | None) -> bool:
    if not hostname:
        return True
    text = hostname.strip().lower()
    if text == "localhost" or text.endswith(".localhost"):
        return True
    try:
        address = ipaddress.ip_address(text)
    except ValueError:
        return False
    return (address.is_loopback or address.is_private or address.is_link_local
            or address.is_reserved or address.is_unspecified)


def _local_url(url: str) -> bool:
    return _local_hostname(urllib.parse.urlparse(url).hostname)


def http_json(url: str, *, method: str = "GET", body=None, timeout: float = 20):
    data = None
    headers = {"accept": "application/json"}
    if body is not None:
        data = json.dumps(body).encode()
        headers["content-type"] = "application/json"
    request = urllib.request.Request(url, data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return json.loads(response.read())
    except urllib.error.HTTPError as error:
        raise Precondition(f"{url}: HTTP {error.code}: {error.read()[-300:]!r}") from error
    except (urllib.error.URLError, OSError, ValueError) as error:
        raise Precondition(f"{url}: {type(error).__name__}: {error}") from error


def http_bytes(url: str, *, timeout: float = 20) -> bytes:
    try:
        with urllib.request.urlopen(url, timeout=timeout) as response:
            return response.read()
    except (urllib.error.URLError, OSError) as error:
        raise Precondition(f"{url}: {type(error).__name__}: {error}") from error


def first_block(response):
    if isinstance(response, list):
        return response[0] if response and isinstance(response[0], dict) else None
    if isinstance(response, dict):
        blocks = response.get("blocks")
        if isinstance(blocks, list) and blocks and isinstance(blocks[0], dict):
            return blocks[0]
        if "blockHash" in response:
            return response
    return None


def probe_term(name: str) -> str:
    return f'new return in {{ for (@v <- @{json.dumps(name)}) {{ return!(v) }} }}'


def marker_present(response, value: int = MARKER_VALUE) -> bool:
    return (isinstance(response, dict)
            and isinstance(response.get("expr"), list)
            and len(response["expr"]) == 1
            and response["expr"][0] == {"ExprInt": {"data": value}})


def wait_until(description: str, probe, timeout: float, *, interval: float = 2.0,
               error=FleetError):
    deadline = time.monotonic() + timeout
    while True:
        value = probe()
        if value:
            return value
        if time.monotonic() >= deadline:
            raise error(f"{description}: not observed within {int(timeout)}s")
        time.sleep(interval)


def _trace_listing(directory: str, prefixes: tuple[str, ...]) -> str:
    """List the requested trace prefixes; boot traces first and separately.

    Boot traces are few; a single `tail` over boot and a long merge history once pushed the
    victim's restart decision out of the record. Other prefixes are tailed together.
    """
    quoted = shlex.quote(directory)
    parts = []
    if "boot" in prefixes:
        parts.append(f"ls -1 {quoted} 2>/dev/null | grep -E '^boot-.*\\.json$' | tail -n 50")
    others = [prefix for prefix in prefixes if prefix != "boot"]
    if others:
        pattern = "|".join(re.escape(prefix) for prefix in others)
        parts.append(f"ls -1 {quoted} 2>/dev/null "
                     f"| grep -E '^({pattern})-.*\\.json$' | tail -n 400")
    return "; ".join(parts) if parts else "true"


class Transport:
    """A node's read and action surface. One interface for the public fleet and the devnet."""

    name = "base"

    def __init__(self, manifest: dict):
        self.manifest = manifest
        self.nodes = manifest["nodes"]

    def read_json(self, index: int, path: str, *, admin: bool = False,
                  method: str = "GET", body=None, timeout: float = 20):
        raise NotImplementedError

    def read_raw(self, index: int, path: str, *, timeout: float = 20) -> bytes:
        raise NotImplementedError

    def binary_sha256(self, index: int) -> str:
        raise NotImplementedError

    def binary_version(self, index: int) -> str | None:
        raise NotImplementedError

    def deploy_cli(self, index: int, term: str, deploy: dict, status: dict) -> tuple[int, str, str]:
        raise NotImplementedError

    def submit_deploy(self, index: int, term: str, deploy: dict) -> dict:
        status = self.read_json(index, "/api/v1/status")
        if not isinstance(status, dict):
            raise Precondition("deploy: /api/v1/status is not an object")
        code, stdout, stderr = self.deploy_cli(index, term, deploy, status)
        match = DEPLOY_ID_RE.search(stdout + stderr)
        if not match:
            raise Precondition(
                f"deploy on {self.nodes[index]['name']} returned no deploy id: "
                f"{(stdout + stderr)[-500:]}")
        return {"deploy_id": match.group(1).lower(),
                "valid_after_block_number": status.get("latestBlockNumber")}

    def stop(self, index: int, timeout: float) -> dict:
        raise NotImplementedError

    def start(self, index: int, timeout: float) -> dict:
        raise NotImplementedError

    def host_boot_id(self, index: int) -> str | None:
        raise NotImplementedError

    def unit_state(self, index: int) -> dict:
        raise NotImplementedError

    def shell(self, index: int, command: str, *, timeout: float = 60, stdin=None):
        raise NotImplementedError

    def shell_node(self, node: dict, command: str, *, timeout: float = 60, stdin=None):
        raise NotImplementedError

    def read_remote_file(self, node: dict, path: str, *, timeout: float = 60) -> str:
        result = self.shell_node(node, "cat " + shlex.quote(path), timeout=timeout)
        if result.returncode:
            raise Precondition(
                f"{node.get('name', node.get('unit'))}: cannot read {path}: "
                f"{(result.stderr or result.stdout)[-300:]}")
        return result.stdout

    def unit_env(self, node: dict) -> dict[str, str]:
        result = self.shell_node(
            node, "systemctl show -p Environment --value " + shlex.quote(node["unit"]),
            timeout=30)
        if result.returncode:
            raise Precondition(
                f"{node['name']}: cannot read the unit environment: {result.stderr[-300:]}")
        found = {}
        for token in shlex.split(result.stdout.strip()):
            key, separator, value = token.partition("=")
            if separator and key:
                found[key] = value
        return found

    def set_unit_env(self, node: dict, key: str, value: str | None) -> None:
        """Set or remove one unit environment variable. SSH only; Docker cannot mutate env."""
        raise Precondition(
            "this transport cannot change a running unit's environment; "
            "set it at provisioning time")

    def restart(self, index: int, timeout: float) -> dict:
        stopped = self.stop(index, timeout)
        if stopped.get("exit_code") or not stopped.get("stopped", False):
            raise Precondition(f"the stop did not take effect on {self.nodes[index]['name']}")
        started = self.start(index, timeout)
        if started.get("exit_code") or not started.get("active", False):
            raise Precondition(f"the start did not take effect on {self.nodes[index]['name']}")
        return {"stop": stopped, "start": started,
                "boot_id": self.process_boot_id(index)}

    def install_partition(self, index: int, peers: list[str], ports: list[int],
                          mechanism: str) -> dict:
        raise Precondition("this transport cannot install a partition")

    def remove_partition(self, index: int, mechanism: str) -> dict:
        raise Precondition("this transport cannot remove a partition")

    def collect_traces(self, index: int, dest: Path | None = None,
                       prefixes: tuple[str, ...] = ("boot", "merge")) -> list[dict]:
        raise NotImplementedError

    def journal_tail(self, index: int, lines: int = 80) -> str:
        raise NotImplementedError

    def process_boot_id(self, index: int) -> str | None:
        try:
            health = self.read_json(index, "/health")
        except FleetError:
            return None
        return health.get("bootId") if isinstance(health, dict) else None

    def observe(self, index: int) -> dict:
        node = self.nodes[index]
        out: dict = {"name": node["name"], "errors": []}
        try:
            status = self.read_json(index, "/api/v1/status")
            if isinstance(status, dict):
                out["network_id"] = status.get("networkId")
                out["shard_id"] = status.get("shardId")
                out["height"] = status.get("latestBlockNumber")
                version = status.get("version")
                out["version"] = version.get("node") if isinstance(version, dict) else version
        except FleetError as error:
            out["errors"].append(f"status: {error}")
        try:
            health = self.read_json(index, "/health")
            if isinstance(health, dict):
                for key in ("bootId", "ready", "finalizedHeight", "finalityStalled",
                            "proposalHalted", "proposalHaltReason", "selfValidationRefusal"):
                    out[key] = health.get(key)
        except FleetError as error:
            out["errors"].append(f"health: {error}")
        try:
            genesis = first_block(self.read_json(index, "/api/blocks/0/0"))
            if genesis:
                out["genesis_hash"] = genesis.get("blockHash")
                bonds, digest, total = pin_probe.canonical_bonds(genesis.get("bonds"))
                out["bonds_sha256"] = digest
                out["total_stake"] = total
                if out.get("genesis_hash"):
                    raw = self.read_raw(index, f"/api/block/{out['genesis_hash']}/raw")
                    out["genesis_raw_sha256"] = sha256_bytes(raw)
        except FleetError as error:
            out["errors"].append(f"genesis: {error}")
        try:
            out["binary_sha256"] = self.binary_sha256(index)
            version = self.binary_version(index)
            out["version_text"] = version
            out["source_sha_observed"] = pin_probe.source_sha_from_version(version)
            out["host_boot_id"] = self.host_boot_id(index)
            out["unit_state"] = self.unit_state(index)
        except (FleetError, OSError, subprocess.SubprocessError) as error:
            out["errors"].append(f"host: {type(error).__name__}: {error}")
        return out


class SshSystemdTransport(Transport):
    name = "ssh-systemd"

    def __init__(self, manifest: dict):
        super().__init__(manifest)
        key = os.environ.get("FLEET_SSH_KEY", "")
        self.base = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10",
                     "-o", "StrictHostKeyChecking=accept-new", "-o", "LogLevel=ERROR"]
        if key:
            self.base += ["-i", os.path.expanduser(key)]

    def _run(self, index: int, command: str, *, timeout: float = 60, stdin=None):
        args = list(self.base)
        if stdin is None:
            args.append("-n")
        args += [self.nodes[index]["ssh"], command]
        return subprocess.run(args, input=stdin, text=True, capture_output=True, timeout=timeout)

    def shell(self, index: int, command: str, *, timeout: float = 60, stdin=None):
        return self._run(index, command, timeout=timeout, stdin=stdin)

    def shell_node(self, node: dict, command: str, *, timeout: float = 60, stdin=None):
        args = list(self.base)
        if stdin is None:
            args.append("-n")
        args += [node["ssh"], command]
        return subprocess.run(args, input=stdin, text=True, capture_output=True, timeout=timeout)

    def set_unit_env(self, node: dict, key: str, value: str | None) -> None:
        service = shlex.quote(node["unit"])
        directory = f"/etc/systemd/system/{node['unit']}.d"
        dropin = f"{directory}/rc-fleet-pressure.conf"
        environment = "" if value is None else f"Environment={key}={value}"
        script = " && ".join([
            f"mkdir -p {shlex.quote(directory)}",
            (f"printf '%s\\n' {shlex.quote('[Service]')} {shlex.quote(environment)} "
             f"> {shlex.quote(dropin)}") if environment
            else f"rm -f {shlex.quote(dropin)}",
            "systemctl daemon-reload",
            f"systemctl restart {service}",
        ])
        result = self.shell_node(node, script, timeout=180)
        if result.returncode:
            raise Precondition(
                f"{node['name']}: cannot set {key}: {(result.stderr or result.stdout)[-400:]}")

    def install_partition(self, index: int, peers: list[str], ports: list[int],
                          mechanism: str) -> dict:
        if mechanism != "nftables":
            raise Precondition(f"the ssh-systemd transport installs nftables, not {mechanism}")
        node = self.nodes[index]
        table = f"rc_partition_{os.getpid()}_{index}"
        commands = [
            f"nft delete table inet {table} 2>/dev/null || true",
            f"nft add table inet {table}",
            f"nft 'add chain inet {table} input {{ type filter hook input priority 0; policy accept; }}'",
            f"nft 'add chain inet {table} output {{ type filter hook output priority 0; policy accept; }}'",
        ]
        for peer in peers:
            address = shlex.quote(peer)
            for port in ports:
                for protocol in ("tcp", "udp"):
                    commands.append(
                        f"nft add rule inet {table} output ip daddr {address} {protocol} "
                        f"dport {int(port)} drop")
                    commands.append(
                        f"nft add rule inet {table} input ip saddr {address} {protocol} "
                        f"dport {int(port)} drop")
        result = self.shell(index, " && ".join(commands), timeout=90)
        if result.returncode:
            raise Precondition(
                f"{node['name']}: cannot install the partition: "
                f"{(result.stderr or result.stdout)[-400:]}")
        return {"node": node["name"], "mechanism": "nftables", "table": table,
                "peers": peers, "ports": ports, "installed": True}

    def remove_partition(self, index: int, mechanism: str) -> dict:
        if mechanism != "nftables":
            raise Precondition(f"the ssh-systemd transport removes nftables, not {mechanism}")
        node = self.nodes[index]
        table = f"rc_partition_{os.getpid()}_{index}"
        result = self.shell(index, f"nft delete table inet {table}", timeout=60)
        return {"node": node["name"], "mechanism": "nftables", "table": table,
                "removed": result.returncode == 0,
                "stderr": (result.stderr or result.stdout)[-300:] if result.returncode else ""}

    @staticmethod
    def _curl(url: str, method: str, body, timeout: float) -> str:
        args = ["curl", "-sS", "--max-time", str(int(timeout))]
        if method != "GET":
            args += ["-X", method, "-H", "content-type: application/json",
                     "--data-raw", json.dumps(body or {})]
        args.append(url)
        return " ".join(shlex.quote(argument) for argument in args)

    def read_json(self, index: int, path: str, *, admin: bool = False,
                  method: str = "GET", body=None, timeout: float = 20):
        node = self.nodes[index]
        base = (node.get("admin") or node["http"]) if admin else node["http"]
        url = base.rstrip("/") + path
        if admin or _local_url(url):
            result = self._run(index, self._curl(url, method, body, timeout),
                               timeout=timeout + 10)
            if result.returncode:
                raise Precondition(
                    f"{node['name']}: {path}: {(result.stderr or result.stdout)[-300:]}")
            try:
                return json.loads(result.stdout)
            except ValueError as error:
                raise Precondition(f"{node['name']}: {path}: not JSON: {error}") from error
        return http_json(url, method=method, body=body, timeout=timeout)

    def read_raw(self, index: int, path: str, *, timeout: float = 20) -> bytes:
        node = self.nodes[index]
        url = node["http"].rstrip("/") + path
        if _local_url(url):
            result = self._run(index, self._curl(url, "GET", None, timeout),
                               timeout=timeout + 10)
            if result.returncode:
                raise Precondition(
                    f"{node['name']}: {path}: {(result.stderr or result.stdout)[-300:]}")
            return result.stdout.encode()
        return http_bytes(url, timeout=timeout)

    def binary_sha256(self, index: int) -> str:
        result = self._run(index, "sha256sum " + shlex.quote(self.nodes[index]["binary_path"]))
        if result.returncode:
            raise Precondition(f"{self.nodes[index]['name']}: binary digest failed")
        return result.stdout.split()[0]

    def binary_version(self, index: int) -> str | None:
        result = self._run(index, shlex.quote(self.nodes[index]["binary_path"]) + " --version")
        text = (result.stdout + result.stderr).strip()
        return text or None

    def deploy_cli(self, index: int, term: str, deploy: dict, status: dict):
        node = self.nodes[index]
        path = f"/tmp/rc-fleet-{os.getpid()}.rho"
        write = self._run(index, f"umask 077 && cat > {shlex.quote(path)}",
                          timeout=30, stdin=term)
        if write.returncode:
            raise Precondition(f"{node['name']}: term write failed: {write.stderr[-300:]}")
        command = " ".join([
            shlex.quote(node["binary_path"]),
            "--grpc-host", shlex.quote(node.get("grpc_host") or "127.0.0.1"),
            "--grpc-port", str(node.get("grpc_port") or 40401),
            "deploy",
            "--phlo-limit", str(deploy["phlo_limit"]),
            "--phlo-price", str(deploy["phlo_price"]),
            "--shard-id", shlex.quote(deploy["shard_id"]),
            "--private-key-path", shlex.quote(deploy["private_key_path"]),
            "--valid-after-block-number", str(status.get("latestBlockNumber") or 0),
            shlex.quote(path),
        ])
        try:
            result = self._run(index, command, timeout=120)
        finally:
            self._run(index, f"rm -f {shlex.quote(path)}", timeout=30)
        return result.returncode, result.stdout, result.stderr

    def stop(self, index: int, timeout: float) -> dict:
        node = self.nodes[index]
        unit = node["unit"]
        requested = utc_now()
        result = self._run(index, f"systemctl stop {shlex.quote(unit)}", timeout=timeout)
        stopped = True
        try:
            wait_until(
                f"{node['name']}: unit {unit} stops",
                lambda: self._run(index, f"systemctl is-active {shlex.quote(unit)}")
                .stdout.strip() in ("inactive", "failed"),
                timeout, interval=1.0)
        except FleetError:
            stopped = False
        return {"node": node["name"], "unit": unit, "action": "stop",
                "exit_code": result.returncode, "requested_at_utc": requested,
                "stopped": stopped,
                "stderr": (result.stderr or result.stdout)[-300:] if result.returncode else ""}

    def start(self, index: int, timeout: float) -> dict:
        node = self.nodes[index]
        unit = node["unit"]
        requested = utc_now()
        result = self._run(index, f"systemctl start {shlex.quote(unit)}", timeout=timeout)
        active = True
        try:
            wait_until(
                f"{node['name']}: unit {unit} is active",
                lambda: self._run(index, f"systemctl is-active {shlex.quote(unit)}")
                .stdout.strip() == "active",
                timeout, interval=1.0)
        except FleetError:
            active = False
        return {"node": node["name"], "unit": unit, "action": "start",
                "exit_code": result.returncode, "requested_at_utc": requested,
                "active": active,
                "stderr": (result.stderr or result.stdout)[-300:] if result.returncode else ""}

    def host_boot_id(self, index: int) -> str | None:
        result = self._run(index, "cat /proc/sys/kernel/random/boot_id")
        return result.stdout.strip() or None if result.returncode == 0 else None

    def unit_state(self, index: int) -> dict:
        unit = self.nodes[index]["unit"]
        result = self._run(index, "systemctl show -p ActiveState -p SubState "
                                 "-p ExecMainStartTimestampMonotonic -p MainPID "
                                 + shlex.quote(unit))
        state = {}
        for line in result.stdout.splitlines():
            key, _, value = line.partition("=")
            if key:
                state[key] = value
        return state

    def collect_traces(self, index: int, dest: Path | None = None,
                       prefixes: tuple[str, ...] = ("boot", "merge")) -> list[dict]:
        node = self.nodes[index]
        directory = str(Path(node["data_dir"]) / "traces")
        command = _trace_listing(directory, prefixes)
        listing = self._run(index, command)
        rows = []
        for name in listing.stdout.split():
            content = self._run(index, f"cat {shlex.quote(directory + '/' + name)}")
            try:
                row = json.loads(content.stdout)
            except ValueError:
                continue
            if isinstance(row, dict):
                rows.append({"validator": index, "file": name, **row})
        return rows

    def journal_tail(self, index: int, lines: int = 80) -> str:
        node = self.nodes[index]
        result = self._run(index, f"journalctl -u {shlex.quote(node['unit'])} "
                                  f"-n {int(lines)} --no-pager")
        return (result.stdout + result.stderr)[-6000:]


class DockerTransport(Transport):
    name = "docker"

    def __init__(self, manifest: dict):
        super().__init__(manifest)
        self._partitions: dict[int, str] = {}

    def _run(self, index: int, argv: list[str], *, timeout: float = 60, stdin=None):
        return subprocess.run(["docker", *argv], input=stdin, text=True,
                              capture_output=True, timeout=timeout)

    def _exec(self, index: int, command: str, *, timeout: float = 60, stdin=None):
        args = ["exec"]
        if stdin is not None:
            args.append("-i")
        return self._run(index, args + [self.nodes[index]["unit"], "sh", "-c", command],
                         timeout=timeout, stdin=stdin)

    def shell(self, index: int, command: str, *, timeout: float = 60, stdin=None):
        return self._exec(index, command, timeout=timeout, stdin=stdin)

    def shell_node(self, node: dict, command: str, *, timeout: float = 60, stdin=None):
        args = ["exec"]
        if stdin is not None:
            args.append("-i")
        return subprocess.run(["docker", *args, node["unit"], "sh", "-c", command],
                              input=stdin, text=True, capture_output=True, timeout=timeout)

    def _network(self, index: int) -> str:
        result = self._run(index, [
            "inspect", "-f",
            "{{range $name, $value := .NetworkSettings.Networks}}{{$name}} {{end}}",
            self.nodes[index]["unit"]])
        names = result.stdout.split()
        if result.returncode or len(names) != 1:
            raise Precondition(
                f"{self.nodes[index]['name']}: cannot identify one docker network: "
                f"{(result.stdout or result.stderr).strip()[-300:]}")
        return names[0]

    def install_partition(self, index: int, peers: list[str], ports: list[int],
                          mechanism: str) -> dict:
        if mechanism != "docker-network":
            raise Precondition(f"the docker transport partitions a network, not {mechanism}")
        node = self.nodes[index]
        network = self._network(index)
        result = self._run(index, ["network", "disconnect", network, node["unit"]], timeout=60)
        if result.returncode:
            raise Precondition(
                f"{node['name']}: cannot disconnect {network}: "
                f"{(result.stderr or result.stdout)[-400:]}")
        self._partitions[index] = network
        return {"node": node["name"], "mechanism": "docker-network", "network": network,
                "container": node["unit"], "installed": True}

    def remove_partition(self, index: int, mechanism: str) -> dict:
        if mechanism != "docker-network":
            raise Precondition(f"the docker transport partitions a network, not {mechanism}")
        node = self.nodes[index]
        network = self._partitions.get(index)
        if network is None:
            raise Precondition(f"{node['name']}: no partition was installed by this driver")
        result = self._run(index, ["network", "connect", network, node["unit"]], timeout=60)
        return {"node": node["name"], "mechanism": "docker-network", "network": network,
                "removed": result.returncode == 0,
                "stderr": (result.stderr or result.stdout)[-300:] if result.returncode else ""}

    def read_json(self, index: int, path: str, *, admin: bool = False,
                  method: str = "GET", body=None, timeout: float = 20):
        node = self.nodes[index]
        base = (node.get("admin") or node["http"]) if admin else node["http"]
        url = base.rstrip("/") + path
        try:
            return http_json(url, method=method, body=body, timeout=timeout)
        except FleetError:
            # A container removed from its docker network keeps its in-container listener. Read
            # it through `docker exec` so a partition can still be observed from inside.
            if method != "GET":
                raise
            port = 40405 if admin else 40403
            result = self._exec(index, f"curl -sS --max-time {int(timeout)} "
                                      f"http://127.0.0.1:{port}{shlex.quote(path)}",
                                timeout=timeout + 5)
            if result.returncode:
                raise
            try:
                return json.loads(result.stdout)
            except ValueError as error:
                raise Precondition(f"{node['name']}: {path}: not JSON: {error}") from error

    def read_raw(self, index: int, path: str, *, timeout: float = 20) -> bytes:
        node = self.nodes[index]
        try:
            return http_bytes(node["http"].rstrip("/") + path, timeout=timeout)
        except FleetError:
            result = self._exec(index, f"curl -sS --max-time {int(timeout)} "
                                      f"http://127.0.0.1:40403{shlex.quote(path)}",
                                timeout=timeout + 5)
            if result.returncode:
                raise
            return result.stdout.encode()

    def binary_sha256(self, index: int) -> str:
        result = self._exec(index, "sha256sum " + shlex.quote(self.nodes[index]["binary_path"]))
        if result.returncode:
            raise Precondition(f"{self.nodes[index]['name']}: binary digest failed")
        return result.stdout.split()[0]

    def binary_version(self, index: int) -> str | None:
        result = self._exec(index, shlex.quote(self.nodes[index]["binary_path"]) + " --version")
        text = (result.stdout + result.stderr).strip()
        return text or None

    def deploy_cli(self, index: int, term: str, deploy: dict, status: dict):
        node = self.nodes[index]
        path = f"/tmp/rc-fleet-{os.getpid()}.rho"
        write = self._exec(index, f"umask 077 && cat > {shlex.quote(path)}",
                           timeout=30, stdin=term)
        if write.returncode:
            raise Precondition(f"{node['name']}: term write failed: {write.stderr[-300:]}")
        command = " ".join([
            shlex.quote(node["binary_path"]),
            "--grpc-host", shlex.quote(node.get("grpc_host") or "127.0.0.1"),
            "--grpc-port", str(node.get("grpc_port") or 40401),
            "deploy",
            "--phlo-limit", str(deploy["phlo_limit"]),
            "--phlo-price", str(deploy["phlo_price"]),
            "--shard-id", shlex.quote(deploy["shard_id"]),
            "--private-key", os.environ.get("FLEET_DEPLOY_PRIVATE_KEY", DOCKER_DEPLOYER_KEY),
            "--valid-after-block-number", str(status.get("latestBlockNumber") or 0),
            shlex.quote(path),
        ])
        try:
            result = self._exec(index, command, timeout=120)
        finally:
            self._exec(index, f"rm -f {shlex.quote(path)}", timeout=30)
        return result.returncode, result.stdout, result.stderr

    def stop(self, index: int, timeout: float) -> dict:
        node = self.nodes[index]
        requested = utc_now()
        result = self._run(index, ["stop", node["unit"]], timeout=timeout)
        return {"node": node["name"], "unit": node["unit"], "action": "stop",
                "exit_code": result.returncode, "requested_at_utc": requested,
                "stopped": result.returncode == 0,
                "stderr": result.stderr[-300:] if result.returncode else ""}

    def start(self, index: int, timeout: float) -> dict:
        node = self.nodes[index]
        requested = utc_now()
        result = self._run(index, ["start", node["unit"]], timeout=timeout)
        # `docker start` returns 0 even when the container exits immediately (a resource-preflight
        # refusal does exactly that). Report what the container did, not what the command returned.
        active = result.returncode == 0
        if active:
            deadline = time.monotonic() + min(timeout, 30)
            while True:
                state = self._run(index, ["inspect", "-f", "{{.State.Running}}",
                                          node["unit"]]).stdout.strip()
                if state == "true":
                    break
                if state == "false" and time.monotonic() >= deadline:
                    active = False
                    break
                time.sleep(1)
        return {"node": node["name"], "unit": node["unit"], "action": "start",
                "exit_code": result.returncode, "requested_at_utc": requested,
                "active": active,
                "stderr": (result.stderr or result.stdout)[-300:] if result.returncode else "",
                "log_tail": "" if active else self.journal_tail(index, 20)}

    def host_boot_id(self, index: int) -> str | None:
        result = self._exec(index, "cat /proc/sys/kernel/random/boot_id")
        return result.stdout.strip() or None if result.returncode == 0 else None

    def unit_state(self, index: int) -> dict:
        result = self._run(index, ["inspect", "-f", "{{.State.Status}} {{.State.StartedAt}}",
                                   self.nodes[index]["unit"]])
        parts = result.stdout.strip().split()
        return {"ActiveState": parts[0] if parts else None,
                "StartedAt": parts[1] if len(parts) > 1 else None}

    def collect_traces(self, index: int, dest: Path | None = None,
                       prefixes: tuple[str, ...] = ("boot", "merge")) -> list[dict]:
        node = self.nodes[index]
        directory = str(Path(node["data_dir"]) / "traces")
        command = _trace_listing(directory, prefixes)
        listing = self._exec(index, command)
        rows = []
        for name in listing.stdout.split():
            content = self._exec(index, f"cat {shlex.quote(directory + '/' + name)}")
            try:
                row = json.loads(content.stdout)
            except ValueError:
                continue
            if isinstance(row, dict):
                rows.append({"validator": index, "file": name, **row})
        return rows

    def journal_tail(self, index: int, lines: int = 80) -> str:
        result = self._run(index, ["logs", "--tail", str(int(lines)),
                                   self.nodes[index]["unit"]])
        return (result.stdout + result.stderr)[-6000:]


def make_transport(manifest: dict) -> Transport:
    return SshSystemdTransport(manifest) if manifest["transport"] == "ssh-systemd" \
        else DockerTransport(manifest)


class FleetTarget:
    """The claim's read view over the transport: the same methods as `common.Target`."""

    def __init__(self, manifest: dict, transport: Transport):
        self.manifest = manifest
        self.transport = transport
        self.nodes = [{"name": node["name"],
                       "http": node["http"],
                       "admin": node.get("admin") or node["http"]}
                      for node in manifest["nodes"]]

    def url(self, index: int, path: str, admin: bool = False) -> str:
        node = self.nodes[index]
        return ((node["admin"] if admin else node["http"]).rstrip("/") + path)

    def get(self, index: int, path: str, admin: bool = False, timeout: float = 10):
        return self.transport.read_json(index, path, admin=admin, timeout=timeout)

    def name(self, index: int) -> str:
        return self.nodes[index]["name"]

    def finalized_head(self, index: int) -> tuple[int, str]:
        response = self.get(index, "/api/last-finalized-block")
        info = response.get("blockInfo") if isinstance(response, dict) else None
        if not isinstance(info, dict):
            raise Precondition(f"{self.name(index)}: last-finalized-block has no blockInfo")
        return int(info["blockNumber"]), str(info["blockHash"])

    def is_finalized(self, index: int, block_hash: str) -> bool:
        answer = self.get(index, f"/api/is-finalized/{block_hash}")
        if type(answer) is not bool:
            raise Precondition(f"{self.name(index)}: is-finalized is not a JSON boolean")
        return answer

    def settlement(self, index: int, deploy_id: str) -> dict:
        response = self.get(index, f"/api/v1/deploy-settlement/{deploy_id}")
        if not isinstance(response, dict):
            raise Precondition(f"{self.name(index)}: settlement is not an object")
        return response

    def deploy_status(self, index: int, deploy_id: str):
        return self.get(index, f"/api/v1/deploy-status/{deploy_id}")

    def status(self, index: int) -> dict:
        return self.get(index, "/api/v1/status")

    def health(self, index: int) -> dict:
        return self.get(index, "/health")

    def block_range(self, index: int, start: int, end: int):
        return self.get(index, f"/api/blocks/{int(start)}/{int(end)}")

    def block_info(self, index: int, block_hash: str):
        return self.get(index, f"/api/block/{block_hash}")

    def block_raw(self, index: int, block_hash: str) -> bytes:
        return self.transport.read_raw(index, f"/api/block/{block_hash}/raw")

    def state_at(self, index: int, block_hash: str, name: str = "__fleet_state__"):
        return self.probe(index, name, block_hash)

    def probe(self, index: int, name: str, block_hash: str):
        return self.transport.read_json(
            index, "/api/v1/explore-deploy-by-block-hash", method="POST",
            body={"term": probe_term(name), "blockHash": block_hash,
                  "usePreStateHash": False}, timeout=10)


def resolve_term(manifest: dict) -> str:
    deploy = manifest["deploy"]
    if deploy.get("term"):
        return deploy["term"]
    path = Path(deploy["term_file"])
    if not path.is_file():
        raise Precondition(f"deploy.term_file {path} does not exist")
    data = path.read_bytes()
    digest = sha256_bytes(data)
    if digest != deploy.get("term_sha256"):
        raise Precondition(
            f"deploy.term_file digest {digest[:12]} is not the declared "
            f"{str(deploy.get('term_sha256'))[:12]}")
    return data.decode()


def _status_text(response) -> str | None:
    """Normalise the deploy status.

    The route returns the execution enum directly — `{"ProcessedWithSuccess": {...}}`,
    `{"NotProcessed": {"status": "pooled", …}}` — not a `status` field. Reading only `status` made
    every admitted deploy look unknown and turned a healthy run into a timeout.
    """
    if isinstance(response, str):
        return response.lower()
    if not isinstance(response, dict):
        return None
    for key in ("status", "deployStatus", "stage"):
        value = response.get(key)
        if isinstance(value, str):
            return value.lower()
    for key, value in response.items():
        lowered = key.lower()
        if lowered == "notprocessed":
            inner = value if isinstance(value, dict) else {}
            status = inner.get("status")
            return str(status).lower() if status else "notprocessed"
        if lowered in ("processedwithsuccess", "processederror", "processedsuccess"):
            return lowered
    return None


def deploy_status_rows(target: FleetTarget, deploy_id: str,
                       only: list[str] | None = None) -> list[dict]:
    rows = []
    for index in range(len(target.nodes)):
        if only is not None and target.name(index) not in only:
            continue
        try:
            rows.append({"validator": target.name(index),
                         "status": _status_text(target.deploy_status(index, deploy_id))})
        except FleetError as error:
            rows.append({"validator": target.name(index), "status": None,
                         "error": str(error)})
    return rows


def submit_and_admit(manifest: dict, transport: Transport, target: FleetTarget,
                     marker: str, value: int, term: str, timeout: float,
                     only: list[str] | None = None) -> dict:
    names = [node["name"] for node in manifest["nodes"]]
    ingress = names.index(manifest["deploy"]["from"])
    submitted = transport.submit_deploy(ingress, term, manifest["deploy"])
    deploy_id = submitted["deploy_id"]

    def admitted():
        rows = deploy_status_rows(target, deploy_id, only=only)
        for row in rows:
            if row["status"] in REJECTED_STATUSES:
                raise Precondition(
                    f"the deploy was rejected by {row['validator']}: {row['status']}")
        return rows if all(row["status"] in ADMITTED_STATUSES for row in rows) else None

    try:
        rows = wait_until(
            "every validator admits the deploy (a deploy that reaches one pool is not the campaign)",
            admitted, timeout, interval=3.0)
    except Precondition as error:
        receipts = deploy_status_rows(target, deploy_id, only=only)
        raise Precondition(f"{error}; receipts: {json.dumps(receipts)}") from error
    return {"deploy_id": deploy_id, "from": manifest["deploy"]["from"], "marker": marker,
            "value": value, "term_sha256": sha256_bytes(term.encode()),
            "submitted_at_utc": utc_now(), "valid_after_block_number":
                submitted.get("valid_after_block_number"), "receipts": rows}


def fleet_preflight(manifest: dict, transport: Transport) -> dict:
    rehearsal = manifest.get("rehearsal") is True
    public = not rehearsal
    observed = [transport.observe(index) for index in range(len(manifest["nodes"]))]
    provenance = manifest["provenance"]
    checks: dict = {}
    errors: list[str] = []

    def agreement(field: str):
        values = {entry.get(field) for entry in observed if entry.get(field) not in (None, "")}
        checks[f"{field}_agreement"] = len(values) <= 1
        if len(values) > 1:
            errors.append(f"{field} disagrees across validators: {sorted(map(str, values))}")

    for field in ("source_sha_observed", "binary_sha256", "genesis_hash",
                  "genesis_raw_sha256", "bonds_sha256", "network_id", "shard_id"):
        agreement(field)

    def all_match(field: str, declared: str | None, *, prefix: bool = False,
                  allow_missing: bool = False) -> bool:
        values = [entry.get(field) for entry in observed]
        if not declared:
            return True
        return all(
            (allow_missing and value in (None, ""))
            or (isinstance(value, str)
                and (value == declared or (prefix and declared.startswith(value))))
            for value in values)

    checks["source_is_candidate"] = all_match(
        "source_sha_observed", provenance.get("source_sha"), prefix=True,
        allow_missing=rehearsal)
    checks["source_observed"] = (
        all(entry.get("source_sha_observed") for entry in observed) if public else True)
    checks["binary_is_candidate"] = all_match("binary_sha256", provenance.get("binary_sha256"))
    checks["genesis_is_candidate"] = all_match("genesis_hash", provenance.get("genesis_hash"))
    checks["bonds_is_candidate"] = all_match("bonds_sha256", provenance.get("bonds_sha256"))
    checks["genesis_raw_is_candidate"] = all_match(
        "genesis_raw_sha256", provenance.get("genesis_raw_sha256"))
    checks["network_agrees"] = all_match("network_id", provenance.get("network_id"))
    checks["shard_agrees"] = all_match("shard_id", provenance.get("shard_id"))
    declared_hosts = [node["host"] for node in manifest["nodes"]]
    checks["distinct_names"] = len({node["name"] for node in manifest["nodes"]}) == 4
    checks["distinct_host_labels"] = len(set(declared_hosts)) == 4
    host_boots = [entry.get("host_boot_id") for entry in observed]
    distinct_boots = len(set(host_boots)) == 4 and all(host_boots)
    checks["distinct_host_boot_ids"] = distinct_boots if public else True
    checks["all_ready"] = all(entry.get("ready") is True for entry in observed)
    checks["pins_observed"] = all(
        entry.get(field) for entry in observed
        for field in ("source_sha_observed", "binary_sha256", "genesis_hash", "bonds_sha256"))
    checks["ok"] = all(checks.get(key, True) for key in (
        "source_is_candidate", "source_observed", "binary_is_candidate", "genesis_is_candidate",
        "bonds_is_candidate", "genesis_raw_is_candidate", "network_agrees", "shard_agrees",
        "distinct_names", "distinct_host_labels", "distinct_host_boot_ids", "all_ready",
        "pins_observed",
        "source_sha_observed_agreement", "binary_sha256_agreement", "genesis_hash_agreement",
        "genesis_raw_sha256_agreement", "bonds_sha256_agreement", "network_id_agreement",
        "shard_id_agreement"))
    if not checks["ok"]:
        errors.append("the fleet does not match the manifest")
    return {"observed": observed, "checks": checks, "synthetic_boot_ids": not distinct_boots,
            "errors": errors, "ok": checks["ok"]}


def _heads(target: FleetTarget, only: list[str] | None = None) -> list[dict]:
    rows = []
    for index in range(len(target.nodes)):
        if only is not None and target.name(index) not in only:
            continue
        height, block_hash = target.finalized_head(index)
        rows.append({"validator": target.name(index), "height": height, "hash": block_hash})
    return rows


def _common_head(target: FleetTarget) -> tuple[list[dict], dict, bool]:
    heads = _heads(target)
    lowest = min(heads, key=lambda row: row["height"])
    on_every = all(target.is_finalized(index, lowest["hash"])
                   for index in range(len(target.nodes)))
    return heads, lowest, on_every


def _regressions(samples: list[dict]) -> list[dict]:
    last: dict[str, tuple[int, str]] = {}
    found = []
    for sample in samples:
        for row in sample["heads"]:
            previous = last.get(row["validator"])
            if previous:
                if row["height"] < previous[0]:
                    found.append({"validator": row["validator"], "from": previous,
                                  "to": [row["height"], row["hash"]],
                                  "sample": sample["label"]})
                elif row["height"] == previous[0] and row["hash"] != previous[1]:
                    found.append({"validator": row["validator"], "from": previous,
                                  "to": [row["height"], row["hash"]],
                                  "sample": sample["label"]})
            last[row["validator"]] = (row["height"], row["hash"])
    return found


def _wait_marker(target: FleetTarget, marker: str, value: int, timeout: float,
                 *, error=Precondition, only: list[str] | None = None) -> list[dict]:
    def probe():
        rows = []
        for index in range(len(target.nodes)):
            if only is not None and target.name(index) not in only:
                continue
            height, block_hash = target.finalized_head(index)
            present = marker_present(target.probe(index, marker, block_hash), value)
            rows.append({"validator": target.name(index), "height": height,
                         "hash": block_hash, "marker_present": present})
        return rows if rows and all(row["marker_present"] for row in rows) else None

    return wait_until("the marker is present at every finalised head", probe, timeout,
                      interval=3.0, error=error)


def _health(target: FleetTarget, index: int) -> dict:
    try:
        health = target.get(index, "/health")
    except FleetError:
        return {"error": "health unreadable"}
    return health if isinstance(health, dict) else {"error": "health is not an object"}


def _certificates(target: FleetTarget) -> dict:
    found = {}
    for index in range(len(target.nodes)):
        try:
            status = target.get(index, "/api/v1/status/operator", admin=True)
        except FleetError as error:
            found[target.name(index)] = {"error": str(error)}
            continue
        checkpoint = (status or {}).get("checkpoint") or {}
        found[target.name(index)] = {
            "kind": checkpoint.get("kind"),
            "digest": checkpoint.get("digest"),
            "anchorHeight": checkpoint.get("anchorHeight"),
        }
    return found


class RecoveryRun:
    def __init__(self, manifest: dict, transport: Transport):
        self.manifest = manifest
        self.transport = transport
        self.target = FleetTarget(manifest, transport)
        self.timeouts = {**DEFAULT_TIMEOUTS, **((manifest.get("actions") or {})
                                                .get("timeouts") or {})}
        self.actions = manifest.get("actions") or {}
        self.result = {
            "schema": 1, "campaign": "rc-public-fleet",
            "locality": manifest["locality"], "transport": manifest["transport"],
            "rehearsal": manifest.get("rehearsal") is True,
            "manifest_sha256": fleet_manifest.manifest_sha256(manifest),
            "started_at_utc": utc_now(), "harness_result": None,
            "reasons": [], "errors": [], "samples": [],
        }

    def _sample(self, label: str, only: list[str] | None = None):
        self.result["samples"].append({"label": label,
                                       "heads": _heads(self.target, only=only)})

    def _victim(self, ingress: str) -> str:
        names = [node["name"] for node in self.manifest["nodes"]]
        heads = {row["validator"]: row["height"] for row in _heads(self.target)}
        candidates = [(names[index], heads.get(names[index], 0))
                      for index in range(len(names))
                      if index != 0 and names[index] != ingress]
        if len(candidates) < 1 or len(names) - 1 < 3:
            raise Precondition("no victim leaves three survivors")
        rule = "highest finalised height among non-bootstrap, non-ingress validators"
        chosen = max(candidates, key=lambda pair: (pair[1], pair[0]))
        self.result["victim_selection"] = {
            "rule": rule, "candidates": [{"name": name, "height": height}
                                         for name, height in candidates],
            "chosen": chosen[0],
        }
        return chosen[0]

    def run(self) -> dict:
        preflight = fleet_preflight(self.manifest, self.transport)
        self.result["preflight"] = preflight
        if not preflight["ok"]:
            raise Precondition("preflight: " + "; ".join(preflight["errors"] or ["failed"]))

        def baseline():
            try:
                heads, lowest, on_every = _common_head(self.target)
            except (FleetError, OSError, subprocess.SubprocessError):
                return None
            return (heads, lowest, on_every) if on_every else None

        heads, lowest, _ = wait_until(
            "the baseline finalised head is available on every validator", baseline,
            self.timeouts["preflight_s"], interval=2.0, error=Precondition)
        self.result["baseline"] = {"per_node": heads, "common_height": lowest["height"],
                                   "common_hash": lowest["hash"]}
        self._sample("baseline")

        before = {node["name"]: self.transport.process_boot_id(index)
                  for index, node in enumerate(self.manifest["nodes"])}
        if any(value is None for value in before.values()):
            raise Precondition("baseline: a validator reports no process boot id")
        self.result["boot_ids"] = {"before": before, "after": {}, "source": "node-process"}

        deploy = self.manifest["deploy"]
        term = resolve_term(self.manifest)
        first = submit_and_admit(self.manifest, self.transport, self.target,
                                 deploy["marker"], int(deploy.get("value", MARKER_VALUE)),
                                 term, self.timeouts["settle_s"])
        self.result["deploy"] = first
        self.result["pre_stop_marker"] = _wait_marker(
            self.target, deploy["marker"], int(deploy.get("value", MARKER_VALUE)),
            self.timeouts["settle_s"])

        victim = self._victim(deploy["from"])
        victim_index = [node["name"] for node in self.manifest["nodes"]].index(victim)
        survivors = [name for name in before if name != victim]
        self.result["victim"] = victim
        self.result["survivors"] = survivors

        stop = self.transport.stop(victim_index, self.timeouts["stop_s"])
        self.result["stop"] = stop
        if stop.get("exit_code") or not stop.get("stopped"):
            raise Precondition(f"the stop did not take effect on {victim}")

        advance = int(self.actions.get("survivor_advance_blocks", 3))
        target_height = self.result["baseline"]["common_height"] + advance

        def advanced():
            rows = _heads(self.target, only=survivors)
            return rows if all(row["height"] >= target_height for row in rows) else None

        wait_until("the survivors finalise past the kill", advanced,
                   self.timeouts["advance_s"], interval=3.0, error=Unsafe)
        self._sample("survivors-advanced", only=survivors)

        survivor_marker = f"{deploy['marker']}:survivor"
        second_term = f'@{json.dumps(survivor_marker)}!({int(deploy.get("value", MARKER_VALUE))})'
        survivor_ingress = next(name for name in survivors if name != deploy["from"])
        second_manifest = json.loads(json.dumps(self.manifest))
        second_manifest["deploy"]["from"] = survivor_ingress
        second = submit_and_admit(
            second_manifest, self.transport, self.target, survivor_marker,
            int(deploy.get("value", MARKER_VALUE)), second_term, self.timeouts["settle_s"],
            only=survivors)
        _wait_marker(self.target, survivor_marker, int(deploy.get("value", MARKER_VALUE)),
                     self.timeouts["settle_s"], error=Unsafe, only=survivors)
        self.result["survivor_deploy"] = {"deploy_id": second["deploy_id"],
                                          "from": survivor_ingress,
                                          "marker": survivor_marker, "resolved": True,
                                          "receipts": second["receipts"]}

        start = self.transport.start(victim_index, self.timeouts["start_s"])
        self.result["start"] = start
        if start.get("exit_code") or not start.get("active"):
            raise Precondition(f"the start did not take effect on {victim}")

        def restarted():
            value = self.transport.process_boot_id(victim_index)
            return value if value and value != before[victim] else None

        new_boot_id = wait_until("the victim reports a new process boot id", restarted,
                                 self.timeouts["start_s"], interval=2.0, error=Precondition)
        self.result["boot_ids"]["after"] = {
            node["name"]: (new_boot_id if node["name"] == victim
                           else self.transport.process_boot_id(index))
            for index, node in enumerate(self.manifest["nodes"])}

        margin = int(self.actions.get("rejoin_margin_blocks", 3))
        rejoin_height = self.result["baseline"]["common_height"] + margin

        def rejoined():
            rows = _heads(self.target)
            victim_row = next(row for row in rows if row["validator"] == victim)
            if victim_row["height"] < rejoin_height:
                return None
            lowest = min(rows, key=lambda row: row["height"])
            same = all(self.target.is_finalized(index, lowest["hash"])
                       for index in range(len(self.target.nodes)))
            return {"heads": rows, "common": lowest, "same_head": same} if same else None

        rejoin = wait_until("the victim rejoins the common finalised head", rejoined,
                            self.timeouts["rejoin_s"], interval=3.0, error=Unsafe)
        self._sample("victim-rejoined")
        self.result["rejoin"] = {
            "height": next(row["height"] for row in rejoin["heads"]
                           if row["validator"] == victim),
            "hash": next(row["hash"] for row in rejoin["heads"]
                         if row["validator"] == victim),
            "same_head": rejoin["same_head"],
            "common_height": rejoin["common"]["height"],
            "heights_above_baseline":
                rejoin["common"]["height"] - self.result["baseline"]["common_height"],
        }

        health_after = {node["name"]: _health(self.target, index)
                        for index, node in enumerate(self.manifest["nodes"])}
        self.result["health_after"] = health_after
        self.result["own_key_halt"] = any(
            entry.get("proposalHalted") is True for entry in health_after.values())
        self.result["self_replay"] = any(
            entry.get("selfValidationRefusal") is True for entry in health_after.values())
        self.result["regressions"] = _regressions(self.result["samples"])
        self.result["certificates_after"] = _certificates(self.target)
        try:
            self.result["boot_traces"] = self.transport.collect_traces(victim_index)
        except (FleetError, OSError, subprocess.SubprocessError) as error:
            self.result["boot_traces"] = []
            self.result["errors"].append(f"trace collection: {error}")
        if not self.result["boot_traces"]:
            self.result["errors"].append(
                "the victim carries no boot trace; the run reads evidence-insufficient")

        if self.result["own_key_halt"] or self.result["self_replay"]:
            raise Unsafe("a validator halted or refused its own key after the restart")
        if self.result["regressions"]:
            raise Unsafe("a finalised height regressed or changed hash")
        self.result["harness_result"] = "PASS"
        return self.result


PARTITION_PASS = "PASS_BOUNDED_WORKLOAD_PARTITION_REJOIN"


def _begin(manifest: dict, transport: Transport, result: dict) -> dict:
    preflight = fleet_preflight(manifest, transport)
    result["preflight"] = preflight
    if not preflight["ok"]:
        raise Precondition("preflight: " + "; ".join(preflight["errors"] or ["failed"]))
    return preflight


def _height(target: FleetTarget, index: int) -> tuple[int, int]:
    status = target.status(index)
    health = target.health(index)
    latest = status.get("latestBlockNumber") if isinstance(status, dict) else None
    finalized = health.get("finalizedHeight") if isinstance(health, dict) else None
    if type(latest) is not int or type(finalized) is not int:
        raise Precondition(f"{target.name(index)}: latest/finalised height is unavailable")
    return latest, finalized


class PartitionRun:
    def __init__(self, manifest: dict, transport: Transport, out: Path):
        self.manifest = manifest
        self.transport = transport
        self.target = FleetTarget(manifest, transport)
        self.out = out
        self.fault = manifest.get("fault") or {}
        self.actions = manifest.get("actions") or {}
        self.result: dict = {}

    def _victims(self) -> list[int]:
        names = [node["name"] for node in self.manifest["nodes"]]
        ingress = (self.manifest.get("deploy") or {}).get("from")
        heads = {row["validator"]: row["height"] for row in _heads(self.target)}
        candidates = [index for index in range(len(names))
                      if index != 0 and names[index] != ingress]
        if len(candidates) < 2:
            raise Precondition("a partition cycle needs two non-bootstrap, non-ingress victims")
        candidates.sort(key=lambda index: (-heads.get(names[index], 0), index))
        return candidates

    def _anchor(self) -> dict:
        def probe():
            try:
                heads, lowest, on_every = _common_head(self.target)
            except (FleetError, OSError, subprocess.SubprocessError):
                return None
            return {"heads": heads, "lowest": lowest, "on_every": on_every} if on_every else None

        found = wait_until("the common finalised anchor is available", probe,
                           float(self.actions.get("timeouts", {}).get("preflight_s", 120)))
        return {"height": found["lowest"]["height"], "hash": found["lowest"]["hash"],
                "heads": found["heads"]}

    def run(self) -> dict:
        _begin(self.manifest, self.transport, self.result)
        mechanism = self.fault["mechanism"]
        ports = [int(port) for port in self.fault["p2p_ports"]]
        dwell_seconds = float(self.fault["dwell_seconds"])
        dwell_blocks = int(self.fault["dwell_blocks"])
        advance = int(self.actions.get("survivor_advance_blocks", 3))
        victims = self._victims()
        names = [node["name"] for node in self.manifest["nodes"]]
        cycles = []
        failures = []
        for number in range(1, int(self.fault["cycles"]) + 1):
            victim_index = victims[(number - 1) % len(victims)]
            victim = names[victim_index]
            peers = [node.get("p2p_host") or node["host"]
                     for index, node in enumerate(self.manifest["nodes"])
                     if index != victim_index]
            anchor = self._anchor()
            latest_before, finalized_before = _height(self.target, victim_index)
            install = self.transport.install_partition(
                victim_index, peers, ports, mechanism)
            removed = None
            try:
                deadline = time.monotonic() + dwell_seconds
                latest_after, finalized_after = latest_before, finalized_before
                while time.monotonic() < deadline:
                    time.sleep(2.0)
                    latest_after, finalized_after = _height(self.target, victim_index)
                    if latest_after - latest_before >= dwell_blocks:
                        break
                survivors = [index for index in range(len(names)) if index != victim_index]
                survivor_names = [names[index] for index in survivors]

                def advanced():
                    rows = _heads(self.target, only=survivor_names)
                    return rows if all(row["height"] >= anchor["height"] + advance
                                       for row in rows) else None

                wait_until(f"cycle {number}: the survivors finalise past the anchor",
                           advanced,
                           float(self.actions.get("timeouts", {}).get("advance_s", 300)),
                           interval=3.0, error=Unsafe)
            finally:
                removed = self.transport.remove_partition(victim_index, mechanism)
            if not removed.get("removed"):
                failures.append(f"cycle {number}: the partition was not removed")
            producing = (latest_after - latest_before >= dwell_blocks
                         and finalized_after <= finalized_before)
            if not producing:
                failures.append(
                    f"cycle {number}: the victim was not producing under partition "
                    f"(latest {latest_before}->{latest_after}, "
                    f"finalised {finalized_before}->{finalized_after})")
            rejoin_height = anchor["height"] + max(1, dwell_blocks)

            def rejoined():
                victim_height = _height(self.target, victim_index)[1]
                if victim_height < rejoin_height:
                    return None
                common = min(_heads(self.target), key=lambda row: row["height"])
                on_every = all(self.target.is_finalized(index, common["hash"])
                               for index in range(len(names)))
                return {"victim_height": victim_height, "common": common,
                        "same_head": on_every} if on_every else None

            rejoin = wait_until(f"cycle {number}: the victim rejoins", rejoined,
                                float(self.actions.get("timeouts", {}).get("rejoin_s", 600)),
                                interval=3.0, error=Unsafe)
            retention = []
            for index in range(len(names)):
                try:
                    retained = self.target.is_finalized(index, anchor["hash"]) is True
                except FleetError:
                    retained = False
                retention.append({"node": index, "finalized_anchor": retained})
            if not all(row["finalized_anchor"] for row in retention):
                failures.append(f"cycle {number}: an anchor was not retained")
            cycles.append({
                "cycle": number, "victim": victim, "victim_index": victim_index,
                "producing_victim": producing,
                "anchor": {"height": anchor["height"], "hash": anchor["hash"]},
                "dwell": {"secured_seconds": dwell_seconds, "blocks": dwell_blocks,
                          "latest_before": latest_before, "latest_after": latest_after,
                          "finalized_before": finalized_before,
                          "finalized_after": finalized_after},
                "rejoin": rejoin,
                "anchor_retention": retention,
                "partition": {"installed": install, "removed": removed},
            })
        finality_traces = []
        for index in range(len(names)):
            try:
                finality_traces.extend(
                    self.transport.collect_traces(index, prefixes=("finality",)))
            except (FleetError, OSError, subprocess.SubprocessError) as error:
                self.result.setdefault("errors", []).append(
                    f"{names[index]} finality traces: {error}")
        self.result["partition"] = {
            "cycles": cycles, "mechanism": mechanism, "ports": ports,
            "survivor_advance_blocks": advance, "failures": failures,
        }
        self.result["cycles"] = cycles
        self.result["finality_traces"] = finality_traces
        if failures:
            raise Unsafe("; ".join(failures))
        self.result["harness_result"] = PARTITION_PASS
        return self.result


class AvailabilityRun:
    def __init__(self, manifest: dict, transport: Transport, out: Path,
                 join_artifact: str | None = None):
        self.manifest = manifest
        self.transport = transport
        self.target = FleetTarget(manifest, transport)
        self.out = out
        self.availability = manifest.get("availability") or {}
        self.join_artifact = join_artifact
        self.result: dict = {}

    def _join_block(self) -> dict | None:
        if not self.join_artifact:
            return None
        try:
            document = json.loads(Path(self.join_artifact).read_text())
        except (OSError, ValueError) as error:
            raise Precondition(f"the join artifact is unreadable: {error}") from error
        if document.get("join_result") != "pass":
            raise Precondition("the join artifact did not pass; the availability sweep is not "
                               "RC-6's arm")
        bootstrap = (self.manifest.get("join") or {}).get("bootstrap_from")
        if document.get("bootstrap_from") != bootstrap:
            raise Precondition("the join artifact names a different bootstrap")
        return {"join_result": "pass", "bootstrap_from": bootstrap,
                "joiner": document.get("joiner"), "chain_complete":
                    (document.get("chain") or {}).get("complete") is True,
                "audit_verdict": (document.get("audit") or {}).get("verdict")}

    def run(self) -> dict:
        _begin(self.manifest, self.transport, self.result)
        names = [node["name"] for node in self.manifest["nodes"]]
        window = int(self.availability.get("window_blocks", 10))
        if window < 10:
            raise Precondition("availability.window_blocks must be at least 10")
        heads = _heads(self.target)
        common_height = min(row["height"] for row in heads)
        start = common_height - window + 1
        if start < 1:
            raise Precondition(f"the chain has no {window}-block finalised window")
        rows = target_rows = self.target.block_range(0, start, common_height)
        blocks = target_rows if isinstance(target_rows, list) else (
            target_rows.get("blocks") if isinstance(target_rows, dict) else None)
        if not isinstance(blocks, list):
            raise Precondition("the block range is not a list")
        by_height = {}
        for row in blocks:
            if isinstance(row, dict) and type(row.get("blockNumber")) is int:
                by_height[row["blockNumber"]] = row.get("blockHash")
        missing_heights = [height for height in range(start, common_height + 1)
                           if not isinstance(by_height.get(height), str)]
        if missing_heights:
            raise Precondition(f"the block range omits finalised heights {missing_heights[:5]}")
        readings = []
        not_available = []
        for height in range(start, common_height + 1):
            block_hash = by_height[height]
            validators = []
            for index, name in enumerate(names):
                row = {"validator": index, "body_present": False, "state_served": False,
                       "error": None}
                try:
                    row["body_present"] = bool(self.target.block_raw(index, block_hash))
                except FleetError as error:
                    row["error"] = f"body: {error}"
                try:
                    state = self.target.state_at(index, block_hash)
                    row["state_served"] = isinstance(state, dict) and state.get("expr") is not None
                    if not row["state_served"] and row["error"] is None:
                        row["error"] = "state: the block served no state"
                except FleetError as error:
                    row["error"] = (row["error"] + "; " if row["error"] else "") + f"state: {error}"
                validators.append(row)
                if not row["body_present"] or not row["state_served"] or row["error"]:
                    not_available.append({"height": height, "hash": block_hash,
                                          "validator": index, "error": row["error"]})
            readings.append({"height": height, "hash": block_hash, "validators": validators})
        traces = []
        for index in range(len(names)):
            try:
                traces.extend(self.transport.collect_traces(index, prefixes=("state",)))
            except (FleetError, OSError, subprocess.SubprocessError) as error:
                self.result.setdefault("errors", []).append(
                    f"{names[index]} state traces: {error}")
        self.result["availability"] = {"blocks": readings,
                                       "not_available": not_available,
                                       "window_blocks": window,
                                       "common_height": common_height}
        self.result["state_traces"] = traces
        join = self._join_block()
        if join is not None:
            self.result["availability"]["join"] = join
        if not_available:
            raise Unsafe(f"{len(not_available)} finalised block reads were unavailable")
        self.result["harness_result"] = "PASS"
        return self.result


class PressureRun:
    def __init__(self, manifest: dict, transport: Transport, out: Path):
        self.manifest = manifest
        self.transport = transport
        self.target = FleetTarget(manifest, transport)
        self.out = out
        self.pressure = manifest.get("pressure") or {}
        self.result: dict = {}

    def run(self) -> dict:
        _begin(self.manifest, self.transport, self.result)
        names = [node["name"] for node in self.manifest["nodes"]]
        endpoint = self.pressure["endpoint"]
        rehearsal = self.manifest.get("rehearsal") is True
        if not rehearsal and _local_url(endpoint):
            raise Precondition("a public pressure run needs a non-local endpoint")
        host = self.pressure["host"]
        if host not in names:
            raise Precondition(f"pressure.host {host!r} is not a node")
        index = names.index(host)
        node = self.manifest["nodes"][index]
        key = "RNODE_HTTP_MAX_INFLIGHT_DEPLOY"
        original = self.transport.unit_env(node)
        cap = int(self.pressure["cap"])
        port = int(self.pressure["port"])
        phases = {}
        try:
            for mode in self.pressure["mode_order"]:
                self.transport.set_unit_env(node, key, str(cap) if mode == "capped" else None)
                wait_until(f"{mode}: the unit is ready",
                           lambda: (self.target.health(index) or {}).get("ready") is True,
                           float(self.pressure["settle_s"]) + 120, interval=2.0)
                inspect_path = self.out / f"inspect-{mode}.json"
                inspect_path.parent.mkdir(parents=True, exist_ok=True)
                inspect_path.write_text(json.dumps(
                    {"env": [f"{name}={value}" for name, value in
                             self.transport.unit_env(node).items()]}, sort_keys=True) + "\n")
                version_path = self.out / f"version-{mode}.txt"
                version_path.write_text((self.transport.binary_version(index) or "") + "\n")
                phase_out = self.out / mode
                command = [sys.executable, str(ROOT / "tools" / "m2-pressure-arm.py"),
                           "phase", "--mode", mode, "--out", str(phase_out),
                           "--endpoint", endpoint, "--inspect", str(inspect_path),
                           "--version", str(version_path), "--port", str(port),
                           "--cap", str(cap), "--probes", str(int(self.pressure["probes"]))]
                done = subprocess.run(command, cwd=ROOT, text=True, capture_output=True,
                                      timeout=1800)
                (self.out / f"phase-{mode}.log").write_text(done.stdout + done.stderr)
                phases[mode] = {"exit": done.returncode, "out": str(phase_out)}
                if done.returncode:
                    raise Unsafe(f"{mode}: the pressure phase exited {done.returncode}")
        finally:
            self.transport.set_unit_env(node, key, original.get(key))
        grpc_out = self.out / "grpc"
        provenance = self.manifest["provenance"]
        grpc = subprocess.run(
            [sys.executable, str(ROOT / "tools" / "m2-pressure-arm.py"), "grpc",
             "--out", str(grpc_out), "--endpoint", endpoint,
             "--source-sha", provenance["source_sha"],
             "--binary-sha256", provenance["binary_sha256"],
             "--locality", self.manifest["locality"]],
            cwd=ROOT, text=True, capture_output=True, timeout=900)
        (self.out / "grpc.log").write_text(grpc.stdout + grpc.stderr)
        if grpc.returncode:
            raise Unsafe(f"the gRPC pressure observation exited {grpc.returncode}")
        pressure_out = self.out / "pressure"
        assemble = subprocess.run(
            [sys.executable, str(ROOT / "tools" / "m2-pressure-arm.py"), "assemble",
             "--capped", str(self.out / "capped"), "--unset", str(self.out / "unset"),
             "--grpc", str(grpc_out), "--out", str(pressure_out),
             "--locality", self.manifest["locality"], "--endpoint", endpoint,
             "--port", str(port), "--source-sha", provenance["source_sha"],
             "--binary-sha256", provenance["binary_sha256"]],
            cwd=ROOT, text=True, capture_output=True, timeout=600)
        (self.out / "assemble.log").write_text(assemble.stdout + assemble.stderr)
        if assemble.returncode:
            raise Unsafe(f"the pressure record did not assemble: {assemble.returncode}")
        self.result["pressure"] = {"endpoint": endpoint, "host": host, "port": port,
                                   "cap": cap, "phases": phases,
                                   "record": str(pressure_out)}
        self.result["pressure_record"] = str(pressure_out)
        self.result["harness_result"] = "PASS"
        return self.result


class JoinRun:
    def __init__(self, manifest: dict, transport: Transport, out: Path):
        self.manifest = manifest
        self.transport = transport
        self.out = out
        self.join = manifest.get("join") or {}
        self.result: dict = {}

    def _admin_json(self, node: dict, path: str):
        result = self.transport.shell_node(
            node, "curl -sS --max-time 20 " + shlex.quote("http://127.0.0.1:40405" + path),
            timeout=30)
        if result.returncode:
            raise Precondition(f"{node['name']}: admin {path}: {result.stderr[-300:]}")
        try:
            return json.loads(result.stdout)
        except ValueError as error:
            raise Precondition(f"{node['name']}: admin {path} is not JSON: {error}") from error

    def run(self) -> dict:
        _begin(self.manifest, self.transport, self.result)
        names = [node["name"] for node in self.manifest["nodes"]]
        joiner = self.join["joiner"]
        bootstrap = self.join["bootstrap_from"]
        if bootstrap not in names or bootstrap == names[0]:
            raise Precondition("join.bootstrap_from must be a non-producer validator")
        index = names.index(bootstrap)
        bootstrap_node = self.manifest["nodes"][index]
        listing = self.transport.shell_node(
            joiner, "ls -A " + shlex.quote(joiner["data_dir"]) + " 2>/dev/null | head -n 5")
        if listing.stdout.strip():
            raise Precondition("join.fresh_store is declared but the joiner store is not empty")
        exec_start = self.transport.shell_node(
            joiner, "systemctl show -p ExecStart --value " + shlex.quote(joiner["unit"]),
            timeout=30).stdout
        self.result["joiner_exec_start"] = exec_start[-1200:]
        started_at = utc_now()
        start = self.transport.shell_node(
            joiner, "systemctl start " + shlex.quote(joiner["unit"]), timeout=120)
        if start.returncode:
            raise Precondition(f"the joiner did not start: {start.stderr[-300:]}")
        try:
            def joined():
                try:
                    status = self._admin_json(joiner, "/api/last-finalized-block")
                except Precondition:
                    return None
                info = (status or {}).get("blockInfo") if isinstance(status, dict) else None
                height = info.get("blockNumber") if isinstance(info, dict) else None
                if type(height) is not int or height < int(self.join["min_height"]):
                    return None
                common_height, common_hash = FleetTarget(
                    self.manifest, self.transport).finalized_head(index)
                if height < common_height:
                    return None
                return {"height": height, "hash": info.get("blockHash"),
                        "bootstrap_height": common_height,
                        "bootstrap_hash": common_hash,
                        "same_head": info.get("blockHash") == common_hash}

            observed = wait_until("the joiner reaches the bootstrap's finalised head", joined,
                                  float(self.join["timeout_s"]), interval=3.0)
        finally:
            joiner_log = self.transport.shell_node(
                joiner, "journalctl -u " + shlex.quote(joiner["unit"])
                        + " --since " + shlex.quote(started_at) + " --no-pager",
                timeout=120).stdout
            responder_log = self.transport.shell_node(
                bootstrap_node, "journalctl -u " + shlex.quote(bootstrap_node["unit"])
                                + " --since " + shlex.quote(started_at) + " --no-pager",
                timeout=120).stdout
            (self.out / "joiner.log").write_text(joiner_log)
            (self.out / "responder.log").write_text(responder_log)
        self.result["join_result"] = "pass" if observed["same_head"] else "fail"
        self.result["bootstrap_from"] = bootstrap
        self.result["joiner"] = joiner.get("name")
        self.result["joined"] = observed
        chain_path = self.out / "chain.json"
        build = subprocess.run(
            [sys.executable, str(ROOT / "tools" / "m3-join-chain.py"), "build",
             "--joiner-log", str(self.out / "joiner.log"),
             "--responder-log", str(self.out / "responder.log"),
             "--locality", "public-live",
             "--joiner-source", "raw/joiner.log",
             "--responder-source", "raw/responder.log",
             "--out", str(chain_path)],
            cwd=ROOT, text=True, capture_output=True, timeout=300)
        (self.out / "chain-build.log").write_text(build.stdout + build.stderr)
        chain = {}
        if chain_path.is_file():
            try:
                chain = json.loads(chain_path.read_text())
            except ValueError:
                chain = {}
        self.result["chain"] = chain
        self.result["chain_problems"] = chain.get("incomplete_reasons") or []
        if build.returncode or chain.get("complete") is not True:
            self.result["join_result"] = "not run"
            raise Precondition("the six-stage join chain is incomplete: "
                               + "; ".join(map(str, self.result["chain_problems"] or
                                              [build.stdout + build.stderr])))
        checkpoint_status = self._admin_json(joiner, "/api/v1/status/operator")
        checkpoint = (checkpoint_status or {}).get("checkpoint") or {}
        self.result["joiner_checkpoint"] = {
            "kind": checkpoint.get("kind"), "digest": checkpoint.get("digest"),
            "anchor": checkpoint.get("anchor"), "anchorHeight": checkpoint.get("anchorHeight"),
        }
        config = self.join.get("audit_config") or {}
        data_dir = "/tmp/rc6-audit-data"
        audit_out = "/tmp/rc6-audit-out"
        trace_dir = "/tmp/rc6-audit-traces"
        command = ["rnode", "audit", "--bootstrap",
                   bootstrap_node["http"], "--data-dir", data_dir, "--out", audit_out]
        if isinstance(config, dict):
            for key in ("bonds_file", "wallets_file"):
                if config.get(key):
                    command += [f"--{key.replace('_', '-')}", config[key]]
            if config.get("dev_mode"):
                command.append("--dev-mode")
            if config.get("epoch_length"):
                command += ["--epoch-length", str(config["epoch_length"])]
        if checkpoint.get("kind") == "checkpoint" and checkpoint.get("digest"):
            command += ["--checkpoint", checkpoint["digest"]]
        self.transport.shell_node(joiner, "rm -rf " + " ".join(
            shlex.quote(path) for path in (data_dir, audit_out, trace_dir)), timeout=60)
        audit = self.transport.shell_node(
            joiner, "RNODE_TRACE_DIR=" + shlex.quote(trace_dir) + " "
                    + " ".join(shlex.quote(part) for part in command), timeout=900)
        (self.out / "audit.log").write_text(audit.stdout + audit.stderr)
        report = {}
        try:
            report = json.loads(self.transport.read_remote_file(
                joiner, audit_out + "/audit.json"))
        except (Precondition, ValueError) as error:
            self.result["errors"] = [f"audit report: {error}"]
        traces = []
        trace_listing = self.transport.shell_node(
            joiner, "ls -1 " + shlex.quote(trace_dir) + " 2>/dev/null", timeout=30)
        for name in (trace_listing.stdout or "").split():
            try:
                row = json.loads(self.transport.read_remote_file(
                    joiner, trace_dir + "/" + name))
            except (Precondition, ValueError):
                continue
            if isinstance(row, dict):
                traces.append({"validator": "joiner", "file": name, **row})
        self.result["audit"] = {
            "exit": audit.returncode, "verdict": report.get("verdict"),
            "checkpointDigest": report.get("checkpointDigest"),
            "claimedRoot": report.get("claimedRoot"),
            "derivedRoot": report.get("derivedRoot"), "blocks": report.get("blocks"),
            "reason": report.get("reason"),
            "error": None if audit.returncode == 0 else (audit.stderr or audit.stdout)[-800:],
        }
        self.result["digest_matches"] = (
            self.result["audit"].get("checkpointDigest")
            == self.result["joiner_checkpoint"].get("digest")
            if self.result["joiner_checkpoint"].get("kind") == "checkpoint" else None)
        self.result["derived_root_matches"] = (
            self.result["audit"].get("derivedRoot") is not None
            and self.result["audit"].get("derivedRoot") == self.result["audit"].get("claimedRoot"))
        self.result["audit_traces"] = traces
        self.result["boot_traces"] = [
            row for row in traces if str(row.get("kind")) == "boot"]
        if audit.returncode or not report:
            raise Unsafe("the join reached the head but the audit did not produce a verdict")
        if self.result["audit"].get("verdict") != "AGREE":
            raise Unsafe("the join audit did not agree with the checkpoint claim")
        self.result["harness_result"] = "PASS"
        return self.result


def redact_result(result: dict, manifest: dict) -> dict:
    secrets = []
    for node in manifest.get("nodes") or []:
        for field in ("host", "ssh", "http", "admin"):
            if node.get(field):
                secrets.append(str(node[field]))
    deploy = manifest.get("deploy") or {}
    if deploy.get("private_key_path"):
        secrets.append(str(deploy["private_key_path"]))

    def scrub(value):
        if isinstance(value, str):
            for secret in secrets:
                value = value.replace(secret, "<redacted>")
            return value
        if isinstance(value, list):
            return [scrub(item) for item in value]
        if isinstance(value, dict):
            return {key: scrub(item) for key, item in value.items()}
        return value

    return scrub(json.loads(json.dumps(result)))


def write_result(out: Path, result: dict) -> Path:
    out.mkdir(parents=True, exist_ok=True)
    path = out / "fleet-result.json"
    path.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    return path


def load_manifest(path: str) -> dict:
    try:
        return fleet_manifest.load(path)
    except (OSError, ValueError) as error:
        raise Precondition(f"cannot read {path}: {error}") from error


def validate_manifest(manifest: dict, claim: str | None, head: str | None,
                      rehearsal: bool) -> list[str]:
    return fleet_manifest.validate(manifest, claim_id=claim, environment="public",
                                   head=head, rehearsal=rehearsal)


def git_head() -> str:
    try:
        return subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT,
                                       text=True).strip()
    except subprocess.SubprocessError:
        return ""


def command_validate(args) -> int:
    try:
        manifest = load_manifest(args.manifest)
    except Precondition as error:
        print(json.dumps({"result": "REFUSED", "problems": [str(error)]}))
        return 2
    problems = validate_manifest(manifest, args.claim, args.head or git_head(), args.rehearse)
    print(json.dumps({"result": "OK" if not problems else "REFUSED", "problems": problems}))
    return 0 if not problems else 3


def _with_result(args, action) -> int:
    try:
        manifest = load_manifest(args.manifest)
    except Precondition as error:
        print(json.dumps({"action": args.command, "harness_result": "PRECONDITION",
                          "reasons": [str(error)]}))
        return 2
    problems = validate_manifest(manifest, args.claim, args.head or git_head(), args.rehearse)
    if problems:
        result = {"schema": 1, "campaign": "rc-public-fleet", "harness_result": "PRECONDITION",
                  "reasons": problems, "errors": []}
        write_result(Path(args.out), redact_result(result, manifest))
        return 3
    transport = make_transport(manifest)
    target = FleetTarget(manifest, transport)
    result = {"schema": 1, "campaign": "rc-public-fleet",
              "manifest_sha256": fleet_manifest.manifest_sha256(manifest),
              "locality": manifest["locality"], "transport": manifest["transport"],
              "rehearsal": manifest.get("rehearsal") is True,
              "started_at_utc": utc_now(), "harness_result": None,
              "reasons": [], "errors": []}
    code = 0
    try:
        result.update(action(manifest, transport, target, result))
    except Precondition as error:
        result.update({"harness_result": "PRECONDITION", "reasons": [str(error)]})
        code = 3
    except Unsafe as error:
        result.update({"harness_result": "FAIL", "reasons": [str(error)]})
        code = 1
    except (FleetError, OSError, subprocess.SubprocessError, ValueError) as error:
        result.update({"harness_result": "FAIL",
                       "reasons": [f"{type(error).__name__}: {error}"]})
        code = 1
    result["finished_at_utc"] = utc_now()
    write_result(Path(args.out), redact_result(result, manifest))
    print(json.dumps({"action": args.command, "harness_result": result.get("harness_result"),
                      "reasons": result.get("reasons"), "out": str(Path(args.out))}))
    return code


def command_observe(args) -> int:
    def action(manifest, transport, target, result):
        preflight = fleet_preflight(manifest, transport)
        result["preflight"] = preflight
        if not preflight["ok"]:
            raise Precondition("preflight: " + "; ".join(preflight["errors"]))
        result["harness_result"] = "PASS"
        return {}
    return _with_result(args, action)


def command_deploy(args) -> int:
    def action(manifest, transport, target, result):
        preflight = fleet_preflight(manifest, transport)
        result["preflight"] = preflight
        if not preflight["ok"]:
            raise Precondition("preflight: " + "; ".join(preflight["errors"]))
        deploy = manifest["deploy"]
        term = resolve_term(manifest)
        result["deploy"] = submit_and_admit(
            manifest, transport, target, deploy["marker"],
            int(deploy.get("value", MARKER_VALUE)), term,
            (manifest.get("actions") or {}).get("timeouts", {}).get("settle_s", 300))
        result["harness_result"] = "PASS"
        return {}
    return _with_result(args, action)


def command_traces(args) -> int:
    def action(manifest, transport, target, result):
        rows = []
        for index in range(len(manifest["nodes"])):
            try:
                rows.extend(transport.collect_traces(index))
            except (FleetError, OSError, subprocess.SubprocessError) as error:
                result["errors"].append(f"{manifest['nodes'][index]['name']}: {error}")
        result["traces"] = rows
        result["harness_result"] = "PASS" if rows else "PRECONDITION"
        return {}
    return _with_result(args, action)


def command_recovery(args) -> int:
    def action(manifest, transport, target, result):
        run = RecoveryRun(manifest, transport)
        try:
            produced = run.run()
        except (Precondition, Unsafe):
            result.update(run.result)
            raise
        result.update(produced)
        return {}
    return _with_result(args, action)


def _consume(run_class, args):
    def action(manifest, transport, target, result):
        produced = run_class(manifest, transport, Path(args.out)).run()
        for key, value in produced.items():
            if key not in result:
                result[key] = value
        result["harness_result"] = produced.get("harness_result")
        return {}
    return action


def command_partition(args) -> int:
    return _with_result(args, _consume(PartitionRun, args))


def command_availability(args) -> int:
    def action(manifest, transport, target, result):
        produced = AvailabilityRun(manifest, transport, Path(args.out),
                                   join_artifact=getattr(args, "join_artifact", None)).run()
        for key, value in produced.items():
            if key not in result:
                result[key] = value
        result["harness_result"] = produced.get("harness_result")
        return {}
    return _with_result(args, action)


def command_pressure(args) -> int:
    return _with_result(args, _consume(PressureRun, args))


def command_join(args) -> int:
    return _with_result(args, _consume(JoinRun, args))


def command_manifest(args) -> int:
    if args.transport != "docker":
        print(json.dumps({"result": "REFUSED",
                          "problems": ["the live manifest is authored from the runbook; "
                                       "the generator builds the docker rehearsal"]}))
        return 2
    http_base = int(os.environ.get("DEVNET_HTTP_BASE", "40403"))
    admin_base = int(os.environ.get("DEVNET_ADMIN_BASE", "40405"))
    grpc_base = int(os.environ.get("DEVNET_GRPC_BASE", "40402"))
    prefix = args.prefix
    nodes = []
    for index in range(4):
        name = f"{prefix}-bootstrap" if index == 0 else f"{prefix}-validator-{index}"
        nodes.append({
            "name": name,
            "host": f"host-{index}",
            "unit": name,
            "binary_path": "/usr/local/bin/rnode",
            "data_dir": "/var/lib/rnode",
            "http": f"http://127.0.0.1:{http_base + index * 1000}",
            "admin": f"http://127.0.0.1:{admin_base + index * 1000}",
            "grpc_host": "127.0.0.1",
            "grpc_port": 40401,
        })
    status_url = nodes[0]["http"] + "/api/v1/status"
    try:
        status = http_json(status_url, timeout=10)
    except Precondition as error:
        print(json.dumps({"result": "REFUSED",
                          "problems": [f"the devnet is not answering {status_url}: {error}"]}))
        return 2
    transport = DockerTransport({"transport": "docker", "nodes": nodes})
    observed = transport.observe(0)
    source = observed.get("source_sha_observed") or git_head()
    marker = f"rc-public-fleet:rehearsal:{int(time.time())}"
    document = {
        "schema": 1,
        "campaign": "rc-public-fleet",
        "generated_at_utc": utc_now(),
        "locality": "local-devnet",
        "transport": "docker",
        "rehearsal": True,
        "provenance": {
            "source_sha": source,
            "binary_sha256": observed.get("binary_sha256"),
            "genesis_hash": observed.get("genesis_hash"),
            "genesis_raw_sha256": observed.get("genesis_raw_sha256"),
            "bonds_sha256": observed.get("bonds_sha256"),
            "network_id": observed.get("network_id"),
            "shard_id": observed.get("shard_id"),
        },
        "nodes": nodes,
        "deploy": {
            "from": nodes[0]["name"], "marker": marker, "value": MARKER_VALUE,
            "term": f'@{json.dumps(marker)}!({MARKER_VALUE})',
            "phlo_limit": 1000000, "phlo_price": 1,
            "shard_id": status.get("shardId") or observed.get("shard_id") or "/root",
            "relay": {"mode": "relay", "aliases": [node["name"] for node in nodes],
                      "reachability_checked_utc": utc_now(), "unreachable_aliases": []},
        },
        "actions": {"survivor_advance_blocks": 3, "rejoin_margin_blocks": 3,
                    "epoch_length": 10, "retention_epochs": 2,
                    "timeouts": dict(DEFAULT_TIMEOUTS)},
    }
    if args.claim == "RC-2":
        document["fault"] = {
            "kind": "partition", "cycles": 2, "victim_policy": "highest-finalised",
            "p2p_ports": [40400, 40404], "mechanism": "docker-network",
            "dwell_blocks": 2, "dwell_seconds": 15, "heal_timeout_s": 300,
        }
    if args.claim == "RC-3":
        document["availability"] = {"window_blocks": 10, "probe_timeout_s": 20}
    if not document["provenance"]["binary_sha256"]:
        print(json.dumps({"result": "REFUSED",
                          "problems": ["the devnet container reports no binary digest; "
                                       "is it running?"]}))
        return 2
    problems = fleet_manifest.validate(document, claim_id=args.claim, environment="public",
                                       head=source, rehearsal=True)
    if problems:
        print(json.dumps({"result": "REFUSED", "problems": problems}))
        return 2
    path = Path(args.out)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(document, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"result": "WROTE", "manifest": str(path),
                      "manifest_sha256": fleet_manifest.manifest_sha256(document)}))
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    subparsers = parser.add_subparsers(dest="command", required=True)

    def common(subparser, *, manifest=True, out=True):
        if manifest:
            subparser.add_argument("--manifest", required=True)
        if out:
            subparser.add_argument("--out", required=True)
        subparser.add_argument("--claim")
        subparser.add_argument("--head")
        subparser.add_argument("--rehearse", action="store_true")

    validate = subparsers.add_parser("validate")
    common(validate, manifest=True, out=False)
    validate.set_defaults(func=command_validate)
    for name, function in (("observe", command_observe), ("deploy", command_deploy),
                           ("traces", command_traces), ("recovery", command_recovery),
                           ("partition", command_partition), ("pressure", command_pressure),
                           ("join", command_join)):
        subparser = subparsers.add_parser(name)
        common(subparser, manifest=True)
        subparser.set_defaults(func=function)
    availability = subparsers.add_parser("availability")
    common(availability, manifest=True)
    availability.add_argument("--join-artifact",
                              help="a driver-produced join.json to record as RC-6's precondition")
    availability.set_defaults(func=command_availability)
    generate = subparsers.add_parser("manifest")
    generate.add_argument("--prefix", required=True)
    generate.add_argument("--transport", choices=("docker",), default="docker")
    generate.add_argument("--claim", choices=("RC-1", "RC-2", "RC-3", "RC-4", "RC-5", "RC-6",
                                              "RC-7"),
                          help="include the typed input the claim's driver requires")
    generate.add_argument("--out", required=True)
    generate.set_defaults(func=command_manifest)

    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
