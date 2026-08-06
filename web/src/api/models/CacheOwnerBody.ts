/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { OwnerType } from "./OwnerType";
/**
 * Owner selector accepted in cache request bodies.
 *
 * `owner_ref` is the pack/action/sensor reference; it is omitted for the
 * `system` scope and resolved to the authenticated identity for `identity`.
 */
export type CacheOwnerBody = {
  owner_ref?: string | null;
  owner_type: OwnerType;
};
