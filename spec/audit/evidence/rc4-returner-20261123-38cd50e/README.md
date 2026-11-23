# rc4-returner-20261123-38cd50e

**Evidence locality.** `local-devnet`

## Scope

The local returner arm of RC-4, on the same disposable devnet as the bond-leave campaign. After the
leave and payout, when the surviving bonded set is a strict supermajority, validator 3 is stopped,
left down while four deploys are driven, and restarted without manual surgery. The arm asks whether
it comes back, catches up to the survivors' tip, and reaches the same finalised head on every
validator. It measures HAZOP DE-11 (a rejoining validator never retries a missing justification and
stays behind) in `docs/src/spec/stall-coverage.md`.

## Provenance

| | |
|---|---|
| gate | `RC-4` |
| source | `38cd50ecf1d6ab64863eb1a318e781b062ffdc7e` |
| binary | `41ccf63ca66b5575c4b93731f11482e03e9478e2c406e2c32491a7528e901d94` |
| run | local Docker devnet; no GitHub Actions run |
| reference | HAZOP DE-11 in `docs/src/spec/stall-coverage.md` |
| packaged | `2026-11-23T15:57:53+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The arm's own `check(record)` returned `pass`. The measured sequence:

- validator 3 was stopped after the payout at height 89; it was down for **30.2 s** while four
  deploys were driven;
- it restarted with a new boot id at height 94, without manual surgery, and reached the survivors'
  tip;
- the common finalised head was height **90**, hash
  `b5e45dab92f099fa1695d338b1593b8e6e7d2d12c00504f5b31fa7fcce06e1c4`; every validator answered
  `is_finalized: true` for it, and `heads_agree` is true;
- the returner's boot trace records a certificate acquisition (`acquisition: certificate`,
  `outcome: certified`, served anchor
  `c826781cf45d7e5b1881eda396f59cc6313a9227825393ed984e6ab3e1e4bae8`, supporting/total stake
  250/250);
- no support refusal was observed while it was down (`gate_refusal: null`), which is expected when
  the survivors hold 200 of 200.

`stall_episode`: `protocol-stall` / `stuck` / `loss-blind-refusal`, R2, `Rchain.System.Stuck`. The
episode's `exercise` block records `trigger_exercised: true`, the 30.2 s downtime, the new boot, the
rejoin and `deviation_observed: false` (the returner retried and rejoined; the safeguard held).

## What this does not establish

A finite local witness, not public evidence and not release readiness. It does not show that a
returner with every possible missing-justification shape retries; it shows this stop/restart gap on
this candidate. It does not re-time the C211 reproducer, and it does not replace the public arms.
The decisive unit falsifier for the dependency set is
`casper/src/blocks/block_receiver.rs::a_block_waits_for_every_parent_not_yet_validated`, whose
control releases the block on the old not-stored set.

## Reproduce

```sh
RNODE_IMAGE=rnode:pr653 tools/devnet.sh build
python3 tools/bond-leave-campaign.py --out target/bond-leave-campaign/pr653
python3 tools/release-check.py run --claim RC-4 --arm returner \
  --campaign-result target/bond-leave-campaign/pr653/result.json
python3 tools/release-check.py record --claim RC-4 --arm returner \
  --record target/release-claims/RC-4/returner
```

Source `38cd50ecf1d6ab64863eb1a318e781b062ffdc7e`, binary
`41ccf63ca66b5575c4b93731f11482e03e9478e2c406e2c32491a7528e901d94`.
