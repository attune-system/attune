/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { EffectivePermissionResponse } from "./EffectivePermissionResponse";
import type { ProviderProfileResponse } from "./ProviderProfileResponse";
/**
 * Current user response
 */
export type CurrentUserResponse = {
  /**
   * Permission set refs assigned to this identity, including role-derived assignments.
   */
  assigned_permission_set_refs: Array<string>;
  /**
   * Authentication provider backing this identity.
   */
  auth_provider: string;
  /**
   * Whether this identity can change its password through Attune.
   */
  can_change_password: boolean;
  /**
   * Display name
   */
  display_name?: string | null;
  /**
   * Effective resource-level permissions assigned to this identity.
   */
  effective_permissions: Array<EffectivePermissionResponse>;
  /**
   * Identity ID
   */
  id: number;
  /**
   * Whether this identity is managed locally by Attune.
   */
  is_local: boolean;
  /**
   * Identity login
   */
  login: string;
  provider_profile?: null | ProviderProfileResponse;
};
