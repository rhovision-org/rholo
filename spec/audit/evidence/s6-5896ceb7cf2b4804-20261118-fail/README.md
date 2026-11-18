# s6-5896ceb7cf2b4804-20261118-fail

**Evidence locality.** `single-host`

## Scope

The height maximum counts failed justifications; a failed parent still sets the child's bound.

The witness is `casper/src/validate.rs::a_failed_parent_still_sets_its_childs_height`. The mutation is:

```text
-        max_block_number = max_block_number.max(i64::from(meta.block_num));
+        if !meta.validation_failed {
            max_block_number = max_block_number.max(i64::from(meta.block_num));
        }
```

## Provenance

| | |
|---|---|
| gate | `S6-5896ceb7cf2b4804` |
| source | `98e8681f156ab0d171fdd7f727576d25744334f4` |
| binary | `7b3eb4484bb133eb27e5310a82b8f3804c044f0c722cc03a6bbc6db3fec6fd1c` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation reintroduces the skip and the same test fails.

FAIL log tail:

```text
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    validate::effectful_tests::a_failed_parent_still_sets_its_childs_height

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 747 filtered out; finished in 0.00s

   Compiling rchain-casper v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/casper)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 11.19s
     Running unittests src/lib.rs (target/debug/deps/rchain_casper-64d507b1a93523ca)
error: test failed, to rerun pass `-p rchain-casper --lib`
```

## What this does not establish

A current-build regression witness; the historical fork-point rollout is separate.

## Reproduce

```sh
cargo test -p rchain-casper --lib a_failed_parent_still_sets_its_childs_height
```
