#!/usr/bin/env python3
"""Explicit operator-anchor LFS: verify block ancestry, state roots, durable restart.

Only for a disposable hosted Actions devnet. Does not assert an archive of all
finalized sibling branches: the sync contract follows the named anchor's ancestry.
"""
import importlib.util
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import subprocess
import time
import urllib.error

def load(name, file):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(file))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

history = load("history", "lfs-finalized-history.py")
adversarial = history.adversarial
positive = adversarial.positive
OUTPUT = Path("target/lfs-anchor-evidence")
OUTPUT.mkdir(parents=True, exist_ok=True)


def save(name, doc):
    (OUTPUT / name).write_text(json.dumps(doc, sort_keys=True, indent=2) + "\n")


def read_block(port, hash_):
    doc = history.fetch(port, "/api/block/" + hash_)
    if not isinstance(doc, dict) or not isinstance(doc.get("blockInfo"), dict):
        raise AssertionError("missing block-detail envelope " + hash_)
    info = doc["blockInfo"]
    if info.get("blockHash") != hash_:
        raise AssertionError("wrong block returned")
    return info


def closure(port, anchor):
    found = {}
    stack = [anchor]
    while stack:
        current = stack.pop()
        if current in found:
            continue
        if len(found) > 128:
            raise AssertionError("anchor ancestry exceeds explicit test bound")
        info = read_block(port, current)
        fp = history.fingerprint(info)
        parents = info.get("justifications") or []
        if not isinstance(parents, list) or any(not isinstance(h, str) for h in parents):
            raise AssertionError("unexpected justification format")
        found[current] = fp
        for parent in parents:
            if parent not in found:
                stack.append(parent)
    return found


def inspect_anchor(expected, anchor, port=41403, deadline=140):
    """Verify an exact anchor-ancestry and state transfer independently of LFB progress.

    A restored block body is not automatically a *locally finalized* block.
    We preserve that distinction in an explicit, still-strict finality gate below.
    """
    end = time.monotonic() + deadline
    last = {}
    while time.monotonic() < end:
        matched = {}
        mismatched = {}
        for key, fp in expected.items():
            try:
                observed = history.fingerprint(read_block(port, key))
                if observed == fp:
                    matched[key] = observed
                else:
                    mismatched[key] = {"expected": fp, "actual": observed}
            except (OSError, ValueError, KeyError, AssertionError, urllib.error.URLError):
                pass
        missing = sorted(set(expected) - set(matched) - set(mismatched))
        finalized = history.last_finalized(port)
        try:
            anchor_finalized = history.is_finalized(port, anchor["blockHash"])
        except (OSError, ValueError, urllib.error.URLError, AssertionError):
            anchor_finalized = None
        last = {
            "matched": len(matched), "total": len(expected), "missing": missing,
            "mismatched": mismatched, "lfb": finalized["blockNumber"] if finalized else None,
            "lfb_hash": finalized["blockHash"] if finalized else None,
            "anchor_height": anchor["blockNumber"], "anchor_hash": anchor["blockHash"],
            "anchor_locally_finalized": anchor_finalized,
            "state_hashes_identical": not missing and not mismatched,
        }
        if not missing and not mismatched:
            return last
        time.sleep(3)
    save("anchor_failure.json", last)
    raise AssertionError("anchor ancestry/pre-post state did not restore: " + repr(last))


def finality_reached(result):
    return (
        result["lfb"] is not None
        and result["lfb"] >= result["anchor_height"]
        and result["anchor_locally_finalized"] is True
    )


def wait_for_local_finality(result, port=41403, deadline=75):
    """Observe local finality while explicitly driving a quiet source chain.

    The imported anchor is a recovery root, not a finality certificate. With
    --no-autopropose, time alone need not supply any new CBC evidence. Record
    each source proposal and both nodes' finality instead of assuming it will.
    """
    end = time.monotonic() + deadline
    last = dict(result)
    proposals = []
    next_proposal = time.monotonic() + 4
    while time.monotonic() < end:
        source_lfb = history.last_finalized(40403)
        finalized = history.last_finalized(port)
        try:
            yes = history.is_finalized(port, result["anchor_hash"])
        except (OSError, ValueError, urllib.error.URLError, AssertionError):
            yes = None
        last.update({
            "lfb": finalized["blockNumber"] if finalized else None,
            "lfb_hash": finalized["blockHash"] if finalized else None,
            "source_lfb": source_lfb["blockNumber"] if source_lfb else None,
            "source_lfb_hash": source_lfb["blockHash"] if source_lfb else None,
            "anchor_locally_finalized": yes,
            "post_restore_proposals": proposals,
        })
        if finality_reached(last):
            return last
        if time.monotonic() >= next_proposal and len(proposals) < 12:
            trial = subprocess.run(["tools/devnet.sh", "propose"], text=True,
                                   capture_output=True, timeout=35)
            proposals.append({
                "attempt": len(proposals) + 1, "code": trial.returncode,
                "reply": trial.stdout.strip()[-600:],
                "stderr_head": trial.stderr.strip()[:300] if trial.returncode else "",
                "source_lfb_before": last["source_lfb"],
                "joiner_lfb_before": last["lfb"],
            })
            print("LFS_POST_RESTORE_PROPOSE " + json.dumps(proposals[-1]), flush=True)
            next_proposal = time.monotonic() + 3
        time.sleep(2)
    return last



def reset_with_reply_loss(bootstrap_ip, node_id, genesis):
    """Drop bootstrap replies while the joiner request is acknowledged."""
    if os.environ.get("LFS_INITIAL_REPLY_LOSS") != "1":
        adversarial.reset(bootstrap_ip, key=node_id)
        return
    rule = ["-s", bootstrap_ip, "-p", "tcp", "--dport", "40400",
            "-m", "comment", "--comment", "lfs-initial-reply-control", "-j", "DROP"]
    def firewall(*args):
        pid = positive.cmd("docker", "inspect", "-f", "{{.State.Pid}}", positive.BOOTSTRAP)
        if not pid.isdecimal() or int(pid) <= 1:
            raise AssertionError("invalid disposable bootstrap PID")
        return subprocess.run(["sudo", "nsenter", "-t", pid, "-n", "iptables", *args], check=True, text=True,
                              capture_output=True, timeout=15).stdout
    firewall("-I", "OUTPUT", "1", *rule)
    started = time.monotonic()
    try:
        adversarial.reset(bootstrap_ip, key=node_id)
        reserved = positive.cmd("docker", "exec", positive.JOINER, "cat",
                                "/proc/sys/net/ipv4/ip_local_reserved_ports").strip()
        if reserved != "40400-40405":
            raise AssertionError("SETUP_PORT_RESERVATION: node service ports are not reserved")
        save("joiner_port_reservation.json", {"reserved_ports": reserved,
                                              "scope": "disposable container network namespace"})
        limit = time.monotonic() + 45
        while time.monotonic() < limit:
            observed = adversarial.run("docker", "logs", positive.JOINER, timeout=15)
            startup_errors = [line for line in observed.splitlines()
                              if "Server error:" in line or "listener failed:" in line]
            if startup_errors:
                save("joiner_startup_failure.json", {"errors": startup_errors})
                raise AssertionError("SETUP_JOINER_STARTUP: " + startup_errors[-1])
            requests = [line for line in observed.splitlines()
                        if "LFS fringe request " in line and "sent; awaiting admitted reply" in line]
            if "LFS state is successfully restored." in observed:
                raise AssertionError("reply-loss control failed: restored through blocked path")
            if len(requests) >= 2:
                break
            time.sleep(1)
        else:
            raise AssertionError("no second acknowledged solicitation while reply path blocked")
        counters = firewall("-L", "OUTPUT", "-n", "-v", "-x")
        matching = [line.split() for line in counters.splitlines() if "lfs-initial-reply-control" in line]
        if len(matching) != 1 or not matching[0][0].isdecimal() or int(matching[0][0]) <= 0:
            raise AssertionError("reply-loss rule did not actually drop any packet")
        save("initial_reply_loss_control.json", {
            "source_sha": subprocess.check_output(["git","rev-parse","HEAD"], text=True).strip(),
            "bootstrap_ip": bootstrap_ip, "acknowledged_request_logs": requests,
            "blocked_seconds": time.monotonic() - started, "iptables_counters": counters,
            "restored_while_blocked": False,
        })
        print("LFS_INITIAL_REPLY_LOSS_CONTROL " + json.dumps({"acknowledged_requests":len(requests),
              "blocked_seconds":round(time.monotonic()-started,2)}), flush=True)
    finally:
        firewall("-D", "OUTPUT", *rule)


def main():
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted":
        raise SystemExit("disposable hosted CI only")
    if os.environ.get("DEVNET_PREFIX", "devnet") != "devnet":
        raise SystemExit("unexpected devnet prefix")
    genesis_observations = []
    for i in range(3):
        node = positive.BOOTSTRAP if i == 0 else f"devnet-validator-{i}"
        genesis_observations.append({
            "node": node, "genesis": positive.genesis_hash(40403+i*1000),
            "status": positive.status(40403+i*1000),
            "binary_sha256": positive.cmd("docker","exec",node,"sha256sum","/usr/local/bin/rnode").split()[0],
        })
    assert len({x["genesis"] for x in genesis_observations}) == 1
    assert len({x["binary_sha256"] for x in genesis_observations}) == 1
    assert all(os.environ.get("RNODE_EVIDENCE_SOURCE_SHA", os.environ["GITHUB_SHA"]) in x["status"].get("version",{}).get("node","")
               for x in genesis_observations), "node source provenance differs"
    save("integrated_genesis_and_binary.json", genesis_observations)
    genesis = positive.genesis_hash(40403)
    if not genesis:
        raise AssertionError("missing genesis before controlled proposal")
    deployment = subprocess.run(["tools/devnet.sh", "deploy", "hello.rho"], text=True,
                                capture_output=True, timeout=60)
    if deployment.returncode:
        raise AssertionError("seed deploy rejected: " + deployment.stdout[-500:])
    print("LFS_ANCHOR_DEPLOY " + deployment.stdout[-200:], flush=True)

    baseline = None
    for attempt in range(30):
        proposed = subprocess.run(["tools/devnet.sh", "propose"], text=True,
                                  capture_output=True, timeout=30)
        anchor = history.last_finalized(40403)
        if anchor and anchor["blockNumber"] >= 2:
            ancestor = closure(40403, anchor["blockHash"])
            changed = [fp for fp in ancestor.values()
                       if fp["blockNumber"] > 0 and fp["preStateHash"] and fp["preStateHash"] != fp["postStateHash"]]
            if len(ancestor) >= 3 and changed:
                baseline = {"anchor": anchor, "ancestry": ancestor,
                            "changed_ancestors": len(changed), "attempts": attempt+1}
                break
        time.sleep(2)
    if not baseline:
        raise AssertionError("could not drive a nontrivial finalized anchor and state change")
    save("anchor_baseline.json", baseline)
    print("LFS_ANCHOR_BASELINE " + json.dumps({
        "hash": baseline["anchor"]["blockHash"], "height": baseline["anchor"]["blockNumber"],
        "ancestor_count": len(baseline["ancestry"]), "changed": baseline["changed_ancestors"],
    }), flush=True)

    bootstrap_ip = positive.cmd("docker", "inspect", "-f",
        '{{with index .NetworkSettings.Networks "devnet"}}{{.IPAddress}}{{end}}', positive.BOOTSTRAP)
    node_id = adversarial.cert_id(positive.BOOTSTRAP)
    os.environ["DEVNET_RESET_SYNC_ANCHOR"] = baseline["anchor"]["blockHash"]
    os.environ["DEVNET_RESET_PASSIVE"] = "1"
    reset_with_reply_loss(bootstrap_ip, node_id, genesis)
    args = json.loads(positive.cmd(
        "docker", "inspect", "-f", "{{json .Config.Cmd}}", positive.JOINER))
    if "--sync-anchor" not in args or args[args.index("--sync-anchor") + 1] != baseline["anchor"]["blockHash"]:
        raise AssertionError("explicit anchor flag was not installed on joiner")
    if "--validator-private-key-path" in args:
        raise AssertionError("passive recovery unexpectedly enabled the validator key")
    if not adversarial.joined(genesis):
        adversarial.wait_joined(genesis, seconds=140)
    first_snapshot = inspect_anchor(baseline["ancestry"], baseline["anchor"])
    result = wait_for_local_finality(first_snapshot)
    logs_before = adversarial.logs()
    count_before = logs_before.count("LFS state is successfully restored.")
    if count_before < 1:
        raise AssertionError("missing successful LFS checkpoint log")
    save("first_restore.json", {
        "first_import_observation": first_snapshot,
        "after_bounded_finality_wait": result,
        "restore_log_count": count_before,
    })
    print("LFS_EXPLICIT_ANCHOR_RESTORED " + json.dumps(result), flush=True)

    restart_since = datetime.now(timezone.utc).isoformat()
    adversarial.run("docker", "stop", positive.JOINER, timeout=50)
    adversarial.run("docker", "start", positive.JOINER, timeout=50)
    second_snapshot = inspect_anchor(baseline["ancestry"], baseline["anchor"])
    result2 = wait_for_local_finality(second_snapshot)
    fresh = subprocess.run(
        ["docker", "logs", "--since", restart_since, positive.JOINER],
        text=True, capture_output=True, timeout=20, check=True,
    )
    fresh_logs = fresh.stdout + fresh.stderr
    new_lfs = ("Starting from bootstrap node, syncing LFS" in fresh_logs
               or "LFS state is successfully restored." in fresh_logs)
    checkpoint = {
        "result": result2,
        "first_after_restart_observation": second_snapshot,
        "before_restart": count_before,
        "new_lfs": new_lfs,
        "restart_log_scope": "docker logs --since restart timestamp",
    }
    save("restart_checkpoint.json", checkpoint)
    if new_lfs:
        raise AssertionError("restarted node reentered LFS despite retained store")
    print("LFS_EXPLICIT_ANCHOR_PERSISTENT " + json.dumps(checkpoint), flush=True)

    summary = {
        "recovery_mode": "passive_until_validated_catchup",
        "source_anchor_height": baseline["anchor"]["blockNumber"],
        "source_anchor_hash": baseline["anchor"]["blockHash"],
        "ancestry_blocks": len(baseline["ancestry"]),
        "state_changing_ancestors": baseline["changed_ancestors"],
        "before_restart": result,
        "after_restart": result2,
        "durable_without_second_lfs": not checkpoint["new_lfs"],
        "finality_gate_before": finality_reached(result),
        "finality_gate_after": finality_reached(result2),
    }
    summary["source_sha"] = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    save("anchored_checkpoint_summary.json", summary)
    print("LFS_ANCHOR_STATE_PERSISTENCE_PASS " + json.dumps({
        "ancestry_blocks": len(baseline["ancestry"]), "matching_pre_post_state": True,
        "durable_without_second_lfs": not checkpoint["new_lfs"],
    }), flush=True)
    if not summary["finality_gate_before"] or not summary["finality_gate_after"]:
        print("LFS_ANCHOR_FINALITY_GATE_FAIL " + json.dumps({
            "anchor_height": summary["source_anchor_height"],
            "before_lfb": result["lfb"], "after_lfb": result2["lfb"],
            "before_anchor_finalized": result["anchor_locally_finalized"],
            "after_anchor_finalized": result2["anchor_locally_finalized"],
        }), flush=True)
        raise AssertionError(
            "anchor bodies and state persisted, but local finality has not advanced "
            "to the operator-named anchor; release finality gate remains red"
        )
    print("LFS_ANCHOR_FINALITY_GATE_PASS " + json.dumps({
        "anchor_height": summary["source_anchor_height"],
        "after_lfb": result2["lfb"],
    }), flush=True)
    # Only after strict anchored-finality acceptance: test original-key
    # reactivation against the exact same durable state volume.
    subprocess.run(["python3", "tests/lfs-passive-reactivation.py"], check=True)


if __name__ == "__main__":
    main()
