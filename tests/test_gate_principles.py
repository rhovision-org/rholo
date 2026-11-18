"""The generic criterion suite: every gate is satisfiable, falsifiable and total.

The suites in `test_live_gate.py`, `test_gate_criterion.py` and `test_c343_live.py` are the
negative controls for one instance each. This suite is the principle-level control: it builds
one satisfying observation per registered criterion, applies every declared negative control,
removes each required observation, and corrupts the document, asserting the same properties for
all of them.
"""
from __future__ import annotations

import copy
import hashlib
import importlib.util
import json
import unittest
from pathlib import Path

import gate_fixtures

ROOT = Path(__file__).resolve().parents[1]
CRITERIA = ROOT / "spec" / "audit" / "gate-criteria.json"
EXTERNAL = ROOT / "spec" / "audit" / "mainnet-gates.external.json"


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


principles = _load("gate_principles", ROOT / "tools" / "gate-principles.py")


def _delete(document: dict, path: str) -> dict:
    return gate_fixtures.apply_mutation(document, ["delete", path])


def _rename_actors(document: object) -> object:
    """Rename every actor/alias to a neutral value, preserving distinctness."""
    document = copy.deepcopy(document)
    mapping: dict[str, str] = {}

    def rename(value: str) -> str:
        if value not in mapping:
            mapping[value] = f"actor-{len(mapping) + 1}"
        return mapping[value]

    def walk(value: object, key: str | None = None):
        if isinstance(value, dict):
            for name in list(value):
                if name in {"alias", "operator"} and isinstance(value[name], str):
                    value[name] = rename(value[name])
                else:
                    walk(value[name], name)
        elif isinstance(value, list):
            for item in value:
                walk(item, key)

    walk(document)
    return document


class CriterionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.document = principles.load_criteria(CRITERIA)
        cls.entries = {entry["gate_id"]: entry for entry in cls.document["criteria"]}
        cls.external = {row["id"]: row for row in json.loads(EXTERNAL.read_text())["gates"]}

    @classmethod
    def tearDownClass(cls):
        gate_fixtures.cleanup()

    def evaluate(self, gate_id: str, observations: dict) -> dict:
        return principles.evaluate_entry(self.entries[gate_id], observations)

    def test_every_criterion_is_satisfiable(self):
        for gate_id, entry in self.entries.items():
            with self.subTest(gate=gate_id):
                result = self.evaluate(gate_id, gate_fixtures.base(gate_id))
                self.assertEqual(result["status"], "PASS", result)

    def test_every_declared_control_produces_its_expected_status(self):
        for gate_id, entry in self.entries.items():
            for control in entry["negative_controls"]:
                with self.subTest(gate=gate_id, control=control["name"]):
                    result = self.evaluate(
                        gate_id, gate_fixtures.controlled(gate_id, control))
                    self.assertEqual(result["status"], control["expected"],
                                     (control["name"], result))
                    self.assertTrue(result["reasons"], result)

    def test_a_missing_required_observation_is_precondition(self):
        for gate_id, entry in self.entries.items():
            for path in entry["requires"]:
                with self.subTest(gate=gate_id, path=path):
                    observations = _delete(gate_fixtures.base(gate_id), path)
                    result = self.evaluate(gate_id, observations)
                    self.assertEqual(result["status"], "PRECONDITION", result)

    def test_malformed_observations_produce_a_verdict_not_an_exception(self):
        corruptions = [
            {},
            {"unexpected": True},
            {"subject": None, "record": None, "source": None},
            {"events": "not a list", "run": 7, "preflight": []},
            {"record_data": {"bundle": [], "anchor": "x", "validators": {}}},
            {"health": [], "operator": "x", "block6": 4},
        ]
        for gate_id, entry in self.entries.items():
            for index, corruption in enumerate(corruptions):
                with self.subTest(gate=gate_id, corruption=index):
                    observations = copy.deepcopy(gate_fixtures.base(gate_id))
                    observations.update(copy.deepcopy(corruption))
                    result = principles.evaluate(
                        entry["principle"], observations, entry.get("parameters") or {})
                    self.assertIn(result["status"], principles.STATUSES, result)

    def test_every_criterion_is_non_vacuous(self):
        for gate_id, entry in self.entries.items():
            with self.subTest(gate=gate_id):
                expected = [control["expected"] for control in entry["negative_controls"]]
                self.assertIn("FAIL", expected, "no contradictory control")
                for control in entry["negative_controls"]:
                    result = self.evaluate(
                        gate_id, gate_fixtures.controlled(gate_id, control))
                    self.assertNotEqual(result["status"], "PASS", control["name"])

    def test_the_verdict_does_not_depend_on_actor_names(self):
        for gate_id, entry in self.entries.items():
            with self.subTest(gate=gate_id):
                original = self.evaluate(gate_id, gate_fixtures.base(gate_id))
                renamed = self.evaluate(
                    gate_id, _rename_actors(gate_fixtures.base(gate_id)))
                self.assertEqual(original["status"], renamed["status"],
                                 (gate_id, original, renamed))
                renamed_text = json.dumps(renamed["reasons"])
                self.assertNotIn("v1", renamed_text)
                self.assertNotIn("operator", renamed_text)

    def test_the_registry_covers_every_machine_checkable_external_gate(self):
        executable = {"python", "rust-unit", "rust-integration", "two-binary",
                      "local-devnet", "live-host"}
        expected = {
            gate_id for gate_id, row in self.external.items()
            if row.get("machine_check") is True or row.get("witness_kind") in executable
        }
        self.assertEqual(set(self.entries), expected)

    def test_every_entry_is_bound_to_its_prose_bar_and_adapter(self):
        for gate_id, entry in self.entries.items():
            with self.subTest(gate=gate_id):
                row = self.external[gate_id]
                digest = hashlib.sha256(str(row["pass_condition"]).encode()).hexdigest()
                self.assertEqual(entry["pass_condition_sha256"], digest)
                self.assertIn(entry["principle"], principles.PRINCIPLES)
                self.assertEqual(principles.validate_parameters(
                    entry["principle"], entry.get("parameters")), [])
                self.assertTrue(entry["requires"])
                self.assertTrue(entry["negative_controls"])
                reference = entry["adapter"]
                path_text, _, symbol = reference.partition("::")
                path = ROOT / path_text
                self.assertTrue(path.is_file(), path)
                self.assertIn(symbol, path.read_text(errors="ignore"), reference)


if __name__ == "__main__":
    unittest.main()
