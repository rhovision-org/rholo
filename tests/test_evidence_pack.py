import importlib.util
import json
import os
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

    def test_a_tampered_byte_fails_verification(self):
        self.packed()
        target = self.record / "raw" / "node-0.log"
        target.write_text("a different line\n")
        with self.assertRaises(SystemExit):
            pack.cmd_verify(SimpleNamespace(record=str(self.record)))

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

    def test_redactions_are_omitted_and_recorded(self):
        self.packed(redact=["*.log"])
        manifest = json.loads((self.record / "manifest.json").read_text())
        self.assertEqual(manifest["redactions"], ["*.log"])
        paths = {entry["path"] for entry in manifest["files"]}
        self.assertNotIn("raw/node-0.log", paths)


if __name__ == "__main__":
    unittest.main()
