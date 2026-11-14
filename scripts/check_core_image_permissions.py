#!/usr/bin/env python3
"""Owned image-level core publication check; caller owns the shared Docker lease."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import uuid

PROGRAM = r'''
import hashlib,importlib.util,io,json,os,subprocess,tarfile
from pathlib import Path
assert os.getuid() == 1000
spec=importlib.util.spec_from_file_location('bootstrap','/scripts/bootstrap_core_pack.py')
bootstrap=importlib.util.module_from_spec(spec);spec.loader.exec_module(bootstrap)
pack=Path('/source/packs/core')
if EXPECT_PRIVATE:
    assert pack.joinpath('actions').stat().st_mode & 0o777 == 0o700
    try: bootstrap.validate_native_payload(pack)
    except PermissionError:
        print(json.dumps({'uid':os.getuid(),'private0700_rejected':True}))
    else: raise AssertionError('Private image unexpectedly traversable by UID1000')
else:
    bootstrap.validate_native_payload(pack)
    files={str(p.relative_to(pack)):{'sha256':hashlib.sha256(p.read_bytes()).hexdigest(),'executable':bool(p.stat().st_mode & 0o111)} for p in sorted(pack.rglob('*')) if p.is_file()}
    data=bootstrap.deterministic_archive(pack)
    with tarfile.open(fileobj=io.BytesIO(data),mode='r:gz') as archive:
        assert sum(entry.isfile() for entry in archive) == len(files)
    release=json.loads(subprocess.check_output(['/canonical-helper',str(pack),'/tmp/canonical-check'],text=True))
    print(json.dumps({'uid':os.getuid(),'files':files,'upload_tar_sha256':hashlib.sha256(data).hexdigest(),'canonical_digest':release['digest'],'publisher_script_sha256':hashlib.sha256(Path('/scripts/bootstrap_core_pack.py').read_bytes()).hexdigest()}))
'''

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image',required=True)
    parser.add_argument('--context',required=True)
    parser.add_argument('--owner',required=True)
    parser.add_argument('--report',type=Path,required=True)
    parser.add_argument('--expect-private',action='store_true')
    parser.add_argument('--expected-pack',type=Path)
    parser.add_argument('--canonical-helper',type=Path)
    parser.add_argument('--expected-digest')
    parser.add_argument('--publisher-script',type=Path)
    args=parser.parse_args()
    if not args.expect_private:
        assert all([args.expected_pack,args.canonical_helper,args.expected_digest,args.publisher_script])
    docker=['docker','--context',args.context]
    name=args.owner+'-uid-'+uuid.uuid4().hex[:8]
    created=False
    def run(*parts):
        result=subprocess.run([*docker,*map(str,parts)],capture_output=True,text=True,timeout=180)
        if result.returncode:
            raise RuntimeError('Owned image check command failed: '+result.stderr+result.stdout)
        return result.stdout
    with tempfile.TemporaryDirectory(prefix='core-image-uid-') as directory:
        program=Path(directory)/'check.py'
        program.write_text('EXPECT_PRIVATE='+repr(args.expect_private)+'\n'+PROGRAM)
        program.chmod(0o644)
        try:
            run('create','--name',name,'--label','io.attune.acceptance.owner='+args.owner,'--user','1000:1000','--network','none','--memory','512m','--cpus','1','--entrypoint','python3',args.image,'/check.py')
            created=True
            run('cp',program,name+':/check.py')
            if args.canonical_helper:
                run('cp',args.canonical_helper,name+':/canonical-helper')
            report=json.loads(run('start','--attach',name))
            if not args.expect_private:
                expected={str(p.relative_to(args.expected_pack)):{'sha256':hashlib.sha256(p.read_bytes()).hexdigest(),'executable':bool(p.stat().st_mode & 0o111)} for p in sorted(args.expected_pack.rglob('*')) if p.is_file()}
                assert report['files']==expected,'Image changed payload bytes or executable intent'
                assert report['canonical_digest']==args.expected_digest,'Canonical archive digest changed'
                assert report['publisher_script_sha256']==hashlib.sha256(args.publisher_script.read_bytes()).hexdigest()
            args.report.write_text(json.dumps(report,indent=2)+'\n')
            print('PASS UID1000 '+('private0700 rejection' if args.expect_private else 'preflight/read/tar/canonical publication')+' '+args.image)
        finally:
            if created:run('rm','-fv',name)

if __name__=='__main__':main()
