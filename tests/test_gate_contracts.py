"""Negative controls for the three release-blocker contract checkers."""
import copy
import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "tools/check-gate-contracts.py"
SPEC = importlib.util.spec_from_file_location("gate_contracts", SCRIPT)
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)

PROVENANCE = {
    "operator": "operator",
    "signed_at": "2026-11-18T12:00:00Z",
    "digest": "d" * 64,
    "record": "spec/audit/evidence/n564-binary-qualification-20261113",
}
VAULT = {
    "schema": 1,
    "provenance": PROVENANCE,
    "checkpoint": {"height": 10, "hash": "a" * 64},
    "entries": [
        {"id": "v1", "kind": "vault", "balance": 100},
        {"id": "b1", "kind": "bond", "balance": 50},
        {"id": "r1", "kind": "reward", "balance": 25},
        {"id": "p1", "kind": "pot", "balance": 25},
    ],
    "totals": {"supply": 200, "vaults": 100, "bonded": 50, "rewards": 25, "pot": 25},
}
STATE = {
    "schema": 1,
    "provenance": PROVENANCE,
    "checkpoint": {"height": 10, "hash": "a" * 64},
    "roots": {"native": "b" * 64, "rspace": "c" * 64, "history": "d" * 64},
    "counts": {"native": 2, "rspace": 3, "history": 4},
    "entries": {"native": [1, 2], "rspace": [1, 2, 3], "history": [1, 2, 3, 4]},
    "complete": True,
}
ATTACHMENT = {
    "schema": 1,
    "provenance": PROVENANCE,
    "selected": {"source_sha": "a" * 40, "artifact_digest": "b" * 64,
                 "binary_sha256": "c" * 64},
    "validators": [
        {"id": "B", "version": "2", "binary_sha256": "c" * 64,
         "activated_utc": "2026-11-18T20:00:00Z", "still_serving_old": False},
        {"id": "C", "version": "2", "binary_sha256": "c" * 64,
         "activated_utc": "2026-11-18T20:00:05Z", "still_serving_old": False},
        {"id": "D", "version": "2", "binary_sha256": "c" * 64,
         "activated_utc": "2026-11-18T20:00:10Z", "still_serving_old": False},
    ],
    "clients": [
        {"id": "new", "version": "2", "supports_attachments": True, "refuses_old": False},
        {"id": "old", "version": "1", "supports_attachments": False, "refuses_old": True},
    ],
    "window_seconds": 30,
}


class ContractTests(unittest.TestCase):
    def test_valid_records_pass(self):
        for checker, record in ((gate.check_vault, VAULT), (gate.check_state, STATE),
                                (gate.check_attachment, ATTACHMENT)):
            with self.subTest(checker=checker.__name__):
                result, errors, _ = checker(copy.deepcopy(record))
                self.assertEqual(result, "PASS", errors)

    def test_vault_conservation_and_uniqueness(self):
        bad = copy.deepcopy(VAULT)
        bad["totals"]["supply"] = 201
        self.assertEqual(gate.check_vault(bad)[0], "FAIL")
        duplicate = copy.deepcopy(VAULT)
        duplicate["entries"].append({"id": "v1", "kind": "vault", "balance": 1})
        self.assertEqual(gate.check_vault(duplicate)[0], "FAIL")
        negative = copy.deepcopy(VAULT)
        negative["entries"][0]["balance"] = -1
        self.assertEqual(gate.check_vault(negative)[0], "FAIL")

    def test_schema_and_per_kind_conservation_mutants(self):
        schema = copy.deepcopy(VAULT)
        schema["schema"] = 2
        self.assertEqual(gate.check_vault(schema)[0], "FAIL")
        kind = copy.deepcopy(VAULT)
        kind["totals"]["vaults"] = 99
        self.assertEqual(gate.check_vault(kind)[0], "FAIL")
        negative = copy.deepcopy(VAULT)
        negative["totals"]["supply"] = -1
        self.assertEqual(gate.check_vault(negative)[0], "FAIL")

    def test_roots_format_is_checked(self):
        bad = copy.deepcopy(STATE)
        bad["roots"]["native"] = "x"
        self.assertEqual(gate.check_state(bad)[0], "FAIL")

    def test_state_completeness_is_counts_and_roots(self):
        incomplete = copy.deepcopy(STATE)
        incomplete["complete"] = False
        self.assertEqual(gate.check_state(incomplete)[0], "FAIL")
        mismatch = copy.deepcopy(STATE)
        mismatch["entries"]["native"] = [1]
        self.assertEqual(gate.check_state(mismatch)[0], "FAIL")

    def test_attachment_lockstep_and_refusals(self):
        mixed = copy.deepcopy(ATTACHMENT)
        mixed["validators"][0]["binary_sha256"] = "d" * 64
        self.assertEqual(gate.check_attachment(mixed)[0], "FAIL")
        old = copy.deepcopy(ATTACHMENT)
        old["validators"][0]["still_serving_old"] = True
        self.assertEqual(gate.check_attachment(old)[0], "FAIL")
        no_clients = copy.deepcopy(ATTACHMENT)
        no_clients.pop("clients")
        self.assertEqual(gate.check_attachment(no_clients)[0], "PRECONDITION")
        span = copy.deepcopy(ATTACHMENT)
        span["validators"][2]["activated_utc"] = "2026-11-18T20:01:00Z"
        self.assertEqual(gate.check_attachment(span)[0], "FAIL")

    def test_missing_provenance_is_precondition(self):
        for checker, record in ((gate.check_vault, VAULT), (gate.check_state, STATE),
                                (gate.check_attachment, ATTACHMENT)):
            with self.subTest(checker=checker.__name__):
                bad = copy.deepcopy(record)
                bad.pop("provenance")
                self.assertEqual(checker(bad)[0], "PRECONDITION")

    def test_state_entries_are_required(self):
        bad = copy.deepcopy(STATE)
        bad.pop("entries")
        self.assertEqual(gate.check_state(bad)[0], "FAIL")

    def test_zero_roots_are_refused(self):
        bad = copy.deepcopy(STATE)
        bad["roots"]["native"] = "0" * 64
        self.assertEqual(gate.check_state(bad)[0], "FAIL")

    def test_a_contract_pass_does_not_flip_a_register_gate(self):
        register = json.loads((ROOT / "spec/audit/mainnet-gates.json").read_text())
        for ident in ("HF51-A-vault-export", "HF51-Batch-state-preservation",
                      "HF51-Release-attachment-upgrade"):
            row = next(row for row in register["gates"] if row["id"] == ident)
            self.assertNotEqual(row["status"], "met", ident)

    def test_missing_record_is_precondition(self):
        with tempfile.TemporaryDirectory() as tmp:
            result = subprocess.run(
                [sys.executable, str(SCRIPT), "vault", "--record", str(Path(tmp) / "absent.json")],
                capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 3)
        self.assertIn("PRECONDITION", result.stdout)


if __name__ == "__main__":
    unittest.main()
