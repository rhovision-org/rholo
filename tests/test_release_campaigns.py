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

MUTATIONS = {
    "RC-1": lambda record: record["observations"]["finalised"][0].update(marker_present=False),
    "RC-2": lambda record: record["observations"]["containment"][0]["answers"][0].update(
        finalized=False),
    "RC-3": lambda record: record["observations"]["blocks"][0].update(disagreement=True),
    "RC-4": lambda record: record["observations"]["post_heads"][0].update(marker_present=False),
    "RC-5": lambda record: record["observations"].update(verdict="FAIL"),
    "RC-6": lambda record: record["observations"]["audit"].update(verdict="DISAGREE"),
    "RC-7": lambda record: record["observations"]["downgrade"].update(refused=False),
}


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
        self.assertEqual(out["old"], 12)

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
            with self.subTest(claim=row["id"]):
                module = checker.load_test(row)
                record = module.fixture()
                verdict, reasons = module.check(record)
                self.assertEqual(verdict, "pass", reasons)
                mutated = copy.deepcopy(record)
                MUTATIONS[row["id"]](mutated)
                verdict, reasons = module.check(mutated)
                self.assertEqual(verdict, "fail", (row["id"], reasons))

    TRACE_MUTATIONS = {
        "RC-1": ("traces", lambda row: row.update(fork_blocks=["11" * 32]), "fork_ok"),
        "RC-2": ("finality_traces", lambda row: row.update(quorum_total=100), "denominator_ok"),
        "RC-3": ("state_traces", lambda row: row.update(computed_pre="ff" * 32), "pre_ok"),
        "RC-4": ("boot_traces", lambda row: row.update(served_supporting_stake=100),
                 "verification_ok"),
        "RC-5": ("admission_traces", lambda row: row.update(retry_after=None),
                 "retry_signal_ok"),
        "RC-6": ("audit_traces", lambda row: row.update(target_hashes=["ab" * 32, "cd" * 32]),
                 "target_shape_ok"),
        "RC-7": ("format_traces", lambda row: row.update(decision="accept"), "format_ok"),
    }

    def test_every_fixture_carries_a_trace_and_a_trace_mutation_names_its_predicate(self):
        for row in self.payload["claims"]:
            with self.subTest(claim=row["id"]):
                module = checker.load_test(row)
                record = module.fixture()
                key, mutate, predicate = self.TRACE_MUTATIONS[row["id"]]
                traces = record["observations"].get(key) or []
                self.assertTrue(traces, f"{row['id']} fixture carries no {key}")
                mutated = copy.deepcopy(record)
                mutate(mutated["observations"][key][0])
                verdict, reasons = module.check(mutated)
                self.assertEqual(verdict, "fail", (row["id"], reasons))
                self.assertTrue(any(predicate in reason for reason in reasons),
                                (row["id"], predicate, reasons))

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
            self.assertEqual(derived["public"]["status"], "not run")
            # A stored pass whose observations fail is a fail, never a pass.
            bad = copy.deepcopy(good)
            MUTATIONS["RC-1"](bad)
            bad["verdict"] = "pass"
            path.write_text(json.dumps(bad))
            derived = builder.derive_claim(row, roots=(Path(root),))
            self.assertEqual(derived["local"]["status"], "fail")
            path.unlink()
            derived = builder.derive_claim(row, roots=(Path(root),))
            self.assertEqual(derived["local"]["status"], "not run")

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

    def test_both_environments_are_required(self):
        self.claim()["environment"]["public"] = ""
        with self.assertRaisesRegex(checker.ClaimError, "both environments"):
            self.inspect()

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


if __name__ == "__main__":
    unittest.main()
