# s6-aff8e713fee0981e-20261118-fail

**Evidence locality.** `single-host`

## Scope

The minimum-message sender set must equal the bonded set, not merely match its size.

The witness is `block-storage/src/dag/finalizer.rs::check_min_messages_needs_all_bonded_senders`. The mutation is:

```text
-        min_msgs.len() == bonds_map.len() && minimum_senders == bonded_senders
+        min_msgs.len() == bonds_map.len() && minimum_senders.len() == bonded_senders.len()
```

## Provenance

| | |
|---|---|
| gate | `S6-aff8e713fee0981e` |
| source | `22f17d10310859262924799a5f594ed6c12c13ba` |
| binary | `436c7dd778331fe50c849e705b8ebc5b2ab4647281260bf20c4f0f65e44c7864` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation accepts a non-bonded sender and the same test fails.

FAIL log tail:

```text
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    dag::finalizer::tests::check_min_messages_needs_all_bonded_senders

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 97 filtered out; finished in 0.00s

   Compiling rchain-block-storage v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/block-storage)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.72s
     Running unittests src/lib.rs (target/debug/deps/rchain_block_storage-29bd045d5a9311b6)
error: test failed, to rerun pass `-p rchain-block-storage --lib`
```

## What this does not establish

A current-build regression witness; the live finality campaign is separate.

## Reproduce

```sh
cargo test -p rchain-block-storage --lib check_min_messages_needs_all_bonded_senders
```
