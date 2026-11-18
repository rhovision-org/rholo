"""Hermetic controls for the G520 populated-store acceptance predicate.

The drill itself needs Docker; these tests exercise the predicate, including the
fresh-genesis substitution control that the register's negative control mutates.
"""
from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "populated_store_migration_drill",
    ROOT / "tools" / "populated-store-migration-drill.py")
drill = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(drill)

PRE = {"genesis_hash": "a" * 64, "height": 5, "block_hash": "b" * 64,
       "post_state": "c" * 64, "digest": "d" * 64,
       "anchor": "b" * 64, "anchor_height": 5, "anchor_post_state": "c" * 64}
COPIED = dict(PRE, verified=True)
ROLLBACK = dict(PRE)
ADVANCED = dict(PRE, height=6)
FRESH = {"genesis_hash": "e" * 64, "height": 0, "block_hash": "f" * 64,
         "post_state": "0" * 64, "digest": "1" * 64}


class AcceptancePredicateTests(unittest.TestCase):
    def test_the_populated_copy_rollback_and_fresh_arms_pass(self):
        verdict, reasons = drill.populated_acceptance(
            PRE, COPIED, ROLLBACK, ADVANCED, FRESH)
        self.assertEqual(verdict, "PASS", reasons)

    def test_a_fresh_genesis_substitute_is_refused(self):
        substitute = dict(PRE)
        verdict, reasons = drill.populated_acceptance(
            PRE, COPIED, ROLLBACK, ADVANCED, substitute)
        self.assertEqual(verdict, "FAIL")
        self.assertTrue(any("substituted" in reason for reason in reasons), reasons)

    def test_the_substitution_check_is_the_mutation_target(self):
        original = drill._fresh_is_substitute
        drill._fresh_is_substitute = lambda fresh, pre: False
        try:
            verdict, _ = drill.populated_acceptance(
                PRE, COPIED, ROLLBACK, ADVANCED, dict(PRE))
        finally:
            drill._fresh_is_substitute = original
        self.assertEqual(verdict, "PASS")

    def test_a_changed_genesis_is_inconclusive(self):
        verdict, _ = drill.populated_acceptance(
            PRE, dict(COPIED, genesis_hash="9" * 64), ROLLBACK, ADVANCED, FRESH)
        self.assertEqual(verdict, "INCONCLUSIVE")

    def test_a_changed_post_state_fails(self):
        verdict, _ = drill.populated_acceptance(
            PRE, dict(COPIED, post_state="8" * 64), ROLLBACK, ADVANCED, FRESH)
        self.assertEqual(verdict, "FAIL")

    def test_a_rollback_that_does_not_return_fails(self):
        verdict, _ = drill.populated_acceptance(
            PRE, COPIED, dict(ROLLBACK, height=6), ADVANCED, FRESH)
        self.assertEqual(verdict, "FAIL")

    def test_an_advance_that_does_not_move_fails(self):
        verdict, _ = drill.populated_acceptance(
            PRE, COPIED, ROLLBACK, dict(ADVANCED, height=5), FRESH)
        self.assertEqual(verdict, "FAIL")

    def test_a_replay_divergence_log_fails(self):
        verdict, _ = drill.populated_acceptance(
            PRE, COPIED, ROLLBACK, ADVANCED, FRESH,
            "thread panicked: replay divergence")
        self.assertEqual(verdict, "FAIL")

    def test_a_different_certificate_digest_is_not_a_state_divergence(self):
        # The digest commits to the live fringe (fringe hashes, metadata/history
        # roots), which can move between a running node and a restart at the same
        # anchor; the identity is the anchor/post-state, not the digest.
        verdict, reasons = drill.populated_acceptance(
            PRE, dict(COPIED, digest="9" * 64), ROLLBACK, ADVANCED, FRESH)
        self.assertEqual(verdict, "PASS", reasons)

    def test_a_different_checkpoint_anchor_fails(self):
        verdict, _ = drill.populated_acceptance(
            dict(PRE, anchor="a" * 64, anchor_post_state="b" * 64),
            dict(COPIED, anchor="c" * 64, anchor_post_state="b" * 64),
            ROLLBACK, ADVANCED, FRESH)
        self.assertEqual(verdict, "FAIL")

    def test_a_missing_certificate_verdict_fails(self):
        verdict, _ = drill.populated_acceptance(
            PRE, dict(COPIED, verified=False), ROLLBACK, ADVANCED, FRESH)
        self.assertEqual(verdict, "FAIL")

    def test_verified_reads_the_node_verdict_shapes(self):
        self.assertTrue(drill.verified(True))
        self.assertTrue(drill.verified({"verified": True}))
        self.assertTrue(drill.verified({"valid": True}))
        self.assertFalse(drill.verified(False))
        self.assertFalse(drill.verified({"verified": False}))
        self.assertFalse(drill.verified(None))


if __name__ == "__main__":
    unittest.main()
