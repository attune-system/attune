#!/bin/sh
# Real core action entrypoints; owned loopback HTTP and fake curl only.
set -eu
cd "$(dirname "$0")"
exec python3 -m unittest discover -s tests -p 'test_*.py' -v
