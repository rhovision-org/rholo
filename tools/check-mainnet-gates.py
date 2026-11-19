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
import datetime as dt
import hashlib
import importlib.util
import json
import re
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
AUDIT = ROOT / "spec" / "audit"
REGISTER = AUDIT / "mainnet-gates.json"
RENDERED = AUDIT / "mainnet-gates.md"
MATRIX = AUDIT / "final-gap-matrix.json"
INVENTORY = AUDIT / "section6-gap-inventory.json"
LEDGER = AUDIT / "hardfork-51-closeout.json"
EXTERNAL = AUDIT / "mainnet-gates.external.json"
CRITERIA = AUDIT / "gate-criteria.json"
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
FAMILIES = {"G428", "G584", "TA", "S6", "C343", "G520"}
PINNED_EXTERNAL = {"G428", "G584", "TA"}


class GateError(ValueError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise GateError(message)


def canonical_hash(value: object) -> str:
    raw = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(raw).hexdigest()


def _load_tool(name: str, filename: str):
    spec = importlib.util.spec_from_file_location(name, ROOT / "tools" / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


witness = _load_tool("gate_witness", "gate-witness.py")


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


def normalize_source(text: str) -> str:
    return re.sub(r"\s+", " ", text.replace("*", "").replace("`", "")).strip().lower()


def resolve_test_ref(ref: str) -> None:
    require(isinstance(ref, str) and "::" in ref,
            "a negative control must name a test as path::symbol")
    path_text, symbol = ref.split("::", 1)
    path = ROOT / path_text
    require(path.is_file(), f"control path {path_text} does not exist")
    text = path.read_text(errors="ignore")
    if path.suffix == ".rs":
        require(witness.test_attribute(text, symbol),
                f"control {ref} is not a #[test]")
    elif path.suffix == ".py":
        require(re.search(rf"\bdef\s+{re.escape(symbol)}\b", text)
                and (symbol.startswith("test") or symbol == "self_test"),
                f"control {ref} is not a test function or self_test")
    else:
        require(symbol in text, f"control symbol {symbol} not found in {path_text}")


def record_path(value: str) -> Path:
    path = Path(value)
    return path if path.is_absolute() else ROOT / path


def verify_record(value: object, gate_id: str, verdict: str) -> dict:
    require(isinstance(value, str) and value.strip(),
            f"{gate_id}: an evidence record path is required")
    path = record_path(value)
    require(path.is_dir(), f"{gate_id}: evidence record {path} is not a directory")
    require((path / "manifest.json").is_file() and (path / "SHA256SUMS.json").is_file(),
            f"{gate_id}: evidence record needs manifest.json and SHA256SUMS.json")
    proc = subprocess.run(
        [sys.executable, str(ROOT / "tools/evidence-pack.py"), "verify",
         "--record", str(path), "--require-pins"],
        capture_output=True, text=True, timeout=120, check=False)
    require(proc.returncode == 0,
            f"{gate_id}: evidence-pack verify failed: {proc.stdout.strip()} {proc.stderr.strip()}")
    manifest = json.loads((path / "manifest.json").read_text())
    require(manifest.get("execution_proven") is False,
            f"{gate_id}: execution_proven must be false")
    named = manifest.get("gate_id") == gate_id or gate_id in (manifest.get("component_of") or [])
    require(named, f"{gate_id}: the evidence record names {manifest.get('gate_id')!r}")
    require(manifest.get("verdict") == verdict,
            f"{gate_id}: the evidence verdict is {manifest.get('verdict')!r}, not {verdict}")
    return manifest


_BUILDER = None


def load_builder():
    global _BUILDER
    if _BUILDER is None:
        spec = importlib.util.spec_from_file_location("build_mainnet_gates", BUILDER)
        if spec is None or spec.loader is None:
            raise GateError("cannot load the register builder")
        _BUILDER = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(_BUILDER)
    return _BUILDER


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
    witness_problem = witness.problem(kind, path_text, symbol)
    require(witness_problem is None, f"{row['id']}: {witness_problem}")
    binding = row.get("witness_binding")
    require(isinstance(binding, dict),
            f"{row['id']}: a met gate needs witness_binding")
    expected_asserts = hashlib.sha256(str(row["pass_condition"]).encode()).hexdigest()
    require(binding.get("asserts_sha256") == expected_asserts,
            f"{row['id']}: witness_binding.asserts_sha256 does not match the gate acceptance")
    control = binding.get("control") or {}
    require(isinstance(control, dict), f"{row['id']}: witness_binding.control is required")
    require(control.get("control_of") == ref,
            f"{row['id']}: the control must be of this witness")
    require(isinstance(control.get("description"), str) and control["description"].strip(),
            f"{row['id']}: the control needs a description")
    mutation = control.get("mutation") or {}
    mutation_file = ROOT / str(mutation.get("file") or "")
    require(mutation_file.is_file(),
            f"{row['id']}: the mutation file does not exist")
    before = mutation.get("before")
    after = mutation.get("after")
    require(isinstance(before, str) and before.strip()
            and isinstance(after, str) and after.strip() and before != after,
            f"{row['id']}: the mutation needs distinct before/after text")
    require(before in mutation_file.read_text(errors="ignore"),
            f"{row['id']}: the mutation's before text is not in the file")
    require(isinstance(control.get("test"), str) and control["test"].strip(),
            f"{row['id']}: the control test is required")
    resolve_test_ref(control["test"])
    require(control["test"] in (ref, f"{path_text}::self_test"),
            f"{row['id']}: the control test must be the witness or its self_test, "
            "in the witness's own file")
    require(control.get("expected") == "FAIL",
            f"{row['id']}: the control's expected result must be FAIL")
    verify_record(control.get("run_record"), row["id"], "FAIL")
    evidence = row.get("evidence") or {}
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
    manifest = verify_record(evidence.get("record"), row["id"], "PASS")
    require(manifest.get("binary_sha256") == binary,
            f"{row['id']}: the evidence binary does not match the record")
    if manifest.get("source_sha") is not None:
        require(manifest["source_sha"] == source,
                f"{row['id']}: the evidence source does not match the record")
    # **The declared digest is the record's manifest digest.** A format-only 64-hex value binds
    # nothing; the convention (also used by `s6-bind.py`) is sha256(manifest.json).
    manifest_path = record_path(evidence.get("record"))
    require(hashlib.sha256((manifest_path / "manifest.json").read_bytes()).hexdigest() == digest,
            f"{row['id']}: evidence.digest does not match the record's manifest")
    # The executable criterion is a promotion precondition, not a parallel note: the same
    # observation must still PASS, from the same verified record, on every check.
    criterion_problem = load_builder().criterion_binding_problem(
        row["id"], row.get("criterion"), manifest_path)
    require(criterion_problem is None, f"{row['id']}: {criterion_problem}")
    if kind == "human-attestation":
        attestation = row.get("attestation") or {}
        for field in ("reviewer", "relationship", "independent", "signed_at",
                      "verdict", "review_record", "evidence_sha256"):
            require(field in attestation, f"{row['id']}: attestation.{field} is required")
        require(attestation.get("independent") is True,
                f"{row['id']}: the reviewer must be independent")
        require(attestation.get("verdict") in ("PASS", "category-A-accepted"),
                f"{row['id']}: the attestation verdict must be PASS or category-A-accepted")
        signed_digest = attestation.get("evidence_sha256")
        require(isinstance(signed_digest, str) and HEX64.fullmatch(signed_digest),
                f"{row['id']}: attestation.evidence_sha256 must be a 64-hex digest")
        require(signed_digest == digest,
                f"{row['id']}: the reviewer-signed digest is not the evidence digest")
        owner = str(row.get("owner") or "").casefold()
        reviewer = str(attestation.get("reviewer") or "").casefold()
        require(reviewer and reviewer not in owner and owner not in reviewer,
                f"{row['id']}: the reviewer must not be the gate owner")
        try:
            signed = dt.datetime.fromisoformat(str(attestation["signed_at"]).replace("Z", "+00:00"))
        except ValueError as exc:
            raise GateError(f"{row['id']}: signed_at is not a UTC timestamp") from exc
        require(signed.tzinfo is not None, f"{row['id']}: signed_at must be timezone-aware")
        review_path = record_path(str(attestation["review_record"]))
        require(review_path.exists(), f"{row['id']}: the review record does not exist")
        review_text = ((review_path / "README.md").read_text(errors="ignore")
                       if review_path.is_dir() else review_path.read_text(errors="ignore"))
        require(str(attestation["reviewer"]) in review_text,
                f"{row['id']}: the review record does not name the reviewer")
        require(str(attestation["verdict"]) in review_text,
                f"{row['id']}: the review record does not carry the verdict")
        require(signed_digest in review_text,
                f"{row['id']}: the review record does not carry the evidence digest")


def _package_name(manifest: Path) -> str | None:
    """The `[package].name` of a manifest, or None when it is not a package manifest."""
    try:
        data = tomllib.loads(manifest.read_text(errors="ignore"))
    except tomllib.TOMLDecodeError:
        return None
    name = (data.get("package") or {}).get("name")
    return str(name) if isinstance(name, str) and name else None


def cargo_crate_root(test_path: Path, root: Path) -> Path | None:
    """The directory whose `Cargo.toml` names the package that compiles `test_path`."""
    directory = test_path.parent
    while True:
        manifest = directory / "Cargo.toml"
        if manifest.is_file() and _package_name(manifest) is not None:
            return directory
        if directory == root or directory.parent == directory:
            return None
        directory = directory.parent


def cargo_package(test_path: Path, root: Path) -> str | None:
    """The workspace package that compiles `test_path`, from its nearest `Cargo.toml`.

    Derived rather than mapped: `tools/check-rust-witnesses.sh` already answers the
    same question with a hand-maintained crate table, and a second copy in Python
    would be a second thing to drift. The workspace is one package per sbt module,
    so the path's nearest manifest is its package.
    """
    crate = cargo_crate_root(test_path, root)
    return None if crate is None else _package_name(crate / "Cargo.toml")


def cargo_target(test_path: Path, crate_root: Path) -> list[str]:
    """`--test <stem>` under the crate's `tests/`, `--lib` otherwise.

    The split is relative to the crate root, not the file's immediate parent, and it
    mirrors the shell witness runner: `--lib` keeps cargo out of the integration
    targets' build.
    """
    if test_path.relative_to(crate_root).parts[:1] == ("tests",) and \
            test_path.suffix == ".rs":
        return ["--test", test_path.stem]
    return ["--lib"]


def rust_control_verdict(output: str, symbol: str) -> str:
    """Classify a cargo run by libtest's own line for `symbol`.

    A nonzero exit is **not** a control: a compile error exits nonzero with no test
    line, and reading that as CONTROLLED would let a broken build masquerade as a
    failing witness. `ignored` is not a verdict either — the same trap the shell
    witness runner records (AUDIT C128): a filtered-in `#[ignore]` test prints
    `ignored` and `cargo test` exits 0.

    Exactly one line may name the symbol: a filter that matches two tests cannot say
    which one the mutation controlled, so a second match is an ERROR, not a pick.
    """
    name = re.escape(symbol)
    matches = re.findall(
        rf"^test (?:\S+::)?{name} \.\.\. (\S+)$", output, re.MULTILINE)
    if len(matches) != 1:
        return "ERROR"
    return {"FAILED": "CONTROLLED", "ok": "UNPROVEN"}.get(matches[0], "ERROR")


def _in_root(root: Path, value: object) -> Path | None:
    """A relative path under `root`; absolute, `..` or a resolving escape is refused."""
    text = str(value or "")
    raw = Path(text)
    if not text or raw.is_absolute() or ".." in raw.parts:
        return None
    lexical = root / raw
    try:
        lexical.resolve().relative_to(root.resolve())
    except ValueError:
        return None
    return lexical


def _clean_target(root: Path, path: Path) -> bool:
    """Refuse to mutate a file with uncommitted changes in a git worktree.

    A leftover mutation from a killed run, or an operator's work in progress, must be
    a refusal rather than a silent clobber. A non-git root (the toy test) is clean by
    construction.
    """
    if not (root / ".git").exists():
        return True
    relative = path.relative_to(root)
    for args in (("diff", "--quiet", "--", str(relative)),
                 ("diff", "--cached", "--quiet", "--", str(relative))):
        proc = subprocess.run(["git", "-C", str(root), *args],
                              capture_output=True, text=True, timeout=30, check=False)
        if proc.returncode != 0:
            return False
    return True


def run_control(row: dict, *, mode: str = "all", root: Path = ROOT,
                timeout: int = 1800, cargo_flags: tuple[str, ...] = ("--locked",),
                log_dir: Path | None = None) -> str:
    """Apply a control's declared mutation and run its control test.

    Returns CONTROLLED (the test failed as required), UNPROVEN (it passed),
    SKIPPED (not selected by `mode`), or ERROR (the control could not be run).

    A Rust control is run by name through cargo, and the verdict comes from the
    witness symbol's own libtest line — not from the process exit code. The
    mutation is restored in `finally`, and a restore that is not byte-identical
    is a register error, not a silent dirty tree.
    """
    binding = row.get("witness_binding") or {}
    control = binding.get("control") or {}
    mutation = control.get("mutation") or {}
    test = str(control.get("test") or "")
    path_text, symbol = test.split("::", 1) if "::" in test else (test, "")
    if path_text.endswith(".rs"):
        kind = "rust"
    elif symbol == "self_test" or (
            path_text.startswith("tests/") and path_text.endswith(".py")):
        kind = "python"
    else:
        return "ERROR"
    if mode != "all" and mode != kind:
        return "SKIPPED"
    path = _in_root(root, mutation.get("file"))
    if path is None or not path.is_file() or not _clean_target(root, path):
        return "ERROR"
    original = path.read_text()
    before = str(mutation.get("before") or "")
    after = str(mutation.get("after") or "")
    if not before or before not in original or before == after:
        return "ERROR"
    path.write_text(original.replace(before, after, 1))
    try:
        return _run_selected_control(row, path_text, symbol, kind, root, timeout,
                                     cargo_flags, log_dir)
    finally:
        path.write_text(original)
        if path.read_text() != original:
            raise GateError(
                f"{row.get('id')}: the control mutation could not be restored")


def _run_selected_control(row: dict, path_text: str, symbol: str, kind: str,
                          root: Path, timeout: int, cargo_flags: tuple[str, ...],
                          log_dir: Path | None) -> str:
    """Run one selected control and classify it.

    The Python/tool branches are unchanged. The Rust branch runs the witness by
    name after the mutation; its output is logged under `target/`, never `/tmp`.
    """
    if kind == "python":
        if symbol == "self_test":
            proc = subprocess.run(
                [sys.executable, str(root / path_text), "self-test"],
                capture_output=True, text=True, timeout=600, check=False)
        else:
            module = path_text[:-3].replace("/", ".")
            proc = subprocess.run(
                [sys.executable, "-m", "unittest", module, "-q"],
                capture_output=True, text=True, timeout=600, check=False)
        return "CONTROLLED" if proc.returncode != 0 else "UNPROVEN"
    test_path = _in_root(root, path_text)
    if test_path is None:
        return "ERROR"
    crate = cargo_crate_root(test_path, root)
    package = None if crate is None else _package_name(crate / "Cargo.toml")
    if crate is None or package is None:
        return "ERROR"
    verdict = "ERROR"
    try:
        proc = subprocess.run(
            ["cargo", "test", *cargo_flags, "-p", package,
             *cargo_target(test_path, crate), symbol],
            cwd=root, capture_output=True, text=True, timeout=timeout, check=False)
    except subprocess.TimeoutExpired as exc:
        output = ((exc.stdout or "") if isinstance(exc.stdout, str) else "") + \
                 ((exc.stderr or "") if isinstance(exc.stderr, str) else "")
    else:
        output = (proc.stdout or "") + (proc.stderr or "")
        verdict = rust_control_verdict(output, symbol)
    directory = log_dir or (root / "target" / "rust-controls")
    log_path = directory / f"{row.get('id')}.log"
    log_path.parent.mkdir(parents=True, exist_ok=True)
    log_path.write_text(output)
    if verdict != "CONTROLLED":
        print(f"{row.get('id')}: {verdict} (cargo log {log_path})", file=sys.stderr)
    return verdict


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
            if row["family"] != by_id[target]["family"]:
                require(re.search(
                    r"\b(not|only|subset|peer|separate|prerequisite|component)\b",
                    criterion, re.I),
                    f"{row['id']}: a cross-family edge to {target} needs a direction")
            reverse = by_id[target].get("related") or []
            require(any(back.get("id") == row["id"] for back in reverse),
                    f"{row['id']}: related edge to {target} is not symmetric")


def validate(document: dict, matrix: dict, inventory: dict, ledger: dict) -> dict:
    require(document.get("schema") == 1, "schema 1 required")
    require(HEX40.fullmatch(str(document.get("register_dev_sha"))),
            "register_dev_sha must be a commit")
    # **Ancestry is unprovable in a shallow checkout.** CI checks out at depth 1, where the
    # pinned dev commit is absent; failing there would make the register uncheckable in CI. The
    # report records the degradation, and a full checkout still enforces the ancestor rule.
    shallow = subprocess.run(
        ["git", "-C", str(ROOT), "rev-parse", "--is-shallow-repository"],
        capture_output=True, text=True, timeout=30, check=False)
    if shallow.returncode == 0 and shallow.stdout.strip() == "true":
        ancestry = "unverified-shallow"
    else:
        ancestor = subprocess.run(
            ["git", "-C", str(ROOT), "merge-base", "--is-ancestor",
             str(document["register_dev_sha"]), "HEAD"],
            capture_output=True, timeout=30, check=False)
        require(ancestor.returncode == 0,
                "register_dev_sha must be an ancestor of HEAD")
        ancestry = "verified"
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
    for key, source in sources.items():
        excerpt = source.get("excerpt")
        if not excerpt:
            continue
        path = ROOT / str(excerpt)
        require(path.is_file(), f"{key}: source excerpt {excerpt} is absent")
        require(source.get("excerpt_sha256")
                == hashlib.sha256(path.read_bytes()).hexdigest(),
                f"{key}: source excerpt hash is stale")
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
        if row["family"] in PINNED_EXTERNAL:
            require(isinstance(row.get("source_phrase"), str) and row["source_phrase"].strip(),
                    f"{ident}: a source phrase is required")
            source = sources.get(row["family"]) or {}
            excerpt_path = ROOT / str(source.get("excerpt") or "")
            require(excerpt_path.is_file(), f"{ident}: the source excerpt is absent")
            require(normalize_source(row["source_phrase"])
                    in normalize_source(excerpt_path.read_text(errors="ignore")),
                    f"{ident}: the source phrase is not in the excerpt")
            criteria = "\n".join(str(row.get(field, "")) for field in
                                 ("title", "pass_condition", "fail_condition", "observable"))
            require(row.get("criteria_sha256")
                    == hashlib.sha256(criteria.encode()).hexdigest(),
                    f"{ident}: the criteria hash does not match the gate text")
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
    check_criteria(rows)
    check_met_ratchet(document, rows)
    unresolved = [row["id"] for row in rows if row["status"] != "met"]
    return {
        "result": "REGISTER_VALID",
        "gates": len(rows),
        "met": len(rows) - len(unresolved),
        "unresolved": len(unresolved),
        "unresolved_ids": unresolved,
        "ancestry": ancestry,
    }


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


_SELFTEST_CACHE: dict = {"key": None, "errors": []}


def _selftest_errors(entries: dict, principles) -> list[str]:
    """The criteria self-test, memoised by the bytes that define it and the entries fed in."""
    digest = hashlib.sha256()
    for path in (CRITERIA, ROOT / "tools" / "gate-principles.py",
                 ROOT / "tools" / "gate-selftest.py", ROOT / "tests" / "gate_fixtures.py"):
        digest.update(path.read_bytes())
    digest.update(json.dumps(entries, sort_keys=True).encode())
    key = digest.hexdigest()
    if _SELFTEST_CACHE["key"] != key:
        module = _load_tool("gate_selftest", "gate-selftest.py")
        _SELFTEST_CACHE["key"] = key
        _SELFTEST_CACHE["errors"] = module.run(entries, principles=principles)
    return list(_SELFTEST_CACHE["errors"])


def check_criteria(rows: list[dict]) -> None:
    """Validate the executable criterion beside each machine-checkable external gate.

    The prose `pass_condition` stays authoritative: the entry must hash it. Coverage is an
    equality over the external rows that declare an executable witness (or `machine_check`),
    so a new checkable gate cannot arrive without a criterion and a criterion cannot outlive
    its row.
    """
    if not CRITERIA.is_file():
        raise GateError("spec/audit/gate-criteria.json is absent")
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
    external = {row["id"]: row for row in json.loads(EXTERNAL.read_text())["gates"]}
    executable = {"python", "rust-unit", "rust-integration", "two-binary",
                  "local-devnet", "live-host"}
    expected = {
        gate_id for gate_id, row in external.items()
        if row.get("gate", True)
        and (row.get("machine_check") is True or row.get("witness_kind") in executable)
    }
    require(set(by_id) == expected,
            "criterion coverage differs: "
            f"extra={sorted(set(by_id) - expected)} missing={sorted(expected - set(by_id))}")
    rows_by_id = {row["id"]: row for row in rows}
    principle_path = ROOT / "tools" / "gate-principles.py"
    spec = importlib.util.spec_from_file_location("gate_principles_for_register", principle_path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    for gate_id, entry in by_id.items():
        require(gate_id in external, f"{gate_id}: criterion names no external gate")
        require(gate_id in rows_by_id, f"{gate_id}: criterion gate is not in the register")
        row = rows_by_id[gate_id]
        expected_hash = hashlib.sha256(str(row["pass_condition"]).encode()).hexdigest()
        require(entry.get("pass_condition_sha256") == expected_hash,
                f"{gate_id}: the criterion does not hash the gate's pass_condition")
        principle = entry.get("principle")
        require(principle in module.PRINCIPLES, f"{gate_id}: unknown principle {principle!r}")
        for error in module.validate_parameters(principle, entry.get("parameters")):
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
    # **A criterion that has never been shown to fail is not a criterion.** The shape checks
    # above cannot see a principle that passes a control it declares; the behavioural self-test
    # can, and it runs here so the CI-wired register check executes it.
    errors = _selftest_errors(by_id, module)
    require(not errors, "the criteria self-test failed: " + "; ".join(errors[:5]))


def check_met_ratchet(document: dict, rows: list[dict]) -> None:
    """The met count is a ratchet: it cannot regress, and every met row is authored.

    The ratchet is a floor plus the exact set of authored promotions, so a
    hand-edited register that drops a binding or adds a `met` row without one is
    refused even when `--check` is not passed.
    """
    ratchet = document.get("met_ratchet")
    require(isinstance(ratchet, dict), "met_ratchet is required")
    minimum = ratchet.get("minimum")
    required = ratchet.get("required_ids")
    require(type(minimum) is int and minimum >= 0,
            "met_ratchet.minimum must be a non-negative integer")
    require(isinstance(required, list)
            and all(isinstance(ident, str) and ident for ident in required),
            "met_ratchet.required_ids must be a list of gate ids")
    require(len(required) == len(set(required)),
            "met_ratchet.required_ids has duplicates")
    require(minimum == len(required),
            "met_ratchet.minimum must equal the required-id count")
    floor = getattr(load_builder(), "MINIMUM_MET", 0)
    require(minimum >= floor,
            f"met_ratchet.minimum {minimum} is below the floor {floor}")
    by_id = {row["id"]: row for row in rows}
    for ident in required:
        require(ident in by_id, f"met_ratchet names unknown gate {ident}")
        require(by_id[ident]["status"] == "met",
                f"met_ratchet gate {ident} is {by_id[ident]['status']}")
    met = {row["id"] for row in rows if row["status"] == "met"}
    require(met == set(required),
            f"the met set {sorted(met)} does not equal the ratchet {sorted(set(required))}")


def unproven_controls(controls: dict) -> dict:
    """The controls that did not demonstrate their failure.

    SKIPPED is not one, but it is only honest when it means "not selected by
    `--controls`"; the default mode is `all`, so the full run has no SKIPPED rows.
    """
    return {
        ident: result for ident, result in controls.items()
        if result in ("UNPROVEN", "ERROR")
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("register", nargs="?", type=Path, default=REGISTER)
    parser.add_argument("--check", action="store_true",
                        help="compare the committed register with a fresh build")
    parser.add_argument("--assert-closeable", action="store_true",
                        help="exit 3 unless every gate is met (implies --check)")
    parser.add_argument("--run-controls", action="store_true",
                        help="apply each met gate's mutation and run its control test")
    parser.add_argument("--controls", choices=("python", "rust", "all"), default=None,
                        help="which control kinds to execute (implies --run-controls; "
                             "default: all)")
    parser.add_argument("--control-timeout", type=int, default=1800,
                        help="seconds one Rust control may take (default: 1800)")
    args = parser.parse_args()
    if args.controls is not None:
        args.run_controls = True
    args.controls = args.controls or "all"
    if args.assert_closeable:
        # Closeability must not trust a hand-edited register: compare it to the build.
        args.check = True
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
        if args.run_controls:
            # One log directory per invocation: a fixed per-gate path is shared state,
            # the same concurrency hazard `check-rust-witnesses.sh` records.
            base = ROOT / "target" / "rust-controls"
            base.mkdir(parents=True, exist_ok=True)
            log_dir = Path(tempfile.mkdtemp(prefix="run-", dir=base))
            report["controls_mode"] = args.controls
            report["controls"] = {
                row["id"]: run_control(row, mode=args.controls,
                                       timeout=args.control_timeout,
                                       log_dir=log_dir)
                for row in document["gates"] if row.get("status") == "met"
            }
    except (OSError, ValueError, KeyError, TypeError, GateError) as exc:
        print(json.dumps({"result": "INVALID_REGISTER", "reason": str(exc)}, sort_keys=True))
        return 2
    if args.assert_closeable and report["unresolved"]:
        report["result"] = "TRACKER_MUST_REMAIN_OPEN"
        print(json.dumps(report, sort_keys=True))
        return 3
    if args.run_controls:
        unproven = unproven_controls(report["controls"])
        if args.controls == "all":
            # In the default mode nothing may be unrun: a control the runner could not
            # classify is not a pass, and "all" must not hide a SKIPPED.
            unproven.update({ident: "SKIPPED"
                             for ident, result in report["controls"].items()
                             if result == "SKIPPED"})
        if unproven:
            report["result"] = "UNPROVEN_CONTROL"
            report["unproven"] = unproven
            print(json.dumps(report, sort_keys=True))
            return 1
    print(json.dumps(report, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
