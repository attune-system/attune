/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Simplified rule response (for list endpoints)
 */
export type RuleSummary = {
  /**
   * Parameters to pass to the action when rule is triggered
   */
  action_params: Record<string, any>;
  /**
   * Action reference
   */
  action_ref: string;
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
   * Human-readable label
   */
  label: string;
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
  sensor_worker_affinity: Record<string, any>;
  sensor_worker_selector: Record<string, any>;
  sensor_worker_tolerations: Array<Record<string, any>>;
  /**
   * Optional template used to resolve execution trace tags for this rule.
   */
  trace_tag_template?: string | null;
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
