# s6-0963a698bc9c4ee4-20261118

**Evidence locality.** `single-host`

## Scope

The named liveness artifact runs the current build's in-process jail rule: a sustained absence jails without touching the bond.

The witness is `tests/liveness-enforcement.py::self_test`. The mutation is:

```text
-            if !record.is_jailed() && record.absent_boundaries >= jail_after {
+            if false && !record.is_jailed() && record.absent_boundaries >= jail_after {
```

## Provenance

| | |
|---|---|
| gate | `S6-0963a698bc9c4ee4` |
| source | `0d4e1abf564cc3bc5044b7b8e862dbfd955c8e57` |
| binary | `98c62deb1fdaa588f7c1b369b0d26d9635508a9dca547120fb9b4169356e1f68` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation disables the jail and the inner test fails.

PASS log tail:

```text
running 1 test
test native_state::liveness_tests::a_sustained_absence_jails_without_touching_the_bond ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 375 filtered out; finished in 0.01s

test self_test ... ok
   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 5.50s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
```

## What this does not establish

The in-process jail rule only. The devnet/partition arms of tools/liveness-enforcement-drill.py and the live G428-M4 campaign are release obligations.

## Reproduce

```sh
python3 tests/liveness-enforcement.py self-test
```
