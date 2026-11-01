# Aria's testing workflow: the Pareto audit, 2026-11-01

A review of improvements to **Aria's (aixaria0) pre-production testing job** across his open issues
(#520, #487, #428, #412, #339, #522) and the harness/evidence workflow. Rewards here are **operator
time and evidence risk removed**, not gate closure — a deliberately different objective from the
M3/M5 audit.

## Method

Three inventories, then a scoring/referee pass with adversarial corrections:

| pass | reads | returns |
|---|---|---|
| issue/workload | the six issue bodies and their comment threads | per-issue criterion, run/record burden, pain points, last status |
| harness/tooling | every acceptance harness, `devnet.sh`, the workflows | harness table, artifact flow, ranked friction with `file:line` |
| evidence/automation | evidence convention, checkers, issue-comment workflow, replay tools, CI vs local | artifact/step table, automation candidates with drift and risk |
| referee | the three | rubric, dominance, frontier, corrections, the picks below |

Rules the referee applied: a reward claim must name the manual step it removes; every automation
states what stays manual and how it could look green while removing no work; "already exists" is
settled at `file:line`; a shape-only check does not remove a step; two candidates removing different
steps do not dominate each other.

**Rubric.** Reward 0–5 = operator time and evidence risk removed per campaign (3 = 1–4 hours or one
unanswerable question made one command; 4 = half a day or a whole diagnosis loop; 5 = an acceptance
question becomes one command plus an auditable record, or a multi-day / evidence-invalidating class
is removed). Risk 0–5 = blast radius (0 tests/tools only; 1 additive read-only; 2 harness change or a
failure-path diagnostic; 3 shared refactor/readiness; 4 consensus/state write; 5 live/safety).
Effort 1–5 includes evidence.

## The step ledger (the rows every reward cites)

| step | frequency | evidence | minutes |
|---|---|---|---|
| copy a run out of gitignored `target/` into `spec/audit/evidence/` before the 14-day artifact expires | every published run | `.gitignore:18,42`; `target/…` in every arm | 10–30 |
| hand-write the manifest, per-file digests and README Provenance/Commands | every evidence bundle | only `n479` has `SHA256SUMS.json`; every other README is authored | 20–60 |
| type the action-run URL and expand short hashes | every comment | #515's run URL 404'd after being typed against the fork; `n420-root-capture.py` needs full hashes | 5–15 |
| diff `node-*.log` against `causal/node-*.json` hash by hash to name an orphan | each #520/#339 failure | Aria, #520: "identify the exact two body hashes and disposition stages"; #339 manual diffing | 60–240 |
| re-check which recorded PASS/FAIL are stale after `dev` advances | every merge | Aria: "the result above is not transferred to that binary" | 15–45 |
| triage a single-string failure | each failed arm | `consensus-restart-under-load.py` `{"verdict":"FAIL","error":str}`; `devnet-fuzz.py` one `FAIL:` | 30–120 |
| grep logs for "is it at the 1024 cap?" | each #522 run | no live gauge; the component test is unit-level | 20–60 |

## Two corrections the audit made

1. **The self-baselining harness is largely built.** `tests/consensus-restart-under-load.py` already
   has `wait_baseline` (agreed finalized baseline, fail-closed) and `victim_for_cycle` (non-master
   then master). The missing piece is the negative controls, not a rewrite (#523).
2. **The 1024 cap is not the mpsc queue.** `"block receiver state full (1024 blocks)"` comes from
   `BlockReceiverState::end_stored_awaiting` when `blocks_st.len() >= MAX_PENDING_BLOCKS`
   (`casper/src/blocks/block_receiver.rs:191`). The existing `queue_observer` measures the validated
   mpsc queue — a different 1024. A gauge of the wrong one would look green while the node still
   refuses.

## The frontier

| ID | piece | removes | reward | risk | effort | disposition |
|---|---|---|---|---|---|---|
| **D** | Evidence packager + verifier: manifest, per-file digests, README skeleton, upstream run URL, PASS/FAIL conflict refusal | rows 1–3 | 5 | 1 | 2 | **landed** |
| **C** | Failure classifier + disposition witness | row 4 | 5 | 2 | 3 | **C1 landed**; C2 deferred |
| **A** | Live receiver-cap gauge (`blocks_st` pending + queues) | row 7 | 4 | 1 | 2 | **landed** |
| **M** | Structured failure envelope | row 6 | 3 | 0 | 1 | **landed** |
| **E** | Source/binary staleness guard (`PIN_SAME`/`PIN_STALE`/`PIN_UNKNOWN`) | row 5 | 3 | 0 | 1 | frontier; deferred |
| **K** | Residue/cleanup guard | silent teardown poisoning | 3 | 0 | 1 | frontier; deferred |
| B | self-baselining harness | already built | 1 | 2 | 3 | dominated |
| F/I/J/L/N/O/P | dashboard, #485 field, shared library, reconcile waits, register shape, retention | — | 2 | — | — | dominated or absorbed into D |
| G/H | 840→841 replay, M3 off-host runner | gate work, blocked on data | 2 | — | — | out of this objective |

## What landed in this unit

- **D — `tools/evidence-pack.py`**: packs a run directory into a record with `manifest.json`
  (source/binary pins, run URL/attempt, verdict, arms, per-file sha256/bytes/kind, redactions,
  `execution_proven: false`), `SHA256SUMS.json`, and a README **skeleton** whose boundary and
  non-claims sections are left for the author. It refuses a missing pin, a PASS beside a FAIL arm, a
  destination under `target/`, symlinks, a fork URL, and unresolved placeholders on verify. `verify`
  recomputes every digest and rejects extra or missing files. Default is **copy**; it never moves.
- **A — `blockPipeline` on `/health`**: `{capacity, pending, sampledPeak, validatedQueue,
  incomingQueue}` per primary shard, where `pending` is `blocks_st.len()` — the queue the cap is on.
  **Advisory only**: no readiness term, no gate consumption.
- **M + C1 — `failure.json` + `tools/classify-run.py`**: the drill, the restart-under-load arm, the
  partition arm and `devnet-fuzz` write a structured envelope (`class`, arm, phase, node, first
  error, health rows, last events, missing data); the classifier returns a cause from a closed
  vocabulary or `UNDETERMINED` with the missing datum — never a guess. The old `verdict`/`error`
  keys stay.

## Deferred, with reasons

- **C2** (runtime orphan witness in `block_disposition.rs`): it reads the store on a failure path;
  fund it only after a fixture shows the existing artifacts cannot name the stages.
- **E**, **K**: cheap and on the frontier, but not in this unit.
- **B**: already built; the negatives remain.
- **G/H**: gate work blocked on unretrievable data, not operator-time work.

## What must stay manual

The boundary and non-claims prose; the Result reading and verdict; close decisions and owners; the
independent sign-off; destructive live operations; the reviewed host table; perishable captures. The
picks avoid false assurance by refusing to write any of these: the manifest records provenance and
says `execution_proven: false`; the classifier returns `UNDETERMINED` rather than a cause it cannot
see; the gauge is advisory.

## Non-claims

This is an audit of operator time, not of safety. No candidate changes consensus, block validity,
attestation, finality or RCHIP-55. A proof that a run's bytes are intact is not a proof that the run
happened; the manifest says so.
