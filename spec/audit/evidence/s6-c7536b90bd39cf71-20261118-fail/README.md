# s6-c7536b90bd39cf71-20261118-fail

**Evidence locality.** `single-host`

## Scope

The negative control for S6-c7536b90bd39cf71. It raises the produced-value bound in
`rholang/src/storage.rs` and runs the same witness:

```text
-const MAX_VALUE_DEPTH: usize = if 256 < crate::parser::MAX_AST_DEPTH {
+const MAX_VALUE_DEPTH: usize = if 1024 < crate::parser::MAX_AST_DEPTH {
```

The mutation is in `raw/mutation.patch`; `raw/test.log` is the failure. The 400-deep fold now passes
the bound and the worker stack overflows, as the pre-guard code did.

## Provenance

| | |
|---|---|
| gate | `S6-c7536b90bd39cf71` |
| source | `8b4c6b21ca9c6eea266a73ce9bb8127f914ff510` |
| binary | `250b445c5fac8a8285cf5e311d7d53e258d51b8521ff57d36e310d725846a582` |
| run | local operator run (not a GitHub Actions run) |
| reference | none — local run |
| packaged | `2026-11-18T16:53:00+00:00` |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

`raw/test.log` records:

```text
thread 'tokio-rt-worker' (1467742) has overflowed its stack
fatal runtime error: stack overflow, aborting
error: test failed ... (signal: 6, SIGABRT: process abort signal)
```

That is the control working: the unmutated source passes
(`s6-c7536b90bd39cf71-20261118`), while the raised bound lets the 400-deep runtime value reach the
space and the worker stack overflows. A SIGABRT is a non-zero test exit; the failure was recorded
once and not retried into green.

## What this does not establish

- It is a negative control for this witness, not a second acceptance run.
- It does not establish that every value shape above the bound aborts; it exercises the fold shape
  the witness uses.
- `execution_proven` is false; the record proves the bytes, not that the run happened.

## Reproduce

```sh
# apply raw/mutation.patch, then:
cargo test -p rchain-rholang --test deep_value_bound -- \
  --exact a_value_built_at_runtime_is_refused_above_the_bound   # exits 101 (SIGABRT)
# restore the source and rerun:
cargo test -p rchain-rholang --test deep_value_bound -- \
  --exact a_value_built_at_runtime_is_refused_above_the_bound   # exits 0
```
