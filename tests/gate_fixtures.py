"""Base observation documents and the shared mutation vocabulary for gate criteria.

The generic meta-test (`tests/test_gate_principles.py`) builds one satisfying observation per
registered criterion, then applies the criterion's declared negative controls. The fixture is
one document, not one test per gate: adding a criterion is a data edit.

Mutation paths use JSON Pointer separators (`/`), with `~1` for a literal slash in a key and
numeric segments for list indices. `remove_where`/`set_where` predicates use dotted keys.
"""
from __future__ import annotations

import copy
import hashlib
import importlib.util
import json
import tempfile
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
_TEMPDIRS: list[tempfile.TemporaryDirectory] = []


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


legacy = _load("gate_fixtures_live_gate", ROOT / "tests" / "test_live_gate.py")
c343 = _load("gate_fixtures_c343", ROOT / "tools" / "c343-live.py")
c357 = _load("gate_fixtures_c357", ROOT / "tools" / "c357-acceptance.py")
pack = _load("gate_fixtures_pack", ROOT / "tools" / "evidence-pack.py")

DEPLOY = legacy.DEPLOY
ALIASES = legacy.ALIASES
TOKEN_SOURCE = "a" * 40
TOKEN_BINARY = "b" * 64


def tempdir(under_target: bool = True) -> Path:
    # A Python-only CI job may run without compiling Rust first, so target/ need not exist.
    if under_target:
        (ROOT / "target").mkdir(parents=True, exist_ok=True)
    directory = (tempfile.TemporaryDirectory(dir=ROOT / "target") if under_target
                 else tempfile.TemporaryDirectory())
    _TEMPDIRS.append(directory)
    return Path(directory.name)


def cleanup() -> None:
    while _TEMPDIRS:
        _TEMPDIRS.pop().cleanup()


def _pack_record(gate_id: str, token: str) -> str:
    base = tempdir(under_target=False)
    run = base / "run"
    run.mkdir()
    (run / "source-sha.txt").write_text(TOKEN_SOURCE + "\n")
    (run / "binary-sha256.txt").write_text(TOKEN_BINARY + "\n")
    (run / "result.json").write_text(json.dumps({"result": "PASS"}))
    record = base / gate_id
    args = SimpleNamespace(
        run_dir=str(run), record=str(record), gate_id=gate_id, verdict="PASS",
        source_sha=None, binary_sha256=None,
        run_url="https://github.com/rhovision-org/rholo/actions/runs/1",
        reference_url=None, locality="local-devnet", redact=[], move=False,
    )
    pack.cmd_pack(args)
    readme = record / "README.md"
    readme.write_text(pack.PLACEHOLDER.sub("authored", readme.read_text())
                      + f"\nCriterion token: {token}\n")
    return str(record)


def manifest_sha256(record: str | Path) -> str:
    """The digest a criterion's `evidence.digest` must carry: sha256(manifest.json)."""
    return hashlib.sha256((Path(record) / "manifest.json").read_bytes()).hexdigest()


# --------------------------------------------------------------------------------------------------
# per-gate base observations


def _run_document() -> dict:
    return {
        "schema": 1,
        "tool": "live-gate",
        "deploy_id": DEPLOY,
        "started_utc": "2026-11-14T10:00:00Z",
        "ended_utc": "2026-11-14T10:12:00Z",
        "validators": [
            {"alias": alias, "endpoint_sha256": hashlib.sha256(alias.encode()).hexdigest()}
            for alias in ALIASES
        ],
        "counts": {},
        "execution_proven": False,
    }


def base_g428_m1() -> dict:
    """A supported-tier record: four validators, one per host, equal stake.

    The cgroup limit is present and equal to the declared 8 GiB share; the four-host layout is
    the one that tolerates a one-host loss under the code's supermajority rule.
    """
    def validator(index: int) -> dict:
        return {
            "id": f"v{index}",
            "host": f"host-{'abcd'[index - 1]}",
            "container": f"n420-validator-{index}",
            "stake": 100,
            "measured": {
                "rss_mb": 512,
                "peak_rss_mb": 900,
                "mem_total_mb": 32768,
                "cgroup_limit_mb": 8192,
                "cpu_cores": 2.0,
                "host_cores": 8,
                "data_dir_free_mb": 200000,
                "rnode_processes": 1,
                "chain_height": 5200,
                "replay_seconds": 240,
                "replay_peak_rss_mb": 1200,
                "replay_height": 5000,
                "oom_kills_delta": 0,
                "restart_count_delta": 0,
                "fallback_states": ["normal", "throttled"],
                "runtime_override": False,
                "argv": ["rnode", "run", "--bootstrap", "rnode://host-a:40400"],
                "container_config": {"cmd": ["run", "--bootstrap", "rnode://host-a:40400"]},
            },
        }

    return {"record": {
        "schema": 1,
        "kind": "supported-tier",
        "declared": {
            "tier": "minimal",
            "per_validator": {"cores": 2, "ram_gib": 8, "disk_gib": 100},
            "max_peak_rss_mb": 6000,
            "max_replay_seconds": 600,
            "served_height": 5000,
            "host_failure_tolerance": 1,
            "validator_count": 4,
            "total_stake": 400,
        },
        "validators": [validator(index) for index in range(1, 5)],
    }}


def _m2_jsonl(records: list[dict]) -> str:
    return "".join(json.dumps(record) + "\n" for record in records)


def base_g428_m2() -> dict:
    """One complete pressure record: capped, unset, direct-socket and gRPC.

    The single source of the synthetic transcripts, so a fixture and the checker cannot drift
    apart. The record is inline under `pressure.record` so the criterion's declared controls can
    mutate its fields; the raw files stay separate so editing a transcript without editing the
    summary is the failure the checker recomputes.
    """
    message = "HTTP in-flight capacity exhausted"
    capped_http = (
        [
            {"kind": "held", "status": None, "retry_after": None, "body": "", "t_ms": 0},
            {"kind": "held", "status": None, "retry_after": None, "body": "", "t_ms": 100},
        ]
        + [
            {"kind": "probe", "status": 429, "retry_after": 1, "body": message,
             "t_ms": 200 + 50 * index}
            for index in range(4)
        ]
        + [
            {"kind": "after", "status": 400, "retry_after": None, "body": "invalid json",
             "t_ms": 500},
        ]
    )
    unset_http = [
        {"kind": "probe", "status": 400, "retry_after": None, "body": "invalid json",
         "t_ms": 120 * index}
        for index in range(6)
    ]
    capped_direct = (
        "POST /api/v1/deploy HTTP/1.1\n"
        "Content-Length: 64\n\n"
        "HTTP/1.1 429 Too Many Requests\n"
        "retry-after: 1\n"
        "content-type: application/json\n\n"
        '{"error":"HTTP in-flight capacity exhausted"}\n'
        "-- release --\n"
        "HTTP/1.1 400 Bad Request\n"
    )
    unset_direct = "HTTP/1.1 400 Bad Request\n"
    grpc_capped = [
        {"arm": "ingress_rate", "code": 0, "message": "ok", "retry_after": None},
        {"arm": "ingress_rate", "code": 8, "message": "rate limited", "retry_after": None},
        {"arm": "pool_full", "code": 8, "message": "pool full", "retry_after": 1},
    ]
    grpc_permissive = [
        {"arm": "permissive", "code": 0, "message": "ok", "retry_after": None},
        {"arm": "permissive", "code": 0, "message": "ok", "retry_after": None},
    ]
    source_sha, binary = "a" * 40, "b" * 64
    record = {
        "schema": 1,
        "gate_id": "G428-M2",
        "locality": "public-live",
        "endpoint": {"url": "http://203.0.113.7:40403", "proxy": "none"},
        "candidate": {"source_sha": source_sha, "binary_sha256": binary},
        "runs": {
            "capped": {
                "env": {"RNODE_HTTP_MAX_INFLIGHT_DEPLOY": 2},
                "inspect": "raw/capped-inspect.json",
                "version": "raw/capped-version.txt",
            },
            "unset": {
                "env": {},
                "inspect": "raw/unset-inspect.json",
                "version": "raw/unset-version.txt",
            },
        },
        "surfaces": {
            "http": {
                "capped": {
                    "cap": 2, "held": 2, "probe": 4, "admitted": 0, "refused": 4,
                    "server_errors": 0, "retry_after": 4,
                    "refusal": {"class": "429", "message": message},
                    "handler_reached_after_release": True,
                    "transcript": "raw/http-capped.jsonl",
                    "requests_per_second": 20.0,
                },
                "unset": {
                    "probe": 6, "admitted": 6, "refused": 0, "server_errors": 0,
                    "retry_after": 0, "handler_reached_after_release": True,
                    "transcript": "raw/http-unset.jsonl",
                    "requests_per_second": 8.3,
                },
            },
            "direct_http": {
                "capped": {"status_line": "HTTP/1.1 429", "retry_after": 1,
                           "transcript": "raw/direct-capped.txt"},
                "unset": {"status_line": "HTTP/1.1 400",
                          "transcript": "raw/direct-unset.txt"},
            },
            "grpc": {
                "capped": {
                    "arms": {
                        "ingress_rate": {"admitted": 1, "refused": 1, "code": 8,
                                         "retry_after": None},
                        "pool_full": {"admitted": 0, "refused": 1, "code": 8,
                                      "retry_after": 1},
                    },
                    "server_errors": 0,
                    "transcript": "raw/grpc-capped.jsonl",
                },
                "permissive": {"admitted": 2, "refused": 0,
                               "transcript": "raw/grpc-permissive.jsonl"},
            },
        },
    }
    files = {
        "README.md": (
            "# M2 pressure witness\n\n"
            "**Evidence locality.** public-live — four independent hosts.\n"
        ),
        "raw/http-capped.jsonl": _m2_jsonl(capped_http),
        "raw/http-unset.jsonl": _m2_jsonl(unset_http),
        "raw/direct-capped.txt": capped_direct,
        "raw/direct-unset.txt": unset_direct,
        "raw/grpc-capped.jsonl": _m2_jsonl(grpc_capped),
        "raw/grpc-permissive.jsonl": _m2_jsonl(grpc_permissive),
        "raw/capped-inspect.json": json.dumps(
            [{"Config": {"Env": ["RNODE_HTTP_MAX_INFLIGHT_DEPLOY=2"]}}]
        ),
        "raw/unset-inspect.json": json.dumps([{"Config": {"Env": []}}]),
        "raw/capped-version.txt": f"commit {source_sha}\n",
        "raw/unset-version.txt": f"commit {source_sha}\n",
    }
    files["SHA256SUMS.json"] = json.dumps(
        {name: hashlib.sha256(text.encode()).hexdigest() for name, text in files.items()}
    )
    return {
        "source": {"text": (
            "const MAX_PENDING_BLOCKS: usize = 1024;\n"
            "const MAX_PROCESSOR_INPUT: usize = 1024;\n"
            "const MAX_VALIDATED_BLOCKS: usize = 1024;\n"
            "const MAX_TAP_VALIDATED_BLOCKS: usize = 1024;\n"
            "let (incoming_blocks_tx, incoming_blocks_rx) = mpsc::channel(MAX_PENDING_BLOCKS);\n"
            "let (processor_input_tx, processor_input_rx) = mpsc::channel(MAX_PROCESSOR_INPUT);\n"
            "let (validated_blocks_tx, validated_blocks_rx) = mpsc::channel(MAX_VALIDATED_BLOCKS);\n"
            "let (tap_tx, tap_rx) = mpsc::channel(MAX_TAP_VALIDATED_BLOCKS);\n"
        )},
        "pressure": {"record": record, "files": files},
    }


def m3_chain(
    joiner: str = "1" * 64,
    responder: str = "2" * 64,
    state: str = "a" * 64,
    blocks: list[str] | None = None,
    accepted_root: str = "c" * 64,
    locality: str = "off-host-claim",
    kind: str = "checkpoint",
    advertised_host: str = "203.0.113.7",
) -> dict:
    """One complete, internally consistent six-stage chain.

    The single source of the synthetic log lines the checker parses, so a fixture and the checker
    cannot drift apart. `accepted_root` deliberately differs from `state` for a checkpoint anchor:
    a certified join's accepted run root is the certificate's signed root, not the fringe hash.
    """
    if blocks is None:
        blocks = ["b" * 64]
    advertised = f"rnode://{joiner}@{advertised_host}?protocol=40400&discovery=40404"
    return {
        "schema": 1,
        "gate_id": "G428-M3",
        "tool": "tests/gate_fixtures.py",
        "generated_utc": "2026-11-18T00:00:00Z",
        "locality": locality,
        "complete": True,
        "incomplete_reasons": [],
        "logs": {"joiner": "joiner.log", "responder": "responder.log"},
        "correlation": {
            "joiner_key": joiner,
            "responder_key": responder,
            "advertised_peer": advertised,
            "anchor": "latest",
            "state_hash": state,
        },
        "stages": {
            "1_request": {
                "event": "request_sent",
                "peer": responder,
                "anchor": "latest",
                "attempt": "1/10",
                "source": "casper/src/protocol/comm_util.rs::request_initial_fringe_with_policy",
                "log_line": (
                    f"LFS fringe request 1/10 sent to peer {responder} (anchor latest); "
                    "awaiting admitted reply"
                ),
            },
            "2_receipt": {
                "event": "request_received",
                "peer": advertised,
                "peer_key": joiner,
                "anchor": "latest",
                "source": "casper/src/engine/node_running.rs::NodeRunning::handle",
                "log_line": (
                    f"Received FinalizedFringeRequest from {advertised} "
                    "(requested anchor latest)"
                ),
            },
            "3_construction": {
                "event": "response_constructed",
                "peer": advertised,
                "peer_key": joiner,
                "anchor": "latest",
                "state_hash": state,
                "block_count": len(blocks),
                "first_block": blocks[0],
                "last_block": blocks[-1],
                "source": "casper/src/engine/node_running.rs::NodeRunning::handle",
                "log_line": (
                    f"Fringe response prepared for {advertised}: state {state}, requested anchor "
                    f"latest, {len(blocks)} block(s) [{blocks[0]} .. {blocks[-1]}]"
                ),
            },
            "4_transport": {
                "event": "transport_outcome",
                "outcome": "delivered",
                "peer": advertised,
                "peer_key": joiner,
                "anchor": "latest",
                "state_hash": state,
                "source": "casper/src/engine/node_running.rs::handle_finalized_fringe_request",
                "log_line": (
                    f"FinalizedFringe sent to {advertised}: transport outcome delivered "
                    f"(state {state})"
                ),
            },
            "5_receive": {
                "event": "response_received",
                "peer": responder,
                "anchor": "latest",
                "state_hash": state,
                "blocks": list(blocks),
                "source": "casper/src/engine/node_syncing.rs::NodeSyncing::handle",
                "log_line": (
                    f"Received finalized fringe from bootstrap node ({state}) "
                    f"{' '.join(blocks)} from peer {responder}."
                ),
            },
            "6_apply": {
                "event": "state_restored",
                "peer": responder,
                "anchor": "latest",
                "served_state_hash": state,
                "restored_state_hash": state,
                "walk_completed": True,
                "walk_state_hash": state,
                "pages_finished": len(blocks),
                "pages_total": len(blocks),
                "checkpoint_readback": True,
                "accepted_root": accepted_root,
                "accepted_root_kind": kind,
                "source": "casper/src/engine/node_syncing.rs::report_restored_checkpoint",
                "log_line": (
                    f"LFS state is successfully restored. Served state {state}, accepted run "
                    f"root {accepted_root} ({kind}), approved checkpoint read back."
                ),
                "walk_log_line": (
                    f"LFS tuple-space walk finished for served state {state}: "
                    f"{len(blocks)} of {len(blocks)} page key(s) done."
                ),
            },
        },
        "non_claims": ["a fixture is not a run"],
    }


def m3_logs(chain: dict) -> tuple[str, str]:
    """The joiner and responder logs a chain's quoted lines must appear in."""
    stages = chain["stages"]
    joiner = "\n".join(
        [
            stages["1_request"]["log_line"],
            stages["5_receive"]["log_line"],
            stages["6_apply"]["walk_log_line"],
            stages["6_apply"]["log_line"],
        ]
    )
    responder = "\n".join(
        [
            stages["2_receipt"]["log_line"],
            stages["3_construction"]["log_line"],
            stages["4_transport"]["log_line"],
        ]
    )
    return joiner, responder


def base_g428_m3() -> dict:
    joiner, responder, state = "1" * 64, "2" * 64, "a" * 64
    readme = (
        "# M3 off-host witness\n\n"
        "**Evidence locality.** off-host-claim — the bootstrap dial-back from the second host.\n\n"
        "The bootstrap's dial-back to 203.0.113.7:40400 succeeded; the captured peer is "
        "raw/peer.txt.\n"
    )
    manifest = json.dumps({"locality": "off-host-claim", "execution_proven": False})
    chain = m3_chain(joiner=joiner, responder=responder, state=state)
    joiner_log, responder_log = m3_logs(chain)
    sums = json.dumps(
        {
            "joiner.log": hashlib.sha256(joiner_log.encode()).hexdigest(),
            "responder.log": hashlib.sha256(responder_log.encode()).hexdigest(),
        }
    )
    return {"record": {
        "files": {
            "README.md": readme,
            "raw/peer.txt": f"rnode://{joiner}@203.0.113.7?protocol=40400&discovery=40404\n",
            "manifest.json": manifest,
            "joiner.log": joiner_log,
            "responder.log": responder_log,
            "SHA256SUMS.json": sums,
        },
        "chain": chain,
    }}


def base_g428_m4() -> dict:
    anchor = "a" * 64
    record = _pack_record("G428-M4", anchor)
    return {"record_data": {
        "anchor": {"hash": anchor, "height": 100},
        "dwell": {"samples": 3, "stable": True},
        "window": {"seconds": 300},
        "validators": [
            {"id": "B", "finalized_anchor": True},
            {"id": "C", "finalized_anchor": True},
            {"id": "D", "finalized_anchor": True},
        ],
        "evidence": {"record": record, "digest": manifest_sha256(record)},
    }}


def base_g428_m5() -> dict:
    bundle = "e" * 64
    base = tempdir()
    capture = base / "capture.json"
    capture.write_text(json.dumps({"complete": True, "preimages": [{"verify_ok": True}]}))
    record = _pack_record("G428-M5", bundle)
    return {"record_data": {
        "bundle": {"available": True, "record": str(capture), "digest": bundle},
        "final_scope_rejected_chains": 1,
        "rejections_without_a_conflict": 0,
        "unapplied_kept_writes": 0,
        "per_deploy_effects": [
            {"deploy_id": "d1", "verdict": "accepted"},
            {"deploy_id": "d2", "verdict": "final_rejected"},
        ],
        "kept_readback": True,
        "evidence": {"record": record, "digest": manifest_sha256(record)},
    }}


def base_g584_g1() -> dict:
    return {"preflight": legacy.preflight()}


def base_g584_g2() -> dict:
    return {"subject": {"deploy_id": DEPLOY}, "run": _run_document(),
            "submission": legacy.submission(), "events": legacy.good_events()}


def base_g584_g3() -> dict:
    return {"claims": legacy.claims(), "events": legacy.good_events()}


def base_g584_g4() -> dict:
    return {"recovery": legacy.recovery(), "events": legacy.good_events()}


def base_g584_g5() -> dict:
    base = tempdir()
    run_dir = base / "run"
    run_dir.mkdir()
    run = _run_document()
    documents = {
        "run.json": json.dumps(run),
        "submission.json": json.dumps(legacy.submission()),
        "claims.json": json.dumps(legacy.claims()),
        "recovery.json": json.dumps(legacy.recovery()),
        "samples.jsonl": "\n".join(
            json.dumps(event, sort_keys=True) for event in legacy.good_events()) + "\n",
    }
    for name, text in documents.items():
        (run_dir / name).write_text(text)
    (run_dir / "SHA256SUMS").write_text("".join(
        f"{hashlib.sha256((run_dir / name).read_bytes()).hexdigest()}  {name}\n"
        for name in documents))
    return {"run": run, "events": legacy.good_events(),
            "record": {"dir": str(run_dir)}}


def base_c343() -> dict:
    return {"locality": "live-host", "health": c343._base_health(),
            "operator": c343._base_operator(), "block6": "block 6 accepted"}


def base_c357() -> dict:
    return {"record_data": c357._base_record("coordinated-new-genesis")}


ATTESTATION_RECORD = "spec/audit/evidence/c343-observation-shape-20261119"


def _attestation(url: str) -> dict:
    return {"attestation": {
        "reviewer": "independent-reviewer",
        "relationship": "not the gate owner, operator or author",
        "independent": True,
        "signed_at": "2026-11-18T12:00:00Z",
        "verdict": "PASS",
        # The signed digest is the cited record's manifest, not a format-valid placeholder.
        "evidence_sha256": manifest_sha256(ROOT / ATTESTATION_RECORD),
        "verdict_url": url,
        "evidence_record": ATTESTATION_RECORD,
    }}


def base_ta_c1() -> dict:
    return _attestation("https://example.invalid/ta-c1-verdict")


def base_ta_c2() -> dict:
    return _attestation("https://example.invalid/ta-c2-verdict")


BUILDERS = {
    "G428-M1": base_g428_m1,
    "G428-M2": base_g428_m2,
    "G428-M3": base_g428_m3,
    "G428-M4": base_g428_m4,
    "G428-M5": base_g428_m5,
    "G584-G1": base_g584_g1,
    "G584-G2": base_g584_g2,
    "G584-G3": base_g584_g3,
    "G584-G4": base_g584_g4,
    "G584-G5": base_g584_g5,
    "C343": base_c343,
    "C357": base_c357,
    "TA-C1": base_ta_c1,
    "TA-C2": base_ta_c2,
}


def base(gate_id: str) -> dict:
    if gate_id not in BUILDERS:
        raise KeyError(f"no base observation for {gate_id}")
    return BUILDERS[gate_id]()


# --------------------------------------------------------------------------------------------------
# mutation vocabulary


def _segments(path: str) -> list[str]:
    return [part.replace("~1", "/").replace("~0", "~") for part in path.split("/")]


def _parent(document: object, segments: list[str]):
    current = document
    for part in segments[:-1]:
        if isinstance(current, list):
            current = current[int(part)]
        else:
            current = current[part]
    return current, segments[-1]


def _assign(value: dict, dotted: str, new_value: object) -> None:
    parts = dotted.split(".")
    current = value
    for part in parts[:-1]:
        current = current.setdefault(part, {})
    current[parts[-1]] = copy.deepcopy(new_value)


def _match(value: object, predicate: dict) -> bool:
    if not isinstance(value, dict):
        return False
    for key, expected in predicate.items():
        current = value
        for part in key.split("."):
            if not isinstance(current, dict) or part not in current:
                return False
            current = current[part]
        if current != expected:
            return False
    return True


def apply_mutation(document: dict, mutation: object) -> dict:
    document = copy.deepcopy(document)
    if isinstance(mutation, list) and mutation and isinstance(mutation[0], list):
        for item in mutation:
            document = apply_mutation(document, item)
        return document
    if not isinstance(mutation, list) or not mutation:
        raise ValueError("a mutation must be an operation list")
    op = mutation[0]
    if op == "set":
        segments = _segments(mutation[1])
        container, key = _parent(document, segments)
        if isinstance(container, list):
            container[int(key)] = copy.deepcopy(mutation[2])
        else:
            container[key] = copy.deepcopy(mutation[2])
        return document
    if op == "delete":
        segments = _segments(mutation[1])
        container, key = _parent(document, segments)
        if isinstance(container, list):
            del container[int(key)]
        else:
            container.pop(key, None)
        return document
    if op == "append":
        segments = _segments(mutation[1])
        container, key = _parent(document, segments)
        target = container[int(key)] if isinstance(container, list) else container[key]
        target.append(copy.deepcopy(mutation[2]))
        return document
    if op == "remove_where":
        segments = _segments(mutation[1])
        container, key = _parent(document, segments)
        target = container[int(key)] if isinstance(container, list) else container[key]
        predicate = mutation[2]
        target[:] = [item for item in target if not _match(item, predicate)]
        return document
    if op == "set_where":
        segments = _segments(mutation[1])
        container, key = _parent(document, segments)
        target = container[int(key)] if isinstance(container, list) else container[key]
        predicate, update = mutation[2], mutation[3]
        for item in target:
            if _match(item, predicate):
                for key, value in update.items():
                    _assign(item, key, value)
        return document
    raise ValueError(f"unknown mutation operation {op!r}")


def controlled(gate_id: str, control: dict) -> dict:
    return apply_mutation(base(gate_id), control["mutation"])
