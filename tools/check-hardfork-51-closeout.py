#!/usr/bin/env python3
"""Issue #51 evidence-led closeout. Never closes an issue or authorizes a release.

The register is an authored review record, not a live GitHub query. CI validates
its integrity and verifies the negative control. An independent operator must
verify current GitHub state and archived data before formally closing #51.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

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
PASSES_PATH = "spec/audit/passes.md"
DATE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
CI_RUN = re.compile(r"^https://github\.com/rhovision-org/rholo/actions/runs/\d+$")
TEST_BINDING = re.compile(r"^(?P<path>[^:\s]+\.rs)::(?P<symbol>[A-Za-z_][A-Za-z0-9_]*)$")


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


def repo_path(value: object, label: str, root: Path) -> Path:
    """Resolve an in-repo artifact path; a URL, an escape or a symlink is refused.

    Shape-only records were the fog: `artifact` was any non-empty string, so a
    fabricated acceptance record passed. The path must name real bytes, and a
    symlink is not a pinned artifact (the same rule `evidence-pack.py verify`
    applies to a record).
    """
    text = good_text(value, label)
    raw = Path(text)
    require(not raw.is_absolute() and ".." not in raw.parts and "://" not in text,
            f"{label}: a repo-relative path is required, not a URL")
    lexical = root / raw
    cursor = lexical
    while True:
        require(not cursor.is_symlink(), f"{label}: a symlink is not a pinned artifact")
        if cursor == root or cursor.parent == cursor:
            break
        cursor = cursor.parent
    resolved = lexical.resolve()
    require(resolved != root.resolve() and root.resolve() in resolved.parents,
            f"{label}: the path escapes the repo")
    require(resolved.exists(), f"{label}: {text} does not exist")
    return resolved


def artifact_digest(path: Path, label: str) -> str:
    """sha256 of the artifact's bytes, or of its manifest for an evidence record."""
    if path.is_dir():
        manifest = path / "manifest.json"
        require(manifest.is_file(), f"{label}: an evidence record needs manifest.json")
        return hashlib.sha256(manifest.read_bytes()).hexdigest()
    return hashlib.sha256(path.read_bytes()).hexdigest()


def record_digest(path: Path, label: str) -> str:
    """sha256 of an evidence record's manifest; a bare file is not a record."""
    require(path.is_dir() and (path / "manifest.json").is_file(),
            f"{label}: an evidence record directory with manifest.json is required")
    return hashlib.sha256((path / "manifest.json").read_bytes()).hexdigest()


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


def git_blob(repo: Path, rev: str, path: str) -> tuple[str, str | None, str | None]:
    """Read `path` at `rev` from git, never from the worktree.

    Returns `(status, text, reason)`: `verified` is the blob at the pinned commit;
    `unverified-shallow` and `unavailable` are recorded rather than raised, because
    a depth-1 checkout cannot prove the pin — the same boundary `is_ancestor`
    records. A full checkout whose pinned commit or path is absent is an error: the
    pin itself is malformed, not merely unprovable.
    """
    require(isinstance(rev, str) and HEXSHA.fullmatch(rev),
            "the pinned revision must be a commit")
    raw = Path(path)
    require(not raw.is_absolute() and ".." not in raw.parts and "://" not in path,
            "the pinned path must be repo-relative")
    shallow = subprocess.run(["git", "-C", str(repo), "rev-parse",
                              "--is-shallow-repository"], capture_output=True,
                             text=True, timeout=30, check=False)
    if shallow.returncode != 0:
        return "unavailable", None, "Git history unavailable"
    if shallow.stdout.strip() != "false":
        return "unverified-shallow", None, "a shallow checkout cannot prove the pinned blob"
    present = subprocess.run(["git", "-C", str(repo), "cat-file", "-e", f"{rev}^{{commit}}"],
                             capture_output=True, text=True, timeout=30, check=False)
    if present.returncode != 0:
        return "unavailable", None, "the pinned commit is not in this checkout"
    proc = subprocess.run(["git", "-C", str(repo), "show", f"{rev}:{path}"],
                          capture_output=True, text=True, timeout=120, check=False)
    require(proc.returncode == 0, f"the pinned blob {path} is absent at {rev}")
    return "verified", proc.stdout, None


def _gate_checker():
    path = Path(__file__).resolve().parent / "check-mainnet-gates.py"
    spec = importlib.util.spec_from_file_location("mainnet_gate_checker", path)
    if spec is None or spec.loader is None:
        raise ValueError("cannot load check-mainnet-gates.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


_INVENTORY_CHECKER = None


def _inventory_checker():
    """Load the §6 checker lazily; it imports this module at load time."""
    global _INVENTORY_CHECKER
    if _INVENTORY_CHECKER is None:
        path = Path(__file__).resolve().parent / "check-section6-inventory.py"
        spec = importlib.util.spec_from_file_location("section6_inventory", path)
        if spec is None or spec.loader is None:
            raise ValueError("cannot load check-section6-inventory.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        _INVENTORY_CHECKER = module
    return _INVENTORY_CHECKER


def _pinned_blob(raw: dict, root: Path) -> dict:
    """Validate the §6 inventory against the register's blob at `source_dev_sha`."""
    section6 = _inventory_checker()
    audit = root / "spec" / "audit"
    inventory = json.loads((audit / "section6-gap-inventory.json").read_text())
    matrix = json.loads((audit / "final-gap-matrix.json").read_text())
    return section6.validate_pinned(root, inventory, matrix, raw)


def merged_bindings(raw: dict, root: Path) -> dict[str, list[str]]:
    """Validate each merged row's current-tree acceptance binding.

    A merged row proves only that its merge commit is an ancestor; a later revert is
    also an ancestor. The binding names the test that must still pass, so the claim
    is falsifiable on the checkout under test. `tests` and `absent_reason` are
    exclusive: a row without a current-tree witness says so rather than inventing
    one.
    """
    gate = _gate_checker()
    out: dict[str, list[str]] = {}
    for item in raw["items"]:
        ident = item["id"]
        merged = item.get("status") == "merged"
        acceptance = item.get("acceptance")
        if not merged:
            require("acceptance" not in item,
                    f"{ident}: only a merged row can claim current-build acceptance")
            continue
        require(isinstance(acceptance, dict),
                f"{ident}: merged acceptance binding missing")
        has_tests = "tests" in acceptance
        has_absent = "absent_reason" in acceptance
        require(has_tests != has_absent,
                f"{ident}: acceptance needs exactly one of tests or absent_reason")
        if has_absent:
            reason = good_text(acceptance["absent_reason"],
                               ident + ".absent_reason", 10)
            require(not PLACEHOLDER.search(reason)
                    and not re.search(r"[\r\n\v\f\x1c-\x1e\x85  ]", reason)
                    and "\\n" not in reason and "\\r" not in reason
                    and len(reason) <= 256,
                    f"{ident}: absent_reason must be one specific, non-placeholder line")
            out[ident] = []
            continue
        tests = acceptance["tests"]
        require(isinstance(tests, list) and bool(tests),
                f"{ident}: acceptance tests must be a non-empty list")
        bindings: list[str] = []
        for entry in tests:
            require(isinstance(entry, str),
                    f"{ident}: each acceptance test must be a string")
            match = TEST_BINDING.fullmatch(entry.strip())
            require(match is not None,
                    f"{ident}: acceptance test must be <path.rs>::<symbol>")
            path_text, symbol = match.group("path"), match.group("symbol")
            test_path = repo_path(path_text, ident + ".acceptance", root)
            require(test_path.suffix == ".rs",
                    f"{ident}: an acceptance test must be Rust")
            text = test_path.read_text(errors="ignore")
            definitions = re.findall(
                rf"^\s*(?:pub\s+)?(?:async\s+)?fn\s+{re.escape(symbol)}\s*\(",
                text, re.MULTILINE)
            require(len(definitions) == 1,
                    f"{ident}: the acceptance symbol must be one fn in its file")
            crate = gate.cargo_crate_root(test_path, root)
            package = None if crate is None else gate._package_name(crate / "Cargo.toml")
            require(crate is not None and package is not None,
                    f"{ident}: the acceptance test must derive a workspace package")
            bindings.append(f"{path_text}::{symbol}")
        require(len(set(bindings)) == len(bindings),
                f"{ident}: duplicate acceptance test")
        out[ident] = bindings
    if out:
        observed = raw.get("acceptance_observed_date")
        require(isinstance(observed, str) and DATE.fullmatch(observed),
                "acceptance_observed_date must be an ISO date when merged rows are bound")
    return out


def merged_test_verdict(output: str, symbol: str) -> str:
    """Classify an acceptance run by the test's own libtest line.

    A nonzero exit is not a verdict: a build error exits nonzero producing no test
    line. Exactly one test may run — a filter that also selects a longer name cannot
    say which test the `ok` belongs to, so a collision is an ERROR, not a pick.
    """
    running = re.search(r"^running (\d+) tests?$", output, re.MULTILINE)
    matches = re.findall(
        rf"^test (?:\S+::)?{re.escape(symbol)} \.\.\. (\S+)$", output, re.MULTILINE)
    if running is None or running.group(1) != "1" or len(matches) != 1:
        return "ERROR"
    return {"ok": "ACCEPTED", "FAILED": "REJECTED"}.get(matches[0], "ERROR")


def run_merged_tests(raw: dict, *, repo: Path, timeout: int = 1800,
                     cargo_flags: tuple[str, ...] = ("--locked",),
                     log_dir: Path | None = None) -> dict:
    """Run every merged acceptance bound to a current-tree test, serially.

    Static binding validation runs first, so a missing symbol never reaches cargo.
    One log per test, under `target/hf51-merged/`; no source is mutated.
    """
    bindings = merged_bindings(raw, repo)
    gate = _gate_checker()
    base = repo / "target" / "hf51-merged"
    base.mkdir(parents=True, exist_ok=True)
    directory = log_dir or Path(tempfile.mkdtemp(prefix="run-", dir=base))
    directory.mkdir(parents=True, exist_ok=True)
    rows: dict[str, dict] = {}
    for item in raw["items"]:
        if item.get("status") != "merged":
            continue
        ident = item["id"]
        entries = bindings.get(ident, [])
        if "absent_reason" in item["acceptance"]:
            rows[ident] = {"verdict": "ABSENT", "tests": []}
            continue
        results = []
        for entry in entries:
            path_text, symbol = entry.split("::", 1)
            test_path = repo_path(path_text, ident + ".acceptance", repo)
            crate = gate.cargo_crate_root(test_path, repo)
            package = gate._package_name(crate / "Cargo.toml")
            target = gate.cargo_target(test_path, crate)
            try:
                proc = subprocess.run(
                    ["cargo", "test", *cargo_flags, "-p", package, *target, symbol],
                    cwd=repo, capture_output=True, text=True, timeout=timeout,
                    check=False)
                output = (proc.stdout or "") + (proc.stderr or "")
            except subprocess.TimeoutExpired as exc:
                output = (((exc.stdout or "") if isinstance(exc.stdout, str) else "")
                          + ((exc.stderr or "") if isinstance(exc.stderr, str) else ""))
            verdict = merged_test_verdict(output, symbol)
            safe = re.sub(r"[^A-Za-z0-9_.-]", "_", ident)
            log_path = directory / f"{safe}--{test_path.stem}--{symbol}.log"
            log_path.parent.mkdir(parents=True, exist_ok=True)
            log_path.write_text(output)
            if verdict != "ACCEPTED":
                print(f"{ident}: {verdict} (cargo log {log_path})", file=sys.stderr)
            results.append({"test": entry, "verdict": verdict, "log": str(log_path)})
        verdict = ("ERROR" if any(r["verdict"] == "ERROR" for r in results)
                   else "REJECTED" if any(r["verdict"] == "REJECTED" for r in results)
                   else "ACCEPTED")
        rows[ident] = {"verdict": verdict, "tests": results}
    head = None
    dirty = False
    if (repo / ".git").exists():
        top = subprocess.run(["git", "-C", str(repo), "rev-parse", "HEAD"],
                             capture_output=True, text=True, timeout=30, check=False)
        head = top.stdout.strip() if top.returncode == 0 else None
        status = subprocess.run(["git", "-C", str(repo), "status", "--porcelain"],
                                capture_output=True, text=True, timeout=60, check=False)
        dirty = status.returncode == 0 and bool(status.stdout.strip())
    return {"head": head, "dirty": dirty, "log_dir": str(directory), "rows": rows}


def gate_binding(repo: Path | None) -> tuple[bool, list[str], str | None]:
    """Bind each #51 row to its canonical-register status, when requested.

    This is deliberately non-fatal: a missing, stale or invalid register is an
    unbound gate (the tracker stays open), not a malformed ledger. The status
    strings are not trusted on their own: the register is validated first, so a
    hand-edited all-`met` file with no witness is unbound.
    """
    root = repo or Path(__file__).resolve().parents[1]
    audit = root / "spec" / "audit"
    path = audit / "mainnet-gates.json"
    try:
        document = json.loads(path.read_text())
    except OSError:
        return False, [], "mainnet-gates.json unavailable"
    except (ValueError, TypeError) as exc:
        return False, [], f"mainnet-gates.json invalid: {exc}"
    rows = [row for row in document.get("gates", []) if row.get("family") == "HF51"]
    if not rows:
        # HF51 is a release/decision ledger, not a gate family. The gate
        # register carries only boolean current-build tests; this ledger's own
        # pending/audit/closure fields decide #51.
        return True, [], "HF51 is tracked by this ledger, not the gate register"
    expected = {"HF51-" + ident.replace(":", "-") for ident in REQUIRED}
    actual = {row.get("id") for row in rows}
    unresolved = [row["id"] for row in rows if row.get("status") != "met"]
    try:
        matrix = json.loads((audit / "final-gap-matrix.json").read_text())
        inventory = json.loads((audit / "section6-gap-inventory.json").read_text())
        ledger = json.loads((audit / "hardfork-51-closeout.json").read_text())
        _gate_checker().validate(document, matrix, inventory, ledger)
    except (OSError, ValueError, KeyError, TypeError) as exc:
        # A shallow checkout cannot prove ancestry; keep the real unresolved list
        # so the caller reports the tracker open rather than an empty mismatch.
        return False, unresolved, f"register invalid: {exc}"
    if actual != expected:
        return False, sorted(expected - actual), "HF51 gate coverage mismatch"
    return not unresolved, unresolved, None


def validate(raw: dict, *, verify_git: bool = False, repo: Path | None = None,
             require_gates: bool = False, verify_blob: bool = False,
             merged_tests: dict[str, str] | None = None,
             merged_dirty: bool = False,
             acceptance_root: Path | None = None) -> dict:
    root = (repo or ROOT).resolve()
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
            approved_by = good_text(item.get("approved_by"), ident + ".approved_by")
            digest = item.get("evidence_sha256")
            # Format first: a placeholder digest is refused before the record is read.
            require(isinstance(digest, str) and SHA256.fullmatch(digest),
                    f"{ident}: evidence SHA-256 required")
            # **The signed digest is the record's digest, and the review carries it.**
            # A bare 64-hex string, two URLs and a name were the forgeable sign-off.
            evidence = repo_path(item.get("evidence_record"), ident + ".evidence_record", root)
            require(digest == record_digest(evidence, ident + ".evidence_record"),
                    f"{ident}: evidence_sha256 is not the evidence record's digest")
            review = repo_path(item.get("review_record"), ident + ".review_record", root)
            review_text = ((review / "README.md").read_text(errors="ignore")
                           if review.is_dir() else review.read_text(errors="ignore"))
            require(approved_by in review_text,
                    f"{ident}: the review record does not name the approver")
            require(digest in review_text,
                    f"{ident}: the review record does not carry the evidence digest")
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
    # **A merged row is an ancestry claim until a test is named.** The binding check
    # runs on every validation; the live run is the caller's explicit choice.
    # Acceptance paths name the checkout under test, which is `ROOT` unless the
    # caller says otherwise; `repo` may be a fixture root that hosts only records.
    bindings = merged_bindings(raw, (acceptance_root or ROOT).resolve())
    merged_ids = {item["id"] for item in items if item.get("status") == "merged"}
    if merged_tests is not None:
        require(isinstance(merged_tests, dict), "merged_tests must be a map")
        require(set(merged_tests) == merged_ids,
                "merged_tests must key exactly the merged rows")
        require(all(verdict in ("ACCEPTED", "REJECTED", "ERROR", "ABSENT")
                    for verdict in merged_tests.values()),
                "merged_tests verdicts must be ACCEPTED, REJECTED, ERROR or ABSENT")
    merged_acceptance_ok = (merged_tests is not None
                            and set(merged_tests) == merged_ids
                            and all(verdict == "ACCEPTED"
                                    for verdict in merged_tests.values())
                            and not merged_dirty)
    # **A pass must name an artifact, and one digest cannot both pass and fail.**
    # The campaign writer in tests/consensus-partition-rejoin.py emits this shape
    # (campaign.json); a green arm quoted beside a red one is a gate invalidation,
    # not a sampling nuisance to be re-rolled.
    records = raw.get("acceptance_records", [])
    require(isinstance(records, list), "acceptance_records must be a list")
    verdicts = {}
    for record in records:
        require(isinstance(record, dict), "every acceptance record must be an object")
        gate = good_text(record.get("gate_id"), "acceptance gate_id")
        source = record.get("source_sha")
        binary = record.get("binary_sha256")
        require(isinstance(source, str) and HEXSHA.fullmatch(source),
                f"{gate}: acceptance source_sha must be a commit")
        require(isinstance(binary, str) and SHA256.fullmatch(binary),
                f"{gate}: acceptance binary_sha256 must be a digest")
        aggregate = record.get("aggregate")
        require(aggregate in ("PASS", "FAIL"),
                f"{gate}: aggregate must be PASS or FAIL")
        good_text(record.get("artifact"), f"{gate}.artifact")
        # **The artifact is bytes, not a string.** Shape-only records let a
        # fabricated PASS skip every witness; the path must exist and the digest
        # must pin it (a directory is digested by its manifest).
        artifact = repo_path(record.get("artifact"), f"{gate}.artifact", root)
        artifact_sha = record.get("artifact_sha256")
        require(isinstance(artifact_sha, str) and SHA256.fullmatch(artifact_sha),
                f"{gate}: artifact_sha256 must be a digest")
        require(artifact_sha == artifact_digest(artifact, f"{gate}.artifact"),
                f"{gate}: artifact_sha256 does not match the artifact")
        if artifact.is_dir():
            manifest = json.loads((artifact / "manifest.json").read_text())
            for field, expected in (("source_sha", source),
                                    ("binary_sha256", binary),
                                    ("verdict", aggregate)):
                require(manifest.get(field) == expected,
                        f"{gate}: the artifact manifest's {field} does not match the record")
        key = (gate, source, binary)
        verdicts.setdefault(key, set()).add(aggregate)
        require(len(verdicts[key]) == 1,
                f"{gate}: PASS/FAIL conflict at one source and binary digest "
                "invalidates the gate; do not re-run to green")
    gates_bound = None
    gates_unresolved: list[str] = []
    gates_error = None
    if require_gates:
        gates_bound, gates_unresolved, gates_error = gate_binding(repo)
    # **The §6 inventory is bound to the bytes at the pinned commit.** A shallow
    # checkout records the pin as unverified rather than reading the worktree.
    pinned = _pinned_blob(raw, root) if verify_blob else None
    pinned_status = None if pinned is None else pinned.get("blob_status")
    closeable = (not pending and completed_audit and raw["closure_requested"] is True
                 and verify_git and (gates_bound is True if require_gates else True)
                 and merged_acceptance_ok
                 and (pinned_status == "verified" if verify_blob else True))
    return {
        "result": "CLOSEOUT_ELIGIBLE_FOR_INDEPENDENT_REVIEW" if closeable else "TRACKER_MUST_REMAIN_OPEN",
        "merged": len(merged), "deferred": len(deferred), "verified": len(verified),
        "audit_signoff_complete": completed_audit, "unresolved": len(pending),
        "unresolved_ids": pending,
        "verified_local_git_ancestry": verify_git,
        "closure_requested": raw["closure_requested"],
        "gates_bound": gates_bound,
        "gates_unresolved": gates_unresolved,
        "gates_error": gates_error,
        "acceptance_bindings": {ident: len(entries) for ident, entries in bindings.items()},
        "merged_with_tests": sum(1 for entries in bindings.values() if entries),
        "merged_absent": sum(1 for entries in bindings.values() if not entries),
        "merged_tests": merged_tests,
        "merged_tests_ok": merged_acceptance_ok,
        "merged_tests_dirty": merged_dirty,
        "pinned_blob": pinned_status,
        "limitations": [
            "The authored ledger is not a live status feed",
            "Git ancestry cannot attest a release decision or archival completeness",
            "An eligibility result does not close Issue #51 or approve deployment",
        ],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("ledger", type=Path,
                        nargs="?", default=ROOT / "spec/audit/hardfork-51-closeout.json")
    parser.add_argument("--assert-closeable", action="store_true",
                        help="exit 3 unless all rows are resolved and git provenance verified")
    parser.add_argument("--verify-local-git", action="store_true",
                        help="cross-check merged commits against pinned dev commit (full history required)")
    parser.add_argument("--verify-pinned-blob", action="store_true",
                        help="validate the §6 inventory against passes.md at source_dev_sha")
    parser.add_argument("--run-merged-tests", action="store_true",
                        help="run each merged row's acceptance test and classify its own line")
    parser.add_argument("--merged-test-timeout", type=int, default=1800)
    parser.add_argument("--merged-test-log-dir", type=Path, default=None)
    parser.add_argument("--repo", type=Path, default=ROOT)
    args = parser.parse_args()
    if args.assert_closeable:
        # A closeable claim must not rest on an unverified pin. The blob is local
        # and cheap; a shallow checkout records it as unverified and stays open.
        args.verify_pinned_blob = True
    try:
        payload = json.loads(args.ledger.read_text(), object_pairs_hook=no_duplicates)
        detail = None
        merged_tests = None
        merged_dirty = False
        if args.run_merged_tests:
            detail = run_merged_tests(payload, repo=args.repo.resolve(),
                                      timeout=args.merged_test_timeout,
                                      log_dir=args.merged_test_log_dir)
            merged_tests = {ident: row["verdict"]
                            for ident, row in detail["rows"].items()}
            merged_dirty = detail["dirty"]
        result = validate(payload, verify_git=args.verify_local_git, repo=args.repo,
                          require_gates=args.assert_closeable,
                          verify_blob=args.verify_pinned_blob,
                          merged_tests=merged_tests, merged_dirty=merged_dirty,
                          acceptance_root=args.repo)
        if detail is not None:
            result["merged_tests_detail"] = detail
    except (OSError, ValueError, subprocess.SubprocessError) as exc:
        print(json.dumps({"result": "INVALID_LEDGER", "reason": str(exc)}, sort_keys=True))
        return 2
    print(json.dumps(result, indent=2, sort_keys=True))
    if args.assert_closeable and result["result"] != "CLOSEOUT_ELIGIBLE_FOR_INDEPENDENT_REVIEW":
        return 3
    if args.verify_pinned_blob and result.get("pinned_blob") != "verified":
        # An explicitly requested provenance check that could not be established is
        # not a pass: a shallow clone reports UNVERIFIED, never green.
        return 4
    if args.run_merged_tests and any(v != "ACCEPTED" for v in (merged_tests or {}).values()):
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
