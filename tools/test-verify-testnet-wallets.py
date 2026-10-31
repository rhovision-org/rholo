#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import unittest

path = Path(__file__).with_name("verify-testnet-wallets.py")
spec = importlib.util.spec_from_file_location("verify_testnet_wallets", path)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class GenesisWalletUnitsTest(unittest.TestCase):
    def test_rev_values_are_rejected_when_written_as_drops(self):
        errors = module.check_wallets("validator,1000000000000\ndeployer,10000\n", 10_000)
        self.assertEqual(len(errors), 1)
        self.assertIn("10000 drops", errors[0])

    def test_correct_10000_rev_allocation_is_accepted(self):
        self.assertEqual(module.check_wallets(
            "validator,1000000000000\ndeployer,1000000000000\n", 10_000
        ), [])

    def test_duplicate_and_invalid_entries_fail_closed(self):
        self.assertEqual(len(module.check_wallets(
            "validator,1000000000000\nvalidator,1000000000000\ninvalid\n", 10_000
        )), 2)

    def test_empty_manifest_fails_closed(self):
        self.assertTrue(module.check_wallets("# no wallets\n", 10_000))


if __name__ == "__main__":
    unittest.main()
