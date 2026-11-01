import importlib.util
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("root_capture", ROOT / "tools/n420-root-capture.py")
capture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(capture)
A, B = "aa" * 32, "bb" * 32


class RootCaptureTests(unittest.TestCase):
    def verdict(self, output, code=0):
        return subprocess.CompletedProcess([], code, output, b"diagnostic on stderr")

    def test_success_requires_a_verified_verdict_for_the_requested_hash(self):
        cases = [
            (json.dumps({"verified": True, "blockHash": A}).encode(), 0, True),
            (json.dumps({"verified": True, "blockHash": B}).encode(), 0, False),
            (json.dumps({"verified": False, "blockHash": A}).encode(), 0, False),
            (b"{}", 0, False), (b"[]", 0, False), (b"not JSON", 0, False),
            (json.dumps({"verified": True, "blockHash": A}).encode(), 1, False),
        ]
        for output, code, expected in cases:
            with self.subTest(output=output, code=code), patch.object(
                capture.subprocess, "run", return_value=self.verdict(output, code)
            ):
                self.assertEqual(capture.verify("rnode", b"raw", A)[0], expected)

    def run_capture(self, *, verified=A, summary=A, finalized_error=False):
        with tempfile.TemporaryDirectory() as directory:
            argv = ["capture", "--out", directory, "--endpoint", "http://one",
                    "--endpoint", "http://two", "--block", A]
            def get(endpoint, path, timeout):
                if path.endswith("/raw"):
                    return b"raw response", None
                if path == "/api/last-finalized-block":
                    return (None, "unreachable") if finalized_error else (b"{}", None)
                return json.dumps({"blockInfo": {"blockHash": summary}}).encode(), None
            verdict = json.dumps({"verified": True, "blockHash": verified}).encode()
            with patch("sys.argv", argv), patch.object(capture, "get", side_effect=get), \
                 patch.object(capture.subprocess, "run", return_value=self.verdict(verdict)):
                status = capture.main()
            return status, json.loads((Path(directory) / "capture.json").read_text())

    def test_wrong_raw_or_summary_identity_fails_the_capture(self):
        for kwargs in ({"verified": B}, {"summary": B}):
            status, record = self.run_capture(**kwargs)
            self.assertEqual(status, 1)
            self.assertFalse(record["complete"])

    def test_hash_bound_capture_does_not_claim_the_missing_m5_state_witness(self):
        status, record = self.run_capture()
        self.assertEqual(status, 0)
        self.assertTrue(record["complete"])
        self.assertFalse(record["m5_witness_complete"])

    def test_all_endpoint_failures_are_retained(self):
        status, record = self.run_capture(finalized_error=True)
        self.assertEqual(status, 1)
        self.assertEqual(set(record["finalized_error"]), {"http://one", "http://two"})

    def test_prefixes_and_path_components_are_rejected_before_capture(self):
        for value in ("abc", "../file", "/" + A, "x" * 64):
            with self.assertRaises(capture.argparse.ArgumentTypeError):
                capture.block_hash(value)
        self.assertEqual(capture.block_hash(A.upper()), A)


if __name__ == "__main__":
    unittest.main()
