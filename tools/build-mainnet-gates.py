#!/usr/bin/env python3
"""Build the canonical mainnet-gate register from the authored ledgers.

The external gate families (M1-M6, G1-G6, the testnet criteria, C343/C357,
#520) are hand-authored in `spec/audit/mainnet-gates.external.json`; the #51
and section-6 families are derived from their existing ledgers so those rows
are not copied by hand. Every derived row carries the source row's hash, so a
ledger edit that is not reflected here is a drift failure.

This builder is not an oracle: it records what the ledgers say, and it never
promotes a gate to `met`. The checker is `tools/check-mainnet-gates.py`.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
AUDIT = ROOT / "spec" / "audit"
OUT_JSON = AUDIT / "mainnet-gates.json"
OUT_MD = AUDIT / "mainnet-gates.md"
EXTERNAL = AUDIT / "mainnet-gates.external.json"
MATRIX = AUDIT / "final-gap-matrix.json"
LEDGER = AUDIT / "hardfork-51-closeout.json"
INVENTORY = AUDIT / "section6-gap-inventory.json"

REGISTER_DEV_SHA = "dd6a25d69128375d83b7a7701c169ce672e57536"
TEST_PATH = re.compile(
    r"((?:[A-Za-z0-9_./-]+\.(?:py|rs|sh|lean)::[A-Za-z0-9_:]+)"
    r"|(?:[A-Za-z0-9_./-]+/tests?/[A-Za-z0-9_./-]+\.(?:py|rs|sh|lean))"
    r"|(?:tests?/[A-Za-z0-9_./-]+\.(?:py|rs|sh|lean)))")


def _is_test(path: Path, symbol: str) -> bool:
    text = path.read_text(errors="ignore")
    if path.suffix == ".rs":
        lines = text.splitlines()
        for index, line in enumerate(lines):
            if re.search(rf"\bfn\s+{re.escape(symbol)}\b", line):
                window = "\n".join(lines[max(0, index - 3):index + 1])
                return "#[test]" in window or "#[tokio::test]" in window
        return False
    if path.suffix == ".py":
        return symbol.startswith("test") and bool(
            re.search(rf"\bdef\s+{re.escape(symbol)}\b", text))
    if path.suffix == ".lean":
        return bool(re.search(rf"\btheorem\s+{re.escape(symbol)}\b", text))
    return "test" in path.name or "tests" in path.parts


def named_test_status(row: dict) -> str | None:
    statuses = []
    for match in TEST_PATH.finditer(str(row.get("acceptance_test") or "")):
        path_text = match.group(1)
        path_name, _, symbol = path_text.partition("::")
        path = ROOT / path_name
        if not path.is_file():
            if "/" in path_name:
                statuses.append("missing")
            continue
        if _is_test(path, symbol):
            statuses.append("named")
    if "missing" in statuses:
        return "missing"
    if "named" in statuses:
        return "named"
    return None
OVERRIDES = {
    "B:breaking-release": (
        "blocked",
        "The release runbook is the release-blocking work; its archive, validator and "
        "new-genesis attestations are absent.",
    ),
    "Batch:crypto-agility": (
        "blocked",
        "The registry/wire freeze and implementation are not landed; this is work, "
        "not only a decision.",
    ),
}
CROSSWALK = {
    "A:block-merge": [("G428-M5", "A:block-merge is an open semantic decision, not an M5 witness")],
    "Audit:sidecar-merge": [("G428-M5", "sidecar-merge is cost composition, not per-chain accounting")],
    "Audit:law52-partition": [("G428-M4", "partition/liveness component")],
    "Audit:section6-categoryA": [("G428-M6", "the section-6 category-A disposition is a separate audit sign-off")],
    "A:shard-config": [("G428-M4", "committed bounds are a finality/recovery prerequisite")],
    "Release:attachment-upgrade": [("G584-G1", "uniform validator/client rollout is a release prerequisite")],
    "Batch:activation-schedule": [("G584-G1", "the activation schedule is a release prerequisite")],
    "A:vault-export": [("G428-M5", "state-export conservation is a state-integrity input, not merge accounting")],
    "Batch:state-preservation": [("G428-M6", "the preservation export is a reproducibility component")],
    "Audit:inactivity-leak": [("G428-M4", "the inactivity leak is a liveness component, explicitly deferred")],
}
S6_CROSSWALK = {
    "S6:f735095c746556f3": [("G428-M5", "one merge-diff component")],
    "S6:5651614bf8e3eeb7": [("C357", "C357 replay-order component")],
    "S6:0963a698bc9c4ee4": [("G428-M4", "liveness-enforcement component")],
}


def canonical_hash(value: object) -> str:
    raw = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(raw).hexdigest()


def infer_locality(text: str, status: str) -> str:
    if status in {"decision", "external"}:
        return "external"
    low = text.lower()
    if any(word in low for word in ("devnet", "partition", "three-node", "multi-node", "distributed")):
        return "local-devnet"
    if any(word in low for word in ("live", "operator", "uniform", "independent", "release")):
        return "external"
    if any(word in low for word in ("historical replay", "second-binary", "two binaries", "replay")):
        return "integration"
    if any(word in low for word in ("fixture", "corpus", "unit", "regression", "test")):
        return "unit"
    return "unclassified"


def infer_status(ledger_item: dict, matrix_item: dict) -> tuple[str, str]:
    if ledger_item["status"] == "deferred":
        return "decision", "owner-and-reason deferral recorded"
    classification = matrix_item["classification"]
    missing = str(matrix_item.get("missing_proof") or "").strip()
    if matrix_item["id"] == "Audit:section6-categoryA":
        return "owes", "47 section-6 A candidates lack a per-candidate disposition."
    if matrix_item["id"] in OVERRIDES:
        return OVERRIDES[matrix_item["id"]]
    if classification == "REQUIRES MAINTAINER DECISION":
        return "decision", missing
    if classification == "EXPLICITLY DEFERRED":
        return "decision", "recorded owner-and-reason deferral"
    if classification == "RELEASE BLOCKER":
        return "blocked", missing
    if classification == "INTEGRATION GAP":
        return "owes", missing
    if ledger_item["status"] in {"open", "merged"}:
        return "owes", missing
    return "reviewable", missing


def hf51_rows(matrix: dict, ledger: dict) -> list[dict]:
    ledger_rows = {row["id"]: row for row in ledger["items"]}
    source_sha = ledger["source_dev_sha"]
    rows = []
    for item in matrix["items"]:
        ident = item["id"]
        authored = ledger_rows[ident]
        status, reason = infer_status(authored, item)
        text = " ".join(str(item.get(key) or "") for key in (
            "exact_file_function_or_invariant", "missing_proof",
            "required_acceptance_test", "smallest_corrective_action"))
        locality = infer_locality(text, status)
        related = [
            {"id": target, "criterion": criterion}
            for target, criterion in CROSSWALK.get(ident, [])
        ]
        rows.append({
            "id": "HF51-" + ident.replace(":", "-"),
            "family": "HF51",
            "title": authored.get("title") or ident,
            "source_label": f"final-gap-matrix.json#{ident}",
            "source_url": f"https://github.com/rhovision-org/rholo/blob/{source_sha}/spec/audit/final-gap-matrix.json",
            "source_retrieved": matrix.get("observed_date"),
            "source_sha256": canonical_hash(item),
            "pass_condition": item.get("required_acceptance_test"),
            "fail_condition": "The required acceptance is absent: " + str(item.get("missing_proof") or ""),
            "observable": item.get("exact_file_function_or_invariant"),
            "witness_kind": None,
            "witness_ref": None,
            "negative_control": None,
            "docker_free": False,
            "sole_evidence": "none",
            "locality": locality,
            "witness_scope": None,
            "owner": item.get("decision_owner"),
            "status": status,
            "status_reason": reason,
            "evidence": {"url": None, "digest": None, "source_sha": None, "binary_sha256": None},
            "depends_on": [],
            "related": related,
            "laws": [],
            "source_ref": f"final-gap-matrix.json#items/{ident}",
            "source_hash": canonical_hash(item),
            "does_not_claim": "A merged implementation is not the acceptance witness; the matrix acceptance is prose.",
        })
    return rows


def s6_rows(inventory: dict) -> list[dict]:
    dispositions = {}
    path = AUDIT / "section6-dispositions.json"
    if path.exists():
        dispositions = {row["id"]: row for row in json.loads(path.read_text()).get("rows", [])}
    rows = []
    for row in inventory["rows"]:
        if row.get("fork_class") != "A_CANDIDATE":
            continue
        ident = row["id"]
        evidence = row.get("existing_evidence") or {}
        source_url = evidence.get("url")
        disposition = dispositions.get(ident) or {}
        test_status = named_test_status(row)
        if disposition.get("verdict") == "withdrawn":
            status = "superseded"
            reason = "Recorded withdrawn: " + str(disposition.get("reason") or "")
        elif test_status == "missing":
            missing = next(
                (match.group(1) for match in TEST_PATH.finditer(
                    str(row.get("acceptance_test") or ""))
                 if "/" in match.group(1)
                 and not (ROOT / match.group(1).split("::", 1)[0]).is_file()),
                "the named test")
            status = "owes"
            reason = f"The named test {missing} is absent."
        elif test_status == "named":
            status = "owes"
            reason = "A named test exists, but no recorded negative control."
        else:
            status = "reviewable"
            reason = ("No per-candidate disposition; the acceptance text is templated, "
                      "not a witness.")
        related = [
            {"id": target, "criterion": criterion}
            for target, criterion in S6_CROSSWALK.get(ident, [])
        ]
        rows.append({
            "id": "S6-" + ident.split(":", 1)[1],
            "family": "S6",
            "title": str(row.get("invariant") or ident)[:160],
            "source_label": f"section6-gap-inventory.json#{ident}",
            "source_url": source_url,
            "source_retrieved": inventory.get("observed_date"),
            "source_sha256": row.get("row_sha256"),
            "pass_condition": row.get("acceptance_test"),
            "fail_condition": "The required acceptance is absent: " + str(row.get("missing_proof") or ""),
            "observable": row.get("invariant"),
            "witness_kind": None,
            "witness_ref": None,
            "negative_control": None,
            "docker_free": False,
            "sole_evidence": "none",
            "locality": "unclassified",
            "witness_scope": None,
            "owner": row.get("decision_owner"),
            "status": status,
            "status_reason": reason,
            "evidence": {"url": None, "digest": None, "source_sha": None, "binary_sha256": None},
            "depends_on": [],
            "related": related,
            "laws": [],
            "source_ref": f"section6-gap-inventory.json#rows/{ident}",
            "source_hash": row.get("row_sha256"),
            "does_not_claim": "Source-row coverage is not an independent safety check; the row is an A candidate, not a pass.",
        })
    return rows


def symmetrize(rows: list[dict]) -> None:
    by_id = {row["id"]: row for row in rows}
    for row in rows:
        for edge in list(row.get("related") or []):
            target = by_id.get(edge["id"])
            if target is None:
                raise ValueError(f"{row['id']} relates to unknown {edge['id']}")
            if not any(back["id"] == row["id"] for back in target.get("related") or []):
                target.setdefault("related", []).append(
                    {"id": row["id"], "criterion": edge["criterion"]}
                )


def build() -> dict:
    matrix = json.loads(MATRIX.read_text())
    ledger = json.loads(LEDGER.read_text())
    inventory = json.loads(INVENTORY.read_text())
    external = json.loads(EXTERNAL.read_text())
    for source in external["sources"].values():
        excerpt = source.get("excerpt")
        if excerpt:
            path = ROOT / excerpt
            if not path.is_file():
                raise ValueError(f"source excerpt {excerpt} is absent")
            source["excerpt_sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
    for row in external["gates"]:
        criteria = "\n".join(str(row.get(field, "")) for field in
                             ("title", "pass_condition", "fail_condition", "observable"))
        row["criteria_sha256"] = hashlib.sha256(criteria.encode()).hexdigest()
    rows = list(external["gates"])
    rows.extend(hf51_rows(matrix, ledger))
    rows.extend(s6_rows(inventory))
    seen = set()
    for row in rows:
        if row["id"] in seen:
            raise ValueError(f"duplicate gate id {row['id']}")
        seen.add(row["id"])
    symmetrize(rows)
    rows.sort(key=lambda row: (row["family"], row["id"]))
    return {
        "schema": 1,
        "register_dev_sha": REGISTER_DEV_SHA,
        "ledger_source_dev_sha": ledger["source_dev_sha"],
        "generated": "2026-11-18",
        "sources": external["sources"],
        "gates": rows,
    }


def render_md(document: dict) -> str:
    lines = [
        "# Mainnet gate register",
        "",
        "Emitted from `spec/audit/mainnet-gates.json` by `tools/build-mainnet-gates.py`.",
        "Do not edit by hand; the checker compares this file with the JSON.",
        "",
        f"- register dev: `{document['register_dev_sha']}`",
        f"- ledger source dev: `{document['ledger_source_dev_sha']}`",
        f"- generated: {document['generated']}",
        f"- gates: {len(document['gates'])}",
        "",
        "| gate | family | status | title | locality | owner |",
        "|---|---|---|---|---|---|",
    ]
    def cell(value: object) -> str:
        text = "" if value is None else str(value)
        return text.replace("|", "\\|").replace("\n", " ")
    for row in document["gates"]:
        lines.append(
            f"| `{cell(row['id'])}` | {cell(row['family'])} | **{cell(row['status'])}** | "
            f"{cell(row['title'])[:120]} | {cell(row['locality'])} | {cell(row['owner'])[:60]} |"
        )
    lines.extend([
        "",
        "Status is a disposition of the evidence, not a release claim. A gate is `met` only",
        "when its witness resolves, its negative control is recorded, its evidence is pinned,",
        "and every dependency is satisfied. This register ships no `met` gate.",
        "",
    ])
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true",
                        help="compare the committed register with a fresh build")
    parser.add_argument("--stdout", action="store_true", help="print the JSON")
    args = parser.parse_args()
    document = build()
    payload = json.dumps(document, indent=2, sort_keys=True) + "\n"
    md = render_md(document)
    if args.stdout:
        print(payload, end="")
        return 0
    if args.check:
        stale = []
        if not OUT_JSON.exists() or OUT_JSON.read_text() != payload:
            stale.append(str(OUT_JSON.relative_to(ROOT)))
        if not OUT_MD.exists() or OUT_MD.read_text() != md:
            stale.append(str(OUT_MD.relative_to(ROOT)))
        if stale:
            print(json.dumps({"result": "STALE", "files": stale}, sort_keys=True))
            return 1
        print(json.dumps({"result": "CURRENT", "gates": len(document["gates"])}, sort_keys=True))
        return 0
    OUT_JSON.write_text(payload)
    OUT_MD.write_text(md)
    print(json.dumps({"result": "WROTE", "gates": len(document["gates"])}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
