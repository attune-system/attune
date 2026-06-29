/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Standard API response wrapper
 */
export type ApiResponse_AuthSettingsResponse = {
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
};
