"""Negative controls for the per-candidate section-6 dispositions."""
import copy
import importlib.util
import json
import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "tools/check-section6-dispositions.py"
SPEC = importlib.util.spec_from_file_location("section6_dispositions", SCRIPT)
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)
INVENTORY = json.loads((ROOT / "spec/audit/section6-gap-inventory.json").read_text())
DOCUMENT = json.loads((ROOT / "spec/audit/section6-dispositions.json").read_text())


class DispositionTests(unittest.TestCase):
    def setUp(self):
        self.payload = copy.deepcopy(DOCUMENT)

    def row(self, ident):
        return next(row for row in self.payload["rows"] if row["id"] == ident)

    def inspect(self):
        return gate.validate(INVENTORY, self.payload)

    def test_current_dispositions_are_complete_and_open(self):
        out = self.inspect()
        self.assertEqual(out["candidates"], 47)
        self.assertEqual(out["reviewable"], 46)
        self.assertEqual(out["withdrawn"], 1)
        self.assertEqual(out["result"], "DISPOSITIONS_OPEN")

    def test_missing_candidate_is_refused(self):
        self.payload["rows"].pop()
        with self.assertRaisesRegex(gate.DispositionError, "coverage mismatch"):
            self.inspect()

    def test_stale_source_hash_is_refused(self):
        self.row("S6:f735095c746556f3")["row_sha256"] = "0" * 64
        with self.assertRaisesRegex(gate.DispositionError, "stale"):
            self.inspect()

    def test_accepted_needs_a_named_reviewer(self):
        row = self.row("S6:f735095c746556f3")
        row.update(verdict="accepted", reviewer=None,
                   reviewer_independent=True,
                   review_url="https://github.com/rhovision-org/rholo/issues/1",
                   evidence_sha256="a" * 64,
                   category_a_effect="replay-verdict",
                   category_a_derivation="a replay verdict moves")
        with self.assertRaisesRegex(gate.DispositionError, "named reviewer"):
            self.inspect()

    def test_accepted_needs_independence(self):
        row = self.row("S6:f735095c746556f3")
        row.update(verdict="accepted", reviewer="reviewer",
                   reviewer_independent=False,
                   review_url="https://github.com/rhovision-org/rholo/issues/1",
                   evidence_sha256="a" * 64,
                   category_a_effect="replay-verdict",
                   category_a_derivation="a replay verdict moves")
        with self.assertRaisesRegex(gate.DispositionError, "independent reviewer"):
            self.inspect()

    def test_accepted_needs_a_derived_effect(self):
        row = self.row("S6:f735095c746556f3")
        row.update(verdict="accepted", reviewer="reviewer",
                   reviewer_independent=True,
                   review_url="https://github.com/rhovision-org/rholo/issues/1",
                   evidence_sha256="a" * 64)
        with self.assertRaisesRegex(gate.DispositionError, "derived category-A"):
            self.inspect()

    def test_accepted_needs_a_review_record(self):
        row = self.row("S6:f735095c746556f3")
        row.update(verdict="accepted", reviewer="reviewer",
                   reviewer_independent=True,
                   review_url="https://github.com/rhovision-org/rholo/issues/1",
                   evidence_sha256="a" * 64,
                   category_a_effect="replay-verdict",
                   category_a_derivation="a replay verdict moves")
        with self.assertRaisesRegex(gate.DispositionError, "review record"):
            self.inspect()

    def test_withdrawn_needs_a_decision_url(self):
        row = self.row("S6:dc8df48ecb6197ee")
        row["review_url"] = None
        with self.assertRaisesRegex(gate.DispositionError, "decision URL"):
            self.inspect()

    def test_cannot_self_award_signoff(self):
        self.payload["independent_signoff"] = True
        with self.assertRaisesRegex(gate.DispositionError, "self-award"):
            self.inspect()

    def test_assert_closeable_exits_three(self):
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--assert-closeable"],
            capture_output=True, text=True, timeout=60,
        )
        self.assertEqual(result.returncode, 3)
        self.assertIn("SECTION6_SIGNOFF_NOT_CLOSEABLE", result.stdout)


if __name__ == "__main__":
    unittest.main()
