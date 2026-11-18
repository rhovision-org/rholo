# s6-d7dd0a06a5dee537-20261118

**Evidence locality.** `single-host`

## Scope

The S6-d7dd0a06a5dee537 witness: `New.bind_count` and `Receive.bind_count` are `FreeCount` carriers
and a negative count is refused at the proto boundary. The test feeds a negative count to both
decoders and requires an error from each.

## Provenance

| | |
|---|---|
| gate | `S6-d7dd0a06a5dee537` |
| source | `8b4c6b21ca9c6eea266a73ce9bb8127f914ff510` |
| binary | `284ceca58b39178508d3811a670134ea3930c2be56ea6c58e61fa33a9b25e29e` |
| run | local operator run (not a GitHub Actions run) |
| reference | none — local run |
| packaged | `2026-11-18T16:46:00+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

`raw/test.log` records:

```text
running 1 test
test wire::differential::round_trips::a_negative_bind_count_is_refused_at_the_wire_boundary ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 191 filtered out
```

The negative control is `s6-d7dd0a06a5dee537-20261118-fail`: restoring the clamp on the first
decoder makes the same test fail.

## What this does not establish

- It is a current-code regression witness, not a scan of historical blocks for malformed counts.
- It covers the two decoder boundaries, not every later reader of a bind count.
- A coordinated validator/client rollout remains a separate release obligation.
- `execution_proven` is false; the record proves the bytes, not that the run happened.

## Reproduce

```sh
cargo test -p rchain-models --lib a_negative_bind_count_is_refused_at_the_wire_boundary
python3 tools/evidence-pack.py verify \
  --record spec/audit/evidence/s6-d7dd0a06a5dee537-20261118 --require-pins
```
