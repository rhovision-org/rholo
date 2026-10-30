# Summary

[Introduction](introduction.md)

# Part I — Rholang & the ρ-calculus

- [Why rholang](rholang/why-rholang.md)
- [Models of computation](rholang/models-of-computation.md)
- [Processes and names](rholang/processes-names.md)
- [Sends and receives](rholang/sends-receives.md)
- [Names are quoted processes](rholang/names-are-processes.md)
- [Unforgeable names](rholang/unforgeable-names.md)
- [Patterns and matching](rholang/patterns-matching.md)
- [Data structures](rholang/data-structures.md)
- [Control flow and state](rholang/control-flow.md)
- [Joins and concurrency](rholang/joins-concurrency.md)
- [Object capabilities](rholang/object-capabilities.md)
- [Smart contracts](rholang/smart-contracts.md)
- [Language reference](rholang/reference.md)

# Part II — The ρ-calculus, formally

- [The formal layer: how to read this set](formal/README.md)
- [The laws](formal/laws.md)
- [Grammar and sorts](formal/grammar-sorts.md)
- [Substitution and matching](formal/substitution-matching.md)
- [Closedness and the Calculus of Constructions](formal/closedness-coc.md)
- [Concurrency: the model, the calculus, and the soundness theorems](formal/concurrency.md)
- [Effect scheduling](formal/scheduling.md)
- [Determinism of the block state transition](formal/determinism.md)
- [Cross-shard transactions](formal/cross-shard-transactions.md)
- [Progress: the shapes of non-progress](formal/progress.md)

# Part III — The node

- [Consensus (Casper)](node/consensus.md)
- [The tuple space (RSpace)](node/rspace.md)
- [Block merging (RCHIP-02)](node/block-merge.md)
- [Sorted matching (proposal)](node/sorted-matching.md)
- [Storage](node/storage.md)
- [Operating the node](node/operating.md)
- [Cross-shard invoke (remote deploy)](node/shard-invoke.md)
- [OCapN interoperability](node/ocapn.md)
- [ERTP: brands, issuers, purses and payments](node/ertp.md)
- [Running a validator: hardware requirements](node/validator-requirements.md)
- [Validator economics](node/validator-economics.md)
- [Scaling and performance limits](node/scaling.md)
- [Local devnet (Docker)](node/devnet.md)
- [The public testnet](node/testnet.md)
- [Running a public testnet of your own](node/running-a-public-testnet.md)
- [The history chain](node/history-chain.md)
- [Security audit (September 2026)](node/security-audit.md)

# Part IV — Building applications

- [Building applications: local devnet and public testnet](developer/building-apps.md)
- [Tokens in Rholang: ERTP](developer/ertp.md)
- [Talking to a node from another implementation](developer/ocapn.md)
- [Porting an app from rnode](developer/porting-a-client.md)

# Part V — Contributor / port

- [Why Rust](contributor/why-rust.md)
- [Architecture & port status](contributor/architecture.md)
- [The laws → Rust code](contributor/laws-to-rust.md)
- [On-chain validation phases (Laws 23–25)](contributor/onchain-validation-phases.md)
- [Formal specification & audit](contributor/spec.md)
- [Plan: post-quantum migration](contributor/post-quantum-plan.md)
- [Design: the genesis-contract upgrade path (RCHIP #43)](contributor/genesis-contract-upgrade-path.md)
- [Design: governance-set parameters (plan H2)](contributor/governance-parameters.md)

# Part VI — QuCalc: native AI & governance

- [Overview](qucalc/README.md)
- [Quantum operators → the ρ-calculus](qucalc/quantum-to-rho.md)
- [Multi-stakeholder governance](qucalc/multi-stakeholder-governance.md)
- [Architecture](qucalc/architecture.md)
- [Experimental: TreeProc + zero-action ledger concurrent reducer](qucalc/zfa-concurrent-reducer.md)
- [Using the extensions](qucalc/extensions.md)
- [Examples](qucalc/examples.md)
- [Upstream references](qucalc/references.md)

# Part VII — Testnet acceptance

- [Testnet acceptance specification](spec/testnet-acceptance.md)
- [The bounded adversary](spec/adversary.md)
- [Failure-mode HAZOP and disposition](spec/failure-hazop.md)

# Part VIII — Methodology (draft papers)

- [Paper A — The law-first port (draft)](methodology/law-first-port.md)
- [Paper B — Auditable development by AI agents (draft)](methodology/agentic-assurance.md)

---

- [Navigation for AI agents](ai-entrypoint.md)
