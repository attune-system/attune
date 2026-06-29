/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Response DTO for queue statistics
 */
export type QueueStatsResponse = {
  /**
   * Action ID
   */
  action_id: number;
  /**
   * Action reference
   */
  action_ref: string;
  /**
   * Number of currently running executions
   */
  active_count: number;
  /**
   * Timestamp of last statistics update
   */
  last_updated: string;
  /**
   * Maximum concurrent executions allowed
   */
  max_concurrent: number;
  /**
   * Timestamp of oldest queued execution (if any)
   */
  oldest_enqueued_at?: string | null;
  /**
   * Number of executions waiting in queue
   */
  queue_length: number;
  /**
   * Total executions completed since queue creation
   */
  total_completed: number;
  /**
   * Total executions enqueued since queue creation
   */
  total_enqueued: number;
};
