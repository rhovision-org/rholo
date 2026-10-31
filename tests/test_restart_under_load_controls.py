"""Each arm predicate must fail on a bad sample (the arm's own offline control)."""

import importlib.util
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "restart_controls", Path(__file__).with_name("restart_under_load_controls.py")
)
controls = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(controls)


def test_agreement_rejects_disagreement_at_equal_height():
    assert controls.agreement_verdict([(10, "aa"), (10, "bb")], 3) is not None
    assert controls.agreement_verdict([(10, "aa"), (9, "aa")], 3) is not None


def test_agreement_rejects_a_height_below_the_floor():
    assert controls.agreement_verdict([(2, "aa"), (2, "aa")], 3) is not None


def test_agreement_accepts_one_identity_above_the_floor():
    assert controls.agreement_verdict([(10, "aa"), (10, "aa")], 3) is None


def test_baseline_rejects_an_ungreed_baseline():
    assert controls.baseline_verdict([(5, "aa"), (5, "bb")]) is not None
    assert controls.baseline_verdict([]) is not None


def test_a_deploy_that_is_not_resolved_fails():
    assert not controls.deploy_is_success({"NotProcessed": {"status": "Unknown"}})
    assert not controls.deploy_is_success({"ProcessedWithError": {"deployError": "boom"}})
    assert controls.deploy_is_success({"ProcessedWithSuccess": {"deployResult": []}})


def test_health_rejects_each_fault_field():
    good = {"pipelineFaults": 0, "orphanBodies": 0, "proposalHalted": False}
    assert controls.health_verdict(good) is None
    for field, value in [
        ("pipelineFaults", 1),
        ("orphanBodies", 1),
        ("proposalHalted", True),
    ]:
        bad = dict(good)
        bad[field] = value
        assert controls.health_verdict(bad) is not None, field
    missing = {"pipelineFaults": 0, "orphanBodies": 0}
    assert controls.health_verdict(missing) is not None, "a missing field is not an all-clear"


def test_a_silent_restart_is_not_a_proposal():
    assert not controls.proposed_after_restart("started; syncing")
    assert controls.proposed_after_restart("proposed and added block #12 (seq 3)")


if __name__ == "__main__":
    # Runnable without pytest: every predicate's failing sample is asserted here too.
    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            fn()
            print(f"ok {name}")
