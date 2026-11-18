//! Real controller transport/effect ordering over owned HTTP and Unix sockets.
//! These fixtures do not prove CP admission/native persistence or OS isolation;
//! those remain the API and actual-container integration lanes.
use crate::{
    actuator::{Actuator, ActuatorError},
    cache::{CacheLimits, CacheStore},
    control_plane::ControlPlane,
    engine::DockerEngine,
};
use attune_common::workload_launch::*;
use axum::{
    body::Bytes,
    extract::{OriginalUri, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
    Json, Router,
};
use std::sync::Arc;
use tokio::{
    sync::{oneshot, Mutex},
    task::JoinHandle,
};

struct Effects {
    manifest: LaunchManifest,
    lease: LaunchLease,
    original: Option<EngineResourceEvidence>,
    create_nonce: uuid::Uuid,
    closed: bool,
    present: bool,
    foreign: bool,
    removed: bool,
    remove_uncertain: bool,
    ack_rejected: bool,
    stop_denied: bool,
    acknowledged: Arc<tokio::sync::Notify>,
    events: Vec<&'static str>,
    receipt_root: std::path::PathBuf,
}

async fn control(
    State(state): State<Arc<Mutex<Effects>>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    assert_eq!(headers["authorization"], "Bearer owned-controller");
    assert!(uri.query().is_none());
    let operation = uri
        .path()
        .strip_prefix("/api/v1/workload-launches/1/")
        .unwrap();
    let mut state = state.lock().await;
    match operation {
        "stop" => {
            assert_eq!(headers[LAUNCH_DISPATCH_HEADER], "owned-dispatch");
            let request: StopLaunchRequest = serde_json::from_slice(&body).unwrap();
            assert_eq!(
                request.request.admitted_digest,
                state.manifest.admitted_digest().unwrap()
            );
            if state.stop_denied {
                state.events.push("cp.stop-denied");
                return StatusCode::FORBIDDEN.into_response();
            }
            state.events.push("cp.stop");
            state.closed = true;
            let phase = if state.removed {
                LaunchState::Stopped
            } else {
                LaunchState::StopRequired
            };
            Json(status(&state, phase, state.original.clone())).into_response()
        }
        "claim" => {
            assert_eq!(headers[LAUNCH_DISPATCH_HEADER], "owned-dispatch");
            state.events.push("cp.closed-claim");
            state.closed = true;
            let status = status(&state, LaunchState::StopRequired, state.original.clone());
            Json(ClaimedLaunch::Cleanup {
                manifest: Box::new(state.manifest.clone()),
                status,
                lease: state.lease.clone(),
            })
            .into_response()
        }
        "create-intent" => {
            assert_eq!(headers[LAUNCH_LEASE_HEADER], "owned-cleanup");
            let request: LaunchLeaseRequest = serde_json::from_slice(&body).unwrap();
            assert_eq!(request.handle, state.lease.handle);
            assert!(state.closed);
            state.events.push("cp.original-create-intent");
            Json(LaunchCreateIntent {
                handle: request.handle,
                create_nonce: state.create_nonce,
                disposition: CreateDisposition::StopOnly,
            })
            .into_response()
        }
        "stop-ack" => {
            assert_eq!(headers[LAUNCH_LEASE_HEADER], "owned-cleanup");
            let acknowledgement: StopLaunchAcknowledgement = serde_json::from_slice(&body).unwrap();
            assert_eq!(acknowledgement.handle, state.lease.handle);
            assert!(state.closed);
            if state.create_nonce.is_nil() {
                assert!(matches!(
                    acknowledgement.create,
                    ProviderCreateSettlement::NeverSubmitted
                ));
                assert!(acknowledgement.removed_resource.is_none());
            } else {
                assert!(
                    state.removed,
                    "CP ACK preceded native removal acknowledgement"
                );
                let ProviderCreateSettlement::Created {
                    create_nonce,
                    resource,
                } = &acknowledgement.create
                else {
                    panic!("unknown creation was treated as absent")
                };
                assert_eq!(*create_nonce, state.create_nonce);
                assert_eq!(acknowledgement.removed_resource.as_ref(), Some(resource));
                assert_eq!(resource.resource_id, "b".repeat(64));
                assert_eq!(resource.engine_name_nonce, state.manifest.engine_name_nonce);
                assert_eq!(
                    resource.admitted_digest,
                    state.manifest.admitted_digest().unwrap()
                );
                let path = state
                    .receipt_root
                    .join("removed")
                    .join(state.manifest.engine_name_nonce.simple().to_string());
                let persisted: EngineResourceEvidence =
                    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
                assert_eq!(&persisted, resource, "native receipt must precede CP ACK");
            }
            state.events.push("cp.stop-ack");
            if state.ack_rejected {
                state.ack_rejected = false;
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            state.acknowledged.notify_one();
            Json(status(
                &state,
                LaunchState::Stopped,
                acknowledgement.removed_resource,
            ))
            .into_response()
        }
        _ => panic!("cleanup requested forbidden private context/Run authority"),
    }
}

fn status(
    state: &Effects,
    phase: LaunchState,
    resource: Option<EngineResourceEvidence>,
) -> LaunchStatus {
    LaunchStatus {
        request: LaunchRequest {
            launch_id: state.manifest.launch_id,
            admitted_digest: state.manifest.admitted_digest().unwrap(),
        },
        state: phase,
        runtime_state: LaunchRuntimeState::Unstarted,
        exit_outcome: None,
        publication: LaunchPublicationState::NotRequired,
        capture: LaunchCaptureState::Complete,
        capture_deadline: None,
        resource,
        stop_reason: Some(LaunchStopReason::Cancelled),
    }
}

async fn engine(
    State(state): State<Arc<Mutex<Effects>>>,
    OriginalUri(uri): OriginalUri,
    method: Method,
    headers: HeaderMap,
) -> Response {
    assert!(!headers.contains_key("authorization"));
    let mut state = state.lock().await;
    let path = uri.path();
    if path == "/v1.47/info" {
        assert_eq!(method, Method::GET);
        state.events.push("engine.binding");
        return Json(serde_json::json!({"ID":"owned-daemon","OSType":"linux","Architecture":"amd64","SecurityOptions":["name=seccomp,profile=builtin"]})).into_response();
    }
    assert!(
        state.closed,
        "native cleanup preceded durable CP Run closure"
    );
    let selected = format!("/v1.47/containers/{}/json", state.manifest.engine_name());
    let resource = format!("/v1.47/containers/{}", "b".repeat(64));
    if method == Method::GET && path == selected {
        state.events.push("engine.select-owned-nonce");
        if !state.present {
            return StatusCode::NOT_FOUND.into_response();
        }
        return Json(serde_json::json!({
            "Id":"b".repeat(64), "Name":format!("/{}",state.manifest.engine_name()),
            "Config":{"Image":state.manifest.image.reference,"User":"1000:1000","Tty":false,
                "Labels":{"io.attune.launch.engine":state.manifest.engine_id.to_string(),
                    "io.attune.launch.id":state.manifest.launch_id.to_string(),
                    "io.attune.launch.nonce":if state.foreign { uuid::Uuid::new_v4().to_string() } else {state.manifest.engine_name_nonce.to_string()},
                    "io.attune.launch.manifest":state.manifest.admitted_digest().unwrap()}},
            "HostConfig":{"ReadonlyRootfs":true,"CapDrop":["ALL"],"CapAdd":null,"SecurityOpt":["no-new-privileges"],
                "Privileged":false,"PidMode":"","IpcMode":"private","CgroupnsMode":"private","NetworkMode":"owned-network",
                "Mounts":[],"Tmpfs":{},"Memory":268435456,"PidsLimit":64,"NanoCpus":1000000000,"AutoRemove":false,"Init":true}
        })).into_response();
    }
    if method == Method::POST && path == resource.clone() + "/stop" {
        state.events.push("engine.stop");
        return StatusCode::NO_CONTENT.into_response();
    }
    if method == Method::POST && path == resource.clone() + "/wait" {
        assert_eq!(state.events.last(), Some(&"engine.stop"));
        state.events.push("engine.wait");
        return Json(serde_json::json!({"StatusCode":137,"Error":null})).into_response();
    }
    if method == Method::GET && path == resource.clone() + "/json" {
        assert_eq!(state.events.last(), Some(&"engine.wait"));
        state.events.push("engine.confirm-not-running");
        return Json(serde_json::json!({"State":{"Running":false,"OOMKilled":false}}))
            .into_response();
    }
    if method == Method::DELETE && path == resource {
        assert_eq!(uri.query(), Some("force=false&v=false"));
        assert_eq!(state.events.last(), Some(&"engine.confirm-not-running"));
        state.present = false;
        state.events.push("engine.remove");
        if state.remove_uncertain {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        state.removed = true;
        return StatusCode::NO_CONTENT.into_response();
    }
    panic!("cleanup attempted create/start/attach, foreign selector or image pull");
}

struct Harness {
    root: tempfile::TempDir,
    actuator: Actuator,
    reserved: ReservedLaunch,
    state: Arc<Mutex<Effects>>,
    stops: Vec<oneshot::Sender<()>>,
    servers: Vec<JoinHandle<()>>,
}
impl Harness {
    async fn new(recorded: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let image = format!("sha256:{}", "a".repeat(64));
        let spec = crate::test_support::spec(image.clone());
        let request = LaunchRequest {
            launch_id: 1,
            admitted_digest: spec.manifest.admitted_digest().unwrap(),
        };
        let lease = LaunchLease {
            purpose: LaunchLeasePurpose::Cleanup,
            handle: LaunchHandle {
                launch_id: 1,
                admitted_digest: request.admitted_digest.clone(),
                lease_id: uuid::Uuid::new_v4(),
                lease_generation: 2,
                lease_deadline: chrono::Utc::now() + chrono::Duration::seconds(60),
                engine_id: 5,
                engine_name_nonce: spec.manifest.engine_name_nonce,
            },
            token: LaunchToken::new("owned-cleanup".into()).unwrap(),
        };
        let original = recorded.then(|| EngineResourceEvidence {
            engine_id: 5,
            resource_id: "b".repeat(64),
            engine_name_nonce: spec.manifest.engine_name_nonce,
            admitted_digest: request.admitted_digest.clone(),
            public_manifest_digest: Sha256Digest::of_bytes(b"original-owned-policy"),
        });
        let cache = Arc::new(
            CacheStore::open(
                root.path().join("cache"),
                CacheLimits {
                    compressed_bytes: 4096,
                    expanded_bytes: 4096,
                    files: 32,
                },
            )
            .unwrap()
            .for_current_test_user(),
        );
        cache.prepare_private(&spec.manifest).unwrap();
        let state = Arc::new(Mutex::new(Effects {
            manifest: spec.manifest,
            lease,
            original,
            create_nonce: uuid::Uuid::new_v4(),
            closed: false,
            present: true,
            foreign: false,
            removed: false,
            remove_uncertain: false,
            ack_rejected: false,
            stop_denied: false,
            acknowledged: Arc::new(tokio::sync::Notify::new()),
            events: Vec::new(),
            receipt_root: root.path().join("cache"),
        }));
        let cp_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}/", cp_listener.local_addr().unwrap());
        let cp = Router::new()
            .fallback(any(control))
            .with_state(state.clone());
        let (cp_stop, cp_stopped) = oneshot::channel();
        let cp_server = tokio::spawn(async move {
            axum::serve(cp_listener, cp)
                .with_graceful_shutdown(async {
                    let _ = cp_stopped.await;
                })
                .await
                .unwrap();
        });
        let socket = root.path().join("owned-engine.sock");
        let engine_listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let api = Router::new()
            .fallback(any(engine))
            .with_state(state.clone());
        let (engine_stop, engine_stopped) = oneshot::channel();
        let engine_server = tokio::spawn(async move {
            axum::serve(engine_listener, api)
                .with_graceful_shutdown(async {
                    let _ = engine_stopped.await;
                })
                .await
                .unwrap();
        });
        // Revocation of new image admission must not require decrypting an old
        // invocation or block removal of this exact originally owned resource.
        let policy = crate::test_support::policy(
            format!("sha256:{}", "c".repeat(64)),
            "owned-network".into(),
        );
        let actuator = Actuator {
            cp: ControlPlane::new(
                origin.parse().unwrap(),
                LaunchToken::new("owned-controller".into()).unwrap(),
            )
            .unwrap(),
            engine: DockerEngine::connect(&socket).unwrap(),
            policy,
            cache,
            binding: LaunchEngineBinding {
                engine_id: 5,
                daemon_id: "owned-daemon".into(),
                platform: LaunchPlatform::LinuxAmd64,
            },
            compressed_ceiling: 4096,
        };
        Self {
            root,
            actuator,
            reserved: ReservedLaunch {
                request,
                dispatch_token: LaunchToken::new("owned-dispatch".into()).unwrap(),
            },
            state,
            stops: vec![cp_stop, engine_stop],
            servers: vec![cp_server, engine_server],
        }
    }
    async fn retained(&self) -> bool {
        self.root
            .path()
            .join("cache/private")
            .join(
                self.state
                    .lock()
                    .await
                    .manifest
                    .engine_name_nonce
                    .simple()
                    .to_string(),
            )
            .exists()
    }
    async fn finish(self) {
        for stop in self.stops {
            stop.send(()).unwrap();
        }
        for server in self.servers {
            server.await.unwrap();
        }
    }
}

#[tokio::test]
async fn cleanup_orders_closed_authority_stop_wait_remove_and_persisted_receipt_before_ack() {
    for recorded in [false, true] {
        let harness = Harness::new(recorded).await;
        let result = harness
            .actuator
            .reconcile_retained(&harness.reserved)
            .await
            .unwrap();
        assert_eq!(result.state, LaunchState::Stopped);
        assert!(!harness.retained().await);
        assert_eq!(
            harness.state.lock().await.events,
            vec![
                "cp.closed-claim",
                "engine.binding",
                "cp.original-create-intent",
                "engine.select-owned-nonce",
                "engine.stop",
                "engine.wait",
                "engine.confirm-not-running",
                "engine.remove",
                "cp.stop-ack"
            ]
        );
        harness.finish().await;
    }
}

#[tokio::test]
async fn unknown_create_404_retains_stage_until_exact_late_container_is_removed() {
    let harness = Harness::new(false).await;
    harness.state.lock().await.present = false;
    assert!(matches!(
        harness.actuator.reconcile_retained(&harness.reserved).await,
        Err(ActuatorError::Unsettled)
    ));
    assert!(harness.retained().await);
    assert!(!harness.state.lock().await.events.contains(&"cp.stop-ack"));
    harness.state.lock().await.present = true;
    harness
        .actuator
        .reconcile_retained(&harness.reserved)
        .await
        .unwrap();
    assert!(!harness.retained().await);
    harness.finish().await;
}

#[tokio::test]
async fn foreign_nonce_or_uncertain_remove_cannot_ack_absence_or_delete_retained_stage() {
    for foreign in [true, false] {
        let harness = Harness::new(false).await;
        harness.state.lock().await.foreign = foreign;
        harness.state.lock().await.remove_uncertain = !foreign;
        assert!(harness
            .actuator
            .reconcile_retained(&harness.reserved)
            .await
            .is_err());
        assert!(harness.retained().await);
        assert!(!harness.state.lock().await.events.contains(&"cp.stop-ack"));
        if !foreign {
            assert!(matches!(
                harness.actuator.reconcile_retained(&harness.reserved).await,
                Err(ActuatorError::Unsettled)
            ));
            assert!(harness.retained().await);
            assert!(!harness.state.lock().await.events.contains(&"cp.stop-ack"));
        }
        harness.finish().await;
    }
}

#[tokio::test]
async fn native_removal_receipt_survives_rejected_cp_ack_without_repeating_engine_effects() {
    let mut harness = Harness::new(true).await;
    harness.state.lock().await.ack_rejected = true;
    assert!(harness
        .actuator
        .reconcile_retained(&harness.reserved)
        .await
        .is_err());
    assert!(harness.retained().await);
    let engine_effects = harness
        .state
        .lock()
        .await
        .events
        .iter()
        .filter(|event| event.starts_with("engine.") && **event != "engine.binding")
        .count();
    harness.actuator.cache = Arc::new(
        CacheStore::open(
            harness.root.path().join("cache"),
            CacheLimits {
                compressed_bytes: 4096,
                expanded_bytes: 4096,
                files: 32,
            },
        )
        .unwrap()
        .for_current_test_user(),
    );
    harness
        .actuator
        .reconcile_retained(&harness.reserved)
        .await
        .unwrap();
    assert!(!harness.retained().await);
    assert_eq!(
        harness
            .state
            .lock()
            .await
            .events
            .iter()
            .filter(|event| event.starts_with("engine.") && **event != "engine.binding")
            .count(),
        engine_effects
    );
    harness.finish().await;
}

#[tokio::test]
async fn never_submitted_create_uses_typed_settlement_without_engine_container_selection() {
    let harness = Harness::new(false).await;
    harness.state.lock().await.create_nonce = uuid::Uuid::nil();
    harness
        .actuator
        .reconcile_retained(&harness.reserved)
        .await
        .unwrap();
    assert_eq!(
        harness.state.lock().await.events,
        vec![
            "cp.closed-claim",
            "engine.binding",
            "cp.original-create-intent",
            "cp.stop-ack"
        ]
    );
    assert!(!harness.retained().await);
    harness.finish().await;
}

#[tokio::test]
async fn actual_frontend_cancel_requires_grant_and_positive_cp_close_before_engine_cleanup() {
    let Harness {
        root,
        actuator,
        reserved,
        state,
        mut stops,
        mut servers,
    } = Harness::new(true).await;
    let service = crate::service::LaunchService::new(
        Arc::new(actuator),
        crate::service::ServiceLimits {
            max_active: 1,
            max_retained: 2,
            capture_bytes_per_stream: 4096,
            minimum_free_bytes: 0,
        },
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/v1/launches/1/cancel",
        listener.local_addr().unwrap()
    );
    let router = service.clone().router();
    let (stop, stopped) = oneshot::channel();
    stops.push(stop);
    servers.push(tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    }));
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let body = StopLaunchRequest {
        request: reserved.request,
        reason: LaunchStopReason::Cancelled,
    };
    let missing = client.post(&url).json(&body).send().await.unwrap().status();
    let no_missing_effects = state.lock().await.events.is_empty();
    state.lock().await.stop_denied = true;
    let denied = client
        .post(&url)
        .header(LAUNCH_DISPATCH_HEADER, "owned-dispatch")
        .json(&body)
        .send()
        .await
        .unwrap()
        .status();
    let no_denied_engine_effects = !state
        .lock()
        .await
        .events
        .iter()
        .any(|event| event.starts_with("engine."));
    state.lock().await.stop_denied = false;
    let notified = state.lock().await.acknowledged.clone();
    let accepted = client
        .post(&url)
        .header(LAUNCH_DISPATCH_HEADER, "owned-dispatch")
        .json(&body)
        .send()
        .await
        .unwrap()
        .status();
    tokio::time::timeout(std::time::Duration::from_secs(5), notified.notified())
        .await
        .unwrap();
    service.shutdown().await;
    for stop in stops {
        stop.send(()).unwrap();
    }
    for server in servers {
        server.await.unwrap();
    }
    let state = state.lock().await;
    assert_eq!(missing, StatusCode::UNAUTHORIZED);
    assert!(no_missing_effects);
    assert_eq!(denied, StatusCode::FORBIDDEN);
    assert!(no_denied_engine_effects);
    assert_eq!(accepted, StatusCode::OK);
    let close = state
        .events
        .iter()
        .position(|event| *event == "cp.stop")
        .unwrap();
    let stop = state
        .events
        .iter()
        .position(|event| *event == "engine.stop")
        .unwrap();
    let remove = state
        .events
        .iter()
        .position(|event| *event == "engine.remove")
        .unwrap();
    let ack = state
        .events
        .iter()
        .position(|event| *event == "cp.stop-ack")
        .unwrap();
    assert!(close < stop && stop < remove && remove < ack);
    assert!(!root
        .path()
        .join("cache/private")
        .join(state.manifest.engine_name_nonce.simple().to_string())
        .exists());
}
