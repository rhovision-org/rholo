# Gate closure programme

## The rule

**A gate is a boolean test of the current build.** Run the build, get pass or fail. A decision, an
independent reviewer's verdict, an unavailable historical archive, or an operator export is not a
gate. Those live in their own trackers; they do not sit in `spec/audit/mainnet-gates.json`.

After the 2026-11-18 sense check the register is **61 gates, 5 met, 56 open**. The rows removed from
the gate register (49) remain recorded but are not gates:

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

The S6 independent review is also no longer a gate condition. A review record is a sign-off; the
gate is the witness and its mutation control. The review records remain in
`spec/audit/section6-reviews/` as the sign-off trail.

## What the 61 gates are

- **48 S6 invariants** — each a testable claim about the current build. Four are met:
  `S6-25bfdbe2c179c81e`, `S6-c7536b90bd39cf71`, `S6-91b05027373a3107`,
  `S6-d7dd0a06a5dee537`.
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

1. **Write the missing S6 tests.** 41 S6 invariants are reviewable because no dedicated test exists;
   add the test and its mutation control, then bind it. Do not add a reviewer requirement to a gate.
2. **Run the live/multi-host gates on the current build.** G428-M1..M4, G584-G1..G5, C343, TA-C1/C2
   need the declared hosts and a selected candidate. The local-devnet arms are prerequisites, not
   substitutes.
3. **Track the non-gates separately.** `hardfork-51-closeout.json` holds the 44 HF51 rows; the
   decision rows name their owners; the operator export/rollout rows are release obligations.
4. **Do not promote a gate from a review, a decision or an unavailable archive.**

## What this does not claim

- No external campaign has run and no operator artefact exists.
- `--assert-closeable` exits 3: 56 gate tests remain open.
- The four S6 review records are sign-off evidence, not gate conditions.
- CI wiring for the new tests is requested in `ci-wiring-request.md`; no workflow file is edited
  here.
