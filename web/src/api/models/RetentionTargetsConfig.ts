/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { RetentionTargetConfig } from './RetentionTargetConfig';
/**
 * Per-table runtime retention targets.
 */
export type RetentionTargetsConfig = {
    audit_events?: RetentionTargetConfig;
    continuous_aggregates?: RetentionTargetConfig;
    enforcements?: RetentionTargetConfig;
    events?: RetentionTargetConfig;
    execution_admission?: RetentionTargetConfig;
    execution_history?: RetentionTargetConfig;
    executions?: RetentionTargetConfig;
    inquiries?: RetentionTargetConfig;
    notifications?: RetentionTargetConfig;
    pack_test_executions?: RetentionTargetConfig;
    sensor_process_history?: RetentionTargetConfig;
    sensor_processes?: RetentionTargetConfig;
    webhook_event_logs?: RetentionTargetConfig;
    work_queue_dispatches?: RetentionTargetConfig;
    work_queue_items?: RetentionTargetConfig;
    worker_history?: RetentionTargetConfig;
    workers?: RetentionTargetConfig;
};

