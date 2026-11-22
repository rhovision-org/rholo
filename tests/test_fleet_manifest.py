"""The rc-public-fleet manifest is an input, and it says so.

A manifest is the campaign's four nodes, the candidate pins, the deploy plan and the action policy.
It is not allowed to carry the answer: no `harness_result`, no `victim`, no `boot_ids`, no
`post_heads`, no `certificates_after`, and no pre-made `deploy_id`. A claim that reads its own
outcome from the operator's file is an attestation; these tests make that shape unrepresentable.
"""
import copy
import importlib.util
import json
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "fleet_manifest", ROOT / "tools" / "fleet_manifest.py")
fleet = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(fleet)

HEAD = "a" * 40
DIGEST = "b" * 64


def node(index, **overrides):
    entry = {
        "name": f"validator-{index}",
        "host": f"8.8.8.{index + 1}",
        "ssh": f"root@8.8.4.{index + 1}",
        "unit": f"rnode-{index}",
        "binary_path": "/usr/local/bin/rnode",
        "data_dir": f"/var/lib/rnode-{index}",
        "http": f"https://node{index}.example.org",
        "admin": f"http://127.0.0.1:{40405 + index * 1000}",
        "grpc_host": "127.0.0.1",
        "grpc_port": 40401,
    }
    entry.update(overrides)
    return entry


def manifest(**overrides):
    document = {
        "schema": 1,
        "campaign": "rc-public-fleet",
        "generated_at_utc": "2026-11-22T00:00:00Z",
        "locality": "public-live",
        "transport": "ssh-systemd",
        "provenance": {
            "source_sha": HEAD,
            "binary_sha256": DIGEST,
            "genesis_hash": "c" * 64,
            "genesis_raw_sha256": "d" * 64,
            "bonds_sha256": "e" * 64,
            "network_id": "testnet",
            "shard_id": "/testnet",
        },
        "nodes": [node(index) for index in range(4)],
        "deploy": {
            "from": "validator-0",
            "marker": "rc-public:20261122",
            "value": 42,
            "term_file": "target/rc-public-fleet/term.rho",
            "term_sha256": "f" * 64,
            "phlo_limit": 1000000,
            "phlo_price": 1,
            "shard_id": "/testnet",
            "private_key_path": "/etc/rnode/deploy.key",
            "relay": {
                "mode": "relay",
                "aliases": [f"validator-{index}" for index in range(4)],
                "reachability_checked_utc": "2026-11-22T00:00:00Z",
                "unreachable_aliases": [],
            },
        },
        "actions": {
            "survivor_advance_blocks": 3,
            "rejoin_margin_blocks": 3,
            "epoch_length": 10,
            "retention_epochs": 2,
            "timeouts": {"deploy_s": 120},
        },
    }
    document.update(overrides)
    return document


def rehearsal(**overrides):
    nodes = [node(index) for index in range(4)]
    for index, entry in enumerate(nodes):
        entry["host"] = f"host-{index}"
        entry["unit"] = "rcfleet-bootstrap" if index == 0 else f"rcfleet-validator-{index}"
        for field in ("ssh", "binary_path", "data_dir"):
            entry.pop(field, None)
    document = manifest(transport="docker", locality="local-devnet", rehearsal=True, nodes=nodes)
    document["deploy"].pop("private_key_path", None)
    document.update(overrides)
    return document


class ManifestTests(unittest.TestCase):
    def test_a_four_host_public_manifest_passes(self):
        self.assertEqual(fleet.validate(manifest(), claim_id="RC-1", head=HEAD), [])

    def test_an_unknown_key_is_refused(self):
        problems = fleet.validate(manifest(surprise=True), claim_id="RC-1", head=HEAD)
        self.assertTrue(any("unknown keys" in problem for problem in problems))

    def test_three_nodes_are_not_four_hosts(self):
        document = manifest()
        document["nodes"] = document["nodes"][:3]
        problems = fleet.validate(document, claim_id="RC-1", head=HEAD)
        self.assertTrue(any("4 independent hosts" in problem for problem in problems))

    def test_duplicate_declared_hosts_are_refused(self):
        document = manifest()
        document["nodes"][3]["host"] = document["nodes"][0]["host"]
        problems = fleet.validate(document, claim_id="RC-1", head=HEAD)
        self.assertTrue(any("not distinct" in problem for problem in problems))

    def test_loopback_and_private_hosts_are_refused(self):
        for label, reason in (("127.0.0.1", "loopback"), ("10.1.2.3", "private"),
                              ("localhost", "loopback"), ("192.168.1.9", "private")):
            document = manifest()
            document["nodes"][2]["host"] = label
            problems = fleet.validate(document, claim_id="RC-1", head=HEAD)
            self.assertTrue(any(reason in problem for problem in problems), (label, problems))

    def test_a_manifest_from_another_revision_is_refused(self):
        problems = fleet.validate(manifest(), claim_id="RC-1", head=HEAD[:-1] + "b")
        self.assertTrue(any("is not HEAD" in problem for problem in problems))

    def test_public_pins_are_required(self):
        document = manifest()
        for key in ("genesis_hash", "genesis_raw_sha256", "bonds_sha256"):
            document["provenance"].pop(key)
        problems = fleet.validate(document, claim_id="RC-1", head=HEAD)
        self.assertEqual(len([p for p in problems if "provenance." in p]), 3)

    def test_rc4_cannot_read_its_own_result(self):
        for key in ("harness_result", "boot_ids", "victim", "post_heads",
                    "certificates_after", "results"):
            document = manifest()
            document[key] = {"PASS": True}
            problems = fleet.validate(document, claim_id="RC-4", head=HEAD)
            self.assertTrue(any(key in problem for problem in problems), (key, problems))

    def test_rc4_cannot_be_told_the_victim(self):
        document = manifest()
        document["actions"]["victim"] = 2
        problems = fleet.validate(document, claim_id="RC-4", head=HEAD)
        self.assertTrue(any("driver chooses the victim" in problem for problem in problems))

    def test_rc1_cannot_read_a_made_deploy_id(self):
        document = manifest()
        document["deploy"]["deploy_id"] = "ab" * 32
        problems = fleet.validate(document, claim_id="RC-1", head=HEAD)
        self.assertTrue(any("deploy_id" in problem for problem in problems))

    def test_the_deploy_must_reach_every_alias(self):
        document = manifest()
        document["deploy"]["relay"]["aliases"] = ["validator-0"] * 4
        problems = fleet.validate(document, claim_id="RC-1", head=HEAD)
        self.assertTrue(any("exactly the four nodes" in problem for problem in problems))

    def test_an_unreachable_alias_is_not_a_plan(self):
        document = manifest()
        document["deploy"]["relay"]["unreachable_aliases"] = ["validator-3"]
        problems = fleet.validate(document, claim_id="RC-1", head=HEAD)
        self.assertTrue(any("must be empty" in problem for problem in problems))

    def test_four_ingress_needs_a_signed_payload(self):
        document = manifest()
        document["deploy"]["relay"]["mode"] = "four-ingress"
        problems = fleet.validate(document, claim_id="RC-1", head=HEAD)
        self.assertTrue(any("signed_payload_file" in problem for problem in problems))

    def test_a_rehearsal_is_local_and_declared(self):
        self.assertEqual(
            fleet.validate(rehearsal(), claim_id="RC-1", environment="public",
                           head=HEAD, rehearsal=True), [])
        undeclared = rehearsal()
        undeclared.pop("rehearsal")
        problems = fleet.validate(undeclared, claim_id="RC-1", environment="public",
                                  head=HEAD, rehearsal=True)
        self.assertTrue(any("must set rehearsal true" in problem for problem in problems))

    def test_a_rehearsal_cannot_be_run_as_a_public_claim(self):
        problems = fleet.validate(rehearsal(), claim_id="RC-1", environment="public", head=HEAD)
        self.assertTrue(any("cannot be run as a public claim" in problem for problem in problems))

    def test_a_docker_transport_cannot_declare_public_live(self):
        document = rehearsal(locality="public-live")
        problems = fleet.validate(document, claim_id="RC-1", environment="public",
                                  head=HEAD, rehearsal=True)
        self.assertTrue(any("public-live" in problem for problem in problems))

    def test_redaction_removes_the_endpoints_and_keeps_a_list(self):
        redacted = fleet.redact(manifest())
        self.assertTrue(redacted["redactions"])
        for entry in redacted["nodes"]:
            self.assertEqual(entry["host"], "<redacted>")
            self.assertEqual(entry["http"], "<redacted>")
            self.assertEqual(entry["ssh"], "<redacted>")
        self.assertEqual(redacted["deploy"]["private_key_path"], "<redacted>")
        self.assertNotEqual(fleet.manifest_sha256(manifest()),
                            fleet.manifest_sha256(redacted))

    def test_the_digest_is_key_order_independent(self):
        one = manifest()
        two = copy.deepcopy(one)
        two["provenance"] = dict(reversed(list(two["provenance"].items())))
        self.assertEqual(fleet.manifest_sha256(one), fleet.manifest_sha256(two))


if __name__ == "__main__":
    unittest.main()
