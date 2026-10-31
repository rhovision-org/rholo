# rholo: a language with a theorem, not just a test suite

Rholang's <!-- counts:laws -->91 laws<!-- counts:end --> are machine-checked in Lean 4 and Coq, and
its authority cannot be forged — there is no syntax that writes a private name. Pre-testnet.

> **Operator risk.** A validator is bonded and slashable for an invalid block or proven equivocation.
> An absent or stuck validator is **not slashed automatically**, but it holds finality back, still
> counts in the quorum denominator, and on a network that arms RCHIP #55 liveness enforcement it can be
> **jailed after sustained absence and then slashed and permanently ejected by a stake-weighted vote
> of strictly more than two thirds of the active stake**. The shipped genesis leaves that enforcement
> off; a network that arms it prices absence. Read
> [`TERMS.md` §1](TERMS.md#1-a-validator-is-a-bonded-slashable-role) and
> [validator risk, hardware and participation](docs/src/node/operator-risk.md) before bonding.

**Whole classes of defect are absent by construction**: no `unsafe` in any of the fourteen crates, so
memory-safety bugs are unwritable rather than mitigated. The October 2026 adversarial audit read this node and
Solana (Agave), Sui and Bitcoin SV at pinned revisions: no chain split, no fund loss, no remote code
execution here.

**A small artifact, deliberately**: recounted <!-- recount:date -->2026-10-30<!-- recount:end -->,
<!-- recount:rs-lines -->231,044<!-- recount:end --> lines of Rust across
<!-- recount:rs-files -->513<!-- recount:end --> files compile to a single
**<!-- recount:binary-mib -->47.0<!-- recount:end --> MiB** native binary
(<!-- recount:binary-bytes -->49,317,360<!-- recount:end --> bytes) — nothing to boot but the process,
and no collector inside it to pause — so the node runs on a commodity laptop with an NVMe. A
**validator** still needs the per-validator hardware floor; a laptop is a good observer, not
automatically a good validator.

## Provenance

This repository was developed through commit `14b8b77` on
[`PatrickMockridge/rchain-rust`](https://github.com/PatrickMockridge/rchain-rust) — a fork of
[`rchain/rchain`](https://github.com/rchain/rchain). As of that commit, the repository moved to
`rchain-community/rchain-rust`, renamed on 23 October 2026 to
[`rhovision-org/rholo`](https://github.com/rhovision-org/rholo). A snapshot of the
code at that state is timestamped on Arweave — transaction
[`MK4WA8w3NTIIFd6iaD06EPyxVbFhoV9MtfhMNcHWWMw`](https://arweave.net/MK4WA8w3NTIIFd6iaD06EPyxVbFhoV9MtfhMNcHWWMw),
22 August 2026 7:15pm.

The node is a Cargo workspace at the top level (one crate per original sbt module); the upstream Scala
fork is preserved for reference at
[`1b7583649`](https://github.com/rhovision-org/rholo/tree/1b7583649/legacy) — see
[Where the Scala went](#where-the-scala-went) for why it is a revision rather than a directory.

## Documentation

The documentation is served as a book (`mdbook serve docs`). It is organized software-first:

- **Part I — Rholang & the ρ-calculus** ([`docs/src/rholang/`](docs/src/rholang/)) — the language:
  what it is, why it fits a blockchain, and from processes and names through object-capability smart
  contracts.
- **Part II — The ρ-calculus, formally** ([`docs/src/formal/`](docs/src/formal/)) — the grammar, the
  sorts, and all <!-- counts:laws -->91 laws<!-- counts:end --> — the calculus (1–29), the surface a
  client writes and a matcher reads (30–43), the Proof-of-Stake epoch (44–47), the fee and charging
  rows (48–49), the depth guards (50), the progress rows (51–54), the bounded-work and namespace rows
  (55–56), the delegator's stake (57), the finality gate's forward dual (58), the wire and session laws
  (59–65), the distributed-system set (66–71), the durable reset, the stop and the read construction
  (72–77), the genesis's developer allocation (78) and the schedule it unlocks on (79), and the
  sync, Proof-of-Stake, Casper, admission, decimal and disposition rows (80–89), the published
  hostlist (90) and the charge magnitudes (91) — mapped to their machine-checked proofs.
- **Part III — The node** ([`docs/src/node/`](docs/src/node/)) — consensus, the tuple space, storage,
  and operation.
- **Part IV — Building applications** ([`docs/src/developer/`](docs/src/developer/)) — building
  rholang applications on the local devnet.
- **Part V — Contributor / port** ([`docs/src/contributor/`](docs/src/contributor/)) — why Rust, and
  the per-module status.
- **Part VI — QuCalc: native AI & governance** ([`docs/src/qucalc/`](docs/src/qucalc/)) — the
  Rust-first quantum-to-ρ operators, multi-stakeholder governance, and the concurrent-reducer
  experiments.
- **Part VII — Testnet acceptance** ([`docs/src/spec/`](docs/src/spec/)) — the acceptance
  specification, the bounded adversary, the failure-mode HAZOP, and the resource-safety case.

The entry point for the book is [`docs/src/introduction.md`](docs/src/introduction.md); the
goal-indexed map for readers and AI agents is
[`docs/src/ai-entrypoint.md`](docs/src/ai-entrypoint.md).

## Why Rust

Three reasons drive the rewrite.

**Memory safety.** The Scala/JVM node's latency and footprint were the collector's — it shipped
`Memory`/`GarbageCollector` diagnostics and needed `SBT_OPTS="-Xmx4g -Xss2m"` to run. In Rust the
`unsafe` keyword is forbidden, so the *memory-unsafety* class — use-after-free, double free,
out-of-bounds access, data races — is unwritable; and the binary ships no tracing collector, so
nothing stops the world to collect. Neither claim says memory cannot leak or grow: a leak is safe
code, and a stop-the-world collector is safe Rust — we simply do not ship one. Footprint is an
engineering concern either way, and this node measures its own (see
[hardware requirements](docs/src/node/validator-requirements.md)).

**Decentralization.** Recounted <!-- recount:date -->2026-10-30<!-- recount:end -->,
<!-- recount:rs-lines -->231,044<!-- recount:end --> lines of Rust compile to a single
<!-- recount:binary-mib -->47.0<!-- recount:end --> MiB native binary
(<!-- recount:binary-bytes -->49,317,360<!-- recount:end --> bytes) — no JVM and no collector in the
process — so the node runs on any modern desktop or laptop with an
NVMe SSD. A **validator** still needs the per-validator floor; a laptop is a good observer, not
automatically a good validator (see [hardware requirements](docs/src/node/validator-requirements.md)
and [validator risk](docs/src/node/operator-risk.md)). Validator operation can sit within
consumer-grade hardware when that floor is met, which is what makes the network genuinely decentralized.

**The calculus hierarchy.** Rust natively expresses the λ-calculus (closures), the π-calculus
(channels and `Send`/`Sync` name mobility), and the ρ-calculus (the reflective π-calculus: a name is a
quoted process, expressed here as the sortable `Par` value). The port's type discipline embeds ρ as
the base sort of a Calculus of Constructions, constructible and provable in Lean 4 and Coq.

The full argument — including the co-op lesson and the Rust → calculus → formalization correspondence
table — is in [docs/src/contributor/why-rust.md](docs/src/contributor/why-rust.md). The prose
documentation is also served as a book: `mdbook serve docs`.

## Models of computation

Six formal models of computation, scored on the six capabilities a blockchain language needs — and why the
last row is the one this node is built on:

| Model | Complete | Compositional | Concurrent | Complexity | Reflection | Behavioural types | Score |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Turing machines | ✅ | ❌ | ❌ | ✅ | ❌ | ❌ | 2 / 6 |
| Lambda calculus | ✅ | ✅ | ❌ | ❌ | ❌ | ❌ | 2 / 6 |
| CCS | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ | 3 / 6 |
| Petri nets | ✅ | ❌ | ✅ | ✅ | ❌ | ❌ | 3 / 6 |
| π-calculus | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | 5 / 6 |
| **ρ-calculus** | ✅ | ✅ | ✅ | ✅ | **✅** | ✅ | **6 / 6** |

**Concurrency**, in that column, is not "can run things at once" — every model can be scheduled. It is
whether the model *defines when two concurrent steps commute*, so that parallelism is sound rather than
hoped for: rholang processes run concurrently by construction, communicate only by messages on channels,
and the calculus says which reorderings preserve behaviour. That is what lets a node evaluate a deploy
concurrently and still agree with its peers on the state hash.
[The concurrency laws](docs/src/formal/concurrency.md) are that argument, not a convention about it.

**Safe by construction — the same claim, one level down.** A model's guarantees are worth only what the
implementation keeps, so the same discipline is applied to the code: `unsafe` is forbidden across the
crate graph, so use-after-free, double free, out-of-bounds access and data races do not compile;
refinement newtypes carry invariants that would otherwise be comments; and every consensus rule is a law
tied to the code by a corpus or a named test. Its limits are stated too — a leak is still safe code, and
shipping no collector is a property of this binary rather than of the language
([why Rust](docs/src/contributor/why-rust.md)).

The comparison — what each capability means, what each model is for, and what "6 / 6" does and does not
claim — is [Models of computation](docs/src/rholang/models-of-computation.md).

## Security and assurance

**A pre-testnet node whose security record is a programme, not a report — and one already larger than
the node it reviews.** Every layer below lives in the tree, and each is held there by something that
refuses to let it drift. The reference implementation's contribution runs the other way: its contracts
are kept as a differential corpus, and the fragile patterns the port caught from it as *type errors and
review findings* are catalogued in [`spec/RUST-VS-SCALA.md`](spec/RUST-VS-SCALA.md) §1 — eight of them,
from a gas cost that wraps negative to a hash-map iteration order that could have split a chain.

**Proved.** The ρ-calculus and its node-level laws are machine-checked in Lean 4, with an α-equivalence
metatheory in Coq: <!-- counts:laws -->91 laws<!-- counts:end -->. What binds them to *this* code is not
review but a **conformance corpus** — each corpus is emitted from the Lean definition and run 1:1
through the node by a Rust consumer, so a definition cannot drift from its corpus without the gate
noticing ([`spec/INVENTORY.md`](spec/INVENTORY.md), [`spec/TYPE-SYSTEM.md`](spec/TYPE-SYSTEM.md)).

**Reviewed, adversarially, repeatedly.** The pass record is
[`spec/audit/passes.md`](spec/audit/passes.md) — 150 sections of findings, refutations and remediations,
from type-system conformance and ρ-calculus mirroring through a red-team series that reached pass 9 to a
full-system HAZOP. The check-off is [`spec/AUDIT.md`](spec/AUDIT.md): a finding has **exactly one home**
and closes by construction — a row is `done` only when the bad state is no longer representable, with
the citation that forbids it, or as a recorded decision. Read the file for the current figures; it is
the only place they are stated.

**HAZOP, and a tabletop wargame.** A failure-mode HAZOP dispositions all eleven IEC guide words across
21 study nodes and 96 deviations, with six bow-tie top events
([`docs/src/spec/failure-hazop.md`](docs/src/spec/failure-hazop.md),
[`docs/src/spec/testnet-acceptance.md`](docs/src/spec/testnet-acceptance.md)). And a bounded-adversary
tabletop walks the running system node by node — deploy ingress, validator keys, OCapN transports, the
clock — asking which of seven personas can act against each: **97 rows**, each with a decided verdict
and a falsifier, held to shape by a CI gate and driven against one live devnet boot with the runs
committed ([`docs/src/spec/adversary.md`](docs/src/spec/adversary.md)).

**Tested in-process and live, and gated.** ~2,615 test functions (counted 2026-10-30), a 165-file
Scala differential corpus run 1:1 through the node, a nightly devnet fuzz in four modes, and 56
committed evidence directories (40 campaign runs) from live drills. Static gates run in CI on every
push beside `cargo fmt --check`,
clippy with `-D warnings`, `cargo deny check`, the `wasm32` reducer check and a nightly formal gate,
under an 87 % line-coverage floor
([`spec/AUDIT.md`](spec/AUDIT.md)).

### Comparison with other chains

**Comparison with three live mainnets.** The October 2026 pass read this node and Solana (Agave), Sui
and Bitcoin SV at pinned revisions (`v4.4.0`, `testnet-v1.83.0`, `v1.2.3`) against one question — *can
a remote attacker make the node do unbounded work?* — and the table is the answer.

| | **This node** (pre-testnet) | **Solana** (Agave) | **Sui** | **Bitcoin SV** |
|---|---|---|---|---|
| **Memory safety** | No `unsafe` anywhere — `#![forbid(unsafe_code)]` in all fourteen crates | 52 `unsafe` blocks in the loader alone; forbidden in one crate | `unsafe` in `sui-types`; not forbidden | C++ — assertions cannot be compiled out |
| **Authority cannot be forged** | **Unrepresentable** — no syntax writes a private name | Checked at runtime — a program-derived address has no key, but granting the privilege is a check | **Unrepresentable** — a linear `UID` minted from the transaction digest | Absent as a concept |
| **Authority cannot be captured** | **Unrepresentable, and proved** — a COMM moves only the datum sent, into the receiver's own environment; closedness is a theorem | Checked at runtime | Unrepresentable — no dynamic dispatch | Moot — there are no calls |
| **Machine-checked semantics** | Lean 4 + Coq — <!-- counts:laws -->91 laws<!-- counts:end -->; every registered witness runs by name; `sorry`/`admit`/`opaque` refused | `frozen-abi` digests, which are checksums | Static bytecode verifier; **Move Prover absent from its tree and CI** | Nothing |
| **Peer identity** | Mutual TLS; the certificate key is bound to the identity the peer claims. Which keys are admitted is unconstrained — a recorded decision, not an oversight ([§116](spec/audit/passes.md)) | Signed shreds, but no binding to a claimed identity | P2P layer is a git dependency — uninspectable in-tree | Plaintext; the handshake signs nothing |
| **Supermajority arithmetic** | Exact `i128` integer — no floating point | `2f64/3f64` | Integer | Proof of work |
| **Cost model bounds attacker work** | ✅ per-block phlo and deploy caps; every priced operation is registered and gated ([`spec/COST.md`](spec/COST.md)) | ❌ account copies are unbilled | Instruction tiers, 128 KiB transaction bound | ❌ no step budget at all |

Two limits: Sui's crypto/P2P and Solana's Agave VM are unvendored, so those cells are **could not trace**, not
cleared; and this node's assurance ends where the review says — the chain layer's authority is access
control by another name, and unforgeability is axiomatised at the crypto boundary (law 19), not proved.

**The October 2026 re-audit in full — the cost-model class, the new ingress surface, the referee
verdicts, the comparison at pinned revisions, and the remediation of all eight findings in the same
PR; September 2026 is kept at §9:
[`docs/src/node/security-audit.md`](docs/src/node/security-audit.md).**

**How the apparatus was built is written up too, because the method is the part that is unusual here.**
Two draft papers in the book state it and its own limits:
[Paper A — the law-first port](docs/src/methodology/law-first-port.md) argues why the reference
implementation is not the oracle, and what holds an emitted conformance corpus in place;
[Paper B — auditable development by AI agents](docs/src/methodology/agentic-assurance.md) asks what
makes agent-written work checkable *after* the session that wrote it is gone. Both name the same missing
experiment — attributing each finding to the layer that surfaced it, and its cost — and both say plainly
that they are drafts. Every claim in either one is checkable against this tree.

## Governance

The rewrite is governed by [`AGENTS.md`](AGENTS.md) — the binding intent + formal specification —
and the machine-checked formalizations in [`spec/`](spec/) (the law register
[`spec/INVENTORY.md`](spec/INVENTORY.md) — <!-- counts:laws -->91 laws<!-- counts:end -->, counted and
emitted by `Rchain/Laws.lean` so the number cannot drift — plus the Lean/Coq tracks). The prime
directive is a **faithful implementation of the ρ-calculus**: the laws are the oracle; the Scala node
was the port reference.

## Funding

Development is funded via [OpenCollective](https://opencollective.com/rho-vision-community), under
the **Rho Vision (formerly RChain Community)** collective:

- [Rholang – Rust Implementation](https://opencollective.com/rholang-rust) — this rewrite.
- [RhoGOV: EIES3](https://opencollective.com/eies3) — electronic information exchange / governance.
- [RHO Tools in Rust](https://opencollective.com/rho-tools-in-rust).

## Layout

The Cargo workspace has fourteen members — twelve crates ported from the original sbt modules
(`sdk`, `shared`, `crypto`, `graphz`, `models`, `block-storage`, `comm`, `rspace`, `rholang`,
`casper`, `node`, `rspace-bench`), plus `ocapn`, the CapTP/OCapN transports that let a peer outside
this node hold a chain-backed capability ([`docs/src/node/ocapn.md`](docs/src/node/ocapn.md)), and
`qucalc`, the Rust-first native AI + governance crate (Part VI of the book). The per-crate status, the layer map, the rewrite order, and the remaining work
are consolidated in
[docs/src/contributor/architecture.md](docs/src/contributor/architecture.md).

## Build & test

```sh
cargo build --release -p rchain-node --bin rnode   # the `rnode` binary
cargo test --workspace                              # the full test suite
```

Build and serve the documentation book:

```sh
mdbook serve docs   # or: mdbook build docs
```

## REPL

The `rnode` binary doubles as a thin gRPC client. Run the interactive rholang REPL against a node:

```sh
# Start a local standalone node (creates genesis), then in another terminal:
cargo run --release -p rchain-node --bin rnode -- run -s

# Interactive REPL (prompt `rholang $ `, history, tab-completion):
cargo run --release -p rchain-node --bin rnode -- repl

# …or point the client at a remote node (Repl is on the internal port 40402; Deploy is on 40401):
rnode --grpc-host <host> --grpc-port 40402 repl
```

Each term is parsed, normalized (an `Evaluating:` line is echoed on the node console), evaluated
against the node's isolated `eval-*` store, and printed as `Deployment cost:` + `Storage Contents:`.
`:q` quits. Evaluate files non-interactively with `rnode eval <file>...`.

## Docker

`tools/devnet.sh` is the single script for a local Docker network, with two modes:

- **devnet** (default) — 1–3 bonded validators with autopropose + a funded deployer wallet, for
  deploying/testing rholang contracts and reading results back.
- **network** (`up --nodes N`) — a bare 1–5 node topology with no autopropose/deployer, for exercising
  sync/gossip, driven manually via `cli <node> propose`.

```sh
tools/devnet.sh build                 # build the rnode:local image (cached)
tools/devnet.sh build --fresh         # force a clean rebuild (--no-cache --pull)

# contract devnet:
tools/devnet.sh up --validators 1     # single validator, autoproposing
tools/devnet.sh deploy hello.rho      # signed deploy (examples/hello.rho sends "world")
tools/devnet.sh query hello           # -> "world"
tools/devnet.sh demo                  # deploy examples/wallet.rho + assert its save/load round-trip
python3 tools/devnet-fuzz.py --validators 3   # robustness + determinism fuzz against a live devnet

# bare network topology:
tools/devnet.sh up --nodes 3          # bootstrap + 2 peers, manual propose
tools/devnet.sh cli devnet-bootstrap propose

tools/devnet.sh down -v               # stop + drop volumes
```

A block is created only when one of these fires: (a) a deploy arrives and `--propose-on-deploy` is set,
(b) `--autopropose`'s timer fires, or (c) someone calls `propose`/`POST /api/v1/propose`. An idle node
with none of these produces no blocks.

See [docs/src/node/devnet.md](docs/src/node/devnet.md) (contract devnet),
[docs/src/node/operating.md](docs/src/node/operating.md) (network topology), and `tools/devnet.sh help`
for the full flag reference.

## Where the Scala went

The upstream Scala fork — the sbt modules (`node/`, `sdk/`, `shared/`, `crypto/`, `models/`,
`rspace/`, `comm/`, `casper/`, `rholang/`, `block-storage/`, `regex/`, `graphz/`, `roscala/`,
`rosette/`, `rspace-bench/`), the sbt build (`build.sbt`, `project/`), configuration, CI, docs,
tooling, and data files — lived under `legacy/` from the start of the rewrite.

**It was archived out of the working tree on 2026-09-28.** Two reasons: it was
<!-- recount:legacy-mib -->26.2<!-- recount:end --> MiB
(<!-- recount:legacy-bytes -->27,425,437<!-- recount:end --> bytes) across
<!-- recount:legacy-files -->1,596<!-- recount:end --> files that no CI job built, tested or scanned,
and it carried its own 2020–21 dependency manifest
(`build.sbt`, `project/Dependencies.scala` — netty, logback, protobuf, lz4) that `cargo-deny` cannot
see because it walks only the Rust lockfile.

**It is still there, and every citation to it still resolves** — at the commit that froze it:

| | |
|---|---|
| Last commit carrying `legacy/` | [`1b7583649`](https://github.com/rhovision-org/rholo/tree/1b7583649/legacy) |
| Read a file | `https://github.com/rhovision-org/rholo/blob/1b7583649/legacy/<path>` |
| Restore it locally | `git checkout 1b7583649 -- legacy` |

That is the point of recording the revision rather than just deleting the tree: this repository cites
the Scala by <!-- recount:legacy-citation-paths -->162<!-- recount:end --> distinct `legacy/...` paths
(<!-- recount:legacy-citation-refs -->249<!-- recount:end --> references), as the port's reference
implementation, and a citation that cannot be followed is not a citation. Every `legacy/...` path in a code comment, in `spec/`, or in the book
resolves at `1b7583649` — including the ones written after the archive, which is why the revision is
here and not just in the commit that removed it.

## License

The Rust rewrite is licensed under the [GNU Affero General Public License, version 3](LICENSE)
(AGPL-3.0). The upstream Scala fork — archived out of the working tree on 2026-09-28 and readable at
[`1b7583649`](https://github.com/rhovision-org/rholo/tree/1b7583649/legacy) — retains its
original [Apache License 2.0](https://github.com/rhovision-org/rholo/blob/1b7583649/legacy/LICENSE.TXT).
