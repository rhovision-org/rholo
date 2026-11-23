"""The deadly-embrace HAZOP gate refuses what it says it refuses.

The study classifies a stall by representability, shape and root cause, and places its measurement in
an existing claim arm without changing the release-claim surface. A gate that only passes on the good
document is not a gate, so each test below mutates one fact the document asserts and requires the
checker to name it: the two surfaces stay separate, the vocabularies stay closed, a claim arm must be
real, and the debt line must tell the truth.
"""
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CHECKER = ROOT / "tools" / "check-stall-coverage.py"
PAGE = ROOT / "docs" / "src" / "spec" / "stall-coverage.md"


def run(page_text: str) -> subprocess.CompletedProcess:
    with tempfile.NamedTemporaryFile("w", suffix=".md", delete=False) as handle:
        handle.write(page_text)
        path = Path(handle.name)
    try:
        return subprocess.run(
            [sys.executable, str(CHECKER), "--page", str(path)],
            capture_output=True, text=True, cwd=ROOT, timeout=120)
    finally:
        path.unlink(missing_ok=True)


class StallCoverageGate(unittest.TestCase):
    def setUp(self) -> None:
        self.source = PAGE.read_text()

    def assert_refused(self, mutated: str, fragment: str) -> None:
        result = run(mutated)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn(fragment, result.stderr)

    def test_the_committed_study_passes(self) -> None:
        result = run(self.source)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("ok", result.stdout)

    def test_a_claim_column_in_table_a_is_refused(self) -> None:
        mutated = self.source.replace("| ID | Guide word |", "| ID | Claim | Guide word |", 1)
        self.assert_refused(mutated, "Table A carries an RC column")

    def test_a_nonexistent_placement_arm_is_refused(self) -> None:
        mutated = self.source.replace(
            "| DE-01 | RC-1 | local | rc-confirm-local |",
            "| DE-01 | RC-1 | missing-stall-arm | rc-confirm-local |", 1)
        self.assert_refused(mutated, "missing-stall-arm")

    def test_a_missing_required_arm_is_refused(self) -> None:
        line = next(l for l in self.source.splitlines()
                    if l.startswith("| DE-09 | RC-6 | availability |"))
        self.assert_refused(self.source.replace(line + "\n", "", 1),
                            "RC-6/availability has no placement row")

    def test_an_unknown_root_cause_is_refused(self) -> None:
        mutated = self.source.replace("| loss-blind-refusal |", "| mystery-cause |", 1)
        self.assert_refused(mutated, "mystery-cause")

    def test_an_unknown_representation_is_refused(self) -> None:
        mutated = self.source.replace("| protocol-wait |", "| network-partition |", 1)
        self.assert_refused(mutated, "network-partition")

    def test_a_missing_lean_declaration_is_refused(self) -> None:
        mutated = self.source.replace(
            "Rchain.MergeScope.scope_is_a_function_of_the_dag", "Rchain.not_a_declaration", 1)
        self.assert_refused(mutated, "Rchain.not_a_declaration")

    def test_a_withdrawn_law_is_refused(self) -> None:
        self.assert_refused(self.source + "\nLaw 129 is owed.\n", "withdrawn Law 129")

    def test_a_live_placement_without_evidence_is_refused(self) -> None:
        mutated = self.source.replace("| owed | - |", "| live | - |", 1)
        self.assert_refused(mutated, "live without a spec/audit/evidence path")

    def test_a_claim_reading_on_the_hazop_surface_is_refused(self) -> None:
        mutated = self.source.replace("| owed | - |", "| safe | - |", 1)
        self.assert_refused(mutated, "RC reading 'safe'")

    def test_a_debt_line_that_disagrees_is_refused(self) -> None:
        mutated = self.source.replace("placement rows owed: 25", "placement rows owed: 24", 1)
        self.assert_refused(mutated, "debt line says")

    def test_a_guide_word_with_no_disposition_is_refused(self) -> None:
        mutated = self.source.replace("NO, MORE, LESS", "MORE, LESS", 1)
        self.assert_refused(mutated, "guide word NO disposed 0 times")


if __name__ == "__main__":
    unittest.main()
