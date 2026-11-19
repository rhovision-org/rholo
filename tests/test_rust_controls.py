"""The Rust witness controls execute, and their verdict comes from libtest.

`--run-controls` used to return SKIPPED for every Rust witness: the mutation was
applied and reverted, and no test ran. This suite covers the pieces that make the
run honest — crate/target derivation, the classifier that refuses to read a
nonzero exit as a control, and one live toy crate that is actually mutated,
rebuilt, failed and restored.
"""
import importlib.util
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "mainnet_gates", ROOT / "tools/check-mainnet-gates.py")
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)

TOY_LIB = """\
pub fn boundary() -> i64 {
    0
}

pub fn untouched() -> i64 {
    7
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_boundary_is_zero() {
        assert_eq!(boundary(), 0);
    }
}
"""


def toy_crate(root: Path) -> None:
    """A dependency-free crate with one passing lib test and one untouched function.

    Shared with `tests/test_hardfork_51_closeout.py`, whose acceptance runner must
    be shown to execute a test, not only classify a fabricated log.
    """
    (root / "src").mkdir(exist_ok=True)
    (root / "Cargo.toml").write_text(
        '[package]\nname = "toy-control"\nversion = "0.1.0"\nedition = "2021"\n')
    (root / "src" / "lib.rs").write_text(TOY_LIB)


def control_row(before: str, after: str, symbol: str = "the_boundary_is_zero") -> dict:
    return {
        "id": "toy-control",
        "witness_binding": {
            "control": {
                "test": f"src/lib.rs::{symbol}",
                "expected": "FAIL",
                "mutation": {"file": "src/lib.rs", "before": before, "after": after},
            }
        },
    }


class DerivationTests(unittest.TestCase):
    def test_the_nearest_manifest_names_the_package(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "crate" / "src").mkdir(parents=True)
            (root / "crate" / "tests").mkdir()
            (root / "crate" / "Cargo.toml").write_text(
                '[package]\nname = "rchain-toy"\nversion = "0.1.0"\n')
            self.assertEqual(
                gate.cargo_package(root / "crate" / "src" / "lib.rs", root),
                "rchain-toy")
            self.assertEqual(
                gate.cargo_package(root / "crate" / "tests" / "arm.rs", root),
                "rchain-toy")

    def test_a_manifest_without_a_package_name_is_not_a_package(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "src").mkdir()
            (root / "Cargo.toml").write_text("[workspace]\nmembers = []\n")
            self.assertIsNone(gate.cargo_package(root / "src" / "lib.rs", root))

    def test_the_target_is_lib_or_the_integration_stem(self):
        self.assertEqual(
            gate.cargo_target(Path("casper/src/dag.rs"), Path("casper")), ["--lib"])
        self.assertEqual(
            gate.cargo_target(Path("node/tests/lean_stake_corpus.rs"), Path("node")),
            ["--test", "lean_stake_corpus"])

    def test_every_registered_rust_witness_derives_its_witness_package(self):
        register = json.loads((ROOT / "spec/audit/mainnet-gates.json").read_text())
        rust = [row for row in register["gates"] if row.get("status") == "met"
                and str(row.get("witness_kind", "")).startswith("rust")]
        self.assertEqual(len(rust), 45)
        for row in rust:
            path_text = row["witness_ref"].split("::", 1)[0]
            test_path = ROOT / path_text
            crate = gate.cargo_crate_root(test_path, ROOT)
            self.assertIsNotNone(crate, row["id"])
            self.assertIsNotNone(gate.cargo_package(test_path, ROOT), row["id"])
            self.assertIn(gate.cargo_target(test_path, crate)[0],
                          ("--lib", "--test"), row["id"])
        # A mutation in another crate must not move the invocation: these four
        # controls mutate a dependency, and the witness's own crate runs the test.
        for ident, package in (("S6-0d97bafd93b8fda9", "rchain-casper"),
                               ("S6-5651614bf8e3eeb7", "rchain-casper"),
                               ("S6-a641f6fa8cc3ce54", "rchain-rholang"),
                               ("S6-f4e50770cc349ad3", "rchain-node")):
            row = next(x for x in rust if x["id"] == ident)
            witness = ROOT / row["witness_ref"].split("::", 1)[0]
            self.assertEqual(gate.cargo_package(witness, ROOT), package)


class ClassifierTests(unittest.TestCase):
    def test_the_symbols_own_failed_line_is_a_control(self):
        log = "test wire::tests::the_invariant ... FAILED\n\ntest result: FAILED.\n"
        self.assertEqual(
            gate.rust_control_verdict(log, "the_invariant"), "CONTROLLED")

    def test_a_passing_symbol_is_unproven(self):
        log = "test wire::tests::the_invariant ... ok\n\ntest result: ok.\n"
        self.assertEqual(
            gate.rust_control_verdict(log, "the_invariant"), "UNPROVEN")

    def test_an_ignored_symbol_is_not_a_verdict(self):
        log = "test wire::tests::the_invariant ... ignored\n"
        self.assertEqual(gate.rust_control_verdict(log, "the_invariant"), "ERROR")

    def test_a_build_error_is_not_a_control(self):
        log = "error[E0308]: mismatched types\n --> src/lib.rs:1:1\n"
        self.assertEqual(gate.rust_control_verdict(log, "the_invariant"), "ERROR")

    def test_another_tests_failure_is_not_this_witnesss_control(self):
        log = "test wire::tests::another ... FAILED\n"
        self.assertEqual(gate.rust_control_verdict(log, "the_invariant"), "ERROR")

    def test_a_prefixed_symbol_is_not_a_match(self):
        log = "test wire::tests::the_invariant_extra ... FAILED\n"
        self.assertEqual(gate.rust_control_verdict(log, "the_invariant"), "ERROR")

    def test_two_matching_tests_cannot_say_which_one_the_mutation_controlled(self):
        log = ("test a::the_invariant ... ok\n"
               "test b::the_invariant ... FAILED\n")
        self.assertEqual(gate.rust_control_verdict(log, "the_invariant"), "ERROR")


class LiveToyControlTests(unittest.TestCase):
    """One real mutation, one real cargo run, one restored source file."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        toy_crate(self.root)
        patcher = mock.patch.dict(
            os.environ,
            {"CARGO_TARGET_DIR": str(ROOT / "target" / "rust-control-toy")})
        patcher.start()
        self.addCleanup(patcher.stop)
        self.addCleanup(self.temp.cleanup)

    def source(self) -> str:
        return (self.root / "src" / "lib.rs").read_text()

    def control(self, row: dict, mode: str = "rust") -> str:
        return gate.run_control(row, mode=mode, root=self.root,
                                cargo_flags=("--offline",))

    def test_a_mutation_the_witness_asserts_is_controlled(self):
        row = control_row("    0\n", "    -1\n")
        self.assertEqual(self.control(row), "CONTROLLED")
        self.assertEqual(self.source(), TOY_LIB)

    def test_a_mutation_the_witness_does_not_assert_is_unproven(self):
        row = control_row("    7\n", "    8\n")
        self.assertEqual(self.control(row), "UNPROVEN")
        self.assertEqual(self.source(), TOY_LIB)

    def test_a_mutation_that_does_not_compile_is_error(self):
        row = control_row("    7\n", "    fn broken(\n")
        self.assertEqual(self.control(row), "ERROR")
        self.assertEqual(self.source(), TOY_LIB)

    def test_an_unselected_control_does_not_touch_the_source(self):
        row = control_row("    0\n", "    -1\n")
        self.assertEqual(self.control(row, mode="python"), "SKIPPED")
        self.assertEqual(self.source(), TOY_LIB)

    def test_a_dirty_mutation_target_is_refused(self):
        # A git worktree with an uncommitted target is operator work or a leftover
        # mutation; it must not be silently clobbered.
        proc = subprocess.run(["git", "init", "-q", str(self.root)],
                              capture_output=True, text=True, check=False)
        if proc.returncode != 0:
            self.skipTest("git is unavailable")
        subprocess.run(["git", "-C", str(self.root), "add", "."],
                       capture_output=True, check=False)
        subprocess.run(["git", "-C", str(self.root), "-c", "user.name=t",
                        "-c", "user.email=t@t", "commit", "-qm", "base"],
                       capture_output=True, check=False)
        (self.root / "src" / "lib.rs").write_text(TOY_LIB + "\n// operator edit\n")
        row = control_row("    0\n", "    -1\n")
        self.assertEqual(self.control(row), "ERROR")
        self.assertIn("// operator edit", self.source())


if __name__ == "__main__":
    unittest.main()
