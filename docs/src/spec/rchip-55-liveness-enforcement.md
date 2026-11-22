# RCHIP #55 — Liveness enforcement: jail first, a vote for the bond

**Status:** accepted maintainer decision, 2026-10-29. Implemented in this release behind new genesis
parameters, **default off**. Arming requires a new genesis. This RCHIP does **not** authorise mainnet
arming; that waits on the live evidence in §7 and #428.

**Owner:** @PatrickMockridge. The separate recovery and validator-lifecycle questions are tracked in #70/#39; reviewing this RCHIP does not transfer ownership of those topics. RCHIP #46 (rotation/longevity/open enrollment) remains a separate, deferred economics row and is cross-referenced, not reused.

## 1. The problem

A bonded validator that stops keeps its bond, counts in the whole-bonded finality denominator for
ever, and under the shipped `absence-slack = 0` is still paid for an epoch it was drawn for. A
sub-quorum outage therefore freezes finality while the absent stake makes the quorum unreachable.
`docs/src/node/validator-economics.md` and `TERMS.md` state the asymmetry; see #70/#39 for the separate liveness and validator-lifecycle discussion.

## 2. The decision

The response has three steps, and they are deliberately different:

0. **Inactivity leak (automatic, priced; added 2026-11-22).** At an epoch boundary, a validator the
   finality gate names as not covering the whole partition — `NoAdvance::Support.not_full`, the
   refusal Law 52 makes attributable; `Coverage.missing` is absence by height and stays the jail's
   business — is counted silent. After
   `N` consecutive boundaries of that silence (the same window as the jail) it is **retired** from the
   pool and the active snapshot and its bond is forfeited to the Coop vault. This is the priced
   response to the *coverage* silence that blocks finality: a layer at 100/100/50/50 whose support is
   exactly 200 of 300 — two thirds, one stake short of the strict threshold — refuses for ever
   otherwise, and removing the sender that does not see the partition is what lets the survivors'
   support become the whole denominator. The input is the gate's own refusal, derived by every node
   from the block's justification set and not carried on the wire, so a proposer cannot choose whom
   to leak.
1. **Jail (automatic, bond-preserving).** After a validator has been absent across `N` consecutive
   **advancing** epoch boundaries, it is removed from the active set for `J` boundaries and paid
   nothing. Its bond and its delegators' principal are untouched. A message releases it when the term
   expires; continued silence renews the term.
2. **Vote-gated slash (explicit).** A slash vote opens **automatically** on a jail. Any active
   validator may cast a stake-weighted ballot (`voteSlash`/`unvoteSlash`). At an advancing boundary,
   strictly more than two thirds of the active stake in favour slashes the target at the configured
   tier and **permanently ejects** (bans) it.

A stalled chain has no advancing boundary, so the jail and vote clocks do not move. A partition can
jail the minority on the side that still finalises; the jail's bond is only at risk if that side's
supermajority votes for it. The leak is the one **automatic** path to a bond, and its trigger is not
absence but a message that does not cover the partition — a condition the validator's own production
can end at any boundary, which is what keeps it a statement about the current condition rather than a
latch on history.

## 3. What this is not

- **Not a bond burn for absence.** The leak is triggered by the finality gate's coverage refusal, not
  by a height lag; a validator that is merely behind is jailed (step 1) with its bond intact. The
  2026-10-02 decision to refuse a leak (`docs/src/formal/progress.md`) is superseded on 2026-11-22 by
  the bond-leave measurement: the exact-two-thirds layer could not be repaired by any predicate,
  window or round-clock change, and retirement was chosen and modelled as law 89's leak clause.
- **Not the C111 single-actor eviction.** `untrust` remains admission-only; the leak's input is the
  gate's own derivation and a vote needs >2/3 of the active stake.
- **Not a proposer-only slash.** Participation is the receiver-derived finalised fringe and
  `layer_missing` is derived from the same merge, not a proposer-chosen field.
- **Not slashing a stuck-but-present node.** The trigger is the partition-coverage refusal; #485's
  present-but-not-finalising defect is separate.

## 4. Parameters (genesis, default 0 = off)

| key | meaning | testnet example |
|---|---|---|
| `liveness-absence-threshold` | heights behind the quorum-supported tip that count as absent | `epoch-length` (10000) |
| `liveness-jail-after-boundaries` | consecutive advancing boundaries absent before jail; `0` disables | `2` |
| `liveness-jail-epochs` | advancing boundaries a jail lasts | `5` |
| `liveness-vote-window` | advancing boundaries a vote stays open; `0` disables voting | `3` |
| `liveness-slash-severity` | `none` / `malicious` / `misdemeanour` / `honest-mistake` | `misdemeanour` |

Validation is at genesis: a half-configured rule is refused (`PosParams::validate`), and the vote
window must fit inside the withdrawal quarantine so a bond cannot be paid before the tally. A zeroed
config encodes the pre-change format-2 `PosParams` record byte-for-byte and writes no new leaves, so
an old chain replays to the same hashes.

The inactivity leak adds **no parameter**: its window is `liveness-jail-after-boundaries`, and it is
active exactly when the mechanism is armed. That is deliberate — one bound for "sustained silence",
applied to whichever kind of silence the validator shows (absence by height → jail; a message that
does not cover the partition → leak).

## 5. Consensus surface

New `pos` leaves: `pos:liveness`, `pos:liveness-anchor`, `pos:vote-windows`, `pos:slash-votes`,
`pos:banished`, and (from 2026-11-22) `pos:silence` for the leak's counters. New `rho:rchain:pos`
methods: `voteSlash`, `unvoteSlash`, `getLiveness`, `getVotes`. The leak's `layer_missing` is a
`CloseBlock` input derived from the gate's refusal on both play and replay — it is not published and
not carried in the system-deploy data, the same discipline `participation` follows.
The tally and slash are a **boundary state transition**, not a block-level `Slash` system deploy:
`validate::slashed_validators` does not list it and `slash_is_unjustified` does not apply; its
authority is the on-chain tally every node replays. This is hard-fork class (#51 category A).

**A no-op rewrite is not a native write (Law 17e).** The prelude rewrites its leaves at every
advancing boundary, and the native store records the write even when the bytes are the pre-state's.
Counting that as a slot a chain wrote made a finalised boundary chain a conflict partner of a
concurrent `voteSlash` chain, which `incompatible_with_final` then rejected — the ballot died though
nothing changed. Chain indices are now built from the **effective** writes (those whose value changed,
walked in deploy order), at index construction rather than in the store so stored and regenerated
sidecars agree. This is also hard-fork class: two binaries can reject different chains at a merge.

## 6. Safety properties (Law 89)

Jail never writes the pool; one lapse is not sustained absence; a stalled or off-boundary step changes
nothing; a vote is admitted only above strict 2/3; a passing vote slashes and ejects; a returning
validator is released; continued silence renews. A passing vote and a release each close the target's
window and clear its ballots, and a banished validator cannot rebond. **The leak (law 89's leak
clause)** retires
only the sender the gate named, never raises the denominator, and clears its counter on the first
boundary whose layer covers the partition; the play/replay seam test pins that the derived refusal
reaches both paths. The Lean model is `spec/Rchain/Liveness.lean`, the register rows 89 and 129 (with
row 17e for the merge rule), the HAZOP U21 and safety-case B48/B49/H-RS-17. The
behaviour-by-behaviour coverage table, including what is not covered, is
`spec/audit/evidence/c344-law-coverage-20261116.md`.

## 7. Acceptance (met 2026-11-17)

A four-validator devnet and then a live testnet must show: jail after two advancing boundaries with the
bond unchanged; a >2/3 vote slash and permanent ejection with replay agreement; a partition that jails
but does not slash without a vote; a stall that moves neither clock; a restart during jail that returns
at expiry and renews while silent; the exact 2/3 boundary.

**The harness exists and its reads are finalised-state reads.** `tools/liveness-enforcement-drill.py`
stages the devnet with the five genesis keys in one shared `network.conf`, stops a validator and reads
`getLiveness`/`getBonds`/`getVotes`/`getActiveValidators` through `rho:rchain:pos`. Every read is
pinned to one `/api/last-finalized-block` hash through
`/api/v1/explore-deploy-by-block-hash`, and the anchor is recorded with the observation; the
exploratory reply must come from the term's own `new`-bound name, because `@"out"` accumulates every
deploy's output. Each vote deploy is followed from its id through `/api/v1/deploy-status` to its
block, the block is waited to finality, and the ballot is read in that block and again at a finalised
anchor.

**The exact-threshold failure is explained and fixed at unit level (2026-11-16).** The arm lost
validator 1's ballot with the target still jailed. The cause was Law 17e's absence: a finalised
boundary's unchanged rewrite of `pos:slash-votes` forced the concurrent vote chain out of the merge.
`casper/src/merging.rs::a_finalized_boundary_noop_does_not_reject_a_concurrent_ballot` reproduced the
rejection before the fix (the deploy id in `rejected_deploys`, the slot in `dropped_native_slots`) and
keeps both ballots after it.

**The capstone ran on 2026-11-17 and C344 is closed.** `jail-vote`, `exact-threshold` and `stall`
pass on a locally operated four-validator devnet (the 900 denominator, 600/900 exactly two thirds
with no slash, 800/900 strict with the slash, and a frozen finality fence below quorum). The
restart/expiry clause is the rule's state machine: a jailed validator does not propose; its node may
send a funded deploy that an unjailed validator proposes; and `releaseIfPresent` clears the jail at
expiry, with renewal while silent. Law 89 and its named unit witnesses
(`a_jailed_validator_returns_after_the_term`, `a_still_absent_validator_is_renewed`) are the
acceptance for that clause. The arm ran and its jail half read as expected; its restart/release half needs a new test (the
local build refused an in-place restart and a cold rejoin stalled), recorded in
`spec/audit/evidence/c344-capstone-20261117.md`.

## 8. Known residuals

A >2/3 active stake can eject a dissenter after a jail (fixed policy, no unban); a minority partition
can be jailed on the finalising side; a proposer can present a stale reachable fringe; **nothing
requires a boundary block to carry `CloseBlock`**, so the epoch transition (and this rule) is
skippable until a validity rule lands. The accepted discharge of the live-testnet clause is a locally
operated four-validator devnet; public-testnet and mainnet arming remain external under **#428**.

## 9. Voting-weight consistency at an epoch boundary

The active-set stake snapshot, not the mutable bonded pool, supplies **both** the numerator and denominator for liveness slash votes. Delegation changes a pool entry immediately and its active weight only on the next epoch draw. Using the pool for one side of the inequality allows an active minority to acquire an artificial supermajority. The regression cases in `rholang/src/native_state.rs` exercise a real mid-epoch delegation and the strict two-thirds boundary.

This safeguard does not restore finality once bonded participation is below the chain's finality quorum: jail and slash voting both require advancing boundaries. That recovery case remains a separate acceptance obligation.
