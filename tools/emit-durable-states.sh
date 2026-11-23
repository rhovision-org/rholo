#!/usr/bin/env bash
# emit-durable-states — the census's grep recipe, made executable.
#
# The HAZOP's durable-state census (U26) is only complete if a later pass can re-derive the set it
# classified. This script is that recipe. It emits one row per durable identifier:
#
#     token <TAB> kind <TAB> literal <TAB> path <TAB> symbol <TAB> line
#
# `token` is what a Table D state cell (or a Table F exclusion) must name: the constant's symbol for
# a named key/DB/marker, or the literal itself for a `Db::new("...")` that has no constant. `kind` is
# db, key, or marker. The scan stops at the first `#[cfg(test)]` in each file, so a test-only key
# never becomes required.
#
# Usage:
#   tools/emit-durable-states.sh                 print the TSV
#   tools/emit-durable-states.sh --check [<tsv>] diff against the committed evidence copy
#
# The default check target is
#   spec/audit/evidence/658-durable-state-census-20261123/durable-states.tsv
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
check=0
target=""
for arg in "$@"; do
  case "$arg" in
    --check) check=1 ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) target="$arg" ;;
  esac
done
target="${target:-$root/spec/audit/evidence/658-durable-state-census-20261123/durable-states.tsv}"

# The declared scan surface. A new store file is added here deliberately — the census's own
# limitation, recorded in U26's provenance.
files=(
  block-storage/src/dag/dag_storage.rs
  casper/src/dag.rs
  casper/src/storage.rs
  casper/src/block_disposition.rs
  block-storage/src/block_store.rs
  block-storage/src/approved_store.rs
  block-storage/src/audit_store.rs
  block-storage/src/checkpoint_store.rs
  rspace/src/history/roots_store.rs
  rspace/src/state/instances.rs
)

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT

emit_consts() {
  local f rel
  rel="$1"
  f="$root/$rel"
  [[ -r "$f" ]] || return 0
  # Stop at the first test module; a key defined under #[cfg(test)] is not a durable state.
  local re_bytes re_str re_u8
  re_bytes='^(pub[[:space:]]+)?const[[:space:]]+([A-Z0-9_]+):[[:space:]]*&\[u8\][[:space:]]*=[[:space:]]*b"(.*)";$'
  re_str='^(pub[[:space:]]+)?const[[:space:]]+([A-Z0-9_]+):[[:space:]]*&str[[:space:]]*=[[:space:]]*"(.*)";$'
  re_u8='^(pub[[:space:]]+)?const[[:space:]]+([A-Z0-9_]+):[[:space:]]*u8[[:space:]]*=[[:space:]]*([0-9]+);$'
  awk '/^#\[cfg\(test\)\]/{exit} {print NR "\t" $0}' "$f" \
    | while IFS=$'\t' read -r lineno line; do
        local kind="" sym="" lit=""
        if [[ "$line" =~ $re_bytes ]]; then
          kind=key; sym="${BASH_REMATCH[2]}"; lit="${BASH_REMATCH[3]}"
        elif [[ "$line" =~ $re_str ]]; then
          kind=key; sym="${BASH_REMATCH[2]}"; lit="${BASH_REMATCH[3]}"
        elif [[ "$line" =~ $re_u8 ]]; then
          kind=key; sym="${BASH_REMATCH[2]}"; lit="${BASH_REMATCH[3]}"
        else
          continue
        fi
        case "$sym" in
          *_KEY|*_MARKER|CURRENT_ROOT|*_DB|*_COMPLETE) ;;
          # A `_DB` here is a key in a non-DB store only if it names one; keep the filter explicit.
          *) continue ;;
        esac
        [[ "$sym" == *_MARKER ]] && kind=marker
        printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$sym" "$kind" "$lit" "$rel" "$sym" "$lineno" >> "$tmp"
      done
}

for rel in "${files[@]}"; do emit_consts "$rel"; done

# The DB mapping. Each `Db::new(...)` and each `Db { id: ... }` is one durable identifier; the
# constant is resolved from the same file's declarations, and a literal Db has no symbol.
mapping="$root/casper/src/storage.rs"
if [[ -r "$mapping" ]]; then
  re_literal='Db::new\("([^"]+)"\)'
  re_const='Db::new\(([A-Z0-9_]+)\)'
  re_struct='id:[[:space:]]*([A-Z0-9_]+)\.to_string\(\)'
  awk '/^pub fn rnode_db_mapping\(\)/{f=1} f{print NR "\t" $0} f && /^}$/{exit}' "$mapping" \
    | while IFS=$'\t' read -r lineno line; do
        token="" lit="" sym=""
        if [[ "$line" =~ $re_literal ]]; then
          lit="${BASH_REMATCH[1]}"; token="$lit"
        elif [[ "$line" =~ $re_const ]]; then
          sym="${BASH_REMATCH[1]}"; token="$sym"
          lit="$(grep -m1 -E "^pub const ${sym}: &str = \"[^\"]+\";" "$mapping" | sed -E 's/.*"(.*)";/\1/')"
        elif [[ "$line" =~ $re_struct ]]; then
          sym="${BASH_REMATCH[1]}"; token="$sym"
          lit="$(grep -m1 -E "^pub const ${sym}: &str = \"[^\"]+\";" "$mapping" | sed -E 's/.*"(.*)";/\1/')"
        else
          continue
        fi
        [[ -n "$lit" ]] || { echo "emit-durable-states: cannot resolve $token" >&2; exit 2; }
        printf '%s\tdb\t%s\t%s\t%s\t%s\n' "$token" "$lit" "casper/src/storage.rs" "$sym" "$lineno" >> "$tmp"
      done
fi

# One row per token: prefer the row that names a symbol (a literal `Db::new("blocks")` and the
# `BLOCKS_DB` constant are the same durable identifier). Then sort for a stable diff.
LC_ALL=C sort -u -t$'\t' -k1,1 -k4,4 "$tmp" \
  | awk -F'\t' '!seen[$1]++ || ($5 != "" && best[$1] == "") { print; seen[$1]=1; best[$1]=$5 }' \
  | LC_ALL=C sort -t$'\t' -k1,1 > "$tmp.sorted"

if (( check == 1 )); then
  if [[ ! -r "$target" ]]; then
    echo "emit-durable-states: no committed census at $target" >&2
    exit 2
  fi
  if diff -u "$target" "$tmp.sorted" > /dev/null; then
    echo "emit-durable-states: the census evidence is current ($(wc -l < "$tmp.sorted") identifiers)."
    exit 0
  fi
  echo "emit-durable-states: the durable-state set has moved; re-run and commit the new TSV." >&2
  diff -u "$target" "$tmp.sorted" >&2 || true
  exit 1
fi

cat "$tmp.sorted"
