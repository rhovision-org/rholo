# rc2-local-20261120-81ee948

**Evidence locality.** `local-devnet`

## Scope

The RC-2 baseline local arm: a four-validator local devnet, two partition/rejoin cycles with a
**producing victim** (victim 3, then victim 1). The question is whether the online 3/4 supermajority
keeps finalising while a 1/4 victim produces behind a frozen finalised fence, whether the victim
rejoins the same finalised anchor, and whether every finalised block is finalised by every validator
at or above its height with no regression.

## Provenance

| | |
|---|---|
| gate | `RC-2` |
| source | `81ee9480bc4873ea9a1d35ab27e8c267b23d1bd0` |
| binary | `cba8680143aeb7e408f3a9fac68a5472d7efd88aebf572eb9f30310467acded4` |
| run | local harness; no upstream actions run |
| reference | none |
| packaged | `2026-11-20T23:03:42+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

Both cycles passed. The victims rotated (3 then 1) and each was a producing victim: its tip moved
while its finalised fence stayed settled, and every validator retained the healing anchor (4/4 per
cycle). Both missing ranges crossed an epoch boundary. The online 3/4 supermajority finalised the
three-height advance in each cycle; the containment read covered every validator at or above each
sampled finalised block (three consecutive `true` polls), and there was no finalised regression.
The record carries 1,888 finality traces; `denominator_ok`, `support_ok` and `partition_ok` hold on
them (the whole bonded map is the quorum denominator, and each advance has `3·supporting >
2·quorum`).

The run includes the producer round-escape fix: the escaping block justifies every sender's live
latest message rather than the frozen round's stale peers. It is the code that made the online
supermajority finalise; the Rust test `escape_uses_each_senders_live_latest_message` pins the parent
policy.

## What this does not establish

A local mechanism check on a four-validator devnet, not the public, multi-host, equivocation or
validator-set arms. It is a finite witness for two cycles, not a universal safety theorem; it
assumes the ≤1/3 Byzantine/equivocation bound. The other four RC-2 arms remain open.

## Reproduce

```sh
python3 tools/release-check.py run --claim RC-2 --arm local
```

with `rnode:local` built at `81ee9480bc4873ea9a1d35ab27e8c267b23d1bd0` (binary
`cba86801…`). The arm boots its own four-validator devnet, runs the two partition/rejoin cycles, and
tears it down.
