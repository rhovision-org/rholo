#!/usr/bin/env python3
"""Validate the release-claims tracker and derive its statuses.

The 12 old release-campaign rows were deleted and replaced by the seven claims in
`spec/audit/release-claims.json`. A claim is one observable node behaviour with one
executable test under `tests/`. The tracker stores no status and no `missing`
observation: a claim's status in an environment is the verdict its own test's
`check(record)` returns over that environment's newest committed run record.

There is no `open`/`owed`/`closed` vocabulary. A claim whose test does not exist,
does not import, or does not define `CLAIM_ID`, `run`, `check` and `fixture` is
refused; that is the machine check behind "every test needs to exist".
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
CLAIM_MINIMUM = 7
FORBIDDEN = ("status", "missing", "sub_observations", "tool")
REQUIRED = ("id", "title", "claim", "pass_condition", "fail_condition", "observable",
            "negative_control", "environment", "rerun", "test", "owner",
            "trace", "close", "next_pr")
TRACE_KINDS = ("merge", "finality", "state", "boot", "admission", "audit", "format")
CLOSE_KEYS = ("not-observed", "precondition", "unsafe-point", "model-incomplete",
              "evidence-insufficient")


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


def load_test(row: dict):
    """Import the claim's test module, refusing anything that cannot be run."""
    ident = row.get("id")
    relative = row.get("test")
    require(isinstance(relative, str) and relative.endswith(".py"),
            f"{ident}: test must name a .py file")
    path = ROOT / relative
    require(path.is_file(), f"{ident}: test {relative} does not exist")
    spec = importlib.util.spec_from_file_location(
        "claim_test_" + str(ident).lower().replace("-", "_"), path)
    try:
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
    except Exception as error:  # noqa: BLE001 - any import failure is a refusal
        raise ClaimError(f"{ident}: test {relative} does not import: "
                         f"{type(error).__name__}: {error}") from error
    require(getattr(module, "CLAIM_ID", None) == ident,
            f"{ident}: {relative} declares CLAIM_ID "
            f"{getattr(module, 'CLAIM_ID', None)!r}")
    for name in ("run", "check", "fixture"):
        require(callable(getattr(module, name, None)),
                f"{ident}: {relative} does not define a callable {name}()")
    return module


def validate(source: dict, builder) -> dict:
    """The authored claims plus the generated tracker, checked together."""
    require(source.get("schema") == 2, "release-claims schema must be 2")
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

    require(isinstance(source.get("out_of_scope"), list),
            "out_of_scope must be a list (it is not a status)")
    action_prs = source.get("action_prs")
    require(isinstance(action_prs, list) and action_prs,
            "action_prs must name the next PRs")
    pr_ids = set()
    for entry in action_prs:
        ident = entry.get("id")
        require(ident and entry.get("scope") and entry.get("claims"),
                "every action PR needs an id, a scope and the claims it closes")
        require(ident not in pr_ids, f"action PR {ident!r} appears twice")
        pr_ids.add(ident)
    for row in claims:
        ident = row.get("id")
        for field in REQUIRED:
            require(row.get(field), f"{ident}: {field} is required")
        for forbidden in FORBIDDEN:
            require(forbidden not in row,
                    f"{ident}: {forbidden!r} is retired; status is measured, not authored")
        env = row.get("environment") or {}
        require(env.get("local") and env.get("public"),
                f"{ident}: both environments are required")
        rerun = row.get("rerun") or {}
        require(rerun.get("local") and rerun.get("public"),
                f"{ident}: both re-run commands are required")
        require("tools/release-check.py run --claim " + str(ident) in rerun["local"],
                f"{ident}: rerun.local must run tools/release-check.py for this claim")
        require("tools/release-check.py run --claim " + str(ident) in rerun["public"],
                f"{ident}: rerun.public must run tools/release-check.py for this claim")
        require(row.get("trace") in TRACE_KINDS,
                f"{ident}: trace must be one of {TRACE_KINDS}")
        close = row.get("close") or {}
        for key in CLOSE_KEYS:
            require(isinstance(close.get(key), str) and close[key].strip(),
                    f"{ident}: close.{key} must say what to do")
        next_pr = row.get("next_pr") or {}
        for environment in ("local", "public"):
            require(next_pr.get(environment) in pr_ids,
                    f"{ident}: next_pr.{environment} must name an action PR")
        supersedes = row.get("supersedes") or []
        require(set(supersedes) <= OLD_IDS,
                f"{ident}: supersedes an unknown row")
        module = load_test(row)
        fixture = module.fixture()
        require(isinstance(fixture, dict), f"{ident}: fixture() must return a record")
        require(fixture.get("claim_id") == ident,
                f"{ident}: fixture() is for {fixture.get('claim_id')!r}")
        verdict, reasons = module.check(fixture)
        require(verdict == "pass",
                f"{ident}: fixture() does not pass its own check: {verdict} {reasons}")
        require(fixture.get("verdict") == "pass",
                f"{ident}: fixture() must be a passing record")

    fresh = builder.campaigns()
    require([row["id"] for row in fresh["campaigns"]] == ids,
            "the generated tracker does not carry the authored claims")
    return {
        "claims": len(claims),
        "actions": {row["id"]: row.get("action") or {} for row in fresh["campaigns"]},
        "statuses": {row["id"]: row.get("status") or {} for row in fresh["campaigns"]},
        "old": len(old),
    }


def ready(result: dict) -> bool:
    for actions in result["actions"].values():
        for environment in ("local", "public"):
            if (actions.get(environment) or {}).get("reading") != "safe":
                return False
    return True


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true",
                        help="compare the committed tracker with a fresh build")
    parser.add_argument("--assert-ready", action="store_true",
                        help="exit 3 unless every claim is pass in both environments")
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

    is_ready = ready(result)
    body = {
        "result": "RELEASE_CLAIMS_READY" if is_ready else "RELEASE_CLAIMS_NOT_READY",
        "claims": result["claims"],
        "superseded": result["old"],
        "readings": {ident: {env: (entry.get("reading") or "not-observed")
                             for env, entry in actions.items()}
                     for ident, actions in result["actions"].items()},
        "open_actions": sum(len(((actions.get(env) or {}).get("items") or []))
                            for actions in result["actions"].values()
                            for env in ("local", "public")),
    }
    if args.json:
        print(json.dumps(body, indent=2, sort_keys=True))
    else:
        print(body["result"])
        print(f"release claims: {result['claims']}; superseded: {result['old']}; "
              f"open actions: {body['open_actions']}")
        for ident, readings in body["readings"].items():
            print(f"  {ident}: local={readings.get('local')} "
                  f"public={readings.get('public')}")
    if args.assert_ready and not is_ready:
        return 3
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
