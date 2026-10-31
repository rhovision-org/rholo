#!/usr/bin/env bash
# reconcile-network.sh — bring a busted RChain testnet back to ONE chain, without a genesis.
#
# Derived from the reconciliation utility on PR #294 (jimscarver), with the review's five invariants and
# #287's own corrections implemented. Design and rationale: issue #287. The four-divergent-heads incident
# it exists for: spec/audit/evidence/te-1-2026-10-09-four-divergent-heads.md.
#
# What it does
#   1. **computes the meet**, and the arithmetic is over artefacts rather than over what a node says: every
#      node is asked to *attest* its own finalised block, signed by its validator key
#      (`/api/finality-attestation`, bound to a nonce this run draws), and every block the tool touches is
#      fetched as the **preimage of its hash** (`/api/block/{h}/raw`) and verified — the bytes must hash to
#      the claimed hash and the signature must verify under the claimed sender, both checked with this
#      node's own rules (`rnode verify-block` / `rnode verify-attestation`). The anchor is the **deepest
#      block a strict supermajority of stake has finalised** (`3 * stake > 2 * total`, on integers —
#      exactly two thirds is not one), which tolerates the ordinary lag between honest nodes and still
#      cannot be dragged back by a laggard or vetoed by one. Every refusal is named, and a state the tool
#      cannot prove is a state it does not act on. When no node can attest anything — a net with no
#      finality left, which is the state this tool's own recovery exists for — the operator may **name**
#      the anchor instead (`RECONCILE_ANCHOR=<hash>`), and the tool states that it is the operator's root
#      rather than a meet it computed (C269);
#   2. **proves** equivocation where the artefacts reach: two distinct blocks by one sender at one height,
#      *each fetched and verified*, is an offence anyone can re-derive — and a proven equivocator's stake
#      leaves the numerator **and** the denominator, which is the only way a pool may legitimately shrink
#      (never by silence: a threshold that drops when a validator goes quiet would let whoever can silence
#      honest nodes finalise with a minority). The proof scans a bounded window of heights around the
#      deepest attested block and prints which, so absence of a proof in it reads as a bound rather than a
#      clean bill of health. §2 still **reports** the whole chain from the block API's own `sender` field
#      — one endpoint's word, deciding nothing;
#   3. states the anchor — **which** anchor it is, computed or operator-named — with the fraction it
#      computed, the subset it counted, the nodes it left out and why, rather than implying a quorum it
#      did not compute;
#   4. enumerates what is above the point, per **unique** block — the nodes that served it, its accepted
#      deploy count, and the signatures of the deploys its merge rejected — and never drops it silently;
#   5. (`--apply`) stops every non-master node, moves each data directory aside — **never deleting** —
#      and restarts them to resync from the master's DAG;
#   5b. (`--restore-from-master`) copies the master's *chain state* onto each joiner instead, for the case
#      step 5 cannot reach — a net with no finalised fringe has nothing to resync to (C259). This is
#      **not a sync**: the joiners adopt the master's view wholesale, which is the operator asserting a
#      winner. It is a labelled stopgap, it never copies node identity, and the tool prints what it is
#      adopting and what it is dropping before it runs. **It refuses by default when the master holds a
#      block above the point** — naming the heights it would be installing, before anything is stopped —
#      and proceeds only under `RECONCILE_ADOPT_ABOVE=1`, which is the operator repeating the assertion
#      at the moment it costs something (W-N11-07, W-N11-09). An environment the master does not hold is
#      moved **aside** on the joiner rather than left standing, because a joiner's own copy of it is the
#      divergence residue this mode exists to remove (W-N11-08), and every environment adopted is printed
#      as a manifest — files, size and a digest of the file list — so the operator can compare the two
#      hosts (W-N11-04; that no *signature* attests the copy is the recorded residual);
#   6. verifies the outcome on **block hashes per height** — not on heights, where four nodes at zero
#      are equal — **and** on finality past the point, refusing (exit 9) when either is unmet.
#
# What it never does
#   * treat an unfinalised block as the truth;
#   * weigh a report it has not verified, count one validator twice (two endpoints attesting one bond is
#     a refusal — that is how a single stake is read as a supermajority), or print a stake fraction it
#     has not computed: a meet it cannot prove is refused, never approximated;
#   * shrink the denominator by silence — only a *proven* equivocation moves it;
#   * install a block above the point that only the master accepted, without the operator asking a second
#     time (`RECONCILE_ADOPT_ABOVE=1`) at the moment it is installed;
#   * touch the master's data directory;
#   * run without `--apply` — the default prints the plan and exits 0;
#   * report a height as converged (four nodes at height 0 are equal).
#
# Transport and control
#   `RECONCILE_NODES` is a file of `host api-port unit name master?` rows; the built-in table is the
#   live testnet. `host` being `local`/`127.0.0.1`/`localhost` reads the API with curl on this host
#   (which is how the devnet drill runs it); anything else is ssh'd to.
#   `RECONCILE_CONTROL` is `systemd` (default: stop/start the `unit`, whose data dir is moved with mv)
#   or `docker` (the devnet: stop/start the container, and its data hangs off a volume, so "aside" is a
#   tar on the host kept beside the volume rather than a rename).
#
# Built-in endpoints are intended for planning only; a systemd --apply or
# --restore-from-master requires RECONCILE_NODES=<reviewed live host table>.
# Always verify hosts and units immediately before a destructive operation.
#
# Usage:  RECONCILE_NODES=<file> RECONCILE_CONTROL=docker reconcile-network.sh \
#           [--apply | --restore-from-master] [--master NAME]
#   and, when no node has a finalised block to attest at all:
#         RECONCILE_ANCHOR=<block hash> ... reconcile-network.sh [--apply]
#   The hash is the block the net last agreed on (the operator's call, not this tool's). The joiners must
#   be configured with `--sync-anchor <the same hash>` for the wipe path to resync there; `--restore-from-
#   master` does not need it, because it copies the master's chain state.
#
# The meet needs a verifier: `rnode` on `PATH`, in `./target/{release,debug}/`, or at `RECONCILE_RNODE`.
# `RECONCILE_EQUIV_WINDOW` (default 16) is the height window the equivocation proof scans, counted back
# from the deepest attested block; 0 skips it and says so. `RECONCILE_MAX_WALK` (default 512) bounds the
# ancestry walk and the descent below a divergent subset — hitting either is a refusal, not a guess.

set -uo pipefail

# host  api-port  unit  name  master?
DEFAULT_NODES='164.90.140.144 40403 rnode    A master
164.90.140.144 41403 rnode-d  D
167.99.43.214 40403 rnode   B
167.99.43.214 41403 rnode-c C'

if [ -n "${RECONCILE_NODES:-}" ]; then NODES="$(cat "$RECONCILE_NODES")"; else NODES="$DEFAULT_NODES"; fi
SSH_KEY=${SSH_KEY:-$HOME/.ssh/id_droplet}
# **`-n`, because ssh otherwise eats the node table.** Every `api()` call happens inside a
# `while read … done <<< "$NODES"` loop, and ssh reads stdin by default: on the first node it consumed
# the remaining rows, the loop ended after one iteration, and section 3 then refused with "no strict
# supermajority of stake attested a finalised block" about a chain it had asked one node out of four.
# A refusal is the right response to silence, but this silence was the tool's own. `-n` reads /dev/null.
SSH="ssh -n -i $SSH_KEY -o BatchMode=yes -o ConnectTimeout=10"
CONTROL=${RECONCILE_CONTROL:-systemd}
# The ancestry walk's bound, in steps. Every step is a block this tool has verified, so the bound is
# what keeps a pathological DAG from turning a refusal into a hang — and hitting it is a refusal, with
# the step count named, not a point guessed below it (`is_ancestor_of`).
MAX_WALK=${RECONCILE_MAX_WALK:-512}
# The heights the equivocation **proof** scans, counted back from the deepest block any node attested.
# 0 skips the scan, which the tool then says out loud rather than leaving the omission silent.
EQUIV_WINDOW=${RECONCILE_EQUIV_WINDOW:-16}
# Scratch for the verified-block cache (`verified_block`). A directory rather than an array because the
# callers read through command substitutions, and a subshell's assignments do not come back.
CACHE_DIR=$(mktemp -d "${TMPDIR:-/tmp}/reconcile-cache.XXXXXX" 2>/dev/null || true)
[ -n "$CACHE_DIR" ] && trap 'rm -rf "$CACHE_DIR"' EXIT
APPLY=0; MASTER=A; RESTORE=0
while [ $# -gt 0 ]; do case "$1" in
  --apply) APPLY=1;;
  --restore-from-master) RESTORE=1; APPLY=1;;
  --master) MASTER=$2; shift;;
esac; shift; done

# Never allow service stops or data-directory moves based on a compiled-in
# address inventory. A published testnet host may be replaced while this
# script remains at an older revision. Planning uses the documented defaults;
# mutations require a separately inspected, operator-provided inventory.
# The controller's operation branches treat every non-Docker value as
# systemd. Reject unrecognised values before the live-target guard, so a
# typo cannot bypass it while still reaching the systemd stop/move paths.
case "$CONTROL" in
  docker|systemd) ;;
  *) echo "REFUSING: unsupported RECONCILE_CONTROL=$CONTROL" >&2; exit 2 ;;
esac

if [ "$APPLY" = "1" ] && [ "$CONTROL" = "systemd" ] && [ -z "${RECONCILE_NODES:-}" ]; then
  echo "REFUSING: live apply requires RECONCILE_NODES pointing to the verified current host table." >&2
  echo "          Built-in endpoints are for planning only; no node has been stopped." >&2
  exit 2
fi

# Invalid or unreadable inventories must never silently fall back to a
# built-in table when a service controller is about to be used.
if [ -n "${RECONCILE_NODES:-}" ]; then
  if [ ! -f "$RECONCILE_NODES" ] || [ ! -r "$RECONCILE_NODES" ] || [ -z "$NODES" ]; then
    echo "REFUSING: RECONCILE_NODES is missing, unreadable, or empty: $RECONCILE_NODES" >&2
    exit 2
  fi
fi

is_local() { case "$1" in local|127.0.0.1|localhost) return 0;; esac; return 1; }

api() { # host port path
  if is_local "$1"; then
    curl -s --max-time 15 "http://127.0.0.1:$2$3" 2>/dev/null
  else
    $SSH "root@$1" "curl -s --max-time 15 http://127.0.0.1:$2$3" 2>/dev/null
  fi
}

# Every block a node holds at one height, one line per block:
#   "<blockHash> <sender> <postStateHash> <deployCount> <rejectedDeploys>"
# A height legitimately holds one block per bonded validator, so this returns *all* of them — which is
# what makes the equivocation check possible from the API alone, with no node-internal surface.
#
# **The last field is a compact JSON array and is never empty** (`[]` when a block rejected nothing),
# so the line has a fixed field count with nothing that can vanish. `read` splits on *runs* of
# whitespace with the default IFS, so an absent field would shift every field after it — the shape a
# reader here must not have. The rejected deploys are base16 deploy **signatures** (the deploy id,
# `casper/src/merging.rs`: `deploy_id: d.deploy.sig`), so no element of the array can carry a space.
#
# The bond map was a sixth field until the stake-weighted meet was removed (#305). Nothing reads it
# now — the meet and the `stake_of` helper that consumed it went together — and a field that only
# looks like a denominator is worse than no field: this tool has **no** stake arithmetic (C261).
blocks_at() { # host port height
  # Served from the per-node range fetch when there is one: the parse below is unchanged, so a cached
  # answer and a live one are the same answer. Falls back to a single-height request when the range was
  # not cached (a node that refused it, or `RECONCILE_NO_CACHE=1`).
  local range_file="${CACHE_DIR:-}/blocks_${1}_${2}.json"
  if [ -z "${RECONCILE_NO_CACHE:-}" ] && [ -n "${CACHE_DIR:-}" ] && [ -s "$range_file" ]; then
    cat "$range_file"
  else
    api "$1" "$2" "/api/blocks/$3/$3"
  fi | python3 -c "
import json,sys
h=int('$3')
try:
    b=json.load(sys.stdin)
    b=b if isinstance(b,list) else []
except Exception:
    sys.exit(0)
for x in b:
    if not isinstance(x,dict): continue
    if x.get('blockNumber') != h: continue
    rejected=json.dumps([str(d) for d in (x.get('rejectedDeploys') or [])], separators=(',',':'))
    print('%s %s %s %s %s' % (x.get('blockHash',''), x.get('sender',''),
                              x.get('postStateHash',''), x.get('deployCount',0), rejected))"
}

# A range is a snapshot of one node's block inventory, not immutable data for this endpoint.
# Refresh it after recovery and on every verification round: blocks already present do not change,
# but the set a recovering node holds does. Failed/invalid refreshes discard the previous snapshot
# and use live per-height requests instead of presenting stale or partial data as a current answer.
# **The API refuses a wide range.** `/api/blocks/{start}/{end}` answers
#   "Your request startBlockNumber 0 and endBlockNumber 968 exceed the max limit 50"
# so a single request for `0..MAXH` only ever worked on a chain of fifty blocks or fewer. On anything
# real it failed, printed the fallback NOTE, and issued one ssh per height for every height — the cost
# the range cache exists to remove, restored silently. Walk the range in windows instead and merge the
# arrays here, so `blocks_at` still reads one list. Any failed window discards the whole snapshot: a
# partial inventory would look valid and answer for heights it never fetched.
RANGE_CHUNK=${RECONCILE_RANGE_CHUNK:-50}
# The server caps the *inclusive* height span at 50. An invalid override must
# fail before querying or applying a reconciliation plan, not create an
# unbounded loop (0/negative) or silently fall back to slow live reads (>50).
if ! [[ "$RANGE_CHUNK" =~ ^([1-9]|[1-4][0-9]|50)$ ]]; then
  echo "ERROR: RECONCILE_RANGE_CHUNK must be an integer from 1 through 50" >&2
  exit 2
fi
refresh_block_ranges() { # inclusive upper height for this phase
  [ -z "${RECONCILE_NO_CACHE:-}" ] && [ -n "${CACHE_DIR:-}" ] || return 0
  local upper="$1" n range_file pending lo hi ok
  for n in "${NAMES[@]}"; do
    range_file="$CACHE_DIR/blocks_${HOST[$n]}_${PORT[$n]}.json"
    pending="$range_file.pending"
    rm -f -- "$range_file" "$pending" "$pending.merged"
    : > "$pending"
    ok=1
    lo=0
    while [ "$lo" -le "$upper" ]; do
      hi=$(( lo + RANGE_CHUNK - 1 ))
      [ "$hi" -gt "$upper" ] && hi=$upper
      if ! api "${HOST[$n]}" "${PORT[$n]}" "/api/blocks/$lo/$hi" >> "$pending" 2>/dev/null; then
        ok=0
        break
      fi
      lo=$(( hi + 1 ))
    done
    if [ "$ok" = "1" ] && python3 -c '
import json,sys
# Windows are ordered, inclusive and bounded to 50. An empty, out-of-window,
# or missing response must never publish a partial but apparently valid cache.
upper, chunk = (int(x) for x in sys.argv[1:])
raw=sys.stdin.read()
data=[]; dec=json.JSONDecoder(); i=0
try:
    for lo in range(0, upper+1, chunk):
        hi=min(lo+chunk-1, upper)
        while i < len(raw) and raw[i].isspace(): i += 1
        part, i = dec.raw_decode(raw, i)
        if not isinstance(part, list) or not part: raise ValueError("empty or non-list window")
        if any(not isinstance(b, dict) or type(b.get("blockNumber")) is not int
               for b in part):
            raise ValueError("invalid block element")
        # Require an in-window witness; some API/fixture responses also include
        # adjacent heights, which blocks_at filters by requested height.
        if not any(lo <= b["blockNumber"] <= hi for b in part):
            raise ValueError("no blocks for requested window")
        data.extend(part)
    if raw[i:].strip(): raise ValueError("unexpected extra window")
except (ValueError,TypeError):
    sys.exit(1)
# Emit ONE document, not the concatenation: `blocks_at` reads the cache with json.load.
sys.stdout.write(json.dumps(data))
' "$upper" "$RANGE_CHUNK" < "$pending" > "$pending.merged"; then
      mv -- "$pending.merged" "$range_file"
      rm -f -- "$pending"
    else
      rm -f -- "$pending" "$pending.merged"
      echo "  NOTE: range snapshot unavailable or invalid for ${HOST[$n]}:${PORT[$n]}; using live per-height requests" >&2
    fi
  done
  return 0
}
# The last-finalised endpoint answers with an *error string* rather than an object when there is no
# finalised fringe yet (`"Finalized fringe is not available."`) — which is the state a diverged net is in,
# so this refuses to assume an object.
json_field() { # expr over `b`, evaluated with `b` the block object (or {})
  python3 -c "
import json,sys
try:
    d=json.load(sys.stdin)
    b=(d.get('blockInfo') or d.get('block') or d) if isinstance(d,dict) else {}
    print($1)
except Exception: print('')"
}
lfb_num()  { api "$1" "$2" /api/last-finalized-block | json_field "b.get('blockNumber','')"; }
lfb_hash() { api "$1" "$2" /api/last-finalized-block | json_field "b.get('blockHash','')"; }
height_of() { api "$1" "$2" /api/status | json_field "d.get('latestBlockNumber','')"; }
# The height of a named block, read from the block itself — `/api/block/{hash}` answers with its own
# `blockInfo`, which carries the number, so an operator-named anchor needs no height from the operator.
anchor_height() { api "$1" "$2" "/api/block/$3" | json_field "b.get('blockNumber','')"; }


# --- A: the chain-state environments, by `casper/src/storage.rs::rnode_db_mapping` ---------------
#
# **Copied**: `blockstorage` (block bodies), `dagstorage` (block metadata, the fringe records, the
# approved store, the deploy and deployer indices, and the two merge caches), `rspace/history` and
# `rspace/cold` (the on-chain tuple space), and `transaction`.
#
# **Not copied, each for its own reason**: `deploypoolstorage` (this node's pending deploy pool — not
# the network's; owners re-submit), `reporting` (a local trace cache), `eval/history` and `eval/cold`
# (the *off-chain* evaluator's space, not consensus), `gateway` (its own comment says "node-local, never
# consensus state").
#
# **And never the identity.** `node.key.pem`, `node.certificate.pem` and the validator key are the
# node's own and are not in this list. A joiner holding the survivor's key *is* an equivocator, and an
# agreement check cannot see it — the two look identical until they later sign conflicting blocks.
CHAIN_ENVS="blockstorage dagstorage rspace/history rspace/cold transaction"

# The named volume a container keeps its shard data on.
data_volume() {
  docker inspect -f '{{range .Mounts}}{{if eq .Destination "/var/lib/rnode"}}{{.Name}}{{end}}{{end}}' "$1" 2>/dev/null
}

# **The stopgap, and it says so.** This is not a sync: it copies the survivor's chain state onto each
# joiner, so what the joiners agree about afterwards is the *survivor's view*, including the blocks above
# the agreed anchor that only it accepted. The operator is asserting a winner, which is why `--apply` prints the
# blocks being adopted and the blocks being dropped before it runs. It exists because on a net with no
# finalised fringe there is nothing to sync *to* (see section 3's anchor and `--sync-anchor`), and it is
# marked a stopgap because it depends on the data-dir layout being movable — the thing #287's design
# deliberately avoided depending on.
restore_chain_state_from_master() {
  local n="$1" mvol vol mdir dir staging
  if [ "$CONTROL" = "docker" ]; then
    # `$MASTER` is the node's *name*; the container that holds its volume is `UNIT[$MASTER]`. Passing the
    # name is the bug that made this report "no data volume on A" while the volume sat there — found by
    # running it, and worth the sentence because the two are easy to confuse in this script.
    mvol="$(data_volume "${UNIT[$MASTER]}")"; vol="$(data_volume "${UNIT[$n]}")"
    [ -z "$mvol" ] && { echo "    WARNING: no data volume on ${UNIT[$MASTER]} — nothing copied" >&2; return 1; }
    [ -z "$vol" ] && { echo "    WARNING: no data volume on ${UNIT[$n]} — nothing copied" >&2; return 1; }
    docker run --rm -v "$mvol":/src:ro -v "$vol":/dst alpine sh -c "
      set -e
      copied=''; skipped=''; orphaned=''
      for d in $CHAIN_ENVS; do
        # **A source that is not there is skipped, not fatal — and not a licence to keep the joiner's
        # own.** LMDB creates an environment only when something writes to it, so a directory the
        # mapping names (`transaction` on this build) can simply not exist on the survivor; under
        # \`set -e\` a failed \`cp\` aborted the whole copy, which is how the first run reported a failure
        # while the store was fine. But a joiner holding that environment when the survivor does not is
        # the divergence residue this mode exists to remove (W-N11-08), so it is moved aside rather than
        # left standing — aside, never gone, the rule the rest of this tool keeps.
        if [ ! -e \"/src/\$d\" ]; then
          skipped=\"\$skipped \$d\"
          if [ -e \"/dst/\$d\" ]; then
            rm -rf \"/dst/\$d.reconcile-orphan\"
            mv \"/dst/\$d\" \"/dst/\$d.reconcile-orphan\"
            orphaned=\"\$orphaned \$d\"
          fi
          continue
        fi
        # **Copy beside, then swap.** Removing the destination first and *then* copying means a failed
        # copy leaves the joiner with nothing — the opposite of the 'aside, never gone' rule this tool
        # holds itself to everywhere else. So the new copy lands at a staging name and the old one is
        # only removed once the copy is complete.
        mkdir -p \"/dst/\$(dirname \"\$d\")\"
        rm -rf \"/dst/\$d.reconcile-new\"
        cp -a \"/src/\$d\" \"/dst/\$d.reconcile-new\"
        rm -rf \"/dst/\$d\"
        mv \"/dst/\$d.reconcile-new\" \"/dst/\$d\"
        copied=\"\$copied \$d\"
      done
      # A copied LMDB lock file is a stale lock: the joiner would wait on a lock nobody holds.
      find /dst -name 'lock.mdb' -delete
      echo \"    copied:\$copied\${skipped:+; not present on the survivor:\$skipped}\${orphaned:+; moved aside here:\$orphaned}\" >&2
      # **What was adopted, as a manifest the operator can compare** (W-N11-04). A file list and its
      # digest, not the bytes: an LMDB environment is a handful of files (data.mdb is the size), and
      # walking the bytes of a multi-gigabyte store costs more than the copy did. The same command on
      # the survivor's volume gives the other half of the comparison; that no *signature* attests the
      # copy is the residual, and it is recorded rather than papered over.
      for d in \$copied; do
        files=\$(find \"/dst/\$d\" -type f | wc -l)
        kib=\$(du -sk \"/dst/\$d\" | cut -f1)
        digest=\$(cd /dst && find \"\$d\" -type f | sort | while read -r f; do printf '%s %s\n' \"\$f\" \"\$(stat -c %s \"\$f\")\"; done | sha256sum | cut -c1-16)
        echo \"    adopted \$d: \$files file(s), \${kib}KiB, list-digest \$digest\" >&2
      done
      true" \
      && echo "    chain state copied from ${UNIT[$MASTER]}'s volume; identity and genesis untouched" \
      || { echo "    WARNING: the copy failed — $n is unchanged" >&2; return 1; }
  else
    mdir="$($SSH "root@${HOST[$MASTER]}" "systemctl cat ${UNIT[$MASTER]} | grep -oP '(?<=--data-dir ).*?(?= |\$)' | head -1" 2>/dev/null)"
    dir="$($SSH "root@${HOST[$n]}" "systemctl cat ${UNIT[$n]} | grep -oP '(?<=--data-dir ).*?(?= |\$)' | head -1" 2>/dev/null)"
    [ -z "$mdir" ] || [ -z "$dir" ] && { echo "    WARNING: a --data-dir could not be read" >&2; return 1; }
    # **Copy beside, then swap, on this side too** (2026-10-13, §107's D6). The first form streamed the
    # tar straight into the live data directory: a failure mid-stream left the joiner partially written
    # while it printed *"unchanged"*, and a directory the survivor does not have survived the
    # "restore" — the divergence residue this mode exists to remove. The tar now lands beside the data
    # dir and each environment is swapped only once the whole copy has arrived, which is what makes
    # "unchanged" true when it is printed. tar on the survivor, through this host, so no key passes
    # between the two.
    staging="$dir.reconcile-new-$$"
    $SSH "root@${HOST[$MASTER]}" "tar -C '$mdir' -cf - $CHAIN_ENVS" 2>/dev/null \
      | $SSH "root@${HOST[$n]}" "set -e
          rm -rf '$staging' && mkdir -p '$staging' && tar -C '$staging' -xf -
          for d in $CHAIN_ENVS; do
            if [ -e '$staging'/\$d ]; then
              mkdir -p '$dir'/\$(dirname \$d)
              rm -rf '$dir'/\$d
              mv '$staging'/\$d '$dir'/\$d
            elif [ -e '$dir'/\$d ]; then
              # **The survivor does not hold it, so the joiner must not keep its own** (W-N11-08): this is
              # the divergence residue the restore exists to remove, and the version before moved only
              # what the survivor had. Aside, never gone.
              rm -rf '$dir'/\$d.reconcile-orphan
              mv '$dir'/\$d '$dir'/\$d.reconcile-orphan
              echo \"    moved aside here: \$d (not present on the survivor)\"
            fi
          done
          rm -rf '$staging'
          find '$dir' -name lock.mdb -delete
          # **What was adopted, as a manifest the operator can compare** (W-N11-04) — a file list and its
          # digest, not the bytes, for the reason the docker side states. Printed on stdout because this
          # side's stderr is the pipeline's, and a manifest nobody sees is not a manifest.
          for d in $CHAIN_ENVS; do
            [ -e '$dir'/\$d ] || continue
            files=\$(find '$dir'/\$d -type f | wc -l)
            kib=\$(du -sk '$dir'/\$d | cut -f1)
            digest=\$(cd '$dir' && find \$d -type f | sort | while read -r f; do printf '%s %s\n' \"\$f\" \"\$(stat -c %s \"\$f\")\"; done | sha256sum | cut -c1-16)
            echo \"    adopted \$d: \$files file(s), \${kib}KiB, list-digest \$digest\"
          done" 2>/dev/null \
      && echo "    chain state copied beside and swapped in from ${MASTER}; identity and genesis untouched" \
      || { echo "    WARNING: the copy failed — $n is unchanged unless the swap had begun (staging: $staging)" >&2; return 1; }
  fi
}

stop_node() {
  # **Resolve the node first, in its own statement.** `local n="$1" unit="${UNIT[$n]}"` looks right and is
  # not: the words are expanded *before* `local` runs, so the subscript uses the caller's `n` — which is
  # whatever the last loop left there. It worked by accident in one loop (whose variable and argument
  # coincided) and started the wrong container in another, found by running it.
  local n="$1" host unit
  host="${HOST[$n]}"; unit="${UNIT[$n]}"
  if [ "$CONTROL" = "docker" ]; then
    docker stop "$unit" >/dev/null 2>&1 && echo "    $unit stopped (container)" && return
    echo "    WARNING: $unit did not stop" >&2
  else
    $SSH "root@$host" "systemctl stop $unit" >/dev/null 2>&1 && echo "    $unit stopped" && return
    echo "    WARNING: $unit did not stop" >&2
  fi
  return 1
}
start_node() {
  local n="$1" host unit
  host="${HOST[$n]}"; unit="${UNIT[$n]}"
  if [ "$CONTROL" = "docker" ]; then
    docker start "$unit" >/dev/null 2>&1 && echo "    $unit started (container)" && return
  else
    $SSH "root@$host" "systemctl start $unit" >/dev/null 2>&1 && echo "    $unit started" && return
  fi
  echo "    WARNING: $unit did not start" >&2
  return 1
}
# **Aside, never gone.** systemd: the data directory is renamed in place. docker: the data lives on a
# volume, which cannot be renamed, so its whole contents are tarred to a host file named after the
# volume and the timestamp *before* anything is removed — the backup is the "aside", and the run prints
# where it is. The genesis files are a special case and are handled first, because they may live *inside*
# the data directory: a standalone node's default genesis is `<data_dir>/genesis`, and wiping the data
# dir is what destroyed the genesis inputs on the live net. On the devnet they are mounted from outside
# at `/genesis`, so the copy is a no-op there and must not read as failure.
move_data_dir_aside() {
  local n="$1" ts="$2" host unit vol backup
  host="${HOST[$n]}"; unit="${UNIT[$n]}"
  if [ "$CONTROL" = "docker" ]; then
    vol="$(docker inspect -f '{{range .Mounts}}{{if eq .Destination "/var/lib/rnode"}}{{.Name}}{{end}}{{end}}' "$unit" 2>/dev/null)"
    if [ -z "$vol" ]; then echo "    WARNING: no data volume found on $unit — refusing" >&2; return 1; fi
    backup="${RECONCILE_BACKUP_DIR:-$PWD/target}/reconcile-backup-${vol}-${ts}.tar"
    # **Never overwrite an aside** (2026-10-13, §107's D6). `TS` was to the minute, so two runs in one
    # minute named the same file and `tar cf` wrote the *second* run's — already-cleared — volume over
    # the first run's backup, which is the only copy of the state the operator still had. The name is to
    # the second now, and this refuses rather than trusting it: a collision is a reason to stop, not to
    # lose the thing the aside exists for.
    if [ -e "$backup" ]; then
      echo "    WARNING: $backup already exists — refusing to overwrite a backup" >&2; return 1
    fi
    docker run --rm -v "$vol":/data -v "$(dirname "$backup")":/backup alpine \
      tar cf "/backup/$(basename "$backup")" -C /data . >/dev/null 2>&1 \
      && echo "    $vol backed up to $backup (never deleted — this is the 'aside')" \
      || { echo "    WARNING: could not back up $vol — refusing to touch it" >&2; return 1; }
    # Preserve the stopped container and its volume so docker start can restart it.
    docker run --rm -v "$vol":/data alpine sh -c 'cd /data && rm -rf -- blockstorage dagstorage rspace/history rspace/cold transaction' >/dev/null 2>&1 \
      || { echo "    WARNING: could not clear chain state; refusing restart" >&2; return 1; }
    echo "    $vol chain state cleared; container preserved; backup at $backup"
  else
    $SSH "root@$host" "d=\$(systemctl cat $unit | grep -oP '(?<=--data-dir ).*?(?= |\$)' | head -1)
      cp -a \$d/genesis \${d}.genesis-keep-$ts 2>/dev/null && echo '    genesis inputs preserved'
      mv \$d \${d}.bak-reconcile-$ts && echo \"    \$d -> \${d}.bak-reconcile-$ts (never deleted)\"
      mkdir -p \$d && cp -a \${d}.genesis-keep-$ts/. \$d/genesis/ 2>/dev/null
      chown -R rnode:rnode \$d" 2>/dev/null
  fi
}

declare -A HOST PORT UNIT LFB LFH HGT

# Ask the master for one block. A `--no-autopropose` net that nobody deploys to produces nothing, and
# finality is a function of the blocks' justifications, so it cannot move: a converged net can *still*
# report no finality, which reads as the recovery failing when it is the measurement that is idle. §8b
# found this by measuring; §10 needs it repeatedly, because a fringe can take more than one round of new
# blocks to pass the anchor. Best-effort by design — the live net's faucet is mounted only in dev mode,
# so a refusal is a note and the wait still runs.
trigger_block() {
  # **The transport is `api()`'s, and for a while it was not.** This POSTed to
  # `http://${HOST[$MASTER]}:…`, which is the *table's* host token: for a node file that spells a local
  # node `local` — one of the three spellings the header documents — that URL does not resolve, the
  # curl fails, and the section printed "no block could be requested", which reads as a net without the
  # faucet rather than as a broken request. Measured 2026-10-10: `http://local:42403` exits 6 while
  # `http://localhost:42403` answers, and the run that should have driven the chain produced nothing.
  local body='{"address":"11112wWGeUA5qt6MpH9CantYj2UWWt4C3LP4cx8TpQmeM79dyen6Sk"}'
  if is_local "${HOST[$MASTER]}"; then
    curl -s -X POST --max-time 30 -H 'Content-Type: application/json' --data-binary "$body" \
      "http://127.0.0.1:${PORT[$MASTER]}/api/faucet" >/dev/null 2>&1
  else
    $SSH "root@${HOST[$MASTER]}" "curl -s -X POST --max-time 30 -H 'Content-Type: application/json' \
      --data-binary '$body' http://127.0.0.1:${PORT[$MASTER]}/api/faucet" >/dev/null 2>&1
  fi \
    && echo "    a block was requested (a faucet transfer on the master)" \
    || { echo "    NOTE: no block could be requested — on a net without the faucet, deploy something" >&2; return 1; }
}

SNAPSHOT_TS=$(date -u +%Y-%m-%dT%H:%M:%SZ)
NAMES=()
echo "== 1. what each node is showing =="
while read -r host port unit name master; do
  [ -z "${name:-}" ] && continue
  if [ -n "${HOST[$name]:-}" ]; then
    echo "  REFUSING: node name '$name' appears more than once in the table." >&2
    echo "            A table that repeats a name weights one validator twice and is not an audit." >&2
    exit 2
  fi
  NAMES+=("$name"); HOST[$name]=$host; PORT[$name]=$port; UNIT[$name]=$unit
  LFB[$name]=$(lfb_num "$host" "$port"); LFH[$name]=$(lfb_hash "$host" "$port")
  HGT[$name]=$(height_of "$host" "$port")
  [ "${master:-}" = "master" ] && MASTER=$name
  printf "  %-2s h=%-6s finalised=%-6s %s\n" "$name" "${HGT[$name]:-?}" "${LFB[$name]:-?}" "${LFH[$name]:0:44}"
done <<< "$NODES"

# **An audit of a subset is not an audit.** `ssh` reads stdin by default, so a transport without `-n`
# consumes the remaining rows and ends the loop after the first node — the tool then reports the
# first node's silence as the chain's ("no strict supermajority …"), which is the most dangerous
# failure mode it has. The row count is the guard that turns that from a silent one-node audit into
# a named refusal.
EXPECTED_ROWS=$(awk 'NF { n++ } END { print n + 0 }' <<< "$NODES")
if [ "${#NAMES[@]}" -ne "$EXPECTED_ROWS" ]; then
  echo "  REFUSING: read ${#NAMES[@]} of $EXPECTED_ROWS node row(s) from the table." >&2
  echo "            A transport that consumes the table's stdin (ssh without -n) ends this loop early," >&2
  echo "            and an audit of a subset is not an audit. No node was touched." >&2
  exit 2
fi
if [ "${#NAMES[@]}" -eq 0 ]; then echo "no nodes in the table — nothing to reconcile" >&2; exit 2; fi
MAXH=0; for n in "${NAMES[@]}"; do [ "${HGT[$n]:-0}" -gt "$MAXH" ] 2>/dev/null && MAXH=${HGT[$n]}; done
echo "  snapshot: MAXH=$MAXH taken at $SNAPSHOT_TS (the highest latestBlockNumber in this sample)."
echo "  Sections 2 and 4 are bounded by this snapshot; a producing chain may advance past it."

# A producing chain is legitimate. The audit is a snapshot; movement during it is reported, not fatal,
# and it does not widen what the audit above claims. §10 says when it reads above the snapshot.
report_movement() {
  local n now moved=0
  for n in "${NAMES[@]}"; do
    now=$(height_of "${HOST[$n]}" "${PORT[$n]}")
    case "$now" in ''|*[!0-9]*) continue ;; esac
    case "${HGT[$n]:-}" in ''|*[!0-9]*) continue ;; esac
    if [ "$now" -gt "${HGT[$n]}" ]; then
      moved=1
      printf '  NOTE: %s advanced %s → %s since the snapshot; the audit above covers only heights 0..%s\n' \
        "$n" "${HGT[$n]}" "$now" "$MAXH"
    fi
  done
  if [ "$moved" = "1" ]; then
    echo "  Movement during an audit is normal on a producing chain, not a failure: the plan is bounded"
    echo "  to the snapshot above, and section 10 states any range it reads above it."
  fi
  return 0
}

# **One range request per node, not one request per height.** `/api/blocks/{start}/{end}` is a range
# route (`node/src/web/http.rs`), and the two scans below ask about every height in 0..MAXH — 0..MAXH for
# the equivocation report, MEET+1..MAXH for what sits above the point. Asking per height cost an ssh
# round trip per height per node: measured 27m51s on a 223-block chain, and the cost scales with height,
# so the 3122-block chain this tool exists for would have taken most of a day. A block never changes
# after it is created, so one fetch per node answers every height, and `blocks_at` filters locally with
# the parsing it already had. `RECONCILE_NO_CACHE=1` restores the per-height path.
refresh_block_ranges "$MAXH"

# --- 2. reported equivocation -------------------------------------------------
# **A report, not a proof, and it decides nothing.** A sender is flagged when two *distinct* blocks by it
# appear at one height in the answers the endpoints gave. The `sender` field is the endpoint's word: this
# script verifies no signature and binds no endpoint to a bonded key, so the output is a suspicion for a
# human to chase, never an attributable artefact — the proof, and the slash, are the node's (#287's
# obligation 4, #290 part 1). **No arithmetic consumes it**: since 2026-10-09 there is no stake-weighted
# meet here and no denominator, so nothing is dropped or reweighted on this output. Silence, slowness and
# absence are not evidence either way, here or anywhere else in this script.
declare -A REPORTED=() REPORTED_WHY=()
EQUIV_LINES=()
echo "== 2. reported equivocation from the block API — a report, not a proof (heights 0..$MAXH) =="
for (( h=0; h<=MAXH; h++ )); do
  declare -A seen_sender_block=()
  for n in "${NAMES[@]}"; do
    while read -r bh sender _post _deploys _rejected; do
      [ -z "${bh:-}" ] && continue
      prev="${seen_sender_block[$sender]:-}"
      if [ -n "$prev" ] && [ "$prev" != "$bh" ]; then
        if [ -z "${REPORTED[$sender]:-}" ]; then
          REPORTED[$sender]=1
          REPORTED_WHY[$sender]="two distinct blocks at height $h"
          EQUIV_LINES+=("height $h: ${prev:0:12}… and ${bh:0:12}… by ${sender:0:16}…")
        fi
      fi
      seen_sender_block[$sender]="$bh"
    done < <(blocks_at "${HOST[$n]}" "${PORT[$n]}" "$h")
  done
done
if [ "${#REPORTED[@]}" -eq 0 ]; then
  echo "  none reported — every sender has at most one block per height in these endpoints' answers"
else
  for s in "${!REPORTED[@]}"; do echo "  REPORTED ${s:0:16}… — ${REPORTED_WHY[$s]}"; done
  for e in "${EQUIV_LINES[@]}"; do echo "    $e"; done
  echo "  A report only: these sender fields are the endpoints' word. §3 *proves* what it can from"
  echo "  verified blocks and removes a proven equivocator's stake from both sides of the fraction."
fi

# --- the meet's artefacts, and the verifier they are checked with -------------
#
# A meet over what nodes *say* is not a meet. An endpoint reporting its last finalised block binds no
# stake to anything (the report is not signed and need not come from a validator at all), and the block
# it names is a *summary*: `LightBlockInfo` carries the fields but not the bytes they came from, so a
# client cannot recompute the hash and every field it reads is the node's word. C261 is the register row
# for both, and this is the arithmetic that replaces the guesses.
#
# So §3 works over artefacts instead:
#
#   * every node is asked to **attest** (`GET /api/finality-attestation?nonce=…`) — its own finalised
#     block signed by its validator key over `blake2b256("rchain-finality-attestation-v1" || nonce ||
#     hash)`. The nonce is *this run's*, and it is signed, so an answer captured from an earlier run is
#     not an answer to this one;
#   * every block the tool touches is fetched as its **preimage** (`GET /api/block/{h}/raw`) and
#     hashed: bytes that do not hash to the claimed hash, or a signature that does not verify under the
#     claimed sender, is refused rather than used.
#
# Both checks are the **node's own rules**, reached through `rnode verify-attestation` / `rnode
# verify-block` (`casper::validate::block_signature`, `casper::proto_util::hash_block`,
# `casper::validator_identity::attestation_prehash`), and the attestation *is* the node's finaliser's
# own verdict — nothing here re-derives a rule the node owns. Two implementations of one rule can
# disagree (`Split`, law 51b), and a recovery tool is the last place that should be discovering it.
#
# **What this replaced, and why the old gate could not be repaired.** Until 2026-10-10 this section
# required every node to report the *identical* finalised height and hash, and refused otherwise. That
# was honest — it can only refuse, never fabricate — and unusable in the state the tool exists for: on
# a four-validator net with two validators stopped, the three survivors read finalised 102 / 97 / 110
# (`spec/audit/evidence/n-anchor-drill/frozen-finality-recovery.txt`), which is an ordinary producing
# chain, not a disagreement. Unanimity is not a property a producing chain has, so the gate refused
# exactly where recovery was needed. The meet below tolerates the lag, and cannot be dragged back or
# vetoed by it: it takes the **deepest** block a strict supermajority of **stake** has attested, and a
# node that has finalised less simply has fewer ancestors to prove.

RNODE_BIN_CACHE=""
rnode_bin() { # the verifier: $RECONCILE_RNODE, else `rnode` on PATH, else the build tree
  if [ -n "$RNODE_BIN_CACHE" ]; then printf '%s' "$RNODE_BIN_CACHE"; return 0; fi
  local candidate
  for candidate in "${RECONCILE_RNODE:-}" "$(command -v rnode 2>/dev/null || true)" \
                   ./target/release/rnode ./target/debug/rnode; do
    if [ -n "$candidate" ] && [ -x "$candidate" ]; then
      RNODE_BIN_CACHE="$candidate"; printf '%s' "$candidate"; return 0
    fi
  done
  return 1
}

# Refuse, naming the reason and what it means — the tool's whole contract is that a state it cannot
# prove is a state it does not act on. **Exit 4 is that refusal in general, not only the anchor**: it
# means the plan could not be established and nothing has been touched. §3's anchor refusals were its
# first users; §4's disagreement between two nodes about one block's own numbers is the same kind of
# answer — a precondition of the report that cannot be proved — and takes the same code.
refuse() { # reason [more lines...]
  echo "  REFUSING: $1" >&2; shift
  local line
  for line in "$@"; do echo "            $line" >&2; done
  exit 4
}

jfield() { # a python expression over `b`, the JSON on stdin
  python3 -c "
import json,sys
try:
    b=json.load(sys.stdin)
    print($1)
except Exception:
    print('')"
}

# The signed statement from one node, verified, on stdout as JSON — or a refusal. The verification
# happens here rather than at the point of use so that **no unverified hash ever enters the arithmetic**:
# `ATT_HASH` and `ATT_KEY` are read from this verdict and nowhere else.
#
# **`2>/dev/null`, deliberately, and it is not cosmetic.** `rnode`'s stderr is not empty: jemalloc is
# built with `stats_print:true` for the memory work (`.cargo/config.toml`, issue #117) and writes ~190 KB
# of allocator statistics at process exit. Merging the two streams would put that in the middle of the
# JSON this function parses — which is exactly what happened on the first live run of this meet, and why
# the verdict is on stdout alone. The verifier prints its refusal to *stdout* (`main.rs``'s error path
# `println!``s it), so nothing is lost by dropping stderr.
attestation_of() { # host port nonce name
  local ra raw verdict rc
  ra=$(rnode_bin) || refuse "no verifier: rnode not found" \
    "The meet is computed over artefacts this tool *verifies*, and the verification is the node's own" \
    "rules, so it needs the binary. Set RECONCILE_RNODE=/path/to/rnode, or build it."
  raw=$(api "$1" "$2" "/api/finality-attestation?nonce=$3")
  if [ -z "$raw" ]; then
    refuse "$4 did not answer the attestation request" \
      "The meet needs a signed statement from every node (C261). A node that does not answer cannot be" \
      "weighed, and silence is not agreement." \
      "(A node built before this route existed answers 404; recovering without it means naming the" \
      "anchor yourself: RECONCILE_ANCHOR=<hash>.)"
  fi
  verdict=$(printf '%s' "$raw" | "$ra" verify-attestation --nonce "$3" 2>/dev/null); rc=$?
  [ $rc -eq 0 ] || refuse "$4's attestation did not verify" "${verdict:-the verifier printed nothing}"
  printf '%s' "$verdict"
}

declare -A VERIFIED_JSON
# A block's preimage, verified, on stdout as JSON — memoised **on disk**, keyed by hash, because the
# ancestry walk revisits the same block from several directions and the identity of a block is its
# hash. On disk rather than in an array because most of the callers read it through `$( )`, and an
# assignment inside a command substitution happens in a subshell that is gone by the time the parent
# looks at the array — a memo an array cannot hold.
verified_block() { # host port hash
  local cache="${CACHE_DIR:+$CACHE_DIR/}$3.json" ra raw verdict rc
  if [ -n "$CACHE_DIR" ] && [ -s "$cache" ]; then cat "$cache"; return 0; fi
  ra=$(rnode_bin) || refuse "no verifier: rnode not found"
  raw=$(api "$1" "$2" "/api/block/$3/raw")
  if [ -z "$raw" ]; then
    refuse "$1:$2 did not answer for block ${3:0:16}…" \
      "The meet walks block ancestry, and every step has to be a block this tool has verified."
  fi
  verdict=$(printf '%s' "$raw" | "$ra" verify-block 2>/dev/null); rc=$?
  [ $rc -eq 0 ] || refuse "block ${3:0:16}… did not verify" "${verdict:-the verifier printed nothing}"
  [ -n "$CACHE_DIR" ] && printf '%s' "$verdict" > "$cache"
  printf '%s' "$verdict"
}

# The same, but *without* refusing: empty output and a non-zero status when the block cannot be fetched
# or does not verify. Two callers need the weak form, and both need it for the same reason — a refusal
# there would end a run over something that is not a finding: the equivocation scan (an unverifiable
# block is a *report*, not a proof, and the run continues without the exclusion) and the descent's step
# downward (a parent that cannot be read leaves the walk where it was, bounded).
verified_block_opt() { verified_block "$1" "$2" "$3" 2>/dev/null; }

# **The proven equivocations in a height window**, one `<height> <sender> <hashA> <hashB>` per line.
#
# §2 *reports* an equivocation from the block API's own `sender` field, which is the endpoint's word.
# This *proves* one the way the node's own rule does (`casper::validate::equivocation_is_proved`): two
# distinct blocks by one sender at one height, **each fetched and verified**, so the offence is
# re-derived from artefacts instead of asserted by an endpoint. The sender is taken from the verified
# bytes, not from the summary that suggested where to look.
#
# **The window is a bound and the tool says so.** Proving an equivocation needs both blocks, so the
# scan costs a pass over the heights it covers; a full-chain pass on a long net is not something an
# emergency tool should do by default. Absence of a proof *within the window* is not absence of
# equivocation, which is why every run prints the window it looked at.
# **The scan is across the endpoints, not within one of them, and the reason is a property of the
# protocol rather than a convenience.** A node refuses the *second* block a sender offers at a height
# (`insert`'s H-1 gate), so the pair that proves the offence is almost never held by one endpoint —
# each holds one of the twins. A per-node scan therefore looks precisely where the evidence is not.
# Aggregating the endpoints' answers and verifying the pair is what makes the proof reachable at all;
# it also stays honest, because both blocks are fetched and verified before anything is called proved.
proven_equivocations() { # from_height to_height
  local h n hash sender rest key a b sender_a sender_b
  declare -A FIRST_HASH FIRST_FROM SECOND_HASH SECOND_FROM
  for (( h=$1; h<=$2; h++ )); do
    for n in "${NAMES[@]}"; do
      while read -r hash sender rest; do
        [ -n "$hash" ] || continue
        key="$h $sender"
        if [ -z "${FIRST_HASH[$key]:-}" ]; then
          FIRST_HASH[$key]="$hash"; FIRST_FROM[$key]="$n"; continue
        fi
        [ "${FIRST_HASH[$key]}" = "$hash" ] && continue
        SECOND_HASH[$key]="$hash"; SECOND_FROM[$key]="$n"
      done < <(blocks_at "${HOST[$n]}" "${PORT[$n]}" "$h")
    done
  done
  for key in "${!SECOND_HASH[@]}"; do
    read -r h sender <<< "$key"
    a=$(verified_block_opt "${HOST[${FIRST_FROM[$key]}]}" "${PORT[${FIRST_FROM[$key]}]}" "${FIRST_HASH[$key]}")
    b=$(verified_block_opt "${HOST[${SECOND_FROM[$key]}]}" "${PORT[${SECOND_FROM[$key]}]}" "${SECOND_HASH[$key]}")
    [ -n "$a" ] && [ -n "$b" ] || continue
    sender_a=$(printf '%s' "$a" | jfield "b['sender']")
    sender_b=$(printf '%s' "$b" | jfield "b['sender']")
    # The two blocks have to agree that they are one sender's, verified — the summary's claim is only
    # what told us where to look.
    [ -n "$sender_a" ] && [ "$sender_a" = "$sender_b" ] || continue
    printf '%s %s %s %s\n' "$h" "$sender_a" "${FIRST_HASH[$key]}" "${SECOND_HASH[$key]}"
  done
}

# Is `cand` (at height `cand_h`) an ancestor of the block `from` on that node's chain? 0 = yes, 1 = no.
#
# Walked rather than looked up, because there is no ancestor endpoint and the walk is the *proof*: each
# step is a verified block, and its justifications are parents the block itself signed over. A descent
# that falls below the candidate's height has passed it without meeting it, which answers no.
is_ancestor_of() { # node cand_hash cand_h from_hash from_h
  local node=$1 cand=$2 cand_h=$3 cur=$4 cur_h=$5 step=0 next next_h parent ph parents verdict
  while :; do
    if [ "$cur" = "$cand" ]; then return 0; fi
    if [ "$cur_h" -lt "$cand_h" ]; then return 1; fi
    if [ "$step" -ge "$MAX_WALK" ]; then
      refuse "the ancestry walk hit its bound (RECONCILE_MAX_WALK=$MAX_WALK steps)" \
        "A chain this long without reaching ${cand:0:16}… is not a shape this tool can rule on, and" \
        "guessing a point below it is exactly what it must not do."
    fi
    verdict=$(verified_block "${HOST[$node]}" "${PORT[$node]}" "$cur") || return 1
    parents=$(printf '%s' "$verdict" | jfield "' '.join(b['justifications'])")
    next=""; next_h=""
    for parent in $parents; do
      ph=$(verified_block "${HOST[$node]}" "${PORT[$node]}" "$parent" | jfield "b['blockNumber']")
      if [ "$parent" = "$cand" ]; then return 0; fi
      # Descend by the highest parent below the current block: it is the shortest way down, and the
      # bound is on steps rather than on heights.
      if [ -n "$ph" ] && [ "$ph" -lt "$cur_h" ]; then
        if [ -z "$next_h" ] || [ "$ph" -gt "$next_h" ]; then next="$parent"; next_h="$ph"; fi
      fi
    done
    if [ -z "$next" ]; then return 1; fi
    cur="$next"; cur_h="$next_h"; step=$((step + 1))
  done
}

# --- 3. the anchor: the meet, computed over verified artefacts, or named by the operator -----------
# Every node attests what it has finalised, this tool verifies each attestation and each block it
# touches, and the anchor is the **deepest block a strict supermajority of stake has finalised**
# (`3 * stake > 2 * total`, on integers — exactly two thirds is not a supermajority). Where the net has
# no finality left at all, no node can attest anything, and the block the **operator** names is the
# answer (`RECONCILE_ANCHOR`). The two are different answers and the tool says which one it has: a
# computed meet, or a root it labels as the operator's and verifies no further (C269). What it never
# does is present the operator's choice as a computed one.
echo "== 3. the anchor =="
ANCHOR=${RECONCILE_ANCHOR:-}
MEET=""; MEET_HASH=""
if [ -n "$ANCHOR" ]; then
  # **The operator's anchor, and it is labelled as theirs.** Checked here only for the one thing this
  # tool can check without a quorum: that the master holds the block at all (an anchor nobody has is
  # nothing to restore to). Its height comes from the block itself, because §4 enumerates by height.
  MEET=$(anchor_height "${HOST[$MASTER]}" "${PORT[$MASTER]}" "$ANCHOR")
  if ! [[ "$MEET" =~ ^[0-9]+$ ]]; then
    echo "  REFUSING: RECONCILE_ANCHOR $ANCHOR could not be read from ${MASTER} (no such block, or the" >&2
    echo "            node did not answer). An anchor to restore to has to be a block a node holds." >&2
    exit 4
  fi
  MEET_HASH="$ANCHOR"
  echo "  the operator's anchor: height $MEET, hash $MEET_HASH"
  echo "  This is **your** root, not a computed meet: this tool has verified that ${MASTER} holds the block"
  echo "  and nothing else about it — not that stake finalised it, not that any other node agrees."
  echo "  What the nodes report, reported rather than resolved:"
  for n in "${NAMES[@]}"; do
    printf "    %-2s finalised %-6s %s\n" "$n" "${LFB[$n]:-none}" "${LFH[$n]:0:44}"
  done
  echo "  If they disagree, that is the state you are recovering from; the block you named is the point"
  echo "  this recovery starts at, and every block above it is validated rather than installed — in"
  echo "  --restore-from-master too, which refuses to copy unless you set RECONCILE_ADOPT_ABOVE=1 (it names"
  echo "  the blocks above the point it would be adopting, one per line, in section 4's record)."
else
  # --- what each node attests, verified ----------------------------------------------------------
  #
  # The nonce is this run's, drawn fresh: an attestation is a signature over
  # `tag || nonce || blockHash`, so a reply captured from an earlier run verifies against *that* run's
  # nonce and not this one, and `verify-attestation` is handed the nonce we asked for rather than the
  # one that came back.
  NONCE=$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')
  echo "  this run's nonce: ${NONCE:0:16}… (signed, so no earlier run's answer can be replayed into it)"
  declare -A ATT_HASH ATT_KEY ATT_H ATT_BONDS SEEN_KEY
  for n in "${NAMES[@]}"; do
    verdict=$(attestation_of "${HOST[$n]}" "${PORT[$n]}" "$NONCE" "$n"); rc=$?
    [ $rc -eq 0 ] || exit $rc
    ATT_HASH[$n]=$(printf '%s' "$verdict" | jfield "b['blockHash']")
    ATT_KEY[$n]=$(printf '%s' "$verdict" | jfield "b['sender']")
    if [ -z "${ATT_HASH[$n]}" ] || [ -z "${ATT_KEY[$n]}" ]; then
      refuse "$n's attestation verified but carried no block or no signer" \
        "A verdict without both halves is not something to weigh stake on."
    fi
    # **One validator, one weight.** Two endpoints attesting as the same validator is either a
    # duplicated entry in the node file or one validator running twice — and either way, counting the
    # bond twice is how a single stake is read as a supermajority. This is #302's class caught on the
    # artefact instead of on the total: the first meet summed a proposer's stake once per *observation*
    # of a block, so a block four nodes held counted 4× and printed "400 of 400 = 100%".
    if [ -n "${SEEN_KEY[${ATT_KEY[$n]}]:-}" ]; then
      refuse "$n and ${SEEN_KEY[${ATT_KEY[$n]}]} both attested as ${ATT_KEY[$n]:0:16}…" \
        "One bond is one weight. Two endpoints claiming it would let a single stake pass the strict" \
        "supermajority on its own, which is the arithmetic this meet exists to refuse."
    fi
    SEEN_KEY[${ATT_KEY[$n]}]="$n"
    # The block the attestation names, from its own bytes: the height, the ancestry and the bond map
    # all come from a preimage this tool hashed, not from the summary the heights route returns.
    block=$(verified_block "${HOST[$n]}" "${PORT[$n]}" "${ATT_HASH[$n]}"); rc=$?
    [ $rc -eq 0 ] || exit $rc
    ATT_H[$n]=$(printf '%s' "$block" | jfield "b['blockNumber']")
    ATT_BONDS[$n]=$(printf '%s' "$block" | jfield "json.dumps(sorted((x['validator'].lower(), x['stake']) for x in b['bonds']))")
    printf "    %-2s attests height %-5s %s by %s\n" \
      "$n" "${ATT_H[$n]}" "${ATT_HASH[$n]:0:16}…" "${ATT_KEY[$n]:0:12}…"
  done

  # --- the denominator ---------------------------------------------------------------------------
  #
  # The bond map, read off a block this tool verified, and required to be the **same** map on every
  # node. Two different maps is not a matter of arithmetic: the nodes are not agreeing about who the
  # validators are, which is state divergence wearing a quorum's clothes — the case the design says to
  # stop on rather than resolve.
  ref_bonds=""; ref_node=""
  for n in "${NAMES[@]}"; do
    if [ -z "$ref_bonds" ]; then ref_bonds="${ATT_BONDS[$n]}"; ref_node="$n"
    elif [ "$ref_bonds" != "${ATT_BONDS[$n]}" ]; then
      refuse "the nodes disagree on the bond set ($ref_node vs $n)" \
        "A denominator that differs is not a denominator. Nothing here can decide which pool is the" \
        "chain's, and choosing one would be the operator picking a winner by hand."
    fi
  done
  TOTAL=$(printf '%s' "$ref_bonds" | python3 -c "import json,sys; print(sum(s for _,s in json.load(sys.stdin)))")
  if ! [[ "$TOTAL" =~ ^[0-9]+$ ]] || [ "$TOTAL" -le 0 ]; then
    refuse "the verified bond map has no stake in it" \
      "A meet against an empty denominator is arithmetic about nothing."
  fi

  # --- proven equivocation: the validator leaves the numerator *and* the denominator --------------
  #
  # The rule the issue states ("equivocators removed from numerator and denominator **by proof**") and
  # the one #290's slash is the on-chain form of. It is the only way a pool may legitimately shrink:
  # never by silence — a threshold that drops when a validator goes quiet would let whoever can silence
  # honest nodes finalise with a minority — but by an offence anyone can re-derive from two signed
  # blocks. §2 has *reported* equivocations since #308 and nothing consumed the report; this consumes
  # one, and says which heights it looked at so the absence of a proof is read as a bound rather than
  # as a clean bill of health.
  declare -A NO_WEIGH
  deepest=0
  for n in "${NAMES[@]}"; do [ "${ATT_H[$n]}" -gt "$deepest" ] && deepest="${ATT_H[$n]}"; done
  if [ "$EQUIV_WINDOW" -gt 0 ]; then
    from=$(( deepest - EQUIV_WINDOW )); [ "$from" -lt 0 ] && from=0
    echo "  proven equivocation, over heights $from..$deepest (RECONCILE_EQUIV_WINDOW=$EQUIV_WINDOW):"
    proven=$(proven_equivocations "$from" "$deepest")
    if [ -z "$proven" ]; then
      echo "    none proved in the window — which bounds the claim, and does not make it universal"
    fi
    while read -r ph pkey pfirst psecond; do
      [ -n "$pkey" ] || continue
      pstake=$(printf '%s' "$ref_bonds" | python3 -c "
import json,sys
keys = {v.lower(): s for v, s in json.load(sys.stdin)}
print(keys.get('$pkey'.lower(), 0))")
      if [ "${pstake:-0}" -le 0 ]; then
        echo "    PROVED at height $ph: $pkey equivocated (${pfirst:0:16}… / ${psecond:0:16}…) — and holds no"
        echo "      bond in the verified map, so it was never in the fraction"
        continue
      fi
      NO_WEIGH[$pkey]=1
      TOTAL=$(( TOTAL - pstake ))
      echo "    PROVED at height $ph: $pkey equivocated — ${pfirst:0:16}… and ${psecond:0:16}…, both verified."
      echo "      Its stake ($pstake) leaves the numerator and the denominator: the pool is now $TOTAL."
    done <<< "$proven"
  else
    echo "  proven equivocation: skipped (RECONCILE_EQUIV_WINDOW=0) — no validator was removed by proof,"
    echo "  and no claim is made that none equivocated"
  fi
  if [ "$TOTAL" -le 0 ]; then
    refuse "the whole bonded pool is proven to have equivocated" \
      "With every bond removed there is no denominator left, and a fraction over nothing is not a meet."
  fi

  # Each attesting node's stake, by the key it attested with. **Counted once per validator**, keyed on
  # the key — which is the defect that deleted the first meet (#302): its predecessor summed a
  # proposer's stake once per *observation* of the block, so a block four nodes held counted 4×, and
  # it printed "100%" for one validator's 100 of 400.
  declare -A STAKE
  for n in "${NAMES[@]}"; do
    if [ -n "${NO_WEIGH[${ATT_KEY[$n]}]:-}" ]; then
      STAKE[$n]=0
      echo "    $n attests as ${ATT_KEY[$n]:0:16}…, proved to have equivocated: not weighed, either side"
      continue
    fi
    STAKE[$n]=$(printf '%s' "$ref_bonds" | python3 -c "
import json,sys
keys = {v.lower(): s for v, s in json.load(sys.stdin)}
print(keys.get('${ATT_KEY[$n]}'.lower(), 0))")
    if [ "${STAKE[$n]:-0}" -le 0 ]; then
      refuse "$n attests as ${ATT_KEY[$n]:0:16}…, which the bond set does not bond" \
        "An attestation from a key with no stake weighs nothing, and a node whose validator key is not" \
        "in the pool the chain is running on is not a node this meet can count."
    fi
  done

  # --- the meet: the deepest block a strict supermajority of stake has finalised ----------------
  #
  # Order the attesting nodes by the height of the block each finalised, deepest first, and accumulate
  # until the stake **strictly** exceeds two thirds. Adding a node can only lower the meet (the meet is
  # a common ancestor, so it is at most the shallowest member's block), so the first subset that
  # reaches the threshold is the best one — and the ones left out are the laggards, named below rather
  # than silently dropped. This is Jim's rule as the issue states it: a lagging or faulting node must
  # neither drag the network back nor veto its recovery.
  order=$(for n in "${NAMES[@]}"; do printf '%s %s\n' "${ATT_H[$n]}" "$n"; done | sort -k1,1rn | awk '{print $2}')
  ACC=0; PREFIX=(); EXCLUDED=(); last_h=""
  for n in $order; do
    # A node with no weight left is not in the subset at all — and it must not be, because the meet is
    # the *shallowest* block in the subset, so a proven equivocator's attestation at a low height would
    # otherwise decide the point. It was excluded by proof, and the proof was printed above.
    [ "${STAKE[$n]}" -le 0 ] && continue
    # Once the threshold is crossed, a node is out only if it is *below* the height that crossed it: a
    # node at the same height cannot lower the meet, and dropping it would report a laggard that is not
    # one — which is the kind of reading this tool's output must not invite.
    if [ $(( ACC * 3 )) -gt $(( TOTAL * 2 )) ] && [ "${ATT_H[$n]}" != "$last_h" ]; then
      EXCLUDED+=("$n"); continue
    fi
    PREFIX+=("$n"); ACC=$(( ACC + STAKE[$n] )); last_h="${ATT_H[$n]}"
  done
  if [ $(( ACC * 3 )) -le $(( TOTAL * 2 )) ]; then
    refuse "no strict supermajority of stake attested a finalised block ($ACC of $TOTAL)" \
      "Exactly two thirds is not a supermajority (the protocol's own rule is 3 * stake > 2 * total on" \
      "integers), and a threshold a silent validator can move is not a quorum."
  fi

  # The candidate: the **shallowest** attested block in the subset, because every member finalised it
  # only if it is on every member's chain — and each member's finalised set is the ancestry of the
  # block it attested. When it is not an ancestor of some member the two are on divergent branches, and
  # the meet is *below* it: the common ancestor is finalised by both (finality is transitive over
  # ancestry), so the walk steps the candidate down its own chain, each step verified, until it is an
  # ancestor of every member. Bounded, and a refusal at the bound rather than a point guessed below it.
  cand=""; cand_h=""; cand_node=""
  for n in "${PREFIX[@]}"; do
    if [ -z "$cand_h" ] || [ "${ATT_H[$n]}" -lt "$cand_h" ]; then
      cand="${ATT_HASH[$n]}"; cand_h="${ATT_H[$n]}"; cand_node="$n"
    fi
  done
  descend=0
  while :; do
    missing=""
    for n in "${PREFIX[@]}"; do
      [ "$n" = "$cand_node" ] && continue
      if ! is_ancestor_of "$n" "$cand" "$cand_h" "${ATT_HASH[$n]}" "${ATT_H[$n]}"; then missing="$n"; break; fi
    done
    if [ -z "$missing" ]; then break; fi
    if [ "$descend" -ge "$MAX_WALK" ]; then
      refuse "the meet's descent hit its bound (RECONCILE_MAX_WALK=$MAX_WALK blocks below height $cand_h)" \
        "$missing's chain does not meet $cand_node's within the bound, so the depth of their common" \
        "finalised ancestor is not established — and a point below the bound would be a guess."
    fi
    parents=$(verified_block "${HOST[$cand_node]}" "${PORT[$cand_node]}" "$cand" | jfield "' '.join(b['justifications'])")
    # The candidate's parents are in the verified preimage; take the *deepest* (highest) one below it,
    # which is the shortest way down. Each parent is verified before it is used, because a walk over
    # unverified links would prove nothing.
    next=""; next_h=""
    for parent in $parents; do
      ph=$(verified_block "${HOST[$cand_node]}" "${PORT[$cand_node]}" "$parent" | jfield "b['blockNumber']")
      if [ -n "$ph" ] && [ "$ph" -lt "$cand_h" ] && { [ -z "$next_h" ] || [ "$ph" -gt "$next_h" ]; }; then
        next="$parent"; next_h="$ph"
      fi
    done
    if [ -z "$next" ]; then
      refuse "the candidate at height $cand_h has no parent below it" \
        "Every block above genesis has justifications; a block without one means this walk has reached" \
        "something that is not a chain, and the meet cannot be computed from it."
    fi
    echo "    the subset's blocks do not meet at ${cand_h} — stepping down to ${next_h} (${next:0:16}…)"
    cand="$next"; cand_h="$next_h"; descend=$((descend + 1))
  done
  MEET="$cand_h"; MEET_HASH="$cand"
  echo "  the meet: height $MEET, hash $MEET_HASH"
  echo "  Written out rather than implied: the deepest block a strict supermajority of stake finalised,"
  echo "  over attestations this tool verified, with each validator's stake counted exactly once."
  printf "  stake: %s of %s = %s%% — a strict supermajority (>66.6%%, 3*stake > 2*total on integers)\n" \
    "$ACC" "$TOTAL" "$(( ACC * 100 / TOTAL ))"
  printf "  the subset: %s\n" "${PREFIX[*]}"
  if [ "${#EXCLUDED[@]}" -gt 0 ]; then
    printf "  not in the subset (lagging, not vetoing): %s\n" "${EXCLUDED[*]}"
    echo "  Each one is rewound onto the master's DAG like any other joiner; none of them was asked to"
    echo "  agree before this could proceed."
  fi
  echo "  What every node attested, for the record:"
  for n in "${NAMES[@]}"; do
    printf "    %-2s height %-5s %s\n" "$n" "${ATT_H[$n]}" "${ATT_HASH[$n]:0:44}"
  done
  if [ "$descend" -gt 0 ]; then
    echo "  The subset's attested blocks were not on one branch; the meet is $descend block(s) below the"
    echo "  shallowest of them, which is the divergence this recovery is rewinding past."
  fi
fi

# --- 4. what is above the point ----------------------------------------------
echo "== 4. what is above the point =="
echo "  The survivor is ${MASTER}'s DAG, not a chain: every validator proposes its own block each round, so"
echo "  a height legitimately holds one sibling per validator and there is no single line to extract. The"
echo "  joiners resync onto this DAG; the writers re-submit what they can see is missing."
echo "  (heights $((MEET + 1))..$MAXH; snapshot $MAXH taken at ${SNAPSHOT_TS:-unknown})"
REPORT="${RECONCILE_REPORT:-/tmp/reconcile-deploys.jsonl}"
: > "$REPORT"
total=0; ublocks=0; nobs=0
for (( h=MEET+1; h<=MAXH; h++ )); do
  # **One block is one block, however many nodes serve it.** `/api/blocks/h/h` is answered *per node*,
  # so a block held by all four comes back four times — and the version this replaces counted each
  # *observation*, inflating both the block count and the deploy total by the replication factor. That
  # is #302's double-count (the one that got the stake meet deleted) reappearing one section down, and
  # a write report an operator re-submits from has to be the block's own count, not the network's view
  # of it. The key is the block's **hash**; `obs_seen` also keys on the observing node, so a node that
  # answers with one block twice cannot inflate it either. `observers` keeps where it was seen, so the
  # dedupe is visible rather than silent.
  #
  # **And the dedupe is where a disagreement has to be caught, not hidden.** Two nodes answering for
  # one hash are answering about *one* block, so the sender, the deploy count and the rejected set are
  # fixed by that hash — a node reporting different ones for it is wrong or lying, and taking the first
  # answer would put a number into the operator's re-submission list that no node agrees with. The tool
  # refuses instead (PR #309 raised this; it is the tool's own rule everywhere else — "a state it
  # cannot prove is a state it does not act on"). Height, sender, `deployCount` and `rejectedDeploys`
  # are all checked, because all four reach the report.
  declare -A obs_seen=() obs_sender=() obs_deploys=() obs_rejected=() obs_nodes=() obs_first=()
  seen_bh=""; nblocks=0; nobs=0
  for n in "${NAMES[@]}"; do
    while read -r bh sender _post deploys rejected; do
      [ -z "${bh:-}" ] && continue
      [ -n "${obs_seen[$bh:$n]:-}" ] && continue
      obs_seen[$bh:$n]=1; nobs=$((nobs+1))
      if [ -z "${obs_nodes[$bh]:-}" ]; then
        nblocks=$((nblocks+1)); seen_bh="$seen_bh $bh"
        obs_sender[$bh]="$sender"; obs_deploys[$bh]="${deploys:-0}"
        obs_rejected[$bh]="${rejected:-[]}"; obs_nodes[$bh]="\"$n\""
        obs_first[$bh]="$n"
      else
        if [ "${obs_sender[$bh]}" != "$sender" ] \
           || [ "${obs_deploys[$bh]}" != "${deploys:-0}" ] \
           || [ "${obs_rejected[$bh]}" != "${rejected:-[]}" ]; then
          refuse "two nodes disagree about block $bh at height $h" \
            "$n reports: sender=$sender deployCount=${deploys:-0} rejectedDeploys=${rejected:-[]}" \
            "${obs_first[$bh]} reports: sender=${obs_sender[$bh]} deployCount=${obs_deploys[$bh]} rejectedDeploys=${obs_rejected[$bh]}" \
            "A block's hash fixes its content, so these cannot both be right — one node is wrong or lying, and this report is what an operator re-submits deploys from. Nothing is acted on."
        fi
        obs_nodes[$bh]="${obs_nodes[$bh]},\"$n\""
      fi
    done < <(blocks_at "${HOST[$n]}" "${PORT[$n]}" "$h")
  done
  for bh in $seen_bh; do
    total=$(( total + ${obs_deploys[$bh]:-0} ))
    printf '{"height":%s,"blockHash":"%s","sender":"%s","deployCount":%s,"observers":[%s],"rejectedDeploys":%s}\n' \
      "$h" "$bh" "${obs_sender[$bh]}" "${obs_deploys[$bh]}" "${obs_nodes[$bh]}" "${obs_rejected[$bh]}" \
      >> "$REPORT"
  done
  ublocks=$((ublocks+nblocks))
  [ "$nblocks" -gt 0 ] && printf "  #%s: %s unique block(s) from %s answer(s)\n" "$h" "$nblocks" "$nobs"
done
echo "  $total deploy(s) above the point across $ublocks unique block(s); the per-block record is $REPORT"
echo "  (a block's *bodies* are not in the heights route — /api/block/{hash} carries the terms, the heights"
echo "   route carries counts. The owners re-submit; this tool does not claim to replay them.)"

# --- movement: report nodes that advanced past the snapshot -----------------
# Before the plan/apply split, so plan-only and apply both see it and nothing has been stopped.
report_movement

if [ "$APPLY" != "1" ]; then
  echo "== plan only =="
  echo "  would: stop every non-master node, move each data directory aside (never delete), restart them to"
  echo "        resync onto ${MASTER}'s DAG, then verify block hashes per height."
  echo "  re-run with --apply to execute. ${MASTER}'s data directory is never touched."
  exit 0
fi

if [ "$RESTORE" = "1" ]; then
  echo "== 5. apply (--restore-from-master): the whole network is stopped, the survivor included =="
  # **The point is the point** (W-N11-07, W-N11-09). Section 3 named the anchor — computed, or the
  # operator's own — and section 4 enumerated what sits above it. This is where that stops being a
  # report: a copy installs ${MASTER}'s whole DAG, so every block above the point that only ${MASTER}
  # holds would be adopted by every joiner, which is the operator asserting a winner the rule never
  # picked. So the default refuses and names them, and the operator who means it says so again —
  # RECONCILE_ADOPT_ABOVE=1 — at the moment it costs something rather than in a sentence read earlier.
  # Before the stop loop, because refusing a network into a stopped state is not a refusal.
  above_blocks="$(grep -F "\"$MASTER\"" "$REPORT" 2>/dev/null || true)"
  above_n="$(printf '%s' "$above_blocks" | grep -c . || true)"
  if [ "${above_n:-0}" -gt 0 ]; then
    above_h="$(printf '%s\n' "$above_blocks" | sed -n 's/.*"height":\([0-9]*\).*/\1/p' | sort -n | uniq | tr '\n' ' ')"
    if [ "${RECONCILE_ADOPT_ABOVE:-0}" != "1" ]; then
      echo "REFUSING: ${MASTER} holds ${above_n} block(s) above the point (height ${MEET}) that it accepted" >&2
      echo "          and its peers did not. Copying would install them on every joiner — the operator" >&2
      echo "          asserting a winner the rule never picked — so nothing is stopped and nothing is copied." >&2
      echo "          heights: ${above_h:-?}" >&2
      echo "          the per-block record is ${REPORT}" >&2
      echo "          to adopt them deliberately, re-run with RECONCILE_ADOPT_ABOVE=1" >&2
      exit 9
    fi
    echo "  **adopting ${above_n} block(s) above the point** (RECONCILE_ADOPT_ABOVE=1): heights ${above_h:-?}"
    echo "  these are the blocks ${MASTER} holds and its peers do not; ${REPORT} records what they carry."
  fi
  # The survivor is stopped too, and that is not an oversight: a filesystem-level copy of a live LMDB is a
  # torn snapshot, and in this mode the copy *is* the truth the network will run on.
  for n in "${NAMES[@]}"; do echo "  $n:"; stop_node "$n" || { echo "REFUSING: node stop failed: $n" >&2; exit 6; }; done

  echo "== 6. the fiat, printed before it is applied =="
  echo "  This is NOT a sync. The joiners adopt ${MASTER}'s chain state wholesale, so what they agree about"
  echo "  afterwards is ${MASTER}'s view of the chain — including any block above the point that ${MASTER}"
  echo "  accepted and its peers did not. Section 4 enumerates the blocks this drops, one record per unique"
  echo "  block, with each block's accepted deploy count and the signatures of the deploys its merge"
  echo "  rejected; that record — not its total, which counts accepted deploys — is what the owners"
  echo "  re-submit from. Running this is the operator asserting that ${MASTER} is the chain. It is a"
  echo "  stopgap because it depends on the data-dir layout being movable — which #287's design"
  echo "  deliberately avoided depending on — and not a substitute for an agreed anchor."

  TS=$(date -u +%Y%m%d-%H%M%S)
  echo "== 7. chain state copied onto each joiner (identity and genesis untouched) =="
  for n in "${NAMES[@]}"; do
    [ "$n" = "$MASTER" ] && continue
    echo "  $n:"
    restore_chain_state_from_master "$n" || { echo "REFUSING: state copy failed: $n" >&2; exit 7; }
  done

  echo "== 8. restarted: the survivor first, then the joiners =="
  start_node "$MASTER" || { echo "REFUSING: master start failed" >&2; exit 8; }; sleep 20
  for n in "${NAMES[@]}"; do
    [ "$n" = "$MASTER" ] && continue
    echo "  $n:"; start_node "$n" || { echo "REFUSING: joiner start failed: $n" >&2; exit 8; }
  done

  echo "== 8b. triggering a block, because an idle chain cannot finalise =="
  # **Found by measuring.** The first run of this mode reported no finality and looked like a failure of
  # the restore; it was an idle net. This is a `--no-autopropose` network, so a chain that nobody deploys
  # to produces nothing and finality cannot move. §10 asks again while it waits, for both paths — the
  # wipe path has no trigger of its own, which is why its §7 waits on a joiner reaching the anchor and
  # then hands over to §10, whose wait drives the chain rather than only watching it.
  trigger_block || true
else
echo "== 5. apply: every non-master node is stopped first, before anything is moved =="
# All of them, then the moves. The version this derives from stopped and restarted each node inside one
# iteration, putting production back up before agreement had been proven.
for n in "${NAMES[@]}"; do
  [ "$n" = "$MASTER" ] && continue
  echo "  $n:"; stop_node "$n" || { echo "REFUSING: node stop failed: $n" >&2; exit 6; }
done

TS=$(date -u +%Y%m%d-%H%M%S)
echo "== 6. data directories moved aside (never deleted) =="
for n in "${NAMES[@]}"; do
  [ "$n" = "$MASTER" ] && continue
  echo "  $n:"; move_data_dir_aside "$n" "$TS" || { echo "REFUSING: backup/move failed: $n" >&2; exit 7; }
done

echo "== 7. one joiner is restarted and must reach the point before the rest follow =="
first_joiner=""; for n in "${NAMES[@]}"; do [ "$n" = "$MASTER" ] && continue; first_joiner=$n; break; done
if [ -n "$first_joiner" ]; then
  start_node "$first_joiner" || { echo "REFUSING: first joiner start failed" >&2; exit 8; }
  f=""
  for i in $(seq 1 40); do
    sleep 15
    f=$(lfb_num "${HOST[$first_joiner]}" "${PORT[$first_joiner]}")
    h="$(lfb_hash "${HOST[$first_joiner]}" "${PORT[$first_joiner]}")"
    echo "  [$i] $first_joiner finalised=${f:-?} ${h:0:32}"
    [ "$f" = "$MEET" ] && [ "$h" = "$MEET_HASH" ] && break
  done
  if [ "$f" != "$MEET" ] || [ "$h" != "$MEET_HASH" ]; then
    echo "  the first joiner did not reach the point — refusing to restart the others" >&2
    exit 5
  fi
fi

echo "== 8. the rest are restarted =="
for n in "${NAMES[@]}"; do
  [ "$n" = "$MASTER" ] && continue
  [ "$n" = "$first_joiner" ] && continue
  echo "  $n:"; start_node "$n" || { echo "REFUSING: joiner start failed: $n" >&2; exit 8; }
done

fi

echo "== 9. verification: block hashes per height, not heights =="
# A converged *height* is not a converged chain — four nodes at height 0 are equal. Every assertion is on
# the hash at a height, and the frontier is compared within the protocol's own lag (one block).
ok=0
for i in $(seq 1 40); do
  sleep 15
  ok=1; line=""
  for n in "${NAMES[@]}"; do line="$line $n=$(height_of "${HOST[$n]}" "${PORT[$n]}")"; done
  refresh_block_ranges "$MAXH"
  for (( h=MEET; h<=MAXH; h++ )); do
    # **A height nobody has produced is not a disagreement.** `MAXH` is `/api/status`'s
    # `latestBlockNumber`, which counts the round a node is in and sits one ahead of the highest
    # height the block API serves — so the walk's last step is routinely a height with no blocks on
    # any node. Requiring a non-empty set there failed a **converged** net on 2026-10-10: the three
    # nodes agreed at every height 40–44, height 45 was empty on all three, and this section reported
    # "the nodes still disagree on a block hash at or above 41". Empty on *every* node is the frontier
    # and is skipped; empty on *one* node while another has blocks is the divergence being compared
    # for, and still fails.
    first=""; any_empty=0; any_full=0
    for n in "${NAMES[@]}"; do
      hashes="$(blocks_at "${HOST[$n]}" "${PORT[$n]}" "$h" | awk 'NF {print $1}' | LC_ALL=C sort -u)"
      if [ -z "$hashes" ]; then any_empty=1; continue; fi
      any_full=1
      if [ -z "$first" ]; then first="$hashes"
      elif [ "$hashes" != "$first" ]; then ok=0; break; fi
    done
    if [ "$ok" = "0" ] || { [ "$any_empty" = "1" ] && [ "$any_full" = "1" ]; }; then ok=0; break; fi
  done
  echo "  [$i]$line  hashes-agree=$ok"
  [ "$ok" = "1" ] && break
done
if [ "$ok" != "1" ]; then
  echo "  NOT converged after the wait: the nodes still disagree on a block hash at or above $MEET" >&2
  exit 6
fi

echo "== 10. finality past the point =="
# **This section used to be a print.** It printed each node's last-finalized height and hash and exited
# 0 — including when every node still answered "Finalized fringe is not available.", which is what the
# drill transcript `spec/audit/evidence/n-reconcile-drill/restore-run.txt` shows: four `finalised ?`
# lines and a zero exit, under a closing sentence claiming to be #287's falsifier. Nothing here could
# fail. It asserts the acceptance clause instead — *finality advances past the reconciliation point* —
# from four rules, each refusing with a named reason:
#
#   A. **usable** — every node reports an integer finalized height and a non-empty hash: the same
#      condition §3 refuses on at the anchor, applied at the end. A frozen fringe reads as unusable.
#   B. **advanced** — every node's finalized height is strictly greater than $MEET. This is the clause
#      itself; equal to the anchor is not progress.
#   C. **one head** — no two nodes report *different* hashes at the *same* finalized height. Two
#      finalized blocks at one height is a finality conflict; different finalized *heights* are a lag,
#      which is why this is not §3's unanimity rule.
#   D. **in the common DAG** — each node's finalized block is present at its height in that node's own
#      block answer, and in the answer of every *other* node whose own height reaches that height. The
#      last-finalized endpoint and the block API are different surfaces: a hash one reports and no
#      node's block index holds is a string, not a block.
#
# **D alone is not enough, and C is why.** A DAG legitimately holds every sibling at a height, so
# "each node's finalized block is in every node's answer" is true while two *finalized* heads exist.
# C is what makes the pairwise rule mean one chain, and it cannot fire on an honest net: the fringe's
# `max()` is by (height, id), so nodes holding one fringe finalize the same block.
#
# **What this cannot decide, stated rather than implied.** The heights API carries no ancestry, so
# nothing here proves the finalized blocks lie on one chain — only that they are blocks every node that
# reached their height actually holds. With §9 (identical unordered hash sets at every height
# MEET..MAXH) that is the strongest statement these two endpoints support; the ancestry question is
# #287's, and it is why the meet is still owed rather than declared.
adv_ok=0; why="the finality wait never ran"
for i in $(seq 1 "${RECONCILE_FINALITY_ROUNDS:-40}"); do
  sleep 15
  trigger_block >/dev/null 2>&1 || true
  declare -A FINH=() FINB=() NOWH=()
  adv_ok=1; line=""
  for n in "${NAMES[@]}"; do
    FINH[$n]="$(lfb_num "${HOST[$n]}" "${PORT[$n]}")"
    FINB[$n]="$(lfb_hash "${HOST[$n]}" "${PORT[$n]}")"
    NOWH[$n]="$(height_of "${HOST[$n]}" "${PORT[$n]}")"
    line="$line $n=${FINH[$n]:-?}"
  done
  for n in "${NAMES[@]}"; do
    if ! [[ "${FINH[$n]:-}" =~ ^[0-9]+$ ]] || [ -z "${FINB[$n]:-}" ]; then
      adv_ok=0; why="$n reports no usable finalized block (height '${FINH[$n]:-}', hash '${FINB[$n]:-}')"; break
    fi
  done
  if [ "$adv_ok" = "1" ]; then
    for n in "${NAMES[@]}"; do
      if [ "${FINH[$n]}" -le "$MEET" ] 2>/dev/null; then
        adv_ok=0; why="$n finalised at ${FINH[$n]}, which is not past the point ($MEET)"; break
      fi
    done
  fi
  if [ "$adv_ok" = "1" ]; then
    declare -A fin_seen=()
    for n in "${NAMES[@]}"; do
      k="${FINH[$n]}"
      if [ -n "${fin_seen[$k]:-}" ] && [ "${fin_seen[$k]}" != "${FINB[$n]}" ]; then
        adv_ok=0; why="two finalized blocks at height $k: ${fin_seen[$k]:0:12}… and ${FINB[$n]:0:12}…"; break
      fi
      fin_seen[$k]="${FINB[$n]}"
    done
  fi
  if [ "$adv_ok" = "1" ]; then
    # Finality may now be above the original plan's MAXH. Read a fresh inventory through every
    # height this round will check, rather than asking the pre-recovery snapshot about new blocks.
    range_top="$MAXH"
    for n in "${NAMES[@]}"; do
      for candidate_height in "${FINH[$n]}" "${NOWH[$n]:-}"; do
        if [[ "$candidate_height" =~ ^[0-9]+$ ]] && [ "$candidate_height" -gt "$range_top" ]; then
          range_top="$candidate_height"
        fi
      done
    done
    if [ "$range_top" -gt "$MAXH" ] && [ "${RANGE_WIDEN_NOTED:-0}" = "0" ]; then
      echo "  (verifying up to $range_top, above the $MAXH snapshot; this round re-reads 0..$range_top)"
      RANGE_WIDEN_NOTED=1
    fi
    refresh_block_ranges "$range_top"
    for n in "${NAMES[@]}"; do
      fh="${FINH[$n]}"; fb="${FINB[$n]}"
      for m in "${NAMES[@]}"; do
        # A node's own block index must serve its finalized block whatever its tip says; another
        # node's must, but only once it has reached that height — below it, absence is a lag.
        if [ "$m" != "$n" ] && ! [ "${NOWH[$m]:-0}" -ge "$fh" ] 2>/dev/null; then continue; fi
        if ! blocks_at "${HOST[$m]}" "${PORT[$m]}" "$fh" | awk 'NF {print $1}' | grep -Fqx "$fb"; then
          adv_ok=0; why="$n finalised $fh ${fb:0:12}… but $m (height ${NOWH[$m]:-?}) does not hold it"
          break 2
        fi
      done
    done
  fi
  echo "  finality [$i]$line  past-the-point=$adv_ok"
  [ "$adv_ok" = "1" ] && break
done
if [ "$adv_ok" != "1" ]; then
  echo "  NOT final: $why" >&2
  echo "  Finality has not advanced past $MEET on every node, so the recovery is not verified." >&2
  echo "  (a chain that converged to one head but cannot finalise is C259's state, and this refuses it" >&2
  echo "   rather than reporting it as recovered.)" >&2
  exit 9
fi
echo "  finality advanced past $MEET on every node, and every finalized block is one the nodes' own DAGs hold."
echo "  (this section plus section 9 is #287's falsifier: one head, agreeing block hashes, no genesis.)"
