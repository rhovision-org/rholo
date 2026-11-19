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

ROOT = Path(__file__).resolve().parents[1]


def _fixtures():
    spec = importlib.util.spec_from_file_location(
        "gate_fixtures", ROOT / "tests" / "gate_fixtures.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


gate_fixtures = _fixtures()
CRITERIA = ROOT / "spec" / "audit" / "gate-criteria.json"
EXTERNAL = ROOT / "spec" / "audit" / "mainnet-gates.external.json"


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


principles = _load("gate_principles", ROOT / "tools" / "gate-principles.py")
gate_selftest = _load("gate_selftest", ROOT / "tools" / "gate-selftest.py")


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

    def test_the_shared_self_test_passes(self):
        """The same function the register check runs: satisfiable, falsifiable, load-bearing.

        One implementation, so the suite and the CI-reached `check_criteria` cannot drift.
        """
        errors = gate_selftest.run(self.entries, principles=principles)
        self.assertEqual(errors, [])

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

    def test_the_human_attestation_record_is_typed_and_independent(self):
        attestation = {
            "reviewer": "independent-reviewer",
            "relationship": "not the owner or operator",
            "independent": True,
            "signed_at": "2026-11-18T12:00:00Z",
            "verdict": "PASS",
            "evidence_sha256": gate_fixtures.manifest_sha256(
                ROOT / gate_fixtures.ATTESTATION_RECORD),
            "verdict_url": "https://example.invalid/verdict",
            "evidence_record": gate_fixtures.ATTESTATION_RECORD,
        }
        parameters = {"owner": "Owner / release coordination"}
        self.assertEqual(
            principles.evaluate("human_attestation", {"attestation": attestation},
                                parameters)["status"],
            "PASS")
        self_owned = dict(attestation, reviewer="Owner")
        self.assertEqual(
            principles.evaluate("human_attestation", {"attestation": self_owned},
                                parameters)["status"],
            "FAIL")
        missing = dict(attestation)
        missing.pop("verdict_url")
        self.assertEqual(
            principles.evaluate("human_attestation", {"attestation": missing},
                                parameters)["status"],
            "FAIL")
        inconclusive = dict(attestation, verdict="INCONCLUSIVE")
        self.assertEqual(
            principles.evaluate("human_attestation", {"attestation": inconclusive},
                                parameters)["status"],
            "PRECONDITION")
        self.assertEqual(
            principles.evaluate("human_attestation", {}, parameters)["status"],
            "PRECONDITION")

    def test_a_materialised_map_cannot_escape_its_directory(self):
        result = principles.evaluate(
            "off_host_claim", {"record": {"files": {"../escape.log": "x"}}}, {})
        self.assertEqual(result["status"], "FAIL", result)
        self.assertFalse((ROOT / "target" / "escape.log").exists())

    def test_an_invalid_parameter_is_a_fail_not_a_coercion(self):
        """The evaluate path, not just the meta-test, refuses a malformed criterion parameter."""
        entry = {"principle": "no_false_settlement",
                 "parameters": {"min_observers": "4"},
                 "requires": ["record"]}
        result = principles.evaluate_entry(entry, {"record": {}})
        self.assertEqual(result["status"], "FAIL", result)
        self.assertTrue(any("parameters" in reason for reason in result["reasons"]), result)
        self.assertEqual(
            principles.evaluate_entry(
                {"principle": "no_false_settlement", "parameters": {}, "requires": ["record"]},
                {"record": {}})["status"],
            "FAIL")
        self.assertEqual(
            principles.evaluate_entry(
                {"principle": "no_false_settlement",
                 "parameters": {"min_observers": 4, "unknown": True},
                 "requires": ["record"]},
                {"record": {}})["status"],
            "FAIL")

    def test_a_process_pid_is_listed_once_across_names(self):
        document = {"record": {
            "host_processes": [
                {"pid": 101, "name": "rnode"},
                {"pid": 101, "name": "rnode-watchdog"},
            ],
            "declared_count": 1,
        }}
        self.assertEqual(
            principles.evaluate("resource_attribution", document,
                                {"process_name": "rnode"})["status"],
            "FAIL")

    def test_the_registry_covers_every_machine_checkable_external_gate(self):
        executable = {"python", "rust-unit", "rust-integration", "two-binary",
                      "local-devnet", "live-host"}
        expected = {
            gate_id for gate_id, row in self.external.items()
            if row.get("gate", True)
            and (row.get("machine_check") is True or row.get("witness_kind") in executable)
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
                module = _load(f"adapter_{gate_id.replace('-', '_').lower()}", path)
                self.assertTrue(callable(getattr(module, symbol, None)),
                                f"{reference} does not name a callable")
                if path_text == "tools/gate-principles.py" and symbol == entry["principle"]:
                    continue
                self.assertIn(
                    symbol, principles.PRINCIPLES[entry["principle"]].__code__.co_names,
                    f"{reference} is not called by {entry['principle']}")


if __name__ == "__main__":
    unittest.main()
