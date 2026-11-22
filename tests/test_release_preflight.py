"""The preflight refuses a run whose inputs cannot support an explanation.

Every case is a `not run` coordinate: the claim has not been tested, so a claim fail would be a
false accusation of the code. The public branch delegates to the fleet manifest contract, so a
manifest that cannot name four independent hosts, the current candidate and the deploy plan is
refused before any claim code runs.
"""
import importlib.util
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "release_claim_common", ROOT / "tests" / "release_claim_common.py")
common = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(common)

HEAD = "a" * 40
DIGEST = "b" * 64


def node(index):
    return {
        "name": f"validator-{index}",
        "host": f"8.8.8.{index + 1}",
        "ssh": f"root@8.8.4.{index + 1}",
        "unit": f"rnode-{index}",
        "binary_path": "/usr/local/bin/rnode",
        "data_dir": f"/var/lib/rnode-{index}",
        "http": f"https://node{index}.example.org",
        "grpc_port": 40401,
    }


def public_manifest(**overrides):
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
        },
        "nodes": [node(index) for index in range(4)],
        "actions": {"epoch_length": 10, "retention_epochs": 2},
        "deploy": {
            "from": "validator-0",
            "marker": "rc-public:20261122",
            "value": 42,
            "term_file": "target/term.rho",
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
    }
    document.update(overrides)
    return document


class Result:
    def __init__(self, returncode=0, stdout=""):
        self.returncode = returncode
        self.stdout = stdout
        self.stderr = ""


def runner(*, docker=0, image=0, label=HEAD, status="", head=HEAD):
    def run(command):
        if command[:2] == ["docker", "info"]:
            return Result(docker)
        if command[:3] == ["docker", "image", "inspect"] and "--format" in command:
            return Result(0 if image == 0 else image, label)
        if command[:3] == ["docker", "image", "inspect"]:
            return Result(image)
        if command[:2] == ["git", "rev-parse"]:
            return Result(0, head)
        if command[:2] == ["git", "status"]:
            return Result(0, status)
        raise AssertionError(f"unexpected command {command}")
    return run


class PreflightTests(unittest.TestCase):
    def test_a_matching_image_and_clean_tree_pass(self):
        self.assertEqual(common.preflight("local", runner=runner(), head=HEAD), [])

    def test_a_missing_image_names_the_build_command(self):
        problems = common.preflight("local", runner=runner(image=1), head=HEAD)
        self.assertTrue(any("missing" in p and "devnet.sh build" in p for p in problems))

    def test_an_unlabelled_image_is_refused(self):
        problems = common.preflight("local", runner=runner(label=""), head=HEAD)
        self.assertTrue(any("revision" in p for p in problems))

    def test_a_stale_image_names_both_revisions(self):
        problems = common.preflight("local", runner=runner(label="b" * 40), head=HEAD)
        self.assertTrue(any("not HEAD" in p for p in problems))

    def test_a_dirty_tree_is_refused_unless_allowed(self):
        problems = common.preflight("local", runner=runner(status=" M casper/src/x.rs"),
                                    head=HEAD)
        self.assertTrue(any("dirty" in p for p in problems))
        with mock.patch.dict(os.environ, {"RELEASE_ALLOW_DIRTY": "1"}):
            self.assertEqual(
                common.preflight("local", runner=runner(status=" M casper/src/x.rs"), head=HEAD), [])

    def test_a_public_run_needs_a_manifest(self):
        problems = common.preflight("public", None, runner=runner(), head=HEAD)
        self.assertTrue(any("--public-manifest" in p for p in problems))

    def test_a_manifest_from_another_revision_is_refused(self):
        document = public_manifest()
        document["provenance"]["source_sha"] = "c" * 40
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "manifest.json"
            path.write_text(json.dumps(document))
            problems = common.preflight("public", str(path), runner=runner(), head=HEAD,
                                        claim_id="RC-1")
        self.assertTrue(any("source_sha" in p for p in problems))

    def test_four_independent_hosts_are_required(self):
        document = public_manifest()
        document["nodes"][2]["host"] = document["nodes"][0]["host"]
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "manifest.json"
            path.write_text(json.dumps(document))
            problems = common.preflight("public", str(path), runner=runner(), head=HEAD,
                                        claim_id="RC-1")
        self.assertTrue(any("not distinct" in p for p in problems))

    def test_an_operator_supplied_result_is_refused(self):
        document = public_manifest()
        document["harness_result"] = "PASS"
        document["victim"] = "validator-2"
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "manifest.json"
            path.write_text(json.dumps(document))
            problems = common.preflight("public", str(path), runner=runner(), head=HEAD,
                                        claim_id="RC-4")
        self.assertTrue(any("harness_result" in p for p in problems))
        self.assertTrue(any("victim" in p for p in problems))

    def test_a_matching_manifest_passes(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "manifest.json"
            path.write_text(json.dumps(public_manifest()))
            self.assertEqual(
                common.preflight("public", str(path), runner=runner(), head=HEAD,
                                 claim_id="RC-1"), [])

    def test_a_rehearsal_manifest_is_accepted_only_as_a_rehearsal(self):
        document = public_manifest(
            transport="docker", locality="local-devnet", rehearsal=True,
            nodes=[{key: value for key, value in node(index).items()
                    if key in ("name", "host", "http", "unit", "grpc_port")}
                   for index in range(4)],
        )
        for index, entry in enumerate(document["nodes"]):
            entry["host"] = f"host-{index}"
            entry["unit"] = "rcfleet-bootstrap" if index == 0 else f"rcfleet-validator-{index}"
        document["deploy"].pop("private_key_path")
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "manifest.json"
            path.write_text(json.dumps(document))
            self.assertEqual(
                common.preflight("public", str(path), runner=runner(), head=HEAD,
                                 claim_id="RC-1", rehearsal=True), [])
            problems = common.preflight("public", str(path), runner=runner(), head=HEAD,
                                        claim_id="RC-1")
        self.assertTrue(any("rehearsal manifest cannot be run" in p for p in problems))


if __name__ == "__main__":
    unittest.main()
