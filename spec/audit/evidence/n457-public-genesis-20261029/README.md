# Live recovery: canonical public genesis inputs

## A. Root Cause

The final #474 binary was exercised against the deployed `a5259969a7c5673ccdfc5dad97981d0a3607fcf0` network. The earlier 300-second campaign stopped during ancestry acquisition, not at a demonstrated protocol deadlock.

After allowing the acquisition and state hydration to complete, an empty-store observer without the network's public genesis configuration failed on the first non-genesis block, `0b1ab890cf5c9878077a1fee6b03d4f7f9a1a22988ce8f6e9759302a2ce35f89`. Rebuilding the genesis sidecars computed `e3ed93d89d086974af3b258fc526a7a7640199d40cad96f56fafdc4d158d8a2c` instead of the committed `c465c62e86c4da95903ce913c2c48d5bcaceb1a067f8111f3968de278f3f06be`.

`node/src/runtime/node_runtime.rs` loads genesis descriptors through `genesis_descriptors_from_config`; missing bonds yield no descriptors and leave default PoS state and empty vaults. `casper/src/merging.rs::regenerate_sidecars` consumes those descriptors when replaying genesis. The native PoS state and initial vault balances were installed outside the signed genesis deploy list. A trusted genesis hash does not reconstruct those inputs.

The corrected configuration passed that failure point and replayed history through epoch boundary 590 within the 1,200-second experiment. This is evidence for the configuration diagnosis; it is not completed mature-chain acceptance.

## B. Implemented Correction

Supply the canonical public `bonds.txt`, `wallets.txt`, multisig public key/quorum and original genesis parameters to the joining observer. The files are outside its initially empty data directory. No validator key or existing chain database is copied.

The observer command in `docs/src/node/testnet.md` now includes these inputs and a trusted genesis pin. Its bootstrap URI is quoted so the shell does not interpret `&discovery` as backgrounding. Historical genesis-only timing is labelled as historical.

Hash comparisons, independent replay, finalized-state checks and checkpoint requirements remain enabled. The correction changes configuration/documentation, not consensus code.

## C. Reproduction Results

| Network / observer | Inputs and store | Result |
| --- | --- | --- |
| deployed old / final #474 | Empty store, no canonical public genesis inputs, 300 s | Budget expired during download; no replay diagnosis possible |
| deployed old / final #474 | Empty store, no canonical public genesis inputs, longer budget | FAIL: deterministic genesis sidecar replay mismatch after download/hydration |
| deployed old / final #474 | Empty store, canonical public genesis inputs, 1,200 s | Genesis mismatch cleared; replay reached boundary 590; budget expired before Running |
| deployed old / final #474 | New empty store, canonical public genesis inputs, 3,600 s, separate ports (`qualified-v2`) | RUNNING at this evidence checkpoint |
| Same recovered store restart / wrong genesis | Scheduled only after successful fresh recovery | NOT TESTED at this checkpoint |

The initial 60-minute launch refused a busy port before starting the node; that setup failure is retained in `qualified-probe`. The retry uses a new empty directory and ports starting at 44400.

The 60-minute experiment retains the same one-core, 768 MiB limit, memory floor and cleanup. It does not change protocol retry timeouts.

## D. Acceptance Results

| Gate | Status at this checkpoint |
| --- | --- |
| 1 — Static quality | Final #474 source has passing CI/native/formal campaigns; documentation diff and command shell syntax pass. No new Rust code. |
| 2 — Fresh live recovery | NOT TESTED to completion; qualified experiment running |
| 3 — Restart | NOT TESTED on the qualified recovered store |
| 4 — Negative controls | Missing genesis inputs explicitly refused; other new live controls NOT TESTED |
| 5 — Network safety | Prior experiments preserve local validator service identities and normalized firewall policy. Full four-validator availability FAIL: two endpoints refuse connections. |
| 6 — Independent complete repeat | NOT TESTED |

## E. Source, Executable and Evidence

* Documentation base: `dev@8cbee80374b58eb730425381c3fc4554608b93f6`.
* Tested executable source: `6ff5947bea886ff3ed882a5af4ee5d408c5b6692` (final #474 head).
* Executable SHA-256: `484c382cec31121466f314c1f1e7f8511337dfa2a3eadc71ba12bd4f654ddce1`.
* Trusted genesis: `c596bef4bb678b3ac0cdb931b5b6968653209ca40bc35b6fec0050254d479b26`.
* Captured finalized anchor: height `1759`, hash `ec5a08ef2307dc9a4e44a75e3f123ac345776a7c8b7a7811859060a78021f941`.
* [Existing source CI](https://github.com/rhovision-org/rholo/actions/runs/40623695819), [native executable and N4 controls](https://github.com/rhovision-org/rholo/actions/runs/40623695821), [formal conformance](https://github.com/rhovision-org/rholo/actions/runs/40623695911).
* [Compact commands, resource measurements, log checksums and before/after witnesses](diagnostic-summary.json).
* Retained raw logs and stores: `/home/aria/tmp/rholo-recovery-probe-20261029/`. Earlier evidence directories are preserved.

Public input SHA-256:

| File | SHA-256 |
| --- | --- |
| bonds.txt | `bddd18b25c989faf955ea2e66597dc31be9ba516bbe6545ebaaab6b35eac8b70` |
| wallets.txt | `a1fbc9921990c4f7dd92a15380d0f552b053d1c53852e611e3f630ccaf6fa00a` |

## F. Residual Risks

The two responsive production APIs still identify the old binary. The other two configured endpoints are unavailable; host A's `rnode-d` is inactive. No production service has been started, stopped, upgraded or reconfigured by this campaign. The two responsive nodes agree on the captured finalized anchor, but this is not a new finality-progress witness.

Canonical public configuration fixes the demonstrated genesis mismatch. Mature-history replay cost, completed restore, subsequent work, restart and independent repeat remain separate qualification requirements. Target-finality authority retains the documented bootstrap/operator assumption. No C339 closure or wider N4/N8 acceptance is claimed.

Rollback of this correction discards only the disposable observer resources or reverts the documentation change. Original validator databases and keys are not involved.

## G. Deployment Recommendation

**CONDITIONAL — configuration corrected, but acceptance incomplete.** The known replay mismatch is cleared, but the fresh observer has not yet completed the full mature-chain campaign. Production rollout requires the remaining acceptance evidence and explicit approval.

Refs #457, #456, #435, #339; C339 remains in progress.
