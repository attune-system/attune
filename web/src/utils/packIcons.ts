import { OpenAPI } from "@/api";

export function packIconUrl(packRef: string): string {
  return `${OpenAPI.BASE}/api/v1/packs/${encodeURIComponent(packRef)}/icon`;
}

export function packRefFromComponentRef(ref?: string | null): string | null {
  if (!ref) {
    return null;
  }

  const [packRef] = ref.split(".");
  return packRef || null;
}
