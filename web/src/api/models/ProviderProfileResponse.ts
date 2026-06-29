/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Sanitized user information supplied by an external identity provider.
 */
export type ProviderProfileResponse = {
  /**
   * Provider-issued display name.
   */
  display_name?: string | null;
  /**
   * LDAP distinguished name, when available.
   */
  distinguished_name?: string | null;
  /**
   * Provider-issued email address.
   */
  email?: string | null;
  /**
   * Whether the provider reported the email address as verified.
   */
  email_verified?: boolean | null;
  /**
   * Provider groups associated with this identity.
   */
  groups: Array<string>;
  /**
   * OIDC issuer URL, when available.
   */
  issuer?: string | null;
  /**
   * Provider-issued login or preferred username.
   */
  login?: string | null;
  /**
   * Provider backing this identity.
   */
  provider: string;
  /**
   * OIDC subject identifier, when available.
   */
  subject?: string | null;
};
