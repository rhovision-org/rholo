# s6-91b05027373a3107-20261118-fail

**Evidence locality.** `single-host`

## Scope

The negative control for S6-91b05027373a3107. It bypasses the `PREHASH_LEN` check in
`crypto/src/signatures/secp256k1.rs` and runs the same witness:

```text
-        if data.len() != PREHASH_LEN {
+        if false && data.len() != PREHASH_LEN {
```

The mutation is in `raw/mutation.patch`; `raw/test.log` is the failure.

## Provenance

| | |
|---|---|
| gate | `S6-91b05027373a3107` |
| source | `8b4c6b21ca9c6eea266a73ce9bb8127f914ff510` |
| binary | `3cc143aedabe58a28d1bf89c73f46e221affc54a9f0173ee1398550459c39a5b` |
| run | local operator run (not a GitHub Actions run) |
| reference | none — local run |
| packaged | `2026-11-18T16:49:00+00:00` |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

`raw/test.log` records:

```text
test signatures::secp256k1::tests::a_message_that_is_not_32_bytes_is_refused_not_truncated ... FAILED
thread '...' panicked at crypto/src/signatures/secp256k1.rs:173:9:
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 107 filtered out
```

That is the control working: the unmutated source passes
(`s6-91b05027373a3107-20261118`), while the bypass lets the suffixed message verify over its prefix
and the `!verify_bytes(...)` assertion fails.

## What this does not establish

- It is a negative control for this witness, not a second acceptance run.
- It does not establish that every non-prehash input shape is covered; it exercises the length
  refusal.
- `execution_proven` is false; the record proves the bytes, not that the run happened.

## Reproduce

```sh
# apply raw/mutation.patch, then:
cargo test -p rchain-crypto --lib a_message_that_is_not_32_bytes_is_refused_not_truncated  # exits 101
# restore the source and rerun:
cargo test -p rchain-crypto --lib a_message_that_is_not_32_bytes_is_refused_not_truncated  # exits 0
```
