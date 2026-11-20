"""Shared plumbing for the seven release-claim tests.

A release claim's test is a live observation and a pure predicate over its record:

    run(argv)                 collect, write claim.json, return 0/1/3
    check(record)             ("pass" | "fail" | "not run", [reasons]) — no network, no clock
    fixture()                 a complete passing record, for the offline suite

This module owns only the parts every claim repeats: HTTP reads, the signed deploy, the
finalised-head and marker reads, provenance, record writing and the devnet lifecycle.
`tools/release-check.py` is the operator surface; it never trusts a record's stored verdict and
always re-runs `check`.
"""
from __future__ import annotations

import datetime
import json
import os
import re
import subprocess
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
VERDICTS = ("pass", "fail", "not run")
DEPLOYER_PRIV = "a68a6e6cca30f81bd24a719f3145d20e8424bd7b396309b0708a16c7d8000b76"
MARKER_VALUE = 42


class ClaimError(RuntimeError):
    """A precondition or observation failed; the message is the reason in the record."""


def utc_now() -> str:
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def http_json(url: str, method: str = "GET", body=None, timeout: float = 10):
    data = None
    headers = {}
    if body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            raw = response.read().decode()
            return response.status, json.loads(raw) if raw else None
    except urllib.error.HTTPError as error:
        raw = error.read().decode(errors="replace")
        try:
            payload = json.loads(raw) if raw else None
        except ValueError:
            payload = raw
        return error.code, payload


def get_json(url: str, timeout: float = 10):
    status, body = http_json(url, timeout=timeout)
    if status >= 400:
        raise ClaimError(f"{url}: HTTP {status}: {body}")
    return body


def wait_until(description: str, condition, timeout: float = 180, interval: float = 1.0):
    """Poll a predicate; return its first truthy value or raise a named timeout."""
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            value = condition()
            if value:
                return value
        except Exception as error:  # noqa: BLE001 - the last read is the reason
            last = f"{type(error).__name__}: {error}"
        time.sleep(interval)
    raise ClaimError(f"{description} did not hold within {timeout:.0f}s ({last or 'no answer'})")


def wait_for_advance(target: "Target", blocks: int = 6, timeout: float = 300) -> list[int]:
    """Wait until every node has produced and finalised past its startup tip.

    A freshly booted devnet answers its API before autopropose has a cadence; the drills treat a
    `finalityStalled` at workload-start as a precondition failure, which is correct for a run but
    wrong for a boot race. Wait for live production and finality before handing over.
    """
    start = []
    for i in range(len(target.nodes)):
        status = target.status(i)
        start.append(int(status.get("latestBlockNumber") or 0))

    def advanced():
        heights = []
        for i in range(len(target.nodes)):
            status = target.status(i)
            health = target.get(i, "/health")
            if status.get("latestBlockNumber", 0) < start[i] + blocks:
                return None
            if health.get("finalityStalled") is not False:
                return None
            if int(health.get("finalizedHeight") or 0) <= 0:
                return None
            heights.append(status["latestBlockNumber"])
        return heights

    return wait_until(f"the chain advances {blocks} blocks and finalises on every validator",
                      advanced, timeout=timeout)


def block_identity(value) -> tuple[int, str]:
    if not isinstance(value, dict):
        raise ClaimError("block response is not an object")
    height, block_hash = value.get("blockNumber"), value.get("blockHash")
    if type(height) is not int or height < 0:
        raise ClaimError("block response has no nonnegative height")
    if not isinstance(block_hash, str) or len(block_hash) != 64:
        raise ClaimError("block response has no 32-byte hash")
    bytes.fromhex(block_hash)
    return height, block_hash


def marker_present(response, expected: int = MARKER_VALUE) -> bool:
    if not isinstance(response, dict) or not isinstance(response.get("expr"), list):
        raise ClaimError("exploration did not return an expression list")
    values = response["expr"]
    return len(values) == 1 and values[0] == {"ExprInt": {"data": expected}}


def probe_term(name: str) -> str:
    return f'new return in {{ for (@v <- @{json.dumps(name)}) {{ return!(v) }} }}'


class Target:
    """A read-only view of N nodes, local (devnet ports) or public (manifest URLs)."""

    def __init__(self, nodes: list[dict]):
        if not nodes:
            raise ClaimError("a target needs at least one node")
        self.nodes = nodes

    def url(self, index: int, path: str, admin: bool = False) -> str:
        node = self.nodes[index]
        base = node["admin"] if admin else node["http"]
        return base.rstrip("/") + path

    def get(self, index: int, path: str, admin: bool = False, timeout: float = 10):
        return get_json(self.url(index, path, admin), timeout=timeout)

    def name(self, index: int) -> str:
        return self.nodes[index].get("name") or f"validator-{index}"

    def finalized_head(self, index: int) -> tuple[int, str]:
        response = self.get(index, "/api/last-finalized-block")
        if not isinstance(response, dict) or "blockInfo" not in response:
            raise ClaimError("last-finalized-block has no blockInfo envelope")
        return block_identity(response["blockInfo"])

    def is_finalized(self, index: int, block_hash: str) -> bool:
        answer = self.get(index, f"/api/is-finalized/{block_hash}")
        if type(answer) is not bool:
            raise ClaimError(f"{self.name(index)}: is-finalized is not a JSON boolean")
        return answer

    def block_info(self, index: int, block_hash: str) -> dict:
        response = self.get(index, f"/api/block/{block_hash}")
        info = response.get("blockInfo") if isinstance(response, dict) else None
        if not isinstance(info, dict):
            raise ClaimError(f"{self.name(index)}: block/{block_hash} has no blockInfo")
        return info

    def blocks(self, index: int, depth: int) -> list[dict]:
        response = self.get(index, f"/api/blocks/{depth}")
        if not isinstance(response, list):
            raise ClaimError(f"{self.name(index)}: blocks/{depth} is not a list")
        return response

    def settlement(self, index: int, deploy_id: str) -> dict:
        response = self.get(index, f"/api/v1/deploy-settlement/{deploy_id}")
        if not isinstance(response, dict):
            raise ClaimError(f"{self.name(index)}: settlement is not an object")
        return response

    def deploy_status(self, index: int, deploy_id: str) -> str:
        response = self.get(index, f"/api/v1/deploy-status/{deploy_id}")
        if isinstance(response, dict):
            return str(response.get("status") or response.get("deployStatus") or response)
        return str(response)

    def probe(self, index: int, name: str, block_hash: str):
        status, body = http_json(
            self.url(index, "/api/v1/explore-deploy-by-block-hash"), "POST",
            {"term": probe_term(name), "blockHash": block_hash, "usePreStateHash": False},
            timeout=10)
        if status >= 400:
            raise ClaimError(f"{self.name(index)}: probe HTTP {status}: {body}")
        return body

    def status(self, index: int) -> dict:
        return self.get(index, "/api/v1/status")


class Devnet:
    """A disposable local devnet, booted and torn down by one test."""

    def __init__(self, prefix: str, *, validators: int = 4, http_base: int = 40403,
                 admin_base: int = 40405, grpc_base: int = 40402, network: str | None = None,
                 memory: str = "2g", node_env: str | None = None, extra_up: tuple = (),
                 node_images: dict | None = None):
        self.prefix = prefix
        self.validators = validators
        self.http_base = http_base
        self.admin_base = admin_base
        self.grpc_base = grpc_base
        self.network = network or f"{prefix}net"
        self.memory = memory
        self.node_env = node_env
        self.extra_up = tuple(extra_up)
        self.node_images = dict(node_images or {})

    def env(self) -> dict:
        env = dict(os.environ)
        env.update({
            "DEVNET_PREFIX": self.prefix,
            "DEVNET_NETWORK": self.network,
            "DEVNET_HTTP_BASE": str(self.http_base),
            "DEVNET_ADMIN_BASE": str(self.admin_base),
            "DEVNET_GRPC_BASE": str(self.grpc_base),
            "DEVNET_NODE_MEMORY": self.memory,
        })
        if self.node_env:
            env["DEVNET_NODE_ENV"] = self.node_env
        # `tools/devnet.sh` names the per-node image by container name with dashes as underscores.
        for index, image in self.node_images.items():
            key = "RNODE_IMAGE_" + self.node(index).replace("-", "_")
            env[key] = image
        return env

    def node(self, index: int) -> str:
        return f"{self.prefix}-bootstrap" if index == 0 else f"{self.prefix}-validator-{index}"

    def nodes(self) -> list[dict]:
        return [
            {"name": self.node(i),
             "http": f"http://127.0.0.1:{self.http_base + i * 1000}",
             "admin": f"http://127.0.0.1:{self.admin_base + i * 1000}"}
            for i in range(self.validators)
        ]

    def target(self) -> Target:
        return Target(self.nodes())

    def shell(self, *args: str, timeout: float = 600) -> subprocess.CompletedProcess:
        return subprocess.run(
            ["bash", str(ROOT / "tools/devnet.sh"), *args],
            env=self.env(), cwd=ROOT, text=True, capture_output=True, timeout=timeout)

    def up(self, fresh: bool = True) -> None:
        args = ["up", "--validators", str(self.validators)]
        if fresh:
            args.append("--fresh")
        args.extend(self.extra_up)
        result = self.shell(*args, timeout=900)
        if result.returncode:
            raise ClaimError(
                f"devnet up failed ({result.returncode}): "
                f"{(result.stderr or result.stdout)[-1200:]}")
        wait_until(
            "the devnet answers /api/v1/status on every validator",
            lambda: all(self._ready(i) for i in range(self.validators)), timeout=240)

    def _ready(self, index: int) -> bool:
        try:
            status, _ = http_json(
                f"http://127.0.0.1:{self.http_base + index * 1000}/api/v1/status", timeout=3)
            return status == 200
        except Exception:
            return False

    def down(self) -> None:
        self.shell("down", "-v", timeout=300)

    def deploy(self, term: str, index: int = 0, timeout: float = 60) -> str:
        """Submit one signed deploy through node `index`; return its id."""
        container = self.node(index)
        status = self.target().status(index)
        write = subprocess.run(
            ["docker", "exec", "-i", container, "sh", "-c", "cat > /tmp/release-claim.rho"],
            input=term, text=True, capture_output=True, timeout=30)
        if write.returncode:
            raise ClaimError(f"deploy file write failed: {write.stderr[-800:]}")
        result = subprocess.run(
            ["docker", "exec", container, "rnode", "--grpc-host", "localhost", "deploy",
             "--phlo-limit", "1000000", "--phlo-price", "1",
             "--private-key", DEPLOYER_PRIV, "--shard-id", status["shardId"],
             "--valid-after-block-number", str(status["latestBlockNumber"]),
             "/tmp/release-claim.rho"],
            text=True, capture_output=True, timeout=timeout)
        match = re.search(r"DeployId is: ([0-9a-fA-F]+)", result.stdout + result.stderr)
        if not match:
            raise ClaimError(
                f"deploy on {container} returned no deploy id: "
                f"{(result.stdout + result.stderr)[-800:]}")
        return match.group(1)

    def provenance(self) -> dict:
        def run(*args: str) -> str | None:
            try:
                return subprocess.check_output(args, cwd=ROOT, text=True,
                                               stderr=subprocess.DEVNULL).strip()
            except Exception:
                return None

        binary = run("docker", "exec", self.node(0), "sha256sum", "/usr/local/bin/rnode")
        image = run("docker", "inspect", "-f", "{{.Id}}", self.node(0))
        return {
            "source_sha": run("git", "rev-parse", "HEAD"),
            "source_dirty": bool(run("git", "status", "--porcelain")),
            "binary_sha256": binary.split()[0] if binary else None,
            "image_id": image,
            "profile": f"local-{self.validators}",
            "devnet_prefix": self.prefix,
        }


def public_target(manifest: dict) -> Target:
    nodes = manifest.get("nodes") or manifest.get("validators")
    if not isinstance(nodes, list) or not nodes:
        raise ClaimError("the public manifest names no nodes")
    clean = []
    for node in nodes:
        if not isinstance(node, dict) or not node.get("http"):
            raise ClaimError("every public node needs an http endpoint")
        clean.append({
            "name": node.get("name") or node.get("alias") or f"validator-{len(clean)}",
            "http": node["http"],
            "admin": node.get("admin") or node["http"],
        })
    return Target(clean)


def load_manifest(path: str | None) -> dict | None:
    if not path:
        return None
    return json.loads(Path(path).read_text())


def new_record(claim_id: str, title: str, environment: str, locality: str) -> dict:
    return {
        "schema": 1,
        "claim_id": claim_id,
        "claim_title": title,
        "environment": environment,
        "locality": locality,
        "started_at": utc_now(),
        "finished_at": None,
        "provenance": {},
        "preconditions": [],
        "harness": {},
        "observations": {},
        "verdict": None,
        "reasons": [],
        "errors": [],
        "files": {},
    }


def finish(record: dict, verdict: str, reasons=None, errors=None) -> dict:
    record["verdict"] = verdict
    record["reasons"] = list(reasons or [])
    record["errors"] = list(errors or [])
    record["finished_at"] = utc_now()
    return record


def write_record(out: str | Path, record: dict) -> tuple[str, int]:
    path = Path(out)
    path.mkdir(parents=True, exist_ok=True)
    (path / "claim.json").write_text(json.dumps(record, indent=2, sort_keys=True) + "\n")
    code = exit_code_for(record.get("verdict"))
    print(json.dumps({"claim": record.get("claim_id"),
                      "environment": record.get("environment"),
                      "verdict": record.get("verdict"),
                      "reasons": record.get("reasons"),
                      "record": str(path / "claim.json")}))
    return str(path / "claim.json"), code


def exit_code_for(verdict: str | None) -> int:
    return {"pass": 0, "fail": 1, "not run": 3}.get(verdict or "", 1)


def not_run(out: str | Path, record: dict, reason: str) -> tuple[str, int]:
    record["preconditions"].append(reason)
    finish(record, "not run", [reason])
    return write_record(out, record)


def evaluate(record: dict, problems: list[str], not_run_reasons: list[str] | None = None):
    """The common tail of every `check`: a precondition is not run, a problem is fail."""
    if not_run_reasons:
        return "not run", not_run_reasons
    if problems:
        return "fail", problems
    return "pass", []


def expect(record: dict, key: str, kind: type, problems: list[str], where: str = ""):
    value = record.get(key)
    if not isinstance(value, kind):
        problems.append(f"{where or key}: missing or not {kind.__name__}")
        return None
    return value
