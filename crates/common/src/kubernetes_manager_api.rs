//! In-cluster Kubernetes access for scoped manager credential reconciliation.
use std::{net::IpAddr, time::Duration};

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::OnceCell;
use uuid::Uuid;

use crate::{Error, Result};

const SERVICE_ACCOUNT: &str = "/var/run/secrets/kubernetes.io/serviceaccount";

pub struct KubernetesManagerApi {
    base: String,
    http: reqwest::Client,
}

impl KubernetesManagerApi {
    pub async fn in_cluster() -> Result<&'static Self> {
        static CLIENT: OnceCell<KubernetesManagerApi> = OnceCell::const_new();
        CLIENT
            .get_or_try_init(|| async {
                let ip = std::env::var("KUBERNETES_SERVICE_HOST")
                    .ok()
                    .and_then(|host| host.parse::<IpAddr>().ok())
                    .ok_or_else(unavailable)?;
                let authority = match ip {
                    IpAddr::V4(ip) => ip.to_string(),
                    IpAddr::V6(ip) => format!("[{ip}]"),
                };
                let port = std::env::var("KUBERNETES_SERVICE_PORT_HTTPS")
                    .ok()
                    .and_then(|port| port.parse::<u16>().ok())
                    .unwrap_or(443);
                let pem = tokio::fs::read(format!("{SERVICE_ACCOUNT}/ca.crt"))
                    .await
                    .map_err(|_| unavailable())?;
                let ca = reqwest::Certificate::from_pem(&pem).map_err(|_| unavailable())?;
                let http = reqwest::Client::builder()
                    .no_proxy()
                    .redirect(reqwest::redirect::Policy::none())
                    .timeout(Duration::from_secs(3))
                    .tls_certs_only([ca])
                    .build()
                    .map_err(|_| unavailable())?;
                Ok(Self {
                    base: format!("https://{authority}:{port}"),
                    http,
                })
            })
            .await
    }

    pub async fn access_token() -> Result<String> {
        tokio::fs::read_to_string(format!("{SERVICE_ACCOUNT}/token"))
            .await
            .map_err(|_| unavailable())
    }

    pub async fn token_review(&self, caller: &str, audience: &str, token: &str) -> Result<Value> {
        self.http
            .post(format!(
                "{}/apis/authentication.k8s.io/v1/tokenreviews",
                self.base
            ))
            .bearer_auth(caller)
            .json(
                &json!({"apiVersion":"authentication.k8s.io/v1", "kind":"TokenReview",
                "spec":{"token":token,"audiences":[audience]}}),
            )
            .send()
            .await
            .map_err(|_| unavailable())?
            .error_for_status()
            .map_err(|_| unavailable())?
            .json()
            .await
            .map_err(|_| unavailable())
    }

    /// A failed Kubernetes request is distinct from a confirmed missing Pod.
    pub async fn live_pod(
        &self,
        caller: &str,
        namespace: &str,
        name: &str,
        expected_uid: Uuid,
    ) -> Result<Option<String>> {
        if !crate::manager_control::valid_manager_name(namespace)
            || !crate::manager_control::valid_manager_name(name)
        {
            return Err(unavailable());
        }
        let response = self
            .http
            .get(format!(
                "{}/api/v1/namespaces/{namespace}/pods/{name}",
                self.base
            ))
            .bearer_auth(caller)
            .send()
            .await
            .map_err(|_| unavailable())?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let pod: Pod = response
            .error_for_status()
            .map_err(|_| unavailable())?
            .json()
            .await
            .map_err(|_| unavailable())?;
        let account = pod.spec.service_account_name;
        let managed = pod.metadata.uid == expected_uid
            && pod.metadata.deletion_timestamp.is_none()
            && name
                .strip_prefix(&account)
                .and_then(|suffix| suffix.strip_prefix('-'))
                .is_some_and(|ordinal| {
                    !ordinal.is_empty() && ordinal.bytes().all(|c| c.is_ascii_digit())
                })
            && pod.metadata.owner_references.iter().any(|owner| {
                owner.kind == "StatefulSet"
                    && owner.name == account
                    && owner.controller == Some(true)
            });
        Ok(managed.then_some(account))
    }
}

#[derive(Deserialize)]
struct Pod {
    metadata: PodMetadata,
    spec: PodSpec,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PodMetadata {
    uid: Uuid,
    deletion_timestamp: Option<String>,
    #[serde(default)]
    owner_references: Vec<PodOwner>,
}
#[derive(Deserialize)]
struct PodOwner {
    kind: String,
    name: String,
    controller: Option<bool>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PodSpec {
    service_account_name: String,
}

fn unavailable() -> Error {
    Error::external_service("Kubernetes manager identity verification is unavailable")
}
