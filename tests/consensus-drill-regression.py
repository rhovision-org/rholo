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

    def test_status_row_reads_the_contract_tip_not_the_one_past_height(self):
        run = drill.Drill(SimpleNamespace(out=".", validators=1, window=30))
        bodies = {
            "/api/v1/status/operator": {"latestBlockNumber": 4, "pipelineFaultsOutstanding": 0,
                                        "orphanBodies": 0, "proposalHalted": False,
                                        "finalityStalled": False, "unrestorableRecords": 0,
                                        "finalityCountersDurable": True,
                                        "resources": {"cgroupOomKills": 0}},
            "/health": {"maxObservedHeight": 3, "height": 4, "ready": True,
                        "readyReasons": [], "finalizedHeight": 3, "finalizedLag": 0},
        }
        with patch.object(drill, "get", side_effect=lambda i, path, **kw: bodies[path]):
            row = run.status_row(0)
        # `latestBlockNumber` is one past the observed height; the tip is `maxObservedHeight`.
        self.assertEqual(row["latest_block_number"], 4)
        self.assertEqual(row["height"], 4)
        self.assertEqual(row["tip"], 3)
        self.assertEqual(row["finalized_height"], 3)
        self.assertEqual(row["finalized_lag"], 0)

    def test_an_unknown_tip_is_a_named_precondition_not_zero(self):
        run = drill.Drill(SimpleNamespace(out=".", validators=1, window=30))
        operator = {"pipelineFaultsOutstanding": 0, "orphanBodies": 0, "proposalHalted": False,
                    "finalityStalled": False, "unrestorableRecords": 0,
                    "finalityCountersDurable": True, "resources": {"cgroupOomKills": 0}}
        health = {"ready": True, "readyReasons": [], "maxObservedHeight": None, "height": 0}
        with patch.object(drill, "get",
                          side_effect=lambda i, path, **kw: operator if "operator" in path else health):
            with self.assertRaisesRegex(RuntimeError, "FAIL_PRECONDITION_tipUnknown"):
                run.health_precondition("test")

    def test_a_ready_reason_names_the_precondition(self):
        run = drill.Drill(SimpleNamespace(out=".", validators=1, window=30))
        operator = {"pipelineFaultsOutstanding": 0, "orphanBodies": 0, "proposalHalted": False,
                    "finalityStalled": False, "unrestorableRecords": 0,
                    "finalityCountersDurable": True, "resources": {"cgroupOomKills": 0}}
        health = {"ready": False, "readyReasons": ["shard_not_running"],
                  "maxObservedHeight": 3, "height": 4}
        with patch.object(drill, "get",
                          side_effect=lambda i, path, **kw: operator if "operator" in path else health):
            with self.assertRaisesRegex(RuntimeError, "FAIL_PRECONDITION_shards"):
                run.health_precondition("test")

    def _precondition(self, health, operator=None):
        run = drill.Drill(SimpleNamespace(out=".", validators=1, window=30))
        body = {"pipelineFaultsOutstanding": 0, "orphanBodies": 0, "proposalHalted": False,
                "finalityStalled": False, "unrestorableRecords": 0,
                "finalityCountersDurable": True, "resources": {"cgroupOomKills": 0}}
        body.update(operator or {})
        with patch.object(drill, "get",
                          side_effect=lambda i, path, **kw: body if "operator" in path else health):
            return run.health_precondition("test")

    def test_a_tip_that_is_not_height_minus_one_is_a_precondition(self):
        with self.assertRaisesRegex(RuntimeError, "FAIL_PRECONDITION_tipMismatch"):
            self._precondition({"ready": True, "readyReasons": [], "maxObservedHeight": 4,
                                "height": 4, "finalizedHeight": 3, "finalizedLag": 0})

    def test_a_ready_reason_that_contradicts_a_gate_is_a_precondition(self):
        with self.assertRaisesRegex(RuntimeError, "FAIL_PRECONDITION_readyReasonsInconsistent"):
            self._precondition({"ready": False, "readyReasons": ["pipeline_fault_outstanding"],
                                "maxObservedHeight": 3, "height": 4,
                                "finalizedHeight": 3, "finalizedLag": 0})

    def test_a_lag_that_disagrees_with_the_fence_is_a_precondition(self):
        with self.assertRaisesRegex(RuntimeError, "FAIL_PRECONDITION_finalizedLag"):
            self._precondition({"ready": True, "readyReasons": [], "maxObservedHeight": 3,
                                "height": 4, "finalizedHeight": 3, "finalizedLag": 1})

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
