/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { PackDiagnosticSeverity } from "./PackDiagnosticSeverity";
export type PackDiagnostic = {
  code: string;
  column?: number | null;
  component_kind?: string | null;
  component_ref?: string | null;
  hint?: string | null;
  line?: number | null;
  message: string;
  metadata_path?: string | null;
  path?: string | null;
  reference_chain?: Array<string>;
  severity: PackDiagnosticSeverity;
};
