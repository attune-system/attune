import { useEffect, useMemo, useRef, useState, type MouseEvent as ReactMouseEvent } from "react";
import { Link, Navigate, useNavigate, useParams, useSearchParams } from "react-router-dom";
import yaml from "js-yaml";
import {
  AlertTriangle,
  ArrowLeft,
  Eye,
  Plus,
  RefreshCw,
  Save,
  Settings,
  Trash2,
  X,
} from "lucide-react";
import AutocompleteInput from "@/components/common/AutocompleteInput";
import ErrorDisplay from "@/components/common/ErrorDisplay";
import { DashboardChartRenderer } from "@/components/dashboard/charts/DashboardChartRenderer";
import { DashboardPreviewGrid } from "@/components/dashboard/DashboardPreviewGrid";
import { DashboardPreviewStatusList } from "@/components/dashboard/DashboardPreviewStatusList";
import { DashboardYamlPanel } from "@/components/dashboard/DashboardYamlPanel";
import { useAuth } from "@/contexts/AuthContext";
import {
  useCreateDashboard,
  useDashboardMetadata,
  useDashboardSourceCatalog,
  useDeleteDashboard,
  usePreviewDashboard,
  useUpdateDashboard,
} from "@/hooks/useDashboards";
import {
  getDashboardClientErrorInfo,
} from "@/lib/dashboard-client";
import { DASHBOARD_SOURCE_CATALOG_FALLBACK } from "@/lib/dashboard-source-catalog";
import { hasPermission } from "@/lib/permissions";
import type {
  DashboardAuthoringDocument,
  DashboardBreakpoint,
  DashboardDataRequest,
  DashboardFilterValue,
  DashboardGridRect,
  DashboardSourceContract,
  DashboardSourceResult,
  DashboardSpecRecord,
} from "@/types/dashboard";
import {
  cloneDashboardDocument,
  createEmptyDashboardDocument,
  dashboardDocumentToCreateRequest,
  dashboardDocumentToSpec,
  dashboardDocumentToUpdateRequest,
  dashboardDocumentToYamlObject,
  dashboardMetadataToDocument,
} from "@/types/dashboard";

type VisualizationFieldKey =
  | "value_field"
  | "x_field"
  | "y_field"
  | "series_field";

interface VisualizationFieldConfig {
  key: VisualizationFieldKey;
  label: string;
}

const DEFAULT_VISUALIZATION_FIELDS: VisualizationFieldConfig[] = [
  { key: "value_field", label: "Value field" },
  { key: "x_field", label: "X field" },
  { key: "y_field", label: "Y field" },
  { key: "series_field", label: "Series field" },
];

const VISUALIZATION_FIELDS: Record<string, VisualizationFieldConfig[]> = {
  table: [],
  stat: [{ key: "value_field", label: "Value field" }],
  kpi: [{ key: "value_field", label: "Value field" }],
  gauge: [{ key: "value_field", label: "Value field" }],
  timeseries: [
    { key: "x_field", label: "X field" },
    { key: "y_field", label: "Y field" },
    { key: "series_field", label: "Series field" },
  ],
  stacked_timeseries: [
    { key: "x_field", label: "X field" },
    { key: "y_field", label: "Y field" },
    { key: "series_field", label: "Series field" },
  ],
  bar: [
    { key: "x_field", label: "X field" },
    { key: "y_field", label: "Y field" },
    { key: "series_field", label: "Series field" },
  ],
  heatmap: [
    { key: "x_field", label: "X field" },
    { key: "y_field", label: "Y field" },
    { key: "value_field", label: "Value field" },
  ],
  histogram: [{ key: "value_field", label: "Value field" }],
  funnel: [
    { key: "x_field", label: "Stage field" },
    { key: "y_field", label: "Value field" },
  ],
  treemap: [
    { key: "x_field", label: "Label field" },
    { key: "y_field", label: "Value field" },
  ],
  status_matrix: [
    { key: "x_field", label: "X field" },
    { key: "y_field", label: "Y field" },
    { key: "value_field", label: "Status/value field" },
  ],
};

function visualizationFieldConfigFor(type: string): VisualizationFieldConfig[] {
  return VISUALIZATION_FIELDS[type] ?? DEFAULT_VISUALIZATION_FIELDS;
}

function sourceDataFields(data: unknown): string[] {
  if (Array.isArray(data) && data.length > 0) {
    const keys = new Set<string>();
    for (const row of data.slice(0, 25)) {
      if (row !== null && typeof row === "object" && !Array.isArray(row)) {
        Object.keys(row as Record<string, unknown>).forEach((key) => keys.add(key));
      }
    }
    return Array.from(keys);
  }
  if (data !== null && typeof data === "object") {
    return Object.keys(data as Record<string, unknown>);
  }
  return [];
}

function mergeFieldOptions(...groups: Array<string[] | undefined>): string[] {
  const merged: string[] = [];
  for (const group of groups) {
    for (const field of group ?? []) {
      if (field && !merged.includes(field)) {
        merged.push(field);
      }
    }
  }
  return merged;
}

function clampNumber(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function safeNumber(value: unknown, fallback: number): number {
  if (typeof value === "number" && Number.isFinite(value)) {
    return value;
  }
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : fallback;
}

type GaugeMode = "high_is_bad" | "low_is_bad" | "target_range";
type CardLayoutInteractionMode = "move" | "resize_right" | "resize_left";

interface CardLayoutInteraction {
  cardId: string;
  breakpoint: string;
  mode: CardLayoutInteractionMode;
  startClientX: number;
  startClientY: number;
  origin: DashboardGridRect;
  columns: number;
  cellWidth: number;
  rowHeight: number;
  gap: number;
}

function normalizeRectForColumns(
  rect: DashboardGridRect,
  fromColumns: number,
  toColumns: number,
): DashboardGridRect {
  const safeFrom = Math.max(1, fromColumns);
  const safeTo = Math.max(1, toColumns);
  const nextW = Math.max(1, Math.round((rect.w / safeFrom) * safeTo));
  const unclampedX = Math.max(0, Math.round((rect.x / safeFrom) * safeTo));
  const nextX = Math.min(Math.max(0, safeTo - nextW), unclampedX);
  return {
    x: nextX,
    y: Math.max(0, rect.y),
    w: Math.min(safeTo, nextW),
    h: Math.max(1, rect.h),
  };
}

function resolveRectForBreakpoint(
  position: Record<string, DashboardGridRect>,
  breakpoint: string,
  breakpoints: Record<string, DashboardBreakpoint>,
  defaultColumns: number,
): DashboardGridRect {
  if (position[breakpoint]) {
    return position[breakpoint];
  }
  const preferredFallbackKey = position.lg ? "lg" : Object.keys(position)[0];
  const fallbackRect = preferredFallbackKey
    ? position[preferredFallbackKey]
    : ({ x: 0, y: 0, w: 1, h: 1 } as DashboardGridRect);
  const fromColumns =
    breakpoints[preferredFallbackKey]?.columns ?? defaultColumns;
  const toColumns = breakpoints[breakpoint]?.columns ?? defaultColumns;
  return normalizeRectForColumns(fallbackRect, fromColumns, toColumns);
}

function gaugeModeForVisualization(visualization: { mode?: string }): GaugeMode {
  if (visualization.mode === "low_is_bad") return "low_is_bad";
  if (visualization.mode === "target_range") return "target_range";
  return "high_is_bad";
}

function gaugeDefaults(min: number, max: number) {
  const span = Math.max(1, max - min);
  return {
    warningStart: min + span * 0.6,
    badStart: min + span * 0.85,
  };
}

function gaugeThresholds(
  visualization: {
    mode?: string;
    bands?: Array<{ from: number; to: number; level: string }>;
  },
  min: number,
  max: number,
): { warningStart: number; badStart: number } {
  const defaults = gaugeDefaults(min, max);
  const higherIsBetter = gaugeModeForVisualization(visualization) === "low_is_bad";
  const warningBand = visualization.bands?.find((band) => band.level === "warning");
  const badBand = visualization.bands?.find((band) => band.level === "bad");
  const goodBand = visualization.bands?.find((band) => band.level === "good");
  const warningStart = clampNumber(
    warningBand?.from ?? defaults.warningStart,
    min,
    max,
  );
  const badStart = clampNumber(
    (higherIsBetter
      ? goodBand?.from ?? warningBand?.to
      : badBand?.from) ?? defaults.badStart,
    warningStart,
    max,
  );
  return { warningStart, badStart };
}

function targetRangeDefaults(min: number, max: number) {
  const span = Math.max(1, max - min);
  return {
    lowerWarningStart: min + span * 0.2,
    goodStart: min + span * 0.35,
    goodEnd: min + span * 0.65,
    upperWarningEnd: min + span * 0.8,
  };
}

function gaugeTargetRangeThresholds(
  visualization: {
    bands?: Array<{ from: number; to: number; level: string }>;
  },
  min: number,
  max: number,
): {
  lowerWarningStart: number;
  goodStart: number;
  goodEnd: number;
  upperWarningEnd: number;
} {
  const defaults = targetRangeDefaults(min, max);
  const warningBands = (visualization.bands ?? []).filter((band) => band.level === "warning");
  const lowerWarningBand = warningBands[0];
  const upperWarningBand = warningBands[1];
  const goodBand = visualization.bands?.find((band) => band.level === "good");

  const lowerWarningStart = clampNumber(
    lowerWarningBand?.from ?? defaults.lowerWarningStart,
    min,
    max,
  );
  const goodStart = clampNumber(
    goodBand?.from ?? lowerWarningBand?.to ?? defaults.goodStart,
    lowerWarningStart,
    max,
  );
  const goodEnd = clampNumber(
    goodBand?.to ?? upperWarningBand?.from ?? defaults.goodEnd,
    goodStart,
    max,
  );
  const upperWarningEnd = clampNumber(
    upperWarningBand?.to ?? defaults.upperWarningEnd,
    goodEnd,
    max,
  );

  return { lowerWarningStart, goodStart, goodEnd, upperWarningEnd };
}

function buildGaugeDirectionalBands(
  min: number,
  max: number,
  warningStart: number,
  badStart: number,
  mode: GaugeMode,
) {
  const normalizedWarning = clampNumber(warningStart, min, max);
  const normalizedBad = clampNumber(badStart, normalizedWarning, max);
  if (mode === "low_is_bad") {
    return [
      { from: min, to: normalizedWarning, level: "bad" },
      { from: normalizedWarning, to: normalizedBad, level: "warning" },
      { from: normalizedBad, to: max, level: "good" },
    ];
  }
  return [
    { from: min, to: normalizedWarning, level: "good" },
    { from: normalizedWarning, to: normalizedBad, level: "warning" },
    { from: normalizedBad, to: max, level: "bad" },
  ];
}

function buildGaugeTargetRangeBands(
  min: number,
  max: number,
  lowerWarningStart: number,
  goodStart: number,
  goodEnd: number,
  upperWarningEnd: number,
) {
  const normalizedLowerWarningStart = clampNumber(lowerWarningStart, min, max);
  const normalizedGoodStart = clampNumber(goodStart, normalizedLowerWarningStart, max);
  const normalizedGoodEnd = clampNumber(goodEnd, normalizedGoodStart, max);
  const normalizedUpperWarningEnd = clampNumber(upperWarningEnd, normalizedGoodEnd, max);
  return [
    { from: min, to: normalizedLowerWarningStart, level: "bad" },
    { from: normalizedLowerWarningStart, to: normalizedGoodStart, level: "warning" },
    { from: normalizedGoodStart, to: normalizedGoodEnd, level: "good" },
    { from: normalizedGoodEnd, to: normalizedUpperWarningEnd, level: "warning" },
    { from: normalizedUpperWarningEnd, to: max, level: "bad" },
  ];
}


function slugify(value: string): string {
  return value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9_.-]+/g, "_")
    .replace(/^_+|_+$/g, "");
}

function refPrefixFromDashboard(document: DashboardAuthoringDocument): string {
  if (document.scope_type === "pack" && document.scope_ref.trim()) {
    return slugify(document.scope_ref) || "core";
  }
  const firstSegment = document.ref.split(".")[0];
  return firstSegment ? slugify(firstSegment) : "core";
}

function extractDashboardLocalRef(ref: string, prefix?: string): string {
  if (!ref) return "";
  if (prefix && ref.startsWith(`${prefix}.`)) {
    return ref.slice(prefix.length + 1);
  }
  const dotIndex = ref.indexOf(".");
  if (dotIndex >= 0) {
    return ref.slice(dotIndex + 1);
  }
  return ref;
}

function composeDashboardRef(prefix: string, localRef: string): string {
  const normalizedPrefix = slugify(prefix) || "core";
  const normalizedLocalRef = slugify(localRef).replaceAll(".", "_");
  if (!normalizedLocalRef) return "";
  return `${normalizedPrefix}.${normalizedLocalRef}`;
}

function generateUniqueId(prefix: string, existing: string[]): string {
  const base = slugify(prefix) || prefix;
  if (!existing.includes(base)) {
    return base;
  }
  let index = 2;
  while (existing.includes(`${base}_${index}`)) {
    index += 1;
  }
  return `${base}_${index}`;
}

function serializeDocument(document: DashboardAuthoringDocument | null): string {
  if (!document) return "";
  return JSON.stringify({
    metadata: dashboardDocumentToCreateRequest(document),
    revision: document.revision,
  });
}

function parseTagInput(value: string): string[] {
  return value
    .split(",")
    .map((tag) => tag.trim())
    .filter(Boolean)
    .sort((left, right) => left.localeCompare(right));
}

function formatFilterValue(value: DashboardFilterValue | undefined): string {
  if (Array.isArray(value)) {
    return value.join(", ");
  }
  if (value === undefined || value === null) {
    return "";
  }
  return String(value);
}

function parseFilterValueInput(
  filterType: string,
  value: string,
  preferArray = false,
): DashboardFilterValue | undefined {
  const trimmed = value.trim();
  if (!trimmed) {
    return undefined;
  }
  if (preferArray || trimmed.includes(",")) {
    const items = trimmed
      .split(",")
      .map((item) => item.trim())
      .filter(Boolean);
    if (filterType === "number") {
      return items
        .map((item) => Number(item))
        .filter((item) => Number.isFinite(item));
    }
    return items;
  }
  if (filterType === "number") {
    const numeric = Number(trimmed);
    return Number.isFinite(numeric) ? numeric : trimmed;
  }
  if (filterType === "boolean") {
    return trimmed.toLowerCase() === "true";
  }
  return trimmed;
}

function formatOptions(options: DashboardFilterValue[] | undefined): string {
  if (!options?.length) {
    return "";
  }
  return options.map((option) => formatFilterValue(option)).join(", ");
}

function parseOptionsInput(
  filterType: string,
  value: string,
): DashboardFilterValue[] | undefined {
  const trimmed = value.trim();
  if (!trimmed) {
    return undefined;
  }
  return trimmed
    .split(",")
    .map((item) => parseFilterValueInput(filterType, item))
    .filter((item): item is DashboardFilterValue => item !== undefined);
}

function dashboardDefaultsRequest(spec: DashboardSpecRecord): DashboardDataRequest {
  const filters = Object.fromEntries(
    (spec.filters ?? [])
      .filter((filter) => filter.default !== undefined)
      .map((filter) => [filter.id, filter.default as DashboardFilterValue]),
  );

  return {
    filters,
    time_window: spec.defaults?.time_window,
    timezone: spec.defaults?.timezone ?? "UTC",
    card_ids: spec.cards.map((card) => card.id),
    include_meta: true,
  };
}


function referencesMissingFilters(params: Record<string, unknown>, filterIds: Set<string>): string[] {
  const missing = new Set<string>();
  const visit = (value: unknown) => {
    if (typeof value === "string") {
      const matches = value.matchAll(/\{\{\s*filters\.([a-zA-Z0-9_.-]+)\s*\}\}/g);
      for (const match of matches) {
        const filterId = match[1];
        if (!filterIds.has(filterId)) {
          missing.add(filterId);
        }
      }
      return;
    }
    if (Array.isArray(value)) {
      value.forEach(visit);
      return;
    }
    if (value && typeof value === "object") {
      Object.values(value).forEach(visit);
    }
  };
  visit(params);
  return [...missing].sort((left, right) => left.localeCompare(right));
}

function validateDocument(
  document: DashboardAuthoringDocument,
  contractsByType: Map<string, DashboardSourceContract>,
): { errors: string[]; warnings: string[] } {
  const errors: string[] = [];
  const warnings: string[] = [];

  if (!document.ref.trim()) {
    errors.push("Dashboard ref is required.");
  }
  if (!/^[a-zA-Z0-9_.-]+$/.test(document.ref.trim())) {
    errors.push("Dashboard ref may only contain letters, numbers, dots, underscores, and hyphens.");
  }
  if (!document.label.trim()) {
    errors.push("Dashboard label is required.");
  }
  if (!document.scope_ref.trim()) {
    errors.push("Dashboard scope ref is required.");
  }

  const spec = dashboardDocumentToSpec(document);
  const breakpointEntries = Object.entries(spec.layout.breakpoints);
  if (!breakpointEntries.some(([key]) => key === "lg")) {
    errors.push("Layout must include an lg breakpoint.");
  }
  if (!breakpointEntries.some(([key]) => key === "sm")) {
    errors.push("Layout should include an sm breakpoint for MVP editing.");
  }

  const filterIds = new Set((spec.filters ?? []).map((filter) => filter.id));
  if (filterIds.size !== (spec.filters ?? []).length) {
    errors.push("Filter ids must be unique.");
  }

  const sourceIds = new Set(Object.keys(spec.data_sources));
  const cardIds = new Set<string>();

  for (const [sourceId, source] of Object.entries(spec.data_sources)) {
    if (!sourceId.trim()) {
      errors.push("Data source ids cannot be empty.");
      continue;
    }
    const contract = contractsByType.get(source.type);
    if (!contract) {
      warnings.push(`Source ${sourceId} uses unknown type ${source.type}.`);
    } else if (contract.availability !== "available_now") {
      warnings.push(
        `Source ${sourceId} is ${contract.availability.replaceAll("_", " ")}. ${contract.notes ?? ""}`.trim(),
      );
    }

    const missingFilters = referencesMissingFilters(source.params ?? {}, filterIds);
    if (missingFilters.length > 0) {
      errors.push(
        `Source ${sourceId} references missing filters: ${missingFilters.join(", ")}.`,
      );
    }
  }

  for (const card of spec.cards) {
    if (cardIds.has(card.id)) {
      errors.push(`Card id ${card.id} must be unique.`);
    }
    cardIds.add(card.id);
    if (!card.title.trim()) {
      errors.push(`Card ${card.id || "(unnamed)"} requires a title.`);
    }
    if (!sourceIds.has(card.source)) {
      errors.push(`Card ${card.id || "(unnamed)"} references unknown source ${card.source}.`);
    }
    for (const [breakpointKey, breakpoint] of breakpointEntries) {
      const rect = card.position[breakpointKey];
      if (!rect) {
        errors.push(`Card ${card.id} is missing a ${breakpointKey} position.`);
        continue;
      }
      if (rect.w <= 0 || rect.h <= 0) {
        errors.push(`Card ${card.id} must have positive width and height at ${breakpointKey}.`);
      }
      if (rect.x < 0 || rect.y < 0) {
        errors.push(`Card ${card.id} cannot use negative coordinates at ${breakpointKey}.`);
      }
      if (rect.w > breakpoint.columns || rect.x + rect.w > breakpoint.columns) {
        errors.push(`Card ${card.id} exceeds ${breakpointKey} columns.`);
      }
    }
  }

  return { errors, warnings };
}

function availabilityBadgeClass(availability: string): string {
  switch (availability) {
    case "available_now":
      return "bg-green-100 text-green-700";
    case "partial":
      return "bg-amber-100 text-amber-800";
    case "planned":
      return "bg-blue-100 text-blue-700";
    default:
      return "bg-gray-100 text-gray-700";
  }
}

function formatSourceTypeLabel(sourceType: string): string {
  return sourceType
    .split("_")
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
}

function sourceTypeOptionLabel(
  sourceType: string,
  contract: DashboardSourceContract | undefined,
): string {
  const base = formatSourceTypeLabel(sourceType);
  if (!contract) {
    return `${base} (${sourceType})`;
  }
  const shape = contract.response_shape;
  if (contract.availability === "available_now") {
    return `${base} — ${shape} (${sourceType})`;
  }
  const availability = contract.availability.replaceAll("_", " ");
  return `${base} — ${shape}, ${availability} (${sourceType})`;
}

function parseSourceParamValue(input: string, kind: DashboardSourceContract["params"][number]["input"]): unknown {
  if (kind === "boolean") {
    return input === "true";
  }
  if (kind === "number") {
    const numeric = Number(input);
    return Number.isFinite(numeric) ? numeric : input;
  }
  return input;
}

function formatSourceParamValue(value: unknown): string {
  if (typeof value === "boolean") {
    return value ? "true" : "false";
  }
  if (value === undefined || value === null) {
    return "";
  }
  if (typeof value === "object") {
    return JSON.stringify(value);
  }
  return String(value);
}

function sampleMeta(ordering: string[]): DashboardSourceResult["meta"] {
  return {
    authorization_mode: "operator_global",
    freshness_mode: "raw_only",
    aggregate_watermark: null,
    cache_hit: false,
    bucket_size: null,
    truncated: false,
    unit_hints: {},
    ordering,
    authorized_refs: null,
  };
}

function sampleSourceForVisualization(type: string): DashboardSourceResult {
  switch (type) {
    case "timeseries":
    case "stacked_timeseries":
      return {
        source_id: "__sample__",
        source_type: "__sample__",
        status: "ok",
        data: [
          { bucket_start: "08:00", series: "ok", count: 12 },
          { bucket_start: "09:00", series: "ok", count: 18 },
          { bucket_start: "10:00", series: "ok", count: 9 },
        ],
        meta: sampleMeta(["bucket_start", "series", "count"]),
        error: null,
      };
    case "bar":
      return {
        source_id: "__sample__",
        source_type: "__sample__",
        status: "ok",
        data: [
          { category: "A", count: 14 },
          { category: "B", count: 9 },
          { category: "C", count: 19 },
        ],
        meta: sampleMeta(["category", "count"]),
        error: null,
      };
    case "heatmap":
      return {
        source_id: "__sample__",
        source_type: "__sample__",
        status: "ok",
        data: [
          { x: "Mon", y: "Queue A", value: 4 },
          { x: "Tue", y: "Queue A", value: 8 },
          { x: "Mon", y: "Queue B", value: 6 },
          { x: "Tue", y: "Queue B", value: 2 },
        ],
        meta: sampleMeta(["x", "y", "value"]),
        error: null,
      };
    case "histogram":
      return {
        source_id: "__sample__",
        source_type: "__sample__",
        status: "ok",
        data: [{ value: 3 }, { value: 5 }, { value: 7 }, { value: 7 }, { value: 12 }, { value: 18 }],
        meta: sampleMeta(["value"]),
        error: null,
      };
    case "funnel":
      return {
        source_id: "__sample__",
        source_type: "__sample__",
        status: "ok",
        data: [
          { stage: "Received", value: 120 },
          { stage: "Validated", value: 92 },
          { stage: "Completed", value: 70 },
        ],
        meta: sampleMeta(["stage", "value"]),
        error: null,
      };
    case "treemap":
      return {
        source_id: "__sample__",
        source_type: "__sample__",
        status: "ok",
        data: [
          { name: "Team A", value: 44 },
          { name: "Team B", value: 26 },
          { name: "Team C", value: 30 },
        ],
        meta: sampleMeta(["name", "value"]),
        error: null,
      };
    case "status_matrix":
      return {
        source_id: "__sample__",
        source_type: "__sample__",
        status: "ok",
        data: [
          { x: "us-east", y: "worker-1", status: "good" },
          { x: "us-east", y: "worker-2", status: "warning" },
          { x: "us-west", y: "worker-3", status: "bad" },
        ],
        meta: sampleMeta(["x", "y", "status"]),
        error: null,
      };
    case "gauge":
      return {
        source_id: "__sample__",
        source_type: "__sample__",
        status: "ok",
        data: [{ count: 64 }],
        meta: sampleMeta(["count"]),
        error: null,
      };
    case "table":
      return {
        source_id: "__sample__",
        source_type: "__sample__",
        status: "ok",
        data: [{ service: "api", status: "ok", count: 14 }],
        meta: sampleMeta(["service", "status", "count"]),
        error: null,
      };
    case "kpi":
    case "stat":
    default:
      return {
        source_id: "__sample__",
        source_type: "__sample__",
        status: "ok",
        data: [{ count: 42 }],
        meta: sampleMeta(["count"]),
        error: null,
      };
  }
}

export default function DashboardEditorPage() {
  const { user } = useAuth();
  const navigate = useNavigate();
  const { ref: editingRef } = useParams<{ ref?: string }>();
  const [searchParams] = useSearchParams();
  const cloneRef = searchParams.get("clone") || "";
  const sourceRef = editingRef || cloneRef;
  const isEditing = Boolean(editingRef);
  const isCloneMode = !editingRef && Boolean(cloneRef);

  const canCreate = hasPermission(user, "dashboards", "create");
  const canUpdate = hasPermission(user, "dashboards", "update");
  const canDelete = hasPermission(user, "dashboards", "delete");

  const {
    data: loadedMetadata,
    isLoading: metadataLoading,
    error: metadataError,
    refetch: refetchMetadata,
  } = useDashboardMetadata(sourceRef);
  const { data: sourceCatalog, isLoading: sourceCatalogLoading } =
    useDashboardSourceCatalog();

  const createMutation = useCreateDashboard();
  const updateMutation = useUpdateDashboard();
  const deleteMutation = useDeleteDashboard();
  const previewMutation = usePreviewDashboard();

  const catalogContracts = useMemo(
    () =>
      sourceCatalog?.contracts && sourceCatalog.contracts.length > 0
        ? sourceCatalog.contracts
        : DASHBOARD_SOURCE_CATALOG_FALLBACK.contracts,
    [sourceCatalog?.contracts],
  );
  const sourceContractsByType = useMemo(
    () =>
      new Map<string, DashboardSourceContract>(
        catalogContracts.map((contract) => [contract.source_type, contract]),
      ),
    [catalogContracts],
  );

  const [draft, setDraft] = useState<DashboardAuthoringDocument>(() =>
    createEmptyDashboardDocument(),
  );
  const [baseline, setBaseline] = useState<DashboardAuthoringDocument | null>(() =>
    createEmptyDashboardDocument(),
  );
  const [initKey, setInitKey] = useState("new");
  const [pageError, setPageError] = useState<string | null>(null);
  const [saveMessage, setSaveMessage] = useState<string | null>(null);
  const [conflictMessage, setConflictMessage] = useState<string | null>(null);
  const [newBreakpointId, setNewBreakpointId] = useState("");
  const [previewBreakpoint, setPreviewBreakpoint] = useState("lg");
  const [layoutBreakpoint, setLayoutBreakpoint] = useState("lg");
  const [activeSourceConfigId, setActiveSourceConfigId] = useState<string | null>(null);
  const [activeCardConfigId, setActiveCardConfigId] = useState<string | null>(null);
  const [editorView, setEditorView] = useState<"config" | "preview" | "yaml">(
    "config",
  );
  const [layoutCanvasWidth, setLayoutCanvasWidth] = useState(0);
  const [layoutInteraction, setLayoutInteraction] = useState<CardLayoutInteraction | null>(null);
  const layoutCanvasRef = useRef<HTMLDivElement | null>(null);

  const resolvedInitKey = isEditing
    ? `edit:${editingRef}:${loadedMetadata?.revision ?? "pending"}`
    : isCloneMode
      ? `clone:${cloneRef}:${loadedMetadata?.revision ?? "pending"}`
      : "new";

  useEffect(() => {
    if (!isEditing && !isCloneMode && initKey !== "new") {
      const fresh = createEmptyDashboardDocument();
      setDraft(fresh);
      setBaseline(cloneDashboardDocument(fresh));
      setInitKey("new");
      return;
    }

    if (!loadedMetadata || resolvedInitKey === initKey) {
      return;
    }

    const next = dashboardMetadataToDocument(loadedMetadata, {
      cloneFromExisting: isCloneMode,
    });
    setDraft(next);
    setBaseline(cloneDashboardDocument(next));
    setInitKey(resolvedInitKey);
    setSaveMessage(null);
    setPageError(null);
    setConflictMessage(null);
  }, [initKey, isCloneMode, isEditing, loadedMetadata, resolvedInitKey]);

  useEffect(() => {
    const breakpointKeys = Object.keys(draft.spec.layout.breakpoints);
    if (!breakpointKeys.includes(previewBreakpoint)) {
      setPreviewBreakpoint(breakpointKeys[0] || "lg");
    }
    if (!breakpointKeys.includes(layoutBreakpoint)) {
      setLayoutBreakpoint(breakpointKeys[0] || "lg");
    }
  }, [draft.spec.layout.breakpoints, layoutBreakpoint, previewBreakpoint]);

  useEffect(() => {
    if (!activeSourceConfigId) return;
    if (!Object.prototype.hasOwnProperty.call(draft.spec.data_sources, activeSourceConfigId)) {
      setActiveSourceConfigId(null);
    }
  }, [activeSourceConfigId, draft.spec.data_sources]);

  useEffect(() => {
    if (!activeCardConfigId) return;
    if (!draft.spec.cards.some((card) => card.id === activeCardConfigId)) {
      setActiveCardConfigId(null);
    }
  }, [activeCardConfigId, draft.spec.cards]);

  useEffect(() => {
    if (editorView !== "config") return;
    const node = layoutCanvasRef.current;
    if (!node) return;

    const measure = () => {
      const nextWidth = node.getBoundingClientRect().width;
      if (nextWidth > 0) {
        setLayoutCanvasWidth((current) =>
          Math.abs(current - nextWidth) > 0.5 ? nextWidth : current,
        );
      }
    };

    measure();
    const frame = window.requestAnimationFrame(measure);
    const settleTimer = window.setTimeout(measure, 120);
    const observer = new ResizeObserver(() => measure());
    observer.observe(node);
    window.addEventListener("resize", measure);
    return () => {
      window.cancelAnimationFrame(frame);
      window.clearTimeout(settleTimer);
      observer.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, [editorView, draft.spec.cards.length, layoutBreakpoint]);

  const isDirty = useMemo(
    () => serializeDocument(baseline) !== serializeDocument(draft),
    [baseline, draft],
  );

  const validation = useMemo(
    () => validateDocument(draft, sourceContractsByType),
    [draft, sourceContractsByType],
  );

  const yamlText = useMemo(
    () =>
      yaml.dump(dashboardDocumentToYamlObject(draft), {
        noRefs: true,
        lineWidth: 120,
        sortKeys: false,
      }),
    [draft],
  );

  const previewSpec = useMemo(() => dashboardDocumentToSpec(draft), [draft]);
  const publishedSpec = loadedMetadata?.spec;
  const previewSourceMap = useMemo(() => {
    const map = new Map<string, DashboardSourceResult>();
    for (const source of previewMutation.data?.data.sources ?? []) {
      map.set(source.source_id, source);
    }
    return map;
  }, [previewMutation.data?.data.sources]);
  const refPrefix = useMemo(() => refPrefixFromDashboard(draft), [draft]);
  const refLocalPart = useMemo(
    () => extractDashboardLocalRef(draft.ref, refPrefix),
    [draft.ref, refPrefix],
  );

  const updateDraft = (updater: (current: DashboardAuthoringDocument) => DashboardAuthoringDocument) => {
    setDraft((current) => updater(cloneDashboardDocument(current)));
    setSaveMessage(null);
    setPageError(null);
    setConflictMessage(null);
  };

  const updateMetadata = <K extends keyof DashboardAuthoringDocument>(
    key: K,
    value: DashboardAuthoringDocument[K],
  ) => {
    updateDraft((current) => {
      current[key] = value;
      if (key === "ref" || key === "label" || key === "description") {
        current.spec[key] = value as never;
      }
      if (key === "tags") {
        current.spec.tags = value as DashboardAuthoringDocument["tags"];
      }
      return current;
    });
  };

  const updateScopeField = (
    key: "scope_type" | "scope_ref",
    value: DashboardAuthoringDocument["scope_type"] | DashboardAuthoringDocument["scope_ref"],
  ) => {
    updateDraft((current) => {
      const previousPrefix = refPrefixFromDashboard(current);
      const currentLocalRef = extractDashboardLocalRef(current.ref, previousPrefix);
      if (key === "scope_ref") {
        const nextScopeRefRaw = String(value ?? "");
        current.scope_ref =
          current.scope_type === "pack"
            ? slugify(nextScopeRefRaw)
            : nextScopeRefRaw;
      } else {
        current.scope_type = value as DashboardAuthoringDocument["scope_type"];
      }

      if (!isEditing && currentLocalRef.trim()) {
        const nextPrefix = refPrefixFromDashboard(current);
        current.ref = composeDashboardRef(nextPrefix, currentLocalRef);
      }
      return current;
    });
  };

  const addBreakpoint = () => {
    const breakpointId = slugify(newBreakpointId);
    if (!breakpointId) return;
    updateDraft((current) => {
      if (current.spec.layout.breakpoints[breakpointId]) {
        return current;
      }
      current.spec.layout.breakpoints[breakpointId] = {
        min_width: 768,
        columns: current.spec.layout.columns,
      };
      current.spec.cards.forEach((card) => {
        card.position[breakpointId] = {
          x: 0,
          y: 0,
          w: Math.min(current.spec.layout.columns, 4),
          h: 4,
        };
      });
      return current;
    });
    setNewBreakpointId("");
  };

  const removeBreakpoint = (breakpointId: string) => {
    if (["lg", "sm"].includes(breakpointId)) {
      return;
    }
    updateDraft((current) => {
      delete current.spec.layout.breakpoints[breakpointId];
      current.spec.cards.forEach((card) => {
        delete card.position[breakpointId];
      });
      return current;
    });
  };

  const addSource = () => {
    updateDraft((current) => {
      const sourceId = generateUniqueId(
        "source",
        Object.keys(current.spec.data_sources),
      );
      current.spec.data_sources[sourceId] = {
        type: sourceCatalog?.contracts[0]?.source_type || "execution_count",
        params: {},
      };
      return current;
    });
  };

  const addCard = () => {
    updateDraft((current) => {
      const cardId = generateUniqueId(
        "card",
        current.spec.cards.map((card) => card.id),
      );
      const firstSource = Object.keys(current.spec.data_sources)[0] || "";
      const position = Object.fromEntries(
        Object.entries(current.spec.layout.breakpoints).map(([breakpointId, breakpoint]) => [
          breakpointId,
          {
            x: 0,
            y: current.spec.cards.length * 4,
            w: Math.min(4, breakpoint.columns),
            h: 4,
          },
        ]),
      ) as Record<string, DashboardGridRect>;
      current.spec.cards.push({
        id: cardId,
        title: "New Card",
        subtitle: "",
        source: firstSource,
        visualization: { type: "table" },
        position,
      });
      return current;
    });
  };

  const addFilter = () => {
    updateDraft((current) => {
      const filterId = generateUniqueId(
        "filter",
        current.spec.filters?.map((filter) => filter.id) ?? [],
      );
      current.spec.filters = current.spec.filters ?? [];
      current.spec.filters.push({
        id: filterId,
        type: "text",
        label: "New Filter",
      });
      return current;
    });
  };

  const activeLayoutColumns = Math.max(
    1,
    safeNumber(
      draft.spec.layout.breakpoints[layoutBreakpoint]?.columns,
      safeNumber(draft.spec.layout.columns, 12),
    ),
  );
  const activeLayoutGap = Math.max(0, safeNumber(draft.spec.layout.gap, 0));
  const activeLayoutRowHeight = Math.max(1, safeNumber(draft.spec.layout.row_height, 40));
  const effectiveLayoutCanvasWidth =
    layoutCanvasWidth > 0
      ? layoutCanvasWidth
      : layoutCanvasRef.current?.getBoundingClientRect().width ?? 0;
  const usableLayoutCanvasWidth = Math.max(
    0,
    effectiveLayoutCanvasWidth -
      activeLayoutGap * Math.max(0, activeLayoutColumns - 1),
  );
  const layoutUnitWidth =
    activeLayoutColumns > 0 ? usableLayoutCanvasWidth / activeLayoutColumns : 0;
  const layoutRows = useMemo(() => {
    const maxBottom = draft.spec.cards.reduce((currentMax, card) => {
      const rect = resolveRectForBreakpoint(
        card.position,
        layoutBreakpoint,
        draft.spec.layout.breakpoints,
        draft.spec.layout.columns,
      );
      return Math.max(currentMax, rect.y + rect.h);
    }, 0);
    return Math.max(6, maxBottom + 2);
  }, [draft.spec.cards, layoutBreakpoint, draft.spec.layout.breakpoints, draft.spec.layout.columns]);

  const updateCardRect = (
    cardId: string,
    breakpointId: string,
    updater: (rect: DashboardGridRect) => DashboardGridRect,
  ) => {
    updateDraft((current) => {
      const card = current.spec.cards.find((entry) => entry.id === cardId);
      if (!card) return current;
      const existing = resolveRectForBreakpoint(
        card.position,
        breakpointId,
        current.spec.layout.breakpoints,
        current.spec.layout.columns,
      );
      card.position[breakpointId] = updater(existing);
      return current;
    });
  };

  const startCardInteraction = (
    event: ReactMouseEvent,
    cardId: string,
    mode: CardLayoutInteractionMode,
  ) => {
    const card = draft.spec.cards.find((entry) => entry.id === cardId);
    if (!card || layoutUnitWidth <= 0) return;
    const rect = resolveRectForBreakpoint(
      card.position,
      layoutBreakpoint,
      draft.spec.layout.breakpoints,
      draft.spec.layout.columns,
    );
    event.preventDefault();
    event.stopPropagation();
    setLayoutInteraction({
      cardId,
      breakpoint: layoutBreakpoint,
      mode,
      startClientX: event.clientX,
      startClientY: event.clientY,
      origin: { ...rect },
      columns: activeLayoutColumns,
      cellWidth: layoutUnitWidth,
      rowHeight: activeLayoutRowHeight,
      gap: activeLayoutGap,
    });
  };

  useEffect(() => {
    if (!layoutInteraction) return;
    const onMouseMove = (event: MouseEvent) => {
      const stepX = layoutInteraction.cellWidth + layoutInteraction.gap;
      const stepY = layoutInteraction.rowHeight + layoutInteraction.gap;
      const dx = stepX > 0 ? Math.round((event.clientX - layoutInteraction.startClientX) / stepX) : 0;
      const dy = stepY > 0 ? Math.round((event.clientY - layoutInteraction.startClientY) / stepY) : 0;

      updateCardRect(layoutInteraction.cardId, layoutInteraction.breakpoint, (rect) => {
        const base = layoutInteraction.origin;
        if (layoutInteraction.mode === "move") {
          const nextX = Math.max(
            0,
            Math.min(layoutInteraction.columns - base.w, base.x + dx),
          );
          const nextY = Math.max(0, base.y + dy);
          return { ...rect, x: nextX, y: nextY };
        }

        const nextH = Math.max(1, base.h + dy);
        if (layoutInteraction.mode === "resize_left") {
          const desiredX = Math.max(0, Math.min(base.x + base.w - 1, base.x + dx));
          const nextW = Math.max(1, Math.min(layoutInteraction.columns - desiredX, base.w + (base.x - desiredX)));
          return { ...rect, x: desiredX, w: nextW, h: nextH };
        }

        const nextW = Math.max(
          1,
          Math.min(layoutInteraction.columns - base.x, base.w + dx),
        );
        return { ...rect, w: nextW, h: nextH };
      });
    };

    const onMouseUp = () => setLayoutInteraction(null);
    window.addEventListener("mousemove", onMouseMove);
    window.addEventListener("mouseup", onMouseUp);
    return () => {
      window.removeEventListener("mousemove", onMouseMove);
      window.removeEventListener("mouseup", onMouseUp);
    };
  }, [layoutInteraction]);

  const saveDashboard = async () => {
    setPageError(null);
    setSaveMessage(null);
    setConflictMessage(null);

    if (validation.errors.length > 0) {
      setPageError("Resolve validation errors before saving.");
      return;
    }

    try {
      if (isEditing) {
        const saved = await updateMutation.mutateAsync({
          ref: editingRef!,
          request: dashboardDocumentToUpdateRequest(draft),
        });
        const next = dashboardMetadataToDocument(saved);
        setDraft(next);
        setBaseline(cloneDashboardDocument(next));
        setSaveMessage("Dashboard updated.");
      } else {
        const saved = await createMutation.mutateAsync(
          dashboardDocumentToCreateRequest(draft),
        );
        navigate(`/dashboards/${encodeURIComponent(saved.ref)}/edit`, {
          replace: true,
        });
      }
    } catch (error) {
      const info = getDashboardClientErrorInfo(error, "Failed to save dashboard");
      if (info.conflict) {
        setConflictMessage(
          info.message ||
            "The dashboard changed on the server. Reload the latest revision before saving again.",
        );
        return;
      }
      setPageError(info.message);
    }
  };

  const deleteDashboard = async () => {
    if (!editingRef) {
      return;
    }
    if (!window.confirm(`Delete dashboard ${editingRef}?`)) {
      return;
    }
    setPageError(null);
    try {
      await deleteMutation.mutateAsync(editingRef);
      navigate("/", { replace: true });
    } catch (error) {
      const info = getDashboardClientErrorInfo(error, "Failed to delete dashboard");
      setPageError(info.message);
    }
  };

  const runPreview = async () => {
    setPageError(null);
    try {
      await previewMutation.mutateAsync({
        dashboard: dashboardDocumentToCreateRequest(draft),
        request: dashboardDefaultsRequest(previewSpec),
        fallback_dashboard_ref: sourceRef || undefined,
        fallback_request: publishedSpec
          ? dashboardDefaultsRequest(publishedSpec)
          : undefined,
      });
    } catch (error) {
      const info = getDashboardClientErrorInfo(error, "Failed to preview dashboard");
      setPageError(info.message);
    }
  };

  if ((isEditing || isCloneMode) && metadataLoading) {
    return <div className="p-6 text-sm text-gray-600">Loading dashboard draft…</div>;
  }

  if ((isEditing || isCloneMode) && metadataError) {
    return (
      <div className="p-6">
        <ErrorDisplay
          error={metadataError}
          title="Unable to load dashboard"
          showRetry
          onRetry={() => {
            void refetchMetadata();
          }}
        />
      </div>
    );
  }

  const packManagedEditBlocked =
    isEditing && loadedMetadata?.is_adhoc === false;

  if (packManagedEditBlocked) {
    return <Navigate to={`/?ref=${encodeURIComponent(loadedMetadata?.ref || sourceRef)}`} replace />;
  }

  const title = isEditing
    ? `Edit ${draft.ref}`
    : isCloneMode
      ? "Clone Dashboard"
      : "New Dashboard";

  return (
    <div className="p-6 space-y-6">
      <header className="flex flex-wrap items-start justify-between gap-4">
        <div>
          <Link
            to={draft.ref ? `/?ref=${encodeURIComponent(draft.ref)}` : "/"}
            className="inline-flex items-center gap-2 text-sm text-blue-700 hover:text-blue-900"
          >
            <ArrowLeft className="h-4 w-4" />
            Back to dashboards
          </Link>
          <h1 className="mt-2 text-2xl font-bold text-gray-900">{title}</h1>
          <p className="mt-1 text-sm text-gray-600">
            Functional MVP editor for dashboard metadata, layout, cards, sources,
            defaults, and YAML round-tripping.
          </p>
          <div className="mt-2 flex flex-wrap items-center gap-2 text-xs text-gray-500">
            <span>revision: {draft.revision ?? "new"}</span>
            {isDirty && <span className="rounded bg-amber-100 px-2 py-0.5 text-amber-800">unsaved changes</span>}
            {sourceCatalog?.source === "fallback" && (
              <span className="rounded bg-blue-100 px-2 py-0.5 text-blue-700">
                source catalog fallback
              </span>
            )}
          </div>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          {draft.ref && (
            <Link
              to={`/dashboards/new?clone=${encodeURIComponent(draft.ref)}`}
              className="inline-flex items-center gap-2 rounded border border-gray-300 bg-white px-3 py-1.5 text-sm text-gray-700 hover:bg-gray-50"
            >
              <RefreshCw className="h-4 w-4" />
              Clone to new draft
            </Link>
          )}
          <button
            type="button"
            onClick={() => void runPreview()}
            className="inline-flex items-center gap-2 rounded border border-gray-300 bg-white px-3 py-1.5 text-sm text-gray-700 hover:bg-gray-50"
            disabled={previewMutation.isPending}
          >
            <Eye className="h-4 w-4" />
            {previewMutation.isPending ? "Previewing…" : "Preview"}
          </button>
          <button
            type="button"
            onClick={() => void saveDashboard()}
            disabled={
              (!isEditing && !canCreate) || (isEditing && !canUpdate) || createMutation.isPending || updateMutation.isPending
            }
            className="inline-flex items-center gap-2 rounded bg-blue-600 px-3 py-1.5 text-sm text-white hover:bg-blue-700 disabled:cursor-not-allowed disabled:bg-blue-300"
          >
            <Save className="h-4 w-4" />
            {createMutation.isPending || updateMutation.isPending ? "Saving…" : "Save"}
          </button>
          {isEditing && canDelete && (
            <button
              type="button"
              onClick={() => void deleteDashboard()}
              className="inline-flex items-center gap-2 rounded border border-red-300 bg-white px-3 py-1.5 text-sm text-red-700 hover:bg-red-50"
              disabled={deleteMutation.isPending}
            >
              <Trash2 className="h-4 w-4" />
              Delete
            </button>
          )}
        </div>
      </header>

      <div className="rounded-lg border border-gray-200 bg-white p-1 inline-flex">
        {[
          { id: "config", label: "Config" },
          { id: "preview", label: "Preview" },
          { id: "yaml", label: "YAML" },
        ].map((option) => (
          <button
            key={option.id}
            type="button"
            onClick={() =>
              setEditorView(option.id as "config" | "preview" | "yaml")
            }
            className={`rounded px-3 py-1.5 text-sm ${
              editorView === option.id
                ? "bg-blue-600 text-white"
                : "text-gray-700 hover:bg-gray-100"
            }`}
          >
            {option.label}
          </button>
        ))}
      </div>

      {validation.errors.length > 0 && (
        <div className="rounded border border-red-200 bg-red-50 px-4 py-3">
          <div className="flex items-start gap-2">
            <AlertTriangle className="mt-0.5 h-4 w-4 text-red-700" />
            <div>
              <p className="text-sm font-medium text-red-900">Validation issues</p>
              <ul className="mt-2 list-disc space-y-1 pl-5 text-sm text-red-800">
                {validation.errors.map((error) => (
                  <li key={error}>{error}</li>
                ))}
              </ul>
            </div>
          </div>
        </div>
      )}

      {validation.warnings.length > 0 && (
        <div className="rounded border border-amber-200 bg-amber-50 px-4 py-3">
          <p className="text-sm font-medium text-amber-900">Authoring warnings</p>
          <ul className="mt-2 list-disc space-y-1 pl-5 text-sm text-amber-800">
            {validation.warnings.map((warning) => (
              <li key={warning}>{warning}</li>
            ))}
          </ul>
        </div>
      )}

      {conflictMessage && (
        <div className="rounded border border-orange-200 bg-orange-50 px-4 py-3 text-sm text-orange-900">
          <p className="font-medium">Revision conflict</p>
          <p className="mt-1">{conflictMessage}</p>
          <button
            type="button"
            onClick={() => {
              void refetchMetadata();
            }}
            className="mt-3 inline-flex items-center gap-2 rounded border border-orange-300 bg-white px-3 py-1.5 text-sm text-orange-900 hover:bg-orange-100"
          >
            Reload latest revision
          </button>
        </div>
      )}

      {saveMessage && (
        <div className="rounded border border-green-200 bg-green-50 px-4 py-3 text-sm text-green-800">
          {saveMessage}
        </div>
      )}

      {pageError && (
        <div className="rounded border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-800">
          {pageError}
        </div>
      )}

      <div>
        {editorView === "config" && <div className="space-y-6">
          <section className="rounded-lg border border-gray-200 bg-white p-4">
            <div className="mb-4">
              <h2 className="text-lg font-semibold text-gray-900">Metadata</h2>
              <p className="text-sm text-gray-600">
                Core lifecycle fields: identity, scope, visibility, tags, and default-home behavior.
              </p>
            </div>
            <div className="grid gap-4 md:grid-cols-2">
              <label className="text-sm text-gray-700">
                <span className="mb-1 block">Scope type</span>
                <select
                  value={draft.scope_type}
                  onChange={(event) => updateScopeField("scope_type", event.target.value)}
                  className="w-full rounded border border-gray-300 px-3 py-2"
                >
                  <option value="global">global</option>
                  <option value="pack">pack</option>
                  <option value="identity">identity</option>
                  <option value="tenant">tenant</option>
                </select>
              </label>
              <label className="text-sm text-gray-700">
                <span className="mb-1 block">Scope ref</span>
                <input
                  value={draft.scope_ref}
                  onChange={(event) => updateScopeField("scope_ref", event.target.value)}
                  className="w-full rounded border border-gray-300 px-3 py-2"
                />
              </label>
              <label className="text-sm text-gray-700">
                <span className="mb-1 block">Label</span>
                <input
                  value={draft.label}
                  onChange={(event) => updateMetadata("label", event.target.value)}
                  onBlur={() => {
                    if (!isEditing && !refLocalPart.trim() && draft.label.trim()) {
                      updateMetadata("ref", composeDashboardRef(refPrefix, draft.label));
                    }
                  }}
                  className="w-full rounded border border-gray-300 px-3 py-2"
                  placeholder="Operations"
                />
              </label>
              <label className="text-sm text-gray-700">
                <span className="mb-1 block">Reference</span>
                <div className="input-with-prefix">
                  <span className="prefix">{refPrefix}.</span>
                  <input
                    value={refLocalPart}
                    onChange={(event) =>
                      updateMetadata(
                        "ref",
                        composeDashboardRef(refPrefix, event.target.value),
                      )
                    }
                    disabled={isEditing}
                    className="disabled:bg-gray-100"
                    placeholder="operations"
                  />
                </div>
              </label>
              <label className="text-sm text-gray-700 md:col-span-2">
                <span className="mb-1 block">Description</span>
                <textarea
                  value={draft.description ?? ""}
                  onChange={(event) => updateMetadata("description", event.target.value)}
                  rows={3}
                  className="w-full rounded border border-gray-300 px-3 py-2"
                />
              </label>
              <label className="text-sm text-gray-700">
                <span className="mb-1 block">Visibility</span>
                <select
                  value={draft.visibility}
                  onChange={(event) => updateMetadata("visibility", event.target.value)}
                  className="w-full rounded border border-gray-300 px-3 py-2"
                >
                  <option value="public">public</option>
                  <option value="pack">pack</option>
                  <option value="private">private</option>
                </select>
              </label>
              <label className="text-sm text-gray-700">
                <span className="mb-1 block">Tags</span>
                <input
                  value={draft.tags.join(", ")}
                  onChange={(event) => updateMetadata("tags", parseTagInput(event.target.value))}
                  className="w-full rounded border border-gray-300 px-3 py-2"
                  placeholder="ops, overview"
                />
              </label>
              <label className="text-sm text-gray-700">
                <span className="mb-1 block">Spec version</span>
                <input
                  type="number"
                  min={1}
                  value={draft.spec_version}
                  onChange={(event) =>
                    updateMetadata(
                      "spec_version",
                      Math.max(1, Number(event.target.value) || 1),
                    )
                  }
                  className="w-full rounded border border-gray-300 px-3 py-2"
                />
              </label>
              <div className="flex flex-wrap items-center gap-4 text-sm text-gray-700 md:col-span-2">
                <label className="inline-flex items-center gap-2">
                  <input
                    type="checkbox"
                    checked={draft.enabled}
                    onChange={(event) => updateMetadata("enabled", event.target.checked)}
                  />
                  Enabled
                </label>
                <label className="inline-flex items-center gap-2">
                  <input
                    type="checkbox"
                    checked={draft.is_default_home}
                    onChange={(event) =>
                      updateMetadata("is_default_home", event.target.checked)
                    }
                  />
                  Default home in scope
                </label>
              </div>
            </div>
          </section>

          <section className="rounded-lg border border-gray-200 bg-white p-4">
            <div className="mb-4 flex flex-wrap items-start justify-between gap-3">
              <div>
                <h2 className="text-lg font-semibold text-gray-900">Layout basics</h2>
                <p className="text-sm text-gray-600">
                  Edit base grid sizing and required per-breakpoint columns for card positioning.
                </p>
              </div>
              <div className="flex items-center gap-2">
                <input
                  value={newBreakpointId}
                  onChange={(event) => setNewBreakpointId(event.target.value)}
                  className="rounded border border-gray-300 px-3 py-2 text-sm"
                  placeholder="md"
                />
                <button
                  type="button"
                  onClick={addBreakpoint}
                  className="inline-flex items-center gap-2 rounded border border-gray-300 px-3 py-2 text-sm text-gray-700 hover:bg-gray-50"
                >
                  <Plus className="h-4 w-4" />
                  Add breakpoint
                </button>
              </div>
            </div>
            <div className="grid gap-4 md:grid-cols-3">
              <label className="text-sm text-gray-700">
                <span className="mb-1 block">Columns</span>
                <input
                  type="number"
                  min={1}
                  value={draft.spec.layout.columns}
                  onChange={(event) =>
                    updateDraft((current) => {
                      current.spec.layout.columns = Math.max(
                        1,
                        Number(event.target.value) || 1,
                      );
                      return current;
                    })
                  }
                  className="w-full rounded border border-gray-300 px-3 py-2"
                />
              </label>
              <label className="text-sm text-gray-700">
                <span className="mb-1 block">Row height</span>
                <input
                  type="number"
                  min={1}
                  value={draft.spec.layout.row_height}
                  onChange={(event) =>
                    updateDraft((current) => {
                      current.spec.layout.row_height = Math.max(
                        1,
                        Number(event.target.value) || 1,
                      );
                      return current;
                    })
                  }
                  className="w-full rounded border border-gray-300 px-3 py-2"
                />
              </label>
              <label className="text-sm text-gray-700">
                <span className="mb-1 block">Gap</span>
                <input
                  type="number"
                  min={0}
                  value={draft.spec.layout.gap}
                  onChange={(event) =>
                    updateDraft((current) => {
                      current.spec.layout.gap = Math.max(
                        0,
                        Number(event.target.value) || 0,
                      );
                      return current;
                    })
                  }
                  className="w-full rounded border border-gray-300 px-3 py-2"
                />
              </label>
            </div>

            <div className="mt-4 overflow-x-auto">
              <table className="min-w-full text-sm">
                <thead>
                  <tr className="border-b border-gray-200 text-left text-gray-600">
                    <th className="px-3 py-2">Breakpoint</th>
                    <th className="px-3 py-2">Min width</th>
                    <th className="px-3 py-2">Columns</th>
                    <th className="px-3 py-2"></th>
                  </tr>
                </thead>
                <tbody>
                  {Object.entries(draft.spec.layout.breakpoints).map(([key, breakpoint]) => (
                    <tr key={key} className="border-b border-gray-100">
                      <td className="px-3 py-2 font-medium text-gray-900">{key}</td>
                      <td className="px-3 py-2">
                        <input
                          type="number"
                          min={0}
                          value={breakpoint.min_width}
                          onChange={(event) =>
                            updateDraft((current) => {
                              current.spec.layout.breakpoints[key].min_width = Math.max(
                                0,
                                Number(event.target.value) || 0,
                              );
                              return current;
                            })
                          }
                          className="w-28 rounded border border-gray-300 px-2 py-1"
                        />
                      </td>
                      <td className="px-3 py-2">
                        <input
                          type="number"
                          min={1}
                          value={breakpoint.columns}
                          onChange={(event) =>
                            updateDraft((current) => {
                              current.spec.layout.breakpoints[key].columns = Math.max(
                                1,
                                Number(event.target.value) || 1,
                              );
                              return current;
                            })
                          }
                          className="w-24 rounded border border-gray-300 px-2 py-1"
                        />
                      </td>
                      <td className="px-3 py-2 text-right">
                        {!(["lg", "sm"].includes(key)) && (
                          <button
                            type="button"
                            onClick={() => removeBreakpoint(key)}
                            className="text-sm text-red-600 hover:text-red-800"
                          >
                            Remove
                          </button>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </section>

          <section className="rounded-lg border border-gray-200 bg-white p-4 space-y-6">
            <div>
              <h2 className="text-lg font-semibold text-gray-900">Cards, data sources, filters, and defaults</h2>
              <p className="text-sm text-gray-600">
                Pragmatic MVP controls for runtime defaults, filter definitions, source contracts, and card placement.
              </p>
            </div>

            <div className="grid gap-4 md:grid-cols-3">
              <label className="text-sm text-gray-700">
                <span className="mb-1 block">Default timezone</span>
                <input
                  value={draft.spec.defaults?.timezone ?? ""}
                  onChange={(event) =>
                    updateDraft((current) => {
                      current.spec.defaults = current.spec.defaults ?? {};
                      current.spec.defaults.timezone = event.target.value;
                      return current;
                    })
                  }
                  className="w-full rounded border border-gray-300 px-3 py-2"
                />
              </label>
              <label className="text-sm text-gray-700">
                <span className="mb-1 block">Default time window</span>
                <input
                  value={draft.spec.defaults?.time_window ?? ""}
                  onChange={(event) =>
                    updateDraft((current) => {
                      current.spec.defaults = current.spec.defaults ?? {};
                      current.spec.defaults.time_window = event.target.value;
                      return current;
                    })
                  }
                  className="w-full rounded border border-gray-300 px-3 py-2"
                />
              </label>
              <label className="text-sm text-gray-700">
                <span className="mb-1 block">Refresh seconds</span>
                <input
                  type="number"
                  min={0}
                  value={draft.spec.defaults?.refresh_seconds ?? 0}
                  onChange={(event) =>
                    updateDraft((current) => {
                      current.spec.defaults = current.spec.defaults ?? {};
                      current.spec.defaults.refresh_seconds = Math.max(
                        0,
                        Number(event.target.value) || 0,
                      );
                      return current;
                    })
                  }
                  className="w-full rounded border border-gray-300 px-3 py-2"
                />
              </label>
            </div>

            <div className="space-y-4">
              <div className="flex items-center justify-between gap-3">
                <div>
                  <h3 className="text-base font-semibold text-gray-900">Filters</h3>
                  <p className="text-sm text-gray-600">Filter definitions used by template-aware sources.</p>
                </div>
                <button
                  type="button"
                  onClick={addFilter}
                  className="inline-flex items-center gap-2 rounded border border-gray-300 px-3 py-1.5 text-sm text-gray-700 hover:bg-gray-50"
                >
                  <Plus className="h-4 w-4" />
                  Add filter
                </button>
              </div>
              {(draft.spec.filters ?? []).length === 0 ? (
                <p className="text-sm text-gray-500">No filters configured.</p>
              ) : (
                <div className="space-y-3">
                  {(draft.spec.filters ?? []).map((filter, index) => (
                    <div key={`filter-${index}`} className="rounded border border-gray-200 p-3">
                      <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-4">
                        <label className="text-sm text-gray-700">
                          <span className="mb-1 block">Id</span>
                          <input
                            value={filter.id}
                            onChange={(event) =>
                              updateDraft((current) => {
                                current.spec.filters![index].id = slugify(event.target.value);
                                return current;
                              })
                            }
                            className="w-full rounded border border-gray-300 px-3 py-2"
                          />
                        </label>
                        <label className="text-sm text-gray-700">
                          <span className="mb-1 block">Type</span>
                          <select
                            value={filter.type}
                            onChange={(event) =>
                              updateDraft((current) => {
                                current.spec.filters![index].type = event.target.value;
                                return current;
                              })
                            }
                            className="w-full rounded border border-gray-300 px-3 py-2"
                          >
                            {[
                              "text",
                              "enum",
                              "number",
                              "boolean",
                              "pack_ref",
                              "action_ref",
                              "queue_ref",
                              "trigger_ref",
                              "time_window",
                            ].map((option) => (
                              <option key={option} value={option}>
                                {option}
                              </option>
                            ))}
                          </select>
                        </label>
                        <label className="text-sm text-gray-700 xl:col-span-2">
                          <span className="mb-1 block">Label</span>
                          <input
                            value={filter.label}
                            onChange={(event) =>
                              updateDraft((current) => {
                                current.spec.filters![index].label = event.target.value;
                                return current;
                              })
                            }
                            className="w-full rounded border border-gray-300 px-3 py-2"
                          />
                        </label>
                        <label className="text-sm text-gray-700 xl:col-span-2">
                          <span className="mb-1 block">Default</span>
                          <input
                            value={formatFilterValue(filter.default)}
                            onChange={(event) =>
                              updateDraft((current) => {
                                current.spec.filters![index].default = parseFilterValueInput(
                                  current.spec.filters![index].type,
                                  event.target.value,
                                  Array.isArray(current.spec.filters![index].default),
                                );
                                return current;
                              })
                            }
                            className="w-full rounded border border-gray-300 px-3 py-2"
                            placeholder="comma-separated for arrays"
                          />
                        </label>
                        <label className="text-sm text-gray-700 xl:col-span-2">
                          <span className="mb-1 block">Options</span>
                          <input
                            value={formatOptions(filter.options)}
                            onChange={(event) =>
                              updateDraft((current) => {
                                current.spec.filters![index].options = parseOptionsInput(
                                  current.spec.filters![index].type,
                                  event.target.value,
                                );
                                return current;
                              })
                            }
                            className="w-full rounded border border-gray-300 px-3 py-2"
                            placeholder="value1, value2"
                          />
                        </label>
                      </div>
                      <div className="mt-3 text-right">
                        <button
                          type="button"
                          onClick={() =>
                            updateDraft((current) => {
                              current.spec.filters!.splice(index, 1);
                              return current;
                            })
                          }
                          className="text-sm text-red-600 hover:text-red-800"
                        >
                          Remove filter
                        </button>
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </div>

            <div className="space-y-4">
              <div className="flex items-center justify-between gap-3">
                <div>
                  <h3 className="text-base font-semibold text-gray-900">Data sources</h3>
                  <p className="text-sm text-gray-600">Typed inputs come from the source contract catalog when available.</p>
                </div>
                <button
                  type="button"
                  onClick={addSource}
                  className="inline-flex items-center gap-2 rounded border border-gray-300 px-3 py-1.5 text-sm text-gray-700 hover:bg-gray-50"
                >
                  <Plus className="h-4 w-4" />
                  Add source
                </button>
              </div>
              {Object.entries(draft.spec.data_sources).length === 0 ? (
                <p className="text-sm text-gray-500">No data sources configured.</p>
              ) : (
                <div className="space-y-3">
                  <div className="space-y-2">
                    {Object.entries(draft.spec.data_sources).map(([sourceId, source]) => (
                      (() => {
                        const paramEntries = Object.entries(source.params ?? {}).filter(
                          ([, value]) => value !== undefined && value !== null && value !== "",
                        );
                        const populatedParamPreview =
                          paramEntries.length > 0
                            ? paramEntries
                                .slice(0, 3)
                                .map(([key, value]) => `${key}=${formatSourceParamValue(value)}`)
                                .join(", ")
                            : "no params";
                        const hasMoreParams = paramEntries.length > 3;
                        return (
                      <div
                        key={`source-row-${sourceId}`}
                        className="flex items-center justify-between rounded border border-gray-200 bg-white px-3 py-2"
                      >
                        <div className="min-w-0">
                          <p className="truncate text-sm font-medium text-gray-900">{sourceId}</p>
                          <p className="truncate text-xs text-gray-500">
                            type: {source.type} · {populatedParamPreview}
                            {hasMoreParams ? ", …" : ""}
                          </p>
                        </div>
                        <button
                          type="button"
                          onClick={() => setActiveSourceConfigId(sourceId)}
                          className="inline-flex items-center gap-1 rounded border border-gray-300 px-2 py-1 text-sm text-gray-700 hover:bg-gray-50"
                        >
                          <Settings className="h-4 w-4" />
                          Configure
                        </button>
                      </div>
                        );
                      })()
                    ))}
                  </div>

                  {Object.entries(draft.spec.data_sources).map(([sourceId, source]) => {
                    if (activeSourceConfigId !== sourceId) {
                      return null;
                    }
                    const contract = sourceContractsByType.get(source.type);
                    const knownParams = new Set(contract?.params.map((param) => param.name) ?? []);
                    const extraParams = Object.keys(source.params ?? {}).filter(
                      (key) => !knownParams.has(key),
                    );
                    return (
                      <div
                        key={sourceId}
                        className="fixed inset-0 z-40 flex items-center justify-center bg-gray-900/50 p-4"
                      >
                        <div className="w-full max-w-4xl rounded-lg bg-white shadow-xl">
                          <div className="flex items-center justify-between border-b border-gray-200 px-4 py-3">
                            <h4 className="text-base font-semibold text-gray-900">
                              Configure source: {sourceId}
                            </h4>
                            <button
                              type="button"
                              onClick={() => setActiveSourceConfigId(null)}
                              className="rounded p-1 text-gray-500 hover:bg-gray-100 hover:text-gray-700"
                              aria-label="Close source configuration"
                            >
                              <X className="h-4 w-4" />
                            </button>
                          </div>
                          <div className="max-h-[80vh] overflow-y-auto p-4">
                      <div className="rounded border border-gray-200 p-3">
                        <div className="grid gap-3 md:grid-cols-2">
                          <label className="text-sm text-gray-700">
                            <span className="mb-1 block">Source id</span>
                            <input
                              defaultValue={sourceId}
                              onBlur={(event) => {
                                const nextId = slugify(event.target.value);
                                if (!nextId || nextId === sourceId) {
                                  event.target.value = sourceId;
                                  return;
                                }
                                updateDraft((current) => {
                                  const currentSource = current.spec.data_sources[sourceId];
                                  delete current.spec.data_sources[sourceId];
                                  current.spec.data_sources[nextId] = currentSource;
                                  current.spec.cards.forEach((card) => {
                                    if (card.source === sourceId) {
                                      card.source = nextId;
                                    }
                                  });
                                  return current;
                                });
                                setActiveSourceConfigId((current) =>
                                  current === sourceId ? nextId : current,
                                );
                              }}
                              onKeyDown={(event) => {
                                if (event.key === "Enter") {
                                  event.preventDefault();
                                  event.currentTarget.blur();
                                }
                              }}
                              className="w-full rounded border border-gray-300 px-3 py-2"
                            />
                          </label>
                          <label className="text-sm text-gray-700">
                            <span className="mb-1 block">Source type</span>
                            <select
                              value={source.type}
                              onChange={(event) =>
                                updateDraft((current) => {
                                  current.spec.data_sources[sourceId].type = event.target.value;
                                  return current;
                                })
                              }
                              className="w-full rounded border border-gray-300 px-3 py-2"
                              disabled={sourceCatalogLoading}
                            >
                              {Array.from(
                                new Set([
                                  ...catalogContracts.map((entry) => entry.source_type),
                                  source.type,
                                ]),
                              )
                                .sort((left, right) => left.localeCompare(right))
                                .map((sourceType) => {
                                  const optionContract =
                                    sourceContractsByType.get(sourceType);
                                  return (
                                  <option key={sourceType} value={sourceType}>
                                    {sourceTypeOptionLabel(
                                      sourceType,
                                      optionContract,
                                    )}
                                  </option>
                                  );
                                })}
                            </select>
                          </label>
                        </div>

                        {contract && (
                          <div className="mt-3 flex flex-wrap items-center gap-2 text-xs text-gray-600">
                            {contract.availability !== "available_now" && (
                              <span className={`rounded px-2 py-0.5 ${availabilityBadgeClass(contract.availability)}`}>
                                {contract.availability.replaceAll("_", " ")}
                              </span>
                            )}
                            <span>response: {contract.response_shape}</span>
                            <span>auth: {contract.authorization_basis}</span>
                            {contract.notes && <span>{contract.notes}</span>}
                          </div>
                        )}

                        <div className="mt-3 grid gap-3 md:grid-cols-2 xl:grid-cols-3">
                          {(contract?.params ?? []).map((param) => {
                            const currentValue = source.params?.[param.name];
                            return (
                              <label key={param.name} className="text-sm text-gray-700">
                                <span className="mb-1 block">
                                  {param.name}
                                  {param.required && <span className="text-red-500"> *</span>}
                                </span>
                                {param.input === "boolean" ? (
                                  <select
                                    value={String(currentValue ?? "")}
                                    onChange={(event) =>
                                      updateDraft((current) => {
                                        current.spec.data_sources[sourceId].params =
                                          current.spec.data_sources[sourceId].params ?? {};
                                        if (!event.target.value) {
                                          delete current.spec.data_sources[sourceId].params[param.name];
                                        } else {
                                          current.spec.data_sources[sourceId].params[param.name] =
                                            event.target.value === "true";
                                        }
                                        return current;
                                      })
                                    }
                                    className="w-full rounded border border-gray-300 px-3 py-2"
                                  >
                                    <option value="">Unset</option>
                                    <option value="true">true</option>
                                    <option value="false">false</option>
                                  </select>
                                ) : (
                                  <input
                                    type={param.input === "number" ? "number" : "text"}
                                    value={formatSourceParamValue(currentValue)}
                                    onChange={(event) =>
                                      updateDraft((current) => {
                                        current.spec.data_sources[sourceId].params =
                                          current.spec.data_sources[sourceId].params ?? {};
                                        if (!event.target.value) {
                                          delete current.spec.data_sources[sourceId].params[param.name];
                                        } else {
                                          current.spec.data_sources[sourceId].params[param.name] =
                                            parseSourceParamValue(
                                              event.target.value,
                                              param.input,
                                            );
                                        }
                                        return current;
                                      })
                                    }
                                    placeholder={param.input === "text" ? "Supports {{ filters.* }} templates" : undefined}
                                    className="w-full rounded border border-gray-300 px-3 py-2"
                                  />
                                )}
                              </label>
                            );
                          })}
                        </div>

                        {extraParams.length > 0 && (
                          <div className="mt-3 rounded border border-blue-200 bg-blue-50 px-3 py-2 text-xs text-blue-800">
                            Extra params preserved in YAML: {extraParams.join(", ")}
                          </div>
                        )}

                        <div className="mt-3 text-right">
                          <button
                            type="button"
                            onClick={() =>
                              updateDraft((current) => {
                                delete current.spec.data_sources[sourceId];
                                return current;
                              })
                            }
                            className="text-sm text-red-600 hover:text-red-800"
                          >
                            Remove source
                          </button>
                        </div>
                      </div>
                          </div>
                        </div>
                      </div>
                    );
                  })}
                </div>
              )}
            </div>

            <div className="space-y-4">
              <div className="flex items-center justify-between gap-3">
                <div>
                  <h3 className="text-base font-semibold text-gray-900">Cards</h3>
                  <p className="text-sm text-gray-600">Basic card metadata, field mappings, and per-breakpoint positions.</p>
                </div>
                <button
                  type="button"
                  onClick={addCard}
                  className="inline-flex items-center gap-2 rounded border border-gray-300 px-3 py-1.5 text-sm text-gray-700 hover:bg-gray-50"
                >
                  <Plus className="h-4 w-4" />
                  Add card
                </button>
              </div>
              {draft.spec.cards.length === 0 ? (
                <p className="text-sm text-gray-500">No cards configured.</p>
              ) : (
                <div className="space-y-3">
                  <div className="rounded border border-gray-200 bg-gray-50 p-3">
                    <div className="mb-3 flex items-center justify-between gap-3">
                      <p className="text-sm text-gray-700">
                        Drag cards to move. Drag the corner handle to resize.
                      </p>
                      <label className="text-sm text-gray-700">
                        <span className="mr-2">Breakpoint</span>
                        <select
                          value={layoutBreakpoint}
                          onChange={(event) => setLayoutBreakpoint(event.target.value)}
                          className="rounded border border-gray-300 px-2 py-1"
                        >
                          {Object.keys(draft.spec.layout.breakpoints).map((breakpoint) => (
                            <option key={breakpoint} value={breakpoint}>
                              {breakpoint}
                            </option>
                          ))}
                        </select>
                      </label>
                    </div>
                    <div
                      ref={layoutCanvasRef}
                      className="relative w-full overflow-hidden rounded border border-dashed border-gray-300 bg-white"
                      style={{
                        height: layoutRows * activeLayoutRowHeight + Math.max(0, layoutRows - 1) * activeLayoutGap,
                        backgroundImage:
                          "linear-gradient(to right, rgba(148,163,184,0.25) 1px, transparent 1px), linear-gradient(to bottom, rgba(148,163,184,0.2) 1px, transparent 1px)",
                        backgroundSize:
                          layoutUnitWidth > 0
                            ? `${layoutUnitWidth + activeLayoutGap}px ${activeLayoutRowHeight + activeLayoutGap}px`
                            : undefined,
                      }}
                    >
                      {draft.spec.cards.map((card) => {
                        const rect = resolveRectForBreakpoint(
                          card.position,
                          layoutBreakpoint,
                          draft.spec.layout.breakpoints,
                          draft.spec.layout.columns,
                        );
                        const source = draft.spec.data_sources[card.source];
                        const previewSource = previewSourceMap.get(card.source);
                        const matchingPreviewSource =
                          source && previewSource?.source_type === source.type
                            ? previewSource
                            : undefined;
                        const canvasSource =
                          matchingPreviewSource && matchingPreviewSource.data
                            ? matchingPreviewSource
                            : sampleSourceForVisualization(card.visualization.type);
                        const rowSample =
                          Array.isArray(canvasSource.data) && canvasSource.data.length > 0
                            ? canvasSource.data[0]
                            : canvasSource.data && !Array.isArray(canvasSource.data)
                              ? canvasSource.data
                              : null;
                        const rowSampleText = rowSample
                          ? Object.entries(rowSample)
                              .slice(0, 2)
                              .map(([key, value]) => `${key}: ${String(value)}`)
                              .join(" · ")
                          : "No sample data";
                        const canRenderMiniChart = !["table", "stat", "kpi"].includes(
                          card.visualization.type,
                        );
                        const left = rect.x * (layoutUnitWidth + activeLayoutGap);
                        const top = rect.y * (activeLayoutRowHeight + activeLayoutGap);
                        const width = rect.w * layoutUnitWidth + Math.max(0, rect.w - 1) * activeLayoutGap;
                        const height = rect.h * activeLayoutRowHeight + Math.max(0, rect.h - 1) * activeLayoutGap;
                        return (
                          <div
                            key={`${card.id}-${layoutBreakpoint}`}
                            className="absolute flex flex-col overflow-hidden rounded border border-blue-300 bg-blue-50 shadow-sm"
                            style={{ left, top, width, height }}
                            onMouseDown={(event) => startCardInteraction(event, card.id, "move")}
                          >
                            <div className="flex items-center justify-between gap-2 border-b border-blue-200 px-2 py-1 text-xs text-blue-900">
                              <span className="truncate font-medium">{card.title || card.id}</span>
                              <button
                                type="button"
                                onMouseDown={(event) => {
                                  event.stopPropagation();
                                }}
                                onClick={() => setActiveCardConfigId(card.id)}
                                className="rounded p-0.5 text-blue-700 hover:bg-blue-100"
                                aria-label={`Configure ${card.id}`}
                              >
                                <Settings className="h-3.5 w-3.5" />
                              </button>
                            </div>
                            <div className="px-2 py-1 text-[10px] text-blue-800">
                              {card.id} · {rect.w}x{rect.h}
                            </div>
                            <div className="px-2 pb-1 text-[10px] text-blue-900">
                              <span className="rounded bg-blue-100 px-1.5 py-0.5 font-medium">
                                {card.visualization.type}
                              </span>
                              <span className="ml-1 text-blue-700">
                                {matchingPreviewSource?.data ? "live preview" : "sample"}
                              </span>
                            </div>
                            <div className="flex-1 min-h-0 px-2 pb-2">
                              {canRenderMiniChart ? (
                                <div className="h-full overflow-hidden rounded border border-blue-200 bg-white">
                                  <DashboardChartRenderer card={card} source={canvasSource} />
                                </div>
                              ) : (
                                <div className="h-full rounded border border-blue-200 bg-white px-2 py-1 text-[10px] text-blue-900">
                                  {rowSampleText}
                                </div>
                              )}
                            </div>
                            <button
                              type="button"
                              onMouseDown={(event) => startCardInteraction(event, card.id, "resize_left")}
                              className="absolute bottom-0 left-0 h-3 w-3 cursor-sw-resize rounded-tr border-r border-t border-blue-300 bg-blue-200/80"
                              aria-label={`Resize ${card.id} from left`}
                            />
                            <button
                              type="button"
                              onMouseDown={(event) => startCardInteraction(event, card.id, "resize_right")}
                              className="absolute bottom-0 right-0 h-3 w-3 cursor-se-resize rounded-tl border-l border-t border-blue-300 bg-blue-200/80"
                              aria-label={`Resize ${card.id} from right`}
                            />
                          </div>
                        );
                      })}
                    </div>
                  </div>

                  {draft.spec.cards.map((card, index) => {
                    if (activeCardConfigId !== card.id) {
                      return null;
                    }
                    const source = draft.spec.data_sources[card.source];
                    const contract = source
                      ? sourceContractsByType.get(source.type)
                      : undefined;
                    const previewSource = previewSourceMap.get(card.source);
                    const matchingPreviewSource =
                      source && previewSource?.source_type === source.type
                        ? previewSource
                        : undefined;
                    const fieldOptions = mergeFieldOptions(
                      matchingPreviewSource?.meta.ordering,
                      Object.keys(matchingPreviewSource?.meta.unit_hints ?? {}),
                      sourceDataFields(matchingPreviewSource?.data),
                      contract?.ordering,
                    );
                    const visualizationFieldSuggestions =
                      contract?.ordering && contract.ordering.length > 0
                        ? contract.ordering
                        : fieldOptions;
                    const visualizationFields = visualizationFieldConfigFor(
                      card.visualization.type,
                    );
                    const gaugeMin = card.visualization.min ?? 0;
                    const gaugeMax = card.visualization.max ?? 100;
                    const gaugeMode = gaugeModeForVisualization(card.visualization);
                    const gaugeHigherIsBetter = gaugeMode === "low_is_bad";
                    const gaugeIsTargetRange = gaugeMode === "target_range";
                    const { warningStart, badStart } = gaugeThresholds(
                      card.visualization,
                      gaugeMin,
                      gaugeMax,
                    );
                    const {
                      lowerWarningStart,
                      goodStart,
                      goodEnd,
                      upperWarningEnd,
                    } = gaugeTargetRangeThresholds(card.visualization, gaugeMin, gaugeMax);
                    return (
                      <div
                        key={`card-config-${index}`}
                        className="fixed inset-0 z-40 flex items-center justify-center bg-gray-900/50 p-4"
                      >
                        <div className="w-full max-w-5xl rounded-lg bg-white shadow-xl">
                          <div className="flex items-center justify-between border-b border-gray-200 px-4 py-3">
                            <h4 className="text-base font-semibold text-gray-900">
                              Configure card: {card.title || card.id}
                            </h4>
                            <button
                              type="button"
                              onClick={() => setActiveCardConfigId(null)}
                              className="rounded p-1 text-gray-500 hover:bg-gray-100 hover:text-gray-700"
                              aria-label="Close card configuration"
                            >
                              <X className="h-4 w-4" />
                            </button>
                          </div>
                          <div className="max-h-[80vh] overflow-y-auto p-4">
                      <div className="rounded border border-gray-200 p-3">
                        <div className="space-y-4">
                          <div className="rounded border border-gray-100 p-3">
                            <h5 className="mb-3 text-sm font-semibold text-gray-900">Card details</h5>
                            <div className="grid gap-3 md:grid-cols-2">
                              <div className="grid gap-3 md:grid-cols-2">
                                <label className="text-sm text-gray-700">
                                  <span className="mb-1 block">Card id</span>
                                  <input
                                    value={card.id}
                                    onChange={(event) => {
                                      const nextCardId = slugify(event.target.value);
                                      setActiveCardConfigId(nextCardId);
                                      updateDraft((current) => {
                                        current.spec.cards[index].id = nextCardId;
                                        return current;
                                      });
                                    }}
                                    className="w-full rounded border border-gray-300 px-3 py-2"
                                  />
                                </label>
                                <label className="text-sm text-gray-700">
                                  <span className="mb-1 block">Title</span>
                                  <input
                                    value={card.title}
                                    onChange={(event) =>
                                      updateDraft((current) => {
                                        current.spec.cards[index].title = event.target.value;
                                        return current;
                                      })
                                    }
                                    className="w-full rounded border border-gray-300 px-3 py-2"
                                  />
                                </label>
                              </div>
                              <label className="text-sm text-gray-700">
                                <span className="mb-1 block">Subtitle</span>
                                <input
                                  value={card.subtitle ?? ""}
                                  onChange={(event) =>
                                    updateDraft((current) => {
                                      current.spec.cards[index].subtitle = event.target.value;
                                      return current;
                                    })
                                  }
                                  className="w-full rounded border border-gray-300 px-3 py-2"
                                />
                              </label>
                            </div>
                          </div>

                          <div className="rounded border border-gray-100 p-3">
                            <h5 className="mb-3 text-sm font-semibold text-gray-900">Data Source and Diplay Format</h5>
                            <div className="grid gap-3 md:grid-cols-2">
                              <label className="text-sm text-gray-700">
                                <span className="mb-1 block">Source</span>
                                <select
                                  value={card.source}
                                  onChange={(event) =>
                                    updateDraft((current) => {
                                      current.spec.cards[index].source = event.target.value;
                                      return current;
                                    })
                                  }
                                  className="w-full rounded border border-gray-300 px-3 py-2"
                                >
                                  {Object.keys(draft.spec.data_sources).map((sourceId) => (
                                    <option key={sourceId} value={sourceId}>
                                      {sourceId}
                                    </option>
                                  ))}
                                </select>
                              </label>
                              <label className="text-sm text-gray-700">
                                <span className="mb-1 block">Visualization</span>
                                <select
                                  value={card.visualization.type}
                                  onChange={(event) =>
                                    updateDraft((current) => {
                                      current.spec.cards[index].visualization.type = event.target.value;
                                      return current;
                                    })
                                  }
                                  className="w-full rounded border border-gray-300 px-3 py-2"
                                >
                                  {[
                                    "table",
                                    "stat",
                                    "kpi",
                                    "timeseries",
                                    "stacked_timeseries",
                                    "gauge",
                                    "bar",
                                    "heatmap",
                                    "histogram",
                                    "funnel",
                                    "treemap",
                                    "status_matrix",
                                  ].map((option) => (
                                    <option key={option} value={option}>
                                      {option}
                                    </option>
                                  ))}
                                </select>
                              </label>
                            </div>
                          </div>

                          <div className="rounded border border-gray-100 p-3">
                            <h5 className="mb-2 text-sm font-semibold text-gray-900">Chart Config</h5>
                            {visualizationFieldSuggestions.length > 0 && (
                              <p className="mb-3 text-xs text-gray-600">
                                Schema keywords: {visualizationFieldSuggestions.join(", ")}
                              </p>
                            )}
                            <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
                              {visualizationFields.map((field) => (
                                <AutocompleteInput
                                  key={`${card.id}-${field.key}`}
                                  label={field.label}
                                  value={card.visualization[field.key] ?? ""}
                                  onChange={(value) =>
                                    updateDraft((current) => {
                                      const nextValue = value.trim();
                                      current.spec.cards[index].visualization[field.key] =
                                        nextValue || undefined;
                                      return current;
                                    })
                                  }
                                  suggestions={visualizationFieldSuggestions}
                                  placeholder="Search schema keywords or type a field"
                                />
                              ))}
                            </div>
                            {visualizationFields.length === 0 && (
                              <p className="text-sm text-gray-500">
                                This visualization does not require explicit field mapping.
                              </p>
                            )}
                          </div>

                          {card.visualization.type === "gauge" && (
                            <>
                              <label className="text-sm text-gray-700 md:col-span-2 xl:col-span-3">
                                <span className="mb-1 block">Gauge mode</span>
                                <select
                                  value={gaugeMode}
                                  onChange={(event) =>
                                    updateDraft((current) => {
                                      const visualization =
                                        current.spec.cards[index].visualization;
                                      const currentMin = visualization.min ?? 0;
                                      const currentMax = visualization.max ?? 100;
                                      const nextMode = event.target.value as GaugeMode;
                                      visualization.mode = nextMode;
                                      if (nextMode === "target_range") {
                                        const nextThresholds = gaugeTargetRangeThresholds(
                                          visualization,
                                          currentMin,
                                          currentMax,
                                        );
                                        visualization.bands = buildGaugeTargetRangeBands(
                                          currentMin,
                                          currentMax,
                                          nextThresholds.lowerWarningStart,
                                          nextThresholds.goodStart,
                                          nextThresholds.goodEnd,
                                          nextThresholds.upperWarningEnd,
                                        );
                                      } else {
                                        const nextThresholds = gaugeThresholds(
                                          visualization,
                                          currentMin,
                                          currentMax,
                                        );
                                        visualization.bands = buildGaugeDirectionalBands(
                                          currentMin,
                                          currentMax,
                                          nextThresholds.warningStart,
                                          nextThresholds.badStart,
                                          nextMode,
                                        );
                                      }
                                      return current;
                                    })
                                  }
                                  className="w-full rounded border border-gray-300 px-3 py-2"
                                >
                                  <option value="high_is_bad">Lower is better</option>
                                  <option value="low_is_bad">Higher is better</option>
                                  <option value="target_range">Target range</option>
                                </select>
                              </label>
                              <label className="text-sm text-gray-700">
                                <span className="mb-1 block">Gauge min</span>
                                <input
                                  type="number"
                                  value={gaugeMin}
                                  onChange={(event) =>
                                    updateDraft((current) => {
                                      const visualization =
                                        current.spec.cards[index].visualization;
                                      const nextMin = Number(event.target.value) || 0;
                                      const nextMax = Math.max(
                                        nextMin + 1,
                                        visualization.max ?? 100,
                                      );
                                      const nextThresholds = gaugeThresholds(
                                        visualization,
                                        nextMin,
                                        nextMax,
                                      );
                                      visualization.min = nextMin;
                                      visualization.max = nextMax;
                                      if (gaugeModeForVisualization(visualization) === "target_range") {
                                        const targetThresholds = gaugeTargetRangeThresholds(
                                          visualization,
                                          nextMin,
                                          nextMax,
                                        );
                                        visualization.bands = buildGaugeTargetRangeBands(
                                          nextMin,
                                          nextMax,
                                          targetThresholds.lowerWarningStart,
                                          targetThresholds.goodStart,
                                          targetThresholds.goodEnd,
                                          targetThresholds.upperWarningEnd,
                                        );
                                      } else {
                                        visualization.bands = buildGaugeDirectionalBands(
                                          nextMin,
                                          nextMax,
                                          nextThresholds.warningStart,
                                          nextThresholds.badStart,
                                          gaugeModeForVisualization(visualization),
                                        );
                                      }
                                      return current;
                                    })
                                  }
                                  className="w-full rounded border border-gray-300 px-3 py-2"
                                />
                              </label>
                              <label className="text-sm text-gray-700">
                                <span className="mb-1 block">Gauge max</span>
                                <input
                                  type="number"
                                  value={gaugeMax}
                                  onChange={(event) =>
                                    updateDraft((current) => {
                                      const visualization =
                                        current.spec.cards[index].visualization;
                                      const currentMin = visualization.min ?? 0;
                                      const nextMax = Math.max(
                                        currentMin + 1,
                                        Number(event.target.value) || 100,
                                      );
                                      const nextThresholds = gaugeThresholds(
                                        visualization,
                                        currentMin,
                                        nextMax,
                                      );
                                      visualization.max = nextMax;
                                      if (gaugeModeForVisualization(visualization) === "target_range") {
                                        const targetThresholds = gaugeTargetRangeThresholds(
                                          visualization,
                                          currentMin,
                                          nextMax,
                                        );
                                        visualization.bands = buildGaugeTargetRangeBands(
                                          currentMin,
                                          nextMax,
                                          targetThresholds.lowerWarningStart,
                                          targetThresholds.goodStart,
                                          targetThresholds.goodEnd,
                                          targetThresholds.upperWarningEnd,
                                        );
                                      } else {
                                        visualization.bands = buildGaugeDirectionalBands(
                                          currentMin,
                                          nextMax,
                                          nextThresholds.warningStart,
                                          nextThresholds.badStart,
                                          gaugeModeForVisualization(visualization),
                                        );
                                      }
                                      return current;
                                    })
                                  }
                                  className="w-full rounded border border-gray-300 px-3 py-2"
                                />
                              </label>
                              {!gaugeIsTargetRange && (
                                <>
                                  <label className="text-sm text-gray-700">
                                    <span className="mb-1 block">Warning threshold</span>
                                    <input
                                      type="number"
                                      value={warningStart}
                                      onChange={(event) =>
                                        updateDraft((current) => {
                                          const visualization =
                                            current.spec.cards[index].visualization;
                                          const currentMin = visualization.min ?? 0;
                                          const currentMax = visualization.max ?? 100;
                                          const nextWarning = Number(event.target.value) || 0;
                                          const currentThresholds = gaugeThresholds(
                                            visualization,
                                            currentMin,
                                            currentMax,
                                          );
                                          visualization.bands = buildGaugeDirectionalBands(
                                            currentMin,
                                            currentMax,
                                            nextWarning,
                                            currentThresholds.badStart,
                                            gaugeModeForVisualization(visualization),
                                          );
                                          return current;
                                        })
                                      }
                                      className="w-full rounded border border-gray-300 px-3 py-2"
                                    />
                                  </label>
                                  <label className="text-sm text-gray-700">
                                    <span className="mb-1 block">
                                      {gaugeHigherIsBetter ? "Good threshold" : "Bad threshold"}
                                    </span>
                                    <input
                                      type="number"
                                      value={badStart}
                                      onChange={(event) =>
                                        updateDraft((current) => {
                                          const visualization =
                                            current.spec.cards[index].visualization;
                                          const currentMin = visualization.min ?? 0;
                                          const currentMax = visualization.max ?? 100;
                                          const nextBad = Number(event.target.value) || 0;
                                          const currentThresholds = gaugeThresholds(
                                            visualization,
                                            currentMin,
                                            currentMax,
                                          );
                                          visualization.bands = buildGaugeDirectionalBands(
                                            currentMin,
                                            currentMax,
                                            currentThresholds.warningStart,
                                            nextBad,
                                            gaugeModeForVisualization(visualization),
                                          );
                                          return current;
                                        })
                                      }
                                      className="w-full rounded border border-gray-300 px-3 py-2"
                                    />
                                  </label>
                                </>
                              )}
                              {gaugeIsTargetRange && (
                                <>
                                  <label className="text-sm text-gray-700">
                                    <span className="mb-1 block">Lower warning start</span>
                                    <input
                                      type="number"
                                      value={lowerWarningStart}
                                      onChange={(event) =>
                                        updateDraft((current) => {
                                          const visualization =
                                            current.spec.cards[index].visualization;
                                          const currentMin = visualization.min ?? 0;
                                          const currentMax = visualization.max ?? 100;
                                          const nextLowerWarningStart = Number(event.target.value) || 0;
                                          const currentThresholds = gaugeTargetRangeThresholds(
                                            visualization,
                                            currentMin,
                                            currentMax,
                                          );
                                          visualization.bands = buildGaugeTargetRangeBands(
                                            currentMin,
                                            currentMax,
                                            nextLowerWarningStart,
                                            currentThresholds.goodStart,
                                            currentThresholds.goodEnd,
                                            currentThresholds.upperWarningEnd,
                                          );
                                          return current;
                                        })
                                      }
                                      className="w-full rounded border border-gray-300 px-3 py-2"
                                    />
                                  </label>
                                  <label className="text-sm text-gray-700">
                                    <span className="mb-1 block">Good range start</span>
                                    <input
                                      type="number"
                                      value={goodStart}
                                      onChange={(event) =>
                                        updateDraft((current) => {
                                          const visualization =
                                            current.spec.cards[index].visualization;
                                          const currentMin = visualization.min ?? 0;
                                          const currentMax = visualization.max ?? 100;
                                          const nextGoodStart = Number(event.target.value) || 0;
                                          const currentThresholds = gaugeTargetRangeThresholds(
                                            visualization,
                                            currentMin,
                                            currentMax,
                                          );
                                          visualization.bands = buildGaugeTargetRangeBands(
                                            currentMin,
                                            currentMax,
                                            currentThresholds.lowerWarningStart,
                                            nextGoodStart,
                                            currentThresholds.goodEnd,
                                            currentThresholds.upperWarningEnd,
                                          );
                                          return current;
                                        })
                                      }
                                      className="w-full rounded border border-gray-300 px-3 py-2"
                                    />
                                  </label>
                                  <label className="text-sm text-gray-700">
                                    <span className="mb-1 block">Good range end</span>
                                    <input
                                      type="number"
                                      value={goodEnd}
                                      onChange={(event) =>
                                        updateDraft((current) => {
                                          const visualization =
                                            current.spec.cards[index].visualization;
                                          const currentMin = visualization.min ?? 0;
                                          const currentMax = visualization.max ?? 100;
                                          const nextGoodEnd = Number(event.target.value) || 0;
                                          const currentThresholds = gaugeTargetRangeThresholds(
                                            visualization,
                                            currentMin,
                                            currentMax,
                                          );
                                          visualization.bands = buildGaugeTargetRangeBands(
                                            currentMin,
                                            currentMax,
                                            currentThresholds.lowerWarningStart,
                                            currentThresholds.goodStart,
                                            nextGoodEnd,
                                            currentThresholds.upperWarningEnd,
                                          );
                                          return current;
                                        })
                                      }
                                      className="w-full rounded border border-gray-300 px-3 py-2"
                                    />
                                  </label>
                                  <label className="text-sm text-gray-700">
                                    <span className="mb-1 block">Upper warning end</span>
                                    <input
                                      type="number"
                                      value={upperWarningEnd}
                                      onChange={(event) =>
                                        updateDraft((current) => {
                                          const visualization =
                                            current.spec.cards[index].visualization;
                                          const currentMin = visualization.min ?? 0;
                                          const currentMax = visualization.max ?? 100;
                                          const nextUpperWarningEnd = Number(event.target.value) || 0;
                                          const currentThresholds = gaugeTargetRangeThresholds(
                                            visualization,
                                            currentMin,
                                            currentMax,
                                          );
                                          visualization.bands = buildGaugeTargetRangeBands(
                                            currentMin,
                                            currentMax,
                                            currentThresholds.lowerWarningStart,
                                            currentThresholds.goodStart,
                                            currentThresholds.goodEnd,
                                            nextUpperWarningEnd,
                                          );
                                          return current;
                                        })
                                      }
                                      className="w-full rounded border border-gray-300 px-3 py-2"
                                    />
                                  </label>
                                </>
                              )}
                            </>
                          )}
                        </div>

                        <div className="mt-3 text-right">
                          <button
                            type="button"
                            onClick={() =>
                              updateDraft((current) => {
                                current.spec.cards.splice(index, 1);
                                return current;
                              })
                            }
                            className="text-sm text-red-600 hover:text-red-800"
                          >
                            Remove card
                          </button>
                        </div>
                      </div>
                          </div>
                        </div>
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          </section>
        </div>}

        {editorView === "yaml" && <DashboardYamlPanel yamlText={yamlText} />}

        {editorView === "preview" && (
          <section className="rounded-lg border border-gray-200 bg-white p-4">
            <div className="flex items-start justify-between gap-3">
              <div>
                <h2 className="text-lg font-semibold text-gray-900">Preview</h2>
                <p className="text-sm text-gray-600">
                  Use preview to inspect source-level statuses and a basic live rendering of the current draft.
                </p>
              </div>
              <label className="text-sm text-gray-700">
                <span className="mb-1 block">Breakpoint</span>
                <select
                  value={previewBreakpoint}
                  onChange={(event) => setPreviewBreakpoint(event.target.value)}
                  className="rounded border border-gray-300 px-3 py-2"
                >
                  {Object.keys(draft.spec.layout.breakpoints).map((breakpoint) => (
                    <option key={breakpoint} value={breakpoint}>
                      {breakpoint}
                    </option>
                  ))}
                </select>
              </label>
            </div>

            {previewMutation.data?.warning && (
              <div className="mt-4 rounded border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-800">
                {previewMutation.data.warning}
              </div>
            )}

            <div className="mt-4">
              <DashboardPreviewStatusList sources={previewMutation.data?.data.sources ?? []} />
            </div>

            {previewMutation.data && (
              <div className="mt-4 space-y-3">
                <div className="rounded border border-gray-200 bg-gray-50 px-3 py-2 text-xs text-gray-600">
                  resolved at {previewMutation.data.data.resolved_at} • {previewMutation.data.mode === "draft" ? "draft preview" : "published fallback"}
                </div>
                <DashboardPreviewGrid
                  spec={previewSpec}
                  breakpoint={previewBreakpoint}
                  sourceById={previewSourceMap}
                />
              </div>
            )}
          </section>
        )}
      </div>
    </div>
  );
}
