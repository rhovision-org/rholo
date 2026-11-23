# AGENTS.md — rholo: intent, laws and how work is verified

This file is the **authoritative intent + formal specification** for this node. It is written for both
AI coding agents and humans: read it in full before writing or changing any Rust code, and treat its
statements as binding constraints, not suggestions.

**Snapshot — 2026-11-23, `dev` at `b8bbdd7bf`.**

- **The port is complete.** All <!-- recount:members -->14<!-- recount:end --> workspace members are at
  the repository root; the node builds, runs and serves its API.
- **The oracle is the law register**, not the archived Scala implementation: <!-- counts:laws -->128 laws<!-- counts:end -->
  across <!-- counts:entries -->187 entries<!-- counts:end -->, with 0 `owed` and 1 `open` (law 63a, a
  cross-implementation relay this tree cannot observe). Per-row status is the register's and is not
  restated here.
- **What passes today.** The current-build mainnet gate register reads **47 of 47 met**; the external
  register carries eight rows (one `blocked`, two `external`, three `reviewable`, one `owes`, one
  `superseded`). The release-claim register has seven claims and 25 arms: the seven local-devnet arms
  read `safe`, the other eighteen read `not-observed`.
- **What remains is the release campaign**, not defects on the local path: the public four-host fleet,
  plus the bond/leave, off-host, failover, multi-host, equivocation, transport and fleet-upgrade arms.
  Each is a named campaign PR in the generated tracker (below).
- **No readiness claim.** The public gate is open. The project is not claiming readiness for mainnet,
  production, or outside validators, and issue #428 remains NO-GO; `--assert-ready` is not flipped.

## Documentation map

Single sources of truth (do not duplicate these). The curated book is `docs/src/` (built with
`mdbook build docs`); Parts I–III are the **software**, Part IV is **applications**, Part V is the
**port**, Part VI is **QuCalc**, Part VII is **testnet acceptance**, Part VIII is the **methodology
papers**.

| Content | Canonical location |
|---|---|
| **Methods** — how this repository decides what is true and what may close | [Paper A — the law-first port](docs/src/methodology/law-first-port.md), [Paper B — auditable development by AI agents](docs/src/methodology/agentic-assurance.md), [Paper C — release claims and the mainnet gate](docs/src/methodology/release-claims.md) |
| The rholang language & the ρ-calculus (the software) | [`docs/src/rholang/`](docs/src/rholang/) (book Part I) |
| The ρ-calculus, formally — grammar, sorts, the law mapping | [`docs/src/formal/`](docs/src/formal/) (book Part II) |
| The node — consensus, RSpace, storage, operation | [`docs/src/node/`](docs/src/node/) (book Part III) |
| Running/operating the node — REPL, standalone genesis, ports, Docker network | [`docs/src/node/operating.md`](docs/src/node/operating.md) |
| Reader/agent navigation map (goal-indexed) | [`docs/src/ai-entrypoint.md`](docs/src/ai-entrypoint.md) |
| The ρ-calculus core spec (grammar, sorts, operations, refinements) | [`spec/RHO-CALCULUS.md`](spec/RHO-CALCULUS.md) |
| The law register — every invariant, its checked status, axioms and Rust realization | [`spec/LAWS.md`](spec/LAWS.md) (long-form catalogue: [`spec/INVENTORY.md`](spec/INVENTORY.md)) |
| The laws as a reading — one plain-English line per law, grouped by layer | [`docs/src/formal/laws.md`](docs/src/formal/laws.md) |
| Operating manual for the register — numbering, statuses, commands, the Lean-first protocol | [`docs/src/ai/laws-reference.md`](docs/src/ai/laws-reference.md) |
| Human-facing walkthrough: each law → concrete Rust file/type/function + test | [`docs/src/contributor/laws-to-rust.md`](docs/src/contributor/laws-to-rust.md) |
| The ρ→CoC type-system spec | [`spec/TYPE-SYSTEM.md`](spec/TYPE-SYSTEM.md) |
| **Release claims** — seven claims, 25 arms, readings derived from the newest committed record | [`spec/audit/release-claims.json`](spec/audit/release-claims.json); generated tracker [`spec/audit/release-campaigns.md`](spec/audit/release-campaigns.md) |
| **Mainnet gates** — the current-build register and the external obligations | [`spec/audit/mainnet-gates.json`](spec/audit/mainnet-gates.json), [`spec/audit/mainnet-gates.external.json`](spec/audit/mainnet-gates.external.json); normative rules [Gate measurement](docs/src/spec/gate-measurement.md), [Gate terms](docs/src/spec/gate-terms.md) |
| **Public fleet campaign** — the four-host runbook, the driver, the manifest | [`docs/src/node/public-fleet.md`](docs/src/node/public-fleet.md) |
| **Testnet acceptance** — is it ready for outside validators, and what falsifies that | [`docs/src/spec/testnet-acceptance.md`](docs/src/spec/testnet-acceptance.md) |
| How Rust made the Scala fragility explicit (bugs caught, production-readiness) | [`spec/RUST-VS-SCALA.md`](spec/RUST-VS-SCALA.md) |
| What the language let the architecture *build* — the architectural gains, each with its artefact | [`spec/RUST-AFFORDANCES.md`](spec/RUST-AFFORDANCES.md) |
| **Audit check-off** — what the adversarial audit has left, in three states (`todo` · `in progress` · `done`), with the T1 coverage that bounds any claim it makes | [`spec/AUDIT.md`](spec/AUDIT.md) |
| The audit's **pass record** — the evidence behind every check-off row; one section per pass, and the Scala-deviation register (§6) | [`spec/audit/passes.md`](spec/audit/passes.md) |
| Native system contracts (registry/PoS/vault state model + replay determinism) | [`spec/RUST-FIRST.md`](spec/RUST-FIRST.md) |
| Test-coverage audit & gap analysis — machine-checked: per-crate inventory, law property matrix, risk tiers, exempt modules, and the census that requires every source file to be tested or exempt | [`spec/TEST-COVERAGE.md`](spec/TEST-COVERAGE.md) |
| Machine-checked Lean/Coq definitions & proofs | [`spec/`](spec/) |
| Why the rewrite + layer map / module status (port appendix) | [`docs/src/contributor/why-rust.md`](docs/src/contributor/why-rust.md), [`docs/src/contributor/architecture.md`](docs/src/contributor/architecture.md) |

### The methods, in one screen

The three methodology papers are **descriptive**. They explain why the repository is shaped this way;
the registers and tools they describe are the authority, and a paper reports no current reading.

- **Paper A — the law-first port.** The reference implementation is not the oracle. The oracle is the
  law register, machine-checked in Lean 4 and Coq; conformance corpora are *emitted* from the Lean
  definitions and run 1:1 through the node. The archived Scala implementation survives only as test
  vectors, never as truth.
- **Paper B — auditable development by AI agents.** Every claim has exactly one home and a machine
  that reads that home, so "done" is a property of the repository rather than of a conversation. The
  paper records the discipline's own failures, which is the part worth reading.
- **Paper C — release claims.** Release truth is a derived property of the newest committed record. A
  claim is one observable behaviour with one executable test; it has arms in two environments (a local
  four-validator devnet and a public four-host fleet); its reading is one of six values ordered by
  conclusiveness; no status is authored; a local record cannot close a public arm; the fault driver
  chooses the victim; and superseded ids stay mapped to what they became.

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
   the port, release claims).

The formal oracle is `spec/`; the book explains it, it does not duplicate it.

## Intent

The RChain node runs Rholang natively. It has been rewritten in **Rust**, absorbing both the Scala/JVM
code and the C++ Rosette VM. The motivation — memory safety and the calculus-native expression of the
node (λ → π → ρ → Calculus of Constructions) — is laid out in
[`docs/src/contributor/why-rust.md`](docs/src/contributor/why-rust.md).

**Prime directive:** the Scala/JVM + Rosette *port* is complete; the node is a **faithful
implementation of the ρ-calculus**. The oracle is the mathematical specification — the laws in
[`spec/INVENTORY.md`](spec/INVENTORY.md) and the ρ→CoC type discipline in
[`spec/TYPE-SYSTEM.md`](spec/TYPE-SYSTEM.md) — **not** the Scala code. Implement each law using Rust's
strengths: carry the invariants *structurally* in the type system (refinement types, no silent
partiality), rather than mechanically reproducing Scala's patterns. Where the archived Scala code and
the specification disagree, the specification is correct and the code is brought into line — a latent
Scala bug (e.g. wrapping a negative cost into a `uint64`) is **not** preserved; such deviations are
recorded in the Scala-deviation register, [`spec/audit/passes.md`](spec/audit/passes.md) §6.

## Where the project is

**The oracle is done.** The register reads 0 `owed`, 1 `open` across
<!-- counts:entries -->187 entries<!-- counts:end --> — <!-- counts:proved-model-entries -->142<!-- counts:end --> `proved-model`, <!-- counts:proved-tied-entries -->31<!-- counts:end -->
`proved-tied`, <!-- counts:axioms -->10 axioms<!-- counts:end --> (the cryptographic boundary and one named) — and the single open row
(law 63a, a bridged deploy's attribution) is a cross-implementation relay this tree cannot reach rather
than a proof debt. The mathematical programme the prime directive sets is finished. Adding a law, or
re-opening a proof, is a deliberate act rather than the default next step. What each status word means
is [the laws reading](docs/src/formal/laws.md#proved-assumed-and-open); the [register](spec/LAWS.md)
is the only authority for a row.

**What passes on every change.** CI runs five jobs on a pull request:

| Job | What it establishes |
|---|---|
| **Formatting and lints** | `cargo fmt --check` first, then Clippy with `-D warnings` (a formatting failure stops the lint step), the bench targets compile, and the mature-join acceptance predicate tests |
| **Tests and the node's static gates** | the workspace tests; the wasm32 reducer check and tests; and the static-gate battery — audit status, hard-fork #51 closure, release-pin and live-gate/evidence contracts, type system, vendored sources, workflow pins, conflict markers, ingress-queue bounds, spawn observability, cost register, review-ledger coverage, HAZOP worksheets, resource-safety case, node diagnostics, evidence locality, pin-locked build paths, and the wargame section's completeness claim |
| **Documentation book** | the link/SUMMARY gate, the artifact recount, the law-count markers and `mdbook build docs` |
| **Dependency audit** | the pinned dependency tree against the advisory database |
| **Two-sided Docker partition topology** | the offline partition/rejoin regressions and their negative controls |

The Lean/conformance gate runs on spec changes
([`formal-on-change.yml`](.github/workflows/formal-on-change.yml)); the long devnet, fuzz and 16 GB
campaigns are scheduled/dispatched workflows, not PR gates. A green PR
means the build's tests and static gates pass; it does **not** mean a live behaviour was observed.

**What passes in the registers.** The current-build gate register reads **47 of 47 met**
([`spec/audit/mainnet-gates.json`](spec/audit/mainnet-gates.json)). A gate is a boolean test of the
current build, carrying a witness with a real negative control; a decision, a reviewer verdict, an
operator export or an unavailable archive is **not** a gate. The rules are normative at
[Gate measurement](docs/src/spec/gate-measurement.md) and [Gate terms](docs/src/spec/gate-terms.md).

The **external** register ([`mainnet-gates.external.json`](spec/audit/mainnet-gates.external.json))
carries the obligations a build cannot discharge:

| Obligation | State | What it is |
|---|---|---|
| `G428-M5` | `blocked` | finalized state integrity and merge accounting |
| `G428-M6` | `external` | reproducibility and independent sign-off |
| `C357` | `external` | self-replay recovery and section-6 lockstep acceptance |
| `TA-C3` | `reviewable` | every attack vector is handled |
| `S6-PR600-CHECKPOINT-V3` | `reviewable` | the checkpoint-v3 sync-protocol rule (Law 117) |
| `S6-PR600-MERGE-FORK` | `reviewable` | the merge fork rule (Law 115) |
| `G520-MIGRATION` | `owes` | populated-store migration and replay qualification |
| `G584-G6` | `superseded` | retired; superseded by the release-claim reading |

**What remains is the release campaign.** Release truth is not authored. Each claim has named arms; an
arm's state is a **reading** of the newest committed record, and `safe` is the only reading that closes
it. The register holds seven claims and 25 arms; the seven local-devnet arms (`RC-1/local` …
`RC-7/local`) read `safe`, and the other eighteen read `not-observed`. Their next actions are the
campaign PRs in the generated tracker — the authority is
[`spec/audit/release-campaigns.md`](spec/audit/release-campaigns.md). The remaining work:

| PR | Claims | Scope |
|---|---|---|
| `rc-public-fleet` | RC-1 … RC-7 | bring up the four independent hosts on a coordinated genesis; submit the deploy and perform the single-validator restart through `tools/fleet-run.py`; record the public arms |
| `rc-bond-leave` | RC-1, RC-2, RC-4 | one bond/leave campaign feeding RC-1 bond-leave, RC-2 validator-set and RC-4 returner |
| `rc-offhost` | RC-6 | the pre-registered two-host join, including the loopback negative control |
| `rc-failover` | RC-4 | ordered seed list; run with the first seed down |
| `rc-multi-host` | RC-2, RC-4 | the multi-host driver; N=4 and N=8 on declared hosts |
| `rc-equivocation` | RC-2 | inject an equivocation under a validator key; add the finality coordinate |
| `rc-transport` | RC-5 | one deploy id through HTTP, direct HTTP, gRPC and a proxy |
| `rc-harness-old-image` | RC-7 | pin a prior image and run the mixed-store local arm |
| `rc-fleet-upgrade` | RC-7 | roll the public fleet through the mixed window to the new image |
| `launch-gate` | all | aggregate the required arms and external obligations; refuse ready while an external item is open without a dated disposition |

The local arms were re-confirmed on 2026-11-22. **No release claim is made while any required arm
reads `not-observed`:** a local pass is a mechanism check, and the public gate stays open until the
fleet records read `safe`.

## The laws, by group

The register is <!-- counts:laws -->128 laws<!-- counts:end --> / <!-- counts:entries -->187 entries<!-- counts:end -->, emitted from
[`spec/Rchain/Laws.lean`](spec/Rchain/Laws.lean) into [`spec/LAWS.md`](spec/LAWS.md) and refused stale by
the gate. The per-law reading — one line each, grouped like this table — is
[`docs/src/formal/laws.md`](docs/src/formal/laws.md). This table is the **map**, not a third copy: it
covers every law number exactly once, and each row links to the register and to the reading.

| Laws | Group | What the group guarantees | Read |
|---|---|---|---|
| [1–6](spec/LAWS.md#rholang) | **Rholang** | the calculus: canonicalisation, α/name equivalence, substitution, COMM, spatial matching, closedness | [reading](docs/src/formal/laws.md#rholang--the-language-laws-16) |
| [7–11](spec/LAWS.md#rspace) | **RSpace** | the tuple space: join keys, deterministic COMM, merge, the Merkle trie, replay agreement | [reading](docs/src/formal/laws.md#rspace--the-tuple-space-laws-711) |
| [12–13](spec/LAWS.md#rosette) | **Rosette** | the actor VM — orphaned for this port; rows kept so the catalogue does not silently shrink | [reading](docs/src/formal/laws.md#rosette-laws-1213) |
| [14–17](spec/LAWS.md#casper), [18](spec/LAWS.md#storage), [19](spec/LAWS.md#crypto) | **Casper, storage, crypto** | finality and the fringe, the DAG, block validity, merge determinism, the height map, the primitives | [reading](docs/src/formal/laws.md#casper-storage-crypto-laws-1419) |
| [20–25](spec/LAWS.md#scheduler) | **Scheduler** | effect scheduling: the per-channel claim queue, the DFS gate, on-chain validated speculation | [reading](docs/src/formal/laws.md#the-scheduler-laws-2025) |
| [26–29](spec/LAWS.md#cross-shard) | **Cross-shard protocol** | two-phase commit: shard scope, atomicity, idempotency, the durable decision | [reading](docs/src/formal/laws.md#cross-shard-laws-2629) |
| [30–38](spec/LAWS.md#rholang), [39–41](spec/LAWS.md#protocol), [42–43](spec/LAWS.md#json) | **The client surface** | what a client writes and a matcher reads: grammar, lexing, normalisation, matching, reply shapes, JSON | [reading](docs/src/formal/laws.md#the-surface-a-client-writes-and-a-matcher-reads-laws-3043) |
| [44–47](spec/LAWS.md#pos) | **Proof of Stake** | the epoch gate, the reward split and its dust, staged withdrawal | [reading](docs/src/formal/laws.md#the-validator-lifecycle--the-proof-of-stake-epoch-laws-4447) |
| [48](spec/LAWS.md#casper), [49–50](spec/LAWS.md#rholang) | **Charging and depth** | a denied deploy's fee, what a matched deploy costs, the AST and value depth budgets | [charging](docs/src/formal/laws.md#charging-laws-4849), [depth guards](docs/src/formal/laws.md#depth-guards-law-50) |
| [51](spec/LAWS.md#progress), [52–54](spec/LAWS.md#casper) | **Progress** | the shapes and causes of non-progress, the finality gate's two answers, terminal refusal, which view a rule reads | [reading](docs/src/formal/laws.md#progress--the-shapes-of-non-progress-laws-5154) |
| [55, 58](spec/LAWS.md#casper), [56](spec/LAWS.md#rholang), [57](spec/LAWS.md#pos) | **Bounds, names, delegation, participation** | bounded work per step, a rooted namespace, a delegator's stake, the quorum on a fully-participating set | [reading](docs/src/formal/laws.md#bounds-names-delegation-participation-laws-5558) |
| [59, 62, 64](spec/LAWS.md#wire), [60](spec/LAWS.md#rspace), [61](spec/LAWS.md#progress), [63](spec/LAWS.md#protocol), [65](spec/LAWS.md#rholang) | **Wire and sessions** | the CapTP bridge, install totality, the session loop, dial policy, attribution, interop, literal printing | [reading](docs/src/formal/laws.md#the-wire-and-the-session-laws-5965) |
| [66–68](spec/LAWS.md#casper), [69–71](spec/LAWS.md#sync) | **Distributed state and sync** | the record key and join, versioned reads, the page walk, import, the catch-up window | [reading](docs/src/formal/laws.md#distributed-state-and-sync-laws-6671) |
| [72–73, 75–76](spec/LAWS.md#casper), [74, 77](spec/LAWS.md#rspace) | **Durability** | the reset bracket, its request, checkpoints, dependent actions, the stop, the reset's projection | [reading](docs/src/formal/laws.md#durability-laws-7277) |
| [78–79](spec/LAWS.md#casper) | **Genesis** | the developer allocation and its vesting schedule | [reading](docs/src/formal/laws.md#genesis-laws-7879) |
| [80, 84–85](spec/LAWS.md#casper), [81, 83](spec/LAWS.md#sync), [82](spec/LAWS.md#pos) | **The join and the chain's identity** | root publication, the verified-answer join, the validator lifecycle, content-transfer trust, the genesis anchor, fringe state | [reading](docs/src/formal/laws.md#the-join-and-the-chains-identity-laws-8085) |
| [86](spec/LAWS.md#admission) | **Admission and the pool** | the pool is the throttle: a bounded queue, no silent loss, independent work in parallel, one permit progresses, pending work relays | [reading](docs/src/formal/laws.md#admission-and-the-pool-law-86) |
| [87](spec/LAWS.md#decimal) | **Decimal — retired** | the exact value algebra was removed before mainnet; the clause rows say what was read | [reading](docs/src/formal/laws.md#decimal-law-87--retired) |
| [88](spec/LAWS.md#disposition), [89](spec/LAWS.md#pos), [90](spec/LAWS.md#node) | **Disposition, liveness, hostlist** | a stored body is accounted for; jail before any vote reaches a bond; the published hostlist is earned, bounded and fresh | [summary](docs/src/formal/laws.md#the-set-at-a-glance), [hostlist](docs/src/formal/laws.md#the-node-hostlist-law-90) |
| [91](spec/LAWS.md#rholang) | **The cost model** | canonicalisation and union charge their elements, `censure` its cubic fold, a tally its option lists | [reading](docs/src/formal/laws.md#the-cost-model-law-91) |
| [92–94, 97](spec/LAWS.md#casper), [95](spec/LAWS.md#rspace), [96](spec/LAWS.md#scheduler) | **Replay evidence and split accounting** | a cost verdict is not a state verdict; a replay failure's cause is provenance-dependent; a local fault has a bounded exit; the replay record is the play's order; a split does not change accounting or state without its certificate | [summary](docs/src/formal/laws.md#the-set-at-a-glance), [split execution](docs/src/formal/laws.md#split-execution-and-its-accounting-laws-9597) |
| [98–100](spec/LAWS.md#cross-shard) | **Cross-shard acceptance** | the escrow moves exactly and conserves value; the decision is durable before its effects; every phase is a shard-local deploy | [reading](docs/src/formal/laws.md#cross-shard-acceptance-laws-98100) |
| [101–112](spec/LAWS.md#sync), [113](spec/LAWS.md#casper) | **Certified sync, history and merge** | verified answers are peer-independent; replay has one external state; bounded serving; attributable checkpoints; sync separated from verification; history as a checked service; an anchored running node; settlement follows the possession challenge; a merge above a finalised base preserves kept data | [sync and history](docs/src/formal/laws.md#certified-sync-history-and-its-service-laws-101112), [merge](docs/src/formal/laws.md#the-set-at-a-glance) |
| [114, 116–117, 121–124, 127](spec/LAWS.md#sync), [115, 119, 125–126, 128](spec/LAWS.md#casper), [118](spec/LAWS.md#storage), [120](spec/LAWS.md#pos) | **Join, recovery and the checked release path** | complete join inputs; a deterministic, dependency-closed fork rule; a two-arm pre-replay gate; honest checkpoint statements; store-format admission; loss accounting; a joining validator is not drawn in early; catch-up, signing and announcement continuity; a node-local finality read; total settlement; recomputed fringes; recovery that preserves accepted work | [register](spec/LAWS.md), [reading](docs/src/formal/laws.md) |

Every law number falls in exactly one row. A clause (`16a`, `16b`, …) is an **entry**, not a new law:
<!-- counts:clause-laws -->30<!-- counts:end --> laws carry clauses, and the register's status counts are per entry. `C###` is an
audit finding, `F-U…`/`H-U…` a HAZOP row, `W-N…` a wargame row and `§N` an audit pass — different
namespaces, never a law.

## How to use this file

For any component you are about to write in Rust:

1. Find its **layer** in the map below and its **group** in the law map above; read the group's
   one-line guarantee.
2. Open the law's row in [`spec/LAWS.md`](spec/LAWS.md) (or [`spec/laws.tsv`](spec/laws.tsv) for the
   machine form) and read its statement, status, axioms, corpus and falsifier. The register is the
   only status authority; where this file and the register disagree, the register is right.
3. Read the corresponding formalization (Lean 4 and/or Coq) and the Rust witness the row names; run it.
4. Write Rust that satisfies the law, gated by a property test and a corpus/differential vector (see
   *Translation contract*). Where the invariant is representable in the type system, make the bad state
   unrepresentable rather than checked.

## The formal specifications

| Track | Scope | Location | Build |
|---|---|---|---|
| **Lean 4** (primary) | algebraic/order laws, canonicalization, merge monoids, consensus arithmetic, the sync and join laws | [`spec/`](spec/), definitions in [`spec/Rchain/`](spec/Rchain/) | `cd spec && lake build` |
| **Coq** | substitution, α-equivalence, and programming-language metatheory (Autosubst) | [`spec/coq/`](spec/coq/) | `make -C spec/coq` |
| **Register** | the laws, each with status, declarations, axioms and falsifier | [`spec/LAWS.md`](spec/LAWS.md) (long form: [`spec/INVENTORY.md`](spec/INVENTORY.md)) | — |
| **Conformance corpora** | *emitted* from the Lean definitions, committed, and run through the node by a Rust consumer | [`spec/conformance/`](spec/conformance/) | `tools/emit-lean-corpus.sh` |
| **Type system** | the port's own type discipline: ρ-calculus as the base sort of a Calculus of Constructions, no silent partiality | [`spec/TYPE-SYSTEM.md`](spec/TYPE-SYSTEM.md), `Rchain/Rho.lean`, `Rchain/Ty.lean` | `cd spec && lake build` |

The Lean and Coq tracks are deliberately parallel: both define the same core `Proc` syntax and the same
canonicalization (`sort`), and both state Law 1 (`sort` is idempotent and `par` commutative) as the
shared linchpin. Coq is the home of the substitution metatheory (capture-avoiding de Bruijn
substitution, α-equivalence); Lean 4 is the home of the algebraic/order, consensus and sync laws.

Proofs-first was the original policy; the port is complete and the register remains the oracle for any
subsequent change. The historical Meredith lineage behind the calculus is cited in
[Paper A](docs/src/methodology/law-first-port.md) and in [`docs/src/formal/`](docs/src/formal/); it is
not repeated here.

## Layer map

The workspace has <!-- recount:members -->14<!-- recount:end --> members at the repository root:

- **Rholang** (`rholang/`, `models/`) — the ρ-calculus interpreter (canonical order, substitution,
  reduction, spatial matching).
- **RSpace** (`rspace/`) — the concurrent tuple space (join commutativity, deterministic COMM, merge
  monoid, Merkle radix trie, replay).
- **Casper** (`casper/`, `block-storage/`, `sdk/`) — CBC-Casper consensus + DAG (>2/3 finality,
  fringe/estimator, block validation, merge determinism, sync and join).
- **Crypto** (`crypto/`) — Blake2b256, `Blake2b512Random`, secp256k1, Curve25519.
- **Supporting crates** — `shared`, `comm` (transports), `graphz` (the DAG store), `rspace-bench`.
- **OCapN** (`ocapn/`) — the CapTP/OCapN transports and the object-capability boundary.
- **QuCalc** (`qucalc/`) — the native AI + governance crate (book Part VI).
- **Node** (`node/`, binary `rnode`) — configuration, API, proposal loop, operation.

`rosette`/`roscala` are **not in this tree**: the C++ actor VM is orphaned for the port. Laws 12–13
remain in the register (formalization in scope, Rust realization out of scope). Per-layer source
pointers and the workspace layout are in
[`docs/src/contributor/architecture.md`](docs/src/contributor/architecture.md).

## Translation contract (spec → Rust)

For every law in the register:

1. **Property test** — a `proptest`/`quickcheck` property asserting the law on the Rust
   implementation (e.g. `sort(sort(p)) == sort(p)`, merge is associative/commutative).
2. **Corpus / differential vector** — run the committed vectors through the node and compare exactly:
   the conformance corpora emitted from the Lean definitions (`spec/conformance/*.tsv`) and the
   archived reference corpus at [`rholang/tests/legacy-corpus/`](rholang/tests/legacy-corpus/). The
   reference implementation is a source of vectors, not an oracle.
3. **Type-level fidelity** — the Lean/Coq types are the reference for the Rust data model and its
   `Ord`/`Hash`/`Eq` derivations. In particular `sort` becomes the `Ord` implementation that makes
   `Par` an order-insensitive (canonical) container.
4. **Axiomatized crypto** maps to Rust traits whose implementations are the *only* places the
   primitive may differ in behavior, and are pinned by known-answer test vectors.

## Ground truth — where the reference is

The `legacy/` tree was archived out of the working tree on 2026-09-28 —
<!-- recount:legacy-mib -->26.2<!-- recount:end --> MiB (<!-- recount:legacy-bytes -->27,425,437<!-- recount:end --> bytes) across
<!-- recount:legacy-files -->1,596<!-- recount:end --> files that no CI job built, tested or scanned, carrying a 2020-21 dependency
manifest `cargo-deny` cannot see. It is readable at the revision that froze it, **`1b7583649`**:
`https://github.com/rhovision-org/rholo/blob/1b7583649/legacy/<path>`, or locally with
`git checkout 1b7583649 -- legacy`. Every `legacy/...` path cited anywhere in this repository — here,
in `spec/`, in the book, in code comments — resolves at that revision, which is why it is recorded
rather than left to a deletion commit's message. See `README.md`'s *Where the Scala went*.

The archived implementation's surviving role is **vectors**. Its contract corpus is committed at
[`rholang/tests/legacy-corpus/`](rholang/tests/legacy-corpus/) (165 files); the archived Scala tests
encode behaviours the laws also state, and the Rust implementation must agree with the vectors. A
divergence from the *specification* is fixed in the Rust code and recorded in the
[`spec/AUDIT.md`](spec/AUDIT.md) check-off and its [pass record](spec/audit/passes.md); it is **not**
propagated to stay byte-identical with a Scala bug.

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
the scalar in `/proc/<pid>/cmdline` (W-N11-05) — and a `~/.rnode/genesis/wallets.txt` (must exist; may
be empty — `bonds.txt` auto-generates). The API host must be a literal IP (`SocketAddr::from_str`
rejects hostnames like `localhost`). Deploy serves on **40401**; Propose/Repl on loopback **40402**.
Full instructions: [`docs/src/node/operating.md`](docs/src/node/operating.md).

## Working discipline — registers, tracker and docs

**A finding has one home.** Its row in [`spec/findings.tsv`](spec/findings.tsv), rendered into
[`spec/AUDIT.md`](spec/AUDIT.md). A [`spec/audit/passes.md`](spec/audit/passes.md) entry is optional and
short: the mechanism, the evidence, what it rules out. A finding never spawns an issue by itself.

**A close condition names an observable behaviour** — a test, a measurement, a run. Paperwork — a
register entry, a proof, a "decision recorded" — is evidence *for* the row, never a conjunct of the
condition. A condition that conjoins the two cannot be closed by finishing the work.

**An issue is opened only when someone commits to the work**, and it names the owner and the next
action. Every open issue names the target it advances, or it closes. The register is the record; the
tracker is the worklist.

**Reserve the C-number when the branch is opened**, not when the finding is written, and let
`tools/next-audit-number.sh` read the remote branches. A number taken from one tree is a reservation,
not a fact. `C###` is an audit finding, `F-U…`/`H-U…` a HAZOP row, `W-N…` a wargame row, `§N` an audit
pass — never conflate the namespaces.

**Documentation discipline.** Plans, roadmaps and hypotheticals are quarantined outside the repository
(`~/.claude/plans/`); the book and `spec/` describe what compiles and what is proved. A document claim
that cannot be traced to a `.rs`, `.lean` or `.v` artefact says so in the sentence that makes it, and
links the artefact. Every book page is listed exactly once in
[`docs/src/SUMMARY.md`](docs/src/SUMMARY.md); every `spec/` document is reachable from
[`spec/README.md`](spec/README.md) or from this file. The link/SUMMARY/fragment gate is
`tools/check-docs-links.sh --gate` (run by `make check-docs`); the artifact figures are re-emitted by
`tools/recount-artifact.sh`, and a hand-stated law/entry/axiom total is refused by
`tools/emit-lean-counts.sh --check`. `HANDOVER.md` is a deliberately unlinked working note, not a page
of the book or of `spec/`.

## Open questions

1. **Law 63a** — a bridged deploy's attribution is `open`: it needs a cross-implementation relay this
   tree cannot reach. The other three laws of that family are proved; the row records the boundary
   rather than a debt.
2. **The public gate** — the external obligations above and the `rc-public-fleet` campaign are the
   remaining release work. Until every required public arm reads `safe`, no readiness claim is made.
3. **Rosette scope** — the VM is in scope for the *formalization* (Laws 12–13) and out of scope for the
   *rewrite* (`rosette`/`roscala` are orphaned). That split is a decision, not an omission.
