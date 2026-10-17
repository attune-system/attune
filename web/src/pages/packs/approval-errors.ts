import { ApiError } from "@/api/core/ApiError";
import { deploymentErrorText } from "@/lib/deployment-error";

export function approvalErrorText(error: unknown): string {
  // Do not forward response bodies, diagnostic source paths, or transport text.
  // Coverage failures can contain private refs or credential-bearing context.
  const status = error instanceof ApiError ? error.status : 0;
  const message =
    status === 403
      ? "Approval denied. Current covering grants or permission-administration authority are required. Your draft is unchanged."
      : status === 404 || status === 409
        ? "The approval or retained target is unavailable or changed. Your draft is unchanged; review authorized metadata before retrying."
        : "Approval request failed. Check your authority, retained target, grants and expiry. Your draft is unchanged.";
  return deploymentErrorText({ body: { message } }, message);
}
