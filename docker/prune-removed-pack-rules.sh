#!/bin/sh
set -eu

if [ "$#" -ne 2 ]; then
    printf '%s\n' "usage: $0 SOURCE_PACK_DIR TARGET_PACK_DIR" >&2
    exit 2
fi

source_pack_dir=$1
target_pack_dir=$2
target_rules_dir="$target_pack_dir/rules"

[ -d "$target_rules_dir" ] || exit 0

for target_rule in "$target_rules_dir"/*.yaml "$target_rules_dir"/*.yml; do
    [ -f "$target_rule" ] || continue
    rule_name=$(basename "$target_rule")
    if [ ! -f "$source_pack_dir/rules/$rule_name" ]; then
        rm -f "$target_rule"
    fi
done
