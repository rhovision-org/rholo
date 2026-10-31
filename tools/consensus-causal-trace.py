#!/usr/bin/env python3
"""Capture and compare accepted DAG views; diagnostics, not consensus proof."""
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import time
import zlib


def hex_value(value, size, label):
    if not isinstance(value, str) or len(value) != size * 2:
        raise ValueError(f"invalid {label}")
    bytes.fromhex(value)
    return value.lower()


def index_blocks(blocks):
    if not isinstance(blocks, list):
        raise ValueError("block inventory is not a list")
    result = {}
    for block in blocks:
        key = hex_value(block.get("blockHash"), 32, "block hash")
        hex_value(block.get("sender"), 65, "validator identity")
        if type(block.get("blockNumber")) is not int or block["blockNumber"] < 0:
            raise ValueError("invalid block height")
        if type(block.get("seqNum")) is not int or block["seqNum"] < 0:
            raise ValueError("invalid sender sequence")
        hex_value(block.get("preStateHash"), 32, "pre-state hash")
        hex_value(block.get("postStateHash"), 32, "post-state hash")
        parents = block.get("justifications")
        if not isinstance(parents, list):
            raise ValueError("missing justifications")
        for parent in parents:
            hex_value(parent, 32, "justification")
        if len(set(parents)) != len(parents):
            raise ValueError("duplicate justification")
        if key in result and result[key] != block:
            raise ValueError("contradictory observations for one block hash")
        result[key] = block
    return result


def unique_bonds(rows):
    if not isinstance(rows, list) or not rows:
        raise ValueError("missing bonded set")
    bonds = {}
    for row in rows:
        sender = hex_value(row.get("validator"), 65, "bonded identity")
        weight = row.get("stake")
        if type(weight) is not int or weight < 0:
            raise ValueError("invalid stake")
        if sender in bonds:
            raise ValueError("duplicate bonded identity")
        bonds[sender] = weight
    return bonds


def summarize(blocks):
    index = index_blocks(blocks)
    if not index:
        raise ValueError("empty DAG inventory")
    latest = {}
    for block in index.values():
        sender = block["sender"]
        if sender not in latest or block["seqNum"] > latest[sender]["seqNum"]:
            latest[sender] = block
    tips = sorted((b for b in index.values()
                   if b["blockNumber"] == max(x["blockNumber"] for x in index.values())),
                  key=lambda b: b["blockHash"])
    missing = sorted({p for b in index.values() for p in b["justifications"] if p not in index})
    # Keep each tip's bonded context separate. A union across epochs would be
    # misleading, and these weights are never treated as signed quorum evidence.
    contexts = []
    for tip in tips:
        bonds = unique_bonds(tip["bonds"])
        contexts.append({"block_hash": tip["blockHash"], "bonds": bonds,
                         "total_stake": sum(bonds.values())})
    return {"accepted_ids": sorted(index), "missing_dependencies": missing,
            "latest_by_sender": {s: {"hash": b["blockHash"], "height": b["blockNumber"],
                                      "seq": b["seqNum"]} for s, b in sorted(latest.items())},
            "tip_bond_contexts": contexts,
            "complete_inventory": not missing,
            "boundary": "accepted DAG observations; not proof of finality or stake support"}


def compare(views):
    sets = {str(i): set(v["accepted_ids"]) for i, v in views.items()}
    union = set().union(*sets.values()) if sets else set()
    return {"nodes": len(sets), "union_ids": sorted(union),
            "missing_by_node": {i: sorted(union - ids) for i, ids in sets.items()},
            "same_accepted_ids": bool(sets) and all(ids == union for ids in sets.values()),
            "boundary": "same accepted IDs does not establish finality or independently signed quorum"}


def decisions(log):
    result = []
    for line in log.splitlines():
        match = re.search(r"attestation at tip (\d+): (licensed|withheld).*?round=\[([^]]*)\]", line)
        if match:
            fields = dict(re.findall(r"(\w+)=(true|false|\d+)", line))
            result.append({"height": int(match[1]), "decision": match[2],
                           "round_sender_prefixes": match[3].split(), "inputs": fields,
                           "source_line": line})
    return result


def verify_raw(block, raw):
    if raw.get("blockHash") != block["blockHash"] or raw.get("sender") != block["sender"]:
        raise ValueError("raw block identity disagrees with inventory")
    encoded = raw.get("encoded")
    if not isinstance(encoded, str):
        raise ValueError("raw block has no preimage")
    digest = hashlib.blake2b(bytes.fromhex(encoded), digest_size=32).hexdigest()
    if digest != block["blockHash"]:
        raise ValueError("raw block preimage hash mismatch")


def encode_capture(data):
    return hashlib.sha256(data).hexdigest(), base64.b64encode(zlib.compress(data, 9)).decode()


def decode_capture(digest, text):
    data = zlib.decompress(base64.b64decode(text, validate=True))
    if hashlib.sha256(data).hexdigest() != digest:
        raise ValueError("capture transport checksum mismatch")
    return data


def emit_capture(path):
    digest, text = encode_capture(path.read_bytes())
    print(f"=== BEGIN ZLIB {path} sha256={digest} ===", flush=True)
    for offset in range(0, len(text), 4096):
        print(text[offset:offset + 4096], flush=True)
    print(f"=== END ZLIB {path} ===", flush=True)


def read_bounded_blocks(get, node, height):
    """Read every accepted height without exceeding the API's default range limit."""
    blocks = []
    for start in range(0, height + 1, 50):
        end = min(start + 49, height)
        page = get(node, f"/api/blocks/{start}/{end}")
        if not isinstance(page, list):
            raise ValueError(f"block range {start}..{end} is not a list")
        blocks.extend(page)
    index_blocks(blocks)  # Reject inconsistent overlapping observations.
    return blocks


def capture(run, get, node_name):
    out = run.out / "causal"
    out.mkdir(exist_ok=True)
    views, errors = {}, {}
    manifest = {"utc": time.time(), "source_sha": subprocess.check_output(
        ["git", "rev-parse", "HEAD"], text=True).strip(),
        "validators": run.args.validators, "nodes": {},
        "unavailable": ["internal round ID", "signature verification of captured raw bytes",
                        "candidate fringe internals; use gate verdict in operator status"]}
    for i in range(run.args.validators):
        node = node_name(i)
        try:
            capture_start = time.time()
            # Container evidence first: an unreachable or partitioned node must
            # still yield its log, its state and its binary identity.
            log = subprocess.run(["docker", "logs", node], text=True,
                                 capture_output=True, timeout=15)
            if log.returncode:
                raise RuntimeError("container log capture failed")
            text = log.stdout + log.stderr
            (out / f"node-{i}.log").write_text(text)
            binary = subprocess.check_output(
                ["docker", "exec", node, "sha256sum", "/usr/local/bin/rnode"],
                text=True, timeout=10).strip().split()[0]
            state = subprocess.check_output(["docker", "inspect", "--format",
                                             "{{json .State}}", node], text=True, timeout=10)
            manifest["nodes"][str(i)] = {"binary_sha256": binary,
                                         "container_state": json.loads(state)}
            status = get(i, "/api/v1/status/operator", admin=True)
            height = status["latestBlockNumber"]
            if type(height) is not int or not 0 <= height <= 256:
                raise ValueError(
                    f"CAPTURE_HEIGHT_REFUSED: latestBlockNumber={height} is outside the "
                    "bounded 0..256 range; the drill verdict is unaffected")
            # Height-range reads include accepted unfinalized blocks. Do not
            # substitute a wallet/name read that now selects finalized state.
            blocks = read_bounded_blocks(get, i, height)
            index = index_blocks(blocks)
            raw_blocks = {}
            for key, block in sorted(index.items()):
                raw = get(i, f"/api/v1/block/{key}/raw")
                verify_raw(block, raw)
                raw_blocks[key] = raw
            view = summarize(blocks)
            views[i] = view
            payload = {"node": i, "operator_status": status, "blocks": blocks,
                       "raw_blocks": raw_blocks, "summary": view,
                       "capture_started_utc": capture_start, "capture_finished_utc": time.time(),
                       "operator_status_after": get(i, "/api/v1/status/operator", admin=True),
                       "attestation_decisions": decisions(text)}
            (out / f"node-{i}.json").write_text(json.dumps(payload, indent=2) + "\n")
        except Exception as error:
            errors[str(i)] = f"{type(error).__name__}: {error}"
    comparison = compare(views)
    comparison.update({"errors": errors,
                       "complete_capture": not errors and len(views) == run.args.validators})
    (out / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    (out / "comparison.json").write_text(json.dumps(comparison, indent=2) + "\n")
    report_capture(out, comparison)
    # Capture is diagnostic. An incomplete capture must not replace the drill verdict.
    # comparison.json retains the refusal and complete_capture=false for reviewers.


def report_capture(out, comparison):
    """Preserve complete captures as artifacts without exhausting the step log."""
    if os.environ.get("ACCEPTANCE_ARTIFACTS") == "1":
        summary = {"complete_capture": comparison["complete_capture"],
                   "errors": comparison["errors"],
                   "accepted_ids": len(comparison.get("union_ids", [])),
                   "full_evidence": str(out)}
        print("CAUSAL_CAPTURE " + json.dumps(summary), flush=True)
        return
    # For standalone runners without artifact uploads, retain checksum-
    # verified compressed frames. Never silently discard evidence.
    print("CAUSAL_CAPTURE " + json.dumps(comparison), flush=True)
    for path in sorted([*out.glob("*.json"), *out.glob("*.log")]):
        emit_capture(path)
