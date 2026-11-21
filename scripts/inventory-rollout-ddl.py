#!/usr/bin/env python3
"""Source DDL index (not live privilege proof). Emits definitions and Rust call sites."""
import json
from pathlib import Path
import re
ROOT=Path(__file__).resolve().parents[1]
result=[]
for file in sorted((ROOT/'migrations').glob('*.sql')):
    text=file.read_text()
    # Dollar-quoted canonical function bodies; keep overload/version provenance.
    pattern=r'CREATE\s+(?:OR\s+REPLACE\s+)?FUNCTION\s+(\w+)\b([\s\S]*?)\bAS\s+(\$\w*\$)([\s\S]*?)\3'
    for match in re.finditer(pattern,text,re.I):
        body=match.group(4)
        if not re.search(r'\b(?:CREATE|ALTER|DROP|TRUNCATE|ANALYZE|VACUUM|REINDEX)\s+(?:TABLE|INDEX|FUNCTION|TRIGGER|SCHEMA|cache_entry)',body,re.I):continue
        name=match.group(1);callers=[]
        for source in sorted((ROOT/'crates').rglob('*.rs')):
            if '/tests/' in str(source):continue
            for line,content in enumerate(source.read_text().splitlines(),1):
                if name+'(' in content:callers.append({'path':str(source.relative_to(ROOT)),'line':line,'source':content.strip()})
        result.append({'migration':str(file.relative_to(ROOT)),'line':text[:match.start()].count('\n')+1,'function':name,'security_definer':'SECURITY DEFINER' in match.group(2).upper(),'body':body.strip(),'rust_references':callers})
print(json.dumps({'scope':'canonical function DDL bodies and lexical Rust references; includes replaced definitions/test candidates, excludes install-only top-level DDL; live ACL inventory is separate','definitions':result},indent=2))
