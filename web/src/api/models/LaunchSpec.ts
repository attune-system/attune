/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ClasspathEntry } from "./ClasspathEntry";
import type { IntrinsicHandler } from "./IntrinsicHandler";
import type { LiteralArgument } from "./LiteralArgument";
export type LaunchSpec =
  | {
      arguments?: Array<LiteralArgument>;
      program: string;
      type: "command";
    }
  | {
      path: string;
      type: "file";
    }
  | {
      arguments?: Array<LiteralArgument>;
      artifact: string;
      type: "native";
    }
  | {
      artifact: string;
      jvm_args?: Array<string>;
      type: "java_jar";
    }
  | {
      classpath: Array<ClasspathEntry>;
      jvm_args?: Array<string>;
      main_class: string;
      type: "java_class";
    }
  | {
      handler: IntrinsicHandler;
      type: "intrinsic";
    };
