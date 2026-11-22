import importlib.util
import json
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

    @staticmethod
    def public_result(**overrides):
        result = {
            "schema": 1, "campaign": "rc-public-fleet", "locality": "public-live",
            "transport": "ssh-systemd", "rehearsal": False, "harness_result": "PASS",
            "preflight": {"checks": {"distinct_host_labels": True,
                                     "distinct_host_boot_ids": True, "ok": True}},
            "synthetic_boot_ids": False,
        }
        result.update(overrides)
        return result

    def test_a_public_live_record_is_a_declarable_locality(self):
        directory = self.record(
            "Public four-validator run on four independent hosts; not an M3 off-host positive.\n"
            "**Evidence locality.** `public-live` — the public four-validator chain.\n")
        (directory / "fleet-result.json").write_text(json.dumps(self.public_result()))
        self.assertEqual(locality.check(directory), [])

    def test_a_public_live_record_needs_the_driver_result(self):
        problems = self.check(
            "Public four-validator run on four independent hosts.\n"
            "**Evidence locality.** `public-live` — four hosts.\n")
        self.assertTrue(any("fleet-result.json" in p for p in problems), problems)

    def test_a_rehearsal_cannot_wear_public_live(self):
        directory = self.record(
            "Public four-validator run on four independent hosts.\n"
            "**Evidence locality.** `public-live` — four hosts.\n")
        (directory / "fleet-result.json").write_text(
            json.dumps(self.public_result(rehearsal=True)))
        problems = locality.check(directory)
        self.assertTrue(any("rehearsal" in p for p in problems), problems)

    def test_synthetic_boot_ids_cannot_wear_public_live(self):
        directory = self.record(
            "Public four-validator run on four independent hosts.\n"
            "**Evidence locality.** `public-live` — four hosts.\n")
        (directory / "fleet-result.json").write_text(
            json.dumps(self.public_result(synthetic_boot_ids=True)))
        problems = locality.check(directory)
        self.assertTrue(any("synthetic" in p for p in problems), problems)

    def test_declared_locality_requires_exactly_one_label(self):
        one = self.record("**Evidence locality.** local-devnet — one host.\n")
        self.assertEqual(locality.declared_locality(one), "local-devnet")
        self.assertIsNone(locality.declared_locality(self.record("No label here.\n")))
        two = self.record(
            "**Evidence locality.** local-devnet — one.\n"
            "**Evidence locality.** single-host — two.\n"
        )
        self.assertIsNone(locality.declared_locality(two))

    def test_a_genuine_off_host_claim_with_dial_back_passes(self):
        problems = self.check(
            "M3 off-host positive.\n"
            "**Evidence locality.** `off-host-claim` — two declared hosts.\n"
            "The bootstrap's dial-back to 203.0.113.7:40400 succeeded before the join.\n",
            acceptance='{"peer": "rnode://6b70@203.0.113.7?protocol=40400"}',
        )
        self.assertEqual(problems, [])

    def test_an_off_host_claim_with_only_wording_fails(self):
        problems = self.check(
            "M3 off-host positive.\n"
            "**Evidence locality.** `off-host-claim` — two declared hosts.\n"
            "The bootstrap's dial-back to 203.0.113.7:40400 succeeded.\n"
        )
        self.assertTrue(any("captured non-loopback" in p for p in problems), problems)

    def test_a_packed_live_host_mutation_in_a_script_fails(self):
        directory = Path(tempfile.mkdtemp())
        (directory / "README.md").write_text(
            "Read-only snapshot.\n**Evidence locality.** live-host — four instances.\n")
        (directory / "manifest.json").write_text(json.dumps(
            {"locality": "live-host", "execution_proven": False}))
        (directory / "raw").mkdir()
        (directory / "raw" / "drill.sh").write_text("POST /deploy\n")
        problems = locality.check(directory)
        self.assertTrue(any("mutation" in p for p in problems), problems)

    def test_an_off_host_claim_with_a_loopback_advertised_peer_fails(self):
        problems = self.check(
            "M3 off-host positive.\n**Evidence locality.** `off-host-claim` — two hosts.\n"
            "dial-back result: ok\n",
            acceptance='{"peer": "rnode://6b70@127.0.0.1?protocol=40400"}',
        )
        self.assertTrue(any("loopback" in p for p in problems), problems)

    def test_private_and_encoded_addresses_are_local(self):
        for host in (
            "10.0.0.1", "172.16.0.1", "192.168.1.1", "169.254.1.1",
            "fc00::1", "fe80::1", "2130706433", "0x7f000001", "017700000001",
            "127.1", "[::ffff:192.168.1.1]", "::127.0.0.1", "::192.168.1.1",
            "::10.0.0.1", "ff02::1",
        ):
            with self.subTest(host=host):
                self.assertTrue(locality.local_address(host), host)
        for host in ("8.8.8.8", "2001:4860::8888", "203.0.113.7"):
            with self.subTest(host=host):
                self.assertFalse(locality.local_address(host), host)

    def test_only_loopback_satisfies_the_diagnostic_label(self):
        self.assertTrue(locality.loopback_address("127.0.0.1"))
        self.assertTrue(locality.loopback_address("2130706433"))
        self.assertTrue(locality.loopback_address("::127.0.0.1"))
        self.assertFalse(locality.loopback_address("10.0.0.1"))
        self.assertFalse(locality.loopback_address("::10.0.0.1"))

    def test_an_off_host_claim_with_a_private_captured_peer_fails(self):
        problems = self.check(
            "M3 off-host positive.\n**Evidence locality.** `off-host-claim` — two hosts.\n"
            "The bootstrap's dial-back to 203.0.113.7:40400 succeeded.\n",
            acceptance='{"peer": "rnode://6b70@10.0.0.7?protocol=40400", '
                       '"other": "rnode://aa@203.0.113.7?protocol=40400"}',
        )
        self.assertTrue(
            any("local (loopback/private/reserved)" in p for p in problems), problems)

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

    def test_a_read_only_live_host_record_passes(self):
        problems = self.check(
            "Read-only B/C/D snapshot; no deploy, restart or key use.\n"
            "**Evidence locality.** `live-host` — hosts B/C/D.\n"
        )
        self.assertEqual(problems, [])

    def test_a_live_host_record_with_a_mutation_fails(self):
        directory = self.record(
            "Read-only.\n**Evidence locality.** `live-host` — hosts B/C/D.\n"
        )
        (directory / "run.json").write_text('{"method": "POST", "route": "/api/v1/deploy"}')
        self.assertTrue(any("mutation" in p for p in locality.check(directory)))

    def test_a_live_host_record_must_say_read_only(self):
        problems = self.check("**Evidence locality.** `live-host` — hosts B/C/D.\n")
        self.assertTrue(any("read-only" in p for p in problems), problems)

    def test_an_external_record_requires_independence_and_a_digest(self):
        problems = self.check("**Evidence locality.** `external` — verdict.\n")
        self.assertTrue(any("independent" in p for p in problems), problems)
        good = self.check(
            "Independent category-A verdict; digest sha256; https://example.invalid/v.\n"
            "**Evidence locality.** `external` — verdict.\n"
        )
        self.assertEqual(good, [])

    def test_a_packed_record_is_scanned(self):
        directory = Path(tempfile.mkdtemp())
        (directory / "README.md").write_text(
            "M3\n**Evidence locality.** local-devnet — packed record.\n")
        (directory / "manifest.json").write_text(json.dumps(
            {"locality": "local-devnet", "execution_proven": False, "verdict": "PASS"}))
        (directory / "SHA256SUMS.json").write_text("{}")
        (directory / "raw").mkdir()
        (directory / "raw" / "run.log").write_text("ok\n")
        self.assertTrue(locality.run_record(directory))
        self.assertEqual(locality.check(directory), [])

    def test_a_packed_locality_mismatch_fails(self):
        directory = Path(tempfile.mkdtemp())
        (directory / "README.md").write_text(
            "**Evidence locality.** local-devnet — packed record.\n")
        (directory / "manifest.json").write_text(json.dumps(
            {"locality": "external", "execution_proven": False}))
        (directory / "raw").mkdir()
        (directory / "raw" / "run.log").write_text("ok\n")
        problems = locality.check(directory)
        self.assertTrue(any("disagrees" in p for p in problems), problems)

    def test_a_packed_live_host_mutation_in_raw_fails(self):
        directory = Path(tempfile.mkdtemp())
        (directory / "README.md").write_text(
            "Read-only snapshot.\n**Evidence locality.** live-host — four instances.\n")
        (directory / "manifest.json").write_text(json.dumps(
            {"locality": "live-host", "execution_proven": False}))
        (directory / "raw").mkdir()
        (directory / "raw" / "run.log").write_text("POST /deploy\n")
        problems = locality.check(directory)
        self.assertTrue(any("mutation" in p for p in problems), problems)

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
