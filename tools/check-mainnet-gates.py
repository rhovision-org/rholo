#!/usr/bin/env python3
"""Check the canonical mainnet-gate register.

This is a measurement-binding checker, not a release oracle. A gate is `met`
only when its witness resolves, its negative control is recorded, its evidence
is pinned to a source commit and binary, its dependencies are met, and the
owner is accountable. Anything else is `owes`, `blocked`, `external`,
`decision`, `precondition`, `reviewable` or `superseded` with a reason.

`--assert-closeable` exits 3 while any gate is not met (the expected state
today). A malformed register exits 2. `--check` compares the committed JSON
and rendered markdown with a fresh build.
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import importlib.util
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
AUDIT = ROOT / "spec" / "audit"
REGISTER = AUDIT / "mainnet-gates.json"
RENDERED = AUDIT / "mainnet-gates.md"
MATRIX = AUDIT / "final-gap-matrix.json"
INVENTORY = AUDIT / "section6-gap-inventory.json"
LEDGER = AUDIT / "hardfork-51-closeout.json"
BUILDER = ROOT / "tools" / "build-mainnet-gates.py"

HEX40 = re.compile(r"^[0-9a-f]{40}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
DATE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
STATUSES = {
    "met", "blocked", "owes", "external", "decision",
    "precondition", "reviewable", "superseded",
}
LOCALITIES = {
    "unit", "integration", "two-binary", "local-devnet", "single-host",
    "live-host", "external", "human-attestation", "decision",
    "multi-host", "off-host", "reviewable", "unclassified",
}
WITNESS_KINDS = {
    "lean", "rust-unit", "rust-integration", "python", "two-binary",
    "local-devnet", "live-host", "human-attestation", "decision",
}
SOLE_EVIDENCE = {
    "none", "docker", "local-devnet", "live-host", "external", "multi-host",
}
FAMILIES = {"G428", "G584", "TA", "HF51", "S6", "C343", "C357", "G520"}
PINNED_EXTERNAL = {"G428", "G584", "TA"}


class GateError(ValueError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise GateError(message)


def canonical_hash(value: object) -> str:
    raw = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(raw).hexdigest()


def load_laws() -> dict[str, dict[str, str]]:
    path = ROOT / "spec" / "laws.tsv"
    index: dict[str, dict[str, str]] = {}
    with path.open(newline="") as handle:
        for row in csv.DictReader(handle, delimiter="\t"):
            number = str(row.get("number") or "").strip()
            clause = str(row.get("clause") or "").strip()
            if number:
                index[number + clause] = row
                index.setdefault(number, row)
    return index


def test_attribute(text: str, symbol: str) -> bool:
    lines = text.splitlines()
    for index, line in enumerate(lines):
        if re.search(rf"\bfn\s+{re.escape(symbol)}\b", line):
            window = "\n".join(lines[max(0, index - 3):index + 1])
            return "#[test]" in window or "#[tokio::test]" in window
    return False


def resolve_test_ref(ref: str) -> None:
    require(isinstance(ref, str) and "::" in ref,
            "a negative control must name a test as path::symbol")
    path_text, symbol = ref.split("::", 1)
    path = ROOT / path_text
    require(path.is_file(), f"control path {path_text} does not exist")
    text = path.read_text(errors="ignore")
    if path.suffix == ".rs":
        require(test_attribute(text, symbol),
                f"control {ref} is not a #[test]")
    elif path.suffix == ".py":
        require(re.search(rf"\bdef\s+{re.escape(symbol)}\b", text)
                and symbol.startswith("test"),
                f"control {ref} is not a test function")
    else:
        require(symbol in text, f"control symbol {symbol} not found in {path_text}")


def load_builder():
    spec = importlib.util.spec_from_file_location("build_mainnet_gates", BUILDER)
    if spec is None or spec.loader is None:
        raise GateError("cannot load the register builder")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def resolve_source(row: dict, matrix: dict, inventory: dict, ledger: dict) -> None:
    ref = row.get("source_ref")
    if row["family"] == "HF51":
        require(isinstance(ref, str) and ref.startswith("final-gap-matrix.json#items/"),
                f"{row['id']}: a matrix source_ref is required")
        ident = ref.split("#items/", 1)[1]
        require(row["id"] == "HF51-" + ident.replace(":", "-"),
                f"{row['id']}: source_ref {ident} does not name this gate")
        item = next((entry for entry in matrix["items"] if entry["id"] == ident), None)
        require(item is not None, f"{row['id']}: matrix item {ident} not found")
        authored = next((entry for entry in ledger["items"] if entry["id"] == ident), None)
        require(authored is not None, f"{row['id']}: ledger item {ident} not found")
        require(row["title"] == authored.get("title"),
                f"{row['id']}: title does not match the ledger row")
        require(row.get("source_hash") == canonical_hash(item),
                f"{row['id']}: matrix source hash is stale")
    elif row["family"] == "S6" and isinstance(ref, str) and ref.startswith(
            "section6-gap-inventory.json#rows/"):
        ident = ref.split("#rows/", 1)[1]
        require(row["id"] == "S6-" + ident.split(":", 1)[1],
                f"{row['id']}: source_ref {ident} does not name this gate")
        source = next((entry for entry in inventory["rows"] if entry["id"] == ident), None)
        require(source is not None, f"{row['id']}: inventory row {ident} not found")
        require(row["title"] == str(source.get("invariant") or "")[:160],
                f"{row['id']}: title does not match the inventory row")
        require(source.get("row_sha256") == row.get("source_sha256"),
                f"{row['id']}: inventory source row hash is stale")
    else:
        require(isinstance(row.get("source_url"), str)
                and row["source_url"].startswith("https://"),
                f"{row['id']}: an https source_url is required")
        digest = row.get("source_sha256")
        if row["family"] in PINNED_EXTERNAL:
            require(isinstance(digest, str) and HEX64.fullmatch(digest),
                    f"{row['id']}: a pinned external source SHA-256 is required")
        else:
            require(digest is None or (isinstance(digest, str) and HEX64.fullmatch(digest)),
                    f"{row['id']}: source_sha256 must be a digest or null")


def resolve_witness(row: dict) -> None:
    kind = row.get("witness_kind")
    require(kind in WITNESS_KINDS, f"{row['id']}: unknown witness_kind {kind!r}")
    ref = row.get("witness_ref")
    require(isinstance(ref, str) and "::" in ref,
            f"{row['id']}: a met witness must be path::symbol, not a bare file")
    path_text, symbol = ref.split("::", 1)
    path = ROOT / path_text
    require(path.is_file(), f"{row['id']}: witness path {path_text} does not exist")
    text = path.read_text(errors="ignore")
    if symbol.startswith("--"):
        require(symbol in text, f"{row['id']}: witness flag {symbol} not found")
    elif kind.startswith("rust-"):
        require(path.suffix == ".rs", f"{row['id']}: a rust witness must be a .rs file")
        require(test_attribute(text, symbol),
                f"{row['id']}: witness {ref} is not a #[test]")
    elif kind == "python":
        require(path.suffix in {".py", ".sh"},
                f"{row['id']}: a python witness must be .py or .sh")
        require(re.search(rf"\bdef\s+{re.escape(symbol)}\b", text) or symbol in text,
                f"{row['id']}: witness {symbol} not found in {path_text}")
    elif kind == "lean":
        require(path.suffix == ".lean"
                and re.search(rf"\btheorem\s+{re.escape(symbol)}\b", text),
                f"{row['id']}: a lean witness must be a theorem")
    elif kind == "human-attestation":
        require(path.suffix == ".md",
                f"{row['id']}: a human attestation must be a markdown record")
    elif kind == "decision":
        require(False, f"{row['id']}: a decision gate has no executable witness")
    control = row.get("negative_control")
    require(isinstance(control, dict),
            f"{row['id']}: a met gate needs a control object, not a string")
    for field in ("description", "control_ref", "expected"):
        require(isinstance(control.get(field), str) and control[field].strip(),
                f"{row['id']}: negative_control.{field} is required")
    require(control.get("expected") in ("FAIL", "PRECONDITION"),
            f"{row['id']}: negative_control.expected must be FAIL or PRECONDITION")
    resolve_test_ref(control["control_ref"])
    evidence = row.get("evidence") or {}
    record = evidence.get("record")
    require(isinstance(record, str) and (ROOT / record).is_dir(),
            f"{row['id']}: evidence.record must be a retained record directory")
    require((ROOT / record / "manifest.json").is_file()
            and (ROOT / record / "SHA256SUMS.json").is_file(),
            f"{row['id']}: evidence.record must carry manifest.json and SHA256SUMS.json")
    source = evidence.get("source_sha")
    binary = evidence.get("binary_sha256")
    digest = evidence.get("digest")
    url = evidence.get("url")
    require(isinstance(source, str) and HEX40.fullmatch(source),
            f"{row['id']}: evidence.source_sha must be a commit")
    require(isinstance(binary, str) and HEX64.fullmatch(binary),
            f"{row['id']}: evidence.binary_sha256 must be a digest")
    require(isinstance(digest, str) and HEX64.fullmatch(digest),
            f"{row['id']}: evidence.digest must be a digest")
    require(isinstance(url, str) and url.startswith("https://"),
            f"{row['id']}: evidence.url must be https")
    if kind == "human-attestation":
        attestation = row.get("attestation") or {}
        for field in ("reviewer", "relationship", "independent", "signed_at", "verdict"):
            require(field in attestation, f"{row['id']}: attestation.{field} is required")
        require(attestation.get("independent") is True,
                f"{row['id']}: the reviewer must be independent")
        require(attestation.get("verdict") in ("PASS", "category-A-accepted"),
                f"{row['id']}: the attestation verdict must be PASS or category-A-accepted")
        owner = str(row.get("owner") or "").casefold()
        reviewer = str(attestation.get("reviewer") or "").casefold()
        require(reviewer and reviewer not in owner and owner not in reviewer,
                f"{row['id']}: the reviewer must not be the gate owner")


def check_dependencies(rows: list[dict]) -> None:
    by_id = {row["id"]: row for row in rows}
    for row in rows:
        for dep in row.get("depends_on") or []:
            require(dep in by_id, f"{row['id']}: unknown dependency {dep}")
    visiting, done = set(), set()

    def visit(ident: str) -> None:
        if ident in done:
            return
        require(ident not in visiting, f"dependency cycle at {ident}")
        visiting.add(ident)
        for dep in by_id[ident].get("depends_on") or []:
            visit(dep)
        visiting.remove(ident)
        done.add(ident)

    for row in rows:
        visit(row["id"])
    for row in rows:
        if row["status"] == "met":
            for dep in row.get("depends_on") or []:
                require(by_id[dep]["status"] == "met",
                        f"{row['id']}: cannot be met while dependency {dep} is {by_id[dep]['status']}")


def check_related(rows: list[dict]) -> None:
    by_id = {row["id"]: row for row in rows}
    for row in rows:
        for edge in row.get("related") or []:
            require(isinstance(edge, dict), f"{row['id']}: related entries must be objects")
            target = edge.get("id")
            criterion = edge.get("criterion")
            require(isinstance(target, str) and target in by_id,
                    f"{row['id']}: related target {target!r} not found")
            require(isinstance(criterion, str) and len(criterion.strip()) >= 10,
                    f"{row['id']}: related edge to {target} needs a criterion")
            reverse = by_id[target].get("related") or []
            require(any(back.get("id") == row["id"] for back in reverse),
                    f"{row['id']}: related edge to {target} is not symmetric")


def validate(document: dict, matrix: dict, inventory: dict, ledger: dict) -> dict:
    require(document.get("schema") == 1, "schema 1 required")
    require(HEX40.fullmatch(str(document.get("register_dev_sha"))),
            "register_dev_sha must be a commit")
    ancestry = subprocess.run(
        ["git", "-C", str(ROOT), "merge-base", "--is-ancestor",
         str(document["register_dev_sha"]), "HEAD"],
        capture_output=True, timeout=30, check=False)
    require(ancestry.returncode == 0,
            "register_dev_sha must be an ancestor of HEAD")
    require(document.get("ledger_source_dev_sha") == ledger.get("source_dev_sha"),
            "the register must pin the ledger's source commit")
    rows = document.get("gates")
    require(isinstance(rows, list) and rows, "gates must be a non-empty list")
    ids = [row.get("id") for row in rows]
    require(len(ids) == len(set(ids)), "duplicate gate id")
    require({row.get("family") for row in rows} == FAMILIES,
            "every gate family must be represented")
    sources = document.get("sources") or {}
    for family, key in (("G428", "G428"), ("G584", "G584"), ("TA", "TA")):
        require(key in sources and HEX64.fullmatch(str(sources[key].get("sha256"))),
                f"{key}: a pinned source hash is required")
        family_rows = [row for row in rows if row["family"] == family]
        require(all(row["source_sha256"] == sources[key]["sha256"] for row in family_rows),
                f"{family}: a gate does not pin the family source hash")
    laws = load_laws()
    for row in rows:
        ident = row["id"]
        require(isinstance(ident, str) and ident.startswith(row["family"] + "-") or ident == row["family"],
                f"{ident}: id must be namespaced by family")
        for field in ("title", "pass_condition", "fail_condition", "observable",
                      "owner", "source_label", "source_url", "does_not_claim"):
            require(isinstance(row.get(field), str) and row[field].strip(),
                    f"{ident}: {field} is required")
        require(row.get("source_url", "").startswith("https://"),
                f"{ident}: source_url must be https")
        require(re.fullmatch(r"\d{4}-\d{2}-\d{2}", str(row.get("source_retrieved"))),
                f"{ident}: source_retrieved must be YYYY-MM-DD")
        status = row.get("status")
        require(status in STATUSES, f"{ident}: unknown status {status!r}")
        require(row.get("locality") in LOCALITIES, f"{ident}: unknown locality {row.get('locality')!r}")
        require(type(row.get("docker_free")) is bool, f"{ident}: docker_free must be boolean")
        require(row.get("sole_evidence") in SOLE_EVIDENCE,
                f"{ident}: unknown sole_evidence {row.get('sole_evidence')!r}")
        cited = row.get("laws", [])
        require(isinstance(cited, list), f"{ident}: laws must be a list")
        for law in cited:
            require(law in laws, f"{ident}: law {law} is not in laws.tsv")
            if status == "met":
                formal = str(laws[law].get("formal") or "").strip()
                require(formal not in {"", "-"},
                        f"{ident}: law {law} has no formal rung")
        if status != "met":
            require(isinstance(row.get("status_reason"), str) and row["status_reason"].strip(),
                    f"{ident}: a non-met gate needs a status_reason")
        if status == "met":
            require(row.get("locality") != "unclassified",
                    f"{ident}: an unclassified locality cannot be met")
            require(row.get("sole_evidence") != "docker",
                    f"{ident}: Docker cannot be the sole evidence for a met gate")
            resolve_witness(row)
        resolve_source(row, matrix, inventory, ledger)
    check_dependencies(rows)
    check_related(rows)
    unresolved = [row["id"] for row in rows if row["status"] != "met"]
    return {
        "result": "REGISTER_VALID",
        "gates": len(rows),
        "met": len(rows) - len(unresolved),
        "unresolved": len(unresolved),
        "unresolved_ids": unresolved,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("register", nargs="?", type=Path, default=REGISTER)
    parser.add_argument("--check", action="store_true",
                        help="compare the committed register with a fresh build")
    parser.add_argument("--assert-closeable", action="store_true",
                        help="exit 3 unless every gate is met")
    args = parser.parse_args()
    try:
        document = json.loads(args.register.read_text())
        matrix = json.loads(MATRIX.read_text())
        inventory = json.loads(INVENTORY.read_text())
        ledger = json.loads(LEDGER.read_text())
        if args.check:
            builder = load_builder()
            fresh = builder.build()
            payload = json.dumps(fresh, indent=2, sort_keys=True) + "\n"
            rendered = builder.render_md(fresh)
            stale = []
            if args.register.read_text() != payload:
                stale.append(str(args.register))
            if not RENDERED.exists() or RENDERED.read_text() != rendered:
                stale.append(str(RENDERED.relative_to(ROOT)))
            if stale:
                print(json.dumps({"result": "STALE", "files": stale}, sort_keys=True))
                return 1
        report = validate(document, matrix, inventory, ledger)
    except (OSError, ValueError, KeyError, TypeError, GateError) as exc:
        print(json.dumps({"result": "INVALID_REGISTER", "reason": str(exc)}, sort_keys=True))
        return 2
    if args.assert_closeable and report["unresolved"]:
        report["result"] = "TRACKER_MUST_REMAIN_OPEN"
        print(json.dumps(report, sort_keys=True))
        return 3
    print(json.dumps(report, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
