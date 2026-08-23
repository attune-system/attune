#!/bin/sh

set -eu

repo_root=$(CDPATH= cd "$(dirname "$0")/.." && pwd)
test_dir=$(mktemp -d "${TMPDIR:-/tmp}/attune-seeder-test.XXXXXX")
trap 'rm -rf -- "$test_dir"' EXIT HUP INT TERM
mkdir "$test_dir/bin"

cat >"$test_dir/bin/wget" <<'EOF'
#!/bin/sh
while [ "$#" -gt 0 ]; do
    case "$1" in
        -O)
            output=$2
            shift 2
            ;;
        *)
            url=$1
            shift
            ;;
    esac
done
printf '%s\n' "$url" >>"$ATTUNE_SEED_TEST_WGET_LOG"
printf '{"registry_name":"test","packs":[]}\n' >"$output"
EOF

cat >"$test_dir/bin/psql" <<'EOF'
#!/bin/sh
count=0
[ ! -f "$ATTUNE_SEED_TEST_PSQL_COUNT" ] || count=$(cat "$ATTUNE_SEED_TEST_PSQL_COUNT")
count=$((count + 1))
printf '%s\n' "$count" >"$ATTUNE_SEED_TEST_PSQL_COUNT"
cat >"$ATTUNE_SEED_TEST_PSQL_INPUT.$count"
printf '%s\n' "$*" >"$ATTUNE_SEED_TEST_PSQL_ARGS.$count"
printf '%s\n' "${PGDATABASE:-}" >"$ATTUNE_SEED_TEST_PSQL_ENV.$count"
if [ "$count" -eq 1 ]; then
    printf '%s\n' "${ATTUNE_SEED_TEST_STATE:-update}"
elif [ "$count" -eq 2 ]; then
    printf 't\n'
fi
EOF
chmod +x "$test_dir/bin/wget" "$test_dir/bin/psql"

run_seed() {
    rm -f "$test_dir"/psql-count "$test_dir"/psql-input.* \
        "$test_dir"/psql-args.* "$test_dir"/psql-env.* "$test_dir"/wget.log
    ATTUNE_SEED_TEST_WGET_LOG="$test_dir/wget.log" \
    ATTUNE_SEED_TEST_PSQL_COUNT="$test_dir/psql-count" \
    ATTUNE_SEED_TEST_PSQL_INPUT="$test_dir/psql-input" \
    ATTUNE_SEED_TEST_PSQL_ARGS="$test_dir/psql-args" \
    ATTUNE_SEED_TEST_PSQL_ENV="$test_dir/psql-env" \
    PATH="$test_dir/bin:$PATH" \
        sh "$repo_root/scripts/seed-standard-pack-index.sh" "$@"
}

default_ref=4c87ca62a4313f7e9646a50c44ab6b2b530e5f43
legacy_ref=793aabcc0eb537af7681a386b591de6c4fafd7a1

run_seed >"$test_dir/default.out"
grep -Fq "/$default_ref/index.json" "$test_dir/wget.log"
grep -Fq "index_url=https://raw.githubusercontent.com/attune-system/index/$default_ref/index.json" \
    "$test_dir/psql-args.3"
grep -Fq 'WHERE is_standard' "$test_dir/psql-input.3"

DATABASE_URL= \
ATTUNE__DATABASE__URL=postgresql://custom:secret@database.example:6543/attune \
    run_seed >"$test_dir/database-url.out"
grep -Fxq -- '-X -v ON_ERROR_STOP=1 -v index_url=https://raw.githubusercontent.com/attune-system/index/'"$default_ref"'/index.json -Atq' \
    "$test_dir/psql-args.1"
grep -Fxq 'postgresql://custom:secret@database.example:6543/attune' \
    "$test_dir/psql-env.1"

run_seed --ref "$legacy_ref" >"$test_dir/legacy.out"
grep -Fq "/$legacy_ref/index.json" "$test_dir/wget.log"
grep -Fq "index_url=https://raw.githubusercontent.com/attune-system/index/$legacy_ref/index.json" \
    "$test_dir/psql-args.3"
grep -Fq "([0-9a-f]{40}|main)" "$test_dir/psql-input.3"

if run_seed --ref main >"$test_dir/invalid.out" 2>"$test_dir/invalid.err"; then
    printf 'Seeder accepted a mutable ref\n' >&2
    exit 1
fi
grep -Fq '40-character lowercase commit SHA' "$test_dir/invalid.err"

ATTUNE_SEED_TEST_STATE=deleted run_seed >"$test_dir/deleted.out"
grep -Fq 'remains deleted; skipping seed' "$test_dir/deleted.out"
[ ! -e "$test_dir/wget.log" ]

ATTUNE_SEED_TEST_STATE=custom run_seed >"$test_dir/custom.out"
grep -Fq 'administrator-managed URL; skipping seed update' "$test_dir/custom.out"
[ ! -e "$test_dir/wget.log" ]

printf 'standard pack index seeder tests passed\n'
