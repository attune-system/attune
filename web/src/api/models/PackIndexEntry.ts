/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { InstallSource } from "./InstallSource";
import type { PackContents } from "./PackContents";
import type { PackDependencies } from "./PackDependencies";
import type { PackMeta } from "./PackMeta";
/**
 * Pack entry in a registry index
 */
export type PackIndexEntry = {
  /**
   * Pack author/maintainer name
   */
  author: string;
  /**
   * Pack components summary
   */
  contents: PackContents;
  dependencies?: null | PackDependencies;
  /**
   * Brief pack description
   */
  description: string;
  /**
   * Contact email
   */
  email?: string | null;
  /**
   * Pack homepage URL
   */
  homepage?: string | null;
  /**
   * Available installation sources
   */
  install_sources: Array<InstallSource>;
  /**
   * Searchable keywords/tags
   */
  keywords: Array<string>;
  /**
   * Human-readable pack name
   */
  label: string;
  /**
   * SPDX license identifier
   */
  license: string;
  meta?: null | PackMeta;
  /**
   * Unique pack identifier (matches pack.yaml ref)
   */
  ref: string;
  /**
   * Source repository URL
   */
  repository?: string | null;
  /**
   * Required runtimes (python3, nodejs, shell)
   */
  runtime_deps: Array<string>;
  /**
   * Brief use-case summary for browsing/install decisions
   */
  use_case?: string | null;
  /**
   * Semantic version (latest available)
   */
  version: string;
};
