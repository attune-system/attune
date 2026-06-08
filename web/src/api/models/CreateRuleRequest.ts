/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Request DTO for creating a new rule
 */
export type CreateRuleRequest = {
    /**
     * Parameters to pass to the action when rule is triggered
     */
    action_params?: Record<string, any>;
    /**
     * Action reference to execute when rule matches
     */
    action_ref: string;
    /**
     * Conditions for rule evaluation (JSON Logic or custom format)
     */
    conditions?: Record<string, any>;
    /**
     * Rule description
     */
    description?: string | null;
    /**
     * Whether the rule is enabled
     */
    enabled?: boolean;
    /**
     * Human-readable label
     */
    label: string;
    /**
     * Pack reference this rule belongs to
     */
    pack_ref: string;
    /**
     * Permission set refs to apply to executions created by this rule. Omit to
     * inherit the action default. Provide an empty array to force no API token.
     */
    permission_set_refs?: any[] | null;
    /**
     * Unique reference identifier (e.g., "mypack.notify_on_error")
     */
    ref: string;
    /**
     * Parameters for trigger configuration and event filtering
     */
    trigger_params?: Record<string, any>;
    /**
     * Trigger reference that activates this rule
     */
    trigger_ref: string;
};

