# The #564 adversarial audit

**Method.** One row per attack on the incident narrative, the prior HAZOP, and PR #565. Each row names
the claim under attack, how it was tested (`read` = code read; `measured` = a run or test; `reported` =
the issue/log, not re-derived), the verdict, the evidence anchor, and the disposition. A confirmed
defect is fixed in this PR or registered with an `owed:`; a refuted claim is recorded with its
falsifier. The audit is deliberately hostile to the PR it accompanies.

**Scope of the evidence.** The incident's store, terms and block body are unavailable (see
`incident/unavailable.tsv`). No row below claims to have replayed #1115. The incident-binary read is
`0a1e02362`; the dev read is this branch.

## A-rows

| id | claim under attack | method | verdict | evidence | disposition |
|---|---|---|---|---|---|
| **A-01** | The replay decides a recorded-cost mismatch **before** comparing the declared post-state hash; a state-correct block can be refused on cost alone. | read both revisions; `casper/tests/determinism.rs::a_cost_only_tamper_is_refused_before_the_state_hash_is_compared` | **confirmed** | `casper/src/runtime_replay.rs:489-497` returns `ReplayCostMismatch`; `casper/src/interpreter_util.rs:127-142` compares hashes only on `Ok`; the trace check (`runtime_replay.rs:333`) runs after the early return. Same order at `0a1e02362`. | C358; the shadow replay (`CostMismatchDetail` with `state_hash_matched`) and the operator record are in; the acceptance rule is a protocol decision (`cost-criticality.md`). |
| **A-02** | The self-replay tracker keys on the whole block deploy set, so a varying system-deploy tail defeats the bound. | read `RholangState` | **refuted** | `models/src/casper/protocol/casper_message.rs:731-735` separates `deploys` (user) and `system_deploys`; the tracker maps `block.state.deploys`. | A pinning test is owed; no production change. |
| **A-03** | `excluded` grows without pruning or a cap, and `poolQuarantined` is monotone. | read `proposer.rs`; `the_tracker_counts_only_identical_sets_and_keeps_exclusions_on_success` | **confirmed** | `SelfReplayTracker::exclude` only extended; no prune; no cap. | Fixed: capped at 256, pruned against the pool the last selection read; a full set escalates to the counted path. |
| **A-04** | `last_selected` is correct only because one queue consumer calls the proposer; the type does not hold the association. | read `proposer.rs`; the proposer test | **confirmed** | `set_selected` and the refusal arm were separate lock acquisitions; correctness rested on the single mpsc consumer. | Fixed: the selection is keyed by `(height, sender, pre_state_hash)`, so the refusal reads the block's own selection. |
| **A-05** | A peer block carrying an excluded deploy is still refused as `Divergence`; the exclusion is proposer-local. | read `validate_with_origin` peer path | **confirmed residual** | `refusal_for(Peer, Failed) -> InvalidState` (`casper/src/multi_parent_casper.rs`); unchanged by design. | Documented as a runbook residual (R4); the node is faulty (RC1) if it cannot replay a peer's block. |
| **A-06** | Excluding the whole pool with autopropose off reintroduces an idle freeze: nothing proposes, the height cannot advance, and the excluded deploys cannot expire. | read the counted/bounded paths; fault-halt test planned | **confirmed conditional** | `select_deploys` returns empty; with `--autopropose` off there is no timer and no dummy; expiry is height-keyed. | Fixed: local fault halt after N/age + operator resume + `recover-pool` (R2/R4). |
| **A-07** | `/health`'s hand-built 4-field refusal object drifts from the 10-field DTO/OpenAPI schema. | compare `node/src/web/http.rs` to `node/src/api/dto.rs` | **confirmed** | `/health` builds `{code, blockNumber, seqNum, atUnixMs}` inline; the operator DTO has the replay detail; nothing checks the public projection. | **Owed**: one declared public projection in `health_contract.rs` + a contract test; the detail is operator-only by design, but the four-key shape is not checked. |
| **A-08** | The refusal, the exclusion and the refused block body do not survive a restart; the incident is not reconstructible. | read tracker lifetime and `put_block` placement; restart test planned | **confirmed** | The tracker is an in-process `Arc<Mutex<...>>`; `put_block` runs after validation succeeds, so a refused self-built block is never stored. | Fixed: `SelfReplayRecord` reserved key + refused body + boot seed + admin resume (C359). |
| **A-09** | The new proposer tests use a fake `validate` closure that always returns `SelfReplayFailed`; they never run the real replay/selection path. | read the test rig; the real-runtime cost-only tamper | **confirmed** | `proposer_failing_self_replay` injects the failure; `select_deploys` is exercised separately. | Partly fixed: the real replay path is exercised by `a_cost_only_tamper_is_refused_before_the_state_hash_is_compared`; a full `validate_with_origin` block fixture is still owed. |
| **A-10** | The PR/issue narrative overclaims: "deterministic" is two observations on one node; "2 user deploys" vs 7 pooled is unexplained; "all four run 0a1e0236" has no binary-hash evidence; "replay validation is correct to refuse" is in tension with A-01. | read the issue and pass §153 | **confirmed** | Issue #564 body; `spec/audit/passes.md` §153. | Corrected in the PR text, C357 and the issue update; the reported facts are labelled. |
| **A-11** | The synthetic determinism test is a root-cause reproduction. | read the test and its docs | **confirmed overclaim** | It runs `dev`, a 3-deploy shape, no cost trace, no cost magnitude; it passes. | Reclassified as negative evidence for its shape; it is not the RC1 falsifier. |
| **A-12** | Batched play with `RNODE_CROSS_DEPLOY_BATCH_VERIFY=0` can record a cost the sequential replay refuses — and, measured on the incident binary, a **state** the replay rejects. | read `play_deploys_batched_with`; matrix M07/M08 on `0a1e02362`; `a_batch_policy_without_verification_is_not_representable` | **confirmed measured, now closed** | The only `raw.cost == batched.cost` comparison was inside `if self.batch.verify` (`runtime_manager.rs:1832-1838`). M08 (`verify=0`) replays with `state_match=false`; M07 (`verify=1`) matches. | R1 fix: an enabled batch always verifies (`canonical_batch_policy`); the node warns when the unsafe flag is requested. |
| **A-13** | An unused COMM or a failed deploy's trace/refund difference surfaces as a cost mismatch and never as `UnusedCommEvent`, hiding the real condition. | trace-mask test (decision A; matrix M04/M15) | **measured** | `check_replay_data_with_fix` runs after `replay_deploy_eval` returns (`runtime_replay.rs:333`); the cost return is earlier. `a_cost_refusal_masks_the_trace_check_and_the_shadow_reports_it` makes the trace disagree independently and shows the enforcing arm reports only the cost, while the cost-ignoring shadow reports the unused COMM. | C358 closed: `spec/audit/evidence/n564-self-replay/cost-criticality.md` (decision A) + the trace-mask test. |
| **A-14** | A storage/refund difference (produce vs COMM, refund counts) can move the cost without moving the state. | trace/refund census + matrix | **unresolved until measured** | `rholang/src/storage.rs:191-222`, `:284-466`; the census is new. | C358 evidence. |
| **A-15** | The 45-hour freeze is the C270 divergence arm plus the absence of a bound. | read the proposer arms; issue timeline | **confirmed** | The divergence arm returned `NotEnoughNewBlocks` without counting/surfacing; the same deploy set rebuilt the block. | C357 (the liveness barrier); the fault halt and `recover-pool` are the remaining fallbacks. |

## What this audit does not claim

- It does not establish that #1115 was state-correct. The body was never stored, no peer received it,
  and A's store and terms are unavailable. The tamper and shadow tests prove the **mechanism** (the
  state verdict is not computed on a cost refusal), not the incident's state.
- It does not identify the cause of the 274 difference. The leading untested hypotheses are the trace/
  refund asymmetry (A-13/A-14) and the non-canonical play modes (A-12/P0′); the matrix is the evidence
  that will rank them.
- It does not change consensus. Whether a state-correct/cost-mismatched block should be accepted is
  recorded in `cost-criticality.md` as a decision, not encoded as a local fix.
- It does not treat the passing synthetic test as evidence about the incident. That test is negative
  evidence for its own shape.
