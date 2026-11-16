# C402 — a partition stalls finality on the merged base (2026-11-16)

The digest-checked record is the directory `spec/audit/evidence/c402-partition-stall-20261116/`
(`README.md`, `manifest.json`, `SHA256SUMS.json`, `raw/`).

Isolation run: the same `jail-vote` arm against the unmodified `d4b9bfb` image (the #596 merge). It
stalls at height 12 / finalised 2 with the same `regenerated mergeable channels for block ab5b9b84… but
replay computed …` error, so the stall predates C344's `final_scope_rejections` repair. The suspect is
the replay's `participation_for_block` derivation or its pre-state; C402 owns the diagnosis and fix,
after which C344's jail/vote arms can be re-run.

See the directory README for the exact command, the binary pin, and the non-claims.

## Closure — 2026-11-16 (PR #599)

The repair landed in #597 (`d46bff8cd`: admit a multi-parent pre-state when every justification is
the anchor or a block this node validated; seed the checkpoint's committed fringe and anchor closure
at boot; refuse to index a `validation_failed` record with its zero derived fringe). #597's own
`jail-vote` run passed and was left in `target/`, which is not evidence a later reader can open;
#598's end-to-end run on the merged tree is committed:

- `spec/audit/evidence/c401-unsealer-auth-20261116/raw/raw/result.json` — `jailed: true`,
  `bond_untouched_by_jail: true`, `vote_slashed_the_bond: true`; bonds after the vote are
  700/100/100.

That run reaches the liveness response (the jail record is `[0, 10]` and the vote removes the bond),
which is the criterion this row owed ("then re-run C344's jail/vote arms"). The live testnet arms
remain C344's, not C402's. No public-validator operation, deployment, key use or transfer is claimed.
