#!/usr/bin/env bash
set -euo pipefail

action=${1:-}
: "${RUNTIME_LOG_HARNESS_OWNER:?RUNTIME_LOG_HARNESS_OWNER is required}"
: "${RUNTIME_LOG_MINIO_CONTAINER:?RUNTIME_LOG_MINIO_CONTAINER is required}"
: "${RUNTIME_LOG_MINIO_NETWORK:?RUNTIME_LOG_MINIO_NETWORK is required}"
: "${RUNTIME_LOG_MINIO_IMAGE:?RUNTIME_LOG_MINIO_IMAGE is required}"
: "${RUNTIME_LOG_MC_IMAGE:?RUNTIME_LOG_MC_IMAGE is required}"
: "${RUNTIME_LOG_MINIO_PORT:?RUNTIME_LOG_MINIO_PORT is required}"
: "${RUNTIME_LOG_MINIO_USER:?RUNTIME_LOG_MINIO_USER is required}"
: "${RUNTIME_LOG_MINIO_PASSWORD:?RUNTIME_LOG_MINIO_PASSWORD is required}"
: "${RUNTIME_LOG_MINIO_BUCKET:?RUNTIME_LOG_MINIO_BUCKET is required}"
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

cleanup_prefix() {
    [[ $RUNTIME_LOG_S3_PREFIX == runtime-log-tests/"$RUNTIME_LOG_HARNESS_OWNER" ]] || {
        printf 'refusing broad S3 cleanup for prefix %s\n' "$RUNTIME_LOG_S3_PREFIX" >&2
        exit 1
    }
    docker run --rm --network host --entrypoint /bin/sh \
        -e MC_ENDPOINT="${ATTUNE_TEST_S3_ENDPOINT:-http://127.0.0.1:$RUNTIME_LOG_MINIO_PORT}" \
        -e MC_USER="$RUNTIME_LOG_MINIO_USER" \
        -e MC_PASSWORD="$RUNTIME_LOG_MINIO_PASSWORD" \
        -e MC_BUCKET="$RUNTIME_LOG_MINIO_BUCKET" \
        -e MC_PREFIX="$RUNTIME_LOG_S3_PREFIX" \
        "$RUNTIME_LOG_MC_IMAGE" -c \
        'mc alias set attune "$MC_ENDPOINT" "$MC_USER" "$MC_PASSWORD" >/dev/null && mc rm --recursive --force --versions "attune/$MC_BUCKET/$MC_PREFIX" >/dev/null'
}

case "$action" in
    up)
        refuse_foreign_resource container "$RUNTIME_LOG_MINIO_CONTAINER"
        refuse_foreign_resource network "$RUNTIME_LOG_MINIO_NETWORK"
        if owned_resource container "$RUNTIME_LOG_MINIO_CONTAINER"; then
            docker rm -f "$RUNTIME_LOG_MINIO_CONTAINER" >/dev/null
        fi
        if ! owned_resource network "$RUNTIME_LOG_MINIO_NETWORK"; then
            docker network create \
                --label "$label_key=$RUNTIME_LOG_HARNESS_OWNER" \
                "$RUNTIME_LOG_MINIO_NETWORK" >/dev/null
        fi
        docker run -d --name "$RUNTIME_LOG_MINIO_CONTAINER" \
            --label "$label_key=$RUNTIME_LOG_HARNESS_OWNER" \
            --network "$RUNTIME_LOG_MINIO_NETWORK" \
            -p "$RUNTIME_LOG_MINIO_PORT:9000" \
            -e MINIO_ROOT_USER="$RUNTIME_LOG_MINIO_USER" \
            -e MINIO_ROOT_PASSWORD="$RUNTIME_LOG_MINIO_PASSWORD" \
            "$RUNTIME_LOG_MINIO_IMAGE" server /data >/dev/null
        docker run --rm --network "$RUNTIME_LOG_MINIO_NETWORK" --entrypoint /bin/sh \
            -e MINIO_ROOT_USER="$RUNTIME_LOG_MINIO_USER" \
            -e MINIO_ROOT_PASSWORD="$RUNTIME_LOG_MINIO_PASSWORD" \
            -e MINIO_CONTAINER="$RUNTIME_LOG_MINIO_CONTAINER" \
            -e MINIO_BUCKET="$RUNTIME_LOG_MINIO_BUCKET" \
            "$RUNTIME_LOG_MC_IMAGE" -c \
            'until mc alias set attune "http://$MINIO_CONTAINER:9000" "$MINIO_ROOT_USER" "$MINIO_ROOT_PASSWORD" >/dev/null 2>&1; do sleep 1; done; mc mb --ignore-existing "attune/$MINIO_BUCKET" >/dev/null; mc version enable "attune/$MINIO_BUCKET" >/dev/null'
        ;;
    clean-prefix)
        cleanup_prefix
        ;;
    down)
        refuse_foreign_resource container "$RUNTIME_LOG_MINIO_CONTAINER"
        refuse_foreign_resource network "$RUNTIME_LOG_MINIO_NETWORK"
        if owned_resource container "$RUNTIME_LOG_MINIO_CONTAINER"; then
            if [[ $(docker inspect "$RUNTIME_LOG_MINIO_CONTAINER" --format '{{.State.Running}}') == true ]]; then
                cleanup_prefix
            fi
            docker rm -f "$RUNTIME_LOG_MINIO_CONTAINER" >/dev/null
        fi
        if owned_resource network "$RUNTIME_LOG_MINIO_NETWORK"; then
            docker network rm "$RUNTIME_LOG_MINIO_NETWORK" >/dev/null
        fi
        ;;
    *)
        printf 'usage: %s {up|clean-prefix|down}\n' "$0" >&2
        exit 2
        ;;
esac
