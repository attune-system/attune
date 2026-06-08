/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CreateIdentityRequest } from '../models/CreateIdentityRequest';
import type { CreateIdentityRoleAssignmentRequest } from '../models/CreateIdentityRoleAssignmentRequest';
import type { CreateIntegrationTokenRequest } from '../models/CreateIntegrationTokenRequest';
import type { CreatePermissionAssignmentRequest } from '../models/CreatePermissionAssignmentRequest';
import type { CreatePermissionSetRoleAssignmentRequest } from '../models/CreatePermissionSetRoleAssignmentRequest';
import type { IdentityRoleAssignmentResponse } from '../models/IdentityRoleAssignmentResponse';
import type { IntegrationTokenResponse } from '../models/IntegrationTokenResponse';
import type { PaginatedResponse_IdentitySummary } from '../models/PaginatedResponse_IdentitySummary';
import type { PermissionAssignmentResponse } from '../models/PermissionAssignmentResponse';
import type { PermissionSetRoleAssignmentResponse } from '../models/PermissionSetRoleAssignmentResponse';
import type { PermissionSetSummary } from '../models/PermissionSetSummary';
import type { RevokeIntegrationTokenRequest } from '../models/RevokeIntegrationTokenRequest';
import type { UpdateIdentityRequest } from '../models/UpdateIdentityRequest';
import type { UpdatePermissionSetRequest } from '../models/UpdatePermissionSetRequest';
import type { Value } from '../models/Value';
import type { CancelablePromise } from '../core/CancelablePromise';
import { OpenAPI } from '../core/OpenAPI';
import { request as __request } from '../core/request';
export class PermissionsService {
    /**
     * @returns PaginatedResponse_IdentitySummary List identities
     * @throws ApiError
     */
    public static listIdentities({
        page,
        pageSize,
    }: {
        /**
         * Page number (1-based)
         */
        page?: number,
        /**
         * Number of items per page
         */
        pageSize?: number,
    }): CancelablePromise<PaginatedResponse_IdentitySummary> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/api/v1/identities',
            query: {
                'page': page,
                'page_size': pageSize,
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
        requestBody: CreateIdentityRequest,
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
            method: 'POST',
            url: '/api/v1/identities',
            body: requestBody,
            mediaType: 'application/json',
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
        id: number,
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
            method: 'DELETE',
            url: '/api/v1/identities/roles/{id}',
            path: {
                'id': id,
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
        id: number,
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
            method: 'GET',
            url: '/api/v1/identities/{id}',
            path: {
                'id': id,
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
        id: number,
        requestBody: UpdateIdentityRequest,
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
            method: 'PUT',
            url: '/api/v1/identities/{id}',
            path: {
                'id': id,
            },
            body: requestBody,
            mediaType: 'application/json',
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
        id: number,
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
            method: 'DELETE',
            url: '/api/v1/identities/{id}',
            path: {
                'id': id,
            },
            errors: {
                404: `Identity not found`,
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
        id: number,
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
            method: 'POST',
            url: '/api/v1/identities/{id}/freeze',
            path: {
                'id': id,
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
        id: number,
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
            method: 'GET',
            url: '/api/v1/identities/{id}/integration-tokens',
            path: {
                'id': id,
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
        id: number,
        requestBody: CreateIntegrationTokenRequest,
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
            method: 'POST',
            url: '/api/v1/identities/{id}/integration-tokens',
            path: {
                'id': id,
            },
            body: requestBody,
            mediaType: 'application/json',
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
        id: number,
        /**
         * Integration token ID
         */
        tokenId: number,
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
            method: 'DELETE',
            url: '/api/v1/identities/{id}/integration-tokens/{token_id}',
            path: {
                'id': id,
                'token_id': tokenId,
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
        id: number,
        /**
         * Integration token ID
         */
        tokenId: number,
        requestBody: RevokeIntegrationTokenRequest,
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
            method: 'POST',
            url: '/api/v1/identities/{id}/integration-tokens/{token_id}/revoke',
            path: {
                'id': id,
                'token_id': tokenId,
            },
            body: requestBody,
            mediaType: 'application/json',
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
        id: number,
    }): CancelablePromise<Array<PermissionAssignmentResponse>> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/api/v1/identities/{id}/permissions',
            path: {
                'id': id,
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
        id: number,
        requestBody: CreateIdentityRoleAssignmentRequest,
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
            method: 'POST',
            url: '/api/v1/identities/{id}/roles',
            path: {
                'id': id,
            },
            body: requestBody,
            mediaType: 'application/json',
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
        id: number,
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
            method: 'POST',
            url: '/api/v1/identities/{id}/unfreeze',
            path: {
                'id': id,
            },
            errors: {
                404: `Identity not found`,
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
        requestBody: CreatePermissionAssignmentRequest,
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
            method: 'POST',
            url: '/api/v1/permissions/assignments',
            body: requestBody,
            mediaType: 'application/json',
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
        id: number,
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
            method: 'DELETE',
            url: '/api/v1/permissions/assignments/{id}',
            path: {
                'id': id,
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
    }: {
        packRef?: string | null,
    }): CancelablePromise<Array<PermissionSetSummary>> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/api/v1/permissions/sets',
            query: {
                'pack_ref': packRef,
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
        id: number,
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
            method: 'DELETE',
            url: '/api/v1/permissions/sets/roles/{id}',
            path: {
                'id': id,
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
    }: {
        /**
         * Permission set ID
         */
        id: number,
        requestBody: UpdatePermissionSetRequest,
    }): CancelablePromise<{
        data: {
            description?: string | null;
            grants: Value;
            id: number;
            label?: string | null;
            pack_ref?: string | null;
            ref: string;
            roles: Array<PermissionSetRoleAssignmentResponse>;
        };
        /**
         * Optional message
         */
        message?: string | null;
    }> {
        return __request(OpenAPI, {
            method: 'PUT',
            url: '/api/v1/permissions/sets/{id}',
            path: {
                'id': id,
            },
            body: requestBody,
            mediaType: 'application/json',
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
        id: number,
        requestBody: CreatePermissionSetRoleAssignmentRequest,
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
            method: 'POST',
            url: '/api/v1/permissions/sets/{id}/roles',
            path: {
                'id': id,
            },
            body: requestBody,
            mediaType: 'application/json',
            errors: {
                404: `Permission set not found`,
            },
        });
    }
}
