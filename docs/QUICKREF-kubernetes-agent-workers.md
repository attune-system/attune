# Quick Reference: Kubernetes Agent Workers

Agent-based workers let you run Attune actions inside **any container image** by injecting a statically-linked `attune-agent` binary via a Kubernetes init container. No custom Dockerfile required — just point at an image that has your runtime installed.

## ⚠️ Required: Bootstrap Token

The API endpoint that serves the agent binary (`GET /api/v1/agent/binary`) **requires** a bootstrap token (`agent.bootstrap_token` in API config). The API will refuse to start without it, and download requests without a valid `X-Agent-Token` header are rejected with HTTP 503/401.

```bash
# Generate a strong token and store it as a Kubernetes secret
openssl rand -hex 32 | kubectl create secret generic attune-agent-token \
  --from-file=token=/dev/stdin
```

Reference the token from your API Helm values (e.g., `api.env.ATTUNE__AGENT__BOOTSTRAP_TOKEN`) and from each agent worker's environment (`ATTUNE_AGENT_TOKEN`) so the bootstrap script can authenticate.

When agent workers use the standard Kubernetes init-container pattern below, the binary is copied directly from the `attune-agent` image into an `emptyDir` volume — no HTTP download is performed, so the token is only required for agents bootstrapping over the network.

## How It Works

1. An **init container** (`agent-loader`) copies the `attune-agent` binary from the `attune-agent` image into an `emptyDir` volume
2. The **worker container** uses your chosen image (e.g., `ruby:3.3`) and runs the agent binary as its entrypoint
3. The agent **auto-detects** available runtimes (python, ruby, node, shell, etc.) and registers with Attune
4. Actions targeting those runtimes are routed to the agent worker via RabbitMQ

## Helm Values

Add entries to `agentWorkers` in your `values.yaml`:

```yaml
agentWorkers:
  - name: ruby
    image: ruby:3.3
    replicas: 2

  - name: python-gpu
    image: nvidia/cuda:12.3.1-runtime-ubuntu22.04
    replicas: 1
    runtimes: [python, shell]
    runtimeClassName: nvidia
    nodeSelector:
      gpu: "true"
    tolerations:
      - key: nvidia.com/gpu
        operator: Exists
        effect: NoSchedule
    resources:
      limits:
        nvidia.com/gpu: 1

  - name: custom
    image: my-org/my-custom-image:latest
    replicas: 1
    env:
      - name: MY_CUSTOM_VAR
        value: my-value
```

### Supported Fields

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `name` | Yes | — | Unique name (used in Deployment and worker names) |
| `image` | Yes | — | Container image with your desired runtime(s) |
| `replicas` | No | `1` | Number of pod replicas |
| `runtimes` | No | `[]` (auto-detect) | List of runtimes to expose (e.g., `[python, shell]`) |
| `resources` | No | `{}` | Kubernetes resource requests/limits |
| `env` | No | — | Extra environment variables (`[{name, value}]`) |
| `imagePullPolicy` | No | — | Pull policy for the worker image |
| `logLevel` | No | `info` | `RUST_LOG` level |
| `runtimeClassName` | No | — | Kubernetes RuntimeClass (e.g., `nvidia`) |
| `nodeSelector` | No | — | Node selector for pod scheduling |
| `tolerations` | No | — | Tolerations for pod scheduling |
| `stopGracePeriod` | No | `45` | Termination grace period (seconds) |

## Install / Upgrade

```bash
: "${ATTUNE_VERSION:?Set ATTUNE_VERSION to the release version}"
helm upgrade --install attune oci://registry.example.com/namespace/helm/attune \
  --version "$ATTUNE_VERSION" \
  --set global.imageRegistry=registry.example.com \
  --set global.imageNamespace=namespace \
  --set global.imageTag="$ATTUNE_VERSION" \
  -f my-values.yaml
```

## What Gets Created

For each `agentWorkers` entry, the chart creates a Deployment named `<release>-attune-agent-worker-<name>` with:

- **Init containers**:
  - `agent-loader` — copies the agent binary from the `attune-agent` image to an `emptyDir` volume
  - `wait-for-schema` — polls PostgreSQL until the Attune schema is ready
  - `wait-for-packs` — waits for the core pack to be available on the shared PVC
- **Worker container** — runs `attune-agent` as the entrypoint inside your chosen image
- **Volumes**: `agent-bin` (emptyDir), `config` (ConfigMap), `packs` (PVC, read-only), `runtime-envs` (PVC), `artifacts` (PVC)

## Runtime Auto-Detection

When `runtimes` is empty (the default), the agent probes the container for interpreters:

| Runtime | Probed Binaries |
|---------|----------------|
| Shell | `bash`, `sh` |
| Python | `python3`, `python` |
| Node.js | `node`, `nodejs` |
| Ruby | `ruby` |
| Go | `go` |
| Java | `java` |
| R | `Rscript` |
| Perl | `perl` |

Set `runtimes` explicitly to skip auto-detection and only register the listed runtimes.

## Prerequisites

- The `attune-agent` image must be available in your registry (built from `docker/Dockerfile.agent`, target `agent-init`)
- Shared PVCs (`packs`, `runtime-envs`, `artifacts`) must support `ReadWriteMany` if agent workers run on different nodes than the standard worker
- The Attune database and RabbitMQ must be reachable from agent worker pods

## Differences from the Standard Worker

| Aspect | Standard Worker (`worker`) | Agent Worker (`agentWorkers`) |
|--------|---------------------------|-------------------------------|
| Image | Built from `Dockerfile.worker.optimized` | Any image (ruby, python, cuda, etc.) |
| Binary | Baked into the image | Injected via init container |
| Runtimes | Configured at build time | Auto-detected or explicitly listed |
| Use case | Known, pre-built runtime combos | Custom images, exotic runtimes, GPU |

Both worker types coexist — actions are routed to whichever worker has the matching runtime registered.

## Troubleshooting

**Agent binary not found**: Check that the `agent-loader` init container completed. View its logs:
```bash
kubectl logs <pod> -c agent-loader
```

**Runtime not detected**: Run the agent with `--detect-only` to see what it finds:
```bash
kubectl exec <pod> -c worker -- /opt/attune/agent/attune-agent --detect-only
```

**Worker not registering**: Check the worker container logs for database/MQ connectivity:
```bash
kubectl logs <pod> -c worker
```

**Packs not available**: Ensure the `init-packs` job has completed and the PVC is mounted:
```bash
kubectl get jobs | grep init-packs
kubectl exec <pod> -c worker -- ls /opt/attune/packs/core/
```

## See Also

- [Agent Workers (Docker Compose)](QUICKREF-agent-workers.md)
- [Universal Worker Agent Plan](plans/universal-worker-agent.md)
- [GitHub Packages and Helm](deployment/github-packages-and-helm.md)
- [Production Deployment](deployment/production-deployment.md)
