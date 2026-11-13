#!/bin/sh
# Real core action entrypoints; owned loopback HTTP and fake curl only.
set -eu
cd "$(dirname "$0")/../.."
exec python3 -m unittest packs.core.tests.test_actions packs.core.tests.test_stdin_json -v
