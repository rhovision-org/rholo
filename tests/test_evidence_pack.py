import importlib.util
import json
import os
import shutil
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("evidence_pack", ROOT / "tools/evidence-pack.py")
pack = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pack)


class EvidencePackTests(unittest.TestCase):
    def setUp(self):
        self.base = Path(tempfile.mkdtemp())
        self.run_dir = self.base / "run"
        self.run_dir.mkdir()
        (self.run_dir / "source-sha.txt").write_text("a" * 40 + "\n")
        (self.run_dir / "binary-sha256.txt").write_text("b" * 64 + "\n")
        (self.run_dir / "result.json").write_text(
            json.dumps({"result": "PASS", "rounds": [{"cycle": 1, "result": "PASS"}]})
        )
        (self.run_dir / "observations.jsonl").write_text('{"node": 0, "height": 3}\n')
        (self.run_dir / "node-0.log").write_text("a log line\n")
        self.record = self.base / "record"

    def args(self, **overrides):
        values = {
            "run_dir": str(self.run_dir),
            "record": str(self.record),
            "gate_id": "n8-partition-rejoin",
            "verdict": None,
            "source_sha": None,
            "binary_sha256": None,
            "run_url": "https://github.com/rhovision-org/rholo/actions/runs/41237664538/attempts/1",
            "reference_url": None,
            "locality": "local-devnet",
            "redact": [],
            "move": False,
        }
        values.update(overrides)
        return SimpleNamespace(**values)

    def author(self):
        readme = self.record / "README.md"
        readme.write_text(pack.PLACEHOLDER.sub("authored", readme.read_text()))

    def packed(self, **overrides):
        pack.cmd_pack(self.args(**overrides))
        self.author()
        return pack.cmd_verify(SimpleNamespace(record=str(self.record)))

    def test_pack_then_authored_verify_passes(self):
        self.assertEqual(self.packed(), 0)
        manifest = json.loads((self.record / "manifest.json").read_text())
        self.assertIs(manifest["execution_proven"], False)
        self.assertEqual(manifest["verdict"], "PASS")
        self.assertEqual(manifest["source_sha"], "a" * 40)
        sums = json.loads((self.record / "SHA256SUMS.json").read_text())
        self.assertEqual(len(sums), 5)  # source, binary, result, observations, log
        self.assertTrue(all(path.startswith("raw/") for path in sums))

    def test_a_pin_with_the_wrong_format_is_refused(self):
        with self.assertRaises(SystemExit):
            self.packed(source_sha="abc")
        self.setUp()
        with self.assertRaises(SystemExit):
            self.packed(binary_sha256="xyz")

    def test_a_tampered_byte_fails_verification(self):
        self.packed()
        target = self.record / "raw" / "node-0.log"
        target.write_text("a different line\n")
        with self.assertRaises(SystemExit):
            pack.cmd_verify(SimpleNamespace(record=str(self.record)))

    def test_inconclusive_and_aborted_are_recorded(self):
        for verdict in ("INCONCLUSIVE", "ABORTED"):
            with self.subTest(verdict=verdict):
                self.setUp()
                self.assertEqual(self.packed(verdict=verdict), 0)
                manifest = json.loads((self.record / "manifest.json").read_text())
                self.assertEqual(manifest["verdict"], verdict)

    def test_an_unknown_verdict_is_refused(self):
        with self.assertRaises(SystemExit):
            self.packed(verdict="GREEN")

    def test_a_pass_beside_a_fail_arm_is_refused(self):
        (self.run_dir / "result.json").write_text(
            json.dumps({"result": "PASS", "rounds": [{"cycle": 1, "result": "FAIL"}]})
        )
        with self.assertRaises(SystemExit):
            pack.cmd_pack(self.args())

    def test_a_fork_run_url_is_refused(self):
        os.environ["GITHUB_REPOSITORY"] = "someone/rholo-fork"
        os.environ["GITHUB_RUN_ID"] = "123"
        try:
            with self.assertRaises(SystemExit):
                pack.cmd_pack(self.args(run_url=None))
        finally:
            del os.environ["GITHUB_REPOSITORY"]
            del os.environ["GITHUB_RUN_ID"]

    def test_a_record_under_target_is_refused(self):
        with self.assertRaises(SystemExit):
            pack.cmd_pack(self.args(record=str(ROOT / "target" / "evidence")))

    def test_a_missing_binary_pin_is_refused(self):
        (self.run_dir / "binary-sha256.txt").unlink()
        with self.assertRaises(SystemExit):
            pack.cmd_pack(self.args())

    def test_unresolved_placeholders_fail_verification(self):
        pack.cmd_pack(self.args())
        with self.assertRaises(SystemExit):
            pack.cmd_verify(SimpleNamespace(record=str(self.record)))

    def test_a_symlink_in_the_run_is_refused(self):
        (self.run_dir / "link.log").symlink_to(self.run_dir / "node-0.log")
        with self.assertRaises(SystemExit):
            pack.cmd_pack(self.args())

    def test_a_non_upstream_url_is_refused(self):
        with self.assertRaises(SystemExit):
            pack.cmd_pack(self.args(run_url="https://github.com/other/repo/actions/runs/1"))

    def test_precondition_and_live_host_reference_are_recorded(self):
        self.packed(verdict="PRECONDITION", locality="live-host",
                    reference_url="https://example.invalid/c343-report")
        manifest = json.loads((self.record / "manifest.json").read_text())
        self.assertEqual(manifest["verdict"], "PRECONDITION")
        self.assertEqual(manifest["locality"], "live-host")
        self.assertEqual(manifest["reference"]["url"], "https://example.invalid/c343-report")

    def test_a_non_https_reference_is_refused(self):
        with self.assertRaises(SystemExit):
            pack.cmd_pack(self.args(reference_url="http://example.invalid/report"))

    def test_redactions_are_omitted_and_recorded(self):
        self.packed(redact=["*.log"])
        manifest = json.loads((self.record / "manifest.json").read_text())
        self.assertEqual(manifest["redactions"], ["*.log"])
        paths = {entry["path"] for entry in manifest["files"]}
        self.assertNotIn("raw/node-0.log", paths)

    def verify(self, require_pins=False):
        return pack.cmd_verify(
            SimpleNamespace(record=str(self.record), require_pins=require_pins)
        )

    def manifest(self):
        return json.loads((self.record / "manifest.json").read_text())

    def write_manifest(self, manifest):
        (self.record / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True))

    def test_the_packer_stamps_version_two(self):
        self.packed()
        self.assertEqual(self.manifest()["packer"]["version"], 2)

    def test_a_v2_record_without_authored_non_claims_fails(self):
        pack.cmd_pack(self.args())
        self.author()
        readme = self.record / "README.md"
        readme.write_text(
            readme.read_text().replace(
                "## What this does not establish", "## Other notes"
            )
        )
        with self.assertRaises(SystemExit):
            self.verify()

    def test_require_pins_refuses_an_absent_pin(self):
        self.packed()
        manifest = self.manifest()
        manifest["source_sha"] = None
        self.write_manifest(manifest)
        with self.assertRaises(SystemExit):
            self.verify(require_pins=True)
        # Without the gate flag the legacy-shaped record still verifies.
        self.assertEqual(self.verify(), 0)

    def test_require_pins_accepts_the_packed_pins(self):
        self.packed()
        manifest = self.manifest()
        self.assertRegex(manifest["source_sha"], r"^[0-9a-f]{40}$")
        self.assertRegex(manifest["binary_sha256"], r"^[0-9a-f]{64}$")
        self.assertEqual(self.verify(require_pins=True), 0)

    def test_a_v1_record_is_grandfathered_but_not_for_gate_evidence(self):
        pack.cmd_pack(self.args())
        self.author()
        readme = self.record / "README.md"
        readme.write_text(
            readme.read_text().replace(
                "## What this does not establish", "## Other notes"
            )
        )
        manifest = self.manifest()
        manifest["packer"]["version"] = 1
        self.write_manifest(manifest)
        self.assertEqual(self.verify(), 0)
        with self.assertRaises(SystemExit):
            self.verify(require_pins=True)

    def test_an_empty_file_list_cannot_verify_green(self):
        self.packed()
        manifest = self.manifest()
        manifest["files"] = []
        self.write_manifest(manifest)
        (self.record / "SHA256SUMS.json").write_text("{}\n")
        shutil.rmtree(self.record / "raw")
        with self.assertRaises(SystemExit):
            self.verify()

    def test_an_absent_file_list_cannot_verify_green(self):
        self.packed()
        manifest = self.manifest()
        manifest.pop("files")
        self.write_manifest(manifest)
        with self.assertRaises(SystemExit):
            self.verify()

    def test_an_absent_sums_file_cannot_verify_green(self):
        self.packed()
        (self.record / "SHA256SUMS.json").unlink()
        with self.assertRaises(SystemExit):
            self.verify()

    def test_a_duplicate_file_entry_is_refused(self):
        self.packed()
        manifest = self.manifest()
        manifest["files"].append(dict(manifest["files"][0]))
        self.write_manifest(manifest)
        with self.assertRaises(SystemExit):
            self.verify()

    def test_a_traversing_path_is_refused(self):
        self.packed()
        manifest = self.manifest()
        manifest["files"][0]["path"] = "../outside"
        self.write_manifest(manifest)
        with self.assertRaises(SystemExit):
            self.verify()


if __name__ == "__main__":
    unittest.main()
