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

    def collect_traces(self, index: int, dest: Path | None = None) -> list[dict]:
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

    def collect_traces(self, index: int, dest: Path | None = None) -> list[dict]:
        node = self.nodes[index]
        directory = str(Path(node["data_dir"]) / "traces")
        listing = self._run(index, f"ls -1 {shlex.quote(directory)} 2>/dev/null "
                                   "| grep -E '^(merge|boot)-.*\\.json$' | tail -n 200")
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

    def _run(self, index: int, argv: list[str], *, timeout: float = 60, stdin=None):
        return subprocess.run(["docker", *argv], input=stdin, text=True,
                              capture_output=True, timeout=timeout)

    def _exec(self, index: int, command: str, *, timeout: float = 60, stdin=None):
        args = ["exec"]
        if stdin is not None:
            args.append("-i")
        return self._run(index, args + [self.nodes[index]["unit"], "sh", "-c", command],
                         timeout=timeout, stdin=stdin)

    def read_json(self, index: int, path: str, *, admin: bool = False,
                  method: str = "GET", body=None, timeout: float = 20):
        node = self.nodes[index]
        base = (node.get("admin") or node["http"]) if admin else node["http"]
        return http_json(base.rstrip("/") + path, method=method, body=body, timeout=timeout)

    def read_raw(self, index: int, path: str, *, timeout: float = 20) -> bytes:
        node = self.nodes[index]
        return http_bytes(node["http"].rstrip("/") + path, timeout=timeout)

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
        return {"node": node["name"], "unit": node["unit"], "action": "start",
                "exit_code": result.returncode, "requested_at_utc": requested,
                "active": result.returncode == 0,
                "stderr": result.stderr[-300:] if result.returncode else ""}

    def host_boot_id(self, index: int) -> str | None:
        result = self._exec(index, "cat /proc/sys/kernel/random/boot_id")
        return result.stdout.strip() or None if result.returncode == 0 else None

    def unit_state(self, index: int) -> dict:
        result = self._run(index, ["inspect", "-f", "{{.State.Status}} {{.State.StartedAt}}",
                                   self.nodes[index]["unit"]])
        parts = result.stdout.strip().split()
        return {"ActiveState": parts[0] if parts else None,
                "StartedAt": parts[1] if len(parts) > 1 else None}

    def collect_traces(self, index: int, dest: Path | None = None) -> list[dict]:
        node = self.nodes[index]
        directory = str(Path(node["data_dir"]) / "traces")
        listing = self._exec(index, f"ls -1 {shlex.quote(directory)} 2>/dev/null "
                                    "| grep -E '^(merge|boot)-.*\\.json$' | tail -n 200")
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
    if isinstance(response, dict):
        for key in ("status", "deployStatus", "stage"):
            value = response.get(key)
            if isinstance(value, str):
                return value.lower()
    if isinstance(response, str):
        return response.lower()
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

    rows = wait_until(
        "every validator admits the deploy (a deploy that reaches one pool is not the campaign)",
        admitted, timeout, interval=3.0)
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

        heads, lowest, on_every = _common_head(self.target)
        if not on_every:
            raise Precondition("baseline: the validators do not share a finalised head")
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
    if not document["provenance"]["binary_sha256"]:
        print(json.dumps({"result": "REFUSED",
                          "problems": ["the devnet container reports no binary digest; "
                                       "is it running?"]}))
        return 2
    problems = fleet_manifest.validate(document, claim_id=None, environment="public",
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
                           ("traces", command_traces), ("recovery", command_recovery)):
        subparser = subparsers.add_parser(name)
        common(subparser, manifest=True)
        subparser.set_defaults(func=function)
    generate = subparsers.add_parser("manifest")
    generate.add_argument("--prefix", required=True)
    generate.add_argument("--transport", choices=("docker",), default="docker")
    generate.add_argument("--out", required=True)
    generate.set_defaults(func=command_manifest)

    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
