# RCA — the bond-leave finality stall (2026-11-22)

**Status:** root cause identified; attribution and the Lean law land in one PR, the clock correction in
the next. This is a liveness defect at a validator-set change, found by the the
`rc-bond-leave` campaign — not by a test that only measured heights.

## Symptom

A three-validator net (stakes 100,100,50), epoch 10, `--no-autopropose --propose-on-deploy`, with a
newcomer bonding 50 (total 300). After the newcomer's bond activates:

```
finality did not advance at tip 33: a layer exists but its supporting stake is not a supermajority
— 200 of 300 (3 full partition(s) among 4 candidate(s) of a 4-sender partition)
finality did not advance at tip 38: ... — 0 of 300 (0 full partition(s) among 3 candidate(s) of a
4-sender partition)
```

Finality froze at 35; the tip ran to 67; the proposer then refused at exactly 32 heights past the
fence (`proposalHalted`, `ready=false`). The same rig passed on 2026-10-04 (`9a75d45`) — measured on
heights and deploy status only; the support denominator was never observed.

## Root cause

1. **Arithmetic.** `calculate_fringe_numbers` sums the full-partition stake against the whole bonded
   map (`block-storage/src/dag/finalizer.rs:273-301`). `is_super_majority` is
   `3·support > 2·total` (`sdk/src/consensus.rs:19-21`); with 300 bonded, the threshold is 201. A
   layer with three of four senders full credits 100+50+50 = **200, exactly two thirds**, and is
   refused. The non-full sender is one of the 100-stake originals.
2. **Trigger.** The newcomer's first bonded block is produced on a lagging view; a concurrently
   produced 100-stake original's block does not see it, so `sees_the_whole_partition`
   (`finalizer.rs:471-476`) is false for that sender's candidate.
3. **Regression.** `b9324947f` (2026-10-21) moved the liveness-window tip to
   `quorum_supported_height` (`block-storage/src/dag/liveness.rs:199`) and made `advance_round`
   close on sender-local progress against that lagging clock (`message_state.rs:117-144`). Its own
   comment admits the boundary can stay frozen until quorum-backed progress resumes. The same shape
   froze a healthy net on a six-height skew (`565c8dec6`, removed in `5524f2b25` for the proposer
   gate). The n220 pass predates the change.
4. **Round freeze.** The stale `round_parents` then produce the lockstep shape — three candidates of
   a four-sender partition, 0 of 300 — that the finalizer cannot advance on.
5. **Attribution gap (why it was silent).** `NoAdvance::Support` had no not-full set; the trace took
   `full_partitions`/`candidates` from the last *advance* (`None` in a stall), recording `0/0`;
   `/health` published a thresholded boolean (2 × partition refusals) and a counter, the reason was
   operator-only, `/api/v1/finality-observation` carried no reason or numbers, and `ready` has no
   finality term. The stall read as "healthy" for eight refusals and was never attributable.

## What the fix does

- **Attribution (PR A).** The support refusal carries `not_full`; the trace reads the refusal's own
  counts and names the senders; `/health` and `/api/v1/finality-observation` publish an immediate
  `finalityGate` (reason, supporting/total, full partitions, candidates, partition, not-full and
  missing counts); the evaluator's `attributable_ok` makes a reasonless refusal not safe. Lean law 52
  is amended: a safe refusal **must** carry a `NoAdvance`, `supporting` is the gate's
  `fullPartitionStake`, and the denominator is the active snapshot's bond total.
- **Clock (PR B).** Only `advance_round`'s retirement comparisons move from `supported_tip` to the
  max observed height; the finality live window stays quorum-clocked and `round_height` stays the
  quorum height, so the N=8 outlier protection (`b9324947f`'s intent) is preserved. Regression tests
  reproduce the 200/300 then 0/300 shape under the old clock and advance under the new one.

## Evidence

- Campaign: `tools/bond-leave-campaign.py` (this tree), result under
  `target/bond-leave-campaign/`; the join and withdrawal mechanics work, the payout probe fails
  because the state is unfinalised.
- Log: the two `finality did not advance` lines above, plus the proposer's
  `refusing to propose ... (bound 32)` and `[pos] close_block` / `participation lag` lines.
- Prior pass: `spec/audit/evidence/n220-join-results.md` (`9a75d45`, J4a at finalised 20) — heights
  only.
- N=8 protection: `block-storage/src/dag/message_state.rs:818-869`
  (`fast_proposer_cannot_retire_an_online_quorum_or_advance_round_alone`) and the current N=8
  campaign run `45022600421` (pass).

## Non-claims

- Lean proves every refusal is attributable and the denominator is the boundary snapshot; it does
  **not** prove liveness. "A live quorum produces a full layer in bounded rounds" remains Law 58's
  named schedule hypothesis, measured by the campaign.
- No denominator, `LIVENESS_WINDOW`, `stall_threshold`, timeout or `MAX_PROPOSAL_FINALITY_LAG`
  change.
- The 200/300 layer is correctly refused; the fix is to stop the round freeze that keeps producing
  it, not to lower the threshold.
