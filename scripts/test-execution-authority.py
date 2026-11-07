#!/usr/bin/env python3
"""Owned API/broker/production-worker acceptance for delegated system Key reads.

Requires built API/executor/worker binaries, Docker, OpenSSL, PyYAML and requests.
No recorded readiness flags, direct token minting, or deployment exemptions.
"""
import argparse
import base64
import io
import hashlib
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import tarfile
import time
import uuid

import requests
import yaml

ROOT = Path(__file__).resolve().parents[1]

ACTION = r'''
import json, os, sys, urllib.request, urllib.error
from pathlib import Path
p=json.load(sys.stdin)
token=os.environ.get('ATTUNE_API_TOKEN')
assert token
base=os.environ['ATTUNE_API_URL']
def get(ref):
    req=urllib.request.Request(base+'/api/v1/keys/'+ref,headers={'Authorization':'Bearer '+token})
    try:
        with urllib.request.urlopen(req,timeout=10) as r:
            return r.status,json.load(r)['data'].get('value')
    except urllib.error.HTTPError as e:
        return e.code,None
status,value=get(p['key'])
other,other_value=get(p['other'])
assert other in (403,404) and other_value is None
if p['plaintext']:
    assert status==200 and isinstance(value,str)
    print(value,file=sys.stderr,flush=True)
    if p.get('fail_once'):
        marker=Path(os.environ['ATTUNE_ARTIFACTS_DIR'])/('retry-'+p['marker'])
        try:
            fd=os.open(marker,os.O_CREAT|os.O_EXCL|os.O_WRONLY,0o600)
            os.close(fd)
            print(json.dumps({'first_attempt':True,'value':value}))
            sys.exit(1)
        except FileExistsError:
            pass
else:
    assert status in (200,403,404) and value is None
output={'value':value,'credential':token,'unrelated_denied':True,'plaintext':p['plaintext']}
if p.get('item_id') is not None:
    output['queue_ack']={'version':1,'items':[{'id':p['item_id'],'status':value if p.get('ack_invalid') else 'completed','summary':{'value':value},'error':{'value':value}}]}
print(json.dumps(output))
'''


def wait_for(predicate, seconds, description):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            value = predicate()
            if value:
                return value
        except (requests.RequestException, subprocess.SubprocessError):
            pass
        time.sleep(0.2)
    raise RuntimeError('Timed out: ' + description)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--bin-dir', type=Path, default=ROOT / 'target/debug')
    parser.add_argument('--runtime-stack-bytes',type=int,default=0,help='Default: leave RUST_MIN_STACK unset; a positive value is an explicitly recorded diagnostic override')
    parser.add_argument('--service-image', help='Run the supplied Linux release binaries in this ABI-compatible Docker runtime image on an owned network')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False, mode=0o700)
    evidence = args.output.resolve()
    run = 'authority-' + uuid.uuid4().hex[:12]
    containers, processes, logs = [], [], []
    network = None
    secret = secrets.token_hex(24)
    result = {'run': run, 'head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(), 'checks': []}

    result['runtime_stack_bytes']=args.runtime_stack_bytes
    result['runtime']='docker' if args.service_image else 'host'
    result['binaries'] = {}
    result['binary_paths'] = {}
    for binary in ['attune-api','attune-executor','attune-worker']:
        result['binary_paths'][binary]=str((args.bin_dir / binary).resolve())
        with (args.bin_dir / binary).open('rb') as file:
            result['binaries'][binary]=hashlib.file_digest(file,'sha256').hexdigest()
    changed=sorted(set(subprocess.check_output(['git','diff','--name-only','origin/main'],cwd=ROOT,text=True).splitlines()+subprocess.check_output(['git','ls-files','--others','--exclude-standard'],cwd=ROOT,text=True).splitlines()))
    (evidence/'tested.patch').write_bytes(subprocess.check_output(['git','diff','HEAD'],cwd=ROOT))
    (evidence/'tested-source.sha256').write_text(''.join(hashlib.sha256((ROOT/path).read_bytes()).hexdigest()+'  '+path+'\n' for path in sorted(changed) if (ROOT/path).is_file()))

    # Test services take all Attune configuration from this run's explicit
    # file/context; inherited operator settings must never select another DB.
    result['ignored_attune_environment']=sorted(key for key in os.environ if key.startswith('ATTUNE_'))
    def owned_environment():
        values={key:value for key,value in os.environ.items() if not key.startswith('ATTUNE_')}
        if args.runtime_stack_bytes:
            values['RUST_MIN_STACK']=str(args.runtime_stack_bytes)
        else:
            values.pop('RUST_MIN_STACK',None)
        return values

    def command(argv, **kwargs):
        kwargs.setdefault('env', dict(owned_environment(), ATTUNE__ENVIRONMENT='authority_acceptance'))
        return subprocess.run(argv, check=True, timeout=kwargs.pop('timeout', 60), **kwargs)

    def docker_container(kind, image, ports, extra):
        name = run + '-' + kind
        # Register ownership before invoking Docker: a client timeout may still
        # have created the container on the daemon.
        containers.append(name)
        command(['docker', 'create', '--name', name, '--label', 'attune.authority.run=' + run,
                 *(['--network', network] if network else []),
                 *sum((['-p', '127.0.0.1::' + str(port)] for port in ports), []), *extra, image], stdout=subprocess.DEVNULL, timeout=180)
        command(['docker','start',name],stdout=subprocess.DEVNULL,timeout=180)
        return name

    def port(name, value):
        return int(subprocess.check_output(['docker', 'port', name, str(value) + '/tcp'], text=True).strip().rsplit(':', 1)[1])

    def psql(sql):
        return subprocess.check_output(['docker', 'exec', pg, 'psql', '-X', '-U', 'attune', '-d', 'attune', '-v', 'ON_ERROR_STOP=1', '-qAt', '-c', 'SET search_path TO attune,public; ' + sql], text=True, timeout=20).strip()

    def service_command(binary, config, migrate=False):
        if not args.service_image:
            return [str(args.bin_dir.resolve() / binary), '--config', str(config), *(['--migrate'] if migrate else [])]
        name=run+'-'+binary.removeprefix('attune-')+('-migrate' if migrate else '')
        containers.append(name)
        argv=['docker','run','--name',name,'--label','attune.authority.run='+run,
              '--network',network,'--user',str(os.getuid())+':'+str(os.getgid()),
              '--entrypoint','/usr/local/bin/attune-service',
              '-v',str(evidence)+':'+str(evidence),
              '-v',str((args.bin_dir/binary).resolve())+':/usr/local/bin/attune-service:ro',
              '-e','ATTUNE__ENVIRONMENT=authority_acceptance','-e','RUST_LOG=info',
              '-e','ATTUNE_WORKER_NAME='+run,
              '-e','ATTUNE_API_URL=http://'+run+'-api:'+str(api_port)]
        if args.runtime_stack_bytes:
            argv += ['-e','RUST_MIN_STACK='+str(args.runtime_stack_bytes)]
        if binary=='attune-api' and not migrate:
            argv += ['-p','127.0.0.1:'+str(api_port)+':'+str(api_port)]
        return [*argv,args.service_image,'--config',str(config),*(['--migrate'] if migrate else [])]

    def start(binary, config):
        log = (evidence / (binary + '.log')).open('w')
        logs.append(log)
        env = dict(owned_environment(), ATTUNE_API_URL=base, ATTUNE_WORKER_NAME=run, ATTUNE__ENVIRONMENT='authority_acceptance', RUST_LOG='info')
        if args.runtime_stack_bytes:
            env['RUST_MIN_STACK']=str(args.runtime_stack_bytes)
        else:
            env.pop('RUST_MIN_STACK',None)
        proc = subprocess.Popen(service_command(binary,config), cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT)
        processes.append(proc)

    try:
        wait_for(lambda: subprocess.run(['docker', 'info', '--format', '{{.ServerVersion}}'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10).returncode == 0, 180, 'Docker daemon')
        if args.service_image:
            result['service_image_ref']=args.service_image
            command(['docker','pull',args.service_image],stdout=(evidence/'image-pull.log').open('w'),stderr=subprocess.STDOUT,timeout=300)
            metadata=json.loads(subprocess.check_output(['docker','image','inspect',args.service_image],text=True))[0]
            if not args.runtime_stack_bytes:
                assert not any(value.startswith('RUST_MIN_STACK=') for value in metadata['Config'].get('Env',[])), 'runtime image overrides the default Rust stack'
            args.service_image=metadata['Id']
            result['service_image_id']=args.service_image
            network=run
            command(['docker','network','create','--label','attune.authority.run='+run,network],stdout=subprocess.DEVNULL)
        pg = docker_container('pg', 'postgres:18-alpine', [5432], ['-e', 'POSTGRES_HOST_AUTH_METHOD=trust', '-e', 'POSTGRES_USER=attune', '-e', 'POSTGRES_DB=attune', '--tmpfs', '/var/lib/postgresql'])
        broker = docker_container('mq', 'rabbitmq:4.3.6-management-alpine', [5672], ['-e', 'RABBITMQ_DEFAULT_USER=attune', '-e', 'RABBITMQ_DEFAULT_PASS=attune', '-e', 'RABBITMQ_DEFAULT_VHOST=' + run])
        wait_for(lambda: subprocess.run(['docker', 'exec', pg, 'pg_isready', '-h', '127.0.0.1', '-U', 'attune'], stdout=subprocess.DEVNULL, timeout=10).returncode == 0, 90, 'PostgreSQL')
        with socket.socket() as s:
            s.bind(('127.0.0.1', 0))
            api_port = s.getsockname()[1]
        base = 'http://127.0.0.1:' + str(api_port)
        config = yaml.safe_load((ROOT / 'config.test.yaml').read_text())
        config['database'].update(url=f'postgresql://attune@127.0.0.1:{port(pg,5432)}/attune', schema='attune', max_connections=15)
        config['message_queue']['url'] = f'amqp://attune:attune@127.0.0.1:{port(broker,5672)}/{run}'
        config['server'].update(port=api_port, request_timeout=600, max_body_size=104857600)
        if args.service_image:
            config['database']['url']=f'postgresql://attune@{pg}:5432/attune'
            config['message_queue']['url']=f'amqp://attune:attune@{broker}:5672/{run}'
            config['server']['host']='0.0.0.0'

        config['log'].update(level='info', mirror_runtime_stdout_to_stdio=True, mirror_runtime_stderr_to_stdio=True)
        for key in ['packs_base_dir', 'runtime_envs_dir', 'artifacts_dir']:
            path = evidence / key
            path.mkdir()
            config[key] = str(path)
        config['artifacts'] = {'transport': 'volume'}
        config['storage'] = {'provider': 'filesystem', 'root': str(evidence / 'blobs')}
        config['worker'] = None
        cfg = evidence / 'config.json'
        cfg.write_text(json.dumps(config))
        command(service_command('attune-api',cfg,migrate=True), stdout=(evidence / 'migrations.log').open('w'), stderr=subprocess.STDOUT, timeout=180)
        # Existing owned bootstrap seed, with production catalog role assignments.
        command(['docker', 'exec', '-i', '-e', 'DB_HOST=127.0.0.1', '-e', 'DB_NAME=attune', pg, 'sh'], input=(ROOT / 'docker/init-user.sh').read_bytes(), stdout=(evidence / 'init-user.log').open('w'), stderr=subprocess.STDOUT)
        wait_for(lambda: subprocess.run(['docker', 'exec', broker, 'rabbitmq-diagnostics', '-q', 'check_running'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10).returncode == 0, 120, 'RabbitMQ')
        start('attune-api', cfg)
        wait_for(lambda: requests.get(base + '/health/ready', timeout=2).status_code == 200, 120, 'API readiness')
        admin = requests.Session()
        login = admin.post(base + '/auth/login', json={'login': 'test@attune.local', 'password': 'TestPass123!'}, timeout=20)
        assert login.status_code == 200, 'bootstrap login failed'
        token = login.json()['data']['access_token']
        admin.headers['Authorization'] = 'Bearer ' + token
        start('attune-executor', cfg)
        start('attune-worker', cfg)
        def worker_registered():
            if any(proc.poll() is not None for proc in processes):
                raise RuntimeError('A production service exited during startup; inspect service logs')
            return int(psql("SELECT count(*) FROM worker WHERE status='active' AND capabilities->>'delegated_execution_authority_v1'='true'"))>0
        wait_for(worker_registered,120,'production worker registration')

        def api(method, path, **kwargs):
            response = admin.request(method, base + '/api/v1' + path, timeout=180, **kwargs)
            if response.status_code not in (200, 201, 204):
                # Error envelopes contain diagnostics, never Key response values.
                raise RuntimeError(f'{method} {path}: HTTP {response.status_code}: {response.text[:1000]}')
            return response.json().get('data') if response.content else None

        keys = []
        result['system_keys']=[]
        for suffix, value in [('selected', secret), ('unrelated', secrets.token_hex(24))]:
            local = run.replace('-', '_') + '_' + suffix
            key = api('POST', '/keys', json={'local_ref': local, 'owner_type': 'system', 'name': local, 'value': value, 'encrypted': True})
            assert key['owner_type'] == 'system' and key['encrypted']
            keys.append(key['ref'])
            result['system_keys'].append({'role':suffix,'ref':key['ref'],'owner_type':key['owner_type'],'encrypted':key['encrypted']})
        signer_local=run.replace('-','_')+'_signer'
        signer=api('POST','/keys',json={'local_ref':signer_local,'owner_type':'system','name':signer_local,'encrypted':True,'value':{'private_key_pem':(ROOT/'crates/common/tests/fixtures/jwt_signing_test_private.pem').read_text(),'profiles':{'acceptance':{'issuer':'approved-client','audience':'https://auth.example.test','subjects':['approved@example.test'],'pack_refs':[run.replace('-','_')],'max_ttl_seconds':120}}}})
        assert signer['owner_type']=='system' and signer['encrypted']
        result['system_keys'].append({'role':'signer','ref':signer['ref'],'owner_type':signer['owner_type'],'encrypted':signer['encrypted']})
        pack = evidence / 'pack'
        for directory in ['actions', 'runtimes', 'permission_sets', 'queues', 'triggers']:
            (pack / directory).mkdir(parents=True, exist_ok=True)
        pack_ref = run.replace('-', '_')
        action_ref = pack_ref + '.read_system'
        (pack / 'pack.yaml').write_text(yaml.safe_dump({'ref': pack_ref, 'name': pack_ref, 'version': '1.0.0', 'description': 'Delegated authority acceptance'}))
        (pack / 'runtimes/python.yaml').write_text(yaml.safe_dump({'ref': pack_ref + '.python', 'name': 'Python', 'aliases': ['python3'], 'execution_config': {'interpreter': {'binary': 'python3', 'args': ['-u'], 'file_extension': '.py'}}}))
        (pack / 'actions/read_system.yaml').write_text(yaml.safe_dump({'ref': action_ref, 'name': 'read_system', 'label': 'Read system Key', 'runner_type': pack_ref + '.python', 'entry_point': 'read_system.py', 'parameter_delivery': 'stdin', 'parameter_format': 'json', 'output_format': 'json', 'parameters': {'key': {'type': 'string', 'required': True}, 'other': {'type': 'string', 'required': True}, 'plaintext': {'type': 'boolean', 'required': True}, 'fail_once': {'type': 'boolean', 'default': False}, 'marker': {'type': 'string', 'default': ''}, 'item_id': {'type':'integer'}, 'ack_invalid': {'type':'boolean','default':False}}}))
        (pack / 'actions/read_system.py').write_text(ACTION)
        (pack / 'actions/workflows').mkdir()
        flow_ref=pack_ref+'.read_flow'
        (pack / 'actions/read_flow.yaml').write_text(yaml.safe_dump({'ref':flow_ref,'label':'Delegated retry workflow','workflow_file':'workflows/read_flow.workflow.yaml','parameters':{'key':{'type':'string','required':True},'other':{'type':'string','required':True},'plaintext':{'type':'boolean','required':True},'fail_once':{'type':'boolean','default':False},'marker':{'type':'string','default':''}}}))
        (pack / 'actions/workflows/read_flow.workflow.yaml').write_text(yaml.safe_dump({'version':'1.0.0','tasks':[{'name':'read','action':action_ref,'permission_set_refs':['standard',pack_ref+'.read_system'],'input':{name:'{{ parameters.'+name+' }}' for name in ['key','other','plaintext','fail_once','marker']},'retry':{'count':1,'delay':0},'next':[]}]}))
        tokenless_ref=pack_ref+'.tokenless'
        (pack / 'actions/tokenless.yaml').write_text(yaml.safe_dump({'ref':tokenless_ref,'label':'Tokenless','runner_type':pack_ref+'.python','entry_point':'tokenless.py','parameter_format':'json','output_format':'json','parameters':{'message':{'type':'string','required':True},'item_id':{'type':'integer'}}}))
        (pack / 'actions/tokenless.py').write_text("import json,os,sys\np=json.load(sys.stdin)\nr={'token_present':bool(os.environ.get('ATTUNE_API_TOKEN')),'message':p['message']}\nif 'item_id' in p: r['queue_ack']={'version':1,'items':[{'id':p['item_id'],'status':'completed'}]}\nprint(json.dumps(r))\n")

        # Native workflows keep real managed definitions and separate producer
        # fences; output_map exercises source restoration and private propagation.
        for private in [False, True]:
            name = 'private_ack_flow' if private else 'public_ack_flow'
            params = ({'key':{'type':'string','required':True},'other':{'type':'string','required':True},'plaintext':{'type':'boolean','required':True},'item_id':{'type':'integer','required':True}}
                      if private else {'message':{'type':'string','required':True},'item_id':{'type':'integer','required':True}})
            (pack / ('actions/'+name+'.yaml')).write_text(yaml.safe_dump({'ref':pack_ref+'.'+name,'label':name,'workflow_file':'workflows/'+name+'.workflow.yaml','parameters':params}))
            task = {'name':'emit','action':action_ref if private else tokenless_ref,
                    'permission_set_refs':['standard',pack_ref+'.read_system'] if private else [],
                    'input':{key:'{{ parameters.'+key+' }}' for key in params},'next':[]}
            (pack / ('actions/workflows/'+name+'.workflow.yaml')).write_text(yaml.safe_dump({'version':'1.0.0','tasks':[task],'output_map':{'queue_ack':'{{ task.emit.queue_ack }}'}}))

        managed_queue_ref=pack_ref+'.sponsored_queue'
        (pack/'queues/sponsored_queue.yaml').write_text(yaml.safe_dump({'ref':managed_queue_ref,'label':'Recorded-only sponsored queue','dispatch_action':tokenless_ref,'permission_set_refs':[],'action_params':{'message':'sponsored'}}))
        for name, actions in [('read_system', ['read', 'decrypt']), ('use_system', ['use']), ('read_metadata', ['read'])]:
            (pack / ('permission_sets/' + name + '.yaml')).write_text(yaml.safe_dump({'ref': pack_ref + '.' + name, 'scope': 'external', 'grants': [{'resource': 'keys', 'actions': actions, 'constraints': {'refs': [keys[0]], 'owner_types': ['system']}}]}))
        signer_ref=pack_ref+'.sign_system'
        (pack/'actions/sign_system.yaml').write_text(yaml.safe_dump({'ref':signer_ref,'label':'Approved use-only signer','runner_type':pack_ref+'.python','entry_point':'sign_system.py','parameter_format':'json','output_format':'json','parameters':{'signer':{'type':'string','required':True}}}))
        (pack/'actions/sign_system.py').write_text("""
import json,os,sys,urllib.request,urllib.error
p=json.load(sys.stdin)
base=os.environ['ATTUNE_API_URL']+'/api/v1/keys/'+p['signer']
headers={'Authorization':'Bearer '+os.environ['ATTUNE_API_TOKEN'],'Content-Type':'application/json'}
try:
    with urllib.request.urlopen(urllib.request.Request(base,headers=headers),timeout=10) as r:
        assert json.load(r)['data'].get('value') is None
except urllib.error.HTTPError as e:
    assert e.code in (403,404)
def sign(subject):
    body=json.dumps({'profile_ref':'acceptance','subject':subject,'ttl_seconds':60}).encode()
    with urllib.request.urlopen(urllib.request.Request(base+'/sign-jwt',data=body,headers=headers),timeout=10) as r:
        return json.load(r)['data']
result=sign('approved@example.test')
assert 'private_key_pem' not in result and len(result['assertion'].split('.'))==3
try:
    sign('unapproved@example.test')
    raise AssertionError('unapproved signing subject accepted')
except urllib.error.HTTPError as e:
    assert e.code==403
print(json.dumps({'assertion':result['assertion'],'raw_denied':True,'profile_denied':True}))
""")
        (pack/'permission_sets/sign_system.yaml').write_text(yaml.safe_dump({'ref':signer_ref,'scope':'external','grants':[{'resource':'keys','actions':['use'],'constraints':{'refs':[signer['ref']],'owner_types':['system']}}]}))
        (pack / 'permission_sets/invoke.yaml').write_text(yaml.safe_dump({'ref': pack_ref + '.invoke', 'grants': [{'resource': 'actions', 'actions': ['execute'], 'constraints': {'refs': [action_ref,signer_ref]}}, {'resource': 'executions', 'actions': ['read', 'decrypt'], 'constraints': {'refs': [action_ref,signer_ref]}}, {'resource':'artifacts','actions':['read']}, {'resource':'queues','actions':['create'],'constraints':{'refs':[pack_ref+'.private_ack_queue',pack_ref+'.invalid_ack_queue']}}]}))
        event_ref=pack_ref+'.authority_event'
        emitter_ref=pack_ref+'.emit_event'
        (pack/'triggers/authority_event.yaml').write_text(yaml.safe_dump({'ref':event_ref,'label':'Authority acceptance event','out_schema':{'marker':{'type':'string'}}}))
        (pack/'actions/emit_event.yaml').write_text(yaml.safe_dump({'ref':emitter_ref,'label':'Delegated event emitter','runner_type':pack_ref+'.python','entry_point':'emit_event.py','parameter_format':'json','output_format':'json','parameters':{'trigger':{'type':'string','required':True},'marker':{'type':'string','required':True}}}))
        (pack/'permission_sets/emit_event.yaml').write_text(yaml.safe_dump({'ref':emitter_ref,'scope':'external','grants':[{'resource':'events','actions':['create'],'constraints':{'refs':[event_ref]}}]}))
        (pack/'actions/emit_event.py').write_text("import json,os,sys,urllib.request\np=json.load(sys.stdin)\nr=urllib.request.Request(os.environ['ATTUNE_API_URL']+'/api/v1/events',data=json.dumps({'trigger_ref':p['trigger'],'payload':{'marker':p['marker']}}).encode(),headers={'Authorization':'Bearer '+os.environ['ATTUNE_API_TOKEN'],'Content-Type':'application/json'},method='POST')\nwith urllib.request.urlopen(r,timeout=15) as response: event=json.load(response)['data']\nprint(json.dumps({'event_id':event['id']}))\n")
        archive = io.BytesIO()
        with tarfile.open(fileobj=archive, mode='w:gz') as tar:
            tar.add(pack, arcname=pack_ref)
        api('POST', '/packs/upload', files={'pack': ('pack.tar.gz', archive.getvalue(), 'application/gzip')})
        assert api('GET', '/actions/' + action_ref)['is_adhoc'] is False
        result['key_ref'], result['unrelated_ref'], result['pack_ref'] = *keys, pack_ref
        result['key_owner_type']='system'
        result['signer_ref']=signer['ref']

        # The positive requester covers precisely the selected system-Key grant,
        # without administrator or installer authority.
        caller_login=run+'-executor@example.test'
        caller_identity=api('POST','/identities',json={'login':caller_login,'password':'TestPass123!','display_name':'Narrow execution caller'})
        for ref in [pack_ref+'.invoke',pack_ref+'.read_system',pack_ref+'.sign_system']:
            api('POST','/permissions/assignments',json={'identity_id':caller_identity['id'],'permission_set_ref':ref})
        caller_auth=requests.post(base+'/auth/login',json={'login':caller_login,'password':'TestPass123!'},timeout=20)
        assert caller_auth.status_code==200
        caller=requests.Session()
        caller.headers['Authorization']='Bearer '+caller_auth.json()['data']['access_token']

        def execute(refs, plaintext, reference=action_ref, fail_once=False, parameters=None, requester=None):
            parameters = parameters if parameters is not None else {'key': keys[0], 'other': keys[1], 'plaintext': plaintext, 'fail_once':fail_once,'marker':run}
            response=(requester or admin).post(base+'/api/v1/executions/execute',json={'action_ref':reference,'permission_set_refs':refs,'parameters':parameters},timeout=30)
            assert response.status_code==201, 'execution admission rejected: HTTP '+str(response.status_code)+' '+response.text[:1000]
            ex=response.json()['data']
            def terminal():
                if any(proc.poll() is not None for proc in processes):
                    raise RuntimeError('A production service exited; inspect service logs')
                row = api('GET', '/executions/' + str(ex['id']))
                return row if row['status'] in ('completed', 'failed', 'timeout', 'cancelled') else None
            row = wait_for(terminal, 90, 'action process completion')
            assert row['status'] == 'completed', 'action failed: ' + str(row['result'])
            return row

        positive = execute(['standard', pack_ref + '.read_system'], True, requester=caller)
        assert psql(f"SELECT executor FROM execution WHERE id={positive['id']}")==str(caller_identity['id'])
        assert secret not in json.dumps(positive)
        assert psql(f"SELECT mode FROM execution_authority_selection WHERE execution={positive['id']}") == 'delegated'
        assert psql(f"SELECT NOT (executable_snapshot->'executable'->'action'->>'is_adhoc')::boolean FROM execution WHERE id={positive['id']}")=='t'
        result['delegated_selection']={'execution':positive['id'],'requester':caller_identity['id'],'action_ref':action_ref,'is_adhoc':False,'mode':'delegated','permission_set_refs':positive['permission_set_refs']}

        assert int(psql(f"SELECT count(*) FROM runtime_authority_admission WHERE owner_kind='execution' AND owner_id={positive['id']}")) == 0
        assert int(psql(f"SELECT count(*) FROM delegated_execution_key_origin WHERE execution={positive['id']}")) == 1
        assert psql(f"SELECT origin->>'key_ref' FROM delegated_execution_key_origin WHERE execution={positive['id']}")==keys[0]
        opened = api('GET', f"/executions/{positive['id']}?include_secret_values=true")
        data = opened['result']['data']
        positive_logs=json.loads(psql(f"SELECT json_agg(json_build_object('artifact',artifact,'version',version,'encoding',meta->>'log_encoding')) FROM artifact_version WHERE execution={positive['id']}"))
        assert len(positive_logs)==2 and all(log['encoding']=='attune_encrypted_log_v1' for log in positive_logs)
        log_paths=[f"/api/v1/artifacts/{log['artifact']}/versions/{log['version']}/download" for log in positive_logs]
        log_paths += [f"/api/v1/artifacts/{log['artifact']}/download" for log in positive_logs]
        result['protected_log_paths']=log_paths
        for path in log_paths:
            revealed=admin.get(base+path,timeout=15)
            assert revealed.status_code==200 and secret in revealed.text, 'authorized protected log download failed'
        result['checks'] += ['dynamic Key stdout/stderr encrypted and authorized log downloads recover original bytes']
        assert data['value'] == secret and data['unrelated_denied']
        payload=data['credential'].split('.')[1]
        credential_claims=json.loads(base64.urlsafe_b64decode(payload+'='*(-len(payload)%4)))
        assert credential_claims['exp']>time.time()+5, 'credential must remain unexpired for lifecycle denials'
        assert credential_claims['metadata']['delegated_attempt']==psql(f"SELECT attempt::text FROM delegated_execution_attempt WHERE execution={positive['id']}")
        stale = requests.get(base + '/api/v1/keys/' + keys[0], headers={'Authorization': 'Bearer ' + data['credential']}, timeout=10)
        assert stale.status_code == 401, 'terminal credential accepted'
        # Reproduce the legitimate closure-to-terminal status lag using the
        # already completed owned fixture. The real process credential must
        # stay invalid while the attempt is closed and its row says running.
        assert psql(f"SELECT closed FROM delegated_execution_attempt WHERE execution={positive['id']}")=='t'
        psql(f"UPDATE execution SET status='running' WHERE id={positive['id']}")
        try:
            assert psql(f"SELECT status FROM execution WHERE id={positive['id']}")=='running'
            closed=requests.get(base+'/api/v1/keys/'+keys[0],headers={'Authorization':'Bearer '+data['credential']},timeout=10)
            assert closed.status_code==401, 'closed attempt credential accepted while source row remains running'
        finally:
            psql(f"UPDATE execution SET status='completed' WHERE id={positive['id']}")
        result['checks'] += ['closed attempt credential denied while source execution still says running']
        result['credential_fencing']={'execution':positive['id'],'terminal_status':stale.status_code,'closed_running_status':closed.status_code,'unexpired':True,'attempt_matches':True}

        result['checks'] += ['narrow requester managed system-Key read+decrypt through API/broker/production worker/process', 'unrelated system Key denied', 'terminal credential denied', 'runtime origin retained', 'default result redacted']
        for refs in [['standard'], ['standard', pack_ref + '.use_system'], ['standard', pack_ref + '.read_metadata']]:
            execute(refs, False)
        result['checks'] += ['missing selected grant denies plaintext', 'keys:use alone denies plaintext', 'keys:read without decrypt denies plaintext']
        signed=execute([pack_ref+'.sign_system'],False,reference=signer_ref,parameters={'signer':signer['ref']})
        signed_data=api('GET',f"/executions/{signed['id']}?include_secret_values=true")['result']['data']
        assert signed_data['raw_denied'] and signed_data['profile_denied'] and len(signed_data['assertion'].split('.'))==3
        assert 'private_key_pem' not in json.dumps(signed_data)
        header,claims,signature=signed_data['assertion'].split('.')
        decoded_header=json.loads(base64.urlsafe_b64decode(header+'='*(-len(header)%4)))
        decoded_claims=json.loads(base64.urlsafe_b64decode(claims+'='*(-len(claims)%4)))
        assert decoded_header['alg']=='RS256'
        assert decoded_claims['iss']=='approved-client' and decoded_claims['aud']=='https://auth.example.test' and decoded_claims['sub']=='approved@example.test'
        assert decoded_claims['exp']>time.time() and decoded_claims['exp']-decoded_claims['iat']==60
        public_key=evidence/'signer-public.pem'
        signature_file=evidence/'approved-signature.bin'
        public_key.write_bytes(command(['openssl','pkey','-pubout','-in',str(ROOT/'crates/common/tests/fixtures/jwt_signing_test_private.pem')],capture_output=True).stdout)
        signature_file.write_bytes(base64.urlsafe_b64decode(signature+'='*(-len(signature)%4)))
        command(['openssl','dgst','-sha256','-verify',str(public_key),'-signature',str(signature_file)],input=(header+'.'+claims).encode(),capture_output=True)
        result['approved_signature_verified']=True

        result['checks'] += ['keys:use-only approved signer succeeds without private material; unapproved subject denied']
        # Native workflow fanout and failed-attempt retry retain delegated mode.
        workflow=execute(['standard',pack_ref+'.read_system'],True,reference=flow_ref,fail_once=True)
        children=int(psql(f"SELECT count(*) FROM execution WHERE parent={workflow['id']}"))
        assert children>=2, 'workflow did not create a retry child'
        assert psql(f"SELECT bool_and(s.mode='delegated') AND count(DISTINCT e.executable_snapshot::text)=1 FROM execution e JOIN execution_authority_selection s ON s.execution=e.id WHERE e.parent={workflow['id']}")=='t'
        assert int(psql(f"SELECT count(*) FROM execution WHERE parent={workflow['id']} AND status='failed'"))>=1
        assert secret not in json.dumps(workflow)
        result['checks'] += ['workflow fanout and retry preserve delegated mode and executable snapshot']
        tokenless=execute([],False,reference=tokenless_ref,parameters={'message':'flat stdin input'})
        assert tokenless['result']['data']=={'token_present':False,'message':'flat stdin input'}
        result['checks'] += ['tokenless selection preserves flat input and completion without an API token']
        adhoc_ref=pack_ref+'.adhoc_read'
        adhoc=api('POST','/actions',json={'ref':adhoc_ref,'pack_ref':pack_ref,'label':'Ordinary ad hoc','entrypoint':'read_system.py','runtime_ref':pack_ref+'.python','param_schema':{'key':{'type':'string','required':True},'other':{'type':'string','required':True},'plaintext':{'type':'boolean','required':True},'fail_once':{'type':'boolean'},'marker':{'type':'string'}},})
        assert adhoc['is_adhoc'] is True
        execute(['standard',pack_ref+'.read_system'],True,reference=adhoc_ref)
        result['checks'] += ['ordinary API-created ad hoc execution remains supported']
        rule_ref=pack_ref+'.delegated_rule'
        rule=api('POST','/rules',json={'ref':rule_ref,'pack_ref':pack_ref,'label':'Delegated rule selection','trigger_ref':event_ref,'action_ref':action_ref,'permission_set_refs':['standard',pack_ref+'.read_system'],'action_params':{'key':keys[0],'other':keys[1],'plaintext':True}})
        assert rule['is_adhoc'] is True
        execute([emitter_ref],False,reference=emitter_ref,parameters={'trigger':event_ref,'marker':run})
        rule_execution=wait_for(lambda: psql("SELECT x.id FROM execution x JOIN enforcement e ON e.id=x.enforcement WHERE e.rule_ref='"+rule_ref+"' AND x.status='completed' LIMIT 1"),60,'delegated rule execution')
        rule_result=api('GET',f'/executions/{rule_execution}?include_secret_values=true')
        assert rule_result['result']['data']['value']==secret
        assert psql("SELECT count(*) FROM delegated_enforcement_selection s JOIN enforcement e ON e.id=s.enforcement WHERE e.rule_ref='"+rule_ref+"'")=='1'
        result['checks'] += ['delegated event-to-rule-to-worker execution retains match-time selection and protected system-Key results']


        # Ordinary queues use their attributed selected refs; managed sponsorship
        # remains unavailable, even for tokenless executions.
        queue_ref=pack_ref+'.ordinary_queue'
        queue=api('POST','/queues',json={'ref':queue_ref,'pack_ref':pack_ref,'label':'Delegated queue','dispatch_action_ref':tokenless_ref,'permission_set_refs':[],'action_params':{'message':'queued flat input','item_id':'{{ queue_item.id }}'}})
        assert queue['is_adhoc'] is True
        api('POST','/queues/'+queue_ref+'/items',json={'payload':{}})
        def queue_completed():
            value=psql("SELECT execution FROM work_queue_dispatch WHERE queue_ref='"+queue_ref+"' ORDER BY id DESC LIMIT 1")
            if not value:
                return None
            row=api('GET','/executions/'+value)
            return row if row['status'] in ('completed','failed','timeout','cancelled') else None
        queued=wait_for(queue_completed,90,'delegated queue action completion')
        assert queued['status']=='completed' and queued['result']['data']['token_present'] is False and queued['result']['data']['message']=='queued flat input', 'queued action failed: '+json.dumps(queued['result'])
        wait_for(lambda: psql("SELECT status::text FROM work_queue_item WHERE queue_ref='"+queue_ref+"'")=="completed",30,'queue acknowledgement')
        assert psql(f"SELECT mode FROM execution_authority_selection WHERE execution={queued['id']}")=='delegated'
        sponsored=admin.post(base+'/api/v1/queues/'+managed_queue_ref+'/items',json={'payload':{}},timeout=20)
        assert sponsored.status_code==501 and 'readiness' in sponsored.text, 'recorded queue did not report readiness separately'
        assert int(psql("SELECT count(*) FROM work_queue_item WHERE queue_ref='"+managed_queue_ref+"'"))==0
        result['checks'] += ['attributed delegated queue completes; recorded-only managed queue rejects before enqueue with readiness diagnostic']

        # Protected queue results expose only validated control effects. Opaque
        # summary/error values remain solely in the encrypted execution result.
        for invalid in [False, True]:
            private_queue_ref=pack_ref+('.invalid_ack_queue' if invalid else '.private_ack_queue')
            api('POST','/queues',json={'ref':private_queue_ref,'pack_ref':pack_ref,'label':'Protected acknowledgement','dispatch_action_ref':action_ref,'permission_set_refs':['standard',pack_ref+'.read_system'],'action_params':{'key':keys[0],'other':keys[1],'plaintext':True,'item_id':'{{ queue_item.id }}','ack_invalid':invalid}})
            accepted=caller.post(base+'/api/v1/queues/'+private_queue_ref+'/items',json={'payload':{}},timeout=30)
            assert accepted.status_code==201, 'narrow queue admission rejected: HTTP '+str(accepted.status_code)
            def private_queue_terminal():
                state=psql("SELECT status::text FROM work_queue_item WHERE queue_ref='"+private_queue_ref+"'")
                return state if state in ('completed','failed','cancelled') else None
            status=wait_for(private_queue_terminal,90,'protected queue acknowledgement')
            assert status==('failed' if invalid else 'completed'), 'protected queue acknowledgement state mismatch'
            ordinary=psql("SELECT json_build_object('summary',ack_summary,'error',last_error)::text FROM work_queue_item WHERE queue_ref='"+private_queue_ref+"'")
            assert secret not in ordinary, 'private acknowledgement disclosed through queue fields'
            ordinary=json.loads(ordinary)
            if invalid:
                assert ordinary['error']['code']=='queue_ack_invalid' and ordinary['error']['message']=='Protected queue acknowledgement is invalid'
            else:
                assert ordinary['summary']=={'status':'completed'}
            queue_execution=psql("SELECT execution FROM work_queue_dispatch WHERE queue_ref='"+private_queue_ref+"' ORDER BY id DESC LIMIT 1")
            assert psql('SELECT executor FROM execution WHERE id='+queue_execution)==str(caller_identity['id'])
            assert int(psql("SELECT count(*) FROM execution_secret_value WHERE entity_type='execution_result' AND entity_id="+queue_execution+" AND json_path='/queue_ack'"))==1
            protected=api('GET','/executions/'+queue_execution+'?include_secret_values=true')['result']['queue_ack']
            assert protected['items'][0]['summary']['value']==secret and protected['items'][0]['error']['value']==secret
        result['checks'] += ['API-capable system-Key queue consumes closed-attempt acknowledgement controls while freeform details remain encrypted', 'malformed protected acknowledgement fails without leaking parser input through queue errors']

        for private in [False, True]:
            native_queue_ref = pack_ref + ('.native_private_queue' if private else '.native_public_queue')
            params = ({'key':keys[0],'other':keys[1],'plaintext':True,'item_id':'{{ queue_item.id }}'}
                      if private else {'message':'native public input','item_id':'{{ queue_item.id }}'})
            api('POST','/queues',json={'ref':native_queue_ref,'pack_ref':pack_ref,'label':'Native workflow controls','dispatch_action_ref':pack_ref+('.private_ack_flow' if private else '.public_ack_flow'),'permission_set_refs':['standard',pack_ref+'.read_system'] if private else [],'action_params':params})
            api('POST','/queues/'+native_queue_ref+'/items',json={'payload':{}})
            def native_queue_terminal():
                state=psql("SELECT status::text FROM work_queue_item WHERE queue_ref='"+native_queue_ref+"'")
                return state if state in ('completed','failed','cancelled') else None
            status=wait_for(native_queue_terminal,90,'native workflow queue acknowledgement')
            assert status=='completed', 'native workflow queue failed: '+psql("SELECT coalesce(last_error::text,'') FROM work_queue_item WHERE queue_ref='"+native_queue_ref+"'")
            native_execution=psql("SELECT execution FROM work_queue_dispatch WHERE queue_ref='"+native_queue_ref+"' ORDER BY id DESC LIMIT 1")
            assert psql("SELECT a.producer_kind='workflow' AND a.closed AND a.worker IS NULL AND e.worker IS NULL AND e.status='completed' FROM execution e JOIN delegated_execution_attempt a ON a.execution=e.id WHERE e.id="+native_execution)=='t'
            ordinary=psql("SELECT json_build_object('summary',ack_summary,'error',last_error)::text FROM work_queue_item WHERE queue_ref='"+native_queue_ref+"'")
            assert secret not in ordinary
            if private:
                ordinary_result=api('GET','/executions/'+native_execution)
                assert secret not in json.dumps(ordinary_result)
                restored=api('GET','/executions/'+native_execution+'?include_secret_values=true')['result']['queue_ack']
                assert restored['items'][0]['summary']['value']==secret and restored['items'][0]['error']['value']==secret
                assert json.loads(ordinary)['summary']=={'status':'completed'}
                assert int(psql("SELECT count(*) FROM execution_secret_value WHERE entity_type='execution_result' AND entity_id="+native_execution))>0
            result['checks'] += ['native '+('protected system-Key' if private else 'tokenless public')+' workflow queue output_map completes with a closed noncredential producer fence']

        # A principal with only execute/observe rights cannot borrow pack sponsorship.
        login_name = run + '@example.test'
        identity = api('POST', '/identities', json={'login': login_name, 'password': 'TestPass123!', 'display_name': run})
        api('POST', '/permissions/assignments', json={'identity_id': identity['id'], 'permission_set_ref': pack_ref + '.invoke'})
        login = requests.post(base + '/auth/login', json={'login': login_name, 'password': 'TestPass123!'}, timeout=20)
        assert login.status_code == 200, 'restricted login failed'
        restricted = requests.Session()
        restricted.headers['Authorization'] = 'Bearer ' + login.json()['data']['access_token']
        denied = restricted.post(base + '/api/v1/executions/execute', json={'action_ref': action_ref, 'permission_set_refs': ['standard', pack_ref + '.read_system'], 'parameters': {'key': keys[0], 'other': keys[1], 'plaintext': True}}, timeout=15)
        assert denied.status_code == 403, 'uncovered named grants were admitted'
        assert int(psql(f"SELECT count(*) FROM execution WHERE executor={identity['id']}")) == 0
        # A signer output retains Use provenance even though private material
        # was never returned. This selection has no standard/read grants that
        # could otherwise mask the approved-operation disclosure boundary.
        hidden_signer=restricted.get(base+f"/api/v1/executions/{signed['id']}?include_secret_values=true",timeout=15)
        assert hidden_signer.status_code in (200,403,404) and signed_data['assertion'] not in hidden_signer.text
        result['checks'] += ['missing keys:use cannot disclose approved-operation output']
        # Visibility and entity decrypt rights alone cannot reveal the source Key.
        visible = restricted.get(base + f"/api/v1/executions/{positive['id']}", timeout=15)
        assert visible.status_code == 200, 'restricted observer lacks execution visibility'
        observed = restricted.get(base + f"/api/v1/executions/{positive['id']}?include_secret_values=true", timeout=15)
        assert observed.status_code in (200, 403, 404)
        assert secret not in observed.text
        for path in log_paths:
            hidden=restricted.get(base+path,timeout=15)
            assert hidden.status_code in (403,404) and secret not in hidden.text, 'missing Key origin rights disclosed logs'
        result['checks'] += ['missing Key origin rights deny artifact log download']
        result['checks'] += ['execute-only caller rejected before execution creation', 'missing Key origin rights cannot disclose result']
        # Dispatch rejects a worker that does not advertise the protected-output path.
        psql("UPDATE worker SET capabilities=jsonb_set(capabilities,'{delegated_execution_authority_v1}','false') WHERE name='"+run+"'")
        blocked=api('POST','/executions/execute',json={'action_ref':tokenless_ref,'permission_set_refs':[],'parameters':{'message':'must not dispatch'}})
        def blocked_terminal():
            row=api('GET','/executions/'+str(blocked['id']))
            return row if row['status'] in ('failed','completed','cancelled','timeout') else None
        rejected=wait_for(blocked_terminal,30,'incompatible worker rejection')
        assert rejected['status']=='failed' and rejected.get('worker') is None
        assert int(psql(f"SELECT count(*) FROM delegated_execution_attempt WHERE execution={blocked['id']}"))==0
        psql("UPDATE worker SET capabilities=jsonb_set(capabilities,'{delegated_execution_authority_v1}','true') WHERE name='"+run+"'")
        result['checks'] += ['worker without delegated completion capability rejected before dispatch']
        for path in (evidence / 'artifacts_dir').rglob('*'):
            if path.is_file():
                assert secret.encode() not in path.read_bytes(), 'plaintext in artifact storage'
        for log in logs:
            log.flush()
            assert secret not in Path(log.name).read_text(errors='replace'), 'runtime mirroring leaked Key'
        result['checks'] += ['logs/results have no plaintext file copies', 'runtime stdout/stderr mirroring suppressed']
        result['status'] = 'passed'
    except BaseException as error:
        result['status'] = 'failed'
        result['error'] = str(error)
        raise
    finally:
        failures = []
        for proc in reversed(processes):
            if proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=30)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait(timeout=10)
        for log in logs:
            log.close()
        for name in reversed(containers):
            with (evidence / (name + '.log')).open('w') as log:
                subprocess.run(['docker', 'logs', name], stdout=log, stderr=subprocess.STDOUT, timeout=20)
            cleanup = subprocess.run(['docker', 'rm', '-f', '-v', name], stdout=subprocess.DEVNULL, timeout=60)
            if cleanup.returncode:
                failures.append(name)
        if network:
            cleanup=subprocess.run(['docker','network','rm',network],stdout=subprocess.DEVNULL,timeout=60)
            if cleanup.returncode:
                failures.append(network)
        result['cleanup_failures'] = failures
        (evidence / 'result.json').write_text(json.dumps(result, indent=2))
        print(json.dumps({'status': result.get('status'), 'checks': result['checks'], 'evidence': str(evidence), 'cleanup_failures': failures}))
        if failures:
            raise RuntimeError('Owned infrastructure cleanup failed')


if __name__ == '__main__':
    main()
