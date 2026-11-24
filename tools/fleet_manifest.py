#!/usr/bin/env python3
"""The `rc-public-fleet` manifest contract.

The manifest is the campaign's input: four validator nodes, the candidate pins, the one deploy plan
and the action policy. It is deliberately **input-only**. Results — `harness_result`, `victim`,
`boot_ids`, `post_heads`, `certificates_after`, a pre-made `deploy_id` — are produced by
`tools/fleet-run.py` or read independently by the claim. A manifest that carries them is refused,
because a claim that reads its own answer from the operator's file is an attestation, not a test.

The validator is pure: no network, no clock, no filesystem beyond `load`. `preflight` in
`tests/release_claim_common.py` and `tools/fleet-run.py` both call it, so the manifest a run accepts
and the manifest the operator believes they wrote cannot drift apart.

Usage as a module:

    validate(manifest, claim_id="RC-4", environment="public", head=<git HEAD>)

Returns a list of problems, empty when the manifest is acceptable. A problem is a `not run`
coordinate, never a claim failure.
"""
from __future__ import annotations

import hashlib
import ipaddress
import json
import re
from pathlib import Path

SCHEMA = 1
CAMPAIGN = "rc-public-fleet"
TRANSPORTS = ("ssh-systemd", "docker")
LOCALITIES = ("public-live", "local-devnet", "single-host")
RELAY_MODES = ("relay", "four-ingress")
SOURCE_RE = re.compile(r"^[0-9a-f]{40}$")
DIGEST_RE = re.compile(r"^[0-9a-f]{64}$")

TOP_LEVEL = {
    "schema", "campaign", "generated_at_utc", "locality", "transport", "rehearsal",
    "provenance", "nodes", "deploy", "actions", "redaction", "operator_fields",
    # Typed inputs for the public arms beyond RC-1/RC-4.
    "fault", "availability", "pressure", "join", "upgrade",
    # Present so a stale result block is refused by name rather than as a typo.
    "results", "harness_result", "boot_ids", "victim", "post_heads", "certificates_after",
    "cycles", "pressure_record", "join_result", "audit_bootstrap", "upgrade_record",
    "redactions",
}
NODE_KEYS = {"name", "host", "ssh", "unit", "binary_path", "data_dir",
             "http", "admin", "grpc_host", "grpc_port", "p2p_host"}
FAULT_KEYS = {"kind", "cycles", "victim_policy", "p2p_ports", "mechanism",
              "dwell_blocks", "dwell_seconds", "heal_timeout_s"}
AVAILABILITY_KEYS = {"window_blocks", "probe_timeout_s"}
PRESSURE_KEYS = {"endpoint", "host", "port", "cap", "probes", "mode_order", "settle_s"}
JOIN_KEYS = {"joiner", "bootstrap_from", "fresh_store", "min_height", "timeout_s",
             "audit_config"}
UPGRADE_KEYS = {"old", "new_binary_path", "old_binary_path", "order", "staging_host",
                "mixed_window_blocks", "rollback_on_refusal", "timeout_s"}
DEPLOY_KEYS = {"from", "marker", "value", "term", "term_file", "term_sha256",
               "phlo_limit", "phlo_price", "shard_id", "private_key_path", "relay",
               "deploy_id", "stage", "receipts"}
RELAY_KEYS = {"mode", "aliases", "destination_port", "reachability_checked_utc",
              "unreachable_aliases", "signed_payload_file"}
ACTION_KEYS = {"survivor_advance_blocks", "rejoin_margin_blocks", "timeouts", "victim",
               "epoch_length", "retention_epochs"}
TIMEOUT_KEYS = {"preflight_s", "deploy_s", "settle_s", "stop_s", "advance_s",
                "start_s", "rejoin_s", "trace_s"}
REDACTION_KEYS = {"fields", "host_pseudonyms"}

# A result block is an output. It is never a legitimate input to any claim.
OUTPUT_KEYS = ("results", "boot_ids", "victim", "post_heads", "certificates_after",
               "harness_result", "cycles", "pressure_record", "join_result",
               "audit_bootstrap", "upgrade_record")
# The driver performs the fault and captures the witness for every arm it actuates.
ACTUATED_CLAIMS = ("RC-1", "RC-2", "RC-3", "RC-4", "RC-5", "RC-6", "RC-7")
DEPLOY_CLAIMS = ("RC-1", "RC-3", "RC-4")


def load(path: str | Path) -> dict:
    """Read a manifest file. Parse errors are the caller's to turn into a precondition."""
    return json.loads(Path(path).read_text())


def manifest_sha256(manifest: dict) -> str:
    """A stable digest of the manifest's content, independent of key order and spacing."""
    payload = json.dumps(manifest, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(payload).hexdigest()


def redact(manifest: dict) -> dict:
    """A copy safe to commit: endpoints, SSH targets, paths and operator identities removed.

    The redacted copy is evidence, not an input; re-validating it will fail the distinct-host
    check, which is the point — a committed manifest does not pretend to be a runnable one.
    """
    document = json.loads(json.dumps(manifest))
    redactions: list[str] = []
    for index, node in enumerate(document.get("nodes") or []):
        if not isinstance(node, dict):
            continue
        for field in ("host", "ssh", "http", "admin", "grpc_host", "data_dir"):
            if node.get(field):
                node[field] = "<redacted>"
                redactions.append(f"nodes[{index}].{field}")
    deploy = document.get("deploy")
    if isinstance(deploy, dict) and deploy.get("private_key_path"):
        deploy["private_key_path"] = "<redacted>"
        redactions.append("deploy.private_key_path")
    operator = document.get("operator_fields")
    if isinstance(operator, dict):
        for field, value in list(operator.items()):
            if isinstance(value, str) and value:
                operator[field] = "<redacted>"
                redactions.append(f"operator_fields.{field}")
    document["redactions"] = redactions
    return document


def _text(value) -> bool:
    return isinstance(value, str) and bool(value.strip())


def _bad_host_label(label: str) -> str | None:
    """Why a declared host label cannot be one of four independent public bases."""
    text = label.strip().lower()
    if not text:
        return "empty"
    if text == "localhost" or text.endswith(".localhost"):
        return "loopback"
    try:
        address = ipaddress.ip_address(text)
    except ValueError:
        return None
    if address.is_loopback:
        return "loopback"
    if address.is_private:
        return "private"
    if address.is_link_local:
        return "link-local"
    if address.is_reserved:
        return "reserved"
    if address.is_unspecified:
        return "unspecified"
    if address.is_multicast:
        return "multicast"
    return None


def _unknown(prefix: str, value: dict, allowed: set, problems: list[str]) -> None:
    extra = sorted(set(value) - allowed)
    if extra:
        problems.append(f"{prefix}: unknown keys {extra}")


def _positive_int(value) -> bool:
    return type(value) is int and value > 0


def _non_negative_int(value) -> bool:
    return type(value) is int and value >= 0


def validate(manifest, *, claim_id: str | None = None, environment: str = "public",
             head: str | None = None, rehearsal: bool = False) -> list[str]:
    """Return the manifest's problems; empty means the run's inputs can support a reading."""
    problems: list[str] = []
    if not isinstance(manifest, dict):
        return ["manifest: not an object"]
    _unknown("manifest", manifest, TOP_LEVEL, problems)

    if manifest.get("schema") != SCHEMA:
        problems.append(f"manifest: schema is {manifest.get('schema')!r}, expected {SCHEMA}")
    if manifest.get("campaign") != CAMPAIGN:
        problems.append(f"manifest: campaign is {manifest.get('campaign')!r}, expected {CAMPAIGN!r}")
    if not _text(manifest.get("generated_at_utc")):
        problems.append("manifest: generated_at_utc is missing")

    transport = manifest.get("transport")
    locality = manifest.get("locality")
    if transport not in TRANSPORTS:
        problems.append(f"manifest: transport {transport!r} is not one of {list(TRANSPORTS)}")
    if locality not in LOCALITIES:
        problems.append(f"manifest: locality {locality!r} is not one of {list(LOCALITIES)}")
    if transport == "ssh-systemd" and locality != "public-live":
        problems.append("manifest: the ssh-systemd transport is a public-live run")
    if transport == "docker" and locality == "public-live":
        problems.append("manifest: a docker transport cannot declare public-live")

    declared_rehearsal = manifest.get("rehearsal")
    if rehearsal:
        if transport != "docker":
            problems.append("manifest: a rehearsal uses the docker transport")
        if declared_rehearsal is not True:
            problems.append("manifest: a rehearsal manifest must set rehearsal true")
        if locality == "public-live":
            problems.append("manifest: a rehearsal cannot declare public-live")
    else:
        if declared_rehearsal is True:
            problems.append("manifest: a rehearsal manifest cannot be run as a public claim")
        if environment == "public":
            if transport != "ssh-systemd":
                problems.append("manifest: a public run uses the ssh-systemd transport")
            if locality != "public-live":
                problems.append("manifest: a public run declares locality public-live")

    problems.extend(_provenance(manifest.get("provenance"), environment, head, rehearsal))
    problems.extend(_nodes(manifest.get("nodes"), transport, environment, rehearsal))
    names = [node.get("name") for node in manifest.get("nodes") or [] if isinstance(node, dict)]

    for key in OUTPUT_KEYS:
        if key in manifest:
            problems.append(f"manifest: {key} is an output, not an input; the driver produces it")
    if claim_id in ACTUATED_CLAIMS and "harness_result" in manifest:
        problems.append(
            "manifest: harness_result is an output; the driver performs the action and records it")
    if claim_id in DEPLOY_CLAIMS:
        deploy = manifest.get("deploy")
        if isinstance(deploy, dict):
            for key in ("deploy_id", "stage", "receipts"):
                if key in deploy:
                    problems.append(
                        f"manifest: deploy.{key} is an output; the driver submits its own deploy")

    if claim_id in DEPLOY_CLAIMS:
        problems.extend(_deploy(manifest.get("deploy"), names, transport, locality))
    problems.extend(_actions(manifest.get("actions"), claim_id))
    problems.extend(_fault(manifest.get("fault"), claim_id))
    problems.extend(_availability(manifest.get("availability"), claim_id))
    problems.extend(_pressure(manifest.get("pressure"), claim_id, names, transport))
    problems.extend(_join(manifest.get("join"), claim_id, names,
                          manifest.get("nodes") or [], transport))
    problems.extend(_upgrade(manifest.get("upgrade"), claim_id, names, transport))
    redaction = manifest.get("redaction")
    if redaction is not None:
        if not isinstance(redaction, dict):
            problems.append("manifest: redaction is not an object")
        else:
            _unknown("redaction", redaction, REDACTION_KEYS, problems)
            if "fields" in redaction and not (
                    isinstance(redaction["fields"], list)
                    and all(isinstance(item, str) for item in redaction["fields"])):
                problems.append("manifest: redaction.fields is not a list of strings")
    if "operator_fields" in manifest and not isinstance(manifest["operator_fields"], dict):
        problems.append("manifest: operator_fields is not an object")
    if "redactions" in manifest and not isinstance(manifest["redactions"], list):
        problems.append("manifest: redactions is not a list")
    return problems


def _provenance(provenance, environment: str, head: str | None,
                rehearsal: bool) -> list[str]:
    problems: list[str] = []
    if not isinstance(provenance, dict):
        return ["manifest: provenance is missing"]
    source = provenance.get("source_sha")
    if not isinstance(source, str) or not SOURCE_RE.match(source):
        problems.append("manifest: provenance.source_sha is not a 40-hex commit")
    elif environment == "public" and not rehearsal and head and source != head:
        problems.append(
            f"manifest: provenance source_sha {source[:12]} is not HEAD {head[:12]} "
            "(a not-observed arm is satisfied only by the current candidate)")
    binary = provenance.get("binary_sha256")
    if not isinstance(binary, str) or not DIGEST_RE.match(binary):
        problems.append("manifest: provenance.binary_sha256 is not a 64-hex digest")
    for key in ("genesis_hash", "genesis_raw_sha256", "bonds_sha256"):
        value = provenance.get(key)
        if environment == "public" and not rehearsal and key not in provenance:
            problems.append(f"manifest: provenance.{key} is missing on a public run")
        elif value is not None and (not isinstance(value, str) or not DIGEST_RE.match(value)):
            problems.append(f"manifest: provenance.{key} is not a 64-hex digest")
    for key in ("network_id", "shard_id"):
        if key in provenance and not _text(provenance[key]):
            problems.append(f"manifest: provenance.{key} is not a string")
    return problems


def _nodes(nodes, transport, environment: str, rehearsal: bool) -> list[str]:
    problems: list[str] = []
    if not isinstance(nodes, list):
        return ["manifest: nodes is missing"]
    if len(nodes) != 4:
        problems.append(f"manifest: nodes has {len(nodes)} entries, expected 4 independent hosts")
    names: list[str] = []
    hosts: list[str] = []
    for index, node in enumerate(nodes):
        prefix = f"nodes[{index}]"
        if not isinstance(node, dict):
            problems.append(f"{prefix}: not an object")
            continue
        _unknown(prefix, node, NODE_KEYS, problems)
        for field in ("name", "host", "http"):
            if not _text(node.get(field)):
                problems.append(f"{prefix}: {field} is missing")
        name, host = node.get("name"), node.get("host")
        if _text(name):
            names.append(name)
        if _text(host):
            hosts.append(host)
            if environment == "public" and not rehearsal:
                reason = _bad_host_label(host)
                if reason:
                    problems.append(
                        f"{prefix}: the declared host {host!r} is {reason}; a public run needs four "
                        "independent public bases")
        if transport == "ssh-systemd":
            for field in ("ssh", "unit", "binary_path", "data_dir"):
                if not _text(node.get(field)):
                    problems.append(f"{prefix}: {field} is missing on an ssh-systemd node")
        if "grpc_port" in node and not (
                type(node["grpc_port"]) is int and 1 <= node["grpc_port"] <= 65535):
            problems.append(f"{prefix}: grpc_port is not a port")
        if "grpc_host" in node and not _text(node["grpc_host"]):
            problems.append(f"{prefix}: grpc_host is not a string")
        if "admin" in node and not _text(node["admin"]):
            problems.append(f"{prefix}: admin is not a URL")
    if len(names) != len(set(names)):
        problems.append("manifest: node names are not distinct")
    if len(hosts) != len(set(hosts)):
        problems.append("manifest: node host labels are not distinct")
    return problems


def _deploy(deploy, names: list[str], transport, locality) -> list[str]:
    problems: list[str] = []
    if not isinstance(deploy, dict):
        return ["manifest: deploy is missing for RC-1/RC-4"]
    _unknown("deploy", deploy, DEPLOY_KEYS, problems)
    ingress = deploy.get("from")
    if ingress not in names:
        problems.append(f"deploy.from {ingress!r} is not a node name")
    if not _text(deploy.get("marker")):
        problems.append("deploy.marker is missing")
    if type(deploy.get("value")) is not int:
        problems.append("deploy.value is not an integer")
    if not _positive_int(deploy.get("phlo_limit")):
        problems.append("deploy.phlo_limit is not a positive integer")
    if not _non_negative_int(deploy.get("phlo_price")):
        problems.append("deploy.phlo_price is not a non-negative integer")
    if not _text(deploy.get("shard_id")):
        problems.append("deploy.shard_id is missing")
    if transport == "ssh-systemd" and not _text(deploy.get("private_key_path")):
        problems.append("deploy.private_key_path is missing on an ssh-systemd run")
    if not _text(deploy.get("term")) and not _text(deploy.get("term_file")):
        problems.append("deploy needs a term or a term_file")
    if _text(deploy.get("term_file")):
        term_sha = deploy.get("term_sha256")
        if not isinstance(term_sha, str) or not DIGEST_RE.match(term_sha):
            problems.append("deploy.term_file needs a 64-hex term_sha256")
    relay = deploy.get("relay")
    if not isinstance(relay, dict):
        problems.append("deploy.relay is missing; the deploy must reach every validator")
    else:
        _unknown("deploy.relay", relay, RELAY_KEYS, problems)
        mode = relay.get("mode")
        if mode not in RELAY_MODES:
            problems.append(f"deploy.relay.mode {mode!r} is not one of {list(RELAY_MODES)}")
        aliases = relay.get("aliases")
        if not isinstance(aliases, list) or sorted(aliases) != sorted(names):
            problems.append("deploy.relay.aliases must name exactly the four nodes")
        if not _text(relay.get("reachability_checked_utc")):
            problems.append("deploy.relay.reachability_checked_utc is missing")
        if relay.get("unreachable_aliases") != []:
            problems.append("deploy.relay.unreachable_aliases must be empty")
        if mode == "four-ingress" and not _text(relay.get("signed_payload_file")):
            problems.append("deploy.relay.signed_payload_file is required for four-ingress")
        if "destination_port" in relay and not (
                type(relay["destination_port"]) is int
                and 1 <= relay["destination_port"] <= 65535) and relay["destination_port"] is not None:
            problems.append("deploy.relay.destination_port is not a port")
    return problems


def _port(value) -> bool:
    return type(value) is int and 1 <= value <= 65535


def _output_words(prefix: str, value: dict, words: tuple[str, ...],
                  problems: list[str]) -> None:
    for word in words:
        if word in value:
            problems.append(f"{prefix}: {word} is an output; the driver produces it")


def _fault(fault, claim_id: str | None) -> list[str]:
    if claim_id == "RC-2" and fault is None:
        return ["manifest: fault is missing; RC-2's partition window is an input"]
    if fault is None:
        return []
    if not isinstance(fault, dict):
        return ["manifest: fault is not an object"]
    problems: list[str] = []
    _unknown("fault", fault, FAULT_KEYS, problems)
    _output_words("fault", fault,
                  ("victim", "victim_index", "anchors", "containment", "harness_result",
                   "producing_victim"), problems)
    if fault.get("kind") != "partition":
        problems.append("manifest: fault.kind must be partition")
    if not _positive_int(fault.get("cycles")):
        problems.append("manifest: fault.cycles is not a positive integer")
    if not _text(fault.get("victim_policy")):
        problems.append("manifest: fault.victim_policy is missing")
    ports = fault.get("p2p_ports")
    if not isinstance(ports, list) or not ports or not all(_port(port) for port in ports):
        problems.append("manifest: fault.p2p_ports is not a non-empty list of ports")
    if fault.get("mechanism") not in ("nftables", "docker-network"):
        problems.append("manifest: fault.mechanism must be nftables or docker-network")
    for field in ("dwell_blocks", "dwell_seconds", "heal_timeout_s"):
        if not _positive_int(fault.get(field)):
            problems.append(f"manifest: fault.{field} is not a positive integer")
    return problems


def _availability(availability, claim_id: str | None) -> list[str]:
    if availability is None:
        return []
    if not isinstance(availability, dict):
        return ["manifest: availability is not an object"]
    problems: list[str] = []
    _unknown("availability", availability, AVAILABILITY_KEYS, problems)
    _output_words("availability", availability,
                  ("blocks", "not_available", "verdict"), problems)
    window = availability.get("window_blocks")
    if not (type(window) is int and window >= 10):
        problems.append("manifest: availability.window_blocks must be at least 10")
    if not _positive_int(availability.get("probe_timeout_s")):
        problems.append("manifest: availability.probe_timeout_s is not a positive integer")
    return problems


def _pressure(pressure, claim_id: str | None, names: list[str],
              transport: str | None) -> list[str]:
    if claim_id == "RC-5" and pressure is None:
        return ["manifest: pressure is missing; RC-5's endpoint and cap are inputs"]
    if pressure is None:
        return []
    if not isinstance(pressure, dict):
        return ["manifest: pressure is not an object"]
    problems: list[str] = []
    _unknown("pressure", pressure, PRESSURE_KEYS, problems)
    _output_words("pressure", pressure,
                  ("pressure_record", "transcript", "verdict", "counts", "refusal_class"),
                  problems)
    endpoint = pressure.get("endpoint")
    if not _text(endpoint) or not (endpoint.startswith("http://")
                                   or endpoint.startswith("https://")):
        problems.append("manifest: pressure.endpoint is not an http(s) URL")
    if pressure.get("host") not in names:
        problems.append("manifest: pressure.host is not a node name")
    if not _port(pressure.get("port")):
        problems.append("manifest: pressure.port is not a port")
    if not _positive_int(pressure.get("cap")):
        problems.append("manifest: pressure.cap is not a positive integer")
    if not _positive_int(pressure.get("probes")):
        problems.append("manifest: pressure.probes is not a positive integer")
    order = pressure.get("mode_order")
    if (not isinstance(order, list) or not order
            or any(mode not in ("capped", "unset") for mode in order)
            or len(set(order)) != len(order)):
        problems.append("manifest: pressure.mode_order must be a non-empty unique list of "
                        "capped/unset")
    elif claim_id == "RC-5" and set(order) != {"capped", "unset"}:
        problems.append("manifest: RC-5 needs both the capped and the unset lifetime")
    if not _positive_int(pressure.get("settle_s")):
        problems.append("manifest: pressure.settle_s is not a positive integer")
    if transport == "ssh-systemd" and not _text(pressure.get("host")):
        problems.append("manifest: pressure.host is missing on an ssh-systemd run")
    return problems


def _joiner(joiner, names: list[str], node_hosts: set[str], transport: str | None) -> list[str]:
    problems: list[str] = []
    if not isinstance(joiner, dict):
        return ["manifest: join.joiner is not an object"]
    _unknown("join.joiner", joiner, NODE_KEYS, problems)
    for field in ("name", "host", "http"):
        if not _text(joiner.get(field)):
            problems.append(f"join.joiner: {field} is missing")
    if joiner.get("name") in names:
        problems.append("join.joiner.name duplicates a validator node name")
    host = joiner.get("host")
    if _text(host):
        reason = _bad_host_label(host)
        if reason:
            problems.append(f"join.joiner: the declared host {host!r} is {reason}")
        if host in node_hosts:
            problems.append("join.joiner: the host is one of the four validators, not a fifth host")
    if transport == "ssh-systemd":
        for field in ("ssh", "unit", "binary_path", "data_dir"):
            if not _text(joiner.get(field)):
                problems.append(f"join.joiner: {field} is missing on an ssh-systemd joiner")
    return problems


def _join(join, claim_id: str | None, names: list[str], nodes: list[dict],
          transport: str | None) -> list[str]:
    if claim_id == "RC-6" and join is None:
        return ["manifest: join is missing; RC-6 must supply the joiner and its bootstrap"]
    if join is None:
        return []
    if not isinstance(join, dict):
        return ["manifest: join is not an object"]
    problems: list[str] = []
    _unknown("join", join, JOIN_KEYS, problems)
    _output_words("join", join,
                  ("join_result", "chain", "chain_problems", "audit", "joiner_checkpoint",
                   "audit_traces", "boot_traces"), problems)
    bootstrap = join.get("bootstrap_from")
    if bootstrap not in names:
        problems.append("manifest: join.bootstrap_from is not a node name")
    elif names and bootstrap == names[0]:
        problems.append("manifest: join.bootstrap_from must be a non-producer, not the ceremony node")
    if join.get("fresh_store") is not True:
        problems.append("manifest: join.fresh_store must be true; the driver never deletes a store")
    if not _positive_int(join.get("min_height")):
        problems.append("manifest: join.min_height is not a positive integer")
    if not _positive_int(join.get("timeout_s")):
        problems.append("manifest: join.timeout_s is not a positive integer")
    if "audit_config" in join:
        config = join["audit_config"]
        if isinstance(config, str):
            if not _text(config):
                problems.append("manifest: join.audit_config is not a path")
        elif isinstance(config, dict):
            _unknown("join.audit_config", config,
                     {"bonds_file", "wallets_file", "epoch_length", "dev_mode"}, problems)
            for key in ("bonds_file", "wallets_file"):
                if key in config and not _text(config[key]):
                    problems.append(f"manifest: join.audit_config.{key} is not a path")
            if "epoch_length" in config and not _positive_int(config["epoch_length"]):
                problems.append("manifest: join.audit_config.epoch_length is not a positive integer")
            if "dev_mode" in config and not isinstance(config["dev_mode"], bool):
                problems.append("manifest: join.audit_config.dev_mode is not a boolean")
        else:
            problems.append("manifest: join.audit_config is not a path or an object")
    node_hosts = {node.get("host") for node in nodes
                  if isinstance(node, dict) and _text(node.get("host"))}
    problems.extend(_joiner(join.get("joiner"), names, node_hosts, transport))
    return problems


def _upgrade(upgrade, claim_id: str | None, names: list[str],
             transport: str | None) -> list[str]:
    if claim_id == "RC-7" and upgrade is None:
        return ["manifest: upgrade is missing; RC-7's rolling swap is an input"]
    if upgrade is None:
        return []
    if not isinstance(upgrade, dict):
        return ["manifest: upgrade is not an object"]
    problems: list[str] = []
    _unknown("upgrade", upgrade, UPGRADE_KEYS, problems)
    _output_words("upgrade", upgrade,
                  ("upgrade_record", "mixed", "migration_result", "downgrade", "rolled_back"),
                  problems)
    old = upgrade.get("old")
    if not isinstance(old, dict):
        problems.append("manifest: upgrade.old is not an object")
    else:
        _unknown("upgrade.old", old, {"image", "source_sha", "binary_sha256"}, problems)
        digest = old.get("binary_sha256")
        if not isinstance(digest, str) or not DIGEST_RE.match(digest):
            problems.append("manifest: upgrade.old.binary_sha256 is not a 64-hex digest")
        source = old.get("source_sha")
        if source is not None and (not isinstance(source, str) or not SOURCE_RE.match(source)):
            problems.append("manifest: upgrade.old.source_sha is not a 40-hex commit")
    for field in ("new_binary_path", "old_binary_path", "staging_host"):
        if not _text(upgrade.get(field)):
            problems.append(f"manifest: upgrade.{field} is missing")
    order = upgrade.get("order")
    if not isinstance(order, list) or sorted(order) != sorted(names):
        problems.append("manifest: upgrade.order must be a permutation of the four node names")
    if not _positive_int(upgrade.get("mixed_window_blocks")):
        problems.append("manifest: upgrade.mixed_window_blocks is not a positive integer")
    if not isinstance(upgrade.get("rollback_on_refusal"), bool):
        problems.append("manifest: upgrade.rollback_on_refusal is not a boolean")
    if not _positive_int(upgrade.get("timeout_s")):
        problems.append("manifest: upgrade.timeout_s is not a positive integer")
    if transport == "ssh-systemd" and not _text(upgrade.get("staging_host")):
        problems.append("manifest: upgrade.staging_host is missing on an ssh-systemd run")
    return problems


def _actions(actions, claim_id: str | None) -> list[str]:
    if actions is None:
        return []
    if not isinstance(actions, dict):
        return ["manifest: actions is not an object"]
    problems: list[str] = []
    _unknown("actions", actions, ACTION_KEYS, problems)
    for key in ("survivor_advance_blocks", "rejoin_margin_blocks",
                "epoch_length", "retention_epochs"):
        if key in actions and not _positive_int(actions[key]):
            problems.append(f"actions.{key} is not a positive integer")
    if claim_id == "RC-1" and not _positive_int(actions.get("epoch_length")):
        problems.append(
            "actions.epoch_length is required by RC-1's retention check (two epoch boundaries)")
    if claim_id in ACTUATED_CLAIMS and "victim" in actions:
        problems.append(
            "actions.victim is an output decision; the driver chooses the victim after inclusion")
    timeouts = actions.get("timeouts")
    if timeouts is not None:
        if not isinstance(timeouts, dict):
            problems.append("actions.timeouts is not an object")
        else:
            _unknown("actions.timeouts", timeouts, TIMEOUT_KEYS, problems)
            for key, value in timeouts.items():
                if not _positive_int(value):
                    problems.append(f"actions.timeouts.{key} is not a positive integer")
    return problems
