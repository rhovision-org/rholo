"""The M2 pressure record: typed, raw-bound, and refused when local.

The generic criterion suite exercises the declared controls through `gate-principles.py`; this
suite is the instrument's own control. It proves the two policies separate the instrument from the
gate: a complete local record passes the local policy and fails the public-live criterion.
"""
from __future__ import annotations

import hashlib
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


fixtures = _load("gate_fixtures", ROOT / "tests" / "gate_fixtures.py")
pressure = _load("m2_pressure", ROOT / "tools" / "m2-pressure.py")

PARAMETERS = {"locality": "public-live", "rate_ceiling": 100}


def mutate(path: str, value: object) -> dict:
    return fixtures.apply_mutation(fixtures.base_g428_m2(), ["set", path, value])


class PressureTests(unittest.TestCase):
    def test_a_complete_record_passes_the_public_live_policy(self):
        status, problems = pressure.check_pressure(fixtures.base_g428_m2()["pressure"], PARAMETERS)
        self.assertEqual(status, "PASS", problems)

    def test_the_unset_control_must_not_refuse(self):
        spec = mutate("pressure/record/surfaces/http/unset/refused", 3)
        status, problems = pressure.check_pressure(spec["pressure"], PARAMETERS)
        self.assertEqual(status, "FAIL")
        self.assertTrue(any("unset" in problem for problem in problems), problems)

    def test_a_transcript_that_disagrees_with_its_summary_fails(self):
        spec = mutate(
            "pressure/files/raw~1http-capped.jsonl",
            '{"kind":"probe","status":500,"retry_after":null,"body":"","t_ms":0}\n',
        )
        status, problems = pressure.check_pressure(spec["pressure"], PARAMETERS)
        self.assertEqual(status, "FAIL")
        self.assertTrue(any("probe count" in problem for problem in problems), problems)

    def test_a_packed_file_must_be_digest_pinned(self):
        spec = mutate("pressure/files/SHA256SUMS.json", "{}")
        status, problems = pressure.check_pressure(spec["pressure"], PARAMETERS)
        self.assertEqual(status, "FAIL")
        self.assertTrue(any("not digest-pinned" in problem for problem in problems), problems)

    def test_a_missing_surface_is_a_precondition(self):
        spec = fixtures.apply_mutation(
            fixtures.base_g428_m2(), ["delete", "pressure/record/surfaces/grpc"]
        )
        status, _ = pressure.check_pressure(spec["pressure"], PARAMETERS)
        self.assertEqual(status, "PRECONDITION")

    def test_a_local_record_passes_the_local_policy_and_fails_the_live_policy(self):
        spec = mutate("pressure/record/locality", "local-devnet")
        spec["pressure"]["record"]["endpoint"]["url"] = "http://172.20.0.2:40403"
        spec["pressure"]["files"]["README.md"] = (
            "# M2 local rehearsal\n\n**Evidence locality.** local-devnet — one Docker host.\n"
        )
        local = {"locality": "local-devnet", "rate_ceiling": 100}
        status, problems = pressure.check_pressure(spec["pressure"], local)
        self.assertEqual(status, "PASS", problems)
        status, problems = pressure.check_pressure(spec["pressure"], PARAMETERS)
        self.assertEqual(status, "FAIL")
        self.assertTrue(any("public-live" in problem for problem in problems), problems)

    def test_a_loopback_endpoint_is_refused_even_when_relabelled(self):
        spec = mutate("pressure/record/endpoint/url", "http://127.0.0.1:40403")
        spec["pressure"]["files"]["README.md"] = (
            "# M2 live claim\n\n**Evidence locality.** public-live — four hosts.\n"
        )
        status, problems = pressure.check_pressure(spec["pressure"], PARAMETERS)
        self.assertEqual(status, "FAIL")
        self.assertTrue(any("local" in problem for problem in problems), problems)


class CliTests(unittest.TestCase):
    def write(self, spec: dict, public: bool) -> Path:
        directory = Path(tempfile.mkdtemp())
        files = dict(spec["pressure"]["files"])
        files["pressure.json"] = json.dumps(spec["pressure"]["record"])
        if not public:
            record = spec["pressure"]["record"]
            record["locality"] = "local-devnet"
            record["endpoint"]["url"] = "http://172.20.0.2:40403"
            files["README.md"] = (
                "# M2 local rehearsal\n\n**Evidence locality.** local-devnet — one Docker host.\n"
            )
            files["pressure.json"] = json.dumps(record)
        files.pop("SHA256SUMS.json", None)
        files["SHA256SUMS.json"] = json.dumps(
            {name: hashlib.sha256(text.encode()).hexdigest()
             for name, text in files.items() if name != "README.md"}
        )
        for name, text in files.items():
            path = directory / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
        return directory

    def test_the_cli_separates_the_policies(self):
        public = self.write(fixtures.base_g428_m2(), public=True)
        local = self.write(fixtures.base_g428_m2(), public=False)
        self.assertEqual(pressure.check_record(public, "public-live")[0], 0)
        self.assertEqual(pressure.check_record(local, "local-devnet")[0], 0)
        self.assertEqual(pressure.check_record(local, "public-live")[0], 1)
        self.assertEqual(pressure.check_record(public, "any")[0], 0)

    def test_an_absent_record_is_a_precondition(self):
        directory = Path(tempfile.mkdtemp())
        self.assertEqual(pressure.check_record(directory, "any")[0], 3)


if __name__ == "__main__":
    unittest.main()
