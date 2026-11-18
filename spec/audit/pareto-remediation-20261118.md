# Pareto remediation of the remaining mediums — 2026-11-18

**Triage and selected fix set.** PR #606 merged with the register at 111 gates, 0 met. This
note triages every open item after that merge and selects the Pareto set: the small,
Docker-free fixes that remove a false-pass path and, where possible, also reduce machinery.
Nothing here closes a gate, flips a row, or authorizes release.

## Method

Three read-only inventories after #606:

1. the register and external artefacts (`mainnet-gates.json`, `mainnet-gates.md`);
2. the fog register F1–F36 in `mainnet-gate-review-20261118.md`;
3. the M1–M5 code-side gaps with file:line and existing-test coverage.

Each item is scored on **benefit** (removes a concrete defect-passing path, enables closure
when evidence lands, or removes a false claim) and **cost** (implementation plus ongoing
maintenance: new schemas, records to keep in sync, CI minutes, campaign harnesses).
**Double Pareto** means benefit up and maintenance down.

Selection rule: S/M effort, Docker-free, gate-relevant, no new campaign harness, no new
per-row bookkeeping.

## Selected (one remediation PR)

| ID | fix | attack removed | effort | maintenance |
|---|---|---|---|---|
| A | One closed deploy-refusal vocabulary | the three-prefix `contains` classifier can report a new bound as HTTP 400; dynamic messages are invented in two crates | S | down |
| B | The queue gate refuses an absent or unpinned bound | an absent binding, a continuation-line `unbounded_channel`, `mpsc::unbounded` and `channel(n)` read green | S | down |
| C | gRPC interceptor and `do_deploy` tests | `InterceptedService` and the `resource_exhausted`/`retry-after` mapping are untested | S | flat |
| D | Evidence-pack strict mode (`--require-pins`) | absent pins and a missing non-claims section pass `verify` | S | down (opt-in) |
| E | Locality refuses private and encoded addresses | RFC1918, ULA, link-local, `2130706433` and `127.1` pass an off-host claim | S | down |
| F | M5-04 user-deploy rejection falsifier | "absence in `RejectionsMap` means not rejected" has no user-id red test | S | flat |
| G | M5-06 kept-effect state readback | the coverage report is computed from the same fold that builds the state | S | closes a hole |
| H | `stream()` reports per-peer completion | `stream()` returns `()` and "FinalizedFringe sent" is logged unconditionally | M | flat |
| I | Relay capacity-refusal counter | a relayed 429/`RESOURCE_EXHAUSTED` folds into generic `relay_failures` | S/M | flat |
| J | A real-TCP-listener admission test | every admission test calls the router directly; the served path is unexercised | M | flat |

Design boundaries that keep the cost down:

- **A** does not change `ApiErr<String>`. `pool.rs` is the sole producer of refusal phrases;
  classification is exact, not a substring.
- **D** is opt-in. The register checker uses `--require-pins`; non-claims are required for
  packer-v2 manifests; the eight-plus legacy records without a binary pin stay verifiable but
  cannot serve as gate evidence.
- **E** normalises legacy numeric IPv4 with `socket.inet_aton` and checks the address
  classes explicitly rather than by string.
- **F** and **G** are tests: they do not change the production fold; they falsify the two
  quiet-report paths independently of the report.
- **H** is one atomic commit so the compiler finds every ignored result.
- **I** adds an inherent `deploy_with_capacity`; the `DeployService` trait does not churn.
- **J** holds the semaphore before the request so the 429 is deterministic, uses an ephemeral
  port and a finite timeout, and aborts the server task.

## Not doing, and why

| item | why not |
|---|---|
| C357 reviewer git-author check | handles do not map to git identities; new schema for a claim it cannot prove |
| F19 validator key-mismatch startup refusal | changes boot behaviour and needs a key-rotation policy; a key-hygiene unit, not a gate fix |
| M1 typed tier / RSS / replay curve | a record without a measured soak is maintenance with no assurance; the soak is Docker |
| M1 host/stake map and host-kill; M3 off-host, loopback negative and failover; M4 N=4/N=8 and host loss; M2 direct HTTP | multi-host campaigns: high implementation and harness maintenance; operators/Jim |
| M5 historical 840→841 bundle | unavailable (continuing blocker); no waiver |
| M5-03 per-chain verdict DTO | gated on M5-01/M5-02 inputs that do not exist |
| 45 per-candidate §6 dispositions; per-candidate witnesses | independent audit work and per-row bookkeeping; the 47-row disposition structure is the right shape |
| register-wide witness controls and evidence tightening | volume bookkeeping with little marginal assurance; a met gate is already deeply bound |
| owner `@login` conversion; testnet live rows; A1.3 | human/live, not code |
| CI workflow wiring | the token lacks `workflow` scope; the exact request stays in `ci-wiring-request.md` |

## Follow-up (separate PR)

HF51 and S6 rows are built with `witness_kind: null`, no control and no evidence, so those
families cannot reach `met`. The follow-up adds an authored binding and lets the builder emit
`met` only when every field `resolve_witness` validates is present; an incomplete binding is a
hard error, `verdict: accepted` alone promotes nothing, and the register stays 0 met until
real evidence is authored. It is separate because it changes the builder's promotion
invariant and needs the evidence-model owner.

## Boundary

No gate is met; no campaign, export or independent verdict happened. The multi-host and
operator artefacts remain owed and are named above, not waived. The remediation PR changes
code and checkers; it does not manufacture evidence.

**State.** 111 gates, 0 met; 47 dispositions (46 reviewable, 1 withdrawn); register S6
45 reviewable, 3 owes, 1 superseded. No row is flipped; no release is claimed.
