"""`tools/devrewards_manifest.py` — the canonical encoding, attribution and refusals.

The golden values are shared with `casper/tests/devrewards_manifest_encoding.rs`: both read
`tests/fixtures/devrewards_manifest_golden.json`, so a drift on either side fails one half.
"""

import importlib.util
import json
import pathlib
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
_spec = importlib.util.spec_from_file_location("devrewards_manifest", ROOT / "tools" / "devrewards_manifest.py")
m = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(m)

GOLDEN = json.loads((ROOT / "tests" / "fixtures" / "devrewards_manifest_golden.json").read_text())


def input_data(blocks, epoch=3, height=3, finalized=True):
    return {
        "epoch": epoch,
        "anchor": {"height": height, "hash": "cc" * 32, "finalized": finalized},
        "blocks": blocks,
    }


A = "aa" * 65
B = "bb" * 65
C = "cc" * 65


class EncodingTest(unittest.TestCase):
    def test_golden_leaf_root_and_digest(self):
        keys = [bytes.fromhex(k) for k in GOLDEN["keys"]]
        amounts = GOLDEN["amounts"]
        self.assertEqual(m.leaf(keys[0], amounts[0]).hex(), GOLDEN["leaf"])
        root, _ = m.build_tree(keys, amounts)
        self.assertEqual(root.hex(), GOLDEN["root"])
        self.assertEqual(
            m.digest(GOLDEN["epoch"], bytes.fromhex(GOLDEN["anchor_hash"]), root).hex(),
            GOLDEN["digest"],
        )

    def test_a_negative_amount_is_refused(self):
        with self.assertRaises(ValueError):
            m.leaf(bytes.fromhex(A), -1)

    def test_a_short_key_is_refused(self):
        with self.assertRaises(ValueError):
            m.leaf(b"\x11" * 64, 1)

    def test_a_manifest_needs_a_leaf(self):
        with self.assertRaises(ValueError):
            m.build_tree([], [])


class AttributionTest(unittest.TestCase):
    def test_the_first_appearance_installs_and_a_later_use_invokes(self):
        blocks = [
            {"height": 1, "finalized": True,
             "deploys": [{"deployer": A, "burned": 10, "events": [{"consume_hash": "01" * 32}]}]},
            {"height": 2, "finalized": True,
             "deploys": [{"deployer": B, "burned": 7, "events": [{"consume_hash": "01" * 32}]}]},
        ]
        totals = m.attribute(input_data(blocks))
        self.assertEqual(totals, {bytes.fromhex(A): 17})

    def test_two_installers_split_with_the_remainder_to_the_lowest_key(self):
        blocks = [
            {"height": 1, "finalized": True,
             "deploys": [{"deployer": A, "burned": 0, "events": [{"consume_hash": "01" * 32}]}]},
            {"height": 2, "finalized": True,
             "deploys": [{"deployer": B, "burned": 0, "events": [{"consume_hash": "02" * 32}]}]},
            {"height": 3, "finalized": True,
             "deploys": [{"deployer": C, "burned": 11,
                          "events": [{"consume_hash": "01" * 32}, {"consume_hash": "02" * 32}]}]},
        ]
        totals = m.attribute(input_data(blocks))
        self.assertEqual(totals[bytes.fromhex(A)], 6)
        self.assertEqual(totals[bytes.fromhex(B)], 5)
        self.assertNotIn(bytes.fromhex(C), totals)

    def test_block_order_does_not_change_the_manifest(self):
        blocks = [
            {"height": 1, "finalized": True,
             "deploys": [{"deployer": A, "burned": 10, "events": [{"consume_hash": "01" * 32}]}]},
            {"height": 2, "finalized": True,
             "deploys": [{"deployer": B, "burned": 7, "events": [{"consume_hash": "01" * 32}]}]},
        ]
        forward = m.build(input_data(blocks))
        backward = m.build(input_data(list(reversed(blocks))))
        self.assertEqual(forward, backward)

    def test_ten_thousand_usages_aggregate_into_one_leaf(self):
        blocks = [
            {"height": 1, "finalized": True,
             "deploys": [{"deployer": A, "burned": 1, "events": [{"consume_hash": "01" * 32}]}]},
        ]
        for i in range(10_000):
            blocks.append(
                {"height": 2, "finalized": True,
                 "deploys": [{"deployer": C, "burned": 1, "events": [{"consume_hash": "01" * 32}]}]}
            )
        manifest = m.build(input_data(blocks, epoch=3, height=2))
        self.assertEqual(len(manifest["leaves"]), 1)
        self.assertEqual(manifest["leaves"][0]["key"], A)
        self.assertEqual(manifest["leaves"][0]["amount"], 10_001)

    def test_a_float_burn_is_refused(self):
        blocks = [{"height": 1, "finalized": True,
                   "deploys": [{"deployer": A, "burned": 1.5, "events": []}]}]
        with self.assertRaises(ValueError):
            m.attribute(input_data(blocks))


class ManifestTest(unittest.TestCase):
    def sample(self):
        blocks = [
            {"height": 1, "finalized": True,
             "deploys": [{"deployer": A, "burned": 10, "events": [{"consume_hash": "01" * 32}]}]},
            {"height": 2, "finalized": True,
             "deploys": [
                 {"deployer": B, "burned": 20, "events": [{"consume_hash": "02" * 32}]},
                 {"deployer": C, "burned": 9,
                  "events": [{"consume_hash": "01" * 32}, {"consume_hash": "02" * 32}]},
             ]},
        ]
        return m.build(input_data(blocks, epoch=9, height=2))

    def test_build_and_verify_round_trip(self):
        manifest = self.sample()
        m.verify(manifest)
        self.assertEqual(manifest["root"], m.build_tree(
            [bytes.fromhex(leaf["key"]) for leaf in manifest["leaves"]],
            [leaf["amount"] for leaf in manifest["leaves"]],
        )[0].hex())

    def test_a_tampered_amount_is_refused(self):
        manifest = self.sample()
        manifest["leaves"][0]["amount"] += 1
        with self.assertRaises(ValueError):
            m.verify(manifest)

    def test_a_tampered_total_is_refused(self):
        manifest = self.sample()
        manifest["total"] += 1
        with self.assertRaises(ValueError):
            m.verify(manifest)

    def test_a_tampered_proof_is_refused(self):
        manifest = self.sample()
        key = manifest["leaves"][0]["key"]
        manifest["proofs"][key][0]["left"] = not manifest["proofs"][key][0]["left"]
        with self.assertRaises(ValueError):
            m.verify(manifest)

    def test_an_unfinalized_anchor_is_refused(self):
        blocks = [{"height": 1, "finalized": True,
                   "deploys": [{"deployer": A, "burned": 1, "events": []}]}]
        with self.assertRaises(ValueError):
            m.build(input_data(blocks, finalized=False))

    def test_a_deploy_after_the_anchor_is_refused(self):
        blocks = [
            {"height": 1, "finalized": True,
             "deploys": [{"deployer": A, "burned": 1, "events": []}]},
            {"height": 5, "finalized": True,
             "deploys": [{"deployer": A, "burned": 1, "events": []}]},
        ]
        with self.assertRaises(ValueError):
            m.build(input_data(blocks, height=3))


if __name__ == "__main__":
    unittest.main()
