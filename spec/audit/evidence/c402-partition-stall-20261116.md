# C402 — a partition stalls finality on the merged base (2026-11-16)

The digest-checked record is the directory `spec/audit/evidence/c402-partition-stall-20261116/`
(`README.md`, `manifest.json`, `SHA256SUMS.json`, `raw/`).

Isolation run: the same `jail-vote` arm against the unmodified `d4b9bfb` image (the #596 merge). It
stalls at height 12 / finalised 2 with the same `regenerated mergeable channels for block ab5b9b84… but
replay computed …` error, so the stall predates C344's `final_scope_rejections` repair. The suspect is
the replay's `participation_for_block` derivation or its pre-state; C402 owns the diagnosis and fix,
after which C344's jail/vote arms can be re-run.

See the directory README for the exact command, the binary pin, and the non-claims.
