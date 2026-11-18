"""Contract tests for tools/live-gate.py: a worksheet, never a verdict.

The fixtures are built in a temporary directory rather than committed: the point is the negative
controls, and each one mutates the same small good bundle into the shape it must refuse.
"""

import gzip
import hashlib
import importlib.util
import json
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("live_gate", ROOT / "tools/live-gate.py")
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)

DEPLOY = "ab" * 32
OTHER = "cd" * 32
SOURCE = "1" * 40
BINARY = "2" * 64
ALIASES = ("v1", "v2", "v3", "v4")


def sample(alias, route, tick, utc, status=200, payload=None, error=None):
    text = json.dumps(payload, sort_keys=True) if payload is not None else ""
    return {
        "schema": 1,
        "tool": "live-gate",
        "event": "sample",
        "tick": tick,
        "alias": alias,
        "route": route,
        "path": f"/{route}",
        "utc": utc,
        "monotonic_ns": tick * 1_000_000,
        "http_status": status,
        "elapsed_ms": 5,
        "body_raw": text,
        "body_sha256": hashlib.sha256(text.encode()).hexdigest(),
        "body_truncated": False,
        "body": payload,
        "error": error,
    }


def finality(height, block_hash, utc, tick, status=200):
    return {
        "availability": "unknown",
        "scope": "nodeLocal",
        "observedAtUnixMs": 1_790_000_000_000,
        "latestBlockNumber": 102,
        "finalizedBlockNumber": height,
        "finalizedBlockHash": block_hash,
        "observedFinalityLag": 102 - height,
    }


def settlement(stage, confirmed, finalized, block_hash=None, block_number=None):
    return {
        "deployId": DEPLOY,
        "stage": stage,
        "execution": "unknown" if stage in ("pooled", "notProcessed") else "succeeded",
        "finalized": finalized,
        "settlementConfirmed": confirmed,
        "blockHash": block_hash,
        "blockNumber": block_number,
        "nodeStatus": None,
        "scope": "nodeLocal",
    }


def preflight():
    validators = []
    for index, alias in enumerate(ALIASES):
        validators.append(
            {
                "name": alias,
                "version": "rholo 0.1.0",
                "source_sha": SOURCE,
                "binary_sha256": BINARY,
                "binary_path": "/usr/local/bin/rnode",
                "boot_id": f"boot-{index}",
                "network_id": "testnet",
                "shard_id": "root",
                "genesis_hash": "genesis",
                "genesis_raw_sha256": "3" * 64,
                "bonds": [],
                "bonds_sha256": "4" * 64,
                "total_stake": 100,
                "height": 102,
                "finalized_height": 98,
                "finalized_hash": "98hash",
                "finalized_lag": 4,
                "ready": True,
                "unset": [],
            }
        )
    return {
        "schema": 1,
        "generated_at_utc": "2026-11-14T10:00:00Z",
        "source": {"sha": SOURCE, "tag": "v-test", "branch": "dev"},
        "validators": validators,
        "operator_fields": {
            "release_tag": "v-test",
            "release_branch": "dev",
            "selected_source_sha": SOURCE,
            "selected_artifact_digest": "6" * 64,
            "selected_binary_sha256": BINARY,
            "window_start_utc": "2026-11-14T10:00:00Z",
            "window_end_utc": "2026-11-14T10:15:00Z",
            "operator": "operator",
            "observers": ["observer-1", "observer-2"],
            "independent_reviewer": "reviewer",
            "rollback_owner": "rollback",
            "test_transfer": {"amount": 1},
            "rollback": {"build": "previous"},
            "evidence_destination": "spec/audit/evidence/n487-live-2026-11-14",
            "deploy_relay": {
                "mode": "relay",
                "aliases": list(ALIASES),
                "destination_port": 40401,
                "reachability_checked_utc": "2026-11-14T09:55:00Z",
                "unreachable_aliases": [],
            },
        },
        "unset_observed": [],
        "unset_operator": [],
        "checks": {"ok": True, "validator_count": 4},
    }


def submission():
    return {
        "deploy_id": DEPLOY,
        "utc": "2026-11-14T10:00:40Z",
        "http_status": 200,
        "payload_sha256": "5" * 64,
    }


def claims(settled=True, basis="finalizedSuccess"):
    return {"settled_claim": settled, "basis": basis}


def recovery():
    return {
        "arm": "restart",
        "start_utc": "2026-11-14T10:05:00Z",
        "end_utc": "2026-11-14T10:06:00Z",
        "same_deploy_finalized": True,
        "accepted_deploys_before": [DEPLOY, OTHER],
        "accepted_deploys_after": [DEPLOY, OTHER],
        "final_stages": {DEPLOY: "finalizedSuccess", OTHER: "finalizedError"},
        "lost_deploys": [],
        "own_key_halt_observed": False,
        "self_replay_observed": False,
        "commands": ["systemctl restart rnode"],
        "post_recovery_settlement": {
            "stage": "finalizedSuccess",
            "settlementConfirmed": True,
            "finalized": True,
        },
    }


def good_events():
    events = []

    def tick(name, utc, number):
        for alias in ALIASES:
            events.append(sample(alias, "health", number, utc, payload={"ready": True}))
            events.append(sample(alias, "status", number, utc, payload={"acceptsDeploys": True}))
            events.append(
                sample(alias, "last_finalized", number, utc, payload={"blockNumber": 102, "blockHash": "tip"})
            )

    tick("baseline", "2026-11-14T10:00:20Z", 0)
    for alias in ALIASES:
        events.append(sample(alias, "finality", 0, "2026-11-14T10:00:20Z", payload=finality(98, "98hash", "", 0)))
        events.append(
            sample(
                alias,
                "deploy_settlement",
                0,
                "2026-11-14T10:00:20Z",
                payload=settlement("pooled", False, None),
            )
        )
        events.append(
            sample(alias, "deploy_status", 0, "2026-11-14T10:00:20Z", payload={"NotProcessed": {"status": "Pooled"}})
        )

    tick("included", "2026-11-14T10:01:10Z", 1)
    for alias in ALIASES:
        events.append(sample(alias, "finality", 1, "2026-11-14T10:01:10Z", payload=finality(98, "98hash", "", 1)))
        events.append(
            sample(
                alias,
                "deploy_settlement",
                1,
                "2026-11-14T10:01:10Z",
                payload=settlement("includedUnfinalized", False, False, "block-101", 101),
            )
        )
        events.append(
            sample(
                alias,
                "deploy_status",
                1,
                "2026-11-14T10:01:10Z",
                payload={
                    "ProcessedWithSuccess": {
                        "deployResult": [],
                        "block": {"blockHash": "block-101", "blockNumber": 101},
                    }
                },
            )
        )

    tick("finalized", "2026-11-14T10:11:00Z", 2)
    for alias in ALIASES:
        events.append(sample(alias, "finality", 2, "2026-11-14T10:11:00Z", payload=finality(101, "block-101", "", 2)))
        events.append(
            sample(
                alias,
                "deploy_settlement",
                2,
                "2026-11-14T10:11:00Z",
                payload=settlement("finalizedSuccess", True, True, "block-101", 101),
            )
        )
        events.append(
            sample(
                alias,
                "deploy_status",
                2,
                "2026-11-14T10:11:00Z",
                payload={
                    "ProcessedWithSuccess": {
                        "deployResult": [],
                        "block": {"blockHash": "block-101", "blockNumber": 101},
                    }
                },
            )
        )
    return events


class LiveGateFixture(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory(dir=ROOT / "target")
        self.addCleanup(self._tmp.cleanup)
        self.base = Path(self._tmp.name)
        self.run_dir = self.base / "run"
        self.run_dir.mkdir()
        self.preflight_path = self.base / "preflight.json"
        self.write_bundle()

    def write_bundle(self, events=None, preflight_doc=None):
        self.preflight_path.write_text(json.dumps(preflight_doc or preflight()))
        (self.run_dir / "run.json").write_text(
            json.dumps(
                {
                    "schema": 1,
                    "tool": "live-gate",
                    "deploy_id": DEPLOY,
                    "started_utc": "2026-11-14T10:00:00Z",
                    "ended_utc": "2026-11-14T10:12:00Z",
                    "validators": [
                        {"alias": alias, "endpoint_sha256": hashlib.sha256(alias.encode()).hexdigest()}
                        for alias in ALIASES
                    ],
                    "counts": {},
                    "execution_proven": False,
                }
            )
        )
        (self.run_dir / "submission.json").write_text(json.dumps(submission()))
        (self.run_dir / "claims.json").write_text(json.dumps(claims()))
        (self.run_dir / "recovery.json").write_text(json.dumps(recovery()))
        events = good_events() if events is None else events
        samples = self.run_dir / "samples.jsonl"
        samples.write_text("\n".join(json.dumps(event, sort_keys=True) for event in events) + "\n")
        self.write_sums()
        return events

    def write_sums(self, names=None):
        names = names or (
            "samples.jsonl",
            "run.json",
            "submission.json",
            "claims.json",
            "recovery.json",
        )
        (self.run_dir / "SHA256SUMS").write_text(
            "".join(
                f"{hashlib.sha256((self.run_dir / name).read_bytes()).hexdigest()}  {name}\n"
                for name in names
            )
        )

    def check(self):
        report = gate.evaluate(self.run_dir, self.preflight_path)
        code = gate.cmd_check(
            SimpleNamespace(run_dir=str(self.run_dir), preflight=str(self.preflight_path),
                            report=None, verdict=None)
        )
        return report, code

    def events(self):
        return [
            json.loads(line)
            for line in (self.run_dir / "samples.jsonl").read_text().splitlines()
            if line.strip()
        ]

    def rewrite_events(self, events):
        samples = self.run_dir / "samples.jsonl"
        samples.write_text("\n".join(json.dumps(event, sort_keys=True) for event in events) + "\n")
        self.write_sums()

    def assert_status(self, report, name, expected):
        self.assertEqual(report["criteria"][name]["status"], expected, report["criteria"][name]["reasons"])


class GoodBundleTests(LiveGateFixture):
    def test_the_good_bundle_is_a_complete_worksheet_and_not_a_verdict(self):
        report, code = self.check()
        for name in ("G1", "G2", "G3", "G4", "G5"):
            self.assert_status(report, name, "PASS")
        self.assertEqual(code, 0)
        self.assertEqual(report["G6"]["status"], "PENDING")
        self.assertIsNone(report["G6"]["reviewer"])
        self.assertEqual(report["overall"], "NOT_A_VERDICT")
        self.assertIs(report["acceptance"], False)
        self.assertIs(report["execution_proven"], False)


class PackedRecordTests(LiveGateFixture):
    def test_a_packed_record_resolves_its_inputs_under_raw(self):
        pack_spec = importlib.util.spec_from_file_location("evidence_pack", ROOT / "tools/evidence-pack.py")
        pack = importlib.util.module_from_spec(pack_spec)
        pack_spec.loader.exec_module(pack)
        record_tmp = tempfile.TemporaryDirectory()  # evidence-pack refuses a record under target/
        self.addCleanup(record_tmp.cleanup)
        record = Path(record_tmp.name) / "record"
        self.assertEqual(
            pack.cmd_pack(
                SimpleNamespace(
                    run_dir=str(self.run_dir),
                    record=str(record),
                    gate_id="n487-live",
                    verdict="PASS",
                    source_sha=SOURCE,
                    binary_sha256=BINARY,
                    run_url=None,
                    locality="public-live",
                    redact=[],
                    move=False,
                )
            ),
            0,
        )
        report = gate.evaluate(record, self.preflight_path)
        self.assert_status(report, "G5", "PASS")
        self.assertEqual(
            gate.cmd_check(SimpleNamespace(run_dir=str(record), preflight=str(self.preflight_path),
                                           report=None, verdict=None)),
            0,
        )


class G1Tests(LiveGateFixture):
    def test_two_binary_digests_fail_the_pin_criterion(self):
        document = preflight()
        document["validators"][1]["binary_sha256"] = "9" * 64
        self.write_bundle(preflight_doc=document)
        report, code = self.check()
        self.assert_status(report, "G1", "FAIL")
        self.assertEqual(code, 1)

    def test_an_unset_operator_field_is_incomplete_not_a_pass(self):
        document = preflight()
        document["operator_fields"]["independent_reviewer"] = None
        self.write_bundle(preflight_doc=document)
        report, code = self.check()
        self.assert_status(report, "G1", "INCOMPLETE")
        self.assertEqual(code, 3)

    def test_the_reviewer_may_not_be_the_operator(self):
        document = preflight()
        document["operator_fields"]["independent_reviewer"] = "operator"
        self.write_bundle(preflight_doc=document)
        report, _ = self.check()
        self.assert_status(report, "G1", "FAIL")

    def test_three_records_are_incomplete(self):
        document = preflight()
        document["validators"] = document["validators"][:3]
        self.write_bundle(preflight_doc=document)
        report, _ = self.check()
        self.assert_status(report, "G1", "INCOMPLETE")


    def test_agreeing_records_that_are_not_the_selected_candidate_fail(self):
        document = preflight()
        for validator in document["validators"]:
            validator["binary_sha256"] = "9" * 64
        self.write_bundle(preflight_doc=document)
        report, code = self.check()
        self.assert_status(report, "G1", "FAIL")
        self.assertEqual(code, 1)

    def test_a_missing_selected_binary_is_incomplete(self):
        document = preflight()
        del document["operator_fields"]["selected_binary_sha256"]
        self.write_bundle(preflight_doc=document)
        report, code = self.check()
        self.assert_status(report, "G1", "INCOMPLETE")
        self.assertEqual(code, 3)

    def test_a_missing_deploy_relay_pin_is_incomplete(self):
        document = preflight()
        del document["operator_fields"]["deploy_relay"]
        self.write_bundle(preflight_doc=document)
        report, code = self.check()
        self.assert_status(report, "G1", "INCOMPLETE")
        self.assertEqual(code, 3)

    def test_an_unreachable_relay_arm_is_a_failure(self):
        document = preflight()
        document["operator_fields"]["deploy_relay"]["unreachable_aliases"] = ["v4"]
        self.write_bundle(preflight_doc=document)
        report, code = self.check()
        self.assert_status(report, "G1", "FAIL")
        self.assertEqual(code, 1)

    def test_deploy_relay_aliases_must_name_the_records(self):
        document = preflight()
        document["operator_fields"]["deploy_relay"]["aliases"] = ["v1"]
        self.write_bundle(preflight_doc=document)
        report, _ = self.check()
        self.assert_status(report, "G1", "FAIL")

    def test_selected_artifact_digest_must_be_a_64_hex_digest(self):
        document = preflight()
        document["operator_fields"]["selected_artifact_digest"] = "6" * 63
        self.write_bundle(preflight_doc=document)
        report, code = self.check()
        self.assert_status(report, "G1", "FAIL")
        self.assertEqual(code, 1)

    def test_a_missing_boot_id_is_incomplete(self):
        document = preflight()
        del document["validators"][2]["boot_id"]
        self.write_bundle(preflight_doc=document)
        report, code = self.check()
        self.assert_status(report, "G1", "INCOMPLETE")
        self.assertEqual(code, 3)


class G2Tests(LiveGateFixture):
    def test_a_missed_pooled_stage_is_incomplete_not_a_pass(self):
        events = [event for event in good_events() if not (event["route"] == "deploy_settlement" and event["body"]["stage"] == "pooled")]
        self.write_bundle(events=events)
        report, code = self.check()
        self.assert_status(report, "G2", "INCOMPLETE")
        self.assertEqual(code, 3)

    def test_never_finalized_is_incomplete_while_g3_still_sees_no_false_claim(self):
        events = [
            event
            for event in good_events()
            if not (
                event["body"]
                and isinstance(event["body"], dict)
                and event["body"].get("stage") in ("finalizedSuccess",)
            )
        ]
        self.write_bundle(events=events)
        (self.run_dir / "claims.json").write_text(json.dumps(claims(settled=False, basis="notFinalized")))
        self.write_sums()
        report, code = self.check()
        self.assert_status(report, "G2", "INCOMPLETE")
        self.assert_status(report, "G3", "PASS")
        self.assertEqual(code, 3)

    def test_one_alias_missing_pooled_is_incomplete(self):
        events = [
            event for event in good_events()
            if not (event.get("alias") == "v2"
                    and event["route"] == "deploy_settlement"
                    and event["body"].get("stage") == "pooled")
        ]
        self.write_bundle(events=events)
        report, code = self.check()
        self.assert_status(report, "G2", "INCOMPLETE")
        self.assertEqual(code, 3)

    def test_two_inclusion_blocks_fail_the_lifecycle(self):
        events = self.events()
        for event in events:
            if event["route"] == "deploy_settlement" and event["alias"] == "v2" and event["body"]["blockHash"]:
                event["body"]["blockHash"] = "other-block"
                event["body_raw"] = json.dumps(event["body"], sort_keys=True)
                event["body_sha256"] = hashlib.sha256(event["body_raw"].encode()).hexdigest()
        self.rewrite_events(events)
        report, _ = self.check()
        self.assert_status(report, "G2", "FAIL")


class G3Tests(LiveGateFixture):
    def test_the_487_shape_fails_when_the_record_calls_execution_settlement(self):
        (self.run_dir / "claims.json").write_text(json.dumps(claims(settled=True, basis="processWithSuccess")))
        self.write_sums()
        report, code = self.check()
        self.assert_status(report, "G3", "FAIL")
        self.assertEqual(code, 1)

    def test_confirmed_on_an_unfinalized_stage_fails(self):
        events = self.events()
        for event in events:
            if event["route"] == "deploy_settlement" and event["body"]["stage"] == "includedUnfinalized":
                event["body"]["settlementConfirmed"] = True
                event["body_raw"] = json.dumps(event["body"], sort_keys=True)
                event["body_sha256"] = hashlib.sha256(event["body_raw"].encode()).hexdigest()
                break
        self.rewrite_events(events)
        report, _ = self.check()
        self.assert_status(report, "G3", "FAIL")

    def test_settled_below_the_inclusion_height_fails(self):
        events = self.events()
        for event in events:
            if event["route"] == "finality" and event["alias"] == "v1" and event["tick"] == 2:
                event["body"]["finalizedBlockNumber"] = 100
                event["body_raw"] = json.dumps(event["body"], sort_keys=True)
                event["body_sha256"] = hashlib.sha256(event["body_raw"].encode()).hexdigest()
        self.rewrite_events(events)
        report, _ = self.check()
        self.assert_status(report, "G3", "FAIL")

    def test_contradictory_finalized_hashes_at_one_height_fail(self):
        events = self.events()
        for event in events:
            if event["route"] == "finality" and event["alias"] == "v2" and event["tick"] == 2:
                event["body"]["finalizedBlockHash"] = "different"
                event["body_raw"] = json.dumps(event["body"], sort_keys=True)
                event["body_sha256"] = hashlib.sha256(event["body_raw"].encode()).hexdigest()
        self.rewrite_events(events)
        report, _ = self.check()
        self.assert_status(report, "G3", "FAIL")

    def test_a_finality_route_that_claims_quorum_fails(self):
        events = self.events()
        for event in events:
            if event["route"] == "finality":
                event["body"]["availability"] = "healthy"
                event["body_raw"] = json.dumps(event["body"], sort_keys=True)
                event["body_sha256"] = hashlib.sha256(event["body_raw"].encode()).hexdigest()
                break
        self.rewrite_events(events)
        report, _ = self.check()
        self.assert_status(report, "G3", "FAIL")


    def test_no_pre_finality_negative_sample_is_incomplete(self):
        events = [
            event for event in good_events()
            if not (event["route"] == "deploy_settlement"
                    and event["body"].get("finalized") is not True)
        ]
        self.write_bundle(events=events)
        report, code = self.check()
        self.assert_status(report, "G3", "INCOMPLETE")
        self.assertEqual(code, 3)


class G4Tests(LiveGateFixture):
    def test_no_recovery_arm_is_incomplete(self):
        (self.run_dir / "recovery.json").unlink()
        self.write_sums(names=("samples.jsonl", "run.json", "submission.json", "claims.json"))
        report, code = self.check()
        self.assert_status(report, "G4", "INCOMPLETE")
        self.assertEqual(code, 3)

    def test_lost_accepted_work_fails(self):
        document = recovery()
        document["accepted_deploys_after"] = [DEPLOY]
        document["lost_deploys"] = [OTHER]
        (self.run_dir / "recovery.json").write_text(json.dumps(document))
        self.write_sums()
        report, _ = self.check()
        self.assert_status(report, "G4", "FAIL")

    def test_a_halt_on_a_healthy_key_fails(self):
        document = recovery()
        document["own_key_halt_observed"] = True
        (self.run_dir / "recovery.json").write_text(json.dumps(document))
        self.write_sums()
        report, _ = self.check()
        self.assert_status(report, "G4", "FAIL")

    def test_a_claimed_post_recovery_settlement_without_a_sample_is_incomplete(self):
        events = [
            event for event in good_events()
            if not (event["route"] == "deploy_settlement"
                    and event["body"].get("stage") == "finalizedSuccess")
        ]
        self.write_bundle(events=events)
        (self.run_dir / "claims.json").write_text(
            json.dumps(claims(settled=False, basis="notFinalized")))
        self.write_sums()
        report, code = self.check()
        self.assert_status(report, "G4", "INCOMPLETE")
        self.assertEqual(code, 3)

    def test_an_unfinalized_recovery_fails(self):
        document = recovery()
        document["same_deploy_finalized"] = False
        document.pop("post_recovery_settlement")
        (self.run_dir / "recovery.json").write_text(json.dumps(document))
        self.write_sums()
        report, _ = self.check()
        self.assert_status(report, "G4", "FAIL")


class G5Tests(LiveGateFixture):
    def test_a_missing_validator_is_incomplete(self):
        events = [event for event in self.events() if event["alias"] != "v4"]
        self.rewrite_events(events)
        report, _ = self.check()
        self.assert_status(report, "G5", "INCOMPLETE")

    def test_a_tampered_body_fails(self):
        events = self.events()
        events[0]["body_raw"] = events[0]["body_raw"] + " "
        self.rewrite_events(events)
        report, _ = self.check()
        self.assert_status(report, "G5", "FAIL")

    def test_duplicate_endpoints_fail(self):
        document = json.loads((self.run_dir / "run.json").read_text())
        document["validators"][1]["endpoint_sha256"] = document["validators"][0]["endpoint_sha256"]
        (self.run_dir / "run.json").write_text(json.dumps(document))
        self.write_sums()
        report, _ = self.check()
        self.assert_status(report, "G5", "FAIL")

    def test_an_endpoint_in_run_json_fails(self):
        document = json.loads((self.run_dir / "run.json").read_text())
        document["note"] = "http://validator.example:40403"
        (self.run_dir / "run.json").write_text(json.dumps(document))
        self.write_sums()
        report, _ = self.check()
        self.assert_status(report, "G5", "FAIL")

    def test_an_unhashed_run_is_incomplete(self):
        (self.run_dir / "SHA256SUMS").unlink()
        report, _ = self.check()
        self.assert_status(report, "G5", "INCOMPLETE")

    def test_an_uncovered_record_file_is_incomplete(self):
        self.write_sums(names=("samples.jsonl",))
        report, _ = self.check()
        self.assert_status(report, "G5", "INCOMPLETE")

    def test_a_digest_mismatch_fails(self):
        samples = self.run_dir / "samples.jsonl"
        samples.write_text(samples.read_text() + "{}\n")
        report, _ = self.check()
        self.assert_status(report, "G5", "FAIL")


class G6Tests(LiveGateFixture):
    def verdict(self, **overrides):
        record = {
            "reviewer": "reviewer",
            "relationship": "not the operator or release owner",
            "independent": True,
            "verdict": "PASS",
            "evidence_sha256": "7" * 64,
            "verdict_url": "https://example.invalid/verdict",
            "signed_at": "2026-11-14T10:20:00Z",
            "evidence_record": str(self.run_dir),
        }
        record.update(overrides)
        path = self.base / "verdict.json"
        path.write_text(json.dumps(record))
        return path

    def test_a_typed_g6_record_is_checked_but_not_recorded(self):
        path = self.verdict()
        report = gate.evaluate(self.run_dir, self.preflight_path, path)
        self.assertEqual(report["G6"]["status"], "PASS")
        self.assertEqual(report["G6"]["reviewer"], "reviewer")
        self.assertEqual(report["overall"], "NOT_A_VERDICT")
        self.assertIs(report["acceptance"], False)
        code = gate.cmd_check(SimpleNamespace(
            run_dir=str(self.run_dir), preflight=str(self.preflight_path),
            report=None, verdict=str(path)))
        self.assertEqual(code, 0)

    def test_g6_reviewer_must_be_the_named_reviewer(self):
        path = self.verdict(reviewer="someone-else")
        report = gate.evaluate(self.run_dir, self.preflight_path, path)
        self.assertEqual(report["G6"]["status"], "FAIL")
        code = gate.cmd_check(SimpleNamespace(
            run_dir=str(self.run_dir), preflight=str(self.preflight_path),
            report=None, verdict=str(path)))
        self.assertEqual(code, 1)

    def test_a_missing_g6_record_is_incomplete(self):
        report = gate.evaluate(self.run_dir, self.preflight_path, self.base / "missing.json")
        self.assertEqual(report["G6"]["status"], "INCOMPLETE")
        code = gate.cmd_check(SimpleNamespace(
            run_dir=str(self.run_dir), preflight=str(self.preflight_path),
            report=None, verdict=str(self.base / "missing.json")))
        self.assertEqual(code, 3)


class ParserTests(unittest.TestCase):
    def test_the_default_sample_cadence_is_one_second(self):
        args = gate.build_parser().parse_args(["sample", "--plan", "p", "--out", "o"])
        self.assertEqual(args.interval, 1.0)


class SamplerTests(unittest.TestCase):
    def serve(self, statuses=None, methods=None):
        statuses = statuses or {}
        methods = methods if methods is not None else []

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                methods.append("GET")
                status = statuses.get(self.path, 200)
                payload = {
                    "/health": {"ready": True, "acceptsDeploys": True},
                    "/api/status": {"acceptsDeploys": True},
                    "/api/last-finalized-block": {"blockNumber": 102, "blockHash": "tip"},
                    "/api/v1/finality-observation": finality(101, "block-101", "", 0),
                }.get(self.path.split("/api/v1/deploy-")[0], {"stage": "pooled", "settlementConfirmed": False, "finalized": None})
                if self.path.startswith("/api/v1/deploy-"):
                    payload = {"stage": "pooled", "settlementConfirmed": False, "finalized": None}
                raw = json.dumps(payload).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(raw)))
                self.end_headers()
                self.wfile.write(raw)

            def do_POST(self):
                methods.append("POST")
                self.send_error(405)

            def log_message(self, *args):
                pass

        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(thread.join, 5)
        self.addCleanup(server.shutdown)
        return server.server_address[1], methods

    def plan(self, ports):
        return {
            "schema": 1,
            "deploy_id": DEPLOY,
            "validators": [
                {"alias": alias, "base": f"http://127.0.0.1:{port}"}
                for alias, port in zip(ALIASES, ports)
            ],
        }

    def tempdir(self):
        directory = tempfile.TemporaryDirectory(dir=ROOT / "target")
        self.addCleanup(directory.cleanup)
        return Path(directory.name)

    def run_sample(self, plan, out, duration=0.25):
        path = out.parent / "plan.json"
        path.write_text(json.dumps(plan))
        return gate.cmd_sample(
            SimpleNamespace(plan=str(path), out=str(out), interval=0.05, duration=duration, timeout=2.0)
        )

    def test_sampling_is_get_only_and_records_every_answer(self):
        ports = []
        methods = []
        for index in range(4):
            port, _ = self.serve({"/health": 503} if index == 3 else {}, methods)
            ports.append(port)
        out = self.tempdir() / "run"
        code = self.run_sample(self.plan(ports), out)
        self.assertEqual(code, 0)
        self.assertTrue(all(method == "GET" for method in methods))
        with gzip.open(out / "samples.jsonl.gz", "rt", encoding="utf-8") as stream:
            events = [json.loads(line) for line in stream if line.strip()]
        self.assertTrue(events)
        self.assertTrue(all(event["http_status"] for event in events))
        self.assertTrue(any(event["http_status"] == 503 for event in events))
        self.assertTrue(all(event["body_sha256"] == hashlib.sha256(event["body_raw"].encode()).hexdigest() for event in events))
        run = json.loads((out / "run.json").read_text())
        self.assertIs(run["execution_proven"], False)
        public = json.loads((out / "plan.public.json").read_text())
        self.assertNotIn("base", json.dumps(public))

    def test_a_validator_that_never_answers_exits_three(self):
        ports = [self.serve()[0] for _ in range(3)]
        ports.append(1)  # nothing listens
        out = self.tempdir() / "run"
        code = self.run_sample(self.plan(ports), out, duration=0.1)
        self.assertEqual(code, 3)

    def test_the_sampler_refuses_four_names_for_one_endpoint(self):
        port = self.serve()[0]
        out = self.tempdir() / "run"
        with self.assertRaises(SystemExit):
            self.run_sample(self.plan([port, port, port, port]), out)

    def test_the_sampler_refuses_a_non_empty_run_directory(self):
        port = self.serve()[0]
        out = self.tempdir() / "run"
        out.mkdir()
        (out / "old.json").write_text("{}")
        with self.assertRaises(SystemExit):
            self.run_sample(self.plan([port, port + 1, port + 2, port + 3]), out)


if __name__ == "__main__":
    unittest.main()
