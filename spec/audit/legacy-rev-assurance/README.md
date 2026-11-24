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

Deployment gate: #669 reports cross-block multisig custody failure; no escrow
or vesting deployment on that custody path until its actual cross-block spend
and play/replay witnesses pass. #655/#660 are related but separate. No production
genesis changes, actual holder data, or real funds.

Run in CI: python3 test_assurance.py (from this directory). Negative controls and
finite exhaustive checks are not a universal formal proof.
