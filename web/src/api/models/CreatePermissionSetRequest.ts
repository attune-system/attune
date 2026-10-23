/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { PermissionScope } from "./PermissionScope";
import type { Value } from "./Value";
export type CreatePermissionSetRequest = {
  description?: string | null;
  grants: Value;
  label?: string | null;
  /**
   * Unique reference in namespace.name format. Does not imply pack ownership.
   */
  ref: string;
  scope?: null | PermissionScope;
};
