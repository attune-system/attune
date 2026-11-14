/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { Value } from "./Value";
export type UpdateIdentityRequest = {
  attributes?: null | Value;
  /**
   * Profile field; requires identities:update on the target.
   */
  display_name?: string | null;
  /**
   * Administrative freeze state; requires identities:manage on the target.
   */
  frozen?: boolean | null;
  /**
   * Password reset; requires identities:manage on the target.
   */
  password?: string | null;
};
