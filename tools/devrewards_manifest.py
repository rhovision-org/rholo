#!/usr/bin/env python3
"""The canonical developer-rewards manifest (`devrewards-usage-v1`).

Off-chain and deterministic. It reads a finalized anchor and the deploys that preceded it, links
each invocation to the contract installation it consumed, attributes each deploy's burned phlo to
the installer(s) of the contracts it invoked, and emits an ed25519-agnostic manifest: sorted
leaves, Merkle root, attestation digest and per-leaf proofs. Nothing here signs; the attestors sign
the digest, and `accept` on chain verifies those signatures.

Input schema (the experiment's export; heights are integers, byte strings are lowercase hex):

    {"epoch": 1,
     "anchor": {"height": 42, "hash": "<hex64>", "finalized": true},
     "blocks": [{"height": 1, "hash": "<hex64>", "finalized": true,
                 "deploys": [{"deployer": "<hex130>", "burned": 1234,
                              "events": [{"consume_hash": "<hex64>"}]}]}]}

Encoding (shared with `Settlement.rho` and `casper/tests/devrewards_manifest_encoding.rs`):

    leaf  = blake2b256(0x00 || deployer_key(65) || amount_i64_le(8))
    node  = blake2b256(0x01 || left(32) || right(32)), an odd node is promoted
    digest = blake2b256("devrewards-manifest-v1" || epoch_u64_le || anchor_hash(32) || root(32))

Attribution (`devrewards-usage-v1`): a consume hash's **first** appearance installs the contract; a
later deploy whose event consumes that hash is an invocation of it. An invocation deploy's burn is
split equally among the distinct installers it invoked, with the integer remainder given to the
lowest keys in sorted order. A deploy that invoked nothing prior is attributed to its own deployer.

Usage:
  tools/devrewards_manifest.py build  --input IN.json --out MANIFEST.json
  tools/devrewards_manifest.py verify --manifest MANIFEST.json
  tools/devrewards_manifest.py self-test
"""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import sys

SCHEMA = 1
ALGORITHM = "devrewards-usage-v1"
DOMAIN = b"devrewards-manifest-v1"


def h32(data: bytes) -> bytes:
    return hashlib.blake2b(data, digest_size=32).digest()


def leaf(key: bytes, amount: int) -> bytes:
    if not 0 <= amount < (1 << 63):
        raise ValueError(f"amount must be a non-negative i64: {amount}")
    if len(key) != 65:
        raise ValueError(f"a deployer key is 65 bytes: {len(key)}")
    return h32(b"\x00" + key + amount.to_bytes(8, "little"))


def node(left: bytes, right: bytes) -> bytes:
    return h32(b"\x01" + left + right)


def digest(epoch: int, anchor_hash: bytes, root: bytes) -> bytes:
    return h32(DOMAIN + epoch.to_bytes(8, "little") + anchor_hash + root)


def _hex(value: str, length: int | None = None) -> bytes:
    raw = bytes.fromhex(value)
    if length is not None and len(raw) != length:
        raise ValueError(f"expected {length} bytes, got {len(raw)}: {value}")
    return raw


def attribute(data: dict) -> dict[bytes, int]:
    """Run the `devrewards-usage-v1` attribution over the exported deploys."""
    installers: dict[bytes, bytes] = {}
    totals: dict[bytes, int] = {}
    # **Order is a property of the chain, not of the file.** Sorting by height makes the attribution
    # independent of how the exporter serialised the blocks.
    for block in sorted(data["blocks"], key=lambda b: int(b["height"])):
        for deploy in block.get("deploys", []):
            deployer = _hex(deploy["deployer"], 65)
            burned = deploy["burned"]
            if isinstance(burned, bool) or not isinstance(burned, int) or burned < 0:
                raise ValueError("a deploy's burned amount is a non-negative integer")
            hashes = [_hex(event["consume_hash"], 32) for event in deploy.get("events", [])]
            priors = sorted({installers[h] for h in hashes if h in installers})
            if priors:
                base, rem = divmod(burned, len(priors))
                for i, key in enumerate(priors):
                    totals[key] = totals.get(key, 0) + base + (1 if i < rem else 0)
            else:
                totals[deployer] = totals.get(deployer, 0) + burned
            for h in hashes:
                installers.setdefault(h, deployer)
    return totals


def build_tree(keys: list[bytes], amounts: list[int]) -> tuple[bytes, dict[bytes, list]]:
    """The root and per-key proofs. A one-leaf tree's root is its leaf and its proof is empty.

    A node is `H(0x01 || left || right)`; an odd node at a level is promoted unchanged, so it adds
    no step to the proofs of the leaves beneath it.
    """
    if not keys:
        raise ValueError("a manifest needs at least one beneficiary")
    level = [leaf(k, a) for k, a in zip(keys, amounts)]
    positions = list(range(len(keys)))
    proofs: dict[bytes, list] = {k: [] for k in keys}
    while len(level) > 1:
        nxt: list[bytes] = []
        next_positions = [0] * len(keys)
        for i in range(0, len(level), 2):
            new_pos = len(nxt)
            paired = i + 1 < len(level)
            nxt.append(node(level[i], level[i + 1]) if paired else level[i])
            for j, p in enumerate(positions):
                if p == i:
                    if paired:
                        proofs[keys[j]].append({"left": False, "sibling": level[i + 1].hex()})
                    next_positions[j] = new_pos
                elif paired and p == i + 1:
                    proofs[keys[j]].append({"left": True, "sibling": level[i].hex()})
                    next_positions[j] = new_pos
        level, positions = nxt, next_positions
    return level[0], proofs


def build(data: dict) -> dict:
    if not data.get("anchor", {}).get("finalized"):
        raise ValueError("the anchor must be a finalized block")
    anchor_height = int(data["anchor"]["height"])
    anchor_hash = _hex(data["anchor"]["hash"], 32)
    for block in data["blocks"]:
        if int(block["height"]) > anchor_height:
            raise ValueError("a deploy after the anchor cannot enter the manifest")
    totals = attribute(data)
    if not totals:
        raise ValueError("no deploy was attributed")
    keys = sorted(totals)
    amounts = [totals[k] for k in keys]
    root, proofs = build_tree(keys, amounts)
    epoch = int(data["epoch"])
    if epoch < 0:
        raise ValueError("epoch must be non-negative")
    return {
        "schema": SCHEMA,
        "algorithm": ALGORITHM,
        "epoch": epoch,
        "anchor": {"height": anchor_height, "hash": anchor_hash.hex()},
        "total": sum(amounts),
        "leaves": [{"key": k.hex(), "amount": a} for k, a in zip(keys, amounts)],
        "root": root.hex(),
        "digest": digest(epoch, anchor_hash, root).hex(),
        "proofs": {k.hex(): p for k, p in proofs.items()},
    }


def verify(manifest: dict) -> None:
    if manifest.get("schema") != SCHEMA or manifest.get("algorithm") != ALGORITHM:
        raise ValueError("unknown manifest schema or algorithm")
    epoch = int(manifest["epoch"])
    anchor_hash = _hex(manifest["anchor"]["hash"], 32)
    leaves = manifest["leaves"]
    if not leaves:
        raise ValueError("a manifest needs at least one leaf")
    keys = [_hex(leaf["key"], 65) for leaf in leaves]
    if keys != sorted(keys):
        raise ValueError("leaves are not sorted by deployer key")
    if len(set(keys)) != len(keys):
        raise ValueError("a deployer key appears more than once")
    amounts = [int(leaf["amount"]) for leaf in leaves]
    if any(a < 0 for a in amounts):
        raise ValueError("a leaf amount is negative")
    if int(manifest["total"]) != sum(amounts):
        raise ValueError("total does not equal the sum of the leaves")
    root = _hex(manifest["root"], 32)
    built_root, proofs = build_tree(keys, amounts)
    if built_root != root:
        raise ValueError("the root does not match the leaves")
    if manifest.get("digest") != digest(epoch, anchor_hash, root).hex():
        raise ValueError("the digest does not match the canonical preimage")
    given = manifest.get("proofs", {})
    for key, proof in proofs.items():
        if given.get(key.hex()) != proof:
            raise ValueError(f"the proof for {key.hex()} is not the canonical one")


GOLDEN = {
    "keys": ["11" * 65, "22" * 65],
    "amounts": [1000, 2001],
    "epoch": 1,
    "anchor_hash": "aa" * 32,
    "root": None,
    "digest": None,
}


def self_test() -> None:
    keys = [_hex(k, 65) for k in GOLDEN["keys"]]
    amounts = GOLDEN["amounts"]
    root, _ = build_tree(keys, amounts)
    dg = digest(GOLDEN["epoch"], _hex(GOLDEN["anchor_hash"], 32), root)
    fixture = pathlib.Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "devrewards_manifest_golden.json"
    golden = json.loads(fixture.read_text())
    assert leaf(keys[0], amounts[0]).hex() == golden["leaf"], "leaf vector drifted"
    assert root.hex() == golden["root"], f"root drifted: {root.hex()}"
    assert dg.hex() == golden["digest"], f"digest drifted: {dg.hex()}"

    # A deploy that installs (first consume hash) is attributed to itself; a deploy that invokes two
    # installations splits its burn between them, with the remainder to the lowest key; its own new
    # installation is not an invocation of itself.
    a, b, c = "aa" * 65, "bb" * 65, "cc" * 65
    data = {
        "epoch": 7,
        "anchor": {"height": 3, "hash": "dd" * 32, "finalized": True},
        "blocks": [
            {"height": 1, "finalized": True,
             "deploys": [{"deployer": a, "burned": 10, "events": [{"consume_hash": "01" * 32}]}]},
            {"height": 2, "finalized": True,
             "deploys": [{"deployer": b, "burned": 20, "events": [{"consume_hash": "02" * 32}]}]},
            {"height": 3, "finalized": True,
             "deploys": [{"deployer": c, "burned": 11,
                          "events": [{"consume_hash": "01" * 32}, {"consume_hash": "02" * 32}]}]},
        ],
    }
    totals = attribute(data)
    assert totals == {_hex(a, 65): 16, _hex(b, 65): 25}, totals
    manifest = build(data)
    verify(manifest)
    bad = json.loads(json.dumps(manifest))
    bad["leaves"][0]["amount"] += 1
    try:
        verify(bad)
    except ValueError:
        pass
    else:
        raise AssertionError("a tampered leaf must be refused")
    unfinalized = json.loads(json.dumps(data))
    unfinalized["anchor"]["finalized"] = False
    try:
        build(unfinalized)
    except ValueError:
        pass
    else:
        raise AssertionError("an unfinalized anchor must be refused")
    print("devrewards_manifest self-test: ok")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    p_build = sub.add_parser("build")
    p_build.add_argument("--input", required=True)
    p_build.add_argument("--out", required=True)
    p_verify = sub.add_parser("verify")
    p_verify.add_argument("--manifest", required=True)
    sub.add_parser("self-test")
    args = parser.parse_args(argv)
    try:
        if args.command == "build":
            manifest = build(json.loads(pathlib.Path(args.input).read_text()))
            pathlib.Path(args.out).write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        elif args.command == "verify":
            verify(json.loads(pathlib.Path(args.manifest).read_text()))
            print("devrewards manifest: verified")
        else:
            self_test()
    except (ValueError, KeyError) as e:
        print(f"devrewards_manifest: {e}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
