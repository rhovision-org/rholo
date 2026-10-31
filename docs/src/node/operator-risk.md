# Validator risk, hardware and participation

> **Read this before you bond.** A validator is a bonded, slashable role. Slashing is behavioural: an
> invalid attributable block or a proven equivocation can cost a stated share of your bond, accrued
> rewards and escrowed withdrawal claim. A merely absent or slow validator is **not** slashed under the
> shipped protocol, but it holds finality back, still counts in the quorum denominator, and may still be
> paid. The legal/operator notice is [`TERMS.md`](../../../TERMS.md).

This page is the operator's risk view: what the protocol can do to stake, what it does not do, how much
hardware a validator actually needs, and where a small operator can participate without becoming a
network risk. It is an estimate and a runbook, not a warranty or a promise of rewards.

## 1. The two failures that matter

A validator can fail in two different ways, and the protocol treats them differently.

| failure | current treatment | what it does to the network |
|---|---|---|
| **invalid block / equivocation** | behavioural slash, graded (`Malicious`, `Misdemeanour`, `HonestMistake`); the block's own work is re-derived by every node | a safety/attribution event; the sender's stake is at risk |
| **absent / slow / stuck** | **no slash** under the shipped protocol; the bond stays, the stake stays in the whole-map denominator, and the default `absence-slack = 0` still pays a drawn validator | a liveness drag: enough absent stake stops finality; it does not by itself split the chain |

An overloaded or buggy node can produce an invalid block, so **hardware and software health are part of
the slash risk** even though silence alone is not. The inverse is also important: a node that is merely
offline is not a thief, and a protocol that treated it as one would punish an honest partition or a
sleeping laptop. The current design chooses the asymmetry deliberately; see
[validator economics](validator-economics.md).

## 2. Hardware: per validator, not per host

The minimum is **per validator**, because every node on a host competes for the same memory and disk:

| | CPU | RAM | disk | network |
|---|---|---|---|---|
| minimal validator | 2–4 vCPU | 8 GB | 100–250 GB NVMe | 10 Mbit+ stable |
| comfortable mainnet validator | 8+ vCPU | 16–32 GB | 1 TB+ NVMe | 100 Mbit+ low latency |
| two validators on one host | size for both | 16 GB min / 32–64 GB comfortable | sum of both | sum of both |
| observer | modest | 1–4 GB | enough for the state you serve | stable |

An **observer** is the low-risk participation path: it is not in the finality denominator, has no bond
at risk for absence, and can serve reads, relay work and act as a witness. A **validator** is a
different role with a different floor. The public testnet's roughly 2 GiB per validator is a declared
capacity-limited test configuration, **not** a production sizing recommendation
([hardware requirements](validator-requirements.md)).

The start-up preflight refuses a below-floor validator. A below-floor start is possible only with both
`--allow-under-minimum` and `--accept-under-minimum-risk`; both are logged. The second flag is an
explicit acceptance of this page and of `TERMS.md`.

## 3. Quorum arithmetic: why one stuck node is not local

Finality is **strictly more than two thirds of the whole bonded stake**; the denominator does not
shrink when a validator is absent ([consensus](consensus.md), Law 14a/52). With equal stakes:

- four validators: one stop → 75% remains → finality continues; two stops → 50% → finality stops;
- three validators: one stop → exactly 66.6…% → **not** `> 2/3` → finality stops;
- a permanent loss of one third or more of the bond makes a quorum unreachable until the bonded set
  changes.

These are arithmetic, not probabilities. A single node can be the difference between a finalising chain
and a frozen one; no single node should be a **failure domain** for the network. Do not co-locate
validators that are meant to be independent; one host, one cloud account, one bad upgrade or one
network partition can otherwise remove more than a third at once.

## 4. Who bears what

| risk | network bears | operator bears | current control | residual |
|---|---|---|---|---|
| below-floor host / bad disk | finality lag or stall | income loss, no slash for absence; a buggy invalid block can slash | preflight, the two flags, notice, cgroup guidance | `--allow-under-minimum` remains possible; the memory floor is a policy table, not a measured curve above ~5k blocks |
| `throttled` host under load | execution pressure can reach `critical` and withhold production | missed production/attestation income | fallback word, execution permit lease, watchdog | the proposer acquires the execution permits and caps a batched block; the sequential path is inherently one-at-a-time, and zero headroom withholds |
| co-located / correlated validators | >1/3 stake can disappear together → no quorum | bond/income and reputation | co-location advisory, runbook | host topology is the operator's; no protocol-level anti-affinity |
| silence is paid | finality can stall while an absent validator is still paid | no bond loss today; opportunity/reward only | notice, future policy options below | `absence-slack` is a genesis decision and is off by default; the participation rule is in [Incident history](incident-history.md) |
| late detection | finality is already frozen before the operator notices | reputation, missed income | `/health` peer fields, `/metrics`, operator status | the signal exists; the operator still owns the external alert |

The operator owns the ringfence, the keys, the host and the monitoring; the protocol owns the quorum
rule and the slash predicate. Neither side can make the other's risk disappear.

## 5. Small operators, laptops and Raspberry Pis

A low-power host is a **good observer** and a **risky validator**:

- An observer adds read/service/relay capacity without entering the finality denominator. It is the
  right role for a laptop, a Raspberry Pi, a home connection or an experiment.
- A validator needs the per-validator floor, a stable network and continuous uptime. A Raspberry Pi 4/5
  can be a useful observer; treat it as a validator only if it actually meets the floor and you have
  measured it on your chain.
- More validators do not automatically mean more TPS. Throughput is bounded by block rate, block size,
  execution and network/finality; a swarm of under-provisioned validators can add failure domains
  without adding capacity. Load management (admission, permits, batching, relay) lets constrained nodes
  shed work; a batched block takes only the execution permits the host has, and a sequential block is
  one-at-a-time, but a slower host still produces less.
- Do not co-locate independent validators. If you want resilience, diversify host, provider, region and
  upgrade timing before adding another key.

## 6. Runbook when the node is degraded or stuck

1. Read `/health`, `/metrics` and the operator status. `degraded` is advisory; `acceptsDeploys` is the
   pool's capacity, not a memory verdict.
2. If the word is `throttled`, stop adding load and check memory/headroom/disk; the node is meant to do
   less, not to be healthy. If it reaches `critical`, production is deliberately withheld — fix the
   host, do not force it to propose.
3. Check finality lag, not just height. A chain can produce blocks while finality is frozen.
4. Restart only after the host has headroom; a restart into the same pressure starts the loop again.
5. If the node is merely behind, let it catch up; a returning validator can rejoin quickly. If it is
   stuck with a divergent finality, use the documented restore path, not an ad-hoc rewind.
6. Record the timestamps, fallback words, cgroup facts and logs. They are the evidence for a postmortem
   and for any future penalty policy.

## 7. If the network later prices absence

The current protocol has behavioural slashing only. It has no inactivity leak and no eviction; the
existing `participation-grace`/`absence-slack` rule scales **income only**, is off by default, and is a
genesis decision for each network. If a network wants to price absence, the staged path is:

| stage | mechanism | bond at risk? | status |
|---|---|---|---|
| 0 | current: behavioural slashing; absence paid; no inactivity leak | only invalid/equivocation | shipped |
| 1 | arm the existing income rule after measuring the participation lag | no | a network/genesis decision; measure first |
| 2 | an inactivity leak / reward decay (new state) | no | not built; Lean-first and maintainer decision |
| 3 | liveness slashing — **now the opt-in jail + stake vote** (RCHIP #55, Law 89) | only through a >2/3 active-stake vote | implemented, **default off**; jail first (bond untouched), the vote is the only bond path; live testnet evidence owed |
| 4 | mainnet arming | as above | **not authorised**; waits on the live artefact |

**Policy decision.** For the inactivity leak, no automatic step burns a bond. RCHIP #55 adds a *bond-preserving jail* and a *vote-gated slash*; the two
are not a leak. The recommendation for a default network is still **do not arm**; a network that arms
accepts that a >2/3 active-stake vote can eject a validator, and that a partitioned minority can be
jailed on the side that still finalises. Mainnet arming waits on the live testnet artefact.

## 8. Terms and reporting

Read [`TERMS.md`](../../../TERMS.md) (draft for legal review) for
the operator notice. Report a suspected defect through the repository's issue tracker; do not publish a
key, a signed deploy or a private host detail in a public issue.
