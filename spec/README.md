# rholo formal specification (Phase 0)

A machine-checked specification of the mathematical invariants that govern the rholo node, written
as a [Lean 4](https://lean-lang.org/) formalization. It is the source of truth the Rust node is
written against: every law here maps to a Rust property/differential test in later phases.

## Why

The full motivation — memory safety and the calculus-native expression of the node (λ → π → ρ →
Calculus of Constructions) — is in [`../docs/src/contributor/why-rust.md`](../docs/src/contributor/why-rust.md). The node
(Scala + a C++ Rosette VM) is broadly sound, so the rewrite is not a correctness repair. This spec's
job is therefore **preservation under translation** — pin down the invariants so the port cannot
silently drop them.

## Building

```sh
cd spec
lake build        # the library, the corpus emitter and the law register
```

Three things are compiled, and all three matter. The library is the formalization. `rchain-corpus` is the
conformance emitter, whose `decide`d verdicts are what the Rust tests read 1:1. `rchain-laws` is the law
register, and *elaborating it is the check*: its compile-time checks (numbering, reference integrity, axiom
accounting in both directions, non-vacuity) only run when the module is built, so a `sorry` scan cannot
stand in for them. `defaultTargets` in `lakefile.toml` names all three so that a bare `lake build` is the
whole gate; `tools/check-lean-conformance.sh` remains the full gate (it additionally re-emits the corpora
and the register and refuses a diff).

The build requires **Mathlib** (pinned to `v4.12.0` in `lakefile.toml`); the `.lean` files already
import `Mathlib.Data.*`/`Mathlib.Order.*` for `Multiset`/`Finset`/`Order`.

## Layout

```
spec/
  Rchain.lean          root module (imports everything)
  Rchain/
    Syntax.lean        Ground / Var — the core Rholang scalar ADT (de Bruijn levels)
    Par.lean           the flat `Par` ADT (8 list fields) + `nilPar`/`parMerge`
    Cmp.lean           the `Comparator` scaffold + `sortList`
    Rho.lean           Law 2 core (`StrCong` ≡) + Law 4 core (`Reduce` ⟶ COMM)
    Sort.lean          Law 1: canonicalization `sortPar` + `sortPar_idempotent`/`sortPar_comm`
    Ty.lean            the CoC layer: `PSort`/`Closed` (Law 6) + the proven fundamentals
    Subst.lean         Law 3: capture-avoiding substitution (`sort ∘ subst` commute)
    Reduce.lean        Law 4: determinism + `new` freshness
    Match.lean         Law 5: `BindsAtMostOnce` + decidable spatial matching
    FreeVars.lean      Law 6: `freeVarOf` + `Closed ↔ no free vars`
    Effect.lean        Law 9 (effect level): disjoint-closure commute + `effect_reorder_diverges`
    Scheduler.lean     Laws 20–22: claim-queue path order, gate refinement, depth-2 counterexample
    SchedulerOnchain.lean  Laws 23–25: validated speculation (write-record layer + certificate + fallback)
    CrossShard.lean    Laws 26–29: shard scope determinism + 2PC cross-shard atomicity
    Concurrent.lean    concurrency-model soundness theorems
    Tree.lean          tree-model confluence up to `StrCongT`
    RSpace/            Laws 7–11: Join/Comm/Merge/Merkle (7–10 proven, 11 vacuous)
    Casper/            Laws 14b, 15, 16, 18, 52–54, 58, 66–68, 82, 85: the fringe, the DAG,
                       block/merge/storage validation, the finality gate's partition and its
                       forward dual, the validator lifecycle, the record and the fringe state
    Crypto/            Law 19: Random/Spec (axiomatized by design)
  INVENTORY.md         the law catalog (Laws 1–77; the register covers 1–91): source-of-truth → theorem → Rust test
  cost-register.tsv    the cost register: every priced operation, the shape of its charge, and its residual
  COST.md              the rendered register (emitted from the TSV; refused stale)
  STYLE.md             conventions of this tree: naming, measured options, what is deliberately absent
```

Laws 12–13 (Rosette) are **orphaned**: the `rosette`/`roscala` VM is out of scope (not wired into
`build.sbt`); they are documented in `INVENTORY.md` but have no Lean files.

## Proven vs stated

**Per-law status lives in one place.** [`LAWS.md`](LAWS.md) is emitted from `Rchain/Laws.lean` and the
gate refuses a stale copy: each row says what a law's proof is worth — `proved-tied` (proved *and* tied
to the running node by a conformance corpus), `proved-model` (proved about the Lean model), `owed`,
`open`, `orphaned`, `vacuous`, `retired` (no rule of that shape in the port), or `axiomatized by design` —
together with the axioms it rests on and the declarations that would falsify it.

This section used to restate that per law, and it is a fair example of why it should not: by the time it
was replaced it had Law 1 resting on thirty element-comparator axioms (the register counts four),
described Laws 26–29 as "stated" (two of them are `proved-model`), and called Law 3's substitution
"stated" long after it became a definition. A status repeated by hand is a status nothing checks — and
the numbers are spelled out here on purpose: `tools/emit-lean-counts.sh` fails on a register total
written in digits outside a generated span, which is how this paragraph was caught when it first quoted
the old figure, and a *historical* figure is still a figure a reader may believe.

Three categories are worth stating *here*, because they are not per-law facts:

- **`axiomByDesign`** — Law 19's cryptographic primitives (Blake2b, secp256k1, Curve25519) are abstract
  interfaces whose properties are *postulated* (`Crypto/Random.lean`, `Crypto/Spec.lean`). Proving real
  crypto is out of scope; this is the register's one deliberate boundary.
- **`proved-tied` vs `proved-model`** — a conformance corpus is the only mechanical Lean↔Rust link this
  repo has, so only a law whose layer has a Rust consumer can be `proved-tied`. The rest are claims
  about a model a human keeps in sync, and the register says which rows are which.
- **The Coq track** ([`coq/`](coq/)) states its laws as axioms and proves nothing yet; the register's
  rows that make a Coq claim carry an anchor the emitter resolves, and the gate counts the Coq axioms
  against a printed ceiling and pins the Coq version.

## How laws drive the Rust port

| Spec artifact | Rust counterpart |
|---------------|------------------|
| `Sort.sort` (canonical form) | `Sorted<Par>` (canonical `Eq`/`Ord`/`Hash`/`Serialize`) |
| a proven law `L` | a `proptest`/`quickcheck` property asserting `L`, plus a differential test feeding identical inputs to the Scala node and comparing state hashes |
| `Syntax.Proc` (the ADT) | reference for the Rust data model and its `Eq`/`Hash` derivations |
| axiomatized crypto interfaces | Rust traits pinned by known-answer vectors |

## Ground truth

The laws are validated against — not replacing — the Scala tests that already encode them:

- `rholang/src/test/scala/coop/rchain/rholang/interpreter/{ReduceSpec,ReplaySpec}.scala`
- `models/src/test/scala/coop/rchain/models/rholang/SortTest.scala`
- `node/src/test/scala/coop/rchain/node/mergeablity/MergeabilityRules.scala`
- `casper/src/test/scala/coop/rchain/casper/batch1/MultiParentCasperReportingSpec.scala`

## Operational incident evidence

- [N=8 finality and bootstrap recovery, 19 October 2026](audit/evidence/n8-finality-20261019/README.md): source lineage, contribution attribution, passing campaigns and the remaining no-helper gate. Historical evidence; no change to the law register or finding status.

## Release assurance records

- [Hard-fork tracker closeout](audit/hardfork-51-closeout.md): executable accountability and closure refusal.
- [Current release gap matrix](audit/final-gap-matrix.md): exact tracker obligations and the pinned section-six reverse inventory; independent acceptance and sign-off remain separate.
