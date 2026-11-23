#!/usr/bin/env python3
"""check-stall-coverage — gate the deadly-embrace HAZOP study.

`docs/src/spec/stall-coverage.md` is a HAZOP study of stalls that stop block production. It is **not**
a release claim and carries no reading. This gate keeps the three surfaces apart and the study honest:

  * Table A is classified by representation, shape and root cause — never by claim or arm. A `Claim`,
    `Claim arm` or `Stall arm` column in Table A is refused;
  * every guide word disposed exactly once, and a `row` word carried by a Table A row;
  * a Safeguard whose first token is a definite rung R1..R4;
  * every representation, shape and root cause in the closed vocabularies, with Table B naming all
    three representations and Table C naming all nine root causes;
  * a Lean cell that names a law in `spec/laws.tsv` or a declaration that actually exists under
    `spec/Rchain/` (namespace-aware), or says `owed:`;
  * a Falsifier that is a real `path::symbol` or `owed:`;
  * the "Outside the diagram" residue table, each row naming a real declaration or path;
  * Table E placing every required claim arm exactly once, against a real `next_pr`, with `owed` or
    `live` status only and evidence for every `live` row — and no claim, arm or reading change;
  * the debt line matching what the tables actually owe;
  * no withdrawn `Law 129`–`Law 133` reference and no RC reading word on a table.

Usage: tools/check-stall-coverage.py [--gate]
Exit: 0 ok · 1 violation · 2 usage.
"""
from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PAGE = ROOT / "docs" / "src" / "spec" / "stall-coverage.md"
CLAIMS = ROOT / "spec" / "audit" / "release-claims.json"
LAWS = ROOT / "spec" / "laws.tsv"
LEAN = ROOT / "spec" / "Rchain"
VOCABULARY = ROOT / "spec" / "progress-vocabulary.tsv"
CROSSWALK = ROOT / "spec" / "progress-crosswalk.tsv"

GUIDE_WORDS = [
    "NO", "MORE", "LESS", "AS WELL AS", "PART OF", "OTHER THAN", "REVERSE", "EARLY", "LATE",
    "BEFORE", "AFTER",
]
REPRESENTATIONS = ("closed-silence", "protocol-wait", "protocol-stall")
SHAPES = (
    "void", "terminal", "drift", "split", "historic", "stuck", "silent", "persistent",
    "unrestorable", "paced",
)
ROOT_CAUSES = (
    "unsatisfiable-requirement", "self-referential-gating", "loss-blind-refusal",
    "divergent-readers", "historic-predicate", "recovery-before-anchor", "premature-verdict",
    "unverified-trust", "mixed-version",
)
READINGS = (
    "safe", "not-observed", "precondition", "unsafe-point", "model-incomplete",
    "evidence-insufficient",
)

TABLE_A_HEADER = [
    "ID", "Guide word", "Deviation", "RCA anchor", "Root cause", "Shape", "Representation",
    "Consequence", "Safeguard", "Lean cell", "Falsifier",
]
TABLE_B_HEADER = [
    "Representation", "What can be stuck", "Possible shapes", "Lean module/law",
    "Non-vacuity witness", "What it is not",
]
TABLE_C_HEADER = [
    "Root cause", "Definition", "DE rows", "Canonical shapes", "Classifying laws", "Default rung",
]
TABLE_D_HEADER = ["Deviation", "Law or shape that captures it", "Why"]
TABLE_E_HEADER = ["DE", "Claim", "Claim arm", "Campaign", "Measurement status", "Evidence"]
RESIDUE_HEADER = ["Residue", "Why Lean cannot state it", "Witness"]

RUNG = re.compile(r"^R[1-4]\b")
DE_ID = re.compile(r"^DE-[0-9]+$")
LAW_REF = re.compile(r"\bLaw ([0-9]+[a-z]?)\b")
LEAN_REF = re.compile(r"\bRchain\.[A-Za-z0-9_'.]+")
PATH_SYMBOL = re.compile(
    r"([A-Za-z0-9_./-]+\.(?:rs|lean|md|sh|py|tsv|txt|json|toml|v))(::([A-Za-z0-9_:]+))?")
WITHDRAWN_LAW = re.compile(r"\bLaw (129|130|131|132|133)\b")


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


def is_separator(row: list[str]) -> bool:
    return bool(row) and all(cell and set(cell) <= {"-", ":"} for cell in row) and any(
        "-" in cell for cell in row)


def table_rows(lines: list[str], header: list[str], label: str,
               violations: Violations) -> list[list[str]]:
    header_at = next((i for i, line in enumerate(lines)
                      if is_table_row(line) and cells(line) == header), None)
    if header_at is None:
        violations.add(f"no {label} header (`| {' | '.join(header)} |`)")
        return []
    rows: list[list[str]] = []
    for line in lines[header_at + 1:]:
        if not is_table_row(line):
            if rows:
                break
            continue
        row = cells(line)
        if is_separator(row):
            continue
        rows.append(row)
    return rows


def load_claims() -> tuple[dict[str, dict], dict[tuple[str, str], str], set[str]]:
    data = json.loads(CLAIMS.read_text())
    claims = {claim["id"]: claim for claim in data["claims"]}
    required = {
        (claim["id"], arm["id"]): arm["environment"]
        for claim in data["claims"]
        for arm in claim["arms"]
        if arm.get("required")
    }
    campaigns = {pr["id"] for pr in data.get("action_prs", [])}
    return claims, required, campaigns


def load_vocabulary(violations: Violations) -> dict[str, set[str]]:
    """Read the emitted vocabulary; the page's closed axes are the Lean module's, not a copy."""
    if not VOCABULARY.exists():
        violations.add(
            "no spec/progress-vocabulary.tsv — run tools/emit-lean-laws.sh; the classification is "
            "emitted from Lean")
        return {}
    vocabulary: dict[str, set[str]] = {}
    for line in VOCABULARY.read_text().splitlines():
        if not line or line.startswith("axis\t"):
            continue
        parts = line.split("\t")
        if len(parts) != 2:
            violations.add(f"vocabulary row is not `axis<TAB>wire`: {line!r}")
            continue
        vocabulary.setdefault(parts[0], set()).add(parts[1])
    expected = {
        "representation": set(REPRESENTATIONS),
        "shape": set(SHAPES),
        "root-cause": set(ROOT_CAUSES),
    }
    for axis, wires in expected.items():
        if vocabulary.get(axis, set()) != wires:
            violations.add(
                f"vocabulary drift: {axis} is {sorted(vocabulary.get(axis, set()))}, "
                f"want {sorted(wires)}")
    return vocabulary


def load_crosswalk(violations: Violations) -> dict[tuple[str, str], tuple[str, str, str]]:
    """The explicitly attributed register entries: `(number, clause) -> (rep, shape, cause)`."""
    if not CROSSWALK.exists():
        violations.add(
            "no spec/progress-crosswalk.tsv — run tools/emit-lean-laws.sh; the classification is "
            "emitted from Lean")
        return {}
    crosswalk: dict[tuple[str, str], tuple[str, str, str]] = {}
    for line in CROSSWALK.read_text().splitlines():
        if not line or line.startswith("number\t"):
            continue
        parts = line.split("\t")
        if len(parts) < 8:
            violations.add(f"crosswalk row has {len(parts)} cells, want at least 8: {line!r}")
            continue
        number, clause, _role, rep, shape, cause, _rung, explicit = parts[:8]
        if explicit == "true":
            crosswalk[(number, clause)] = (rep, shape, cause)
    return crosswalk


def crosswalk_keys(law: str, crosswalk: dict[tuple[str, str], tuple[str, str, str]]
                   ) -> list[tuple[str, str]]:
    match = re.fullmatch(r"([0-9]+)([a-z]?)", law)
    if not match:
        return []
    number, clause = match.groups()
    if clause:
        key = (number, clause)
        return [key] if key in crosswalk else []
    return [key for key in crosswalk if key[0] == number]


def check_law_explicit(law: str, crosswalk: dict[tuple[str, str], tuple[str, str, str]],
                       violations: Violations, label: str) -> None:
    if not crosswalk_keys(law, crosswalk):
        violations.add(f"{label}: law {law} has no explicit attribution in progress-crosswalk.tsv")


def load_law_keys() -> set[str]:
    """`number` and `number+clause`: `Law 51` and `Law 51b` both resolve."""
    keys: set[str] = set()
    for line in LAWS.read_text().splitlines():
        if not line or line.startswith("#"):
            continue
        fields = line.split("\t")
        number = fields[0].strip()
        if not number:
            continue
        keys.add(number)
        clause = fields[1].strip() if len(fields) > 1 else ""
        if clause:
            keys.add(f"{number}{clause}")
    return keys


def strip_comments(raw: str, in_block: bool) -> tuple[str, bool]:
    """Remove Lean line and block comments from one line; return the line and the block state."""
    line = raw
    while "/-" in line:
        before, rest = line.split("/-", 1)
        if "-/" in rest:
            line = before + rest.split("-/", 1)[1]
            continue
        line = before
        in_block = True
        break
    if in_block:
        if "-/" in line:
            line = line.split("-/", 1)[1]
            in_block = False
        else:
            return "", True
    return line.split("--", 1)[0], in_block


def load_lean_names() -> set[str]:
    """Fully qualified declarations and namespaces under spec/Rchain, section-aware."""
    names: set[str] = set()
    declaration = re.compile(
        r"^\s*(?:@\[[^\]]*\]\s*)?"
        r"(?:protected\s+|private\s+|noncomputable\s+|partial\s+|unsafe\s+)?"
        r"(theorem|lemma|def|abbrev|structure|inductive|class)\s+"
        r"([A-Za-z_][A-Za-z0-9_'.]*)")
    namespace = re.compile(r"^\s*namespace\s+([A-Za-z_][A-Za-z0-9_'.]*)\s*$")
    section = re.compile(r"^\s*section(?:\s+[A-Za-z_][A-Za-z0-9_'.]*)?\s*$")
    end = re.compile(r"^\s*end(?:\s+[A-Za-z_][A-Za-z0-9_'.]*)?\s*$")
    for path in LEAN.rglob("*.lean"):
        stack: list[str] = []
        in_block = False
        for raw in path.read_text(errors="ignore").splitlines():
            line, in_block = strip_comments(raw, in_block)
            match = namespace.match(line)
            if match:
                stack.append(match.group(1))
                names.add(".".join(stack))
                continue
            if section.match(line):
                stack.append("")
                continue
            if end.match(line):
                if stack:
                    stack.pop()
                continue
            match = declaration.match(line)
            if match:
                prefix = [part for part in stack if part]
                names.add(".".join(prefix + [match.group(2)]))
                names.add(match.group(2))
    return names


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


def check_laws(cell: str, laws: set[str], violations: Violations, row_id: str) -> None:
    for law in LAW_REF.findall(cell):
        if law not in laws:
            violations.add(f"{row_id}: law {law} is not in spec/laws.tsv")


def check_lean_tokens(cell: str, lean: set[str], violations: Violations, row_id: str) -> None:
    for token in LEAN_REF.findall(cell):
        if token not in lean:
            violations.add(f"{row_id}: Lean declaration {token!r} not found under spec/Rchain/")


def lean_cell_ok(cell: str, laws: set[str], lean: set[str], violations: Violations,
                 row_id: str) -> bool:
    text = strip_code(cell)
    if "owed" in text.lower():
        return True
    ok = False
    for part in [p.strip().strip("`") for p in text.split(",") if p.strip()]:
        if LAW_REF.fullmatch(part):
            check_laws(part, laws, violations, row_id)
            ok = True
            continue
        if LEAN_REF.fullmatch(part):
            check_lean_tokens(part, lean, violations, row_id)
            ok = True
            continue
        violations.add(
            f"{row_id}: Lean cell part is neither a Law, an Rchain declaration, nor owed: {part!r}")
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


def parse_table_a(lines: list[str], laws: set[str], lean: set[str],
                  crosswalk: dict[tuple[str, str], tuple[str, str, str]],
                  violations: Violations) -> tuple[dict[str, str], set[str]]:
    """Return DE id → its root cause, and the set of guide words carried."""
    for line in lines:
        if not is_table_row(line):
            continue
        row = cells(line)
        if row and row[0] == "ID" and any(
                cell in {"Claim", "Claim arm", "Stall arm"} for cell in row):
            violations.add(
                "Table A carries an RC column; the HAZOP surface has no Claim, Claim arm or Stall "
                "arm column — placement belongs in Table E")
    seen: dict[str, str] = {}
    words: set[str] = set()
    for row in table_rows(lines, TABLE_A_HEADER, "Table A", violations):
        if len(row) != 11:
            violations.add(f"Table A row has {len(row)} cells, want 11: {' | '.join(row)[:90]!r}")
            continue
        (row_id, word, deviation, anchor, cause, shape, representation, consequence, safeguard,
         lean_cell, falsifier) = row
        if not DE_ID.fullmatch(row_id):
            violations.add(f"bad id {row_id!r}")
            continue
        if row_id in seen:
            violations.add(f"duplicate id {row_id}")
        seen[row_id] = cause
        if word not in GUIDE_WORDS:
            violations.add(f"{row_id}: guide word {word!r} is not canonical")
        words.add(word)
        if cause not in ROOT_CAUSES:
            violations.add(f"{row_id}: root cause {cause!r} is not in the vocabulary")
        if shape not in SHAPES:
            violations.add(f"{row_id}: shape {shape!r} is not in the vocabulary")
        if representation not in REPRESENTATIONS:
            violations.add(f"{row_id}: representation {representation!r} is not in the vocabulary")
        if not RUNG.match(strip_code(safeguard)):
            violations.add(f"{row_id}: safeguard does not start with R1..R4: {safeguard!r}")
        if not deviation or not consequence or not anchor:
            violations.add(f"{row_id}: empty deviation, RCA anchor or consequence")
        lean_cell_ok(lean_cell, laws, lean, violations, row_id)
        for law in LAW_REF.findall(lean_cell):
            keys = crosswalk_keys(law, crosswalk)
            if not keys:
                violations.add(f"{row_id}: law {law} has no explicit crosswalk attribution")
            for key in keys:
                cross_rep, cross_shape, _cross_cause = crosswalk[key]
                if cross_rep != "-" and (cross_rep != representation or cross_shape != shape):
                    violations.add(
                        f"{row_id}: law {law}'s crosswalk shape is {cross_rep}/{cross_shape}, not "
                        f"the row's {representation}/{shape}")
        falsifier_exists(falsifier, violations, row_id)
    if not seen:
        violations.add("Table A has no rows")
    return seen, words


def parse_table_b(lines: list[str], laws: set[str], lean: set[str],
                  violations: Violations) -> None:
    seen: set[str] = set()
    for row in table_rows(lines, TABLE_B_HEADER, "Table B", violations):
        if len(row) != 6:
            violations.add(f"Table B row has {len(row)} cells, want 6")
            continue
        representation = strip_code(row[0])
        if representation not in REPRESENTATIONS:
            violations.add(f"Table B: unknown representation {representation!r}")
        if representation in seen:
            violations.add(f"Table B: duplicate representation {representation!r}")
        seen.add(representation)
        check_laws(row[3], laws, violations, f"Table B/{representation}")
        check_lean_tokens(" ".join(row[3:]), lean, violations, f"Table B/{representation}")
        if not LEAN_REF.search(" ".join(row[3:])) and "owed" not in " ".join(row[3:]).lower():
            violations.add(f"Table B/{representation}: names no non-vacuity witness or owed debt")
    missing = set(REPRESENTATIONS) - seen
    if missing:
        violations.add(f"Table B omits representation(s): {', '.join(sorted(missing))}")


def parse_table_c(lines: list[str], de_ids: set[str], laws: set[str],
                  violations: Violations) -> None:
    seen: set[str] = set()
    for row in table_rows(lines, TABLE_C_HEADER, "Table C", violations):
        if len(row) != 6:
            violations.add(f"Table C row has {len(row)} cells, want 6")
            continue
        cause = strip_code(row[0])
        if cause not in ROOT_CAUSES:
            violations.add(f"Table C: unknown root cause {cause!r}")
        if cause in seen:
            violations.add(f"Table C: duplicate root cause {cause!r}")
        seen.add(cause)
        ids = re.findall(r"DE-[0-9]+", row[2])
        if not ids:
            violations.add(f"Table C/{cause}: names no DE row")
        for de in ids:
            if de not in de_ids:
                violations.add(f"Table C/{cause}: unknown DE row {de}")
        shapes = [part.strip() for part in row[3].split(",") if part.strip()]
        if not shapes:
            violations.add(f"Table C/{cause}: names no canonical shape")
        for shape in shapes:
            if shape not in SHAPES:
                violations.add(f"Table C/{cause}: unknown shape {shape!r}")
        check_laws(row[4], laws, violations, f"Table C/{cause}")
        if not RUNG.match(strip_code(row[5])):
            violations.add(f"Table C/{cause}: default rung is not R1..R4: {row[5]!r}")
    missing = set(ROOT_CAUSES) - seen
    if missing:
        violations.add(f"Table C omits root cause(s): {', '.join(sorted(missing))}")


def parse_table_d(lines: list[str], de_ids: set[str], laws: set[str], lean: set[str],
                  violations: Violations) -> None:
    covered: set[str] = set()
    for row in table_rows(lines, TABLE_D_HEADER, "Table D", violations):
        if len(row) != 3:
            violations.add(f"Table D row has {len(row)} cells, want 3")
            continue
        ids = re.findall(r"DE-[0-9]+", row[0])
        if not ids and row[0].strip().lower() != "all":
            violations.add(f"Table D row names no DE row: {row[0]!r}")
        for de in ids:
            if de not in de_ids:
                violations.add(f"Table D: unknown DE row {de}")
            covered.add(de)
        check_laws(row[1], laws, violations, f"Table D/{row[0]}")
        check_lean_tokens(row[1], lean, violations, f"Table D/{row[0]}")
    missing = de_ids - covered
    if missing:
        violations.add(f"Table D omits deviation(s): {', '.join(sorted(missing))}")


def tracked(relative: str) -> bool:
    result = subprocess.run(["git", "-C", str(ROOT), "ls-files", "--error-unmatch", relative],
                            capture_output=True, text=True)
    return result.returncode == 0


def parse_table_e(lines: list[str], required: dict[tuple[str, str], str],
                  campaigns: set[str], de_ids: set[str],
                  violations: Violations) -> tuple[int, int]:
    seen: set[tuple[str, str]] = set()
    covered: set[str] = set()
    owed = 0
    for row in table_rows(lines, TABLE_E_HEADER, "Table E", violations):
        if len(row) != 6:
            violations.add(f"Table E row has {len(row)} cells, want 6")
            continue
        de, claim_id, arm_id, campaign, status, evidence = row
        if de not in de_ids:
            violations.add(f"Table E: unknown DE row {de!r}")
        covered.add(de)
        key = (claim_id, arm_id)
        if key not in required:
            violations.add(f"Table E: {claim_id}/{arm_id} is not a required arm")
            continue
        if key in seen:
            violations.add(f"Table E: duplicate row for {claim_id}/{arm_id}")
        seen.add(key)
        if campaign not in campaigns:
            violations.add(f"Table E: {claim_id}/{arm_id} campaign {campaign!r} is not a next_pr")
        if status == "owed":
            owed += 1
            if evidence != "-":
                violations.add(f"Table E: {claim_id}/{arm_id} is owed but names evidence {evidence!r}")
        elif status == "live":
            if not evidence.startswith("spec/audit/evidence/"):
                violations.add(
                    f"Table E: {claim_id}/{arm_id} is live without a spec/audit/evidence path")
            elif not (ROOT / evidence).exists() or not tracked(evidence):
                violations.add(f"Table E: {claim_id}/{arm_id} evidence is not a tracked file: {evidence}")
        elif status in READINGS:
            violations.add(
                f"Table E: {claim_id}/{arm_id} uses the RC reading {status!r}; the HAZOP surface "
                "records owed/live measurement, not a claim reading")
        else:
            violations.add(f"Table E: {claim_id}/{arm_id} status {status!r} is neither owed nor live")
    for key in required:
        if key not in seen:
            violations.add(f"Table E: required arm {key[0]}/{key[1]} has no placement row")
    missing = de_ids - covered
    if missing:
        violations.add(f"Table E places no measurement for deviation(s): {', '.join(sorted(missing))}")
    return owed, len(seen)


def parse_residue(lines: list[str], lean: set[str], violations: Violations) -> int:
    rows = table_rows(lines, RESIDUE_HEADER, "residue", violations)
    for row in rows:
        if len(row) != 3:
            violations.add(f"residue row has {len(row)} cells, want 3")
            continue
        witness = row[2]
        tokens = LEAN_REF.findall(witness)
        if tokens:
            check_lean_tokens(witness, lean, violations, f"residue/{row[0]}")
            continue
        match = PATH_SYMBOL.search(witness)
        if not match or not (ROOT / match.group(1)).exists():
            violations.add(f"residue/{row[0]}: witness resolves to neither a declaration nor a path")
    if len(rows) < 6:
        violations.add(f"residue table has {len(rows)} rows, want at least 6")
    return len(rows)


def debt_line(lines: list[str], deviations: int, owed: int, placed: int, required: int,
              violations: Violations) -> None:
    line = next((l for l in lines if l.strip().startswith("**Debt.**")), None)
    if line is None:
        violations.add("no `**Debt.**` line")
        return
    match = re.search(
        r"deviations: ([0-9]+) · placement rows owed: ([0-9]+) · required arms placed: "
        r"([0-9]+)/([0-9]+)", line)
    if not match:
        violations.add(f"debt line is not in the gated form: {line.strip()!r}")
        return
    declared = tuple(int(value) for value in match.groups())
    actual = (deviations, owed, placed, required)
    if declared != actual:
        violations.add(f"debt line says {declared}; the tables say {actual}")


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
    text = page.read_text()
    lines = text.splitlines()
    claims, required, campaigns = load_claims()
    laws = load_law_keys()
    lean = load_lean_names()
    violations = Violations()
    load_vocabulary(violations)
    crosswalk = load_crosswalk(violations)

    for law in sorted(set(WITHDRAWN_LAW.findall(text))):
        violations.add(f"the page cites withdrawn Law {law}; the consolidation folds it into 51/53")
    for word in ("pass_condition", "fail_condition"):
        if word in text:
            violations.add(f"the HAZOP surface must not carry the RC field {word!r}")

    row_words = parse_guide_words(lines, violations)
    de_causes, table_words = parse_table_a(lines, laws, lean, crosswalk, violations)
    for word in row_words:
        if word not in table_words:
            violations.add(f"guide word {word} is declared `row` but no Table A row carries it")
    for word in table_words:
        if word not in row_words:
            violations.add(f"Table A carries {word!r}, which is not declared `row`")
    if "TE-7" not in text:
        violations.add("the page never names TE-7")

    de_ids = set(de_causes)
    parse_table_b(lines, laws, lean, violations)
    parse_table_c(lines, de_ids, laws, violations)
    parse_table_d(lines, de_ids, laws, lean, violations)
    for header, label in ((TABLE_B_HEADER, "Table B"), (TABLE_C_HEADER, "Table C"),
                          (TABLE_D_HEADER, "Table D")):
        for row in table_rows(lines, header, label, Violations()):
            for cell in row:
                for law in LAW_REF.findall(cell):
                    check_law_explicit(law, crosswalk, violations, label)
    owed, placed = parse_table_e(lines, required, campaigns, de_ids, violations)
    parse_residue(lines, lean, violations)

    for row in table_rows(lines, TABLE_A_HEADER, "Table A", Violations()):
        for cell in row:
            if strip_code(cell) in READINGS:
                violations.add(f"Table A carries the RC reading {strip_code(cell)!r}")

    debt_line(lines, len(de_ids), owed, placed, len(required), violations)

    if not violations.ok():
        return violations.report("stall-coverage.md")
    print(
        f"check-stall-coverage: ok {len(de_ids)} deviation row(s), {placed} placement row(s) "
        f"({owed} owed), {len(required)} required arm(s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
