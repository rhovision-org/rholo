import Rchain.Laws.Base

/-!
# The law register, chunk 05 (laws 60–79)

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

def lawsChunk05 : List Law := [
  { number := 60, clause := "a", layer := "RSpace",
    statement := "**An install is total on a channel.** An install that would replace a *different* \
      installed continuation is **refused**, and an idempotent re-install of the same one changes \
      nothing — so an install that cannot take effect says so rather than dropping what was there.",
    status := .provedModel,
    declarations := [`Rchain.installed, `Rchain.InstallStep, `Rchain.ReplacingStep, `Rchain.a_different_install_has_no_step],
    axioms := [],
    rust := ["rspace/src/hot_store.rs", "rholang/src/system_processes.rs"],
    rustWitness := [ "rspace/src/hot_store.rs:a_second_different_install_on_a_channel_is_refused", "casper/tests/determinism.rs:a_minted_vault_handle_answers_its_balance_arm"],
    witness := [`Rchain.a_different_install_has_no_step],
    falsifiable := some "**the defect's shape is a second relation, and the row is the pair.** \
      `ReplacingStep` is what `installed_continuations.insert` did — the second install replaces the \
      first — and `the_replacing_rule_does_not_keep_the_first` states that the first is then gone. \
      `the_vault_handles_balance_arm_was_lost` is the same fact at the size of the finding: install \
      `transfer` after `balance` on one channel and the channel no longer carries `balance`. On the \
      port, `rspace`'s test asserts the `Err`, and \
      `casper/tests/determinism.rs:a_minted_vault_handle_answers_its_balance_arm` reads the arm that \
      used to answer nothing — a vault seeded with 1_000_000_000, so a handler that replied a \
      constant fails it.",
    note := "**The refusal is the clause, and it is why 60a is not the store's own type.** The map \
      holds one `WaitingContinuation` per channel, so a second `insert` *cannot* fail — the \
      statement has to be about the operation, not the container: an install that would replace a \
      different continuation has no step, which is what `install_continuation` now returns. The \
      idempotent case is load-bearing rather than incidental: play and replay both install the system \
      contracts over one store, and a rule that refused the *same* install twice would break every \
      deploy. **What the tie does not reach:** the model's continuations are abstract tokens, so it \
      says which one a channel keeps and nothing about what a continuation is; `locked_install`'s \
      consume comparison is the node's, pinned by the rspace test rather than by a theorem." },
  { number := 60, clause := "b", layer := "RSpace",
    statement := "**What a channel carries after any permitted run** — the continuation it was first \
      given. A contract with several methods is therefore **one** continuation that dispatches on the \
      method, not several installs on one name.",
    status := .provedModel,
    declarations := [`Rchain.a_permitted_run_keeps_the_first, `Rchain.InstallStep, `Rchain.installed],
    axioms := [],
    rust := ["rholang/src/system_processes.rs", "rspace/src/hot_store.rs"],
    rustWitness := [ "casper/tests/determinism.rs:a_minted_vault_handle_spends_in_the_deploy_that_minted_it"],
    witness := [`Rchain.a_permitted_run_keeps_the_first],
    falsifiable := some "**the pair with 60a's falsifier is the falsifier**, which is the shape \
      `Rchain/Casper/Dag.lean` establishes for law 14b: `a_permitted_run_keeps_the_first` is the law \
      on the rule, and `the_replacing_rule_does_not_keep_the_first` is the *same statement* false of \
      the rule the fix replaces. Without the second, the first would be a restatement of a container's \
      type; with it, the rule is what is being judged. On the port the control is a *movement*: \
      `a_minted_vault_handle_spends_in_the_deploy_that_minted_it` asserts the transfer moved \
      30_000_000, so the one-continuation handle is a working capability rather than merely a \
      non-refusing one.",
    note := "**The shape every multi-method contract here already used, now the only shape that \
      installs.** `rev_vault`, `ertp` and `pos` are one continuation at `arity: 1, remainder: true` \
      with the method dispatched inside; a vault handle is the one site that did it the other way, and \
      this is the row that says which way is the rule. **What the change costs:** the handle's \
      `body_ref` is derived from `(name, arity)` and the arity went 2/5 → 1, so newly minted handles \
      move — consensus-visible, with the note in `spec/GENESIS.md`. The oracle's \
      `RevVault.rho:196-200` spells the two arms as two `contract`s on one name, which is what the \
      port's one-continuation-per-channel store cannot express; the dispatch is where that difference \
      is paid." },
  { number := 61, clause := "a", layer := "Progress",
    statement := "**A delivery in flight does not stall the loop.** With an answer outstanding, the \
      session's loop can still read the next delivery — so an unrelated delivery on the same session \
      is served while an object waits on something outside the session.",
    status := .provedModel,
    declarations := [`Rchain.Delivery, `Rchain.LoopState, `Rchain.Step, `Rchain.BlockingStep, `Rchain.a_pending_answer_does_not_block_the_next_delivery],
    axioms := [],
    rust := ["ocapn/src/conn.rs", "ocapn/src/owner.rs", "ocapn/src/bootstrap.rs"],
    rustWitness := [ "ocapn/tests/session_owner.rs:a_claim_that_waits_does_not_stall_its_session"],
    witness := [`Rchain.a_pending_answer_does_not_block_the_next_delivery],
    falsifiable := some "**the defect's shape is a second relation, and the pair is the claim.** \
      `BlockingStep` is what polling inside the delivery did — an answer outstanding means the loop \
      stays exactly where it is — and `the_blocking_rule_never_shortens_the_queue` states that under \
      it **no** step shortens the queue while an answer is outstanding: the loop spins, and the \
      delivery behind it is unreachable. That is the stall, and it is false of `Step`, where \
      `Step.defer` reads the delivery and takes the answer on. On the port \
      `ocapn/tests/session_owner.rs:a_claim_that_waits_does_not_stall_its_session` sends a signed \
      handoff claim for a gift nobody will ever deposit and then, on the **same** socket, a fetch: the \
      fetch must be fulfilled within seconds, where the old shape could not read it until the \
      ten-second deposit wait ended.",
    note := "**The statement is about the loop, because that is where the stall was.** `handle_deliver` \
      is awaited on the task that owns the socket, so an object that awaits something outside the \
      session does not merely delay its own answer — it stops the session reading anything. The fix \
      is `Reply::Deferred`: the object hands the loop a future, the loop writes the answer when it \
      lands, and the *connection stays single-owner*, which is the invariant the whole module is \
      built on. **What the model does not claim:** deliveries are abstract tokens, so it says when the \
      loop reads and nothing about what a delivery says, and the queue is a `List` where the port's is \
      a socket. **What the deferral costs:** the answer position of a deferred delivery is recorded \
      with no object, so a delivery *pipelined* onto it breaks rather than resolving — the honest \
      answer while the answer itself is unknown, and re-recording it later would be the silent \
      re-point C223 already refuses." },
  { number := 61, clause := "b", layer := "Progress",
    statement := "**And the outstanding answer is still written** — deferring is not dropping. The loop \
      reaches a state with nothing pending.",
    status := .provedModel,
    declarations := [`Rchain.Reach, `Rchain.a_deferred_answer_is_still_written, `Rchain.Step],
    axioms := [],
    rust := ["ocapn/src/owner.rs", "ocapn/src/conn.rs"],
    rustWitness := [ "ocapn/tests/session_owner.rs:a_claim_that_waits_does_not_stall_its_session"],
    witness := [`Rchain.a_deferred_answer_is_still_written],
    falsifiable := some "**the clause a deferral most easily gets wrong**, and the reason it is stated \
      separately: 'the loop reads on' is trivially satisfiable by never answering at all. The proof \
      exhibits the run — serve, then fulfil — so a relation that only had the reading step would not \
      satisfy it. On the port the same shape is the conformance suite's \
      `test_valid_handoff_wait_deposit_gift`, which withdraws *before* the deposit and requires the \
      gift anyway: the deferred path is exercised by \
      `spec/audit/evidence/ocapn-conformance/run-11.txt` (24/24), and a deferral that dropped the \
      answer would fail it.",
    note := "**Two clauses rather than one, because they fail independently.** 61a says the loop is not \
      blocked; 61b says the work is not lost. A change that answered nothing would satisfy the first \
      and fail the second, which is why the row exists — and why the port's fix had to write the \
      answer *from the loop* (a spawned waiter would have had to reach the socket, which is the \
      single-owner invariant). **What the model does not claim:** that the answer is written in \
      bounded time — `DEPOSIT_WAIT` bounds the wait and the `Deferred` queue is bounded \
      (`MAX_DEFERRED_ANSWERS`), but neither bound is in this model." },
  { number := 62, layer := "Wire",
    statement := "**A peer cannot extend this node's reach.** The node never dials, on a peer's word, \
      anything that peer could not dial itself — so a *remote* peer cannot aim this node at its own \
      loopback services, while a loopback peer (the conformance suite's own case) still can.",
    status := .provedModel,
    declarations := [`Rchain.Origin, `Rchain.Target, `Rchain.ownReach, `Rchain.isLocal, `Rchain.permits, `Rchain.originBlind, `Rchain.the_node_never_exceeds_the_peers_own_reach, `Rchain.the_origin_blind_rule_exceeds_the_peers_reach, `Rchain.denying_local_targets_outright_would_refuse_a_local_peer],
    axioms := [],
    rust := ["ocapn/src/dial_policy.rs", "ocapn/src/netlayer.rs", "ocapn/src/enliven.rs", "ocapn/src/fixtures.rs"],
    rustWitness := [ "ocapn/src/dial_policy.rs:a_remote_peer_cannot_reach_this_nodes_own_services"],
    witness := [`Rchain.the_node_never_exceeds_the_peers_own_reach],
    falsifiable := some "**the defect's shape is a second rule, and the pair is the claim.** \
      `originBlind` judges the target without the peer — what the policy did before the socket address \
      was threaded to it — and `the_origin_blind_rule_exceeds_the_peers_reach` states that under it, \
      with the **default** configuration, a peer elsewhere reaches this node's own services: reach the \
      peer does not have. The law's own thesis is a *containment* rather than a list of denied \
      targets, proved for every configuration of the policy's ingredients (the always-refused set, the \
      operator's `deny_local`, and the origin), so refusing more can only shrink the node's side. \
      `denying_local_targets_outright_would_refuse_a_local_peer` is the control that rules out the \
      cheap fix: refusing local targets outright breaks the demo the conformance suite is. On the port, \
      `ocapn/src/dial_policy.rs:a_remote_peer_cannot_reach_this_nodes_own_services` asserts both halves \
      — remote refused, loopback permitted — and \
      `a_remote_peer_cannot_reach_this_node_by_name` that a *name* resolving to loopback is the same \
      dial.",
    note := "**The origin had to be carried to the policy, and that is most of the fix.** The policy \
      could not tell one peer from another because nothing on the dial path knew where the request came \
      from: `NetConn` gains a provided `peer_address`, the session handle carries it, and `Netlayer` \
      gains a provided `new_outgoing_connection_from` so the two dial sites that *do* know — the \
      enlivener and the handoff greeter, both session-local objects — can say which peer asked. Both \
      are **provided** methods: the netlayer standard fixes two functions, and a transport that cannot \
      report an origin keeps the behaviour it had rather than becoming non-conforming. **What the model \
      does not claim:** `Origin` is two-valued where the port has a socket address and a list of \
      ranges, and `Target` is three-valued where the port has every `IpAddr` — the model says the shape \
      of the rule and the port says which addresses are in which class, pinned by its own tests. \
      **What the operator still decides:** whether local targets are refused at all is `deny_local`, \
      unchanged; the origin rule is not switchable, because it is the containment rather than a \
      preference." },
  { number := 63, clause := "a", layer := "Protocol",
    statement := "**A bridged delivery's consensus footprint is attributable to a payer** — the state a \
      session causes is one the peer that caused it pays for, and this node neither pays phlo it did \
      not choose to spend nor carries consensus state it cannot remove.",
    status := .open,
    declarations := [`Rchain.Payer, `Rchain.Write, `Rchain.Attributable, `Rchain.bridgedWrite, `Rchain.a_bridged_write_is_not_attributable, `Rchain.AttributableWrite, `Rchain.the_relay_would_satisfy_it, `Rchain.bounding_the_state_does_not_make_it_attributable],
    axioms := [],
    falsifiable := some "**the falsifier is the state as it is, and it is a theorem rather than a \
      paragraph about one.** `a_bridged_write_is_not_attributable` states that a write a session \
      caused has `thisNode` as its payer, for every size of write: every delivery to a \
      method-carrying chain capability runs `insertArbitrary` unconditionally and the deploy is \
      signed by the node's key, so the chain binds the node and the node's vault pays. \
      `bounding_the_state_does_not_make_it_attributable` is the sentence the row's *first* proposed \
      close condition got wrong: the law is about **who pays**, so a smaller write is the same \
      unattributable write — which is why the repair is the relay and not a bound. \
      `the_relay_would_satisfy_it` shows the law is not vacuous: a write whose payer is the caller and \
      whose state is within what the caller's phlo bought exists, which is what the relay produces.",
    witness := [`Rchain.a_bridged_write_is_not_attributable, `Rchain.the_relay_would_satisfy_it],
    note := "**Why this one is `open` and the other three laws of this family are not.** The OCapN \
      surface's *bounds* are already guards before the work — `MAX_SESSIONS`, the export and answer \
      tables' caps, `MAX_DEFERRED_ANSWERS`, the bridged-deploy rate limiter — and that is **Law 55's \
      clause a**, which is why C222's row now cites `55a` rather than proposing a law of its own \
      (rule 3: one home). What no bound reaches is *attribution*: the node cannot know whether a \
      reply holds a capability before registering it, because the value does not survive the \
      evaluation, so it cannot refuse the write on that ground. The close is the **relay** — the node \
      hands the peer the exact `DeployData` to sign with its own secp256k1 key, so the chain binds the \
      peer as `deployerId` and the peer's vault pays — and it cannot be landed here: Endo does not \
      speak this shard's `DeployData` shape, so it is a cross-implementation protocol change. The row \
      names that rather than a fix this tree cannot reach. **What the model does not claim:** the \
      state is a byte count and the payer is two-valued, so it says the *shape* of the obligation and \
      nothing about what a reply holds; the surviving growth is a chain-wide property of the registry \
      (no delete anywhere), bounded economically by whoever pays, and that property is the registry \
      layer's rather than this one's." },
  { number := 63, clause := "b", layer := "Protocol",
    statement := "**Given the relay, a bridged delivery is attributable.** If the peer signs the \
      `DeployData` the node hands it, the write's payer is the caller and its state is bounded by the \
      caller's own phlo — the shape 63a names as its close.",
    status := .provedModel,
    formal := `Rchain.law63b_statement,
    mutant := `Rchain.law63b_mutant,
    falsifier := `Rchain.law63b_falsifier,
    declarations := [`Rchain.law63b_statement, `Rchain.law63b, `Rchain.law63b_mutant,
      `Rchain.law63b_falsifier, `Rchain.the_relay_would_satisfy_it, `Rchain.AttributableWrite],
    axioms := [],
    rust := ["casper/src/shard_invoke.rs"],
    witness := [`Rchain.law63b_falsifier, `Rchain.the_relay_would_satisfy_it],
    falsifiable := "`law63b_falsifier` is the current bridge: `bridgedWrite` is an unattributable write, \
      so the conditional is not vacuous — the relay is what removes the mutant's witness. The relay \
      itself is not in this tree, which is why 63a stays `open`.",
    note := "**This clause does not close 63a.** The relay is a cross-implementation protocol change \
      (Endo does not speak this shard's `DeployData`), so no code here can satisfy it; what is proved is \
      the conditional shape a fix must have. **No executable Rust witness exists** for the same reason: \
      `casper/src/shard_invoke.rs` is the unrelayed path, and the model's \
      `the_relay_would_satisfy_it` is the positive half." },

  { number := 64, clause := "a", layer := "Wire",
    statement := "**Where the draft and the implementations differ, the port speaks the \
      implementations' reading.** The prose is not the oracle; what every peer actually speaks is.",
    status := .provedModel,
    declarations := [`Rchain.Source, `Rchain.Form, `Rchain.Disagreement, `Rchain.speaks, `Rchain.the_port_speaks_the_implementations_reading, `Rchain.the_draft_is_not_the_oracle],
    axioms := [],
    rust := ["ocapn/src/captp.rs", "ocapn/src/session.rs", "casper/src/shard_invoke.rs"],
    rustWitness := [ "ocapn/tests/reference_vectors.rs:start_session_matches_the_reference", "casper/src/shard_invoke.rs:reply_channel_is_the_deploy_id"],
    witness := [`Rchain.the_port_speaks_the_implementations_reading, `Rchain.the_draft_is_not_the_oracle],
    falsifiable := some "**the figures are the drafts', and they disagree.** AUDIT C216: the draft \
      defines `op:start-session` with **five** fields and contradicts itself about one of them \
      (`Ed25519_SHA256` when constructing, `Ed25519` when receiving), while the suite's \
      `OpStartSession` carries **four** — so a port following the prose fails against every \
      implementation. AUDIT C218: `invoke_term` passed the reply channel as a backticked \
      `` `rho:rchain:deployId` ``, which is a `GUri` *ground* — an ordinary, guessable name — where the \
      reference binds the unforgeable per-deploy channel; the port spoke a reading **no** \
      implementation produces, and every reply went nowhere while the deploy reported success. \
      `the_draft_is_not_the_oracle` is the clause as a theorem: the two readings differ, so speaking \
      one is not speaking the other. The tie is the known-answer corpus — \
      `ocapn/tests/reference_vectors.rs`'s vectors are produced by the suite's own \
      `contrib/syrup.py` encoder, and `start_session_matches_the_reference` pins the four-field form.",
    note := "**Both halves of this row are about the same mistake, made in opposite directions.** \
      Following the prose (C216) fails against the peers; inventing a reading the prose and the peers \
      both lack (C218) fails silently, because the wire does not complain about a name nobody reads. \
      The oracle is the **implementation**, which is what `ocapn/src/captp.rs`'s module doc states and \
      what the KATs enforce. **What the model does not claim:** a reading is a `Nat` where the port has \
      a record shape, so the model says *which* reading is spoken and nothing about its fields; the \
      divergences themselves are rows in the findings register (C216, C218, C224), which is where a \
      reader finds what each one is." },
  { number := 64, clause := "b", layer := "Wire",
    statement := "**Where the implementations disagree with each other, the port accepts every reading \
      they produce** — the union on the reading side, because there is no single reading to speak.",
    status := .provedModel,
    declarations := [`Rchain.accepts, `Rchain.every_reference_reading_is_accepted, `Rchain.oneReading, `Rchain.one_reading_refuses_another, `Rchain.the_union_accepts_it],
    axioms := [],
    rust := ["ocapn/src/bootstrap.rs", "ocapn/src/session.rs"],
    rustWitness := [ "ocapn/src/bootstrap.rs:bootstrap_deliver_accepts_a_swiss_number_as_bytes_or_as_a_string"],
    witness := [`Rchain.every_reference_reading_is_accepted],
    falsifiable := some "**the falsifier is the port's own history, stated as a rule.** `oneReading` is \
      what 'the reference implementation is the oracle' was taken to mean before it was noticed that \
      there are two — a port that speaks one reading — and `one_reading_refuses_another` is AUDIT C217 \
      exactly: `@endo/ocapn` sends a swiss number as a Syrup **String** (which is what the draft says \
      it is) and the Python suite sends a **byte array**, so a port built to either alone refuses the \
      other. `the_union_accepts_it` states the contrast on the same row, so the pair is a claim rather \
      than a preference. On the port, \
      `bootstrap_deliver_accepts_a_swiss_number_as_bytes_or_as_a_string` asserts both readings are \
      accepted, and C224's locator-hints case is the same shape one field over.",
    note := "**Why the reading side is a union and the speaking side is not.** A port has to *choose* \
      one shape to emit, and choosing the implementations' is right because they are what peers read; \
      it does not have to choose one to *accept*, and choosing would refuse a peer that is not wrong. \
      The asymmetry is the law's content, not an inconsistency. **What the model does not claim:** \
      that every divergence is tolerated — C217's is, and C224's four are settled readings the port \
      *pins* rather than accepts both ways; the register's rows say which is which. **What the law \
      cannot reach:** a third implementation with a third reading would be another row, and nothing in \
      this model finds it — the corpus is where a new reading shows up." },
  { number := 65, layer := "Rholang",
    statement := "**A value with no faithful literal is refused, not written.** Every literal the \
      printer writes reads back as the value it was written for, and a value that has no such literal \
      is refused rather than written as one that reads back as something else.",
    status := .provedModel,
    declarations := [`Rchain.Literal, `Rchain.Faithful, `Rchain.mayWrite, `Rchain.the_printer_writes_only_what_reads_back, `Rchain.an_unfaithful_literal_is_refused, `Rchain.theQuotedStringCase, `Rchain.the_quoted_string_case_is_refused],
    axioms := [],
    rust := ["rholang/src/pretty_printer.rs", "casper/src/shard_invoke.rs"],
    rustWitness := [ "casper/src/shard_invoke.rs:an_argument_that_would_close_its_literal_is_refused"],
    witness := [`Rchain.the_printer_writes_only_what_reads_back, `Rchain.the_quoted_string_case_is_refused],
    falsifiable := some "**the falsifier is a concrete literal, and it was written into signed terms.** \
      Rholang's literal grammar has **no escape**, so a string containing a quote has no faithful \
      literal at all: `theQuotedStringCase` is one — the printer meant seven units and a reader gets \
      the three before the quote — and `the_quoted_string_case_is_refused` says both that it is \
      unfaithful and that the guard refuses it. Two production paths printed values into terms they \
      then parsed, and **the values arrived from a peer** in the OCapN bridge's case \
      (`casper/src/shard_invoke.rs`'s builders), so what was signed was a body the peer chose. On the \
      port, `an_argument_that_would_close_its_literal_is_refused` is that case.",
    note := "**This is Law 59's clause c for a second encoder.** 59c says an encoder never emits a shape \
      outside its domain; here the domain is the literals that read back, and the encoder is the \
      printer rather than the bridge. It is a separate law rather than a clause of 59 because the \
      *domain* is different — a printable literal is a syntactic object, not a wire value — and because \
      the failing thing is different: 59's writes go to a peer, this one's go into a term the node's \
      own key signs. **What the model does not claim:** the values are `Nat`s standing for a value and \
      its read-back, so it says the *guard* and nothing about the encoding; that Rholang *could* have \
      an escape (it cannot — that is the grammar's choice, and it is why the answer is refusal rather \
      than quoting) is stated in the module doc, not modelled. **What the tie does not reach:** the \
      three printer warts Law 33 names are a different row's." },

  -- ---------------------------------------------------------------------------------------------
  -- The distributed-system assumptions: an atomicity model and a model of join (2026-10-10).
  -- These six rows are the first laws about *the node as a distributed system* rather than about
  -- rholang evaluation. Each was written refutation-first, and two of them are the refutation: Law
  -- 66's statement is false of the code, and that is the theorem. Two adversarial reviewers attacked
  -- all six; their verdicts are in each row's `note`, including the verdicts that cost the plan its
  -- chosen fix.
  -- ---------------------------------------------------------------------------------------------

  { number := 66, layer := "Casper",
    statement := "**The value a node stores at a fringe key is not determined by that key.** The record \
      at `fringe_hash_of(fringe)` holds a `state_hash` the key never mentions: the merge that produced \
      it started from a **base** (the previous fringe's state, or the base block's post-state), and two \
      writers whose merges started from different bases store different states under one key. The join's \
      `min` tie-break then silently settles it — and `the_join_rewrites_a_writers_value` is what that \
      costs: a writer that had computed the other value finds its own block failing its own validation \
      (C270, measured)",
    status := .provedModel,
    declarations := [`Rchain.the_state_is_named_by_a_block, `Rchain.a_disagreement_is_visible,
      `Rchain.claimsOf, `Rchain.the_fringe_only_key_is_incomplete,
      `Rchain.the_join_rewrites_a_writers_value],
    axioms := [],
    rust := ["models/src/block_metadata.rs", "casper/src/dag.rs",
      "casper/src/multi_parent_casper.rs"],
    witness := [`Rchain.the_state_is_named_by_a_block],
    falsifiable := "Two honest writers whose merges started from different bases, reaching one fringe: \
      their records share the key and disagree about `stateHash`. The measured instance is the four-key \
      divergence in `spec/audit/evidence/n-anchor-drill/self-reject-stall.txt`.",
    note := "**What this law is, precisely, and what the reviewers took away from it.** The statement is \
      an *impossibility about the design*: a key reading only the fringe cannot determine a value that \
      also depends on the base. It is **not** a re-derivation of the measured incident — the witness \
      (`recordOf ∅ 0` / `recordOf ∅ 1`) is an artefact of the model's abstract `mergeFringe f b := b` \
      in which the state *is* the base by construction, while the incident's pair had non-empty fringes. \
      **The positive half of that module is vacuous and is deliberately not cited**: \
      `the_composite_key_determines_the_value` and its join-side twins are tuple injectivity (`rfl`), \
      i.e. the statement that `recordOf` is a function; and `no_fringe_only_key_can_be_complete` is a \
      near-duplicate whose universal quantifier adds nothing (a remark, not a cell). **The refutation \
      that mattered was of the fix, not of the claim**: keying by `(fringe, base)` is *not implementable \
      at the reader that needs it*. `multi_parent_casper.rs:156-165` reads by the fringe-only key, and \
      the base it would need is computed *later* (`:230-238`) as the base block's post-state else the \
      previous fringe's state — the base **is** the value that read returns, so a reader asking for a \
      fringe's state cannot name which base the record it wants was written under. The design chosen \
      afterwards (2026-10-10) is that the **value drops the derived state**: the record keeps the \
      contributions and the reader derives, which makes the store append-only — the property Law 68 \
      proves is what makes a read a function of its version. **Delivered 2026-10-10, and the row now
      states what the code does.** The store, the type, the join and the tie-break have retired: the
      claim travels with the block that made it (`BlockMetadata.fringe_state_hash`, written once) and
      the readers derive. `the_state_is_named_by_a_block` is the version rule they rely on, and
      `a_disagreement_is_visible` is this refutation's positive counterpart, which is why the
      refutation is kept rather than deleted. The readers take the claim of the block that **finalised**
      the fringe — the parent whose own `fringe` field is that set — and not the claims of the fringe's
      members, which are one fringe earlier: the first cut read the members, and a live four-validator
      devnet stalled at height 20 with `active-bonds leaf not found in state` until the drill caught it.
      With the correction the same rig runs level and advancing — 218 heights in seven minutes, all four
      within two heights of each other, zero self-rejections, zero `active-bonds` failures
      (`spec/audit/evidence/n-anchor-drill/divergence-fixed.txt`; `passes.md` §96)." },

  { number := 67, layer := "Casper",
    statement := "**The record join is a commutative idempotent semigroup at one key, so the value at a \
      key is a function of the set of arriving records — and it is not a monoid.** `joinRecord` copies \
      `fringe_hash` and `fringe` from its left operand, so commutativity holds exactly when both \
      arguments are records for one key (the code's own stated precondition, `dag.rs:79-80`), and it has \
      no identity, which the model proves absent rather than fabricating. The sharpening is the one that \
      matters: order-independence does not come from the arrivals agreeing — `min` is commutative, so two \
      records that **disagree** still fold to one, which is why a divergence was invisible in the store",
    status := .retired,
    declarations := [`Rchain.joinRecord_assoc, `Rchain.joinRecord_comm, `Rchain.joinRecord_idempotent,
      `Rchain.joinRecord_fold_perm, `Rchain.the_joined_value_is_a_function_of_the_set,
      `Rchain.the_join_changes_nothing_iff_the_report_adds_nothing],
    axioms := [],
    rust := ["casper/src/dag.rs:82"],
    witness := [`Rchain.joinRecord_fold_perm],
    falsifiable := "A pair of records at one key whose folds in two orders differ — \
      `joinRecord_not_commutative` exhibits the failure for records at *different* keys, so the \
      `AtOneKey` hypothesis cannot be read as a convenience.",
    note := "**Verified faithful field by field** against `join_fringe_records` (left-biased key data, \
      unioned `fringeDiff` and rejection sets, `min` state) by a review that tried and failed to break \
      the laws, and confirmed `the_join_changes_nothing_iff_the_report_adds_nothing` has no missing \
      conjunct. **What is not cited: that module's `writeKey` section.** `writers_at_one_writeKey_agree` \
      and its four siblings are consequences of the tuple identity `writeKey f b = (f, b)`, as vacuous \
      as Law 66's positive half — and their premise, that a caller can *form* a `writeKey`, is exactly \
      what the review refuted. One honesty note the review added: `joinRecord_not_commutative`'s witness \
      violates the model's own `fringeHash = fringeHashOf fringe` invariant, so it is a non-vacuity \
      check and not a pair the store can hold. **RETIRED 2026-10-10 with the map it describes.** There
      is no record join left to be a semigroup: the claims are per block and the 'join' is a union of
      them. The row is kept, as the register's ``retired`` status intends, because its finding is the
      reason the store went — order-independence did *not* come from the arrivals agreeing, which is
      why a divergence could hide in the store — and because a reader of the old design deserves to
      find that argument where the join used to be." },

  { number := 68, layer := "Casper",
    statement := "**A read is a function of the version it names, and what a store must be for that to \
      hold is append-only, not content-addressed.** Two content-addressed reads of one version agree \
      because a hash collision is impossible (Law 10's `root_collision_free`); a store that only ever \
      appends has the same property without hashing at all. The record store today is neither: its \
      `put` and join **merge into an existing key**, and a read for a fringe's state can then return \
      either of two values",
    status := .provedModel,
    declarations := [`Rchain.two_content_addressed_reads_of_one_version_agree,
      `Rchain.a_read_of_an_append_only_store_is_stable,
      `Rchain.a_read_of_a_blocks_claim_is_stable,
      `Rchain.a_non_content_addressed_read_is_not_a_function_of_its_version],
    axioms := [],
    rust := ["rspace/src/history/radix_tree.rs", "models/src/block_metadata.rs",
      "tools/reconcile-network.sh"],
    witness := [`Rchain.a_read_of_an_append_only_store_is_stable],
    falsifiable := "Two stores that differ only in the base of the record at one key, read at that key: \
      different values for one version. The Rust instance is the `fringe-data` environment; the \
      operational one is the recovery tool's `--restore-from-master`, whose own text calls a copy of a \
      live store a torn snapshot.",
    note := "**The dichotomy is the finding.** The positive half is a composition over Law 10 and adds \
      no new mathematics; the refutations restate Law 66's collision in store vocabulary, which the \
      review called duplicative and which is registered here as **clauses in substance rather than a \
      second law** — the declaration cited is the append-only half, the part that is new. The general \
      theorems (`read_is_a_function_of_its_version`, `read_reads_by_version`) are `rfl` unpackings of \
      `read` and are **not cited**. **And the row corrects the tool's own framing**: a torn copy of a \
      *content-addressed* store cannot violate a read for any key it holds whole, so the fiat is needed \
      only for the non-append-only half. That module's header claim — that the composite key would bring \
      the record store into the safe class — is unproved and, as Law 66 records, inexpressible; the \
      chosen design gets there another way. **Since 2026-10-10 it holds for the fringe state**, which
      is the point of the redesign: a claim is written once, at the block that made it, so
      `a_read_of_a_blocks_claim_is_stable` is this law's positive half instantiated at the claim store
      rather than a new law. The tool's fiat still applies to the chain-state *directories*, which are
      not where the claims live." },

  { number := 69, layer := "Sync",
    statement := "**A joiner's state-page walk need not terminate, and with a give-up rule it is paced.** \
      As written, the walk's request loop exits only on an error or `is_finished`, so a peer that simply \
      never answers leaves an infinite run — an infinite *trace*, not the absence of reachability, \
      because the walk can finish and need not. Add the idle give-up the block leg already has and every \
      turn spends fuel: no infinite run, and a bounded exit",
    status := .provedModel,
    declarations := [`Rchain.Sync.Walk.the_page_walk_as_written_spins_for_ever,
      `Rchain.Sync.Walk.the_fixed_walk_cannot_run_for_ever,
      `Rchain.Sync.Walk.the_page_walk_with_the_give_up_rule_is_paced],
    axioms := [],
    rust := ["casper/src/engine/lfs_tuple_space_requester.rs", "casper/src/engine/lfs_block_requester.rs",
      "casper/src/engine/node_syncing.rs"],
    witness := [`Rchain.Sync.Walk.the_page_walk_as_written_spins_for_ever],
    falsifiable := "A run of the machine as written that never reaches `is_finished` — and the adversary \
      need not answer unsolicited keys: the same constant trace is an infinite run using only the \
      request loop's resend, which the Rust does with no bound. The measured stall is C268's stuck \
      joiner. `ReachesTheGoal` is left as an unproved `Prop`: reaching the goal needs the *peer* to \
      answer, which a give-up rule cannot supply.",
    note := "**The review strengthened this one.** The falsifier looks as if it depends on an adversarial \
      environment answering a key the walk never requested; a reviewer compiled the variant that uses \
      only the resend, so it needs nothing but a silent peer — the machine the Rust is in, with no \
      bound. The modelled `accept` also makes the stall condition precise: the model cannot represent \
      \"answered the requested keys and still unfinished\", so the stall is exactly the open C268 \
      question, whether the cursor advances in a run that does not finish. `MAX_IDLE_ROUNDS = 3` matches \
      the block leg — and **the page walk's own timeout is 120 s, not the 30 s that constant's rationale \
      assumes**, so a copied value means six minutes of silence. **Two Rust defects were found while \
      proving this** and are filed as their own rows: that timeout mismatch, and an unfinished walk \
      returned as `Ok`." },

  { number := 70, layer := "Sync",
    statement := "**Importing the walk of a root yields exactly the trie rooted there, and the checker \
      is sound.** The walk delivers the reachable nodes however the pages are cut; a page-wise import \
      whose pages pass their checks reads at the root what the peer reads there. The joiner is not \
      trusting the peer, it is checking it — and the whole of that rests on one assumption, \
      `blake2b256_collision_free`, through Law 10",
    status := .provedModel,
    declarations := [`Rchain.Sync.import_reads_the_rooted_trie, `Rchain.Sync.reads_determined,
      `Rchain.Sync.WalkListing.pages_are_the_nodes],
    axioms := [],
    rust := ["rspace/src/state/mod.rs", "rspace/src/history/export.rs",
      "rspace/src/history/radix_tree.rs"],
    witness := [`Rchain.Sync.import_reads_the_rooted_trie],
    falsifiable := "Two stores that agree on the reachable set and read differently at the root: \
      impossible, which is `reads_determined`; or a page whose leaves do not hash to their keys, which \
      `validate_state_items` refuses.",
    note := "**Survives attack**, with the gap named: the model's checker is deliberately **weaker** than \
      `validate_state_items` — it keeps key-honesty and coverage and drops the Rust's key-*sequence* \
      equality, its boundedness against extras, and the data (leaf-value) store. That is the right \
      direction: the Rust's stronger check implies the model's hypotheses, so soundness is not \
      compromised, and the one unproved bridge (per-chunk sequence equality implies whole-walk coverage) \
      is prose, not a theorem. `reads_determined` does not itself invoke `root_collision_free` — the \
      axiom is consumed in `import_agrees`." },

  { number := 71, layer := "Sync",
    statement := "**A catch-up window bounds what a joining node holds, so its progress does not depend \
      on the network standing still.** The frontier is monotone along any run, stalls included; an \
      answered window advances it one width; and the pending set is a function of the *window* and the \
      frontier — never of how far above it the peer's tip has run. The hash-keyed downward walk has no \
      such property: what it must hold is the whole gap",
    status := .provedModel,
    declarations := [`Rchain.Sync.run_monotone, `Rchain.Sync.answered_run_reaches,
      `Rchain.Sync.windowPending_independent_of_tip, `Rchain.Sync.gapPending_exceeds_the_window],
    axioms := [],
    rust := ["casper/src/engine/catchup.rs", "casper/src/engine/node_running.rs"],
    witness := [`Rchain.Sync.run_monotone],
    falsifiable := "A run that lowers the frontier, or a tip above it that changes what is pending. The \
      measured instances of the failures this replaces are `run-2-catchup-stall.txt` (frozen at 121, 173 \
      drops) against `run-3-windowed-catchup.txt` (the same 1200-block gap closed in 98 s, zero drops).",
    note := "**Two corrections the review made.** The module **cited the wrong row** in its header — C268 \
      where the catch-up is C267; the rows were split after it was written, and the citation is fixed in \
      this pass. And `windowPending` counts **heights** while `MAX_PENDING_BLOCKS` counts **blocks**: the \
      bridge (a window of eight heights is at most 32 blocks on a four-validator shard) is prose in the \
      module, not a theorem, so the headline is a *proxy* for the bound the receiver needs, and this cell \
      says so. The step's terminal condition is an **idealisation** — the Rust frontier is the local \
      DAG's and can exceed a peer's advertised tip — so the bound statements describe a narrower machine \
      than the Rust; the window arithmetic itself was verified correct and faithful to `catchup.rs`." },

  { number := 72, layer := "Casper",
    statement := "**A completion record brackets every durable effect.** The reset's marker may be \
      written only once *every* effect it brackets has been applied — the DAG rewind, the body purge, \
      the runtime's durable root, **and the cleanup of the derived deploy and deployer indexes**. No \
      reachable state has the record written with an effect missing, and the boot repair re-applies the \
      whole set from any half-applied state",
    status := .provedModel,
    declarations := [`Rchain.ResetState, `Rchain.resetStep, `Rchain.skippedIndexStep,
      `Rchain.resetSystem, `Rchain.a_rewound_record_implies_every_effect_applied],
    axioms := [],
    rust := ["casper/src/engine/reset.rs", "casper/src/dag.rs",
      "rspace/src/history/history_repository.rs"],
    rustWitness := ["casper/tests/reset.rs:a_crash_after_the_rewind_is_repaired_at_boot_by_the_marker",
      "casper/tests/reset.rs:the_guard_refuses_a_rewinding_whose_anchor_body_is_absent"],
    witness := [`Rchain.an_early_commit_reaches_a_record_that_lies,
      `Rchain.a_skipped_index_cleanup_reaches_a_record_that_lies,
      `Rchain.a_reset_can_complete],
    falsifiable := "Two of the port's own orders. `earlyCommitStep` records the completion after the \
      rewind and the purge but before the runtime root; `skippedIndexStep` records it with the \
      derived-index effect optional — the early return in `drop_above` when the metadata drop is empty. \
      Each exhibits a reachable `rewound` record with an effect missing, so the law was red on the code \
      as it stood, and `a_reset_can_complete` keeps it from being vacuous.",
    note := "**Why the record must bracket all four effects.** `rewind_and_purge` writes the marker when \
      the first two have returned (`casper/src/engine/reset.rs:81`), the runtime root is written \
      *afterwards* through `HistoryRepository::reset` → `validate_and_set_current_root` \
      (`rspace/src/history/history_repository.rs:337`), and the derived indexes are cleaned by \
      `BlockDagKeyValueStorage::drop_above` (`casper/src/dag.rs:410`). A crash before any of them \
      leaves the record claiming the stores agree while one does not, and the boot guard reading \
      `rewound` re-applies nothing (`reset.rs:140-151`). `reset_volatile`'s doc \
      (`history_repository.rs:346-358`) names the same class for the merge. HAZOP rows F-U13-01/02 and \
      **F-U17-11** (docs/src/spec/failure-hazop.md): the root half landed earlier; the index half is \
      B39, and its rule is that stale rows are derived from the **surviving** block set, not from the \
      list the current call happened to drop." },
  { number := 72, clause := "b", layer := "Casper",
    statement := "**A completion record is released only from the completed phase.** The catch-up \
      release — the marker is cleared once the frontier passes the anchor it rewound to — may clear a \
      record only when every effect it brackets has been applied, so a release cannot hide a partial \
      reset.",
    status := .provedModel,
    formal := `Rchain.law72b_statement,
    mutant := `Rchain.law72b_mutant,
    falsifier := `Rchain.law72b_falsifier,
    declarations := [`Rchain.releaseStep, `Rchain.resetReleaseStep, `Rchain.law72b_statement,
      `Rchain.law72b, `Rchain.law72b_falsifier, `Rchain.a_completed_reset_can_be_released],
    axioms := [],
    rust := ["casper/src/engine/catchup.rs", "casper/src/engine/reset.rs", "casper/src/dag.rs"],
    rustWitness := ["casper/tests/reset.rs:a_crash_after_the_rewind_is_repaired_at_boot_by_the_marker",
      "casper/src/engine/catchup.rs:a_rewinding_marker_is_never_released"],
    witness := [`Rchain.law72b_falsifier, `Rchain.a_completed_reset_can_be_released],
    falsifiable := "`unsafeReleaseStep` drops the `phase = .rewound` requirement, and \
      `law72b_falsifier` exhibits a reachable `rewinding` state released by it — the partial reset the \
      guarded rule forbids. The catch-up branch's condition (`frontier > marker.height`) is the `rust` \
      anchor; the marker test pins the `Rewound` semantics the release depends on.",
    note := "**The release condition is the walk's own result, not its exit reason** \
      (`casper/src/engine/catchup.rs:273-299`): a walk that ended with nothing above the anchor — a \
      silent peer, a closed door — leaves the marker, so the next boot re-enters the guard. The model \
      states the safety half: release starts only from `rewound`, so the marker cannot be cleared while \
      an effect is missing. **What it does not model:** the frontier itself is a `Nat` hypothesis, not a \
      walk; the walk's liveness is Law 71's. **HAZOP U12-02** asked when the record may be cleared; the \
      answer is this clause plus the status surface (C287), not an unbounded lifetime. **The code \
      guard is landed:** `may_release` (`casper/src/engine/catchup.rs`) clears only a `Rewound` \
      marker, pinned by `a_rewinding_marker_is_never_released`." },

  { number := 73, layer := "Casper",
    statement := "**A raised request is not spent until the act commits.** The self-reset's request cell \
      is cleared by, and only by, the step that applies the reset's effects: no reachable state has the \
      request taken with nothing applied, so a raised request is always still actionable",
    status := .provedModel,
    declarations := [`Rchain.RequestState, `Rchain.spendRequestStep, `Rchain.spendSystem,
      `Rchain.a_request_is_not_spent_without_the_act],
    axioms := [],
    rust := ["casper/src/engine/reset.rs"],
    rustWitness := ["casper/tests/reset.rs:a_reset_request_rewinds_the_stores_when_the_switch_is_on_and_does_nothing_when_it_is_off"],
    witness := [`Rchain.a_split_take_reaches_a_state_that_spent_the_request,
      `Rchain.a_raised_request_can_be_acted_on],
    falsifiable := "The port's split: taking the request and applying the effects in two statements. \
      `a_split_take_reaches_a_state_that_spent_the_request` exhibits the reachable state that admits — \
      the cell empty, nothing applied, and nothing to re-raise it in-process — and \
      `a_raised_request_can_be_acted_on` keeps the law from being vacuous.",
    note := "**The port takes first and acts second.** `act_on_reset_request` calls `take_reset_request()` \
      as its opening statement (`casper/src/engine/reset.rs:215`) and only then runs `perform_reset`, \
      which can fail before touching a store (the anchor body absent, `:246-258`). A failure there spends \
      the request with nothing done; recovery is a restart, re-derivable only because the detector's \
      count (`restore_attempts`) is persisted. HAZOP row F-U11-03 (docs/src/spec/failure-hazop.md). \
      **Fixed 2026-10-13**: `act_on_reset_request` peeks (`reset_request()`) and runs the anchor \
      pre-flight *before* the take, so a failure that destroyed nothing leaves the request pending — \
      witnessed by the reset test named above; the switch-off arm still consumes, because that is a \
      terminal configuration rather than a transient fault." },

  { number := 74, layer := "RSpace",
    statement := "**A checkpoint does not drain an overlay it has not folded.** The hot-store and native \
      overlays are folded into the trie before they are cleared, so an interrupted checkpoint loses \
      nothing it has not committed — no reachable state has an overlay empty while its changes are \
      unfolded",
    status := .provedModel,
    declarations := [`Rchain.CheckpointState, `Rchain.checkpointStep, `Rchain.checkpointSystem,
      `Rchain.a_checkpoint_does_not_drain_what_it_has_not_folded],
    axioms := [],
    rust := ["rspace/src/rspace.rs", "rspace/src/native_store.rs"],
    witness := [`Rchain.a_drain_before_the_fold_reaches_a_state_that_lost_the_overlay,
      `Rchain.a_checkpoint_can_complete],
    falsifiable := "The port's order, clearing the native overlay before the fold. \
      `a_drain_before_the_fold_reaches_a_state_that_lost_the_overlay` exhibits the state it admits — the \
      overlay empty, the changes unfolded — and `a_checkpoint_can_complete` keeps the law from being \
      vacuous.",
    note := "**The two overlays are not treated alike.** `create_checkpoint` reads the hot-store changes \
      (`rspace/src/rspace.rs:604-605`, a snapshot that is not cleared) and clears the native overlay at \
      `:606-609` (whose `drain_native` ends in `overlay.clear()`, `rspace/src/native_store.rs:477`) \
      *before* the only fallible step, `checkpoint_with_native` (`:616-621`). A fold that returns `Err` \
      therefore loses the native mutations and records the failed drain in `last_native_*`, while the \
      hot-store side is untouched. The model is one overlay because the property is per-overlay — \
      applying it to both is what \"both overlays or neither\" means. HAZOP rows F-U15-01/02 \
      (docs/src/spec/failure-hazop.md). **Fixed 2026-10-13**: `create_checkpoint` snapshots the native \
      overlay before the drain and restores it if the fold fails, and writes `last_native_*` only after \
      the fold has committed. `rustWitness` is empty: the fold's failure path has no fault-injection \
      point in the rspace tests yet, so that falsifier is owed." },
  { number := 74, clause := "b", layer := "RSpace",
    statement := "**A soft checkpoint reverts only over the base it was taken over.** The soft \
      checkpoint carries the history root it snapshotted; a hard checkpoint between the snapshot and \
      the revert makes the revert unavailable, so the old overlay is never applied over a new base.",
    status := .provedModel,
    formal := `Rchain.law74b_statement,
    mutant := `Rchain.law74b_mutant,
    falsifier := `Rchain.law74b_falsifier,
    declarations := [`Rchain.SoftState, `Rchain.guardedRevert, `Rchain.law74b_statement,
      `Rchain.law74b, `Rchain.law74b_falsifier, `Rchain.a_guarded_revert_uses_the_snapshots_base],
    axioms := [],
    rust := ["rspace/src/checkpoint.rs", "rspace/src/rspace.rs"],
    rustWitness := ["rspace/src/rspace.rs:a_soft_checkpoint_revert_over_a_moved_base_is_refused"],
    witness := [`Rchain.law74b_falsifier],
    falsifiable := "The port's `revert_to_soft_checkpoint` re-reads `history.root()` at revert time; \
      `law74b_falsifier` reaches a state where the unguarded rule used the new base. The guarded rule's \
      invariant is `a_guarded_revert_uses_the_snapshots_base`.",
    note := "**The Rust guard is landed.** `SoftCheckpoint` carries the history root it was taken over \
      (`rspace/src/checkpoint.rs`), and `revert_to_soft_checkpoint` refuses with \
      `SoftCheckpointBaseMoved` when the root moved (`rspace/src/rspace.rs`), pinned by \
      `a_soft_checkpoint_revert_over_a_moved_base_is_refused`; the old code re-read \
      `history.root()` at revert time. **What it does not model:** the history's own base derivation is \
      Law 70's trie, not this file. HAZOP U15-02's owed law is this clause; the interleaving test is \
      the Rust witness above." },

  { number := 75, layer := "Casper",
    statement := "**A dependent action is taken only on a committed record.** The refusal budget's \
      increment and the reset request it can raise are ordered: no reachable state has the request raised \
      while the record it rests on is uncommitted, so the request's premise is always durable",
    status := .provedModel,
    declarations := [`Rchain.AttemptState, `Rchain.attemptStep, `Rchain.attemptSystem,
      `Rchain.a_dependent_action_is_taken_only_on_a_committed_record],
    axioms := [],
    rust := ["casper/src/multi_parent_casper.rs"],
    witness := [`Rchain.a_raised_request_on_an_uncommitted_record_is_reachable,
      `Rchain.a_committed_record_can_gate_an_action],
    falsifiable := "The port's order, raising the request on the uncommitted record. \
      `a_raised_request_on_an_uncommitted_record_is_reachable` exhibits the state it admits — the request \
      raised, the record it rests on uncommitted — and `a_committed_record_can_gate_an_action` keeps the \
      law from being vacuous.",
    note := "**The request runs on the value the store has not yet accepted.** `revalidated_record` builds \
      the incremented `BlockMetadata` and `note_unrestorable_if_exhausted` — which raises the request — \
      runs on it before the `update_metadata` that persists it (`casper/src/multi_parent_casper.rs:796` \
      and `:1202` precede `:797` and `:1214`). The node's read-modify-write half of the same node \
      (F-U14-01) is the shape law 73 states and is not modelled twice. HAZOP rows F-U14-01/02 \
      (docs/src/spec/failure-hazop.md). **Fixed 2026-10-13**: `note_unrestorable_if_exhausted` now runs \
      *after* the `update_metadata` that persists the count, so a request raised by it names a count the \
      store holds. `rustWitness` is owed — the write's failure path has no fault-injection point here." },

  { number := 76, layer := "Casper",
    statement := "**A concluded stop stops, and a safe halt stops production without stopping service.** \
      Once a fault the node cannot serve through is reported, the node is not serving and cannot return \
      to serving: the report and the stop are one step, and it is the last step available. A divergence \
      the node cannot repair without an anchor the network has not agreed is the weaker case: proposing \
      stops, serving continues, and only an operator's act resumes it",
    status := .provedModel,
    declarations := [`Rchain.ServeState, `Rchain.serveStep, `Rchain.serveSystem,
      `Rchain.a_reported_fault_stops_the_node,
      `Rchain.HaltState, `Rchain.haltFresh, `Rchain.safeHaltStep, `Rchain.operatorResumeStep,
      `Rchain.haltStep, `Rchain.a_safe_halt_stops_production_not_serving,
      `Rchain.a_safe_halt_has_no_automatic_exit,
      `Rchain.a_safe_halt_can_be_resumed_by_an_operator],
    axioms := [],
    rust := ["node/src/runtime/shutdown.rs", "node/src/main.rs",
      "node/src/runtime/node_runtime.rs", "casper/src/engine/reset.rs",
      "casper/src/blocks/proposer/proposer.rs", "casper/src/api/block_api.rs",
      "casper/src/multi_parent_casper.rs"],
    rustWitness := ["node/src/runtime/shutdown.rs:a_reported_fatal_resolves_the_wait_with_the_first_reason",
      "casper/tests/divergence_halt.rs:a_spent_restore_budget_halts_every_proposal_trigger_and_survives_a_restart"],
    witness := [`Rchain.a_note_without_a_stop_leaves_the_node_serving,
      `Rchain.a_reported_fault_reaches_a_stopped_state,
      `Rchain.a_safe_halt_stops_production_not_serving,
      `Rchain.a_safe_halt_has_no_automatic_exit,
      `Rchain.a_safe_halt_can_be_resumed_by_an_operator],
    falsifiable := "The port's old shape, where reporting the fault leaves the node serving. \
      `a_note_without_a_stop_leaves_the_node_serving` exhibits the reachable state it admits — the latch \
      set, the listeners up — and `a_reported_fault_reaches_a_stopped_state` keeps the law from being \
      vacuous.",
    note := "**Two statements, one gap.** The self-reset's failed act and the LFS sync terminal both say \
      \"this node is stopping\", and both were a `return Err` from a task nobody joins: the launched \
      `node_launch` future's error was only logged and `NodeProgram::serve` never awaited it, so the \
      receiver, processor, proposer and API kept serving a node whose stores may have moved under a view \
      that was not rebuilt. **Fixed 2026-10-13**: `shutdown::report_fatal` sets a latch the spawn wrapper \
      writes and `main`'s wait selects on, stopping the process without draining. HAZOP row F-U12-03 \
      (docs/src/spec/failure-hazop.md). **The safe-halt half was added 2026-11-23 for F-U26-02 \
      (#658)**: a spent restore budget has no automatic exit — `a_safe_halt_has_no_automatic_exit` — \
      and the operator's acknowledgement of the refused block is the only resume. The node keeps \
      serving the chain it holds; it stops producing on it." },
  { number := 77, layer := "RSpace",
    statement := "**A reset is a projection onto its root.** The reset keeps the root it names and drops \
      the overlay, so an install made *before* a reset to that root is **unobservable** after it — and \
      the read path's construction, which installs the system processes, resets to the block's root and \
      installs them again, is therefore the **same function** as the one that resets and installs once",
    status := .provedModel,
    declarations := [`Rchain.StoreView, `Rchain.installInto, `Rchain.resetTo, `Rchain.foldingResetTo,
      `Rchain.installAll, `Rchain.readPathAsWritten, `Rchain.readPathDerived,
      `Rchain.the_first_pass_is_erased, `Rchain.an_install_before_a_reset_is_unobservable],
    axioms := [],
    rust := ["rholang/src/runtime.rs", "rspace/src/rspace.rs", "casper/src/runtime_manager.rs"],
    rustWitness := ["casper/tests/exploratory_reply.rs:a_read_makes_no_reset"],
    witness := [`Rchain.a_folding_reset_keeps_the_first_pass, `Rchain.the_two_reset_rules_differ],
    falsifiable := "The shape a *folding* reset would have — and it is not a straw man: folding an \
      overlay in rather than dropping it is what a **checkpoint** does, and Law 74 is the law that it \
      must fold what it drains. Under that rule the same pre-reset install **is** observable \
      (`a_folding_reset_keeps_the_first_pass`), so `the_first_pass_is_erased` is a claim about the \
      reset's rule rather than a restatement of a container. `the_two_reset_rules_differ` keeps it from \
      being vacuous: the two rules are distinguishable on a store with one definition installed.",
    note := "**Why this law exists, and what it is for.** The read path forks a runtime at the last \
      finalised block's post-state, `build_runtime_core` installs the 31 system-process definitions into \
      the store it builds (`rholang/src/runtime.rs:175`), and then `RhoRuntime::reset` **replaces** that \
      store (`ISpace::reset` assigns a fresh `InMemHotStore`, `rspace/src/rspace.rs:666`) and installs \
      all 31 again (`rholang/src/runtime.rs:447`). One read therefore pays the install pass twice and \
      one of them lands in a store that is discarded — 62 definitions installed to keep 31. The code \
      said so in a *comment* (\"the reset replaces the hot store, dropping the in-memory system-process \
      installs (they are not checkpointed)\"), and a fix chosen from a comment is a fix chosen from \
      whoever wrote the comment. This row makes the same content a law, so that removing the first pass \
      is **derived** — the two constructions are one function, so the change cannot alter any answer, and \
      the derivation is what the measurement is measured against (`casper::runtime_manager::\
      read_path_census`; issue #364). **What it does not claim**: nothing here is about cost. The law \
      says the constructions answer alike; it does not say keeping the pass is cheap, and only the \
      census says what removing it buys." },
  { number := 78, layer := "Casper",
    statement := "**The genesis carries a developer allocation, derived from the total the wallets \
      file allocates.** One allocation that no wallets file contains is credited at genesis — to a \
      **vesting account**, not a spendable vault: *when* it becomes spendable is Law 79 — of \
      `max(20 000 000 REV, 2% of the allocated total)`, so below a one-billion-REV allocation the \
      floor governs and at or above it the rate does, and the creator of the genesis and a node \
      replaying it derive the same amount from the same file",
    status := .provedModel,
    declarations := [`Rchain.devTaxAmount, `Rchain.devTaxFloor, `Rchain.devTaxBps,
      `Rchain.devTaxCrossover, `Rchain.devTaxAmount_ge_floor,
      `Rchain.devTaxAmount_eq_floor_of_le_crossover,
      `Rchain.devTaxAmount_eq_percentage_of_crossover_le,
      `Rchain.devTaxAmount_of_the_published_allocation],
    axioms := [],
    rust := ["casper/src/genesis/dev_tax.rs", "casper/src/genesis/mod.rs",
      "casper/src/engine/node_launch.rs", "node/src/configuration/configuration.rs"],
    rustWitness := ["casper/src/genesis/dev_tax.rs:the_floor_governs_below_a_billion_rev_and_two_percent_above"],
    witness := [`Rchain.the_rate_strictly_exceeds_the_floor_above_the_crossover,
      `Rchain.devTaxAmount_monotone],
    falsifiable := "Each arm is a theorem, and they are the falsifiers: \
      `devTaxAmount_eq_floor_of_le_crossover` says an allocation at or below one billion REV takes the \
      floor, `devTaxAmount_eq_percentage_of_crossover_le` says one above it takes the rate, and the \
      two would contradict each other if the rule were mis-stated. \
      `the_rate_strictly_exceeds_the_floor_above_the_crossover` is the non-vacuity witness — without \
      it both directions would hold of any constant — and \
      `devTaxAmount_of_the_published_allocation` pins the arm this network is on (908 962 714 REV \
      allocated, so the floor).",
    note := "**A deliberate deviation, registered as one.** The Scala ceremony installs exactly the \
      vaults its wallets file names; this port derives one more from their total, so the row is \
      `provedModel` — there is no conformance corpus for a rule the oracle does not have — and the \
      deviation is recorded in `spec/audit/passes.md` §139 with its own finding row. Three things the \
      law is *for*: the balance is installed outside the genesis block's deploys and content-addressed \
      into its post-state hash, so the arithmetic has to be one function of a file that a creator and \
      a joiner both read (AUDIT C46); only the *address* is a setting, never the amount; and the rule \
      has a home other than the comment that would otherwise be its only one. **What it does not \
      claim**: nothing here is about the allocation being owed, just, or final — those are the \
      decision this row records, taken 2026-10-16, and the arithmetic is the only part a machine can \
      check." },
  { number := 79, layer := "Casper",
    statement := "**The developer allocation unlocks on a schedule, and never runs ahead of it.** The \
      genesis credits it to a vesting account rather than a vault, and each epoch boundary moves what \
      has vested into the recipient's own vault: the vested fraction is the schedule position, linear \
      in the height, zero at genesis and total at the end, and a duration of `0` is *no schedule* \
      rather than a zero-length one",
    status := .provedModel,
    declarations := [`Rchain.devTaxVested, `Rchain.devTaxVested_zero_duration,
      `Rchain.devTaxVested_at_genesis, `Rchain.devTaxVested_at_end,
      `Rchain.devTaxVested_le_allocation, `Rchain.devTaxVested_monotone],
    axioms := [],
    rust := ["rholang/src/native_state.rs", "casper/src/genesis/mod.rs",
      "node/src/configuration/defaults.conf"],
    rustWitness := ["rholang/src/native_state.rs:the_dev_allocation_vests_linearly_and_releases_at_boundaries"],
    witness := [`Rchain.devTaxVested_le_allocation, `Rchain.devTaxVested_monotone,
      `Rchain.the_schedule_is_not_a_constant, `Rchain.the_schedule_truncates],
    falsifiable := "The bound and the monotonicity are the falsifiers, and they are what the release \
      depends on: the release moves `vested − released`, so `devTaxVested_le_allocation` is what keeps \
      the recipient's vault bounded by the allocation however long the chain runs, and \
      `devTaxVested_monotone` is what keeps that difference non-negative. \
      `the_schedule_is_not_a_constant` is the non-vacuity witness — without it every theorem here \
      would hold of `fun _ => allocation` — and `the_schedule_truncates` pins the direction of the \
      rounding, which is toward the network rather than the recipient.",
    note := "**Why this row exists, and what it is for.** Law 78 fixes *how much* the genesis owes; on \
      its own that is an allocation credited to a vault at genesis and therefore spendable at genesis — \
      an immediate claim on the whole amount. This row is the schedule that makes it not that. The \
      allocation is installed into a **vesting account** (`pos:dev-vesting`, holding the recipient, the \
      total and the unreleased balance in one record) by the same `install_genesis` both the ceremony \
      and a replaying node call, and `close_block` releases at epoch boundaries — last, and \
      deliberately independent of the reward steps, because it debits that account and credits a *user* \
      vault and so can reach neither the staking vault nor the pot. **The term is a genesis \
      parameter, not a constant** (`dev-tax-vesting-blocks`, 36 months at the cadence \
      `defaults.conf` states): the arithmetic is this row's, the term is the deal's, and the block \
      cadence is not a constant in this tree (`docs/src/node/scaling.md`), so a month is an assumption \
      the parameter makes visible. **What it does not claim**: nothing here is about the term being the \
      right one. **And the granularity is the epoch, not the block** — native state is written at \
      boundaries — which is stated because a reader comparing the schedule to a balance needs to know \
      which clock the balance follows." },
]

end Laws
end Rchain
