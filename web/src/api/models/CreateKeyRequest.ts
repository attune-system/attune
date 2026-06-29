/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
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
   * Human-readable name for the key
   */
  name: string;
  /**
   * Optional owner string identifier
   */
  owner?: string | null;
  owner_action?: null | i64;
  /**
   * Optional owner action reference
   */
  owner_action_ref?: string | null;
  owner_identity?: null | i64;
  owner_pack?: null | i64;
  /**
   * Optional owner pack reference
   */
  owner_pack_ref?: string | null;
  owner_sensor?: null | i64;
  /**
   * Optional owner sensor reference
   */
  owner_sensor_ref?: string | null;
  /**
   * Type of owner (system, identity, pack, action, sensor)
   */
  owner_type: OwnerType;
  /**
   * Unique reference for the key (e.g., "github_token", "aws_secret_key")
   */
  ref: string;
  /**
   * The secret value to store. Can be a string, object, array, number, or boolean.
   */
  value: any;
};
