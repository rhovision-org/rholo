#!/usr/bin/env python3
"""Build the canonical mainnet-gate register from the authored ledgers.

The external gate families (M1-M6, G1-G6, the testnet criteria, C343/C357,
#520) are hand-authored in `spec/audit/mainnet-gates.external.json`; the #51
and section-6 families are derived from their existing ledgers so those rows
are not copied by hand. Every derived row carries the source row's hash, so a
ledger edit that is not reflected here is a drift failure.

This builder is not an oracle. It records what the ledgers say and applies the
authored measurement bindings in `mainnet-gates.bindings.json`. A gate is
promoted to `met` only when a complete binding exists and the row is eligible:
a decision/blocked/precondition/superseded row is never eligible, an S6 row
needs an accepted disposition with a category-A effect, an HF51 row needs a
merged or verified ledger item, and every dependency must already be met. The
builder never invents a witness, a control or evidence; the checker
`tools/check-mainnet-gates.py` is the judge of the resulting `met` row.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
AUDIT = ROOT / "spec" / "audit"
OUT_JSON = AUDIT / "mainnet-gates.json"
OUT_MD = AUDIT / "mainnet-gates.md"
EXTERNAL = AUDIT / "mainnet-gates.external.json"
LEDGER = AUDIT / "hardfork-51-closeout.json"
INVENTORY = AUDIT / "section6-gap-inventory.json"
BINDINGS = AUDIT / "mainnet-gates.bindings.json"

REGISTER_DEV_SHA = "dd6a25d69128375d83b7a7701c169ce672e57536"
TEST_PATH = re.compile(
    r"((?:[A-Za-z0-9_./-]+\.(?:py|rs|sh|lean)::[A-Za-z0-9_:]+)"
    r"|(?:[A-Za-z0-9_./-]+/tests?/[A-Za-z0-9_./-]+\.(?:py|rs|sh|lean))"
    r"|(?:tests?/[A-Za-z0-9_./-]+\.(?:py|rs|sh|lean)))")
HEX40 = re.compile(r"^[0-9a-f]{40}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
WITNESS_KINDS = {
    "lean", "rust-unit", "rust-integration", "python", "two-binary",
    "local-devnet", "live-host", "human-attestation",
}
PROMOTABLE_STATUSES = {"owes", "external", "reviewable"}
INELIGIBLE_IDS = {"TA-C3"}


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def _load_tool(name: str, filename: str):
    spec = importlib.util.spec_from_file_location(name, ROOT / "tools" / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


witness = _load_tool("gate_witness", "gate-witness.py")


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
S6_CROSSWALK = {
    "S6:f735095c746556f3": [("G428-M5", "one merge-diff component")],
    "S6:5651614bf8e3eeb7": [("C357", "C357 replay-order component")],
    "S6:0963a698bc9c4ee4": [("G428-M4", "liveness-enforcement component")],
}


def load_dispositions() -> dict[str, dict]:
    path = AUDIT / "section6-dispositions.json"
    if not path.exists():
        return {}
    return {row["id"]: row for row in json.loads(path.read_text()).get("rows", [])}


def _verify_record(record: object, ident: str, label: str) -> Path:
    """The binding's evidence records must be verifiable, not merely present.

    This is the same rule `check-mainnet-gates.py::verify_record` applies. The builder runs it so
    a `met` row is never emitted from an empty or unpinned directory.
    """
    require(isinstance(record, str) and record.strip(),
            f"{ident}: the {label} record path is required")
    path = Path(record)
    if not path.is_absolute():
        path = ROOT / path
    require((path / "manifest.json").is_file() and (path / "SHA256SUMS.json").is_file(),
            f"{ident}: the {label} record needs manifest.json and SHA256SUMS.json")
    proc = subprocess.run(
        [sys.executable, str(ROOT / "tools/evidence-pack.py"), "verify",
         "--record", str(path), "--require-pins"],
        capture_output=True, text=True, timeout=120, check=False)
    require(proc.returncode == 0,
            f"{ident}: the {label} record does not verify: {proc.stderr.strip()}")
    return path


def _binding_shape(binding: dict, row: dict) -> None:
    """Refuse a binding that cannot be a witness/control/evidence record.

    The checker repeats these checks on the finished row; this copy exists so a
    malformed binding fails the build with the gate id, not only the render.
    """
    ident = binding["gate_id"]
    for field in ("witness_kind", "witness_ref", "witness_scope", "locality",
                  "sole_evidence", "does_not_claim"):
        require(isinstance(binding.get(field), str) and binding[field].strip(),
                f"{ident}: binding.{field} is required")
    require(type(binding.get("docker_free")) is bool,
            f"{ident}: binding.docker_free must be boolean")
    require(binding["witness_kind"] in WITNESS_KINDS,
            f"{ident}: unknown witness_kind {binding['witness_kind']!r}")
    ref = binding["witness_ref"]
    require("::" in ref, f"{ident}: witness_ref must be path::symbol")
    path_text, _, symbol = ref.partition("::")
    require((ROOT / path_text).is_file(),
            f"{ident}: witness path {path_text} does not exist")
    require(bool(symbol.strip()), f"{ident}: witness symbol is empty")
    # **The builder asks the checker's question.** A file and a non-empty symbol are not a
    # witness; the kind-specific rule lives in one shared resolver.
    witness_problem = witness.problem(binding["witness_kind"], path_text, symbol)
    require(witness_problem is None, f"{ident}: {witness_problem}")
    block = binding.get("witness_binding")
    require(isinstance(block, dict), f"{ident}: witness_binding is required")
    expected = hashlib.sha256(str(row["pass_condition"]).encode()).hexdigest()
    require(block.get("asserts_sha256") == expected,
            f"{ident}: witness_binding.asserts_sha256 does not match the gate acceptance")
    control = block.get("control")
    require(isinstance(control, dict), f"{ident}: witness_binding.control is required")
    require(control.get("control_of") == ref,
            f"{ident}: the control must be of this witness")
    require(isinstance(control.get("description"), str) and control["description"].strip(),
            f"{ident}: the control needs a description")
    mutation = control.get("mutation")
    require(isinstance(mutation, dict), f"{ident}: the control needs a mutation")
    mutation_file = ROOT / str(mutation.get("file") or "")
    require(mutation_file.is_file(), f"{ident}: the mutation file does not exist")
    before = mutation.get("before")
    after = mutation.get("after")
    require(isinstance(before, str) and before.strip()
            and isinstance(after, str) and after.strip() and before != after,
            f"{ident}: the mutation needs distinct before/after text")
    require(before in mutation_file.read_text(errors="ignore"),
            f"{ident}: the mutation's before text is not in the file")
    test = control.get("test")
    require(isinstance(test, str) and test.strip(),
            f"{ident}: the control test is required")
    require(test in (ref, f"{path_text}::self_test"),
            f"{ident}: the control test must be the witness or its self_test, "
            "in the witness's own file")
    require(control.get("expected") == "FAIL",
            f"{ident}: the control must expect FAIL")
    _verify_record(control.get("run_record"), ident, "control run")
    evidence = binding.get("evidence")
    require(isinstance(evidence, dict), f"{ident}: evidence is required")
    require(isinstance(evidence.get("url"), str)
            and evidence["url"].startswith("https://"),
            f"{ident}: evidence.url must be https")
    require(isinstance(evidence.get("source_sha"), str)
            and HEX40.fullmatch(evidence["source_sha"]),
            f"{ident}: evidence.source_sha must be a commit")
    for field in ("binary_sha256", "digest"):
        require(isinstance(evidence.get(field), str)
                and HEX64.fullmatch(evidence[field]),
                f"{ident}: evidence.{field} must be a 64-hex digest")
    record = _verify_record(evidence.get("record"), ident, "evidence")
    require(hashlib.sha256((record / "manifest.json").read_bytes()).hexdigest()
            == evidence["digest"],
            f"{ident}: evidence.digest does not match the record's manifest")


def _eligible(row: dict, met_ids: set[str]) -> tuple[bool, str]:
    ident = row["id"]
    if ident in INELIGIBLE_IDS:
        return False, "the row is not a pass/fail gate"
    if row["status"] not in PROMOTABLE_STATUSES:
        return False, f"status {row['status']!r} is not promotable"
    for dependency in row.get("depends_on") or []:
        if dependency not in met_ids:
            return False, f"dependency {dependency} is not met"
    return True, ""


def apply_bindings(rows: list[dict], document: dict) -> None:
    """Promote only complete, eligible authored bindings; verify the ratchet."""
    require(document.get("schema") == 1, "bindings schema 1 required")
    bindings = document.get("bindings")
    require(isinstance(bindings, list), "bindings must be a list")
    require(all(isinstance(binding, dict) for binding in bindings),
            "every binding must be an object")
    ids = [binding.get("gate_id") for binding in bindings]
    require(all(isinstance(ident, str) and ident for ident in ids),
            "every binding needs a gate_id")
    require(len(ids) == len(set(ids)), "duplicate binding gate_id")
    by_id = {row["id"]: row for row in rows}
    intended = set(ids)
    met_ids = {row["id"] for row in rows if row["status"] == "met"} | intended
    for binding in bindings:
        ident = binding["gate_id"]
        require(ident in by_id, f"{ident}: binding names no gate")
        row = by_id[ident]
        require(row["status"] != "met", f"{ident}: already met")
        eligible, reason = _eligible(row, met_ids)
        require(eligible, f"{ident}: {reason}")
        _binding_shape(binding, row)
        control = binding["witness_binding"]["control"]
        row.update({
            "witness_kind": binding["witness_kind"],
            "witness_ref": binding["witness_ref"],
            "witness_scope": binding["witness_scope"],
            "locality": binding["locality"],
            "sole_evidence": binding["sole_evidence"],
            "docker_free": binding["docker_free"],
            "negative_control": binding.get("negative_control") or control["description"],
            "does_not_claim": binding["does_not_claim"],
            "witness_binding": binding["witness_binding"],
            "evidence": binding["evidence"],
            "status": "met",
            "status_reason": None,
        })
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
    met = {row["id"] for row in rows if row["status"] == "met"}
    require(met == set(required),
            f"the met set {sorted(met)} does not equal the ratchet {sorted(set(required))}")


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
            # A withdrawn row is a recorded correction, not a gate.
            continue
        if test_status == "missing":
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
    ledger = json.loads(LEDGER.read_text())
    inventory = json.loads(INVENTORY.read_text())
    external = json.loads(EXTERNAL.read_text())
    bindings = (json.loads(BINDINGS.read_text()) if BINDINGS.exists() else
                {"schema": 1, "met_ratchet": {"minimum": 0, "required_ids": []},
                 "bindings": []})
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
    # A gate is a boolean test of the current build. The release/decision
    # obligations (HF51) and the sign-off/phantom-data rows are not gates; they
    # live in their own trackers and are filtered out here.
    rows = [row for row in external["gates"] if row.get("gate", True)]
    rows.extend(s6_rows(inventory))
    gate_ids = {row["id"] for row in rows}
    for row in rows:
        row["related"] = [edge for edge in row.get("related") or []
                          if edge.get("id") in gate_ids]
    apply_bindings(rows, bindings)
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
        "met_ratchet": bindings["met_ratchet"],
        "sources": external["sources"],
        "gates": rows,
    }


def render_md(document: dict) -> str:
    met = sum(1 for row in document["gates"] if row["status"] == "met")
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
        f"- met: {met}",
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
        "and every dependency is satisfied.",
        f"The register records **{met} of {len(document['gates'])} gates `met`**; every other",
        "gate is open with a stated reason.",
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
