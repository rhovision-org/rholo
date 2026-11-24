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

## Phase III — authenticated decision and atomic local settlement witness

This separate executable research adapter uses a domain-separated canonical
JSON decision, exact manifest/policy/network/authority binding, immutable
canonical-source membership, validity interval, and a quorum of distinct
Ed25519 signatures verified by OpenSSL. Keys and funds are synthetic.
The independently pinned configuration commits to the verified Phase I
manifest, unresolved registry, reserve and authority set. No real authority
is authenticated simply by constructing that configuration.

A SQLite BEGIN IMMEDIATE transaction commits the reserve debit, beneficiary
ledger credit, unique source consumption and full signed receipt together.
The full audit rechecks receipt signatures and reserve conservation. Tests
include concurrent duplicate claims, competing reserve debits, process death
before/after commit, domain and signature tampering and refusal controls.
A crash after commit is recoverable through a persisted receipt; retries
cannot pay the same source twice. This is a single-host database witness,
not a distributed finality or on-chain custody guarantee.

Integration references (inspected, not integrated):
- RhoGov main f93afaaf: src/chain/gov.ts derives outcomes from on-chain facts
  and derives write identity from the signing deployer. A finalized Issue
  result and decision authorization must be mapped into the envelope through
  a separately verified adapter; a quorum signature is not itself a vote.
- Rholo #674 merge e63b7e5a: examples/devrewards/Treasury.rho and Settlement.rho
  use install-time distinct attestors, secp verification and private vault
  payment. The Ed25519 research wire is not that contract's wire format.
  Actual legacy custody and atomic debit/consumption need their own witness.
- Quantum OS evidence export and QLF bridge are future work.

The pinned trust configuration, finalized execution height and registry
are external inputs. Policy rotation, appeals, chain reorganization, hostile
database administrators, power-loss/storage faults and external payment
atomicity are outside this model. No Genesis change or approved monetary
policy is implied. Results require CI confirmation.

## Phase IV-A — narrow disposable real-chain witness

The new live_governance.mjs harness installs the actual RhoGov main
Issue/4 source (f93afaaf), signs two distinct votes using the pinned Quantum
OS client and uses the shipped RNode executable from Quantum OS 766c782.
The node source version and executable hash are retained; this is not
automatically a test of current Rholo dev e63b7e5a. All writes target only
127.0.0.1 on an isolated Actions runner with public disposable fixture keys.

The one-claim gate fixes the exact claim title (canonical JSON), manifest,
policy, amount, beneficiary, issue URI, opener, group and two-voter roll.
It ignores opener-written results and requires closed status, exact ballots,
no guests or roll edits. The proposed result before voting is a refusal
control. The independent reader obtains a last-finalized block and requires
an exact block-hash response for every snapshot. The gate uses a fresh
private unforgeable vault and resolves its handle at payment time. The
target checks are a 100-unit reserve, one 70-unit transfer, consumed flag
and cross-deployer replay refusal with unchanged balances.

This is a deliberately narrow fixed unanimous-vote policy, not RhoGov's
general weighted/liquid-democracy tally. The installed Issue/4 admin can
load state; trusted installation and immutable registry provenance are
assumptions. On-chain gate checks current facts, not a cryptographically
verified historical-finality certificate. The off-chain receipt is an
observed block-pinned artifact from the same node, not independent
consensus proof. Single-validator, one-claim execution does not prove
multi-validator atomicity, arbitrary claim registry, crash recovery,
reserve insolvency handling, or general refinement of Phase III.
No production policy or real funds. No success is claimed before CI.
