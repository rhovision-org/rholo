"""Offline falsifiers for issue #522: executable control-flow witnesses."""
import importlib.util
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parent
s = importlib.util.spec_from_file_location("restart_load_harness", ROOT / "consensus-restart-under-load.py")
harness = importlib.util.module_from_spec(s)
s.loader.exec_module(harness)


class RestartLoadHarnessTests(unittest.TestCase):
    def test_restart_schedule_includes_genesis_master(self):
        self.assertEqual([harness.victim_for_cycle(c) for c in range(2)], [1, 0])

    def test_finalized_height_regression_is_not_a_pass(self):
        with tempfile.TemporaryDirectory() as folder:
            events = Path(folder) / "events.jsonl"
            with patch.object(harness, "agreement", return_value=[(9, "a"), (10, "a")]):
                with self.assertRaisesRegex(RuntimeError, "regressed"):
                    harness.observed_finality(2, {0: (10, "a"), 1: (10, "a")}, events)

    def test_same_height_conflicting_hash_is_not_a_pass(self):
        with tempfile.TemporaryDirectory() as folder:
            events = Path(folder) / "events.jsonl"
            with patch.object(harness, "agreement", return_value=[(10, "a"), (10, "b")]):
                with self.assertRaisesRegex(RuntimeError, "conflicting finalized"):
                    harness.observed_finality(2, {}, events)

    def test_deploy_status_waits_for_a_delayed_success(self):
        with tempfile.TemporaryDirectory() as folder:
            events = Path(folder) / "events.jsonl"
            responses = iter([{"NotProcessed": {}}, {"ProcessedWithSuccess": {}}])
            with (
                patch.object(harness, "observed_finality", return_value=[(10, "a")]),
                patch.object(harness.drill, "get", side_effect=lambda *_: next(responses)),
                patch.object(harness.time, "sleep", return_value=None),
            ):
                harness.wait_deploys_resolved(1, [("deploy1", "marker", 0, 0)],
                                              time.monotonic() + 1, events, {})


if __name__ == "__main__":
    unittest.main()
