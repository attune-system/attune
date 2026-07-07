import { OpenAPI } from "@/api";

const packIconResultCache = new Map<string, string | null>();
const packIconRequestCache = new Map<string, Promise<string | null>>();

export function packIconUrl(packRef: string): string {
  return `${OpenAPI.BASE}/api/v1/packs/${encodeURIComponent(packRef)}/icon`;
}

export function loadPackIconSrc(packRef: string): Promise<string | null> {
  const normalizedPackRef = packRef.trim();
  if (!normalizedPackRef) {
    return Promise.resolve(null);
  }

  if (packIconResultCache.has(normalizedPackRef)) {
    return Promise.resolve(packIconResultCache.get(normalizedPackRef) ?? null);
  }

  const inFlight = packIconRequestCache.get(normalizedPackRef);
  if (inFlight) {
    return inFlight;
  }

  const request = fetch(packIconUrl(normalizedPackRef), {
    credentials: "include",
  })
    .then(async (response) => {
      if (!response.ok) {
        packIconResultCache.set(normalizedPackRef, null);
        return null;
      }
      const blob = await response.blob();
      const objectUrl = URL.createObjectURL(blob);
      packIconResultCache.set(normalizedPackRef, objectUrl);
      return objectUrl;
    })
    .catch(() => {
      packIconResultCache.set(normalizedPackRef, null);
      return null;
    })
    .finally(() => {
      packIconRequestCache.delete(normalizedPackRef);
    });

  packIconRequestCache.set(normalizedPackRef, request);
  return request;
}

export function packRefFromComponentRef(ref?: string | null): string | null {
  if (!ref) {
    return null;
  }

  const [packRef] = ref.split(".");
  return packRef || null;
}
