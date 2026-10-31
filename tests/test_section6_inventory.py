import copy
import importlib.util
import json
from pathlib import Path
import unittest

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location("section6",ROOT/"tools/check-section6-inventory.py")
module=importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class InventoryTests(unittest.TestCase):
    def setUp(self):
        base=ROOT/"spec/audit"
        self.source=(base/"passes.md").read_text()
        self.inventory=json.loads((base/"section6-gap-inventory.json").read_text())
        self.matrix=json.loads((base/"final-gap-matrix.json").read_text())
        self.ledger=json.loads((base/"hardfork-51-closeout.json").read_text())
    def check(self):
        return module.validate(self.source,self.inventory,self.matrix,self.ledger)
    def test_exact_inventory_never_authorizes_release(self):
        result=self.check()
        self.assertEqual(result["section6_rows"],93)
        self.assertEqual(result["tracker_obligations"],44)
        self.assertFalse(result["release_authorized"])
    def test_new_source_row_is_not_silently_skipped(self):
        self.source=self.source.replace("\n## 7.","\n| Additional validity rule | oracle | changes acceptance |\n\n## 7.",1)
        with self.assertRaisesRegex(ValueError,"coverage"): self.check()
    def test_removed_source_row_is_detected(self):
        self.inventory["rows"].pop()
        with self.assertRaisesRegex(ValueError,"coverage"): self.check()
    def test_changed_source_semantics_is_detected(self):
        self.source=self.source.replace('Malformed("expr_instance")','Malformed("altered_instance")',1)
        with self.assertRaisesRegex(ValueError,"digest"): self.check()
    def test_coverage_cannot_be_promoted_to_verified_safety(self):
        self.inventory["rows"][0]["classification"]="IMPLEMENTED / VERIFIED"
        with self.assertRaisesRegex(ValueError,"verified safety"): self.check()
    def test_independent_signoff_cannot_be_self_awarded(self):
        self.inventory["independent_signoff"]=True
        with self.assertRaisesRegex(ValueError,"independent"): self.check()
    def test_missing_tracker_item_is_detected(self):
        self.matrix["items"].pop()
        with self.assertRaisesRegex(ValueError,"coverage"): self.check()
    def test_fake_release_deferral_is_refused(self):
        self.matrix["items"][0]["recorded_owner_reason_deferral"]=True
        with self.assertRaisesRegex(ValueError,"deferral"): self.check()

    def test_source_line_drift_is_detected(self):
        self.inventory["rows"][0]["source_line"] += 1
        with self.assertRaisesRegex(ValueError,"source row line"):
            self.check()

    def test_source_evidence_url_must_use_pinned_commit(self):
        self.inventory["rows"][0]["existing_evidence"]["url"] = "https://github.com/rhovision-org/rholo/blob/dev/spec/audit/passes.md"
        with self.assertRaisesRegex(ValueError,"not pinned"):
            self.check()

    def test_owner_reason_deferral_can_be_reconciled_without_release(self):
        item=next(x for x in self.ledger["items"] if x["status"]=="open"
                  and x["id"]!="Audit:section6-categoryA")
        item.update(status="deferred",decision_owner="@jimscarver",
                    reason="Retain the current rule pending an accepted governance proposal.")
        row=next(x for x in self.matrix["items"] if x["id"]==item["id"])
        row.update(classification="EXPLICITLY DEFERRED",recorded_owner_reason_deferral=True,
                   decision_owner=item["decision_owner"],deferral_reason=item["reason"])
        self.assertFalse(self.check()["release_authorized"])
        row["deferral_reason"]="A different decision cannot replace the recorded reason."
        with self.assertRaisesRegex(ValueError,"owner and reason"):
            self.check()

    def test_invalid_ledger_deferral_is_not_accepted_by_inventory(self):
        item=next(x for x in self.ledger["items"] if x["status"]=="open"
                  and x["id"]!="Audit:section6-categoryA")
        item.update(status="deferred",decision_owner="maintainers",reason="Retain the current rule pending review.")
        with self.assertRaisesRegex(ValueError,"GitHub user"):
            self.check()

    def test_ledger_deferral_cannot_be_hidden_by_matrix(self):
        item=next(x for x in self.ledger["items"] if x["status"]=="open"
                  and x["id"]!="Audit:section6-categoryA")
        item.update(status="deferred",decision_owner="@jimscarver",
                    reason="Retain the current rule pending an accepted governance proposal.")
        with self.assertRaisesRegex(ValueError,"deferral mismatch"):
            self.check()

if __name__=="__main__": unittest.main()
