"""Offline topology/cleanup regressions for the #447 two-by-two network helper."""

import importlib.util
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parents[1] / "tools" / "n447_partition.py"
SPEC = importlib.util.spec_from_file_location("n447_partition", SCRIPT)
partition = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(partition)


class FakeDocker:
    def __init__(self):
        self.nodes = partition.SplitNetwork(None).nodes
        self.networks = {name: {"devnet"} for name in self.nodes}
        self.bridges = {"devnet"}
        self.operations = []
        self.fail_on = None

    def members(self, node):
        return set(self.networks[node])

    def call(self, *args, check=True):
        self.operations.append(args)
        if self.fail_on == args:
            raise RuntimeError("simulated Docker command failure")
        if args[:2] == ("network", "inspect"):
            return SimpleNamespace(returncode=0 if args[2] in self.bridges else 1, stdout="")
        if args[:2] == ("network", "create"):
            if args[2] in self.bridges:
                raise RuntimeError("network exists")
            self.bridges.add(args[2])
        elif args[:2] == ("network", "connect"):
            if args[2] == "--alias":
                bridge, node = args[4], args[5]
            else:
                bridge, node = args[2], args[3]
            if bridge not in self.bridges:
                raise RuntimeError("unknown bridge")
            self.networks[node].add(bridge)
        elif args[:2] == ("network", "disconnect"):
            bridge, node = args[2], args[3]
            if bridge not in self.networks[node]:
                raise RuntimeError("not connected")
            self.networks[node].remove(bridge)
        elif args[:2] == ("network", "rm"):
            bridge = args[2]
            if any(bridge in members for members in self.networks.values()):
                raise RuntimeError("bridge still in use")
            self.bridges.remove(bridge)
        else:
            raise AssertionError(f"unexpected Docker operation: {args}")
        return SimpleNamespace(returncode=0, stdout="")


class PartitionTopologyTests(unittest.TestCase):
    def setUp(self):
        self.docker = FakeDocker()
        self.scope = partition.SplitNetwork(self.docker)

    def assert_restored(self):
        self.assertEqual(self.docker.bridges, {"devnet"})
        for node in self.scope.nodes:
            self.assertEqual(self.docker.members(node), {"devnet"})

    def test_two_groups_have_internal_bridge_and_no_common_cross_bridge(self):
        with self.scope as active:
            view = active.require_partition()
            for node in active.left:
                self.assertIn("devnet", view[node])
            for node in active.right:
                self.assertEqual(set(view[node]), {active.bridge})
            self.assertNotIn("devnet", view[active.right[0]])
            self.assertIn(("network", "connect", "--alias",
                           active.right[0], active.bridge, active.right[0]),
                          self.docker.operations)
        self.assert_restored()

    def test_failed_workload_restores_all_origins(self):
        with self.assertRaisesRegex(ValueError, "application failed"):
            with self.scope:
                raise ValueError("application failed")
        self.assert_restored()

    def test_partial_setup_failure_is_rolled_back(self):
        self.docker.fail_on = ("network", "disconnect", "devnet",
                               self.scope.right[1])
        with self.assertRaisesRegex(RuntimeError, "simulated Docker"):
            with self.scope:
                self.fail("no workload may start before the full partition exists")
        self.assert_restored()

    def test_existing_network_is_not_overwritten_or_removed(self):
        self.docker.bridges.add(self.scope.bridge)
        with self.assertRaisesRegex(RuntimeError, "refusing to reuse"):
            with self.scope:
                self.fail("must fail closed")
        self.assertEqual(self.docker.bridges, {"devnet", self.scope.bridge})
        for node in self.scope.nodes:
            self.assertEqual(self.docker.members(node), {"devnet"})

    def test_cross_side_network_leak_fails_instead_of_claiming_partition(self):
        self.docker.bridges.add("unexpected-shared")
        self.docker.networks[self.scope.left[0]].add("unexpected-shared")
        self.docker.networks[self.scope.right[0]].add("unexpected-shared")
        original = self.scope.snapshot()
        with self.assertRaisesRegex(RuntimeError, "share a Docker network"):
            with self.scope:
                self.fail("cross-side bridge must be rejected")
        self.assertEqual(self.scope.snapshot(), original)
        self.assertEqual(self.docker.bridges, {"devnet", "unexpected-shared"})

    def test_cli_retains_failure_and_cleanup_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory) / "evidence.json"
            with patch.object(partition.subprocess, "run",
                              return_value=SimpleNamespace(returncode=17)):
                result = partition.main(["--out", str(out), "--", "false"],
                                        backend=self.docker)
            self.assertEqual(result, 17)
            record = json.loads(out.read_text())
            self.assertTrue(record["topology_recovered"])
            self.assertEqual(record["workload_exit"], 17)
            self.assert_restored()


if __name__ == "__main__":
    unittest.main()
