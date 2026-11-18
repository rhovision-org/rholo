# Mainnet gate register

Emitted from `spec/audit/mainnet-gates.json` by `tools/build-mainnet-gates.py`.
Do not edit by hand; the checker compares this file with the JSON.

- register dev: `dd6a25d69128375d83b7a7701c169ce672e57536`
- ledger source dev: `e33125f65a95cff4f12baf8139a7361c7d158e43`
- generated: 2026-11-18
- gates: 111

| gate | family | status | title | locality | owner |
|---|---|---|---|---|---|
| `C343` | C343 | **precondition** | Private-chain surface: live B/C/D read-only phase diagnosis | live-host | Aria / operator |
| `C357` | C357 | **external** | Self-replay recovery and section-6 lockstep acceptance | external | Jim / release coordination |
| `G428-M1` | G428 | **blocked** | Supported capacity and physical fault domains | multi-host | Jim / release coordination |
| `G428-M2` | G428 | **owes** | Bounded public ingress and overload behavior | unit | Jim / release coordination |
| `G428-M3` | G428 | **blocked** | Off-host bootstrap and fringe completion | off-host | Aria / evidence work |
| `G428-M4` | G428 | **blocked** | Finality, liveness and autonomous recovery | multi-host | Jim / release coordination |
| `G428-M5` | G428 | **blocked** | Finalized state integrity and merge accounting | integration | Jim / release coordination |
| `G428-M6` | G428 | **external** | Reproducibility and independent sign-off | external | Patrick / independent technical review |
| `G520-MIGRATION` | G520 | **owes** | Populated-store migration and replay qualification | single-host | Jim / release coordination |
| `G584-G1` | G584 | **external** | Installed pins | live-host | Aria / operator |
| `G584-G2` | G584 | **external** | Signed deploy lifecycle | live-host | Aria / operator |
| `G584-G3` | G584 | **external** | No false settlement | live-host | Aria / operator |
| `G584-G4` | G584 | **external** | Recovery without loss | live-host | Aria / operator |
| `G584-G5` | G584 | **external** | Independent observations | live-host | Aria / operator |
| `G584-G6` | G584 | **external** | Independent verdict | external | Patrick / independent technical review |
| `HF51-A-asset-store` | HF51 | **decision** | RCHIP #30 blessed assetStore if chosen | external | @PatrickMockridge |
| `HF51-A-bigint-decimal` | HF51 | **owes** | RCHIP #51: BigInt (the exact Decimal was removed by #527) | external | maintainers |
| `HF51-A-binary-attachments` | HF51 | **owes** | RCHIP #39 binary attachments on DeployData | external | Jim / release operators |
| `HF51-A-block-merge` | HF51 | **decision** | RCHIP #23 block merging / event-log semantics | external | Jim / consensus |
| `HF51-A-block-timestamp` | HF51 | **owes** | Informational block timestamp | integration | Jim / release operators |
| `HF51-A-decentralized-rspace` | HF51 | **decision** | RCHIP #53 RSpace decentralization, if consensus-affecting | external | Jim / consensus |
| `HF51-A-double-mint` | HF51 | **decision** | RCHIP #44 bonded REV double-mint correction | external | Jim / governance |
| `HF51-A-genesis-contract-upgrade` | HF51 | **decision** | RCHIP #43 governed replacement of genesis contracts | external | Jim / governance |
| `HF51-A-hardfork1-migration` | HF51 | **decision** | RCHIP #42 Hard Fork 1 supply migration | external | Jim / governance |
| `HF51-A-phlo-exchange` | HF51 | **decision** | RCHIP #7/#8 phlogiston↔REV exchange and per-block limits | external | @PatrickMockridge |
| `HF51-A-prepaid-rholang` | HF51 | **decision** | RCHIP #38 prepaid execution | external | @PatrickMockridge |
| `HF51-A-rev-economics` | HF51 | **decision** | RCHIP #47 REV stabilization/mint and vesting | external | @PatrickMockridge |
| `HF51-A-shard-config` | HF51 | **owes** | RCHIP #24 committed shard/PoS config | local-devnet | Jim / maintainers |
| `HF51-A-storage-rent` | HF51 | **decision** | RCHIP #40 data retention/rent | external | Jim / governance |
| `HF51-A-unbond-throttle` | HF51 | **decision** | RCHIP #4 unbonding rate throttle | external | @PatrickMockridge |
| `HF51-A-validator-rewards` | HF51 | **decision** | RCHIP #46 rotation/longevity/open enrollment | external | @PatrickMockridge |
| `HF51-A-vault-export` | HF51 | **blocked** | RCHIP #41 REV vault state export | external | Jim / operators |
| `HF51-Audit-active-set-draw` | HF51 | **owes** | Epoch active validator set stake-weighted draw | external | Jim / consensus |
| `HF51-Audit-cost-insert-and-tally` | HF51 | **owes** | The cost register's charges: set.add/set + x (C353), the tallies (C354) | external | Jim / consensus |
| `HF51-Audit-cost-tracks-work` | HF51 | **owes** | The C345/C346 charge changes: collection canonicalisation, union, censure | external | Jim / consensus |
| `HF51-Audit-inactivity-leak` | HF51 | **decision** | Activity-aware finality weight / inactivity leak | external | @aixaria0 |
| `HF51-Audit-law52-partition` | HF51 | **owes** | Law 52 / C174 live finality partition | local-devnet | Jim / consensus |
| `HF51-Audit-law53-estrangement` | HF51 | **decision** | Law 53 C173 estrangement decision | external | @PatrickMockridge |
| `HF51-Audit-merge-resolution-stage4` | HF51 | **decision** | #127 stage 4: blocks carry merge resolution | external | @PatrickMockridge |
| `HF51-Audit-parent-limit` | HF51 | **owes** | C191 #153 parent set bound | local-devnet | Jim / consensus |
| `HF51-Audit-section6-categoryA` | HF51 | **owes** | Exhaustive §6 category-A reverse inventory | external | Jim / auditors |
| `HF51-Audit-sidecar-merge` | HF51 | **owes** | C207 per-accepted-deploy cost merge composition | external | Jim / consensus |
| `HF51-B-babel` | HF51 | **decision** | RCHIP #48 Rholang compatibility translator | external | Jim / language |
| `HF51-B-binary-attachments` | HF51 | **owes** | RCHIP #39 client/deploy attachment path | external | Jim / release operators |
| `HF51-B-breaking-release` | HF51 | **blocked** | RCHIP #10 coordinated breaking-release runbook | external | Jim / operators |
| `HF51-B-genesis-contract-upgrade` | HF51 | **decision** | RCHIP #43 client-visible blessed-contract upgrade | external | Jim / governance |
| `HF51-B-rev-receipt` | HF51 | **decision** | RCHIP #31 execute-on-REV-receipt | external | @PatrickMockridge |
| `HF51-B-staking-pools` | HF51 | **decision** | RCHIP #32 staking pools and REV addresses | external | @PatrickMockridge |
| `HF51-B-staking-pools-interface` | HF51 | **decision** | RCHIP #32 beneficiary/reward channel compatibility | external | @PatrickMockridge |
| `HF51-B-string-functions` | HF51 | **owes** | RCHIP #37 string functions | external | Jim / release operators |
| `HF51-Batch-activation-schedule` | HF51 | **owes** | Height/version schedule, committed bounds, fundPot and startup guards | local-devnet | Jim / maintainers |
| `HF51-Batch-attestation-format` | HF51 | **owes** | Potential lightweight attestation or non-merge-parent change | external | Jim / consensus |
| `HF51-Batch-crypto-agility` | HF51 | **blocked** | RCHIP #274 signature/key/deploy ID agility fork | integration | @jimscarver |
| `HF51-Batch-faucet-genesis` | HF51 | **owes** | Faucet-funded genesis wallet or funded key reuse | external | Jim / operators |
| `HF51-Batch-governance-schedule` | HF51 | **decision** | Governance-settable version schedule and PoS parameters | external | Jim / governance |
| `HF51-Batch-state-preservation` | HF51 | **blocked** | Old-state export and new-genesis replay/snapshot ceremony | external | Jim / operators |
| `HF51-Decision-developer-allocation` | HF51 | **owes** | Canonical developer-allocation implementation track | external | @jimscarver |
| `HF51-Release-attachment-upgrade` | HF51 | **blocked** | Coordinated rollout for RCHIP #39 attachment-bearing deploys | external | Jim / operators |
| `HF51-Release-decimal-genesis` | HF51 | **decision** | Decimal genesis — withdrawn with the decimal type (issue #527) | external | @PatrickMockridge |
| `S6-002725fa6a14a386` | S6 | **reviewable** | **The finality gate's partition is the *live weight set***: `calculate_fringe` takes the set a candidate must have been  | unclassified | Jim / independent audit reviewer |
| `S6-0879bf292d995e9f` | S6 | **reviewable** | **Equivocating blocks are refused** — at `insert` (H-1) and at restore (H1c) — where the oracle has no such gate | unclassified | Jim / independent audit reviewer |
| `S6-0963a698bc9c4ee4` | S6 | **owes** | **Liveness enforcement is a new PoS boundary rule (RCHIP #55, Law 89).** | unclassified | @PatrickMockridge |
| `S6-0b46037b3783e4ca` | S6 | **reviewable** | Integer arithmetic **promotes to `BigInt`** on `i64` overflow and never wraps; mixed `Int`/`BigInt` operands promote | unclassified | Jim / independent audit reviewer |
| `S6-0d97bafd93b8fda9` | S6 | **reviewable** | **A boundary's participation comes from the last finalised fringe** — the epoch's participation rule reads each drawn va | unclassified | Jim / independent audit reviewer |
| `S6-0f37971fba3a1c92` | S6 | **reviewable** | The block path **verifies every deploy's signature** — `validate::deploy_signatures` in `block_summary`'s pure list, ref | unclassified | Jim / independent audit reviewer |
| `S6-1587e04b86742658` | S6 | **reviewable** | **A validator's epoch reward is scaled by its participation** — `PosParams::absence_slack` is the *knee* (the lag at whi | unclassified | Jim / independent audit reviewer |
| `S6-212cef89b85456e0` | S6 | **reviewable** | `BindPattern.freeCount` **rejected** when negative at the proto boundary | unclassified | Jim / independent audit reviewer |
| `S6-2228415e1ac6b83e` | S6 | **reviewable** | rholang depth guards (`MAX_PARSE_DEPTH = 128` on the parser's own recursion, `MAX_CHAIN_LENGTH = 512` on one flat chain, | unclassified | Jim / independent audit reviewer |
| `S6-25bfdbe2c179c81e` | S6 | **reviewable** | An instance-less or unknown-only `Expr` is **refused** with `Malformed("expr_instance")` rather than decoded as `false` | unclassified | Jim / independent audit reviewer |
| `S6-29a0672aff87290b` | S6 | **reviewable** | Number-channel merge diffs are **checked** (`i64` overflow is an error) — the subtraction, the merge, **and the accumula | unclassified | Jim / independent audit reviewer |
| `S6-3d5848fba9a91692` | S6 | **reviewable** | **The block's producer is paid a share of what its deploys burned** — `PosParams::executor_share` basis points of a depl | unclassified | Jim / independent audit reviewer |
| `S6-423749eda11ee12d` | S6 | **reviewable** | **A slash fans the untaken remainder out to each delegator's own vault, and clears the delegation ledgers** — `atRisk` r | unclassified | Jim / independent audit reviewer |
| `S6-45c551728731f6da` | S6 | **reviewable** | A slashed validator that had a **staged withdrawal** is removed from the pending map | unclassified | Jim / independent audit reviewer |
| `S6-47c941dac413cd3b` | S6 | **reviewable** | Epoch length `0` (the port's permissive default) means **every block is a boundary**; the contract's `%`/`/` by it would | unclassified | Jim / independent audit reviewer |
| `S6-4a1606a2375f4e97` | S6 | **reviewable** | **An equivocation is slashable, and its proof travels in the block** — the `Slash` system deploy carries an `Equivocatio | unclassified | Jim / independent audit reviewer |
| `S6-4cefbdfed1e1520b` | S6 | **reviewable** | **A validator's bond may be other people's stake: `pos:bonds` carries the *aggregate*, and `pos:delegations` attributes  | unclassified | Jim / independent audit reviewer |
| `S6-5651614bf8e3eeb7` | S6 | **owes** | Replay COMM candidates follow chronological execution order; strict replay verdicts can differ for order-sensitive trace | unclassified | Jim / consensus and release operators |
| `S6-5896ceb7cf2b4804` | S6 | **reviewable** | **The height maximum counts failed justifications** — a child numbered above a failed parent is admitted to the other ch | unclassified | Jim / independent audit reviewer |
| `S6-5bd51477a56edf07` | S6 | **reviewable** | Negative deploy cost **rejected** (not wrapped to `uint64`) | unclassified | Jim / independent audit reviewer |
| `S6-5c91a4e66518c7ec` | S6 | **reviewable** | Active validator set = a **stake-weighted draw without replacement** from the eligible pool, seeded one epoch ahead | unclassified | Jim / independent audit reviewer |
| `S6-7b9fe37e74b318db` | S6 | **reviewable** | **The fee floor and the parent bound are committed in the genesis state** — `casper.min-phlo-price` and `casper.max-numb | unclassified | Jim / independent audit reviewer |
| `S6-83825d04fdb174bc` | S6 | **reviewable** | **A resolved parent at or above the block's number is refused** — the **failed** ones included (H1b, `casper/src/validat | unclassified | Jim / independent audit reviewer |
| `S6-91b05027373a3107` | S6 | **reviewable** | `Secp256k1::verify_bytes` **refuses a message that is not the 32-byte prehash** (a named `PREHASH_LEN`), where the depen | unclassified | Jim / independent audit reviewer |
| `S6-9297637b16a7e18c` | S6 | **reviewable** | **An epoch's reward for a key with delegators is split pro-rata, and a delegator's share is credited to `pos:delegated_r | unclassified | Jim / independent audit reviewer |
| `S6-957351356597c77d` | S6 | **reviewable** | **`fundPot` adds REV to the validators' reward pot with no claim** — `rho:rchain:pos!("fundPot", deployerId, amount, ret | unclassified | Jim / independent audit reviewer |
| `S6-9b7d0ce349e22aa8` | S6 | **reviewable** | Vaults are a **balance map keyed by REV address**; `transfer` takes the caller's `deployerId` rather than a minted purse | unclassified | Jim / independent audit reviewer |
| `S6-PR600-CHECKPOINT-V3` | S6 | **reviewable** | Checkpoint v3 (Law 117): fringeHeadersRoot sync-protocol rule | integration | Jim / release coordination |
| `S6-PR600-MERGE-FORK` | S6 | **reviewable** | Merge fork rule (Law 115): trimmed conflict scope | external | Jim / release coordination |
| `S6-a03b7b63647b8746` | S6 | **reviewable** | **A block's justification set is bounded by `max-number-of-parents`, refused on the receiving side** (`casper/src/valida | unclassified | Jim / independent audit reviewer |
| `S6-a2f660ef8d236b06` | S6 | **reviewable** | **`withdraw` refuses while delegations are outstanding, and `delegate` refuses while a withdrawal is pending** — so a va | unclassified | Jim / independent audit reviewer |
| `S6-a641f6fa8cc3ce54` | S6 | **reviewable** | **A slash is graded by the offence** — `Malicious` takes everything the validator holds in the PoS system, `Misdemeanour | unclassified | Jim / independent audit reviewer |
| `S6-aff8e713fee0981e` | S6 | **reviewable** | `check_min_messages` requires the minimum-message **sender set** to equal the bonded set, where the oracle compares coun | unclassified | Jim / independent audit reviewer |
| `S6-c59c38eeab097a7b` | S6 | **reviewable** | The Coop **multisig public keys** are read as the port's initial **trusted stakeholder** set | unclassified | Jim / independent audit reviewer |
| `S6-c7536b90bd39cf71` | S6 | **reviewable** | `MAX_VALUE_DEPTH = 256` on a **produced value** (`rholang/src/storage.rs`, both produce paths) | unclassified | Jim / independent audit reviewer |
| `S6-cc87a089357b25fa` | S6 | **reviewable** | RSpace candidate selection is **sorted-first** by content hash (not newest-first insertion order) | unclassified | Jim / independent audit reviewer |
| `S6-ccb08bb8dcafdd3c` | S6 | **reviewable** | Cost charges track the work of collection canonicalisation, union and rho:gov:censure. | unclassified | Jim / consensus |
| `S6-d36503203399d227` | S6 | **reviewable** | `bonds_map`/stake carried as `NonNegI64` (reject negative) | unclassified | Jim / independent audit reviewer |
| `S6-d39645b95737497d` | S6 | **reviewable** | The node's own block metadata **carries the `slashable` flag** (`BlockMetadataProto.slashable = 22`), where the port dro | unclassified | Jim / independent audit reviewer |
| `S6-d7dd0a06a5dee537` | S6 | **reviewable** | `New.bind_count`/`Receive.bind_count` are a **`FreeCount`** carrier, and a negative count is **rejected at the proto bou | unclassified | Jim / independent audit reviewer |
| `S6-dc8df48ecb6197ee` | S6 | **superseded** | A **new exact decimal value type** (`GDecimal`, wire tag 35, sort rank 14, scale ≤ 38, canonical coefficient) | unclassified | Jim / independent audit reviewer |
| `S6-dcbe72b936ef74ae` | S6 | **reviewable** | `if`'s condition is normalized against an **empty** par (`normalizer.rs::normalize_if`), as `match`'s target is | unclassified | Jim / independent audit reviewer |
| `S6-de66e3b5b934584f` | S6 | **reviewable** | **The full-partition filter is containment, not equality** (`sees_the_whole_partition`, `block-storage/src/dag/finalizer | unclassified | Jim / independent audit reviewer |
| `S6-e2cdf44a5cc0c81e` | S6 | **reviewable** | **The merge composes cost accounting per accepted deploy instead of a block carrying it** — `pre_charge`, `refund` and ` | unclassified | Jim / independent audit reviewer |
| `S6-f4e50770cc349ad3` | S6 | **reviewable** | super-majority as exact integer `3·stake > 2·total` | unclassified | Jim / independent audit reviewer |
| `S6-f735095c746556f3` | S6 | **owes** | **A merge diffs each affected channel once, not once per produce/consume event.** | unclassified | Jim / consensus |
| `S6-f7d55fe093312364` | S6 | **reviewable** | **Native writes join the merge's conflict relation** (issue #83): two chains of different blocks that wrote a common nat | unclassified | Jim / independent audit reviewer |
| `S6-fb6f884e759d7af7` | S6 | **reviewable** | **The parent-relative timestamp validity bound is retired; header time is informational** (`casper/src/validate.rs::bloc | unclassified | Jim / independent audit reviewer |
| `S6-fbcd04455372e58c` | S6 | **reviewable** | Storage is **refunded** when a produce/consume matches (the gas a matched op costs) | unclassified | Jim / independent audit reviewer |
| `TA-C1` | TA | **blocked** | Anyone can propose, and production is bounded | live-host | Jim / release coordination |
| `TA-C2` | TA | **external** | Validators can be dropped and joined without risk | live-host | Aria / evidence work |
| `TA-C3` | TA | **reviewable** | Every attack vector is handled | reviewable | Jim / release coordination |

Status is a disposition of the evidence, not a release claim. A gate is `met` only
when its witness resolves, its negative control is recorded, its evidence is pinned,
and every dependency is satisfied. This register ships no `met` gate.
