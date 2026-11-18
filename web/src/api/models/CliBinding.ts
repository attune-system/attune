/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { Encoding } from "./Encoding";
import type { MissingPolicy } from "./MissingPolicy";
import type { OptionStyle } from "./OptionStyle";
/**
 * Action-input annotation; values and their paths are never duplicated here.
 */
export type CliBinding =
  | {
      encoding?: Encoding;
      expand?: boolean;
      kind: "positional";
      order: number;
    }
  | {
      encoding?: Encoding;
      expand?: boolean;
      kind: "option";
      missing?: MissingPolicy;
      name: string;
      order: number;
      style?: OptionStyle;
    }
  | {
      false_name?: string | null;
      kind: "flag";
      missing?: MissingPolicy;
      name: string;
      order: number;
    };
