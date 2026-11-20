#!/usr/bin/env python3
"""Offline security/packaging controls; real role SQL is exercised by Rust fixture."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import subprocess
import sys
from unittest import mock
import unittest
import uuid
import yaml
ROOT=Path(__file__).resolve().parents[1]

def load(name, file):
    spec=importlib.util.spec_from_file_location(name, ROOT/'scripts'/file)
    module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module);return module
job=load('job','render-rollout-job.py')
dba=load('dba','provision-rollout-admin.py')

class Assets(unittest.TestCase):
    def args(self, **kw):
        values=dict(name='owned-stage',namespace='owned',image='registry/api@sha256:'+'a'*64,input_secret='offline-input',receipt_pvc='offline-custody',receipt_name='receipt',operation='stage',confirm_generation=None)
        values.update(kw);return argparse.Namespace(**values)
    def test_job_separates_private_projection_and_custody(self):
        pod=job.render(self.args())['spec']['template']['spec']
        self.assertFalse(pod['automountServiceAccountToken'])
        self.assertFalse(pod['enableServiceLinks'])
        main=pod['containers'][0];init=pod['initContainers'][0]
        self.assertEqual(main['command'],['/usr/local/bin/attune-rollout-admin'])
        self.assertNotIn('projected', [m['name'] for m in main['volumeMounts']])
        self.assertNotIn('custody', [m['name'] for m in init['volumeMounts']])
        self.assertEqual(main['securityContext']['capabilities']['drop'],['ALL'])
        self.assertEqual(job.render(self.args())['spec']['backoffLimit'],0)
        self.assertNotIn('env',main)
    def test_confirmation_and_digest_required(self):
        for a in [self.args(image='api:latest'),self.args(operation='activate'),self.args(name='../secret'),self.args(operation='stage',confirm_generation=str(uuid.uuid4()))]:
            with self.assertRaises(ValueError):job.render(a)
        target=str(uuid.uuid4());a=job.render(self.args(operation='activate',confirm_generation=target))
        self.assertIn(target,a['spec']['template']['spec']['containers'][0]['args'])
    def test_compose_opt_in_no_default_mutation(self):
        spec=yaml.safe_load((ROOT/'deployment/rollout-admin/compose.yaml').read_text())
        service=spec['services']['rollout-admin']
        self.assertEqual(service['profiles'],['offline-rollout'])
        self.assertEqual(service['restart'],'no')
        self.assertEqual(service['command'][0],'verify-provisioning')
        self.assertNotIn('environment',service)
        self.assertTrue(all(not x['bind']['create_host_path'] for x in service['volumes']))
    def test_cache_profile_separates_migration_and_runtime_credentials(self):
        spec=yaml.safe_load((ROOT/'deployment/rollout-admin/cache-runtime.compose.yaml').read_text())
        migrate=spec['services']['cache-sqlx-migrate'];runtime=spec['services']['cache-api']
        self.assertEqual(migrate['profiles'],['cache-sqlx-migrate'])
        self.assertEqual(runtime['profiles'],['cache-api'])
        self.assertEqual(migrate['command'],['--config','/private/migrator.yaml','--migrate'])
        self.assertEqual(runtime['command'],['--config','/private/api.yaml'])
        self.assertNotEqual(migrate['volumes'][0]['source'],runtime['volumes'][0]['source'])
        for service in [migrate,runtime]:
            self.assertEqual(service['entrypoint'],['/usr/local/bin/attune-service'])
            self.assertEqual(service['restart'],'no')
            self.assertNotIn('environment',service)
            self.assertNotIn('depends_on',service)
            self.assertTrue(service['volumes'][0]['read_only'])
            self.assertFalse(service['volumes'][0]['bind']['create_host_path'])
    def test_private_dba_inputs(self):
        with tempfile.TemporaryDirectory() as root:
            p=Path(root)/'input';p.write_text('{"owned":true}');p.chmod(0o600)
            self.assertEqual(dba.private_json(p,100),{'owned':True})
            with self.assertRaises(ValueError):dba.private_json(p,1)
            link=Path(root)/'link';link.symlink_to(p)
            with self.assertRaises(OSError):dba.private_json(link,100)
            link.unlink();os.link(p,link)
            with self.assertRaises(ValueError):dba.private_json(p,100)
            link.unlink();p.chmod(0o640)
            with self.assertRaises(ValueError):dba.private_json(p,100)
    def test_redacted_driver_argument_error(self):
        result=subprocess.run([sys.executable,str(ROOT/'scripts/provision-rollout-admin.py'),'--unexpected','dummy-secret-marker'],env={},capture_output=True,timeout=10)
        self.assertEqual(result.returncode,1)
        self.assertEqual(json.loads(result.stderr),{'error':'provisioning_denied_or_commit_unknown'})
        self.assertNotIn(b'dummy-secret-marker',result.stderr)
    def test_private_input_path_swap_denied(self):
        with tempfile.TemporaryDirectory() as root:
            p=Path(root)/'input';p.write_text('{}');p.chmod(0o600)
            original=os.fstat; calls=0
            def checked(fd):
                nonlocal calls
                calls+=1
                if calls==2:
                    p.rename(Path(root)/'original');p.write_text('{}');p.chmod(0o600)
                return original(fd)
            with mock.patch.object(dba.os,'fstat',side_effect=checked):
                with self.assertRaises(ValueError):dba.private_json(p,100)
    def test_no_automatic_role_or_schema_creation(self):
        sql=(ROOT/'deployment/rollout-admin/provision.sql').read_text().upper()
        for prohibited in ['CREATE ROLE','ALTER ROLE','CREATE SCHEMA','ALTER SCHEMA','ALTER DEFAULT PRIVILEGES']:
            self.assertNotIn(prohibited,sql)
        self.assertIn('PG_STAT_ACTIVITY',sql)
        self.assertIn('INHERIT FALSE, SET FALSE',sql)

if __name__=='__main__':unittest.main()
