"""The public-arm drivers choose the fault and capture the witness.

These are offline tests over a fake four-validator fleet. They prove the partition driver rotates
two victims and fails when the victim does not produce or the partition leaks, and that the
availability driver reads every finalised body and state on every validator and fails on a missing
or unserved block. The pressure driver refuses a local endpoint on a public run before it touches
a unit.
"""
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def _load(name, relative):
    spec = importlib.util.spec_from_file_location(name, ROOT / relative)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


fleet = _load("fleet_run_arms", "tools/fleet-run.py")
common = _load("release_claim_common_arms", "tests/release_claim_common.py")
base = _load("fleet_run_tests", "tests/test_fleet_run.py")
NAMES = base.NAMES


class ArmFleet(base.FakeFleet):
    """A fake fleet that also answers the block and state reads the new actions use."""

    def __init__(self, document):
        super().__init__(document)
        self.partitioned = set()
        self.partition_polls = {}
        self.removal_ok = True
        self.producing = True
        self.latest_bonus = 5
        self.missing_body = None
        self.missing_state = None
        self.blocks = {height: f"{height:064x}" for height in range(1, 41)}

    def _block_hash(self, height):
        return self.blocks.get(height)

    def read_json(self, index, path, *, admin=False, method="GET", body=None, timeout=20):
        name = self.nodes[index]["name"]
        if path.startswith("/api/blocks/") and path != "/api/blocks/0/0":
            _, _, rest = path.partition("/api/blocks/")
            start, _, end = rest.partition("/")
            rows = [{"blockNumber": height, "blockHash": self.blocks[height]}
                    for height in range(int(start), int(end) + 1) if height in self.blocks]
            return rows
        if path.startswith("/api/block/") and path.endswith("/raw"):
            return super().read_raw(index, path, timeout=timeout)
        if path.startswith("/api/block/"):
            block_hash = path.rsplit("/", 1)[-1]
            return {"blockHash": block_hash}
        if path == "/api/v1/status" and name in self.partitioned:
            self.partition_polls[name] = self.partition_polls.get(name, 0) + 1
            return {"networkId": "testnet", "shardId": "/testnet",
                    "latestBlockNumber": self.heights[name] + 1
                    + (self.latest_bonus + self.partition_polls[name]
                       if self.producing else 0)}
        if path == "/health" and name in self.partitioned:
            return {"bootId": self.boot_ids[name], "ready": True,
                    "finalizedHeight": self.heights[name],
                    "proposalHalted": False, "selfValidationRefusal": False}
        if path == "/api/v1/explore-deploy-by-block-hash":
            height = int((body or {}).get("blockHash", "0" * 64), 16)
            if self.missing_state == index:
                return {}
            return {"expr": [{"ExprInt": {"data": height}}]}
        return super().read_json(index, path, admin=admin, method=method, body=body,
                                 timeout=timeout)

    def read_raw(self, index, path, *, timeout=20):
        if (self.missing_body == index and path.endswith("/raw")
                and base.GENESIS not in path):
            return b""
        return super().read_raw(index, path, timeout=timeout)

    def collect_traces(self, index, dest=None, prefixes=("boot", "merge")):
        return []

    def install_partition(self, index, peers, ports, mechanism):
        name = self.nodes[index]["name"]
        self.partitioned.add(name)
        self.partition_polls[name] = 0
        for other in NAMES:
            if other != name:
                self.heights[other] += 5
        return {"node": name, "mechanism": mechanism, "installed": True}

    def remove_partition(self, index, mechanism):
        name = self.nodes[index]["name"]
        self.partitioned.discard(name)
        # The victim catches up to the anchor plus the dwell it produced while isolated.
        self.heights[name] += self.latest_bonus + 2
        return {"node": name, "mechanism": mechanism, "removed": self.removal_ok}


def partition_manifest(**overrides):
    fault = {"kind": "partition", "cycles": 2, "victim_policy": "highest-finalised",
             "p2p_ports": [40400, 40404], "mechanism": "docker-network",
             "dwell_blocks": 2, "dwell_seconds": 6.5, "heal_timeout_s": 30}
    return base.manifest(fault=fault, **overrides)


class PartitionArmTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.out = Path(self.tmp.name)

    def test_two_cycles_rotate_the_victim_and_capture_retention(self):
        document = partition_manifest()
        transport = ArmFleet(document)
        run = fleet.PartitionRun(document, transport, self.out)
        produced = run.run()
        self.assertEqual(produced["harness_result"], fleet.PARTITION_PASS)
        self.assertEqual(len(produced["cycles"]), 2)
        self.assertNotEqual(produced["cycles"][0]["victim_index"],
                            produced["cycles"][1]["victim_index"])
        for cycle in produced["cycles"]:
            self.assertTrue(cycle["producing_victim"])
            self.assertTrue(all(row["finalized_anchor"] for row in cycle["anchor_retention"]))
            self.assertTrue(cycle["partition"]["removed"]["removed"])

    def test_a_non_producing_victim_fails(self):
        document = partition_manifest()
        transport = ArmFleet(document)
        transport.producing = False
        run = fleet.PartitionRun(document, transport, self.out)
        with self.assertRaises(fleet.Unsafe) as raised:
            run.run()
        self.assertIn("not producing", str(raised.exception))
        self.assertIn("failures", run.result["partition"])

    def test_an_unremoved_partition_is_unsafe(self):
        document = partition_manifest()
        transport = ArmFleet(document)
        transport.removal_ok = False
        run = fleet.PartitionRun(document, transport, self.out)
        with self.assertRaises(fleet.Unsafe) as raised:
            run.run()
        self.assertIn("not removed", str(raised.exception))


def availability_manifest(**overrides):
    return base.manifest(availability={"window_blocks": 10, "probe_timeout_s": 5}, **overrides)


class AvailabilityArmTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.out = Path(self.tmp.name)

    def test_ten_finalised_blocks_are_read_on_every_validator(self):
        document = availability_manifest()
        transport = ArmFleet(document)
        produced = fleet.AvailabilityRun(document, transport, self.out).run()
        self.assertEqual(produced["harness_result"], "PASS")
        blocks = produced["availability"]["blocks"]
        self.assertGreaterEqual(len(blocks), 10)
        self.assertTrue(all(len(row["validators"]) == 4 for row in blocks))
        self.assertTrue(all(row["body_present"] and row["state_served"]
                            for block in blocks for row in block["validators"]))
        self.assertEqual(produced["availability"]["not_available"], [])

    def test_a_missing_body_is_unavailable_and_unsafe(self):
        document = availability_manifest()
        transport = ArmFleet(document)
        transport.missing_body = 2
        run = fleet.AvailabilityRun(document, transport, self.out)
        with self.assertRaises(fleet.Unsafe):
            run.run()
        self.assertTrue(run.result["availability"]["not_available"])
        self.assertTrue(any(row["validator"] == 2
                            for row in run.result["availability"]["not_available"]))

    def test_a_missing_state_is_unavailable_and_unsafe(self):
        document = availability_manifest()
        transport = ArmFleet(document)
        transport.missing_state = 1
        run = fleet.AvailabilityRun(document, transport, self.out)
        with self.assertRaises(fleet.Unsafe):
            run.run()
        self.assertTrue(any(row["validator"] == 1
                            for row in run.result["availability"]["not_available"]))

    def test_rc6_availability_without_the_join_witness_fails(self):
        record = common.availability_fixture("RC-6", "Join and audit")
        del record["observations"]["availability"]["join"]
        verdict, reasons = common.check_availability(record)
        self.assertEqual(verdict, "fail")
        self.assertTrue(any("non-producer join witness" in reason for reason in reasons))

    def test_rc6_records_the_non_producer_join_precondition(self):
        document = availability_manifest(
            join={"joiner": {"name": "joiner", "host": "joiner.example", "http": "https://joiner"},
                  "bootstrap_from": NAMES[1], "fresh_store": True, "min_height": 10,
                  "timeout_s": 30})
        artifact = self.out / "join.json"
        artifact.write_text(json.dumps({
            "join_result": "pass", "bootstrap_from": NAMES[1],
            "chain": {"complete": True}, "audit": {"verdict": "AGREE"}}))
        transport = ArmFleet(document)
        produced = fleet.AvailabilityRun(
            document, transport, self.out, join_artifact=str(artifact)).run()
        self.assertEqual(produced["availability"]["join"]["join_result"], "pass")
        self.assertTrue(produced["availability"]["join"]["chain_complete"])
        self.assertEqual(produced["availability"]["join"]["audit_verdict"], "AGREE")


class PressureArmTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.out = Path(self.tmp.name)

    def test_a_public_run_refuses_a_local_endpoint(self):
        document = base.manifest(
            rehearsal=False, transport="ssh-systemd", locality="public-live",
            pressure={"endpoint": "http://127.0.0.1:40403", "host": NAMES[1], "port": 40403,
                      "cap": 2, "probes": 4, "mode_order": ["capped", "unset"],
                      "settle_s": 5},
            nodes=[{**node, "ssh": f"root@{node['host']}",
                    "host": f"node-{index}.example"}
                   for index, node in enumerate(base.manifest()["nodes"])])
        transport = ArmFleet(document)
        run = fleet.PressureRun(document, transport, self.out)
        with self.assertRaises(fleet.Precondition) as raised:
            run.run()
        self.assertIn("non-local endpoint", str(raised.exception))


if __name__ == "__main__":
    unittest.main()
