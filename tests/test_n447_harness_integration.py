"""Offline end-to-end failure-path checks for the #447 fork-point driver."""

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
SPEC = importlib.util.spec_from_file_location("fork_447", ROOT / "tools/n420-fork-point.py")
fork = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(fork)

from test_n447_partition import FakeDocker  # noqa: E402


class ForkPointEvidenceTests(unittest.TestCase):
    def run_failed_case(self, gossip_ready):
        backend = FakeDocker()
        with tempfile.TemporaryDirectory() as folder:
            base = {"blockNumber": 5, "blockHash": "base"}
            left = {"blockNumber": 6, "blockHash": "left", "postStateHash": "post-left"}
            right = {"blockNumber": 6, "blockHash": "right", "postStateHash": "post-right"}
            tips = iter([base, left, right])
            def transport(a, b):
                left_side = {fork.CONTAINERS[0], fork.CONTAINERS[1]}
                right_side = {fork.CONTAINERS[2], fork.CONTAINERS[3]}
                return (a in left_side and b in left_side) or (
                    a in right_side and b in right_side
                )
            def observations(containers, hashes):
                return {name: {h: gossip_ready for h in hashes} for name in containers}
            args = [
                "fork-point.py", "--out", folder, "--timeout", "1",
                "--gossip-timeout", "0",
            ]
            with (
                patch.object(sys, "argv", args),
                patch.object(fork, "docker",
                             return_value=SimpleNamespace(returncode=0)),
                patch.object(fork, "DockerBackend", return_value=backend),
                patch.object(fork, "wait_all", return_value=[5, 5, 5, 5]),
                patch.object(fork, "tip", side_effect=tips),
                patch.object(fork, "deploy", return_value=None),
                patch.object(fork, "propose", return_value=None),
                patch.object(fork, "probe_transport", side_effect=transport),
                patch.object(fork, "observe_cross_tip_gossip",
                             side_effect=observations),
                patch.object(fork, "find_merge", return_value=None),
                patch.object(fork, "status", return_value={"peers": 1}),
            ):
                with self.assertRaises(SystemExit) as raised:
                    fork.main()
            evidence = json.loads((Path(folder) / "fork-point.json").read_text())
            self.assertEqual(evidence["result"], "INCOMPLETE")
            self.assertEqual(backend.bridges, {"devnet"})
            self.assertTrue(all(backend.members(node) == {"devnet"}
                                for node in backend.nodes))
            return str(raised.exception), [entry["step"] for entry in evidence["steps"]]

    def test_gossip_absent_reports_its_own_failure_and_restores_topology(self):
        message, events = self.run_failed_case(gossip_ready=False)
        self.assertIn("gossip recovery failed", message)
        self.assertIn("healed_tip_gossip", events)
        self.assertNotIn("merge_not_observed", events)

    def test_gossip_ready_but_no_merge_preserves_distinct_failure(self):
        message, events = self.run_failed_case(gossip_ready=True)
        self.assertIn("no multi-parent merge appeared", message)
        self.assertIn("merge_not_observed", events)
        self.assertIn("healed_tip_gossip", events)


if __name__ == "__main__":
    unittest.main()
