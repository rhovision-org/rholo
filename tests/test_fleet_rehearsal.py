"""The opt-in four-validator dress rehearsal.

It boots a real four-validator devnet, generates the campaign manifest, runs the public claim paths
with `--rehearse`, and proves the driver performed the deploy and the stop/start. The records are
`not run` and stay under `target/`; nothing here is evidence. Skipped unless `FLEET_REHEARSAL=1`
because it starts four containers and takes minutes.
"""
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PREFIX = "rcfleet"


def environment():
    env = dict(os.environ)
    env.update({
        "DEVNET_PREFIX": PREFIX,
        "DEVNET_NODE_ENV": "RNODE_TRACE_DIR=/var/lib/rnode/traces",
        # A populated store at ~1,600 blocks reads 4,087 MiB against the 4,096 floor at 2 GiB;
        # the local RC-4 arm uses 6 GiB for the same reason.
        "DEVNET_NODE_MEMORY": "6g",
    })
    return env


@unittest.skipUnless(os.environ.get("FLEET_REHEARSAL") == "1",
                     "set FLEET_REHEARSAL=1 to start the four-validator rehearsal")
class RehearsalTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.env = environment()
        subprocess.run(["bash", str(ROOT / "tools" / "devnet.sh"), "up",
                        "--validators", "4", "--fresh"],
                       cwd=ROOT, env=cls.env, check=True, timeout=1200)
        cls.work = Path(tempfile.mkdtemp(dir=ROOT / "target", prefix="rc-public-fleet-"))
        cls.manifests = {}
        for claim in ("RC-1", "RC-4"):
            cls.manifests[claim] = cls._manifest(claim)
        for claim in ("RC-2", "RC-3"):
            cls.manifests[claim] = cls._manifest(claim, typed=True)

    @classmethod
    def _manifest(cls, claim: str, typed: bool = False) -> Path:
        path = cls.work / f"manifest-{claim.lower()}.json"
        command = [sys.executable, str(ROOT / "tools" / "fleet-run.py"), "manifest",
                   "--prefix", PREFIX, "--transport", "docker", "--out", str(path)]
        if typed:
            command += ["--claim", claim]
        subprocess.run(command, cwd=ROOT, env=cls.env, check=True, timeout=300)
        return path

    @classmethod
    def tearDownClass(cls):
        subprocess.run(["bash", str(ROOT / "tools" / "devnet.sh"), "down", "-v"],
                       cwd=ROOT, env=cls.env, timeout=600)

    def _run_claim(self, claim: str, arm: str | None = None) -> tuple[int, dict]:
        out = self.work / (claim if arm is None else f"{claim}-{arm}")
        command = [sys.executable, str(ROOT / "tools" / "release-check.py"), "run",
                   "--claim", claim, "--env", "public", "--rehearse",
                   "--public-manifest", str(self.manifests[claim]), "--out", str(out)]
        if arm is not None:
            command += ["--arm", arm]
        result = subprocess.run(command, cwd=ROOT, env=self.env, text=True,
                                capture_output=True, timeout=3600)
        action = "fleet-deploy" if claim == "RC-1" else "fleet-recovery"
        fleet = out / "fleet-result.json"
        if not fleet.is_file():
            fleet = out / action / "fleet-result.json"
        document = json.loads(fleet.read_text()) if fleet.is_file() else {}
        return result.returncode, document

    def test_the_public_paths_run_on_four_local_validators(self):
        rc1_code, rc1 = self._run_claim("RC-1")
        self.assertEqual(rc1_code, 3, rc1.get("reasons"))  # a rehearsal is not evidence
        self.assertEqual(rc1.get("harness_result"), "PASS", rc1.get("reasons"))
        self.assertTrue((rc1.get("deploy") or {}).get("deploy_id"))

        rc4_code, rc4 = self._run_claim("RC-4")
        self.assertEqual(rc4_code, 3, rc4.get("reasons"))
        self.assertEqual(rc4.get("harness_result"), "PASS", rc4.get("reasons"))
        victim = rc4["victim"]
        self.assertNotEqual(rc4["boot_ids"]["before"][victim],
                            rc4["boot_ids"]["after"][victim])
        self.assertGreaterEqual(rc4["rejoin"]["heights_above_baseline"], 3)
        self.assertTrue(rc4["boot_traces"])
        for claim in ("RC-1", "RC-4"):
            record = json.loads((self.work / claim / "claim.json").read_text())
            self.assertTrue(record.get("rehearsal"))
            self.assertEqual(record.get("locality"), "local-devnet")
            self._predicate_passes(claim, record)

    def test_the_typed_public_arms_run_on_four_local_validators(self):
        rc2_code, rc2 = self._run_claim("RC-2")
        self.assertEqual(rc2_code, 3, rc2.get("reasons"))  # a rehearsal is not evidence
        self.assertEqual(rc2.get("harness_result"),
                         "PASS_BOUNDED_WORKLOAD_PARTITION_REJOIN", rc2.get("reasons"))
        self.assertEqual(len(rc2.get("cycles") or []), 2)
        for cycle in rc2["cycles"]:
            self.assertTrue(cycle["producing_victim"])
            self.assertTrue(all(row["finalized_anchor"]
                                for row in cycle["anchor_retention"]))
        self._predicate_passes("RC-2",
                               json.loads((self.work / "RC-2" / "claim.json").read_text()))

        rc3_code, rc3 = self._run_claim("RC-3", arm="availability")
        self.assertEqual(rc3_code, 3, rc3.get("reasons"))
        self.assertEqual(rc3.get("harness_result"), "PASS", rc3.get("reasons"))
        availability = rc3.get("availability") or {}
        self.assertGreaterEqual(len(availability.get("blocks") or []), 10)
        self.assertEqual(availability.get("not_available"), [])
        self._predicate_passes(
            "RC-3",
            json.loads((self.work / "RC-3-availability" / "claim.json").read_text()))

    def _predicate_passes(self, claim: str, record: dict) -> None:
        """The observations, read without the rehearsal marker, satisfy the claim's own check."""
        import importlib.util
        record = json.loads(json.dumps(record))
        record.pop("rehearsal", None)
        record["verdict"] = "pass"
        spec = importlib.util.spec_from_file_location(
            "rehearsal_claim", ROOT / "tests" / f"release-claim-{claim[-1]}.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        verdict, reasons = module.check(record)
        self.assertEqual(verdict, "pass", reasons)


if __name__ == "__main__":
    unittest.main()
