export type SafeErrorSummary = Readonly<{
  name: "Error" | "HttpError" | "UnknownError";
  status?: number;
}>;

function readHttpStatus(value: unknown): number | undefined {
  return typeof value === "number" &&
    Number.isInteger(value) &&
    value >= 100 &&
    value <= 599
    ? value
    : undefined;
}

export function safeErrorSummary(error: unknown): SafeErrorSummary {
  if (typeof error !== "object" || error === null) {
    return { name: "UnknownError" };
  }

  try {
    const directStatus =
      "status" in error ? readHttpStatus(error.status) : undefined;
    const response =
      "response" in error &&
      typeof error.response === "object" &&
      error.response !== null
        ? error.response
        : undefined;
    const responseStatus =
      response && "status" in response
        ? readHttpStatus(response.status)
        : undefined;
    const status = directStatus ?? responseStatus;

    if (status !== undefined) {
      return { name: "HttpError", status };
    }

    return { name: error instanceof Error ? "Error" : "UnknownError" };
  } catch {
    return { name: "UnknownError" };
  }
}
