"""Unit controls for the #447 same-channel application conflict verdict."""

import importlib.util
from pathlib import Path
import sys
import unittest


TOOLS = Path(__file__).resolve().parents[1] / "tools"
sys.path.insert(0, str(TOOLS))
SPEC = importlib.util.spec_from_file_location("n447_conflict", TOOLS / "n447-same-channel.py")
conflict = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(conflict)


class WeightedConflictControls(unittest.TestCase):
    def test_only_selected_writer_has_extra_runtime_arithmetic(self):
        for writer in ("A", "B"):
            a, b = conflict.conflict_terms(writer)
            self.assertEqual(" + 0" in a, writer == "A")
            self.assertEqual(" + 0" in b, writer == "B")
            self.assertIn("old + 1", a)
            self.assertIn("old + 2", b)

    def test_default_terms_and_invalid_writer(self):
        self.assertEqual(conflict.conflict_terms(), (conflict.A, conflict.B))
        with self.assertRaises(ValueError):
            conflict.conflict_terms("unknown")


class FinalizedAgreementControls(unittest.TestCase):
    def views(self):
        return {n: {"blockHash": "final", "preStateHash": "pre",
                    "postStateHash": "post"} for n in conflict.CONTAINERS}

    def test_missing_validator_fails(self):
        with self.assertRaisesRegex(AssertionError, "missing validator"):
            conflict.require_finalized_agreement({}, "merge", {})

    def test_different_finalized_blocks_fail(self):
        views = self.views()
        views[conflict.CONTAINERS[-1]]["blockHash"] = "other"
        with self.assertRaisesRegex(AssertionError, "different finalized"):
            conflict.require_finalized_agreement(views, "merge", {})

    def test_unrelated_finalized_block_fails(self):
        from unittest.mock import patch
        with patch.object(conflict.fork, "merge_state_agreement"), patch.object(
                conflict, "verify_branch_descends_from_seed",
                side_effect=AssertionError("unrelated finalized block")):
            with self.assertRaisesRegex(AssertionError, "unrelated"):
                conflict.require_finalized_agreement(self.views(), "merge", {})

    def test_finalized_state_mismatch_fails(self):
        from unittest.mock import patch
        with patch.object(conflict.fork, "merge_state_agreement"), patch.object(
                conflict, "verify_branch_descends_from_seed"), patch.object(
                conflict, "pinned_application_snapshot", return_value={"cell": [0]}):
            with self.assertRaisesRegex(AssertionError, "application state"):
                conflict.require_finalized_agreement(self.views(), "merge", {"cell": [2]})

    def test_common_finalized_descendant_with_winner_passes(self):
        from unittest.mock import patch
        with patch.object(conflict.fork, "merge_state_agreement"), patch.object(
                conflict, "verify_branch_descends_from_seed") as ancestry, patch.object(
                conflict, "pinned_application_snapshot", return_value={"cell": [2]}):
            self.assertEqual(conflict.require_finalized_agreement(
                self.views(), "merge", {"cell": [2]}), "final")
            self.assertEqual(ancestry.call_count, 4)


class VerdictControls(unittest.TestCase):
    def test_a_survives_b_rejected(self):
        self.assertEqual(conflict.classify_effects(True, False, {"b"}, "a", "b"), "A")

    def test_b_survives_a_rejected(self):
        self.assertEqual(conflict.classify_effects(False, True, {"a"}, "a", "b"), "B")

    def test_both_effects_is_not_a_pass(self):
        with self.assertRaises(AssertionError):
            conflict.classify_effects(True, True, {"a"}, "a", "b")

    def test_no_effect_is_not_a_pass(self):
        with self.assertRaises(AssertionError):
            conflict.classify_effects(False, False, {"a"}, "a", "b")

    def test_missing_rejection_is_not_a_pass(self):
        with self.assertRaises(AssertionError):
            conflict.classify_effects(True, False, set(), "a", "b")

    def test_both_rejected_is_not_a_pass(self):
        with self.assertRaises(AssertionError):
            conflict.classify_effects(False, False, {"a", "b"}, "a", "b")

    def test_opaque_unrelated_rejection_is_not_a_pass(self):
        with self.assertRaises(AssertionError):
            conflict.classify_effects(True, False, {"unknown"}, "a", "b")

    def test_rejection_does_not_match_effect_is_not_a_pass(self):
        with self.assertRaises(AssertionError):
            conflict.classify_effects(True, False, {"a"}, "a", "b")

    def test_raw_pinned_reads_return_all_values_without_consumption(self):
        from unittest.mock import patch
        values = [{"ExprInt": {"data": 0}}, {"ExprInt": {"data": 1}}]
        with patch.object(conflict.fork, "request",
                          return_value={"expr": values,
                                        "block": {"blockHash": "pinned"}}) as call:
            self.assertEqual(conflict.expr("node", "n447_linear_cell", "pinned"),
                             values)
        self.assertEqual(call.call_args.args[1], "/api/v1/data-at-name-by-block-hash")
        self.assertEqual(call.call_args.args[2], {
            "name": {"ExprString": {"data": "n447_linear_cell"}},
            "blockHash": "pinned", "usePreStateHash": False,
        })
        with self.assertRaises(AssertionError):
            conflict.require_single_cell_value(values, "A")

    def test_raw_state_cannot_substitute_another_block(self):
        from unittest.mock import patch
        with patch.object(conflict.fork, "request",
                          return_value={"expr": [{"ExprInt": {"data": 1}}],
                                        "block": {"blockHash": "wrong"}}):
            with self.assertRaisesRegex(AssertionError, "requested block"):
                conflict.expr("node", "n447_linear_cell", "pinned")

    def test_raw_read_rejects_http_error_and_invalid_envelope(self):
        from unittest.mock import patch
        for envelope in ({"error": "block not found"},
                         {"expr": "not-an-array", "block": {"blockHash": "pinned"}},
                         {"expr": [], "block": None}):
            with self.subTest(envelope=envelope):
                with patch.object(conflict.fork, "request", return_value=envelope):
                    with self.assertRaises(AssertionError):
                        conflict.expr("node", "n447_linear_cell", "pinned")

    def test_unexpected_channel_is_not_probed(self):
        with self.assertRaisesRegex(AssertionError, "unexpected application channel"):
            conflict.expr("node", "other", "pinned")

    def test_transient_non_block_inventory_is_not_inclusion(self):
        from unittest.mock import patch
        with patch.object(conflict.fork, "request", return_value=["syncing"]):
            self.assertEqual(
                conflict.find_signed_carrier("node", "signed-term", 0),
                (None, None))

    def test_inclusion_from_verified_block_inventory(self):
        from unittest.mock import patch
        sample = [{"blockHash": "a" * 64, "blockNumber": 1,
                   "deployCount": 1}]
        with patch.object(
            conflict.fork, "request", side_effect=[
                sample, {"deploys": [{"term": "signed-term", "sig": "signature"}]}
            ]
        ):
            block, sig = conflict.find_signed_carrier("node", "signed-term", 0)
        self.assertEqual(block["blockHash"], "a" * 64)
        self.assertEqual(sig, "signature")

    def test_carrier_queries_supported_depth_and_finds_signed_term(self):
        from unittest.mock import patch
        items = [{"blockHash": "h0", "blockNumber": 0, "deployCount": 16},
                 {"blockHash": "h1", "blockNumber": 1, "deployCount": 1}]
        with patch.object(conflict.fork, "request", side_effect=[
            items, {"deploys": [{"term": "signed-cell", "sig": "s1"}]}
        ]) as req:
            block, signature = conflict.find_signed_carrier("n", "signed-cell", 0)
        self.assertEqual(block["blockHash"], "h1")
        self.assertEqual(signature, "s1")
        self.assertEqual(req.call_args_list[0].args[1], "/api/blocks/25")

    def test_block_list_error_does_not_become_no_inclusion(self):
        from unittest.mock import patch
        with patch.object(conflict.fork, "request",
                          return_value={"error": "depth limit is 50"}):
            with self.assertRaisesRegex(AssertionError, "block-list API"):
                conflict.find_signed_carrier("n", "signed-cell", 0)

    def test_winning_update_has_the_expected_linear_cell(self):
        self.assertEqual(conflict.require_single_cell_value(
            [{"ExprInt": {"data": 1}}], "A"), 1)
        self.assertEqual(conflict.require_single_cell_value(
            [{"ExprInt": {"data": 2}}], "B"), 2)

    def test_wrong_or_partial_cell_cannot_pass(self):
        with self.assertRaises(AssertionError):
            conflict.require_single_cell_value([{"ExprInt": {"data": 3}}], "A")
        with self.assertRaises(AssertionError):
            conflict.require_single_cell_value([{"ExprInt": {"data": 1}},
                                                {"ExprInt": {"data": 2}}], "A")
        with self.assertRaises(AssertionError):
            conflict.require_single_cell_value([], "B")

    def test_bool_cannot_masquerade_as_numeric_value(self):
        with self.assertRaises(AssertionError):
            conflict.require_single_cell_value([{"ExprInt": {"data": True}}], "A")

    def test_both_branches_have_provable_seed_ancestry(self):
        dag = {"seed": [], "tipA": ["seed"], "tipB": ["right", "seed"],
               "right": ["seed"]}
        blocks = {h: {"blockHash": h, "justifications": parents}
                  for h, parents in dag.items()}
        self.assertEqual(
            conflict.verify_branch_descends_from_seed(
                blocks.__getitem__, "tipA", "seed"), ["tipA", "seed"])
        self.assertEqual(
            conflict.verify_branch_descends_from_seed(
                blocks.__getitem__, "tipB", "seed"), ["tipB", "seed"])

    def test_height_advance_without_seed_ancestry_is_rejected(self):
        dag = {"tipA": ["unrelated"], "unrelated": []}
        blocks = {h: {"blockHash": h, "justifications": parents}
                  for h, parents in dag.items()}
        with self.assertRaisesRegex(AssertionError, "does not descend"):
            conflict.verify_branch_descends_from_seed(
                blocks.__getitem__, "tipA", "seed")

    def test_forged_ancestry_response_fails_closed(self):
        with self.assertRaisesRegex(AssertionError, "invalid block"):
            conflict.verify_branch_descends_from_seed(
                lambda h: {"blockHash": "wrong", "justifications": ["seed"]},
                "tipA", "seed")

    def test_partition_forks_need_not_be_at_identical_height(self):
        a = {"blockNumber": 3, "blockHash": "fork-A", "preStateHash": "seed"}
        b = {"blockNumber": 5, "blockHash": "fork-B", "preStateHash": "seed"}
        fact = conflict.require_divergent_signed_tips(a, b, 2, "sigA", "sigB")
        self.assertFalse(fact["same_height"])
        self.assertTrue(fact["pre_roots_equal"])

    def test_partition_fork_must_not_claim_duplicate_signed_identity(self):
        a = {"blockNumber": 3, "blockHash": "fork-A"}
        b = {"blockNumber": 4, "blockHash": "fork-B"}
        with self.assertRaises(AssertionError):
            conflict.require_divergent_signed_tips(a, b, 2, "same", "same")

    def test_partition_fork_rejects_no_advance_past_seed(self):
        a = {"blockNumber": 2, "blockHash": "fork-A"}
        b = {"blockNumber": 4, "blockHash": "fork-B"}
        with self.assertRaises(AssertionError):
            conflict.require_divergent_signed_tips(a, b, 2, "A", "B")

    def test_each_signed_branch_persists_its_own_linear_update(self):
        for side, number in (("A", 1), ("B", 2)):
            snapshot = {
                "cell": [{"ExprInt": {"data": number}}],
                "a": [{"ExprString": {"data": "A"}}] if side == "A" else [],
                "b": [{"ExprString": {"data": "B"}}] if side == "B" else [],
            }
            self.assertTrue(conflict.require_branch_tip_state(snapshot, side))

    def test_marker_without_committed_update_fails_before_merge(self):
        snapshot = {"cell": [{"ExprInt": {"data": 0}}],
                    "a": [{"ExprString": {"data": "A"}}], "b": []}
        with self.assertRaisesRegex(AssertionError, "linear cell value"):
            conflict.require_branch_tip_state(snapshot, "A")

    def test_wrong_branch_marker_fails(self):
        snapshot = {"cell": [{"ExprInt": {"data": 1}}],
                    "a": [{"ExprString": {"data": "A"}}],
                    "b": [{"ExprString": {"data": "B"}}]}
        with self.assertRaisesRegex(AssertionError, "competing marker"):
            conflict.require_branch_tip_state(snapshot, "A")

    def test_dag_diagnostic_preserves_direct_parents_and_rejections(self):
        sample = {"blockNumber": 4, "blockHash": "block", "justifications": ["A", "B"],
                  "rejectedDeploys": ["r1"], "postStateHash": "state", "bonds": ["private"]}
        item = conflict.brief_block_evidence(sample, ["A", "B"])
        self.assertEqual(item["fork_tips_directly_justified"], ["A", "B"])
        self.assertEqual(item["rejected_deploys"], ["r1"])
        self.assertNotIn("bonds", item)

    def test_indirect_merge_accepts_both_real_ancestry_paths(self):
        dag = {
            "seed": ["genesis"],
            "tipA": ["seed"],
            "tipB": ["seed"],
            "childA": ["tipA"],
            "childB": ["tipB"],
            "merged": ["childA", "childB"],
            "genesis": [],
        }
        blocks = {
            h: {"blockHash": h, "justifications": parents}
            for h, parents in dag.items()
        }
        paths = conflict.causal_ancestor_paths(blocks.__getitem__,
                                               "merged", {"tipA", "tipB"})
        self.assertEqual(set(paths), {"tipA", "tipB"})
        self.assertEqual(paths["tipA"], ["merged", "childA", "tipA"])
        self.assertEqual(paths["tipB"], ["merged", "childB", "tipB"])

    def test_matching_rejection_alone_cannot_fake_two_tip_merge(self):
        dag = {
            "seed": ["genesis"],
            "tipA": ["seed"], "tipB": ["seed"],
            "onlyA": ["tipA"],
            "genesis": [],
        }
        blocks = {
            h: {"blockHash": h, "justifications": parents}
            for h, parents in dag.items()
        }
        inventory = [
            {"blockHash": "onlyA", "blockNumber": 6,
             "rejectedDeploys": ["sigB"]},
        ]
        candidate, paths, hints = conflict.scan_causal_conflict(
            "observer", 3, {"tipA", "tipB"}, {"sigA", "sigB"},
            fetch_block=blocks.__getitem__, inventory=inventory)
        self.assertIsNone(candidate)
        self.assertEqual(paths, {})
        self.assertEqual(hints[0]["reachable_tips"], ["tipA"])

    def test_indirect_conflict_candidate_requires_one_rejection_and_both_paths(self):
        dag = {
            "seed": [], "tipA": ["seed"], "tipB": ["seed"],
            "left": ["tipA"], "right": ["tipB"],
            "mergeLater": ["left", "right"],
        }
        blocks = {
            h: {"blockHash": h, "justifications": parents}
            for h, parents in dag.items()
        }
        inventory = [
            {"blockHash": "mergeLater", "blockNumber": 20,
             "rejectedDeploys": ["sigB", "unrelated"]},
        ]
        candidate, paths, hints = conflict.scan_causal_conflict(
            "observer", 3, {"tipA", "tipB"}, {"sigA", "sigB"},
            fetch_block=blocks.__getitem__, inventory=inventory)
        self.assertEqual(candidate["blockHash"], "mergeLater")
        self.assertEqual(set(paths), {"tipA", "tipB"})
        self.assertEqual(hints, [])

    def test_both_conflicting_deploys_rejected_is_not_a_candidate(self):
        block = {"blockHash": "both", "blockNumber": 20,
                 "rejectedDeploys": ["sigA", "sigB"]}
        candidate, paths, _ = conflict.scan_causal_conflict(
            "observer", 3, {"tipA", "tipB"}, {"sigA", "sigB"},
            fetch_block=lambda h: {"blockHash": h, "justifications": []},
            inventory=[block])
        self.assertIsNone(candidate)
        self.assertEqual(paths, {})

    def test_malformed_or_substituted_ancestry_cannot_pass(self):
        with self.assertRaisesRegex(AssertionError, "invalid block response"):
            conflict.causal_ancestor_paths(
                lambda h: {"blockHash": "substituted", "justifications": []},
                "merged", {"tipA", "tipB"})
        with self.assertRaisesRegex(AssertionError, "malformed DAG"):
            conflict.causal_ancestor_paths(
                lambda h: {"blockHash": h, "justifications": "tipA"},
                "merged", {"tipA", "tipB"})

    def test_bounded_ancestry_refuses_deep_or_cyclic_work(self):
        dag = {"merged": ["intermediate"],
               "intermediate": ["tipA", "tipB"]}
        blocks = {h: {"blockHash": h, "justifications": parents}
                  for h, parents in dag.items()}
        with self.assertRaisesRegex(AssertionError, "block bound"):
            conflict.causal_ancestor_paths(
                blocks.__getitem__, "merged", {"tipA", "tipB"},
                max_blocks=1)
        cycle = {"x": {"blockHash": "x", "justifications": ["y"]},
                 "y": {"blockHash": "y", "justifications": ["x"]}}
        self.assertEqual(conflict.causal_ancestor_paths(
            cycle.__getitem__, "x", {"tipA", "tipB"}), {})

    def test_duplicate_or_unexpected_marker_cannot_pass(self):
        marker = {"ExprString": {"data": "A"}}
        with self.assertRaisesRegex(AssertionError, "duplicate"):
            conflict.contains_marker([marker, marker], "A")
        with self.assertRaisesRegex(AssertionError, "unexpected"):
            conflict.contains_marker([marker, {"ExprInt": {"data": 99}}], "A")
        with self.assertRaisesRegex(AssertionError, "malformed"):
            conflict.contains_marker([{"ExprString": {}}], "A")

    def test_missing_marker_is_exactly_empty_list(self):
        self.assertFalse(conflict.contains_marker([], "B"))
        with self.assertRaisesRegex(AssertionError, "invalid effect"):
            conflict.contains_marker(None, "B")

    def test_marker_json_shape(self):
        self.assertTrue(conflict.contains_marker([{"ExprString": {"data": "A"}}], "A"))
        with self.assertRaisesRegex(AssertionError, "unexpected"):
            conflict.contains_marker([{"ExprString": {"data": "B"}}], "A")


if __name__ == "__main__":
    unittest.main()
