#!/usr/bin/env python3
"""Static review census, not execution or role-isolation proof."""
import json
from pathlib import Path
import re
ROOT=Path(__file__).resolve().parents[1]
rows=[]
for p in sorted((ROOT/'crates').rglob('*.rs')):
    relative=str(p.relative_to(ROOT))
    if 'cache' not in relative or '/tests/' not in relative or '/fixtures/' in relative or 'recorded_rollout' in relative:continue
    text=p.read_text();tests=list(re.finditer(r'#\[(?:tokio::)?test(?:\([^\]]*\))?\]',text))
    if not tests:continue
    ddl=[{'line':i,'source':line.strip()} for i,line in enumerate(text.splitlines(),1) if re.search(r'\b(?:CREATE\s+(?:TABLE|TRIGGER|FUNCTION)|ALTER\s+(?:TABLE|FUNCTION)|DROP\s+(?:TABLE|TRIGGER|FUNCTION)|DISABLE\s+TRIGGER|ENABLE\s+TRIGGER)\b',line,re.I)]
    maintenance=[]; cases=[]
    for i,m in enumerate(tests):
        body=text[m.end():tests[i+1].start() if i+1<len(tests) else len(text)]
        name=re.search(r'(?:async\s+)?fn\s+(\w+)',body)
        maint=bool(re.search(r'drop_if_cleanup_eligible|refresh_statistics|drop_cleanup_cache_generation|ANALYZE cache_entry',body))
        admin=bool(re.search(r'\b(?:CREATE\s+(?:TABLE|TRIGGER|FUNCTION)|ALTER\s+(?:TABLE|FUNCTION)|DROP\s+(?:TABLE|TRIGGER|FUNCTION)|DISABLE\s+TRIGGER|ENABLE\s+TRIGGER)\b',body,re.I))
        if name:
            cases.append({'name':name.group(1),'direct_admin_ddl':admin,'maintenance_candidate':maint})
            if maint:maintenance.append(name.group(1))
    rows.append({'path':relative,'declared_tests':len(tests),'test_candidates':cases,'direct_ddl_lines':ddl,'maintenance_test_candidates':maintenance,'uses_test_context':'TestContext' in text,'uses_owner_pool_fixture':bool(re.search(r'create_test_pool|TestDatabase::create',text))})
print(json.dumps({'scope':'source candidates, includes ignored tests and helper DDL; no assertion that all need runtime creation; CLI has no direct PG identity','files':rows},indent=2))
