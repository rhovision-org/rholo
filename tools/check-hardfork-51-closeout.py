#!/usr/bin/env python3
"""Issue #51 evidence-led closeout. Never closes an issue or authorizes a release.

The register is an authored review record, not a live GitHub query. CI validates
its integrity and verifies the negative control. An independent operator must
verify current GitHub state and archived data before formally closing #51.
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

REQUIRED = frozenset("""
A:block-timestamp A:binary-attachments B:binary-attachments B:string-functions
A:bigint-decimal A:double-mint A:hardfork1-migration A:vault-export
A:genesis-contract-upgrade A:shard-config A:block-merge A:rev-economics
A:validator-rewards A:unbond-throttle A:phlo-exchange A:storage-rent
A:prepaid-rholang A:asset-store A:decentralized-rspace B:staking-pools
B:rev-receipt B:genesis-contract-upgrade B:babel B:breaking-release
B:staking-pools-interface Batch:activation-schedule Batch:crypto-agility
Batch:governance-schedule Batch:state-preservation Batch:faucet-genesis
Batch:attestation-format Audit:section6-categoryA Audit:law52-partition
Audit:law53-estrangement Audit:active-set-draw Audit:parent-limit
Audit:sidecar-merge Release:attachment-upgrade Release:decimal-genesis
Audit:merge-resolution-stage4 Audit:inactivity-leak Decision:developer-allocation
Audit:cost-tracks-work Audit:cost-insert-and-tally
""".split())
HEXSHA = re.compile(r"^[0-9a-f]{40}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
AUDIT_SIGNOFF_ID = "Audit:section6-categoryA"
PLACEHOLDER = re.compile(r"\b(tbd|unknown|unspecified|unnamed|someday|later|future|unassigned)\b", re.I)
OWNER = re.compile(r"^@[a-zA-Z0-9](?:[a-zA-Z0-9]|-(?=[a-zA-Z0-9])){0,38}$")


class LedgerError(ValueError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise LedgerError(message)


def no_duplicates(pairs: list[tuple[str, object]]) -> dict:
    out = {}
    for key, val in pairs:
        require(key not in out, f"duplicate JSON key: {key}")
        out[key] = val
    return out


def good_text(value: object, label: str, minimum: int = 3) -> str:
    require(isinstance(value, str) and len(value.strip()) >= minimum, f"{label}: text required")
    return value.strip()


def url(value: object, label: str) -> str:
    value = good_text(value, label)
    require(value.startswith("https://github.com/") and " " not in value,
            f"{label}: GitHub evidence URL required")
    return value


def is_ancestor(repo: Path, ancestor: str, descendant: str) -> bool:
    shallow = subprocess.run(["git", "-C", str(repo), "rev-parse",
                             "--is-shallow-repository"], capture_output=True,
                            text=True, timeout=10, check=False)
    require(shallow.returncode == 0 and shallow.stdout.strip() == "false",
            "shallow or unavailable Git history cannot verify merge ancestry")
    p = subprocess.run(["git", "-C", str(repo), "merge-base", "--is-ancestor",
                        ancestor, descendant], capture_output=True, timeout=30, check=False)
    require(p.returncode in (0, 1), "Git history unavailable; a shallow clone is not proof")
    return p.returncode == 0


def validate(raw: dict, *, verify_git: bool = False, repo: Path | None = None) -> dict:
    require(isinstance(raw, dict) and type(raw.get("schema")) is int
            and raw["schema"] == 1, "schema 1 required")
    url(raw.get("tracker"), "tracker")
    sha = raw.get("source_dev_sha")
    require(isinstance(sha, str) and HEXSHA.fullmatch(sha), "source_dev_sha must be a commit")
    good_text(raw.get("observed_date"), "observed_date")
    require(type(raw.get("closure_requested")) is bool,
            "closure_requested must be an explicit boolean")
    good_text(raw.get("policy"), "policy", 80)
    items = raw.get("items")
    require(isinstance(items, list), "items must be a list")
    require(all(isinstance(item, dict) for item in items),
            "every item must be an object")
    ids = [item.get("id") for item in items]
    require(all(isinstance(i, str) for i in ids), "each item needs an id")
    require(len(set(ids)) == len(ids), "duplicate item ID")
    require(set(ids) == REQUIRED,
            f"inventory mismatch: missing={sorted(REQUIRED - set(ids))}; "
            f"unknown={sorted(set(ids) - REQUIRED)}")
    pending, merged, deferred, verified = [], [], [], []
    for item in items:
        ident = item["id"]
        good_text(item.get("title"), ident + ".title")
        state = item.get("status")
        require(state in ("open", "merged", "deferred", "verified"),
                f"{ident}: status must be open, merged, deferred or verified")
        if state == "merged":
            ev = item.get("evidence")
            require(isinstance(ev, dict), f"{ident}: merged evidence missing")
            pr = ev.get("pr")
            require(type(pr) is int and pr > 0, f"{ident}: merged PR number missing")
            commit = ev.get("merge_commit")
            require(isinstance(commit, str) and HEXSHA.fullmatch(commit),
                    f"{ident}: merged commit missing")
            good_text(item.get("notes"), ident + ".notes")
            if verify_git:
                require(repo is not None, "--repo required for git ancestry verification")
                require(is_ancestor(repo, commit, sha),
                        f"{ident}: merge commit is not an ancestor of source_dev_sha")
            merged.append(ident)
        elif state == "deferred":
            require(ident != AUDIT_SIGNOFF_ID,
                    "§6 inventory cannot be deferred: full classification is a closure prerequisite")
            owner = good_text(item.get("decision_owner"), ident + ".decision_owner")
            require(bool(OWNER.fullmatch(owner)),
                    f"{ident}: decision_owner must identify one accountable GitHub user (@login)")
            reason = good_text(item.get("reason"), ident + ".reason", 10)
            require(not re.search(r"[\r\n\v\f\x1c-\x1e\x85\u2028\u2029]", item["reason"])
                    and "\\n" not in reason and "\\r" not in reason and len(reason) <= 256
                    and not PLACEHOLDER.search(reason),
                    f"{ident}: deferral reason must be one specific, non-placeholder line")
            # The #51 close clause requires owner + reason, not a named release.
            # An optional decision link improves traceability but is not a prerequisite.
            if "approval_url" in item:
                url(item["approval_url"], ident + ".approval_url")
            deferred.append(ident)
        elif state == "verified":
            require(ident == AUDIT_SIGNOFF_ID,
                    f"{ident}: verified status cannot replace landed-or-deferred disposition")
            url(item.get("evidence_url"), ident + ".evidence_url")
            url(item.get("approval_url"), ident + ".approval_url")
            good_text(item.get("approved_by"), ident + ".approved_by")
            digest = item.get("evidence_sha256")
            require(isinstance(digest, str) and SHA256.fullmatch(digest),
                    f"{ident}: evidence SHA-256 required")
            verified.append(ident)
        else:
            good_text(item.get("decision_owner"), ident + ".decision_owner")
            refs = item.get("references")
            require(isinstance(refs, list) and bool(refs) and
                    all(isinstance(ref, str) and ref.strip() for ref in refs),
                    f"{ident}: references required")
            good_text(item.get("blocker"), ident + ".blocker", 15)
            pending.append(ident)
    completed_audit = AUDIT_SIGNOFF_ID in verified
    closeable = not pending and completed_audit and raw["closure_requested"] is True and verify_git
    return {
        "result": "CLOSEOUT_ELIGIBLE_FOR_INDEPENDENT_REVIEW" if closeable else "TRACKER_MUST_REMAIN_OPEN",
        "merged": len(merged), "deferred": len(deferred), "verified": len(verified),
        "audit_signoff_complete": completed_audit, "unresolved": len(pending),
        "unresolved_ids": pending,
        "verified_local_git_ancestry": verify_git,
        "closure_requested": raw["closure_requested"],
        "limitations": [
            "The authored ledger is not a live status feed",
            "Git ancestry cannot attest a release decision or archival completeness",
            "An eligibility result does not close Issue #51 or approve deployment",
        ],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("ledger", type=Path,
                        nargs="?", default=Path(__file__).resolve().parent.parent /
                        "spec/audit/hardfork-51-closeout.json")
    parser.add_argument("--assert-closeable", action="store_true",
                        help="exit 3 unless all rows are resolved and git provenance verified")
    parser.add_argument("--verify-local-git", action="store_true",
                        help="cross-check merged commits against pinned dev commit (full history required)")
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parent.parent)
    args = parser.parse_args()
    try:
        payload = json.loads(args.ledger.read_text(), object_pairs_hook=no_duplicates)
        result = validate(payload, verify_git=args.verify_local_git, repo=args.repo)
    except (OSError, ValueError, subprocess.SubprocessError) as exc:
        print(json.dumps({"result": "INVALID_LEDGER", "reason": str(exc)}, sort_keys=True))
        return 2
    print(json.dumps(result, indent=2, sort_keys=True))
    if args.assert_closeable and result["result"] != "CLOSEOUT_ELIGIBLE_FOR_INDEPENDENT_REVIEW":
        return 3
    return 0


if __name__ == "__main__":
    sys.exit(main())
