/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * A single cache record. Extra descriptive fields beyond `external_id`/`value`
 * are ignored by minimal clients.
 */
export type CacheEntryResponse = {
  external_id: string;
  size_bytes: number;
  source_checksum: string | null;
  source_updated_at: string | null;
  value: any;
};
