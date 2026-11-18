# s6-0963a698bc9c4ee4-20261118-fail

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
| binary | `5fe9b2d9e9644fa5f4d51005ee3cebbd025d5e5d57c10c69795d35729c670fb5` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation disables the jail and the inner test fails.

FAIL log tail:

```text


failures:
    native_state::liveness_tests::a_sustained_absence_jails_without_touching_the_bond

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 375 filtered out; finished in 0.01s

test self_test ... FAILED
   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 5.63s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
error: test failed, to rerun pass `-p rchain-rholang --lib`
```

## What this does not establish

The in-process jail rule only. The devnet/partition arms of tools/liveness-enforcement-drill.py and the live G428-M4 campaign are release obligations.

## Reproduce

```sh
python3 tests/liveness-enforcement.py self-test
```
