# C301 Phase-3 tranche — end-to-end consensus check (2026-11-16)

The tranche's two consensus-visible changes — B2's HTTP claim-registry charge and bounds, and C5's
on-chain epoch trigger plus the `InvalidEpochBoundary` validity rule — were checked end to end on a
disposable four-validator devnet built from `9aaa67446` (`rnode:local`, image
`sha256:676040ba30d8cab74f81c25f90ac5e7e54ae49a9c72cc6b3429926cba37b979c`):

```
DEVNET_PREFIX=c301res599 python3 tools/liveness-enforcement-drill.py \
  --arm jail-vote --out target/c301-c5-jailvote-20261116
```

**Result: PASS.** `jailed: true`, `bond_untouched_by_jail: true`, `vote_slashed_the_bond: true`;
bonds after the vote are 700/100/100; the jail record is `[0, 10]`. The chain crossed the epoch
boundaries (`epoch_length: 3`) that the new validation rule governs and still reached the liveness
response, so the rule and the charge did not disturb boot, block production, the jail or the vote.

The tool's `execution_proven` field is `false` as recorded; the three checks above and
`verdict_hint: PASS` are the evidence. No public-validator operation, deployment, key use or
transfer is claimed.

Files: `raw/result.json`, `raw/bootstrap.log`. The devnet was torn down with
`DEVNET_PREFIX=c301res599 tools/devnet.sh down -v`.
