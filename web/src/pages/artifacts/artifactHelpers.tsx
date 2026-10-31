import {
  FileText,
  FileImage,
  File,
  BarChart3,
  Link as LinkIcon,
  Table2,
  Package,
} from "lucide-react";
import type { ArtifactType, OwnerType } from "@/hooks/useArtifacts";
import { OpenAPI } from "@/api/core/OpenAPI";

// ============================================================================
// Filter option constants
// ============================================================================

export const TYPE_OPTIONS: { value: ArtifactType; label: string }[] = [
  { value: "file_text", label: "Text File" },
  { value: "file_image", label: "Image" },
  { value: "file_binary", label: "Binary" },
  { value: "file_datatable", label: "Data Table" },
  { value: "progress", label: "Progress" },
  { value: "url", label: "URL" },
  { value: "other", label: "Other" },
];

export const VISIBILITY_OPTIONS: { value: string; label: string }[] = [
  { value: "public", label: "Public" },
  { value: "private", label: "Private" },
];

export const SCOPE_OPTIONS: { value: OwnerType; label: string }[] = [
  { value: "system", label: "System" },
  { value: "pack", label: "Pack" },
  { value: "action", label: "Action" },
  { value: "sensor", label: "Sensor" },
  { value: "identity", label: "Identity" },
];

// ============================================================================
// Icon / badge helpers
// ============================================================================

export function getArtifactTypeIcon(type: ArtifactType) {
  switch (type) {
    case "file_text":
      return <FileText className="h-4 w-4 text-blue-500" />;
    case "file_image":
      return <FileImage className="h-4 w-4 text-purple-500" />;
    case "file_binary":
      return <File className="h-4 w-4 text-gray-500" />;
    case "file_datatable":
      return <Table2 className="h-4 w-4 text-green-500" />;
    case "progress":
      return <BarChart3 className="h-4 w-4 text-amber-500" />;
    case "url":
      return <LinkIcon className="h-4 w-4 text-cyan-500" />;
    case "other":
    default:
      return <Package className="h-4 w-4 text-gray-400" />;
  }
}

export function getArtifactTypeBadge(type: ArtifactType): {
  label: string;
  classes: string;
} {
  switch (type) {
    case "file_text":
      return { label: "Text File", classes: "bg-blue-100 text-blue-800" };
    case "file_image":
      return { label: "Image", classes: "bg-purple-100 text-purple-800" };
    case "file_binary":
      return { label: "Binary", classes: "bg-gray-100 text-gray-800" };
    case "file_datatable":
      return { label: "Data Table", classes: "bg-green-100 text-green-800" };
    case "progress":
      return { label: "Progress", classes: "bg-amber-100 text-amber-800" };
    case "url":
      return { label: "URL", classes: "bg-cyan-100 text-cyan-800" };
    case "other":
    default:
      return { label: "Other", classes: "bg-gray-100 text-gray-700" };
  }
}

export function getScopeBadge(scope: OwnerType): {
  label: string;
  classes: string;
} {
  switch (scope) {
    case "system":
      return { label: "System", classes: "bg-purple-100 text-purple-800" };
    case "pack":
      return { label: "Pack", classes: "bg-green-100 text-green-800" };
    case "action":
      return { label: "Action", classes: "bg-yellow-100 text-yellow-800" };
    case "sensor":
      return { label: "Sensor", classes: "bg-indigo-100 text-indigo-800" };
    case "identity":
      return { label: "Identity", classes: "bg-blue-100 text-blue-800" };
    default:
      return { label: scope, classes: "bg-gray-100 text-gray-700" };
  }
}

export function getVisibilityBadge(visibility: string): {
  label: string;
  classes: string;
} {
  if (visibility === "public") {
    return { label: "Public", classes: "text-green-700" };
  }
  return { label: "Private", classes: "text-gray-600" };
}

// ============================================================================
// Formatting helpers
// ============================================================================

export function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null || bytes === 0) return "\u2014";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function formatDate(dateString: string) {
  return new Date(dateString).toLocaleString();
}

export function formatTime(timestamp: string) {
  const date = new Date(timestamp);
  const now = new Date();
  const diff = now.getTime() - date.getTime();

  if (diff < 60000) return "just now";
  if (diff < 3600000) return `${Math.floor(diff / 60000)}m ago`;
  if (diff < 86400000) return `${Math.floor(diff / 3600000)}h ago`;
  return date.toLocaleDateString();
}

// ============================================================================
// Download helper
// ============================================================================

export async function downloadArtifact(
  artifactId: number,
  artifactRef: string,
  versionNumber?: number,
  versionId?: number,
) {
  if (versionNumber !== undefined && versionId === undefined)
    throw new Error("Content unavailable or access denied");
  const token = localStorage.getItem("access_token");
  const url = `${OpenAPI.BASE}/api/v1/artifacts/${artifactId}${versionNumber === undefined ? "" : `/versions/${versionNumber}`}/download${versionNumber === undefined ? "" : `?version_id=${versionId}`}`;

  const response = await fetch(url, {
    headers: {
      Authorization: `Bearer ${token}`,
    },
  });

  if (!response.ok) {
    throw new Error("Content unavailable or access denied");
  }

  const disposition = response.headers.get("Content-Disposition");
  let filename =
    artifactRef.replace(/\./g, "_") +
    (versionNumber === undefined ? "" : `_v${versionNumber}`) +
    ".bin";
  if (disposition) {
    const match = disposition.match(/filename="?([^"]+)"?/);
    if (match) filename = match[1];
  }

  const blob = await response.blob();
  const blobUrl = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = blobUrl;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  URL.revokeObjectURL(blobUrl);
}

export function isDownloadable(type: ArtifactType): boolean {
  return (
    type === "file_text" ||
    type === "file_image" ||
    type === "file_binary" ||
    type === "file_datatable"
  );
}
