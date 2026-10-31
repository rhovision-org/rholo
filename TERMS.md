# rholo — Operator Terms and Risk Notice

**Status:** draft for legal review · **Version:** 1 · **Date:** 2026-10-29

This notice is for anyone who runs a **validator**. It supplements, and does not replace, the
GNU Affero General Public License v3.0 in [`LICENSE`](LICENSE). The licence grants rights and
disclaims warranties; this notice tells you what the protocol can do to your bonded stake and your
node, and what the software does **not** promise.

## 1. A validator is a bonded, slashable role

A validator's bond, accrued and unwithdrawn rewards, and any escrowed withdrawal claim are at risk.
Slashing is **behavioural**: every node checks the block's work, and a block that fails validation for
an attributable reason, or a proven equivocation, can cost a stated share of that stake. Severity is
graded (`Malicious`, `Misdemeanour`, `HonestMistake`); the bond is not a deposit with a guaranteed
return.

An **invalid block can be produced by an overloaded or buggy node as well as by a malicious one.**
Running below the documented hardware floor, overloading the host, or skipping upgrades can therefore
lead to a slash even when you did not intend to attack the network.

An absent or merely slow validator is **not** slashed under the shipped protocol: its bond stays in
the pool, it continues to count in the finality denominator, and (with the shipped `absence-slack = 0`)
it is still paid for an epoch it was drawn for, even if it did not run. That asymmetry is current
behaviour, not a promise: the protocol, a network's genesis parameters, or a future release may price
absence differently.

A network may **arm liveness enforcement** at genesis (RCHIP #55, `docs/src/spec/rchip-55-liveness-enforcement.md`). On such a network, after sustained absence across a configured number of **advancing** epoch boundaries the validator is removed from the active set for a configured number of epochs and earns nothing; **its bond and its delegators' principal are untouched**. A **stake-weighted vote of strictly more than two thirds of the active stake**, over a configured window, may then **slash it at the configured tier and permanently eject it** from bonding. The bond is at risk only through that vote. A network with the shipped default genesis is unaffected.

## 2. Hardware and uptime are your responsibility

The documented minimum is **per validator**: 2–4 vCPU, 8 GB RAM, 100–250 GB NVMe SSD, 10 Mbit+
stable network. The comfortable mainnet estimate is 8+ vCPU, 16–32 GB RAM, 1 TB+ NVMe, 100 Mbit+.
A host running two validators must size for both and give each its own cgroup. See
[the hardware requirements](docs/src/node/validator-requirements.md) and
[the ringfence guide](docs/src/node/resource-ringfence.md).

The node's start-up preflight refuses a validator below its memory floor. A below-floor start is
possible only with **both** `--allow-under-minimum` and `--accept-under-minimum-risk`; the second flag
is your explicit acceptance of the risk this notice describes, and both are logged. An observer may
run below the validator floor; it is not in the finality denominator and cannot be slashed for
absence.

**Uptime matters more than horsepower.** An absent validator holds finality back. You are responsible
for power, network, disk, monitoring, keys, upgrades, and the host's cgroup/co-tenancy.

## 3. Enough stuck validators stop the chain finalising

Finality requires **strictly more than two thirds of the whole bonded stake**, not merely more than two
thirds of the nodes that happen to be running. The denominator does not shrink when a validator is
absent.

- with four equal validators, one can stop and the other three still finalise; two stopping leaves 50%,
  which cannot reach a quorum;
- with three equal validators, one stopping leaves exactly 2/3, which is **not** `> 2/3`, so finality
  stops;
- a permanent loss of one third or more of the bonded stake makes a quorum permanently unreachable
  unless the bonded set changes.

A stuck node is therefore a network-level liveness risk, not only a local outage. Do not co-locate
validators that are meant to be independent failure domains, and do not treat a low-power host as a
validator merely because it can start.

## 4. What you are not promised

No reward, no yield, no uptime credit, no insurance, no bail-out, and no underwriting of a slash. A
validator may earn nothing in an epoch, may forfeit production income while degraded, and may lose a
stated share of its stake for a slashable act. The software is provided under the AGPL-3.0 warranty
disclaimer and limitation of liability; nothing here creates a fiduciary duty, a deposit, or a
guarantee of participation.

## 5. Monitoring and degraded states

The node exposes `/health`, `/metrics`, operator status, the fallback word (`normal`, `throttled`,
`critical`, `unavailable`), memory/headroom facts and the permit limits. A `critical` validator
deliberately withholds production; a `throttled` validator is meant to do less work, and the operator
should investigate before it becomes critical. Read
[Validator risk, hardware and participation](docs/src/node/operator-risk.md) for the runbook and the
current limits.

## 6. Acceptance and changes

By bonding a validator, running a validator key, or starting a below-floor validator with the two
flags, you accept this notice. The protocol and this notice may change; a network's genesis parameters
(for example the participation/absence rule) are chosen by that network. Check the version shipped with
your binary and re-read this file after an upgrade.

This is an operator risk notice, not legal advice. Have counsel review it before relying on it as a
contract.
