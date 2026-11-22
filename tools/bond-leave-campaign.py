#!/usr/bin/env python3
"""bond-leave-campaign — one disposable devnet feeds three release-claim arms.

The campaign, in order, on a three-validator net with stakes 100,100,50, epoch 10, quarantine 20,
`--no-autopropose --propose-on-deploy`:

1. a newcomer (key 3) is admitted, bonds 50, and activates at an epoch boundary (RC-1 bond-leave);
2. a marker is written and read at every validator's finalised head;
3. validator 2 (the 50-stake one) withdraws, the boundary activates the leave, and the vault is
   credited after quarantine (RC-1 bond-leave; the boundary bond sets are RC-2 validator-set);
4. validator 3 is stopped before the leave boundary and restarted after the payout, without manual
   surgery, and rejoins the common finalised head (RC-4 returner).

The mechanics are the committed n220/n223 rigs' (`spec/audit/evidence/n220-join-run.sh`,
`n220-leave-run.sh`, `n223-rejoin-run.sh`), driven from the HTTP API so every observation is a
typed value rather than a parsed line.

Usage: tools/bond-leave-campaign.py --out target/bond-leave-campaign/<id> [--keep]
Exit: 0 the campaign completed (observations may still fail their predicates); 2 usage/IO.
"""
from __future__ import annotations

import argparse
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
HTTP = [40403, 41403, 42403, 43403]
ADMIN = [40405, 41405, 42405, 43405]
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
    """An unbonded newcomer is not `ready` until it is activated, but it does sync and answer."""
    common.wait_until(f"validator {index} answers and has synced",
                      lambda: latest_height(HTTP[index]) > 0,
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


def drive(count: int = 1, pause: float = 0.0) -> list[str]:
    ids = []
    for index in range(count):
        ids.append(deploy(CONTAINERS[index % 3], GENESIS_PRIV, drive_term()))
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
    result = subprocess.run(
        ["docker", "exec", container, "rnode", "--grpc-host", "localhost",
         "listen-data-at-name", "-t", "pub", "-c", f'"{channel}"'],
        text=True, capture_output=True, timeout=90)
    text = result.stdout + result.stderr
    import re
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


def campaign(out: Path, keep: bool) -> dict:
    result: dict = {"schema": 1, "ok": False, "errors": [],
                    "bond_leave": {}, "validator_set": {}, "returner": {},
                    "finality_gate": {}, "finality_gate_after_leave": {},
                    "traces": [], "finality_traces": [], "boot_traces": []}
    env = dict(os.environ)
    env["DEVNET_EXTRA_FLAGS"] = "--quarantine-length 20"
    os.environ.update(env)
    devnet = common.Devnet(
        PREFIX, validators=3, memory="8g",
        node_env="RNODE_TRACE_DIR=/var/lib/rnode/traces",
        extra_up=("--no-autopropose", "--propose-on-deploy", "--stakes", "100,100,50",
                  "--epoch-length", str(EPOCH)))
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
        deadline = time.monotonic() + 420
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
            drive(1)
            time.sleep(8)
        if activation is None:
            raise common.ClaimError("the newcomer's bond never appeared in a block's bonds")
        result["bond_leave"]["activation_height"] = activation
        result["bond_leave"]["newcomer_produced_height"] = newcomer_produced

        # The bond set at the activation boundary, read on every validator.
        boundary_block = block_at(activation)
        if boundary_block is None:
            raise common.ClaimError("the activation block is not readable by height")
        sets = [{"validator": i, "bonds": bond_map(
            common.get_json(f"http://127.0.0.1:{HTTP[i]}/api/block/"
                            f"{boundary_block['blockHash']}").get("blockInfo") or {})}
            for i in range(4)]
        result["validator_set"]["boundaries"] = [{"height": activation, "sets": sets}]
        result["finality_gate"] = gate_samples()

        # --- the marker ----------------------------------------------------------------
        marker_id = deploy(CONTAINERS[0], GENESIS_PRIV, f'@{json.dumps(MARKER)}!({MARKER_VALUE})')
        marker_block = deploy_status(marker_id)
        wait_finalized(int(marker_block.get("blockNumber") or 0))

        # --- the leave -----------------------------------------------------------------
        fund = deploy(CONTAINERS[0], GENESIS_PRIV,
                      (ROOT / "examples" / "leave-fund.rho").read_text())
        deploy_status(fund)
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
            if active_count() == 2 and latest_height() > quarantine:
                wait_finalized(quarantine + 1, timeout=300)
                post_balance, post_block = balance(
                    CONTAINERS[0], "leave-balance-after", "leave-balance-after.rho")
                paid = (type(pre_balance) is int and type(post_balance) is int
                        and post_balance > pre_balance)
                result["bond_leave"].update({
                    "quarantine_height": quarantine,
                    "paid_height": finalized()[0] or latest_height(),
                    "active_at_boundary": active_count() == 2,
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
        final_block = common.get_json(f"http://127.0.0.1:{HTTP[0]}/api/block/{head_hash}")
        info = final_block.get("blockInfo") or {}
        sets = [{"validator": i, "bonds": bond_map(
            common.get_json(f"http://127.0.0.1:{HTTP[i]}/api/block/{head_hash}").get("blockInfo")
            or {})} for i in range(4)]
        result["validator_set"]["boundaries"].append({"height": head_height, "sets": sets})
        totals = [sum(v for v in row["bonds"].values() if type(v) is int)
                  for row in sets]
        result["validator_set"].update({
            "denominators_match": len(set(totals)) == 1,
            "active_set_agrees": all(row["bonds"] == sets[0]["bonds"] for row in sets),
            "finality_denominator": totals[0] if totals else 0,
        })
        result["finality_gate_after_leave"] = gate_samples()

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

        # --- the returner --------------------------------------------------------------
        # **Stop after the leave, not during the join.** With the newcomer (50) down before the
        # leave, the survivors hold 250 of 300 — but the finality layer counted only 200 of 300,
        # exactly two thirds and therefore not a strict supermajority. Once the 50-stake leaver is
        # out, the surviving set is 200 of 200 and the returner's absence is tolerable.
        before_boot = common.get_json(f"http://127.0.0.1:{HTTP[3]}/health").get("bootId")
        down_at, _ = finalized(HTTP[3])
        if down_at is None:
            down_at, _ = finalized(HTTP[0])
        devnet.shell("stop", "3", timeout=120)
        result["returner"].update({"victim": 3, "down_at_height": down_at,
                                   "manual_surgery": False})
        for _ in range(4):
            drive(1)
            time.sleep(6)
        devnet.shell("start", "3", timeout=120)
        wait_ready(3, timeout=420)
        tip_at_return = latest_height()
        common.wait_until(
            "the returner reaches the survivors' tip",
            lambda: latest_height(HTTP[3]) >= tip_at_return, timeout=300, interval=3.0)
        after_boot = common.get_json(f"http://127.0.0.1:{HTTP[3]}/health").get("bootId")
        common_head = common.wait_until(
            "the common finalised head is finalised on every validator",
            lambda: _common_head(), timeout=420, interval=3.0)
        rows = []
        for index in range(4):
            common.get_json(f"http://127.0.0.1:{HTTP[index]}/api/is-finalized/"
                            f"{common_head['hash']}")
            rows.append({"validator": index, "height": common_head["height"],
                         "hash": common_head["hash"]})
        result["returner"].update({
            "new_boot_id": bool(before_boot and after_boot and before_boot != after_boot),
            "heads_agree": True, "finalised": rows,
            "rejoin_height": common_head["height"],
        })

        traces = copy_traces(out, ("merge", "finality", "boot"))
        result["traces"] = traces["merge"]
        result["finality_traces"] = traces["finality"]
        result["boot_traces"] = traces["boot"]
        result["provenance"] = devnet.provenance()
        result["ok"] = True
    except Exception as error:  # noqa: BLE001 - the partial result is the report
        result["errors"].append(f"{type(error).__name__}: {error}")
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
