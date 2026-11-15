#!/usr/bin/env python3
"""Offline tests for the checkpoint-join acceptance predicate.

The stage-one audit found two benchmark defects that no test could catch: the restore clock was
sampled before the readiness wait (a 20-second delay was recorded as 0), and the verdict passed on
`maxObservedHeight` alone (a witness with another root and `finalizedHeight: 0` passed). These tests
pin the corrected predicate and the clock ordering without Docker or a network.

Run:
  python3 tools/tests/test_c339_acceptance.py
"""

from __future__ import annotations

import datetime
import importlib.util
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "c339_mature_replay.py"
SPEC = importlib.util.spec_from_file_location("c339_mature_replay", SCRIPT)
assert SPEC and SPEC.loader
c339 = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(c339)


def certificate(anchor: str, post: str, height: int, root=None) -> dict:
    return {
        "checkpoint": {
            "shardId": "root",
            "anchor": anchor,
            "anchorHeight": height,
            "anchorPostState": post,
            "fringeHashes": [anchor],
            "parentDigest": None,
            "epoch": 0,
            "metadataRoot": root,
        },
        "attestations": [],
    }


def accepted(**overrides) -> tuple[str, list[str]]:
    fields = {
        "joiner_health": {
            "ready": True,
            "maxObservedHeight": 12,
            "finalizedHeight": 10,
        },
        "bootstrap_finalized": 10,
        "joiner_certificate": certificate("01" * 32, "02" * 32, 10),
        "bootstrap_post_state": "02" * 32,
        "restart": {
            "ready": True,
            "certified_anchor_height": 10,
            "finalized_height": 10,
        },
        "post_join_progress": {
            "height_advanced": True,
            "finality_advanced": True,
        },
        "log_signals": {
            "replayed_none": True,
            "replay_markers": False,
            "active_bonds_error": False,
            "receiver_overflow": False,
        },
    }
    fields.update(overrides)
    return c339.checkpoint_acceptance(**fields)


class RestoreClockTests(unittest.TestCase):
    def test_the_ready_delay_is_inside_the_measured_restore(self) -> None:
        start = datetime.datetime(2026, 11, 15, tzinfo=datetime.timezone.utc)
        calls: list[str] = []
        clock = lambda: start + datetime.timedelta(seconds=20)  # noqa: E731

        def probe() -> None:
            calls.append("probe")

        elapsed = c339.measure_restore(start, probe, clock=clock)
        self.assertEqual(calls, ["probe"], "the probe runs before the end sample")
        self.assertEqual(elapsed, 20.0, "a 20-second readiness delay is 20 seconds, not 0")

    def test_the_elapsed_clock_never_goes_negative(self) -> None:
        start = datetime.datetime(2026, 11, 15, tzinfo=datetime.timezone.utc)
        self.assertEqual(
            c339.restore_elapsed(start, start - datetime.timedelta(seconds=3)), 0.0
        )


class DigestTests(unittest.TestCase):
    def test_the_python_encoding_matches_the_rust_golden_digest(self) -> None:
        checkpoint = {
            "shardId": "root",
            "anchor": "01" * 32,
            "anchorHeight": 42,
            "anchorPostState": "02" * 32,
            "fringeHashes": ["01" * 32],
            "parentDigest": None,
            "epoch": 4,
        }
        self.assertEqual(
            c339.checkpoint_digest_hex(checkpoint),
            "688779f55fb88078a2a5939ca49ee62e726131d3dfbe2fdf4ce6d6c8d0ed803d",
        )


class AcceptanceTests(unittest.TestCase):
    def test_a_matching_certificate_and_progress_passes(self) -> None:
        verdict, reasons = accepted()
        self.assertEqual(verdict, "PASS", reasons)

    def test_height_and_ready_alone_do_not_pass(self) -> None:
        # The audit's boundary witness: the height is reached, but the root is another and the
        # finalized height is zero.
        verdict, reasons = accepted(
            joiner_certificate=certificate("01" * 32, "99" * 32, 10),
            joiner_health={
                "ready": True,
                "maxObservedHeight": 12,
                "finalizedHeight": 0,
            },
        )
        self.assertEqual(verdict, "FAIL")
        self.assertTrue(any("post-state" in reason for reason in reasons))
        self.assertTrue(any("finalized" in reason for reason in reasons))

    def test_a_missing_joiner_certificate_is_inconclusive(self) -> None:
        verdict, _ = accepted(joiner_certificate=None)
        self.assertEqual(verdict, "INCONCLUSIVE")

    def test_an_unreadable_bootstrap_post_state_is_inconclusive(self) -> None:
        verdict, _ = accepted(bootstrap_post_state=None)
        self.assertEqual(verdict, "INCONCLUSIVE")

    def test_an_anchor_below_the_bootstrap_finalized_height_fails(self) -> None:
        verdict, reasons = accepted(
            joiner_certificate=certificate("01" * 32, "02" * 32, 9)
        )
        self.assertEqual(verdict, "FAIL")
        self.assertTrue(any("anchor" in reason for reason in reasons))

    def test_a_missing_post_join_block_fails(self) -> None:
        verdict, reasons = accepted(
            post_join_progress={"height_advanced": False, "finality_advanced": False}
        )
        self.assertEqual(verdict, "FAIL")
        self.assertTrue(any("post-join block" in reason for reason in reasons))

    def test_an_unmeasured_post_join_arm_is_inconclusive(self) -> None:
        verdict, _ = accepted(post_join_progress={"error": "propose refused"})
        self.assertEqual(verdict, "INCONCLUSIVE")

    def test_a_restart_that_loses_the_certified_anchor_fails(self) -> None:
        verdict, reasons = accepted(
            restart={
                "ready": True,
                "certified_anchor_height": 9,
                "finalized_height": 10,
            }
        )
        self.assertEqual(verdict, "FAIL")
        self.assertTrue(any("restart lost the certified anchor" in r for r in reasons))

    def test_a_log_without_replayed_none_fails(self) -> None:
        verdict, reasons = accepted(
            log_signals={
                "replayed_none": False,
                "replay_markers": False,
                "active_bonds_error": False,
                "receiver_overflow": False,
            }
        )
        self.assertEqual(verdict, "FAIL")
        self.assertTrue(any("replayed none" in reason for reason in reasons))

    def test_a_replay_marker_fails(self) -> None:
        verdict, reasons = accepted(
            log_signals={
                "replayed_none": True,
                "replay_markers": True,
                "active_bonds_error": False,
                "receiver_overflow": False,
            }
        )
        self.assertEqual(verdict, "FAIL")
        self.assertTrue(any("replay marker" in reason for reason in reasons))

    def test_the_observers_own_collector_warning_is_not_a_join_failure(self) -> None:
        # A non-validator observer's collector logs "active-bonds leaf not found" while its state
        # is being hydrated. The certified join is proven by the certificate/post-state/finality;
        # the collector's own certification is not part of it.
        verdict, reasons = accepted(
            log_signals={
                "replayed_none": True,
                "replay_markers": False,
                "active_bonds_error": True,
                "receiver_overflow": False,
                "out_of_memory": False,
            }
        )
        self.assertEqual(verdict, "PASS", reasons)

    def test_a_receiver_overflow_fails(self) -> None:
        verdict, reasons = accepted(
            log_signals={
                "replayed_none": True,
                "replay_markers": False,
                "active_bonds_error": False,
                "receiver_overflow": True,
                "out_of_memory": False,
            }
        )
        self.assertEqual(verdict, "FAIL")
        self.assertTrue(any("receiver-overflow" in reason for reason in reasons))


if __name__ == "__main__":
    unittest.main()
