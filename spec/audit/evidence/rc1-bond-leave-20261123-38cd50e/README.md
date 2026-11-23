# rc1-bond-leave-20261123-38cd50e

**Evidence locality.** `local-devnet`

## Scope

The local bond-leave arm of RC-1, on a disposable three-validator Docker devnet with bonded stakes
100/100/50, epoch length 10 and quarantine length 20. A fourth key (validator 3) is admitted,
funded, bonds 50 and activates at an epoch boundary; a marker is written and read at every
validator's finalised head; validator 2 (the 50-stake bond) withdraws, the boundary activates the
leave, and its vault is credited after quarantine. The arm asks whether the bond, the activation,
the withdrawal and the payout all settle truthfully, and whether a finalised-floor refusal is a
loss. It measures HAZOP DE-02 (a finalised-floor refusal repeats and has no inverse) in
`docs/src/spec/stall-coverage.md`.

## Provenance

| | |
|---|---|
| gate | `RC-1` |
| source | `38cd50ecf1d6ab64863eb1a318e781b062ffdc7e` |
| binary | `41ccf63ca66b5575c4b93731f11482e03e9478e2c406e2c32491a7528e901d94` |
| run | local Docker devnet; no GitHub Actions run |
| reference | HAZOP DE-02 in `docs/src/spec/stall-coverage.md` |
| packaged | `2026-11-23T15:57:49+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The arm's own `check(record)` returned `pass`. The measured sequence:

- the newcomer bonded 50 and activated at height 30, producing its first block at 30;
- the withdrawal was requested at 54, its quarantine deadline was 80, and the payout was read at
  86; validator 2's vault moved from 100,000,000 to 100,200,617 and `paid_after_quarantine` is
  true;
- the marker was retained at the finalised head on all four validators, and the withdrawal's
  settlement read `stage: finalizedSuccess`, `settlementConfirmed: true` on all four;
- 256 merge traces carried a finalised-floor rejection; every accepted deploy settled and the payout
  landed. **The no-inverse condition was not observed**: the refusals had an inverse, which is the
  R3 safeguard.

`stall_episode`: `protocol-stall` / `terminal` / `loss-blind-refusal`, R3,
`Rchain.FinalizedMerge.a_finalised_deploy_is_not_in_the_ledger`. The episode's `exercise` block
carries the measured heights, the 256 refusals and `deviation_observed: false`.

## What this does not establish

A finite local witness, not public evidence and not release readiness. It does not reproduce a
finalised-floor refusal that repeats without an inverse; it measures the refusal surface and the
safeguard under a real bond/leave/payout. It does not show that every refusal class is classified
correctly on every build. The decisive unit falsifier is
`casper/src/merging.rs::a_duplicate_rejection_is_not_a_loss`.

## Reproduce

```sh
RNODE_IMAGE=rnode:pr653 tools/devnet.sh build
python3 tools/bond-leave-campaign.py --out target/bond-leave-campaign/pr653
python3 tools/release-check.py run --claim RC-1 --arm bond-leave \
  --campaign-result target/bond-leave-campaign/pr653/result.json
python3 tools/release-check.py record --claim RC-1 --arm bond-leave \
  --record target/release-claims/RC-1/bond-leave
```

Source `38cd50ecf1d6ab64863eb1a318e781b062ffdc7e`, binary
`41ccf63ca66b5575c4b93731f11482e03e9478e2c406e2c32491a7528e901d94`.
