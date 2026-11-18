# s6-dcbe72b936ef74ae-20261118

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
| binary | `5f7d102327bd17f5666a96b8c81447ab762d9b64cdbae45f09947a92a6604714` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation makes the condition eat its prefix and the same test fails.

PASS log tail:

```text
running 1 test
test if_not_first_in_a_par_holds_across_effect_modes ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out; finished in 0.09s

   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 13.70s
     Running tests/if_par.rs (target/debug/deps/if_par-3bd1a10b378d9384)
```

## What this does not establish

A current-build regression witness; a live rollout is separate.

## Reproduce

```sh
cargo test -p rchain-rholang --test if_par -- --exact if_not_first_in_a_par_holds_across_effect_modes
```
