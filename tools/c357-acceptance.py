#!/usr/bin/env python3
"""c357-acceptance — the section-6 boundary record and its checker.

C357's remaining external obligation is a selected release with every validator activating in
lockstep, or a coordinated new genesis on unswept history; an independently accepted
retained-history sweep for an existing chain; and an independent category-A verdict. A mixed-version
rollout is refused. The owner's decision (2026-11-17) is that a source-pinned reconstructed trace
plus the independent reviewer's explicit acceptance of the reconstruction is the close condition,
with the original rejected bytes recorded as a residual.

The requirement list is read from the emitted Lean table (`spec/audit/c357-acceptance.tsv`); this
tool does not carry a second copy of the shape list. `check` returns `PASS`, `FAIL` (the record
contradicts the contract) or `PRECONDITION` (an external artefact is absent). `--assert-closeable`
turns anything but `PASS` into a non-zero exit.

`execution_proven` is always false: this tool validates a record, it does not prove a campaign ran.
"""
from __future__ import annotations

import argparse
import datetime as dt
import json
import re
import sys
import tempfile
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_TABLE = ROOT / "spec/audit/c357-acceptance.tsv"
SHAPES = ("coordinated-new-genesis", "retained-history-sweep")
CORE_REQUIREMENTS = {
    "campaignDigest", "releaseSelected", "lockstep", "noMixedVersion", "reviewerIndependent",
    "verdictAccepted", "incidentTrace",
}
HEX40 = re.compile(r"^[0-9a-fA-F]{40}$")
HEX64 = re.compile(r"^[0-9a-fA-F]{64}$")


def is_hex(value: Any, pattern: re.Pattern[str]) -> bool:
    return isinstance(value, str) and bool(pattern.match(value))


def is_https(value: Any) -> bool:
    return isinstance(value, str) and value.startswith("https://") and len(value) > 8


def load_table(path: Path) -> dict[str, list[str]]:
    requirements: dict[str, list[str]] = {}
    lines = path.read_text().splitlines()
    if not lines or not lines[0].startswith("shape\t"):
        raise SystemExit(f"c357-acceptance: {path} is not the acceptance table")
    for line in lines[1:]:
        if not line.strip():
            continue
        shape, reqs = line.split("\t")
        if shape not in SHAPES:
            raise SystemExit(f"c357-acceptance: unknown shape {shape!r} in the table")
        requirements[shape] = [r for r in reqs.split(",") if r]
    for shape in SHAPES:
        reqs = set(requirements.get(shape, []))
        missing = CORE_REQUIREMENTS - reqs
        if missing:
            raise SystemExit(
                f"c357-acceptance: the {shape} row lacks core requirement(s) {sorted(missing)}")
        if shape == "coordinated-new-genesis" and "noSweep" not in reqs:
            raise SystemExit("c357-acceptance: the new-genesis row lacks noSweep")
        if shape == "retained-history-sweep" and not {"sweepPresent", "sweepComplete"} <= reqs:
            raise SystemExit("c357-acceptance: the retained-history row lacks its sweep checks")
    return requirements


def parse_utc(value: Any) -> dt.datetime | None:
    if not isinstance(value, str):
        return None
    try:
        return dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None


def artifact_text(value: Any) -> str | None:
    if not isinstance(value, str) or not value.strip():
        return None
    path = Path(value)
    path = path if path.is_absolute() else ROOT / path
    if path.is_dir():
        path = path / "README.md"
    return path.read_text(errors="ignore") if path.is_file() else None


MAX_WINDOW = 600.0


def window_status(record: dict[str, Any]) -> tuple[float | None, str | None]:
    """The derived lockstep window, or why the campaign's window is invalid.

    The window is not an author-chosen number: it is `round_seconds x
    liveness_heights`, capped at the measurement ceiling. A declared value must
    equal the derivation, and the rationale is required.
    """
    campaign = record.get("campaign") or {}
    round_seconds = campaign.get("round_seconds")
    heights = campaign.get("liveness_heights")
    if not (isinstance(round_seconds, (int, float))
            and not isinstance(round_seconds, bool) and round_seconds > 0
            and isinstance(heights, int) and not isinstance(heights, bool) and heights > 0):
        return None, "round_seconds and liveness_heights must be positive"
    derived = float(round_seconds) * float(heights)
    if derived > MAX_WINDOW:
        return None, f"derived window {derived:.0f}s exceeds the {MAX_WINDOW:.0f}s maximum"
    rationale = campaign.get("lockstep_window_rationale")
    if not (isinstance(rationale, str) and rationale.strip()):
        return None, "lockstep_window_rationale is required"
    declared = campaign.get("lockstep_window_seconds")
    if declared is not None:
        if not (isinstance(declared, (int, float))
                and not isinstance(declared, bool) and declared > 0):
            return None, "lockstep_window_seconds must be a positive number"
        if abs(float(declared) - derived) > 1e-9:
            return None, f"declared window {declared} != derived {derived:.0f}s"
    return derived, None


def record_window(record: dict[str, Any]) -> float | None:
    return window_status(record)[0]


def derive_mixed(record: dict[str, Any], window: float | None) -> tuple[bool, str | None]:
    validators = record.get("validators") or []
    if not isinstance(validators, list) or len(validators) < 2:
        return True, "fewer than two validators cannot prove a lockstep span"
    declared_count = (record.get("campaign") or {}).get("validator_count")
    if declared_count is not None and declared_count != len(validators):
        return True, f"campaign validator_count {declared_count} != {len(validators)} records"
    for validator in validators:
        if not isinstance(validator, dict):
            return True, "a validator record is not an object"
        for field in ("new_version", "binary_sha256", "activation_height"):
            if validator.get(field) in (None, ""):
                return True, f"a validator is missing {field}"
    if any(v.get("still_serving_old") is True for v in validators):
        return True, "a validator still serves the old version"
    if len({v.get("new_version") for v in validators}) > 1:
        return True, "validators run different versions"
    if len({v.get("binary_sha256") for v in validators}) > 1:
        return True, "validators run different binaries"
    if len({v.get("activation_height") for v in validators}) > 1:
        return True, "validators activated at different heights"
    times = [parse_utc(v.get("activated_utc")) for v in validators]
    if any(t is None for t in times):
        return True, "an activation time is missing or unparseable"
    assert all(t is not None for t in times)
    span = (max(times) - min(times)).total_seconds()  # type: ignore[type-var]
    if window is not None and span > window:
        return True, f"activations span {span:.0f}s, beyond the {window:.0f}s lockstep window"
    return False, None


def check_requirement(name: str, record: dict[str, Any], window: float) -> tuple[str, str]:
    campaign = record.get("campaign") or {}
    owner = record.get("release_owner") or {}
    review = record.get("independent_review") or {}
    trace = record.get("incident_trace") or {}
    sweep = record.get("sweep") or {}
    validators = record.get("validators") or []
    if name == "campaignDigest":
        ok = (is_hex(campaign.get("digest"), HEX64)
              and is_hex(campaign.get("manifest_sha256"), HEX64)
              and is_hex(campaign.get("source_sha"), HEX40)
              and is_hex(campaign.get("artifact_digest"), HEX64)
              and is_hex(campaign.get("binary_sha256"), HEX64))
        return ("PASS", "the campaign is digest-pinned") if ok else (
            "FAIL", "the campaign is missing a pinned digest")
    if name == "releaseSelected":
        ok = (bool(owner.get("name"))
              and owner.get("selected_source_sha") == campaign.get("source_sha")
              and owner.get("selected_artifact_digest") == campaign.get("artifact_digest")
              and owner.get("selected_binary_sha256") == campaign.get("binary_sha256")
              and is_https(owner.get("decision_url")))
        return ("PASS", "the release owner selected the campaign pins") if ok else (
            "FAIL", "the release owner's selected pins do not match the campaign")
    if name == "lockstep":
        effective, problem = window_status(record)
        if problem:
            return ("FAIL", problem)
        if effective is None:
            return ("PRECONDITION", "no declared lockstep window")
        mixed, why = derive_mixed(record, effective)
        if mixed:
            return ("FAIL", why or "the activation is not lockstep")
        return ("PASS", "every validator activated in lockstep")
    if name == "noMixedVersion":
        effective, problem = window_status(record)
        if problem:
            return ("FAIL", problem)
        mixed, why = derive_mixed(record, effective)
        if mixed:
            return ("FAIL", why or "a mixed-version rollout is present")
        if record.get("mixed_versions") is True:
            return ("FAIL", "the record claims a mixed-version rollout")
        return ("PASS", "no mixed version")
    if name == "noSweep":
        ok = not sweep
        return ("PASS", "a new genesis carries no sweep") if ok else (
            "FAIL", "a coordinated new genesis carries a retained-history sweep")
    if name == "sweepPresent":
        return ("PASS", "the retained-history sweep is present") if sweep else (
            "FAIL", "the retained-history shape has no sweep")
    if name == "sweepComplete":
        ok = (sweep.get("complete") is True
              and is_hex(sweep.get("checkpoint_hash"), HEX64)
              and isinstance(sweep.get("checkpoint_height"), int)
              and bool(sweep.get("accepted_by"))
              and is_https(sweep.get("independent_acceptance_url")))
        return ("PASS", "the sweep is complete and pinned") if ok else (
            "FAIL", "the retained-history sweep is incomplete or unpinned")
    if name == "reviewerIndependent":
        reviewer = (review.get("reviewer") or "").casefold()
        owner_name = (owner.get("name") or "").casefold()
        operator = (record.get("operator") or "").casefold()
        independence = review.get("independence") or {}
        attested = (isinstance(independence, dict)
                    and independence.get("attested") is True
                    and all(isinstance(independence.get(field), str)
                            and independence[field].strip()
                            for field in ("relationship", "affiliation",
                                          "prior_authorship", "conflicts")))
        signed = parse_utc(review.get("signed_at"))
        review_text = artifact_text(review.get("review_record"))
        record_ok = (review_text is not None
                     and str(review.get("reviewer") or "").casefold() in review_text.casefold()
                     and str(review.get("verdict") or "") in review_text
                     and str(review.get("evidence_sha256") or "") in review_text)

        def overlaps(left: str, right: str) -> bool:
            return bool(left and right and (left in right or right in left))

        ok = (bool(reviewer)
              and not overlaps(reviewer, owner_name)
              and not overlaps(reviewer, operator)
              and attested
              and signed is not None and signed.tzinfo is not None
              and record_ok
              and review.get("verdict") == "category-A-accepted"
              and is_hex(review.get("evidence_sha256"), HEX64)
              and is_https(review.get("verdict_url")))
        return ("PASS", "the publisher is independent and accepted") if ok else (
            "FAIL", "the independent review is missing, unattested, self-authored or unpinned")
    if name == "verdictAccepted":
        effect = review.get("category_a_effect")
        derivation = review.get("category_a_derivation")
        ok = (review.get("verdict") == "category-A-accepted"
              and effect in ("state-hash", "replay-verdict", "wire-format")
              and isinstance(derivation, str) and derivation.strip())
        return ("PASS", "the category-A verdict is accepted and derived") if ok else (
            "FAIL", "the category-A verdict is absent, underived or not accepted")
    if name == "incidentTrace":
        mode = trace.get("mode")
        if mode not in ("original-bytes", "reconstructed"):
            return ("PRECONDITION", "the incident trace is missing")
        if not is_hex(trace.get("sha256"), HEX64) or not is_https(trace.get("provenance_url")):
            return ("FAIL", "the incident trace is not digest-pinned or has no provenance URL")
        if mode == "reconstructed":
            if getattr(record.get("_checker"), "require_original_trace", False):
                return ("PRECONDITION", "--require-original-trace and the trace is reconstructed")
            reconstruction = artifact_text(trace.get("reconstruction_record"))
            ok = (is_https(trace.get("owner_decision_url"))
                  and review.get("reconstruction_accepted") is True
                  and reconstruction is not None
                  and str(trace.get("sha256") or "") in reconstruction
                  and str(review.get("reviewer") or "").casefold() in reconstruction.casefold())
            return ("PASS", "the reconstructed trace is owner- and reviewer-accepted") if ok else (
                "FAIL", "a reconstructed trace needs the owner's decision, the reviewer's "
                        "acceptance and a digest-bound reconstruction record")
        return ("PASS", "the original incident bytes are pinned")
    return ("FAIL", f"unknown requirement {name!r} in the emitted table")


class _Checker:
    def __init__(self, require_original_trace: bool) -> None:
        self.require_original_trace = require_original_trace


def check_record(record: dict[str, Any], requirements: dict[str, list[str]],
                 require_original_trace: bool = False,
                 window: float | None = None) -> dict[str, Any]:
    record = dict(record)
    record["_checker"] = _Checker(require_original_trace)
    checks: dict[str, dict[str, str]] = {}
    shape = record.get("shape")
    if shape not in requirements:
        return {"result": "PRECONDITION", "checks": {},
                "detail": "the record names no admitted boundary shape"}
    for requirement in requirements[shape]:
        verdict, detail = check_requirement(requirement, record, window)
        checks[requirement] = {"verdict": verdict, "detail": detail}
    if record.get("execution_proven") is not False:
        checks["execution_proven"] = {
            "verdict": "FAIL", "detail": "the record must declare execution_proven false"}
    verdicts = [check["verdict"] for check in checks.values()]
    result = "FAIL" if "FAIL" in verdicts else (
        "PRECONDITION" if "PRECONDITION" in verdicts else "PASS")
    return {"result": result, "checks": checks, "shape": shape,
            "lockstep_window_seconds": record_window(record)}


def init_record(out: Path, shape: str | None) -> int:
    placeholder = "<fill: required>"
    record = {
        "schema": 1,
        "campaign": {"id": placeholder, "digest": placeholder, "manifest_sha256": placeholder,
                     "source_sha": placeholder, "artifact_digest": placeholder,
                     "binary_sha256": placeholder, "boundary_height": 0,
                     "lockstep_window_seconds": 0,
                     "lockstep_window_rationale": placeholder,
                     "round_seconds": 0, "liveness_heights": 0,
                     "validator_count": 0},
        "shape": shape or placeholder,
        "release_owner": {"name": placeholder, "selected_source_sha": placeholder,
                          "selected_artifact_digest": placeholder,
                          "selected_binary_sha256": placeholder, "decision_url": placeholder},
        "validators": [{"id": "B", "old_version": placeholder, "new_version": placeholder,
                        "activated_utc": placeholder, "activation_height": 0,
                        "binary_sha256": placeholder, "still_serving_old": False}],
        "sweep": {"checkpoint_hash": placeholder, "checkpoint_height": 0,
                  "accepted_by": [], "complete": False,
                  "independent_acceptance_url": placeholder},
        "mixed_versions": False,
        "independent_review": {"reviewer": placeholder, "verdict": placeholder,
                               "reconstruction_accepted": False, "verdict_url": placeholder,
                               "evidence_sha256": placeholder, "signed_at": placeholder,
                               "review_record": placeholder,
                               "category_a_effect": placeholder,
                               "category_a_derivation": placeholder,
                               "independence": {"attested": False,
                                                "relationship": placeholder,
                                                "affiliation": placeholder,
                                                "prior_authorship": placeholder,
                                                "conflicts": placeholder}},
        "incident_trace": {"mode": placeholder, "sha256": placeholder,
                           "provenance_url": placeholder, "owner_decision_url": placeholder},
        "execution_proven": False,
    }
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(record, indent=1, sort_keys=True) + "\n")
    print(f"c357-acceptance: wrote a skeleton to {out}")
    return 0


def _base_record(shape: str) -> dict[str, Any]:
    digest = "a" * 64
    source = "b" * 40
    owner = "@owner"
    validators = [
        {"id": "B", "old_version": "1", "new_version": "2",
         "activated_utc": "2026-11-17T20:00:00Z", "activation_height": 10,
         "binary_sha256": digest, "still_serving_old": False},
        {"id": "C", "old_version": "1", "new_version": "2",
         "activated_utc": "2026-11-17T20:00:05Z", "activation_height": 10,
         "binary_sha256": digest, "still_serving_old": False},
        {"id": "D", "old_version": "1", "new_version": "2",
         "activated_utc": "2026-11-17T20:00:10Z", "activation_height": 10,
         "binary_sha256": digest, "still_serving_old": False},
    ]
    return {
        "schema": 1,
        "campaign": {"id": "campaign", "digest": digest, "manifest_sha256": digest,
                     "source_sha": source, "artifact_digest": digest,
                     "binary_sha256": digest, "boundary_height": 10,
                     "lockstep_window_seconds": 30,
                     "lockstep_window_rationale": "30 s = three 10 s rounds; the campaign's declared round time.",
                     "round_seconds": 10, "liveness_heights": 3, "validator_count": 3},
        "shape": shape,
        "release_owner": {"name": owner, "selected_source_sha": source,
                          "selected_artifact_digest": digest,
                          "selected_binary_sha256": digest,
                          "decision_url": "https://example.invalid/decision"},
        "validators": validators,
        "sweep": ({"checkpoint_hash": digest, "checkpoint_height": 9,
                   "accepted_by": ["B", "C", "D"], "complete": True,
                   "independent_acceptance_url": "https://example.invalid/sweep"}
                  if shape == "retained-history-sweep" else {}),
        "mixed_versions": False,
        "independent_review": {"reviewer": "@reviewer", "verdict": "category-A-accepted",
                               "reconstruction_accepted": True,
                               "verdict_url": "https://example.invalid/verdict",
                               "evidence_sha256": digest,
                               "signed_at": "2026-11-17T21:00:00Z",
                               "review_record": "tests/fixtures/c357-review.md",
                               "category_a_effect": "replay-verdict",
                               "category_a_derivation": "the COMM order change moves a replay verdict; 1860/2606/4844 witness",
                               "independence": {
                                   "attested": True,
                                   "relationship": "not the release owner or operator",
                                   "affiliation": "independent reviewer",
                                   "prior_authorship": "none on the selected source or evidence",
                                   "conflicts": "none"}},
        "incident_trace": {"mode": "original-bytes", "sha256": digest,
                           "provenance_url": "https://example.invalid/trace",
                           "owner_decision_url": ""},
        "execution_proven": False,
    }


def _case(name: str, record: dict[str, Any], requirements: dict[str, list[str]],
          expected: bool, require_original_trace: bool = False) -> list[str]:
    result = check_record(record, requirements, require_original_trace)["result"]
    if expected and result != "PASS":
        return [f"{name}: expected PASS, got {result}"]
    if not expected and result == "PASS":
        return [f"{name}: expected a refusal, got PASS"]
    return []


def self_test() -> int:
    requirements = load_table(DEFAULT_TABLE)
    failures: list[str] = []
    failures += _case("valid-new-genesis", _base_record("coordinated-new-genesis"),
                      requirements, True)
    failures += _case("valid-retained-sweep", _base_record("retained-history-sweep"),
                      requirements, True)
    reconstructed = _base_record("retained-history-sweep")
    reconstructed["incident_trace"] = {
        "mode": "reconstructed", "sha256": "c" * 64,
        "provenance_url": "https://example.invalid/reconstructed",
        "owner_decision_url": "https://example.invalid/owner-decision",
        "reconstruction_record": "tests/fixtures/c357-reconstruction.md"}
    failures += _case("reconstructed-with-both-acceptances", reconstructed, requirements, True)
    mixed = _base_record("coordinated-new-genesis")
    mixed["validators"][0]["still_serving_old"] = True
    failures += _case("mixed-version", mixed, requirements, False)
    no_window = _base_record("coordinated-new-genesis")
    for key in ("lockstep_window_seconds", "round_seconds", "liveness_heights"):
        no_window["campaign"].pop(key, None)
    failures += _case("no-declared-window", no_window, requirements, False)
    no_independence = _base_record("coordinated-new-genesis")
    del no_independence["independent_review"]["independence"]
    failures += _case("no-independence-attestation", no_independence, requirements, False)
    unattested = _base_record("coordinated-new-genesis")
    unattested["independent_review"]["independence"]["attested"] = False
    failures += _case("unattested-independence", unattested, requirements, False)
    span = _base_record("coordinated-new-genesis")
    span["validators"][1]["activated_utc"] = "2026-11-17T20:00:20Z"
    span["validators"][2]["activated_utc"] = "2026-11-17T20:00:40Z"
    failures += _case("activation-span-beyond-window", span, requirements, False)
    huge = _base_record("coordinated-new-genesis")
    huge["campaign"]["lockstep_window_seconds"] = 999999
    failures += _case("huge-declared-window", huge, requirements, False)
    no_rationale = _base_record("coordinated-new-genesis")
    del no_rationale["campaign"]["lockstep_window_rationale"]
    failures += _case("missing-window-rationale", no_rationale, requirements, False)
    mismatch = _base_record("coordinated-new-genesis")
    mismatch["campaign"]["lockstep_window_seconds"] = 31
    failures += _case("declared-window-mismatch", mismatch, requirements, False)
    single = _base_record("coordinated-new-genesis")
    single["validators"] = [single["validators"][0]]
    single["campaign"]["validator_count"] = 1
    failures += _case("single-validator", single, requirements, False)
    bad_signed = _base_record("coordinated-new-genesis")
    bad_signed["independent_review"]["signed_at"] = "yes"
    failures += _case("unparseable-signed-at", bad_signed, requirements, False)
    substring = _base_record("coordinated-new-genesis")
    substring["release_owner"]["name"] = "@alice-smith"
    substring["independent_review"]["reviewer"] = "@alice"
    failures += _case("reviewer-substring-of-owner", substring, requirements, False)
    no_review_record = _base_record("coordinated-new-genesis")
    del no_review_record["independent_review"]["review_record"]
    failures += _case("missing-review-record", no_review_record, requirements, False)
    no_effect = _base_record("coordinated-new-genesis")
    del no_effect["independent_review"]["category_a_effect"]
    failures += _case("missing-category-a-effect", no_effect, requirements, False)
    no_reconstruction_record = _base_record("retained-history-sweep")
    no_reconstruction_record["incident_trace"] = dict(
        reconstructed["incident_trace"], reconstruction_record="")
    failures += _case("reconstructed-without-record", no_reconstruction_record, requirements, False)
    missing_reviewer = _base_record("coordinated-new-genesis")
    del missing_reviewer["independent_review"]["reviewer"]
    failures += _case("missing-reviewer", missing_reviewer, requirements, False)
    both = _base_record("coordinated-new-genesis")
    both["sweep"] = {"complete": True}
    failures += _case("both-shapes", both, requirements, False)
    missing_digest = _base_record("coordinated-new-genesis")
    missing_digest["campaign"]["digest"] = ""
    failures += _case("missing-digest", missing_digest, requirements, False)
    no_owner = _base_record("retained-history-sweep")
    no_owner["incident_trace"] = {"mode": "reconstructed", "sha256": "c" * 64,
                                  "provenance_url": "https://example.invalid/r",
                                  "owner_decision_url": ""}
    failures += _case("reconstructed-without-owner", no_owner, requirements, False)
    no_reviewer_accept = _base_record("coordinated-new-genesis")
    no_reviewer_accept["independent_review"]["reconstruction_accepted"] = False
    no_reviewer_accept["incident_trace"] = {"mode": "reconstructed", "sha256": "c" * 64,
                                            "provenance_url": "https://example.invalid/r",
                                            "owner_decision_url": "https://example.invalid/o"}
    failures += _case("reconstructed-without-reviewer", no_reviewer_accept, requirements, False)
    # The hard branch: a reconstructed trace is a PRECONDITION, not a PASS.
    hard = _base_record("retained-history-sweep")
    hard["incident_trace"] = reconstructed["incident_trace"]
    result = check_record(hard, requirements, require_original_trace=True)["result"]
    if result != "PRECONDITION":
        failures.append(f"require-original-trace: expected PRECONDITION, got {result}")
    # A table missing a core requirement is refused before any record is read.
    with tempfile.TemporaryDirectory() as temp:
        broken = Path(temp) / "broken.tsv"
        broken.write_text("shape\trequirements\n" + "\n".join(
            f"{shape}\t{','.join(r for r in reqs if r != 'reviewerIndependent')}"
            for shape, reqs in requirements.items()) + "\n")
        try:
            load_table(broken)
        except SystemExit:
            pass
        else:
            failures.append("mutant table: a missing core requirement was accepted")
    if failures:
        for failure in failures:
            print(f"c357-acceptance self-test: FAIL {failure}", file=sys.stderr)
        return 1
    print("c357-acceptance self-test: ok")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    init_p = sub.add_parser("init")
    init_p.add_argument("--out", required=True)
    init_p.add_argument("--shape", choices=SHAPES)
    check_p = sub.add_parser("check")
    check_p.add_argument("--record", required=True)
    check_p.add_argument("--table", default=str(DEFAULT_TABLE))
    check_p.add_argument("--assert-closeable", action="store_true")
    check_p.add_argument("--require-original-trace", action="store_true")
    check_p.add_argument("--report")
    sub.add_parser("self-test")
    args = parser.parse_args()
    if args.command == "init":
        return init_record(Path(args.out), args.shape)
    if args.command == "self-test":
        return self_test()
    requirements = load_table(Path(args.table))
    record = json.loads(Path(args.record).read_text())
    report = check_record(record, requirements, args.require_original_trace)
    if args.report:
        Path(args.report).write_text(json.dumps(report, indent=1, sort_keys=True) + "\n")
    print(f"c357-acceptance: {report['result']}")
    if args.assert_closeable and report["result"] != "PASS":
        return 3 if report["result"] == "PRECONDITION" else 1
    return {"PASS": 0, "FAIL": 1, "PRECONDITION": 3}[report["result"]]


if __name__ == "__main__":
    sys.exit(main())
