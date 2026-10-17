/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ApiResponse_RuntimeApprovalResponse } from "../models/ApiResponse_RuntimeApprovalResponse";
import type { CreateIdentityRequest } from "../models/CreateIdentityRequest";
import type { CreateIdentityRoleAssignmentRequest } from "../models/CreateIdentityRoleAssignmentRequest";
import type { CreateIntegrationTokenRequest } from "../models/CreateIntegrationTokenRequest";
import type { CreatePermissionAssignmentRequest } from "../models/CreatePermissionAssignmentRequest";
import type { CreatePermissionSetRequest } from "../models/CreatePermissionSetRequest";
import type { CreatePermissionSetRoleAssignmentRequest } from "../models/CreatePermissionSetRoleAssignmentRequest";
import type { IdentityRoleAssignmentResponse } from "../models/IdentityRoleAssignmentResponse";
import type { IntegrationTokenResponse } from "../models/IntegrationTokenResponse";
import type { IssueDeclaredInputApprovalRequest } from "../models/IssueDeclaredInputApprovalRequest";
import type { IssueRuntimeApprovalRequest } from "../models/IssueRuntimeApprovalRequest";
import type { ManagementOriginKind } from "../models/ManagementOriginKind";
import type { PaginatedResponse_DeclaredInputApprovalTarget } from "../models/PaginatedResponse_DeclaredInputApprovalTarget";
import type { PaginatedResponse_IdentitySummary } from "../models/PaginatedResponse_IdentitySummary";
import type { PaginatedResponse_PermissionBindingResponse } from "../models/PaginatedResponse_PermissionBindingResponse";
import type { PaginatedResponse_RuntimeApprovalResponse } from "../models/PaginatedResponse_RuntimeApprovalResponse";
import type { PermissionAssignmentResponse } from "../models/PermissionAssignmentResponse";
import type { PermissionSetRoleAssignmentResponse } from "../models/PermissionSetRoleAssignmentResponse";
import type { PermissionSetSummary } from "../models/PermissionSetSummary";
import type { RevokeIntegrationTokenRequest } from "../models/RevokeIntegrationTokenRequest";
import type { UpdateIdentityRequest } from "../models/UpdateIdentityRequest";
import type { UpdatePermissionSetRequest } from "../models/UpdatePermissionSetRequest";
import type { Value } from "../models/Value";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class PermissionsService {
  /**
   * @returns PaginatedResponse_IdentitySummary List identities
   * @throws ApiError
   */
  public static listIdentities({
    page,
    pageSize,
    login,
  }: {
    /**
     * Page number (1-based)
     */
    page?: number;
    /**
     * Number of items per page
     */
    pageSize?: number;
    /**
     * Exact identity login.
     */
    login?: string | null;
  }): CancelablePromise<PaginatedResponse_IdentitySummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/identities",
      query: {
        page: page,
        page_size: pageSize,
        login: login,
      },
    });
  }
  /**
   * @returns any Identity created
   * @throws ApiError
   */
  public static createIdentity({
    requestBody,
  }: {
    requestBody: CreateIdentityRequest;
  }): CancelablePromise<{
    data: {
      attributes: Value;
      direct_permissions: Array<PermissionAssignmentResponse>;
      display_name?: string | null;
      frozen: boolean;
      id: number;
      login: string;
      roles: Array<IdentityRoleAssignmentResponse>;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/identities",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        409: `Identity already exists`,
      },
    });
  }
  /**
   * @returns any Identity role assignment deleted
   * @throws ApiError
   */
  public static deleteIdentityRoleAssignment({
    id,
  }: {
    /**
     * Identity role assignment ID
     */
    id: number;
  }): CancelablePromise<{
    /**
     * Success message response (for operations that don't return data)
     */
    data: {
      /**
       * Message describing the operation
       */
      message: string;
      /**
       * Success indicator
       */
      success: boolean;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "DELETE",
      url: "/api/v1/identities/roles/{id}",
      path: {
        id: id,
      },
      errors: {
        404: `Identity role assignment not found`,
      },
    });
  }
  /**
   * @returns any Identity details
   * @throws ApiError
   */
  public static getIdentity({
    id,
  }: {
    /**
     * Identity ID
     */
    id: number;
  }): CancelablePromise<{
    data: {
      attributes: Value;
      direct_permissions: Array<PermissionAssignmentResponse>;
      display_name?: string | null;
      frozen: boolean;
      id: number;
      login: string;
      roles: Array<IdentityRoleAssignmentResponse>;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/identities/{id}",
      path: {
        id: id,
      },
      errors: {
        404: `Identity not found`,
      },
    });
  }
  /**
   * @returns any Identity updated
   * @throws ApiError
   */
  public static updateIdentity({
    id,
    requestBody,
  }: {
    /**
     * Identity ID
     */
    id: number;
    requestBody: UpdateIdentityRequest;
  }): CancelablePromise<{
    data: {
      attributes: Value;
      direct_permissions: Array<PermissionAssignmentResponse>;
      display_name?: string | null;
      frozen: boolean;
      id: number;
      login: string;
      roles: Array<IdentityRoleAssignmentResponse>;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "PUT",
      url: "/api/v1/identities/{id}",
      path: {
        id: id,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        404: `Identity not found`,
      },
    });
  }
  /**
   * @returns any Identity deleted
   * @throws ApiError
   */
  public static deleteIdentity({
    id,
  }: {
    /**
     * Identity ID
     */
    id: number;
  }): CancelablePromise<{
    /**
     * Success message response (for operations that don't return data)
     */
    data: {
      /**
       * Message describing the operation
       */
      message: string;
      /**
       * Success indicator
       */
      success: boolean;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "DELETE",
      url: "/api/v1/identities/{id}",
      path: {
        id: id,
      },
      errors: {
        404: `Identity not found`,
        409: `Identity cache cleanup is pending`,
      },
    });
  }
  /**
   * @returns any Identity frozen
   * @throws ApiError
   */
  public static freezeIdentity({
    id,
  }: {
    /**
     * Identity ID
     */
    id: number;
  }): CancelablePromise<{
    /**
     * Success message response (for operations that don't return data)
     */
    data: {
      /**
       * Message describing the operation
       */
      message: string;
      /**
       * Success indicator
       */
      success: boolean;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/identities/{id}/freeze",
      path: {
        id: id,
      },
      errors: {
        404: `Identity not found`,
      },
    });
  }
  /**
   * @returns any List integration tokens
   * @throws ApiError
   */
  public static listIntegrationTokens({
    id,
  }: {
    /**
     * Identity ID
     */
    id: number;
  }): CancelablePromise<{
    data: Array<{
      active: boolean;
      created: string;
      created_by?: number | null;
      description?: string | null;
      expires_at?: string | null;
      id: number;
      identity_id: number;
      label: string;
      last_used_at?: string | null;
      last_used_ip?: string | null;
      revocation_reason?: string | null;
      revoked_at?: string | null;
      revoked_by?: number | null;
      token_prefix: string;
      token_suffix: string;
      updated: string;
    }>;
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/identities/{id}/integration-tokens",
      path: {
        id: id,
      },
      errors: {
        404: `Identity not found`,
      },
    });
  }
  /**
   * @returns any Integration token created
   * @throws ApiError
   */
  public static createIntegrationToken({
    id,
    requestBody,
  }: {
    /**
     * Identity ID
     */
    id: number;
    requestBody: CreateIntegrationTokenRequest;
  }): CancelablePromise<{
    data: {
      integration_token: IntegrationTokenResponse;
      token: string;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/identities/{id}/integration-tokens",
      path: {
        id: id,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        404: `Identity not found`,
      },
    });
  }
  /**
   * @returns any Integration token deleted
   * @throws ApiError
   */
  public static deleteIntegrationToken({
    id,
    tokenId,
  }: {
    /**
     * Identity ID
     */
    id: number;
    /**
     * Integration token ID
     */
    tokenId: number;
  }): CancelablePromise<{
    /**
     * Success message response (for operations that don't return data)
     */
    data: {
      /**
       * Message describing the operation
       */
      message: string;
      /**
       * Success indicator
       */
      success: boolean;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "DELETE",
      url: "/api/v1/identities/{id}/integration-tokens/{token_id}",
      path: {
        id: id,
        token_id: tokenId,
      },
      errors: {
        404: `Integration token not found`,
      },
    });
  }
  /**
   * @returns any Integration token revoked
   * @throws ApiError
   */
  public static revokeIntegrationToken({
    id,
    tokenId,
    requestBody,
  }: {
    /**
     * Identity ID
     */
    id: number;
    /**
     * Integration token ID
     */
    tokenId: number;
    requestBody: RevokeIntegrationTokenRequest;
  }): CancelablePromise<{
    data: {
      active: boolean;
      created: string;
      created_by?: number | null;
      description?: string | null;
      expires_at?: string | null;
      id: number;
      identity_id: number;
      label: string;
      last_used_at?: string | null;
      last_used_ip?: string | null;
      revocation_reason?: string | null;
      revoked_at?: string | null;
      revoked_by?: number | null;
      token_prefix: string;
      token_suffix: string;
      updated: string;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/identities/{id}/integration-tokens/{token_id}/revoke",
      path: {
        id: id,
        token_id: tokenId,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        404: `Integration token not found`,
      },
    });
  }
  /**
   * @returns PermissionAssignmentResponse List permission assignments for an identity
   * @throws ApiError
   */
  public static listIdentityPermissions({
    id,
  }: {
    /**
     * Identity ID
     */
    id: number;
  }): CancelablePromise<Array<PermissionAssignmentResponse>> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/identities/{id}/permissions",
      path: {
        id: id,
      },
      errors: {
        404: `Identity not found`,
      },
    });
  }
  /**
   * @returns any Identity role assignment created
   * @throws ApiError
   */
  public static createIdentityRoleAssignment({
    id,
    requestBody,
  }: {
    /**
     * Identity ID
     */
    id: number;
    requestBody: CreateIdentityRoleAssignmentRequest;
  }): CancelablePromise<{
    data: {
      created: string;
      id: number;
      identity_id: number;
      managed: boolean;
      role: string;
      source: string;
      updated: string;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/identities/{id}/roles",
      path: {
        id: id,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        404: `Identity not found`,
      },
    });
  }
  /**
   * @returns any Identity unfrozen
   * @throws ApiError
   */
  public static unfreezeIdentity({
    id,
  }: {
    /**
     * Identity ID
     */
    id: number;
  }): CancelablePromise<{
    /**
     * Success message response (for operations that don't return data)
     */
    data: {
      /**
       * Message describing the operation
       */
      message: string;
      /**
       * Success indicator
       */
      success: boolean;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/identities/{id}/unfreeze",
      path: {
        id: id,
      },
      errors: {
        404: `Identity not found`,
      },
    });
  }
  /**
   * @returns PaginatedResponse_RuntimeApprovalResponse Fixed external approvals, including revoked and expired records
   * @throws ApiError
   */
  public static listRuntimeApprovals({
    packRef,
    page,
    pageSize,
  }: {
    packRef: string;
    /**
     * Page number (1-based)
     */
    page?: number;
    /**
     * Number of items per page
     */
    pageSize?: number;
  }): CancelablePromise<PaginatedResponse_RuntimeApprovalResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/packs/{pack_ref}/external-approvals",
      path: {
        pack_ref: packRef,
      },
      query: {
        page: page,
        page_size: pageSize,
      },
    });
  }
  /**
   * @returns ApiResponse_RuntimeApprovalResponse External approval issued after current authority validation
   * @throws ApiError
   */
  public static issueRuntimeApproval({
    packRef,
    requestBody,
  }: {
    packRef: string;
    requestBody: IssueRuntimeApprovalRequest;
  }): CancelablePromise<ApiResponse_RuntimeApprovalResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/packs/{pack_ref}/external-approvals",
      path: {
        pack_ref: packRef,
      },
      body: requestBody,
      mediaType: "application/json",
    });
  }
  /**
   * @returns ApiResponse_RuntimeApprovalResponse Immutable external approval
   * @throws ApiError
   */
  public static getRuntimeApproval({
    packRef,
    id,
  }: {
    packRef: string;
    id: number;
  }): CancelablePromise<ApiResponse_RuntimeApprovalResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/packs/{pack_ref}/external-approvals/{id}",
      path: {
        pack_ref: packRef,
        id: id,
      },
    });
  }
  /**
   * @returns ApiResponse_RuntimeApprovalResponse New approval issued and predecessor explicitly revoked atomically
   * @throws ApiError
   */
  public static replaceRuntimeApproval({
    packRef,
    id,
    requestBody,
  }: {
    packRef: string;
    id: number;
    requestBody: IssueRuntimeApprovalRequest;
  }): CancelablePromise<ApiResponse_RuntimeApprovalResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/packs/{pack_ref}/external-approvals/{id}/replace",
      path: {
        pack_ref: packRef,
        id: id,
      },
      body: requestBody,
      mediaType: "application/json",
    });
  }
  /**
   * @returns ApiResponse_RuntimeApprovalResponse Approval explicitly revoked
   * @throws ApiError
   */
  public static revokeRuntimeApproval({
    packRef,
    id,
  }: {
    packRef: string;
    id: number;
  }): CancelablePromise<ApiResponse_RuntimeApprovalResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/packs/{pack_ref}/external-approvals/{id}/revoke",
      path: {
        pack_ref: packRef,
        id: id,
      },
    });
  }
  /**
   * @returns ApiResponse_RuntimeApprovalResponse Independent fixed approval for one retained input declaration, with no execution API grants
   * @throws ApiError
   */
  public static issueDeclaredInputApproval({
    packRef,
    requestBody,
  }: {
    packRef: string;
    requestBody: IssueDeclaredInputApprovalRequest;
  }): CancelablePromise<ApiResponse_RuntimeApprovalResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/packs/{pack_ref}/external-input-approvals",
      path: {
        pack_ref: packRef,
      },
      body: requestBody,
      mediaType: "application/json",
    });
  }
  /**
   * @returns PaginatedResponse_DeclaredInputApprovalTarget Authorized immutable input targets, filtered before pagination; metadata is not consent or an invocation permit
   * @throws ApiError
   */
  public static listInputApprovalTargets({
    packRef,
    policyRevision,
    page,
    pageSize,
  }: {
    packRef: string;
    /**
     * Omit for this pack's current policy; explicit IDs select retained metadata.
     */
    policyRevision?: number | null;
    page?: number;
    pageSize?: number;
  }): CancelablePromise<PaginatedResponse_DeclaredInputApprovalTarget> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/packs/{pack_ref}/input-approval-targets",
      path: {
        pack_ref: packRef,
      },
      query: {
        policy_revision: policyRevision,
        page: page,
        page_size: pageSize,
      },
    });
  }
  /**
   * @returns PaginatedResponse_PermissionBindingResponse Direct identity and role permission assignments
   * @throws ApiError
   */
  public static listPermissionAssignments({
    page,
    pageSize,
    identityId,
    identityLogin,
    role,
    permissionSetRef,
  }: {
    /**
     * Page number (1-based)
     */
    page?: number;
    /**
     * Number of items per page
     */
    pageSize?: number;
    identityId?: number | null;
    identityLogin?: string | null;
    role?: string | null;
    permissionSetRef?: string | null;
  }): CancelablePromise<PaginatedResponse_PermissionBindingResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/permissions/assignments",
      query: {
        page: page,
        page_size: pageSize,
        identity_id: identityId,
        identity_login: identityLogin,
        role: role,
        permission_set_ref: permissionSetRef,
      },
    });
  }
  /**
   * @returns any Permission assignment created
   * @throws ApiError
   */
  public static createPermissionAssignment({
    requestBody,
  }: {
    requestBody: CreatePermissionAssignmentRequest;
  }): CancelablePromise<{
    data: {
      created: string;
      id: number;
      identity_id: number;
      permission_set_id: number;
      permission_set_ref: string;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/permissions/assignments",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        404: `Identity or permission set not found`,
        409: `Assignment already exists`,
      },
    });
  }
  /**
   * @returns any Permission assignment deleted
   * @throws ApiError
   */
  public static deletePermissionAssignment({
    id,
  }: {
    /**
     * Permission assignment ID
     */
    id: number;
  }): CancelablePromise<{
    /**
     * Success message response (for operations that don't return data)
     */
    data: {
      /**
       * Message describing the operation
       */
      message: string;
      /**
       * Success indicator
       */
      success: boolean;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "DELETE",
      url: "/api/v1/permissions/assignments/{id}",
      path: {
        id: id,
      },
      errors: {
        404: `Assignment not found`,
      },
    });
  }
  /**
   * @returns PermissionSetSummary List permission sets
   * @throws ApiError
   */
  public static listPermissionSets({
    packRef,
    includeRetired,
  }: {
    packRef?: string | null;
    /**
     * Include retired permission sets in administrative results.
     */
    includeRetired?: boolean;
  }): CancelablePromise<Array<PermissionSetSummary>> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/permissions/sets",
      query: {
        pack_ref: packRef,
        include_retired: includeRetired,
      },
    });
  }
  /**
   * @returns any Permission set created
   * @throws ApiError
   */
  public static createPermissionSet({
    requestBody,
  }: {
    requestBody: CreatePermissionSetRequest;
  }): CancelablePromise<{
    data: {
      authored_definition: Value;
      /**
       * Canonical authored scope, distinct from normalized effective ownership.
       */
      authored_grants: Value;
      description?: string | null;
      grants: Value;
      id: number;
      label?: string | null;
      management_origin: ManagementOriginKind;
      pack_ref?: string | null;
      ref: string;
      retired_at?: string | null;
      roles: Array<PermissionSetRoleAssignmentResponse>;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/permissions/sets",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid reference or grants`,
        409: `Permission set reference already exists`,
        422: `Validation error`,
      },
    });
  }
  /**
   * @returns any Permission set details, including retired sets
   * @throws ApiError
   */
  public static getPermissionSet({
    permissionSetRef,
  }: {
    /**
     * Permission set ref
     */
    permissionSetRef: string;
  }): CancelablePromise<{
    data: {
      authored_definition: Value;
      /**
       * Canonical authored scope, distinct from normalized effective ownership.
       */
      authored_grants: Value;
      description?: string | null;
      grants: Value;
      id: number;
      label?: string | null;
      management_origin: ManagementOriginKind;
      pack_ref?: string | null;
      ref: string;
      retired_at?: string | null;
      roles: Array<PermissionSetRoleAssignmentResponse>;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/permissions/sets/by-ref/{permission_set_ref}",
      path: {
        permission_set_ref: permissionSetRef,
      },
    });
  }
  /**
   * @returns any Permission set role assignment deleted
   * @throws ApiError
   */
  public static deletePermissionSetRoleAssignment({
    id,
  }: {
    /**
     * Permission set role assignment ID
     */
    id: number;
  }): CancelablePromise<{
    /**
     * Success message response (for operations that don't return data)
     */
    data: {
      /**
       * Message describing the operation
       */
      message: string;
      /**
       * Success indicator
       */
      success: boolean;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "DELETE",
      url: "/api/v1/permissions/sets/roles/{id}",
      path: {
        id: id,
      },
      errors: {
        404: `Permission set role assignment not found`,
      },
    });
  }
  /**
   * @returns any Permission set updated
   * @throws ApiError
   */
  public static updatePermissionSet({
    id,
    requestBody,
    dryRun,
  }: {
    /**
     * Permission set ID
     */
    id: number;
    requestBody: UpdatePermissionSetRequest;
    /**
     * Validate and preview the update without persisting it.
     */
    dryRun?: boolean;
  }): CancelablePromise<{
    data: {
      authored_definition: Value;
      /**
       * Canonical authored scope, distinct from normalized effective ownership.
       */
      authored_grants: Value;
      description?: string | null;
      grants: Value;
      id: number;
      label?: string | null;
      management_origin: ManagementOriginKind;
      pack_ref?: string | null;
      ref: string;
      retired_at?: string | null;
      roles: Array<PermissionSetRoleAssignmentResponse>;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "PUT",
      url: "/api/v1/permissions/sets/{id}",
      path: {
        id: id,
      },
      query: {
        dry_run: dryRun,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Validation error`,
        404: `Permission set not found`,
      },
    });
  }
  /**
   * @returns any Permission set role assignment created
   * @throws ApiError
   */
  public static createPermissionSetRoleAssignment({
    id,
    requestBody,
  }: {
    /**
     * Permission set ID
     */
    id: number;
    requestBody: CreatePermissionSetRoleAssignmentRequest;
  }): CancelablePromise<{
    data: {
      created: string;
      id: number;
      permission_set_id: number;
      permission_set_ref?: string | null;
      role: string;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/permissions/sets/{id}/roles",
      path: {
        id: id,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        404: `Permission set not found`,
      },
    });
  }
}
