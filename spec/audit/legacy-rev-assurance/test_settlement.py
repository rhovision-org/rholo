"""Synthetic keys and funds only. Execute in CI on isolated SQLite databases."""
import concurrent.futures
from dataclasses import asdict
import json
import multiprocessing
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import unittest

from assurance import Entitlement, Policy, digest, reconcile
from settlement import (Refused, authority_id, canonical, configuration, initialize,
                        signed_bytes, sha, snapshot, settle, audit, MAX)


def crash_worker(path, config, pin, wire, stage):
    settle(path, config, pin, wire, 10, lambda s: os._exit(73) if s == stage else None)


class SettlementTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.keys_dir = tempfile.TemporaryDirectory()
        cls.keys = []
        for i in range(4):
            key = Path(cls.keys_dir.name)/f"k{i}.pem"
            subprocess.run(["openssl","genpkey","-algorithm","ED25519","-out",str(key)],check=True)
            pub = subprocess.check_output(["openssl","pkey","-in",str(key),"-pubout","-outform","DER"]).hex()
            cls.keys.append((key,pub))

    @classmethod
    def tearDownClass(cls):
        cls.keys_dir.cleanup()

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.path = str(Path(self.tmp.name)/"ledger.db")
        self.rows = [Entitlement("s1","alice",70,"e1",False),
                     Entitlement("s2","bob",50,"e2",False),
                     Entitlement("s3","alice",20,"e3",False),
                     Entitlement("verified","alice",7,"e4",True)]
        self.policy = Policy("snapshot",10,5,100,"synthetic-authority")
        self.pdigest = digest(asdict(self.policy))
        self.manifest = reconcile(self.rows,self.policy,self.pdigest)
        self.config = configuration(self.manifest,self.rows,self.policy,self.pdigest,
                                    "isolated-chain",[p for _,p in self.keys[:3]],2)
        self.pin = sha(canonical(self.config))
        initialize(self.path,self.config,self.pin)

    def decision(self, source="s1", **changes):
        return dict(version=1,network=self.config["network"],manifest=self.config["manifest"],
                    policy=self.config["policy"],authority=authority_id(self.config),
                    decision_id="decision-"+source,source=source,not_before=5,expires=15,
                    action="approve",**self.config["pending"][source]) | changes

    def wire(self, d=None, signers=(0,1)):
        d = self.decision() if d is None else d
        msg = signed_bytes(d)
        signatures = []
        for i in signers:
            key,pub=self.keys[i]
            with tempfile.TemporaryDirectory() as tmp:
                file=Path(tmp)/"msg"
                file.write_bytes(msg)
                sig=subprocess.check_output(["openssl","pkeyutl","-sign","-inkey",str(key),
                                             "-rawin","-in",str(file)]).hex()
            signatures.append({"key":pub,"signature":sig})
        return canonical({"decision":d,"signatures":signatures})

    def apply(self, raw, height=10):
        return settle(self.path,self.config,self.pin,raw,height)

    def state(self):
        return snapshot(self.path,self.config,self.pin)

    def reject_unchanged(self, raw, height=10):
        before=self.state()
        with self.assertRaises(Refused):
            self.apply(raw,height)
        self.assertEqual(self.state(),before)

    def test_valid_and_replay(self):
        raw=self.wire()
        self.assertEqual(self.apply(raw),sha(signed_bytes(self.decision())))
        self.assertEqual(self.state()["remaining"],30)
        self.assertEqual(self.state()["balances"],{"alice":70})
        self.reject_unchanged(raw)
        self.reject_unchanged(self.wire(self.decision(decision_id="new-decision")))
        self.reject_unchanged(self.wire(self.decision("s3",decision_id="decision-s1")))
        self.apply(self.wire(self.decision("s3")))
        self.assertEqual(audit(self.path,self.config,self.pin)["balances"],{"alice":90})
        self.reject_unchanged(self.wire(self.decision("s2")))

    def test_authentication_and_domain_controls(self):
        for raw in (self.wire(signers=()),self.wire(signers=(0,)),self.wire(signers=(0,0)),
                    self.wire(signers=(0,3)),self.wire(self.decision(network="other-chain")),
                    self.wire(self.decision(manifest="0"*64)),self.wire(self.decision(policy="1"*64)),
                    self.wire(self.decision(authority="2"*64)),self.wire(self.decision(owner="mallory")),
                    self.wire(self.decision(amount=71)),self.wire(self.decision(evidence="fake")),
                    self.wire(self.decision(source="unknown")),self.wire(self.decision(source="verified"))):
            self.reject_unchanged(raw)
        raw=self.wire()
        altered=json.loads(raw);altered["decision"]["decision_id"]="tampered"
        self.reject_unchanged(canonical(altered))
        altered=json.loads(raw);altered["signatures"][0]["signature"]="00"*64
        self.reject_unchanged(canonical(altered))
        self.reject_unchanged(raw,4);self.reject_unchanged(raw,16)
        self.reject_unchanged(raw+b" ")
        self.reject_unchanged(raw.replace(b'"version":1',b'"version":1,"version":1'))
        self.reject_unchanged(raw.replace(b'"amount":70',b'"amount":true'))
        self.reject_unchanged(raw.replace(b'"amount":70',b'"amount":9223372036854775808'))

    def test_config_binding_and_missing_db(self):
        raw=self.wire();before=self.state()
        modified=json.loads(canonical(self.config));modified["reserve"]=200
        with self.assertRaises(Refused):
            settle(self.path,modified,self.pin,raw,10)
        with self.assertRaises(Refused):
            settle(self.path,modified,sha(canonical(modified)),raw,10)
        with self.assertRaises(Refused):
            configuration(self.manifest,self.rows,self.policy,self.pdigest,
                          "isolated-chain",[self.keys[0][1]]*2,2)
        with self.assertRaises(sqlite3.OperationalError):
            settle(self.path+".missing",self.config,self.pin,raw,10)
        self.assertFalse(Path(self.path+".missing").exists())
        self.assertEqual(self.state(),before)

    def test_concurrent_same_source(self):
        raws=[self.wire(self.decision(decision_id=f"race-{i}")) for i in range(8)]
        def attempt(raw):
            try:
                self.apply(raw);return True
            except Refused:
                return False
        with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
            outcomes=list(pool.map(attempt,raws))
        self.assertEqual(sum(outcomes),1)
        self.assertEqual(len(self.state()["receipts"]),1)
        self.assertEqual(self.state()["remaining"],30)
        audit(self.path,self.config,self.pin)

    def test_concurrent_reserve_contention(self):
        def attempt(raw):
            try:
                self.apply(raw);return True
            except Refused:
                return False
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            results=list(pool.map(attempt,[self.wire(),self.wire(self.decision("s2"))]))
        self.assertEqual(sum(results),1)
        state=audit(self.path,self.config,self.pin)
        self.assertIn(state["remaining"],(30,50))
        self.assertEqual(len(state["receipts"]),1)

    def test_process_death_before_and_after_commit(self):
        raw=self.wire()
        initial=self.state()
        ctx=multiprocessing.get_context("fork")
        for stage in ("after_debit","after_credit","after_receipt"):
            proc=ctx.Process(target=crash_worker,args=(self.path,self.config,self.pin,raw,stage))
            proc.start();proc.join(20)
            if proc.is_alive(): proc.kill();proc.join();self.fail("child hung")
            self.assertEqual(proc.exitcode,73)
            self.assertEqual(audit(self.path,self.config,self.pin),initial)
        proc=ctx.Process(target=crash_worker,args=(self.path,self.config,self.pin,raw,"after_commit"))
        proc.start();proc.join(20)
        if proc.is_alive(): proc.kill();proc.join();self.fail("child hung")
        self.assertEqual(proc.exitcode,73)
        self.assertEqual(audit(self.path,self.config,self.pin)["remaining"],30)
        self.reject_unchanged(raw)

    def test_persisted_signature_tampering_detected(self):
        self.apply(self.wire())
        with sqlite3.connect(self.path) as db:
            raw=db.execute("SELECT envelope FROM receipts").fetchone()[0]
            altered=json.loads(raw)
            altered["signatures"][0]["signature"]="00"*64
            db.execute("UPDATE receipts SET envelope=?",(canonical(altered),))
        with self.assertRaises(Refused):
            audit(self.path,self.config,self.pin)


if __name__=="__main__":
    unittest.main(verbosity=2)
