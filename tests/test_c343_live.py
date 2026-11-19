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


class HostEvaluationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.module = _load("c343_live_eval", ROOT / "tools/c343-live.py")
        cls.table = cls.module.load_table(cls.module.DEFAULT_TABLE)

    def evaluate(self, health=None, operator=None, block6="block 6 accepted"):
        health = self.module._base_health() if health is None else health
        operator = self.module._base_operator() if operator is None else operator
        return self.module.host_evaluation(health, operator, block6, self.table)

    def test_the_base_host_passes_every_cell(self):
        result = self.evaluate()
        self.assertEqual(result["verdict"], "PASS", result)
        self.assertEqual(result["diagnosis"], "healthy")
        self.assertTrue(all(cell["verdict"] == "PASS"
                            for cell in result["cells"].values()), result)

    def test_a_missing_health_field_is_a_precondition(self):
        health = self.module._base_health()
        health.pop("blockPipeline")
        result = self.evaluate(health=health)
        self.assertEqual(result["verdict"], "PRECONDITION", result)
        self.assertEqual(result["cells"]["health-route"]["verdict"], "PRECONDITION")
        self.assertIn("blockPipeline", result["cells"]["health-route"]["detail"])

    def test_a_missing_operator_field_is_a_precondition(self):
        operator = self.module._base_operator()
        operator.pop("shards")
        result = self.evaluate(operator=operator)
        self.assertEqual(result["verdict"], "PRECONDITION", result)
        self.assertEqual(result["cells"]["operator-route"]["verdict"], "PRECONDITION")

    def test_a_height_gap_contradiction_is_a_failure(self):
        result = self.evaluate(health=self.module._base_health(finalizedLag=2))
        self.assertEqual(result["verdict"], "FAIL", result)
        self.assertEqual(result["cells"]["height-gap"]["verdict"], "FAIL")

    def test_a_non_numeric_height_is_a_precondition_not_an_exception(self):
        result = self.evaluate(health=self.module._base_health(maxObservedHeight="five"))
        self.assertEqual(result["verdict"], "PRECONDITION", result)
        self.assertEqual(result["cells"]["height-gap"]["verdict"], "PRECONDITION")

    def test_a_string_typed_contract_field_is_a_precondition(self):
        """`bool("false")` is true: a string is not an observation, it is unreadable input."""
        health = self.module._base_health()
        health["ready"] = "false"
        result = self.evaluate(health=health)
        self.assertEqual(result["verdict"], "PRECONDITION", result)
        self.assertIn("health.ready", " ".join(
            cell["detail"] for cell in result["cells"].values()))
        operator = self.module._base_operator()
        operator["finalityStalled"] = "false"
        self.assertEqual(self.evaluate(operator=operator)["verdict"], "PRECONDITION")

    def test_a_missing_journal_is_a_precondition(self):
        result = self.evaluate(block6="")
        self.assertEqual(result["verdict"], "PRECONDITION", result)
        self.assertEqual(result["cells"]["block6-disposition"]["verdict"], "PRECONDITION")

    def test_an_orphan_block_is_a_failure(self):
        result = self.evaluate(block6="block 6 orphan")
        self.assertEqual(result["verdict"], "FAIL", result)
        self.assertEqual(result["cells"]["block6-disposition"]["verdict"], "FAIL")


if __name__ == "__main__":
    unittest.main()
