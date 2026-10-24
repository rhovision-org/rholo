import importlib.util
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


class RejoinAgreementTests(unittest.TestCase):
    def drill(self, identities):
        from types import SimpleNamespace
        from unittest.mock import Mock
        run = object.__new__(m.WorkloadDrill)
        run.args = SimpleNamespace(validators=8, window=150)
        run.finalized_marker = Mock(return_value=identities)
        run.drive = Mock()
        return run

    def heads(self):
        return [{"node":i,"height":70,"hash":"ab"*32} for i in range(8)]

    def test_healing_wait_does_not_inject_helper_work(self):
        from unittest.mock import patch
        run = self.drill(self.heads())
        with patch.object(m.a.Drill, "wait", return_value="aligned") as wait:
            self.assertEqual(run.wait_for_healing("rejoin", lambda: True), "aligned")
        wait.assert_called_once()
        self.assertEqual(wait.call_args.kwargs, {"drive": False})
        run.drive.assert_not_called()

    def test_marker_retention_at_different_heights_is_not_agreement(self):
        heads = self.heads()
        heads[-1]["height"] = 60
        self.assertIsNone(self.drill(heads).finalized_marker_agreement(60))

    def test_equal_heights_with_different_hashes_are_not_agreement(self):
        heads = self.heads()
        heads[-1]["hash"] = "cd"*32
        self.assertIsNone(self.drill(heads).finalized_marker_agreement(60))

    def test_missing_or_duplicate_validator_cannot_prove_agreement(self):
        heads = self.heads()
        duplicate = self.heads()
        duplicate[-1]["node"] = 0
        for sample in [None, heads[:-1], duplicate]:
            self.assertIsNone(self.drill(sample).finalized_marker_agreement(60))

    def test_all_eight_matching_heads_preserve_the_marker_check(self):
        heads = self.heads()
        run = self.drill(heads)
        self.assertEqual(run.finalized_marker_agreement(60), heads)
        run.finalized_marker.assert_called_once_with(60, required=True)

    def test_persistent_disagreement_fails_at_the_existing_healing_deadline(self):
        from unittest.mock import patch
        heads = self.heads()
        heads[-1]["height"] = 60
        run = self.drill(heads)
        with patch.object(m.time,"monotonic",side_effect=[0,0,151]), \
             patch.object(m.time,"sleep"):
            with self.assertRaisesRegex(RuntimeError,"did not pass within 150s"):
                run.wait("rejoined finalized identity",lambda:run.finalized_marker_agreement(60))


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
        import json
        from tempfile import TemporaryDirectory
        from unittest.mock import Mock,patch
        with TemporaryDirectory() as directory:
            out=Path(directory)
            (out/"result.json").write_text(json.dumps({"initial_finalized":[]}))
            run=Mock(out=out,marker="marker",value=42)
            run.wait_for_healing.side_effect=RuntimeError("healing timed out")
            captures=[]
            def capture(*args):
                captures.append(run.out.name)
            mount={"Mounts":[{"Type":"volume","Name":"state","Source":"/vol/state",
                              "Destination":"/var/lib/rnode","RW":True}]}
            def command(*args):
                return "{}" if "-f" in args else json.dumps([mount])
            with patch.dict(m.os.environ,{"GITHUB_ACTIONS":"true","RUNNER_ENVIRONMENT":"github-hosted",
                                          "DEVNET_PREFIX":"acceptance"}), \
                 patch("sys.argv",["partition","--validators","4","--out",directory]), \
                 patch.object(m,"WorkloadDrill",return_value=run), \
                 patch.object(m,"cmd",side_effect=command), \
                 patch.object(m.a,"get"),patch.object(m.a,"probe"), \
                 patch.object(m.a,"finalized_identity",side_effect=[(10,"a"),(13,"b"),(13,"b"),(13,"b")]), \
                 patch.object(m.a,"marker_present",return_value=True), \
                 patch.object(m.time,"monotonic",side_effect=[0,1,2]), \
                 patch.object(m.a.causal_trace,"capture",side_effect=capture), \
                 patch.object(m.subprocess,"run") as cleanup:
                with self.assertRaisesRegex(RuntimeError,"healing timed out"):
                    m.main()
                cleanup.assert_called_once()
                self.assertEqual(cleanup.call_args.kwargs["timeout"],25)
            self.assertEqual(captures,["before-partition","partition-1-before-healing","partition-1-healing-failed"])
            result=json.loads((out/"partition-result.json").read_text())
            self.assertEqual(result["failed_phase"],"healing")
            self.assertTrue(result["rounds"][0]["partition_progress_observed"])
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
