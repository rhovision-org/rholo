# Live recovery: canonical public genesis inputs

## A. Root Cause

The final #474 binary was exercised against the deployed `a5259969a7c5673ccdfc5dad97981d0a3607fcf0` network. The earlier 300-second campaign stopped during ancestry acquisition, not at a demonstrated protocol deadlock.

After allowing the acquisition and state hydration to complete, an empty-store observer without the network's public genesis configuration failed on the first non-genesis block, `0b1ab890cf5c9878077a1fee6b03d4f7f9a1a22988ce8f6e9759302a2ce35f89`. Rebuilding the genesis sidecars computed `e3ed93d89d086974af3b258fc526a7a7640199d40cad96f56fafdc4d158d8a2c` instead of the committed `c465c62e86c4da95903ce913c2c48d5bcaceb1a067f8111f3968de278f3f06be`.

`node/src/runtime/node_runtime.rs` loads genesis descriptors through `genesis_descriptors_from_config`; missing bonds yield no descriptors and leave default PoS state and empty vaults. `casper/src/merging.rs::regenerate_sidecars` consumes those descriptors when replaying genesis. The native PoS state and initial vault balances were installed outside the signed genesis deploy list. A trusted genesis hash does not reconstruct those inputs.

The corrected configuration passed that failure point. A second genuinely empty-store campaign reached epoch boundary 1390, then the harness stopped its disposable node when host available memory fell below its safety floor. This is evidence for the configuration diagnosis; it is not completed mature-chain acceptance.

## B. Implemented Correction

Supply the canonical public `bonds.txt`, `wallets.txt`, multisig public key/quorum and original genesis parameters to the joining observer. The files are outside its initially empty data directory. No validator key or existing chain database is copied.

The #472 root manifest and #474 bootstrap-affinity/progress corrections remain intact. These observations do not demonstrate a defect in the merged #472 manifest.

The observer command in `docs/src/node/testnet.md` now includes these inputs and a trusted genesis pin. Its bootstrap URI is quoted so the shell does not interpret `&discovery` as backgrounding. Historical genesis-only timing is labelled as historical.

Hash comparisons, independent replay, finalized-state checks and checkpoint requirements remain enabled. The correction changes configuration/documentation, not consensus code.

## C. Reproduction Results

| Network / observer | Inputs and store | Result |
| --- | --- | --- |
| deployed old / final #474 | Empty store, no canonical public genesis inputs, 300 s | Budget expired during download; no replay diagnosis possible |
| deployed old / final #474 | Empty store, no canonical public genesis inputs, longer budget | FAIL: deterministic genesis sidecar replay mismatch after download/hydration |
| deployed old / final #474 | Empty store, canonical public genesis inputs, 1,200 s | Genesis mismatch cleared; replay reached boundary 590; budget expired before Running |
| deployed old / final #474 | New empty store, canonical public genesis inputs, 3,600 s budget | FAIL acceptance: safety floor stopped the observer after 3,128.340 s, at epoch boundary 1390, before Running |
| Same recovered store restart / wrong genesis | Gated on successful fresh recovery | NOT TESTED: no successful recovered store |

The long experiment used one core, 768 MiB RAM, and a 700,000 KiB host-available-memory floor. The observer's swap allowance was bounded to 256 MiB during the run; the exact runtime change is retained in `resource-control.json`. This did not change protocol retry timeouts. The initial long launch refused a busy port before starting a node (`qualified-probe`); the independent `qualified-v2` attempt used a new empty directory and ports starting at 44400.

Measured peak process RSS was **807,216 KiB** and minimum host available memory **699,180 KiB**. The stop was the harness safety guard, not an established OOM. Journal ordering records SIGINT at `01:10:27 UTC`, followed by the replay task-cancelled error at `01:10:39 UTC`. No earlier sync failure, active-bonds failure or BlockReceiver overflow was observed in this corrected-input run. The node drained with exit 0, while the acceptance JSON correctly remained false.

The retained prototype launcher also exited 0 after cleanup; it must not be used as an acceptance verdict. The corrected launcher maps failed or incomplete reports to a nonzero exit status and supports `--report-only` to verify this against the retained failed report. All four CLI verdict controls passed (`verdict-checks.json`). The exact launcher/helper used by the live campaign are retained at commit `5f5120690db5af4b05f7c3a3cae8e81bfdfc3a94`; the current helper also installs the explicit swap bound at launch. The campaign runtime bound change is recorded separately.

## D. Acceptance Results

| Gate | Status at this checkpoint |
| --- | --- |
| 1 — Static quality | PASS on recorded sources: final #474 CI/native/formal campaigns and all four jobs of documentation CI `40793844405` at `5f5120690db5af4b05f7c3a3cae8e81bfdfc3a94`; local command/script/report checks pass. No new Rust code. |
| 2 — Fresh live recovery | FAIL acceptance: safety guard stopped the attempt before Running |
| 3 — Restart | NOT TESTED: fresh restore did not complete |
| 4 — Negative controls | Missing genesis inputs explicitly refused and no partial acceptance observed; full set NOT TESTED |
| 5 — Network safety | Prior experiments preserve local validator service identities and normalized firewall policy. Full four-validator availability FAIL: two endpoints refuse connections. |
| 6 — Independent complete repeat | NOT TESTED |

## E. Source, Executable and Evidence

* Documentation base: `dev@8cbee80374b58eb730425381c3fc4554608b93f6`.
* Tested executable source: `6ff5947bea886ff3ed882a5af4ee5d408c5b6692` (final #474 head).
* Executable SHA-256: `484c382cec31121466f314c1f1e7f8511337dfa2a3eadc71ba12bd4f654ddce1`.
* Trusted genesis: `c596bef4bb678b3ac0cdb931b5b6968653209ca40bc35b6fec0050254d479b26`.
* Captured finalized anchor: height `1759`, hash `ec5a08ef2307dc9a4e44a75e3f123ac345776a7c8b7a7811859060a78021f941`.
* [Existing source CI](https://github.com/rhovision-org/rholo/actions/runs/40623695819), [native executable and N4 controls](https://github.com/rhovision-org/rholo/actions/runs/40623695821), [formal conformance](https://github.com/rhovision-org/rholo/actions/runs/40623695911), [documentation/source checks](https://github.com/rhovision-org/rholo/actions/runs/40793844405).
* [Compact commands, resource measurements, log checksums and before/after witnesses](diagnostic-summary.json).
* Retained raw logs and stores: `/home/aria/tmp/rholo-recovery-probe-20261029/`. Earlier evidence directories are preserved.
* [Final raw acceptance report](acceptance.json), [end-of-test safety readings](final-safety.json), [runtime observer limits](resource-control.json), [protected-validator readiness sample](production-readiness.json), [progress/resource trace](progress-samples.jsonl), [observer shutdown journal](observer-journal.log), [used harness provenance](harness-provenance.json), [CLI verdict controls](verdict-checks.json).

Public input SHA-256:

| File | SHA-256 |
| --- | --- |
| bonds.txt | `bddd18b25c989faf955ea2e66597dc31be9ba516bbe6545ebaaab6b35eac8b70` |
| wallets.txt | `a1fbc9921990c4f7dd92a15380d0f552b053d1c53852e611e3f630ccaf6fa00a` |

## F. Residual Risks

The two responsive production APIs still identify the old binary. The other two configured endpoints are unavailable; host A's `rnode-d` is inactive. After cleanup the responsive nodes were both `normal`, admitted deploys, and reported tip 2363 and the same finalized anchor 1759. Host A's original PID/start time and normalized firewall policy were unchanged; the observer unit was inactive. No production service has been started, stopped, upgraded or reconfigured by this campaign. These samples do not establish continuous availability or new finality progress.

Canonical public configuration fixes the demonstrated genesis mismatch. The exact external blocker for completing this live campaign is sufficient isolated compute/memory headroom: the authorized production host has approximately 3.9 GiB total RAM, already runs an original validator, and the observer consumed the safe spare allowance before it completed. Increasing its allowance on that host would violate the protection objective. Complete live qualification needs an independent Linux x86-64 host with sufficient capacity and a working bootstrap reply path, or an explicitly approved capacity change. The documented validator floor is [8 GiB per validator](https://github.com/rhovision-org/rholo/blob/8cbee80374b58eb730425381c3fc4554608b93f6/docs/src/node/validator-requirements.md); this campaign does not establish the exact minimum for an observer.

Completed restore, subsequent work, restart and independent repeat remain unproven. No mature-history CI replay was run; the cited native/N4 campaign covers its own finite fixture. Target-finality authority retains the documented bootstrap/operator assumption. No C339 closure or wider N4/N8 acceptance is claimed.

Rollback of this correction discards only the disposable observer resources or reverts the documentation change. Original validator databases and keys are not involved.

## G. Deployment Recommendation

**NO-GO — Unresolved or failed acceptance.** The known configuration mismatch is corrected, but no fresh observer on this final replay implementation has completed the required live acceptance here. Do not infer a successful live join from CI, partial replay, clean shutdown or these documentation changes. Production rollout requires the remaining acceptance evidence and explicit approval.

Refs #457, #456, #435, #339; C339 remains in progress.
