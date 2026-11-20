# Release checks: local devnet first, public testnet second

**Status — 2026-11-19.** The local stage is defined here and is being run. No local check has been
declared passed in this document yet; results go in the records, not in prose. No public testnet run
has happened, and this document does not authorise one.

**Superseded by the seven claims, 2026-11-20.** The release set is
`spec/audit/release-claims.json`: RC-1 truthful settlement, RC-2 safety, RC-3 state agreement,
RC-4 recovery without loss, RC-5 overload refusal, RC-6 join and audit, RC-7 upgrade compatibility.
Each claim names one executable test under `tests/`; this page's five checks map to them as
finality→RC-1, overload→RC-5, partition→RC-2, state agreement→RC-3, join/upgrade→RC-6/RC-7.

```sh
python3 tools/release-check.py list                         # measured statuses
python3 tools/release-check.py run --claim RC-1 --env local # one test, one record
python3 tools/release-check.py record --claim RC-1 --env local --record target/release-claims/local/RC-1
```

A status is `pass`, `fail` or `not run`; it is derived by re-running the claim test's
`check(record)` over the newest committed record. There is no `owed` status and no authored
placeholder.

## The rule

A release check is **one sentence about observable behaviour**, plus **one observation that could
prove it false**, plus **one standard setup**. If a requirement is a property of the laboratory
(a public address, four boot IDs), a sizing estimate (RAM/CPU), or a recording rule (independent
observations, a reviewer), it is not a release check. It is a precondition or a record rule.

This is why the old twelve were illegible: they mixed those four things and gave every row its own
setup. The same check on a different setup is the same check; the setup is not the claim.

## Stage 1 — the local devnet

One Linux host, Docker, `tools/devnet.sh`, one image, fixed ports. Two profiles:

| profile | validators | memory per validator | total | why |
|---|---|---|---|---|
| `local-4` | 4 | 4 GiB | 16 GiB | fits the box |
| `local-8` | 8 | 2 GiB | 16 GiB | fits the box |

The box has 47 GiB total and about 25 GiB free; 4 x 8 GiB = 32 GiB and 8 x 8 GiB = 64 GiB do not fit.
**Local runs are mechanism checks, below the documented 8 GiB tier, and are recorded as
`local-devnet`.** They are never relabelled as off-host, multi-host or live. Capacity at the
documented tier is a sizing question, not a local release check.

Run one network at a time. Between checks: `tools/devnet.sh down -v`. Logs and outputs under
`target/`; only the packaged evidence records are committed.

## The five local checks

### Check 1 — a transfer finalises and every node agrees

**Pass:** a signed transfer is included; every validator reports the marker in finalised state at the
same height and hash; the finalised write is still there through two further epoch boundaries; the
marker is absent before the write (the absent-marker control).
**Falsifier:** the transfer never finalises, nodes disagree on the finalised block, the finalised
write disappears, or the absent-marker control fires.

Tool: `tools/consensus-acceptance-drill.py` (deploy, finality, retention, per-validator agreement).

**What this local check does not claim:** early finality on the settlement surfaces (G584-G3). That
needs the live settlement endpoints and stays in stage 2.

```sh
DEVNET_PREFIX=acceptance DEVNET_NETWORK=acceptance DEVNET_NODE_MEMORY=4g \
  tools/devnet.sh up --validators 4 --fresh
python3 tools/consensus-acceptance-drill.py --validators 4 --local --window 240 \
  --out target/rc/check1-n4
DEVNET_PREFIX=acceptance DEVNET_NETWORK=acceptance tools/devnet.sh down -v

DEVNET_PREFIX=acceptance DEVNET_NETWORK=acceptance DEVNET_NODE_MEMORY=2g \
  tools/devnet.sh up --validators 8 --fresh
python3 tools/consensus-acceptance-drill.py --validators 8 --local --window 300 \
  --out target/rc/check1-n8
DEVNET_PREFIX=acceptance DEVNET_NETWORK=acceptance tools/devnet.sh down -v
```

### Check 2 — a validator can be lost and restarted without losing work

**Pass:** with one validator stopped, the survivors keep finalising; every deploy submitted in the
window resolves successfully on every survivor; after restart the victim reports the same finalised
`(height, hash)` as the survivors, at least three heights above the pre-kill baseline; no finalised
height regresses and no fixed height changes hash; no fault counter is set; the victim proposes
again.
**Falsifier:** finality stalls, accepted work is lost, the rejoiner needs manual surgery, or a
spurious halt fires.

Tool: `tests/consensus-restart-under-load.py`.

```sh
DEVNET_PREFIX=restartload DEVNET_NETWORK=restartnet \
DEVNET_GRPC_BASE=59402 DEVNET_HTTP_BASE=59403 DEVNET_ADMIN_BASE=59405 \
DEVNET_NODE_MEMORY=4g tools/devnet.sh up --validators 4 --fresh
DEVNET_PREFIX=restartload DEVNET_HTTP_BASE=59403 DEVNET_ADMIN_BASE=59405 \
python3 tests/consensus-restart-under-load.py --validators 4 --cycles 2 --window 240 \
  --load-seconds 20 --out target/rc/check2
DEVNET_PREFIX=restartload DEVNET_NETWORK=restartnet \
DEVNET_GRPC_BASE=59402 DEVNET_HTTP_BASE=59403 DEVNET_ADMIN_BASE=59405 \
tools/devnet.sh down -v
```

### Check 3 — a fresh node joins and syncs

**Pass:** a validator that loses its store and is restarted against the running network restores the
bootstrap's finalised state from scratch, reaches the bootstrap's height and finalised block, clears
the sync surface, and shows no `LFS state sync failed`. The supported recovery shape stages the node
without its key first; the key is activated from the restored DAG.
**Falsifier:** the walk stops short, the node serves a different finalised state, or the key-present
shape signs a conflicting block.

Tool: `tools/n457-mature-join-arm.py --supported-recovery`.

```sh
DEVNET_PREFIX=n457join DEVNET_NETWORK=n457net \
DEVNET_GRPC_BASE=55402 DEVNET_HTTP_BASE=55403 DEVNET_ADMIN_BASE=55405 \
DEVNET_NODE_MEMORY=4g tools/devnet.sh up --validators 4 --fresh
python3 tools/n457-mature-join-arm.py --min-height 28 --timeout 600 \
  --supported-recovery --out target/rc/check3
DEVNET_PREFIX=n457join DEVNET_NETWORK=n457net \
DEVNET_GRPC_BASE=55402 DEVNET_HTTP_BASE=55403 DEVNET_ADMIN_BASE=55405 \
tools/devnet.sh down -v
```

The "public address" condition is **not** part of this check. A different machine is a property of
the public-testnet setup; locally the joiner is a container on the same host, and the record says so.

### Check 4 — overload is refused, not silently dropped

**Pass:** with the accepted in-flight work held, further HTTP and direct-HTTP requests are refused
with the admission message and `retry-after: 1`; the unset default admits every probe; the gRPC
ingress and pool bounds refuse with `RESOURCE_EXHAUSTED`; the node stays alive and no accepted work
disappears.
**Falsifier:** silent accepted loss, an unbounded queue, a 5xx, an unset default that refuses, or the
node dies.

Tool: `tools/m2-pressure-arm.py` (two devnet lifetimes, because the cap is read at listener start).

```sh
DEVNET_PREFIX=m2cap DEVNET_NETWORK=m2net \
DEVNET_GRPC_BASE=56402 DEVNET_HTTP_BASE=56403 DEVNET_ADMIN_BASE=56405 \
DEVNET_NODE_MEMORY=2g DEVNET_NODE_ENV=RNODE_HTTP_MAX_INFLIGHT_DEPLOY=2 \
  tools/devnet.sh up --validators 1 --fresh
python3 tools/m2-pressure-arm.py phase --mode capped --out target/rc/check4/capped \
  --container m2cap-bootstrap --port 56403 --cap 2
DEVNET_PREFIX=m2cap DEVNET_NETWORK=m2net \
DEVNET_GRPC_BASE=56402 DEVNET_HTTP_BASE=56403 DEVNET_ADMIN_BASE=56405 \
  tools/devnet.sh down -v

DEVNET_PREFIX=m2cap DEVNET_NETWORK=m2net \
DEVNET_GRPC_BASE=56402 DEVNET_HTTP_BASE=56403 DEVNET_ADMIN_BASE=56405 \
DEVNET_NODE_MEMORY=2g tools/devnet.sh up --validators 1 --fresh
python3 tools/m2-pressure-arm.py phase --mode unset --out target/rc/check4/unset \
  --container m2cap-bootstrap --port 56403
DEVNET_PREFIX=m2cap DEVNET_NETWORK=m2net \
DEVNET_GRPC_BASE=56402 DEVNET_HTTP_BASE=56403 DEVNET_ADMIN_BASE=56405 \
  tools/devnet.sh down -v

python3 tools/m2-pressure-arm.py grpc --out target/rc/check4/grpc
python3 tools/m2-pressure-arm.py assemble --capped target/rc/check4/capped \
  --unset target/rc/check4/unset --grpc target/rc/check4/grpc \
  --out spec/audit/evidence/local-check4-pressure-20261119 --locality local-devnet --port 56403
python3 tools/m2-pressure.py check --record spec/audit/evidence/local-check4-pressure-20261119 \
  --locality local-devnet
```

The gRPC half is the in-process test against a local listener; the record says
`origin: candidate-test`, not endpoint transport. That is the mechanism check. A public endpoint and
the pool-full half are stage-2 questions.

### Check 5 — a partition heals without two conflicting finalised chains

**Pass:** after a split, at most one side finalises a conflicting block; after healing there is one
chain, finality resumes, the approved anchor is retained, and every validator at or above an anchor
height has that anchor finalised. Two cycles cut two different victims.
**Falsifier:** two conflicting finalised blocks, a permanent wedge, or the anchor changes.

Tool: `tests/consensus-partition-rejoin.py --local` (a single-host mechanism run; the cross-node
finalised-disagreement assertion already exists in the harness).

```sh
DEVNET_PREFIX=acceptance DEVNET_NETWORK=acceptance DEVNET_NODE_MEMORY=4g \
  tools/devnet.sh up --validators 4 --fresh
python3 tests/consensus-partition-rejoin.py --validators 4 --local --out target/rc/check5-n4
DEVNET_PREFIX=acceptance DEVNET_NETWORK=acceptance tools/devnet.sh down -v

DEVNET_PREFIX=acceptance DEVNET_NETWORK=acceptance DEVNET_NODE_MEMORY=2g \
  tools/devnet.sh up --validators 8 --fresh
python3 tests/consensus-partition-rejoin.py --validators 8 --local --out target/rc/check5-n8
DEVNET_PREFIX=acceptance DEVNET_NETWORK=acceptance tools/devnet.sh down -v
```

## What is not a local check — and where each old id went

| old id | fate |
|---|---|
| `C343` | operator diagnostics runbook; not a release check |
| `G428-M1` | the local profiles use what fits the box; sizing at the documented tier is a runbook, not a release check |
| `G428-M2` | check 4 |
| `G428-M3` | check 3 |
| `G428-M4` | checks 1, 2 and 5 at N=4 and N=8 |
| `G584-G1` | public-testnet setup precondition: every host runs the chosen release |
| `G584-G2` | check 1 |
| `G584-G3` | check 1 (no early finality) |
| `G584-G4` | check 2 |
| `G584-G5` | public-testnet evidence rule: raw replies and checksums in the record |
| `TA-C1` | check 1; the independent reviewer is a public-testnet evidence rule |
| `TA-C2` | check 2; the independent reviewer is a public-testnet evidence rule |

Nothing is deleted from the record: the old rows and their source links stay in
`mainnet-gates.external.json` until the local stage is complete and the tracker is rewritten as the
public-testnet stage.

## Stage 2 — the public testnet list (defined, not run here)

After all five local checks pass, the same five are run on **`testnet-4`**: four independent hosts,
one validator each, the chosen release on every host, real operators and observers, public
endpoints. `testnet-8` is used only where scale matters. The records keep raw observations and
checksums.

This stage is the dev team's. It needs the chosen release, the hosts and the operator window.
**This document does not close it, does not authorise it, and does not decide to announce a public
testnet.**

## What this does not claim

- A local pass is a mechanism pass, not release readiness.
- Local records are `local-devnet`, below the documented tier, and are never relabelled.
- The public testnet stage, the operator window and the announcement decision are not ours.
- No invented evidence; a failed check is a code defect or a real limitation, and the setup is not
  changed to manufacture a pass.
