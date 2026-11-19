"""Negative controls for the Issue #51 hard-fork closeout evidence gate."""
import copy
import hashlib
import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "tools/check-hardfork-51-closeout.py"
SPEC = importlib.util.spec_from_file_location("hardfork_51", SCRIPT)
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)
LEDGER = json.loads((ROOT / "spec/audit/hardfork-51-closeout.json").read_text())
ARTIFACT = "spec/audit/hardfork-51-closeout.json"


def artifact_sha() -> str:
    return hashlib.sha256((ROOT / ARTIFACT).read_bytes()).hexdigest()


class CloseoutTests(unittest.TestCase):
    def setUp(self):
        self.payload = copy.deepcopy(LEDGER)

    def inspect(self, verify_git=False, require_gates=False):
        return gate.validate(self.payload, verify_git=verify_git, repo=ROOT,
                             require_gates=require_gates)

    def test_current_tracker_is_not_closeable(self):
        out = self.inspect()
        self.assertEqual(out["result"], "TRACKER_MUST_REMAIN_OPEN")
        self.assertFalse(out["closure_requested"])
        self.assertGreater(out["unresolved"], 0)
        self.assertEqual(out["merged"], 13)

    def test_jim_20261021_deferrals_are_source_attributed(self):
        out = self.inspect()
        self.assertEqual(out["merged"], 13)
        # The 2026-10-21 close-clause deferrals, by their own decision URL ...
        legacy = [i for i in self.payload["items"] if i["status"] == "deferred"
                  and i.get("approval_url")
                  == "https://github.com/rhovision-org/rholo/issues/51#issuecomment-6317072399"]
        # ... plus the decimal withdrawal (#527), which carries its own issue as the decision.
        self.assertEqual(out["deferred"], len(legacy) + 1)
        for item in legacy:
            self.assertIn(item["decision_owner"], ("@PatrickMockridge","@aixaria0"))
        withdrawn = next(x for x in self.payload["items"] if x["id"] == "Release:decimal-genesis")
        self.assertEqual(withdrawn["status"], "deferred")
        self.assertEqual(withdrawn["decision_owner"], "@PatrickMockridge")
        self.assertIn("#527", withdrawn["reason"])
        self.assertEqual(withdrawn["approval_url"],
                         "https://github.com/rhovision-org/rholo/issues/527")

    def test_merged_allocation_track_does_not_resolve_economics_deferral(self):
        self.assertEqual(next(x for x in self.payload["items"] if x["id"]=="A:rev-economics")["status"], "deferred")
        self.assertEqual(next(x for x in self.payload["items"] if x["id"]=="Decision:developer-allocation")["status"], "merged")
        self.assertEqual(next(x for x in self.payload["items"] if x["id"]=="Audit:inactivity-leak")["decision_owner"], "@aixaria0")

    def test_exact_inventory_is_required(self):
        self.payload["items"].pop()
        with self.assertRaisesRegex(gate.LedgerError, "inventory mismatch"):
            self.inspect()

    def test_unknown_entry_is_not_silently_accepted(self):
        fake = copy.deepcopy(self.payload["items"][0])
        fake["id"] = "A:unreviewed-forgery"
        self.payload["items"].append(fake)
        with self.assertRaisesRegex(gate.LedgerError, "inventory mismatch"):
            self.inspect()

    def test_duplicate_record_is_rejected(self):
        self.payload["items"].append(copy.deepcopy(self.payload["items"][0]))
        with self.assertRaisesRegex(gate.LedgerError, "duplicate item ID"):
            self.inspect()

    def test_bad_merged_commit_is_rejected(self):
        record = self.payload["items"][0]
        self.assertEqual(record["status"], "merged")
        record["evidence"]["merge_commit"] = "tbd"
        with self.assertRaisesRegex(gate.LedgerError, "merged commit missing"):
            self.inspect()

    def test_open_item_cannot_claim_a_closed_tracker(self):
        self.payload["closure_requested"] = True
        self.assertEqual(self.inspect()["result"], "TRACKER_MUST_REMAIN_OPEN")

    def test_without_status_accountability_open_item_is_invalid(self):
        record = next(x for x in self.payload["items"] if x["status"] == "open")
        record.pop("decision_owner")
        with self.assertRaisesRegex(gate.LedgerError, "decision_owner"):
            self.inspect()

    def test_named_owner_and_reason_are_sufficient_without_release(self):
        item = next(x for x in self.payload["items"] if x["status"] == "open")
        item.update(status="deferred", decision_owner="@jimscarver",
                    reason="Defer the economics rule pending an accepted governance proposal.")
        self.assertEqual(self.inspect()["deferred"], sum(x["status"] == "deferred" for x in LEDGER["items"]) + 1)
        self.assertEqual(self.inspect()["result"], "TRACKER_MUST_REMAIN_OPEN")

    def test_generic_role_is_not_a_named_decision_owner(self):
        item = next(x for x in self.payload["items"] if x["status"] == "open")
        item.update(status="deferred", decision_owner="Jim / governance",
                    reason="Defer while the governance policy is reviewed.")
        with self.assertRaisesRegex(gate.LedgerError, "GitHub user"):
            self.inspect()

    def test_missing_or_placeholder_reason_is_rejected(self):
        item = next(x for x in self.payload["items"] if x["status"] == "open")
        item.update(status="deferred", decision_owner="@jimscarver")
        with self.assertRaisesRegex(gate.LedgerError, "reason"):
            self.inspect()
        item["reason"] = "TBD"
        with self.assertRaisesRegex(gate.LedgerError, "reason"):
            self.inspect()
        item["reason"] = "Defer to some future change."
        with self.assertRaisesRegex(gate.LedgerError, "placeholder"):
            self.inspect()

    def test_multiline_reason_is_rejected(self):
        item = next(x for x in self.payload["items"] if x["status"] == "open")
        for separator in ("\n", "\r", "\r\n", "\u0085", "\u2028", "\u2029", r"\n", r"\r"):
            with self.subTest(separator=repr(separator)):
                item.update(status="deferred", decision_owner="@jimscarver",
                            reason="Defer this release." + separator + "Another decision.")
                with self.assertRaisesRegex(gate.LedgerError, "one specific"):
                    self.inspect()

    def test_deferral_needs_one_well_formed_account(self):
        item = next(x for x in self.payload["items"] if x["status"] == "open")
        item.update(status="deferred", reason="Retain the current rule pending governance review.")
        for owner in ("@jimscarver/@aixaria0", "@team/release", "@trailing-", "@double--dash", "@" + "a" * 40):
            with self.subTest(owner=owner):
                item["decision_owner"] = owner
                with self.assertRaisesRegex(gate.LedgerError, "GitHub user"):
                    self.inspect()

    def test_legacy_release_fields_cannot_replace_reason(self):
        item = next(x for x in self.payload["items"] if x["status"] == "open")
        item.update(status="deferred", decision_owner="@jimscarver",
                    named_release="RChain protocol release 2.4", approved_by="Maintainer",
                    risk_accepted="Legacy approval metadata does not supply a reason.")
        with self.assertRaisesRegex(gate.LedgerError, "reason"):
            self.inspect()

    def test_optional_decision_link_must_be_valid_if_present(self):
        item = next(x for x in self.payload["items"] if x["status"] == "open")
        item.update(status="deferred", decision_owner="@jimscarver",
                    reason="Retain the current rule until a replacement is accepted.",
                    approval_url="not a link")
        with self.assertRaisesRegex(gate.LedgerError, "GitHub evidence URL"):
            self.inspect()

    def test_fake_history_is_rejected_when_provenance_check_requested(self):
        with patch.object(gate, "is_ancestor", return_value=False):
            with self.assertRaisesRegex(gate.LedgerError, "merge commit is not an ancestor"):
                self.inspect(verify_git=True)

    def test_consistent_history_does_not_override_open_decisions(self):
        with patch.object(gate, "is_ancestor", return_value=True):
            result = self.inspect(verify_git=True)
        self.assertTrue(result["verified_local_git_ancestry"])
        self.assertEqual(result["result"], "TRACKER_MUST_REMAIN_OPEN")

    def test_eligibility_is_not_automatic_merge_or_release(self):
        # Synthetic fixture only; no real approvals are represented.
        for item in self.payload["items"]:
            item.clear()
            item.update(id="placeholder",title="Placeholder",status="deferred",
                        decision_owner="@jimscarver",
                        reason="Delay until a separately approved first-release fork design.")
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            record = root / "evidence"
            record.mkdir()
            (record / "manifest.json").write_text("{}\n")
            digest = hashlib.sha256((record / "manifest.json").read_bytes()).hexdigest()
            (root / "review.md").write_text(f"Maintainer verified {digest}\n")
            # Reattach the identities; §6 classification cannot be deferred.
            for item, ident in zip(self.payload["items"], sorted(gate.REQUIRED)):
                item["id"] = ident
                if ident == gate.AUDIT_SIGNOFF_ID:
                    item["status"] = "verified"
                    item["evidence_url"] = "https://github.com/rhovision-org/rholo/issues/51"
                    item["approval_url"] = "https://github.com/rhovision-org/rholo/issues/51"
                    item["approved_by"] = "Maintainer"
                    item["evidence_sha256"] = digest
                    item["evidence_record"] = "evidence"
                    item["review_record"] = "review.md"
            self.payload["closure_requested"] = True
            self.assertEqual(gate.validate(self.payload, repo=root)["result"],
                             "TRACKER_MUST_REMAIN_OPEN")
            with patch.object(gate, "is_ancestor", return_value=True):
                self.assertEqual(
                    gate.validate(self.payload, repo=root, verify_git=True)["result"],
                    "CLOSEOUT_ELIGIBLE_FOR_INDEPENDENT_REVIEW")

    def test_the_ledger_is_the_hf51_tracker(self):
        bound, unresolved, note = gate.gate_binding(ROOT)
        self.assertTrue(bound)
        self.assertEqual(unresolved, [])
        self.assertIn("not the gate register", note)
        result = self.inspect(require_gates=True)
        self.assertTrue(result["gates_bound"])
        self.assertEqual(result["gates_unresolved"], [])
        self.assertEqual(result["result"], "TRACKER_MUST_REMAIN_OPEN")

    def test_missing_gate_register_is_unbound_not_invalid(self):
        with tempfile.TemporaryDirectory() as tmp:
            result = gate.validate(self.payload, repo=Path(tmp), require_gates=True)
        self.assertFalse(result["gates_bound"])
        self.assertIn("unavailable", result["gates_error"])

    def test_cannot_defer_the_exhaustive_audit_itself(self):
        item = next(x for x in self.payload["items"] if x["id"] == gate.AUDIT_SIGNOFF_ID)
        item.update(status="deferred", decision_owner="@jimscarver",
                    reason="The audit inventory would be deferred without verification.")
        with self.assertRaisesRegex(gate.LedgerError, "inventory cannot be deferred"):
            self.inspect()

    def test_verified_cannot_bypass_land_or_defer(self):
        item = next(x for x in self.payload["items"] if x["status"] == "open")
        item.update(status="verified",
                    evidence_url="https://github.com/rhovision-org/rholo/issues/51",
                    approval_url="https://github.com/rhovision-org/rholo/issues/51",
                    approved_by="Maintainer", evidence_sha256="a" * 64)
        with self.assertRaisesRegex(gate.LedgerError, "cannot replace"):
            self.inspect()

    def test_audit_review_requires_digested_evidence(self):
        item = next(x for x in self.payload["items"] if x["id"] == gate.AUDIT_SIGNOFF_ID)
        item.update(status="verified",
                    evidence_url="https://github.com/rhovision-org/rholo/issues/51",
                    approval_url="https://github.com/rhovision-org/rholo/issues/51",
                    approved_by="Maintainer",
                    evidence_sha256="tbd")
        with self.assertRaisesRegex(gate.LedgerError, "evidence SHA-256 required"):
            self.inspect()

    def test_cli_close_request_fails_until_every_row_resolved(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "ledger.json"
            path.write_text(json.dumps(self.payload))
            with patch("sys.argv", ["check-hardfork-51-closeout.py", str(path),
                                    "--assert-closeable"]), redirect_stdout(StringIO()):
                self.assertEqual(gate.main(), 3)

    def test_duplicate_top_level_json_field_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "duplicated.json"
            raw = json.dumps(self.payload)
            path.write_text(raw[:-1] + ',"schema":1}')
            output = StringIO()
            with patch("sys.argv", ["check-hardfork-51-closeout.py", str(path)]), redirect_stdout(output):
                self.assertEqual(gate.main(), 2)
            self.assertIn("duplicate JSON key", output.getvalue())

    def test_closure_requested_must_be_boolean(self):
        self.payload["closure_requested"] = "true"
        with self.assertRaisesRegex(gate.LedgerError, "explicit boolean"):
            self.inspect()

    def test_acceptance_record_must_name_an_artifact_and_digests(self):
        record = {"gate_id": "n8-partition-rejoin", "source_sha": "a" * 40,
                  "binary_sha256": "b" * 64, "aggregate": "PASS",
                  "artifact": ARTIFACT, "artifact_sha256": artifact_sha()}
        self.payload["acceptance_records"] = [record]
        self.assertEqual(self.inspect()["result"], "TRACKER_MUST_REMAIN_OPEN")
        for field, value in (("source_sha", "tbd"), ("binary_sha256", "tbd"),
                             ("aggregate", "MAYBE"), ("artifact", "")):
            with self.subTest(field=field):
                bad = dict(record)
                bad[field] = value
                self.payload["acceptance_records"] = [bad]
                with self.assertRaises(gate.LedgerError):
                    self.inspect()

    def test_an_acceptance_record_binds_to_the_artifact_bytes(self):
        record = {"gate_id": "n8-partition-rejoin", "source_sha": "a" * 40,
                  "binary_sha256": "b" * 64, "aggregate": "PASS",
                  "artifact": ARTIFACT, "artifact_sha256": artifact_sha()}
        for field, value in (
                ("artifact", "https://github.com/rhovision-org/rholo/issues/51"),
                ("artifact", "/etc/passwd"),
                ("artifact", "../outside.json"),
                ("artifact", "spec/audit/no-such-record.json"),
                ("artifact_sha256", "tbd"),
                ("artifact_sha256", "a" * 64)):
            with self.subTest(field=field, value=value):
                bad = dict(record)
                bad[field] = value
                self.payload["acceptance_records"] = [bad]
                with self.assertRaises(gate.LedgerError):
                    self.inspect()

    def test_a_symlinked_artifact_is_not_a_pinned_artifact(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            real = root / "real.json"
            real.write_text("{}\n")
            (root / "link.json").symlink_to(real)
            record = {"gate_id": "n8-partition-rejoin", "source_sha": "a" * 40,
                      "binary_sha256": "b" * 64, "aggregate": "PASS",
                      "artifact": "link.json",
                      "artifact_sha256": hashlib.sha256(real.read_bytes()).hexdigest()}
            self.payload["acceptance_records"] = [record]
            with self.assertRaisesRegex(gate.LedgerError, "symlink"):
                gate.validate(self.payload, repo=root)

    def test_an_evidence_record_artifact_must_agree_with_the_acceptance_record(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            record_dir = root / "record"
            record_dir.mkdir()
            manifest = record_dir / "manifest.json"
            manifest.write_text(json.dumps({"source_sha": "a" * 40,
                                            "binary_sha256": "b" * 64,
                                            "verdict": "PASS"}))
            record = {"gate_id": "n8-partition-rejoin", "source_sha": "a" * 40,
                      "binary_sha256": "b" * 64, "aggregate": "PASS",
                      "artifact": "record",
                      "artifact_sha256": hashlib.sha256(manifest.read_bytes()).hexdigest()}
            self.payload["acceptance_records"] = [record]
            self.assertEqual(gate.validate(self.payload, repo=root)["result"],
                             "TRACKER_MUST_REMAIN_OPEN")
            for field, value in (("source_sha", "c" * 40),
                                 ("binary_sha256", "c" * 64),
                                 ("verdict", "INCONCLUSIVE")):
                with self.subTest(field=field):
                    manifest.write_text(json.dumps(
                        {"source_sha": "a" * 40, "binary_sha256": "b" * 64,
                         "verdict": "PASS", field: value}))
                    bad = dict(record)
                    bad["artifact_sha256"] = hashlib.sha256(
                        manifest.read_bytes()).hexdigest()
                    self.payload["acceptance_records"] = [bad]
                    with self.assertRaisesRegex(gate.LedgerError, "manifest"):
                        gate.validate(self.payload, repo=root)
            # A manifest that omits a field is not agreement.
            manifest.write_text(json.dumps({"source_sha": "a" * 40,
                                            "binary_sha256": "b" * 64}))
            bad = dict(record)
            bad["artifact_sha256"] = hashlib.sha256(manifest.read_bytes()).hexdigest()
            self.payload["acceptance_records"] = [bad]
            with self.assertRaisesRegex(gate.LedgerError, "verdict"):
                gate.validate(self.payload, repo=root)

    def test_a_pass_and_a_fail_at_one_digest_invalidate_the_gate(self):
        base = {"gate_id": "n8-partition-rejoin", "source_sha": "a" * 40,
                "binary_sha256": "b" * 64, "artifact": ARTIFACT,
                "artifact_sha256": artifact_sha()}
        self.payload["acceptance_records"] = [{**base, "aggregate": "PASS"},
                                              {**base, "aggregate": "FAIL"}]
        with self.assertRaisesRegex(gate.LedgerError, "PASS/FAIL conflict"):
            self.inspect()


    def test_the_sign_off_binds_the_digest_to_an_in_repo_record_and_review(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            record = root / "evidence"
            record.mkdir()
            (record / "manifest.json").write_text("{}\n")
            digest = hashlib.sha256((record / "manifest.json").read_bytes()).hexdigest()
            (root / "review.md").write_text(f"Maintainer verified {digest}\n")
            review_dir = root / "review-dir"
            review_dir.mkdir()
            (review_dir / "README.md").write_text(f"Maintainer verified {digest}\n")
            item = next(x for x in self.payload["items"] if x["id"] == gate.AUDIT_SIGNOFF_ID)
            item.update(status="verified",
                        evidence_url="https://github.com/rhovision-org/rholo/issues/51",
                        approval_url="https://github.com/rhovision-org/rholo/issues/51",
                        approved_by="Maintainer", evidence_sha256=digest,
                        evidence_record="evidence", review_record="review.md")
            self.assertEqual(gate.validate(self.payload, repo=root)["result"],
                             "TRACKER_MUST_REMAIN_OPEN")
            # The directory form is the register's human-attestation convention.
            item["review_record"] = "review-dir"
            self.assertEqual(gate.validate(self.payload, repo=root)["result"],
                             "TRACKER_MUST_REMAIN_OPEN")
            item["review_record"] = "review.md"
            original = dict(item)
            for field, value in (("evidence_sha256", "a" * 64),
                                 ("evidence_record", "missing"),
                                 ("evidence_record", "review.md"),
                                 ("evidence_record", "../evidence"),
                                 ("review_record", "missing.md"),
                                 ("approved_by", "SomeoneElse")):
                with self.subTest(field=field):
                    item.clear()
                    item.update(original)
                    item[field] = value
                    with self.assertRaises(gate.LedgerError):
                        gate.validate(self.payload, repo=root)
            for field in ("evidence_record", "review_record"):
                with self.subTest(absent=field):
                    item.clear()
                    item.update(original)
                    del item[field]
                    with self.assertRaises(gate.LedgerError):
                        gate.validate(self.payload, repo=root)
            item.clear()
            item.update(original)
            (root / "review.md").write_text("Maintainer verified\n")
            with self.assertRaisesRegex(gate.LedgerError, "digest"):
                gate.validate(self.payload, repo=root)


class MainnetRegisterTests(unittest.TestCase):
    """The wired path for the mainnet gate register.

    This file's discovery line is in `ci.yml`; `check-mainnet-gates.py --check` rebuilds the
    register and verifies every `met` row's witness/control/evidence/pins. The register holds
    only current-build gates; the 12 host/live obligations are release campaigns, judged by
    `check-release-campaigns.py` and reached through `ReleaseCampaignTests` below. Ancestry is
    skipped in a shallow checkout (recorded as `unverified-shallow`), which is why this can run
    in CI without a `fetch-depth` edit.
    """

    def test_the_committed_register_validates_and_is_current(self):
        result = subprocess.run(
            [sys.executable, str(ROOT / "tools/check-mainnet-gates.py"), "--check"],
            capture_output=True, text=True, cwd=ROOT, timeout=600, check=False)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        report = json.loads(result.stdout)
        self.assertEqual(report["result"], "REGISTER_VALID", report)
        self.assertEqual(
            (report["gates"], report["met"], report["unresolved"]), (47, 47, 0), report)


class CapacityTierTests(unittest.TestCase):
    """The collector and the evaluate-path parameter check, reached where CI looks.

    `test_capacity_tier.py` has no discovery line in `ci.yml`; this file does, so the suite runs
    from the same seam `MainnetRegisterTests` uses for the register check. The collector's own
    tests are the contract: a record it writes is evaluated by the real M1 criterion.
    """

    def test_the_collector_suite_passes(self):
        suite = unittest.defaultTestLoader.discover(
            str(ROOT / "tests"), pattern="test_capacity_tier.py",
            top_level_dir=str(ROOT / "tests"))
        result = unittest.TextTestRunner(stream=StringIO(), verbosity=0).run(suite)
        self.assertTrue(result.wasSuccessful(), result.errors + result.failures)

    def test_the_evaluate_path_refuses_a_malformed_parameter(self):
        spec = importlib.util.spec_from_file_location(
            "capacity_tier_principles", ROOT / "tools/gate-principles.py")
        principles = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(principles)
        entry = {"principle": "no_false_settlement",
                 "parameters": {"min_observers": "4"}, "requires": ["record"]}
        self.assertEqual(principles.evaluate_entry(entry, {"record": {}})["status"], "FAIL")


class M2PressureArmTests(unittest.TestCase):
    """The M2 endpoint/locality and gRPC-binding controls, reached where CI looks."""

    def test_the_pressure_suite_passes(self):
        suite = unittest.defaultTestLoader.discover(
            str(ROOT / "tests"), pattern="test_m2_pressure.py",
            top_level_dir=str(ROOT / "tests"))
        result = unittest.TextTestRunner(stream=StringIO(), verbosity=0).run(suite)
        self.assertTrue(result.wasSuccessful(), result.errors + result.failures)


class FinalityCampaignTests(unittest.TestCase):
    """The M4 collector, reached where CI looks.

    `test_m4_campaign.py` has no discovery line in `ci.yml`; this file does, so the suite runs
    from the same seam `CapacityTierTests` uses. The collector's own tests are the contract: the
    record it writes is evaluated by the real M4 criterion.
    """

    def test_the_campaign_suite_passes(self):
        suite = unittest.defaultTestLoader.discover(
            str(ROOT / "tests"), pattern="test_m4_campaign.py",
            top_level_dir=str(ROOT / "tests"))
        result = unittest.TextTestRunner(stream=StringIO(), verbosity=0).run(suite)
        self.assertTrue(result.wasSuccessful(), result.errors + result.failures)


class ReleaseCampaignTests(unittest.TestCase):
    def test_the_campaign_suite_passes(self):
        suite = unittest.defaultTestLoader.discover(
            str(ROOT / "tests"), pattern="test_release_campaigns.py",
            top_level_dir=str(ROOT / "tests"))
        result = unittest.TextTestRunner(stream=StringIO(), verbosity=0).run(suite)
        self.assertTrue(result.wasSuccessful(), result.errors + result.failures)
        self.assertGreater(result.testsRun, 0)


class RustControlTests(unittest.TestCase):
    """The Rust control runner, reached where CI looks.

    `test_rust_controls.py` has no discovery line in `ci.yml`; this file does. The live
    case builds a dependency-free toy crate, so this adds a cargo invocation but not the
    45-witness corpus (that is `--run-controls --controls all`, a release-time run).
    """

    def test_the_control_runner_suite_passes(self):
        suite = unittest.defaultTestLoader.discover(
            str(ROOT / "tests"), pattern="test_rust_controls.py",
            top_level_dir=str(ROOT / "tests"))
        result = unittest.TextTestRunner(stream=StringIO(), verbosity=0).run(suite)
        self.assertTrue(result.wasSuccessful(), result.errors + result.failures)
        self.assertGreater(result.testsRun, 0)


if __name__ == "__main__":
    unittest.main()
