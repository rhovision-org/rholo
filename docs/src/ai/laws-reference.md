# Laws reference for AI agents

The law register is **checked data**, not prose. This page is the operating manual for it: the files,
the numbering, the status vocabulary, the commands, and the protocol an agent follows when it touches a
law. Read it before editing `spec/Rchain/`, `spec/laws.tsv`, `spec/findings.tsv`, or any page that
quotes a total. The human reading is [The laws](../formal/laws.md); where the two disagree, the register
is right.

## 1. The authority chain

| Artifact | What it is | Who writes it |
|---|---|---|
| [`spec/Rchain/Laws.lean`](../../../spec/Rchain/Laws.lean) | **The source.** Each law is a `Law` record: number, clause, layer, status, declarations, axioms, corpus, Rust anchors, Coq anchors, witness, falsifiability, statement, note, `rustWitness`. | a person, by hand |
| [`spec/Rchain/LawsMain.lean`](../../../spec/Rchain/LawsMain.lean) | **The checks.** Numbering `1..lawCeiling` with no gaps, `entryCeiling`, reference integrity, axiom accounting, a falsifier on every proved row, a real file behind every `rust` anchor, `fn <symbol>` behind every `rustWitness`. Runs at compile time. | a person, by hand |
| [`spec/laws.tsv`](../../../spec/laws.tsv) | **The emitted data.** One row per **entry**; columns: `number`, `clause`, `layer`, `status`, `declarations`, `axioms`, `corpus`, `rust`, `coq`, `witness`, `falsifiable`, `statement`, `note`, `rustWitness`. | `tools/emit-lean-laws.sh` |
| [`spec/LAWS.md`](../../../spec/LAWS.md) | **The rendered tables**, with a header summary of the status decomposition. | `tools/emit-lean-laws.sh` |
| [`spec/INVENTORY.md`](../../../spec/INVENTORY.md) | The long-form catalogue: source of truth → theorem → Rust test, per law. Its table ends at **Law 77**; the register is the authority for 1–91. | a person, by hand |
| [`spec/findings.tsv`](../../../spec/findings.tsv) → [`spec/AUDIT.md`](../../../spec/AUDIT.md) | The audit check-off: one row per finding, with its pass section and the law it instantiates. | a person; rendered by `tools/emit-findings-register.sh` |
| [`spec/audit/passes.md`](../../../spec/audit/passes.md) | The audit's evidence record, one `## N.` section per pass. **A record of what was true then** — do not rewrite its history. | a person |

Generated files are committed so the gate can compare. **Never hand-edit `laws.tsv` or `LAWS.md`** — they
are re-emitted and a diff is a failure.

## 2. The numbering key

Every identifier in the specification belongs to exactly one namespace. Mixing them is the most common
citation error.

| Form | Means | Lives in |
|---|---|---|
| `Law N` | a stable law number, 1–91. Numbers are never reused. | the register |
| `Law Nx` | a **clause** of a law whose parts differ in status or support (`14a`, `16b`). | the register |
| an **entry** | one row of the register; a law with clauses has one row per clause. <!-- counts:laws-entries -->91 laws and 131 entries<!-- counts:end -->. | `spec/laws.tsv` |
| `C###` | an **audit finding** (`C1`, `C337`). Never a law. | `spec/findings.tsv` |
| `S#`, `F#`, `H#`, `M#`, `R#`, `TS#` | earlier finding series in the same register (for example `TS` is the type-system pass). Same file, same `todo`/`in progress`/`done` states as `C###`. | `spec/findings.tsv` |
| `F-U<n>-<m>` | a **HAZOP deviation row**: guide word, study node `U<n>`, row `m` (`F-U5-03`). | [`failure-mode HAZOP`](../spec/failure-hazop.md) |
| `H-U<n>-<m>` | a HAZOP **hazard** row. | [`testnet acceptance`](../spec/testnet-acceptance.md) §1 |
| `U1`–`U17`, `TE-1`–`TE-6` | HAZOP **study node** and **top event** identifiers. | the HAZOP page |
| `R1`–`R4` | the HAZOP **disposition ladder** (accept / monitor / mitigate / refuse), not a finding id. | the same page |
| `W-N<n>-<m>` | a **wargame** row: node `n`, row `m` (`W-N1-04`). | [`the bounded adversary`](../spec/adversary.md) |
| `§N` | an **audit pass**: the `## N.` section that carries the full account. | `spec/audit/passes.md` |
| `F1`–`F6` | the **fundamentals** of the CoC type discipline — theorems, not findings. | [`grammar and sorts`](../formal/grammar-sorts.md) |
| `C.1`, `C.2`… | a **concurrency claim** in the formal layer — a theorem, not finding `C1`. | [`concurrency`](../formal/concurrency.md) |

**The pairs that look alike.** `C.1` (theorem) is not `C1` (finding). `F1` (fundamental) is not `F-U5`
(HAZOP node) and not an `F#` finding. `R1` (disposition) is not an `R#` finding. When a citation is
ambiguous, add the namespace word: “finding C1”, “Fundamental F1”, “pass §141”.

## 3. Status vocabulary

Status is per **entry**, not per law. The words are a closed set in `Rchain/Laws.lean`; a new one is a
change to the type and to every consumer.

| Status | Means | What it permits |
|---|---|---|
| `proved-tied` | proved, **and** tied to the node by a conformance corpus a Rust consumer runs | may be cited as holding of the code through the corpus |
| `proved-model` | proved about a model a person keeps in sync with the Rust | may be cited as a theorem; the model↔code gap is named in the note |
| `axiom-by-design` | a primitive assumed rather than proved (cryptography) | counted in the axiom total, never as coverage |
| `owed` | stated, proof outstanding; `falsifiable` names the witness that would close it | may **not** be cited as proved |
| `deferred` | deliberately postponed, with the reason in the note | the same restriction |
| `open` | a design question, not a proof gap | cite the question, never the invariant |
| `orphaned` | out of scope (the Rosette VM was not ported) | kept so the catalogue does not silently shrink |
| `vacuous` | proved, but the statement restates its own definition | visible; **never** counted as coverage |
| `retired` | the port has no rule of that shape; the note carries the evidence and the `rust` anchor names the code that was read | a decision taken, recorded |

A finding's state is a different, three-word vocabulary: `todo`, `in progress`, `done`.

## 4. Commands

| Command | What it does | When |
|---|---|---|
| `tools/emit-lean-laws.sh` | re-emits `spec/laws.tsv` and `spec/LAWS.md` from the Lean | after changing `Laws.lean` **on purpose**; read the diff |
| `tools/emit-lean-laws.sh --check` | refuses if the committed files are not what the Lean says | what the gate runs |
| `tools/emit-lean-counts.sh` | rewrites every `<!-- counts:KEY -->…<!-- counts:end -->` span from `laws.tsv` | after adding a page or a marker |
| `tools/emit-lean-counts.sh --check` | refuses a stale span **or a hand-written register total** in a scanned file | part of `check-lean-conformance.sh` |
| `tools/check-lean-conformance.sh` | the full formal gate: Lean and Coq builds, no `sorry`/`admit`, import completeness, corpus freshness, Rust agreement, axiom accounting, register currency, prose counts, Rust witnesses | before pushing anything under `spec/` or a counted page |
| `tools/check-rust-witnesses.sh` | runs each `path.rs:symbol` by name; a name matching no test fails | by the gate; manually with the `rustWitness` column |
| `tools/emit-lean-corpus.sh` | re-emits `spec/conformance/*.tsv` from the Lean definitions | after changing a modelled relation |
| `tools/audit-status.sh` | the check-off gate: malformed rows, unresolvable evidence, a law cell naming a law the register does not define | after editing `spec/findings.tsv` |
| `cd spec && lake build` | the Lean half | while iterating |
| `make -C spec/coq` | the Coq half | while iterating |

The witness script reads **one `path.rs:symbol` per line** from stdin, not the TSV. The gate derives the
column by header name:

```sh
awk -F'\t' -v c="$(awk -F'\t' 'NR==1{for(i=1;i<=NF;i++) if($i=="rustWitness"){print i;exit}}' spec/laws.tsv)" \
  'NR>1 && $c!="-" { n=split($c,a,", "); for(i=1;i<=n;i++) print a[i] }' spec/laws.tsv \
  | tools/check-rust-witnesses.sh
```

## 5. The Lean-first protocol

A law is not a comment added beside the code. The order is fixed:

1. **Write the Lean first** — the definitions and the theorem, in the layer's module under
   `spec/Rchain/`. Import it from `Rchain.lean` (an unimported module fails step 3 of the gate).
2. **Emit and tie the corpus** — if the law is model-checkable, add the corpus layer and its Rust
   consumer; a corpus with no consumer is refused. Re-emit with `tools/emit-lean-corpus.sh`.
3. **Add the register row** — number, clause, layer, status, declarations (checked **by name** against
   the elaborated environment), axioms, corpus, `rust` anchor, `rustWitness`, statement, falsifier.
4. **Raise the ceiling** — `lawCeiling` in `LawsMain.lean` and `entryCeiling` pin the shape. A new
   law without a raised ceiling fails numbering; a ceiling raised without the row fails the count.
5. **Point the Rust at it** — the `rust` cell names a file that must exist; `rustWitness` names a test
   the gate runs. Grep for the **behaviour**, not the law number.
6. **Re-emit and sweep the prose** — `emit-lean-laws.sh`, then `emit-lean-counts.sh`. A total written
   by hand in a scanned file is a failure.
7. **Run the gate** — `tools/check-lean-conformance.sh`, then the targeted Rust test.

### Failure modes, with the fix

| Symptom | Cause | Fix |
|---|---|---|
| “missing number N” | a gap, a duplicated row, or a forgotten ceiling raise | number contiguously; keep one literal per chunk |
| `entryCeiling` mismatch | a row added/removed without updating the pin | update the ceiling **and** the counts |
| `counts:` gate fails | a stale marker, or a hand-written total in a scanned file | re-emit; wrap the number in a marker |
| “witness names no theorem” | the declaration was renamed or lives in an unimported module | fix the `declarations` name; import the module |
| “witness matched no test” | the test was renamed, deleted or `#[ignore]`d | fix the anchor, or the test is no longer evidence |
| “no such file” for every witness | the TSV was fed to `check-rust-witnesses.sh` instead of a `path.rs:symbol` column | extract the column first (above) |
| stale corpus | the Lean definition changed and the corpus was not re-emitted | `tools/emit-lean-corpus.sh`, then the consumer |
| axiom accounting fails | a row rests on an axiom the tree does not declare, or vice versa | reconcile the `axioms` cell with the declaration list |
| `sorry`/`admit` refused | a proof was left incomplete | prove it, or `owed` it and say so |

A green gate is a statement about **shape**, not about semantics: it proves the register is internally
consistent and the named theorems exist. Whether a law says the right thing, and whether the Rust is
honest about it, is a reading — and the falsifier is where that reading is recorded.

## 6. Do not

- **Do not hand-write a register total.** Use a `<!-- counts:KEY -->…<!-- counts:end -->` span; the
  key set is in `tools/emit-lean-counts.sh`.
- **Do not cite a law without its clause** where the clauses differ in status (`14a`, not `14`).
- **Do not call a `C###` a law**, or a law a finding. They are different namespaces with different
  lifecycles.
- **Do not edit a generated file** (`laws.tsv`, `LAWS.md`, the corpora, `AUDIT.md`) — change the source
  and re-emit.
- **Do not rewrite a historical pass** in `spec/audit/passes.md`; a correction is a new pass, not an
  edit to the record.
- **Do not treat CI green as a proof of semantics.** It is a proof that nothing drifted from what the
  register claims.
- **Do not add a row without a falsifier** — `falsifiable := none` on a proved entry is refused.
- **Do not leave a `candidate:` slug and call it a law.** A candidate has no theorem; the property is
  proposed, not established.

## 7. Where to read next

- [The laws](../formal/laws.md) — the human reading, one line per law.
- [`spec/LAWS.md`](../../../spec/LAWS.md) — every entry's status, declarations, axioms and falsifier.
- [`spec/INVENTORY.md`](../../../spec/INVENTORY.md) — the long-form catalogue (through Law 77).
- [The laws → Rust code](../contributor/laws-to-rust.md) — per-law Lean and Rust anchors (Laws 1–29).
- [The formal specification tree](../../../spec/README.md) — the oracle and every gate.
- [Navigation for AI agents](../ai-entrypoint.md) — the goal-indexed map of the book.
