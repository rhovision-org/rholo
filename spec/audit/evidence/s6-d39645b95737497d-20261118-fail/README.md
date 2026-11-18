# s6-d39645b95737497d-20261118-fail

**Evidence locality.** `single-host`

## Scope

The node's block metadata carries the slashable flag (proto field 22); it must survive the store round trip.

The witness is `models/src/block_metadata.rs::a_slashable_flag_survives_the_store_round_trip`. The mutation is:

```text
-            slashable: self.slashable,
+            slashable: false,
```

## Provenance

| | |
|---|---|
| gate | `S6-d39645b95737497d` |
| source | `22f17d10310859262924799a5f594ed6c12c13ba` |
| binary | `758f9e5b81c6f9ebe51d96873fab9b270b66836b9b45f0f88974b63872dbb0b4` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation loses the flag and the same test fails.

FAIL log tail:

```text
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    block_metadata::tests::a_slashable_flag_survives_the_store_round_trip

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 192 filtered out; finished in 0.00s

   Compiling rchain-models v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/models)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 3.22s
     Running unittests src/lib.rs (target/debug/deps/rchain_models-ba348ab9afbaf01c)
error: test failed, to rerun pass `-p rchain-models --lib`
```

## What this does not establish

A current-build regression witness; it does not replay historical block metadata.

## Reproduce

```sh
cargo test -p rchain-models --lib a_slashable_flag_survives_the_store_round_trip
```
