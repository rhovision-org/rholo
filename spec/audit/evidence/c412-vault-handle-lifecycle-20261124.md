# C412 — a minted vault handle did not survive the block boundary (#669)

**Date:** 2026-11-24 · **Branch:** `finish/655-forkless-devrewards` (base `dev` at `946160242`) ·
**Class:** consensus-visible, pre-genesis (no launched chain exists).

## The defect

`rho:rchain:revVault!("findOrCreate", …)` mints a native handle and installs its continuation with
`install_native` (`rholang/src/contract_call.rs`). Before this change that install called
`space.install`, which writes the hot store's `installed_continuations` — a map that is deliberately
**absent from `changes()`** and therefore not checkpointed — and the handler existed only in the
minting block's dispatcher. At the next block's `reset`, and on a replay's fresh dispatcher, neither
existed: a send on the handle found no continuation, or a `ScalaBodyRef` with no handler, and the
handle answered nothing. The genesis developer reserve (#655) is held by such a handle, so the reserve
could accrue and never be spent — the reason `RESERVE_SPENDABLE` was false.

## The fix

1. `install_native` installs a **persistent consume** (`persist: true`) with the `ScalaBodyRef`
   continuation. A persistent `WaitingContinuation` is checkpointed and remains installed after it
   fires, so the continuation is present in the next block and on replay.
2. `findOrCreate` records `PREFIX_VAULT_HANDLE` (`rspace/src/native_store.rs`,
   `rholang/src/native_state.rs`): a durable `body_ref -> base58 address` leaf written in the same
   native-state transaction as the mint. Native `Put` actions are folded into the checkpoint's radix
   trie root (`rspace/src/history/history_repository.rs`), so the record is consensus state.
3. `NativeBodyResolver` (`rholang/src/dispatch.rs`; `VaultHandleResolver` in
   `rholang/src/system_processes.rs`) is asked only on a `ScalaBodyRef` miss for a negative id. It
   reads the durable record and rebuilds the same `vault_handle_handler`, then caches it by id.
4. `clear_minted_handlers` at `reset` drops negative-id handlers so play resolves the handle from
   durable state exactly as a replay does. This is a **handler-parity** guard; resolution itself
   charges no phlo.
5. `findOrCreate` now prices the third native leaf (vault + name map + dispatch record).

## Witness and controls (all measured in this worktree)

- `cargo test -p rchain-casper --test vault_handle_across_blocks` — mint in block 1, `balance` in block
  2, **and replay both blocks with `with_cost_accounting = true`**. Pass. The replay path refuses a
  cost mismatch (`casper/src/runtime_replay.rs`, `recorded_cost != result.cost.value`), so this is a
  real parity assertion, not a vacuous state compare. Measured costs: mint 1422, balance 1324, replay
  equal.
- `cargo test -p rchain-casper --test developer_reserve` — all four, none ignored. The spend witness
  creates the vault at **genesis** and moves REV out of it at **block 4**, through the genesis-minted
  handle: `the_reserve_is_spent_by_its_quorum_and_by_nobody_else ... ok`.
- **Mutation control.** Reverting `install_native`'s persistent consume to the pre-#669
  `space.install` path reddens the cross-block witness exactly as predicted:
  `the handle must answer balance in block 2: []` → `test result: FAILED`. Restoring the fixed file
  returns it to `ok`. The reverted file was restored from a byte copy; `git diff` against the saved
  copy was empty before the final run.

## Withdrawn diagnosis — the reserve witness's second "defect" was the test harness

The previous session claimed the genesis `MultiSigRevVault::deployerAuthKey` was "unreachable from a
later block" and left the witness ignored on that basis. That is wrong, and it was proved twice:

- The witness failed at `casper/tests/developer_reserve.rs` block 3 with `(false, "invalid auth")` — a
  reply, which requires `deployerAuthKey`, the alias and `multiSig("transfer")` to have fired.
- The diagnostic that produced "not fired" inserted `@MultiSigRevVault!("extractState", …)` before
  `deployerAuthKey`. `extractState` is a **single-shot** `for` (`MultiSigRevVault.rho:272`), consumed
  by the block-2 outsider, so the probe hung at block 3 before reaching the method. The one green
  witness run used that instrumented source and is not evidence.
- `casper/tests/devrewards_auth_shape.rs` now pins the real facts: `AuthKey.check` accepts the
  `DeployerIdOps` bytes, a literal `hexToBytes` copy of them, and rejects a different shape; and a
  two-key vault created with the reserve's literal key list accepts signer A in one block.
- **Root cause of `invalid auth`:** every `compute_state` in `developer_reserve.rs` shared one
  `Blake2b512Random`. `new`-bound names derive from the seed, so block 2's fresh `authKey` contract
  was installed on the same name block 3 minted, and block 3's `check` was answered by block 2's
  token (a different shape). One seed per evaluate makes the witness pass with **no contract change**.
  The same trap is recorded in the project memory (`rholang-test-rng-reuse`). `vault_handle_across_blocks`
  had the same shared seed; it now uses one per block too.

## What this does not claim

No mainnet-scale or timing claim, no Docker evidence, and no claim that the reserve's economics are
sound. The change is consensus-visible and pre-genesis only; there is no launched chain to migrate.
