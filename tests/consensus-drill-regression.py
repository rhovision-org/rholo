#!/usr/bin/env python3
"""The execution witness must reject malformed and wrong state evidence."""
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch
from tempfile import TemporaryDirectory
from types import SimpleNamespace

spec = importlib.util.spec_from_file_location("drill", Path(__file__).resolve().parents[1] / "tools/consensus-acceptance-drill.py")
drill = importlib.util.module_from_spec(spec)
spec.loader.exec_module(drill)


class EvidenceTests(unittest.TestCase):
    def test_operator_status_uses_the_admin_port(self):
        with patch.object(drill.fuzz, "http_json", return_value=(200, {})) as read:
            drill.get(2, "/api/v1/status/operator", admin=True)
            self.assertEqual(read.call_args.args[0], "http://127.0.0.1:42405/api/v1/status/operator")

    def test_before_finalization_is_waiting_but_later_loss_fails(self):
        with TemporaryDirectory() as out:
            run = drill.Drill(SimpleNamespace(out=out, validators=1, window=30))
            snapshot = [({}, {"blockInfo": {"blockNumber": 2, "blockHash": "ab" * 32}, "deploys": []})]
            with patch.object(run, "snapshot", return_value=snapshot), patch.object(drill, "probe", return_value={"expr": []}):
                self.assertIsNone(run.finalized_marker(1))
                with self.assertRaises(RuntimeError):
                    run.finalized_marker(1, required=True)

    def test_transient_drive_refusal_does_not_replace_finality_wait(self):
        with TemporaryDirectory() as out:
            run = drill.Drill(SimpleNamespace(out=out, validators=1, window=30))
            import itertools
            ticks = itertools.count()
            next_count = [0]
            def condition():
                next_count[0] += 1
                return None if next_count[0] == 1 else 42
            with patch.object(drill.time, "monotonic", side_effect=lambda: next(ticks)), \
                 patch.object(drill.time, "sleep"), \
                 patch.object(run, "drive", side_effect=RuntimeError("busy")), \
                 patch.object(run, "record") as record:
                self.assertEqual(run.wait("finality", condition), 42)
                self.assertTrue(any("drive_error" in call.kwargs for call in record.call_args_list))

    def test_capture_failure_preserves_the_primary_verdict(self):
        with TemporaryDirectory() as out:
            argv = ["drill", "--validators", "4", "--out", out]
            with patch.dict(drill.os.environ, {"GITHUB_ACTIONS": "true"}), \
                 patch("sys.argv", argv), \
                 patch.object(drill.Drill, "run", side_effect=RuntimeError("primary failure")), \
                 patch.object(drill.causal_trace, "capture", side_effect=RuntimeError("capture failure")):
                with self.assertRaisesRegex(RuntimeError, "primary failure"):
                    drill.main()
            result = __import__("json").loads((Path(out) / "workload-result.json").read_text())
            self.assertEqual(result["reason"], "primary failure")

    def test_original_finalized_snapshot_must_still_contain_the_marker(self):
        with TemporaryDirectory() as out:
            run = drill.Drill(SimpleNamespace(out=out, validators=1, window=30))
            identity = [{"node": 0, "hash": "ab" * 32}]
            with patch.object(drill, "probe", return_value={"expr": [{"ExprInt": {"data": 42}}]}), \
                 patch.object(run, "is_finalized", return_value=True):
                run.fixed_snapshots(identity)
            with patch.object(drill, "probe", return_value={"expr": [{"ExprInt": {"data": 42}}]}), \
                 patch.object(run, "is_finalized", return_value=False):
                with self.assertRaisesRegex(RuntimeError, "no longer reports its original finalized block"):
                    run.fixed_snapshots(identity)
            with patch.object(drill, "probe", return_value={"expr": [{"ExprInt": {"data": 43}}]}):
                with self.assertRaises(RuntimeError):
                    run.fixed_snapshots(identity)

    def test_campaign_refuses_to_touch_a_non_ci_network(self):
        with patch.dict(drill.fuzz.os.environ, {"GITHUB_ACTIONS": "false"}), patch.object(drill.fuzz.subprocess, "run") as run:
            with self.assertRaises(SystemExit):
                drill.fuzz.run_acceptance_campaign()
            run.assert_not_called()

    def test_trace_campaign_also_refuses_a_non_ci_network(self):
        with patch.dict(drill.fuzz.os.environ, {"GITHUB_ACTIONS": "false"}), patch.object(drill.fuzz.subprocess, "run") as run:
            with self.assertRaises(SystemExit):
                drill.fuzz.run_acceptance_campaign(trace=True)
            run.assert_not_called()

    def test_block_identity_is_validated(self):
        self.assertEqual(drill.block_identity({"blockNumber": 10, "blockHash": "ab" * 32}), (10, "ab" * 32))
        for value in [None, [], {}, {"blockNumber": -1, "blockHash": "ab" * 32},
                      {"blockNumber": True, "blockHash": "ab" * 32},
                      {"blockNumber": 10, "blockHash": "zz" * 32},
                      {"blockNumber": 10, "blockHash": "abc"}]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                drill.block_identity(value)

    def test_finalized_endpoint_uses_its_block_info_envelope(self):
        light = {"blockNumber": 10, "blockHash": "ab" * 32}
        self.assertEqual(drill.finalized_identity({"blockInfo": light, "deploys": []}), (10, "ab" * 32))
        for value in [light, {"blockInfo": None}, {"blockInfo": {}}]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                drill.finalized_identity(value)

    def test_only_the_exact_marker_value_passes(self):
        self.assertTrue(drill.marker_present({"expr": [{"ExprInt": {"data": 42}}]}, 42))
        for value in [{"expr": []}, {"expr": [{"ExprInt": {"data": 41}}]},
                      {"expr": [{"ExprString": {"data": "42"}}]},
                      {"expr": [{"ExprInt": {"data": 42}}, {"ExprInt": {"data": 42}}]}]:
            self.assertFalse(drill.marker_present(value, 42))
        for value in [None, [], {}, {"expr": "42"}]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                drill.marker_present(value, 42)


if __name__ == "__main__":
    unittest.main()
