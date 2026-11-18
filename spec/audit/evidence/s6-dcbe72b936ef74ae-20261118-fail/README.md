# s6-dcbe72b936ef74ae-20261118-fail

**Evidence locality.** `single-host`

## Scope

if's condition is normalized against an empty par, never the accumulated one.

The witness is `rholang/tests/if_par.rs::if_not_first_in_a_par_holds_across_effect_modes`. The mutation is:

```text
-    let target = normalize_proc(
        value,
        ProcVisitInputs {
            par: Par::default(),
+    let target = normalize_proc(
        value,
        ProcVisitInputs {
            par: input_par.clone(),
```

## Provenance

| | |
|---|---|
| gate | `S6-dcbe72b936ef74ae` |
| source | `c87ade74356ebd33f964b10d47942b99a5fe86e8` |
| binary | `bc3e670774a056d2e8af9430c14a2c8c66748347f2bc21732a75fe9864187233` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation makes the condition eat its prefix and the same test fails.

FAIL log tail:

```text
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    if_not_first_in_a_par_holds_across_effect_modes

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 4 filtered out; finished in 0.02s

   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.74s
     Running tests/if_par.rs (target/debug/deps/if_par-3bd1a10b378d9384)
error: test failed, to rerun pass `-p rchain-rholang --test if_par`
```

## What this does not establish

A current-build regression witness; a live rollout is separate.

## Reproduce

```sh
cargo test -p rchain-rholang --test if_par -- --exact if_not_first_in_a_par_holds_across_effect_modes
```
