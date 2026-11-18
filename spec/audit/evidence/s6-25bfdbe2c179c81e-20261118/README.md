# s6-25bfdbe2c179c81e-20261118

**Evidence locality.** `single-host`

## Scope

The S6-25bfdbe2c179c81e witness: an instance-less or unknown-only `Expr` is refused with
`Malformed("expr_instance")` rather than decoded as `false`. The test decodes real wire bytes and
requires the refusal; this is the current-code regression witness for that invariant.

## Provenance

| | |
|---|---|
| gate | `S6-25bfdbe2c179c81e` |
| source | `8b4c6b21ca9c6eea266a73ce9bb8127f914ff510` |
| binary | `4d5cd904ca50217ea7a9d33a4cea4a829cabefd38061bfef83cb275c949385f0` |
| run | local operator run (not a GitHub Actions run) |
| reference | none — local run |
| packaged | `2026-11-18T16:44:00+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

`raw/test.log` records:

```text
running 1 test
test an_unknown_only_expression_is_refused_after_real_wire_decode ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out
```

The negative control is `s6-25bfdbe2c179c81e-20261118-fail`: mutating
`models/src/wire.rs`'s `None` arm to return `a::Expr::GBool(false)` makes the same test fail.

## What this does not establish

- It is a current-code regression witness, not a historical Scala-era replay or a scan of blocks
  already accepted by an earlier build.
- It covers this construct on this source and binary; it is not a universal wire-format theorem.
- A coordinated validator/client rollout remains a separate release obligation.
- `execution_proven` is false; the record proves the bytes, not that the run happened.

## Reproduce

```sh
cargo test -p rchain-models --test expr_refusal -- \
  --exact an_unknown_only_expression_is_refused_after_real_wire_decode
python3 tools/evidence-pack.py verify \
  --record spec/audit/evidence/s6-25bfdbe2c179c81e-20261118 --require-pins
```
