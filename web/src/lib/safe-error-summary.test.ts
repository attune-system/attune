import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/core/ApiError";
import { safeErrorSummary } from "./safe-error-summary";

describe("safeErrorSummary", () => {
  it("classifies ApiError without retaining server or request strings", () => {
    const error = new ApiError(
      {
        method: "POST",
        url: "/auth/login",
        headers: { Authorization: "Bearer access-secret" },
        cookies: { session: "cookie-secret" },
        body: { password: "password-secret" },
      },
      {
        url: "/auth/login",
        ok: false,
        status: 401,
        statusText: "status-secret",
        body: { refresh_token: "refresh-secret" },
      },
      'Generic Error: body: {"refresh_token":"refresh-secret"}',
    );

    const summary = safeErrorSummary(error);

    expect(summary).toEqual({
      name: "HttpError",
      status: 401,
    });
    expect(JSON.stringify(summary)).not.toMatch(
      /access-secret|cookie-secret|password-secret|refresh-secret|status-secret|ApiError/,
    );
  });

  it("classifies Axios-style errors without copying attacker-controlled labels", () => {
    const error = {
      name: "name-secret",
      message: "request included login-secret",
      code: "code-secret",
      config: {
        headers: { Authorization: "Bearer access-secret" },
        data: { password: "password-secret" },
      },
      request: { cookie: "cookie-secret" },
      response: {
        status: 403,
        statusText: "status-text-secret",
        data: { token: "response-secret" },
        headers: { "set-cookie": "session-secret" },
      },
    };

    const summary = safeErrorSummary(error);

    expect(summary).toEqual({
      name: "HttpError",
      status: 403,
    });
    expect(JSON.stringify(summary)).not.toMatch(
      /name-secret|code-secret|status-text-secret|login-secret|access-secret|password-secret|cookie-secret|response-secret|session-secret/,
    );
  });

  it("does not log arbitrary thrown scalar values or error messages", () => {
    expect(safeErrorSummary("Bearer token-secret")).toEqual({
      name: "UnknownError",
    });
    expect(safeErrorSummary(new Error("token-secret"))).toEqual({
      name: "Error",
    });
  });

  it("rejects numbers that are not valid HTTP statuses", () => {
    expect(
      safeErrorSummary({
        name: "name-secret",
        code: "code-secret",
        status: 123456789,
        statusText: "status-secret",
      }),
    ).toEqual({ name: "UnknownError" });
  });
});
