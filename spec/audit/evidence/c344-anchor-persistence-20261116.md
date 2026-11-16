# C344's anchor-persistence arm — local-devnet FAIL (2026-11-16)

The digest-checked record is the directory `spec/audit/evidence/c344-anchor-persistence-20261116/`
(`README.md`, `manifest.json`, `SHA256SUMS.json`, `raw/`).

The repaired tree (`40b4396`) cannot accept the repair end-to-end yet: the `jail-vote` drill advanced
to height 12 with finality at height 2, then failed sidecar regeneration for block `7e276d01…` on
every propose. `getLiveness` stayed empty and no jail was imposed. The run has **no**
`final scope rejected` lines and no anchor-slot (`04/688c6e…`) drop, so the repaired path is gone from
the log, but the arm never reaches the jail. The same arm on the unmodified `d4b9bfb` base stalls
identically — **C402**.

See the directory README for the exact command, the provenance pins, and the non-claims.
