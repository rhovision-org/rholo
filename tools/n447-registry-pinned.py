#!/usr/bin/env python3
"""Compare signed insertion with native lookup at identical block roots (#447).

Disposable one-validator devnet only. Do not use on any public network.
"""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

PREFIX = os.environ.get("DEVNET_PREFIX", "n447registry")
NODE = PREFIX + "-bootstrap"
URI_PATTERN = re.compile(r"rho:id:[a-z0-9]+")
BT = chr(96)


def docker(*args, check=True):
    return subprocess.run(["docker", *args], check=check, text=True, capture_output=True)


def req(path, body=None):
    cmd = ["curl", "-fsS", "--connect-timeout", "3", "--max-time", "8"]
    if body is not None:
        cmd += ["-X", "POST", "-H", "content-type: application/json", "-d", json.dumps(body)]
    return json.loads(docker("exec", NODE, *cmd, "http://127.0.0.1:40403" + path).stdout)


def blocks():
    result = req("/api/blocks/25")
    return result.get("blocks", []) if isinstance(result, dict) else result


def tip():
    entries = blocks()
    if not entries:
        raise RuntimeError("block listing is empty")
    return max(entries, key=lambda x: int(x["blockNumber"]))


def explore(term, block_hash, pre=False):
    return req("/api/v1/explore-deploy-by-block-hash", {
        "term": term, "blockHash": block_hash, "usePreStateHash": pre})


def expr_values(reply):
    if not isinstance(reply, dict) or not isinstance(reply.get("expr"), list):
        raise AssertionError(f"invalid exploratory response: {reply!r}")
    return json.dumps(reply["expr"], sort_keys=True)


def get_test_key():
    # A public throwaway devnet key; never emit private-key command arguments.
    source = (Path(__file__).resolve().parents[1] / "tools/devnet.sh").read_text()
    match = re.search(r'^VALIDATOR_PRIV=\(\s*\n\s*"([0-9a-f]{64})"', source, re.M)
    if not match:
        raise RuntimeError("devnet test key table is unavailable")
    return match.group(1)


def signed_deploy(term, height):
    encoded = base64.b64encode(term.encode()).decode()
    path = "/tmp/n447-registry-insert.rho"
    docker("exec", NODE, "sh", "-c", f"echo {encoded} | base64 -d > {path}")
    result = docker("exec", NODE, "rnode", "deploy",
                    "--phlo-limit", "1000000", "--phlo-price", "1",
                    "--private-key", get_test_key(), "--shard-id", "/root",
                    "--valid-after-block-number", str(height), path)
    return {"receipt_sha256": hashlib.sha256(result.stdout.encode()).hexdigest(),
            "term_sha256": hashlib.sha256(term.encode()).hexdigest()}


def find_signed_carrier(history, height, term, block_loader):
    """Use deploy inclusion, not highest block number or execution status.

    With propose-on-deploy enabled, rnode deploy can already have produced the
    carrying block. A later manual propose creates an EMPTY higher block; the
    latest-tip API is then the wrong anchor for the signed write.
    """
    for block in sorted(history, key=lambda item: int(item["blockNumber"])):
        if int(block["blockNumber"]) <= height or int(block.get("deployCount", 0)) < 1:
            continue
        response = block_loader(block["blockHash"])
        matches = [d for d in response.get("deploys", []) if d.get("term") == term]
        if len(matches) == 1:
            return block, matches[0].get("sig")
        if len(matches) > 1:
            raise AssertionError("duplicate matching signed terms in carrying block")
    return None, None


def propose_until_carrier(height, term, timeout):
    deadline = time.time() + timeout
    while time.time() < deadline:
        carrying, deploy_sig = find_signed_carrier(
            blocks(), height, term, lambda h: req("/api/block/" + h))
        if carrying is not None:
            return carrying, deploy_sig
        docker("exec", NODE, "rnode", "propose", check=False)
        time.sleep(2)
    raise RuntimeError("no signed deploy carrying block before timeout")


def lookup_term(uri):
    if not URI_PATTERN.fullmatch(uri):
        raise AssertionError("malformed registry URI")
    return (
        "new rl(" + BT + "rho:registry:lookup" + BT
        + "), ack in { rl!(" + BT + uri + BT
        + ', *ack) | for (@value <- ack) { @"out"!(value) } }'
    )


def classify(expr):
    if '"ExprNil"' in expr:
        return "Nil"
    if re.search(r'"Expr(?:Int|Number|GInt)"\s*:\s*\{\s*"data"\s*:\s*(?:"42"|42)', expr):
        return "42"
    return "other"


def classify_reply(reply):
    """Classify both the value and whether the output channel captured a reply."""
    if not isinstance(reply, dict):
        raise AssertionError("exploratory response is not an object")
    expr = reply.get("expr")
    source = reply.get("replySource")
    if expr == [] and source == "out":
        # Nil is an empty RhoExpr list; 'out' records a captured reply.
        return "Nil"
    if expr == [] and source == "none":
        return "no_reply"
    return classify(expr_values(reply))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True)
    parser.add_argument("--timeout", type=int, default=120)
    args = parser.parse_args(argv)
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    report = {"result": "INCOMPLETE", "boundary": "disposable native registry-at-block"}
    try:
        base = tip()
        report["base"] = base
        term = (
            "new put(" + BT + "rho:registry:insertArbitrary" + BT
            + '), ack in { put!(42, *ack) | for (@uri <- ack) { @"n447_reg_uri"!(uri) } }'
        )
        report["signed_insert"] = signed_deploy(term, base["blockNumber"])
        carrying, included_signature = propose_until_carrier(
            base["blockNumber"], term, args.timeout)
        report["carrying"] = carrying
        report["included_deploy_signature"] = included_signature
        if int(carrying.get("deployCount", 0)) < 1 or not included_signature:
            raise AssertionError("the carrying block does not prove inclusion")
        if carrying.get("postStateHash") == carrying.get("preStateHash"):
            raise AssertionError("the registration block left state unchanged")
        uri_reply = explore('for (@uri <- @"n447_reg_uri") { @"out"!(uri) }',
                            carrying["blockHash"])
        report["uri_reply"] = uri_reply
        found = set(URI_PATTERN.findall(json.dumps(uri_reply)))
        if len(found) != 1:
            raise AssertionError(f"expected one registered URI: {uri_reply!r}")
        uri = next(iter(found))
        report["uri"] = uri
        query = lookup_term(uri)
        report["lookup_term_sha256"] = hashlib.sha256(query.encode()).hexdigest()
        report["reads"] = {
            "ancestor": explore(query, base["blockHash"]),
            "pre": explore(query, carrying["blockHash"], pre=True),
            "post": explore(query, carrying["blockHash"]),
        }
        report["classification"] = {
            name: classify_reply(reply) for name, reply in report["reads"].items()}
        if report["classification"] != {"ancestor": "Nil", "pre": "Nil", "post": "42"}:
            raise AssertionError(f"registry state mismatch: {report['classification']}")
        report["result"] = "PASS"
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        out.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
        print("registry verdict:", report["result"], "evidence:", out, flush=True)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"FAIL: {type(error).__name__}: {error}", file=sys.stderr)
        sys.exit(1)
