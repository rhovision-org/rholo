"""The fleet driver performs the action; it does not restate an operator's claim.

The fake transport models four validators, a victim that goes down and comes back with a new
process boot id, and a marker that becomes readable after the deploy. The tests prove that the
driver chooses the victim itself, stops and starts exactly that node, captures the new boot id,
records the survivors' advance and the victim's rejoin, and refuses to pass when the restart did
not happen or a validator halts.
"""
import importlib.util
import json
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("fleet_run", ROOT / "tools" / "fleet-run.py")
fleet = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(fleet)

PIN_SPEC = importlib.util.spec_from_file_location(
    "release_pin_probe", ROOT / "tools" / "release-pin-probe.py")
pin_probe = importlib.util.module_from_spec(PIN_SPEC)
PIN_SPEC.loader.exec_module(pin_probe)

SOURCE = "a" * 40
BINARY = "b" * 64
GENESIS = "c" * 64
RAW = b"genesis-bytes"
BONDS = [{"validator": "v1", "stake": 100}, {"validator": "v2", "stake": 100}]
BONDS_DIGEST = pin_probe.canonical_bonds(BONDS)[1]
NAMES = [f"validator-{index}" for index in range(4)]


def manifest(**overrides):
    document = {
        "schema": 1,
        "campaign": "rc-public-fleet",
        "generated_at_utc": "2026-11-22T00:00:00Z",
        "locality": "local-devnet",
        "transport": "docker",
        "rehearsal": True,
        "provenance": {
            "source_sha": SOURCE,
            "binary_sha256": BINARY,
            "genesis_hash": GENESIS,
            "genesis_raw_sha256": fleet.sha256_bytes(RAW),
            "bonds_sha256": BONDS_DIGEST,
            "network_id": "testnet",
            "shard_id": "/testnet",
        },
        "nodes": [
            {"name": name, "host": f"host-{index}", "unit": f"rcfleet-{index}",
             "binary_path": "/usr/local/bin/rnode", "data_dir": "/var/lib/rnode",
             "http": f"http://127.0.0.1:{40403 + index * 1000}",
             "admin": f"http://127.0.0.1:{40405 + index * 1000}",
             "grpc_host": "127.0.0.1", "grpc_port": 40401}
            for index, name in enumerate(NAMES)
        ],
        "deploy": {
            "from": "validator-0", "marker": "rc-public:fixture", "value": 42,
            "term": '@"rc-public:fixture"!(42)',
            "phlo_limit": 1000000, "phlo_price": 1, "shard_id": "/testnet",
            "relay": {"mode": "relay", "aliases": list(NAMES),
                      "reachability_checked_utc": "2026-11-22T00:00:00Z",
                      "unreachable_aliases": []},
        },
        "actions": {
            "survivor_advance_blocks": 3, "rejoin_margin_blocks": 3,
            "epoch_length": 10, "retention_epochs": 2,
            "timeouts": {"preflight_s": 5, "deploy_s": 5, "settle_s": 5, "stop_s": 5,
                         "advance_s": 5, "start_s": 2, "rejoin_s": 5, "trace_s": 5},
        },
    }
    document.update(overrides)
    return document


class FakeFleet(fleet.Transport):
    name = "fake"

    def __init__(self, document):
        super().__init__(document)
        self.heights = {name: 20 for name in NAMES}
        self.boot_ids = {name: f"boot-{name}-0" for name in NAMES}
        self.down = set()
        self.present_markers = set()
        self.stops = []
        self.starts = []
        self.submissions = []
        self.halted = False
        self.new_boot_on_start = True

    @staticmethod
    def _hash(height: int) -> str:
        return f"{height:064x}"

    def _up(self, name):
        if name in self.down:
            raise fleet.Precondition(f"{name} is down")

    def read_json(self, index, path, *, admin=False, method="GET", body=None, timeout=20):
        name = self.nodes[index]["name"]
        self._up(name)
        if path == "/api/v1/status":
            return {"networkId": "testnet", "shardId": "/testnet",
                    "latestBlockNumber": self.heights[name] + 1}
        if path == "/health":
            return {"bootId": self.boot_ids[name], "ready": True,
                    "finalizedHeight": self.heights[name],
                    "proposalHalted": self.halted, "selfValidationRefusal": False}
        if path == "/api/blocks/0/0":
            return [{"blockHash": GENESIS, "bonds": BONDS}]
        if path == "/api/last-finalized-block":
            return {"blockInfo": {"blockNumber": self.heights[name],
                                  "blockHash": self._hash(self.heights[name])}}
        if path.startswith("/api/is-finalized/"):
            try:
                height = int(path.rsplit("/", 1)[1], 16)
            except ValueError:
                return False
            return height <= self.heights[name]
        if path.startswith("/api/v1/deploy-status/"):
            return {"status": "finalizedSuccess" if self.submissions else "pooled"}
        if path == "/api/v1/status/operator":
            return {"checkpoint": {"kind": "checkpoint", "digest": "d" * 64,
                                   "anchorHeight": self.heights[name] - 1}}
        if path == "/api/v1/explore-deploy-by-block-hash":
            term = (body or {}).get("term", "")
            match = re.search(r'@("(?:[^"\\]|\\.)*")', term)
            marker = json.loads(match.group(1)) if match else None
            if marker in self.present_markers:
                return {"expr": [{"ExprInt": {"data": 42}}]}
            return {"expr": []}
        raise AssertionError(f"unexpected read {path}")

    def read_raw(self, index, path, *, timeout=20):
        self._up(self.nodes[index]["name"])
        return RAW

    def binary_sha256(self, index):
        self._up(self.nodes[index]["name"])
        return BINARY

    def binary_version(self, index):
        self._up(self.nodes[index]["name"])
        return f"rnode 0.3.0 ({SOURCE})"

    def host_boot_id(self, index):
        self._up(self.nodes[index]["name"])
        return f"kernel-boot-{index}"

    def unit_state(self, index):
        self._up(self.nodes[index]["name"])
        return {"ActiveState": "active"}

    def submit_deploy(self, index, term, deploy):
        self._up(self.nodes[index]["name"])
        self.submissions.append({"index": index, "term": term})
        match = re.search(r'@("(?:[^"\\]|\\.)*")', term)
        if match:
            self.present_markers.add(json.loads(match.group(1)))
        for name in NAMES:
            if name not in self.down:
                self.heights[name] += 2
        return {"deploy_id": f"{len(self.submissions):064x}", "valid_after_block_number": 20}

    def stop(self, index, timeout):
        name = self.nodes[index]["name"]
        self.down.add(name)
        self.stops.append(name)
        for other in NAMES:
            if other not in self.down:
                self.heights[other] += 5
        return {"node": name, "unit": self.nodes[index]["unit"], "action": "stop",
                "exit_code": 0, "requested_at_utc": fleet.utc_now(), "stopped": True}

    def start(self, index, timeout):
        name = self.nodes[index]["name"]
        self.down.discard(name)
        self.starts.append(name)
        if self.new_boot_on_start and self.halted is False:
            self.boot_ids[name] = f"boot-{name}-1"
        self.heights[name] = max(self.heights.values())
        return {"node": name, "unit": self.nodes[index]["unit"], "action": "start",
                "exit_code": 0, "requested_at_utc": fleet.utc_now(), "active": True}

    def collect_traces(self, index, dest=None):
        name = self.nodes[index]["name"]
        return [{"kind": "boot", "stage": "decision", "validator": index,
                 "acquisition": "certificate", "accepted": True, "genesis_only": False,
                 "outcome": "certified", "dag_len": self.heights[name], "store_head": None,
                 "source_configured": True, "shard_id": "root",
                 "served_supporting_stake": 200, "served_total_stake": 200,
                 "file": "boot-0001.json"}]

    def journal_tail(self, index, lines=80):
        return ""


class FleetRunTests(unittest.TestCase):
    def setUp(self):
        self.document = manifest()
        self.transport = FakeFleet(self.document)
        self.run = fleet.RecoveryRun(self.document, self.transport)

    def test_the_preflight_pins_the_candidate_and_four_hosts(self):
        preflight = fleet.fleet_preflight(self.document, self.transport)
        self.assertTrue(preflight["ok"], preflight["errors"])
        self.assertTrue(preflight["checks"]["distinct_host_labels"])
        self.assertTrue(preflight["checks"]["binary_is_candidate"])
        self.assertFalse(preflight["synthetic_boot_ids"])

    def test_another_source_is_refused(self):
        document = manifest()
        document["provenance"]["source_sha"] = "d" * 40
        preflight = fleet.fleet_preflight(document, self.transport)
        self.assertFalse(preflight["ok"])

    def test_recovery_stops_the_victim_it_chose_and_captures_the_new_boot_id(self):
        produced = self.run.run()
        self.assertEqual(produced["harness_result"], "PASS")
        victim = produced["victim"]
        self.assertEqual(self.transport.stops, [victim])
        self.assertEqual(self.transport.starts, [victim])
        self.assertNotEqual(produced["boot_ids"]["before"][victim],
                            produced["boot_ids"]["after"][victim])
        self.assertEqual(produced["boot_ids"]["source"], "node-process")
        self.assertGreaterEqual(produced["rejoin"]["heights_above_baseline"], 3)
        self.assertTrue(produced["rejoin"]["same_head"])
        self.assertTrue(produced["survivor_deploy"]["resolved"])
        self.assertEqual(produced["regressions"], [])

    def test_the_victim_is_not_the_bootstrap_or_the_deploy_ingress(self):
        produced = self.run.run()
        self.assertNotIn(produced["victim"], ("validator-0",))

    def test_a_restart_without_a_new_boot_id_is_a_precondition(self):
        self.transport.new_boot_on_start = False
        with self.assertRaises(fleet.Precondition):
            self.run.run()
        self.assertEqual(self.run.result["boot_ids"]["after"], {})
        self.assertIsNone(self.run.result["harness_result"])

    def test_a_halt_after_the_restart_is_unsafe(self):
        original_start = self.transport.start

        def halting_start(index, timeout):
            result = original_start(index, timeout)
            self.transport.halted = True
            return result

        self.transport.start = halting_start
        with self.assertRaises(fleet.Unsafe):
            self.run.run()
        self.assertTrue(self.run.result["own_key_halt"])

    def test_the_manifest_contract_accepts_this_rehearsal(self):
        problems = fleet.fleet_manifest.validate(self.document, claim_id="RC-4",
                                                 environment="public", head=SOURCE,
                                                 rehearsal=True)
        self.assertEqual(problems, [])


if __name__ == "__main__":
    unittest.main()
