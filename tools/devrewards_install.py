#!/usr/bin/env python3
"""Render the DevRewards install term from `examples/devrewards/`.

The install is one deploy: `install.rho` carries the `new` bindings and the wiring, and the three
`// {{NAME}}` markers are replaced by `BeneficiaryRegistry.rho`, `Treasury.rho` and `Settlement.rho`.
`$$ATTESTORS$$` becomes a Rholang list of `"<hex>".hexToBytes()` keys and `$$THRESHOLD$$` an integer.

The same renderer is used by `tools/devrewards-experiment.py` and by the Rust tests (which carry a
small equivalent; the marker strings are the contract).

Usage:
  tools/devrewards_install.py --attestor <hex65> [--attestor <hex65> ...] --threshold N
  tools/devrewards_install.py --self-test
"""

from __future__ import annotations

import argparse
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SRC = ROOT / "examples" / "devrewards"

FRAGMENTS = (
    ("// {{BENEFICIARY_REGISTRY}}", "BeneficiaryRegistry.rho"),
    ("// {{TREASURY}}", "Treasury.rho"),
    ("// {{SETTLEMENT}}", "Settlement.rho"),
)


def _key_list(attestors: list[str]) -> str:
    for key in attestors:
        if len(key) != 130 or any(c not in "0123456789abcdefABCDEF" for c in key):
            raise ValueError(f"an attestor key is a 65-byte hex string: {key!r}")
    body = ",\n".join(f'  "{key}".hexToBytes()' for key in attestors)
    return "[\n" + body + "\n]"


def render(attestors: list[str], threshold: int) -> str:
    """The install term for this attestor set and threshold."""
    if threshold < 1 or threshold > len(attestors):
        raise ValueError(
            f"threshold {threshold} must be between 1 and the {len(attestors)} attestor key(s)"
        )
    term = (SRC / "install.rho").read_text()
    for marker, name in FRAGMENTS:
        if marker not in term:
            raise ValueError(f"install.rho is missing {marker}")
        term = term.replace(marker, (SRC / name).read_text().rstrip() + "\n|", 1)
    term = term.replace("$$ATTESTORS$$", _key_list(attestors))
    term = term.replace("$$THRESHOLD$$", str(threshold))
    if "$$" in term or "{{" in term:
        raise ValueError("an install placeholder was left unrendered")
    return term


def self_test() -> None:
    a = "11" * 65
    b = "22" * 65
    term = render([a, b], 2)
    assert f'"{a}".hexToBytes()' in term
    assert "$$ATTESTORS$$" not in term and "$$THRESHOLD$$" not in term
    assert "{{BENEFICIARY_REGISTRY}}" not in term
    assert term.count("contract registry(") == 1
    assert term.count("contract treasury(") == 3
    assert term.count("contract settlement(") == 3
    assert "devrewards/treasury-address" in term
    try:
        render([a, b], 3)
    except ValueError:
        pass
    else:
        raise AssertionError("a threshold above the key count must be refused")
    print("devrewards_install self-test: ok")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--attestor", action="append", default=[], help="65-byte hex public key")
    parser.add_argument("--threshold", type=int, default=0)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)
    if args.self_test:
        self_test()
        return 0
    try:
        sys.stdout.write(render(args.attestor, args.threshold))
    except ValueError as e:
        print(f"devrewards_install: {e}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
