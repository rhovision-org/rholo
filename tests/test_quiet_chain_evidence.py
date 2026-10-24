"""Offline falsifiers for the quiet-chain release verdict; no HTTP is allowed."""
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('quiet', Path(__file__).resolve().parents[1] / 'tools/quiet-chain-issue-435.py')
quiet = importlib.util.module_from_spec(spec)
spec.loader.exec_module(quiet)

class QuietEvidence(unittest.TestCase):
    def setUp(self):
        self.observations = {str(i): dict(finalized_height=10, finalized_hash='a'*64, marker_present=True) for i in range(3)}
        self.operators = {str(i): dict(unrestorableRecords=0) for i in range(3)}
        self.network = patch.object(quiet.urllib.request, 'urlopen', side_effect=AssertionError('offline test attempted HTTP'))
        self.network.start()
        self.addCleanup(self.network.stop)

    def verdict(self):
        return quiet.complete_finalized_observation(self.observations, self.operators, range(3))

    def test_complete_agreement_passes(self):
        self.assertTrue(self.verdict())

    def test_different_finalized_hashes_fail_even_with_every_marker(self):
        self.observations['2']['finalized_hash'] = 'b'*64
        self.assertFalse(self.verdict())

    def test_missing_or_failed_current_readback_cannot_reuse_an_old_pass(self):
        del self.observations['2']
        self.assertFalse(self.verdict())
        self.observations['2'] = dict(error='HTTP 400')
        self.assertFalse(self.verdict())

    def test_operator_evidence_is_mandatory(self):
        del self.operators['2']
        self.assertFalse(self.verdict())

    def test_divergence_or_unknown_counter_refuses_pass(self):
        for value in (1, None, '0', False):
            self.operators['2']['unrestorableRecords'] = value
            self.assertFalse(self.verdict())

    def test_invalid_finalized_identity_refuses_pass(self):
        for value in ('', 'g'*64, None):
            self.observations['2']['finalized_hash'] = value
            self.assertFalse(self.verdict())

    def test_bond_map_must_be_complete_positive_and_unique(self):
        bonds = [dict(validator=str(i), stake=100) for i in range(4)]
        self.assertEqual(quiet.canonical_bonds(dict(bonds=bonds), 4), {str(i): 100 for i in range(4)})
        for rows in (bonds[:3], bonds + bonds[:1], bonds[:3] + [bonds[0]], bonds[:3] + [dict(validator='3', stake=0)]):
            with self.assertRaisesRegex(RuntimeError, 'BOND_MAP_MISMATCH'):
                quiet.canonical_bonds(dict(bonds=rows), 4)

    def test_absent_marker_refuses_pass(self):
        self.observations['2']['marker_present'] = False
        self.assertFalse(self.verdict())

if __name__ == '__main__':
    unittest.main(verbosity=2)
