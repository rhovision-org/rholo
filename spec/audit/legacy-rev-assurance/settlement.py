"""Phase III research witness: signed exact claims + durable local transactions.
Not a RhoGov adapter, on-chain contract, production key store or external transfer.
"""
from contextlib import contextmanager
from pathlib import Path
import hashlib
import json
import re
import sqlite3
import subprocess
import tempfile

from assurance import verify_manifest

DOMAIN = b"legacy-rev/decision/v1\x00"
MAX = (1 << 63) - 1
PUB_PREFIX = bytes.fromhex("302a300506032b6570032100")
FIELDS = {"version", "network", "manifest", "policy", "authority", "decision_id",
          "source", "owner", "amount", "evidence", "not_before", "expires", "action"}


class Refused(ValueError):
    pass


def require(ok, reason):
    if not ok:
        raise Refused(reason)


def uint(v):
    require(type(v) is int and 0 <= v <= MAX, "invalid atomic integer")
    return v


def ident(v):
    require(type(v) is str and re.fullmatch(r"[A-Za-z0-9_.:/-]{1,256}", v) is not None,
            "invalid identifier")
    return v


def hexstr(v, n):
    require(type(v) is str and re.fullmatch(r"[0-9a-f]{%d}" % n, v) is not None,
            "invalid hex")
    return v


def canonical(v):
    return json.dumps(v, sort_keys=True, separators=(",", ":"), ensure_ascii=True,
                      allow_nan=False).encode("ascii")


def sha(v):
    return hashlib.sha256(v).hexdigest()


def unique_pairs(pairs):
    d = {}
    for k, v in pairs:
        require(k not in d, "duplicate JSON member")
        d[k] = v
    return d


def decode(raw):
    require(type(raw) is bytes and len(raw) <= 65536, "invalid envelope size")
    try:
        v = json.loads(raw.decode("ascii"), object_pairs_hook=unique_pairs)
        require(canonical(v) == raw, "noncanonical wire bytes")
    except (UnicodeError, ValueError, TypeError, RecursionError) as e:
        raise Refused("invalid canonical envelope") from e
    require(type(v) is dict and set(v) == {"decision", "signatures"}, "invalid envelope fields")
    return v


def pubkey(v):
    b = bytes.fromhex(hexstr(v, 88))
    require(b.startswith(PUB_PREFIX), "not canonical Ed25519 SPKI")
    return b


def signed_bytes(d):
    require(type(d) is dict and set(d) == FIELDS, "invalid decision fields")
    require(type(d["version"]) is int and d["version"] == 1 and d["action"] == "approve",
            "invalid version/action")
    for f in ("network", "decision_id", "source", "owner", "evidence"):
        ident(d[f])
    for f in ("manifest", "policy", "authority"):
        hexstr(d[f], 64)
    for f in ("amount", "not_before", "expires"):
        uint(d[f])
    require(d["not_before"] <= d["expires"], "invalid validity interval")
    return DOMAIN + canonical(d)


def verify_sig(pk, sig, msg):
    with tempfile.TemporaryDirectory(prefix="revverify-") as tmp:
        p = Path(tmp)
        (p/"key.der").write_bytes(pubkey(pk))
        (p/"sig").write_bytes(bytes.fromhex(hexstr(sig, 128)))
        (p/"msg").write_bytes(msg)
        try:
            proc = subprocess.run(["openssl", "pkeyutl", "-verify", "-pubin",
                                   "-keyform", "DER", "-inkey", str(p/"key.der"),
                                   "-rawin", "-in", str(p/"msg"), "-sigfile", str(p/"sig")],
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10)
        except (OSError, subprocess.TimeoutExpired) as e:
            raise Refused("signature verifier unavailable") from e
        require(proc.returncode == 0, "invalid signature")


def configuration(manifest, rows, policy, approved_digest, network, keys, threshold):
    """Caller must independently authenticate the approved manifest/policy and keys."""
    verify_manifest(manifest, rows, policy, approved_digest)
    require(type(keys) is list and 1 <= len(keys) <= 32, "invalid authority size")
    for k in keys:
        pubkey(k)
    require(len(set(keys)) == len(keys), "duplicate authority key")
    require(type(threshold) is int and 1 <= threshold <= len(keys), "invalid quorum")
    payload = manifest["payload"]
    pending = {}
    for r in payload["unresolved"]:
        source = ident(r["source_id"])
        require(source not in pending, "duplicate source")
        pending[source] = {"owner": ident(r["owner"]), "amount": uint(r["amount"]),
                           "evidence": ident(r["evidence"])}
    return {"network": ident(network), "manifest": hexstr(manifest["sha256"],64),
            "policy": hexstr(approved_digest,64), "reserve": uint(payload["reserve"]),
            "pending": pending, "keys": sorted(keys), "threshold": threshold}


def authority_id(config):
    return sha(canonical({"keys": config["keys"], "threshold": config["threshold"]}))


def authenticate(raw, config, height):
    uint(height)
    wire = decode(raw)
    d, sigs = wire["decision"], wire["signatures"]
    msg = signed_bytes(d)
    require((d["network"],d["manifest"],d["policy"],d["authority"]) ==
            (config["network"],config["manifest"],config["policy"],authority_id(config)),
            "wrong trust domain")
    require(d["not_before"] <= height <= d["expires"], "outside validity interval")
    require(config["pending"].get(d["source"]) ==
            {k:d[k] for k in ("owner","amount","evidence")}, "claim mismatch")
    require(type(sigs) is list and config["threshold"] <= len(sigs) <= len(config["keys"]),
            "insufficient or excessive signatures")
    seen = set()
    for s in sigs:
        require(type(s) is dict and set(s) == {"key","signature"}, "invalid attestation")
        k = s["key"]
        pubkey(k)
        require(k in config["keys"] and k not in seen, "unknown or duplicate signer")
        verify_sig(k,s["signature"],msg)
        seen.add(k)
    return d, sha(msg)


def connect(path, create=False):
    uri = Path(path).resolve().as_uri() + ("?mode=rwc" if create else "?mode=rw")
    db = sqlite3.connect(uri, uri=True, isolation_level=None, timeout=15)
    db.execute("PRAGMA synchronous=FULL")
    db.execute("PRAGMA foreign_keys=ON")
    return db


@contextmanager
def transaction(path, create=False):
    db = connect(path, create)
    try:
        db.execute("BEGIN IMMEDIATE")
        yield db
        db.commit()
    except BaseException:
        db.rollback()
        raise
    finally:
        db.close()


def bind(db, config, expected_config):
    blob = canonical(config)
    require(sha(blob) == expected_config, "external config pin mismatch")
    row = db.execute("SELECT config FROM treasury WHERE singleton=1").fetchone()
    require(row is not None and row[0] == blob, "database config mismatch")


def initialize(path, config, expected_config):
    blob = canonical(config)
    require(sha(blob) == expected_config, "external config pin mismatch")
    with transaction(path, create=True) as db:
        db.execute("CREATE TABLE treasury(singleton INTEGER PRIMARY KEY CHECK(singleton=1), config BLOB NOT NULL, remaining INTEGER NOT NULL CHECK(remaining>=0))")
        db.execute("CREATE TABLE receipts(source TEXT PRIMARY KEY, decision_id TEXT UNIQUE NOT NULL, digest TEXT NOT NULL, owner TEXT NOT NULL, amount INTEGER NOT NULL CHECK(amount>=0), height INTEGER NOT NULL, envelope BLOB NOT NULL)")
        db.execute("CREATE TABLE balances(owner TEXT PRIMARY KEY, amount INTEGER NOT NULL CHECK(amount>=0))")
        db.execute("INSERT INTO treasury VALUES(1,?,?)", (blob,config["reserve"]))


def snapshot_db(db, config):
    remaining = db.execute("SELECT remaining FROM treasury WHERE singleton=1").fetchone()[0]
    receipts = db.execute("SELECT source,decision_id,digest,owner,amount,height,envelope FROM receipts ORDER BY source").fetchall()
    balances = dict(db.execute("SELECT owner,amount FROM balances ORDER BY owner"))
    derived = {}
    for source, decision_id, digest, owner, amount, height, raw in receipts:
        d = decode(raw)["decision"]
        require(sha(signed_bytes(d)) == digest and
                (d["source"],d["decision_id"],d["owner"],d["amount"]) ==
                (source,decision_id,owner,amount), "receipt inconsistency")
        derived[owner] = derived.get(owner,0) + amount
    require(derived == balances and remaining + sum(balances.values()) == config["reserve"],
            "ledger conservation failure")
    return {"remaining": remaining, "balances": balances,
            "receipts": [(s,i,g,o,a,h) for s,i,g,o,a,h,_ in receipts]}


def snapshot(path, config, expected_config):
    with transaction(path) as db:
        bind(db,config,expected_config)
        return snapshot_db(db,config)


def settle(path, config, expected_config, raw, height, fault=lambda _: None):
    # Authentication precedes the write lock. The trust root is frozen and bound
    # again inside the transaction; no external transfer occurs in this model.
    config = json.loads(canonical(config))
    require(sha(canonical(config)) == expected_config, "external config pin mismatch")
    d, digest = authenticate(raw,config,height)
    with transaction(path) as db:
        bind(db,config,expected_config)
        snapshot_db(db,config)
        require(not db.execute("SELECT 1 FROM receipts WHERE source=? OR decision_id=?",
                               (d["source"],d["decision_id"])).fetchone(), "consumed source or decision")
        updated = db.execute("UPDATE treasury SET remaining=remaining-? WHERE singleton=1 AND remaining>=?",
                             (d["amount"],d["amount"]))
        require(updated.rowcount == 1, "reserve exhausted")
        fault("after_debit")
        db.execute("INSERT INTO balances VALUES(?,?) ON CONFLICT(owner) DO UPDATE SET amount=amount+excluded.amount",
                   (d["owner"],d["amount"]))
        fault("after_credit")
        db.execute("INSERT INTO receipts VALUES(?,?,?,?,?,?,?)",
                   (d["source"],d["decision_id"],digest,d["owner"],d["amount"],height,raw))
        fault("after_receipt")
        snapshot_db(db,config)
    fault("after_commit")
    return digest


def audit(path, config, expected_config):
    """Re-verify persisted signature envelopes at their recorded execution heights."""
    with transaction(path) as db:
        bind(db,config,expected_config)
        result = snapshot_db(db,config)
        for raw,height in db.execute("SELECT envelope,height FROM receipts"):
            authenticate(raw,config,height)
        return result
