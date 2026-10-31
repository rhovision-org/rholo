#!/usr/bin/env python3
"""The cost register's gate: every priced operation is declared, and its declaration matches the code.

**Why this exists.** The "flat charge for superlinear work" defect has been found twice (September
2026's `Set.union`, October 2026's conversions, union asymmetry and `censure`) and both times by
*measurement*: no gate in this tree could see a charge that does not track its work
(`docs/src/node/security-audit.md` §5). The censuses that do exist — `check-spawn-observability.sh`,
the panic whitelist in `audit-type-system.sh` — are the pattern: enumerate the sites, require each to
be *declared*, and refuse an undeclared one and a stale declaration. This is the same check one layer
over: the reducer's arms and the builtins are the sites, `spec/cost-register.tsv` is the declaration.

**What it checks.**
  1. Coverage — every arm of `reduce.rs::eval_method` and every `Expr::E*` arm of
     `reduce.rs::eval_arm` appears in some row's `operations`.
  2. Staleness — every row's operation still exists (an arm, or a symbol in its file). A row for
     nothing is refused; that is how a register stops meaning anything.
  3. Consistency — `identity` and `capped` rows name no `Costs::` constructor; every other row names
     one, and the constructor (or a `helper:<fn>`) occurs in the code region the row covers.
  4. Evidence — every `path::symbol` in the `evidence` column resolves: the path is in the tree and
     the symbol occurs in that file (or the path alone exists, for a page).
  5. Vocabulary — the shape is one of `constant|linear|product|quadratic|cubic|capped|identity`, and
     every row has all ten fields.

**What it cannot see, said rather than implied.** It reads the *declaration* and the charge
constructor, not the Rust arm's complexity. A row declaring `linear` over work that is `O(n log n)`
passes — that is the recorded log-factor residual, not something static reading can catch. A new arm
that charges a constant passes if it *declares* `constant`; the register makes that a reviewable claim
where before there was silence, and the shape test (`rholang/tests/cost_shapes.rs`) is what makes a
charge that stops growing fail.

Usage:  python3 tools/check-cost-register.py [--register PATH] [--reduce PATH]
Exit:   0 every check holds · 1 a failure (named) · 2 usage
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SHAPES = ("constant", "linear", "product", "quadratic", "cubic", "capped", "identity")
COLUMNS = ("id", "file", "operations", "work", "shape", "basis", "charge", "evidence", "residual", "note")
# An arm is either an or-pattern of method-name strings (`"bitAnd" | "bitOr" => `) or one `Expr::`
# variant. Anchored at exactly the dispatch's indentation, so a string literal or a nested match
# inside an arm body is not mistaken for an arm.
METHOD_ARM = re.compile(r'^        ((?:"[A-Za-z][A-Za-z0-9]*"\s*(?:\|\s*)?)+)\s*=>')
OPERATOR_ARM = re.compile(r"^        Expr::(E[A-Za-z0-9]+)(?:\([^)]*\))?\s*=>")
CTOR = re.compile(r"Costs::([a-z_0-9]+)")


def read_rows(path: Path) -> list[dict[str, str]]:
    rows = []
    for number, raw in enumerate(path.read_text().splitlines(), 1):
        if not raw.strip() or raw.lstrip().startswith("#"):
            continue
        cells = raw.split("\t")
        if cells[0] == "id":
            continue
        if len(cells) != len(COLUMNS):
            raise ValueError(f"line {number}: {len(cells)} columns, expected {len(COLUMNS)}")
        rows.append(dict(zip(COLUMNS, cells)))
    return rows


def function_span(lines: list[str], name: str) -> tuple[int, int]:
    start = next((i for i, line in enumerate(lines) if line.startswith(name)), None)
    if start is None:
        raise ValueError(f"{name} not found — the extractor cannot see the dispatch")
    end = next(
        (i for i in range(start + 1, len(lines))
         if lines[i].startswith("fn ") or lines[i].startswith("pub fn ")),
        len(lines),
    )
    return start, end


def arms_with_regions(lines: list[str], name: str, expr_only: bool) -> dict[str, str]:
    """Operation name → its source region (from its line to the next arm at the same indent)."""
    start, end = function_span(lines, name)
    arm_lines = [
        i for i in range(start, end)
        if (OPERATOR_ARM if expr_only else METHOD_ARM).match(lines[i])
    ]
    out: dict[str, str] = {}
    for n, i in enumerate(arm_lines):
        following = arm_lines[n + 1] if n + 1 < len(arm_lines) else end
        region = "\n".join(lines[i:following])
        if expr_only:
            out[f"Expr::{OPERATOR_ARM.match(lines[i]).group(1)}"] = region
        else:
            for label in re.findall(r'"([A-Za-z][A-Za-z0-9]*)"', lines[i]):
                out[label] = region
    return out


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--register", type=Path, default=ROOT / "spec/cost-register.tsv")
    parser.add_argument("--reduce", type=Path, default=ROOT / "rholang/src/reduce.rs")
    args = parser.parse_args()

    try:
        rows = read_rows(args.register)
    except (OSError, ValueError) as exc:
        print(f"FAIL  register: {exc}")
        return 1
    if not rows:
        print("FAIL  the register is empty — a check over nothing is not a check")
        return 1

    lines = args.reduce.read_text().splitlines()
    try:
        methods = arms_with_regions(lines, "fn eval_method(", expr_only=False)
        operators = arms_with_regions(lines, "fn eval_arm(", expr_only=True)
    except ValueError as exc:
        print(f"FAIL  extractor: {exc}")
        return 1
    if not methods or not operators:
        print(f"FAIL  extractor found {len(methods)} method arms and {len(operators)} operator arms — "
              "an empty extraction would make this check vacuous")
        return 1

    declared: dict[str, dict[str, str]] = {}
    failures: list[str] = []

    for row in rows:
        ident = row["id"]
        where = f"{ident}"
        for column in COLUMNS:
            if not row[column].strip():
                failures.append(f"{where}.{column}: empty")
        if row["shape"] not in SHAPES:
            failures.append(f"{where}.shape: {row['shape']!r} is not one of {', '.join(SHAPES)}")

        file_path = ROOT / row["file"]
        if not file_path.is_file():
            failures.append(f"{where}.file: {row['file']} does not exist")
            continue
        source = file_path.read_text()

        charges = [
            c.strip() for c in row["charge"].split(",")
            if c.strip() and c.strip() != "-"
        ]
        ctors = [c.removeprefix("Costs::") for c in charges if not c.startswith("helper:")]
        helpers = [c.removeprefix("helper:") for c in charges if c.startswith("helper:")]
        if row["shape"] in ("identity", "capped") and ctors:
            failures.append(f"{where}: shape {row['shape']} must not name a charge ({row['charge']})")
        if row["shape"] not in ("identity", "capped") and not ctors and not helpers:
            failures.append(f"{where}: shape {row['shape']} names no charge")
        for ctor in ctors:
            if f"Costs::{ctor}" not in source:
                failures.append(f"{where}.charge: Costs::{ctor} does not occur in {row['file']}")
        for helper in helpers:
            if not re.search(rf"^(?:pub )?fn {re.escape(helper)}\b", source, re.MULTILINE):
                failures.append(f"{where}.charge: helper {helper} is not a fn in {row['file']}")

        # Evidence resolves: `path::symbol`, or a path that exists (a page).
        for token in (t.strip() for t in row["evidence"].split(",")):
            if not token:
                continue
            path_text, _, symbol = token.partition("::")
            target = ROOT / path_text
            if not target.is_file():
                failures.append(f"{where}.evidence: {path_text} does not exist")
            elif symbol and symbol not in target.read_text():
                failures.append(f"{where}.evidence: {symbol} does not occur in {path_text}")

        for operation in (o.strip() for o in row["operations"].split(",")):
            if not operation:
                continue
            if operation in declared:
                failures.append(f"{where}: operation {operation} is already declared by {declared[operation]['id']}")
                continue
            declared[operation] = row

    # Coverage: every arm has a row, and an arm-level row's charge occurs in the arm's region.
    for label, region in {**methods, **operators}.items():
        row = declared.get(label)
        if row is None:
            failures.append(f"undeclared arm: {label} has no row in the register")
            continue
        if row["file"] != "rholang/src/reduce.rs":
            failures.append(f"{row['id']}: arm {label} is declared against {row['file']}")
            continue
        charges = [
            c.strip() for c in row["charge"].split(",")
            if c.strip() and c.strip() != "-"
        ]
        names = [c.removeprefix("Costs::") if not c.startswith("helper:") else c.removeprefix("helper:")
                 for c in charges]
        if names and not any(name in region for name in names):
            failures.append(f"{row['id']}: {label}'s arm names none of its declared charges "
                            f"({', '.join(names)})")

    # Staleness: every arm-level row names a live arm; every builtin row names a symbol in its file.
    for operation, row in declared.items():
        if operation.startswith("Expr::"):
            if operation not in operators:
                failures.append(f"{row['id']}: {operation} is not an operator arm any more")
        elif row["file"] == "rholang/src/reduce.rs":
            if operation not in methods:
                failures.append(f"{row['id']}: {operation} is not a method arm any more")
        else:
            source = (ROOT / row["file"]).read_text()
            symbol = operation.split("::")[-1]
            if operation not in source and not re.search(rf"\bfn {re.escape(symbol)}\b", source):
                failures.append(f"{row['id']}: {operation} does not occur in {row['file']}")

    if failures:
        for failure in failures:
            print(f"FAIL  {failure}")
        print(f"\nFAIL  {len(failures)} problem(s) in {len(rows)} rows "
              f"({len(methods)} method arms, {len(operators)} operator arms)")
        return 1

    print(f"ok    {len(rows)} rows cover {len(methods)} method arms and {len(operators)} operator arms; "
          f"every declared charge occurs where it claims")
    return 0


if __name__ == "__main__":
    sys.exit(main())
