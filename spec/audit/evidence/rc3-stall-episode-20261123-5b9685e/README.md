# rc3-stall-episode-20261123-5b9685e

**Evidence locality.** `local-devnet`

## Scope

The local stall arm of RC-3 (`stall-episode`), on the same disposable four-validator devnet at
bonded stakes 100/100/50/50. The question: while finality is frozen at exactly 200 of 300 live, is
the stall visible, and after the two validators restart does the episode survive the read that would
otherwise report "none"? It measures HAZOP DE-09 (availability read after the observation window
reports a stall that ended as none).

## Provenance

| | |
|---|---|
| gate | `RC-3` |
| source | `5b9685eca38250a9f0d81fb3da2f410328721aec` |
| binary | `a402763636d5c8229f024e70cb574cd6a70b4fadfb36b1cfa3121c3c0854d822` |
| run | local Docker devnet; no GitHub Actions run |
| reference | HAZOP DE-09 in `docs/src/spec/stall-coverage.md` |
| packaged | `2026-11-23T14:39:06+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The arm's own `check(record)` returned `pass`. The measured episode surface, per live validator:

- baseline and the no-trigger control (after the ordinary-lag arm, before the stop): 2 episodes,
  `finalityStalled: false`, counters durable.
- during the stall: 3 episodes, `finalityStalled: true`, counters durable.
- after the restore: 3 episodes, `finalityStalled: false`, counters durable — the live boolean has
  cleared while the persisted count still names the episode. That is the DE-09 reading: a later
  read cannot honestly say the shard never stalled.
- the stalled deploy settled `finalizedSuccess` after the restart, and finality resumed
  (16 -> 33).

`stall_episode`: `protocol-wait` / `historic` / `historic-predicate`, R2, `Rchain.traceRefines`.

## What this does not establish

A finite local witness, not public evidence and not readiness. It does not claim that
`finalityStalled` is readable at every instant of a stall; the claim is that the persisted count
survives the restore, and the run shows both the live boolean and the count moving. It does not
cover the public availability arm (RC-3/availability) or the RC-6 availability arm.

## Reproduce

```sh
RNODE_IMAGE=rnode:local tools/devnet.sh build
python3 tools/release-check.py run --claim RC-3 --arm stall-episode --keep
python3 tools/release-check.py record --claim RC-3 --arm stall-episode \
  --record target/release-claims/RC-3/stall-episode
```

Source `5b9685eca38250a9f0d81fb3da2f410328721aec`, binary
`a402763636d5c8229f024e70cb574cd6a70b4fadfb36b1cfa3121c3c0854d822`.
