import errno
import os
from pathlib import Path

pack = Path('/opt/attune/pack')
runtime = Path('/opt/attune/runtime')
artifacts = Path('/tmp/artifacts')
assert Path.cwd() == pack
assert (pack / 'source.txt').read_text() == 'immutable candidate source'
assert not (pack / 'generated.lock').exists()
assert not (artifacts / 'candidate.txt').exists()
assert (runtime / 'project/source.txt').read_text() == 'published runtime project'
assert not os.statvfs(pack).f_flag & os.ST_RDONLY
assert os.statvfs(runtime).f_flag & os.ST_RDONLY

# Candidate tests keep their mutable Pack cwd even with published dependencies.
(pack / 'source.txt').write_text('private candidate update')
(pack / 'generated.lock').write_text('private generated lock')
try:
    (runtime / 'project/source.txt').write_text('poisoned runtime')
except OSError as error:
    # Docker's sealed cache file can reject permission before the mount check.
    # The positive statvfs assertion above independently requires a RO mount.
    assert error.errno in (errno.EROFS, errno.EACCES)
else:
    raise AssertionError('published Runtime was writable')

assert not Path('/var/run/docker.sock').exists()
assert not Path('/var/run/secrets/kubernetes.io/serviceaccount/token').exists()
assert not any(name.startswith(('ATTUNE__', 'DOCKER_', 'KUBERNETES_')) for name in os.environ)
(artifacts / 'candidate.txt').write_bytes(b'owned-candidate-artifact')
print('owned-candidate-private-copy', end='')
