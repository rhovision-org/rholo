"""Negative controls for the M4 rejoin and M5 merge-accounting criteria."""
import copy
import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "tools/check-gate-criterion.py"
SPEC = importlib.util.spec_from_file_location("gate_criterion", SCRIPT)
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)

REJOIN = {
    "anchor": {"hash": "a" * 64, "height": 100},
    "dwell": {"samples": 3, "stable": True},
    "window": {"seconds": 300},
    "validators": [
        {"id": "B", "finalized_anchor": True},
        {"id": "C", "finalized_anchor": True},
        {"id": "D", "finalized_anchor": True},
    ],
}
MERGE = {
    "bundle": {"available": True},
    "final_scope_rejected_chains": 1,
    "rejections_without_a_conflict": 0,
    "unapplied_kept_writes": 0,
    "per_deploy_effects": [
        {"deploy_id": "d1", "verdict": "accepted"},
        {"deploy_id": "d2", "verdict": "final_rejected"},
    ],
    "kept_readback": True,
}


class CriterionTests(unittest.TestCase):
    def test_valid_records_pass(self):
        self.assertEqual(gate.check_rejoin(copy.deepcopy(REJOIN))[0], "PASS")
        self.assertEqual(gate.check_merge(copy.deepcopy(MERGE))[0], "PASS")

    def test_rejoin_anchor_dwell_window_and_validators(self):
        bad = copy.deepcopy(REJOIN)
        bad["dwell"]["samples"] = 2
        self.assertEqual(gate.check_rejoin(bad)[0], "FAIL")
        slow = copy.deepcopy(REJOIN)
        slow["window"]["seconds"] = 100
        self.assertEqual(gate.check_rejoin(slow)[0], "FAIL")
        victim = copy.deepcopy(REJOIN)
        victim["validators"][0]["finalized_anchor"] = False
        self.assertEqual(gate.check_rejoin(victim)[0], "FAIL")

    def test_merge_unavailable_bundle_is_precondition(self):
        unavailable = copy.deepcopy(MERGE)
        unavailable["bundle"]["available"] = False
        self.assertEqual(gate.check_merge(unavailable)[0], "PRECONDITION")

    def test_merge_accounting_and_readback(self):
        lost = copy.deepcopy(MERGE)
        lost["unapplied_kept_writes"] = 1
        self.assertEqual(gate.check_merge(lost)[0], "FAIL")
        unexplained = copy.deepcopy(MERGE)
        unexplained["rejections_without_a_conflict"] = 1
        self.assertEqual(gate.check_merge(unexplained)[0], "FAIL")
        no_readback = copy.deepcopy(MERGE)
        no_readback["kept_readback"] = False
        self.assertEqual(gate.check_merge(no_readback)[0], "FAIL")
        unknown = copy.deepcopy(MERGE)
        unknown["per_deploy_effects"][0]["verdict"] = "maybe"
        self.assertEqual(gate.check_merge(unknown)[0], "FAIL")

    def test_missing_record_is_precondition(self):
        with tempfile.TemporaryDirectory() as tmp:
            result = subprocess.run(
                [sys.executable, str(SCRIPT), "rejoin", "--record", str(Path(tmp) / "absent.json")],
                capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 3)
        self.assertIn("PRECONDITION", result.stdout)


if __name__ == "__main__":
    unittest.main()
