# syntax=docker/dockerfile:1

# --- Build stage ---------------------------------------------------------------
#
# **Pinned by digest, not by tag.** A tag is a mutable name: whoever controls the upstream image can
# repoint `rust:1.95.0` at different content and the artifact that ships changes with no commit here.
# A digest cannot be repointed, so the base the binary was actually built on is a fact rather than a
# promise (W-N6-07). Re-derive either digest with
#   docker buildx imagetools inspect <ref>
# and read the top-level `Digest:` — it is the *index* digest, so both platforms stay served.
FROM rust:1.95.0@sha256:f49565f188ee00bc2a18dd418183f2c5f23ef7d6e691890517ed341a598f67c3 AS builder

# `protoc` (prost-build .proto codegen) + the protobuf well-known types
# (`google/protobuf/empty.proto` etc., shipped by libprotobuf-dev) + build-essential
# + perl (ring/lmdb-rkv C builds, already in the full rust image).
RUN apt-get update \
    && apt-get install -y --no-install-recommends protobuf-compiler libprotobuf-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY . .

# The revision this image is built from, so the binary can name it (`/version`, `/status`, `--version`).
#
# **The build argument exists because the context cannot answer it.** `.dockerignore` excludes `.git/`,
# so `node/build.rs`'s `git rev-parse HEAD` finds nothing here and the image served
# `rholo node 0.1.0 (commit # unknown)` — measured, and the exact question the provenance work
# (issue #32) set out to answer for the artifact that actually ships. `tools/devnet.sh build` passes
# `--build-arg GIT_HEAD_COMMIT=$(git rev-parse HEAD)`; a plain `docker build` without it keeps the
# honest `commit # unknown` rather than inventing one.
#
# **It is honoured only where git cannot answer** (W-N7-01). `node/build.rs` gives git precedence and
# warns when the two disagree, so setting this to a canonical hash while building modified source no
# longer makes `/version` name a commit the binary was not built from. Here git cannot answer, which
# is the one case the argument is for.
ARG GIT_HEAD_COMMIT=""

# `--locked`: the shipped binary is built from the `Cargo.lock` that was reviewed, not from whatever
# resolution the registry happened to produce at build time (W-N7-04). CI builds the release the same
# way, so a lockfile drift fails here rather than shipping a differently-resolved binary.
#
# **No `RUSTFLAGS`/`CARGO_FEATURES` build argument on the shipped artifact.** A build argument is
# caller-controlled, so one that reaches `cargo` lets a caller compile a *different* binary under the
# audited tag — and the two that used to be here existed for the #117 memory profiling, which is not
# the artifact that ships. That build has its own definition: `docker/rnode/Dockerfile.profiling`.
RUN cargo build --release --locked -p rchain-node --bin rnode

# --- Runtime stage -------------------------------------------------------------
# Must match the builder's glibc (rust:1.95.0 = Debian trixie / glibc 2.41).
FROM debian:trixie-slim@sha256:a29215f6a35e51e22adffa17f89e9d2ef06214e64a2bad10d765c46aea49f11f

# **The runtime stage carries the revision too.** The builder's `ARG` is visible only there; a
# preflight that inspects the image cannot read an `oci` label that was never emitted, and a run
# against an unlabelled image is exactly the "which binary failed?" question this label answers.
ARG GIT_HEAD_COMMIT=""
LABEL org.opencontainers.image.revision=$GIT_HEAD_COMMIT

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/rnode /usr/local/bin/rnode

# `--profile docker` points `storage.data-dir` at /var/lib/rnode.
RUN useradd --create-home rnode \
    && mkdir -p /var/lib/rnode \
    && chown -R rnode:rnode /var/lib/rnode

# Healthy once the public HTTP API answers /api/status.
HEALTHCHECK --interval=10s --timeout=3s --start-period=30s --retries=5 \
    CMD curl -fsS http://localhost:40403/api/status || exit 1

USER rnode
WORKDIR /var/lib/rnode

ENTRYPOINT ["rnode"]
