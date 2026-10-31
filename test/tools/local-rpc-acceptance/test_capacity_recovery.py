"""No workloads: exact old-seal and one new correctness-sample controller tests."""
import copy
import json
import os
from pathlib import Path
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch
import test_continuous_220 as fixtures
import capacity_recovery as RECOVERY


class RecoveryTests(unittest.TestCase):
    def setUp(self):
        self.env=patch.dict(os.environ,dict(GITHUB_ACTIONS='true',RPC_SOURCE_SHA='6'*40,GITHUB_SHA='8'*40,GITHUB_RUN_ID='900'),clear=True)
        self.env.start()
        self.git=patch.object(RECOVERY.screen,'git',side_effect=lambda root,*args: '' if args[0]=='status' else '6'*40 if args[-1]=='HEAD' else '7'*40)
        self.git.start()
        self.disk=patch.object(RECOVERY.shutil,'disk_usage',return_value=SimpleNamespace(free=16*RECOVERY.screen.GIB))
        self.disk.start()
    def tearDown(self):
        self.disk.stop();self.git.stop();self.env.stop()
    def setup_files(self,root):
        bundle=root/'bundle';bundle.mkdir()
        pins=dict(baseline=RECOVERY.CONTROL,candidate=RECOVERY.PRODUCT)
        binaries={}
        for label in pins:
            for kind in ('seed','server'):
                name=label+'-'+kind;(bundle/name).write_text(name);binaries[name]=RECOVERY.screen.sha(bundle/name)
        manifest=dict(controller=dict(run_id=str(RECOVERY.BUILD_RUN),sha=RECOVERY.OLD_CARRIER,
            normal_source_sha=RECOVERY.PRODUCT[0],normal_source_tree=RECOVERY.PRODUCT[1],source_pins=pins,
            engineering_control=RECOVERY.continuous.provenance(pins)),
            source={label:dict(product_sha=p[0],base_tree=p[1],production_source_changes=False)for label,p in pins.items()},binaries=binaries)
        RECOVERY.screen.write(bundle/'manifest.json',manifest)
        data=fixtures.ContinuousTests.data(self,1)
        data.update(status='incomplete',controller_sha=RECOVERY.OLD_CARRIER,normal_source_sha=RECOVERY.PRODUCT[0],manifest_sha256=RECOVERY.screen.sha(bundle/'manifest.json'))
        data['runs'][1]['status']='incomplete';data['runs'][1]['phases'].pop('tails-verify')
        prior=root/'prior.json';RECOVERY.screen.write(prior,data)
        return bundle,prior,manifest,data
    def bind(self,bundle,prior):
        return patch.multiple(RECOVERY,MANIFEST_SHA=RECOVERY.screen.sha(bundle/'manifest.json'),PRIOR_PAIR_SHA=RECOVERY.screen.sha(prior))
    def test_exact_manifest_source_tree_carrier_artifact_and_binary_rejection(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);bundle,prior,manifest,_=self.setup_files(root)
            with self.bind(bundle,prior):
                RECOVERY.checked_bundle(bundle);RECOVERY.prior_sample(prior)
                (bundle/'candidate-seed').write_text('changed')
                with self.assertRaises(AssertionError):RECOVERY.checked_bundle(bundle)
                (bundle/'candidate-seed').write_text('candidate-seed')
            for mutate in [lambda m:m['controller'].update(sha='0'*40),lambda m:m['controller'].update(run_id='1'),
                           lambda m:m['source']['candidate'].update(product_sha='0'*40),lambda m:m['source']['candidate'].update(base_tree='0'*40),
                           lambda m:m['source']['candidate'].update(production_source_changes=True),lambda m:m['controller'].update(normal_source_tree='0'*40)]:
                bad=copy.deepcopy(manifest);mutate(bad);RECOVERY.screen.write(bundle/'manifest.json',bad)
                with self.bind(bundle,prior),self.assertRaises(AssertionError):RECOVERY.checked_bundle(bundle)
            RECOVERY.screen.write(bundle/'manifest.json',manifest)
            with self.bind(bundle,prior):
                (bundle/'manifest.json').write_text('{}')
                with self.assertRaises(AssertionError):RECOVERY.checked_bundle(bundle)
    def test_prior_oracle_must_be_complete_unchanged_and_original_identity(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);bundle,prior,_,data=self.setup_files(root)
            for mutate in [lambda d:d.update(controller_sha='0'*40),lambda d:d['runs'][0]['phases'].pop('tails'),
                           lambda d:d['runs'][0]['phases']['tails-verify'].update(retained_seed_receipts=219),
                           lambda d:d['runs'][0]['phases']['tails-verify'].update(principal='other'),
                           lambda d:d['runs'][0]['phases']['tails-verify']['capacity']['principal']['metadata_bytes'].update(limit=str(128*1024*1024))]:
                bad=copy.deepcopy(data);mutate(bad);RECOVERY.screen.write(prior,bad)
                with self.bind(bundle,prior),self.assertRaises((AssertionError,KeyError)):RECOVERY.prior_sample(prior)
            RECOVERY.screen.write(prior,data)
            with self.bind(bundle,prior):
                prior.write_text('{}')
                with self.assertRaises(AssertionError):RECOVERY.prior_sample(prior)
    def test_plan_binds_budget_and_observer_and_rejects_expired_window(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);bundle,prior,_,_=self.setup_files(root)
            with self.bind(bundle,prior):
                plan=root/'plan.json';RECOVERY.select(bundle,prior,plan);original=json.loads(plan.read_text())
                for key,value in [('cpus',3),('phase_seconds',1201),('deadline_unix',original['deadline_unix']+1),('principal','other'),('principal_metadata_limit',128*1024*1024),('binary_artifact',0),('product',['0'*40,'0'*40])]:
                    bad=copy.deepcopy(original);bad[key]=value;RECOVERY.screen.write(plan,bad)
                    os.environ['RPC_PLAN_SHA256']=RECOVERY.screen.sha(plan)
                    with self.assertRaises(AssertionError):RECOVERY.checked_plan(plan)
                RECOVERY.screen.write(plan,original);os.environ['RPC_PLAN_SHA256']=RECOVERY.screen.sha(plan)
                with patch.object(RECOVERY.time,'time',return_value=original['deadline_unix']),patch.object(RECOVERY.screen,'phase') as phase:
                    with self.assertRaises(AssertionError):RECOVERY.run(bundle,prior,plan,root/'out')
                    phase.assert_not_called()
                os.environ['RUNNER_TEMP']=tmp
                with patch.object(RECOVERY.shutil,'disk_usage',return_value=SimpleNamespace(free=8*RECOVERY.screen.GIB-1)),patch.object(RECOVERY.screen,'host_observation',return_value={'child_cpus':[0,1]}),patch.object(RECOVERY.screen,'phase') as phase,patch('builtins.print'):
                    with self.assertRaises(AssertionError):RECOVERY.run(bundle,prior,plan,root/'space-failed')
                    phase.assert_not_called()
    def test_only_one_candidate_seven_phases_combines_with_exact_prior_without_relabeling_abba(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);bundle,prior,_,data=self.setup_files(root);os.environ['RUNNER_TEMP']=tmp
            with self.bind(bundle,prior):
                plan=root/'plan.json';RECOVERY.select(bundle,prior,plan);os.environ['RPC_PLAN_SHA256']=RECOVERY.screen.sha(plan)
                calls=[]
                def phase(command,env,directory,name,deadline):
                    self.assertIn('candidate-',command[0]);self.assertEqual(env['ZXS_RPC_COMMITS'],'220')
                    self.assertEqual(deadline,json.loads(plan.read_text())['deadline_unix']);calls.append(name)
                    work=Path(env['ZXS_RPC_ROOT'])
                    for file in ('operations','rpc-receipts.json','tails-receipts.json'):(work/file).write_text('evidence')
                    return copy.deepcopy(data['runs'][0]['phases'][name])
                with patch.object(RECOVERY.screen,'phase',side_effect=phase),patch.object(RECOVERY.screen,'host_observation',return_value={'child_cpus':[0,1]}),patch('builtins.print'):
                    RECOVERY.run(bundle,prior,plan,root/'out')
                report=json.loads((root/'out/summary.json').read_text())
                self.assertEqual(calls,list(RECOVERY.PHASES));self.assertEqual(report['candidate_samples_passed'],2)
                self.assertEqual(report['candidate_continuous_capacity'],'PASS');self.assertTrue(report['original_abba'].startswith('INCOMPLETE'))
                def incomplete(*args):
                    value=phase(*args)
                    if args[3]=='tails-verify':value['retained_workspaces']=358
                    return value
                with patch.object(RECOVERY.screen,'phase',side_effect=incomplete),patch.object(RECOVERY.screen,'host_observation',return_value={'child_cpus':[0,1]}),patch('builtins.print'),self.assertRaises(AssertionError):
                    RECOVERY.run(bundle,prior,plan,root/'bad')
                self.assertEqual(json.loads((root/'bad/summary.json').read_text())['candidate_samples_passed'],1)
                expired=[False]
                timing=json.loads(plan.read_text())
                def late_phase(*args):
                    result=phase(*args);expired[0]=True;return result
                with patch.object(RECOVERY.time,'time',side_effect=lambda:timing['deadline_unix'] if expired[0] else timing['created_unix']),patch.object(RECOVERY.screen,'phase',side_effect=late_phase),patch.object(RECOVERY.screen,'host_observation',return_value={'child_cpus':[0,1]}),patch('builtins.print'),self.assertRaises(AssertionError):
                    RECOVERY.run(bundle,prior,plan,root/'late')
                late=json.loads((root/'late/summary.json').read_text())
                self.assertEqual(set(late['arm']['phases']),{'build'})
                self.assertEqual(late['candidate_samples_passed'],1)
