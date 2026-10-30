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

The response has two steps, and they are deliberately different:

1. **Jail (automatic, bond-preserving).** After a validator has been absent across `N` consecutive
   **advancing** epoch boundaries, it is removed from the active set for `J` boundaries and paid
   nothing. Its bond and its delegators' principal are untouched. A message releases it when the term
   expires; continued silence renews the term.
2. **Vote-gated slash (explicit).** A slash vote opens **automatically** on a jail. Any active
   validator may cast a stake-weighted ballot (`voteSlash`/`unvoteSlash`). At an advancing boundary,
   strictly more than two thirds of the active stake in favour slashes the target at the configured
   tier and **permanently ejects** (bans) it. No other path reaches the bond through liveness.

A stalled chain has no advancing boundary, so neither clock moves. A partition can jail the minority
on the side that still finalises; the bond is only at risk if that side's supermajority votes for it.

## 3. What this is not

- **Not an inactivity leak.** No automatic step burns a bond. The 2026-10-02 decision
  (`docs/src/formal/progress.md`) that a leak is refused is retained for the leak and superseded only
  for this mechanism.
- **Not the C111 single-actor eviction.** `untrust` remains admission-only; a vote needs >2/3 of the
  active stake.
- **Not a proposer-only slash.** Participation is the receiver-derived finalised fringe, not a
  proposer-chosen field.
- **Not slashing a stuck-but-present node.** The trigger is absence from the finalised fringe; #485's
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

## 5. Consensus surface

New `pos` leaves: `pos:liveness`, `pos:liveness-anchor`, `pos:vote-windows`, `pos:slash-votes`,
`pos:banished`. New `rho:rchain:pos` methods: `voteSlash`, `unvoteSlash`, `getLiveness`, `getVotes`.
The tally and slash are a **boundary state transition**, not a block-level `Slash` system deploy:
`validate::slashed_validators` does not list it and `slash_is_unjustified` does not apply; its
authority is the on-chain tally every node replays. This is hard-fork class (#51 category A).

## 6. Safety properties (Law 89)

Jail never writes the pool; one lapse is not sustained absence; a stalled or off-boundary step changes
nothing; a vote is admitted only above strict 2/3; a passing vote slashes and ejects; a returning
validator is released; continued silence renews. The Lean model is `spec/Rchain/Liveness.lean`, the
register row 89, the HAZOP U21 and safety-case B48/B49/H-RS-17.

## 7. Acceptance (not yet met)

A four-validator devnet and then a live testnet must show: jail after two advancing boundaries with the
bond unchanged; a >2/3 vote slash and permanent ejection with replay agreement; a partition that jails
but does not slash without a vote; a stall that moves neither clock; a restart during jail that returns
at expiry and renews while silent; the exact 2/3 boundary. **The devnet harness and evidence
contract are still owed.** The proposed `tests/liveness-enforcement.py` harness does not exist in
this branch; acceptance evidence under `spec/audit/evidence/` and the live testnet arm remain
unverified. No live arm was run.

## 8. Known residuals

A >2/3 active stake can eject a dissenter after a jail (fixed policy, no unban); a minority partition
can be jailed on the finalising side; a proposer can present a stale reachable fringe; **nothing
requires a boundary block to carry `CloseBlock`**, so the epoch transition (and this rule) is
skippable until a validity rule lands; no live evidence yet.

## 9. Voting-weight consistency at an epoch boundary

The active-set stake snapshot, not the mutable bonded pool, supplies **both** the numerator and denominator for liveness slash votes. Delegation changes a pool entry immediately and its active weight only on the next epoch draw. Using the pool for one side of the inequality allows an active minority to acquire an artificial supermajority. The regression cases in `rholang/src/native_state.rs` exercise a real mid-epoch delegation and the strict two-thirds boundary.

This safeguard does not restore finality once bonded participation is below the chain's finality quorum: jail and slash voting both require advancing boundaries. That recovery case remains a separate acceptance obligation.
