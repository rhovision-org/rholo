"""The deadly-embrace HAZOP gate refuses what it says it refuses.

The study is the map from a stall deviation to the release claim and arm that can measure it. A gate
that only passes on the good document is not a gate, so each arm below mutates one fact the document
asserts and requires the checker to name it: a nonexistent arm, a missing ledger row, a Lean
declaration that does not exist, a debt line that disagrees with the tables, and a guide word that is
not dispositioned.
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

    def test_the_committed_study_passes(self) -> None:
        result = run(self.source)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("ok", result.stdout)

    def test_a_nonexistent_stall_arm_is_refused(self) -> None:
        mutated = self.source.replace(
            "owed: stall-merge-funding", "arm:missing-stall-arm", 1)
        result = run(mutated)
        self.assertEqual(result.returncode, 1)
        self.assertIn("missing-stall-arm", result.stderr)

    def test_a_missing_ledger_row_is_refused(self) -> None:
        line = next(l for l in self.source.splitlines()
                    if l.startswith("| RC-6 | availability |"))
        result = run(self.source.replace(line + "\n", "", 1))
        self.assertEqual(result.returncode, 1)
        self.assertIn("RC-6/availability has no row", result.stderr)

    def test_a_missing_lean_declaration_is_refused(self) -> None:
        mutated = self.source.replace(
            "Law 131 (owed)", "Rchain.not_a_declaration", 1)
        result = run(mutated)
        self.assertEqual(result.returncode, 1)
        self.assertIn("Rchain.not_a_declaration", result.stderr)

    def test_a_debt_line_that_disagrees_is_refused(self) -> None:
        mutated = self.source.replace(
            "stall rows owed: 19", "stall rows owed: 18", 1)
        result = run(mutated)
        self.assertEqual(result.returncode, 1)
        self.assertIn("debt line says 18", result.stderr)

    def test_a_guide_word_with_no_disposition_is_refused(self) -> None:
        mutated = self.source.replace(
            "NO, MORE, LESS", "MORE, LESS", 1)
        result = run(mutated)
        self.assertEqual(result.returncode, 1)
        self.assertIn("guide word NO disposed 0 times", result.stderr)


if __name__ == "__main__":
    unittest.main()
