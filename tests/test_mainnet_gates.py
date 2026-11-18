"""Negative controls for the canonical mainnet-gate register."""
import copy
import hashlib
import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "tools/check-mainnet-gates.py"
SPEC = importlib.util.spec_from_file_location("mainnet_gates", SCRIPT)
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)
PACK_SPEC = importlib.util.spec_from_file_location(
    "evidence_pack", ROOT / "tools/evidence-pack.py")
pack = importlib.util.module_from_spec(PACK_SPEC)
PACK_SPEC.loader.exec_module(pack)
REGISTER = json.loads((ROOT / "spec/audit/mainnet-gates.json").read_text())
MATRIX = json.loads((ROOT / "spec/audit/final-gap-matrix.json").read_text())
INVENTORY = json.loads((ROOT / "spec/audit/section6-gap-inventory.json").read_text())
LEDGER = json.loads((ROOT / "spec/audit/hardfork-51-closeout.json").read_text())
M5_TEST = ("casper/tests/n420_boundary_merge.rs"
           "::merging_a_tuple_space_rmw_block_duplicates_its_produce")
MUTATION = {
    "file": "casper/src/merging.rs",
    "before": "expected.0.extend(diff.added.iter().map(Vec::as_slice));",
    "after": "expected.0.extend(diff.removed.iter().map(Vec::as_slice));",
}


class RegisterTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        run = Path(cls.tmp.name) / "run"
        run.mkdir()
        (run / "source-sha.txt").write_text("a" * 40 + "\n")
        (run / "binary-sha256.txt").write_text("b" * 64 + "\n")
        (run / "result.json").write_text(json.dumps({"result": "PASS"}))
        cls.records = {}
        for gate_id in ("G428-M5", "G584-G6"):
            for verdict in ("PASS", "FAIL"):
                record = Path(cls.tmp.name) / f"{gate_id}-{verdict.lower()}"
                args = SimpleNamespace(
                    run_dir=str(run), record=str(record), gate_id=gate_id,
                    verdict=verdict, source_sha=None, binary_sha256=None,
                    run_url="https://github.com/rhovision-org/rholo/actions/runs/1",
                    reference_url=None, locality="local-devnet", redact=[], move=False,
                )
                pack.cmd_pack(args)
                readme = record / "README.md"
                readme.write_text(pack.PLACEHOLDER.sub("authored", readme.read_text()))
                cls.records[(gate_id, verdict)] = str(record)

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def setUp(self):
        self.payload = copy.deepcopy(REGISTER)

    def row(self, ident):
        return next(row for row in self.payload["gates"] if row["id"] == ident)

    def inspect(self):
        return gate.validate(self.payload, MATRIX, INVENTORY, LEDGER)

    def make_met(self, ident="G428-M5"):
        row = self.row(ident)
        row["status"] = "met"
        row["witness_kind"] = "rust-integration"
        row["witness_ref"] = M5_TEST
        row["locality"] = "integration"
        row["sole_evidence"] = "none"
        row["witness_binding"] = {
            "asserts_sha256": hashlib.sha256(row["pass_condition"].encode()).hexdigest(),
            "control": {
                "description": "Remove the kept-effect coverage; the merge test must fail.",
                "control_of": M5_TEST,
                "mutation": dict(MUTATION),
                "test": M5_TEST,
                "expected": "FAIL",
                "run_record": self.records[("G428-M5", "FAIL")],
            },
        }
        row["evidence"] = {
            "url": "https://github.com/rhovision-org/rholo/actions/runs/1",
            "digest": "c" * 64,
            "source_sha": "a" * 40,
            "binary_sha256": "b" * 64,
            "record": self.records[("G428-M5", "PASS")],
        }
        return row

    def make_human_met(self, ident="G584-G6"):
        row = self.make_met(ident)
        row["witness_kind"] = "human-attestation"
        row["witness_ref"] = "docs/src/spec/gate-terms.md::reviewer"
        row["locality"] = "external"
        row["sole_evidence"] = "external"
        control = row["witness_binding"]["control"]
        control.update(
            control_of="docs/src/spec/gate-terms.md::reviewer",
            test="docs/src/spec/gate-terms.md::reviewer",
            mutation={"file": "docs/src/spec/gate-terms.md",
                      "before": "Independent", "after": "Dependent"},
            run_record=self.records[("G584-G6", "FAIL")],
        )
        row["evidence"]["record"] = self.records[("G584-G6", "PASS")]
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

    def test_met_gate_without_witness_binding_is_refused(self):
        row = self.make_met()
        del row["witness_binding"]
        with self.assertRaisesRegex(gate.GateError, "witness_binding"):
            self.inspect()

    def test_control_must_be_of_the_witness(self):
        row = self.make_met()
        row["witness_binding"]["control"]["control_of"] = (
            "node/src/web/http.rs::health_is_ready_only_when_every_shard_runs")
        with self.assertRaisesRegex(gate.GateError, "control must be of this witness"):
            self.inspect()

    def test_control_mutation_text_must_exist(self):
        row = self.make_met()
        row["witness_binding"]["control"]["mutation"]["before"] = "not-in-the-file"
        with self.assertRaisesRegex(gate.GateError, "before text"):
            self.inspect()

    def test_control_must_be_a_test(self):
        row = self.make_met()
        row["witness_binding"]["control"]["test"] = "README.md"
        with self.assertRaisesRegex(gate.GateError, "path::symbol"):
            self.inspect()

    def test_aggregate_only_evidence_is_refused(self):
        row = self.make_met()
        row["evidence"] = {
            "url": "https://example.invalid",
            "digest": "c" * 64,
            "source_sha": "a" * 40,
            "binary_sha256": "b" * 64,
            "aggregate": "PASS",
        }
        with self.assertRaisesRegex(gate.GateError, "evidence record"):
            self.inspect()

    def test_unrelated_evidence_record_is_refused(self):
        row = self.make_met()
        row["evidence"]["record"] = "spec/audit/evidence/m5-06-m5-04-20261113"
        with self.assertRaisesRegex(gate.GateError, "names"):
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
        row = self.make_met("G428-M5")
        row["depends_on"] = ["G584-G1"]
        with self.assertRaisesRegex(gate.GateError, "cannot be met"):
            self.inspect()

    def test_dependency_cycle_is_refused(self):
        self.row("G428-M1")["depends_on"] = ["G428-M2"]
        self.row("G428-M2")["depends_on"] = ["G428-M1"]
        with self.assertRaisesRegex(gate.GateError, "cycle"):
            self.inspect()

    def test_cross_family_related_needs_a_direction(self):
        edge = next(edge for edge in self.row("G428-M5")["related"]
                    if edge["id"].startswith("HF51"))
        edge["criterion"] = "M5 is the block-merge gate"
        with self.assertRaisesRegex(gate.GateError, "direction"):
            self.inspect()

    def test_related_edges_must_be_symmetric(self):
        target = self.row("G584-G1")
        target["related"] = [edge for edge in target["related"] if edge["id"] != "G428-M1"]
        with self.assertRaisesRegex(gate.GateError, "not symmetric"):
            self.inspect()

    def test_disposition_register_drift_is_refused(self):
        row = self.row("S6-f735095c746556f3")
        row["status"] = "superseded"
        with self.assertRaisesRegex(gate.GateError, "disposition"):
            self.inspect()

    def test_stale_matrix_hash_is_refused(self):
        self.row("HF51-A-block-merge")["source_hash"] = "0" * 64
        with self.assertRaisesRegex(gate.GateError, "stale"):
            self.inspect()

    def test_human_attestation_needs_an_independent_reviewer(self):
        row = self.make_human_met("G584-G6")
        self.assertNotIn("attestation", row)
        with self.assertRaisesRegex(gate.GateError, "attestation.reviewer"):
            self.inspect()

    def test_human_attestation_needs_a_review_record(self):
        row = self.make_human_met("G584-G6")
        row["attestation"] = {
            "reviewer": "external-reviewer",
            "relationship": "not the owner",
            "independent": True,
            "verdict": "PASS",
            "signed_at": "2026-11-18T12:00:00Z",
            "evidence_sha256": "c" * 64,
        }
        with self.assertRaisesRegex(gate.GateError, "attestation.review_record"):
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

    def test_source_phrase_must_be_in_the_excerpt(self):
        self.row("G428-M1")["source_phrase"] = "a tautology"
        with self.assertRaisesRegex(gate.GateError, "source phrase"):
            self.inspect()

    def test_criteria_hash_must_match_the_gate_text(self):
        self.row("G428-M1")["pass_condition"] = "Trivially met"
        with self.assertRaisesRegex(gate.GateError, "criteria hash"):
            self.inspect()

    def test_excerpt_hash_must_match(self):
        self.payload["sources"]["G428"]["excerpt_sha256"] = "0" * 64
        with self.assertRaisesRegex(gate.GateError, "excerpt hash"):
            self.inspect()

    def test_does_not_claim_is_required(self):
        self.row("G428-M5")["does_not_claim"] = ""
        with self.assertRaisesRegex(gate.GateError, "does_not_claim"):
            self.inspect()

    def test_assert_closeable_exits_three(self):
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--assert-closeable"],
            capture_output=True, text=True, timeout=120,
        )
        self.assertEqual(result.returncode, 3)
        self.assertIn("TRACKER_MUST_REMAIN_OPEN", result.stdout)

    def test_check_mode_is_current(self):
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--check"],
            capture_output=True, text=True, timeout=120,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("REGISTER_VALID", result.stdout)

    def test_assert_closeable_implies_check(self):
        with tempfile.TemporaryDirectory() as tmp:
            edited = Path(tmp) / "mainnet-gates.json"
            payload = copy.deepcopy(REGISTER)
            payload["gates"][0]["title"] = "A hand-edited title"
            edited.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
            result = subprocess.run(
                [sys.executable, str(SCRIPT), str(edited), "--assert-closeable"],
                capture_output=True, text=True, timeout=120,
            )
        self.assertEqual(result.returncode, 1)
        self.assertIn("STALE", result.stdout)


if __name__ == "__main__":
    unittest.main()
