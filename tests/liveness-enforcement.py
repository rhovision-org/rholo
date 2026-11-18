#!/usr/bin/env python3
"""The named liveness witness: the in-process jail rule, not a live net.

S6-0963's pass condition names `tests/liveness-enforcement.py`. This file is that
witness. It runs the current build's in-process liveness test and requires it to
pass. The devnet/partition arms of `tools/liveness-enforcement-drill.py` and the
live `G428-M4` campaign are release obligations; they are not claimed here.

Usage:
  python3 tests/liveness-enforcement.py self-test
"""
from __future__ import annotations

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
INNER = "a_sustained_absence_jails_without_touching_the_bond"


def self_test() -> int:
    completed = subprocess.run(
        ["cargo", "test", "--locked", "-p", "rchain-rholang", "--lib", INNER],
        cwd=ROOT, text=True, capture_output=True)
    print(completed.stdout, end="")
    print(completed.stderr, end="", file=sys.stderr)
    ok = (completed.returncode == 0
          and "1 passed" in completed.stdout
          and INNER in completed.stdout)
    print(f"test self_test ... {'ok' if ok else 'FAILED'}")
    return 0 if ok else 1


def main(argv: list[str] | None = None) -> int:
    argv = sys.argv[1:] if argv is None else argv
    if argv and argv[0] in {"self-test", "self_test"}:
        return self_test()
    print("usage: tests/liveness-enforcement.py self-test", file=sys.stderr)
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
