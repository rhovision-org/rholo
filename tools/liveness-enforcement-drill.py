#!/usr/bin/env python3
"""C344's liveness-enforcement devnet arms: jail, vote, partition, expiry.

Why this exists. `U21-01/04/09` and `H-RS-17` name the live/devnet arms as owed. The unit tests and
the Lean model pin the rules; these arms drive the real PoS surface on a disposable four-validator
devnet. Each arm is one behaviour:

  * `jail-vote` — stakes 700,100,100,100; stop validator 3; wait two advancing boundaries; read
    `getLiveness` until its record shows a jail, and `getBonds` to show the bond is untouched; then
    submit a signed `voteSlash` from validator 0, cross a boundary, and show the bond is removed.
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


def explore(term: str):
    reply = http_json(HTTP_BASE, "/api/v1/explore-deploy", method="POST", body=term)
    return [decode(item) for item in reply.get("expr", [])]


def read_value(term: str):
    values = explore(term)
    return values[0] if values else None


def read_map(term: str):
    values = explore(term)
    if not values:
        return {}
    value = values[0]
    if isinstance(value, list) and len(value) == 1 and isinstance(value[0], dict):
        value = value[0]
    return value if isinstance(value, dict) else {}


def liveness() -> dict:
    # The reply goes to the term's **first `new`-bound name**: exploratory deploy reads that name
    # and falls back to `@"out"` only when it is empty. `@"out"` accumulates every real deploy's
    # reply in the block's state, so a read there can return a stale map or an old vote ack.
    return read_map(
        'new reply, pos(`rho:rchain:pos`) in { pos!("getLiveness", [*reply]) }'
    )


def bonds() -> dict:
    return read_map(
        'new reply, pos(`rho:rchain:pos`) in { pos!("getBonds", [*reply]) }'
    )


def active_validators() -> list[str]:
    value = read_value(
        'new reply, pos(`rho:rchain:pos`) in { pos!("getActiveValidators", [*reply]) }'
    )
    return value if isinstance(value, list) else []


def vote_state(target_hex: str):
    """`getVotes` answers Nil when no window is open, else (opened_at, remaining, ballots)."""
    value = read_value(
        'new reply, pos(`rho:rchain:pos`) in { '
        f'pos!("getVotes", ["{target_hex}".hexToBytes(), *reply]) }}'
    )
    if isinstance(value, tuple) and len(value) == 3:
        return {"opened_at": value[0], "remaining": value[1], "ballots": value[2]}
    return None


def health() -> dict:
    return http_json(HTTP_BASE, "/health")


def latest_height() -> int:
    return int(http_json(HTTP_BASE, "/api/status").get("latestBlockNumber", 0))


def pos_status() -> dict:
    return http_json(HTTP_BASE, "/api/v1/pos")


def recent_blocks(depth: int = 100) -> list:
    body = http_json(HTTP_BASE, f"/api/blocks/{depth}")
    if isinstance(body, dict):
        body = body.get("blocks", [])
    return body if isinstance(body, list) else []


def find_block(sender_hex: str, min_height: int):
    wanted = sender_hex.lower()
    for block in recent_blocks():
        sender = str(block.get("sender", "")).lower()
        height = int(block.get("blockNumber", -1))
        if height >= min_height and (sender == wanted or wanted in sender):
            return block
    return None


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


def submit_signed(
    term: str,
    height: int,
    *,
    validator: int = 0,
    phlo_limit: int = VOTE_PHLO_LIMIT,
    phlo_price: int = 1,
) -> str:
    """Deploy `term` signed by `validator`'s own key, in that validator's container.

    The signer *is* the deployer id, so a ballot from validator 1 must be signed by validator 1's
    key; the CLI can reach the network from any validator container, so no key leaves its volume.
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
        "--shard-id", "/root", "--valid-after-block-number", str(max(height - 1, 0)),
        f"/tmp/{container}",
    ])
    match = re.search(r"DeployId is:\s*([0-9a-f]+)", deployed.stdout)
    if not match:
        raise RuntimeError(f"signed deploy rejected: {deployed.stdout[-1200:]}")
    return match.group(1)


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


def bootstrap_log() -> str:
    return docker(["logs", "--timestamps", f"{PREFIX}-bootstrap"]).stdout


def count_log(log: str, needle: str) -> int:
    return log.count(needle)


def run_jail_vote_arm(keys: list[str], args) -> tuple[dict, dict]:
    checks: dict[str, bool] = {}
    observations: dict = {}
    target = keys[3]
    baseline_bonds = bonds()
    observations["bonds_before"] = baseline_bonds
    baseline_stake = target_record(baseline_bonds, target)

    run(["tools/devnet.sh", "stop", "3"])
    record = wait_for_jail(target, args)
    observations["jail_record"] = record
    checks["jailed"] = int(record[1]) > 0
    checks["bond_untouched_by_jail"] = target_record(bonds(), target) == baseline_stake

    active = wait_until(
        lambda: (lambda a: a if a and target not in a else None)(active_validators()),
        args.timeout, args.poll, "the jailed validator leaves the active snapshot",
    )
    observations["active_after_jail"] = active
    checks["target_excluded_from_active_after_jail"] = target not in active

    window = wait_until(lambda: vote_state(target), args.timeout, args.poll, "vote window")
    observations["vote_window"] = window
    checks["vote_window_opened"] = window is not None and int(window["remaining"]) > 0

    submit_signed(vote_term(target), latest_height())
    propose()
    wait_until(
        lambda: True if target not in bonds() else None,
        args.timeout, args.poll, "passing vote slashes the bond",
    )
    observations["bonds_after_vote"] = bonds()
    checks["vote_slashed_the_bond"] = target not in bonds()
    checks["liveness_record_cleared_after_slash"] = target_record(liveness(), target) is None
    checks["vote_window_closed_after_slash"] = vote_state(target) is None
    return checks, observations


def run_exact_threshold_arm(keys: list[str], args) -> tuple[dict, dict]:
    checks: dict[str, bool] = {}
    observations: dict = {}
    target = keys[3]

    observations["second_voter_funding"] = fund_validator(keys[1])
    checks["second_voter_funded"] = observations["second_voter_funding"]["balance"] >= VOTE_FUNDING

    baseline_bonds = bonds()
    observations["bonds_before"] = baseline_bonds

    run(["tools/devnet.sh", "stop", "3"])
    record = wait_for_jail(target, args)
    observations["jail_record"] = record
    checks["jailed"] = int(record[1]) > 0

    active = wait_until(
        lambda: (lambda a: a if a and target not in a else None)(active_validators()),
        args.timeout, args.poll, "the jailed validator leaves the active snapshot",
    )
    observations["active_after_jail"] = active
    checks["target_excluded_from_active_after_jail"] = target not in active
    pool = bonds()
    active_bonds = {key: int(pool[key]) for key in active if key in pool}
    total_active = sum(active_bonds.values())
    observations["active_bonds"] = active_bonds
    observations["total_active"] = total_active
    checks["active_denominator_is_900"] = total_active == 900

    window = wait_until(lambda: vote_state(target), args.timeout, args.poll, "vote window")
    checks["vote_window_opens_with_margin"] = window is not None and int(window["remaining"]) >= 2

    submit_signed(vote_term(target), latest_height())
    propose()
    first = wait_until(
        lambda: (lambda w: w if w and w["ballots"].get(keys[0]) else None)(vote_state(target)),
        args.timeout, args.poll, "validator 0's ballot is recorded",
    )
    observations["votes_after_first_ballot"] = first
    checks["first_ballot_recorded"] = bool(first["ballots"].get(keys[0]))

    # A boundary must tally. The jail clock only moves at an advancing boundary, and the ballot is
    # already in state, so a decrement proves this ballot was in the tally.
    jail_at_ballot = target_record(liveness(), target)
    boundary = wait_until(
        lambda: (lambda r: r if r and jail_at_ballot and int(r[1]) < int(jail_at_ballot[1]) else None)(
            target_record(liveness(), target)),
        args.timeout, args.poll, "a boundary tallies the first ballot",
    )
    observations["jail_after_first_tally"] = boundary
    observations["bonds_after_first_tally"] = bonds()
    tally_window = vote_state(target)
    observations["votes_after_first_tally"] = tally_window
    first_for_stake = for_stake(tally_window["ballots"], active_bonds)
    observations["first_for_stake"] = first_for_stake
    checks["first_ballot_is_exactly_two_thirds"] = first_for_stake * 3 == total_active * 2
    checks["target_bond_unchanged_at_exactly_two_thirds"] = (
        target_record(bonds(), target) == target_record(baseline_bonds, target)
    )
    second_window = vote_state(target)
    checks["vote_window_still_open_after_first_tally"] = (
        second_window is not None and int(second_window["remaining"]) >= 2
    )

    submit_signed(vote_term(target), latest_height(), validator=1)
    propose()
    second = wait_until(
        lambda: (lambda w: w if w and w["ballots"].get(keys[1]) else None)(vote_state(target)),
        args.timeout, args.poll, "validator 1's ballot is recorded",
    )
    observations["votes_after_second_ballot"] = second
    checks["second_ballot_recorded"] = bool(second["ballots"].get(keys[1]))
    second_for_stake = for_stake(second["ballots"], active_bonds)
    observations["second_for_stake"] = second_for_stake
    checks["second_ballot_is_strict_supermajority"] = second_for_stake * 3 > total_active * 2

    wait_until(
        lambda: True if target not in bonds() else None,
        args.timeout, args.poll, "the strict supermajority slashes the bond",
    )
    observations["bonds_after_slash"] = bonds()
    checks["second_vote_slashed_the_bond"] = target not in bonds()
    checks["liveness_record_cleared_after_slash"] = target_record(liveness(), target) is None
    checks["vote_window_closed_after_slash"] = vote_state(target) is None
    return checks, observations


def run_stall_arm(keys: list[str], args) -> tuple[dict, dict]:
    checks: dict[str, bool] = {}
    observations: dict = {}
    for i in (1, 2, 3):
        run(["tools/devnet.sh", "stop", str(i)])

    # Wait for finality to stop moving, then observe a window during which it must stay stopped.
    frozen_height = wait_until_stable(lambda: health().get("finalizedHeight"), STALL_QUIESCE_S,
                                      args.timeout, args.poll, "finality quiesces below quorum")
    if frozen_height is None:
        raise RuntimeError("finalizedHeight was never readable while waiting for quiescence")
    before = {
        "health": health(),
        "pos": pos_status(),
        "liveness": liveness(),
        "bonds": bonds(),
    }
    time.sleep(STALL_WINDOW_S)
    after = {
        "health": health(),
        "pos": pos_status(),
        "liveness": liveness(),
        "bonds": bonds(),
    }
    observations["before"] = before
    observations["after"] = after
    observations["note"] = (
        "pos.epoch derives from DAG height, which the surviving proposer may keep extending; the "
        "quorum claim is measured on /health.finalizedHeight and the liveness counters."
    )
    checks["finalized_height_frozen_below_quorum"] = (
        before["health"].get("finalizedHeight") == after["health"].get("finalizedHeight")
    )
    checks["liveness_counters_frozen_below_quorum"] = before["liveness"] == after["liveness"]
    checks["no_jail_below_quorum"] = not any(
        len(record) > 1 and int(record[1]) > 0 for record in after["liveness"].values()
    )
    return checks, observations


def run_restart_expiry_arm(keys: list[str], args) -> tuple[dict, dict]:
    checks: dict[str, bool] = {}
    observations: dict = {}
    target = keys[3]
    baseline_bonds = bonds()
    observations["bonds_before"] = baseline_bonds
    baseline_stake = target_record(baseline_bonds, target)

    run(["tools/devnet.sh", "stop", "3"])
    first = wait_for_jail(target, args)
    observations["first_jail"] = first
    checks["first_jail_imposed"] = int(first[1]) > 0
    checks["bond_untouched_by_first_jail"] = target_record(bonds(), target) == baseline_stake

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
    signed_height = int(block.get("blockNumber", -1))
    observations["restarted_signed_height"] = signed_height
    wait_until(
        lambda: True if int(health().get("finalizedHeight", 0)) >= signed_height else None,
        args.timeout, args.poll, "the restarted validator's block reaches finality",
    )
    checks["restarted_block_finalized"] = True

    wait_until(
        lambda: True if target_record(liveness(), target) is None else None,
        args.timeout, args.poll, "the restarted validator is released",
    )
    observations["liveness_after_release"] = liveness()
    observations["bonds_after_release"] = bonds()
    checks["released_after_term"] = target_record(liveness(), target) is None
    released_active = wait_until(
        lambda: (lambda a: a if a and target in a else None)(active_validators()),
        args.timeout, args.poll, "the released validator is drawn again",
    )
    observations["active_after_release"] = released_active
    checks["target_reentered_active"] = target in released_active
    checks["bond_untouched_by_release"] = target_record(bonds(), target) == baseline_stake

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
    renewed = target_record(liveness(), target)
    observations["renewed_record"] = renewed
    observations["renewal_lines"] = count_log(bootstrap_log(), "renewed the jail on") - renewals_before
    checks["jail_renewed_when_still_absent"] = bool(renewed) and int(renewed[1]) > 0
    checks["bond_untouched_by_renewal"] = target_record(bonds(), target) == baseline_stake
    checks["target_not_drawn_while_jailed"] = target not in active_validators()
    return checks, observations


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

    try:
        checks, observations = ARMS[args.arm](keys, args)
    except (TimeoutError, RuntimeError) as exc:
        # A failed arm is a result, not a missing record: keep the evidence that shows why.
        observations = {"error": str(exc)}
        checks = {"arm_completed": False}

    try:
        log = docker(["logs", "--timestamps", f"{PREFIX}-bootstrap"])
        (raw / "bootstrap.log").write_text(log.stdout)
    except subprocess.CalledProcessError as exc:
        observations["log_error"] = str(exc)

    verdict_hint = "PASS" if checks and all(checks.values()) else "FAIL"
    result = {
        "tool": "liveness-enforcement-drill.py",
        "arm": args.arm,
        "arm_config": asdict(config),
        "source_sha": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
        "epoch_length": EPOCH_LENGTH,
        "checks": checks,
        "observations": observations,
        "verdict_hint": verdict_hint,
        "execution_proven": False,
    }
    (raw / "result.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if verdict_hint == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
