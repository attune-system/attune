"""Pure preparation checks using explicit synthetic files; no daemon/image claims."""
import copy
import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest
import zipfile
import yaml
spec = importlib.util.spec_from_file_location('prepare',Path(__file__).with_name('prepare-native-compose.py'))
module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
class Preparation(unittest.TestCase):
    def inputs(self,root):
        runner=root/'runner.zip'
        with zipfile.ZipFile(runner,'w') as archive:
            entry=zipfile.ZipInfo('linux-amd64');entry.external_attr=0o100555<<16
            archive.writestr(entry,b'synthetic-render-fixture-not-a-runner')
        runner.chmod(0o444)
        image={'reference':'sha256:'+'a'*64,'sha256':'a'*64}
        config={'image':image,'platform':'linux/amd64','cache_root':'/var/lib/attune/workloads','helper_archive':'/opt/attune/public/workload-runner.zip','memory_bytes':268435456,'scratch_bytes':67108864,'storage':{'compressed_bytes':67108864,'expanded_bytes':134217728,'entries':4096},'helper':{'pin_id':1,'kind':'runner','object_id':2,'object_version':'fixture','size_bytes':runner.stat().st_size,'sha256':hashlib.sha256(runner.read_bytes()).hexdigest(),'max_expanded_bytes':4096,'max_files':1},'backend':{'kind':'docker','socket':'/var/run/docker.sock','network':'owned-network','binding':{'engine_id':1,'daemon_id':'fixture-observed-daemon','platform':'linux/amd64'},'pids_limit':128,'helper_image':image}}
        result={}
        for service in module.MANAGERS:
            c=copy.deepcopy(config);c['backend']['cache_volume']=service+'-cache'
            result[service]={'workload':c,'runner_source':str(runner),'custody_volume':service+'-custody'}
        return result
    def test_all_managers_keep_policy_and_private_sources(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);profiles=self.inputs(root);output=root/'output'
            module.prepare(profiles,{},output,1234)
            compose=yaml.safe_load((output/'compose.yaml').read_text())
            for service in module.MANAGERS:
                manager=compose['services'][service]
                self.assertEqual(manager['user'],'1000:1000');self.assertEqual(manager['group_add'],['1234'])
                self.assertIn('ATTUNE__DATABASE__URL',manager['environment'])
                self.assertFalse('trusted-only' in str(manager))
                targets=[v.split(':')[1] for v in manager['volumes']]
                self.assertEqual(len(targets),len(set(targets)))
                self.assertIn(service+'-cache:/var/lib/attune/workloads',manager['volumes'])
                config=yaml.safe_load((output/(service+'.yaml')).read_text())
                self.assertEqual(config['worker']['log_delivery']['spool_dir'],'/var/lib/attune/custody/spool')
                self.assertEqual(config['artifacts_dir'],'/var/lib/attune/custody/artifacts')
                self.assertEqual(config['packs_base_dir'],'/var/lib/attune/custody/packs')
                self.assertEqual(config['packs']['transport'],'api')
            self.assertEqual(compose['services']['worker-toolchains']['profiles'],['toolchains'])
            self.assertEqual(compose['services']['sensor-secondary']['environment']['ATTUNE__SENSOR__LABELS__sensor_pool'],'priority')
    def test_missing_profile_shared_cache_and_wrong_archive_fail_before_writes(self):
        for bad in ('missing','shared','archive'):
            with self.subTest(bad=bad),tempfile.TemporaryDirectory() as d:
                root=Path(d);profiles=self.inputs(root);output=root/'output'
                if bad=='missing':profiles.pop('worker-toolchains')
                if bad=='shared':profiles['worker-node']['custody_volume']=profiles['worker-python']['custody_volume']
                if bad=='archive':profiles['sensor']['workload']['helper']['sha256']='f'*64
                with self.assertRaises(ValueError):module.prepare(profiles,{},output,1234)
                self.assertFalse(output.exists())
if __name__=='__main__':unittest.main()
