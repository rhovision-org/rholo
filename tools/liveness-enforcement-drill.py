#!/usr/bin/env python3
"""C344's liveness-enforcement devnet arms: jail, vote, partition, expiry.

Why this exists. `U21-01/04/09` and `H-RS-17` name the live/devnet arms as owed. The unit tests and
the Lean model pin the rules; these arms drive the real PoS surface on a disposable four-validator
devnet. Each arm is one behaviour:

  * `jail-vote` — stakes 700,100,100,100; stop validator 3; wait two advancing boundaries; read
    `getLiveness` until its record shows a jail, and `getBonds` to show the bond is untouched; then
    submit a signed `voteSlash` from validator 0, wait for its block to finalise and a boundary to
    tally it, and show the bond is removed.
  * `exact-threshold` — stakes 600,200,100,100; jail validator 3. The active snapshot after the jail
    excludes the jailed validator, so the tally denominator is 900 and validator 0's 600 is exactly
    two thirds: the boundary must **not** slash. Validator 1 then votes 200; 800/900 is strict and
    must slash. The second voter is funded from the devnet faucet (harness funding only).
  * `stall` — stakes 400,300,200,100; stop validators 1..3 so only 400/1000 of the bonded stake
    remains (below 2/3). Assert finality freezes, the liveness counters do not advance, and no jail
    is imposed. The surface is `/health.finalizedHeight`, **not** `/api/v1/pos`: that endpoint's
    `epoch` is derived from DAG height, which the surviving proposer may keep extending up to the
    proposer's finality bound.
  * `restart-expiry` — jail validator 3, restart it, and show a block it signs releases the jail;
    stop it again and show continued silence renews the term. The bond is untouched throughout.

**Every native-state read is pinned, and the merged view is a block's pre-state.** An exploratory
deploy with no block hash anchors at `exploration_anchor` — the **last finalised block**
(`casper/src/api/block_api_impl.rs:174-182`) — which moves between polls. Worse, a finalised *fringe*
can hold several blocks at one height: each block's own post-state is that block's view, and a
sibling that never saw a ballot does not carry its effect (the first run of this tool read a
non-slashing sibling and thought the slash had failed while the merged state had it). The drill
therefore reads the **pre-state of the latest block** through
`/api/v1/explore-deploy-by-block-hash` (`usePreStateHash: true`) — the merge of that block's
justifications, which is the state a later block builds on — and records that anchor's hash and
number with every observation. The exploratory reply must come from the term's first `new`-bound
name (`replySource == "firstPrivateName"`); a reply from `@"out"` is a stale read, because that
channel accumulates every deploy's output in the block's state
(`casper/src/runtime_manager.rs:2270-2305`).

**Every vote deploy is tracked to its block.** `rnode deploy` reports a deploy id; the drill polls
`/api/v1/deploy-status/{id}`, records whether the deploy processed and in which block, waits for that
block to finalise, and then asks two questions of the same ballot: was it in its **own block's**
post-state (the stale-voter discriminator), and is it in the **merged** state a later block builds
on (the merge-loss discriminator)? A `ProcessedWithError` is a rule-side refusal and is reported with
its message, not as a lost ballot. A window that closed before the observation is INCONCLUSIVE, not
FAIL.

The tool asserts and records; it does not author a verdict. It writes `raw/result.json`, the route
replies and the bootstrap journal. A failed assertion exits non-zero and the record is FAIL /
INCONCLUSIVE, never a pass.

Usage:
  python3 tools/liveness-enforcement-drill.py --arm jail-vote --out target/c344/jail-vote
  python3 tools/liveness-enforcement-drill.py --arm exact-threshold --out target/c344/exact-threshold
  python3 tools/liveness-enforcement-drill.py --arm stall --out target/c344/stall
  python3 tools/liveness-enforcement-drill.py --arm restart-expiry --out target/c344/restart-expiry
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from dataclasses import asdict, dataclass
from pathlib import Path

HTTP_BASE = int(os.environ.get("DEVNET_HTTP_BASE", "40403"))
ADMIN_BASE = int(os.environ.get("DEVNET_ADMIN_BASE", "40405"))
PREFIX = os.environ.get("DEVNET_PREFIX", "devnet")
ROOT = Path(__file__).resolve().parents[1]
EPOCH_LENGTH = 3
STALL_QUIESCE_S = 10.0
STALL_WINDOW_S = 30.0
VOTE_PHLO_LIMIT = 1_000_000
VOTE_FUNDING = 1_000_000
REPLY_SOURCE = "firstPrivateName"


@dataclass(frozen=True)
class ArmConfig:
    """The genesis parameters one arm needs. Stakes are per validator, in validator order."""

    stakes: str
    absence_threshold: int
    jail_after: int
    jail_epochs: int
    vote_window: int
    severity: str


ARM_CONFIG = {
    "jail-vote": ArmConfig("700,100,100,100", 1, 2, 10, 6, "misdemeanour"),
    "exact-threshold": ArmConfig("600,200,100,100", 1, 2, 24, 20, "misdemeanour"),
    "stall": ArmConfig("400,300,200,100", 1, 2, 10, 6, "misdemeanour"),
    # Voting disabled: the arm is about the jail clock, not the ballot. The window must be 0 when
    # the severity is `none` (the genesis validator refuses a half-configured chain).
    "restart-expiry": ArmConfig("100,100,100,100", 2, 2, 6, 0, "none"),
}


def http_json(port: int, path: str, method: str = "GET", body=None, timeout: float = 20.0):
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(
        f"http://127.0.0.1:{port}{path}", data=data, method=method,
        headers={"Content-Type": "application/json"} if data is not None else {},
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        raw = response.read()
        return json.loads(raw) if raw else {}


def wait_until(fn, timeout_s: float, poll_s: float, label: str):
    deadline = time.monotonic() + timeout_s
    last = None
    while time.monotonic() < deadline:
        try:
            last = fn()
            if last is not None:
                return last
        except (urllib.error.URLError, TimeoutError, json.JSONDecodeError, ConnectionError) as exc:
            last = {"error": str(exc)}
        time.sleep(poll_s)
    raise TimeoutError(f"{label}: no qualifying answer in {timeout_s:.0f}s; last={last}")


def wait_until_stable(fn, stable_s: float, timeout_s: float, poll_s: float, label: str):
    """Return the first value that has not changed for `stable_s`."""
    deadline = time.monotonic() + timeout_s
    last = object()
    since = time.monotonic()
    while time.monotonic() < deadline:
        try:
            value = fn()
        except (urllib.error.URLError, TimeoutError, json.JSONDecodeError, ConnectionError):
            value = None
        now = time.monotonic()
        if value != last:
            last = value
            since = now
        elif now - since >= stable_s:
            return value
        time.sleep(poll_s)
    raise TimeoutError(f"{label}: never stable for {stable_s:.0f}s in {timeout_s:.0f}s")


def run(argv: list[str], timeout: float | None = None) -> subprocess.CompletedProcess:
    return subprocess.run(argv, check=True, text=True, capture_output=True, timeout=timeout)


def docker(argv: list[str]) -> subprocess.CompletedProcess:
    return subprocess.run(["docker", *argv], check=True, text=True,
                          stdout=subprocess.PIPE, stderr=subprocess.STDOUT)


def decode(node):
    """Decode the node's RhoExpr wire shape into Python values."""
    if isinstance(node, list):
        return [decode(item) for item in node]
    if not isinstance(node, dict) or len(node) != 1:
        return node
    (tag, payload), = node.items()
    data = payload.get("data") if isinstance(payload, dict) else payload
    if tag in ("ExprPar", "ExprList", "ExprSet"):
        return [decode(item) for item in data]
    if tag == "ExprTuple":
        return tuple(decode(item) for item in data)
    if tag == "ExprMap":
        return {key: decode(value) for key, value in data.items()}
    if tag == "ExprInt":
        return data
    if tag == "ExprBool":
        return data
    if tag in ("ExprString", "ExprUri", "ExprBytes"):
        return data
    return node


# --- the pinned observation surface -------------------------------------------------------------


def last_finalized(port: int = HTTP_BASE) -> dict:
    """The last finalised block — the freeze surface, not the canonical state read.

    A finalised **fringe** can hold several blocks at one height, and each block's own post-state is
    that block's view; a sibling that never saw a ballot does not carry its effect. The canonical
    merged state is the pre-state of the latest block (the merge of its justifications), which is what
    `canonical_anchor` names.
    """
    body = http_json(port, "/api/last-finalized-block")
    info = body.get("blockInfo") or body.get("block_info") or {}
    anchor = {"hash": info.get("blockHash"), "number": info.get("blockNumber"), "port": port}
    if not anchor["hash"]:
        raise RuntimeError(f"no finalised block on port {port}: {body}")
    return anchor


def latest_block(port: int = HTTP_BASE) -> dict:
    body = http_json(port, "/api/blocks/1")
    blocks = body if isinstance(body, list) else body.get("blocks", [])
    if not blocks:
        raise RuntimeError(f"no blocks on port {port}")
    return max(blocks, key=lambda b: int(b.get("blockNumber", -1)))


def canonical_anchor(port: int = HTTP_BASE) -> dict:
    """The latest block's **pre-state**: the merged state its proposer built on.

    It is the state a block after the effect under test *sees*, so an effect a finalised sibling
    carried is present here even when the last-finalised block the node names is a sibling that did
    not carry it.
    """
    block = latest_block(port)
    return {
        "hash": block.get("blockHash"),
        "number": block.get("blockNumber"),
        "port": port,
        "pre": True,
    }


def explore_pinned(term: str, anchor: dict | None = None, port: int | None = None):
    """Run `term` against one block's post- or pre-state and return its reply.

    The block hash is explicit, so the read cannot slide to a later block between two polls;
    `replySource` is checked so a stale `@"out"` read is a harness failure rather than state.
    """
    if anchor is None:
        anchor = canonical_anchor(port or HTTP_BASE)
    target_port = port or anchor.get("port", HTTP_BASE)
    reply = http_json(
        target_port,
        "/api/v1/explore-deploy-by-block-hash",
        method="POST",
        body={
            "term": term,
            "blockHash": anchor["hash"],
            "usePreStateHash": bool(anchor.get("pre", False)),
        },
    )
    source = reply.get("replySource")
    if source != REPLY_SOURCE:
        raise RuntimeError(
            f"exploratory reply came from {source!r}, not the term's private channel "
            f"(anchor {anchor['hash']})"
        )
    return [decode(item) for item in reply.get("expr", [])]


def read_value(term: str, anchor: dict | None = None, port: int | None = None):
    values = explore_pinned(term, anchor, port)
    return values[0] if values else None


def read_map(term: str, anchor: dict | None = None, port: int | None = None):
    values = explore_pinned(term, anchor, port)
    if not values:
        return {}
    value = values[0]
    if isinstance(value, list) and len(value) == 1 and isinstance(value[0], dict):
        value = value[0]
    return value if isinstance(value, dict) else {}


def liveness(anchor: dict | None = None, port: int | None = None) -> dict:
    return read_map(
        'new reply, pos(`rho:rchain:pos`) in { pos!("getLiveness", [*reply]) }', anchor, port
    )


def bonds(anchor: dict | None = None, port: int | None = None) -> dict:
    return read_map(
        'new reply, pos(`rho:rchain:pos`) in { pos!("getBonds", [*reply]) }', anchor, port
    )


def active_validators(anchor: dict | None = None, port: int | None = None) -> list[str]:
    value = read_value(
        'new reply, pos(`rho:rchain:pos`) in { pos!("getActiveValidators", [*reply]) }',
        anchor,
        port,
    )
    return value if isinstance(value, list) else []


def vote_state(target_hex: str, anchor: dict | None = None, port: int | None = None):
    """`getVotes` answers Nil when no window is open, else (opened_at, remaining, ballots)."""
    value = read_value(
        'new reply, pos(`rho:rchain:pos`) in { '
        f'pos!("getVotes", ["{target_hex}".hexToBytes(), *reply]) }}',
        anchor,
        port,
    )
    if isinstance(value, tuple) and len(value) == 3:
        return {"opened_at": value[0], "remaining": value[1], "ballots": value[2]}
    return None


def snapshot(anchor: dict | None = None, port: int | None = None) -> dict:
    """Liveness, bonds and the active set read at **one** pinned canonical state."""
    anchor = anchor or canonical_anchor(port or HTTP_BASE)
    return {
        "anchor": anchor,
        "liveness": liveness(anchor, port),
        "bonds": bonds(anchor, port),
        "active": active_validators(anchor, port),
    }


def health(port: int = HTTP_BASE) -> dict:
    return http_json(port, "/health")


def latest_height(port: int = HTTP_BASE) -> int:
    return int(http_json(port, "/api/status").get("latestBlockNumber", 0))


def pos_status(port: int = HTTP_BASE) -> dict:
    return http_json(port, "/api/v1/pos")


def recent_blocks(depth: int = 50, port: int = HTTP_BASE) -> list:
    body = http_json(port, f"/api/blocks/{depth}")
    if isinstance(body, dict):
        body = body.get("blocks", [])
    return body if isinstance(body, list) else []


def find_block(sender_hex: str, min_height: int, port: int = HTTP_BASE):
    wanted = sender_hex.lower()
    for block in recent_blocks(port=port):
        sender = str(block.get("sender", "")).lower()
        height = int(block.get("blockNumber", -1))
        if height >= min_height and (sender == wanted or wanted in sender):
            return block
    return None


def is_finalized(block: dict, port: int = HTTP_BASE) -> bool:
    block_hash = block.get("blockHash")
    height = int(block.get("blockNumber", -1))
    if block_hash:
        try:
            return bool(http_json(port, f"/api/is-finalized/{block_hash}"))
        except urllib.error.HTTPError:
            pass
    return int(health(port).get("finalizedHeight") or 0) >= height


def validator_keys() -> list[str]:
    text = (ROOT / "tools" / "devnet.sh").read_text()
    block = re.search(r"VALIDATOR_PUB=\(\s*(.*?)\s*\)", text, re.S)
    if not block:
        raise RuntimeError("devnet.sh has no VALIDATOR_PUB table")
    return re.findall(r'"([0-9a-f]{130})"', block.group(1))


def target_record(records: dict, target_hex: str):
    for key, value in records.items():
        if target_hex in key or key in target_hex:
            return value
    return None


def target_is_jailed(records: dict, target_hex: str) -> bool:
    record = target_record(records, target_hex)
    return bool(record) and len(record) > 1 and int(record[1]) > 0


def node_container(validator: int = 0) -> str:
    return f"{PREFIX}-bootstrap" if validator == 0 else f"{PREFIX}-validator-{validator}"


# --- signed deploys and their outcome -----------------------------------------------------------


def submit_signed(
    term: str,
    *,
    validator: int = 0,
    phlo_limit: int = VOTE_PHLO_LIMIT,
    phlo_price: int = 1,
) -> dict:
    """Deploy `term` signed by `validator`'s own key, in that validator's container.

    The signer *is* the deployer id, so a ballot from validator 1 must be signed by validator 1's
    key; the CLI can reach the network from any validator container, so no key leaves its volume.
    The validity floor is left to the CLI, which resolves `-1` against **the signer's own** height;
    passing the bootstrap's height here made the deploy future-dated on a lagging signer.
    """
    with tempfile.NamedTemporaryFile("w", suffix=".rho", delete=False) as handle:
        handle.write(term)
        host_path = handle.name
    container = "c344-term.rho"
    docker(["cp", host_path, f"{node_container(validator)}:/tmp/{container}"])
    deployed = docker([
        "exec", node_container(validator), "rnode", "deploy",
        "--phlo-limit", str(phlo_limit), "--phlo-price", str(phlo_price),
        "--private-key-path", "/var/lib/rnode/validator.hex",
        "--shard-id", "/root",
        f"/tmp/{container}",
    ])
    match = re.search(r"DeployId is:\s*([0-9a-f]+)", deployed.stdout)
    if not match:
        raise RuntimeError(f"signed deploy rejected: {deployed.stdout[-1200:]}")
    return {
        "deploy_id": match.group(1),
        "validator": validator,
        "signer_port": HTTP_BASE + validator * 1000,
    }


def deploy_status(deploy: dict):
    """The signer's answer first, then the bootstrap's; `None` while nothing has processed it."""
    for port in (deploy["signer_port"], HTTP_BASE):
        try:
            body = http_json(port, f"/api/v1/deploy-status/{deploy['deploy_id']}")
        except urllib.error.HTTPError as exc:
            if exc.code in (400, 404):
                continue
            raise
        if not isinstance(body, dict) or len(body) != 1:
            continue
        (kind, payload), = body.items()
        if kind == "NotProcessed":
            continue
        return {"kind": kind, "payload": payload, "port": port}
    return None


def rejected_in_dag(deploy_id: str, depth: int = 50) -> list:
    """Blocks whose `rejectedDeploys` names this deploy — the merge-loss instrument.

    The node caps `/api/blocks/{depth}` at 50 ("Your request depth 100 exceed the max limit 50").
    """
    wanted = deploy_id.lower()
    found = []
    for block in recent_blocks(depth):
        ids = [str(item).lower() for item in block.get("rejectedDeploys", [])]
        if any(wanted == item or wanted in item or item in wanted for item in ids):
            found.append({"block": block.get("blockHash"), "height": block.get("blockNumber")})
    return found


def commit_deploy(deploy: dict, args, label: str) -> dict:
    """Wait for the deploy to process, refuse a rule-side error, and finalise its block."""
    status = wait_until(
        lambda: deploy_status(deploy), args.timeout, args.poll, f"{label}: deploy processes"
    )
    if status["kind"] == "ProcessedWithError":
        raise RuntimeError(
            f"{label}: the deploy was refused: {status['payload'].get('deployError')}"
        )
    block = status["payload"].get("block") or {}
    height = int(block.get("blockNumber", -1))
    if height < 0 or not block.get("blockHash"):
        raise RuntimeError(f"{label}: processed but the status names no block: {status}")
    wait_until(
        lambda: True if is_finalized(block) else None,
        args.timeout,
        args.poll,
        f"{label}: the processing block finalises",
    )
    return {
        "deploy_id": deploy["deploy_id"],
        "validator": deploy["validator"],
        "signer_port": deploy["signer_port"],
        "status": status["kind"],
        "block": block,
        "height": height,
        "rejected_in": rejected_in_dag(deploy["deploy_id"]),
    }


def wait_signer_synced(validator: int, min_height: int, args, label: str) -> dict:
    """The next signer's own finalised height must have reached the previous step's block."""
    port = HTTP_BASE + validator * 1000
    wait_until(
        lambda: True
        if int(health(port).get("finalizedHeight") or 0) >= min_height
        else None,
        args.timeout,
        args.poll,
        label,
    )
    return canonical_anchor(port)


def window_margin(config: ArmConfig) -> int:
    """Enough window left that the deploy can land and be tallied without racing its close."""
    if config.vote_window <= 0:
        return 0
    return min(5, max(2, config.vote_window // 2))


def propose() -> None:
    """Nudge the bootstrap; with `--autopropose` another prologue may hold the lock (HTTP 400)."""
    try:
        http_json(ADMIN_BASE, "/api/v1/propose", method="POST")
    except urllib.error.HTTPError as exc:
        if exc.code != 400:
            raise


def vote_term(target_hex: str) -> str:
    return (
        'new pos(`rho:rchain:pos`), deployerId(`rho:rchain:deployerId`), ret in { '
        f'pos!("voteSlash", [*deployerId, "{target_hex}".hexToBytes(), true, *ret]) | '
        'for (@r <- ret) { @"out"!(r) } }'
    )


def rev_address(pubkey_hex: str):
    return read_value(
        'new reply, ra(`rho:rev:address`) in { '
        f'ra!("fromPublicKey", "{pubkey_hex}".hexToBytes(), *reply) }}'
    )


def rev_balance(address: str) -> int:
    value = read_value(
        'new reply, vault(`rho:rchain:revVault`) in { '
        f'vault!("getBalance", "{address}", *reply) }}'
    )
    return value if isinstance(value, int) else 0


def fund_validator(pubkey_hex: str) -> dict:
    address = rev_address(pubkey_hex)
    if not isinstance(address, str):
        raise RuntimeError(f"validator {pubkey_hex[:16]} has no REV address")
    drip = http_json(HTTP_BASE, "/api/v1/faucet", method="POST", body={"address": address})
    wait_until(
        lambda: True if rev_balance(address) >= VOTE_FUNDING else None, 180.0, 1.0,
        "the faucet drip reaches the second voter",
    )
    return {"address": address, "balance": rev_balance(address), "drip": drip}


def wait_for_jail(target: str, args):
    def jailed_record():
        records = liveness()
        return target_record(records, target) if target_is_jailed(records, target) else None

    return wait_until(jailed_record, args.timeout, args.poll, "validator jailed")


def for_stake(ballots: dict, active_bonds: dict) -> int:
    """The active-snapshot stake behind the in-favour ballots."""
    return sum(
        int(active_bonds.get(voter, 0)) for voter, in_favor in ballots.items() if in_favor
    )


def ballot_keys(ballots: dict) -> list[str]:
    return [str(key) for key in ballots]


def bootstrap_log() -> str:
    return docker(["logs", "--timestamps", f"{PREFIX}-bootstrap"]).stdout


def count_log(log: str, needle: str) -> int:
    return log.count(needle)


def vote_once(
    target: str,
    voter: int,
    config: ArmConfig,
    args,
    *,
    after_height: int = 0,
    earlier_voters: tuple[int, ...] = (),
) -> dict:
    """Submit one signed ballot and follow it from the signer's status to the finalised state.

    `after_height` is the previous step's finalised block: the signer must have finalised at least
    that much before it builds the ballot. `earlier_voters` are the ballots this one must contain if
    the block that carried it was built on a current view — a block missing one is the stale-voter
    shape, not a merge loss.

    Returns the deploy record plus a two-anchor reading: `own_block_ballots` is the ballot map in
    the block that carried the deploy, and `merged_ballots` is the map a later block builds on
    anchor. The arm decides what a missing ballot means.
    """
    signer_anchor = wait_signer_synced(
        voter,
        max(0, after_height),
        args,
        f"validator {voter} has finalised the previous step",
    )
    window = vote_state(target, signer_anchor, HTTP_BASE + voter * 1000)
    record = {
        "window_at_submission": window,
        "signer_anchor": signer_anchor,
        "margin_required": window_margin(config),
    }
    if window is None:
        record["outcome"] = "window_closed_before_submission"
        return record
    if int(window["remaining"]) < window_margin(config):
        record["outcome"] = "window_margin_too_low"
        return record

    deploy = submit_signed(vote_term(target), validator=voter)
    propose()
    try:
        committed = commit_deploy(deploy, args, f"validator {voter}'s ballot")
    except RuntimeError as exc:
        # A rule-side refusal (the window closed, the target was released, the voter is not
        # active) is a precondition outcome, not a lost ballot: report it and let the arm decide.
        message = str(exc)
        if any(
            needle in message
            for needle in (
                "no vote window is open",
                "not jailed",
                "not a bonded validator",
                "only an active validator",
                "banished validator",
                "liveness voting is not enabled",
            )
        ):
            record["outcome"] = "refused_by_the_rule"
            record["refusal"] = message
            return record
        raise
    record.update(committed)
    record["outcome"] = "committed"

    own_anchor = {
        "hash": committed["block"].get("blockHash"),
        "number": committed["block"].get("blockNumber"),
        "port": committed["signer_port"],
        "pre": False,
    }
    try:
        own = vote_state(target, own_anchor, committed["signer_port"])
    except urllib.error.HTTPError:
        # The signer may not serve an exploratory read until it has indexed its own block; the
        # bootstrap has the same block once it validated it, and the reading is block-addressed.
        own = vote_state(target, own_anchor, HTTP_BASE)
    record["own_block_votes"] = own
    record["own_block_ballots"] = ballot_keys(own["ballots"]) if own else []
    own_bonds = bonds(own_anchor, committed["signer_port"])
    own_liveness = liveness(own_anchor, committed["signer_port"])
    record["own_block_target_absent"] = target_record(own_bonds, target) is None
    # A vote deploy can land in a **boundary** block, whose prelude tallies it in the same block: a
    # passing vote then consumes its own ballot and window before any post-state exists. That is the
    # rule working, not a lost ballot — the deploy's block shows the slash instead of the ballot.
    record["consumed_in_own_block"] = (
        own is None
        and record["own_block_target_absent"]
        and target_record(own_liveness, target) is None
    )
    record["stale_snapshot"] = (
        []
        if record["consumed_in_own_block"]
        else [
            earlier
            for earlier in earlier_voters
            if keys_missing(record["own_block_ballots"], earlier)
        ]
    )

    # The merged state a later block builds on. A block at or below the vote block would not see it,
    # so wait for the tip to move past it, then pin that block's pre-state.
    merged = wait_until(
        lambda: (lambda anchor: anchor if int(anchor["number"]) > committed["height"] else None)(
            canonical_anchor()
        ),
        args.timeout,
        args.poll,
        "a block above the ballot's block exists",
    )
    merged_window = vote_state(target, merged)
    record["merged_anchor"] = merged
    record["merged_votes"] = merged_window
    record["merged_ballots"] = ballot_keys(merged_window["ballots"]) if merged_window else []
    record["missing_ballot"] = (
        f"validator {voter}'s ballot is not in the merged ballot map"
        if merged_window is not None
        and not record["consumed_in_own_block"]
        and keys_missing(record["merged_ballots"], voter)
        else None
    )
    return record


def keys_missing(ballot_keys_: list[str], voter: int) -> bool:
    wanted = validator_keys()[voter].lower()
    return not any(wanted in key.lower() or key.lower() in wanted for key in ballot_keys_)


# --- the arms -----------------------------------------------------------------------------------


def run_jail_vote_arm(keys: list[str], args, config: ArmConfig) -> tuple[dict, dict, list[str]]:
    checks: dict[str, bool] = {}
    observations: dict = {}
    target = keys[3]
    baseline = snapshot()
    observations["baseline"] = baseline
    baseline_stake = target_record(baseline["bonds"], target)

    run(["tools/devnet.sh", "stop", "3"])
    record = wait_for_jail(target, args)
    observations["jail_record"] = record
    checks["jailed"] = int(record[1]) > 0

    jailed = snapshot()
    observations["after_jail"] = jailed
    checks["bond_untouched_by_jail"] = target_record(jailed["bonds"], target) == baseline_stake
    checks["target_excluded_from_active_after_jail"] = not any(
        target in key or key in target for key in jailed["active"]
    )

    window = wait_until(lambda: vote_state(target), args.timeout, args.poll, "vote window")
    observations["vote_window"] = window
    checks["vote_window_opened"] = window is not None and int(window["remaining"]) > 0

    vote = vote_once(target, 0, config, args)
    observations["vote"] = vote
    checks["vote_deploy_committed"] = vote.get("outcome") == "committed"
    checks["vote_deploy_not_rejected"] = not vote.get("rejected_in")
    if vote.get("outcome") != "committed":
        return checks, observations, [f"ballot not cast: {vote.get('outcome')}"]
    consumed = bool(vote.get("consumed_in_own_block"))
    checks["vote_ballot_in_own_block"] = consumed or not keys_missing(
        vote.get("own_block_ballots", []), 0
    )
    checks["vote_ballot_merged"] = consumed or (
        vote.get("merged_votes") is not None
        and not keys_missing(vote.get("merged_ballots", []), 0)
    )

    wait_until(
        lambda: True if target_record(bonds(), target) is None else None,
        args.timeout, args.poll, "passing vote slashes the bond",
    )
    after = snapshot()
    observations["after_slash"] = after
    checks["vote_slashed_the_bond"] = target_record(after["bonds"], target) is None
    checks["liveness_record_cleared_after_slash"] = target_record(after["liveness"], target) is None
    checks["vote_window_closed_after_slash"] = vote_state(target, after["anchor"]) is None
    inconclusive = []
    if vote.get("missing_ballot"):
        inconclusive.append(vote["missing_ballot"])
    return checks, observations, inconclusive


def run_exact_threshold_arm(keys: list[str], args, config: ArmConfig) -> tuple[dict, dict, list[str]]:
    checks: dict[str, bool] = {}
    observations: dict = {}
    inconclusive: list[str] = []
    target = keys[3]

    observations["second_voter_funding"] = fund_validator(keys[1])
    checks["second_voter_funded"] = observations["second_voter_funding"]["balance"] >= VOTE_FUNDING

    baseline = snapshot()
    observations["baseline"] = baseline
    baseline_stake = target_record(baseline["bonds"], target)

    run(["tools/devnet.sh", "stop", "3"])
    record = wait_for_jail(target, args)
    observations["jail_record"] = record
    checks["jailed"] = int(record[1]) > 0

    wait_until(
        lambda: (lambda a: a if a and not any(target in k or k in target for k in a) else None)(
            active_validators()
        ),
        args.timeout, args.poll, "the jailed validator leaves the active snapshot",
    )
    jailed = snapshot()
    observations["after_jail"] = jailed
    checks["target_excluded_from_active_after_jail"] = not any(
        target in key or key in target for key in jailed["active"]
    )
    pool = jailed["bonds"]
    active_bonds = {
        key: int(pool[key]) for key in jailed["active"] if key in pool
    }
    total_active = sum(active_bonds.values())
    observations["active_bonds"] = active_bonds
    observations["total_active"] = total_active
    checks["active_denominator_is_900"] = total_active == 900

    window = wait_until(
        lambda: (lambda w: w if w and int(w["remaining"]) >= window_margin(config) else None)(
            vote_state(target)
        ),
        args.timeout, args.poll, "vote window opens with margin",
    )
    observations["vote_window"] = window
    checks["vote_window_opens_with_margin"] = int(window["remaining"]) >= window_margin(config)

    jail_before_vote = int(target_record(liveness(), target)[1])
    first = vote_once(target, 0, config, args)
    observations["first_vote"] = first
    checks["first_vote_deploy_committed"] = first.get("outcome") == "committed"
    checks["first_vote_deploy_not_rejected"] = not first.get("rejected_in")
    if first.get("outcome") != "committed":
        inconclusive.append(f"first ballot not cast: {first.get('outcome')}")
        return checks, observations, inconclusive
    if first.get("merged_votes") is None:
        inconclusive.append("first window closed before the finalised read")
        return checks, observations, inconclusive
    checks["first_ballot_recorded"] = not keys_missing(first.get("merged_ballots", []), 0)

    # A boundary must advance the clock. The jail clock only moves at an advancing boundary; the
    # tally's content is asserted next, on the pinned state at that boundary: if the ballot were
    # lost, `first_ballot_is_exactly_two_thirds` would read an empty map.
    wait_until(
        lambda: True
        if (lambda rec: rec and int(rec[1]) < jail_before_vote)(
            target_record(liveness(), target)
        )
        else None,
        args.timeout, args.poll, "a boundary tallies the first ballot",
    )
    tallied = snapshot()
    observations["after_first_tally"] = tallied
    tally_window = vote_state(target, tallied["anchor"])
    observations["votes_after_first_tally"] = tally_window
    if tally_window is None:
        inconclusive.append("window closed before the first tally was observed")
        return checks, observations, inconclusive
    first_for_stake = for_stake(tally_window["ballots"], active_bonds)
    observations["first_for_stake"] = first_for_stake
    checks["first_ballot_is_exactly_two_thirds"] = first_for_stake * 3 == total_active * 2
    checks["target_bond_unchanged_at_exactly_two_thirds"] = (
        target_record(tallied["bonds"], target) == baseline_stake
    )
    checks["vote_window_still_open_after_first_tally"] = (
        tally_window is not None and int(tally_window["remaining"]) >= 2
    )

    second = vote_once(
        target, 1, config, args, after_height=first["height"], earlier_voters=(0,)
    )
    observations["second_vote"] = second
    checks["second_vote_deploy_committed"] = second.get("outcome") == "committed"
    checks["second_vote_deploy_not_rejected"] = not second.get("rejected_in")
    if second.get("outcome") != "committed":
        inconclusive.append(f"second ballot not cast: {second.get('outcome')}")
        return checks, observations, inconclusive
    observations["second_voter_stale_snapshot"] = second.get("stale_snapshot")
    if second.get("consumed_in_own_block"):
        # The second vote landed in a boundary block and its tally passed there: the block shows the
        # slash instead of the ballot. The threshold is then the ballots' known weights, and the
        # slash is the execution's own evidence.
        expected = int(target_record(active_bonds, keys[0]) or 0) + int(
            target_record(active_bonds, keys[1]) or 0
        )
        observations["second_for_stake_expected"] = expected
        checks["second_ballot_counted_in_its_block"] = True
        checks["second_ballot_weights_are_strict_and_the_vote_slashed"] = (
            expected * 3 > total_active * 2
        )
    else:
        if second.get("merged_votes") is None:
            inconclusive.append("second window closed before the merged read")
            return checks, observations, inconclusive
        checks["second_ballot_recorded"] = not keys_missing(second.get("merged_ballots", []), 1)
        checks["both_ballots_in_merged_state"] = not keys_missing(
            second.get("merged_ballots", []), 0
        ) and not keys_missing(second.get("merged_ballots", []), 1)
        second_for_stake = for_stake(second["merged_votes"]["ballots"], active_bonds)
        observations["second_for_stake"] = second_for_stake
        checks["second_ballot_is_strict_supermajority"] = second_for_stake * 3 > total_active * 2

    wait_until(
        lambda: True if target_record(bonds(), target) is None else None,
        args.timeout, args.poll, "the strict supermajority slashes the bond",
    )
    after = snapshot()
    observations["after_slash"] = after
    checks["second_vote_slashed_the_bond"] = target_record(after["bonds"], target) is None
    checks["liveness_record_cleared_after_slash"] = target_record(after["liveness"], target) is None
    checks["vote_window_closed_after_slash"] = vote_state(target, after["anchor"]) is None
    if second.get("missing_ballot"):
        inconclusive.append(second["missing_ballot"])
    return checks, observations, inconclusive


def run_stall_arm(keys: list[str], args, config: ArmConfig) -> tuple[dict, dict, list[str]]:
    checks: dict[str, bool] = {}
    observations: dict = {}
    for i in (1, 2, 3):
        run(["tools/devnet.sh", "stop", str(i)])

    # Wait for finality to stop moving, then observe a window during which it must stay stopped.
    frozen_height = wait_until_stable(lambda: health().get("finalizedHeight"), STALL_QUIESCE_S,
                                      args.timeout, args.poll, "finality quiesces below quorum")
    if frozen_height is None:
        raise RuntimeError("finalizedHeight was never readable while waiting for quiescence")
    before = snapshot()
    before["health"] = health()
    before["pos"] = pos_status()
    before["finalized_anchor"] = last_finalized()
    time.sleep(STALL_WINDOW_S)
    after = snapshot()
    after["health"] = health()
    after["pos"] = pos_status()
    after["finalized_anchor"] = last_finalized()
    observations["before"] = before
    observations["after"] = after
    observations["note"] = (
        "pos.epoch derives from DAG height, which the surviving proposer may keep extending; the "
        "quorum claim is measured on /health.finalizedHeight, the last-finalised anchor and the "
        "liveness counters. The canonical read anchor may advance with DAG blocks below quorum; "
        "the freeze claim is the finalised one."
    )
    checks["finalized_height_frozen_below_quorum"] = (
        before["health"].get("finalizedHeight") == after["health"].get("finalizedHeight")
    )
    checks["finalized_anchor_unchanged_below_quorum"] = (
        before["finalized_anchor"].get("hash") == after["finalized_anchor"].get("hash")
    )
    checks["liveness_counters_frozen_below_quorum"] = before["liveness"] == after["liveness"]
    checks["no_jail_below_quorum"] = not any(
        len(record) > 1 and int(record[1]) > 0 for record in after["liveness"].values()
    )
    return checks, observations, []


def run_restart_expiry_arm(keys: list[str], args, config: ArmConfig) -> tuple[dict, dict, list[str]]:
    checks: dict[str, bool] = {}
    observations: dict = {}
    target = keys[3]
    baseline = snapshot()
    observations["baseline"] = baseline
    baseline_stake = target_record(baseline["bonds"], target)

    run(["tools/devnet.sh", "stop", "3"])
    first = wait_for_jail(target, args)
    observations["first_jail"] = first
    jailed = snapshot()
    observations["after_first_jail"] = jailed
    checks["first_jail_imposed"] = int(first[1]) > 0
    checks["bond_untouched_by_first_jail"] = target_record(jailed["bonds"], target) == baseline_stake

    restart_height = latest_height()
    observations["restart_height"] = restart_height
    run(["tools/devnet.sh", "start", "3"])
    wait_until(
        lambda: http_json(HTTP_BASE + 3 * 1000, "/api/v1/status") or None,
        args.timeout, args.poll, "validator 3 serves again",
    )
    block = wait_until(
        lambda: find_block(target, restart_height), args.timeout, args.poll,
        "validator 3 signs a block after the restart",
    )
    observations["restarted_block"] = block
    checks["restarted_validator_spoke"] = block is not None
    wait_until(
        lambda: True if is_finalized(block) else None,
        args.timeout, args.poll, "the restarted validator's block reaches finality",
    )
    checks["restarted_block_finalized"] = True

    wait_until(
        lambda: True if target_record(liveness(), target) is None else None,
        args.timeout, args.poll, "the restarted validator is released",
    )
    released = snapshot()
    observations["after_release"] = released
    checks["released_after_term"] = target_record(released["liveness"], target) is None
    checks["target_reentered_active"] = any(
        target in key or key in target for key in released["active"]
    )
    checks["bond_untouched_by_release"] = target_record(released["bonds"], target) == baseline_stake

    renewals_before = count_log(bootstrap_log(), "renewed the jail on")
    run(["tools/devnet.sh", "stop", "3"])
    second = wait_for_jail(target, args)
    observations["second_jail"] = second
    checks["second_jail_imposed"] = int(second[1]) > 0

    # Stay absent through the term: the jail renews rather than releasing.
    wait_until(
        lambda: True if count_log(bootstrap_log(), "renewed the jail on") > renewals_before
        else None,
        args.timeout, args.poll, "the jail renews while the validator stays silent",
    )
    renewed = snapshot()
    observations["after_renewal"] = renewed
    observations["renewal_lines"] = count_log(bootstrap_log(), "renewed the jail on") - renewals_before
    records = renewed["liveness"]
    checks["jail_renewed_when_still_absent"] = target_is_jailed(records, target)
    checks["bond_untouched_by_renewal"] = target_record(renewed["bonds"], target) == baseline_stake
    checks["target_not_drawn_while_jailed"] = not any(
        target in key or key in target for key in renewed["active"]
    )
    return checks, observations, []


ARMS = {
    "jail-vote": run_jail_vote_arm,
    "exact-threshold": run_exact_threshold_arm,
    "stall": run_stall_arm,
    "restart-expiry": run_restart_expiry_arm,
}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--arm", choices=sorted(ARM_CONFIG), default="jail-vote")
    parser.add_argument("--out", required=True)
    parser.add_argument("--skip-up", action="store_true")
    parser.add_argument("--poll", type=float, default=1.0)
    parser.add_argument("--timeout", type=float, default=20 * 60)
    args = parser.parse_args()

    out = Path(args.out)
    raw = out / "raw"
    raw.mkdir(parents=True, exist_ok=False)
    keys = validator_keys()
    config = ARM_CONFIG[args.arm]

    if not args.skip_up:
        run(["tools/devnet.sh", "up", "--validators", "4", "--fresh", "--autopropose", "--admin",
             "--epoch-length", str(EPOCH_LENGTH), "--stakes", config.stakes,
             "--liveness-absence-threshold", str(config.absence_threshold),
             "--liveness-jail-after-boundaries", str(config.jail_after),
             "--liveness-jail-epochs", str(config.jail_epochs),
             "--liveness-vote-window", str(config.vote_window),
             "--liveness-slash-severity", config.severity])

    def ready():
        body = http_json(HTTP_BASE, "/health")
        return body if body.get("ready") and body.get("finalizedHeight") else None

    wait_until(ready, args.timeout, args.poll, "devnet ready")

    inconclusive: list[str] = []
    try:
        checks, observations, inconclusive = ARMS[args.arm](keys, args, config)
    except (TimeoutError, RuntimeError) as exc:
        # A failed arm is a result, not a missing record: keep the evidence that shows why.
        observations = {"error": str(exc)}
        checks = {"arm_completed": False}

    try:
        log = docker(["logs", "--timestamps", f"{PREFIX}-bootstrap"])
        (raw / "bootstrap.log").write_text(log.stdout)
    except subprocess.CalledProcessError as exc:
        observations["log_error"] = str(exc)

    if checks and not all(checks.values()):
        verdict_hint = "FAIL"
    elif inconclusive:
        verdict_hint = "INCONCLUSIVE"
    elif checks:
        verdict_hint = "PASS"
    else:
        verdict_hint = "FAIL"
    result = {
        "tool": "liveness-enforcement-drill.py",
        "arm": args.arm,
        "arm_config": asdict(config),
        "source_sha": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
        "epoch_length": EPOCH_LENGTH,
        "checks": checks,
        "observations": observations,
        "inconclusive": inconclusive,
        "verdict_hint": verdict_hint,
        "execution_proven": False,
    }
    (raw / "result.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if verdict_hint == "PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
