//! Native, namespace-scoped Pod effects. Admission and retries belong to the
//! existing trusted worker. No Docker service, Job, operator or HTTP relay.
use crate::{
    cache::CacheStore,
    process_policy::{invalid, pinned_image, public_digest},
    runner_protocol::encode_start,
};
use async_trait::async_trait;
use attune_common::{
    isolated_process::{
        ProcessControl, ProcessCreateDisposition, ProcessIo, ProcessLauncher, ProcessSpec,
        ResourceIdentity, PAYLOAD_CONTAINER, TRANSFER_CONTAINER,
    },
    workload_launch::{
        LaunchExitOutcome, LaunchPlatform, LaunchStopReason, PackAccess, PinnedImage,
    },
    workload_stdin::LaunchStdinPayload,
    Result,
};
use k8s_openapi::api::core::v1::Pod;
use kube::{
    api::{AttachParams, DeleteParams, PostParams, Preconditions},
    Api, Client,
};
use serde_json::json;
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    sync::Mutex,
};

pub const BOOTSTRAP_ROOT: &str = "/opt/attune/bootstrap";
pub const BOOTSTRAP_RUNNER: &str = "/opt/attune/bootstrap/runner";
const OWNER: &str = "attune.io/process-intent";
const PROOF: &str = "attune.io/process-policy";
/// Operator-owned configuration. Never deserialized from pack metadata.
#[derive(Clone)]
pub struct KubernetesPolicy {
    pub cluster: String,
    pub namespace: String,
    pub helper_image: PinnedImage,
    pub allowed_images: BTreeSet<String>,
    pub memory_bytes: u64,
    /// Retained-tree bounds are separate; emptyDir sizeLimit is not a hard
    /// live per-workload filesystem quota on every Kubernetes storage driver.
    pub scratch_bytes: u64,
}
impl KubernetesPolicy {
    pub fn pod(&self, spec: &ProcessSpec) -> Result<Pod> {
        self.build_pod(spec, true)
    }
    fn build_pod(&self, spec: &ProcessSpec, admission: bool) -> Result<Pod> {
        let digest = public_digest(spec)?;
        if self.cluster.is_empty()
            || !dns_label(&self.namespace)
            || !pinned_image(&self.helper_image)
            || (admission && !self.allowed_images.contains(&spec.image.reference))
            || self.memory_bytes < 32 * 1024 * 1024
            || self.scratch_bytes == 0
        {
            return Err(invalid());
        }
        let arch = match spec.platform {
            LaunchPlatform::LinuxAmd64 => "amd64",
            LaunchPlatform::LinuxArm64 => "arm64",
        };
        let mut mounts = vec![
            json!({"name":"bootstrap","mountPath":BOOTSTRAP_ROOT,"readOnly":true}),
            json!({"name":"scratch","mountPath":"/tmp"}),
            json!({"name":"artifacts","mountPath":"/tmp/artifacts"}),
        ];
        let mut transfer_mounts = vec![
            json!({"name":"bootstrap","mountPath":BOOTSTRAP_ROOT,"readOnly":true}),
            json!({"name":"transfer-state","mountPath":"/opt/attune/transfer"}),
            json!({"name":"transfer-tmp","mountPath":"/tmp"}),
            json!({"name":"artifacts","mountPath":"/tmp/artifacts"}),
        ];
        let mut volumes = vec![
            json!({"name":"bootstrap","emptyDir":{}}),
            json!({"name":"transfer-state","emptyDir":{}}),
            json!({"name":"transfer-tmp","emptyDir":{"sizeLimit":quantity_bytes(self.scratch_bytes)}}),
            json!({"name":"scratch","emptyDir":{"sizeLimit":quantity_bytes(self.scratch_bytes)}}),
            json!({"name":"artifacts","emptyDir":{"sizeLimit":quantity_bytes(self.scratch_bytes)}}),
        ];
        for pin in &spec.inputs {
            let name = match pin.kind {
                attune_common::workload_launch::LaunchInputKind::Pack => "pack",
                attune_common::workload_launch::LaunchInputKind::Runtime => "runtime",
                attune_common::workload_launch::LaunchInputKind::Runner => "runner",
            };
            volumes.push(json!({"name":name,"emptyDir":{}}));
            mounts.push(json!({"name":name,"mountPath":pin.kind.container_root(),"readOnly": !(name=="pack" && spec.pack_access==PackAccess::CandidateCopy)}));
            transfer_mounts.push(json!({"name":name,"mountPath":pin.kind.container_root()}));
        }
        if let Some(stage) = &spec.stage {
            volumes.push(
                json!({"name":"runtime","emptyDir":{"sizeLimit":quantity_bytes(stage.quota_bytes)}}),
            );
            mounts.push(json!({"name":"runtime","mountPath":"/opt/attune/runtime"}));
            transfer_mounts.push(json!({"name":"runtime","mountPath":"/opt/attune/runtime"}));
        }
        let security = json!({"runAsUser":1000,"runAsGroup":1000,"runAsNonRoot":true,"allowPrivilegeEscalation":false,
            "readOnlyRootFilesystem":true,"privileged":false,"capabilities":{"drop":["ALL"]},"seccompProfile":{"type":"RuntimeDefault"}});
        let resources = json!({"limits":{"memory":quantity_bytes(self.memory_bytes),"cpu":"1"},"requests":{"memory":quantity_bytes(self.memory_bytes),"cpu":"1"}});
        serde_json::from_value(json!({"apiVersion":"v1","kind":"Pod","metadata":{"name":pod_name(spec),"namespace":self.namespace,
            "labels":{OWNER:spec.intent.to_string()},"annotations":{PROOF:digest.as_str()}},"spec":{
            "restartPolicy":"Never","automountServiceAccountToken":false,"enableServiceLinks":false,
            "hostPID":false,"hostIPC":false,"hostNetwork":false,"shareProcessNamespace":false,
            "securityContext":{"runAsUser":1000,"runAsGroup":1000,"runAsNonRoot":true,"fsGroup":1000,"seccompProfile":{"type":"RuntimeDefault"}},
            "nodeSelector":{"kubernetes.io/os":"linux","kubernetes.io/arch":arch},"terminationGracePeriodSeconds":10,
            "volumes":volumes,"initContainers":[{"name":"bootstrap","image":self.helper_image.reference,"imagePullPolicy":"IfNotPresent",
                "command":["/usr/local/bin/attune-workload-runner","install"],"securityContext":security,"resources":resources,
                "volumeMounts":[{"name":"bootstrap","mountPath":BOOTSTRAP_ROOT}]}],
            "containers":[{"name":PAYLOAD_CONTAINER,"image":spec.image.reference,"imagePullPolicy":"IfNotPresent",
                "command":[BOOTSTRAP_RUNNER,"gated"],"stdin":true,"stdinOnce":true,"tty":false,
                "securityContext":security,"resources":resources,"volumeMounts":mounts},
                {"name":TRANSFER_CONTAINER,"image":self.helper_image.reference,"imagePullPolicy":"IfNotPresent",
                "command":[BOOTSTRAP_RUNNER,"transfer"],"securityContext":security,"resources":resources,"volumeMounts":transfer_mounts}]
        }})).map_err(|_| invalid())
    }
}
fn quantity_bytes(mut value: u64) -> String {
    let units = ["", "k", "M", "G", "T", "P", "E"];
    let mut index = 0;
    while value > 0 && value.is_multiple_of(1000) && index + 1 < units.len() {
        value /= 1000;
        index += 1;
    }
    format!("{value}{}", units[index])
}
fn dns_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !value.starts_with('-')
        && !value.ends_with('-')
}
fn pod_name(spec: &ProcessSpec) -> String {
    format!("attune-{}", spec.intent.simple())
}
#[derive(Clone)]
pub struct KubernetesLauncher {
    client: Client,
    policy: KubernetesPolicy,
    cache: Arc<CacheStore>,
}
impl KubernetesLauncher {
    pub fn new(client: Client, policy: KubernetesPolicy, cache: Arc<CacheStore>) -> Self {
        Self {
            client,
            policy,
            cache,
        }
    }
    fn pods(&self) -> Api<Pod> {
        Api::namespaced(self.client.clone(), &self.policy.namespace)
    }
    fn verify(&self, spec: &ProcessSpec, pod: &Pod, uid: Option<&str>) -> Result<ResourceIdentity> {
        let expected = self.policy.build_pod(spec, false)?;
        if pod.metadata.name != expected.metadata.name
            || pod.metadata.namespace != expected.metadata.namespace
            || pod.metadata.uid.as_deref().is_none_or(str::is_empty)
            || uid.is_some_and(|u| pod.metadata.uid.as_deref() != Some(u))
            || pod.metadata.deletion_timestamp.is_some()
            || pod.metadata.labels.as_ref().and_then(|l| l.get(OWNER))
                != expected.metadata.labels.as_ref().and_then(|l| l.get(OWNER))
            || pod.metadata.annotations.as_ref().and_then(|l| l.get(PROOF))
                != expected
                    .metadata
                    .annotations
                    .as_ref()
                    .and_then(|l| l.get(PROOF))
        {
            return Err(invalid());
        }
        verify_pod_policy(
            pod.spec.as_ref().ok_or_else(invalid)?,
            expected.spec.as_ref().ok_or_else(invalid)?,
        )?;
        Ok(ResourceIdentity::Kubernetes {
            cluster: self.policy.cluster.clone(),
            namespace: self.policy.namespace.clone(),
            pod_name: pod_name(spec),
            pod_uid: pod.metadata.uid.clone().ok_or_else(invalid)?,
        })
    }
    async fn exact(&self, spec: &ProcessSpec, resource: &ResourceIdentity) -> Result<Pod> {
        let ResourceIdentity::Kubernetes {
            cluster,
            namespace,
            pod_name: name,
            pod_uid,
        } = resource
        else {
            return Err(invalid());
        };
        if cluster != &self.policy.cluster
            || namespace != &self.policy.namespace
            || name != &pod_name(spec)
        {
            return Err(invalid());
        }
        let pod = self.pods().get(name).await.map_err(|_| invalid())?;
        self.verify(spec, &pod, Some(pod_uid))?;
        Ok(pod)
    }
    /// Original persisted ownership is enough for stop/removal after policy or
    /// helper-image updates. It never permits attach, stdin or a new start.
    async fn cleanup_exact(&self, spec: &ProcessSpec, resource: &ResourceIdentity) -> Result<Pod> {
        let ResourceIdentity::Kubernetes {
            cluster,
            namespace,
            pod_name: name,
            pod_uid,
        } = resource
        else {
            return Err(invalid());
        };
        if cluster != &self.policy.cluster
            || namespace != &self.policy.namespace
            || name != &pod_name(spec)
        {
            return Err(invalid());
        }
        let pod = self.pods().get(name).await.map_err(|_| invalid())?;
        let digest = public_digest(spec)?;
        if pod.metadata.uid.as_deref() != Some(pod_uid)
            || pod.metadata.labels.as_ref().and_then(|v| v.get(OWNER))
                != Some(&spec.intent.to_string())
            || pod
                .metadata
                .annotations
                .as_ref()
                .and_then(|v| v.get(PROOF))
                .map(String::as_str)
                != Some(digest.as_str())
        {
            return Err(invalid());
        }
        Ok(pod)
    }
    /// The trusted owning loop first commits terminal state/custody and proves
    /// the ordinary-file inventory empty or durably published. This method is
    /// only the exact provider effect, never a custody/publication ACK.
    pub async fn remove_stopped(
        &self,
        spec: &ProcessSpec,
        resource: &ResourceIdentity,
    ) -> Result<()> {
        let pod = self.cleanup_exact(spec, resource).await?;
        if !pod
            .status
            .as_ref()
            .and_then(|s| s.container_statuses.as_ref())
            .is_some_and(|ss| {
                ss.iter().any(|s| {
                    s.name == PAYLOAD_CONTAINER
                        && s.state.as_ref().is_some_and(|s| s.terminated.is_some())
                })
            })
        {
            return Err(invalid());
        }
        let ResourceIdentity::Kubernetes {
            pod_name, pod_uid, ..
        } = resource
        else {
            return Err(invalid());
        };
        // A positive UID-preconditioned Delete acknowledgement precedes all
        // absence observations. Timeout/404 before ACK remains unknown.
        self.pods()
            .delete(
                pod_name,
                &DeleteParams {
                    preconditions: Some(Preconditions {
                        uid: Some(pod_uid.clone()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await
            .map_err(|_| invalid())?;
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                match self.pods().get_opt(pod_name).await.map_err(|_| invalid())? {
                    None => return Ok(()),
                    Some(pod) if pod.metadata.uid.as_deref() != Some(pod_uid) => {
                        return Err(invalid())
                    }
                    _ => {}
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .map_err(|_| invalid())?
    }
    async fn ready(&self, spec: &ProcessSpec, resource: &ResourceIdentity) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                let pod = self.exact(spec, resource).await?;
                let statuses = pod
                    .status
                    .and_then(|s| s.container_statuses)
                    .unwrap_or_default();
                if statuses.iter().any(|s| {
                    s.name == PAYLOAD_CONTAINER
                        && s.state.as_ref().is_some_and(|s| s.terminated.is_some())
                }) {
                    return Err(invalid());
                }
                if [PAYLOAD_CONTAINER, TRANSFER_CONTAINER].iter().all(|name| {
                    statuses.iter().any(|s| {
                        s.name == *name && s.state.as_ref().is_some_and(|s| s.running.is_some())
                    })
                }) {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        })
        .await
        .map_err(|_| invalid())?
    }
    async fn seed(&self, spec: &ProcessSpec) -> Result<()> {
        for pin in &spec.inputs {
            let cache = self.cache.clone();
            let selected = pin.clone();
            let file = tokio::task::spawn_blocking(move || cache.open_input_archive(&selected))
                .await
                .map_err(|_| invalid())?
                .map_err(|_| invalid())?;
            let args = vec![
                BOOTSTRAP_RUNNER.into(),
                "seed".into(),
                match pin.kind {
                    attune_common::workload_launch::LaunchInputKind::Pack => "pack",
                    attune_common::workload_launch::LaunchInputKind::Runtime => "runtime",
                    attune_common::workload_launch::LaunchInputKind::Runner => "runner",
                }
                .into(),
                pin.sha256.as_str().into(),
                pin.size_bytes.to_string(),
                pin.max_expanded_bytes.to_string(),
                pin.max_files.to_string(),
            ];
            let mut streams = crate::kubernetes_io::exec(
                &self.client,
                &self.pods(),
                &pod_name(spec),
                TRANSFER_CONTAINER,
                args,
            )
            .await?;
            let mut file = tokio::fs::File::from_std(file);
            let writing = async {
                tokio::io::copy(&mut file, &mut streams.stdin)
                    .await
                    .map_err(|_| invalid())?;
                streams.stdin.shutdown().await.map_err(|_| invalid())
            };
            let mut out_sink = tokio::io::sink();
            let out = tokio::io::copy(&mut streams.stdout, &mut out_sink);
            let mut err_sink = tokio::io::sink();
            let err = tokio::io::copy(&mut streams.stderr, &mut err_sink);
            let (written, out, err) = tokio::join!(writing, out, err);
            written?;
            out.map_err(|_| invalid())?;
            err.map_err(|_| invalid())?;
            streams.success().await?;
        }
        Ok(())
    }
}
#[async_trait]
impl ProcessLauncher for KubernetesLauncher {
    async fn prepare(
        &self,
        spec: &ProcessSpec,
        create: ProcessCreateDisposition,
    ) -> Result<ResourceIdentity> {
        if create == ProcessCreateDisposition::ReconcileOnly {
            // Discovery recovers original public ownership even after image or
            // helper policy changes. It grants no attach/start/stdin effect.
            let pod = self
                .pods()
                .get_opt(&pod_name(spec))
                .await
                .map_err(|_| invalid())?
                .ok_or_else(invalid)?;
            let resource = ResourceIdentity::Kubernetes {
                cluster: self.policy.cluster.clone(),
                namespace: self.policy.namespace.clone(),
                pod_name: pod_name(spec),
                pod_uid: pod
                    .metadata
                    .uid
                    .filter(|s| !s.is_empty())
                    .ok_or_else(invalid)?,
            };
            self.cleanup_exact(spec, &resource).await?;
            return Ok(resource);
        }
        let expected = self.policy.pod(spec)?;
        match self
            .pods()
            .get_opt(&pod_name(spec))
            .await
            .map_err(|_| invalid())?
        {
            Some(pod) => self.verify(spec, &pod, None),
            None => {
                let pod = self
                    .pods()
                    .create(&PostParams::default(), &expected)
                    .await
                    .map_err(|_| invalid())?;
                self.verify(spec, &pod, None)
            }
        }
    }
    async fn start(&self, spec: &ProcessSpec, resource: &ResourceIdentity) -> Result<ProcessIo> {
        self.policy.pod(spec)?; // Current operator image ceiling for new Run.
        crate::process_policy::validate_invocation(spec)?;
        self.ready(spec, resource).await?;
        self.seed(spec).await?;
        self.exact(spec, resource).await?;
        let streams =
            crate::kubernetes_io::attach(&self.client, &self.pods(), &pod_name(spec)).await?;
        Ok(ProcessIo {
            resource: resource.clone(),
            stdout: streams.stdout,
            stderr: streams.stderr,
            control: Arc::new(PodControl {
                launcher: self.clone(),
                spec: spec.clone(),
                resource: resource.clone(),
                stdin: Mutex::new(Some(streams.stdin)),
                header: encode_start(&spec.invocation).map_err(|_| invalid())?,
            }),
        })
    }
}
struct PodControl {
    launcher: KubernetesLauncher,
    spec: ProcessSpec,
    resource: ResourceIdentity,
    stdin: Mutex<Option<Box<dyn AsyncWrite + Unpin + Send>>>,
    header: Vec<u8>,
}
#[async_trait]
impl ProcessControl for PodControl {
    async fn write_stdin_once(&self, payload: LaunchStdinPayload) -> Result<()> {
        self.launcher.exact(&self.spec, &self.resource).await?;
        let mut writer = self.stdin.lock().await.take().ok_or_else(invalid)?;
        writer
            .write_all(&self.header)
            .await
            .map_err(|_| invalid())?;
        writer
            .write_all(payload.as_bytes())
            .await
            .map_err(|_| invalid())?;
        writer.shutdown().await.map_err(|_| invalid())
    }
    async fn wait(&self) -> Result<LaunchExitOutcome> {
        loop {
            let pod = self
                .launcher
                .cleanup_exact(&self.spec, &self.resource)
                .await?;
            if let Some(terminal) = pod
                .status
                .and_then(|s| s.container_statuses)
                .unwrap_or_default()
                .into_iter()
                .find(|s| s.name == PAYLOAD_CONTAINER)
                .and_then(|s| s.state)
                .and_then(|s| s.terminated)
            {
                return Ok(LaunchExitOutcome {
                    exit_code: terminal.exit_code,
                    oom_killed: terminal.reason.as_deref() == Some("OOMKilled"),
                });
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    async fn stop(&self, _: LaunchStopReason) -> Result<LaunchExitOutcome> {
        self.launcher
            .cleanup_exact(&self.spec, &self.resource)
            .await?;
        self.stdin.lock().await.take();
        // The fixed helper kills only the payload PID namespace. Namespace
        // teardown may close exec before its reply; only the subsequent exact
        // payload terminated observation proves stop, never this exec response.
        let signal = async {
            let mut process = self
                .launcher
                .pods()
                .exec(
                    &pod_name(&self.spec),
                    [BOOTSTRAP_RUNNER, "terminate"],
                    &AttachParams {
                        container: Some(PAYLOAD_CONTAINER.into()),
                        stdout: false,
                        stderr: false,
                        tty: false,
                        ..Default::default()
                    },
                )
                .await
                .map_err(|_| invalid())?;
            let status = process.take_status().ok_or_else(invalid)?;
            let _ = status.await;
            process.join().await.map_err(|_| invalid())
        };
        let _signal_result = tokio::time::timeout(Duration::from_secs(5), signal).await;
        tokio::time::timeout(Duration::from_secs(15), self.wait())
            .await
            .map_err(|_| invalid())?
    }
}
fn verify_pod_policy(
    actual: &k8s_openapi::api::core::v1::PodSpec,
    expected: &k8s_openapi::api::core::v1::PodSpec,
) -> Result<()> {
    // Admission may supply scheduling defaults; it may not inject authority,
    // mounts, containers, env or weaken the exact process boundary.
    let mut a = serde_json::to_value(actual).map_err(|_| invalid())?;
    let mut e = serde_json::to_value(expected).map_err(|_| invalid())?;
    // Kubernetes omits false value fields on GET. Normalize only documented
    // zero-value booleans, never token automount or privilege policy defaults.
    for pod in [&mut a, &mut e] {
        for field in ["containers", "initContainers"] {
            if let Some(containers) = pod[field].as_array_mut() {
                for container in containers {
                    for field in ["stdin", "stdinOnce", "tty"] {
                        let value = container
                            .get(field)
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false);
                        container[field] = json!(value);
                    }
                    if let Some(mounts) = container["volumeMounts"].as_array_mut() {
                        for mount in mounts {
                            let value = mount
                                .get("readOnly")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(false);
                            mount["readOnly"] = json!(value);
                        }
                    }
                }
            }
        }
    }
    for field in [
        "restartPolicy",
        "automountServiceAccountToken",
        "enableServiceLinks",
        "hostPID",
        "hostIPC",
        "hostNetwork",
        "shareProcessNamespace",
        "securityContext",
        "nodeSelector",
        "volumes",
    ] {
        if ["hostPID", "hostIPC", "hostNetwork", "shareProcessNamespace"].contains(&field) {
            if a.get(field)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
            {
                return Err(invalid());
            }
        } else if a.get(field) != e.get(field) {
            return Err(invalid());
        }
    }
    for field in ["containers", "initContainers"] {
        let aa = a[field].as_array().ok_or_else(invalid)?;
        let ee = e[field].as_array().ok_or_else(invalid)?;
        if aa.len() != ee.len() {
            return Err(invalid());
        }
        for (a, e) in aa.iter().zip(ee) {
            for field in [
                "name",
                "image",
                "command",
                "args",
                "env",
                "envFrom",
                "volumeMounts",
                "volumeDevices",
                "securityContext",
                "resources",
                "stdin",
                "stdinOnce",
                "tty",
                "ports",
                "lifecycle",
            ] {
                if a.get(field) != e.get(field) {
                    return Err(invalid());
                }
            }
        }
    }
    if actual
        .ephemeral_containers
        .as_ref()
        .is_some_and(|v| !v.is_empty())
    {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::workload_launch::*;
    fn fixture() -> (KubernetesPolicy, ProcessSpec) {
        let image = PinnedImage {
            reference: format!("example.invalid/payload@sha256:{}", "a".repeat(64)),
            sha256: Sha256Digest::parse("a".repeat(64)).unwrap(),
        };
        let policy = KubernetesPolicy {
            cluster: "owned-fixture".into(),
            namespace: "attune-workloads".into(),
            helper_image: PinnedImage {
                reference: format!("example.invalid/helper@sha256:{}", "b".repeat(64)),
                sha256: Sha256Digest::parse("b".repeat(64)).unwrap(),
            },
            allowed_images: BTreeSet::from([image.reference.clone()]),
            memory_bytes: 256 * 1024 * 1024,
            scratch_bytes: 64 * 1024 * 1024,
        };
        let spec = ProcessSpec {
            intent: uuid::Uuid::new_v4(),
            purpose: LaunchPurpose::Action,
            target: LaunchTarget::Execution {
                execution: 1,
                worker: 2,
                worker_instance: uuid::Uuid::new_v4(),
                attempt: uuid::Uuid::new_v4(),
            },
            image,
            platform: LaunchPlatform::LinuxAmd64,
            inputs: vec![],
            pack_access: PackAccess::ReadOnly,
            stage: None,
            timeout_ms: Some(1000),
            invocation: LaunchInvocation {
                argv: vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "owned-private-command".into(),
                ],
                env: std::collections::BTreeMap::from([(
                    "SELECTED_SECRET".into(),
                    "owned-private-value".into(),
                )]),
                working_root: LaunchRoot::Scratch,
            },
        };
        (policy, spec)
    }
    #[test]
    fn ordinary_pod_contains_no_private_invocation_or_platform_authority() {
        let (policy, mut spec) = fixture();
        let pod = policy.pod(&spec).unwrap();
        let json = serde_json::to_string(&pod).unwrap();
        assert!(!json.contains("owned-private"));
        assert!(!json.contains("SELECTED_SECRET"));
        assert!(!json.contains("hostPath"));
        assert!(!json.contains("secretName"));
        assert!(!json.contains("projected"));
        let body = pod.spec.as_ref().unwrap();
        assert_eq!(body.restart_policy.as_deref(), Some("Never"));
        assert_eq!(body.automount_service_account_token, Some(false));
        assert_eq!(
            body.containers
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            [PAYLOAD_CONTAINER, TRANSFER_CONTAINER]
        );
        for c in &body.containers {
            let sc = c.security_context.as_ref().unwrap();
            assert_eq!(sc.run_as_user, Some(1000));
            assert_eq!(sc.read_only_root_filesystem, Some(true));
            assert_eq!(sc.allow_privilege_escalation, Some(false));
            assert_eq!(
                sc.capabilities.as_ref().unwrap().drop.as_ref().unwrap(),
                &["ALL"]
            );
            assert!(c.env.is_none());
        }
        let original = public_digest(&spec).unwrap();
        spec.invocation
            .env
            .insert("SELECTED_SECRET".into(), "different-private".into());
        assert_eq!(original, public_digest(&spec).unwrap());
        spec.platform = LaunchPlatform::LinuxArm64;
        let arm = policy.pod(&spec).unwrap();
        assert_eq!(
            arm.spec.unwrap().node_selector.unwrap()["kubernetes.io/arch"],
            "arm64"
        );
    }
    #[test]
    fn effective_policy_rejects_injected_credentials_mounts_capabilities_and_shared_namespaces() {
        let (policy, spec) = fixture();
        let expected = policy.pod(&spec).unwrap().spec.unwrap();
        for mutation in 0..9 {
            let mut bad = serde_json::to_value(&expected).unwrap();
            match mutation {
                0 => bad["hostPID"] = json!(true),
                1 => bad["hostNetwork"] = json!(true),
                2 => bad["shareProcessNamespace"] = json!(true),
                3 => bad["automountServiceAccountToken"] = json!(true),
                4 => {
                    bad["containers"][0]["env"] =
                        json!([{"name":"DATABASE_URL","value":"owned-dummy"}])
                }
                5 => {
                    bad["containers"][0]["securityContext"]["capabilities"]["add"] =
                        json!(["SYS_ADMIN"])
                }
                6 => bad["volumes"][0] = json!({"name":"bootstrap","hostPath":{"path":"/"}}),
                7 => bad["containers"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"name":"injected","image":"unselected"})),
                _ => {
                    bad["containers"][0]["securityContext"]["readOnlyRootFilesystem"] = json!(false)
                }
            }
            let actual = serde_json::from_value(bad).unwrap();
            assert!(
                verify_pod_policy(&actual, &expected).is_err(),
                "mutation{mutation}"
            );
        }
        let mut defaulted = expected.clone();
        defaulted.host_pid = None;
        defaulted.host_ipc = None;
        defaulted.host_network = None;
        defaulted.share_process_namespace = None;
        verify_pod_policy(&defaulted, &expected).unwrap();
    }
    #[test]
    fn stage_and_candidate_mounts_are_attempt_private_and_inputs_remain_selected() {
        let (policy, mut spec) = fixture();
        spec.purpose = LaunchPurpose::Setup;
        spec.stage = Some(PrivateStage {
            cache_key: "opaque".into(),
            quota_bytes: 4096,
            quota_files: 8,
        });
        let pod = policy.pod(&spec).unwrap();
        let body = pod.spec.unwrap();
        let runtime = body.containers[0]
            .volume_mounts
            .as_ref()
            .unwrap()
            .iter()
            .find(|m| m.mount_path == RUNTIME_ROOT)
            .unwrap();
        assert_eq!(runtime.read_only, None);
        assert!(body.volumes.unwrap().iter().all(|v| v.empty_dir.is_some()));
        spec.inputs.push(PinnedLaunchInput {
            pin_id: 1,
            kind: LaunchInputKind::Runtime,
            object_id: 1,
            object_version: "v".into(),
            size_bytes: 1,
            sha256: Sha256Digest::of_bytes(b"x"),
            max_expanded_bytes: 2,
            max_files: 1,
        });
        assert!(policy.pod(&spec).is_err());
        spec.stage = None;
        spec.inputs.clear();
        spec.pack_access = PackAccess::CandidateCopy;
        assert!(policy.pod(&spec).is_err());
    }
}
