# C301 — the roster review's consensus check (2026-11-16)

The roster review's fixes — uncharged native writes across the registry, qucalc, ERTP, txn and
vault; censure's charge universe; the vault destination rule; the txn coordinator length;
`rev:address`'s wrong-type refusal — are consensus-visible: they change what a deploy costs and
which inputs are refused. They were checked end to end on a disposable four-validator devnet built
from `ca0902bf5` (`rnode:local`, image
`sha256:083143a606aa661e3a696e6c0f2d8a02bf1d4782838a73cc5f0231741b5742d4`):

```
DEVNET_PREFIX=c301rev599 python3 tools/liveness-enforcement-drill.py \
  --arm jail-vote --out target/c301-review-jailvote-20261116
```

**Result: PASS.** `jailed: true`, `bond_untouched_by_jail: true`, `vote_slashed_the_bond: true`;
bonds after the vote are 700/100/100; the jail record is `[0, 10]`. The chain crossed the epoch-3
boundaries the C5 rule governs under the review's charges and still reached the liveness response,
so the charging and the new refusals did not disturb boot, block production, the jail or the vote.

The tool's `execution_proven` field is `false` as recorded; the three checks above and
`verdict_hint: PASS` are the evidence. No public-validator operation, deployment, key use or
transfer is claimed.

The earlier B2/C5 run is `spec/audit/evidence/c301-phase3-20261116/` (source `9aaa67446`); this run
is the review-fix head. The devnet was torn down with `DEVNET_PREFIX=c301rev599 tools/devnet.sh
down -v`.
