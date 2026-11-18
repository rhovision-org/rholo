# s6-d36503203399d227-20261118-fail

**Evidence locality.** `single-host`

## Scope

bonds_map/stake are carried as NonNegI64; a negative stake is refused by the carrier, not clamped.

The witness is `shared/src/refined.rs::non_neg_i64_accepts_non_negative_rejects_negative`. The mutation is:

```text
-                if v >= 0 {
+                if true {
```

## Provenance

| | |
|---|---|
| gate | `S6-d36503203399d227` |
| source | `0d4e1abf564cc3bc5044b7b8e862dbfd955c8e57` |
| binary | `aa0b278e94fdef2bd3efe93a0c1a02e3f9db1cc27bb2481d58eaf7cc8d00d540` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation accepts the negative and the same test fails.

FAIL log tail:

```text
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    refined::tests::non_neg_i64_accepts_non_negative_rejects_negative

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 105 filtered out; finished in 0.00s

   Compiling rchain-shared v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/shared)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.69s
     Running unittests src/lib.rs (target/debug/deps/rchain_shared-6ba55e676870ee2a)
error: test failed, to rerun pass `-p rchain-shared --lib`
```

## What this does not establish

A current-build regression witness; the genesis-hash acceptance is separate.

## Reproduce

```sh
cargo test -p rchain-shared --lib non_neg_i64_accepts_non_negative_rejects_negative
```
