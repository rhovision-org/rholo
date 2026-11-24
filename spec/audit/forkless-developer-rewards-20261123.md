# Forkless developer usage rewards — capability audit (2026-11-23)

**Scope.** Issue [#655](https://github.com/rhovision-org/rholo/issues/655) requires that, before the
first mainnet genesis, the project show that fee-funded rewards for developers whose contracts others
use can be switched on **later** without a hard fork. This record is that issue's first step (PR-A):
for every capability the requirement needs, it says what `dev` already has, what is missing, and
whether a missing piece becomes a **fork** if it is left until after genesis. It reads the tree at
`108d6fac9`. Each `existing` row names the artefact that holds it. A `missing` row describes something
absent and makes no claim about how it should be built: design choices belong on the issue, per
[`AGENTS.md`](../../AGENTS.md) *Documentation discipline*.

The obligation this record supports is the external-register row `G655-FORKLESS-DEVREWARDS` in
[`mainnet-gates.external.json`](mainnet-gates.external.json). That row is `owes`: this audit is
evidence for it, not its witness, and it does not move the row.

## Classifications

- **existing** — the capability is in the tree; the artefact is named.
- **missing (forkless)** — absent, but it can be added after genesis as ordinary Rholang deploys or
  off-chain software on existing primitives, with no change to block validity, finality, native
  accounting or the genesis.
- **fork-required-if-deferred** — absent, and adding it after genesis changes how nodes compute state.
  It has to land before genesis or it will need a hard fork.

## The matrix

| # | Capability | Classification | Evidence (artefact) | Note |
|---|---|---|---|---|
| 1 | Native fee charging: a deploy pays its max phlo, and the unused part is refunded | existing | `NativeSystemState::pre_charge`, `NativeSystemState::refund` (`rholang/src/native_state.rs`) | The staking vault keeps exactly what each deploy burned. This is the only fee stream there is. |
| 2 | A per-deploy split of the burn to a party other than the epoch pot | existing, **producer only** | `NativeSystemState::pay_executor` (`rholang/src/native_state.rs`), called from `casper/src/runtime_manager.rs` and `casper/src/runtime_replay.rs`; conservation `payExecutor_conserves` (`spec/Rchain/Pos.lean`) | The payee is always the block's signed sender. Nothing in the split can name a developer or a treasury. |
| 3 | Epoch pot allocation | existing | `epoch_pot`, `epoch_reward`, `epoch_rewards` (`rholang/src/native_state.rs`); laws 44–47 | The pot goes to the drawn validator set by stake, with no third recipient. |
| 4 | **A reserved developer share of the burn** | **fork-required-if-deferred** | `PosParams` is described as immutable and is written only by the genesis installer (`set_params` is called from genesis installation; the other caller is a test); `executor_share` is decoded and bounded by `decode_params` | Every fee parameter is genesis state, and no deploy can write `PosParams`. A share that does not exist at genesis, or exists at zero, can never be enabled without a fork. Whatever the genesis fixes is the most that can ever be reserved. |
| 5 | Adding money to rewards after genesis | existing, **validator pot only** | `NativeSystemState::fund_pot`; `pos!("fundPot", …)` (`rholang/src/system_processes.rs`) | It's forkless, but it pays validators by stake. It does not reserve fees for developers. It can carry money *back* to the pot, for example from a treasury while rewards are inactive. |
| 6 | The existing "developer allocation" | existing, **not this capability** | laws 78–79 (`spec/Rchain/Genesis.lean`, `spec/Rchain/Vesting.lean`); `casper/src/genesis/dev_tax.rs` | A one-off vesting genesis account for the project developer. It is not a fee levy and is not paid to contract authors. Developer *usage* rewards must not reuse the `dev-tax` names. |
| 7 | A treasury vault that receives REV and pays out by ordinary transfer | missing (forkless) | primitives: `rho:rchain:revVault`, the `MultiSigRevVault` contract installed at genesis (`casper/src/genesis/mod.rs`, reached by `rho:registry:lookup`) | Deployable as a contract after genesis. What genesis has to settle is only where row 4's share is paid. |
| 8 | Threshold authorization by signers | missing (forkless) | primitives: `rho:crypto:secp256k1Verify`, `rho:crypto:blake2b256Hash`, `pos!("getBonds")` (`rholang/src/system_processes.rs`) | A settlement contract can check signatures and stake weights. `getBonds` reads the executing block's pre-state, not a finalized snapshot, so the snapshot rule has to be specified. That is a design point, not a fork. |
| 9 | Deployer identity of an installing deploy | existing | `ProcessedDeploy.deploy` (`models/src/casper/protocol/casper_message.rs`), which carries the signed deploy; `RevAddress::from_public_key` | deployId → deployer key → REV address is derivable from the block body. |
| 10 | Linking an invocation to the deploy that installed the continuation | existing in data, **unverified as an attribution method** | `ProcessedDeploy.deploy_log`: `ConsumeEvent { channels_hashes, hash, persistent }` and `CommEvent { consume, produces, peeks }` (`models/src/casper/protocol/casper_message.rs`); `Consume::apply` hashes channels, patterns, continuation and `persistent` (`rspace/src/trace/event.rs`) | An observer can match a COMM's consume hash to the deploy that logged that `Consume`. Two things are unshown: that the hash is unique per install (the continuation carries `ParWithRandom.random_state`, which suggests it is, but no test pins it), and how to attribute a continuation installed by another continuation rather than directly by a deploy. |
| 11 | Fee attributable to a contract | missing (forkless) | the burn is recorded per deploy (`ProcessedDeploy.cost`) | Splitting one deploy's burn across the contracts it touched is reward-algorithm policy, computed off-chain. No per-contract cost metering exists, and none is needed for forkless activation. |
| 12 | A beneficiary other than the deployer's own address | missing (forkless) | primitives: `rho:rchain:deployerId:ops` `pubKeyBytes` (`rholang/src/system_processes.rs`), the registry | A registry contract keyed by deployer key, where only the deployer's own unforgeable `deployerId` can change it, is ordinary Rholang. The default with no record is the deployer's own address (row 9). |
| 13 | Execution history and the finality anchor | existing | Casper finality is strictly more than two thirds of bonded stake (`docs/src/node/consensus.md`); the finalized fringe (laws 14–17) | A manifest can name a finalized fringe. If the boundary is a fringe rather than a height, the deploy set it defines must be unique, and that is what the reward algorithm's spec must fix. |
| 14 | History retention across checkpoint, restore and bootstrap | existing, **as a service** | `docs/src/node/archive-service.md`, `docs/src/node/sync-and-history.md`: bodies below a checkpoint are served by archives, and each body is hash-checked against its committed header | Old deploy logs can be fetched and verified, but keeping them is paid custody, not a protocol guarantee. Joiners hydrate certified state without bodies (`--no-backfill-bodies`). |
| 15 | Canonical serialization and hashing for a manifest | missing (forkless) | `rho:crypto:blake2b256Hash` exists on-chain | The manifest's byte encoding is off-chain specification work. On-chain verification only needs the hash and the signatures. |
| 16 | Consensus isolation of the reward service | existing by construction, **unmeasured** | rows 7, 8, 12 and 15 are deploys or off-chain software, so none of them touches block validation | The isolation holds as long as nothing in rows 7–15 becomes a validity rule. #655 §7 is the experiment that observes it. |
| 17 | Recovery of service state | missing (forkless) | `docs/src/spec/failure-hazop.md` is the method | There are no HAZOP rows yet, because the service doesn't exist. Rows belong with the PR that builds it (PR-C). |

## What follows

1. **One row is fork-required-if-deferred: row 4.** Every other gap can be closed after genesis. PR-B
   on #655 therefore has to land before genesis. It is Lean-first: the split's conservation extends
   `payExecutor_conserves` in `spec/Rchain/Pos.lean` before the Rust change, and the change is a
   category-A item for the hard-fork tracker #51.
2. **Row 4 fixes a ceiling, not a rate.** Because `PosParams` is genesis state, the share that genesis
   commits is the most that can ever be reserved without a fork. The effective rate below that ceiling
   can be policy run by deploys (row 5 shows the return path to the pot exists). #655 §3B sets out the
   options, and choosing between them is the owner's decision.
3. **Row 10 is the one attribution item that needs a test before it is relied on.** The test is that
   two installs of identical source by different deploys log distinct consume hashes, and that an
   invocation's COMM resolves to exactly one installing deploy.

**Status of row 4 after PR-B.** The developer fee reserve (`developer-share`, the
`pos:developer-reserve` leaf, and the split in `pay_executor` and the merge composition) supplies the
mechanism, and row 7's question of where the share is paid is settled: a `MultiSigRevVault` the
genesis creates over `developer-reserve-public-keys`/`developer-reserve-quorum`, bound as the reserve's
address. A genesis with a zero share still has no reserve, so the row is closed for a given network
only if its genesis commits a non-zero share. **#669 is fixed (2026-11-24, C412):** a minted vault
handle is a checkpointed persistent consume whose handler is rebuilt from a durable dispatch record,
so a vault created in the genesis can be spent from a later block. The spend witness now passes, and
`RESERVE_SPENDABLE` is true. What remains for row 4 is the post-genesis treasury/activation service
(PR-C/PR-D), not the custody mechanism.

**Status after PR-C (2026-11-24).** The post-genesis service now exists as ordinary deploys in
`examples/devrewards/` and is exercised offline:

- **Row 7/8 (treasury and activation).** A treasury holds a fresh unforgeable vault; before a threshold
  of distinct install-time validator keys activates it, `pay` refuses and the REV is held. The private
  `pay` name is held only by the settlement; the public bundle exposes the address, status and
  activation. `casper/tests/devrewards_flows.rs` covers inactive refusal, activation, and the
  replayed-attestation count.
- **Row 10 (attribution).** The two properties this record said were unverified are now pinned by
  `casper/tests/devrewards_attribution.rs`: byte-identical installs by different deploys log
  **distinct** consume hashes, and an invocation's COMM resolves to **exactly one** install. The
  attribution policy (first appearance installs; a burn splits equally among invoked installers;
  deterministic remainder) is `devrewards-usage-v1` in `tools/devrewards_manifest.py`.
- **Row 11 (fee policy).** The split is off-chain and versioned in that tool; the protocol does not
  meter per contract.
- **Row 12 (beneficiary).** `BeneficiaryRegistry.rho` is append-only and self-scoped: the caller
  supplies its own `deployerId`, the key is derived from it, and `beneficiaryOf(key, asOfHeight)`
  applies a change at its height and never before.
- **Row 15 (encoding).** Leaf, node and digest are pinned in
  `tests/fixtures/devrewards_manifest_golden.json`, read by both the Python suite and
  `casper/tests/devrewards_manifest_encoding.rs`.
- **Row 17 (recovery).** `docs/src/spec/failure-hazop.md` U27 dispositions all eleven guide words for
  duplicate epoch, conflicting manifest, unauthorized change, over-claim, crash/restart, stale anchor,
  drained treasury and replayed attestation, each with a named falsifier.

**Row 16 and §7 remain observed, not asserted.** `tools/devrewards-experiment.py` is the witness and
its verdict engine refuses to call a `not-run` arm a pass. A **one-validator production-genesis smoke
is green** — the non-dev genesis commits, and the DevRewards install is an ordinary deploy that
publishes the treasury address in block 4 — which measures the plan's main feasibility risk. The
four-validator run, the independent observer (no block-event export in this CLI), the reserve funding,
the claim and the restart drills are owed, so `G655-FORKLESS-DEVREWARDS` stays `owes`. Nothing above
promotes it.

## Non-claims

This record does not show that any reward can be activated, that the treasury is secure, that
attribution is unambiguous, or that the reward service is isolated from consensus. It classifies
capabilities from the source. #655 §7 is the observation that would establish them.
