"""The phase-space evaluator recomputes each predicate from the trace coordinates.

A trace is evidence, not a label: the node's own `predicates`/`bounded`/`accepted` booleans are
deliberately wrong in the fixtures, and the evaluator must still find the unsafe coordinate.
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
        "rejected_user_hosts": [],
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


def finality(**overrides):
    row = {
        "kind": "finality", "validator": 0,
        "bonded_total": 400, "quorum_total": 400, "supporting": 300,
        "partition": ["aa"], "bonded_senders": ["aa", "bb", "cc", "dd"],
        "advanced": True, "reason_kind": None,
    }
    row.update(overrides)
    return row


def state(**overrides):
    row = {
        "kind": "state", "validator": 0, "block": "ab" * 32,
        "claimed_pre": "01", "computed_pre": "01",
        "claimed_post": "02", "replayed_post": "02",
        "claimed_rejected": [], "computed_rejected": [], "verdict": "matched",
    }
    row.update(overrides)
    return row


def boot(**overrides):
    row = {
        "kind": "boot", "validator": 1, "acquisition": "certificate", "accepted": True,
        "genesis_only": False, "dag_len": 40, "store_head": None,
        "source_configured": True, "served_supporting_stake": 300,
        "served_total_stake": 400, "outcome": "certified",
    }
    row.update(overrides)
    return row


def admission(**overrides):
    row = {
        "kind": "admission", "site": "http-inflight", "surface": "http",
        "refused": True, "class": "inflight", "status": 429, "retry_after": 1,
        "inflight": 2, "inflight_limit": 2,
    }
    row.update(overrides)
    return row


def audit(**overrides):
    row = {
        "kind": "audit", "anchor": "ab" * 32, "anchor_in_fringe": True,
        "target_hashes": ["ab" * 32], "target_state": "cd" * 32,
        "anchor_post_state": "cd" * 32, "verdict": "AGREE",
        "derived_root": "ef" * 32, "claimed_root": "ef" * 32,
    }
    row.update(overrides)
    return row


def store_format(**overrides):
    row = {
        "kind": "format", "build": 4, "on_disk": 5,
        "decision": "refuse_newer", "stamped": False,
    }
    row.update(overrides)
    return row


class MergeTests(unittest.TestCase):
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

    def test_a_rejected_finalising_host_makes_ledger_ok_false(self):
        row = trace(conflict_scope=["ef" * 32], final_scope=["aa" * 32],
                    rejected_hosts=["ef" * 32], rejected_user_hosts=["ef" * 32])
        report, code = evaluate_tool.evaluate(record([row]))
        self.assertEqual(code, 1)
        self.assertIn("ledger_ok", report["results"][0]["false"])

    def test_a_final_scope_only_rejection_is_not_a_ledger_failure(self):
        row = trace(conflict_scope=["aa" * 32], final_scope=["ef" * 32],
                    rejected_hosts=["ef" * 32], rejected_user_hosts=["ef" * 32])
        report, _ = evaluate_tool.evaluate(record([row]))
        self.assertEqual(report["results"][0]["false"], [])

    def test_a_system_chain_rejection_is_not_a_ledger_failure(self):
        row = trace(conflict_scope=["ef" * 32], rejected_hosts=["ef" * 32],
                    rejected_user_hosts=[])
        report, _ = evaluate_tool.evaluate(record([row]))
        self.assertEqual(report["results"][0]["false"], [])

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


class FinalityTests(unittest.TestCase):
    def test_a_supermajority_advance_is_safe(self):
        report, code = evaluate_tool.evaluate(record([finality()], verdict="pass"))
        self.assertEqual((code, report["reading"]), (0, "safe"))

    def test_a_live_denominator_makes_denominator_ok_false(self):
        row = finality(quorum_total=100, supporting=100)
        report, code = evaluate_tool.evaluate(record([row]))
        self.assertEqual(code, 1)
        self.assertIn("denominator_ok", report["results"][0]["false"])

    def test_an_advance_without_support_makes_support_ok_false(self):
        row = finality(supporting=200)
        report, code = evaluate_tool.evaluate(record([row]))
        self.assertEqual(code, 1)
        self.assertIn("support_ok", report["results"][0]["false"])

    def test_an_unbonded_partition_member_makes_partition_ok_false(self):
        row = finality(partition=["zz"])
        report, code = evaluate_tool.evaluate(record([row]))
        self.assertEqual(code, 1)
        self.assertIn("partition_ok", report["results"][0]["false"])


class StateTests(unittest.TestCase):
    def test_an_accepted_matching_state_is_safe(self):
        report, code = evaluate_tool.evaluate(record([state()], verdict="pass"))
        self.assertEqual((code, report["reading"]), (0, "safe"))

    def test_an_accepted_pre_state_mismatch_is_unsafe(self):
        row = state(computed_pre="ff")
        report, code = evaluate_tool.evaluate(record([row]))
        self.assertEqual(code, 1)
        self.assertIn("pre_ok", report["results"][0]["false"])

    def test_a_refused_mismatch_is_a_safe_refusal(self):
        row = state(computed_pre="ff", verdict="pre-mismatch")
        report, code = evaluate_tool.evaluate(record([row], verdict="pass"))
        self.assertEqual(code, 0)
        self.assertEqual(report["results"][0]["false"], [])

    def test_two_computed_triples_for_one_block_are_an_agreement_failure(self):
        rows = [state(validator=0), state(validator=1, computed_post="ee", replayed_post="ee",
                                            claimed_post="ee")]
        report, code = evaluate_tool.evaluate(record(rows))
        self.assertEqual(code, 1)
        self.assertEqual(report["reading"], "unsafe-point")
        self.assertEqual(len(report["state_disagreement"]), 1)


class BootTests(unittest.TestCase):
    def test_a_certified_boot_is_safe(self):
        report, code = evaluate_tool.evaluate(record([boot()], verdict="pass"))
        self.assertEqual((code, report["reading"]), (0, "safe"))

    def test_a_non_genesis_boot_accepted_without_an_acquisition_is_unsafe(self):
        row = boot(acquisition="none", accepted=True)
        report, code = evaluate_tool.evaluate(record([row]))
        self.assertEqual(code, 1)
        self.assertIn("acquisition_ok", report["results"][0]["false"])

    def test_an_accepted_boot_below_quorum_is_unsafe(self):
        row = boot(served_supporting_stake=200)
        report, code = evaluate_tool.evaluate(record([row]))
        self.assertEqual(code, 1)
        self.assertIn("verification_ok", report["results"][0]["false"])

    def test_a_decision_stage_without_the_anchor_closure_is_unsafe(self):
        row = boot(stage="decision", anchor_closure_present=False, fringe_agrees=True)
        report, code = evaluate_tool.evaluate(record([row]))
        self.assertEqual(code, 1)
        self.assertIn("anchor_ok", report["results"][0]["false"])

    def test_a_refused_decision_names_the_anchor_predicate(self):
        # A quorate certificate that does not anchor the accepted fringe refuses the boot. The
        # decision trace must read as an unsafe point on `anchor_ok`; treating it as a model gap
        # would hide a refused restart behind a trace that says `accepted: true` at acquisition.
        row = boot(stage="decision", accepted=False, fringe_agrees=False,
                   anchor_closure_present=False, refusal_coordinate="fringe-mismatch")
        report, code = evaluate_tool.evaluate(record([row]))
        self.assertEqual(code, 1)
        self.assertEqual(report["reading"], "unsafe-point")
        self.assertIn("anchor_ok", report["results"][0]["false"])

    def test_a_decision_with_an_agreeing_fringe_is_safe(self):
        row = boot(stage="decision", fringe_agrees=True, anchor_closure_present=True)
        report, code = evaluate_tool.evaluate(record([row], verdict="pass"))
        self.assertEqual((code, report["reading"]), (0, "safe"))

    def test_a_legacy_boot_trace_is_still_read(self):
        legacy = {"validator": 0, "acquisition": "certificate", "accepted": True,
                  "dag_len": 4, "store_head": None, "source_configured": True}
        report, code = evaluate_tool.evaluate(record([legacy], verdict="pass"))
        self.assertEqual(report["traces"], 1)
        self.assertEqual(code, 0)


class AdmissionTests(unittest.TestCase):
    def test_a_429_with_a_retry_signal_is_safe(self):
        report, code = evaluate_tool.evaluate(record([admission()], verdict="pass"))
        self.assertEqual((code, report["reading"]), (0, "safe"))

    def test_a_429_without_a_retry_signal_is_unsafe(self):
        row = admission(retry_after=None)
        report, code = evaluate_tool.evaluate(record([row]))
        self.assertEqual(code, 1)
        self.assertIn("retry_signal_ok", report["results"][0]["false"])

    def test_a_503_is_never_a_bounded_refusal(self):
        row = admission(status=503, retry_after=1, **{"class": "full"})
        report, code = evaluate_tool.evaluate(record([row]))
        self.assertEqual(code, 1)
        self.assertIn("retry_signal_ok", report["results"][0]["false"])

    def test_a_permit_overrun_is_unsafe(self):
        row = admission(inflight=3)
        report, code = evaluate_tool.evaluate(record([row]))
        self.assertEqual(code, 1)
        self.assertIn("permit_ok", report["results"][0]["false"])

    def test_a_pool_overrun_is_unsafe(self):
        row = admission(site="pool-admit", pool_count=9, pool_limit=8)
        report, code = evaluate_tool.evaluate(record([row]))
        self.assertEqual(code, 1)
        self.assertIn("pool_bound_ok", report["results"][0]["false"])


class AuditTests(unittest.TestCase):
    def test_an_anchored_target_with_an_agreeing_replay_is_safe(self):
        report, code = evaluate_tool.evaluate(record([audit()], verdict="pass"))
        self.assertEqual((code, report["reading"]), (0, "safe"))

    def test_a_whole_fringe_target_is_unsafe(self):
        row = audit(target_hashes=["ab" * 32, "cd" * 32])
        report, code = evaluate_tool.evaluate(record([row]))
        self.assertEqual(code, 1)
        self.assertIn("target_shape_ok", report["results"][0]["false"])

    def test_a_disagreeing_replay_is_unsafe(self):
        row = audit(derived_root="00" * 32)
        report, code = evaluate_tool.evaluate(record([row]))
        self.assertEqual(code, 1)
        self.assertIn("replay_agrees_ok", report["results"][0]["false"])

    def test_a_disagreeing_audit_is_a_safe_refusal(self):
        row = audit(verdict="DISAGREE", derived_root="00" * 32)
        report, code = evaluate_tool.evaluate(record([row], verdict="pass"))
        self.assertEqual(code, 0)
        self.assertEqual(report["results"][0]["false"], [])


class FormatTests(unittest.TestCase):
    def test_refusing_a_newer_store_is_safe(self):
        report, code = evaluate_tool.evaluate(record([store_format()], verdict="pass"))
        self.assertEqual((code, report["reading"]), (0, "safe"))

    def test_opening_a_newer_store_is_unsafe(self):
        row = store_format(decision="accept")
        report, code = evaluate_tool.evaluate(record([row]))
        self.assertEqual(code, 1)
        self.assertIn("format_ok", report["results"][0]["false"])

    def test_advancing_an_older_store_is_safe(self):
        row = store_format(on_disk=3, decision="advance")
        report, code = evaluate_tool.evaluate(record([row], verdict="pass"))
        self.assertEqual(code, 0)
        self.assertEqual(report["results"][0]["false"], [])

    def test_a_stamp_that_is_not_durable_is_unsafe(self):
        row = store_format(on_disk=None, decision="stamp", stamped=True, durable=False)
        report, code = evaluate_tool.evaluate(record([row]))
        self.assertEqual(code, 1)
        self.assertIn("stamp_ok", report["results"][0]["false"])


class CliTests(unittest.TestCase):
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
