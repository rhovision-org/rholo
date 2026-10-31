#!/usr/bin/env python3
"""Exercise the real range helpers and verification phases with disposable API fixtures."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

DEFAULT_SCRIPT = Path(__file__).resolve().parents[1] / "tools/reconcile-network.sh"
SCRIPT = DEFAULT_SCRIPT


def block(digest, height=1):
    return {"blockHash": digest, "blockNumber": height, "sender": "sender",
            "postStateHash": "state", "deployCount": 0, "rejectedDeploys": []}


class RangeSnapshotTests(unittest.TestCase):
    def run_case(self, cache_b="aa", live_b="aa", phase="verification", mode="valid", no_cache=""):
        src = SCRIPT.read_text()
        helpers = src[src.index("blocks_at() {"):src.index("# The last-finalised endpoint")]
        if phase == "verification":
            code = src[src.index('echo "== 9. verification:'):src.index('echo "== 10. finality')]
        elif phase == "complete":
            code = src[src.index('echo "== 9. verification:'):]
        elif phase == "finality":
            code = src[src.index('echo "== 10. finality'):]
        else:
            start = src.index("# **One range request per node")
            code = src[start:src.index("# --- 2. reported equivocation", start)]
            code += '\nblocks_at A 1 1\nblocks_at B 1 1\n'
        with tempfile.TemporaryDirectory(prefix="reconcile-cache-test-") as directory:
            root = Path(directory)
            for name, digest in (("A", "aa"), ("B", cache_b)):
                cached = [block("ff", 2), block(digest, 3)] if phase == "complete" else [block(digest)]
                (root / f"blocks_{name}_1.json").write_text(json.dumps(cached))
            # The API is the only external boundary replaced. No SSH, Docker, node control,
            # faucet, or validator process can be invoked by these source fragments.
            setup = '''set -uo pipefail
NAMES=(A B); declare -A HOST PORT
HOST[A]=A; HOST[B]=B; PORT[A]=1; PORT[B]=1
MEET=1; MAXH=1
sleep() { :; }; seq() { echo 1; }
height_of() { echo 3; }; lfb_num() { echo 2; }; lfb_hash() { echo ff; }
trigger_block() { :; }
api() {
  printf '%s %s\n' "$1" "$3" >> "$API_LOG"
  case "$3" in
    /api/blocks/0/*)
      case "$RANGE_MODE" in
        object) printf '%s' '{"error":"range unavailable"}'; return 0;;
        truncated) printf '%s' '[{"blockNumber":1'; return 28;;
        partial) printf '%s' "$OLD_RANGE"; return 28;;
        invalid-element) printf '%s' '["range unavailable"]'; return 0;;
        empty) printf '%s' '[]'; return 0;;
      esac;;
  esac
  if [ "$1" = A ]; then printf '%s' "$LIVE_A"; else printf '%s' "$LIVE_B"; fi
}
'''
            rows_a, rows_b = [block("aa"), block("ff", 2)], [block(live_b), block("ff", 2)]
            if phase == "complete":
                setup += "\nMAXH=3\n"
                rows_a, rows_b = [block("ff", 2), block("aa", 3)], [block("ff", 2), block(live_b, 3)]
            env = dict(os.environ, CACHE_DIR=directory, LIVE_A=json.dumps(rows_a),
                       LIVE_B=json.dumps(rows_b), API_LOG=str(root / "requests"),
                       RANGE_MODE=mode, OLD_RANGE=json.dumps([block("stale")]),
                       RECONCILE_NO_CACHE=no_cache)
            result = subprocess.run(["bash", "-c", setup + helpers + code], env=env,
                                    capture_output=True, text=True, timeout=10)
            requests = (root / "requests").read_text().splitlines() if (root / "requests").exists() else []
            return result, requests

    def test_recovered_node_is_not_rejected_using_its_old_inventory(self):
        result, calls = self.run_case(cache_b="bb", live_b="aa")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("hashes-agree=1", result.stdout)
        self.assertEqual(calls, ["A /api/blocks/0/1", "B /api/blocks/0/1"])

    def test_new_divergence_cannot_pass_using_an_old_agreement(self):
        result, _ = self.run_case(cache_b="aa", live_b="bb")
        self.assertEqual(result.returncode, 6, result.stdout + result.stderr)
        self.assertIn("hashes-agree=0", result.stdout)

    def test_finality_above_the_plan_range_is_checked_in_current_inventory(self):
        result, calls = self.run_case(phase="finality")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("past-the-point=1", result.stdout)
        self.assertEqual(calls, ["A /api/blocks/0/3", "B /api/blocks/0/3"])

    def test_recovery_cannot_exit_success_with_current_unfinalized_divergence(self):
        result, _ = self.run_case(cache_b="aa", live_b="bb", phase="complete")
        self.assertEqual(result.returncode, 6, result.stdout + result.stderr)
        self.assertNotIn("finality advanced past", result.stdout)

    def test_nonempty_error_object_is_not_cached_as_an_empty_inventory(self):
        result, calls = self.run_case(phase="plan", mode="object")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout.count("aa sender state 0 []"), 2)
        self.assertIn("A /api/blocks/1/1", calls)
        self.assertIn("using live per-height requests", result.stderr)

    def test_failed_transfer_cannot_publish_a_partial_snapshot(self):
        result, calls = self.run_case(phase="plan", mode="partial")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout.count("aa sender state 0 []"), 2)
        self.assertNotIn("stale sender", result.stdout)
        self.assertIn("B /api/blocks/1/1", calls)

    def test_truncated_json_uses_live_fallback(self):
        result, _ = self.run_case(phase="plan", mode="truncated")
        self.assertEqual(result.stdout.count("aa sender state 0 []"), 2)

    def test_ranges_wider_than_the_api_window_are_chunked(self):
        """The API answers "exceed the max limit 50" for a wider range, so the snapshot walks windows.

        A single request for 0..MAXH therefore fails on every chain taller than 50 heights: the tool
        prints the fallback NOTE and the audit degenerates into one request per height per node, which
        is the cost this cache exists to remove. The fixture is 0..120 so the windows have to be real.
        """
        src = SCRIPT.read_text()
        # `helpers` already spans blocks_at and refresh_block_ranges (both sit above the
        # last-finalised marker), so this case calls the real function directly.
        helpers = src[src.index("blocks_at() {"):src.index("# The last-finalised endpoint")]
        code = 'refresh_block_ranges 120\nblocks_at A 1 0\nblocks_at A 1 50\n'
        with tempfile.TemporaryDirectory(prefix="reconcile-window-test-") as directory:
            root = Path(directory)
            setup = '''set -uo pipefail
NAMES=(A); declare -A HOST PORT
HOST[A]=A; PORT[A]=1
MAXH=120
api() {
  printf '%s %s\\n' "$1" "$3" >> "$API_LOG"
  lo=${3#/api/blocks/}; lo=${lo%%/*}
  if [ "${RANGE_MODE:-}" = "empty-window" ] && [ "$lo" = "50" ]; then
    printf '[]'
    return 0
  fi
  height="$lo"
  if [ "${RANGE_MODE:-}" = "wrong-height" ] && [ "$lo" = "50" ]; then height=0; fi
  printf '[{"blockHash":"h%s","blockNumber":%s,"sender":"s","postStateHash":"p","deployCount":0,"rejectedDeploys":[]}]' "$lo" "$height"
  return 0
}
'''
            env = dict(os.environ, CACHE_DIR=directory, API_LOG=str(root / "requests"))
            result = subprocess.run(["bash", "-c", setup + helpers + code], env=env,
                                    capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertNotIn("using live per-height requests", result.stderr,
                             "a chain taller than the API window fell back to per-height requests")
            windows = []
            for line in (root / "requests").read_text().splitlines():
                _host, path = line.split()
                lo, hi = (int(x) for x in path.rsplit("/", 2)[-2:])
                windows.append((lo, hi))
                self.assertLessEqual(hi - lo + 1, 50, f"window {path} exceeds the API limit")
            covered = [h for lo, hi in windows for h in range(lo, hi + 1)]
            self.assertEqual(sorted(covered), list(range(0, 121)), "windows do not tile 0..MAXH")
            # and blocks_at must answer from the merged snapshot, not from a per-height request
            self.assertIn("h0 s p 0 []", result.stdout)
            self.assertIn("h50 s p 0 []", result.stdout)

            # Bad operator override must fail immediately, not hang the loop or
            # make a >50 request that falls back for the whole chain.
            for value in ("0", "-1", "51", "not-a-number"):
                bad_env = dict(env, RECONCILE_RANGE_CHUNK=value)
                bad = subprocess.run(["bash", "-c", setup + helpers + code],
                                     env=bad_env, capture_output=True, text=True, timeout=10)
                self.assertEqual(bad.returncode, 2, (value, bad.stdout, bad.stderr))
                self.assertIn("RECONCILE_RANGE_CHUNK", bad.stderr)

            # A successful HTTP response is insufficient if one window is
            # empty or describes a block from outside the requested heights.
            for mode in ("empty-window", "wrong-height"):
                bad_env = dict(env, RANGE_MODE=mode)
                bad = subprocess.run(["bash", "-c", setup + helpers + code],
                                     env=bad_env, capture_output=True, text=True, timeout=10)
                self.assertEqual(bad.returncode, 0, bad.stdout + bad.stderr)
                self.assertIn("using live per-height requests", bad.stderr,
                              "a partial/out-of-window cache was silently accepted")
                self.assertFalse((root / "blocks_A_1.json").exists())

    def test_invalid_list_elements_use_live_fallback(self):
        result, _ = self.run_case(phase="plan", mode="invalid-element")
        self.assertEqual(result.stdout.count("aa sender state 0 []"), 2)

    def test_empty_range_does_not_hide_live_blocks(self):
        result, calls = self.run_case(phase="plan", mode="empty")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout.count("aa sender state 0 []"), 2)
        self.assertIn("A /api/blocks/1/1", calls)
        self.assertIn("B /api/blocks/1/1", calls)
        self.assertIn("using live per-height requests", result.stderr)

    def test_valid_range_is_fetched_once_per_node_in_plan(self):
        result, calls = self.run_case(phase="plan")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout.count("aa sender state 0 []"), 2)
        self.assertEqual(calls, ["A /api/blocks/0/1", "B /api/blocks/0/1"])

    def test_no_cache_control_reads_current_blocks(self):
        result, calls = self.run_case(cache_b="bb", phase="plan", no_cache="1")
        self.assertEqual(result.stdout.count("aa sender state 0 []"), 2)
        self.assertEqual(calls, ["A /api/blocks/1/1", "B /api/blocks/1/1"])


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--script", type=Path, default=DEFAULT_SCRIPT)
    args, remaining = parser.parse_known_args()
    SCRIPT = args.script.resolve()
    unittest.main(argv=[__file__, *remaining], verbosity=2)
