"""Isolated controls for the #457 mature-join arm's evidence extraction."""
import importlib.util
from pathlib import Path
import unittest

SPEC = importlib.util.spec_from_file_location(
    "n457_mature_join", Path(__file__).parents[1] / "tools/n457-mature-join-arm.py"
)
module = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(module)

FIXTURE_SPEC = importlib.util.spec_from_file_location(
    "gate_fixtures", Path(__file__).parents[1] / "tests/gate_fixtures.py"
)
fixtures = importlib.util.module_from_spec(FIXTURE_SPEC)
FIXTURE_SPEC.loader.exec_module(fixtures)


class MatureJoinEvidence(unittest.TestCase):
    def test_build_join_chain_correlates_the_two_logs(self):
        stages = fixtures.m3_chain()["stages"]
        joiner = "\n".join(
            [
                stages["1_request"]["log_line"],
                stages["5_receive"]["log_line"],
                stages["6_apply"]["walk_log_line"],
                stages["6_apply"]["log_line"],
            ]
        )
        responder = "\n".join(
            [
                stages["2_receipt"]["log_line"],
                stages["3_construction"]["log_line"],
                stages["4_transport"]["log_line"],
            ]
        )
        chain = module.build_join_chain(joiner, responder)
        self.assertTrue(chain["complete"], chain["incomplete_reasons"])
        self.assertEqual(module.chain_tool().check_chain(chain), [])
        pinned = module.build_join_chain(
            joiner, responder,
            joiner_source="raw/passive.log", responder_source="raw/responder.log",
        )
        self.assertEqual(pinned["sources"], {
            "joiner_log": "raw/passive.log", "responder_log": "raw/responder.log",
        })
        self.assertEqual(module.chain_tool().check_chain(pinned), [])


    def test_two_digit_sequences_stay_whole(self):
        # The 2026-11-13 defect: one capture group unpacked into `(_, seq)` splits "15" into "1"
        # and "5", so the arm witnessed sequence 5 for a block that used 15.
        log = (
            "INFO proposed and added block #16 (seq 15)\n"
            "INFO proposed and added block #18 (seq 16)\n"
        )
        self.assertEqual(module.proposal_sequences(log), [(16, 15), (18, 16)])

    def test_a_log_without_a_proposal_yields_nothing(self):
        self.assertEqual(
            module.proposal_sequences("LFS state is successfully restored."), []
        )

    def test_a_refusal_line_is_not_a_proposal(self):
        log = (
            "ERROR Block #14 852e11dc faulted at dag insert (equivocation detected)\n"
            "INFO proposed and added block #14 (seq 13)\n"
        )
        self.assertEqual(module.proposal_sequences(log), [(14, 13)])

    def test_only_production_continuing_past_the_settle_window_is_late(self):
        log = (
            "2026-11-13T16:48:52.135Z ERROR this validator's key equivocated: block #18 "
            "(sequence 16) was refused\n"
            # 107 ms later: the proposal already in flight, not production continuing.
            "2026-11-13T16:48:52.242Z INFO proposed and added block #18 (seq 17)\n"
            "2026-11-13T16:48:57.000Z ERROR propose halted: this validator's key equivocated\n"
            # 17.9 s after the halt: the gate did not hold.
            "2026-11-13T16:49:10.000Z INFO proposed and added block #19 (seq 18)\n"
        )
        late = module.late_proposals_after_halt(log)
        self.assertEqual(len(late), 1)
        self.assertIn("seq 18", late[0])
        self.assertEqual(module.late_proposals_after_halt("no halt here", settle_seconds=10), [])

    def test_one_offence_is_counted_once_despite_the_quotes(self):
        # The 2026-11-13 defect: the halt sentence and the proposer status both quote the DAG's
        # reason, so counting the substring reported one refusal three times.
        log = (
            "ERROR [casper.blocks.BlockProcessor] this validator's key equivocated: block #18 "
            "(sequence 16) was refused (equivocation detected: sender produced two blocks)\n"
            "ERROR [coop.rchain.node.instances.ProposerInstance] propose halted: this validator's "
            "key equivocated (… equivocation detected: sender produced two blocks)\n"
            "WARN [casper.blocks.BlockProcessor] Block #15 abc faulted at dag insert "
            "(equivocation detected: sender produced two blocks)\n"
        )
        self.assertEqual(len(module.equivocation_refusals(log)), 2)


if __name__ == "__main__":
    unittest.main()
