#!/usr/bin/env python3
"""Compare actual repository execution batches across producer SQL variants."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import uuid

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location("native_verify",ROOT/"scripts/verify-native-partitions.py")
verify=importlib.util.module_from_spec(spec)
spec.loader.exec_module(verify)
protocol=verify.protocol
require=protocol.require


def summarize_plans(output, versions):
    result={}
    for version in versions:
        lines=(output/f"pg{version}.postgres.log").read_text().splitlines()
        plans=[]
        for index,line in enumerate(lines):
            match=re.search(r"\] nd_path_(\w+) LOG:  duration: ([\d.]+) ms\s+plan:",line)
            if not match: continue
            body=[]
            for later in lines[index+1:]:
                if re.match(r"^\d{4}-\d\d-\d\d ",later): break
                body.append(later)
            try: plan,_=json.JSONDecoder().raw_decode("\n".join(body).lstrip())
            except json.JSONDecodeError: continue
            query=plan.get("Query Text","")
            if "native_summary_invalidation" not in query: continue
            scans=[]
            def walk(node):
                if node.get("Relation Name")=="native_summary_invalidation":
                    scans.append({k:node[k] for k in ["Node Type","Index Name","Index Cond","Filter","Rows Removed by Filter","Actual Rows","Actual Loops"] if k in node})
                for child in node.get("Plans",[]): walk(child)
            walk(plan["Plan"])
            plans.append({"variant":match[1],"server_ms":float(match[2]),"query":query,"scans":scans})
        (output/f"pg{version}.plans.json").write_text(json.dumps(plans,indent=2)+"\n")
        totals={}
        for p in plans:
            s=totals.setdefault(p["variant"],{"queries":0,"server_ms":0,"indexes":{}})
            s["queries"]+=1;s["server_ms"]+=p["server_ms"]
            for scan in p["scans"]:
                key=scan.get("Index Name",scan["Node Type"])+" | "+scan.get("Index Cond","")+" | "+scan.get("Filter","")
                s["indexes"][key]=s["indexes"].get(key,0)+1
        result[version]=totals
    (output/"plan-summary.json").write_text(json.dumps(result,indent=2)+"\n")
    print(json.dumps(result,indent=2))


def variants(baseline_sql=None):
    text=baseline_sql.read_text() if baseline_sql else verify.native_sql(verify.SUMMARIES)
    original=re.search(r"CREATE (?:OR REPLACE )?FUNCTION native_summary_notify\(\)[\s\S]+?^END \$\$;",text,re.MULTILINE).group()
    original=original.replace("CREATE FUNCTION","CREATE OR REPLACE FUNCTION",1)
    bound=original.replace("DECLARE rows_sql TEXT; time_col TEXT;","DECLARE rows_sql TEXT; time_col TEXT; v_origin xid8 := pg_current_xact_id(); v_xmin xid := v_origin::xid;")
    bound=bound.replace("existing.transaction_origin=pg_current_xact_id()","existing.transaction_origin=$1").replace("existing.xmin=pg_current_xact_id()::xid","existing.xmin=$2")
    bound=bound.replace("|| own_marker_filter;","|| own_marker_filter USING v_origin,v_xmin;")
    result={"original":original}
    if "DECLARE rows_sql TEXT; time_col TEXT;" in original:
        result["bound"]=bound
    for style in ["cached_combined","cached_loop"]:
        body="""CREATE OR REPLACE FUNCTION native_summary_notify() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    v_origin xid8 := pg_current_xact_id();
    v_xmin xid := v_origin::xid;
    v_status native_summary_kind := CASE TG_TABLE_NAME WHEN 'execution_history' THEN 'execution_status'::native_summary_kind ELSE 'worker_status'::native_summary_kind END;
    v_creation boolean := TG_TABLE_NAME='execution_history';
    v_group record;
BEGIN
"""
        for source in ["event","history"]:
            body+=("IF" if source=="event" else "ELSE")+ (" TG_TABLE_NAME='event' THEN\n" if source=="event" else "\n")
            for n,op in enumerate(["INSERT","DELETE","UPDATE"]):
                rows={"INSERT":"SELECT * FROM native_new","DELETE":"SELECT * FROM native_old","UPDATE":"SELECT * FROM native_old UNION ALL SELECT * FROM native_new"}[op]
                body+=("IF" if n==0 else "ELSIF")+f" TG_OP='{op}' THEN\n"
                if source=="event":
                    grouped=f"SELECT 'event_volume'::native_summary_kind AS kind,date_trunc('hour',created,'UTC') AS bucket FROM ({rows}) r GROUP BY 2"
                else:
                    grouped=f"SELECT kinds.kind,date_trunc('hour',r.time,'UTC') AS bucket FROM ({rows}) r CROSS JOIN LATERAL (VALUES(v_status,'status'=ANY(r.changed_fields)),('execution_creation'::native_summary_kind,v_creation AND r.operation='INSERT')) kinds(kind,relevant) WHERE kinds.relevant GROUP BY kinds.kind,bucket"
                own="SELECT 1 FROM native_summary_invalidation i WHERE i.kind=v_group.kind AND i.bucket=v_group.bucket AND i.transaction_origin=v_origin AND i.xmin=v_xmin"
                if style=="cached_loop":
                    body+=f"FOR v_group IN {grouped} LOOP\n IF NOT EXISTS ({own}) THEN\n INSERT INTO native_summary_invalidation(kind,bucket) VALUES(v_group.kind,v_group.bucket);\n END IF;\nEND LOOP;\n"
                else:
                    body+=f"INSERT INTO native_summary_invalidation(kind,bucket) SELECT a.kind,a.bucket FROM ({grouped}) a WHERE NOT EXISTS (SELECT 1 FROM native_summary_invalidation i WHERE i.kind=a.kind AND i.bucket=a.bucket AND i.transaction_origin=v_origin AND i.xmin=v_xmin);\n"
            body+="END IF;\n"
        body+="END IF;\nRETURN NULL;\nEND $$;"
        result[style]=body
    fast=result["cached_loop"].replace("    v_group record;","    v_group record; v_row record; v_first record; v_count integer := 0; v_bucket timestamptz; v_kind native_summary_kind; v_kinds native_summary_kind[];")
    fast_path="""
IF TG_OP='INSERT' THEN
  IF TG_TABLE_NAME='event' THEN
    FOR v_row IN SELECT created FROM native_new LIMIT 2 LOOP
      v_count:=v_count+1; IF v_count=1 THEN v_first:=v_row; END IF;
    END LOOP;
  ELSE
    FOR v_row IN SELECT time,operation,changed_fields FROM native_new LIMIT 2 LOOP
      v_count:=v_count+1; IF v_count=1 THEN v_first:=v_row; END IF;
    END LOOP;
  END IF;
  IF v_count=0 THEN RETURN NULL; END IF;
  IF v_count=1 THEN
    v_kinds:=ARRAY[]::native_summary_kind[];
    IF TG_TABLE_NAME='event' THEN
      v_bucket:=date_trunc('hour',v_first.created,'UTC'); v_kinds:=ARRAY['event_volume'::native_summary_kind];
    ELSE
      v_bucket:=date_trunc('hour',v_first.time,'UTC');
      IF 'status'=ANY(v_first.changed_fields) THEN v_kinds:=array_append(v_kinds,v_status); END IF;
      IF v_creation AND v_first.operation='INSERT' THEN v_kinds:=array_append(v_kinds,'execution_creation'::native_summary_kind); END IF;
    END IF;
    FOREACH v_kind IN ARRAY v_kinds LOOP
      IF NOT EXISTS (SELECT 1 FROM native_summary_invalidation i WHERE i.kind=v_kind AND i.bucket=v_bucket AND i.transaction_origin=v_origin AND i.xmin=v_xmin) THEN
        INSERT INTO native_summary_invalidation(kind,bucket) VALUES(v_kind,v_bucket);
      END IF;
    END LOOP;
    RETURN NULL;
  END IF;
END IF;
"""
    result["fast_single"]=fast.replace("BEGIN\n","BEGIN\n"+fast_path,1)
    return result


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--versions",nargs="+",choices=["16","18"],default=["16","18"])
    parser.add_argument("--output",type=Path,required=True)
    parser.add_argument("--variants",nargs="+",choices=["original","bound","cached_combined","cached_loop","fast_single"],default=["original","bound","cached_combined","cached_loop"])
    parser.add_argument("--diagnostic-only",action="store_true")
    parser.add_argument("--summarize-plans",action="store_true")
    parser.add_argument("--baseline-sql",type=Path,help="explicit frozen producer function for a historical comparison")
    args=parser.parse_args()
    if args.summarize_plans:
        summarize_plans(args.output,args.versions)
        return
    require(not args.output.exists(),"refusing existing output")
    args.output.parent.resolve(strict=True)
    args.output.mkdir()
    ddls=variants(args.baseline_sql)
    require(all(name in ddls for name in args.variants),"bound variant needs a dynamic historical --baseline-sql; production producer is already cached")
    for name,ddl in ddls.items(): (args.output/f"{name}.sql").write_text(ddl)
    (args.output/"variants.json").write_text(json.dumps(args.variants))
    evidence={"run_id":"paths-"+uuid.uuid4().hex[:12],"variant_sha256":{n:hashlib.sha256(d.encode()).hexdigest() for n,d in ddls.items()},"versions":[]}
    build=subprocess.run(["cargo","build","-p","attune-common","--example","measure_native_producer"],cwd=ROOT,text=True,capture_output=True,timeout=2400,env={**os.environ,"SQLX_OFFLINE":"true"})
    (args.output/"build.log").write_text(build.stdout+build.stderr)
    require(build.returncode==0,"producer benchmark compilation failed; see build.log")
    try:
        for version in args.versions:
            report={"version":version,"rejections":{},"observations":{}}
            evidence["versions"].append(report)
            with protocol.Server(f"postgres:{version}-alpine",evidence["run_id"],report) as server:
                server.sql("ALTER SYSTEM SET log_line_prefix='%m [%p] %a '; SELECT pg_reload_conf();")
                verify.database_ddl(server,"CREATE DATABASE producer_paths;")
                port=report["port"].rsplit(":",1)[1]
                env={**os.environ,"ATTUNE__DATABASE__URL":f"postgresql://postgres@127.0.0.1:{port}/producer_paths","ATTUNE_TEST_RUN_ID":"np"+uuid.uuid4().hex[:10],"SQLX_OFFLINE":"true","NATIVE_PRODUCER_DIAGNOSTIC_ONLY":"1" if args.diagnostic_only else "0"}
                result=subprocess.run([str(ROOT/"target/debug/examples/measure_native_producer"),str(args.output.resolve()),str((args.output/f"pg{version}.json").resolve())],cwd=ROOT,env=env,capture_output=True,text=True,timeout=2400)
                (args.output/f"pg{version}.stdout.log").write_text(result.stdout)
                (args.output/f"pg{version}.stderr.log").write_text(result.stderr)
                report.update({"exit_code":result.returncode,"stdout":result.stdout,"stderr":result.stderr})
                logs=protocol.command("docker","logs",server.name,check=False)
                (args.output/f"pg{version}.postgres.log").write_text(logs.stdout+logs.stderr)
                require(result.returncode==0,"producer comparison failed; see output logs")
                require(server.sql("SELECT count(*) FROM pg_database WHERE datname LIKE 'attune_db_%';")=="0","test clone leak")
                report["passed"]=True
    except BaseException as error:
        evidence["error"]=str(error)
        raise
    finally:
        (args.output/"run.json").write_text(json.dumps(evidence,indent=2)+"\n")
        print(json.dumps({"run_id":evidence["run_id"],"error":evidence.get("error"),"output":str(args.output),"versions":[{"version":r["version"],"exit_code":r.get("exit_code"),"stdout":r.get("stdout"),"cleanup_errors":r.get("cleanup_errors")} for r in evidence["versions"]]},indent=2))


if __name__=="__main__": main()
