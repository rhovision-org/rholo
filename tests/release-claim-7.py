#!/usr/bin/env python3
"""RC-7 — upgrade compatibility: populated-store migration, downgrade refusal, mixed fleet.

Test: `python3 tools/release-check.py run --claim RC-7 --env local [--old-image rnode:old]`

Three arms:

1. the G520 populated-store copy/rollback/restart drill;
2. a store stamped with a store-format version this build does not know must be refused;
3. two binaries in one network: the block produced during the window must have the same state
   triples on both, or the old node must refuse on the consensus path; after the swap the upgraded
   node must reach the same finalised head.

The mixed arm needs a separately built prior image. Without one it is `not run`, not a fabricated
pass; the other two arms still run and are recorded.
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location(
    "release_claim_common", ROOT / "tests" / "release_claim_common.py")
common = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(common)

CLAIM_ID = "RC-7"
TITLE = "Upgrade compatibility"
STORE_FORMAT_VERSION = 4  # casper/src/storage.rs; the test stamps one higher
EPOCH_LENGTH = 10  # the mixed devnet's epoch; the boundary writes the prior binary's native state
DE18_CAUSE = "mixed-version"
DE18_SHAPE = "terminal"
DE18_REPRESENTATION = "protocol-stall"


def _state_triple(row: dict) -> tuple:
    return (row.get("preStateHash"), row.get("postStateHash"),
            tuple(row.get("rejectedDeploys") or []))


def _fixture_baseline(arm: str = "local") -> dict:
    environment = "public" if arm == "public" else "local"
    record = {
        "schema": 1, "claim_id": CLAIM_ID, "claim_title": TITLE, "arm": arm,
        "environment": environment,
        "locality": "public-live" if environment == "public" else "local-devnet",
        "provenance": {"source_sha": "0" * 40, "binary_sha256": "1" * 64},
        "preconditions": [], "verdict": "pass", "reasons": [], "errors": [],
        "harness": {"name": "populated-store-migration-drill", "exit_code": 0},
        "observations": {
            "migration_result": "PASS",
            "downgrade": {"refused": True, "stderr": "written by a newer build (store format 5"},
            "mixed": {
                "old_image": "rnode:old", "old_binary_sha256": "2" * 64,
                "new_binary_sha256": "3" * 64,
                "agreement": True, "refusal": None,
                "upgrade": {"heads_agree": True, "marker_present": True},
            },
            "format_traces": [{
                "kind": "format", "stage": "probe", "present": str(STORE_FORMAT_VERSION + 1),
                "build": STORE_FORMAT_VERSION, "on_disk": STORE_FORMAT_VERSION + 1,
                "decision": "refuse_newer", "stamped": False, "durable": False,
            }],
        },
    }
    if environment == "local":
        record["observations"]["stall_episode"] = mixed_store_episode(
            boundary={"epoch_length": EPOCH_LENGTH, "crossed": True, "height": 11},
            old_sha=record["observations"]["mixed"]["old_binary_sha256"],
            new_sha=record["observations"]["mixed"]["new_binary_sha256"],
            install={"deploy_id": "ab" * 32, "submitted_via": 3, "height": 10,
                     "hash": "cd" * 32, "marker_present": True},
            invoke={"deploy_id": "ef" * 32, "height": 12, "hash": "12" * 32,
                    "settled": True, "error": None},
            reread={"height": 10, "hash": "cd" * 32, "marker_present": True,
                    "state_triple_matches": True, "error": None},
            store={"validator": 3, "written_build": STORE_FORMAT_VERSION,
                   "read_build": STORE_FORMAT_VERSION,
                   "read_present": str(STORE_FORMAT_VERSION),
                   "read_decision": "accept", "same_format": True,
                   "format_boundary_crossed": False},
            result_present=True)
    return record


def _de18_problems(record: dict, observations: dict) -> list[str]:
    """The local mixed arm must record its DE-18 exercise, and a dead prior path fails it.

    The HAZOP checker validates the classification only; the honesty of `trigger_exercised` and
    the deviation reading is enforced here, in the claim's own predicate.
    """
    if record.get("arm") != "local":
        return []
    episode = observations.get("stall_episode")
    if not isinstance(episode, dict):
        return ["the local mixed arm did not record the DE-18 prior-path exercise"]
    problems: list[str] = []
    if episode.get("deviations") != ["DE-18"]:
        problems.append(f"DE-18: the episode names {episode.get('deviations')!r}, not ['DE-18']")
    observed = (episode.get("cause"), episode.get("shape"), episode.get("representation"))
    if observed != (DE18_CAUSE, DE18_SHAPE, DE18_REPRESENTATION):
        problems.append(f"DE-18: the episode classification is {observed!r}")
    exercise = episode.get("exercise") or {}
    if exercise.get("trigger_exercised") is not True:
        problems.append("DE-18: the prior-format contract path was not exercised: "
                        + str(exercise.get("not_exercised_reason") or "no reason recorded"))
    if exercise.get("deviation_observed") is not False:
        problems.append("DE-18 observed: the upgrade reported success while the prior path was dead")
    if exercise.get("safeguard_observed") is not True:
        problems.append("DE-18: the safeguard was not observed")
    if exercise.get("old_binary_sha256") in (None, exercise.get("new_binary_sha256")):
        problems.append("DE-18: the mixed arm did not run two distinct binaries")
    store = exercise.get("store_format") or {}
    if store.get("read_decision") != "accept":
        problems.append(f"DE-18: the upgraded store was not opened "
                        f"(decision {store.get('read_decision')!r})")
    if exercise.get("deviation_observed") is False:
        reread = exercise.get("prior_block_reread") or {}
        if (reread.get("marker_present") is not True
                or reread.get("state_triple_matches") is not True):
            problems.append("DE-18: the pre-swap block was not re-read on the upgraded validator")
    return problems


def _check_baseline(record: dict) -> tuple[str, list[str]]:
    if record.get("verdict") == "not run":
        return "not run", list(record.get("preconditions") or record.get("reasons") or [])
    problems: list[str] = []
    observations = record.get("observations")
    if not isinstance(observations, dict):
        return "fail", ["observations: missing or not an object"]
    if observations.get("migration_result") != "PASS":
        problems.append(f"the populated-store migration drill returned "
                        f"{observations.get('migration_result')!r}")
    downgrade = observations.get("downgrade")
    if not isinstance(downgrade, dict) or downgrade.get("refused") is not True:
        problems.append("a store written by a newer build was not refused")
    mixed = observations.get("mixed")
    if not isinstance(mixed, dict):
        problems.append("mixed: missing or not an object")
        return "fail", problems
    if mixed.get("old_binary_sha256") == mixed.get("new_binary_sha256"):
        problems.append("the mixed arm did not run two distinct binaries")
    if mixed.get("agreement") is not True and not mixed.get("refusal"):
        problems.append("the mixed fleet neither agreed on the block nor refused on the consensus path")
    upgrade = mixed.get("upgrade") or {}
    if upgrade.get("heads_agree") is not True:
        problems.append("the upgraded node did not reach the same finalised head")
    if upgrade.get("marker_present") is not True:
        problems.append("the marker is absent after the upgrade")
    problems.extend(_de18_problems(record, observations))
    problems.extend(common.phase_space_reading(observations, bool(problems)))
    return common.evaluate(record, problems)


FIXTURES = {
    "local": lambda: _fixture_baseline("local"),
    "public": lambda: _fixture_baseline("public"),
}
ARMS = {
    "local": _check_baseline,
    "public": _check_baseline,
}


def fixture(arm: str = "local") -> dict:
    return FIXTURES[arm]()


def check(record: dict) -> tuple[str, list[str]]:
    if record.get("verdict") == "not run":
        return "not run", list(record.get("preconditions") or record.get("reasons") or [])
    return common.dispatch_arm(ARMS, record)


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--env", choices=("local", "public"))
    parser.add_argument("--arm")
    parser.add_argument("--out", required=True)
    parser.add_argument("--public-manifest")
    parser.add_argument("--keep", action="store_true")
    parser.add_argument("--old-image", default=os.environ.get("RNODE_OLD_IMAGE"))
    args = parser.parse_args(argv)
    arm = args.arm or args.env or "local"
    meta = common.claim_arm(CLAIM_ID, arm)
    environment = meta["environment"]
    record = common.new_record(CLAIM_ID, TITLE, environment, meta["locality"], arm=arm)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    failure = common.preflight_failure(CLAIM_ID, environment, out, record,
                                       getattr(args, "public_manifest", None))
    if failure is not None:
        return failure

    if environment == "public":
        manifest = common.load_manifest(args.public_manifest)
        if not manifest:
            return common.not_run(out, record, "no --public-manifest for the public fleet")[1]
        supplied = manifest.get("upgrade_record")
        if not supplied:
            return common.not_run(
                out, record,
                "the public run needs the operator's migration and mixed-version record")[1]
        observations = json.loads(Path(supplied).read_text())
        record["observations"] = observations
        record["provenance"] = manifest.get("provenance") or {}
        verdict, reasons = check(record)
        common.finish(record, verdict, reasons)
        return common.write_record(out, record)[1]

    try:
        record["provenance"] = _provenance()
        _migration(record, out)
        _downgrade(record, out)
        if not args.old_image:
            record["preconditions"].append(
                "no --old-image/RNODE_OLD_IMAGE: the mixed-version arm needs a separately built "
                "prior image")
            common.finish(record, "not run", list(record["preconditions"]))
            return common.write_record(out, record)[1]
        _mixed(record, out, args.old_image, args.keep)
    except Exception as error:  # noqa: BLE001
        common.finish(record, "fail", [f"{type(error).__name__}: {error}"])
        return common.write_record(out, record)[1]
    verdict, reasons = check(record)
    common.finish(record, verdict, reasons)
    return common.write_record(out, record)[1]


def _provenance() -> dict:
    def run(*args: str):
        try:
            return subprocess.check_output(args, cwd=ROOT, text=True,
                                           stderr=subprocess.DEVNULL).strip()
        except Exception:
            return None
    current = os.environ.get("RNODE_IMAGE", "rnode:local")
    image = run("docker", "inspect", "-f", "{{.Id}}", current)
    binary = run("docker", "run", "--rm", "--entrypoint", "sha256sum", current,
                 "/usr/local/bin/rnode")
    return {"source_sha": run("git", "rev-parse", "HEAD"),
            "source_dirty": bool(run("git", "status", "--porcelain")),
            "binary_sha256": binary.split()[0] if binary else None,
            "image_id": image, "profile": "local-4", "devnet_prefix": "rc7"}


def _migration(record: dict, out: Path) -> None:
    migration_out = out / "migration"
    command = [sys.executable, str(ROOT / "tools" / "populated-store-migration-drill.py"),
               "--gate", "--out", str(migration_out)]
    result = subprocess.run(command, cwd=ROOT, text=True, capture_output=True, timeout=3600)
    (out / "migration.log").write_text(result.stdout + result.stderr)
    doc = _read_json(migration_out / "result.json") or {}
    record["harness"] = {"name": "populated-store-migration-drill", "command": command,
                         "exit_code": result.returncode, "out": str(migration_out)}
    record["observations"]["migration_result"] = doc.get("result") or (
        "PASS" if result.returncode == 0 else "FAIL")
    record["observations"]["migration_reasons"] = doc.get("reasons") or []


def _downgrade(record: dict, out: Path) -> None:
    with tempfile.TemporaryDirectory(prefix="rc7-store-") as scratch:
        (Path(scratch) / "store-format").write_text(f"{STORE_FORMAT_VERSION + 1}\n")
        trace_dir = Path(scratch) / "traces"
        trace_dir.mkdir(mode=0o777, exist_ok=True)
        # The image entrypoint is `rnode`; `--profile` is a top-level option before the
        # subcommand. Repeating the binary name makes the CLI read it as a subcommand.
        command = ["docker", "run", "--rm", "-v", f"{scratch}:/data",
                   "-e", "RNODE_TRACE_DIR=/data/traces",
                   os.environ.get("RNODE_IMAGE", "rnode:local"),
                   "--profile", "docker", "run", "--data-dir", "/data"]
        try:
            result = subprocess.run(command, cwd=ROOT, text=True, capture_output=True,
                                    timeout=120)
            stderr = result.stdout + result.stderr
        except subprocess.TimeoutExpired as error:
            stderr = (error.stdout or "") + (error.stderr or "")
        rows = record["observations"].setdefault("format_traces", [])
        for path in sorted(trace_dir.glob("format-*.json")):
            try:
                rows.append({"file": path.name, **json.loads(path.read_text())})
            except ValueError:
                continue
    (out / "downgrade.log").write_text(stderr)
    expected = f"written by a newer build (store format {STORE_FORMAT_VERSION + 1}"
    record["observations"]["downgrade"] = {
        "refused": expected in stderr,
        "stderr": stderr[-2000:],
        "command": command,
    }


def _not_exercised_reason(boundary: dict, old_sha, new_sha, install: dict) -> str:
    if boundary.get("crossed") is not True:
        return "the mixed fleet did not finalise the first epoch boundary"
    if install.get("marker_present") is not True:
        return "the prior binary's marker/contract install was not readable on validator 3"
    if not old_sha or not new_sha:
        return "a binary sha was unreadable"
    if old_sha == new_sha:
        return "the mixed arm did not run two distinct binaries"
    return "the prior-format path was not exercised"


def mixed_store_episode(*, boundary: dict, old_sha, new_sha, install: dict,
                        invoke: dict, reread: dict, store: dict,
                        result_present: bool) -> dict:
    """The DE-18 episode, derived from the mixed arm's measured prior-path facts.

    The deviation is a prior-format contract path that stalls while the upgrade reports success.
    `trigger_exercised` is true only when the prior binary finalised an epoch boundary, wrote the
    contract and marker, and runs a distinct binary from the current one. The prior path is alive
    only when the marker and the pre-swap block's state triple are readable on the upgraded
    validator *and* the invoked contract's result is present. A missing result is the deviation.
    """
    trigger_exercised = bool(
        boundary.get("crossed") is True
        and install.get("marker_present") is True
        and old_sha and new_sha and old_sha != new_sha)
    old_path_alive = bool(
        reread.get("marker_present") is True
        and reread.get("state_triple_matches") is True
        and result_present is True)
    deviation_observed = bool(trigger_exercised and not old_path_alive)
    return {
        "cause": DE18_CAUSE,
        "shape": DE18_SHAPE,
        "representation": DE18_REPRESENTATION,
        "rung": "R3",
        "law": "Law 53b",
        "deviations": ["DE-18"],
        "exercise": {
            "trigger": "a persistent contract installed by the prior binary is invoked after "
                       "the current binary opens the store the prior binary wrote",
            "trigger_exercised": trigger_exercised,
            "not_exercised_reason": (
                None if trigger_exercised
                else _not_exercised_reason(boundary, old_sha, new_sha, install)),
            "deviation_observed": deviation_observed,
            "safeguard_observed": bool(trigger_exercised and not deviation_observed),
            "old_binary_sha256": old_sha,
            "new_binary_sha256": new_sha,
            "result_present": bool(result_present),
            "boundary": boundary,
            "install": install,
            "invoke": invoke,
            "prior_block_reread": reread,
            "store_format": store,
        },
    }


def _store_format_summary(before: list[dict], after: list[dict]) -> dict:
    """The store-format decision validator 3 recorded, before and after the swap.

    The prior collection carries the old binary's `build`; the post-swap collection carries the
    current binary's `build`/`present`/`decision`. `same_format` and `format_boundary_crossed`
    are recorded facts: this run does not cross a format boundary, and it must not imply one.
    """
    def last(rows: list[dict], key: str):
        values = [row.get(key) for row in rows if row.get(key) is not None]
        return values[-1] if values else None

    written = last(before, "build")
    read_build = last(after, "build")
    return {
        "validator": 3,
        "written_build": written,
        "read_build": read_build,
        "read_present": last(after, "present"),
        "read_decision": last(after, "decision"),
        "same_format": (written is not None and read_build is not None
                        and written == read_build),
        "format_boundary_crossed": (written is not None and read_build is not None
                                    and written != read_build),
    }


def _mixed(record: dict, out: Path, old_image: str, keep: bool) -> None:
    devnet = common.Devnet("rc7mix", validators=4, http_base=44403, admin_base=44405,
                           grpc_base=44402, node_images={3: old_image},
                           extra_up=("--epoch-length", str(EPOCH_LENGTH)))
    try:
        devnet.up()
        _collect_format_traces(devnet, out / "traces" / "mixed", record)
        target = devnet.target()
        # **The prior binary must execute an epoch boundary before the swap.** A boundary runs
        # `close_block` and writes the native PoS state, so the store the new binary opens holds
        # state the prior binary's contract path produced — not only a fresh-genesis marker.
        boundary = {"epoch_length": EPOCH_LENGTH, "crossed": False, "height": None}
        def boundary_reached():
            height, _ = target.finalized_head(3)
            return height if height >= EPOCH_LENGTH else None
        boundary["height"] = common.wait_until(
            "the mixed fleet finalises past the first epoch boundary",
            boundary_reached, timeout=600)
        boundary["crossed"] = True
        stamp = str(time.time_ns())
        marker = "release-claim:" + stamp
        contract_name = "release-claim:contract:" + stamp
        result_name = "release-claim:result:" + stamp
        value = common.MARKER_VALUE
        # **The prior-format contract path (DE-18).** The old binary installs a persistent
        # contract and writes the marker in one block; after the swap the current binary must
        # execute the contract from the store the old binary wrote. The marker wait below is also
        # the install wait: both terms are in the same deploy, so the block both binaries compare
        # carries the contract's persisted continuation.
        term = (f'contract @{json.dumps(contract_name)}(_) = '
                f'{{ @{json.dumps(result_name)}!({value}) }} | '
                f'@{json.dumps(marker)}!({value})')
        submitted_via = 3
        try:
            deploy_id = devnet.deploy(term, index=3)
        except common.ClaimError:
            # The prior CLI may refuse a deploy; the marker and block are still the prior
            # binary's if it processes them, which the reads below measure.
            submitted_via = 0
            deploy_id = devnet.deploy(term, index=0)
        common.wait_until(
            "the mixed fleet finalises the marker on every validator",
            lambda: all(common.marker_present(
                target.probe(i, marker, target.finalized_head(i)[1]), value)
                for i in range(4)), timeout=360)
        write_height, write_hash = target.finalized_head(3)
        write_present = common.marker_present(
            target.probe(3, marker, write_hash), value)
        write_triple = _state_triple(target.block_info(3, write_hash))
        install = {"deploy_id": deploy_id, "submitted_via": submitted_via,
                   "height": write_height, "hash": write_hash,
                   "marker_present": write_present}
        height, block_hash = target.finalized_head(0)
        old_sha = _binary_sha(devnet.node(3))
        new_sha = _binary_sha(devnet.node(0))
        agreement, refusal = False, None
        try:
            old_info = target.block_info(3, block_hash)
            new_info = target.block_info(0, block_hash)
            agreement = _state_triple(old_info) == _state_triple(new_info)
            record["observations"]["mixed_block"] = {
                "height": height, "hash": block_hash,
                "old": _state_triple(old_info), "new": _state_triple(new_info)}
        except Exception as error:  # noqa: BLE001
            refusal = f"{type(error).__name__}: {error}"
        logs = subprocess.run(["docker", "logs", devnet.node(3)], text=True,
                              capture_output=True, timeout=60)
        lines = [line for line in (logs.stdout + logs.stderr).splitlines()
                 if any(word in line.lower() for word in
                        ("reject", "refus", "fork", "equivocat", "incompatible"))]
        if lines and not agreement:
            refusal = "\n".join(lines[-20:])
        record["observations"]["mixed"] = {
            "old_image": old_image, "old_binary_sha256": old_sha,
            "new_binary_sha256": new_sha, "deploy_id": deploy_id,
            "agreement": agreement, "refusal": refusal,
            "upgrade": {"heads_agree": False, "marker_present": False},
        }
        # **A rolling upgrade replaces one validator; the rest keep serving.** Restarting the whole
        # fleet also restarts the standalone bootstrap, and NodeLaunch refuses a populated DAG it
        # cannot attribute without a stored checkpoint certificate or a checkpoint source. The
        # swap therefore recreates only validator 3 on the current image, keeping its store, while
        # the other three continue.
        _restart_on_current_image(devnet.node(3))
        upgraded = devnet
        common.wait_until(
            "validator 3 answers after the swap",
            lambda: upgraded._ready(3), timeout=360)
        upgraded_target = upgraded.target()
        # **A live chain advances between four sequential reads.** Equality of a single snapshot
        # fails a correct upgrade when one node finalises the next block while the reads are in
        # flight. The claim is the common finalised head: the lowest head read must be finalised on
        # every validator.
        def common_finalized_head():
            heads = [upgraded_target.finalized_head(i) for i in range(4)]
            lowest = min(heads, key=lambda pair: pair[0])
            if all(upgraded_target.is_finalized(i, lowest[1]) for i in range(4)):
                return {"height": lowest[0], "hash": lowest[1]}
            return None

        common_head = common.wait_until(
            "the upgraded node reaches the common finalised head",
            common_finalized_head, timeout=600)
        before_formats = [row for row in record["observations"].get("format_traces") or []
                          if row.get("validator") == 3]
        _collect_format_traces(upgraded, out / "traces" / "upgraded", record)
        after_formats = [row for row in record["observations"].get("format_traces") or []
                         if row.get("validator") == 3][len(before_formats):]
        # The upgraded node reaches the head as soon as its store opens; the state read at that
        # head can lag the finality read. Give the marker a bounded window rather than a single
        # snapshot (the same race the RC-1 finalised-head read has).
        deadline = time.monotonic() + 120
        while True:
            head_height, head_hash = upgraded_target.finalized_head(3)
            present = common.marker_present(upgraded_target.probe(3, marker, head_hash), value)
            if present or time.monotonic() >= deadline:
                break
            time.sleep(3)
        record["observations"]["mixed"]["upgrade"] = {
            "heads_agree": True,
            "common_height": common_head["height"],
            "common_hash": common_head["hash"],
            "marker_present": present, "height": head_height, "hash": head_hash}
        # **Invoke the contract the prior binary installed.** A persistent contract is part of the
        # state; executing it on the upgraded node is the prior-format contract path DE-18 names.
        # A missing settlement or result is recorded as the deviation; the claim's own predicate
        # then fails, so a dead path cannot pass on the marker alone.
        invoke = {"deploy_id": None, "height": None, "hash": None,
                  "settled": False, "error": None}
        result_present = False
        try:
            invoke_id = devnet.deploy(f'@{json.dumps(contract_name)}!(Nil)')
            invoke["deploy_id"] = invoke_id

            def invocation_settled() -> bool:
                row = upgraded_target.settlement(0, invoke_id)
                return (row.get("stage") == "finalizedSuccess"
                        and row.get("settlementConfirmed") is True)

            common.wait_until("the prior-format contract invocation settles",
                              invocation_settled, timeout=360)
            invoke["settled"] = True

            def result_on_every_validator():
                head = common_finalized_head()
                if not head:
                    return None
                rows = {i: common.marker_present(
                    upgraded_target.probe(i, result_name, head["hash"]), value)
                    for i in range(4)}
                return {"head": head, "rows": rows} if all(rows.values()) else None

            resolved = common.wait_until(
                "the prior-format contract result is readable on every validator",
                result_on_every_validator, timeout=360)
            invoke["height"] = resolved["head"]["height"]
            invoke["hash"] = resolved["head"]["hash"]
            result_present = bool(resolved["rows"][3])
        except Exception as error:  # noqa: BLE001 - the failure is the observation
            invoke["error"] = f"{type(error).__name__}: {error}"
        # **Re-read the block the prior binary wrote.** The marker alone is a head read; the
        # pre-swap block's state triple is the prior binary's own derivation, and the upgraded
        # node must reproduce it from the store it inherited.
        reread = {"height": write_height, "hash": write_hash, "marker_present": False,
                  "state_triple_matches": False, "error": None}
        try:
            reread["state_triple_matches"] = (
                _state_triple(upgraded_target.block_info(3, write_hash)) == write_triple)
            reread["marker_present"] = common.marker_present(
                upgraded_target.probe(3, marker, write_hash), value)
        except Exception as error:  # noqa: BLE001 - the failure is the observation
            reread["error"] = f"{type(error).__name__}: {error}"
        record["observations"]["prior_path"] = {
            "install_deploy_id": deploy_id,
            "submitted_via": submitted_via,
            "install_height": write_height,
            "install_hash": write_hash,
            "installed_on_old_binary": old_sha is not None and old_sha != new_sha,
            "invoke_deploy_id": invoke["deploy_id"],
            "invoke_height": invoke["height"],
            "invoke_hash": invoke["hash"],
            "prior_block_reread": reread,
            "read_after_upgrade": reread["marker_present"],
            "result_present": result_present,
        }
        store = _store_format_summary(before_formats, after_formats)
        record["observations"]["store_format"] = store
        record["observations"]["stall_episode"] = mixed_store_episode(
            boundary=boundary, old_sha=old_sha, new_sha=new_sha,
            install=install, invoke=invoke, reread=reread, store=store,
            result_present=result_present)
    finally:
        if not keep:
            devnet.down()


def _collect_format_traces(devnet, dest: Path, record: dict) -> None:
    """Copy each node's `FormatTrace` files before its container is torn down."""
    rows = record["observations"].setdefault("format_traces", [])
    for i in range(4):
        node_dest = dest / f"node-{i}"
        try:
            devnet.copy_traces(i, node_dest)
        except Exception:  # noqa: BLE001 - an old image may not write traces
            continue
        for path in sorted(node_dest.glob("format-*.json")):
            try:
                rows.append({"validator": i, "file": path.name, **json.loads(path.read_text())})
            except ValueError:
                continue


def _restart_on_current_image(name: str) -> None:
    """Recreate one devnet container from its own inspect, on the selected image.

    A rolling upgrade keeps the volume and the genesis bind mount; only the binary changes. The
    container is rebuilt from the inspect data rather than from a second `devnet.sh up`, because
    that would restart the whole fleet (and the bootstrap's populated-DAG gate).
    """
    info = json.loads(subprocess.check_output(["docker", "inspect", name], text=True))[0]
    subprocess.run(["docker", "rm", "-f", name], text=True, capture_output=True, timeout=120)
    config, host = info["Config"], info["HostConfig"]
    args = ["docker", "run", "-d", "--name", name,
            "--network", host.get("NetworkMode") or "bridge"]
    for container_port, bindings in (host.get("PortBindings") or {}).items():
        for binding in bindings or []:
            host_ip = binding.get("HostIp") or "0.0.0.0"
            args += ["-p", f"{host_ip}:{binding['HostPort']}:{container_port.split('/')[0]}"]
    for mount in info.get("Mounts") or []:
        if mount.get("Type") == "volume":
            args += ["-v", f"{mount['Name']}:{mount['Destination']}"]
        elif mount.get("Type") == "bind":
            mode = "" if mount.get("RW") else ":ro"
            args += ["-v", f"{mount['Source']}:{mount['Destination']}{mode}"]
    for env in config.get("Env") or []:
        args += ["-e", env]
    if config.get("Entrypoint"):
        args += ["--entrypoint", config["Entrypoint"][0]]
    args += [os.environ.get("RNODE_IMAGE", "rnode:local")]
    args += list(config.get("Cmd") or [])
    result = subprocess.run(args, text=True, capture_output=True, timeout=120)
    if result.returncode:
        raise common.ClaimError(f"restarting {name} failed: {result.stderr[-800:]}")


def _binary_sha(container: str):
    try:
        return subprocess.check_output(
            ["docker", "exec", container, "sha256sum", "/usr/local/bin/rnode"],
            text=True, stderr=subprocess.DEVNULL).split()[0]
    except Exception:
        return None


def _read_json(path: Path):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return None


if __name__ == "__main__":
    raise SystemExit(run(sys.argv[1:]))
