#!/usr/bin/env python3
"""Drive the DevRewards live flow on the namespaced G655 devnet.

The offline witness is `devrewards-experiment.py`; this script is what it calls for the on-chain
steps: accept a manifest the Rust observer and the Python builder agree on, refuse a claim while the
treasury is inactive, activate with the install-time attestors, pay a claim exactly once, and show
the negative arms. It reads the manifest the experiment wrote and prints one JSON line per step.

Usage:
  python3 tools/devrewards_live.py --manifest /tmp/g655-manifest.json --treasury <addr> \
      --anchor-height 51 --anchor-hash <hex> [--v0-priv <hex>] [--v1-priv <hex>] step...
  steps: accept inactive activate claim replay conflict overclaim balances all

This is a **local-devnet** tool; the keys below are the devnet's throwaway validator keys.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import subprocess
import sys
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent
GATE = ROOT / "examples" / "devrewards" / "gate"
NODE = "g655-bootstrap"

V0_PUB = "04f700a417754b775d95421973bdbdadb2d23c8a5af46f1829b1431f5c136e549e8a0d61aa0c793f1a614f8e437711c7758473c6ceb0859ac7e9e07911ca66b5c4"
V0_PRIV = "a68a6e6cca30f81bd24a719f3145d20e8424bd7b396309b0708a16c7d8000b76"
V1_PUB = "04dbe32c2062240a4ba0bcad01d7edd98c78b51c77765d5e1e5e9fa3743d2f12a1f82f42cd7dc4f41445979117d790f23e9b3d08d0aa06d527c236172043e747fc"
V1_PRIV = "b8a48b02757c0cfc9325498a93c3b28582e3967072f8fab0cdc8bd04d0d401ee"
TARGET = "111121VP3YyBCToBxa5ikc2nSgh3XK9P9F9bc3LmCZpZPpwnG3Subk"


def sign(priv_hex: str, digest_hex: str) -> str:
    from ecdsa import SigningKey, SECP256k1, util

    key = SigningKey.from_string(bytes.fromhex(priv_hex), curve=SECP256k1)
    signature = key.sign_digest(bytes.fromhex(digest_hex), sigencode=util.sigencode_der_canonize)
    return signature.hex()


def write_term(name: str, term: str) -> str:
    GATE.mkdir(parents=True, exist_ok=True)
    path = GATE / f"live_{name}.rho"
    path.write_text(term)
    return f"devrewards/gate/live_{name}.rho"


def docker(*args: str, timeout: int = 120) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["docker", "exec", NODE, "rnode", "--grpc-host", "localhost", *args],
        capture_output=True,
        text=True,
        timeout=timeout,
    )


def deploy(term: str, private_key: str = V0_PRIV) -> tuple[bool, int | None, str]:
    result = docker(
        "deploy",
        "--phlo-limit",
        "1000000",
        "--phlo-price",
        "1",
        "--private-key",
        private_key,
        "--shard-id",
        "/root",
        f"/contracts/{term}",
    )
    match = re.search(r"DeployId is: (\S+)", result.stdout)
    if not match:
        return False, None, (result.stdout + result.stderr)[-400:]
    deploy_id = match.group(1)
    for _ in range(45):
        status = docker("deploy-status", "--deploy-signature", deploy_id).stdout
        if "processedWithSuccess" in status:
            height = re.search(r'"blockNumber": (\d+)', status)
            return True, int(height.group(1)) if height else None, "success"
        if "processedWithError" in status:
            error = re.search(r'"deployError": "([^"]*)"', status)
            return False, None, error.group(1) if error else status[-400:]
        time.sleep(1)
    return False, None, "deploy status timed out"


def query(channel: str, timeout: int = 12) -> str:
    env = {
        **dict(__import__("os").environ),
        "DEVNET_PREFIX": "g655",
        "DEVNET_NETWORK": "g655",
        "DEVNET_GRPC_BASE": "41402",
        "DEVNET_HTTP_BASE": "41403",
        "DEVNET_ADMIN_BASE": "41405",
        "RNODE_IMAGE": "rnode:devrewards",
    }
    result = subprocess.run(
        ["bash", str(ROOT / "tools" / "devnet.sh"), "query", channel],
        capture_output=True,
        text=True,
        env=env,
        timeout=timeout,
    )
    return result.stdout


def strings(text: str) -> list[str]:
    return re.findall(r'GString\("([^"]*)"\)', text)


def bools(text: str) -> list[str]:
    return re.findall(r"GBool\((true|false)\)", text)


def ints(text: str) -> list[str]:
    return re.findall(r"GInt\((\d+)\)", text)


def attestations(digest_hex: str, v0_priv: str, v1_priv: str) -> str:
    return (
        f'("{V0_PUB}".hexToBytes(), "{sign(v0_priv, digest_hex)}".hexToBytes()), '
        f'("{V1_PUB}".hexToBytes(), "{sign(v1_priv, digest_hex)}".hexToBytes())'
    )


def proof_literal(proof: list[dict]) -> str:
    if not proof:
        return "[]"
    items = ",".join(
        f'({"true" if step["left"] else "false"}, "{step["sibling"]}".hexToBytes())' for step in proof
    )
    return f"[{items}]"


def accept_term(manifest: dict, anchor_height: int, anchor_hash: str, v0_priv: str, v1_priv: str) -> str:
    return f"""new ret in {{
  for (s <<- @"devrewards/settlement") {{
    s!("accept", {manifest['epoch']}, {anchor_height}, "{anchor_hash}".hexToBytes(), "{manifest['root']}".hexToBytes(), {manifest['total']}, [{attestations(manifest['digest'], v0_priv, v1_priv)}], *ret) |
    for (@r <- ret) {{ @"gate/live-accept"!(r) }}
  }}
}}"""


def claim_term(manifest: dict, key_hex: str, amount: int, proof: list[dict], out: str) -> str:
    return f"""new ret in {{
  for (s <<- @"devrewards/settlement") {{
    s!("claim", {manifest['epoch']}, "{key_hex}".hexToBytes(), {amount}, {proof_literal(proof)}, *ret) |
    for (@r <- ret) {{ @"{out}"!(r) }}
  }}
}}"""


def activate_term(treasury: str, v0_priv: str, v1_priv: str) -> str:
    digest = __import__("hashlib").blake2b(
        b"devrewards-activate-v1" + treasury.encode(), digest_size=32
    ).hexdigest()
    return f"""new ret in {{
  for (t <<- @"devrewards/treasury") {{
    t!("activate", [{attestations(digest, v0_priv, v1_priv)}], *ret) |
    for (@r <- ret) {{ @"gate/live-activate"!(r) }}
  }}
}}"""


def balance_term(address: str) -> str:
    return f"""new rv(`rho:rchain:revVault`), vaultCh, b in {{
  rv!("findOrCreate", "{address}", *vaultCh) |
  for (@(true, *h) <- vaultCh) {{
    h!("balance", *b) |
    for (@bal <- b) {{ @"gate/live-balance"!(bal) }}
  }}
}}"""


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--treasury", required=True)
    parser.add_argument("--anchor-height", type=int, required=True)
    parser.add_argument("--anchor-hash", required=True)
    parser.add_argument("--v0-priv", default=V0_PRIV)
    parser.add_argument("--v1-priv", default=V1_PRIV)
    parser.add_argument("--key", default=V0_PUB, help="the claimed deployer key")
    parser.add_argument("--out", default="/tmp/g655-live-results.json")
    parser.add_argument("steps", nargs="+")
    args = parser.parse_args(argv)
    manifest = json.loads(pathlib.Path(args.manifest).read_text())
    key = args.key
    leaf = next(item for item in manifest["leaves"] if item["key"] == key)
    amount = leaf["amount"]
    proof = manifest["proofs"][key]
    results: dict[str, dict] = {}

    def record(name: str, ok: bool, detail: str) -> None:
        results[name] = {"ok": ok, "detail": detail}
        print(json.dumps({"step": name, "ok": ok, "detail": detail}))

    for step in args.steps:
        if step == "accept":
            term = write_term("accept", accept_term(manifest, args.anchor_height, args.anchor_hash, args.v0_priv, args.v1_priv))
            ok, height, detail = deploy(term, args.v0_priv)
            reply = strings(query("gate/live-accept")) if ok else []
            record("accept", ok and "accepted" in " ".join(reply), f"{detail} reply={reply}")
        elif step == "inactive":
            term = write_term("claim_inactive", claim_term(manifest, key, amount, proof, "gate/live-claim-inactive"))
            ok, height, detail = deploy(term, args.v0_priv)
            reply = query("gate/live-claim-inactive") if ok else ""
            record("inactive", ok and "treasury inactive" in reply, f"{detail} reply={reply[-200:]}")
        elif step == "activate":
            term = write_term("activate", activate_term(args.treasury, args.v0_priv, args.v1_priv))
            ok, height, detail = deploy(term, args.v0_priv)
            reply = query("gate/live-activate") if ok else ""
            record("activate", ok and "active" in reply, f"{detail} reply={reply[-200:]}")
        elif step == "claim":
            term = write_term("claim", claim_term(manifest, key, amount, proof, "gate/live-claim"))
            ok, height, detail = deploy(term, args.v0_priv)
            reply = query("gate/live-claim") if ok else ""
            record("claim", ok and "paid" in reply, f"amount={amount} {detail} reply={reply[-200:]}")
        elif step == "replay":
            term = write_term("claim_replay", claim_term(manifest, key, amount, proof, "gate/live-claim-replay"))
            ok, height, detail = deploy(term, args.v0_priv)
            reply = query("gate/live-claim-replay") if ok else ""
            record("replay", ok and "already paid" in reply, f"{detail} reply={reply[-200:]}")
        elif step == "overclaim":
            term = write_term("claim_over", claim_term(manifest, key, amount + 1, proof, "gate/live-claim-over"))
            ok, height, detail = deploy(term, args.v0_priv)
            reply = query("gate/live-claim-over") if ok else ""
            record("overclaim", ok and "invalid proof" in reply, f"{detail} reply={reply[-200:]}")
        elif step == "conflict":
            other_root = __import__("hashlib").blake2b(
                b"\x00" + bytes.fromhex(key) + (amount + 1).to_bytes(8, "little"), digest_size=32
            ).digest()
            anchor = bytes.fromhex(args.anchor_hash)
            digest = __import__("hashlib").blake2b(
                b"devrewards-manifest-v1" + manifest["epoch"].to_bytes(8, "little") + anchor + other_root,
                digest_size=32,
            ).hexdigest()
            term = f"""new ret in {{
  for (s <<- @"devrewards/settlement") {{
    s!("accept", {manifest['epoch']}, {args.anchor_height}, "{args.anchor_hash}".hexToBytes(), "{other_root.hex()}".hexToBytes(), {amount + 1}, [{attestations(digest, args.v0_priv, args.v1_priv)}], *ret) |
    for (@r <- ret) {{ @"gate/live-conflict"!(r) }}
  }}
}}"""
            term = write_term("conflict", term)
            ok, height, detail = deploy(term, args.v0_priv)
            reply = query("gate/live-conflict") if ok else ""
            record("conflict", ok and "conflicting manifest" in reply, f"{detail} reply={reply[-200:]}")
        elif step == "balances":
            term = write_term("balance", balance_term(TARGET))
            ok, height, detail = deploy(term, args.v0_priv)
            reply = query("gate/live-balance") if ok else ""
            record("balances", ok, f"target={TARGET} reply={reply[-120:]}")
        elif step == "all":
            # Ordered: accept, inactive, activate, claim, replay, overclaim, conflict, balances.
            sys.argv = [sys.argv[0], *argv[: argv.index("all")], "accept", "inactive", "activate",
                        "claim", "replay", "overclaim", "conflict", "balances"]
            return main(sys.argv[1:])
        else:
            print(f"unknown step {step}", file=sys.stderr)
            return 2
    pathlib.Path(args.out).write_text(json.dumps(results, indent=2, sort_keys=True) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
