# What a joining node trusts

A node with no state restores a prefix of the chain from a peer: it downloads blocks by hash, imports
a tuple-space trie whose pages it checks, and adopts the bootstrap's `FinalizedFringe`. "Trustless"
is a word that hides which of those inputs are actually checked. This page states the model precisely,
and Law 83 (`spec/Rchain/Sync/Trust.lean`) is its machine form.

The headline is **content-trustless, not finality-trustless**: every state byte and every block body a
joiner reads is checked against a hash, so two joiners with the same verified inputs cannot read
differently — but the joiner does *not* verify which root is final, nor the signatures that finalised
it.

## Transfer and replay are separate checks

Law 83 (`spec/Rchain/Sync/Trust.lean`) models the original **transfer** boundary:
content-addressed blocks and trie pages at a peer-selected root. Its enumeration
of residual inputs is not a theorem about the added runtime replay stage.

Before publishing recovered metadata, `engine::recovery::RecoveryReplay::verify`
now builds an isolated DAG from the downloaded history, parents first. It checks
block hashes, shard/format and non-genesis signatures, then calls the existing
`multi_parent_casper::validate` for each non-genesis block. Each merge therefore
reads locally validated parent metadata, never a peer's fringe-state claim.
The first state anchor is the downloaded genesis: use `--expected-genesis-hash`
to bind that block to an independently trusted network identity. Without a pin,
the bootstrap's choice of genesis remains an assumed input.

| Input | Runtime admission check |
|---|---|
| Genesis | Exactly one height-zero block with no parents; content hash and configured genesis pin when present |
| Downloaded block | Hash/index/height, shard, format and non-genesis signature; normal block summary, replay and bonds checks |
| Imported trie page | Existing content-addressed page/walk validation (Law 70) |
| Supplied ancestry metadata | Membership and duplicate checks; supplied fringe and fringe-state must match locally recomputed metadata |
| Missing ancestry metadata | Reconstructed by replay; no empty/zero metadata fallback |
| Ordinary target root | Compared with the locally derived claim of the max `(height, hash)` target member, as Law 85 requires |
| Explicit anchor or fresh-genesis target | Compared with the named block's post-state |
| Target finality | **Still assumed** from the bootstrap/operator; no quorum certificate was added |
| Liveness | **Still assumed**; existing bounded walk retries cannot manufacture an answer |

A singleton target is not generally its block's pre/post state. The ordinary,
anchor and fresh-shard cases remain distinct. The refuted shortcut in Law 85 has
not been restored.

## Publication and restart

Successful verification returns `VerifiedRecovery`, whose metadata is constructed
only by replay. Publication accepts an empty destination or a matching verified
prefix; it refuses incompatible existing metadata. State pages and raw blocks
may already be on disk, but they are not evidence of an accepted DAG.

The approved store's `RECOVERY_PENDING_KEY` is written/read back before DAG
publication. A restart seeing this marker re-enters LFS even with a populated
DAG. Completion requires writing and reading back the accepted fringe, deleting
the pending marker and verifying its absence. An error remains a failed sync
attempt and cannot notify `finished`. A crash between these steps conservatively
leaves recovery pending. The actual active-bonds read is exercised before
publication; the launch-time validity/allocation guards still run before Running.

Store format **2** records these semantics. The node stamps an older format before
opening it under the new rules; a format-1 binary refuses a format-2 directory.
Use a retained copy for rollback. This is not a claim of atomic multi-environment
publication or completed hardware power-loss qualification. Existing non-pending
populated stores are not retroactively replay-certified by changing the marker.

## Evidence and limits

`casper/tests/recovery_replay.rs` constructs a signed history with real epoch
transitions, replays it through a separate runtime with empty sidecars, checks
forged metadata/target refusal, missing-history refusal, pin refusal and anchor
semantics, then validates new work on the restored DAG. Node LFS restore tests
exercise the actual transport; `node/tests/pinned_genesis.rs` covers both matching/
mismatching restart pins and a pending marker over an existing populated DAG.
Checkpoint tests inject write/read-back/delete faults.

The added replay is O(history) and uses a staging DAG; it is not a succinct state
proof or a scale result. Law 83's transfer theorem and Law 85's root relation
remain unchanged; no new machine proof of the Rust replay/publication protocol is
claimed. C339's exact-source acceptance is recorded in the findings register;
previous passing live measurements from `f3eef292` do not certify later code.

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
- It does not certify a release or retroactively certify a legacy populated store.
- It does not claim the bootstrap is honest about liveness: a peer that will not serve blocks fails
  the walk after the give-up rule, which is a bounded failure, not a restored state.

---

**Operating one:** [Running a node: testnet now, mainnet later](../node/validator-observer-setup.md).
