#!/usr/bin/env bash
#
# check-docs-links.sh — the book's structural checks, in one place.
#
# There was no check of any kind on the book: a page could be absent from SUMMARY.md, a relative link
# could point at a deleted heading, and `[]()` / `**()**` could sit in a reader-facing paragraph
# indefinitely. Each of those has happened. This is the smallest check that catches the class:
#
#   1. SUMMARY is in sync: every target exists and every page is listed exactly once.
#   2. Every relative link resolves against the linking file's directory (the book and the root
#      reader-facing documents).
#   3. Every `#fragment` resolves to a heading on the target page (GitHub/mdbook slug rule).
#   4. No empty link text, empty destination, or empty parenthetical outside code.
#
# It deliberately does **not** read `<!-- counts:… -->` or `<!-- recount:… -->`: those are the
# emitters' business (`emit-lean-counts.sh`, `recount-artifact.sh`), and a link checker that parses
# generated data acquires a second, weaker implementation of the same rule.
#
# Usage:
#   tools/check-docs-links.sh            # report only; exit 0
#   tools/check-docs-links.sh --gate     # exit 1 on any failure (what CI runs)
#   tools/check-docs-links.sh --gate --allowlist PATH
#
# `--tail-report` additionally lists paragraph-final lines that look truncated. It is report-only
# because English paragraphs end in many ways; the two real cases it was written for were fixed by
# hand, and a noisy gate is one people learn to ignore.
#
# The allowlist is a tab-separated `path<TAB>link-target` pair per line; `#` comments are ignored. It
# exists for pre-existing links in Part VII that this pass does not own. A gate that scans nothing
# fails, the same rule the witness checker uses.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
gate=0
allowlist="$ROOT/docs/link-check-allow.txt"
tails=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --gate) gate=1 ;;
    --allowlist) allowlist="${2:?--allowlist needs a path}"; shift ;;
    --tail-report) tails=1 ;;
    -h|--help) sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) printf 'check-docs-links: unknown argument %s\n' "$1" >&2; exit 2 ;;
  esac
  shift
done

python3 - "$ROOT" "$gate" "$allowlist" "$tails" <<'PY'
import os, re, sys, urllib.parse

root = sys.argv[1]
gate = sys.argv[2] == "1"
allow_path = sys.argv[3]
tail_report = sys.argv[4] == "1"
docs = os.path.join(root, "docs")
src = os.path.join(docs, "src")
summary = os.path.join(src, "SUMMARY.md")

allow = set()
if os.path.exists(allow_path):
    for line in open(allow_path, encoding="utf-8"):
        line = line.rstrip("\n")
        if not line or line.startswith("#"):
            continue
        parts = line.split("\t")
        if len(parts) == 2:
            allow.add((os.path.normpath(parts[0]), parts[1]))

failures = []
def fail(where, msg):
    failures.append(f"{where}: {msg}")

# --- read a file, blanking fenced code; keep line numbers -----------------------------------------
def read(path):
    text = open(path, encoding="utf-8").read()
    out, fence = [], None
    for line in text.splitlines():
        stripped = line.lstrip()
        if stripped.startswith("```") or stripped.startswith("~~~"):
            mark = stripped[:3]
            fence = None if fence == mark else (fence or mark)
            out.append("")
            continue
        out.append("" if fence else line)
    return "\n".join(out)

def code_spans(text):
    # Inline code spans, by position. Links and parentheses *inside* them are not prose constructs;
    # deleting the span instead would turn [`x`](y) into [] (y) and invent an empty link.
    return [(m.start(), m.end()) for m in re.finditer(r"(`+)(.*?)\1", text, flags=re.S)]

def in_code(pos, spans):
    return any(a <= pos < b for a, b in spans)

def slug(text):
    text = re.sub(r"<[^>]+>", "", text)
    text = re.sub(r"!\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = text.replace("`", "")
    text = text.lower()
    text = re.sub(r"[^\w\s-]", "", text, flags=re.UNICODE)
    return re.sub(r"\s", "-", text)

def anchors(path):
    seen, out = {}, set()
    for line in read(path).splitlines():
        m = re.match(r"^(#{1,6})\s+(.*?)\s*$", line)
        if not m:
            continue
        explicit = re.search(r"\s*\{#([A-Za-z0-9_.:-]+)\}\s*$", m.group(2))
        if explicit:
            base = explicit.group(1)
            line = line[: explicit.start()].rstrip()
        else:
            base = slug(m.group(2))
        n = seen.get(base, 0)
        seen[base] = n + 1
        out.add(base if n == 0 else f"{base}-{n}")
    return out

# --- 1. SUMMARY sync --------------------------------------------------------------------------------
pages = []
for dirpath, _, names in os.walk(src):
    for name in sorted(names):
        if name.endswith(".md") and name != "SUMMARY.md":
            pages.append(os.path.normpath(os.path.join(dirpath, name)))
pages_set = set(pages)
if not pages:
    fail("docs/src", "no pages found")
listed = []
if not os.path.exists(summary):
    fail("docs/src/SUMMARY.md", "missing")
else:
    for target in re.findall(r"\]\(([^)#]+\.md)(?:#[^)]*)?\)", read(summary)):
        listed.append(os.path.normpath(os.path.join(src, urllib.parse.unquote(target))))
    for target in listed:
        if not os.path.exists(target):
            fail("docs/src/SUMMARY.md", f"target does not exist: {target}")
    dupes = {p for p in listed if listed.count(p) > 1}
    for p in sorted(dupes):
        fail("docs/src/SUMMARY.md", f"listed more than once: {os.path.relpath(p, root)}")
    for p in sorted(pages_set - set(listed)):
        fail("docs/src/SUMMARY.md", f"page not listed: {os.path.relpath(p, root)}")

# --- 2/3. links and anchors ------------------------------------------------------------------------
link_re = re.compile(r"(!?)\[([^\]]*)\]\(\s*([^)\s]+)(?:\s+\"[^\"]*\")?\s*\)")
# The reader-facing root documents are not book pages, but their links are still links. They are
# checked for resolution and anchors; they are not part of the SUMMARY-sync rule.
link_files = sorted(pages_set) + [os.path.join(root, n) for n in ("README.md", "AGENTS.md", "TERMS.md")
                                  if os.path.exists(os.path.join(root, n))]
total_links = total_anchors = 0
for path in link_files:
    rel = os.path.relpath(path, root)
    text = read(path)
    spans = code_spans(text)
    target_anchors = {}
    for m in link_re.finditer(text):
        if in_code(m.start(), spans):
            continue
        dest = m.group(3).strip()
        if dest.startswith(("http://", "https://", "mailto:", "tel:")):
            continue
        total_links += 1
        if dest.startswith("#"):
            frag = urllib.parse.unquote(dest[1:])
            target = path
        else:
            raw = dest.split("#", 1)[0]
            frag = urllib.parse.unquote(dest.split("#", 1)[1]) if "#" in dest else None
            target = os.path.normpath(os.path.join(os.path.dirname(path), urllib.parse.unquote(raw)))
        key = (os.path.normpath(rel), dest)
        if key in allow:
            continue
        if not os.path.exists(target):
            fail(rel, f"relative link does not resolve: {dest}")
            continue
        if frag:
            total_anchors += 1
            if target not in target_anchors:
                target_anchors[target] = anchors(target)
            if frag not in target_anchors[target]:
                fail(rel, f"anchor not found in {os.path.relpath(target, root)}: #{frag}")

# --- 4. empty constructs ----------------------------------------------------------------------------
# The patterns are applied outside code; `rev_authority()` and `pos!("trust", …)` are removed with the
# inline-code pass above.
empty_res = [
    (re.compile(r"\]\(\s*\)"), "empty link destination"),
    (re.compile(r"\]\(\s*#\s*\)"), "link with an empty fragment"),
    (re.compile(r"\[\]\([^)]*\)"), "empty link text"),
    (re.compile(r"\*\*\(\s*\)\*\*"), "empty parenthetical"),
    (re.compile(r"(?<![\w`])\(\s*\)(?![\w`])"), "bare empty parentheses"),
]
total_checks = len(pages_set)
for path in sorted(pages_set):
    rel = os.path.relpath(path, root)
    text = read(path)
    spans = code_spans(text)
    for rx, label in empty_res:
        for m in rx.finditer(text):
            if in_code(m.start(), spans):
                continue
            line = text[: m.start()].count("\n") + 1
            fail(f"{rel}:{line}", label)
    if tail_report:
        for i, line in enumerate(read(path).splitlines(), 1):
            if not line.strip() or line.rstrip().endswith((".", "!", "?", ":", ";", ")", "`", "|", ">")):
                continue
            if re.search(r"\b(see|to|the|a|an|and|or|of|for|with|in|on|by|from|that|which|is|are|was|were)$",
                         line.rstrip()):
                print(f"tail  {rel}:{i}: {line.strip()[:100]}")

if total_links == 0 or total_checks == 0:
    fail("docs/src", "scan was vacuous (no links or no empty-construct checks ran)")

if failures:
    for f in failures:
        print(f"FAIL  {f}", file=sys.stderr)
    print(f"FAIL  {len(failures)} documentation problem(s); {total_links} links, {total_anchors} anchors checked",
          file=sys.stderr)
    sys.exit(1 if gate else 0)
print(f"ok    {len(pages_set)} pages, {total_links} links, {total_anchors} anchors")
PY
