# s6-d7dd0a06a5dee537-20261118-fail

**Evidence locality.** `single-host`

## Scope

The negative control for S6-d7dd0a06a5dee537. It restores the clamp on the first (`Receive`)
decoder in `models/src/wire.rs` and runs the same witness:

```text
-        bind_count: FreeCount::try_from(p.bind_count).map_err(ModelsError::Decode)?,
+        bind_count: FreeCount::new(p.bind_count).unwrap_or(FreeCount::ZERO),
```

Only the first of two identical occurrences is mutated; the control description in the register
records that. The mutation is in `raw/mutation.patch`; `raw/test.log` is the failure.

## Provenance

| | |
|---|---|
| gate | `S6-d7dd0a06a5dee537` |
| source | `8b4c6b21ca9c6eea266a73ce9bb8127f914ff510` |
| binary | `aa6b3c53f4a76de264eacf30b0fbf6e84bb1aa6e087f5b19d66e1e79036392d8` |
| run | local operator run (not a GitHub Actions run) |
| reference | none — local run |
| packaged | `2026-11-18T16:47:00+00:00` |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

`raw/test.log` records:

```text
test wire::differential::round_trips::a_negative_bind_count_is_refused_at_the_wire_boundary ... FAILED
thread '...' panicked at models/src/wire.rs:1721:13:
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 191 filtered out
```

That is the control working: the unmutated source passes
(`s6-d7dd0a06a5dee537-20261118`), while the clamped decoder accepts the negative count and the first
`is_err()` assertion fails.

## What this does not establish

- It is a negative control for this witness, not a second acceptance run.
- It does not establish that every malformed count path is caught; it exercises the receive decoder.
- `execution_proven` is false; the record proves the bytes, not that the run happened.

## Reproduce

```sh
# apply raw/mutation.patch, then:
cargo test -p rchain-models --lib a_negative_bind_count_is_refused_at_the_wire_boundary  # exits 101
# restore the source and rerun:
cargo test -p rchain-models --lib a_negative_bind_count_is_refused_at_the_wire_boundary  # exits 0
```
