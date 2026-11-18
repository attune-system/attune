#!/usr/bin/env python3
"""Build every configured injected runtime; no build success implies OS readiness."""
import json
import subprocess
from pathlib import Path
import yaml

root = Path(__file__).resolve().parent.parent
compose = yaml.safe_load((root / 'docker-compose.yaml').read_text())
receipts = []
for name in ['worker-shell', 'worker-python', 'worker-node', 'worker-full', 'sensor']:
    service = compose['services'][name]
    build = service['build']
    arguments = ['docker', 'build', '--file', build['dockerfile'], '--target', build['target'], '--tag', service['image']]
    for key, value in sorted(build['args'].items()):
        assert '@sha256:' in value, 'runtime bases must be actual immutable references'
        arguments.extend(['--build-arg', f'{key}={value}'])
    arguments.append('.')
    subprocess.run(arguments, cwd=root, check=True, timeout=1800)
    check = 'set -eu; . /etc/os-release; test "$VERSION_CODENAME" = bookworm; test "$(dpkg-query -W -f=\'${Version}\' bubblewrap)" = 0.8.0-2+deb12u1; bwrap --help | grep -- --disable-userns'
    if name in ['worker-python', 'worker-full', 'sensor']:
        check += '; python3 -c "import sys; assert sys.version_info[:2] == (3,12)"'
    if name in ['worker-node', 'worker-full', 'sensor']:
        check += '; node -e "if(process.versions.node.split(\'.\')[0] !== \'22\') process.exit(1)"; npm --version'
    subprocess.run(['docker', 'run', '--rm', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges:true', '--entrypoint', '/bin/sh', service['image'], '-c', check], check=True, timeout=60)
    image_id = subprocess.check_output(['docker', 'image', 'inspect', '--format', '{{.Id}}', service['image']], text=True).strip()
    receipts.append({'service': name, 'image': service['image'], 'image_id': image_id, 'target': build['target'], 'bases': build['args'], 'namespace_support_proved': False})
print(json.dumps(receipts, indent=2))
