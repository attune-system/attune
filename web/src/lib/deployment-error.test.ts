import { expect, it } from "vitest";
import { deploymentErrorText } from "./deployment-error";

it("shows authored source paths, adjustments and install identity", () => {
  const message = deploymentErrorText(
    {
      body: {
        error: "Pack metadata is invalid",
        code: "PACK_METADATA_INVALID",
        details: {
          stage: "metadata_validation",
          outcome: "active_release_unchanged",
          install_id: 42,
          diagnostics: [
            {
              path: "actions/send.yaml",
              metadata_path: "/timeout_seconds",
              message: "Expected a positive integer",
              hint: "Use seconds",
            },
          ],
        },
      },
    },
    "Request failed",
  );
  expect(message).toContain("actions/send.yaml at /timeout_seconds");
  expect(message).toContain("Use seconds");
  expect(message).toContain("Install 42");
});

it("does not display raw bodies or internal fallback errors", () => {
  expect(
    deploymentErrorText(
      { body: "SECRET_CANARY", message: "Generic Error: SECRET_CANARY" },
      "Request failed",
    ),
  ).toBe("Request failed");
  expect(
    deploymentErrorText(
      {
        body: {
          error: "Invalid metadata",
          details: {
            secret: "SECRET_CANARY",
            diagnostics: [{ message: { secret: "SECRET_CANARY" } }],
          },
        },
      },
      "Request failed",
    ),
  ).not.toContain("SECRET_CANARY");
});
