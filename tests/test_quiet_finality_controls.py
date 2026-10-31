"""Offline falsifiers for exact-hash quiet finality acceptance."""
import importlib.util
import json
from pathlib import Path
import unittest
from unittest.mock import patch
import tempfile

path = Path(__file__).resolve().parent / "consensus-quiet-finality.py"
spec = importlib.util.spec_from_file_location("quiet", path)
quiet = importlib.util.module_from_spec(spec)
spec.loader.exec_module(quiet)


class QuietFinalityControls(unittest.TestCase):
    def healthy(self, **overrides):
        h = {"ready": True, "orphanBodies": 0, "pipelineFaults": 0,
             "proposalHalted": False}
        h.update({k: v for k, v in overrides.items() if k in h})
        o = {"unrestorableRecords": overrides.get("unrestorableRecords", 0),
             "resources": {"cgroupOomKills": overrides.get("cgroupOomKills", 0)}}
        return quiet.verified_health(h, o)

    def rows(self):
        return {i: {"height": 30, "hash": "ab" * 32,
                    "healthy": True, "marker_present": True} for i in range(8)}

    def test_exact_quorum_finality_witness(self):
        rows = self.rows()
        self.assertTrue(quiet.agreed_finality(rows, list(range(8)), 27, True))
        self.assertTrue(quiet.agreed_finality({i: rows[i] for i in range(7)}, list(range(7)), 27, True))

    def test_equal_heights_with_different_hash_fail(self):
        rows = {i: self.rows()[i] for i in range(7)}
        rows[6]["hash"] = "cd" * 32
        self.assertFalse(quiet.agreed_finality(rows, list(range(7)), 27, True))

    def test_changed_fringe_representative_requires_retained_finality(self):
        old = {"height": 13, "hash": "ab" * 32}
        for height in [13, 14]:
            row = {"height": height, "hash": "cd" * 32}
            with patch.object(quiet.a, "get", return_value=True) as get:
                quiet.verify_finality_progress(7, old, row)
                get.assert_called_once_with(7, "/api/is-finalized/" + old["hash"])
            self.assertIs(row["previous_finality"]["retained"], True)
            for retained in [False, None, 1]:
                with patch.object(quiet.a, "get", return_value=retained):
                    with self.assertRaisesRegex(AssertionError, "previous finalized block lost"):
                        quiet.verify_finality_progress(7, old, row)

    def test_height_regression_still_fails(self):
        with patch.object(quiet.a, "get") as get:
            with self.assertRaisesRegex(AssertionError, "height regressed"):
                quiet.verify_finality_progress(7, {"height": 14, "hash": "ab" * 32},
                                               {"height": 13, "hash": "cd" * 32})
            get.assert_not_called()

    def test_finality_loss_preserves_the_violating_observation(self):
        old = {"height": 13, "hash": "ab" * 32, "healthy": True}
        new = {"height": 13, "hash": "cd" * 32, "healthy": True}
        with tempfile.TemporaryDirectory() as temp, \
                patch.object(quiet, "sample", side_effect=[old, new]), \
                patch.object(quiet.a, "get", return_value=False), \
                patch.object(quiet.time, "monotonic", return_value=0), \
                patch.object(quiet.time, "sleep"):
            out = Path(temp)
            with self.assertRaisesRegex(AssertionError, "previous finalized block lost"):
                quiet.observe_until(out, "loss", [7], 14, None, None, 240)
            attempts = json.loads((out / "loss-observations.json").read_text())["attempts"]
            self.assertEqual(attempts[-1]["nodes"]["7"]["hash"], new["hash"])
            self.assertIs(attempts[-1]["nodes"]["7"]["previous_finality"]["retained"], False)
            self.assertIn("7", attempts[-1]["errors"])

    def test_stale_height_and_absent_marker_fail(self):
        rows = self.rows()
        self.assertFalse(quiet.agreed_finality(rows, list(range(8)), 31, True))
        rows[5]["marker_present"] = False
        self.assertFalse(quiet.agreed_finality(rows, list(range(8)), 27, True))

    def test_missing_node_and_unhealthy_node_fail(self):
        rows = self.rows()
        del rows[2]
        self.assertFalse(quiet.agreed_finality(rows, list(range(8)), 27, True))
        rows = self.rows()
        rows[0]["healthy"] = False
        self.assertFalse(quiet.agreed_finality(rows, list(range(8)), 27, True))

    def test_stalled_finality_cannot_pass_with_matching_hashes_and_marker(self):
        row = {"height": 10, "hash": "ab" * 32, "healthy": True, "marker_present": True}
        with tempfile.TemporaryDirectory() as temp, \
                patch.object(quiet, "sample", return_value=row), \
                patch.object(quiet.time, "monotonic", side_effect=[0, 0, 0, 241]), \
                patch.object(quiet.time, "sleep"):
            with self.assertRaisesRegex(AssertionError, "within 240s"):
                quiet.observe_until(Path(temp), "stalled", list(range(7)), 11, "marker", 42, 240)

    def test_missing_and_bad_health_fields_fail(self):
        self.assertTrue(self.healthy())
        for key, value in [("ready", False), ("ready", None),
                           ("orphanBodies", None), ("orphanBodies", 1),
                           ("pipelineFaults", 2), ("proposalHalted", True),
                           ("unrestorableRecords", 1),
                           ("unrestorableRecords", None), ("cgroupOomKills", 1),
                           ("cgroupOomKills", None)]:
            with self.subTest(key=key, value=value):
                self.assertFalse(self.healthy(**{key: value}))
        self.assertFalse(quiet.verified_health(
            {"ready": True, "orphanBodies": 0}, {"unrestorableRecords": 0}))


if __name__ == "__main__":
    unittest.main()
