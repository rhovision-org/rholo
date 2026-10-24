# Failure-mode HAZOP and disposition

**Reviewed revision.** `dev` at the merge of #281 (`5fc591e33`) — the tree every `file:line` below was
**re-derived** against. The pass began on that branch (`410af4cbf`, the tip of
`fix/n280-merge-loses-a-write`), and the citations were re-derived a second time after the branch was
squash-merged, because the fix for CI's hang moved `casper/src/merging.rs` by ~28 lines and this
repository's rule is that a citation is re-derived, never remapped by a delta
(`spec/audit/evidence/ocapn-hazop.md` §6, hypothesis **R4**). That rule paid for itself here: the drift
was 26 lines at one end of the file and 28 at the other, so a delta would have been wrong in both
directions.

**Why this study exists.** A merge lost a committed deploy's write at an epoch boundary, **unanimously,
with no counter and no log line anywhere moving** — four nodes agreeing on a state that three rounds
earlier had contained the deploy (#280 / C248). On 2026-10-08 the same class produced **four divergent
heads** on the public testnet with finality frozen at 101 for more than thirteen hours, and *nothing in
this tree recovered it*. Both were found by an operator reading a live chain, not by a test.

The property every row below is dispositioned against is the requirement, stated as the target:

> These failure modes should ideally be **unrepresentable**. Where they are not, they must **fail loudly
> and hard**, and then **hard-reset to a safe state and catch up** — never wedge silently.

**Method.** The worksheet format is this repository's own (`testnet-acceptance.md` §1): per study node,
a design intent, the eleven IEC 61882 guide words each dispositioned, and a row per credible deviation.
The study nodes **U1–U10 are reused verbatim**, so a hazard appears once and is dispositioned here.

**Extended 2026-10-13 — the durability nodes U11–U16.** This pass studied the node's *functions*. A later
one studies its **durability** — every place it discards or rewinds durable state — which is a different
grain and needed its own nodes. U11–U16 are this pass's own, derived against `dev` at the merge of #340
(`9b88b0825`); the citations above are unchanged and remain against `5fc591e33`. Each new node carries a
**Table C — law coverage** naming the law a deviation instantiates, or the law it owes. That column is the
**Lean4-first rule**: a deviation an existing law does not already capture is a deviation whose law is
written — as a theorem, in `spec/Rchain/`, with a `witness` and a bumped `lawCeiling` — **before** any Rust
changes. A `candidate:` slug would be the register's marker that the property has *no theorem*, which this
pass does not accept where the state is representable.

**Companion, not a replacement.** The hazards and their root-cause analysis live in
`testnet-acceptance.md` §1 as rows `H-U*-*`; this page does not restate them. What it adds is the
**disposition** — which rung of the ladder below the deviation lands on, what the node does today, and
what would prove it fixed. Its rows are prefixed `F-U*` so the two worksheets compose, and each names its
`H-` sibling where one exists. A guide word this pass has not reached is declared **`owed`** rather than
left blank, and the debt is counted in §7 where a check reads it.

## 1. The disposition ladder

- **R1 — unrepresentable.** The bad state cannot be constructed: a refinement type, or an operation
  deleted rather than narrowed (`spec/TYPE-SYSTEM.md` §1.7; `tools/audit-type-system.sh`). The worked
  precedent is **C248**, which did not narrow `reject_whole_blocks` but *deleted* it, so "reject every
  chain of a block because of a write the block made" has no operation left to express it, and moved the
  relation onto the chain (`DeployChainIndex::native_effects`, `casper/src/merging.rs`).
- **R2 — fail loudly.** The failure is representable but must not be silent: a report struct + counters +
  a rate-limited log that re-fires on change + a named test. The `MergeReport` (invariants I1/I2,
  `casper/src/merging.rs:274`) and `NoAdvance<S>` + `describe_no_advance`
  (`block-storage/src/dag/finalizer.rs:61`, `casper/src/interpreter_util.rs:428`) are the shape.
- **R3 — hard reset, then catch up.** Detect a node that cannot self-heal, say so, then reset to a
  known-safe state and re-sync. **The rung landed 2026-10-10 for the per-node case** (#320, register
  C274): behind `casper.self-reset`, a spent restore budget rewinds to the last finalised block, drops
  the bodies, resets the runtime and restarts the catch-up walk. The net-wide case is the reconciliation
  utility (§5). What is still owed on the rung is named in F-U5-01's row and C274.
- **R4 — runbook.** Nothing better is affordable yet; the operator step is documented and made
  observable.

**What this ladder found.** Almost every path below already **fails loudly** — the merge refusal is a
`log.error` (`casper/src/blocks/block_processor.rs:172`), an I1/I2 violation a non-rate-limited `log.warn`
(`casper/src/interpreter_util.rs:535`), a finality stall a `log.warn` that re-fires on change
(`casper/src/interpreter_util.rs:519`), an ingress drop a `log.warn`
(`casper/src/engine/node_running.rs:664`). **Two paths were genuinely silent** (F-U8-01 and F-U9-03; both now have their source half — the one
reports its error, the other counts the recovery). So the
gap is not mostly loudness: it is that the loud line has **no action behind it** — nothing re-queues the
dropped block, nothing restores the refused one, nothing resets the wedged node. R2 is largely won;
**R3 is entirely unbuilt**, and §5 names where it is owed.

## 2. Top events

`testnet-acceptance.md` §2 fixes four. Two are added here because the 2026-10-08 witness is a shape those
four do not separate, and the distinction is what decides the disposition.

| id | top event | source |
|---|---|---|
| **TE-1** | The chain halts and cannot recover | `testnet-acceptance.md` §2. **Live-witnessed 2026-10-08**: four divergent heads, finality frozen at 101 — `spec/audit/evidence/te-1-2026-10-09-four-divergent-heads.md`, **in this tree** since PR #298 (this cell read *"on branch `spec/te-1-witness` (not in this tree)"* for as long as that was true) |
| **TE-2** | A fragment of the net reaches finality that the rest rejects | `testnet-acceptance.md` §2 |
| **TE-3** | An outsider validator cannot safely join or leave | `testnet-acceptance.md` §2 |
| **TE-4** | State, disk or memory exhaustion halts the net | `testnet-acceptance.md` §2 |
| **TE-5** | **Node wedge** — a single node degrades and can neither progress nor recover itself, while the net runs on | new: the witness's restart experiments (a restart restores *production* and not *finality*), `AUDIT` C181 |
| **TE-6** | **Rewind-forcing divergence** — a state difference no local restore heals, so *the chain* must be rewound rather than a node | new: #280/#287 |

TE-5 and TE-6 are the two that R3 answers, and they are separated because TE-5 is per-node (a reset
heals it) while TE-6 is net-wide (only a reconciliation heals it — §5).

## 3. The worksheet

### U1 — Block production / proposal

**Design intent.** When and only when a trigger fires, the node builds at most one block per round per
validator, whose number and sequence are determined by its justification set, and offers it.

**Guide words.** row: PART OF · folded: LATE → NO, BEFORE → EARLY, AFTER → MORE · vacuous: REVERSE · owed: NO, MORE, LESS, AS WELL AS, OTHER THAN, EARLY

The REVERSE vacuity is the acceptance sheet's (`testnet-acceptance.md` §1 U1: the node has one directed
action, and `block_num`/`seq_num` are refined non-negative types whose validation refuses any parent at
or above the block's own number). The folds are its folds.

### Table A — deviations

| ID | Guide word | Deviation | RCA → causal chain | Current behaviour | Class | Disposition | Falsifier |
|---|---|---|---|---|---|---|---|
| **F-U1-01** | PART OF | The block is built, self-validated and inserted, but the **broadcast leg is fire-and-forget** — a proposal that reaches no peer while the node logs it as proposed (H-U1-05) | the callee returns no status and logs success unconditionally (`casper/src/protocol/comm_util.rs:182`) → the send error is swallowed one frame down in `send_to_peers` (`casper/src/protocol/comm_util.rs:83`), which drops `TransportLayer::broadcast`'s `Vec<CommErr<()>>` (`comm/src/transport/transport_layer.rs:18`) → the round cannot close on a block no peer holds | loud but false: the only line is a **success** log; nothing reports the drop | Drift | R2 | owed: a test that a failed broadcast is surfaced (no re-queue exists to ticket) |

### U2 — Attestation, round closure, quorum

**Design intent.** A validator answers a remote block at most once per sender per height; a round closes
only when every live bonded sender has a message above the boundary; the quorum that closes it is a
strict supermajority of the whole bonded map.

**Guide words.** row: LATE · folded: AS WELL AS → MORE, BEFORE → LESS, EARLY → LESS, NO → LATE, AFTER → LATE, PART OF → OTHER THAN · vacuous: REVERSE · owed: MORE, LESS, OTHER THAN

The folds and the REVERSE vacuity are the acceptance sheet's (§1 U2).

### Table A — deviations

| ID | Guide word | Deviation | RCA → causal chain | Current behaviour | Class | Disposition | Falsifier |
|---|---|---|---|---|---|---|---|
| **F-U2-01** | LATE | The **attestation guard is node-local while the finalizer's partition is per-block** — the guard suppresses on *this* node's seen view, the partition is derived from the block's own justifications, and the two can disagree (H-U1-01/03's root cause) | the guard reads `fringe_seen` from `pre_state.prev_fringe` and decides `nothing_to_finalize` (`casper/src/blocks/proposer/proposer.rs:955`, `:1037`) → the finalizer's live set is deterministic per block (`block-storage/src/dag/liveness.rs:153`, whose comment says "every node validating the same block derives the same live set") → a node can suppress where the partition has work, freezing the fringe while production continues | loud: the stall reason is logged (`casper/src/interpreter_util.rs:519`), rate-limited and re-firing on change | **Drift, not Split** | R2 | owed: the suppression must be counted on a surface (see F-U10-01) |

**This row corrects the seed catalogue.** The programme's plan called this "a `Split` risk". The two
surfaces *can* disagree, but validity is recomputed deterministically from the block's own justifications
by every peer — so the disagreement produces a **liveness stall, never a validity split**. Class is
`Drift`; the fix is not "make both readers ask one side".

### U3 — Finality / fringe / estimator

**Design intent.** The fringe advances only over a full partition whose every member has seen every
other, and a finalised block is never undone.

**Guide words.** row: NO · folded: MORE → AS WELL AS, PART OF → AS WELL AS, OTHER THAN → AS WELL AS, AFTER → LATE, BEFORE → EARLY · owed: LESS, AS WELL AS, LATE, REVERSE, EARLY

No word is vacuous at this node (the acceptance sheet's §1 U3 contests its own EARLY/BEFORE vacuity in
CH-U3-03, which this pass does not reopen). The folds are its folds.

### Table A — deviations

| ID | Guide word | Deviation | RCA → causal chain | Current behaviour | Class | Disposition | Falsifier |
|---|---|---|---|---|---|---|---|
| **F-U3-01** | NO | **While finality is stalled there is no fringe to sync from**, so the *only* path that restores a node is closed for exactly as long as the chain is broken — a restoring or joining node answers `"Finalized fringe is not available."` | the restore/sync path reads the **finalised** fringe → finality frozen at 101 means no new fringe is minted (`spec/audit/evidence/te-1-2026-10-09-four-divergent-heads.md`, "The dependency this exposes") → the node's recovery and the chain's recovery are the same dependency, so a stalled chain cannot be repaired by restarting nodes into it | loud, and already stated in the specification (`testnet-acceptance.md`:1147, "Recovery from the divergent finality itself: none") | Terminal | R3 — **the tool is delivered** (#287 closed) | owed: the **live-testnet drill**, which is the venue #287 recorded as owed and blocked — this box holds no root key, and the live build predates the routes the meet needs |

### U4 — Merge & DAG search

**Design intent.** The merge of a branch set is deterministic, order-independent, and declines safely
when its budget is exhausted.

**Guide words.** row: MORE, AS WELL AS, OTHER THAN, PART OF · folded: NO → MORE, BEFORE → LATE, AFTER → LATE · vacuous: REVERSE, EARLY · owed: LESS, LATE

The vacuity reasons are the acceptance sheet's (§1 U4: a `min` over a materialised `BTreeSet` has no
order to reverse; the search is a synchronous pure function metered in work units, not time).

### Table A — deviations

| ID | Guide word | Deviation | RCA → causal chain | Current behaviour | Class | Disposition | Falsifier |
|---|---|---|---|---|---|---|---|
| **F-U4-01** | MORE | A **search-budget refusal, a native-relation refusal and a number-channel overflow all become one drop with no re-queue** — the node cannot advance at that height until the scope shrinks (H-U4-01) | the three refusals are `Err(String)` inside `MergeScope::merge` (`casper/src/merging.rs:1876`, `:1641`, `:2014`) → `validate_block_checkpoint` carries them out (`casper/src/interpreter_util.rs:485`) → `validate_block` maps them to `ValidateError::Internal` (`casper/src/multi_parent_casper.rs:850`) → `block_processor::apply` logs and `continue`s (`casper/src/blocks/block_processor.rs:168`), and the comment there states that no deferral loop is intended for this class | **loud** (`log.error`, `block_processor.rs:172`) and **actionless**: nothing retries, nothing re-queues | Terminal | R2 + R3 | owed: a test that a refused merge is re-queued rather than dropped (the search *budget* test exists — `sdk/src/dag/merging.rs:a_budget_refuses_without_answering_and_never_changes_the_answer` — but it witnesses the refusal, not the recovery) |
| **F-U4-02** | AS WELL AS | **The merge's two invariants are computed, reported and not enforced** — a merge that violates I1 or I2 still returns a state hash | I1 (every rejected chain conflicts with a kept one) and I2 (every kept chain's write is in the batch) are computed into `MergeReport` (`casper/src/merging.rs:1957`, `:1974`) and `MergeOutcome` is returned unconditionally right after (`casper/src/merging.rs:1981`) → the only consumer is `describe()`/`is_quiet()` (`casper/src/merging.rs:299`, `:322`) → the caller logs it when non-quiet (`casper/src/interpreter_util.rs:534`) | loud (`log.warn`, not rate-limited) but **informational**: the violating state is used. **Counted now** — a non-quiet report increments `non_quiet_merge_reports` (`casper/src/interpreter_util.rs:595`), so the event is a number on the operator status and not only a line (`/api/v1/status/operator` on the admin listener since 2026-10-13 — W-N3-01 moved the divergence counters off the public one) | Void | R2 | node/src/api/conversion.rs::to_api_status_maps_fields |
| **F-U4-03** | OTHER THAN | **Historical / closed — the R1 precedent.** `reject_whole_blocks` propagated one rejected chain to its *whole block*, so a rejection took chains that wrote no contended slot; the fix **deleted** the operation rather than narrowing it (C248) | the native relation was keyed on the host block (`NativeRelations::conflicting`), so at an epoch boundary every concurrent pair conflicted → the rejection closed over the block → chains died with their host | (closed) the operation no longer exists; the relation is on the chain (`DeployChainIndex::native_effects`) | Terminal | **R1** (worked precedent) | casper/src/merging.rs::a_rejected_boundary_chain_does_not_take_its_blocks_other_chains |
| **F-U4-04** | PART OF | **Historical / closed — the second R1 precedent, and it came from #281's review, not from this pass.** A block's native effects were drained **one action per slot, credited to that slot's last writer**, so when two deploys of one block wrote the same slot the earlier deploy's write was a value no chain could reconstruct | `NativeStoreAction`'s drain kept one action per key (the trie refuses two), so the sidecar's per-chain effects held the **last** writer only → had resolution rejected that later chain and kept the earlier, the merged state could not have rebuilt the earlier's value: rejecting one chain would have silently dropped a *different* deploy's write, one layer below the defect this fix was for (and I2 could not see it, the earlier chain's slot set already having been emptied) | (closed) `Slot` carries per-deploy write **history** now, and the drain produces two views — `by_deploy`, each deploy's own last write per slot, and `all`, the checkpoint's one action per slot | Terminal | **R1** (worked precedent) | rspace/src/native_store.rs::two_deploys_writing_one_slot_keep_their_own_writes |
| **F-U4-05** | MORE | **A type whose order disagreed with its equality made two distinct chains one key** — `DeployChainIndex` ordered by `(host_block, post_state_hash)` and compared by `deploys_with_cost`, which Rust forbids because `BTreeMap`/`BTreeSet`/`sort` read the order as the identity | the port carried the Scala split deliberately and **pinned it as a known hazard** ("visible rather than latent"); the chain-level native relation is what made it fire on an ordinary block, since two chains of one block share both host and post-state → a set of chains silently held one of them, and the dependency map it built held a key whose own value looked that key up again | realized **twice**: as a hang (how it was found — CI's job limit, not a failing assertion) and, with the loop fixed alone, as a merge that keeps one of two distinct chains — a **silent** lost chain | Terminal | **R1** (one key for `Eq`/`Ord`/`Hash`) | casper/src/merging.rs::a_chains_order_agrees_with_its_equality |

### U5 — LFS sync & join / bootstrap

**Design intent.** A node with no state restores a consistent prefix (the finalized fringe plus
tuple-space) from a peer, then replays its own blocks to the same state hashes the rest of the network
holds.

**Guide words.** row: NO, REVERSE · folded: AS WELL AS → MORE, OTHER THAN → REVERSE, BEFORE → EARLY, AFTER → LATE · owed: PART OF, LATE, EARLY, MORE, LESS

No word is vacuous at this node — the acceptance sheet's §1 U5 has an empty vacuous set and was flagged
for a second pass for exactly that reason. The folds are its folds. **This node carries the R3 gap.**

### Table A — deviations

| ID | Guide word | Deviation | RCA → causal chain | Current behaviour | Class | Disposition | Falsifier |
|---|---|---|---|---|---|---|---|
| **F-U5-01** | NO | **There is no reset-and-resync path for a running node.** `NodeSyncing` is entered only at boot, when the DAG is empty; once running, a permanent divergence has no way back but deleting the shard data dir by hand | the boot guard is an `if` chain — `repr.dag_set.is_empty() && standalone` → genesis, `else if repr.dag_set.is_empty()` → `NodeSyncing::new` (`casper/src/engine/node_launch.rs:249`, `:265`), `else` → straight to `NodeRunning` (`:316`) — and `NodeSyncing::new` is called **nowhere else**; a running node that diverges stays in `NodeRunning` and permanently refuses the block and its descendants (`casper/src/multi_parent_casper.rs:883`, neglected-invalid rule), while `populate_dag` is reachable only from the sync task the boot branch starts (`casper/src/engine/node_syncing.rs:515`) | **The failure it is about has since been designed out — see §8.** The producer was retired, not guarded (the fringe-record `min` tie-break, law 66/67/68); the merge became a pure function of the parent set, so a *view* difference no longer diverges at all; and three instruments built to re-create the state could not (§110, §113) — the last producing the divergence and the spent budget 96 times and then finding no anchor to rewind to. So the arm's being unrun is the hardening rather than a gap. **And the node's own behaviour where the state is reachable:** loud (`log.warn` per refused block) and **permanent** — unless the node resets itself. **The acting half landed 2026-10-11 (#325, register C274, law 53c)**: with `casper.self-reset` on, a spent restore budget raises a reset — rewind to the last finalised block, remove the dropped bodies, reset the runtime to that state, restart the catch-up walk from the anchor (row 168's numbers are the dry run's, now also the action's). It refuses where it cannot name a height, and stays off by default. ****Not owed — unstatable, and that is the honest treatment.** The deviation this row names (*there is no reset-and-resync path*) is **closed**: the path landed, and its falsifier is the crash-repair test above. What is *not* closed — and is **not owed either** — is the *arm*: the claim *a diverged running node re-enters sync* has no **statable failure mode on this build**. The producers it had are gone (the fringe-record tie-break retired with law 66/67/68; the merge is now a pure function of the parent set), and three instruments built to re-create one could not (§110, §113 — the last producing the divergence and the spent budget 96 times and then finding no anchor to rewind to). **A failure mode nobody can name is a nebulous risk, not a register item**: this repository's own rule is that a statement nothing can falsify is not a statement (the register's `vacuous` status), and an item that cannot be reproduced, named or falsified is that shape. It is therefore recorded as a risk and kept out of the debt. What *is* verified is the backstop's own pieces — the durable half, the crash marker, the boot repair, the fatal stop and the ordering (laws 72–76, `casper/tests/reset.rs`); the `apply`-level wiring remains read rather than run. | Terminal / Unrestorable | **R3** (owner: **#328**) | `casper/tests/reset.rs::a_crash_after_the_rewind_is_repaired_at_boot_by_the_marker` |
| **F-U5-02** | REVERSE | **The divergence has no inverse.** The refusal rules are absorbing: the only local undo is capped at three attempts and then the refusal is permanent for the process lifetime | `restore_is_warranted` requires `stored.restore_attempts < RESTORE_ATTEMPT_LIMIT` (`casper/src/multi_parent_casper.rs:478`, the constant `= 3` at `:434`), the count is persisted so it survives restart (`:430`), and `restore_divergent_justifications` (`:652`) is the only rule that could clear the refusal — after three it never fires again | loud, bounded, and then **absorbing** (C173's shape: `Persistent`) | Terminal / Persistent | **R3** | spec/Rchain/Casper/Stranding.lean::the_refusal_is_persistent — *the guard on the fix*: the theorem is stated over the rule set **without** a restoring rule, so adding one (which is what R3 is) reddens it by construction |

**F-U5-02 is the programme's sharpest instrument.** The Lean module is named `Stranding.lean`, and its
own comment records that `the_refusal_is_persistent` is *false by construction* once a step unmarks in
the relation. So the R3 fix has a falsifier that is red on the old behaviour **before it is written** —
which is the plan's verification requirement, already satisfied.

**Corrected 2026-10-10 (#320).** The sentence above was written as though R3's *first* fix would do the
reddening. It did not, and it cannot: the reddening came from **#125's restoring rule**
(`restore_divergent_justifications`, modelled as `restoreStep`, the exact inverse of `strandStep`), and
`the_refusal_is_not_persistent_once_a_rule_restores` is that falsifier *already fired*. **#320's step is
not an unmark** — a reset does not clear a refusal, it discards the state the refusal was about and
re-enters sync below it — so on the `Strand` model (`failed : List Nat`) it is a **prefix** step, a
coarsening, not the inverse. Reddening `the_refusal_is_persistent` by adding a rewind would therefore be
a false falsifier: it would assert a rule the port does not have for that hash. What the model owes for
#320 is a step **and** a theorem of its own (`rewindStep`, `the_refusal_is_not_persistent_once_a_rule_
rewinds`), **guarded by the anchor precondition** — because an unguarded `rewindStep` would certify a
relief the port deliberately withholds in one of its two arms. **Delivered 2026-10-11**: `rewindStep`,
`strandSystemRewinding`, `the_refusal_is_not_persistent_once_a_rule_rewinds` and the guard
`a_rewind_is_not_available_without_an_anchor` are in `Stranding.lean` as law 53 clause c, and register row
**C274** records it.

### U6 — Bonding, epoch boundary, PoS

**Design intent.** A bond or withdrawal takes effect exactly once, at the next epoch boundary, on every
node identically.

**Guide words.** row: EARLY · folded: LESS → NO, PART OF → NO, BEFORE → EARLY, AFTER → LATE · vacuous: REVERSE · owed: NO, MORE, AS WELL AS, LATE, OTHER THAN

The REVERSE vacuity is the acceptance sheet's (§1 U6: no inverse transition exists — a bond cannot debit
the pool, and finality/state undo is a stated deferral).

### Table A — deviations

| ID | Guide word | Deviation | RCA → causal chain | Current behaviour | Class | Disposition | Falsifier |
|---|---|---|---|---|---|---|---|
| **F-U6-01** | EARLY | At an epoch boundary the concurrent boundary chains each compute a pot, and **which sibling's effect survives still depends on which chains are in scope** — so a boundary that carried a deploy computes a different pot from its empty siblings | C248 moved the native relation onto the chain and deleted `reject_whole_blocks`, which stopped the *whole-block* loss; the boundary chains *themselves* still contend, so the merged pot is scope-dependent (C248's residue clause 1, stated there as "Aria's criterion 2 is NOT met and must not be read as met") | loud only if a violation lands in `MergeReport`; the scope-dependence itself is **unreported**. **The arrival-order half is measured, and it is not a fork** (§102, 2026-10-11): with one block set in two orders the round snapshot really differs (`{a2,b1}` vs `{a2,b2}`) and `MergeScope::from_dag` really gives two scopes for it — but a block carries its own justifications and every node derives its pre-state from those, so the two nodes derive the *same* scope for each block. What differs is what a node would **propose**, not what it computes. What is left here is the boundary-chain contention itself | Split | R1 | `casper/tests/merge_determinism.rs::an_arrival_order_moves_the_round_snapshot_and_not_the_scope_a_block_derives` — and what it does **not** reach is named in §102: a boundary chain that carried a deploy against its empty siblings, which needs an epoch-running fixture |

### U7 — Deploy pool, gas & admission

**Design intent.** A deploy is admitted once, held while valid, included exactly once or expired; the
cost of admitting is bounded per unit time.

**Guide words.** row: LATE · folded: NO → MORE, AS WELL AS → LESS, AFTER → LATE · vacuous: OTHER THAN · owed: MORE, LESS, PART OF, REVERSE, EARLY, BEFORE

The OTHER THAN vacuity is the acceptance sheet's (§1 U7: the signature verifier refuses any non-canonical
spelling, so a deploy cannot be admitted under a sig the pool keys on *other than* the one `repeat_deploy`
dedups on).

### Table A — deviations

| ID | Guide word | Deviation | RCA → causal chain | Current behaviour | Class | Disposition | Falsifier |
|---|---|---|---|---|---|---|---|
| **F-U7-01** | LATE | **A deploy expiring at the boundary can make the proposer reject its own block and stall permanently** (#284, found by the #144 campaign's void arm) | the frozen campaign at `74cda38f1` processed 95–99 of 200 deploys and exposed a deploy-expiry boundary wedge; the proposer's own self-validation then refuses the block it just built, which is the shape the 2026-10-08 witness records as `the block's rejected-deploy set does not match its parent` | loud (the self-validation failure is logged) and **sticky** — the stall persists | Terminal | R2 + R3 | owed: #284's reproduction (the campaign arm that exposed it is preserved as VOID) |

**A hypothesis, stated as one.** #284's mechanism and the `#106` self-validation failure in the
2026-10-08 witness share a shape — a block whose rejected-deploy set disagrees with its parent — and #280
remains open on that failure. This pass does **not** claim they are one defect; the falsifier that would
settle it is #280's log at `#100–#106`, which the witness partly supplies (§6).

### U8 — Wire protocol, transport, discovery

**Design intent.** A message from a bounded-adversary peer is authenticated, bounded in size and rate, and
cannot make the node do unbounded work.

**Guide words.** row: PART OF, MORE, NO · folded: LESS → MORE, OTHER THAN → PART OF · owed: REVERSE, AS WELL AS, BEFORE, AFTER, EARLY, LATE

No word is vacuous at this node — the acceptance sheet's §1 U8 contests its own REVERSE vacuity (CH-U8-02)
and records a candidate deviation instead; this pass does not reopen it. The folds are its folds.

### Table A — deviations

| ID | Guide word | Deviation | RCA → causal chain | Current behaviour | Class | Disposition | Falsifier |
|---|---|---|---|---|---|---|---|
| **F-U8-01** | PART OF | **`send_to_validate` swallowed a store-read error** — an unreadable block in the batch was dropped with no log, no re-queue and no counter. **The class was wider than this row**: the rule the fix motivated found **five more sites** of "an awaited read whose error is erased into an absence", all fixed, and it now gates (C251) | `block_store.get(&[*hash]).await.ok()` discarded the `Err`; only `Some(block)` reached `put_to_incoming_queue`, so the hash was consumed by the batch and never re-requested. **Fixed**: the read propagates and the caller logs it — the third site of **C67's class**, beside `parents_not_stored` and `not_validated`. The other five (`block_receiver`'s `contains`, the LFS pump, the genesis read a joiner uses, and two in `multi_parent_casper` including the restore rule) are fixed the same way: **log the error, keep the branch** | **was silent** — no `log` call at all on the error arm. **Now loud**, and the *shape* is gated: `tools/audit-type-system.sh`'s `silent` class carries `\.await…\.ok\(\)` and `\.await…\.unwrap_or_default\(\)` as **hard** violations with no allow-list, so a new instance fails the build rather than being found by the next audit | Terminal | R2 | casper/src/blocks/block_receiver.rs::a_store_that_cannot_be_read_is_not_a_parent_with_nothing_to_re_send/tools/audit-type-system.sh |
| **F-U8-02** | MORE | **A full ingress queue drops the block with a warning** — `try_send(...).is_err()` covers "full" and "receiver dropped" alike, and the block is gone | `casper/src/engine/node_running.rs:664` (BlockMessage arm; the same shape at `:746` for HasBlock) warns and returns; the bounded queue is `MAX_PENDING_BLOCKS` (`node/src/runtime/node_runtime.rs:852`) | loud (`log.warn`) and **actionless**: no re-request, no back-pressure on the sender | Terminal | R2 | owed: a test that a dropped ingress block is re-requested or the peer is told to slow |
| **F-U8-03** | NO | **The block retriever forgets a hash at capacity** — `CapacityReached` returns the state unchanged, so the hash is never recorded and never requested | `admit_hash` returns early when `state.len() >= MAX_REQUESTED_BLOCKS` (`casper/src/blocks/block_retriever.rs:116`) → `CapacityReached` is handled by a bare warn (`:227`) and the hash is neither inserted nor broadcast | loud (`log.warn`) and **forgetting**: recoverable only if a peer re-offers the same hash | Terminal | R2 | owed: a test that a capacity-reached hash is retained for a later request |

### U9 — State, storage & DAG residency

**Design intent.** The DAG and its stores retain exactly what the liveness and finality rules may still
need, at a residency an operator can size for.

**Guide words.** row: MORE, PART OF · folded: NO → MORE, AS WELL AS → MORE, REVERSE → MORE, OTHER THAN → MORE, EARLY → PART OF, BEFORE → PART OF, AFTER → LATE · owed: LATE, LESS

The folds are the acceptance sheet's (§1 U9 — absence of a release path *is* the over-retention; the
leaf-before-root commit *is* the partial-atomicity deviation). Its own LESS reason is recorded there as
false and reopened as `owed` here rather than inherited.

**A note on scope.** The acceptance sheet's node list has no *process-resource* node, so this pass rows
the node's retention and resource bounds here as well. The memo caches, the lock's poison policy and the
blocking pool are not stores — but they are the same question the design intent asks ("at a residency an
operator can size for"), and a node that dies of any of them reaches TE-4 identically.

### Table A — deviations

| ID | Guide word | Deviation | RCA → causal chain | Current behaviour | Class | Disposition | Falsifier |
|---|---|---|---|---|---|---|---|
| **F-U9-01** | MORE | **The global insert lock is held across all store I/O and both representation locks**, so one slow insert serializes *all* block admission | `insert` binds `let _guard = self.lock.lock().await` (`casper/src/dag.rs:489`) and that guard drops only at the end of the body (`:696`); inside it are sixteen further `.await` points and `representation.read().await` (`:502`, `:554`) and `.write().await` (`:649`), with the author's own comment "this function is serialized by `self.lock`" (`:647`) | not loud and not silent: the node is *slow*, not wedged, until a slow store makes admission stop entirely | Drift → Terminal | R2 | owed: a bound or a timing assertion on admission under a slow store |
| **F-U9-02** | PART OF | **`update_metadata` holds the representation write lock across an LMDB read**, against the discipline `insert` documents — so the lock discipline holds for *part* of the module | `let mut guard = self.representation.write().await` (`casper/src/dag.rs:729`) then `repr.height_map = self.block_metadata_store.height_map().await` (`:736`); `insert` documents the opposite at `:547` ("the guard is a *block expression* rather than a binding … a binding left in scope would hold the read lock across `fringe_data_store.put`") | not loud; a held write lock across an await is a stall if the store stalls | → Terminal | R2 | owed: a test or lint that a representation guard is not held across an await |
| **F-U9-03** | MORE | **A poisoned-lock recovery was invisible.** The recovery itself is **deliberate** — `shared/src/lock.rs`'s module doc and `spec/TYPE-SYSTEM.md` §3.2 record it as the remedy that closed the port's production panic sources, so "refuse the torn state" is **not** the intent and this row does not claim it. The deviation is the **event**: a poison means a panic happened while shared state was held | `rlock`/`wlock`/`mlock` (`shared/src/lock.rs:79`, `:84`, `:89`) each recover via `PoisonError::into_inner`; the stores funnel through these accessors, so the policy is one function rather than a scatter. **Fixed at the source**: a process-wide counter and `poison_recoveries()` (`shared/src/lock.rs:48`) | **was silent**; **now counted and published** — `poison_recoveries` on the operator status (`/api/v1/status/operator`; public until W-N3-01 moved it, 2026-10-13), read from the process-wide counter at the node's status gather | Terminal | R2 | shared/src/lock.rs::a_poisoned_recovery_is_counted |
| **F-U9-04** | MORE | **The memo caches have no count bound** — the radix read/write caches and the hot-store channel memo grow until a checkpoint clears them, in a struct with no eviction path | `RadixTreeImpl`'s `cache_read`/`cache_write` are plain `HashMap`s (`rspace/src/history/radix_tree.rs:152`), the write cache cleared only at the end of `save_and_commit` (`:660`) and the read cache only from `RadixHistory::process` (`rspace/src/history/instances/radix_history.rs:73`); the channel memo is a `HistoryStoreCache` behind a `Mutex` with **no** `clear` method (`rspace/src/hot_store.rs:80`, `:137`), dropped only when the whole `InMemHotStore` is rebuilt at `create_checkpoint`/`reset`/`revert_to_soft_checkpoint` (`rspace/src/rspace.rs:627`, `:649`, `:702`) | not loud: the growth is invisible until memory is exhausted (TE-4) | Drift | R2 | owed: a count bound per cache, with the bound as an observable |
| **F-U9-05** | MORE | **The blocking pool is uncapped** — `spawn_blocking` runs on tokio's 512-thread default, each thread on the runtime's 32 MiB stack | the runtime is built without `max_blocking_threads` (`node/src/main.rs:91`; `thread_stack_size(32 * 1024 * 1024)` at `:92`), the setting appears **nowhere** in the workspace, and the production sites are `node/src/runtime/node_main.rs:45`, `:69` | not loud: a thread explosion reads as a slow node, and then as an OOM (TE-4) | Drift → Terminal | R2 | owed: a `max_blocking_threads` bound, set and tested |

### U10 — Operator surface

**Design intent.** An operator can observe block production, finality and the proposer-halt state, and
restart the node back to the same chain state.

**Guide words.** row: NO, AFTER, OTHER THAN · folded: REVERSE → NO, EARLY → LESS, AS WELL AS → PART OF, BEFORE → LATE · owed: MORE, LESS, PART OF, LATE

No word is vacuous at this node (the acceptance sheet's §1 U10 has none; its BEFORE/AFTER fold is
contested there in CH-U10-03 and this pass does not reopen it). The folds are its folds.

### Table A — deviations

| ID | Guide word | Deviation | RCA → causal chain | Current behaviour | Class | Disposition | Falsifier |
|---|---|---|---|---|---|---|---|
| **F-U10-01** | NO | **The wedge had no machine-readable surface**: the finality-stall *reason* (`NoAdvance`'s three exits) was log-only, so `/api/status` could not say *why* finality had stopped | `describe_no_advance` (`casper/src/interpreter_util.rs:588`) had exactly one call site, the rate-limited warn beside it; the reason is carried on `ParentsMergedState.finality_stall` (`casper/src/merging.rs:419`, assigned `casper/src/multi_parent_casper.rs:347`) and reached no response type — `ApiStatus` (`node/src/api/dto.rs:74`, since 2026-10-13 the operator half `OperatorStatus` at `node/src/api/dto.rs:137` — the public status no longer carries it, W-N3-01) had no field for it. **Fixed**: a second cell written on **every** observation — *not* the log's rate limiter, because a surface inheriting that would report a stale reason for up to `STALL_LOG_INTERVAL` heights, exactly while an operator is looking — exposed as `finality_stall` and `finality_stall_episodes` (`casper/src/interpreter_util.rs:313`, `:333`) | loud (`log.warn`, rate-limited, re-firing on change) and **only** in the log; **now** on the surface a probe can ask, with the numbers that make it a diagnosis. **Refined 2026-10-10 (#318, register C273):** the counter this row exposed counted every single refusal, and on a producing net most blocks are refused — so it read as an alarm while liveness was fine. It now counts a *sustained* run (twice the partition, from the gate's own payload), reports `finalityStalled` for "stalled now", and says in the reason string which partition the layer was partial against | Terminal | R2 | casper/src/interpreter_util.rs::a_stall_is_a_run_of_refusals_and_the_run_scales_with_the_partition/node/src/api/conversion.rs::to_api_status_maps_fields |
| **F-U10-02** | AFTER | **The halt is sticky and not persisted**, so a restart makes the surface forget a halt it just reported | `ProposeHealth` is five `Arc<Atomic*>` fields (`casper/src/api/block_api.rs:101`); `note_timer_halted` is the only write (`:148`, called from `node/src/runtime/node_runtime.rs:1791`) and there is **no `store(false)` anywhere**; the process reads a fresh `false` after a restart. **The row is not closed by the 2026-10-13 work, and the first draft of that work said it was**: `autopropose_timer_halt_episodes` (`casper/src/api/block_api.rs:76`) is an in-memory counter, so it too is `0` after a restart, and the ambiguity this row names — `false` on a node that never halted versus `false` on one that halted and restarted — is untouched. Its fix is a **durable record on the halt path**, the shape the reset marker already has (`ResetMarker`, `block-storage/src/dag/dag_storage.rs`) | loud while the process lives; **forgotten after a restart** — an operator restarting to clear an alarm gets a false all-clear. **Closed 2026-10-14**: the halt is a durable record, **obeyed** — `HaltRecord` (16 fixed bytes, one reserved key beside the reset marker's in the store they share, `block-storage/src/dag/dag_storage.rs`) is written where the timer halts (`node/src/runtime/node_runtime.rs`), read at boot and seeded into the shard's cell, and a shard under it **does not start its timer**; the surface carries `autoproposeHalt { at, durable }` so "unknown" is distinguishable from "never halted"; and the one way out is `POST /api/v1/autopropose/resume` on the admin listener, which clears the record and wakes the parked timer in-process | Historic | R2 | casper/tests/reset.rs::a_halt_written_before_the_restart_is_read_at_the_next_boot/node/src/runtime/node_runtime.rs::a_halted_timer_does_not_tick_until_the_halt_is_cleared |
| **F-U10-03** | OTHER THAN | **`[pos]` diagnostics bypass the log sink** — three sites write to stderr with `eprintln!`, not the `Log` source, so they are outside rate-limiting, log routing and any scrape | `casper/src/blocks/proposer/proposer.rs:232`, `:861`, `:897` all call `eprintln!` where the module has a `log` in scope | loud on the terminal, invisible to every surface that reads the `Log` sink | —  | R2 | owed: route the three `[pos]` sites through the module's `Log` source |

### U11 — The reset request's lifecycle

**Design intent.** A spent restore budget raises exactly one request for a shard; the request reaches the
driver that can act on it; and it is consumed only at the point of commitment, so a failure before any
durable effect leaves it actionable rather than spent.

**Guide words.** row: NO, AS WELL AS, EARLY · folded: MORE → NO, LESS → NO, PART OF → NO, AFTER → NO, OTHER THAN → AS WELL AS, BEFORE → EARLY, LATE → EARLY · vacuous: REVERSE (the lifecycle is one directed sequence — raise, deliver, take, act — so there is no inverse to deviate from)

The consolidation is the node's own. A request that cannot name a height is not raised at all
(`reset_decision`'s two refusing arms), so PART OF and LESS collapse into "no request"; an out-of-order
delivery (BEFORE, LATE, AFTER) is the same failure as no delivery; and MORE is *not* a deviation — a
second request while one is pending changes nothing that matters, because `notify_one` **stores a permit**
when no driver is waiting, so the first request's single wake is not lost. (That the guarantee rests on
the primitive's permit retention rather than on this module is why it is folded here and stated, not
left to be rediscovered.)

### Table A — deviations

| ID | Guide word | Deviation | RCA → causal chain | Current behaviour | Class | Disposition | Falsifier |
|---|---|---|---|---|---|---|---|
| **F-U11-01** | NO | **A request raised before a driver is installed was stranded**: the detector records it, but nothing wakes a driver, and the later request that would re-wake it is refused because the slot is already occupied | `request_reset` stores the request and notifies only when a waker is present (`casper/src/interpreter_util.rs:429-441`) → before #340 `install_reset_waker` overwrote the cell without observing the pending slot (`:423-425`), and a duplicate request returns `false` at the `slot.is_some()` guard (`:432`) *before* any notify → a request raised between a detector scan and the waker's installation had no wake at all | **loud** (the detector's `log.error` fires) but the node never resets; **closed 2026-10-13 (#340, register C275)**: installation now takes the request lock before the waker lock and notifies when the slot is occupied | Drift | R2 | casper/src/interpreter_util.rs::a_reset_request_is_raised_once_taken_once_and_wakes_the_driver |
| **F-U11-02** | AS WELL AS | **The cells were process-global while the reset state is per-shard**: a request raised in shard A woke whichever shard installed the waker last | `RESET_REQUEST`/`RESET_WAKE` were process statics, and `install_reset_waker` runs once per shard (`casper/src/engine/node_launch.rs`, from the shard loop at `node/src/runtime/node_runtime.rs`) — so the last shard won and `take_reset_request` consumed A's request for B | silent: shard A never saw its own request; shard B refused it (A's anchor is not in its block store). **Closed 2026-10-13 (C280)**: both cells are keyed by the shard's lane (`ShardId::to_string()`), so a request reaching another shard's driver is unrepresentable rather than refused — and with the fatal stop of F-U12-03 landed, a misdelivery that refused would have been a whole-node kill | Drift | R1 | casper/src/interpreter_util.rs::a_reset_request_is_raised_once_taken_once_and_wakes_the_driver |
| **F-U11-03** | EARLY | **The request is consumed before the act**: `act_on_reset_request` took it, then `perform_reset` could fail before touching any store (the anchor body may be absent), spending the request with nothing done | `take_reset_request()` was the first statement; the anchor read that can fail was inside `perform_reset` | loud (`apply` returns the error) but the request was gone and there was no in-process retry; recovery was a restart, re-derivable only because `restore_attempts` is persisted. **Closed 2026-10-13 (law 73, C276)**: the act peeks (`reset_request()`), runs the anchor pre-flight, and only takes the request once `perform_reset` has returned — so a failure that destroyed nothing leaves the request pending and the detector's next scan can act on it | Unrestorable | R2 | casper/tests/reset.rs::a_reset_request_rewinds_the_stores_when_the_switch_is_on_and_does_nothing_when_it_is_off |

### Table C — law coverage

| deviation | law that captures it, or the law owed | why |
|---|---|---|
| U11-01 | law 53c (`rewindStep`, `strandSystemRewinding`) | the rewind's *availability* (anchored) and *effect* (a refusal is removed); the delivery half is Rust-only and not a state relation |
| U11-02 | **R1 — unrepresentable, and no theorem is owed.** The lane *is* the key (`ShardId::to_string()`), so "a request raised for shard A is taken by shard B" has no state to write; a per-lane model would make the bad state unwritable, and a law about an unwritable state restates its own definition — the register's `vacuous` status, which exists to stop exactly that | the shard lane is a Rust partition, and the doctrine is to make the bad state unrepresentable rather than to guard (or prove) against it |
| U11-03 | owed, and written first as law 73: **a raised request is not spent until the act commits** | idempotent retry of a pre-commit failure — law 53c models the rewind as an *effect*, not as a retryable transaction |

### U12 — The durable rewind (metadata, bodies, marker)

**Design intent.** A rewind either completes and is recorded, or is repaired at boot — never half-applied
and unrecorded — and the bodies it drops go with the metadata so nothing can serve or re-inject them.

**Guide words.** row: PART OF, AFTER, NO · folded: MORE → PART OF, LATE → AFTER, EARLY → PART OF, LESS → PART OF, BEFORE → PART OF, AS WELL AS → PART OF, OTHER THAN → PART OF · vacuous: REVERSE (a rewind is a one-way truncation)

### Table A — deviations

| ID | Guide word | Deviation | RCA → causal chain | Current behaviour | Class | Disposition | Falsifier |
|---|---|---|---|---|---|---|---|
| **F-U12-01** | PART OF | **The rewind is two store operations with no transaction between them**, so a crash between them leaves bodies the DAG does not hold — the receiver's own re-injection set | `drop_above` rewrites `block-metadata` in the `dagstorage` LMDB environment while `block_store.delete` removes bodies from the separate `blockstorage` environment (`casper/src/storage.rs:47-56`; no cross-store batch or two-phase commit exists anywhere in `shared/src/lmdb.rs`) → the window → **closed 2026-10-11 (#328, C274)**: a two-phase `ResetMarker` brackets the pair and `resume_after_reset` repairs a `Rewinding` at boot | **loud and repaired**: the marker is read at boot, the rewind re-applied, and the bodies purged by predicate (`casper/src/engine/reset.rs:105-186`) | Drift | R3 | casper/tests/reset.rs::a_crash_after_the_rewind_is_repaired_at_boot_by_the_marker |
| **F-U12-02** | AFTER | **The marker's lifetime is unbounded on a node that cannot catch up**: it is cleared only when the catch-up frontier passes the anchor, so a node that resets and never catches up re-enters the boot guard at every restart | `clear_reset_marker` has exactly one caller, `run_catchup`, and only when `frontier > marker.height` (`casper/src/engine/catchup.rs:276-296`); a walk that times out or ends below the anchor leaves it → intended (the doc says the condition is the walk's result, not its exit reason) — but the residue is a durable artifact that outlives its reset with no bound and no surface | loud (the boot guard warns each time) and **unbounded**: nothing reported how long a marker had stood. **Closed 2026-10-13 (C287, §111)**: the marker is on the status surface — `resetMarker` renders its phase, anchor and height, written by every marker write, read and clear (`casper/src/dag.rs`) so it cannot go stale behind the store — and a standing marker read beside `unrestorableRecords` and a height that does not move is the operator's signal that the repair is not converging | Historic | R2 | casper/src/interpreter_util.rs::the_reset_marker_surface_follows_the_store |
| **F-U12-03** | NO | **A failed reset does not stop the node**: the "fatal on purpose" error is a `return Err` from a task nobody joins, so the block receiver, processor, proposer and API keep serving a node whose stores may have moved under a view that was not rebuilt | `engine::reset` propagates its error (`casper/src/engine/reset.rs`) and the spawn wrapper only logs it (`node/src/runtime/node_runtime.rs`), while `main`'s wait selects on the listeners and a signal only (`node/src/main.rs`) — so the process does not end and the other tasks are not signalled. The same gap makes the LFS terminal path's "this node is stopping" (`casper/src/engine/node_launch.rs`) a logged sentence | loud (the line is logged) but **not a stop** — the outcome the reset's own comment says must not happen. **Closed 2026-10-13 (C279)**: `shutdown::report_fatal` sets a latch the wrapper writes and `main`'s wait selects on, so a failed reset — and the terminal path that shares the sentence — stops the process without draining | Terminal | R2 | node/src/runtime/shutdown.rs::a_reported_fatal_resolves_the_wait_with_the_first_reason |

### Table C — law coverage

| deviation | law that captures it, or the law owed | why |
|---|---|---|
| U12-01 | **law 72**, a completion record brackets every durable effect (`Rchain/Durability.lean`) | `rewindStep` (law 53c) gives the rewind's *effect*; law 72 gives its *durability* |
| U12-02 | owed: law 72's release condition — when the record may be cleared | law 72 states that the record is written and what it brackets, not when it is released |
| U12-03 | owed, and written first as **law 76**: a concluded stop stops (`Rchain/Stop.lean`) | the fault's response is the process's own liveness, stated over law 51's system |

### U13 — The runtime root

**Design intent.** When the reset completes, the runtime's durable root names the anchor's state, so the
node validates and serves from the state it rewound to.

**Guide words.** row: AFTER, PART OF · folded: LATE → AFTER, EARLY → PART OF, NO → PART OF, LESS → PART OF, MORE → PART OF, BEFORE → PART OF, AS WELL AS → PART OF, OTHER THAN → PART OF · vacuous: REVERSE (the root is a single pointer, not a directed operation)

### Table A — deviations

| ID | Guide word | Deviation | RCA → causal chain | Current behaviour | Class | Disposition | Falsifier |
|---|---|---|---|---|---|---|---|
| **F-U13-01** | AFTER | **The runtime root is written *after* the completion marker**, so a crash between the two leaves the record claiming the stores agree while `CURRENT_ROOT` still names the dropped tip | `rewind_and_purge` wrote `Rewound` and only then did `perform_reset` call `runtime().reset()` → `HistoryRepository::reset` → `roots_repository.validate_and_set_current_root` (`rspace/src/history/history_repository.rs:337`), a durable write; `reset_volatile`'s own doc (`:346-358`) names this exact class for the merge ("a crash between the two leaves `CURRENT_ROOT` naming an ancestor … resumes from neither state") | **silent** before the fix: `resume_after_reset` on a `Rewound` marker re-applies nothing, so the disagreement was permanent within the marker protocol. **Closed 2026-10-13 (law 72, C277)**: the two marker writes moved to the callers, around all three effects, so a `Rewound` record is now written only once the root has moved — the state is gone by construction | Split | R1 | casper/tests/reset.rs::the_guard_refuses_a_rewinding_whose_anchor_body_is_absent |
| **F-U13-02** | PART OF | **The boot repair covered two of the three durable effects**: the `Rewinding` branch re-applied the rewind and the body purge but not the root, because the guard held no `RuntimeManager` | `resume_after_reset`'s signature took `dag`, `block_store`, `log`, `source` only (`casper/src/engine/reset.rs:105-110`); its caller (`casper/src/engine/node_launch.rs:401-402`) *does* hold the `RuntimeManager` | loud at the point of failure, but a node that crashed after the rewind and before the root write repaired two-thirds of its reset and then served from the wrong base. **Closed 2026-10-13 (law 72, C277)**: the guard now takes the manager and re-establishes the root, and refuses an anchor whose body it cannot read | Drift | R2 | casper/tests/reset.rs::a_crash_after_the_rewind_is_repaired_at_boot_by_the_marker |

### Table C — law coverage

| deviation | law that captures it, or the law owed | why |
|---|---|---|
| U13-01 | owed: the completion-record law (U12), whose shape is *three effects, one record* | no existing law orders a record against a set of durable effects |
| U13-02 | owed: the same law's repair clause | the guard's repair must be total over the effects the record brackets |

### U14 — The refusal budget

**Design intent.** An attempt is spent exactly once per revalidation; the cap bounds the process; and the
count survives a restart rather than handing the record a fresh budget.

**Guide words.** row: MORE, AS WELL AS · folded: LATE → MORE, NO → MORE, LESS → MORE, PART OF → AS WELL AS, OTHER THAN → AS WELL AS, BEFORE → MORE, AFTER → MORE, EARLY → MORE · vacuous: REVERSE (the count is monotone; it is never decremented)

### Table A — deviations

| ID | Guide word | Deviation | RCA → causal chain | Current behaviour | Class | Disposition | Falsifier |
|---|---|---|---|---|---|---|---|
| **F-U14-01** | MORE | **The read-modify-write of `restore_attempts` is two separately-locked operations**, so two concurrent restores of one hash can both read `n` and both write `n + 1` — two revalidations spending one attempt | the read is `dag.lookup` (`casper/src/multi_parent_casper.rs:726`, `:1138`) and the write is `dag.update_metadata` (`:797`, `:1214`), and the insert lock is taken only *inside* `update_metadata` (`casper/src/dag.rs:961-974`) → the pair is crash-atomic (one `store.put`) but not concurrent-atomic | silent: the cap is reached one revalidation early or late, and nothing counts the discrepancy | Drift | R2 | owed: the read-modify-write under one lock, with the test that a concurrent double-restore spends two attempts |
| **F-U14-02** | AS WELL AS | **The reset request was raised on the uncommitted record**: `note_unrestorable_if_exhausted` ran with the incremented-but-unpersisted `restore_attempts`, so a request could be raised for a count the store then failed to record | the call sites ran *before* the `update_metadata` that persists the record, so if the write failed the node had already asked for a reset on a record whose stored count is one lower | loud (the detector logged) but the request's premise was not durable. **Closed 2026-10-13 (law 75, C281)**: the note now runs *after* the write in both paths, so a request raised by it names a count the store holds | Drift | R2 | owed: the write's failure path has no fault-injection point here, so the ordering is pinned by reading rather than by a test |

### Table C — law coverage

| deviation | law that captures it, or the law owed | why |
|---|---|---|
| U14-01 | law 73's shape — a two-phase operation on a cell is one step — applied to the count | the cap is a count, not a state, so the concurrency half is the split-read-write shape law 73 already states, and the fix is the Rust one-lock section |
| U14-02 | owed, and written first as law 75: **a dependent action is taken only on a committed record** | the request must be raised on the committed count, not on the value the store has not accepted |

### U15 — The RSpace checkpoint

**Design intent.** A checkpoint commits the hot-store overlay and the native overlay together, or
neither.

**Guide words.** row: PART OF, EARLY, AFTER · folded: MORE → PART OF, LESS → PART OF, NO → PART OF, AS WELL AS → PART OF, OTHER THAN → PART OF, BEFORE → EARLY, LATE → AFTER · vacuous: REVERSE (a checkpoint is a one-way commit)

### Table A — deviations

| ID | Guide word | Deviation | RCA → causal chain | Current behaviour | Class | Disposition | Falsifier |
|---|---|---|---|---|---|---|---|
| **F-U15-01** | EARLY | **`create_checkpoint` drains the native overlay before the trie fold**, so a fold failure loses it while the hot-store side is left untouched — the two sides do not commit or fail together | `native_store.drain_native()` clears the overlay (`rspace/src/native_store.rs:477`) at `rspace/src/rspace.rs:604-609`, and `last_native_changes`/`last_native_drain` are written there — all *before* `checkpoint_with_native`, the only `?` in the function (`:616-621`) | **silent**: on a fold error the native mutations were gone from memory and not in the trie, and `last_native_*` held the failed checkpoint's drain. **Closed 2026-10-13 (law 74, C278)**: the overlay is snapshotted before the drain and restored if the fold fails, and `last_native_*` is written only once the fold has committed — the state a failed fold leaves is the state it started in | Split | R1 | owed: a fault-injection point for the fold, which the rspace tests do not yet have |
| **F-U15-02** | PART OF | **A `SoftCheckpoint` carries no root**, so a revert rebuilds the snapshot over the history base *at revert time* — a hard checkpoint between the snapshot and the revert silently restores old overlay over a new base | `SoftCheckpoint` holds the cache snapshot, log, produce counter and native snapshot, no root (`rspace/src/checkpoint.rs:14-21`); `revert_to_soft_checkpoint` re-reads `history.root()` at revert time (`rspace/src/rspace.rs:698-707`) | silent but **latent**: no production caller runs a hard checkpoint between a soft checkpoint and its revert (the per-deploy fallback is taken and reverted inside one `validate`) | Historic | R2 | owed: either a root on the soft checkpoint, or a guard, with a test of the interleaving |
| **F-U15-03** | AFTER | **The hard commit is leaf-before-root**: the new root is recorded last, so an interrupted commit leaves orphaned leaves and trie nodes that are not reachable from any root | leaves are written (`rspace/src/history/history_repository.rs:302-313`), then trie nodes (`:316`), then the root (`:317`) — three independent transactions in one LMDB environment, not one | not loud; the orphans are harmless because content-addressed, but the record's partial state is the programme's **named atomicity deviation** | Drift | R2 | owed: the deviation is documented (`docs/src/spec/failure-hazop.md` §1's ladder target); a test would pin the reachability, not the write order |

### Table C — law coverage

| deviation | law that captures it, or the law owed | why |
|---|---|---|
| U15-01 | owed, and written first as law 74: **a checkpoint does not drain an overlay it has not folded** | no existing law models the overlay's drain against its fold; `Rchain/RSpace/*.lean` models the trie, not the clear |
| U15-02 | owed: the same law, with the soft checkpoint's base as a modelled field | the missing field is the absent root |
| U15-03 | law 49 / `Rchain/RSpace/Merkle.lean` cover content addressing; the *ordering* is owed | content-addressing makes the orphans harmless; the write order is not a modelled state |

### U16 — The operator's reconciliation

**Design intent.** A recovery the operator runs is resumable, never overwrites its own asides, and states
what it adopted rather than implying a quorum it did not compute.

**Guide words.** row: PART OF, MORE, AFTER · folded: AS WELL AS → MORE, NO → PART OF, LESS → PART OF, EARLY → PART OF, LATE → PART OF, BEFORE → PART OF, OTHER THAN → PART OF · vacuous: REVERSE (the tool performs one directed recovery)

### Table A — deviations

| ID | Guide word | Deviation | RCA → causal chain | Current behaviour | Class | Disposition | Falsifier |
|---|---|---|---|---|---|---|---|
| **F-U16-01** | MORE | **The aside names collided at minute resolution**, so a retry within the same minute **overwrote the only good backup** on the docker path — the operator's one guarantee ("never deleted") failing exactly when it is needed | `TS` was `%Y%m%d-%H%M`; the docker branch's `tar cf "$backup"` wrote the same-named file again, so a second same-minute run re-tarred the *already-cleared* volume over the first (good) backup. **Closed 2026-10-13 (C282)**: the name is to the second *and* the branch refuses an existing target (`tools/reconcile-network.sh`) | loud on the terminal, but the data was gone; the systemd branch `mv`s into the existing dir instead (mangled, not lost) | Unrestorable | R2 | owed: the tool has no test harness, so the refusal is pinned by reading |
| **F-U16-02** | AFTER | **The systemd `--restore-from-master` branch overwrote in place and printed "unchanged"**: a mid-stream `tar` failure left the joiner partially written while the message said it was not | the branch streamed a `tar` into the live data dir, never removing envs the master lacks (joiner-only files survived → divergence residue), and printed `"the copy failed — $n is unchanged"` on failure; the docker branch was already the careful copy-beside-then-swap. **Closed 2026-10-13 (C283)**: the systemd branch stages beside the data dir and swaps each environment only once the whole copy has arrived, so "unchanged" is true when it is printed (`tools/reconcile-network.sh`) | loud but **false**, the message contradicting the state | Drift | R2 | owed: the tool has no test harness, so the swap is pinned by reading |
| **F-U16-03** | PART OF | **The tool keeps no durable intent journal**, so a partial application is reconstructible only from the timestamped asides — unlike the node's own reset, there is no phase record a later run can read | the step-5 move loop (`tools/reconcile-network.sh:953-958`) and the step-5b copy loop (`:906-935`) write no marker; a crash mid-loop leaves some nodes moved and others not, with nothing on disk saying so | loud (each refusal names itself and exits with its own code) but **not resumable**: a retry recomputes the meet against a half-mutated net and may refuse | Drift | R4 | owed: a journal or a `--resume`, stating which nodes were already applied |

**`dagstorage` travels in `CHAIN_ENVS`** (`tools/reconcile-network.sh:180`), and the reset marker lives in
a `reset-marker` DB inside `dagstorage` — so step 5b copies a marker onto a joiner and step 5's docker
`rm -rf` clears one. Recorded in F-U16-03's RCA rather than given a row: it is a consequence of the same
absence of a phase record.

### Table C — law coverage

| deviation | law that captures it, or the law owed | why |
|---|---|---|
| U16-01 | owed: nothing in the register speaks of an operator's asides | the register is the node's mathematics and the tool is not modelled — the fix is a shell refusal, not a theorem |
| U16-02 | owed: — | same; the fix is a copy-beside-then-swap |
| U16-03 | owed: — | the tool's phases are not modelled, and this row is not fixed here either |

### U17 — Resource admission and degradation

**Design intent.** A node starts only where it can replay and serve, sheds work before it is out of
memory, and degrades into a defined, observable, recoverable state rather than being killed — while never
acknowledging work it has not durably accepted.

**Guide words.** row: NO, MORE, LESS, AS WELL AS, PART OF, OTHER THAN, REVERSE, EARLY, LATE, BEFORE, AFTER

### Table A — deviations

| ID | Guide word | Deviation | RCA → causal chain | Current behaviour | Class | Disposition | Falsifier |
|---|---|---|---|---|---|---|---|
| **F-U17-01** | NO | **No start-up preflight**: a host below the chain's replay floor starts, replays into OOM and never opens its API | nothing reads `/proc/meminfo`, cgroups or disk free; the expensive `BlockMetadataStore::create` rebuild runs before any check; the documented replay model is unenforced | silent until the kernel OOM-kills `rnode`; the service still reports `active` | Terminal | R2 | node/tests/resource_preflight_arming.rs::the_preflight_refuses_a_validator_warns_an_observer_and_honours_the_override |
| **F-U17-02** | MORE | **More in-flight work than memory allows**: no memory-aware admission; HTTP can decode 2 MiB per request, gRPC 16 MiB, explores fork runtimes, and the pool is count-based | admission is rate/count only (`dag.rs:35,47,66`); #426 is unmerged and disclaims memory semantics and gRPC | accepts the work, then the host OOMs | Terminal | R2 | node/src/web/http.rs::a_deploy_is_refused_503_before_the_body_is_decoded |
| **F-U17-03** | LESS | **Less headroom than the floor and no ringfence**: no cgroup limit/CPU-quota read, two validators per 1 GB host, `available_parallelism` ignores `cpu.max` | no cgroup awareness; no co-location detection | silent; the operator sees only the external monitor | Drift | R2 | node/src/resources/cgroup.rs::parses_v2_memory_cpu_and_events |
| **F-U17-04** | AS WELL AS | **Alongside the pressure the node keeps accepting and advertising healthy**: no `degraded`/`acceptsDeploys`, `is_node_read_only` is config-only | no pressure state machine or status field; the autopropose halt stops production only | 200s and a healthy-looking status while starved | Drift | R2 | node/src/api/conversion.rs::accepts_deploys_is_false_under_pressure |
| **F-U17-05** | PART OF | **Only part of the path is bounded**: per-peer rate and pool count yes; bytes, duplicates and gRPC no | term bound applied after body decode (`multi_parent_casper.rs:99,120`); no byte pool; full pool is 400; duplicate reparse admitted | unauthenticated amplification | Drift | R2 | owed: unit B — `casper/src/dag.rs::a_pool_at_its_byte_cap_refuses_before_the_store` |
| **F-U17-06** | OTHER THAN | **The node does work other than requested**: a byte-identical duplicate is parsed and committed again; a 16 MiB wire form is decoded before the 64 KiB term bound; a large term is parsed before the block phlo check | `deploy()` has no duplicate/in-flight check before `source_to_adt_with_env`; decode caps are above the term bound | CPU/memory amplification on unauthenticated ingress | Drift | R2 | owed: unit B — `casper/tests/deploy_duplicate_reparse.rs::a_duplicate_deploy_is_refused_before_the_parse` |
| **F-U17-07** | REVERSE | **Restart reverses the failure into a loop**: a node killed during replay restarts and replays into the same OOM | no crash-loop marker or preflight; `Restart=on-failure` repeats the replay | the unit stays `active` while the API never opens | Terminal | R3 | spec/audit/evidence/n420-restart-20261024/restart.json |
| **F-U17-08** | EARLY | **The guard fires early on a healthy node**: a shared cgroup or page-cache accounting can make `MemAvailable`/`memory.current` look critically low and throttle a healthy validator | thresholds/hysteresis do not exist yet; cgroups may be shared | latent — there is no guard today | Drift | R2 | node/src/resources/fallback.rs::a_reading_at_the_boundary_does_not_oscillate |
| **F-U17-09** | LATE | **The guard fires late**: a watchdog on the tokio runtime or a long sample interval misses a fast allocation spike, and the node OOMs before admission closes | no watchdog exists; a tokio-based one would share the starved executor | the kernel chooses the victim | Terminal | R2 | node/src/resources/fallback.rs::a_stale_watchdog_is_unavailable_not_normal |
| **F-U17-10** | BEFORE | **`CURRENT_ROOT` is published before the block is accepted**: replay reaches `create_checkpoint` → `record_root` before `dag.insert`, so a refused or dropped block can leave the node's current root at a state no accepted block references | checkpoints now record the root as **known only** (`rspace/src/rspace.rs`, `rspace/src/history/history_repository.rs`); an accepted block publishes at the insert site (`casper/src/blocks/block_processor.rs`), and boot republishes the latest accepted root (`casper/src/engine/node_launch.rs`) | observable after restart: `CURRENT_ROOT` names an unaccepted state | Drift | R2 | casper/tests/current_root_publication.rs::a_failed_insert_leaves_the_current_root_where_it_was |
| **F-U17-11** | AFTER | **Derived-index cleanup after a metadata drop is skipped**: `drop_above` returns on an empty metadata drop before cleaning the deploy/deployer indexes, so a crash between the two leaves rows pointing at unknown blocks and a pooled deploy can be silently skipped | early return removed; stale rows are now derived from the **surviving** block set, and the reset bracket and boot both run the audit (`casper/src/dag.rs:410`, `casper/src/engine/reset.rs:110`, `casper/src/engine/node_launch.rs:458`) | silent skip of a pooled deploy | Drift | R3 | casper/src/dag.rs::repair_removes_stale_entries_and_releases_the_pool_slot |

### Table C — law coverage

| deviation | law that captures it, or the law owed | why |
|---|---|---|
| U17-01 to U17-09 | owed: no law models host admission; these are node-local operating-policy rows, not consensus state relations | the disposition is a refusal/status/recovery test, not a theorem; the Lean4-first rule applies where a consensus state relation is at stake |
| U17-10 | law 80 — `CURRENT_ROOT` names only an accepted block | `Rchain.Publication`: `record` writes a known root, `accept` publishes one, and `a_current_root_is_an_accepted_state` rules out the orphan; `an_early_publish_reaches_an_orphan_root` is the replay-checkpoint shape |
| U17-11 | law 72 (extended) — a rewind's completion record brackets its derived indexes as its fourth effect | `a_rewound_record_implies_every_effect_applied` now concludes `indexed = true`, and `a_skipped_index_cleanup_reaches_a_record_that_lies` is the early-return shape the extension rules out |

## 4. The fault tree

```
                    the node cannot repair itself
                                │
    ┌────────────┬──────────────┼──────────────┬──────────────┐
    │            │              │              │              │
 the merge    the file       the lock      the operator   the buffers
 refuses      ingests       serializes     cannot see     never shrink
    │            │              │              │              │
 F-U4-01..05  F-U8-01        F-U9-01        F-U10-01       F-U9-04 caches
 F-U1-01      F-U8-02        F-U9-02        F-U10-02       F-U9-05 pool
 F-U5-02      F-U8-03        F-U9-03        F-U10-03       F-U2-01 guard
    │         F-U2-01           │              │          vs partition
    │            │              └──────┬───────┘              │
    └────────────┴─────────────────────┤                      │
                                       │                      │
                     ┌─────────────────┼──────────────────────┘
                     │                 │
                a NODE stalls     the CHAIN stalls       the net runs out
                 (TE-5)            (TE-1, TE-6)              (TE-4)
                     │                 │                       │
                F-U5-01 no reset   F-U3-01 no fringe      R2's bounds —
                path once running  to restore from, and    the three
                     │             F-U5-01's absence is    unbounded ones
                   ── R3 ──        what makes it permanent  are the whole
                (owner: #328)             │                 finding
                                      ── R3 ──
                                   (owner: #328)
```

**The tree's one sentence.** Every branch ends in the same place: a state the node cannot leave, or a
resource no rule releases — reached with a log line at most, and with no operation to undo it. The
loudness is nearly all won; the undoing is entirely owed.

## 5. The R3 gap, and its owner

**The gap.** A node that cannot locally heal has, today, exactly one way back: an operator stops it and
deletes the shard data dir by hand, so `dag_set` empties and `NodeSyncing` runs. That path is written
down — as a *comment*, in `casper/src/dag.rs:534` ("the only way back is deleting the shard data dir so
`dag_set` empties and `NodeSyncing` runs") — and it is the only one. For the *chain* (TE-6) there is no
path at all: the 2026-10-08 witness recorded four divergent heads, one restart experiment on two nodes
(which restored production and added two more heads), and finality frozen at 101 throughout.

**The owner, and what it became.** The reconciliation utility is **#287** — "bring a busted network back
to one chain without a genesis" — and its design has been corrected twice by measurement since this page
was written, which is why the paragraph here describes what it *does* rather than what it was first asked
for:

- the first form was "read `last-finalized-block` from every node and **stop if they disagree**", which
  the issue replaced with a **meet** — the deepest block a strict supermajority of *stake* has finalised,
  because stopping leaves the operator to choose a winner by hand and an operator choosing is how a stall
  becomes a lie;
- the second form was a **unanimous-anchor gate** (identical finalised height *and* hash on every node),
  which is honest and unusable where the tool is needed: on a four-validator net with two stopped, the
  three survivors read finalised 102 / 97 / 110, which is an ordinary producing chain rather than a
  disagreement. That gate was #287's design "as it stands" for a while, and the tool said so everywhere it
  printed;
- what is in the tree now (§98) is the **meet over artefacts**: every node *attests* its finalised block
  (`/api/finality-attestation`, signed by its validator key and bound to the run's nonce), every block the
  tool touches is verified from the preimage of its hash (`/api/block/{hash}/raw`), the point is the
  deepest block a strict supermajority of stake has finalised, and a *proven* equivocator's stake leaves
  both sides of the fraction. Where nothing is finalised at all there is nothing to meet on, and the
  operator names the block (`RECONCILE_ANCHOR`), which the tool labels as their root rather than a meet it
  computed.

Its invariants are this page's TE-5/TE-6 separation in one line: *never* treat an unfinalised block as the
truth, and *never* drop a deploy silently — and, after §98, a third: *never* weigh a report it has not
verified.

**What stays in the tree when #287 lands.** The utility is the operator's tool (R4's runbook, made
executable), and it is delivered — the operator path exercised on a frozen net, the meet exercised on a
devnet. The *node-side* rung — a running node that detects its own unrecoverable divergence and resets
itself — is F-U5-01, and it is a separate unit: #287 repairs a chain from outside, while F-U5-01 would let
a node rejoin one from inside. They share the detection (a divergence record that exhausts its restore
budget) and the reset primitive; they do not share a deliverable, and F-U5-01's acting half is **#320**.

**Ships dark.** A reset is destructive. If F-U5-01 is built, it ships detect + log + count first, so an
operator sees what it *would* have done before it is allowed to do it — and F-U10-01's stall-reason
surface is the prerequisite, not a nicety.

**Built, 2026-10-10 (#320, register C274).** Dark first was the rule and it was kept: the detector, the
`unrestorableRecords` count and the dry run shipped in §82, and the acting half came after, **off by
default** behind `casper.self-reset` — so the two-key shape this paragraph asks for is what landed (the
node detects, counts and logs; the operator consents). The prerequisite is in the tree too
(F-U10-01's surface, refined by #318). See F-U5-01's row for what the rung still owes (the engine-level
test, the restart-surviving marker, and the Lean step + companion theorem).

## 6. What this pass corrects in the record

1. **The 2026-10-08 incident had two faces, and the register carries one.** C248's row says *"It is not
   C215: no two-node divergence, no `InvalidPreStateHash` between peers"*. The TE-1 witness for the same
   incident records the opposite above the write loss: `state-hash disagreement on pre-state: block
   #104` with three different `#104` hashes cited, and four distinct `#106` blocks. **Adjudication**: the
   clause is true of the *write-loss mechanism* (which is why no node refused a block at height 103) and
   false as a statement about the incident. #280 remains open on the second failure, and this is recorded
   rather than silently corrected.
2. **The log C248's residue clause 4 asked for now exists.** That clause says *"#280's second failure
   (validators stopping production) is not causally closed — it needs node A's log around #106."* The
   witness contains exactly that line — `ERROR Self-created block #106 (seq 106) failed validation: the
   block's rejected-deploy set does not match its parent` — and the two restart experiments that show the
   pre-restart failure was transient *process* state. The register's residue is therefore narrower than
   it reads.
3. **The witness mis-cites the reconciliation issue.** It says *"a candidate mechanism is proposed in the
   reconciliation issue (#283)"*. #283 is `perf(#144): measure soft-checkpoint cost before choosing the
   first TPS lever`; the reconciliation utility is **#287**. A citation defect in a committed artifact —
   the class the OCapN HAZOP recorded as R4, and the reason §7's provenance is a list rather than a
   gesture.
4. **The two documents label the same validators with different letters.** The #280 capture fixes
   `A 0410b8c5, B 041ed2a2, C 04d7707c, D 04dce59b`; the witness calls `041ed2a2` **D**, `04d7707c`
   **B** and `04dce59b` **C**. Same keys, rotated letters — so a reader cannot join the two by name, and
   the witness's "A and D share a host" conclusion is about `0410b8c5` and `04dce59b`, not about the
   capture's A and D. Worth fixing in the witness before it merges.
5. **The programme's own seed catalogue carries a claim this pass refutes.** It listed *"Unbounded
   merge-scope search — measured (`search_census`) but not bounded"* as an R1/R2 candidate. The
   measurement is real and **the bound is too**: `SearchBudget::NODE` (10,000,000 steps / 1,000,000
   options, `sdk/src/dag/merging.rs:280`) is applied to the merge search at `casper/src/merging.rs:1851`,
   and *exceeding* it is precisely the drop F-U4-01 rows. What is unbounded is the **neighbouring fold**,
   which the acceptance sheet already records: `fold_rejection`/`traverse_tree` take no budget and
   `traverse_tree` walks with no visited set (H-U4-04, U4's `PART OF`, `owed` here). So the seed's
   conclusion was right about the node and wrong about the site — it pointed at the one place a budget
   already exists.
6. **Two of the rows above were not found by this pass, and the page says so rather than implying
   otherwise.** F-U4-04 came from #281's review and F-U4-05 from the fix for the CI hang that review led
   to. Both deserve their row and both were missed here, and the reason is specific enough to be useful:
   this pass looked for hazards in **behaviour** — what a node does when a store read fails, a lock is
   poisoned, a search is refused — and neither of these is a behaviour. One is a **sidecar** that kept one
   action per slot; the other is a **type** whose order disagreed with its equality. The class they share
   is the one the ladder's first rung exists for, and it is reachable only by reading the types rather
   than the control flow — which is the pass this page has not yet run.

## 7. Provenance

read: `casper/src/merging.rs` (the three refusal sites, `MergeReport`, I1/I2, `MergeOutcome`),
`casper/src/blocks/block_processor.rs` (the drop arm and the deferral note), `casper/src/multi_parent_casper.rs`
(`ValidateError`, the `:850` funnel, `RESTORE_ATTEMPT_LIMIT`, `restore_is_warranted`, `mark_failed`, the
neglect rule), `casper/src/interpreter_util.rs` (`validate_block_checkpoint`, `describe_no_advance`, the
merge-report warn, the stall warn), `casper/src/dag.rs` (`insert` and its discipline comment,
`update_metadata`, the shard-dir comment), `casper/src/engine/node_launch.rs` (the boot `if` chain),
`casper/src/engine/node_syncing.rs` (`MAX_SYNC_ATTEMPTS`, the terminal stop, `populate_dag`),
`casper/src/blocks/block_receiver.rs`, `casper/src/blocks/block_retriever.rs`,
`casper/src/engine/node_running.rs`, `casper/src/blocks/proposer/proposer.rs`, `casper/src/api/block_api.rs`,
`casper/src/protocol/comm_util.rs`, `block-storage/src/dag/finalizer.rs`, `block-storage/src/dag/liveness.rs`,
`node/src/api/dto.rs`, `node/src/api/conversion.rs`, `node/src/runtime/node_runtime.rs`,
`node/src/runtime/node_main.rs`, `node/src/main.rs`, `sdk/src/dag/merging.rs` (`SearchBudget`),
`rspace/src/lock.rs`, `rspace/src/history/radix_tree.rs`, `rspace/src/history/instances/radix_history.rs`,
`rspace/src/hot_store.rs`, `rspace/src/rspace.rs`, `spec/Rchain/Casper/Stranding.lean`,
`docs/src/formal/progress.md` (the shape/cause vocabulary), `docs/src/spec/testnet-acceptance.md` §1–§2
(the study nodes, the top events, the TE-1 recovery verdict),
`spec/audit/evidence/n280-merge-loses-a-write-results.md`, the TE-1 witness at
`spec/audit/evidence/te-1-2026-10-09-four-divergent-heads.md` — which was on branch `spec/te-1-witness`
when this pass ran and has been **in this tree** since PR #298, so a claim taken from it while it was a
branch says so — and the reconciliation issue **#287**. ·
ran: `git rev-parse HEAD`, `git status`,
`git fetch`, `git show origin/spec/te-1-witness:<path>` (the file is readable in the tree now, so this
was the step it took then), `grep`/`ls`/`sed` over the files above (read
only), and `tools/check-hazop-worksheet.sh`. The anchors were re-derived in this pass; the ones on
`casper/src/dag.rs:489/696/729/736` and the two test names in §3's F-U4-03/F-U5-02 were re-read directly
after the earlier reads, because a citation this page publishes is a claim.

not read: the whole of `casper/src/merging.rs` (3,800+ lines), `casper/src/dag.rs`, and
`casper/src/multi_parent_casper.rs` — the named functions only; `spec/AUDIT.md`'s and
`spec/findings.tsv`'s C248 row in the register's emitted form; the remaining `spec/audit/evidence/`
runs; and the `rspace` history and store internals beyond the named cache and lock sites.

**Debt.** falsifiers owed: 22 · node-words owed: 44

*(A `F-` row with an `owed` falsifier is a deviation this pass **dispositioned** but did not **prove**;
the number above is checked against the page by `tools/check-hazop-worksheet.sh`, so it cannot rot.
`node-words owed` counts the acceptance worksheet's `row`-declared guide words this pass has not yet
reached.)*

## 8. Why this failure is hard to reach — the hardening, and what the R3 backstop is for

F-U5-01's arm spent three instruments failing to produce the state the reset repairs (the partition, the
state tamper, the perturbed build — §108, §110, §113), and the honest reading of that is not "the arm is
unrun" but **"the state has been designed out".** The hazard was dispositioned against the ladder's target
("ideally **unrepresentable**; where it is not, fail loudly and hard-reset"), and the tree has moved it
towards that first rung by *removing its producers* rather than by handling it better. Each step is
measured:

1. **The producer was deleted, not guarded.** §92's self-reject and TE-1's four heads came from the
   fringe-record store's `min` tie-break: two honest blocks finalising one fringe from two bases wrote one
   key, and the tie-break chose a `state_hash` the node had already built on. Law 66/67/68's per-block
   redesign retired the store, the join and the tie-break — *"two blocks that disagree are two visible
   claims, not one rewritten value"* — so the class has no producer left. **Measured**: `up --validators 4
   --fresh` in ordinary operation produced **zero** `[fringe-divergence]` and **zero** self-rejections over
   ~170 blocks, where the pre-fix build stalled three of four nodes at height 49 (§108, `#344`).
2. **The merge is a pure function of the parent set.** `casper/tests/merge_determinism.rs` is the standing
   property — two arrival orders of one block set merge identically — and **E1 measured it live**: a
   partition's two heads simply *re-converged* on reconnect, because a node with a different block set
   computes the same answer for a delivered block (`e1-partition.md`). So the whole family of
   *view* differences — a lag, a split, a reordered delivery — no longer produces a `Divergence` at all.
3. **What remains is healed before the budget spends.** A view-dependent refusal is revalidated up to three
   times (`restore_divergent_justifications`), and the budget only spends if the node's view is *still*
   divergent — so the counter the reset acts on is a **backstop for a genuine, persistent** disagreement,
   not for the ordinary lag of a producing net. The rule's own doc is explicit that it *"does not
   manufacture agreement"*: a node whose view has converged restores the block, and one that is still
   divergent is right to keep refusing.
4. **And the three instruments' failure is the measurement of (1)–(3).** A partition could not create a
   difference; a state tamper *did* change the computation (it stalls the chain with the merge's own
   documented wrong-base error) but could not be *aimed* at the block the merge selects; a whole-build
   perturbation produced the divergence and the spent budget **96 times** and then found nothing to rewind
   *to* — because a node that diverges before finalising anything has no anchor. The state R3 repairs
   needs a **partial** divergence: one that arises *after* a node has finalised, which is exactly the class
   (1) removed the producer of.

**What the R3 backstop is therefore for.** The deviation's *disposition* is still **R3** — a hard reset and
catch-up is what the node does if it ever gets there — but its *likelihood* is what the hardening changed:
nothing in the tree's own operation produces the state. So the arm being unrun is not a gap in coverage; it
is the **strongest form of the first rung applied to the hazard**, and the rung's remaining duty is not to
drill the reset but to keep it **safe if it ever fires**: the durable half, the two-phase crash marker, the
boot repair, the fatal stop when it cannot carry on, and the ordered request/spend — each pinned by a test
and, where the state is representable, by a law (72–76). That is the same standing a fire escape has: harden
the building so it is never needed, and keep the escape sound and tested in pieces.

**What it is not.** The hazard is not *impossible* — a future regression, or a genuine disagreement no
local restore heals (TE-6, the net-wide case), would still need R3 or the reconciliation. The claim is
narrower and checkable: **the producers this tree had are gone, and the three instruments that tried to
re-create one could not** (§110, §113). Nothing here rests on the arm having run.

**The progression, and the limit that has to be said with it.** An inherently safe design *retires* the
reproducible failure mode — that is what the first rung means, and it is forward motion: the states this
row's own reproductions exhibited (§92's self-reject, TE-1's four heads) **cannot be constructed from before
any more**. But the progression has a limit, and it is the point a reader is owed: **retiring the old
producer says nothing about what this build's failure mode would be.** The old reproduction is evidence
about the old build. **No failure mode is statable for the current one** — every candidate reduces either to
*a future regression that reintroduces a producer* (a statement about a build that does not exist) or to a
net-wide disagreement whose cause is by construction not in the tree (TE-6) — and **a failure mode nobody can
name is a nebulous risk, not a register item.** This repository's own rule is that a statement nothing can
falsify is not a statement (the register's `vacuous` status); an item that cannot be reproduced, cannot be
named and cannot be falsified is that shape, whatever column it sits in. So the arm is recorded here as a
risk and kept **out of the debt**, F-U5-01's row says so in its own words, and what remains checkable is the
backstop's own pieces — which is the whole of what "keep the fire escape sound" means.
