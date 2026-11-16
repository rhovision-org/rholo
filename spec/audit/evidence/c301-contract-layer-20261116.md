# C301's contract layer — re-verification at the merged base

**Revision.** `dev` at `d4b9bfbaf529fa754630c0ddfc695a5f2b62e576` (the #596 merge), 2026-11-16.
**Read-class.** Every row below was re-derived from the fetched tree; the source lines name the
mechanism. Two rows were also armed with a local genesis-replay run (A1 and A2's correction). No
public-validator run, deployment, key use or transfer is claimed.

This replaces the read-class input `spec/audit/evidence/coverage-gap-f9521edd7.md`, which is
revision-pinned at `f9521edd7`. The row labels (A1…E4) are that document's.

## The headline: A1 is refuted

A1 claimed the published `bundle+{*AuthKey}` exposes `extractState`, which returns `_authKey` and
`_multiSigRevVault`. It does not. The extract arms listen on a name **derived from the private name**:

```rholang
for (ret <- @(*AuthKey, "extractState")) { ret!({"authKeyUnf": *_authKey}) }
```

while the registry hands out `bundle+{*AuthKey}` — a bundle is a *different* name, and sending
`("extractState", ret)` to it matches neither public contract (`make`, `check`). Both attempts were
armed in `casper/tests/genesis_registry.rs::the_published_auth_key_surface_does_not_hand_out_its_private_names`:
the public method call and the `@(*AuthKey, "extractState")` derived-name call. Neither receives a
reply, while both registry lookups fire and `AuthKey!("make", …)` answers. The probe's output is the
witness; the test asserts the negative and the live control.

**Consequence:** the forge step is unreachable, no genesis change is made, and the pre-authorised
whitelist/removal fix is **not** applied. The adaptor's registration is equivalent for public callers:
the public name matches only `make`/`check`; `extractState` and `securityCheck` are on private derived
names.

## The 19-row matrix

| # | Finding | Current evidence | Verdict | Action |
|---|---|---|---|---|
| A1 | published bundle exposes `extractState` | `standard_deploys.rs:398-400`; `resources/AuthKey.rho:75-77`; `resources/MultiSigRevVault.rho:272-274`; new negative test | **refuted** | close — no genesis change |
| A2 | `info.thn(0)` typo on the unsealer branch | `resources/MultiSigRevVault.rho:235`; corrected in `multi_sig_rev_vault_source` | **confirmed, corrected** | landed, source-pinned |
| A2b | sealed auth stalls `verify`'s public-key probe, so the corrected branch is unreachable | `resources/MultiSigRevVault.rho:108-151` (`check` at 120, `partialFold` at 131, join at 151) | **confirmed** | **C401**, contract-layer fix (hard-fork class) |
| B1 | `rho:io:http` is not an oracle | `rholang/src/native_state.rs:1342-1375` — no fetch, first writer wins, no auth/expiry/delete | **confirmed** | record; docs correction |
| B2 | uncharged quadratic state growth | `native_state.rs:475, 1358-1375` — whole table re-encoded, no charge | **confirmed** | record (charging is consensus class) |
| B3 | channel squatting on quoted/registry names | Rholang persistent-receive semantics; `new` names immune (Law 38) | **confirmed (by semantics)** | decision/record |
| B4 | stdout/stderr uncharged and unescaped | `rholang/src/system_processes.rs:929-947` — `println!("{}", …)`; no charge | **confirmed** | Phase 3: escaping; charge recorded |
| C1 | synchrony/height proposer guards do nothing | `configuration.rs:316-330` — both now refused | **fixed-in-dev** | close |
| C2 | fork-choice settings do nothing | `configuration.rs:331-344` — both now refused | **fixed-in-dev** | close |
| C3 | the inert-config rule missed C1/C2 | `configuration.rs:274-355` — the four are in the list | **fixed-in-dev** | close |
| C4 | `effect-scheduler` relaxed stalls block work, undisclosed | `runtime_manager.rs:906-912, 1057-1061`; `conf.rs:222`; no capability field | **confirmed** | record; add disclosure |
| C5 | local `--epoch-length` vs on-chain `PosParams`; no CloseBlock validity rule | `node_runtime.rs:2556`; `proposer.rs:2389-2394`; `native_state.rs:1102-1110`; `interpreter_util.rs:784-810` (diagnostic only) | **confirmed** | record (hard-fork validity rule) |
| D1 | orphan body served, purged only on reset | `node_running.rs:1037-1047` (`block_store.contains`); `reset.rs:134,331` | **confirmed** | Phase 3 fix |
| D2 | equivocation evidence volatile | `casper/src/dag.rs:194` (in-memory map), `:1088` (insert) | **confirmed** | record (durable evidence) |
| D3 | disk-full unhandled | `shared/src/lmdb.rs:141` (size fixed at open); `block_receiver.rs:576-640` per-block log-and-skip; no ENOSPC classification | **confirmed** | record (+ error classification) |
| D4 | inconsistent durability of markers | `node_runtime.rs:3262,3303` (`std::fs::write`); faucet ledger properly synced at `web_api_impl.rs:163-173` | **confirmed** | Phase 3 fix |
| E1 | raw internals in error bodies | `node/src/web/http.rs:396-401` (`json_result` returns `err.0`) | **confirmed** | Phase 3 fix |
| E2 | public OpenAPI discloses admin/dev routes | `http.rs:1764,1793,1928`; public router at `:2834`, admin at `:3042` | **confirmed** | Phase 3 fix |
| E3 | unbounded txn leg amount | `casper/src/txn_coordinator.rs:98`; already **C348** (`done`) | **duplicate** | close as C348 |
| E4 | acceptance page cites a deleted constant | `docs/src/spec/testnet-acceptance.md:26,541,1381,1434,1850`; no symbol in `node/src` | **confirmed** | Phase 3 doc fix |

## A2/A2b — what the correction does and does not buy

The adapted term now uses `unsealers.nth(info.nth(0))`; the vendored original is untouched and the
marker is asserted (`standard_deploys.rs::tests::the_adapted_multi_sig_vault_corrects_the_vendored_thn_typo`).
A functional probe (a quorum-2 vault with one sealer/unsealer, confirming through the unsealer)
produced no reply even after the correction. The reason is upstream: `verify` joins
`loopFindAny!(publicKeys, …)` with the unsealer fold, and `loopFindAny` calls
`AuthKey!("check", auth, …)`, which sends `"challenge"` to the auth. A sealed box is a plain name with
no receiver, so `check` never answers and the join never fires. The typo fix is necessary but not
sufficient; C401 owns the reachability repair. The functional probe was removed rather than committed
red; the source path above is the evidence.

## Verification runs

- `cargo test -p rchain-casper --test genesis_registry the_published_auth_key_surface_does_not_hand_out_its_private_names`
- `cargo test -p rchain-casper --lib genesis::standard_deploys::tests::the_adapted_multi_sig_vault_corrects_the_vendored_thn_typo`
- `cargo test -p rchain-casper --test genesis_registry a_multisig_vault_spends_through_the_contract_that_holds_it`
- `cargo test -p rchain-casper --test merge_determinism system_deploy_ids_are_not_final_scope_rejections`
- `cargo test -p rchain-casper --test n420_boundary_merge a_final_scope_system_deploy_chain_is_not_rejected`

All pass on the Phase 1 branch.

## What remains under C301

Fourteen of the original nineteen still hold (A2 as corrected-but-limited, plus B1–B4, C4/C5,
D1–D4, E1/E2 and E4), and A2b is new: **fifteen findings plus C401**. They are prioritised in the
Phase 3 tranche: B4 escaping, C4 disclosure, D1, D4 and E4 first (small); B1/B2/B3/C5/D2/D3 as
recorded decisions or consensus-class work; E1/E2 as the web-layer fixes. The thirty T3 process rows
stay `deferred` until Phase 3's contract-layer review.
