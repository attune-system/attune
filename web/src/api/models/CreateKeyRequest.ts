/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { OwnerType } from "./OwnerType";
/**
 * Request to create a new key/secret
 */
export type CreateKeyRequest = {
  /**
   * Whether to encrypt the value at rest (default: false; use --encrypt / -e from CLI)
   */
  encrypted?: boolean;
  /**
   * Identifier within the selected owner scope. The server uses it to construct the canonical ref.
   */
  local_ref: string;
  /**
   * Human-readable name for the key
   */
  name: string;
  /**
   * Optional owner action reference
   */
  owner_action_ref?: string | null;
  /**
   * Optional owner identity login
   */
  owner_identity_login?: string | null;
  /**
   * Optional owner pack reference
   */
  owner_pack_ref?: string | null;
  /**
   * Optional owner sensor reference
   */
  owner_sensor_ref?: string | null;
  /**
   * Type of owner (system, identity, pack, action, sensor)
   */
  owner_type: OwnerType;
  /**
   * The secret value to store. Can be a string, object, array, number, or boolean.
   */
  value: any;
};
