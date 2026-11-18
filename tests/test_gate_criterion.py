"""Negative controls for the M4 rejoin and M5 merge-accounting criteria."""
import copy
import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "tools/check-gate-criterion.py"
SPEC = importlib.util.spec_from_file_location("gate_criterion", SCRIPT)
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)
PACK_SPEC = importlib.util.spec_from_file_location(
    "evidence_pack", ROOT / "tools/evidence-pack.py")
pack = importlib.util.module_from_spec(PACK_SPEC)
PACK_SPEC.loader.exec_module(pack)
ANCHOR = "a" * 64
BUNDLE = "e" * 64


class CriterionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        run = Path(cls.tmp.name) / "run"
        run.mkdir()
        (run / "source-sha.txt").write_text("a" * 40 + "\n")
        (run / "binary-sha256.txt").write_text("b" * 64 + "\n")
        (run / "result.json").write_text(json.dumps({"result": "PASS"}))
        cls.records = {}
        for gate_id, token in (("G428-M4", ANCHOR), ("G428-M5", BUNDLE)):
            record = Path(cls.tmp.name) / gate_id
            args = SimpleNamespace(
                run_dir=str(run), record=str(record), gate_id=gate_id,
                verdict="PASS", source_sha=None, binary_sha256=None,
                run_url="https://github.com/rhovision-org/rholo/actions/runs/1",
                reference_url=None, locality="local-devnet", redact=[], move=False,
            )
            pack.cmd_pack(args)
            readme = record / "README.md"
            readme.write_text(pack.PLACEHOLDER.sub("authored", readme.read_text())
                              + f"\nCriterion token: {token}\n")
            cls.records[gate_id] = str(record)
        cls.capture = Path(cls.tmp.name) / "capture.json"
        cls.capture.write_text(json.dumps(
            {"complete": True, "preimages": [{"verify_ok": True}]}))

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def base_rejoin(self):
        return {
            "anchor": {"hash": ANCHOR, "height": 100},
            "dwell": {"samples": 3, "stable": True},
            "window": {"seconds": 300},
            "validators": [
                {"id": "B", "finalized_anchor": True},
                {"id": "C", "finalized_anchor": True},
                {"id": "D", "finalized_anchor": True},
            ],
            "evidence": {"record": self.records["G428-M4"], "digest": "d" * 64},
        }

    def base_merge(self):
        return {
            "bundle": {"available": True, "record": str(self.capture), "digest": BUNDLE},
            "final_scope_rejected_chains": 1,
            "rejections_without_a_conflict": 0,
            "unapplied_kept_writes": 0,
            "per_deploy_effects": [
                {"deploy_id": "d1", "verdict": "accepted"},
                {"deploy_id": "d2", "verdict": "final_rejected"},
            ],
            "kept_readback": True,
            "evidence": {"record": self.records["G428-M5"], "digest": "d" * 64},
        }

    def test_valid_records_pass(self):
        self.assertEqual(gate.check_rejoin(self.base_rejoin())[0], "PASS")
        self.assertEqual(gate.check_merge(self.base_merge())[0], "PASS")

    def test_rejoin_anchor_dwell_window_and_validators(self):
        bad = self.base_rejoin()
        bad["dwell"]["samples"] = 2
        self.assertEqual(gate.check_rejoin(bad)[0], "FAIL")
        slow = self.base_rejoin()
        slow["window"]["seconds"] = 100
        self.assertEqual(gate.check_rejoin(slow)[0], "FAIL")
        victim = self.base_rejoin()
        victim["validators"][0]["finalized_anchor"] = False
        self.assertEqual(gate.check_rejoin(victim)[0], "FAIL")

    def test_dwell_and_window_ceiling(self):
        unstable = self.base_rejoin()
        unstable["dwell"]["stable"] = False
        self.assertEqual(gate.check_rejoin(unstable)[0], "FAIL")
        above = self.base_rejoin()
        above["window"]["seconds"] = 700
        self.assertEqual(gate.check_rejoin(above)[0], "FAIL")

    def test_empty_lists_and_negative_counters(self):
        empty = self.base_rejoin()
        empty["validators"] = []
        self.assertEqual(gate.check_rejoin(empty)[0], "FAIL")
        effects = self.base_merge()
        effects["per_deploy_effects"] = []
        self.assertEqual(gate.check_merge(effects)[0], "FAIL")
        negative = self.base_merge()
        negative["final_scope_rejected_chains"] = -1
        self.assertEqual(gate.check_merge(negative)[0], "FAIL")

    def test_rejoin_needs_the_evidence_record(self):
        bad = self.base_rejoin()
        bad.pop("evidence")
        self.assertEqual(gate.check_rejoin(bad)[0], "PRECONDITION")
        wrong = self.base_rejoin()
        wrong["evidence"]["record"] = self.records["G428-M5"]
        self.assertEqual(gate.check_rejoin(wrong)[0], "FAIL")

    def test_merge_unavailable_bundle_is_precondition(self):
        unavailable = self.base_merge()
        unavailable["bundle"]["available"] = False
        self.assertEqual(gate.check_merge(unavailable)[0], "PRECONDITION")

    def test_merge_available_without_capture_fails(self):
        bad = self.base_merge()
        bad["bundle"].pop("record")
        self.assertEqual(gate.check_merge(bad)[0], "FAIL")

    def test_merge_preimage_must_verify(self):
        capture = Path(self.tmp.name) / "unverified.json"
        capture.write_text(json.dumps(
            {"complete": True, "preimages": [{"verify_ok": False}]}))
        bad = self.base_merge()
        bad["bundle"]["record"] = str(capture)
        self.assertEqual(gate.check_merge(bad)[0], "FAIL")

    def test_merge_accounting_and_readback(self):
        lost = self.base_merge()
        lost["unapplied_kept_writes"] = 1
        self.assertEqual(gate.check_merge(lost)[0], "FAIL")
        unexplained = self.base_merge()
        unexplained["rejections_without_a_conflict"] = 1
        self.assertEqual(gate.check_merge(unexplained)[0], "FAIL")
        no_readback = self.base_merge()
        no_readback["kept_readback"] = False
        self.assertEqual(gate.check_merge(no_readback)[0], "FAIL")
        unknown = self.base_merge()
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
