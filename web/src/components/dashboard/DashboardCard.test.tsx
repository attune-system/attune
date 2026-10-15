import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type {
  DashboardCardSpec,
  DashboardSourceResult,
} from "@/types/dashboard";
import { DashboardCard } from "./DashboardCard";

const card: DashboardCardSpec = {
  id: "execution-count",
  title: "Execution count",
  source: "executions",
  visualization: { type: "stat", value_field: "value", format: "integer" },
  position: { lg: { x: 0, y: 0, w: 4, h: 3 } },
};

function source(
  overrides: Partial<DashboardSourceResult> = {},
): DashboardSourceResult {
  return {
    source_id: card.source,
    source_type: "execution_count",
    status: "ok",
    data: [{ value: 987 }],
    meta: {
      authorization_mode: "operator_global",
      freshness_mode: "raw_only",
      aggregate_watermark: null,
      read_coverage: null,
      cache_hit: false,
      bucket_size: null,
      truncated: false,
      unit_hints: {},
      ordering: ["value"],
      authorized_refs: null,
    },
    error: null,
    ...overrides,
  };
}

describe("dashboard source-state rendering", () => {
  it("keeps partial data visible, including a zero-valued statistic", () => {
    render(
      <DashboardCard
        card={card}
        source={source({ status: "partial", data: [{ value: 0 }] })}
      />,
    );

    expect(screen.getByText("Partial")).toBeVisible();
    expect(screen.getByText("0")).toBeVisible();
    expect(
      screen.queryByText("No values in selected range."),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("Error")).not.toBeInTheDocument();
    expect(screen.queryByText("Forbidden")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Retry" }),
    ).not.toBeInTheDocument();
  });

  it("renders available table rows for a partial source", () => {
    render(
      <DashboardCard
        card={{ ...card, visualization: { type: "table", format: "integer" } }}
        source={source({
          status: "partial",
          data: [
            { queue: "priority", value: 2 },
            { queue: "standard", value: 4 },
          ],
        })}
      />,
    );

    const table = within(screen.getByRole("table"));
    expect(screen.getByText("Partial")).toBeVisible();
    expect(table.getAllByRole("row")).toHaveLength(3);
    expect(table.getByRole("columnheader", { name: "queue" })).toBeVisible();
    expect(table.getByRole("cell", { name: "priority" })).toBeVisible();
    expect(table.getByRole("cell", { name: "2" })).toBeVisible();
    expect(table.getByRole("cell", { name: "standard" })).toBeVisible();
    expect(table.getByRole("cell", { name: "4" })).toBeVisible();
  });

  it("shows the forbidden fallback without rendering supplied data or offering retry", () => {
    render(
      <DashboardCard
        card={card}
        source={source({ status: "forbidden" })}
        onRetry={vi.fn()}
      />,
    );

    expect(screen.getByText("Forbidden")).toBeVisible();
    expect(
      screen.getByText("You are not authorized to view this source."),
    ).toBeVisible();
    expect(screen.queryByText("987")).not.toBeInTheDocument();
    expect(screen.queryByText("Error")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Retry" }),
    ).not.toBeInTheDocument();
  });

  it("uses the server's forbidden message but never treats it as a retryable error", () => {
    render(
      <DashboardCard
        card={card}
        source={source({
          status: "forbidden",
          error: {
            code: "source_forbidden",
            message: "Queue access is required.",
            retryable: true,
            details: null,
          },
        })}
        onRetry={vi.fn()}
      />,
    );

    expect(screen.getByText("Forbidden")).toBeVisible();
    expect(screen.getByText("Queue access is required.")).toBeVisible();
    expect(screen.queryByText("987")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Retry" }),
    ).not.toBeInTheDocument();
  });

  it("shows error details and invokes retry for a retryable source failure", async () => {
    const onRetry = vi.fn();
    render(
      <DashboardCard
        card={card}
        source={source({
          status: "error",
          error: {
            code: "source_unavailable",
            message: "Source temporarily unavailable.",
            retryable: true,
            details: null,
          },
        })}
        onRetry={onRetry}
      />,
    );

    expect(screen.getByText("Error")).toBeVisible();
    expect(screen.getByText("Source temporarily unavailable.")).toBeVisible();
    expect(screen.getByText("Code: source_unavailable")).toBeVisible();
    expect(screen.queryByText("987")).not.toBeInTheDocument();
    expect(screen.queryByText("Forbidden")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalledOnce();
  });

  it.each([
    { retryable: false, provideHandler: true },
    { retryable: true, provideHandler: false },
  ])(
    "does not offer retry when retryable=$retryable and provideHandler=$provideHandler",
    ({ retryable, provideHandler }) => {
      render(
        <DashboardCard
          card={card}
          source={source({
            status: "error",
            error: {
              code: "source_failed",
              message: "Source failed.",
              retryable,
              details: null,
            },
          })}
          onRetry={provideHandler ? vi.fn() : undefined}
        />,
      );

      expect(screen.getByText("Error")).toBeVisible();
      expect(screen.getByText("Source failed.")).toBeVisible();
      expect(
        screen.queryByRole("button", { name: "Retry" }),
      ).not.toBeInTheDocument();
    },
  );

  it("uses the error fallback when no source error detail is available", () => {
    render(
      <DashboardCard
        card={card}
        source={source({ status: "error", error: null })}
      />,
    );

    expect(screen.getByText("Error")).toBeVisible();
    expect(screen.getByText("Failed to load source data.")).toBeVisible();
    expect(screen.queryByText("987")).not.toBeInTheDocument();
    expect(screen.queryByText(/^Code:/)).not.toBeInTheDocument();
  });

  it("distinguishes invalid source parameters from a runtime error", () => {
    render(
      <DashboardCard
        card={card}
        source={source({
          status: "invalid",
          error: {
            code: "invalid_params",
            message: "Select a valid queue.",
            retryable: false,
            details: null,
          },
        })}
        onRetry={vi.fn()}
      />,
    );

    expect(screen.getByText("Invalid")).toBeVisible();
    expect(screen.getByText("Select a valid queue.")).toBeVisible();
    expect(screen.queryByText("Error")).not.toBeInTheDocument();
    expect(screen.queryByText("987")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Retry" }),
    ).not.toBeInTheDocument();
  });

  it("treats a missing source result as a retryable error, not empty data", async () => {
    const onRetry = vi.fn();
    render(<DashboardCard card={card} onRetry={onRetry} />);

    expect(screen.getByText("Error")).toBeVisible();
    expect(
      screen.getByText("Source result missing from response."),
    ).toBeVisible();
    expect(screen.getByText("Code: missing_source")).toBeVisible();
    expect(
      screen.queryByText("No data available for current filters."),
    ).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalledOnce();
  });

  it("distinguishes an empty source from an error or forbidden source", () => {
    render(
      <DashboardCard
        card={card}
        source={source({ status: "empty", data: null })}
        onRetry={vi.fn()}
      />,
    );

    expect(screen.getByText("Empty")).toBeVisible();
    expect(
      screen.getByText("No data available for current filters."),
    ).toBeVisible();
    expect(screen.queryByText("Error")).not.toBeInTheDocument();
    expect(screen.queryByText("Forbidden")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Retry" }),
    ).not.toBeInTheDocument();
  });

  it("renders mixed partial, forbidden, and error cards independently", async () => {
    const onRetry = vi.fn();
    render(
      <>
        <DashboardCard
          card={{
            ...card,
            id: "partial",
            title: "Partial executions",
            source: "partial_source",
          }}
          source={source({
            source_id: "partial_source",
            status: "partial",
            data: [{ value: 0 }],
          })}
          onRetry={onRetry}
        />
        <DashboardCard
          card={{
            ...card,
            id: "forbidden",
            title: "Restricted executions",
            source: "forbidden_source",
          }}
          source={source({
            source_id: "forbidden_source",
            status: "forbidden",
          })}
          onRetry={onRetry}
        />
        <DashboardCard
          card={{
            ...card,
            id: "error",
            title: "Failed executions",
            source: "error_source",
          }}
          source={source({
            source_id: "error_source",
            status: "error",
            error: {
              code: "source_unavailable",
              message: "Source temporarily unavailable.",
              retryable: true,
              details: null,
            },
          })}
          onRetry={onRetry}
        />
      </>,
    );

    const cards = screen.getAllByRole("article");
    expect(cards).toHaveLength(3);
    const partial = within(cards[0]);
    const forbidden = within(cards[1]);
    const error = within(cards[2]);
    expect(partial.getByText("Partial")).toBeVisible();
    expect(partial.getByText("0")).toBeVisible();
    expect(
      partial.queryByRole("button", { name: "Retry" }),
    ).not.toBeInTheDocument();
    expect(forbidden.getByText("Forbidden")).toBeVisible();
    expect(
      forbidden.getByText("You are not authorized to view this source."),
    ).toBeVisible();
    expect(forbidden.queryByText("987")).not.toBeInTheDocument();
    expect(
      forbidden.queryByRole("button", { name: "Retry" }),
    ).not.toBeInTheDocument();
    expect(error.getByText("Error")).toBeVisible();
    expect(error.getByText("Source temporarily unavailable.")).toBeVisible();
    expect(error.queryByText("987")).not.toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Retry" })).toHaveLength(1);
    await userEvent.click(error.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalledOnce();
  });
});
