# #412 — Legacy REV assurance: a policy-bound reference witness

**Status: research proposal, synthetic data, no production authorization.**

Separate entitlement verification from release policy. Group only independently
established common ownership; no address clustering, no allegation of wrongdoing,
no developer exemption. A threshold is a policy input, not a fact. The example
uses a uniform immediate allowance and vests the excess by finalized height.
Existing Law 79 (spec/Rchain/Vesting.lean) proves the corresponding developer
schedule arithmetic; it does not prove this model or authorize its application
to legacy holders.

The maintainer selected option B on 2026-11-15, leaving the claims authority and
cut-off open. This model illustrates a prefunded reserve; its size is illustrative
and intentionally insufficient for every synthetic unresolved claim. It never
silently accepts or zeros unresolved claims. Reserve funding and supply policy
must be explicitly approved. Coop custody migration is a distinct manifest rule,
not implemented here.

Invariants: unique canonical source positions; accepted value equals immediate
plus vested allocation; unresolved liabilities excluded from issued supply;
canonical permutation-invariant manifest; policy binding; bounded monotone
vesting; repeated same-height release zero; settlement consumes one entitlement
and debits an existing reserve. All values are integer atomic units.

Trust boundary: verified flags, canonical source identity, owner grouping,
policy approval and expected decision digest are supplied by trusted external
adjudication. SHA-256 binds bytes; it does not authenticate decisions. The
sequential model is not concurrent or on-chain. A real implementation needs
verified signatures/quorum, appeal and conflict-of-interest rules, atomic
settlement, canonical historical extraction and independent data validation.

Deployment gate: #674 reports the #669 cross-block custody fix and devnet
witnesses for developer rewards. Those do not establish a legacy claims or
vesting contract. Separate production-path custody, replay and atomic settlement
witnesses are still required. #655/#660/#674 are related but separate. No production
genesis changes, actual holder data, or real funds.

Run in CI: python3 test_assurance.py (from this directory). Negative controls and
finite exhaustive checks are not a universal formal proof.


## Phase 2 — formal research kernel

`LegacyREV.lean` is a standalone Lean 4.12 model, not a registered Rholo law.
It specifies per-owner allocation conservation, portfolio conservation,
bounded monotone vesting and an abstract sequential settlement transition.
Successful settlement requires exact claim membership in an immutable
registry, a matching manifest identifier, an external approval predicate,
an unused canonical source identifier and adequate reserve. It marks the
source consumed, retains prior consumption and conserves the reserve ledger
over every reachable sequence. Refusals return no successor. The state is
abstract; no claim about distributed atomicity or recovery is inferred.

The schedule uses the piecewise arithmetic of Law 79, while restricted
release rejects duration zero. The independent Lean executable emits a
finite allocation and vesting corpus; compare_lean.py requires complete
coverage and exact agreement with the existing Python model. This is
cross-language testing, not a general proof of Python refinement.

The identity-splitting counterexample demonstrates why address thresholds
cannot be treated as beneficial-owner thresholds. Approval is an externally
provided predicate, not authenticated by a Boolean or a digest; the formal
kernel does not identify legitimate owners or authenticate historical
evidence. It models a single fixed manifest and registry, not policy
rotation, appeal, reorganization, concurrency or actual RhoGov authority.

No results are claimed until CI succeeds. No production law, custody path,
real balance or governance policy is changed.
