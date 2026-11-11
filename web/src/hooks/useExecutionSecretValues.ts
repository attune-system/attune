import { useEffect, useRef, useState } from "react";
import {
  ExecutionsService,
  type CurrentUserResponse,
  type ExecutionResponse,
} from "@/api";
import { useAuth } from "@/contexts/AuthContext";
import { hasPermission } from "@/lib/permissions";
import { safeErrorSummary } from "@/lib/safe-error-summary";

type Disclosure = {
  execution: ExecutionResponse;
  user: CurrentUserResponse | null;
  pending: boolean;
  values?: ExecutionResponse;
  error?: string;
};

/** Explicit, view-local disclosure; plaintext never enters the query cache. */
export function useExecutionSecretValues(execution?: ExecutionResponse) {
  const { user } = useAuth();
  const canReveal = hasPermission(user, "executions", "decrypt");
  const [disclosure, setDisclosure] = useState<Disclosure>();
  const request = useRef(0);

  if (
    disclosure &&
    (disclosure.execution !== execution ||
      disclosure.user !== user ||
      !canReveal)
  ) {
    setDisclosure(undefined);
  }

  // Invalidate in-flight responses on navigation, redacted-data refresh, auth
  // changes, or unmount. Rendering also checks scope before displaying values.
  useEffect(() => {
    return () => {
      request.current += 1;
    };
  }, [execution, user]);

  const current =
    canReveal &&
    disclosure?.execution === execution &&
    disclosure?.user === user
      ? disclosure
      : undefined;

  function hide() {
    request.current += 1;
    setDisclosure(undefined);
  }

  async function reveal() {
    if (!execution || !canReveal) return;
    const requestId = ++request.current;
    setDisclosure({ execution, user, pending: true });
    try {
      const response = await ExecutionsService.getExecution({
        id: execution.id,
        includeSecretValues: true,
      });
      if (request.current === requestId) {
        setDisclosure({
          execution,
          user,
          pending: false,
          values: response.data,
        });
      }
    } catch (error) {
      if (request.current === requestId) {
        setDisclosure({
          execution,
          user,
          pending: false,
          error:
            safeErrorSummary(error).status === 403
              ? "You do not have permission to reveal secret values for this execution."
              : "Secret values could not be loaded. Please try again.",
        });
      }
    }
  }

  return {
    canReveal,
    isRevealed: !!current?.values,
    isPending: current?.pending ?? false,
    error: current?.error,
    config: current?.values?.config ?? execution?.config,
    result: current?.values?.result ?? execution?.result,
    channels: current?.values?.revealed_output_channels,
    reveal,
    hide,
  };
}
