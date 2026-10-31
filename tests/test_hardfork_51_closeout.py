"""Negative controls for the Issue #51 hard-fork closeout evidence gate."""
import copy
import importlib.util
import json
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


class CloseoutTests(unittest.TestCase):
    def setUp(self):
        self.payload = copy.deepcopy(LEDGER)

    def inspect(self, verify_git=False):
        return gate.validate(self.payload, verify_git=verify_git, repo=ROOT)

    def test_current_tracker_is_not_closeable(self):
        out = self.inspect()
        self.assertEqual(out["result"], "TRACKER_MUST_REMAIN_OPEN")
        self.assertFalse(out["closure_requested"])
        self.assertGreater(out["unresolved"], 0)
        self.assertEqual(out["merged"], 13)

    def test_jim_20261021_deferrals_are_source_attributed(self):
        out = self.inspect()
        self.assertEqual(out["merged"], 13)
        self.assertEqual(out["deferred"], 12)
        for item in self.payload["items"]:
            if item["status"] == "deferred":
                self.assertIn(item["decision_owner"], ("@PatrickMockridge","@aixaria0"))
                self.assertEqual(item["approval_url"],
                  "https://github.com/rhovision-org/rholo/issues/51#issuecomment-6317072399")

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
        # Reattach the identities; §6 classification cannot be deferred.
        for item, ident in zip(self.payload["items"], sorted(gate.REQUIRED)):
            item["id"] = ident
            if ident == gate.AUDIT_SIGNOFF_ID:
                item["status"] = "verified"
                item["evidence_url"] = "https://github.com/rhovision-org/rholo/issues/51"
                item["approval_url"] = "https://github.com/rhovision-org/rholo/issues/51"
                item["approved_by"] = "Maintainer"
                item["evidence_sha256"] = "a" * 64
        self.payload["closure_requested"] = True
        self.assertEqual(self.inspect()["result"], "TRACKER_MUST_REMAIN_OPEN")
        with patch.object(gate, "is_ancestor", return_value=True):
            self.assertEqual(self.inspect(verify_git=True)["result"],
                             "CLOSEOUT_ELIGIBLE_FOR_INDEPENDENT_REVIEW")

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


if __name__ == "__main__":
    unittest.main()
