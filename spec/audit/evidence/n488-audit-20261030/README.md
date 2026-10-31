# #488 — the audit terminates on a producing-but-unfinalising chain

**Claim.** With the #489 range cache and the snapshot/movement statement of this unit, `tools/reconcile-network.sh`
reads every node in a four-row table and completes (plan only, exit 0) on a two-by-two partition: finality
frozen at 32/33 while all four nodes keep producing.

**What does not prove.** This is a finite termination witness on a disposable devnet, not a recovery
proof: the tool was run in plan mode, nothing was stopped or moved, and the chain was not reconciled.
The known limits — no intent journal (`F-U16-03`), the meet's ancestry bound — are unchanged.

## Run

- Date: 2026-10-30, ~14:38 UTC.
- Source: branch `fix/488-reconcile-audit` (this unit's commit plus its parent), on top of `dev@21b03ead7`.
- Node image: `rnode:477` (`rnode 0.1.0 (8967e714f8ad4d55385741a6ce1a5f024b9ea39f)`), reused because the
  audit is host-side and the image only has to produce a four-validator chain.
- Host verifier: `./target/debug/rnode` from the same tree, for `verify-block` / `verify-attestation`.
- Topology: `tools/devnet.sh up --validators 4 --stakes 250,250,250,250 --autopropose --fresh`
  with `DEVNET_PREFIX=n488`, `DEVNET_NETWORK=n488net`, HTTP base 50403 (validators on 51403/52403/53403).
- Partition: `tools/n447_partition.py --prefix n488 --network n488net` — bootstrap + validator-1
  versus validator-2 + validator-3. `partition.json` records `topology_recovered: true`.

## Readings

Section 1 (the node table, all four rows):

```
  A  h=38     finalised=33     afe10701237c7c4f…
  B  h=40     finalised=33     afe10701237c7c4f…
  C  h=37     finalised=32     a40a5baa3a1b8bfb…
  D  h=37     finalised=33     afe10701237c7c4f…
  snapshot: MAXH=40 taken at 2026-10-30T14:38:20Z
```

Finality is frozen at 32/33 and the tips are 37–40 — the state the tool exists for. The meet is height 33
(750 of 1000, the subset A B D; C lagging, not vetoing). Section 4 states `(heights 34..40; snapshot 40 …)`.

The movement NOTEs are the live half of the new statement — the chain advanced **during** the audit:

```
  NOTE: A advanced 38 → 49 since the snapshot; the audit above covers only heights 0..40
  NOTE: B advanced 40 → 49 since the snapshot; …
  NOTE: C advanced 37 → 39 since the snapshot; …
  NOTE: D advanced 37 → 39 since the snapshot; …
```

`audit_seconds=10 exit=0`. Before #489 the same shape was 27 m 51 s and still growing; the deterministic
half of the evidence is `tests/reconcile-enumeration-regression.sh` (the fake transport consumes the table
unless `ssh -n` is passed, and the no-`-n` mutant exits 2 with `read 1 of 4 node row(s)`).

## Files

- `audit.log` — the tool's full output for the run.
- `partition.json` — the harness's topology snapshot and its restored state.
