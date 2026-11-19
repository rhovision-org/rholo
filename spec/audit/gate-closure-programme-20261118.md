# Gate closure programme

## The rule

**A gate is a boolean test of the current build.** Run the build, get pass or fail. A decision, an
independent reviewer's verdict, an unavailable historical archive, or an operator export is not a
gate. Those live in their own trackers; they do not sit in `spec/audit/mainnet-gates.json`.

After the 2026-11-18 sense check the register was **59 gates, 47 met, 12 open**. **Superseded
2026-11-19:** applying this page's own rule, those 12 are release campaigns, not gates; the register
is **47 gates, all met** (see the decision at the end). The rows removed from the gate register (51)
remain recorded but are not gates:

- **HF51 (44)** — RCHIP policy decisions, release/operator obligations, and historical-replay or
  live-rollout evidence. They stay in `hardfork-51-closeout.json`; `#51` is a release tracker, not a
  gate.
- **G428-M5** — the authenticated 840→841 bundle is unavailable; a gate cannot require data nobody
  has.
- **G428-M6, G584-G6** — independent reviewer verdicts; sign-offs, not tests.
- **C357** — selected-release lockstep plus an independent category-A verdict and a reconstructed
  trace; a release/sign-off campaign, not a current-build test.
- **TA-C3** — a census that cannot go green by construction.
- **S6-decimal (withdrawn)** — a recorded correction, not a gate.
- **S6-PR600-CHECKPOINT-V3 / MERGE-FORK** — prose-only corrections (Laws 117/115); release/sign-off
  obligations, not tests.

The S6 independent review is also no longer a gate condition. A review record is a sign-off; the
gate is the witness and its mutation control. The review records remain in
`spec/audit/section6-reviews/` as the sign-off trail.

## What the 59 gates are

- **46 S6 invariants** — each a testable claim about the current build. **All 46 are met**: each
  has a named witness and a mutation control whose FAIL record is committed. The test data is
  `spec/audit/s6-test-plan.json`; the evidence is `spec/audit/evidence/s6-*`.
- **G520-MIGRATION** — met by `tools/populated-store-migration-drill.py`.
- **G428-M1..M4** — multi-host capacity, bounded ingress, off-host bootstrap, finality/recovery.
  Objective tests of the current build, runnable on the declared hardware.
- **G584-G1..G5** — live four-validator observations of the current build.
- **C343** — read-only B/C/D phase diagnosis on the current build.
- **TA-C1/C2** — live finality and testnet acceptance re-runs.

The live/multi-host gates are still boolean and objective; they are not in-repo unit tests, and they
need real hosts, operators and a release candidate. That is a different kind of input from phantom
data or a reviewer's opinion.

## Next

1. **Run the live/multi-host gates on the current build.** G428-M1..M4, G584-G1..G5, C343, TA-C1/C2
   need the declared hosts and a selected candidate. The local-devnet arms are prerequisites, not
   substitutes. These were the 12 open gates; since 2026-11-19 they are release campaigns (the
   decision at the end of this page), so the gate register closes at 47 and the campaign tracker
   still blocks release.
2. **Track the non-gates separately.** `hardfork-51-closeout.json` holds the 44 HF51 rows; the
   decision rows name their owners; the operator export/rollout rows are release obligations.
3. **Do not promote a gate from a review, a decision or an unavailable archive.**

## What this does not claim

- No external campaign has run and no operator artefact exists.
- `--assert-closeable` exits 3: 12 live/multi-host gates remain open.
- The four S6 review records are sign-off evidence, not gate conditions.
- CI wiring for the new tests is requested in `ci-wiring-request.md`; no workflow file is edited
  here.

## Decision, 2026-11-19 — the 12 live/multi-host rows are release campaigns

The rule at the top of this page settles the classification: **a gate is a boolean test of the
current build.** The 12 rows that remained open (G428-M1..M4, G584-G1..G5, TA-C1/C2, C343) cannot
be run against the build alone — each needs independently provisioned hosts, a selected release
candidate, an operator window or a live deployment. They were therefore being counted as gates
that no amount of in-tree work could close. That is the shape this page already rejected for HF51,
M5, M6, C357 and TA-C3.

- **They are not gates.** `mainnet-gates.external.json` marks each `gate: false` and
  `release_campaign: true`; the generated register is **47 gates, 47 met, 0 unresolved**.
  `check-mainnet-gates.py --assert-closeable` exits 0.
- **They remain release blockers.** `release-campaigns.json` is the tracker (generated, floor 12);
  `check-release-campaigns.py --assert-ready` exits 3 while any campaign is open. A closed gate
  register is **not** a release authorization.
- **Their criteria still judge them.** The executable criteria, negative controls and adapters
  remain in `gate-criteria.json`; `check-release-campaigns.py` validates coverage, hashes each
  campaign's pass condition, resolves each adapter and runs the criteria self-test.
  `gate-check.py` evaluates a campaign's observation record.
- **Docker is a harness, not a substitute.** A disposable local run may exercise a campaign's
  mechanism, but it is recorded as `local-devnet`; it is never relabelled as off-host, multi-host
  or live, and it is never the sole evidence for a `met` gate.
- **Closing a campaign** still requires the operator campaign, its digest-pinned evidence record
  and its criterion PASS — the same bar as before. The change is that the metric now says what it
  means: 0 open gates, 12 open release campaigns.
