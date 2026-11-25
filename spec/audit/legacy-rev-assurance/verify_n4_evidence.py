"""Offline verifier for a proposed Phase V evidence format.
Does not connect to nodes, execute transactions or authenticate observations.
"""
import re

class InvalidEvidence(ValueError):
    pass

def need(condition, reason):
    if not condition:
        raise InvalidEvidence(reason)

def integer(v):
    need(type(v) is int and v >= 0, "invalid atomic amount or height")
    return v

def block(b):
    need(type(b) is dict and set(b)=={"height","hash"}, "invalid block fields")
    integer(b["height"])
    need(type(b["hash"]) is str and re.fullmatch("[0-9a-f]{64}",b["hash"]), "invalid hash")

def verify(report):
    need(report["schema"]=="legacy-rev-n4-evidence/1", "wrong schema")
    nodes=report["nodes"]
    need(type(nodes) is list and len(nodes)==4 and all(type(n) is str and n for n in nodes)
         and len(set(nodes))==4, "four distinct validators required")
    claims=report["claims"]
    need(claims=={"A":{"owner":"carol","amount":20},"B":{"owner":"carol","amount":70},
                  "C":{"owner":"dave","amount":50},"D":{"owner":"dave","amount":5}},
         "unexpected frozen synthetic claim matrix")
    reserve=integer(report["initial_reserve"])
    need(reserve==100, "wrong synthetic reserve")
    initial=report["initial_balances"]
    need(set(initial)=={"carol","dave"}, "unexpected beneficiary")
    for a in initial.values(): integer(a)
    phases=report["phases"]
    names=["initial","invalid-governance","duplicate","contention","healed","restart","replay"]
    need([p["name"] for p in phases]==names,"missing, duplicate or reordered phase")
    prior=None
    prior_paid=set()
    winner=None
    for phase in phases:
        b=phase["block"];block(b)
        if prior is not None:
            need(b["height"]>prior["height"],"stale phase anchor")
            need(b["hash"]!=prior["hash"],"reused anchor")
        prior=b
        rows=phase["reads"]
        need(len(rows)==4 and {r["node"] for r in rows}==set(nodes),
             "incomplete or duplicate validator census")
        name=phase["name"]
        for r in rows:
            need(r["block"]==b and r["finalized"] is True,"unfinalized or mismatched read")
            need(type(r["paid"]) is list and len(set(r["paid"]))==len(r["paid"]),"duplicate source")
            need(set(r["paid"])<=set(claims),"unknown source")
            paid=set(r["paid"])
            need(prior_paid<=paid,"consumed state lost")
            need("D" not in paid,"invalid governance paid")
            if name in ("initial","invalid-governance"): need(not paid,"premature payment")
            elif name=="duplicate": need(paid=={"A"},"duplicate gate")
            else:
                need(paid in ({"A","B"},{"A","C"}),"contention gate")
                if winner is None: winner=paid-{"A"}
                need(paid=={"A"}|winner,"settlement winner changed")
            expected=dict(initial)
            for c in paid:
                expected[claims[c]["owner"]]+=claims[c]["amount"]
            need(set(r["balances"])==set(expected),"unexpected balance owner")
            for amount in r["balances"].values(): integer(amount)
            need(r["balances"]==expected,"beneficiary accounting mismatch")
            need(integer(r["remaining"])==reserve-sum(claims[c]["amount"] for c in paid),
                 "reserve accounting mismatch")
        first=rows[0]
        need(all(set(r["paid"])==set(first["paid"]) and r["balances"]==first["balances"]
                 and r["remaining"]==first["remaining"] for r in rows),"node divergence")
        prior_paid=set(first["paid"])
    # Readbacks alone do not prove actions happened. Distinguish structural
    # accounting acceptance from provenance and fault-execution validation.
    return "ACCOUNTING_AND_ANCHOR_SHAPE_ACCEPTED"
