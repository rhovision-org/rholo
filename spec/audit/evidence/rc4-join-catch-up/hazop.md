# RC-4 — HAZOP of the light join's catch-up

**Reviewed revision.** `84d43d8fe` (draft PR #639). The machine-checked phase row is Law 121
(`spec/Rchain/JoinCatchUp.lean`); the claim's record is owed until the drill passes.

**Why this study exists.** The RC-4 drill's light join works: the victim adopts the certified anchor,
restores the state and enters `running`. It then stops. In the run at `84d43d8fe` the victim joined
at anchor 181, reached tip 183, and did not advance again for the remaining 240-second window while
the other three validators ran to 306. Its `/health` read `running`, ready, no pipeline fault, and
its proposer logged `attestation at tip 183: licensed … nothing_to_finalize=false
new_state_transition=true quorum_reachable=true cadence_due=false … blocked_attempts=0 escape=false`.
Nothing in that line is false; nothing in it says the node is **stuck**. That is the hazard: a
joined node can be live, honest, licensed — and frozen, with no named blocker.

The earlier failures in this PR are the same class one layer down: the boot refused (named), the
publisher refused (named), the proposer held every round (named). This one is silent, so it is the
one this study is about.

## §1 What the node was asked to do

| # | step | succeeded? | evidence |
|---|---|---|---|
| S1 | fetch and verify a quorate certificate | yes | acquisition trace, 300/400 |
| S2 | adopt the certified anchor and rewind to the last fully-certified height | yes | decision trace `recovery: light-join` |
| S3 | restore the certified state and publish it | yes | checkpoint store, anchor 181 |
| S4 | **reach the live tip and stay current** | **no** | tip frozen at 183 while peers are at 306 |

S1–S3 are the sync engine's contract; S4 is the running engine's. The join crosses the boundary
between two components with no shared progress obligation.

## §2 Hazard inventory

| id | hazard | sharpest fact | how known |
|---|---|---|---|
| **J1** | **Gossip-only catch-up.** After the join the node holds the certified history to the anchor and relies on ordinary block gossip for what lies above it. Gossip carries new tips, not the missing range; the ancestry walk that closes the range lives in `NodeSyncing`, which has already exited. | the victim validated the first two blocks it received and then had nothing it could validate; no fault, no retry | measured (tip 181→183, then stationary) |
| **J2** | **Silent deferral.** A block whose ancestry or pre-state the node does not hold is deferred, and the deferral is not a counted fault or a named status. `/health` stays `ready`. | the attestation line reports a clean pipeline while the tip is frozen | read: the receiver has no deferral counter on the health surface |
| **J3** | **The phase lies.** `Running` is entered when the *state* is restored, not when the *fence* has caught up. Every downstream reader (readiness, deploy admission, the operator) treats `Running` as "serves a synced chain". | S3 passed and the node was `running` at tip 183, 123 heights behind | read: the launch's phase write |
| **J4** | **No progress obligation.** Nothing owns "a joined node must close the gap". The proposer has a private-chain bound and the finality engine has a fence, but neither is a catch-up deadline or a fallback. | the node sat in the frozen state until the harness's window expired | measured |
| **J5** | **Backfill competes with the live path.** The join sets a history floor at 1 and backfills bodies; under the drill's deploy burst the historical work and the live receiver share the node's budget, and a live-path stall is indistinguishable from slowness. | the three earlier runs stalled at 161, 202 and 183 — three different plateaus | measured |
| **J6** | **The round-escape masks the stall.** A round that does not close produces an escape every 16 s; the escape is loud but it is not evidence that the tip is advancing. | `round gate escaped after 218 attempt(s)` in the same log as a stationary tip | measured |
| **J7** | **Diagnostics name the local view only.** `/health` reports the node's own fence, never "the peers are at N and I am at M" or "my deferral is X". A frozen node and a quiet chain read alike. | the failure had to be reconstructed from the harness's identity samples | measured |

## §3 Guard and fallback, by hazard

| id | guard (the state is named or unrepresentable) | fallback (a safe way out) | status |
|---|---|---|---|
| J1 | keep the catch-up obligation alive after the join: the node is not `Running` until its fence is within `LIVENESS_WINDOW` of the quorum-supported tip it can see | re-enter the checkpoint sync (the running node's existing rewind-and-sync path) when the gap does not close | this unit |
| J2 | count and name every deferral: `deferredBlocks`, `lastDeferralReason` on `/health`; a non-zero deferral is not `ready` | the deferral names its missing hash; the ancestry request retries it | this unit |
| J3 | **Law 121**: `Running` carries a proof that the fence is within the window; the phase is otherwise `CatchingUp { supported }`, a value that names its own blocker | the phase machine has no third answer — a node cannot be running and behind | this unit (Lean) + the Rust phase check |
| J4 | the catch-up state carries a deadline measured in **observed peer progress**, not wall clock | a watchdog re-enters sync after a bounded non-progress interval, naming the coordinate | this unit |
| J5 | the live path never waits on the historical backfill; both publish their own progress | the backfill is resumable and idempotent (already true) | read; test owed |
| J6 | the escape counter resets only on a fence advance, and a repeated escape without progress is the watchdog's trigger | as J4 | this unit |
| J7 | `/health` gains `joinAnchor`, `fence`, `highestLatestMessage`, `quorumSupportedHeight`, `peerTip`, `deferredBlocks`, `lastDeferralReason` | an operator line names the same coordinates | this unit |

## §4 The phase machine, as this unit states it

```
Starting ──launch──▶ Syncing ──restored──▶ CatchingUp(supported) ──fence within window──▶ Running
                         ▲                        │
                         └──── watchdog: no progress for T ────┘
```

`CatchingUp` is a new `NodePhase` value. It admits no proposal and no attestation, exactly as
`Syncing` does today, but it is a **running engine**: it receives, validates and backfills, so the
fence can close. The transition to `Running` is the only one that requires a proof, and Law 121 makes
the unproved form unrepresentable.

## §5 What the next drill must show

The victim joins, passes through `CatchingUp` with the coordinates exposed, reaches `Running` with
its fence within the window, and the harness's agreement and deploy checks pass. A victim that cannot
close the gap must leave a `CatchingUp` record naming `quorumSupportedHeight`, the peers' tip and the
last deferral — never a silent freeze.
