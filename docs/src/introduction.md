# rholo

> **Provenance.** This repository was developed through commit `14b8b77` on
> [`PatrickMockridge/rchain-rust`](https://github.com/PatrickMockridge/rchain-rust) — a fork of
> [`rchain/rchain`](https://github.com/rchain/rchain). As of that commit, the repository moved to
> `rchain-community/rchain-rust`, renamed on 23 October 2026 to
> [`rhovision-org/rholo`](https://github.com/rhovision-org/rholo). A snapshot of the
> code at that state is timestamped on Arweave — transaction
> [`MK4WA8w3NTIIFd6iaD06EPyxVbFhoV9MtfhMNcHWWMw`](https://arweave.net/MK4WA8w3NTIIFd6iaD06EPyxVbFhoV9MtfhMNcHWWMw),
> 22 August 2026 7:15pm.

rholo is a blockchain platform whose smart-contract language — **rholang** — is not a language
grafted onto a ledger, but a *concurrent, reflective process calculus*: the **ρ-calculus**. This book
is the software's documentation: what rholang is, why it is the way it is, how it runs, and how the
node that executes it works.

Rholang is a process-oriented language. A program is not a sequence of statements but a set of
**concurrent processes** that communicate by sending messages over **channels** (called *names*). The
defining idea of the ρ-calculus is **reflection**: a *name* is a *quoted process* (`@P`), and a process
can *evaluate* a name back into a process (`*x`). Because quoting and dereferencing code are built into
the language, programs can name other programs, pass code over channels, and build their own security
boundaries. That one fact is the foundation of everything rholang does well — deterministic
communication, compositional smart contracts, and unforgeable names.

## How to read this book

The book is layered, so it works for a first-time reader and for someone who has built on RChain for a
decade.

- **Part I — Rholang & the ρ-calculus** is the language. It starts with *why* rholang exists and
  builds up to object-capability smart contracts. Each chapter leads with intuition and ends with a
  pointer into the formal treatment.
- **Part II — The ρ-calculus, formally** is the precise semantics: the grammar, the sorts, and the
  **[<!-- counts:laws -->80 laws<!-- counts:end -->](formal/laws.md)** that govern the node — the
  calculus (1–29), the surface a client writes and a matcher reads (30–43), the Proof-of-Stake epoch
  (44–47), the fee and charging rows (48–49), the two depth guards (50), the progress rows (51–54), the
  bounded-work and namespace rows (55–56), the delegator's stake (57), the finality gate's forward dual
  (58), the wire and session laws (59–65), the distributed-system set (66–71), the durable reset,
  the stop and the read construction (72–77), the genesis's developer allocation (78) and the schedule
  it unlocks on (79). Each law is
  mapped
  to its machine-checked formalization, and the [folder's index](formal/README.md) says which document
  carries which set.
- **Part III — The node** describes the software that executes rholang: the tuple space, the Merkle
  state, and the Casper consensus protocol.
- **Part IV — Building applications** is the developer guide: stand up the local Docker devnet and
  configure your app's endpoints to deploy rholang and read back the results.
- **Part V — Contributor / port** is the engineering appendix: why the node is written in Rust, and
  the module-by-module status of the implementation.
- **Part VI — QuCalc: native AI & governance** is the Rust-first extension: quantum operators mapped
  into the ρ-calculus, multi-stakeholder governance, and the experimental concurrent reducers.
- **Part VII — Testnet acceptance** is the gate: a HAZOP worksheet and bow-tie analysis of the node, and
  the acceptance checklist that decides whether the net is fit for outside validators.

## For AI agents

If you are an AI agent (or want the shortest path to a specific fact), start at
[Navigation for AI agents](ai-entrypoint.md) — a goal-indexed map of this book and the machine-checked
specification it links to. The authoritative formal specification lives outside this book, in the
[`spec/`](../../spec/) tree (the law register — <!-- counts:laws -->80 laws<!-- counts:end -->, the Lean/Coq proofs, and the conformance corpora
that bind the two to the node); this book explains it, it does not duplicate it.
