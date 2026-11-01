import importlib.util
from types import SimpleNamespace
import unittest
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location("partition",ROOT/"tests/consensus-partition-rejoin.py")
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)

class ConsumerContractTests(unittest.TestCase):
    """**The harness may only read declared fields.**

    A rename in the diagnostics contract should fail this suite, not a live acceptance run. The
    health side is checked against `PUBLIC_HEALTH_KEYS`; the operator side against
    `OPERATOR_GATE_KEYS` plus the operator-only state the harness legitimately needs; the public
    status side is restricted to `autopropose` (the only field it is for, now that the tip comes from
    `/health`).
    """

    HARNESS = [
        ROOT / "tools/consensus-acceptance-drill.py",
        ROOT / "tests/consensus-partition-rejoin.py",
        # The other live /health consumers in the tree; a rename must fail this suite for them too.
        ROOT / "tests/consensus-quiet-finality.py",
    ]
    OPERATOR_ONLY = {
        "latestBlockNumber", "peers", "finalityCountersDurable", "unrestorableRecords",
        "unrestorableBlock", "undeliveredSends", "resources", "cgroupOomKills",
        # Published on the operator surface beside the gates, but not readiness terms.
        "finalityStalled", "finalityStallEpisodes",
    }
    # `autopropose` is the mode flag; `shardId` and `latestBlockNumber` are what the deploy CLI
    # needs (`--shard-id`, `--valid-after-block-number`), not a tip reading.
    PUBLIC_STATUS_FIELDS = {"autopropose", "latestBlockNumber", "shardId"}

    def declared(self):
        import re
        source = (ROOT / "node/src/diagnostics/health_contract.rs").read_text()

        def names(constant):
            match = re.search(rf"pub const {constant}: &\[&str\] = &\[(.*?)\];", source, re.S)
            self.assertIsNotNone(match, constant)
            return set(re.findall(r'"([^"]+)"', match.group(1)))

        return names("PUBLIC_HEALTH_KEYS"), names("OPERATOR_GATE_KEYS")

    def reads(self, receiver):
        import re
        found = set()
        for path in self.HARNESS:
            text = path.read_text()
            found.update(re.findall(rf'{receiver}\.get\("([^"]+)"\)', text))
            # A subscript read is the same claim as `.get`; the first version of this scan missed
            # `status["latestBlockNumber"]` and gave false assurance.
            found.update(re.findall(rf'{receiver}\["([^"]+)"\]', text))
        return found

    def test_every_health_field_the_harness_reads_is_declared(self):
        public, _ = self.declared()
        self.assertTrue(self.reads("health"), "the scan must not be vacuous")
        self.assertEqual(self.reads("health") - public, set())

    def test_every_operator_field_the_harness_reads_is_declared_or_operator_only(self):
        _, gates = self.declared()
        self.assertTrue(self.reads("operator"))
        self.assertEqual(self.reads("operator") - gates - self.OPERATOR_ONLY, set())

    def test_the_public_status_read_is_only_the_mode_flag(self):
        self.assertEqual(self.reads("status") - self.PUBLIC_STATUS_FIELDS, set())

    def test_the_harness_readiness_vocabulary_is_the_nodes(self):
        """The drill maps the node's codes to its own verdict names; the two sets must be equal."""
        import re
        rust = (ROOT / "node/src/diagnostics/health_contract.rs").read_text()
        body = rust.split("pub fn ready_reasons", 1)[1].split("\n}", 1)[0]
        codes = set(re.findall(r'reasons\.push\("([^"]+)"\)', body))
        py = (ROOT / "tools/consensus-acceptance-drill.py").read_text()
        block = py.split("READY_REASON_VERDICT_NAMES = {", 1)[1].split("}", 1)[0]
        names = set(re.findall(r'"([^"]+)":', block))
        self.assertEqual(names, codes, "the drill's readiness vocabulary drifted from the node's")


class RecoveryHarnessTests(unittest.TestCase):
    def fixture(self):
        return {"Mounts":[
            {"Type":"volume","Name":"acceptance-validator-3-data","Source":"/vol/data",
             "Destination":"/var/lib/rnode","RW":True},
            {"Type":"bind","Source":"/tmp/genesis","Destination":"/genesis","RW":False}]}
    def test_mount_order_does_not_imply_a_new_state_volume(self):
        before=self.fixture();after=self.fixture();after["Mounts"].reverse()
        self.assertEqual(m.durable_state_volume(before),m.durable_state_volume(after))
    def test_actual_state_volume_replacement_is_detected(self):
        before=self.fixture();after=self.fixture();after["Mounts"][0]["Name"]="other"
        self.assertNotEqual(m.durable_state_volume(before),m.durable_state_volume(after))
    def test_missing_or_ambiguous_state_volume_is_refused(self):
        for mounts in [[],self.fixture()["Mounts"]*2]:
            with self.assertRaises(AssertionError):m.durable_state_volume({"Mounts":mounts})


ANCHOR = {"height": 60, "hash": "ab" * 32, "node": 0, "case": "seven-ahead"}


def healthy_status(node):
    return {
        "node": node, "ready": True, "orphan_bodies": 0, "orphan_detections": 0,
        "pipeline_faults": 0, "pipeline_faults_outstanding": 0,
        "last_pipeline_fault": None, "proposal_halted": False,
        "proposal_halt_reason": None, "finality_stalled": False,
        "finality_stall_episodes": 0, "tip": 69, "height": 70,
        "ready_reasons": [], "finalized_height": 70, "finalized_lag": 0, "peers": 7,
        "finality_counters_durable": True, "unrestorable_records": 0,
        "unrestorable_block": None, "undelivered_sends": 0, "cgroup_oom_kills": 0,
    }


class RejoinAgreementTests(unittest.TestCase):
    def drill(self, *, heads=None, statuses=None, is_finalized_fn=None):
        from types import SimpleNamespace
        from unittest.mock import Mock
        run = object.__new__(m.WorkloadDrill)
        run.args = SimpleNamespace(validators=8, window=150)
        run.marker = "marker"
        run.value = 42
        run.drive = Mock()
        run.record = Mock()
        run.status_row = Mock(
            side_effect=lambda i: dict(statuses[i]) if statuses is not None else healthy_status(i))
        run.finalized_head = Mock(
            side_effect=lambda i: (heads[i]["height"], heads[i]["hash"])
            if heads is not None else (70, "ef" * 32))
        run.is_finalized = Mock(side_effect=is_finalized_fn or (lambda i, h: True))
        return run

    def heads(self):
        return [{"node": i, "height": 70, "hash": "ef" * 32} for i in range(8)]

    def settled(self, run):
        """Drive the K-sample dwell and the anchored cross-checks."""
        from unittest.mock import patch
        blocks = [{"blockNumber": ANCHOR["height"], "blockHash": ANCHOR["hash"]}]
        with patch.object(m.a, "get", return_value=blocks), \
             patch.object(m.a, "probe", return_value={"expr": [{"ExprInt": {"data": 42}}]}):
            values = [m.anchor_agreement(run, ANCHOR) for _ in range(3)]
        return values

    def test_healing_wait_does_not_inject_helper_work(self):
        from unittest.mock import patch
        run = self.drill()
        with patch.object(m.a.Drill, "wait", return_value="aligned") as wait:
            self.assertEqual(run.wait_for_healing("rejoin", lambda: True), "aligned")
        wait.assert_called_once()
        self.assertEqual(wait.call_args.kwargs, {"drive": False, "window": None})
        run.drive.assert_not_called()

    def test_different_finalized_heads_are_agreement_when_the_anchor_is_finalized(self):
        heads = self.heads()
        heads[-1] = {"node": 7, "height": 46, "hash": "cd" * 32}
        run = self.drill(heads=heads)
        values = self.settled(run)
        self.assertEqual(values[-1], heads)
        self.assertIsNone(values[0])

    def test_anchor_not_finalized_on_the_victim_keeps_waiting(self):
        run = self.drill(is_finalized_fn=lambda i, h: i != 7)
        self.assertIsNone(run.anchor_agreement(ANCHOR))

    def test_stable_anchor_false_on_an_online_validator_is_a_fork(self):
        run = self.drill(is_finalized_fn=lambda i, h: i != 3)
        for _ in range(2):
            self.assertIsNone(run.anchor_agreement(ANCHOR))
        with self.assertRaisesRegex(RuntimeError, "FAIL_FORK"):
            run.anchor_agreement(ANCHOR)

    def test_is_finalized_read_error_is_never_agreement(self):
        def broken(i, h):
            raise OSError("connection refused")
        run = self.drill(is_finalized_fn=broken)
        with self.assertRaises(OSError):
            run.anchor_agreement(ANCHOR)

    def test_same_height_conflicting_hash_is_still_a_fork(self):
        heads = self.heads()
        heads[-1] = {"node": 7, "height": 70, "hash": "cd" * 32}
        run = self.drill(heads=heads,
                         is_finalized_fn=lambda i, h: h == ANCHOR["hash"])
        with self.assertRaisesRegex(RuntimeError, "FAIL_FINALIZED_DISAGREEMENT"):
            self.settled(run)

    def test_missing_divergence_counter_cannot_certify_rejoin(self):
        statuses = [healthy_status(i) for i in range(8)]
        statuses[7]["unrestorable_records"] = None
        run = self.drill(statuses=statuses)
        self.assertIsNone(run.anchor_agreement(ANCHOR))

    def test_permanent_divergence_refuses_even_when_health_reports_ready(self):
        statuses = [healthy_status(i) for i in range(8)]
        statuses[7]["unrestorable_records"] = 1
        statuses[7]["unrestorable_block"] = "fatal-block"
        run = self.drill(statuses=statuses)
        with self.assertRaisesRegex(RuntimeError, "FAIL_PERMANENT_DIVERGENCE"):
            run.anchor_agreement(ANCHOR)

    def test_latched_pipeline_fault_is_a_named_precondition(self):
        statuses = [healthy_status(i) for i in range(8)]
        statuses[7]["pipeline_faults_outstanding"] = 1
        statuses[7]["pipeline_faults"] = 1
        statuses[7]["last_pipeline_fault"] = "missing justification"
        run = self.drill(statuses=statuses)
        with self.assertRaisesRegex(RuntimeError,
                                    "FAIL_PRECONDITION_pipelineFaultsOutstanding"):
            run.anchor_agreement(ANCHOR)

    def test_a_settled_historical_fault_does_not_withhold_agreement(self):
        statuses = [healthy_status(i) for i in range(8)]
        statuses[7]["pipeline_faults"] = 3          # lifetime count: evidence only
        statuses[7]["pipeline_faults_outstanding"] = 0
        run = self.drill(statuses=statuses)
        self.assertIsNone(run.anchor_agreement(ANCHOR))

    def test_proposal_halt_is_a_named_precondition(self):
        statuses = [healthy_status(i) for i in range(8)]
        statuses[7]["proposal_halted"] = True
        run = self.drill(statuses=statuses)
        with self.assertRaisesRegex(RuntimeError, "FAIL_PRECONDITION_proposalHalted"):
            run.anchor_agreement(ANCHOR)

    def test_a_persistent_finality_alarm_does_not_abort_the_healing_window(self):
        statuses = [healthy_status(i) for i in range(8)]
        statuses[7]["finality_stalled"] = True
        run = self.drill(statuses=statuses, is_finalized_fn=lambda i, h: i != 7)
        run.healing_samples = []
        for height in (45, 46, 47, 48):
            statuses[7]["tip"] = height
            self.assertIsNone(run.anchor_agreement(ANCHOR))
        self.assertEqual(len(run.healing_samples), 4, "one poll must count once")
        self.assertEqual(run.classify_healing_timeout(), "INCONCLUSIVE_SLOW_PROGRESS")

    def test_timeout_without_a_victim_read_is_inconclusive(self):
        run = self.drill()
        run.healing_samples = []
        self.assertEqual(run.classify_healing_timeout(), "INCONCLUSIVE_MEASUREMENT")

    def test_timeout_with_a_flat_victim_is_a_stall(self):
        run = self.drill()
        run.healing_samples = [{"victim_height": 45, "victim_peers": 7, "victim_undelivered": 0}] * 3
        self.assertEqual(run.classify_healing_timeout(), "FAIL_STALLED")

    def test_timeout_with_an_advancing_victim_is_slow_progress(self):
        run = self.drill()
        run.healing_samples = [{"victim_height": h, "victim_peers": 7, "victim_undelivered": 0} for h in (45, 46, 47)]
        self.assertEqual(run.classify_healing_timeout(), "INCONCLUSIVE_SLOW_PROGRESS")

    def test_wait_for_healing_prefixes_the_timeout_with_its_class(self):
        from unittest.mock import patch
        run = self.drill()

        def timeout(*args, **kwargs):
            run.healing_samples = [{"victim_height": 45, "victim_peers": 7, "victim_undelivered": 0}] * 3
            raise m.a.WaitTimeout("window closed")

        with patch.object(m.a.Drill, "wait", side_effect=timeout):
            with self.assertRaisesRegex(RuntimeError, "FAIL_STALLED"):
                run.wait_for_healing("rejoin", lambda: None)
        self.assertEqual(run.healing_failure_class, "FAIL_STALLED")

    def test_persistent_victim_lag_fails_at_the_existing_healing_deadline(self):
        from unittest.mock import patch
        run = self.drill(is_finalized_fn=lambda i, h: i != 7)
        with patch.object(m.time, "monotonic", side_effect=[0, 0, 151, 151]), \
             patch.object(m.time, "sleep"):
            with self.assertRaisesRegex(m.a.WaitTimeout, "did not pass within 150s"):
                run.wait("rejoined anchor", lambda: run.anchor_agreement(ANCHOR))


class HealingAnchorTests(unittest.TestCase):
    def test_epoch_wait_drives_until_the_boundary_instead_of_accepting_false(self):
        from unittest.mock import Mock, patch
        run = object.__new__(m.WorkloadDrill)
        run.args = SimpleNamespace(window=150)
        run.record = Mock()
        height = [68]
        run.finalized_head = Mock(side_effect=lambda i: (height[0], "a" * 64))
        run.drive = Mock(side_effect=lambda: height.__setitem__(0, height[0] + 1))
        with patch.object(m.time, "monotonic", return_value=0), patch.object(m.time, "sleep"):
            self.assertEqual(m.wait_for_epoch_boundary(run, range(7), 70), [70] * 7)
        self.assertEqual(run.drive.call_count, 2)

    def test_epoch_wait_expiry_is_an_inconclusive_coverage_result(self):
        from unittest.mock import Mock, patch
        run = object.__new__(m.WorkloadDrill)
        run.args = SimpleNamespace(window=150)
        run.record = Mock()
        run.finalized_head = Mock(return_value=(68, "a" * 64))
        run.drive = Mock()
        with patch.object(m.time, "monotonic", side_effect=[0, 0, 301, 301]), \
             patch.object(m.time, "sleep"):
            with self.assertRaisesRegex(RuntimeError, "INCONCLUSIVE_EPOCH_COVERAGE"):
                m.wait_for_epoch_boundary(run, range(7), 70)

    def test_epoch_target_requires_the_next_boundary_even_when_starting_on_one(self):
        for height, expected in [(9, 10), (10, 20), (60, 70), (68, 70)]:
            self.assertEqual(m.next_epoch_height(height, 10), expected)
        with self.assertRaisesRegex(RuntimeError, "INCONCLUSIVE_EPOCH_COVERAGE"):
            m.next_epoch_height(None, 10)

    def drill(self, *, fences, victim, answers, victim_answer):
        from types import SimpleNamespace
        from unittest.mock import Mock
        run = object.__new__(m.WorkloadDrill)
        run.args = SimpleNamespace(validators=8, window=150)
        run.record = Mock()
        run.finalized_head = Mock(side_effect=lambda i: (fences[i]["height"], fences[i]["hash"]))
        run.is_finalized = Mock(side_effect=lambda i, h: answers[i])
        return run, victim, victim_answer

    def test_anchor_is_the_seven_ahead_minimum_fence(self):
        from unittest.mock import patch
        fences = [{"height": 54, "hash": "aa" * 32}, {"height": 53, "hash": "bb" * 32}] + \
                 [{"height": 54, "hash": "aa" * 32}] * 5
        run, _, _ = self.drill(fences=fences, victim=None, answers=[True] * 7, victim_answer=False)
        with patch.object(m, "victim_sample",
                          return_value={"finalized_height": 45, "finalized_hash": "cc" * 32,
                                        "latest": 46}), \
             patch.object(m, "container_is_finalized", return_value=False):
            anchor = m.establish_healing_anchor(run, range(7), 7, 1)
        self.assertEqual(anchor["height"], 53)
        self.assertEqual(anchor["hash"], "bb" * 32)
        self.assertEqual(anchor["case"], "seven-ahead")

    def test_anchor_swaps_when_the_victim_fence_is_ahead(self):
        from unittest.mock import patch
        fences = [{"height": 54, "hash": "aa" * 32}] * 7
        run, _, _ = self.drill(fences=fences, victim=None, answers=[False] * 7, victim_answer=True)
        with patch.object(m, "victim_sample",
                          return_value={"finalized_height": 60, "finalized_hash": "dd" * 32,
                                        "latest": 61}), \
             patch.object(m, "container_is_finalized", return_value=True):
            anchor = m.establish_healing_anchor(run, range(7), 7, 1)
        self.assertEqual(anchor["case"], "victim-ahead")
        self.assertEqual(anchor["hash"], "dd" * 32)

    def test_anchor_refuses_when_the_victim_already_finalized_it(self):
        from unittest.mock import patch
        fences = [{"height": 54, "hash": "aa" * 32}] * 7
        run, _, _ = self.drill(fences=fences, victim=None, answers=[True] * 7, victim_answer=True)
        with patch.object(m, "victim_sample",
                          return_value={"finalized_height": 45, "finalized_hash": "cc" * 32,
                                        "latest": 46}), \
             patch.object(m, "container_is_finalized", return_value=True):
            with self.assertRaisesRegex(RuntimeError, "INCONCLUSIVE_ANCHOR"):
                m.establish_healing_anchor(run, range(7), 7, 1)

    def test_anchor_refuses_a_same_height_hash_conflict(self):
        from unittest.mock import patch
        fences = [{"height": 54, "hash": "aa" * 32}] * 7
        run, _, _ = self.drill(fences=fences, victim=None, answers=[True] * 7, victim_answer=False)
        with patch.object(m, "victim_sample",
                          return_value={"finalized_height": 54, "finalized_hash": "cc" * 32,
                                        "latest": 55}), \
             patch.object(m, "container_is_finalized", return_value=False):
            with self.assertRaisesRegex(RuntimeError, "FAIL_FINALIZED_DISAGREEMENT"):
                m.establish_healing_anchor(run, range(7), 7, 1)

    def test_anchor_retries_a_transient_online_disagreement(self):
        from unittest.mock import patch
        fences = [{"height": 54, "hash": "aa" * 32}] * 7
        answers = iter([False] * 7 + [True] * 7)
        run = object.__new__(m.WorkloadDrill)
        run.args = SimpleNamespace(validators=8, window=150)
        run.record = __import__("unittest").mock.Mock()
        run.finalized_head = __import__("unittest").mock.Mock(
            side_effect=lambda i: (54, "aa" * 32))
        run.is_finalized = __import__("unittest").mock.Mock(side_effect=lambda i, h: next(answers))
        with patch.object(m, "victim_sample",
                          return_value={"finalized_height": 45, "finalized_hash": "cc" * 32,
                                        "latest": 46}), \
             patch.object(m, "container_is_finalized", return_value=False), \
             patch.object(m.time, "sleep"):
            anchor = m.establish_healing_anchor(run, range(7), 7, 1)
        self.assertEqual(anchor["case"], "seven-ahead")
        attempts = [c.kwargs["attempt"] for c in run.record.call_args_list]
        self.assertEqual(attempts, [1, 2])

    def test_anchor_fails_after_the_attempt_budget(self):
        from unittest.mock import patch
        run = object.__new__(m.WorkloadDrill)
        run.args = SimpleNamespace(validators=8, window=150)
        run.record = __import__("unittest").mock.Mock()
        run.finalized_head = __import__("unittest").mock.Mock(side_effect=lambda i: (54, "aa" * 32))
        run.is_finalized = __import__("unittest").mock.Mock(side_effect=lambda i, h: False)
        with patch.object(m, "victim_sample",
                          return_value={"finalized_height": 45, "finalized_hash": "cc" * 32,
                                        "latest": 46}), \
             patch.object(m, "container_is_finalized", return_value=False), \
             patch.object(m.time, "sleep"):
            with self.assertRaisesRegex(RuntimeError, "FAIL_FORK"):
                m.establish_healing_anchor(run, range(7), 7, 1)


class ActiveRecoveryWitnessTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        import importlib.util
        spec=importlib.util.spec_from_file_location("active_recovery",ROOT/"tests/lfs-passive-reactivation.py")
        cls.witness=importlib.util.module_from_spec(spec);spec.loader.exec_module(cls.witness)

    def test_successful_post_restore_sequence_is_recorded(self):
        log="\n".join(f"INFO proposed and added block #{q+1} (seq {q})" for q in range(5,12))
        self.assertEqual([r["seq"] for r in self.witness.active_proposal_witness(log)],list(range(5,12)))

    def test_key_enabled_and_passive_finality_cannot_substitute_for_proposal(self):
        with self.assertRaises(AssertionError):
            self.witness.active_proposal_witness("validator key enabled; LFB advanced; no errors")

    def test_sequence_reuse_or_regression_refuses_acceptance(self):
        for q in [4,5]:
            with self.assertRaises(AssertionError):
                self.witness.active_proposal_witness(f"proposed and added block #6 (seq 5)\nproposed and added block #7 (seq {q})")



class HealingWindowTests(unittest.TestCase):
    def drill(self, series=None):
        run = object.__new__(m.WorkloadDrill)
        run.args = SimpleNamespace(validators=8, window=150)
        run.online_finality_samples = series or []
        return run

    def test_floor_without_a_measurement_or_a_gap(self):
        run = self.drill()
        self.assertEqual(run.healing_window(ANCHOR, 60), m.HEALING_FLOOR_SECONDS)
        self.assertEqual(run.healing_window(ANCHOR, 54), m.HEALING_FLOOR_SECONDS)

    def test_window_scales_with_the_gap_and_the_measured_interval(self):
        run = self.drill([(i * 10.0, 10 + i) for i in range(7)])
        self.assertAlmostEqual(run.measured_finality_interval(), 10.0)
        self.assertEqual(run.healing_window(ANCHOR, 51), m.HEALING_FLOOR_SECONDS)
        self.assertEqual(run.healing_window(ANCHOR, 40), 400)

    def test_window_is_capped(self):
        run = self.drill([(i * 60.0, 10 + i) for i in range(7)])
        self.assertEqual(run.healing_window(ANCHOR, 0), m.HEALING_CEILING_SECONDS)

    def test_a_short_series_is_not_used(self):
        run = self.drill([(0.0, 10), (5.0, 11), (10.0, 12)])
        self.assertIsNone(run.measured_finality_interval())
        self.assertEqual(run.healing_window(ANCHOR, 0), m.HEALING_FLOOR_SECONDS)

    def test_the_rotation_cuts_two_validators_and_never_the_bootstrap(self):
        self.assertEqual(m.victim_indices(8), [7, 1])
        self.assertNotIn(0, m.victim_indices(8))

    def test_rotated_victim_is_never_polled_as_online(self):
        for victim in m.victim_indices(8):
            online=m.online_validator_indices(8, victim)
            self.assertEqual(len(online), 7)
            self.assertNotIn(victim, online)
            self.assertIn(0, online)
            self.assertEqual(set(online) | {victim}, set(range(8)))
        self.assertIn(7, m.online_validator_indices(8, 1))
        with self.assertRaises(ValueError):
            m.online_validator_indices(8, 0)

    def test_queued_pre_cut_blocks_must_finish_before_stable_dwell(self):
        from unittest.mock import patch
        observations=[{"tip": 32}, {"tip": 33}, {"tip": 34}]
        states=iter([
            {"tip": 35, "peers": 1, "finalized_height": 25, "autopropose": False},
            {"tip": 35, "peers": 1, "finalized_height": 25, "autopropose": False},
            {"tip": 35, "peers": 1, "finalized_height": 25, "autopropose": False},
        ])
        with patch.object(m, "victim_sample", side_effect=lambda victim: next(states)), \
             patch.object(m.time, "sleep") as pause:
            self.assertTrue(m.verify_isolated_tip_dwell("acceptance-validator-7",observations,max_extra_samples=3))
        self.assertEqual(pause.call_count,3)
        self.assertEqual([x["tip"] for x in observations[-3:]],[35]*3)

    def test_moving_or_unobservable_victim_never_passes(self):
        from unittest.mock import patch
        observations=[{"tip": 32}, {"tip": 33}, {"tip": 34}]
        state=iter([{"tip": i, "peers": 1, "finalized_height": 25, "autopropose": False}
                    for i in (35,36,37)])
        with patch.object(m, "victim_sample", side_effect=lambda victim: next(state)), \
             patch.object(m.time, "sleep"):
            self.assertFalse(m.verify_isolated_tip_dwell("acceptance-validator-7",observations,max_extra_samples=3))
        errors=[{"error": "Connection refused"}]*3
        with patch.object(m, "victim_sample", side_effect=OSError("unreachable")), \
             patch.object(m.time, "sleep"):
            self.assertFalse(m.verify_isolated_tip_dwell("acceptance-validator-7",errors,max_extra_samples=3))

    def test_epoch_coverage_of_the_missing_range(self):
        self.assertTrue(m.epoch_boundary_crossed(45, 54, 10))
        self.assertFalse(m.epoch_boundary_crossed(51, 54, 10))
        self.assertFalse(m.epoch_boundary_crossed(10, 19, 10))
        self.assertTrue(m.epoch_boundary_crossed(9, 10, 10))


class IncidentReplayTests(unittest.TestCase):
    """The run that prompted the change, replayed from its committed trace."""
    TRACE = ROOT/"spec/audit/evidence/rejoin-acceptance-20261031/run-41208692673-cycle2-trace.json"

    def trace(self):
        import json
        return json.loads(self.TRACE.read_text())

    def test_the_incident_is_slow_progress_not_a_stall(self):
        trace = self.trace()
        samples = [{"victim_height": p["latest"][-1]} for p in trace["probes"]
                   if p["latest"][-1] is not None]
        run = object.__new__(m.WorkloadDrill)
        run.healing_samples = samples
        self.assertEqual(run.classify_healing_timeout(), "INCONCLUSIVE_SLOW_PROGRESS")
        self.assertGreater(len({x["victim_height"] for x in samples}), 5)

    def test_the_online_nodes_never_had_one_identity(self):
        final = self.trace()["probes"][-1]["latest"]
        self.assertGreater(len(set(final[:-1])), 1)
        self.assertLess(final[-1], max(final[:-1]))

    def test_the_victim_finalized_the_anchor_inside_the_window(self):
        trace = self.trace()
        victim = [(r["utc"], r["height"]) for r in trace["finalized"] if r["node"] == 7]
        anchor_height = 54
        first = min(utc for utc, height in victim if height >= anchor_height)
        self.assertLessEqual(first - trace["healing_start_utc"], 150)

class PartitionFailureEvidenceTests(unittest.TestCase):
    def test_named_inconclusive_and_precondition_failures_survive_campaign_serialization(self):
        import json
        from tempfile import TemporaryDirectory
        from unittest.mock import Mock, patch
        for code in ["INCONCLUSIVE_EPOCH_COVERAGE", "FAIL_PRECONDITION_orphanBodies"]:
            with TemporaryDirectory() as directory, patch.object(m, "cmd", side_effect=OSError):
                with self.assertRaises(RuntimeError):
                    with m.record_cycle(Path(directory), [], {"cycle": 1, "phase": "healing"}, Mock()):
                        raise RuntimeError(f"{code}: details")
                campaign=json.loads((Path(directory)/"campaign.json").read_text())
                self.assertEqual(campaign["cycles"][0]["failure_class"], code)
                self.assertEqual(campaign["aggregate"], "INCONCLUSIVE" if code.startswith("INCONCLUSIVE_") else "FAIL")

    def test_failure_preserves_cycle_progress_and_primary_error(self):
        import json
        from tempfile import TemporaryDirectory
        from unittest.mock import Mock
        with TemporaryDirectory() as directory:
            out=Path(directory);rounds=[]
            row={"cycle":1,"phase":"healing","partition_progress_observed":True}
            capture=Mock(side_effect=RuntimeError("diagnostics unavailable"))
            primary=RuntimeError("healing timed out")
            with self.assertRaises(RuntimeError) as caught:
                with m.record_cycle(out,rounds,row,capture):
                    raise primary
            self.assertIs(caught.exception,primary)
            result=json.loads((out/"partition-result.json").read_text())
            self.assertEqual(result["failed_phase"],"healing")
            self.assertEqual(result["partition_cycles_completed"],0)
            self.assertTrue(result["rounds"][0]["partition_progress_observed"])
            self.assertEqual(result["rounds"][0]["capture_error"],"diagnostics unavailable")
            capture.assert_called_once_with("partition-1-healing-failed")
            self.assertEqual(json.loads((out/"partition-observations.json").read_text()),rounds)

    def test_successful_cycle_does_not_emit_a_campaign_pass(self):
        from tempfile import TemporaryDirectory
        from unittest.mock import Mock
        with TemporaryDirectory() as directory:
            out=Path(directory);rounds=[];capture=Mock()
            with m.record_cycle(out,rounds,{"cycle":1,"phase":"retention"},capture):
                pass
            self.assertEqual(rounds[0]["result"],"PASS")
            self.assertFalse((out/"partition-result.json").exists())
            capture.assert_not_called()

    def test_later_failure_retains_completed_cycle(self):
        import json
        from tempfile import TemporaryDirectory
        from unittest.mock import Mock
        with TemporaryDirectory() as directory:
            out=Path(directory);rounds=[{"cycle":1,"result":"PASS"}]
            with self.assertRaisesRegex(AssertionError,"online progress"):
                with m.record_cycle(out,rounds,{"cycle":2,"phase":"online-progress-verdict",
                                               "partition_progress_observed":False,
                                               "rejoined":[{"height":59}]},Mock()):
                    raise AssertionError("online progress")
            result=json.loads((out/"partition-result.json").read_text())
            self.assertEqual(result["partition_cycles_completed"],1)
            self.assertEqual(result["failed_cycle"],2)
            self.assertFalse(result["rounds"][1]["partition_progress_observed"])
            self.assertEqual(result["rounds"][1]["rejoined"],[{"height":59}])

    def test_main_captures_healing_timeout_before_cleanup(self):
        import itertools
        import json
        from tempfile import TemporaryDirectory
        from unittest.mock import Mock,patch
        with TemporaryDirectory() as directory:
            out=Path(directory)
            (out/"workload-result.json").write_text(json.dumps({"initial_finalized":[]}))
            run=Mock(out=out,marker="marker",value=42)
            run.wait_for_healing.side_effect=RuntimeError("healing timed out")
            run.healing_failure_class="INCONCLUSIVE_SLOW_PROGRESS"
            run.finalized_head.return_value=(13,"b"*64)
            run.measured_finality_interval.return_value=2.0
            run.healing_window.return_value=270
            captures=[]
            def capture(*args):
                captures.append(run.out.name)
            mount={"Mounts":[{"Type":"volume","Name":"state","Source":"/vol/state",
                              "Destination":"/var/lib/rnode","RW":True}]}
            def command(*args):
                return "{}" if "-f" in args else json.dumps([mount])
            clock=itertools.count()
            sample={"tip":10,"height":11,"autopropose":False,"peers":0,"finalized_height":9,
                    "finalized_hash":"c"*64,"ready":True,"ready_reasons":[]}
            with patch.dict(m.os.environ,{"GITHUB_ACTIONS":"true","RUNNER_ENVIRONMENT":"github-hosted",
                                          "DEVNET_PREFIX":"acceptance"}), \
                 patch("sys.argv",["partition","--validators","4","--out",directory]), \
                 patch.object(m,"WorkloadDrill",return_value=run), \
                 patch.object(m,"cmd",side_effect=command), \
                 patch.object(m,"victim_sample",return_value=sample), \
                 patch.object(m,"establish_healing_anchor",return_value=ANCHOR), \
                 patch.object(m.a,"get"),patch.object(m.a,"probe"), \
                 patch.object(m.a,"finalized_identity",
                              side_effect=[(10,"a")] + [(11,"b")]*3 + [(12,"b")]*3 + [(13,"b")]*3), \
                 patch.object(m.a,"marker_present",return_value=True), \
                 patch.object(m.time,"monotonic",side_effect=lambda: next(clock)), \
                 patch.object(m.time,"sleep"), \
                 patch.object(m.a.causal_trace,"capture",side_effect=capture), \
                 patch.object(m.subprocess,"run") as cleanup:
                with self.assertRaisesRegex(RuntimeError,"healing timed out"):
                    m.main()
                self.assertEqual(cleanup.call_count,2)
                self.assertEqual([c.args[0][-1] for c in cleanup.call_args_list],
                                 ["acceptance-validator-3","acceptance-validator-1"])
                for call in cleanup.call_args_list:
                    self.assertEqual(call.kwargs["timeout"],25)
            self.assertEqual(captures,["before-partition","partition-1-before-healing",
                                       "partition-1-rejoin-failed"])
            result=json.loads((out/"partition-result.json").read_text())
            self.assertEqual(result["failed_phase"],"rejoin")
            self.assertEqual(result["failure_class"],"INCONCLUSIVE_SLOW_PROGRESS")
            self.assertTrue(result["rounds"][0]["partition_progress_observed"])
            self.assertEqual(result["rounds"][0]["healing_anchor"],ANCHOR)
            run.fixed_snapshots.assert_not_called()



class RecoveryStartupTests(unittest.TestCase):
    def test_node_ports_are_reserved_in_generated_container_arguments(self):
        import re, shlex, subprocess
        text = (ROOT / "tools/devnet.sh").read_text()
        body = re.search(r"(?ms)^docker_opts\(\) \{\n(.*?)^\}", text).group(1)
        for name in ("devnet-bootstrap", "devnet-validator-1"):
            script = ('BOOTSTRAP=devnet-bootstrap; NETWORK=devnet; ADMIN=true; '
                      'DEVNET_SKIP_JOINER_GRPC_PUBLISH=1; DEVNET_NODE_MEMORY=2g; '
                      'CONTRACTS_DIR=/contracts; data_volume() { echo node-data; }; '
                      'docker_opts() {\n' + body + '\n}; '
                      'docker_opts ' + name + ' 40402 40403 40405')
            result = subprocess.run(["bash", "-c", script], capture_output=True, text=True, check=True)
            args = shlex.split(result.stdout)
            self.assertEqual(args[args.index("--sysctl")+1],
                             "net.ipv4.ip_local_reserved_ports=40400-40405")

    def test_listener_failure_is_a_setup_error_not_a_missing_reply(self):
        from types import SimpleNamespace
        from unittest.mock import patch
        spec = importlib.util.spec_from_file_location("anchor_startup", ROOT / "tests/lfs-anchored-checkpoint.py")
        anchor = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(anchor)
        def command(*args):
            return "40400-40405" if "exec" in args else "99"
        with patch.dict(anchor.os.environ, {"LFS_INITIAL_REPLY_LOSS": "1"}), \
             patch.object(anchor.adversarial, "reset"), \
             patch.object(anchor.positive, "cmd", side_effect=command), \
             patch.object(anchor.subprocess, "run", return_value=SimpleNamespace(stdout="")), \
             patch.object(anchor.adversarial, "run", return_value="Server error: the deploy gRPC listener failed: Address already in use"), \
             patch.object(anchor.time, "monotonic", side_effect=[0, 0, 0]), \
             patch.object(anchor, "save"):
            with self.assertRaisesRegex(AssertionError, "SETUP_JOINER_STARTUP"):
                anchor.reset_with_reply_loss("172.18.0.2", "node-id", "genesis")


if __name__=="__main__":unittest.main()
