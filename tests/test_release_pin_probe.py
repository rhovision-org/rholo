import contextlib
import importlib.util
import io
import json
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("release_pin_probe", ROOT / "tools/release-pin-probe.py")
probe = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(probe)


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


SOURCE = "a" * 40
BINARY = "b" * 64
ARTIFACT = "c" * 64
ALIASES = [f"validator-{index}" for index in range(4)]


def complete_record(name: str) -> dict:
    return {
        "name": name,
        "version": "rnode 0.1.0 (" + SOURCE + ")",
        "source_sha": SOURCE,
        "binary_sha256": BINARY,
        "binary_path": "/usr/local/bin/rnode",
        "network_id": "testnet",
        "shard_id": "/root",
        "genesis_hash": "b" * 64,
        "genesis_raw_sha256": "c" * 64,
        "bonds_sha256": "d" * 64,
        "total_stake": 400,
        "height": 100,
        "finalized_height": 99,
        "finalized_hash": "e" * 64,
        "finalized_lag": 1,
        "ready": True,
        "boot_id": f"boot-{name}",
        "boot_id_source": "kernel",
        "unset": [],
    }


def complete_operator(aliases=None) -> dict:
    return {
        "release_tag": "v0.2.0",
        "release_branch": "main",
        "selected_source_sha": SOURCE,
        "selected_artifact_digest": ARTIFACT,
        "selected_binary_sha256": BINARY,
        "window_start_utc": "2026-12-01T10:00:00Z",
        "window_end_utc": "2026-12-01T11:00:00Z",
        "operator": "operator-key",
        "observers": ["observer-a"],
        "independent_reviewer": "reviewer-b",
        "rollback_owner": "rollback-c",
        "test_transfer": {"source": "s", "destination": "d", "amount": 1},
        "rollback": {"build": "previous-sha"},
        "evidence_destination": "spec/audit/evidence/n487-live-postdeployment-20261201",
        "deploy_relay": {
            "mode": "relay",
            "aliases": list(aliases or ALIASES),
            "destination_port": 40401,
            "reachability_checked_utc": "2026-12-01T09:55:00Z",
            "unreachable_aliases": [],
        },
    }


class VersionParseTests(unittest.TestCase):
    def test_source_sha_is_read_from_the_version_string(self):
        self.assertEqual(
            probe.source_sha_from_version("rholo node 0.1.0 (0123456789abcdef)"),
            "0123456789abcdef",
        )
        self.assertIsNone(probe.source_sha_from_version("rnode 0.1.0"))
        self.assertIsNone(probe.source_sha_from_version(None))


class CanonicalBondsTests(unittest.TestCase):
    def test_canonical_bonds_is_order_independent(self):
        a = [{"validator": "v2", "stake": 200}, {"validator": "v1", "stake": 200}]
        b = [{"validator": "v1", "stake": 200}, {"validator": "v2", "stake": 200}]
        bonds_a, sha_a, total_a = probe.canonical_bonds(a)
        bonds_b, sha_b, total_b = probe.canonical_bonds(b)
        self.assertEqual(sha_a, sha_b)
        self.assertEqual(bonds_a, bonds_b)
        self.assertEqual(total_a, 400)
        self.assertEqual(total_b, 400)

    def test_unparseable_bond_is_kept_in_the_hash(self):
        _, sha_a, _ = probe.canonical_bonds([{"odd": 1}])
        _, sha_b, _ = probe.canonical_bonds([{"odd": 2}])
        self.assertNotEqual(sha_a, sha_b)


class ManifestTests(unittest.TestCase):
    def test_complete_manifest_is_ok(self):
        records = [complete_record(f"validator-{i}") for i in range(4)]
        manifest = probe.build_manifest(records, complete_operator())
        self.assertTrue(manifest["checks"]["ok"], manifest["errors"])
        self.assertEqual(manifest["checks"]["validator_count"], 4)
        self.assertEqual(manifest["unset_observed"], [])
        self.assertEqual(manifest["unset_operator"], [])

    def test_missing_observed_and_operator_fields_fail_closed(self):
        record = complete_record("validator-1")
        record["finalized_hash"] = None
        record["unset"] = ["finalized_hash"]
        manifest = probe.build_manifest([record], {})
        self.assertFalse(manifest["checks"]["ok"])
        self.assertIn("validator-1:finalized_hash", manifest["unset_observed"])
        self.assertIn("operator", manifest["unset_operator"])
        self.assertFalse(manifest["checks"]["all_operator_fields"])

    def test_disagreement_across_validators_is_an_error(self):
        records = [complete_record("validator-1"), complete_record("validator-2")]
        records[1]["genesis_hash"] = "f" * 64
        records[1]["binary_sha256"] = "f" * 64
        manifest = probe.build_manifest(records, complete_operator())
        self.assertFalse(manifest["checks"]["ok"])
        self.assertFalse(manifest["checks"]["genesis_hash_agreement"])
        self.assertFalse(manifest["checks"]["binary_sha256_agreement"])
        self.assertTrue(any("genesis_hash disagrees" in e for e in manifest["errors"]))

    def test_redact_removes_endpoints_and_host_identity(self):
        record = complete_record("validator-1")
        record["http"] = "http://private.example:40403"
        record["admin"] = "http://private.example:40405"
        record["local"] = {"host": "validator-host-1"}
        manifest = probe.build_manifest([record], complete_operator())
        redacted = probe.redact_manifest(manifest)
        self.assertEqual(redacted["validators"][0]["http"], "<redacted>")
        self.assertEqual(redacted["validators"][0]["admin"], "<redacted>")
        self.assertEqual(redacted["validators"][0]["local"]["host"], "<redacted>")
        self.assertEqual(redacted["operator_fields"]["operator"], "<redacted>")
        self.assertEqual(redacted["operator_fields"]["independent_reviewer"], "<redacted>")


    def test_selected_candidate_pins_are_carried_and_required(self):
        records = [complete_record(name) for name in ALIASES]
        operator = complete_operator()
        manifest = probe.build_manifest(records, operator)
        for field in ("selected_source_sha", "selected_artifact_digest",
                      "selected_binary_sha256"):
            self.assertEqual(manifest["operator_fields"][field], operator[field])
        del operator["selected_artifact_digest"]
        manifest = probe.build_manifest(records, operator)
        self.assertIn("selected_artifact_digest", manifest["unset_operator"])
        self.assertFalse(manifest["checks"]["ok"])

    def test_selected_pin_formats_are_enforced(self):
        records = [complete_record(name) for name in ALIASES]
        operator = complete_operator()
        operator["selected_source_sha"] = "abc1234"
        operator["selected_artifact_digest"] = "d" * 63
        manifest = probe.build_manifest(records, operator)
        self.assertFalse(manifest["checks"]["operator_fields_typed"])
        self.assertFalse(manifest["checks"]["ok"])
        self.assertTrue(any("selected_source_sha" in error for error in manifest["errors"]))
        self.assertTrue(any("selected_artifact_digest" in error for error in manifest["errors"]))

    def test_selected_candidate_must_match_the_validator_records(self):
        records = [complete_record(name) for name in ALIASES]
        records[2]["binary_sha256"] = "f" * 64
        manifest = probe.build_manifest(records, complete_operator())
        self.assertFalse(manifest["checks"]["selected_candidate_matches_validators"])
        self.assertFalse(manifest["checks"]["ok"])

    def test_deploy_relay_is_required_and_typed(self):
        records = [complete_record(name) for name in ALIASES]
        operator = complete_operator()
        del operator["deploy_relay"]
        manifest = probe.build_manifest(records, operator)
        self.assertIn("deploy_relay", manifest["unset_operator"])
        self.assertFalse(manifest["checks"]["ok"])
        operator = complete_operator()
        operator["deploy_relay"]["mode"] = "broadcast"
        manifest = probe.build_manifest(records, operator)
        self.assertFalse(manifest["checks"]["operator_fields_typed"])
        self.assertTrue(any("deploy_relay.mode" in error for error in manifest["errors"]))

    def test_distinct_boot_ids_are_required(self):
        records = [complete_record(name) for name in ALIASES]
        records[3]["boot_id"] = records[0]["boot_id"]
        manifest = probe.build_manifest(records, complete_operator())
        self.assertFalse(manifest["checks"]["distinct_boot_ids"])
        self.assertFalse(manifest["checks"]["ok"])

    def test_duplicate_validator_names_are_not_ok(self):
        records = [complete_record(name) for name in ALIASES]
        records[3]["name"] = records[0]["name"]
        manifest = probe.build_manifest(records, complete_operator())
        self.assertFalse(manifest["checks"]["distinct_names"])
        self.assertFalse(manifest["checks"]["ok"])

    def test_a_rehearsal_boot_id_is_marked_synthetic(self):
        records = [complete_record(name) for name in ALIASES]
        records[0]["boot_id_source"] = "rehearsal"
        manifest = probe.build_manifest(records, complete_operator())
        self.assertTrue(manifest["synthetic_boot_ids"])


class ProtocolAgreementTests(unittest.TestCase):
    """The collector, the live gate and the schema must describe one operator record."""

    @classmethod
    def setUpClass(cls):
        cls.gate = _load("gate_agreement_live_gate", ROOT / "tools" / "live-gate.py")
        cls.schema = json.loads(
            (ROOT / "spec" / "audit" / "runbooks"
             / "n487-live-postdeployment.preflight.schema.json").read_text()
        )

    def test_the_two_tools_share_one_operator_field_list(self):
        self.assertEqual(list(probe.OPERATOR_FIELDS), list(self.gate.OPERATOR_FIELDS))

    def test_the_schema_requires_the_same_operator_fields(self):
        required = self.schema["properties"]["operator_fields"]["required"]
        self.assertEqual(set(required), set(probe.OPERATOR_FIELDS))
        self.assertIn("boot_id",
                      self.schema["properties"]["validators"]["items"]["required"])

    def test_a_collector_produced_preflight_passes_g1(self):
        records = [complete_record(name) for name in ALIASES]
        manifest = probe.build_manifest(records, complete_operator())
        self.assertTrue(manifest["checks"]["ok"], manifest["errors"])
        report = self.gate.check_g1(manifest)
        self.assertEqual(report["status"], "PASS", report)


class CliTests(unittest.TestCase):
    def run_merge(self, out: Path, operator: Path | None) -> int:
        argv = ["merge", "--out", str(out), "--expect", "4", "--check"]
        if operator:
            argv += ["--operator-json", str(operator)]
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            return probe.main(argv)

    def test_merge_check_passes_only_with_pins_and_operator_fields(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            for index in range(4):
                (out / f"validator-{index}.json").write_text(
                    json.dumps(complete_record(f"validator-{index}"))
                )
            operator = out / "operator.json"
            operator.write_text(json.dumps(complete_operator()))
            self.assertEqual(self.run_merge(out, operator), 0)
            self.assertTrue((out / "manifest.json").is_file())
            operator.unlink()
            self.assertEqual(self.run_merge(out, None), 1)

    def test_wrong_validator_count_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            (out / "validator-1.json").write_text(json.dumps(complete_record("validator-1")))
            operator = out / "operator.json"
            operator.write_text(json.dumps(complete_operator()))
            self.assertEqual(self.run_merge(out, operator), 1)

    def test_merge_check_refuses_an_unreachable_relay_pin(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            for index in range(4):
                (out / f"validator-{index}.json").write_text(
                    json.dumps(complete_record(f"validator-{index}")))
            operator = complete_operator()
            operator["deploy_relay"]["unreachable_aliases"] = ["validator-3"]
            path = out / "operator.json"
            path.write_text(json.dumps(operator))
            self.assertEqual(self.run_merge(out, path), 1)
            manifest = json.loads((out / "manifest.json").read_text())
            self.assertFalse(manifest["checks"]["operator_fields_typed"])
            self.assertFalse(manifest["checks"]["ok"])


if __name__ == "__main__":
    unittest.main()
