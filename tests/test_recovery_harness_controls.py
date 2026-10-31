import importlib.util
from types import SimpleNamespace
import unittest
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location("partition",ROOT/"tests/consensus-partition-rejoin.py")
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)

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
        "pipeline_faults": 0, "last_pipeline_fault": None, "proposal_halted": False,
        "proposal_halt_reason": None, "finality_stalled": False,
        "finality_stall_episodes": 0, "latest": 70, "peers": 7,
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
        statuses[7]["pipeline_faults"] = 1
        statuses[7]["last_pipeline_fault"] = "missing justification"
        run = self.drill(statuses=statuses)
        with self.assertRaisesRegex(RuntimeError, "FAIL_PRECONDITION_pipelineFaults"):
            run.anchor_agreement(ANCHOR)

    def test_proposal_halt_is_a_named_precondition(self):
        statuses = [healthy_status(i) for i in range(8)]
        statuses[7]["proposal_halted"] = True
        run = self.drill(statuses=statuses)
        with self.assertRaisesRegex(RuntimeError, "FAIL_PRECONDITION_proposalHalted"):
            run.anchor_agreement(ANCHOR)

    def test_the_stall_alarm_must_persist_before_it_fails(self):
        statuses = [healthy_status(i) for i in range(8)]
        statuses[7]["finality_stalled"] = True
        run = self.drill(statuses=statuses)
        self.assertIsNone(run.anchor_agreement(ANCHOR))
        self.assertIsNone(run.anchor_agreement(ANCHOR))
        with self.assertRaisesRegex(RuntimeError, "FAIL_STALLED"):
            run.anchor_agreement(ANCHOR)

    def test_timeout_without_a_victim_read_is_inconclusive(self):
        run = self.drill()
        run.healing_samples = []
        self.assertEqual(run.classify_healing_timeout(), "INCONCLUSIVE_MEASUREMENT")

    def test_timeout_with_a_flat_victim_is_a_stall(self):
        run = self.drill()
        run.healing_samples = [{"victim_height": 45}] * 3
        self.assertEqual(run.classify_healing_timeout(), "FAIL_STALLED")

    def test_timeout_with_an_advancing_victim_is_slow_progress(self):
        run = self.drill()
        run.healing_samples = [{"victim_height": h} for h in (45, 46, 47)]
        self.assertEqual(run.classify_healing_timeout(), "INCONCLUSIVE_SLOW_PROGRESS")

    def test_wait_for_healing_prefixes_the_timeout_with_its_class(self):
        from unittest.mock import patch
        run = self.drill()

        def timeout(*args, **kwargs):
            run.healing_samples = [{"victim_height": 45}] * 3
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
            captures=[]
            def capture(*args):
                captures.append(run.out.name)
            mount={"Mounts":[{"Type":"volume","Name":"state","Source":"/vol/state",
                              "Destination":"/var/lib/rnode","RW":True}]}
            def command(*args):
                return "{}" if "-f" in args else json.dumps([mount])
            clock=itertools.count()
            sample={"latest":10,"autopropose":False,"peers":0,"finalized_height":9,
                    "finalized_hash":"c"*64}
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
                cleanup.assert_called_once()
                self.assertEqual(cleanup.call_args.kwargs["timeout"],25)
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
