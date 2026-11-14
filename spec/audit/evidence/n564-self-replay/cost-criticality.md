# Is a recorded-cost mismatch a block-validity rule?

**The decision this PR does not take.** The replay refuses a block when the replayed per-deploy
evaluation cost differs from `ProcessedDeploy.cost` (`casper/src/runtime_replay.rs:489-497`). The
post-state hash is not compared on that path. The port follows the Scala oracle (`ReplayFailure
.ReplayCostMismatch` is a validation failure), so keeping the refusal is the faithful choice. But the
determinism page says the recorded cost field is not part of determinism, and the incident shows the
consequence: a node can refuse a block whose state is correct, not broadcast it, and freeze finality.

**Why the cost is consensus-carried anyway.** `ProcessedDeploy.cost` is in the block and covered by the
block hash. On replay, the refund and executor payment are derived from the **recorded** cost
(`casper_message.rs:700-726`; `runtime_replay.rs:414,433`), so a proposer that under- or over-reports a
cost is making a false accounting claim even when the post-state (built from the same record) matches.
The check protects the phlo economy, not the state hash. The two verdicts answer different questions.

**Options.**

| id | rule | consequence | cost |
|---|---|---|---|
| **A** | **Keep the refusal** (current, oracle-faithful). On a cost mismatch the block is invalid; the proposer must not broadcast. | A state-correct block with a wrong cost claim is refused. A node that cannot reproduce the claim is stranded until another proposer advances the chain. | The liveness fallback (recover-pool, independent pool, fault halt) is mandatory. No consensus change. |
| **B** | **Accept on state-match**: if the post-state hash matches, accept the block and treat the cost mismatch as an accounting anomaly. | The chain keeps moving on a state-correct block; the phlo accounting is wrong and the anomaly must be attributable/slashable or corrected. | A hard fork: every validator's acceptance rule changes; a peer that refuses while this node accepts is a fragment (TE-2). |
| **C** | **Make the cost replayable and keep the refusal**: the proposer records only the canonical cost (the R1 play-mode fix), and the replay's cost check is paired with a state verdict for diagnosis. | The condition becomes much rarer (no non-canonical mode can record a cost), and the next occurrence is diagnosable. The oracle rule is unchanged. | No consensus change; the fallback is still needed for a true sequential disagreement. |

**What this PR implements.** C's verification half: canonical play cost, the state verdict beside the
cost verdict, the trace/cost census, the durable refused body, the bounded retry, the fault halt and the
recover-pool fallback. **A** is the acceptance rule (unchanged). **B** is recorded as the open decision;
it is not encoded.

**What would justify B.** Evidence that the cost claim can differ while the state is correct *and* that
the difference is not attributable to a faulty proposer (e.g., a play-mode asymmetry that cannot be made
canonical), plus a protocol decision on attribution (who pays for the wrong accounting) and a hard-fork
plan. The matrix and the next live occurrence's `stateHashMatched` are the inputs.

**Who decides.** The protocol owner (the oracle's rule is the default). The PR's job is to make the
condition visible, contain it, and leave the node/network a safe fallback; it is not to change
consensus.

## Decision (2026-11-14) — A, keep the refusal

The protocol owner's decision is **A — keep the refusal**. A block whose replayed per-deploy cost
differs from `ProcessedDeploy.cost` remains invalid even when its post-state hash matches: the
recorded cost is consensus-carried and drives the refund and executor payment, so a wrong cost is a
false accounting claim rather than a state disagreement. The port follows the Scala oracle. **Option B
is not adopted**: no acceptance rule changes, no hard fork and no lockstep rollout is required, and a
node that accepts while a peer refuses would be the fragment the option warned about.

Option C's verification half remains and is what makes the decision safe: canonical play cost, the
state verdict beside the cost verdict, the trace/cost census, the durable refused body, the bounded
retry, the fault halt and the `recover-pool` fallback. The trace-mask test
`casper/tests/determinism.rs::a_cost_refusal_masks_the_trace_check_and_the_shadow_reports_it` pins the
one diagnostic consequence the decision accepts: the enforcing replay reports only
`ReplayCostMismatch`, while the cost-ignoring shadow reaches `check_replay_data_with_fix` and reports
the `UnusedCommEvent` the enforcing path masks. This closes C358; it does not recover the original
incident body or establish RC-B/RC1, which remain C357's.
