import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


envelope = load("failure_envelope", ROOT / "tools/failure_envelope.py")
classifier = load("classify_run", ROOT / "tools/classify-run.py")


class ClassificationTests(unittest.TestCase):
    def test_named_verdicts_map_to_their_class(self):
        cases = {
            "FAIL_PRECONDITION_pipelineFaultsOutstanding: node 3 outstanding=1":
                "pipeline_fault",
            "FAIL_PRECONDITION_proposalHalted: node 1": "not_ready",
            "FAIL_FORK: online validator(s) [3] do not report the anchor": "agreement",
            "FAIL_FINALIZED_DISAGREEMENT: two hashes at one height": "agreement",
            "FAIL_STALLED: no progress": "timeout",
            "INCONCLUSIVE_SLOW_PROGRESS: window closed": "timeout",
        }
        for text, expected in cases.items():
            self.assertEqual(envelope.classify(text)[0], expected, text)

    def test_signatures_map_without_a_named_verdict(self):
        self.assertEqual(
            envelope.classify("block receiver state full (1024 blocks); cannot admit x")[0],
            "receiver_capacity")
        self.assertEqual(
            envelope.classify("block summary failed: missing justification")[0],
            "deploy_unresolved")

    def test_no_evidence_is_undetermined_with_the_missing_datum(self):
        failure_class, missing = envelope.classify("something odd happened")
        self.assertEqual(failure_class, "UNDETERMINED")
        self.assertTrue(missing)

    def test_a_capture_error_never_replaces_the_primary_error(self):
        body = envelope.envelope("FAIL", "the primary", capture_error="health read failed")
        self.assertEqual(body["failure"]["first_error"], "the primary")
        self.assertEqual(body["failure"]["capture_error"], "health read failed")


class ClassifyRunTests(unittest.TestCase):
    def run_dir(self):
        return Path(tempfile.mkdtemp())

    def test_a_failure_envelope_is_authoritative(self):
        directory = self.run_dir()
        (directory / "failure.json").write_text(json.dumps({
            "schema": 1, "verdict": "FAIL",
            "failure": {"class": "pipeline_fault", "first_error": "x", "missing": []},
        }))
        report = classifier.classify_run(directory)
        self.assertEqual(report["class"], "pipeline_fault")
        self.assertEqual(report["source"], "failure.json")

    def test_no_artifacts_is_undetermined_not_a_guess(self):
        report = classifier.classify_run(self.run_dir())
        self.assertEqual(report["class"], "UNDETERMINED")
        self.assertTrue(report["missing"])

    def test_a_log_signature_classifies_without_an_envelope(self):
        directory = self.run_dir()
        (directory / "node-3.log").write_text(
            "ERROR block receiver state full (1024 blocks); cannot admit BlockHash(ab)\n")
        report = classifier.classify_run(directory)
        self.assertEqual(report["class"], "receiver_capacity")

    def test_an_orphan_without_a_witness_is_undetermined(self):
        directory = self.run_dir()
        (directory / "failure.json").write_text(json.dumps({
            "schema": 1, "verdict": "FAIL",
            "failure": {"class": "orphan_body", "first_error": "orphanBodies=2", "missing": []},
        }))
        report = classifier.classify_run(directory)
        self.assertEqual(report["class"], "UNDETERMINED")
        self.assertIn("orphan_witness", report["missing"])

    def test_an_orphan_witness_is_parsed(self):
        directory = self.run_dir()
        (directory / "failure.json").write_text(json.dumps({
            "schema": 1, "verdict": "FAIL",
            "failure": {"class": "orphan_body", "first_error": "orphanBodies=2", "missing": []},
        }))
        (directory / "node-6.log").write_text(
            "ORPHAN_WITNESS hash=" + "ab" * 32 + " disposition=faulted(stage=validation,attempts=2)\n")
        report = classifier.classify_run(directory)
        self.assertEqual(report["class"], "orphan_body")
        self.assertEqual(report["orphans"][0]["hash"], "ab" * 32)
        self.assertIn("faulted", report["orphans"][0]["disposition"])


if __name__ == "__main__":
    unittest.main()
