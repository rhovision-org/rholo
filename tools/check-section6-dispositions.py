#!/usr/bin/env python3
"""Check the per-candidate section-6 dispositions.

The inventory's one `Audit:section6-categoryA` row is not a substitute for 47
candidate-level dispositions. This checker requires one record per A candidate,
bound to the source row hash. A `reviewable` row stays open; `accepted` needs an
independent reviewer, an https verdict and a digest; `withdrawn` needs a reason
and a decision URL. This does not supply an independent sign-off.
"""
from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
INVENTORY = ROOT / "spec/audit/section6-gap-inventory.json"
DISPOSITIONS = ROOT / "spec/audit/section6-dispositions.json"
HEX64 = re.compile(r"^[0-9a-f]{64}$")
VERDICTS = {"accepted", "withdrawn", "reviewable"}
EFFECTS = {"state-hash", "replay-verdict", "wire-format", "none"}


class DispositionError(ValueError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise DispositionError(message)


def url(value: object) -> bool:
    return isinstance(value, str) and value.startswith("https://") and " " not in value


def validate(inventory: dict, document: dict) -> dict:
    require(document.get("schema") == 1, "schema 1 required")
    require(document.get("independent_signoff") is False,
            "this record cannot self-award independent sign-off")
    require(document.get("source_dev_sha") == inventory.get("source_dev_sha"),
            "the dispositions must pin the inventory's source commit")
    candidates = [row for row in inventory["rows"] if row.get("fork_class") == "A_CANDIDATE"]
    expected = {row["id"]: row for row in candidates}
    rows = document.get("rows")
    require(isinstance(rows, list), "rows must be a list")
    ids = [row.get("id") for row in rows]
    require(len(ids) == len(set(ids)), "duplicate disposition id")
    require(set(ids) == set(expected),
            f"disposition coverage mismatch: missing={sorted(set(expected) - set(ids))}; "
            f"unknown={sorted(set(ids) - set(expected))}")
    accepted = withdrawn = reviewable = 0
    for row in rows:
        ident = row["id"]
        source = expected[ident]
        require(row.get("row_sha256") == source.get("row_sha256"),
                f"{ident}: source row hash is stale")
        verdict = row.get("verdict")
        require(verdict in VERDICTS, f"{ident}: unknown verdict {verdict!r}")
        require(isinstance(row.get("derivation"), str) and row["derivation"].strip(),
                f"{ident}: a derivation is required")
        require(isinstance(row.get("reason"), str) and row["reason"].strip(),
                f"{ident}: a reason is required")
        effect = row.get("category_a_effect")
        require(effect in EFFECTS, f"{ident}: unknown category_a_effect {effect!r}")
        require(isinstance(row.get("category_a_derivation"), str)
                and row["category_a_derivation"].strip(),
                f"{ident}: a category-A derivation is required")
        if verdict == "accepted":
            require(effect != "none",
                    f"{ident}: an accepted row needs a derived category-A effect")
            require(isinstance(row.get("reviewer"), str) and row["reviewer"].strip(),
                    f"{ident}: an accepted row needs a named reviewer")
            require(row.get("reviewer_independent") is True,
                    f"{ident}: an accepted row needs an independent reviewer")
            require(url(row.get("review_url")), f"{ident}: an accepted row needs an https verdict URL")
            require(isinstance(row.get("evidence_sha256"), str)
                    and HEX64.fullmatch(row["evidence_sha256"]),
                    f"{ident}: an accepted row needs an evidence SHA-256")
            review_record = row.get("review_record")
            require(isinstance(review_record, str) and review_record.strip(),
                    f"{ident}: an accepted row needs a review record")
            review_path = Path(review_record)
            review_path = review_path if review_path.is_absolute() else ROOT / review_path
            if review_path.is_dir():
                review_path = review_path / "README.md"
            require(review_path.is_file(), f"{ident}: the review record does not exist")
            text = review_path.read_text(errors="ignore")
            require(str(row["reviewer"]) in text,
                    f"{ident}: the review record does not name the reviewer")
            require(str(row["evidence_sha256"]) in text,
                    f"{ident}: the review record does not carry the evidence digest")
            accepted += 1
        elif verdict == "withdrawn":
            require(url(row.get("review_url")),
                    f"{ident}: a withdrawn row needs a decision URL")
            withdrawn += 1
        else:
            reviewable += 1
    return {
        "result": "DISPOSITIONS_COMPLETE" if reviewable == 0 else "DISPOSITIONS_OPEN",
        "candidates": len(expected),
        "accepted": accepted,
        "withdrawn": withdrawn,
        "reviewable": reviewable,
        "reviewable_ids": [row["id"] for row in rows if row["verdict"] == "reviewable"],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dispositions", nargs="?", type=Path, default=DISPOSITIONS)
    parser.add_argument("--assert-closeable", action="store_true",
                        help="exit 3 while any candidate is reviewable")
    args = parser.parse_args()
    try:
        inventory = json.loads(INVENTORY.read_text())
        document = json.loads(args.dispositions.read_text())
        report = validate(inventory, document)
    except (OSError, ValueError, KeyError, TypeError, DispositionError) as exc:
        print(json.dumps({"result": "INVALID_DISPOSITIONS", "reason": str(exc)}, sort_keys=True))
        return 2
    if args.assert_closeable and report["reviewable"]:
        report["result"] = "SECTION6_SIGNOFF_NOT_CLOSEABLE"
        print(json.dumps(report, sort_keys=True))
        return 3
    print(json.dumps(report, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
