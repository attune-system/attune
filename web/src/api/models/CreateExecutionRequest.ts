/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { RetentionPolicyType } from './RetentionPolicyType';
/**
 * Request DTO for creating a manual execution
 */
export type CreateExecutionRequest = {
    /**
     * Action reference to execute
     */
    action_ref: string;
    /**
     * Retention limit override for non-log artifacts created by this execution.
     * Omit to inherit the action default.
     */
    artifact_retention_limit?: number | null;
    artifact_retention_policy?: (null | RetentionPolicyType);
    /**
     * Environment variables for this execution
     */
    env_vars: Record<string, any>;
    /**
     * Execution parameters/configuration
     */
    parameters: Record<string, any>;
    /**
     * Permission set refs to apply to this execution's API token. Omit to use
     * the action default. Provide an empty array to force no API token.
     */
    permission_set_refs?: any[] | null;
    /**
     * Execution timeout override in seconds. Omit to inherit the action default
     * (or the app-level `default_execution_timeout_seconds` when the action has
     * no default). Must be a positive integer.
     */
    timeout_seconds?: number | null;
    /**
     * Worker affinity override. Omit to inherit the action default; provide
     * `{}` to explicitly clear affinity requirements/preferences.
     */
    worker_affinity?: any | null;
    /**
     * Worker label selector override. Omit to inherit the action default;
     * provide `{}` to explicitly clear selector requirements.
     */
    worker_selector?: any | null;
    /**
     * Worker taint tolerations override. Omit to inherit the action default;
     * provide `[]` to explicitly clear tolerations.
     */
    worker_tolerations?: any[] | null;
};

