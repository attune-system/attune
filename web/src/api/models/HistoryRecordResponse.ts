/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Response DTO for a single entity history record.
 */
export type HistoryRecordResponse = {
  /**
   * Names of fields that changed (empty for INSERT/DELETE)
   */
  changed_fields: Array<string>;
  /**
   * The primary key of the changed entity
   */
  entity_id: number;
  /**
   * Denormalized human-readable identifier (e.g., action_ref, worker name)
   */
  entity_ref?: string | null;
  /**
   * New values of changed fields (null for DELETE)
   */
  new_values: Record<string, any>;
  /**
   * Previous values of changed fields (null for INSERT)
   */
  old_values: Record<string, any>;
  /**
   * The operation: `INSERT`, `UPDATE`, or `DELETE`
   */
  operation: string;
  /**
   * When the change occurred
   */
  time: string;
};
