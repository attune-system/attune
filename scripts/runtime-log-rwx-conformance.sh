#!/usr/bin/env bash
set -Eeuo pipefail

confirmation_value=delete-attune-rwx-conformance-namespace

preflight_failure() {
    local code=$1
    local message=$2
    printf '%s\n' "$message" >&2
    printf '{"schema_version":1,"status":"failed","deployed":false,"error":"%s"}\n' "$code"
    exit 2
}

if [[ ${1:-} == --validate-only ]]; then
    [[ $confirmation_value == delete-attune-rwx-conformance-namespace ]]
    printf '{"schema_version":1,"status":"validated","deployed":false}\n'
    exit 0
fi

context=
namespace=
storage_class=
confirmation=
while (( $# > 0 )); do
    case "$1" in
        --context) context=${2:-}; shift 2 ;;
        --namespace) namespace=${2:-}; shift 2 ;;
        --storage-class) storage_class=${2:-}; shift 2 ;;
        --confirm) confirmation=${2:-}; shift 2 ;;
        *) preflight_failure unknown_argument "unknown argument: $1" ;;
    esac
done

[[ $context =~ ^[A-Za-z0-9._:@/-]+$ ]] || {
    preflight_failure invalid_context '--context is required and contains unsupported characters'
}
[[ $namespace =~ ^attune-rwx-conformance-[a-z0-9]([-a-z0-9]*[a-z0-9])?$ ]] || {
    preflight_failure invalid_namespace '--namespace must be a new name beginning with attune-rwx-conformance-'
}
[[ $storage_class =~ ^[a-z0-9]([-a-z0-9.]*[a-z0-9])?$ ]] || {
    preflight_failure invalid_storage_class '--storage-class is required and must be a Kubernetes DNS name'
}
[[ $confirmation == "$confirmation_value" ]] || {
    preflight_failure confirmation_required "--confirm must equal $confirmation_value"
}
command -v kubectl >/dev/null || preflight_failure kubectl_not_found 'kubectl is required'

status=failed
step=preflight
namespace_created=false
manifest=$(mktemp "${TMPDIR:-/tmp}/attune-rwx-conformance.XXXXXX.yaml")

cleanup() {
    local command_status=$?
    trap - EXIT ERR
    rm -f "$manifest"
    if [[ $namespace_created == true ]]; then
        if ! kubectl --context "$context" delete namespace "$namespace" \
            --wait=true --timeout=120s >/dev/null; then
            status=failed
            step=namespace_cleanup
            command_status=1
        fi
    fi
    printf '{"schema_version":1,"status":"%s","deployed":true,"context":"%s","namespace":"%s","storage_class":"%s","last_step":"%s","checks":{"cross_pod_visibility":%s,"flock_exclusion":%s,"sigkill_lock_release":%s,"truncation_visibility":%s,"file_cleanup":%s,"namespace_cleanup":%s}}\n' \
        "$status" "$context" "$namespace" "$storage_class" "$step" \
        "$cross_pod_visibility" "$flock_exclusion" "$sigkill_lock_release" \
        "$truncation_visibility" "$file_cleanup" \
        "$([[ $step != namespace_cleanup ]] && printf true || printf false)"
    if [[ $status == passed && $command_status -eq 0 ]]; then
        exit 0
    fi
    exit 1
}
trap cleanup EXIT
trap 'status=failed' ERR

cross_pod_visibility=false
flock_exclusion=false
sigkill_lock_release=false
truncation_visibility=false
file_cleanup=false

if kubectl --context "$context" get namespace "$namespace" >/dev/null 2>&1; then
    printf 'refusing to use existing namespace %s\n' "$namespace" >&2
    exit 1
fi
kubectl --context "$context" get storageclass "$storage_class" >/dev/null
kubectl --context "$context" create namespace "$namespace" >/dev/null
namespace_created=true

cat >"$manifest" <<EOF
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: runtime-log-rwx
  namespace: $namespace
spec:
  accessModes: [ReadWriteMany]
  storageClassName: $storage_class
  resources:
    requests:
      storage: 1Gi
---
apiVersion: v1
kind: Pod
metadata:
  name: writer
  namespace: $namespace
spec:
  restartPolicy: Never
  containers:
    - name: writer
      image: busybox:1.37.0
      command: ["sh", "-c", "printf initial >/rwx/log; exec 9>/rwx/lock; flock -x 9; printf locked >/rwx/writer-ready; while :; do sleep 60; done"]
      volumeMounts:
        - {name: rwx, mountPath: /rwx}
  volumes:
    - name: rwx
      persistentVolumeClaim: {claimName: runtime-log-rwx}
---
apiVersion: v1
kind: Pod
metadata:
  name: reader
  namespace: $namespace
spec:
  restartPolicy: Never
  containers:
    - name: reader
      image: busybox:1.37.0
      command: ["sh", "-c", "while :; do sleep 60; done"]
      volumeMounts:
        - {name: rwx, mountPath: /rwx}
  volumes:
    - name: rwx
      persistentVolumeClaim: {claimName: runtime-log-rwx}
EOF

step=create_writer_and_reader
kubectl --context "$context" apply -f "$manifest" >/dev/null
kubectl --context "$context" -n "$namespace" wait pod/writer pod/reader \
    --for=condition=Ready --timeout=180s >/dev/null
kubectl --context "$context" -n "$namespace" exec reader -- timeout 180 sh -c \
    'until test -f /rwx/writer-ready; do sleep 1; done; test "$(cat /rwx/log)" = initial'
cross_pod_visibility=true

step=flock_exclusion
if kubectl --context "$context" -n "$namespace" exec reader -- \
    flock -n /rwx/lock -c 'exit 0' >/dev/null 2>&1; then
    printf '%s\n' 'reader unexpectedly acquired the writer lock' >&2
    exit 1
fi
flock_exclusion=true

step=kill_writer
kubectl --context "$context" -n "$namespace" delete pod writer \
    --grace-period=0 --force --wait=true >/dev/null

cat >"$manifest" <<EOF
apiVersion: v1
kind: Pod
metadata:
  name: lock-reaper
  namespace: $namespace
spec:
  restartPolicy: Never
  containers:
    - name: lock-reaper
      image: busybox:1.37.0
      command: ["sh", "-c", "flock -x /rwx/lock -c 'printf x >/rwx/log; printf released >/rwx/reaper-ready'; while :; do sleep 60; done"]
      volumeMounts:
        - {name: rwx, mountPath: /rwx}
  volumes:
    - name: rwx
      persistentVolumeClaim: {claimName: runtime-log-rwx}
EOF
kubectl --context "$context" apply -f "$manifest" >/dev/null
kubectl --context "$context" -n "$namespace" wait pod/lock-reaper \
    --for=condition=Ready --timeout=180s >/dev/null
kubectl --context "$context" -n "$namespace" exec reader -- timeout 180 sh -c \
    'until test -f /rwx/reaper-ready; do sleep 1; done'
sigkill_lock_release=true

step=truncation_visibility
kubectl --context "$context" -n "$namespace" exec reader -- sh -c \
    'test "$(cat /rwx/log)" = x'
truncation_visibility=true

step=file_cleanup
kubectl --context "$context" -n "$namespace" exec reader -- sh -c \
    'rm -f /rwx/log /rwx/lock /rwx/writer-ready /rwx/reaper-ready; test -z "$(find /rwx -mindepth 1 -maxdepth 1 -print -quit)"'
file_cleanup=true
status=passed
step=complete
