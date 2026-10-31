# Rejoin acceptance: the HAZOP and the criterion change, 2026-10-31

Evidence for the change to the N8/N4 partition/rejoin acceptance check. The review was a HAZOP of the
**acceptance design**, not of the node: six study nodes (the checker, the experiment, chain dynamics,
the health signals, the evidence layer and acceptance governance), the ten IEC 61882 guide words per
node, adversarial verification of the six highest-severity rows, synthesis, and a referee.

## Why

Run [41208692673](https://github.com/rhovision-org/rholo/actions/runs/41208692673) failed arm 1 of the
`N8 partition/rejoin` drill in cycle 2 after 150 s; arm 2 passed on the same commit and the identical
failure occurred 16 h earlier on `7c24c3b53`. The failing check required all eight validators'
`(last-finalized height, hash)` to be **exactly equal in one 2 s probe** of a producing chain.

Replaying the shipped artifact (`isolated-n8-16gb-41208692673-1`):

| fact | value |
|---|---|
| cycle-2 healing start | `observations.jsonl` `utc = 1793457156.83` |
| seven online nodes at that sample | height **54**, hash `908c38…` (all seven) |
| victim at that sample | height 45, hash `d698…` |
| victim crosses the old `target = 48` | **+89.1 s** |
| victim reaches the exact anchor `54 / 908c38…` | **+97.5 s** |
| final probe (~ +149.8 s) | node 0 at 69, nodes 1–6 at 67, node 7 at 63 — **three distinct identities among the online nodes themselves** |

The exact-equality predicate was unattainable for a healthy net, and the safety property the drill
exists to certify (every validator had finalized the same block) already held at +97.5 s.

## What changed

The criterion is now a **fixed finalized anchor**, chosen while the victim is still isolated: a block
one side has finalized and the other has not. The victim may hold it; it must not have finalized it.
After rejoin every node must answer `GET /api/is-finalized/{B*}` true on a K-sample dwell, the anchor
height must serve `B*` among its blocks (a Casper height holds one block per sender), the marker must
be readable **at `B*`**, and a
pairwise finalization check (`is-finalized` of every node's own last-finalized hash on every other
node) catches a fork above the anchor. Head heights may differ. A timeout is classified
(`FAIL_STALLED`, `INCONCLUSIVE_SLOW_PROGRESS`, `INCONCLUSIVE_MEASUREMENT`, or a named precondition),
not collapsed into one string.

The experiment's preconditions are now checked rather than assumed: the post-cut network set is
exactly empty, no consensus port is published for the victim, its tip is frozen across three
isolation samples (read through its own container loopback), and its container identity is unchanged
at install and at the verdict. The base drill refuses a node with a latched pipeline fault, a
sustained finality-stall alarm or non-durable divergence counters at start, instead of reporting a
later timeout.

**The window is measured, not wished.** `W = clamp(270 s, 2 × gap × T_h, 600 s)`, where `gap` is the
finalized distance the victim must close at rejoin and `T_h` is the median finality interval measured
on the **seven online nodes** over at least three advances spanning two intervals. The floor is the
node's own block-request maintenance (`requested_blocks_timeout` 240 s plus the 30 s casper loop); the
ceiling is the drill's admissible maximum. The rate is never taken from the victim — a blocked
victim's own rate is the quantity under test. A slow victim is `INCONCLUSIVE_SLOW_PROGRESS`; a stall
needs every progress signal flat (finalized height, tip, peers, undelivered sends), not finalized
height alone.

**The node's fault latch is a current obligation.** `pipelineFaultsOutstanding` is the set of blocks
this process still owes a verdict for: a fault inserts its block, a settled verdict removes it, and a
restart seeds the set from the durable disposition ledger (an unreadable ledger fails closed). The
lifetime `pipelineFaults` counter stays published for evidence. Readiness, deploy admission, proposal
and the block-API gate read the outstanding set, so a transient fault no longer withholds a node for
the rest of the process, and a restarted node no longer reads a fresh zero as a clean pipeline
(Law 54a: the current view, not a history).

**Coverage.** The two cycles cut two different validators (never the bootstrap), and the victim's
missing range must cross an epoch boundary; a run that does not is `INCONCLUSIVE_EPOCH_COVERAGE`
rather than a pass of a different code path.

## Later dispatches, and the rule they teach

Two further dispatches on 2026-10-31 failed, each on a check that read a **moving quantity once** —
the same class as the defect this change repairs:

| run | head | outcome | defect |
|---|---|---|---|
| [41219535778](https://github.com/rhovision-org/rholo/actions/runs/41219535778) | `0aa57584` | both arms failed | the progress loop and the anchor iterated `range(validators - 1)`, which is the online set only when the victim is the last index; the rotation cuts validator 1 in cycle 2 and the loop read the isolated victim |
| [41225136080](https://github.com/rhovision-org/rholo/actions/runs/41225136080) | `412166377` | arm 2 passed; arm 1 failed at rejoin | `FAIL_STALLED` was raised on the victim's `finalityStalled` latch alone, while the rejoining victim was still catching up |

The standing rule: **a check names the sound invariant, not a snapshot of a moving quantity.** The
online set is the rotated complement (`online_validator_indices`); isolation is a frozen **finalized
fence** (a local proposal raises the tip only) plus the bounded tip dwell; and a stall verdict needs
the alarm *and* every progress signal flat across the dwell, the same rule
`classify_healing_timeout` applies. Each has an offline control.

## The review

`hazop-rows.json` is the full worksheet: 48 rows with guide word, deviation, RCA, consequence, class,
severity, disposition rung, formal status, safeguard and falsifier, plus the six adversarial
verifications and their corrections. It is machine-readable so a reader can check a row rather than a
summary.

The six study nodes map onto this repository's own worksheet (`docs/src/spec/failure-hazop.md`):

| study node | in-tree node | rows this review re-uses |
|---|---|---|
| checker / anchor | U3 finality / fringe / estimator | F-U3-01; H-U3-01 … H-U3-06 |
| experiment preconditions | U5 sync & join, U19 block acceptance | F-U5-01; U19/U20 rows |
| chain dynamics | U3, U4 merge & DAG search | F-U4-01, F-U4-02 |
| health signals | U18 validator liveness, U10 restart | F-U18-09, F-U10-02 |
| evidence and CI | — (no in-tree node; layer only) | — |
| acceptance governance | — (layers and runbook) | — |

The working IDs in `hazop-rows.json` (`HC-*`, `D-CH-*`, `HR-*`, `AG-*`, `D1-*` …) are the review's
own; they are not citations of the in-tree register.

## Formal status

The harness rests on **Law 15** (`seen_monotone_of_reaches`; and the refutations
`fringe_monotone_is_false`, `cross_sender_height_monotone_is_false`), **Law 54a** (a liveness predicate
reads the current view, not a history), **Law 51** (the `Void`/`Terminal`/`Drift` and
`Historic`/`Split` vocabulary), **Law 52a** (`spec/LAWS.md:79`), **Law 58** (whose schedule hypothesis
is explicitly unproved) and **Law 71** (tip-independent catch-up). Four lemmas are owed and are named
as `candidate:` slugs rather than proved: `finalized-membership-monotone`, the dual
`rejoin-certificate-predicates-prefix`, `pairwise-finalized-imply-one-chain`, and
`post-state-is-a-function-of-block`. Every layer in this change is an R2 layer of protection; none is
R1, and the register must not mark one done.

## Non-claims

A finite devnet witness, not universal CBC safety; no claim about conflicting finalisation above the
observed anchor; `is-finalized` is a node-local view; the anchor limb is a layer of protection until
`finalized-membership-monotone` is proved; causal capture stays diagnostic and never gates the verdict;
this N8 drill is not criterion 2's live-net rejoin witness.

## Reproduce

```sh
python3 -m unittest discover -s tests -p test_recovery_harness_controls.py -v
python3 tests/consensus-drill-regression.py
python3 -m unittest discover -s tests -p test_quiet_finality_controls.py -v
python3 -m unittest discover -s tests -p test_post_restart_proposal.py -v
python3 -m unittest discover -s tests -p test_hardfork_51_closeout.py -v
```

A live run is one `workflow_dispatch` of *Recovery v2 — full acceptance on dev (16GB)*; its artifact
carries `campaign.json` and every cycle's `healing_anchor`, `victim_finalized_at_rejoin` and verdict.
