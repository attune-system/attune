#!/usr/bin/env bash
set -euo pipefail

action=${1:-}
: "${RUNTIME_LOG_HARNESS_OWNER:?RUNTIME_LOG_HARNESS_OWNER is required}"
: "${RUNTIME_LOG_S3_CONTAINER:?RUNTIME_LOG_S3_CONTAINER is required}"
: "${RUNTIME_LOG_S3_NETWORK:?RUNTIME_LOG_S3_NETWORK is required}"
: "${RUNTIME_LOG_S3_IMAGE:?RUNTIME_LOG_S3_IMAGE is required}"
: "${RUNTIME_LOG_AWS_CLI_IMAGE:?RUNTIME_LOG_AWS_CLI_IMAGE is required}"
: "${RUNTIME_LOG_S3_PORT:?RUNTIME_LOG_S3_PORT is required}"
: "${RUNTIME_LOG_S3_ACCESS_KEY:?RUNTIME_LOG_S3_ACCESS_KEY is required}"
: "${RUNTIME_LOG_S3_SECRET_KEY:?RUNTIME_LOG_S3_SECRET_KEY is required}"
: "${RUNTIME_LOG_S3_BUCKET:?RUNTIME_LOG_S3_BUCKET is required}"
: "${RUNTIME_LOG_S3_PREFIX:?RUNTIME_LOG_S3_PREFIX is required}"

label_key=io.attune.runtime-log-harness

owned_resource() {
    local type=$1
    local name=$2
    local owner
    if [[ $type == container ]]; then
        owner=$(docker container inspect "$name" --format "{{ index .Config.Labels \"$label_key\" }}" 2>/dev/null) || return 1
    else
        owner=$(docker network inspect "$name" --format "{{ index .Labels \"$label_key\" }}" 2>/dev/null) || return 1
    fi
    [[ $owner == "$RUNTIME_LOG_HARNESS_OWNER" ]]
}

refuse_foreign_resource() {
    local type=$1
    local name=$2
    if docker "$type" inspect "$name" >/dev/null 2>&1 && ! owned_resource "$type" "$name"; then
        printf 'refusing to modify %s %s without ownership label %s=%s\n' \
            "$type" "$name" "$label_key" "$RUNTIME_LOG_HARNESS_OWNER" >&2
        exit 1
    fi
}

run_aws() {
    local network=$1
    shift
    docker run --rm --network "$network" --entrypoint /usr/local/bin/aws \
        -e AWS_ACCESS_KEY_ID="$RUNTIME_LOG_S3_ACCESS_KEY" \
        -e AWS_SECRET_ACCESS_KEY="$RUNTIME_LOG_S3_SECRET_KEY" \
        -e AWS_DEFAULT_REGION=us-east-1 -e AWS_PAGER= \
        "$RUNTIME_LOG_AWS_CLI_IMAGE" "$@"
}

cleanup_prefix() {
    [[ $RUNTIME_LOG_S3_PREFIX == runtime-log-tests/"$RUNTIME_LOG_HARNESS_OWNER" ]] || {
        printf 'refusing broad S3 cleanup for prefix %s\n' "$RUNTIME_LOG_S3_PREFIX" >&2
        exit 1
    }

    local endpoint=${ATTUNE_TEST_S3_ENDPOINT:-http://127.0.0.1:$RUNTIME_LOG_S3_PORT}
    local first_version
    local payload
    while true; do
        first_version=$(run_aws host --endpoint-url "$endpoint" s3api list-object-versions \
            --bucket "$RUNTIME_LOG_S3_BUCKET" --prefix "$RUNTIME_LOG_S3_PREFIX" \
            --max-items 1 --query '[Versions, DeleteMarkers][][] | [0].VersionId' --output text)
        [[ $first_version != None ]] || break
        payload=$(run_aws host --endpoint-url "$endpoint" s3api list-object-versions \
            --bucket "$RUNTIME_LOG_S3_BUCKET" --prefix "$RUNTIME_LOG_S3_PREFIX" \
            --max-items 1000 \
            --query '{Objects: [Versions, DeleteMarkers][][] | [].{Key: Key, VersionId: VersionId}, Quiet: `true`}')
        run_aws host --endpoint-url "$endpoint" s3api delete-objects \
            --bucket "$RUNTIME_LOG_S3_BUCKET" --delete "$payload" >/dev/null
    done
}

case "$action" in
    up)
        refuse_foreign_resource container "$RUNTIME_LOG_S3_CONTAINER"
        refuse_foreign_resource network "$RUNTIME_LOG_S3_NETWORK"
        if owned_resource container "$RUNTIME_LOG_S3_CONTAINER"; then
            docker rm -f "$RUNTIME_LOG_S3_CONTAINER" >/dev/null
        fi
        if ! owned_resource network "$RUNTIME_LOG_S3_NETWORK"; then
            docker network create \
                --label "$label_key=$RUNTIME_LOG_HARNESS_OWNER" \
                "$RUNTIME_LOG_S3_NETWORK" >/dev/null
        fi
        docker run -d --name "$RUNTIME_LOG_S3_CONTAINER" \
            --label "$label_key=$RUNTIME_LOG_HARNESS_OWNER" \
            --network "$RUNTIME_LOG_S3_NETWORK" \
            -p "$RUNTIME_LOG_S3_PORT:9000" \
            -e RUSTFS_ACCESS_KEY="$RUNTIME_LOG_S3_ACCESS_KEY" \
            -e RUSTFS_SECRET_KEY="$RUNTIME_LOG_S3_SECRET_KEY" \
            "$RUNTIME_LOG_S3_IMAGE" >/dev/null
        endpoint="http://$RUNTIME_LOG_S3_CONTAINER:9000"
        ready=false
        for _ in {1..60}; do
            if run_aws "$RUNTIME_LOG_S3_NETWORK" --endpoint-url "$endpoint" \
                s3api list-buckets >/dev/null 2>&1; then
                ready=true
                break
            fi
            if [[ $(docker inspect "$RUNTIME_LOG_S3_CONTAINER" --format '{{.State.Running}}') != true ]]; then
                break
            fi
            sleep 1
        done
        if [[ $ready != true ]]; then
            printf 'S3 test storage did not become ready\n' >&2
            docker logs "$RUNTIME_LOG_S3_CONTAINER" >&2
            exit 1
        fi
        run_aws "$RUNTIME_LOG_S3_NETWORK" --endpoint-url "$endpoint" \
            s3api create-bucket --bucket "$RUNTIME_LOG_S3_BUCKET" >/dev/null
        run_aws "$RUNTIME_LOG_S3_NETWORK" --endpoint-url "$endpoint" \
            s3api put-bucket-versioning --bucket "$RUNTIME_LOG_S3_BUCKET" \
            --versioning-configuration Status=Enabled
        ;;
    clean-prefix)
        cleanup_prefix
        ;;
    down)
        refuse_foreign_resource container "$RUNTIME_LOG_S3_CONTAINER"
        refuse_foreign_resource network "$RUNTIME_LOG_S3_NETWORK"
        if owned_resource container "$RUNTIME_LOG_S3_CONTAINER"; then
            if [[ $(docker inspect "$RUNTIME_LOG_S3_CONTAINER" --format '{{.State.Running}}') == true ]]; then
                cleanup_prefix
            fi
            docker rm -f "$RUNTIME_LOG_S3_CONTAINER" >/dev/null
        fi
        if owned_resource network "$RUNTIME_LOG_S3_NETWORK"; then
            docker network rm "$RUNTIME_LOG_S3_NETWORK" >/dev/null
        fi
        ;;
    *)
        printf 'usage: %s {up|clean-prefix|down}\n' "$0" >&2
        exit 2
        ;;
esac
