#!/usr/bin/env python3
"""Bounded no-helper, one-validator-paused finality witness on an isolated devnet.

No traffic to the public testnet. Never manufacture progress with Nil/helper deploys.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import time
import urllib.error
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("devnet_fuzz", ROOT / "tools/devnet-fuzz.py")
fuzz = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fuzz)

def call(port, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(
        "http://127.0.0.1:%d%s" % (port, path), data=data,
        headers={"Content-Type": "application/json"},
        method="POST" if data is not None else "GET",
    )
    with urllib.request.urlopen(req, timeout=8) as response:
        return json.load(response)

def node(i):
    return "acceptance-bootstrap" if i == 0 else "acceptance-validator-%d" % i


def canonical_bonds(block, validators):
    rows = block.get("bonds")
    if not isinstance(rows, list) or len(rows) != validators:
        raise RuntimeError("BOND_MAP_MISMATCH: incomplete bonded topology")
    result = {}
    for row in rows:
        validator, stake = row.get("validator"), row.get("stake")
        if not isinstance(validator, str) or not validator or type(stake) is not int or stake <= 0 or validator in result:
            raise RuntimeError("BOND_MAP_MISMATCH: invalid or duplicate bond")
        result[validator] = stake
    return result


def require_ready(validators, events, seconds=90):
    """Require connected, same-genesis nodes before taking away bonded stake.

    The devnet is a bootstrap-star, not a complete mesh: a joiner normally has
    only one peer. Do not demand N-1 peers on each joiner.
    """
    deadline = time.monotonic() + seconds
    stable = 0
    while time.monotonic() < deadline:
        try:
            states = [call(40403 + 1000*i, "/api/status") for i in range(validators)]
            blocks = [call(40403 + 1000*i, "/api/blocks/1") for i in range(validators)]
            roots = [xs[0]["blockHash"] for xs in blocks]
            ready = (all(isinstance(xs,list) and len(xs)>0 for xs in blocks)
                     and len(set(roots)) == 1
                     and states[0].get("peers", 0) >= validators - 1
                     and all(x.get("peers", 0) >= 1 and
                             x.get("latestBlockNumber", 0) >= 1 for x in states[1:]))
            record(events, phase="readiness", ready=ready, peers=[x.get("peers") for x in states],
                   genesis_hashes=roots, heights=[x.get("latestBlockNumber") for x in states])
            stable = stable + 1 if ready else 0
            if stable >= 3:
                bonds = [canonical_bonds(xs[0], validators) for xs in blocks]
                if any(b != bonds[0] for b in bonds):
                    raise RuntimeError("BOND_MAP_MISMATCH: validators disagree")
                return bonds[0]
        except (OSError, ValueError, KeyError, IndexError, TypeError) as exc:
            stable = 0
            record(events, phase="readiness_error", error=repr(exc))
        time.sleep(2)
    raise RuntimeError("SETUP_NOT_READY: peer/bootstrap connectivity and genesis agreement were not stable")


def sign_term(term, status):
    """Use the real signed CLI deploy path, never the admin propose endpoint."""
    write = subprocess.run(
        ["docker", "exec", "-i", node(0), "sh", "-c", "cat > /tmp/quiet435.rho"],
        input=term, capture_output=True, text=True, timeout=10)
    if write.returncode:
        raise RuntimeError("term file write failed: " + write.stderr[:1000])
    cmd = ["docker", "exec", node(0), "rnode", "--grpc-host", "localhost",
           "deploy", "--phlo-limit", "1000000", "--phlo-price", "1",
           "--private-key", fuzz.DEPLOYER_PRIV, "--shard-id", status["shardId"],
           "--valid-after-block-number", str(status["latestBlockNumber"]),
           "/tmp/quiet435.rho"]
    return subprocess.run(cmd, capture_output=True, text=True, timeout=30)


def preseed_finality(validators, events):
    """Optional explicit CONTROL: establish a healthy finalized baseline with work.

    No bootstrap work is included in the post-pause quiet-chain verdict.
    A failure to establish the baseline is a setup failure, not a quiet PASS.
    """
    deadline = time.monotonic() + 180
    count = 0
    next_seed = 0
    stable = 0
    while time.monotonic() < deadline:
        if time.monotonic() >= next_seed:
            submission = sign_term("Nil", call(40403, "/api/status"))
            count += 1
            record(events, phase="preseed", index=count, returncode=submission.returncode,
                   stdout=submission.stdout[:400], stderr=submission.stderr[:400])
            if submission.returncode:
                raise RuntimeError("SETUP_SEED_DEPLOY_FAILED")
            next_seed = time.monotonic() + 6
        try:
            finalized = [call(40403+1000*i, "/api/last-finalized-block")["blockInfo"]
                         for i in range(validators)]
            ids = {(x["blockNumber"], x["blockHash"]) for x in finalized}
            stable = stable+1 if len(ids)==1 and min(x["blockNumber"] for x in finalized)>=2 else 0
            if stable >= 2:
                record(events, phase="preseed_finalized", identity=list(ids)[0], deploys=count)
                return count
        except (urllib.error.HTTPError, KeyError, OSError, ValueError) as exc:
            record(events, phase="preseed_wait", error=str(exc))
            stable = 0
        time.sleep(3)
    raise RuntimeError("SETUP_NO_FINALIZED_BASELINE: preseed control could not finalize within 180s")

def record(path, **data):
    with path.open("a") as out:
        out.write(json.dumps({"utc": time.time(), **data}, sort_keys=True) + "\n")

def run(args):
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    events = out / "observations.jsonl"
    marker = "quiet-435-" + uuid.uuid4().hex
    paused = args.validators - 1
    survivors = list(range(paused))
    source = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    base = { "source": source, "validators": args.validators,
             "paused_validator": paused, "live_validators": survivors,
             "quiet_window_seconds": args.window,
             "marker": marker, "marker_value": 435, "signed_deploy_count": 0,
             "helper_deploy_count": 0, "initialization": args.initialization, "preseed_deploy_count": 0}
    record(events, phase="setup", **base)
    versions = []
    for i in range(args.validators):
        status = call(40403 + 1000*i, "/api/status")
        versions.append(status.get("version", {}).get("node"))
        if status.get("latestBlockNumber", 0) < 1:
            raise AssertionError("validator %d lacks genesis" % i)
        if status.get("shardId") != versions_shard(args):
            raise AssertionError("unexpected shard ID on validator %d: %s" % (i, status.get("shardId")))
        record(events, phase="initial_status", node=i, status=status)
    if len(set(versions)) != 1:
        raise AssertionError("mixed node binary versions")
    if any(not isinstance(v, str) or source not in v for v in versions):
        raise AssertionError("node version does not match exact source SHA")
    base["version"] = versions[0]
    base["original_bonds"] = require_ready(args.validators, events)
    if args.initialization == "seeded":
        base["preseed_deploy_count"] = preseed_finality(args.validators, events)
    # Only one marker deploy is permitted after pausing the validator.
    subprocess.run(["docker", "pause", node(paused)], check=True, timeout=12)
    check = subprocess.check_output(
        ["docker", "inspect", "-f", "{{.State.Paused}}", node(paused)], text=True, timeout=10).strip()
    if check != "true":
        raise AssertionError("the selected validator did not pause")
    record(events, phase="paused", node=paused, paused=check)
    status = call(40403, "/api/status")
    term = '@' + json.dumps(marker) + '!(435)'
    submit = sign_term(term, status)
    base["signed_deploy_count"] = 1
    record(events, phase="signed_marker_deploy", returncode=submit.returncode,
           stdout=submit.stdout[:2000], stderr=submit.stderr[:2000])
    if submit.returncode:
        raise RuntimeError("marker deploy submission failed")
    deadline = time.monotonic() + args.window
    last = {}
    cycles = 0
    while time.monotonic() < deadline:
        cycles += 1
        ready = 0
        last = {}
        operators = {}
        for i in survivors:
            try:
                block = call(40403+1000*i, "/api/last-finalized-block")["blockInfo"]
                latest = call(40403+1000*i, "/api/blocks/1")[0]
                if (canonical_bonds(block, args.validators) != base["original_bonds"]
                        or canonical_bonds(latest, args.validators) != base["original_bonds"]):
                    raise RuntimeError("BOND_MAP_MISMATCH: validator %d changed bonded stake" % i)
                block_hash = block["blockHash"]
                term_read = 'new return in { for (@v <- @' + json.dumps(marker) + ') { return!(v) } }'
                response = call(40403+1000*i, "/api/v1/explore-deploy-by-block-hash",
                                {"term": term_read, "blockHash": block_hash, "usePreStateHash": False})
                match = response.get("expr") == [{"ExprInt": {"data": 435}}]
                observed = {"finalized_height": block["blockNumber"],
                            "finalized_hash": block_hash, "marker_present": match,
                            "reply_source": response.get("replySource")}
                last[str(i)] = observed
                if match: ready += 1
                record(events, phase="poll", node=i, **observed)
            except urllib.error.HTTPError as error:
                body = error.read(3000).decode("utf-8", "replace")
                last[str(i)] = {"http_status": error.code, "response_body": body}
                record(events, phase="poll_http_error", node=i, status=error.code, body=body)
            except (OSError, ValueError, KeyError) as error:
                last[str(i)] = {"error": repr(error)}
                record(events, phase="poll_error", node=i, error=repr(error))
        for i in survivors:
            try:
                status = call(40405+1000*i, "/api/v1/status/operator")
                operators[str(i)] = status
                record(events, phase="operator_status", node=i,
                       latest=status.get("latestBlockNumber"),
                       finality_stall=status.get("finalityStall"),
                       unrestorable=status.get("unrestorableRecords"), peers=status.get("peers"))
                if type(status.get("unrestorableRecords")) is int and status["unrestorableRecords"] > 0:
                    raise RuntimeError("PERMANENT_DIVERGENCE: validator %d has unrestorable records" % i)
            except urllib.error.HTTPError as error:
                body = error.read(3000).decode("utf-8", "replace")
                record(events, phase="operator_http_error", node=i, status=error.code, body=body)
            except (OSError, ValueError) as error:
                record(events, phase="operator_error", node=i, error=repr(error))
        if complete_finalized_observation(last, operators, survivors):
            base.update({"result":"PASS", "observations":last})
            (out / "result.json").write_text(json.dumps(base, indent=2, sort_keys=True)+"\n")
            print("QUIET_CHAIN_PASS " + json.dumps(base, sort_keys=True), flush=True)
            return
        time.sleep(4)
    base.update({"result":"FAIL", "reason":"no finalized marker at all surviving validators within bounded window",
                 "observations":last})
    (out / "result.json").write_text(json.dumps(base, indent=2, sort_keys=True)+"\n")
    print("QUIET_CHAIN_FAIL "+json.dumps(base, sort_keys=True), flush=True)
    raise RuntimeError(base["reason"])

def complete_finalized_observation(observations, operators, survivors):
    """Only one complete, current poll with common finality can pass."""
    expected = {str(i) for i in survivors}
    if not expected or set(observations) != expected or set(operators) != expected:
        return False
    identities = set()
    for i in expected:
        value, operator = observations[i], operators[i]
        block_hash = value.get("finalized_hash")
        height = value.get("finalized_height")
        if (value.get("marker_present") is not True
                or not isinstance(block_hash, str) or len(block_hash) != 64
                or any(c not in "0123456789abcdef" for c in block_hash)
                or type(height) is not int or height < 0
                or type(operator.get("unrestorableRecords")) is not int
                or operator["unrestorableRecords"] != 0):
            return False
        identities.add((height, block_hash))
    return len(identities) == 1


def capture_causal_evidence(args):
    """Capture admitted block preimages after measurement; never alter its verdict."""
    from types import SimpleNamespace
    trace_spec = importlib.util.spec_from_file_location("quiet_causal_trace", ROOT / "tools/consensus-causal-trace.py")
    trace = importlib.util.module_from_spec(trace_spec)
    trace_spec.loader.exec_module(trace)
    def get(i, path, body=None, admin=False):
        return call((40405 if admin else 40403) + 1000*i, path, body)
    os.environ["ACCEPTANCE_ARTIFACTS"] = "1"
    trace.capture(SimpleNamespace(out=Path(args.out), args=args), get, node)


def versions_shard(args):
    return os.environ.get("DEVNET_SHARD_ID", "/root")

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--validators", required=True, type=int, choices=[4,8])
    ap.add_argument("--window", type=int, default=240)
    ap.add_argument("--initialization", choices=["fresh", "seeded"], default="fresh")
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    if os.environ.get("GITHUB_ACTIONS") != "true":
        ap.error("disposable GitHub Actions runner required")
    if not 30 <= args.window <= 600:
        ap.error("invalid window")
    out = Path(args.out)
    try:
        run(args)
    except Exception as error:
        # A topology/seed failure is not evidence that quiet finality failed.
        # Preserve a machine-readable refusal even if the test stopped before
        # the marker reached its measured window.
        out.mkdir(parents=True, exist_ok=True)
        result = out / "result.json"
        if not result.exists():
            kind = ("SETUP_FAIL" if str(error).startswith("SETUP_")
                    else "HARNESS_OR_SUBMISSION_FAIL")
            result.write_text(json.dumps({
                "result": kind,
                "reason": str(error),
                "validators": args.validators,
                "initialization": args.initialization,
                "window": args.window,
            }, indent=2, sort_keys=True) + "\n")
        raise
    finally:
        try:
            capture_causal_evidence(args)
        except Exception as error:
            record(out / "observations.jsonl", phase="causal_capture_error", error=repr(error))
            print("CAUSAL_CAPTURE_ERROR " + repr(error), flush=True)

if __name__ == "__main__":
    main()
