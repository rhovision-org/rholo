#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
script=tools/reconcile-network.sh
bash -n "$script"
docker info >/dev/null
id="reconcile-ci-${GITHUB_RUN_ID:-local}-$$"
vol="$id-data"; unit="$id-node"; backup_dir="$(mktemp -d)"
cleanup() { docker rm -f "$unit" >/dev/null 2>&1 || true; docker volume rm "$vol" >/dev/null 2>&1 || true; rm -rf "$backup_dir"; }
trap cleanup EXIT
docker volume create "$vol" >/dev/null
docker run --name "$unit" -v "$vol":/var/lib/rnode alpine sh -c '
mkdir -p /var/lib/rnode/blockstorage /var/lib/rnode/dagstorage /var/lib/rnode/rspace/history /var/lib/rnode/rspace/cold /var/lib/rnode/transaction /var/lib/rnode/genesis
echo block > /var/lib/rnode/blockstorage/block
echo dag > /var/lib/rnode/dagstorage/dag
echo history > /var/lib/rnode/rspace/history/history
echo cold > /var/lib/rnode/rspace/cold/cold
echo transaction > /var/lib/rnode/transaction/tx
echo genesis > /var/lib/rnode/genesis/config
echo identity > /var/lib/rnode/node.key.pem
' >/dev/null
source <(sed -n '/^move_data_dir_aside() {/,/^declare -A HOST /p' "$script" | sed '$d')
declare -A HOST UNIT
HOST[J]=local; UNIT[J]="$unit"; CONTROL=docker; RECONCILE_BACKUP_DIR="$backup_dir"
move_data_dir_aside J "$id"
test "$(docker inspect -f '{{.Name}}' "$unit")" = "/$unit"
test "$(docker inspect -f '{{range .Mounts}}{{if eq .Destination "/var/lib/rnode"}}{{.Name}}{{end}}{{end}}' "$unit")" = "$vol"
test -s "$backup_dir/reconcile-backup-${vol}-${id}.tar"
docker run --rm -v "$vol":/data alpine sh -ec '
test ! -e /data/blockstorage
test ! -e /data/dagstorage
test ! -e /data/rspace/history
test ! -e /data/rspace/cold
test ! -e /data/transaction
test "$(cat /data/genesis/config)" = genesis
test "$(cat /data/node.key.pem)" = identity
'
docker start "$unit" >/dev/null
docker run --rm -v "$vol":/data alpine sh -c 'mkdir -p /data/blockstorage && echo retained > /data/blockstorage/block'

# **F-U16-01**: an aside that already exists is refused, never overwritten (§107's D6). The first
# move above created this exact backup; a second move with the same timestamp must stop with the
# reason, and the volume must still hold what the test just wrote.
test_backup_collision_is_refused() {
  if move_data_dir_aside J "$id" 2>"$backup_dir/collision.err"; then
    echo 'ERROR: an existing aside was overwritten' >&2; return 1
  fi
  grep -q 'already exists — refusing to overwrite a backup' "$backup_dir/collision.err" || return 1
  docker run --rm -v "$vol":/data alpine sh -c 'test "$(cat /data/blockstorage/block)" = retained'
}
test_backup_collision_is_refused

touch "$backup_dir/not-a-directory"
RECONCILE_BACKUP_DIR="$backup_dir/not-a-directory"
if move_data_dir_aside J failure; then echo 'ERROR: backup failure was accepted' >&2; exit 1; fi
docker run --rm -v "$vol":/data alpine sh -ec 'test "$(cat /data/blockstorage/block)" = retained'
UNIT[J]="$id-missing"
if move_data_dir_aside J missing; then echo 'ERROR: missing volume was accepted' >&2; exit 1; fi
echo 'PASS: Docker lifecycle, cleanup, identity, restart, fail-closed backup'
