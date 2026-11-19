"""The M3 join chain: every stage typed, every negative control refused.

The generic criterion suite exercises the declared mutations; this suite is the instrument's own
control. It also proves the two policies separate the instrument from the gate: a complete
local-devnet chain passes the chain policy and fails the off-host criterion.
"""
from __future__ import annotations

import copy
import hashlib
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


fixtures = _load("gate_fixtures", ROOT / "tests" / "gate_fixtures.py")
chain_tool = _load("m3_join_chain", ROOT / "tools" / "m3-join-chain.py")
principles = _load("gate_principles", ROOT / "tools" / "gate-principles.py")

STAGE_NAMES = (
    "1_request",
    "2_receipt",
    "3_construction",
    "4_transport",
    "5_receive",
    "6_apply",
)


class ChainTests(unittest.TestCase):
    def test_a_complete_chain_passes(self):
        self.assertEqual(chain_tool.check_chain(fixtures.m3_chain()), [])

    def test_a_send_log_only_chain_fails(self):
        chain = fixtures.m3_chain()
        chain["stages"]["4_transport"] = {
            "event": "send_log",
            "peer": chain["correlation"]["advertised_peer"],
            "anchor": "latest",
            "log_line": "Sent fringe response (aaaa).",
        }
        problems = chain_tool.check_chain(chain)
        self.assertTrue(any("4_transport" in problem for problem in problems), problems)

    def test_every_missing_stage_fails(self):
        for name in STAGE_NAMES:
            with self.subTest(stage=name):
                chain = fixtures.m3_chain()
                del chain["stages"][name]
                problems = chain_tool.check_chain(chain)
                self.assertTrue(any(name in problem for problem in problems), problems)

    def test_a_served_restored_mismatch_fails(self):
        chain = fixtures.m3_chain()
        chain["stages"]["6_apply"]["restored_state_hash"] = "e" * 64
        problems = chain_tool.check_chain(chain)
        self.assertTrue(any("restored_state_hash" in problem for problem in problems), problems)

    def test_a_chain_keyed_by_a_send_line_without_peer_or_anchor_fails(self):
        chain = fixtures.m3_chain()
        chain["stages"]["2_receipt"] = {
            "event": "send_log",
            "log_line": "Sent fringe response (aaaa).",
        }
        problems = chain_tool.check_chain(chain)
        self.assertTrue(any("2_receipt" in problem for problem in problems), problems)

    def test_the_fields_must_agree_with_the_log_lines(self):
        chain = fixtures.m3_chain()
        chain["stages"]["3_construction"]["state_hash"] = "f" * 64
        problems = chain_tool.check_chain(chain)
        self.assertTrue(any("3_construction" in problem for problem in problems), problems)

    def test_the_genesis_accepted_root_must_equal_the_served_state(self):
        mismatch = fixtures.m3_chain(kind="genesis", accepted_root="c" * 64)
        self.assertTrue(chain_tool.check_chain(mismatch))
        checkpoint = fixtures.m3_chain(kind="checkpoint", accepted_root="c" * 64)
        self.assertEqual(chain_tool.check_chain(checkpoint), [])

    def test_a_failed_transport_is_not_delivery(self):
        chain = fixtures.m3_chain()
        chain["stages"]["4_transport"]["outcome"] = "failed"
        chain["stages"]["4_transport"]["log_line"] = (
            f"FinalizedFringe to {chain['stages']['4_transport']['peer']} was not sent: "
            f"transport outcome failed (state {chain['correlation']['state_hash']})"
        )
        problems = chain_tool.check_chain(chain)
        self.assertTrue(any("not delivered" in problem for problem in problems), problems)

    def test_walk_completion_and_readback_are_required(self):
        chain = fixtures.m3_chain()
        chain["stages"]["6_apply"]["walk_completed"] = False
        chain["stages"]["6_apply"]["checkpoint_readback"] = False
        problems = chain_tool.check_chain(chain)
        self.assertTrue(any("walk_completed" in problem for problem in problems), problems)
        self.assertTrue(any("checkpoint_readback" in problem for problem in problems), problems)

    def test_completeness_metadata_is_required(self):
        missing = fixtures.m3_chain()
        del missing["complete"]
        self.assertTrue(chain_tool.check_chain(missing))
        string = fixtures.m3_chain()
        string["complete"] = "true"
        self.assertTrue(chain_tool.check_chain(string))
        reasons = fixtures.m3_chain()
        reasons["incomplete_reasons"] = ["a missing stage"]
        self.assertTrue(chain_tool.check_chain(reasons))
        untyped = fixtures.m3_chain()
        untyped["incomplete_reasons"] = None
        self.assertTrue(chain_tool.check_chain(untyped))
        self.assertEqual(chain_tool.check_chain(fixtures.m3_chain()), [])

    def test_the_chain_must_name_its_raw_logs(self):
        missing = fixtures.m3_chain()
        del missing["logs"]
        self.assertTrue(chain_tool.check_chain(missing))
        path = fixtures.m3_chain()
        path["logs"]["joiner"] = "../joiner.log"
        self.assertTrue(chain_tool.check_chain(path))
        reused = fixtures.m3_chain()
        reused["logs"]["joiner"] = reused["logs"]["responder"]
        problems = chain_tool.check_chain(reused)
        self.assertTrue(any("distinct" in problem for problem in problems), problems)

    def test_the_transport_words_must_agree_with_the_outcome(self):
        peer = fixtures.m3_chain()["stages"]["4_transport"]["peer"]
        state = "a" * 64
        sent_failed = fixtures.m3_chain()
        sent_failed["stages"]["4_transport"]["outcome"] = "failed"
        sent_failed["stages"]["4_transport"]["log_line"] = (
            f"FinalizedFringe sent to {peer}: transport outcome failed (state {state})"
        )
        self.assertTrue(chain_tool.check_chain(sent_failed))
        not_sent_delivered = fixtures.m3_chain()
        not_sent_delivered["stages"]["4_transport"]["log_line"] = (
            f"FinalizedFringe to {peer} was not sent: transport outcome delivered "
            f"(state {state})"
        )
        self.assertTrue(chain_tool.check_chain(not_sent_delivered))


class PolicyTests(unittest.TestCase):
    def record(self, readme: str, chain: dict) -> Path:
        directory = Path(tempfile.mkdtemp())
        (directory / "README.md").write_text(readme)
        (directory / "manifest.json").write_text(
            json.dumps({"locality": chain["locality"], "execution_proven": False})
        )
        (directory / "chain.json").write_text(json.dumps(chain))
        (directory / "raw").mkdir()
        (directory / "raw" / "peer.txt").write_text(
            f"{chain['correlation']['advertised_peer']}\n"
        )
        joiner_log, responder_log = fixtures.m3_logs(chain)
        (directory / "joiner.log").write_text(joiner_log)
        (directory / "responder.log").write_text(responder_log)
        (directory / "SHA256SUMS.json").write_text(
            json.dumps(
                {
                    "joiner.log": hashlib.sha256(joiner_log.encode()).hexdigest(),
                    "responder.log": hashlib.sha256(responder_log.encode()).hexdigest(),
                }
            )
        )
        return directory

    def test_a_local_devnet_chain_passes_its_policy_and_fails_the_off_host_policy(self):
        chain = fixtures.m3_chain(locality="local-devnet", advertised_host="172.20.0.2")
        directory = self.record(
            "# M3 local chain rehearsal\n\n"
            "**Evidence locality.** local-devnet — one Docker host.\n\n"
            "This is **not** an M3 off-host positive.\n",
            chain,
        )
        observations = {"record": {"dir": str(directory)}}
        self.assertEqual(
            principles.join_chain(observations, {"locality": "local-devnet"})["status"], "PASS"
        )
        result = principles.join_chain(observations, {"locality": "off-host"})
        self.assertEqual(result["status"], "FAIL")
        self.assertTrue(any("off-host-claim" in reason for reason in result["reasons"]))

    def test_a_loopback_advertised_peer_is_refused_by_the_off_host_policy(self):
        chain = fixtures.m3_chain(advertised_host="127.0.0.1")
        directory = self.record(
            "# M3 off-host witness\n\n"
            "**Evidence locality.** off-host-claim — the bootstrap dial-back.\n\n"
            "The dial-back result is captured in raw/peer.txt.\n",
            chain,
        )
        result = principles.join_chain(
            {"record": {"dir": str(directory)}}, {"locality": "off-host"}
        )
        self.assertEqual(result["status"], "FAIL")
        self.assertTrue(any("local" in reason for reason in result["reasons"]))

    def test_the_record_check_binds_the_quoted_lines_to_the_raw_logs(self):
        chain = fixtures.m3_chain(locality="local-devnet", advertised_host="172.20.0.2")
        directory = self.record(
            "# M3 local chain rehearsal\n\n"
            "**Evidence locality.** local-devnet — one Docker host.\n\n"
            "This is **not** an M3 off-host positive.\n",
            chain,
        )
        self.assertEqual(chain_tool.check_record(directory, "local-devnet"), (0, []))
        (directory / "joiner.log").write_text("not the captured log\n")
        code, problems = chain_tool.check_record(directory, "local-devnet")
        self.assertEqual(code, 1)
        self.assertTrue(any("quoted line" in problem for problem in problems), problems)

    def test_an_off_host_record_must_be_digest_pinned(self):
        chain = fixtures.m3_chain(advertised_host="203.0.113.7")
        directory = self.record(
            "# M3 off-host witness\n\n"
            "**Evidence locality.** off-host-claim — the bootstrap dial-back.\n\n"
            "The dial-back result is captured in raw/peer.txt.\n",
            chain,
        )
        (directory / "SHA256SUMS.json").unlink()
        result = principles.join_chain(
            {"record": {"dir": str(directory)}}, {"locality": "off-host"}
        )
        self.assertEqual(result["status"], "FAIL")
        self.assertTrue(any("SHA256SUMS" in reason for reason in result["reasons"]), result)

    def test_a_missing_chain_is_a_precondition(self):
        directory = Path(tempfile.mkdtemp())
        (directory / "README.md").write_text(
            "**Evidence locality.** local-devnet — no chain.\n"
        )
        result = principles.join_chain(
            {"record": {"dir": str(directory)}}, {"locality": "local-devnet"}
        )
        self.assertEqual(result["status"], "PRECONDITION")


class PinTests(unittest.TestCase):
    def packed_record(self) -> tuple[Path, dict]:
        chain = fixtures.m3_chain()
        directory = Path(tempfile.mkdtemp())
        (directory / "raw").mkdir()
        joiner, responder = fixtures.m3_logs(chain)
        files = {
            "raw/chain.json": json.dumps(chain).encode(),
            "raw/joiner.log": joiner.encode(),
            "raw/responder.log": responder.encode(),
        }
        entries, sums = [], {}
        for name, raw in files.items():
            (directory / name).write_bytes(raw)
            digest = hashlib.sha256(raw).hexdigest()
            sums[name] = digest
            entries.append({
                "path": name,
                "kind": "json" if name.endswith(".json") else "log",
                "sha256": digest,
                "bytes": len(raw),
            })
        (directory / "manifest.json").write_text(json.dumps({"files": entries}))
        (directory / "SHA256SUMS.json").write_text(json.dumps(sums))
        return directory, chain

    def test_the_packed_chain_document_is_itself_pinned(self):
        directory, chain = self.packed_record()
        self.assertEqual(chain_tool.pin_problems(directory, chain), [])
        (directory / "raw" / "chain.json").write_text(json.dumps(chain, indent=2))
        problems = chain_tool.pin_problems(directory, chain)
        self.assertTrue(any("chain.json" in problem for problem in problems), problems)

    def test_an_unlisted_chain_document_is_refused(self):
        directory, chain = self.packed_record()
        manifest = json.loads((directory / "manifest.json").read_text())
        manifest["files"] = [
            entry for entry in manifest["files"] if entry["path"] != "raw/chain.json"
        ]
        (directory / "manifest.json").write_text(json.dumps(manifest))
        problems = chain_tool.pin_problems(directory, chain)
        self.assertTrue(any("not listed" in problem for problem in problems), problems)

    def test_a_manifest_without_a_file_list_is_not_a_packed_record(self):
        directory, chain = self.packed_record()
        (directory / "manifest.json").write_text(json.dumps({"locality": "local-devnet"}))
        self.assertEqual(chain_tool.pin_problems(directory, chain), [])


class BuildTests(unittest.TestCase):
    def logs(self) -> tuple[str, str]:
        return fixtures.m3_logs(fixtures.m3_chain())

    def test_build_chain_correlates_two_logs(self):
        joiner, responder = self.logs()
        chain = chain_tool.build_chain(
            joiner_log=joiner,
            responder_log=responder,
            locality="local-devnet",
        )
        self.assertTrue(chain["complete"], chain["incomplete_reasons"])
        self.assertEqual(chain_tool.check_chain(chain), [])

    def test_build_chain_ignores_a_decoy_exchange(self):
        joiner, responder = self.logs()
        decoy = fixtures.m3_chain(responder="3" * 64, state="d" * 64)
        joiner = "\n".join(
            [
                "[info] " + decoy["stages"]["1_request"]["log_line"],
                "[info] " + decoy["stages"]["5_receive"]["log_line"],
                joiner,
            ]
        )
        chain = chain_tool.build_chain(
            joiner_log=joiner, responder_log=responder, locality="local-devnet"
        )
        self.assertTrue(chain["complete"], chain["incomplete_reasons"])
        self.assertEqual(chain["correlation"]["responder_key"], "2" * 64)
        self.assertEqual(chain["correlation"]["state_hash"], "a" * 64)

    def test_build_chain_marks_an_unmatched_log_incomplete(self):
        chain = chain_tool.build_chain(
            joiner_log="", responder_log="", locality="local-devnet"
        )
        self.assertFalse(chain["complete"])
        self.assertTrue(chain["incomplete_reasons"])


class CliTests(unittest.TestCase):
    def test_the_cli_exit_codes_are_pass_fail_precondition(self):
        with tempfile.TemporaryDirectory() as directory:
            valid = Path(directory) / "chain.json"
            valid.write_text(json.dumps(fixtures.m3_chain()))
            self.assertEqual(chain_tool.main(["check-chain", "--chain", str(valid)]), 0)

            broken = copy.deepcopy(fixtures.m3_chain())
            del broken["stages"]["4_transport"]
            invalid = Path(directory) / "broken.json"
            invalid.write_text(json.dumps(broken))
            self.assertEqual(chain_tool.main(["check-chain", "--chain", str(invalid)]), 1)

            self.assertEqual(
                chain_tool.main(["check-chain", "--chain", str(Path(directory) / "absent.json")]),
                3,
            )


if __name__ == "__main__":
    unittest.main()
