/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { Value } from "./Value";
/**
 * Request to update an existing key/secret
 */
export type UpdateKeyRequest = {
  /**
   * Update encryption status (re-encrypts if changing from false to true)
   */
  encrypted?: boolean | null;
  /**
   * Update the human-readable name
   */
  name?: string | null;
  /**
   * Explicitly select current installed named revisions and revalidate.
   */
  rebind_schema?: boolean;
  schema_digest?: string | null;
  /**
   * Update the secret value. Can be a string, object, array, number, or boolean.
   */
  value?: any;
  value_schema?: null | Value;
};
