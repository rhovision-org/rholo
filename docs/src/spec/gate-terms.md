# Gate terms

This page fixes the vocabulary the release gates use. A term that is load-bearing in a gate
and not defined here is a review finding, not an implementation detail.

## Gate

A **gate** is a release-blocking acceptance condition with a pass condition, a fail/block
condition, an observable, a witness and a locality. A CI check is a gate only when it blocks
the release; a source-shape check is not a pressure test.

A **mainnet gate** is one of the #428 M1–M6 conditions, the #584 G1–G6 live verdict, the
#51/§6 compatibility tracker, the testnet acceptance criteria in
[Testnet acceptance](testnet-acceptance.md), or one of the open audit rows. The audit register
is an input to these gates, not the gate.

## Capacity and topology

**Supported tier** — the declared per-validator CPU, RAM, disk, peak RSS and replay-headroom
envelope. A below-minimum injection is a diagnosis arm, not supported-tier acceptance.

**Failure domain** — the host/rack/region unit whose loss the quorum map must survive. A PID
namespace is not a failure domain; containers can hide sibling validators.

## Ingress

**Bounded ingress / accepted envelope** — the declared capacity within which accepted work is
not silently lost. A refusal is explicit (HTTP 429 or gRPC `RESOURCE_EXHAUSTED`); a bound must
not be reported as a malformed request.

**Backpressure** — an explicit wait or refusal signal. A log-and-drop, or a spawn whose result
is discarded, is not backpressure.

**Direct HTTP** — a request to the node's own listener, not through a proxy. An nginx access
log is a measurement of nginx, not of direct HTTP or gRPC.

## Bootstrap

**Off-host bootstrap** — a peer on a different host advertises a routable address,
authenticates, joins, **receives and applies** the approved fringe and hydrates state. A send
log is not delivery; the transport completion and the restored-state equality are separate
observations.

**Loopback negative control** — a loopback address advertised from another host must fail
clearly. A private (RFC 1918) or decimal-encoded address is not a loopback string, and a
document that merely contains the word "dial-back" is not a negative control.

## Finality and recovery

**Finality** — strictly more than two thirds of the whole bonded stake. Admission and inclusion
are not finality; `acceptsDeploys` is pool capacity, not settlement. See
[Consensus](../node/consensus.md) and [Finality is not admission](../node/admission.md).

**Anchor retention** — a fixed finalized block that one side had not finalized before a
partition remains finalized by every validator on a K-sample dwell after the heal. Moving heads
may differ; equality of last-finalized representatives is not the criterion.

**Dwell** — the minimum number of distinct, stable polls before a claim is made.

**Healing window** — the measured interval, with a recorded floor and ceiling, within which a
rejoining validator must finalize the anchor. A floor derived from the node's own maintenance
is a measurement, not a wish.

**Safe recovery** — a restart or recovery arm finalizes the same deploy with no accepted work
lost, and no spurious self-replay or own-key halt fires on a healthy key; the approved root is
unchanged.

## Compatibility and release

**Category-A** — a deviation whose fix changes merged-state hashes, a replay verdict, or the
wire format. It is derived from those effects, not asserted by a row label.

**Independent verdict / acceptance** — a verdict by a reviewer who is not the release owner,
the operator, or an author/committer of the selected source or evidence. The record names the
reviewer's relationship, affiliation, prior authorship and conflicts. Name inequality alone is
not independence.

**Lockstep** — every validator activates the selected binary inside the declared window. A
mixed-version rollout is a fail, not a slower lockstep.

**Selected candidate** — the source/artifact/executable pin chosen by the release owner. Four
agreeing records that pin a different binary are not the selected candidate.

**Reproducibility record** — the pinned source, binary, artifact, genesis, config, stake, host
and clock boundaries; log hashes; deploy IDs; included/finalized block hashes; commands; CI
runs; raw outcomes; the reviewer's conclusion; and the non-claims.

**Non-claim** — an explicit statement of what a record does not establish.

## Failure vocabulary

**Divergence** — two correct nodes compute different post-states for the same block. A
self-built replay failure is a local fault, never divergence (Law 92).

**Fault** has three in-repo senses: a *local fault* (the self-replay failure), a *pipeline
fault* (an outstanding body or verdict), and a *slashable fault* (`Malicious`, `Misdemeanour`,
`HonestMistake`). Say which one a gate means.

**Isolation** has three senses: *proposer isolation* (the `proposal_isolated` latch),
*structural worktree isolation*, and *shard/cell isolation*. Say which one a gate means.

## Evidence and status

**Evidence** — a committed, digest-pinned record, or a code symbol the reader actually read. A
citation is not evidence, and an identifier that merely occurs somewhere is not a definition.

**PASS / FAIL / PRECONDITION** — PASS means the witness ran and met the condition; FAIL means a
contradiction; PRECONDITION means the input needed to decide is absent. **PRECONDITION is never
PASS.**

**Register status** — `met` (witness resolves, negative control recorded, evidence pinned, every
dependency met), `owes` (a named witness or record is missing), `blocked` (an external input is
absent), `external` (only a human or an external campaign can supply it), `decision` (an owner
must record a decision), `precondition` (the observation cannot be completed), `reviewable` (an
authored disposition, not a measurement), `superseded` (a recorded correction replaced it).

## Heights and rounds

A **height** counts rounds; one block per bonded validator per round. A height is not a block
count. See [Height is not a block](../node/consensus.md).
