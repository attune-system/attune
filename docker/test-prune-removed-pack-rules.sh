#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TMP_DIR=$(mktemp -d)
trap 'rm -rf "$TMP_DIR"' EXIT

SOURCE="$TMP_DIR/source"
TARGET="$TMP_DIR/target"
mkdir -p "$SOURCE/rules" "$TARGET/rules"

touch "$SOURCE/rules/current.yaml"
touch "$TARGET/rules/current.yaml"
touch "$TARGET/rules/removed.yaml"
touch "$TARGET/rules/notes.txt"

sh "$SCRIPT_DIR/prune-removed-pack-rules.sh" "$SOURCE" "$TARGET"

test -f "$TARGET/rules/current.yaml"
test ! -e "$TARGET/rules/removed.yaml"
test -f "$TARGET/rules/notes.txt"

rm -rf "$SOURCE/rules"
sh "$SCRIPT_DIR/prune-removed-pack-rules.sh" "$SOURCE" "$TARGET"

test ! -e "$TARGET/rules/current.yaml"
test -f "$TARGET/rules/notes.txt"

printf '%s\n' "removed pack rule pruning checks passed"
