# Preregistration — the durable-state census (issue #658)

**Written before the rows were derived.** This file fixes the hypotheses, the class vocabulary, the
resolution rules and the completeness contract. The derivation (`results.md`), the classifier's output
(`durable-states.tsv`) and the HAZOP rows were authored against it, not the other way round. If a row
departs from these rules, the departure is named in `results.md`.

**Reviewed revision.** `dev` at `f9e509038` (2026-11-23), fetched at the start of the pass. Every
`path:symbol` in the census is re-derived against this revision; a citation is never remapped by a
delta.

## The starting hypotheses

The issue's first-pass list, carried as hypotheses to confirm or reject (not as rows to copy):

1. reset marker `last-reset` (Rewinding / Rewound)
2. autopropose halt record `autopropose-halt`
3. self-replay record `self-replay-record`, plus one reset per anchor
4. own signed sequence `own-latest` (Law 122)
5. block disposition ledger: Pending (Stored / Validating / Retry / Deferred), Faulted, equivocation
   verdict
6. key-equivocation halt
7. DAG `restore_attempts` / `unrestorableRecords` / `validation_failed`
8. store-format and shard-id refusals at boot
9. legacy body-scan marker; legacy disposition v3 copy
10. equivocation evidence DB
11. gateway transaction records
12. durable health counters (stall episodes, unrestorable)
13. checkpoint, approved and audit stores
14. RSpace root and history (leaf-before-root)
15. LFS / catch-up in-progress state
16. proposer self-validation halt paths

A hypothesis may resolve to a Table D row, to a Table F exclusion with a reason, or to "in-memory,
not durable — RC-4's domain". The census does not inherit a row because the issue listed it.

## The completeness contract

The set of durable identifiers is derived, not remembered:

- every `Db::new(...)` and `Db { id: ... }` inside `casper/src/storage.rs::rnode_db_mapping()`;
- every `pub const *_KEY` / `*_DB` / `*_MARKER` and `const CURRENT_ROOT` / `*_COMPLETE` in the store
  files named in U26's recipe, stopping at the first `#[cfg(test)]` in each file;
- the two on-disk markers (`shard-id`, `store-format`).

`tools/emit-durable-states.sh` is the executable recipe. `tools/check-durable-state-census.py`
refuses (a) an emitted identifier with no Table D row or Table F exclusion, (b) a Table D/F identifier
that is not emitted, (c) an `owed:` test not counted in §7, (d) an `owed:` surface with no
`node-diagnostics.md` §5 row, and (e) a §5 row the census does not cite and that is not one of the
declared non-durable gaps.

**The declared limit.** The scan covers the store files named in the recipe. A durable store added in
a new module is added to the recipe deliberately; the census does not claim to discover it.

## Resolution classes and the rules for assigning them

- **auto-recover** — the node clears or repairs the state on boot or the next advance. Assigned only
  when a named detector, a named clearer, and a restart/crash test already exist. If the mechanism
  exists but no test proves it, the row is *not* auto-recover: it is classed by what is true today and
  the test is recorded as `owed:`.
- **operator-action** — the state is correct to persist until an authenticated operator acts. This
  includes append-only evidence and monotone counters, which the node must not clear: the class says
  who may end the state, not whether an end is desirable.
- **safe-halt** — the node must stay halted, or refuse to open the store, to protect consensus. The
  row must name the invariant/law, the surface, and the documented exit (itself an operator-action).

A row whose class is `auto-recover` but whose test proves only the writer, not the clear, is a failed
derivation; `results.md` records that as a limit rather than promoting the row.

## Lean-first rule

The census changes no Rust. A row whose follow-up changes Rust earns its theorem before the code; a
row that needs a law the register does not have carries `candidate:<slug>` now, and the follow-up
writes the theorem first. A node-local record with no consensus property may say `vacuous:` with the
reason instead of inventing a theorem.

## Negative controls (to be run and recorded)

1. remove one Table D row — the checker is red because an emitted identifier is unclassified;
2. change a class to `auto` — red;
3. point a test at a symbol that does not occur in its file — red;
4. add an `owed:` test without updating §7 — red;
5. cite an `owed:` surface with no §5 row — red;
6. add a `#[cfg(test)]` key to a scanned file — green (test-only keys are excluded).

The untouched page must be green in both report and `--gate` modes.
