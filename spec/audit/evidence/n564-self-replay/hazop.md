# #564 — HAZOP study of the self-replay cost refusal

**Reviewed revisions.** `fix/564-self-replay-classification` (this branch) and the incident binary
`0a1e02362`. The machine-checked rows are **U22** in `docs/src/spec/failure-hazop.md`; this is the
companion study in the OCapN format (`spec/audit/evidence/ocapn-hazop.md`), with the measured/reasoned
distinction, severity, likelihood, the steelman and the fault tree.

**Why this study exists.** The first #565 unit classified and bounded a self-replay refusal; it did not
establish how the failure is represented under what conditions, or whether the protocol's refusal was
a state verdict. This study is that pass. It is adversarial to the fix it accompanies: the central
finding is that the replay decides the recorded cost **before** it compares the declared post-state, so
a state-correct block can be refused on an accounting field and the operator is never told which case
it was.

## §1 Hazard inventory

| id | hazard | sharpest fact | how known |
|---|---|---|---|
| **H1** | The cost verdict precedes the state verdict | `replay_deploy_eval` returns `ReplayCostMismatch` (`casper/src/runtime_replay.rs:489-497`) before `handle_errors` compares the hash (`interpreter_util.rs:127-142`); the trace check runs after (`runtime_replay.rs:333`). Same order at `0a1e02362`. | read |
| **H2** | Two play modes can record a non-canonical cost | `RelaxedValidated` accepts on state/COMM/rig but not per-deploy cost and returns the relaxed results (`runtime_manager.rs:1944-2041`); batching with `_VERIFY=0` skips the only cost comparison (`:1832-1838`) | read |
| **H3** | A trace/refund difference is masked by the cost return | an unused COMM or fewer COMM/refund steps surfaces as cost and never `UnusedCommEvent`; `check_replay_data_with_fix` swallows it for a failed deploy (RCHAIN-3505) | read; the matrix measures it |
| **H4** | The refusal destroys its own evidence | `put_block` runs only after validation succeeds, so a refused self-built block leaves no body; the incident's #1115 is unreconstructible | read; `incident/unavailable.tsv` |
| **H5** | The exclusion is unbounded and process-local | `excluded` only extends; no prune/cap; the tracker dies with the process | read |
| **H6** | Excluding the whole pool at a frozen height idles the proposer | no pooled deploy -> no block; expiry is height-keyed; with autopropose off nothing advances | read |
| **H7** | The public `/health` projection can drift from the operator DTO | `/health` hand-builds 4 keys; the DTO has 10; nothing checks the public projection | read |
| **H8** | The synthetic determinism test is cited as the RCA | it runs `dev`, a 3-deploy shape, no cost trace and no cost magnitude; it passes | read |
| **H9** | The incident terms, state and unit environment are unmeasured | the six terms and A's store were not captured; the batching/scheduler environment is unknown | reported/unavailable |

## §2 Hypotheses and what would confirm them

| # | hypothesis | evidence that would confirm it | current verdict |
|---|---|---|---|
| R1 | A non-canonical play mode recorded the cost | a relaxed/batched run whose per-deploy cost differs from the sequential oracle, on the incident binary | representable (H2); the R1 fixes remove it |
| R2 | A replay trace/refund difference moved the cost on a state-neutral deploy | the trace census and the shadow on a failed/unused-COMM fixture | open; leading hypothesis |
| R3 | Default Sequential play/replay disagrees | a 2-deploy transfer+Nil matrix cell on `0a1e02362` with a cost delta | open |
| R4 | The block was state-correct and refused on cost alone | the shadow's `state_hash_matched` on a future occurrence | mechanism confirmed; incident unestablished |
| R5 | The unit environment decides which of R1–R3 is reachable | `systemctl show -p Environment,ExecStart rnode` | unmeasured |

## §3 Remedies

**Tier 1 (R1, this PR).** Canonical recorded cost: `RelaxedValidated` requires per-deploy cost and
status equality with the oracle; an enabled batch always verifies; the node warns when the unsafe flag
is requested. The refusal's selection is keyed by the block it produced.

**Tier 2 (R2, this PR).** The cost/trace census, the bounded shadow replay with `state_hash_matched`,
the durable refused record and body, prune/cap and a local fault halt at a frozen height. The protocol
decision on accepting a state-correct/cost-mismatched block is recorded, not encoded
(`cost-criticality.md`).

**Tier 3 (R4, this PR or owed).** `recover-pool`, the signed pool fields, the runbook row ("route one
deploy to an independent-pool validator; capture the pool before expiry; restart is not a remedy"),
and the devnet fault-injection arm.

**Not proposed.** Removing the RCHAIN-3505 swallow or accepting on a state match: both are protocol
decisions. A test hook that perturbs the replay's cost in production is refused; the shadow uses the
real replay with the cost check ignored.

## §4 Worksheet

The machine-checked rows are U22 (`F-U22-01…12`). Their measured/reasoned status:

| row | word | known | severity | likelihood | steelman |
|---|---|---|---|---|---|
| F-U22-01 | NO | read + cost-only tamper test | S1 (chain halt when this validator is the producer) | L-D (needs a cost-side disagreement) | fails: no safeguard checked the state |
| F-U22-02 | EARLY | measured (trace-mask test) | S2 | L-D | fails: the trace check is unreachable behind the cost return; `a_cost_refusal_masks_the_trace_check_and_the_shadow_reports_it` shows the enforcing path reports only `ReplayCostMismatch` and the cost-ignoring shadow reports the unused COMM |
| F-U22-03 | AS WELL AS | measured (shadow test) | S2 | L-D | fixed: the shadow computes the verdict |
| F-U22-04 | PART OF | read | S2 | L-D | fails as a protocol reading; the decision is recorded |
| F-U22-05 | OTHER THAN | measured (classification test) | S2 | L-D | fixed by Law 92 |
| F-U22-06 | MORE | measured (bounded test) | S1 | L-D | fixed: the set is held out after three |
| F-U22-07 | LESS | measured (census tests) | S3 | L-D | fixed: the categories and trace are visible |
| F-U22-08 | LATE | read; halt owed | S2 | L-C | open until the fault halt |
| F-U22-09 | BEFORE | read; durable body owed | S2 | L-C | open until the durable record |
| F-U22-10 | AFTER | read; prune/cap owed | S3 | L-C | open until the prune/cap |
| F-U22-11 | REVERSE | decided (A — keep the refusal) | S1 | L-D | closed by the decision record in `cost-criticality.md`; no hard fork |
| F-U22-12 | REVERSE | read; peer test owed | S2 | L-D | the runbook residual |

## §5 Table B — barriers that held

| attack / condition | defence |
|---|---|
| an inbound block reaching `SelfReplayFailed` | `validate` is the `Peer` wrapper; the variant is only constructed by `SelfBuilt`; the block processor treats it as a terminal wiring fault |
| a self-built status/COMM failure becoming `Divergence` | `ValidationOrigin` + `refusal_for`; peer path unchanged |
| a batched cost with verification disabled | `canonical_batch_policy` forces verification; the node warns |
| a relaxed run's non-canonical cost | the cost/status equality in `accepted`; the oracle-result arm |
| a cost-only tamper reaching the state comparison | the state is never compared on that path; the shadow now reports it |
| a second block overwriting the selection association | the selection is keyed by height, sender and pre-state |

## §6 Fault tree

```
                 a self-replay cost refusal at a frozen height
                                  │
       ┌──────────────────────────┼──────────────────────────────┐
       │                          │                              │
  H1 cost-before-state      H3 trace/refund mask            H4 no evidence
  (S1, R2)                  (S2, R2)                        (S2, R2)
       │                          │                              │
  the state verdict is      unused COMM becomes            the refused body
  never computed; a         cost; RCHAIN-3505               is never stored;
  state-correct block       swallows it for a               the incident is
  is refused                failed deploy                   unreconstructible
       │                          │                              │
  H6 exclusion empties     H2 play-mode cost                H5/H8 the retry
  the pool at a frozen     (S2, R1 fixed)                   and the register
  height (R2/R4)                                              (R2)
```

Top events: **TE-5** (node wedge) is the primary; **TE-1** (chain halt) when the affected validator
is the producer or a quorum member; **TE-2** (fragment) if a state-match acceptance is ever decided
without peers; **TE-4** bounded by the exclusions cap and the census; **TE-6** the residual if a
state-correct refusal becomes a divergent head. TE-3 is not reached.

## §7 Verdict

The liveness barrier is real: the classification, the bound, the canonical play cost and the evidence
are improvements, and the HAZOP gate covers every guide word. But **the root cause of the 274 is not
established**, and the incident's own state cannot be reconstructed. The leading condition is H3, the
trace/refund mask; H2 is closed by construction; H1 is now diagnosable and its acceptance rule is a
recorded protocol decision. The matrix and the next live occurrence's `stateHashMatched` are what can
turn the leading hypothesis into a cause.

The most transferable output: **a refusal on a non-state field must not be allowed to look like a
state divergence**. The code now says which verdict it computed, and the durable body makes the next
incident reconstructible.
