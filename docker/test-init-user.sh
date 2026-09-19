#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

cat > "$TMP/date" <<'EOF'
#!/bin/sh
state=${MOCK_DATE_STATE:?}
value=100
if [ -f "$state" ]; then
    value=$(cat "$state")
fi
printf '%s\n' "$value"
printf '%s\n' $((value + 1)) > "$state"
EOF

cat > "$TMP/psql" <<'EOF'
#!/bin/sh
if [ "${MOCK_PSQL_MODE:-ready}" = unavailable ]; then
    exit 1
fi
input=$(cat)
printf '%s\n' "$input" >> "${MOCK_PSQL_LOG:?}"
case "$input" in
    *'SELECT COUNT(*) FROM attune.identity'*) printf '0\n' ;;
    *'SELECT EXISTS ('*) printf 't\n' ;;
esac
EOF

cat > "$TMP/sleep" <<'EOF'
#!/bin/sh
exit 0
EOF
chmod +x "$TMP/date" "$TMP/psql" "$TMP/sleep"

PATH="$TMP:$PATH" MOCK_DATE_STATE="$TMP/date-state" MOCK_PSQL_LOG="$TMP/psql.log" \
    ATTUNE_BOOTSTRAP_TIMEOUT_SECONDS=300 sh "$ROOT/docker/init-user.sh" >/dev/null

grep -q "permission_set.management_origin = 'platform'" "$TMP/psql.log"
grep -q "ON CONFLICT (identity, permset) DO NOTHING" "$TMP/psql.log"

if PATH="$TMP:$PATH" MOCK_DATE_STATE="$TMP/timeout-date-state" MOCK_PSQL_LOG="$TMP/timeout.log" \
    MOCK_PSQL_MODE=unavailable ATTUNE_BOOTSTRAP_TIMEOUT_SECONDS=999 \
    sh "$ROOT/docker/init-user.sh" >/dev/null 2>&1; then
    printf 'init-user accepted an unavailable database past its bounded deadline\n' >&2
    exit 1
fi
