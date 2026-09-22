use std::sync::Arc;

use attune_common::{
    audit::{event_type, AuditCategory, AuditEventBuilder, AuditOutcome, PendingAuditEvent},
    crypto::decrypt_json,
    inquiry_callback_adapter::{
        sensor_inquiry_callback_adapters, NormalizedInquiryCallbackSelection,
    },
    inquiry_response_handle::resolve_inquiry_response_handle,
    models::{
        enums::{ExecutionStatus, InquiryStatus},
        inquiry::Inquiry,
        Id, JsonDict,
    },
    mq::{InquiryRespondedPayload, MessageEnvelope, MessageType},
    rbac::{Action as RbacAction, AuthorizationContext, Resource},
    repositories::{
        execution::ExecutionRepository,
        external_identity_mapping::{ExternalIdentityMappingRepository, ResolvedExternalIdentity},
        inquiry::InquiryRepository,
        inquiry_callback_delivery::InquiryCallbackDeliveryRepository,
        sensor_admission::SensorAdmissionRepository,
        trigger::SensorRepository,
        workflow::WorkflowExecutionRepository,
        FindById,
    },
};
use tokio::time::{Duration, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use crate::{
    authz::AuthorizationCheck,
    middleware::{ApiError, ApiResult},
    state::AppState,
    validation::validate_inquiry_response,
};

pub enum InquiryResponseSubmission {
    Human {
        inquiry_id: Id,
        response: JsonDict,
        identity_id: Id,
        execution_id: Option<Id>,
    },
    CallbackAdapter {
        delivery_id: Id,
    },
}

struct ResolvedResponseActor {
    identity_id: Id,
    execution_id: Option<Id>,
    external_actor: Option<serde_json::Value>,
    audit_event: PendingAuditEvent,
}

pub async fn submit_inquiry_response(
    state: &Arc<AppState>,
    submission: InquiryResponseSubmission,
) -> ApiResult<Inquiry> {
    let encryption_key = state.config.security.encryption_key.as_deref();
    let mut transaction = state.db.begin().await?;
    let mut callback_delivery_id = None;
    let mut callback_selection = None;
    let (inquiry_id, option_index) = match &submission {
        InquiryResponseSubmission::Human { inquiry_id, .. } => (*inquiry_id, None),
        InquiryResponseSubmission::CallbackAdapter { delivery_id } => {
            let encryption_key = encryption_key.ok_or_else(|| {
                ApiError::InternalServerError(
                    "Cannot resolve inquiry response handles without security.encryption_key"
                        .to_string(),
                )
            })?;
            let delivery =
                InquiryCallbackDeliveryRepository::find_by_id(&mut *transaction, *delivery_id)
                    .await?
                    .ok_or_else(|| {
                        ApiError::NotFound("Callback delivery was not found".to_string())
                    })?;
            if delivery.state != "pending" {
                return Err(ApiError::Conflict(
                    "Callback delivery is no longer pending".to_string(),
                ));
            }

            SensorAdmissionRepository::lock_workload_checks(&mut transaction).await?;
            let sensor = SensorRepository::find_by_id_for_update(&mut transaction, delivery.sensor)
                .await?
                .ok_or_else(|| {
                    ApiError::Forbidden("Sensor transport is unavailable".to_string())
                })?;
            if !sensor.enabled || sensor.retired_at.is_some() {
                return Err(ApiError::Forbidden(
                    "Sensor callback adapter is unavailable".to_string(),
                ));
            }
            let delivery = InquiryCallbackDeliveryRepository::find_by_id_for_update(
                &mut transaction,
                *delivery_id,
            )
            .await?
            .ok_or_else(|| ApiError::NotFound("Callback delivery was not found".to_string()))?;
            if delivery.state != "pending" || delivery.sensor != sensor.id {
                return Err(ApiError::Conflict(
                    "Callback delivery is no longer pending for this sensor".to_string(),
                ));
            }
            let adapters = sensor_inquiry_callback_adapters(sensor.config.as_ref())
                .map_err(|error| ApiError::InternalServerError(error.to_string()))?;
            if !adapters
                .get(&delivery.adapter_ref)
                .is_some_and(|adapter| adapter.enabled)
            {
                return Err(ApiError::Forbidden(
                    "Sensor callback adapter is unavailable".to_string(),
                ));
            }
            let selection: NormalizedInquiryCallbackSelection =
                serde_json::from_value(decrypt_json(&delivery.encrypted_payload, encryption_key)?)
                    .map_err(|_| {
                        ApiError::InternalServerError(
                            "Stored callback delivery payload is invalid".to_string(),
                        )
                    })?;
            let handle_selection =
                resolve_inquiry_response_handle(&selection.response_handle, encryption_key)
                    .map_err(|_| {
                        ApiError::NotFound("Inquiry response handle was not found".to_string())
                    })?;
            callback_delivery_id = Some(*delivery_id);
            callback_selection = Some((delivery.integration_identity, selection));
            (
                handle_selection.inquiry_id,
                Some(handle_selection.option_index),
            )
        }
    };

    let actor = match &submission {
        InquiryResponseSubmission::Human {
            identity_id,
            execution_id,
            ..
        } => ResolvedResponseActor {
            identity_id: *identity_id,
            execution_id: *execution_id,
            external_actor: None,
            audit_event: build_human_response_audit(
                inquiry_id,
                *identity_id,
                execution_id.is_some(),
            ),
        },
        InquiryResponseSubmission::CallbackAdapter { delivery_id, .. } => {
            let (integration_identity_id, selection) = callback_selection
                .as_ref()
                .expect("callback delivery was resolved");
            state
                .authorization_service()
                .authorize_identity_fresh(
                    &mut transaction,
                    *integration_identity_id,
                    AuthorizationCheck {
                        resource: Resource::Inquiries,
                        action: RbacAction::Respond,
                        context: AuthorizationContext {
                            target_id: Some(inquiry_id),
                            ..AuthorizationContext::new(*integration_identity_id)
                        },
                    },
                )
                .await?;
            let resolved = ExternalIdentityMappingRepository::resolve_exact_with_mapping_for_share(
                &mut transaction,
                *integration_identity_id,
                &selection.provider,
                &selection.tenant,
                &selection.subject_kind,
                &selection.external_subject,
            )
            .await?
            .ok_or_else(|| {
                ApiError::Forbidden(
                    "External identity is not mapped to an active identity".to_string(),
                )
            })?;
            let external_actor = serde_json::json!({
                "provider": resolved.mapping.provider,
                "subject_kind": resolved.mapping.subject_kind,
                "mapping_id": resolved.mapping.id,
                "integration_identity_id": integration_identity_id,
                "delivery_id": delivery_id,
            });
            ResolvedResponseActor {
                identity_id: resolved.identity.id,
                execution_id: None,
                external_actor: Some(external_actor),
                audit_event: build_callback_response_audit(
                    inquiry_id,
                    &resolved,
                    *integration_identity_id,
                    *delivery_id,
                ),
            }
        }
    };

    let initial = InquiryRepository::find_by_id(&mut *transaction, inquiry_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Inquiry with ID {inquiry_id} not found")))?;

    if let Some(execution_id) = actor.execution_id {
        if initial.created_by_execution == execution_id {
            return Err(ApiError::Forbidden(
                "An execution cannot respond to an inquiry it created (privilege loop)".to_string(),
            ));
        }
        if ExecutionRepository::is_in_execution_tree(
            &mut *transaction,
            initial.created_by_execution,
            execution_id,
            false,
        )
        .await?
        {
            return Err(ApiError::Forbidden(
                "A descendant execution cannot respond to an ancestor's inquiry (privilege loop)"
                    .to_string(),
            ));
        }
    }

    if let Some(workflow_execution_id) = initial.workflow_execution {
        WorkflowExecutionRepository::acquire_advisory_lock(&mut transaction, workflow_execution_id)
            .await?;
        let workflow = WorkflowExecutionRepository::find_by_id_for_update(
            &mut *transaction,
            workflow_execution_id,
        )
        .await?
        .ok_or_else(|| ApiError::Conflict("Owning workflow no longer exists".to_string()))?;
        if matches!(
            workflow.status,
            ExecutionStatus::Completed
                | ExecutionStatus::Failed
                | ExecutionStatus::Canceling
                | ExecutionStatus::Cancelled
                | ExecutionStatus::Timeout
                | ExecutionStatus::Abandoned
        ) {
            return Err(ApiError::Conflict(
                "Owning workflow is cancelling or terminal".to_string(),
            ));
        }
    }

    let inquiry = InquiryRepository::find_by_id_for_update(&mut transaction, inquiry_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Inquiry with ID {inquiry_id} not found")))?;
    if inquiry.status != InquiryStatus::Pending {
        return Err(ApiError::Conflict(
            "Inquiry is no longer pending".to_string(),
        ));
    }
    if inquiry
        .timeout_at
        .is_some_and(|timeout_at| timeout_at <= chrono::Utc::now())
    {
        return Err(ApiError::Conflict(
            "Inquiry has timed out and can no longer be responded to".to_string(),
        ));
    }

    match (&submission, inquiry.assigned_to) {
        (_, Some(assigned_to)) if assigned_to != actor.identity_id => {
            return Err(ApiError::Forbidden(format!(
                "Inquiry {inquiry_id} is assigned to identity {assigned_to} and can only be answered by them"
            )));
        }
        (InquiryResponseSubmission::CallbackAdapter { .. }, None) => {
            return Err(ApiError::Forbidden(
                "Callback responses require an assigned inquiry".to_string(),
            ));
        }
        _ => {}
    }

    let response = match submission {
        InquiryResponseSubmission::Human { response, .. } => response,
        InquiryResponseSubmission::CallbackAdapter { .. } => inquiry
            .response_options
            .get(usize::from(
                option_index.expect("callback selection was resolved"),
            ))
            .map(|option| option.response.clone())
            .ok_or_else(|| {
                ApiError::NotFound("Inquiry response handle was not found".to_string())
            })?,
    };
    validate_inquiry_response(inquiry_id, inquiry.response_schema.as_ref(), &response)?;
    let updated = InquiryRepository::respond_pending(
        &mut *transaction,
        inquiry_id,
        response,
        actor.identity_id,
        actor.external_actor,
    )
    .await?
    .ok_or_else(|| {
        ApiError::Conflict("Inquiry is no longer pending or has timed out".to_string())
    })?;
    if let Some(delivery_id) = callback_delivery_id {
        if !InquiryCallbackDeliveryRepository::mark_accepted(
            &mut transaction,
            delivery_id,
            updated.id,
        )
        .await?
        {
            return Err(ApiError::Conflict(
                "Callback delivery is no longer pending".to_string(),
            ));
        }
    }
    transaction.commit().await?;
    state.audit_emitter.emit(actor.audit_event);
    publish_inquiry_responded(state, &updated).await;
    Ok(updated)
}

pub async fn process_inquiry_callback_delivery(
    state: &Arc<AppState>,
    delivery_id: Id,
) -> ApiResult<()> {
    match submit_inquiry_response(
        state,
        InquiryResponseSubmission::CallbackAdapter { delivery_id },
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(error) if error.status_code().is_client_error() => {
            let rejection_code = error.code().to_ascii_lowercase();
            let mut transaction = state.db.begin().await?;
            let delivery = InquiryCallbackDeliveryRepository::find_by_id_for_update(
                &mut transaction,
                delivery_id,
            )
            .await?;
            if delivery.is_some_and(|delivery| delivery.state == "pending") {
                InquiryCallbackDeliveryRepository::mark_rejected(
                    &mut transaction,
                    delivery_id,
                    &rejection_code,
                )
                .await?;
            }
            transaction.commit().await?;
            Ok(())
        }
        Err(error) => Err(error),
    }
}

pub async fn start_inquiry_callback_delivery_monitor(
    state: Arc<AppState>,
    shutdown: CancellationToken,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = interval.tick() => {
                let delivery_ids = match InquiryCallbackDeliveryRepository::claim_pending_ids(
                    &state.db,
                    100,
                    30,
                )
                .await
                {
                    Ok(delivery_ids) => delivery_ids,
                    Err(error) => {
                        tracing::warn!(%error, "Failed to scan pending inquiry callback deliveries");
                        continue;
                    }
                };
                for delivery_id in delivery_ids {
                    if shutdown.is_cancelled() {
                        return;
                    }
                    if let Err(error) = process_inquiry_callback_delivery(&state, delivery_id).await {
                        tracing::warn!(
                            delivery_id,
                            code = error.code(),
                            "Inquiry callback delivery remains pending"
                        );
                    }
                }
            }
        }
    }
}

fn build_human_response_audit(
    inquiry_id: Id,
    identity_id: Id,
    execution_scoped: bool,
) -> PendingAuditEvent {
    AuditEventBuilder::new(
        AuditCategory::Api,
        event_type::inquiry::HUMAN_RESPONSE_ACCEPTED,
        AuditOutcome::Success,
    )
    .actor_identity(identity_id)
    .actor_token_type(if execution_scoped {
        "execution"
    } else {
        "access"
    })
    .resource("inquiry")
    .resource_id(inquiry_id)
    .build()
}

fn build_callback_response_audit(
    inquiry_id: Id,
    resolved: &ResolvedExternalIdentity,
    integration_identity_id: Id,
    delivery_id: Id,
) -> PendingAuditEvent {
    AuditEventBuilder::new(
        AuditCategory::Api,
        event_type::inquiry::CALLBACK_RESPONSE_ACCEPTED,
        AuditOutcome::Success,
    )
    .actor_identity(resolved.identity.id)
    .actor_login(resolved.identity.login.clone())
    .actor_token_type("callback_adapter")
    .resource("inquiry")
    .resource_id(inquiry_id)
    .with_details(serde_json::json!({
        "provider": resolved.mapping.provider,
        "subject_kind": resolved.mapping.subject_kind,
        "mapping_id": resolved.mapping.id,
        "integration_identity_id": integration_identity_id,
        "delivery_id": delivery_id,
    }))
    .build()
}

pub fn resolve_option_response(
    handle: &str,
    encryption_key: &str,
    inquiry: &Inquiry,
) -> ApiResult<JsonDict> {
    let selection = resolve_inquiry_response_handle(handle, encryption_key)
        .map_err(|_| ApiError::NotFound("Inquiry response handle was not found".to_string()))?;
    if selection.inquiry_id != inquiry.id {
        return Err(ApiError::NotFound(
            "Inquiry response handle was not found".to_string(),
        ));
    }
    inquiry
        .response_options
        .get(usize::from(selection.option_index))
        .map(|option| option.response.clone())
        .ok_or_else(|| ApiError::NotFound("Inquiry response handle was not found".to_string()))
}

async fn publish_inquiry_responded(state: &Arc<AppState>, inquiry: &Inquiry) {
    let Some(response) = inquiry.response.clone() else {
        tracing::error!(
            inquiry_id = inquiry.id,
            "Responded inquiry has no response payload"
        );
        return;
    };
    if let Some(publisher) = state.get_publisher().await {
        let payload = InquiryRespondedPayload {
            inquiry_id: inquiry.id,
            created_by_execution_id: inquiry.created_by_execution,
            response,
            responded_by: inquiry.responded_by,
            responded_at: inquiry.responded_at.unwrap_or_else(chrono::Utc::now),
        };
        let envelope =
            MessageEnvelope::new(MessageType::InquiryResponded, payload).with_source("api");
        if let Err(error) = publisher.publish_envelope(&envelope).await {
            tracing::error!(inquiry_id = inquiry.id, %error, "Failed to publish InquiryResponded message");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::{
        inquiry_response_handle::issue_inquiry_response_handle,
        models::inquiry::{InquiryResponseOption, InquiryResponseOptionStyle},
    };
    use chrono::Utc;
    use serde_json::json;

    const KEY: &str = "test-encryption-key-that-is-long-enough";

    fn inquiry() -> Inquiry {
        Inquiry {
            id: 42,
            created_by_execution: 1,
            workflow_execution: None,
            workflow_task_name: None,
            action_attempt_family: None,
            purpose: Some("approval".to_string()),
            prompt: "Approve?".to_string(),
            response_schema: None,
            response_options: vec![
                InquiryResponseOption {
                    r#ref: "approve".to_string(),
                    label: "Approve".to_string(),
                    style: InquiryResponseOptionStyle::Positive,
                    response: json!({"approved": true}),
                },
                InquiryResponseOption {
                    r#ref: "reject".to_string(),
                    label: "Reject".to_string(),
                    style: InquiryResponseOptionStyle::Destructive,
                    response: json!({"approved": false}),
                },
            ],
            assigned_to: Some(2),
            status: InquiryStatus::Pending,
            response: None,
            timeout_at: None,
            timeout_seconds: None,
            responded_by: None,
            external_actor: None,
            responded_at: None,
            created: Utc::now(),
            updated: Utc::now(),
        }
    }

    #[test]
    fn option_handle_selects_only_its_bound_response() {
        let approve = issue_inquiry_response_handle(42, 0, KEY).unwrap();
        let reject = issue_inquiry_response_handle(42, 1, KEY).unwrap();
        assert_eq!(
            resolve_option_response(&approve, KEY, &inquiry()).unwrap(),
            json!({"approved": true})
        );
        assert_eq!(
            resolve_option_response(&reject, KEY, &inquiry()).unwrap(),
            json!({"approved": false})
        );
    }
}
