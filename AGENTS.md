# AGENTS.md — RChain → hardened Rust node: intent & formal specification

This file is the **authoritative intent + formal specification** for rewriting this node in Rust. It
is written for both AI coding agents and humans: read it in full before writing or changing any Rust
code, and treat its statements as binding constraints, not suggestions.

## Documentation map

Single sources of truth (do not duplicate these). The curated book is `docs/src/` (built with
`mdbook build docs`); the **software** documentation is Parts I–III, the **port** documentation is Part
IV.

| Content | Canonical location |
|---|---|
| The rholang language & the ρ-calculus (the software) | [`docs/src/rholang/`](docs/src/rholang/) (book Part I) |
| The ρ-calculus, formally — grammar, sorts, the law mapping | [`docs/src/formal/`](docs/src/formal/) (book Part II) |
| The node — consensus, RSpace, storage, operation | [`docs/src/node/`](docs/src/node/) (book Part III) |
| Running/operating the node — REPL, standalone genesis, ports, Docker network | [`docs/src/node/operating.md`](docs/src/node/operating.md) |
| Reader/agent navigation map (goal-indexed) | [`docs/src/ai-entrypoint.md`](docs/src/ai-entrypoint.md) |
| The ρ-calculus core spec (grammar, sorts, operations, refinements) | [`spec/RHO-CALCULUS.md`](spec/RHO-CALCULUS.md) |
| The law register — every invariant, its checked status, axioms and Rust realization | [`spec/LAWS.md`](spec/LAWS.md) (long-form catalogue: [`spec/INVENTORY.md`](spec/INVENTORY.md)) |
| Operating manual for the register — numbering, statuses, commands, the Lean-first protocol | [`docs/src/ai/laws-reference.md`](docs/src/ai/laws-reference.md) |
| Human-facing walkthrough: each law → concrete Rust file/type/function + test | [`docs/src/contributor/laws-to-rust.md`](docs/src/contributor/laws-to-rust.md) |
| The ρ→CoC type-system spec | [`spec/TYPE-SYSTEM.md`](spec/TYPE-SYSTEM.md) |
| How Rust made the Scala fragility explicit (bugs caught, production-readiness) | [`spec/RUST-VS-SCALA.md`](spec/RUST-VS-SCALA.md) |
| What the language let the architecture *build* — the architectural gains, each with its artefact | [`spec/RUST-AFFORDANCES.md`](spec/RUST-AFFORDANCES.md) |
| **Audit check-off** — what the adversarial audit has left, in three states (`todo` · `in progress` · `done`), with the T1 coverage that bounds any claim it makes (all 89 modules read, closed 2026-09-27). Start here. | [`spec/AUDIT.md`](spec/AUDIT.md) |
| The audit's **pass record** — the evidence behind every check-off row: one section per pass, and the Scala-deviation register (§6). This is what a row's `§` cites. | [`spec/audit/passes.md`](spec/audit/passes.md) |
| Native system contracts (registry/PoS/vault state model + replay determinism) | [`spec/RUST-FIRST.md`](spec/RUST-FIRST.md) |
| Test-coverage audit & gap analysis — machine-checked: the per-crate inventory, the law property matrix, the risk tiers, the exempt-module table, and the census that requires every source file to be tested or exempt | [`spec/TEST-COVERAGE.md`](spec/TEST-COVERAGE.md) |
| Machine-checked Lean/Coq definitions & proofs | [`spec/`](spec/) |
| Why the rewrite + layer map / module status (port appendix) | [`docs/src/contributor/why-rust.md`](docs/src/contributor/why-rust.md), [`docs/src/contributor/architecture.md`](docs/src/contributor/architecture.md) |

### Learning rholang (AI navigation)

For an agent that needs to *understand the language* (rather than port code), the shortest path is:

1. [`docs/src/rholang/why-rholang.md`](docs/src/rholang/why-rholang.md) → the model and why it fits a
   blockchain.
2. [`docs/src/rholang/processes-names.md`](docs/src/rholang/processes-names.md) →
   [`docs/src/rholang/unforgeable-names.md`](docs/src/rholang/unforgeable-names.md) → the core
   constructs.
3. [`docs/src/formal/grammar-sorts.md`](docs/src/formal/grammar-sorts.md) +
   [`docs/src/formal/laws.md`](docs/src/formal/laws.md) → the precise semantics.
4. [`docs/src/ai-entrypoint.md`](docs/src/ai-entrypoint.md) → any other goal (consensus, capabilities,
   the port).

The formal oracle is `spec/`; the book explains it, it does not duplicate it.

### The documentation network

**Two rules decide where a document lives. Neither is enforced by a tool; both are enforced by this
section being read.**

- **Quarantine.** Plans, roadmaps and hypotheticals are labelled as such and live *outside* the
  repository (today that is `~/.claude/plans/`). The book and `spec/` describe what compiles and what
  is proved. A claim in a document that cannot be traced to a `.rs`, `.lean` or `.v` file is a claim a
  reader will take as implemented — so a document making one says so in the sentence that makes it,
  and links the artifact.
- **A page has exactly one parent.** Every page in the book is listed in
  [`docs/src/SUMMARY.md`](docs/src/SUMMARY.md); every `spec/` document is reachable from
  [`spec/README.md`](spec/README.md) or from this file. The single sources of truth are the table
  above: where the book explains a law it links `spec/`, and does not restate it.

**Routing is intent-based in two places, and they are not duplicates.** The README's *Documentation*
section routes by part (the book's I–VI). [`docs/src/ai-entrypoint.md`](docs/src/ai-entrypoint.md)
routes by goal — "I want to…" — for readers and agents, and mirrors the map above. A new page goes in
`SUMMARY.md`, and into the goal table if it answers a question someone would arrive with.

**The orphan audit, run 2026-09-28.** Every tracked `.md` outside `legacy/` was resolved as a link
target across every other tracked `.md`, expanding each `](target)` against the linking file's own
directory. It is short enough to re-run as it stands:

```sh
for f in $(git ls-files '*.md' | grep -v '^legacy/'); do
  grep -ohE '\]\([^)#]*\)' "$f" 2>/dev/null | sed -E 's/^\]\(|\)$//g' | grep -v '^http' \
    | while read -r t; do readlink -m "$(dirname "$f")/$t"; done
done | sed "s|^$PWD/||" | sort -u > /tmp/tgt
git ls-files '*.md' | grep -v '^legacy/' | grep -vxF -f /tmp/tgt
```

**It printed two files then. Re-run on 2026-10-11 it prints 62, and the difference is not drift in the
rule but growth in the tree.** The two are still there — `spec/STYLE.md` and `spec/coq/README.md`, both
**book-orphans rather than orphans**: `spec/STYLE.md` is named in `spec/README.md`'s layout tree, and
`spec/coq/README.md` is the README of a directory this file and `spec/README.md` both link — so neither
is unreachable, but neither is a *clickable* target anywhere, and a reader who follows links rather than
browsing the tree does not arrive at them.

**Of the other 60, 59 are evidence artefacts, and the census is asking the wrong question about them.**
`spec/audit/evidence/**` is cited from a register row's **evidence cell** — `spec/findings.tsv`,
rendered into `spec/AUDIT.md` — and this snippet resolves `](target)` links only, so a file *named in a
table* is counted here as named nowhere. The rule is "reachable as a link target", and the evidence
tree is reachable by a mechanism the rule does not read. That is a limit of the instrument, stated
rather than papered over: widening the census to read `.tsv` cells would make it a *different* check,
and one nothing else re-runs.

**The one that is genuinely unreferenced is `HANDOVER.md`** — a root handover note, no incoming link
anywhere, and deliberately so: it is a working document, not a page of the book or of `spec/`.

**The earlier run, kept for the lesson.** The run before this section existed printed three; the third
was `docs/src/SUMMARY.md`, the book's index, a root by construction — and the `SUMMARY.md` link in the
first rule above is what gave it an incoming link, so re-running the snippet on a tree without that link
restores it. The number this audit reports is a property of the tree it is run on, *including this
file* — and the figure has gone from 2 to 62 without a single book-orphan being added, which is exactly
why an audit that edits the thing it measures should say so.

**The Lean side has no orphans at all**: the two modules nothing imports, `Rchain/Corpus.lean` and
`Rchain/LawsMain.lean`, are the conformance emitter and the law-register checker — `lean_exe` roots,
named in `spec/lakefile.toml`'s `defaultTargets`, which is load-bearing, because an `lean_exe` root
is not pulled in by the library target and a bare `lake build` would otherwise compile neither.

**Not adopted here: a review process enforced on every documentation PR.** The discussion this section
answers asks for one — a "network usability test" applied to every doc change. It is not recorded as
adopted, because a policy nothing checks reads as a policy that holds. What does hold is the narrower
and true thing: the two rules above, the audit's residue stated above, and the registers that *are*
gated (`spec/AUDIT.md`'s check-off, the law register, the test-coverage census, the cost register). Whether a doc PR gets
a checklist is a maintainer's decision rather than an edit to this file.

## Intent

The RChain node runs Rholang natively. We are rewriting it in **Rust**, absorbing both the Scala/JVM
code and the C++ Rosette VM. The motivation — memory safety and the calculus-native expression of the
node (λ → π → ρ → Calculus of Constructions) — is laid out in
[`docs/src/contributor/why-rust.md`](docs/src/contributor/why-rust.md).

**Prime directive:** the Scala/JVM + Rosette *port* is complete; the node is now a **faithful
implementation of the ρ-calculus**. The oracle is the mathematical specification — the laws in
[`spec/INVENTORY.md`](spec/INVENTORY.md) and the ρ→CoC type discipline in
[`spec/TYPE-SYSTEM.md`](spec/TYPE-SYSTEM.md) — **not** the Scala code. Implement each law using Rust's
strengths: carry the invariants *structurally* in the type system (refinement types, no silent
partiality), rather than mechanically reproducing Scala's patterns. Where the Scala code and the
specification disagree, the specification is correct and the code is brought into line — a latent
Scala bug (e.g. wrapping a negative cost into a `uint64`) is **not** preserved; such deviations are
recorded in the Scala-deviation register, [`spec/audit/passes.md`](spec/audit/passes.md) §6.

## The target, and how work is tracked

**The oracle is done.** `spec/laws.tsv` reads **0 `owed`, 1 `open`** across
<!-- counts:entries -->131 entries<!-- counts:end --> — <!-- counts:proved-model-entries -->86<!-- counts:end --> `proved-model`, <!-- counts:proved-tied-entries -->36<!-- counts:end -->
`proved-tied`, <!-- counts:axioms -->10 axioms<!-- counts:end --> (9 crypto by design, 1 named) — the single open row
(law 63a, a bridged deploy's attribution) being a cross-implementation relay this tree cannot reach
rather than a proof debt, so the mathematical programme the prime directive sets is finished. Adding a
law, or re-opening a proof, is a deliberate act rather than the default next step.

**What remains is operational, and it is one sentence:** `docs/src/node/testnet.md` says *"this net takes one
validator on purpose, and adding a second is unsafe today"*. The target is to lift that — **a net of ≥2
validators that finalises.** The four defects named here as the obstruction (C171, C192, C190+C193, C173)
are all `done`; so are the two that followed — **C209** (the attestation guard read the round's snapshot,
which is the genesis alone before the first round closes, so a validator that had just spoken saw nobody
moving and never spoke again) and **C210** (a refused propose on a quiet net now arms one retry). **Both
acceptance criteria pass on a live net**, whose falsifiers are the
[testnet acceptance specification](docs/src/spec/testnet-acceptance.md) §3.1 and §3.2. What remains on the
public gate is a **re-verification on the two-host net**, not a defect; #213 and #214 stay open pending
their authors' close. The falsifiers that
decide the question are the [testnet acceptance specification](docs/src/spec/testnet-acceptance.md).

**Four rules, because the register and the tracker had grown into two records of one thing.**

1. **A close condition names an observable behaviour** — a test, a measurement, a run. Paperwork — a register
   entry, a proof, a "decision recorded" — is evidence *for* the row, never a conjunct of the condition. A
   condition that conjoins the two cannot be closed by finishing the work, which is how #70 came to be
   closed, reopened, and rolled forward.
2. **A finding goes to the register.** An issue is opened only when someone commits to the work, and it names
   the owner and the next action. A finding never spawns an issue by itself.
3. **A finding has one home** — its row in `spec/findings.tsv`, rendered into `spec/AUDIT.md`. A
   `spec/audit/passes.md` entry is optional and **short**: the mechanism, the evidence, what it rules out.
4. **Reserve the C-number when the branch is opened, not when the finding is written**, and let
   `tools/next-audit-number.sh` read the remote branches. A number taken from one tree is a reservation, not
   a fact — four pull requests in one day were the same work re-filed under a new number.

**Every open issue names the target it advances, or it closes.** The register is the record; the tracker is
the worklist.

## How to use this file

For any component you are about to write in Rust:

1. Find its layer below and its law numbers in [`spec/INVENTORY.md`](spec/INVENTORY.md).
2. Read the corresponding formalization (Lean 4 and/or Coq) and the law's invariant statement; the
   Scala/C++ file is reference material for the ported behavior, not the oracle.
3. Read the ground-truth Scala test that already encodes the law.
4. Write Rust that satisfies the law, gated by a property test and a differential test (see
   *Translation contract*).

## The formal specifications

| Track | Scope | Location | Build |
|-------|-------|----------|-------|
| **Lean 4** (primary) | algebraic/order laws, canonicalization, merge monoids, consensus arithmetic | [`spec/`](spec/) | `cd spec && lake build` |
| **Coq** | substitution, α-equivalence, and programming-language metatheory (Autosubst in Phase 1) | [`spec/coq/`](spec/coq/) | `make -C spec/coq` |
| **Register** | the laws, each with status, declarations, axioms and falsifier | [`spec/LAWS.md`](spec/LAWS.md) (long form: [`spec/INVENTORY.md`](spec/INVENTORY.md)) | — |
| **Type system** | the port's own type discipline: ρ-calculus as the base sort of a Calculus of Constructions, no silent partiality | [`spec/TYPE-SYSTEM.md`](spec/TYPE-SYSTEM.md), `Rchain/Rho.lean`, `Rchain/Ty.lean` | `cd spec && lake build` |

The **type-system spec** ([`spec/TYPE-SYSTEM.md`](spec/TYPE-SYSTEM.md)) overlaps the Lean/Coq split
deliberately: Lean 4 proves the six fundamentals over the flat `Par` (sort classification, `≡`,
minimal substitution, minimal COMM reduction, canonicalization, totality); Coq keeps the deep
Autosubst α-equivalence reconciliation. It is a hardening of the port, not a new law.

The Lean 4 and Coq tracks are deliberately parallel: both define the same core `Proc` syntax and the
same canonicalization (`sort`) as Phase 0, and both state Law 1 (`sort` is idempotent and `par`
commutative) as a Phase 1 proof obligation. Coq is the home of the substitution metatheory (capture-
avoiding de Bruijn substitution, α-equivalence); Lean 4 is the home of the algebraic/order laws.

## Formal proof plan — rspace, rholang & Rosette

The two Meredith-designed cores — the **rholang executor** (`rholang/`, the ρ-calculus interpreter)
and **RSpace** (`rspace/`, the concurrent tuple space) — get the deepest machine-checked treatment, in
both **Coq** and **Lean 4**, *before* their Rust is written (**proofs-first**). The **Rosette VM**
(`rosette/`, `roscala/`) is also **in scope for the formalization** (Laws 12–13: actor atomicity,
reflection), formalized in a later phase. Note this is independent of the *rewrite*, where
`rosette`/`roscala` are deferred (orphaned). The split is by
strength:

| Tool | Laws | Scope |
|------|------|-------|
| **Coq** (Autosubst, de Bruijn) | 2–6 | ρ-calculus PL metatheory: α-equivalence, structural congruence `≡`, capture-avoiding substitution, reduction (comm), spatial matching, free variables |
| **Lean 4** (Mathlib) | 1, 7–11 | order/algebra: canonicalization (`sort`), RSpace join commutativity, deterministic COMM, merge monoid, Merkle determinism, replay determinism |

**Proofs-first policy** (historical): the original plan paused the rewrite until Laws 1–11 were
proven. The rewrite is now complete (see Status); the formalization continues in parallel as the
residual proof track, and the specification remains the oracle for any subsequent change to the Rust
code.

### Phase sequence

- **P0** — full ADT in both tools; add Mathlib (Lean) and Autosubst (Coq); pin Meredith citations.
- **P1** — Law 1 (canonicalization) in both tools — the shared linchpin.
- **P2** — Coq: α-equivalence + substitution (Laws 2–3).
- **P3** — Coq: reduction + matching + free variables (Laws 4–6).
- **P4** — Lean: RSpace (Laws 7–11).
- **P5** — reconcile, update status, resume the rewrite.

### Meredith lineage (foundational references)

- Meredith & Radestock, *A Reflective Higher-Order Calculus* (2005) — the ρ-calculus.
- Meredith, *Higher Category Models of the π-Calculus* — the categorical semantics.
- The RChain architecture / RSpace model.
- In-repo executable semantics: `rholang/src/main/k/rholang/*.k` (`name-equivalence.k`,
  `processes-semantics.k`, `sending-receiving.k`, `matching-function.k`, `free.k`).

Per-law proof status lives in the register, [`spec/LAWS.md`](spec/LAWS.md); the long-form catalogue is
[`spec/INVENTORY.md`](spec/INVENTORY.md). The numbering key and the protocol for changing a law are in
[`docs/src/ai/laws-reference.md`](docs/src/ai/laws-reference.md).

## What must be preserved

The full law table, with line-level source pointers, lives in [`spec/INVENTORY.md`](spec/INVENTORY.md);
the register is the authority for status, and it is not repeated here.

**Proven vs. axiomatized:** the algebraic/combinatorial laws are provable statements — Law 1
(idempotence/commutativity), Law 2's core (≡), Law 4's core (COMM), and Law 6 (`Closed`) are already
**proven** in `Rchain/`; the rest are **stated** (precise signature, definition deferred); Laws 12–13
(Rosette VM) are **orphaned** (out of scope — the Rust reducer replaces the VM). Cryptographic
primitives (Blake2b, secp256k1, Curve25519 — Law 19) are **axiomatized** — modeled as abstract
interfaces whose required properties are postulated, not proven. Liveness (eventual finality) is an
open question, not an inductive invariant.

## Layer map

- **Rholang** (`rholang/`, `models/`) — the ρ-calculus interpreter (canonical order, substitution,
  reduction, spatial matching).
- **RSpace** (`rspace/`) — the concurrent tuple space (join commutativity, deterministic COMM, merge
  monoid, Merkle radix trie, replay).
- **Rosette** (`rosette/`, `roscala/`) — the C++ actor VM (actor atomicity, reflection, fork-join).
- **Casper** (`casper/`, `block-storage/`, `sdk/`) — CBC-Casper consensus + DAG (>2/3 finality,
  fringe/estimator, block validation, merge determinism).
- **Crypto** (`crypto/`) — Blake2b256, `Blake2b512Random`, secp256k1, Curve25519.

Per-layer Scala source-of-truth files are listed in
[`docs/src/contributor/architecture.md`](docs/src/contributor/architecture.md).

## Translation contract (spec → Rust)

For every law in the inventory:

1. **Property test** — a `proptest`/`quickcheck` property asserting the law on the Rust
   implementation (e.g. `sort(sort(p)) == sort(p)`, merge is associative/commutative).
2. **Differential test** — feed identical inputs to the Scala node and the Rust node and compare
   state hashes / results; they must match exactly.
3. **Type-level fidelity** — the Lean/Coq types are the reference for the Rust data model and its
   `Ord`/`Hash`/`Eq` derivations. In particular `sort` becomes the `Ord` implementation that makes
   `Par` an order-insensitive (canonical) container.
4. **Axiomatized crypto** maps to Rust traits whose implementations are the *only* places the
   primitive may differ in behavior, and are pinned by known-answer test vectors.

## Ground truth

**Where the Scala is.** The `legacy/` tree was archived out of the working tree on 2026-09-28 —
<!-- recount:legacy-mib -->26.2<!-- recount:end --> MiB
(<!-- recount:legacy-bytes -->27,425,437<!-- recount:end --> bytes) across
<!-- recount:legacy-files -->1,596<!-- recount:end --> files that no CI job built, tested or scanned,
carrying a 2020-21 dependency manifest
`cargo-deny` cannot see. It is readable at the revision that froze it, **`1b7583649`**:
`https://github.com/rhovision-org/rholo/blob/1b7583649/legacy/<path>`, or locally with
`git checkout 1b7583649 -- legacy`. Every `legacy/...` path cited anywhere in this repository — here,
in `spec/`, in the book, in code comments — resolves at that revision, which is why it is recorded
rather than left to a deletion commit's message. See `README.md`'s *Where the Scala went*.

The oracle is the invariant catalog + the machine-checked formalization. The Scala tests below encode
the laws and remain **differential reference vectors** — the Rust implementation must agree with them
on every law, since they pin the ρ-calculus behavior:

- `rholang/src/test/scala/coop/rchain/rholang/interpreter/{ReduceSpec,ReplaySpec}.scala`
- `models/src/test/scala/coop/rchain/models/rholang/SortTest.scala`
- `node/src/test/scala/coop/rchain/node/mergeablity/MergeabilityRules.scala`
- `casper/src/test/scala/coop/rchain/casper/batch1/MultiParentCasperReportingSpec.scala`

A divergence from the *specification* is fixed in the Rust code and recorded in the
[`spec/AUDIT.md`](spec/AUDIT.md) check-off and its [`pass record`](spec/audit/passes.md); it is **not** propagated to stay byte-identical with a Scala bug.

## Module scoping & rewrite order

The rewrite order is dependency-driven and easiest-first; it deliberately differs from the
*formalization* phases above (which rank by invariant value). Both run in parallel: laws are proven
in phase order while code is ported in dependency order. The per-module LOC/difficulty/person-day
ratings, the full bottom-up order, and the workspace layout live in
[`docs/src/contributor/architecture.md`](docs/src/contributor/architecture.md).

Bottom-up order: `sdk` → `shared` → `crypto` + `graphz` → `models` →
`block-storage` + `rspace` + `comm` → `rholang` → `casper` → `node` → `rspace-bench`. **Defer**
`roscala`/`rosette`.

### Findings

- **`rosette`/`roscala` are orphaned** (absent from `build.sbt`, imported by nothing) — deferred.
- **Hoist `Blake2b256Hash`** out of `rspace` into `crypto`/`shared` so `models` stops depending on
  `rspace` (`models/.../ByteStringSyntax.scala`, `FringeData.scala`, `BlockMetadata.scala`).
  **Done**: `Blake2b256Hash` is `crypto/src/hash/blake2b256_hash.rs` and `models/Cargo.toml` has no
  `rspace` dependency.

## Running the node

Build and run the `rnode` binary (Rust pinned in `rust-toolchain.toml`):

```sh
cargo build --release -p rchain-node --bin rnode
rnode run -s \
  --validator-private-key-path /var/lib/rnode/validator.hex \
  --host 127.0.0.1 --api-host 127.0.0.1 --no-upnp
```

Standalone (`run -s`) is the **genesis master**: it needs a secp256k1 key file of 32 bytes base16
through `--validator-private-key-path` — there is no flag that takes the hex itself, because that put
the scalar in `/proc/<pid>/cmdline` (W-N11-05) — and a
`~/.rnode/genesis/wallets.txt` (must exist; may be empty — `bonds.txt` auto-generates). The API host
must be a literal IP (`SocketAddr::from_str` rejects hostnames like `localhost`). Deploy serves on
**40401**; Propose/Repl on loopback **40402**. Full instructions:
[`docs/src/node/operating.md`](docs/src/node/operating.md).

## Open questions

1. `legacy/casper/src/main/resources/casper.tla` (the Scala tree's copy; the port does not carry it)
   models only the genesis **bootstrap ceremony**, not the
   finality rule — the formal finality spec must be reconstructed from `Finalizer.scala` and
   `MessageMapSyntax.scala`.
2. The `faultTolerance` field is asserted by the **legacy** suite's
   `legacy/integration-tests/test/test_dag_correctness.py`, which is not in this tree: the port's
   integration checklist *mirrors* that suite (`tools/run-integration-tests.sh:34`), it does not run it,
   so nothing here computes the field. Recorded as legacy-only rather than as an owed formula.
3. **Rosette scope** — two independent decisions: (a) **formalization**: the Rosette VM is **in
   scope** (Laws 12–13, actor atomicity + reflection, a later proof phase); (b) **rewrite**:
   `rosette`/`roscala` are **deferred** — orphaned (absent from `build.sbt`, imported by nothing).

## Status

- **Phase 0 — complete**: Lean 4 skeleton (`spec/`), Coq skeleton (`spec/coq/`), the law
  inventory, and this document.
- **Channel scheduler (Laws 20–22) — implemented**: the per-channel claim queue (`rspace/src/
  concurrent/channel_queue.rs`), the DFS gate and relaxed effect modes (`rholang/src/scheduler.rs`
  + `rholang/src/reduce.rs`), the scheduled produce phase-one/two split
  (`rspace/src/scheduled_space.rs`), the `--effect-scheduler {dfs,gate,relaxed}` node flag, and the
  casper block-path hard-reject for `relaxed` (off-chain only — explore-deploy is its outlet). The
  Lean formalization is in `spec/Rchain/Scheduler.lean` — `queue_commit_path_ordered` and
  `pathSorted_head_minimal` (law 20), `gate_await_closure_orders` and `one_hop_depth2_diverges`
  (law 21). **This bullet used to cite `gate_exec_refines_apply` and `next_step_closure_computable`,
  and neither exists**: the first proved the fold was the fold, and the second was `by rfl` — both were
  deleted as saying nothing, which is law 22's `vacuous` status and the register's own finding. The
  reader-facing spec is [`docs/src/formal/scheduling.md`](docs/src/formal/scheduling.md).
- **On-chain scheduling (Laws 23–25) — implemented and proven**: the Lean formalization
  (`spec/Rchain/SchedulerOnchain.lean`) closes the Law 23–25 argument with no axioms remaining —
  `read_state_determines_outcome` (law 23); the writer chain `serializable_writer_chain` (path-nodup +
  initial-record hypotheses); the pinned publication theorem `pinned_run_publication`; the boundary
  witnesses `s3_pair_fails_validation`, `dispatched_serializable_log_inequality`,
  `writer_chain_needs_nodup` and `certificate_blind_late_writer_diverges` (the certificate's
  published blind spot, which is why the oracle backstop stays load-bearing); and law 25's
  `published_state_is_the_oracles` with `fallback_rerun_published`. **Two names this bullet used to
  cite do not exist**: `dfs_serializable_implies_log_equal` (the publication theorem, since renamed
  `pinned_run_publication` to state what it proves) and `validated_speculation_refines_apply` (a
  disjunction whose second arm held for every run, deleted because it said nothing about the
  certificate, the fallback or the code). The Rust realization is the
  `relaxed-validated` block-path mode: relaxed speculation under the claim queue with
  dispatch-time pre-claiming, the per-commit certificate in `try_acquire` (the write-record
  layer's prefix-visibility check, fail-fast into the per-deploy sequential fallback with a
  whole-set safety net in `compute_state`), and the forked sequential oracle legs on the accept
  path (`RuntimeManager::validate_relaxed_block` — post-state hash + per-channel COMM multisets +
  Law 11 rig-replay); certificate and oracle are complementary, each with a decided witness.
  Pure `relaxed` stays hard-rejected on the block paths. Phase-by-phase account:
  [`docs/src/contributor/onchain-validation-phases.md`](docs/src/contributor/onchain-validation-phases.md).
- **Rewrite — complete**: all <!-- recount:members -->14<!-- recount:end --> workspace members are at
  the workspace root — the eleven ported crates (`sdk`, `shared`, `crypto`, `graphz`, `models`,
  `block-storage`, `rspace`, `rholang`, `casper`, `comm`, `node`) plus `rspace-bench`, `ocapn` (the
  CapTP/OCapN transports) and `qucalc`, the Rust-first native AI + governance crate (Part VI of the
  book). The proofs-first *pause* was lifted in practice; the port was written against the verified
  spec rather than waiting on Laws 1–11.
- **Node — operational**: `rnode` builds and runs. A standalone node reaches the `Running` state
  (genesis block created) and serves the API — Deploy **40401**, Propose/Repl **40402** (loopback),
  HTTP **40403**, admin **40405**, protocol **40400**, discovery **40404**. Run instructions:
  [`docs/src/node/operating.md`](docs/src/node/operating.md).
- **Formalization — residual, and smaller than this list used to say**: the per-law status is the
  register's (`spec/Rchain/Laws.lean`, emitted to [`spec/LAWS.md`](spec/LAWS.md) and
  [`spec/laws.tsv`](spec/laws.tsv)), which is the only place that answers "is this proved?" — this
  bullet deliberately does not restate it, because a status repeated by hand is one nothing checks
  (`spec/STYLE.md`). What is worth saying here: the trust surface is a handful of `axiom`s, all of them
  either the deliberate cryptography boundary (Law 19) or named proof debt, and the register's own
  checks refuse a row that cites an axiom the tree does not declare. Laws 12–13 are orphaned (the
  Rosette VM is out of scope). The type-system fundamentals F1–F6 are proven in `Rchain/Ty.lean`. The
  adversarial audit findings are in `spec/AUDIT.md`'s check-off, with the evidence in
  `spec/audit/passes.md`.
