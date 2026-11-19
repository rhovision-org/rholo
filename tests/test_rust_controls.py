"""The Rust witness controls execute, and their verdict comes from libtest.

`--run-controls` used to return SKIPPED for every Rust witness: the mutation was
applied and reverted, and no test ran. This suite covers the pieces that make the
run honest — crate/target derivation, the classifier that refuses to read a
nonzero exit as a control, and one live toy crate that is actually mutated,
rebuilt, failed and restored.
"""
import importlib.util
import os
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
        self.assertEqual(gate.cargo_target(Path("casper/src/dag.rs")), ["--lib"])
        self.assertEqual(
            gate.cargo_target(Path("node/tests/lean_stake_corpus.rs")),
            ["--test", "lean_stake_corpus"])


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


class LiveToyControlTests(unittest.TestCase):
    """One real mutation, one real cargo run, one restored source file."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        (self.root / "src").mkdir()
        (self.root / "Cargo.toml").write_text(
            '[package]\nname = "toy-control"\nversion = "0.1.0"\nedition = "2021"\n')
        (self.root / "src" / "lib.rs").write_text(TOY_LIB)
        patcher = mock.patch.dict(
            os.environ,
            {"CARGO_TARGET_DIR": str(ROOT / "target" / "rust-control-toy")})
        patcher.start()
        self.addCleanup(patcher.stop)
        self.addCleanup(self.temp.cleanup)

    def source(self) -> str:
        return (self.root / "src" / "lib.rs").read_text()

    def test_a_mutation_the_witness_asserts_is_controlled(self):
        row = control_row("    0\n", "    -1\n")
        self.assertEqual(
            gate.run_control(row, mode="rust", root=self.root), "CONTROLLED")
        self.assertEqual(self.source(), TOY_LIB)

    def test_a_mutation_the_witness_does_not_assert_is_unproven(self):
        row = control_row("    7\n", "    8\n")
        self.assertEqual(
            gate.run_control(row, mode="rust", root=self.root), "UNPROVEN")
        self.assertEqual(self.source(), TOY_LIB)

    def test_a_mutation_that_does_not_compile_is_error(self):
        row = control_row("    7\n", "    fn broken(\n")
        self.assertEqual(
            gate.run_control(row, mode="rust", root=self.root), "ERROR")
        self.assertEqual(self.source(), TOY_LIB)

    def test_an_unselected_control_does_not_touch_the_source(self):
        row = control_row("    0\n", "    -1\n")
        self.assertEqual(
            gate.run_control(row, mode="python", root=self.root), "SKIPPED")
        self.assertEqual(self.source(), TOY_LIB)


if __name__ == "__main__":
    unittest.main()
