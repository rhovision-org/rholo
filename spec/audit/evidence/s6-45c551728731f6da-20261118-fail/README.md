# s6-45c551728731f6da-20261118-fail

**Evidence locality.** `single-host`

## Scope

A slashed validator that had a staged withdrawal is removed from the pending map.

The witness is `rholang/src/native_state.rs::slash_persists_the_pending_map_it_edits`. The mutation is:

```text
-        pending.remove(validator);
        let accrued = committed.remove(validator)
+        let accrued = committed.remove(validator)
```

## Provenance

| | |
|---|---|
| gate | `S6-45c551728731f6da` |
| source | `714e5d9346ed554ea29fe7dba91d635b84c98268` |
| binary | `c35bb9bf4e2ff2411bb4ca56f68ebc298019f55bd6fb793e0cf3c580d4769b0a` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation leaves the pending entry and the same test fails.

FAIL log tail:

```text
     |
2781 |         let mut pending = self.pending_withdrawers().await?;
     |             ----^^^^^^^
     |             |
     |             help: remove this `mut`
     |
     = note: `#[warn(unused_mut)]` (part of `#[warn(unused)]`) on by default

warning: `rchain-rholang` (lib test) generated 1 warning (run `cargo fix --lib -p rchain-rholang --tests` to apply 1 suggestion)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 7.70s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
error: test failed, to rerun pass `-p rchain-rholang --lib`
```

## What this does not establish

A current-build regression witness; the live slashing campaign is separate.

## Reproduce

```sh
cargo test -p rchain-rholang --lib slash_persists_the_pending_map_it_edits
```
