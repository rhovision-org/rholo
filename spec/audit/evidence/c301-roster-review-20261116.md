# C301 — the contract-layer roster review (2026-11-16)

This is the review C301's own finding said had never happened: the thirty system processes the
node installs, read adversarially rather than counted. It closes the finding's denominator and the
`roster-self` row's two halves — the roster and the ledger's own review.

**Revision.** `dev` at `70b8ea255` (the #598 merge) plus the C301 branch, head `7f502e7f8` and the
review commits that follow it. **Read-class.** Every handler body and the native methods it calls
were read; the citations name the symbol. No public-validator run, deployment, key use or transfer
is claimed apart from the committed local `jail-vote` drill in `c301-phase3-20261116/`.

## Method

For each of the **31** URNs in `rholang/src/system_processes.rs` (the thirty ledger rows plus
`rho:rchain:ertp:ledger`, which the roster was missing):

- **Authorization** — what the caller must hold, and whether a deploy can act on another identity
  or authority. The binding is `GDeployerId`, minted by the normalizer from the signed deploy
  (`models/src/normalizer_env.rs`), so a program-authored byte array cannot present someone else's
  deployer id; vault handovers use the separate authority map.
- **Bounds and charging** — whether the body can make the validator do work or grow state without
  the deploy paying. System-process bodies run outside the reducer's phlo accounting, so every
  native read/write and every caller-sized fold must be priced explicitly.
- **Determinism** — no wall clock, filesystem, network, or randomness other than the send's
  carried `Blake2b512Random`.
- **Partiality** — malformed arity/type is refused, never a panic, and never a silent `Nil` that a
  caller could read as a value.
- **State** — which native prefix is written, whether overwrite/delete is intended, and whether the
  leaf's permanence is priced.

## Verdicts

| URN | Verdict | Evidence |
|---|---|---|
| `rho:block:data` | cleared | `system_processes.rs:get_block_data` — public block number/sender/timestamp; the timestamp is a committed field, not a clock read |
| `rho:crypto:secp256k1Verify` | cleared | `secp256k1.rs:verify_bytes`; `a_message_that_is_not_32_bytes_is_refused_not_truncated` |
| `rho:crypto:blake2b256Hash` | cleared | `system_processes.rs:hash_contract`; `blake2b256_hash_contract_replies_with_hash` |
| `rho:crypto:keccak256Hash` | cleared | `system_processes.rs:hash_contract` — pure, linear, storage-charged input |
| `rho:crypto:sha256Hash` | cleared | `system_processes.rs:hash_contract` — pure, linear, storage-charged input |
| `rho:crypto:ed25519Verify` | cleared | `ed25519.rs:verify_bytes`; `verification_answers_false_for_every_malformed_input` |
| `rho:gov:censure` | cleared after fix | The cubic charge was computed on the entry sum; a censure/voucher tuple names two distinct members, so the work universe could reach 2× the bound and the work 8× the charge. Now charged the distinct universe (`censure_is_charged_the_cube_of_its_universe`) |
| `rho:gov:resolveWeights` | cleared | `gov_resolve_weights_reports_weight_map`; bounded at 512 and charged |
| `rho:gov:tally` | cleared | `gov_tally_ranked_returns_winner`; `a_tally_charges_the_option_lists_it_scans` |
| `rho:gov:trustLevels` | cleared | `gov_trust_levels_reports_admin_rooted_levels`; bounded and charged |
| `rho:io:http` | cleared | Bounded and charged claim registry (`http_registry_is_bounded_and_prices_the_encoded_table`) |
| `rho:io:stderr` | cleared | `one_log_line`; state-free, arity-refused |
| `rho:io:stderrAck` | cleared | `one_log_line`; state-free, arity-refused |
| `rho:io:stdout` | cleared | `one_log_line`; state-free, arity-refused |
| `rho:io:stdoutAck` | cleared | `one_log_line`; state-free, arity-refused |
| `rho:qucalc:zfa` | cleared after fix | The linear Pauli fold was uncharged; now priced (`qucalc_zfa_reports_balance_and_phase`) |
| `rho:qucalc:grant` | cleared after fix | Uncharged fold plus an uncapped registry write; both priced (`qucalc_grant_then_verify_across_deploys`) |
| `rho:qucalc:verify` | cleared after fix | Uncharged decode/fold of the stored leaf; now priced (`qucalc_grant_then_verify_across_deploys`) |
| `rho:qucalc:fuse` | cleared after fix | Uncharged fold plus an uncapped registry write; both priced (`qucalc_fuse_mints_syllogism_as_capability`) |
| `rho:rchain:deployerId:ops` | cleared | `deployer_id_ops` — only the caller's own id; a wrong type is now refused rather than `Nil` |
| `rho:rchain:ertp:ledger` | cleared after fix | **The roster was missing this row.** Authorization is the brand's authority name; amounts are `NonNegI64`; every write is now priced (`the_ertp_ledger_mints_a_kit_and_consumes_a_payment_exactly_once`, `the_ertp_ledger_charges_the_leaf_it_writes`) |
| `rho:rchain:multiSigRevVault` | cleared | `the_fixed_channel_refusal_points_at_the_installed_contract` — the fixed channel refuses and names the installed contract |
| `rho:rchain:pos` | cleared | `pos_get_bonds_returns_bonds_map`, `pos_vote_slash_is_authorised_by_the_deployer_id`, `pos_fund_pot_moves_the_callers_rev_into_the_staking_vault`; the getters are now priced. Residual: `trust` lets a trusted stakeholder grow the admission set (design; C111 records `untrust` does not confiscate) |
| `rho:rchain:revVault` | cleared after fix | **The address form of `findOrCreate` and `transfer`'s destination accepted any string**, each call writing free leaves; both now require a REV address, and the leaves are priced (`rev_vault_refuses_a_destination_that_is_not_a_rev_address`) |
| `rho:registry:insertArbitrary` | cleared after fix | The native `PREFIX_REGISTRY` write was uncharged; now priced by the stored value (`registry_insert_arbitrary_charges_the_stored_value`) |
| `rho:registry:insertSigned:secp256k1` | cleared after fix | Same uncharged native write; nonce-monotonic and own-slot only, now priced (`registry_insert_arbitrary_then_lookup_round_trips`) |
| `rho:registry:lookup` | cleared | `registry_lookup`; public read, now priced by the decoded value |
| `rho:registry:ops` | cleared | `the_registry_ops_build_a_uri_and_refuse_what_they_cannot`; pure URI derivation |
| `rho:rev:address` | cleared after fix | **A wrong argument type replied `Nil`**, which is also the reply for a *valid* address; both arms now refuse the type (`rev_address_refuses_a_wrong_argument_type`) |
| `rho:txn` | cleared after fix | **A non-65-byte coordinator wrote a record `decode_txn` could never read**, so the escrow was taken and could never be released; the coordinator length and the REV destination are now checked, and the record is priced (`txn_prepare_refuses_a_short_coordinator_and_a_non_rev_destination`) |
| `sys:authToken:ops` | cleared | `the_sys_auth_token_contract_recognizes_the_token_and_nothing_else` — nothing mints the token; the path fails closed |

## Findings the review produced, and their fixes

1. **revVault accepted any string as a vault address.** `findOrCreate`'s address form and
   `transfer`'s destination wrote zero-balance leaves for arbitrary strings; each call also wrote a
   name leaf and (for `findOrCreate`) a handle. `transfer!(ownId, "<arbitrary>", 0, ret)` created a
   leaf for free. **Fixed:** both arms require `RevAddress::parse`; the writes are priced by
   `Costs::native_store_cost`. Tests: `rev_vault_refuses_a_destination_that_is_not_a_rev_address`,
   `rev_vault_transfer_uses_deployer_capability_and_deposit_is_rejected`.
2. **Native writes were uncharged across the contract layer.** The only charges before this review
   were the `gov` folds and the HTTP claim registry; registry inserts, qucalc's minted capabilities,
   the ERTP ledger, the txn record and the vault leaves were permanent free state. **Fixed:** each
   write/read is priced by its encoded bytes (`Costs::native_store_cost`), and the caller-sized
   folds by their length. Tests: `registry_insert_arbitrary_charges_the_stored_value`,
   `the_ertp_ledger_charges_the_leaf_it_writes`, plus the existing handler tests.
3. **`gov:censure` charged the entry sum, not the work universe.** **Fixed:** the distinct members
   are counted and charged; the bound stays on the entry sum so the accepted input set does not move.
   Test: `censure_is_charged_the_cube_of_its_universe`.
4. **`rho:rev:address` answered `Nil` to a wrong argument type**, indistinguishable from "valid".
   **Fixed:** wrong types are `Err`. Test: `rev_address_refuses_a_wrong_argument_type`.
5. **`rho:txn` accepted a coordinator of any length**, producing a record `decode_txn` (fixed 65-byte
   read) rejects; the escrow was unrecoverable. **Fixed:** the handler refuses a non-65-byte
   coordinator and a non-REV destination, and prices the record. Test:
   `txn_prepare_refuses_a_short_coordinator_and_a_non_rev_destination`.
6. **The ledger roster had drifted from the code.** The ledger held 30 `process` rows; the node
   installs 31 definitions — `rho:rchain:ertp:ledger` (issue #249) had no row, and nothing refused
   the drift. **Fixed:** the row is added and `tools/check-review-ledger.sh` now derives the process
   roster from the `urn:` strings the node installs and fails on any difference.

**Consensus-visible.** Findings 1–5 change what a deploy costs and which inputs are refused; they
are hard-fork class, like the B2 and C5 changes in the same branch. No deployment is part of this
change. The `jail-vote` drill was re-run after these additions on the review head (`ca0902bf5`):
`c301-review-20261116/` — PASS, `jailed`, bond untouched, vote slash, bonds 700/100/100, jail
record `[0, 10]`. The release record must still carry the charges and the new refusals.

## The ledger itself

- **Ceilings** — `tools/check-review-ledger.sh` matches every `ceiling <kind> <n>` to its row count;
  the process ceiling moves 30 → 31 with the new row.
- **The T1 claim** — every T1 row names a tracked path or a law id; the check is green.
- **The roster** — the process roster is now derived (above). The **config** and **tool** rosters
  remain authored: a ledger `api-host` is `api-server.host` in `defaults.conf`, and the tool rows are
  a selected audit set rather than every file under `tools/`. Deriving those would guess a mapping,
  which is the bookkeeping gate the check's own header says it avoids; the `roster-self` row states
  the split rather than claiming a derivation it does not have.
- **What was not read.** The line-level callers outside the handlers (the runtime's dispatch and
  the space's storage charging) are separate rows; this review is the contract layer, not the
  reducer. The `pos` mutating methods are auth/fund-gated and their leaves are bounded by the
  validator/delegator sets the chain admits; their writes are not separately priced here — stated
  as a residual rather than claimed.
