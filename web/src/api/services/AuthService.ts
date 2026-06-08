/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ChangePasswordRequest } from '../models/ChangePasswordRequest';
import type { EffectivePermissionResponse } from '../models/EffectivePermissionResponse';
import type { LdapLoginRequest } from '../models/LdapLoginRequest';
import type { LoginRequest } from '../models/LoginRequest';
import type { ProviderProfileResponse } from '../models/ProviderProfileResponse';
import type { RefreshTokenRequest } from '../models/RefreshTokenRequest';
import type { RegisterRequest } from '../models/RegisterRequest';
import type { TokenLoginRequest } from '../models/TokenLoginRequest';
import type { UpdateCurrentUserRequest } from '../models/UpdateCurrentUserRequest';
import type { UserInfo } from '../models/UserInfo';
import type { CancelablePromise } from '../core/CancelablePromise';
import { OpenAPI } from '../core/OpenAPI';
import { request as __request } from '../core/request';
export class AuthService {
    /**
     * Change password endpoint
     * POST /auth/change-password
     * @returns any Password changed successfully
     * @throws ApiError
     */
    public static changePassword({
        requestBody,
    }: {
        requestBody: ChangePasswordRequest,
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
            url: '/auth/change-password',
            body: requestBody,
            mediaType: 'application/json',
            errors: {
                400: `Validation error`,
                401: `Invalid current password or unauthorized`,
                404: `Identity not found`,
            },
        });
    }
    /**
     * Authenticate via LDAP directory.
     * POST /auth/ldap/login
     * @returns any Successfully authenticated via LDAP
     * @throws ApiError
     */
    public static ldapLogin({
        requestBody,
    }: {
        requestBody: LdapLoginRequest,
    }): CancelablePromise<{
        /**
         * Token response
         */
        data: {
            /**
             * Access token (JWT)
             */
            access_token: string;
            /**
             * Access token expiration in seconds
             */
            expires_in: number;
            /**
             * Refresh token
             */
            refresh_token: string;
            /**
             * Token type (always "Bearer")
             */
            token_type: string;
            user?: (null | UserInfo);
        };
        /**
         * Optional message
         */
        message?: string | null;
    }> {
        return __request(OpenAPI, {
            method: 'POST',
            url: '/auth/ldap/login',
            body: requestBody,
            mediaType: 'application/json',
            errors: {
                401: `Invalid LDAP credentials`,
                501: `LDAP not configured`,
            },
        });
    }
    /**
     * Login endpoint
     * POST /auth/login
     * @returns any Successfully logged in
     * @throws ApiError
     */
    public static login({
        requestBody,
    }: {
        requestBody: LoginRequest,
    }): CancelablePromise<{
        /**
         * Token response
         */
        data: {
            /**
             * Access token (JWT)
             */
            access_token: string;
            /**
             * Access token expiration in seconds
             */
            expires_in: number;
            /**
             * Refresh token
             */
            refresh_token: string;
            /**
             * Token type (always "Bearer")
             */
            token_type: string;
            user?: (null | UserInfo);
        };
        /**
         * Optional message
         */
        message?: string | null;
    }> {
        return __request(OpenAPI, {
            method: 'POST',
            url: '/auth/login',
            body: requestBody,
            mediaType: 'application/json',
            errors: {
                400: `Validation error`,
                401: `Invalid credentials`,
            },
        });
    }
    /**
     * Get current user endpoint
     * GET /auth/me
     * @returns any Current user information
     * @throws ApiError
     */
    public static getCurrentUser(): CancelablePromise<{
        /**
         * Current user response
         */
        data: {
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
            provider_profile?: (null | ProviderProfileResponse);
        };
        /**
         * Optional message
         */
        message?: string | null;
    }> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/auth/me',
            errors: {
                401: `Unauthorized`,
                404: `Identity not found`,
            },
        });
    }
    /**
     * Update current user profile endpoint
     * PUT /auth/me
     * @returns any Current user profile updated
     * @throws ApiError
     */
    public static updateCurrentUser({
        requestBody,
    }: {
        requestBody: UpdateCurrentUserRequest,
    }): CancelablePromise<{
        /**
         * Current user response
         */
        data: {
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
            provider_profile?: (null | ProviderProfileResponse);
        };
        /**
         * Optional message
         */
        message?: string | null;
    }> {
        return __request(OpenAPI, {
            method: 'PUT',
            url: '/auth/me',
            body: requestBody,
            mediaType: 'application/json',
            errors: {
                400: `Validation error`,
                401: `Unauthorized`,
                403: `Profile is managed by an external provider`,
                404: `Identity not found`,
            },
        });
    }
    /**
     * Refresh token endpoint
     * POST /auth/refresh
     * @returns any Successfully refreshed token
     * @throws ApiError
     */
    public static refreshToken({
        requestBody,
    }: {
        requestBody: RefreshTokenRequest,
    }): CancelablePromise<{
        /**
         * Token response
         */
        data: {
            /**
             * Access token (JWT)
             */
            access_token: string;
            /**
             * Access token expiration in seconds
             */
            expires_in: number;
            /**
             * Refresh token
             */
            refresh_token: string;
            /**
             * Token type (always "Bearer")
             */
            token_type: string;
            user?: (null | UserInfo);
        };
        /**
         * Optional message
         */
        message?: string | null;
    }> {
        return __request(OpenAPI, {
            method: 'POST',
            url: '/auth/refresh',
            body: requestBody,
            mediaType: 'application/json',
            errors: {
                400: `Validation error`,
                401: `Invalid or expired refresh token`,
            },
        });
    }
    /**
     * Register endpoint
     * POST /auth/register
     * @returns any Successfully registered
     * @throws ApiError
     */
    public static register({
        requestBody,
    }: {
        requestBody: RegisterRequest,
    }): CancelablePromise<{
        /**
         * Token response
         */
        data: {
            /**
             * Access token (JWT)
             */
            access_token: string;
            /**
             * Access token expiration in seconds
             */
            expires_in: number;
            /**
             * Refresh token
             */
            refresh_token: string;
            /**
             * Token type (always "Bearer")
             */
            token_type: string;
            user?: (null | UserInfo);
        };
        /**
         * Optional message
         */
        message?: string | null;
    }> {
        return __request(OpenAPI, {
            method: 'POST',
            url: '/auth/register',
            body: requestBody,
            mediaType: 'application/json',
            errors: {
                400: `Validation error`,
                409: `User already exists`,
            },
        });
    }
    /**
     * Authentication settings endpoint
     * GET /auth/settings
     * @returns any Authentication settings
     * @throws ApiError
     */
    public static authSettings(): CancelablePromise<{
        /**
         * Public authentication settings for the login page.
         */
        data: {
            /**
             * Whether authentication is enabled for the server.
             */
            authentication_enabled: boolean;
            /**
             * Whether LDAP login is configured and enabled.
             */
            ldap_enabled: boolean;
            /**
             * Optional icon URL shown beside the provider label.
             */
            ldap_provider_icon_url?: string | null;
            /**
             * User-facing provider label for the login button.
             */
            ldap_provider_label?: string | null;
            /**
             * Provider name for `?auth=<provider>`.
             */
            ldap_provider_name?: string | null;
            /**
             * Whether LDAP login should be shown by default.
             */
            ldap_visible_by_default: boolean;
            /**
             * Whether local username/password login is configured.
             */
            local_password_enabled: boolean;
            /**
             * Whether local username/password login should be shown by default.
             */
            local_password_visible_by_default: boolean;
            /**
             * Whether OIDC login is configured and enabled.
             */
            oidc_enabled: boolean;
            /**
             * Optional icon URL shown beside the provider label.
             */
            oidc_provider_icon_url?: string | null;
            /**
             * User-facing provider label for the login button.
             */
            oidc_provider_label?: string | null;
            /**
             * Provider name for `?auth=<provider>`.
             */
            oidc_provider_name?: string | null;
            /**
             * Whether OIDC login should be shown by default.
             */
            oidc_visible_by_default: boolean;
            /**
             * Whether unauthenticated self-service registration is allowed.
             */
            self_registration_enabled: boolean;
        };
        /**
         * Optional message
         */
        message?: string | null;
    }> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/auth/settings',
        });
    }
    /**
     * Passwordless integration-token login endpoint.
     * POST /auth/token-login
     * @returns any Successfully logged in with integration token
     * @throws ApiError
     */
    public static tokenLogin({
        requestBody,
    }: {
        requestBody: TokenLoginRequest,
    }): CancelablePromise<{
        /**
         * Token response
         */
        data: {
            /**
             * Access token (JWT)
             */
            access_token: string;
            /**
             * Access token expiration in seconds
             */
            expires_in: number;
            /**
             * Refresh token
             */
            refresh_token: string;
            /**
             * Token type (always "Bearer")
             */
            token_type: string;
            user?: (null | UserInfo);
        };
        /**
         * Optional message
         */
        message?: string | null;
    }> {
        return __request(OpenAPI, {
            method: 'POST',
            url: '/auth/token-login',
            body: requestBody,
            mediaType: 'application/json',
            errors: {
                400: `Validation error`,
                401: `Invalid integration token`,
            },
        });
    }
}
