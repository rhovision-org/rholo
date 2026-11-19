"""The 12 host/live obligations are release campaigns, not gates.

A gate is a boolean test of the current build; these need hosts, a release
candidate or an operator window. The checker keeps them blocking, validates
their executable criteria, and refuses to let one appear as a gate.
"""
import copy
import importlib.util
import json
import subprocess
import sys
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
CHECKER_SPEC = importlib.util.spec_from_file_location(
    "release_campaigns", ROOT / "tools/check-release-campaigns.py")
checker = importlib.util.module_from_spec(CHECKER_SPEC)
CHECKER_SPEC.loader.exec_module(checker)
TRACKER = json.loads((ROOT / "spec/audit/release-campaigns.json").read_text())
REGISTER = json.loads((ROOT / "spec/audit/mainnet-gates.json").read_text())


class CampaignTests(unittest.TestCase):
    def setUp(self):
        self.payload = copy.deepcopy(TRACKER)

    def campaign(self, ident="G428-M1"):
        return next(row for row in self.payload["campaigns"] if row["id"] == ident)

    def inspect(self):
        return checker.validate(self.payload)

    def test_the_tracker_validates_and_every_campaign_is_open(self):
        out = self.inspect()
        self.assertEqual(out["result"], "RELEASE_CAMPAIGNS_OPEN")
        self.assertEqual(out["campaigns"], 12)
        self.assertEqual(out["open"], 12)
        self.assertEqual(len(out["open_ids"]), 12)

    def test_the_tracker_is_current(self):
        result = subprocess.run(
            [sys.executable, str(ROOT / "tools/check-release-campaigns.py"), "--check"],
            capture_output=True, text=True, timeout=300,
            cwd=ROOT,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("RELEASE_CAMPAIGNS_OPEN", result.stdout)

    def test_assert_ready_exits_three(self):
        result = subprocess.run(
            [sys.executable, str(ROOT / "tools/check-release-campaigns.py"),
             "--assert-ready"],
            capture_output=True, text=True, timeout=300, cwd=ROOT,
        )
        self.assertEqual(result.returncode, 3)
        self.assertIn("RELEASE_CAMPAIGNS_OPEN", result.stdout)

    def test_no_campaign_is_in_the_gate_register(self):
        register_ids = {row["id"] for row in REGISTER["gates"]}
        self.assertFalse(register_ids & {row["id"] for row in TRACKER["campaigns"]})

    def test_a_campaign_cannot_be_gated_back_in(self):
        self.campaign()["gate"] = True
        with self.assertRaisesRegex(checker.CampaignError, "gate: false"):
            self.inspect()

    def test_the_campaign_floor_holds(self):
        self.assertEqual(checker.CAMPAIGN_MINIMUM, 12)
        self.assertEqual(len(self.payload["campaigns"]), 12)

    def test_a_removed_campaign_is_refused_by_the_floor(self):
        self.payload["campaigns"].pop()
        with self.assertRaisesRegex(checker.CampaignError, "floor"):
            self.inspect()

    def test_dependency_cycle_is_refused(self):
        self.campaign("G428-M1")["depends_on"] = ["G428-M2"]
        self.campaign("G428-M2")["depends_on"] = ["G428-M1"]
        with self.assertRaisesRegex(checker.CampaignError, "cycle"):
            self.inspect()

    def test_cross_family_related_needs_a_direction(self):
        edge = next(edge for edge in self.campaign("G428-M2")["related"]
                    if edge["id"] == "TA-C1")
        edge["criterion"] = "M2 is the ingress campaign"
        with self.assertRaisesRegex(checker.CampaignError, "direction"):
            self.inspect()

    def test_related_edges_must_be_symmetric(self):
        target = self.campaign("G584-G1")
        target["related"] = [edge for edge in target["related"]
                             if edge["id"] != "G428-M1"]
        with self.assertRaisesRegex(checker.CampaignError, "not symmetric"):
            self.inspect()

    def test_source_phrase_must_be_in_the_excerpt(self):
        self.campaign()["source_phrase"] = "a tautology"
        with self.assertRaisesRegex(checker.CampaignError, "source phrase"):
            self.inspect()

    def test_criteria_hash_must_match_the_campaign_text(self):
        self.campaign()["pass_condition"] = "Trivially met"
        with self.assertRaisesRegex(checker.CampaignError, "criteria hash"):
            self.inspect()

    def test_a_stale_source_excerpt_is_refused(self):
        self.payload["sources"]["G428"]["excerpt_sha256"] = "0" * 64
        with self.assertRaisesRegex(checker.CampaignError, "excerpt hash"):
            self.inspect()

    def test_the_checker_runs_the_criteria_self_test(self):
        with mock.patch.object(
                checker, "_selftest",
                return_value=["boom: a vacuous control"]):
            with self.assertRaisesRegex(checker.CampaignError, "criteria self-test"):
                self.inspect()


if __name__ == "__main__":
    unittest.main()
