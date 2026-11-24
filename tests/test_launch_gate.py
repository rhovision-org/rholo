"""The launch gate has two halves: claim arms and external obligations.

The open set is derived from `mainnet-gates.external.json`; the dispositions register carries
decisions only. These tests prove the schema, the anti-rot rules, the dated-disposition clause
and that `--assert-ready` cannot be satisfied by a disposition for a still-open external row.
"""
import contextlib
import importlib.util
import io
import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "check_launch_gate", ROOT / "tools/check-launch-gate.py")
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)

TODAY = "2026-11-24"
EXTERNAL = {
    "gates": [
        {"id": "G428-M5", "status": "blocked"},
        {"id": "G428-M6", "status": "external"},
        {"id": "G584-G6", "status": "superseded"},
    ]
}
M5 = {
    "gate_id": "G428-M5",
    "disposition": "deferred",
    "dated": "2026-11-15",
    "authority": "owner",
    "reference": "https://github.com/rhovision-org/rholo/issues/428",
    "note": "unavailable",
}


class LaunchGateTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)

    def write(self, name, document):
        path = self.dir / name
        path.write_text(json.dumps(document))
        return path

    def run_gate(self, dispositions, *args, external=None):
        external_path = self.write("external.json", external or EXTERNAL)
        disposition_path = self.write("dispositions.json", dispositions)
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            code = gate.main([
                *args, "--today", TODAY,
                "--external", str(external_path),
                "--dispositions", str(disposition_path),
            ])
        return code, output.getvalue()

    def register(self, rows, **top):
        return {"schema": 1, "as_of": TODAY, "rule": "rule", "dispositions": rows, **top}

    def test_the_committed_register_needs_the_other_seven_dispositions(self):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            code = gate.main(["--json", "--today", TODAY])
        self.assertEqual(code, 0)
        report = json.loads(output.getvalue())
        self.assertEqual(report["open"], [
            "C357", "G428-M5", "G428-M6", "G520-MIGRATION",
            "G655-FORKLESS-DEVREWARDS", "S6-PR600-CHECKPOINT-V3",
            "S6-PR600-MERGE-FORK", "TA-C3",
        ])
        self.assertEqual(report["missing"], [
            "C357", "G428-M6", "G520-MIGRATION", "G655-FORKLESS-DEVREWARDS",
            "S6-PR600-CHECKPOINT-V3", "S6-PR600-MERGE-FORK", "TA-C3",
        ])
        self.assertEqual(report["dispositioned"], ["G428-M5"])

    def test_a_missing_disposition_is_not_dispositioned(self):
        code, output = self.run_gate(self.register([M5]), "--gate")
        self.assertEqual(code, 3)
        self.assertIn("G428-M6: MISSING", output)

    def test_every_open_row_dispositioned_passes_the_clause(self):
        rows = [M5, {**M5, "gate_id": "G428-M6"}]
        code, output = self.run_gate(self.register(rows), "--gate")
        self.assertEqual(code, 0)
        self.assertIn("LAUNCH_GATE_DISPOSITIONED", output)

    def test_duplicate_disposition_is_refused(self):
        code, output = self.run_gate(self.register([M5, M5]), "--gate")
        self.assertEqual(code, 2)
        self.assertIn("duplicate disposition", output)

    def test_a_disposition_on_a_superseded_row_is_refused(self):
        row = {**M5, "gate_id": "G584-G6"}
        code, output = self.run_gate(self.register([row]), "--gate")
        self.assertEqual(code, 2)
        self.assertIn("already superseded", output)

    def test_an_unknown_gate_id_is_refused(self):
        row = {**M5, "gate_id": "G-UNKNOWN"}
        code, output = self.run_gate(self.register([row]), "--gate")
        self.assertEqual(code, 2)
        self.assertIn("unknown external gate", output)

    def test_a_future_date_is_refused(self):
        row = {**M5, "dated": "2027-01-01"}
        code, output = self.run_gate(self.register([row]), "--gate")
        self.assertEqual(code, 2)
        self.assertIn("in the future", output)

    def test_an_unknown_disposition_is_refused(self):
        row = {**M5, "disposition": "open"}
        code, output = self.run_gate(self.register([row]), "--gate")
        self.assertEqual(code, 2)
        self.assertIn("is not one of", output)

    def test_an_empty_authority_is_refused(self):
        row = {**M5, "authority": " "}
        code, output = self.run_gate(self.register([row]), "--gate")
        self.assertEqual(code, 2)
        self.assertIn("authority is empty", output)

    def test_a_reference_outside_https_or_the_tree_is_refused(self):
        row = {**M5, "reference": "issue comment 12"}
        code, output = self.run_gate(self.register([row]), "--gate")
        self.assertEqual(code, 2)
        self.assertIn("reference must be", output)

    def test_an_unknown_top_level_key_is_refused(self):
        code, output = self.run_gate(self.register([M5], status="met"), "--gate")
        self.assertEqual(code, 2)
        self.assertIn("unknown key", output)

    def test_assert_ready_requires_the_claim_arms(self):
        claims = SimpleNamespace(returncode=3, stdout="RELEASE_CLAIMS_NOT_READY\n", stderr="")
        with mock.patch.object(gate.subprocess, "run", return_value=claims):
            code, output = self.run_gate(self.register([M5]), "--assert-ready")
        self.assertEqual(code, 3)
        self.assertIn("RELEASE_CLAIMS_NOT_READY", output)

    def test_assert_ready_requires_external_closure_not_a_disposition(self):
        claims = SimpleNamespace(returncode=0, stdout="RELEASE_CLAIMS_READY\n", stderr="")
        with mock.patch.object(gate.subprocess, "run", return_value=claims):
            code, output = self.run_gate(self.register([M5]), "--assert-ready")
        self.assertEqual(code, 3)
        self.assertIn("G428-M5", output)
        self.assertIn("G428-M6 (no dated disposition)", output)

    def test_assert_ready_passes_only_when_claims_and_externals_are_closed(self):
        closed = {"gates": [
            {"id": "G428-M5", "status": "met"},
            {"id": "G584-G6", "status": "superseded"},
        ]}
        claims = SimpleNamespace(returncode=0, stdout="RELEASE_CLAIMS_READY\n", stderr="")
        with mock.patch.object(gate.subprocess, "run", return_value=claims):
            code, output = self.run_gate(self.register([]), "--assert-ready",
                                         external=closed)
        self.assertEqual(code, 0)
        self.assertIn("LAUNCH_GATE_READY", output)


if __name__ == "__main__":
    unittest.main()
