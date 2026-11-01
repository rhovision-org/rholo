#!/usr/bin/env python3
"""check-evidence-locality — no off-host claim may carry a loopback observer.

Why this exists. #428 M3 asks for an **off-host** observer: a routable address, a join that
receives and applies the approved fringe, and a negative control where a `127.0.0.1` observer from
another host fails clearly. The 2026-10-29 record was cited as a live positive while its observer
advertised `rnode://…@127.0.0.1` — a configured-input diagnosis on loopback. That false positive is
the class this check exists to stop, and it is a claim about a document, so it is checked here.

Contract. A run record is a directory under `spec/audit/evidence/` containing a run artefact
(`acceptance.json`, `run.json`, `result.json`, `journal*`, `observations*`, `progress-samples*`) and a
`README.md` that mentions `M3` or `off-host`. Such a record **must** declare:

    **Evidence locality.** off-host-claim — <one line>
    **Evidence locality.** loopback-diagnostic — <one line>
    **Evidence locality.** local-devnet — <one line>
    **Evidence locality.** not-a-run — <one line>

Checks:
  1  a record that mentions M3/off-host carries exactly one declared locality;
  2  `off-host-claim` carries **no** loopback advertised peer or host in any text file, and names a
     **dial-back** (the bootstrap's own probe of the advertised address) — the M3 precondition;
  3  `loopback-diagnostic` carries at least one loopback advertised peer/host and says in words that
     it is **not an M3 off-host positive** (the corrected 10-29 shape);
  4  `local-devnet` does not use the words "off-host positive";
  5  the scan is not vacuous (at least one run record was examined).

The advertised-peer patterns are deliberately narrow (`rnode://<id>@…`, `--host`, `--bootstrap-host`,
`--bootstrap`), not "any 127.0.0.1 anywhere": an off-host run legitimately reads its own API on
loopback, and a checker that forbids the string would reject a valid record.

Usage: tools/check-evidence-locality.py [--gate] [<evidence-dir>]
       --gate exits non-zero on a violation (for CI). Without it the report is printed and the
       exit is 0 so a reader can run it mid-edit.
"""
from __future__ import annotations

import ipaddress
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EVIDENCE = ROOT / "spec/audit/evidence"

RUN_ARTEFACTS = ("acceptance.json", "run.json", "result.json")
RUN_PREFIXES = ("journal", "observations", "progress-samples")
LABELS = ("off-host-claim", "loopback-diagnostic", "local-devnet", "not-a-run")
ADVERTISED = [
    re.compile(r"rnode://[0-9a-zA-Z]+@(\[[^\]]+\]|[^\s?:/\"',}]+)"),
    re.compile(r"--(?:host|bootstrap-host)[=\s]+[\"']?(\[[^\]]+\]|[^\s\"',}]+)"),
    re.compile(r"--bootstrap[=\s]+[^\s@]*@(\[[^\]]+\]|[^\s?:/\"',}]+)"),
]
LABEL_RE = re.compile(r"\*\*Evidence locality\.\*\*\s*`?([a-z-]+)`?\s*[—-]?")


def run_record(directory: Path) -> bool:
    names = [p.name for p in directory.iterdir() if p.is_file()]
    return any(n in RUN_ARTEFACTS for n in names) or any(
        n.startswith(RUN_PREFIXES) for n in names
    )


def text_files(directory: Path) -> list[Path]:
    return [
        p
        for p in directory.iterdir()
        if p.is_file() and p.suffix in {".md", ".json", ".py", ".txt", ".log", ".jsonl"}
    ]


def loopback_claims(directory: Path) -> list[str]:
    found = []
    for path in text_files(directory):
        try:
            text = path.read_text(errors="replace")
        except OSError:
            continue
        # Run records may store argv as a JSON array rather than a shell command.
        texts = [text]
        try:
            def strings(value):
                if isinstance(value, str):
                    return [value]
                if isinstance(value, list):
                    return [s for v in value for s in strings(v)]
                if isinstance(value, dict):
                    return [s for v in value.values() for s in strings(v)]
                return []
            texts.append(" ".join(strings(json.loads(text))))
        except ValueError:
            pass
        for value in texts:
            for pattern in ADVERTISED:
                for match in pattern.finditer(value):
                    if local_address(match.group(1)):
                        found.append(f"{path.name}: {match.group(0)}")
    return found


def local_address(host: str) -> bool:
    host = host.strip("[]").lower().rstrip(".")
    if host == "localhost" or host.endswith(".localhost"):
        return True
    try:
        address = ipaddress.ip_address(host)
    except ValueError:
        return False
    address = getattr(address, "ipv4_mapped", None) or address
    return address.is_loopback or address.is_unspecified


def mentions_m3(readme: str) -> bool:
    return "M3" in readme or "off-host" in readme or "off host" in readme


def check(directory: Path) -> list[str]:
    problems: list[str] = []
    readme_path = directory / "README.md"
    if not readme_path.is_file():
        return [f"{directory.name}: a run record without README.md cannot be dispositioned"]
    readme = readme_path.read_text(errors="replace")
    if not mentions_m3(readme):
        return []  # not an M3 claim; the check does not reach it
    labels = LABEL_RE.findall(readme)
    if len(labels) != 1:
        problems.append(
            f"{directory.name}: mentions M3/off-host but declares {len(labels)} localities "
            f"(one of {', '.join(LABELS)} is required)"
        )
        return problems
    label = labels[0]
    if label not in LABELS:
        problems.append(f"{directory.name}: unknown locality {label!r}")
        return problems
    loopbacks = loopback_claims(directory)
    if label == "off-host-claim":
        for claim in loopbacks:
            problems.append(f"{directory.name}: an off-host claim carries a loopback {claim}")
        if not re.search(r"dial[- ]?back", readme, re.I):
            problems.append(
                f"{directory.name}: an off-host claim must name the bootstrap's dial-back result"
            )
    elif label == "loopback-diagnostic":
        if not loopbacks:
            problems.append(
                f"{directory.name}: labelled loopback-diagnostic but no loopback advertised peer "
                "or host is present"
            )
        if "not an M3 off-host positive" not in readme:
            problems.append(
                f"{directory.name}: a loopback diagnostic must say in words that it is not an M3 "
                "off-host positive"
            )
    elif label == "local-devnet":
        if "off-host positive" in readme:
            problems.append(f"{directory.name}: a local-devnet record claims an off-host positive")
    return problems


def main() -> int:
    gate = "--gate" in sys.argv
    args = [a for a in sys.argv[1:] if not a.startswith("-")]
    base = Path(args[0]) if args else EVIDENCE
    if not base.is_dir():
        print(f"check-evidence-locality: no evidence directory at {base}", file=sys.stderr)
        return 2

    records = [d for d in sorted(base.iterdir()) if d.is_dir() and run_record(d)]
    problems: list[str] = []
    for directory in records:
        problems.extend(check(directory))
    if not records:
        problems.append("no run records were examined (the scan would be vacuous)")

    claims = sum(1 for d in records if (d / "README.md").is_file()
                 and mentions_m3((d / "README.md").read_text(errors="replace")))
    print(
        f"check-evidence-locality: {len(records)} run record(s), {claims} M3/off-host "
        f"claim(s), {len(problems)} violation(s)"
    )
    for problem in problems:
        print(f"  - {problem}")
    if gate and problems:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
