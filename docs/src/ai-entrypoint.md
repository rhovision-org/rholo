# Navigation for AI agents

This page is a goal-indexed map of the documentation and the machine-checked specification. Read it
first, then jump to the single page that answers your question. It mirrors the documentation map in
[`AGENTS.md`](../../AGENTS.md) but is organized by *goal* rather than by artifact.

The **authoritative formal specification** is the [`spec/`](../../spec/) tree — the law register
([`spec/LAWS.md`](../../spec/LAWS.md), **<!-- counts:entries -->131 entries<!-- counts:end -->**: the calculus laws, the rows covering the
surface a client writes, the Proof-of-Stake epoch, the fee and charging rows, the progress set, and the
bounded-work, namespace, wire, session and distributed-system rows that follow; its long-form
catalogue is [`spec/INVENTORY.md`](../../spec/INVENTORY.md)), the ρ-calculus core
([`spec/RHO-CALCULUS.md`](../../spec/RHO-CALCULUS.md)), and the ρ→CoC type discipline
([`spec/TYPE-SYSTEM.md`](../../spec/TYPE-SYSTEM.md)). This book explains those; it never duplicates them.

## By goal

| I want to… | Read |
|---|---|
| Understand *why* rholang exists and what makes it powerful | [Why rholang](rholang/why-rholang.md) |
| Learn the language from scratch | [Processes and names](rholang/processes-names.md) → [Sends and receives](rholang/sends-receives.md) |
| Understand `@` and `*` (quote/eval) | [Names are quoted processes](rholang/names-are-processes.md) |
| Understand what an **unforgeable name** is and why `new` matters | [Unforgeable names](rholang/unforgeable-names.md) |
| Understand pattern matching / spatial matching | [Patterns and matching](rholang/patterns-matching.md) |
| Build a secure contract (facets, revocation, sealer/unsealer, multisig) | [Object capabilities](rholang/object-capabilities.md), [Smart contracts](rholang/smart-contracts.md) |
| **Port an existing app** (or debug a parser that worked elsewhere) | [Porting an app from rnode](developer/porting-a-client.md) — reply shapes, refused terms, the traps |
| See the exact grammar and sorts | [Grammar and sorts](formal/grammar-sorts.md) |
| Map a language feature, or a **syntax, matching, reply-shape or JSON** feature, to its **law** and its proof | [The laws](formal/laws.md) |
| Ask whether gas bounds work, or why a charge is the shape it is | [The cost model, and the gate that keeps it honest](formal/cost-model.md) |
| **Work on the law register** — add or change a law, read a status, run the gates | [Laws reference for AI agents](ai/laws-reference.md) |
| Understand how concurrent effects are linearized (claim queues, the DFS gate, the relaxed mode) | [The channel scheduler](formal/scheduling.md) |
| Understand how effect concurrency becomes sound on-chain (validated speculation, the Law 24 certificate) | [On-chain scheduling: validated speculation](formal/scheduling.md) |
| Understand `≡` and `⟶` precisely | [Structural congruence and reduction](formal/concurrency.md) |
| Understand the "no silent partiality" / totality guarantee | [Closedness and the Calculus of Constructions](formal/closedness-coc.md) |
| Understand consensus / finality | [Consensus (Casper)](node/consensus.md) |
| Understand the tuple space / storage | [The tuple space (RSpace)](node/rspace.md), [Storage](node/storage.md) |
| Run a validator (hardware requirements / sizing) | [Running a validator: hardware requirements](node/validator-requirements.md) |
| Understand what a validator is **paid** and what it can **lose** (epoch pot, slashing, the draw) | [Validator economics](node/validator-economics.md) |
| Know whether the node is **ready for outside validators** (the testnet gate and the falsifiers that decide it) | [Testnet acceptance specification](spec/testnet-acceptance.md) |
| Know how the node **fails**, and what it does about it (the R1–R4 ladder, the silent paths, and the recovery that does not exist) | [Failure-mode HAZOP and disposition](spec/failure-hazop.md) |
| Build an app against a running node (deploy rholang, read responses) | [Building applications on the local devnet](developer/building-apps.md) |
| Understand the port (why Rust, module status) | [Part V](contributor/why-rust.md) |
| Find the machine-checked proofs | [`spec/Rchain/`](../../spec/Rchain/) (Lean), [`spec/coq/`](../../spec/coq/) (Coq) |

## The invariant catalog

rholo's behaviour is pinned by **<!-- counts:laws -->91 laws<!-- counts:end -->**, one per layer of the
system. The reading — one plain-English line per law, grouped by layer, with the numbering scheme and
the proved/assumed decomposition — is [The laws](formal/laws.md). The register
([`spec/LAWS.md`](../../spec/LAWS.md), emitted from `Rchain/Laws.lean`) is the authority for every
status and falsifier, and [`spec/INVENTORY.md`](../../spec/INVENTORY.md) is the long-form catalogue.
For the register's columns, the full namespace key and the Lean-first protocol, read
[Laws reference for AI agents](ai/laws-reference.md). Neither page is restated here: a catalog copied
into a third place is a catalog nothing checks.

## The formalization, in one screen

| Artifact | What it proves/states | Build |
|---|---|---|
| `spec/Rchain/*.lean` (Lean 4) | the model half of the register: each law's Lean declarations, its status (`proved-tied`/`proved-model`/`owed`/…), the axioms it rests on and the declarations that would falsify it. **Per-law status lives in one place** — [`spec/LAWS.md`](../../spec/LAWS.md), emitted from `Rchain/Laws.lean` and refused stale by the gate; this table no longer restates it, because a status repeated here is a status nothing checks (see `laws.md` for what that cost) | `cd spec && lake build` |
| `spec/coq/*.v` (Coq) | Laws 2–6 (substitution / α-equivalence metatheory) **stated** | `make -C spec/coq` |
| `spec/conformance/*.tsv` | the conformance corpora: *emitted* from the Lean definitions, committed, and read by a Rust consumer that runs the same cases through the node | `tools/emit-lean-corpus.sh` |
| `spec/INVENTORY.md` | the long-form law catalogue (its table ends at Law 77; the register covers <!-- counts:laws-entries -->91 laws and 131 entries<!-- counts:end -->) with source-of-truth + Rust realization | — |
| `spec/TYPE-SYSTEM.md` | the ρ→CoC type discipline (totality, refinements) | — |
