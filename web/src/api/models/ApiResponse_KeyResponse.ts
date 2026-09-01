/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
import type { OwnerType } from "./OwnerType";
/**
 * Standard API response wrapper
 */
export type ApiResponse_KeyResponse = {
  /**
   * Full key response with all details (value redacted in list views)
   */
  data: {
    /**
     * Creation timestamp
     */
    created: string;
    /**
     * Whether the value is encrypted
     */
    encrypted: boolean;
    /**
     * Unique key ID
     */
    id: i64;
    /**
     * Key identifier within its owner scope
     */
    local_ref: string;
    /**
     * Human-readable name
     */
    name: string;
    /**
     * Authoritative owner reference
     */
    owner?: string | null;
    owner_action?: null | i64;
    /**
     * Owner action reference
     */
    owner_action_ref?: string | null;
    owner_identity?: null | i64;
    owner_pack?: null | i64;
    /**
     * Owner pack reference
     */
    owner_pack_ref?: string | null;
    owner_sensor?: null | i64;
    /**
     * Owner sensor reference
     */
    owner_sensor_ref?: string | null;
    /**
     * Type of owner
     */
    owner_type: OwnerType;
    /**
     * Unique reference identifier
     */
    ref: string;
    /**
     * Last update timestamp
     */
    updated: string;
    /**
     * The secret value (decrypted if encrypted). Can be a string, object, array, number, or boolean.
     */
    value: any;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
