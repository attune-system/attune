/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
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
   * Update the secret value. Can be a string, object, array, number, or boolean.
   */
  value?: any;
};
