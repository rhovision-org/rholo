"""The generic collector: config validation and the observation document."""
from __future__ import annotations

import gzip
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("gate_collect", ROOT / "tools" / "gate-collect.py")
collector = importlib.util.module_from_spec(spec)
spec.loader.exec_module(collector)

DEPLOY = "ab" * 32
CONFIG = {
    "schema": 1,
    "subject": {"deploy_id": DEPLOY},
    "observers": [
        {"alias": "B", "http": "http://127.0.0.1:9"},
        {"alias": "C", "http": "http://127.0.0.1:10"},
    ],
}


class CadenceTests(unittest.TestCase):
    def test_the_default_window_cadence_is_one_second(self):
        live_gate = importlib.util.spec_from_file_location(
            "collector_live_gate", ROOT / "tools" / "live-gate.py")
        module = importlib.util.module_from_spec(live_gate)
        live_gate.loader.exec_module(module)
        self.assertEqual(collector.DEFAULT_INTERVAL, 1.0)
        self.assertEqual(module.DEFAULT_INTERVAL, 1.0)


class ConfigTests(unittest.TestCase):
    def test_no_observer_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "config.json"
            path.write_text(json.dumps({"schema": 1, "observers": []}))
            with self.assertRaises(SystemExit):
                collector.read_config(path)

    def test_duplicate_alias_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "config.json"
            document = dict(CONFIG)
            document["observers"] = [CONFIG["observers"][0], CONFIG["observers"][0]]
            path.write_text(json.dumps(document))
            with self.assertRaises(SystemExit):
                collector.read_config(path)

    def test_the_plan_keeps_the_configured_observers(self):
        plan = collector._plan(CONFIG)
        self.assertEqual(plan["deploy_id"], DEPLOY)
        self.assertEqual([entry["alias"] for entry in plan["validators"]], ["B", "C"])


class SampleTests(unittest.TestCase):
    def test_the_observation_document_keeps_digests_not_endpoints(self):
        original = collector.live_gate.cmd_sample

        def fake(args):
            out = Path(args.out)
            out.mkdir(parents=True, exist_ok=True)
            (out / "run.json").write_text(json.dumps(
                {"schema": 1, "deploy_id": DEPLOY, "validators": [], "execution_proven": False}))
            with gzip.open(out / "samples.jsonl.gz", "wt", encoding="utf-8") as stream:
                stream.write(json.dumps({"alias": "B", "route": "health",
                                         "http_status": 200}) + "\n")
                stream.write(json.dumps({"alias": "B", "route": "status",
                                         "http_status": 200}) + "\n")
            return 0

        collector.live_gate.cmd_sample = fake
        try:
            with tempfile.TemporaryDirectory() as tmp:
                out = Path(tmp) / "run"
                code = collector.cmd_sample(CONFIG, out)
                document = json.loads((out / "observations.json").read_text())
        finally:
            collector.live_gate.cmd_sample = original
        self.assertEqual(code, 0)
        self.assertEqual(document["subject"]["deploy_id"], DEPLOY)
        self.assertEqual(len(document["observers"]), 2)
        self.assertTrue(all("http" not in entry for entry in document["observers"]))
        self.assertTrue(all(len(entry["endpoint_sha256"]) == 64
                            for entry in document["observers"]))
        self.assertEqual(len(document["events"]), 2)
        self.assertEqual(document["record"]["dir"], str(out))

    def test_the_configured_routes_filter_the_observation_document(self):
        original = collector.live_gate.cmd_sample

        def fake(args):
            out = Path(args.out)
            out.mkdir(parents=True, exist_ok=True)
            (out / "run.json").write_text(json.dumps(
                {"schema": 1, "deploy_id": DEPLOY, "validators": [], "execution_proven": False}))
            with gzip.open(out / "samples.jsonl.gz", "wt", encoding="utf-8") as stream:
                for route in ("health", "status"):
                    stream.write(json.dumps({"alias": "B", "route": route,
                                             "http_status": 200}) + "\n")
            return 0

        collector.live_gate.cmd_sample = fake
        try:
            with tempfile.TemporaryDirectory() as tmp:
                out = Path(tmp) / "run"
                config = dict(CONFIG)
                config["routes"] = ["health"]
                collector.cmd_sample(config, out)
                document = json.loads((out / "observations.json").read_text())
        finally:
            collector.live_gate.cmd_sample = original
        self.assertEqual([event["route"] for event in document["events"]], ["health"])


if __name__ == "__main__":
    unittest.main()
