"""The seven release claims each carry an executable test; status is measured, not authored.

A gate is a boolean test of the current build. A release claim is one observable node behaviour
with one test under `tests/`, re-runnable on the standard local devnet or on `testnet-4`. There is
no `open`/`owed`/`closed` vocabulary: a claim's status is `pass`, `fail` or `not run`, derived from
the newest committed run record by that test's own `check(record)`.

This suite is offline. It proves the tracker's schema, that every named test exists and passes its
fixture, that a decisive mutation of each fixture fails, and that status derivation recomputes the
verdict instead of trusting the stored one.
"""
import copy
import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CHECKER_SPEC = importlib.util.spec_from_file_location(
    "release_claims", ROOT / "tools/check-release-campaigns.py")
checker = importlib.util.module_from_spec(CHECKER_SPEC)
CHECKER_SPEC.loader.exec_module(checker)
BUILDER_SPEC = importlib.util.spec_from_file_location(
    "build_mainnet_gates", ROOT / "tools/build-mainnet-gates.py")
builder = importlib.util.module_from_spec(BUILDER_SPEC)
BUILDER_SPEC.loader.exec_module(builder)
SOURCE = json.loads((ROOT / "spec/audit/release-claims.json").read_text())
TRACKER = json.loads((ROOT / "spec/audit/release-campaigns.json").read_text())
REGISTER = json.loads((ROOT / "spec/audit/mainnet-gates.json").read_text())

BASELINE_MUTATIONS = {
    "RC-1": lambda record: record["observations"]["finalised"][0].update(marker_present=False),
    "RC-2": lambda record: record["observations"]["containment"][0]["answers"][0].update(
        finalized=False),
    "RC-3": lambda record: record["observations"]["blocks"][0].update(disagreement=True),
    "RC-4": lambda record: record["observations"]["post_heads"][0].update(marker_present=False),
    "RC-5": lambda record: record["observations"].update(verdict="FAIL"),
    "RC-6": lambda record: record["observations"]["audit"].update(verdict="DISAGREE"),
    "RC-7": lambda record: record["observations"]["downgrade"].update(refused=False),
}

ARM_MUTATIONS = {
    ("RC-1", "bond-leave"): lambda record: record["observations"]["bond_leave"].update(
        paid_after_quarantine=False),
    ("RC-2", "multi-host"): lambda record: record["observations"]["multi_host"].update(
        conflicting_finality=["aa" * 32]),
    ("RC-2", "equivocation"): lambda record: record["observations"]["equivocation"][
        "finalised_by"].update({"0": ["aa" * 32, "bb" * 32]}),
    ("RC-2", "validator-set"): lambda record: record["observations"]["validator_set"].update(
        denominators_match=False),
    ("RC-3", "availability"): lambda record: record["observations"]["availability"]["blocks"][0][
        "validators"][0].update(body_present=False),
    ("RC-4", "multi-host"): lambda record: record["observations"]["multi_host"]["recovery"].update(
        lost_deploys=["ab" * 32]),
    ("RC-4", "failover"): lambda record: record["observations"]["failover"].update(joined=False),
    ("RC-4", "returner"): lambda record: record["observations"]["returner"].update(
        heads_agree=False),
    ("RC-5", "transport"): lambda record: record["observations"]["transport"]["surfaces"][0].update(
        settlement_stage="Queued"),
    ("RC-6", "off-host"): lambda record: record["observations"]["off_host"].update(
        loopback_advertisement_refused=False),
    ("RC-6", "availability"): lambda record: record["observations"]["availability"]["blocks"][0][
        "validators"][0].update(state_served=False),
    ("RC-2", "stall-denominator"): lambda record: record["observations"]["stall_series"][2].update(
        finality_advanced=True),
    ("RC-3", "stall-episode"): lambda record: record["observations"]["stall_counters"][
        "restore"]["0"].update(stalled=True),
}


def mutation(claim: str, arm: str):
    if arm in ("local", "public"):
        return BASELINE_MUTATIONS[claim]
    return ARM_MUTATIONS[(claim, arm)]


BASELINE_TRACE_MUTATIONS = {
    "RC-1": ("traces", lambda row: row.update(fork_blocks=["11" * 32]), "fork_ok"),
    "RC-2": ("finality_traces", lambda row: row.update(quorum_total=100), "denominator_ok"),
    "RC-3": ("state_traces", lambda row: row.update(computed_pre="ff" * 32), "pre_ok"),
    "RC-4": ("boot_traces", lambda row: row.update(served_supporting_stake=100),
             "verification_ok"),
    "RC-5": ("admission_traces", lambda row: row.update(retry_after=None), "retry_signal_ok"),
    "RC-6": ("audit_traces", lambda row: row.update(target_hashes=["ab" * 32, "cd" * 32]),
             "target_shape_ok"),
    "RC-7": ("format_traces", lambda row: row.update(decision="accept"), "format_ok"),
}

ARM_TRACE_MUTATIONS = {
    ("RC-1", "bond-leave"): ("traces",
                             lambda row: row.update(fork_blocks=["11" * 32]), "fork_ok"),
    ("RC-2", "multi-host"): ("finality_traces",
                             lambda row: row.update(quorum_total=100), "denominator_ok"),
    ("RC-2", "equivocation"): ("finality_traces",
                               lambda row: row.update(quorum_total=100), "denominator_ok"),
    ("RC-2", "validator-set"): ("finality_traces",
                                lambda row: row.update(quorum_total=100), "denominator_ok"),
    ("RC-3", "availability"): ("state_traces",
                               lambda row: row.update(computed_pre="ff" * 32), "pre_ok"),
    ("RC-4", "multi-host"): ("finality_traces",
                             lambda row: row.update(quorum_total=100), "denominator_ok"),
    ("RC-4", "failover"): ("boot_traces",
                           lambda row: row.update(served_supporting_stake=100),
                           "verification_ok"),
    ("RC-4", "returner"): ("boot_traces",
                           lambda row: row.update(served_supporting_stake=100),
                           "verification_ok"),
    ("RC-5", "transport"): ("admission_traces",
                            lambda row: row.update(retry_after=None), "retry_signal_ok"),
    ("RC-6", "off-host"): ("audit_traces",
                           lambda row: row.update(target_hashes=["ab" * 32, "cd" * 32]),
                           "target_shape_ok"),
    ("RC-6", "availability"): ("state_traces",
                               lambda row: row.update(computed_pre="ff" * 32), "pre_ok"),
    ("RC-2", "stall-denominator"): ("stall_series",
                                    lambda row: row.update(height=0), "baseline_ok"),
    ("RC-3", "stall-episode"): ("stall_series",
                                lambda row: row.update(height=0), "baseline_ok"),
}


def trace_mutation(claim: str, arm: str):
    if arm in ("local", "public"):
        return BASELINE_TRACE_MUTATIONS[claim]
    return ARM_TRACE_MUTATIONS[(claim, arm)]


class FakeBuilder:
    """The generated view of the mutated claims, so `validate` can be unit-tested."""

    def __init__(self, rows):
        self.rows = rows

    def campaigns(self):
        return {"campaigns": self.rows}


class ClaimTests(unittest.TestCase):
    def setUp(self):
        self.payload = copy.deepcopy(SOURCE)

    def claim(self, ident="RC-1"):
        return next(row for row in self.payload["claims"] if row["id"] == ident)

    def inspect(self):
        return checker.validate(self.payload, FakeBuilder(self.payload["claims"]))

    def test_the_tracker_validates(self):
        out = self.inspect()
        self.assertEqual(out["claims"], 7)
        self.assertEqual(out["arms"], 27)
        self.assertEqual(out["old"], 13)

    def test_no_claim_authors_a_status_or_a_missing_observation(self):
        for row in self.payload["claims"]:
            for forbidden in ("status", "missing", "sub_observations", "tool"):
                self.assertNotIn(forbidden, row, row["id"])

    def test_every_test_exists_and_defines_the_contract(self):
        for row in self.payload["claims"]:
            with self.subTest(claim=row["id"]):
                module = checker.load_test(row)
                self.assertEqual(module.CLAIM_ID, row["id"])
                for name in ("run", "check", "fixture"):
                    self.assertTrue(callable(getattr(module, name)), name)

    def test_every_test_parses_the_help_surface(self):
        for row in self.payload["claims"]:
            with self.subTest(claim=row["id"]):
                result = subprocess.run(
                    [sys.executable, str(ROOT / row["test"]), "--help"],
                    capture_output=True, text=True, timeout=60, cwd=ROOT)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_every_fixture_passes_and_a_decisive_mutation_fails(self):
        for row in self.payload["claims"]:
            module = checker.load_test(row)
            for arm in row["arms"]:
                with self.subTest(claim=row["id"], arm=arm["id"]):
                    record = module.fixture(arm["id"])
                    verdict, reasons = module.check(record)
                    self.assertEqual(verdict, "pass", reasons)
                    mutated = copy.deepcopy(record)
                    mutation(row["id"], arm["id"])(mutated)
                    verdict, reasons = module.check(mutated)
                    self.assertEqual(verdict, "fail", (row["id"], arm["id"], reasons))

    def test_every_fixture_carries_a_trace_and_a_trace_mutation_names_its_predicate(self):
        for row in self.payload["claims"]:
            module = checker.load_test(row)
            for arm in row["arms"]:
                with self.subTest(claim=row["id"], arm=arm["id"]):
                    record = module.fixture(arm["id"])
                    key, mutate, predicate = trace_mutation(row["id"], arm["id"])
                    traces = record["observations"].get(key) or []
                    self.assertTrue(traces, f"{row['id']}/{arm['id']} carries no {key}")
                    mutated = copy.deepcopy(record)
                    mutate(mutated["observations"][key][0])
                    verdict, reasons = module.check(mutated)
                    self.assertEqual(verdict, "fail", (row["id"], arm["id"], reasons))
                    self.assertTrue(any(predicate in reason for reason in reasons),
                                    (row["id"], arm["id"], predicate, reasons))

    def test_a_fringe_rejection_of_a_finalising_host_names_ledger_ok(self):
        row = self.claim("RC-1")
        module = checker.load_test(row)
        record = module.fixture("local")
        trace = record["observations"]["traces"][0]
        trace["rejected_user_hosts"] = list(trace["conflict_scope"])
        verdict, reasons = module.check(record)
        self.assertEqual(verdict, "fail")
        self.assertTrue(any("ledger_ok" in reason for reason in reasons), reasons)

    def test_a_final_scope_only_rejection_is_not_a_ledger_failure(self):
        row = self.claim("RC-1")
        module = checker.load_test(row)
        record = module.fixture("local")
        trace = record["observations"]["traces"][0]
        trace["conflict_scope"] = ["aa" * 32]
        trace["final_scope"] = ["ef" * 32]
        trace["rejected_user_hosts"] = ["ef" * 32]
        verdict, reasons = module.check(record)
        self.assertEqual(verdict, "pass", reasons)

    def test_a_system_chain_rejection_is_not_a_ledger_failure(self):
        row = self.claim("RC-1")
        module = checker.load_test(row)
        record = module.fixture("local")
        trace = record["observations"]["traces"][0]
        trace["rejected_hosts"] = list(trace["conflict_scope"])
        trace["rejected_user_hosts"] = []
        verdict, reasons = module.check(record)
        self.assertEqual(verdict, "pass", reasons)

    def test_every_claim_names_its_trace_close_actions_and_next_pr(self):
        pr_ids = {entry["id"] for entry in self.payload["action_prs"]}
        for row in self.payload["claims"]:
            with self.subTest(claim=row["id"]):
                self.assertIn(row.get("trace"), checker.TRACE_KINDS)
                for key in checker.CLOSE_KEYS:
                    self.assertTrue((row.get("close") or {}).get(key), (row["id"], key))
                environments = set()
                for arm in row["arms"]:
                    self.assertIn(arm.get("next_pr"), pr_ids, (row["id"], arm["id"]))
                    self.assertIn(arm.get("locality"), checker.LOCALITIES)
                    if arm.get("required"):
                        environments.add(arm["environment"])
                self.assertEqual(environments, {"local", "public"}, row["id"])

    def test_a_false_predicate_action_names_the_predicate_and_anchors(self):
        row = self.claim("RC-1")
        module = checker.load_test(row)
        record = module.fixture()
        record["verdict"] = "fail"
        record["observations"]["traces"][0]["fork_blocks"] = ["11" * 32]
        record["finished_at"] = "2026-11-20T10:00:00Z"
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "rc1" / "claim.json"
            path.parent.mkdir()
            path.write_text(json.dumps(record))
            derived = builder.derive_claim(row, roots=(Path(root),))
        action = derived["local"]["action"]
        self.assertEqual(action["reading"], "unsafe-point")
        item = next(item for item in action["items"] if item["item"].endswith("fork_ok"))
        self.assertIn("fork_ok", item["finding"])
        self.assertIn("casper/src/merging.rs", item["anchor"]["rust"])
        self.assertTrue(item["anchor"]["lean"].startswith("Rchain."))

    def test_an_unimplemented_arm_precondition_names_its_next_pr(self):
        row = self.claim("RC-6")
        module = checker.load_test(row)
        record = module.fixture("off-host")
        record["verdict"] = "not run"
        record["preconditions"] = ["the off-host arm's harness lands in rc-offhost"]
        record["finished_at"] = "2026-11-20T10:00:00Z"
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "rc6" / "claim.json"
            path.parent.mkdir()
            path.write_text(json.dumps(record))
            derived = builder.derive_claim(row, roots=(Path(root),))
        action = derived["off-host"]["action"]
        self.assertEqual(action["reading"], "precondition")
        self.assertIn("rc-offhost", action["items"][0]["action"])

    def test_a_precondition_action_says_what_to_supply(self):
        row = self.claim("RC-2")
        module = checker.load_test(row)
        record = module.fixture()
        record["verdict"] = "not run"
        record["preconditions"] = ["no partition was installed"]
        record["finished_at"] = "2026-11-20T10:00:00Z"
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "rc2" / "claim.json"
            path.parent.mkdir()
            path.write_text(json.dumps(record))
            derived = builder.derive_claim(row, roots=(Path(root),))
        action = derived["local"]["action"]
        self.assertEqual(action["reading"], "precondition")
        self.assertIn("no partition", action["items"][0]["finding"])
        self.assertTrue(action["items"][0]["action"])

    def test_a_rehearsal_record_cannot_close_a_public_arm(self):
        row = self.claim("RC-4")
        module = checker.load_test(row)
        record = module.fixture("public")
        record["rehearsal"] = True
        record["locality"] = "local-devnet"
        verdict, reasons = module.check(record)
        self.assertEqual(verdict, "not run")
        self.assertIn("rehearsal", reasons[0])
        record["verdict"] = "pass"
        record["finished_at"] = "2026-11-22T10:00:00Z"
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "rc4" / "claim.json"
            path.parent.mkdir()
            path.write_text(json.dumps(record))
            derived = builder.derive_claim(row, roots=(Path(root),))
        self.assertNotEqual(derived["public"]["action"]["reading"], "safe")

    def test_a_public_not_observed_action_names_the_manifest(self):
        row = self.claim("RC-1")
        with tempfile.TemporaryDirectory() as root:
            derived = builder.derive_claim(row, roots=(Path(root),))
        action = derived["public"]["action"]
        self.assertEqual(action["reading"], "not-observed")
        self.assertEqual(action["next_pr"], "rc-public-fleet")
        self.assertIn("four-host public manifest", action["items"][0]["action"])

    def test_check_is_deterministic(self):
        for row in self.payload["claims"]:
            module = checker.load_test(row)
            self.assertEqual(module.check(module.fixture()),
                             module.check(module.fixture()))

    def test_a_missing_test_is_refused(self):
        self.claim()["test"] = "tests/release-claim-does-not-exist.py"
        with self.assertRaisesRegex(checker.ClaimError, "does not exist"):
            self.inspect()

    def test_a_test_without_check_is_refused(self):
        with tempfile.NamedTemporaryFile(
                "w", suffix=".py", dir=ROOT / "target", delete=False) as handle:
            handle.write("CLAIM_ID = 'RC-1'\ndef run(argv):\n    return 0\ndef fixture():\n"
                         "    return {'claim_id': 'RC-1'}\n")
            path = Path(handle.name)
        try:
            self.claim()["test"] = str(path.relative_to(ROOT))
            with self.assertRaisesRegex(checker.ClaimError, "check"):
                self.inspect()
        finally:
            path.unlink()

    def test_a_wrong_claim_id_is_refused(self):
        with tempfile.NamedTemporaryFile(
                "w", suffix=".py", dir=ROOT / "target", delete=False) as handle:
            handle.write("CLAIM_ID = 'RC-2'\n")
            path = Path(handle.name)
        try:
            self.claim()["test"] = str(path.relative_to(ROOT))
            with self.assertRaisesRegex(checker.ClaimError, "CLAIM_ID"):
                self.inspect()
        finally:
            path.unlink()

    def test_status_is_derived_from_a_record_not_authored(self):
        row = next(row for row in SOURCE["claims"] if row["id"] == "RC-1")
        module = checker.load_test(row)
        good = module.fixture()
        good["finished_at"] = "2026-11-20T10:00:00Z"
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "rc1" / "claim.json"
            path.parent.mkdir()
            path.write_text(json.dumps(good))
            derived = builder.derive_claim(row, roots=(Path(root),))
            self.assertEqual(derived["local"]["status"], "pass")
            self.assertEqual(derived["local"]["action"]["reading"], "safe")
            self.assertEqual(derived["public"]["status"], "not run")
            self.assertEqual(derived["public"]["action"]["reading"], "not-observed")
            # A stored pass whose observations fail is a fail, never a pass.
            bad = copy.deepcopy(good)
            BASELINE_MUTATIONS["RC-1"](bad)
            bad["verdict"] = "pass"
            path.write_text(json.dumps(bad))
            derived = builder.derive_claim(row, roots=(Path(root),))
            self.assertEqual(derived["local"]["status"], "fail")
            self.assertEqual(derived["local"]["action"]["reading"], "model-incomplete")
            path.unlink()
            derived = builder.derive_claim(row, roots=(Path(root),))
            self.assertEqual(derived["local"]["status"], "not run")
            self.assertEqual(derived["local"]["action"]["reading"], "not-observed")

    def test_the_tracker_is_current_and_not_ready(self):
        result = subprocess.run(
            [sys.executable, str(ROOT / "tools/check-release-campaigns.py"), "--check"],
            capture_output=True, text=True, timeout=600, cwd=ROOT)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("RELEASE_CLAIMS_NOT_READY", result.stdout)

    def test_assert_ready_exits_three(self):
        result = subprocess.run(
            [sys.executable, str(ROOT / "tools/check-release-campaigns.py"),
             "--assert-ready"],
            capture_output=True, text=True, timeout=600, cwd=ROOT)
        self.assertEqual(result.returncode, 3)
        self.assertIn("RELEASE_CLAIMS_NOT_READY", result.stdout)

    def test_no_claim_is_in_the_gate_register(self):
        register_ids = {row["id"] for row in REGISTER["gates"]}
        self.assertFalse(register_ids & {row["id"] for row in TRACKER["campaigns"]})

    def test_every_deleted_row_is_mapped(self):
        mapped = {entry["id"] for entry in self.payload["superseded"]}
        self.assertEqual(mapped, checker.OLD_IDS)
        for row in self.payload["claims"]:
            self.assertTrue(set(row.get("supersedes") or []) <= checker.OLD_IDS)

    def test_an_unmapped_deleted_row_is_refused(self):
        self.payload["superseded"] = [
            entry for entry in self.payload["superseded"] if entry["id"] != "C343"]
        with self.assertRaisesRegex(checker.ClaimError, "every deleted row"):
            self.inspect()

    def test_a_claim_may_not_reuse_a_deleted_id(self):
        self.claim()["id"] = "G428-M1"
        with self.assertRaisesRegex(checker.ClaimError, "deleted campaign id"):
            self.inspect()

    def test_a_required_arm_is_needed_in_both_environments(self):
        row = self.claim()
        row["arms"] = [arm for arm in row["arms"] if arm["environment"] != "public"]
        with self.assertRaisesRegex(checker.ClaimError, "both environments"):
            self.inspect()

    def test_every_excluded_behaviour_maps_to_an_arm(self):
        refs = {f"{row['id']}/{arm['id']}"
                for row in self.payload["claims"] for arm in row["arms"]}
        for entry in self.payload["excluded_behaviours"]:
            with self.subTest(behaviour=entry["id"]):
                self.assertTrue(entry.get("arms"))
                for reference in entry["arms"]:
                    self.assertIn(reference, refs)

    def test_an_unmapped_excluded_behaviour_is_refused(self):
        self.payload["excluded_behaviours"][0]["arms"] = ["RC-1/does-not-exist"]
        with self.assertRaisesRegex(checker.ClaimError, "unknown arm"):
            self.inspect()

    def test_a_retired_behaviour_cannot_be_covered_by_an_optional_arm(self):
        arm = next(arm for arm in self.claim("RC-5")["arms"] if arm["id"] == "transport")
        arm["required"] = False
        with self.assertRaisesRegex(checker.ClaimError, "non-required arm"):
            self.inspect()

    def test_the_retired_behaviours_are_covered(self):
        self.assertNotIn("out_of_scope", self.payload)
        self.assertEqual(len(self.payload["excluded_behaviours"]), 7)

    def test_multi_host_refuses_one_host_labelled_as_several(self):
        row = self.claim("RC-2")
        module = checker.load_test(row)
        record = module.fixture("multi-host")
        record["observations"]["multi_host"]["declared_hosts"] = ["host-a"] * 8
        verdict, reasons = module.check(record)
        self.assertEqual(verdict, "fail")
        self.assertTrue(any("labelled as several" in reason for reason in reasons), reasons)

    def test_an_unknown_arm_fails_naming_the_accepted_arms(self):
        row = self.claim("RC-1")
        module = checker.load_test(row)
        record = module.fixture("local")
        record["arm"] = "no-such-arm"
        verdict, reasons = module.check(record)
        self.assertEqual(verdict, "fail")
        self.assertTrue(any("unknown arm" in reason for reason in reasons), reasons)

    def test_both_rerun_commands_are_required(self):
        self.claim()["rerun"]["local"] = ""
        with self.assertRaisesRegex(checker.ClaimError, "re-run commands"):
            self.inspect()

    def test_the_floor_holds(self):
        self.assertEqual(checker.CLAIM_MINIMUM, 7)
        self.assertEqual(len(self.payload["claims"]), 7)

    def test_a_removed_claim_is_refused(self):
        self.payload["claims"].pop()
        with self.assertRaisesRegex(checker.ClaimError, "floor"):
            self.inspect()

    def test_the_generated_tracker_carries_the_authored_claims(self):
        generated = {row["id"] for row in TRACKER["campaigns"]}
        authored = {row["id"] for row in SOURCE["claims"]}
        self.assertEqual(generated, authored)


class HarnessSuiteTests(unittest.TestCase):
    """The fleet harness suites are reached where CI already looks: through this file."""

    def test_the_fleet_harness_suites_pass(self):
        from io import StringIO
        for pattern in ("test_fleet_manifest.py", "test_fleet_run.py",
                        "test_release_preflight.py"):
            suite = unittest.defaultTestLoader.discover(
                str(ROOT / "tests"), pattern=pattern, top_level_dir=str(ROOT / "tests"))
            result = unittest.TextTestRunner(stream=StringIO(), verbosity=0).run(suite)
            self.assertTrue(result.wasSuccessful(),
                            (pattern, result.errors, result.failures))
            self.assertGreater(result.testsRun, 0)


if __name__ == "__main__":
    unittest.main()
