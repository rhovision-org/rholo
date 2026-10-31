#!/usr/bin/env python3
"""Exercise reconcile's live host table and fail-closed mutation gate offline."""
from pathlib import Path
import os
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "tools/reconcile-network.sh"


def read_prelude():
    data = SCRIPT.read_text()
    # The live effects are strictly after this boundary. This fixture evaluates
    # the real argument parser and target guard without calling any transport.
    return data[data.index("set -uo pipefail"):data.index("is_local() {")]


def run_case(args, nodefile=None, control="systemd"):
    env = os.environ.copy()
    env.pop("RECONCILE_NODES", None)
    env.update(RECONCILE_CONTROL=control)
    if nodefile is not None:
        env["RECONCILE_NODES"] = str(nodefile)
    return subprocess.run(
        ["bash", "-c", read_prelude(), "reconcile-target-guard", *args],
        capture_output=True, text=True, check=False, timeout=10, env=env
    )


class ReconcileTargetGuard(unittest.TestCase):
    def test_current_b_and_c_endpoints_are_correct(self):
        data = read_prelude()
        self.assertIn("167.99.43.214 40403 rnode   B", data)
        self.assertIn("167.99.43.214 41403 rnode-c C", data)
        self.assertNotIn("104.131.176.164", data)

    def test_plan_only_is_allowed_without_an_inventory(self):
        self.assertEqual(run_case([]).returncode, 0)

    def test_live_apply_refuses_implicit_defaults(self):
        result = run_case(["--apply"])
        self.assertEqual(result.returncode, 2)
        self.assertIn("RECONCILE_NODES", result.stderr)

    def test_restore_path_refuses_implicit_defaults(self):
        self.assertEqual(run_case(["--restore-from-master"]).returncode, 2)

    def test_verified_file_allows_prelude(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "nodes"
            path.write_text("164.90.140.144 40403 rnode A master\n"
                            "167.99.43.214 40403 rnode B\n")
            self.assertEqual(run_case(["--apply"], path).returncode, 0)

    def test_unreadable_inventory_does_not_fall_back(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "absent"
            result = run_case(["--apply"], path)
            self.assertEqual(result.returncode, 2)
            self.assertIn("missing, unreadable, or empty", result.stderr)

    def test_unknown_controller_cannot_bypass_live_target_guard(self):
        result = run_case(["--apply"], control="sysdem")
        self.assertEqual(result.returncode, 2)
        self.assertIn("unsupported RECONCILE_CONTROL", result.stderr)

    def test_docker_control_remains_available_for_disposable_tests(self):
        self.assertEqual(run_case(["--apply"], control="docker").returncode, 0)


if __name__ == "__main__":
    unittest.main()
