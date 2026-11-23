#!/usr/bin/env bash
# Re-run the durable-state census (issue #658) against the checked-out tree.
set -euo pipefail
root="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$root"

echo "revision: $(git rev-parse HEAD)"
echo

echo "== the derived set =="
tools/emit-durable-states.sh | tee /tmp/durable-states.tsv
echo
tools/emit-durable-states.sh --check

echo
echo "== the census contract =="
python3 tools/check-durable-state-census.py

echo
echo "== the worksheet's own checks =="
tools/check-hazop-worksheet.sh

echo
echo "== the diagnostics page =="
python3 tools/check-node-diagnostics.py
