# October 2026 re-audit — preregistration and verdict ledger

**Pinned tree:** `origin/dev` `be5016682efcfc6dbc2df7cb409ece7a55533e08` (2026-10-30). Later merges
before publication, if any, are recorded in the page's frame; node code has been unchanged since
`9d88b787e`.

**Report-only.** No production code was changed. Probes are instrumentation under
`rholang/tests/gas_amplification.rs` (env-gated `RNODE_GAS_PROBE=1`) and shell runs recorded here.

## Method

Three independent lenses, each blind to this ledger and to the September audit: **A** work and the
block path (cost/DoS), **B** network ingress and the new surface, **C** consensus/recovery/liveness and
the assurance claims. Each returned at most six candidates with mechanism (`path::symbol`),
reachability, an artifact/falsifier, and a severity argument; each included one candidate it expected a
referee to reject.

Every candidate was handed to a separate referee that saw only the claim, mechanism, class and
falsifier. Verdicts:

- `CONFIRMED` — mechanism re-derived in the code and the falsifier named;
- `NOT-RE-ESTABLISHED` — plausible, not re-provable from source; the measurement it needs is named;
- `REFUTED` — only with a positive artifact (a named line, an existing test, or a measurement); an
  empty-handed refutation is forced to `NOT-RE-ESTABLISHED`;
- `UNSTATABLE` — only for proposer-side claims with no reachability fact.

**Seeded controls:** A6 (linear-but-flat charge), B6 (relay fan-out), C6 (finality bound skipped when
no finality exists) were written by the lenses as likely-refuted. Calibration and verdicts are below.

## Candidate ledger

| id | lens | claim | verdict |
|---|---|---|---|
| A1 | A | `toSet`/`toMap` (and set/map literal eval) run an uncharged comparison sort | **CONFIRMED** |
| A2 | A | `rho:gov:censure` charges N² for Θ(U³) work; `MAX_GOV_UNIVERSE` permits it | **CONFIRMED** |
| A3 | A | `Set/Map ++` and `.union` charge `3·|right|` for Θ((L+R)log) work | **CONFIRMED** |
| A4 | A | `Map.set`/`Set.add` charge flat 3 for clone+scan+full re-sort | pending |
| A5 | A | `keys` charges flat 10 for clone+full sort | pending |
| A6 | A | control: string `length` flat 10 for Θ(n) | pending |
| B1 | B | the greeter dials on a peer's handoff without drawing from `MAX_DIALED_SESSIONS` | **CONFIRMED** (capped at `MAX_PEERS`=256, not 16) |
| B2 | B | txn API leg `amount` is bounded only by the coordinator's full balance | **CONFIRMED** (the C121 residual) |
| B3 | B | websocket upgrade+challenge run in the serial accept task (~40 s stall) | **CONFIRMED** |
| B4 | B | dev faucet dispenses node-signed REV on the public bind | pending |
| B5 | B | `/health` and `/metrics` are unlimited public disclosures | pending |
| B6 | B | control: relay fan-out / seen-set clear | pending |
| C1 | C | a >2/3 coalition can jail then slash a censored honest validator | **CONFIRMED** (already recorded: H-RS-17 / RCHIP #55 §8) |
| C2 | C | B46's "`Running` requires no unresolved orphan" is false | **CONFIRMED** (wording; readiness is the gated predicate) |
| C3 | C | the epoch seed is still steerable via a stale fringe | pending |
| C4 | C | body `put` and `mark_pending` are separate transactions; a crash between them orphans the body | **CONFIRMED** |
| C5 | C | HAZOP/resource-safety gates check symbol occurrence, not that the test asserts the claim | pending |
| C6 | C | control: the finality bound is skipped when no finalised block exists | pending |

## Measurements (this pass)

`gas-probe.log`, release profile, `RNODE_GAS_PROBE=1`:

- `toSet` delta is a flat **18 phlo** at N = 500, 1 000, 2 000, 4 000, 10 000, 20 000, 40 000, while
  the converted wall time grows 7 → 12 → 22 → 47 → 151 → 445 → 724 ms.
- `toMap` delta is negative at every N (−4 982 … −99 982) with wall time 17 → 620 ms: no positive
  charge site in the arm.
- `set` delta is a flat **22 phlo** across N = 500…10 000.
- `union` of two equal N-element sets: delta 6 949 / 38 949 / 78 949 at N = 1 000 / 5 000 / 10 000 —
  proportional to N, so the September union defect is **closed**; the *asymmetric* case (huge left,
  singleton right) remains, and is A3.

## Three-chain pins

`three-chain.json`: Agave `v4.4.0` / `9ef63128…`; Sui `testnet-v1.83.0` / `16d2dc1e…` (sparse);
Bitcoin SV `v1.2.3` / `6504a3af…`.
