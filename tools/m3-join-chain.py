#!/usr/bin/env python3
"""m3-join-chain — the typed six-stage witness for #428 M3.

M3 asks for an off-host observer that "receives and applies the approved fringe and hydrates
state", with a send log explicitly refused. `M3-14-journal.md` names the six stages and the
keying rule: join the two logs by peer identity and anchor (or state path), never by a send line.

This tool makes those rules executable twice over:

  * `build_chain` correlates the joiner's and the responder's captured logs into `chain.json`;
  * `check_chain` validates a chain — every stage present and typed, every field agreeing with the
    log line it quotes, stage 4 carrying a transport *outcome*, stage 6 carrying the served and
    restored roots plus a completed page walk.

The chain is a **correlation**, not a cryptographic delivery receipt. Stage 4 is the joiner's gRPC
receiver acknowledgement (`grpc_transport_receiver.rs` acks after spawning the dispatch), not the
handler finishing; stages 5 and 6 are the joiner-side witnesses. A local-devnet chain is never an
off-host positive, and `check --locality off-host` is what refuses it.

Usage:
  tools/m3-join-chain.py check --record DIR --locality off-host|local-devnet|any
  tools/m3-join-chain.py check-chain --chain FILE
  tools/m3-join-chain.py build --joiner-log FILE --responder-log FILE --out FILE
                             [--locality local-devnet] [--joiner-container NAME]
                             [--responder-container NAME]

Exit codes mirror `gate-check.py`: 0 PASS, 1 FAIL, 3 PRECONDITION (a chain is absent or a stage
cannot be keyed).
"""
from __future__ import annotations

import argparse
import datetime as dt
import importlib.util
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
STAGE_ORDER = (
    "1_request",
    "2_receipt",
    "3_construction",
    "4_transport",
    "5_receive",
    "6_apply",
)
EVENTS = {
    "1_request": "request_sent",
    "2_receipt": "request_received",
    "3_construction": "response_constructed",
    "4_transport": "transport_outcome",
    "5_receive": "response_received",
    "6_apply": "state_restored",
}
RECORD_LABELS = (
    "off-host-claim",
    "loopback-diagnostic",
    "local-devnet",
    "single-host",
    "public-live",
    "live-host",
    "external",
    "not-a-run",
)
POLICIES = ("off-host", "local-devnet", "any")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
HEX_KEY = re.compile(r"^[0-9a-fA-F]+$")
ANCHOR = re.compile(r"^(latest|[0-9a-f]{64})$")

# One capture group per field the chain names. Group order is documented at each use.
RE1 = re.compile(
    r"LFS fringe request (\d+)/(\d+) sent to peer ([0-9a-fA-F]+) "
    r"\(anchor ([^)]+)\); awaiting admitted reply"
)
RE2 = re.compile(
    r"Received FinalizedFringeRequest from (rnode://([0-9a-fA-F]+)@[^\s(]+) "
    r"\(requested anchor ([^)]+)\)"
)
RE3 = re.compile(
    r"Fringe response prepared for (rnode://([0-9a-fA-F]+)@\S+?): "
    r"state ([0-9a-f]{64}), requested anchor ([^,]+), (\d+) block\(s\) "
    r"\[([0-9a-f]{64}) \.\. ([0-9a-f]{64})\]"
)
RE4_OK = re.compile(
    r"FinalizedFringe sent to (rnode://([0-9a-fA-F]+)@\S+?): "
    r"transport outcome (delivered|failed) \(state ([0-9a-f]{64})\)"
)
RE4_FAIL = re.compile(
    r"FinalizedFringe to (rnode://([0-9a-fA-F]+)@\S+?) was not sent: "
    r"transport outcome (delivered|failed) \(state ([0-9a-f]{64})\)"
)
RE5 = re.compile(
    r"Received finalized fringe from bootstrap node \(([0-9a-f]{64})\) "
    r"(.*?) from peer ([0-9a-fA-F]+)\."
)
RE6_WALK = re.compile(
    r"LFS tuple-space walk finished for served state ([0-9a-f]{64}): "
    r"(\d+) of (\d+) page key\(s\) done\."
)
RE6_RESTORE = re.compile(
    r"LFS state is successfully restored\. Served state ([0-9a-f]{64}), "
    r"accepted run root ([0-9a-f]{64}) \((genesis|checkpoint)\), "
    r"approved checkpoint read back\."
)


def _module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


_locality = _module(
    "m3_join_chain_locality", ROOT / "tools" / "check-evidence-locality.py"
)


def declared_host(advertised: str | None) -> str | None:
    """The host literal of an `rnode://<key>@<host>?…` address (bracketed or bare IPv6 included)."""
    if not isinstance(advertised, str):
        return None
    match = re.match(r"rnode://[^@]+@(.+?)(?:\?|$)", advertised)
    return match.group(1) if match else None


def _nonempty(value: object) -> bool:
    return isinstance(value, str) and bool(value)


def _key(value: object) -> bool:
    return (
        isinstance(value, str)
        and bool(HEX_KEY.match(value))
        and len(value) % 2 == 0
    )


def _hex64(value: object) -> bool:
    return isinstance(value, str) and bool(HEX64.match(value))


def _anchor(value: object) -> bool:
    return isinstance(value, str) and bool(ANCHOR.match(value))


def _same(left: object, right: object) -> bool:
    if isinstance(left, str) and isinstance(right, str):
        return left.lower() == right.lower()
    return left == right


def _stage_problem(problems: list[str], name: str, stage: object) -> bool:
    """Common shape checks; returns whether the stage is usable for deeper checks."""
    if not isinstance(stage, dict):
        problems.append(f"{name}: the stage is not an object")
        return False
    event = stage.get("event")
    if event == "send_log":
        problems.append(f"{name}: a send log is not a stage witness")
    elif event != EVENTS[name]:
        problems.append(f"{name}: event {event!r} is not {EVENTS[name]!r}")
    for field in ("peer", "anchor", "source", "log_line"):
        if not _nonempty(stage.get(field)):
            problems.append(f"{name}: {field} is absent or empty")
    if not _anchor(stage.get("anchor")):
        problems.append(f"{name}: anchor {stage.get('anchor')!r} is neither 'latest' nor 64-hex")
    source = stage.get("source")
    if _nonempty(source) and ".rs::" not in source:
        problems.append(f"{name}: source {source!r} does not name a .rs symbol")
    line = stage.get("log_line")
    if not _nonempty(line):
        problems.append(f"{name}: log_line is absent")
        return False
    return True


def _stage1(problems: list[str], stage: dict) -> dict | None:
    if not _stage_problem(problems, "1_request", stage):
        return None
    match = RE1.search(stage["log_line"])
    if not match:
        problems.append("1_request: the log line is not the requester's sent line")
        return None
    attempt, attempts, peer, anchor = match.groups()
    if not _same(stage.get("peer"), peer):
        problems.append(f"1_request: peer {stage.get('peer')!r} != line's {peer!r}")
    if not _same(stage.get("anchor"), anchor):
        problems.append(f"1_request: anchor {stage.get('anchor')!r} != line's {anchor!r}")
    if _nonempty(stage.get("attempt")) and stage["attempt"] != f"{attempt}/{attempts}":
        problems.append(f"1_request: attempt {stage.get('attempt')!r} != line's {attempt}/{attempts}")
    return {
        "attempt": f"{attempt}/{attempts}",
        "peer": peer.lower(),
        "anchor": anchor,
    }


def _stage2(problems: list[str], stage: dict) -> dict | None:
    if not _stage_problem(problems, "2_receipt", stage):
        return None
    match = RE2.search(stage["log_line"])
    if not match:
        problems.append("2_receipt: the log line is not the responder's receipt line")
        return None
    advertised, key, anchor = match.groups()
    if not _same(stage.get("peer"), advertised):
        problems.append(f"2_receipt: peer {stage.get('peer')!r} != line's {advertised!r}")
    if not _same(stage.get("peer_key"), key):
        problems.append(f"2_receipt: peer_key {stage.get('peer_key')!r} != line's {key!r}")
    if not _same(stage.get("anchor"), anchor):
        problems.append(f"2_receipt: anchor {stage.get('anchor')!r} != line's {anchor!r}")
    return {"peer": advertised, "peer_key": key.lower(), "anchor": anchor}


def _stage3(problems: list[str], stage: dict) -> dict | None:
    if not _stage_problem(problems, "3_construction", stage):
        return None
    match = RE3.search(stage["log_line"])
    if not match:
        problems.append("3_construction: the log line is not the responder's prepared line")
        return None
    advertised, key, state, anchor, count, first, last = match.groups()
    if not _same(stage.get("peer"), advertised):
        problems.append(f"3_construction: peer {stage.get('peer')!r} != line's {advertised!r}")
    if not _same(stage.get("peer_key"), key):
        problems.append(f"3_construction: peer_key {stage.get('peer_key')!r} != line's {key!r}")
    if not _same(stage.get("state_hash"), state):
        problems.append(f"3_construction: state_hash {stage.get('state_hash')!r} != line's {state!r}")
    if not _same(stage.get("anchor"), anchor):
        problems.append(f"3_construction: anchor {stage.get('anchor')!r} != line's {anchor!r}")
    if stage.get("block_count") != int(count):
        problems.append(f"3_construction: block_count {stage.get('block_count')!r} != line's {count}")
    if not _same(stage.get("first_block"), first):
        problems.append(f"3_construction: first_block {stage.get('first_block')!r} != line's {first!r}")
    if not _same(stage.get("last_block"), last):
        problems.append(f"3_construction: last_block {stage.get('last_block')!r} != line's {last!r}")
    return {
        "peer": advertised,
        "peer_key": key.lower(),
        "state_hash": state,
        "anchor": anchor,
        "block_count": int(count),
        "first_block": first,
        "last_block": last,
    }


def _stage4(problems: list[str], stage: dict) -> dict | None:
    if not _stage_problem(problems, "4_transport", stage):
        return None
    match = RE4_OK.search(stage["log_line"]) or RE4_FAIL.search(stage["log_line"])
    if not match:
        problems.append(
            "4_transport: the line carries no 'transport outcome delivered|failed' result"
        )
        return None
    advertised, key, outcome, state = match.groups()
    if not _same(stage.get("peer"), advertised):
        problems.append(f"4_transport: peer {stage.get('peer')!r} != line's {advertised!r}")
    if not _same(stage.get("peer_key"), key):
        problems.append(f"4_transport: peer_key {stage.get('peer_key')!r} != line's {key!r}")
    if not _same(stage.get("outcome"), outcome):
        problems.append(f"4_transport: outcome {stage.get('outcome')!r} != line's {outcome!r}")
    if not _same(stage.get("state_hash"), state):
        problems.append(f"4_transport: state_hash {stage.get('state_hash')!r} != line's {state!r}")
    if outcome != "delivered":
        problems.append("4_transport: the transport outcome is failed, not delivered")
    return {
        "peer": advertised,
        "peer_key": key.lower(),
        "outcome": outcome,
        "state_hash": state,
    }


def _stage5(problems: list[str], stage: dict) -> dict | None:
    if not _stage_problem(problems, "5_receive", stage):
        return None
    match = RE5.search(stage["log_line"])
    if not match:
        problems.append("5_receive: the log line is not the joiner's received line")
        return None
    state, hashes, peer = match.groups()
    if not _same(stage.get("state_hash"), state):
        problems.append(f"5_receive: state_hash {stage.get('state_hash')!r} != line's {state!r}")
    if not _same(stage.get("peer"), peer):
        problems.append(f"5_receive: peer {stage.get('peer')!r} != line's {peer!r}")
    blocks = hashes.split()
    if stage.get("blocks") != blocks:
        problems.append(f"5_receive: blocks {stage.get('blocks')!r} != line's {blocks!r}")
    if not blocks or not all(_hex64(block) for block in blocks):
        problems.append("5_receive: the line names no 64-hex block")
    return {"state_hash": state, "peer": peer.lower(), "blocks": blocks}


def _stage6(problems: list[str], stage: dict) -> dict | None:
    if not _stage_problem(problems, "6_apply", stage):
        return None
    restore = RE6_RESTORE.search(stage["log_line"])
    if not restore:
        problems.append("6_apply: the log line is not the restore line")
        return None
    served, root, kind = restore.groups()
    walk = RE6_WALK.search(stage.get("walk_log_line") or "")
    if not walk:
        problems.append("6_apply: walk_log_line is not the page-walk completion line")
        return None
    walk_state, finished, total = walk.groups()

    if not _same(stage.get("served_state_hash"), served):
        problems.append(f"6_apply: served_state_hash {stage.get('served_state_hash')!r} != line's {served!r}")
    if not _same(stage.get("restored_state_hash"), served):
        problems.append(f"6_apply: restored_state_hash {stage.get('restored_state_hash')!r} != served {served!r}")
    if not _same(stage.get("walk_state_hash"), walk_state):
        problems.append(f"6_apply: walk_state_hash {stage.get('walk_state_hash')!r} != line's {walk_state!r}")
    if not _same(stage.get("accepted_root"), root):
        problems.append(f"6_apply: accepted_root {stage.get('accepted_root')!r} != line's {root!r}")
    if stage.get("accepted_root_kind") != kind:
        problems.append(f"6_apply: accepted_root_kind {stage.get('accepted_root_kind')!r} != line's {kind!r}")
    if stage.get("pages_finished") != int(finished):
        problems.append(f"6_apply: pages_finished {stage.get('pages_finished')!r} != line's {finished}")
    if stage.get("pages_total") != int(total):
        problems.append(f"6_apply: pages_total {stage.get('pages_total')!r} != line's {total}")
    if stage.get("walk_completed") is not True:
        problems.append("6_apply: walk_completed must be true")
    if int(finished) != int(total):
        problems.append("6_apply: the page walk did not finish every key")
    if stage.get("checkpoint_readback") is not True:
        problems.append("6_apply: checkpoint_readback must be true")
    if kind == "genesis" and not _same(root, served):
        problems.append(
            "6_apply: a genesis anchor's accepted root must equal the served state hash"
        )
    if not _hex64(root):
        problems.append("6_apply: accepted_root is not 64-hex")
    return {
        "served_state_hash": served,
        "restored_state_hash": served,
        "walk_state_hash": walk_state,
        "accepted_root": root,
        "accepted_root_kind": kind,
        "pages_finished": int(finished),
        "pages_total": int(total),
    }


def check_chain(chain: object) -> list[str]:
    """Problems with a typed six-stage chain; `[]` means PASS. Pure, no file I/O."""
    if not isinstance(chain, dict):
        return ["the chain is not an object"]
    problems: list[str] = []
    if chain.get("send_log_only") is True:
        problems.append("the chain declares itself a send log")
    if chain.get("schema") != 1:
        problems.append(f"schema {chain.get('schema')!r} is not 1")
    if chain.get("gate_id") != "G428-M3":
        problems.append(f"gate_id {chain.get('gate_id')!r} is not 'G428-M3'")
    if chain.get("locality") not in RECORD_LABELS:
        problems.append(f"locality {chain.get('locality')!r} is not a record locality")
    if chain.get("complete") is False:
        reasons = chain.get("incomplete_reasons") or []
        problems.append(
            "the chain is incomplete: " + "; ".join(str(reason) for reason in reasons)
            if reasons
            else "the chain is incomplete"
        )
    non_claims = chain.get("non_claims")
    if non_claims is not None and not isinstance(non_claims, list):
        problems.append("non_claims must be a list")

    correlation = chain.get("correlation")
    if not isinstance(correlation, dict):
        problems.append("correlation is absent")
        correlation = {}
    for field in ("joiner_key", "responder_key"):
        if not _key(correlation.get(field)):
            problems.append(f"correlation.{field} is not an even-length hex peer key")
    if not _nonempty(correlation.get("advertised_peer")):
        problems.append("correlation.advertised_peer is absent")
    if declared_host(correlation.get("advertised_peer")) is None:
        problems.append("correlation.advertised_peer is not an rnode:// address")
    if not _anchor(correlation.get("anchor")):
        problems.append("correlation.anchor is neither 'latest' nor 64-hex")
    if not _hex64(correlation.get("state_hash")):
        problems.append("correlation.state_hash is not 64-hex")

    stages = chain.get("stages")
    if not isinstance(stages, dict):
        problems.append("stages is absent")
        return problems
    for name in STAGE_ORDER:
        if name not in stages:
            problems.append(f"{name}: the stage is absent")
    for name in sorted(set(stages) - set(STAGE_ORDER)):
        problems.append(f"{name}: unknown stage")

    parsed: dict[str, dict] = {}
    for name, check in (
        ("1_request", _stage1),
        ("2_receipt", _stage2),
        ("3_construction", _stage3),
        ("4_transport", _stage4),
        ("5_receive", _stage5),
        ("6_apply", _stage6),
    ):
        if name in stages:
            result = check(problems, stages[name])
            if result is not None:
                parsed[name] = result

    for name, stage in stages.items():
        if isinstance(stage, dict) and stage.get("event") == "send_log":
            problems.append(f"{name}: a send log is not a stage witness")

    if len(parsed) != len(STAGE_ORDER):
        return problems

    # Cross-stage: the identity, anchor and served hash must all be one exchange.
    joiner = correlation.get("joiner_key")
    responder = correlation.get("responder_key")
    state = correlation.get("state_hash")
    anchor = correlation.get("anchor")
    if not _same(joiner, parsed["2_receipt"]["peer_key"]):
        problems.append("2_receipt: the requester key is not correlation.joiner_key")
    for name in ("3_construction", "4_transport"):
        if not _same(joiner, parsed[name]["peer_key"]):
            problems.append(f"{name}: the requester key is not correlation.joiner_key")
    for name in ("1_request", "5_receive"):
        if not _same(responder, parsed[name]["peer"]):
            problems.append(f"{name}: the responder key is not correlation.responder_key")
    if not _same(parsed["2_receipt"]["peer"], correlation.get("advertised_peer")):
        problems.append("2_receipt: the advertised peer is not correlation.advertised_peer")
    for name in ("3_construction", "4_transport"):
        if not _same(parsed[name]["peer"], parsed["2_receipt"]["peer"]):
            problems.append(f"{name}: the peer address differs from stage 2")

    for name in STAGE_ORDER:
        stage = stages[name]
        if _same(stage.get("anchor"), anchor) is False:
            problems.append(f"{name}: the anchor is not correlation.anchor")
        if _same(stage.get("anchor"), parsed["3_construction"]["anchor"]) is False:
            problems.append(f"{name}: the anchor is not the exchange's requested anchor")

    for name in ("3_construction", "4_transport", "5_receive"):
        if not _same(parsed[name]["state_hash"], state):
            problems.append(f"{name}: the state hash is not correlation.state_hash")
    for field in ("served_state_hash", "restored_state_hash", "walk_state_hash"):
        if not _same(parsed["6_apply"][field], state):
            problems.append(f"6_apply: {field} is not correlation.state_hash")
    return problems


# --------------------------------------------------------------------------------------------------
# correlation


def build_chain(
    *,
    joiner_log: str,
    responder_log: str,
    locality: str = "local-devnet",
    joiner_container: str | None = None,
    responder_container: str | None = None,
) -> dict:
    """Correlate two captured logs into one chain.

    The join key is the served state hash from the joiner's restore line, not a send line. An
    absent or ambiguous match is reported in `incomplete_reasons`; nothing is guessed.
    """
    joiner_lines = joiner_log.splitlines()
    responder_lines = responder_log.splitlines()
    reasons: list[str] = []

    restores = [
        (index, match)
        for index, line in enumerate(joiner_lines)
        if (match := RE6_RESTORE.search(line))
    ]
    stages: dict[str, dict] = {}
    correlation: dict[str, object] = {}

    if not restores:
        reasons.append("the joiner log has no completed restore line")
        return _chain(
            stages={},
            correlation={},
            locality=locality,
            reasons=reasons,
            joiner_container=joiner_container,
            responder_container=responder_container,
        )
    restore_index, restore = restores[-1]
    served, accepted_root, accepted_kind = restore.groups()

    walk = None
    for index in range(restore_index, -1, -1):
        match = RE6_WALK.search(joiner_lines[index])
        if match and match.group(1) == served:
            walk = match
            break
    if walk is None:
        reasons.append("the joiner log has no page-walk completion line for the served state")
    else:
        stages["6_apply"] = {
            "event": EVENTS["6_apply"],
            "peer": "pending",
            "anchor": "pending",
            "served_state_hash": served,
            "restored_state_hash": served,
            "walk_completed": True,
            "walk_state_hash": walk.group(1),
            "pages_finished": int(walk.group(2)),
            "pages_total": int(walk.group(3)),
            "checkpoint_readback": True,
            "accepted_root": accepted_root,
            "accepted_root_kind": accepted_kind,
            "source": "casper/src/engine/node_syncing.rs::report_restored_checkpoint",
            "log_line": joiner_lines[restore_index].strip(),
            "walk_log_line": joiner_lines[index].strip(),
        }

    receive = None
    for index in range(restore_index, -1, -1):
        match = RE5.search(joiner_lines[index])
        if match and match.group(1) == served:
            receive = (index, match)
            break
    if receive is None:
        reasons.append("the joiner log has no received-fringe line for the served state")
        responder_key = None
    else:
        receive_index, received = receive
        responder_key = received.group(3).lower()
        stages["5_receive"] = {
            "event": EVENTS["5_receive"],
            "peer": responder_key,
            "anchor": "pending",
            "state_hash": served,
            "blocks": received.group(2).split(),
            "source": "casper/src/engine/node_syncing.rs::NodeSyncing::handle",
            "log_line": joiner_lines[receive_index].strip(),
        }

    construction = None
    if responder_key is not None:
        for index, line in enumerate(responder_lines):
            match = RE3.search(line)
            if not match or match.group(3) != served:
                continue
            joiner_key = match.group(2).lower()
            receipt = None
            for earlier in range(index - 1, -1, -1):
                found = RE2.search(responder_lines[earlier])
                if found and found.group(2).lower() == joiner_key:
                    receipt = (earlier, found)
                    break
            if receipt is not None:
                construction = (index, match, receipt, joiner_key)
    if construction is None:
        reasons.append("the responder log has no construction line for the served state")
    else:
        index, prepared, receipt, joiner_key = construction
        receipt_index, received_request = receipt
        stages["2_receipt"] = {
            "event": EVENTS["2_receipt"],
            "peer": received_request.group(1),
            "peer_key": joiner_key,
            "anchor": received_request.group(3),
            "source": "casper/src/engine/node_running.rs::NodeRunning::handle",
            "log_line": responder_lines[receipt_index].strip(),
        }
        stages["3_construction"] = {
            "event": EVENTS["3_construction"],
            "peer": prepared.group(1),
            "peer_key": joiner_key,
            "anchor": prepared.group(4),
            "state_hash": prepared.group(3),
            "block_count": int(prepared.group(5)),
            "first_block": prepared.group(6),
            "last_block": prepared.group(7),
            "source": "casper/src/engine/node_running.rs::NodeRunning::handle",
            "log_line": responder_lines[index].strip(),
        }
        correlation["joiner_key"] = joiner_key
        correlation["advertised_peer"] = prepared.group(1)
        correlation["anchor"] = prepared.group(4)

        transport = None
        for later in range(index + 1, len(responder_lines)):
            match = RE4_OK.search(responder_lines[later]) or RE4_FAIL.search(
                responder_lines[later]
            )
            if match and match.group(4) == served and match.group(2).lower() == joiner_key:
                transport = (later, match)
        if transport is None:
            reasons.append("the responder log has no transport outcome for the served state")
        else:
            later, sent = transport
            stages["4_transport"] = {
                "event": EVENTS["4_transport"],
                "outcome": sent.group(3),
                "peer": sent.group(1),
                "peer_key": joiner_key,
                "anchor": prepared.group(4),
                "state_hash": served,
                "source": "casper/src/engine/node_running.rs::handle_finalized_fringe_request",
                "log_line": responder_lines[later].strip(),
            }

        request = None
        for earlier in range(receive_index, -1, -1):
            match = RE1.search(joiner_lines[earlier])
            if (
                match
                and match.group(3).lower() == responder_key
                and match.group(4) == prepared.group(4)
            ):
                request = (earlier, match)
                break
        if request is None:
            reasons.append("the joiner log has no request line for the responder and anchor")
        else:
            earlier, sent_request = request
            stages["1_request"] = {
                "event": EVENTS["1_request"],
                "peer": sent_request.group(3).lower(),
                "anchor": sent_request.group(4),
                "attempt": f"{sent_request.group(1)}/{sent_request.group(2)}",
                "source": "casper/src/protocol/comm_util.rs::request_initial_fringe_with_policy",
                "log_line": joiner_lines[earlier].strip(),
            }

    if "6_apply" in stages:
        stages["6_apply"]["peer"] = responder_key or "pending"
        stages["6_apply"]["anchor"] = correlation.get("anchor", "pending")
    if "5_receive" in stages:
        stages["5_receive"]["anchor"] = correlation.get("anchor", "pending")

    correlation["responder_key"] = responder_key or ""
    correlation["state_hash"] = served
    return _chain(
        stages=stages,
        correlation=correlation,
        locality=locality,
        reasons=reasons,
        joiner_container=joiner_container,
        responder_container=responder_container,
    )


def _chain(
    *,
    stages: dict,
    correlation: dict,
    locality: str,
    reasons: list[str],
    joiner_container: str | None,
    responder_container: str | None,
) -> dict:
    chain = {
        "schema": 1,
        "gate_id": "G428-M3",
        "tool": "tools/m3-join-chain.py",
        "generated_utc": dt.datetime.now(dt.timezone.utc)
        .isoformat(timespec="seconds")
        .replace("+00:00", "Z"),
        "locality": locality,
        "complete": not reasons,
        "incomplete_reasons": reasons,
        "containers": {
            "joiner": joiner_container or "",
            "responder": responder_container or "",
        },
        "correlation": correlation,
        "stages": {name: stages[name] for name in STAGE_ORDER if name in stages},
        "non_claims": [
            "a log correlation is not a cryptographic delivery receipt",
            "stage 4 is the joiner's gRPC receiver acknowledgement, not handler completion",
            "a local-devnet chain is not an off-host positive",
        ],
    }
    return chain


# --------------------------------------------------------------------------------------------------
# CLI


def chain_path(directory: Path) -> Path | None:
    """`chain.json` at the record root, or under `raw/` as `evidence-pack.py` lays it out."""
    for candidate in (directory / "chain.json", directory / "raw" / "chain.json"):
        if candidate.is_file():
            return candidate
    return None


def _record_chain(directory: Path) -> tuple[object | None, str | None]:
    path = chain_path(directory)
    if path is None:
        return None, f"{directory}: chain.json is absent (root or raw/)"
    try:
        return json.loads(path.read_text()), None
    except ValueError as error:
        return None, f"{path}: chain.json is not valid JSON: {error}"


def check_record(directory: Path, policy: str) -> tuple[int, list[str]]:
    chain, error = _record_chain(directory)
    if error is not None:
        return 3, [error]
    if policy not in POLICIES:
        return 3, [f"unknown locality policy {policy!r}"]
    problems = check_chain(chain)
    label = _locality.declared_locality(directory)
    if policy == "off-host":
        if label != "off-host-claim":
            problems.append(
                f"the record declares {label or 'no locality'}; an off-host criterion requires "
                "'off-host-claim'"
            )
        problems.extend(_locality.check(directory))
        advertised = (chain.get("correlation") or {}).get("advertised_peer") if isinstance(chain, dict) else None
        host = declared_host(advertised)
        if host is not None and _locality.local_address(host):
            problems.append(f"the advertised peer {host!r} is local, not routable")
        if isinstance(chain, dict) and chain.get("locality") not in (None, label):
            problems.append(
                f"the chain declares {chain.get('locality')!r} but the record declares {label!r}"
            )
    elif policy == "local-devnet":
        if label not in ("local-devnet", "single-host"):
            problems.append(
                f"the record declares {label or 'no locality'}; a local policy requires "
                "'local-devnet' or 'single-host'"
            )
        problems.extend(_locality.check(directory))
        if isinstance(chain, dict) and chain.get("locality") not in (None, label):
            problems.append(
                f"the chain declares {chain.get('locality')!r} but the record declares {label!r}"
            )
    return (1 if problems else 0), problems


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)

    check = sub.add_parser("check", help="check a record's chain.json and locality")
    check.add_argument("--record", required=True, type=Path)
    check.add_argument("--locality", default="any", choices=POLICIES)

    chain_cmd = sub.add_parser("check-chain", help="check a chain file only")
    chain_cmd.add_argument("--chain", required=True, type=Path)

    build = sub.add_parser("build", help="correlate two logs into chain.json")
    build.add_argument("--joiner-log", required=True, type=Path)
    build.add_argument("--responder-log", required=True, type=Path)
    build.add_argument("--out", required=True, type=Path)
    build.add_argument("--locality", default="local-devnet")
    build.add_argument("--joiner-container", default="")
    build.add_argument("--responder-container", default="")

    args = parser.parse_args(argv)
    if args.command == "check":
        code, problems = check_record(args.record, args.locality)
        for problem in problems:
            print(problem, file=sys.stderr)
        return code
    if args.command == "check-chain":
        if not args.chain.is_file():
            print(f"{args.chain}: chain is absent", file=sys.stderr)
            return 3
        try:
            chain = json.loads(args.chain.read_text())
        except ValueError as error:
            print(f"{args.chain}: not valid JSON: {error}", file=sys.stderr)
            return 3
        problems = check_chain(chain)
        for problem in problems:
            print(problem, file=sys.stderr)
        return 1 if problems else 0
    chain = build_chain(
        joiner_log=args.joiner_log.read_text(errors="replace"),
        responder_log=args.responder_log.read_text(errors="replace"),
        locality=args.locality,
        joiner_container=args.joiner_container,
        responder_container=args.responder_container,
    )
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(chain, indent=2, sort_keys=True) + "\n")
    for reason in chain["incomplete_reasons"]:
        print(reason, file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
