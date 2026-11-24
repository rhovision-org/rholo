# RC-4 failover — pre-registered protocol

**Status: DRAFT — NOT EXECUTED.** This runbook fixes the two-host window before a run so the
record cannot be assembled around the result. It does not claim that any run has happened.

- Arm: `RC-4/failover`, locality `off-host-claim`, required.
- Behaviour: a joiner with an ordered seed list joins when the **first** seed is down and the
  **second** is live.
- Runtime: an ordered bootstrap seed list. `-b` is repeatable and the HOCON value is an ordered
  array; a single string is still one seed. The connection loop tries the seeds in authored order
  and stops at the first success; the checkpoint source does the same.
- Evidence destination: `spec/audit/evidence/rc4-failover-<date>-<sha7>/`.

## 1. What the window must establish

Two routable, non-private hosts. Seed 0 is down after it has been registered; seed 1 is the live
bootstrap. A fresh joiner is configured with both seeds in that order, joins through seed 1, and
reaches the bootstrap's finalised head. The joiner's log carries the fallback witness:

```
bootstrap seed 2/2 connected: <seed-1-host>
```

The record is judged by `tests/release-claim-4.py --arm failover`; its `check` reads `seeds`,
`seed_status`, `joined`, `joined_via` and `finalised_head_agrees`. `safe` is the only closing
reading.

## 2. Hard NO-GO conditions

- **A loopback, private, link-local or reserved seed.** The arm's locality is `off-host-claim`;
  the runner refuses these as `not run`, and the locality gate refuses them at pack time.
- **Fewer than two seeds**, or the same seed twice.
- **The first seed live.** The arm is the fallback order, not a join in general.
- **A joiner that did not reach the bootstrap's finalised head**, or a head disagreement.
- **A loopback diagnostic offered as the record.** `tools/failover-diagnostic.py` proves the
  mechanism on one host; it is `loopback-diagnostic`, is never packed, and is not this arm.
- **A missing `joiner.log`.** The log is the fallback witness and the raw evidence the locality
  gate scans.

## 3. Preflight pins

| field | value |
|---|---|
| selected source SHA (40-hex) | |
| selected binary SHA-256 (64-hex) | |
| seed 0 address (registered, then stopped) | |
| seed 1 address (live bootstrap) | |
| joiner node id / genesis | |
| window start / end (UTC) | |
| operator / reviewer | |
| evidence destination | |

## 4. Procedure

1. Bring up the network with seed 1 as its bootstrap; record its node id and advertised address.
2. Register seed 0 and stop it. Its address stays in the joiner's ordered list.
3. Start the joiner with the ordered list, first seed down:
   ```sh
   rnode run --bootstrap "rnode://<seed0-id>@<seed0-host>?protocol=40400&discovery=40404" \
             --bootstrap "rnode://<seed1-id>@<seed1-host>?protocol=40400&discovery=40404" \
             ...
   ```
4. Capture `joiner.log`; require the `bootstrap seed 2/2 connected:` line naming seed 1.
5. Wait for the joiner's `/api/last-finalized-block` to equal the bootstrap's.
6. Write the run directory `failover.json`:
   ```json
   {
     "seeds": ["<seed0>", "<seed1>"],
     "seed_status": ["down", "live"],
     "joined": true,
     "joined_via": "<seed1>",
     "finalised_head_agrees": true,
     "provenance": {"source_sha": "<40hex>", "binary_sha256": "<64hex>"}
   }
   ```
   Keep `joiner.log` beside it.
7. Run and pack:
   ```sh
   python3 tests/release-claim-4.py --arm failover --campaign-result <run dir> \
     --head <40hex> --out <record>
   python3 tools/release-check.py record --claim RC-4 --arm failover --record <record>
   python3 tools/check-evidence-locality.py --gate
   ```

## 5. Negative controls

- **The seeds reversed** (first live, second down): `joined_via` is the first seed, and the
  predicate FAILs the down-then-live order.
- **Both seeds down**: the joiner does not join; `joined` is false and the predicate FAILs.
- **One seed**: `not run`, because an ordered list needs two.
- **A loopback seed**: `not run`; the locality gate would refuse the record.
- **A joiner log without the fallback witness**: the observation is incomplete; do not infer
  `joined_via` from the configuration.

## 6. What this window does not establish

- It does not measure the multi-host arms, the public fleet or the off-host M3 join.
- It does not establish a slashing or peer-scoring policy; it measures selection across a down
  seed and a live seed.
- `execution_proven` is false; the digests prove the bytes, not the run.
