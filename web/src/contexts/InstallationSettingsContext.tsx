/* eslint-disable react-refresh/only-export-components -- the hook and provider share this context */
import { createContext, useContext, useEffect, useState } from "react";
import type { ReactNode } from "react";
import axios from "axios";
import { OpenAPI, type ApiResponse_AuthSettingsResponse } from "@/api";
import { safeErrorSummary } from "@/lib/safe-error-summary";

type InstallationSettings = ApiResponse_AuthSettingsResponse["data"];

interface InstallationSettingsResult {
  settings: InstallationSettings | null;
  isLoading: boolean;
}

const pendingSettings = { settings: null, isLoading: true };
const InstallationSettingsContext =
  createContext<InstallationSettingsResult>(pendingSettings);
const settingsClient = axios.create({ timeout: 30000 });

export function InstallationSettingsProvider({
  children,
}: {
  children: ReactNode;
}) {
  const [result, setResult] =
    useState<InstallationSettingsResult>(pendingSettings);

  useEffect(() => {
    const controller = new AbortController();
    settingsClient
      .get<ApiResponse_AuthSettingsResponse>("/auth/settings", {
        baseURL: OpenAPI.BASE,
        signal: controller.signal,
      })
      .then(
        ({ data }) => {
          if (!controller.signal.aborted)
            setResult({ settings: data?.data ?? null, isLoading: false });
        },
        (error: unknown) => {
          if (!controller.signal.aborted) {
            console.error(
              "Failed to load installation settings:",
              safeErrorSummary(error),
            );
            setResult({ settings: null, isLoading: false });
          }
        },
      );
    return () => controller.abort();
  }, []);

  const faviconUrl = result.settings?.branding?.favicon_url;
  useEffect(() => {
    const favicon = document.querySelector<HTMLLinkElement>('link[rel="icon"]');
    if (!favicon || !faviconUrl) return;

    const originalHref = favicon.href;
    const originalType = favicon.type;
    favicon.removeAttribute("type");
    favicon.href = faviconUrl;
    return () => {
      favicon.type = originalType;
      favicon.href = originalHref;
    };
  }, [faviconUrl]);

  return (
    <InstallationSettingsContext.Provider value={result}>
      {children}
    </InstallationSettingsContext.Provider>
  );
}

export function useInstallationSettings() {
  return useContext(InstallationSettingsContext);
}
