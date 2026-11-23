# rc2-stall-denominator-20261123-5b9685e

**Evidence locality.** `local-devnet`

## Scope

The local stall arm of RC-2 (`stall-denominator`), on a disposable four-validator Docker devnet at
bonded stakes 100/100/50/50. The question: with exactly 200 of 300 bonded stake live, does height
advance while finality freezes, and does the same deploy settle after the two stopped validators
restart? It measures HAZOP DE-04 (a silent bonded validator keeps its denominator share), DE-05
(the partition leaves neither side a strict supermajority) and DE-06 (the finality gate refuses at
exactly two thirds).

## Provenance

| | |
|---|---|
| gate | `RC-2` |
| source | `5b9685eca38250a9f0d81fb3da2f410328721aec` |
| binary | `a402763636d5c8229f024e70cb574cd6a70b4fadfb36b1cfa3121c3c0854d822` |
| run | local Docker devnet; no GitHub Actions run |
| reference | HAZOP DE-04, DE-05, DE-06 in `docs/src/spec/stall-coverage.md` |
| packaged | `2026-11-23T14:39:05+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The arm's own `check(record)` returned `pass`. The measured series:

- baseline: height 21, finalised 16.
- trigger: validators 2 and 3 stopped (the two 50-stake bonds); live stake is exactly 200/300, which
  is not a strict supermajority.
- stall: the finality gate reported `reason: support`, `supporting: 200`, `total: 300`; finality
  stayed at 16 while height advanced to 26. The gate refuses a strict supermajority, so the measured
  freeze is the refused support, not a pause at the stop. The one or two steps between the stop and
  the first refusal are the silent validators' last messages still supporting the chain; they are
  recorded in `raw/denominator/observations.jsonl` (`warmup`, `warmup_gate`) and are not counted as
  production at 2/3.
- the stalled deploy stayed `includedUnfinalized`; no settlement claimed success while finality was
  frozen.
- restore: both validators restarted; finality resumed (16 -> 32) and the same deploy id settled
  `finalizedSuccess`, with every accepted deploy resolved.

`stall_episode`: `protocol-stall` / `void` / `unsatisfiable-requirement`, R2, Law 52b.

## What this does not establish

A finite local witness, not public evidence and not release readiness. It does not show that the
finality denominator is the bonded set on every build; it shows this candidate at these stakes. It
does not claim that finality cannot advance on the silent validators' last messages — the warm-up
steps are exactly that lag, and the freeze is measured only after the gate refuses for support. It
does not replace `rc-bond-leave` (the bond-set boundary) or the public arms.

## Reproduce

```sh
RNODE_IMAGE=rnode:local tools/devnet.sh build
python3 tools/release-check.py run --claim RC-2 --arm stall-denominator --keep
python3 tools/release-check.py record --claim RC-2 --arm stall-denominator \
  --record target/release-claims/RC-2/stall-denominator
```

Source `5b9685eca38250a9f0d81fb3da2f410328721aec`, binary
`a402763636d5c8229f024e70cb574cd6a70b4fadfb36b1cfa3121c3c0854d822`.
