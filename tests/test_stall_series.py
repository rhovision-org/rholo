"""The stall series' own controls.

Every test here feeds `stall_series.verdict` a complete passing record and one decisive mutation,
so a predicate that stops discriminating fails without a devnet. The record shape is the one the
stall arms write; `tests/release-claim-{2,3,4}.py` carry the same shape as their fixtures.
"""
import copy
import importlib.util
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location("stall_series", ROOT / "tests" / "stall_series.py")
stall = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(stall)

DEPLOY = "ab" * 32
EXPECT = {
    "deviations": {"DE-04", "DE-05", "DE-06"},
    "representation": "protocol-stall",
    "shape": "void",
    "cause": "unsatisfiable-requirement",
    "rung": "R2",
    "control": "no-trigger",
    "deploys": [DEPLOY],
}


def record() -> dict:
    return {
        "schema": 1, "claim_id": "RC-2", "claim_title": "Safety",
        "arm": "stall-denominator", "environment": "local", "locality": "local-devnet",
        "provenance": {"source_sha": "0" * 40, "binary_sha256": "1" * 64},
        "preconditions": [], "verdict": "pass", "reasons": [], "errors": [],
        "observations": {
            "stall_series": [
                {"kind": "stall", "phase": "baseline", "height": 40, "finalized": 38},
                {"kind": "stall", "phase": "trigger", "mechanism": "stop-validators",
                 "exercised": True, "live_stake": 200, "bonded_total": 300,
                 "stopped": [2, 3]},
                {"kind": "stall", "phase": "stall", "height": 44, "finalized": 38,
                 "window_s": 60, "progress_advanced": True, "finality_advanced": False,
                 "progress_expected": True, "finality_expected": False,
                 "settlement_claimed": False},
                {"kind": "stall", "phase": "restore", "action": "restart-validators",
                 "rule": "R2", "height": 46, "finalized": 44,
                 "progress_resumed": True, "finality_resumed": True},
                {"kind": "stall", "phase": "settle", "deploy_status": {DEPLOY: "finalizedSuccess"},
                 "rejected_deploys": [], "surfaces": {}},
            ],
            "stall_episode": {
                "deviations": ["DE-04", "DE-05", "DE-06"],
                "representation": "protocol-stall", "shape": "void",
                "cause": "unsatisfiable-requirement", "rung": "R2", "law": "Law 52b",
            },
            "stall_control": {"kind": "no-trigger", "stall_absent": True},
        },
    }


class SeriesTests(unittest.TestCase):
    def check(self, value, expect=EXPECT):
        return stall.verdict(value, expect)

    def test_a_complete_series_passes(self):
        self.assertEqual(self.check(record()), ("pass", []))

    def test_a_missing_phase_is_refused(self):
        value = record()
        value["observations"]["stall_series"] = [
            row for row in value["observations"]["stall_series"]
            if row["phase"] != "restore"]
        verdict, reasons = self.check(value)
        self.assertEqual(verdict, "fail")
        self.assertTrue(any("missing phase" in reason for reason in reasons), reasons)

    def test_phases_out_of_order_are_refused(self):
        value = record()
        rows = value["observations"]["stall_series"]
        rows[2], rows[3] = rows[3], rows[2]
        verdict, reasons = self.check(value)
        self.assertEqual(verdict, "fail")
        self.assertTrue(any("phases are" in reason for reason in reasons), reasons)

    def test_a_series_without_a_restore_is_refused(self):
        value = record()
        restore = next(row for row in value["observations"]["stall_series"]
                       if row["phase"] == "restore")
        restore["progress_resumed"] = False
        restore["finality_resumed"] = False
        verdict, reasons = self.check(value)
        self.assertEqual(verdict, "fail")
        self.assertTrue(any("restore_ok is false" in reason for reason in reasons), reasons)

    def test_a_false_settlement_during_the_stall_is_refused(self):
        value = record()
        stall_row = next(row for row in value["observations"]["stall_series"]
                         if row["phase"] == "stall")
        stall_row["settlement_claimed"] = True
        verdict, reasons = self.check(value)
        self.assertEqual(verdict, "fail")
        self.assertTrue(any("stall_ok is false" in reason for reason in reasons), reasons)

    def test_a_silent_drop_is_refused(self):
        value = record()
        settle = next(row for row in value["observations"]["stall_series"]
                      if row["phase"] == "settle")
        settle["deploy_status"] = {DEPLOY: "terminal"}
        settle["surfaces"] = {}
        verdict, reasons = self.check(value)
        self.assertEqual(verdict, "fail")
        self.assertTrue(any("no_silent_drop_ok is false" in reason for reason in reasons),
                        reasons)

    def test_a_terminal_unit_with_a_surface_is_not_a_silent_drop(self):
        value = record()
        settle = next(row for row in value["observations"]["stall_series"]
                      if row["phase"] == "settle")
        settle["deploy_status"] = {DEPLOY: "terminal"}
        settle["surfaces"] = {DEPLOY: "deploy-status: finalizedError"}
        expect = copy.deepcopy(EXPECT)
        expect["deploys"] = [DEPLOY]
        self.assertEqual(self.check(value, expect), ("pass", []))

    def test_an_unexercised_trigger_is_not_run_never_pass(self):
        value = record()
        trigger = next(row for row in value["observations"]["stall_series"]
                       if row["phase"] == "trigger")
        trigger["exercised"] = False
        verdict, reasons = self.check(value)
        self.assertEqual(verdict, "not run")
        self.assertTrue(any("did not exercise the trigger" in reason for reason in reasons),
                        reasons)

    def test_an_unknown_episode_wire_is_refused(self):
        value = record()
        value["observations"]["stall_episode"]["representation"] = "lantern"
        verdict, reasons = self.check(value)
        self.assertEqual(verdict, "fail")
        self.assertTrue(any("episode_ok is false" in reason for reason in reasons), reasons)

    def test_a_wrong_deviation_is_refused(self):
        value = record()
        value["observations"]["stall_episode"]["deviations"] = ["DE-04"]
        verdict, reasons = self.check(value)
        self.assertEqual(verdict, "fail")
        self.assertTrue(any("deviations" in reason for reason in reasons), reasons)

    def test_the_no_trigger_control_must_stay_live(self):
        value = record()
        value["observations"]["stall_control"]["stall_absent"] = False
        verdict, reasons = self.check(value)
        self.assertEqual(verdict, "fail")
        self.assertTrue(any("no-trigger control" in reason for reason in reasons), reasons)


class DenominatorTests(unittest.TestCase):
    """The denominator rule, independent of any live run."""

    def test_exactly_two_thirds_is_not_a_strict_supermajority(self):
        self.assertFalse(stall.strict_supermajority(200, 300))
        self.assertFalse(stall.strict_supermajority(400, 600))

    def test_one_more_than_two_thirds_is_strict(self):
        self.assertTrue(stall.strict_supermajority(201, 300))
        self.assertTrue(stall.strict_supermajority(401, 600))

    def test_a_partition_can_leave_neither_side_strict(self):
        live, total = 200, 300
        self.assertFalse(stall.strict_supermajority(live, total))
        self.assertFalse(stall.strict_supermajority(total - live, total))


class EpisodeTests(unittest.TestCase):
    def test_an_episode_survives_its_restore(self):
        value = record()
        value["observations"]["stall_episode"] = {
            "deviations": ["DE-09"], "representation": "protocol-wait",
            "shape": "historic", "cause": "historic-predicate", "rung": "R2",
            "law": "Rchain.traceRefines",
        }
        value["observations"]["stall_control"] = {
            "kind": "never-stalled", "stall_absent": True, "episode_count_equal": True,
        }
        expect = {
            "deviations": {"DE-09"}, "representation": "protocol-wait", "shape": "historic",
            "cause": "historic-predicate", "rung": "R2", "control": "never-stalled",
            "deploys": [DEPLOY],
        }
        self.assertEqual(stall.verdict(value, expect), ("pass", []))

    def test_a_never_stalled_control_that_changed_its_count_is_refused(self):
        value = record()
        value["observations"]["stall_episode"] = {
            "deviations": ["DE-09"], "representation": "protocol-wait",
            "shape": "historic", "cause": "historic-predicate", "rung": "R2",
            "law": "Rchain.traceRefines",
        }
        value["observations"]["stall_control"] = {
            "kind": "never-stalled", "stall_absent": True, "episode_count_equal": False,
        }
        expect = {
            "deviations": {"DE-09"}, "representation": "protocol-wait", "shape": "historic",
            "cause": "historic-predicate", "rung": "R2", "control": "never-stalled",
        }
        verdict, reasons = stall.verdict(value, expect)
        self.assertEqual(verdict, "fail")
        self.assertTrue(any("never-stalled control" in reason for reason in reasons), reasons)


if __name__ == "__main__":
    unittest.main()
