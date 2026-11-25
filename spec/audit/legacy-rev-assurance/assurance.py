"""Synthetic reference model. Not an on-chain implementation or evidence oracle."""
from dataclasses import dataclass, asdict
import hashlib
import json

def require(condition, message):
    if not condition:
        raise ValueError(message)

def nat(x):
    require(type(x) is int and x >= 0, "nonnegative integer required")
    return x

def digest(x):
    return hashlib.sha256(json.dumps(x, sort_keys=True, separators=(",", ":"),
                                    ensure_ascii=True, allow_nan=False).encode()).hexdigest()

@dataclass(frozen=True)
class Entitlement:
    source_id: str  # Canonical legacy position at the approved snapshot, not claim ID.
    owner: str
    amount: int
    evidence: str
    verified: bool

@dataclass(frozen=True)
class Policy:
    snapshot: str
    threshold: int
    duration: int
    reserve: int
    authority_ref: str  # External decision reference; NOT authenticated by this model.

def reconcile(rows, policy, approved_digest):
    require(digest(asdict(policy)) == approved_digest, "policy digest mismatch")
    require(bool(policy.authority_ref and policy.snapshot), "missing policy binding")
    nat(policy.threshold); nat(policy.duration); nat(policy.reserve)
    require(policy.duration > 0, "restricted schedule requires positive duration")
    seen, owners, pending = set(), {}, []
    for r in sorted(rows, key=lambda r:r.source_id):
        require(bool(r.source_id and r.owner and r.evidence), "missing evidence reference")
        require(type(r.verified) is bool, "invalid evidence status")
        nat(r.amount)
        require(r.source_id not in seen, "duplicate source entitlement")
        seen.add(r.source_id)
        if r.verified:
            owners[r.owner] = owners.get(r.owner, 0) + r.amount
        else:
            pending.append(asdict(r))
    accounts = {o: {"liquid": min(a, policy.threshold),
                    "vesting": max(0, a-policy.threshold)}
                for o,a in sorted(owners.items())}
    accepted = sum(owners.values())
    require(sum(v["liquid"]+v["vesting"] for v in accounts.values()) == accepted,
            "conservation failure")
    payload = {"policy": asdict(policy), "policy_digest": approved_digest,
               "inputs": [asdict(r) for r in sorted(rows,key=lambda r:r.source_id)],
               "accounts": accounts, "accepted": accepted,
               "unresolved": pending, "reserve": policy.reserve,
               "genesis_supply": accepted+policy.reserve}
    return {"payload": payload, "sha256": digest(payload)}

def verify_manifest(manifest, rows, policy, approved_digest):
    require(manifest == reconcile(rows, policy, approved_digest), "manifest mismatch")
    return True

def vested(amount, duration, height):
    nat(amount); nat(duration); nat(height)
    require(duration > 0, "duration must be positive")
    return amount * min(height, duration) // duration

def release(amount, duration, height, released):
    nat(released)
    v = vested(amount, duration, height)
    require(released <= v, "release state exceeds vested ceiling")
    return v-released

class Claims:
    """Sequential model of an atomic reserve debit + entitlement consumption.
    Real authorization, evidence adjudication and atomic chain execution are owed."""
    def __init__(self, manifest, rows, policy, approved_digest):
        verify_manifest(manifest, rows, policy, approved_digest)
        self.remaining = policy.reserve
        self.pending = {r.source_id:r for r in rows if not r.verified}
        self.paid = {}
        self.reserve = policy.reserve
        self.manifest = manifest["sha256"]

    def settle(self, source_id, owner, amount, decision, expected_decision):
        nat(amount)
        require(source_id in self.pending and source_id not in self.paid,
                "unknown or consumed entitlement")
        r = self.pending[source_id]
        require((owner, amount)==(r.owner,r.amount), "claim mismatch")
        expected = digest({"manifest":self.manifest,"source_id":source_id,
                           "owner":owner,"amount":amount,"decision":"approve"})
        require(decision == expected == expected_decision, "decision binding mismatch")
        require(amount <= self.remaining, "reserve exhausted")
        self.remaining -= amount
        self.paid[source_id] = amount
        require(self.remaining+sum(self.paid.values())==self.reserve, "reserve conservation")
        return amount
