import Rchain.Laws.Base

/-!
# The law register, chunk 04 (laws 44–59)

One module of the register catalog. The register is split across modules because elaborating it as a
single module peaked at ~15 GiB on the 16 GiB CI runner (measured 2026-11-23); a module boundary is a
process boundary, so each chunk's elaboration memory is bounded. The register's numbering check
(`1..lawCeiling`) and `entryCeiling` make a dropped or duplicated row across chunk boundaries a build
failure. See `Rchain/Laws.lean` for the assembly and the measurement.
-/

set_option autoImplicit false
set_option maxHeartbeats 4000000

namespace Rchain
namespace Laws

def lawsChunk04 : List Law := [
  -- ── Proof-of-Stake: the epoch, its split, and the withdrawal (Laws 44–47) ───────────────────────
  { number := 44, layer := "PoS",
    rustWitness := [
      "rholang/src/native_state.rs:the_epoch_gate_does_nothing_off_a_boundary",
      "rholang/src/native_state.rs:bond_escrows_the_stake_and_activates_at_the_boundary",
      "rholang/src/property_tests.rs:law44_the_absence_rule_never_raises_a_reward",
      "rholang/src/property_tests.rs:law44_a_validator_inside_the_grace_is_kept",
      "rholang/src/property_tests.rs:law44_a_validator_outside_the_knee_is_not_paid",
      "rholang/src/property_tests.rs:law44_a_validator_that_never_spoke_is_not_paid",
      "rholang/src/property_tests.rs:law44_a_zero_knee_withholds_nothing",
      "rholang/src/property_tests.rs:law44_between_the_grace_and_the_knee_a_validator_is_paid_in_part",
      "rholang/src/native_state.rs:the_boundary_pays_on_the_participation_it_is_handed",
      "casper/src/runtime_manager.rs:a_boundarys_participation_reaches_play_and_replay_alike",
      "rholang/src/native_state.rs:the_developer_reserve_is_paid_a_share_of_what_the_deploy_burned",
      "rholang/src/native_state.rs:a_zero_developer_share_writes_no_leaf_and_pays_nothing",
      "rholang/src/native_state.rs:a_developer_reserve_the_genesis_cannot_honour_is_refused",
      "rholang/src/native_state.rs:the_fee_split_floors_each_share_and_refuses_more_than_the_whole",
      "casper/src/merging.rs:the_developer_reserve_composes_across_concurrent_blocks"],
    statement := "Membership takes effect at an **epoch boundary**: the epoch sequence runs only when \
      `blockNumber % epochLength = 0`, and off a boundary a bond is pooled but not activated, a \
      withdrawal is staged but not moved, and no claim is paid. The same machine carries the **slash**, \
      which is graded by the offence (AUDIT C199): `bps` basis points of everything the validator holds \
      in the PoS system — its bond, its accrued rewards and an escrowed claim — go to the Coop vault and \
      the remainder returns to its own vault, so the loss is bounded by the tier and a milder tier never \
      takes more. And the same vault pays for **work**: a share of a deploy's burned phlo goes to the \
      block's own signed sender (`payExecutor`), which is a transfer inside the staking vault — it moves \
      income, never stake, and it is monotone in what the deploy burned. The same move can pay a \
      **developer fee reserve** (#655, `payFees`): a second share, committed at genesis, to an address \
      the genesis names; the two shares together are at most the whole burn (`fee_split_le_burned`, and \
      `shares_above_the_whole_overpay` shows the bound is load-bearing), so the split conserves value \
      (`payFees_conserves`), and a zero reserve is exactly the producer's share alone \
      (`a_zero_reserve_is_the_old_rule`). A boundary may also **scale** a \
      drawn validator's reward by its participation (`absenceAdjusted`, over `participationWeight`), which \
      is income only: the weight is a function of the reward and of how far behind the last finalised \
      fringe the validator's latest *message* sits, so no stake is reachable from it. The weight is the \
      whole `10000` inside a **grace**, ramps linearly to `0` at a **knee**, and is never more than the \
      whole — which is what carries law 46 through the rule (`weighted_rewards_le_pot`: the split is \
      unchanged, and the rule only multiplies its outputs by at most one). **A partial weight withholds** \
      (`absence_withholds`), and that theorem is deliberately paired with the decided instance \
      `the_ramp_is_a_ramp`, because it is an *implication*: a weight that was always `10000` satisfies it \
      **vacuously** — measured, not argued, since that mutation fails the instance and leaves the \
      implication alone. Neither is a check without the other, which is the absence rule's own vacuity \
      one level up. `grace = knee` reproduces a binary threshold rule rather than losing it, and \
      `knee = 0` is the off switch",
    status := .provedModel,
    declarations := [`Rchain.PosState, `Rchain.PosClaim, `Rchain.PosRequest, `Rchain.totalRev,
      `Rchain.divisor, `Rchain.isBoundary, `Rchain.bond, `Rchain.epochStep, `Rchain.closeBlock,
      `Rchain.payDue, `Rchain.payDue_conserves, `Rchain.the_ledger_steps_leave_the_coins,
      `Rchain.epochStep_conserves, `Rchain.closeBlock_off_a_boundary,
      `Rchain.a_bond_pools_but_does_not_activate, `Rchain.a_boundary_activates_the_pool,
      `Rchain.atRisk, `Rchain.malicious, `Rchain.misdemeanour, `Rchain.honestMistake, `Rchain.slash,
      `Rchain.taken_le_risk, `Rchain.slash_conserves, `Rchain.a_milder_tier_takes_no_more,
      `Rchain.slash_clears_every_ledger,
      `Rchain.executorShare, `Rchain.producer_share_le_burned, `Rchain.payExecutor,
      `Rchain.payExecutor_conserves, `Rchain.payExecutor_leaves_the_stake,
      `Rchain.producer_pay_is_monotone,
      `Rchain.payReserve, `Rchain.payFees, `Rchain.fee_split_le_burned,
      `Rchain.shares_above_the_whole_overpay, `Rchain.payFees_conserves,
      `Rchain.payFees_leaves_the_stake, `Rchain.a_zero_reserve_is_the_old_rule,
      `Rchain.the_split_is_a_split,
      `Rchain.absenceAdjusted, `Rchain.absence_never_raises,
      `Rchain.a_returning_validator_is_paid_in_full, `Rchain.the_absence_rule_moves_no_stake,
      `Rchain.absence_withholds,
      `Rchain.participationWeight, `Rchain.participationWeight_le, `Rchain.participationWeight_full,
      `Rchain.participationWeight_zero, `Rchain.participationWeight_antitone,
      `Rchain.nsum_zipWith_le_map, `Rchain.weighted_rewards_le_pot,
      `Rchain.absence_withholds_past_the_knee, `Rchain.the_ramp_is_a_ramp],
    axioms := [],
    rust := ["rholang/src/native_state.rs"],
    witness := [`Rchain.closeBlock_off_a_boundary, `Rchain.epochStep_conserves, `Rchain.the_ledger_steps_leave_the_coins, `Rchain.a_bond_pools_but_does_not_activate, `Rchain.a_boundary_activates_the_pool, `Rchain.slash_conserves, `Rchain.a_milder_tier_takes_no_more, `Rchain.slash_clears_every_ledger, `Rchain.taken_le_risk, `Rchain.payExecutor_conserves, `Rchain.payExecutor_leaves_the_stake, `Rchain.producer_pay_is_monotone, `Rchain.fee_split_le_burned, `Rchain.shares_above_the_whole_overpay, `Rchain.payFees_conserves, `Rchain.payFees_leaves_the_stake, `Rchain.a_zero_reserve_is_the_old_rule, `Rchain.the_split_is_a_split, `Rchain.absence_never_raises, `Rchain.a_returning_validator_is_paid_in_full, `Rchain.the_absence_rule_moves_no_stake, `Rchain.absence_withholds, `Rchain.participationWeight_le, `Rchain.participationWeight_full, `Rchain.participationWeight_zero, `Rchain.participationWeight_antitone, `Rchain.weighted_rewards_le_pot, `Rchain.absence_withholds_past_the_knee, `Rchain.the_ramp_is_a_ramp],
    falsifiable := some "`the_epoch_gate_does_nothing_off_a_boundary` builds the off-boundary state — a \
      staged withdrawal *and* a full reward pot — and asserts the whole state is unchanged, then that \
      the same call at the boundary moves it and pays it; the bond half is in the same test (pooled at \
      once, still not active at a non-boundary block), and the two membership changes carry their own: \
      `bond_escrows_the_stake_and_activates_at_the_boundary`, \
      `withdraw_stages_the_validator_until_the_next_boundary`",
    note := "**the state machine now exists, so the row is a model claim rather than a Rust claim** \
      (2026-09-23). `Rchain/Pos.lean` grew `PosState` (the three coin fields, the pool, the active map, \
      the three ledgers, the two parameters), the gate (`isBoundary`, `divisor = max(epochLength, 1)`), \
      and the four steps in `close_block`'s order (`epochStep` = commit → move → pay → reselect), with \
      the gate *outside* the transition so `closeBlock_off_a_boundary` is a fact about the gate rather \
      than a restatement of a branch. What the row's statement names is now proved: a bond pools the \
      stake and leaves the active map alone (`a_bond_pools_but_does_not_activate`), the boundary is what \
      activates it **with the stakes it selects** (`a_boundary_activates_the_pool`, a statement about \
      the selected pairs rather than a set of ids since AUDIT C92's model change), and **an epoch \
      conserves** — every transfer is a \
      vault-to-vault move, so user vaults + staking vault + Coop vault are invariant \
      (`epochStep_conserves`, from `payDue_conserves` plus the observation that the other three steps \
      are ledger steps). **The refusal is structural, not a hypothesis**: `payDue` is partial \
      (`Option`) and returns `none` when the vault cannot cover the payout, because the port's \
      `debit_pos_vault` *fails* the transfer (`native_state.rs:debit_pos_vault`, its refusal inside `debit_pos_vault`) and \
      `close_block` (`native_state.rs:close_block`) writes nothing on that path — the debit's `?` inside it \
      returns before any of its state writes — an unguarded `Nat` subtraction would truncate the debit and mint the \
      difference, which is the quiet-wrong-answer shape this project refuses everywhere else. Two \
      falsifications, both run: deleting the payout's vault debit makes `payDue_conserves` unprovable, \
      and weakening the guard (`≤` to always-pay) does the same — the second is the one that shows the \
      guard is what keeps the theorem true. The earlier note, kept for the record: \
      not `close_block`'s four steps or the gate over them, so the statement above is a claim about the \
      Rust, evidenced by those tests and anchored here. What is worth formalizing is not the gate \
      itself (`if boundary then … else s` restates its own definition, which is the `vacuous` shape) \
      but the **conservation** an epoch preserves: the staking vault plus the Coop vault plus every user \
      vault is invariant, which is what makes a payout a transfer rather than a mint. That is this \
      row's Programme C item. **The absence rule's third form, and what it cost (2026-10-02, #150).** \
      It was first a filter over a per-validator record in native state (`pos:last_spoke`), read out of \
      the boundary block's **pre-state** — which is a function of the proposer's justification set, and \
      nothing requires a block to justify everything it has seen, so the penalty could be *aimed* at a \
      chosen rival. It also measured the wrong event: an attestation block carries no system deploys and \
      so recorded nothing, while an attestation *is* a message. It now reads a participation map derived \
      from the **last finalised fringe**, which is what the epoch seed reads and for the same reason, and \
      the record it replaced is **retired** — leaf, codecs, op, proto field (reserved) and variant, with \
      the block-level system-deploy list losing an entry. **Two things that cost are stated here rather \
      than left to the diff**: retiring the entry shifts every positional seed in that list down by one, \
      which is a consensus change of the same class as an insertion; and a block written before the \
      retirement is refused at **replay** rather than at the wire, because an unknown proto oneof field \
      decodes to `Empty` and `replay_block_system_deploy` is what rejects an `Empty` entry in a block's \
      system-deploy list. The read's residual is the seed's own: the steering space is \"one per \
      reachable fringe\", so a stale fringe moves every validator's reading back together and singles \
      nobody out, but it is a reduction and not a closure" },
  { number := 45, layer := "PoS",
    rustWitness := ["rholang/src/native_state.rs:an_epoch_splits_the_pot_and_keeps_the_dust"],
    statement := "The epoch's split: `pot * (bondᵢ / minimumBond) / (activeBonds / minimumBond)` per \
      active validator, out of `pot = posBalance − totalBond − totalWithdraw − committedRewards`, \
      committed per validator and paid only when the validator leaves",
    status := .provedModel,
    declarations := [`Rchain.rewardPot, `Rchain.reward],
    rust := ["rholang/src/native_state.rs"],
    witness := [`Rchain.the_dust_is_real],
    falsifiable := some "`Rchain.the_dust_is_real` decides an instance: minimum bond 3, bonds `[4, 8]`, \
      pot 10 — the validators' scaled shares are `4/3 = 1` and `8/3 = 2`, so they are paid 2 and 5 and \
      the epoch distributes **7 of 10**. A statement that said the shares sum to the pot is refuted by \
      that line, and so is one that dropped either division. On the Rust side \
      `an_epoch_splits_the_pot_and_keeps_the_dust` builds exactly that state (bonds 4 and 8, \
      `minimum_bond` 3, a pot of 10 paid in as phlo) and reads the split back, so the implementation is \
      checked against the arithmetic rather than against a remembered number. **The two bonds differ by \
      more than one `minimumBond` for a reason** (AUDIT C149): this instance used to bond `[4, 5]`, where \
      both scaled shares are 1, so the factor `bond / minimumBond` under test was the identity and a \
      `reward` that dropped it satisfied this line exactly as the real one did — the degeneracy is in the \
      fixture's parameters, so no assertion-level reading and no coverage count can see it",
    note := "the formula is the Scala's `getCurrentEpochRewards` (`casper/src/genesis/resources/Pos.rhox:241-256`), and the port's \
      `epoch_reward` agrees with it wherever the contract is *defined*; where it is not — \
      `minimumBond = 0`, or a normaliser of zero — the contract divides by zero and faults the deploy, \
      and the port pays zero instead (registered in `spec/audit/passes.md` §6). The model's `reward` is total, so \
      the two-part statement is: the model is the formula, and the Rust is the model on the model's \
      domain" },
  { number := 46, layer := "PoS",
    rustWitness := ["rholang/src/native_state.rs:an_epoch_splits_the_pot_and_keeps_the_dust",
      "rholang/src/property_tests.rs:law46_the_shares_never_exceed_the_pot"],
    statement := "The split **does not conserve**: `Σ rewards ≤ pot`, and the difference is the dust of \
      two integer divisions — which is not lost but stays in the pot for the next epoch",
    status := .provedModel,
    declarations := [`Rchain.sum_rewards_le_pot, `Rchain.list_sum_div_le, `Rchain.div_add_div_le,
      `Rchain.nsum_map_mul_left, `Rchain.the_dust_is_real],
    rust := ["rholang/src/native_state.rs"],
    witness := [`Rchain.the_dust_is_real, `Rchain.sum_rewards_le_pot],
    falsifiable := some "the inequality is **strict in an instance**: `the_dust_is_real` is minimum \
      bond 3, bonds `[4, 8]`, pot 10, seven units distributed of ten (`decide`d, so the strictness is a \
      computation rather than a remark), and `an_epoch_splits_the_pot_and_keeps_the_dust` reads the \
      same three units of dust back out of the Rust's pot afterwards — the dust is still there for the \
      next epoch. **The bonds are 4 and 8, and that gap is load-bearing** (AUDIT C149): they were 4 and \
      5, where `4/3 = 5/3 = 1`, so the factor `bond / minimumBond` under test was the identity for every \
      validator the instance built and a `reward` that dropped the factor satisfied it exactly as the \
      real one did",
    note := "the row this register most needed from Programme B: a conservation law written as an \
      *equality* would have been false, and the Scala's own comment does not say which it means. The \
      two divisions are the whole content — the model decides the question by computing `Pos.rhox`'s \
      arithmetic in `Nat` — and the Lean statement's hypothesis is the case the contract leaves \
      defined (`0 < activeBonds / minimumBond`), which is the same boundary `spec/audit/passes.md` §6 records on \
      the Rust side" },
  { number := 47, layer := "PoS",
    rustWitness := [
      "rholang/src/native_state.rs:withdraw_stages_the_validator_until_the_next_boundary",
      "rholang/src/native_state.rs:a_released_withdrawal_pays_the_bond_plus_the_committed_rewards"],
    statement := "A withdrawal is **staged**: the request records `quarantineLength + epochLength * \
      (1 + blockNumber / epochLength)` and changes nothing else; the validator leaves the pool at the \
      next boundary, and is paid `bond + committed rewards` at the first boundary past its quarantine",
    status := .provedModel,
    declarations := [`Rchain.PosClaim, `Rchain.PosRequest, `Rchain.withdrawDeadline, `Rchain.stage,
      `Rchain.movePending, `Rchain.dueClaims, `Rchain.payoutOf, `Rchain.duePayout, `Rchain.setKey,
      `Rchain.lookup, `Rchain.a_staged_withdrawal_moves_no_coins,
      `Rchain.the_move_escrows_the_bond_and_pays_nothing,
      `Rchain.a_due_claim_is_paid_its_bond_plus_its_committed,
      `Rchain.a_claim_before_its_deadline_is_not_paid,
      `Rchain.the_move_does_not_disturb_the_ledger,
      `Rchain.the_reward_is_committed_before_the_leave],
    axioms := [],
    rust := ["rholang/src/native_state.rs"],
    witness := [`Rchain.a_staged_withdrawal_moves_no_coins, `Rchain.the_move_escrows_the_bond_and_pays_nothing, `Rchain.a_due_claim_is_paid_its_bond_plus_its_committed, `Rchain.a_claim_before_its_deadline_is_not_paid, `Rchain.the_move_does_not_disturb_the_ledger, `Rchain.the_reward_is_committed_before_the_leave],
    falsifiable := some "`withdraw_stages_the_validator_until_the_next_boundary` asserts the three \
      stages separately — still bonded and still active with a deadline after the request; out of the \
      pool and escrowed (vault balance still zero) after the first boundary; paid after the second — \
      and `a_released_withdrawal_pays_the_bond_plus_the_committed_rewards` asserts the *sum* \
      (`bond 40 + committed 5 = 45`) **and** that the staking vault is exactly emptied by it, which \
      fails if the payout is not the claim's own two parts",
    note := "the contract's ordering is the law's substance, and **it is now modelled** (2026-09-23): \
      the reward is committed *before* the move, so a validator earns in the epoch it leaves and is paid \
      that reward later, when it is no longer in the pool — which is why the claim stores the bond and \
      reads the reward from the committed map at payment time \
      (`casper/src/genesis/resources/Pos.rhox:582`, the `withdrawers` write, and `:604`, the \
      `payWithdrawer` send; the contract's \
      header comment describing the stored pair as `bond + reward` is the *payee's* sum, not the \
      record's). The three stages are three theorems over `Rchain/Pos.lean`'s state machine: staged \
      (`a_staged_withdrawal_moves_no_coins` — the deadline is `quarantineLength + epochLength * (1 + n / \
      divisor)` and the pool, active set and coins are untouched), escrowed \
      (`the_move_escrows_the_bond_and_pays_nothing` — the claim carries the bond and the payer's vault \
      is still empty), and paid (`a_due_claim_is_paid_its_bond_plus_its_committed`; \
      `a_claim_before_its_deadline_is_not_paid` is the other half of 'only expired quarantines'). The \
      ordering itself is `the_move_does_not_disturb_the_ledger` plus \
      `the_reward_is_committed_before_the_leave`, which is the port's own test run as a decided \
      instance; the *general* form of that instance (a `foldl` induction over `setKey` for any pool and \
      any reward function) remains owed, and is recorded as owed rather than glossed — the instance \
      catches a dropped or reordered commitment, the general lemma would catch nothing more about \
      *this* mechanism. **Read against the contract before accepting that argument (2026-09-24), and \
      it holds**: the removal *is* a `ListOps fold` \
      (`casper/src/genesis/resources/Pos.rhox:607`, `computeRemove`), but it folds over a literal \
      `setKey` sequence with no parametric pool or reward function to vary, so a general `foldl` \
      induction would quantify over a function the contract does not have — and what the ordering \
      actually is, and what the decided instance pins, is the *phase* order: `payWithdraw` reads \
      `committedRewards` at `:604` and `computeRemove` deletes it only from `:610`, the payments \
      awaited first (`:601`). Owed with that reason, rather than as an aspiration" },
  { number := 48, layer := "Casper",
    statement := "A **denied** deploy's effects are excluded from the merged state, and the merge's \
      objective counts its cost exactly as an included deploy's — so the fee consequence RCHIP-02 \
      proposes (the deployer is not charged, the validator is not rewarded) holds in neither tree",
    status := .retired,
    rust := ["casper/src/merging.rs", "rholang/src/native_state.rs", "docs/src/node/block-merge.md"],
    falsifiable := some "the statement is a claim about what the port *does not* do, so its falsifier \
      is an implementation: a test that refunds a denied deploy's phlo, or one that drops its cost \
      from the merge objective, would refute it. Both halves are visible today — a denied deploy \
      appears in `BlockMessage::rejected_deploys` while its `deploy_chain_cost` still counts toward \
      `deployChainCost`'s sum, and its phlo was taken by the pre-charge and now funds the epoch pot \
      (laws 45/46) rather than being returned",
    note := "**retired 2026-09-25: the port has no rule of this shape** — the first half of this row's \
      statement is true *by construction*, the second is a proposal, and so there is nothing here to \
      prove. That is a decision recorded, not an omission. **The construction, first**: the port's merge \
      builds its event-log index from the deploys it is handed and from nothing else — \
      `DeployChainIndex::apply`'s `for d in deploys { EventLogIndex::combine(&event_log_index, \
      &d.event_log_index) }` (`casper/src/merging.rs:192-215`) — and its caller hands it the **accepted** \
      set, with the denied deploys carried beside it in `rejected_deploys` (`:98-100`). A denied \
      deploy's effects are therefore absent from the merged state because they are never read: a fact \
      about a `for` loop, whose theorem would be one that cannot fail — and the rule this register \
      applies to its own models (G6's, and C95's) is that a statement holding by construction is a \
      statement nothing can falsify. So the first half is recorded rather than modelled, and what is \
      left is the second, which is a **design decision and not a gap. And it is a question shared with \
      the Scala, not a port divergence.** RCHIP-02 is a \
      proposal: `MergeScope.scala:87` defaults `rejectionCost` to `DeployChainIndex.deployChainCost`, \
      and `DeployChainIndex.scala:73` defines that as `deploysWithCost.map(_.cost).sum` — the same \
      objective this port's `merging.rs` computes over the same set. Nothing in either tree refunds a \
      denied deploy, and `docs/src/node/block-merge.md`'s open-items list has said so since the merge \
      work landed. **What Programme B changed about it**: before the staking vault existed the \
      deployer's phlo was *burned*, so 'the validator is not rewarded' was true for want of any \
      reward; now the phlo is in the pot and the epoch distributes it to the active set (laws 45/46), \
      so the RCHIP's second half has become false in a new way, indirectly and at epoch granularity — \
      and the port's epoch rewards have no per-deploy counterpart in which 'the validator is not \
      rewarded *for this deploy*' could even be stated. Closing it means choosing an objective and a \
      refund path that the Scala does not have, which is a consensus change without an oracle; the \
      decision taken is to leave it open *with the reason*, in the row and in the doc, rather than to \
      implement a proposal and call it a port" },
  -- ── Rholang: what a deploy is charged (Law 49) ─────────────────────────────────────────────────
  { number := 49, layer := "Rholang",
    rustWitness := ["rholang/src/storage.rs:a_matched_produce_refunds_its_storage_before_the_event_costs"],
    statement := "For a matched produce/consume the charged gas is the Scala's: the storage is charged       up front and what the match consumed is **refunded** — the continuation's consume storage and the       produce storage of every removed datum — *before* the event and COMM costs",
    status := .provedModel,
    declarations := [`Rchain.prefixes, `Rchain.peak, `Rchain.itotal, `Rchain.peak_four,
      `Rchain.peak_refunds_first, `Rchain.itotal_refunds_first],
    rust := ["rholang/src/storage.rs"],
    witness := [`Rchain.peak_refunds_first, `Rchain.itotal_refunds_first],
    falsifiable := some "two halves, and the Rust test asserts both: \
      `a_matched_produce_refunds_its_storage_before_the_event_costs` (1) checks the matched call's total \
      against the same op *without* a match plus the COMM cost minus the two refunds — which fails if a \
      refund is dropped — and (2) finds a balance between the two peaks at which the deploy completes, \
      failing one phlo below it. The Lean side carries the second half as arithmetic: \
      `peak_refunds_first` says the refunds-first order's peak is at most the other's, and \
      `itotal_refunds_first` says their totals are *equal* — so a test that checked only the total \
      would pass with the refunds charged last, and (2) is what cannot",
    note := "**the first law about gas rather than about state**, and the reason it needed a model is \
      the *peak*, not the sum: `CostAccounting::charge` refuses a step that would take the balance \
      negative, so what a deploy needs is the largest prefix total of its charges. A refund credited \
      before the event and COMM charges lowers that peak; one credited after them does not — the two \
      sequences then need 192 and 200 phlo respectively for the same 184 of work, which is what the \
      test's tight balance measures. The port charged the storage and never refunded it, recording the \
      gap as a 'safe over-charge': safe because no deployer is ever *under*-charged, which is exactly \
      why it survived review. Modelled at the instance the charge sequence has (two charges, two \
      refunds) rather than in general: the general form is the same argument iterated, and the \
      iteration needs a permutation lemma over `List` that carries no further content — `Charging.lean` \
      says so rather than proving a statement no caller uses" },
  { number := 50, clause := "a", layer := "Rholang",
    statement := "A term's **AST depth** is bounded: `parDepth` is the quantity, `maxAstDepth` (768) \
      is the bound, and the parser refuses a source whose tree exceeds it — so no consumer of a term \
      (the normalizer, the sorter, `well_scoped`, the evaluator, the matcher, the printer) recurses \
      deeper than the bound, on a stack the bound was measured against",
    status := .provedModel,
    declarations := [`Rchain.parDepth, `Rchain.maxAstDepth, `Rchain.notsDepth,
                     `Rchain.walkPar, `Rchain.walkExceeds, `Rchain.walkParDroppingExpr,
                     `Rchain.walkPar_iff_parDepth, `Rchain.walkExceeds_sound,
                     `Rchain.walkExceeds_complete, `Rchain.parDepth_notsDepth,
                     `Rchain.a_dropped_arm_breaks_soundness,
                     `Rchain.the_walk_refuses_the_mutant_witness],
    witness := [`Rchain.walkPar_iff_parDepth, `Rchain.walkExceeds_sound,
                `Rchain.walkExceeds_complete, `Rchain.a_dropped_arm_breaks_soundness,
                `Rchain.parDepth_notsDepth,
                `Rchain.the_walk_refuses_the_mutant_witness],
    rust := ["rholang/src/parser.rs"],
    rustWitness := [
      "rholang/src/parser.rs:rejects_a_deep_ast_that_stays_inside_both_component_guards",
      "rholang/src/parser.rs:accepts_a_deep_ast_under_the_budget",
      "rholang/src/parser.rs:a_maximal_chain_still_parses",
      "rholang/src/parser.rs:every_construct_in_the_parser_walk_is_descended"],
    falsifiable := some "**the walk's own children function is where it fails, so the falsifier is a \
      dropped arm — and it is a theorem now, not a comment.** `walkPar_iff_parDepth` is the agreement \
      (`walkPar p k = false ↔ parDepth p ≤ k`), of which the two directions this row quotes are \
      `walkExceeds_sound` and `walkExceeds_complete`. The witness is `notsDepth`, whose depth is \
      `parDepth_notsDepth`'s `2 * n + 1`: `a_dropped_arm_breaks_soundness` shows the walk with its \
      `exprs` arm deleted (`walkParDroppingExpr`) accepting `notsDepth 384` — depth 769, one past the \
      bound — so a walk that could not be broken that way would be a restatement of the definition \
      rather than a check of it, which is the vacuity law 22 records. \
      `the_walk_refuses_the_mutant_witness` is the contrast: the real walk refuses the very term the \
      mutant admits. **The witness had to be repaired to make that true** (2026-09-28): `notsDepth` \
      was `List.replicate n (enot unit)` — `n` *siblings*, so `parDepth` was 3 for every `n ≥ 1` and \
      the mutation test would have passed **vacuously**, accepting at the bound along with every \
      mutant of it. **The half that is not a Lean claim at all**: that the Rust walk descends into \
      every `Proc` constructor. No theorem can say it — this walk is over the de Bruijn `Par`, and \
      `exceeds_ast_depth` walks the parser's surface `Proc` — so it is pinned by \
      `every_construct_in_the_parser_walk_is_descended`, which parks a deep child in each position \
      `push_sub_procs` must reach",
    note := "**`proved-model` (2026-09-28), and the reason it was `owed` for two days is worth \
      keeping** (2026-09-26, AUDIT C99). The parser's older guards bound two *shapes* and compose into \
      nothing: `MAX_PARSE_DEPTH` bounds the parser's recursion, and a flat chain is built by a loop — \
      bounded frames, an AST of depth `n` — so `d` levels of `c` operators compose into an AST of \
      depth `d × c` with both guards satisfied. Measured on the node's own 32 MiB worker: a debug \
      build **aborts the process** between AST depth 732 and 994 (a ~4 KB deploy term, SIGABRT), and a \
      release build costs **64 s** at 8,040 and aborts at 50,100 — while the deploy path runs parse \
      and normalize *before* any phlo or balance check, so that is free to the submitter. The fix is \
      `MAX_AST_DEPTH` + the walk, and this row is that fix's *quantity*; the agreement between the two \
      is `walkPar_iff_parDepth`, in `Rchain/Depth.lean`'s 23-member `mutual` theorem block — the \
      recipe this row names, `Rchain/FreeVars.lean`'s, and the block carries **no `termination_by` \
      and no `decreasing_by`**. \
      **Why it took three attempts, and the correction** (2026-09-28): this file's own note had blamed \
      `termination_by`. `Rchain/FreeVars.lean` falsifies that — its definitions carry it and its 23 \
      theorems sit in a block without it. The variable was where the extra `Nat` sits: `FreeVars` \
      takes the recursed term first with the level as a plain pattern variable, while the walk as \
      first committed matched the budget *in the equation header*, which routes the definition through \
      a well-founded fixpoint whose equations the theorems must then be proved about. The reshape is \
      the row's own recipe applied correctly. Dropping `termination_by` was never the fix, and the \
      attempt that dropped it paid in `Decidable`. \
      **What this row does not claim.** `rholang/src/parser.rs::exceeds_ast_depth` walks the surface \
      `Proc`; its own doc comment calls its count \"a proxy — and a deliberate one\", because the `Par` \
      \"adds a small constant per construct\". So `walkPar_iff_parDepth` is an agreement *inside the \
      model*, and the bridge to the parser's tree is a modelling argument rather than a theorem. \
      **One of the consumers this clause's statement names is not bounded by this constant, and is \
      handled rather than bounded** (2026-09-26, AUDIT C101): the *evaluator*'s frames are larger than \
      the normalizer's — a parsed flat chain as send data (inside both parser guards) returned `Ok` at \
      300 and 320 links and **aborted the process** at 350, below this bound — and the fix is that a \
      left-nested operator chain is now evaluated **iteratively**, so its length costs no stack and no \
      second constant is needed. So the sentence above holds for the normalizer, the sorter, \
      `well_scoped`, the matcher and the printer *by this bound*, and for the evaluator *by \
      construction*; lowering this constant instead would have refused every chain of 257..512 links \
      the chain guard admits" },
  { number := 50, clause := "b", layer := "Rholang",
    statement := "A **runtime-built value** is depth-bounded on the route the parser cannot see: the \
      space refuses a produced value whose **`Par`-nesting** exceeds `maxValueDepth` (256), so a value \
      a program built by folding cannot reach a consumer with more than 256 `Par` levels — and at most \
      `3 * 256 = 768` of clause a's `parDepth`, the same number as `maxAstDepth`, which is the honest \
      form of the relation between the two quantities rather than an identification of them",
    status := .provedModel,
    declarations := [`Rchain.parDepth, `Rchain.maxValueDepth, `Rchain.pairsDepth,
                     `Rchain.parNestDepth, `Rchain.walkValuePar, `Rchain.valueWalkExceeds,
                     `Rchain.walkValueParDroppingExpr, `Rchain.walkValuePar_iff_parNestDepth,
                     `Rchain.valueWalkExceeds_sound, `Rchain.valueWalkExceeds_complete,
                     `Rchain.valueWalkExceeds_sound_parDepth,
                     `Rchain.parDepth_le_three_mul_parNestDepth,
                     `Rchain.parNestDepth_pairsDepth, `Rchain.parDepth_pairsDepth,
                     `Rchain.a_dropped_value_arm_breaks_soundness,
                     `Rchain.the_value_walk_refuses_the_mutant_witness],
    witness := [`Rchain.walkValuePar_iff_parNestDepth, `Rchain.valueWalkExceeds_sound,
                `Rchain.valueWalkExceeds_complete, `Rchain.valueWalkExceeds_sound_parDepth,
                `Rchain.parDepth_le_three_mul_parNestDepth, `Rchain.parNestDepth_pairsDepth,
                `Rchain.parDepth_pairsDepth, `Rchain.a_dropped_value_arm_breaks_soundness,
                `Rchain.the_value_walk_refuses_the_mutant_witness],
    rust := ["models/src/types.rs", "rholang/src/storage.rs"],
    rustWitness := [
      "models/src/types.rs:the_value_walk_admits_the_limit_and_refuses_past_it",
      "models/src/types.rs:every_construct_that_carries_a_par_is_walked",
      "rholang/src/storage.rs:a_value_at_the_bound_is_stored_and_one_past_it_is_refused_on_both_produce_paths"],
    falsifiable := some "**the falsifier is a dropped arm of the walk's children function, over the \
      value route's own shape — and it is a theorem now, not a comment.** \
      `walkValuePar_iff_parNestDepth` is the agreement (`walkValuePar p k = false ↔ parNestDepth p ≤ \
      k`), of which `valueWalkExceeds_sound` and `valueWalkExceeds_complete` are the two directions \
      this clause quotes. `pairsDepth` — nested pairs, what a folding contract builds and what an \
      attacker builds — has `parDepth_pairsDepth`'s `2 * n + 1` of `parDepth` against \
      `parNestDepth_pairsDepth`'s `n + 1` of nesting, so at `maxValueDepth` the guard admits \
      `pairsDepth 255` (`parDepth` 511) and refuses `pairsDepth 256` (`parDepth` 513). \
      `a_dropped_value_arm_breaks_soundness` shows the walk with its `exprs` arm deleted \
      (`walkValueParDroppingExpr`) accepting `pairsDepth 256` at the bound — so a walk that could not \
      be broken that way would be a restatement of the definition rather than a check of it, which is \
      the vacuity law 22 records — and `the_value_walk_refuses_the_mutant_witness` is the contrast. \
      **The Rust half of the same risk** (a constructor the walk never descends into) is not a Lean \
      claim at all — this walk is over the de Bruijn `Par`, the Rust value walk is over the Rust AST, \
      which has no Lean model — and is pinned by \
      `models/src/types.rs:every_construct_that_carries_a_par_is_walked`, which parks a depth-10 child \
      in each of the 22 positions the walk must reach and asks for a limit of 5. Verified by mutation \
      before it was trusted: disabling the `bundles` arm makes exactly that case fail",
    note := "**`proved-model` (2026-09-28), and the number is a measurement rather than a preference** \
      (2026-09-26, AUDIT \
      C100). A contract folding its accumulator into a deeper pair reaches depth `n` in `O(n)` reduce \
      steps, inside `DEFAULT_MAX_REDUCE_STEPS`; on the node's 32 MiB worker in a debug build a fold to \
      depth 101 costs 7.3 s of CPU and a fold to depth 401 **aborts the process** \
      (`thread 'tokio-rt-worker' has overflowed its stack`, SIGABRT) inside `eval_single_expr`'s \
      recursion over the value. So the bound sits below that abort, at 256, and *apart from* clause \
      a's 768 — the two walks have different frames (the parser route's overspill is ~1,000, the value \
      route's is ~401), and sharing one number would be a bound that does not fire before the crash it \
      exists to prevent, which is what this unit's first draft shipped. Depth counts *nesting*, not \
      length: a flat list of any size is depth 2, and it is the accumulator-nesting shape — which no \
      contract needs — that this refuses. \
      **What the guard covers, and the residues it does not.** It sits at \
      `rholang/src/storage.rs::ChargingRSpace::check_value_depth`, called by **both** produce paths, so \
      it bounds the values the space holds and therefore every later reader of them. Four routes \
      remain unguarded and are named rather than implied away: a produce's *channel*, a consume's \
      channels/patterns, a continuation's body, and state restored from persistence at boot. It also \
      does not bound the **evaluator's** recursion while building the value, because the guard runs \
      after that walk — the honest statement of the mechanism is that the deepest value ever built is \
      capped at the bound + 1 (iteration `i` evaluates depth `i` and produces depth `i + 1`). \
      **The accounting the owed statement got wrong, and what replaced it** (2026-09-28): this row \
      said the Rust walk gives an `Expr` node **no level of its own** while `parDepth` counts it, and \
      that the gap is therefore a slack bounded by `MAX_PARSE_DEPTH` (128) because \"no runtime path \
      builds `Expr` nodes\". Every clause of that is false. The walk gives **no element node** a level \
      — `push_value_fields` pushes its fields at the depth it was handed, as do `push_value_expr` and \
      `push_value_connective`, so `Send`/`Receive`/`New`/`Match`/`Bundle`/`MatchCase`/`Connective` \
      are transparent along with `Expr` — so the counted quantity is the number of **`Par` nodes** on \
      the deepest `Par`-chain, which is `Rchain.parNestDepth` in `Rchain/ValueDepth.lean` and not \
      `parDepth` at all. And a runtime path *does* build `Expr` nodes: `(a, b)` **is** \
      `Expr::ETuple`, built by the reducer, which is the shape `rholang/tests/deep_value_bound.rs` \
      exists to exercise. The gap is a *factor*, not a constant: \
      `Rchain.parDepth_le_three_mul_parNestDepth` proves it is at most three, and three is tight — two \
      element nodes can sit between consecutive `Par`s (`Receive`→`ReceiveBind`, \
      `Match`→`MatchCase`), so a `Match` ladder spends three levels per `Par` and \
      `parDepth ≤ 2 * parNestDepth` fails from two rungs. At this bound that is `3 * 256 = 768`. \
      **Two boundaries of the tie are named rather than hidden**: the Rust guard never pushes its root \
      (its fields go on the worklist at depth 2), so it accepts a value with no `Par` child when \
      `limit = 0` where the model refuses it, and the two agree for every `limit ≥ 1` — which is what \
      `maxValueDepth` is; and the guard walks `New.injections`, which the model's `New.mk` does not \
      have, so it is *stricter* there, which is the safe direction. \
      **The gap this unit's own claim hid**: the guard's first draft sat inside `produce`, under a \
      comment calling it the one place a value enters the space; \
      `rholang/src/storage.rs::produce_at` — the scheduled path the channel scheduler and the deferred \
      block paths use — reaches RSpace directly and never passed through it, so one of two produce \
      entries was open. One shared method now, and the test fails on the old shape: with the scheduled \
      call disabled, a 257-deep datum is *stored* (`ScheduledProduce { application: None, .. }`, no \
      error) and \
      `rholang/src/storage.rs:a_value_at_the_bound_is_stored_and_one_past_it_is_refused_on_both_produce_paths` \
      reports it. **The ordering between the two bounds is structural rather than asserted**: \
      `MAX_VALUE_DEPTH` is written as the smaller of 256 and `crate::parser::MAX_AST_DEPTH`, so a \
      parser bound \
      lowered below 256 lowers it too and the ordering cannot drift. (`min` spelled as an `if`, because it is not const-callable here.) Asserting it instead was tried \
      and refused twice, which is worth recording: a `#[test]` over two constants can never fail, and \
      the `const`-block the linter suggests in its place is a *panic site in production code*, which \
      the type-system gate counts — so the invariant is carried by the definition rather than by a \
      check. **Not closed by this clause, and filed separately**: a *parsed* term of depth \
      257..768 is still an abort route (the evaluator recurses before the guard sees the datum), and \
      nested tuples make the parser exponential — both are C-row findings of this pass, and neither is \
      a depth bound's business" },
  -- ── Progress: the shapes of non-progress (Laws 51) ───────────────────────────────────────────────
  { number := 51, clause := "a", layer := "Progress",
    rustWitness := [
      "casper/src/engine/lfs_block_requester.rs:a_walk_nobody_serves_fails_rather_than_hangs",
      "casper/src/engine/lfs_block_requester.rs:a_slow_but_progressing_walk_is_not_abandoned"],
    statement := "A protocol's failure to make progress has three **shapes**, and each implies a \
      different repair: the requirement is unsatisfiable in **every reachable state** (`Void` — change \
      the requirement), a satisfying state was left and **no rule restores it** (`Terminal` — give the \
      refused state an inverse), or a run's **measure never moves** however long it runs (`Drift` — \
      bound the rate). The vocabulary is parameterised over a transition system, so one set of \
      definitions serves the reducer and the protocol built on it, and each shape is separated from the \
      others by one question: `Void` from `Terminal` is whether the goal was ever satisfiable, \
      `Terminal` from `Drift` is whether anything moved at all. The three axes are machine-checked in \
      `Rchain.Progress.Classification`: **representation** (closed silence, protocol wait, protocol \
      stall), **shape**, and **root cause**, with `Paced` and the two polarities stated as theorems",
    status := .provedModel,
    declarations := [`Rchain.System.Reach, `Rchain.System.Enabled, `Rchain.System.Stuck,
      `Rchain.System.Silent, `Rchain.System.Void, `Rchain.System.Terminal, `Rchain.System.Unrestorable,
      `Rchain.System.Persistent,
      `Rchain.System.Run, `Rchain.System.Drift, `Rchain.System.Paced,
      `Rchain.Progress.Classification.Representation, `Rchain.Progress.Classification.Shape,
      `Rchain.Progress.Classification.RootCause, `Rchain.Progress.Classification.Cell,
      `Rchain.Progress.Classification.Attribution, `Rchain.Progress.Classification.attributions,
      `Rchain.waitSystem, `Rchain.waitGoal,
      `Rchain.System.persistent_iff_unrestorable_complement],
    axioms := [],
    formal := some `Rchain.Progress.Classification.law51_statement,
    mutant := some `Rchain.Progress.Classification.law51_mutant,
    falsifier := some `Rchain.Progress.Classification.law51_falsifier,
    rust := ["casper/src/validate.rs", "block-storage/src/dag/liveness.rs",
      "casper/src/blocks/proposer/proposer.rs", "casper/src/engine/lfs_block_requester.rs"],
    witness := [`Rchain.System.void_goal_is_never_waiting,
      `Rchain.System.unrestorable_lost_is_never_regained, `Rchain.the_two_cycle_drifts,
      `Rchain.a_wait_is_one_step_from_the_goal, `Rchain.a_wait_is_not_terminal,
      `Rchain.System.persistent_iff_unrestorable_complement],
    falsifiable := some "each shape is inhabited, which is what stops the vocabulary being a list of \
      empty names: `Rchain.the_two_cycle_drifts` exhibits a `Drift` on the smallest system that has one \
      (a 2-cycle with a constant measure, so no `Paced` bound holds), and the `Void` and `Terminal` \
      lemmas are *implications* whose hypotheses the instances must prove of real rules — C174's \
      partition filter is `Void` because a validator that never speaks has no message to be seen, and \
      C173's refusal is `Terminal` because the rules have no inverse for it. **Refuted by deleting a \
      hypothesis**: an `Unrestorable` property that a step restores makes \
      `Rchain.System.unrestorable_lost_is_never_regained` false by construction, and a system whose steps \
      all advance the measure makes `Rchain.the_two_cycle_drifts` unprovable — which is exactly the \
      obligation C171's fix is owed (`Paced`). The classification's own mutation is \
      `Rchain.Progress.Classification.law51_falsifier`: a system whose two polarities disagree",
    note := "**What this row is about, and why it is a law rather than a module.** Four findings of \
      2026-09-28/29 were the same property failing and were argued and fixed as one-offs: C174 (a \
      fringe partition no state could satisfy), C173 (a refusal no rule clears), C171 (steps forever, \
      finality unmoved), and the two *causes* below (C170, C172). Nothing in the tree stated the \
      general property, so two readings of one paragraph could disagree about a fix with nothing able \
      to settle it — and one of them proposed shrinking finality's quorum denominator, which the \
      counterexample under law 52's row now refuses. **`Fair` is named and not modelled**, and that is \
      a theorem of this tree rather than a preference: \"an enabled step that no schedule takes\" is \
      unstatable over a transition *relation*, and `Rchain.reduce_not_deterministic` \
      (`Rchain/Concurrent.lean`) is the repo's own proof that the flat calculus fixes no schedule at \
      all — so starvation is registered `open` rather than defined as a shape. **Not modelled either**: \
      the protocol's inactivity leak (a state change that burns a silent validator's *bond*) — and that \
      is a **decision rather than a debt** (2026-10-02, §59): the constraint governing this workstream is \
      that a staker must not lose its bond through no fault of its own, and a partition is the canonical \
      case of exactly that, so the instrument the port has instead is an **income-only** participation \
      rule (law 44) that scales an epoch's reward and cannot reach a stake. The citations this used to \
      carry (#24, #39) are both closed; the leak is not owed by anyone. **`Drift` has a port-side instance \
      now, not only C171's**: the LFS block walk's give-up rule (`MAX_IDLE_ROUNDS`, \
      `casper/src/engine/lfs_block_requester.rs`, §6, issue #102) is a `Paced` condition on a measure \
      the state already kept — `LfsState::finished`, monotone because `done` only adds and `add` \
      refuses a key that is there — and this row's two `rustWitness` entries falsify it in both \
      directions: deleting the give-up leaves the walk drifting, and deleting the reset abandons a \
      slow walk that is still completing blocks. **The classification is a lens over the register, not \
      a new law number**: `Rchain.Progress.Classification.attributions` assigns the progress-family \
      rows a role and a cell, `spec/progress-crosswalk.tsv` emits every entry (defaulting the rest to \
      `independent`), and check 11 refuses a progress law left at that default. The crosswalk does not \
      claim the register is one theorem; most laws are not about stalls at all" },
  { number := 51, clause := "b", layer := "Progress",
    rustWitness := ["casper/src/blocks/proposer/proposer.rs:a_silent_validators_stale_message_does_not_carry_the_quorum"],
    statement := "The two **causes** of a non-progress shape, on the predicate side: a liveness \
      predicate read off a **history** is not reading the current view (`Historic` — C170's guard \
      counted a validator that had *ever* spoken), and two readers of one state can **split** (`Split` \
      — C172's index and store answered different questions about the same block). The causes produce \
      the shapes: the historic predicate is what let the guard's suppression never fire (C171's \
      `Drift`), and the split is what turned an unresolvable justification into a dropped block with \
      nothing to re-queue it (a `Terminal` stall). **Two causes of the cause are named as \
      constructors** in `Rchain.Progress.Classification.RootCause`: `selfReferentialGating` — the \
      guard's predicate is a function of the surface it freezes — and `lossBlindRefusal` — a unit \
      leaves the working set with no re-queue, reset or terminal surface",
    status := .provedModel,
    declarations := [`Rchain.Historic, `Rchain.ReadsTheView, `Rchain.System.Split,
      `Rchain.Progress.Classification.RootCause, `Rchain.Progress.Classification.Attribution],
    axioms := [],
    rust := ["casper/src/blocks/proposer/proposer.rs", "casper/src/block_metadata_store.rs"],
    witness := [`Rchain.historic_refutes_reading_the_view,
      `Rchain.System.split_refutes_agreement],
    falsifiable := some "both are *refutations*, so the falsifier is a predicate that does what the \
      cause says cannot be done: a liveness predicate that reads only the current view refutes \
      `Rchain.Historic` (the window in `block-storage/src/dag/liveness.rs` is such a predicate, \
      and the Rust test named above is its witness), and two readers that cannot disagree refute \
      `Rchain.System.Split` (which is what C172's fix — one order, or one authoritative side — \
      establishes). **The index/store test for the second half lands with the fix itself** (PR #106, \
      `a_failed_store_write_is_not_left_in_the_index`): this row may not name it until the branch it is \
      on carries it, because the witness checker refuses a symbol that is not in the file",
    note := "The two axes are the point, and they are diagnostic rather than taxonomic: the shape says \
      *what* is stuck, the cause says *where the fix goes* — a `Drift` whose cause is `Historic` is \
      fixed in the predicate, not with a rate limiter, and C171 is exactly that case. \
      `docs/src/formal/progress.md` carries the triage table keyed on both columns, and is the page an \
      agent reads before proposing a fix. This clause now also names the **causes of the cause**: a \
      guard that reads the view it freezes is `selfReferentialGating`, and a refusal that drops the \
      unit is `lossBlindRefusal`; the crosswalk's `guard` rows are the laws whose proof reddens when a \
      fix removes one" },
  { number := 51, clause := "c", layer := "Progress",
    rustWitness := [],
    statement := "The vocabulary is **shared with the ρ-calculus rather than parallel to it**: a \
      `System` is a step relation plus a decidable \"is a step available now\" predicate *tied to the \
      relation by the structure's own field*, and the calculus supplies that field with its own \
      `takesStep`/`ReduceP` tie. Silence in the calculus is therefore stuckness, by the tie, and the \
      layers differ in exactly one way: in the closed calculus nothing outside the term can add a send, \
      so a lone receive is silent forever, while at the protocol level delivery is itself a step — \
      which is why a blocked wait and a permanent stall must not share a name",
    status := .provedModel,
    declarations := [`Rchain.silenceSystem, `Rchain.twoCycle],
    axioms := [],
    rust := ["spec/conformance/silence.tsv", "rholang/tests/lean_silence_corpus.rs"],
    witness := [`Rchain.a_silent_term_is_stuck],
    falsifiable := some "`Rchain.a_silent_term_is_stuck` is the checkable half — a term that reports no \
      step has no step — and it is proved *from* the tie, so a `takesStep` that answered `false` where \
      a `ReduceP` step exists would falsify it. The calculus's own verdicts are pinned by the silence \
      corpus (`spec/conformance/silence.tsv`, 13 cases, `spec/Rchain/Silence.lean`'s `silenceCases_decide` and \
      `rholang/tests/lean_silence_corpus.rs`), which is what makes this a claim about the calculus \
      rather than about a definition in a new file",
    note := "The correspondence is documented, not proved as an embedding, and every row of it must say \
      which kind it is — `definitional`, `instance`, `theorem <name>`, or `not modelled, because \
      <reason>` — the register's own `status` discipline applied to a table that is prose. Two rows are \
      honestly `not modelled`: the join rule (`spec/Rchain/Silence.lean` already records that boundary, and \
      a half-filled join is silence there as it is in the node) and the inactivity leak above" },
  { number := 51, clause := "d", layer := "Progress",
    rustWitness := [
      "casper/src/blocks/proposer/proposer.rs:round_escape_permit_is_consumed_even_when_the_round_stays_open",
      "casper/src/blocks/proposer/proposer.rs:late_escape_and_repeated_ticks_do_not_bank_unused_permits"],
    statement := "**A time-gated action consumes its permit.** The proposer's escape from a round \
      that is not closing may fire only after a full `ROUND_STALL_ESCAPE` interval, and firing it \
      re-arms the sighting: a still-open round must wait another full interval. A banked permit — \
      the defect shape, `banked` — licenses every later tick once the first interval elapsed, so the \
      bound stops pacing anything and a fast producer can exhaust the finality-lag bound while its \
      peers are still assembling the next shared justification layer",
    status := .provedTied,
    corpus := some "recovery",
    declarations := [`Rchain.Pacing.Permit, `Rchain.Pacing.take, `Rchain.Pacing.banked,
      `Rchain.Pacing.a_permit_is_consumed, `Rchain.Pacing.the_banked_gate_fires_every_tick,
      `Rchain.Pacing.the_correct_gate_waits_a_full_interval],
    axioms := [],
    rust := ["casper/src/blocks/proposer/proposer.rs"],
    witness := [`Rchain.Pacing.a_permit_is_consumed,
      `Rchain.Pacing.the_banked_gate_fires_every_tick,
      `Rchain.Pacing.the_correct_gate_waits_a_full_interval],
    falsifiable := some "Deleting the sighting reset in `take_round_escape` makes \
      `round_escape_permit_is_consumed_even_when_the_round_stays_open` fire: the second tick at +1 ms \
      escapes. `late_escape_and_repeated_ticks_do_not_bank_unused_permits` pins the late sighting \
      (90 s) and the burst after it. The corpus row `pace 15000 15001` expects no escape and fires \
      under the banked model, while `the_banked_gate_fires_every_tick` exhibits that model's defect \
      as a theorem",
    note := "The code was adopted from candidate `b26bda1a` (branch \
      `aixaria0:verify/quiet-finality-integrated-20261030`): `proposer.rs` is byte-identical between \
      that branch's merge-base and this tree, so only the proposer hunk was taken. The fix is \
      **consensus-neutral** — it gates `AlreadyProposedThisRound`, not block validity, the state \
      hash or the wire — and `ROUND_STALL_ESCAPE` is unchanged at 15 s. The model's `bound` is 15 000 \
      ms; the corpus (`spec/conformance/recovery.tsv`) is the tie to `take_escape_permit`. The layer \
      is named `recovery` because the decline of an escape storm is a recovery property; later \
      clauses (the receiver re-offer and the fault classification) extend it." },
  { number := 52, clause := "a", layer := "Casper",
    rustWitness := [
      "casper/src/blocks/proposer/proposer.rs:the_quorum_is_measured_against_the_whole_bonded_map_not_the_live_one",
      "block-storage/src/dag/finalizer.rs:the_support_refusal_names_the_not_full_sender"],
    statement := "**Finality's quorum denominator is the whole bonded stake, and only the partition may \
      shrink.** Two disjoint groups cannot each hold a strict supermajority of one total \
      (`supermajorities_overlap`: `3·s₁ > 2·t` and `3·s₂ > 2·t` force `t < s₁ + s₂`), so a denominator \
      taken from the *live* set — the fix this repository's own plan proposed on 2026-09-29, *so a \
      silent validator leaves numerator and denominator together* — lets each side of a partition \
      finalise its own history. Shrinking the **partition** is what restores liveness, because a \
      validator that has stopped producing messages stops being required to have seen the cut",
    status := .provedModel,
    formal := some `Rchain.finalityDenominatorLaw,
    mutant := some `Rchain.staleJoinDenominator,
    falsifier := some `Rchain.staleJoinDenominator_is_not_safe,
    declarations := [`Rchain.supermajorities_overlap, `Rchain.bonds4, `Rchain.bondsAB, `Rchain.bondsCD,
      `Rchain.suppOf, `Rchain.Participation, `Rchain.Delivery, `Rchain.StalenessBound,
      `Rchain.FinalityDecision, `Rchain.finalitySafe,
      `Rchain.a_safe_advance_has_a_supermajority, `Rchain.finality_safe_denominator,
      `Rchain.finality_safe_partition, `Rchain.finality_decision_is_total,
      `Rchain.NoAdvance, `Rchain.noAdvance, `Rchain.gateDecision,
      `Rchain.the_support_is_the_full_partition_stake,
      `Rchain.the_gate_denominator_is_the_active_snapshot, `Rchain.the_gate_decision_is_safe,
      `Rchain.a_refusal_carries_the_gates_numbers, `Rchain.a_refusal_is_not_a_supermajority,
      `Rchain.the_join_layer_is_exactly_two_thirds, `Rchain.the_join_layer_refuses,
      `Rchain.the_join_layer_names_sender_one, `Rchain.the_join_layer_advances_once_all_see,
      `Rchain.finalityDenominatorLaw, `Rchain.finalityDenominatorLaw_holds,
      `Rchain.staleJoinDenominator, `Rchain.staleJoinDenominator_is_not_safe,
      `Rchain.the_live_denominator_is_the_unsafe_witness],
    axioms := [],
    rust := ["block-storage/src/dag/liveness.rs", "block-storage/src/dag/finalizer.rs",
      "casper/src/blocks/proposer/proposer.rs", "casper/src/multi_parent_casper.rs"],
    witness := [`Rchain.supermajorities_overlap, `Rchain.the_live_denominator_lets_two_sides_finalise,
      `Rchain.the_quorum_is_the_whole_bonded_map, `Rchain.a_safe_advance_has_a_supermajority,
      `Rchain.finality_safe_denominator, `Rchain.finality_decision_is_total,
      `Rchain.the_join_layer_is_exactly_two_thirds, `Rchain.the_join_layer_refuses,
      `Rchain.the_join_layer_names_sender_one, `Rchain.the_join_layer_advances_once_all_see,
      `Rchain.a_refusal_is_not_a_supermajority, `Rchain.staleJoinDenominator_is_not_safe,
      `Rchain.the_live_denominator_is_the_unsafe_witness],
    falsifiable := some "the arithmetic is general and the counterexample is its instance on one fixture, \
      and the instance is **two-sided on purpose**: `Rchain.the_live_denominator_lets_two_sides_finalise` \
      shows the two disjoint sides each advancing at their own denominator (200 of 200) and both being \
      refused at the bonded one (200 of 400), while `Rchain.the_quorum_is_the_whole_bonded_map` shows the \
      *full* partition advancing there (300 of 400) — so the second cannot be read as *the gate never \
      fires*. A change that made the denominator the live set makes the second conjunct of the first \
      theorem false, which is the disagreement this week settled by an instance. **What it does not \
      claim**: that two conflicting *messages* cannot be supported — the step from *the supporting sets \
      overlap* to *the chain cannot fork* is the DAG's causality, which is Law 15's \
      (`Rchain.seen_monotone_of_reaches`) and is a named hook here rather than a theorem",
    note := "**This is the row the increment-2 plan needed and the tree did not have.** The plan handed \
      the live weight set to `calculate_finalization` as the bonds map — both questions of one map — and \
      the argument for it was that a silent validator leaves numerator and denominator together. It does, \
      and that is the defect: over the live set the gate is `3·F > 2·L` with `F ≤ L`, which holds for \
      *any* self-consistent subset, so under a network partition each side finalises its own view. The \
      shipped fix is asymmetric for this reason (partition = the live set, quorum = the whole bonded map, \
      registered in `spec/audit/passes.md` §6, AUDIT C174), and the phrasing of the requirement that this \
      law pins is the issue's own title: finality needs quorum *stake*, not live nodes. **The hypotheses \
      are named in the instance module** (`Rchain.Participation`, `Rchain.Delivery`, \
      `Rchain.StalenessBound`) rather than left implicit, because a liveness claim without its hypothesis \
      is Law 20's trap: `law20_deadlock_freedom` was an axiom until it was deleted as unprovable as \
      stated" },
  { number := 52, clause := "b", layer := "Casper",
    rustWitness := [
      "casper/tests/finalization.rs:a_silent_bonded_validator_does_not_cap_the_fringe",
      "block-storage/src/dag/liveness.rs:the_window_is_heights_behind_the_tip",
      "block-storage/src/dag/finalizer.rs:a_seer_outside_the_live_partition_does_not_void_a_candidate",
      "casper/src/multi_parent_casper.rs:a_finality_trace_names_the_not_full_sender"],
    statement := "**The requirement must be one a reachable state can satisfy.** With the whole bonded set \
      as the *partition*, a validator that produces no message makes the full-partition filter \
      unsatisfiable — not slow, **empty**: `allBonded` demands that every seer's seen set contain the \
      bonded set, and no seer can have seen a validator that never spoke. That is C174's measured shape, \
      where the survivors at 80 % of the stake could not lift the gate at all, and it is Law 51's `Void` \
      with its proof obligation discharged here: the emptiness is *derived from the predicate* rather \
      than observed on a fixture. **The filter is containment, not equality** (#213): a seer outside the \
      live partition — a stopped validator that last spoke after the candidate — does not void a \
      candidate the whole partition has seen (`Rchain.a_seer_outside_the_partition_does_not_void_a_candidate`)",
    status := .provedModel,
    formal := some `Rchain.attributableRefusalLaw,
    mutant := some `Rchain.silentRefusal,
    falsifier := some `Rchain.silentRefusal_is_not_a_refinement,
    declarations := [`Rchain.allBonded_false_of_a_silent_seer,
      `Rchain.allBonded_false_of_seers_omitting,
      `Rchain.fullPartitionStake_eq_zero_of_a_silent_bonded,
      `Rchain.a_seer_outside_the_partition_does_not_void_a_candidate,
      `Rchain.a_safe_refusal_names_its_reason, `Rchain.a_refusal_always_names_its_coordinates,
      `Rchain.a_refusal_names_the_not_full_senders, `Rchain.FinalityTrace, `Rchain.RefusalKind,
      `Rchain.traceRefines, `Rchain.a_refusal_trace_carries_its_reason,
      `Rchain.a_reasonless_refusal_is_not_a_refinement, `Rchain.attributableRefusalLaw,
      `Rchain.attributableRefusalLaw_holds, `Rchain.silentRefusal,
      `Rchain.silentRefusal_is_not_a_refinement],
    axioms := [],
    rust := ["block-storage/src/dag/liveness.rs", "block-storage/src/dag/finalizer.rs",
      "casper/tests/finalization.rs", "casper/src/multi_parent_casper.rs"],
    witness := [`Rchain.fullPartitionStake_eq_zero_of_a_silent_bonded,
      `Rchain.the_whole_bonded_partition_is_unsatisfiable,
      `Rchain.a_safe_refusal_names_its_reason,
      `Rchain.a_refusal_always_names_its_coordinates,
      `Rchain.a_reasonless_refusal_is_not_a_refinement,
      `Rchain.silentRefusal_is_not_a_refinement],
    falsifiable := some "the general theorem is an induction over the support map whose step is \
      `Rchain.allBonded_false_of_a_silent_seer` — a seer whose seen set omits a bonded validator cannot \
      contain the bonded set — so a filter that kept such a candidate would falsify it. Its fixture instance \
      is **two-sided**: `Rchain.the_whole_bonded_partition_is_unsatisfiable` has the gate refusing three \
      speaking validators' candidates at 300 of 400 *and* advancing the identical fixture once the fourth \
      speaks, so the first half is a finding rather than a definition. Delete the hypothesis that a \
      validator is silent and the theorem is unprovable, which is the honest shape: what the fix changed \
      was the *hypothesis* (the partition is `Rchain.Participation`'s live subset), not the arithmetic",
    note := "The two clauses of this law are the two halves of the increment that landed on 2026-09-29, \
      and they are separable: clause a is safety (the denominator) and clause b is liveness (the \
      partition). C174's measurement is what forced the second — the survivors at 80 % could not resume \
      finality while the third validator was stopped, and #105's run froze with the survivor at 91 %, \
      where the binding constraint was a *message* and not a stake share. **What clause b still does not \
      fix, said plainly**: a net that stays below two thirds permanently (three equal validators minus \
      one is exactly two thirds) still cannot finalise. **And that is a decision rather than a debt** \
      (2026-10-02, §59): the instrument that would price it is an inactivity leak, a state change that \
      burns a silent validator's *bond*, and the constraint governing this whole workstream is that a \
      staker must not lose its bond through no fault of its own — a partition is the canonical case of \
      exactly that. What the port has instead is an **income-only** participation rule (law 44), which \
      scales a validator's epoch reward and cannot reach a stake. The citations this used to carry (#24, \
      #39) are both closed, and the leak is not owed by anyone" },
  { number := 53, clause := "a", layer := "Casper",
    rustWitness := [
      "casper/src/validate.rs:neglected_invalid_block_detects_bonded_invalid_justification",
      "casper/src/dag.rs:h1b_a_justified_bonded_failed_block_is_refused_rather_than_forced"],
    statement := "**One attribution is terminal: the refusal persists and the rule refuses its children.** \
      A node that marks a bonded validator's block failed never follows that validator's chain again, \
      because (i) the record persists — `mark_failed` records the metadata of every \
      `ValidateError::ValidationFailed` and no rule clears it — and (ii) `neglected_invalid_block` refuses \
      any block justifying a failed **bonded** sender's block, before any other rule runs (and, \
      independently, `block_number`'s maximum skips failed justifications, so a child numbered above the \
      last non-failed height is refused too). Together they are `Rchain.System.persistent_blocks_the_goal`: \
      from a state holding the refusal, **no** reachable state admits a block above it. The node is \
      `Terminal`, not slow",
    status := .provedModel,
    declarations := [`Rchain.Strand, `Rchain.strandStep, `Rchain.strandSystem, `Rchain.neglects],
    axioms := [],
    rust := ["casper/src/validate.rs", "casper/src/dag.rs", "casper/src/multi_parent_casper.rs"],
    witness := [`Rchain.the_refusal_is_persistent, `Rchain.a_neglected_block_is_detected,
      `Rchain.a_refused_validator_is_never_followed],
    falsifiable := some "**the falsifier is a rule that restores, and the port now has one.** \
      `Rchain.the_refusal_is_persistent` is proved from the modelled step relation, so a step that \
      unmarked a sender — a revalidation of failed metadata, or a bounded re-fetch — makes it false *by \
      construction*. That is exactly what happened: clause **b** models the port's restoring rule as \
      `Rchain.restoreStep`, and `Rchain.the_refusal_is_not_persistent_once_a_rule_restores` **is this \
      cell executed** — the red the guard was stated to produce, read off the elaborated environment \
      rather than asserted. This clause is therefore stated over the rule set *without* the restoring \
      rule, and is the guard that fired. The rule half is non-vacuous on the model's own fixture \
      (`Rchain.a_neglected_block_is_detected`: a block justifying a failed bonded sender's block is \
      neglected, one justifying a failed *unbonded* sender's is not) — which is also why option (a) of \
      C173's decision, counting failed justifications in `block_number`'s maximum, ships as well: the \
      unbonded route is the one `neglected_invalid_block` cannot close",
    note := "This is C173, filed as #105 from a live-testnet run on 2026-09-29: a two-validator chain \
      froze with the survivor holding 91 % of the stake, and the reason was not the quorum — the \
      survivor had marked the other's block failed, so it refused every block above it for good. **Both \
      rules are the oracle's**, which is why the law was a *decision* rather than a bug fix: `block_number` \
      skipped failed justifications (the Scala's `if (!m.validationFailed)`) and `neglectedInvalidBlock` \
      is a straight port. **What was not upstream is the consequence**: the failure is attributed \
      (reached by every `ValidateError::ValidationFailed`), so the block was recorded failed *and* \
      slashable, and the one-block-per-node divergence became a deterministic estrangement with slash \
      evidence attached. **The decision was taken on 2026-10-01 and both halves shipped** — clause b's \
      restoring rule, and option (a), which is now a registered §6 divergence from the Scala (the \
      maximum counts failed justifications). **Independently of C172's fix**, which is a dropped \
      `Internal` rather than a recorded `ValidationFailed` and does not touch this path" },
  { number := 53, clause := "b", layer := "Casper",
    rustWitness := [
      "casper/src/multi_parent_casper.rs:only_a_divergence_is_restorable",
      "casper/tests/restoring_rule.rs:only_a_divergence_is_revalidated",
      "casper/tests/restoring_rule.rs:one_block_budget_bounds_the_revalidations",
      "casper/tests/restoring_rule.rs:a_record_at_the_attempt_limit_is_not_revalidated_again",
      "casper/src/merging.rs:a_spend_whose_funding_chain_lost_is_rejected_rather_than_halting_the_merge",
      "casper/src/merging.rs:a_system_deploy_in_the_dependent_closure_is_not_dropped"],
    statement := "**A refusal has an inverse, and the inverse is bounded.** Clause a's `Terminal` is \
      repaired by a rule that *unmarks*: a node re-validates a failed record against its current view, \
      and a record that now passes is cleared — so the block above it is admitted rather than refused. \
      This is `restore_divergent_justifications` (`casper/src/multi_parent_casper.rs`), and it is \
      `Rchain.restoreStep`, the exact inverse of `strandStep`. **What makes it a rule and not a retry \
      loop is that its work is bounded by three quantities the protocol bounds**: it is keyed on the \
      *cause* (only a view-dependent `Divergence` is eligible — an `Attributable` failure is the \
      block's own fault and is permanent, a `Cascade` is not about that block at all); it is capped per \
      record by a count that is *persisted*, so a restart does not refresh the budget; and it is \
      budgeted per incoming block, so the work one block can provoke is a constant times the cost the \
      protocol already pays to validate it. Clearing a record keeps `slashable` exactly as it was — the \
      refusal has an inverse, attribution does not. **The same shape holds at the merge boundary**: a \
      cost composition that goes negative used to fail the merge as a whole, so every proposal on that \
      view failed and a restart did not clear it. The inverse names the offending chain and its \
      dependent closure, drops them, and recomposes; when nothing is removable the merge still fails, \
      which is the honest `Terminal` rather than a silent success",
    status := .provedModel,
    declarations := [`Rchain.Strand, `Rchain.restoreStep, `Rchain.strandSystemRestoring,
      `Rchain.MergeRepair.Spend, `Rchain.MergeRepair.repairStep, `Rchain.MergeRepair.removable,
      `Rchain.MergeRepair.dropHosts, `Rchain.MergeRepair.funded, `Rchain.MergeRepair.withSystem,
      `Rchain.MergeRepair.systemOnly, `Rchain.MergeRepair.law53b_statement],
    axioms := [],
    formal := some `Rchain.MergeRepair.law53b_statement,
    mutant := some `Rchain.MergeRepair.law53b_mutant,
    falsifier := some `Rchain.MergeRepair.law53b_falsifier,
    rust := ["casper/src/multi_parent_casper.rs", "casper/tests/restoring_rule.rs",
      "casper/src/merging.rs"],
    witness := [`Rchain.the_refusal_is_not_persistent_once_a_rule_restores,
      `Rchain.a_restoring_step_removes_a_refusal, `Rchain.MergeRepair.law53b,
      `Rchain.MergeRepair.a_repair_reports_every_drop,
      `Rchain.MergeRepair.a_system_spend_survives,
      `Rchain.MergeRepair.a_repair_never_silently_succeeds],
    falsifiable := some "**the falsifier is the absence of the step**: delete `restoreStep` from \
      `strandSystemRestoring`'s relation and the reachable set is clause a's again, so \
      `the_refusal_is_not_persistent_once_a_rule_restores` cannot be proved — persistence returns. On \
      the Rust side it is the three bounds: `only_a_divergence_is_restorable` (the cause key), \
      `a_record_at_the_attempt_limit_is_not_revalidated_again` (the cap, and that it is spent), and \
      `one_block_budget_bounds_the_revalidations` (the budget), each observed red by deleting the clause \
      it names — which is what keeps this out of `candidate:bounded-work-per-step`'s class (C180): work \
      whose cost grows with an input nothing bounds. **What the clause does not claim**: it does not \
      promise the inverse always fires — a node whose own view is still divergent re-validates to the \
      same refusal, and is right to keep refusing a block it cannot verify. The rule gives the refused \
      state an inverse, which is what `Terminal` demands; the refutation theorem is a statement about \
      the relation, not a liveness promise about any particular run. At the merge boundary the mutant \
      is `Rchain.MergeRepair.law53b_falsifier`: the old whole-merge failure on the measured instance, \
      refuted by the repair that names and drops the unfundable chain",
    note := "C173's fix, #125, landed 2026-10-01. The name is deliberately a *clause* rather than a \
      new number: it is not a new shape but the other half of clause a's — `Persistent` and its dual \
      (`Rchain.System.Unrestorable`) are what Law 51's vocabulary exists to tell apart, and a \
      refusal that its own fault cannot clear is exactly the case where the two must be stated \
      together. The three Rust bounds are the ones the register's C180 class asks for, because a \
      restoring rule that re-validated on every child would be an unbounded re-fetch triggered by an \
      input nothing bounds. **The merge-boundary half is a second instance, not a new law number**: \
      `Rchain.MergeRepair` proves that a repair reports every drop, that a system deploy is not \
      removable, and that a merge with nothing removable fails rather than succeeding with a silent \
      loss; the conditional success is deliberate, because a surviving set that cannot compose must \
      fail" },
  { number := 53, clause := "c", layer := "Casper",
    rustWitness := [
      "casper/src/multi_parent_casper.rs:a_spent_budget_rewinds_only_where_a_height_can_be_named",
      "casper/src/dag.rs:the_reset_rewinds_the_dag_and_removes_the_bodies_it_dropped",
      "casper/src/interpreter_util.rs:a_reset_request_is_raised_once_taken_once_and_wakes_the_driver",
      "casper/tests/reset.rs:a_reset_request_rewinds_the_stores_when_the_switch_is_on_and_does_nothing_when_it_is_off"],
    statement := "**A refusal has a second inverse, and it is not an unmark.** Clause b clears a record: \
      the node re-validates a failed block against its current view and unmarks it if it now passes. The \
      port has since grown a different rule (#320, F-U5-01's acting half): a node whose revalidation \
      budget is spent **rewinds to its last finalised block** and re-syncs from there, and the records \
      above the anchor go with the state they were about. On this model the two are different steps and \
      the difference is the point — `restoreStep` removes one element from the head, while `rewindStep` \
      drops a *prefix*, leaving a suffix. A rewind that dropped nothing, or from the other end, would be \
      a different rule about a different mechanism. **And the step is guarded, because the port refuses \
      it in two of its three states**: `rewind_plan` answers with an anchor *and* its height (the arm \
      that acts), with an anchor whose height the view cannot place, or with nothing at all — and only \
      the first can rewind, because `drop_above` takes a height and a reset without one would have to \
      replay from genesis. `Rchain.a_rewind_is_not_available_without_an_anchor` is that guard as the \
      theorem it is; without it this clause would certify a relief the port withholds",
    status := .provedModel,
    declarations := [`Rchain.Strand, `Rchain.rewindStep, `Rchain.strandSystemRewinding],
    axioms := [],
    rust := ["casper/src/engine/reset.rs", "casper/src/engine/node_launch.rs",
      "casper/src/multi_parent_casper.rs"],
    witness := [`Rchain.the_refusal_is_not_persistent_once_a_rule_rewinds,
      `Rchain.a_rewinding_step_removes_a_refusal, `Rchain.a_rewind_is_not_available_without_an_anchor],
    falsifiable := some "**the falsifier is the absence of the step, and the guard is the second one.** \
      Delete `rewindStep` from `strandSystemRewinding`'s relation and the reachable set is clause a's \
      again, so `the_refusal_is_not_persistent_once_a_rule_rewinds` cannot be proved — persistence \
      returns. On the Rust side the three witnesses are the reset's own decisions rather than its \
      machinery: the decision rewinds only where a height can be named (the middle and no-anchor arms \
      refuse), the durable half leaves the DAG at the anchor with the dropped bodies gone, and the \
      request is raised once, taken once and wakes the driver. **What the clause does not claim**: it is \
      not a liveness promise about any particular run — a node that rewinds and cannot reach a peer \
      again is in C259's state, which is the net-wide reconciliation's subject and not this clause's",
    note := "#320, register C274, landed 2026-10-11. Modelled as a clause rather than a number for the \
      same reason clause b is: it is the *other* repair of clause a's `Terminal`, and a reader must be \
      able to tell the two apart — `Persistent` versus `Unrestorable` is the distinction Law 51's \
      vocabulary exists for, and \"the refusal was cleared\" and \"the state it was about was discarded\" \
      are different answers to the same terminal state. **The worksheet had this wrong**: \
      `failure-hazop.md` credited R3 with reddening `the_refusal_is_persistent`, and that reddening came \
      from clause b's restoring rule (#125), not from #320 — a rewind is not an unmark, and reddening the \
      persistence theorem with one would be a false falsifier, asserting a rule the port does not have \
      for that hash. The corrected cell is F-U5-02's" },
  { number := 54, clause := "a", layer := "Casper",
    rustWitness := [
      "casper/src/blocks/proposer/proposer.rs:a_silent_validators_stale_message_does_not_carry_the_quorum",
      "block-storage/src/dag/liveness.rs:a_validator_with_no_message_at_all_is_not_live"],
    statement := "**A liveness predicate reads the current view, not a history.** A predicate that searches \
      a retention map — the port's `latest_msgs`, which keeps a silent sender's last message indefinitely \
      — has a verdict that depends on what has *ever* happened, so it is `Historic` (Law 51's cause): two \
      histories that agree on the current view and differ only in their past get different answers from it. \
      The predicate that replaces it takes the sender's **entry** and tests it against the window, which is \
      what makes the same three-validator fixture read 200 rather than 300 — the false supermajority C170 \
      measured",
    status := .provedModel,
    declarations := [`Rchain.Retention, `Rchain.everSpoke, `Rchain.inWindow, `Rchain.Historic,
      `Rchain.ReadsTheView],
    axioms := [],
    rust := ["block-storage/src/dag/liveness.rs", "casper/src/blocks/proposer/proposer.rs",
      "casper/src/multi_parent_casper.rs"],
    witness := [`Rchain.everSpoke_is_historic, `Rchain.the_retention_reader_counts_a_stale_sender,
      `Rchain.historic_refutes_reading_the_view],
    falsifiable := some "both halves are refutations of the other reading: `Rchain.everSpoke_is_historic` \
      exhibits two maps that agree on the current view and disagree on `everSpoke`, so a predicate with \
      that shape can never be `Rchain.ReadsTheView` (which `Rchain.historic_refutes_reading_the_view` \
      proves as the general complement); and `Rchain.the_retention_reader_counts_a_stale_sender` carries \
      the port's own numbers into the model — sender 3, seven heights behind the tip, counted by the \
      retention reader and refused by the window. The Rust test named above fails with `left: 200, \
      right: 100`, which is the same disagreement in stake rather than in verdict, so the model and the \
      code are pinned to the same fixture",
    note := "**This is the cause rather than the shape**, and the distinction is what makes the      classification diagnostic: C170's predicate is why C171's storm ran (the guard's suppression never \
      fired while the quorum *was* reachable) and why the arithmetic looked wrong when the defect was in \
      what the guard was reading. The fix has two halves and only one of them is arithmetic: the predicate \
      now reads the entry the view supplies (`liveness::live_weight_set`), and the *window* makes \
      \"current\" a bounded claim rather than an appeal to history — so a predicate that reads a view \
      still needs `Rchain.StalenessBound` to be a liveness predicate at all. **What it does not claim**: \
      that every predicate over a map is historic (a map with exactly one entry per sender is its own \
      view, which is why the port quotients to the newest per sender); the cause is the *shape of the \
      read*, and `Rchain.everSpoke` is the shape" },
  { number := 54, clause := "b", layer := "Casper",
    rustWitness := [],
    statement := "**One view, or both written together.** When a node keeps two views of the same set — the \
      port's in-memory DAG index and the persisted store it is built from — and updates them by \
      **separate** steps, a state is reachable in which one holds a key the other does not, and the two \
      readers that disagree there are exactly the ones whose answers decide validation (`has_all_deps` \
      asks the index; `block_summary` asks the store). The fix's shape is one step writing both: \
      `the_atomic_order_cannot_split` proves the invariant for every reachable state from a consistent \
      start, and `the_separate_steps_can_split` exhibits the state where the invariant fails",
    status := .provedModel,
    declarations := [`Rchain.TwoViews, `Rchain.viewsStep, `Rchain.atomicStep],
    axioms := [],
    rust := ["casper/src/block_metadata_store.rs"],
    witness := [`Rchain.the_separate_steps_can_split, `Rchain.the_atomic_order_cannot_split],
    falsifiable := some "**the falsifier is a rule, not a schedule**: `Rchain.the_atomic_order_cannot_split` \
      is an induction whose step rewrites both views, so a rule that updates one side alone — which is the \
      port's own pre-fix order — makes it unprovable by construction, and \
      `Rchain.the_separate_steps_can_split` is that rule's reachable witness. The initial condition is a \
      hypothesis rather than a fact, and it is the honest one: a node whose two views start out of step \
      stays out of step, which is why the fix states *which* side a reader should ask as well as ordering \
      the writes. The Rust side of this clause is AUDIT C172's fix and its two tests \
      (`a_refused_height_gap_is_not_left_in_the_index`, \
      `a_failed_store_write_is_not_left_in_the_index`, PR #106): the row may not name them until the branch \
      carrying them is the base, because the witness checker refuses a symbol that is not in the file",
    note := "The second cause, and the one that shows why the two axes are one axis: C172 is not a \
      progress defect at all — nothing is stuck, two readers simply answer different questions about the \
      same block — and it *produces* a `Terminal` stall, because the reader that disagrees feeds \
      validation, the failure is attributed, and Law 53's refusal then makes the stall permanent. The \
      diagnosis is what links them: a rule read a view that was not the authoritative one. **Independent \
      of clause a**: there the fix is to read the view, here it is to have one" },
  { number := 55, clause := "a", layer := "Casper",
    rustWitness := [
      "sdk/src/dag/merging.rs:a_budget_refuses_without_answering_and_never_changes_the_answer",
      "sdk/tests/merging_scaling.rs:rejection_options_are_bounded_on_a_directed_shape"],
    statement := "**The guard is before the work.** Every step the protocol takes has a cost bounded by a \
      quantity the protocol bounds, and the bound is a **hypothesis of the step** rather than a check \
      after it: `spendStep` charges one unit only while `spent < limit`, so `the_work_never_exceeds_the_ \
      budget` holds in every reachable state and `an_exhausted_budget_enables_no_step` says there is \
      nothing to take once the budget is gone. **The defect this is the inverse of has a shape, not just \
      a symptom**, and it is in the same model: `lateGuardStep` charges the step and *then* asks whether \
      it was allowed, and `the_late_guard_overspends` exhibits a state one unit past the budget reachable \
      in a single step from an exhausted start. That the two relations differ is what makes this clause a \
      claim rather than a definition — every row of the class AUDIT C180 names had the late guard",
    status := .provedModel,
    declarations := [`Rchain.Work, `Rchain.spendStep, `Rchain.workSystem, `Rchain.lateGuardStep,
      `Rchain.lateGuardSystem],
    axioms := [],
    rust := ["sdk/src/dag/merging.rs", "casper/src/merging.rs",
      "casper/src/blocks/block_receiver.rs", "casper/src/blocks/block_processor.rs"],
    witness := [`Rchain.the_work_never_exceeds_the_budget, `Rchain.an_exhausted_budget_enables_no_step,
      `Rchain.the_late_guard_overspends],
    falsifiable := some "**the falsifier is a relation, not a schedule**: `Rchain.the_late_guard_overspends` \
      derives an over-budget state from a step relation that carries no guard, so a port whose bound sat \
      after the work would make `Rchain.the_work_never_exceeds_the_budget` unprovable by construction — \
      the budget would be a counter, not a bound. On the Rust side the pair is \
      `a_budget_refuses_without_answering_and_never_changes_the_answer` (the refusal fires) and \
      `rejection_options_are_bounded_on_a_directed_shape` (the work counting toward the budget is bounded \
      by the **output** rather than by the width — observed red with the quotient disabled: 16,383 states \
      for one option at fourteen keys, where the bounded search expands fourteen)",
    note := "**This is the class statement AUDIT C180 asked for, and C178 is its first held member.** \
      C180 was filed as `candidate:bounded-work-per-step` with three symptoms — the merge search \
      (C178), the unbounded ingress queue (C175) and the start-up envelope (#68) — because the register \
      had no law that named what they shared, which is how one cause came to be tracked as three. The \
      `rust` cell is the merge search and the ingress path, and the two bounds landed on 2026-10-01 \
      (#147, #151); the third member is #68's envelope, and C189 records that its measurement no longer \
      reproduces on the current build, which is a fact about the rig rather than about this clause. \
      **What the clause does not claim**: that the cost is *small*, only that it is bounded by something \
      the protocol bounds. Raising the cost of an attack is not the same as removing it, and the \
      honest sentence every row of this class carries is that below the bound the work may still be \
      exponential" },
  { number := 55, clause := "b", layer := "Casper",
    rustWitness := [
      "sdk/src/dag/merging.rs:a_budget_refuses_without_answering_and_never_changes_the_answer"],
    statement := "**The bound cannot change an answer.** A run whose budget suffices returns what the work \
      produced, and **every larger budget returns the same thing** — \
      `a_larger_budget_does_not_change_an_answer` — so two nodes that both complete compute the identical \
      answer however different their budgets are. A spent budget carries **nothing** \
      (`a_spent_budget_carries_no_answer`), which is the port's shape rather than an analogy: \
      `SearchBudgetExceeded` carries two counters and deliberately no option set, because a truncated \
      option set would pick a different rejection. Together the two make the bound a **resource policy \
      and not a consensus change** — which is what Law 17's determinism requires of anything that can \
      stop a search early, and what the class's rows could not state while the law was missing",
    status := .provedModel,
    declarations := [`Rchain.boundedRun],
    axioms := [],
    rust := ["sdk/src/dag/merging.rs", "casper/src/multi_parent_casper.rs"],
    witness := [`Rchain.a_spent_budget_carries_no_answer,
      `Rchain.a_larger_budget_does_not_change_an_answer,
      `Rchain.a_completion_within_the_budget_is_returned],
    falsifiable := some "**the falsifier is a refusal that carries a partial answer**, and it is the shape \
      the port's type refuses: `a_spent_budget_carries_no_answer` is the empty case, so a `Refused` \
      constructor holding a truncated accumulation would have no model here — which is why \
      `SearchBudgetExceeded` has fields `steps` and `options` and no set. \
      `a_completion_within_the_budget_is_returned` is the non-vacuity witness: the clause is about a run \
      that *does* answer, not about one that never does. The Rust side is \
      `a_budget_refuses_without_answering_and_never_changes_the_answer`, which asserts both halves on the \
      port's own fixtures: an exceeded search returns an `Err` with no option set, and a budget that is \
      not hit returns the identical set the unbounded search returns",
    note := "**Why the clause is stated over a schedule and not over a counter.** The claim is \
      *all-or-nothing*, and in the port that is a fact about a type: the refusal has no field to put a \
      partial answer in, so a caller cannot mistake a truncated search for a result. A model that \
      carried a counter and an accumulator would make the property a policy the code documents rather \
      than a thing the code cannot express — which is the difference between a law and a comment, and \
      the reason Law 51's vocabulary exists one level up. **What it does not claim**: that any \
      particular budget is safe. `SearchBudget::NODE` is a provisional number (C184) and this clause \
      says only that whatever it is, two nodes that finish with it agree" },
  { number := 56, clause := "a", layer := "Rholang",
    statement := "**A name is rooted in the identity that owns it.** `publish(path, value)` is \
      accepted iff the path's owner prefix equals the caller's **derived** REV address — derived \
      from the deployer id (`rho:rev:address(\"fromDeployerId\", …)`), never supplied by the caller — \
      so a write outside your own root is *inexpressible* rather than refused, and a caller with no \
      derivable identity is refused before any shared state is touched. Versions are append-only and \
      `seal` closes a path; the mutable pointer lives only in the **alias tier**, which one authority \
      governs.",
    status := .provedModel,
    declarations := [`Rchain.NsPath, `Rchain.NsState, `Rchain.Verdict, `Rchain.publish,
      `Rchain.setAlias, `Rchain.writekeyValid, `Rchain.unguardedPublish],
    axioms := [],
    rust := ["casper/src/genesis/resources/rgov/MasterDictionary.rho",
      "casper/src/genesis/rgov.rs", "casper/src/genesis/mod.rs"],
    rustWitness := [
      "casper/tests/master_dictionary.rs:the_four_must_fail_cases",
      "casper/tests/master_dictionary.rs:publish_is_append_only_and_versions_are_pinned",
      "casper/tests/master_dictionary.rs:seal_closes_a_path_and_the_key_still_resolves",
      "casper/tests/master_dictionary.rs:a_granted_key_works_until_it_is_revoked",
      "casper/tests/genesis_registry.rs:only_the_ceremony_key_can_reach_the_admin_handle"],
    corpus := some "namespace",
    witness := [`Rchain.a_stranger_publishing_under_alices_root_is_refused,
      `Rchain.alice_publishing_under_her_own_root_is_accepted,
      `Rchain.a_forged_identity_is_refused,
      `Rchain.an_unguarded_publish_hits_anothers_root,
      `Rchain.a_sealed_path_refuses_a_further_version_and_keeps_its_versions,
      `Rchain.a_non_root_may_not_set_a_short_name,
      `Rchain.a_revoked_writekey_is_refused],
    falsifiable := some "**the falsifier is the same function with the ownership test removed.** \
      `unguardedPublish` differs from `publish` in exactly that one comparison, and \
      `an_unguarded_publish_hits_anothers_root` proves that the inputs \
      `a_stranger_publishing_under_alices_root_is_refused` refuses are *accepted* by it — so the \
      refusal is a claim about the guard rather than a restatement of the code. The controls keep the \
      other three clauses honest in the same way: `alice_publishing_under_her_own_root_is_accepted` \
      and `the_root_may_set_a_short_name` show the refusals are not a `publish` that refuses \
      everything, and `a_sealed_path_refuses_a_further_version_and_keeps_its_versions` carries both \
      halves (the refusal *and* the surviving version), because a seal that also destroyed the log \
      would satisfy the first alone. The Rust side is `casper/tests/master_dictionary.rs`, which runs \
      the four must-fail cases against the **deployed** dictionary — writing under another's root, a \
      granted key after `revoke`, publishing to a sealed path, a non-root setting a short name — and \
      `rgov.rs::the_deployer_id_is_only_ever_the_address_derivation`, which asserts the disclosure \
      rule of SECURITY.md over the contract's source",
    note := "**This is the design #71's thread converged on, and it is why #71 was only half closed.** \
      #71's stopgap published a `grant` capability, which answers \"who may write\" by *grant*; it does \
      not answer who may claim a name at block 0, whether a publish overwrites or extends, or whether \
      the bundled classes belong in genesis. The rooted design answers the first by *derivation* — the \
      owner prefix comes from the caller's identity, so there is nothing to claim and no admission \
      policy to freeze — and this row's clauses b and c answer the other two: `publish` appends rather \
      than overwrites, and the alias tier is the one governed pointer. **What it does not claim**: \
      anything about *encoding*. The model is over abstract tokens because the law is about which \
      comparisons decide a write; the base58 form, and the fact that a forged id derives `Nil` rather \
      than a different address, are properties of `rho:rev:address` and are pinned by \
      `casper/tests/master_dictionary.rs`, not here. Nor does it claim that a contract cannot *disclose* \
      a deployer id it was given: that is a discipline over the chain's contracts, not a mechanism the \
      calculus can enforce, and the source scan is the floor under it. **The `namespace` corpus** pins \
      the four verdicts (`not-your-namespace`, `published`, `sealed`, `not-root-authority`) against the \
      deployed dictionary through `lean_namespace_corpus`; the row stays `proved-model` because the \
      model's paths are `Nat` tokens, not base58, and the corpus pins the verdict shape rather than the \
      encoding." },
  { number := 57, layer := "PoS",
    statement := "**A delegator's stake is the operator's bond, attributed.** A delegation moves a \
      principal from the delegator's own vault into the staking vault and adds it to the operator's key \
      — the `pool` entry becomes the **aggregate** — so the stake counts in the draw and in what a slash \
      reaches exactly as the operator's own does, while the `pos:delegations` ledger records who owns \
      the part that is there. Each epoch's reward for that key is split pro-rata across the operator's \
      own stake and its delegations **exactly** — the operator keeps the integer-division remainder — so \
      a validator with delegators commits the same total it would have committed alone, and the split is \
      the **identity** when nobody has delegated. An undelegation is a request that moves nothing until \
      a boundary acts on it, so a delegator cannot escape a slash already in flight; and a second \
      delegation from the same delegator **adds** to the first rather than replacing it",
    status := .provedModel,
    declarations := [`Rchain.PosDelegation, `Rchain.PosUndelegation, `Rchain.PosDelegationClaim,
      `Rchain.delegatedTotal, `Rchain.proRata, `Rchain.delegate, `Rchain.undelegate,
      `Rchain.lookup_setKey_self, `Rchain.a_second_delegation_accumulates,
      `Rchain.the_total_belongs_to_one_operator, `Rchain.proRata_sum_le,
      `Rchain.split_sums_to_the_reward, `Rchain.split_with_no_delegators_is_the_operator,
      `Rchain.the_split_is_not_the_identity, `Rchain.the_refund_is_split_pro_rata,
      `Rchain.delegate_conserves,
      `Rchain.a_delegation_enlarges_the_pool, `Rchain.undelegate_moves_no_coins,
      `Rchain.a_slash_clears_the_delegations],
    axioms := [],
    rust := ["rholang/src/native_state.rs", "rholang/src/system_processes.rs"],
    rustWitness := [
      "rholang/src/native_state.rs:the_delegation_leaves_are_absent_until_the_first_delegation",
      "rholang/src/native_state.rs:the_split_is_not_the_identity",
      "rholang/src/property_tests.rs:law57_the_split_sums_to_the_reward",
      "rholang/src/property_tests.rs:law57_the_delegators_never_take_more_than_the_reward",
      "rholang/src/property_tests.rs:law57_the_split_with_no_delegators_is_the_identity",
      "rholang/src/native_state.rs:a_delegation_enlarges_the_pool_and_a_second_one_accumulates",
      "rholang/src/native_state.rs:a_delegation_reaches_the_active_set_that_casper_reads",
      "rholang/src/native_state.rs:a_boundary_splits_the_reward_across_the_delegations",
      "rholang/src/native_state.rs:a_slash_fans_out_to_each_delegators_own_vault_at_every_tier",
      "rholang/src/native_state.rs:an_undelegation_is_staged_then_paid_to_the_delegator",
      "rholang/src/native_state.rs:a_validator_with_delegations_cannot_withdraw",
      "rholang/src/native_state.rs:the_delegation_refusals",
      "rholang/src/native_state.rs:the_delegation_leaves_round_trip_or_refuse"],
    witness := [`Rchain.a_second_delegation_accumulates, `Rchain.the_total_belongs_to_one_operator,
      `Rchain.proRata_sum_le, `Rchain.split_sums_to_the_reward,
      `Rchain.split_with_no_delegators_is_the_operator, `Rchain.the_split_is_not_the_identity,
      `Rchain.the_refund_is_split_pro_rata, `Rchain.delegate_conserves, `Rchain.a_delegation_enlarges_the_pool,
      `Rchain.undelegate_moves_no_coins, `Rchain.a_slash_clears_the_delegations],
    falsifiable := some "**the split's fixture was measured against three plausible wrong splits, not \
      asserted.** `the_split_is_not_the_identity` decides `proRata 100 30 [10, 61] = [9, 60]` — a \
      denominator of 101 over principals 10 and 61, flooring to 9 and 60 and leaving 31 to the operator \
      — together with `proRata 100 0 [30, 70] = [30, 70]` and `proRata 100 40 [1, 2] = [2, 4]`. Compiled \
      against those clauses: the operator keeps everything, every delegator gets an equal share \
      (`amount / (base + weights.length)`), and the base is dropped from the denominator. **Clauses 1 \
      and 3 refute all three; clause 2 refutes only the first**, because with `base = 0` it cannot tell \
      a rule that divides by the base from one that ignores it — recorded rather than left implied. \
      **Clause 3 read `[1, 1, 1]` when it was written and that was nearly vacuous for the same reason**: \
      three equal weights pay `100 / 3` under the real rule and under the equal-share mutant alike, so \
      it distinguished nothing clause 1 did not; `[1, 2]` with base 40 reads `[2, 4]`, which the \
      equal-share mutant reads as `[2, 2]` and the base-drop mutant as `[33, 66]`. The dormancy half is \
      `split_with_no_delegators_is_the_operator`, and the aggregate half is \
      `a_delegation_enlarges_the_pool`, which is red if `delegate` writes the ledger without the pool",
    note := "**Landed in two units, and the row is honest about which half is which.** The model and the \
      state's shape came first (the same change's spec unit); the Rust — the four leaf codecs, the two \
      `rho:rchain:pos` ops, the boundary move and split, the slash fan-out and the two refusals — is the \
      second, and `rustWitness` is declared with it. **Every falsifier was run against a mutation that \
      removes the thing it pins**, which is the standard this register keeps and which the split's own \
      fixture is the newest example of: dropping the slash fan-out, writing an empty delegation leaf, \
      replacing instead of accumulating a second delegation, not splitting the reward, removing the \
      withdrawal guard and paying an undelegation to the operator each turn the matching test red. \
      **Two established laws are touched rather than restated**: `sum_rewards_le_pot` (46) is a statement \
      about the reward *before* this split, which `split_sums_to_the_reward` shows the split preserves \
      exactly; and `atRisk` (law 45's slash) now reads the aggregate pool entry, which is why the \
      delegation ledger is **not** added to it — counting a delegator's principal twice would overstate \
      what a slash may take, and the escrowed claim and the accrued delegated reward, which have left the \
      pool or never joined it, are what the sum does add. The port's `slash` has the same subtraction for \
      the same reason, and getting it wrong there is a slash that mints. **What this row does not \
      claim**: which vault receives a refund, in the *model*. `PosState.user` is the total over every \
      user vault, so the per-vault fan-out is inexpressible there and is pinned by \
      `a_slash_fans_out_to_each_delegators_own_vault_at_every_tier` and \
      `an_undelegation_is_staged_then_paid_to_the_delegator` instead — the same simplification the \
      withdrawal path already carries. Nor does the model claim anything about the port's **dormancy at \
      the store**: that the four leaves are left absent rather than written empty is a property of \
      `native_state.rs`'s write path (`set_*` is an unconditional `put`, so the delegation setters clear \
      instead), pinned by `the_delegation_leaves_are_absent_until_the_first_delegation`" },
  { number := 58, layer := "Casper",
    statement := "**The gate publishes on a fully-participating set — the forward dual Law 14 lacks.** \
      Law 14's gate is a biconditional over a **free** `supp : SupportMap`: it says the fringe advances \
      *given* a support map that is a strict supermajority of the bonded total, and never says one \
      **obtains**. The live stall of 2026-10-03 (C209) was exactly that gap — a fully-live \
      three-validator net produced blocks and finalised nothing — and the model could say nothing about \
      it, because `Participation` and `StalenessBound` were declared and used in no theorem. \
      `participation_all_current` supplies the missing step (a participation-closed view *is* the whole \
      bonded set, hence a full partition), `gate_publishes_of_a_full_partition` and \
      `nextFringe_publishes_of_a_full_partition` carry it through the gate to the fringe, and \
      `the_licence_ends_at_the_horizon` bounds the attestation licence so a finality stall cannot become \
      the C171 storm. **What is hypothesis, not theorem:** \"a fully-live set reaches a full partition \
      within N rounds\" quantifies over a schedule and this model has none — the tree's own \
      `reduce_not_deterministic` proves the flat calculus fixes no schedule, and Law 51's `Fair` is named \
      and undefined — so it stays a named hypothesis, as do `Delivery` and the guard's reader being \
      `ReadsTheView` (the model can refute the *old* reader, not prove the new one). **The \
      `3 × LIVENESS_WINDOW` factor is measured, not proved** (`quiet_chain_tests`: a healthy round \
      finalises a deploy within 3 heights, a killed-validator round within 7).",
    status := .provedTied,
    corpus := some "liveness",
    declarations := [`Rchain.currentOf, `Rchain.stalenessBound_iff, `Rchain.participation_all_current,
      `Rchain.seersOf, `Rchain.suppOfView, `Rchain.allBonded_seersOf, `Rchain.bondedSupport_suppOfView,
      `Rchain.filterMap_stakeOf_eq_totalStake, `Rchain.fullPartitionStake_suppOfView,
      `Rchain.gate_publishes_of_a_full_partition, `Rchain.nextFringe_publishes_of_a_full_partition,
      `Rchain.inHorizon, `Rchain.the_licence_ends_at_the_horizon,
      `Rchain.latestOf, `Rchain.liveOf, `Rchain.currentOf_iff, `Rchain.window_iff_heights_behind],
    axioms := [],
    rust := ["casper/src/blocks/proposer/proposer.rs", "block-storage/src/dag/liveness.rs",
      "block-storage/src/dag/finalizer.rs"],
    rustWitness := [
      "casper/src/blocks/proposer/proposer.rs:the_licence_ends_at_the_horizon_and_the_work_does_not",
      "casper/src/blocks/proposer/proposer.rs:every_deploy_finalises_and_then_the_chain_is_quiet",
      "casper/src/blocks/proposer/proposer.rs:on_the_round_snapshot_a_deploy_sent_to_one_validator_is_never_finalised",
      "casper/src/blocks/proposer/proposer.rs:reading_only_the_work_from_the_seen_view_strands_the_genesis_signer"],
    falsifiable := some "the two round-snapshot *controls* in `quiet_chain_tests` are the falsifier: \
      reverting the guard to the round's parents makes \
      `on_the_round_snapshot_a_deploy_sent_to_one_validator_is_never_finalised` pass and this law's \
      premise false, and `reading_only_the_work_from_the_seen_view_strands_the_genesis_signer` refutes \
      the first cut — so \"every input from the seen view\" carries the forward implication rather than \
      decorating it. The horizon boundary is two-sided (`the_licence_ends_at_the_horizon`, pinned in \
      Rust by the test of the same name).",
    witness := [`Rchain.gate_publishes_of_a_full_partition, `Rchain.participation_all_current,
      `Rchain.the_licence_ends_at_the_horizon],
    note := "**The follow-up this row used to name has landed.** The row was `proved-model` over an \
      algebra the note called coarser than the node's — the model's `Nat`/`Option` against the port's \
      `BlockHeight`/`BTreeMap` — and the `liveness` corpus is the tie: eleven cases, each a tip, a window, \
      a bonds map and a latest-message map, whose verdict is the live weight set the filter returns. \
      `liveOf` is the model's; `block-storage/src/dag/liveness.rs:live_weight_set` is the node's; \
      `currentOf_iff` and `window_iff_heights_behind` are what make those one test rather than two \
      spellings that happen to agree on the cases somebody thought of. The cases carry the boundary \
      (`h + w = tip` is live, one past it is not) and the shape C174 is about — a bonded sender with **no \
      message at all**, which the node's `is_some_and` drops and a default height would have kept. \
      `Participation` and `Delivery` — declared in `Rchain/Casper/Liveness.lean` and used by nothing \
      before this row — are consumed here; `StalenessBound` is reached through `stalenessBound_iff`. \
      **What the tie does not reach:** the `3 × LIVENESS_WINDOW` factor, which is measured rather than \
      proved." },
  { number := 59, clause := "a", layer := "Wire",
    statement := "**A value the bridge decoded is a value it encodes, and decodes again** — the CapTP \
      value map round-trips its domain. Stated as Law 42 states the JSON round trip: over the values \
      `syToPar` answers, so a shape outside the domain carries no claim.",
    status := .provedTied,
    declarations := [`Rchain.parToSy, `Rchain.syToPar, `Rchain.wireable, `Rchain.wireableList, `Rchain.syrup_decode_encode, `Rchain.parToSy_decoded, `Rchain.ssToPars_round, `Rchain.kvsSyToPars_round, `Rchain.syToPar_isSome, `Rchain.ssToPars_isSome],
    axioms := [],
    corpus := some "syrup",
    rust := ["ocapn/src/par_value.rs"],
    rustWitness := ["ocapn/src/par_value.rs:collections_round_trip_and_a_tuple_stays_a_tuple"],
    witness := [`Rchain.syrup_decode_encode, `Rchain.parToSy_decoded],
    falsifiable := some "**the tuple, and it was false when the law was written.** `(true, 0)` — every \
      `(ok, value)` reply in this codebase — crossed as a Syrup `List` and came back an `EList`, so \
      `syToPar` of what `parToSy` produced was not the tuple: a peer could hold an ERTP purse and not \
      fund it (AUDIT C226). The URI was the same failure on a leaf (`GUri → Symbol` with `Symbol` \
      refused back). `syrupCases_decide` pins twelve cases, of which the tuple and the nested tuple are \
      two, and `node/tests/lean_syrup_corpus.rs` runs every one through the node's own bridge.",
    note := "**The law decided the wire shape, and two candidates were refuted by the references \
      rather than by preference.** A Syrup `record` is *labelled* and the label must be a string, \
      selector or bytestring (`@endo/ocapn`'s `decode.js`), so a bare record would need the label \
      `true`; and a record is not in Endo's CapTP passable union (`{list, struct, tagged}`) either. A \
      tuple therefore crosses as OCapN's **tagged** value, `<desc:tagged 'rho:tuple' [fields…]>` — the \
      union's own extension point, whose `value` may be any passable. `Sy.tuple` is that shape named, \
      so the round trip descends structurally instead of through a label test the equation compiler \
      cannot see (`taggedRecord` is the same shape spelled as a record). **Three shapes are outside the \
      domain, named:** a `Float64` (Rholang has no float), a **small** `GBigInt` — Syrup's integer is \
      one type where Rholang has two, so `GBigInt 5` and `GInt 5` share a wire form — and, until the \
      type was removed (issue #527), a `GDecimal` — there is no decimal left to refuse. **What the tie \
      does not reach:** the refusals' *reason strings* are the node's (`BridgeError`), tied by the \
      corpus's sources rather than by a theorem." },
  { number := 59, clause := "b", layer := "Wire",
    statement := "**Two wireable Syrup values that decode to the same `Par` are the same wire form.** \
      The decode is injective on the domain, so no two shapes stand for one value.",
    status := .provedTied,
    declarations := [`Rchain.syToPar_injective, `Rchain.parToSy_decoded],
    axioms := [],
    corpus := some "syrup",
    rust := ["ocapn/src/par_value.rs"],
    witness := [`Rchain.syToPar_injective],
    falsifiable := some "**a rendering rule refutes it, which is what makes the clause load-bearing \
      rather than decorative.** 'An inbound list argument becomes a tuple' is the cheap fix for C226, \
      and under it a peer's `[brand, 10]` and a peer's `(brand, 10)` would be one `Par` reached from \
      two wire forms — `syToPar_injective` is false of it by construction. The corpus's \
      `[1, 2]` row beside its `(true, 0)` row is the same statement on the wire: the two render \
      differently, and `node/tests/lean_syrup_corpus.rs` asserts it of the node.",
    note := "**The clause the bridge had no way to state before.** With only an encode and a decode, \
      'a list and a tuple are different things' is a fact about two functions that happens to hold; \
      stated as injectivity it is a property of the map, and it is what makes the *shape* a decision \
      the law fixes. Both clauses 59a and 59b follow from `parToSy_decoded` — what a value the decoder \
      answered encodes back to — so the row carries one proof obligation, not two." },
  { number := 59, clause := "c", layer := "Wire",
    statement := "**The encoder never emits a shape outside the domain.** A value with no counterpart on \
      the other side is *refused*, never approximated by one that nearly fits — §1.6's no-silent-\
      partiality applied to a wire.",
    status := .provedTied,
    declarations := [`Rchain.parToSy_wireable, `Rchain.exprToSy_wireable, `Rchain.parsToSy_wireable, `Rchain.kvsToSy_wireable, `Rchain.wireable],
    axioms := [],
    corpus := some "syrup",
    rust := ["ocapn/src/par_value.rs"],
    witness := [`Rchain.parToSy_wireable],
    falsifiable := some "**the URI was the violation, and it is why this clause exists.** `GUri → \
      `Symbol` outbound with `Symbol` refused inbound is a *lossy* map: the encoder emits a shape from \
      which the value does not come back, which is exactly what this clause forbids. The domain's \
      `float` and small-`GBigInt` exclusions are the other side of it — shapes the encoder must not \
      produce and the decoder must not accept. `node/tests/lean_syrup_corpus.rs`'s round trip is the \
      falsifier on the node: an encoder that emitted a lossy shape fails on the case that carries it.",
    note := "**This is the clause that turns a documented decision into a checked one.** The bridge's \
      module doc used to call the `GUri`/`Symbol` asymmetry 'deliberate and worth a second opinion', \
      and the tuple's image 'the one loss, and it is the loss Syrup forces'. Both were true as prose \
      about two functions and neither was a property: 59c says the encoder's image lies inside the \
      domain, so a loss is a violation rather than a note. The leaf is `exprToSy_wireable` over the \
      expression constructors — every one the encoder refuses is refused by the definition's own \
      fall-through, so the proof is a case per constructor rather than an argument." },
]

end Laws
end Rchain
