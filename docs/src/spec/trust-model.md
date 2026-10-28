# What a joining node trusts

A node with no state restores a prefix of the chain from a peer: it downloads blocks by hash, imports
a tuple-space trie whose pages it checks, and adopts the bootstrap's `FinalizedFringe`. "Trustless"
is a word that hides which of those inputs are actually checked. This page states the model precisely,
and Law 83 (`spec/Rchain/Sync/Trust.lean`) is its machine form.

The headline is **content-trustless, not finality-trustless**: every state byte and every block body a
joiner reads is checked against a hash, so two joiners with the same verified inputs cannot read
differently — but the joiner does *not* verify which root is final, nor the signatures that finalised
it.

## The classification

| Input | Class | What the joiner does |
|---|---|---|
| The configured genesis block and state | **genesis-anchored** | Reads them from configuration; they are the root of trust |
| The genesis `pos:trusted` set (`bond keys ∪ pos-multi-sig`) | **genesis-anchored** | Installed at genesis (`casper/src/genesis/mod.rs`); no later input extends it except a chain-authorised `trust` |
| A downloaded block body | **verified** | Recomputes `Blake2b256` over the body and refuses a mismatch (`validate::block_hash`) |
| An imported trie page | **verified** | Recomputes the node hash against its key, and the walk against the walk listing (Law 70) |
| An ancestry row's `block_hash` | **verified** | Must be a block the walk downloaded; duplicates refused |
| An ancestry row's `fringe` | **verified** | Must equal the fringe the finality gate derives from the block's justifications and bond map — the value a validating node derives |
| Which root the bootstrap calls finalised | **residual** | Assumed; the joiner cannot see the quorum that finalised it |
| The fringe's `state_hash` (root and each per-block `fringe_state_hash`) | **residual** | Assumed. It is the state at the fringe the served member finalises — one fringe earlier than the named block's pre-state — not a field of the block. **Law 85** proves the served value is always some member's claim and why a pin cannot work; a singleton pin was tried and refuted by the node tests (**C339**). The first block validated after the restore checks it indirectly against its `pre_state_hash` |
| Liveness (the peer serves the blocks) | **residual** | Assumed; the walk's give-up rule bounds the wait, it does not produce an answer |
| Wire signatures and quorum finality proofs | **residual** | Not checked on the join path |

The residual is `spec/Rchain/Sync/Trust.lean`'s `the_residual_is_exactly_named`: of the nine input
kinds the join consumes, the four that are neither anchored nor verified are exactly the ones listed.
Adding a tenth without classifying it fails the Lean build.

## The proved claim

`join_is_content_trustless`: if two joiners obtain the same verified block map and the same trie, then
at the same claimed root they read the same value for every key and answer the same block lookup. The
proof composes Law 70 (`import_reads_the_rooted_trie`, the trie pages) with `reads_determined`; block
bodies are content-addressed, so a block map that two joiners agree on holds the same bodies.

**Not claimed:** finality. The joiner trusts the bootstrap's choice of root. A proof-carrying fringe
would have to carry the attestations that finalised it, and the join path does not read them; that is a
protocol change, not a join-path check.

## The derivable half and the residual (C338, C339)

The per-block `fringe` **is** derivable: it is what `liveness::calculate_finalization` produces for
the block — the parent fringe when the gate does not advance, the next layer when it does — from the
block's justifications and its own bond map. The join path runs the same gate and refuses a row that
disagrees (`check_derived_fringe`). The fringe's **state** (the root and each per-block
`fringe_state_hash`) is **not**: it names the state at a fringe, not a field of a block, and
`close_block` folds it into the epoch seed. The joiner keeps the claim and logs it. That is **C339**,
and it stays open until the join path recomputes the merge (or the wire carries a proof for it).

**The state is a claim, not a field** (Law 85). The ordinary answer takes the max member's
`fringe_state_hash`, whose meaning is the state at the fringe **that member** finalises; the
recovery-anchor and fresh-shard answers take a block's `post_state_hash` instead. A member is not a
finaliser of the set it belongs to, and the two states differ by one fringe — which is precisely why a
pin comparing the root with the named block's pre- or post-state was false. It was implemented, the
node's restore tests refuted it, and it was removed rather than kept as false assurance. The law proves
the served value is always a member's claim; it does not supply the merge that would let the joiner
check it.

The same reasoning applies to the ancestry's shape: a row for a block the walk did not download, or two
rows for one block, is refused before anything is written. A peer that sends **no** ancestry rows is
older, not hostile: the join keeps the pre-#139 per-block WARN fallback rather than refusing to sync.

## The bonding trust axis

Admission and consensus membership are different lists (Law 82, `spec/Rchain/Casper/Lifecycle.lean`):

- `trust`/`untrust` change `pos:trusted`, and the caller must be a trusted key. The caller is bound to
  the unforgeable `deployerId`, so a program-authored key cannot act for another validator.
- `bond` is self-only and requires the bonder to be trusted; it moves the stake into the pool and
  **does not** activate — activation is the epoch boundary's re-selection.
- `untrust` **does not confiscate** (AUDIT C111): a bond is property, admission is not. An untrusted
  validator keeps its bond, stays in the active set until a boundary re-selects, and can withdraw.
- A slash needs evidence (`casper::validate::slashable_senders`); it is the only transition that
  confiscates, and it is graded by the offence.

The genesis trust set is therefore the anchor for admission, and the epoch boundary is the only place
membership moves. The Rust header at `rholang/src/native_state.rs` said the opposite for a long time
("`slash`/`untrust` remove the validator and confiscate the stake"); the comment was the defect.

## What this page does not say

- It does not certify a join. The in-process and devnet arms are finite witnesses; the live chain's
  replay remains the observation that counts.
- It does not close C339.
- It does not claim the bootstrap is honest about liveness: a peer that will not serve blocks fails
  the walk after the give-up rule, which is a bounded failure, not a restored state.

---

**Operating one:** [Running a node: testnet now, mainnet later](../node/validator-observer-setup.md).
