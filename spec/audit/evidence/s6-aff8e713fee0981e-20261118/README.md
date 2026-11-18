# s6-aff8e713fee0981e-20261118

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
| binary | `44044386d0ab84aa2ae2804dbd512514db96c5f75eb1ae26cbe1c0b8d4c67257` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation accepts a non-bonded sender and the same test fails.

PASS log tail:

```text
running 1 test
test dag::finalizer::tests::check_min_messages_needs_all_bonded_senders ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 97 filtered out; finished in 0.00s

   Compiling rchain-block-storage v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/block-storage)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.66s
     Running unittests src/lib.rs (target/debug/deps/rchain_block_storage-29bd045d5a9311b6)
```

## What this does not establish

A current-build regression witness; the live finality campaign is separate.

## Reproduce

```sh
cargo test -p rchain-block-storage --lib check_min_messages_needs_all_bonded_senders
```
