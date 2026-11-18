# s6-91b05027373a3107-20261118

**Evidence locality.** `single-host`

## Scope

The S6-91b05027373a3107 witness: `Secp256k1::verify_bytes` refuses a message that is not the
32-byte prehash. The test signs a 32-byte prehash, verifies a suffixed message and requires
`false`; the dependency truncates a longer message to its leftmost 32 bytes, so the refusal is what
prevents that truncation from verifying.

## Provenance

| | |
|---|---|
| gate | `S6-91b05027373a3107` |
| source | `8b4c6b21ca9c6eea266a73ce9bb8127f914ff510` |
| binary | `b5816de4f92df77a9953898f2eb93dd6b94550a1bc98ffa1c3d0c2832417c6e5` |
| run | local operator run (not a GitHub Actions run) |
| reference | none — local run |
| packaged | `2026-11-18T16:48:00+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

`raw/test.log` records:

```text
running 1 test
test signatures::secp256k1::tests::a_message_that_is_not_32_bytes_is_refused_not_truncated ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 107 filtered out
```

The negative control is `s6-91b05027373a3107-20261118-fail`: bypassing the `PREHASH_LEN` check makes
the suffixed message verify over its prefix and the same test fail.

## What this does not establish

- It is a current-code regression witness, not a historical validation replay.
- The C134 oracle ambiguity about the Scala comparison remains recorded and is not discharged here.
- A coordinated validator/client rollout remains a separate release obligation.
- `execution_proven` is false; the record proves the bytes, not that the run happened.

## Reproduce

```sh
cargo test -p rchain-crypto --lib a_message_that_is_not_32_bytes_is_refused_not_truncated
python3 tools/evidence-pack.py verify \
  --record spec/audit/evidence/s6-91b05027373a3107-20261118 --require-pins
```
