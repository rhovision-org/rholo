#!/usr/bin/env python3
"""Validate the release-campaign tracker. Never authorizes a release.

The 12 host/live obligations are **not gates**: a gate is a boolean test of the
current build, while each campaign needs hosts, a release candidate or an
operator window and a digest-pinned evidence record. The tracker keeps them
blocking, carries the executable criterion that judges a campaign's record, and
refuses to let them be passed off as current-build gates.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
AUDIT = ROOT / "spec" / "audit"
EXTERNAL = AUDIT / "mainnet-gates.external.json"
REGISTER = AUDIT / "mainnet-gates.json"
CRITERIA = AUDIT / "gate-criteria.json"
TRACKER = AUDIT / "release-campaigns.json"
TRACKER_MD = AUDIT / "release-campaigns.md"

CAMPAIGN_FAMILIES = {"G428", "G584", "TA", "C343"}
CAMPAIGN_MINIMUM = 12
HEX40 = re.compile(r"^[0-9a-f]{40}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")


class CampaignError(ValueError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise CampaignError(message)


def normalize_source(text: str) -> str:
    """The same emphasis-stripping normalisation the gate register uses."""
    return re.sub(r"\s+", " ", text.replace("*", "").replace("`", "")).strip().lower()


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise CampaignError(f"cannot load {path.name}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _principles():
    return _load("campaign_principles", ROOT / "tools" / "gate-principles.py")


def _builder():
    return _load("campaign_builder", ROOT / "tools" / "build-mainnet-gates.py")


def _selftest(entries: dict, principles) -> list[str]:
    module = _load("campaign_selftest", ROOT / "tools" / "gate-selftest.py")
    return list(module.run(entries, principles=principles))


def _resolve_adapter(reference: object, gate_id: str) -> None:
    require(isinstance(reference, str) and "::" in reference,
            f"{gate_id}: the criterion adapter must be path::symbol")
    path_text, symbol = reference.split("::", 1)
    path = ROOT / path_text
    require(path.is_file(), f"{gate_id}: adapter path {path_text} does not exist")
    text = path.read_text(errors="ignore")
    if symbol.startswith("--"):
        require(symbol in text, f"{gate_id}: adapter flag {symbol} not found")
        return
    require(re.search(rf"\b(?:def|fn|struct)\s+{re.escape(symbol)}\b", text)
            or f"function {symbol}" in text or f"module {symbol}" in text,
            f"{gate_id}: adapter symbol {symbol} not found in {path_text}")


def check_criteria(campaigns: list[dict], principles) -> None:
    """Every campaign carries one executable criterion that hashes its pass condition."""
    require(CRITERIA.is_file(), "spec/audit/gate-criteria.json is absent")
    document = json.loads(CRITERIA.read_text())
    require(document.get("schema") == 1, "gate-criteria schema 1 required")
    entries = document.get("criteria")
    require(isinstance(entries, list) and entries, "gate-criteria has no criteria")
    by_id: dict[str, dict] = {}
    for entry in entries:
        require(isinstance(entry, dict), "every criterion must be an object")
        gate_id = entry.get("gate_id")
        require(isinstance(gate_id, str) and gate_id, "every criterion needs a gate_id")
        require(gate_id not in by_id, f"{gate_id}: duplicate criterion")
        by_id[gate_id] = entry
    expected = {row["id"] for row in campaigns}
    require(set(by_id) == expected,
            "criterion coverage differs: "
            f"extra={sorted(set(by_id) - expected)} missing={sorted(expected - set(by_id))}")
    rows_by_id = {row["id"]: row for row in campaigns}
    for gate_id, entry in by_id.items():
        row = rows_by_id[gate_id]
        expected_hash = hashlib.sha256(str(row["pass_condition"]).encode()).hexdigest()
        require(entry.get("pass_condition_sha256") == expected_hash,
                f"{gate_id}: the criterion does not hash the campaign's pass_condition")
        principle = entry.get("principle")
        require(principle in principles.PRINCIPLES,
                f"{gate_id}: unknown principle {principle!r}")
        for error in principles.validate_parameters(principle, entry.get("parameters")):
            require(False, f"{gate_id}: {error}")
        requires = entry.get("requires")
        require(isinstance(requires, list) and requires
                and all(isinstance(item, str) and item for item in requires),
                f"{gate_id}: requires must be a non-empty list of observation paths")
        controls = entry.get("negative_controls")
        require(isinstance(controls, list) and controls,
                f"{gate_id}: at least one negative control is required")
        expected_fail = 0
        for control in controls:
            require(isinstance(control, dict), f"{gate_id}: every control must be an object")
            require(isinstance(control.get("name"), str) and control["name"],
                    f"{gate_id}: every control needs a name")
            require(isinstance(control.get("mutation"), (list, str)),
                    f"{gate_id}: every control needs a mutation")
            require(control.get("expected") in ("FAIL", "PRECONDITION"),
                    f"{gate_id}: a control must expect FAIL or PRECONDITION")
            expected_fail += control.get("expected") == "FAIL"
        require(expected_fail > 0, f"{gate_id}: no control expects FAIL")
        _resolve_adapter(entry.get("adapter"), gate_id)
    errors = _selftest(by_id, principles)
    require(not errors, "the criteria self-test failed: " + "; ".join(errors[:5]))


def _check_graph(campaigns: list[dict], known_ids: set[str]) -> None:
    by_id = {row["id"]: row for row in campaigns}
    for row in campaigns:
        ident = row["id"]
        for dep in row.get("depends_on") or []:
            require(dep in by_id or dep in known_ids,
                    f"{ident}: unknown dependency {dep}")
        for edge in row.get("related") or []:
            require(isinstance(edge, dict), f"{ident}: related entries must be objects")
            target = edge.get("id")
            criterion = edge.get("criterion")
            require(target in by_id or target in known_ids,
                    f"{ident}: related target {target!r} not found")
            require(isinstance(criterion, str) and len(criterion.strip()) >= 10,
                    f"{ident}: related edge to {target} needs a criterion")
            if target in by_id and row["family"] != by_id[target]["family"]:
                require(re.search(
                    r"\b(not|only|subset|peer|separate|prerequisite|component)\b",
                    criterion, re.I),
                    f"{ident}: a cross-family edge to {target} needs a direction")
            if target in by_id:
                reverse = by_id[target].get("related") or []
                require(any(back.get("id") == ident for back in reverse),
                        f"{ident}: related edge to {target} is not symmetric")
    visiting, done = set(), set()

    def visit(ident: str) -> None:
        if ident in done:
            return
        require(ident not in visiting, f"dependency cycle at {ident}")
        visiting.add(ident)
        for dep in by_id[ident].get("depends_on") or []:
            if dep in by_id:
                visit(dep)
        visiting.remove(ident)
        done.add(ident)

    for ident in by_id:
        visit(ident)


def validate(document: dict) -> dict:
    require(document.get("schema") == 1, "schema 1 required")
    require(document.get("source") == "spec/audit/mainnet-gates.external.json",
            "campaigns must name their external source")
    require(document.get("register") == "spec/audit/mainnet-gates.json",
            "campaigns must name the gate register they are excluded from")
    campaigns = document.get("campaigns")
    require(isinstance(campaigns, list) and bool(campaigns), "campaigns must be a list")
    require(len(campaigns) >= CAMPAIGN_MINIMUM,
            f"only {len(campaigns)} campaigns remain; the floor is {CAMPAIGN_MINIMUM}")
    ids = [row.get("id") for row in campaigns]
    require(all(isinstance(ident, str) for ident in ids), "every campaign needs an id")
    require(len(set(ids)) == len(ids), "duplicate campaign id")
    external_rows = json.loads(EXTERNAL.read_text())["gates"]
    external = {row["id"]: row for row in external_rows if row.get("release_campaign")}
    require(set(ids) == set(external),
            "the tracker and the external source disagree: "
            f"missing={sorted(set(external) - set(ids))} "
            f"unknown={sorted(set(ids) - set(external))}")
    register_ids = {row["id"] for row in json.loads(REGISTER.read_text())["gates"]}
    require(not (set(ids) & register_ids),
            "a campaign cannot also appear in the gate register")
    sources = document.get("sources") or {}
    for key, source in sources.items():
        excerpt = source.get("excerpt")
        if not excerpt:
            continue
        path = ROOT / str(excerpt)
        require(path.is_file(), f"{key}: source excerpt {excerpt} is absent")
        require(source.get("excerpt_sha256")
                == hashlib.sha256(path.read_bytes()).hexdigest(),
                f"{key}: source excerpt hash is stale")
    statuses = []
    for row in campaigns:
        ident = row["id"]
        source = external[ident]
        require(row.get("gate") is False, f"{ident}: a campaign must declare gate: false")
        require(row.get("release_campaign") is True,
                f"{ident}: a campaign must declare release_campaign: true")
        require(row.get("family") in CAMPAIGN_FAMILIES,
                f"{ident}: family {row.get('family')!r} is not a campaign family")
        require(ident.startswith(row["family"] + "-") or ident == row["family"],
                f"{ident}: id must be namespaced by family")
        for field in ("title", "pass_condition", "fail_condition", "observable",
                      "owner", "status_reason", "does_not_claim"):
            require(isinstance(row.get(field), str) and row[field].strip(),
                    f"{ident}: {field} is required")
        for field in ("source_label", "source_url", "locality", "status"):
            require(isinstance(row.get(field), str) and row[field].strip(),
                    f"{ident}: {field} is required")
        require((row.get("source_sha256") or "") == (source.get("source_sha256") or ""),
                f"{ident}: the campaign does not pin its source row")
        family_source = sources.get(row["family"], {})
        if family_source.get("excerpt"):
            excerpt = (ROOT / family_source["excerpt"]).read_text(errors="ignore")
            require(isinstance(row.get("source_phrase"), str)
                    and row["source_phrase"].strip()
                    and normalize_source(row["source_phrase"]) in normalize_source(excerpt),
                    f"{ident}: the source phrase is not in {family_source['excerpt']}")
        criteria = "\n".join(str(row.get(field, "")) for field in
                             ("title", "pass_condition", "fail_condition", "observable"))
        require(row.get("criteria_sha256")
                == hashlib.sha256(criteria.encode()).hexdigest(),
                f"{ident}: the criteria hash does not match the campaign text")
        status = row.get("campaign_status", "open")
        require(status in ("open", "closed"), f"{ident}: campaign_status must be open or closed")
        statuses.append(status)
    _check_graph(campaigns, {row["id"] for row in external_rows} | register_ids)
    check_criteria(campaigns, _principles())
    open_ids = [row["id"] for row, status in zip(campaigns, statuses) if status == "open"]
    return {
        "result": "RELEASE_CAMPAIGNS_OPEN" if open_ids else "RELEASE_CAMPAIGNS_CLOSED",
        "campaigns": len(campaigns),
        "open": len(open_ids),
        "open_ids": open_ids,
        "gates": "not gates; see mainnet-gates.json",
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tracker", nargs="?", type=Path, default=TRACKER)
    parser.add_argument("--assert-ready", action="store_true",
                        help="exit 3 unless every release campaign is closed")
    parser.add_argument("--check", action="store_true",
                        help="compare the committed tracker with a fresh build")
    args = parser.parse_args()
    try:
        if args.check:
            builder = _builder()
            document = builder.campaigns()
            payload = json.dumps(document, indent=2, sort_keys=True) + "\n"
            rendered = builder.render_campaigns_md(document)
            stale = []
            if not TRACKER.exists() or TRACKER.read_text() != payload:
                stale.append(str(TRACKER.relative_to(ROOT)))
            if not TRACKER_MD.exists() or TRACKER_MD.read_text() != rendered:
                stale.append(str(TRACKER_MD.relative_to(ROOT)))
            if stale:
                print(json.dumps({"result": "STALE", "files": stale}, sort_keys=True))
                return 1
        report = validate(json.loads(args.tracker.read_text()))
    except (OSError, ValueError, KeyError, TypeError) as exc:
        print(json.dumps({"result": "INVALID_TRACKER", "reason": str(exc)}, sort_keys=True))
        return 2
    print(json.dumps(report, sort_keys=True))
    if args.assert_ready and report["open"]:
        return 3
    return 0


if __name__ == "__main__":
    sys.exit(main())
