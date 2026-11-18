# s6-212cef89b85456e0-20261118

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
| binary | `28b67825b898f5cb80e7269728afa45d18a351d319ff412cd7eba34c364aa787` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation clamps the negative and the same test fails.

PASS log tail:

```text
running 1 test
test wire::differential::round_trips::a_negative_bind_pattern_free_count_is_refused_at_the_proto_boundary ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 193 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.22s
     Running unittests src/lib.rs (target/debug/deps/rchain_models-ba348ab9afbaf01c)
```

## What this does not establish

A current-build regression witness; the historical replay is separate.

## Reproduce

```sh
cargo test -p rchain-models --lib a_negative_bind_pattern_free_count_is_refused_at_the_proto_boundary
```
