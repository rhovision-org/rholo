#!/usr/bin/env python3
"""The behavioural self-test every criterion must pass.

`spec/audit/gate-criteria.json` declares a base shape and the negative controls that must fail.
This module builds one satisfying observation per registered criterion, applies every declared
control, removes each required observation, and proves the principle reads every declared input.
It returns a list of error strings (empty is PASS); it never raises for a bad criterion, so the
caller can report all of them.

The register check (`check-mainnet-gates.py::check_criteria`) runs this, which is how the criteria
become CI-checked; `tests/test_gate_principles.py` calls the same function.
"""
from __future__ import annotations

import contextlib
import importlib.util
import io
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _principles():
    return _load("gate_selftest_principles", ROOT / "tools" / "gate-principles.py")


def _fixtures():
    return _load("gate_selftest_fixtures", ROOT / "tests" / "gate_fixtures.py")


def _cleanup_own(fixtures, before: int) -> None:
    """Remove only the temporary directories this run created.

    The fixture module keeps a process-global stack; a caller (the meta-test) may still own
    earlier directories, so a blanket `cleanup()` would remove them.
    """
    stack = getattr(fixtures, "_TEMPDIRS", None)
    if stack is None:
        return
    while len(stack) > before:
        stack.pop().cleanup()


def _check(entry: dict, gate_id: str, principles, fixtures) -> list[str]:
    errors: list[str] = []
    base = fixtures.base(gate_id)
    result = principles.evaluate_entry(entry, base)
    if result.get("status") != "PASS":
        errors.append(f"{gate_id}: the base observation is {result.get('status')}: "
                      f"{result.get('reasons')}")
    fail_controls = 0
    for control in entry.get("negative_controls") or []:
        name = control.get("name")
        mutated = fixtures.controlled(gate_id, control)
        result = principles.evaluate_entry(entry, mutated)
        expected = control.get("expected")
        if result.get("status") != expected:
            errors.append(f"{gate_id}/{name}: expected {expected}, got "
                          f"{result.get('status')}: {result.get('reasons')}")
        if not result.get("reasons"):
            errors.append(f"{gate_id}/{name}: a verdict without reasons")
        if expected == "FAIL":
            fail_controls += 1
    if fail_controls == 0:
        errors.append(f"{gate_id}: no control expects FAIL (the criterion is vacuous)")
    for path in entry.get("requires") or []:
        removed = fixtures.apply_mutation(base, ["delete", path])
        result = principles.evaluate_entry(entry, removed)
        if result.get("status") != "PRECONDITION":
            errors.append(f"{gate_id}/{path}: deleting a required input is "
                          f"{result.get('status')}, not PRECONDITION")
        # `evaluate_entry` short-circuits on a missing required path, so call the principle
        # directly: a declared input the principle ignores is a false claim of dependence.
        direct = principles.evaluate(
            entry.get("principle"), removed, entry.get("parameters") or {})
        if direct.get("status") == "PASS":
            errors.append(f"{gate_id}/{path}: the principle passes without a declared input")
    return errors


def run(entries: dict, principles=None) -> list[str]:
    """Every failure for every entry; an empty list is PASS."""
    principles = principles or _principles()
    fixtures = _fixtures()
    stack = getattr(fixtures, "_TEMPDIRS", [])
    before = len(stack)
    errors: list[str] = []
    try:
        # The fixtures pack records in-process and the packer reports on stdout. This runs inside
        # the register check, whose only stdout is its JSON report; keep the noise out of it.
        with contextlib.redirect_stdout(io.StringIO()):
            for gate_id, entry in entries.items():
                try:
                    errors.extend(_check(entry, gate_id, principles, fixtures))
                except Exception as error:  # a self-test that raises is a failed self-test
                    errors.append(
                        f"{gate_id}: the self-test raised {type(error).__name__}: {error}")
    finally:
        _cleanup_own(fixtures, before)
    return errors


def main() -> int:
    principles = _principles()
    entries = principles.criteria_by_gate()
    errors = run(entries, principles=principles)
    for error in errors:
        print(error)
    return 0 if not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
