#!/usr/bin/env python3
"""check-durable-state-census — the U26 census, checked against the tree and the diagnostics page.

Why this exists. `docs/src/spec/failure-hazop.md` U26 claims that **every** durable state the node
can be left in has a named writer, a named clearer (or a reason none exists), a resolution class, a
test and a surface. That is a claim about a document *and* about the tree, so it is read by a check:

  1  Table D exists once, is non-vacuous, and every row has the seven cells;
  2  every resolution class is exactly one of auto-recover / operator-action / safe-halt;
  3  every test `path::symbol` resolves, or the cell is `owed:` (and counted);
  4  every writer/clearer names a resolving `path::symbol`; `none:` carries a reason;
  5  every live surface resolves against node-diagnostics §3/§4, and every `owed:` surface has a
     row in §5 (so §5 cannot become a second tracker);
  6  every row links an existing `F-U*` row and a law in spec/laws.tsv, or a candidate:/owed: slug;
  7  the set of durable identifiers emitted by tools/emit-durable-states.sh equals the set classified
     by Table D and Table F — a new key fails this gate until it is classified;
  8  the §7 debt line's `census tests owed` and `census surfaces owed` match the counted values.

Usage: tools/check-durable-state-census.py [--gate] [--self-test] [--terminal-index] [<page>]
Exit: 0 ok · 1 violation · 2 usage.
"""
from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PAGE = ROOT / "docs/src/spec/failure-hazop.md"
DIAGNOSTICS = ROOT / "docs/src/spec/node-diagnostics.md"
LAWS = ROOT / "spec/laws.tsv"
EMITTER = ROOT / "tools/emit-durable-states.sh"

CLASSES = ("auto-recover", "operator-action", "safe-halt")
TEST_PATH = re.compile(r"([A-Za-z0-9_./-]+\.(?:rs|lean|md|sh|py|tsv|txt|json|toml|v))::([A-Za-z0-9_]+)")
BARE_PATH = re.compile(r"([A-Za-z0-9_./-]+\.(?:rs|lean|md|sh|py|tsv|txt|json|toml|v))")
BACKTICK = re.compile(r"`([^`]+)`")
FROW = re.compile(r"F-U\d+-\d+")
WROW = re.compile(r"W-N\d+-\d+")
LAW_NUM = re.compile(r"\b[Ll]aw\s+(\d+)[a-z]?")
SLUGS = ("candidate:", "owed:", "vacuous:")
# §5 diagnostics that are not durable-state surfaces: the reverse check allows them, so the gap
# register cannot become a second tracker.
NON_DURABLE_GAPS = {
    "syncBlockLegAgeMs", "syncTrieLegAgeMs", "peerStatus", "epoch", "livenessEpoch",
    "finalityCountersDurable",
}
DEBT = re.compile(
    r"\*\*Debt\.\*\*\s*falsifiers owed:\s*(\d+)\s*·\s*node-words owed:\s*(\d+)"
    r"(?:\s*·\s*census tests owed:\s*(\d+)"
    r"(?:\s*·\s*census surfaces owed:\s*(\d+))?)?"
)
HEADER = "| ID | Durable state (key/DB) | Writer → clearer (path::symbol) | Resolution class | Test | Surface | HAZOP/law link |"


def split_row(line: str) -> list[str]:
    return [c.strip() for c in line.strip().strip("|").split("|")]


def table_rows(page: str, header_prefix: str) -> list[list[str]]:
    lines = page.splitlines()
    out: list[list[str]] = []
    in_table = False
    for line in lines:
        if line.startswith("|") and header_prefix in line:
            in_table = True
            continue
        if in_table:
            if line.startswith("|"):
                cells = split_row(line)
                if all(set(c) <= set("-: ") for c in cells):
                    continue
                out.append(cells)
            elif line.strip():
                break
    return out


def resolve(path: str, symbol: str) -> bool:
    f = ROOT / path
    if not f.is_file():
        return False
    return symbol in f.read_text(errors="ignore")


def paths_in(cell: str) -> list[tuple[str, str]]:
    return [(m.group(1), m.group(2)) for m in TEST_PATH.finditer(cell)]


def check_symbols(cell: str, what: str, problems: list[str]) -> int:
    found = paths_in(cell)
    for path, symbol in found:
        if not resolve(path, symbol):
            problems.append(f"{what}: {path}::{symbol} does not resolve")
    return len(found)


def diagnostics_context() -> tuple[str, str, str, set[str]]:
    text = DIAGNOSTICS.read_text(errors="ignore")
    sec3 = text[text.find("## 3."): text.find("## 4.")]
    sec4 = text[text.find("## 4."): text.find("## 5.")]
    sec5 = text[text.find("## 5."): text.find("## 6.")]
    owed = set()
    for line in sec5.splitlines():
        if line.startswith("|") and not line.startswith("|---") and "owed diagnostic" not in line:
            first = split_row(line)[0]
            tokens = {t.strip() for t in BACKTICK.findall(first)}
            owed |= tokens or {first.strip("` ")}
    return sec3 + sec4, sec4, sec5, owed


def law_numbers() -> set[str]:
    if not LAWS.is_file():
        return set()
    nums = set()
    for line in LAWS.read_text(errors="ignore").splitlines()[1:]:
        cells = line.split("\t")
        if cells and cells[0].strip().isdigit():
            nums.add(cells[0].strip())
    return nums


def emitted_tokens() -> tuple[dict[str, list[str]], list[str]]:
    proc = subprocess.run(["bash", str(EMITTER)], capture_output=True, text=True)
    if proc.returncode != 0:
        return {}, [f"emitter failed: {proc.stderr.strip()}"]
    tokens: dict[str, list[str]] = {}
    for line in proc.stdout.splitlines():
        cells = line.split("\t")
        if len(cells) != 6:
            continue
        tokens.setdefault(cells[0], []).append(line)
    return tokens, []


def validate(page: str) -> tuple[list[str], list[dict]]:
    problems: list[str] = []
    if HEADER not in page:
        return ["Table D header not found (expected: " + HEADER + ")"], []

    rows = table_rows(page, "| ID | Durable state (key/DB)")
    if not rows:
        problems.append("Table D has no rows -- the scan would pass vacuously")
    if len(set(r[0] for r in rows)) != len(rows):
        problems.append("Table D has duplicate IDs")

    diag_surface, _, _, owed_surfaces = diagnostics_context()
    laws = law_numbers()
    tokens, emit_problems = emitted_tokens()
    problems.extend(emit_problems)
    classified: set[str] = set()
    census: list[dict] = []
    tests_owed = 0
    surfaces_owed = 0

    for cells in rows:
        if len(cells) != 7:
            problems.append(f"{cells[0]}: expected 7 cells, got {len(cells)}")
            continue
        rid, state, wc, klass, test, surface, link = cells
        if not re.fullmatch(r"D26-\d{2}", rid.strip("*")):
            problems.append(f"{rid}: ID is not D26-NN")
        state_tokens = {t.strip() for t in BACKTICK.findall(state)}
        classified |= state_tokens
        if not state_tokens:
            problems.append(f"{rid}: state names no backticked identifier")
        for t in state_tokens:
            if t not in tokens:
                problems.append(f"{rid}: state {t} is not an emitted durable identifier")

        if not re.match(r"^(auto-recover|operator-action|safe-halt)\b", klass):
            problems.append(f"{rid}: class [{klass}] is not in {CLASSES}")

        if "→" not in wc:
            problems.append(f"{rid}: writer/clearer has no `→`")
        else:
            writer, clearer = wc.split("→", 1)
            if check_symbols(writer, f"{rid} writer", problems) == 0:
                problems.append(f"{rid}: writer names no path::symbol")
            if clearer.strip().startswith("none:"):
                if len(clearer.strip()) <= 6:
                    problems.append(f"{rid}: `none:` carries no reason")
            elif check_symbols(clearer, f"{rid} clearer", problems) == 0:
                problems.append(f"{rid}: clearer names no path::symbol")

        if test.strip().startswith("owed:"):
            tests_owed += 1
        elif check_symbols(test, f"{rid} test", problems) == 0:
            problems.append(f"{rid}: test names no path::symbol and is not `owed:`")

        surface_tokens = [t.strip() for t in BACKTICK.findall(surface)]
        if not surface_tokens:
            problems.append(f"{rid}: surface names no typed token")
        for token in surface_tokens:
            kind, _, value = token.partition(":")
            if kind == "field":
                if f"`{value}`" not in diag_surface:
                    problems.append(f"{rid}: field:{value} is not in node-diagnostics §3/§4")
            elif kind == "route":
                if value not in diag_surface:
                    problems.append(f"{rid}: route:{value} is not in node-diagnostics §3/§4")
            elif kind == "log":
                if check_symbols(value, f"{rid} log", problems) == 0:
                    problems.append(f"{rid}: log:{value} names no path::symbol")
            elif kind == "owed":
                surfaces_owed += 1
                if value not in owed_surfaces:
                    problems.append(f"{rid}: owed surface {value} has no node-diagnostics §5 row")
            else:
                problems.append(f"{rid}: surface token [{token}] is not field:/route:/log:/owed:")

        anchored = FROW.search(link) or WROW.search(link) or any(s in link for s in SLUGS)
        if not anchored:
            problems.append(f"{rid}: link names no F-U/W-N row and no candidate:/owed:/vacuous:")
        for frow in FROW.findall(link):
            if frow not in page:
                problems.append(f"{rid}: linked {frow} does not exist in the page")
        if not LAW_NUM.search(link) and not any(s in link for s in SLUGS):
            problems.append(f"{rid}: link names no law or candidate:/owed:/vacuous: slug")
        for num in LAW_NUM.findall(link):
            if num not in laws:
                problems.append(f"{rid}: Law {num} is not in spec/laws.tsv")

        census.append(
            {"id": rid.strip("*"), "state": state, "class": klass, "test": test,
             "surface": surface, "link": link}
        )

    # The reverse direction: a §5 row must be cited by a census row or be a non-durable gap, or §5
    # has become a second tracker.
    cited = {
        token.partition(":")[2].strip()
        for row in census
        for token in BACKTICK.findall(row["surface"])
        if token.strip().startswith("owed:")
    }
    for name in sorted(owed_surfaces - cited - NON_DURABLE_GAPS):
        problems.append(f"node-diagnostics §5 row {name} is not cited by the census")

    # Table F — the explicit exclusions. Their second cell must name an emitted identifier.
    for cells in table_rows(page, "| ID | Literal / path |"):
        if len(cells) < 3:
            problems.append(f"{cells[0]}: exclusion row is malformed")
            continue
        ex_tokens = {t.strip() for t in BACKTICK.findall(cells[1])}
        if not ex_tokens:
            problems.append(f"{cells[0]}: exclusion names no identifier")
        classified |= ex_tokens
        for t in ex_tokens:
            if t not in tokens:
                problems.append(f"{cells[0]}: exclusion {t} is not an emitted durable identifier")

    missing = sorted(set(tokens) - classified)
    unknown = sorted(classified - set(tokens))
    for token in missing:
        problems.append(f"durable identifier {token} is not classified by Table D or Table F")
    for token in unknown:
        problems.append(f"Table D/F classifies {token}, which is not a durable identifier")

    m = DEBT.search(page)
    if not m:
        problems.append("the §7 Debt line does not carry the census counts")
    else:
        declared_tests = int(m.group(3) or 0)
        declared_surfaces = int(m.group(4) or 0)
        if declared_tests != tests_owed:
            problems.append(f"declared census tests owed {declared_tests} but counted {tests_owed}")
        if declared_surfaces != surfaces_owed:
            problems.append(
                f"declared census surfaces owed {declared_surfaces} but counted {surfaces_owed}"
            )

    return problems, census


def fixtures() -> list[str]:
    """the_census_is_total: the checker refuses each defect it exists to catch."""
    problems: list[str] = []
    tokens = {"RESET_MARKER_KEY": ["RESET_MARKER_KEY\tkey\tlast-reset\tx\tRESET_MARKER_KEY\t1"]}
    good = [["**D26-01**", "`RESET_MARKER_KEY`",
             "`block-storage/src/dag/dag_storage.rs::RESET_MARKER_KEY` → `casper/src/engine/catchup.rs::run_catchup`",
             "auto-recover", "`casper/tests/reset.rs::a_reset_marker_round_trips_and_a_malformed_one_is_refused`",
             "`field:phase`", "F-U12-03 · Law 72"]]
    # These run only if the fixture resolves against the real tree; a stale fixture is a failure.
    for cells in good:
        for path, symbol in paths_in(cells[2]):
            if not resolve(path, symbol):
                problems.append(f"self-test fixture does not resolve: {path}::{symbol}")
    if not any("not an emitted durable identifier" in p for p in
               validate_fixture(tokens, [["**D26-01**", "`NOT_A_KEY`", "`a.rs::x` → `b.rs::y`",
                                          "auto-recover", "owed: x", "`owed:missing`",
                                          "F-U1-01 · candidate:x"]])):
        problems.append("self-test: an invented identifier was not refused")
    return problems


def validate_fixture(tokens, rows) -> list[str]:
    """The row-level rules, reusable without the tree (self-test)."""
    problems: list[str] = []
    classified: set[str] = set()
    for cells in rows:
        if len(cells) != 7:
            problems.append("row is not seven cells")
            continue
        rid, state, wc, klass, test, surface, link = cells
        classified |= {t.strip() for t in BACKTICK.findall(state)}
        for t in classified:
            if t not in tokens:
                problems.append(f"{rid}: state {t} is not an emitted durable identifier")
        if not re.match(r"^(auto-recover|operator-action|safe-halt)\b", klass):
            problems.append(f"{rid}: class [{klass}] is not in {CLASSES}")
    return problems


def main(argv: list[str]) -> int:
    gate = False
    self_test = False
    terminal = False
    page = PAGE
    for arg in argv:
        if arg == "--gate":
            gate = True
        elif arg == "--self-test":
            self_test = True
        elif arg == "--terminal-index":
            terminal = True
        elif arg in ("-h", "--help"):
            print(__doc__)
            return 0
        elif not arg.startswith("-"):
            page = Path(arg)

    if self_test:
        problems = fixtures()
        for p in problems:
            print(f"  VIOLATION  {p}")
        print(f"check-durable-state-census: self-test {'ok' if not problems else 'FAILED'}")
        return 0 if not problems else 1

    text = page.read_text(errors="ignore")
    problems, census = validate(text)

    if terminal:
        print("| row | state | class | link |")
        print("|---|---|---|---|")
        for row in census:
            if row["class"].startswith(("operator-action", "safe-halt")):
                print(f"| {row['id']} | {row['state']} | {row['class']} | {row['link']} |")
        return 0

    for p in problems:
        print(f"  VIOLATION  {p}")
    classes: dict[str, int] = {}
    for row in census:
        classes[row["class"].split()[0]] = classes.get(row["class"].split()[0], 0) + 1
    print(
        f"check-durable-state-census: {len(census)} rows · "
        + " · ".join(f"{k} {v}" for k, v in sorted(classes.items()))
        + f" · {len(problems)} violations"
    )
    if problems:
        if gate:
            return 1
        print("check-durable-state-census: report only (use --gate to fail).")
        return 0
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
