"""Negative controls for the canonical mainnet-gate register."""
import copy
import importlib.util
import json
import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "tools/check-mainnet-gates.py"
SPEC = importlib.util.spec_from_file_location("mainnet_gates", SCRIPT)
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)
REGISTER = json.loads((ROOT / "spec/audit/mainnet-gates.json").read_text())
MATRIX = json.loads((ROOT / "spec/audit/final-gap-matrix.json").read_text())
INVENTORY = json.loads((ROOT / "spec/audit/section6-gap-inventory.json").read_text())
LEDGER = json.loads((ROOT / "spec/audit/hardfork-51-closeout.json").read_text())
VALID_EVIDENCE = {
    "url": "https://github.com/rhovision-org/rholo/actions/runs/1",
    "digest": "c" * 64,
    "source_sha": "a" * 40,
    "binary_sha256": "b" * 64,
    "record": "spec/audit/evidence/m5-06-m5-04-20261113",
}


class RegisterTests(unittest.TestCase):
    def setUp(self):
        self.payload = copy.deepcopy(REGISTER)

    def row(self, ident):
        return next(row for row in self.payload["gates"] if row["id"] == ident)

    def inspect(self):
        return gate.validate(self.payload, MATRIX, INVENTORY, LEDGER)

    def make_met(self, ident="G428-M5"):
        row = self.row(ident)
        row["status"] = "met"
        row["witness_kind"] = "rust-unit"
        row["witness_ref"] = "node/src/web/http.rs::health_is_ready_only_when_every_shard_runs"
        row["locality"] = "unit"
        row["sole_evidence"] = "none"
        row["negative_control"] = {
            "description": "Remove the readiness conjunction; the witness must fail.",
            "control_ref": "tests/test_mainnet_gates.py::test_current_register_is_valid_and_not_closeable",
            "expected": "FAIL",
        }
        row["evidence"] = copy.deepcopy(VALID_EVIDENCE)
        return row

    def test_current_register_is_valid_and_not_closeable(self):
        out = self.inspect()
        self.assertEqual(out["result"], "REGISTER_VALID")
        self.assertEqual(out["met"], 0)
        self.assertEqual(out["unresolved"], len(self.payload["gates"]))

    def test_met_gate_without_a_witness_is_refused(self):
        row = self.make_met()
        row["witness_ref"] = None
        with self.assertRaisesRegex(gate.GateError, "witness"):
            self.inspect()

    def test_met_gate_without_a_negative_control_is_refused(self):
        row = self.make_met()
        row["negative_control"] = ""
        with self.assertRaisesRegex(gate.GateError, "control"):
            self.inspect()

    def test_aggregate_only_evidence_is_refused(self):
        row = self.make_met()
        row["evidence"] = {"url": VALID_EVIDENCE["url"], "aggregate": "PASS"}
        with self.assertRaisesRegex(gate.GateError, "evidence.record|source_sha"):
            self.inspect()

    def test_docker_cannot_be_the_sole_evidence(self):
        row = self.make_met()
        row["sole_evidence"] = "docker"
        with self.assertRaisesRegex(gate.GateError, "Docker"):
            self.inspect()

    def test_unclassified_locality_cannot_be_met(self):
        row = self.make_met()
        row["locality"] = "unclassified"
        with self.assertRaisesRegex(gate.GateError, "unclassified"):
            self.inspect()

    def test_met_gate_cannot_depend_on_an_unmet_gate(self):
        row = self.make_met("G428-M4")
        row["witness_ref"] = "tests/consensus-partition-rejoin.py::establish_healing_anchor"
        row["witness_kind"] = "python"
        row["locality"] = "multi-host"
        row["sole_evidence"] = "local-devnet"
        with self.assertRaisesRegex(gate.GateError, "cannot be met"):
            self.inspect()

    def test_dependency_cycle_is_refused(self):
        self.row("G428-M1")["depends_on"] = ["G428-M2"]
        self.row("G428-M2")["depends_on"] = ["G428-M1"]
        with self.assertRaisesRegex(gate.GateError, "cycle"):
            self.inspect()

    def test_related_edges_must_be_symmetric(self):
        target = self.row("G584-G1")
        target["related"] = [edge for edge in target["related"] if edge["id"] != "G428-M1"]
        with self.assertRaisesRegex(gate.GateError, "not symmetric"):
            self.inspect()

    def test_stale_matrix_hash_is_refused(self):
        self.row("HF51-A-block-merge")["source_hash"] = "0" * 64
        with self.assertRaisesRegex(gate.GateError, "stale"):
            self.inspect()

    def test_human_attestation_needs_an_independent_reviewer(self):
        row = self.make_met("G584-G6")
        row["witness_kind"] = "human-attestation"
        row["witness_ref"] = "README.md::reviewer"
        row["sole_evidence"] = "external"
        row["locality"] = "external"
        self.assertNotIn("attestation", row)
        with self.assertRaisesRegex(gate.GateError, "attestation.reviewer"):
            self.inspect()

    def test_bare_path_witness_is_refused(self):
        row = self.make_met()
        row["witness_ref"] = "README.md"
        with self.assertRaisesRegex(gate.GateError, "path::symbol"):
            self.inspect()

    def test_struct_witness_is_refused(self):
        row = self.make_met()
        row["witness_ref"] = "casper/src/merging.rs::MergeReport"
        with self.assertRaisesRegex(gate.GateError, "#\\[test\\]"):
            self.inspect()

    def test_negative_control_must_name_a_test(self):
        row = self.make_met()
        row["negative_control"]["control_ref"] = "README.md"
        with self.assertRaisesRegex(gate.GateError, "path::symbol"):
            self.inspect()

    def test_source_ref_must_name_the_gate(self):
        self.row("HF51-A-block-merge")["source_ref"] = (
            "final-gap-matrix.json#items/A:vault-export")
        with self.assertRaisesRegex(gate.GateError, "does not name this gate"):
            self.inspect()

    def test_law_citation_must_resolve(self):
        self.row("C343")["laws"] = ["nope"]
        with self.assertRaisesRegex(gate.GateError, "not in laws.tsv"):
            self.inspect()

    def test_register_dev_sha_must_be_an_ancestor(self):
        self.payload["register_dev_sha"] = "0" * 40
        with self.assertRaisesRegex(gate.GateError, "ancestor"):
            self.inspect()

    def test_source_block_must_match(self):
        self.payload["sources"]["G428"]["sha256"] = "0" * 64
        with self.assertRaisesRegex(gate.GateError, "source hash"):
            self.inspect()

    def test_does_not_claim_is_required(self):
        self.row("G428-M5")["does_not_claim"] = ""
        with self.assertRaisesRegex(gate.GateError, "does_not_claim"):
            self.inspect()

    def test_assert_closeable_exits_three(self):
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--assert-closeable"],
            capture_output=True, text=True, timeout=60,
        )
        self.assertEqual(result.returncode, 3)
        self.assertIn("TRACKER_MUST_REMAIN_OPEN", result.stdout)

    def test_check_mode_is_current(self):
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--check"],
            capture_output=True, text=True, timeout=60,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("REGISTER_VALID", result.stdout)


if __name__ == "__main__":
    unittest.main()
