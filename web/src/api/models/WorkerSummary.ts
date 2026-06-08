/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { WorkerHealthState } from './WorkerHealthState';
import type { WorkerLoadSnapshot } from './WorkerLoadSnapshot';
import type { WorkerRole } from './WorkerRole';
import type { WorkerRuntimeSupport } from './WorkerRuntimeSupport';
import type { WorkerStatus } from './WorkerStatus';
import type { WorkerType } from './WorkerType';
export type WorkerSummary = {
    cordon_reason?: string | null;
    cordoned: boolean;
    cordoned_at?: string | null;
    cordoned_by?: number | null;
    created: string;
    health_state: WorkerHealthState;
    heartbeat_age_seconds?: number | null;
    heartbeat_stale: boolean;
    host?: string | null;
    id: number;
    last_heartbeat?: string | null;
    load: WorkerLoadSnapshot;
    name: string;
    port?: number | null;
    status?: (null | WorkerStatus);
    supported_runtimes: Array<WorkerRuntimeSupport>;
    updated: string;
    worker_role: WorkerRole;
    worker_type: WorkerType;
};

