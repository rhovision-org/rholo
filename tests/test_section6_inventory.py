import copy
import importlib.util
import json
import subprocess
import tempfile
from pathlib import Path
import unittest
from unittest import mock

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
        self.assertEqual(result["section6_rows"],94)
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

    # --- the §6 source is the pinned blob, not the worktree -------------------

    def git_repo(self, document):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = Path(tmp.name)
        target = root / module.SOURCE_PATH
        target.parent.mkdir(parents=True)
        target.write_text(document)
        subprocess.run(["git", "init", "-q", str(root)], capture_output=True, check=False)
        subprocess.run(["git", "-C", str(root), "add", "."], capture_output=True, check=False)
        subprocess.run(["git", "-C", str(root), "-c", "user.name=t",
                        "-c", "user.email=t@t", "commit", "-qm", "base"],
                       capture_output=True, check=False)
        head = subprocess.run(["git", "-C", str(root), "rev-parse", "HEAD"],
                              capture_output=True, text=True, check=False)
        return root, head.stdout.strip()

    def test_git_blob_reads_the_commit_not_the_worktree(self):
        root, rev = self.git_repo(self.source)
        (root / module.SOURCE_PATH).write_text(
            self.source.replace('Malformed("expr_instance")',
                                'Malformed("worktree_only")', 1))
        status, text, reason = module._GATE.git_blob(root, rev, module.SOURCE_PATH)
        self.assertEqual(status, "verified", reason)
        self.assertEqual(text, self.source)
        module.validate(text, self.inventory, self.matrix, self.ledger,
                        repo=ROOT)

    def test_pinned_validation_refuses_a_changed_row_at_the_pin(self):
        doctored = self.source.replace('Malformed("expr_instance")',
                                       'Malformed("pinned_only")', 1)
        with mock.patch.object(
                module._GATE, "git_blob",
                return_value=("verified", doctored, None)):
            with self.assertRaisesRegex(ValueError, "digest"):
                module.validate_pinned(ROOT, self.inventory, self.matrix,
                                       self.ledger)
        self.assertEqual(self.check()["section6_rows"], 94)

    def test_a_missing_pinned_commit_is_unavailable(self):
        root, _ = self.git_repo(self.source)
        status, text, _ = module._GATE.git_blob(root, "0" * 40, module.SOURCE_PATH)
        self.assertEqual(status, "unavailable")
        self.assertIsNone(text)

    def test_a_shallow_checkout_is_unverified(self):
        root, rev = self.git_repo(self.source)
        with tempfile.TemporaryDirectory() as clone:
            proc = subprocess.run(
                ["git", "clone", "-q", "--depth", "1", f"file://{root}", clone],
                capture_output=True, text=True, check=False)
            if proc.returncode != 0:
                self.skipTest("git clone --depth is unavailable")
            status, text, reason = module._GATE.git_blob(
                Path(clone), rev, module.SOURCE_PATH)
        self.assertEqual(status, "unverified-shallow", reason)
        self.assertIsNone(text)

    def test_the_real_inventory_matches_the_pinned_blob(self):
        shallow = subprocess.run(
            ["git", "-C", str(ROOT), "rev-parse", "--is-shallow-repository"],
            capture_output=True, text=True, check=False)
        if shallow.stdout.strip() != "false":
            self.skipTest("a depth-1 checkout cannot prove the pinned blob")
        result = module.validate_pinned(
            ROOT, self.inventory, self.matrix, self.ledger)
        self.assertEqual(result["blob_status"], "verified")
        self.assertEqual(result["section6_rows"], 94)
        self.assertFalse(result["release_authorized"])

    def test_pinned_cli_exits_four_when_the_blob_is_unverified(self):
        with mock.patch.object(
                module._GATE, "git_blob",
                return_value=("unverified-shallow", None, "shallow")), \
                mock.patch("sys.argv",
                                    ["check-section6-inventory.py",
                                     "--verify-pinned-blob"]):
            self.assertEqual(module.main(), 4)

    def test_pinned_cli_exits_two_on_a_mismatch(self):
        doctored = self.source.replace('Malformed("expr_instance")',
                                       'Malformed("pinned_only")', 1)
        with mock.patch.object(
                module._GATE, "git_blob",
                return_value=("verified", doctored, None)), \
                mock.patch("sys.argv",
                                    ["check-section6-inventory.py",
                                     "--verify-pinned-blob"]):
            self.assertEqual(module.main(), 2)

    def test_register_and_pinned_mode_are_mutually_exclusive(self):
        with mock.patch("sys.argv",
                                 ["check-section6-inventory.py",
                                  "--register", "some.md",
                                  "--verify-pinned-blob"]):
            with self.assertRaises(SystemExit):
                module.main()

if __name__=="__main__": unittest.main()
