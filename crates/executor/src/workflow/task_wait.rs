use attune_common::models::{
    enums::{ExecutionStatus, WorkQueueItemStatus},
    Execution, WorkQueueItem,
};
use serde_json::{json, Value as JsonValue};

#[derive(Debug, Clone, PartialEq)]
pub enum TargetResolution {
    Waiting,
    Released(JsonValue),
    Failed(JsonValue),
    TimedOut(JsonValue),
}

pub fn execution_resolution(execution: &Execution) -> TargetResolution {
    let snapshot = json!({
        "id": execution.id,
        "status": execution.status,
        "result": execution.result,
    });
    classify_execution(execution.status, snapshot)
}

fn classify_execution(status: ExecutionStatus, snapshot: JsonValue) -> TargetResolution {
    match status {
        ExecutionStatus::Requested
        | ExecutionStatus::Scheduling
        | ExecutionStatus::Scheduled
        | ExecutionStatus::Running
        | ExecutionStatus::Canceling => TargetResolution::Waiting,
        ExecutionStatus::Completed => TargetResolution::Released(snapshot),
        ExecutionStatus::Timeout => TargetResolution::TimedOut(snapshot),
        ExecutionStatus::Failed | ExecutionStatus::Abandoned | ExecutionStatus::Cancelled => {
            TargetResolution::Failed(snapshot)
        }
    }
}

pub fn work_queue_item_resolution(item: &WorkQueueItem) -> TargetResolution {
    let result = match item.status {
        WorkQueueItemStatus::Completed => item.ack_summary.clone(),
        WorkQueueItemStatus::Failed | WorkQueueItemStatus::Cancelled => item.last_error.clone(),
        WorkQueueItemStatus::Skipped => item.ack_summary.clone(),
        WorkQueueItemStatus::Queued | WorkQueueItemStatus::Leased | WorkQueueItemStatus::Retry => {
            None
        }
    };
    let snapshot = json!({
        "id": item.id,
        "queue_ref": item.queue_ref,
        "status": item.status,
        "result": result,
    });
    classify_work_queue_item(item.status, snapshot)
}

fn classify_work_queue_item(status: WorkQueueItemStatus, snapshot: JsonValue) -> TargetResolution {
    match status {
        WorkQueueItemStatus::Queued | WorkQueueItemStatus::Leased | WorkQueueItemStatus::Retry => {
            TargetResolution::Waiting
        }
        WorkQueueItemStatus::Completed => TargetResolution::Released(snapshot),
        WorkQueueItemStatus::Failed
        | WorkQueueItemStatus::Skipped
        | WorkQueueItemStatus::Cancelled => TargetResolution::Failed(snapshot),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_statuses_have_expected_resolution() {
        for status in [
            ExecutionStatus::Requested,
            ExecutionStatus::Scheduling,
            ExecutionStatus::Scheduled,
            ExecutionStatus::Running,
            ExecutionStatus::Canceling,
        ] {
            assert!(matches!(
                execution_resolution(&execution(status)),
                TargetResolution::Waiting
            ));
        }
        assert!(matches!(
            execution_resolution(&execution(ExecutionStatus::Completed)),
            TargetResolution::Released(_)
        ));
        assert!(matches!(
            execution_resolution(&execution(ExecutionStatus::Timeout)),
            TargetResolution::TimedOut(_)
        ));
        for status in [ExecutionStatus::Failed, ExecutionStatus::Abandoned] {
            assert!(matches!(
                execution_resolution(&execution(status)),
                TargetResolution::Failed(_)
            ));
        }
        let TargetResolution::Failed(snapshot) =
            execution_resolution(&execution(ExecutionStatus::Cancelled))
        else {
            panic!("cancelled execution should terminate its wait");
        };
        assert_eq!(
            snapshot,
            json!({"id": 42, "status": "cancelled", "result": null})
        );
    }

    #[test]
    fn queue_statuses_have_expected_resolution() {
        let snapshot = json!({"id": 7});
        for status in [
            WorkQueueItemStatus::Queued,
            WorkQueueItemStatus::Leased,
            WorkQueueItemStatus::Retry,
        ] {
            assert_eq!(
                classify_work_queue_item(status, snapshot.clone()),
                TargetResolution::Waiting
            );
        }
        assert!(matches!(
            classify_work_queue_item(WorkQueueItemStatus::Completed, snapshot.clone()),
            TargetResolution::Released(_)
        ));
        for status in [
            WorkQueueItemStatus::Failed,
            WorkQueueItemStatus::Skipped,
            WorkQueueItemStatus::Cancelled,
        ] {
            assert!(matches!(
                classify_work_queue_item(status, snapshot.clone()),
                TargetResolution::Failed(_)
            ));
        }
    }

    #[test]
    fn queue_snapshot_excludes_payload_and_metadata() {
        let TargetResolution::Released(snapshot) =
            work_queue_item_resolution(&work_queue_item(WorkQueueItemStatus::Completed))
        else {
            panic!("completed queue item should release its wait");
        };
        assert_eq!(
            snapshot,
            json!({
                "id": 7,
                "queue_ref": "test.queue",
                "status": "completed",
                "result": {"processed": true}
            })
        );
    }

    fn work_queue_item(status: WorkQueueItemStatus) -> WorkQueueItem {
        WorkQueueItem {
            id: 7,
            queue: 2,
            queue_ref: "test.queue".to_string(),
            pack_release: None,
            pack_release_digest: None,
            executable_snapshot: None,
            item_key: None,
            priority: 0,
            status,
            payload: json!({"secret": "payload"}),
            metadata: json!({"secret": "metadata"}),
            trace_tag: None,
            enqueue_source: "test".to_string(),
            requested_by_identity: None,
            requested_by_execution: Some(1),
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 1,
            last_error: Some(json!({"message": "failed"})),
            ack_summary: Some(json!({"processed": true})),
            created: chrono::Utc::now(),
            updated: chrono::Utc::now(),
        }
    }

    fn execution(status: ExecutionStatus) -> Execution {
        let mut execution: Execution = serde_json::from_value(json!({
            "id": 42,
            "action": null,
            "action_ref": "test.action",
            "pack_release": null,
            "pack_release_digest": null,
            "executable_snapshot": null,
            "parent": 1,
            "enforcement": null,
            "config": null,
            "env_vars": null,
            "executor": null,
            "permission_set_refs": [],
            "artifact_retention_policy": null,
            "artifact_retention_limit": null,
            "worker_selector": null,
            "worker_tolerations": null,
            "worker_affinity": null,
            "worker": null,
            "status": "requested",
            "trace_tag": null,
            "timeout_seconds": null,
            "result": null,
            "retry_count": 0,
            "max_retries": null,
            "retry_reason": null,
            "original_execution": null,
            "workflow_task": null,
            "created": "2026-01-01T00:00:00Z",
            "updated": "2026-01-01T00:00:00Z",
            "started_at": null
        }))
        .expect("execution fixture");
        execution.status = status;
        execution
    }
}
