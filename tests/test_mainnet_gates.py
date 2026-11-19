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
BUILD_SPEC = importlib.util.spec_from_file_location(
    "build_mainnet_gates", ROOT / "tools/build-mainnet-gates.py")
builder = importlib.util.module_from_spec(BUILD_SPEC)
BUILD_SPEC.loader.exec_module(builder)
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


def record_digest(record: str) -> str:
    """The digest a binding's `evidence.digest` must carry: sha256(manifest.json)."""
    return hashlib.sha256((Path(record) / "manifest.json").read_bytes()).hexdigest()


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
        for gate_id in ("G520-MIGRATION", "G584-G1"):
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
        cls.binding_root = Path(cls.tmp.name) / "binding"
        cls.binding_root.mkdir()
        (cls.binding_root / "witness.py").write_text(
            "def witness():\n    pass\n\ndef self_test():\n    pass\n")
        (cls.binding_root / "mutation.py").write_text("before\n")

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def setUp(self):
        self.payload = copy.deepcopy(REGISTER)

    def row(self, ident):
        return next(row for row in self.payload["gates"] if row["id"] == ident)

    def inspect(self):
        return gate.validate(self.payload, MATRIX, INVENTORY, LEDGER)

    def make_met(self, ident="G520-MIGRATION"):
        row = self.row(ident)
        ratchet = self.payload.setdefault(
            "met_ratchet", {"minimum": 0, "required_ids": []})
        if ident not in ratchet["required_ids"]:
            ratchet["required_ids"].append(ident)
        ratchet["minimum"] = len(ratchet["required_ids"])
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
                "run_record": self.records[("G520-MIGRATION", "FAIL")],
            },
        }
        row["evidence"] = {
            "url": "https://github.com/rhovision-org/rholo/actions/runs/1",
            "digest": record_digest(self.records[("G520-MIGRATION", "PASS")]),
            "source_sha": "a" * 40,
            "binary_sha256": "b" * 64,
            "record": self.records[("G520-MIGRATION", "PASS")],
        }
        return row

    def make_human_met(self, ident="G584-G1"):
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
            run_record=self.records[("G584-G1", "FAIL")],
        )
        row["evidence"]["record"] = self.records[("G584-G1", "PASS")]
        row["evidence"]["digest"] = record_digest(row["evidence"]["record"])
        return row

    def write_review(self, reviewer: str, verdict: str, digest: str) -> Path:
        path = Path(self.tmp.name) / "review.md"
        path.write_text(f"# Review\n\nReviewer: {reviewer}\nVerdict: {verdict}\n"
                        f"Evidence: {digest}\n")
        return path

    def make_binding(self, ident="G520-MIGRATION"):
        row = next(row for row in REGISTER["gates"] if row["id"] == ident)
        witness = self.binding_root / "witness.py"
        ref = f"{witness}::witness"
        mutation = self.binding_root / "mutation.py"
        return {
            "gate_id": ident,
            "witness_kind": "python",
            "witness_ref": ref,
            "witness_scope": "integration",
            "locality": "single-host",
            "sole_evidence": "local-devnet",
            "docker_free": False,
            "does_not_claim": "A synthetic binding for the promotion test.",
            "witness_binding": {
                "asserts_sha256": hashlib.sha256(
                    str(row["pass_condition"]).encode()).hexdigest(),
                "control": {
                    "description": "Apply the mutation; the control must fail.",
                    "control_of": ref,
                    "mutation": {"file": str(mutation),
                                 "before": "before", "after": "after"},
                    "test": f"{witness}::self_test",
                    "expected": "FAIL",
                    "run_record": self.records[("G520-MIGRATION", "FAIL")],
                },
            },
            "evidence": {
                "url": "https://example.invalid/evidence",
                "digest": record_digest(self.records[("G520-MIGRATION", "PASS")]),
                "source_sha": "a" * 40,
                "binary_sha256": "b" * 64,
                "record": self.records[("G520-MIGRATION", "PASS")],
            },
        }

    def unmet(self, ident="G520-MIGRATION"):
        """A pre-promotion copy: the real register now carries the met rows."""
        row = copy.deepcopy(self.row(ident))
        row.update({
            "status": "owes",
            "status_reason": "synthetic unmet row for the binding tests",
            "witness_kind": None,
            "witness_ref": None,
            "witness_scope": None,
            "witness_binding": None,
            "negative_control": None,
            "evidence": {"url": None, "digest": None, "source_sha": None,
                         "binary_sha256": None, "record": None},
        })
        return row

    def apply_bindings(self, bindings, rows=None, required=None):
        rows = rows if rows is not None else [
            self.unmet("G520-MIGRATION"),
            copy.deepcopy(self.row("G428-M1"))]
        required = required if required is not None else [
            binding["gate_id"] for binding in bindings]
        document = {
            "schema": 1,
            "met_ratchet": {"minimum": len(required), "required_ids": required},
            "bindings": bindings,
        }
        builder.apply_bindings(rows, document)
        return rows

    def test_a_complete_binding_promotes_only_the_bound_row(self):
        rows = self.apply_bindings([self.make_binding()])
        self.assertEqual(rows[0]["status"], "met")
        self.assertIsNone(rows[0]["status_reason"])
        self.assertEqual(rows[0]["witness_kind"], "python")
        self.assertEqual(rows[1]["status"], "blocked")
        self.assertTrue(rows[1]["status_reason"])

    def test_an_orphan_binding_is_refused(self):
        rows = [self.unmet("G520-MIGRATION")]
        with self.assertRaisesRegex(ValueError, "names no gate"):
            self.apply_bindings([self.make_binding("G428-M1")], rows=rows)

    def test_a_duplicate_binding_is_refused(self):
        binding = self.make_binding()
        with self.assertRaisesRegex(ValueError, "duplicate"):
            self.apply_bindings([binding, copy.deepcopy(binding)],
                                rows=[self.unmet("G520-MIGRATION")],
                                required=["G520-MIGRATION"])

    def test_a_partial_binding_is_refused(self):
        binding = self.make_binding()
        del binding["witness_scope"]
        with self.assertRaisesRegex(ValueError, "binding.witness_scope"):
            self.apply_bindings([binding])

    def test_the_asserts_hash_must_match_the_pass_condition(self):
        binding = self.make_binding()
        binding["witness_binding"]["asserts_sha256"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "asserts_sha256"):
            self.apply_bindings([binding])

    def test_the_control_must_be_of_the_witness(self):
        binding = self.make_binding()
        binding["witness_binding"]["control"]["control_of"] = "x.py::other"
        with self.assertRaisesRegex(ValueError, "control must be of this witness"):
            self.apply_bindings([binding])

    def test_the_control_test_must_be_the_witness_or_self_test(self):
        binding = self.make_binding()
        binding["witness_binding"]["control"]["test"] = "x.py::other"
        with self.assertRaisesRegex(ValueError, "witness or its self_test"):
            self.apply_bindings([binding])

    def test_the_builder_refuses_a_rust_witness_that_is_not_a_test(self):
        """The builder asks the checker's question, so it cannot emit a row the judge refuses."""
        rust = self.binding_root / "witness.rs"
        rust.write_text("fn not_a_test() {}\n")
        binding = self.make_binding()
        binding["witness_kind"] = "rust-unit"
        binding["witness_ref"] = f"{rust}::not_a_test"
        control = binding["witness_binding"]["control"]
        control.update(control_of=binding["witness_ref"], test=binding["witness_ref"])
        with self.assertRaisesRegex(ValueError, "#\\[test\\]"):
            self.apply_bindings([binding])

    def test_the_builder_refuses_a_python_comment_as_a_witness(self):
        path = self.binding_root / "comment-only.py"
        path.write_text("# the witness is mentioned here only\n")
        binding = self.make_binding()
        binding["witness_ref"] = f"{path}::witness"
        control = binding["witness_binding"]["control"]
        control.update(control_of=binding["witness_ref"], test=f"{path}::self_test")
        with self.assertRaisesRegex(ValueError, "not defined"):
            self.apply_bindings([binding])

    def test_a_binding_without_its_fail_record_is_refused(self):
        binding = self.make_binding()
        binding["witness_binding"]["control"]["run_record"] = str(
            self.binding_root / "absent")
        with self.assertRaisesRegex(ValueError, "control run record"):
            self.apply_bindings([binding])

    def test_a_decision_binding_is_refused(self):
        row = {"id": "X", "family": "S6", "status": "decision", "depends_on": []}
        eligible, reason = builder._eligible(row, set())
        self.assertFalse(eligible)
        self.assertIn("not promotable", reason)

    def test_a_blocked_binding_is_refused(self):
        ident = "G428-M1"
        with self.assertRaisesRegex(ValueError, "not promotable"):
            self.apply_bindings([self.make_binding(ident)],
                                rows=[copy.deepcopy(self.row(ident))],
                                required=[ident])

    def test_ta_c3_is_not_a_pass_fail_gate(self):
        eligible, reason = builder._eligible(
            {"id": "TA-C3", "family": "TA", "status": "reviewable",
             "depends_on": []}, set())
        self.assertFalse(eligible)
        self.assertIn("not a pass/fail gate", reason)

    def test_an_s6_binding_does_not_need_a_review(self):
        row = {"id": "S6-25bfdbe2c179c81e", "family": "S6",
               "status": "reviewable", "depends_on": []}
        eligible, reason = builder._eligible(row, set())
        self.assertTrue(eligible, reason)

    def test_hf51_is_not_in_the_gate_register(self):
        self.assertFalse(any(row["family"] == "HF51"
                             for row in self.payload["gates"]))

    def test_a_dependency_must_be_met(self):
        row = {
            "id": "G520-MIGRATION",
            "family": "G520",
            "status": "owes",
            "depends_on": ["G584-G1"],
        }
        eligible, reason = builder._eligible(row, set())
        self.assertFalse(eligible)
        self.assertIn("dependency G584-G1", reason)
        eligible, _ = builder._eligible(row, {"G584-G1"})
        self.assertTrue(eligible)

    def test_the_met_ratchet_cannot_regress(self):
        with self.assertRaisesRegex(ValueError, "does not equal the ratchet"):
            self.apply_bindings([self.make_binding()], required=[])

    def test_an_unbound_row_keeps_its_status_and_reason(self):
        rows = self.apply_bindings([], rows=[self.unmet("G520-MIGRATION")],
                                   required=[])
        self.assertEqual(rows[0]["status"], "owes")
        self.assertTrue(rows[0]["status_reason"])

    def test_current_register_is_valid_and_not_closeable(self):
        out = self.inspect()
        self.assertEqual(out["result"], "REGISTER_VALID")
        promoted = self.payload["met_ratchet"]["required_ids"]
        self.assertEqual(out["met"], len(promoted))
        self.assertEqual(out["unresolved"], len(self.payload["gates"]) - len(promoted))
        for ident in promoted:
            self.assertEqual(self.row(ident)["status"], "met")

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
        row = self.make_met("G520-MIGRATION")
        row["depends_on"] = ["G584-G1"]
        with self.assertRaisesRegex(gate.GateError, "cannot be met"):
            self.inspect()

    def test_dependency_cycle_is_refused(self):
        self.row("G428-M1")["depends_on"] = ["G428-M2"]
        self.row("G428-M2")["depends_on"] = ["G428-M1"]
        with self.assertRaisesRegex(gate.GateError, "cycle"):
            self.inspect()

    def test_cross_family_related_needs_a_direction(self):
        edge = next(edge for edge in self.row("G428-M2")["related"]
                    if edge["id"] == "TA-C1")
        edge["criterion"] = "M2 is the ingress gate"
        with self.assertRaisesRegex(gate.GateError, "direction"):
            self.inspect()

    def test_related_edges_must_be_symmetric(self):
        target = self.row("G584-G1")
        target["related"] = [edge for edge in target["related"] if edge["id"] != "G428-M1"]
        with self.assertRaisesRegex(gate.GateError, "not symmetric"):
            self.inspect()

    def test_a_withdrawn_row_is_not_a_gate(self):
        self.assertFalse(any(row["id"] == "S6-dc8df48ecb6197ee"
                             for row in self.payload["gates"]))

    def test_stale_inventory_hash_is_refused(self):
        self.row("S6-25bfdbe2c179c81e")["source_sha256"] = "0" * 64
        with self.assertRaisesRegex(gate.GateError, "stale"):
            self.inspect()

    def test_human_attestation_needs_an_independent_reviewer(self):
        row = self.make_human_met("G584-G1")
        self.assertNotIn("attestation", row)
        with self.assertRaisesRegex(gate.GateError, "attestation.reviewer"):
            self.inspect()

    def test_human_attestation_needs_a_review_record(self):
        row = self.make_human_met("G584-G1")
        row["attestation"] = {
            "reviewer": "external-reviewer",
            "relationship": "not the owner",
            "independent": True,
            "verdict": "PASS",
            "signed_at": "2026-11-18T12:00:00Z",
            "evidence_sha256": row["evidence"]["digest"],
        }
        with self.assertRaisesRegex(gate.GateError, "attestation.review_record"):
            self.inspect()

    def test_human_attestation_needs_the_evidence_digest(self):
        row = self.make_human_met("G584-G1")
        digest = row["evidence"]["digest"]
        review = self.write_review("external-reviewer", "PASS", digest)
        row["attestation"] = {
            "reviewer": "external-reviewer",
            "relationship": "not the owner",
            "independent": True,
            "verdict": "PASS",
            "signed_at": "2026-11-18T12:00:00Z",
            "review_record": str(review),
        }
        with self.assertRaisesRegex(gate.GateError, "evidence_sha256"):
            self.inspect()

    def test_the_signed_digest_must_be_the_evidence_digest(self):
        row = self.make_human_met("G584-G1")
        review = self.write_review("external-reviewer", "PASS", "c" * 64)
        row["attestation"] = {
            "reviewer": "external-reviewer",
            "relationship": "not the owner",
            "independent": True,
            "verdict": "PASS",
            "signed_at": "2026-11-18T12:00:00Z",
            "review_record": str(review),
            "evidence_sha256": "c" * 64,
        }
        with self.assertRaisesRegex(gate.GateError, "not the evidence digest"):
            self.inspect()

    def test_the_review_record_must_carry_the_digest(self):
        row = self.make_human_met("G584-G1")
        digest = row["evidence"]["digest"]
        review = self.write_review("external-reviewer", "PASS", "d" * 64)
        row["attestation"] = {
            "reviewer": "external-reviewer",
            "relationship": "not the owner",
            "independent": True,
            "verdict": "PASS",
            "signed_at": "2026-11-18T12:00:00Z",
            "review_record": str(review),
            "evidence_sha256": digest,
        }
        with self.assertRaisesRegex(gate.GateError, "carry the evidence digest"):
            self.inspect()

    def test_a_complete_attestation_is_accepted(self):
        row = self.make_human_met("G584-G1")
        digest = row["evidence"]["digest"]
        review = self.write_review("external-reviewer", "PASS", digest)
        row["attestation"] = {
            "reviewer": "external-reviewer",
            "relationship": "not the owner",
            "independent": True,
            "verdict": "PASS",
            "signed_at": "2026-11-18T12:00:00Z",
            "review_record": str(review),
            "evidence_sha256": digest,
        }
        self.assertIsInstance(self.inspect(), dict)

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

    def test_python_witness_must_be_a_definition(self):
        row = self.make_met()
        path = self.binding_root / "comment-only.py"
        path.write_text("# the witness is mentioned here only\n")
        row["witness_kind"] = "python"
        row["witness_ref"] = f"{path}::witness"
        with self.assertRaisesRegex(gate.GateError, "not defined"):
            self.inspect()

    def test_the_control_self_test_must_be_in_the_witness_file(self):
        row = self.make_met()
        row["witness_binding"]["control"]["test"] = (
            f"{self.binding_root / 'witness.py'}::self_test")
        with self.assertRaisesRegex(gate.GateError, "own file"):
            self.inspect()

    def test_evidence_digest_must_match_the_manifest(self):
        row = self.make_met()
        row["evidence"]["digest"] = "c" * 64
        with self.assertRaisesRegex(gate.GateError, "manifest"):
            self.inspect()

    def test_source_ref_must_name_the_gate(self):
        self.row("S6-25bfdbe2c179c81e")["source_ref"] = (
            "section6-gap-inventory.json#rows/S6:0000000000000000")
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
        self.row("G520-MIGRATION")["does_not_claim"] = ""
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
