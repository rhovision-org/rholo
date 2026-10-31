"""Isolated controls for the #457 asymmetric reachability diagnostic."""
import importlib.util
from pathlib import Path
import unittest

SPEC = importlib.util.spec_from_file_location(
    "n457_asymmetric", Path(__file__).parents[1] / "tools/n457-asymmetric-inbound.py"
)
module = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(module)


class AsymmetricReachabilityControls(unittest.TestCase):
    def test_filter_targets_joiner_namespace_and_required_ports(self):
        argv = module.block_rule(987, "172.22.0.4", "tcp", 40400)
        self.assertEqual(argv[:8], [
            "sudo", "nsenter", "-t", "987", "-n", "iptables", "-w", "-I"
        ])
        self.assertIn("172.22.0.4", argv)
        self.assertEqual(argv[-2:], ["-j", "DROP"])
        self.assertEqual(len(module.BLOCKED_PORTS), 3)
        self.assertIn(("tcp", 40404), module.BLOCKED_PORTS)
        self.assertIn(("udp", 40404), module.BLOCKED_PORTS)

    def test_negative_pass_requires_blocked_join_and_exact_hash_diagnostic(self):
        log = (
            "LFS state sync failed (attempt 1 of 3): no block finished in "
            "3 consecutive idle rounds. "
            "no block was received from the peers they were requested from"
        )
        self.assertEqual(module.classify_negative(log, 0), [])
        self.assertIn("block timeout omitted per-hash missing-answer attribution",
                      module.classify_negative(log.replace(module.ATTRIBUTION, ""), 0))
        self.assertTrue(module.classify_negative(log, 1))

    def test_metadata_must_precede_bootstrap_reply_cutoff(self):
        self.assertFalse(module.store_items_arrived("Received finalized fringe"))
        self.assertTrue(module.store_items_arrived(
            "Received StoreItems(history: 29, data: 230) from bootstrap"
        ))

    def test_tuple_page_failure_is_not_mislabeled_as_block_hash_attribution(self):
        log = (
            "LFS state sync failed (attempt 1 of 3): "
            "the state-page walk was abandoned: "
            "no tuple-space state page finished in 3 consecutive idle rounds"
        )
        self.assertEqual(module.classify_negative(log, 0), [])
        self.assertTrue(module.classify_negative(log, 1))

    def test_packet_counters_rule_out_vacuous_network_block(self):
        lines = (
            "0 0 DROP tcp -- * * 172.22.0.4 0.0.0.0/0 tcp dpt:40400\n"
            "5 300 DROP tcp -- * * 172.22.0.5 0.0.0.0/0 tcp dpt:40400\n"
            "7 700 ACCEPT tcp -- * * 172.22.0.3 0.0.0.0/0 tcp dpt:40400\n"
        )
        self.assertEqual(module.dropped_packets(lines, ["172.22.0.4", "172.22.0.5"]), 5)
        self.assertEqual(module.dropped_packets(lines, ["172.22.0.8"]), 0)

    def test_running_during_negative_is_fatal(self):
        log = ("Making a transition to Running state. "
               "LFS state sync failed; no block finished; "
               + module.ATTRIBUTION)
        self.assertTrue(any("entered Running" in x
                            for x in module.classify_negative(log, 0)))


if __name__ == "__main__":
    unittest.main()
