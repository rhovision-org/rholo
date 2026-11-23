"""The mixed-store episode's pure half.

DE-18 is a prior-format contract path that stalls while the upgrade reports success. The mixed arm
measures it by letting the prior binary finalise an epoch boundary and install a contract, then
invoking the contract after the swap. The episode is derived from those facts, never authored: a
resolved result is a safeguard measurement (`deviation_observed: false`); a missing result is the
deviation; and an unexercised trigger names what was missing. The store-format summary records
whether the run crossed a format boundary, so the record cannot imply a v3→v4 advance it did not
make.
"""
import importlib.util
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


claim = _load("release_claim_7", ROOT / "tests" / "release-claim-7.py")


def _episode(*, result_present: bool = True, boundary_crossed: bool = True,
             install_present: bool = True, old_sha: str = "1" * 64,
             new_sha: str = "2" * 64, reread_marker: bool = True,
             reread_triple: bool = True) -> dict:
    return claim.mixed_store_episode(
        boundary={"epoch_length": 10, "crossed": boundary_crossed, "height": 11},
        old_sha=old_sha, new_sha=new_sha,
        install={"deploy_id": "ab" * 32, "submitted_via": 3, "height": 10,
                 "hash": "cd" * 32, "marker_present": install_present},
        invoke={"deploy_id": "ef" * 32, "height": 12, "hash": "12" * 32,
                "settled": True, "error": None},
        reread={"height": 10, "hash": "cd" * 32, "marker_present": reread_marker,
                "state_triple_matches": reread_triple, "error": None},
        store={"validator": 3, "written_build": 4, "read_build": 4,
               "read_present": "4", "read_decision": "accept", "same_format": True,
               "format_boundary_crossed": False},
        result_present=result_present)


class MixedStoreEpisode(unittest.TestCase):
    def test_a_resolved_prior_path_is_a_safeguard_measurement(self) -> None:
        episode = _episode()
        self.assertEqual(episode["deviations"], ["DE-18"])
        self.assertEqual(
            (episode["cause"], episode["shape"], episode["representation"]),
            ("mixed-version", "terminal", "protocol-stall"))
        self.assertEqual(episode["rung"], "R3")
        self.assertEqual(episode["law"], "Law 53b")
        exercise = episode["exercise"]
        self.assertTrue(exercise["trigger_exercised"])
        self.assertIsNone(exercise["not_exercised_reason"])
        self.assertFalse(exercise["deviation_observed"])
        self.assertTrue(exercise["safeguard_observed"])

    def test_a_missing_result_is_the_deviation(self) -> None:
        exercise = _episode(result_present=False)["exercise"]
        self.assertTrue(exercise["deviation_observed"])
        self.assertFalse(exercise["safeguard_observed"])

    def test_a_prior_block_that_does_not_re_derive_is_the_deviation(self) -> None:
        exercise = _episode(reread_triple=False)["exercise"]
        self.assertTrue(exercise["deviation_observed"])

    def test_an_unfinalised_boundary_is_not_an_exercised_trigger(self) -> None:
        exercise = _episode(boundary_crossed=False)["exercise"]
        self.assertFalse(exercise["trigger_exercised"])
        self.assertIn("epoch boundary", exercise["not_exercised_reason"])
        self.assertFalse(exercise["deviation_observed"])
        self.assertFalse(exercise["safeguard_observed"])

    def test_identical_binaries_are_not_an_exercised_trigger(self) -> None:
        exercise = _episode(old_sha="3" * 64, new_sha="3" * 64)["exercise"]
        self.assertFalse(exercise["trigger_exercised"])
        self.assertIn("distinct binaries", exercise["not_exercised_reason"])

    def test_an_unreadable_install_is_not_an_exercised_trigger(self) -> None:
        exercise = _episode(install_present=False)["exercise"]
        self.assertFalse(exercise["trigger_exercised"])
        self.assertIn("install", exercise["not_exercised_reason"])


class StoreFormatSummary(unittest.TestCase):
    def test_a_same_format_swap_does_not_cross_a_boundary(self) -> None:
        summary = claim._store_format_summary(
            [{"build": 4, "decision": "stamp", "present": None}],
            [{"build": 4, "decision": "accept", "present": "4"}])
        self.assertTrue(summary["same_format"])
        self.assertFalse(summary["format_boundary_crossed"])

    def test_a_prior_format_store_is_visible(self) -> None:
        summary = claim._store_format_summary(
            [{"build": 3, "decision": "stamp", "present": None}],
            [{"build": 4, "decision": "accept", "present": "4"}])
        self.assertFalse(summary["same_format"])
        self.assertTrue(summary["format_boundary_crossed"])


if __name__ == "__main__":
    unittest.main()
