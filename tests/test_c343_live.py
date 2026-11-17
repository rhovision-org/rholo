"""The C343 live tool's own checks.

The tool asserts; this test runs its self-test, which covers the five named diagnoses, the
unmodelled fallback, the processor-row mutant (removing the row must stop the processor case being
named), and the ready/reasons contradiction. A fixture that cannot fail is not evidence.
"""
from __future__ import annotations

import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class C343LiveToolTest(unittest.TestCase):
    def test_self_test(self) -> None:
        result = subprocess.run(
            [sys.executable, str(ROOT / "tools/c343-live.py"), "self-test"],
            capture_output=True,
            text=True,
            timeout=120,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("self-test: ok", result.stdout)


if __name__ == "__main__":
    unittest.main()
