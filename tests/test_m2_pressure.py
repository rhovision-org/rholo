"""The M2 pressure record: typed, raw-bound, and refused when local.

The generic criterion suite exercises the declared controls through `gate-principles.py`; this
suite is the instrument's own control. It proves the two policies separate the instrument from the
gate: a complete local record passes the local policy and fails the public-live criterion.
"""
from __future__ import annotations

import argparse
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
arm = _load("m2_pressure_arm", ROOT / "tools" / "m2-pressure-arm.py")

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

    def test_a_materialised_path_cannot_escape_its_directory(self):
        spec = fixtures.base_g428_m2()
        spec["pressure"]["files"]["../escape.jsonl"] = "{}\n"
        status, problems = pressure.check_pressure(spec["pressure"], PARAMETERS)
        self.assertEqual(status, "FAIL")
        self.assertTrue(any("escapes" in problem for problem in problems), problems)

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

    def test_a_grpc_candidate_record_that_disagrees_fails(self):
        spec = fixtures.base_g428_m2()
        files = spec["pressure"]["files"]
        document = json.loads(files["raw/grpc-candidate.json"])
        document["origin"] = "endpoint"
        files["raw/grpc-candidate.json"] = json.dumps(document)
        files["SHA256SUMS.json"] = json.dumps(
            {name: hashlib.sha256(text.encode()).hexdigest()
             for name, text in files.items() if name != "SHA256SUMS.json"})
        status, problems = pressure.check_pressure(spec["pressure"], PARAMETERS)
        self.assertEqual(status, "FAIL")
        self.assertTrue(any("candidate record disagrees" in problem
                            for problem in problems), problems)

    def test_a_loopback_endpoint_is_refused_even_when_relabelled(self):
        spec = mutate("pressure/record/endpoint/url", "http://127.0.0.1:40403")
        spec["pressure"]["files"]["README.md"] = (
            "# M2 live claim\n\n**Evidence locality.** public-live — four hosts.\n"
        )
        status, problems = pressure.check_pressure(spec["pressure"], PARAMETERS)
        self.assertEqual(status, "FAIL")
        self.assertTrue(any("local" in problem for problem in problems), problems)


class PackedRecordTests(unittest.TestCase):
    def test_a_packed_record_resolves_its_files_under_raw(self):
        base = Path(tempfile.mkdtemp())
        pressure_spec = fixtures.base_g428_m2()["pressure"]
        (base / "raw").mkdir()
        (base / "README.md").write_text(pressure_spec["files"]["README.md"])
        (base / "raw" / "README.md").write_text(pressure_spec["files"]["README.md"])
        (base / "raw" / "pressure.json").write_text(json.dumps(pressure_spec["record"]))
        sums = {}
        for name, text in pressure_spec["files"].items():
            if name in ("README.md", "SHA256SUMS.json"):
                continue
            relative = "raw/" + name.split("raw/", 1)[-1]
            (base / relative).write_text(text)
            sums[relative] = hashlib.sha256(text.encode()).hexdigest()
        chain = json.dumps(pressure_spec["record"])
        (base / "raw" / "pressure.json").write_text(chain)
        sums["raw/pressure.json"] = hashlib.sha256(chain.encode()).hexdigest()
        (base / "SHA256SUMS.json").write_text(json.dumps(sums))
        status, problems = pressure.check_pressure({"dir": str(base)}, PARAMETERS)
        self.assertEqual(status, "PASS", problems)


class ArmTests(unittest.TestCase):
    def phase(self, base: Path, mode: str, files: dict, endpoint: str) -> Path:
        out = base / mode
        out.mkdir(parents=True)
        (out / f"http-{mode}.jsonl").write_text(files[f"raw/http-{mode}.jsonl"])
        (out / f"direct-{mode}.txt").write_text(files[f"raw/direct-{mode}.txt"])
        (out / f"{mode}-inspect.json").write_text(files[f"raw/{mode}-inspect.json"])
        (out / f"{mode}-version.txt").write_text(files[f"raw/{mode}-version.txt"])
        summary = {
            "mode": mode,
            "endpoint": endpoint,
            "cap": 2 if mode == "capped" else None,
            "held": 2 if mode == "capped" else 0,
            "probe": 4 if mode == "capped" else 6,
            "admitted": 0 if mode == "capped" else 6,
            "refused": 4 if mode == "capped" else 0,
            "server_errors": 0,
            "retry_after": 4 if mode == "capped" else 0,
            "handler_reached_after_release": True,
            "requests_per_second": 20.0 if mode == "capped" else 8.3,
            "refusal": {"class": "429" if mode == "capped" else "none",
                        "message": "HTTP in-flight capacity exhausted" if mode == "capped"
                        else ""},
            "transcript": f"raw/http-{mode}.jsonl",
            "direct": {
                "status_line": "HTTP/1.1 429" if mode == "capped" else "HTTP/1.1 400",
                "retry_after": 1 if mode == "capped" else None,
                "transcript": f"raw/direct-{mode}.txt",
            },
            "binary_sha256": "b" * 64,
        }
        (out / "phase.json").write_text(json.dumps(summary))
        return out

    def assemble(self, base: Path, files: dict, *, endpoint: str, locality):
        capped = self.phase(base, "capped", files, endpoint)
        unset = self.phase(base, "unset", files, endpoint)
        grpc_dir = base / "grpc"
        grpc_dir.mkdir()
        (grpc_dir / "grpc-capped.jsonl").write_text(files["raw/grpc-capped.jsonl"])
        (grpc_dir / "grpc-permissive.jsonl").write_text(files["raw/grpc-permissive.jsonl"])
        (grpc_dir / "grpc-candidate.json").write_text(
            files["raw/grpc-candidate.json"])
        (grpc_dir / "grpc-summary.json").write_text(json.dumps({
            "ingress_rate": {"admitted": 1, "refused": 1, "code": 8, "retry_after": None},
            "pool_full": {"admitted": 0, "refused": 1, "code": 8, "retry_after": 1},
            "permissive": {"admitted": 2, "refused": 0},
        }))
        out = base / "record"
        code = arm.assemble(argparse.Namespace(
            capped=str(capped), unset=str(unset), grpc=str(grpc_dir), out=str(out),
            locality=locality, endpoint=endpoint, port=40403, source_sha="a" * 40,
            binary_sha256="",
        ))
        return code, out

    def test_assemble_writes_a_record_that_passes_the_local_policy(self):
        base = Path(tempfile.mkdtemp())
        files = fixtures.base_g428_m2()["pressure"]["files"]
        code, out = self.assemble(base, files, endpoint="http://127.0.0.1:40403",
                                  locality="local-devnet")
        self.assertEqual(code, 0)
        self.assertTrue((out / "observations.json").is_file())
        local = {"locality": "local-devnet", "rate_ceiling": 100}
        status, problems = pressure.check_pressure({"dir": str(out)}, local)
        self.assertEqual(status, "PASS", problems)
        status, problems = pressure.check_pressure({"dir": str(out)},
                                                   {"locality": "public-live",
                                                    "rate_ceiling": 100})
        self.assertEqual(status, "FAIL")

    def test_assemble_writes_a_public_live_record(self):
        base = Path(tempfile.mkdtemp())
        files = fixtures.base_g428_m2()["pressure"]["files"]
        code, out = self.assemble(base, files, endpoint="http://203.0.113.7:40403",
                                  locality=None)
        self.assertEqual(code, 0)
        status, problems = pressure.check_pressure({"dir": str(out)}, PARAMETERS)
        self.assertEqual(status, "PASS", problems)

    def test_assemble_refuses_a_locality_contradiction(self):
        base = Path(tempfile.mkdtemp())
        files = fixtures.base_g428_m2()["pressure"]["files"]
        with self.assertRaises(SystemExit):
            self.assemble(base, files, endpoint="http://203.0.113.7:40403",
                          locality="local-devnet")

    def test_assemble_refuses_mismatched_phase_endpoints(self):
        base = Path(tempfile.mkdtemp())
        files = fixtures.base_g428_m2()["pressure"]["files"]
        capped = self.phase(base, "capped", files, "http://203.0.113.7:40403")
        unset = self.phase(base, "unset", files, "http://203.0.113.8:40403")
        with self.assertRaises(SystemExit):
            arm.assemble(argparse.Namespace(
                capped=str(capped), unset=str(unset), grpc=str(base / "absent"),
                out=str(base / "record"), locality=None,
                endpoint="http://203.0.113.7:40403", port=40403,
                source_sha="a" * 40, binary_sha256="b" * 64,
            ))

    def test_grpc_refuses_an_endpoint_origin(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(SystemExit):
                arm.grpc(argparse.Namespace(
                    out=tmp, origin="endpoint", source_sha="a" * 40,
                    binary_sha256="b" * 64, endpoint="http://203.0.113.7:40404",
                    locality="public-live"))

    def test_phase_refuses_a_remote_endpoint_without_captures(self):
        with tempfile.TemporaryDirectory() as tmp:
            args = argparse.Namespace(
                out=tmp, mode="unset", endpoint="http://203.0.113.7:40403",
                inspect="", version="", container="unsampled", port=40403,
                cap=2, probe=1, spacing=0, settle=0)
            with self.assertRaises(SystemExit):
                arm.phase(args)


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
