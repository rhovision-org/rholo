# s6-de66e3b5b934584f-20261118-fail

**Evidence locality.** `single-host`

## Scope

The full-partition filter is containment, not equality: a seer outside the partition does not invalidate support.

The witness is `block-storage/src/dag/finalizer.rs::the_seer_test_is_containment_of_the_partition`. The mutation is:

```text
-    !seen_by.is_empty() && seen_by.values().all(|v| must_be_seen.is_subset(v))
+    !seen_by.is_empty() && seen_by.values().all(|v| v == must_be_seen)
```

## Provenance

| | |
|---|---|
| gate | `S6-de66e3b5b934584f` |
| source | `22f17d10310859262924799a5f594ed6c12c13ba` |
| binary | `efa3e10695ccf828603b9936618a87cf59f78b28eea866dc8cc0409c60406772` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation rejects the containing seer set and the same test fails.

FAIL log tail:

```text
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    dag::finalizer::tests::the_seer_test_is_containment_of_the_partition

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 97 filtered out; finished in 0.00s

   Compiling rchain-block-storage v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/block-storage)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.09s
     Running unittests src/lib.rs (target/debug/deps/rchain_block_storage-29bd045d5a9311b6)
error: test failed, to rerun pass `-p rchain-block-storage --lib`
```

## What this does not establish

A current-build regression witness; the partition campaign is separate.

## Reproduce

```sh
cargo test -p rchain-block-storage --lib the_seer_test_is_containment_of_the_partition
```
