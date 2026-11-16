# C301's contract layer — re-verification at the merged base

**Revision.** `dev` at `d4b9bfbaf529fa754630c0ddfc695a5f2b62e576` (the #596 merge), 2026-11-16.
**Tranche revision.** `dev` at `70b8ea255` (the #598 merge), plus PR #599. The Phase-3 section at
the end re-derives its six rows against that tree; a line number below that no longer matches the
tree is the finding's revision, not the current citation, and the symbol is the citation that holds.
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
| A2b | sealed auth stalls `verify`'s public-key probe, so the corrected branch is unreachable | `resources/MultiSigRevVault.rho:108-151` (`check` at 120, `partialFold` at 131, join at 151) | **confirmed, closed** | C401: fold-first adaptation landed, test-pinned (hard-fork class) |
| B1 | `rho:io:http` is not an oracle | `rholang/src/native_state.rs:1342-1375` — no fetch, first writer wins, no auth/expiry/delete | **confirmed** | recorded decision — no code change; the documentation correction is not part of this tranche |
| B2 | uncharged quadratic state growth | `native_state.rs:475, 1358-1375` — whole table re-encoded, no charge | **confirmed** | recorded decision — charging is consensus-class; unchanged |
| B3 | channel squatting on quoted/registry names | Rholang persistent-receive semantics; `new` names immune (Law 38) | **confirmed (by semantics)** | recorded decision — `new` names are immune (Law 38); unchanged |
| B4 | stdout/stderr uncharged and unescaped | `rholang/src/system_processes.rs::one_log_line` — the four `stdout`/`stderr` handlers; the finding's original lines 929-947 (revision `d4b9bfb`) | **confirmed, fixed** | **fixed** — control characters are escaped before the line is printed; charging recorded as consensus-class and deliberately unchanged. Test `system_processes::tests::contract_output_cannot_forge_a_second_log_line` |
| C1 | synchrony/height proposer guards do nothing | `configuration.rs:316-330` — both now refused | **fixed-in-dev** | close |
| C2 | fork-choice settings do nothing | `configuration.rs:331-344` — both now refused | **fixed-in-dev** | close |
| C3 | the inert-config rule missed C1/C2 | `configuration.rs:274-355` — the four are in the list | **fixed-in-dev** | close |
| C4 | `effect-scheduler` relaxed stalls block work, undisclosed | `casper/src/runtime_manager.rs:906-912,1056-1061` hard-rejects `relaxed` on the block path; `casper/src/conf.rs:222-224` states it; `docs/src/formal/scheduling.md` and `docs/src/node/scaling.md` state it. Original finding at `runtime_manager.rs:906-912, 1057-1061` | **fixed-in-dev (disclosed)** | close — the refusal and the disclosure both exist; no code change in this tranche |
| C5 | local `--epoch-length` vs on-chain `PosParams`; no CloseBlock validity rule | `node_runtime.rs:2556`; `proposer.rs:2389-2394`; `native_state.rs:1102-1110`; `interpreter_util.rs:784-810` (diagnostic only) | **confirmed** | recorded decision — a CloseBlock validity rule is hard-fork class; unchanged |
| D1 | orphan body served, purged only on reset | `casper/src/engine/node_running.rs::handle_block_request` — the caller's DAG read, not `block_store.contains`; a body the DAG does not hold is not served. Original finding at `node_running.rs:1037-1047`, `reset.rs:134,331` (revision `d4b9bfb`) | **confirmed, fixed** | **fixed** — the serve path reads DAG presence, matching `HasBlockRequest`. Test `engine::node_running::tests::handle_block_request_does_not_stream_a_body_the_dag_does_not_hold` |
| D2 | equivocation evidence volatile | `casper/src/dag.rs:194` (in-memory map), `:1088` (insert) | **confirmed** | recorded decision — durable evidence is storage-class work, unchanged by this tranche |
| D3 | disk-full unhandled | `shared/src/lmdb.rs:141` (size fixed at open); `block_receiver.rs:576-640` per-block log-and-skip; no ENOSPC classification | **confirmed** | recorded decision — error classification is unchanged by this tranche |
| D4 | inconsistent durability of markers | `node/src/runtime/node_runtime.rs::write_marker_durably` — the shard-id and store-format stamps, the shape the faucet ledger already used. Original finding at `node_runtime.rs:3262,3303` (revision `d4b9bfb`) | **confirmed, fixed** | **fixed** — both markers write, sync the file and sync the directory entry. Test `runtime::node_runtime::tests::a_marker_is_written_before_the_directory_is_trusted` |
| E1 | raw internals in error bodies | `node/src/web/http.rs::json_result` remains the boundary; the two constructors that embedded I/O and decode errors now use `node/src/api/web_api_impl.rs::internal`. Original finding at `http.rs:396-401` | **confirmed, fixed in scope** | **fixed** — the detail is logged and the wire message names only the operation. Authored client-facing messages are unchanged; a type-level split of `BlockApiException` is the stronger future fix and is not claimed here. Test `an_internal_failure_keeps_its_detail_out_of_the_response_message` |
| E2 | public OpenAPI discloses admin/dev routes | `node/src/web/http.rs::OPENAPI_JSON` — served on the public listener; the admin paths (`/propose`, `/txn`, `/txn/{txnId}`) are removed. Original finding at `http.rs:1764,1793,1928` | **confirmed, fixed** | **fixed** — the served document describes the public listener only. Test `web::http::tests::the_public_openapi_does_not_disclose_admin_routes` |
| E3 | unbounded txn leg amount | `casper/src/txn_coordinator.rs:98`; already **C348** (`done`) | **duplicate** | close as C348 |
| E4 | acceptance page cites a deleted constant | `docs/src/spec/testnet-acceptance.md:26,541,1381,1434,1850`; no symbol in `node/src` | **confirmed, fixed** | **fixed** — the four citations are rewritten against `finalized_rev_balance`/`clear_faucet_reservation`; P9 is retired as a pair. Tests named on the page (`a_failed_submit_does_not_consume_faucet_budget`, `crash_between_reservation_and_submit_resends_the_same_deploy`, `a_pending_drip_keeps_the_address_reserved`) |

## A2/A2b — what the correction does and does not buy

The adapted term now uses `unsealers.nth(info.nth(0))`; the vendored original is untouched and the
marker is asserted (`standard_deploys.rs::tests::the_adapted_multi_sig_vault_corrects_the_vendored_thn_typo`).
A functional probe (a quorum-2 vault with one sealer/unsealer, confirming through the unsealer)
produced no reply even after the correction. The reason is upstream: `verify` joins
`loopFindAny!(publicKeys, …)` with the unsealer fold, and `loopFindAny` calls
`AuthKey!("check", auth, …)`, which sends `"challenge"` to the auth. A sealed box is a plain name with
no receiver, so `check` never answers and the join never fires. The typo fix was necessary but not
sufficient; C401 owned the reachability repair. **C401 landed**:
`standard_deploys.rs::verify_fold_first` folds into an internal channel, forwards `(unsealed, info)`
to the caller, and answers the public-key channel without challenging a sealed box. The functional
probe is now the committed falsifier
`casper/tests/genesis_registry.rs::a_multisig_vault_confirms_through_a_sealed_unsealer_auth` (mixed
quorum-2 vault, two unsealer confirmations, exact 30,000,000 REV movement); `spec/GENESIS.md` records
the consensus-visible change.

## Verification runs

- `cargo test -p rchain-casper --test genesis_registry the_published_auth_key_surface_does_not_hand_out_its_private_names`
- `cargo test -p rchain-casper --lib genesis::standard_deploys::tests::the_adapted_multi_sig_vault_corrects_the_vendored_thn_typo`
- `cargo test -p rchain-casper --test genesis_registry a_multisig_vault_spends_through_the_contract_that_holds_it`
- `cargo test -p rchain-casper --test merge_determinism system_deploy_ids_are_not_final_scope_rejections`
- `cargo test -p rchain-casper --test n420_boundary_merge a_final_scope_system_deploy_chain_is_not_rejected`

All pass on the Phase 1 branch.

## What remains under C301

Six rows are fixed by the Phase-3 tranche (B4, D1, D4, E1, E2, E4) and C4 is fixed-in-dev (the
block path hard-rejects `relaxed`, and both the scheduler page and the scaling page state it). A2
is corrected but its unsealer branch was unreachable until C401 closed A2b. What remains under
C301 is B1–B3, C5 and D2/D3 as recorded decisions or consensus-class work, and the contract-layer
review of the thirty T3-deferred system processes — the denominator itself. The tranche section
below records what was fixed, what was decided, and the residuals it leaves.

## The Phase-3 tranche (2026-11-16, PR #599)

This is the tranche #598 named as next. It re-derived six rows against `dev` at `70b8ea255` (the
#598 merge) and fixed them; each has a named test:

| Row | Fix | Test |
|---|---|---|
| B4 | Contract output is escaped onto one log line before `stdout`/`stderr` prints it (`rholang/src/system_processes.rs::one_log_line`) | `system_processes::tests::contract_output_cannot_forge_a_second_log_line` |
| D1 | A block body is served only when the DAG holds the block; the caller passes the same presence read `HasBlockRequest` uses (`casper/src/engine/node_running.rs::handle_block_request`) | `engine::node_running::tests::handle_block_request_does_not_stream_a_body_the_dag_does_not_hold` |
| D4 | The shard-id and store-format markers go through `write_marker_durably` (write, file sync, directory sync) | `runtime::node_runtime::tests::a_marker_is_written_before_the_directory_is_trusted` |
| E1 | The two constructors that embedded I/O and decode errors now use `web_api_impl::internal`: the detail goes to the node's log, the body names the operation. Authored client-facing messages are unchanged | `api::web_api_impl::tests::an_internal_failure_keeps_its_detail_out_of_the_response_message` |
| E2 | The served OpenAPI describes the public listener only; the admin paths (`/propose`, `/txn`, `/txn/{txnId}`) are removed | `web::http::tests::the_public_openapi_does_not_disclose_admin_routes` |
| E4 | The four `FAUCET_MAX_DRIPS_PER_ADDRESS` citations are rewritten against `finalized_rev_balance`/`clear_faucet_reservation`; P9 is retired as a pair | The faucet tests named on the acceptance page: `a_failed_submit_does_not_consume_faucet_budget`, `crash_between_reservation_and_submit_resends_the_same_deploy`, `a_pending_drip_keeps_the_address_reserved` |

Deliberately not fixed, and recorded rather than implied:

- **B4's charge.** `stdout`/`stderr` remain uncharged. Charging them changes a term's phlo cost and
  therefore whether a block is valid — consensus-class work. The escape is the part that is not.
- **B1, B2, B3, C5, D2, D3.** Recorded decisions: `rho:io:http` is not an oracle; uncharged state
  growth is consensus-class; quoted/registry-name squatting is persistent-receive semantics;
  the `CloseBlock` validity rule is hard-fork class; equivocation evidence durability and disk-full
  classification are storage-class work. None is a one-line change and none is claimed here.
- **E1's boundary.** `BlockApiException` still carries one string, so the type cannot force a future
  site to keep internals out. The stronger fix — a client/internal split on the type — is named, not
  done.
- **C4's extension.** `gate` and `relaxed` exist, but making the relaxed interleaving sound on-chain
  is the validated-speculation work specified in `docs/src/formal/scheduling.md`; it is not a
  disclosure fix and is not part of this tranche.

## The C401 residual that #598 said it recorded

An auth that is neither an `AuthKey` nor a box from *this* vault still leaves `loopFindAny` waiting
on the challenge: a name with no `"challenge"` receiver and a foreign box are indistinguishable to
the contract without challenging them. The fold-first repair does not change that and does not claim
to; it closes the *sealed box from this vault* path. The residual was named in #598's body as
"recorded" but had no committed home; this section is that home. Pre-existing, not widened into
C401, no code change.

## Close-out note: PR #598

#598 was merged at `70b8ea255` (squash of `e1befc504`) on 2026-11-16T17:21:09Z with its merge-gate
CI checkbox unchecked. Formatting/lints, Dependency audit, Documentation and the partition topology
had passed; **Tests and the node's static gates was still pending**, and the Formal gate did not
fire because the workflow is path-filtered and the diff touched no formal-surface file.

The Tests job then **failed** at `node/tests/node_api.rs::http_surface_without_genesis` ("HTTP
server did not come up" after its 15 s deadline). This was not a genesis-term regression: the test
boots a node with no genesis, so the adapted `MultiSigRevVault` term is never evaluated. The failing
path is the harness: `free_ports` allocates before the heavyweight node lock is taken (the collision
class the lock's own doc names), and a node whose `serve` returned an error produced only a bare
timeout. #597's head ran the same job green (Tests 12m21s), and the same test passes locally on the
merged tree in 0.62 s.

#599 repairs the harness so the next failure is diagnosable and the collision cannot happen:
`common::start_standalone` allocates the ports **under** the lock, and `common::wait_for_http`
checks the node's join handle and fails with the node's own error. Both `node_api` tests pass
locally. The process correction is explicit: **#599 is not merged by the agent**; its gate is green
CI on the head plus the operator's approval.
