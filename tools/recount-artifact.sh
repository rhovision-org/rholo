#!/usr/bin/env bash
#
# recount-artifact.sh — one record for the figures a reader checks, one emitter, one cheap check.
#
# The drift this closes is the ordinary kind: four reader-facing pages quoted four different line
# counts for the same tree, two of them quoted different binary sizes, and the crate count was wrong in
# two more. Each number was true when written and nothing recomputed it. `emit-lean-counts.sh` closes
# exactly this class for the law register; this is the same discipline for the artifact itself.
#
# Three modes, deliberately separated:
#
#   --record      measure the tracked tree and (optionally) the release binary, write
#                 `tools/artifact-counts.json`, and rewrite the `<!-- recount:... -->` spans.
#   --check       read the committed record, rewrite the spans in a scratch copy, and refuse a diff.
#                 **It never runs git, wc, cargo or the binary**, so an ordinary code commit does not
#                 turn it red. That is the point: a push gate that fails whenever a source file changes
#                 is a gate people learn to ignore.
#   --verify-live recompute from the tree and compare with the record (nightly, not push), default 1%
#                 tolerance. It is the only mode that notices the record has aged.
#
# Usage:
#   tools/recount-artifact.sh [--print]
#   tools/recount-artifact.sh --record [--binary PATH] [--date YYYY-MM-DD] [--no-binary]
#                              [--allow-code-dirty]
#   tools/recount-artifact.sh --check
#   tools/recount-artifact.sh --verify-live [--strict] [--tolerance PERCENT] [--binary PATH]
#
# A run that finds no marker fails, the same rule `check-rust-witnesses.sh` uses: a check that scans
# nothing is not evidence.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
JSON="$ROOT/tools/artifact-counts.json"
LEGACY_REV="1b7583649"

FILES=(
  "$ROOT/README.md"
  "$ROOT/AGENTS.md"
  "$ROOT/docs/src/contributor/architecture.md"
  "$ROOT/docs/src/contributor/why-rust.md"
)

mode="print"
binary="$ROOT/target/release/rnode"
date_str="$(date -u +%F)"
use_binary=1
allow_code_dirty=0
strict=0
tolerance=1

usage() { sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'; exit 0; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --print)        mode=print ;;
    --record)       mode=record ;;
    --check)        mode=check ;;
    --verify-live)  mode=verify ;;
    --binary)       binary="${2:?--binary needs a path}"; shift ;;
    --date)         date_str="${2:?--date needs YYYY-MM-DD}"; shift ;;
    --no-binary)    use_binary=0 ;;
    --allow-code-dirty) allow_code_dirty=1 ;;
    --strict)       strict=1 ;;
    --tolerance)    tolerance="${2:?--tolerance needs a percent}"; shift ;;
    -h|--help)      usage ;;
    *) printf 'recount-artifact: unknown argument %s\n' "$1" >&2; exit 2 ;;
  esac
  shift
done

fail() { printf 'FAIL  %s\n' "$*" >&2; exit 1; }
warn() { printf 'warn  %s\n' "$*" >&2; }

commify() { printf '%s' "$1" | sed ':a;s/\B[0-9]\{3\}\>/,&/;ta'; }
mib() { awk -v b="$1" 'BEGIN { printf "%.1f", b / 1048576 }'; }
mb()  { awk -v b="$1" 'BEGIN { printf "%.1f", b / 1000000 }'; }

# --- measurement -----------------------------------------------------------------------------------
members() {
  python3 - "$ROOT/Cargo.toml" <<'PY'
import sys, tomllib
with open(sys.argv[1], "rb") as f:
    data = tomllib.load(f)
for m in data.get("workspace", {}).get("members", []):
    # Paths are workspace-relative; the tree uses crate roots today.
    print(m.strip().strip('"'))
PY
}

rs_files_all() { git -C "$ROOT" ls-files -- '*.rs' | wc -l | tr -d ' '; }
rs_lines_all() { git -C "$ROOT" ls-files -- '*.rs' | xargs wc -l | tail -1 | awk '{print $1}'; }
crate_files() { git -C "$ROOT" ls-files -- "$1/*.rs" | wc -l | tr -d ' '; }
crate_lines() { git -C "$ROOT" ls-files -- "$1/*.rs" | xargs wc -l 2>/dev/null | tail -1 | awk '{print $1+0}'; }

legacy_files() { git -C "$ROOT" ls-tree -r --name-only "$LEGACY_REV" legacy | wc -l | tr -d ' '; }
legacy_bytes() {
  git -C "$ROOT" ls-tree -r -l "$LEGACY_REV" legacy \
    | awk '{s += $4} END {printf "%d", s}'
}
# `legacy/...` and `legacy/<path>` are how the prose *names* the convention, not citations to a path;
# they are filtered so the count does not change every time a sentence about it is edited.
legacy_citations() {
  { git -C "$ROOT" grep -ohE 'legacy/[A-Za-z0-9_./-]+' -- . ':!legacy' 2>/dev/null || true; } \
    | grep -vE '/\.\.\.$|^legacy/$' || true
}
legacy_citation_paths() { legacy_citations | sort -u | wc -l | tr -d ' '; }
legacy_citation_refs() { legacy_citations | wc -l | tr -d ' '; }

code_dirty_paths() {
  git -C "$ROOT" status --porcelain --untracked-files=no -- '*.rs' ':(glob)**/*.rs' Cargo.toml Cargo.lock \
    | awk '{print $2}'
}

last_code_commit_epoch() {
  git -C "$ROOT" log -1 --format='%ct' -- '*.rs' Cargo.toml Cargo.lock 2>/dev/null || echo 0
}

collect() {
  local out="$1"
  : > "$out"
  local files lines
  files="$(rs_files_all)"; lines="$(rs_lines_all)"

  {
    printf 'tracked_rs_files\t%s\n' "$files"
    printf 'tracked_rs_lines\t%s\n' "$lines"
    printf 'members\t%s\n' "$(members | wc -l | tr -d ' ')"
    printf 'date\t%s\n' "$date_str"
    printf 'git_commit\t%s\n' "$(git -C "$ROOT" rev-parse --short HEAD)"
    printf 'git_dirty\t%s\n' "$([[ -n "$(git -C "$ROOT" status --porcelain --untracked-files=no)" ]] && echo true || echo false)"
    printf 'code_dirty\t%s\n' "$([[ -n "$(code_dirty_paths)" ]] && echo true || echo false)"
  } >> "$out"

  local crate cf cl total_files=0 total_lines=0
  while IFS= read -r crate; do
    [[ -n "$crate" ]] || continue
    cf="$(crate_files "$crate")"
    cl="$(crate_lines "$crate")"
    total_files=$((total_files + cf))
    total_lines=$((total_lines + cl))
    printf 'crate.%s.files\t%s\n' "$crate" "$cf" >> "$out"
    printf 'crate.%s.lines\t%s\n' "$crate" "$cl" >> "$out"
  done < <(members)

  local wf wl
  wf=$((files - total_files))
  wl=$((lines - total_lines))
  [[ "$wf" -ge 0 && "$wl" -ge 0 ]] || fail "per-crate counts exceed the tracked total"
  printf 'workspace.files\t%s\n' "$wf" >> "$out"
  printf 'workspace.lines\t%s\n' "$wl" >> "$out"

  printf 'legacy.files\t%s\n' "$(legacy_files)" >> "$out"
  printf 'legacy.bytes\t%s\n' "$(legacy_bytes)" >> "$out"
  printf 'legacy.citation_paths\t%s\n' "$(legacy_citation_paths)" >> "$out"
  printf 'legacy.citation_refs\t%s\n' "$(legacy_citation_refs)" >> "$out"

  if [[ "$use_binary" -eq 1 ]]; then
    [[ -f "$binary" ]] || fail "no release binary at ${binary#"$ROOT"/} — build it or pass --no-binary"
    local bytes sha rustc_version fileline strip build_epoch
    bytes="$(stat -c %s "$binary")"
    sha="$(sha256sum "$binary" | awk '{print $1}')"
    rustc_version="$(rustc -Vv 2>/dev/null | sed -n '1p')"
    fileline="$(file -b "$binary" 2>/dev/null || echo unknown)"
    strip="not stripped"; [[ "$fileline" == *", stripped"* || "$fileline" == stripped* ]] && strip="stripped"
    [[ "$fileline" == unknown ]] && strip="unknown"
    build_epoch="$(stat -c %Y "$binary")"
    printf 'binary.path\t%s\n' "${binary#"$ROOT"/}" >> "$out"
    printf 'binary.bytes\t%s\n' "$bytes" >> "$out"
    printf 'binary.sha256\t%s\n' "$sha" >> "$out"
    printf 'binary.rustc\t%s\n' "$rustc_version" >> "$out"
    printf 'binary.strip\t%s\n' "$strip" >> "$out"
    printf 'binary.build_command\t%s\n' "cargo build --release -p rchain-node --bin rnode" >> "$out"
    printf 'binary.mtime\t%s\n' "$build_epoch" >> "$out"
    printf 'binary.stale\t%s\n' "$([[ "$build_epoch" -lt "$(last_code_commit_epoch)" ]] && echo true || echo false)" >> "$out"
  fi
}

# --- JSON ------------------------------------------------------------------------------------------
# The record is JSON because it is read by both this script and a human; the TSV above is only the
# transport. `--check` loads it and never touches the tree.
record_json() {
  local tsv="$1"
  python3 - "$tsv" "$JSON" <<'PY'
import json, os, sys, tempfile
tsv, out = sys.argv[1], sys.argv[2]
d = {"schema": 1, "per_crate": {}, "legacy": {}, "binary": None}
for line in open(tsv, encoding="utf-8"):
    k, v = line.rstrip("\n").split("\t", 1)
    if k.startswith("crate."):
        _, name, field = k.split(".", 2)
        d["per_crate"].setdefault(name, {})[field] = int(v)
    elif k.startswith("workspace."):
        d.setdefault("workspace", {})[k.split(".", 1)[1]] = int(v)
    elif k.startswith("legacy."):
        d["legacy"][k.split(".", 1)[1]] = int(v)
    elif k.startswith("binary."):
        if not isinstance(d.get("binary"), dict):
            d["binary"] = {}
        d["binary"][k.split(".", 1)[1]] = int(v) if v.isdigit() else v
    else:
        d[k] = int(v) if v.isdigit() else (v == "true" if v in ("true", "false") else v)
# Preserve the stale-value list; add the values this record replaces. The seeded values are the
# hand-written figures this record replaces in README/architecture/why-rust, so the next --check
# catches a reappearance even though no earlier JSON existed.
old = {}
if os.path.exists(out):
    try:
        old = json.load(open(out, encoding="utf-8"))
    except Exception:
        old = {}
stale = set(old.get("known_stale", [])) | {"203000", "474", "197000", "467", "189725", "461",
                                            "39102784", "43.6"}
for key in ("tracked_rs_files", "tracked_rs_lines"):
    if key in old and str(old[key]) != str(d.get(key)):
        stale.add(str(old[key]))
oldb = (old.get("binary") or {})
if d.get("binary") and oldb.get("bytes") not in (None, d["binary"].get("bytes")):
    stale.add(str(oldb["bytes"]))
# --no-binary records the tree but keeps the last measured binary block.
if not d.get("binary") and oldb:
    d["binary"] = oldb
d["known_stale"] = sorted(stale, key=lambda x: (-len(x), x))
tmp = out + ".tmp"
with open(tmp, "w", encoding="utf-8") as f:
    json.dump(d, f, indent=2, sort_keys=True)
    f.write("\n")
os.replace(tmp, out)
PY
}

# A `key<TAB>value` map of the marker values, formatted here and read by the rewriter.
values_from_json() {
  python3 - "$JSON" <<'PY'
import json, sys
d = json.load(open(sys.argv[1], encoding="utf-8"))

def commas(n): return f"{int(n):,}"
def mib(n): return f"{int(n)/1048576:.1f}"
def mb(n): return f"{int(n)/1000000:.1f}"

out = {
    "date": d.get("date", ""),
    "members": str(d.get("members", "")),
    "rs-files": commas(d.get("tracked_rs_files", 0)),
    "rs-lines": commas(d.get("tracked_rs_lines", 0)),
    "rs-files-raw": str(d.get("tracked_rs_files", 0)),
    "rs-lines-raw": str(d.get("tracked_rs_lines", 0)),
    "workspace-files": commas(d.get("workspace", {}).get("files", 0)),
    "workspace-lines": commas(d.get("workspace", {}).get("lines", 0)),
    "legacy-files": commas(d.get("legacy", {}).get("files", 0)),
    "legacy-bytes": commas(d.get("legacy", {}).get("bytes", 0)),
    "legacy-mib": mib(d.get("legacy", {}).get("bytes", 0)),
    "legacy-citation-paths": commas(d.get("legacy", {}).get("citation_paths", 0)),
    "legacy-citation-refs": commas(d.get("legacy", {}).get("citation_refs", 0)),
}
for name, c in (d.get("per_crate") or {}).items():
    out[f"crate:{name}-files"] = commas(c.get("files", 0))
    out[f"crate:{name}-lines"] = commas(c.get("lines", 0))
b = d.get("binary") or {}
for k, mk in (("bytes", "binary-bytes"), ("sha256", "binary-sha256"), ("rustc", "binary-rustc"),
              ("strip", "binary-strip"), ("path", "binary-path")):
    if k in b:
        out[mk] = commas(b[k]) if k == "bytes" else str(b[k])
if "bytes" in b:
    out["binary-mib"] = mib(b["bytes"])
    out["binary-mb"] = mb(b["bytes"])
for k, v in out.items():
    print(f"{k}\t{v}")
PY
}

stale_values() {
  python3 - "$JSON" <<'PY'
import json, sys
d = json.load(open(sys.argv[1], encoding="utf-8"))
for v in d.get("known_stale", []):
    print(v)
PY
}

# --- rewrite / check -------------------------------------------------------------------------------
emit() {
  local check="$1" mapfile="$2"
  local changed=0 scanned=0 f tmp
  for f in "${FILES[@]}"; do
    [[ -f "$f" ]] || continue
    if ! grep -q '<!-- recount:' "$f"; then
      warn "${f#"$ROOT"/} has no recount marker"
      continue
    fi
    scanned=$((scanned + 1))
    tmp="$(mktemp)"
    awk -v mapfile="$mapfile" '
      BEGIN { while ((getline line < mapfile) > 0) {
                i = index(line, "\t"); if (i > 0) V[substr(line,1,i-1)] = substr(line,i+1) } }
      {
        out = ""; rest = $0
        while (match(rest, /<!-- recount:[A-Za-z0-9.:-]+ -->/)) {
          key = substr(rest, RSTART, RLENGTH); gsub(/<!-- recount:| -->/, "", key)
          pre = substr(rest, 1, RSTART + RLENGTH - 1)
          tail = substr(rest, RSTART + RLENGTH)
          if (!match(tail, /<!-- recount:end -->/)) { out = out rest; rest = ""; break }
          if (!(key in V)) { out = out pre "???" "<!-- recount:end -->" }
          else { out = out pre V[key] "<!-- recount:end -->" }
          rest = substr(tail, RSTART + RLENGTH)
        }
        print out rest
      }' "$f" > "$tmp"
    # An unknown key is a refusal, not a `???` committed into a document.
    local bad
    bad="$(grep -o '<!-- recount:[A-Za-z0-9.:-]* -->' "$f" | grep -v 'recount:end' \
      | sed -e 's/<!-- recount://' -e 's/ -->//' | while IFS= read -r k; do
            awk -F'\t' -v key="$k" '$1 == key {found=1} END {exit !found}' "$mapfile" || echo "$k"
          done || true)"
    if [[ -n "$bad" ]]; then
      rm -f "$tmp"
      fail "${f#"$ROOT"/} has unknown recount key(s): $(printf '%s ' $bad)"
    fi
    if ! cmp -s "$f" "$tmp"; then
      if [[ "$check" -eq 1 ]]; then
        printf 'FAIL  %s is not what the record says — run tools/recount-artifact.sh --record:\n' \
          "${f#"$ROOT"/}" >&2
        diff "$f" "$tmp" | head -12 | sed 's/^/      /' >&2 || true
        changed=1
      else
        cp "$tmp" "$f"
        printf 'emitted  %s\n' "${f#"$ROOT"/}"
      fi
    fi
    rm -f "$tmp"
  done
  [[ "$scanned" -gt 0 ]] || fail "no file carried a recount marker"
  return "$changed"
}

scan_stale() {
  local f line buf values any=0
  values="$(stale_values | paste -sd'|' - || true)"
  [[ -n "$values" ]] || return 0
  for f in "${FILES[@]}"; do
    [[ -f "$f" ]] || continue
    local file_hit=0
    buf=""
    scan_one() {
      local p="$1"
      [[ -n "$p" ]] || return 0
      # A generated span is removed, not a licence to skip the paragraph it sits in (the C106 rule).
      p="$(printf '%s' "$p" | sed -E 's/<!-- recount:[A-Za-z0-9.:-]+ -->[^<]*<!-- recount:end -->//g')"
      local re="(^|[^A-Za-z0-9])(${values})([^A-Za-z0-9]|$)"
      [[ "$p" =~ $re ]] || return 0
      local m="${BASH_REMATCH[0]}"
      local rest="${p#*"${m}"}"
      if [[ "${rest:0:80}" =~ (lines?|files?|crates?|members?|MB|MiB) ]]; then
        printf '      %s … %s\n' "${m// /}" "${p:0:120}" >&2
        return 1
      fi
      return 0
    }
    while IFS= read -r line || [[ -n "$line" ]]; do
      if [[ -z "${line//[[:space:]]/}" ]]; then
        scan_one "$buf" || file_hit=1; buf=""
      else
        buf+="$line "
      fi
    done < "$f"
    scan_one "$buf" || file_hit=1
    if [[ "$file_hit" -eq 1 ]]; then
      printf 'FAIL  %s states a previous artifact total by hand — use a recount marker\n' \
        "${f#"$ROOT"/}" >&2
      any=1
    fi
  done
  [[ "$any" -eq 0 ]]
}

# --- modes -----------------------------------------------------------------------------------------
case "$mode" in
  print)
    tsv="$(mktemp)"; trap 'rm -f "$tsv"' EXIT
    collect "$tsv"
    printf '%-28s %s\n' "tracked .rs files" "$(awk -F'\t' '$1=="tracked_rs_files"{print $2}' "$tsv")"
    printf '%-28s %s\n' "tracked .rs lines" "$(awk -F'\t' '$1=="tracked_rs_lines"{print $2}' "$tsv")"
    printf '%-28s %s\n' "workspace members" "$(awk -F'\t' '$1=="members"{print $2}' "$tsv")"
    awk -F'\t' '$1 ~ /^crate\..*\.lines$/ {n=$1; sub(/^crate\./,"",n); sub(/\.lines$/,"",n); printf "  %-26s %s\n", n, $2}' "$tsv"
    printf '%-28s %s\n' "workspace-level lines" "$(awk -F'\t' '$1=="workspace.lines"{print $2}' "$tsv")"
    printf '%-28s %s\n' "legacy files" "$(awk -F'\t' '$1=="legacy.files"{print $2}' "$tsv")"
    printf '%-28s %s\n' "legacy bytes" "$(awk -F'\t' '$1=="legacy.bytes"{print $2}' "$tsv")"
    if awk -F'\t' '$1=="binary.bytes"{found=1} END{exit !found}' "$tsv"; then
      printf '%-28s %s bytes (%s MiB, %s MB)\n' "release binary" \
        "$(awk -F'\t' '$1=="binary.bytes"{print $2}' "$tsv")" \
        "$(mib "$(awk -F'\t' '$1=="binary.bytes"{print $2}' "$tsv")")" \
        "$(mb "$(awk -F'\t' '$1=="binary.bytes"{print $2}' "$tsv")")"
    else
      printf '%-28s %s\n' "release binary" "absent (--no-binary)"
    fi
    ;;

  record)
    tsv="$(mktemp)"; trap 'rm -f "$tsv"' EXIT
    collect "$tsv"
    if [[ "$allow_code_dirty" -eq 0 && -n "$(code_dirty_paths)" ]]; then
      fail "tracked code is modified — commit or stash it, or pass --allow-code-dirty"
    fi
    if [[ "$use_binary" -eq 1 ]] && awk -F'\t' '$1=="binary.stale" && $2=="true"{found=1} END{exit !found}' "$tsv"; then
      fail "the release binary predates the last code commit — rebuild it"
    fi
    record_json "$tsv"
    printf 'recorded %s (%s tracked .rs files, %s lines)\n' "${JSON#"$ROOT"/}" \
      "$(awk -F'\t' '$1=="tracked_rs_files"{print $2}' "$tsv")" \
      "$(awk -F'\t' '$1=="tracked_rs_lines"{print $2}' "$tsv")"
    map="$(mktemp)"; trap 'rm -f "$tsv" "$map"' EXIT
    values_from_json > "$map"
    emit 0 "$map"
    ;;

  check)
    [[ -f "$JSON" ]] || fail "no record at ${JSON#"$ROOT"/} — run --record"
    map="$(mktemp)"; trap 'rm -f "$map"' EXIT
    values_from_json > "$map"
    changed=0
    if ! emit 1 "$map"; then changed=1; fi
    if ! scan_stale "$map"; then changed=1; fi
    if [[ "$changed" -eq 0 ]]; then
      printf 'ok    artifact figures match the record (%s)\n' \
        "$(awk -F'\t' '$1=="rs-lines"{print $2}' "$map") lines"
    else
      exit 1
    fi
    ;;

  verify)
    [[ -f "$JSON" ]] || fail "no record at ${JSON#"$ROOT"/} — run --record"
    tsv="$(mktemp)"; trap 'rm -f "$tsv"' EXIT
    collect "$tsv"
    python3 - "$JSON" "$tsv" "$tolerance" "$strict" <<'PY'
import json, sys
rec = json.load(open(sys.argv[1], encoding="utf-8"))
live = {}
for line in open(sys.argv[2], encoding="utf-8"):
    k, v = line.rstrip("\n").split("\t", 1)
    live[k] = int(v) if v.isdigit() else v
tol = float(sys.argv[3]); strict = sys.argv[4] == "1"
bad = []
def cmp(label, old, new):
    if old == new: return
    if strict:
        bad.append(f"{label}: record {old}, live {new}")
    elif old and abs(new - old) / old * 100 > tol:
        bad.append(f"{label}: record {old}, live {new} ({abs(new-old)/old*100:.2f}% > {tol}%)")
cmp("tracked .rs files", rec.get("tracked_rs_files"), live.get("tracked_rs_files"))
cmp("tracked .rs lines", rec.get("tracked_rs_lines"), live.get("tracked_rs_lines"))
for name, c in (rec.get("per_crate") or {}).items():
    cmp(f"{name} lines", c.get("lines"), live.get(f"crate.{name}.lines"))
    cmp(f"{name} files", c.get("files"), live.get(f"crate.{name}.files"))
for k in ("files", "bytes", "citation_paths", "citation_refs"):
    cmp(f"legacy {k}", rec.get("legacy", {}).get(k), live.get(f"legacy.{k}"))
if "binary" in live and rec.get("binary"):
    cmp("binary bytes", rec["binary"].get("bytes"), live["binary.bytes"])
if bad:
    print("FAIL  the artifact record has aged — run tools/recount-artifact.sh --record:", file=sys.stderr)
    for b in bad: print("      " + b, file=sys.stderr)
    sys.exit(1)
print(f"ok    artifact record is current (tolerance {tol}%)")
PY
    ;;
esac
