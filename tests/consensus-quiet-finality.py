#!/usr/bin/env python3
"""Isolated N8 and 7/8 quiet-chain finality witness: one marker deploy per arm.

"Quiet" means no supporting/helper deploys after the signed marker. The
test intentionally uses the node's normal autopropose timer; it does not
claim zero internally generated blocks or universal consensus safety.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location(
    "acceptance", ROOT / "tools/consensus-acceptance-drill.py")
a = importlib.util.module_from_spec(spec)
spec.loader.exec_module(a)


def docker(*args):
    return subprocess.check_output(
        ["docker", *args], text=True, stderr=subprocess.STDOUT, timeout=30).strip()


def verified_health(health, operator):
    """Fail closed when any safety/readiness counter is unavailable."""
    return (
        health.get("ready") is True
        and type(health.get("orphanBodies")) is int
        and health["orphanBodies"] == 0
        and type(health.get("pipelineFaults")) is int
        and health["pipelineFaults"] == 0
        and health.get("proposalHalted") is False
        and type(operator.get("unrestorableRecords")) is int
        and operator["unrestorableRecords"] == 0
        and type(operator.get("resources", {}).get("cgroupOomKills")) is int
        and operator["resources"]["cgroupOomKills"] == 0
    )


def agreed_finality(rows, indices, min_height, require_marker):
    """A finality witness requires exact hash agreement, not just heights."""
    if set(rows) != set(indices):
        return False
    identities = [(rows[i]["height"], rows[i]["hash"]) for i in indices]
    if len(set(identities)) != 1 or identities[0][0] < min_height:
        return False
    if any(not rows[i]["healthy"] for i in indices):
        return False
    if require_marker and any(rows[i].get("marker_present") is not True for i in indices):
        return False
    return True


def sample(index, marker=None, expected=None):
    health = a.get(index, "/health")
    operator = a.get(index, "/api/v1/status/operator", admin=True)
    finalized = a.get(index, "/api/last-finalized-block")
    height, block_hash = a.finalized_identity(finalized)
    row = {
        "node": index,
        "height": height,
        "hash": block_hash,
        "healthy": verified_health(health, operator),
        "health": {
            field: health.get(field) for field in
            ["ready", "orphanBodies", "pipelineFaults", "proposalHalted"]
        },
        "operator": {
            field: operator.get(field) for field in
            ["unrestorableRecords", "unrestorableBlock", "finalityStall", "resources"]
        },
    }
    if marker is not None:
        response = a.probe(index, marker, block_hash)
        row["marker_present"] = a.marker_present(response, expected)
    return row


def verify_finality_progress(index, old, row):
    if old is None:
        return
    if row["height"] < old["height"]:
        raise AssertionError(f"finalized height regressed on validator {index}")
    if row["hash"] != old["hash"]:
        # The API selects max(height, hash) from a DAG fringe. A growing
        # fringe can change that representative at the same height without
        # losing finality. Require the former block to remain finalized,
        # including when the representative's height increases.
        retained = a.get(index, f"/api/is-finalized/{old['hash']}")
        row["previous_finality"] = {
            "height": old["height"], "hash": old["hash"], "retained": retained}
        if retained is not True:
            raise AssertionError(f"previous finalized block lost on validator {index}")


def observe_until(out, label, indices, min_height, marker, expected, window):
    deadline = time.monotonic() + window
    seen = {}
    attempts = []
    while time.monotonic() < deadline:
        rows = {}
        errors = {}
        for index in indices:
            try:
                row = sample(index, marker, expected)
                rows[index] = row
                verify_finality_progress(index, seen.get(index), row)
                seen[index] = row
            except AssertionError as exc:
                # Preserve the violating response before failing; otherwise
                # the evidence ends at the preceding successful sample.
                errors[str(index)] = repr(exc)
                attempts.append({"elapsed_sec": round(window - max(0, deadline - time.monotonic()), 2),
                                 "nodes": rows, "errors": errors})
                (out / f"{label}-observations.json").write_text(
                    json.dumps({"phase": label, "target_min_finalized_height": min_height,
                                "attempts": attempts}, indent=2) + "\n")
                raise
            except (OSError, ValueError, KeyError, TimeoutError) as exc:
                errors[str(index)] = repr(exc)
        attempts.append({"elapsed_sec": round(window - max(0, deadline - time.monotonic()), 2),
                         "nodes": rows, "errors": errors})
        (out / f"{label}-observations.json").write_text(
            json.dumps({"phase": label, "target_min_finalized_height": min_height,
                        "attempts": attempts}, indent=2) + "\n")
        if not errors and agreed_finality(rows, indices, min_height, marker is not None):
            return {"height": rows[indices[0]]["height"],
                    "hash": rows[indices[0]]["hash"],
                    "samples": len(attempts),
                    "marker_present": marker is not None}
        time.sleep(3)
    raise AssertionError(
        f"{label}: exact finalized height/hash, healthy gate and marker not all proven "
        f"within {window}s; see {label}-observations.json")


def verify_namespace():
    if os.environ.get("GITHUB_ACTIONS") != "true" or \
            os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted":
        raise AssertionError("GitHub-hosted isolated runner required")
    if os.environ.get("DEVNET_PREFIX") != "acceptance" or \
            os.environ.get("DEVNET_NETWORK") != "acceptance":
        raise AssertionError("unexpected network namespace")
    for index in range(8):
        name = a.node_name(index)
        node = json.loads(docker("inspect", name))[0]
        if not node["State"]["Running"] or node["HostConfig"]["NetworkMode"] != "acceptance":
            raise AssertionError(f"{name}: incorrect network or process state")
        if node["HostConfig"]["Memory"] != 1610612736:
            raise AssertionError(f"{name}: unexpected memory cap")
        if "--autopropose" not in (node["Config"].get("Cmd") or []):
            raise AssertionError(f"{name}: autonomous production not configured")
    binary = [docker("exec", a.node_name(i), "sha256sum", "/usr/local/bin/rnode")
              .split()[0] for i in range(8)]
    if len(set(binary)) != 1:
        raise AssertionError("node binary hashes differ")
    return binary[0]


def run(out, window):
    report = {
        "source_sha": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
        "result": "INCOMPLETE", "measurement_window_seconds": window,
        "mode": "autopropose; one external signed marker per arm; zero helper deploys",
        "arms": [], "binary_sha256": None,
    }

    def save():
        (out / "quiet-finality-result.json").write_text(json.dumps(report, indent=2) + "\n")

    save()
    victim = a.node_name(7)
    disconnected = False
    try:
        report["binary_sha256"] = verify_namespace()
        report["baseline"] = observe_until(
            out, "baseline-eight", list(range(8)), 0, None, None, window)
        save()
        for label, indices in [("eight-of-eight", list(range(8))),
                               ("seven-of-eight", list(range(7)))]:
            arm = {"name": label, "phase": "prepare", "helper_deploys": 0,
                   "external_signed_marker_deploys": 0, "result": "INCOMPLETE"}
            report["arms"].append(arm)
            save()
            if label == "seven-of-eight":
                docker("network", "disconnect", "acceptance", victim)
                disconnected = True
                networks = json.loads(docker(
                    "inspect", "-f", "{{json .NetworkSettings.Networks}}", victim))
                if "acceptance" in networks:
                    raise AssertionError("partition did not isolate validator 7")
                arm["victim_disconnected"] = True

            arm["phase"] = "pre-marker-finality"
            baseline = observe_until(
                out, f"{label}-before", indices, 0, None, None, window)
            arm["baseline"] = baseline
            marker = f"quiet-finality:{label}:{uuid.uuid4().hex}"
            value = 42
            # Fail before submission if the marker was somehow already present.
            for index in indices:
                row = sample(index, marker, value)
                if a.marker_present(a.probe(index, marker, row["hash"]), value):
                    raise AssertionError("marker present before single signed deploy")

            arm["phase"] = "single-signed-marker"
            arm["marker"] = marker
            save()
            dargs = argparse.Namespace(out=str(out / label), validators=8, window=window)
            drill = a.Drill(dargs)
            drill.deploy(f'@{json.dumps(marker)}!({value})')
            arm["external_signed_marker_deploys"] = 1
            arm["phase"] = "quiet-measurement"
            save()
            # NO d.drive(), Nil deploy, repeated submit, or manual propose here.
            arm["finality"] = observe_until(
                out, f"{label}-quiet", indices,
                baseline["height"] + 3, marker, value, window)
            arm["result"] = "PASS"
            arm["phase"] = "completed"
            save()
        # A separate rejoin observation does not extend either quiet-arm deadline.
        docker("network", "connect", "acceptance", victim)
        disconnected = False
        report["victim_reconnected_before_cleanup"] = True
        last = report["arms"][-1]
        report["rejoin"] = observe_until(
            out, "quiet-rejoin", list(range(8)), last["finality"]["height"],
            last["marker"], 42, window)
        # Retain every arm's signed marker at the exact agreed finalized hash.
        report["retained_markers"] = {}
        for arm in report["arms"]:
            retained = {i: a.marker_present(a.probe(i, arm["marker"], report["rejoin"]["hash"]), 42)
                        for i in range(8)}
            report["retained_markers"][arm["name"]] = retained
            if not all(retained.values()):
                raise AssertionError(f"finalized marker lost after rejoin: {arm['name']}")
        report["result"] = "PASS_N8_AND_N7_QUIET_FINALITY"
    except Exception as exc:
        report["result"] = "FAIL_N8_N7_QUIET_FINALITY"
        report["error"] = {"type": type(exc).__name__, "message": str(exc)}
        save()
        raise
    finally:
        if disconnected:
            try:
                docker("network", "connect", "acceptance", victim)
                report["victim_reconnected_before_cleanup"] = True
            except Exception as exc:
                report["victim_reconnect_error"] = repr(exc)
                report["result"] = "FAIL_N8_N7_QUIET_FINALITY"
                save()
                raise
        save()
    print(json.dumps({"result": report["result"], "source": report["source_sha"],
                      "binary": report["binary_sha256"],
                      "arms": [{"name": arm["name"], "finality": arm.get("finality")}
                               for arm in report["arms"]]}), flush=True)
    if report["result"] != "PASS_N8_AND_N7_QUIET_FINALITY":
        raise AssertionError("quiet-chain result or cleanup did not pass")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--window", type=int, default=240)
    args = parser.parse_args()
    if args.window != 240:
        parser.error("Quiet-chain release gate is fixed at 240 seconds")
    args.out.mkdir(parents=True, exist_ok=True)
    run(args.out, args.window)
