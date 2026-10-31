# Affordances — what the language let the architecture do

[`RUST-VS-SCALA.md`](RUST-VS-SCALA.md) registers where this port **differs from the reference**, and
gives each divergence its reasoning without giving it a verdict. This page is the other half: what the
port could **build** because it is Rust — decisions that the JVM made impossible, or that it made
possible only as a convention a reviewer had to remember — each with the artefact that evidences it and
the cost the JVM would have charged.

**The rule this page exists to enforce: a divergence is not a gain.** A gain is a divergence with an
artefact — a compile failure, a named test, or a committed run. Every entry below carries one, and every
entry names the pass that found it. Where an entry cannot carry an artefact, it says so.

## 0. What this page is, and is not

- **It is** a catalogue keyed by the *language property*, not by the finding. Each entry is:
  what the JVM forced → what Rust afforded → what landed (file) → the evidence → the JVM's cost.
- **It is not** a claim that divergence is progress. Some divergences in §3 of the companion page are
  defensive (a bound the reference lacked) and some are refusals (a call the reference accepts and this
  one does not); whether each is *better* is argued there and not assumed here.
- **It is not** an external-assurance claim. The evidence below is this project's own: multi-lens
  adversarial passes with a rebuilt refutation stage, 309 findings closed. That is independent **by
  lens** and not independent **by party** — no external auditor has read this tree, and none of the
  entries below should be read as though one had. The two axes are different and this page does not
  blur them.

**Two entries claim *impossible*; the rest claim *much harder*.** The difference is marked per entry.
`[II]` — impossible: no JVM build does it. `[PI]` — possible but *prior*, i.e. the JVM could express it
and the claim is about cost and enforcement, so the JVM column is an **argument, not a measurement**.

---

## 1. The enabling pair: no `unsafe`, and no runtime `[II]`

**JVM.** Memory safety is real, and it is *rented from the runtime*: the process carries a collector
and a JIT, and in the Scala node it also carried a C++ Rosette VM in the same address space behind
JNI — the boundary at which the JVM's safety stops. You cannot remove the runtime to keep the safety,
and you cannot compile the node to a target the runtime does not exist for.

**Rust.** `#![forbid(unsafe_code)]` at the root of every one of the thirteen library crates (the
fourteenth workspace member, `rspace-bench`, is a benches-only crate); `tools/audit-type-system.sh` finds zero
production `unsafe` (the type-system gate, re-run on this tree 2026-10-11 via `make check-register`:
clean). The safety is *structural* and the runtime is *absent*: a single static musl binary, counted at 37 MB
(2026-10-10), with no collector inside the process to pause.

**What that makes possible, that a JVM build cannot do at all:** the reducer compiles for
`wasm32-unknown-unknown`, and CI **enforces that it stays compilable** —
`.github/workflows/ci.yml:219` runs `cargo check -p rchain-rspace --target wasm32-unknown-unknown` and
the same for `rchain-rholang`, with the comment naming Issue #98. A guarantee that holds because
*nothing in the dependency graph needs a runtime* is not a guarantee the JVM can make; it is the
difference between memory safety you own and memory safety you rent.

**Evidence.** `#![forbid(unsafe_code)]` × 13 (verified per crate, 2026-10-11); the `wasm32` job in
`ci.yml`; the type-system gate green on this tree.

---

## 2. Invalid states, unrepresentable `[PI]`

**JVM.** `null`, unchecked casts, boxed primitives, and the sentinel: a height of `-1` meaning "none",
a `Vec[Item]` whose "exactly 256 slots" is a comment, a "negative gas cost" that is a `Long` until it
meets a `uint64`.

**Rust.** The invariant is the type. Refinement newtypes (`NonNegI64`, `BlockHeight`, `SeqNum`,
`Port`, `WireLen`, `Hash32`) carry their invariant with no `Deref` and no public field, so an invalid
value is not *rejected* — it is not *constructible*. `[Item; 256]` is the radix node. `Sorted<Par>`
(`models/src/sorted.rs:26`) makes canonical form a property of the value: its `Eq`/`Ord`/`Hash`/
`Serialize` are derived *coherently*, so a term cannot be hashed in two orders.

**The falsifier is the strongest kind this project recognises: a compile failure.** A finding whose
close condition is "the bad state is no longer representable" cannot regress silently, because the
regression does not build. Three register rows are exactly this (`spec/findings.tsv`):

| Row | was | now |
|---|---|---|
| `TS1` | `fringe_height` returned an int with a `-1` sentinel | `Option<BlockHeight>`, and `message_from_block_metadata` no longer discharges the negative — `casper/src/dag.rs` |
| `TS2` | a height delta was a bare integer | `Add<NonNegI64>`, so the delta carries non-negativity |
| `TS3` | `Node` was a `Vec` with an implicit 256 | `Node = [Item; NUM_ITEMS]`, `std::array::from_fn` |

**JVM cost.** Scala can write a `BlockHeight` class and a `TreeMap`; what it cannot do is make the
*use* of them obligatory. The compiler that refuses you is the affordance, not the wrapper.

---

## 3. Determinism as a type property `[PI]` — and the live case that proves the difference

**JVM.** `HashMap` iteration order fed `New.injections` and matcher state in the reference — the
classic JVM fork source, and the reason a consensus node has to *hope* every map is a `TreeMap`.

**Rust.** `BTreeMap`/`BTreeSet` throughout; `Sorted<Par>` as the canonical value. The merge is then not
*tested* for commutativity — it is **a commutative monoid that can be stated and proved**: Law 9
(`StateChange::combine`), Law 10 (content-addressed trie), Law 17 (the rejection winner is a unique
lexicographic minimum), each with a Lean row and a Rust witness, and 321 executed conformance rows
across 17 layers behind the ones that are `proved-tied`.

**The live case is the strongest evidence on this page, and it is a *negative* result that became a
fix.** On 2026-10-08 the public testnet split four ways: four validators produced four `#106` blocks,
each citing a *different* hash for the same `#104`, and finality froze for over thirteen hours
(`spec/audit/evidence/te-1-2026-10-09-four-divergent-heads.md`). The carrier, once found, was a store
holding a **derived value at a fringe key** — a map whose value was not a function of its key. The fix
(`C215` → `C250` → `C270`, passes §84–§96) was not to tolerate the disagreement, nor to tie-break it:
it was to **delete the store** so the value cannot exist. `models/src/fringe_data.rs` is gone; the
claim now travels with the block that made it, written once
(`models/src/block_metadata.rs:215`, `BlockMetadata.fringe_state_hash`). **Law 67 was retired with the
mechanism it described** — the register shrinks when the object it was about does not exist, which is
what "unrepresentable" means operationally.

**Measured.** A four-validator devnet on the rig that produced the divergence ran level to
212/212/213/212 with **0** self-rejections, **0** `active-bonds` failures, autopropose running, and
identical block-hash sets at every sampled height.

**JVM cost.** A `TreeMap` fixes the iteration order of *one* map. It does not give you a language in
which "this value is a function of its key" is a property you can enforce, which is why the fix here
was a deletion rather than a comparator.

---

## 4. Concurrency as a compile-time property, and a residue you can *enumerate* `[PI]`

**JVM.** Shared mutable state behind `Ref`/`var`; in the reference, a global `connections` write-lock
held across I/O and a mutable `BlockRetriever` map. Whether a lock outlives an `await` is a code-review
question — and one the reference got wrong.

**Rust.** `Send`/`Sync` make "who may be touched from another thread" a compile-time fact, and
`Mutex`/`RwLock` make the *scope* of a mutation explicit. But the architectural artefact that a JVM
cannot produce is the **census**: `tools/check-spawn-observability.sh` enumerates every detached task
on the ingress path and requires each to be supervised, held, or **bare with a written reason** it
cannot be dropped unnoticed. Measured on this tree, 2026-10-11: **30 sites — 6 supervised, 7 held, 17
bare**, each bare one carrying the argument for why the failure it represents is visible elsewhere.

That census is not a style; it is what replaced "we audited the `Future`s" with a finite list a gate
re-checks. The `clippy::await_holding_lock` lint is still allow-listed, and the only occurrence in
production code is a comment explaining the decision (`node/src/effects/repl_client.rs:57`).

---

## 5. Partiality is finite, gated, and argued `[PI]`

**JVM.** `throw`/`catch`, `???`, `BugFoundError`. A `???` stub deep in a `Future` aborts a task
silently, and there is no list of the ways a JVM program can fail — unchecked exceptions are not a
set you can enumerate.

**Rust.** `Result`/`Option` at every boundary, and then the part that follows from *having* a closed
set: the panic surface is **enumerable and enumerated**, by hand, with reasons.
`tools/audit-type-system.sh` strips `#[cfg(test)]` blocks and holds a whitelist of **29 production
panic sites**, each an entry `(file ;; pattern ;; guard ;; reason)`; any *unlisted* production
`unwrap`/`expect`/`panic!`/`unsafe` is a hard failure. I ran it: clean on this tree.

The list is worth reading for what it shows about the method, not just the count:

- Most entries are **length asserts on fixed-width values** whose wire paths were re-walked to use
  checked `TryFrom` (`C97`), so the panic is on internally-produced data only — and two entries
  exist precisely because the *first* justification ("an internal length assert") was wrong.
- One entry is a `const _: () = assert!(...)` (`casper/src/engine/catchup.rs`): a check evaluated
  **while the crate compiles**, so it cannot fire at run time at all. A compile-time assertion on the
  relationship between two coupled constants is not expressible as a JVM runtime check.
- One entry (`ocapn/src/owner.rs`) is a panic chosen *over* totality, with the argument that the
  alternative — `Option::take` returning a closed channel — would drop answers with no error.

**The honest half, which this page will not omit.** The gate also *counts* classes it does not
*enforce* — `cast` 366, `index` 414, `div` 77, `overflow` 41 — and reports them. Green means "no
unlisted panic", not "no panic can fire": the 29 guards are arguments a reviewer made, not proofs.
The counted classes are the next rung and the gate says plainly that it does not hold it.

---

## 6. The cost model can be made to *track* work, by construction `[PI]`

**JVM.** `Costs.toProto` was `PCost(c.value)` — a negative `Long` wrapping into a `uint64` — and the
reference charges *flat* rates for operations whose cost is not flat, which is what lets a phlo buy
unbounded work.

**Rust.** Two things became available that the reference did not use. First, the sign is a type:
negative cost is rejected at the boundary (`casper/src/runtime_manager.rs`), not wrapped. Second — and
this is the architectural one — a charge can be computed as **the bound on the output, in constant
time, before the allocation it bounds**. `replace`'s charge is its output size, not its inputs, so a
10 GB amplification is stopped by the phlo budget before the allocation (`RUST-VS-SCALA.md` §3.11);
`--`/`.diff`/`indexOf`/`contains` are charged the *product* of their operands, because a linear scan
per element is `n·m` and charging one factor is the sublinear-charge shape that made the per-block cap
inoperative (§3.10). The same shape is the collection one: `toSet`, `toMap`, `set` and `keys` charge
three phlo per element of the output they canonicalise, and a union charges both operands — the October
2026 re-audit's finding, and the reason it was found by measurement rather than by a gate is that
nothing in the tree compared a charge with its work (`RUST-VS-SCALA.md` §3.13, Law 91a, pass 149). **And
the comparison is an instrument now**: `spec/COST.md` is a register of every priced operation with the
shape of its charge, `tools/check-cost-register.py` refuses an arm that declares none, and
`rholang/tests/cost_shapes.rs` measures that a declared shape still grows. Its first census found two
more of the same class (`set.add`, and the tallies' uncounted option lists), which is the honest
measure of how long "by construction" was a claim rather than a construction (pass 150, Law 91c).

**Where this page earns its honesty.** One of the planned fixes — a per-byte charge on the crypto
builtins — was **written, tested, and reverted**, because implementing it showed *the finding was
wrong*: those bytes must already have crossed the storage path, which charges proportionally, so the
flat charge bought no unbounded work. The test written for it would have gone on passing if the charge
were deleted — a vacuous test — and it is not in the tree. A catalogue of "found to be better" that
did not carry its own refutation would be the thing this project's audit culture exists to catch.

---

## 7. Why this is a language property, not a style choice

Each affordance above reduces to the same mechanism: **the compiler is the enforcement point, so an
invariant can be named and made to fail a build.** The artefacts that follow:

- `#![forbid(unsafe_code)]` at thirteen crate roots — a whole class removed at the crate boundary.
- `tools/check-rust-witnesses.sh` requires every registered law witness to resolve to a real symbol in
  its file and **refuses a zero-match filter**, so a renamed or `#[ignore]`d test cannot pass as a
  witness — the anti-vacuity clause exists because a test that runs nothing and exits 0 is
  indistinguishable from a test that passes.
- `tools/check-lean-conformance.sh` **executes** the corpus: it builds Lean and Coq, re-emits every
  `spec/conformance/*.tsv`, refuses a stale diff, and runs the named Rust consumer for each corpus,
  failing if one has vanished.

None of these is a Rust *syntax* feature; each is possible *because* the invariants live in the types,
where a tool can name them. That is the distinction this page is about: Rust did not make the node
correct — it made certain *incorrectnesses unbuildable* and the rest *enumerable*.

---

## 8. Provenance — which pass found each

| Entry | Artefact | Found by |
|---|---|---|
| §1 no `unsafe` / wasm target | `ci.yml:219`, 13 crate roots | #98 (PR #187/#188) |
| §2 unrepresentable states | `TS1`/`TS2`/`TS3`; `models/src/sorted.rs:26` | the type-system sweep (`TYPE-SYSTEM.md`) |
| §3 determinism / the split | `te-1-2026-10-09-four-divergent-heads.md`; `C215`→`C270` | the live incident, then passes §84–§96 |
| §4 the spawn census | `tools/check-spawn-observability.sh` | `passes.md` §93–§96 |
| §5 the panic whitelist | `tools/audit-type-system.sh` `WHITELIST_PANIC` (29) | the type-system sweep, `C97`, `C255` |
| §6 proportional charges | `models/src/sorter.rs`, `rholang/src/accounting.rs` | the September 2026 audit, §3; the collection conversions and censure by the October 2026 re-audit, pass 149 (Law 91) |

---

## 9. What this page does not claim

1. **Rust did not make the node bounded.** No collector is not a bounded footprint: memory that
   grows is safe code, and this node's DAG residency is Θ(N²) **by recorded decision**, measured live
   at 556 MB on a 5,885-block chain (`RUST-VS-SCALA.md` §4, H6). A memory-safe language cannot make an
   unbounded design bounded.
2. **"Impossible" is claimed for two things only** — §1's wasm target and the structural
   absence of `unsafe` *without a runtime*. Every other entry is `[PI]`: the JVM could express it, so
   the claim is about cost and enforcement, and the JVM side of each row is an **argument**, not a
   measurement. Where this page says "much harder", read it as a claim about reviewability, not about
   compile times.
3. **The assurance is internal.** Lens-independence is not party-independence (§0). The 309 closed
   findings and the incidents they closed are this project's evidence about itself, and the record
   includes the case where the project's own account lagged its tree by a week.
4. **A green gate is a claim about the gate.** The panic whitelist is a review of 29 sites, the
   counted classes are unenforced, and the corpus tie covers the laws that are `proved-tied` — not the
   63 that are `proved-model`, which are claims about a Lean model a human keeps in sync.

---

## 10. Cross-links

- [`RUST-VS-SCALA.md`](RUST-VS-SCALA.md) — the divergence register: where this port differs, with
  reasoning and without a verdict.
- [`TYPE-SYSTEM.md`](TYPE-SYSTEM.md) — the ρ→CoC type discipline and the refinement types of §2/§5.
- [`AUDIT.md`](AUDIT.md) · [`audit/passes.md`](audit/passes.md) — the check-off and the pass record.
- [`../docs/src/node/security-audit.md`](../docs/src/node/security-audit.md) — the comparative review
  the §6 charges came from, and §5 of that page (*what a green register does not cover*).
