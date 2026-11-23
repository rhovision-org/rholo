"""Offline proofs for the release-arm runners that consume an operator artifact.

The arms these runners serve need hardware this host cannot supply. The runners are the last mile:
they map a producer's artifact into the record the arm's own pure predicate reads. These tests use
the real producers where one exists and hand-built artifacts where the producer is an operator's
two-host run, and they prove both directions of the false-pass boundary:

* a missing coordinate or a local address is a **precondition** (`not run`), never a pass;
* a readable negative observation reaches the predicate as a **failure**.

No devnet, no Docker, no committed evidence.
"""
from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
_COMMON_SPEC = importlib.util.spec_from_file_location(
    "release_claim_common", ROOT / "tests" / "release_claim_common.py")
common = importlib.util.module_from_spec(_COMMON_SPEC)
_COMMON_SPEC.loader.exec_module(common)

TITLE = "Recovery without loss"
REMOTE_A = "host-a.example:40403"
REMOTE_B = "host-b.example:40403"


def failover_document(**overrides):
    document = {
        "seeds": [REMOTE_A, REMOTE_B],
        "seed_status": ["down", "live"],
        "joined": True,
        "joined_via": REMOTE_B,
        "finalised_head_agrees": True,
    }
    document.update(overrides)
    return document


class FailoverRunnerTests(unittest.TestCase):
    def run_arm(self, document, *, log=True, with_file=True):
        with tempfile.TemporaryDirectory() as root:
            run = Path(root) / "run"
            run.mkdir()
            if with_file:
                (run / "failover.json").write_text(json.dumps(document))
            if log:
                (run / "joiner.log").write_text("bootstrap seed 2/2 connected: host-b.example\n")
            record = common.new_record("RC-4", TITLE, "local", "off-host-claim", arm="failover")
            code = common.run_failover_arm(Path(root) / "out", record, run)
            return code, record

    def test_a_down_first_seed_and_a_live_second_passes(self):
        code, record = self.run_arm(failover_document())
        self.assertEqual(code, 0, record["reasons"])
        self.assertEqual(record["verdict"], "pass")
        self.assertEqual(record["observations"]["failover"]["joined_via"], REMOTE_B)

    def test_a_reversed_seed_order_fails(self):
        code, record = self.run_arm(failover_document(seed_status=["live", "down"]))
        self.assertEqual(code, 1)
        self.assertTrue(any("down-then-live" in reason for reason in record["reasons"]))

    def test_a_joiner_that_did_not_join_fails(self):
        code, record = self.run_arm(failover_document(joined=False))
        self.assertEqual(code, 1)
        self.assertTrue(any("did not join" in reason for reason in record["reasons"]))

    def test_one_seed_is_not_an_ordered_list(self):
        code, record = self.run_arm(failover_document(seeds=[REMOTE_A]))
        self.assertEqual(code, 3)
        self.assertTrue(any("at least two seeds" in reason for reason in record["reasons"]))

    def test_a_missing_coordinate_is_not_run(self):
        document = failover_document()
        document.pop("joined_via")
        code, record = self.run_arm(document)
        self.assertEqual(code, 3)
        self.assertTrue(any("joined_via" in reason for reason in record["reasons"]))

    def test_a_missing_joiner_log_is_not_run(self):
        code, record = self.run_arm(failover_document(), log=False)
        self.assertEqual(code, 3)
        self.assertTrue(any("joiner.log" in reason for reason in record["reasons"]))

    def test_a_missing_artifact_is_not_run(self):
        code, record = self.run_arm(failover_document(), with_file=False)
        self.assertEqual(code, 3)
        self.assertTrue(any("missing" in reason for reason in record["reasons"]))

    def test_a_loopback_seed_is_not_an_off_host_claim(self):
        for seed in ("127.0.0.1:40403", "localhost:40403", "[::1]:40403", "::1", "10.0.0.5:40403"):
            with self.subTest(seed=seed):
                code, record = self.run_arm(
                    failover_document(seeds=[seed, REMOTE_B]))
                self.assertEqual(code, 3, record["reasons"])
                self.assertTrue(any("local seed" in reason for reason in record["reasons"]))


def _load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


m4 = _load("m4_campaign_fixture", ROOT / "tests" / "test_m4_campaign.py")
HOSTS8 = [f"host-{letter}" for letter in "abcdefgh"]
SHARED_ROOT = {"n4": "c" * 64, "n8": "c" * 64}


def release_pairs():
    return {
        "n4": [(f"v{index}", HOSTS8[index - 1]) for index in range(1, 5)],
        "n8": [(f"v{index}", HOSTS8[index - 1]) for index in range(1, 9)],
    }


def collect_campaign(root, *, pairs=None, hosts=None, approved=None) -> Path:
    """Run the real M4 collector over a synthetic campaign."""
    campaign = m4.Campaign(Path(root), pairs=pairs or release_pairs(),
                           hosts=hosts or HOSTS8, approved=approved or SHARED_ROOT)
    declaration = campaign.declaration()
    artifact = Path(root) / "observations.json"
    result = subprocess.run(
        [sys.executable, str(m4.TOOL), "collect", "--declaration", str(declaration),
         "--out", str(artifact)],
        capture_output=True, text=True)
    if result.returncode:
        raise AssertionError(result.stdout + result.stderr)
    return artifact


class MultiHostRunnerTests(unittest.TestCase):
    def run_arm(self, artifact, claim="RC-2", head=None):
        with tempfile.TemporaryDirectory() as root:
            record = common.new_record(claim, "Safety" if claim == "RC-2" else TITLE,
                                       "local", "multi-host", arm="multi-host")
            code = common.run_multi_host_arm(Path(root) / "out", record, artifact, head=head)
            return code, record

    def mutate(self, artifact: Path, change):
        document = json.loads(artifact.read_text())
        change(document["record_data"])
        artifact.write_text(json.dumps(document))
        return artifact

    def test_an_eight_host_campaign_passes_both_claims(self):
        with tempfile.TemporaryDirectory() as root:
            artifact = collect_campaign(root)
            for claim in ("RC-2", "RC-4"):
                code, record = self.run_arm(artifact, claim)
                self.assertEqual(code, 0, (claim, record["reasons"]))
                self.assertEqual(record["observations"]["multi_host"]["declared_hosts"], HOSTS8)

    def test_a_four_host_campaign_fails_the_distinct_host_rule(self):
        with tempfile.TemporaryDirectory() as root:
            artifact = collect_campaign(
                root, pairs=m4.PAIRS,
                hosts=["host-a", "host-b", "host-c", "host-d"], approved=SHARED_ROOT)
            code, record = self.run_arm(artifact)
            self.assertEqual(code, 1)
            self.assertTrue(any("distinct declared hosts" in reason
                                for reason in record["reasons"]), record["reasons"])

    def test_workloads_with_different_roots_are_not_run(self):
        with tempfile.TemporaryDirectory() as root:
            artifact = collect_campaign(root, approved={"n4": "a" * 64, "n8": "b" * 64})
            code, record = self.run_arm(artifact)
            self.assertEqual(code, 3)
            self.assertTrue(any("approved root" in reason for reason in record["reasons"]))

    def test_a_wrong_head_pin_is_not_run(self):
        with tempfile.TemporaryDirectory() as root:
            artifact = collect_campaign(root)
            code, record = self.run_arm(artifact, head="f" * 40)
            self.assertEqual(code, 3)
            self.assertTrue(any("not the requested" in reason for reason in record["reasons"]))

    def test_a_lost_deploy_fails_both_claims(self):
        with tempfile.TemporaryDirectory() as root:
            artifact = collect_campaign(root)

            def lost(data):
                data["workloads"]["n8"]["arms"]["safe_recovery"]["accepted_work_lost"] = 1

            self.mutate(artifact, lost)
            for claim in ("RC-2", "RC-4"):
                code, record = self.run_arm(artifact, claim)
                self.assertEqual(code, 1, (claim, record["reasons"]))

    def test_an_own_key_halt_fails_the_recovery_arm(self):
        with tempfile.TemporaryDirectory() as root:
            artifact = collect_campaign(root)

            def halt(data):
                data["workloads"]["n8"]["arms"]["safe_recovery"]["own_key_halt"] = True

            self.mutate(artifact, halt)
            code, record = self.run_arm(artifact, "RC-4")
            self.assertEqual(code, 1)
            self.assertTrue(any("own-key halt" in reason for reason in record["reasons"]))

    def test_a_false_anchor_retention_is_conflicting_finality(self):
        with tempfile.TemporaryDirectory() as root:
            artifact = collect_campaign(root)

            def lose_anchor(data):
                cycle = data["workloads"]["n4"]["arms"]["partition_rejoin"]["cycles"][0]
                cycle["validators"][0]["finalized_anchor"] = False

            self.mutate(artifact, lose_anchor)
            code, record = self.run_arm(artifact)
            self.assertEqual(code, 1)
            self.assertTrue(record["observations"]["multi_host"]["conflicting_finality"])

    def test_a_missing_artifact_is_not_run(self):
        with tempfile.TemporaryDirectory() as root:
            code, record = self.run_arm(Path(root) / "absent.json")
            self.assertEqual(code, 3)
            self.assertTrue(any("missing" in reason for reason in record["reasons"]))


fixtures = _load("m3_gate_fixtures", ROOT / "tests" / "gate_fixtures.py")
m3 = _load("m3_join_chain_tool", ROOT / "tools" / "m3-join-chain.py")
PUBLIC_A = "93.184.216.34"
PUBLIC_B = "93.184.216.35"


def offhost_run(root, *, advertised_host=PUBLIC_A, host_a=PUBLIC_A, host_b=PUBLIC_B,
                refused=True) -> Path:
    """Build a real M3 chain from synthetic logs and the operator's companion files."""
    run = Path(root) / "run"
    run.mkdir()
    chain = fixtures.m3_chain(advertised_host=advertised_host)
    joiner, responder = fixtures.m3_logs(chain)
    (run / "joiner.log").write_text(joiner)
    (run / "responder.log").write_text(responder)
    built = m3.build_chain(joiner_log=joiner, responder_log=responder,
                           locality="off-host-claim")
    (run / "chain.json").write_text(json.dumps(built))
    (run / "declaration.json").write_text(json.dumps({"host_a": host_a, "host_b": host_b}))
    (run / "negative").mkdir()
    (run / "negative" / "loopback.json").write_text(json.dumps(
        {"advertised": "127.0.0.1:40403", "refused": refused}))
    return run


class OffHostRunnerTests(unittest.TestCase):
    def run_arm(self, run, out=None):
        record = common.new_record("RC-6", "Join and audit", "local", "off-host-claim",
                                   arm="off-host")
        out = out or Path(run).parent / "out"
        code = common.run_offhost_arm(out, record, run)
        return code, record, out

    def test_a_complete_chain_passes(self):
        with tempfile.TemporaryDirectory() as root:
            run = offhost_run(root)
            code, record, out = self.run_arm(run, Path(root) / "out")
            self.assertEqual(code, 0, record["reasons"])
            self.assertTrue((out / "raw" / "chain.json").is_file())
            self.assertTrue((out / "raw" / "joiner.log").is_file())

    def test_each_ipv6_loopback_literal_is_refused(self):
        for advertised in ("::1", "[::1]:40403"):
            with self.subTest(advertised=advertised):
                with tempfile.TemporaryDirectory() as root:
                    run = offhost_run(root, advertised_host=advertised)
                    code, record, _ = self.run_arm(run)
                    self.assertEqual(code, 3, record["reasons"])
                    self.assertTrue(any("local" in reason for reason in record["reasons"]))

    def test_a_private_advertised_host_is_refused(self):
        with tempfile.TemporaryDirectory() as root:
            run = offhost_run(root, advertised_host="10.0.0.5")
            code, record, _ = self.run_arm(run)
            self.assertEqual(code, 3, record["reasons"])

    def test_the_same_host_twice_is_refused(self):
        with tempfile.TemporaryDirectory() as root:
            run = offhost_run(root, host_b=PUBLIC_A)
            code, record, _ = self.run_arm(run)
            self.assertEqual(code, 3)
            self.assertTrue(any("same host" in reason for reason in record["reasons"]))

    def test_an_incomplete_chain_is_not_run(self):
        with tempfile.TemporaryDirectory() as root:
            run = offhost_run(root)
            chain = json.loads((run / "chain.json").read_text())
            chain["complete"] = False
            chain["incomplete_reasons"] = ["5_receive is absent"]
            (run / "chain.json").write_text(json.dumps(chain))
            code, record, _ = self.run_arm(run)
            self.assertEqual(code, 3)
            self.assertTrue(any("incomplete" in reason for reason in record["reasons"]))

    def test_a_hash_disagreement_fails(self):
        with tempfile.TemporaryDirectory() as root:
            run = offhost_run(root)
            chain = json.loads((run / "chain.json").read_text())
            chain["stages"]["5_receive"]["state_hash"] = "b" * 64
            (run / "chain.json").write_text(json.dumps(chain))
            code, record, _ = self.run_arm(run)
            self.assertEqual(code, 1)
            self.assertTrue(any("hashes disagree" in reason for reason in record["reasons"]))

    def test_a_negative_control_that_did_not_refuse_is_not_run(self):
        with tempfile.TemporaryDirectory() as root:
            run = offhost_run(root, refused=False)
            code, record, _ = self.run_arm(run)
            self.assertEqual(code, 3)
            self.assertTrue(any("negative control" in reason for reason in record["reasons"]))


def equivocation_document(**overrides):
    document = {
        "injector": "v1",
        "height": 50,
        "blocks": ["a" * 64, "b" * 64],
        "validators": ["v1", "v2", "v3", "v4"],
        "finalised_by": {ident: ["a" * 64] for ident in ("v1", "v2", "v3", "v4")},
        "detected": True,
        "slashing_enabled": False,
    }
    document.update(overrides)
    return document


class EquivocationRunnerTests(unittest.TestCase):
    def run_arm(self, document):
        with tempfile.TemporaryDirectory() as root:
            artifact = Path(root) / "equivocation.json"
            if document is not None:
                artifact.write_text(json.dumps(document))
            record = common.new_record("RC-2", "Safety", "local", "multi-host",
                                       arm="equivocation")
            code = common.run_equivocation_arm(Path(root) / "out", record, artifact)
            return code, record

    def test_a_safe_history_passes(self):
        code, record = self.run_arm(equivocation_document())
        self.assertEqual(code, 0, record["reasons"])

    def test_a_validator_that_finalised_both_twins_fails(self):
        document = equivocation_document()
        document["finalised_by"]["v2"] = ["a" * 64, "b" * 64]
        code, record = self.run_arm(document)
        self.assertEqual(code, 1)
        self.assertTrue(any("finalised both blocks" in reason for reason in record["reasons"]))

    def test_an_undetected_fault_fails(self):
        code, record = self.run_arm(equivocation_document(detected=False))
        self.assertEqual(code, 1)
        self.assertTrue(any("not detected" in reason for reason in record["reasons"]))

    def test_slashing_still_enabled_fails(self):
        code, record = self.run_arm(equivocation_document(slashing_enabled=True))
        self.assertEqual(code, 1)
        self.assertTrue(any("slashing" in reason for reason in record["reasons"]))

    def test_fewer_than_four_validators_is_not_run(self):
        document = equivocation_document()
        document["validators"] = ["v1", "v2", "v3"]
        code, record = self.run_arm(document)
        self.assertEqual(code, 3)
        self.assertTrue(any("at least four" in reason for reason in record["reasons"]))

    def test_a_missing_policy_is_not_run(self):
        document = equivocation_document()
        document.pop("slashing_enabled")
        code, record = self.run_arm(document)
        self.assertEqual(code, 3)
        self.assertTrue(any("slashing_enabled" in reason for reason in record["reasons"]))


if __name__ == "__main__":
    unittest.main()
