/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
export type InvocationBinding =
  | {
      action_ref: string;
      kind: "action";
    }
  | {
      action_ref: string;
      kind: "workflow_task";
      task_path: string;
      workflow_ref: string;
    }
  | {
      action_ref: string;
      kind: "rule";
      rule_ref: string;
    }
  | {
      action_ref: string;
      kind: "queue";
      queue_ref: string;
    };
