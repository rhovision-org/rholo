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
a decision/blocked/precondition/superseded row is never eligible, an HF51 row
needs a merged or verified ledger item, and every dependency must already be
met. The section-6 review is tracked in `section6-dispositions.json`; since
2026-11-18 it is not a gate condition. An executable-criterion gate also needs
a `criterion` block whose pinned observation the gate's own evaluator passes.
The
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
OUT_CAMPAIGNS_JSON = AUDIT / "release-campaigns.json"
OUT_CAMPAIGNS_MD = AUDIT / "release-campaigns.md"
CLAIMS = AUDIT / "release-claims.json"
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
# **The ratchet's floor is not a copy of the data it guards.** `minimum == len(required_ids)` is
# satisfied by lowering both; a promotion raises this constant in the same commit, and a demotion
# is then an explicit two-file change.
MINIMUM_MET = 47
# **The release claims are not gates.** They cannot be closed by a boolean test
# of the current build; the tracker must not shrink silently.
CLAIM_MINIMUM = 7


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def _load_tool(name: str, filename: str):
    spec = importlib.util.spec_from_file_location(name, ROOT / "tools" / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


witness = _load_tool("gate_witness", "gate-witness.py")
_CRITERIA = None


def criteria_entries():
    """The executable criteria and the evaluator, loaded once."""
    global _CRITERIA
    if _CRITERIA is None:
        principles = _load_tool("gate_principles_for_builder", "gate-principles.py")
        _CRITERIA = (principles, principles.criteria_by_gate())
    return _CRITERIA


def criterion_binding_problem(ident: str, block: object, record: Path) -> str | None:
    """Why this binding's criterion observation is not a PASS over pinned bytes, or None.

    The observation must be a file inside the verified evidence record, listed in its manifest,
    with the declared digest, and the gate's own criterion must return PASS on those bytes. The
    builder and the checker both call this; a `met` row for an executable-criterion gate cannot
    exist without it.
    """
    principles, entries = criteria_entries()
    entry = entries.get(ident)
    if entry is None:
        if block is not None:
            return "a criterion binding is present for a gate with no criterion entry"
        return None
    if not isinstance(block, dict):
        return "a criterion binding is required for this gate"
    relative = block.get("observations")
    if not isinstance(relative, str) or not relative:
        return "criterion.observations is required"
    candidate = Path(relative)
    record = Path(record).resolve()
    path = (record / relative).resolve()
    if candidate.is_absolute() or ".." in candidate.parts or not path.is_relative_to(record):
        return "criterion.observations escapes the evidence record"
    if not path.is_file():
        return f"criterion.observations {relative!r} does not exist in the record"
    try:
        manifest = json.loads((record / "manifest.json").read_text())
    except (OSError, ValueError) as error:
        return f"the evidence record manifest is unreadable: {error}"
    listed = {
        item.get("path"): item for item in manifest.get("files") or []
        if isinstance(item, dict)
    }.get(relative)
    if not isinstance(listed, dict):
        return "criterion.observations is not listed in the evidence record manifest"
    actual = hashlib.sha256(path.read_bytes()).hexdigest()
    if block.get("sha256") != actual or listed.get("sha256") != actual:
        return "criterion.sha256 does not match the observation's bytes"
    if block.get("verdict") != "PASS":
        return "criterion.verdict must be PASS"
    try:
        observations = json.loads(path.read_text())
    except ValueError as error:
        return f"criterion.observations is not valid JSON: {error}"
    result = principles.evaluate_entry(entry, observations)
    if result.get("status") != "PASS":
        return (f"the criterion is {result.get('status')}, not PASS: "
                f"{result.get('reasons')}")
    return None


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
    # **A criterion that is not run at promotion is decoration.** The observation is pinned in
    # the same verified record and the gate's own evaluator must return PASS on it.
    criterion_problem = criterion_binding_problem(ident, binding.get("criterion"), record)
    require(criterion_problem is None, f"{ident}: {criterion_problem}")


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


def apply_bindings(rows: list[dict], document: dict, floor: int = 0) -> None:
    """Promote only complete, eligible authored bindings; verify the ratchet.

    `floor` is the independent minimum the build enforces; synthetic tests pass 0.
    """
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
        # Only when present: a `criterion: null` on the 47 existing rows would make the
        # rebuilt register differ from the committed bytes for no reason.
        if "criterion" in binding:
            row["criterion"] = binding["criterion"]
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
    require(minimum >= floor,
            f"met_ratchet.minimum {minimum} is below the floor {floor}")
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


def _prepare_external(external: dict) -> None:
    """Pin the source excerpts and hash each row's authored criteria."""
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


def build() -> dict:
    ledger = json.loads(LEDGER.read_text())
    inventory = json.loads(INVENTORY.read_text())
    external = json.loads(EXTERNAL.read_text())
    bindings = (json.loads(BINDINGS.read_text()) if BINDINGS.exists() else
                {"schema": 1, "met_ratchet": {"minimum": 0, "required_ids": []},
                 "bindings": []})
    _prepare_external(external)
    # A gate is a boolean test of the current build. The release/decision
    # obligations (HF51) and the sign-off/phantom-data rows are not gates; they
    # live in their own trackers and are filtered out here.
    rows = [row for row in external["gates"] if row.get("gate", True)]
    rows.extend(s6_rows(inventory))
    gate_ids = {row["id"] for row in rows}
    for row in rows:
        row["related"] = [edge for edge in row.get("related") or []
                          if edge.get("id") in gate_ids]
    apply_bindings(rows, bindings, floor=MINIMUM_MET)
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


EVIDENCE = AUDIT / "evidence"
ENVIRONMENTS = ("local", "public")


def claim_module(row: dict):
    """Import a claim's test module; it carries run/check/fixture/CLAIM_ID."""
    path = ROOT / row["test"]
    spec = importlib.util.spec_from_file_location(
        "release_claim_" + row["id"].lower().replace("-", "_"), path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def claim_records(roots=(EVIDENCE,)):
    """Every claim.json under the given roots, newest last by `finished_at`."""
    found = []
    for root in roots:
        if not Path(root).exists():
            continue
        for path in sorted(Path(root).rglob("claim.json")):
            try:
                record = json.loads(path.read_text())
            except (OSError, ValueError):
                continue
            if isinstance(record, dict) and record.get("claim_id"):
                found.append((path, record))
    return found


def _claim_common():
    spec = importlib.util.spec_from_file_location(
        "release_claim_common", ROOT / "tests" / "release_claim_common.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _trace_anchors() -> dict:
    spec = importlib.util.spec_from_file_location(
        "trace_evaluate", ROOT / "tools" / "trace-evaluate.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.ANCHORS


def _reading_and_items(row: dict, entry: dict, record: dict | None,
                       arm: dict | None = None) -> dict:
    """The measured reading and the actions that close it, for one arm.

    A status says whether the predicate holds; a reading says what to do next. `unsafe-point` names
    the false predicate and its anchors; `model-incomplete` names the trace that is the gap;
    `evidence-insufficient` names the missing trace or coordinate. `safe` has no action.
    """
    ident = row["id"]
    arm_id = (arm or {}).get("id") or entry.get("_arm") or entry.get("_environment")
    environment = entry.get("_environment")
    arm_close = (arm or {}).get("close") or {}
    close = {**(row.get("close") or {}), **arm_close}
    next_pr = (arm or {}).get("next_pr") or (row.get("next_pr") or {}).get(environment)
    reasons = list(entry.get("reasons") or [])
    if entry.get("status") == "pass":
        return {"reading": "safe", "items": [], "next_pr": None}
    if record is None:
        if arm_close.get("not-observed"):
            action = arm_close["not-observed"]
        elif environment == "public":
            action = ("Supply the four-host public manifest (deploy id, marker, provenance) and "
                      f"run `{(row.get('rerun') or {}).get('public', 'the public command')}`.")
        elif arm_id in ("local", "public"):
            action = close.get("not-observed", "run the arm's test")
        else:
            action = (f"Run `python3 tools/release-check.py run --claim {ident} --arm {arm_id}` "
                      f"and commit its record; the arm's harness lands in "
                      f"`{next_pr or 'its action PR'}`.")
        return {
            "reading": "not-observed",
            "items": [{"item": f"{ident}/{arm_id}/observe",
                       "finding": "no committed run record",
                       "action": action}],
            "next_pr": next_pr,
        }
    if record.get("verdict") == "not run" or entry.get("status") == "not run":
        action = close.get("precondition", "supply the precondition and re-run")
        if arm_id not in ("local", "public") and not arm_close.get("precondition"):
            action = (f"The arm did not produce its observation; its harness lands in "
                      f"`{next_pr or 'its action PR'}`.")
        return {
            "reading": "precondition",
            "items": [{"item": f"{ident}/{arm_id}/precondition",
                       "finding": "; ".join(reasons) or "the record is not run",
                       "action": action}],
            "next_pr": next_pr,
        }
    common = _claim_common()
    traces = common.traces_from(record)
    evaluations = [(trace, common.evaluate_trace(trace)) for trace in traces]
    false = [(trace, evaluation) for trace, evaluation in evaluations if evaluation["false"]]
    if false:
        anchors = _trace_anchors()
        items = []
        for trace, evaluation in false:
            for predicate in evaluation["false"]:
                anchor = anchors.get((evaluation["kind"], predicate)) or {}
                items.append({
                    "item": f"{ident}/{arm_id}/{predicate}",
                    "finding": (f"{evaluation['kind']} trace (validator "
                                f"{trace.get('validator')}, site "
                                f"{trace.get('site') or trace.get('mode') or '-'}): "
                                f"predicate {predicate} false"),
                    "action": close.get("unsafe-point", "fix the false predicate at its anchor"),
                    "anchor": anchor,
                })
        return {"reading": "unsafe-point", "items": items, "next_pr": next_pr}
    missing = sorted({name for _, evaluation in evaluations for name in evaluation["missing"]})
    if not traces or missing:
        return {
            "reading": "evidence-insufficient",
            "items": [{"item": f"{ident}/{arm_id}/trace",
                       "finding": ("no decision trace in the record" if not traces
                                   else "trace lacks coordinate(s): " + ", ".join(missing)),
                       "action": close.get("evidence-insufficient",
                                           "capture the decision trace")}],
            "next_pr": next_pr,
        }
    return {
        "reading": "model-incomplete",
        "items": [{"item": f"{ident}/{arm_id}/model",
                   "finding": (f"every predicate holds on {len(traces)} trace(s), yet the claim "
                               "failed"),
                   "action": close.get("model-incomplete",
                                       "extend the model before changing the test")}],
        "next_pr": next_pr,
    }


READING_SEVERITY = {
    "not-observed": 6, "precondition": 5, "evidence-insufficient": 4,
    "unsafe-point": 3, "model-incomplete": 2, "safe": 1,
}


def claim_reading(derived: dict) -> str:
    """The worst required-arm reading; every arm is still shown separately."""
    required = [entry for entry in derived.values() if entry.get("required", True)]
    if not required:
        return "not-observed"
    return max((entry["action"].get("reading") or "not-observed" for entry in required),
               key=lambda reading: READING_SEVERITY.get(reading, 6))


def derive_claim(row: dict, roots=(EVIDENCE,)) -> dict:
    """The claim's reading and close action per arm, from the arm's newest record.

    There is no authored status. A record whose stored verdict says `pass` but whose predicate
    refutes it is a `fail`: the tracker measures the node, not the file. `record.get("arm")` is the
    arm; a legacy record with no arm maps by its environment, so the baseline `local`/`public` arms
    keep their history.
    """
    module = claim_module(row)
    derived = {}
    records = claim_records(roots)
    for arm in row.get("arms") or []:
        arm_id = arm.get("id")
        environment = arm.get("environment")
        candidates = [
            (path, record) for path, record in records
            if record.get("claim_id") == row["id"]
            and (record.get("arm") or record.get("environment")) == arm_id
        ]
        entry = {"status": "not run", "record": None, "reasons": [],
                 "required": bool(arm.get("required", True)),
                 "_environment": environment, "_arm": arm_id}
        newest_record = None
        if candidates:
            path, newest_record = max(
                candidates, key=lambda pair: str(pair[1].get("finished_at") or ""))
            try:
                verdict, reasons = module.check(newest_record)
            except Exception as error:  # noqa: BLE001 - a broken predicate is a fail
                verdict, reasons = "fail", [f"{type(error).__name__}: {error}"]
            if newest_record.get("verdict") == "not run":
                verdict = "not run"
            if newest_record.get("verdict") == "pass" and verdict != "pass":
                verdict = "fail"
                reasons = list(reasons) + ["the record says pass but its predicate refutes it"]
            entry.update({
                "status": verdict,
                "record": (str(path.relative_to(ROOT))
                           if Path(path).is_relative_to(ROOT) else str(path)),
                "reasons": list(reasons),
                "finished_at": newest_record.get("finished_at"),
                "source_sha": (newest_record.get("provenance") or {}).get("source_sha"),
                "binary_sha256": (newest_record.get("provenance") or {}).get("binary_sha256"),
            })
        entry["action"] = _reading_and_items(row, entry, newest_record, arm)
        entry["action"]["required"] = entry["required"]
        derived[arm_id] = entry
    for entry in derived.values():
        entry.pop("_environment", None)
        entry.pop("_arm", None)
    return derived


def campaigns(roots=(EVIDENCE,)) -> dict:
    """The release claims: observable node behaviours, each with an existing test.

    A gate is a boolean test of the current build. A release claim is a product
    behaviour that is re-runnable on the standard local devnet or on public
    testnet-4. The status is measured from committed run records; nothing is authored.
    """
    document = json.loads(CLAIMS.read_text())
    require(document.get("schema") == 3, "release-claims schema must be 3")
    rows = document.get("claims")
    require(isinstance(rows, list) and len(rows) >= CLAIM_MINIMUM,
            f"only {len(rows) if isinstance(rows, list) else 0} release claims; "
            f"floor {CLAIM_MINIMUM}")
    ids = [row.get("id") for row in rows]
    require(len(ids) == len(set(ids)), "release claim ids must be unique")
    required = ("title", "claim", "pass_condition", "fail_condition", "observable",
                "negative_control", "environment", "rerun", "test", "owner", "arms")
    for row in rows:
        ident = row.get("id")
        for field in required:
            require(row.get(field), f"{ident}: {field} is required")
        require(row["environment"].get("local") and row["environment"].get("public"),
                f"{ident}: both environments are required")
        require(row["rerun"].get("local") and row["rerun"].get("public"),
                f"{ident}: both re-run commands are required")
        arm_ids = [arm.get("id") for arm in row["arms"]]
        require(len(arm_ids) == len(set(arm_ids)), f"{ident}: arm ids must be unique")
        for arm in row["arms"]:
            require(arm.get("environment") in ENVIRONMENTS,
                    f"{ident}/{arm.get('id')}: environment must be local or public")
            require(arm.get("test_mode") and arm.get("behaviour"),
                    f"{ident}/{arm.get('id')}: test_mode and behaviour are required")
        test = ROOT / row["test"]
        require(test.is_file(), f"{ident}: test {row['test']} does not exist")
    superseded = document.get("superseded")
    require(isinstance(superseded, list) and superseded,
            "the superseded list must map the old rows")
    rendered = []
    for row in rows:
        derived = derive_claim(row, roots)
        rendered.append({
            **row,
            "status": derived,
            "action": {arm["id"]: derived[arm["id"]].get("action")
                       for arm in row["arms"]},
            "claim_reading": claim_reading(derived),
        })
    return {
        "schema": 3,
        "generated": "2026-11-20",
        "source": "spec/audit/release-claims.json",
        "review": document.get("source_review"),
        "rule": document.get("rule"),
        "campaigns": rendered,
        "action_prs": document.get("action_prs") or [],
        "superseded": superseded,
        "excluded_behaviours": document.get("excluded_behaviours") or [],
    }


def render_campaigns_md(document: dict) -> str:
    def cell(value: object) -> str:
        text = "" if value is None else str(value)
        return text.replace("|", "\\|").replace("\n", " ")

    def action_for(row: dict, arm: dict) -> dict:
        return (row.get("action") or {}).get(arm.get("id")) or {}

    def next_action(row: dict, arm: dict) -> str:
        items = action_for(row, arm).get("items") or []
        return items[0]["action"] if items else "—"

    arm_count = sum(len(row.get("arms") or []) for row in document["campaigns"])
    lines = [
        "# Release claims",
        "",
        "Emitted from `spec/audit/release-claims.json` by `tools/build-mainnet-gates.py`.",
        "",
        "Each claim is a gate with one executable test over a decision trace, exercised by named",
        "**arms**. An arm has its own environment, evidence locality and next PR; its state is a",
        "measured **reading** of the newest committed record. `safe` is the only reading that",
        "closes an arm. `unsafe-point` names a false predicate and its Rust and Lean anchors;",
        "`model-incomplete` names the trace that is the model gap; `evidence-insufficient` names",
        "the missing trace or coordinate; `precondition` and `not-observed` name the run that",
        "cannot yet be made. No status is authored.",
        "",
        f"- generated: {document['generated']}",
        f"- claims: {len(document['campaigns'])}",
        f"- arms: {arm_count}",
        f"- review: `{document.get('review')}`",
        "",
        "| claim | arm | env | locality | reading | next action | next PR |",
        "|---|---|---|---|---|---|---|",
    ]
    for row in document["campaigns"]:
        for arm in row.get("arms") or []:
            action = action_for(row, arm)
            pr = action.get("next_pr")
            pr_cell = f"`{cell(pr)}`" if pr else "—"
            lines.append(
                f"| `{cell(row['id'])}` | `{cell(arm['id'])}` | {cell(arm['environment'])} | "
                f"{cell(arm.get('locality'))} | **{cell(action.get('reading') or 'not-observed')}** | "
                f"{cell(next_action(row, arm))[:180]} | {pr_cell} |")
    if document.get("action_prs"):
        lines.extend(["", "## Next PRs", "",
                      "| PR | closes | scope |", "|---|---|---|"])
        for entry in document["action_prs"]:
            lines.append(
                f"| `{cell(entry.get('id'))}` | {', '.join(entry.get('claims') or [])} | "
                f"{cell(entry.get('scope'))} |")
    lines.extend(["", "---", ""])
    for row in document["campaigns"]:
        lines.extend([
            f"## {row['id']} — {row['title']}",
            "",
            f"**Claim reading.** `{row.get('claim_reading', 'not-observed')}` (the worst required "
            "arm).",
            "",
            f"**Claim.** {row['claim']}",
            "",
            f"**Pass.** {row['pass_condition']}",
            "",
            f"**Fail.** {row['fail_condition']}",
            "",
            f"**Observe.** {row['observable']}",
            "",
            f"**Control.** {row['negative_control']}",
            "",
            f"**Test.** `{row['test']}`",
            "",
            f"**Local.** {row['environment']['local']}",
            "",
            f"**Public.** {row['environment']['public']}",
            "",
            f"**Re-run (local).** `{row['rerun']['local']}`",
            "",
            f"**Re-run (public).** `{row['rerun']['public']}`",
            "",
        ])
        for arm in row.get("arms") or []:
            entry = (row.get("status") or {}).get(arm["id"]) or {"status": "not run"}
            action = action_for(row, arm)
            detail = entry.get("record") or "no committed run record"
            lines.append(
                f"**Arm `{arm['id']}`** ({arm['environment']}, `{arm.get('locality')}`) — "
                f"{arm.get('behaviour')}")
            lines.append("")
            lines.append(
                f"Reading: `{action.get('reading', 'not-observed')}` — {detail}")
            for item in action.get("items") or []:
                anchor = item.get("anchor") or {}
                suffix = ""
                if anchor.get("rust"):
                    suffix += f" Rust: `{anchor['rust']}`."
                if anchor.get("lean"):
                    suffix += f" Lean: `{anchor['lean']}`."
                lines.append(
                    f"- `{item.get('item')}` — {item.get('finding')}. "
                    f"**Next:** {item.get('action')}{suffix}")
            if not action.get("items"):
                lines.append("- No action: the reading is safe; carry the record forward.")
            if action.get("next_pr"):
                lines.append(f"- Next PR: `{action['next_pr']}`")
            observed = [reason for reason in (entry.get("reasons") or [])
                        if action.get("reading") != "safe"]
            for reason in observed[:4]:
                text = " ".join(str(reason).split())
                lines.append(f"  - observed: {text[:500]}")
            lines.append("")
        if row.get("supersedes"):
            lines.extend([f"**Supersedes.** {', '.join(row['supersedes'])}", ""])
        if row.get("does_not_claim"):
            lines.extend([f"**Does not claim.** {row['does_not_claim']}", ""])
        lines.extend(["---", ""])
    lines.extend([
        "## Superseded rows",
        "",
        "| old id | fate | home |",
        "|---|---|---|",
    ])
    for entry in document["superseded"]:
        lines.append(
            f"| `{cell(entry.get('id'))}` | {cell(entry.get('fate'))} | "
            f"{cell(entry.get('home'))} |")
    lines.extend([
        "",
        "## Formerly out of scope — now arms",
        "",
        "| behaviour | covering arms |",
        "|---|---|",
    ])
    for entry in document.get("excluded_behaviours") or []:
        lines.append(
            f"| {cell(entry.get('behaviour'))} | "
            f"{', '.join('`' + arm + '`' for arm in entry.get('arms') or [])} |")
    lines.extend([
        "",
        "`check-release-campaigns.py --assert-ready` exits 3 unless every required arm reads",
        "`safe`; every open reading names its next action and PR. A closed register is still not a",
        "release authorization.",
        "",
    ])
    return "\n".join(lines)


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
        f"The register records **{met} of {len(document['gates'])} gates `met`**.",
        "It contains only current-build gates; the observable release behaviours are claims in",
        "[`release-campaigns.md`](release-campaigns.md), and a closed register is not a release",
        "authorization.",
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
    campaign_doc = campaigns()
    campaign_payload = json.dumps(campaign_doc, indent=2, sort_keys=True) + "\n"
    campaign_md = render_campaigns_md(campaign_doc)
    if args.stdout:
        print(payload, end="")
        return 0
    if args.check:
        stale = []
        for path, expected in ((OUT_JSON, payload), (OUT_MD, md),
                               (OUT_CAMPAIGNS_JSON, campaign_payload),
                               (OUT_CAMPAIGNS_MD, campaign_md)):
            if not path.exists() or path.read_text() != expected:
                stale.append(str(path.relative_to(ROOT)))
        if stale:
            print(json.dumps({"result": "STALE", "files": stale}, sort_keys=True))
            return 1
        print(json.dumps({"result": "CURRENT", "gates": len(document["gates"]),
                          "claims": len(campaign_doc["campaigns"])}, sort_keys=True))
        return 0
    OUT_JSON.write_text(payload)
    OUT_MD.write_text(md)
    OUT_CAMPAIGNS_JSON.write_text(campaign_payload)
    OUT_CAMPAIGNS_MD.write_text(campaign_md)
    print(json.dumps({"result": "WROTE", "gates": len(document["gates"]),
                      "claims": len(campaign_doc["campaigns"])}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
