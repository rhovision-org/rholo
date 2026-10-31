# Handover — the diverged-chain programme, closed 2026-10-09

**Read §0 before touching anything. Read §2 before choosing what to do.**

---

## 0. How to work here

**Write code. Do not run hours of testing to test code that is not finished. Integration tests are for
finished code.**

Per unit: make the change; produce the **falsifier** — something demonstrably **red before** the change and
green after. A passing test is not a falsifier. Three legitimate forms: a named test that fails on the
pre-fix body; a **compile failure** (the bad state is unrepresentable); a one-shot committed run under
`spec/audit/evidence/`. If a unit cannot produce a red-before artifact, **say so** — it is a refactor, and
name it as one.

Then: `cargo fmt --all`, clippy with CI's allow-list (`CLIPPY_DEBT` in `.github/workflows/ci.yml:52-90` —
without it a local run exits 0 on a tree CI calls red), the crate's own targeted tests, and
`make check-register` at register boundaries. **CI is the sweep.** A devnet is for an issue's *close
condition*, once, at the end — not a per-unit habit, and never sleep-polled.

**This session's own evidence for that rule**: a fix was written, verified, and then *not landed* because
the falsifier turned out to be green on the unfixed tree (§2). That is the rule working.

---

## 1. What landed

| what | where | establishes |
|---|---|---|
| the TE-1 witness | #298, on `dev` | the incident as an artefact; C250's two defects (rotated letters, a #283→#287 citation) corrected |
| C215 reproduced | #299, on `dev` | `fringe_states` is keyed by the fringe **set** and carries per-block values — a map whose value is not a function of its key decides the merge |
| the divergence, stageable | #299 | a devnet-only injection produces four heads on demand; the live diagnostic names the map (`fringe_state` = `prev_fringe_lookup`) |
| the recovery tool | #301, on `dev` | plan phase verified against a real divergence: stake-weighted meet, equivocation by proof, a written report |
| **the stopgap** | #301 | `--restore-from-master` converges a frozen chain to **one head, agreeing hashes at every height, no genesis, finality resumed** — the acceptance's four clauses, demonstrated (`spec/audit/evidence/n-reconcile-drill/restore-disarmed.txt`) |
| a CI flake | #300, on `dev` | `cwait_returns_on_a_notification` was a lost-wakeup race costing 45-minute jobs |
| **C215's fix** | #304 (**open PR**) | `insert` joins a record at a fringe key instead of overwriting it, so the map is a function of its key — with the full-path falsifier the first attempt lacked (pass §85) |
| `--sync-anchor` | #303 (**open PR**) | a node restores to a named block's state **and its ancestry travels**, so the catch-up has something to validate against (pass §87) |

Register: **296 findings, 6 todo, 1 in progress** — C249, C250, C254, C255, C256, C260; C259 in progress.

**Correction, 2026-10-14, after the wargame close-out.** The register now reads **365 findings — 0 `todo`,
364 `done`, 1 `in progress` (C301)**; every row the line above names is `done`. The wargame's Table A has
no `owed:` cells left either: all 97 rows carry a taken falsifier, a gate citation or a recorded
decision (pass record §128–§136, register C313–C330), and `tools/check-wargame.sh` — wired into CI —
refuses a page that says otherwise.

**Correction, 2026-10-26, after #457's close-out.** The register reads **374 findings — 1 `todo`
(C339, Law 85's fringe-state residual), 372 `done`, 1 `in progress` (C301)**. The 2026-10-14 line
above is the record as it stood then, not a description of the tree.

**Correction, 2026-10-28, after the #477 close-out.** The register reads **375 findings — 0 `todo`,
373 `done`, 2 `in progress` (C301, C339)**. C340 is the admission row (Law 86) and pass 142 is its
account. The admission and relay arms are recorded in
`spec/audit/evidence/n477-live-arm-20261028/` and `n477-relay-arm-20261028/`; the public testnet's
shared-host cgroup topology remains owed and is named there.

**Correction, 2026-10-09, after this handover was written.** §2's first entry below — "C215's fix is
unproven, and the proof attempt is the evidence" — was true when written and is not any more. The
full-path test was made to bite (the shortfall was `incompatible_with_final`'s asymmetry, and the
construction needed a conflict-scope chain that the final-scope chain *produces* for), and the fix
landed in #304 with that test as its falsifier. Read §2's C215 section as the record of why the first
attempt did not count, not as a description of the tree.

---

## 2. What remains, in the order to take it

### C215 — the fix is unproven, and the proof attempt is the evidence

The reproduction exists at two levels: a **storage-level counterexample** (two arrival orders leave two
different `FringeState` records at one key — `casper/tests/merge_determinism.rs`) and a unit test of the
read that consumes it (`merging::tests::two_caches_of_one_block_set_reject_differently`). What does **not**
exist is a demonstration that the difference reaches the merge's **outcome**: the full-path test
(`the_same_validated_blocks_merge_identically_in_both_arrival_orders`) **passes on the unfixed tree** — two
orders, one outcome, nothing rejected.

**So the fix has no falsifier and has not been written. Do not write it on an argument.** The claim that the
merge's outcome depends on arrival order is **withdrawn** until a construction exists where the final-scope
rejection fires. The shortfall is between `rejections_for`'s read and `rejected_finally`, not in the setup:
the test deliberately puts the colliding key in the merge's *final* scope, which is where `rejections_for`
reads. Start by finding out why the rejection does not fire — whether the final scope's chains reach that
loop at all, and what `first_id` resolves to for them.

### C259(a) — the anchor's catch-up, two halves

`--sync-anchor` restores a node to an agreed block's state and the node then **stops there**. The mechanism,
from reading after the drill:

1. **The seed carries no fringe ancestry** (`node_launch.rs`: `ancestry: Vec::new()`), which **bypasses
   exactly the #139 fix** that makes a restored block replayable. The fix is to carry the anchor's ancestry
   instead of fabricating an empty one — reuse `collect_fringe_ancestry` (`node_running.rs:333`), and add
   the bounded `anchor` field to `FinalizedFringeRequest` (the #139 compatibility pattern).
2. **The sidecars are not synced.** `mergeable-channel-cache` and `native-changes-cache`
   (`casper/src/storage.rs:71,77`) are separate stores, not part of the LFS state transfer, so every
   restored block needs regeneration — which only succeeds once (1) is in. Then either extend the sync or
   bound a one-time replay of the restored range.

Observed: near the tip the node **stalls silently** (the single-parent arm reads the parent's post-state and
never calls `block_index`, so one hop works while the multi-parent suffix never validates); far back it
fails loudly (`regenerated mergeable channels` on the suffix's epoch-boundary blocks — **C188**). Filed with
C188's thread (#139).

### C260 — the merge's report cannot show a finalised chain it dropped

`MergeReport`'s chain counts are **conflict-scope only** (`merging.rs:1934`), so a merge that drops a
*finalised* chain reports `rejected_chains: 0`. `rejected_deploys` carries those chains' ids — the only
reason the C215 negative result was readable at all. Small: one field (or two counts) and the doc sentence
that currently defines the count as conflict-only.

### The rest

**C249**'s reset half (the runtime reset and the wiring that calls `drop_above`; the detect/log/count half
and the primitive are in, shipping dark) · **C250** (needs the four nodes' logs read together) ·
**C253/C254/C255/C256** (see their rows).

---

## 3. Mechanics that will bite

- **The register is authored in two places**: rows in `spec/findings.tsv`, the pass account in
  `spec/audit/passes.md` as `## N.`, rendered by `tools/emit-findings-register.sh` (in place — it refuses a
  file with no marker). `make check-register` runs the lot.
- **The emitter's evidence check reads `HEAD`** and **splits the cell on `/`** — every component ≥6
  characters must exist as an identifier in the committed tree. A test name on *another branch* will not
  resolve; a directory name like `n-reconcile-drill` does not either. Cite file names and symbols.
- **Commit code and evidence before the register row**, or `--check` refuses with *evidence naming nothing
  in the tree*.
- **`tools/check-staged-rows.sh <C-row|->`** goes on the left of `&&`; a commit adding no *new* C-number
  declares `-`.
- **`gh` defaults to the parent repo** (`rchain/rchain`). Always `--repo rhovision-org/rholo`.
- **Force-pushing a rebased branch and merging a PR are both blocked by the local auto-mode** ("Merge
  Without Review"). Rebase and hand the push/merge to the user.
- **A `local` in bash expands with the caller's variables**: `local n="$1" unit="${UNIT[$n]}"` resolves the
  subscript with the *caller's* `n`. Two statements.
- **Per-node devnet flags need `env`**: `VAR_$i=x cmd` is not an assignment (the name is not expanded).

---

## 4. Traps hit today

1. **The staging instrument reaches block content** — a proposer's `rejected_deploys` is computed by its own
   merge under the perturbed cache and written into the block, so an injected node's blocks are
   un-validatable by a clean node. A recovery drill needs the **survivor clean**.
2. **The instrument cannot stage a *post-finality* divergence** — its synthetic rejected-deploy id never
   matches a real chain, so once the records agree the collision is inert. That is why the anchor's
   frozen-finality drill has no reproduction.
3. **A `docker start` reuses the container's command**, so an injected joiners stays injected across a
   restart. `devnet.sh reset` recreates (and empties) — which is also how to un-inject while preserving the
   store the tool copies in.
4. **An idle chain cannot finalise.** On `--no-autopropose`, a restore that is followed by no deploy reads
   as a failure. The tool triggers a block for exactly this reason; a manual run must too.
5. **`git checkout -- <file>` restores from the index**: it silently discarded an uncommitted edit of mine
   mid-session. Use `git restore --source=HEAD` deliberately.

---

## 5. The one thing to carry forward

Three times in this programme, **the code read one way and the run said another**: the audit found shapes
while a live net found the blocker (#293); a drill found that the recovery's trigger state closes it; and a
fix was written and withheld because its falsifier was green before it. A green suite is not evidence about
a running node, and it is not evidence about an improvement.
