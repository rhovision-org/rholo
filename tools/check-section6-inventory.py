#!/usr/bin/env python3
"""Check exact authored §6 coverage. This does not certify category or safety."""
from __future__ import annotations
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE_PATH = "spec/audit/passes.md"
_SPEC = importlib.util.spec_from_file_location("hardfork_51_gate", ROOT / "tools/check-hardfork-51-closeout.py")
_GATE = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(_GATE)
CLASSIFICATIONS = {
    "IMPLEMENTED / VERIFIED", "IMPLEMENTED / EVIDENCE INCOMPLETE", "CODE DEFECT",
    "INTEGRATION GAP", "RELEASE BLOCKER", "REQUIRES MAINTAINER DECISION",
    "EXPLICITLY DEFERRED",
}

def source_rows(document):
    marker = "## 6. Scala-deviation register"
    if document.count(marker) != 1:
        raise ValueError("exactly one section 6 required")
    section = document.split(marker, 1)[1].split("\n## 7.", 1)[0]
    rows = [line for line in section.splitlines()
            if line.startswith("|") and not line.startswith("|---")
            and not line.startswith("| Deviation")]
    if not rows:
        raise ValueError("empty section 6 inventory")
    return rows

def validate(document, inventory, matrix, ledger, *, repo=None):
    root = (repo or ROOT).resolve()
    _GATE.validate(ledger, repo=root, acceptance_root=root)
    expected = source_rows(document)
    ledger_rows = {row["id"]: row for row in ledger["items"]}
    source_lines = {raw: number for number, raw in enumerate(document.splitlines(), 1)}
    rows = inventory.get("rows")
    if not isinstance(rows, list) or len(rows) != len(expected):
        raise ValueError("section 6 row coverage changed")
    if inventory.get("source_dev_sha") != ledger["source_dev_sha"]:
        raise ValueError("inventory and tracker have different source commits")
    if matrix.get("source_dev_sha") != ledger["source_dev_sha"]:
        raise ValueError("matrix and tracker have different source commits")
    if inventory.get("independent_signoff") is not False:
        raise ValueError("this authored inventory cannot supply independent sign-off")
    seen = set()
    for raw, row in zip(expected, rows):
        digest = hashlib.sha256(raw.encode()).hexdigest()
        if row.get("row_sha256") != digest or row.get("id") != "S6:"+digest[:16]:
            raise ValueError("source row digest or order changed")
        if row["id"] in seen:
            raise ValueError("duplicate source row")
        seen.add(row["id"])
        if row.get("existing_evidence", {}).get("source_row") != raw:
            raise ValueError("authored source evidence changed")
        if type(row.get("source_line")) is not int or row["source_line"] != source_lines[raw]:
            raise ValueError("source row line changed")
        expected_url = ("https://github.com/rhovision-org/rholo/blob/"
                        + ledger["source_dev_sha"] + "/spec/audit/passes.md")
        if row.get("existing_evidence", {}).get("url") != expected_url:
            raise ValueError("source evidence URL is not pinned to the ledger commit")
        if row.get("fork_class") not in {"A_CANDIDATE", "NON_A_RECORDED"}:
            raise ValueError("every row needs an explicit category disposition")
        if row.get("classification") != "IMPLEMENTED / EVIDENCE INCOMPLETE":
            raise ValueError("source coverage must not be promoted to verified safety")
        for field in ("invariant", "missing_proof", "smallest_action", "acceptance_test", "decision_owner"):
            if not isinstance(row.get(field), str) or not row[field].strip():
                raise ValueError("missing row obligation: "+field)
    entries = matrix.get("items", [])
    ids = [x.get("id") for x in entries]
    if len(set(ids)) != len(ids) or set(ids) != {x["id"] for x in ledger["items"]}:
        raise ValueError("tracker matrix coverage mismatch")
    for row in entries:
        if row.get("classification") not in CLASSIFICATIONS:
            raise ValueError("unclassified tracker item")
        authored = ledger_rows[row["id"]]
        deferred = authored["status"] == "deferred"
        if type(row.get("recorded_owner_reason_deferral")) is not bool or row["recorded_owner_reason_deferral"] != deferred:
            raise ValueError("matrix/ledger deferral mismatch")
        if deferred:
            if (row["classification"] != "EXPLICITLY DEFERRED"
                    or row.get("decision_owner") != authored["decision_owner"]
                    or row.get("deferral_reason") != authored["reason"]):
                raise ValueError("matrix deferral must carry the ledger's owner and reason")
        elif row["classification"] == "EXPLICITLY DEFERRED":
            raise ValueError("matrix cannot fabricate a ledger deferral")
        if row["classification"] == "IMPLEMENTED / VERIFIED":
            raise ValueError("this source-only matrix cannot certify execution")
        for field in ("exact_file_function_or_invariant", "missing_proof",
                      "smallest_corrective_action", "required_acceptance_test", "decision_owner"):
            if not isinstance(row.get(field), str) or not row[field].strip():
                raise ValueError("missing tracker obligation: "+field)
    return {"result":"EXACT_SOURCE_COVERAGE_ONLY_INDEPENDENT_REVIEW_REQUIRED",
            "section6_rows":len(rows), "category_a_candidates":sum(x["fork_class"]=="A_CANDIDATE" for x in rows),
            "tracker_obligations":len(entries), "independent_signoff":False, "release_authorized":False}


def validate_pinned(repo, inventory, matrix, ledger):
    """Validate the inventory against passes.md **as committed** at source_dev_sha.

    The local register is not the evidence: a §6 row can be edited in the worktree
    while the pinned blob stays unchanged. A shallow or absent pin is reported, not
    turned into a malformed inventory; the caller decides whether that is fatal.
    """
    status, text, reason = _GATE.git_blob(Path(repo), ledger["source_dev_sha"], SOURCE_PATH)
    if status != "verified":
        return {"result": "SOURCE_BLOB_UNVERIFIED", "blob_status": status,
                "source": SOURCE_PATH, "source_dev_sha": ledger["source_dev_sha"],
                "reason": reason, "independent_signoff": False,
                "release_authorized": False}
    result = validate(text, inventory, matrix, ledger, repo=repo)
    result.update({"blob_status": "verified", "source": SOURCE_PATH,
                   "source_dev_sha": ledger["source_dev_sha"]})
    return result


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=ROOT)
    mode=parser.add_mutually_exclusive_group()
    mode.add_argument("--register", type=Path, default=None)
    mode.add_argument("--verify-pinned-blob", action="store_true")
    args=parser.parse_args()
    try:
        audit=args.repo/"spec/audit"
        inventory=json.loads((audit/"section6-gap-inventory.json").read_text())
        matrix=json.loads((audit/"final-gap-matrix.json").read_text())
        ledger=json.loads((audit/"hardfork-51-closeout.json").read_text())
        if args.verify_pinned_blob:
            result=validate_pinned(args.repo, inventory, matrix, ledger)
            print(json.dumps(result,sort_keys=True))
            return 0 if result.get("blob_status")=="verified" else 4
        document=(args.register or (args.repo/SOURCE_PATH)).read_text()
        result=validate(document, inventory, matrix, ledger, repo=args.repo)
        result["blob_status"]="local"
    except (OSError,ValueError,KeyError,TypeError) as exc:
        print(json.dumps({"result":"INVALID_INVENTORY","reason":str(exc)}))
        return 2
    print(json.dumps(result,sort_keys=True))
    return 0

if __name__=="__main__":
    raise SystemExit(main())
