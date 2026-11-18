# Gate-closure programme

This note sequences the work that moves `spec/audit/mainnet-gates.json` from **111 gates, 0 met**
toward closure. It does not promote a gate by itself; the register and its checker are the
authority.

## The rule

A gate is `met` only when its witness resolves, its negative control is recorded, its evidence is
pinned, and every dependency is met. The first promotion path is
`spec/audit/mainnet-gates.bindings.json`: an authored, complete binding, applied by
`tools/build-mainnet-gates.py` only when the row is eligible (S6: accepted disposition and a
derived category-A effect; HF51: merged/verified ledger item; decision/blocked/precondition/
superseded rows and `TA-C3` are never eligible). The builder refuses an orphan, duplicate, partial
or ineligible binding, and the register carries a met ratchet so a promotion cannot silently
regress.

## First tranche (this unit)

- The promotion machinery and its negative controls.
- `G520-MIGRATION`: `tools/populated-store-migration-drill.py`, a populated-store
  copy/rollback/restart drill with replay qualification and a fresh-genesis control on a disposable
  local devnet.
- Four S6 rows, each already backed by a witness and a declared mutation:
  `S6-25bfdbe2c179c81e` (unknown-only `Expr` refusal), `S6-c7536b90bd39cf71` (produced-value depth
  bound), `S6-91b05027373a3107` (32-byte prehash refusal), `S6-d7dd0a06a5dee537` (negative
  `bind_count` refusal).

The S6 rows are accepted only after the independent reviewer (Patrick Mockridge) confirms the packed
PASS/FAIL records and the review record; until then the dispositions stay `reviewable`.

## Next tranches

1. **S6 batches.** The remaining reviewable rows, grouped by witness family and category-A effect.
   Two rows are held for dedicated witnesses: `S6-212cef89b85456e0` (the proto-boundary assertion is
   embedded in a broad round-trip test) and `S6-2228415e1ac6b83e` (no single witness covers all
   three depth guards). The three S6 `owes` rows need their named tests/controls: 
   `tests/liveness-enforcement.py` is absent for `S6-0963a698bc9c4ee4`; `S6-5651614bf8e3eeb7` and
   `S6-f735095c746556f3` have named tests but no recorded negative control.
2. **HF51 in-repo owes.** `A:shard-config` (`casper/tests/committed_bounds.rs` plus a three-node
   new-genesis/restart/LFS arm; the ledger is still `open` and the wider RCHIP #24 scope is a
   decision), `Audit:parent-limit` (merged; opposing-config and over-limit fixture), and
   `Batch:activation-schedule` (height boundary, mismatch rejection, startup/restart/replay on the
   integrated source). Promote only after the ledger/evidence bar is met.
3. **Decision queue.** The 22 decision rows stay open. This is a tracking queue, not a closure
   path: each row names its owner and the input that would let the owner decide.
   - `@PatrickMockridge`: `A-rev-economics`, `A-validator-rewards`, `A-unbond-throttle`,
     `A-phlo-exchange`, `A-prepaid-rholang`, `A-asset-store`, `B-staking-pools`,
     `B-rev-receipt`, `B-staking-pools-interface`, `Audit-law53-estrangement`,
     `Release-decimal-genesis`, `Audit-merge-resolution-stage4`.
   - `@aixaria0`: `Audit-inactivity-leak`.
   - Jim: `A-double-mint`, `A-hardfork1-migration`, `A-genesis-contract-upgrade`,
     `B-genesis-contract-upgrade`, `A-block-merge`, `A-storage-rent`, `A-decentralized-rspace`,
     `B-babel`, `Batch-governance-schedule`.
   The nine Jim rows also carry unlanded work; a decision alone does not make them `met`.
4. **External campaign queue.** `G584-G1..G6` (the live four-validator window and an independent
   G6 verdict), `G428-M1/M3/M4/M5/M6` (multi-host capacity, off-host bootstrap, finality/recovery,
   the unavailable 840→841 bundle, reproducibility), `C343` (operator read-only B/C/D snapshot),
   `C357` (lockstep campaign and independent category-A verdict), and `TA-C1/C2` (live re-runs).
   These need live or off-host observers, operator archives, independent reviewers, or release
   coordination. Local-devnet arms are prerequisites, not substitutes.

## What this programme does not claim

- No external campaign has run and no operator artefact exists.
- No decision gate is closed; the register refuses a decision witness by construction.
- `TA-C3` is a census, not a pass/fail gate; it cannot go green.
- `--assert-closeable` and the #51 tracker keep exiting 3 while any row is unresolved.
- CI wiring for the new tests is requested in `ci-wiring-request.md`; no workflow file is edited
  here.
