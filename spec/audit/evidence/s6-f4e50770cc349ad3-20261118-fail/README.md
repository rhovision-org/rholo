# s6-f4e50770cc349ad3-20261118-fail

**Evidence locality.** `single-host`

## Scope

The supermajority predicate is the exact integer 3*stake > 2*total; equality is not a supermajority.

The witness is `node/tests/lean_stake_corpus.rs::the_node_agrees_with_the_model_on_every_supermajority_pair`. The mutation is:

```text
-    stake * 3 > total_stake * 2
+    stake * 3 >= total_stake * 2
```

## Provenance

| | |
|---|---|
| gate | `S6-f4e50770cc349ad3` |
| source | `22f17d10310859262924799a5f594ed6c12c13ba` |
| binary | `af62983a2653c04fbdc8c55f7dd4960231af4273bb12494247529483620c164b` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named corpus test passes on the current build; the mutation flips the exact-boundary case and the same test fails.

FAIL log tail:

```text
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Compiling rchain-sdk v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/sdk)
   Compiling rchain-node v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/node)
   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
   Compiling rchain-block-storage v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/block-storage)
   Compiling rchain-casper v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/casper)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 21.44s
warning: the following packages contain code that will be rejected by a future version of Rust: nom v4.2.3
note: to see what the problems were, use the option `--future-incompat-report`, or run `cargo report future-incompatibilities --id 1`
     Running tests/lean_stake_corpus.rs (target/debug/deps/lean_stake_corpus-10fec3f199242337)
error: test failed, to rerun pass `-p rchain-node --test lean_stake_corpus`
```

## What this does not establish

A current-build regression witness; the live N=8 finality campaign is separate (TA-C1).

## Reproduce

```sh
cargo test -p rchain-node --test lean_stake_corpus -- --exact the_node_agrees_with_the_model_on_every_supermajority_pair
```
