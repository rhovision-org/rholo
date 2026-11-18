# s6-cc87a089357b25fa-20261118-fail

**Evidence locality.** `single-host`

## Scope

RSpace candidate selection is sorted-first by content hash, not newest-first insertion order.

The witness is `rspace/src/rspace.rs::candidate_selection_is_sorted_by_content_hash_not_insertion_order`. The mutation is:

```text
-            // matching datum is chosen regardless of insertion order (Law 4/8).
            indexed.sort_by(|a, b| a.0.source.cmp(&b.0.source));
+            // matching datum is chosen regardless of insertion order (Law 4/8).
            indexed.sort_by(|a, b| a.1.cmp(&b.1));
```

## Provenance

| | |
|---|---|
| gate | `S6-cc87a089357b25fa` |
| source | `c87ade74356ebd33f964b10d47942b99a5fe86e8` |
| binary | `e6cb1bbebfead85996bffa46b12d20176abc09dd4bcb218fc8f4cb3b0dbaa8b1` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation restores insertion order and the same test fails.

FAIL log tail:

```text
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    rspace::tests::candidate_selection_is_sorted_by_content_hash_not_insertion_order

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 194 filtered out; finished in 0.00s

   Compiling rchain-rspace v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rspace)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 5.18s
     Running unittests src/lib.rs (target/debug/deps/rchain_rspace-3e8cc9b76677a749)
error: test failed, to rerun pass `-p rchain-rspace --lib`
```

## What this does not establish

A current-build regression witness; a live RSpace campaign is separate.

## Reproduce

```sh
cargo test -p rchain-rspace --lib candidate_selection_is_sorted_by_content_hash_not_insertion_order
```
