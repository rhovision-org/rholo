# rc2-validator-set-20261123-38cd50e

**Evidence locality.** `local-devnet`

## Scope

The local validator-set arm of RC-2, on a disposable four-node Docker devnet (three bonded
validators at 100/100/50 with epoch length 10 and quarantine length 20, plus a newcomer that bonds
50). It reads the boundary bond set at the newcomer's activation and after the leave, on every
validator, and asks whether the finality denominator is each boundary's own bond total. It is placed
against HAZOP DE-06 (the finality layer counts exactly two thirds and refuses a strict
supermajority) in `docs/src/spec/stall-coverage.md`.

## Provenance

| | |
|---|---|
| gate | `RC-2` |
| source | `38cd50ecf1d6ab64863eb1a318e781b062ffdc7e` |
| binary | `41ccf63ca66b5575c4b93731f11482e03e9478e2c406e2c32491a7528e901d94` |
| run | local Docker devnet; no GitHub Actions run |
| reference | HAZOP DE-06 in `docs/src/spec/stall-coverage.md` |
| packaged | `2026-11-23T15:57:51+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The arm's own `check(record)` returned `pass`. The measured boundaries:

- activation, height 30: bond total **300**, read on all four validators; the set includes the
  100/100/50 genesis bonds and the newcomer's 50.
- after the leave, height 89: bond total **250**, read on all four validators; validator 2's 50 is
  gone.
- `finality_denominators` is `[300, 250]`, `denominators_match` is true and `active_set_agrees` is
  true on both boundaries.

The record carries 800 finality decision traces: 773 advanced and 27 are support refusals. All 27
refusals report `supporting: 0` on quiet/lockstep rounds; **none is the exact two-thirds
(200 of 300) exposure**. The `/health.finalityGate` samples during the run likewise recorded no
support refusal. DE-06's trigger was therefore **not exercised** in this run, and its placement row
stays `owed`; the existing live DE-06 row is the `stall-denominator` record, which measured it.

`stall_episode` is attached for the classification
(`protocol-stall` / `void` / `unsatisfiable-requirement`, R2, `Law 52b`) with
`two_thirds_support_refusal_observed: false` in its `exercise` block. It is not used to close the
placement.

## What this does not establish

A finite local witness of the bond-set denominators, not public evidence and not release readiness.
It does not reproduce a 200/300 support refusal, and no DE-06 series is emitted because the samples
do not carry a progress step with finality held, a later advance, and the two-thirds refusal. It
does not show that the boundary set can never drift on another timing.

## Reproduce

```sh
RNODE_IMAGE=rnode:pr653 tools/devnet.sh build
python3 tools/bond-leave-campaign.py --out target/bond-leave-campaign/pr653
python3 tools/release-check.py run --claim RC-2 --arm validator-set \
  --campaign-result target/bond-leave-campaign/pr653/result.json
python3 tools/release-check.py record --claim RC-2 --arm validator-set \
  --record target/release-claims/RC-2/validator-set
```

Source `38cd50ecf1d6ab64863eb1a318e781b062ffdc7e`, binary
`41ccf63ca66b5575c4b93731f11482e03e9478e2c406e2c32491a7528e901d94`.
