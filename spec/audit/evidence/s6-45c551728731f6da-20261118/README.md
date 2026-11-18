# s6-45c551728731f6da-20261118

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
| binary | `98c62deb1fdaa588f7c1b369b0d26d9635508a9dca547120fb9b4169356e1f68` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation leaves the pending entry and the same test fails.

PASS log tail:

```text
running 1 test
test native_state::tests::slash_persists_the_pending_map_it_edits ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 375 filtered out; finished in 0.00s

   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 5.46s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
```

## What this does not establish

A current-build regression witness; the live slashing campaign is separate.

## Reproduce

```sh
cargo test -p rchain-rholang --lib slash_persists_the_pending_map_it_edits
```
