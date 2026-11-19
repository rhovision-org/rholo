"""The C343 live tool's own checks.

The tool asserts; this test runs its self-test, which covers the five named diagnoses, the
unmodelled fallback, the processor-row mutant (removing the row must stop the processor case being
named), and the ready/reasons contradiction. A fixture that cannot fail is not evidence.
"""
from __future__ import annotations

import importlib.util
import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


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

    def test_the_collector_emits_the_criterion_shape_with_its_locality(self) -> None:
        module = _load("c343_live", ROOT / "tools/c343-live.py")
        document = module.observation_document(
            "live-host", {"ready": True}, {"finalityStalled": False},
            "block 6 accepted",
        )
        self.assertEqual(document["gate_id"], "C343")
        self.assertEqual(document["locality"], "live-host")
        self.assertIsInstance(document["health"], dict)
        self.assertIsInstance(document["operator"], dict)
        self.assertEqual(document["block6"], "block 6 accepted")


if __name__ == "__main__":
    unittest.main()
