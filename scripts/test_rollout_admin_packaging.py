#!/usr/bin/env python3
"""Offline bundle-layout/one-shot smoke; never launches a service or daemon."""
import argparse,json,os,pathlib,shutil,subprocess,tempfile,uuid
p=argparse.ArgumentParser();p.add_argument('--binary',required=True);a=p.parse_args()
root=pathlib.Path(__file__).resolve().parents[1]
for name,fragment in [('docker/Dockerfile.optimized','/build/rollout-tools/'),('docker/Dockerfile.runtime','dist/rollout-tools/'),('.github/workflows/publish.yml','cp "${service_output_dir}/attune-rollout-admin" "$bundle_root/bin/"')]:
 assert fragment in (root/name).read_text(),name
for name in ['docker/Dockerfile.optimized','docker/Dockerfile.runtime']:
 assert 'CMD ["/usr/local/bin/attune-service"]' in (root/name).read_text()
with tempfile.TemporaryDirectory(prefix='rollout-bundle-') as temp:
 d=pathlib.Path(temp);d.chmod(0o700);(d/'bin').mkdir();b=d/'bin'/'attune-rollout-admin';shutil.copy2(a.binary,b)
 result=subprocess.run([str(b),'--help'],env={},capture_output=True,timeout=10,check=True)
 assert b'one-shot' in result.stdout and not result.stderr
 manifest={'snapshot':{'contract_version':1,'catalog_epoch':1,'catalog_revision':0,'release_digest':'a'*64,'rollout_generation':str(uuid.uuid4())},'members':[{'role':r,'instance':str(uuid.uuid4()),'boot':str(uuid.uuid4()),'required_mask':m} for r,m in [('api',15),('notifier',16),('executor',65),('worker',40)]]}
 file=d/'manifest.json';file.write_text(json.dumps(manifest));file.chmod(0o600)
 result=subprocess.run([str(b),'validate','--manifest',str(file)],env={},capture_output=True,timeout=10,check=True)
 assert json.loads(result.stdout)['valid'] is True
 result=subprocess.run([str(b),'--config','does-not-exist'],env={},capture_output=True,timeout=10)
 assert result.returncode and json.loads(result.stderr)=={'error':'invalid_arguments'}
print('PASS: API bundle companion, unchanged service CMD, one-shot help/validate/argument rejection')
