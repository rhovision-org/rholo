#!/usr/bin/env python3
"""check-node-diagnostics — the diagnostics standard, checked against the route and the page.

Why this exists. `docs/src/spec/node-diagnostics.md` is a claim about a document: every `/health`
field has a declared regime, scope, freshness and invariant, and the page's field table is the
standard. Prose is not a check, so this reads the page *and* the Rust contract:

  1  every field in the page's §4.1 table has a regime in G/C/A/L and a scope in S/P/N/H,
     a non-empty type, freshness and invariant;
  2  the page's field names and `PUBLIC_HEALTH_KEYS` in `node/src/diagnostics/health_contract.rs`
     are the same set (a field added to the route but not the standard, or vice versa, fails);
  3  every `OPERATOR_GATE_KEYS` name is present in the page;
  4  the page carries the required sections (§2 rules, §4 standard, §5 gap register, §7 non-claims);
  5  the scan is not vacuous (at least one field row was parsed).

The route itself is tested in Rust (`health_reports_readiness_and_pool_capacity` compares the served
JSON's key set against `PUBLIC_HEALTH_KEYS`), so this checker closes the other half: the contract and
the human page cannot drift apart.

Usage: tools/check-node-diagnostics.py [--gate] [<page>]
       --gate exits non-zero on a violation (for CI). Without it the report is printed and the
       exit is 0 so a reader can run it mid-edit.
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PAGE = ROOT / "docs/src/spec/node-diagnostics.md"
CONTRACT = ROOT / "node/src/diagnostics/health_contract.rs"

FIELDS_HEADER = ("| field | group | type | regime | scope | freshness | invariant |",)
REQUIRED_SECTIONS = ("## 2. The rules", "## 4. The field standard", "## 5. The gap register",
                     "## 7. Non-claims")
REGIMES = {"G", "C", "A", "L"}
SCOPES = {"S", "P", "N", "H"}


def rust_keys(source: str, name: str) -> list[str]:
    """The string literals of one `pub const NAME: &[&str] = &[ ... ];` in the contract."""
    match = re.search(rf"pub const {name}: &\[&str\] = &\[(.*?)\];", source, re.S)
    if not match:
        raise SystemExit(f"check-node-diagnostics: {name} not found in {CONTRACT}")
    return re.findall(r'"([^"]+)"', match.group(1))


def table_fields(page: str) -> list[tuple[str, list[str]]]:
    """Rows of the §4.1 table: (field name, cells). Two names in one cell split on `/`."""
    rows: list[tuple[str, list[str]]] = []
    start = page.find("### 4.1 Public")
    end = page.find("### 4.2", start)
    if start < 0 or end < 0:
        return rows
    block = page[start:end]
    header = FIELDS_HEADER[0] if isinstance(FIELDS_HEADER, tuple) else FIELDS_HEADER
    if header not in block:
        return rows
    for line in block.splitlines():
        if not line.startswith("| `"):
            continue
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if len(cells) != 7:
            continue
        names = [name.strip() for name in cells[0].strip("`").split("/")]
        for name in names:
            rows.append((name.strip("`"), cells))
    return rows


def main() -> int:
    gate = "--gate" in sys.argv
    args = [a for a in sys.argv[1:] if not a.startswith("-")]
    page_path = Path(args[0]) if args else PAGE
    problems: list[str] = []

    if not page_path.is_file():
        print(f"check-node-diagnostics: no page at {page_path}", file=sys.stderr)
        return 2
    page = page_path.read_text()
    contract = CONTRACT.read_text()

    for section in REQUIRED_SECTIONS:
        if section not in page:
            problems.append(f"page: missing required section {section!r}")

    rows = table_fields(page)
    if not rows:
        problems.append("page: no §4.1 field table rows were parsed (the scan would be vacuous)")

    documented = set()
    for name, cells in rows:
        documented.add(name)
        _field, _group, typ, regime, scope, freshness, invariant = cells
        if not typ:
            problems.append(f"{name}: empty type")
        if regime not in REGIMES:
            problems.append(f"{name}: regime {regime!r} is not one of G/C/A/L")
        if scope not in SCOPES:
            problems.append(f"{name}: scope {scope!r} is not one of S/P/N/H")
        if not freshness:
            problems.append(f"{name}: empty freshness")
        if not invariant:
            problems.append(f"{name}: empty invariant")

    declared = set(rust_keys(contract, "PUBLIC_HEALTH_KEYS"))
    for name in sorted(declared - documented):
        problems.append(f"{name}: declared in PUBLIC_HEALTH_KEYS but absent from the standard's table")
    for name in sorted(documented - declared):
        problems.append(f"{name}: in the standard's table but not declared in PUBLIC_HEALTH_KEYS")

    for name in rust_keys(contract, "OPERATOR_GATE_KEYS"):
        if name not in page:
            problems.append(f"{name}: an operator gate the page never names")

    print(f"check-node-diagnostics: {len(rows)} field(s), {len(declared)} declared public key(s), "
          f"{len(problems)} violation(s)")
    for problem in problems:
        print(f"  - {problem}")
    if gate and problems:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
