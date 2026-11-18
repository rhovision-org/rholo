"""The ingress queue gate refuses an absent, unbounded, unpinned or unknown binding.

The gate's own code is the thing under test: a source file where one queue is wrong must make
`--gate` exit 1. The fixture is a four-binding file; each case overrides one statement.
"""

import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "tools/check-bounded-ingress-queues.sh"

BOUND = {
    "incoming_blocks_tx": "mpsc::channel(MAX_PENDING_BLOCKS)",
    "processor_input_tx": "mpsc::channel(MAX_PENDING_BLOCKS)",
    "validated_blocks_tx": "mpsc::channel(MAX_VALIDATED_BLOCKS)",
    "tap_tx": "mpsc::channel(MAX_VALIDATED_BLOCKS)",
}


def source(overrides: dict[str, str] | None = None, omit: str | None = None) -> str:
    statements = dict(BOUND)
    statements.update(overrides or {})
    lines = ["//! a fixture runtime"]
    for name, statement in statements.items():
        if name == omit:
            continue
        receiver = name.replace("_tx", "_rx")
        lines.append(f"    let ({name}, {receiver}) =")
        lines.append(f"        {statement};")
    return "\n".join(lines) + "\n"


class QueueGateTests(unittest.TestCase):
    def run_gate(self, text: str, *args: str) -> subprocess.CompletedProcess:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "runtime.rs"
            path.write_text(text)
            return subprocess.run(
                [str(SCRIPT), "--source", str(path), *args],
                cwd=ROOT, capture_output=True, text=True, check=False,
            )

    def test_the_real_runtime_passes(self):
        result = subprocess.run(
            [str(SCRIPT), "--gate"], cwd=ROOT, capture_output=True, text=True, check=False
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_named_constant_bounds_pass(self):
        result = self.run_gate(source(), "--gate")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn("absent", result.stdout)
        self.assertNotIn("unpinned", result.stdout)

    def test_an_absent_binding_is_a_violation(self):
        result = self.run_gate(source(omit="tap_tx"), "--gate")
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("absent", result.stdout)
        self.assertIn("tap_validated_blocks", result.stdout)

    def test_a_continuation_line_unbounded_channel_is_a_violation(self):
        result = self.run_gate(
            source({"processor_input_tx": "mpsc::unbounded_channel()"}), "--gate"
        )
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("unbounded", result.stdout)
        self.assertIn("processor_input", result.stdout)

    def test_mpsc_unbounded_is_a_violation(self):
        result = self.run_gate(source({"processor_input_tx": "mpsc::unbounded()"}), "--gate")
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("unbounded", result.stdout)

    def test_an_unbounded_sender_type_is_a_violation(self):
        result = self.run_gate(
            source({"incoming_blocks_tx": "let_pair: mpsc::UnboundedSender<BlockMessage>"}),
            "--gate",
        )
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("unbounded", result.stdout)

    def test_a_numeric_bound_is_unpinned(self):
        result = self.run_gate(
            source({"validated_blocks_tx": "mpsc::channel(64)"}), "--gate"
        )
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("unpinned", result.stdout)
        self.assertIn("validated_blocks", result.stdout)

    def test_usize_max_is_unpinned(self):
        result = self.run_gate(
            source({"validated_blocks_tx": "mpsc::channel(usize::MAX)"}), "--gate"
        )
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("unpinned", result.stdout)

    def test_no_constructor_is_unknown(self):
        result = self.run_gate(
            source({"incoming_blocks_tx": "channel_from_somewhere()"}), "--gate"
        )
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("unknown", result.stdout)

    def test_report_mode_does_not_exit_nonzero(self):
        result = self.run_gate(source(omit="tap_tx"))
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("absent", result.stdout)

    def test_a_custom_queue_set_is_honoured(self):
        bounded = "let (other_tx, other_rx) = mpsc::channel(MAX_OTHER);\n"
        result = self.run_gate(
            bounded, "--queues", "other|let (other_tx", "--gate"
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        unbounded = "let (other_tx, other_rx) = mpsc::unbounded();\n"
        result = self.run_gate(
            unbounded, "--queues", "other|let (other_tx", "--gate"
        )
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("unbounded", result.stdout)


if __name__ == "__main__":
    unittest.main()
