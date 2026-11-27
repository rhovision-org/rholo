#!/usr/bin/env python3
"""Bounded passive-LFS -> original-key reactivation on a disposable CI devnet.

Does not delete the state volume, bypass local finality, or alter production code.
"""
import importlib.util
import json
import os
import re
from pathlib import Path
import subprocess
import time
import urllib.error

ROOT = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("anchor_probe", ROOT / "lfs-anchored-checkpoint.py")
anchor = importlib.util.module_from_spec(spec)
spec.loader.exec_module(anchor)
history = anchor.history
adversarial = anchor.adversarial
positive = anchor.positive
NAME = positive.JOINER
OUT = Path("target/lfs-anchor-evidence")
OUT.mkdir(parents=True, exist_ok=True)


def call(*args, timeout=45):
    return subprocess.run(args, check=True, text=True, capture_output=True, timeout=timeout)


def inspect():
    return json.loads(call("docker", "inspect", NAME).stdout)[0]


def persist(label, value):
    (OUT / label).write_text(json.dumps(value, sort_keys=True, indent=2) + "\n")


def port_bindings(ins):
    result = []
    for target, bindings in (ins["HostConfig"].get("PortBindings") or {}).items():
        if not bindings:
            continue
        for binding in bindings:
            hostport = binding.get("HostPort")
            if not hostport:
                continue
            address = binding.get("HostIp") or ""
            result.extend(["-p", f"{address}:{hostport}:{target}" if address else f"{hostport}:{target}"])
    return result


def active_proposal_witness(log):
    rows=[{"height":int(h),"seq":int(q)} for h,q in re.findall(
        r"proposed and added block #(\d+) \(seq (\d+)\)",log)]
    if not rows:
        raise AssertionError("Key enabled without an observed successful local proposal")
    if any(b["seq"]<=a["seq"] for a,b in zip(rows,rows[1:])):
        raise AssertionError("Reactivated validator reused or regressed its own sequence")
    return rows


def anchored_head_matches(joiner, source, height, expected_hash):
    """Require the exact anchor or a later head independently finalized by both nodes."""
    if not isinstance(joiner, dict) or not isinstance(source, dict):
        return False
    head_height, head_hash = joiner.get("blockNumber"), joiner.get("blockHash")
    if type(head_height) is not int or head_height < height:
        return False
    if head_height == height:
        return head_hash == expected_hash
    return (source.get("blockNumber") == head_height
            and isinstance(head_hash, str)
            and re.fullmatch(r"[0-9a-f]{64}", head_hash) is not None
            and source.get("blockHash") == head_hash)


def main():
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted":
        raise SystemExit("Refusing activation outside disposable hosted Actions runner")
    if os.environ.get("DEVNET_PREFIX", "devnet") != "devnet":
        raise SystemExit("Unexpected network prefix")
    baseline = json.loads((OUT / "anchor_baseline.json").read_text())
    checkpoint = json.loads((OUT / "anchored_checkpoint_summary.json").read_text())
    expected_hash = baseline["anchor"]["blockHash"]
    expected_height = baseline["anchor"]["blockNumber"]
    if not checkpoint["finality_gate_before"] or not checkpoint["finality_gate_after"]:
        raise AssertionError("Passive LFS acceptance is not green; refusing activation")
    if checkpoint["source_anchor_hash"] != expected_hash:
        raise AssertionError("Source anchor changed in persisted evidence")
    previous = checkpoint["after_restart"]
    joiner = {"blockNumber": previous["lfb"], "blockHash": previous["lfb_hash"]}
    source = {"blockNumber": previous["source_lfb"], "blockHash": previous["source_lfb_hash"]}
    if not anchored_head_matches(joiner, source, expected_height, expected_hash):
        raise AssertionError("Passive joiner did not retain the anchor or reach a shared finalized successor")
    live_joiner = history.last_finalized(41403)
    live_source = history.last_finalized(40403)
    if not anchored_head_matches(live_joiner, live_source, expected_height, expected_hash):
        raise AssertionError("The passive finalized head is not corroborated before key activation")
    if not (history.is_finalized(41403, expected_hash)
            and history.is_finalized(40403, expected_hash)):
        raise AssertionError("Anchor no longer finalized on both nodes before key activation")

    before = inspect()
    if before["Name"] != "/" + NAME:
        raise AssertionError("Unexpected Docker container")
    if before["HostConfig"]["NetworkMode"] != "devnet":
        raise AssertionError("Unexpected container network")
    original = list(before["Config"].get("Cmd") or [])
    if "--validator-private-key-path" in original:
        raise AssertionError("Joiner is not passive before activation")
    if "--sync-anchor" not in original or original[original.index("--sync-anchor") + 1] != expected_hash:
        raise AssertionError("Explicit anchor is not preserved in startup")
    mounts = before.get("Mounts") or []
    state_mount = [m for m in mounts if m["Destination"] == "/var/lib/rnode"]
    if len(state_mount) != 1 or state_mount[0]["Type"] != "volume":
        raise AssertionError("Cannot certify durable state volume")
    volume_name = state_mount[0]["Name"]
    if volume_name != NAME + "-data":
        raise AssertionError("Refusing unexpected state volume " + volume_name)
    image = before["Config"]["Image"]

    flags = ["docker", "run", "-d", "--name", NAME, "--network", "devnet"]
    flags += port_bindings(before)
    sysctls = before["HostConfig"].get("Sysctls") or {}
    for name, value in sorted(sysctls.items()):
        flags += ["--sysctl", f"{name}={value}"]
    memory = before["HostConfig"].get("Memory", 0)
    swap = before["HostConfig"].get("MemorySwap", 0)
    if memory:
        flags += ["--memory", str(memory)]
    if swap:
        flags += ["--memory-swap", str(swap)]
    for mount in mounts:
        source = mount["Name"] if mount["Type"] == "volume" else mount["Source"]
        bind = f"{source}:{mount['Destination']}" + ("" if mount["RW"] else ":ro")
        flags += ["-v", bind]
    for env in before["Config"].get("Env", []):
        flags += ["-e", env]
    cmd = original + ["--validator-private-key-path", "/var/lib/rnode/validator.hex"]
    assert "--validator-private-key-path" not in original

    # Containers may be replaced only in this disposable hosted-runner namespace;
    # the node's volume is never deleted or recreated.
    activation_ts = time.time()
    call("docker", "rm", "-f", NAME)
    started = call(*(flags + [image] + cmd), timeout=55).stdout.strip()
    after = inspect()
    state_after = [m for m in after["Mounts"] if m["Destination"] == "/var/lib/rnode"]
    if len(state_after) != 1 or state_after[0].get("Name") != volume_name:
        raise AssertionError("Reactivate did not preserve original data volume")
    if "--validator-private-key-path" not in after["Config"]["Cmd"]:
        raise AssertionError("Validator key was not enabled")
    if (after["HostConfig"].get("Sysctls") or {}) != sysctls:
        raise AssertionError("Reactivation did not preserve the node's service port reservation")
    print("LFS_VALIDATOR_REACTIVATED " + json.dumps({
        "same_volume": True, "volume": volume_name,
        "validator_key_enabled": True, "anchor": expected_hash,
        "container_id": started[:18],
    }), flush=True)

    # A quiet chain is allowed to reject an empty propose as NoNewDeploys.
    # Supply one accepted, state-changing deploy before demanding new finality.
    probe_spec = importlib.util.spec_from_file_location(
        "reactivation_work", ROOT.parent / "tools/consensus-acceptance-drill.py")
    work = importlib.util.module_from_spec(probe_spec)
    probe_spec.loader.exec_module(work)
    work.PREFIX = "devnet"
    from types import SimpleNamespace
    driver = work.Drill(SimpleNamespace(validators=3, window=110,
        out=str(OUT / "reactivation-workload")))
    driver.deploy(f'@{json.dumps(driver.marker)}!({driver.value})')
    print("LFS_REACTIVATION_DEPLOY_ACCEPTED " + json.dumps({
        "marker": driver.marker, "value": driver.value,
        "source_sha": call("git", "rev-parse", "HEAD").stdout.strip(),
    }), flush=True)

    deadline = time.monotonic() + 110
    observations = []
    proposals = []
    last = {}
    while time.monotonic() < deadline:
        try:
            now = history.last_finalized(41403)
            source = history.last_finalized(40403)
            still_final = history.is_finalized(41403, expected_hash)
            consistent = anchor.inspect_anchor(baseline["ancestry"], baseline["anchor"], deadline=3)
            last = {
                "source_lfb": source["blockNumber"] if source else None,
                "source_lfb_hash": source["blockHash"] if source else None,
                "joiner_lfb": now["blockNumber"] if now else None,
                "joiner_lfb_hash": now["blockHash"] if now else None,
                "original_anchor_finalized": still_final,
                "ancestry_matched": consistent["matched"],
                "state_hashes_identical": consistent["state_hashes_identical"],
                "new_marker_finalized": (
                    now is not None and work.marker_present(
                        work.probe(1, driver.marker, now["blockHash"]), driver.value)
                ),
            }
            observations.append({"elapsed_s": round(time.time()-activation_ts, 2), **last})
            if (last["source_lfb"] is not None
                    and last["source_lfb"] > expected_height
                    and last["joiner_lfb"] is not None
                    and last["joiner_lfb"] == last["source_lfb"]
                    and last["joiner_lfb_hash"] == last["source_lfb_hash"]
                    and still_final and consistent["state_hashes_identical"] and last["new_marker_finalized"]):
                break
        except (OSError, AssertionError, ValueError, KeyError, urllib.error.URLError) as exc:
            observations.append({"elapsed_s": round(time.time()-activation_ts, 2), "read_error": str(exc)[:200]})
        if len(proposals) < 12:
            trial = subprocess.run(["tools/devnet.sh", "propose"], text=True,
                                   capture_output=True, timeout=40)
            proposals.append({
                "attempt": len(proposals)+1, "exit_code": trial.returncode,
                "reply": trial.stdout[-400:], "stderr": trial.stderr[-200:],
            })
        time.sleep(3)

    fresh = call("docker", "logs", "--since", str(int(activation_ts)-1), NAME, timeout=25)
    logs = fresh.stdout + fresh.stderr
    active_blocks = active_proposal_witness(logs)
    reported_equivocation = "equivocation detected" in logs
    reported_bad_state = "declared pre-state hash is not the state" in logs
    had_new_lfs = "LFS state is successfully restored." in logs
    result = {
        "source_sha": call("git", "rev-parse", "HEAD").stdout.strip(),
        "anchor_hash": expected_hash, "anchor_height": expected_height,
        "accepted_work_marker": driver.marker,
        "successful_local_proposals": active_blocks,
        "same_volume": True, "validator_key_reenabled": True,
        "observations": observations, "proposals": proposals,
        "after": last, "new_lfs_after_reactivation": had_new_lfs,
        "equivocation_after_reactivation": reported_equivocation,
        "bad_prestate_after_reactivation": reported_bad_state,
    }
    persist("reactivation_result.json", result)
    print("LFS_REACTIVATION_RESULT " + json.dumps({
        "source_sha": result["source_sha"], "after": last,
        "proposals": len(proposals), "new_lfs": had_new_lfs,
        "equivocation": reported_equivocation, "bad_prestate": reported_bad_state
    }), flush=True)

    if had_new_lfs or reported_equivocation or reported_bad_state:
        raise AssertionError("Activation introduced a fresh sync, equivocation, or prestate error")
    if not last.get("original_anchor_finalized") or not last.get("state_hashes_identical") or not last.get("new_marker_finalized"):
        raise AssertionError("Recovery invariant degraded after reactivating validator")
    if not (last.get("source_lfb") is not None and last["source_lfb"] > expected_height
            and last.get("joiner_lfb") == last["source_lfb"]
            and last.get("joiner_lfb_hash") == last.get("source_lfb_hash")):
        raise AssertionError("Activated validator did not demonstrate post-anchor finality progression")
    print("LFS_REACTIVATION_PASS " + json.dumps(last), flush=True)


if __name__ == "__main__":
    main()
