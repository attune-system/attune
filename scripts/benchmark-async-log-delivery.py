#!/usr/bin/env python3
"""Paired native-service / Docker-owned RustFS log latency and recovery workload."""
import argparse
import hashlib
import re
import os, sys, json, time, uuid, signal, shutil, socket, subprocess, urllib.request, urllib.error
from pathlib import Path
import yaml
ROOT=Path(__file__).resolve().parents[1]
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output',type=Path,required=True,help='New private output directory; must not already exist')
parser.add_argument('--bin-dir',type=Path,default=ROOT/'target/release')
parser.add_argument('--timer-bin',type=Path,default=ROOT/'target/core-timer/debug/attune-core-timer-sensor')
parser.add_argument('--seconds',type=int,default=120)
args=parser.parse_args()
if not 30<=args.seconds<=600:parser.error('--seconds must be between 30 and 600')
OUT=args.output.resolve();OUT.mkdir(parents=True,mode=0o700)

TOKEN=uuid.uuid4().hex[:10]
PROJECT='attune-asynclog-'+TOKEN
BIN=args.bin_dir.resolve()
procs=[]
(OUT/'owner.json').write_text(json.dumps({'project':PROJECT,'source':str(ROOT),'base_revision':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip()}))
source_files={}
for entry in subprocess.check_output(['git','status','--porcelain=v1','-z'],cwd=ROOT).split(b'\0'):
    if entry:
        relative=entry[3:].decode();source=ROOT/relative
        if source.is_file():source_files[relative]=hashlib.sha256(source.read_bytes()).hexdigest()
(OUT/'source-sha256.json').write_text(json.dumps(source_files,indent=2))
binary_files=[BIN/('attune-'+name) for name in ('api','executor','worker','sensor','notifier')]+[args.timer_bin.resolve()]
(OUT/'binary-sha256.json').write_text(json.dumps({str(binary):hashlib.sha256(binary.read_bytes()).hexdigest() for binary in binary_files},indent=2))

def run(args,**kw):
    return subprocess.run(args,cwd=ROOT,check=True,timeout=kw.pop('timeout',2400),**kw)
def capture(args):
    return subprocess.check_output(args,cwd=ROOT,text=True,timeout=45).strip()
def compose(*args,**kw):
    return run(['docker','compose','-p',PROJECT,'-f',str(OUT/'compose.yaml'),*args],**kw)
def cid(service):
    return capture(['docker','compose','-p',PROJECT,'-f',str(OUT/'compose.yaml'),'ps','-q',service])
def port(service,internal):
    return int(capture(['docker','port',cid(service),str(internal)]).split(':')[-1])
def unused_port():
    with socket.socket() as s:
        s.bind(('127.0.0.1',0)); return s.getsockname()[1]
def until(fn,seconds,desc):
    end=time.monotonic()+seconds
    while time.monotonic()<end:
        for p in procs:
            if p.poll() is not None: raise RuntimeError(f'service exited: {p.args}, code {p.returncode}')
        try:
            value=fn()
            if value:return value
        except (OSError,urllib.error.URLError,subprocess.CalledProcessError):pass
        time.sleep(.5)
    raise TimeoutError(desc)
def pg(sql):
    return capture(['docker','exec','-e','PGAPPNAME=echo-validation-monitor',cid('postgres'),'psql','-X','-U','attune','-d','attune','-At','-v','ON_ERROR_STOP=1','-c','SET search_path TO attune,public; '+sql]).removeprefix('SET\n')
def request(path,data=None,token=None,method=None):
    headers={'Content-Type':'application/json'}
    if token:headers['Authorization']='Bearer '+token
    req=urllib.request.Request(BASE+path,data=json.dumps(data).encode() if data is not None else None,headers=headers,method=method)
    try:
        with urllib.request.urlopen(req,timeout=30) as r:return json.load(r)
    except urllib.error.HTTPError as e:
        raise RuntimeError(f'{path}: {e.code}: {e.read().decode()[:2000]}')
def start(name,config,extra=None):
    if name=='worker':
        worker_values=yaml.safe_load(config.read_text())
        worker_values['artifacts_dir']=str(OUT/'worker-artifacts')
        config=OUT/'worker-config.yaml'
        config.write_text(yaml.safe_dump(worker_values));config.chmod(0o600)
    env={k:os.environ[k] for k in ('PATH','HOME','LANG') if k in os.environ}
    env.update(ATTUNE__ENVIRONMENT='docker',ATTUNE_CONFIG=str(config),ATTUNE_API_URL=BASE,ATTUNE_MQ_URL=MQ,AWS_ACCESS_KEY_ID='attune-local-key',AWS_SECRET_ACCESS_KEY='attune-local-secret-only',AWS_DEFAULT_REGION='us-east-1',RUST_BACKTRACE='1')
    if extra:env.update(extra)
    log=open(OUT/(name+'.log'),'w')
    p=subprocess.Popen([str(BIN/('attune-'+name))],cwd=ROOT,env=env,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
    procs.append(p)
    return p
manifest={'services':{
 'postgres':{'image':'postgres:18-alpine','environment':{'POSTGRES_USER':'attune','POSTGRES_PASSWORD':'attune','POSTGRES_DB':'attune'},'ports':['127.0.0.1::5432'],'command':['postgres','-c','max_connections=200','-c','log_connections=on','-c','log_disconnections=on','-c','log_line_prefix=%m [%p] %a '],'volumes':['pgdata:/var/lib/postgresql']},
 'rabbitmq':{'image':'rabbitmq:4.3.6-management-alpine','environment':{'RABBITMQ_DEFAULT_USER':'attune','RABBITMQ_DEFAULT_PASS':'attune','RABBITMQ_SERVER_ADDITIONAL_ERL_ARGS':'+S 2:2'},'ports':['127.0.0.1::5672']},
 'rustfs':{'image':'docker.io/rustfs/rustfs:1.0.0@sha256:8cc9801755448b71a786705ce76692c77e14936cccd87cf2fc31842e58f4d1ff','environment':{'TOKIO_WORKER_THREADS':'4','RUSTFS_ACCESS_KEY':'attune-local-key','RUSTFS_SECRET_KEY':'attune-local-secret-only'},'ports':['127.0.0.1::9000'],'volumes':['s3data:/data']},
 'aws':{'image':'docker.io/amazon/aws-cli:2.31.26@sha256:cf1851fa3162c35009b2dc6d2df2797e5b0e9723fe546f545c9fa34a3dc03477','profiles':['tools'],'environment':{'AWS_ACCESS_KEY_ID':'attune-local-key','AWS_SECRET_ACCESS_KEY':'attune-local-secret-only','AWS_DEFAULT_REGION':'us-east-1'},'entrypoint':['aws','--endpoint-url','http://rustfs:9000']},
 'migrations':{'image':'postgres:18-alpine','profiles':['tools'],'environment':{'DB_HOST':'postgres','DB_USER':'attune','DB_PASSWORD':'attune','DB_NAME':'attune'},'volumes':[f'{ROOT}/migrations:/migrations:ro',f'{ROOT}/docker/run-migrations.sh:/run-migrations.sh:ro',f'{ROOT}/scripts/seed-standard-pack-index.sh:/seed-standard-pack-index.sh:ro',f'{ROOT}/docker/init-roles.sql:/docker/init-roles.sql:ro'],'entrypoint':['/bin/sh','/run-migrations.sh']}
},'volumes':{'pgdata':{},'s3data':{}}}
(OUT/'compose.yaml').write_text(yaml.safe_dump(manifest))
owned=False
try:
    for kind in [('container',['ps','-a']),('network',['network','ls']),('volume',['volume','ls'])]:
        existing=capture(['docker',*kind[1],'--filter','label=com.docker.compose.project='+PROJECT,'--format','{{.ID}}' if kind[0]!='volume' else '{{.Name}}'])
        if existing:raise RuntimeError('Refusing pre-existing project resources: '+PROJECT)
    owned=True
    print('Starting owned infrastructure:',PROJECT,flush=True)
    compose('up','-d','postgres','rabbitmq','rustfs',timeout=240)
    until(lambda: subprocess.run(['docker','exec',cid('postgres'),'pg_isready','-U','attune'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode==0,60,'postgres readiness')
    until(lambda: subprocess.run(['docker','exec','--user','rabbitmq',cid('rabbitmq'),'rabbitmq-diagnostics','-q','ping'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode==0,180,'rabbit readiness')
    until(lambda: subprocess.run(['docker','compose','-p',PROJECT,'-f',str(OUT/'compose.yaml'),'run','--rm','aws','s3api','list-buckets'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode==0,90,'RustFS readiness')
    compose('run','--rm','aws','s3api','create-bucket','--bucket','attune-echo-validation')
    compose('run','--rm','aws','s3api','put-bucket-versioning','--bucket','attune-echo-validation','--versioning-configuration','Status=Enabled')
    compose('run','--rm','migrations')
    packs=OUT/'source-packs'; shutil.copytree(ROOT/'packs/core',packs/'core',dirs_exist_ok=True)
    timer=packs/'core/sensors/attune-core-timer-sensor'
    shutil.copy2(args.timer_bin.resolve(),timer)
    run(['strip',str(timer)])
    apiport=unused_port(); wsport=unused_port(); BASE=f'http://127.0.0.1:{apiport}'
    MQ=f'amqp://attune:attune@127.0.0.1:{port("rabbitmq",5672)}'
    config=yaml.safe_load((ROOT/'docker/distributable/config.docker.yaml').read_text())
    config['environment']='development'
    config['database'].update(url=f'postgresql://attune:attune@127.0.0.1:{port("postgres",5432)}/attune',max_connections=20,min_connections=5)
    config['message_queue']['url']=MQ
    config['server'].update(host='127.0.0.1',port=apiport,max_body_size=268435456)
    config['notifier'].update(host='127.0.0.1',port=wsport)
    config.pop('agent',None)
    config['security'].update(jwt_secret='local-echo-validation-jwt-secret-only',encryption_key='local-echo-validation-encryption-key-32-bytes',allow_self_registration=True)
    config['packs_base_dir']=str(OUT/'admitted-packs'); config['packs']['transport']='volume'
    config['runtime_envs_dir']=str(OUT/'runtime-envs')
    config['artifacts_dir']=str(OUT/'api-artifacts'); config['artifacts']['transport']='api'
    config['storage']={'provider':'s3','bucket':'attune-echo-validation','region':'us-east-1','prefix':'echo-run','endpoint':f'http://127.0.0.1:{port("rustfs",9000)}','kms_key':None}
    config['sensor'].update(notifier_ws_url=f'ws://127.0.0.1:{wsport}/ws',worker_name='echo-validation-sensor')
    config['worker'].update(name='echo-validation-worker',max_concurrent_tasks=4,log_delivery={'enabled':False})
    config['log'].update(format='json',level='info')
    cfg=OUT/'config.yaml'; cfg.write_text(yaml.safe_dump(config));cfg.chmod(0o600)
    print('Starting current-source services at',BASE,flush=True)
    start('api',cfg)
    until(lambda: request('/health/ready'),120,'API readiness')
    run(['docker','cp',str(ROOT/'docker/init-user.sh'),cid('postgres')+':/tmp/init-user.sh'])
    run(['docker','exec','-e','DB_HOST=127.0.0.1',cid('postgres'),'sh','/tmp/init-user.sh'])
    access=request('/auth/login',{'login':'test@attune.local','password':'TestPass123!'})['data']['access_token']
    sys.path.insert(0,str(ROOT/'scripts')); import bootstrap_core_pack
    def publication_request(url, **kwargs):
        return urllib.request.build_opener(bootstrap_core_pack.NoApiRedirects()).open(urllib.request.Request(url, **kwargs), timeout=180)
    bootstrap_core_pack.request = publication_request
    bootstrap_core_pack.upload(BASE,access,packs/'core')
    start('notifier',cfg); start('executor',cfg); worker_process=start('worker',cfg)
    sensorconfig=dict(config); sensorconfig['artifacts_dir']=str(OUT/'sensor-artifacts')
    sensorcfg=OUT/'sensor-config.yaml'; sensorcfg.write_text(yaml.safe_dump(sensorconfig));sensorcfg.chmod(0o600); start('sensor',sensorcfg)
    until(lambda: int(pg("SELECT count(*) FROM worker WHERE worker_role::text='action' AND status::text='active'")) > 0 and int(pg("SELECT count(*) FROM worker WHERE worker_role::text='sensor' AND status::text='active'")) > 0,120,'workers and sensor registration')
    rule=request('/api/v1/rules',{'ref':'core.echo_connection_validation','pack_ref':'core','label':'Echo connection validation every second','description':'Run-owned connection reuse validation','action_ref':'core.echo','trigger_ref':'core.intervaltimer','conditions':{},'action_params':{'message':'connection-reuse-validation'},'trigger_params':{'unit':'seconds','interval':1},'enabled':True},access)['data']
    print('Rule active:',rule['id'],flush=True)
    until(lambda:int(pg("SELECT count(*) FROM execution WHERE action_ref='core.echo' AND status::text='completed'"))>=10,180,'ten completed warm-up echoes')
    def snapshot():
        return json.loads(pg("SELECT json_build_object('time',clock_timestamp(),'sessions',(SELECT coalesce(json_agg(json_build_object('pid',pid,'application_name',application_name,'backend_start',backend_start,'state',state,'wait_event_type',wait_event_type,'wait_event',wait_event)),'[]') FROM pg_stat_activity WHERE application_name LIKE 'attune:%'),'executions',(SELECT count(*) FROM execution WHERE action_ref='core.echo'),'completed',(SELECT count(*) FROM execution WHERE action_ref='core.echo' AND status::text='completed'),'failed',(SELECT count(*) FROM execution WHERE action_ref='core.echo' AND status::text IN ('failed','error','timeout')),'logs',(SELECT coalesce(json_agg(row_to_json(x)),'[]') FROM (SELECT backend::text,sealed,count(*) FROM log_stream GROUP BY backend,sealed) x))"))
    # Same optimized binaries, infrastructure, rule and retention settings in
    # both phases; only the worker handoff setting changes.
    import datetime, math, statistics, threading, http.server, http.client
    cohorts={}
    def collect(mode):
        print('Warming',mode,'for 30 seconds',flush=True)
        warmend=time.monotonic()+30
        while time.monotonic()<warmend:snapshot();time.sleep(1)
        start=pg("SELECT clock_timestamp()")
        samples=[];began=time.monotonic()
        duration=args.seconds
        print('Measuring',mode,duration,'seconds',flush=True)
        while time.monotonic()-began<duration:
            samples.append(snapshot());time.sleep(max(0,1-(time.monotonic()-began)%1))
        end=pg("SELECT clock_timestamp()")
        request('/api/v1/rules/'+rule['ref'],{'enabled':False},access,'PUT')
        until(lambda:int(pg("SELECT count(*) FROM execution WHERE action_ref='core.echo' AND status::text NOT IN ('completed','failed','cancelled','timeout','abandoned')"))==0,90,'execution drain')
        until(lambda:int(pg("SELECT count(*) FROM log_stream s JOIN artifact_version v ON v.id=s.artifact_version JOIN execution e ON e.id=v.execution WHERE e.action_ref='core.echo' AND NOT s.sealed"))==0,75,'log delivery drain')
        rows=json.loads(pg("SELECT coalesce(json_agg(row_to_json(x)),'[]') FROM (SELECT e.id,e.status::text,e.result,e.started_at,e.updated, (SELECT min(time) FROM execution_history h WHERE h.entity_id=e.id AND h.new_values->>'status'='running') running_at,(SELECT min(time) FROM execution_history h WHERE h.entity_id=e.id AND h.new_values->>'status'='completed') completed_at,(SELECT max(s.sealed_at) FROM artifact_version v JOIN log_stream s ON s.artifact_version=v.id WHERE v.execution=e.id) sealed_at,(SELECT count(*) FROM artifact_version v JOIN log_stream s ON s.artifact_version=v.id WHERE v.execution=e.id AND s.backend='object_segments' AND s.sealed) sealed_streams FROM execution e WHERE e.action_ref='core.echo' AND e.created>='"+start+"'::timestamptz AND e.created<'"+end+"'::timestamptz ORDER BY e.id) x"))
        cohorts[mode]={'start':start,'end':end,'rows':rows,'samples':samples}
        (OUT/(mode+'.json')).write_text(json.dumps(cohorts[mode],indent=2))
        assert len(rows)>=duration-3,(mode,len(rows),duration)
        assert all(r['status']=='completed' and r['sealed_streams']==1 for r in rows),[r for r in rows if r['status']!='completed' or r['sealed_streams']!=1]
        assert all(r['result'].get('exit_code')==0 and not r['result'].get('logs_incomplete',False) for r in rows),rows
    collect('synchronous')
    os.killpg(worker_process.pid,signal.SIGTERM);worker_process.wait(timeout=80);procs.remove(worker_process)
    # Separate logs preserve both timing cohorts across the worker restart.
    (OUT/'worker.log').rename(OUT/'worker-synchronous.log')
    config['worker']['log_delivery']['enabled']=True
    cfg.write_text(yaml.safe_dump(config));cfg.chmod(0o600);worker_process=start('worker',cfg)
    until(lambda:int(pg("SELECT count(*) FROM worker WHERE worker_role='action' AND status='active'"))>0,120,'asynchronous worker readiness')
    request('/api/v1/rules/'+rule['ref'],{'enabled':True},access,'PUT')
    collect('asynchronous')
    def stamp(value):return datetime.datetime.fromisoformat(value.replace('Z','+00:00'))
    def stats(values):
        values=sorted(values)
        return {'n':len(values),'median_ms':statistics.median(values),'p95_ms':values[math.ceil(.95*len(values))-1],'max_ms':max(values)}
    summary={'storage':'RustFS versioned S3, artifacts.transport=api, private persistent worker spool','build':str(BIN),'revision':capture(['git','rev-parse','HEAD']),'cohorts':{}}
    for mode,cohort in cohorts.items():
        pid_sets=[{session['pid'] for session in sample['sessions'] if session['application_name'].startswith('attune:api:main:')} for sample in cohort['samples']]
        cohort['api_connections']={'initial':sorted(pid_sets[0]),'distinct':sorted(set().union(*pid_sets)),'new':sorted(set().union(*pid_sets)-pid_sets[0]),'peak':max(map(len,pid_sets))}
        assert not cohort['api_connections']['new'],('API connection churn',mode,cohort['api_connections'])
        log=OUT/('worker-synchronous.log' if mode=='synchronous' else 'worker.log')
        exits={};settled={};capture_cost={}
        for line in log.read_text().splitlines():
            try:record=json.loads(line)
            except json.JSONDecodeError:continue
            fields=record.get('fields',{});message=fields.get('message','')
            execution=next((span['execution_id'] for span in reversed([*record.get('spans',[]),record.get('span',{})]) if 'execution_id' in span),fields.get('execution_id'))
            completed=re.fullmatch(r'Execution (\d+) completed: success',message)
            if completed:settled[int(completed[1])]=stamp(record['timestamp'])
            if execution is not None and message=='Runtime log finalization finished':capture_cost[int(execution)]=fields['elapsed_ms']
            if message=='Process wait finished':
                execution=next((span['execution_id'] for span in reversed([*record.get('spans',[]),record.get('span',{})]) if 'execution_id' in span),record.get('fields',{}).get('execution_id'))
                if execution is not None:exits[int(execution)]=stamp(record['timestamp'])
        for row in cohort['rows']:
            assert row['id'] in exits,('missing child wait observation',row['id'])
            row['process_exit_at']=str(exits[row['id']])
            assert row['id'] in settled and row['id'] in capture_cost,('missing settlement observation',row['id'])
            row['exit_to_terminal_observed_ms']=(settled[row['id']]-exits[row['id']]).total_seconds()*1000
            row['post_exit_capture_ms']=capture_cost[row['id']]
            row['process_ms']=row['result']['duration_ms']
            row['before_process_estimate_ms']=(exits[row['id']]-stamp(row['running_at'])).total_seconds()*1000-row['process_ms']
            row['post_capture_to_observation_ms']=row['exit_to_terminal_observed_ms']-row['post_exit_capture_ms']
            row['running_to_completed_ms']=(stamp(row['completed_at'])-stamp(row['running_at'])).total_seconds()*1000
            row['exit_to_completed_ms']=(stamp(row['completed_at'])-exits[row['id']]).total_seconds()*1000
            row['exit_to_sealed_ms']=(stamp(row['sealed_at'])-exits[row['id']]).total_seconds()*1000
            row['completed_to_sealed_ms']=(stamp(row['sealed_at'])-stamp(row['completed_at'])).total_seconds()*1000
        summary['cohorts'][mode]={key:stats([r[key] for r in cohort['rows']]) for key in ['running_to_completed_ms','exit_to_completed_ms','exit_to_terminal_observed_ms','post_exit_capture_ms','process_ms','before_process_estimate_ms','post_capture_to_observation_ms','exit_to_sealed_ms','completed_to_sealed_ms']}
        (OUT/(mode+'.json')).write_text(json.dumps(cohort,indent=2))
    summary['api_connections']={mode:cohort['api_connections'] for mode,cohort in cohorts.items()}
    (OUT/'summary.json').write_text(json.dumps(summary,indent=2));print(json.dumps(summary,indent=2),flush=True)
    # Faults use a worker-only HTTP proxy. API/read traffic and RustFS remain
    # real; segment/close requests can be held without stopping execution.
    blocked=threading.Event();blocked.set();drop_seal_reply=threading.Event();drop_seal_version=None
    class Proxy(http.server.BaseHTTPRequestHandler):
        def log_message(self,*args):pass
        def proxy(self):
            length=int(self.headers.get('Content-Length','0'));body=self.rfile.read(length) if length else None
            if blocked.is_set() and self.path.startswith('/api/v1/internal/logs/'):
                self.send_response(503);self.send_header('Content-Length','0');self.end_headers();return
            conn=http.client.HTTPConnection('127.0.0.1',apiport,timeout=30)
            try:
                conn.request(self.command,self.path,body,{k:v for k,v in self.headers.items() if k.lower() not in ('host','connection')})
                response=conn.getresponse();data=response.read()
                if self.path.startswith('/api/v1/internal/logs/'+str(drop_seal_version)+'/seal') and drop_seal_reply.is_set() and response.status==200:
                    drop_seal_reply.clear();self.send_response(503);self.send_header('Content-Length','0');self.end_headers();return
                self.send_response(response.status)
                for key,value in response.getheaders():
                    if key.lower() not in ('connection','transfer-encoding','content-length'):self.send_header(key,value)
                self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
            finally:conn.close()
        do_GET=do_POST=do_PUT=do_HEAD=do_DELETE=proxy
    proxy=http.server.ThreadingHTTPServer(('127.0.0.1',0),Proxy)
    thread=threading.Thread(target=proxy.serve_forever,daemon=True);thread.start()
    def restart_worker(kill=False):
        global worker_process
        os.killpg(worker_process.pid,signal.SIGKILL if kill else signal.SIGTERM);worker_process.wait(timeout=80);procs.remove(worker_process)
        if (OUT/'worker.log').exists():(OUT/'worker.log').rename(OUT/('worker-before-'+uuid.uuid4().hex[:8]+'.log'))
        worker_process=start('worker',cfg,{'ATTUNE_API_URL':f'http://127.0.0.1:{proxy.server_port}'})
        until(lambda:int(pg("SELECT count(*) FROM worker WHERE worker_role='action' AND status='active' AND last_heartbeat>clock_timestamp()-interval '10 seconds'"))>0,120,'fault worker readiness')
    def echo():
        execution=request('/api/v1/executions/execute',{'action_ref':'core.echo','parameters':{'message':'async-log-custody-fault'}},access)['data']['id']
        until(lambda:pg('SELECT status::text FROM execution WHERE id='+str(execution))=='completed',10,'echo completion during held delivery')
        intent=json.loads(pg('SELECT row_to_json(i) FROM (SELECT artifact_version,custody,state,finalize_by FROM log_delivery_intent WHERE execution='+str(execution)+') i'))
        return execution,intent
    def finite_status(version):
        req=urllib.request.Request(BASE+'/api/v1/artifacts/'+str(version['artifact'])+'/versions/'+str(version['version'])+'/download?version_id='+str(version['id']),headers={'Authorization':'Bearer '+access})
        try:
            with urllib.request.urlopen(req,timeout=30) as response:return response.status,response.read().decode()
        except urllib.error.HTTPError as response:return response.code,json.load(response).get('code')
    def version_for(execution):return json.loads(pg('SELECT row_to_json(v) FROM (SELECT id,artifact,version FROM artifact_version WHERE execution='+str(execution)+') v'))
    (OUT/'worker.log').rename(OUT/'worker-asynchronous.log')
    restart_worker()
    execution,intent=echo();version=version_for(execution)
    assert finite_status(version)==(409,'LOG_FINALIZING')
    assert pg('SELECT log_delivery_pinned('+str(version['id'])+'),file_evidence_version_eligible_without_log_delivery('+str(version['id'])+'),file_evidence_version_eligible('+str(version['id'])+')')=='t|t|f','custody must independently retain a pending version before close selection'
    assert pg('SELECT file_evidence_request_cleanup(ARRAY['+str(version['id'])+']::bigint[])')=='{}','the supervisor cleanup admission must honor active custody'
    assert pg("SELECT NOT EXISTS(SELECT 1 FROM pg_constraint WHERE conrelid='log_delivery_intent'::regclass AND contype='f' AND confrelid IN ('execution'::regclass,'worker'::regclass,'identity'::regclass))")=='t','log custody must not pin runtime/identity retention'
    preserved=pg('SELECT result::text FROM execution WHERE id='+str(execution))
    restart_worker(kill=True);blocked.clear()
    until(lambda:pg('SELECT body_state::text FROM artifact_version WHERE id='+str(version['id']))=='ready',30,'spool recovery and seal')
    assert pg('SELECT result::text FROM execution WHERE id='+str(execution))==preserved
    assert 'async-log-custody-fault' in finite_status(version)[1]
    print('Persistent-spool restart recovery passed',flush=True)
    blocked.set();execution,intent=echo();version=version_for(execution);preserved=pg('SELECT result::text FROM execution WHERE id='+str(execution))
    os.killpg(worker_process.pid,signal.SIGKILL);worker_process.wait(timeout=10);procs.remove(worker_process)
    spool=OUT/'runtime-envs'/'.log-delivery';spool.rename(OUT/'lost-spool-volume')
    worker_process=start('worker',cfg,{'ATTUNE_API_URL':f'http://127.0.0.1:{proxy.server_port}'})
    until(lambda:pg('SELECT state FROM log_delivery_intent WHERE artifact_version='+str(version['id']))=='incomplete',30,'missing spool custody diagnosis')
    assert finite_status(version)==(409,'LOG_INCOMPLETE')
    assert pg('SELECT result::text FROM execution WHERE id='+str(execution))==preserved
    print('Lost-volume incomplete-log diagnosis passed',flush=True)
    blocked.set()
    execution,intent=echo();version=version_for(execution)
    drop_seal_version=version['id'];drop_seal_reply.set();blocked.clear()
    until(lambda:pg('SELECT state FROM log_delivery_intent WHERE artifact_version='+str(version['id']))=='ready',30,'lost seal acknowledgement reconciliation')
    assert not drop_seal_reply.is_set(),'fault must intercept a successful real seal'
    assert pg('SELECT count(*) FROM log_segment WHERE stream=(SELECT id FROM log_stream WHERE artifact_version='+str(version['id'])+')')=='1'
    assert 'async-log-custody-fault' in finite_status(version)[1]
    until(lambda:not any((OUT/'runtime-envs'/'.log-delivery').glob('*/'+str(version['id'])+'.spool')),10,'acknowledged spool cleanup')
    print('Lost seal acknowledgement reconciliation passed',flush=True)
    blocked.set();execution,intent=echo();version=version_for(execution);preserved=pg('SELECT result::text FROM execution WHERE id='+str(execution))
    until(lambda:pg('SELECT finalize_by<=clock_timestamp() FROM log_delivery_intent WHERE artifact_version='+str(version['id']))=='t',75,'authoritative delivery deadline')
    assert finite_status(version)==(409,'LOG_INCOMPLETE')
    assert pg('SELECT result::text FROM execution WHERE id='+str(execution))==preserved
    until(lambda:pg('SELECT state FROM log_delivery_intent WHERE artifact_version='+str(version['id']))=='incomplete',10,'expired delivery settlement')
    blocked.clear()
    assert pg('SELECT body_state::text FROM artifact_version WHERE id='+str(version['id']))=='pending'
    assert pg('SELECT file_evidence_version_eligible('+str(version['id'])+')')=='t','expired custody must release its retention obligation'
    print('Delivery deadline preserves completed action and rejects late publication',flush=True)
    proxy.shutdown();proxy.server_close();thread.join(timeout=10)
    (OUT/'postgres.log').write_text(subprocess.check_output(['docker','logs',cid('postgres')],text=True,stderr=subprocess.STDOUT))
    # Server logs also catch sessions opened/closed between one-second samples.
    connection_events=[];pid_apps={}
    for line in (OUT/'postgres.log').read_text().splitlines():
        match=re.match(r'^(\d{4}-\d\d-\d\d \d\d:\d\d:\d\d\.\d+) UTC \[(\d+)\] (.*?) LOG:\s+(.*)$',line)
        if not match:continue
        when=stamp(match[1]+'+00:00');pid=int(match[2]);message=match[4]
        authorized=re.search(r'application_name=(\S+)',message) if 'connection authorized:' in message else None
        if authorized:pid_apps[pid]=authorized[1]
        app=pid_apps.get(pid,match[3])
        if app.startswith('attune:api:main:') and (authorized or 'disconnection:' in message):
            connection_events.append({'time':when.isoformat(),'pid':pid,'application_name':app,'kind':'open' if authorized else 'close'})
    api_records=[]
    for line in (OUT/'api.log').read_text().splitlines():
        try:record=json.loads(line)
        except json.JSONDecodeError:continue
        text=json.dumps(record.get('fields',{})).lower()
        if record.get('target','').startswith('sqlx') and any(term in text for term in ('pool must dispose','session is not clean','unclean session rejected')):api_records.append(record)
    for mode,cohort in cohorts.items():
        start,end=stamp(cohort['start']),stamp(cohort['end'])
        events=[event for event in connection_events if start<=stamp(event['time'])<=end]
        rejections=[record for record in api_records if start<=stamp(record['timestamp'])<=end]
        summary['api_connections'][mode].update(server_opens=sum(event['kind']=='open' for event in events),server_closes=sum(event['kind']=='close' for event in events),unclean_session_rejections=len(rejections))
        assert not events and not rejections,('warmed API connection churn',mode,events,rejections)
    (OUT/'connection-events.json').write_text(json.dumps(connection_events,indent=2))
    compose('run','--rm','aws','s3api','list-object-versions','--bucket','attune-echo-validation',stdout=open(OUT/'rustfs-object-versions.json','w'))
    stored=json.loads((OUT/'rustfs-object-versions.json').read_text())
    log_objects=[version for version in stored.get('Versions',[]) if '/logs/' in version['Key']]
    assert len(log_objects)>=sum(len(cohort['rows']) for cohort in cohorts.values()),'measured logs must exist as versioned RustFS objects'
    summary['rustfs_log_object_versions']=len(log_objects)
    summary['faults']={'persistent_spool_restart':'passed','lost_volume_incomplete':'passed','lost_seal_acknowledgement':'passed','deadline_incomplete':'passed','bounded_custody_retention':'passed'}
    (OUT/'summary.json').write_text(json.dumps(summary,indent=2))

finally:
    for p in reversed(procs):
        if p.poll() is None:os.killpg(p.pid,signal.SIGTERM)
    for p in reversed(procs):
        try:p.wait(timeout=45)
        except subprocess.TimeoutExpired:
            os.killpg(p.pid,signal.SIGKILL); p.wait(timeout=10)
    if owned:
        try:
            for service in ('postgres','rabbitmq','rustfs'):
                container=cid(service) or capture(['docker','compose','-p',PROJECT,'-f',str(OUT/'compose.yaml'),'ps','-a','-q',service])
                if container:
                    result=subprocess.run(['docker','logs',container],text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,timeout=30)
                    (OUT/(service+'.log')).write_text(result.stdout)
        finally:
            compose('down','--volumes','--remove-orphans',timeout=180)
    cleanup={'native_services_joined':all(p.poll() is not None for p in procs)}
    for kind,command,field in [('containers',['ps','-a'],'{{.ID}}'),('networks',['network','ls'],'{{.ID}}'),('volumes',['volume','ls'],'{{.Name}}')]:
        remaining=capture(['docker',*command,'--filter','label=com.docker.compose.project='+PROJECT,'--format',field])
        cleanup[kind]=remaining.splitlines() if remaining else []
        assert not remaining,('owned resources remain',kind,remaining)
    (OUT/'cleanup.json').write_text(json.dumps(cleanup,indent=2))
    print('Owned stack stopped and volumes removed',flush=True)
