"""The C357 acceptance checker's own checks.

The self-test covers both valid shapes, the reconstructed trace with both acceptances, the mixed-
version refusal, the missing-reviewer/both-shapes/missing-digest refusals, the reconstructed-trace
arms, the `--require-original-trace` precondition, and a table that loses a core requirement.
"""
from __future__ import annotations

import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class C357AcceptanceTest(unittest.TestCase):
    def test_self_test(self) -> None:
        result = subprocess.run(
            [sys.executable, str(ROOT / "tools/c357-acceptance.py"), "self-test"],
            capture_output=True,
            text=True,
            timeout=120,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("self-test: ok", result.stdout)


if __name__ == "__main__":
    unittest.main()
