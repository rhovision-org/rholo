#!/usr/bin/env python3
"""One witness resolver for the builder and the checker.

`build-mainnet-gates.py` emits a `met` row from an authored binding; `check-mainnet-gates.py`
judges the row. If only the judge knows that a Rust witness must be a `#[test]`, the builder can
emit a row the judge would refuse, and the hand-run `--check` is the only thing between them. This
module is the single answer to "does this path::symbol name a real witness of the declared kind?".
"""
from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def test_attribute(text: str, symbol: str) -> bool:
    lines = text.splitlines()
    for index, line in enumerate(lines):
        if re.search(rf"\bfn\s+{re.escape(symbol)}\b", line):
            window = "\n".join(lines[max(0, index - 3):index + 1])
            return "#[test]" in window or "#[tokio::test]" in window
    return False


def problem(kind: str, path_text: str, symbol: str) -> str | None:
    """The reason this path::symbol is not a witness of `kind`, or None when it is."""
    path = ROOT / path_text
    if not path.is_file():
        return f"witness path {path_text} does not exist"
    text = path.read_text(errors="ignore")
    if symbol.startswith("--"):
        return None if symbol in text else f"witness flag {symbol} not found in {path_text}"
    if kind.startswith("rust-"):
        if path.suffix != ".rs":
            return "a rust witness must be a .rs file"
        if not test_attribute(text, symbol):
            return f"witness {path_text}::{symbol} is not a #[test]"
    elif kind == "python":
        if path.suffix not in {".py", ".sh"}:
            return "a python witness must be .py or .sh"
        if path.suffix == ".py":
            if not re.search(rf"\bdef\s+{re.escape(symbol)}\b", text):
                return f"witness {symbol} is not defined in {path_text}"
        elif symbol not in text:
            return f"witness {symbol} not found in {path_text}"
    elif kind == "lean":
        if not (path.suffix == ".lean"
                and re.search(rf"\btheorem\s+{re.escape(symbol)}\b", text)):
            return "a lean witness must be a theorem"
    elif kind == "human-attestation":
        if path.suffix != ".md":
            return "a human attestation must be a markdown record"
    elif kind == "decision":
        return "a decision gate has no executable witness"
    else:
        # local-devnet / two-binary / live-host: the content check is the file's language, not
        # the witness_kind label. A comment or a docstring is not a witness.
        if path.suffix == ".py":
            if not re.search(rf"\bdef\s+{re.escape(symbol)}\b", text):
                return f"witness {symbol} is not defined in {path_text}"
        elif symbol not in text:
            return f"witness {symbol} not found in {path_text}"
    return None
