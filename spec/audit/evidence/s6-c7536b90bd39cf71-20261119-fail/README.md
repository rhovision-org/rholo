# s6-c7536b90bd39cf71-20261119-fail

**Evidence locality.** `single-host`

## Scope

The negative control for S6-c7536b90bd39cf71, redefined after the 2026-11-18 record's mutation (raise
`MAX_VALUE_DEPTH` to 1024) failed by SIGABRT rather than by a libtest assertion. This run changes the
**refusal message** in `rholang/src/storage.rs` and asks one question: does the witness fail when the
depth guard still fires but no longer names its reason?

```text
-"a produced value is nested too deeply".to_string(),
+"a produced value was refused".to_string(),
```

The mutation is in `raw/mutation.patch`; `raw/test.log` is the failure. The depth guard itself is
unchanged: `fold 400` is still refused, and the witness fails on its third assertion — "the refusal
has to say what it refused" — with a clean `libtest FAILED` (exit 101), not an abort.

## Provenance

| | |
|---|---|
| gate | `S6-c7536b90bd39cf71` |
| source | `de2d72fdae0a7c2fb98945cd6e6ac460bba139e5` |
| binary | `34262a71665f17dbe5258e61006ec6708db8a5f74c827bda2e753a8e432599c0` |
| run | local operator run (not a GitHub Actions run) |
| reference | none — local run |
| packaged | `2026-11-19T17:04:49+00:00` |
| verdict | **FAIL** |

The binary pin is the SHA-256 of the `deep_value_bound` test binary the run executed
(`target/debug/deps/deep_value_bound-6b71737a146a265e`), named in `raw/test.log`. Every file listed
in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is false**: the digests prove
the bytes are the bytes, not that the run happened or that its verdict is sound.

## Result

`raw/test.log` records:

```text
fold 20 -> Ok(20)
fold 400 -> Err("ReduceError(\"a produced value was refused\")")
thread '<unnamed>' panicked at rholang/tests/deep_value_bound.rs:97:21:
the refusal has to say what it refused: ReduceError("a produced value was refused")
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1 filtered out
```

The witness's shallow case still evaluates, the deep case is still refused, and the reason assertion
fails because the mutation removed the reason. This is the control S6-c7536b90bd39cf71 declares:
a witness that passes on this code would not be asserting the refusal vocabulary.

## What this does not establish

- It does not re-prove the numeric depth bound (256) or the abort margin; it exercises the witness's
  reason assertion.
- It is a finite negative control, not a universal safety theorem about runtime-built values.
- It does not prove the run happened: `execution_proven` is false, and the digests prove only the
  bytes.
- It does not claim the 2026-11-18 record was wrong; that record documents the same witness failing
  by SIGABRT under a different mutation, and it is retained as provenance.

## Reproduce

```sh
git checkout de2d72fdae0a7c2fb98945cd6e6ac460bba139e5
git apply raw/mutation.patch
cargo test --locked -p rchain-rholang --test deep_value_bound \
  a_value_built_at_runtime_is_refused_above_the_bound
git checkout -- rholang/src/storage.rs
```
