# The formal layer: how to read this set

This folder is the reader-facing half of the machine-checked formalization in
[`spec/`](../../../spec/README.md). It is organised **one document per layer of the system**, and each
document says which **laws** it carries. The register (`spec/LAWS.md`, emitted from `Rchain/Laws.lean`
and refused stale by the gate) is the authority for every law's status;
[`spec/INVENTORY.md`](../../../spec/INVENTORY.md) is the authority for its source-of-truth pointers and
its Rust realization. These pages point at those rather than restating them.

## The documents, and what each one is about

| Document | Laws | What it is about |
|---|---|---|
| [The laws](laws.md) | <!-- counts:laws -->80 laws<!-- counts:end --> | the full set, grouped by layer, each row mapped to the feature it pins and the file it lives in |
| [Grammar and sorts](grammar-sorts.md) | 2, 3, 6 | the ρ-calculus as the base sort of a Calculus of Constructions: what a name, a pattern and a process *are*, and which of them a binder may mention |
| [Substitution and matching](substitution-matching.md) | 2, 3, 4, 5 | the two operations every rholang term goes through, and what happens when a pattern matches nothing |
| [Closedness and the Calculus of Constructions](closedness-coc.md) | 6 | the type discipline's own story: what "no globally free variable" buys, and what it refuses |
| [Concurrency: the model, the calculus, and the soundness theorems](concurrency.md) | 2, 4, 7–11, 19 | the four concurrency levels, the core relations `≡` and `⟶`, the per-law concurrency profile, and why disjoint *footprints* do not commute |
| [Effect scheduling](scheduling.md) | 20–25 | the three scheduler tiers: the effect level (S.1–S.4), the per-channel claim queue with the DFS gate, and on-chain validated speculation |
| [Determinism of the block state transition](determinism.md) | 14–16, 19 | what determinism means for a block: the fringe, the DAG, the merge, and the RNG |
| [Cross-shard transactions](cross-shard-transactions.md) | 26–29 | the two-phase-commit flow across shards, its roles, its state machine, and its recovery caveat |
| [Progress: the shapes of non-progress](progress.md) | 51–54 | the vocabulary for an obligation that cannot be met — its three shapes, its two predicate causes, the hypotheses each law carries, and the table that turns a symptom into a law |
| [The laws](laws.md) — the sets after progress | 55–79 | bounded work and the rooted namespace (55–56), the delegator's stake (57), the finality gate's forward dual (58), the wire and session laws (59–65), the distributed-system set (66–71), the durable reset, the stop and the read construction (72–77), the genesis's developer allocation (78), and the schedule that allocation unlocks on (79) |

## What the set is, and what it is not

- **It is a rendering, not the authority.** Every claim about a status belongs to the register; every
  claim about what the *code* does belongs to `spec/AUDIT.md` (the check-off) and `spec/audit/passes.md` (the evidence); the per-row catalog of invariants, sources
  and Rust types is `spec/INVENTORY.md`. These pages exist to be read by a person.
- **Two relations run through all of it.** The **corpus** (`spec/conformance/*.tsv`, emitted from the
  Lean definitions and read back from the node by a Rust consumer) is what ties a model to the code; and
  the **gate** ([`tools/check-lean-conformance.sh`](https://github.com/rhovision-org/rholo/blob/dev/tools/check-lean-conformance.sh))
  is what refuses a `sorry`, a stale corpus, an unimported module or a corpus with no consumer.
- **Some rows are deliberately not modelled**, and each says so where it is: the Rosette actor VM is
  orphaned; the grammar, the parser and the printer are modelled as data with a corpus of their own (rows 30, 31, 33), while the normalizer's output has no corpus to be held to (row 36); and the
  denied-deploy row is an open decision shared with the Scala rather than a port divergence. What is not
  formalized is named here, and the register is where that is counted.
