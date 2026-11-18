# s6-0f37971fba3a1c92-20261118

**Evidence locality.** `single-host`

## Scope

The block path verifies every deploy's signature; a deploy not signed by its named key is refused.

The witness is `casper/src/validate.rs::a_deploy_that_its_named_key_did_not_sign_is_refused`. The mutation is:

```text
-    if b.state.deploys.iter().all(|d| d.deploy.verify_signature()) {
+    if true || b.state.deploys.iter().all(|d| d.deploy.verify_signature()) {
```

## Provenance

| | |
|---|---|
| gate | `S6-0f37971fba3a1c92` |
| source | `98e8681f156ab0d171fdd7f727576d25744334f4` |
| binary | `287370e1473811d206daf9c37fde3dc48c4ea4360248c5a99c101c56012f5148` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation removes the check and the same test fails.

PASS log tail:

```text
running 1 test
test validate::tests::a_deploy_that_its_named_key_did_not_sign_is_refused ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 747 filtered out; finished in 0.01s

   Compiling rchain-casper v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/casper)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 10.15s
     Running unittests src/lib.rs (target/debug/deps/rchain_casper-64d507b1a93523ca)
```

## What this does not establish

A current-build regression witness; no historical replay or client rollout is claimed.

## Reproduce

```sh
cargo test -p rchain-casper --lib a_deploy_that_its_named_key_did_not_sign_is_refused
```
