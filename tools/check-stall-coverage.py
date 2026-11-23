#!/usr/bin/env python3
"""check-stall-coverage — gate the deadly-embrace HAZOP study.

`docs/src/spec/stall-coverage.md` is a HAZOP study over the release claims: every deviation that can
produce an absorbing stall is disposed against the eleven guide words, mapped to a Lean shape or a law
owed, and assigned to an arm of the claim whose environment can exhibit it. This gate refuses the
document the same way `check-hazop-worksheet.sh` refuses its worksheet:

  * every guide word disposed exactly once, and a `row` word carried by a Table A row;
  * a Safeguard whose first token is a definite rung R1..R4;
  * a Claim that exists in `spec/audit/release-claims.json`, an Environment that claim declares, and
    an Arm that exists under that claim (`arm:<id>`) or is declared debt (`owed:`);
  * a Lean cell that names a real declaration under `spec/Rchain/` or a law in `spec/laws.tsv`, or
    says `owed`;
  * a Falsifier that is a real `path::symbol` or `owed:`;
  * a coverage ledger with every required arm exactly once and every cell naming a real DE row (or
    `n/a (reason)`);
  * the debt line matching what the tables actually owe;
  * a non-vacuous scan.

Usage: tools/check-stall-coverage.py [--gate]
Exit: 0 ok · 1 violation · 2 usage.
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PAGE = ROOT / "docs" / "src" / "spec" / "stall-coverage.md"
CLAIMS = ROOT / "spec" / "audit" / "release-claims.json"
LAWS = ROOT / "spec" / "laws.tsv"
LEAN = ROOT / "spec" / "Rchain"

GUIDE_WORDS = [
    "NO", "MORE", "LESS", "AS WELL AS", "PART OF", "OTHER THAN", "REVERSE", "EARLY", "LATE",
    "BEFORE", "AFTER",
]
RUNG = re.compile(r"^R[1-4]\b")
DE_ID = re.compile(r"^DE-[0-9]+$")
SLUG = re.compile(r"^[a-z0-9]+(-[a-z0-9]+)*$")
PATH_SYMBOL = re.compile(r"([A-Za-z0-9_./-]+\.(?:rs|lean|md|sh|py|tsv|txt|json|toml|v))(::([A-Za-z0-9_:]+))?")


class Violations:
    def __init__(self) -> None:
        self.rows: list[str] = []

    def add(self, message: str) -> None:
        self.rows.append(message)

    def ok(self) -> bool:
        return not self.rows

    def report(self, what: str) -> int:
        if self.rows:
            print(f"check-stall-coverage: {len(self.rows)} violation(s) in {what}", file=sys.stderr)
            for row in self.rows:
                print(f"  {row}", file=sys.stderr)
            return 1
        return 0


def cells(line: str) -> list[str]:
    """A markdown pipe row's cells, trimmed; the leading and trailing pipes are not cells."""
    return [cell.strip() for cell in line.strip().strip("|").split("|")]


def strip_code(cell: str) -> str:
    return cell.strip().strip("`").strip()


def is_table_row(line: str) -> bool:
    return line.lstrip().startswith("|")


def load_claims() -> tuple[dict[str, dict], dict[tuple[str, str], str]]:
    data = json.loads(CLAIMS.read_text())
    claims = {claim["id"]: claim for claim in data["claims"]}
    required = {
        (claim["id"], arm["id"]): arm["environment"]
        for claim in data["claims"]
        for arm in claim["arms"]
        if arm.get("required")
    }
    return claims, required


def load_law_numbers() -> set[str]:
    numbers: set[str] = set()
    for line in LAWS.read_text().splitlines():
        if not line or line.startswith("#"):
            continue
        first = line.split("\t", 1)[0].strip()
        if first:
            numbers.add(first)
            base = re.match(r"[0-9]+", first)
            if base:
                numbers.add(base.group(0))
    return numbers


def lean_declaration_exists(name: str) -> bool:
    """`Rchain.MergeScope.close_spec` → the final component must occur in a Lean source under spec/Rchain."""
    symbol = name.rsplit(".", 1)[-1]
    if not symbol:
        return False
    for path in LEAN.rglob("*.lean"):
        if symbol in path.read_text(errors="ignore"):
            return True
    return False


def falsifier_exists(cell: str, violations: Violations, row_id: str) -> None:
    text = strip_code(cell)
    if text.startswith("owed:"):
        return
    match = PATH_SYMBOL.search(text)
    if not match:
        violations.add(f"{row_id}: Falsifier is neither `owed:` nor a path::symbol: {cell!r}")
        return
    path = ROOT / match.group(1)
    if not path.exists():
        violations.add(f"{row_id}: Falsifier path does not exist: {match.group(1)}")
        return
    symbol = match.group(3)
    if symbol and symbol not in path.read_text(errors="ignore"):
        violations.add(f"{row_id}: Falsifier symbol {symbol!r} not found in {match.group(1)}")


def lean_cell_ok(cell: str, law_numbers: set[str], violations: Violations, row_id: str) -> bool:
    text = strip_code(cell)
    if "owed" in text.lower():
        return True
    ok = False
    for part in [p.strip().strip("`") for p in text.split(",") if p.strip()]:
        law = re.fullmatch(r"Law ([0-9]+[a-z]?)", part)
        if law:
            if law.group(1) not in law_numbers:
                violations.add(f"{row_id}: law {law.group(1)} is not in spec/laws.tsv")
            ok = True
            continue
        if part.startswith("Rchain."):
            if not lean_declaration_exists(part):
                violations.add(f"{row_id}: Lean declaration {part!r} not found under spec/Rchain/")
            ok = True
            continue
        violations.add(f"{row_id}: Lean cell part is neither a Law, an Rchain declaration, nor owed: {part!r}")
    if not ok:
        violations.add(f"{row_id}: Lean cell names nothing: {cell!r}")
    return ok


def parse_guide_words(lines: list[str], violations: Violations) -> set[str]:
    line = next((l for l in lines if l.strip().startswith("**Guide words.**")), None)
    if line is None:
        violations.add("no `**Guide words.**` line")
        return set()
    body = line.split("**Guide words.**", 1)[1].strip().rstrip(".")
    counts: dict[str, int] = {word: 0 for word in GUIDE_WORDS}
    rows: set[str] = set()
    for segment in body.split("·"):
        if ":" not in segment:
            violations.add(f"guide-word segment has no kind: {segment!r}")
            continue
        kind, rest = segment.split(":", 1)
        kind = kind.strip().lower()
        if kind == "folded":
            for pair in rest.split(","):
                if "->" not in pair:
                    violations.add(f"folded guide word without ->: {pair!r}")
                    continue
                left = pair.split("->", 1)[0].strip()
                counts[left] = counts.get(left, 0) + 1
            continue
        if kind not in {"row", "vacuous", "owed"}:
            violations.add(f"unknown guide-word kind {kind!r}")
            continue
        for word in rest.split(","):
            word = re.sub(r"\(.*?\)", "", word).strip()
            if word not in counts:
                violations.add(f"unknown guide word {word!r}")
                continue
            counts[word] += 1
            if kind == "row":
                rows.add(word)
    for word in GUIDE_WORDS:
        if counts[word] != 1:
            violations.add(f"guide word {word} disposed {counts[word]} times (want exactly 1)")
    return rows


def parse_table_a(lines: list[str], claims: dict[str, dict],
                  required: dict[tuple[str, str], str], law_numbers: set[str],
                  violations: Violations) -> dict[str, bool]:
    """Return DE id → whether its Arm cell is debt."""
    header_at = next((i for i, l in enumerate(lines)
                      if is_table_row(l) and cells(l)[:3] == ["ID", "Claim", "Claim arm"]), None)
    if header_at is None:
        violations.add("no Table A header (`| ID | Claim | Claim arm | ...`)")
        return {}
    owed: dict[str, bool] = {}
    seen: set[str] = set()
    for line in lines[header_at + 1:]:
        if not is_table_row(line):
            if seen:
                break
            continue
        row = cells(line)
        if row and all(cell and set(cell) <= {"-", ":"} for cell in row):
            continue
        if len(row) != 11:
            violations.add(f"Table A row has {len(row)} cells, want 11: {line[:80]!r}")
            continue
        (row_id, claim_id, claim_arm, word, deviation, cause, consequence, safeguard,
         lean, stall_arm, falsifier) = row
        if not DE_ID.fullmatch(row_id):
            violations.add(f"bad id {row_id!r}")
            continue
        if row_id in seen:
            violations.add(f"duplicate id {row_id}")
        seen.add(row_id)
        if claim_id not in claims:
            violations.add(f"{row_id}: claim {claim_id} not in release-claims.json")
        environment = None
        if (claim_id, claim_arm) not in required:
            violations.add(f"{row_id}: {claim_arm} is not a required arm of {claim_id}")
        else:
            environment = required[(claim_id, claim_arm)]
        if word not in GUIDE_WORDS:
            violations.add(f"{row_id}: guide word {word!r} is not canonical")
        if not RUNG.match(strip_code(safeguard)):
            violations.add(f"{row_id}: safeguard does not start with R1..R4: {safeguard!r}")
        if not cause:
            violations.add(f"{row_id}: empty cause")
        if not deviation or not consequence:
            violations.add(f"{row_id}: empty deviation or consequence")
        arm_text = strip_code(stall_arm)
        if arm_text.startswith("owed:"):
            owed_id = arm_text.split(":", 1)[1].strip()
            if not SLUG.fullmatch(owed_id):
                violations.add(f"{row_id}: owed arm is not a lowercase slug: {owed_id!r}")
            owed[row_id] = True
        elif arm_text.startswith("arm:"):
            arm_id = arm_text.split(":", 1)[1].strip()
            key = (claim_id, arm_id)
            if key not in required:
                violations.add(f"{row_id}: stall arm {arm_id} is not a required arm of {claim_id}")
            elif environment is not None and required[key] != environment:
                violations.add(
                    f"{row_id}: stall arm {arm_id} is {required[key]}, not the claim arm's "
                    f"{environment}")
            owed[row_id] = False
        else:
            violations.add(f"{row_id}: Stall arm is neither arm:<id> nor owed:<slug>: {stall_arm!r}")
            owed[row_id] = True
        lean_cell_ok(lean, law_numbers, violations, row_id)
        falsifier_exists(falsifier, violations, row_id)
    if not seen:
        violations.add("Table A has no rows")
    return owed


def parse_ledger(lines: list[str], required: dict[tuple[str, str], str],
                 owed: dict[str, bool], violations: Violations) -> int:
    header_at = next((i for i, l in enumerate(lines)
                      if is_table_row(l) and cells(l)[:2] == ["Claim", "Arm"]), None)
    if header_at is None:
        violations.add("no coverage-ledger header (`| Claim | Arm | ...`)")
        return 0
    seen: set[tuple[str, str]] = set()
    resting = 0
    for line in lines[header_at + 1:]:
        if not is_table_row(line):
            if seen:
                break
            continue
        row = cells(line)
        if row and all(cell and set(cell) <= {"-", ":"} for cell in row):
            continue
        if len(row) != 4:
            violations.add(f"ledger row has {len(row)} cells, want 4: {line[:80]!r}")
            continue
        claim_id, arm_id, environment, stall_rows = row
        key = (claim_id, arm_id)
        if key not in required:
            violations.add(f"ledger: {claim_id}/{arm_id} is not a required arm")
            continue
        if key in seen:
            violations.add(f"ledger: duplicate row for {claim_id}/{arm_id}")
        seen.add(key)
        if required[key] != environment:
            violations.add(f"ledger: {claim_id}/{arm_id} is {required[key]}, not {environment}")
        if stall_rows.startswith("n/a"):
            if not re.search(r"n/a \(.+\)", stall_rows):
                violations.add(f"ledger: {claim_id}/{arm_id} is n/a without a reason")
            continue
        ids = [token for token in re.findall(r"DE-[0-9]+", stall_rows)]
        if not ids:
            violations.add(f"ledger: {claim_id}/{arm_id} names no DE row and is not n/a (reason)")
            continue
        if any(de not in owed for de in ids):
            violations.add(f"ledger: {claim_id}/{arm_id} names an unknown DE row: {stall_rows!r}")
        elif any(owed[de] for de in ids):
            resting += 1
    for key in required:
        if key not in seen:
            violations.add(f"ledger: required arm {key[0]}/{key[1]} has no row")
    return resting


def debt_line(lines: list[str], rows_owed: int, resting: int, violations: Violations) -> None:
    line = next((l for l in lines if l.strip().startswith("**Debt.**")), None)
    if line is None:
        violations.add("no `**Debt.**` line")
        return
    match = re.search(r"stall rows owed: ([0-9]+) · ledger rows resting on an owed arm: ([0-9]+)",
                      line)
    if not match:
        violations.add(f"debt line is not in the gated form: {line.strip()!r}")
        return
    if int(match.group(1)) != rows_owed:
        violations.add(f"debt line says {match.group(1)} stall rows owed; the table has {rows_owed}")
    if int(match.group(2)) != resting:
        violations.add(
            f"debt line says {match.group(2)} ledger rows resting on an owed arm; the ledger has {resting}")


def main(argv: list[str]) -> int:
    import argparse

    parser = argparse.ArgumentParser(description="Gate the deadly-embrace HAZOP study.")
    parser.add_argument("--gate", action="store_true", help="the CI form; same check")
    parser.add_argument("--page", type=Path, default=PAGE, help=argparse.SUPPRESS)
    args = parser.parse_args(argv[1:])
    page = args.page
    if not page.exists():
        print(f"check-stall-coverage: no {page}", file=sys.stderr)
        return 1
    lines = page.read_text().splitlines()
    claims, required = load_claims()
    law_numbers = load_law_numbers()
    violations = Violations()

    row_words = parse_guide_words(lines, violations)
    owed = parse_table_a(lines, claims, required, law_numbers, violations)
    table_words = set()
    for line in lines:
        if is_table_row(line):
            row = cells(line)
            if len(row) == 11 and DE_ID.fullmatch(row[0]):
                table_words.add(row[3])
    for word in row_words:
        if word not in table_words:
            violations.add(f"guide word {word} is declared `row` but no Table A row carries it")
    for word in table_words:
        if word not in row_words:
            violations.add(f"Table A carries {word!r}, which is not declared `row`")
    if not any(l.strip().startswith("**TE-7") or "TE-7" in l for l in lines):
        violations.add("the page never names TE-7")
    resting = parse_ledger(lines, required, owed, violations)
    debt_line(lines, sum(1 for value in owed.values() if value), resting, violations)

    if not violations.ok():
        return violations.report("stall-coverage.md")
    print(
        f"check-stall-coverage: ok {len(owed)} deviation row(s), {len(required)} required arm(s), "
        f"{sum(1 for value in owed.values() if value)} owed row(s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
