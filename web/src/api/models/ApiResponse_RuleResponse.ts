/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Standard API response wrapper
 */
export type ApiResponse_RuleResponse = {
    /**
     * Response DTO for rule information
     */
    data: {
        /**
         * Action ID (null if the referenced action has been deleted)
         */
        action?: number | null;
        /**
         * Parameters to pass to the action when rule is triggered
         */
        action_params: Record<string, any>;
        /**
         * Action reference
         */
        action_ref: string;
        /**
         * Conditions for rule evaluation
         */
        conditions: Record<string, any>;
        /**
         * Creation timestamp
         */
        created: string;
        /**
         * Rule description
         */
        description?: string | null;
        /**
         * Whether the rule is enabled
         */
        enabled: boolean;
        /**
         * Rule ID
         */
        id: number;
        /**
         * Whether this is an ad-hoc rule (not from pack installation)
         */
        is_adhoc: boolean;
        /**
         * Human-readable label
         */
        label: string;
        /**
         * Identity that registered the rule. NULL for system-loaded rules.
         */
        owner_identity?: number | null;
        /**
         * Pack ID
         */
        pack: number;
        /**
         * Pack reference
         */
        pack_ref: string;
        /**
         * Optional execution permission override. Null means inherit action default;
         * empty array means force no execution API token.
         */
        permission_set_refs?: any[] | null;
        /**
         * Unique reference identifier
         */
        ref: string;
        /**
         * Trigger ID (null if the referenced trigger has been deleted)
         */
        trigger?: number | null;
        /**
         * Parameters for trigger configuration and event filtering
         */
        trigger_params: Record<string, any>;
        /**
         * Trigger reference
         */
        trigger_ref: string;
        /**
         * Last update timestamp
         */
        updated: string;
    };
    /**
     * Optional message
     */
    message?: string | null;
};

