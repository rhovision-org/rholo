"""The release claims are observable behaviours, not gates and not campaigns.

A gate is a boolean test of the current build. A release claim is one observable
node behaviour with one falsifier and one control, re-runnable on the standard
local devnet or on public testnet-4. The 12 old campaign rows were deleted; their
fate is mapped in `release-claims.json`'s `superseded` table.
"""
import copy
import importlib.util
import json
import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CHECKER_SPEC = importlib.util.spec_from_file_location(
    "release_claims", ROOT / "tools/check-release-campaigns.py")
checker = importlib.util.module_from_spec(CHECKER_SPEC)
CHECKER_SPEC.loader.exec_module(checker)
SOURCE = json.loads((ROOT / "spec/audit/release-claims.json").read_text())
TRACKER = json.loads((ROOT / "spec/audit/release-campaigns.json").read_text())
REGISTER = json.loads((ROOT / "spec/audit/mainnet-gates.json").read_text())


class FakeBuilder:
    """The generated view of the mutated claims, so `validate` can be unit-tested."""

    def __init__(self, rows):
        self.rows = rows

    def campaigns(self):
        return {"campaigns": self.rows}


class ClaimTests(unittest.TestCase):
    def setUp(self):
        self.payload = copy.deepcopy(SOURCE)

    def claim(self, ident="RC-1"):
        return next(row for row in self.payload["claims"] if row["id"] == ident)

    def inspect(self):
        return checker.validate(self.payload, FakeBuilder(self.payload["claims"]))

    def test_the_tracker_validates_and_no_claim_is_closed(self):
        out = self.inspect()
        self.assertEqual(out["claims"], 7)
        self.assertEqual(out["old"], 12)
        self.assertFalse(any(status == "closed" for status in out["statuses"].values()))

    def test_the_tracker_is_current(self):
        result = subprocess.run(
            [sys.executable, str(ROOT / "tools/check-release-campaigns.py"), "--check"],
            capture_output=True, text=True, timeout=300, cwd=ROOT)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("RELEASE_CLAIMS_OPEN", result.stdout)

    def test_assert_ready_exits_three(self):
        result = subprocess.run(
            [sys.executable, str(ROOT / "tools/check-release-campaigns.py"),
             "--assert-ready"],
            capture_output=True, text=True, timeout=300, cwd=ROOT)
        self.assertEqual(result.returncode, 3)
        self.assertIn("RELEASE_CLAIMS_OPEN", result.stdout)

    def test_no_claim_is_in_the_gate_register(self):
        register_ids = {row["id"] for row in REGISTER["gates"]}
        self.assertFalse(register_ids & {row["id"] for row in TRACKER["campaigns"]})

    def test_every_deleted_row_is_mapped(self):
        mapped = {entry["id"] for entry in self.payload["superseded"]}
        self.assertEqual(mapped, checker.OLD_IDS)
        for row in self.payload["claims"]:
            self.assertTrue(set(row.get("supersedes") or []) <= checker.OLD_IDS)

    def test_an_unmapped_deleted_row_is_refused(self):
        self.payload["superseded"] = [
            entry for entry in self.payload["superseded"] if entry["id"] != "C343"]
        with self.assertRaisesRegex(checker.ClaimError, "every deleted row"):
            self.inspect()

    def test_a_claim_may_not_reuse_a_deleted_id(self):
        self.claim()["id"] = "G428-M1"
        with self.assertRaisesRegex(checker.ClaimError, "deleted campaign id"):
            self.inspect()

    def test_both_environments_are_required(self):
        self.claim()["environment"]["public"] = ""
        with self.assertRaisesRegex(checker.ClaimError, "both environments"):
            self.inspect()

    def test_both_rerun_commands_are_required(self):
        self.claim()["rerun"]["local"] = ""
        with self.assertRaisesRegex(checker.ClaimError, "re-run commands"):
            self.inspect()

    def test_an_owed_observation_must_name_what_is_missing(self):
        sub = next(s for s in self.claim()["sub_observations"] if s["status"] == "owed")
        sub.pop("missing")
        with self.assertRaisesRegex(checker.ClaimError, "missing description"):
            self.inspect()

    def test_an_open_observation_must_name_a_tool(self):
        sub = next(s for s in self.claim()["sub_observations"] if s["status"] == "open")
        sub.pop("tool")
        with self.assertRaisesRegex(checker.ClaimError, "needs a tool"):
            self.inspect()

    def test_the_floor_holds(self):
        self.assertEqual(checker.CLAIM_MINIMUM, 7)
        self.assertEqual(len(self.payload["claims"]), 7)

    def test_a_removed_claim_is_refused(self):
        self.payload["claims"].pop()
        with self.assertRaisesRegex(checker.ClaimError, "floor"):
            self.inspect()

    def test_the_generated_tracker_carries_the_authored_claims(self):
        generated = {row["id"] for row in TRACKER["campaigns"]}
        authored = {row["id"] for row in SOURCE["claims"]}
        self.assertEqual(generated, authored)


if __name__ == "__main__":
    unittest.main()
