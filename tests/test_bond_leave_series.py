"""The bond-leave campaign's pure half and the returner's refusal.

`series_from` is what turns the gate samples into a DE-06 series. It must emit nothing when the
samples do not carry a progress step with finality held, a later finalised advance, and an exact
two-thirds support refusal — a refusal alone is not a stall. The returner control is the arm's
falsifier: a validator that stops and comes back but does not reach the common head is a fail, not
a pass with a missing coordinate.
"""
import importlib.util
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


campaign = _load("bond_leave_campaign", ROOT / "tools" / "bond-leave-campaign.py")
common = campaign.common


def _samples(*, resume: bool = True):
    rows = [
        {"validator": 0, "height": 10, "finalised": 8, "finalityGate": None},
        {"validator": 0, "height": 11, "finalised": 8,
         "finalityGate": {"reason": "support", "supporting": 200, "total": 300}},
    ]
    if resume:
        rows.append({"validator": 0, "height": 12, "finalised": 9, "finalityGate": None})
    return rows


class SeriesFrom(unittest.TestCase):
    def test_the_exact_two_thirds_refusal_and_the_resume_are_a_series(self) -> None:
        rows = _samples()
        series = campaign.series_from(rows, rows[1], ("deploy", "finalizedSuccess"))
        self.assertEqual([row["phase"] for row in series],
                         ["baseline", "trigger", "stall", "restore", "settle"])
        self.assertEqual(series[1]["live_stake"], 200)
        self.assertEqual(series[1]["bonded_total"], 300)
        self.assertFalse(series[2]["finality_advanced"])
        self.assertTrue(series[2]["progress_advanced"])
        self.assertTrue(series[3]["finality_resumed"])
        self.assertEqual(series[4]["deploy_status"], {"deploy": "finalizedSuccess"})

    def test_no_refusal_is_not_a_series(self) -> None:
        self.assertEqual(campaign.series_from(_samples(), None, ("deploy", "finalizedSuccess")), [])

    def test_a_refusal_with_no_later_advance_is_not_a_series(self) -> None:
        rows = _samples(resume=False)
        self.assertEqual(campaign.series_from(rows, rows[1], ("deploy", "finalizedSuccess")), [])

    def test_a_refusal_that_is_not_two_thirds_is_not_a_series(self) -> None:
        rows = _samples()
        rows[1]["finalityGate"]["supporting"] = 250
        self.assertEqual(campaign.series_from(rows, rows[1], ("deploy", "finalizedSuccess")), [])

    def test_the_progress_step_must_hold_finality(self) -> None:
        rows = _samples()
        rows[0]["finalised"] = 9
        self.assertEqual(campaign.series_from(rows, rows[1], ("deploy", "finalizedSuccess")), [])


class ReturnerControl(unittest.TestCase):
    def test_a_returner_that_does_not_rejoin_is_not_a_pass(self) -> None:
        record = common.returner_fixture("RC-4", "Recovery without loss")
        record["observations"]["returner"]["heads_agree"] = False
        verdict, reasons = common.check_returner(record)
        self.assertEqual(verdict, "fail")
        self.assertTrue(any("same finalised head" in reason for reason in reasons), reasons)


if __name__ == "__main__":
    unittest.main()
