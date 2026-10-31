"""Pure validation checks for #447 historical registry probe."""
import importlib.util
from pathlib import Path
import unittest


FILE = Path(__file__).resolve().parents[1] / "tools/n447-registry-pinned.py"
SPEC = importlib.util.spec_from_file_location("issue447_registry", FILE)
mod = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(mod)


class RegistryProbeTests(unittest.TestCase):
    def test_no_arbitrary_expression_acceptance(self):
        self.assertEqual(mod.classify('{"ExprString":{"data":"42"}}'), "other")

    def test_nil_is_recognized(self):
        self.assertEqual(mod.classify('[{"ExprNil": {}}]'), "Nil")

    def test_numeric_reply_is_recognized(self):
        self.assertEqual(mod.classify('[{"ExprInt": {"data": 42}}]'), "42")

    def test_different_value_is_not_passing(self):
        self.assertEqual(mod.classify('[{"ExprInt": {"data": 41}}]'), "other")

    def test_missing_expr_envelope_fails_closed(self):
        with self.assertRaises(AssertionError):
            mod.expr_values({"block": "anything"})

    def test_uri_input_is_validated(self):
        uri = "rho:id:" + "a" * 46
        self.assertIn(uri, mod.lookup_term(uri))
        self.assertIn('@"out"!(value)', mod.lookup_term(uri))
        with self.assertRaises(AssertionError):
            mod.lookup_term("bad-name")

    def test_empty_latest_block_must_not_substitute_for_signed_inclusion(self):
        history = [
            {"blockNumber": 2, "blockHash": "empty", "deployCount": 0},
            {"blockNumber": 1, "blockHash": "carrying", "deployCount": 1},
        ]
        def read(hash_value):
            return {"deploys": [{"term": "signed-insert", "sig": "sig1"}]
                    if hash_value == "carrying" else []}
        block, sig = mod.find_signed_carrier(history, 0, "signed-insert", read)
        self.assertEqual(block["blockHash"], "carrying")
        self.assertEqual(sig, "sig1")

    def test_no_matching_signed_deploy_fails_closed(self):
        history = [{"blockNumber": 1, "blockHash": "unrelated", "deployCount": 1}]
        self.assertEqual(
            mod.find_signed_carrier(history, 0, "signed-insert",
                                    lambda h: {"deploys": [{"term": "other"}]}),
            (None, None))


    def test_empty_expr_with_captured_out_means_nil(self):
        self.assertEqual(mod.classify_reply({"expr": [], "replySource": "out"}), "Nil")

    def test_no_output_does_not_count_as_nil(self):
        self.assertEqual(mod.classify_reply({"expr": [], "replySource": "none"}), "no_reply")

    def test_same_root_post_state_returns_number(self):
        self.assertEqual(mod.classify_reply(
            {"expr": [{"ExprInt": {"data": 42}}], "replySource": "out"}), "42")


if __name__ == "__main__":
    unittest.main()
