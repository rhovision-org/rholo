# The archive service and its economics

History below a checkpoint is a **service**. An archive holds the bodies, bears the disk and
bandwidth, and is paid for serving them. A consumer — the observer that does not backfill bodies
(`--no-backfill-bodies`), or an audit client — pays for what it asks for. The balance is deliberate:
bodies cost money and are metered; headers and checkpoint metadata are small, committed by the
certificate, and stay free, so joining the network does not become expensive.

A new light-client mode is **not** part of this design. The bodies-optional observer is the consumer
in scope; a header-only client is a post-mainnet question.

## What is metered, and what is not

| Object | Metered? | Why |
|---|---|---|
| Block bodies below the archive floor | **Yes** | Custody and bandwidth are the archive's cost |
| History range responses | **Yes** | Billed per byte, bounded per response |
| Headers, checkpoint metadata, certificates | No | Small, committed, and needed to join cheaply |
| The possession challenge | No, but capped and rate-limited | It is the integrity check on the paid service, not a substitute for it |

Peer-to-peer block and body replication remains the sync path: rate-limited and retention-bounded.
It is not a substitute for the paid on-demand tier, and the raw-body history routes below the floor
are gated by the same meter.

## Credit

Payment is the direct REV service fee — the decision of record (2026-11-15, owner: Patrick). No
consensus change, no new token, no new slashing rule, and no new native contract. The pieces are:

1. **Payment.** The consumer pays the archive's published REV vault directly. The transfer deploy id
   is recorded with the agreement.
2. **Receipt.** The archive's **settlement key** signs a `HistoryCredit` receipt bound to the payer,
   the archive, the period, the REV amount, the prepaid bytes, the invoice, the payment deploy id,
   and an expiry. The invoice is single-use.
3. **Deposit.** The consumer posts the receipt; the node verifies the settlement signature, the
   expiry and the invoice, and stores the prepaid bytes in a durable meter.
4. **Authorization.** Each range request carries a payer-signed `HistoryAuthorization` binding
   `(payer, archive, period, from, to, nonce, issuedAt)`. The node verifies it, refuses a reused
   nonce, debits the bytes **before** a body leaves, and serves at most 64 heights / 64 MiB per
   response.
5. **Modes.** `receipt` is the default; a receipt policy with no settlement key is a startup error,
   never a silent open archive. `allowance` admits named keys with a fixed byte allowance;
   `open` serves unmetered and logs that it does.

A missing body is reported per height as `null` and is not charged.

## Challenge and settlement

At the end of a period the consumer runs the possession challenge
([Sync, checkpoints and history](sync-and-history.md)). The outcome settles the period:

- **Pass** — unused credit carries over and the archive stays selected.
- **Fail** — the period's fee is forfeited (not renewed, remaining prepaid credit written off), the
  archive is marked, and it is excluded from selection until a cooldown. The direct REV payment is
  **not** reversed on-chain.

Neither outcome is a consensus event. A false claim by a bonded signer is attributable evidence
under the service relationship; it is not a new slashing rule.

## Fallbacks

| Situation | Result |
|---|---|
| No valid authorization | `402` with a fixed reason; the consumer tries the next archive |
| Rate limit exceeded | `429` with `Retry-After` |
| Failed possession challenge | The archive is not selected until its cooldown ends |
| No archive answers | The consumer reports `INCOMPLETE`; it never fabricates a body |

## Configuration

| Setting | Meaning |
|---|---|
| `api-server.archive` | Advertise and serve the archive tier |
| `api-server.history-retention` | Heights retained (`0` = all) |
| `--no-backfill-bodies` | Keep headers only below the floor (the consumer shape) |
| `archive-credit-policy` | `receipt` (default), `allowance`, `open` |
| `archive-settlement-key` | The key whose signature the node accepts on receipts |
| `archive-allow` | `key=bytes` allowances for the `allowance` policy |
| `archive-period-seconds` | Settlement period |
| `archive-rate-bytes-per-sec` | Per-peer serving rate |
| `archive-max-bytes-per-period` | A cap on one peer's period |
| `archive-open` | Explicitly unmetered; warns at startup |

Routes: `POST /api/v1/history/credit`, `POST /api/v1/history/range`, the existing
`GET /api/v1/history/challenge`, and the admin `GET /api/v1/history/meter`.

## What this does not claim

- The node verifies the settlement key's signature and expiry, **not** the on-chain transfer. The
  consumer trusts the archive it chose to pay; on-chain payment proof is future work.
- Forfeiture is service-level: non-renewal, credit write-off and exclusion. It is not a slash.
- A challenge proves possession at challenge time, not retention for ever.
- No scaling claim is made: a metered range is bounded by construction, not benchmarked as a
  throughput result.
