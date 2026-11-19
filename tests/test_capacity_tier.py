"""The M1 collector: it assembles a typed record and refuses to fabricate a measurement.

The collector reads only the arms' recorded JSON, so these tests build synthetic soak/restart
files and run the real CLI. The criterion is then evaluated on the emitted record, which is the
other half of the contract: a record the tool writes must be the shape the criterion judges.
"""
from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TOOL = ROOT / "tools" / "capacity-tier.py"


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


principles = _load("capacity_tier_principles", ROOT / "tools" / "gate-principles.py")
CRITERIA = json.loads((ROOT / "spec/audit/gate-criteria.json").read_text())
M1 = next(entry for entry in CRITERIA["criteria"] if entry["gate_id"] == "G428-M1")
CONTAINERS = [f"n420-validator-{index}" for index in range(1, 5)]


def _node(*, rss: int, cgroup: int | None = 8192, mem_total: int = 32768,
          fallback: str = "normal") -> dict:
    return {
        "rss_mb": rss,
        "mem_total_mb": mem_total,
        "cgroup_limit_mb": cgroup,
        "data_dir_free_mb": 200000,
        "cpu_quota": [200000, 100000],
        "rnode_processes": 1,
        "chain_height": 5200,
        "fallback_state": fallback,
    }


def _soak(*, cgroups=None, mem_totals=None, overrides=None, containers=None) -> dict:
    containers = containers or CONTAINERS
    cgroups = cgroups or {}
    mem_totals = mem_totals or {}
    overrides = overrides or {}
    samples = [
        {"at": float(index), "nodes": {
            container: _node(rss=500 + 50 * index,
                             cgroup=cgroups.get(container, 8192),
                             mem_total=mem_totals.get(container, 32768),
                             fallback="normal" if index == 0 else "throttled")
            for container in containers}}
        for index in range(2)
    ]
    runtime = {
        container: {
            "cmd": ["run", "--bootstrap", "rnode://host-a:40400"],
            "entrypoint": [],
            "env": [],
            "argv": ["rnode", "run", "--bootstrap", "rnode://host-a:40400"],
            "host_cores": 8,
            "override_flags": overrides.get(container, []),
        }
        for container in containers
    }
    facts = {container: {"oom_killed": False, "restarts": 0, "oom_kill": 0}
             for container in containers}
    return {"boundary": "test", "runtime": runtime, "initial": facts,
            "final": json.loads(json.dumps(facts)), "samples": samples}


def _restart() -> dict:
    return {"replay_seconds": 240.0, "replay_height": 5000, "replay_peak_rss_mb": 1200,
            "runtime": {}}


class CollectorTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)

    def tearDown(self):
        self.directory.cleanup()

    def write(self, name: str, document: dict) -> Path:
        path = self.root / name
        path.write_text(json.dumps(document))
        return path

    def declaration(self) -> Path:
        return self.write("tier.json", {
            "schema": 1,
            "tier": "minimal",
            "per_validator": {"cores": 2, "ram_gib": 8, "disk_gib": 100},
            "max_peak_rss_mb": 6000,
            "max_replay_seconds": 600,
            "served_height": 5000,
            "host_failure_tolerance": 1,
            "validators": [
                {"id": f"v{index}", "host": f"host-{'abcd'[index - 1]}",
                 "container": CONTAINERS[index - 1], "stake": 100,
                 "soak": "soak.json", "restart": f"restart-{index}.json"}
                for index in range(1, 5)
            ],
        })

    def standard_run(self, **soak_options) -> Path:
        self.write("soak.json", _soak(**soak_options))
        for index in range(1, 5):
            self.write(f"restart-{index}.json", _restart())
        return self.declaration()

    def collect(self, declaration: Path):
        out = self.root / "observations.json"
        result = subprocess.run(
            [sys.executable, str(TOOL), "collect", "--declaration", str(declaration),
             "--out", str(out)],
            capture_output=True, text=True, check=False)
        return result, out

    def test_a_supported_tier_run_collects_and_passes_the_criterion(self):
        result, out = self.collect(self.standard_run())
        self.assertEqual(result.returncode, 0, result.stderr)
        document = json.loads(out.read_text())
        self.assertEqual(document["record"]["kind"], "supported-tier")
        verdict = principles.evaluate_entry(M1, document)
        self.assertEqual(verdict["status"], "PASS", verdict)

    def test_a_below_tier_run_is_classified_as_fault_injection(self):
        result, out = self.collect(self.standard_run(
            cgroups={container: 1024 for container in CONTAINERS}))
        self.assertEqual(result.returncode, 0, result.stderr)
        document = json.loads(out.read_text())
        self.assertEqual(document["record"]["kind"], "fault-injection")
        self.assertEqual(principles.evaluate_entry(M1, document)["status"], "PRECONDITION")

    def test_an_override_run_is_classified_as_fault_injection(self):
        result, out = self.collect(self.standard_run(
            overrides={CONTAINERS[0]: ["--allow-under-minimum"]}))
        self.assertEqual(result.returncode, 0, result.stderr)
        document = json.loads(out.read_text())
        self.assertEqual(document["record"]["kind"], "fault-injection")
        self.assertEqual(principles.evaluate_entry(M1, document)["status"], "PRECONDITION")

    def test_a_missing_measurement_is_refused_without_an_output(self):
        document = _soak()
        del document["samples"][0]["nodes"][CONTAINERS[0]]["rss_mb"]
        self.write("soak.json", document)
        for index in range(1, 5):
            self.write(f"restart-{index}.json", _restart())
        result, out = self.collect(self.declaration())
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertFalse(out.exists())

    def test_a_missing_restart_is_refused_without_an_output(self):
        self.write("soak.json", _soak())
        for index in range(1, 4):
            self.write(f"restart-{index}.json", _restart())
        result, out = self.collect(self.declaration())
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertFalse(out.exists())

    def test_an_undeclared_container_is_refused_without_an_output(self):
        result, out = self.collect(self.standard_run(
            containers=CONTAINERS + ["n420-validator-5"]))
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertFalse(out.exists())

    def test_four_validators_on_two_hosts_fail_the_quorum_rule(self):
        self.write("soak.json", _soak())
        for index in range(1, 5):
            self.write(f"restart-{index}.json", _restart())
        declaration = self.declaration()
        payload = json.loads(declaration.read_text())
        payload["validators"][1]["host"] = "host-a"
        declaration.write_text(json.dumps(payload))
        result, out = self.collect(declaration)
        self.assertEqual(result.returncode, 0, result.stderr)
        verdict = principles.evaluate_entry(M1, json.loads(out.read_text()))
        self.assertEqual(verdict["status"], "FAIL", verdict)
        self.assertTrue(any("supermajority" in reason for reason in verdict["reasons"]), verdict)

    def test_two_validators_without_a_cgroup_share_fail(self):
        # host-a runs v1 and v2 with no cgroup limit and 8 GiB total: the documented share is
        # 16 GiB for two validators, so the record cannot pass on the shared MemTotal.
        shared = {CONTAINERS[0]: None, CONTAINERS[1]: None}
        totals = {CONTAINERS[0]: 8192, CONTAINERS[1]: 8192}
        self.write("soak.json", _soak(cgroups=shared, mem_totals=totals))
        for index in range(1, 5):
            self.write(f"restart-{index}.json", _restart())
        declaration = self.declaration()
        payload = json.loads(declaration.read_text())
        payload["validators"][1]["host"] = "host-a"
        declaration.write_text(json.dumps(payload))
        result, out = self.collect(declaration)
        self.assertEqual(result.returncode, 0, result.stderr)
        verdict = principles.evaluate_entry(M1, json.loads(out.read_text()))
        self.assertEqual(verdict["status"], "FAIL", verdict)
        self.assertTrue(any("cgroup" in reason for reason in verdict["reasons"]), verdict)


if __name__ == "__main__":
    unittest.main()
