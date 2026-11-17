#!/usr/bin/env bash
# **An identifier may be cited only if the register could hold it.**
#
# The failure this exists for: a commit subject minted a C-number, the number then appeared in code
# comments, law notes, docs and the drill as though it were a finding, and no row ever existed in
# `spec/findings.tsv`. Nothing checked, so the phantom survived and was used to explain a blocked
# test. This is that check.
#
# The rule it enforces is narrow and honest: **no citation may name an id above the highest id the
# register has ever allocated**. An id above the maximum has never been a finding, so citing it can
# only be a phantom. Ids below the maximum that are not live rows are reported as warnings, not
# failures: the register consolidates findings over time (C63, C67, C99 were closed into
# successors and the tree still cites them as history), and rewriting history is not this check's
# job.
#
# Scope: tracked files only (`git ls-files`), excluding generated output (`docs/book/`, `target/`)
# and the register's own rendering. Exit 1 on a phantom; 0 otherwise.
set -euo pipefail

cd "$(dirname "$0")/.."

python3 - "$@" <<'PY'
import csv
import re
import subprocess
import sys

with open('spec/findings.tsv', newline='') as handle:
    rows = [r for r in csv.reader(handle, delimiter='\t') if r and not r[0].startswith('#')]

def number(identifier: str) -> int:
    digits = re.sub(r'\D', '', identifier)
    return int(digits) if digits else -1

live = {row[0] for row in rows}
ceiling = max(number(i) for i in live)
pattern = re.compile(r'\bC[0-9]{2,4}\b')

tracked = subprocess.check_output(['git', 'ls-files'], text=True).splitlines()
skip_prefixes = ('docs/book/', 'target/')
skip_suffixes = ('.gz', '.tar', '.tgz', '.zip', '.pyc', '.png', '.jpg', '.jpeg', '.svg',
                 '.woff', '.ttf', '.pdf', '.bin')
text_suffixes = ('.rs', '.lean', '.md', '.sh', '.py', '.tsv', '.toml', '.yml', '.yaml', '.txt')
phantoms = {}
retired = {}

for path in tracked:
    if path.startswith(skip_prefixes) or path.endswith(skip_suffixes):
        continue
    if '/raw/' in path:          # captured logs and archives are evidence, not citations
        continue
    if not path.endswith(text_suffixes):
        continue
    if path in ('spec/findings.tsv', 'spec/AUDIT.md'):
        continue
    try:
        text = open(path, encoding='utf-8', errors='ignore').read()
    except OSError:
        continue
    for identifier in set(pattern.findall(text)):
        if identifier in live:
            continue
        if number(identifier) > ceiling:
            phantoms.setdefault(identifier, []).append(path)
        else:
            retired.setdefault(identifier, []).append(path)

verbose = '--verbose' in sys.argv
for identifier, paths in sorted(retired.items(), key=lambda item: number(item[0])):
    if verbose:
        print(f'warning: {identifier} is not a live row (retired or historical); cited in {paths[0]}'
              + (f' and {len(paths) - 1} more' if len(paths) > 1 else ''))

if phantoms:
    print(f'\nFAIL: citation(s) above the register maximum C{ceiling} — no such finding can exist:')
    for identifier, paths in sorted(phantoms.items()):
        for path in paths:
            print(f'  {identifier} in {path}')
    sys.exit(1)

print(f'ok: no C-id above the register maximum C{ceiling} is cited '
      f'({len(retired)} retired/historical id(s){" warned" if verbose else "; --verbose lists them"})')
PY

