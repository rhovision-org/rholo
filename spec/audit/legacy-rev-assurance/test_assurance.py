import copy
import itertools
import json
import unittest
from dataclasses import asdict, replace
from assurance import *

P=Policy("synthetic-network/finalized-height-100",100,12,60,"SYNTHETIC-ONLY")
D=digest(asdict(P))
R=[Entitlement("position-1","A",80,"fixture-1",True),
   Entitlement("position-2","A",70,"fixture-2",True),
   Entitlement("position-3","B",40,"fixture-3",True),
   Entitlement("position-4","C",50,"fixture-4",False),
   Entitlement("position-5","D",20,"fixture-5",False)]
M=reconcile(R,P,D)
class Witness(unittest.TestCase):
    def test_conservation(self):
        self.assertEqual(M["payload"]["accounts"],{"A":{"liquid":100,"vesting":50},"B":{"liquid":40,"vesting":0}})
        self.assertEqual(M["payload"]["genesis_supply"],250)
        self.assertEqual(sum(r["amount"] for r in M["payload"]["unresolved"]),70)
    def test_permutations(self):
        for rows in itertools.permutations(R):
            self.assertEqual(reconcile(rows,P,D),M)
    def test_duplicate_source(self):
        with self.assertRaises(ValueError): reconcile(R+[replace(R[0],owner="other")],P,D)
    def test_policy_tamper(self):
        with self.assertRaises(ValueError): reconcile(R,replace(P,threshold=150),D)
    def test_manifest_tamper(self):
        m=copy.deepcopy(M);m["payload"]["accounts"]["A"]["liquid"]+=1
        m["sha256"]=digest(m["payload"]) # adversary can recompute a hash
        with self.assertRaises(ValueError): verify_manifest(m,R,P,D)
    def test_amounts(self):
        for a in (-1,1.5,True):
            with self.assertRaises(ValueError): reconcile([replace(R[0],amount=a)],P,D)
    def test_schedule_exhaustive(self):
        for a in range(65):
            for t in range(1,33):
                last=0
                for h in range(t+3):
                    v=vested(a,t,h); delta=release(a,t,h,last)
                    self.assertTrue(0<=last<=v<=a)
                    self.assertEqual(last+delta,v)
                    self.assertEqual(release(a,t,h,v),0)
                    last=v
                self.assertEqual(last,a)
    def test_regressive_height(self):
        with self.assertRaises(ValueError): release(100,10,1,20)
    def test_claims(self):
        c=Claims(M,R,P,D)
        def grant(s,o,a):
            return digest({"manifest":M["sha256"],"source_id":s,"owner":o,"amount":a,"decision":"approve"})
        d=grant("position-4","C",50)
        with self.assertRaises(ValueError): c.settle("position-4","C",50,"bad",d)
        with self.assertRaises(ValueError): c.settle("position-4","other",50,d,d)
        self.assertEqual(c.settle("position-4","C",50,d,d),50)
        with self.assertRaises(ValueError): c.settle("position-4","C",50,d,d)
        e=grant("position-5","D",20)
        with self.assertRaises(ValueError): c.settle("position-5","D",20,e,e)
        with self.assertRaises(ValueError): c.settle("position-1","A",80,d,d)
        self.assertEqual(c.remaining,10)
        self.assertEqual(c.remaining+sum(c.paid.values()),60)

if __name__=="__main__":
    suite=unittest.defaultTestLoader.loadTestsFromTestCase(Witness)
    result=unittest.TextTestRunner(verbosity=2).run(suite)
    if not result.wasSuccessful(): raise SystemExit(1)
    print(json.dumps({"scope":"synthetic reference model only","tests":result.testsRun,
                      "permutations":120,"schedule_states":sum(65*(t+3) for t in range(1,33)),
                      "manifest":M},sort_keys=True,indent=2))
