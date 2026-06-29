/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Request body for LDAP login.
 */
export type LdapLoginRequest = {
  /**
   * User login name (uid, sAMAccountName, etc.)
   */
  login: string;
  /**
   * User password
   */
  password: string;
};
