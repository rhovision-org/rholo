# The history chain

**A genesis-only, read-only chain carrying the REV allocation — dated 13 May 2026, frozen at that state
until it is updated with the documented final transactions.**

Unlike [the public testnet](testnet.md), this chain has **one block and never produces another**. It is a
public, queryable record rather than a network: no proposer, no peers, no finality, and no way to write to
it. It exists so that an address's allocation can be read from a chain instead of from a spreadsheet.

| | |
|---|---|
| Endpoint | **https://history.rhobot.net** (nginx → the node's HTTP API on the loopback) |
| Network id | `history` (its own, so it cannot be confused with `testnet`) |
| Chain | genesis block `f6aaa149…`, block number **0**, and nothing above it |
| Wallets | **13,946** REV addresses, **908,962,714 REV** in total (at 1e-8 precision) |
| Source of the allocation | [the allocation spreadsheet](https://docs.google.com/spreadsheets/d/1bLEn7WpgESNp8Hp8N9_-5Yf6XXRBMM5KbsH_-G4Fhks/edit) — `address, balance` |
| Host | `rhobot-2` (`138.197.65.34`), unit `rnode-history.service`, data `/var/lib/rnode-history` |

Short hashes here are the first twelve hex characters of the value they name. **The node id is not written
down** — a rebuild regenerates it, which is why [the testnet page](testnet.md) stopped naming its own; read
it from `GET /api/status` → `address`.

## Reading it: r-wallet

**R Wallet has this chain in its node dropdown** — choose **RChain → History (REV allocation)**. The
wallet talks to `https://history.rhobot.net` directly, so its Balance page answers for whatever address
is active; there is nothing to configure and no custom node to add.

What does *not* work there is deploying, by design: the deploy is refused with `403` and a plain-text
reason, because this chain has no proposer. Pick it to read an allocation, not to write one.

## Reading it: curl

The term below is **the wallet's own** (`src/utils/rho.ts`, `fn_check_balance` in the r-wallet repo), so
the two agree by construction:

```bash
ADDR=11112We8VJbQ…                 # any REV address

# The body is a bare JSON *string*, so the term's quotes are escaped — and the heredoc
# delimiter is quoted, so the shell leaves the backslashes exactly as written. (A
# `printf` with the same format string does *not* work: it reads `\"` as `"`, the body
# stops being valid JSON, and the node answers `expected variable, got Eof`.)
sed "s/ADDRESS/$ADDR/" > /tmp/balance.json <<'EOF'
"new return, vault(`rho:rchain:revVault`), ret in { vault!(\"getBalance\", \"ADDRESS\", *ret) | for (@b <- ret) { return!(b) } }"
EOF

curl -s -X POST https://history.rhobot.net/api/explore-deploy \
  -H 'Content-Type: application/json' --data-binary @/tmp/balance.json
```

Current source serializes the balance as
`{"expr":[{"ExprInt":{"data":<balance>}}],"block":{..."blockNumber":0...}}`.
Earlier binaries used the bare `{"ExprInt":<balance>}` form. Record the installed binary version
and the raw response before deciding which shape the history service uses.

```bash
# Save the response before parsing; Python preserves JSON integer precision.
curl -fsS --max-time 30 -X POST https://history.rhobot.net/api/explore-deploy \
  -H 'Content-Type: application/json' --data-binary @/tmp/balance.json \
  -o /tmp/balance-response.json

python3 - /tmp/balance-response.json <<'PY_BALANCE'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    response = json.load(source)

expr = response.get("expr")
if not isinstance(expr, list) or len(expr) != 1 or not isinstance(expr[0], dict):
    raise SystemExit("Expected exactly one balance expression")
if "ExprInt" not in expr[0]:
    raise SystemExit("Expected ExprInt")
balance = expr[0]["ExprInt"]
if isinstance(balance, dict):
    balance = balance.get("data")
if type(balance) is not int or balance < 0:
    raise SystemExit("Expected a non-negative integer balance")
block_number = response.get("block", {}).get("blockNumber")
if type(block_number) is not int or block_number != 0:
    raise SystemExit("Expected history genesis at block number 0")
print(balance)
PY_BALANCE

# what the chain is, and the genesis block
curl -fsS https://history.rhobot.net/api/status
curl -fsS https://history.rhobot.net/api/blocks/1
```

The number printed is in the chain's smallest unit, 1e-8 REV. Keep balances as exact integers:
ordinary JavaScript `JSON.parse` followed by `Number` arithmetic cannot represent every integer
above `9007199254740991`. Converting that rounded value to `BigInt` does not restore precision.

`0` is a zero balance, not an error. It can mean either a vault with zero balance or no vault; this
query does not distinguish the two. Post the body as a *bare JSON string* (the rholang term); an object
gets `invalid type: map, expected a string`.

**Sampled verification:** 8 of 8 sampled addresses return the sheet's own numbers — random draws plus
the largest allocation, the smallest non-zero, a balance of `1` and two zero balances — and a valid REV
address that is *not* in the sheet returns `0`. Every answer comes from the genesis block
(`blockNumber: 0`). This is sampled verification, not a reconciliation of all 13,946 allocations, and it
does not establish the historical accuracy of the spreadsheet.

## What "read-only" means here, precisely

Three separate things, and it is worth knowing which one is doing the work:

1. **The node has no proposer.** It runs as genesis master (`-s`) exactly once, to build the block from
   the wallets file, and then never again: no `--propose-on-deploy`, no `--autopropose`. Nothing is ever
   mined, so a deploy would sit in the pool forever. This is the real mechanism.
2. **The edge refuses the write routes.** nginx answers `403` for `/api/deploy`, `/api/faucet`,
   `/api/txn` and `/api/v1/deploy`, so a caller is told the chain is read-only rather than being left to
   infer it from a deploy that never completes. `explore-deploy` and `data-at-name` are exploratory and
   change nothing, so they stay.
3. **There is no finality.** `GET /api/last-finalized-block` answers
   `"Finalized fringe is not available."` and always will — finality needs blocks to finalise and there
   are none. Reads anchor to the genesis block, which is the whole state.

So the chain is not a small testnet with writes switched off; it is a **snapshot with a read API**.

## How it was built

`wallets.txt` is derived from the allocation spreadsheet, in the form
[the vault parser](https://github.com/rhovision-org/rholo/blob/dev/casper/src/vault_parser.rs)
accepts — `<REV_address>,<balance>`, one per line:

- the sheet's `balance` column carries **thousands separators** (`1,000,000`); the parser wants bare
  digits, so the commas are stripped;
- the sheet's third column (`$0.004`) is not part of the chain;
- balances are used **as they are** — no scaling. The reported total is 908,962,714 REV at 1e-8
  precision. Confirm the source column's unit independently and record the exact smallest-unit
  sum; a plausible total alone does not prove the unit;
- the reported input has 13,946 unique addresses, including 209 zero balances. The parser also
  checks Base58 decoding, address length, prefix and checksum through `RevAddress::parse`.

**Read that total as the spreadsheet's, not as the genesis's.** The figure above is what the 13 May
2026 allocation lists — 13,946 addresses and 908,962,714 REV. The genesis a node builds from that file
credits one vault the file does not name: the developer allocation, `max(20 000 000 REV, 2% of the
total)`, which on this allocation is the floor — 20,000,000 REV — for a wallets-plus-allocation subtotal
of **928,962,714 REV** ([Validator economics](validator-economics.md); Law 78). The addresses and balances
in the table are unchanged; the difference is one vault added on top. That subtotal is not all the REV a
genesis writes: the staking vault is funded separately with the bond sum.
  It does not reject duplicate addresses; the import audit must check uniqueness separately.

The genesis inputs are the ordinary two files:

```
/var/lib/rnode-history/genesis/wallets.txt   13,946 lines, from the spreadsheet
/var/lib/rnode-history/genesis/bonds.txt     one validator, stake 1000
```

and the validator key is a raw 64-hex secret at `/etc/rnode-history/validator.key` (the format the port
reads: `rnode keygen` cannot be used non-interactively — it loops on a password prompt and segfaults
without a TTY — so the key is generated directly and the public half derived from it).

PoS parameters: `--epoch-length 10 --quarantine-length 10 --bond-minimum 1 --bond-maximum 1000000
--number-of-active-validators 10`. They are written into the genesis block and, on a chain that never
advances, nothing reads them again.

A 13,946-vault genesis builds in well under a minute: the data directory is 3.7 MB and the node settles at
~54 MB RSS.

## Operating it

```
systemctl status rnode-history      # on rhobot-2
tail -f /var/log/rnode-history.log  # the node's log (this host's journald does not capture it)
```

Ports are **43400-based** (`--protocol-port 43400`, `--api-port-http 43403`, …) because the playground node
on the same host holds the whole 40400 family and the testnet join holds 42400. Only 80/443 are exposed;
nginx proxies the API from `127.0.0.1:43403`.

A changed allocation produces a **different genesis block**, and therefore a different chain.
Preserve the current snapshot while preparing its replacement:

1. Archive the source export, `wallets.txt`, `bonds.txt`, effective configuration and binary digest.
   Record the full genesis hash and post-state hash. Keep private keys out of public audit artifacts.
2. Keep a canonical copy of the genesis inputs outside `/var/lib/rnode-history`. The paths above
   are inside that data directory: deleting the whole directory also deletes the inputs.
3. Build the candidate in a separate empty data directory, with explicit paths to its genesis inputs.
   Do not overwrite the running snapshot.
4. Reconcile every source row against the candidate genesis using exact integers and the full block
   hash. Record count, zero balances, duplicates, exact total and all mismatches.
5. After verification and maintainer review, switch the service configuration to the new snapshot.
   Preserve the previous version and record which new genesis supersedes it.

The release record should identify the source export hash and allocation effective date separately
from export time, the node commit and binary digest, genesis parameters, full genesis and post-state
hashes, and the reconciliation report. Any final-transaction update needs a versioned input with
unique transaction identifiers, evidence, deterministic ordering and explicit accounting rules.
