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
    good = {"pipelineFaultsOutstanding": 0, "orphanBodies": 0, "proposalHalted": False}
    assert controls.health_verdict(good) is None
    for field, value in [
        ("pipelineFaultsOutstanding", 1),
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


def test_receiver_verdict_accepts_a_coherent_reading_below_the_cap():
    good = {"capacity": 1024, "pending": 3, "sampledPeak": 7}
    assert controls.receiver_verdict({"blockPipeline": good}) is None
    assert controls.receiver_verdict({"blockPipeline": good}, allow_full=True) is None


def test_receiver_verdict_rejects_each_bad_shape():
    good = {"capacity": 1024, "pending": 3, "sampledPeak": 7}
    for bad in [
        None,
        {},
        {"blockPipeline": None},
        {"blockPipeline": {**good, "capacity": 512}},
        {"blockPipeline": {**good, "pending": -1}},
        {"blockPipeline": {**good, "sampledPeak": 1025}},
        {"blockPipeline": {**good, "sampledPeak": 2}},
        {"blockPipeline": {**good, "pending": 1024}},
        {"blockPipeline": {**good, "pending": True}},
    ]:
        assert controls.receiver_verdict(bad) is not None, bad
    full = {"blockPipeline": {**good, "pending": 1024, "sampledPeak": 1024}}
    assert controls.receiver_verdict(full, allow_full=True) is None


def test_wrong_genesis_requires_a_named_refusal_and_no_running_state():
    log = ("restored genesis 11 differs from pinned genesis 22; refusing to enter Running")
    assert controls.wrong_genesis_verdict(1, log, False) is None
    assert controls.wrong_genesis_verdict(0, log, False) is not None
    assert controls.wrong_genesis_verdict(1, "", False) is not None
    assert controls.wrong_genesis_verdict(1, log, True) is not None
    lfs = ("LFS state sync failed: recovery genesis 11 differs from pinned genesis 22\n"
           "Fatal: NodeLaunch exited with error: LFS state sync is terminal")
    assert controls.wrong_genesis_verdict(1, lfs, False) is None


def test_incomplete_ancestry_requires_the_missing_fringe_refusal():
    log = ("LFS bootstrap supplied no authenticated nonempty fringe after 10 requests; "
           "refusing to enter Running")
    assert controls.incomplete_ancestry_verdict(1, log, False) is None
    assert controls.incomplete_ancestry_verdict(0, log, False) is not None
    assert controls.incomplete_ancestry_verdict(1, "refusing to enter Running", False) is not None
    assert controls.incomplete_ancestry_verdict(1, log, True) is not None


def test_a_restarted_victim_needs_a_certificate_covering_the_kill():
    assert controls.certificate_verdict(
        {"kind": "checkpoint", "digest": "ab", "anchorHeight": 30}, 25) is None
    assert controls.certificate_verdict(
        {"kind": "genesis", "digest": None, "anchorHeight": None}, 25) is not None
    assert controls.certificate_verdict(None, 25) is not None
    assert controls.certificate_verdict(
        {"kind": "checkpoint", "digest": "ab", "anchorHeight": 20}, 25) is not None
    assert controls.certificate_verdict(
        {"kind": "checkpoint", "digest": "ab"}, 25) is not None


def test_interrupted_restart_requires_the_expected_sequence():
    assert controls.interrupted_restart_verdict(["1-1", "2-2", "3-3"], "recovered") is None
    assert controls.interrupted_restart_verdict(["1-1", "2-2"], "recovered") is not None
    assert controls.interrupted_restart_verdict(["1-1", "2-2", "3-3"], "recovered",
                                                ready_error="timeout") is not None
    assert controls.interrupted_restart_verdict(["1-1", "2-2"], "detected",
                                                ready_error="timeout") is None
    assert controls.interrupted_restart_verdict(["1-1", "2-2"], "detected") is not None
    assert controls.interrupted_restart_verdict([], "recovered") is not None
    assert controls.interrupted_restart_verdict(["1-1", "2-2"], "other") is not None


if __name__ == "__main__":
    # Runnable without pytest: every predicate's failing sample is asserted here too.
    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            fn()
            print(f"ok {name}")
