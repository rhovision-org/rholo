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
    return {"record": {"host_processes": [{"pid": 101, "name": "rnode"}],
                       "declared_count": 1}}


def base_g428_m2() -> dict:
    return {"source": {"text": (
        "let (incoming_blocks_tx, incoming_blocks_rx) = mpsc::channel(MAX_PENDING_BLOCKS);\n"
        "let (processor_input_tx, processor_input_rx) = mpsc::channel(MAX_PROCESSOR_INPUT);\n"
        "let (validated_blocks_tx, validated_blocks_rx) = mpsc::channel(MAX_VALIDATED_BLOCKS);\n"
        "let (tap_tx, tap_rx) = mpsc::channel(MAX_TAP_VALIDATED_BLOCKS);\n"
    )}}


def base_g428_m3() -> dict:
    readme = (
        "# M3 off-host witness\n\n"
        "**Evidence locality.** off-host-claim — the bootstrap dial-back from the second host.\n\n"
        "The dial-back result is captured in raw/peer.txt.\n"
    )
    manifest = json.dumps({"locality": "off-host-claim", "execution_proven": False})
    return {"record": {"files": {
        "README.md": readme,
        "raw/peer.txt": "rnode://abc@203.0.113.7\n",
        "manifest.json": manifest,
    }}}


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
        "evidence": {"record": record, "digest": "d" * 64},
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
        "evidence": {"record": record, "digest": "d" * 64},
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
    return {"health": c343._base_health(), "operator": c343._base_operator(),
            "block6": "block 6 accepted"}


def base_c357() -> dict:
    return {"record_data": c357._base_record("coordinated-new-genesis")}


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
