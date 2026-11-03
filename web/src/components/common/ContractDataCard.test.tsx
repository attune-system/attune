import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import ContractDataCard from "./ContractDataCard";

it("renders resolved titles in position order without weakening inherited secrecy", () => {
  render(
    <ContractDataCard
      title="Stored annotations"
      contract={{
        annotated_schema: {
          type: "object",
          properties: {
            late: {
              $ref: "#/$defs/secret",
              title: "Second field",
              position: 2,
              secret: false,
            },
            early: { type: "string", title: "First field", position: 1 },
          },
          $defs: {
            secret: { type: "string", secret: true, title: "Inherited title" },
          },
        },
      }}
      values={{ late: "private-annotation-fixture", early: "Public value" }}
      emptyMessage="No values"
      maskSecrets
    />,
  );
  expect(
    screen
      .getByText("First field")
      .compareDocumentPosition(screen.getByText("Second field")) &
      Node.DOCUMENT_POSITION_FOLLOWING,
  ).not.toBe(0);
  expect(screen.getByText("[REDACTED]")).toBeInTheDocument();
  expect(
    screen.queryByText("private-annotation-fixture"),
  ).not.toBeInTheDocument();
});

afterEach(cleanup);

describe("stored contract displays", () => {
  it("rejects inherited reference targets and permits own prototype-like names", () => {
    const definitions = {};
    Object.setPrototypeOf(definitions, { constructor: { type: "string" } });
    const contract = {
      annotated_schema: { $ref: "#/$defs/constructor", $defs: definitions },
    };
    const { rerender } = render(
      <ContractDataCard
        title="Event"
        contract={contract}
        values="private-inherited-value"
        emptyMessage="Empty"
      />,
    );
    expect(screen.getByRole("alert")).toBeInTheDocument();
    expect(
      screen.queryByText("private-inherited-value"),
    ).not.toBeInTheDocument();
    Object.defineProperty(definitions, "constructor", {
      value: { type: "string" },
      enumerable: true,
    });
    rerender(
      <ContractDataCard
        title="Event"
        contract={contract}
        values="own-value"
        emptyMessage="Empty"
      />,
    );
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByText("own-value")).toBeInTheDocument();
  });

  it("resolves shared references, occurrence annotations, and nested array fields", () => {
    const contract = {
      annotated_schema: {
        $ref: "#/$defs/root",
        $defs: {
          root: {
            type: "object",
            properties: {
              addresses: { type: "array", items: { $ref: "#/$defs/address" } },
              token: {
                $ref: "#/$defs/text",
                description: "Captured token",
                secret: true,
              },
            },
          },
          address: {
            type: "object",
            properties: {
              city: { $ref: "#/$defs/text", description: "Captured city" },
            },
          },
          text: { type: ["string", "null"], description: "Base description" },
        },
      },
    };
    render(
      <ContractDataCard
        title="Event"
        contract={contract}
        values={{
          addresses: [{ city: "Oslo" }],
          token: { $attune_secret: true, redacted: true },
        }}
        emptyMessage="Empty"
      />,
    );
    expect(screen.getByText("Captured city")).toBeInTheDocument();
    expect(screen.getByText("Captured token")).toBeInTheDocument();
    expect(screen.getByText("Oslo")).toBeInTheDocument();
    expect(screen.getByText("[REDACTED]")).toBeInTheDocument();
    expect(screen.queryByText("Base description")).not.toBeInTheDocument();
  });

  it("masks secret ancestors and conservative composition markings", () => {
    const contract = {
      annotated_schema: {
        type: "object",
        properties: {
          credentials: { type: "object", secret: true },
          composed: { anyOf: [{ type: "string" }, { secret: true }] },
        },
      },
    };
    render(
      <ContractDataCard
        title="Event"
        contract={contract}
        values={{
          credentials: { privateKey: "private-value" },
          composed: "private-composed",
        }}
        emptyMessage="Empty"
        maskSecrets
      />,
    );
    expect(screen.getAllByText("[REDACTED]")).toHaveLength(2);
    expect(screen.queryByText("privateKey")).not.toBeInTheDocument();
    expect(screen.queryByText("private-value")).not.toBeInTheDocument();
    expect(screen.queryByText("private-composed")).not.toBeInTheDocument();
  });

  it("fails closed on cycles and external references without fetching definitions", () => {
    const { rerender } = render(
      <ContractDataCard
        title="Event"
        contract={{
          annotated_schema: {
            $ref: "#/$defs/loop",
            $defs: { loop: { $ref: "#/$defs/loop" } },
          },
        }}
        values={{ token: "private" }}
        emptyMessage="Empty"
      />,
    );
    expect(screen.getByRole("alert")).toBeInTheDocument();
    expect(screen.queryByText("private")).not.toBeInTheDocument();
    rerender(
      <ContractDataCard
        title="Event"
        contract={{
          annotated_schema: { $ref: "https://example.invalid/schema" },
        }}
        values="private"
        emptyMessage="Empty"
      />,
    );
    expect(screen.getByRole("alert")).toBeInTheDocument();
    expect(screen.queryByText("private")).not.toBeInTheDocument();
  });
});
