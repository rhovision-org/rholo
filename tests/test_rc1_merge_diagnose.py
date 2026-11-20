"""The RC-1 diagnostic's branch classifier is falsifiable offline.

The live tool is one Docker run; these cases prove each branch is reachable from its own log
signal, so a diagnosis cannot be produced by a default.
"""
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "rc1_merge_diagnose", ROOT / "tools" / "rc1-merge-retention-diagnose.py")
diagnose = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(diagnose)

CLOSE = "ab" * 32 + "02"


class ClassifyTests(unittest.TestCase):
    def test_a_fringe_divergence_is_r1(self):
        out = diagnose.classify(
            ["[fringe-divergence] fringe ...: 2 distinct claim(s) — block 11 (height 10) claims a; block 12 claims b"], [])
        self.assertEqual(out["branch"], "R1")
        self.assertTrue(out["signals"]["fringe_divergence"])

    def test_a_fork_rejection_is_r2(self):
        out = diagnose.classify(
            ["merge: 28 chains in scope (24 kept, 4 rejected); 4 chain(s) rejected as forks: "
             "their block did not see the whole final fringe (Rchain.MergeScope.KeptRule)"], [])
        self.assertEqual(out["branch"], "R2")
        self.assertTrue(out["signals"]["fork_rejected"])

    def test_a_close_block_id_beside_the_user_id_is_r6(self):
        out = diagnose.classify([], ["ab" * 32, CLOSE])
        self.assertEqual(out["branch"], "R6")
        self.assertTrue(out["signals"]["close_block_id"])

    def test_a_final_scope_rejection_alone_is_r3(self):
        out = diagnose.classify(
            ["merge: 2 chain(s) of the final scope rejected; 1 deploy(s) rode on a "
             "final-scope rejected chain (ab)"], [])
        self.assertEqual(out["branch"], "R3")

    def test_a_state_hash_disagreement_is_r4_r5(self):
        out = diagnose.classify(["state-hash disagreement on post-state: block #12"], [])
        self.assertEqual(out["branch"], "R4/R5")

    def test_no_signal_is_undetermined_not_a_default_branch(self):
        out = diagnose.classify(["an ordinary line"], [])
        self.assertEqual(out["branch"], "undetermined")
        self.assertFalse(any(out["signals"].values()))

    def test_fringe_divergence_outranks_a_fork_rejection(self):
        out = diagnose.classify(
            ["[fringe-divergence] fringe ...", "4 chain(s) rejected as forks"], [])
        self.assertEqual(out["branch"], "R1")

    def test_marker_events_split_present_and_absent(self):
        events = [
            {"node": 0, "finalized_marker_at": "aa",
             "response": {"expr": [{"ExprInt": {"data": 42}}],
                          "block": {"blockNumber": 3, "rejectedDeploys": []}}},
            {"node": 0, "finalized_marker_at": "bb",
             "response": {"expr": [], "block": {"blockNumber": 24,
                                                "rejectedDeploys": ["ab", CLOSE]}}},
        ]
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "observations.jsonl"
            path.write_text("\n".join(json.dumps(event) for event in events) + "\n")
            parsed = diagnose.marker_events(path)
        self.assertEqual(parsed["present"][0]["height"], 3)
        self.assertEqual(parsed["absent"][0]["height"], 24)
        self.assertIn(CLOSE, parsed["absent"][0]["rejectedDeploys"])

    def test_the_control_is_not_the_loss_and_the_attribution_is_kept(self):
        events = {
            "present": [{"height": 3, "rejectedDeploys": []}],
            "absent": [
                {"height": 0, "rejectedDeploys": []},
                {"height": 1, "rejectedDeploys": []},
                {"height": 4, "rejectedDeploys": []},
                {"height": 5, "rejectedDeploys": ["ab", CLOSE]},
            ],
        }
        first, attribution = diagnose.select_loss(events)
        self.assertEqual(first["height"], 4)
        self.assertEqual(attribution["height"], 5)
        self.assertIn(CLOSE, attribution["rejectedDeploys"])


if __name__ == "__main__":
    unittest.main()
