# s6-212cef89b85456e0-20261118-fail

**Evidence locality.** `single-host`

## Scope

BindPattern.freeCount is rejected when negative at the proto boundary.

The witness is `models/src/wire.rs::a_negative_bind_pattern_free_count_is_refused_at_the_proto_boundary`. The mutation is:

```text
-        free_count: i32::from(FreeCount::try_from(p.free_count).map_err(ModelsError::Decode)?),
+        free_count: p.free_count.max(0),
```

## Provenance

| | |
|---|---|
| gate | `S6-212cef89b85456e0` |
| source | `c87ade74356ebd33f964b10d47942b99a5fe86e8` |
| binary | `e740ee13425fa928f558fbbb0e6739a57c226498a1444373b672f3b4aea6b930` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation clamps the negative and the same test fails.

FAIL log tail:

```text
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    wire::differential::round_trips::a_negative_bind_pattern_free_count_is_refused_at_the_proto_boundary

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 193 filtered out; finished in 0.00s

   Compiling rchain-models v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/models)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 3.45s
     Running unittests src/lib.rs (target/debug/deps/rchain_models-ba348ab9afbaf01c)
error: test failed, to rerun pass `-p rchain-models --lib`
```

## What this does not establish

A current-build regression witness; the historical replay is separate.

## Reproduce

```sh
cargo test -p rchain-models --lib a_negative_bind_pattern_free_count_is_refused_at_the_proto_boundary
```
