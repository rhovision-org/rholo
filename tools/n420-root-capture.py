#!/usr/bin/env python3
"""Capture M5's 840/841 source bundle from a node that still retains the blocks.

Why this is an instrument and not a transcript. The #420 bundle is **perishable**: it lives only on a
node that still holds blocks 835..845, and it is the input M5's replay (M5-02) needs. A summary would
repeat the incident's own mistake — the claim must be re-derivable from bytes, so this writes the
node's *replies* and the verifier's verdict on each.

What it does, per endpoint and block:
  GET /api/v1/block/{hash}/raw  -> raw/{endpoint}-{hash}.pb, and `rnode verify-block` over the bytes
  GET /api/block/{hash}         -> light/{endpoint}-{hash}.json (pre/post state, justifications,
                                   rejectedDeploys — read from the verified bytes, not from this summary)
  GET /api/last-finalized-block -> light/{endpoint}-finalized.json

**It never substitutes a fixture.** If every endpoint refuses or lacks the blocks, the capture fails
and the record says INCOMPLETE; that is the honest outcome, not a reason to fabricate an input.

Usage:
  python3 tools/n420-root-capture.py --out spec/audit/evidence/n420-root-reads \\
      --endpoint http://127.0.0.1:40403 --endpoint http://127.0.0.1:41403 \\
      --block <841-full-hex> --block <parent-1-full-hex> ... --rnode ./target/release/rnode

The 840 parents' full hashes are in the #420 thread; the short prefixes in the issue are
`e6feba1b`, `3600d92d`, `b358a796`, `b88855b9`, and 841 is `2ac6ec40`. Supply **full** hashes: a
prefix cannot be requested.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
import urllib.error
import urllib.request


def get(endpoint: str, path: str, timeout: float = 30.0):
    """(bytes, error) — the error is recorded and never raised: a failed probe is evidence."""
    try:
        with urllib.request.urlopen(f"{endpoint}{path}", timeout=timeout) as response:
            return response.read(), None
    except (urllib.error.URLError, OSError, ValueError) as error:
        return None, repr(error)


def verify(rnode: str, data: bytes):
    """Run `rnode verify-block` over the bytes; (ok, output). A missing binary is not a pass."""
    try:
        result = subprocess.run(
            [rnode, "verify-block"],
            input=data,
            capture_output=True,
            timeout=60,
        )
    except (OSError, subprocess.SubprocessError) as error:
        return False, repr(error)
    output = (result.stdout + result.stderr).decode(errors="replace").strip()
    return result.returncode == 0, output


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True)
    parser.add_argument("--endpoint", action="append", required=True)
    parser.add_argument("--block", action="append", required=True)
    parser.add_argument("--rnode", default="rnode")
    parser.add_argument("--timeout", type=float, default=30.0)
    args = parser.parse_args()

    out = os.path.abspath(args.out)
    raw_dir = os.path.join(out, "raw")
    light_dir = os.path.join(out, "light")
    os.makedirs(raw_dir, exist_ok=True)
    os.makedirs(light_dir, exist_ok=True)

    capture = {"endpoints": args.endpoint, "blocks": args.block, "entries": [], "complete": True}
    for endpoint in args.endpoint:
        tag = endpoint.replace("://", "_").replace("/", "_").replace(":", "_")
        for block in args.block:
            entry = {"endpoint": endpoint, "block": block}
            data, error = get(endpoint, f"/api/v1/block/{block}/raw", args.timeout)
            if data is None:
                entry["raw_error"] = error
                capture["complete"] = False
            else:
                path = os.path.join(raw_dir, f"{tag}-{block}.pb")
                with open(path, "wb") as handle:
                    handle.write(data)
                entry["raw_bytes_sha256"] = hashlib.sha256(data).hexdigest()
                ok, output = verify(args.rnode, data)
                entry["verify_ok"] = ok
                entry["verify_output"] = output
                if not ok:
                    capture["complete"] = False
            light, light_error = get(endpoint, f"/api/block/{block}", args.timeout)
            if light is None:
                entry["light_error"] = light_error
                capture["complete"] = False
            else:
                with open(os.path.join(light_dir, f"{tag}-{block}.json"), "wb") as handle:
                    handle.write(light)
            capture["entries"].append(entry)
        finalized, error = get(endpoint, "/api/last-finalized-block", args.timeout)
        if finalized is None:
            capture["finalized_error"] = {endpoint: error}
            capture["complete"] = False
        else:
            with open(os.path.join(light_dir, f"{tag}-finalized.json"), "wb") as handle:
                handle.write(finalized)

    with open(os.path.join(out, "capture.json"), "w") as handle:
        json.dump(capture, handle, indent=2, sort_keys=True)
        handle.write("\n")
    print(
        f"n420-root-capture: {len(capture['entries'])} block probe(s), "
        f"complete={capture['complete']} -> {out}"
    )
    return 0 if capture["complete"] else 1


if __name__ == "__main__":
    sys.exit(main())
