import importlib.util
import tempfile
import unittest
from unittest.mock import patch
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("locality", ROOT / "tools/check-evidence-locality.py")
locality = importlib.util.module_from_spec(spec)
spec.loader.exec_module(locality)


class EvidenceLocalityTests(unittest.TestCase):
    def test_loopback_aliases_and_unspecified_advertised_hosts_fail(self):
        for host in ("127.0.0.2", "127.255.1.2", "[::1]", "[::ffff:127.0.0.1]",
                     "0.0.0.0", "[::]", "LOCALHOST."):
            for command in (f"rnode://6b70@{host}?protocol=40400", f"--host={host}"):
                with self.subTest(command=command):
                    problems = self.check(
                        "M3\n**Evidence locality.** off-host-claim — hosts\ndial-back ok\n",
                        acceptance=command)
                    self.assertTrue(any("loopback" in p for p in problems), problems)

    def test_json_argv_and_api_locality_are_distinguished(self):
        directory = self.record("M3\n**Evidence locality.** off-host-claim — hosts\ndial-back ok\n",
                                '["--host", "127.0.0.2"]')
        self.assertTrue(locality.check(directory))
        (directory / "acceptance.json").write_text(
            '{"api":"http://127.0.0.2:40403", "peer":"rnode://aa@203.0.113.7?protocol=40400"}')
        self.assertEqual(locality.check(directory), [])
        self.assertFalse(locality.local_address("localhost.example.org"))

    def test_gate_reports_missing_readme_without_a_traceback(self):
        with tempfile.TemporaryDirectory() as directory:
            record = Path(directory) / "missing-readme"
            record.mkdir()
            (record / "run.json").write_text("{}")
            with patch("sys.argv", ["locality", "--gate", directory]):
                self.assertEqual(locality.main(), 1)

    def record(self, readme, acceptance="{}"):
        directory = Path(tempfile.mkdtemp())
        (directory / "README.md").write_text(readme)
        (directory / "acceptance.json").write_text(acceptance)
        return directory

    def check(self, readme, acceptance="{}"):
        return locality.check(self.record(readme, acceptance))

    def test_a_genuine_off_host_claim_with_dial_back_passes(self):
        problems = self.check(
            "M3 off-host positive.\n"
            "**Evidence locality.** `off-host-claim` — two declared hosts.\n"
            "The bootstrap's dial-back to 203.0.113.7:40400 succeeded before the join.\n"
        )
        self.assertEqual(problems, [])

    def test_an_off_host_claim_with_a_loopback_advertised_peer_fails(self):
        problems = self.check(
            "M3 off-host positive.\n**Evidence locality.** `off-host-claim` — two hosts.\n"
            "dial-back result: ok\n",
            acceptance='{"peer": "rnode://6b70@127.0.0.1?protocol=40400"}',
        )
        self.assertTrue(any("loopback" in p for p in problems), problems)

    def test_an_off_host_claim_without_a_dial_back_fails(self):
        problems = self.check(
            "M3 off-host positive.\n**Evidence locality.** `off-host-claim` — two hosts.\n"
        )
        self.assertTrue(any("dial-back" in p for p in problems), problems)

    def test_a_loopback_diagnostic_must_say_what_it_is_not(self):
        problems = self.check(
            "M3 observer.\n**Evidence locality.** `loopback-diagnostic` — loopback.\n",
            acceptance='{"peer": "rnode://6b70@127.0.0.1?protocol=40400"}',
        )
        self.assertTrue(any("not an M3 off-host positive" in p for p in problems), problems)

    def test_the_corrected_shape_passes(self):
        problems = self.check(
            "M3 observer.\n**Evidence locality.** `loopback-diagnostic` — loopback.\n"
            "This is not an M3 off-host positive.\n",
            acceptance='{"peer": "rnode://6b70@127.0.0.1?protocol=40400"}',
        )
        self.assertEqual(problems, [])

    def test_a_record_that_mentions_m3_without_a_label_fails(self):
        problems = self.check("M3 off-host positive, no label.\n")
        self.assertTrue(any("declares 0 localities" in p for p in problems), problems)

    def test_a_local_devnet_claiming_off_host_fails(self):
        problems = self.check(
            "M3 off-host positive.\n**Evidence locality.** `local-devnet` — local.\n"
        )
        self.assertTrue(any("off-host positive" in p for p in problems), problems)

    def test_an_empty_evidence_directory_is_not_vacuous(self):
        # The main() vacuity rule is exercised against a real temporary directory.
        directory = Path(tempfile.mkdtemp())
        records = [d for d in directory.iterdir() if d.is_dir()]
        self.assertEqual(records, [])
        # And a run record without a README cannot be dispositioned.
        record = directory / "n123-empty"
        record.mkdir()
        (record / "acceptance.json").write_text("{}")
        self.assertTrue(locality.check(record))


if __name__ == "__main__":
    unittest.main()
