/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Request DTO for updating a rule
 */
export type UpdateRuleRequest = {
    /**
     * Parameters to pass to the action when rule is triggered
     */
    action_params: any | null;
    /**
     * Action reference to execute when rule matches
     */
    action_ref?: string | null;
    /**
     * Conditions for rule evaluation
     */
    conditions: any | null;
    /**
     * Rule description
     */
    description?: string | null;
    /**
     * Whether the rule is enabled
     */
    enabled?: boolean | null;
    /**
     * Human-readable label
     */
    label?: string | null;
    /**
     * Permission set refs to apply to executions created by this rule. Omit to
     * keep the current value. Provide null to inherit the action default, or an
     * empty array to force no API token.
     */
    permission_set_refs?: any[] | null;
    /**
     * Parameters for trigger configuration and event filtering
     */
    trigger_params: any | null;
    /**
     * Trigger reference that activates this rule
     */
    trigger_ref?: string | null;
    /**
     * Optional template used to resolve execution trace tags for this rule.
     * Provide null to clear.
     */
    trace_tag_template?: string | null;
};
