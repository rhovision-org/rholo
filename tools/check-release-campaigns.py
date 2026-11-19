#!/usr/bin/env python3
"""Validate the release-claims tracker.

The 12 old release-campaign rows were deleted and replaced by the seven claims in
`spec/audit/release-claims.json`. A claim is one observable node behaviour, one
falsifier and one control, re-runnable on the standard local devnet or on
`testnet-4`. The 12 old ids are mapped in the source's `superseded` table; none
may disappear silently.

This checker does not load `gate-criteria.json`. A release claim is not a gate;
its own observation and tool are the criterion, and `--assert-ready` exits 3
while any claim or sub-observation is not closed.
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
AUDIT = ROOT / "spec" / "audit"
SOURCE = AUDIT / "release-claims.json"
TRACKER = AUDIT / "release-campaigns.json"
TRACKER_MD = AUDIT / "release-campaigns.md"

# **The deleted rows, in full.** Deleting a row without a mapped fate is the
# failure this set prevents: the tracker must not shrink silently.
OLD_IDS = {
    "C343",
    "G428-M1", "G428-M2", "G428-M3", "G428-M4",
    "G584-G1", "G584-G2", "G584-G3", "G584-G4", "G584-G5",
    "TA-C1", "TA-C2",
}
STATUSES = {"open", "owed", "closed"}
CLAIM_MINIMUM = 7


class ClaimError(RuntimeError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ClaimError(message)


def _builder():
    spec = importlib.util.spec_from_file_location(
        "build_mainnet_gates", ROOT / "tools" / "build-mainnet-gates.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def claim_status(row: dict) -> str:
    statuses = [sub.get("status") for sub in row.get("sub_observations") or []]
    if statuses and all(status == "closed" for status in statuses):
        return "closed"
    if "owed" in statuses:
        return "owed"
    return "open"


def validate(source: dict, builder) -> dict:
    """The authored claims plus the generated tracker, checked together."""
    require(source.get("schema") == 1, "release-claims schema must be 1")
    claims = source.get("claims")
    require(isinstance(claims, list) and claims, "claims must be a non-empty list")
    require(len(claims) >= CLAIM_MINIMUM,
            f"only {len(claims)} release claims; the floor is {CLAIM_MINIMUM}")
    ids = [row.get("id") for row in claims]
    require(len(ids) == len(set(ids)), "claim ids must be unique")
    require(not (set(ids) & OLD_IDS),
            "a claim may not reuse a deleted campaign id")

    superseded = source.get("superseded")
    require(isinstance(superseded, list) and superseded,
            "the superseded table must map the deleted rows")
    old = {}
    for entry in superseded:
        ident = entry.get("id")
        require(ident in OLD_IDS, f"superseded row {ident!r} is not one of the deleted ids")
        require(ident not in old, f"superseded row {ident!r} appears twice")
        require(entry.get("fate") and entry.get("home"),
                f"superseded row {ident!r} needs a fate and a home")
        old[ident] = entry
    require(set(old) == OLD_IDS,
            "every deleted row must be mapped: "
            + ", ".join(sorted(OLD_IDS - set(old))))

    for row in claims:
        ident = row.get("id")
        require(row.get("title") and row.get("claim"),
                f"{ident}: a claim needs a title and a one-sentence claim")
        require(row.get("pass_condition") and row.get("fail_condition"),
                f"{ident}: pass and fail conditions are required")
        require(row.get("observable") and row.get("negative_control"),
                f"{ident}: an observable and a control are required")
        env = row.get("environment") or {}
        require(env.get("local") and env.get("public"),
                f"{ident}: both environments are required")
        rerun = row.get("rerun") or {}
        require(rerun.get("local") and rerun.get("public"),
                f"{ident}: both re-run commands are required")
        require(row.get("tool") or row.get("missing_observation"),
                f"{ident}: name a tool or a missing observation")
        supersedes = row.get("supersedes") or []
        require(set(supersedes) <= OLD_IDS,
                f"{ident}: supersedes an unknown row")
        subs = row.get("sub_observations")
        require(isinstance(subs, list) and subs,
                f"{ident}: at least one sub-observation is required")
        for sub in subs:
            sid = sub.get("id")
            require(sid and sub.get("name"),
                    f"{ident}: every sub-observation needs an id and a name")
            require(sub.get("status") in STATUSES,
                    f"{ident}/{sid}: status must be open, owed or closed")
            if sub["status"] == "open":
                require(sub.get("tool"),
                        f"{ident}/{sid}: an open observation needs a tool")
            if sub["status"] == "owed":
                require(sub.get("missing"),
                        f"{ident}/{sid}: an owed observation needs a missing description")

    fresh = builder.campaigns()
    require([row["id"] for row in fresh["campaigns"]] == ids,
            "the generated tracker does not carry the authored claims")
    return {
        "claims": len(claims),
        "statuses": {row["id"]: claim_status(row) for row in claims},
        "old": len(old),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true",
                        help="compare the committed tracker with a fresh build")
    parser.add_argument("--assert-ready", action="store_true",
                        help="exit 3 while any claim or sub-observation is not closed")
    parser.add_argument("--json", action="store_true", help="print the result as JSON")
    args = parser.parse_args()

    builder = _builder()
    try:
        source = json.loads(SOURCE.read_text())
        result = validate(source, builder)
    except (ClaimError, ValueError) as error:
        print(json.dumps({"result": "INVALID", "error": str(error)}, sort_keys=True))
        return 2

    if args.check:
        fresh = builder.campaigns()
        payload = json.dumps(fresh, indent=2, sort_keys=True) + "\n"
        markdown = builder.render_campaigns_md(fresh)
        stale = [
            str(path.relative_to(ROOT))
            for path, expected in ((TRACKER, payload), (TRACKER_MD, markdown))
            if not path.exists() or path.read_text() != expected
        ]
        if stale:
            print(json.dumps({"result": "STALE", "files": stale}, sort_keys=True))
            return 2

    open_claims = {ident: status for ident, status in result["statuses"].items()
                   if status != "closed"}
    body = {
        "result": "RELEASE_CLAIMS_OPEN" if open_claims else "RELEASE_CLAIMS_CLOSED",
        "claims": result["claims"],
        "open": open_claims,
        "superseded": result["old"],
    }
    if args.json:
        print(json.dumps(body, indent=2, sort_keys=True))
    else:
        print(body["result"])
        print(f"release claims: {result['claims']}; superseded: {result['old']}")
        for ident, status in result["statuses"].items():
            print(f"  {ident}: {status}")
    if args.assert_ready and open_claims:
        return 3
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
