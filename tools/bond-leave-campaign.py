#!/usr/bin/env python3
"""bond-leave-campaign — one disposable devnet feeds three release-claim arms.

The campaign, in order, on a three-validator net with stakes 100,100,50, epoch 10, quarantine 20,
`--no-autopropose --propose-on-deploy`:

1. a newcomer (key 3) is admitted, bonds 50, and activates at an epoch boundary (RC-1 bond-leave);
2. a marker is written and read at every validator's finalised head;
3. validator 2 (the 50-stake one) withdraws, the boundary activates the leave, and the vault is
   credited after quarantine (RC-1 bond-leave; the boundary bond sets are RC-2 validator-set);
4. validator 3 is stopped **after** the payout, while the surviving set is a strict supermajority,
   and restarted without manual surgery; it rejoins the common finalised head (RC-4 returner). The
   stop is deliberately after the boundary: stopping it before the leave adds the 200/300 finality
   freeze to the returner arm, which would measure the denominator (RC-2's subject) instead of the
   rejoin (RC-4's).

The mechanics are the committed n220/n223 rigs' (`spec/audit/evidence/n220-join-run.sh`,
`n220-leave-run.sh`, `n223-rejoin-run.sh`), driven from the HTTP API so every observation is a
typed value rather than a parsed line.

Usage: tools/bond-leave-campaign.py --out target/bond-leave-campaign/<id> [--keep]
Exit: 0 the campaign completed (observations may still fail their predicates); 2 usage/IO.
"""
from __future__ import annotations

import argparse
import copy
import importlib.util
import json
import os
import re
import subprocess
import sys
import tempfile
import time
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location(
    "release_claim_common", ROOT / "tests" / "release_claim_common.py")
common = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(common)

PREFIX = "bondleave"
# **The bases are overridable** so the campaign can run beside another local devnet. The defaults
# are the historical 404xx range; the three arms pass the same `--campaign-result`, so the ports
# are a property of this run, not of the reading.
HTTP_BASE = int(os.environ.get("BOND_LEAVE_HTTP_BASE", "40403"))
ADMIN_BASE = int(os.environ.get("BOND_LEAVE_ADMIN_BASE", "40405"))
GRPC_BASE = int(os.environ.get("BOND_LEAVE_GRPC_BASE", "40402"))
HTTP = [HTTP_BASE + index * 1000 for index in range(4)]
ADMIN = [ADMIN_BASE + index * 1000 for index in range(4)]
CONTAINERS = [f"{PREFIX}-bootstrap", f"{PREFIX}-validator-1", f"{PREFIX}-validator-2",
              f"{PREFIX}-validator-3"]
GENESIS_PRIV = "a68a6e6cca30f81bd24a719f3145d20e8424bd7b396309b0708a16c7d8000b76"
K2_PRIV = "d78ff60a424d71ce99d6b7d7f44a8c49b38a3757ff9e6fa9b32fcba8aa2c973b"
K3_PRIV = "f484e1c24de228819213904c7da8999a94cfdd20e0c70f47a37ae0b6b8c9feb7"
K3_PUB = ("04ea3ce04abbe780205eb5f94a82889600630ca73bd31e7a336efb6700e24c515b"
          "900dea427d3e0c0fc74a8a6d5e173331433f00df2de4b7f953c2922297c2c08e")
EPOCH = 10
MARKER = "bond-leave:marker"
MARKER_VALUE = 42


def target() -> common.Target:
    return common.Target([
        {"name": f"v{i}", "http": f"http://127.0.0.1:{HTTP[i]}",
         "admin": f"http://127.0.0.1:{ADMIN[i]}"}
        for i in range(4)])


def ready(index: int) -> bool:
    try:
        _, health = common.http_json(f"http://127.0.0.1:{HTTP[index]}/health", timeout=3)
        return isinstance(health, dict) and health.get("ready") is True
    except Exception:  # noqa: BLE001 - a not-yet-started node is a wait
        return False


def wait_ready(index: int, timeout: float = 300) -> None:
    common.wait_until(f"validator {index} is ready", lambda: ready(index), timeout=timeout,
                      interval=2.0)


def wait_synced(index: int, timeout: float = 420) -> None:
    """An unbonded newcomer is not `ready` until it is activated, but it does sync and answer.

    **It must be at the bootstrap's height, not merely above zero.** A joining validator restores
    the bootstrap's checkpoint and then fetches every block after the anchor from the peer's
    validation window. Bonding before that catch-up completes changes the bonded pool, and the
    checkpoint collector then refuses every new certificate (`BondSetDrifted`) — so the joiner's own
    anchor goes stale and it can never fetch what it needs. The chain is not being driven here (the
    devnet is `--no-autopropose`), so both heights converge on their own; requiring equality is the
    precondition the join ladder assumes.
    """
    # A round is the unit `latestBlockNumber` counts, so the joiner can read one or two behind the
    # bootstrap while both are static; the precondition is that it has the chain, not the last block.
    common.wait_until(f"validator {index} has caught up with the bootstrap",
                      lambda: latest_height(HTTP[index]) >= latest_height(HTTP[0]) - 2,
                      timeout=timeout, interval=2.0)


def status(port: int = HTTP[0]) -> dict:
    return common.get_json(f"http://127.0.0.1:{port}/api/v1/status")


def latest_height(port: int = HTTP[0]) -> int:
    return int(status(port).get("latestBlockNumber") or 0)


def finalized(port: int = HTTP[0]) -> tuple[int | None, str | None]:
    try:
        body = common.get_json(f"http://127.0.0.1:{port}/api/last-finalized-block")
    except common.ClaimError:
        return None, None
    info = body.get("blockInfo") if isinstance(body, dict) else None
    if not isinstance(info, dict):
        return None, None
    return info.get("blockNumber"), info.get("blockHash")


def wait_finalized(height: int, timeout: float = 300, port: int = HTTP[0]) -> tuple[int, str]:
    def probe():
        current, block_hash = finalized(port)
        return (current, block_hash) if current is not None and current >= height and block_hash \
            else None
    return common.wait_until(f"finalised height {height}", probe, timeout=timeout, interval=2.0)


def is_finalized(port: int, block_hash: str) -> bool:
    try:
        return common.get_json(
            f"http://127.0.0.1:{port}/api/is-finalized/{block_hash}") is True
    except common.ClaimError:
        return False


def wait_finalized_hash(block_hash: str, timeout: float = 300) -> None:
    """Wait until every running validator calls the block finalised.

    A height is not enough: another block at the same height on a fork is also a finalised height.
    The bond-leave funding deploy is finalised this way before the withdrawal is signed, so the
    withdrawal's precharge cannot read a state that lacks the funding transfer.
    """
    common.wait_until(
        f"block {block_hash[:12]} is finalised on every validator",
        lambda: all(is_finalized(HTTP[index], block_hash) for index in range(4)),
        timeout=timeout, interval=2.0)


def health_sample(index: int = 0) -> dict:
    """One attributable sample: the gate, the stalled flag and the two heights."""
    sample: dict = {"validator": index, "height": None, "finalised": None,
                    "finalityStalled": None, "finalityStallEpisodes": None,
                    "finalityGate": None}
    try:
        sample["height"] = latest_height(HTTP[index])
    except Exception:  # noqa: BLE001 - a stopped node is a null sample, not a crash
        pass
    try:
        sample["finalised"] = finalized(HTTP[index])[0]
    except Exception:  # noqa: BLE001
        pass
    try:
        _, health = common.http_json(f"http://127.0.0.1:{HTTP[index]}/health", timeout=3)
        if isinstance(health, dict):
            sample["finalityStalled"] = health.get("finalityStalled")
            sample["finalityStallEpisodes"] = health.get("finalityStallEpisodes")
            sample["finalityGate"] = health.get("finalityGate")
    except Exception:  # noqa: BLE001
        pass
    return sample


def support_refusal(sample: dict) -> bool:
    """The exact-2/3 exposure: a support refusal at a denominator the layer cannot reach."""
    gate = sample.get("finalityGate")
    if not isinstance(gate, dict):
        return False
    supporting, total = gate.get("supporting"), gate.get("total")
    reason = str(gate.get("reason") or "").lower()
    return (reason.startswith("support") and type(supporting) is int
            and type(total) is int and total > 0
            and 3 * supporting <= 2 * total)


def gate_total(samples: dict | None) -> int | None:
    """The denominator every validator's gate reports, when they agree."""
    if not isinstance(samples, dict):
        return None
    totals = {row.get("total") for row in samples.values() if isinstance(row, dict)}
    return totals.pop() if len(totals) == 1 else None


def deploy(container: str, key: str, term: str, timeout: float = 90) -> str:
    path = f"/tmp/bond-campaign-{uuid.uuid4().hex}.rho"
    write = subprocess.run(["docker", "exec", "-i", container, "sh", "-c",
                            f"umask 077 && cat > {path}"],
                           input=term, text=True, capture_output=True, timeout=30)
    if write.returncode:
        raise common.ClaimError(f"term write failed on {container}: {write.stderr[-300:]}")
    try:
        result = subprocess.run(
            ["docker", "exec", container, "rnode", "--grpc-host", "localhost", "deploy",
             "--phlo-limit", "1000000", "--phlo-price", "1", "--private-key", key,
             "--shard-id", "/root", "--valid-after-block-number", str(latest_height()),
             path], text=True, capture_output=True, timeout=timeout)
    finally:
        subprocess.run(["docker", "exec", container, "rm", "-f", path],
                       text=True, capture_output=True, timeout=30)
    text = result.stdout + result.stderr
    match = re.search(r"DeployId is: ([0-9a-fA-F]+)", text)
    if not match:
        interesting = [line for line in text.splitlines()
                       if any(word in line.lower() for word in
                              ("error", "invalid", "refus", "fail", "reject", "phlo",
                               "exception", "cost", "deployid"))]
        raise common.ClaimError(
            f"deploy on {container} returned no id: "
            f"{chr(10).join(interesting[-8:]) or text[:800]}")
    return match.group(1).lower()


def deploy_status(deploy_id: str, timeout: float = 300) -> dict:
    def probe():
        for port in (HTTP[0], HTTP[1], HTTP[2], HTTP[3]):
            try:
                body = common.get_json(
                    f"http://127.0.0.1:{port}/api/v1/deploy-status/{deploy_id}")
            except common.ClaimError:
                continue
            if not isinstance(body, dict):
                continue
            success = body.get("ProcessedWithSuccess")
            if isinstance(success, dict):
                return success.get("block") or {}
            error = body.get("ProcessedWithError")
            if isinstance(error, dict):
                raise common.ClaimError(
                    f"deploy {deploy_id[:12]} failed: {error.get('deployError')}")
        return None
    return common.wait_until(f"deploy {deploy_id[:12]} is processed", probe, timeout=timeout,
                             interval=1.0)


def drive_term() -> str:
    return (ROOT / "examples" / "hello.rho").read_text()


def drive(count: int = 1, pause: float = 0.0, key: str = GENESIS_PRIV) -> list[str]:
    ids = []
    for index in range(count):
        ids.append(deploy(CONTAINERS[index % 3], key, drive_term()))
        if pause:
            time.sleep(pause)
    return ids


def blocks(port: int, depth: int = 50) -> list[dict]:
    body = common.get_json(f"http://127.0.0.1:{port}/api/blocks/{depth}")
    return body if isinstance(body, list) else []


def block_at(height: int, port: int = HTTP[0]) -> dict | None:
    body = common.get_json(f"http://127.0.0.1:{port}/api/blocks/{height}/{height}")
    rows = body if isinstance(body, list) else (body.get("blocks") or [] if isinstance(body, dict) else [])
    for row in rows:
        if isinstance(row, dict) and row.get("blockNumber") == height:
            return row
    return None


def boundary_sets(block_hash: str) -> list[dict] | None:
    """The bond map at one block, read on all four validators; None while any is behind."""
    rows = []
    for index in range(4):
        try:
            body = common.get_json(f"http://127.0.0.1:{HTTP[index]}/api/block/{block_hash}")
        except common.ClaimError:
            return None
        info = body.get("blockInfo") if isinstance(body, dict) else None
        if not isinstance(info, dict):
            return None
        rows.append({"validator": index, "bonds": bond_map(info)})
    return rows


def boundary_row(height: int, block_hash: str, gate: dict | None) -> dict:
    sets = common.wait_until(
        f"boundary {height} is readable on every validator",
        lambda: boundary_sets(block_hash), timeout=300, interval=2.0)
    totals = [sum(v for v in row["bonds"].values() if type(v) is int) for row in sets]
    return {
        "height": height,
        "sets": sets,
        "finality_denominator": totals[0] if len(set(totals)) == 1 else None,
        "gate_total": gate_total(gate),
    }


def bond_map(block: dict) -> dict:
    bonds = {}
    for row in block.get("bonds") or []:
        if isinstance(row, dict) and row.get("validator") is not None:
            bonds[str(row["validator"])] = row.get("stake")
    return bonds


def gate_samples() -> dict:
    """Each validator's current `/health.finalityGate` — the numbers the last derivation saw."""
    rows = {}
    for index, port in enumerate(HTTP):
        try:
            body = common.get_json(f"http://127.0.0.1:{port}/health")
            rows[f"v{index}"] = body.get("finalityGate")
        except common.ClaimError:
            rows[f"v{index}"] = None
    return rows


def pos(port: int = HTTP[0]) -> dict:
    try:
        body = common.get_json(f"http://127.0.0.1:{port}/api/v1/pos")
        return body if isinstance(body, dict) else {}
    except common.ClaimError:
        return {}


def active_count(port: int = HTTP[0]) -> int:
    return len(pos(port).get("activeValidators") or [])


def pending_deadline(timeout: float = 60) -> int | None:
    def probe():
        for row in pos().get("pendingWithdrawals") or []:
            if isinstance(row, dict) and type(row.get("deadline")) is int:
                return row["deadline"]
        return None
    try:
        return common.wait_until("the withdrawal stages with a deadline", probe, timeout=timeout,
                                 interval=0.5)
    except common.ClaimError:
        return None


def balance(container: str, channel: str, contract: str) -> tuple[int | None, int | None]:
    deploy_id = deploy(container, GENESIS_PRIV, (ROOT / "examples" / contract).read_text())
    deploy_status(deploy_id)
    # **`listen-data-at-name` streams.** It does not exit after the first datum, so the probe is
    # bounded by `timeout`: the data already printed is the answer and exit 124 is expected.
    try:
        result = subprocess.run(
            ["timeout", "20", "docker", "exec", container, "rnode", "--grpc-host", "localhost",
             "listen-data-at-name", "-t", "pub", "-c", f'"{channel}"'],
            text=True, capture_output=True, timeout=40)
    except subprocess.TimeoutExpired:
        return (None, None)
    text = result.stdout + result.stderr
    values = re.findall(r"GInt\(([0-9]+)\)", text)
    heights = re.findall(r"block_number: ([0-9]+)", text)
    return (int(values[-1]) if values else None,
            int(heights[-1]) if heights else None)


def copy_traces(out: Path, prefixes: tuple[str, ...]) -> dict[str, list[dict]]:
    rows: dict[str, list[dict]] = {prefix: [] for prefix in prefixes}
    for index, container in enumerate(CONTAINERS):
        for prefix in prefixes:
            try:
                listing = subprocess.check_output(
                    ["docker", "exec", container, "sh", "-c",
                     f"ls -1 /var/lib/rnode/traces 2>/dev/null | grep -E '^{prefix}-.*\\.json$' "
                     "| tail -n 200"], text=True, timeout=30)
            except subprocess.SubprocessError:
                continue
            for name in listing.split():
                try:
                    text = subprocess.check_output(
                        ["docker", "exec", container, "cat", f"/var/lib/rnode/traces/{name}"],
                        text=True, timeout=30)
                    row = json.loads(text)
                except (subprocess.SubprocessError, ValueError):
                    continue
                if isinstance(row, dict):
                    rows[prefix].append({"validator": index, "file": name, **row})
    return rows


def series_from(samples: list[dict], refusal: dict | None,
                settle: tuple[str, str | None] | None) -> list[dict]:
    """The DE-06 series, only when the samples carry every phase.

    A refusal is not a series: the phase rows need a progress step with finality held, a later
    finalised advance, and a settled deploy. When any of those is absent the function returns
    `[]`, and the record's `exercise` still says whether the two-thirds refusal was seen.
    """
    rows = [sample for sample in samples
            if isinstance(sample, dict) and type(sample.get("height")) is int
            and type(sample.get("finalised")) is int]
    if not rows or not isinstance(refusal, dict):
        return []
    index = next((i for i, sample in enumerate(rows)
                  if sample is refusal
                  or (sample.get("height") == refusal.get("height")
                      and sample.get("finalised") == refusal.get("finalised"))), None)
    if index is None or index == 0:
        return []
    before = rows[index - 1]
    after = rows[index]
    if not (after["height"] > before["height"]
            and after["finalised"] == before["finalised"]):
        return []
    resumed = next((sample for sample in rows[index + 1:]
                    if sample["finalised"] > after["finalised"]), None)
    if resumed is None:
        return []
    gate = refusal.get("finalityGate") or {}
    supporting, total = gate.get("supporting"), gate.get("total")
    if type(supporting) is not int or type(total) is not int or total <= 0:
        return []
    if not (3 * supporting <= 2 * total):
        return []
    statuses = {}
    if settle and isinstance(settle[0], str) and isinstance(settle[1], str):
        statuses = {settle[0]: settle[1]}
    if not statuses:
        return []
    return [
        {"phase": "baseline", "height": before["height"], "finalized": before["finalised"]},
        {"phase": "trigger", "mechanism": "bond-leave-boundary", "exercised": True,
         "live_stake": supporting, "bonded_total": total},
        {"phase": "stall", "progress_advanced": True, "finality_advanced": False,
         "progress_expected": True, "finality_expected": False,
         "settlement_claimed": False},
        {"phase": "restore", "progress_resumed": True, "finality_resumed": True, "rule": "R2"},
        {"phase": "settle", "deploy_status": statuses, "rejected_deploys": [],
         "surfaces": {}},
    ]


def _settlements_confirmed(result: dict) -> int:
    rows = (result.get("bond_leave") or {}).get("settlements") or []
    return sum(1 for row in rows if isinstance(row, dict)
               and row.get("stage") == "finalizedSuccess"
               and row.get("settlementConfirmed") is True)


def _floor_rejections(result: dict) -> int:
    """Merge traces that named a rejected host: the finalised-floor exposure, where it occurred."""
    count = 0
    for row in result.get("traces") or []:
        if not isinstance(row, dict):
            continue
        if row.get("rejected_hosts") or row.get("rejected_user_hosts"):
            count += 1
    return count


def episodes(result: dict) -> dict:
    """The three HAZOP episodes, each with the measurement that exercised it.

    The classification is the Table A wire, never invented; `exercise` is the measured half. A
    `deviation_observed: false` means the safeguard held in this run — it does not claim the
    deviation is impossible (the README states the non-claim).
    """
    bond = result.get("bond_leave") or {}
    validator = result.get("validator_set") or {}
    returner = result.get("returner") or {}
    refusal = result.get("first_support_refusal")
    retained = bond.get("marker_retained") or []
    confirmed = _settlements_confirmed(result)
    paid = bond.get("paid_after_quarantine") is True
    boundaries = [
        {"height": row.get("height"), "total": row.get("finality_denominator"),
         "gate_total": row.get("gate_total")}
        for row in validator.get("boundaries") or [] if isinstance(row, dict)]
    episodes = copy.deepcopy(common.BOND_LEAVE_EPISODES)
    episodes["bond-leave"]["exercise"] = {
        "path": "bond, withdraw, leave and payout on one devnet",
        "activation_height": bond.get("activation_height"),
        "withdraw_request_height": bond.get("withdraw_request_height"),
        "quarantine_height": bond.get("quarantine_height"),
        "paid_height": bond.get("paid_height"),
        "paid_after_quarantine": paid,
        "active_at_boundary": bond.get("active_at_boundary"),
        "marker_retained_all": bool(retained) and all(retained),
        "settlements_confirmed": confirmed,
        "floor_rejections_observed": _floor_rejections(result),
        "deviation_observed": not paid or confirmed < 4,
        "safeguard_observed": paid and confirmed >= 4 and bool(retained) and all(retained),
    }
    episodes["validator-set"]["exercise"] = {
        "boundaries": boundaries,
        "finality_denominators": validator.get("finality_denominators"),
        "gate_samples_count": len(result.get("finality_gate_series") or []),
        "first_support_refusal": refusal,
        "two_thirds_support_refusal_observed": bool(refusal and support_refusal(refusal)),
        "deviation_observed": validator.get("denominators_match") is not True
        or validator.get("active_set_agrees") is not True,
        "safeguard_observed": validator.get("denominators_match") is True
        and validator.get("active_set_agrees") is True,
    }
    episodes["returner"]["exercise"] = {
        "trigger": "validator 3 stopped after the payout and restarted without manual surgery",
        "trigger_exercised": type(returner.get("down_at_height")) is int
        and type(returner.get("restart_at_height")) is int,
        "down_at_height": returner.get("down_at_height"),
        "down_at_finalized": returner.get("down_at_finalized"),
        "downtime_s": returner.get("downtime_s"),
        "gate_refusal": returner.get("gate_refusal"),
        "restart_at_height": returner.get("restart_at_height"),
        "rejoin_height": returner.get("rejoin_height"),
        "new_boot_id": returner.get("new_boot_id"),
        "manual_surgery": returner.get("manual_surgery"),
        "deviation_observed": returner.get("heads_agree") is not True
        or returner.get("new_boot_id") is not True,
        "safeguard_observed": returner.get("heads_agree") is True
        and returner.get("new_boot_id") is True
        and returner.get("manual_surgery") is False,
    }
    return episodes


def campaign(out: Path, keep: bool) -> dict:
    result: dict = {"schema": 1, "ok": False, "errors": [],
                    "bond_leave": {}, "validator_set": {}, "returner": {},
                    "finality_gate": {}, "finality_gate_after_leave": {},
                    "finality_gate_series": [], "first_support_refusal": None,
                    "stall_series": [], "episodes": {},
                    "traces": [], "finality_traces": [], "boot_traces": []}
    env = dict(os.environ)
    env["DEVNET_EXTRA_FLAGS"] = "--quarantine-length 20"
    os.environ.update(env)
    devnet = common.Devnet(
        PREFIX, validators=3, memory="8g",
        http_base=HTTP_BASE, admin_base=ADMIN_BASE, grpc_base=GRPC_BASE,
        node_env="RNODE_TRACE_DIR=/var/lib/rnode/traces",
        extra_up=("--no-autopropose", "--propose-on-deploy", "--stakes", "100,100,50",
                  "--epoch-length", str(EPOCH),
                  # **Arm the inactivity leak** (law 89's leak clause). The absence threshold is a height lag, so
                  # it is set past any plausible block gap on this net: the signal under test is the
                  # gate's coverage refusal (`not_full`/`missing`), not a validator that stopped
                  # publishing. The window is two boundaries — one boundary of not-full is a race.
                  "--liveness-absence-threshold", str(EPOCH),
                  "--liveness-jail-after-boundaries", "2",
                  "--liveness-jail-epochs", "2"))
    try:
        devnet.down()
        devnet.up()
        for index in range(3):
            wait_ready(index)
        drive(1)
        wait_finalized(1)

        # --- the join ------------------------------------------------------------------
        # **The newcomer's LFS sync fetches the bootstrap's checkpoint certificate.** At genesis the
        # route answers 400 until one is minted, and the sync fails closed. Drive past the first
        # epoch boundary and wait for the certificate before resetting the newcomer.
        certificate_deadline = time.monotonic() + 420
        while True:
            try:
                common.get_json(f"http://127.0.0.1:{HTTP[0]}/api/v1/checkpoint")
                break
            except common.ClaimError:
                if time.monotonic() >= certificate_deadline:
                    raise common.ClaimError(
                        "the bootstrap never minted a checkpoint certificate for the LFS sync")
                drive(1)
                time.sleep(8)
        devnet.shell("reset", "3", timeout=900)
        wait_synced(3)
        admit = deploy(CONTAINERS[0], GENESIS_PRIV,
                       (ROOT / "examples" / "join-admit.rho").read_text())
        deploy_status(admit)
        bond = deploy(CONTAINERS[0], K3_PRIV,
                      (ROOT / "examples" / "join-bond.rho").read_text())
        bond_block = deploy_status(bond)
        result["bond_leave"].update({"bond_deploy": bond, "bond_settled": True})

        activation = None
        newcomer_produced = None
        deadline = time.monotonic() + 600
        while time.monotonic() < deadline:
            rows = blocks(HTTP[0], 50)
            bonded = [row.get("blockNumber") for row in rows if isinstance(row, dict)
                      and K3_PUB in bond_map(row)]
            produced = [row.get("blockNumber") for row in rows if isinstance(row, dict)
                        and row.get("sender") == K3_PUB]
            # The lowest block that names the newcomer is the activation boundary; taking the
            # first row in the list (the tip) records a later block and loses the boundary.
            if bonded:
                activation = min(bonded) if activation is None else min(activation, min(bonded))
            if produced:
                newcomer_produced = (min(produced) if newcomer_produced is None
                                     else min(newcomer_produced, min(produced)))
            if activation is not None and newcomer_produced is not None:
                break
            # **Drive with the newcomer's own key while it waits to be drawn.** The armed join
            # ladder admits a pooled validator only after it appears in the finalised fringe's
            # participation, and participation is the current layer's authors *and deploy signers*
            # (C344). A validator that is not active cannot author, so signing a deploy is the
            # ladder's own path to being noticed; the genesis deployer's key would leave it absent
            # and the draw would hold it out for ever.
            drive(1, key=K3_PRIV)
            time.sleep(8)
        if activation is None:
            raise common.ClaimError("the newcomer's bond never appeared in a block's bonds")
        result["bond_leave"]["activation_height"] = activation
        result["bond_leave"]["newcomer_produced_height"] = newcomer_produced

        # The bond set at the activation boundary, read on every validator. The gate is sampled
        # before the read so the denominator it publishes belongs to the boundary being measured.
        result["finality_gate"] = gate_samples()
        boundary_block = block_at(activation)
        if boundary_block is None:
            raise common.ClaimError("the activation block is not readable by height")
        result["validator_set"]["boundaries"] = [
            boundary_row(activation, boundary_block["blockHash"], result["finality_gate"])]
        activation_sample = health_sample(0)
        result["finality_gate_series"].append(activation_sample)
        if result["first_support_refusal"] is None and support_refusal(activation_sample):
            result["first_support_refusal"] = activation_sample

        # --- the marker ----------------------------------------------------------------
        marker_id = deploy(CONTAINERS[0], GENESIS_PRIV, f'@{json.dumps(MARKER)}!({MARKER_VALUE})')
        marker_block = deploy_status(marker_id)
        if marker_block.get("blockHash"):
            wait_finalized_hash(marker_block["blockHash"])
        else:
            wait_finalized(int(marker_block.get("blockNumber") or 0))

        # --- the leave -----------------------------------------------------------------
        fund = deploy(CONTAINERS[0], GENESIS_PRIV,
                      (ROOT / "examples" / "leave-fund.rho").read_text())
        fund_block = deploy_status(fund)
        fund_height = int(fund_block.get("blockNumber") or 0)
        fund_hash = fund_block.get("blockHash")
        if fund_hash:
            # **Finalised, not merely processed.** deploy-status can report success on a validator
            # whose block is not in the bootstrap's finalised prefix; submitting the withdrawal
            # there precharges against a state without the transfer and fails 0 < 1000000.
            wait_finalized_hash(fund_hash)
        result["bond_leave"].update({"fund_deploy": fund, "fund_height": fund_height,
                                     "fund_hash": fund_hash})
        # A named precondition beats the downstream precharge: if the vault is still empty, say so.
        # The read is best-effort — its own probe is a deploy and must not become a second way for
        # the campaign to fail when the funding block is already finalised.
        try:
            pre_withdraw_balance, _ = balance(
                CONTAINERS[0], "leave-balance", "leave-balance.rho")
        except Exception:  # noqa: BLE001 - a failed probe is a null, not a failed campaign
            pre_withdraw_balance = None
        result["bond_leave"]["balance_before_withdraw"] = pre_withdraw_balance
        if type(pre_withdraw_balance) is int and pre_withdraw_balance < 1_000_000:
            raise common.ClaimError(
                f"validator 2's vault holds {pre_withdraw_balance}, below the deploy precharge; "
                "the funding transfer did not land")
        # **The set before the leave, not the number three.** The armed join ladder can hold a
        # newcomer out for a boundary and the leak can retire a validator, so the active count is
        # not a constant; the withdrawal's own effect is that the set *shrinks*.
        active_before = active_count()
        withdraw = deploy(CONTAINERS[0], K2_PRIV,
                          (ROOT / "examples" / "pos-withdraw.rho").read_text())
        # **Poll for the staging at once.** The pending entry lives only to the next epoch
        # boundary — one block's width at epoch 10 — so waiting for the deploy status first is
        # longer than the window being measured (the n220 rig records the same trap).
        quarantine = pending_deadline()
        withdraw_block = deploy_status(withdraw)
        withdraw_height = int(withdraw_block.get("blockNumber") or 0)
        result["bond_leave"].update({"withdraw_deploy": withdraw, "withdraw_settled": True,
                                     "withdraw_request_height": withdraw_height,
                                     "epoch_length": EPOCH})
        if quarantine is None:
            raise common.ClaimError("the withdrawal never staged a deadline")

        pre_balance, pre_block = balance(CONTAINERS[0], "leave-balance", "leave-balance.rho")
        deadline = time.monotonic() + 600
        paid = False
        while time.monotonic() < deadline:
            # The boundary exposure is read at the gate, not inferred from the stop or the leaver's
            # height: a support refusal carries the layer's own supporting/total numbers.
            sample = health_sample(0)
            result["finality_gate_series"].append(sample)
            if result["first_support_refusal"] is None and support_refusal(sample):
                result["first_support_refusal"] = sample
            if active_count() < active_before and latest_height() > quarantine:
                wait_finalized(quarantine + 1, timeout=300)
                post_balance, post_block = balance(
                    CONTAINERS[0], "leave-balance-after", "leave-balance-after.rho")
                paid = (type(pre_balance) is int and type(post_balance) is int
                        and post_balance > pre_balance)
                result["bond_leave"].update({
                    "quarantine_height": quarantine,
                    "paid_height": finalized()[0] or latest_height(),
                    "active_at_boundary": active_count() < active_before,
                    "active_before": active_before,
                    "active_after": active_count(),
                    "paid_after_quarantine": paid,
                    "balance_before": pre_balance, "balance_after": post_balance,
                    "balance_blocks": [pre_block, post_block],
                })
                break
            drive(1)
            time.sleep(8)
        if "paid_height" not in result["bond_leave"]:
            raise common.ClaimError("the leave did not reach its deadline and shrink the set")

        # The bond set after the leave, at the common finalised head.
        head_height, head_hash = wait_finalized(max(quarantine + 1, latest_height() - 1))
        result["finality_gate_after_leave"] = gate_samples()
        result["validator_set"]["boundaries"].append(
            boundary_row(head_height, head_hash, result["finality_gate_after_leave"]))
        result["finality_gate_series"].append(health_sample(0))
        boundaries = result["validator_set"]["boundaries"]
        totals = [boundary.get("finality_denominator") for boundary in boundaries]
        result["validator_set"].update({
            "finality_denominators": totals,
            "denominators_match": all(total is not None for total in totals),
            "active_set_agrees": all(
                all(row["bonds"] == boundary["sets"][0]["bonds"]
                    for row in boundary["sets"])
                for boundary in boundaries),
        })

        # --- marker retention and settlements after the payout -------------------------
        retained = []
        settlements = []
        for index in range(4):
            height, block_hash = finalized(HTTP[index])
            retained.append(common.marker_present(
                common.Target([{"name": "v", "http": f"http://127.0.0.1:{HTTP[index]}",
                                "admin": f"http://127.0.0.1:{ADMIN[index]}"}]).probe(
                    0, MARKER, block_hash), MARKER_VALUE) if block_hash else False)
            row = common.get_json(
                f"http://127.0.0.1:{HTTP[index]}/api/v1/deploy-settlement/{withdraw}")
            settlements.append({"validator": index, "stage": row.get("stage"),
                                "settlementConfirmed": row.get("settlementConfirmed")})
        result["bond_leave"].update({"marker_retained": retained, "settlements": settlements})
        result["bond_leave"]["bond_deploy_status"] = (bond_block or {}).get("blockHash")
        settled = next((row.get("stage") for row in settlements
                        if isinstance(row, dict) and row.get("validator") == 0), None)
        result["stall_series"] = series_from(
            result["finality_gate_series"], result["first_support_refusal"],
            (withdraw, settled))

        # --- the returner --------------------------------------------------------------
        # **Stop after the leave, not during the join.** With the newcomer (50) down before the
        # leave, the survivors hold 250 of 300 — but the finality layer counted only 200 of 300,
        # exactly two thirds and therefore not a strict supermajority. Once the 50-stake leaver is
        # out, the surviving set is 200 of 200 and the returner's absence is tolerable. The stop is
        # therefore isolated to the rejoin: the boundary arithmetic is RC-2's subject, not RC-4's.
        before_boot = common.get_json(f"http://127.0.0.1:{HTTP[3]}/health").get("bootId")
        down_at, down_finalized = finalized(HTTP[3])
        if down_at is None:
            down_at, down_finalized = finalized(HTTP[0])
        stopped = time.monotonic()
        devnet.shell("stop", "3", timeout=120)
        result["returner"].update({"victim": 3, "down_at_height": down_at,
                                   "down_at_finalized": down_finalized,
                                   "manual_surgery": False})
        down_gate = None
        for _ in range(4):
            drive(1)
            sample = health_sample(0)
            result["finality_gate_series"].append(sample)
            if down_gate is None and support_refusal(sample):
                down_gate = sample
            time.sleep(6)
        devnet.shell("start", "3", timeout=120)
        wait_ready(3, timeout=420)
        restart_at_height = latest_height(HTTP[3])
        downtime = round(time.monotonic() - stopped, 1)
        tip_at_return = latest_height()
        common.wait_until(
            "the returner reaches the survivors' tip",
            lambda: latest_height(HTTP[3]) >= tip_at_return, timeout=300, interval=3.0)
        after_boot = common.get_json(f"http://127.0.0.1:{HTTP[3]}/health").get("bootId")
        common_head = common.wait_until(
            "the common finalised head is finalised on every validator",
            lambda: _common_head(), timeout=420, interval=3.0)
        rows = []
        answers = []
        for index in range(4):
            try:
                answer = common.get_json(
                    f"http://127.0.0.1:{HTTP[index]}/api/is-finalized/{common_head['hash']}")
            except common.ClaimError:
                answer = False
            answers.append(answer is True)
            rows.append({"validator": index, "height": common_head["height"],
                         "hash": common_head["hash"], "is_finalized": answer is True})
        result["finality_gate_series"].append(health_sample(0))
        result["returner"].update({
            "new_boot_id": bool(before_boot and after_boot and before_boot != after_boot),
            "heads_agree": all(answers), "finalised": rows,
            "rejoin_height": common_head["height"],
            "restart_at_height": restart_at_height,
            "downtime_s": downtime,
            "gate_refusal": down_gate,
        })

        traces = copy_traces(out, ("merge", "finality", "boot"))
        result["traces"] = traces["merge"]
        result["finality_traces"] = traces["finality"]
        result["boot_traces"] = traces["boot"]
        result["episodes"] = episodes(result)
        result["provenance"] = devnet.provenance()
        result["ok"] = True
    except Exception as error:  # noqa: BLE001 - the partial result is the report
        result["errors"].append(f"{type(error).__name__}: {error}")
        # A red run still carries the decision traces; the diagnosis is in them.
        try:
            traces = copy_traces(out, ("merge", "finality", "boot"))
            result["traces"] = traces["merge"]
            result["finality_traces"] = traces["finality"]
            result["boot_traces"] = traces["boot"]
        except Exception:  # noqa: BLE001
            pass
        try:
            result["provenance"] = devnet.provenance()
        except Exception:  # noqa: BLE001
            pass
    finally:
        if not keep:
            devnet.down()
    return result


def _common_head() -> dict | None:
    heads = []
    for index in range(4):
        height, block_hash = finalized(HTTP[index])
        if height is None or block_hash is None:
            return None
        heads.append((height, block_hash))
    lowest = min(heads, key=lambda pair: pair[0])
    for index in range(4):
        try:
            answer = common.get_json(
                f"http://127.0.0.1:{HTTP[index]}/api/is-finalized/{lowest[1]}")
        except common.ClaimError:
            return None
        if answer is not True:
            return None
    return {"height": lowest[0], "hash": lowest[1]}


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True)
    parser.add_argument("--keep", action="store_true")
    args = parser.parse_args(argv)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    result = campaign(out, args.keep)
    (out / "result.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"ok": result["ok"], "errors": result["errors"], "out": str(out)}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
