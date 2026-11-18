# s6-4a1606a2375f4e97-20261118

**Evidence locality.** `single-host`

## Scope

Equivocation evidence is proved only by a signed conflict; an unsigned or wrongly signed proof is refused.

The witness is `casper/src/validate.rs::equivocation_evidence_is_proved_only_by_a_signed_conflict`. The mutation is:

```text
-    if !proved.is_signed_by(offender) {
+    if false && !proved.is_signed_by(offender) {
```

## Provenance

| | |
|---|---|
| gate | `S6-4a1606a2375f4e97` |
| source | `98e8681f156ab0d171fdd7f727576d25744334f4` |
| binary | `287370e1473811d206daf9c37fde3dc48c4ea4360248c5a99c101c56012f5148` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation accepts the unproved evidence and the same test fails.

PASS log tail:

```text
running 1 test
test validate::effectful_tests::equivocation_evidence_is_proved_only_by_a_signed_conflict ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 747 filtered out; finished in 0.02s

   Compiling rchain-casper v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/casper)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 7.62s
     Running unittests src/lib.rs (target/debug/deps/rchain_casper-64d507b1a93523ca)
```

## What this does not establish

A current-build regression witness for the proof check; wire carriage and a live restart remain separate.

## Reproduce

```sh
cargo test -p rchain-casper --lib equivocation_evidence_is_proved_only_by_a_signed_conflict
```
