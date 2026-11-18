# s6-cc87a089357b25fa-20261118

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
| binary | `4f4235033a6edc639787c6a0ce3d5ab0f862efcd95e5261c43e77fbc0bb46a70` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation restores insertion order and the same test fails.

PASS log tail:

```text
running 1 test
test rspace::tests::candidate_selection_is_sorted_by_content_hash_not_insertion_order ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 194 filtered out; finished in 0.01s

   Compiling rchain-rspace v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rspace)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 9.31s
     Running unittests src/lib.rs (target/debug/deps/rchain_rspace-3e8cc9b76677a749)
```

## What this does not establish

A current-build regression witness; a live RSpace campaign is separate.

## Reproduce

```sh
cargo test -p rchain-rspace --lib candidate_selection_is_sorted_by_content_hash_not_insertion_order
```
