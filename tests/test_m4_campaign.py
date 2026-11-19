"""The M4 collector: it assembles the campaign record and refuses to fabricate a measurement.

The collector reads only the campaign artefacts, so these tests build synthetic harness,
capacity, root, host-loss, restart and recovery files and run the real CLI. The criterion is then
evaluated on the emitted record: a record the tool writes must be the shape the criterion judges.
"""
from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TOOL = ROOT / "tools" / "m4-campaign.py"


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


fixtures = _load("m4_fixtures", ROOT / "tests" / "gate_fixtures.py")
principles = _load("m4_principles", ROOT / "tools" / "gate-principles.py")
CRITERIA = json.loads((ROOT / "spec/audit/gate-criteria.json").read_text())
M4 = next(entry for entry in CRITERIA["criteria"] if entry["gate_id"] == "G428-M4")
SOURCE = "a" * 40
BINARY = "b" * 64
AGGREGATE = "PASS_BOUNDED_WORKLOAD_PARTITION_REJOIN"
PAIRS = {
    "n4": [(f"v{index}", f"host-{'abcd'[index - 1]}") for index in range(1, 5)],
    "n8": [(f"v{index}", f"host-{'abcd'[(index - 1) // 2]}") for index in range(1, 9)],
}


def _round(cycle: int, victim_index: int, anchor: str = "c" * 64,
           window: int = 300) -> dict:
    return {"cycle": cycle, "victim_index": victim_index,
            "isolated_validator": f"n420-v{victim_index + 1}",
            "healing_anchor": {"hash": anchor, "height": 5000},
            "healing_window_seconds": window, "rejoined": [{"node": 0}],
            "healing_failure_class": None, "epoch_boundary_crossed": True}


def _typed_cycle(cycle: int, victim_index: int, count: int,
                 anchor: str = "c" * 64, window: int = 300) -> dict:
    return {"cycle": cycle, "victim_index": victim_index,
            "anchor": {"hash": anchor, "height": 5000},
            "dwell": {"samples": 4, "stable": True},
            "window": {"seconds": window},
            "validators": [{"node": index, "finalized_anchor": True}
                           for index in range(count)],
            "rejoined": True, "epoch_boundary_crossed": True}


class Campaign(dict):
    """One synthetic campaign: the declaration plus every artefact it names."""

    def __init__(self, root: Path):
        super().__init__({"schema": 1, "campaign": {
            "name": "n4-n8-finality-campaign",
            "locality": "multi-host",
            "bootstrap": "v1",
            "selected": {"source_sha": SOURCE, "binary_sha256": BINARY},
            "hosts": ["host-a", "host-b", "host-c", "host-d"],
            "rerun": {
                "n4": "python3 tests/consensus-partition-rejoin.py --validators 4 --out n4",
                "n8": "python3 tests/consensus-partition-rejoin.py --validators 8 --out n8",
            },
        }, "workloads": {}})
        self.root = root
        for key, pairs in PAIRS.items():
            self.add(key, pairs)

    def write(self, path: Path, document: dict) -> Path:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(document))
        return path

    def add(self, key: str, pairs: list[tuple[str, str]]) -> None:
        count = len(pairs)
        directory = self.root / key
        capacity = fixtures._m4_capacity(pairs)
        approved = "a" * 64 if key == "n4" else "b" * 64
        lost_host = "host-d"
        id_host = dict(pairs)
        survivors = [ident for ident, host in pairs if host != lost_host]
        cycles = 2
        rounds = [_round(index + 1, index + 1) for index in range(cycles)]
        typed = [_typed_cycle(index + 1, index + 1, count) for index in range(cycles)]
        self.write(self.root / f"{key}-capacity.json", {"record": capacity})
        self.write(directory / "campaign.json", {
            "gate_id": "n8-partition-rejoin", "gate_version": 2,
            "source_sha": SOURCE, "binary_sha256": BINARY, "validators": count,
            "aggregate": AGGREGATE,
            "cycles": [{"cycle": row["cycle"], "result": "PASS"} for row in rounds],
        })
        self.write(directory / "partition-result.json", {
            "result": AGGREGATE, "validators": count, "rounds": rounds,
        })
        self.write(directory / "partition-rejoin.json", {
            "schema": 1, "gate_id": "G428-M4", "validators": count, "cycles": typed,
        })
        self.write(directory / "workload-result.json", {
            "result": "pass", "validators": count, "target_height": 5200,
            "final_finalized": [{"node": index, "height": 5200, "hash": "e" * 64}
                                for index in range(count)],
        })
        self.write(self.root / f"{key}-root-before.json", {
            "schema": 1, "gate_id": "G428-M4", "phase": "before",
            "approved_root": approved,
            "per_validator": [{"id": ident, "url": f"https://{host}", "finalized": True}
                              for ident, host in pairs],
        })
        self.write(self.root / f"{key}-root-after.json", {
            "schema": 1, "gate_id": "G428-M4", "phase": "after",
            "approved_root": approved,
            "per_validator": [{"id": ident, "url": f"https://{id_host[ident]}",
                               "finalized": True} for ident in survivors],
        })
        self.write(self.root / f"{key}-host-loss.json", {
            "schema": 1, "gate_id": "G428-M4", "approved_root": approved,
            "lost_hosts": [lost_host], "survivors": survivors,
            "per_validator": [{"id": ident, "url": f"https://{id_host[ident]}",
                               "finalized_anchor": True, "root": approved}
                              for ident in survivors],
            "root_before": approved, "root_after": approved, "retained": True,
        })
        self.write(self.root / f"{key}-restart.json", {
            "container": capacity["validators"][0]["container"],
            "replay_seconds": 240, "replay_height": 5000, "recovered_height": 5200,
            "anchor_still_present": True, "fallback_state": "normal",
            "manual_restart": True,
        })
        self.write(self.root / f"{key}-recovery.json", {
            "arm": "restart", "start_utc": "2026-11-19T10:00:00Z",
            "end_utc": "2026-11-19T10:01:00Z", "same_deploy_finalized": True,
            "accepted_deploys_before": ["d1"], "accepted_deploys_after": ["d1"],
            "final_stages": {"d1": "finalizedSuccess"}, "lost_deploys": [],
            "own_key_halt_observed": False, "self_replay_observed": False,
            "commands": ["systemctl restart rnode"],
        })
        self["workloads"][key] = {
            "dir": key, "capacity": f"{key}-capacity.json",
            "restart": f"{key}-restart.json",
            "root_before": f"{key}-root-before.json",
            "root_after": f"{key}-root-after.json",
            "host_loss": f"{key}-host-loss.json",
            "recovery": f"{key}-recovery.json",
            "approved_root": approved, "nodes": [ident for ident, _ in pairs],
        }

    def declaration(self) -> Path:
        return self.write(self.root / "campaign.json", dict(self))


class CollectorTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        self.campaign = Campaign(self.root)

    def tearDown(self):
        self.directory.cleanup()

    def collect(self, declaration: Path):
        out = self.root / "observations.json"
        result = subprocess.run(
            [sys.executable, str(TOOL), "collect", "--declaration", str(declaration),
             "--out", str(out)],
            capture_output=True, text=True, check=False)
        return result, out

    def test_a_complete_campaign_collects_and_passes_the_criterion(self):
        result, out = self.collect(self.campaign.declaration())
        self.assertEqual(result.returncode, 0, result.stderr)
        verdict = principles.evaluate_entry(M4, json.loads(out.read_text()))
        self.assertEqual(verdict["status"], "PASS", verdict)

    def test_a_missing_restart_is_refused_without_an_output(self):
        (self.root / "n4-restart.json").unlink()
        result, out = self.collect(self.campaign.declaration())
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertFalse(out.exists())

    def test_an_undeclared_restart_container_is_refused(self):
        document = json.loads((self.root / "n4-restart.json").read_text())
        document["container"] = "n420-v99"
        (self.root / "n4-restart.json").write_text(json.dumps(document))
        result, out = self.collect(self.campaign.declaration())
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertFalse(out.exists())

    def test_a_candidate_mismatch_is_refused(self):
        document = json.loads((self.root / "n4" / "campaign.json").read_text())
        document["source_sha"] = "c" * 40
        (self.root / "n4" / "campaign.json").write_text(json.dumps(document))
        result, out = self.collect(self.campaign.declaration())
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertFalse(out.exists())

    def test_a_typed_anchor_disagreeing_with_the_raw_round_is_refused(self):
        document = json.loads((self.root / "n4" / "partition-rejoin.json").read_text())
        document["cycles"][0]["anchor"]["hash"] = "d" * 64
        (self.root / "n4" / "partition-rejoin.json").write_text(json.dumps(document))
        result, out = self.collect(self.campaign.declaration())
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertFalse(out.exists())

    def test_a_host_outside_the_campaign_is_refused(self):
        declaration = self.campaign.declaration()
        payload = json.loads(declaration.read_text())
        payload["campaign"]["hosts"] = ["host-a", "host-b", "host-c"]
        declaration.write_text(json.dumps(payload))
        result, out = self.collect(declaration)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertFalse(out.exists())

    def test_more_lost_hosts_than_the_tolerance_is_refused(self):
        document = json.loads((self.root / "n4-host-loss.json").read_text())
        document["lost_hosts"] = ["host-c", "host-d"]
        (self.root / "n4-host-loss.json").write_text(json.dumps(document))
        result, out = self.collect(self.campaign.declaration())
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertFalse(out.exists())

    def test_a_recovery_that_lost_work_is_refused(self):
        document = json.loads((self.root / "n4-recovery.json").read_text())
        document["lost_deploys"] = ["d2"]
        (self.root / "n4-recovery.json").write_text(json.dumps(document))
        result, out = self.collect(self.campaign.declaration())
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertFalse(out.exists())

    def test_a_spurious_self_replay_is_refused(self):
        document = json.loads((self.root / "n4-recovery.json").read_text())
        document["self_replay_observed"] = True
        (self.root / "n4-recovery.json").write_text(json.dumps(document))
        result, out = self.collect(self.campaign.declaration())
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertFalse(out.exists())

    def test_a_below_tier_capacity_record_fails_the_criterion(self):
        capacity = json.loads((self.root / "n4-capacity.json").read_text())
        capacity["record"]["declared"]["per_validator"]["ram_gib"] = 2
        capacity["record"]["declared"]["max_peak_rss_mb"] = 1500
        (self.root / "n4-capacity.json").write_text(json.dumps(capacity))
        result, out = self.collect(self.campaign.declaration())
        self.assertEqual(result.returncode, 0, result.stderr)
        verdict = principles.evaluate_entry(M4, json.loads(out.read_text()))
        self.assertEqual(verdict["status"], "FAIL", verdict)

    def test_a_fault_injection_capacity_record_is_refused(self):
        capacity = json.loads((self.root / "n4-capacity.json").read_text())
        capacity["record"]["kind"] = "fault-injection"
        (self.root / "n4-capacity.json").write_text(json.dumps(capacity))
        result, out = self.collect(self.campaign.declaration())
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertFalse(out.exists())


class _Handler(BaseHTTPRequestHandler):
    answer = b"true"

    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(self.answer)

    def log_message(self, *args):
        pass


class ObserverTests(unittest.TestCase):
    def setUp(self):
        self.server = HTTPServer(("127.0.0.1", 0), _Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.url = f"http://127.0.0.1:{self.server.server_port}"
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.directory.cleanup()

    def run_tool(self, *arguments):
        return subprocess.run([sys.executable, str(TOOL), *arguments],
                              capture_output=True, text=True, check=False)

    def test_observe_root_writes_the_rows(self):
        out = self.root / "root.json"
        result = self.run_tool(
            "observe-root", "--approved-root", "a" * 64, "--phase", "before",
            "--validator", f"v1={self.url}", "--validator", f"v2={self.url}",
            "--out", str(out))
        self.assertEqual(result.returncode, 0, result.stderr)
        document = json.loads(out.read_text())
        self.assertEqual([row["id"] for row in document["per_validator"]], ["v1", "v2"])

    def test_observe_root_reports_a_false_answer(self):
        _Handler.answer = b"false"
        try:
            out = self.root / "root.json"
            result = self.run_tool(
                "observe-root", "--approved-root", "a" * 64, "--phase", "before",
                "--validator", f"v1={self.url}", "--out", str(out))
        finally:
            _Handler.answer = b"true"
        self.assertEqual(result.returncode, 1, result.stderr)

    def test_observe_host_loss_refuses_a_lost_endpoint_that_answers(self):
        out = self.root / "loss.json"
        result = self.run_tool(
            "observe-host-loss", "--approved-root", "a" * 64,
            "--lost", f"host-d={self.url}", "--survivor", f"v1={self.url}",
            "--out", str(out))
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertFalse(out.exists())

    def test_observe_host_loss_writes_the_survivors(self):
        _Handler.answer = b"true"
        out = self.root / "loss.json"
        result = self.run_tool(
            "observe-host-loss", "--approved-root", "a" * 64,
            "--lost", "host-d=http://127.0.0.1:1",
            "--survivor", f"v1={self.url}", "--out", str(out))
        self.assertEqual(result.returncode, 0, result.stderr)
        document = json.loads(out.read_text())
        self.assertEqual(document["survivors"], ["v1"])


if __name__ == "__main__":
    unittest.main()
