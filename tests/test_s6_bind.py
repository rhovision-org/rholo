"""Pure checks for the S6 batch binder."""
from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("s6_bind", ROOT / "tools" / "s6-bind.py")
binder = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(binder)


class BinderTests(unittest.TestCase):
    def test_the_slug_is_the_gate_hash_and_date(self):
        self.assertEqual(
            binder.slug({"gate_id": "S6-25bfdbe2c179c81e"}, "20261118"),
            "s6-25bfdbe2c179c81e-20261118")

    def test_the_command_for_a_library_test(self):
        entry = {"witness_ref": "casper/src/validate.rs::a_test",
                 "witness_kind": "rust-unit", "package": "rchain-casper",
                 "cargo_target": "lib"}
        self.assertEqual(
            binder.command_for(entry),
            ["cargo", "test", "-p", "rchain-casper", "--lib", "a_test"])

    def test_the_command_for_an_integration_test(self):
        entry = {"witness_ref": "casper/tests/replay.rs::a_test",
                 "witness_kind": "rust-integration", "package": "rchain-casper",
                 "cargo_target": "replay"}
        self.assertEqual(
            binder.command_for(entry),
            ["cargo", "test", "-p", "rchain-casper", "--test", "replay",
             "--", "--exact", "a_test"])

    def test_the_command_for_the_python_witness(self):
        entry = {"witness_ref": "tests/liveness-enforcement.py::self_test",
                 "witness_kind": "python"}
        self.assertEqual(
            binder.command_for(entry),
            ["python3", "tests/liveness-enforcement.py", "self-test"])

    def test_the_binary_pin_reads_the_running_line(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "test-binary"
            path.write_bytes(b"binary")
            pin = binder.binary_pin(f"Running tests/x.rs ({path})\n")
        self.assertEqual(pin, binder.hashlib.sha256(b"binary").hexdigest())

    def test_a_missing_or_ambiguous_running_line_is_refused(self):
        with self.assertRaises(binder.BindError):
            binder.binary_pin("no binary here")
        with self.assertRaises(binder.BindError):
            binder.binary_pin("Running a (/one)\nRunning b (/two)\n")

    def test_the_plan_selects_one_batch(self):
        document = {"schema": 1, "entries": [
            {"batch": "A", "gate_id": "X"},
            {"batch": "B", "gate_id": "Y"},
        ]}
        self.assertEqual(
            [entry["gate_id"] for entry in binder.plan_entries(document, "A")], ["X"])
        self.assertEqual(
            [entry["gate_id"] for entry in binder.plan_entries(document, "B")], ["Y"])

    def test_the_test_attribute_is_required(self):
        self.assertTrue(binder.test_attribute("#[test]\nfn a_test() {}", "a_test"))
        self.assertTrue(binder.test_attribute("#[tokio::test]\nasync fn a_test() {}", "a_test"))
        self.assertFalse(binder.test_attribute("fn a_test() {}", "a_test"))


if __name__ == "__main__":
    unittest.main()
