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
        # **Decision traces, for the phase-space reading.** A claim failure then carries the merge
        # inputs, not only the outcome. Production never sets this.
        trace_pair = "RNODE_TRACE_DIR=/var/lib/rnode/traces"
        node_env = self.node_env
        if node_env:
            separator = ";" if ";" in node_env else ","
            node_env = f"{node_env}{separator}{trace_pair}"
        else:
            node_env = trace_pair
        env["DEVNET_NODE_ENV"] = node_env
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
            "the devnet is ready on every validator",
            lambda: all(self._ready(i) for i in range(self.validators)), timeout=300)

    def _ready(self, index: int) -> bool:
        """HTTP is up **and** the shard is serving.

        A joiner answers `/api/v1/status` while its catch-up gate is still closed: the log says
        "not served until a peer probe confirms the catch-up window", and `/health` reports
        `ready=false, readyReasons=['shard_not_running']` for the next probe interval. A drill that
        starts at HTTP-200 reads that as a precondition failure on a node that was about to serve.
        Wait for the node's own readiness verdict instead.
        """
        base = f"http://127.0.0.1:{self.http_base + index * 1000}"
        try:
            status, _ = http_json(f"{base}/api/v1/status", timeout=3)
            if status != 200:
                return False
            _, health = http_json(f"{base}/health", timeout=3)
            return isinstance(health, dict) and health.get("ready") is True
        except Exception:
            return False

    def down(self) -> None:
        self.shell("down", "-v", timeout=300)

    def copy_traces(self, index: int, dest: Path) -> int:
        """Copy one node's decision traces out of its container into `dest`."""
        dest.mkdir(parents=True, exist_ok=True)
        result = subprocess.run(
            ["docker", "cp", f"{self.node(index)}:/var/lib/rnode/traces/.", str(dest)],
            text=True, capture_output=True)
        return result.returncode

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


def arm_of(record: dict) -> str:
    """The arm a record belongs to; the baseline arms use the environment name."""
    return str(record.get("arm") or record.get("environment") or "")


def dispatch_arm(arms: dict, record: dict) -> tuple[str, list[str]]:
    """Run the arm's own check. An unknown arm is a fail, never a guessed baseline."""
    arm = arm_of(record)
    handler = arms.get(arm)
    if handler is None:
        return "fail", [f"unknown arm {arm!r}; expected one of {sorted(arms)}"]
    return handler(record)


def claim_arm(claim_id: str, arm: str) -> dict:
    """The authored arm row, so a test's runner cannot disagree with the tracker."""
    document = json.loads((Path(__file__).resolve().parents[1] / "spec" / "audit" /
                           "release-claims.json").read_text())
    row = next((entry for entry in document["claims"] if entry.get("id") == claim_id), None)
    if row is None:
        raise ClaimError(f"unknown claim {claim_id}")
    entry = next((item for item in row.get("arms") or [] if item.get("id") == arm), None)
    if entry is None:
        raise ClaimError(f"{claim_id}: unknown arm {arm!r}")
    return entry


def new_record(claim_id: str, title: str, environment: str, locality: str,
               arm: str | None = None) -> dict:
    return {
        "schema": 1,
        "claim_id": claim_id,
        "claim_title": title,
        "arm": arm or environment,
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
                      "arm": arm_of(record),
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


class _Result:
    def __init__(self, returncode: int, stdout: str = "", stderr: str = ""):
        self.returncode = returncode
        self.stdout = stdout
        self.stderr = stderr


def preflight(environment: str, public_manifest: str | None = None, *,
              runner=None, head: str | None = None) -> list[str]:
    """Check the run's inputs before a container starts.

    A failure here is a `not run` with the missing coordinate, not a claim fail: the claim has not
    been tested. The checks are the ones that make an otherwise unexplainable run explainable — the
    image's revision, the tree it was built from, the manifest's provenance.
    """
    run = runner or (
        lambda command: subprocess.run(
            command, cwd=ROOT, text=True, capture_output=True))
    problems: list[str] = []
    allow_dirty = os.environ.get("RELEASE_ALLOW_DIRTY") == "1"

    if head is None:
        result = run(["git", "rev-parse", "HEAD"])
        head = result.stdout.strip() if result.returncode == 0 else None

    if environment == "local":
        docker = run(["docker", "info"])
        if docker.returncode != 0:
            problems.append("preflight: the docker daemon is unreachable")
        image = os.environ.get("RNODE_IMAGE", "rnode:local")
        inspect = run(["docker", "image", "inspect", image])
        if inspect.returncode != 0:
            problems.append(f"preflight: image {image} is missing; run tools/devnet.sh build")
        else:
            label = run([
                "docker", "image", "inspect", "--format",
                '{{ index .Config.Labels "org.opencontainers.image.revision" }}', image,
            ]).stdout.strip()
            if not label:
                problems.append(
                    f"preflight: image {image} carries no org.opencontainers.image.revision "
                    "label; rebuild it with tools/devnet.sh build")
            elif head and label != head:
                problems.append(
                    f"preflight: image {image} is revision {label[:12]}, not HEAD {head[:12]}; "
                    "rebuild it or check out the revision the image was built from")
        if not allow_dirty:
            status = run(["git", "status", "--porcelain"]).stdout
            if status.strip():
                problems.append(
                    "preflight: the working tree is dirty, so the image may not contain this "
                    "source; commit, or set RELEASE_ALLOW_DIRTY=1 only if it does")
    elif environment == "public":
        if not public_manifest:
            problems.append("preflight: the public run needs --public-manifest")
        else:
            path = Path(public_manifest)
            if not path.exists():
                problems.append(f"preflight: manifest {public_manifest} does not exist")
            else:
                try:
                    manifest = json.loads(path.read_text())
                except ValueError as error:
                    problems.append(f"preflight: manifest {public_manifest} is not JSON: {error}")
                else:
                    sha = (manifest.get("provenance") or {}).get("source_sha")
                    if sha and head and sha != head:
                        problems.append(
                            f"preflight: the manifest's provenance source_sha {sha[:12]} is not "
                            f"HEAD {head[:12]}")
    return problems


def preflight_failure(claim_id: str, environment: str, out, record: dict,
                      public_manifest: str | None = None) -> int | None:
    """Write a `not run` record when the preflight refuses; `None` means proceed."""
    problems = preflight(environment, public_manifest)
    if not problems:
        return None
    record["claim_id"] = claim_id
    return not_run(out, record, "; ".join(problems))[1]


# --------------------------------------------------------------------------------------
# The phase-space evaluator. Every predicate is recomputed from the trace coordinates; the
# node's own `predicates`/`bounded`/`accepted` labels are never trusted.
# --------------------------------------------------------------------------------------

TRACE_KINDS = ("merge", "finality", "state", "boot", "admission", "audit", "format")

BOUNDED_ADMISSION_CLASSES = {"full", "byte-full", "deployer-share", "rate-limited", "inflight"}


def trace_kind(trace: dict) -> str | None:
    kind = trace.get("kind")
    if isinstance(kind, str) and kind in TRACE_KINDS:
        return kind
    # Records written before `kind` existed.
    if "mode" in trace:
        return "merge"
    if "acquisition" in trace or "dag_len" in trace:
        return "boot"
    return None


def _integer(value) -> int | None:
    return value if type(value) is int else None


def _string_set(value) -> set[str]:
    return {item for item in (value or []) if isinstance(item, str)}


def _predicate_result(predicates: dict[str, bool | None]) -> dict:
    computable = {name: value for name, value in predicates.items() if value is not None}
    return {
        "predicates": computable,
        "missing": [name for name, value in predicates.items() if value is None],
        "false": [name for name, value in computable.items() if not value],
    }


def evaluate_merge_trace(trace: dict) -> dict:
    """The merge decision: base agreement, the fringe fork rule, the finalised floor, the split."""
    mode = trace.get("mode")
    claims = [c for c in (trace.get("claims") or []) if isinstance(c, dict)]
    hashes = {c.get("hash") for c in claims}
    base_ok = trace.get("certified_base") is not None or len(hashes) <= 1
    fork_ok = mode != "fringe" or not (trace.get("fork_blocks") or [])
    conflict = set(trace.get("conflict_scope") or [])
    # The floor reads user-deploy rejections. An older trace has no such field; fall back to the
    # full rejected-host set so the older record is not read as vacuously safe.
    rejected_user = trace.get("rejected_user_hosts")
    if rejected_user is None:
        rejected_user = trace.get("rejected_hosts") or []
    ledger_ok = mode != "fringe" or not (conflict & set(rejected_user))
    report = trace.get("report") or {}
    scope_partition_ok = (
        report.get("conflict_chains")
        == (report.get("kept_chains") or 0) + (report.get("rejected_chains") or 0))
    return _predicate_result({
        "base_ok": base_ok,
        "fork_ok": fork_ok,
        "ledger_ok": ledger_ok,
        "scope_partition_ok": scope_partition_ok,
    })


def evaluate_finality_trace(trace: dict) -> dict:
    """The finality advance: denominator, partition membership, supermajority support."""
    bonded = _integer(trace.get("bonded_total"))
    quorum = _integer(trace.get("quorum_total"))
    supporting = _integer(trace.get("supporting"))
    partition = _string_set(trace.get("partition"))
    bonded_senders = _string_set(trace.get("bonded_senders"))
    advanced = trace.get("advanced") is True
    reason = (trace.get("reason_kind") or "").lower()
    denominator_ok = None if bonded is None or quorum is None else quorum == bonded
    partition_ok = None if not bonded_senders else partition <= bonded_senders
    if supporting is None or quorum is None:
        support_ok = None
    elif advanced:
        support_ok = 3 * supporting > 2 * quorum
    elif reason.startswith("support"):
        support_ok = 3 * supporting <= 2 * quorum
    else:
        support_ok = True
    return _predicate_result({
        "denominator_ok": denominator_ok,
        "support_ok": support_ok,
        "partition_ok": partition_ok,
    })


def evaluate_state_trace(trace: dict) -> dict:
    """A block's state decision: an accepted block's claim equals the derived state and ledger."""
    verdict = trace.get("verdict") or ""
    accepted = verdict == "matched"
    claimed_pre, computed_pre = trace.get("claimed_pre"), trace.get("computed_pre")
    claimed_post = trace.get("claimed_post")
    replayed_post = trace.get("replayed_post", trace.get("computed_post"))
    claimed_rejected = _string_set(trace.get("claimed_rejected"))
    computed_rejected = _string_set(trace.get("computed_rejected"))
    return _predicate_result({
        "pre_ok": True if not accepted else claimed_pre == computed_pre,
        "post_ok": True if not accepted else claimed_post == replayed_post,
        "rejected_ok": True if not accepted else claimed_rejected == computed_rejected,
    })


def evaluate_boot_trace(trace: dict) -> dict:
    """The boot decision: genesis, acquisition, quorum verification and the anchor closure."""
    genesis_only = trace.get("genesis_only") is True
    accepted = trace.get("accepted") is True
    acquisition = (trace.get("acquisition") or "").lower()
    outcome = (trace.get("outcome") or "").lower()
    stage = trace.get("stage") or "acquisition"
    genesis_ok = (not genesis_only) or accepted or outcome == "genesis"
    acquisition_ok = genesis_only or not accepted or acquisition in (
        "certificate", "stored", "fetched")
    supporting = _integer(trace.get("served_supporting_stake")) or 0
    total = _integer(trace.get("served_total_stake")) or 0
    verification_ok = genesis_only or not accepted or total <= 0 or 3 * supporting > 2 * total
    if stage == "decision" and not genesis_only:
        # **A decision trace is written after the agreement decision, so its `accepted` is the
        # outcome, not an input.** Read the anchor predicates even when the boot refused: a quorate
        # certificate that does not anchor the accepted fringe is an unsafe point, not a model gap.
        #
        # **A light join is the safe way out of that mismatch** (join ladder, RC-4): the node did
        # not run from the unanchored fringe; the sync restored the certified anchor, so
        # `anchor_closure_present` is true and `recovery` names the join. The predicate is the
        # *run state*, not which fringe the agreement preferred — a mismatch with no recovery is
        # still unsafe.
        recovery = (trace.get("recovery") or "").lower()
        anchor_ok = (trace.get("anchor_closure_present") is True
                     and (trace.get("fringe_agrees") is True or recovery == "light-join"))
    else:
        anchor_ok = True
    return _predicate_result({
        "genesis_ok": genesis_ok,
        "acquisition_ok": acquisition_ok,
        "verification_ok": verification_ok,
        "anchor_ok": anchor_ok,
    })


def evaluate_admission_trace(trace: dict) -> dict:
    """An admission refusal: bounded class, a visible retry signal, no permit overrun."""
    refused = trace.get("refused") is True
    refusal_class = (trace.get("class") or "").lower()
    status = _integer(trace.get("status"))
    code = _integer(trace.get("grpc_code"))
    retry_after = trace.get("retry_after")
    server_error = status in (500, 502, 503, 504) or code in (13, 14)
    bounded_ok = None if (refused and not refusal_class) else (
        True if not refused else refusal_class in BOUNDED_ADMISSION_CLASSES)
    # Recompute the visibility from the wire, not from the node's `bounded` label.
    retry_signal_ok = True
    if refused:
        if server_error:
            retry_signal_ok = False
        elif status == 429 or code == 8:
            retry_signal_ok = retry_after not in (None, "", 0)
    inflight, inflight_limit = trace.get("inflight"), trace.get("inflight_limit")
    permit_ok = (True if not (type(inflight) is int and type(inflight_limit) is int)
                 else inflight <= inflight_limit)
    pool_count, pool_limit = trace.get("pool_count"), trace.get("pool_limit")
    pool_bytes, byte_limit = trace.get("pool_bytes"), trace.get("byte_limit")
    pool_bound_ok = True
    if type(pool_count) is int and type(pool_limit) is int:
        pool_bound_ok = pool_bound_ok and pool_count <= pool_limit
    if type(pool_bytes) is int and type(byte_limit) is int:
        pool_bound_ok = pool_bound_ok and pool_bytes <= byte_limit
    return _predicate_result({
        "bounded_ok": bounded_ok,
        "retry_signal_ok": retry_signal_ok,
        "permit_ok": permit_ok,
        "pool_bound_ok": pool_bound_ok,
    })


def evaluate_audit_trace(trace: dict) -> dict:
    """The audit target: the anchor is in the fringe, the target is its anchored shape, replay agrees."""
    anchor = trace.get("anchor")
    target_hashes = [row for row in (trace.get("target_hashes") or []) if isinstance(row, str)]
    anchor_membership_ok = None if "anchor_in_fringe" not in trace else (
        trace.get("anchor_in_fringe") is True)
    target_shape_ok = (
        bool(anchor) and target_hashes == [anchor]
        and trace.get("target_state") == trace.get("anchor_post_state"))
    verdict = (trace.get("verdict") or "").upper()
    derived, claimed = trace.get("derived_root"), trace.get("claimed_root")
    if verdict == "AGREE":
        replay_agrees_ok = derived is not None and derived == claimed
    elif verdict == "DISAGREE":
        # The audit ran and refused the claim: a safe refusal, not an unsafe acceptance.
        replay_agrees_ok = True
    else:
        replay_agrees_ok = False
    return _predicate_result({
        "anchor_membership_ok": anchor_membership_ok,
        "target_shape_ok": target_shape_ok,
        "replay_agrees_ok": replay_agrees_ok,
    })


def evaluate_format_trace(trace: dict) -> dict:
    """The store-format gate: the decision is the arithmetic one, and a stamp is durable."""
    present = _integer(trace.get("on_disk"))
    build = _integer(trace.get("build"))
    decision = (trace.get("decision") or "").lower()
    if build is None:
        expected = None
    elif decision == "stamp":
        expected = present is None
    elif decision == "accept":
        expected = present == build
    elif decision == "advance":
        expected = present is not None and present < build
    elif decision == "refuse_newer":
        expected = present is not None and present > build
    elif decision == "refuse_unreadable":
        expected = present is None
    else:
        expected = False
    # A newer marker must never be opened, whatever the decision is called.
    opened = decision in ("stamp", "accept", "advance")
    format_ok = (None if build is None else
                 (expected is True and not (opened and present is not None and present > build)))
    stamped = trace.get("stamped") is True
    stamp_ok = True if not stamped else trace.get("durable") is True
    return _predicate_result({"format_ok": format_ok, "stamp_ok": stamp_ok})


EVALUATORS = {
    "merge": evaluate_merge_trace,
    "finality": evaluate_finality_trace,
    "state": evaluate_state_trace,
    "boot": evaluate_boot_trace,
    "admission": evaluate_admission_trace,
    "audit": evaluate_audit_trace,
    "format": evaluate_format_trace,
}


def evaluate_trace(trace: dict) -> dict:
    kind = trace_kind(trace)
    if kind is None:
        return {"kind": "unknown", "predicates": {}, "missing": [],
                "false": [], "error": "unknown trace kind"}
    result = EVALUATORS[kind](trace)
    result["kind"] = kind
    return result


def traces_from(record: dict) -> list[dict]:
    """Every decision trace a record carries, from any of the observation lists."""
    observations = record.get("observations")
    if not isinstance(observations, dict):
        observations = {}
    rows: list[dict] = []
    for key in ("traces", "boot_traces", "finality_traces", "state_traces",
                "admission_traces", "audit_traces", "format_traces"):
        value = observations.get(key)
        if isinstance(value, list):
            rows.extend(row for row in value if isinstance(row, dict))
    if not rows:
        value = record.get("traces")
        if isinstance(value, list):
            rows.extend(row for row in value if isinstance(row, dict))
    return rows


def evaluate_state_agreement(traces: list[dict]) -> list[str]:
    """The cross-validator half of RC-3: one computed triple per block hash."""
    by_block: dict[str, set[tuple]] = {}
    for trace in traces:
        if trace_kind(trace) != "state":
            continue
        block = trace.get("block")
        if not isinstance(block, str):
            continue
        triple = (trace.get("computed_pre"), trace.get("computed_post"),
                  tuple(sorted(_string_set(trace.get("computed_rejected")))))
        by_block.setdefault(block, set()).add(triple)
    return [block for block, triples in by_block.items() if len(triples) > 1]


# --------------------------------------------------------------------------------------
# The arms beyond the baseline local/public runs. Each is a named behaviour of an existing
# claim: the fixture is a safe synthetic record, the check is the arm's observational
# contract, and the carried trace keeps the claim's phase-space reading. The live harness for
# an arm lands in the PR its `next_pr` names; until then `run` records a precondition.
# --------------------------------------------------------------------------------------

def _arm_record(claim_id: str, title: str, arm: str, environment: str,
                locality: str) -> dict:
    return {
        "schema": 1, "claim_id": claim_id, "claim_title": title, "arm": arm,
        "environment": environment, "locality": locality,
        "provenance": {"source_sha": "0" * 40, "binary_sha256": "1" * 64},
        "preconditions": [], "verdict": "pass", "reasons": [], "errors": [],
        "harness": {"name": arm, "exit_code": 0},
        "observations": {},
    }


def _safe_merge_trace(validator: int = 0) -> dict:
    return {
        "kind": "merge", "validator": validator, "parents": ["ab" * 32], "mode": "fringe",
        "claims": [{"height": 3, "id": "ab" * 32, "hash": "cd" * 32}],
        "certified_base": None, "base_source": "agreed",
        "conflict_scope": ["ef" * 32], "fork_blocks": [], "rejected_hosts": [],
        "report": {"conflict_chains": 1, "kept_chains": 0, "rejected_chains": 1},
    }


def _safe_finality_trace(validator: int = 0) -> dict:
    return {
        "kind": "finality", "validator": validator,
        "bonded_total": 400, "live_total": 400, "quorum_total": 400,
        "supporting": 300, "full_partitions": 3, "candidates": 3, "supported_tip": 40,
        "partition": ["aa", "bb", "cc", "dd"],
        "bonded_senders": ["aa", "bb", "cc", "dd"],
        "prev_fringe": [], "new_fringe": ["aa"], "advanced": True, "reason_kind": None,
    }


def _safe_state_trace(validator: int = 0) -> dict:
    return {
        "kind": "state", "validator": validator, "block": "ab" * 32,
        "parents": ["cd" * 32],
        "claimed_pre": "aa" * 32, "computed_pre": "aa" * 32,
        "claimed_post": "bb" * 32, "replayed_post": "bb" * 32,
        "claimed_rejected": [], "computed_rejected": [], "verdict": "matched",
    }


def _safe_boot_trace(validator: int = 1) -> dict:
    return {
        "kind": "boot", "stage": "acquisition", "validator": validator,
        "acquisition": "certificate", "accepted": True, "genesis_only": False,
        "outcome": "certified", "dag_len": 40, "store_head": None,
        "source_configured": True, "shard_id": "root",
        "served_supporting_stake": 300, "served_total_stake": 400,
    }


def _safe_audit_trace() -> dict:
    return {
        "kind": "audit", "digest": "ab" * 32, "anchor": "cd" * 32,
        "anchor_in_fringe": True, "target_hashes": ["cd" * 32],
        "target_state": "ef" * 32, "anchor_post_state": "ef" * 32,
        "verdict": "AGREE", "derived_root": "ef" * 32, "claimed_root": "ef" * 32,
    }


def _safe_admission_trace() -> dict:
    return {
        "kind": "admission", "site": "http-inflight", "surface": "http",
        "refused": True, "class": "inflight", "status": 429, "retry_after": 1,
        "inflight": 2, "inflight_limit": 2,
    }


def _arm_body(record: dict, key: str):
    observations = record.get("observations")
    if not isinstance(observations, dict):
        return None, ["observations: missing or not an object"]
    body = observations.get(key)
    if not isinstance(body, dict):
        return None, [f"{key}: missing or not an object"]
    return body, []


def bond_leave_fixture(claim_id: str, title: str) -> dict:
    record = _arm_record(claim_id, title, "bond-leave", "local", "local-devnet")
    record["observations"] = {
        "bond_leave": {
            "bond_deploy": "aa" * 32, "withdraw_deploy": "bb" * 32,
            "bond_settled": True, "withdraw_settled": True,
            "epoch_length": 10,
            "activation_height": 10, "withdraw_request_height": 20,
            "quarantine_height": 30, "paid_height": 40,
            "active_at_boundary": True, "paid_after_quarantine": True,
            "marker_retained": [True, True, True, True],
            "settlements": [{"validator": i, "stage": "finalizedSuccess",
                             "settlementConfirmed": True} for i in range(4)],
        },
        "traces": [_safe_merge_trace()],
    }
    return record


def check_bond_leave(record: dict) -> tuple[str, list[str]]:
    if record.get("verdict") == "not run":
        return "not run", list(record.get("preconditions") or record.get("reasons") or [])
    body, problems = _arm_body(record, "bond_leave")
    if body is None:
        return "fail", problems
    for key in ("bond_settled", "withdraw_settled", "active_at_boundary",
                "paid_after_quarantine"):
        if body.get(key) is not True:
            problems.append(f"bond_leave.{key} is not true")
    heights = [body.get(key) for key in
               ("activation_height", "withdraw_request_height", "quarantine_height",
                "paid_height")]
    if not all(type(value) is int for value in heights):
        problems.append("bond_leave: a boundary height is missing or not an integer")
    else:
        activation, request, quarantine, paid = heights
        if not activation <= request < quarantine < paid:
            problems.append("bond_leave: the boundaries are not activation <= request < "
                            "quarantine < payment")
        epoch = body.get("epoch_length")
        if type(epoch) is not int or epoch <= 0:
            problems.append("bond_leave: the epoch length is missing")
        elif activation % epoch != 0:
            problems.append("bond_leave: the bond did not activate at an epoch boundary")
    retained = body.get("marker_retained")
    if not isinstance(retained, list) or len(retained) < 4 or not all(retained):
        problems.append("bond_leave: the settlement marker was not retained on every validator")
    settlements = body.get("settlements")
    confirmed = ([row for row in settlements
                  if isinstance(row, dict) and row.get("stage") == "finalizedSuccess"
                  and row.get("settlementConfirmed") is True]
                 if isinstance(settlements, list) else [])
    if len(confirmed) < 4:
        problems.append("bond_leave: the bond or withdrawal did not settle on every validator")
    problems.extend(phase_space_reading(record.get("observations") or {}, bool(problems)))
    return evaluate(record, problems)


def validator_set_fixture(claim_id: str, title: str) -> dict:
    bonds = {"aa": 100, "bb": 100, "cc": 100, "dd": 100}
    record = _arm_record(claim_id, title, "validator-set", "local", "local-devnet")
    record["observations"] = {
        "validator_set": {
            "boundaries": [
                {"height": 10, "sets": [{"validator": i, "bonds": dict(bonds)}
                                       for i in range(4)]},
                {"height": 20, "sets": [{"validator": i, "bonds": dict(bonds)}
                                       for i in range(4)]},
            ],
            "denominators_match": True, "active_set_agrees": True,
            "finality_denominator": 400,
        },
        "finality_traces": [_safe_finality_trace()],
    }
    return record


def check_validator_set(record: dict) -> tuple[str, list[str]]:
    if record.get("verdict") == "not run":
        return "not run", list(record.get("preconditions") or record.get("reasons") or [])
    body, problems = _arm_body(record, "validator_set")
    if body is None:
        return "fail", problems
    if body.get("denominators_match") is not True:
        problems.append("validator_set: the finality denominators do not match the boundary set")
    if body.get("active_set_agrees") is not True:
        problems.append("validator_set: the validators do not agree on the boundary bond set")
    boundaries = body.get("boundaries")
    if not isinstance(boundaries, list) or not boundaries:
        problems.append("validator_set: no epoch boundary was observed")
        boundaries = []
    for boundary in boundaries:
        if not isinstance(boundary, dict):
            problems.append("validator_set: a boundary row is not an object")
            continue
        sets = boundary.get("sets")
        if not isinstance(sets, list) or len(sets) < 4:
            problems.append(f"validator_set: boundary {boundary.get('height')} was not read "
                            "on every validator")
            continue
        values = [row.get("bonds") for row in sets if isinstance(row, dict)]
        if len(values) < 4 or any(value != values[0] for value in values):
            problems.append(f"validator_set: boundary {boundary.get('height')} has different "
                            "bond sets across validators")
        if values and isinstance(values[0], dict):
            total = sum(value for value in values[0].values() if type(value) is int)
            if body.get("finality_denominator") != total:
                problems.append("validator_set: the finality denominator is not the boundary "
                                "bond total")
    problems.extend(phase_space_reading(record.get("observations") or {}, bool(problems)))
    return evaluate(record, problems)


def returner_fixture(claim_id: str, title: str) -> dict:
    record = _arm_record(claim_id, title, "returner", "local", "local-devnet")
    record["observations"] = {
        "returner": {
            "victim": 3, "down_at_height": 15, "rejoin_height": 40,
            "heads_agree": True, "new_boot_id": True, "manual_surgery": False,
            "finalised": [{"validator": i, "height": 40, "hash": "cd" * 32}
                          for i in range(4)],
        },
        "boot_traces": [_safe_boot_trace(3)],
    }
    return record


def check_returner(record: dict) -> tuple[str, list[str]]:
    if record.get("verdict") == "not run":
        return "not run", list(record.get("preconditions") or record.get("reasons") or [])
    body, problems = _arm_body(record, "returner")
    if body is None:
        return "fail", problems
    if body.get("new_boot_id") is not True:
        problems.append("returner: the returning validator did not restart")
    if body.get("manual_surgery") is not False:
        problems.append("returner: the returning validator needed manual surgery")
    if body.get("heads_agree") is not True:
        problems.append("returner: the returning validator did not reach the same finalised head")
    down, rejoin = body.get("down_at_height"), body.get("rejoin_height")
    if not (type(down) is int and type(rejoin) is int and rejoin > down):
        problems.append("returner: the returner did not rejoin after the boundary it missed")
    rows = body.get("finalised")
    if not isinstance(rows, list) or len(rows) < 4:
        problems.append("returner: the finalised head was not read on every validator")
    else:
        identities = {(row.get("height"), row.get("hash")) for row in rows
                      if isinstance(row, dict)}
        if len(identities) > 1:
            problems.append(f"returner: validators disagree on the finalised head: {identities}")
    problems.extend(phase_space_reading(record.get("observations") or {}, bool(problems)))
    return evaluate(record, problems)


def availability_fixture(claim_id: str, title: str) -> dict:
    record = _arm_record(claim_id, title, "availability", "public", "public-live")
    record["observations"] = {
        "availability": {
            "blocks": [
                {"height": height, "hash": f"{height:064x}",
                 "validators": [{"validator": i, "body_present": True,
                                 "state_served": True, "error": None} for i in range(4)]}
                for height in range(30, 41)
            ],
            "not_available": [],
        },
        "state_traces": [_safe_state_trace()],
    }
    return record


def check_availability(record: dict) -> tuple[str, list[str]]:
    if record.get("verdict") == "not run":
        return "not run", list(record.get("preconditions") or record.get("reasons") or [])
    body, problems = _arm_body(record, "availability")
    if body is None:
        return "fail", problems
    blocks = body.get("blocks")
    if not isinstance(blocks, list) or len(blocks) < 10:
        problems.append("availability: fewer than ten finalised blocks were read")
        blocks = []
    for block in blocks:
        if not isinstance(block, dict):
            problems.append("availability: a block row is not an object")
            continue
        rows = block.get("validators")
        if not isinstance(rows, list) or len(rows) < 4:
            problems.append(f"availability: block {block.get('height')} was not read on every "
                            "validator")
            continue
        for row in rows:
            if not isinstance(row, dict) or row.get("body_present") is not True:
                problems.append(f"availability: block {block.get('height')} body missing on "
                                f"validator {row.get('validator') if isinstance(row, dict) else row}")
            if not isinstance(row, dict) or row.get("state_served") is not True:
                problems.append(f"availability: block {block.get('height')} state not served on "
                                f"validator {row.get('validator') if isinstance(row, dict) else row}")
            if isinstance(row, dict) and row.get("error"):
                problems.append(f"availability: block {block.get('height')} returned an error: "
                                f"{row['error']}")
    if body.get("not_available"):
        problems.append(f"availability: finalised data was reported unavailable: "
                        f"{body['not_available']}")
    problems.extend(phase_space_reading(record.get("observations") or {}, bool(problems)))
    return evaluate(record, problems)


def transport_fixture(claim_id: str, title: str) -> dict:
    record = _arm_record(claim_id, title, "transport", "local", "local-devnet")
    surfaces = [
        {"surface": name, "deploy_id": "ab" * 32, "status": "ProcessedWithSuccess",
         "settlement_stage": "finalizedSuccess", "error_class": None}
        for name in ("http", "direct-http", "grpc", "proxy")
    ]
    record["observations"] = {
        "transport": {"deploy_id": "ab" * 32, "surfaces": surfaces},
        "admission_traces": [_safe_admission_trace()],
    }
    return record


def check_transport(record: dict) -> tuple[str, list[str]]:
    if record.get("verdict") == "not run":
        return "not run", list(record.get("preconditions") or record.get("reasons") or [])
    body, problems = _arm_body(record, "transport")
    if body is None:
        return "fail", problems
    surfaces = body.get("surfaces")
    if not isinstance(surfaces, list):
        problems.append("transport: surfaces is missing or not a list")
        surfaces = []
    names = {row.get("surface") for row in surfaces if isinstance(row, dict)}
    expected = {"http", "direct-http", "grpc", "proxy"}
    if names != expected:
        problems.append(f"transport: surfaces are {sorted(names)}, expected {sorted(expected)}")
    deploy_id = body.get("deploy_id")
    if not deploy_id:
        problems.append("transport: the deploy id is missing")
    triples = {(row.get("status"), row.get("settlement_stage"), row.get("error_class"))
               for row in surfaces if isinstance(row, dict)}
    if len(triples) > 1:
        problems.append(f"transport: the surfaces disagree on status/stage/error: {triples}")
    for row in surfaces:
        if not isinstance(row, dict):
            problems.append("transport: a surface row is not an object")
            continue
        if row.get("deploy_id") != deploy_id:
            problems.append(f"transport: {row.get('surface')} returned a different deploy id")
        if row.get("error_class") in ("5xx", "Internal"):
            problems.append(f"transport: {row.get('surface')} returned a 5xx class")
    problems.extend(phase_space_reading(record.get("observations") or {}, bool(problems)))
    return evaluate(record, problems)


def offhost_fixture(claim_id: str, title: str) -> dict:
    record = _arm_record(claim_id, title, "off-host", "local", "off-host-claim")
    record["observations"] = {
        "off_host": {
            "host_a": "host-a.example:40403", "host_b": "host-b.example:40403",
            "advertised": "host-a.example:40403", "dial_back": True,
            "chain": {"request": True, "receipt": True, "construction": True,
                      "transport_delivered": True, "receive": True, "apply": True},
            "hashes": {"served": "ab" * 32, "received": "ab" * 32,
                       "restored": "ab" * 32, "walked": "ab" * 32},
            "loopback_advertisement_refused": True,
        },
        "audit_traces": [_safe_audit_trace()],
    }
    return record


def _is_loopback(host: str) -> bool:
    name = host.rsplit(":", 1)[0] if ":" in host else host
    return name in ("127.0.0.1", "localhost", "::1", "0.0.0.0")


def check_offhost(record: dict) -> tuple[str, list[str]]:
    if record.get("verdict") == "not run":
        return "not run", list(record.get("preconditions") or record.get("reasons") or [])
    body, problems = _arm_body(record, "off_host")
    if body is None:
        return "fail", problems
    host_a, host_b = body.get("host_a"), body.get("host_b")
    if not host_a or not host_b or host_a == host_b:
        problems.append("off_host: the joiner and bootstrap are not distinct hosts")
    advertised = body.get("advertised")
    if not advertised:
        problems.append("off_host: the bootstrap advertised no routable address")
    elif _is_loopback(str(advertised)):
        problems.append("off_host: the bootstrap advertised a loopback address")
    if body.get("dial_back") is not True:
        problems.append("off_host: the bootstrap did not dial the joiner back")
    chain = body.get("chain")
    stages = ("request", "receipt", "construction", "transport_delivered", "receive", "apply")
    if not isinstance(chain, dict) or any(chain.get(stage) is not True for stage in stages):
        problems.append("off_host: the six-stage join chain is incomplete")
    hashes = body.get("hashes")
    if not isinstance(hashes, dict):
        problems.append("off_host: the join hashes are missing")
    else:
        values = [hashes.get(stage) for stage in ("served", "received", "restored", "walked")]
        if any(not value for value in values) or len(set(values)) != 1:
            problems.append(f"off_host: the join hashes disagree: {values}")
    if body.get("loopback_advertisement_refused") is not True:
        problems.append("off_host: a loopback advertisement was not refused")
    problems.extend(phase_space_reading(record.get("observations") or {}, bool(problems)))
    return evaluate(record, problems)


def failover_fixture(claim_id: str, title: str) -> dict:
    record = _arm_record(claim_id, title, "failover", "local", "off-host-claim")
    record["observations"] = {
        "failover": {
            "seeds": ["host-a.example:40403", "host-b.example:40403"],
            "seed_status": ["down", "live"], "joined": True,
            "joined_via": "host-b.example:40403", "finalised_head_agrees": True,
        },
        "boot_traces": [_safe_boot_trace(3)],
    }
    return record


def check_failover(record: dict) -> tuple[str, list[str]]:
    if record.get("verdict") == "not run":
        return "not run", list(record.get("preconditions") or record.get("reasons") or [])
    body, problems = _arm_body(record, "failover")
    if body is None:
        return "fail", problems
    seeds = body.get("seeds")
    status = body.get("seed_status")
    if not isinstance(seeds, list) or len(seeds) < 2:
        problems.append("failover: the node was given fewer than two seeds")
        seeds = []
    if not isinstance(status, list) or len(status) < 2:
        problems.append("failover: the seed statuses were not recorded")
        status = []
    if status[:1] != ["down"] or status[1:2] != ["live"]:
        problems.append(f"failover: the seed order was not down-then-live: {status}")
    if body.get("joined") is not True:
        problems.append("failover: the joiner did not join through an alternative seed")
    if seeds and body.get("joined_via") != seeds[1]:
        problems.append("failover: the joiner did not use the second seed")
    if body.get("finalised_head_agrees") is not True:
        problems.append("failover: the joiner's finalised head does not agree")
    problems.extend(phase_space_reading(record.get("observations") or {}, bool(problems)))
    return evaluate(record, problems)


def _multi_host_fixture(claim_id: str, title: str, arm: str) -> dict:
    record = _arm_record(claim_id, title, arm, "local", "multi-host")
    record["observations"] = {
        "multi_host": {
            "declared_hosts": ["host-a", "host-b", "host-c", "host-d",
                               "host-e", "host-f", "host-g", "host-h"],
            "validators": 4, "sizes": [4, 8], "capacity_tier": "M1",
            "approved_root": "ab" * 32, "root_retained": True,
            "partition_observed": True, "host_loss_observed": True,
            "conflicting_finality": [], "safe_halt": True,
            "containment": [{"validator": v, "height": 40, "hash": "cd" * 32,
                             "answers": [{"validator": w, "finalized": True}
                                         for w in range(4)]} for v in range(4)],
            "regression": [],
        },
        "finality_traces": [_safe_finality_trace()],
    }
    return record


def multi_host_finality_fixture(claim_id: str, title: str) -> dict:
    return _multi_host_fixture(claim_id, title, "multi-host")


def multi_host_recovery_fixture(claim_id: str, title: str) -> dict:
    record = _multi_host_fixture(claim_id, title, "multi-host")
    record["observations"]["multi_host"]["recovery"] = {
        "restarted": True, "replayed": True, "lost_deploys": [],
        "own_key_halt": False, "self_replay": False, "post_heads_agree": True,
    }
    record["observations"]["boot_traces"] = [_safe_boot_trace(3)]
    return record


def _check_multi_host_common(body: dict) -> list[str]:
    problems: list[str] = []
    hosts = body.get("declared_hosts")
    validators = body.get("validators")
    if not isinstance(hosts, list) or len(set(hosts)) < 4:
        problems.append("multi_host: fewer than four distinct declared hosts")
    if type(validators) is not int or validators < 4:
        problems.append("multi_host: the campaign did not run at least four validators")
    elif isinstance(hosts, list) and len(set(hosts)) < validators:
        problems.append("multi_host: a host is labelled as several validators")
    sizes = body.get("sizes")
    if not isinstance(sizes, list) or not {4, 8} <= set(sizes):
        problems.append("multi_host: the campaign did not cover N=4 and N=8")
    elif isinstance(hosts, list) and len(set(hosts)) < max(sizes):
        problems.append("multi_host: fewer distinct declared hosts than the largest "
                        "campaign size")
    if body.get("capacity_tier") != "M1":
        problems.append("multi_host: the capacity tier is not M1")
    root = body.get("approved_root")
    if not isinstance(root, str) or len(root) != 64:
        problems.append("multi_host: the approved root is missing")
    if body.get("root_retained") is not True:
        problems.append("multi_host: the approved root was not retained")
    if body.get("partition_observed") is not True:
        problems.append("multi_host: no partition was observed")
    if body.get("host_loss_observed") is not True:
        problems.append("multi_host: no host loss was observed")
    if body.get("conflicting_finality"):
        problems.append(f"multi_host: conflicting finality observed: "
                        f"{body['conflicting_finality']}")
    if body.get("safe_halt") is not True:
        problems.append("multi_host: the network did not halt safely beyond tolerance")
    containment = body.get("containment")
    if not isinstance(containment, list) or not containment:
        problems.append("multi_host: no containment answers were recorded")
    else:
        for row in containment:
            answers = row.get("answers") if isinstance(row, dict) else None
            if not isinstance(answers, list) or not answers or any(
                    not isinstance(entry, dict) or entry.get("finalized") is not True
                    for entry in answers):
                problems.append(f"multi_host: containment failed for validator "
                                f"{row.get('validator') if isinstance(row, dict) else row}")
    if body.get("regression"):
        problems.append(f"multi_host: finalised regression observed: {body['regression']}")
    return problems


def check_multi_host_finality(record: dict) -> tuple[str, list[str]]:
    if record.get("verdict") == "not run":
        return "not run", list(record.get("preconditions") or record.get("reasons") or [])
    body, problems = _arm_body(record, "multi_host")
    if body is None:
        return "fail", problems
    problems.extend(_check_multi_host_common(body))
    problems.extend(phase_space_reading(record.get("observations") or {}, bool(problems)))
    return evaluate(record, problems)


def check_multi_host_recovery(record: dict) -> tuple[str, list[str]]:
    if record.get("verdict") == "not run":
        return "not run", list(record.get("preconditions") or record.get("reasons") or [])
    body, problems = _arm_body(record, "multi_host")
    if body is None:
        return "fail", problems
    problems.extend(_check_multi_host_common(body))
    recovery = body.get("recovery")
    if not isinstance(recovery, dict):
        problems.append("multi_host: no recovery arm was recorded")
    else:
        for key in ("restarted", "replayed", "post_heads_agree"):
            if recovery.get(key) is not True:
                problems.append(f"multi_host.recovery.{key} is not true")
        if recovery.get("lost_deploys"):
            problems.append(f"multi_host: accepted deploys lost: {recovery['lost_deploys']}")
        if recovery.get("own_key_halt") is not False:
            problems.append("multi_host: an own-key halt fired")
        if recovery.get("self_replay") is not False:
            problems.append("multi_host: a spurious self-replay fired")
    problems.extend(phase_space_reading(record.get("observations") or {}, bool(problems)))
    return evaluate(record, problems)


def equivocation_fixture(claim_id: str, title: str) -> dict:
    record = _arm_record(claim_id, title, "equivocation", "local", "multi-host")
    record["observations"] = {
        "equivocation": {
            "equivocator": 2, "height": 50, "blocks": ["aa" * 32, "bb" * 32],
            "finalised_by": {"0": ["aa" * 32], "1": ["aa" * 32], "2": [],
                             "3": ["aa" * 32]},
            "conflicts": [], "detected": True, "slashing_enabled": False,
        },
        "finality_traces": [_safe_finality_trace()],
    }
    return record


def check_equivocation(record: dict) -> tuple[str, list[str]]:
    if record.get("verdict") == "not run":
        return "not run", list(record.get("preconditions") or record.get("reasons") or [])
    body, problems = _arm_body(record, "equivocation")
    if body is None:
        return "fail", problems
    blocks = body.get("blocks")
    if not isinstance(blocks, list) or len(set(blocks)) < 2:
        problems.append("equivocation: the injection did not sign two distinct blocks")
        blocks = []
    if type(body.get("height")) is not int:
        problems.append("equivocation: the equivocation height is missing")
    finalised = body.get("finalised_by")
    if not isinstance(finalised, dict):
        problems.append("equivocation: the per-validator finalised histories are missing")
        finalised = {}
    for validator, hashes in finalised.items():
        both = set(hashes or []) & set(blocks)
        if len(both) > 1:
            problems.append(f"equivocation: validator {validator} finalised both blocks at "
                            f"height {body.get('height')}")
    if body.get("conflicts"):
        problems.append(f"equivocation: conflicting finality observed: {body['conflicts']}")
    if body.get("detected") is not True:
        problems.append("equivocation: the fault was not detected")
    if body.get("slashing_enabled") is not False:
        problems.append("equivocation: slashing is enabled; the launch policy is detection only")
    problems.extend(phase_space_reading(record.get("observations") or {}, bool(problems)))
    return evaluate(record, problems)


def phase_space_reading(observations: dict, bad_outcome: bool) -> list[str]:
    """Read a claim's traces against its predicates.

    Returns problems only when the claim's own bad outcome is true: a passing run without traces
    must not be failed for the absence of diagnostics. `model-incomplete` means every predicate
    held and the bad state still happened, so the trace is where the model must extend.
    """
    traces = traces_from({"observations": observations})
    problems: list[str] = []
    if not traces:
        if bad_outcome:
            problems.append("evidence-insufficient: the failure carries no decision trace")
        return problems
    evaluations = [(trace, evaluate_trace(trace)) for trace in traces]
    for trace, evaluation in evaluations:
        if evaluation["false"]:
            problems.append(
                f"{evaluation['kind']} trace (validator {trace.get('validator')}, "
                f"site {trace.get('site') or trace.get('mode') or trace.get('stage') or '-'}): "
                f"predicate(s) {', '.join(evaluation['false'])} false")
        if evaluation["missing"]:
            problems.append(
                f"{evaluation['kind']} trace lacks coordinate(s): "
                f"{', '.join(evaluation['missing'])}")
    disagreed = evaluate_state_agreement(traces)
    if disagreed:
        problems.append(
            "agreement_ok false: validators derived different state for block(s) "
            + ", ".join(disagreed[:5]))
    if bad_outcome and not problems:
        problems.append(
            "model-incomplete: every predicate holds on the captured traces, yet the bad outcome "
            "was observed; the trace is the model gap")
    return problems
