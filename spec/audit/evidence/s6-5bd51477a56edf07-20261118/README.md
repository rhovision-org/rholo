# s6-5bd51477a56edf07-20261118

**Evidence locality.** `single-host`

## Scope

Negative deploy cost is rejected at the protobuf uint64 boundary, not wrapped.

The witness is `casper/src/runtime_manager.rs::a_negative_cost_is_refused_before_it_becomes_a_pcost`. The mutation is:

```text
-        cost: u64::try_from(value).map_err(|_| {
            RuntimeRunError::Other(format!("deploy cost is negative: {value}"))
        })?,
+        cost: value as u64,
```

## Provenance

| | |
|---|---|
| gate | `S6-5bd51477a56edf07` |
| source | `c87ade74356ebd33f964b10d47942b99a5fe86e8` |
| binary | `80e901131f572b59aa34a9e1ebdbd07ee5bd04f08d9aa7ad6d08c0823f61d777` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation wraps the negative and the same test fails.

PASS log tail:

```text
running 1 test
test runtime_manager::tests::a_negative_cost_is_refused_before_it_becomes_a_pcost ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 748 filtered out; finished in 0.00s

   Compiling rchain-rspace v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rspace)
   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
   Compiling rchain-casper v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/casper)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 10.02s
     Running unittests src/lib.rs (target/debug/deps/rchain_casper-64d507b1a93523ca)
```

## What this does not establish

A current-build regression witness; the historical replay is separate.

## Reproduce

```sh
cargo test -p rchain-casper --lib a_negative_cost_is_refused_before_it_becomes_a_pcost
```
