"""Offline fail-closed controls for the exact-17-step r-cash installer witness."""
import importlib.util
from pathlib import Path
import unittest


SPEC = importlib.util.spec_from_file_location(
    "n447_rcash", Path(__file__).resolve().parents[1] / "tools/n447-rcash-evidence.py")
helper = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(helper)


def fixture(n=17):
    return {
        "root_uri": "rho:id:abc123",
        "steps": {
            f"step-{i:02d}": {
                "deploy_id": f"deploy-{i}", "block": f"{i+1:064x}",
                "uris": ["rho:id:abc123"],
            }
            for i in range(n)
        }
    }


class AcceptanceControls(unittest.TestCase):
    def test_exact_17_included_verified_steps_pass(self):
        self.assertEqual(helper.assess(fixture())["steps_verified"], 17)

    def test_16_step_partial_install_fails(self):
        with self.assertRaises(AssertionError):
            helper.assess(fixture(16))

    def test_reused_step_is_not_a_fresh_install(self):
        case = fixture()
        case["steps"]["step-00"]["reused"] = True
        with self.assertRaises(AssertionError):
            helper.assess(case)

    def test_missing_deploy_id_fails(self):
        case = fixture()
        case["steps"]["step-00"].pop("deploy_id")
        with self.assertRaises(AssertionError):
            helper.assess(case)

    def test_missing_inclusion_anchor_fails(self):
        case = fixture()
        case["steps"]["step-00"]["block"] = "not-a-block"
        with self.assertRaises(AssertionError):
            helper.assess(case)

    def test_missing_key_fails(self):
        case = fixture()
        case["steps"]["step-00"]["uris"] = ["missing"]
        with self.assertRaises(AssertionError):
            helper.assess(case)


if __name__ == "__main__":
    unittest.main()
