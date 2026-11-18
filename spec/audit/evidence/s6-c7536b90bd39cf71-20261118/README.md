# s6-c7536b90bd39cf71-20261118

**Evidence locality.** `single-host`

## Scope

The S6-c7536b90bd39cf71 witness: a produced value is depth-bounded at `MAX_VALUE_DEPTH = 256` in
`rholang/src/storage.rs`, on both produce paths. The test builds a value at runtime, requires a
shallow fold to evaluate and a 400-deep fold to be refused with the named reason, on the node's own
32 MiB worker stack.

## Provenance

| | |
|---|---|
| gate | `S6-c7536b90bd39cf71` |
| source | `8b4c6b21ca9c6eea266a73ce9bb8127f914ff510` |
| binary | `122189f08dd425c970464707e3f7c6f3669e30c220ccff731935df5f2a7ffbd0` |
| run | local operator run (not a GitHub Actions run) |
| reference | none — local run |
| packaged | `2026-11-18T16:51:00+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

`raw/test.log` records:

```text
running 1 test
test a_value_built_at_runtime_is_refused_above_the_bound ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out (112.44s)
```

The negative control is `s6-c7536b90bd39cf71-20261118-fail`: raising the depth bound makes the
400-deep value reach the space (or abort as the pre-guard code did) and the same test fail.

## What this does not establish

- It is a current-code regression witness, not a historical replay of values accepted before the
  guard existed.
- The row's hard-fork note and the fork-point/validator rollout remain separate obligations.
- The witness's 400-deep fold is above the 256 bound; it does not measure every value shape.
- `execution_proven` is false; the record proves the bytes, not that the run happened.

## Reproduce

```sh
cargo test -p rchain-rholang --test deep_value_bound -- \
  --exact a_value_built_at_runtime_is_refused_above_the_bound
python3 tools/evidence-pack.py verify \
  --record spec/audit/evidence/s6-c7536b90bd39cf71-20261118 --require-pins
```
