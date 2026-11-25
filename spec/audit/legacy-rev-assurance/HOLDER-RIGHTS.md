# Legacy REV: holder rights, claims and appeals (#412)

**Research proposal for review — not an adopted policy or an active claims service.**
This follows the [Option B decision of 15 November 2026](https://github.com/rhovision-org/rholo/issues/412#issuecomment-6892935283) and [Patrick's review of #675](https://github.com/rhovision-org/rholo/pull/675#pullrequestreview-6064824793). Verified legacy losses are to be addressed through a post-mainnet claims process. Unverifiable entitlements remain unresolved, neither zeroed nor accepted.

## 1. Who is owed what, and how can I check?

Use the historical ledger state at a governance-approved finalized cut-off, with authenticated reconstruction where necessary. Publish the network, height, block/state identifier, evidence manifest and known gaps. If finality or coverage cannot be established, say so; do not substitute a convenient snapshot.

For each entitlement, count the verified historical liquid, staked, locked and unbonding positions once, using their actual legacy accounting semantics. Apply the adopted conversion rule, then subtract the corresponding migration credit and any earlier restitution. A negative result is an accounting discrepancy to investigate, not a negative award. No address clustering or presumed common ownership.

Provide a read-only public-address lookup showing the supported historical amount, amount already credited, outstanding verified amount (if known), unresolved portions, source references and the applicable rule version. A missing record must not appear as a zero entitlement. Checking an address never requires a private key or recovery phrase.

Example (illustrative REV, not actual allocations): 100 supported historical REV, 70 already migrated, 10 previously restored: 20 outstanding under a hypothetical 1:1 conversion. If only 80 of the historical 100 can be substantiated, show the unsupported portion separately; do not present 100 as verified or extinguish the remaining claim.

Preserve the [31 October Coop custody decision](https://github.com/rhovision-org/rholo/issues/412#issuecomment-6542175563): eligible Coop balances move to fresh custody addresses, legacy Coop addresses carry zero in the mainnet manifest, and the mapping is auditable. A legacy Coop key alone must not authorize a second allocation.

## 2. How do I claim, and what if I disagree?

1. Check the address and submit a correction or missing-entitlement claim through the published channel, with a destination address and supporting references. Receive a stable case ID and acknowledgement.
2. Demonstrate ownership with a fresh, case-bound signature where appropriate. For lost or compromised keys, use a separately approved alternative evidence process; possession of a compromised key alone is insufficient. Never submit secrets.
3. The reviewer checks the historical evidence, ownership and previous credits against the canonical entitlement source. Duplicate submissions link to the same case or underlying entitlement; they do not create additional rights.
4. Receive a reasoned outcome: **verified** (amount established), **unresolved** (specific missing or conflicting evidence), or **rejected** (specific rule and reason). Verified is not the same as authorized or paid. A verified amount awaiting funding remains visible as unpaid.
5. Challenge the amount, evidence or decision using the same case ID. The appeal must be heard by a reviewer independent of the original decision and free of conflicts. Preserve the original decision and append the reasoned appeal outcome; do not overwrite history. Disputed amounts remain unpaid until the applicable appeal rule permits settlement.

Record the paid amount and finalized transaction reference when settlement actually occurs. A holder can receive an intelligible status update throughout. Publish reasonable response and appeal windows, accessible notice and procedures for late/new evidence before opening the process. Failure to check a wallet or file promptly must not silently erase a supported right; any limitation requires explicit governance adoption.

## 3. Who decides, under which rule, and who checks the decision?

Governance appoints the accountable process owner, evidence custodian and decision/appeal authorities. Publish the versioned eligibility, evidence, conversion, release, quorum, conflict-of-interest, appeal and funding rules before accepting binding decisions. Apply the same rules to comparable claims; record and explain any lawful exception. Recuse anyone with a conflict; provide a replacement under the published rule.

For every decision keep a durable record of case ID, canonical entitlement reference, amount and prior credits, cut-off and evidence manifest references, policy version, outcome and reasons, authorized decision reference, appeal status, and any settlement receipt/finalized block. Give the claimant the reasons and evidence relevant to their case. Public audit records expose only information needed for accountability; keep personal or sensitive ownership evidence with the authorized custodian. Use aggregate reporting or non-identifying references where linking records would expose private information.

The decision authority establishes entitlement and authorization under the adopted rules. The custodian verifies sources but cannot unilaterally pay. Settlement checks the exact authorized amount, destination, reserve and prior consumption; it must not pay twice. RhoGov is a candidate decision-record mechanism, not an appointed authority by virtue of this document. Any partial settlement or appeal adjustment requires an explicit accounting rule and traceable remaining liability; the existing one-consumption research fixture does not implement that lifecycle.

## Before opening claims

The following remain governance decisions, not engineering defaults:

| Required decision | Status |
| --- | --- |
| Historical cut-off, authoritative sources and data/evidence custodian | Open |
| Eligibility, conversion and any release restrictions | Open |
| Process owner, decision authority, quorum and conflict rules | Open |
| Claim channel, notice, evidence standard, response and appeal windows | Open |
| Reserve funding, supply accounting and settlement authority | Open |

The Option B launch decision is unchanged; this checklist gates reliance on the claims service, not mainnet launch. Keep #412 open.

## Evidence and smallest next gate

[The #675 research witness](https://github.com/rhovision-org/rholo/pull/675) and [its assurance notes](README.md) support accounting conservation, scoped authorization and replay refusal. The disposable single-validator witness and offline N4 controls do not establish real holder entitlement, actual governance authority, an appeal mechanism or distributed settlement safety.

Before any live service, walk a synthetic holder through three cases: a supported unpaid balance (including previous migration credit), a missing/conflicting record followed by an appeal, and a duplicate submission/retry. Each must produce a readable status and reason, preserve prior records, and never authorize a second payment. An appeal correction must not settle against stale authorization.

For the next separately authorized disposable N4 gate, reuse the existing harness and synthetic reserve-contention matrix: check authorized settlement, refusal and replay, including restart/rejoin, against consistent finalized per-node balances and consumed-source state. Do not claim success from offline evidence alone. No new economic model, public-validator fault injection, real funds, production authorization or Genesis changes are proposed here.
