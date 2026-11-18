# s6-25bfdbe2c179c81e-20261118-fail

**Evidence locality.** `single-host`

## Scope

The negative control for S6-25bfdbe2c179c81e. It changes `models/src/wire.rs`'s instance-less
`Expr` arm from the `Malformed("expr_instance")` refusal to `a::Expr::GBool(false)` and runs the
same witness:

```text
-        None => return Err(ModelsError::Malformed("expr_instance")),
+        None => a::Expr::GBool(false),
```

The mutation is recorded in `raw/mutation.patch`; `raw/test.log` is the failure. The test bytes are
unchanged.

## Provenance

| | |
|---|---|
| gate | `S6-25bfdbe2c179c81e` |
| source | `8b4c6b21ca9c6eea266a73ce9bb8127f914ff510` |
| binary | `d73cdabb9e08113640070261595a9cc7bdb808dd781b54718bd88fcc925e982d` |
| run | local operator run (not a GitHub Actions run) |
| reference | none — local run |
| packaged | `2026-11-18T16:45:00+00:00` |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

`raw/test.log` records:

```text
test an_unknown_only_expression_is_refused_after_real_wire_decode ... FAILED
thread 'an_unknown_only_expression_is_refused_after_real_wire_decode' panicked at models/tests/expr_refusal.rs:18:10:
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 4 filtered out
```

That is the control working: the unmutated source passes
(`s6-25bfdbe2c179c81e-20261118`), while the mutated source decodes the unknown-only `Expr` as
`false` and the refusal assertion fails.

## What this does not establish

- It is a negative control for this witness, not a second acceptance run.
- It does not establish that every malformed `Expr` shape is caught; it exercises the
  instance-less/unknown-only arm.
- `execution_proven` is false; the record proves the bytes, not that the run happened.

## Reproduce

```sh
# apply raw/mutation.patch, then:
cargo test -p rchain-models --test expr_refusal -- \
  --exact an_unknown_only_expression_is_refused_after_real_wire_decode   # exits 101
# restore the source and rerun:
cargo test -p rchain-models --test expr_refusal -- \
  --exact an_unknown_only_expression_is_refused_after_real_wire_decode   # exits 0
```
