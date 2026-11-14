# The laws — what they guarantee

rholo's behaviour is pinned by **<!-- counts:laws -->103 laws<!-- counts:end -->** — one invariant per layer of
the system. A law says what must be true of the node (or of the model it is proved over), why it matters,
and what would falsify it. This page is the **reading**: one line per law, grouped by layer. The checked
register is the authority for status, declarations and falsifiers; where this page and the register
disagree, the register is right.

> **How to read a law number**
>
> - **`Law N`** — a stable number, 1–100. Numbers are never reused; a new law takes the next one.
> - **`Law Nx`** — a *clause* of a law whose parts have different proofs or statuses (`16a`, `16b`, …).
>   <!-- counts:clause-laws -->27<!-- counts:end --> laws carry clauses; the register has one **entry** per clause.
> - **Entry** — one row of the register. **<!-- counts:laws -->103 laws<!-- counts:end --> = <!-- counts:entries -->158 entries<!-- counts:end -->**, and the
>   status counts are per entry. Laws 14, 16 and 86 have clauses on both sides of `proved-tied`/`proved-model`,
>   which is why the number of *laws* proved is smaller than the sum of the two status columns.
> - **`C###`** — an audit finding (`spec/audit/passes.md`), never a law. HAZOP rows are `F-U…`/`H-U…`,
>   wargame rows `W-N…`, and an audit pass is `§N`. They are different namespaces.
>
> The machine-readable register is [`spec/laws.tsv`](../../../spec/laws.tsv); the rendered tables, with every
> law's status and falsifier, are [`spec/LAWS.md`](../../../spec/LAWS.md). Both are emitted from
> [`spec/Rchain/Laws.lean`](../../../spec/Rchain/Laws.lean) and refused stale by the gate.

## The set at a glance

| Laws | Layer | What it covers |
|---|---|---|
| 1–6 | **Rholang** | the calculus: canonicalization, α-equivalence, substitution, reduction, matching, closedness |
| 7–11 | **RSpace** | the tuple space: joins, the comm event, merge, the Merkle trie, replay |
| 12–13 | **Rosette** | the actor VM — orphaned, out of scope for this port |
| 14–19 | **Casper, storage, crypto** | finality and the fringe, the DAG, block validity, merge determinism, the height map, the primitives |
| 20–25 | **Scheduler** | effect scheduling: the claim queue, the DFS gate, on-chain validated speculation |
| 26–29 | **Cross-shard** | two-phase commit: scope, atomicity, idempotency, the durable decision |
| 30–43 | **The surface** | what a client writes and a matcher reads: grammar, lexing, normalization, matching, replies, JSON |
| 44–47 | **Proof of Stake** | the epoch gate, the reward split, its dust, staged withdrawal |
| 48–49 | **Charging** | a denied deploy's fee, and what a matched deploy costs |
| 50 | **Depth guards** | the AST and value depth budgets |
| 51–54 | **Progress** | the shapes and causes of non-progress, the finality gate's two answers, terminal refusal, which view a rule reads |
| 55–56 | **Bounds and names** | bounded work per step; a rooted namespace |
| 57–58 | **Delegation and participation** | a delegator's stake; the gate publishes on a fully-participating set |
| 59–65 | **Wire and sessions** | the CapTP bridge, install totality, the session loop, dial policy, attribution, interop, literal printing |
| 66–71 | **Distributed state and sync** | the record key and join, versioned reads, the page walk, import, the catch-up window |
| 72–77 | **Durability** | the reset bracket, its request, checkpoints, dependent actions, the stop, the reset's projection |
| 78–79 | **Genesis** | the developer allocation and its vesting schedule |
| 80–85 | **The join and the chain's identity** | root publication, the verified-answer join, the validator lifecycle, the trust model, the genesis anchor, the fringe state |
| 86 | **Admission and the pool** | the pool is the throttle: a bounded queue, no silent loss, independent work in parallel, one permit progresses, pending work relays |
| 87 | **Decimal — retired** | the exact value algebra was **removed before mainnet** (issue #527); the clause rows are `retired` in the register and the record is [the decimal type, withdrawn](decimal-removed.md) |
| 88–90 | **Disposition, liveness, hostlist** | a stored body is accounted for and the proposal gate is total in its anchor (88); jail before any vote reaches a bond (89); the published hostlist is earned, bounded and fresh (90) |
| 91 | **The cost model** | a canonicalisation and a union charge their elements, `censure` its cubic fold, a tally its option lists; the register and the census that keep the shapes honest are [here](cost-model.md) |
| 92–97 | **Replay evidence and split accounting** | a cost verdict is not a state verdict (93) and a replay failure's cause is provenance-dependent (92); a local fault has a bounded exit (94); the replay record is the play's order (95); a split execution does not change the accounting (96); a merge's accounting is total and single-counted (97) |
| 98–100 | **Cross-shard acceptance** | the escrow moves exactly and conserves value (98); the decision is durable before its effects and recovery closes (99); every phase is a shard-local deploy and the DAG is single-shard (100) |

## Rholang — the language (Laws 1–6)

| Law | What it guarantees |
|---|---|
| 1 | `Par`/`ESet`/`EMap` are commutative; canonical sorting is idempotent and commutes with composition. |
| 2 | α/name equivalence: par order, `\| Nil`, top-level arithmetic, α, and quote/eval. |
| 3 | Capture-avoiding substitution; sorting commutes with it, and closed terms stay closed. |
| 4 | COMM: a send and a matching receive reduce to the receive's body; `new` is fresh; the first match wins. |
| 5 | Spatial matching; a free variable is bound at most once (linearity, enforced before and at the matcher). |
| 6 | A program has no globally free variables. |

## RSpace — the tuple space (Laws 7–11)

| Law | What it guarantees |
|---|---|
| 7 | A join key hashes its channels in sorted order, so it is invariant under permutation. |
| 8 | COMM is deterministic: candidates and produce refs are sorted by content hash, so the trace is content-addressed. |
| 9 | Merge is a monoid, and non-conflicting logs commute — disjoint **closure** is enough. |
| 10 | The Merkle trie is content-addressed and collision-free on well-formed nodes, with one empty root. |
| 11 | The replay check holds exactly when recomputed and recorded COMMs agree on occurrences. |

## Rosette (Laws 12–13)

The actor VM is **orphaned** — `rosette`/`roscala` is out of scope for this port. The rows are kept so the
catalog does not silently shrink.

## Casper, storage, crypto (Laws 14–19)

| Law | What it guarantees |
|---|---|
| 14 | The fringe advances iff its supporting stake is a strict supermajority (`3·s > 2·t`) of the **whole bonded** stake. |
| 15 | The fringe is monotone by height and the seen set never regresses (under the ingress rules the row names). |
| 16 | Block number = max(parent)+1, seqNum strictly +1, content addressing, and the bonds cache is the PoS state. |
| 17 | Merge is deterministic: the rejection is the unique minimum of `(cost, size, set)`, over checked arithmetic. |
| 18 | The height map has no holes; fringe identity is order-independent. |
| 19 | Blake2b256 is canonical; the RNG merge is n-ary and order-sensitive; signatures verify and Curve25519 round-trips. |

## The scheduler (Laws 20–25)

| Law | What it guarantees |
|---|---|
| 20 | Same-channel operations commit in DFS path order through a per-channel claim queue; the head is path-smallest. |
| 21 | The DFS gate refines sequential effect application; one-hop next-step pruning is unsound. |
| 22 | The next-step closure is computable at dispatch — but that does not make cross-channel pruning sound. |
| 23 | An effect's candidate, outcome and trace are a deterministic function of the state it reads. |
| 24 | A concurrent execution is sound iff every commit read the state the DFS-earlier effects produced. |
| 25 | Effects may reorder only if each commit validates Law 24; otherwise the run falls back to the gate. |

## Cross-shard (Laws 26–29)

| Law | What it guarantees |
|---|---|
| 26 | A deploy/block binds to exactly one shard, and the gateway admits only a validated shard id. |
| 27 | Two-phase commit: every leg that prepared reaches the coordinator's single decision. |
| 28 | The leg verbs are idempotent under the transaction id; terminal verbs refuse a reversal. |
| 29 | The decision is a function of the votes and a durable record a prepared participant can recover. |

The acceptance language these four rows leave open — that the escrow moves exactly once and conserves
value, that the decision is durable before its effects and recovery closes, and that every phase is a
shard-local deploy — is Laws 98–100 ([below](#cross-shard-acceptance-laws-98100)).

## The surface a client writes and a matcher reads (Laws 30–43)

| Law | What it guarantees |
|---|---|
| 30 | Every accepted term is in the grammar, modulo Law 31's documented deviations. |
| 31 | Every grammar production is accepted, modulo that data list. |
| 32 | Each lexical construct (comments, `_`, `bundle0`, numbers, operators) lexes one way. |
| 33 | Printing then parsing returns the same `Par` (`parse (print p) ≡ p`). |
| 34 | A value position normalizes against an empty par; only a statement continuation inherits what precedes it. |
| 35 | `connective_used` is sound: connective, free variable, wildcard or remainder, per collection form. |
| 36 | The normalizer's output is well-scoped and closed. |
| 37 | Matching is sound and complete over the shapes the clauses cover. |
| 38 | Silence is specified: an unmatched receive or `match` is no step and no error. |
| 39 | Every `rho:*` urn's reply arity and shape equals its `spec/API-SCHEMA.md` row. |
| 40 | Every call in the protocol catalog has an accepting receive at the target's arity. |
| 41 | A replicable reader restores what it consumes (channel balance). |
| 42 | The JSON round trip, including the `0 → absent`, `1 → unwrapped`, `n → ExprPar` envelope rule. |
| 43 | Each endpoint's serialized shape equals the schema's: camelCase keys, `[]` semantics, error precedence. |

## The validator lifecycle — the Proof-of-Stake epoch (Laws 44–47)

| Law | What it guarantees |
|---|---|
| 44 | Membership moves only at an epoch boundary; off one, a bond is pooled but not activated and a withdrawal is staged but not moved. |
| 45 | The epoch split: `pot · (bondᵢ / minBond) / (activeBonds / minBond)` per active validator. |
| 46 | The split does **not** conserve: `Σ rewards ≤ pot`, and the dust stays in the pot for the next epoch. |
| 47 | A withdrawal stages a deadline, is escrowed out of the pool at the next boundary, and is paid `bond + committed` after quarantine. |

## Charging (Laws 48–49)

| Law | What it guarantees |
|---|---|
| 48 | A denied deploy's effects are excluded from the merged state, but its cost still counts in the merge objective — RCHIP-02's refund is not implemented. |
| 49 | For a matched deploy, storage is charged up front and the consumed storage is refunded before the event and COMM costs. |

## Depth guards (Law 50)

| Law | What it guarantees |
|---|---|
| 50 | A term's AST depth is bounded (768) and a runtime value's pair-depth is bounded (256): the parser refuses the first, the space refuses the second. |

## Progress — the shapes of non-progress (Laws 51–54)

| Law | What it guarantees |
|---|---|
| 51 | Non-progress has three shapes (void requirement, terminal refusal, drift) and two causes (a historic predicate, two split readers). A time-gated action (51d) consumes its permit: a banked permit fires on every tick rather than pacing. |
| 52 | Finality's denominator is the whole bonded stake; only the partition may shrink — two disjoint groups cannot both be a strict supermajority. |
| 53 | An attribution is terminal: the refusal persists and the rule refuses the validator's children, until a restoring rule or a rewind removes the state. |
| 54 | A liveness predicate reads the current view, not a history; and two views of one set written in separate steps can split their readers. |

## Bounds, names, delegation, participation (Laws 55–58)

| Law | What it guarantees |
|---|---|
| 55 | The guard is before the work: a step's cost is bounded by a quantity the protocol bounds, and a spent budget carries no partial answer. |
| 56 | A name is rooted in the caller's derived identity; versions append; `seal` closes a path. |
| 57 | A delegator's stake is attributed to the operator's key; rewards split pro-rata; a slash reaches it; unbonding is staged. |
| 58 | A fully-participating view is the whole bonded set, so the finality gate publishes (the forward dual of Law 52). |

## The wire and the session (Laws 59–65)

| Law | What it guarantees |
|---|---|
| 59 | The CapTP value bridge round-trips its domain, is injective, and never emits a lossy shape (the tuple is tagged). |
| 60 | An install is total on a channel: replacing a different continuation is refused; an identical re-install is a no-op. |
| 61 | A delivery in flight does not stall the session loop, and a deferred answer is still written. |
| 62 | A peer cannot extend this node's reach: the dial policy judges the peer's origin, not only the target. |
| 63 | A bridged delivery's consensus footprint is attributable to a payer. **Open** — the close is a cross-implementation relay. |
| 64 | Where references disagree, the port speaks the implementations' reading and accepts every reading they produce. |
| 65 | The printer refuses a value it cannot write as a faithful literal. |

## Distributed state and sync (Laws 66–71)

| Law | What it guarantees |
|---|---|
| 66 | The value stored at a fringe key is not determined by that key: the merge's base is part of the claim. |
| 67 | The record join is a commutative idempotent semigroup at one key — and not a monoid. |
| 68 | A read is a function of the version it names; that needs an append-only store (or content addressing). |
| 69 | A state-page walk need not terminate as written; a give-up rule makes it paced and bounded. |
| 70 | Importing the walk of a root yields exactly the trie rooted there; the page checks are sound. |
| 71 | A catch-up window bounds what a joiner holds, so progress does not depend on the peer standing still. |

## Durability (Laws 72–77)

| Law | What it guarantees |
|---|---|
| 72 | A completion record brackets every durable reset effect: rewind, body purge, runtime root and derived indexes. |
| 73 | A reset request is not spent until the act commits. |
| 74 | A checkpoint folds an overlay before it drains it. |
| 75 | A dependent action is taken only on a committed record. |
| 76 | A concluded stop stops: the report and the stop are one step, and it is the last. |
| 77 | A reset is a projection onto its root: an install before it is unobservable after it. |

## Genesis (Laws 78–79)

| Law | What it guarantees |
|---|---|
| 78 | The genesis credits one developer allocation derived from the wallets file's total: `max(20 000 000 REV, 2% of it)`. |
| 79 | The allocation vests linearly, never ahead of schedule; `0` means no schedule. |

## The join and the chain's identity (Laws 80–85)

| Law | What it guarantees |
|---|---|
| 80 | `CURRENT_ROOT` names only an accepted block: a candidate post-state is a *known* root until its block is accepted. |
| 81 | A join walk accepts a verified answer for any **known** key and re-asks every outstanding key — a refused body cannot strand one. |
| 82 | The validator lifecycle has six transitions; trust is admission, not membership; revocation does not confiscate. |
| 83 | The join is **content-trustless, not finality-trustless**: every byte is checked against a hash, and the assumed inputs are enumerated. |
| 84 | The genesis anchor is a function of the genesis inputs: the first bond signs and is the sender, and any other key refuses to create a genesis. |
| 85 | The finalized fringe's root is a **claim about a fringe** (the one a member finalises), not a field of the named block — so it cannot be pinned to that block's pre/post-state. |

## Admission and the pool (Law 86)

| Law | What it guarantees |
|---|---|
| 86a | Admission is a bounded queue: a free slot admits, a full pool refuses with a reason; the memory word is not an input to the decision. |
| 86b | No admitted work is silently lost: a step advances a status and never removes the row; a refusal changes nothing. |
| 86c | Independent work commutes and may run concurrently (disjoint **closures**, not footprints); conflicting work serialises. |
| 86d | One permit progresses; the permit budget bounds the batch, never the admission answer. |
| 86e | Pending work is relocatable: a relay copies to a peer and leaves the sender's row intact. |

## Decimal (Law 87) — retired

**Rholang has no decimal value type.** The exact fixed-point decimal added on 2026-10-19 was removed
before mainnet on 2026-10-31 (issue #527, option R): it was isomorphic to a scale-38 integer, it could
not carry a ratio (`1d / 3d` was an error), no contract in the tree used it, and its wire tag and sort
rank were irreversible commitments. The five clauses are `retired` in the register — kept, not
deleted, so the decision travels with the number — and the archived algebra is
`spec/Rchain/Decimal.lean`. What was built, why it went, and what a future attempt must do differently:
[the decimal type, withdrawn](decimal-removed.md).

## The node hostlist (Law 90)

| Law | What it guarantees |
|---|---|
| 90a | A served entry is earned: the peer answered a probe of its derived client endpoint, named itself, matched this network and a member shard, and reported `proposer: true`. An observer, a silent peer or another chain's node is never served. |
| 90b | The list is bounded, deduplicated and ordered: at most `maxEntries`, one entry per node id, self first when it can propose, the rest ascending by node id. |
| 90c | The list is fresh: it is derived only from the current probe cycle, so a peer that stopped answering disappears on the next cycle. |

## The cost model (Law 91)

| Law | What it guarantees |
|---|---|
| 91a | A canonicalisation charges the elements it canonicalises — three phlo per element of the output it is about to build, computed from the operand before the work — and a union charges **both** operands, so the charge does not depend on which operand is which. |
| 91b | `rho:gov:censure` charges `(U+1)³` for a member universe of `U`: a charge that covers every member and grows with the universe, bounding the fold's `U+1`-round fixed point. |
| 91c | A tally charges the option lists it scans: a ranked tally the worst case of one scan per entry per round, an approval tally every approved entry. The member maps keep the universe bound; the lists are priced, not refused. |

## Split execution and its accounting (Laws 95–97)

| Law | What it guarantees |
|---|---|
| 95a | The replay record is built in the play's execution order: RSpace's newest-first log is walked in reverse, so a consume's candidate list is the play's own sequence. |
| 95b | The inverted build changes the binding and the cost: the later request's COMM is offered first, the earlier request is suppressed, and the replayed cost differs (the 5576/5302 class). |
| 96a | Verification is not optional: no effective batch policy has an enabled batch with verification off. |
| 96b | A certified split equals the sequential fold on per-deploy cost, COMM log, failure flag and post-state; an unverified split can change all four (C360). |
| 96c | Concurrent validation of a dependency-free block batch agrees with serial validation, because each block replays on its own fork at its committed pre-state; a shared runtime would not. |
| 96d | A cross-node verdict disagreement is not published: only agreement is reconciled; a difference is attributed and bounded, never silently published. |
| 97a | Every conflict-scope chain is classified (kept, rejected with a reason, or final-scope rejected); every rejection carries a non-zero reason. |
| 97b | No kept effect is silently lost: the coverage check is multiset-sensitive, so a duplicate occurrence cannot pass as a set. |
| 97c | A cost move is applied at most once; a seen slot is skipped and an overdraft is refused, not invented. |
| 97d | Independent chains commute (Law 86c's state half) and their charges add (the cost half). |

## Cross-shard acceptance (Laws 98–100)

The cross-shard protocol's shape is Laws 26–29. These rows carry the **acceptance language** that
shape leaves open: what the escrow verbs do to balances, the ordering between a decision and its
effects, and the shard-local scope that makes the rest safe. There is no M1–M6 gate for cross-shard
transactions; acceptance is the register plus the resolution of issue #33.

| Law | What it guarantees |
|---|---|
| 98a | A prepare debits the payer by exactly the amount, leaves the payee untouched and records the escrow; the payer + payee + escrow total is unchanged. |
| 98b | A commit credits the payee exactly the escrow; an abort refunds the payer exactly it; each releases the escrow, so the total is unchanged. Commit is not a mint and abort is not a second payment. |
| 98c | A multi-shard instance applies one decision to every leg and its totals are the legs': committed credits sum, aborted refunds sum, and a mixed decision is the failure shape. |
| 99a | No phase-two effect without a terminal decision: an undecided record applies nothing; a decided one applies the one verb to the legs that voted ready. |
| 99b | Every drive reaches a terminal state, including the no-legs abort; a record cannot be left in flight by a failure to decide. |
| 99c | A terminal record is returned untouched and contacts nobody, so a retry or restart cannot move funds again. |
| 99d | A resume never re-prepares a leg that already voted; votes are monotone, so a crash mid-phase-one cannot escrow twice. |
| 100a | Membership is admitted before a record opens: an empty or foreign leg list opens no record and submits no deploy, and every opened record names only members. |
| 100b | A phase is anchored at the target shard's head; an unreadable head refuses the phase (it cannot vote ready), and a genesis anchor is born expired once `head ≥ lifespan` (C166/C64). |
| 100c | Block and deploy ingress is shard-local, so every accepted block and every merge input is one shard — the reason the shard-blind DAG and Law 97's accounting are safe per shard. |
| 100d | A deploy is dispatched to its named member or refused with the membership, never relayed; a keyed lookup probes the primary first. |

## Proved, assumed, and open

The register classifies every entry; the words mean different things:

- **proved-tied** — proved, *and* tied to the node by a conformance corpus the Rust consumer runs.
  <!-- counts:proved-tied-entries -->31<!-- counts:end --> entries.
- **proved-model** — proved about a model that a person keeps in sync with the Rust.
  <!-- counts:proved-model-entries -->113<!-- counts:end --> entries.
- **axiom-by-design** — a primitive assumed rather than proved (cryptography): <!-- counts:axioms -->10 axioms<!-- counts:end --> across the tree.
- **owed / deferred** — stated, with the proof or the decision outstanding; **open** — a recorded design
  question, not a proof gap; **vacuous** — proved but restating its own definition (kept visible, never
  counted as coverage); **orphaned** — out of scope (the Rosette rows); **retired** — the port has no
  rule of that shape, evidenced by naming the code that was read.

The count arithmetic, in one sentence: **<!-- counts:laws -->103 laws<!-- counts:end -->, <!-- counts:entries -->158 entries<!-- counts:end -->**, because <!-- counts:clause-laws -->27<!-- counts:end --> laws
carry clauses; **<!-- counts:proved-laws -->95<!-- counts:end --> laws are proved in at least one clause**, split over entries as
<!-- counts:proved-tied-entries -->31<!-- counts:end --> tied and <!-- counts:proved-model-entries -->113<!-- counts:end --> over the model. The two status columns overlap on
laws 14, 16 and 86, which is why they do not add up to the law-level union. No total on this page is
hand-written: the spans are emitted from the register.

## Where the detail lives

- [`spec/LAWS.md`](../../../spec/LAWS.md) — the generated tables: every entry's status, declarations,
  axioms, witnesses and falsifier. **The authority.**
- [`spec/INVENTORY.md`](../../../spec/INVENTORY.md) — the long-form catalogue with the Rust realization per
  law. Its hand-written table currently ends at **Law 77**; the register covers 1–100, and where the two
  disagree the register is right.
- [The laws → Rust code](../contributor/laws-to-rust.md) — per-law Lean/Rust anchors and tests, scoped to
  Laws 1–29 by design.
- [Laws reference for AI agents](../ai/laws-reference.md) — the register's columns, status vocabulary,
  commands and the Lean-first protocol.

The proof scripts are in [`spec/Rchain/`](../../../spec/Rchain/) (Lean) and [`spec/coq/`](../../../spec/coq/)
(Coq). The gate that keeps this page honest — every marker, every corpus, every witness — is
`tools/check-lean-conformance.sh`.
