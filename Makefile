# RNodeRust test targets.
#
# Layered suites:
#   test-unit        inline #[test]/#[tokio::test] unit tests across all crates
#   test-integration in-process node/casper/rholang integration tests (node/tests, casper/tests, ...)
#   test-multinode    multi-node consensus harness (casper/tests/multinode.rs)
#   test-all          unit + integration (default)
#   status            what the audit has left, and whether the check-off is current
#   coverage          line/region coverage via cargo-llvm-cov
#   bench-smoke       compile the Criterion benches so they cannot rot
#
# `--all-features` matters for the unit/integration suites: the `lmdb`-gated tests in
# `shared/src/lmdb.rs` and `rspace/src/state/exporters.rs` are silently skipped without it, which is
# how they escaped local runs while CI (`--all-features`) still exercised them.

.PHONY: test test-unit test-integration test-multinode test-all check-register check-lean check-docs coverage coverage-ledger bench-scheduler bench-smoke spec deep status check

test: test-all

# The loop: what is left, and is the check-off telling the truth. **Seconds.**
#
# Measured: ~2.5 s warm, dominated by the evidence check's single pass over the tracked tree. That is
# the number to defend. It replaced a 78-second register gate whose cost was almost entirely in checks
# that proved documents agreed with each other.
#
# This is the first thing to run, before any gate, because "what is left" is the question; a wall of
# `ok` from a gate that finished is the answer to a question nobody asked yet.
status:
	tools/audit-status.sh

# What a change gets: the status, then the formatter. The third thing is the crate's own test filter
# -- `cargo test -p <crate> <filter>`, 0.2-2 s warm -- and it is not wrapped here because the crate
# and the filter are exactly the parts that change per change.
#
# `check-register` is ~35 s of static gates over the whole tree: a boundary, not a per-edit step. See
# the `deep` target's note and `docs/src/contributor/laws-to-rust.md`.
check: status
	cargo fmt --all --check

test-unit:
	cargo test --workspace --lib --all-features

test-integration:
	cargo test -p rchain-node -p rchain-casper -p rchain-rholang --tests --all-features

test-multinode:
	cargo test -p rchain-casper --test multinode --all-features

test-all: test-unit test-integration

# The static gates that catch a defect in the *node*: a production `unwrap()`/`panic!`/`unsafe`, a
# silent fallible conversion, a refinement surrendering its invariant — and an edit to the oracle's
# own vendored source text, which is a specification change wearing a comment's clothes.
#
# **What used to be here and is not.** `tools/audit-test-register.sh` was 1,453 lines and 78.5 s over
# 17 checks, and its failures were of one kind: a register document disagreed with the tree. Nothing
# production-facing reads those documents, and the gate had itself been wrong more often than it had
# been right about the node — so it was deleted on 2026-09-27 rather than maintained. The check-off
# (`spec/AUDIT.md`, `tools/audit-status.sh`) is what a reader wants from that material, and it costs
# 2.5 seconds.
check-register:
	tools/audit-status.sh --quiet
	tools/audit-type-system.sh
	tools/audit-vendored-sources.sh
	tools/check-workflow-pins.sh
	tools/check-review-ledger.sh --gate
	tools/check-hazop-worksheet.sh --gate
	tools/check-spawn-observability.sh --gate

# The book's own gates, plus the artifact record and the law-count spans. `mdbook build` is here so a
# page that mdbook cannot render fails locally before it reaches CI; the link check resolves relative
# links and `#fragment`s, which mdbook renders as dead anchors without an error.
check-docs:
	tools/check-docs-links.sh --gate
	tools/recount-artifact.sh --check
	tools/emit-lean-counts.sh --check
	mdbook build docs

# The formal gate: build the Lean and Coq specifications, refuse a stale or un-consumed conformance
# corpus, and check the Rust 1:1 against it. `spec` (below) is only the Lean half — this is what CI
# runs, and what a change to `spec/` must satisfy before it is a change to the specification.
check-lean:
	tools/check-lean-conformance.sh

# The deep gates, on demand — the same two the nightly runs, in the same order of cost.
#
# **Do not run this per change.** `check-lean` is ~104 serial `cargo test` invocations behind a Lean
# build that is 16 minutes cold; `coverage` moves the whole workspace through an instrumented codegen
# profile, so it reuses none of the warm `test` cache. Each is a thing to run at the end of a body of
# work, or to leave to `.github/workflows/nightly.yml`.
#
# What *is* per change: `cargo fmt --all --check`, `cargo clippy`, `cargo test -p <crate> <filter>`
# (0.2-2 s warm), `make status` (2.5 s) and `tools/audit-type-system.sh` (~30 s). See
# `docs/src/contributor/laws-to-rust.md`.
deep: check-lean coverage

# Coverage. CI has always run the whole workspace with
# `--all-features` and no exclusions, and is green on every PR — so the crypto crate is *not* flaky
# under instrumentation, and the local `--exclude rchain-crypto` that used to sit here was drift that
# made the local number disagree with the gate. Both now run the same command.
#
# The floor lives in CI (`--fail-under-lines`): raise it only after measuring, never to a number the
# plan hopes to reach. `cargo install cargo-llvm-cov` + `rustup component add llvm-tools-preview`
# are the prerequisites locally.
coverage:
	cargo llvm-cov --workspace --all-features --lcov --output-path lcov.info
	tools/emit-coverage-ledger.sh

# Re-emit `spec/COVERAGE-LEDGER.md` from the committed `lcov.info` without re-running the suite —
# for the case where only the emitter changed. `make coverage` does both, because a measurement that
# is not emitted is a number nobody reads.
coverage-ledger:
	tools/emit-coverage-ledger.sh

# The channel-scheduler benchmarks (Laws 20–22): pingpong/fanout workloads, the dfs/gate/relaxed
# mode sweep, and the striped-vs-unstriped hot-store comparison. Run with cargo and lake strictly
# serialized (each peaks ~6.5 GiB RAM).
bench-scheduler:
	cargo bench -p rspace-bench -- sched/

# Compile every bench target so an API change that breaks them fails a gate rather than going
# unnoticed until someone runs the benchmarks. `bench-scheduler` is the one that executes.
bench-smoke:
	cargo bench -p rspace-bench --no-run

spec:
	cd spec && lake build
