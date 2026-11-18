/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { AssignedAction } from "./AssignedAction";
export type NextActionResponse =
  | {
      state: "idle";
    }
  | {
      assignment: AssignedAction;
      state: "assigned";
    };
