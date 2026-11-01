#!/usr/bin/env python3
"""Measure finality and retention of one finalized write on a disposable devnet.

No quorum or consensus parameter is changed. The write is observed in finalized
post-state on every validator, then checked through two further epoch boundaries.
This is a finite execution witness, not a universal rollback-safety proof.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import time
import urllib.error
import uuid

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("devnet_fuzz", ROOT / "tools/devnet-fuzz.py")
fuzz = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fuzz)
PREFIX = os.environ.get("DEVNET_PREFIX", "acceptance")

# **The readiness vocabulary, mapped back to the verdict names the drill always used.** The node
# owns the vocabulary (`ready_reasons` in `node/src/diagnostics/health_contract.rs`); this is only
# the drill's naming of the same four terms, so a new reason a drill does not know surfaces as
# `FAIL_PRECONDITION_readyReasons` rather than being silently called "not ready".
def _is_positive(value):
    """A gate is set for the readiness vocabulary: a positive count, or the boolean `true`."""
    return value is True or (type(value) is int and value > 0)


READY_REASON_VERDICT_NAMES = {
    "shard_not_running": "shards",
    "pipeline_fault_outstanding": "pipelineFaultsOutstanding",
    "proposal_isolated": "proposalHalted",
    "orphan_body": "orphanBodies",
}
trace_spec = importlib.util.spec_from_file_location("causal_trace", ROOT / "tools/consensus-causal-trace.py")
causal_trace = importlib.util.module_from_spec(trace_spec)
trace_spec.loader.exec_module(causal_trace)
envelope_spec = importlib.util.spec_from_file_location("failure_envelope", ROOT / "tools/failure_envelope.py")
failure_envelope = importlib.util.module_from_spec(envelope_spec)
envelope_spec.loader.exec_module(failure_envelope)


class WaitTimeout(RuntimeError):
    """A bounded wait exhausted its window.

    Subclasses ``RuntimeError`` so existing callers and tests keep working; the
    message names the window actually used, which is not always ``args.window``.
    """


def node_name(i):
    return f"{PREFIX}-bootstrap" if i == 0 else f"{PREFIX}-validator-{i}"


def get(i, path, body=None, admin=False, timeout=10):
    port = (40405 if admin else 40403) + i * 1000
    return fuzz.http_json(f"http://127.0.0.1:{port}{path}",
                          "POST" if body is not None else "GET", body, timeout=timeout)[1]


def block_identity(value):
    if not isinstance(value, dict):
        raise ValueError("block response is not an object")
    height, block_hash = value.get("blockNumber"), value.get("blockHash")
    if type(height) is not int or height < 0:
        raise ValueError("block response has no nonnegative height")
    if not isinstance(block_hash, str) or len(block_hash) != 64:
        raise ValueError("block response has no 32-byte hash")
    bytes.fromhex(block_hash)
    return height, block_hash


def marker_present(response, expected):
    if not isinstance(response, dict) or not isinstance(response.get("expr"), list):
        raise ValueError("exploration did not return an expression list")
    values = response["expr"]
    return len(values) == 1 and values[0] == {"ExprInt": {"data": expected}}


def finalized_identity(response):
    # last-finalized-block returns BlockInfo, unlike blocks/1's LightBlockInfo.
    if not isinstance(response, dict) or "blockInfo" not in response:
        raise ValueError("finalized response has no blockInfo envelope")
    return block_identity(response["blockInfo"])


def probe(i, name, block_hash, timeout=10):
    term = f'new return in {{ for (@v <- @{json.dumps(name)}) {{ return!(v) }} }}'
    return get(i, "/api/v1/explore-deploy-by-block-hash",
               {"term": term, "blockHash": block_hash, "usePreStateHash": False},
               timeout=timeout)


class Drill:
    def __init__(self, args):
        self.args = args
        self.out = Path(args.out)
        self.out.mkdir(parents=True, exist_ok=True)
        self.events = self.out / "observations.jsonl"
        self.marker = "acceptance:" + uuid.uuid4().hex
        self.value = 42
        self.next_drive = 0

    def record(self, **event):
        with self.events.open("a") as f:
            f.write(json.dumps({"utc": time.time(), **event}) + "\n")

    def read_timeout(self):
        """Clamp a read to the active wait's remaining time (10 s outside a wait)."""
        deadline = getattr(self, "deadline", None)
        if deadline is None:
            return 10
        return max(1.0, min(10.0, deadline - time.monotonic()))

    def is_finalized(self, i, block_hash):
        """`GET /api/is-finalized/{hex}` — the only is-finalized oracle.

        It answers membership in the seen-closure of the node's latest fringe.
        A non-boolean body is *unknown*, never a pass: the caller's wait records
        it as a read error and retries.
        """
        answer = get(i, f"/api/is-finalized/{block_hash}", timeout=self.read_timeout())
        if type(answer) is not bool:
            raise ValueError(f"validator {i}: is-finalized did not answer a JSON boolean")
        return answer

    def finalized_head(self, i):
        return finalized_identity(get(i, "/api/last-finalized-block", timeout=self.read_timeout()))

    def status_row(self, i):
        """One row from the two bodies the drill already needs, each read from the surface that
        declares it.

        **The tip is `maxObservedHeight`, not `latestBlockNumber`.** The latter is one past the
        highest observed height (`DagRepresentation::latest_block_number` returns
        `max(height_map) + 1`); the diagnostics contract (`docs/src/spec/node-diagnostics.md` §4.1)
        publishes the actual tip as `maxObservedHeight` and warns that `height` is not it. Using the
        one-past value as a tip is the seam that made a lag of one look like a lag of two.

        The two calls stay: `ready`/`readyReasons` are health-only by the operator seam test, and the
        operator-only counters are not on `/health`. What changes is that no value is read twice.
        """
        operator = get(i, "/api/v1/status/operator", admin=True, timeout=self.read_timeout())
        health = get(i, "/health", timeout=self.read_timeout())
        return {
            "node": i,
            # Health view: readiness and the progress fence.
            "diag_version": health.get("diagVersion"),
            "ready": health.get("ready"),
            "ready_reasons": health.get("readyReasons"),
            "tip": health.get("maxObservedHeight"),
            "height": health.get("height"),
            "finalized_height": health.get("finalizedHeight"),
            "finalized_lag": health.get("finalizedLag"),
            "finalized_height_error": health.get("finalizedHeightError"),
            # Operator view: the gates the operator route carries, and operator-only state.
            "orphan_bodies": operator.get("orphanBodies"),
            "orphan_detections": operator.get("orphanDetections"),
            "pipeline_faults": operator.get("pipelineFaults"),
            "pipeline_faults_outstanding": operator.get("pipelineFaultsOutstanding"),
            "last_pipeline_fault": operator.get("lastPipelineFault"),
            "proposal_halted": operator.get("proposalHalted"),
            "proposal_halt_reason": operator.get("proposalHaltReason"),
            "finality_stalled": operator.get("finalityStalled"),
            "finality_stall_episodes": operator.get("finalityStallEpisodes"),
            "latest_block_number": operator.get("latestBlockNumber"),
            "peers": operator.get("peers"),
            "finality_counters_durable": operator.get("finalityCountersDurable"),
            "unrestorable_records": operator.get("unrestorableRecords"),
            "unrestorable_block": operator.get("unrestorableBlock"),
            "undelivered_sends": operator.get("undeliveredSends"),
            "cgroup_oom_kills": (operator.get("resources") or {}).get("cgroupOomKills"),
        }

    def health_precondition(self, phase="drill"):
        """A node that is not a clean subject is a named precondition, not a timeout.

        **Readiness is the node's answer, not the drill's re-derivation.** `ready` is the
        four-term conjunction and `readyReasons` is its closed vocabulary
        (`node/src/diagnostics/health_contract.rs`); the drill maps the code back to the field name
        it always used in a verdict, so the vocabulary cannot drift from the verdicts.

        Fail-closed: a missing counter is unknown, and unknown is not clean.
        """
        rows = [self.status_row(i) for i in range(self.args.validators)]
        for row in rows:
            reasons = row["ready_reasons"]
            if not isinstance(reasons, list):
                raise RuntimeError(
                    f"FAIL_PRECONDITION_ready: {phase} node {row['node']} "
                    f"ready={row['ready']!r} readyReasons={reasons!r} "
                    f"finalizedHeightError={row['finalized_height_error']!r}")
            # **Cross-checks, not re-derivations.** `ready` and `readyReasons` must agree, the tip
            # must be `height - 1`, and each reason the node names must match the operator gate it
            # points at. A mismatch means the diagnostics disagree with themselves; failing here
            # keeps the drill from trusting either side.
            if row["ready"] is True and reasons:
                raise RuntimeError(
                    f"FAIL_PRECONDITION_readyReasonsInconsistent: {phase} node {row['node']} "
                    f"ready=true with reasons={reasons!r}")
            if type(row["tip"]) is int and type(row["height"]) is int and row["height"] > 0:
                if row["tip"] != row["height"] - 1:
                    raise RuntimeError(
                        f"FAIL_PRECONDITION_tipMismatch: {phase} node {row['node']} "
                        f"maxObservedHeight={row['tip']} height={row['height']}")
            # The lag is `max(0, tip - fence)`; a literal copied into a test cannot catch a route
            # that computes it from the one-past `height` instead.
            if (type(row["tip"]) is int and type(row["finalized_height"]) is int
                    and type(row["finalized_lag"]) is int
                    and row["finalized_lag"] != max(0, row["tip"] - row["finalized_height"])):
                raise RuntimeError(
                    f"FAIL_PRECONDITION_finalizedLag: {phase} node {row['node']} "
                    f"finalizedLag={row['finalized_lag']} tip={row['tip']} "
                    f"finalizedHeight={row['finalized_height']}")
            for code, value in (
                ("pipeline_fault_outstanding", row["pipeline_faults_outstanding"]),
                ("proposal_isolated", row["proposal_halted"]),
                ("orphan_body", row["orphan_bodies"]),
            ):
                if (code in reasons) != _is_positive(value):
                    raise RuntimeError(
                        f"FAIL_PRECONDITION_readyReasonsInconsistent: {phase} node {row['node']} "
                        f"reason {code!r} does not match its operator gate {value!r}")
            if row["ready"] is not True:
                if reasons:
                    name = READY_REASON_VERDICT_NAMES.get(reasons[0], "readyReasons")
                    raise RuntimeError(
                        f"FAIL_PRECONDITION_{name}: {phase} node {row['node']} "
                        f"readyReasons={reasons!r} "
                        f"lastPipelineFault={row['last_pipeline_fault']!r} "
                        f"proposalHaltReason={row['proposal_halt_reason']!r}")
                # False with no reason is the defect the closed vocabulary exists to forbid.
                raise RuntimeError(
                    f"FAIL_PRECONDITION_ready: {phase} node {row['node']} "
                    f"ready=False with an empty readyReasons")
            checks = (
                # Not a readiness term: a stalled subject is still not a clean drill subject.
                ("finalityStalled", row["finality_stalled"] is False),
                # The tip is the contract's `maxObservedHeight`; unknown is not zero and not clean.
                ("tipUnknown", type(row["tip"]) is int),
                ("unrestorableRecords",
                 type(row["unrestorable_records"]) is int and row["unrestorable_records"] == 0),
                ("finalityCountersDurable", row["finality_counters_durable"] is True),
                ("cgroupOomKills",
                 type(row["cgroup_oom_kills"]) is int and row["cgroup_oom_kills"] == 0),
            )
            for name, ok in checks:
                if not ok:
                    raise RuntimeError(
                        f"FAIL_PRECONDITION_{name}: {phase} node {row['node']} "
                        f"tip={row['tip']!r} height={row['height']!r} "
                        f"pipelineFaults={row['pipeline_faults']} "
                        f"lastPipelineFault={row['last_pipeline_fault']!r} "
                        f"finalityStalled={row['finality_stalled']} "
                        f"finalityCountersDurable={row['finality_counters_durable']}")
        return rows

    def snapshot(self):
        results = []
        for i in range(self.args.validators):
            status = get(i, "/api/v1/status/operator", admin=True, timeout=self.read_timeout())
            try:
                finalized = get(i, "/api/last-finalized-block", timeout=self.read_timeout())
            except urllib.error.HTTPError as error:
                finalized = {"http_error": error.code, "body": error.read().decode(errors="replace")}
            self.record(node=i, status=status, finalized=finalized)
            results.append((status, finalized))
        return results

    def deploy(self, term):
        status = get(0, "/api/v1/status")
        node = node_name(0)
        write = subprocess.run(
            ["docker", "exec", "-i", node, "sh", "-c", "cat > /tmp/acceptance.rho"],
            input=term, text=True, capture_output=True, timeout=10)
        if write.returncode:
            self.record(deploy_term=term, exit_code=write.returncode,
                        stdout=write.stdout, stderr=write.stderr)
            raise RuntimeError("deploy file write failed; see observations.jsonl")
        result = subprocess.run(
            ["docker", "exec", node, "rnode", "--grpc-host", "localhost", "deploy",
             "--phlo-limit", "1000000", "--phlo-price", "1",
             "--private-key", fuzz.DEPLOYER_PRIV, "--shard-id", status["shardId"],
             "--valid-after-block-number", str(status["latestBlockNumber"]),
             "/tmp/acceptance.rho"], text=True, capture_output=True, timeout=30)
        self.record(deploy_term=term, exit_code=result.returncode,
                    stdout=result.stdout, stderr=result.stderr)
        if result.returncode:
            raise RuntimeError("signed deploy submission failed; see observations.jsonl")

    def drive(self):
        if time.monotonic() >= self.next_drive:
            self.deploy("Nil")
            self.next_drive = time.monotonic() + 5

    def wait(self, description, condition, drive=True, window=None):
        effective = self.args.window if window is None else window
        started = time.monotonic()
        deadline = started + effective
        previous = getattr(self, "deadline", None)
        self.deadline = deadline
        last_error = None
        polls = 0
        try:
            while time.monotonic() < deadline:
                polls += 1
                try:
                    value = condition()
                    if value is not None:
                        # The bound is the bound: a condition that finishes after
                        # the deadline does not pass, however good its answer.
                        elapsed = time.monotonic() - started
                        if time.monotonic() > deadline:
                            raise WaitTimeout(
                                f"{description} answered only after the {effective}s window "
                                f"(elapsed={elapsed:.1f}s, polls={polls})")
                        print(f"PASS {description}: {value}", flush=True)
                        return value
                except (OSError, ValueError) as error:
                    last_error = repr(error)
                    self.record(wait=description, read_error=last_error)
                if drive:
                    try:
                        self.drive()
                    except RuntimeError as error:
                        last_error = repr(error)
                        self.record(wait=description, drive_error=last_error)
                time.sleep(2)
            raise WaitTimeout(
                f"{description} did not pass within {effective}s "
                f"(elapsed={time.monotonic()-started:.1f}s, polls={polls}); "
                f"last read error={last_error}")
        finally:
            self.deadline = previous

    def finalized_marker(self, min_height, required=False):
        identities = []
        for i, (_, finalized) in enumerate(self.snapshot()):
            if isinstance(finalized, dict) and "http_error" in finalized:
                body = str(finalized.get("body", ""))
                if "Finalized fringe is not available" in body:
                    if required:
                        raise RuntimeError(
                            f"FINALIZED_FRINGE_UNAVAILABLE: validator {i} reports no finalized fringe")
                    return None
            try:
                height, block_hash = finalized_identity(finalized)
            except ValueError:
                return None
            if height < min_height:
                return None
            answer = probe(i, self.marker, block_hash, timeout=self.read_timeout())
            self.record(node=i, finalized_marker_at=block_hash, response=answer)
            if not marker_present(answer, self.value):
                if not required:
                    return None
                raise RuntimeError(f"validator {i} lost or changed the marker in finalized post-state")
            identities.append({"node": i, "height": height, "hash": block_hash})
        return identities

    def fixed_snapshots(self, identities):
        for identity in identities:
            i, block_hash = identity["node"], identity["hash"]
            answer = probe(i, self.marker, block_hash, timeout=self.read_timeout())
            self.record(node=i, original_finalized_snapshot=block_hash, response=answer)
            if not marker_present(answer, self.value):
                raise RuntimeError(f"validator {i} changed the original finalized post-state snapshot")
            # A state read at a fixed hash proves the block's state is readable,
            # not that it is still finalized. Ask the only finality oracle.
            if getattr(self, "events", None) is not None and self.is_finalized(i, block_hash) is not True:
                raise RuntimeError(
                    f"validator {i} no longer reports its original finalized block as finalized")

    def run(self):
        self.record(tree=subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
                    validators=self.args.validators, epoch_length=10, window=self.args.window,
                    marker=self.marker, expected=self.value)
        self.record(binary_sha256=subprocess.check_output(
            ["docker", "exec", node_name(0), "sha256sum", "/usr/local/bin/rnode"], text=True).strip())
        # A node that is not a clean subject is a named precondition, not a later
        # timeout: a latched pipeline fault or a non-durable divergence zero must
        # not be read as evidence.
        self.health_precondition("workload-start")
        for i in range(self.args.validators):
            inspect = subprocess.check_output(["docker", "inspect", node_name(i)], text=True)
            (self.out / f"container-{i}.json").write_text(inspect)

        # An absent marker must be absent before the write; empty/malformed API
        # responses are not accepted as evidence that the marker is present.
        for i in range(self.args.validators):
            blocks = get(i, "/api/blocks/1")
            if not isinstance(blocks, list) or not blocks:
                raise RuntimeError(f"validator {i} has no anchor block")
            _, block_hash = block_identity(blocks[0])
            answer = probe(i, self.marker, block_hash)
            self.record(node=i, absent_marker_at=block_hash, response=answer)
            if not isinstance(answer, dict) or answer.get("expr") != []:
                raise RuntimeError("the absent-marker control did not return an empty expression list")

        self.deploy(f'@{json.dumps(self.marker)}!({self.value})')

        def visible():
            tips = []
            for i in range(self.args.validators):
                blocks = get(i, "/api/blocks/1")
                if not isinstance(blocks, list) or not blocks:
                    return None
                height, block_hash = block_identity(blocks[0])
                answer = probe(i, self.marker, block_hash)
                self.record(node=i, latest_marker_at=block_hash, response=answer)
                if not marker_present(answer, self.value):
                    return None
                tips.append(height)
            return max(tips)

        self.wait("marker observed on every validator", visible, drive=False)
        # The primary arm submits exactly one deploy. A quiet chain may leave
        # its *tip* unfinalized, so the barrier is the marker in finalized state,
        # not a requirement that finality catch up to the observed tip.
        initial = self.wait("marker present in finalized state on every validator",
                            lambda: self.finalized_marker(1), drive=False)
        first_height = max(x["height"] for x in initial)
        # Cross two *further* boundaries after the finalized-write observation.
        target = (first_height // 10 + 2) * 10 + 4
        final = self.wait("finalized marker retained through two further epoch boundaries",
                          lambda: self.finalized_marker(target, required=True))
        self.fixed_snapshots(initial)
        summary = {"result": "pass", "validators": self.args.validators,
                   "initial_finalized": initial, "final_finalized": final,
                   "target_height": target,
                   "boundary": "finite devnet witness; no universal safety theorem"}
        (self.out / "workload-result.json").write_text(json.dumps(summary, indent=2) + "\n")
        print(json.dumps(summary), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--validators", type=int, choices=[4, 5, 8], required=True)
    parser.add_argument("--window", type=int, default=240)
    parser.add_argument("--out", required=True)
    args = parser.parse_args()
    if os.environ.get("GITHUB_ACTIONS") != "true":
        parser.error("execution requires an isolated GitHub Actions runner")
    if not 30 <= args.window <= 600:
        parser.error("window must be between 30 and 600 seconds")
    drill = Drill(args)
    try:
        drill.run()
    except Exception as error:
        # **The structured envelope, before the legacy string** — and a failed health capture is
        # recorded beside the primary error, never instead of it.
        health, capture_error = failure_envelope.best_effort_health(
            drill.status_row, range(args.validators))
        failure_envelope.write(drill.out, "FAIL", error, phase="workload",
                               health=health, capture_error=capture_error)
        (drill.out / "workload-result.json").write_text(
            json.dumps({"result": "fail", "reason": str(error)}) + "\n")
        raise
    finally:
        try:
            causal_trace.capture(drill, get, node_name)
        except Exception as error:
            # Preserve the primary verdict even if diagnostic capture itself fails.
            drill.record(causal_capture_error=f"{type(error).__name__}: {error}")
            print(f"CAUSAL_CAPTURE_ERROR {type(error).__name__}: {error}", flush=True)


if __name__ == "__main__":
    main()
