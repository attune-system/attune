# Retired standalone launch-controller candidate

The separate controller/frontend/TLS-proxy deployment was withdrawn before
activation. Use the in-process Docker and native Kubernetes effects described in
[workload isolation](workload-isolation.md). No external Docker service is
required for Kubernetes. The existing trusted worker/sensor loops retain domain
policy and custody; the adapters own only exact container/Pod effects.

This is not a compatibility path. Old controller images, sessions, Compose
projects and HTTP dispatch configuration do not implement the selected design.
The replacement remains work in progress and is not a deployment readiness claim.
