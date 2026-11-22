# #584 — HAZOP of the settlement/finality/recovery path

**Reviewed revision.** `dev@3ea5381b3` plus the #584 refactor branch. The machine register is
`spec/Rchain/Laws.lean`; the release criteria are `spec/audit/release-claims.json`. This study is the
qualitative companion to `n584-failure-mode-rca.md`: it asks what can go wrong at each node of the
path, including the **measurement** path, and records the guard and the fallback.

**Why this study exists.** The protocol hazards (false settlement, finality stall, join freeze) are
already measured and closed by construction; the RCA says so. What has not been studied as a HAZOP is
the **gate** itself: a runbook that forbids the recovery event its G4 requires, a preflight that a
synthetic local rehearsal passes, an async relay that can miss a per-alias stage, and a closing
condition that is a reviewer's attestation. Those are hazards in the measurement path, and they are
why #584 has never closed.

## §1 The path

| # | step | what the node/operator does | observable |
|---|---|---|---|
| S1 | deploy submission | operator submits one signed native-REV transfer | HTTP 200 + deploy id |
| S2 | pool | the receiving node (and, if configured, peers) hold the deploy | `deploy-status` `Pooled`; settlement `pooled` |
| S3 | proposal | a validator builds a block; its own replay must agree | `create_block`, self-replay checks |
| S4 | self-replay | a refusal falls back to an attestation-only block, a durable record, a reset or a halt | `selfValidationRefusal`; `SelfReplayRecord` |
| S5 | attestation | validators attest; support counts only full partitions against the whole bonded stake | finality support |
| S6 | finality | the fringe advances when support is > 2/3 | `finalizedHeight`, `lastFinalizedBlock` |
| S7 | settlement surface | a deploy is `finalizedSuccess` only if its **inclusion block** is finalized | `/api/v1/deploy-settlement/{id}` |
| S8 | status surface | `ready`, `acceptsDeploys`, `finalizedLag`, `catchUp` | `/health`, `/api/status` |
| S9 | restart | the victim replays its store and rejoins | boot id, phase, catch-up |
| S10 | join/catch-up | a behind node walks the validated frontier and re-enters on a stale view | `CatchingUp{own, peerTip}` |
| S11 | state merge | a finalized fringe is merged at its member's claim | pre/post-state, rejectedDeploys |
| S12 | evidence | the run is sampled, redacted, packed and read by a predicate | `release-check.py` reading |

## §2 Guide-word inventory

Each row is a deviation at a node, its cause, consequence, existing safeguard, and status. `new`
means this study surfaces a gap not already covered by the RCA's closure matrix.

| id | node | guide word | deviation | cause | consequence | safeguard | status |
|---|---|---|---|---|---|---|---|
| **H1** | S1/S2 | more | the deploy is accepted while finality is unavailable, and a client reads that as settlement | `acceptsDeploys` is pool capacity | the #487 false claim | #552 contract; `pooled` is distinct; settlement endpoint | closed |
| **H2** | S2 | part-of | only one validator pools the deploy | relay off / one ingress | the other aliases never see `pooled`; G2 incomplete | relay or four-ingress; RC-1/public manifest | process (RC-1/public) |
| **H3** | S2 | late | a relay offer arrives after inclusion | async best-effort | `pooled` missed on a peer | four-ingress as the deterministic path; 1 s sampler | process (prefer four-ingress) |
| **H4** | S3 | no | the proposer refuses its own block | deterministic replay mismatch | finality capped if it holds the only pool | attestation-only fallback; C361–C365 | closed |
| **H5** | S4 | reverse | a self-replay failure is read as a peer divergence | provenance not consulted | retry loop for 45 h | Law 92; `validation_origin` | closed |
| **H6** | S4 | late | the self-replay record is written after the transition | process dies between | evidence lost | durable record before transition (C365) | closed |
| **H7** | S5 | less | a silent validator's stale message carries the quorum | partition not the live set | finality freezes on an absent validator | Law 14a/52; live weight set | closed |
| **H8** | S6 | other-than | the local fringe is stale, so `finalizedHeight` lags while the node reports `running` | the finalized read is node-local and unmodeled | an operator reads a stale value as network truth | `finality-observation` says `nodeLocal`/`unknown`; **Law 125** | this PR |
| **H9** | S7 | more | `ProcessedWithSuccess` or a tip balance is read as settlement | surface semantics | false settlement | settlement endpoint; **Law 126** | this PR |
| **H10** | S7 | other-than | the settlement route says `finalizedSuccess` for an inclusion block the local fringe does not contain | a stale/incorrect `is_finalized` | false settlement | `is_finalized` is the read; fail-closed on RPC error; **Law 126** | this PR |
| **H11** | S7 | as-well-as | `finalizedError` is read as successful settlement | the stage table conflated | a failed deploy treated as settled | stage is separate; test | closed |
| **H12** | S8 | more | `ready`/`acceptsDeploys` is read as a finality claim | three independent meanings | false readiness | docs; `finalityStalled`; `catchUp` | closed (documented) |
| **H13** | S8 | no | the node admits work while finality is stalled, with no alarm | admission is pool capacity by design | accepted work waits | `finalityStalled`, `lastFinalityAgeMs`; #552 | closed |
| **H14** | S9 | early | the victim is restarted on an empty store with its key | store lost | own-key equivocation | passive catch-up then activation (#576/#578); testnet.md | closed |
| **H15** | S9 | late | the victim restarts but never catches up; it reports `running` | boot-only watchdog | frozen join (#640) | continuous supervisor; **Law 123** | closed |
| **H16** | S10 | no | the catch-up view compares the node with itself | `own − supported` | frozen node reads current; healthy leader reads stale | `peerTip − own`; Law 121 | closed |
| **H17** | S10 | part-of | the walk starts above the missing range | metadata frontier, not validated frontier | the gap is never fetched | validated frontier; Law 121 | closed |
| **H18** | S10 | other-than | an ancestry gap is condemned as `UncertifiedPreState` | absent justification | the block and its descendants are lost | deferral/backfill; Law 116 | closed |
| **H19** | S11 | reverse | a merge above a finalized base undoes a write | base not guarded | finalized effect lost | Law 113; `choose_base_claim` | closed |
| **H20** | S12 | more | a prior build's pins/samples are submitted as this run's evidence | runbook provenance | a stale result is read as current | RC reading only from a fresh run; `not-observed` | **new (this PR)** |
| **H21** | S12 | no | the G4 arm is never named; the runbook forbids the restart G4 needs | two documents disagree | G4 is INCOMPLETE and the gate cannot close | RC-4/public names the real host stop; runbook retired | **new (this PR)** |
| **H22** | S12 | early | a pre-approved G4 restart invalidates the preflight boot pins | §1 freeze rule | the run stops on the event it needs | RC-4/public records the boot before/after; no reviewer freeze | **new (this PR)** |
| **H23** | S12 | other-than | a synthetic boot ID passes the preflight | `check_g1` checks distinctness, not source | a rehearsal reads as live | live-gate is diagnostic; RC public manifest requires live observations | **new (this PR)** |
| **H24** | S12 | late | a stage transition is missed at 1 s | fast chain / slow peer | G2 incomplete; no second deploy allowed | 1 s cadence; four-ingress; capture every tick | process |
| **H25** | S12 | more | one public route is sampled four times | the endpoint proxies to one node | four observations are one | RC manifest names four bases; duplicate endpoint refused | process |
| **H26** | S12 | no | the reviewer is not independent / does not record | human input missing | the issue stays open | **retired**: the claim reading is the verdict | this PR |

## §3 Guard and fallback, by new hazard

### H20 — prior-test provenance

**Rule.** A release-claim reading is derived from the newest **committed** record by recomputing the
test's predicate (`tools/release-check.py`). A previous build's pins, a previous window's samples or
a reviewer's attestation cannot turn a `not-observed` arm into `safe`. **Fallback.** The arm stays
`not-observed`; `--assert-ready` refuses; the next PR is a fresh run.

### H21/H22 — the G4 contradiction

The old runbook required a recovery event for G4 and forbade deliberate restarts; it also froze boot
IDs and invalidated the run on any drift. **Rule.** RC-4/public is the named arm: one real host stop
during `includedUnfinalized`, survivors finalizing past it, the victim rejoining the same finalized
head at least three heights above baseline, no own-key/self-replay halt. The boot before/after is part
of the record, not a contradiction. **Fallback.** If the fleet cannot perform the stop, the arm is
`precondition`/`not-observed`; the gate refuses.

### H23 — synthetic identity

**Rule.** The RC public manifest must carry four distinct live observations; a local rehearsal is
`local-devnet` and never public evidence. `tools/live-gate.py` remains a diagnostic sampler, and its
`check_g1` synthetic-boot gap is documented as a diagnostic limitation rather than a gate. **Fallback.**
A run with synthetic identity is not recorded as public.

### H8/H9/H10 — the unmodeled read surfaces

**Rule.** **Law 125** states `is_finalized(h) ⇔ h ∈ ⋃ seen(latest_fringe)` and that the value is
node-local. **Law 126** states `settlementConfirmed` only for a finalized inclusion block and that
the lifecycle is exhaustive/exclusive/monotone. **Fallback.** A read that cannot establish the
inclusion block's finality is `finalityUnknown`/`unknown`, never `finalizedSuccess`.

## §4 What this study does not claim

- No live run. The RC-1/public and RC-4/public records are owed.
- No release readiness. `--assert-ready` still exits 3.
- No §6 retained-history decision. The public campaign uses a coordinated new genesis.
- No proof of Law 58's liveness-to-finality sufficiency; it is a hypothesis and the campaign measures
  it.
- No claim that the old `live-gate.py` is wrong as a sampler: it is retired as a **gate**, not as an
  observation tool.
