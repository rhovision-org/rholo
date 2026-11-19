#!/usr/bin/env python3
"""Criterion checks for the M4 rejoin and M5 merge-accounting records.

These check a campaign record, not the campaign. `rejoin` requires a fixed
anchor, a stable dwell and every-validator finalization inside the measured
healing window. `merge` requires the accounting fields, a per-deploy effect
list and a state readback; an unavailable historical bundle is PRECONDITION,
never PASS. A missing record is PRECONDITION.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

HEX40 = re.compile(r"^[0-9a-f]{40}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
FLOOR = 270.0
CEILING = 600.0


def nonneg(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


def _string(value: object) -> bool:
    return isinstance(value, str) and bool(value.strip())


def check_evidence(data: dict, gate_id: str, token: object) -> tuple[str, list[str]]:
    evidence = data.get("evidence")
    if not isinstance(evidence, dict):
        return "PRECONDITION", ["no evidence record"]
    record = evidence.get("record")
    path = Path(record) if isinstance(record, str) else None
    if path is not None and not path.is_absolute():
        path = ROOT / path
    if path is None or not path.is_dir():
        return "FAIL", ["evidence.record is not a retained directory"]
    if not ((path / "manifest.json").is_file() and (path / "SHA256SUMS.json").is_file()):
        return "FAIL", ["evidence.record needs manifest.json and SHA256SUMS.json"]
    proc = subprocess.run(
        [sys.executable, str(ROOT / "tools/evidence-pack.py"), "verify",
         "--record", str(path), "--require-pins"],
        capture_output=True, text=True, timeout=120, check=False)
    if proc.returncode != 0:
        return "FAIL", [f"evidence.record does not verify with pins: {proc.stderr.strip()}"]
    manifest = json.loads((path / "manifest.json").read_text())
    errors: list[str] = []
    if manifest.get("gate_id") != gate_id:
        errors.append(f"evidence names {manifest.get('gate_id')!r}, not {gate_id}")
    if manifest.get("execution_proven") is not False:
        errors.append("execution_proven must be false")
    readme = path / "README.md"
    text = readme.read_text(errors="ignore") if readme.is_file() else ""
    if not isinstance(token, str) or not token or token not in text:
        errors.append("the criterion token is not in the evidence record")
    digest = evidence.get("digest")
    if not (isinstance(digest, str) and HEX64.fullmatch(digest)):
        errors.append("evidence.digest must be a 64-hex digest")
    elif digest != hashlib.sha256((path / "manifest.json").read_bytes()).hexdigest():
        errors.append("evidence.digest does not match the record's manifest")
    return ("FAIL" if errors else "PASS"), errors


def check_rejoin(data: dict, floor: float = FLOOR, ceiling: float = CEILING,
                 min_dwell: int = 3,
                 verify_evidence: bool = True) -> tuple[str, list[str], dict]:
    errors: list[str] = []
    anchor = data.get("anchor") or {}
    if verify_evidence:
        evidence_status, evidence_errors = check_evidence(data, "G428-M4", anchor.get("hash"))
        if evidence_status != "PASS":
            return evidence_status, evidence_errors, {}
    if not (isinstance(anchor.get("hash"), str) and HEX64.fullmatch(anchor["hash"])):
        errors.append("anchor.hash must be a 64-hex digest")
    if not nonneg(anchor.get("height")):
        errors.append("anchor.height must be a non-negative integer")
    dwell = data.get("dwell") or {}
    if not (isinstance(dwell.get("samples"), int) and dwell["samples"] >= min_dwell):
        errors.append(f"dwell.samples must be at least {min_dwell}")
    if dwell.get("stable") is not True:
        errors.append("dwell.stable must be true")
    window = data.get("window") or {}
    seconds = window.get("seconds")
    if not (isinstance(seconds, (int, float)) and not isinstance(seconds, bool)):
        errors.append("window.seconds must be a number")
    elif not (floor <= float(seconds) <= ceiling):
        errors.append(f"window.seconds {seconds} is outside [{floor}, {ceiling}]")
    validators = data.get("validators") or []
    if not isinstance(validators, list) or not validators:
        errors.append("validators must be a non-empty list")
        validators = []
    seen_ids: set[str] = set()
    for validator in validators:
        if not isinstance(validator, dict):
            errors.append("every validator must be an object")
            continue
        ident = validator.get("id")
        if not (isinstance(ident, str) and ident.strip()):
            errors.append("every validator must carry a non-empty id")
        elif ident in seen_ids:
            errors.append(f"validator id {ident!r} is listed more than once")
        else:
            seen_ids.add(ident)
        if validator.get("finalized_anchor") is not True:
            errors.append(f"validator {ident!r} did not finalize the fixed anchor")
    return ("FAIL" if errors else "PASS"), errors, {"validators": len(validators)}


def check_finality_campaign(data: dict,
                            parameters: dict | None = None
                            ) -> tuple[str, list[str], dict]:
    """The M4 campaign envelope: N=4 and N=8 on declared hosts, across five arms.

    `check_rejoin` remains the anchor predicate; this function checks the campaign the pass
    condition names. A workload missing an arm is a FAIL (a submitted campaign record is
    incomplete), a field that cannot be read is a PRECONDITION, and a readable record that
    contradicts its declaration is a FAIL. The evidence record is the promotion layer's job,
    not this predicate's: embedding a record's own manifest digest in that record is a fixed
    point, so a campaign record is judged by its measurements and packed separately.
    """
    parameters = parameters or {}
    floor = parameters.get("floor", FLOOR)
    ceiling = parameters.get("ceiling", CEILING)
    min_dwell = parameters.get("min_dwell", 3)
    min_hosts = parameters.get("min_hosts", 2)
    min_cycles = parameters.get("min_cycles", 2)

    if not isinstance(data, dict):
        return "PRECONDITION", ["record_data is not an object"], {}
    schema = data.get("schema")
    if not isinstance(schema, int) or isinstance(schema, bool):
        return "PRECONDITION", ["record_data.schema is absent or not an integer"], {}
    if schema != 1:
        return "FAIL", [f"record_data.schema {schema!r} is not 1"], {}
    if "gate_id" not in data:
        return "PRECONDITION", ["record_data.gate_id is absent"], {}
    if data.get("gate_id") != "G428-M4":
        return "FAIL", [f"record_data.gate_id {data.get('gate_id')!r} is not 'G428-M4'"], {}

    preconditions: list[str] = []
    errors: list[str] = []

    campaign = data.get("campaign")
    if not isinstance(campaign, dict):
        return "PRECONDITION", ["campaign is absent"], {}
    if not _string(campaign.get("name")):
        errors.append("campaign.name is absent or empty")
    locality = campaign.get("locality")
    if locality is None:
        preconditions.append("campaign.locality is absent")
    elif locality != "multi-host":
        errors.append(f"campaign.locality {locality!r} is not 'multi-host'")
    selected = campaign.get("selected")
    if not isinstance(selected, dict):
        preconditions.append("campaign.selected is absent")
        selected = {}
    else:
        if selected.get("source_sha") is None:
            preconditions.append("campaign.selected.source_sha is absent")
        elif not (isinstance(selected.get("source_sha"), str)
                  and HEX40.fullmatch(selected["source_sha"])):
            errors.append("campaign.selected.source_sha is not 40-hex")
        if selected.get("binary_sha256") is None:
            preconditions.append("campaign.selected.binary_sha256 is absent")
        elif not (isinstance(selected.get("binary_sha256"), str)
                  and HEX64.fullmatch(selected["binary_sha256"])):
            errors.append("campaign.selected.binary_sha256 is not 64-hex")
    hosts_value = campaign.get("hosts")
    hosts: list[str] = []
    if not isinstance(hosts_value, list) or not hosts_value:
        preconditions.append("campaign.hosts is absent or empty")
    elif not all(_string(host) for host in hosts_value):
        errors.append("campaign.hosts carries a non-string or empty host id")
    elif len(set(hosts_value)) != len(hosts_value):
        errors.append("campaign.hosts lists a host id more than once")
    else:
        hosts = list(hosts_value)
        if len(hosts) < min_hosts:
            errors.append(
                f"campaign.hosts lists {len(hosts)} host(s), fewer than {min_hosts}")
    rerun = campaign.get("rerun")
    if not isinstance(rerun, dict):
        preconditions.append("campaign.rerun is absent")
        rerun = {}
    for key, count in (("n4", 4), ("n8", 8)):
        command = rerun.get(key)
        if not _string(command):
            preconditions.append(f"campaign.rerun.{key} is absent")
        elif "consensus-partition-rejoin.py" not in command:
            errors.append(f"campaign.rerun.{key} does not name the harness")
        elif f"--validators {count}" not in command:
            errors.append(f"campaign.rerun.{key} does not name --validators {count}")
    bootstrap = campaign.get("bootstrap")
    if not _string(bootstrap):
        preconditions.append("campaign.bootstrap is absent")

    workloads = data.get("workloads")
    if not isinstance(workloads, dict):
        preconditions.append("workloads is absent")
        workloads = {}
    elif set(workloads) != {"n4", "n8"}:
        errors.append(
            f"workloads lists {sorted(workloads)}; the gate requires exactly n4 and n8")

    all_ids: set[str] = set()
    cycles_total = 0
    for key in ("n4", "n8"):
        workload = workloads.get(key)
        if not isinstance(workload, dict):
            errors.append(f"workloads.{key} is absent or not an object")
            continue
        count = 4 if key == "n4" else 8
        if workload.get("validators") != count:
            errors.append(
                f"workloads.{key}.validators is {workload.get('validators')!r}, not {count}")
        approved = workload.get("approved_root")
        if approved is None:
            preconditions.append(f"workloads.{key}.approved_root is absent")
        elif not (isinstance(approved, str) and HEX64.fullmatch(approved)):
            errors.append(f"workloads.{key}.approved_root is not 64-hex")
            approved = None
        candidate = workload.get("candidate")
        if not isinstance(candidate, dict):
            preconditions.append(f"workloads.{key}.candidate is absent")
        elif selected:
            if candidate.get("source_sha") != selected.get("source_sha"):
                errors.append(
                    f"workloads.{key}.candidate.source_sha differs from the campaign")
            if candidate.get("binary_sha256") != selected.get("binary_sha256"):
                errors.append(
                    f"workloads.{key}.candidate.binary_sha256 differs from the campaign")
        capacity = workload.get("capacity")
        if not isinstance(capacity, dict):
            preconditions.append(f"workloads.{key}.capacity is absent")
            continue
        declared = capacity.get("declared") if isinstance(capacity.get("declared"), dict) else {}
        listed = capacity.get("validators")
        if not isinstance(listed, list) or not listed:
            preconditions.append(f"workloads.{key}.capacity.validators is absent")
            continue
        id_host: dict[str, str] = {}
        for entry in listed:
            if not isinstance(entry, dict):
                preconditions.append(
                    f"workloads.{key}.capacity has a non-object validator")
                continue
            ident, host = entry.get("id"), entry.get("host")
            if not _string(ident) or not _string(host):
                preconditions.append(
                    f"workloads.{key}.capacity has a validator without id/host")
                continue
            id_host[ident] = host
        all_ids.update(id_host)
        if len(id_host) != count:
            errors.append(
                f"workloads.{key}.capacity lists {len(id_host)} validator(s), not {count}")
        for ident, host in id_host.items():
            if hosts and host not in hosts:
                errors.append(
                    f"workloads.{key}: validator {ident!r} is on host {host!r}, "
                    "which campaign.hosts does not declare")
        tolerance = declared.get("host_failure_tolerance")
        if not isinstance(tolerance, int) or isinstance(tolerance, bool) or tolerance < 1:
            tolerance = 1
        expected = set(id_host)
        arms = workload.get("arms")
        if not isinstance(arms, dict):
            errors.append(f"workloads.{key}.arms is absent")
            continue
        for arm in ("deploy_finalization", "partition_rejoin", "host_loss",
                    "restart_replay", "safe_recovery"):
            if not isinstance(arms.get(arm), dict):
                errors.append(f"workloads.{key}.arms.{arm} is absent")

        finalization = arms.get("deploy_finalization")
        if isinstance(finalization, dict):
            if not _string(finalization.get("term")):
                errors.append(
                    f"workloads.{key}.arms.deploy_finalization.term is absent")
            if finalization.get("finalized_on_all") is not True:
                errors.append(
                    f"workloads.{key}: the deploy did not finalize on every validator")
            if finalization.get("approved_root_finalized_on_all") is not True:
                errors.append(
                    f"workloads.{key}: the approved root was not finalized before the faults")
            rows = finalization.get("per_validator")
            if not isinstance(rows, list):
                errors.append(
                    f"workloads.{key}.arms.deploy_finalization.per_validator is absent")
            else:
                seen: set[str] = set()
                for row in rows:
                    if not isinstance(row, dict) or not _string(row.get("id")):
                        errors.append(f"workloads.{key}: a finalization row has no validator id")
                        continue
                    ident = row["id"]
                    if ident not in expected:
                        errors.append(
                            f"workloads.{key}: a finalization row names an unknown validator")
                    elif ident in seen:
                        errors.append(
                            f"workloads.{key}: a finalization row for {ident!r} is duplicated")
                    seen.add(ident)
                    if row.get("finalized") is not True:
                        errors.append(
                            f"workloads.{key}: {ident!r} did not finalize the deploy")
                    if not nonneg(row.get("height")):
                        errors.append(
                            f"workloads.{key}: {ident!r} has no readable finalized height")
                if seen != expected:
                    errors.append(
                        f"workloads.{key}: finalization rows do not cover every validator")

        partition = arms.get("partition_rejoin")
        if isinstance(partition, dict):
            cycles = partition.get("cycles")
            if not isinstance(cycles, list) or not cycles:
                errors.append(
                    f"workloads.{key}: the partition/rejoin arm has no cycles")
                cycles = []
            elif len(cycles) < min_cycles:
                errors.append(
                    f"workloads.{key}: the partition/rejoin arm has {len(cycles)} cycle(s), "
                    f"fewer than {min_cycles}")
            cycles_total += len(cycles)
            victims: list[str] = []
            for index, cycle in enumerate(cycles):
                where = f"workloads.{key}.arms.partition_rejoin.cycles[{index}]"
                if not isinstance(cycle, dict):
                    errors.append(f"{where} is not an object")
                    continue
                victim = cycle.get("victim")
                if not _string(victim):
                    errors.append(f"{where}.victim is absent")
                elif victim not in expected:
                    errors.append(f"{where}.victim {victim!r} is not a workload validator")
                elif victim == bootstrap:
                    errors.append(f"{where}.victim is the bootstrap validator")
                else:
                    victims.append(victim)
                status, problems, _ = check_rejoin(
                    cycle, floor, ceiling, min_dwell, verify_evidence=False)
                if status == "FAIL":
                    errors.extend(f"{where}: {problem}" for problem in problems)
                elif status == "PRECONDITION":
                    preconditions.extend(f"{where}: {problem}" for problem in problems)
                listed_cycle = cycle.get("validators")
                ids = {entry.get("id") for entry in listed_cycle
                       if isinstance(entry, dict) and _string(entry.get("id"))} \
                    if isinstance(listed_cycle, list) else set()
                if ids != expected:
                    errors.append(
                        f"{where}: the validators listed are not the workload's validators")
                if approved is not None:
                    if cycle.get("root_before") != approved:
                        errors.append(f"{where}: root_before is not the approved root")
                    if cycle.get("root_after") != approved:
                        errors.append(f"{where}: root_after changed the approved root")
                finalized = cycle.get("approved_root_finalized")
                if not isinstance(finalized, list):
                    errors.append(f"{where}.approved_root_finalized is absent")
                else:
                    seen_final: set[str] = set()
                    for row in finalized:
                        if not isinstance(row, dict):
                            errors.append(
                                f"{where}.approved_root_finalized has a non-object")
                            continue
                        ident = row.get("id")
                        if not _string(ident) or ident not in expected:
                            errors.append(
                                f"{where}.approved_root_finalized names an unknown validator")
                        elif ident in seen_final:
                            errors.append(
                                f"{where}.approved_root_finalized lists {ident!r} twice")
                        else:
                            seen_final.add(ident)
                        if row.get("finalized") is not True:
                            errors.append(
                                f"{where}: a validator did not finalize the approved root")
                    if seen_final != expected:
                        errors.append(
                            f"{where}.approved_root_finalized does not cover every validator")
            if len(victims) != len(set(victims)):
                errors.append(
                    f"workloads.{key}: a partition victim is cut in more than one cycle")

        lost: list[str] = []
        survivors = set(id_host)
        host_loss = arms.get("host_loss")
        if isinstance(host_loss, dict):
            claimed_lost = host_loss.get("lost_hosts")
            if not isinstance(claimed_lost, list) or not claimed_lost \
                    or not all(_string(host) for host in claimed_lost):
                errors.append(f"workloads.{key}.arms.host_loss.lost_hosts is absent")
            else:
                lost = list(claimed_lost)
                for host in lost:
                    if hosts and host not in hosts:
                        errors.append(
                            f"workloads.{key}: lost host {host!r} is not declared")
                if len(lost) != tolerance:
                    errors.append(
                        f"workloads.{key}: {len(lost)} host(s) lost, "
                        f"not the declared tolerance {tolerance}")
                survivors = {ident for ident, host in id_host.items() if host not in set(lost)}
            listed_survivors = host_loss.get("survivors")
            if not isinstance(listed_survivors, list) \
                    or {item for item in listed_survivors if _string(item)} != survivors:
                errors.append(
                    f"workloads.{key}: host_loss.survivors is not the surviving validator set")
            rows = host_loss.get("per_validator")
            if not isinstance(rows, list):
                errors.append(f"workloads.{key}.arms.host_loss.per_validator is absent")
            else:
                seen_loss: set[str] = set()
                for row in rows:
                    if not isinstance(row, dict) or not _string(row.get("id")):
                        errors.append(f"workloads.{key}: a host-loss row has no validator id")
                        continue
                    ident = row["id"]
                    if ident not in survivors:
                        errors.append(
                            f"workloads.{key}: a host-loss row names a non-survivor")
                    elif ident in seen_loss:
                        errors.append(
                            f"workloads.{key}: a host-loss row for {ident!r} is duplicated")
                    seen_loss.add(ident)
                    if row.get("finalized_anchor") is not True:
                        errors.append(
                            f"workloads.{key}: a survivor did not finalize the anchor")
                    if approved is not None and row.get("root") != approved:
                        errors.append(
                            f"workloads.{key}: a survivor's root is not the approved root")
                if seen_loss != survivors:
                    errors.append(
                        f"workloads.{key}: host-loss rows do not cover every survivor")
            if approved is not None:
                if host_loss.get("root_before") != approved:
                    errors.append(
                        f"workloads.{key}.arms.host_loss.root_before is not the approved root")
                if host_loss.get("root_after") != approved:
                    errors.append(
                        f"workloads.{key}.arms.host_loss.root_after is not the approved root")
            if host_loss.get("retained") is not True:
                errors.append(
                    f"workloads.{key}: the anchor was not retained across host loss")

        restart = arms.get("restart_replay")
        if isinstance(restart, dict):
            if not _string(restart.get("validator")) or restart.get("validator") not in expected:
                errors.append(
                    f"workloads.{key}.arms.restart_replay.validator is unknown")
            seconds = restart.get("replay_seconds")
            if not isinstance(seconds, (int, float)) or isinstance(seconds, bool) \
                    or seconds <= 0:
                preconditions.append(
                    f"workloads.{key}.arms.restart_replay.replay_seconds is unreadable")
            for field in ("replay_height", "recovered_height"):
                value = restart.get(field)
                if not nonneg(value):
                    preconditions.append(
                        f"workloads.{key}.arms.restart_replay.{field} is unreadable")
            if nonneg(restart.get("replay_height")) and nonneg(restart.get("recovered_height")) \
                    and restart["recovered_height"] < restart["replay_height"]:
                errors.append(
                    f"workloads.{key}: the restart recovered below the replay height")
            if restart.get("anchor_still_present") is not True:
                errors.append(
                    f"workloads.{key}: the anchor was not present after the restart")
            if restart.get("fallback_state") != "normal":
                errors.append(
                    f"workloads.{key}: the restarted validator is not in the normal state")
            if restart.get("manual_restart") is not True:
                errors.append(
                    f"workloads.{key}: the restart was not recorded as a manual arm")
            if approved is not None:
                if restart.get("root_before") != approved:
                    errors.append(
                        f"workloads.{key}: the restart's root_before is not the approved root")
                if restart.get("root_after") != approved:
                    errors.append(
                        f"workloads.{key}: the restart changed the approved root")

        recovery = arms.get("safe_recovery")
        if isinstance(recovery, dict):
            lost_count = recovery.get("accepted_work_lost")
            if not nonneg(lost_count):
                preconditions.append(
                    f"workloads.{key}.arms.safe_recovery.accepted_work_lost is unreadable")
            elif lost_count != 0:
                errors.append(
                    f"workloads.{key}: recovery lost {lost_count} accepted deploy(s)")
            if recovery.get("self_replay") is not False:
                errors.append(f"workloads.{key}: a spurious self-replay fired")
            if recovery.get("own_key_halt") is not False:
                errors.append(f"workloads.{key}: an own-key halt fired on a healthy key")
            if approved is not None and recovery.get("approved_root") != approved:
                errors.append(
                    f"workloads.{key}: the recovery names a different approved root")
            if recovery.get("retained") is not True:
                errors.append(
                    f"workloads.{key}: the approved root was not retained by the recovery")
            rows = recovery.get("per_validator")
            if not isinstance(rows, list):
                errors.append(
                    f"workloads.{key}.arms.safe_recovery.per_validator is absent")
            else:
                seen_recovery: set[str] = set()
                for row in rows:
                    if not isinstance(row, dict) or not _string(row.get("id")):
                        errors.append(f"workloads.{key}: a recovery row has no validator id")
                        continue
                    ident = row["id"]
                    if ident not in survivors:
                        errors.append(
                            f"workloads.{key}: a recovery row names a non-survivor")
                    elif ident in seen_recovery:
                        errors.append(
                            f"workloads.{key}: a recovery row for {ident!r} is duplicated")
                    seen_recovery.add(ident)
                    if approved is not None and row.get("root_after") != approved:
                        errors.append(
                            f"workloads.{key}: a survivor's post-recovery root changed")
                if seen_recovery != survivors:
                    errors.append(
                        f"workloads.{key}: recovery rows do not cover every survivor")

    if _string(bootstrap) and bootstrap not in all_ids:
        errors.append(f"campaign.bootstrap {bootstrap!r} is not a workload validator")

    detail = {
        "workloads": sum(1 for key in ("n4", "n8")
                         if isinstance(workloads.get(key), dict)),
        "hosts": len(hosts),
        "cycles": cycles_total,
    }
    if preconditions:
        return "PRECONDITION", preconditions + errors, detail
    if errors:
        return "FAIL", errors, detail
    return "PASS", [], detail


def check_merge(data: dict) -> tuple[str, list[str], dict]:
    errors: list[str] = []
    bundle = data.get("bundle") or {}
    if bundle.get("available") is not True:
        return "PRECONDITION", ["the authenticated historical bundle is unavailable"], {}
    capture_path = bundle.get("record")
    path = Path(capture_path) if isinstance(capture_path, str) else None
    if path is not None and not path.is_absolute():
        path = ROOT / path
    if path is None or not path.is_file():
        return "FAIL", ["a bundle claimed available has no capture record"], {}
    try:
        capture = json.loads(path.read_text())
    except (OSError, ValueError):
        return "FAIL", ["the bundle capture record is unreadable"], {}
    if capture.get("complete") is not True:
        return "FAIL", ["the bundle capture record is not complete"], {}
    preimages = capture.get("preimages") or []
    if not isinstance(preimages, list) or not preimages:
        return "FAIL", ["the bundle capture record has no preimages"], {}
    if not all(isinstance(item, dict) and item.get("verify_ok") is True
               for item in preimages):
        return "FAIL", ["a bundle preimage did not verify"], {}
    digest = bundle.get("digest")
    if not (isinstance(digest, str) and HEX64.fullmatch(digest)):
        return "FAIL", ["bundle.digest must be a 64-hex digest"], {}
    evidence_status, evidence_errors = check_evidence(data, "G428-M5", digest)
    if evidence_status != "PASS":
        return evidence_status, evidence_errors, {}
    for field in ("final_scope_rejected_chains", "rejections_without_a_conflict",
                  "unapplied_kept_writes"):
        if not nonneg(data.get(field)):
            errors.append(f"{field} must be a non-negative integer")
    if nonneg(data.get("unapplied_kept_writes")) and data["unapplied_kept_writes"] != 0:
        errors.append("a kept write was not applied")
    if (nonneg(data.get("rejections_without_a_conflict"))
            and data["rejections_without_a_conflict"] != 0):
        errors.append("a rejection has no conflicting transition")
    effects = data.get("per_deploy_effects") or []
    if not isinstance(effects, list) or not effects:
        errors.append("per_deploy_effects must be a non-empty list")
        effects = []
    for effect in effects:
        if not isinstance(effect, dict):
            errors.append("every effect must be an object")
            continue
        if not (isinstance(effect.get("deploy_id"), str) and effect["deploy_id"].strip()):
            errors.append("every effect needs a deploy_id")
        if effect.get("verdict") not in ("accepted", "rejected", "final_rejected"):
            errors.append(f"unknown effect verdict {effect.get('verdict')!r}")
    if data.get("kept_readback") is not True:
        errors.append("kept_readback must be true; the report is not the state")
    return ("FAIL" if errors else "PASS"), errors, {
        "effects": len(effects),
        "final_scope_rejected_chains": data.get("final_scope_rejected_chains"),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("rejoin", "merge", "campaign"):
        command = sub.add_parser(name)
        command.add_argument("--record", required=True)
    args = parser.parse_args()
    path = Path(args.record)
    if not path.is_file():
        print(json.dumps({"result": "PRECONDITION", "detail": f"{path} is absent"}, sort_keys=True))
        return 3
    try:
        data = json.loads(path.read_text())
    except (OSError, ValueError) as exc:
        print(json.dumps({"result": "FAIL", "errors": [f"unreadable record: {exc}"]}, sort_keys=True))
        return 1
    checker = {"rejoin": check_rejoin, "merge": check_merge,
               "campaign": check_finality_campaign}[args.command]
    result, errors, detail = checker(data)
    print(json.dumps({"result": result, "errors": errors, "detail": detail,
                      "record_check": True}, sort_keys=True))
    return 0 if result == "PASS" else (3 if result == "PRECONDITION" else 1)


if __name__ == "__main__":
    raise SystemExit(main())
