/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { AuthErrorResponse } from "./AuthErrorResponse";
import type { ErrorResponse } from "./ErrorResponse";
/**
 * A cache request can be rejected either by the authentication extractor or
 * by cache RBAC after authentication succeeds.
 */
export type CacheForbiddenResponse = AuthErrorResponse | ErrorResponse;
