# s6-2228415e1ac6b83e-20261118

**Evidence locality.** `single-host`

## Scope

The parser depth guards: MAX_PARSE_DEPTH on recursion, MAX_CHAIN_LENGTH on a flat chain, MAX_AST_DEPTH on the term.

The witness is `rholang/src/parser.rs::the_three_depth_guards_refuse_an_over_deep_term`. The mutation is:

```text
-    if exceeds_ast_depth(&proc, MAX_AST_DEPTH) {
+    if false && exceeds_ast_depth(&proc, MAX_AST_DEPTH) {
```

## Provenance

| | |
|---|---|
| gate | `S6-2228415e1ac6b83e` |
| source | `c87ade74356ebd33f964b10d47942b99a5fe86e8` |
| binary | `b9054d88d965f054bf2482b033a0d4c87eb344f7a1583aec7ff2b35c865b422e` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation admits the over-deep AST and the same test fails.

PASS log tail:

```text
running 1 test
test parser::tests::the_three_depth_guards_refuse_an_over_deep_term ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 376 filtered out; finished in 0.01s

   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 6.79s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
```

## What this does not establish

A current-build regression witness; a live corpus replay is separate.

## Reproduce

```sh
cargo test -p rchain-rholang --lib the_three_depth_guards_refuse_an_over_deep_term
```
