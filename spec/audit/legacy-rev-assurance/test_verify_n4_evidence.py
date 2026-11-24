import copy
import unittest
from verify_n4_evidence import verify, InvalidEvidence

def fixture():
    nodes=["v0","v1","v2","v3"]
    claims={"A":{"owner":"carol","amount":20},"B":{"owner":"carol","amount":70},
            "C":{"owner":"dave","amount":50},"D":{"owner":"dave","amount":5}}
    phases=[]
    for h,name,paid in [(1,"initial",[]),(2,"invalid-governance",[]),(3,"duplicate",["A"]),
                        (4,"contention",["A","B"]),(5,"healed",["A","B"]),
                        (6,"restart",["A","B"]),(7,"replay",["A","B"])]:
        b={"height":h,"hash":f"{h:064x}"}
        amounts={"carol":1000,"dave":1000}
        for c in paid: amounts[claims[c]["owner"]]+=claims[c]["amount"]
        rows=[{"node":n,"block":dict(b),"finalized":True,"paid":list(paid),
               "balances":dict(amounts),"remaining":100-sum(claims[c]["amount"] for c in paid)} for n in nodes]
        phases.append({"name":name,"block":b,"reads":rows})
    return {"schema":"legacy-rev-n4-evidence/1","nodes":nodes,"claims":claims,
            "initial_reserve":100,"initial_balances":{"carol":1000,"dave":1000},"phases":phases}

class Controls(unittest.TestCase):
    def test_positive_shape(self):
        self.assertEqual(verify(fixture()),"ACCOUNTING_AND_ANCHOR_SHAPE_ACCEPTED")
    def test_negative_controls(self):
        mutations=[
          ("missing node",lambda x:x["phases"][3]["reads"].pop()),
          ("duplicate node",lambda x:x["phases"][3]["reads"][1].update(node="v0")),
          ("not finalized",lambda x:x["phases"][3]["reads"][0].update(finalized=False)),
          ("different hash",lambda x:x["phases"][3]["reads"][0]["block"].update(hash="0"*64)),
          ("stale height",lambda x:x["phases"][4]["block"].update(height=4)),
          ("early payout",lambda x:x["phases"][1]["reads"][0].update(paid=["A"])),
          ("double source",lambda x:x["phases"][2]["reads"][0].update(paid=["A","A"])),
          ("overspend",lambda x:x["phases"][3]["reads"][0].update(paid=["A","B","C"])),
          ("dissent paid",lambda x:x["phases"][3]["reads"][0].update(paid=["A","B","D"])),
          ("wrong balance",lambda x:x["phases"][3]["reads"][0]["balances"].update(carol=1100)),
          ("reserve divergence",lambda x:x["phases"][3]["reads"][1].update(remaining=11)),
          ("lost consumption",lambda x:x["phases"][5]["reads"][0].update(paid=["A"])),
          ("missing replay",lambda x:x["phases"].pop()),
          ("bool amount",lambda x:x["phases"][0]["reads"][0].update(remaining=True)),
        ]
        for name,change in mutations:
            with self.subTest(name=name):
                x=fixture();change(x)
                with self.assertRaises(InvalidEvidence): verify(x)
                print("REJECT",name)

if __name__=="__main__": unittest.main(verbosity=2)
