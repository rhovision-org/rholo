# rholo formal specification — the Coq track

The Coq formalization, parallel to the Lean 4 track in [`../`](../). Coq is the home of the
**substitution / α-equivalence / programming-language metatheory** (capture-avoiding de Bruijn
substitution, α-equivalence), while Lean 4 covers the algebraic/order laws. Both define the same flat
`Par` ADT — `Syntax.v` mirrors [`../Rchain/Par.lean`](../Rchain/Par.lean) field for field — and the
`Proc`/`score`/`parPair` names this README used to describe are from the retired Phase-0 binary form,
which `Syntax.v` replaced.

## Building

Requires the Coq pinned in [`coq-version`](coq-version) (8.18.0); the gate asserts the installed
version against that file rather than accepting "a Coq 8.x", because the definitions mean what one
version says they mean. The files use only the standard library — `List`, `NArith`, `Bool` — and no
`Autosubst`/`ssreflect`.

```sh
make                 # = coq_makefile -f _CoqProject -o CoqMakefile && make -f CoqMakefile
```

Install Coq if needed: `opam install coq` (or `apt install coq`).

## Status

`Syntax.v` defines the flat `Par` ADT; `Sort.v` declares the canonical order as **axioms** (`cmpPar`,
`sortPar`, `sortPar_idempotent`, `sortPar_comm`). `Laws.v` states Laws 2–6 (`alpha_equiv`, `substPar`
with `subst_commutes_sort`, `reduce`, `spatial_matches`, `closed` + `closed_decidable`) as **axioms** —
Coq owns the *definitions* (capture-avoiding de Bruijn substitution, α-equivalence, Autosubst-style),
which remain the Phase-1 obligations. **There are no theorems in these three files**; that is the honest
description of the track today, and the count of its axioms is what the gate prints.

`binds_at_most_once` was **deleted** on 2026-09-23. It stated a proposition Lean had found *false* as
written (AUDIT C26) and which Lean has since replaced with `linear`, a decidable predicate; an `Axiom`
for a false proposition is worse than no statement, because it makes the file unsound for the row it
claims. The Coq copy of that predicate is owed with the rest of the matcher.

This is **no longer a mirror of the Lean track**, and the difference is the part worth stating: Lean's
law 1a (`sortPar_idempotent`/`sortPar_comm`) is *proved*, and its `substPar`, `spatialMatch` and
`freeVarOf` are *definitions*, not axioms; the comparison this paragraph used to draw was against a
Lean it no longer describes. See [`../LAWS.md`](../LAWS.md) for what the Lean half actually rests on —
the register is emitted from `Rchain/Laws.lean` and refused stale by the gate, and the rows that make a
Coq claim carry a `coq` anchor (`spec/coq/<file>.v:<symbol>`) that the register resolves.

## Trust surface

Two checks, both in [`tools/check-lean-conformance.sh`](../../tools/check-lean-conformance.sh):

1. The installed Coq must match [`coq-version`](coq-version).
2. Every `Axiom`/`Parameter`/`Conjecture`/`Admitted`/`admit` (`Unset Guard Checking`/`Unset Positivity
   Checking` inclusive) under `spec/coq/` is counted and printed every run, against a hand-maintained
   ceiling. All of them are present today, so a zero-tolerance rule would be red on arrival; the ceiling
   is what makes the number move only deliberately, and it is printed so that an unchanged ceiling is
   visible rather than silent.

## Mapping

| Coq | where it comes from |
|-----|-------|
| `Par`, `Send`, `Receive`, `New`, `Match`, `Expr`, … (`Syntax.v`) | the flat `Par` of `models/proto/RhoTypes.proto` (Rust) — the Scala original is `legacy/models/src/main/protobuf/RhoTypes.proto`. The model carries the proto's 8 element lists; the Rust tree's proto adds `locallyFree`/`connective_used`, which the model does not |
| `parMerge` (`Syntax.v`) | the port's `|` / `par_merge` — the same operation `Rchain/Par.lean` calls `parMerge` |
| `cmpPar`, `sortPar` (`Sort.v`) | `models/src/sorter.rs` (or its Scala original, `legacy/models/src/main/scala/coop/rchain/models/rholang/sorter/ordering.scala`) — the comparator Lean's `Rchain/Sort.lean` defines and the register's law 1 is about |

See [`../INVENTORY.md`](../INVENTORY.md) for the full catalog (<!-- counts:laws-entries -->80 laws and 104 entries<!-- counts:end -->).
