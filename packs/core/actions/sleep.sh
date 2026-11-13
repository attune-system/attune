#!/bin/sh
# Canonical flat stdin JSON through a checked Python runtime executable.
set -eu
case "$0" in
    */*) action_dir=${0%/*} ;;
    *) action_dir=. ;;
esac
exec /bin/sh "$action_dir/core_json.sh" sleep
