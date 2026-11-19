"""Fail-closed M3 off-host evidence controls for #613 (synthetic, offline only).

These tests prove checker behavior, NOT existence of an off-host run or
cryptographic provenance of the captured raw logs.
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


def _module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


checker = _module("m3_checker_sources", ROOT / "tools" / "m3-join-chain.py")
fixtures = _module("m3_checker_fixtures", ROOT / "tests" / "gate_fixtures.py")


def make_record(base: Path) -> tuple[Path, dict]:
    record = base / "offhost-synthetic"
    (record / "raw").mkdir(parents=True)
    chain = fixtures.m3_chain(locality="off-host-claim", advertised_host="203.0.113.25")
    chain["sources"] = {
        "joiner_log": "raw/joiner.log",
        "responder_log": "raw/responder.log",
    }
    stages = chain["stages"]
    joiner = [
        stages["1_request"]["log_line"],
        stages["5_receive"]["log_line"],
        stages["6_apply"]["walk_log_line"],
        stages["6_apply"]["log_line"],
    ]
    responder = [
        stages["2_receipt"]["log_line"],
        stages["3_construction"]["log_line"],
        stages["4_transport"]["log_line"],
    ]
    (record / "raw" / "joiner.log").write_text("\n".join(joiner) + "\n")
    (record / "raw" / "responder.log").write_text("\n".join(responder) + "\n")
    (record / "raw" / "chain.json").write_text(json.dumps(chain, indent=2) + "\n")
    (record / "README.md").write_text(
        "# Synthetic off-host checker test\n"
        "**Evidence locality.** off-host-claim — bootstrap dial-back.\n"
        "This is an offline fixture; it does not prove a live run.\n"
    )
    repack(record)
    return record, chain


def repack(record: Path) -> None:
    items = []
    sums = {}
    for name, kind in (
        ("raw/chain.json", "json"),
        ("raw/joiner.log", "log"),
        ("raw/responder.log", "log"),
    ):
        raw = (record / name).read_bytes()
        digest = hashlib.sha256(raw).hexdigest()
        items.append({
            "path": name, "kind": kind,
            "sha256": digest, "bytes": len(raw),
        })
        sums[name] = digest
    (record / "manifest.json").write_text(json.dumps({
        "schema": 1, "locality": "off-host-claim",
        "execution_proven": False, "files": items,
    }))
    (record / "SHA256SUMS.json").write_text(json.dumps(sums))


class M3WitnessIntegrityControls(unittest.TestCase):
    def test_complete_is_required_as_literal_true_and_reasons_empty(self):
        for bad in (None, False, "true", 1, [], {}):
            with self.subTest(complete=repr(bad)):
                chain = fixtures.m3_chain()
                chain["complete"] = bad
                self.assertTrue(any("complete" in p for p in checker.check_chain(chain)))
        chain = fixtures.m3_chain()
        del chain["complete"]
        self.assertTrue(any("complete" in p for p in checker.check_chain(chain)))
        for bad in (None, "none", ["pending"], [123], {}):
            with self.subTest(reasons=repr(bad)):
                chain = fixtures.m3_chain()
                chain["incomplete_reasons"] = bad
                self.assertTrue(any("incomplete_reasons" in p or "incomplete" in p
                                    for p in checker.check_chain(chain)))

    def test_not_sent_delivered_is_rejected_even_with_matching_fields(self):
        chain = fixtures.m3_chain()
        stage = chain["stages"]["4_transport"]
        stage["log_line"] = (
            "FinalizedFringe to " + stage["peer"] +
            " was not sent: transport outcome delivered (state " +
            stage["state_hash"] + ")"
        )
        stage["outcome"] = "delivered"
        self.assertTrue(any("4_transport" in p for p in checker.check_chain(chain)))

    def test_distinct_pinned_logs_and_embedded_lines_are_required(self):
        with tempfile.TemporaryDirectory() as d:
            record, chain = make_record(Path(d))
            self.assertEqual(checker.check_chain(chain), [])
            self.assertEqual(checker.check_pinned_sources(record, chain), [])
            self.assertEqual(checker.check_record(record, "off-host")[0], 0)
            self.assertEqual(checker.check_record(record, "any")[0], 0)

    def test_coordinated_chain_edit_cannot_bypass_raw_log_membership(self):
        with tempfile.TemporaryDirectory() as d:
            record, chain = make_record(Path(d))
            # Change both structured chain and its own manifest SHA-256, while
            # keeping the original pinned responder source untouched.
            original = chain["stages"]["2_receipt"]["log_line"]
            chain["stages"]["2_receipt"]["log_line"] = "FAKE-PREFIX " + original
            (record / "raw" / "chain.json").write_text(json.dumps(chain))
            repack(record)
            self.assertEqual(checker.check_chain(chain), [],
                             "regex-only structural validation still passes")
            problems = checker.check_pinned_sources(record, chain)
            self.assertTrue(any("2_receipt" in p and "absent" in p for p in problems), problems)

    def test_tampered_pinned_file_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            record, chain = make_record(Path(d))
            with (record / "raw" / "responder.log").open("a") as stream:
                stream.write("attacker-added-line\n")
            problems = checker.check_pinned_sources(record, chain)
            self.assertTrue(any("responder_log" in p and "mismatch" in p for p in problems),
                            problems)

    def test_swapped_role_or_reused_log_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            record, chain = make_record(Path(d))
            chain["sources"]["joiner_log"], chain["sources"]["responder_log"] = (
                chain["sources"]["responder_log"], chain["sources"]["joiner_log"],
            )
            problems = checker.check_pinned_sources(record, chain)
            self.assertTrue(any("quoted line absent" in p for p in problems), problems)
            chain["sources"]["responder_log"] = chain["sources"]["joiner_log"]
            self.assertTrue(any("distinct" in p for p in
                                checker.check_pinned_sources(record, chain)))

    def test_missing_or_escape_source_paths_fail_closed(self):
        with tempfile.TemporaryDirectory() as d:
            record, chain = make_record(Path(d))
            del chain["sources"]
            self.assertTrue(checker.check_pinned_sources(record, chain))
            for name in ("../outside.log", "/tmp/outside.log", "raw/../chain.json",
                         "raw/not-listed.log"):
                with self.subTest(path=name):
                    candidate = copy.deepcopy(chain)
                    candidate["sources"] = {
                        "joiner_log": name,
                        "responder_log": "raw/responder.log",
                    }
                    self.assertTrue(checker.check_pinned_sources(record, candidate))

    def test_chain_file_itself_must_be_pinned(self):
        with tempfile.TemporaryDirectory() as d:
            record, chain = make_record(Path(d))
            (record / "raw" / "chain.json").write_text("{}\n")
            problems = checker.check_pinned_sources(record, chain)
            self.assertTrue(any("chain.json" in p and "mismatch" in p for p in problems),
                            problems)

    def test_off_host_policy_refuses_legacy_chain_without_raw_sources(self):
        with tempfile.TemporaryDirectory() as d:
            record, chain = make_record(Path(d))
            del chain["sources"]
            (record / "raw" / "chain.json").write_text(json.dumps(chain))
            repack(record)
            code, problems = checker.check_record(record, "off-host")
            self.assertEqual(code, 1)
            self.assertTrue(any("sources" in p for p in problems), problems)
            # Broad policy must not provide a route around source validation.
            self.assertEqual(checker.check_record(record, "any")[0], 1)

    def test_build_declares_source_roles_when_provided(self):
        chain = fixtures.m3_chain()
        stages = chain["stages"]
        joiner = "\n".join([
            stages["1_request"]["log_line"], stages["5_receive"]["log_line"],
            stages["6_apply"]["walk_log_line"], stages["6_apply"]["log_line"],
        ])
        responder = "\n".join([
            stages["2_receipt"]["log_line"], stages["3_construction"]["log_line"],
            stages["4_transport"]["log_line"],
        ])
        result = checker.build_chain(
            joiner_log=joiner, responder_log=responder, locality="off-host-claim",
            joiner_source_path="raw/joiner.log",
            responder_source_path="raw/responder.log",
        )
        self.assertEqual(result["sources"], {
            "joiner_log": "raw/joiner.log",
            "responder_log": "raw/responder.log",
        })
        self.assertEqual(checker.check_chain(result), [])


if __name__ == "__main__":
    unittest.main()
