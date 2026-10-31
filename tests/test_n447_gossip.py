"""Offline gossip-verdict regressions for the #447 fork-point harness."""

import importlib.util
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch


TOOLS = Path(__file__).resolve().parents[1] / "tools"
sys.path.insert(0, str(TOOLS))
SPEC = importlib.util.spec_from_file_location("n420_fork_point", TOOLS / "n420-fork-point.py")
fork = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(fork)


class GossipGateTests(unittest.TestCase):
    def setUp(self):
        self.nodes = ["a", "b", "c", "d"]
        self.tips = ["first-tip", "second-tip"]

    def test_both_tips_present_at_all_nodes_can_enter_merge_gate(self):
        with patch.object(fork, "block_info", side_effect=lambda _, h: {"blockHash": h}):
            observed = fork.observe_cross_tip_gossip(self.nodes, self.tips)
        self.assertTrue(fork.all_tips_visible(observed))
        self.assertEqual(sum(sum(v.values()) for v in observed.values()), 8)

    def test_one_missing_gossip_tip_cannot_be_misclassified_as_merge_failure(self):
        def lookup(node, h):
            if node == "d" and h == "second-tip":
                return {"error": "not found"}
            return {"blockHash": h}
        with patch.object(fork, "block_info", side_effect=lookup):
            observed = fork.observe_cross_tip_gossip(self.nodes, self.tips)
        self.assertFalse(fork.all_tips_visible(observed))
        self.assertFalse(observed["d"]["second-tip"])
        self.assertTrue(observed["a"]["second-tip"])

    def test_api_failure_is_not_counted_as_block_visibility(self):
        with patch.object(fork, "block_info", side_effect=ConnectionError("node unreachable")):
            observed = fork.observe_cross_tip_gossip(self.nodes, self.tips)
        self.assertFalse(fork.all_tips_visible(observed))

    def test_unrelated_hash_response_cannot_substitute_for_requested_tip(self):
        with patch.object(fork, "block_info", return_value={"blockHash": "wrong-tip"}):
            self.assertFalse(fork.all_tips_visible(
                fork.observe_cross_tip_gossip(self.nodes, self.tips)
            ))

    def test_transport_probe_requires_completed_http_request(self):
        with patch.object(fork, "run", return_value=SimpleNamespace(returncode=0)):
            self.assertTrue(fork.probe_transport("a", "b"))
        with patch.object(fork, "run", return_value=SimpleNamespace(returncode=22)):
            self.assertFalse(fork.probe_transport("a", "b"))


    def test_all_four_merge_roots_and_block_hash_agree(self):
        block = {
            "blockHash": "merged",
            "preStateHash": "pre",
            "postStateHash": "post",
        }
        views = {node: dict(block) for node in self.nodes}
        self.assertEqual(fork.merge_state_agreement(views, self.nodes, "merged"), ("pre", "post"))

    def test_missing_merge_replica_is_not_consensus_agreement(self):
        block = {"blockHash": "merged", "preStateHash": "pre", "postStateHash": "post"}
        views = {node: dict(block) for node in self.nodes[:3]}
        with self.assertRaisesRegex(AssertionError, "all four"):
            fork.merge_state_agreement(views, self.nodes, "merged")

    def test_disagreeing_post_state_hash_is_rejected(self):
        block = {"blockHash": "merged", "preStateHash": "pre", "postStateHash": "post"}
        views = {node: dict(block) for node in self.nodes}
        views[self.nodes[-1]]["postStateHash"] = "not-post"
        with self.assertRaisesRegex(AssertionError, "disagreement"):
            fork.merge_state_agreement(views, self.nodes, "merged")

    def test_wrong_merge_block_hash_is_rejected(self):
        block = {"blockHash": "merged", "preStateHash": "pre", "postStateHash": "post"}
        views = {node: dict(block) for node in self.nodes}
        views[self.nodes[-1]]["blockHash"] = "alternate"
        with self.assertRaisesRegex(AssertionError, "disagreement"):
            fork.merge_state_agreement(views, self.nodes, "merged")

    def test_missing_state_hash_is_not_a_pass(self):
        block = {"blockHash": "merged", "preStateHash": "pre", "postStateHash": None}
        views = {node: dict(block) for node in self.nodes}
        with self.assertRaisesRegex(AssertionError, "disagreement"):
            fork.merge_state_agreement(views, self.nodes, "merged")


if __name__ == "__main__":
    unittest.main()
