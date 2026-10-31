"""Mutation tests for the cost register's gate — the check must fail on the cases it exists for.

Each test mutates a copy of `spec/cost-register.tsv` and runs the real gate over it, so this exercises
the command CI runs rather than a re-imported copy of its logic. The cases are the four ways a register
stops meaning anything: a live arm with no row (silence), a row for nothing (staleness), a declared
charge that is not where the row says (drift), and a register with nothing in it (vacuous).
"""

import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
GATE = ROOT / "tools/check-cost-register.py"
REGISTER = ROOT / "spec/cost-register.tsv"


class CostRegisterTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.path = Path(self.tmp.name) / "cost-register.tsv"
        shutil.copy(REGISTER, self.path)
        self.header = [line for line in self.path.read_text().splitlines()
                       if line.startswith("#") or line.startswith("id\t")]
        self.rows = [line for line in self.path.read_text().splitlines()
                     if line and not line.startswith("#") and not line.startswith("id\t")]

    def tearDown(self):
        self.tmp.cleanup()

    def write(self, rows):
        self.path.write_text("\n".join(self.header + rows) + "\n")

    def gate(self):
        result = subprocess.run(
            [sys.executable, str(GATE), "--register", str(self.path)],
            capture_output=True, text=True, check=False,
        )
        return result.returncode, result.stdout + result.stderr

    def row(self, ident):
        return next(line for line in self.rows if line.startswith(ident + "\t"))

    def replace(self, ident, old, new):
        return [line.replace(old, new, 1) if line.startswith(ident + "\t") else line
                for line in self.rows]

    def test_the_committed_register_passes(self):
        code, output = self.gate()
        self.assertEqual(code, 0, output)

    def test_a_live_arm_with_no_row_is_refused(self):
        # `nth` is an arm of `eval_method`; dropping its row is the silence this gate exists for.
        self.write([line for line in self.rows if not line.startswith("COST001\t")])
        code, output = self.gate()
        self.assertEqual(code, 1)
        self.assertIn("undeclared arm: nth", output)

    def test_a_row_for_nothing_is_refused(self):
        self.write(self.rows + [
            "\t".join(["COST999", "rholang/src/reduce.rs", "goneMethod", "nothing", "constant",
                       "-", "nth_method_call_cost", "-", "-", "-"])
        ])
        code, output = self.gate()
        self.assertEqual(code, 1)
        self.assertIn("goneMethod is not a method arm any more", output)

    def test_a_charge_that_is_not_in_its_arm_is_refused(self):
        # `string_replace_cost` exists in the file, so the presence check passes; it is not in `nth`.
        self.write(self.replace("COST001", "nth_method_call_cost", "string_replace_cost"))
        code, output = self.gate()
        self.assertEqual(code, 1)
        self.assertIn("nth's arm names none of its declared charges", output)

    def test_an_identity_row_may_not_name_a_charge(self):
        self.write(self.replace("COST023", "\tidentity\t-\t-", "\tidentity\t-\tlookup_cost"))
        code, output = self.gate()
        self.assertEqual(code, 1)
        self.assertIn("shape identity must not name a charge", output)

    def test_an_empty_register_is_refused(self):
        self.path.write_text("\n".join(self.header) + "\n")
        code, output = self.gate()
        self.assertEqual(code, 1)
        self.assertIn("the register is empty", output)

    def test_a_short_row_is_refused(self):
        self.write(self.rows + ["COST999\trholang/src/reduce.rs\tnth\twork\tconstant"])
        code, output = self.gate()
        self.assertEqual(code, 1)
        self.assertIn("columns", output)


if __name__ == "__main__":
    unittest.main()
