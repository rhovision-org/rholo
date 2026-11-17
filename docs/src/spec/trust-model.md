# What a joining node trusts

A node with no state does **not** restore a prefix by replaying it. It moves from a trust anchor to
the tip over authenticated data, and it runs from the state that anchor names. "Trustless" is a word
that hides which inputs are checked; this page states the model precisely. The machine forms are
Laws 104, 105, 108 and 109 (`spec/Rchain/Sync/`); Law 83's content-transfer theorem remains, and
the finality residual it enumerated is **closed on the checkpoint path** by Laws 105 and 108.

## The anchor

A joiner has exactly one of two anchors:

- the operator's **pinned genesis** — height zero, the pinned block hash and post-state, with the
  genesis state pages hydrated; or
- a **verified checkpoint certificate** — a statement signed by strictly more than two thirds of
  the genesis **bonded pool**, carrying the anchor, its post-state, the fringe, the epoch and the
  optional `metadataRoot` and `historyRoot` commitments.

The bond set that verifies the certificate is the genesis bonded pool (`pos:bonds` at genesis), not
the epoch's active draw (`pos:active`): the draw is a subset that excludes the jailed and honours the
active cap, so treating it as the anchor made a jail look like a partition change. A certificate
whose anchor state's **pool** has drifted is refused; an active-set difference is not a drift
(C403). A node with neither anchor does not run.

## What the joiner checks

| Input | Admission check |
|---|---|
| Genesis | Exactly one height-zero block, content hash, and the configured genesis pin when present |
| Certificate | Statement shape; every signer is a bonded key; signatures verify over the domain-tagged digest; the signers hold strictly more than two thirds of the bonded stake; the bond set has not drifted |
| `metadataRoot` | The walked ancestor header set recomputes to the committed root; each header's **fringe membership and fringe state root** are part of the projection, and the set's shape is checked (one parentless genesis, older fringe members, non-zero non-genesis fringe roots); a missing or mismatched root is refused |
| `historyRoot` | Every header within `VALIDATION_WINDOW_HEIGHTS = 14` heights of the anchor contributes its pre/post/fringe roots; the set recomputes to the committed root, the joiner hydrates exactly that set and never widens it |
| Signed fringe state | The statement's `fringeState` equals the anchor header's committed `fringe_state_hash`: the merge base a post-anchor block's merge starts from; a v2 statement that names a different root is refused |
| Checkpoint post-state | The anchor's post-state is an audit field; the node **runs from the signed fringe state**, and an accepted state cannot carry a root its anchor does not name |
| Genesis body | The one body this path downloads: re-hashed, and its fields checked against the walked genesis header (empty fringe, fringe state = pre-state) |
| Ancestry shape | The walked closure is contiguous and each header's justifications are present |
| Post-anchor block | Its pre-state must be the anchor root, a certified window root, or a root this node derived from a validated post-anchor block; the state store's contents are not permission |
| Boot | The stored certificate is re-verified and its window rebuilt from the DAG closure; a populated DAG above genesis with no verified certificate is refused |

## What the joiner does not check

- **That the quorum computed the state correctly.** The certificate is a quorum claim about the
  state, not a verified execution: a quorum that all computed the same wrong root still certifies
  it. The audit is the counterweight — `rnode audit` replays the bodies from genesis and reports
  `AGREE`, `DISAGREE` or `INCOMPLETE`. `INCOMPLETE` is not agreement.
- **Liveness.** A peer that will not serve the certificate, headers, window or genesis body fails
  the walk after its bounded retry budget. That is a bounded refusal, not a restored state.
- **Retention below the anchor.** Bodies are a service; a joining node does not need them, and an
  archive's possession is checked by challenge, not by the join.

The per-block `fringe_state_hash` residual that Law 83 enumerated for the removed transfer path is
closed for the current join: each non-genesis header's fringe claim is in the `metadataRoot`
projection, and the genesis header is separately checked against its body. The joiner refuses a
missing or mismatched commitment rather than trusting a peer's per-block claim.

## Publication and restart

The accepted state is carried into the running phase by construction; `CURRENT_ROOT` is the
anchor's root, not a value recovered from a DAG tip. A failed sync leaves the node Syncing and
retries a bounded number of times; when the budget is spent it stops rather than serving a state it
cannot attribute.

The checkpoint store separates evidence from service: `record` keeps digest-keyed certificates and
never moves the head, `adopt` moves the serving head monotonically. A restart re-verifies the stored
certificate before Running. A populated store with no verified certificate is refused and must be
re-synced from a checkpoint; its old history is then audited separately.

## The bonding trust axis

Admission and consensus membership are different lists (Law 82, `spec/Rchain/Casper/Lifecycle.lean`):

- `trust`/`untrust` change `pos:trusted`, and the caller must be a trusted key. The caller is bound
  to the unforgeable `deployerId`, so a program-authored key cannot act for another validator.
- `bond` is self-only and requires the bonder to be trusted; it moves the stake into the pool and
  **does not** activate — activation is the epoch boundary's re-selection.
- `untrust` **does not confiscate** (AUDIT C111): a bond is property, admission is not.
- A slash needs evidence; it is the only transition that confiscates, and it is graded.

The genesis trust set is the anchor for admission and for certificate verification; the epoch
boundary is the only place membership moves.

## What this page does not say

- It does not certify a join. The in-process, restore-matrix and devnet arms are finite witnesses;
  a target network's own join remains the observation that counts.
- It does not certify a release or retroactively certify a legacy populated store.
- It does not claim the bootstrap is honest about liveness; a peer that will not serve fails the
  walk after the give-up rule.
- It does not verify an archive's history; that is the challenge and the audit's job.

---

**Operating one:** [Sync operations](../node/sync-operations.md) and
[What a joining node trusts in practice](../node/validator-observer-setup.md).
