"""The transport arm's pure half.

The arm is one signed deploy read through four submission surfaces. The request is reconstructed
from the node's own `PooledDeploy`, never re-signed; a single field of drift changes the deploy id.
The predicate must require the successful triple — four uniform refusals are agreement, not a
transport pass — and a surface that returns another id must fail.
"""
import importlib.util
import json
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


claim5 = _load("release_claim_5", ROOT / "tests" / "release-claim-5.py")
common = claim5.common

POOLED = {
    "deployId": "ab" * 32,
    "signature": "ab" * 32,
    "sigAlgorithm": "secp256k1",
    "attachments": [],
    "timestamp": 1_700_000_000_000,
    "deployer": "cd" * 33,
    "term": '@"release-claim-transport"!(42)',
    "phloPrice": 1,
    "phloLimit": 1_000_000,
    "validAfterBlockNumber": 1,
    "shardId": "/root",
}


class DeployRequest(unittest.TestCase):
    def test_the_pooled_deploy_reconstructs_the_signed_request(self) -> None:
        request = claim5._deploy_request(POOLED)
        self.assertEqual(request["deployer"], POOLED["deployer"])
        self.assertEqual(request["signature"], POOLED["signature"])
        self.assertEqual(request["sigAlgorithm"], "secp256k1")
        self.assertEqual(request["data"]["term"], POOLED["term"])
        self.assertEqual(request["data"]["timestamp"], POOLED["timestamp"])
        self.assertEqual(request["data"]["phloPrice"], 1)
        self.assertEqual(request["data"]["phloLimit"], 1_000_000)
        self.assertEqual(request["data"]["validAfterBlockNumber"], 1)
        self.assertEqual(request["data"]["shardId"], "/root")
        self.assertNotIn("attachments", request["data"])

    def test_attachments_survive_when_the_deploy_has_them(self) -> None:
        pooled = dict(POOLED, attachments=["deadbeef"])
        self.assertEqual(claim5._deploy_request(pooled)["data"]["attachments"], ["deadbeef"])

    def test_a_tampered_signature_is_a_different_body(self) -> None:
        request = claim5._deploy_request(POOLED)
        tampered = claim5._tampered(request)
        self.assertNotEqual(tampered["signature"], request["signature"])
        self.assertEqual(len(tampered["signature"]), len(request["signature"]))

    def test_the_response_id_is_the_signature(self) -> None:
        body = json.dumps("Success!\nDeployId is: abcdef")
        self.assertEqual(claim5._response_id(body), "abcdef")
        self.assertEqual(claim5._response_id(body.encode()), "abcdef")

    def test_the_captured_body_is_the_request_body(self) -> None:
        raw = b'POST /api/v1/deploy HTTP/1.1\r\nContent-Length: 5\r\n\r\n{"a"}'
        self.assertEqual(claim5._captured_body(raw), b'{"a"}')


def _fixture() -> dict:
    return claim5.fixture("transport")


class TransportPredicate(unittest.TestCase):
    def test_the_fixture_is_a_success(self) -> None:
        verdict, reasons = common.check_transport(_fixture())
        self.assertEqual(verdict, "pass", reasons)

    def test_a_uniform_refusal_is_not_a_pass(self) -> None:
        record = _fixture()
        for row in record["observations"]["transport"]["surfaces"]:
            row.update(status="NotProcessed", settlement_stage="notProcessed",
                       error_class="400")
        verdict, reasons = common.check_transport(record)
        self.assertEqual(verdict, "fail")
        self.assertTrue(any("successful triple" in reason for reason in reasons), reasons)

    def test_a_surface_with_another_id_fails(self) -> None:
        record = _fixture()
        record["observations"]["transport"]["surfaces"][1]["deploy_id"] = "cd" * 32
        verdict, reasons = common.check_transport(record)
        self.assertEqual(verdict, "fail")
        self.assertTrue(any("different deploy id" in reason for reason in reasons), reasons)

    def test_a_missing_surface_fails(self) -> None:
        record = _fixture()
        record["observations"]["transport"]["surfaces"].pop()
        verdict, reasons = common.check_transport(record)
        self.assertEqual(verdict, "fail")


if __name__ == "__main__":
    unittest.main()
