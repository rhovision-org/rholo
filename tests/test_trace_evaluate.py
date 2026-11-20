"""The phase-space evaluator recomputes each predicate from the trace coordinates.

A trace is evidence, not a label: the node's own `predicates` booleans are deliberately wrong in
one case and the evaluator must still find the unsafe coordinate.
"""
import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "trace_evaluate", ROOT / "tools" / "trace-evaluate.py")
evaluate_tool = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(evaluate_tool)


def trace(**overrides):
    row = {
        "validator": 0,
        "mode": "fringe",
        "claims": [{"height": 3, "id": "ab" * 32, "hash": "cd" * 32}],
        "certified_base": None,
        "base_source": "agreed",
        "conflict_scope": ["ef" * 32],
        "final_scope": [],
        "fork_blocks": [],
        "rejected_deploys": [],
        "rejected_hosts": [],
        "report": {"conflict_chains": 1, "kept_chains": 0, "rejected_chains": 1,
                   "fork_rejected_chains": 0, "final_scope_rejected_chains": 0},
        # Deliberately the node's own labels, which the evaluator must not trust.
        "predicates": {"base_ok": True, "fork_ok": True,
                       "ledger_ok": True, "scope_partition_ok": True},
    }
    row.update(overrides)
    return row


def record(traces, verdict="fail"):
    return {"claim_id": "RC-1", "verdict": verdict,
            "observations": {"traces": traces}}


class EvaluateTests(unittest.TestCase):
    def test_a_safe_trace_reads_as_safe(self):
        report, code = evaluate_tool.evaluate(record([trace()], verdict="pass"))
        self.assertEqual(code, 0)
        self.assertEqual(report["reading"], "safe")
        self.assertEqual(report["results"][0]["false"], [])

    def test_disagreeing_claims_make_base_ok_false(self):
        row = trace(claims=[
            {"height": 3, "id": "ab" * 32, "hash": "cd" * 32},
            {"height": 4, "id": "ac" * 32, "hash": "ce" * 32},
        ])
        report, code = evaluate_tool.evaluate(record([row]))
        self.assertEqual(code, 1)
        self.assertIn("base_ok", report["results"][0]["false"])

    def test_a_certified_base_is_agreement_even_when_claims_differ(self):
        row = trace(certified_base="ff" * 32, claims=[
            {"height": 3, "id": "ab" * 32, "hash": "cd" * 32},
            {"height": 4, "id": "ac" * 32, "hash": "ce" * 32},
        ])
        report, _ = evaluate_tool.evaluate(record([row]))
        self.assertEqual(report["results"][0]["false"], [])

    def test_a_fringe_merge_with_fork_blocks_makes_fork_ok_false(self):
        report, code = evaluate_tool.evaluate(record([trace(fork_blocks=["11" * 32])]))
        self.assertEqual(code, 1)
        self.assertIn("fork_ok", report["results"][0]["false"])

    def test_a_conflict_merge_may_name_fork_blocks(self):
        row = trace(mode="conflict", fork_blocks=["11" * 32])
        report, _ = evaluate_tool.evaluate(record([row]))
        self.assertEqual(report["results"][0]["false"], [])

    def test_a_rejected_finalised_host_makes_ledger_ok_false(self):
        row = trace(rejected_hosts=["ef" * 32])
        report, code = evaluate_tool.evaluate(record([row]))
        self.assertEqual(code, 1)
        self.assertIn("ledger_ok", report["results"][0]["false"])

    def test_a_non_partition_makes_scope_partition_ok_false(self):
        row = trace(report={"conflict_chains": 2, "kept_chains": 0,
                            "rejected_chains": 1})
        report, code = evaluate_tool.evaluate(record([row]))
        self.assertEqual(code, 1)
        self.assertIn("scope_partition_ok", report["results"][0]["false"])

    def test_every_predicate_true_and_a_failed_claim_is_a_model_gap(self):
        report, code = evaluate_tool.evaluate(record([trace()], verdict="fail"))
        self.assertEqual(code, 2)
        self.assertEqual(report["reading"], "model-incomplete")

    def test_a_failure_without_a_trace_is_evidence_insufficient(self):
        report, code = evaluate_tool.evaluate(record([], verdict="fail"))
        self.assertEqual(code, 3)
        self.assertEqual(report["reading"], "evidence-insufficient")

    def test_a_false_predicate_carries_its_rust_anchor_and_lean_declaration(self):
        row = trace(fork_blocks=["11" * 32])
        report, _ = evaluate_tool.evaluate(record([row]))
        anchors = report["results"][0]["anchors"]
        self.assertTrue(any(a["rust"].startswith("casper/src/merging.rs:") for a in anchors))
        self.assertTrue(any(a["lean"].startswith("Rchain.") for a in anchors))

    def test_a_boot_trace_is_not_evaluated_as_a_merge(self):
        boot = {"validator": 0, "acquisition": "certificate", "accepted": True,
                "dag_len": 4, "store_head": None, "source_configured": True}
        report, code = evaluate_tool.evaluate(record([boot], verdict="pass"))
        self.assertEqual(report["traces"], 0)
        self.assertEqual(code, 3)

    def test_the_cli_reads_a_record_file(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "claim.json"
            path.write_text(json.dumps(record([trace()], verdict="pass")))
            result = subprocess.run(
                [sys.executable, str(ROOT / "tools" / "trace-evaluate.py"),
                 "--record", str(path)],
                text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["reading"], "safe")


if __name__ == "__main__":
    unittest.main()
