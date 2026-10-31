"""Offline negative controls for active proposer restart acceptance."""
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('post_restart', ROOT/'tests/consensus-post-restart-proposal.py')
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


class ProposalWitnessTests(unittest.TestCase):
    def test_complete_monotone_witness(self):
        old=m.proposals('proposed and added block #67 (seq 42)')
        new=m.proposals('proposed and added block #68 (seq 43)\nproposed and added block #69 (seq 44)')
        self.assertEqual([x['seq'] for x in m.post_restart_proposals(old,new)],[43,44])

    def test_stale_or_passive_finality_is_not_a_proposal(self):
        old=m.proposals('proposed and added block #67 (seq 42)')
        for log in ['validator key enabled; LFB advanced',
                    'proposed and added block #67 (seq 42)',
                    'proposed and added block #68 (seq 41)']:
            with self.subTest(log=log), self.assertRaises(AssertionError):
                m.post_restart_proposals(old,m.proposals(log))

    def test_missing_baseline_is_not_accepted(self):
        with self.assertRaises(AssertionError):
            m.post_restart_proposals([],m.proposals('proposed and added block #1 (seq 0)'))

    def test_regressed_proposal_sequences_fail(self):
        with self.assertRaises(AssertionError):
            m.proposals('proposed and added block #71 (seq 44)\nproposed and added block #72 (seq 43)')

    def test_only_one_durable_state_volume_is_acceptable(self):
        mount={'Destination':'/var/lib/rnode','Type':'volume','Name':'acceptance-validator-7-data','Source':'/vol/v7'}
        node={'Mounts':[mount,{'Destination':'/genesis','Type':'bind','Source':'/fixture'}]}
        self.assertEqual(m.durable_volume(node),('acceptance-validator-7-data','/vol/v7','/var/lib/rnode'))
        for mounts in [[],[mount,mount],[{'Destination':'/var/lib/rnode','Type':'bind','Name':'host','Source':'/tmp'}]]:
            with self.subTest(mounts=mounts),self.assertRaises(AssertionError):
                m.durable_volume({'Mounts':mounts})

    def test_missing_divergence_and_orphan_counters_fail_closed(self):
        health={'ready':True,'orphanBodies':0,'pipelineFaults':0,'proposalHalted':False}
        operator={'unrestorableRecords':0}
        with patch.object(m.a,'get',side_effect=lambda i,path,admin=False: operator if admin else health):
            self.assertTrue(m.healthy(7))
        for h,o in [(dict(health,orphanBodies=None),operator),
                    (dict(health,orphanBodies=1),operator),
                    (dict(health,proposalHalted=True),operator),
                    (health,{'unrestorableRecords':None}),
                    (health,{'unrestorableRecords':1})]:
            with self.subTest(h=h,o=o):
                with patch.object(m.a,'get',side_effect=lambda i,path,admin=False: o if admin else h):
                    self.assertFalse(m.healthy(7))

    def test_deploy_only_to_restart_target(self):
        class Success:
            returncode=0
            stderr=''
            stdout='accepted'
        with patch.object(m.a,'get',return_value={'latestBlockNumber':67,'shardId':'/root'}), \
             patch.object(m.subprocess,'run',return_value=Success()) as run:
            record=m.signed_deploy_to_restarted_validator(7,'acceptance-validator-7',1)
        self.assertEqual(record['valid_after_block_number'],67)
        self.assertEqual(run.call_count,2)
        self.assertEqual(run.call_args.args[0][2],'acceptance-validator-7')
        self.assertIn('--valid-after-block-number',run.call_args.args[0])


if __name__=='__main__':
    unittest.main()
