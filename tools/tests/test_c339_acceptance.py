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
import json
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "c339_mature_replay.py"
SPEC = importlib.util.spec_from_file_location("c339_mature_replay", SCRIPT)
assert SPEC and SPEC.loader
c339 = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(c339)


def certificate(
    anchor: str,
    post: str,
    height: int,
    root="03" * 32,
    history="04" * 32,
    attestations=True,
) -> dict:
    checkpoint = {
        "shardId": "root",
        "anchor": anchor,
        "anchorHeight": height,
        "anchorPostState": post,
        "fringeHashes": [anchor],
        "parentDigest": None,
        "epoch": 0,
        "metadataRoot": root,
        "historyRoot": history,
    }
    certificate = {"checkpoint": checkpoint, "attestations": []}
    if attestations:
        certificate["attestations"] = [
            {"checkpointDigest": c339.checkpoint_digest_hex(checkpoint)}
        ]
    return certificate


def accepted(**overrides) -> tuple[str, list[str]]:
    joiner_certificate = certificate("01" * 32, "02" * 32, 10)
    fields = {
        "joiner_health": {
            "ready": True,
            "maxObservedHeight": 12,
            "finalizedHeight": 10,
        },
        "bootstrap_finalized": 10,
        "joiner_certificate": joiner_certificate,
        "bootstrap_certificate": joiner_certificate,
        "bootstrap_post_state": "02" * 32,
        "quorum": {"supporting": 3, "total": 3},
        "restart": {
            "ready": True,
            "certified_anchor_height": 10,
            "finalized_height": 10,
        },
        "post_join_progress": {
            "height_advanced": True,
            "finality_advanced": True,
            "proposed_block_hash": "ab" * 32,
            "proposed_block_finalized": True,
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


FIXTURE = (
    Path(__file__).resolve().parents[2]
    / "casper"
    / "tests"
    / "fixtures"
    / "checkpoint-digest-goldens.json"
)


class DigestTests(unittest.TestCase):
    def test_the_python_encoding_matches_the_rust_goldens_for_every_shape(self) -> None:
        """Rust and Python read the same fixture; the four optional shapes and the transposition
        must all agree, including `historyRoot` (the old encoder omitted it)."""
        fixture = json.loads(FIXTURE.read_text())
        seen: set[str] = set()
        for case in fixture["cases"]:
            digest = c339.checkpoint_digest_hex(case["checkpoint"])
            self.assertEqual(digest, case["digest"], case["name"])
            self.assertNotIn(digest, seen, f"{case['name']} repeats a digest")
            seen.add(digest)
        self.assertEqual(len(seen), len(fixture["cases"]))

    def test_the_optional_slots_cannot_be_transposed(self) -> None:
        fixture = json.loads(FIXTURE.read_text())
        digests = {case["name"]: case["digest"] for case in fixture["cases"]}
        self.assertNotEqual(digests["metadata_only"], digests["history_only"])
        self.assertNotEqual(digests["both"], digests["transposed"])


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

    def test_a_missing_proposed_block_identity_is_inconclusive(self) -> None:
        # A height increase without the proposed block's hash cannot prove which block finalized.
        verdict, reasons = accepted(
            post_join_progress={"height_advanced": True, "finality_advanced": True}
        )
        self.assertEqual(verdict, "INCONCLUSIVE")
        self.assertTrue(any("did not name the block" in reason for reason in reasons))

    def test_a_proposed_block_that_did_not_finalize_fails(self) -> None:
        verdict, reasons = accepted(
            post_join_progress={
                "height_advanced": True,
                "finality_advanced": True,
                "proposed_block_hash": "ab" * 32,
                "proposed_block_finalized": False,
            }
        )
        self.assertEqual(verdict, "FAIL")
        self.assertTrue(any("did not finalize" in reason for reason in reasons))

    def test_an_unmeasured_post_join_arm_is_inconclusive(self) -> None:
        verdict, _ = accepted(post_join_progress={"error": "propose refused"})
        self.assertEqual(verdict, "INCONCLUSIVE")

    def test_no_attestation_naming_the_digest_is_inconclusive(self) -> None:
        verdict, reasons = accepted(
            joiner_certificate=certificate(
                "01" * 32, "02" * 32, 10, attestations=False
            )
        )
        self.assertEqual(verdict, "INCONCLUSIVE")
        self.assertTrue(any("attestation" in reason for reason in reasons))

    def test_a_certificate_without_commitments_is_inconclusive(self) -> None:
        verdict, reasons = accepted(
            joiner_certificate=certificate(
                "01" * 32, "02" * 32, 10, root=None, history=None
            )
        )
        self.assertEqual(verdict, "INCONCLUSIVE")
        self.assertTrue(any("commitment" in reason for reason in reasons))

    def test_a_certificate_that_disagrees_with_the_bootstrap_fails(self) -> None:
        other = certificate("01" * 32, "02" * 32, 10, root="05" * 32)
        verdict, reasons = accepted(bootstrap_certificate=other)
        self.assertEqual(verdict, "FAIL")
        self.assertTrue(any("disagrees with the bootstrap" in r for r in reasons))

    def test_a_below_quorum_certificate_fails(self) -> None:
        verdict, reasons = accepted(quorum={"supporting": 2, "total": 3})
        self.assertEqual(verdict, "FAIL")
        self.assertTrue(any("not quorate" in reason for reason in reasons))

    def test_an_absent_quorum_line_is_inconclusive(self) -> None:
        verdict, _ = accepted(quorum=None)
        self.assertEqual(verdict, "INCONCLUSIVE")

    def test_the_propose_response_names_the_block_hash(self) -> None:
        named = "cd" * 32
        self.assertEqual(
            c339._propose_hash({"raw": f"Block {named} added to the DAG"}), named
        )
        self.assertEqual(c339._propose_hash(f"Block ({named.upper()})"), named)
        self.assertIsNone(c339._propose_hash({"raw": "Success!"}))

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
