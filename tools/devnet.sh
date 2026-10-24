#!/usr/bin/env bash
#
# devnet — a single Docker script for a local rholo network.
#
# Two modes, one script:
#   • devnet  (default) — bonded validators that produce blocks + a funded deployer wallet, for
#     developing/deploying rholang contracts and reading results back.
#   • network — a bare 1..5 node topology (no autopropose, no deployer) for exercising sync/gossip;
#     drive it manually with `cli <node> propose`.
#
# Prereqs: docker (a running daemon) and openssl.
#
# SECURITY: the validator/deployer keys below are throwaway keys for a LOCAL testnet only.
# Never reuse them for anything with real value.

set -euo pipefail

cd "$(dirname "$0")/.."

IMAGE="${RNODE_IMAGE:-rnode:local}"
# The container/volume prefix is overridable so a *second*, independent network can be brought up
# beside the first — for a measurement against a fresh peer (`DEVNET_PREFIX=perf-sync`) without
# touching the running network's containers or volumes. `NETWORK` stays its own variable because the
# default name is what other tooling and the docs expect.
PREFIX="${DEVNET_PREFIX:-devnet}"

# The shard id this devnet's deploys carry. `/root` is the node default; a devnet that mirrors a
# named net (`/testnet`, `/playground`) sets it here, because a deploy is only accepted when its
# `shardId` equals the node's own (`validate.rs`, `deploys_shard_identifier`) — a bare `root` or a
# stale `/root` is refused outright.
SHARD_ID="${DEVNET_SHARD_ID:-/root}"
NETWORK="${DEVNET_NETWORK:-devnet}"
BOOTSTRAP="${PREFIX}-bootstrap"

# Per-node memory ceiling, applied as `--memory` on every container this script starts. What it
# protects is the *host*: a devnet is usually one of several heavy jobs on a workstation, and an
# unconstrained node that goes wrong takes the machine with it. That is not hypothetical — on
# 2026-09-29 a three-validator devnet under a four-way deploy storm ran beside two other heavy jobs,
# the kernel logged `fill_page_cache_func hogged CPU for >10000us` and then `Under memory pressure,
# flushing caches.`, and the machine froze: 47 GiB of RAM against 2 GiB of swap, so there was no
# reclaim to fall back on. The unconstrained containers were the part of that this script could have
# prevented, and nothing here stopped them.
#
# **The ceiling does bind under load, and that is the designed trade.** Measured the same day, after
# this flag went in: with `4g`, two of three nodes were OOM-killed (exit 137, `oom=true`) *seventeen
# seconds* into the four-way storm, at height ~13. The image is ~132 MiB; a node mid-storm is not. The
# host stayed up with 37 GiB free, which is the outcome this flag is for. A measurement that needs a
# node to *survive* a storm must raise this (`DEVNET_NODE_MEMORY=8g`), lower the storm's concurrency,
# or count the OOM kill as one of its results — a node killed this way ends the run early and, from a
# sampler's side, is indistinguishable from one that merely stopped answering.
#
# `--memory-swap` is set *equal* to `--memory` deliberately: that turns swap off for the node, so a
# node that exceeds its ceiling is OOM-killed inside its own cgroup rather than pushing the host into
# the thrash that makes a freeze a freeze. A killed node costs one run; a thrashed host costs the
# session, and the evidence of what the run was doing.
#
# CPU is deliberately **not** capped. The block timing a devnet measurement observes is the thing
# under test (issue #105's storm is a latency phenomenon), so constraining the scheduler would change
# what is measured. Memory headroom does not. Set to the empty string to opt out of the memory cap.
DEVNET_NODE_MEMORY="${DEVNET_NODE_MEMORY:-4g}"

# The bootstrap's data volume, when a measurement must run against an existing artifact rather than
# `${BOOTSTRAP}-data` (set by `up --data-volume`). Empty means the default.

# The volumes this checkout has carried, and what each is for (recorded 2026-09-24, so the next reader
# does not have to guess from `docker volume ls`):
#   devnet-stale-snapshot      the recorded long chain — *the artifact* measurements run against. It is
#                              a live chain: each run that mounts it extends it (5,844 blocks when it was
#                              recorded, 6,339 after the 2026-09-24 serving-term runs), so a measurement
#                              quoting a height must say which height it measured.
#   devnet-bootstrap-data      the standard bootstrap's own store; `up` reuses it and `--fresh` discards
#   devnet-validator-{1..7}-data  the standard validators' stores, same reuse/`--fresh` semantics
#   devnet-perf-boot           an earlier measurement's bootstrap store, kept for comparison
#   perfsync-validator-{1,2}-data  created by `DEVNET_PREFIX=perfsync` for the fresh-peer measurement;
#                              **disposable** — `DEVNET_PREFIX=perfsync tools/devnet.sh down -v` removes
#                              them and the network they ran on (~40 MiB total)
BOOTSTRAP_DATA_VOLUME=""

# --- mode globals, defaulted at script scope ------------------------------------------------
#
# `cmd_up` sets these before parsing, and `reset` needs them too: it rebuilds a node with the same
# `docker_opts`/`rnode_run_common` the network was started with, and under `set -u` an unbound one is
# fatal rather than merely wrong. They were `cmd_up`'s alone until `reset` landed (#139) and the first
# run died at `EFFECT_SCHEDULER: unbound variable`. `up` still overrides them; `reset` restores them
# from the state file `up` wrote.
AUTOPROPOSE=true
PROPOSE_ON_DEPLOY=true
ADMIN=true
DEPLOYER=true
EFFECT_SCHEDULER=""
POS_EPOCH_LENGTH=""
POS_ACTIVE_VALIDATORS=""
POS_STAKES=""
FRESH=false
N=1
M=0

# Throwaway validator keypairs (secp256k1, base16). validator[0] also funds the deployer wallet, so
# the deployer private key is validator[0]'s private key.
#
# The first three are the original table; entries 4..8 were added for the #149 attestation-cost sweep,
# which needs 5 and 8 validators. `MAX_VALIDATORS` is `#VALIDATOR_PRIV`, so the table *is* the cap — the
# range check at `--validators` and the `node_container` resolver both read it, and the help text's
# `1..8` is a literal only because that text lives in a quoted heredoc. Every public key here is the
# uncompressed `04 || X || Y` of the private key beside it in the same column position; that
# correspondence is what `genesis_files` writes into `bonds.txt`, so a bad row bonds a validator that
# cannot sign.
VALIDATOR_PRIV=(
  "a68a6e6cca30f81bd24a719f3145d20e8424bd7b396309b0708a16c7d8000b76"
  "b8a48b02757c0cfc9325498a93c3b28582e3967072f8fab0cdc8bd04d0d401ee"
  "d78ff60a424d71ce99d6b7d7f44a8c49b38a3757ff9e6fa9b32fcba8aa2c973b"
  "f484e1c24de228819213904c7da8999a94cfdd20e0c70f47a37ae0b6b8c9feb7"
  "0fdf01171ca75c3266ab277b5bee7ef1d05abd63f9bb379f7fb513e5c0778961"
  "515155da7d0242c9c72ae39fda9d19fce088a1c1808011df011380333449ec88"
  "133da16f5f331aa064de134e99803d29d13ad0180258812a38d2530f2bf1e490"
  "3af51aebbbba62593223d85ff47ea73c80da6f199d57be4c83502d6a83782db7"
)
VALIDATOR_PUB=(
  "04f700a417754b775d95421973bdbdadb2d23c8a5af46f1829b1431f5c136e549e8a0d61aa0c793f1a614f8e437711c7758473c6ceb0859ac7e9e07911ca66b5c4"
  "04dbe32c2062240a4ba0bcad01d7edd98c78b51c77765d5e1e5e9fa3743d2f12a1f82f42cd7dc4f41445979117d790f23e9b3d08d0aa06d527c236172043e747fc"
  "04d8b6c325ae12e89823866b2a292a62d7acee520954761890a1621fef79dca1c8e8df79dd8519480e5c015ae6cf3ba7de8669e260561616a36eb9c308b5983ab0"
  "04ea3ce04abbe780205eb5f94a82889600630ca73bd31e7a336efb6700e24c515b900dea427d3e0c0fc74a8a6d5e173331433f00df2de4b7f953c2922297c2c08e"
  "04699fa4251b70aca0d402f0285e9323bde5d00b0d781a60dd95944e453c853e5832086317b8476577100b948db451344d18b11b598dc31216bae117722d511b04"
  "04a58d28edabbe8e1ced19805e3ed3177f8148f5450b50e892ebbd1079c2b2e2ac35723768c74211743ca39e4510fd8ea9d310ae3b97f168d6426b0db1671d2782"
  "0404ba90d6bb60cc079f196580c0c1a70b7e9cb441a054aa05ead1bd4ca697d38869c6047ae734ceb570c2881f51146d666bd86ceb3a16e0f8c0f770435300c645"
  "0494961fa32a09892cb0273fb054492ef7229c596bbc48d73e8f7ea3d9e3e7f7a2da0ad8b702f84a2f895f0821d5a9ff4eb4515b125f88494718cc1e962d2e918c"
)
MAX_VALIDATORS=${#VALIDATOR_PRIV[@]}

# Deployer = validator[0] (its pubkey -> REV address is funded in genesis when DEPLOYER is on).
DEPLOYER_PRIV="${VALIDATOR_PRIV[0]}"
DEPLOYER_REV_ADDR="11112VYAt8rUGNRRZX3eJdgagaAhtWTK8Js7F7X5iqddMVqyDTtYau"
DEPLOYER_BALANCE=1000000000000

# Contract sources are mounted read-only at /contracts; deploy/eval reference them by basename.
CONTRACTS_DIR="$(pwd)/examples"

GRPC_BASE=40402   # host port mapped to the bootstrap's deploy gRPC (in-container 40401)
HTTP_BASE=40403   # host port mapped to the bootstrap's public HTTP API (in-container 40403)
ADMIN_BASE=40405  # host port mapped to the bootstrap's admin HTTP API (in-container 40405)

help() {
  cat >&2 <<'EOF'
usage: tools/devnet.sh <command> [options]

Commands:
  build [--fresh]                build the rnode:local image (--fresh: --no-cache --pull)
  up [options]                   start the network (see options below)
  down [-v]                      stop the network (+ drop data volumes with -v)
  stop <node>                    stop ONE node and leave the rest running — a validator's death, so a
                                 liveness measurement can ask what the survivors do (#70)
  start <node>                   start a node that `stop` stopped (data volume intact)
  reset <i>                      discard validator i's container and store and start it again at once,
                                 so it must LFS-sync the chain the bootstrap is already serving (#139)
  status                         docker ps for the network
  logs <node>                    tail a node's logs
  diagnose                       per-node health check (PASS/WARN/FAIL)
  deploy <contract.rho> [--to N] signed deploy to the bootstrap (file lives in examples/)
  eval <file.rho>                thin-client REPL eval of a file on the bootstrap
  query <name>                   listen for data at a public name
  faucet <rev-address>           transfer 0.3 REV from the funded dev wallet to <rev-address>
  propose [--admin]              force the bootstrap to propose (gRPC, or admin HTTP with --admin)
  demo                           deploy the complex wallet contract and assert its round-trip
  bench [options]                benchmark the *running* devnet: block cost, deploy throughput and
                                 inclusion latency, and the DAG's residency growth (see
                                 `python3 tools/devnet-bench.py --help` for its options). It does not
                                 start a node: run `up --validators 1 --no-autopropose` first, or the
                                 block rate measures the autopropose timer rather than the node
  cli <node> <rnode subcommand…> run the Rust client inside a node container
  help                           this message

`up` options:
  --validators N                 bonded validators (1..8, default 1)
  --observers M                  unbonded observers (0..3, default 0)
  --nodes N                      bare network topology: 1 bootstrap + N-1 peers (1..5),
                                 shorthand for --validators 1 --observers N-1 --no-autopropose
                                 --no-deployer --no-admin
  --autopropose | --no-autopropose
                                 continuously produce blocks on a timer (default: on for devnet,
                                 off for --nodes)
  --propose-on-deploy | --no-propose-on-deploy
                                 propose a block immediately after a deploy (default: on for devnet)
  --admin | --no-admin           publish the admin HTTP API (40405) to the host *and* opt the node's
                                 admin listener back in to binding it. AUDIT C112 binds that listener
                                 to loopback unless the operator asks, and without the opt-in the
                                 published port accepts nothing from the host — so before this, a
                                 published admin port was a port nothing could connect to
                                 (default: on for devnet)
  --fresh                        discard the nodes' data volumes first, so the bootstrap creates the
                                 genesis rather than rebuilding its stored chain (a *restart* rebuilds
                                 and replays the accumulated chain; `up` says which one it is doing)
  --data-volume NAME             mount an existing data volume as the *bootstrap's* data directory,
                                 for measuring against a recorded chain (must already exist; the
                                 validators still use their own `${name}-data` volumes)
  --effect-scheduler MODE        effect-scheduler mode: dfs (default), gate, relaxed-validated, or
                                 relaxed — the last is rejected on the block path at runtime, which
                                 is how tools/devnet-fuzz.py --mode scheduler exercises that guard
  --epoch-length N               PoS epoch length: the first epoch boundary is at block N (default
                                 10000, so a short devnet never reaches one). A *genesis* parameter,
                                 so every bonded validator is given the same value
  --stakes A,B[,C]               per-validator genesis bond, in validator order (default: 100
                                 each). Unequal stakes are how a measurement reaches the cases #70
                                 turns on: three equal validators minus one is *exactly* 2/3, which
                                 is not a supermajority (`two_thirds_is_not_supermajority`), while
                                 100/100/50 minus the 50 leaves 80 % and should recover.
  --active-validators N          the active-set draw's cap. Below the validator count the draw
                                 *selects*, which is the only way to watch randomised selection do
                                 anything on a live chain (default 100, i.e. no selection at 1-3)
  --deployer-key HEX | --no-deployer
                                 fund the deployer wallet + enable dev-mode dummy-deploy keepalive
                                 (default: on for devnet, using validator[0]'s key)

When are blocks created?
  A block is created only when one of these fires: (a) --propose-on-deploy and a deploy is accepted;
  (b) --autopropose's timer/dummy-deploy; or (c) an explicit `propose`/`POST /api/v1/propose`. An idle
  node with none of these produces no blocks.
EOF
  exit 2
}

validator_name() { echo "${PREFIX}-validator-${1}"; }
observer_name()   { echo "${PREFIX}-observer-${1}"; }

cmd_build() {
  local opts=()
  if [[ "${1:-}" == "--fresh" ]]; then
    opts=(--no-cache --pull)
  elif [[ -n "${1:-}" ]]; then
    echo "unknown flag: $1" >&2; help
  fi
  # The image's own provenance. `.dockerignore` excludes `.git/`, so the in-image build cannot run
  # `git rev-parse` — without this argument the served `/version` reads `commit # unknown`, which is
  # what the image did until it was measured (`node/build.rs`, issue #32).
  local commit
  commit="$(git -C "$(cd "$(dirname "$0")/.." && pwd)" rev-parse HEAD 2>/dev/null || true)"
  if [[ -n "$commit" ]]; then
    opts+=(--build-arg "GIT_HEAD_COMMIT=$commit")
  fi
  # Profiling passthroughs (#117): `RNODE_BUILD_FEATURES=dhat-heap` installs the heap profiler, and
  # `RNODE_BUILD_RUSTFLAGS='-C debuginfo=1'` is what makes its output nameable — the release profile
  # is `debug = 0`, so without it the profile is addresses with no symbols. Read from the environment
  # rather than flags for the same reason the node's diagnostic knobs are: they change the *artifact*
  # under investigation, not the network being started.
  #
  # **They select a different Dockerfile, and that is the fix rather than the inconvenience.** A build
  # argument that reaches `cargo` compiles a different binary, so the definition that accepts one must
  # not be the definition a release is produced from (W-N7-03) — `docker/rnode/Dockerfile` no longer
  # declares `RUSTFLAGS` or `CARGO_FEATURES` at all, and this is the only caller that passes them.
  local dockerfile="docker/rnode/Dockerfile"
  if [[ -n "${RNODE_BUILD_FEATURES:-}${RNODE_BUILD_RUSTFLAGS:-}${RNODE_BUILD_MALLOC_CONF:-}" ]]; then
    dockerfile="docker/rnode/Dockerfile.profiling"
  fi
  if [[ -n "${RNODE_BUILD_FEATURES:-}" ]]; then
    opts+=(--build-arg "CARGO_FEATURES=$RNODE_BUILD_FEATURES")
  fi
  if [[ -n "${RNODE_BUILD_RUSTFLAGS:-}" ]]; then
    opts+=(--build-arg "RUSTFLAGS=$RNODE_BUILD_RUSTFLAGS")
  fi
  # And the allocator's configuration, which a profiling build has to set: `prof:true,lg_prof_sample:20,
  # prof_prefix:…` are read by `tikv-jemalloc-sys`'s build script and cannot be supplied at run time
  # (jemalloc marks them read-only after start), so they have to reach the *build*. It is passed through
  # only when set — the Dockerfile guards the empty case, because an empty value would override the
  # purge settings `.cargo/config.toml` compiles in rather than leaving them alone.
  if [[ -n "${RNODE_BUILD_MALLOC_CONF:-}" ]]; then
    opts+=(--build-arg "JEMALLOC_SYS_WITH_MALLOC_CONF=$RNODE_BUILD_MALLOC_CONF")
  fi
  docker build "${opts[@]}" -f "$dockerfile" -t "$IMAGE" .
}

# Write genesis files (N validators + optionally a funded deployer wallet) into `$1`.
genesis_files() {
  local dir="$1" n="$2" i
  : > "$dir/bonds.txt"
  for (( i = 0; i < n; i++ )); do
    local stake=100
    if [[ -n "$POS_STAKES" ]]; then
      stake="$(echo "$POS_STAKES" | cut -d, -f$((i + 1)))"
      if [[ -z "$stake" ]]; then
        echo "--stakes needs one stake per bonded validator ($n wanted, $POS_STAKES given)" >&2
        return 2
      fi
    fi
    echo "${VALIDATOR_PUB[$i]} $stake" >> "$dir/bonds.txt"
  done
  if $DEPLOYER; then
    echo "$DEPLOYER_REV_ADDR,$DEPLOYER_BALANCE" > "$dir/wallets.txt"
  else
    : > "$dir/wallets.txt"
  fi
  # The containers read these as the image's `rnode` uid (1000), which is not necessarily the uid
  # that created the directory. On a CI runner whose temp dir is not world-traversable the bootstrap
  # died with `FAILED PARSING WALLETS FILE: /genesis/wallets.txt — Permission denied`, which the
  # healthcheck reports only as a timeout (the nightly's red from 2026-09-14 to 2026-09-24).
  chmod -R a+rX "$dir"
}

wait_for_cert() {
  local c="$1"
  for _ in $(seq 1 60); do
    if docker exec "$c" test -f /var/lib/rnode/node.certificate.pem 2>/dev/null; then
      return 0
    fi
    sleep 1
  done
  echo "timed out waiting for $c to generate its TLS cert" >&2
  return 1
}

bootstrap_id() {
  # The node's certificate carries the node id as its CN, but `openssl x509 -subject` renders that
  # differently across releases (`subject=CN=<hex>` vs `subject=CN = <hex>`), and taking the last
  # `=`-field of an unnormalized rendering yields a value with a leading space. That is how the
  # devnet-fuzz nightly fed every joining validator `rnode:// <hex>@…` and lost it to
  # `Can not parse the bootstrap address` — a timeout at the healthcheck, ten days running.
  # `-nameopt RFC2253` is one fixed shape, and the whitespace strip covers the rest.
  docker exec "$BOOTSTRAP" cat /var/lib/rnode/node.certificate.pem \
    | openssl x509 -noout -subject -nameopt RFC2253 \
    | sed -n 's/^subject=//p' \
    | tr -d '[:space:]' \
    | sed -n 's/^CN=//p'
}

# Wait until the bootstrap serves /api/v1/status and (if autopropose is on) is producing blocks.
# Wait until the bootstrap has **committed its genesis** — a different event from serving HTTP.
#
# `latest_block_number()` is `max_height + 1`, so genesis alone reports `1`. This matters because the
# genesis master *broadcasts* its approved fringe as it creates genesis — `FinalizedFringe { hashes: [] }`
# with the genesis **pre**-state — and a node that connected first receives it. A joining validator that
# latches on that empty announcement instead of the answer to its own request syncs to nothing and then
# discards the real answer (issue #100). `wait_for_http` cannot serve here: with `--no-autopropose` it
# skips the block-number check altogether, and it runs after the node loops in any case.
#
# The HTTP server is up before genesis exists (the same ordering `node/tests/node_api.rs` polls around),
# so this is a real gate rather than a no-op.
wait_for_genesis() {
  local url="http://localhost:${HTTP_BASE}/api/v1/status"
  local body block_num
  for _ in $(seq 1 120); do
    if body="$(curl -fsS --max-time 5 "$url" 2>/dev/null)"; then
      block_num="$(printf '%s' "$body" | sed -n 's/.*"latestBlockNumber":\([0-9]*\).*/\1/p')"
      if [[ -n "$block_num" && "$block_num" -gt 0 ]]; then
        echo "==> $BOOTSTRAP has committed genesis (latestBlockNumber=$block_num); starting the network"
        return 0
      fi
    fi
    sleep 1
  done
  echo "timed out waiting for $BOOTSTRAP to commit its genesis" >&2
  return 1
}

wait_for_http() {
  local url="http://localhost:${HTTP_BASE}/api/v1/status"
  local body block_num
  for _ in $(seq 1 120); do
    if body="$(curl -fsS --max-time 5 "$url" 2>/dev/null)"; then
      block_num="$(printf '%s' "$body" | sed -n 's/.*"latestBlockNumber":\([0-9]*\).*/\1/p')"
      if ! $AUTOPROPOSE; then
        echo "==> $BOOTSTRAP serving /api/v1/status (autopropose off; block production is manual)"
        return 0
      fi
      if [[ -n "$block_num" && "$block_num" -gt 0 ]]; then
        echo "==> $BOOTSTRAP serving /api/v1/status (latestBlockNumber=$block_num)"
        return 0
      fi
    fi
    sleep 1
  done
  echo "timed out waiting for $BOOTSTRAP to serve /api/v1/status" >&2
  return 1
}

# A devnet is not ready just because the bootstrap HTTP API answers. The
# nightly fuzz starts immediately after `up`, so wait for every advertised
# validator/observer API before returning success.
wait_for_peer_http() {
  local name="$1" port="$2" url body block_num
  url="http://localhost:${port}/api/v1/status"
  # **How long a joining peer gets to serve genesis.** 120 seconds is generous on an idle machine and
  # is not on a loaded one: the adversarial-arm runner saturates the host with parallel clients, and a
  # peer that is still replaying under that load has been seen to miss the window and be healthy a
  # minute later. An override rather than a bigger constant, because the right number is a property of
  # the host and not of the net.
  local tries="${DEVNET_WAIT_TRIES:-120}"
  for (( attempt=1; attempt<=tries; attempt++ )); do
    if ! docker inspect -f '{{.State.Running}}' "$name" 2>/dev/null | grep -qx true; then
      echo "$name stopped before it synced genesis and became ready" >&2
      return 1
    fi
    if body="$(curl -fsS --max-time 3 "$url" 2>/dev/null)"; then
      block_num="$(printf '%s' "$body" | sed -n 's/.*"latestBlockNumber":\([0-9]*\).*/\1/p')"
      # HTTP can answer while a joining peer is still restoring LFS/DAG state. A
      # non-zero latestBlockNumber proves genesis is present locally, which is
      # the minimum contract required by the fuzz harness' explicit block anchor.
      if [[ -n "$block_num" && "$block_num" -gt 0 ]]; then
        echo "==> $name synced genesis and serves /api/v1/status (latestBlockNumber=$block_num port=$port)"
        return 0
      fi
    fi
    sleep 1
  done
  echo "timed out waiting for $name to sync genesis and serve /api/v1/status (port $port)" >&2
  return 1
}

# `docker run` flags shared by every node (container name/network/ports + data + contracts mounts).
# The named volume a node's data directory lives in.
#
# One definition, because two things have to agree about it: the container that runs the node, and
# the `seed_validator_key` step that writes that node's signing key into it. `--data-volume` names the
# *bootstrap's* volume, and only the bootstrap's: the recorded long chain measurements run against is
# the bootstrap's, and pointing every node at one directory would have them fight over the same LMDB
# environments.
data_volume() {
  local name="$1"
  if [[ "$name" == "$BOOTSTRAP" && -n "$BOOTSTRAP_DATA_VOLUME" ]]; then
    echo "$BOOTSTRAP_DATA_VOLUME"
  else
    echo "${name}-data"
  fi
}

# Write this validator's signing key into its own data volume, **from inside a container**.
#
# It is deliberately not a host bind-mount. The image's `rnode` is uid 1000 and a CI runner's uid is
# 1001, so a host file at `0600` is unreadable in-container — while any wider mode is exactly what the
# loader now refuses (`load_validator_private_key`, W-N5-09). Writing the file with the container's
# own uid and `umask 077` makes its owner and its mode right for whatever uid the node runs as, on any
# host, and it is what lets the key stay off the command line: `--validator-private-key` puts the
# scalar in the container's argument list, where `docker inspect` returns it to anyone who can talk to
# the daemon (W-N6-06).
seed_validator_key() {
  local name="$1" idx="$2" vol
  vol="$(data_volume "$name")"
  docker volume create "$vol" >/dev/null
  printf '%s\n' "${VALIDATOR_PRIV[$idx]}" \
    | docker run --rm -i --entrypoint sh -v "${vol}:/var/lib/rnode" "$(node_image "$name")" \
        -c 'umask 077; cat > /var/lib/rnode/validator.hex'
}

docker_opts() {
  local name="$1" grpc_host="$2" http_host="$3" admin_host="$4"
  # Hosted CI may reserve a validator's host gRPC port. LFS nodes talk over
  # the internal Docker network; only the bootstrap's host gRPC is used for
  # deploy/propose. Test-only switch: leave the joiners' host gRPC unpublished.
  local ports="-p ${http_host}:40403"
  if [[ "${DEVNET_SKIP_JOINER_GRPC_PUBLISH:-0}" != "1" || "$name" == "$BOOTSTRAP" ]]; then
    ports="-p ${grpc_host}:40401 $ports"
  fi
  if $ADMIN; then ports="$ports -p ${admin_host}:40405"; fi
  local data; data="$(data_volume "$name")"
  # See `DEVNET_NODE_MEMORY` at the top of this file: the ceiling and the no-swap pair are what keep
  # a runaway node inside its own cgroup instead of in the host's swap.
  local limits=""
  if [[ -n "$DEVNET_NODE_MEMORY" ]]; then
    limits="--memory $DEVNET_NODE_MEMORY --memory-swap $DEVNET_NODE_MEMORY"
  fi
  # Extra container environment, comma-separated `KEY=VALUE` pairs (`DEVNET_NODE_ENV`). The allocator
  # knobs are why this exists: a node's footprint under fork load is allocator-shaped — one worker per
  # core means one glibc arena per worker, each holding its own high-water mark — and
  # `MALLOC_ARENA_MAX=2` / `MALLOC_TRIM_THRESHOLD_` are how that is tested **without a rebuild**, which
  # matters because a rebuild changes the binary under test (#117).
  # Pairs are comma-separated, **or semicolon-separated when the value contains a comma** — which every
  # jemalloc `MALLOC_CONF` does (`background_thread:true,dirty_decay_ms:0,...`), and splitting those
  # commas shreds the setting into fragments that docker then reads as separate variables. With a
  # semicolon anywhere in the string, that becomes the separator and commas belong to the values.
  local envs="" pair spec="${DEVNET_NODE_ENV:-}"
  if [[ -n "$spec" ]]; then
    local sep=","
    [[ "$spec" == *";"* ]] && sep=";"
    for pair in ${spec//$sep/ }; do envs="$envs -e $pair"; done
  fi
  # Outbound bootstrap connections may start before every API listener binds.
  # Keep their ephemeral source ports away from this container's service ports.
  # This is namespaced to the disposable node, not a host or consensus setting.
  echo "-d --name $name --network $NETWORK --sysctl net.ipv4.ip_local_reserved_ports=40400-40405 $ports $limits $envs \
    -v ${data}:/var/lib/rnode \
    -v ${CONTRACTS_DIR}:/contracts:ro"
}

# `rnode run` flags shared by every node, assembled from the current flag globals.
rnode_run_common() {
  local name="$1"
  local flags="run --host $name --api-host 0.0.0.0 --data-dir /var/lib/rnode \
    --protocol-port 40400 --discovery-port 40404 \
    --api-port-grpc-external 40401 --api-port-grpc-internal 40402 \
    --api-port-http 40403 --api-port-admin-http 40405"
  if $AUTOPROPOSE; then flags="$flags --autopropose"; fi
  if $PROPOSE_ON_DEPLOY; then flags="$flags --propose-on-deploy"; fi
  if $ADMIN; then flags="$flags --api-enable-devnet-cors"; fi
  # **The bind, not just the publish.** AUDIT C112 made the admin listener loopback-only unless the
  # operator opts in, and `--admin` *is* that opt-in — but this flag only opened the firewall, so
  # until this line the published port accepted nothing from the host and `propose --admin` (which
  # curls `localhost:$ADMIN_BASE` from here) could never have worked. Found by the benchmark harness,
  # whose first propose got `Connection reset by peer` in 10 ms: the node was healthy, autopropose was
  # producing blocks and the port was mapped — the listener was bound to the container's loopback.
  if $ADMIN; then flags="$flags --api-enable-devnet-admin-public"; fi
  # **Every node a devnet starts is in dev mode**, not only when a deployer key is configured. A
  # *production* genesis must decide the developer allocation and its vesting term and is refused at
  # creation without them (Laws 78–79), and a devnet is exactly the network that need not — so
  # passing dev mode here is what lets `--no-deployer` and `--nodes` devnets build genesis at all.
  # Two things this does **not** preserve: dev mode also enables `exploratory_deploy` on every
  # validator (`casper/src/api/block_api_impl.rs`), and the format-2 PosParams record changes every
  # genesis hash, so "the hashes are unchanged" is not a property of this line.
  flags="$flags --dev-mode"
  if $DEPLOYER; then flags="$flags --deployer-private-key ${DEPLOYER_PRIV}"; fi
  # A verbosity passthrough for diagnosing the **inbound** path, where a node's silence is otherwise
  # indistinguishable from a message that never arrived (issue #100): the frames a node *receives* are
  # logged at debug/trace and nowhere else, so without this a stalled handshake reads as a healthy
  # node with a height that does not move. Read from the environment rather than a flag, because it is
  # a diagnostic knob rather than a property of the network being started.
  if [[ -n "${DEVNET_LOG_LEVEL:-}" ]]; then flags="$flags --log-level $DEVNET_LOG_LEVEL"; fi
  # The scheduler's worker count, from the environment rather than a flag, for the same reason as the
  # log level: it is a diagnostic knob, not a property of the network being started. It is also a
  # memory knob — the worker threads are where allocation happens, and glibc gives each one its own
  # arena that holds its own high-water mark, so the count bounds the *sum* of those peaks (#117).
  if [[ -n "${DEVNET_THREAD_POOL_SIZE:-}" ]]; then
    flags="$flags --thread-pool-size $DEVNET_THREAD_POOL_SIZE"
  fi
  # The effect-scheduler mode (Laws 20-25). The default is the sequential reference; `gate` and
  # `relaxed-validated` are the block-path-capable alternatives, and `relaxed` is rejected on the
  # block path at runtime (casper/tests/scheduler.rs::block_paths_reject_relaxed_mode), so starting
  # a devnet with it is how that rejection is exercised end to end.
  if [[ -n "$EFFECT_SCHEDULER" ]]; then flags="$flags --effect-scheduler $EFFECT_SCHEDULER"; fi
  # The PoS epoch length and active-set cap, for exercising an **epoch boundary** and the randomised
  # active-set draw on a live chain. Both are *genesis* parameters installed outside the genesis
  # block's deploys, so every bonded validator must be given the same values or it computes a
  # different genesis post-state and refuses block #0 forever (AUDIT C46). They are set here, on the
  # flag string every node shares, rather than per node, which is what makes that automatic.
  #
  # `epoch_length` is the boundary period (`block % epoch_length == 0`); the default is 10000, so a
  # devnet crosses its first boundary at block 10000 unless it is lowered. `active_validators` is the
  # draw's cap — set it below the validator count and the draw *selects*, which is the only way to
  # see the rule do anything end to end.
  if [[ -n "$POS_EPOCH_LENGTH" ]]; then flags="$flags --epoch-length $POS_EPOCH_LENGTH"; fi
  if [[ -n "$POS_ACTIVE_VALIDATORS" ]]; then
    flags="$flags --number-of-active-validators $POS_ACTIVE_VALIDATORS"
  fi
  # **Per-node extra `rnode run` flags.** The flag string above is deliberately *shared*, because a
  # genesis parameter handed to one node and not another is a different chain (AUDIT C46) — but a
  # measurement of what a **config mismatch** does needs exactly the opposite: one node set differently
  # from its peers, and everything else identical. So the escape hatch is per node, and it is read from
  # the environment rather than added as a flag, for the reason `DEVNET_LOG_LEVEL` is: it is a property
  # of the node *under investigation* rather than of the network being started.
  #
  #   DEVNET_EXTRA_FLAGS_devnet_validator_1="--min-phlo-price 5" tools/devnet.sh up --validators 2
  #
  # The variable name is the container name with dashes turned into underscores; `DEVNET_EXTRA_FLAGS`
  # applies to every node. Used by the risk plan's A1 arm (#150), whose whole subject is two nodes that
  # disagree about one setting.
  local node_var
  node_var="DEVNET_EXTRA_FLAGS_$(printf '%s' "$name" | tr '-' '_')"
  if [[ -n "${DEVNET_EXTRA_FLAGS:-}" ]]; then flags="$flags ${DEVNET_EXTRA_FLAGS}"; fi
  if [[ -n "${!node_var:-}" ]]; then flags="$flags ${!node_var}"; fi
  echo "$flags"
}

# The **image** one node runs, per node — the same shape as the flags above and for a strictly stronger
# reason. `RNODE_IMAGE` is one image for the whole network, which is right for every measurement that
# runs one build; a measurement of what an **unupgraded** node does beside an upgraded one needs two
# binaries in one network, and that is the only way to observe a fork point directly rather than by
# reasoning about one.
#
#   RNODE_IMAGE_devnet_validator_2="rnode:old" tools/devnet.sh up --validators 2
#
# Unset falls back to `$IMAGE`, so a network that names no per-node image is byte-for-byte the network
# it was before this function existed. The variable name is the container name with dashes turned into
# underscores, exactly as `DEVNET_EXTRA_FLAGS_<container>` is.
node_image() {
  local name="$1"
  local var
  var="RNODE_IMAGE_$(printf '%s' "$name" | tr '-' '_')"
  if [[ -n "${!var:-}" ]]; then echo "${!var}"; else echo "$IMAGE"; fi
}

cmd_up() {
  # Mode globals: devnet defaults.
  local n=1 m=0
  AUTOPROPOSE=true
  PROPOSE_ON_DEPLOY=true
  ADMIN=true
  DEPLOYER=true
  EFFECT_SCHEDULER=""   # default: the node's own default (dfs)
  POS_EPOCH_LENGTH=""   # default: the node's own (10000) — see `rnode_run_common`
  POS_ACTIVE_VALIDATORS=""
  POS_STAKES=""   # default: 100 each; `--stakes` sets them per validator
  FRESH=false

  while [[ $# -gt 0 ]]; do
    case "$1" in
      --validators) n="${2:?}"; shift 2 ;;
      --observers)  m="${2:?}"; shift 2 ;;
      --fresh) FRESH=true; shift ;;
      --data-volume)
        # Only `up` reads this, and only for the bootstrap. It is how a measurement runs against the
        # recorded long chain (`devnet-stale-snapshot`) instead of `${BOOTSTRAP}-data`; `--fresh`
        # still clears the standard volumes, so passing both leaves the named artifact alone.
        BOOTSTRAP_DATA_VOLUME="${2:?--data-volume needs a volume name}"; shift 2 ;;
      --epoch-length) POS_EPOCH_LENGTH="${2:?}"; shift 2 ;;
      --active-validators) POS_ACTIVE_VALIDATORS="${2:?}"; shift 2 ;;
      --stakes) POS_STAKES="${2:?}"; shift 2 ;;
      --effect-scheduler)
        EFFECT_SCHEDULER="${2:?}"
        case "$EFFECT_SCHEDULER" in
          dfs|gate|relaxed|relaxed-validated) ;;
          *) echo "--effect-scheduler must be one of dfs, gate, relaxed, relaxed-validated" >&2; exit 2 ;;
        esac
        shift 2 ;;
      --nodes)
        n=1; m=$((${2:?} - 1)); AUTOPROPOSE=false; PROPOSE_ON_DEPLOY=false; ADMIN=false; DEPLOYER=false
        shift 2 ;;
      --autopropose) AUTOPROPOSE=true; shift ;;
      --no-autopropose) AUTOPROPOSE=false; shift ;;
      --propose-on-deploy) PROPOSE_ON_DEPLOY=true; shift ;;
      --no-propose-on-deploy) PROPOSE_ON_DEPLOY=false; shift ;;
      --admin) ADMIN=true; shift ;;
      --no-admin) ADMIN=false; shift ;;
      --deployer-key) DEPLOYER=true; DEPLOYER_PRIV="${2:?}"; shift 2 ;;
      --no-deployer) DEPLOYER=false; shift ;;
      *) echo "unknown flag: $1" >&2; help ;;
    esac
  done
  if (( n < 1 || n > MAX_VALIDATORS )); then
    echo "--validators must be in 1..$MAX_VALIDATORS" >&2; exit 2
  fi
  if (( m < 0 || m > 3 )); then
    echo "--observers must be in 0..3" >&2; exit 2
  fi

  echo "==> devnet: $n validator(s) + $m observer(s)"
  echo "    autopropose=$AUTOPROPOSE propose-on-deploy=$PROPOSE_ON_DEPLOY admin=$ADMIN deployer=$DEPLOYER"
  if [[ -n "$POS_EPOCH_LENGTH" || -n "$POS_ACTIVE_VALIDATORS" ]]; then
    echo "    pos: epoch-length=${POS_EPOCH_LENGTH:-default} active-validators=${POS_ACTIVE_VALIDATORS:-default}"
  fi
  docker network create "$NETWORK" >/dev/null 2>&1 || true

  local genesis_dir
  genesis_dir="$(mktemp -d)"
  genesis_files "$genesis_dir" "$n"

  # `up` reuses each node's `${name}-data` volume, so a second run against an accumulated chain is a
  # *restart*: it rebuilds and replays what the first run wrote. That is a different animal from a
  # fresh start — the whole of AUDIT C55 was a bootstrap that worked against a fresh volume and
  # appeared to hang on every later one, silently — so say which one this is, every time.
  local reused=() c v
  # `--data-volume` names a volume that must already exist: `docker run -v` would *create* an empty
  # one, and the run would silently start from genesis instead of from the artifact it was asked to
  # measure — the same class of quiet substitution that `--fresh` exists to announce.
  if [[ -n "$BOOTSTRAP_DATA_VOLUME" ]]; then
    if ! docker volume inspect "$BOOTSTRAP_DATA_VOLUME" >/dev/null 2>&1; then
      echo "--data-volume ${BOOTSTRAP_DATA_VOLUME}: no such volume" >&2
      exit 1
    fi
    echo "==> bootstrap data volume: $BOOTSTRAP_DATA_VOLUME (bootstrap only; not ${BOOTSTRAP}-data)"
  fi
  c="$BOOTSTRAP"
  if [[ -z "$BOOTSTRAP_DATA_VOLUME" ]] && docker volume inspect "${c}-data" >/dev/null 2>&1; then
    reused+=("$c")
  fi
  for (( v = 1; v < n; v++ )); do
    c="$(validator_name "$v")"
    if docker volume inspect "${c}-data" >/dev/null 2>&1; then reused+=("$c"); fi
  done
  if $FRESH && (( ${#reused[@]} > 0 )); then
    echo "==> --fresh: discarding the data volumes of ${reused[*]}"
    for c in "${reused[@]}"; do docker volume rm "${c}-data" >/dev/null 2>&1 || true; done
    reused=()
  elif (( ${#reused[@]} > 0 )); then
    echo "==> restarting against existing data volumes: ${reused[*]}"
    echo "    (each rebuilds its accumulated chain; pass --fresh to start from genesis instead)"
  fi

  # Validator 0 = bootstrap: creates + approves genesis (autopropose optional).
  echo "==> starting $BOOTSTRAP (validator 0, standalone, creates genesis)"
  seed_validator_key "$BOOTSTRAP" 0
  # shellcheck disable=SC2046
  docker run $(docker_opts "$BOOTSTRAP" "$GRPC_BASE" "$HTTP_BASE" "$ADMIN_BASE") \
    -v "${genesis_dir}:/genesis:ro" \
    "$(node_image "$BOOTSTRAP")" $(rnode_run_common "$BOOTSTRAP") -s \
      --bonds-file /genesis/bonds.txt --wallets-file /genesis/wallets.txt \
      --validator-private-key-path /var/lib/rnode/validator.hex

  wait_for_cert "$BOOTSTRAP"
  local id
  id="$(bootstrap_id)"
  echo "==> bootstrap id: $id"

  # Before any node joins: the master must have committed genesis, or a joiner races its broadcast.
  wait_for_genesis

  # Validators 1..n-1: bonded in genesis.
  local i name host_port http_port admin_port
  for (( i = 1; i < n; i++ )); do
    name="$(validator_name "$i")"
    host_port=$((GRPC_BASE + i * 1000))
    http_port=$((HTTP_BASE + i * 1000))
    admin_port=$((ADMIN_BASE + i * 1000))
    echo "==> starting $name (validator $i, bootstraps from $BOOTSTRAP)"
    seed_validator_key "$name" "$i"
    # The genesis files go to every *bonded* validator, not only the bootstrap. They are part of the
    # network configuration: a joining validator replays the genesis block when it first indexes it
    # (the finalized fringe points at block #0), and the genesis's native PoS state and REV vault
    # balances are installed outside the block's deploys, so without these files its replay computes
    # a different post-state and it refuses block #0 forever — AUDIT C46, which this is the
    # deployment half of. Mounted read-only: a non-ceremony node reads the bonds file strictly and
    # must not generate one (that would be a different chain's validator set).
    # shellcheck disable=SC2046
    docker run $(docker_opts "$name" "$host_port" "$http_port" "$admin_port") \
      -v "${genesis_dir}:/genesis:ro" \
      "$(node_image "$name")" $(rnode_run_common "$name") \
        --bootstrap "rnode://${id}@${BOOTSTRAP}?protocol=40400&discovery=40404" \
        --bonds-file /genesis/bonds.txt --wallets-file /genesis/wallets.txt \
        --validator-private-key-path /var/lib/rnode/validator.hex
  done

  # Observers / bare peers: unbonded, replicate the chain.
  for (( i = 1; i <= m; i++ )); do
    name="$(observer_name "$i")"
    host_port=$((GRPC_BASE + (n + i) * 1000))
    http_port=$((HTTP_BASE + (n + i) * 1000))
    admin_port=$((ADMIN_BASE + (n + i) * 1000))
    echo "==> starting $name (observer $i, bootstraps from $BOOTSTRAP)"
    # shellcheck disable=SC2046
    docker run $(docker_opts "$name" "$host_port" "$http_port" "$admin_port") \
      "$(node_image "$name")" $(rnode_run_common "$name") \
        --bootstrap "rnode://${id}@${BOOTSTRAP}?protocol=40400&discovery=40404"
  done

  # Recorded so `reset` can rebuild ONE node against the same flags (#139). Written after the nodes are
  # up, so a state file never describes a network that failed to start.
  write_up_state "$n" "$m"
  wait_for_http
  # A successful `up` is the readiness contract for callers such as the fuzz
  # workflow. Wait for all peers, not merely their container creation.
  for (( i = 1; i < n; i++ )); do
    wait_for_peer_http "$(validator_name "$i")" "$((HTTP_BASE + i * 1000))"
  done
  for (( i = 1; i <= m; i++ )); do
    wait_for_peer_http "$(observer_name "$i")" "$((HTTP_BASE + (n + i) * 1000))"
  done

  echo ""
  echo "==> up. Interact with:"
  echo "    tools/devnet.sh deploy <contract.rho>   # signed deploy to $BOOTSTRAP"
  echo "    tools/devnet.sh query <name>            # listen for data at a public name"
  echo "    tools/devnet.sh propose [--admin]       # force a block"
  echo "    tools/devnet.sh status | logs <node> | diagnose | down"
  echo ""
  echo "    Public HTTP API:  http://localhost:${HTTP_BASE}/api/v1/status"
  if $ADMIN; then
    echo "    Admin HTTP API:   http://localhost:${ADMIN_BASE}/api/v1/propose"
  fi
}

cmd_down() {
  local remove_volumes=false
  [[ "${1:-}" == "-v" ]] && remove_volumes=true
  local names=("$BOOTSTRAP")
  local i
  for (( i = 1; i <= MAX_VALIDATORS; i++ )); do names+=("$(validator_name "$i")"); done
  for (( i = 1; i <= 3; i++ )); do names+=("$(observer_name "$i")"); done
  for c in "${names[@]}"; do
    docker rm -f "$c" >/dev/null 2>&1 || true
    if $remove_volumes; then docker volume rm "${c}-data" >/dev/null 2>&1 || true; fi
  done
  docker network rm "$NETWORK" >/dev/null 2>&1 || true
}

cmd_status() {
  docker ps --filter "network=$NETWORK" --format 'table {{.Names}}\t{{.Status}}\t{{.Ports}}'
}

# Resolve what a user calls a node to a container name: `2` -> `devnet-validator-2`, `bootstrap` or a
# full name passes through. One resolver for `stop`/`start`, so they cannot drift apart.
node_container() {
  case "${1:?node required — a validator number (1..$MAX_VALIDATORS), 'bootstrap', or a container name}" in
    [0-9]*) validator_name "$1" ;;
    *) echo "$1" ;;
  esac
}

# stop <node>: stop ONE node and leave the rest of the devnet running.
#
# `down` removes the whole network and takes no node name, so before this a liveness measurement had no
# supported way to remove a single participant — and #70's recovery case is exactly the measurement that
# needs one: kill a validator, then ask whether the survivors keep finalising. Without it the only
# implementable "kill" is `docker rm -f` typed by hand, which is not a repeatable experiment.
#
# Stopped, not removed: the container's logs and its data volume survive, so `start` brings the same
# validator back with its chain — which is the *other* half of the question (a silent validator that
# returns), and the reason this is a stop rather than a `down`.
cmd_stop() {
  local name; name="$(node_container "${1:-}")"
  docker stop "$name" >/dev/null || { echo "could not stop $name" >&2; exit 1; }
  echo "stopped $name"
}

# start <node>: bring back a node that `stop` stopped.
cmd_start() {
  local name; name="$(node_container "${1:-}")"
  docker start "$name" >/dev/null || { echo "could not start $name" >&2; exit 1; }
  echo "started $name"
}

# up-state file — what the last `up` started, so `reset` can rebuild ONE node against it.
#
# **Why a file and not `docker inspect`.** The obvious source is the removed container's own spec, but
# that dies with the container, and `reset` has to remove it. The alternatives were both worse: making
# `up` tolerant of existing containers does not help, because `up` also restarts the *bootstrap*, and a
# joiner that syncs while the bootstrap is replaying its own store is syncing a chain that is still
# being rebuilt — measured, 2026-10-01: the joiner took the LFS path (its log says so) and still saw
# nothing, because the chain it restored was the bootstrap's half-replayed one.
UP_STATE="target/devnet-up-state"   # relative: this script cd's to the repo root at the top

write_up_state() { # <n> <m>
  mkdir -p "$(dirname "$UP_STATE")"
  {
    echo "N=$1"
    echo "M=$2"
    echo "AUTOPROPOSE=$AUTOPROPOSE"
    echo "PROPOSE_ON_DEPLOY=$PROPOSE_ON_DEPLOY"
    echo "ADMIN=$ADMIN"
    echo "DEPLOYER=$DEPLOYER"
    echo "POS_EPOCH_LENGTH=$POS_EPOCH_LENGTH"
    echo "POS_ACTIVE_VALIDATORS=$POS_ACTIVE_VALIDATORS"
    echo "POS_STAKES=$POS_STAKES"
    echo "EFFECT_SCHEDULER=$EFFECT_SCHEDULER"
  } > "$UP_STATE"
}

read_up_state() {
  [[ -f "$UP_STATE" ]] || {
    echo "reset: no $UP_STATE — run 'tools/devnet.sh up ...' first, so the flags to rebuild against are known" >&2
    exit 1
  }
  local k v
  while IFS='=' read -r k v; do
    case "$k" in
      N) N="$v" ;; M) M="$v" ;;
      AUTOPROPOSE) AUTOPROPOSE="$v" ;; PROPOSE_ON_DEPLOY) PROPOSE_ON_DEPLOY="$v" ;;
      ADMIN) ADMIN="$v" ;; DEPLOYER) DEPLOYER="$v" ;;
      POS_EPOCH_LENGTH) POS_EPOCH_LENGTH="$v" ;;
      POS_ACTIVE_VALIDATORS) POS_ACTIVE_VALIDATORS="$v" ;;
      POS_STAKES) POS_STAKES="$v" ;;
      EFFECT_SCHEDULER) EFFECT_SCHEDULER="$v" ;;
    esac
  done < "$UP_STATE"
}

# reset <i>: discard validator `i`'s container and store, and start it again **now**, so it must sync.
#
# **Why this exists as a command.** A node only LFS-syncs when its DAG is empty at start
# (`NodeLaunch` syncs on a fresh store), so "join a chain that is already mature" is not something `up`
# can stage: `up` starts every node at once, from genesis, and the joiner restores block 0 alone — and
# the genesis goes in through `insert_genesis` with the correct fringe, which is why this has never been
# observed. The experiment is therefore *wipe one node's store while the rest of the network keeps
# running*, and neither existing command does it: `stop`/`start` reuse the volume (the node would
# rebuild its stored chain and never sync), and `down`/`up` restarts the bootstrap too, which was
# measured to make the arm say nothing.
cmd_reset() {
  local idx="${1:-}"
  case "$idx" in
    1|2|3) ;;
    *) echo "reset: takes a validator number (1..3); the bootstrap and observers are not supported" >&2; exit 2 ;;
  esac
  read_up_state
  # The joiner is only meaningfully "joining" a chain the bootstrap is already serving, so refuse rather
  # than produce a run that looks like a null result and is really a missing precondition.
  if [[ "$(docker inspect -f '{{.State.Running}}' "$BOOTSTRAP" 2>/dev/null)" != "true" ]]; then
    echo "reset: $BOOTSTRAP is not running — the point is to join a chain that is already there" >&2
    exit 1
  fi

  local name; name="$(validator_name "$idx")"
  docker rm -f "$name" >/dev/null 2>&1 || true
  docker volume rm "${name}-data" >/dev/null 2>&1 || true

  local genesis_dir; genesis_dir="$(mktemp -d)"
  genesis_files "$genesis_dir" "$N"
  local id; id="${DEVNET_RESET_BOOTSTRAP_ID:-$(bootstrap_id)}"
  local host_port=$((GRPC_BASE + idx * 1000))
  local http_port=$((HTTP_BASE + idx * 1000))
  local admin_port=$((ADMIN_BASE + idx * 1000))
  # Test-only alternate locator, without changing the trusted node key.
  local reset_bootstrap_host="${DEVNET_RESET_BOOTSTRAP_HOST:-$BOOTSTRAP}"
  local -a reset_anchor=()
  if [[ -n "${DEVNET_RESET_SYNC_ANCHOR:-}" ]]; then
    reset_anchor=(--sync-anchor "$DEVNET_RESET_SYNC_ANCHOR")
  fi
  echo "==> reset $name: empty store, syncing the chain from $reset_bootstrap_host"
  seed_validator_key "$name" "$idx"
  # Recovery can stage a bonded validator as a passive replicator until its
  # validated catch-up is complete. The key stays in its volume for activation.
  local -a reset_validator_key=()
  if [[ "${DEVNET_RESET_PASSIVE:-0}" != "1" ]]; then
    reset_validator_key=(--validator-private-key-path /var/lib/rnode/validator.hex)
  fi
  # shellcheck disable=SC2046
  docker run $(docker_opts "$name" "$host_port" "$http_port" "$admin_port") \
    -v "${genesis_dir}:/genesis:ro" \
    "$(node_image "$name")" $(rnode_run_common "$name") \
      --bootstrap "rnode://${id}@${reset_bootstrap_host}?protocol=40400&discovery=40404" \
      "${reset_anchor[@]}" \
      --bonds-file /genesis/bonds.txt --wallets-file /genesis/wallets.txt \
      "${reset_validator_key[@]}"
  if [[ "${DEVNET_RESET_NETWORK_PAUSE:-0}" == "1" ]]; then
    docker network disconnect "$NETWORK" "$name"
    echo "reset $name: isolated from $NETWORK until the test reconnects it"
  fi
  echo "reset $name: back up with an empty store; it will LFS-sync the chain from $reset_bootstrap_host"
}

cmd_logs() {
  docker logs -f "${1:?node name required}"
}

# Extract the host port mapped to a container port (e.g. "0.0.0.0:40403" -> "40403").
host_port_for() {
  local c="$1" container_port="$2"
  docker port "$c" "$container_port" 2>/dev/null | head -n1 | sed -n 's/.*:\([0-9]*\)$/\1/p'
}

# check <label> <ok?> — PASS/FAIL; warn <label> — WARN. Both track globals.
check() {
  local label="$1" ok="$2"
  if [[ "$ok" == "0" ]]; then
    echo "  PASS  $label"
  else
    echo "  FAIL  $label"
    FAILURES=$((FAILURES + 1))
  fi
}
warn() {
  echo "  WARN  $1"
  WARNINGS=$((WARNINGS + 1))
}

# diagnose: per-node health report (state, ports, sockets, reachability, syncing, block number);
# non-zero exit on any FAIL.
cmd_diagnose() {
  FAILURES=0
  WARNINGS=0
  local names=("$BOOTSTRAP") i
  for (( i = 1; i <= MAX_VALIDATORS; i++ )); do names+=("$(validator_name "$i")"); done
  for (( i = 1; i <= 3; i++ )); do names+=("$(observer_name "$i")"); done

  for c in "${names[@]}"; do
    docker inspect "$c" >/dev/null 2>&1 || continue
    echo "== $c =="

    local state
    state="$(docker inspect -f '{{.State.Status}}' "$c" 2>/dev/null || true)"
    [[ "$state" == "running" ]]; check "container running" $?

    local grpc_host http_host admin_host
    grpc_host="$(host_port_for "$c" 40401)"
    http_host="$(host_port_for "$c" 40403)"
    admin_host="$(host_port_for "$c" 40405)"
    [[ -n "$grpc_host" ]]; check "deploy gRPC published (40401)" $?
    [[ -n "$http_host" ]]; check "public HTTP published (40403)" $?
    [[ -n "$admin_host" ]]; check "admin HTTP published (40405)" $?

    # Host HTTP reachability + block number + peer counts.
    local body block peers nodes
    if body="$(curl -fsS --max-time 5 "http://localhost:${http_host}/api/v1/status" 2>/dev/null)"; then
      check "GET /api/v1/status reachable" 0
      block="$(printf '%s' "$body" | sed -n 's/.*"latestBlockNumber":\([0-9]*\).*/\1/p')"
      peers="$(printf '%s' "$body" | sed -n 's/.*"peers":\([0-9]*\).*/\1/p')"
      nodes="$(printf '%s' "$body" | sed -n 's/.*"nodes":\([0-9]*\).*/\1/p')"
      if [[ -n "$block" && "$block" -gt 0 ]]; then
        check "block production (latestBlockNumber=$block)" 0
      else
        warn "no blocks yet (latestBlockNumber=0) — idle, still syncing, or not autoproposing"
      fi
      if [[ "$peers" == "0" && "$nodes" == "0" ]]; then
        warn "no peers discovered (peers=0 nodes=0)"
      else
        check "peer connectivity (peers=$peers nodes=$nodes)" 0
      fi
    else
      check "GET /api/v1/status reachable" 1
    fi
  done

  echo ""
  if (( FAILURES > 0 )); then
    echo "==> $FAILURES check(s) FAILED${WARNINGS:+ ($WARNINGS warning(s))}"
    return 1
  fi
  echo "==> all checks passed${WARNINGS:+ ($WARNINGS warning(s))}"
  return 0
}

# Run `rnode` inside a node container (reaches deploy 40401 + propose/repl 40402 via localhost).
node_cli() {
  local node="$1"; shift
  local tty=""
  [[ "${1:-}" == "repl" ]] && tty="-t"
  docker exec -i $tty "$node" rnode --grpc-host localhost "$@"
}

cmd_cli() {
  local node="${1:?node name required}"; shift
  node_cli "$node" "$@"
}

cmd_deploy() {
  local file="${1:?contract file required (relative to examples/)}"; shift
  local node="$BOOTSTRAP"
  if [[ "${1:-}" == "--to" ]]; then node="$(validator_name "${2:?}")"; shift 2; fi
  local base; base="$(basename "$file")"
  # Anchor the deploy to the current height: a `valid_after_block_number = -1` deploy is "valid from
  # genesis" and expires after DEPLOY_LIFESPAN blocks, so on a long-running devnet it would be dropped
  # before it can be proposed.
  local height
  height="$(node_cli "$node" status 2>/dev/null | sed -n 's/.*"latestBlockNumber": *\([0-9]*\).*/\1/p')"
  echo "==> deploying $base to $node (validAfterBlockNumber=${height:-0})"
  node_cli "$node" deploy \
    --phlo-limit 1000000 --phlo-price 1 \
    --private-key "$DEPLOYER_PRIV" \
    --shard-id "$SHARD_ID" \
    --valid-after-block-number "${height:-0}" \
    "/contracts/$base"
}

cmd_eval() {
  local file="${1:?file required (relative to examples/)}"
  local base; base="$(basename "$file")"
  node_cli "$BOOTSTRAP" eval "/contracts/$base"
}

cmd_query() {
  local name="${1:?public name required}"
  # The name is a public (forgeable) name; quote it so the client normalizes it as a rholang
  # *ground string* (matching `@"hello"!("world")`), not as a free variable.
  node_cli "$BOOTSTRAP" listen-data-at-name -t pub -c "\"$name\""
}

# One-shot run of the second (complex) contract: deploy examples/wallet.rho, which derives the
# deployer's REV address, keeps a per-address codeDict map, round-trips save/load, and publishes the
# loaded value on the public name "wallet"; assert the round-trip landed.
cmd_demo() {
  echo "==> deploying the complex wallet contract (examples/wallet.rho)"
  cmd_deploy wallet.rho

  echo "==> waiting for the round-tripped value on the public name 'wallet'"
  local out
  if ! out="$(timeout 120 docker exec -i "$BOOTSTRAP" rnode --grpc-host localhost \
      listen-data-at-name -t pub -c '"wallet"')"; then
    echo "ERROR: query timed out or failed" >&2
    return 1
  fi
  echo "$out"
  if ! grep -q 'world' <<<"$out"; then
    echo "ERROR: expected the wallet contract to round-trip 'world', got:" >&2
    echo "$out" >&2
    return 1
  fi
  echo "==> wallet contract round-trip OK"
}

cmd_faucet() {
  local addr="${1:?REV address required}"
  if ! docker ps --format '{{.Names}}' | grep -q "^${BOOTSTRAP}$"; then
    echo "ERROR: $BOOTSTRAP is not running — start it with 'up --validators 1'." >&2
    exit 2
  fi
  # The faucet is served on the public HTTP API (dev-mode only); it transfers 0.3 REV from the
  # genesis-funded deployer wallet to the requested address.
  curl -s -X POST "http://localhost:${HTTP_BASE}/api/v1/faucet" \
    -H 'Content-Type: application/json' \
    -d "{\"address\":\"${addr}\"}"
  echo
}

cmd_propose() {
  if [[ "${1:-}" == "--admin" ]]; then
    if ! docker port "$BOOTSTRAP" 40405 >/dev/null 2>&1; then
      echo "ERROR: admin HTTP (40405) is not published — restart with 'up --admin'." >&2
      exit 2
    fi
    curl -s -X POST "http://localhost:${ADMIN_BASE}/api/v1/propose"
  else
    node_cli "$BOOTSTRAP" propose
  fi
}

cmd_bench() {
  # Benchmark a **running** devnet. It deliberately does not start one: the numbers depend on the
  # configuration under test (autopropose off, for rates that are the node's rather than the timer's;
  # `--epoch-length 1`, to measure the boundary block), and that choice belongs to whoever started
  # the node. `tools/devnet-bench.py` says so and fails with a clear message when nothing answers.
  command -v python3 >/dev/null 2>&1 || { echo "bench needs python3" >&2; exit 2; }
  exec python3 "$(cd "$(dirname "$0")" && pwd)/devnet-bench.py" "$@"
}

case "${1:-}" in
  build) shift; cmd_build "$@" ;;
  up) shift; cmd_up "$@" ;;
  down) cmd_down "${2:-}" ;;
  stop) cmd_stop "${2:-}" ;;
  start) cmd_start "${2:-}" ;;
  reset) cmd_reset "${2:-}" ;;
  status) cmd_status ;;
  logs) cmd_logs "${2:-}" ;;
  diagnose) cmd_diagnose ;;
  deploy) shift; cmd_deploy "$@" ;;
  eval) shift; cmd_eval "$@" ;;
  query) shift; cmd_query "$@" ;;
  faucet) shift; cmd_faucet "$@" ;;
  demo) cmd_demo ;;
  propose) shift; cmd_propose "${1:-}" ;;
  bench) shift; cmd_bench "$@" ;;
  cli) shift; cmd_cli "$@" ;;
  help|--help|-h) help ;;
  *) help ;;
esac
